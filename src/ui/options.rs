//! Sodium-style Options: category tabs on the left, a list of
//! label + slider / switch rows on the right, one-line description at the
//! bottom while a row is hovered or focused. Values apply instantly through
//! the same `UiCommand::Settings` path the old cycling buttons used.
use super::state::{NodeAction, Paint, Screen, UiState};
use crate::protocol::Result;
use crate::render::GraphicsPreset;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Slider,
    Toggle,
    Choice,
}
pub(super) struct Row {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub desc: &'static str,
}
const fn row(key: &'static str, label: &'static str, kind: Kind, desc: &'static str) -> Row {
    Row {
        key,
        label,
        kind,
        desc,
    }
}

pub(super) const TABS: [&str; 4] = ["Video", "Controls", "Audio", "Interface"];
/// Tab shown for the Accessibility entry of the pause menu.
pub(super) const INTERFACE_TAB: usize = 3;

const VIDEO: [Row; 6] = [
    row(
        "fov",
        "Field of view",
        Kind::Slider,
        "Wider view shows more but stretches the edges.",
    ),
    row(
        "render_distance",
        "Render distance",
        Kind::Slider,
        "Chunks drawn around you. Lower runs faster.",
    ),
    row(
        "max_fps",
        "Max FPS",
        Kind::Slider,
        "Caps FPS with VSync off. Far right is unlimited.",
    ),
    row(
        "vsync",
        "VSync",
        Kind::Toggle,
        "Syncs frames to your display to stop tearing.",
    ),
    row(
        "fullscreen",
        "Fullscreen",
        Kind::Toggle,
        "Borderless fullscreen window.",
    ),
    row(
        "graphics_preset",
        "Graphics",
        Kind::Choice,
        "Low fast vanilla; Medium rich; High max.",
    ),
];
const CONTROLS: [Row; 2] = [
    row(
        "mouse_sensitivity",
        "Sensitivity",
        Kind::Slider,
        "How fast the camera turns with the mouse.",
    ),
    row(
        "damage_tilt_strength",
        "Damage tilt",
        Kind::Slider,
        "How strongly the camera shakes when hurt.",
    ),
];
const AUDIO: [Row; 2] = [
    row(
        "volume",
        "Master volume",
        Kind::Slider,
        "Loudness of all game sounds.",
    ),
    row(
        "subtitles",
        "Subtitles",
        Kind::Toggle,
        "Show captions for sounds that play nearby.",
    ),
];
const INTERFACE: [Row; 8] = [
    row(
        "ui_scale",
        "GUI scale",
        Kind::Slider,
        "Interface size. Auto fits it to your window.",
    ),
    row(
        "hide_hud",
        "Hide HUD",
        Kind::Toggle,
        "Hide the heads-up display, e.g. for screenshots.",
    ),
    row(
        "high_contrast",
        "High contrast",
        Kind::Toggle,
        "Stronger outlines and colours in the interface.",
    ),
    row(
        "narrator",
        "Narrator",
        Kind::Choice,
        "Reads text aloud. Click to cycle the modes.",
    ),
    row(
        "reduce_motion",
        "Reduced motion",
        Kind::Toggle,
        "Calmer interface: fades only, no sliding.",
    ),
    row(
        "chat_background",
        "Chat background",
        Kind::Toggle,
        "Draw a backdrop behind chat text.",
    ),
    row(
        "text_background",
        "Text background",
        Kind::Toggle,
        "Backdrop behind in-world text.",
    ),
    row(
        "text_background_opacity",
        "Text opacity",
        Kind::Slider,
        "Opacity of the text backdrop.",
    ),
];

pub(super) fn rows(tab: usize) -> &'static [Row] {
    match tab {
        0 => &VIDEO,
        1 => &CONTROLS,
        2 => &AUDIO,
        _ => &INTERFACE,
    }
}

/// Slider geometry: `positions` discrete stops from `min` by `step`.
struct Spec {
    min: f64,
    step: f64,
    positions: usize,
}
fn spec(key: &str) -> Option<Spec> {
    let (min, step, positions) = match key {
        "fov" => (30., 1., 81),
        "render_distance" => (2., 1., 15),
        "mouse_sensitivity" => (0.1, 0.05, 59),
        // 30..=260 in tens, then one extra stop for Unlimited (value 0).
        "max_fps" => (30., 10., 25),
        "ui_scale" => (0., 1., 5),
        "volume" | "damage_tilt_strength" | "text_background_opacity" => (0., 0.05, 21),
        _ => return None,
    };
    Some(Spec {
        min,
        step,
        positions,
    })
}

