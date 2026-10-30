//! Painting of the themed (non-vanilla) controls. Pure presentation: input,
//! actions and semantics stay in the retained node tree.
use super::{
    anim::Fx,
    options::track_span,
    render::Painter,
    state::{Paint, SemanticNode, UiState},
    theme::*,
};
use crate::{api::MeshGeometry, protocol::content::Content, protocol::Result};
use glam::{Mat4, Vec3};
use serde_json::json;

const ROW_RIM: [f32; 3] = [0.30, 0.19, 0.09];

impl UiState {
    /// Lay out `text` and emit it at `(x, y)`; returns its GUI-pixel width.
    pub(super) fn themed_text(
        &self,
        content: &Content,
        out: &mut MeshGeometry,
        transform: Mat4,
        text: &str,
        [x, y]: [f32; 2],
        color: u32,
    ) -> Result<f32> {
        if text.is_empty() {
            return Ok(0.);
        }
        let layout = self.text.layout(&json!({"text":text}), 4096.)?;
        self.text.append_layout(
            content,
            &layout,
            transform * Mat4::from_translation(Vec3::new(x, y, 0.)),
            color,
            None,
            out,
        )?;
        Ok(layout.width)
    }

    fn text_width(&self, text: &str) -> Result<f32> {
        if text.is_empty() {
            return Ok(0.);
        }
        Ok(self.text.layout(&json!({"text":text}), 4096.)?.width)
    }

