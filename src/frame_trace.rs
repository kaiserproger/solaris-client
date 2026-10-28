//! Opt-in frame pacing trace: `SOLARIS_CLIENT_FRAME_TRACE=<file>` writes one
//! tab-separated line per submitted frame, read from the same render metrics
//! the observation publishes. Unset (the default) costs nothing.

use serde_json::Value;
use std::{
    fmt::Write as _,
    fs::File,
    io::{LineWriter, Write},
    sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc::{self, SyncSender}},
    thread::JoinHandle,
    time::Instant,
};

const COLUMNS: [&str; 20] = [
    "submitted_frame_interval_ms",
    "frame_wall_ms",
    "surface_acquire_ms",
    "present_call_ms",
    "cpu_client_draw_ms",
    "cpu_session_prepare_frame_ms",
    "cpu_client_advance_ms",
    "cpu_client_publish_ms",
    "cpu_upload_ms",
    "terrain_uploaded_chunks",
    "present_mode",
    "cpu_visibility_sort_ms",
    "cpu_draw_encode_ms",
    "present_thread_cpu_ms",
    "collect_completed_ms",
    "encoder_finish_ms",
    "queue_submit_ms",
    "render_thread_cpu_ms",
    "gpu_timestamps",
    "timestamp_missed_samples",
];

const QUEUED_FRAMES: usize = 4096;

pub(crate) struct FrameTrace {
    out: Option<SyncSender<String>>,
    writer: Option<JoinHandle<std::io::Result<()>>>,
    dropped: Arc<AtomicU64>,
    start: Instant,
    last_frame: Option<String>,
    previous_record_ms: f64,
    skipped_draws: u64,
    skipped_draw_ms: f64,
    skipped_advance_ms: f64,
}

impl FrameTrace {
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os("SOLARIS_CLIENT_FRAME_TRACE")?;
        let out = match File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("frame trace {}: {error}", path.to_string_lossy());
                return None;
            }
        };
        match Self::new(out) {
            Ok(trace) => Some(trace),
            Err(error) => {
                eprintln!("frame trace {}: {error}", path.to_string_lossy());
                None
            }
        }
    }

    fn new(out: File) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<String>(QUEUED_FRAMES);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer_dropped = dropped.clone();
        let writer = std::thread::Builder::new().name("frame-trace".into()).spawn(move || {
            let mut out = LineWriter::new(out);
            writeln!(out, "t_ms\tframe_id\t{}\tlook_events\tprevious_trace_record_ms\tskipped_draw_attempts\tskipped_draw_ms\tskipped_advance_ms", COLUMNS.join("\t"))?;
            for line in receiver {
                out.write_all(line.as_bytes())?;
            }
            writeln!(out, "# trace_complete\tdropped_rows={}", writer_dropped.load(Ordering::Relaxed))?;
            out.flush()
        })?;
        Ok(Self {
            out: Some(sender), writer: Some(writer), dropped,
            start: Instant::now(), last_frame: None, previous_record_ms: 0.,
            skipped_draws: 0, skipped_draw_ms: 0., skipped_advance_ms: 0.,
        })
    }

    /// Records `render` once per submitted frame; skipped draws repeat the id.
    pub(crate) fn record(&mut self, render: &Value, look_events: u32) {
        let record_start = Instant::now();
        let frame = render["submitted_frame_id"].as_str();
        if frame.is_none() {
            return;
        }
        if frame == self.last_frame.as_deref() {
            // App resets owner timings after every draw attempt, including a
            // renderer refusal at the in-flight limit. Retain that lost work
            // until the next submission instead of silently dropping it.
            self.skipped_draws += 1;
            self.skipped_draw_ms += render["cpu_client_draw_ms"].as_f64().unwrap_or(0.);
            self.skipped_advance_ms += render["cpu_client_advance_ms"].as_f64().unwrap_or(0.);
            return;
        }
        self.last_frame = frame.map(str::to_owned);
        let mut line = format!(
            "{:.3}\t{}",
            self.start.elapsed().as_secs_f64() * 1000.,
            frame.unwrap_or_default()
        );
        for column in COLUMNS {
            line.push('\t');
            match &render[column] {
                Value::Number(n) => line.push_str(&n.to_string()),
                Value::String(s) => line.push_str(s),
                Value::Object(_) => {
                    let _ = write!(line, "{}", render[column]);
                }
                _ => line.push('-'),
            }
        }
        let _ = writeln!(line, "\t{look_events}\t{}\t{}\t{}\t{}",
            self.previous_record_ms, self.skipped_draws, self.skipped_draw_ms, self.skipped_advance_ms);
        if self.out.as_ref().is_some_and(|out| out.try_send(line).is_err()) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        // Serialization/enqueue belongs to the NEXT submission interval.
        self.previous_record_ms = record_start.elapsed().as_secs_f64() * 1000.;
        self.skipped_draws = 0;
        self.skipped_draw_ms = 0.;
        self.skipped_advance_ms = 0.;
    }
}

