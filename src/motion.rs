//! Presentation-only easing, spring and decay math shared by camera, hand and
//! entity polish. Every function is pure, allocation-free and takes explicit
//! elapsed time so animation stays frame-rate independent. Nothing here feeds
//! physics, raycasts, packets or server-observed state.
use std::f32::consts::PI;

#[inline]
pub fn saturate(x: f32) -> f32 {
    x.clamp(0., 1.)
}
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
#[inline]
pub fn smoothstep(x: f32) -> f32 {
    let x = saturate(x);
    x * x * (3. - 2. * x)
}
#[inline]
pub fn ease_out_quad(x: f32) -> f32 {
    let x = saturate(x);
    1. - (1. - x) * (1. - x)
}
#[inline]
pub fn ease_in_quad(x: f32) -> f32 {
    let x = saturate(x);
    x * x
}
#[inline]
pub fn ease_out_cubic(x: f32) -> f32 {
    let x = saturate(x);
    1. - (1. - x).powi(3)
}
#[inline]
pub fn ease_in_out_cubic(x: f32) -> f32 {
    let x = saturate(x);
    if x < 0.5 {
        4. * x * x * x
    } else {
        1. - (-2. * x + 2.).powi(3) / 2.
    }
}
/// Overshooting settle: 0 at 0, 1 at 1, peaks about 1.1 near 0.7.
#[inline]
pub fn ease_out_back(x: f32) -> f32 {
    let x = saturate(x);
    const C1: f32 = 1.4;
    const C3: f32 = C1 + 1.;
    1. + C3 * (x - 1.).powi(3) + C1 * (x - 1.).powi(2)
}

/// Frame-rate independent exponential approach: after `1 / rate` seconds about
/// 63% of the gap is closed regardless of how that time is split into frames.
#[inline]
pub fn damp_factor(rate: f32, dt: f32) -> f32 {
    1. - (-rate * dt.max(0.)).exp()
}
#[inline]
pub fn damp(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    current + (target - current) * damp_factor(rate, dt)
}
/// Shortest signed distance in degrees from `from` to `to`, in `[-180, 180)`.
#[inline]
pub fn angle_delta(from: f32, to: f32) -> f32 {
    (to - from + 180.).rem_euclid(360.) - 180.
}
#[inline]
pub fn damp_angle(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    current + angle_delta(current, target) * damp_factor(rate, dt)
}

/// Exact damped harmonic oscillator, unconditionally stable for any `dt`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
}
impl Spring {
    pub const fn new(x: f32) -> Self {
        Self { x, v: 0. }
    }
    /// `omega` is the undamped angular frequency in rad/s; `zeta` the damping
    /// ratio (`< 1` rings and overshoots, `>= 1` settles without overshoot).
    pub fn step(&mut self, target: f32, omega: f32, zeta: f32, dt: f32) {
        let dt = dt.clamp(0., 0.25);
        if dt == 0. {
            return;
        }
        let d = self.x - target;
        let (nd, nv) = if zeta < 1. {
            let wd = omega * (1. - zeta * zeta).sqrt();
            let decay = (-zeta * omega * dt).exp();
            let (s, c) = (wd * dt).sin_cos();
            let b = (self.v + zeta * omega * d) / wd;
            let nd = decay * (d * c + b * s);
            let nv = -zeta * omega * nd + decay * (-d * wd * s + (self.v + zeta * omega * d) * c);
            (nd, nv)
        } else {
            let decay = (-omega * dt).exp();
            let b = self.v + omega * d;
            ((d + b * dt) * decay, (self.v - omega * b * dt) * decay)
        };
        self.x = target + nd;
        self.v = nv;
    }
    pub fn kick(&mut self, velocity: f32) {
        self.v += velocity;
    }
    pub fn settled(&self, epsilon: f32) -> bool {
        self.x.abs() < epsilon && self.v.abs() < epsilon
    }
}

