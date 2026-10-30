//! Opt-in frame pacing trace: `SOLARIS_CLIENT_FRAME_TRACE=<file>` writes one
//! tab-separated line per submitted frame, read from the same render metrics
//! the observation publishes. Unset (the default) costs nothing.

use std::collections::BTreeMap;
use serde_json::Value;
use std::{
    fmt::Write as _,
    fs::File,
    io::{BufRead, BufReader, LineWriter, Write},
    sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc::{self, SyncSender}},
    thread::JoinHandle,
    time::Instant,
};

const COLUMNS: [&str; 30] = [
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
    "cpu_client_drain_ms",
    "cpu_client_tasks_ms",
    "cpu_client_sync_window_ms",
    "cpu_session_tick_ms",
    "cpu_session_events_ms",
    "cpu_session_tick_detail_ms",
    "cpu_terrain_update_ms",
    "cpu_entity_extract_ms",
    "cpu_ui_geometry_ms",
    "terrain_mesh_cpu_total_ms",
];

const QUEUED_FRAMES: usize = 4096;

#[derive(Default)]
struct Summary {
    frames: u64,
    complete: bool,
    dropped_rows: Option<u64>,
    last_gpu_frame: Option<String>,
    timings: BTreeMap<String, (f64, u64, f64)>,
}

impl Summary {
    fn add(&mut self, key: &str, value: f64) {
        if value.is_finite() && value >= 0. {
            let entry = self.timings.entry(key.to_owned()).or_default();
            entry.0 += value;
            entry.1 += 1;
            entry.2 = entry.2.max(value);
        }
    }

    fn line(&mut self, line: &str) {
        if let Some(dropped) = line.strip_prefix("# trace_complete\tdropped_rows=") {
            self.complete = true;
            self.dropped_rows = dropped.trim().parse().ok();
            return;
        }
        if line.starts_with('#') || line.starts_with("t_ms\t") {
            return;
        }
        let cells: Vec<_> = line.trim_end().split('\t').collect();
        if cells.len() != COLUMNS.len() + 7 || cells[1].parse::<u64>().is_err() {
            return;
        }
        self.frames += 1;
        for (index, name) in COLUMNS.iter().enumerate() {
            if name.ends_with("_ms") && *name != "terrain_mesh_cpu_total_ms"
                && let Ok(value) = cells[index + 2].parse::<f64>() {
                self.add(name, value);
            }
        }
        if let Some(index) = COLUMNS.iter().position(|name| *name == "gpu_timestamps")
            && let Ok(gpu) = serde_json::from_str::<Value>(cells[index + 2])
            && let Some(frame) = gpu["frame_id"].as_str()
            && self.last_gpu_frame.as_deref() != Some(frame) {
            self.last_gpu_frame = Some(frame.to_owned());
            if let Some(passes) = gpu["passes_ms"].as_object() {
                for (name, value) in passes {
                    if let Some(value) = value.as_f64() {
                        self.add(&format!("gpu_{name}_ms"), value);
                    }
                }
            }
        }
    }

    fn value(&self) -> Value {
        let timings: BTreeMap<_, _> = self.timings.iter().map(|(name, (sum, count, max))|
            (name, serde_json::json!({"mean_ms":sum / *count as f64,"max_ms":max,"samples":count})))
            .collect();
        serde_json::json!({"frames":self.frames,"complete":self.complete,
            "dropped_rows":self.dropped_rows,"timings":timings})
    }
}