/// Number of discrete stops of a slider key.
pub(super) fn slider_positions(key: &str) -> Option<usize> {
    spec(key).map(|s| s.positions)
}

/// Setting value for stop `pos` (clamped).
pub(super) fn slider_value(key: &str, pos: usize) -> Option<Value> {
    let s = spec(key)?;
    let pos = pos.min(s.positions - 1);
    Some(match key {
        "max_fps" if pos == s.positions - 1 => json!(0),
        "max_fps" => json!((s.min + s.step * pos as f64) as u64),
        "render_distance" => json!((s.min + s.step * pos as f64) as u64),
        _ => json!(((s.min + s.step * pos as f64) * 1e4).round() / 1e4),
    })
}

/// Nearest stop for a stored value.
pub(super) fn slider_pos(key: &str, value: &Value) -> usize {
    let Some(s) = spec(key) else { return 0 };
    let Some(v) = value.as_f64() else { return 0 };
    if key == "max_fps" && v == 0. {
        return s.positions - 1;
    }
    let last = if key == "max_fps" {
        s.positions - 2
    } else {
        s.positions - 1
    };
    (((v - s.min) / s.step).round().max(0.) as usize).min(last)
}

/// Horizontal span `(x, width)` of the slider track inside a row.
pub(super) fn track_span([x, _, w, _]: [f32; 4]) -> (f32, f32) {
    let tx = (x + w * 0.42).floor();
    (tx, (x + w - 64. - tx).max(24.))
}

/// Stop under pointer x for a slider row.
pub(super) fn pos_at(key: &str, bounds: [f32; 4], px: f32) -> Option<usize> {
    let n = slider_positions(key)?;
    let (tx, tw) = track_span(bounds);
    let t = ((px - tx - 3.) / (tw - 6.).max(1.)).clamp(0., 1.);
    Some((t * (n - 1) as f32).round() as usize)
}

fn default_bool(key: &str) -> bool {
    matches!(key, "vsync" | "text_background")
}

pub(super) fn value_text(key: &str, value: Option<&Value>) -> String {
    let pct = |v: f64| format!("{}%", (v * 100.).round() as i64);
    let num = value.and_then(Value::as_f64);
    match key {
        "fov" => format!("{}", num.unwrap_or(70.).round() as i64),
        "render_distance" => format!("{} chunks", num.unwrap_or(8.).round() as i64),
        "max_fps" => match num.unwrap_or(120.) as u64 {
            0 => "Unlimited".into(),
            v => format!("{v} fps"),
        },
        "ui_scale" => match num.unwrap_or(0.) as u64 {
            0 => "Auto".into(),
            v => format!("{v}x"),
        },
        "mouse_sensitivity" => pct(num.unwrap_or(1.)),
        "damage_tilt_strength" => pct(num.unwrap_or(1.)),
        "volume" => pct(num.unwrap_or(1.)),
        "text_background_opacity" => pct(num.unwrap_or(0.5)),
        "narrator" => ["Off", "All", "Chat", "System"]
            [value.and_then(Value::as_u64).unwrap_or(0) as usize % 4]
            .into(),
        "graphics_preset" => value
            .and_then(GraphicsPreset::from_setting)
            .unwrap_or_default()
            .label()
            .into(),
        _ => if value.and_then(Value::as_bool).unwrap_or(default_bool(key)) {
            "On"
        } else {
            "Off"
        }
        .into(),
    }
}

impl UiState {
    fn setting_or_default(&self, key: &str) -> Option<Value> {
        self.settings
            .get(key)
            .cloned()
            .or_else(|| Self::default_setting_value(key))
    }

    /// Vertical stride of one list row and the row height itself.
    const ROW_PITCH: f32 = 24.;
    const ROW_HEIGHT: f32 = 22.;

