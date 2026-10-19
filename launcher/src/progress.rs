//! Shared progress/log sink: the worker threads write, the UI (or the
//! headless printer) reads snapshots.
use std::{
    fs::File,
    io::Write,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

const MAX_LOG_LINES: usize = 4000;

#[derive(Clone)]
pub struct Reporter(Arc<Inner>);

struct Inner {
    cancel: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    stage: Mutex<Stage>,
    logs: Mutex<Vec<String>>,
    file: Mutex<Option<File>>,
    echo: bool,
    speed: Mutex<Speed>,
}

#[derive(Clone, Default)]
struct Stage {
    index: usize,
    count: usize,
    title: String,
}

struct Speed {
    at: Instant,
    bytes: u64,
    bps: f64,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub stage_index: usize,
    pub stage_count: usize,
    pub title: String,
    pub done: u64,
    pub total: u64,
    pub bytes_per_second: f64,
}

pub struct Cancelled;

impl Reporter {
    /// `echo` also prints log lines to stdout (headless mode).
    pub fn new(log_file: Option<&Path>, echo: bool) -> Self {
        let file = log_file.and_then(|path| {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
        });
        Self(Arc::new(Inner {
            cancel: AtomicBool::new(false),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            stage: Mutex::new(Stage::default()),
            logs: Mutex::new(Vec::new()),
            file: Mutex::new(file),
            echo,
            speed: Mutex::new(Speed {
                at: Instant::now(),
                bytes: 0,
                bps: 0.0,
            }),
        }))
    }

    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        if self.0.echo {
            println!("{line}");
        }
        if let Some(file) = self.0.file.lock().unwrap().as_mut() {
            let _ = writeln!(file, "{line}");
        }
        let mut logs = self.0.logs.lock().unwrap();
        logs.push(line);
        if logs.len() > MAX_LOG_LINES {
            let excess = logs.len() - MAX_LOG_LINES;
            logs.drain(..excess);
        }
    }

    pub fn logs(&self) -> Vec<String> {
        self.0.logs.lock().unwrap().clone()
    }

    pub fn clear_logs(&self) {
        self.0.logs.lock().unwrap().clear();
    }

    /// Start a stage; resets the byte counters.
    pub fn stage(&self, index: usize, count: usize, title: &str) {
        *self.0.stage.lock().unwrap() = Stage {
            index,
            count,
            title: title.into(),
        };
        self.0.done.store(0, Ordering::Relaxed);
        self.0.total.store(0, Ordering::Relaxed);
        *self.0.speed.lock().unwrap() = Speed {
            at: Instant::now(),
            bytes: 0,
            bps: 0.0,
        };
        self.log(format!("[{index}/{count}] {title}"));
    }

    pub fn set_total(&self, total: u64) {
        self.0.total.store(total, Ordering::Relaxed);
    }

    pub fn add_done(&self, bytes: u64) {
        self.0.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn sub_done(&self, bytes: u64) {
        let _ = self
            .0
            .done
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(v.saturating_sub(bytes)));
    }

    pub fn cancel(&self) {
        self.0.cancel.store(true, Ordering::Relaxed);
    }

    pub fn reset_cancel(&self) {
        self.0.cancel.store(false, Ordering::Relaxed);
    }

    pub fn check_cancel(&self) -> Result<(), Cancelled> {
        if self.0.cancel.load(Ordering::Relaxed) {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let stage = self.0.stage.lock().unwrap().clone();
        let done = self.0.done.load(Ordering::Relaxed);
        let mut speed = self.0.speed.lock().unwrap();
        let elapsed = speed.at.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            let instant = done.saturating_sub(speed.bytes) as f64 / elapsed;
            speed.bps = if speed.bps == 0.0 {
                instant
            } else {
                speed.bps * 0.6 + instant * 0.4
            };
            speed.at = Instant::now();
            speed.bytes = done;
        }
        Snapshot {
            stage_index: stage.index,
            stage_count: stage.count,
            title: stage.title,
            done,
            total: self.0.total.load(Ordering::Relaxed),
            bytes_per_second: speed.bps,
        }
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