    /// Paint one themed node (control body plus its own text).
    pub(super) fn paint_widget(
        &self,
        node: &SemanticNode,
        fx: &Fx,
        content: &Content,
        transform: Mat4,
        out: &mut MeshGeometry,
    ) -> Result<()> {
        let [x, y, w, h] = node.bounds;
        let text_y = |layout_h: f32| y + ((h - layout_h) / 2.).floor();
        let mut p = Painter {
            content,
            out,
            transform,
            light: [-1.; 2],
            render_layer: crate::api::RenderLayer::Translucent,
        };
        match &node.paint {
            Paint::Frame => {
                p.themed_panel(node.bounds, 1.);
                if node.id == "options/panel" {
                    p.solid([x + 96., y + 24., 1., h - 48.], RIM, 0.55);
                    let [fx0, fy0, fw, _] = self.options_footer;
                    p.solid([fx0, fy0 - 2., fw, 1.], RIM, 0.55);
                }
            }
            Paint::Tab(selected) => {
                let lift = if node.enabled { fx.hover } else { 0. };
                p.themed_button(node.bounds, node.enabled, *selected, lift, fx.press, 1.);
                let color = if *selected {
                    0xffffd98a
                } else if lift > 0.4 {
                    0xffffe2a8
                } else {
                    TEXT
                };
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    &node.label,
                    [x + 14., text_y(9.)],
                    color,
                )?;
            }
            Paint::Slider { t, .. } => {
                self.row_body(&mut p, node, fx);
                let (label, value) = node.label.split_once(": ").unwrap_or((&node.label, ""));
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    label,
                    [x + 8., text_y(9.)],
                    TEXT,
                )?;
                let shown = self.anim_value(&node.id, *t);
                let (tx, tw) = track_span(node.bounds);
                p.themed_slider([tx, y + 3., tw, h - 6.], shown, fx.hover, fx.press);
                let vw = self.text_width(value)?;
                let color = if fx.hover > 0.4 { 0xffffd98a } else { TEXT_DIM };
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    value,
                    [x + w - 8. - vw, text_y(9.)],
                    color,
                )?;
            }
            Paint::Toggle { on, .. } => {
                self.row_body(&mut p, node, fx);
                let (label, value) = node.label.split_once(": ").unwrap_or((&node.label, ""));
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    label,
                    [x + 8., text_y(9.)],
                    TEXT,
                )?;
                let knob = self.anim_value(&node.id, f32::from(u8::from(*on)));
                let sw = [x + w - 8. - 28., y + ((h - 12.) / 2.).floor(), 28., 12.];
                p.themed_switch(sw, knob, fx.hover);
                let vw = self.text_width(value)?;
                let color = if *on { 0xffffc063 } else { TEXT_OFF };
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    value,
                    [sw[0] - 8. - vw, text_y(9.)],
                    color,
                )?;
            }
            Paint::Split { delim, conflict, .. } => {
                self.row_body(&mut p, node, fx);
                let (left, right) = node.label.split_once(*delim).unwrap_or((&node.label, ""));
                let rw = self.text_width(right)?;
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    left,
                    [x + 8., text_y(9.)],
                    TEXT,
                )?;
                let color = if *conflict { 0xffff5555 } else if fx.hover > 0.4 { 0xffffd98a } else { TEXT_DIM };
                self.themed_text(
                    content,
                    p.out,
                    transform,
                    right,
                    [x + w - 8. - rw, text_y(9.)],
                    color,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Recessed row background with hover glow.
    fn row_body(&self, p: &mut Painter, node: &SemanticNode, fx: &Fx) {
        let hot = if node.focused {
            fx.hover.max(0.7)
        } else {
            fx.hover
        };
        p.pixel_glow(node.bounds, ACCENT, hot * (1. - fx.press * 0.5));
        p.pixel_panel(
            node.bounds,
            mix(CONTROL, CONTROL_HOT, hot * 0.8),
            0.62,
            mix(ROW_RIM, ACCENT, hot * 0.75),
            0.95,
        );
    }

    /// One-line description in the options footer while a row is hovered or
    /// focused; concurrent fades cross-dissolve in place.
    pub(super) fn footer_text(
        &self,
        p: &mut Painter,
        node: &SemanticNode,
        fx: &Fx,
        desc: &str,
        content: &Content,
        transform: Mat4,
    ) -> Result<()> {
        let strength = fx.hover.max(if node.focused { 1. } else { 0. });
        if strength <= 0.02 || desc.is_empty() || !self.is_options_screen() || self.options_footer[2] <= 0. {
            return Ok(());
        }
        let [fx0, fy0, fw, fh] = self.options_footer;
        let layout = self.text.layout(&json!({"text":desc}), fw.max(1.))?;
        self.text.append_layout(
            content,
            &layout,
            transform
                * Mat4::from_translation(Vec3::new(
                    fx0 + 4.,
                    fy0 + ((fh - layout.height) / 2.).floor(),
                    0.,
                )),
            with_alpha(0xffffd9a0, strength),
            None,
            p.out,
        )
    }

    /// Screen dimmer. Shell screens get the deeper warm gradient + vignette
    /// over the live scene; in-game screens a lighter veil.
    pub(super) fn paint_backdrop(&self, p: &mut Painter, [x, y, w, h]: [f32; 4], shell: bool) {
        let bands: usize = if shell { 40 } else { 20 };
        let band_h = (h / bands as f32).ceil();
        for i in 0..bands {
            let s = i as f32 / (bands - 1) as f32;
            let top = y + i as f32 * band_h;
            let bh = band_h.min(y + h - top);
            if bh <= 0. {
                break;
            }
            let (color, alpha) = if shell {
                (mix([0.030, 0.018, 0.012], [0.30, 0.12, 0.03], s * s), 0.60 + 0.16 * s)
            } else {
                (mix([0.030, 0.018, 0.012], [0.12, 0.05, 0.02], s * s), 0.46 + 0.20 * s)
            };
            p.solid([x, top, w, bh], color, alpha);
        }
        if shell {
            // Soft vignette: eight 6px strips per side, quadratic falloff.
            for i in 0..8 {
                let a = 0.32 * (1. - i as f32 / 8.).powi(2);
                p.solid([x + i as f32 * 6., y, 6., h], [0.; 3], a);
                p.solid([x + w - (i as f32 + 1.) * 6., y, 6., h], [0.; 3], a);
            }
        }
    }
}