pub(crate) fn summarize(path: &std::path::Path) -> Result<Value, String> {
    let file = File::open(path).map_err(|e| format!("opening profile {}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut summary = Summary::default();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|e| format!("reading profile {}: {e}", path.display()))? == 0 {
            break;
        }
        if !line.ends_with('\n') {
            break;
        }
        summary.line(&line);
    }
    if summary.frames == 0 {
        return Err(format!("profile {} contains no complete frame rows", path.display()));
    }
    Ok(summary.value())
}

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
    needs_metadata: bool,
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
            Ok(mut trace) => {
                trace.needs_metadata = true;
                Some(trace)
            },
            Err(error) => {
                eprintln!("frame trace {}: {error}", path.to_string_lossy());
                None
            }
        }
    }

    fn new(out: File) -> std::io::Result<Self> {
        Self::with_header(out, serde_json::json!({"version": env!("CARGO_PKG_VERSION")}))
    }

    fn with_header(out: File, header: Value) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<String>(QUEUED_FRAMES);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer_dropped = dropped.clone();
        let writer = std::thread::Builder::new().name("frame-trace".into()).spawn(move || {
            let mut out = LineWriter::new(out);
            writeln!(out, "# solaris_frame_profile\t{header}")?;
            writeln!(out, "t_ms\tframe_id\t{}\tlook_events\tprevious_trace_record_ms\tskipped_draw_attempts\tskipped_draw_ms\tskipped_advance_ms", COLUMNS.join("\t"))?;
            let mut summary = Summary::default();
            for line in receiver {
                summary.line(&line);
                out.write_all(line.as_bytes())?;
            }
            summary.complete = true;
            summary.dropped_rows = Some(writer_dropped.load(Ordering::Relaxed));
            writeln!(out, "# summary\t{}", summary.value())?;
            writeln!(out, "# trace_complete\tdropped_rows={}", writer_dropped.load(Ordering::Relaxed))?;
            out.flush()
        })?;
        Ok(Self {
            out: Some(sender), writer: Some(writer), dropped,
            start: Instant::now(), last_frame: None, previous_record_ms: 0.,
            skipped_draws: 0, skipped_draw_ms: 0., skipped_advance_ms: 0.,
            needs_metadata: false,
        })
    }

    fn metadata(&mut self, header: &Value) {
        if self.out.as_ref().is_some_and(|out|
            out.try_send(format!("# metadata\t{header}\n")).is_ok())
        {
            self.needs_metadata = false;
        }
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

    fn finish(mut self) -> std::io::Result<()> {
        self.out.take();
        match self.writer.take().expect("frame trace writer").join() {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::other("frame trace writer panicked")),
        }
    }
}

pub(crate) fn is_toggle_key(code: winit::keyboard::KeyCode, pressed: bool, repeat: bool, screen: &str) -> bool {
    code == winit::keyboard::KeyCode::F10 && pressed && !repeat && screen == "game"
}

fn header(app: &crate::App) -> Result<Value, String> {
    let size = app.window.as_ref().ok_or("window unavailable")?.inner_size();
    let adapter = app.renderer.as_ref().ok_or("renderer unavailable")?.metrics()["adapter"].clone();
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"), "adapter": adapter,
        "resolution": [size.width, size.height],
        "graphics_preset": app.session.native_setting("graphics_preset"),
        "render_distance": app.session.render_distance()
    }))
}

pub(crate) fn supply_metadata(app: &mut crate::App) {
    if app.frame_trace.as_ref().is_some_and(|trace| trace.needs_metadata)
        && let Ok(header) = header(app)
        && let Some(trace) = &mut app.frame_trace {
        trace.metadata(&header);
    }
}

/// Start or stop a player-requested profile. The writer is joined off the
/// event-loop thread; completion is reported only after the file is flushed.
pub(crate) fn toggle(app: &mut crate::App) {
    if app.profile_save.is_some() {
        notice(app, "The previous profile is still saving.");
        return;
    }
    if let Some(trace) = app.frame_trace.take() {
        let Some(path) = app.profile_path.take() else {
            app.frame_trace = Some(trace);
            notice(app, "An environment frame trace is already running.");
            return;
        };
        app.session.enable_scene_profile(false);
        let (tx, rx) = mpsc::sync_channel(1);
        let wake = app.shared.owner_wake.clone();
        std::thread::spawn(move || {
            let result = trace.finish().map_err(|error| error.to_string());
            let _ = tx.send((path, result));
            wake();
        });
        app.profile_save = Some(rx);
        notice(app, "Saving performance profile...");
        return;
    }
    let result = (|| -> Result<_, String> {
        let dir = crate::singleplayer::data_root()?.join("profiles");
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let path = dir.join(format!("frame-{}.tsv", uuid::Uuid::new_v4()));
        let file = File::options().write(true).create_new(true).open(&path)
            .map_err(|e| format!("creating {}: {e}", path.display()))?;
        let header = header(app)?;
        let trace = FrameTrace::with_header(file, header).map_err(|e| e.to_string())?;
        Ok((path, trace))
    })();
    match result {
        Ok((path, trace)) => {
            app.profile_path = Some(path);
            app.session.enable_scene_profile(true);
            app.frame_trace = Some(trace);
            notice(app, "Performance recording started. Press F10 again to save.");
        }
        Err(error) => notice(app, &format!("Could not start performance recording: {error}")),
    }
}

