//! Native rodio/cpal output. A submitted voice is never claimed to be audible-output proof.
mod catalog;
mod materials;
mod playback;
mod resources;
use crate::protocol::Result;
use catalog::identifier;
pub use materials::{BlockSoundAction, BlockSounds};
use parking_lot::Mutex;
use playback::{Gains, Positioned, StreamControl, spatial_gains};
use resources::Resources;
use rodio::{
    OutputStream, OutputStreamBuilder, Sink,
    cpal::{
        self,
        traits::{DeviceTrait, HostTrait},
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Instant,
};

/// Exact 26.1.2 SoundSource enum order, including the new UI category at wire ID 10.
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundCategory {
    Master = 0,
    Music = 1,
    Records = 2,
    Weather = 3,
    Blocks = 4,
    Hostile = 5,
    Neutral = 6,
    Players = 7,
    Ambient = 8,
    Voice = 9,
    Ui = 10,
}
impl SoundCategory {
    pub fn from_wire(id: i32) -> Result<Self> {
        match id {
            0 => Ok(Self::Master),
            1 => Ok(Self::Music),
            2 => Ok(Self::Records),
            3 => Ok(Self::Weather),
            4 => Ok(Self::Blocks),
            5 => Ok(Self::Hostile),
            6 => Ok(Self::Neutral),
            7 => Ok(Self::Players),
            8 => Ok(Self::Ambient),
            9 => Ok(Self::Voice),
            10 => Ok(Self::Ui),
            _ => Err(format!("unknown sound category {id}")),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SoundOrigin {
    Position([f64; 3]),
    Entity {
        id: i32,
        generation: u64,
        position: [f64; 3],
    },
    Relative,
}
impl SoundOrigin {
    fn position(self) -> Option<[f64; 3]> {
        match self {
            Self::Position(p) | Self::Entity { position: p, .. } => Some(p),
            Self::Relative => None,
        }
    }
    fn matches_entity(self, id: i32, generation: u64) -> bool {
        matches!(self,Self::Entity{id:i,generation:g,..} if i==id && g==generation)
    }
}
#[derive(Clone, Debug)]
pub struct SoundRequest {
    pub event: String,
    pub category: SoundCategory,
    pub source: SoundOrigin,
    pub volume: f32,
    pub pitch: f32,
    pub seed: u64,
    pub looping: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SoundHandle {
    epoch: u64,
    id: u64,
}
#[derive(Clone, Debug)]
pub struct SoundSubtitle {
    pub handle: SoundHandle,
    pub translation_key: String,
    pub position: Option<[f64; 3]>,
    pub created: Instant,
}
impl SoundRequest {
    pub fn positional(
        event: &str,
        category: SoundCategory,
        position: [f64; 3],
        volume: f32,
        pitch: f32,
        seed: u64,
    ) -> Self {
        Self {
            event: event.into(),
            category,
            source: SoundOrigin::Position(position),
            volume,
            pitch,
            seed,
            looping: false,
        }
    }
    pub fn relative(
        event: &str,
        category: SoundCategory,
        volume: f32,
        pitch: f32,
        seed: u64,
    ) -> Self {
        Self {
            event: event.into(),
            category,
            source: SoundOrigin::Relative,
            volume,
            pitch,
            seed,
            looping: false,
        }
    }
}

/// A verified bundle clip, decoded once during admission, never from a runtime path.
/// Matches loader-platform-common: Vorbis, mono 8..48 kHz, at most two seconds.
pub(crate) struct PreparedSound {
    samples: Arc<Vec<f32>>,
    rate: u32,
}
impl PreparedSound {
    pub(crate) fn from_ogg(bytes: Arc<[u8]>) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > 256 * 1024 {
            return Err("Loader sound encoded byte bound".into());
        }
        use symphonia::core::{
            audio::SampleBuffer,
            codecs::{CODEC_TYPE_VORBIS, DecoderOptions},
            errors::Error,
            formats::FormatOptions,
            io::MediaSourceStream,
            meta::MetadataOptions,
            probe::Hint,
        };
        let (setup, frames) = validate_bundle_ogg(&bytes)?;
        validate_vorbis_codebooks(&setup)?;
        let input =
            MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("ogg");
        let mut format = symphonia::default::get_probe()
            .format(
                &hint,
                input,
                &FormatOptions {
                    enable_gapless: true,
                    ..FormatOptions::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| format!("invalid Loader Ogg sound: {e}"))?
            .format;
        if format.tracks().len() != 1 {
            return Err("Loader sound requires one Vorbis track".into());
        }
        let track = format.default_track().ok_or("Loader sound track missing")?;
        let rate = track
            .codec_params
            .sample_rate
            .ok_or("Loader sound rate missing")?;
        if track.codec_params.codec != CODEC_TYPE_VORBIS
            || track
                .codec_params
                .channels
                .is_none_or(|channels| channels.count() != 1)
            || !(8000..=48000).contains(&rate)
        {
            return Err("Loader sound must be mono Vorbis at 8..48 kHz".into());
        }
        let limit = (rate as usize * 2).min(96_000);
        let frames = usize::try_from(frames).map_err(|_| "Loader sound frame bound")?;
        if frames == 0 || frames > limit {
            return Err("Loader sound exceeds two decoded seconds or is empty".into());
        }
        // At most one maximum Vorbis block of encoder padding; PCM playback ends at
        // the verified final granule. Symphonia cannot trim a single audio-page clip.
        let decode_limit = frames + 8192;
        let track_id = track.id;
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions { verify: true })
            .map_err(|e| format!("invalid Loader Vorbis sound: {e}"))?;
        let mut samples = Vec::with_capacity(decode_limit);
        let mut converted = None;
        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("invalid Loader Ogg packet: {e}")),
            };
            if packet.track_id() != track_id {
                return Err("Loader sound changed track".into());
            }
            let decoded = decoder
                .decode(&packet)
                .map_err(|e| format!("invalid Loader Vorbis samples: {e}"))?;
            if decoded.spec().channels.count() != 1
                || decoded.spec().rate != rate
                || decoded.capacity() > limit
            {
                return Err("Loader sound changed format or exceeded decode bound".into());
            }
            if converted
                .as_ref()
                .is_none_or(|buffer: &SampleBuffer<f32>| buffer.capacity() < decoded.capacity())
            {
                converted = Some(SampleBuffer::<f32>::new(
                    decoded.capacity() as u64,
                    *decoded.spec(),
                ));
            }
            let buffer = converted
                .as_mut()
                .ok_or("Loader sound conversion buffer missing")?;
            buffer.copy_interleaved_ref(decoded);
            if buffer.samples().len() > decode_limit - samples.len() {
                return Err("Loader sound decoded sample/padding bound".into());
            }
            if buffer.samples().iter().any(|sample| !sample.is_finite()) {
                return Err("Loader sound contains nonfinite PCM".into());
            }
            samples.extend_from_slice(buffer.samples());
        }
        if decoder.finalize().verify_ok == Some(false) {
            return Err("Loader Vorbis verification failed".into());
        }
        if samples.len() < frames {
            return Err("Loader sound decoded fewer frames than its final granule".into());
        }
        samples.truncate(frames);
        Ok(Self {
            samples: Arc::new(samples),
            rate,
        })
    }
}

// Symphonia's Ogg reader deliberately resynchronizes past corrupt pages. Bundle admission
// must instead reject corruption/truncation. Layout and CRC use its pinned page.rs contract.
fn validate_bundle_ogg(bytes: &[u8]) -> Result<(Vec<u8>, u64)> {
    use symphonia::core::{checksum::Crc32, io::Monitor};
    let mut offset = 0;
    let mut serial = None;
    let mut sequence = 0u32;
    let mut ended = false;
    let mut final_granule = None;
    let mut packets = 0usize;
    let mut setup = Vec::new();
    while offset < bytes.len() {
        let header = bytes
            .get(offset..offset + 27)
            .ok_or("truncated Loader Ogg page")?;
        if ended || &header[..4] != b"OggS" || header[4] != 0 || header[5] & 0xf8 != 0 {
            return Err("invalid Loader Ogg page header".into());
        }
        let flags = header[5];
        let page_serial = u32::from_le_bytes(
            header[14..18]
                .try_into()
                .map_err(|_| "Loader Ogg serial missing")?,
        );
        let page_sequence = u32::from_le_bytes(
            header[18..22]
                .try_into()
                .map_err(|_| "Loader Ogg sequence missing")?,
        );
        if serial.is_none() {
            if flags & 3 != 2 {
                return Err("Loader Ogg beginning page missing".into());
            }
            serial = Some(page_serial);
        } else if flags & 2 != 0 || serial != Some(page_serial) {
            return Err("Loader Ogg requires one logical stream".into());
        }
        if page_sequence != sequence {
            return Err("Loader Ogg page sequence mismatch".into());
        }
        sequence = sequence
            .checked_add(1)
            .ok_or("Loader Ogg page sequence bound")?;
        let segments_end = offset + 27 + usize::from(header[26]);
        let segments = bytes
            .get(offset + 27..segments_end)
            .ok_or("truncated Loader Ogg segments")?;
        let body: usize = segments.iter().map(|n| usize::from(*n)).sum();
        let end = segments_end + body;
        let page = bytes
            .get(offset..end)
            .ok_or("truncated Loader Ogg page body")?;
        let expected = u32::from_le_bytes(
            header[22..26]
                .try_into()
                .map_err(|_| "Loader Ogg checksum missing")?,
        );
        let mut crc = Crc32::new(0);
        crc.process_buf_bytes(&page[..22]);
        crc.process_buf_bytes(&[0; 4]);
        crc.process_buf_bytes(&page[26..]);
        if crc.crc() != expected {
            return Err("Loader Ogg page checksum mismatch".into());
        }
        let mut data_offset = segments_end;
        for segment in segments {
            let size = usize::from(*segment);
            if packets == 2 {
                setup.extend_from_slice(&bytes[data_offset..data_offset + size]);
            }
            data_offset += size;
            if size != 255 {
                packets += 1;
            }
        }
        ended = flags & 4 != 0;
        if ended {
            final_granule = Some(u64::from_le_bytes(
                header[6..14]
                    .try_into()
                    .map_err(|_| "Loader Ogg granule missing")?,
            ));
        }
        offset = end;
    }
    if !ended {
        return Err("Loader Ogg end page missing".into());
    }
    Ok((
        setup,
        final_granule.ok_or("Loader Ogg final granule missing")?,
    ))
}

// Vorbis setup can describe huge vector tables in a tiny encoded file. Bound their
// allocation before Symphonia probes/constructs the codec, not after decoding PCM.
fn validate_vorbis_codebooks(setup: &[u8]) -> Result<()> {
    use symphonia::core::io::{BitReaderRtl, ReadBitsRtl};
    if !setup.starts_with(b"\x05vorbis") {
        return Err("Loader Vorbis setup header missing".into());
    }
    let mut bits = BitReaderRtl::new(&setup[7..]);
    let checked = (|| -> std::io::Result<()> {
        let count = bits.read_bits_leq32(8)? + 1;
        let mut entries_total = 0u32;
        let mut vectors_total = 0u64;
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Loader Vorbis codebook allocation/format bound",
            )
        };
        for _ in 0..count {
            if bits.read_bits_leq32(24)? != 0x564342 {
                return Err(invalid());
            }
            let dimensions = bits.read_bits_leq32(16)?;
            let entries = bits.read_bits_leq32(24)?;
            entries_total = entries_total.checked_add(entries).ok_or_else(invalid)?;
            vectors_total += u64::from(entries) * u64::from(dimensions);
            if !(1..=16).contains(&dimensions)
                || entries == 0
                || entries_total > 65_536
                || vectors_total > 1_048_576
            {
                return Err(invalid());
            }
            if bits.read_bool()? {
                let mut length = bits.read_bits_leq32(5)? + 1;
                let mut read = 0;
                while read < entries {
                    if length > 32 {
                        return Err(invalid());
                    }
                    read += bits.read_bits_leq32(32 - (entries - read).leading_zeros())?;
                    if read > entries {
                        return Err(invalid());
                    }
                    length += 1;
                }
            } else if bits.read_bool()? {
                for _ in 0..entries {
                    if bits.read_bool()? {
                        bits.ignore_bits(5)?;
                    }
                }
            } else {
                bits.ignore_bits(entries * 5)?;
            }
            match bits.read_bits_leq32(4)? {
                0 => {}
                lookup @ (1 | 2) => {
                    bits.ignore_bits(64)?;
                    let value_bits = bits.read_bits_leq32(4)? + 1;
                    bits.ignore_bits(1)?;
                    let values = if lookup == 2 {
                        entries * dimensions
                    } else {
                        // Exact integer nth root avoids the decoder's float/pow overflow.
                        let (mut low, mut high) = (1u32, entries);
                        while low < high {
                            let middle = low + (high - low).div_ceil(2);
                            if middle
                                .checked_pow(dimensions)
                                .is_some_and(|value| value <= entries)
                            {
                                low = middle;
                            } else {
                                high = middle - 1;
                            }
                        }
                        low
                    };
                    bits.ignore_bits(values * value_bits)?;
                }
                _ => return Err(invalid()),
            }
        }
        Ok(())
    })();
    checked.map_err(|error| error.to_string())
}
#[derive(Clone, Copy)]
struct Listener {
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
}
enum Command {
    Play(u64, u64, SoundRequest),
    PlayVerified(u64, u64, SoundRequest, Arc<PreparedSound>),
    StopVoice(SoundHandle),
    Parameters(SoundHandle, f32, f32),
    Listener(u64, Listener),
    Entity(u64, i32, u64, [f64; 3]),
    RemoveEntity(u64, i32, u64),
    StopSound(u64, Option<SoundCategory>, Option<String>),
    Volume(SoundCategory, f32),
    Focus(bool, bool),
    Pause(bool),
    Reload(u64),
    Device(Option<String>),
    Wake,
}
struct Shared {
    state: Mutex<Value>,
    epoch: Arc<AtomicU64>,
    next_voice: AtomicU64,
    subtitles: Mutex<VecDeque<SoundSubtitle>>,
    shutdown: AtomicBool,
    device_failed: AtomicBool,
    live: Arc<AtomicU64>,
    decoded: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
}
pub struct Audio {
    sender: SyncSender<Command>,
    shared: Arc<Shared>,
}
impl Audio {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(128);
        let shared = Arc::new(Shared {
            state: Mutex::new(
                json!({"backend":"rodio/cpal","status":"initializing","audible_output_verified":false,"submitted_events":"0"}),
            ),
            epoch: Arc::new(AtomicU64::new(0)),
            next_voice: AtomicU64::new(1),
            subtitles: Mutex::new(VecDeque::with_capacity(64)),
            shutdown: AtomicBool::new(false),
            device_failed: AtomicBool::new(false),
            live: Arc::new(AtomicU64::new(0)),
            decoded: Arc::new(AtomicU64::new(0)),
            underruns: Arc::new(AtomicU64::new(0)),
        });
        let worker_shared = shared.clone();
        let wake = sender.clone();
        if let Err(error) = thread::Builder::new()
            .name("native-audio".into())
            .spawn(move || {
                if let Err(error) = worker(receiver, wake, &worker_shared) {
                    let mut state = worker_shared.state.lock();
                    state["status"] = Value::from("unavailable");
                    state["last_error"] = Value::from(error);
                }
            })
        {
            shared.state.lock()["last_error"] = Value::from(error.to_string());
            shared.state.lock()["status"] = Value::from("unavailable");
        }
        Self { sender, shared }
    }
    fn epoch(&self) -> u64 {
        self.shared.epoch.load(Ordering::Acquire)
    }
    fn send(&self, command: Command) -> Result<()> {
        self.sender.try_send(command).map_err(|_| {
            let error = "bounded audio command queue unavailable".to_owned();
            self.shared.state.lock()["last_error"] = Value::from(error.clone());
            error
        })
    }
    /// The returned handle means queued, not audible or even mixer-submitted.
    pub fn play(&self, mut request: SoundRequest) -> Result<SoundHandle> {
        request.event = identifier(&request.event)?;
        if !request.volume.is_finite()
            || request.volume < 0.
            || request.volume > 65536.
            || !request.pitch.is_finite()
            || request.pitch <= 0.
            || request.pitch > 65536.
            || request
                .source
                .position()
                .is_some_and(|p| p.iter().any(|n| !n.is_finite() || n.abs() > 1.0e12))
        {
            return Err("invalid sound request gain/position".into());
        }
        let handle = SoundHandle {
            epoch: self.epoch(),
            id: self.shared.next_voice.fetch_add(1, Ordering::Relaxed),
        };
        self.send(Command::Play(handle.epoch, handle.id, request))?;
        Ok(handle)
    }
    /// Queues admitted PCM; this handle is not output or audibility evidence.
    pub(crate) fn play_verified(
        &self,
        mut request: SoundRequest,
        clip: Arc<PreparedSound>,
    ) -> Result<SoundHandle> {
        request.event = identifier(&request.event)?;
        if request.category != SoundCategory::Master
            || request.looping
            || !request.volume.is_finite()
            || !(0. ..=1.).contains(&request.volume)
            || !request.pitch.is_finite()
            || !(0.5..=2.).contains(&request.pitch)
            || request
                .source
                .position()
                .is_some_and(|p| p.iter().any(|v| !v.is_finite()))
        {
            return Err("invalid Loader sound playback request".into());
        }
        let handle = SoundHandle {
            epoch: self.epoch(),
            id: self.shared.next_voice.fetch_add(1, Ordering::Relaxed),
        };
        self.send(Command::PlayVerified(
            handle.epoch,
            handle.id,
            request,
            clip,
        ))?;
        Ok(handle)
    }
    pub fn listener(&self, position: [f64; 3], yaw: f32, pitch: f32) -> Result<()> {
        if position.iter().any(|v| !v.is_finite() || v.abs() > 1.0e12)
            || !yaw.is_finite()
            || !pitch.is_finite()
        {
            return Err("invalid audio listener transform".into());
        }
        self.send(Command::Listener(
            self.epoch(),
            Listener {
                position,
                yaw,
                pitch,
            },
        ))
    }
    pub fn entity_position(&self, id: i32, generation: u64, position: [f64; 3]) -> Result<()> {
        if position.iter().any(|v| !v.is_finite() || v.abs() > 1.0e12) {
            return Err("invalid sound entity position".into());
        }
        self.send(Command::Entity(self.epoch(), id, generation, position))
    }
    pub fn remove_entity(&self, id: i32, generation: u64) -> Result<()> {
        self.send(Command::RemoveEntity(self.epoch(), id, generation))
    }
    pub fn stop_voice(&self, handle: SoundHandle) -> Result<()> {
        if handle.epoch != self.epoch() {
            return Err("stale sound handle".into());
        }
        self.send(Command::StopVoice(handle))
    }
    pub fn voice_parameters(&self, handle: SoundHandle, volume: f32, pitch: f32) -> Result<()> {
        if handle.epoch != self.epoch() {
            return Err("stale sound handle".into());
        }
        if !volume.is_finite()
            || !(0. ..=65536.).contains(&volume)
            || !pitch.is_finite()
            || !(0.0001..=65536.).contains(&pitch)
        {
            return Err("invalid live sound gain/pitch".into());
        }
        self.send(Command::Parameters(handle, volume, pitch))
    }
    pub fn stop_sound(&self, category: Option<SoundCategory>, event: Option<&str>) -> Result<()> {
        self.send(Command::StopSound(
            self.epoch(),
            category,
            event.map(identifier).transpose()?,
        ))
    }
    /// Connection/dimension fence. Even if the queue is full, the worker sees the atomic
    /// epoch before its next command and destroys voices before any new-generation play.
    pub fn stop(&self) {
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.sender.try_send(Command::Wake);
    }
    pub fn reload(&self) -> Result<()> {
        self.send(Command::Reload(self.epoch()))
    }
    pub fn set_volume(&self, category: SoundCategory, volume: f32) -> Result<()> {
        if !volume.is_finite() || !(0. ..=1.).contains(&volume) {
            return Err("audio volume must be finite 0..1".into());
        }
        self.send(Command::Volume(category, volume))
    }
    pub fn set_focus(&self, focused: bool, mute_unfocused: bool) -> Result<()> {
        self.send(Command::Focus(focused, mute_unfocused))
    }
    /// Match the target's local-world pause: music and UI remain live. Opening a
    /// multiplayer menu alone is not a simulation pause and must not call this.
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.send(Command::Pause(paused))
    }
    /// Select an existing output device by its exact cpal name, or explicitly reopen the
    /// current default. Never changes the OS sink/default or silently falls back.
    pub fn output_device(&self, name: Option<&str>) -> Result<()> {
        if name.is_some_and(|s| s.len() > 1024) {
            return Err("audio device name bound".into());
        }
        self.send(Command::Device(name.map(str::to_owned)))
    }
    /// Drain actual selected sound events for the ordinary subtitle UI; not output proof.
    pub fn subtitles(&self) -> Vec<SoundSubtitle> {
        let epoch = self.epoch();
        self.shared
            .subtitles
            .lock()
            .drain(..)
            .filter(|s| s.handle.epoch == epoch)
            .collect()
    }
    pub fn observation(&self) -> Value {
        let mut v = self.shared.state.lock().clone();
        v["active_voices"] = Value::from(self.shared.live.load(Ordering::Relaxed));
        v["decoded_samples"] = Value::from(self.shared.decoded.load(Ordering::Relaxed).to_string());
        v["stream_underrun_samples"] =
            Value::from(self.shared.underruns.load(Ordering::Relaxed).to_string());
        v["audio_epoch"] = Value::from(self.epoch().to_string());
        v
    }
}
impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for Audio {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.stop();
    }
}
struct Voice {
    id: u64,
    sample_volume: f32,
    sample_pitch: f32,
    attenuation: f32,
    request: SoundRequest,
    sink: Sink,
    gains: Arc<Gains>,
    stream: Option<StreamControl>,
    volume: f32,
    range: f32,
    local: [f32; 3],
}
impl Voice {
    fn stop(&self) {
        if let Some(stream) = &self.stream {
            stream.stop();
        }
        self.sink.stop();
    }
    fn update(&mut self, listener: Listener, volumes: &[f32; 11], muted: bool) {
        let (gain, local) = match self.request.source.position() {
            Some(p) => spatial_gains(
                p,
                listener.position,
                listener.yaw,
                listener.pitch,
                self.range,
            ),
            None => ([1., 1.], [0.; 3]),
        };
        self.local = local;
        self.gains.set(gain);
        let master = volumes[0];
        let category = if self.request.category == SoundCategory::Master {
            1.
        } else {
            volumes[self.request.category as usize]
        };
        self.sink.set_volume(if muted {
            0.
        } else {
            self.volume.clamp(0., 1.) * master * category
        });
    }
}
impl Drop for Voice {
    fn drop(&mut self) {
        self.stop();
    }
}
fn stop_matches(
    request: &SoundRequest,
    category: Option<SoundCategory>,
    event: Option<&str>,
) -> bool {
    category.is_none_or(|c| c == request.category) && event.is_none_or(|e| e == request.event)
}
fn open_device(
    name: Option<&str>,
    sender: &SyncSender<Command>,
    shared: &Arc<Shared>,
) -> Result<(OutputStream, String)> {
    let host = cpal::default_host();
    let device = if let Some(name) = name {
        let mut matches = host
            .output_devices()
            .map_err(|e| e.to_string())?
            .filter(|d| d.name().ok().as_deref() == Some(name));
        let device = matches
            .next()
            .ok_or("selected native audio device not found")?;
        if matches.next().is_some() {
            return Err("selected native audio device name is ambiguous".into());
        }
        device
    } else {
        host.default_output_device()
            .ok_or("no native output device")?
    };
    let name = device.name().map_err(|e| e.to_string())?;
    let wake = sender.clone();
    let failure = shared.clone();
    let mut stream = OutputStreamBuilder::from_device(device)
        .map_err(|e| e.to_string())?
        .with_error_callback(move |_error| {
            failure.device_failed.store(true, Ordering::Release);
            failure.epoch.fetch_add(1, Ordering::AcqRel);
            let _ = wake.try_send(Command::Wake);
        })
        .open_stream()
        .map_err(|e| e.to_string())?;
    stream.log_on_drop(false);
    Ok((stream, name))
}
fn device_observation(shared: &Shared, stream: &OutputStream, name: &str) {
    let mut state = shared.state.lock();
    state["status"] = Value::from("device_open");
    state["device"] = Value::from(name);
    state["sample_rate"] = Value::from(stream.config().sample_rate());
    state["channels"] = Value::from(stream.config().channel_count());
}
fn worker(
    receiver: Receiver<Command>,
    sender: SyncSender<Command>,
    shared: &Arc<Shared>,
) -> Result<()> {
    let mut resources = Resources::load(&shared.decoded)?;
    let mut stream = match open_device(None, &sender, shared) {
        Ok((stream, name)) => {
            device_observation(shared, &stream, &name);
            Some(stream)
        }
        Err(error) => {
            let mut state = shared.state.lock();
            state["status"] = Value::from("unavailable");
            state["last_error"] = Value::from(error);
            None
        }
    };
    {
        let mut state = shared.state.lock();
        state["catalog_sha1"] = Value::from(resources.catalog_hash.clone());
        state["catalog_events"] = Value::from(resources.catalog.len());
        state["resource_epoch"] = Value::from("0");
        state["category_volumes"] = json!(([1.0f32; 11]));
    }
    let mut voices: VecDeque<Voice> = VecDeque::with_capacity(64);
    let mut listener = Listener {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
    };
    let mut volumes = [1.; 11];
    let mut muted = false;
    let mut paused = false;
    let mut active_epoch = shared.epoch.load(Ordering::Acquire);
    let mut resource_epoch = 0u64;
    let mut submitted = 0u64;
    while let Ok(command) = receiver.recv() {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        let current = shared.epoch.load(Ordering::Acquire);
        if current != active_epoch {
            voices.clear();
            listener = Listener {
                position: [0.; 3],
                yaw: 0.,
                pitch: 0.,
            };
            active_epoch = current;
        }
        if shared.device_failed.swap(false, Ordering::AcqRel) {
            voices.clear();
            stream = None;
            shared.state.lock()["status"] = Value::from("device_lost");
        }
        voices.retain(|v| !v.sink.empty());
        if voices
            .iter()
            .any(|v| v.stream.as_ref().is_some_and(StreamControl::failed))
        {
            shared.state.lock()["last_error"] =
                Value::from("stream decoder failed sample/duration/format bound");
        }
        let result: Result<()> = match command {
            Command::Wake => Ok(()),
            Command::StopVoice(handle) if handle.epoch == active_epoch => {
                voices.retain(|v| v.id != handle.id);
                Ok(())
            }
            Command::Parameters(handle, volume, pitch) if handle.epoch == active_epoch => {
                if let Some(v) = voices.iter_mut().find(|v| v.id == handle.id) {
                    let effective = volume * v.sample_volume;
                    let range = effective.max(1.) * v.attenuation;
                    if !effective.is_finite() || !range.is_finite() {
                        Err("live sound gain/range overflow".into())
                    } else {
                        v.request.volume = volume;
                        v.request.pitch = pitch;
                        v.volume = effective;
                        v.range = range;
                        v.sink.set_speed((pitch * v.sample_pitch).clamp(0.5, 2.));
                        Ok(())
                    }
                } else {
                    Ok(())
                }
            }
            Command::Listener(epoch, p) if epoch == active_epoch => {
                listener = p;
                Ok(())
            }
            Command::Entity(epoch, id, generation, p) if epoch == active_epoch => {
                for voice in &mut voices {
                    if voice.request.source.matches_entity(id, generation) {
                        voice.request.source = SoundOrigin::Entity {
                            id,
                            generation,
                            position: p,
                        };
                    }
                }
                Ok(())
            }
            Command::RemoveEntity(epoch, id, generation) if epoch == active_epoch => {
                voices.retain(|v| !v.request.source.matches_entity(id, generation));
                Ok(())
            }
            Command::StopSound(epoch, category, event) if epoch == active_epoch => {
                voices.retain(|v| !stop_matches(&v.request, category, event.as_deref()));
                Ok(())
            }
            Command::Volume(category, volume) => {
                volumes[category as usize] = volume;
                shared.state.lock()["category_volumes"] = json!(volumes);
                Ok(())
            }
            Command::Focus(focused, mute_unfocused) => {
                muted = !focused && mute_unfocused;
                shared.state.lock()["focus_muted"] = Value::from(muted);
                Ok(())
            }
            Command::Pause(pause) => {
                paused = pause;
                for voice in &voices {
                    if paused
                        && !matches!(
                            voice.request.category,
                            SoundCategory::Music | SoundCategory::Ui
                        )
                    {
                        voice.sink.pause();
                    } else {
                        voice.sink.play();
                    }
                }
                Ok(())
            }
            Command::Device(name) => match open_device(name.as_deref(), &sender, shared) {
                Ok((opened, name)) => {
                    voices.clear();
                    device_observation(shared, &opened, &name);
                    stream = Some(opened);
                    Ok(())
                }
                Err(error) => Err(error),
            },
            Command::Reload(epoch) if epoch == active_epoch => {
                match Resources::load(&shared.decoded) {
                    Ok(staged) => {
                        if shared.epoch.load(Ordering::Acquire) != active_epoch
                            || shared.shutdown.load(Ordering::Acquire)
                        {
                            continue;
                        }
                        voices.clear();
                        resources = staged;
                        resource_epoch += 1;
                        let mut state = shared.state.lock();
                        state["resource_epoch"] = Value::from(resource_epoch.to_string());
                        state["catalog_sha1"] = Value::from(resources.catalog_hash.clone());
                        state["catalog_events"] = Value::from(resources.catalog.len());
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            Command::PlayVerified(epoch, id, request, clip) if epoch == active_epoch => (|| {
                let output = stream.as_ref().ok_or("native audio device unavailable")?;
                // Loader replaces this event's previous voice, including zero-volume play.
                voices.retain(|v| {
                    !stop_matches(
                        &v.request,
                        Some(SoundCategory::Master),
                        Some(&request.event),
                    )
                });
                let range = request.volume.max(1.) * 16.;
                if request.volume == 0.
                    || volumes[0] == 0.
                    || request.source.position().is_some_and(|p| {
                        spatial_gains(p, listener.position, listener.yaw, listener.pitch, range).0
                            == [0., 0.]
                    })
                {
                    return Ok(());
                }
                if shared.epoch.load(Ordering::Acquire) != active_epoch {
                    return Ok(());
                }
                if voices.len() == 64 {
                    voices.pop_front();
                }
                let sink = Sink::connect_new(output.mixer());
                sink.set_speed(request.pitch);
                if paused {
                    sink.pause();
                }
                let source =
                    playback::SharedSamples::new(clip.samples.clone(), 1, clip.rate, false);
                let gains = Arc::new(Gains::default());
                let mut voice = Voice {
                    id,
                    sample_volume: 1.,
                    sample_pitch: 1.,
                    attenuation: 16.,
                    volume: request.volume,
                    range,
                    request,
                    sink,
                    gains: gains.clone(),
                    stream: None,
                    local: [0.; 3],
                };
                voice.update(listener, &volumes, muted);
                voice.sink.append(Positioned::new(
                    Box::new(source),
                    gains,
                    voice.request.source.position().is_some(),
                    shared.live.clone(),
                    shared.epoch.clone(),
                    active_epoch,
                ));
                submitted += 1;
                let mut state = shared.state.lock();
                state["submitted_events"] = Value::from(submitted.to_string());
                state["last_event"] = json!({"event":voice.request.event,"category":"master",
                    "source":voice.request.source,"volume":voice.volume,"pitch":voice.sink.speed(),
                    "looping":false,"attenuation_distance":range,"prepared_samples":clip.samples.len(),
                    "origin":"hash_verified_loader_asset","phase":"submitted_to_native_mixer"});
                voices.push_back(voice);
                Ok(())
            })(
            ),
            Command::Play(epoch, id, request) if epoch == active_epoch => (|| {
                let output = stream.as_ref().ok_or("native audio device unavailable")?;
                let selection = resources.catalog.select(&request.event, request.seed)?;
                if selection.stream && voices.iter().filter(|v| v.stream.is_some()).count() >= 8 {
                    return Err("stream voice bound".into());
                }
                let in_range = request.source.position().is_none_or(|p| {
                    spatial_gains(
                        p,
                        listener.position,
                        listener.yaw,
                        listener.pitch,
                        (request.volume * selection.volume).max(1.) * selection.attenuation,
                    )
                    .0 != [0., 0.]
                });
                if in_range {
                    if let Some(key) = resources.catalog.subtitle(&request.event) {
                        let mut subtitles = shared.subtitles.lock();
                        if subtitles.len() == 64 {
                            subtitles.pop_front();
                        }
                        subtitles.push_back(SoundSubtitle {
                            handle: SoundHandle { epoch, id },
                            translation_key: key.to_owned(),
                            position: request.source.position(),
                            created: Instant::now(),
                        });
                    }
                }
                if request.volume == 0.
                    || (!request.looping
                        && (volumes[0] == 0. || volumes[request.category as usize] == 0.))
                {
                    return Ok(());
                }
                let effective = request.volume * selection.volume;
                let range = effective.max(1.) * selection.attenuation;
                if !effective.is_finite() || !range.is_finite() {
                    return Err("sound gain/range overflow".into());
                }
                // Out-of-range one-shots are not decoded. Loops may move into hearing range.
                if !request.looping
                    && request.source.position().is_some_and(|p| {
                        spatial_gains(p, listener.position, listener.yaw, listener.pitch, range).0
                            == [0., 0.]
                    })
                {
                    return Ok(());
                }
                let (source, control) = resources.source(
                    &selection.name,
                    selection.stream,
                    request.looping,
                    shared.decoded.clone(),
                    shared.underruns.clone(),
                )?;
                if shared.epoch.load(Ordering::Acquire) != active_epoch
                    || shared.shutdown.load(Ordering::Acquire)
                {
                    if let Some(c) = control {
                        c.stop();
                    }
                    return Ok(());
                }
                if voices.len() == 64 {
                    voices.pop_front();
                }
                let sink = Sink::connect_new(output.mixer());
                sink.set_speed((request.pitch * selection.pitch).clamp(0.5, 2.));
                if paused && !matches!(request.category, SoundCategory::Music | SoundCategory::Ui) {
                    sink.pause();
                }
                let gains = Arc::new(Gains::default());
                let mut voice = Voice {
                    id,
                    sample_volume: selection.volume,
                    sample_pitch: selection.pitch,
                    attenuation: selection.attenuation,
                    request,
                    sink,
                    gains: gains.clone(),
                    stream: control,
                    volume: effective,
                    range,
                    local: [0.; 3],
                };
                voice.update(listener, &volumes, muted);
                voice.sink.append(Positioned::new(
                    source,
                    gains,
                    voice.request.source.position().is_some(),
                    shared.live.clone(),
                    shared.epoch.clone(),
                    active_epoch,
                ));
                submitted += 1;
                let mut state = shared.state.lock();
                state["submitted_events"] = Value::from(submitted.to_string());
                state["last_event"] = json!({"event":voice.request.event,"selected_asset":selection.name,"category":voice.request.category,"source":voice.request.source,"volume":voice.volume,"pitch":voice.sink.speed(),"looping":voice.request.looping,"stream":selection.stream,"attenuation_distance":range,"listener_relative_position":voice.local,"subtitle":resources.catalog.subtitle(&voice.request.event),"origin":"packet_or_local_game_input","phase":"submitted_to_native_mixer"});
                voices.push_back(voice);
                Ok(())
            })(),
            _ => Ok(()),
        };
        for voice in &mut voices {
            voice.update(listener, &volumes, muted);
        }
        let mut state = shared.state.lock();
        if let Err(error) = result {
            state["last_error"] = Value::from(error);
        }
        let cache_bytes = Value::from(resources.cache_bytes());
        if let Some(value) = state.get_mut("pcm_cache_bytes") {
            *value = cache_bytes;
        } else {
            state["pcm_cache_bytes"] = cache_bytes;
        }
        update_observation(&mut state, &voices, listener);
    }
    voices.clear();
    shared.state.lock()["status"] = Value::from("stopped");
    Ok(())
}
fn update_observation(state: &mut Value, voices: &VecDeque<Voice>, listener: Listener) {
    if state["listener"].is_null() {
        state["listener"] = json!({"position":[0.,0.,0.],"yaw":0.,"pitch":0.});
    }
    let listener_state = state
        .get_mut("listener")
        .expect("native listener observation schema");
    update_vector(
        listener_state
            .get_mut("position")
            .expect("listener position schema"),
        listener.position,
    );
    *listener_state.get_mut("yaw").expect("listener yaw schema") = Value::from(listener.yaw);
    *listener_state
        .get_mut("pitch")
        .expect("listener pitch schema") = Value::from(listener.pitch);
    let unchanged = state["voices"].as_array().is_some_and(|entries| {
        entries.len() == voices.len()
            && entries
                .iter()
                .zip(voices)
                .all(|(entry, voice)| entry["id"].as_u64() == Some(voice.id))
    });
    if !unchanged {
        state["voices"] = Value::Array(voices.iter().map(|v|json!({"id":v.id,"event":v.request.event,"category":v.request.category,"source":v.request.source,"looping":v.request.looping,"stream":v.stream.is_some(),"listener_relative_position":v.local,"gain":v.sink.volume(),"pitch":v.sink.speed(),"attenuation_distance":v.range})).collect());
    } else if let Some(entries) = state.get_mut("voices").and_then(Value::as_array_mut) {
        for (entry, voice) in entries.iter_mut().zip(voices) {
            *entry.get_mut("gain").expect("voice gain schema") = Value::from(voice.sink.volume());
            *entry.get_mut("pitch").expect("voice pitch schema") = Value::from(voice.sink.speed());
            *entry
                .get_mut("attenuation_distance")
                .expect("voice range schema") = Value::from(voice.range);
            update_vector(
                entry
                    .get_mut("listener_relative_position")
                    .expect("voice relative position schema"),
                voice.local.map(f64::from),
            );
            match voice.request.source {
                SoundOrigin::Position(p) => update_vector(
                    entry
                        .get_mut("source")
                        .and_then(|s| s.get_mut("value"))
                        .expect("positional voice schema"),
                    p,
                ),
                SoundOrigin::Entity { position: p, .. } => update_vector(
                    entry
                        .get_mut("source")
                        .and_then(|s| s.get_mut("value"))
                        .and_then(|s| s.get_mut("position"))
                        .expect("entity voice schema"),
                    p,
                ),
                SoundOrigin::Relative => {}
            }
        }
    }
}
fn update_vector(value: &mut Value, position: [f64; 3]) {
    for (value, coordinate) in value
        .as_array_mut()
        .expect("audio vector observation schema")
        .iter_mut()
        .zip(position)
    {
        *value = Value::from(coordinate);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundle_sound_rejects_compressed_codebook_allocation_bombs_before_decode() {
        let bytes =
            include_bytes!("../fixtures/content/assets/solaris_fixture/sounds/confirmation.ogg");
        let (setup, _) = validate_bundle_ogg(bytes).unwrap();
        validate_vorbis_codebooks(&setup).unwrap();
        let mut bomb = b"\x05vorbis".to_vec();
        bomb.push(0); // One codebook.
        bomb.extend_from_slice(&[0x42, 0x43, 0x56]); // Codebook marker.
        bomb.extend_from_slice(&u16::MAX.to_le_bytes());
        bomb.extend_from_slice(&[0xff, 0xff, 0]); // 65535 entries.
        assert!(
            validate_vorbis_codebooks(&bomb)
                .unwrap_err()
                .contains("allocation/format bound")
        );
    }
    #[test]
    fn bundle_sound_rejects_corrupt_truncated_or_non_vorbis_content() {
        let bytes =
            include_bytes!("../fixtures/content/assets/solaris_fixture/sounds/confirmation.ogg");
        let clip = PreparedSound::from_ogg(Arc::from(&bytes[..])).unwrap();
        assert_eq!(clip.rate, 48000);
        assert_eq!(clip.samples.len(), 12_000); // Authored quarter-second clip, without codec padding.
        assert!(clip.samples.iter().any(|sample| sample.abs() > 0.01));
        for size in [0, 3, 26, bytes.len() - 1] {
            assert!(PreparedSound::from_ogg(Arc::from(&bytes[..size])).is_err());
        }
        let mut corrupt = bytes.to_vec();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(PreparedSound::from_ogg(Arc::from(corrupt)).is_err());
        let mut appended = bytes.to_vec();
        appended.extend_from_slice(bytes);
        assert!(PreparedSound::from_ogg(Arc::from(appended)).is_err());
        assert!(PreparedSound::from_ogg(Arc::from(vec![0; 256 * 1024 + 1])).is_err());
        let wav =
            include_bytes!("../fixtures/content/assets/solaris_fixture/sounds/confirmation.wav");
        assert!(PreparedSound::from_ogg(Arc::from(&wav[..])).is_err());
    }
    #[test]
    fn stop_filter_intersection_and_entity_generation_are_exact() {
        let mut request = SoundRequest::positional(
            "minecraft:block.wood.place",
            SoundCategory::Blocks,
            [0.; 3],
            1.,
            1.,
            1,
        );
        assert!(stop_matches(&request, None, None));
        assert!(stop_matches(&request, Some(SoundCategory::Blocks), None));
        assert!(!stop_matches(
            &request,
            Some(SoundCategory::Music),
            Some("minecraft:block.wood.place")
        ));
        assert!(!stop_matches(
            &request,
            Some(SoundCategory::Blocks),
            Some("minecraft:block.stone.place")
        ));
        request.source = SoundOrigin::Entity {
            id: 7,
            generation: 3,
            position: [0.; 3],
        };
        assert!(request.source.matches_entity(7, 3));
        assert!(!request.source.matches_entity(7, 4));
    }
}