    pub(crate) fn options_tree(&mut self) -> Result<()> {
        if self.options_tab >= TABS.len() {
            self.options_tab = 0;
        }
        self.panel();
        let pw = (self.width - 24.).clamp(300., 400.);
        let ph = (self.height - 16.).max(150.);
        let x0 = ((self.width - pw) / 2.).floor();
        let y0 = ((self.height - ph) / 2.).floor();
        self.add(
            "options/panel".into(),
            "presentation",
            String::new(),
            [x0, y0, pw, ph],
            true,
            None,
            NodeAction::None,
            Paint::Frame,
        )?;
        self.label("options/title", "Options".into(), [x0, y0 + 7., pw, 12.])?;
        for (i, name) in TABS.iter().enumerate() {
            self.add(
                format!("options/tab/{i}"),
                "tab",
                (*name).into(),
                [x0 + 8., y0 + 26. + i as f32 * 26., 84., 24.],
                true,
                None,
                NodeAction::OptionsTab(i),
                Paint::Tab(i == self.options_tab),
            )?;
        }
        self.button(
            "settings/done",
            "Done",
            [x0 + 8., y0 + ph - 32., 84., 24.],
            NodeAction::Screen(self.settings_parent),
            true,
            None,
        )?;
        let lx = x0 + 100.;
        let lw = pw - 100. - 10.;
        let list_top = y0 + 26.;
        let footer_h = 24.;
        self.options_footer = [lx, y0 + ph - footer_h - 2., lw, footer_h];
        let rows = rows(self.options_tab);
        let total = rows.len() + if self.options_tab == 1 { crate::controls::ACTIONS.len() + 1 } else { 0 };
        let visible = (((ph - 26. - footer_h - 8.) + 2.) / Self::ROW_PITCH)
            .floor()
            .max(1.) as usize;
        self.scroll = self.scroll.min(total.saturating_sub(visible));
        let scroll = self.scroll;
        for (i, row) in rows.iter().skip(scroll).take(visible).enumerate() {
            let value = self.setting_or_default(row.key);
            let text = value_text(row.key, value.as_ref());
            let bounds = [
                lx,
                list_top + i as f32 * Self::ROW_PITCH,
                lw,
                Self::ROW_HEIGHT,
            ];
            let label = format!("{}: {text}", row.label);
            let (role, action, paint) = match row.kind {
                Kind::Slider => {
                    let pos = value.as_ref().map_or(0, |v| slider_pos(row.key, v));
                    let n = slider_positions(row.key).unwrap_or(2);
                    (
                        "slider",
                        NodeAction::Slider(row.key),
                        Paint::Slider {
                            t: pos as f32 / (n - 1) as f32,
                            desc: row.desc,
                        },
                    )
                }
                Kind::Toggle => {
                    let on = value
                        .as_ref()
                        .and_then(Value::as_bool)
                        .unwrap_or(default_bool(row.key));
                    (
                        "switch",
                        NodeAction::Setting(row.key),
                        Paint::Toggle { on, desc: row.desc },
                    )
                }
                Kind::Choice => (
                    "button",
                    NodeAction::Setting(row.key),
                    Paint::Split {
                        delim: ": ",
                        desc: row.desc,
                        conflict: false,
                    },
                ),
            };
            self.add(
                format!("settings/{}", row.key),
                role,
                label,
                bounds,
                true,
                None,
                action,
                paint,
            )?;
        }
        if self.options_tab == 1 {
            let bindings = self.settings.get("key_bindings")
                .and_then(crate::controls::Bindings::from_value).unwrap_or_default();
            for (index, (id, label, _)) in crate::controls::ACTIONS.iter().enumerate() {
                let row = rows.len() + index;
                if row < scroll || row >= scroll + visible { continue; }
                let y = list_top + (row - scroll) as f32 * Self::ROW_PITCH;
                let value = if self.rebind == Some(*id) {
                    "Press a key...".to_string()
                } else {
                    bindings.0[*id].label()
                };
                self.add(
                    format!("controls/bind/{id}"), "button", format!("{label}: {value}"),
                    [lx, y, lw - 48., Self::ROW_HEIGHT], true, None,
                    NodeAction::Rebind(id), Paint::Split { delim: ": ", desc: "Click then press a key or mouse button.", conflict: bindings.conflict(id) },
                )?;
                self.button(
                    &format!("controls/reset/{id}"), "Reset",
                    [lx + lw - 46., y, 46., Self::ROW_HEIGHT],
                    NodeAction::ResetBinding(id), true, None,
                )?;
            }
            let row = rows.len() + crate::controls::ACTIONS.len();
            if row >= scroll && row < scroll + visible {
                self.button("controls/reset_all", "Reset all",
                    [lx, list_top + (row - scroll) as f32 * Self::ROW_PITCH, lw, Self::ROW_HEIGHT],
                    NodeAction::ResetBindings, true, None)?;
            }
        }
        if total > visible {
            let track_h = visible as f32 * Self::ROW_PITCH - 2.;
            let thumb_h = (track_h * visible as f32 / total as f32)
                .max(8.)
                .floor();
            let travel = track_h - thumb_h;
            let max = (total - visible) as f32;
            self.add(
                "options/scrollbar".into(),
                "presentation",
                String::new(),
                [
                    lx + lw + 3.,
                    (list_top + travel * scroll as f32 / max).floor(),
                    2.,
                    thumb_h,
                ],
                true,
                None,
                NodeAction::None,
                Paint::Solid(super::theme::ACCENT, 0.8),
            )?;
        }
        Ok(())
    }

