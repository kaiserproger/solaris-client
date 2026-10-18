//! Target MobEffectInstance.BlendState, MobEffects.DARKNESS and
//! LivingEntity.forceAddEffect: 22-tick in/out ramps, post-decrement duration,
//! initial network bit-8 skip, and replacement-state preservation.
//!
//! Tick authority is the local player's client tick, not world-clock progress:
//! TickRateManager.isEntityFrozen never freezes Player effects. Client-side
//! expiry does not remove membership; only the received remove packet does.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DarknessSnapshot {
    /// Fresh on absent-to-present insertion; retained across replacements.
    pub generation: u64,
    /// Changes on every received update, including updates within one tick.
    pub revision: u64,
    pub duration: i32,
    pub flags: u8,
    pub observed_tick: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DarknessBlend {
    snapshot: Option<DarknessSnapshot>,
    duration: i32,
    tick: u64,
    factor: f32,
    previous: f32,
}

impl DarknessBlend {
    /// Observe every received effect update/removal in packet order, then sample
    /// at render time. Merely polling the final snapshot of a packet batch can
    /// lose the first-add skip flag when an effect is replaced within that batch.
    /// All snapshots and ticks must use the same monotonic client-player clock.
    /// Pass partial=1 while world ticking is frozen, matching DeltaTracker.
    pub fn sample(&mut self, snapshot: Option<DarknessSnapshot>, tick: u64, partial: f32) -> f32 {
        let Some(incoming) = snapshot else {
            *self = Self {
                tick,
                ..Self::default()
            };
            return 0.;
        };
        match self.snapshot {
            Some(old) if old.generation == incoming.generation => {
                if old.revision != incoming.revision {
                    self.advance(incoming.observed_tick);
                    self.duration = incoming.duration;
                }
            }
            _ => {
                self.tick = incoming.observed_tick;
                self.duration = incoming.duration;
                self.factor = if incoming.flags & 8 == 0 && self.has_effect() {
                    1.
                } else {
                    0.
                };
                self.previous = self.factor;
            }
        }
        self.snapshot = Some(incoming);
        self.advance(tick);
        self.previous + partial.clamp(0., 1.) * (self.factor - self.previous)
    }

    fn has_effect(&self) -> bool {
        self.duration == -1 || self.duration > 22
    }

    fn advance(&mut self, tick: u64) {
        const STEP: f32 = 1. / 22.;
        while self.tick < tick {
            // Skip only source-stable plateaus. Do not replace repeated f32
            // additions during a ramp with a differently rounded closed form.
            if self.duration == -1 && self.factor == 1.
                || self.duration != -1 && self.duration <= 0 && self.factor == 0.
            {
                self.previous = self.factor;
                self.tick = tick;
                break;
            }
            if self.factor == 1. && self.duration > 23 {
                let skip = (tick - self.tick).min((self.duration - 23) as u64);
                self.duration -= skip as i32;
                self.tick += skip;
                self.previous = self.factor;
                continue;
            }
            self.tick += 1;
            if self.duration > 0 {
                self.duration -= 1;
            }
            self.previous = self.factor;
            let target = if self.has_effect() { 1. } else { 0. };
            self.factor += (target - self.factor).clamp(-STEP, STEP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn effect(duration: i32, flags: u8) -> DarknessSnapshot {
        DarknessSnapshot {
            generation: 1,
            revision: 1,
            duration,
            flags,
            observed_tick: 0,
        }
    }
    #[test]
    fn blends_in_and_out_at_post_decrement_duration_boundary() {
        let mut fade = DarknessBlend::default();
        let state = effect(100, 8);
        assert_eq!(fade.sample(Some(state), 0, 1.), 0.);
        assert!((fade.sample(Some(state), 11, 1.) - 0.5).abs() < 1e-6);
        assert_eq!(fade.sample(Some(state), 22, 1.), 1.);
        assert_eq!(fade.sample(Some(state), 77, 1.), 1.);
        assert!((fade.sample(Some(state), 78, 1.) - 21. / 22.).abs() < 1e-6);
        assert!((fade.sample(Some(state), 89, 1.) - 10. / 22.).abs() < 1e-6);
        assert_eq!(fade.sample(Some(state), 100, 1.), 0.);
    }
    #[test]
    fn first_skip_and_same_tick_replacement_have_different_authority() {
        let mut fade = DarknessBlend::default();
        let initial = effect(100, 0);
        assert_eq!(fade.sample(Some(initial), 0, 1.), 1.);
        let replacement = DarknessSnapshot {
            revision: 2,
            duration: 10,
            flags: 8,
            ..initial
        };
        assert_eq!(fade.sample(Some(replacement), 0, 1.), 1.);
        assert!((fade.sample(Some(replacement), 1, 1.) - 21. / 22.).abs() < 1e-6);
        let mut ramp = DarknessBlend::default();
        let initial = effect(100, 8);
        ramp.sample(Some(initial), 0, 1.);
        let before = ramp.sample(Some(initial), 3, 1.);
        let replacement = DarknessSnapshot {
            revision: 2,
            observed_tick: 3,
            flags: 0,
            ..initial
        };
        assert_eq!(ramp.sample(Some(replacement), 3, 1.), before);
    }
    #[test]
    fn removal_and_same_batch_readd_reset_and_infinite_duration_keeps_blending() {
        let mut fade = DarknessBlend::default();
        let initial = effect(-1, 8);
        fade.sample(Some(initial), 0, 1.);
        assert_eq!(fade.sample(Some(initial), 22, 1.), 1.);
        assert_eq!(fade.sample(None, 22, 1.), 0.);
        let readded = DarknessSnapshot {
            generation: 2,
            revision: 3,
            observed_tick: 22,
            ..initial
        };
        assert_eq!(fade.sample(Some(readded), 22, 1.), 0.);
        assert!((fade.sample(Some(readded), 23, 0.5) - 0.5 / 22.).abs() < 1e-6);
        assert_eq!(fade.sample(Some(readded), 100000, 1.), 1.);
    }
}
