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
    pointer_outside_game: bool,
    keys: Vec<(PhysicalKey, DesktopKey)>,
    buttons: Vec<PointerButton>,
    picker: Vec<WindowInfo>,
    selection: usize,
    message: String,
}
impl Computer {
    pub fn has_overlay(&self) -> bool {
        !self.picker.is_empty() || !self.message.is_empty()
    }
    pub fn lines(&self) -> Vec<String> {
        if !self.picker.is_empty() {
            let mut lines =
                vec!["LOCAL desktop window: Up/Down, Enter selects, Esc cancels".into()];
            let start = self.selection.saturating_sub(5);
            lines.extend(
                self.picker
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(12)
                    .map(|(i, w)| {
                        format!(
                            "{} {}",
                            if i == self.selection { ">" } else { " " },
                            w.title
                        )
                    }),
            );
            lines
        } else if !self.message.is_empty() {
            vec![self.message.clone()]
        } else {
            Vec::new()
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
    fn close(&mut self) {
        self.release_input();
        if let Some(mut source) = self.source.take() {
            source.close();
        }
        self.face = None;
        self.state = None;
        self.binding = None;
        self.pointer_outside_game = false;
        self.epochs = (0, 0);
        self.selection = 0;
        self.size = [0; 2];
        self.picker.clear();
        self.message.clear();
    }
    fn start(&mut self, id: u64) {
        self.picker.clear();
        match super::open(id) {
            Ok(source) => {
                self.source = Some(source);
                self.message="Approve the system window/capture/control dialog. Esc cancels; Shift+Esc closes.".into();
            }
            Err(error) => self.fail(error),
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
            if let Some(row) = Face::row(binding, face.side, |p| self.session.world.block(p) == self.computer.state) {
                self.computer.face = Some(row);
                if let Some(renderer) = &mut self.renderer { renderer.set_desktop_face(row); }
            } else {
                self.computer.close();
            }
        }
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
                        let first = self.computer.size == [0; 2];
                        self.computer.size = size;
                        renderer.update_desktop(face, &frame);
                        if first {
                            eprintln!("desktop frame arrived: {}x{}", frame.width, frame.height);
                            self.computer.message = if source.control_granted() {
                                "LIVE desktop. Right-click to control; Esc releases; Shift+Esc closes"
                            } else {
                                "View only — enable 'Allow remote interaction' in the share dialog to control"
                            }.into();
                            self.publish_session();
                        }
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
    pub(crate) fn computer_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorLeft { .. } => self.computer.pointer_outside_game = true,
            WindowEvent::CursorEntered { .. } | WindowEvent::CursorMoved { .. } =>
                self.computer.pointer_outside_game = false,
            _ => {}
        }
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
        if let WindowEvent::KeyboardInput { event, .. } = event {
            let pressed = event.state == ElementState::Pressed;
            if event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                && pressed
                && (self.computer.source.is_some() || !self.computer.picker.is_empty())
            {
                if self.shift_held || self.computer.size == [0; 2] {
                    self.computer.close();
                } else {
                    self.computer.release_input();
                    self.computer.message =
                        "Desktop control released. Right-click screen resumes; Shift+Esc closes"
                            .into();
                }
                self.publish_session();
                return true;
            }
            if !self.computer.picker.is_empty() {
                if pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::ArrowDown) => {
                            self.computer.selection =
                                (self.computer.selection + 1) % self.computer.picker.len()
                        }
                        PhysicalKey::Code(KeyCode::ArrowUp) => {
                            self.computer.selection =
                                (self.computer.selection + self.computer.picker.len() - 1)
                                    % self.computer.picker.len()
                        }
                        PhysicalKey::Code(KeyCode::Enter) => {
                            let id = self.computer.picker[self.computer.selection].id;
                            self.computer.start(id);
                        }
                        _ => {}
                    }
                    self.publish_session();
                }
                return true;
            }
            if self.computer.controlling {
                if matches!(
                    event.physical_key,
                    PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight)
                ) {
                    self.shift_held = pressed;
                }
                // RemoteDesktop injects into the seat, not into the captured window.
                // Never feed keys back into this focused game.
                if cfg!(target_os = "linux") && pressed && self.focused {
                    return true;
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
                if cfg!(target_os = "linux") && pressed && !self.computer.pointer_outside_game {
                    return true;
                }
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
                let Some(face) = Face::row(hit.position, hit.face, |p| self.session.world.block(p) == Some(hit.state)) else {
                    self.computer.message = "Monitor rows support at most 16 blocks".into();
                    self.publish_session();
                    return true;
                };
                let _ = self
                    .session
                    .command(Command::Input(InputIntent::ReleaseAll));
                if self.computer.face == Some(face) && self.computer.size != [0; 2] {
                    if !self.computer.source.as_ref().is_some_and(|source| source.control_granted()) {
                        self.computer.message = "View only — enable 'Allow remote interaction' in the share dialog to control".into();
                        self.publish_session();
                        return true;
                    }
                    if cfg!(target_os = "linux")
                        && self.window.as_ref().is_some_and(|window| window.fullscreen().is_some()) {
                        self.computer.message = "Wayland input requires windowed mode; keep the captured window uncovered".into();
                        self.publish_session();
                        return true;
                    }
                    self.computer.controlling = true;
                    if let Some(window) = &self.window {
                        #[cfg(target_os = "linux")]
                        {
                            let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
                            window.set_cursor_visible(true);
                            self.cursor_locked = false;
                            self.computer.pointer_outside_game = false;
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                        use winit::window::CursorGrabMode;
                        if window
                            .set_cursor_grab(CursorGrabMode::Locked)
                            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                            .is_ok()
                        {
                            window.set_cursor_visible(false);
                            self.cursor_locked = true;
                        }
                        }
                    }
                    self.computer.message = if cfg!(target_os = "linux") {
                        "Keep captured window uncovered. Seat input is blocked over the game; keyboard cannot target a background window. Esc releases."
                    } else {
                        "CONTROLLING LOCAL DESKTOP — Esc releases; Shift+Esc closes"
                    }.into();
                } else {
                    self.computer.close();
                    self.computer.face = Some(face);
                    self.computer.state = Some(hit.state);
                    self.computer.binding = Some(hit.position);
                    self.computer.epochs = self.session.context_epochs();
                    match super::list_windows() {
                        Ok(windows) if cfg!(target_os = "linux") && windows.len() == 1 => {
                            self.computer.start(windows[0].id)
                        }
                        Ok(windows) if !windows.is_empty() => {
                            self.computer.picker = windows;
                            self.computer.selection = 0;
                        }
                        Ok(_) => self.computer.fail("No capturable desktop windows".into()),
                        Err(error) => self.computer.fail(error),
                    }
                }
                self.publish_session();
                return true;
            }
        }
        if self.computer.controlling
            && let WindowEvent::MouseWheel { delta, .. } = event
        {
            if cfg!(target_os = "linux") && !self.computer.pointer_outside_game {
                return true;
            }
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
            pointer_outside_game: false,
            picker: Vec::new(),
            selection: 0,
            message: String::new(),
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
            face: Some(Face { block: [1, 2, 3], side: 2, width: 1 }),
            binding: Some([1, 2, 3]),
            state: Some(10),
            epochs: (9, 3),
            size: [640, 480],
            pointer: None,
            controlling: false,
            pointer_outside_game: false,
            keys: Vec::new(),
            buttons: Vec::new(),
            picker: Vec::new(),
            selection: 0,
            message: String::new(),
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
