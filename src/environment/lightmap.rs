//! Dynamic lighting remains independent of baked tint/AO/directional shading.
//! Source: verified target LightmapRenderStateExtractor.javap and lightmap.fsh.
use crate::api::LightmapTexture;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightmapInputs {
    pub sky_factor: f32,
    pub block_factor: f32,
    pub night_vision: f32,
    pub darkness: f32,
    pub boss_darkening: f32,
    pub brightness: f32,
    pub block_tint: [f32; 3],
    pub sky_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub night_vision_color: [f32; 3],
}
impl LightmapInputs {
    /// Byte rows are sky 0..15; columns are block 0..15, RGBA8Unorm.
    pub fn generate(self) -> [u8; 1024] {
        let mut out = [0; 1024];
        for sky in 0..16 {
            for block in 0..16 {
                let b = block as f32 / 15.;
                let s = sky as f32 / 15.;
                let b = b / (4. - 3. * b) * self.block_factor;
                let s = s / (4. - 3. * s) * self.sky_factor;
                let tint_mix = 0.9 * (2. * block as f32 / 15. - 1.).powi(2);
                let mut color = [0.; 3];
                for (i, c) in color.iter_mut().enumerate() {
                    let tint = self.block_tint[i] + tint_mix * (1. - self.block_tint[i]);
                    *c = self.ambient_color[i].max(self.night_vision_color[i] * self.night_vision)
                        + self.sky_color[i] * s
                        + tint * b;
                    *c *= 1. + self.boss_darkening * ([0.7, 0.6, 0.6][i] - 1.);
                    *c = (*c - self.darkness).clamp(0., 1.);
                }
                let max = color[0].max(color[1]).max(color[2]);
                // The target shader's all-black 0/0 is treated as zero at UNORM output.
                let gamma_scale = if max > 0. {
                    (1. - (1. - max).powi(4)) / max
                } else {
                    0.
                };
                let offset = (sky * 16 + block) * 4;
                for i in 0..3 {
                    let c = color[i] + self.brightness * (color[i] * gamma_scale - color[i]);
                    out[offset + i] = (c.clamp(0., 1.) * 255.).round() as u8;
                }
                out[offset + 3] = 255;
            }
        }
        out
    }
}
#[derive(Default)]
pub(super) struct LightmapCache {
    texture: Option<LightmapTexture>,
    last_inputs: Option<LightmapInputs>,
    revision: u64,
}
impl LightmapCache {
    pub fn update(&mut self, inputs: LightmapInputs) {
        if self.last_inputs == Some(inputs) {
            return;
        }
        self.last_inputs = Some(inputs);
        let pixels = inputs.generate();
        if self
            .texture
            .as_ref()
            .is_some_and(|v| v.rgba.as_ref() == &pixels)
        {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.texture = Some(LightmapTexture {
            revision: self.revision,
            rgba: Arc::new(pixels),
        });
    }
    pub fn texture(&self) -> Option<LightmapTexture> {
        self.texture.clone()
    }
    pub fn inputs(&self) -> Option<LightmapInputs> {
        self.last_inputs
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Random {
    seed: u64,
}
impl Random {
    pub fn new(seed: u64) -> Self {
        Self {
            seed: (seed ^ 25214903917) & ((1 << 48) - 1),
        }
    }
    fn bits(&mut self, bits: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(25214903917).wrapping_add(11) & ((1 << 48) - 1);
        (self.seed >> (48 - bits)) as u32
    }
    pub fn float(&mut self) -> f32 {
        self.bits(24) as f32 * 5.9604645e-8
    }
    pub fn int(&mut self, bound: i32) -> i32 {
        if (bound as u32).is_power_of_two() {
            return ((bound as i64 * self.bits(31) as i64) >> 31) as i32;
        }
        loop {
            let n = self.bits(31) as i32;
            let v = n % bound;
            if n.wrapping_sub(v).wrapping_add(bound - 1) >= 0 {
                return v;
            }
        }
    }
}
/// Target Mth indexes its 65536-entry sin table, rather than evaluating each
/// incoming angle with an arbitrary graphics fast-math implementation.
pub(super) fn sin(angle: f64) -> f32 {
    let index = ((angle * 10430.378350470453) as i64 & 65535) as f64;
    (index * std::f64::consts::TAU / 65536.).sin() as f32
}
pub(super) fn cos(angle: f64) -> f32 {
    let index = ((angle * 10430.378350470453 + 16384.) as i64 & 65535) as f64;
    (index * std::f64::consts::TAU / 65536.).sin() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> LightmapInputs {
        LightmapInputs {
            sky_factor: 1.,
            block_factor: 1.4,
            night_vision: 0.,
            darkness: 0.,
            boss_darkening: 0.,
            brightness: 0.,
            block_tint: [1., 0.85, 0.55],
            sky_color: [1.; 3],
            ambient_color: [0.; 3],
            night_vision_color: [0.6; 3],
        }
    }
    #[test]
    fn separate_block_and_sky_channels_and_effects() {
        let i = input();
        let p = i.generate();
        assert_eq!(&p[0..4], &[0, 0, 0, 255]);
        let block = &p[7 * 4..7 * 4 + 3];
        let sky = &p[7 * 16 * 4..7 * 16 * 4 + 3];
        assert!(block[0] > block[2]);
        assert_eq!(sky[0], sky[2]);
        let mut night = i;
        night.night_vision = 1.;
        assert_eq!(&night.generate()[..4], &[153, 153, 153, 255]);
        night.darkness = 0.6;
        assert_eq!(&night.generate()[..4], &[0, 0, 0, 255]);
    }
    #[test]
    fn brightness_zero_black_boundary_and_revision_dedup() {
        let mut i = input();
        i.brightness = 1.;
        assert_eq!(&i.generate()[..4], &[0, 0, 0, 255]);
        let mut c = LightmapCache::default();
        c.update(i);
        let r = c.texture().unwrap().revision;
        c.update(i);
        assert_eq!(c.texture().unwrap().revision, r);
        i.night_vision = 1.;
        c.update(i);
        assert_eq!(c.texture().unwrap().revision, r + 1);
    }
}
