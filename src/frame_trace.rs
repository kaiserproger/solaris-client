//! Opt-in frame pacing trace: `SOLARIS_CLIENT_FRAME_TRACE=<file>` writes one
//! tab-separated line per submitted frame, read from the same render metrics
//! the observation publishes. Unset (the default) costs nothing.

use serde_json::Value;
use std::{
    fmt::Write as _,
    fs::File,
    io::{LineWriter, Write},
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
                Value::Object(_) => {
                    let _ = write!(line, "{}", render[column]);
                }
                _ => line.push('-'),
            }
        }
        let _ = writeln!(self.out, "{line}\t{look_events}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trace_preserves_completed_gpu_frame_identity_and_draw_phases() {
        let path = std::env::temp_dir().join(format!("solaris-frame-trace-{}.tsv", std::process::id()));
        let mut trace = FrameTrace {
            out: LineWriter::new(File::create(&path).unwrap()),
            start: Instant::now(),
            last_frame: None,
        };
        let gpu = json!({"frame_id":"39","passes_ms":{"opaque":2.5},"total_ms":2.5});
        let render = json!({
            "submitted_frame_id":"42",
            "gpu_timestamps":gpu,
            "queue_submit_ms":51.0,
            "render_thread_cpu_ms":3.0
        });
        trace.record(&render, 2);
        trace.record(&render, 4);
        drop(trace);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(text.lines().count(), 1);
        let cells: Vec<_> = text.trim_end().split('\t').collect();
        assert_eq!(cells.len(), COLUMNS.len() + 3);
        let cell = |name| cells[2 + COLUMNS.iter().position(|column| *column == name).unwrap()];
        assert_eq!(serde_json::from_str::<Value>(cell("gpu_timestamps")).unwrap(), gpu);
        assert_eq!(cell("queue_submit_ms"), "51.0");
        assert_eq!(cell("render_thread_cpu_ms"), "3.0");
        assert_eq!(cells[1], "42");
    }
}
