//! Graphics presets: the single switch between the vanilla-exact path and the
//! optional shader design passes.
//!
//! * `Low` is the GT 1030 preset: the display-encoded vanilla pipeline with no
//!   HDR post, shadow, atmosphere, god-ray, water or TAA work at all.
//! * `Medium` is the former "Fancy" path (HDR, bloom, ACES, one 2048 shadow
//!   cascade, reflective/tinted water) plus the analytic atmosphere sky/fog
//!   and cheap half-resolution god rays.
//! * `High` adds three shadow cascades, screen-space water reflection and
//!   refraction, denser god rays and temporal anti-aliasing.

use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphicsPreset {
    Low,
    #[default]
    Medium,
    High,
}

/// Everything the renderer branches on, derived once from the preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Features {
    /// HDR linear scene, ACES + bloom post, shadows, atmosphere sky/fog.
    pub hdr: bool,
    /// Directional shadow cascades (1 = the single 2048 map).
    pub cascades: u32,
    /// Radial god-ray taps per half-resolution pixel (0 = pass skipped).
    pub god_ray_taps: u32,
    /// Screen-space water reflection + refraction through a scene copy.
    pub water_ssr: bool,
    /// Jittered projection, history reprojection and resolve.
    pub taa: bool,
}

/// Shadow cascades never exceed this; `shader.wgsl` mirrors it.
pub(super) const MAX_CASCADES: usize = 3;

/// `shader.wgsl` plus the shared atmosphere functions it calls.
pub(super) fn main_shader_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../shader.wgsl"),
        include_str!("atmosphere.wgsl")
    )
}

const JITTER_SAMPLES: u32 = 8;
/// Largest jitter in pixels. The Hi-Z occlusion test has 0.5 px of slack, so
/// jitter stays below it and culling uses the unjittered matrix.
const JITTER_RADIUS: f32 = 0.45;

fn halton(mut index: u32, base: u32) -> f32 {
    let (mut fraction, mut result) = (1.0, 0.0);
    while index > 0 {
        fraction /= base as f32;
        result += fraction * (index % base) as f32;
        index /= base;
    }
    result
}

/// Sub-pixel projection jitter (Halton 2,3, 8 samples), in pixels.
pub(super) fn jitter_pixels(frame: u32) -> [f32; 2] {
    let index = frame % JITTER_SAMPLES + 1;
    [
        (halton(index, 2) - 0.5) * 2. * JITTER_RADIUS,
        (halton(index, 3) - 0.5) * 2. * JITTER_RADIUS,
    ]
}

/// Previous-frame camera state TAA reprojects with.
pub(super) struct Temporal {
    pub frame: u32,
    /// Unjittered view-projection of the previous frame (camera-relative).
    pub previous_view_projection: glam::Mat4,
    pub previous_camera: [f64; 3],
    /// False until one frame has been resolved with the current resources.
    pub history_valid: bool,
}

impl Default for Temporal {
    fn default() -> Self {
        Self {
            frame: 0,
            previous_view_projection: glam::Mat4::IDENTITY,
            previous_camera: [0.; 3],
            history_valid: false,
        }
    }
}

impl super::Renderer {
    /// Rebuilds every preset-sized resource (cascade array, scene copy, post
    /// chain with its TAA history and god-ray targets) for `self.graphics`.
    pub(super) fn apply_graphics(&mut self) {
        let features = self.graphics.features();
        let (width, height) = (self.config.width, self.config.height);
        self.shadow.configure(
            &self.device,
            features.cascades,
            features.water_ssr,
            width,
            height,
        );
        self.post = super::post::Post::new(
            &self.device,
            width,
            height,
            &self.shadow.opaque_depth_view,
            &features,
        );
        self.temporal = Temporal::default();
    }
}

impl GraphicsPreset {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Option-screen label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// Parses a persisted/automation value (case-insensitive string).
    pub fn from_setting(value: &Value) -> Option<Self> {
        let text = value.as_str()?;
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str().eq_ignore_ascii_case(text))
    }

    /// Next preset in the Options cycle Low -> Medium -> High -> Low.
    pub fn next(self) -> Self {
        match self {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Low,
        }
    }

    pub(super) fn features(self) -> Features {
        match self {
            Self::Low => Features {
                hdr: false,
                cascades: 0,
                god_ray_taps: 0,
                water_ssr: false,
                taa: false,
            },
            Self::Medium => Features {
                hdr: true,
                cascades: 1,
                god_ray_taps: 12,
                water_ssr: false,
                taa: false,
            },
            Self::High => Features {
                hdr: true,
                cascades: MAX_CASCADES as u32,
                god_ray_taps: 24,
                water_ssr: true,
                taa: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn low_has_no_design_passes_and_high_has_all() {
        let low = GraphicsPreset::Low.features();
        assert!(!low.hdr && low.cascades == 0 && low.god_ray_taps == 0);
        assert!(!low.water_ssr && !low.taa);
        let medium = GraphicsPreset::Medium.features();
        assert!(medium.hdr && medium.cascades == 1 && !medium.taa && !medium.water_ssr);
        let high = GraphicsPreset::High.features();
        assert_eq!(high.cascades as usize, MAX_CASCADES);
        assert!(high.water_ssr && high.taa && high.god_ray_taps > medium.god_ray_taps);
    }

    #[test]
    fn setting_round_trips_and_cycles() {
        for preset in GraphicsPreset::ALL {
            assert_eq!(
                GraphicsPreset::from_setting(&json!(preset.as_str())),
                Some(preset)
            );
        }
        assert_eq!(
            GraphicsPreset::from_setting(&json!("HIGH")),
            Some(GraphicsPreset::High)
        );
        assert_eq!(GraphicsPreset::from_setting(&json!("ultra")), None);
        assert_eq!(GraphicsPreset::from_setting(&json!(true)), None);
        assert_eq!(
            GraphicsPreset::Low.next().next().next(),
            GraphicsPreset::Low
        );
        assert_eq!(GraphicsPreset::default(), GraphicsPreset::Medium);
    }
}
