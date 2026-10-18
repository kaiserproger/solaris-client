//! Warm translucent UI theme: palette and stepped-corner ("rounded pixel")
//! primitives. Every primitive is built from axis-aligned solid quads on whole
//! GUI pixels, so edges stay crisp at any integer GUI scale and nothing
//! overlaps (translucent fills never double-blend).
use super::render::Painter;

pub(super) const PANEL: [f32; 3] = [0.070, 0.047, 0.032];
pub(super) const CONTROL: [f32; 3] = [0.135, 0.092, 0.060];
pub(super) const CONTROL_HOT: [f32; 3] = [0.250, 0.158, 0.075];
pub(super) const CONTROL_DOWN: [f32; 3] = [0.095, 0.062, 0.040];
pub(super) const RIM: [f32; 3] = [0.470, 0.290, 0.120];
pub(super) const ACCENT: [f32; 3] = [1.000, 0.640, 0.180];
pub(super) const ACCENT_HI: [f32; 3] = [1.000, 0.820, 0.470];
pub(super) const TRACK: [f32; 3] = [0.040, 0.027, 0.018];

/// Body text (ARGB): warm cream.
pub(super) const TEXT: u32 = 0xfffbe9cf;
pub(super) const TEXT_DIM: u32 = 0xffa28866;
pub(super) const TEXT_OFF: u32 = 0xff7a6650;
/// Headings: amber.
pub(super) const HEADING: u32 = 0xffffb84a;

pub(super) fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0., 1.);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// ARGB with the alpha channel scaled by `alpha`.
pub(super) fn with_alpha(argb: u32, alpha: f32) -> u32 {
    let a = ((argb >> 24) as f32 * alpha.clamp(0., 1.)).round() as u32;
    (a << 24) | (argb & 0x00ff_ffff)
}

