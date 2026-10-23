mod sharing;
use super::{DesktopKey, DesktopSource, Face, PointerButton, WindowInfo};
use crate::{
    App,
    api::{Command, InputIntent},
    player::{BlockHit, Hit},
};
use winit::{
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
};

#[derive(Default)]
pub(crate) struct Computer {
    source: Option<Box<dyn DesktopSource>>,
    pub face: Option<Face>,
    state: Option<u32>,
    binding: Option<[i32; 3]>,
    epochs: (u64, u64),
    size: [u32; 2],
    pointer: Option<[f64; 2]>,
    controlling: bool,
    managing: bool,
    waiting: bool,
    keys: Vec<(PhysicalKey, DesktopKey)>,
    buttons: Vec<PointerButton>,
    picker: Vec<WindowInfo>,
    #[cfg(target_os = "linux")]
    apps: Vec<Vec<String>>,
    selection: usize,
    message: String,
    sharing: sharing::Sharing,
}
impl Computer {
    pub fn has_modal(&self) -> bool {
        self.sharing.pending.is_some() || self.managing || !self.picker.is_empty()
    }
    pub(crate) fn dialog(&self) -> Option<crate::ui::desktop::DesktopDialog> {
        use crate::ui::desktop::{DesktopAction, DesktopDialog};
        if let Some(dialog) = self.sharing.dialog(self.selection) { return Some(dialog); }
        if self.managing {
            let mut entries = vec![
                ("Change window".into(), DesktopAction::ChangeWindow),
                ("Stop sharing".into(), DesktopAction::StopCapture),
            ];
            if self.source.as_ref().is_some_and(|source| source.control_granted()) {
                entries.insert(0, ("Control".into(), DesktopAction::Control));
            }
            entries.extend(self.sharing.viewers.iter().map(|(viewer, name)| (
                format!("Revoke: {name}"), DesktopAction::RevokeViewer(*viewer)
            )));
            entries.push(("Done".into(), DesktopAction::DismissMenu));
            return Some(DesktopDialog {
                title: "Your monitor".into(),
                body: vec!["Change its window or manage viewer permissions.".into()],
                entries, selected: self.selection,
            });
        }
        if self.picker.is_empty() { return None; }
        let mut entries = self.picker.iter().map(|window| (window.title.clone(), DesktopAction::PickWindow(window.id))).collect::<Vec<_>>();
        entries.push(("Cancel".into(), DesktopAction::CancelPicker));
        Some(DesktopDialog {
            title: format!("Desktop window — {} monitor block(s)", self.face.map_or(1, |face| u16::from(face.width) * u16::from(face.height))),
            body: vec!["Choose a window to display on this monitor.".into()],
            entries,
            selected: self.selection,
        })
    }
    pub fn append_status(&self, lines: &mut Vec<String>) {
        if !self.has_modal() && !self.message.is_empty() {
            lines.push(self.message.clone());
        }
    }
    pub fn release_input(&mut self) {
        let mut failed = false;
        if let Some(source) = &mut self.source {
            for (_, key) in self.keys.drain(..) {
                failed |= source.send_key(key, false).is_err();
            }
            for button in self.buttons.drain(..) {
                failed |= source.send_button(button, false).is_err();
            }
        }
        if failed && let Some(mut source) = self.source.take() {
            source.close();
            self.message = "Desktop input failed; capture closed to release held input".into();
        }
        self.controlling = false;
        self.pointer = None;
    }
    fn escape(&mut self, close_looked_at_screen: bool) -> bool {
        if close_looked_at_screen {
            self.close();
        } else if self.controlling {
            self.release_input();
            self.message = "Desktop control released. Right-click for monitor menu".into();
        } else {
            return false;
        }
        true
    }
    fn close(&mut self) {
        self.release_input();
        if let Some(mut source) = self.source.take() {
            source.close();
        }
        self.face = None;
        self.state = None;
        self.binding = None;
        self.managing = false;
        self.waiting = false;
        self.epochs = (0, 0);
        self.selection = 0;
        self.size = [0; 2];
        self.picker.clear();
        #[cfg(target_os = "linux")]
        self.apps.clear();
        self.message.clear();
    }
    fn start(&mut self, id: u64) {
        self.picker.clear();
        let source = self.open_choice(id);
        #[cfg(target_os = "linux")]
        self.apps.clear();
        match source {
            Ok(source) => {
                self.release_input();
                if let Some(mut previous) = self.source.take() { previous.close(); }
                self.managing = false;
                self.waiting = true;
                self.source = Some(source);
                self.message="Opening selected source. Shift+Esc while looking closes capture.".into();
            }
            Err(error) => self.fail(error),
        }
    }
    fn open_choice(&self, id: u64) -> Result<Box<dyn DesktopSource>, String> {
        #[cfg(target_os = "linux")]
        if id & (1 << 63) != 0 {
            let argv = self.apps.get((id & !(1 << 63)) as usize)
                .ok_or("Monitor app selection expired")?;
            let face = self.face.ok_or("No monitor selected")?;
            let aspect = (f64::from(face.width) - 0.125)
                / (f64::from(face.height) - 0.125);
            return super::nested::launch(argv, aspect);
        }
        super::open(id)
    }
    #[cfg(target_os = "linux")]
    fn append_apps(&mut self, setting: Option<&serde_json::Value>) {
        self.apps.clear();
        for (name, argv) in super::nested::apps(setting) {
            let id = (1 << 63) | self.apps.len() as u64;
            self.apps.push(argv);
            self.picker.push(WindowInfo { id, title: format!("Launch app: {name}") });
        }
    }
    fn fail(&mut self, error: String) {
        self.close();
        self.message = format!("Desktop computer: {error}");
        eprintln!("{}", self.message);
    }
}
impl Drop for Computer {
    fn drop(&mut self) {
        self.close();
    }
}
impl App {
    fn computer_hit(&self) -> Option<BlockHit> {
        let content = self.session.content.as_ref()?;
        let hit = self
            .session
            .player
            .raycast(
                &self.session.world,
                content,
                &self.session.entities,
                self.session.client_tick,
            )
            .ok()??;
        match hit {
            Hit::Block(hit) if content.loader_block(hit.state) == Some(("solaris:monitor", hit.face)) => Some(hit),
            _ => None,
        }
    }
    pub(crate) fn update_computer(&mut self) {
        let enabled = self
            .session
            .native_setting("desktop_computer")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !enabled
            || self.session.phase != "play"
            || self.computer.binding.is_some_and(|binding| {
                self.session.world.block(binding) != self.computer.state
                    || self.session.context_epochs() != self.computer.epochs
            })
        {
            self.computer.close();
        }
        if let (Some(binding), Some(face)) = (self.computer.binding, self.computer.face) {
            if let Ok(rectangle) = Face::rectangle(binding, face.side, |p| self.session.world.block(p) == self.computer.state) {
                self.computer.face = Some(rectangle);
                if let Some(renderer) = &mut self.renderer { renderer.set_desktop_face(rectangle); }
            } else {
                self.computer.close();
            }
        }
        self.sync_screen_sharing(enabled);
        self.consume_computer_dialog_action();
        if self.computer.controlling {
            let pixel = self.computer_hit().and_then(|hit| {
                self.computer
                    .face
                    .filter(|face| face.contains(hit.position, hit.face))
                    .and_then(|face| face.pixel(hit.point, self.computer.size))
            });
            if let Some([x, y]) = pixel {
                if self.computer.pointer != Some([x, y]) {
                    self.computer.pointer = Some([x, y]);
                    if let Some(source) = &mut self.computer.source
                        && let Err(error) = source.send_pointer(x, y)
                    {
                        self.computer.fail(error);
                    }
                }
            } else {
                self.computer.release_input();
                self.computer.message =
                    "Desktop: right-click the screen to control; Shift+Esc closes".into();
            }
        }
        if let Some(source) = &mut self.computer.source {
            match source.next_frame() {
                Ok(Some(frame)) => {
                    let size = [frame.width, frame.height];
                    if size.contains(&0)
                        || size.iter().any(|v| *v > 8192)
                        || frame.rgba.len() != frame.width as usize * frame.height as usize * 4
                    {
                        self.computer.fail("Invalid desktop frame".into());
                    } else if let (Some(renderer), Some(face)) =
                        (&mut self.renderer, self.computer.face)
                    {
                        let first = self.computer.waiting || self.computer.size == [0; 2];
                        self.computer.waiting = false;
                        self.computer.size = size;
                        renderer.update_desktop(face, &frame);
                        if first {
                            eprintln!("desktop frame arrived: {}x{}", frame.width, frame.height);
                            self.computer.message = if source.control_granted() {
                                "LIVE desktop. Right-click to control; Esc releases; Shift+Esc closes"
                            } else {
                                "LIVE view-only desktop. Right-click for Change window, Stop sharing and viewer permissions"
                            }.into();
                            self.publish_session();
                        }
                        self.publish_screen_frame(&frame);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.computer.fail(error);
                    self.publish_session();
                }
            }
        }
        if self.computer.source.is_none()
            && let Some(renderer) = &mut self.renderer
        {
            renderer.clear_desktop();
        }
    }
    fn consume_computer_dialog_action(&mut self) {
        if let Some(action) = self.session.ui_mut().and_then(|ui| ui.take_desktop_action()) {
            self.apply_computer_dialog_action(action);
        }
    }
    fn pick_computer_window(&mut self) {
        self.computer.managing = false;
        self.computer.selection = 0;
        let error = match super::list_windows() {
            Ok(windows) => { self.computer.picker = windows; None }
            Err(error) => { self.computer.picker.clear(); Some(error) }
        };
        #[cfg(target_os = "linux")]
        self.computer.append_apps(self.session.native_setting("monitor_apps"));
        if self.computer.picker.is_empty() {
            self.computer.fail(error.unwrap_or_else(|| "No capturable windows or monitor apps".into()));
        }
    }
    fn apply_computer_dialog_action(&mut self, action: crate::ui::desktop::DesktopAction) {
        use crate::ui::desktop::DesktopAction;
        match action {
            DesktopAction::PickWindow(id) if self.computer.picker.iter().any(|window| window.id == id) => self.computer.start(id),
            DesktopAction::CancelPicker if !self.computer.picker.is_empty() => {
                if self.computer.source.is_some() { self.computer.picker.clear(); } else { self.computer.close(); }
            }
            DesktopAction::ViewPermission { request, allow } => self.answer_screen_request(request, allow),
            DesktopAction::DismissMenu => self.computer.managing = false,
            DesktopAction::ChangeWindow if self.computer.managing => self.pick_computer_window(),
            DesktopAction::StopCapture if self.computer.managing => self.computer.close(),
            DesktopAction::RevokeViewer(viewer) if self.computer.managing => self.revoke_screen_viewer(viewer),
            DesktopAction::Control if self.computer.managing
                && self.computer.source.as_ref().is_some_and(|source| source.control_granted()) => {
                let _ = self.session.input(InputIntent::ReleaseAll);
                self.voice_focus_lost();
                self.computer.managing = false;
                self.computer.controlling = true;
                self.computer.message = "CONTROLLING LOCAL DESKTOP — Esc releases; Shift+Esc closes".into();
                if let Some(window) = &self.window {
                    use winit::window::CursorGrabMode;
                    if window.set_cursor_grab(CursorGrabMode::Locked)
                        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined)).is_ok() {
                        window.set_cursor_visible(false);
                        self.cursor_locked = true;
                    }
                }
            }
            _ => {}
        }
        self.publish_session();
    }
    pub(crate) fn computer_event(&mut self, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::CloseRequested) {
            self.computer.close();
            return false;
        }
        if !self.focused
            || !self
                .session
                .native_setting("desktop_computer")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            return false;
        }
        if self.computer.has_modal() {
            let pointer = match event {
                WindowEvent::CursorMoved { position, .. } => {
                    self.last_cursor = Some((position.x, position.y));
                    Some((crate::ui::PointerPhase::Move, position.x, position.y))
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } =>
                    self.last_cursor.map(|(x, y)| (crate::ui::PointerPhase::Down, x, y)),
                _ => None,
            };
            if let Some((phase, x, y)) = pointer {
                let _ = self.session.execute_ui(crate::ui::UiIntent::Pointer {
                    phase, x, y, button: Some(crate::ui::PointerButton::Left), modifiers: Default::default(),
                });
                if matches!(phase, crate::ui::PointerPhase::Down) {
                    let _ = self.session.execute_ui(crate::ui::UiIntent::Pointer {
                        phase: crate::ui::PointerPhase::Up, x, y, button: Some(crate::ui::PointerButton::Left), modifiers: Default::default(),
                    });
                }
                self.consume_computer_dialog_action();
                self.publish_session();
                return true;
            }
            if matches!(event, WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }) { return true; }
            if let WindowEvent::KeyboardInput { event, .. } = event {
                if event.state == ElementState::Pressed && !event.repeat {
                    use crate::ui::desktop::DesktopAction;
                    let dialog = self.computer.dialog().unwrap();
                    let count = dialog.entries.len();
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowDown) => self.computer.selection = (self.computer.selection + 1) % count,
                        PhysicalKey::Code(KeyCode::ArrowUp) => self.computer.selection = (self.computer.selection + count - 1) % count,
                        PhysicalKey::Code(KeyCode::Enter) => {
                            if let Some((_, action)) = dialog.entries.get(self.computer.selection) {
                                self.apply_computer_dialog_action(action.clone());
                            }
                        }
                        PhysicalKey::Code(KeyCode::Escape) => {
                            let action = if let Some(request) = &self.computer.sharing.pending {
                                DesktopAction::ViewPermission { request: request.number, allow: false }
                            } else if self.computer.managing { DesktopAction::DismissMenu } else { DesktopAction::CancelPicker };
                            self.apply_computer_dialog_action(action);
                        }
                        _ => {}
                    }
                    self.publish_session();
                }
                return true;
            }
        }
        if self.sharing_event(event) { return true; }
        if let WindowEvent::KeyboardInput { event, .. } = event {
            let pressed = event.state == ElementState::Pressed;
            if event.physical_key == PhysicalKey::Code(KeyCode::Escape) && pressed {
                let close = self.shift_held && self.computer_hit().is_some_and(|hit|
                    self.computer.face.is_some_and(|face| face.contains(hit.position, hit.face)));
                if self.computer.escape(close) {
                    self.publish_session();
                    return true;
                }
            }
            if self.computer.controlling {
                if matches!(
                    event.physical_key,
                    PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight)
                ) {
                    self.shift_held = pressed;
                }
                let key = if pressed {
                    self.computer.input_key(event)
                } else {
                    self.computer
                        .keys
                        .iter()
                        .find(|(physical, _)| *physical == event.physical_key)
                        .map(|(_, key)| *key)
                };
                if let Some(key) = key {
                    if pressed
                        && !self
                            .computer
                            .keys
                            .iter()
                            .any(|(physical, _)| *physical == event.physical_key)
                    {
                        self.computer.keys.push((event.physical_key, key));
                    }
                    if !pressed {
                        self.computer
                            .keys
                            .retain(|(physical, _)| *physical != event.physical_key);
                    }
                    if let Some(source) = &mut self.computer.source
                        && let Err(error) = source.send_key(key, pressed)
                    {
                        self.computer.fail(error);
                    }
                }
                return true;
            }
        }
        if let WindowEvent::MouseInput { state, button, .. } = event {
            let pressed = *state == ElementState::Pressed;
            if self.computer.controlling {
                let button = match button {
                    MouseButton::Left => Some(PointerButton::Left),
                    MouseButton::Right => Some(PointerButton::Right),
                    MouseButton::Middle => Some(PointerButton::Middle),
                    _ => None,
                };
                if let Some(button) = button {
                    if !pressed && !self.computer.buttons.contains(&button) {
                        return true;
                    }
                    if pressed && !self.computer.buttons.contains(&button) {
                        self.computer.buttons.push(button);
                    }
                    if !pressed {
                        self.computer.buttons.retain(|b| *b != button);
                    }
                    if let Some(source) = &mut self.computer.source
                        && let Err(error) = source.send_button(button, pressed)
                    {
                        self.computer.fail(error);
                    }
                }
                return true;
            }
            if !self.computer.picker.is_empty() {
                return true;
            }
            if *button == MouseButton::Right
                && !self.shift_held
                && self.session.screen() == "game"
                && !self.session.dialog_screen_active()
                && let Some(hit) = self.computer_hit()
            {
                if !pressed {
                    return true;
                }
                let face = match Face::rectangle(hit.position, hit.face, |p| self.session.world.block(p) == Some(hit.state)) {
                    Ok(face) => face,
                    Err(error) => {
                        let actual_side = match error {
                            super::RectangleError::Missing(position) => self.session.world.block(position)
                                .and_then(|state| self.session.content.as_ref()?.loader_block(state))
                                .filter(|(name, _)| *name == "solaris:monitor")
                                .map(|(_, side)| side),
                            _ => None,
                        };
                        self.computer.message = error.message(hit.face, actual_side);
                        self.publish_session();
                        return true;
                    }
                };
                if self.request_shared_screen(face) { return true; }
                let _ = self
                    .session
                    .command(Command::Input(InputIntent::ReleaseAll));
                if self.computer.face == Some(face) && self.computer.size != [0; 2] {
                    self.computer.managing = true;
                    self.computer.selection = 0;
                    self.release("desktop monitor menu");
                } else {
                    self.computer.close();
                    self.computer.face = Some(face);
                    self.computer.state = Some(hit.state);
                    self.computer.binding = Some(hit.position);
                    self.computer.epochs = self.session.context_epochs();
                    self.pick_computer_window();
                }
                self.publish_session();
                return true;
            }
        }
        if self.computer.controlling
            && let WindowEvent::MouseWheel { delta, .. } = event
        {
            let (x, y) = match delta {
                MouseScrollDelta::LineDelta(x, y) => (f64::from(*x), f64::from(*y)),
                MouseScrollDelta::PixelDelta(p) => (p.x / 40., p.y / 40.),
            };
            if let Some(source) = &mut self.computer.source
                && let Err(error) = source.send_scroll(x, y)
            {
                self.computer.fail(error);
            }
            return true;
        }
        false
    }
}
impl Computer {
    fn input_key(&self, event: &winit::event::KeyEvent) -> Option<DesktopKey> {
        // PostMessage does not set the target's keyboard state: use the already
        // composed text for ordinary typing, and physical codes for shortcuts.
        #[cfg(windows)]
        if !self
            .keys
            .iter()
            .any(|(_, key)| matches!(key, DesktopKey::Code(29 | 97 | 56 | 100)))
            && let Some(text) = &event.text
        {
            let mut chars = text.chars();
            if let Some(ch) = chars.next()
                && !ch.is_control()
                && chars.next().is_none()
            {
                return Some(DesktopKey::Char(ch));
            }
        }
        physical_key(event.physical_key).or_else(|| desktop_key(&event.logical_key))
    }
}
fn physical_key(key: PhysicalKey) -> Option<DesktopKey> {
    let PhysicalKey::Code(key) = key else {
        return None;
    };
    use KeyCode::*;
    Some(DesktopKey::Code(match key {
        Digit1 => 2,
        Digit2 => 3,
        Digit3 => 4,
        Digit4 => 5,
        Digit5 => 6,
        Digit6 => 7,
        Digit7 => 8,
        Digit8 => 9,
        Digit9 => 10,
        Digit0 => 11,
        Minus => 12,
        Equal => 13,
        Backspace => 14,
        Tab => 15,
        KeyQ => 16,
        KeyW => 17,
        KeyE => 18,
        KeyR => 19,
        KeyT => 20,
        KeyY => 21,
        KeyU => 22,
        KeyI => 23,
        KeyO => 24,
        KeyP => 25,
        BracketLeft => 26,
        BracketRight => 27,
        Enter => 28,
        ControlLeft => 29,
        KeyA => 30,
        KeyS => 31,
        KeyD => 32,
        KeyF => 33,
        KeyG => 34,
        KeyH => 35,
        KeyJ => 36,
        KeyK => 37,
        KeyL => 38,
        Semicolon => 39,
        Quote => 40,
        Backquote => 41,
        ShiftLeft => 42,
        Backslash => 43,
        KeyZ => 44,
        KeyX => 45,
        KeyC => 46,
        KeyV => 47,
        KeyB => 48,
        KeyN => 49,
        KeyM => 50,
        Comma => 51,
        Period => 52,
        Slash => 53,
        ShiftRight => 54,
        NumpadMultiply => 55,
        AltLeft => 56,
        Space => 57,
        CapsLock => 58,
        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        NumLock => 69,
        ScrollLock => 70,
        Numpad7 => 71,
        Numpad8 => 72,
        Numpad9 => 73,
        NumpadSubtract => 74,
        Numpad4 => 75,
        Numpad5 => 76,
        Numpad6 => 77,
        NumpadAdd => 78,
        Numpad1 => 79,
        Numpad2 => 80,
        Numpad3 => 81,
        Numpad0 => 82,
        NumpadDecimal => 83,
        IntlBackslash => 86,
        F11 => 87,
        F12 => 88,
        NumpadEnter => 96,
        ControlRight => 97,
        NumpadDivide => 98,
        PrintScreen => 99,
        AltRight => 100,
        Home => 102,
        ArrowUp => 103,
        PageUp => 104,
        ArrowLeft => 105,
        ArrowRight => 106,
        End => 107,
        ArrowDown => 108,
        PageDown => 109,
        Insert => 110,
        Delete => 111,
        Pause => 119,
        SuperLeft => 125,
        SuperRight => 126,
        ContextMenu => 127,
        _ => return None,
    }))
}
fn desktop_key(key: &Key) -> Option<DesktopKey> {
    match key {
        Key::Character(text) => {
            let mut chars = text.chars();
            let ch = chars.next()?;
            chars.next().is_none().then_some(DesktopKey::Char(ch))
        }
        Key::Named(key) => Some(DesktopKey::Code(match key {
            NamedKey::Enter => 28,
            NamedKey::Tab => 15,
            NamedKey::Backspace => 14,
            NamedKey::Delete => 111,
            NamedKey::Insert => 110,
            NamedKey::ArrowUp => 103,
            NamedKey::ArrowDown => 108,
            NamedKey::ArrowLeft => 105,
            NamedKey::ArrowRight => 106,
            NamedKey::Home => 102,
            NamedKey::End => 107,
            NamedKey::PageUp => 104,
            NamedKey::PageDown => 109,
            NamedKey::Shift => 42,
            NamedKey::Control => 29,
            NamedKey::Alt => 56,
            NamedKey::Super => 125,
            NamedKey::Space => 57,
            NamedKey::F1 => 59,
            NamedKey::F2 => 60,
            NamedKey::F3 => 61,
            NamedKey::F4 => 62,
            NamedKey::F5 => 63,
            NamedKey::F6 => 64,
            NamedKey::F7 => 65,
            NamedKey::F8 => 66,
            NamedKey::F9 => 67,
            NamedKey::F10 => 68,
            NamedKey::F11 => 87,
            NamedKey::F12 => 88,
            _ => return None,
        })),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opt_in_defaults_off() {
        assert_eq!(
            crate::ui::UiState::default_setting_value("desktop_computer"),
            Some(serde_json::json!(false))
        );
    }
    #[test]
    fn key_mapping_keeps_unicode_and_navigation() {
        assert_eq!(
            desktop_key(&Key::Character("я".into())),
            Some(DesktopKey::Char('я'))
        );
        assert_eq!(
            desktop_key(&Key::Named(NamedKey::Enter)),
            Some(DesktopKey::Code(28))
        );
    }
    struct RecordingSource(std::sync::Arc<parking_lot::Mutex<Vec<String>>>);
    impl DesktopSource for RecordingSource {
        fn control_granted(&self) -> bool {
            true
        }
        fn next_frame(&mut self) -> Result<Option<super::super::Frame>, String> {
            Ok(None)
        }
        fn send_pointer(&mut self, _: f64, _: f64) -> Result<(), String> {
            Ok(())
        }
        fn send_button(&mut self, button: PointerButton, pressed: bool) -> Result<(), String> {
            self.0.lock().push(format!("{button:?}:{pressed}"));
            Ok(())
        }
        fn send_scroll(&mut self, _: f64, _: f64) -> Result<(), String> {
            Ok(())
        }
        fn send_key(&mut self, key: DesktopKey, pressed: bool) -> Result<(), String> {
            self.0.lock().push(format!("{key:?}:{pressed}"));
            Ok(())
        }
        fn close(&mut self) {
            self.0.lock().push("close".into());
        }
    }
    #[test]
    fn leaving_control_releases_held_input_without_closing_capture() {
        let log = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let mut computer = Computer {
            source: Some(Box::new(RecordingSource(log.clone()))),
            controlling: true,
            keys: vec![(
                PhysicalKey::Code(KeyCode::ControlLeft),
                DesktopKey::Code(29),
            )],
            buttons: vec![PointerButton::Left],
            face: None,
            state: None,
            binding: None,
            epochs: (0, 0),
            size: [0; 2],
            pointer: None,
            managing: false,
            waiting: false,
            picker: Vec::new(),
            #[cfg(target_os = "linux")]
            apps: Vec::new(),
            selection: 0,
            message: String::new(),
            sharing: Default::default(),
        };
        computer.release_input();
        computer.release_input();
        assert!(!computer.controlling);
        assert!(computer.source.is_some());
        assert_eq!(&*log.lock(), &["Code(29):false", "Left:false"]);
        computer.close();
        computer.close();
        assert_eq!(log.lock().last().map(String::as_str), Some("close"));
        assert_eq!(log.lock().len(), 3);
    }
    #[test]
    fn closing_capture_clears_binding_and_accepts_a_fresh_source() {
        let log = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let mut computer = Computer {
            source: Some(Box::new(RecordingSource(log.clone()))),
            face: Some(Face { block: [1, 2, 3], side: 2, width: 1, height: 1 }),
            binding: Some([1, 2, 3]),
            state: Some(10),
            epochs: (9, 3),
            size: [640, 480],
            pointer: None,
            controlling: false,
            managing: false,
            waiting: false,
            keys: Vec::new(),
            buttons: Vec::new(),
            picker: Vec::new(),
            #[cfg(target_os = "linux")]
            apps: Vec::new(),
            selection: 0,
            message: String::new(),
            sharing: Default::default(),
        };
        computer.close();
        assert!(computer.face.is_none() && computer.source.is_none());
        assert_eq!(computer.size, [0, 0]);
        assert_eq!(computer.epochs, (0, 0));
        computer.source = Some(Box::new(RecordingSource(log.clone())));
        assert!(computer.source.as_ref().unwrap().control_granted());
        computer.close();
        assert_eq!(&*log.lock(), &["close", "close"]);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn app_picker_snapshots_arguments_and_keeps_capture_choices() {
        let mut computer = Computer::default();
        computer.picker.push(WindowInfo { id: 0, title: "Desktop window".into() });
        let mut setting = serde_json::json!([["ptyxis", "-s"]]);
        computer.append_apps(Some(&setting));
        setting[0][0] = serde_json::json!("changed");
        assert_eq!(computer.picker.len(), 2);
        assert_eq!(computer.picker[1].title, "Launch app: ptyxis");
        assert_eq!(computer.picker[1].id, 1 << 63);
        assert_eq!(computer.apps[0], ["ptyxis", "-s"]);
        assert!(computer.open_choice((1 << 63) | 1).is_err());
        computer.close();
        assert!(computer.apps.is_empty());
        computer.append_apps(Some(&serde_json::json!([["ptyxis", "-s"]])));
        assert_eq!(computer.picker.len(), 1);
        assert!(computer.has_modal());
    }
    #[test]
    fn streaming_status_preserves_game_debug_overlay_lines() {
        let mut computer = Computer::default();
        computer.message = "LIVE desktop".into();
        let mut lines = vec!["FPS: 60".into(), "XYZ: 1 2 3".into()];
        computer.append_status(&mut lines);
        assert_eq!(lines, ["FPS: 60", "XYZ: 1 2 3", "LIVE desktop"]);
        let mut lines = Vec::new();
        computer.append_status(&mut lines);
        assert_eq!(lines, ["LIVE desktop"]);
    }
    #[test]
    fn single_window_picker_displays_bound_row_before_capture() {
        let mut computer = Computer::default();
        computer.face = Some(Face { block: [1, 2, 3], side: 2, width: 3, height: 1 });
        computer.picker.push(WindowInfo { id: 0, title: "NEW window".into() });
        assert!(computer.has_modal());
        let dialog = computer.dialog().unwrap();
        assert!(dialog.title.contains("3 monitor block(s)"));
        assert_eq!(dialog.entries[0].0, "NEW window");
        assert_eq!(dialog.entries[1].1, crate::ui::desktop::DesktopAction::CancelPicker);
        assert!(computer.source.is_none());
    }
    #[test]
    fn streaming_escape_keeps_vanilla_pause_and_only_control_consumes_it() {
        let log = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let mut computer = Computer::default();
        computer.source = Some(Box::new(RecordingSource(log.clone())));
        assert!(!computer.escape(false));
        assert!(computer.source.is_some());
        computer.controlling = true;
        assert!(computer.escape(false));
        assert!(!computer.controlling && computer.source.is_some());
        assert!(!computer.escape(false));
        assert!(computer.escape(true));
        assert!(computer.source.is_none());
        assert_eq!(&*log.lock(), &["close"]);
    }
    #[test]
    fn owner_menu_exposes_change_stop_and_named_viewer_revocation() {
        let mut computer = Computer::default();
        computer.managing = true;
        computer.sharing.viewers.insert([7; 16], "Viewer".into());
        let dialog = computer.dialog().unwrap();
        use crate::ui::desktop::DesktopAction;
        assert_eq!(dialog.entries[0].1, DesktopAction::ChangeWindow);
        assert_eq!(dialog.entries[1].1, DesktopAction::StopCapture);
        assert!(dialog.entries.iter().any(|(name, action)| name == "Revoke: Viewer" && *action == DesktopAction::RevokeViewer([7; 16])));
    }
    #[test]
    fn physical_keys_preserve_shortcuts_and_modifier_sides() {
        assert_eq!(
            physical_key(PhysicalKey::Code(KeyCode::KeyC)),
            Some(DesktopKey::Code(46))
        );
        assert_eq!(
            physical_key(PhysicalKey::Code(KeyCode::ControlRight)),
            Some(DesktopKey::Code(97))
        );
        assert_eq!(
            physical_key(PhysicalKey::Code(KeyCode::AltRight)),
            Some(DesktopKey::Code(100))
        );
    }
}
