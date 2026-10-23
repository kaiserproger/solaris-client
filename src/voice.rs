//! Proximity voice chat (`solaris:voice`, ADR 0019): microphone capture, push-to-talk,
//! Opus encoding and the wire format. Playback lives in `audio::voicemix`.
//!
//! Capture runs on its own thread that owns the cpal stream; everything it needs
//! arrives through one channel, so stopping is a message, never a poll.
mod app;
use crate::protocol::Result;
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use unsafe_libopus as libopus;

pub(crate) use app::Client;

pub(crate) const CHANNEL: &str = "solaris:voice";
pub(crate) const RATE: u32 = 48_000;
/// 20 ms of 48 kHz mono.
pub(crate) const FRAME: usize = 960;
const BITRATE: i32 = 24_000;
pub(crate) const MAX_OPUS: usize = 1000;
/// Test hook: a 16-bit PCM WAV looped as the microphone (no audio device needed).
const MIC_WAV_ENV: &str = "SOLARIS_VOICE_MIC_WAV";
/// Test hook: hold push-to-talk down.
const PTT_ALWAYS_ENV: &str = "SOLARIS_VOICE_PTT_ALWAYS";

pub(crate) fn audio_packet(seq: u32, opus: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + opus.len());
    out.extend([1, 0]);
    out.extend(seq.to_be_bytes());
    out.extend(opus);
    out
}
pub(crate) fn listen_packet(on: bool) -> Vec<u8> {
    vec![1, 1, u8::from(on)]
}

/// Per-connection protocol state kept on the session.
#[derive(Default)]
pub(crate) struct Net {
    pub registered: bool,
    /// Server `VOICE_STATE.enabled`.
    pub enabled: bool,
    /// We told the server to relay audio to us (`LISTEN(1)`).
    pub listening: bool,
}

pub(crate) enum Incoming<'a> {
    Audio {
        speaker: [u8; 16],
        position: [f32; 3],
        seq: u32,
        opus: &'a [u8],
    },
    State {
        enabled: bool,
        range: u16,
    },
}
pub(crate) fn parse(bytes: &[u8]) -> Result<Incoming<'_>> {
    let [version, kind, body @ ..] = bytes else {
        return Err("voice payload too short".into());
    };
    if *version != 1 {
        return Err("unsupported voice version".into());
    }
    match (*kind, body) {
        (0, body) if body.len() > 16 + 12 + 4 => {
            let float = |i: usize| f32::from_be_bytes(body[16 + 4 * i..20 + 4 * i].try_into().unwrap());
            let position = [float(0), float(1), float(2)];
            if position.iter().any(|v| !v.is_finite()) {
                return Err("voice speaker position not finite".into());
            }
            let opus = &body[32..];
            if opus.len() > MAX_OPUS {
                return Err("voice opus packet too large".into());
            }
            Ok(Incoming::Audio {
                speaker: body[..16].try_into().unwrap(),
                position,
                seq: u32::from_be_bytes(body[28..32].try_into().unwrap()),
                opus,
            })
        }
        (1, [enabled, hi, lo]) => Ok(Incoming::State {
            enabled: *enabled != 0,
            range: u16::from_be_bytes([*hi, *lo]),
        }),
        _ => Err("malformed voice payload".into()),
    }
}

