//! Source presentation equations, independent of packet storage and geometry emission.
pub(crate) fn sin(angle: f32) -> f32 {
    crate::player::math::sin(f64::from(angle))
}
pub(crate) fn cos(angle: f32) -> f32 {
    crate::player::math::cos(f64::from(angle))
}
pub(crate) fn lerp(previous: f32, current: f32, partial: f32) -> f32 {
    previous + (current - previous) * partial.clamp(0., 1.)
}
pub(crate) fn wrap_radians(mut angle: f32) -> f32 {
    while angle >= std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
}
pub(crate) fn lid_angle(progress: f32) -> f32 {
    -(1. - (1. - progress).powi(3)) * std::f32::consts::FRAC_PI_2
}
pub(crate) fn book_angles(time: f32, flip: f32, open: f32) -> [f32; 3] {
    let openness = (sin(time * 0.02) * 0.1 + 1.25) * open;
    [
        openness,
        ((flip + 0.25).rem_euclid(1.) * 1.6 - 0.3).clamp(0., 1.),
        ((flip + 0.75).rem_euclid(1.) * 1.6 - 0.3).clamp(0., 1.),
    ]
}
pub(crate) fn bell_angles(time: f32, direction: u8) -> [f32; 2] {
    let swing = sin(time / std::f32::consts::PI) / (4. + time / 3.);
    match direction {
        2 => [-swing, 0.],
        3 => [swing, 0.],
        4 => [0., swing],
        5 => [0., -swing],
        _ => [0., 0.],
    }
}
pub(crate) fn brush_translation(direction: u8, dusted: u8) -> [f32; 3] {
    let offset = f32::from(dusted) / 10. * 0.75;
    let mut p = [0.5, 0.5, 0.5];
    match direction {
        0 => p[1] -= 0.23 + offset,
        1 => p[1] += 0.25 + offset,
        2 => p[2] = 0.25 - offset,
        3 => p[2] = 0.73 + offset,
        4 => p[0] = 0.25 - offset,
        5 => p[0] = 0.73 + offset,
        _ => {}
    }
    p
}
pub(crate) fn pot_angles(progress: f32, positive: bool) -> [f32; 3] {
    if !(0. ..=1.).contains(&progress) {
        return [0.; 3];
    }
    if positive {
        let phase = progress * std::f32::consts::TAU;
        [
            -1.5 * (cos(phase) + 0.5) * sin(phase / 2.) / 64.,
            0.,
            sin(phase) / 64.,
        ]
    } else {
        [
            0.,
            sin(-progress * 3. * std::f32::consts::PI) * 0.125 * (1. - progress),
            0.,
        ]
    }
}
pub(crate) fn banner_angle(phase: f32) -> f32 {
    (-0.0125 + 0.01 * cos(std::f32::consts::TAU * phase)) * std::f32::consts::PI
}
pub(crate) fn gateway_beam(
    age: u64,
    cooldown: u32,
    partial: f32,
    max_y: i32,
) -> Option<(f32, i32, bool)> {
    let spawning = age < 200;
    if !spawning && cooldown == 0 {
        return None;
    }
    let progress = if spawning {
        ((age as f32 + partial) / 200.).clamp(0., 1.)
    } else {
        1. - ((cooldown as f32 - partial) / 40.).clamp(0., 1.)
    };
    let scale = sin(progress * std::f32::consts::PI);
    Some((
        scale,
        (scale * if spawning { max_y as f32 } else { 50. }).floor() as i32,
        spawning,
    ))
}
