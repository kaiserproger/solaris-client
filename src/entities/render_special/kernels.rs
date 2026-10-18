//! Numeric presentation kernels for the pinned 26.1.2 renderer contracts.
//! Geometry is emitted by the shared presentation engine, never by a second mesh path.
use crate::protocol::models::JavaRandom;
use glam::{Mat4, Vec3};

pub(super) fn tnt_scale(fuse: f32) -> f32 {
    let urgency = (1. - fuse / 10.).clamp(0., 1.);
    let square = urgency * urgency;
    1. + square * square * 0.3
}
pub(super) fn tnt_matrix(fuse: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(0., 0.5, 0.))
        * Mat4::from_scale(Vec3::splat(tnt_scale(fuse)))
        * Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_translation(Vec3::new(-0.5, -0.5, 0.5))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2)
}
pub(super) fn billboard(rotation: [f32; 2]) -> Mat4 {
    Mat4::from_rotation_y((180. - rotation[0]).to_radians())
        * Mat4::from_rotation_x(-rotation[1].to_radians())
}
pub(super) fn paddle(time: f32, right: bool) -> [f32; 2] {
    let x = super::source_sin(-time) * 0.5 + 0.5;
    let y = super::source_sin(-time + 1.) * 0.5 + 0.5;
    let yaw = -std::f32::consts::FRAC_PI_4 + y * std::f32::consts::FRAC_PI_2;
    [
        -1.0471976 + x * (-0.2617994 + 1.0471976),
        if right {
            std::f32::consts::PI - yaw
        } else {
            yaw
        },
    ]
}
pub(super) fn orb_icon(value: i32) -> u8 {
    for (threshold, icon) in [
        (2477, 10),
        (1237, 9),
        (617, 8),
        (307, 7),
        (149, 6),
        (73, 5),
        (37, 4),
        (17, 3),
        (7, 2),
        (3, 1),
    ] {
        if value >= threshold {
            return icon;
        }
    }
    0
}
pub(super) fn cart_jitter(id: i32) -> Vec3 {
    let seed = i64::from(id).wrapping_mul(493286711);
    let seed = seed
        .wrapping_mul(seed)
        .wrapping_mul(4392167121)
        .wrapping_add(seed.wrapping_mul(98761));
    Vec3::from_array(
        [16, 20, 24].map(|shift| ((((seed >> shift) & 7) as f32 + 0.5) / 8. - 0.5) * 0.004),
    )
}
pub(super) fn line_point(delta: Vec3, progress: f32) -> Vec3 {
    Vec3::new(
        delta.x * progress,
        delta.y * (progress * progress + progress) * 0.5 + 0.25,
        delta.z * progress,
    )
}

pub(super) fn item_amount(count: i32) -> usize {
    if count <= 1 {
        1
    } else if count <= 16 {
        2
    } else if count <= 32 {
        3
    } else if count <= 48 {
        4
    } else {
        5
    }
}
pub(super) fn cluster_offsets(seed: i32, count: i32, depth: f32) -> [Vec3; 5] {
    let amount = item_amount(count);
    let mut points = [Vec3::ZERO; 5];
    let mut random = JavaRandom::new(i64::from(seed));
    if depth > 0.0625 {
        for point in &mut points[1..amount] {
            *point = Vec3::from_array(std::array::from_fn(|_| {
                (random.next_float() * 2. - 1.) * 0.15
            }));
        }
    } else {
        let spacing = depth * 1.5;
        let first = -spacing * (amount - 1) as f32 * 0.5;
        points[0].z = first;
        for (index, point) in points.iter_mut().enumerate().take(amount).skip(1) {
            point.x = (random.next_float() * 2. - 1.) * 0.15 * 0.5;
            point.y = (random.next_float() * 2. - 1.) * 0.15 * 0.5;
            point.z = first + index as f32 * spacing;
        }
    }
    points
}
pub(super) fn lightning(seed: i64, mut quad: impl FnMut([[f32; 3]; 4])) {
    let mut offsets = [[0.; 2]; 8];
    let mut offset = [0.; 2];
    let mut random = JavaRandom::new(seed);
    for h in (0..8).rev() {
        offsets[h] = offset;
        for coordinate in &mut offset {
            *coordinate += (random.next_int(11) as i32 - 5) as f32;
        }
    }
    for shell in 0..4 {
        let mut random = JavaRandom::new(seed);
        for branch in 0..3 {
            let start = if branch == 0 { 7 } else { 7 - branch };
            let end = if branch == 0 { 0 } else { start - 2 };
            let mut lower = [offsets[start][0] - offset[0], offsets[start][1] - offset[1]];
            for h in (end..=start).rev() {
                let upper = lower;
                for coordinate in &mut lower {
                    *coordinate += if branch == 0 {
                        random.next_int(11) as i32 - 5
                    } else {
                        random.next_int(31) as i32 - 15
                    } as f32;
                }
                let thickness = 0.1 + shell as f32 * 0.2;
                let top = thickness * if branch == 0 { h as f32 * 0.1 + 1. } else { 1. };
                let bottom = thickness
                    * if branch == 0 {
                        (h as f32 - 1.) * 0.1 + 1.
                    } else {
                        1.
                    };
                for (a, b) in [
                    ([-1., -1.], [1., -1.]),
                    ([1., -1.], [1., 1.]),
                    ([1., 1.], [-1., 1.]),
                    ([-1., 1.], [-1., -1.]),
                ] {
                    quad([
                        [
                            lower[0] + a[0] * bottom,
                            h as f32 * 16.,
                            lower[1] + a[1] * bottom,
                        ],
                        [
                            upper[0] + a[0] * top,
                            (h + 1) as f32 * 16.,
                            upper[1] + a[1] * top,
                        ],
                        [
                            upper[0] + b[0] * top,
                            (h + 1) as f32 * 16.,
                            upper[1] + b[1] * top,
                        ],
                        [
                            lower[0] + b[0] * bottom,
                            h as f32 * 16.,
                            lower[1] + b[1] * bottom,
                        ],
                    ]);
                }
            }
        }
    }
}