pub(crate) struct Encoder(*mut libopus::OpusEncoder);
// The encoder state is only ever touched by the thread that owns this value.
unsafe impl Send for Encoder {}
impl Encoder {
    pub fn new() -> Result<Self> {
        Self::with_bitrate(BITRATE, false)
    }
    /// `music` selects the general-audio application/signal instead of speech.
    pub fn with_bitrate(bits_per_second: i32, music: bool) -> Result<Self> {
        let (application, signal) = if music {
            (libopus::OPUS_APPLICATION_AUDIO, libopus::OPUS_SIGNAL_MUSIC)
        } else {
            (libopus::OPUS_APPLICATION_VOIP, libopus::OPUS_SIGNAL_VOICE)
        };
        let mut error = 0;
        // SAFETY: valid arguments; the pointer is checked and freed exactly once in Drop.
        let state = unsafe { libopus::opus_encoder_create(RATE as i32, 1, application, &mut error) };
        if error != libopus::OPUS_OK || state.is_null() {
            return Err(format!("opus encoder create failed: {error}"));
        }
        let encoder = Self(state);
        // SAFETY: `state` is a live encoder; each request takes one i32.
        unsafe {
            libopus::opus_encoder_ctl!(state, libopus::OPUS_SET_BITRATE_REQUEST, bits_per_second);
            libopus::opus_encoder_ctl!(state, libopus::OPUS_SET_SIGNAL_REQUEST, signal);
            libopus::opus_encoder_ctl!(state, libopus::OPUS_SET_COMPLEXITY_REQUEST, 5);
        }
        Ok(encoder)
    }
    pub fn encode(&mut self, pcm: &[f32; FRAME]) -> Result<Vec<u8>> {
        let mut out = vec![0u8; MAX_OPUS];
        // SAFETY: `pcm` holds FRAME samples, `out` MAX_OPUS bytes; the encoder is live.
        let n = unsafe { libopus::opus_encode_float(self.0, pcm.as_ptr(), FRAME as i32, out.as_mut_ptr(), MAX_OPUS as i32) };
        if n < 0 {
            return Err(format!("opus encode failed: {n}"));
        }
        out.truncate(n as usize);
        Ok(out)
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_encoder_create and never freed elsewhere.
        unsafe { libopus::opus_encoder_destroy(self.0) }
    }
}

pub(crate) struct Decoder(*mut libopus::OpusDecoder);
// Owned by one speaker entry, used only under the mixer lock.
unsafe impl Send for Decoder {}
impl Decoder {
    pub fn new() -> Result<Self> {
        let mut error = 0;
        // SAFETY: valid arguments; the pointer is checked and freed exactly once in Drop.
        let state = unsafe { libopus::opus_decoder_create(RATE as i32, 1, &mut error) };
        if error != libopus::OPUS_OK || state.is_null() {
            return Err(format!("opus decoder create failed: {error}"));
        }
        Ok(Self(state))
    }
    /// `None` conceals one lost frame.
    pub fn decode(&mut self, packet: Option<&[u8]>) -> Result<[f32; FRAME]> {
        let mut pcm = [0f32; FRAME];
        let (data, len) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        // SAFETY: `pcm` holds FRAME samples; `data`/`len` describe a live slice or are null/0.
        let n = unsafe { libopus::opus_decode_float(self.0, data, len, pcm.as_mut_ptr(), FRAME as i32, 0) };
        if n != FRAME as i32 {
            return Err(format!("opus decode failed: {n}"));
        }
        if pcm.iter().any(|s| !s.is_finite()) {
            return Err("opus decoded non-finite PCM".into());
        }
        Ok(pcm)
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_decoder_create and never freed elsewhere.
        unsafe { libopus::opus_decoder_destroy(self.0) }
    }
}

/// Linear-interpolating mono resampler to 48 kHz that accumulates exact 20 ms frames.
struct Framer {
    step: f64,
    position: f64,
    previous: f32,
    frame: Box<[f32; FRAME]>,
    filled: usize,
}
impl Framer {
    fn new(rate: u32) -> Self {
        Self {
            step: f64::from(rate) / f64::from(RATE),
            position: 0.,
            previous: 0.,
            frame: Box::new([0.; FRAME]),
            filled: 0,
        }
    }
    fn push(&mut self, input: &[f32], mut frame_out: impl FnMut(&[f32; FRAME])) {
        for &sample in input {
            // `position` is the source index of the next output, relative to `previous`.
            while self.position < 1. {
                self.frame[self.filled] = self.previous + (sample - self.previous) * self.position as f32;
                self.filled += 1;
                if self.filled == FRAME {
                    frame_out(&self.frame);
                    self.filled = 0;
                }
                self.position += self.step;
            }
            self.position -= 1.;
            self.previous = sample;
        }
    }
}