pub(crate) fn complete(app: &mut crate::App) {
    let Some(receiver) = &app.profile_save else { return; };
    let result = match receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => {
            app.profile_save = None;
            notice(app, "Could not save performance profile: writer stopped.");
            return;
        }
    };
    app.profile_save = None;
    let message = match result {
        (path, Ok(())) => format!("Saved performance profile: {}", path.display()),
        (path, Err(error)) => format!("Could not save performance profile {}: {error}", path.display()),
    };
    notice(app, &message);
}

fn notice(app: &mut crate::App, message: &str) {
    if let Some(ui) = app.session.ui_mut()
        && let Err(error) = ui.local_message(message) {
        eprintln!("performance profile chat: {error}");
    }
    app.publish_session();
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
    fn profile_header_identifies_format_and_version() {
        let path = std::env::temp_dir().join(format!("solaris-profile-header-{}.tsv", std::process::id()));
        let mut trace = FrameTrace::new(File::create(&path).unwrap()).unwrap();
        trace.metadata(&json!({"adapter":{"name":"test GPU","driver":"test driver"},
            "resolution":[1920,1080],"graphics_preset":"high","render_distance":8}));
        drop(trace);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(text.starts_with("# solaris_frame_profile\t"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("\"name\":\"test GPU\""));
        assert!(text.contains("\"render_distance\":8"));
    }

    #[test]
    fn f10_toggles_only_on_initial_game_press() {
        use winit::keyboard::KeyCode;
        assert!(is_toggle_key(KeyCode::F10, true, false, "game"));
        assert!(!is_toggle_key(KeyCode::F10, false, false, "game"));
        assert!(!is_toggle_key(KeyCode::F10, true, true, "game"));
        assert!(!is_toggle_key(KeyCode::F10, true, false, "inventory"));
        assert!(!is_toggle_key(KeyCode::F11, true, false, "game"));
    }

    #[test]
    fn truncated_profile_can_be_summarized() {
        let path = std::env::temp_dir().join(format!("solaris-truncated-profile-{}.tsv", std::process::id()));
        let mut trace = FrameTrace::new(File::create(&path).unwrap()).unwrap();
        trace.record(&json!({"submitted_frame_id":"1","frame_wall_ms":8.0,
            "gpu_timestamps":{"frame_id":"1","passes_ms":{"opaque":2.0}}}), 0);
        drop(trace);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{}unfinished", text.split("# summary").next().unwrap())).unwrap();
        let summary = summarize(&path).unwrap();
        assert_eq!(summary["frames"], 1);
        assert_eq!(summary["complete"], false);
        assert_eq!(summary["timings"]["frame_wall_ms"]["mean_ms"], 8.0);
        assert_eq!(summary["timings"]["gpu_opaque_ms"]["mean_ms"], 2.0);
        let result = crate::cli(["profile-summary".to_string(), path.display().to_string()]);
        std::fs::remove_file(path).unwrap();
        assert!(result.is_ok(), "{result:?}");
    }

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
            "cpu_client_advance_ms":11.0,
            "cpu_session_events_ms":8.0,
            "cpu_session_tick_detail_ms":{"events":2.0,"player":1.0}
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
        let lines: Vec<_> = text.lines().filter(|line| !line.starts_with('#')).skip(1).collect();
        assert_eq!(lines.len(), 3);
        let cells: Vec<_> = lines[0].split('\t').collect();
        assert_eq!(cells.len(), COLUMNS.len() + 7);
        let cell = |name| cells[2 + COLUMNS.iter().position(|column| *column == name).unwrap()];
        assert_eq!(serde_json::from_str::<Value>(cell("gpu_timestamps")).unwrap(), gpu);
        assert_eq!(cell("queue_submit_ms"), "51.0");
        assert_eq!(cell("render_thread_cpu_ms"), "3.0");
        assert_eq!(cell("cpu_session_events_ms"), "8.0");
        assert_eq!(
            serde_json::from_str::<Value>(cell("cpu_session_tick_detail_ms")).unwrap(),
            json!({"events":2.0,"player":1.0})
        );
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
