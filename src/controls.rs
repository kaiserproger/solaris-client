//! Player-configurable physical keyboard and mouse bindings.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use winit::{event::MouseButton, keyboard::KeyCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
}
impl Binding {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Key(key) => {
                let name = format!("{key:?}");
                name.strip_prefix("Key").or_else(|| name.strip_prefix("Digit"))
                    .unwrap_or(&name).to_owned()
            }
            Self::Mouse(button) => format!("Mouse {button:?}"),
        }
    }
}

pub(crate) const ACTIONS: &[(&str, &str, Binding)] = &[
    ("forward", "Forward", Binding::Key(KeyCode::KeyW)),
    ("backward", "Backward", Binding::Key(KeyCode::KeyS)),
    ("left", "Left", Binding::Key(KeyCode::KeyA)),
    ("right", "Right", Binding::Key(KeyCode::KeyD)),
    ("jump", "Jump", Binding::Key(KeyCode::Space)),
    ("sneak", "Sneak", Binding::Key(KeyCode::ShiftLeft)),
    ("sprint", "Sprint", Binding::Key(KeyCode::ControlLeft)),
    ("attack", "Attack", Binding::Mouse(MouseButton::Left)),
    ("use", "Use", Binding::Mouse(MouseButton::Right)),
    ("inventory", "Inventory", Binding::Key(KeyCode::KeyE)),
    ("drop", "Drop", Binding::Key(KeyCode::KeyQ)),
    ("swap_offhand", "Swap offhand", Binding::Key(KeyCode::KeyF)),
    ("player_list", "Player list", Binding::Key(KeyCode::Tab)),
    ("chat", "Chat", Binding::Key(KeyCode::KeyT)),
    ("command", "Command", Binding::Key(KeyCode::Slash)),
    ("screenshot", "Screenshot", Binding::Key(KeyCode::F2)),
    ("debug", "Debug overlay", Binding::Key(KeyCode::F3)),
    ("perspective", "Perspective", Binding::Key(KeyCode::F5)),
    ("hotbar_1", "Hotbar 1", Binding::Key(KeyCode::Digit1)),
    ("hotbar_2", "Hotbar 2", Binding::Key(KeyCode::Digit2)),
    ("hotbar_3", "Hotbar 3", Binding::Key(KeyCode::Digit3)),
    ("hotbar_4", "Hotbar 4", Binding::Key(KeyCode::Digit4)),
    ("hotbar_5", "Hotbar 5", Binding::Key(KeyCode::Digit5)),
    ("hotbar_6", "Hotbar 6", Binding::Key(KeyCode::Digit6)),
    ("hotbar_7", "Hotbar 7", Binding::Key(KeyCode::Digit7)),
    ("hotbar_8", "Hotbar 8", Binding::Key(KeyCode::Digit8)),
    ("hotbar_9", "Hotbar 9", Binding::Key(KeyCode::Digit9)),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Bindings(pub(crate) BTreeMap<String, Binding>);

impl Default for Bindings {
    fn default() -> Self {
        Self(ACTIONS.iter().map(|(id, _, key)| ((*id).into(), *key)).collect())
    }
}
impl Bindings {
    pub(crate) fn from_value(value: &serde_json::Value) -> Option<Self> {
        let map: BTreeMap<String, Binding> = serde_json::from_value(value.clone()).ok()?;
        if map.len() != ACTIONS.len() || ACTIONS.iter().any(|(id, _, _)| !map.contains_key(*id)) {
            return None;
        }
        Some(Self(map))
    }
    pub(crate) fn value(&self) -> serde_json::Value {
        serde_json::to_value(&self.0).expect("binding table serializes")
    }
    pub(crate) fn action(&self, key: Binding) -> Option<&'static str> {
        ACTIONS.iter().find_map(|(id, _, _)| (self.0.get(*id) == Some(&key)).then_some(*id))
    }
    pub(crate) fn conflict(&self, id: &str) -> bool {
        self.0.get(id).is_some_and(|key| self.0.values().filter(|other| *other == key).count() > 1)
    }
    pub(crate) fn set(&mut self, id: &str, key: Binding) {
        if let Some(value) = self.0.get_mut(id) { *value = key; }
    }
    pub(crate) fn reset(&mut self, id: &str) {
        if let Some((_, _, key)) = ACTIONS.iter().find(|(action, _, _)| *action == id) { self.set(id, *key); }
    }
}
