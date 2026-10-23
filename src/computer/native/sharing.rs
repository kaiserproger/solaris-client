use super::*;
use crate::computer::{Frame, audio::AudioSource, relay::{self, Encoder, Id, Request}};
use crate::voice::{self, FRAME};
use std::{collections::{BTreeMap, BTreeSet}, time::{Duration, Instant}};

#[derive(Default)]
pub(super) struct Sharing {
    epochs: Option<(u64, u64)>,
    announced: Option<Id>,
    pub pending: Option<Request>,
    pub viewers: BTreeMap<[u8; 16], String>,
    watching: BTreeSet<Id>,
    next_frame: Option<Instant>,
    encoder: Encoder,
    audio: Option<AudioCapture>,
    /// `source.audio()` returned nothing (or failed) for this announcement: do not retry.
    audio_unavailable: bool,
}
/// Owner side of a monitor's audio: capture -> 20 ms Opus packets -> relay and/or own mixer.
struct AudioCapture {
    source: Box<dyn AudioSource>,
    encoder: voice::Encoder,
    pcm: Vec<f32>,
    seq: u32,
    sent: u64,
    /// Demand went off at this time; the capture stays warm briefly for looks away and back.
    idle_since: Option<Instant>,
}
/// A non-private capture (Windows loopback) is released this long after demand ends.
const AUDIO_GRACE: Duration = Duration::from_secs(3);
/// Never queue more than this much owner audio (240 ms) behind a stalled frame.
const MAX_PENDING_FRAMES: usize = 12;
/// App audio is music-like, not speech: 64 kbit/s (about 160 bytes per 20 ms packet).
const APP_BITRATE: i32 = 64_000;
impl AudioCapture {
    /// Encode every whole 20 ms frame captured so far (oldest audio beyond 240 ms is
    /// dropped). Returns the packets made before any failure, and the failure.
    fn step(&mut self) -> (Vec<(u32, Vec<u8>)>, Option<String>) {
        let mut packets = Vec::new();
        if let Err(error) = self.source.drain(&mut self.pcm) {
            return (packets, Some(error));
        }
        let excess = self.pcm.len().saturating_sub(MAX_PENDING_FRAMES * FRAME);
        self.pcm.drain(..excess - excess % FRAME);
        while self.pcm.len() >= FRAME {
            let frame: [f32; FRAME] = self.pcm[..FRAME].try_into().expect("one frame");
            self.pcm.drain(..FRAME);
            match self.encoder.encode(&frame) {
                Ok(opus) => {
                    self.seq = self.seq.wrapping_add(1);
                    self.sent += 1;
                    packets.push((self.seq, opus));
                }
                Err(error) => return (packets, Some(error)),
            }
        }
        (packets, None)
    }
}
impl Sharing {
    pub fn dialog(&self, selected: usize) -> Option<crate::ui::desktop::DesktopDialog> {
        use crate::ui::desktop::{DesktopAction, DesktopDialog};
        let request = self.pending.as_ref()?;
        Some(DesktopDialog {
            title: "Allow desktop viewing?".into(),
            body: vec![format!("{} wants to view your screens.", request.name),
                "Pictures only, while looking. No remote input.".into()],
            entries: vec![
                ("Deny".into(), DesktopAction::ViewPermission { request: request.number, allow: false }),
                ("Allow viewing all my open screens".into(), DesktopAction::ViewPermission { request: request.number, allow: true }),
            ],
            selected,
        })
    }
}
impl App {
    pub(super) fn answer_screen_request(&mut self, number: u32, allow: bool) {
        if self.computer.sharing.pending.as_ref().is_none_or(|request| request.number != number) { return; }
        let request = self.computer.sharing.pending.take().unwrap();
        match self.session.send_screen(request.answer(allow)) {
            Ok(()) => {
                if allow { self.computer.sharing.viewers.insert(request.viewer, request.name); }
                self.computer.message = if allow {
                    "Viewing allowed while they look. F8 revokes ALL viewers; Shift+Esc closes"
                } else { "Viewing denied; desktop remains private" }.into();
            }
            Err(error) => self.computer.message = error,
        }
        self.computer.selection = 0;
    }
    pub(super) fn revoke_screen_viewer(&mut self, viewer: [u8; 16]) {
        if self.computer.sharing.viewers.remove(&viewer).is_none() { return; }
        if let Some(id) = self.computer.sharing.announced {
            let mut packet = id.packet(5);
            packet.extend(viewer);
            self.computer.message = match self.session.send_screen(packet) {
                Ok(()) => "Viewer permission revoked".into(),
                Err(error) => error,
            };
        }
        self.computer.selection = 0;
    }
    pub(super) fn sync_screen_sharing(&mut self, enabled: bool) {
        let epochs = self.session.context_epochs();
        if self.computer.sharing.epochs != Some(epochs) {
            self.computer.sharing = Sharing { epochs: Some(epochs), ..Default::default() };
        }
        let live = enabled && self.session.phase == "play";
        if live && let Err(error) = self.session.register_screens() {
            self.computer.message = error;
        }
        // Screen audio uses the voice codec and is enabled by the server's VOICE_STATE;
        // a server without it (older, or [voice] off) never receives AUDIO_PART.
        if live && let Err(error) = self.session.register_voice() {
            self.computer.message = error;
        }
        let current = if live && self.computer.source.is_some() && self.computer.size != [0; 2] {
            self.computer.face.zip(self.session.screen_dimension()).map(|(face, dimension)| Id { face, dimension })
        } else { None };
        if current != self.computer.sharing.announced {
            if let Some(old) = self.computer.sharing.announced.take() {
                let _ = self.session.send_screen(old.packet(1));
                self.session.screen_relay.demand.remove(&old);
            }
            if let Some(request) = self.computer.sharing.pending.take() {
                let _ = self.session.send_screen(request.answer(false));
            }
            self.computer.sharing.viewers.clear();
            self.computer.sharing.next_frame = None;
            self.computer.sharing.audio = None;
            self.computer.sharing.audio_unavailable = false;
            if let Some(id) = current {
                match self.session.send_screen(id.packet(0)) {
                    Ok(()) => self.computer.sharing.announced = Some(id),
                    Err(error) => self.computer.message = error,
                }
            }
        }
        if !live {
            for id in std::mem::take(&mut self.computer.sharing.watching) {
                let _ = self.session.send_screen(id.packet(6));
            }
            self.computer.sharing.pending = None;
            for remote in self.session.screen_relay.active.values_mut() {
                remote.frame = None;
                remote.dirty = true;
            }
        }
        if let Some(notice) = self.session.screen_relay.notice.take() {
            self.computer.message = notice;
            self.publish_session();
        }
        if self.computer.sharing.pending.is_none() {
            while let Some(request) = self.session.screen_relay.requests.pop_front() {
                if live && Some(request.screen) == self.computer.sharing.announced {
                    self.release("desktop viewing permission request");
                    self.computer.selection = 0;
                    self.computer.sharing.pending = Some(request);
                    self.publish_session();
                    break;
                }
                let _ = self.session.send_screen(request.answer(false));
            }
        }
        if let Some(renderer) = &mut self.renderer {
            let own = self.computer.sharing.announced;
            let dimension = self.session.screen_dimension();
            renderer.retain_shared_screens(|id| live && Some(id.dimension) == dimension
                && Some(id) != own && self.session.screen_relay.active.contains_key(&id));
            for (id, remote) in &mut self.session.screen_relay.active {
                if live && Some(id.dimension) == dimension && Some(*id) != own
                    && (remote.dirty || !renderer.has_shared_screen(*id)) {
                    let frame = remote.frame.take().unwrap_or_else(relay::on_frame);
                    renderer.update_shared_screen(*id, &frame);
                    remote.dirty = false;
                }
            }
        }
    }
    pub(super) fn publish_screen_frame(&mut self, frame: &Frame) {
        let Some(id) = self.computer.sharing.announced else { return; };
        // Local approval is required as well as server demand. Never trust a
        // demand alone to authorize disclosure of desktop bytes.
        if self.computer.sharing.viewers.is_empty() { return; }
        let Some(demand) = self.session.screen_relay.demand.get(&id).copied() else { return; };
        let now = Instant::now();
        if self.computer.sharing.next_frame.is_some_and(|next| now < next) { return; }
        self.computer.sharing.next_frame = Some(now + Duration::from_secs_f64(1. / f64::from(demand.fps)));
        let result = self.computer.sharing.encoder.encode(id, frame, demand.width)
            .and_then(|packets| {
                for packet in packets { self.session.send_screen(packet)?; }
                Ok(())
            });
        if let Err(error) = result { self.computer.message = format!("Screen sharing: {error}"); }
    }
    /// Capture the shared source's audio and relay it while viewers are demanded; a
    /// private-sink app (nested monitor) is also played at its own monitor.
    pub(super) fn pump_screen_audio(&mut self) {
        let announced = self.computer.sharing.announced;
        let demanded = self.session.voice_net.enabled
            && announced.is_some_and(|id| {
                !self.computer.sharing.viewers.is_empty() && self.session.screen_relay.demand.contains_key(&id)
            });
        let private = self.computer.source.as_ref().is_some_and(|source| source.private_audio());
        let now = Instant::now();
        let sharing = &mut self.computer.sharing;
        if announced.is_none() || self.computer.source.is_none() {
            sharing.audio = None;
            self.session.screen_audio_owner.capturing = false;
            return;
        }
        if demanded || private {
            if let Some(capture) = &mut sharing.audio { capture.idle_since = None; }
        } else if let Some(capture) = &mut sharing.audio
            && now.duration_since(*capture.idle_since.get_or_insert(now)) >= AUDIO_GRACE
        {
            sharing.audio = None;
        }
        if sharing.audio.is_none() && (demanded || private) && !sharing.audio_unavailable {
            let native = self.computer.source.as_mut().and_then(|source| source.audio());
            match (crate::computer::audio::resolve(native), voice::Encoder::with_bitrate(APP_BITRATE, true)) {
                (Some(source), Ok(encoder)) => {
                    sharing.audio = Some(AudioCapture { source, encoder, pcm: Vec::new(), seq: 0, sent: 0, idle_since: None });
                }
                (_, encoder) => {
                    sharing.audio_unavailable = true;
                    if let Err(error) = encoder { eprintln!("screen audio: {error}"); }
                }
            }
        }
        let owner = &mut self.session.screen_audio_owner;
        owner.capturing = sharing.audio.is_some();
        owner.unavailable = sharing.audio_unavailable;
        let (Some(id), Some(capture)) = (announced, &mut sharing.audio) else { return };
        let (packets, failure) = capture.step();
        owner.packets_sent = capture.sent;
        let local = private;
        if let Some(error) = failure {
            eprintln!("screen audio: {error}");
            sharing.audio = None;
            sharing.audio_unavailable = true;
        }
        for (seq, opus) in packets {
            if local && let Err(error) = self.session.play_screen_audio(id, seq, &opus) {
                eprintln!("screen audio playback: {error}");
            }
            if demanded {
                let mut packet = id.packet(7);
                packet.extend(seq.to_be_bytes());
                packet.extend(&opus);
                if let Err(error) = self.session.send_screen(packet) {
                    self.computer.message = format!("Screen sharing: {error}");
                    break;
                }
            }
        }
    }
    pub(super) fn request_shared_screen(&mut self, face: Face) -> bool {
        let Some(dimension) = self.session.screen_dimension() else { return false; };
        let id = Id { face, dimension };
        if Some(id) == self.computer.sharing.announced || !self.session.screen_relay.active.contains_key(&id) {
            return false;
        }
        self.computer.release_input();
        let _ = self.session.command(Command::Input(InputIntent::ReleaseAll));
        match self.session.send_screen(id.packet(3)) {
            Ok(()) => {
                self.computer.sharing.watching.insert(id);
                self.computer.message = "Viewing requested. Waiting for the owner; F8 cancels/leaves".into();
            }
            Err(error) => self.computer.message = error,
        }
        self.publish_session();
        true
    }
    pub(super) fn sharing_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.physical_key == PhysicalKey::Code(KeyCode::F8)
            && event.state == ElementState::Pressed && !event.repeat
        {
            if let Some(id) = self.computer.sharing.announced {
                for (viewer, _) in std::mem::take(&mut self.computer.sharing.viewers) {
                    let mut packet = id.packet(5); packet.extend(viewer);
                    let _ = self.session.send_screen(packet);
                }
                self.session.screen_relay.demand.remove(&id);
            }
            for id in std::mem::take(&mut self.computer.sharing.watching) {
                let _ = self.session.send_screen(id.packet(6));
            }
            // Clear already-rendered pictures immediately rather than waiting
            // for STOP to return through the network.
            for remote in self.session.screen_relay.active.values_mut() {
                remote.frame = None; remote.dirty = true;
            }
            self.computer.message = "Screen sharing revoked; viewing ended. Local capture continues".into();
            self.publish_session();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(source: crate::computer::audio::WavSource) -> AudioCapture {
        AudioCapture {
            source: Box::new(source),
            encoder: voice::Encoder::with_bitrate(APP_BITRATE, true).unwrap(),
            pcm: Vec::new(),
            seq: 0,
            sent: 0,
            idle_since: None,
        }
    }
    #[test]
    fn captured_audio_becomes_sequenced_decodable_20ms_opus_packets() {
        use crate::computer::audio::WavSource;
        let mut capture = capture(WavSource::tone(440., 1.).advanced(Duration::from_millis(100)));
        let (packets, failure) = capture.step();
        assert!(failure.is_none());
        assert!((4..=6).contains(&packets.len()), "100 ms is five 20 ms packets, got {}", packets.len());
        assert_eq!(packets.iter().map(|p| p.0).collect::<Vec<_>>(), (1..=packets.len() as u32).collect::<Vec<_>>());
        assert!(packets.iter().all(|(_, opus)| (1..=relay::MAX_OPUS).contains(&opus.len())));
        let mut decoder = voice::Decoder::new().unwrap();
        let peak = packets.iter().skip(1).map(|(_, opus)| {
            decoder.decode(Some(opus)).unwrap().iter().fold(0f32, |m, s| m.max(s.abs()))
        }).fold(0f32, f32::max);
        assert!(peak > 0.2, "decoded tone is audible: {peak}");
        assert_eq!(capture.sent, packets.len() as u64);
        // Nothing new has been captured yet: no packets, no failure.
        assert!(capture.step().0.is_empty());
    }
    #[test]
    fn a_stalled_owner_never_queues_more_than_240_ms_of_audio() {
        use crate::computer::audio::WavSource;
        let mut capture = capture(WavSource::tone(220., 3.).advanced(Duration::from_secs(2)));
        let (packets, _) = capture.step();
        assert_eq!(packets.len(), MAX_PENDING_FRAMES);
        assert!(capture.pcm.len() < FRAME);
    }
    #[test]
    fn permission_prompt_defaults_to_deny_and_discloses_scope() {
        let sharing = Sharing {
            pending: Some(Request {
                screen: Id { face: Face { block: [0, 1, 2], side: 2, width: 1, height: 1 }, dimension: 0 },
                number: 7, viewer: [0; 16], name: "Viewer".into(),
            }),
            ..Default::default()
        };
        assert!(sharing.viewers.is_empty());
        let dialog = sharing.dialog(0).unwrap();
        assert_eq!(dialog.selected, 0);
        assert!(dialog.entries[1].0.contains("all my open screens"));
        assert_eq!(dialog.entries[0].1, crate::ui::desktop::DesktopAction::ViewPermission { request: 7, allow: false });
    }
}