enum Message {
    Pcm(Vec<f32>),
    Stop,
}
/// What the HUD shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MicState {
    Off,
    Idle,
    Talking,
    Unavailable,
}
pub(crate) struct Capture {
    sender: Sender<Message>,
    pub packets: Receiver<(u32, Vec<u8>)>,
    ptt: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    gain: Arc<AtomicU32>,
}
impl Capture {
    pub fn start() -> Self {
        let (sender, input) = mpsc::channel();
        let (output, packets) = mpsc::channel();
        let ptt = Arc::new(AtomicBool::new(std::env::var_os(PTT_ALWAYS_ENV).is_some()));
        let failed = Arc::new(AtomicBool::new(false));
        let gain = Arc::new(AtomicU32::new(1f32.to_bits()));
        let state = (sender.clone(), ptt.clone(), failed.clone(), gain.clone());
        if let Err(e) = thread::Builder::new().name("voice-capture".into()).spawn(move || {
            let (sender, ptt, failed, gain) = state;
            if let Err(e) = run(sender, input, output, ptt, gain) {
                eprintln!("voice capture stopped: {e}");
                failed.store(true, Ordering::Release);
            }
        }) {
            eprintln!("voice capture thread: {e}");
            failed.store(true, Ordering::Release);
        }
        Self { sender, packets, ptt, failed, gain }
    }
    pub fn set_ptt(&self, down: bool) {
        if std::env::var_os(PTT_ALWAYS_ENV).is_none() {
            self.ptt.store(down, Ordering::Release);
        }
    }
    /// Options "Microphone volume" (0..=1).
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.clamp(0., 1.).to_bits(), Ordering::Release);
    }
    pub fn state(&self) -> MicState {
        if self.failed.load(Ordering::Acquire) {
            MicState::Unavailable
        } else if self.ptt.load(Ordering::Acquire) {
            MicState::Talking
        } else {
            MicState::Idle
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
    }
}

fn run(sender: Sender<Message>, input: Receiver<Message>, output: Sender<(u32, Vec<u8>)>, ptt: Arc<AtomicBool>, gain: Arc<AtomicU32>) -> Result<()> {
    let mut encoder = Encoder::new()?;
    let mut seq = 0u32;
    let mut was_talking = false;
    let mut encode = |frame: &[f32; FRAME]| {
        let talking = ptt.load(Ordering::Acquire);
        if talking != was_talking {
            was_talking = talking;
            if talking && let Ok(fresh) = Encoder::new() {
                encoder = fresh;
            }
        }
        if !talking {
            return;
        }
        let gain = f32::from_bits(gain.load(Ordering::Acquire));
        let scaled: [f32; FRAME] = std::array::from_fn(|i| frame[i] * gain);
        match encoder.encode(&scaled) {
            Ok(packet) => {
                let _ = output.send((seq, packet));
                seq = seq.wrapping_add(1);
            }
            Err(e) => eprintln!("voice encode dropped a frame: {e}"),
        }
    };
    let (rate, _stream) = if let Some(path) = std::env::var_os(MIC_WAV_ENV) {
        let wav = read_wav(&PathBuf::from(path))?;
        let rate = wav.0;
        thread::Builder::new()
            .name("voice-mic-wav".into())
            .spawn(move || feed_wav(wav, sender))
            .map_err(|e| e.to_string())?;
        (rate, None)
    } else {
        let (rate, stream) = open_microphone(sender)?;
        (rate, Some(stream))
    };
    let mut framer = Framer::new(rate);
    while let Ok(Message::Pcm(chunk)) = input.recv() {
        framer.push(&chunk, &mut encode);
    }
    Ok(())
}