/// Camera dip in blocks for a landing after `fall` blocks of fall distance.
/// Short hops still read; big drops saturate so the view never leaves the head.
pub fn landing_depth(fall: f32) -> f32 {
    if fall < 0.5 {
        0.
    } else {
        (0.035 + 0.045 * fall.sqrt()).min(0.26)
    }
}
pub const DIP_OMEGA: f32 = 20.;
pub const DIP_ZETA: f32 = 0.5;
/// Initial downward velocity that makes the `DIP_*` spring peak at `depth`.
pub fn dip_impulse(depth: f32) -> f32 {
    // Peak of x(t) = v0/wd * e^(-z w t) sin(wd t) is v0/w * e^(-z * a / s) * s
    // with s = sqrt(1 - z^2) and a = atan(s / z).
    let s = (1. - DIP_ZETA * DIP_ZETA).sqrt();
    let a = (s / DIP_ZETA).atan();
    let peak = (-DIP_ZETA * a / s).exp() * a.sin() / s / DIP_OMEGA;
    depth / peak
}

/// Vertical squash amount for an entity `age` seconds after landing with an
/// `amplitude` (0..1). Positive squashes (shorter, wider); a small rebound
/// follows. Closed form, so no per-frame state is needed.
pub fn landing_squash(age: f32, amplitude: f32) -> f32 {
    if !(0. ..0.6).contains(&age) {
        return 0.;
    }
    let attack = smoothstep(age / 0.03);
    amplitude * attack * (-age * 9.).exp() * (age * 22.).cos()
}

/// Body breathing scale offset (positive = taller) for `time` seconds.
pub fn breathing(time: f32, phase: f32) -> f32 {
    ((time * 1.7 * PI + phase).sin()) * 0.011
}

/// Attack swing stroke in `0..=1..0`: quick strike, slower recovery.
pub fn swing_stroke(progress: f32) -> f32 {
    (PI * ease_out_quad(progress)).sin()
}

