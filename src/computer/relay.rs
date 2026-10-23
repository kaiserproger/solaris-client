//! Optional, permission-gated desktop pictures. This protocol never carries input.
use super::{Face, Frame};
use crate::protocol::Reader;
use image::{ImageDecoder, codecs::jpeg::{JpegDecoder, JpegEncoder}};
use std::{collections::{BTreeMap, VecDeque}, io::Cursor};

pub(crate) const CHANNEL: &str = "solaris:screen";
const MAX_JPEG: usize = 131_072;
const PART: usize = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Id {
    pub face: Face,
    pub dimension: u8,
}
impl Id {
    fn read(r: &mut Reader<'_>) -> Result<Self, String> {
        let block = [r.i32()?, r.i32()?, r.i32()?];
        let side = i32::from(r.u8()?);
        let width = r.u8()?;
        let height = r.u8()?;
        let dimension = r.u8()?;
        if !(2..=5).contains(&side) || !(1..=16).contains(&width) || !(1..=16).contains(&height) || dimension > 2 {
            return Err("invalid shared screen identity".into());
        }
        Ok(Self { face: Face { block, side, width, height }, dimension })
    }
    /// The 16 identity bytes on the wire: the key of this screen's audio speaker.
    pub fn speaker(self) -> [u8; 16] {
        self.packet(0)[2..18].try_into().expect("screen id is 16 bytes")
    }
    pub fn packet(self, kind: u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        out.extend([1, kind]);
        for coordinate in self.face.block { out.extend(coordinate.to_be_bytes()); }
        out.extend([self.face.side as u8, self.face.width, self.face.height, self.dimension]);
        out
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub screen: Id,
    pub number: u32,
    pub viewer: [u8; 16],
    pub name: String,
}
impl Request {
    pub fn answer(&self, allow: bool) -> Vec<u8> {
        let mut out = vec![1, 4];
        out.extend(self.number.to_be_bytes());
        out.push(u8::from(allow));
        out
    }
}
#[derive(Default)]
pub(crate) struct Remote {
    pub frame: Option<Frame>,
    pub dirty: bool,
    sequence: Option<u32>,
}
#[derive(Clone, Copy)]
pub(crate) struct Demand {
    pub width: u16,
    pub fps: u8,
}
/// One relayed Opus packet of a screen's audio, waiting for the session mixer.
pub(crate) struct AudioIn {
    pub screen: Id,
    pub seq: u32,
    pub opus: Vec<u8>,
}
/// At most this many undelivered packets (1.3 s); the oldest are dropped.
const MAX_AUDIO_QUEUE: usize = 64;
pub(crate) const MAX_OPUS: usize = 1000;
/// Monitor audio is silent beyond this many blocks (the server's looking range).
pub(crate) const AUDIO_RANGE: f32 = 32.;
#[derive(Default)]
pub(crate) struct Relay {
    pub audio: VecDeque<AudioIn>,
    pub registered: bool,
    pub active: BTreeMap<Id, Remote>,
    pub demand: BTreeMap<Id, Demand>,
    pub requests: VecDeque<Request>,
    /// Owner answers to this viewer's requests; the app knows which are still wanted.
    pub answers: VecDeque<(Id, bool)>,
    pub notice: Option<String>,
}
impl Relay {
    pub fn receive(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_JPEG + 32 { return Err("shared screen payload bound".into()); }
        let mut r = Reader::new(bytes);
        if r.u8()? != 1 { return Err("unsupported shared screen version".into()); }
        let kind = r.u8()?;
        let id = Id::read(&mut r)?;
        match kind {
            0 => {
                let on = r.bool()?;
                let width = r.i16()? as u16;
                let fps = r.u8()?;
                r.done()?;
                if width == 0 || width > 640 || fps == 0 || fps > 5 {
                    return Err("invalid shared screen demand".into());
                }
                if on {
                    if self.demand.len() >= 4 && !self.demand.contains_key(&id) { return Err("screen demand bound".into()); }
                    self.demand.insert(id, Demand { width, fps });
                } else { self.demand.remove(&id); }
            }
            1 => {
                let sequence = r.i32()? as u32;
                let width = r.i16()? as u16;
                let height = r.i16()? as u16;
                if r.u8()? != 0 { return Err("unsupported screen image codec".into()); }
                let jpeg = r.take(r.bytes.len() - r.offset)?;
                let remote = self.active.get_mut(&id).ok_or("frame for inactive screen")?;
                if remote.sequence.is_some_and(|old| sequence.wrapping_sub(old) > i32::MAX as u32 || sequence == old) {
                    return Ok(());
                }
                let frame = decode(jpeg, width, height)?;
                remote.frame = Some(frame);
                remote.sequence = Some(sequence);
                remote.dirty = true;
            }
            2 => {
                let reason = r.u8()?;
                r.done()?;
                if reason > 3 { return Err("invalid screen stop reason".into()); }
                self.audio.retain(|packet| packet.screen != id);
                if let Some(remote) = self.active.get_mut(&id) {
                    remote.frame = None;
                    remote.dirty = true;
                }
                if reason != 0 { self.notice = Some("Screen viewing permission ended".into()); }
            }
            3 => {
                let reason = r.u8()?;
                r.done()?;
                let reason = match reason {
                    0 => "not allowed", 1 => "monitor rectangle changed", 2 => "out of reach",
                    3 => "screen limit", 4 => "another player's screen", 5 => "invalid picture",
                    6 => "screen closed", 7 => "request rate limited",
                    _ => return Err("invalid screen refusal".into()),
                };
                self.demand.remove(&id);
                self.notice = Some(format!("Screen sharing: {reason}"));
            }
            4 => {
                let number = r.i32()? as u32;
                let viewer = r.take(16)?.try_into().map_err(|_| "invalid viewer UUID")?;
                let length = usize::from(r.u8()?);
                let name = std::str::from_utf8(r.take(length)?).map_err(|_| "invalid viewer name")?;
                if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
                    return Err("invalid viewer name".into());
                }
                r.done()?;
                if self.requests.len() >= 32 { return Err("screen permission request bound".into()); }
                self.requests.push_back(Request { screen: id, number, viewer, name: name.into() });
            }
            5 => {
                let allow = r.bool()?;
                r.done()?;
                if self.answers.len() >= 32 {
                    return Err("screen permission answer bound".into());
                }
                self.answers.push_back((id, allow));
            }
            6 => {
                let on = r.bool()?;
                r.done()?;
                if on {
                    if self.active.len() >= 128 && !self.active.contains_key(&id) { return Err("active screen bound".into()); }
                    self.active.entry(id).or_insert_with(|| Remote { dirty: true, ..Default::default() });
                } else {
                    self.active.remove(&id);
                    self.demand.remove(&id);
                    self.audio.retain(|packet| packet.screen != id);
                }
            }
            7 => {
                let seq = r.i32()? as u32;
                let opus = r.take(r.bytes.len() - r.offset)?;
                if opus.is_empty() || opus.len() > MAX_OPUS { return Err("invalid screen audio packet".into()); }
                // Audio racing a STOP/ACTIVE-off is ordinary: ignore it silently.
                if self.active.contains_key(&id) {
                    if self.audio.len() >= MAX_AUDIO_QUEUE { self.audio.pop_front(); }
                    self.audio.push_back(AudioIn { screen: id, seq, opus: opus.to_vec() });
                }
            }
            _ => return Err("unknown screen relay message".into()),
        }
        Ok(())
    }
}

