//! Audio of a shared monitor source (ADR 0018 "Screen audio"). Pure PCM: every
//! source delivers 48 kHz mono `f32`; Opus and the wire live in `native::sharing`.
//!
//! Sources never capture the game's own output: Linux nested apps get a private
//! null sink per monitor (their only output), Windows captures one other
//! process tree. Linux portal windows have no audio source at all (a
//! whole-system monitor would feed Solaris's own playback back into the stream).
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::Arc,
    thread::JoinHandle,
};
#[cfg(target_os = "linux")]
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

pub(crate) const RATE: u32 = 48_000;
/// Captured audio older than this (1 s) is discarded when the consumer stalls.
const MAX_QUEUED: usize = RATE as usize;
/// Test/diagnostic hook: loop this 16-bit PCM WAV instead of the capture's own audio.
pub(crate) const WAV_ENV: &str = "SOLARIS_SCREEN_AUDIO_WAV";

pub trait AudioSource: Send {
    /// Append the samples captured since the last call; never blocks.
    fn drain(&mut self, out: &mut Vec<f32>) -> Result<(), String>;
    /// True when this capture is the only place the app's sound goes (a private
    /// sink), so the owner must hear it through the world mixer.
    fn replaces_app_output(&self) -> bool;
    fn close(&mut self);
}

/// Bounded hand-over between a capture thread and the game thread.
#[derive(Default)]
pub(crate) struct Queue {
    samples: VecDeque<f32>,
    error: Option<String>,
}
#[derive(Clone, Default)]
pub(crate) struct Pcm(Arc<Mutex<Queue>>);
impl Pcm {
    pub fn push(&self, samples: impl IntoIterator<Item = f32>) {
        let mut queue = self.0.lock();
        queue.samples.extend(samples);
        let excess = queue.samples.len().saturating_sub(MAX_QUEUED);
        queue.samples.drain(..excess);
    }
    pub fn fail(&self, error: String) {
        self.0.lock().error.get_or_insert(error);
    }
    pub fn drain(&self, out: &mut Vec<f32>) -> Result<(), String> {
        let mut queue = self.0.lock();
        out.extend(queue.samples.drain(..));
        queue.error.clone().map_or(Ok(()), Err)
    }
}

/// Little-endian `f32` bytes from a pipe to samples, tolerating split floats.
#[derive(Default)]
pub(crate) struct FloatBytes(Vec<u8>);
impl FloatBytes {
    pub fn feed(&mut self, bytes: &[u8], pcm: &Pcm) {
        self.0.extend_from_slice(bytes);
        let whole = self.0.len() / 4 * 4;
        pcm.push(
            self.0[..whole]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .map(|s| if s.is_finite() { s.clamp(-1., 1.) } else { 0. }),
        );
        self.0.drain(..whole);
    }
}

/// The source the owner client actually uses: the test WAV when requested,
/// otherwise whatever the desktop source offers.
pub(crate) fn resolve(native: Option<Box<dyn AudioSource>>) -> Option<Box<dyn AudioSource>> {
    match std::env::var_os(WAV_ENV) {
        Some(path) => match WavSource::open(std::path::Path::new(&path)) {
            Ok(source) => Some(Box::new(source)),
            Err(error) => {
                eprintln!("{WAV_ENV}: {error}");
                native
            }
        },
        None => native,
    }
}