/// Smooth block-placement pop scale after `age` seconds: starts small, lands
/// on exactly 1 at `duration`.
pub fn pop_scale(age: f32, duration: f32) -> f32 {
    if age >= duration {
        return 1.;
    }
    0.7 + 0.3 * ease_out_back(age / duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} != {b}");
    }

    #[test]
    fn easings_hit_endpoints_and_are_monotonic_where_promised() {
        for f in [
            smoothstep,
            ease_out_quad,
            ease_in_quad,
            ease_out_cubic,
            ease_in_out_cubic,
            ease_out_back,
        ] {
            close(f(0.), 0.);
            close(f(1.), 1.);
            close(f(-3.), 0.);
            close(f(9.), 1.);
        }
        let mut last = -1.;
        for i in 0..=100 {
            let v = ease_in_out_cubic(i as f32 / 100.);
            assert!(v >= last);
            last = v;
        }
        let peak = (0..=100)
            .map(|i| ease_out_back(i as f32 / 100.))
            .fold(0., f32::max);
        assert!(peak > 1.05 && peak < 1.15, "{peak}");
    }

    #[test]
    fn damp_is_frame_rate_independent() {
        let one = damp(0., 1., 12., 0.1);
        let mut split = 0.;
        for _ in 0..10 {
            split = damp(split, 1., 12., 0.01);
        }
        close(one, split);
        close(damp(3., 3., 12., 0.1), 3.);
        close(damp(0., 1., 12., 0.), 0.);
    }

    #[test]
    fn angle_damping_takes_the_short_way_across_the_seam() {
        close(angle_delta(350., 10.), 20.);
        close(angle_delta(10., 350.), -20.);
        let next = damp_angle(350., 10., 10., 0.05);
        assert!(next > 350. && next < 370.);
    }

    #[test]
    fn spring_converges_without_overshoot_when_critically_damped() {
        let mut s = Spring::new(1.);
        let mut previous = 1.;
        for _ in 0..200 {
            s.step(0., 14., 1., 1. / 60.);
            assert!(s.x >= -1e-4 && s.x <= previous + 1e-6);
            previous = s.x;
        }
        assert!(s.settled(1e-3));
    }

    #[test]
    fn spring_integration_is_step_size_independent() {
        let mut coarse = Spring { x: 0., v: 5. };
        let mut fine = coarse;
        coarse.step(0.2, 18., 0.45, 0.1);
        for _ in 0..100 {
            fine.step(0.2, 18., 0.45, 0.001);
        }
        close(coarse.x, fine.x);
        close(coarse.v, fine.v);
    }

    #[test]
    fn landing_dip_peaks_at_requested_depth_then_rings_out() {
        let depth = landing_depth(3.);
        assert!(depth > landing_depth(1.) && depth <= 0.26);
        assert_eq!(landing_depth(0.2), 0.);
        let mut s = Spring::new(0.);
        s.kick(dip_impulse(depth));
        let mut peak = 0f32;
        for _ in 0..120 {
            s.step(0., DIP_OMEGA, DIP_ZETA, 1. / 120.);
            peak = peak.max(s.x);
        }
        assert!((peak - depth).abs() < depth * 0.03, "{peak} vs {depth}");
        for _ in 0..240 {
            s.step(0., DIP_OMEGA, DIP_ZETA, 1. / 120.);
        }
        assert!(s.settled(1e-3));
    }

    #[test]
    fn landing_dip_scales_with_fall_and_saturates() {
        assert!(landing_depth(10.) > landing_depth(1.));
        close(landing_depth(100.), 0.26);
    }

    #[test]
    fn squash_starts_flat_peaks_early_and_expires() {
        close(landing_squash(0., 0.3), 0.);
        let early = landing_squash(0.03, 0.3);
        assert!(early > 0.15, "{early}");
        assert!(landing_squash(0.45, 0.3).abs() < 0.03);
        assert_eq!(landing_squash(0.7, 0.3), 0.);
        assert_eq!(landing_squash(-1., 0.3), 0.);
    }

    #[test]
    fn swing_stroke_strikes_fast_and_recovers_slow() {
        close(swing_stroke(0.), 0.);
        close(swing_stroke(1.), 0.);
        let peak_at = (0..=100)
            .map(|i| i as f32 / 100.)
            .max_by(|a, b| swing_stroke(*a).total_cmp(&swing_stroke(*b)))
            .unwrap();
        assert!(peak_at < 0.4, "{peak_at}");
        close(swing_stroke(peak_at), 1.);
    }

    #[test]
    fn pop_scale_lands_exactly_on_one() {
        assert!(pop_scale(0., 0.1) < 0.75);
        close(pop_scale(0.1, 0.1), 1.);
        close(pop_scale(5., 0.1), 1.);
        assert!(pop_scale(0.07, 0.1) > 1.0);
    }

    #[test]
    fn breathing_stays_subtle() {
        for i in 0..200 {
            assert!(breathing(i as f32 * 0.05, 1.3).abs() <= 0.0111);
        }
    }

    #[test]
    fn view_motion_lands_leans_and_lags_without_drift() {
        let mut m = ViewMotion::default();
        let mut input = ViewInput { yaw: 10., grounded: true, ..Default::default() };
        m.step(0.016, &input);
        m.land(4.);
        let mut peak = 0f32;
        for _ in 0..30 {
            m.step(1. / 120., &input);
            peak = peak.max(m.dip.x);
        }
        assert!(peak > 0.05 && peak <= 0.28, "{peak}");
        for _ in 0..300 {
            m.step(1. / 120., &input);
        }
        assert!(m.dip.settled(1e-3));
        // Strafing left rolls counter-clockwise; releasing relaxes back to level.
        input.strafe = 1.;
        for _ in 0..120 {
            m.step(1. / 120., &input);
        }
        assert!((m.roll.x + 1.2).abs() < 0.05, "{}", m.roll.x);
        input.strafe = 0.;
        for _ in 0..240 {
            m.step(1. / 120., &input);
        }
        assert!(m.roll.x.abs() < 0.01);
        // A sudden right turn: the hand lags left, then catches up.
        input.yaw = 70.;
        m.step(0.016, &input);
        assert!(m.hand(0., 70., 0.).offset[0] < -0.02);
        for _ in 0..240 {
            m.step(1. / 120., &input);
        }
        assert!(m.hand(1., 70., 0.).offset[0].abs() < 0.001);
    }

    #[test]
    fn view_motion_time_split_does_not_change_the_result() {
        let input = ViewInput { yaw: 40., strafe: -1., grounded: true, ..Default::default() };
        let mut one = ViewMotion::default();
        let mut split = ViewMotion::default();
        one.step(0., &ViewInput { yaw: 0., ..input });
        split.step(0., &ViewInput { yaw: 0., ..input });
        one.land(2.);
        split.land(2.);
        one.step(0.1, &input);
        for _ in 0..10 {
            split.step(0.01, &input);
        }
        close(one.dip.x, split.dip.x);
        close(one.roll.x, split.roll.x);
        close(one.hand(0.5, 40., 0.).offset[0], split.hand(0.5, 40., 0.).offset[0]);
    }

    #[test]
    fn use_bump_pushes_forward_then_returns() {
        let mut m = ViewMotion::default();
        let input = ViewInput::default();
        m.step(0., &input);
        m.bump_hand();
        let mut deepest = 0f32;
        for _ in 0..30 {
            m.step(1. / 240., &input);
            deepest = deepest.min(m.hand(1., 0., 0.).offset[2]);
        }
        assert!(deepest < -0.03 && deepest > -0.2, "{deepest}");
        for _ in 0..480 {
            m.step(1. / 240., &input);
        }
        assert!(m.hand(1., 0., 0.).offset[2].abs() < 0.001);
    }

    #[test]
    fn eating_blend_eases_in_and_out() {
        let mut m = ViewMotion::default();
        m.step(0., &ViewInput::default());
        let eat = ViewInput { consume: 1., ..Default::default() };
        m.step(0.05, &eat);
        let early = m.consume;
        assert!(early > 0.2 && early < 0.7);
        for _ in 0..60 {
            m.step(0.05, &eat);
        }
        assert!(m.consume > 0.99);
        for _ in 0..60 {
            m.step(0.05, &ViewInput::default());
        }
        assert!(m.consume < 0.01);
    }

    fn standing(tick: u64) -> EntityTick {
        EntityTick {
            tick,
            on_ground: true,
            y: 64.,
            walk_speed: 0.,
            body_yaw: 0.,
            position: [0., 64., 0.],
            can_look: true,
            target: None,
        }
    }

    #[test]
    fn entity_landing_squash_follows_a_real_fall_only() {
        let mut m = EntityMotion::default();
        m.tick(&EntityTick { on_ground: false, y: 67., ..standing(1) });
        m.tick(&EntityTick { on_ground: false, y: 66., ..standing(2) });
        m.tick(&EntityTick { on_ground: false, y: 65., ..standing(3) });
        m.tick(&EntityTick { on_ground: true, y: 64., ..standing(4) });
        let squash = (0..=19)
            .map(|i| m.squash(4, i as f32 * 0.05))
            .fold(0., f32::max);
        assert!(squash > 0.08 && squash <= 0.2, "{squash}");
        assert_eq!(m.squash(4 + 40, 0.), 0.);
        assert_eq!(m.squash(3, 0.5), 0.);
        // Stepping onto the ground by sub-threshold motion does not squash.
        let mut calm = EntityMotion::default();
        calm.tick(&EntityTick { on_ground: false, y: 64.05, ..standing(1) });
        calm.tick(&EntityTick { on_ground: true, y: 64., ..standing(2) });
        assert_eq!(calm.squash(2, 0.1), 0.);
        // Walking down a stair: one airborne tick, a big drop, still no squash.
        let mut stairs = EntityMotion::default();
        stairs.tick(&EntityTick { on_ground: true, y: 65., ..standing(1) });
        stairs.tick(&EntityTick { on_ground: false, y: 64.6, ..standing(2) });
        stairs.tick(&EntityTick { on_ground: true, y: 64., ..standing(3) });
        assert_eq!(stairs.squash(3, 0.1), 0.);
    }

    #[test]
    fn idle_mob_turns_head_to_nearby_player_and_releases_it() {
        let mut m = EntityMotion::default();
        let near = Some([3., 65.6, 3.]);
        for tick in 1..=9 {
            m.tick(&EntityTick { target: near, ..standing(tick) });
        }
        // Must stand still for half a second before reacting.
        assert_eq!(m.look(1.).2, 0.);
        for tick in 10..=60 {
            m.tick(&EntityTick { target: near, ..standing(tick) });
        }
        let (yaw, pitch, weight) = m.look(1.);
        assert!(weight > 0.95);
        // Target is at +x/+z: Minecraft yaw -45 (forward is (-sin, cos)).
        assert!((yaw + 45.).abs() < 1., "{yaw}");
        assert!(pitch.abs() < 15.);
        // Behind the body or out of range or walking releases the look.
        for tick in 61..=140 {
            m.tick(&EntityTick { target: Some([0., 65., -4.]), ..standing(tick) });
        }
        assert!(m.look(1.).2 < 0.01);
        for tick in 141..=200 {
            m.tick(&EntityTick { target: near, ..standing(tick) });
        }
        assert!(m.look(1.).2 > 0.9);
        for tick in 201..=260 {
            m.tick(&EntityTick { target: near, walk_speed: 0.5, ..standing(tick) });
        }
        assert!(m.look(1.).2 < 0.01);
    }

    #[test]
    fn look_interpolation_takes_the_short_arc() {
        let m = EntityMotion {
            look_old: [350., 0., 1.],
            look: [10., 0., 1.],
            ..Default::default()
        };
        let (yaw, _, _) = m.look(0.5);
        assert!((yaw - 360.).abs() < 1e-3 || yaw.abs() < 1e-3, "{yaw}");
    }

    /// Cost of the per-frame/per-tick polish math, run with
    /// `cargo test --release --lib bench_motion_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_motion_cost() {
        use glam::{Mat4, Vec3};
        use std::hint::black_box;
        use std::time::Instant;
        let n = 2_000_000u32;
        let input = ViewInput { yaw: 10., pitch: 5., strafe: 1., grounded: true, consume: 0. };
        let mut view = ViewMotion::default();
        view.step(0., &input);
        let t = Instant::now();
        for i in 0..n {
            view.step(black_box(0.016), &input);
            black_box(view.hand(black_box(0.3), 10. + (i % 7) as f32, 5.));
        }
        let view_ns = t.elapsed().as_nanos() as f64 / f64::from(n);
        let mut entity = EntityMotion::default();
        let target = Some([3., 65.6, 3.]);
        let t = Instant::now();
        for tick in 0..u64::from(n) {
            entity.tick(&EntityTick { target, ..standing(black_box(tick)) });
        }
        let tick_ns = t.elapsed().as_nanos() as f64 / f64::from(n);
        let base = Mat4::from_translation(Vec3::new(1., 2., 3.));
        let t = Instant::now();
        let mut sink = 0f32;
        for i in 0..n {
            let partial = (i % 20) as f32 * 0.05;
            let s = breathing(black_box(i as f32 * 0.05), 1.3) - entity.squash(10, partial);
            let (_, _, w) = entity.look(partial);
            let lean = hurt_lean(partial * 4.);
            let at = Vec3::new(1., 2., 3.);
            let matrix = Mat4::from_translation(at)
                * Mat4::from_scale(Vec3::new(1. - s * 0.5, 1. + s, 1. - s * 0.5))
                * Mat4::from_translation(-at)
                * Mat4::from_axis_angle(Vec3::new(0.6, 0., 0.8), -lean.to_radians())
                * base;
            sink += matrix.w_axis.y + w;
        }
        let entity_frame_ns = t.elapsed().as_nanos() as f64 / f64::from(n);
        black_box(sink);
        println!(
            "motion_cost view_frame_ns={view_ns:.1} entity_tick_ns={tick_ns:.1} entity_frame_ns={entity_frame_ns:.1} \
             frame_ms_for_500_entities={:.4}",
            (view_ns + 500. * entity_frame_ns) / 1e6
        );
    }

    #[test]
    fn hurt_lean_rises_and_expires() {
        close(hurt_lean(0.), 0.);
        assert!(hurt_lean(1.5) > 6.);
        assert!(hurt_lean(6.) < hurt_lean(2.));
        assert_eq!(hurt_lean(10.), 0.);
        assert_eq!(hurt_lean(-1.), 0.);
    }
}