    /// Emit the settings command for `value` (the one path every control uses).
    pub(crate) fn commit_setting(
        &mut self,
        key: &'static str,
        value: Value,
        e: &mut super::state::UiEffects,
    ) {
        self.settings.insert(key.into(), value.clone());
        e.commands.push(super::state::UiCommand::Settings {
            key: key.into(),
            value: value.clone(),
        });
        if key == "ui_scale"
            && let Some(scale) = value.as_f64()
        {
            e.commands.push(super::state::UiCommand::Window(
                super::state::WindowEffect::UiScale(scale),
            ));
        }
        self.invalidate();
    }

    /// Move a slider to stop `pos`; no-op when it is already there.
    pub(crate) fn slide_to(
        &mut self,
        key: &'static str,
        pos: usize,
        e: &mut super::state::UiEffects,
    ) {
        let Some(value) = slider_value(key, pos) else {
            return;
        };
        let current = self.setting_or_default(key);
        if current.as_ref().is_some_and(|c| slider_pos(key, c) == pos) {
            return;
        }
        self.commit_setting(key, value, e);
    }

    /// Step a slider by whole stops (keyboard arrows, wheel notches).
    pub(crate) fn slide_by(
        &mut self,
        key: &'static str,
        delta: i32,
        e: &mut super::state::UiEffects,
    ) {
        let Some(n) = slider_positions(key) else {
            return;
        };
        let now = self
            .setting_or_default(key)
            .map_or(0, |v| slider_pos(key, &v)) as i32;
        self.slide_to(key, (now + delta).clamp(0, n as i32 - 1) as usize, e);
    }