/// A looped 16-bit PCM WAV delivered in real time (test/diagnostic source).
pub(crate) struct WavSource {
    samples: Vec<f32>,
    at: usize,
    start: std::time::Instant,
    delivered: u64,
    local: bool,
}
impl WavSource {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes)
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
            return Err("not a RIFF/WAVE file".into());
        }
        let (mut channels, mut rate, mut bits, mut data) = (0u16, 0u32, 0u16, None);
        let mut at = 12;
        while let Some(header) = bytes.get(at..at + 8) {
            let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let body = bytes.get(at + 8..(at + 8 + size).min(bytes.len())).unwrap_or(&[]);
            match &header[..4] {
                b"fmt " if body.len() >= 16 => {
                    if u16::from_le_bytes([body[0], body[1]]) != 1 {
                        return Err("only integer PCM WAV is supported".into());
                    }
                    channels = u16::from_le_bytes([body[2], body[3]]);
                    rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                    bits = u16::from_le_bytes([body[14], body[15]]);
                }
                b"data" => data = Some(body),
                _ => {}
            }
            at += 8 + size + (size & 1);
        }
        let data = data.ok_or("WAV has no data chunk")?;
        if bits != 16 || !(1..=2).contains(&channels) || rate != RATE {
            return Err("WAV must be 16-bit PCM, mono or stereo, 48000 Hz".into());
        }
        let samples: Vec<f32> = data
            .chunks_exact(2 * usize::from(channels))
            .map(|frame| {
                let sum: f32 = frame
                    .chunks_exact(2)
                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.)
                    .sum();
                sum / f32::from(channels)
            })
            .collect();
        if samples.is_empty() {
            return Err("WAV has no samples".into());
        }
        Ok(Self { samples, at: 0, start: std::time::Instant::now(), delivered: 0, local: false })
    }
    /// Pretend this is a private-sink source (the owner then hears it positioned).
    #[cfg(test)]
    pub fn local(mut self) -> Self {
        self.local = true;
        self
    }
}
impl AudioSource for WavSource {
    fn drain(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        let due = (self.start.elapsed().as_secs_f64() * f64::from(RATE)) as u64;
        let count = due.saturating_sub(self.delivered).min(u64::from(RATE));
        for _ in 0..count {
            out.push(self.samples[self.at]);
            self.at = (self.at + 1) % self.samples.len();
        }
        self.delivered += count;
        Ok(())
    }
    fn replaces_app_output(&self) -> bool {
        self.local
    }
    fn close(&mut self) {}
}

/// A private PipeWire null sink for one nested monitor. A held-open `pw-cli`
/// owns the node (`object.linger=false`), so the sink disappears with it.
#[cfg(target_os = "linux")]
pub(crate) struct NullSink {
    name: String,
    cli: Child,
    /// Closing it (or killing `cli`) destroys the node.
    _stdin: ChildStdin,
}
#[cfg(target_os = "linux")]
impl NullSink {
    pub fn create() -> Result<Self, String> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!("solaris-monitor-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let mut cli = Command::new("pw-cli")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("pw-cli (PipeWire) unavailable: {e}"))?;
        let mut stdin = cli.stdin.take().ok_or("pw-cli stdin")?;
        let stdout = cli.stdout.take().ok_or("pw-cli stdout")?;
        writeln!(
            stdin,
            "create-node adapter {{ factory.name=support.null-audio-sink node.name={name} \
             node.description=\"Solaris monitor\" media.class=Audio/Sink object.linger=false \
             audio.position=[FL FR] }}"
        )
        .map_err(|e| e.to_string())?;
        // pw-cli answers `<n> = @proxy:<id>` once the core created the node.
        let (created, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut seen = String::new();
            let mut line = String::new();
            while stdout.read_line(&mut line).is_ok_and(|n| n > 0) {
                seen.push_str(&line);
                if line.contains("= @proxy:") || line.contains("Error") {
                    let _ = created.send(seen.contains("= @proxy:"));
                    break;
                }
                line.clear();
            }
            // Keep draining so pw-cli never blocks on a full pipe.
            let mut sink = [0u8; 4096];
            let mut raw = stdout.into_inner();
            while raw.read(&mut sink).is_ok_and(|n| n > 0) {}
        });
        match answer.recv_timeout(Duration::from_secs(5)) {
            Ok(true) => Ok(Self { name, cli, _stdin: stdin }),
            _ => {
                let _ = cli.kill();
                let _ = cli.wait();
                Err("PipeWire refused the monitor sink".into())
            }
        }
    }
    /// Environment that sends an app's output to this sink only.
    pub fn env(&self) -> [(&'static str, String); 2] {
        [("PIPEWIRE_NODE", self.name.clone()), ("PULSE_SINK", self.name.clone())]
    }
    /// Capture the sink's monitor. The sink outlives the capture, so the app
    /// never falls back to the real output when a capture ends.
    pub fn capture(&self) -> Result<Box<dyn AudioSource>, String> {
        PipeWireCapture::open(&self.name).map(|c| Box::new(c) as Box<dyn AudioSource>)
    }
}
#[cfg(target_os = "linux")]
impl Drop for NullSink {
    fn drop(&mut self) {
        let _ = self.cli.kill();
        let _ = self.cli.wait();
    }
}