impl Drop for FrameTrace {
    fn drop(&mut self) {
        // Only shutdown waits for disk. Closing the sender drains accepted rows.
        self.out.take();
        if let Some(writer) = self.writer.take() {
            match writer.join() {
                Ok(Ok(())) => {},
                Ok(Err(error)) => eprintln!("frame trace incomplete: {error}"),
                Err(_) => eprintln!("frame trace incomplete: writer panicked"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trace_preserves_completed_gpu_frame_identity_and_draw_phases() {
        let path = std::env::temp_dir().join(format!("solaris-frame-trace-{}.tsv", std::process::id()));
        let mut trace = FrameTrace::new(File::create(&path).unwrap()).unwrap();
        let gpu = json!({"frame_id":"39","passes_ms":{"opaque":2.5},"total_ms":2.5});
        let mut render = json!({
            "submitted_frame_id":"42",
            "gpu_timestamps":gpu,
            "queue_submit_ms":51.0,
            "render_thread_cpu_ms":3.0,
            "cpu_client_draw_ms":7.0,
            "cpu_client_advance_ms":11.0
        });
        trace.record(&render, 2);
        trace.record(&render, 4);
        trace.previous_record_ms = 321.;
        render["submitted_frame_id"] = json!("43");
        trace.record(&render, 1);
        render["submitted_frame_id"] = json!("44");
        trace.record(&render, 1);
        drop(trace);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(text.ends_with("# trace_complete\tdropped_rows=0\n"));
        let lines: Vec<_> = text.lines().skip(1).filter(|line| !line.starts_with('#')).collect();
        assert_eq!(lines.len(), 3);
        let cells: Vec<_> = lines[0].split('\t').collect();
        assert_eq!(cells.len(), COLUMNS.len() + 7);
        let cell = |name| cells[2 + COLUMNS.iter().position(|column| *column == name).unwrap()];
        assert_eq!(serde_json::from_str::<Value>(cell("gpu_timestamps")).unwrap(), gpu);
        assert_eq!(cell("queue_submit_ms"), "51.0");
        assert_eq!(cell("render_thread_cpu_ms"), "3.0");
        assert_eq!(cells[1], "42");
        let second: Vec<_> = lines[1].split('\t').collect();
        assert_eq!(&second[COLUMNS.len() + 3..], &["321", "1", "7", "11"]);
        let third: Vec<_> = lines[2].split('\t').collect();
        assert_eq!(&third[COLUMNS.len() + 4..], &["0", "0", "0"]);
    }

    #[cfg(unix)]
    #[test]
    fn stalled_trace_sink_never_blocks_frame_recording() {
        use std::{io::{BufRead, BufReader, Read}, os::{fd::OwnedFd, unix::net::UnixStream}, sync::mpsc, time::Duration};
        let (mut producer, mut consumer) = UnixStream::pair().unwrap();
        let mut trace = FrameTrace::new(File::from(OwnedFd::from(producer.try_clone().unwrap()))).unwrap();
        // Let the asynchronous header finish before changing descriptor flags.
        BufReader::new(consumer.try_clone().unwrap()).read_line(&mut String::new()).unwrap();
        // Fill the actual sink: the next write cannot complete until we drain it.
        producer.set_nonblocking(true).unwrap();
        loop {
            match producer.write(&[b'x'; 8192]) {
                Ok(_) => {},
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("{error}"),
            }
        }
        producer.set_nonblocking(false).unwrap();
        drop(producer);
        let (sent, received) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            for frame in 1..=QUEUED_FRAMES * 2 {
                trace.record(&json!({"submitted_frame_id":frame.to_string()}), 0);
            }
            sent.send(()).unwrap();
            drop(trace);
        });
        let recorded = received.recv_timeout(Duration::from_secs(2));
        // Always release the blocked writer before asserting, including on old code.
        let mut bytes = Vec::new();
        consumer.read_to_end(&mut bytes).unwrap();
        owner.join().unwrap();
        assert!(recorded.is_ok(), "a blocked trace sink stalled the frame owner");
        let text = String::from_utf8(bytes).unwrap();
        let footer = text.lines().last().unwrap();
        let lost: u64 = footer.strip_prefix("# trace_complete\tdropped_rows=").unwrap().parse().unwrap();
        assert!(lost > 0, "overflow must explicitly invalidate the trace");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn trace_writer_reports_sink_failure() {
        let sink = File::options().write(true).open("/dev/full").unwrap();
        let mut trace = FrameTrace::new(sink).unwrap();
        trace.record(&json!({"submitted_frame_id":"1"}), 0);
        trace.out.take();
        assert!(trace.writer.take().unwrap().join().unwrap().is_err());
    }
}
