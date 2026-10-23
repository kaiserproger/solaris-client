//! Remote proximity voices: per-speaker Opus decode, a small jitter buffer and one
//! mono 48 kHz source mixed into the ordinary output with linear distance attenuation.
use crate::{
    protocol::Result,
    voice::{Decoder, FRAME, RATE},
};
use parking_lot::Mutex;
use rodio::Source;
use std::{
    collections::{HashMap, VecDeque},
    io::{Seek, SeekFrom, Write},
    sync::Arc,
    time::{Duration, Instant},
};

/// Playback starts once this much is buffered (60 ms) and restarts after an underrun.
const PREFILL: usize = 3 * FRAME;
/// Older audio is dropped beyond this (160 ms) so latency cannot grow.
const MAX_BUFFER: usize = 8 * FRAME;
/// A packet gap longer than this starts a fresh talk spurt.
const SPURT_GAP: Duration = Duration::from_millis(120);
const MAX_SPEAKERS: usize = 64;
const MAX_CONCEALED: u32 = 3;
const DEFAULT_RANGE: f32 = 48.;
/// Test hook: write every decoded remote frame to this mono 16-bit WAV.
const RECORD_ENV: &str = "SOLARIS_VOICE_RECORD";

struct Speaker {
    decoder: Decoder,
    position: [f32; 3],
    range: Option<f32>,
    frames: u64,
    peak: f32,
    pcm: VecDeque<f32>,
    playing: bool,
    seq: Option<u32>,
    last: Instant,
}
struct Recorder {
    file: std::fs::File,
    samples: u32,
}
impl Recorder {
    fn create(path: &std::ffi::OsStr) -> Option<Self> {
        let mut file = std::fs::File::create(path).ok()?;
        let mut header = Vec::with_capacity(44);
        header.extend(*b"RIFF");
        header.extend(36u32.to_le_bytes());
        header.extend(*b"WAVEfmt ");
        header.extend(16u32.to_le_bytes());
        header.extend([1, 0, 1, 0]);
        header.extend(RATE.to_le_bytes());
        header.extend((RATE * 2).to_le_bytes());
        header.extend([2, 0, 16, 0]);
        header.extend(*b"data");
        header.extend(0u32.to_le_bytes());
        file.write_all(&header).ok()?;
        Some(Self { file, samples: 0 })
    }
    fn write(&mut self, pcm: &[f32; FRAME]) {
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| ((s.clamp(-1., 1.) * 32767.) as i16).to_le_bytes()).collect();
        self.samples += FRAME as u32;
        let data = self.samples * 2;
        // The header is patched after every frame so a killed client still leaves a valid WAV.
        let _ = self.file.write_all(&bytes);
        let end = self.file.stream_position().unwrap_or(0);
        let _ = self.file.seek(SeekFrom::Start(4)).and_then(|_| self.file.write_all(&(36 + data).to_le_bytes()));
        let _ = self.file.seek(SeekFrom::Start(40)).and_then(|_| self.file.write_all(&data.to_le_bytes()));
        let _ = self.file.seek(SeekFrom::Start(end));
    }
}
struct State {
    listener: [f64; 3],
    range: f32,
    speakers: HashMap<[u8; 16], Speaker>,
    recorder: Option<Recorder>,
}
pub struct VoiceMixer {
    state: Mutex<State>,
}
impl VoiceMixer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                listener: [0.; 3],
                range: DEFAULT_RANGE,
                speakers: HashMap::new(),
                recorder: std::env::var_os(RECORD_ENV).and_then(|path| Recorder::create(&path)),
            }),
        })
    }
    pub fn set_listener(&self, position: [f64; 3]) {
        self.state.lock().listener = position;
    }
    pub fn set_range(&self, range: f32) {
        if range.is_finite() && range > 0. {
            self.state.lock().range = range;
        }
    }
    /// `(decoded frames, peak of the last decoded frame)` for tests and facts.
    pub fn speaker_stats(&self, speaker: [u8; 16]) -> Option<(u64, f32)> {
        self.state.lock().speakers.get(&speaker).map(|s| (s.frames, s.peak))
    }
    pub fn clear(&self) {
        self.state.lock().speakers.clear();
    }
    /// Decode one relayed packet into the speaker's jitter buffer.
    /// `range` overrides the server's voice range for this speaker (e.g. a monitor's speakers).
    pub fn push(&self, speaker: [u8; 16], position: [f32; 3], range: Option<f32>, seq: u32, opus: &[u8]) -> Result<()> {
        let mut state = self.state.lock();
        let State { speakers, recorder, .. } = &mut *state;
        if !speakers.contains_key(&speaker) {
            if speakers.len() >= MAX_SPEAKERS {
                return Err("too many simultaneous voice speakers".into());
            }
            speakers.insert(
                speaker,
                Speaker {
                    decoder: Decoder::new()?,
                    position,
                    range,
                    frames: 0,
                    peak: 0.,
                    pcm: VecDeque::with_capacity(MAX_BUFFER + FRAME),
                    playing: false,
                    seq: None,
                    last: Instant::now(),
                },
            );
        }
        let entry = speakers.get_mut(&speaker).ok_or("voice speaker vanished")?;
        let now = Instant::now();
        if now.duration_since(entry.last) > SPURT_GAP {
            entry.pcm.clear();
            entry.playing = false;
            entry.seq = None;
        }
        entry.last = now;
        entry.position = position;
        entry.range = range;
        let mut lost = 0;
        if let Some(previous) = entry.seq {
            let ahead = seq.wrapping_sub(previous);
            if ahead == 0 || ahead > u32::MAX / 2 {
                return Ok(()); // duplicate or late
            }
            lost = (ahead - 1).min(MAX_CONCEALED);
        }
        for packet in (0..lost).map(|_| None).chain([Some(opus)]) {
            let frame = entry.decoder.decode(packet)?;
            if let Some(recorder) = recorder.as_mut() {
                recorder.write(&frame);
            }
            if packet.is_some() {
                entry.frames += 1;
                entry.peak = frame.iter().fold(0f32, |m, s| m.max(s.abs()));
            }
            entry.pcm.extend(frame);
        }
        entry.seq = Some(seq);
        let excess = entry.pcm.len().saturating_sub(MAX_BUFFER);
        entry.pcm.drain(..excess);
        entry.playing |= entry.pcm.len() >= PREFILL;
        Ok(())
    }
    fn mix(&self, out: &mut [f32; FRAME]) {
        out.fill(0.);
        let mut state = self.state.lock();
        let (listener, range) = (state.listener, state.range);
        let now = Instant::now();
        state.speakers.retain(|_, s| s.playing || now.duration_since(s.last) < Duration::from_secs(2));
        for speaker in state.speakers.values_mut().filter(|s| s.playing) {
            let distance = (0..3)
                .map(|i| (f64::from(speaker.position[i]) - listener[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            let gain = (1. - distance / f64::from(speaker.range.unwrap_or(range))).clamp(0., 1.) as f32;
            for sample in out.iter_mut() {
                match speaker.pcm.pop_front() {
                    Some(value) => *sample += value * gain,
                    None => {
                        speaker.playing = false; // underrun: rebuffer before resuming
                        break;
                    }
                }
            }
        }
        out.iter_mut().for_each(|s| *s = s.clamp(-1., 1.));
    }
}
/// Endless mono 48 kHz source pulling 20 ms blocks from the mixer.
pub(super) struct VoiceSource {
    mixer: Arc<VoiceMixer>,
    block: Box<[f32; FRAME]>,
    at: usize,
}
impl VoiceSource {
    pub fn new(mixer: Arc<VoiceMixer>) -> Self {
        Self { mixer, block: Box::new([0.; FRAME]), at: FRAME }
    }
}
impl Iterator for VoiceSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.at == FRAME {
            self.mixer.mix(&mut self.block);
            self.at = 0;
        }
        self.at += 1;
        Some(self.block[self.at - 1])
    }
}
impl Source for VoiceSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::Encoder;

    fn packets(count: usize) -> Vec<Vec<u8>> {
        let mut encoder = Encoder::new().unwrap();
        (0..count)
            .map(|n| {
                let pcm = std::array::from_fn(|i| 0.5 * ((2. * std::f32::consts::PI * 440. * (n * FRAME + i) as f32) / RATE as f32).sin());
                encoder.encode(&pcm).unwrap()
            })
            .collect()
    }
    fn peak(mixer: &VoiceMixer) -> f32 {
        let mut block = [0.; FRAME];
        mixer.mix(&mut block);
        block.iter().fold(0f32, |m, s| m.max(s.abs()))
    }
    #[test]
    fn jitter_prefill_then_attenuated_playback() {
        let mixer = VoiceMixer::new();
        let speaker = [1; 16];
        let wire = packets(6);
        mixer.push(speaker, [24., 0., 0.], None, 0, &wire[0]).unwrap();
        mixer.push(speaker, [24., 0., 0.], None, 1, &wire[1]).unwrap();
        assert_eq!(peak(&mixer), 0., "still prefilling");
        mixer.push(speaker, [24., 0., 0.], None, 2, &wire[2]).unwrap();
        assert_eq!(mixer.speaker_stats(speaker).map(|s| s.0), Some(3));
        let half = peak(&mixer);
        assert!(half > 0.1, "audible at half range: {half}");
        // Moving the speaker out of range silences it without dropping the stream.
        mixer.push(speaker, [100., 0., 0.], None, 3, &wire[3]).unwrap();
        assert_eq!(peak(&mixer), 0.);
        mixer.set_listener([100., 0., 0.]);
        mixer.push(speaker, [100., 0., 0.], None, 4, &wire[4]).unwrap();
        assert!(peak(&mixer) > half, "full volume at zero distance");
    }
    #[test]
    fn underrun_rebuffers_and_duplicates_are_dropped() {
        let mixer = VoiceMixer::new();
        let speaker = [2; 16];
        let wire = packets(4);
        for (seq, packet) in wire.iter().take(3).enumerate() {
            mixer.push(speaker, [0.; 3], None, seq as u32, packet).unwrap();
        }
        mixer.push(speaker, [0.; 3], None, 2, &wire[2]).unwrap(); // duplicate
        for _ in 0..3 {
            assert!(peak(&mixer) > 0.1);
        }
        assert_eq!(peak(&mixer), 0., "drained buffer is silent");
        mixer.push(speaker, [0.; 3], None, 3, &wire[3]).unwrap();
        assert_eq!(peak(&mixer), 0., "one frame is below the prefill threshold");
    }
    #[test]
    fn per_speaker_range_overrides_the_voice_range() {
        let mixer = VoiceMixer::new();
        let wire = packets(3);
        for (seq, packet) in wire.iter().enumerate() {
            mixer.push([3; 16], [40., 0., 0.], Some(32.), seq as u32, packet).unwrap();
        }
        assert_eq!(peak(&mixer), 0., "40 blocks is beyond a 32-block range");
        assert!(mixer.speaker_stats([3; 16]).unwrap().1 > 0.1);
    }
}