/// `pw-cat` recording of a null sink's monitor as 48 kHz mono `f32`.
#[cfg(target_os = "linux")]
struct PipeWireCapture {
    pcm: Pcm,
    child: Child,
    reader: Option<JoinHandle<()>>,
}
#[cfg(target_os = "linux")]
impl PipeWireCapture {
    fn open(sink: &str) -> Result<Self, String> {
        let mut child = Command::new("pw-cat")
            .args([
                "--record", "--target", sink, "-P", "stream.capture.sink=true",
                "--rate", "48000", "--channels", "1", "--format", "f32", "--latency", "20ms", "--raw", "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("pw-cat (PipeWire) unavailable: {e}"))?;
        let mut pipe = child.stdout.take().ok_or("pw-cat stdout")?;
        let pcm = Pcm::default();
        let output = pcm.clone();
        let reader = std::thread::Builder::new()
            .name("screen-audio-capture".into())
            .spawn(move || {
                let (mut buffer, mut floats) = ([0u8; 4096], FloatBytes::default());
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => floats.feed(&buffer[..n], &output),
                    }
                }
                output.fail("PipeWire capture ended".into());
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { pcm, child, reader: Some(reader) })
    }
}
#[cfg(target_os = "linux")]
impl AudioSource for PipeWireCapture {
    fn drain(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        self.pcm.drain(out)
    }
    fn replaces_app_output(&self) -> bool {
        true
    }
    fn close(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
#[cfg(target_os = "linux")]
impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = b"RIFF".to_vec();
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(*b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend([1, 0]);
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * u32::from(channels) * 2).to_le_bytes());
        out.extend([channels as u8 * 2, 0, 16, 0]);
        out.extend(*b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn wav_source_parses_downmixes_loops_and_rejects_other_formats() {
        let mut source = WavSource::parse(&wav(RATE, 2, &[16384, -16384, 16384, 16384])).unwrap();
        assert_eq!(source.samples, [0., 0.25]);
        source.start -= std::time::Duration::from_millis(10);
        let mut out = Vec::new();
        source.drain(&mut out).unwrap();
        assert!((470..=490).contains(&out.len()), "10 ms of audio, got {}", out.len());
        assert_eq!(&out[..4], [0., 0.25, 0., 0.25], "the clip loops");
        assert!(WavSource::parse(&wav(44_100, 1, &[1])).is_err());
        assert!(WavSource::parse(b"nope").is_err());
    }
    /// Real PipeWire graph: an app (`pw-play`) routed by `PIPEWIRE_NODE` into the
    /// private null sink is heard by the monitor capture, and nothing before it plays.
    /// Run through `tools/screen_audio_graph.sh` (private PipeWire + WirePlumber).
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs a PipeWire graph: run tools/screen_audio_graph.sh"]
    fn null_sink_monitor_captures_only_what_the_routed_app_plays() {
        let sink = NullSink::create().unwrap();
        let mut capture = sink.capture().unwrap();
        assert!(capture.replaces_app_output());
        let tone: Vec<i16> = (0..48_000 * 2 * 2)
            .map(|i| ((i / 2) as f32 * 440. * std::f32::consts::TAU / RATE as f32).sin().mul_add(12_000., 0.) as i16)
            .collect();
        let path = std::env::temp_dir().join(format!("solaris-screen-audio-{}.wav", std::process::id()));
        std::fs::write(&path, wav(RATE, 2, &tone)).unwrap();
        let status = Command::new("pw-play")
            .envs(sink.env())
            .arg(&path)
            .stderr(Stdio::null())
            .status()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(status.success());
        let mut heard = Vec::new();
        capture.drain(&mut heard).unwrap();
        let loud = heard.iter().filter(|s| s.abs() > 0.05).count();
        assert!(loud > RATE as usize / 2, "monitor heard {loud} loud samples of {}", heard.len());
        capture.close();
        drop(sink);
    }
    #[test]
    fn pipe_bytes_become_bounded_finite_samples_across_split_floats() {
        let pcm = Pcm::default();
        let mut bytes = FloatBytes::default();
        let raw: Vec<u8> = [0.5f32, f32::NAN, 7.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        bytes.feed(&raw[..6], &pcm);
        bytes.feed(&raw[6..], &pcm);
        let mut out = Vec::new();
        pcm.drain(&mut out).unwrap();
        assert_eq!(out, [0.5, 0., 1.]);
        pcm.push(std::iter::repeat_n(0.1, MAX_QUEUED + 10));
        pcm.drain(&mut out).unwrap();
        assert_eq!(out.len(), 3 + MAX_QUEUED, "a stalled consumer never grows the queue");
        pcm.fail("gone".into());
        assert_eq!(pcm.drain(&mut out), Err("gone".into()));
    }
}