    /// Slider key under node `id`, if it is a slider row.
    pub(crate) fn slider_key(&self, id: &str) -> Option<&'static str> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| match n.action {
                NodeAction::Slider(key) => Some(key),
                _ => None,
            })
    }

    /// Select an options tab (resets the list scroll).
    pub(crate) fn select_options_tab(&mut self, tab: usize) {
        if tab < TABS.len() && tab != self.options_tab {
            self.options_tab = tab;
            self.scroll = 0;
        }
        self.invalidate();
    }

    pub fn is_options_screen(&self) -> bool {
        matches!(self.screen, Screen::Settings | Screen::Accessibility)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inventory::InventoryStore,
        protocol::content::Content,
        ui::{Modifiers, PointerButton, PointerPhase, UiCommand, UiEffects, UiIntent},
    };

    #[test]
    fn controls_tab_exposes_rebindable_forward_action() {
        let (content, mut inventory, mut ui) = state();
        open_options(&mut ui, &mut inventory, &content);
        ui.activate_for_test(NodeAction::OptionsTab(1), &mut inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert!(ui.nodes.iter().any(|node| node.id == "controls/bind/forward"));
        let [x, y, w, h] = node_bounds(&ui, "controls/bind/forward");
        pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, x + w / 2., y + h / 2.);
        pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, x + w / 2., y + h / 2.);
        assert_eq!(ui.rebind, Some("forward"));
    }

    #[test]
    fn controls_reset_buttons_update_the_persisted_setting() {
        let (content, mut inventory, mut ui) = state();
        let mut bindings = crate::controls::Bindings::default();
        bindings.set("forward", crate::controls::Binding::Key(winit::keyboard::KeyCode::KeyA));
        ui.apply_setting("key_bindings", bindings.value()).unwrap();
        let effects = ui.activate_for_test(NodeAction::ResetBinding("forward"), &mut inventory, &content).unwrap();
        assert!(matches!(&effects.commands[0], UiCommand::Settings { key, value }
            if key == "key_bindings" && !crate::controls::Bindings::from_value(value).unwrap().conflict("forward")));
        ui.activate_for_test(NodeAction::Rebind("jump"), &mut inventory, &content).unwrap();
        assert_eq!(ui.rebind, Some("jump"));
        let effects = ui.activate_for_test(NodeAction::ResetBindings, &mut inventory, &content).unwrap();
        assert!(matches!(&effects.commands[0], UiCommand::Settings { key, .. } if key == "key_bindings"));
    }

    fn state() -> (Content, InventoryStore, UiState) {
        let mut content = Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        let mut ui = UiState::new_state(&content).unwrap();
        ui.resize(427, 240, 1.0);
        (content, InventoryStore::default(), ui)
    }
    fn open_options(ui: &mut UiState, inventory: &mut InventoryStore, content: &Content) {
        ui.transition(Screen::Title);
        ui.activate_for_test(NodeAction::Screen(Screen::Settings), inventory, content)
            .unwrap();
        ui.rebuild_for_test(inventory, content).unwrap();
    }
    fn node_bounds(ui: &UiState, id: &str) -> [f32; 4] {
        ui.nodes.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{id}")).bounds
    }
    fn pointer(
        ui: &mut UiState,
        inventory: &mut InventoryStore,
        content: &Content,
        phase: PointerPhase,
        x: f32,
        y: f32,
    ) -> UiEffects {
        ui.execute(
            UiIntent::Pointer {
                phase,
                x: f64::from(x),
                y: f64::from(y),
                button: Some(PointerButton::Left),
                modifiers: Modifiers::default(),
            },
            inventory,
            content,
        )
        .unwrap()
    }
    fn setting_commands(effects: &UiEffects, key: &str) -> Vec<Value> {
        effects
            .commands
            .iter()
            .filter_map(|c| match c {
                UiCommand::Settings { key: k, value } if k == key => Some(value.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn options_tree_has_tabs_big_hit_areas_and_fits_the_viewport() {
        let (content, mut inventory, mut ui) = state();
        open_options(&mut ui, &mut inventory, &content);
        let ids: Vec<_> = ui.nodes.iter().map(|n| n.id.as_str()).collect();
        for tab in 0..4 {
            assert!(ids.contains(&format!("options/tab/{tab}").as_str()), "{ids:?}");
        }
        for key in ["fov", "render_distance", "max_fps", "vsync", "fullscreen", "graphics_preset"] {
            assert!(ids.contains(&format!("settings/{key}").as_str()), "{key}");
        }
        for node in &ui.nodes {
            let [x, y, w, h] = node.bounds;
            assert!(x >= 0. && y >= 0. && x + w <= 427. && y + h <= 240., "{} {:?}", node.id, node.bounds);
            if matches!(node.role, "slider" | "switch" | "tab" | "button") {
                // 32 physical px at GUI scale 2 is 16 logical px.
                assert!(h >= 22., "{} is {h}px tall", node.id);
            }
        }
        let fov = ui.nodes.iter().find(|n| n.id == "settings/fov").unwrap();
        assert_eq!(fov.role, "slider");
        assert!(fov.label.starts_with("Field of view: "), "{}", fov.label);
        assert_eq!(ui.nodes.iter().find(|n| n.id == "settings/vsync").unwrap().role, "switch");
    }

    #[test]
    fn dragging_a_slider_sets_exact_stops_and_persists_through_commands() {
        let (content, mut inventory, mut ui) = state();
        open_options(&mut ui, &mut inventory, &content);
        let bounds = node_bounds(&ui, "settings/fov");
        let (tx, tw) = track_span(bounds);
        let y = bounds[1] + 11.;
        let down = pointer(&mut ui, &mut inventory, &content, PointerPhase::Down, tx + 3., y);
        assert_eq!(setting_commands(&down, "fov"), vec![json!(30.)]);
        assert!(ui.slider_drag.is_some());
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let moved = pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, tx + tw + 80., y);
        assert_eq!(setting_commands(&moved, "fov"), vec![json!(110.)]);
        assert_eq!(ui.setting("fov"), Some(&json!(110.)));
        // Same stop again emits nothing.
        let again = pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, tx + tw + 90., y);
        assert!(setting_commands(&again, "fov").is_empty());
        pointer(&mut ui, &mut inventory, &content, PointerPhase::Up, tx + tw, y);
        assert!(ui.slider_drag.is_none());
        // A move after release no longer drags.
        let after = pointer(&mut ui, &mut inventory, &content, PointerPhase::Move, tx + 3., y);
        assert!(setting_commands(&after, "fov").is_empty());
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let label = &ui.nodes.iter().find(|n| n.id == "settings/fov").unwrap().label;
        assert_eq!(label, "Field of view: 110");
    }

    #[test]
    fn wheel_and_arrow_keys_step_sliders_by_one_stop() {
        let (content, mut inventory, mut ui) = state();
        open_options(&mut ui, &mut inventory, &content);
        let b = node_bounds(&ui, "settings/render_distance");
        let wheel = |ui: &mut UiState, inventory: &mut InventoryStore, dy: f32| {
            ui.execute(
                UiIntent::Scroll {
                    x: f64::from(b[0] + 20.),
                    y: f64::from(b[1] + 5.),
                    delta_x: 0.,
                    delta_y: dy,
                    modifiers: Modifiers::default(),
                },
                inventory,
                &content,
            )
            .unwrap()
        };
        let up = wheel(&mut ui, &mut inventory, 1.);
        assert_eq!(setting_commands(&up, "render_distance"), vec![json!(9)]);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let down = wheel(&mut ui, &mut inventory, -1.);
        assert_eq!(setting_commands(&down, "render_distance"), vec![json!(8)]);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.focus = Some("settings/render_distance".into());
        let key = |ui: &mut UiState, inventory: &mut InventoryStore, name: &str, shift: bool| {
            ui.execute(
                UiIntent::Key {
                    key: name.into(),
                    pressed: true,
                    modifiers: Modifiers { shift, ..Default::default() },
                },
                inventory,
                &content,
            )
            .unwrap()
        };
        let right = key(&mut ui, &mut inventory, "ArrowRight", false);
        assert_eq!(setting_commands(&right, "render_distance"), vec![json!(9)]);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let end = key(&mut ui, &mut inventory, "End", false);
        assert_eq!(setting_commands(&end, "render_distance"), vec![json!(16)]);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let home = key(&mut ui, &mut inventory, "Home", false);
        assert_eq!(setting_commands(&home, "render_distance"), vec![json!(2)]);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let big = key(&mut ui, &mut inventory, "ArrowRight", true);
        assert_eq!(setting_commands(&big, "render_distance"), vec![json!(7)]);
    }

    #[test]
    fn switch_activation_and_tabs_apply_instantly() {
        let (content, mut inventory, mut ui) = state();
        open_options(&mut ui, &mut inventory, &content);
        let fx = ui
            .activate_for_test(NodeAction::Setting("vsync"), &mut inventory, &content)
            .unwrap();
        assert_eq!(setting_commands(&fx, "vsync"), vec![json!(false)]);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let vsync = ui.nodes.iter().find(|n| n.id == "settings/vsync").unwrap();
        assert_eq!(vsync.label, "VSync: Off");
        assert!(matches!(vsync.paint, Paint::Toggle { on: false, .. }));
        // Tab switch swaps the row set.
        ui.activate_for_test(NodeAction::OptionsTab(2), &mut inventory, &content)
            .unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert!(ui.nodes.iter().any(|n| n.id == "settings/volume"));
        assert!(ui.nodes.iter().any(|n| n.id == "settings/subtitles"));
        assert!(!ui.nodes.iter().any(|n| n.id == "settings/fov"));
    }

    #[test]
    fn interface_tab_scrolls_and_accessibility_opens_it() {
        let (content, mut inventory, mut ui) = state();
        ui.transition(Screen::Pause);
        ui.activate_for_test(NodeAction::Screen(Screen::Accessibility), &mut inventory, &content)
            .unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert_eq!(ui.options_tab, INTERFACE_TAB);
        assert!(ui.nodes.iter().any(|n| n.id == "settings/ui_scale"));
        let first_rows = ui.nodes.iter().filter(|n| n.id.starts_with("settings/") && n.id != "settings/done").count();
        assert!(first_rows < 8, "8 interface rows overflow a 240px viewport: {first_rows}");
        assert!(ui.nodes.iter().any(|n| n.id == "options/scrollbar"));
        ui.scroll = 99;
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert!(ui.nodes.iter().any(|n| n.id == "settings/text_background_opacity"), "scroll clamps to the last row");
        assert!(!ui.nodes.iter().any(|n| n.id == "settings/ui_scale"));
        // Done returns to the pause menu.
        assert_eq!(ui.settings_parent, Screen::Pause);
    }


    #[test]
    fn every_slider_round_trips_through_every_stop() {
        for tab in 0..TABS.len() {
            for row in rows(tab).iter().filter(|r| r.kind == Kind::Slider) {
                let n = slider_positions(row.key).unwrap();
                let mut last = f64::NEG_INFINITY;
                for pos in 0..n {
                    let value = slider_value(row.key, pos).unwrap();
                    assert_eq!(slider_pos(row.key, &value), pos, "{} stop {pos}", row.key);
                    let v = value.as_f64().unwrap();
                    // Session-only keys (e.g. text opacity) are not sanitised.
                    if UiState::MANAGED_SETTING_KEYS.contains(&row.key) {
                        let sane = UiState::sanitize_setting_value(row.key, &value)
                            .unwrap_or_else(|| panic!("{} stop {pos} rejected", row.key));
                        assert!(
                            (sane.as_f64().unwrap() - v).abs() < 1e-6,
                            "{} stop {pos} survives sanitising",
                            row.key
                        );
                    }
                    if !(row.key == "max_fps" && pos == n - 1) {
                        assert!(v > last, "{} increases", row.key);
                        last = v;
                    }
                }
            }
        }
    }

    #[test]
    fn slider_ranges_match_the_managed_limits() {
        assert_eq!(slider_value("fov", 0), Some(json!(30.)));
        assert_eq!(slider_value("fov", 80), Some(json!(110.)));
        assert_eq!(slider_value("render_distance", 14), Some(json!(16)));
        assert_eq!(slider_value("max_fps", 0), Some(json!(30)));
        assert_eq!(slider_value("max_fps", 23), Some(json!(260)));
        assert_eq!(slider_value("max_fps", 24), Some(json!(0)));
        assert_eq!(slider_value("mouse_sensitivity", 58), Some(json!(3.)));
        assert_eq!(slider_value("volume", 20), Some(json!(1.)));
        assert_eq!(slider_value("ui_scale", 4), Some(json!(4.)));
        assert_eq!(slider_pos("mouse_sensitivity", &json!(1.)), 18);
        assert_eq!(slider_pos("max_fps", &json!(120)), 9);
    }

    #[test]
    fn pointer_x_maps_to_the_track_ends() {
        let b = [100., 0., 290., 22.];
        let (tx, tw) = track_span(b);
        assert_eq!(pos_at("fov", b, tx - 50.), Some(0));
        assert_eq!(pos_at("fov", b, tx + tw + 50.), Some(80));
        let mid = pos_at("fov", b, tx + tw / 2.).unwrap();
        assert!((38..=42).contains(&mid), "{mid}");
    }

    #[test]
    fn value_text_names_special_stops() {
        assert_eq!(value_text("max_fps", Some(&json!(0))), "Unlimited");
        assert_eq!(value_text("ui_scale", Some(&json!(0.))), "Auto");
        assert_eq!(value_text("ui_scale", Some(&json!(3.))), "3x");
        assert_eq!(value_text("graphics_preset", Some(&json!("medium"))), "Medium");
        assert_eq!(value_text("graphics_preset", Some(&json!("low"))), "Low");
        assert_eq!(value_text("graphics_preset", Some(&json!("high"))), "High");
        assert_eq!(value_text("vsync", None), "On");
        assert_eq!(value_text("fullscreen", None), "Off");
        assert_eq!(value_text("narrator", Some(&json!(2))), "Chat");
        assert_eq!(value_text("volume", Some(&json!(0.5))), "50%");
    }

    #[test]
    fn every_option_row_has_a_one_line_description() {
        for tab in 0..TABS.len() {
            for row in rows(tab) {
                assert!(!row.desc.is_empty() && row.desc.len() <= 50, "{}", row.key);
            }
        }
    }
}
