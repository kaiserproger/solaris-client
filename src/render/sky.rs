use crate::api::{MeshGeometry, Vertex, WorldEnvironment};
use glam::{Mat3, Vec3};

/// World-space source direction matches the celestial Y(-90) * X(angle) basis.
/// The moon takes over below the horizon; neither light follows the camera yaw.
pub(super) fn dominant_light(sun_angle: f32, moon_angle: f32) -> (Vec3, f32) {
    let angle = sun_angle.to_radians();
    let sun = Vec3::new(-angle.sin(), angle.cos(), 0.);
    if sun.y > 0. {
        (sun, 1.)
    } else {
        let moon = moon_angle.to_radians();
        (Vec3::new(-moon.sin(), moon.cos(), 0.), 0.22)
    }
}

// Geometry/seed/count/dimensions observed in the pinned 26.1.2 SkyRenderer
// bytecode. This is independently authored Rust, not distributed Mojang data.
pub(super) fn stars() -> Vec<[f32; 3]> {
    let mut rng = Random((10842u64 ^ 0x5deece66d) & ((1u64 << 48) - 1));
    let mut vertices = Vec::with_capacity(1500 * 6);
    for _ in 0..1500 {
        let point = Vec3::new(
            rng.float() * 2. - 1.,
            rng.float() * 2. - 1.,
            rng.float() * 2. - 1.,
        );
        let radius = 0.15 + rng.float() * 0.1;
        let length = point.length_squared();
        if length <= 0.010000001 || length >= 1. {
            continue;
        }
        let center = point.normalize() * 100.;
        let spin = (rng.double() * f64::from(std::f32::consts::PI) * 2.) as f32;
        let direction = -center.normalize();
        let right = Vec3::Y.cross(direction).normalize();
        let up = direction.cross(right);
        let rotation = Mat3::from_cols(right, up, direction) * Mat3::from_rotation_z(-spin);
        let points = [
            Vec3::new(radius, -radius, 0.),
            Vec3::new(radius, radius, 0.),
            Vec3::new(-radius, radius, 0.),
            Vec3::new(-radius, -radius, 0.),
        ];
        for i in [0, 1, 2, 0, 2, 3] {
            vertices.push((rotation * points[i] + center).to_array());
        }
    }
    vertices
}
struct Random(u64);
impl Random {
    fn bits(&mut self, count: u32) -> u64 {
        self.0 = (self.0.wrapping_mul(0x5deece66d).wrapping_add(11)) & ((1u64 << 48) - 1);
        self.0 >> (48 - count)
    }
    fn float(&mut self) -> f32 {
        self.bits(24) as f32 / 16_777_216.
    }
    fn double(&mut self) -> f64 {
        ((self.bits(26) << 27) + self.bits(27)) as f64 / 9_007_199_254_740_992.
    }
}

pub(super) fn sunrise(mesh: &mut MeshGeometry, env: &WorldEnvironment) {
    mesh.vertices.clear();
    for indices in &mut mesh.indices {
        indices.clear();
    }
    let [r, g, b, alpha] = env.sunrise_color;
    if alpha <= 0.001 {
        return;
    }
    let flip = if env.sun_angle.to_radians().sin() < 0. {
        std::f32::consts::PI
    } else {
        0.
    };
    let rotation = Mat3::from_rotation_x(std::f32::consts::FRAC_PI_2)
        * Mat3::from_rotation_z(flip + std::f32::consts::FRAC_PI_2);
    let vertex = |point: Vec3, opacity: f32| Vertex {
        position: (rotation * (point * Vec3::new(1., 1., alpha))).to_array(),
        color: [r, g, b],
        uv: [-1.; 2],
        overlay_uv: [-1.; 2],
        overlay_tint: [1.; 3],
        alpha: opacity * alpha,
        texture_layer: 0,
        light: [-1.; 2],
        normal: [0.; 3],
    };
    mesh.vertices.push(vertex(Vec3::new(0., 100., 0.), 1.));
    for i in 0..=16 {
        let angle = i as f32 * std::f32::consts::TAU / 16.;
        mesh.vertices.push(vertex(
            Vec3::new(angle.sin() * 120., angle.cos() * 120., -angle.cos() * 40.),
            0.,
        ));
    }
    for i in 1..=16 {
        mesh.indices[0].extend_from_slice(&[0, i, i + 1]);
    }
}
