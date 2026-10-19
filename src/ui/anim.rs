//! Frame-clock UI animation. Nothing here sleeps or polls: the session feeds
//! the monotonic frame time to `UiState::advance_animations`, which eases
//! per-node hover/press/value factors and reports whether the retained overlay
//! must be re-emitted. At rest (nothing hovered, nothing moving) it reports
//! `false` and the overlay geometry is left untouched, so an idle menu costs
//! only the normal frame.
use super::state::{NodeAction, Paint, Screen, SemanticNode, UiState};
use std::collections::{HashMap, HashSet};

/// Panel/row enter duration (ease-out cubic).
pub(crate) const OPEN_SECS: f64 = 0.20;
/// Leave duration of the previous screen.
pub(crate) const EXIT_SECS: f64 = 0.12;
/// Per-item enter stagger and its cap.
const STAGGER_SECS: f64 = 0.028;
const STAGGER_CAP: f64 = 0.22;
/// Exponential approach rates (1/s): ~95% of the way in 3/rate seconds.
const HOVER_RATE: f32 = 16.;
const PRESS_RATE: f32 = 34.;
const VALUE_RATE: f32 = 20.;
/// Hotbar selection slide duration.
const HOTBAR_SECS: f64 = 0.11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    /// HUD, chat, loading and editors: never animated.
    None,
    /// Shell/overlay menus: every node enters with a stagger.
    Stagger,
    /// Container screens: the whole panel enters together.
    Panel,
}

