//! Short, demand-driven launcher interactions; no animation loop while idle.

use eframe::egui::{self, Color32, Context, Id, Response, Stroke, StrokeKind, Ui};

const REVEAL_SECONDS: f64 = 0.18;

#[derive(Default)]
pub struct Reveal {
    key: Option<Id>,
    started: f64,
}

impl Reveal {
    pub fn reset(&mut self) {
        self.key = None;
    }

    pub fn sample(&mut self, ctx: &Context, key: Id) -> f32 {
        let now = ctx.input(|input| input.time);
        if self.key != Some(key) {
            self.key = Some(key);
            self.started = now;
        }
        if ctx.style().animation_time == 0.0 {
            self.started = now - REVEAL_SECONDS;
            return 1.0;
        }
        let progress = ((now - self.started) / REVEAL_SECONDS).clamp(0.0, 1.0) as f32;
        if progress < 1.0 {
            ctx.request_repaint();
        }
        ease(progress)
    }
}

fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// egui schedules frames only while this state is between its endpoints.
pub fn state(ctx: &Context, id: Id, target: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, target, ctx.style().animation_time, ease)
}

pub fn mix(a: Color32, b: Color32, amount: f32) -> Color32 {
    let a = a.to_array();
    let b = b.to_array();
    let channel =
        |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * amount).round() as u8;
    Color32::from_rgba_premultiplied(channel(0), channel(1), channel(2), channel(3))
}

/// Keep egui's button semantics, padding, focus, keyboard and disabled behavior.
/// Only the background paint contracts on press; layout/hitboxes never move.
pub fn button(ui: &mut Ui, button: egui::Button<'_>, fill: Color32, hover: Color32) -> Response {
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(button.fill(Color32::TRANSPARENT).stroke(Stroke::NONE));
    let hovered = state(
        ui.ctx(),
        response.id.with("hover"),
        response.enabled() && (response.hovered() || response.has_focus()),
    );
    let pressed = state(
        ui.ctx(),
        response.id.with("press"),
        response.enabled() && response.is_pointer_button_down_on(),
    );
    let focused = state(
        ui.ctx(),
        response.id.with("focus"),
        response.enabled() && response.has_focus(),
    );
    let rect = response.rect.shrink(pressed * 0.75);
    ui.painter().set(
        background,
        egui::Shape::rect_filled(rect, 8, mix(fill, hover, hovered)),
    );
    if focused > 0.0 {
        ui.painter().rect_stroke(
            rect,
            8,
            Stroke::new(
                1.5,
                ui.visuals().selection.stroke.color.gamma_multiply(focused),
            ),
            StrokeKind::Inside,
        );
    }
    response
}

pub fn tab(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let chosen = state(
        ui.ctx(),
        ui.make_persistent_id(label).with("chosen"),
        selected,
    );
    let fill = mix(
        ui.visuals().panel_fill,
        ui.visuals().selection.bg_fill,
        chosen,
    );
    let hover = mix(fill, ui.visuals().widgets.hovered.bg_fill, 0.7);
    let response = button(ui, egui::Button::selectable(selected, label), fill, hover);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            response.enabled(),
            selected,
            label,
        )
    });
    response
}

pub fn secondary(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        button(
            ui,
            egui::Button::new(label),
            ui.visuals().widgets.inactive.bg_fill,
            ui.visuals().widgets.hovered.bg_fill,
        )
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    #[test]
    fn button_hover_is_interpolated_without_moving_hitboxes_or_repeating_clicks() {
        let ctx = egui::Context::default();
        let base = egui::Color32::from_rgb(30, 35, 40);
        let hover = egui::Color32::from_rgb(80, 90, 100);
        let mut rect = egui::Rect::NOTHING;
        let mut clicks = 0;
        let mut fills = Vec::new();
        let mut last = None;
        for frame in 0..80 {
            let mut events = Vec::new();
            if frame == 10 {
                events.push(egui::Event::PointerMoved(rect.center()));
            }
            if frame == 20 || frame == 24 {
                events.push(egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 20,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let output = ctx.run(
                egui::RawInput {
                    time: Some(f64::from(frame) / 120.0),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = button(
                            ui,
                            egui::Button::new("Test action").min_size(egui::vec2(120.0, 32.0)),
                            base,
                            hover,
                        );
                        if frame > 0 {
                            assert_eq!(
                                response.rect, rect,
                                "Paint animation must not move hitboxes"
                            );
                        }
                        rect = response.rect;
                        clicks += usize::from(response.clicked());
                    });
                },
            );
            let fill = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(shape)
                        if shape.rect.center() == rect.center()
                            && shape.fill != egui::Color32::TRANSPARENT =>
                    {
                        Some(shape.fill)
                    }
                    _ => None,
                })
                .expect("Animated button background is painted");
            fills.push(fill);
            last = Some(output);
        }
        assert_eq!(fills[0], base);
        assert!(
            fills[12] != base && fills[12] != hover,
            "Hover must pass through intermediate colors"
        );
        assert_eq!(fills[40], hover);
        assert_eq!(clicks, 1);
        assert!(
            last.unwrap().viewport_output[&egui::ViewportId::ROOT].repaint_delay
                > std::time::Duration::from_millis(100)
        );
    }

    #[test]
    fn reduced_motion_finishes_an_active_reveal_and_does_not_resume_it() {
        let ctx = egui::Context::default();
        let mut reveal = Reveal::default();
        let key = Id::new("dialog");
        let mut progress = 0.0;
        let _ = ctx.run(
            egui::RawInput {
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                assert_eq!(reveal.sample(ctx, key), 0.0);
            },
        );
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let _ = ctx.run(
            egui::RawInput {
                time: Some(0.03),
                ..Default::default()
            },
            |ctx| {
                progress = reveal.sample(ctx, key);
            },
        );
        assert_eq!(
            progress, 1.0,
            "Reduced motion must snap, including mid-transition"
        );
        ctx.all_styles_mut(|style| style.animation_time = 0.12);
        let _ = ctx.run(
            egui::RawInput {
                time: Some(0.04),
                ..Default::default()
            },
            |ctx| {
                assert_eq!(
                    reveal.sample(ctx, key),
                    1.0,
                    "Re-enabling animation must not resurrect an old transition"
                );
            },
        );
    }

    #[test]
    fn page_reveal_eases_to_completion_without_idle_repaints() {
        let ctx = egui::Context::default();
        let mut reveal = Reveal::default();
        let key = egui::Id::new("home");
        let mut values = Vec::new();
        let mut output = None;
        for time in [0.0, 0.04, 0.09, 0.18, 0.4, 0.6] {
            output = Some(ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    values.push(reveal.sample(ctx, key));
                },
            ));
        }
        assert_eq!(values[0], 0.0);
        assert!(values[1] > 0.0 && values[1] < values[2] && values[2] < 1.0);
        assert_eq!(&values[3..], &[1.0, 1.0, 1.0]);
        assert!(
            output.unwrap().viewport_output[&egui::ViewportId::ROOT].repaint_delay
                > std::time::Duration::from_millis(100),
            "A completed reveal must not keep repainting"
        );
    }
}
