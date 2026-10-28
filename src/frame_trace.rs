//! Opt-in frame pacing trace: `SOLARIS_CLIENT_FRAME_TRACE=<file>` writes one
//! tab-separated line per submitted frame, read from the same render metrics
//! the observation publishes. Unset (the default) costs nothing.

use serde_json::Value;
use std::{
    fs::File,
    io::{LineWriter, Write},
    time::Instant,
};

const COLUMNS: [&str; 11] = [
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
];

pub(crate) struct FrameTrace {
    out: LineWriter<File>,
    start: Instant,
    last_frame: Option<String>,
}

impl FrameTrace {
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os("SOLARIS_CLIENT_FRAME_TRACE")?;
        let mut out = match File::create(&path) {
            Ok(file) => LineWriter::new(file),
            Err(error) => {
                eprintln!("frame trace {}: {error}", path.to_string_lossy());
                return None;
            }
        };
        let _ = writeln!(out, "t_ms\tframe_id\t{}\tlook_events", COLUMNS.join("\t"));
        Some(Self { out, start: Instant::now(), last_frame: None })
    }

    /// Records `render` once per submitted frame; skipped draws repeat the id.
    pub(crate) fn record(&mut self, render: &Value, look_events: u32) {
        let frame = render["submitted_frame_id"].as_str();
        if frame.is_none() || frame == self.last_frame.as_deref() {
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
                _ => line.push('-'),
            }
        }
        let _ = writeln!(self.out, "{line}\t{look_events}");
    }
}
