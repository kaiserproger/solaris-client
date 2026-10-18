use super::resources::validate_format;
use crate::protocol::Result;
use rodio::{Decoder, Source};
use std::{
    io::Cursor,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, Thread},
    time::Duration,
};

pub(super) struct SharedSamples {
    data: Arc<Vec<f32>>,
    offset: usize,
    channels: u16,
    rate: u32,
    looping: bool,
}
impl SharedSamples {
    pub fn new(data: Arc<Vec<f32>>, channels: u16, rate: u32, looping: bool) -> Self {
        Self {
            data,
            offset: 0,
            channels,
            rate,
            looping,
        }
    }
}
impl Iterator for SharedSamples {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.offset == self.data.len() && self.looping {
            self.offset = 0;
        }
        let v = self.data.get(self.offset).copied();
        self.offset += usize::from(v.is_some());
        v
    }
}
impl Source for SharedSamples {
    fn current_span_len(&self) -> Option<usize> {
        if self.looping {
            None
        } else {
            Some(self.data.len() - self.offset)
        }
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        if self.looping {
            None
        } else {
            Some(Duration::from_secs_f64(
                self.data.len() as f64 / self.rate as f64 / self.channels as f64,
            ))
        }
    }
}
const RING: usize = 65536;
struct Ring {
    data: Box<[AtomicU32]>,
    read: AtomicUsize,
    write: AtomicUsize,
    end: AtomicBool,
    cancelled: AtomicBool,
    failed: AtomicBool,
    producer: OnceLock<Thread>,
}
impl Ring {
    fn new() -> Self {
        Self {
            data: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            end: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            producer: OnceLock::new(),
        }
    }
    fn push(&self, v: f32) -> bool {
        let w = self.write.load(Ordering::Relaxed);
        if w.wrapping_sub(self.read.load(Ordering::Acquire)) == RING {
            return false;
        }
        self.data[w % RING].store(v.to_bits(), Ordering::Relaxed);
        self.write.store(w.wrapping_add(1), Ordering::Release);
        true
    }
    fn pop(&self) -> Option<f32> {
        let r = self.read.load(Ordering::Relaxed);
        if r == self.write.load(Ordering::Acquire) {
            return None;
        }
        let v = f32::from_bits(self.data[r % RING].load(Ordering::Relaxed));
        self.read.store(r.wrapping_add(1), Ordering::Release);
        if r % 256 == 0 {
            self.wake();
        }
        Some(v)
    }
    fn available(&self) -> usize {
        self.write
            .load(Ordering::Acquire)
            .wrapping_sub(self.read.load(Ordering::Relaxed))
    }
    fn wake(&self) {
        if let Some(t) = self.producer.get() {
            t.unpark();
        }
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake();
    }
}
#[derive(Clone)]
pub(super) struct StreamControl {
    ring: Arc<Ring>,
}
impl StreamControl {
    pub fn stop(&self) {
        self.ring.cancel();
    }
    pub fn failed(&self) -> bool {
        self.ring.failed.load(Ordering::Acquire)
    }
}
pub(super) struct StreamSamples {
    ring: Arc<Ring>,
    channels: u16,
    rate: u32,
    underruns: Arc<AtomicU64>,
    right: Option<f32>,
}
impl StreamSamples {
    pub fn control(&self) -> StreamControl {
        StreamControl {
            ring: self.ring.clone(),
        }
    }
}
impl Drop for StreamSamples {
    fn drop(&mut self) {
        self.ring.cancel();
    }
}
impl Iterator for StreamSamples {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.ring.cancelled.load(Ordering::Acquire) {
            return None;
        }
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        // End's acquire precedes the availability read: never discard the producer's
        // final frame, and never insert half a stereo silence frame on an underrun.
        let ended = self.ring.end.load(Ordering::Acquire);
        if self.ring.available() >= self.channels as usize {
            let first = self.ring.pop();
            if self.channels == 2 {
                self.right = self.ring.pop();
            }
            return first;
        }
        if ended {
            return None;
        }
        self.underruns
            .fetch_add(self.channels as u64, Ordering::Relaxed);
        if self.channels == 2 {
            self.right = Some(0.);
        }
        self.ring.wake();
        Some(0.)
    }
}
impl Source for StreamSamples {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
pub(super) fn stream_source(
    bytes: Arc<[u8]>,
    looping: bool,
    decoded: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
) -> Result<StreamSamples> {
    let mut decoder = Decoder::try_from(Cursor::new(bytes.clone())).map_err(|e| e.to_string())?;
    let channels = decoder.channels();
    let rate = decoder.sample_rate();
    validate_format(channels, rate)?;
    let ring = Arc::new(Ring::new());
    let mut initial = 0u64;
    for sample in decoder.by_ref().take(4096) {
        if !sample.is_finite() {
            return Err("nonfinite streamed sound".into());
        }
        ring.push(sample);
        initial += 1;
    }
    if initial == 0 {
        return Err("empty streamed sound".into());
    }
    decoded.fetch_add(initial, Ordering::Relaxed);
    let producer = ring.clone();
    thread::Builder::new()
        .name("audio-stream-decoder".into())
        .spawn(move || {
            let _ = producer.producer.set(thread::current());
            let bound = rate as u64 * channels as u64 * 1200;
            let mut iteration = initial;
            let mut batch = 0u64;
            'decode: loop {
                if producer.cancelled.load(Ordering::Acquire) {
                    break;
                }
                let sample = match decoder.next() {
                    Some(v) if v.is_finite() && iteration < bound => v,
                    Some(_) => {
                        producer.failed.store(true, Ordering::Release);
                        break;
                    }
                    None if looping && iteration > 0 => {
                        match Decoder::try_from(Cursor::new(bytes.clone())) {
                            Ok(d) => decoder = d,
                            Err(_) => {
                                producer.failed.store(true, Ordering::Release);
                                break;
                            }
                        }
                        iteration = 0;
                        continue;
                    }
                    None => break,
                };
                while !producer.push(sample) {
                    if producer.cancelled.load(Ordering::Acquire) {
                        break 'decode;
                    }
                    // unpark tokens close the full-ring -> park race without taking a callback lock.
                    thread::park();
                }
                iteration += 1;
                batch += 1;
                if batch == 4096 {
                    decoded.fetch_add(batch, Ordering::Relaxed);
                    batch = 0;
                }
            }
            decoded.fetch_add(batch, Ordering::Relaxed);
            producer.end.store(true, Ordering::Release);
        })
        .map_err(|e| e.to_string())?;
    Ok(StreamSamples {
        ring,
        channels,
        rate,
        underruns,
        right: None,
    })
}