fn decode(bytes: &[u8], width: u16, height: u16) -> Result<Frame, String> {
    if bytes.is_empty() || bytes.len() > MAX_JPEG || width == 0 || height == 0 || width > 640 || height > 640 {
        return Err("shared image size bound".into());
    }
    let mut decoder = JpegDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(640);
    limits.max_image_height = Some(640);
    limits.max_alloc = Some(4 * 1024 * 1024);
    decoder.set_limits(limits).map_err(|e| e.to_string())?;
    if decoder.dimensions() != (u32::from(width), u32::from(height)) {
        return Err("shared image dimensions disagree with header".into());
    }
    let rgba = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?.into_rgba8().into_raw();
    Ok(Frame { width: u32::from(width), height: u32::from(height), rgba })
}

#[derive(Default)]
pub(crate) struct Encoder {
    rgb: Vec<u8>,
    jpeg: Vec<u8>,
    sequence: u32,
}
impl Encoder {
    /// Only invoked on an explicit server DEMAND and at most five times a second.
    pub fn encode(&mut self, id: Id, frame: &Frame, maximum: u16) -> Result<Vec<Vec<u8>>, String> {
        if frame.width == 0 || frame.height == 0 || frame.width > 8192 || frame.height > 8192
            || frame.rgba.len() != frame.width as usize * frame.height as usize * 4 {
            return Err("invalid source frame".into());
        }
        let maximum = u32::from(maximum.clamp(1, 640));
        let largest = frame.width.max(frame.height);
        let mut width = frame.width.min(frame.width.saturating_mul(maximum) / largest).max(1);
        let mut height = frame.height.min(frame.height.saturating_mul(maximum) / largest).max(1);
        // Size cap is enforced by reducing resolution, not by sending an oversized picture.
        loop {
            self.rgb.resize(width as usize * height as usize * 3, 0);
            for y in 0..height {
                for x in 0..width {
                    let source = ((y * frame.height / height) as usize * frame.width as usize
                        + (x * frame.width / width) as usize) * 4;
                    let target = (y as usize * width as usize + x as usize) * 3;
                    self.rgb[target..target + 3].copy_from_slice(&frame.rgba[source..source + 3]);
                }
            }
            self.jpeg.clear();
            JpegEncoder::new_with_quality(&mut self.jpeg, 60)
                .encode(&self.rgb, width, height, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
            if self.jpeg.len() <= MAX_JPEG { break; }
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        self.sequence = self.sequence.wrapping_add(1);
        let count = self.jpeg.len().div_ceil(PART);
        let mut packets = Vec::with_capacity(count);
        for (index, part) in self.jpeg.chunks(PART).enumerate() {
            let mut packet = id.packet(2);
            packet.extend(self.sequence.to_be_bytes());
            packet.extend((width as u16).to_be_bytes());
            packet.extend((height as u16).to_be_bytes());
            packet.extend([0, index as u8, count as u8]);
            packet.extend(part);
            packets.push(packet);
        }
        Ok(packets)
    }
}

pub(crate) fn on_frame() -> Frame {
    // No desktop content: a plain illuminated panel and central green power mark.
    let mut rgba = vec![0; 32 * 16 * 4];
    for y in 0..16 { for x in 0..32 {
        let i = (y * 32 + x) * 4;
        let lit = (14..18).contains(&x) && (5..11).contains(&y);
        rgba[i..i + 4].copy_from_slice(if lit { &[30, 230, 90, 255] } else { &[12, 22, 28, 255] });
    }}
    Frame { width: 32, height: 16, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> Id { Id { face: Face { block: [-2, 4, 9], side: 2, width: 3, height: 2 }, dimension: 0 } }
    #[test]
    fn wire_consent_and_active_do_not_create_picture_data() {
        let mut relay = Relay::default();
        let mut active = id().packet(6); active.push(1);
        relay.receive(&active).unwrap();
        assert!(relay.active[&id()].frame.is_none());
        let mut request = id().packet(4);
        request.extend(17u32.to_be_bytes()); request.extend([9; 16]); request.push(3); request.extend(b"Bob");
        relay.receive(&request).unwrap();
        let request = relay.requests.pop_front().unwrap();
        assert_eq!(request.answer(false), [1, 4, 0, 0, 0, 17, 0]);
        assert_eq!(request.name, "Bob");
        assert!(relay.demand.is_empty());
        active.push(0);
        assert!(relay.receive(&active).is_err());
    }
    #[test]
    fn owner_answers_are_queued_with_their_screen_for_the_app_to_check() {
        let mut relay = Relay::default();
        let mut allow = id().packet(5);
        allow.push(1);
        relay.receive(&allow).unwrap();
        assert_eq!(relay.answers.pop_front(), Some((id(), true)));
        assert!(
            relay.notice.is_none(),
            "a cancelled request must not announce viewing"
        );
    }
    #[test]
    fn pictures_are_bounded_reassembled_and_removed_on_stop() {
        let frame = Frame { width: 900, height: 700, rgba: vec![180; 900 * 700 * 4] };
        let packets = Encoder::default().encode(id(), &frame, 640).unwrap();
        assert!(packets.len() <= 5);
        assert!(packets.iter().all(|p| p.len() <= 30_029));
        let mut received = id().packet(1);
        received.extend_from_slice(&packets[0][18..27]);
        for part in &packets { received.extend_from_slice(&part[29..]); }
        let mut relay = Relay::default();
        let mut active = id().packet(6); active.push(1); relay.receive(&active).unwrap();
        relay.receive(&received).unwrap();
        let decoded = relay.active[&id()].frame.as_ref().unwrap();
        assert_eq!(decoded.width, 640);
        assert!(decoded.height <= 640);
        let mut stop = id().packet(2); stop.push(2); relay.receive(&stop).unwrap();
        assert!(relay.active[&id()].frame.is_none());
        let mut corrupt = received;
        corrupt[22..24].copy_from_slice(&641u16.to_be_bytes());
        // New sequence makes the dimension error observable rather than a duplicate.
        corrupt[18..22].copy_from_slice(&2u32.to_be_bytes());
        assert!(relay.receive(&corrupt).is_err());
        assert!(relay.active[&id()].frame.is_none());
    }
    #[test]
    fn relayed_audio_is_queued_only_for_active_screens_bounded_and_dropped_on_stop() {
        let mut relay = Relay::default();
        let audio = |seq: u32, opus: &[u8]| {
            let mut packet = id().packet(7);
            packet.extend(seq.to_be_bytes());
            packet.extend(opus);
            packet
        };
        relay.receive(&audio(1, b"early")).unwrap();
        assert!(relay.audio.is_empty(), "no ACTIVE screen: ignored, not an error");
        let mut active = id().packet(6); active.push(1); relay.receive(&active).unwrap();
        for seq in 0..70 { relay.receive(&audio(seq, &[seq as u8; 20])).unwrap(); }
        assert_eq!(relay.audio.len(), MAX_AUDIO_QUEUE);
        assert_eq!(relay.audio.front().unwrap().seq, 6, "oldest dropped");
        assert_eq!(relay.audio.back().unwrap().opus, [69; 20]);
        assert_eq!(relay.audio[0].screen.speaker(), id().packet(0)[2..18]);
        assert!(relay.receive(&audio(1, b"")).is_err());
        assert!(relay.receive(&audio(1, &[0; MAX_OPUS + 1])).is_err());
        let mut stop = id().packet(2); stop.push(0); relay.receive(&stop).unwrap();
        assert!(relay.audio.is_empty(), "STOP discards queued audio");
    }
    #[test]
    fn screen_identity_places_height_before_dimension_without_legacy_fallback() {
        let mut wire = id().packet(0);
        assert_eq!(wire.len(), 18);
        assert_eq!(&wire[14..18], &[2, 3, 2, 0]);
        let mut reader = Reader::new(&wire[2..]);
        assert_eq!(Id::read(&mut reader).unwrap(), id());
        reader.done().unwrap();
        wire[16] = 0;
        assert!(Id::read(&mut Reader::new(&wire[2..])).is_err());
        wire[16] = 17;
        assert!(Id::read(&mut Reader::new(&wire[2..])).is_err());
        wire.remove(16);
        assert!(Id::read(&mut Reader::new(&wire[2..])).is_err());
    }
}