/// Per-entity presentation state advanced once per game tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct EntityMotion {
    ready: bool,
    was_ground: bool,
    last_y: f32,
    land_tick: u64,
    land_amp: f32,
    idle_ticks: u16,
    air_ticks: u8,
    /// `[absolute_yaw_deg, pitch_deg, weight]` of the head look-at, previous
    /// and current tick; sampled with the partial tick.
    look_old: [f32; 3],
    look: [f32; 3],
}
/// One tick of observed facts about an entity.
#[derive(Clone, Copy, Debug)]
pub struct EntityTick {
    pub tick: u64,
    pub on_ground: bool,
    pub y: f32,
    /// Horizontal walk speed of the source gait (0 when standing).
    pub walk_speed: f32,
    pub body_yaw: f32,
    pub position: [f32; 3],
    /// Whether this entity may turn its head toward `target` at all.
    pub can_look: bool,
    /// Local player eye position, when a player is in the world.
    pub target: Option<[f32; 3]>,
}
pub const LOOK_RANGE: f32 = 6.;
pub const LOOK_BODY_LIMIT: f32 = 100.;
pub const LOOK_HEAD_LIMIT: f32 = 60.;
impl EntityMotion {
    pub fn tick(&mut self, t: &EntityTick) {
        if !self.ready {
            self.ready = true;
            self.was_ground = t.on_ground;
            self.last_y = t.y;
        }
        let vy = t.y - self.last_y;
        let airborne = self.air_ticks;
        // A real fall is airborne for a few ticks; stepping down a stair or slab
        // flips the ground flag for one tick and must not squash the mob.
        self.air_ticks = if t.on_ground { 0 } else { self.air_ticks.saturating_add(1) };
        if t.on_ground && !self.was_ground && vy < -0.12 && airborne >= 3 {
            self.land_tick = t.tick;
            self.land_amp = (-vy * 0.6).clamp(0.05, 0.2);
        }
        self.was_ground = t.on_ground;
        self.last_y = t.y;
        self.idle_ticks = if t.walk_speed < 0.05 && t.on_ground {
            self.idle_ticks.saturating_add(1)
        } else {
            0
        };
        self.look_old = self.look;
        let mut desired = [self.look[0], self.look[1], 0.];
        if let (true, Some(target), true) = (t.can_look, t.target, self.idle_ticks >= 10) {
            let d = [
                target[0] - t.position[0],
                target[1] - (t.position[1] + 0.9),
                target[2] - t.position[2],
            ];
            let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
            let yaw = (-d[0]).atan2(d[2]).to_degrees();
            if flat * flat + d[1] * d[1] < LOOK_RANGE * LOOK_RANGE
                && angle_delta(t.body_yaw, yaw).abs() < LOOK_BODY_LIMIT
            {
                desired = [yaw, (-d[1].atan2(flat).to_degrees()).clamp(-35., 35.), 1.];
            }
        }
        // First engagement starts from the body direction, never from a stale heading.
        if self.look[2] < 0.01 && desired[2] > 0. {
            self.look[0] = t.body_yaw;
            self.look_old[0] = t.body_yaw;
        }
        self.look[0] = damp_angle(self.look[0], desired[0], 8., 0.05);
        self.look[1] = damp(self.look[1], desired[1], 8., 0.05);
        self.look[2] = damp(self.look[2], desired[2], 6., 0.05);
        if self.look[2] < 0.002 {
            self.look[2] = 0.;
        }
    }
    /// Vertical squash amount (positive = flatter) `partial` into `tick`.
    pub fn squash(&self, tick: u64, partial: f32) -> f32 {
        if self.land_amp == 0. || tick < self.land_tick {
            return 0.;
        }
        landing_squash((tick - self.land_tick) as f32 * 0.05 + partial * 0.05, self.land_amp)
    }
    /// Head look-at `(absolute_yaw, pitch, weight)` sampled between ticks.
    pub fn look(&self, partial: f32) -> (f32, f32, f32) {
        let p = saturate(partial);
        (
            self.look_old[0] + angle_delta(self.look_old[0], self.look[0]) * p,
            lerp(self.look_old[1], self.look[1], p),
            lerp(self.look_old[2], self.look[2], p),
        )
    }
}
/// Short backward lean (degrees) after being hit, `age` in ticks.
pub fn hurt_lean(age: f32) -> f32 {
    if !(0. ..10.).contains(&age) {
        return 0.;
    }
    smoothstep(age / 1.5) * (1. - smoothstep(age / 9.)) * 9.
}