#[derive(Default)]
pub(super) struct Gains {
    pub left: AtomicU32,
    pub right: AtomicU32,
}
impl Gains {
    pub fn set(&self, g: [f32; 2]) {
        self.left.store(g[0].to_bits(), Ordering::Relaxed);
        self.right.store(g[1].to_bits(), Ordering::Relaxed);
    }
}
/// Only session/worker computes spatial parameters. The callback averages one input frame
/// and reads two atomics: no locks, IO, decoding or per-frame allocations here.
pub(super) struct Positioned {
    source: Box<dyn Source<Item = f32> + Send>,
    gains: Arc<Gains>,
    spatial: bool,
    right: Option<f32>,
    rate: u32,
    channels: u16,
    live: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    expected_epoch: u64,
    finished: bool,
}
impl Positioned {
    pub fn new(
        source: Box<dyn Source<Item = f32> + Send>,
        gains: Arc<Gains>,
        spatial: bool,
        live: Arc<AtomicU64>,
        epoch: Arc<AtomicU64>,
        expected_epoch: u64,
    ) -> Self {
        let channels = source.channels();
        let rate = source.sample_rate();
        live.fetch_add(1, Ordering::Relaxed);
        Self {
            source,
            gains,
            spatial,
            right: None,
            rate,
            channels,
            live,
            epoch,
            expected_epoch,
            finished: false,
        }
    }
    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            self.live.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
impl Drop for Positioned {
    fn drop(&mut self) {
        self.finish();
    }
}
impl Iterator for Positioned {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.epoch.load(Ordering::Acquire) != self.expected_epoch {
            self.finish();
            return None;
        }
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        let Some(first) = self.source.next() else {
            self.finish();
            return None;
        };
        let second = if self.channels == 2 {
            self.source.next().unwrap_or(first)
        } else {
            first
        };
        let (left, right) = if self.spatial {
            let mono = (first + second) * 0.5;
            (mono, mono)
        } else {
            (first, second)
        };
        self.right = Some(right * f32::from_bits(self.gains.right.load(Ordering::Relaxed)));
        Some(left * f32::from_bits(self.gains.left.load(Ordering::Relaxed)))
    }
}
impl Source for Positioned {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
}
/// Minecraft's OpenAL linear-clamped distance model: reference 0, rolloff 1,
/// maximum = max(instance volume,1) * leaf attenuation_distance. Not inverse-square.
pub(super) fn spatial_gains(
    source: [f64; 3],
    listener: [f64; 3],
    yaw: f32,
    pitch: f32,
    range: f32,
) -> ([f32; 2], [f32; 3]) {
    let delta: [f64; 3] = std::array::from_fn(|i| source[i] - listener[i]);
    let distance = (delta.iter().map(|d| d * d).sum::<f64>()).sqrt();
    let y = f64::from(yaw).to_radians();
    let p = f64::from(pitch).to_radians();
    let right = [-y.cos(), 0., -y.sin()];
    let forward = [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()];
    let up = [-y.sin() * p.sin(), p.cos(), y.cos() * p.sin()];
    let dot = |basis: [f64; 3]| -> f64 { delta.iter().zip(basis).map(|(a, b)| a * b).sum() };
    let local = [dot(right) as f32, dot(up) as f32, dot(forward) as f32];
    let pan = if distance > 0. {
        (f64::from(local[0]) / distance).clamp(-1., 1.) as f32
    } else {
        0.
    };
    let attenuation = (1. - distance / range.max(0.0001) as f64).clamp(0., 1.) as f32;
    // Constant-power stereo projection, normalized so centered mono retains its amplitude.
    (
        [
            attenuation * (1. - pan).sqrt(),
            attenuation * (1. + pan).sqrt(),
        ],
        local,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streamed_stereo_underrun_does_not_swap_channels_or_drop_final_frame() {
        let ring = Arc::new(Ring::new());
        let underruns = Arc::new(AtomicU64::new(0));
        let mut source = StreamSamples {
            ring: ring.clone(),
            channels: 2,
            rate: 48000,
            underruns: underruns.clone(),
            right: None,
        };
        ring.push(2.);
        assert_eq!(source.next(), Some(0.));
        assert_eq!(source.next(), Some(0.));
        assert_eq!(underruns.load(Ordering::Relaxed), 2);
        ring.push(4.);
        ring.end.store(true, Ordering::Release);
        assert_eq!(source.next(), Some(2.));
        assert_eq!(source.next(), Some(4.));
        assert_eq!(source.next(), None);
    }
    #[test]
    fn linear_boundary_orientation_and_large_world_coordinates() {
        let (g, local) = spatial_gains(
            [30_000_000., 80., 30_000_008.],
            [30_000_000., 80., 30_000_000.],
            0.,
            0.,
            16.,
        );
        assert_eq!(g, [0.5, 0.5]);
        assert_eq!(local, [0., 0., 8.]);
        assert_eq!(
            spatial_gains([0., 0., 16.], [0.; 3], 0., 0., 16.).0,
            [0., 0.]
        );
        let (g, _) = spatial_gains([-8., 0., 0.], [0.; 3], 0., 0., 16.);
        assert_eq!(g[0], 0.);
        assert!(g[1] > 0.7);
        let (turned, local) = spatial_gains([-8., 0., 0.], [0.; 3], 180., 45., 16.);
        assert!(turned[0] > 0.7 && turned[1] < 0.0001);
        assert!(local[2].abs() < 0.0001);
    }
    #[test]
    fn looping_samples_and_completion_preserve_frames() {
        let mut s = SharedSamples::new(Arc::new(vec![1., 2., 3., 4.]), 2, 48000, true);
        assert_eq!(
            (&mut s).take(7).collect::<Vec<_>>(),
            vec![1., 2., 3., 4., 1., 2., 3.]
        );
        let live = Arc::new(AtomicU64::new(0));
        let gains = Arc::new(Gains::default());
        gains.set([0.5, 1.]);
        let source = Box::new(SharedSamples::new(Arc::new(vec![2., 4.]), 2, 48000, false));
        let mut p = Positioned::new(
            source,
            gains,
            true,
            live.clone(),
            Arc::new(AtomicU64::new(0)),
            0,
        );
        assert_eq!(p.next(), Some(1.5));
        assert_eq!(p.next(), Some(3.));
        assert_eq!(p.next(), None);
        assert_eq!(live.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn dimension_fence_ends_a_loop_even_before_worker_drains_commands() {
        let live = Arc::new(AtomicU64::new(0));
        let epoch = Arc::new(AtomicU64::new(7));
        let gains = Arc::new(Gains::default());
        gains.set([1., 1.]);
        let source = Box::new(SharedSamples::new(Arc::new(vec![0.25]), 1, 48000, true));
        let mut output = Positioned::new(source, gains, true, live.clone(), epoch.clone(), 7);
        assert_eq!(output.next(), Some(0.25));
        epoch.store(8, Ordering::Release);
        assert_eq!(output.next(), None);
        assert_eq!(live.load(Ordering::Relaxed), 0);
    }
}
