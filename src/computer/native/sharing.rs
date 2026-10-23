use super::*;
use crate::computer::{Frame, relay::{self, Encoder, Id, Request}};
use std::{collections::BTreeSet, time::{Duration, Instant}};

#[derive(Default)]
pub(super) struct Sharing {
    epochs: Option<(u64, u64)>,
    announced: Option<Id>,
    pub pending: Option<Request>,
    allow: bool,
    viewers: BTreeSet<[u8; 16]>,
    watching: BTreeSet<Id>,
    next_frame: Option<Instant>,
    encoder: Encoder,
}
impl Sharing {
    pub fn lines(&self) -> Option<Vec<String>> {
        let request = self.pending.as_ref()?;
        Some(vec![
            format!("{} requests permission to VIEW your desktop screens", request.name),
            "Allows pictures from all your open screens, only while they look. No remote input.".into(),
            format!("{} Deny     {} Allow", if self.allow { " " } else { ">" }, if self.allow { ">" } else { " " }),
            "Up/Down selects; Enter confirms; Esc denies. F8 later revokes all viewers.".into(),
        ])
    }
}
impl App {
    pub(super) fn sync_screen_sharing(&mut self, enabled: bool) {
        let epochs = self.session.context_epochs();
        if self.computer.sharing.epochs != Some(epochs) {
            self.computer.sharing = Sharing { epochs: Some(epochs), ..Default::default() };
        }
        let live = enabled && self.session.phase == "play";
        if live && let Err(error) = self.session.register_screens() {
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
                    self.computer.sharing.allow = false;
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
        if self.computer.sharing.pending.is_some() {
            if let WindowEvent::KeyboardInput { event, .. } = event
                && event.state == ElementState::Pressed && !event.repeat
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::ArrowUp | KeyCode::ArrowDown) => {
                        self.computer.sharing.allow = !self.computer.sharing.allow;
                    }
                    PhysicalKey::Code(KeyCode::Enter | KeyCode::Escape) => {
                        if let Some(request) = self.computer.sharing.pending.take() {
                            let allow = self.computer.sharing.allow && event.physical_key != PhysicalKey::Code(KeyCode::Escape);
                            match self.session.send_screen(request.answer(allow)) {
                                Ok(()) => {
                                    if allow { self.computer.sharing.viewers.insert(request.viewer); }
                                    self.computer.message = if allow {
                                        "Viewing allowed while they look. F8 revokes ALL viewers; Shift+Esc closes"
                                    } else { "Viewing denied; desktop remains private" }.into();
                                }
                                Err(error) => self.computer.message = error,
                            }
                            self.computer.sharing.allow = false;
                        }
                    }
                    _ => {}
                }
                self.publish_session();
            }
            return matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. });
        }
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.physical_key == PhysicalKey::Code(KeyCode::F8)
            && event.state == ElementState::Pressed && !event.repeat
        {
            if let Some(id) = self.computer.sharing.announced {
                for viewer in std::mem::take(&mut self.computer.sharing.viewers) {
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
    #[test]
    fn permission_prompt_defaults_to_deny_and_discloses_scope() {
        let sharing = Sharing {
            pending: Some(Request {
                screen: Id { face: Face { block: [0, 1, 2], side: 2, width: 1 }, dimension: 0 },
                number: 7, viewer: [0; 16], name: "Viewer".into(),
            }),
            ..Default::default()
        };
        assert!(!sharing.allow);
        assert!(sharing.viewers.is_empty());
        let lines = sharing.lines().unwrap();
        assert!(lines[1].contains("all your open screens"));
        assert!(lines[2].starts_with("> Deny"));
    }
}