impl Painter<'_> {
    /// Stepped-corner filled shape with a one-pixel ring (corner radius two).
    /// Fill and ring never overlap.
    pub(super) fn pixel_panel(
        &mut self,
        [x, y, w, h]: [f32; 4],
        fill: [f32; 3],
        fill_alpha: f32,
        rim: [f32; 3],
        rim_alpha: f32,
    ) {
        if w < 6. || h < 6. {
            self.solid([x, y, w, h], fill, fill_alpha);
            return;
        }
        if fill_alpha > 0. {
            self.solid([x + 2., y + 1., w - 4., 1.], fill, fill_alpha);
            self.solid([x + 1., y + 2., w - 2., h - 4.], fill, fill_alpha);
            self.solid([x + 2., y + h - 2., w - 4., 1.], fill, fill_alpha);
        }
        self.pixel_ring([x, y, w, h], rim, rim_alpha);
    }

    /// The one-pixel stepped outline alone.
    pub(super) fn pixel_ring(&mut self, [x, y, w, h]: [f32; 4], color: [f32; 3], alpha: f32) {
        if alpha <= 0. || w < 6. || h < 6. {
            return;
        }
        self.solid([x + 2., y, w - 4., 1.], color, alpha);
        self.solid([x + 2., y + h - 1., w - 4., 1.], color, alpha);
        self.solid([x, y + 2., 1., h - 4.], color, alpha);
        self.solid([x + w - 1., y + 2., 1., h - 4.], color, alpha);
        self.solid([x + 1., y + 1., 1., 1.], color, alpha);
        self.solid([x + w - 2., y + 1., 1., 1.], color, alpha);
        self.solid([x + 1., y + h - 2., 1., 1.], color, alpha);
        self.solid([x + w - 2., y + h - 2., 1., 1.], color, alpha);
    }

    /// Hover glow: two faint stepped rings just outside the control.
    pub(super) fn pixel_glow(&mut self, [x, y, w, h]: [f32; 4], color: [f32; 3], strength: f32) {
        if strength <= 0.01 {
            return;
        }
        self.pixel_ring([x - 1., y - 1., w + 2., h + 2.], color, 0.55 * strength);
        self.pixel_ring([x - 2., y - 2., w + 4., h + 4.], color, 0.20 * strength);
    }

    /// Large themed panel: translucent dark-warm body, amber rim, faint top sheen.
    pub(super) fn themed_panel(&mut self, r: [f32; 4], alpha: f32) {
        self.pixel_panel(r, PANEL, 0.80 * alpha, RIM, 0.95 * alpha);
        self.solid([r[0] + 3., r[1] + 1., r[2] - 6., 1.], ACCENT, 0.18 * alpha);
    }

    /// Standard button body. `hot` and `down` are eased 0..1 factors.
    pub(super) fn themed_button(
        &mut self,
        r: [f32; 4],
        enabled: bool,
        selected: bool,
        hot: f32,
        down: f32,
        alpha: f32,
    ) {
        let base = if selected {
            mix(CONTROL, CONTROL_HOT, 0.6)
        } else {
            CONTROL
        };
        let body = mix(mix(base, CONTROL_HOT, hot), CONTROL_DOWN, down);
        let rim = if !enabled {
            mix(RIM, PANEL, 0.6)
        } else {
            mix(
                RIM,
                ACCENT,
                (hot * 0.85).max(if selected { 1. } else { 0. }),
            )
        };
        self.pixel_glow(
            r,
            ACCENT,
            if enabled { hot * (1. - down * 0.6) } else { 0. },
        );
        self.pixel_panel(r, body, 0.88 * alpha, rim, alpha);
        if enabled {
            self.solid(
                [r[0] + 3., r[1] + 1., r[2] - 6., 1.],
                ACCENT_HI,
                (0.10 + 0.16 * hot) * (1. - down),
            );
        }
        if selected {
            // Accent bar on the left edge marks the active tab.
            self.solid([r[0] + 2., r[1] + 4., 2., r[3] - 8.], ACCENT, 1.);
        }
    }

    /// Switch: pill track + square knob. `t` is the eased knob position 0..1.
    pub(super) fn themed_switch(&mut self, [x, y, w, h]: [f32; 4], t: f32, hot: f32) {
        let track = mix(TRACK, [0.62, 0.36, 0.10], t);
        let rim = mix(mix(RIM, ACCENT, hot * 0.7), ACCENT, t * 0.8);
        self.pixel_glow([x, y, w, h], ACCENT, hot * 0.7);
        self.pixel_panel([x, y, w, h], track, 1., rim, 1.);
        let knob = h - 4.;
        let kx = (x + 2. + (w - 4. - knob) * t).round();
        self.pixel_panel(
            [kx, y + 2., knob, knob],
            mix([0.62, 0.52, 0.42], ACCENT_HI, t),
            1.,
            mix(RIM, ACCENT_HI, t),
            1.,
        );
    }

    /// Slider: recessed track, accent fill up to the (eased) thumb, thumb block.
    pub(super) fn themed_slider(&mut self, [x, y, w, h]: [f32; 4], t: f32, hot: f32, down: f32) {
        let t = t.clamp(0., 1.);
        let ty = (y + (h - 6.) / 2.).floor();
        self.pixel_panel([x, ty, w, 6.], TRACK, 1., mix(RIM, ACCENT, hot * 0.6), 1.);
        let thumb_w = 6.;
        let tx = (x + 1. + (w - 2. - thumb_w) * t).round();
        let fill = tx - x - 1. + thumb_w / 2.;
        if fill > 1. {
            self.solid(
                [x + 1., ty + 1., fill, 4.],
                mix([0.62, 0.36, 0.10], ACCENT, 0.55 + 0.45 * hot),
                1.,
            );
        }
        self.pixel_glow([tx, y, thumb_w, h], ACCENT, hot * (1. - down * 0.5));
        self.pixel_panel(
            [tx, y, thumb_w, h],
            mix(ACCENT, ACCENT_HI, hot + down * 0.5),
            1.,
            mix(RIM, ACCENT_HI, 0.8),
            1.,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_scaling_keeps_color_channels() {
        assert_eq!(with_alpha(0xffffb84a, 0.5), 0x80ffb84a);
        assert_eq!(with_alpha(0xffffb84a, 0.), 0x00ffb84a);
        assert_eq!(with_alpha(0xffffb84a, 3.), 0xffffb84a);
    }

    #[test]
    fn mix_clamps_and_interpolates() {
        assert_eq!(mix([0.; 3], [1.; 3], 0.5), [0.5; 3]);
        assert_eq!(mix([0.; 3], [1.; 3], 7.), [1.; 3]);
    }
}
