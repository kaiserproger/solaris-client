//! Mouse-wheel to hotbar-slot conversion (vanilla: one notch = one slot).
//!
//! winit reports wheels either as `LineDelta` (fractional on high-resolution
//! wheels/touchpads, multiples of 1.0 on classic wheels and Windows 120-unit
//! deltas) or as `PixelDelta`. Both are accumulated here as fractional notches
//! and only whole notches move the selection, so a single physical notch can
//! never produce more than one step.

/// Pixel-delta units that make up one wheel notch (touchpad scrolling).
pub(crate) const PIXELS_PER_NOTCH: f64 = 40.0;
const HOTBAR_SLOTS: i64 = 9;

#[derive(Debug, Default)]
pub(crate) struct HotbarWheel {
    accumulated: f64,
}

impl HotbarWheel {
    /// Accumulate a line delta (notches, positive = wheel up).
    pub(crate) fn push_lines(&mut self, lines: f64) -> i64 {
        self.push(lines)
    }

    /// Accumulate a pixel delta (positive = scroll up).
    pub(crate) fn push_pixels(&mut self, pixels: f64) -> i64 {
        self.push(pixels / PIXELS_PER_NOTCH)
    }

    /// Returns whole notches crossed (positive = up/left, negative = down/right).
    fn push(&mut self, notches: f64) -> i64 {
        if !notches.is_finite() || notches == 0.0 {
            return 0;
        }
        // Reversing direction discards the partial notch of the old direction.
        if self.accumulated != 0.0 && self.accumulated.signum() != notches.signum() {
            self.accumulated = 0.0;
        }
        self.accumulated += notches;
        // Tolerance: ten 0.1 steps sum to 0.9999999999999999 in f64.
        let whole = (self.accumulated + self.accumulated.signum() * 1e-9).trunc();
        self.accumulated = if whole == 0.0 {
            self.accumulated
        } else {
            self.accumulated - whole
        };
        if self.accumulated.abs() < 1e-9 {
            self.accumulated = 0.0;
        }
        whole as i64
    }

    /// Reset any partial notch (e.g. focus loss or leaving the game screen).
    pub(crate) fn reset(&mut self) {
        self.accumulated = 0.0;
    }
}

/// Slot reached from `selected` after `notches` (positive = up = left), wrapping 0..=8.
pub(crate) fn step_slot(selected: u8, notches: i64) -> u8 {
    (i64::from(selected) - notches).rem_euclid(HOTBAR_SLOTS) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_notch_moves_exactly_one_slot() {
        let mut w = HotbarWheel::default();
        assert_eq!(step_slot(4, w.push_lines(-1.0)), 5, "wheel down = right");
        assert_eq!(step_slot(4, w.push_lines(1.0)), 3, "wheel up = left");
    }

    #[test]
    fn wraps_both_ways() {
        assert_eq!(step_slot(8, -1), 0);
        assert_eq!(step_slot(0, 1), 8);
        assert_eq!(step_slot(8, -1 - 9), 0);
    }

    #[test]
    fn fractional_line_deltas_accumulate_to_one_notch() {
        let mut w = HotbarWheel::default();
        let mut total = 0;
        for _ in 0..8 {
            total += w.push_lines(-0.125);
        }
        assert_eq!(total, -1);
        assert_eq!(w.push_lines(-0.125), 0, "remainder restarts");
    }

    #[test]
    fn windows_120_multiples_normalised_by_winit_are_whole_notches() {
        let mut w = HotbarWheel::default();
        assert_eq!(w.push_lines(-120.0 / 120.0), -1);
        assert_eq!(w.push_lines(-240.0 / 120.0), -2);
    }

    #[test]
    fn pixel_deltas_accumulate_across_events() {
        let mut w = HotbarWheel::default();
        let mut total = 0;
        for _ in 0..10 {
            total += w.push_pixels(-4.0);
        }
        assert_eq!(total, -1, "40 px total = one notch");
        assert_eq!(
            w.push_pixels(-100.0),
            -2,
            "100 px = two notches, 20 px remain"
        );
        assert_eq!(w.push_pixels(-20.0), -1);
    }

    #[test]
    fn direction_reversal_drops_partial_notch() {
        let mut w = HotbarWheel::default();
        assert_eq!(w.push_lines(-0.75), 0);
        assert_eq!(w.push_lines(0.5), 0);
        assert_eq!(w.push_lines(0.5), 1);
    }

    #[test]
    fn zero_and_non_finite_are_ignored() {
        let mut w = HotbarWheel::default();
        assert_eq!(w.push_lines(0.0), 0);
        assert_eq!(w.push_lines(f64::NAN), 0);
        assert_eq!(w.push_pixels(f64::INFINITY), 0);
    }

    #[test]
    fn full_lap_does_not_change_slot() {
        assert_eq!(step_slot(3, 9), 3);
        assert_eq!(step_slot(3, -18), 3);
    }
}
