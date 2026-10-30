//! The HUD darkness vignette: the screen edges darken while the camera's own
//! block is dark, easing in and out over a few seconds of game ticks.
//!
//! The easing rate and the darkness target were fitted against the vanilla
//! oracle (night/day calibration scenes shot by both clients at the same pose:
//! the screen-edge ratio follows `1 - strength * vignette texel`), not taken
//! from game code. The texture itself is the game's `misc/vignette`, loaded at
//! runtime like every other GUI sprite.
use super::lightmap::level_brightness;

/// Share of the remaining gap to the target closed on every game tick.
const EASE_PER_TICK: f32 = 0.01;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Vignette {
    strength: f32,
}

impl Vignette {
    /// 0 draws nothing; 1 darkens the screen by the full texel value.
    pub(crate) fn strength(self) -> f32 {
        self.strength
    }

    /// One game tick. `light` is the camera block's light after sky
    /// darkening (0..=15); `ambient` is the dimension's ambient light.
    pub(crate) fn tick(&mut self, light: u8, ambient: f32) {
        let brightness = level_brightness(f32::from(light.min(15)) / 15.);
        let brightness = brightness + (1. - brightness) * ambient.clamp(0., 1.);
        let target = 1. - brightness;
        self.strength += (target - self.strength) * EASE_PER_TICK;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darkness_eases_in_over_seconds_and_light_fades_it_out() {
        let mut vignette = Vignette::default();
        assert_eq!(vignette.strength(), 0.);
        vignette.tick(0, 0.);
        assert!((vignette.strength() - 0.01).abs() < 1e-6);
        // A night camera block (raw light 4) settles near its darkness, not 1.
        let mut night = Vignette::default();
        for _ in 0..80 {
            night.tick(4, 0.);
        }
        let early = night.strength();
        for _ in 0..2000 {
            night.tick(4, 0.);
        }
        let settled = night.strength();
        assert!(early > 0.45 && early < 0.65, "{early}");
        assert!(settled > 0.9 && settled < 0.93, "{settled}");
        // Full light at the camera clears it again, just as gradually.
        night.tick(15, 0.);
        assert!(night.strength() < settled && night.strength() > settled * 0.98);
        for _ in 0..2000 {
            night.tick(15, 0.);
        }
        assert!(night.strength() < 1e-3);
    }

    #[test]
    fn ambient_light_lifts_the_darkest_target() {
        let mut overworld = Vignette::default();
        let mut nether = Vignette::default();
        for _ in 0..3000 {
            overworld.tick(0, 0.);
            nether.tick(0, 0.1);
        }
        assert!((overworld.strength() - 1.).abs() < 1e-3);
        assert!((nether.strength() - 0.9).abs() < 1e-3);
    }
}