/// Per-frame inputs for [`ViewMotion::step`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewInput {
    pub yaw: f32,
    pub pitch: f32,
    /// Held strafe input: `+1` left, `-1` right (HeldInput::strafe).
    pub strafe: f32,
    pub grounded: bool,
    /// 1 while eating/drinking with the hand, else 0.
    pub consume: f32,
}
/// Render-only hand offset relative to the camera, before swing/equip.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandPose {
    pub offset: [f32; 3],
    pub roll_deg: f32,
}
/// Frame-time springs behind landing dip, strafe tilt, hand inertia, idle sway
/// and use bump. Owns no physics; sampled once per frame by the camera.
#[derive(Clone, Debug, Default)]
pub struct ViewMotion {
    time: f64,
    ready: bool,
    /// Camera dip in blocks, positive = lower.
    pub dip: Spring,
    /// Camera roll in degrees, positive = clockwise.
    pub roll: Spring,
    /// Hand push in blocks, positive = away from the camera.
    pub bump: Spring,
    lag: [f32; 2],
    /// Eased 0..1 eating/drinking pose blend.
    pub consume: f32,
}
impl ViewMotion {
    pub fn land(&mut self, fall: f32) {
        let depth = landing_depth(fall);
        if depth > 0. {
            self.dip.kick(dip_impulse(depth));
        }
    }
    pub fn bump_hand(&mut self) {
        self.bump.kick(BUMP_IMPULSE);
    }
    pub fn step(&mut self, dt: f32, input: &ViewInput) {
        let dt = dt.clamp(0., 0.1);
        if !self.ready {
            self.lag = [input.yaw, input.pitch];
            self.ready = true;
        }
        self.time += f64::from(dt);
        self.dip.step(0., DIP_OMEGA, DIP_ZETA, dt);
        let lean = if input.grounded { 1.2 } else { 0.5 };
        self.roll.step(-input.strafe * lean, ROLL_OMEGA, 1., dt);
        self.bump.step(0., BUMP_OMEGA, BUMP_ZETA, dt);
        self.lag[0] = damp_angle(self.lag[0], input.yaw, LAG_RATE, dt);
        self.lag[1] = damp(self.lag[1], input.pitch, LAG_RATE, dt);
        self.consume = damp(self.consume, input.consume, 12., dt);
    }
    /// Hand offsets for the current spring state; `walking` is the 0..1
    /// bob amplitude that suppresses idle sway while the walk bob dominates.
    pub fn hand(&self, walking: f32, yaw: f32, pitch: f32) -> HandPose {
        let idle = 1. - smoothstep(walking * 3.);
        let t = (self.time % 600.) as f32;
        let turn = angle_delta(self.lag[0], yaw);
        let tilt = pitch - self.lag[1];
        HandPose {
            offset: [
                (t * 0.9).sin() * 0.004 * idle
                    - (turn * 0.0035).clamp(-0.06, 0.06)
                    - self.roll.x * 0.004,
                (t * 1.3 + 1.).sin() * 0.005 * idle
                    + (tilt * 0.003).clamp(-0.05, 0.05)
                    + self.dip.x * 0.5
                    - self.bump.x * 0.5,
                -self.bump.x,
            ],
            roll_deg: (t * 0.7).sin() * 0.5 * idle - (turn * 0.25).clamp(-5., 5.),
        }
    }
}
pub const ROLL_OMEGA: f32 = 12.;
pub const BUMP_OMEGA: f32 = 24.;
pub const BUMP_ZETA: f32 = 0.45;
pub const BUMP_IMPULSE: f32 = 2.2;
pub const LAG_RATE: f32 = 10.;
