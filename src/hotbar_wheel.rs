//! Mouse-wheel to hotbar-slot conversion (vanilla: one notch = one slot).
//!
//! winit reports wheels either as `LineDelta` (fractional on high-resolution
//! wheels/touchpads, multiples of 1.0 on classic wheels and Windows 120-unit
//! deltas) or as `PixelDelta`. Both are accumulated here as fractional notches
//! and only whole notches move the selection, so a single physical notch can
//! never produce more than one step.
//!
//! X11 sources that report a wheel as buttons 4..=7 (XTest/Xvfb, Xwayland,
//! VNC: the classic core-protocol wheel) send a press *and* a release per
//! notch, and winit 0.30.12 turns both into `MouseWheel` with the same delta.
//! Vanilla (GLFW) counts the press only, so the release of a notch that has
//! just been counted is dropped here; an identical whole notch further than
//! [`SAME_NOTCH_WINDOW`] later is a new notch.

use std::time::{Duration, Instant};

/// Pixel-delta units that make up one wheel notch (touchpad scrolling).
pub(crate) const PIXELS_PER_NOTCH: f64 = 40.0;
/// One button-emulated notch delivers its press and release back to back; a
/// genuinely repeated notch cannot arrive this soon (the fastest free-spinning
/// wheels emit ~10 ms apart).
pub(crate) const SAME_NOTCH_WINDOW: Duration = Duration::from_millis(2);
const HOTBAR_SLOTS: i64 = 9;

#[derive(Debug, Default)]
pub(crate) struct HotbarWheel {
    accumulated: f64,
    /// Delta and receipt time of the last whole line notch, used to drop the
    /// press/release duplicate of the same physical notch.
    previous: Option<(f64, Instant)>,
}

impl HotbarWheel {
    /// Accumulate a line delta (notches, positive = wheel up).
    pub(crate) fn push_lines(&mut self, lines: f64) -> i64 {
        self.push_at(lines, Instant::now(), true)
    }

    /// Accumulate a pixel delta (positive = scroll up).
    pub(crate) fn push_pixels(&mut self, pixels: f64) -> i64 {
        self.push_at(pixels / PIXELS_PER_NOTCH, Instant::now(), false)
    }

    /// Line delta with an explicit receipt time.
    #[cfg(test)]
    pub(crate) fn push_lines_at(&mut self, lines: f64, at: Instant) -> i64 {
        self.push_at(lines, at, true)
    }

    /// Returns whole notches crossed (positive = up/left, negative = down/right).
    fn push_at(&mut self, notches: f64, at: Instant, dedupe: bool) -> i64 {
        if !notches.is_finite() || notches == 0.0 {
            return 0;
        }
        // Whole notches only: a high-resolution wheel emits identical fractional
        // deltas at millisecond cadence and every one of them is real movement.
        let whole_notch = notches.abs() >= 1.0 && notches.fract() == 0.0;
        if dedupe && whole_notch {
            let duplicate = self.previous.take().is_some_and(|(previous, when)| {
                previous == notches && at.saturating_duration_since(when) <= SAME_NOTCH_WINDOW
            });
            if duplicate {
                return 0;
            }
            self.previous = Some((notches, at));
        } else if !whole_notch {
            self.previous = None;
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
        self.previous = None;
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

    /// Live Xvfb/XTest reproduction: one wheel notch is a button press plus a
    /// button release, and winit 0.30 turns both into the same `MouseWheel`.
    /// Observed before the fix: 8 -> 1 for one notch down (two slots).
    #[test]
    fn x11_button_press_and_release_is_one_notch() {
        let mut w = HotbarWheel::default();
        let t = Instant::now();
        let press = w.push_lines_at(-1.0, t);
        assert_eq!(press, -1, "press is the notch");
        assert_eq!(step_slot(8, press), 0, "one notch down from 8 reaches 0");
        let release = w.push_lines_at(-1.0, t + Duration::from_micros(80));
        assert_eq!(
            release, 0,
            "release of the same notch is not a second notch"
        );
    }

    #[test]
    fn two_notches_in_one_burst_are_two_slots() {
        let mut w = HotbarWheel::default();
        let t = Instant::now();
        let totals: Vec<i64> = (0..4)
            .map(|i| w.push_lines_at(1.0, t + Duration::from_micros(80) * i as u32))
            .collect();
        assert_eq!(totals, vec![1, 0, 1, 0], "press/release pairs, not halved");
    }

    #[test]
    fn distinct_notches_outside_the_window_are_all_counted() {
        let mut w = HotbarWheel::default();
        let t = Instant::now();
        assert_eq!(w.push_lines_at(-1.0, t), -1);
        assert_eq!(w.push_lines_at(-1.0, t + SAME_NOTCH_WINDOW * 2), -1);
        assert_eq!(w.push_lines_at(-1.0, t + SAME_NOTCH_WINDOW * 4), -1);
    }

    #[test]
    fn high_resolution_fractional_events_are_never_deduplicated() {
        let mut w = HotbarWheel::default();
        let t = Instant::now();
        let mut total = 0;
        for i in 0..4 {
            total += w.push_lines_at(-0.5, t + Duration::from_micros(50) * i as u32);
        }
        assert_eq!(total, -2, "four half-notches are two notches");
    }

    #[test]
    fn reset_forgets_a_pending_duplicate() {
        let mut w = HotbarWheel::default();
        let t = Instant::now();
        assert_eq!(w.push_lines_at(-1.0, t), -1);
        w.reset();
        assert_eq!(
            w.push_lines_at(-1.0, t + Duration::from_micros(80)),
            -1,
            "after focus loss the next notch counts"
        );
    }
}