fn open_microphone(sender: Sender<Message>) -> Result<(u32, cpal::Stream)> {
    let device = cpal::default_host().default_input_device().ok_or("no microphone found")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let channels = usize::from(config.channels());
    let format = config.sample_format();
    let config = config.into();
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, channels, sender),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, channels, sender),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, channels, sender),
        other => Err(format!("unsupported microphone sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((rate, stream))
}
fn build<T: SizedSample>(device: &cpal::Device, config: &cpal::StreamConfig, channels: usize, sender: Sender<Message>) -> Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mono = data
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().map(|s| f32::from_sample_(*s)).sum::<f32>() / channels as f32)
                    .collect();
                let _ = sender.send(Message::Pcm(mono));
            },
            |error| eprintln!("microphone stream error: {error}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// 16-bit PCM WAV -> (rate, mono samples).
fn read_wav(path: &std::path::Path) -> Result<(u32, Vec<f32>)> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("microphone WAV is not RIFF/WAVE".into());
    }
    let (mut rate, mut channels, mut data) = (0u32, 0usize, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = bytes.get(at + 8..(at + 8 + size).min(bytes.len())).unwrap_or(&[]);
        match &bytes[at..at + 4] {
            b"fmt " if body.len() >= 16 => {
                if u16::from_le_bytes([body[0], body[1]]) != 1 || u16::from_le_bytes([body[14], body[15]]) != 16 {
                    return Err("microphone WAV must be 16-bit PCM".into());
                }
                channels = usize::from(u16::from_le_bytes([body[2], body[3]]));
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    let data = data.ok_or("microphone WAV has no data chunk")?;
    if channels == 0 || rate == 0 {
        return Err("microphone WAV has no fmt chunk".into());
    }
    let samples: Vec<f32> = data
        .chunks_exact(2 * channels)
        .map(|frame| {
            frame.chunks_exact(2).map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.).sum::<f32>() / channels as f32
        })
        .collect();
    if samples.is_empty() {
        return Err("microphone WAV is empty".into());
    }
    Ok((rate, samples))
}
/// Plays the WAV as a real-time microphone: one 20 ms chunk per 20 ms, on a monotonic deadline.
fn feed_wav((rate, samples): (u32, Vec<f32>), sender: Sender<Message>) {
    let chunk = (rate as usize * 20 / 1000).max(1);
    let mut deadline = Instant::now();
    let mut at = 0;
    loop {
        let end = (at + chunk).min(samples.len());
        let mut pcm = samples[at..end].to_vec();
        pcm.resize(chunk, 0.);
        at = if end == samples.len() { 0 } else { end };
        if sender.send(Message::Pcm(pcm)).is_err() {
            return;
        }
        deadline += Duration::from_millis(20);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            thread::park_timeout(left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frame: usize) -> [f32; FRAME] {
        std::array::from_fn(|i| 0.5 * ((2. * std::f32::consts::PI * 440. * (frame * FRAME + i) as f32) / RATE as f32).sin())
    }
    #[test]
    fn encode_decode_loopback_preserves_the_tone() {
        let mut encoder = Encoder::new().unwrap();
        let mut decoder = Decoder::new().unwrap();
        let mut last = [0.; FRAME];
        for n in 0..10 {
            let packet = encoder.encode(&tone(n)).unwrap();
            assert!(!packet.is_empty() && packet.len() <= MAX_OPUS, "{} bytes", packet.len());
            last = decoder.decode(Some(&packet)).unwrap();
        }
        let power = |pcm: &[f32]| pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32;
        let rms = power(&last).sqrt();
        assert!((0.25..0.45).contains(&rms), "decoded rms {rms}");
        // Concealment of one lost frame still yields finite PCM.
        assert!(decoder.decode(None).is_ok());
    }
    #[test]
    fn wire_round_trip_and_rejection() {
        let packet = audio_packet(7, &[1, 2, 3]);
        assert_eq!(packet, [1, 0, 0, 0, 0, 7, 1, 2, 3]);
        let mut server = vec![1, 0];
        server.extend([9u8; 16]);
        for v in [1.5f32, 64., -3.] {
            server.extend(v.to_be_bytes());
        }
        server.extend(42u32.to_be_bytes());
        server.extend([5, 6]);
        match parse(&server).unwrap() {
            Incoming::Audio { speaker, position, seq, opus } => {
                assert_eq!((speaker, position, seq, opus), ([9; 16], [1.5, 64., -3.], 42, &[5u8, 6][..]));
            }
            _ => panic!("expected audio"),
        }
        assert!(matches!(parse(&[1, 1, 1, 0, 48]).unwrap(), Incoming::State { enabled: true, range: 48 }));
        assert!(parse(&[2, 0]).is_err() && parse(&[1, 0, 1]).is_err() && parse(&[1]).is_err());
        assert_eq!(listen_packet(true), [1, 1, 1]);
    }
    #[test]
    fn framer_resamples_to_exact_frames() {
        let mut framer = Framer::new(44_100);
        let mut frames = 0;
        let input = vec![0.25; 44_100];
        framer.push(&input, |frame| {
            frames += 1;
            assert!(frame[FRAME / 2] > 0.2);
        });
        assert!((49..=50).contains(&frames), "{frames}");
    }
}