impl Screen {
    pub(crate) fn profile(self) -> Profile {
        match self {
            Screen::Title
            | Screen::Multiplayer
            | Screen::DirectConnect
            | Screen::Singleplayer
            | Screen::CreateWorld
            | Screen::DeleteWorld
            | Screen::SingleplayerStarting
            | Screen::Disconnected
            | Screen::Pause
            | Screen::Settings
            | Screen::Accessibility
            | Screen::Death
            | Screen::Advancements
            | Screen::Statistics => Profile::Stagger,
            Screen::Menu | Screen::Creative => Profile::Panel,
            _ => Profile::None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NodeAnim {
    /// Frame-clock time at which this node starts entering (stagger included).
    pub born: f64,
    pub hover: f32,
    pub press: f32,
    /// Eased slider position / switch knob; NaN until first target is known.
    pub value: f32,
}

pub(crate) struct Exit {
    pub nodes: Vec<SemanticNode>,
    pub started: f64,
}

#[derive(Default)]
pub(crate) struct UiAnim {
    pub now: f64,
    /// False until the session feeds a frame clock: tests and tools that call
    /// `render` directly then see the settled, un-offset UI.
    pub running: bool,
    last: f64,
    was_active: bool,
    pub nodes: HashMap<String, NodeAnim>,
    pub screen_started: f64,
    pub exit: Option<Exit>,
    pub hovered: Option<String>,
    /// Hotbar selection slide: (slot position it slides from, start time).
    hotbar: Option<(f32, f64)>,
    /// Bumped by every node-tree rebuild; entries are registered once per epoch.
    pub nodes_epoch: u64,
    registered_epoch: u64,
}

/// Per-node presentation factors for one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fx {
    pub alpha: f32,
    pub dx: f32,
    pub dy: f32,
    pub hover: f32,
    pub press: f32,
}
impl Fx {
    pub(crate) const REST: Fx = Fx {
        alpha: 1.,
        dx: 0.,
        dy: 0.,
        hover: 0.,
        press: 0.,
    };
}

pub(crate) fn ease_out_cubic(t: f64) -> f64 {
    f64::from(crate::motion::ease_out_cubic(t as f32))
}

/// Frame-rate independent exponential approach, snapping once visually done.
pub(crate) fn approach(value: f32, target: f32, dt: f64, rate: f32) -> f32 {
    let k = 1. - (-(rate as f64) * dt).exp() as f32;
    let next = value + (target - value) * k;
    if (target - next).abs() < 0.004 {
        target
    } else {
        next
    }
}

/// Paints that are safe to draw after their screen has been replaced: they read
/// nothing from the live inventory/session.
pub(crate) fn exit_safe(paint: &Paint) -> bool {
    matches!(
        paint,
        Paint::Text
            | Paint::RichText(_)
            | Paint::OutlinedText(..)
            | Paint::Button
            | Paint::Tab(_)
            | Paint::Slider { .. }
            | Paint::Toggle { .. }
            | Paint::Split { .. }
            | Paint::Frame
            | Paint::Solid(..)
            | Paint::Image(..)
            | Paint::TintedImage(..)
    )
}

impl UiState {
    /// True while any presentation animation is moving (or just settled and
    /// still needs its final frame).
    pub fn animating(&self) -> bool {
        self.anim.was_active
    }

    /// Advance the animation clock to `now` (seconds, monotonic). Returns
    /// whether the retained overlay must be re-emitted this frame.
    pub fn advance_animations(&mut self, now: f64) -> bool {
        let dt = if self.anim.running {
            (now - self.anim.last).clamp(0., 0.1)
        } else {
            0.
        };
        self.anim.running = true;
        self.anim.last = now;
        self.anim.now = now;
        self.anim_register_nodes();
        let profile = self.screen.profile();
        let mut active = false;
        if let Some(exit) = &self.anim.exit {
            if now - exit.started >= EXIT_SECS {
                self.anim.exit = None;
            }
            active = true;
        }
        if let Some((_, started)) = self.anim.hotbar {
            if now - started >= HOTBAR_SECS {
                self.anim.hotbar = None;
            }
            active = true;
        }
        if profile == Profile::None && self.anim.nodes.is_empty() {
            let report = active || self.anim.was_active;
            self.anim.was_active = active;
            return report;
        }
        let hovered = if profile == Profile::None {
            None
        } else {
            self.nodes
                .iter()
                .rev()
                .find(|n| {
                    n.visible
                        && !matches!(n.action, NodeAction::None)
                        && super::state::contains(n.bounds, self.pointer)
                })
                .filter(|n| n.enabled)
                .map(|n| n.id.clone())
        };
        // A dragged slider keeps its hover/press look while the pointer is off the row.
        let hovered = self
            .slider_drag
            .clone()
            .filter(|id| self.nodes.iter().any(|n| &n.id == id))
            .or(hovered);
        if self.anim.hovered != hovered {
            active = true;
        }
        if let Some(id) = &hovered {
            self.anim.nodes.entry(id.clone()).or_insert(NodeAnim {
                born: f64::NEG_INFINITY,
                hover: 0.,
                press: 0.,
                value: f32::NAN,
            });
        }
        let down = self.drag.is_some() || self.slider_drag.is_some();
        for node in &self.nodes {
            let target = match node.paint {
                Paint::Slider { t, .. } => t,
                Paint::Toggle { on, .. } => f32::from(u8::from(on)),
                _ => continue,
            };
            let Some(entry) = self.anim.nodes.get_mut(&node.id) else {
                continue;
            };
            if entry.value.is_nan() {
                entry.value = target;
            } else {
                entry.value = approach(entry.value, target, dt, VALUE_RATE);
                if entry.value != target {
                    active = true;
                }
            }
        }
        let started = self.anim.screen_started;
        for (id, entry) in self.anim.nodes.iter_mut() {
            let hot = hovered.as_deref() == Some(id.as_str());
            let hover_target = f32::from(u8::from(hot));
            let press_target = f32::from(u8::from(hot && down));
            entry.hover = approach(entry.hover, hover_target, dt, HOVER_RATE);
            entry.press = approach(entry.press, press_target, dt, PRESS_RATE);
            if entry.hover != hover_target || entry.press != press_target {
                active = true;
            }
            let born = if profile == Profile::Panel {
                started
            } else {
                entry.born
            };
            if now < born + OPEN_SECS {
                active = true;
            }
        }
        if profile == Profile::Panel && now < started + OPEN_SECS {
            active = true;
        }
        self.anim.hovered = hovered;
        let report = active || self.anim.was_active;
        self.anim.was_active = active;
        report
    }

    /// Called by `transition` before the screen changes: snapshot the outgoing
    /// screen so it can fade out under the incoming one.
    pub(crate) fn anim_screen_changed(&mut self, old: Screen) {
        if !self.anim.running {
            return;
        }
        self.anim.exit = (old.profile() != Profile::None && !self.nodes.is_empty()).then(|| Exit {
            nodes: self
                .nodes
                .iter()
                .filter(|n| n.visible && exit_safe(&n.paint))
                .cloned()
                .collect(),
            started: self.anim.now,
        });
        self.anim.nodes.clear();
        self.anim.hovered = None;
        self.anim.screen_started = self.anim.now;
        self.anim.was_active = true;
    }

    /// Called at the start of every overlay emit: give unseen nodes of
    /// staggered screens an entry time and drop entries of vanished nodes
    /// (so a returning tab/row enters again).
    pub(crate) fn anim_register_nodes(&mut self) {
        if !self.anim.running || self.anim.registered_epoch == self.anim.nodes_epoch {
            return;
        }
        self.anim.registered_epoch = self.anim.nodes_epoch;
        let ids: HashSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        self.anim.nodes.retain(|id, _| ids.contains(id.as_str()));
        if self.screen.profile() != Profile::Stagger {
            return;
        }
        let mut fresh = 0u32;
        let now = self.anim.now;
        for node in &self.nodes {
            if !node.visible || self.anim.nodes.contains_key(&node.id) {
                continue;
            }
            let delay = if node.id == "screen/background" {
                0.
            } else {
                (f64::from(fresh) * STAGGER_SECS).min(STAGGER_CAP)
            };
            if node.id != "screen/background" {
                fresh += 1;
            }
            self.anim.nodes.insert(
                node.id.clone(),
                NodeAnim {
                    born: now + delay,
                    hover: 0.,
                    press: 0.,
                    value: f32::NAN,
                },
            );
        }
    }

    /// The hotbar selection moved from `old` to the current slot: slide the
    /// selection frame instead of jumping (wrap-arounds still jump).
    pub(crate) fn anim_hotbar_changed(&mut self, old: u8) {
        if !self.anim.running || self.reduce_motion() {
            return;
        }
        let old = f32::from(old);
        let displayed = match self.anim.hotbar {
            Some((from, started)) => {
                let p = ease_out_cubic((self.anim.now - started) / HOTBAR_SECS) as f32;
                old + (from - old) * (1. - p)
            }
            None => old,
        };
        let target = f32::from(self.selected_slot);
        self.anim.hotbar = ((displayed - target).abs() <= 4.5).then_some((displayed, self.anim.now));
    }

    /// Remaining slide distance (in slots) of the hotbar selection frame.
    fn hotbar_slide_offset(&self) -> f32 {
        match self.anim.hotbar {
            Some((from, started)) => {
                let p = ease_out_cubic((self.anim.now - started) / HOTBAR_SECS) as f32;
                (from - f32::from(self.selected_slot)) * (1. - p)
            }
            None => 0.,
        }
    }

    fn reduce_motion(&self) -> bool {
        self.settings
            .get("reduce_motion")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    }

    /// Enter-phase factors for a live node.
    pub(crate) fn fx_enter(&self, node: &SemanticNode) -> Fx {
        let entry = self.anim.nodes.get(&node.id);
        let (hover, press) = entry.map_or((0., 0.), |e| (e.hover, e.press));
        if node.id == "hud/selection" {
            return Fx {
                dx: self.hotbar_slide_offset() * 20.,
                ..Fx::REST
            };
        }
        if !self.anim.running {
            return Fx {
                hover,
                press,
                ..Fx::REST
            };
        }
        let born = match self.screen.profile() {
            Profile::None => {
                return Fx {
                    hover,
                    press,
                    ..Fx::REST
                }
            }
            Profile::Panel => self.anim.screen_started,
            Profile::Stagger => match entry {
                Some(e) if e.born.is_finite() => e.born,
                // Registered at the start of this emit; absent means invisible.
                _ => self.anim.now,
            },
        };
        let p = ease_out_cubic((self.anim.now - born) / OPEN_SECS) as f32;
        let background = node.id == "screen/background";
        let rows = node.id.starts_with("settings/") || node.id.starts_with("options/tab");
        let (dx, dy) = if self.reduce_motion() || background {
            (0., 0.)
        } else if rows && node.id != "settings/done" {
            ((1. - p) * 14., 0.)
        } else {
            (0., (1. - p) * 8.)
        };
        Fx {
            alpha: p,
            dx,
            dy,
            hover,
            press,
        }
    }

    /// Leave-phase factors for a node of the outgoing screen.
    pub(crate) fn fx_exit(&self, node: &SemanticNode, started: f64) -> Fx {
        let p = ((self.anim.now - started) / EXIT_SECS).clamp(0., 1.) as f32;
        let q = p * p;
        Fx {
            alpha: 1. - q,
            dx: 0.,
            dy: if self.reduce_motion() || node.id == "screen/background" {
                0.
            } else {
                q * 6.
            },
            hover: 0.,
            press: 0.,
        }
    }

    /// Eased slider/knob position for a node, falling back to its target.
    pub(crate) fn anim_value(&self, id: &str, target: f32) -> f32 {
        self.anim
            .nodes
            .get(id)
            .map(|e| e.value)
            .filter(|v| !v.is_nan())
            .unwrap_or(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_is_monotonic_and_bounded() {
        let mut last = 0.;
        for i in 0..=20 {
            let v = ease_out_cubic(f64::from(i) / 20.);
            assert!(v >= last && (0. ..=1.).contains(&v));
            last = v;
        }
        assert_eq!(ease_out_cubic(2.), 1.);
        assert_eq!(ease_out_cubic(-1.), 0.);
    }

    #[test]
    fn approach_converges_in_a_quarter_second_and_is_frame_rate_independent() {
        let step = |dt: f64, frames: u32| {
            let mut v = 0.;
            for _ in 0..frames {
                v = approach(v, 1., dt, HOVER_RATE);
            }
            v
        };
        let coarse = step(1. / 30., 8);
        let fine = step(1. / 240., 64);
        assert!(coarse > 0.9, "reaches the target quickly: {coarse}");
        assert!((coarse - fine).abs() < 0.03, "{coarse} vs {fine}");
        assert_eq!(approach(0.999, 1., 0.016, HOVER_RATE), 1.);
        assert!(
            approach(0., 1., 0., HOVER_RATE) == 0.,
            "zero dt does not move"
        );
    }

    use crate::{inventory::InventoryStore, protocol::content::Content};

    fn state() -> (Content, InventoryStore, UiState) {
        let mut content = Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        let mut ui = UiState::new_state(&content).unwrap();
        ui.resize(427, 240, 1.0);
        (content, InventoryStore::default(), ui)
    }

    #[test]
    fn idle_screens_report_no_animation_and_never_request_a_redraw() {
        let (content, inventory, mut ui) = state();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        for frame in 0..30 {
            assert!(!ui.advance_animations(f64::from(frame) / 60.), "HUD frame {frame}");
        }
        ui.transition(Screen::Title);
        ui.invalidate();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        // Enter animation runs ...
        assert!(ui.advance_animations(10.));
        assert!(ui.advance_animations(10.1));
        // ... then settles, reports exactly one final frame and goes quiet.
        let mut quiet = 0;
        for frame in 0..60 {
            if ui.advance_animations(11. + f64::from(frame) / 60.) {
                assert!(quiet == 0, "no activity after settling");
            } else {
                quiet += 1;
            }
        }
        assert!(quiet >= 55, "settled: {quiet}");
        assert!(!ui.animating());
    }

    #[test]
    fn hover_eases_in_and_out_and_only_the_hovered_control_moves() {
        let (content, inventory, mut ui) = state();
        ui.transition(Screen::Title);
        ui.invalidate();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.advance_animations(0.);
        ui.advance_animations(1.); // enter finished
        let quit = ui.nodes.iter().find(|n| n.id == "title/quit").unwrap().bounds;
        ui.pointer = [quit[0] + quit[2] / 2., quit[1] + quit[3] / 2.];
        let mut last = 0.;
        let mut now = 1.;
        for _ in 0..4 {
            now += 1. / 60.;
            assert!(ui.advance_animations(now));
            let h = ui.anim.nodes["title/quit"].hover;
            assert!(h > last && h < 1., "eases monotonically: {last} -> {h}");
            last = h;
        }
        for _ in 0..30 {
            now += 1. / 60.;
            ui.advance_animations(now);
        }
        assert_eq!(ui.anim.nodes["title/quit"].hover, 1.);
        assert_eq!(ui.anim.nodes["title/options"].hover, 0.);
        let fx = ui.fx_enter(ui.nodes.iter().find(|n| n.id == "title/quit").unwrap());
        assert_eq!((fx.alpha, fx.hover), (1., 1.));
        // Pointer leaves: eases back to rest.
        ui.pointer = [1., 1.];
        for _ in 0..40 {
            now += 1. / 60.;
            ui.advance_animations(now);
        }
        assert_eq!(ui.anim.nodes["title/quit"].hover, 0.);
    }

    #[test]
    fn only_enabled_controls_glow() {
        let (content, inventory, mut ui) = state();
        ui.can_respawn = false;
        ui.death = Some(serde_json::json!({"text":"fell"}));
        ui.transition(Screen::Death);
        ui.invalidate();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.advance_animations(0.);
        let respawn = ui.nodes.iter().find(|n| n.id == "death/respawn").unwrap().bounds;
        let title = ui.nodes.iter().find(|n| n.id == "death/disconnect").unwrap().bounds;
        ui.pointer = [respawn[0] + 4., respawn[1] + 4.];
        for frame in 1..30 {
            ui.advance_animations(f64::from(frame) / 60.);
        }
        assert!(!ui.nodes.iter().find(|n| n.id == "death/respawn").unwrap().enabled);
        assert_eq!(ui.anim.nodes["death/respawn"].hover, 0., "disabled control stays flat");
        ui.pointer = [title[0] + 4., title[1] + 4.];
        for frame in 30..60 {
            ui.advance_animations(f64::from(frame) / 60.);
        }
        assert_eq!(ui.anim.nodes["death/disconnect"].hover, 1., "enabled control glows");
    }

    #[test]
    fn entering_a_screen_staggers_nodes_and_leaving_keeps_the_old_one_fading() {
        let (content, inventory, mut ui) = state();
        ui.transition(Screen::Pause);
        ui.invalidate();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.advance_animations(5.);
        let born = |id: &str| ui.anim.nodes[id].born;
        assert!(born("pause/resume") < born("pause/settings"));
        assert!(born("pause/settings") < born("pause/disconnect"));
        assert!(born("pause/disconnect") - 5. <= 0.25, "stagger is capped");
        let resume = ui.nodes.iter().find(|n| n.id == "pause/resume").unwrap().clone();
        assert_eq!(ui.fx_enter(&resume).alpha, 0., "starts transparent");
        ui.advance_animations(5.1);
        let mid = ui.fx_enter(&resume);
        assert!(mid.alpha > 0. && mid.alpha < 1. && mid.dy > 0., "{mid:?}");
        ui.advance_animations(5.5);
        let done = ui.fx_enter(&resume);
        assert_eq!((done.alpha, done.dx, done.dy), (1., 0., 0.));
        // Leaving snapshots the pause nodes for a short fade-out.
        ui.transition(Screen::Game);
        ui.invalidate();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert!(ui.anim.exit.as_ref().is_some_and(|e| !e.nodes.is_empty()));
        assert!(ui.advance_animations(5.55));
        let started = ui.anim.exit.as_ref().unwrap().started;
        let mid = ui.fx_exit(&resume, started);
        assert!(mid.alpha < 1.);
        ui.advance_animations(5.5 + EXIT_SECS + 0.1);
        assert!(ui.anim.exit.is_none());
    }

    #[test]
    fn reduced_motion_removes_the_slide_but_keeps_the_fade() {
        let (content, inventory, mut ui) = state();
        ui.settings.insert("reduce_motion".into(), serde_json::json!(true));
        ui.transition(Screen::Pause);
        ui.invalidate();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.advance_animations(1.);
        ui.advance_animations(1.1);
        let resume = ui.nodes.iter().find(|n| n.id == "pause/resume").unwrap().clone();
        let fx = ui.fx_enter(&resume);
        assert!(fx.alpha > 0. && fx.alpha < 1.);
        assert_eq!((fx.dx, fx.dy), (0., 0.));
    }

    #[test]
    fn slider_thumb_and_switch_knob_ease_toward_new_targets() {
        let (content, mut inventory, mut ui) = state();
        ui.transition(Screen::Title);
        ui.activate_for_test(NodeAction::Screen(Screen::Settings), &mut inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.advance_animations(0.);
        ui.advance_animations(1.);
        let first = ui.anim_value("settings/vsync", 1.);
        assert_eq!(first, 1., "first sighting starts at the target");
        ui.activate_for_test(NodeAction::Setting("vsync"), &mut inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let mut now = 1.;
        let mut last = 1.;
        for _ in 0..3 {
            now += 1. / 60.;
            assert!(ui.advance_animations(now));
            let v = ui.anim_value("settings/vsync", 0.);
            assert!(v < last && v > 0., "knob slides: {last} -> {v}");
            last = v;
        }
        for _ in 0..40 {
            now += 1. / 60.;
            ui.advance_animations(now);
        }
        assert_eq!(ui.anim_value("settings/vsync", 0.), 0.);
    }

    #[test]
    fn hotbar_selection_slides_then_rests_without_requesting_frames() {
        let (content, mut inventory, mut ui) = state();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        ui.advance_animations(0.);
        inventory.selected_slot = 3;
        ui.synchronize(&inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let sel = ui.nodes.iter().find(|n| n.id == "hud/selection").unwrap().clone();
        assert!(ui.advance_animations(0.01));
        let fx = ui.fx_enter(&sel);
        assert!(fx.dx < 0. && fx.dx > -60., "starts left of slot 3: {}", fx.dx);
        assert!(ui.advance_animations(0.2));
        assert_eq!(ui.fx_enter(&sel).dx, 0.);
        ui.advance_animations(0.3); // final settled frame
        assert!(!ui.advance_animations(0.4));
        // Wrap-around jumps instead of sweeping the whole bar.
        inventory.selected_slot = 8;
        ui.synchronize(&inventory, &content).unwrap();
        inventory.selected_slot = 0;
        ui.synchronize(&inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let sel = ui.nodes.iter().find(|n| n.id == "hud/selection").unwrap().clone();
        assert_eq!(ui.fx_enter(&sel).dx, 0.);
    }
}
