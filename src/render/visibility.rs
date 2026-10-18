use glam::{Mat4, Vec3, Vec4};

pub(super) struct Frustum {
    planes: [Vec4; 6],
}
impl Frustum {
    pub fn new(matrix: Mat4) -> Self {
        let rows = matrix.transpose();
        // wgpu/glam clip depth is 0..w, not OpenGL's -w..w.
        Self {
            planes: [
                rows.w_axis + rows.x_axis,
                rows.w_axis - rows.x_axis,
                rows.w_axis + rows.y_axis,
                rows.w_axis - rows.y_axis,
                rows.z_axis,
                rows.w_axis - rows.z_axis,
            ],
        }
    }
    pub fn intersects(&self, min: Vec3, max: Vec3) -> bool {
        // Invalid source bounds must never make content silently disappear.
        if !min.is_finite() || !max.is_finite() || min.cmpgt(max).any() {
            return true;
        }
        self.planes.iter().all(|p| {
            let positive = Vec3::new(
                if p.x >= 0. { max.x } else { min.x },
                if p.y >= 0. { max.y } else { min.y },
                if p.z >= 0. { max.z } else { min.z },
            );
            p.truncate().dot(positive) + p.w >= -0.01
        })
    }
}

pub(super) fn relative(origin: [f64; 3], eye: [f64; 3]) -> Vec3 {
    Vec3::new(
        (origin[0] - eye[0]) as f32,
        (origin[1] - eye[1]) as f32,
        (origin[2] - eye[2]) as f32,
    )
}

/// Vanilla-style section occlusion graph. Face order matches the mesher
/// (DOWN UP NORTH SOUTH WEST EAST); opposite face is `face ^ 1`. Mask bit
/// `from * 6 + to` allows leaving through `to` after entering via `from`.
/// Missing sections are open; the caller bounds traversal to resident extents
/// so open space cannot walk the flood unbounded. Open (missing/empty)
/// sections connect every face pair; opaque means the mesher-side
/// `terrain::visibility::build` conservative occluder
/// (`solid_render && can_occlude && collision_full`).
pub(super) const ALL_OPEN: u64 = (1u64 << 36) - 1;

const OFFSETS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

static OCCLUSION: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    std::env::var_os("SOLARIS_CLIENT_OCCLUSION").is_none_or(|v| v != "0")
});

/// Same-binary A/B bypass: `SOLARIS_CLIENT_OCCLUSION=0` restores frustum-only
/// culling. Captured once at process startup, like `SOLARIS_CLIENT_GREEDY`.
pub(super) fn occlusion_enabled() -> bool {
    *OCCLUSION
}

/// Resident key extents bounding the flood. `None` with no resident sections.
pub(super) fn extents(keys: impl Iterator<Item = [i32; 3]>) -> Option<([i32; 3], [i32; 3])> {
    let mut min = [i32::MAX; 3];
    let mut max = [i32::MIN; 3];
    let mut any = false;
    for key in keys {
        any = true;
        for axis in 0..3 {
            min[axis] = min[axis].min(key[axis]);
            max[axis] = max[axis].max(key[axis]);
        }
    }
    any.then_some((min, max))
}

/// Flood reachable sections from the camera section key.
///
/// `mask_of` returns the resident connectivity mask; missing sections count
/// as open. Reached solid (zero-connectivity) sections are reported but never
/// expanded through. Returns `false` when the camera sits outside resident
/// bounds; the caller must then stay conservative and draw everything. Scratch
/// buffers are caller-owned for per-frame reuse.
pub(super) fn flood(
    camera: [i32; 3],
    bounds: ([i32; 3], [i32; 3]),
    mut mask_of: impl FnMut([i32; 3]) -> Option<u64>,
    mut in_frustum: impl FnMut([i32; 3]) -> bool,
    reached: &mut std::collections::HashMap<[i32; 3], u16>,
    queue: &mut Vec<([i32; 3], u8)>,
) -> bool {
    reached.clear();
    queue.clear();
    let (min, max) = bounds;
    if (0..3).any(|axis| camera[axis] < min[axis] || camera[axis] > max[axis]) {
        return false;
    }
    // Sparse or far-apart retained chunks must not create an enormous empty
    // traversal box. An unavailable graph falls back to frustum-only drawing.
    let volume = (0..3).try_fold(1_u64, |n, axis| {
        n.checked_mul((i64::from(max[axis]) - i64::from(min[axis]) + 1) as u64)
    });
    if volume.is_none_or(|n| n > 131_072) {
        return false;
    }
    reached.insert(camera, 1 << 6);
    queue.push((camera, 6));
    let mut cursor = 0;
    while cursor < queue.len() {
        let (key, entry) = queue[cursor];
        cursor += 1;
        let seen = reached.get_mut(&key).expect("queued section is reached");
        if *seen >> 8 == 0x3f {
            continue;
        }
        let mask = mask_of(key).unwrap_or(ALL_OPEN);
        let exits = if entry == 6 {
            0x3f
        } else {
            ((mask >> (usize::from(entry) * 6)) & 0x3f) as u16
        };
        // Low bits track incoming faces; high bits track exits already expanded.
        // Open sections often receive all six entries but need only one expansion.
        let new_exits = exits & !(*seen >> 8);
        *seen |= exits << 8;
        for to in 0..6u8 {
            if new_exits & (1 << to) == 0 {
                continue;
            }
            let step = OFFSETS[usize::from(to)];
            let neighbor = [key[0] + step[0], key[1] + step[1], key[2] + step[2]];
            if (0..3).any(|axis| neighbor[axis] < min[axis] || neighbor[axis] > max[axis])
                || !in_frustum(neighbor)
            {
                continue;
            }
            let entry = to ^ 1;
            let seen = reached.entry(neighbor).or_default();
            if *seen & (1 << entry) != 0 {
                continue;
            }
            // A section may be entered through several disconnected components.
            // Visiting it only once can hide a later, valid path through it.
            *seen |= 1 << entry;
            queue.push((neighbor, entry));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_bounds_crossing_near_and_side_planes_remain_visible() {
        let f = Frustum::new(Mat4::perspective_rh(70_f32.to_radians(), 1., 0.05, 128.));
        assert!(f.intersects(Vec3::new(-1., -1., -1.), Vec3::new(1., 1., 0.1)));
        assert!(f.intersects(Vec3::new(0.5, -1., -2.), Vec3::new(3., 1., -1.)));
        assert!(!f.intersects(Vec3::new(20., -1., -2.), Vec3::new(21., 1., -1.)));
        assert!(!f.intersects(Vec3::new(-1., -1., 1.), Vec3::new(1., 1., 2.)));
    }
    #[test]
    fn large_coordinate_rebase_preserves_fractional_camera_translation() {
        for x in [-30_000_000., 30_000_000.] {
            assert_eq!(
                relative([x, 64., x], [x + 0.125, 64.5, x - 0.25]),
                Vec3::new(-0.125, -0.5, 0.25)
            );
        }
    }
    #[test]
    fn flood_marks_solid_surface_but_not_sections_behind_it() {
        let mut reached = std::collections::HashMap::new();
        let mut queue = Vec::new();
        assert!(flood(
            [0, 2, 0],
            ([0, 0, 0], [2, 2, 0]),
            |key| Some(if key[1] == 1 { 0 } else { ALL_OPEN }),
            |_| true,
            &mut reached,
            &mut queue,
        ));
        assert!(reached.contains_key(&[2, 2, 0]));
        assert!(reached.contains_key(&[2, 1, 0]));
        assert!(!reached.contains_key(&[2, 0, 0]));
        assert!(flood(
            [0, 0, 0],
            ([0, 0, 0], [2, 2, 0]),
            |key| Some(if key[1] == 1 { 0 } else { ALL_OPEN }),
            |_| true,
            &mut reached,
            &mut queue,
        ));
        assert!(!reached.contains_key(&[2, 2, 0]));
    }

    #[test]
    fn flood_follows_a_tunnel_in_the_sealed_layer() {
        let mut reached = std::collections::HashMap::new();
        let mut queue = Vec::new();
        assert!(flood(
            [0, 2, 0],
            ([0, 0, 0], [2, 2, 0]),
            |key| Some(if key[1] == 1 && key[0] != 1 {
                0
            } else {
                ALL_OPEN
            }),
            |_| true,
            &mut reached,
            &mut queue,
        ));
        assert!(reached.contains_key(&[2, 0, 0]));
    }

    #[test]
    fn flood_revisits_section_on_a_new_incoming_face() {
        let mut reached = std::collections::HashMap::new();
        let mut queue = Vec::new();
        let target = [1, 0, 0];
        // First arrival is from WEST (blocked). A second path through the
        // section above enters from UP and opens EAST.
        let up_east = (1_u64 << 11) | (1_u64 << 31);
        assert!(flood(
            [0, 0, 0],
            ([0, 0, 0], [2, 1, 0]),
            |key| Some(if key == target { up_east } else { ALL_OPEN }),
            |key| key != [2, 1, 0],
            &mut reached,
            &mut queue,
        ));
        assert!(reached.contains_key(&[2, 0, 0]));
        assert_ne!(reached[&target] & (1 << 1), 0);
        assert_ne!(reached[&target] & (1 << 4), 0);
    }

    #[test]
    fn missing_sections_are_open_and_negative_camera_is_supported() {
        let mut reached = std::collections::HashMap::new();
        let mut queue = Vec::new();
        assert!(flood(
            [-2, 0, 0],
            ([-2, 0, 0], [0, 0, 0]),
            |_| None,
            |_| true,
            &mut reached,
            &mut queue,
        ));
        assert_eq!(reached.len(), 3);
        assert!(!flood(
            [-3, 0, 0],
            ([-2, 0, 0], [0, 0, 0]),
            |_| None,
            |_| true,
            &mut reached,
            &mut queue,
        ));
        assert!(reached.is_empty());
    }

    #[test]
    fn flood_does_not_traverse_outside_frustum() {
        let mut reached = std::collections::HashMap::new();
        let mut queue = Vec::new();
        assert!(flood(
            [0, 0, 0],
            ([0, 0, 0], [2, 0, 0]),
            |_| None,
            |key| key[0] != 1,
            &mut reached,
            &mut queue,
        ));
        assert!(!reached.contains_key(&[1, 0, 0]));
        assert!(!reached.contains_key(&[2, 0, 0]));
    }

    #[test]
    fn sparse_retained_extents_fall_back_conservatively() {
        let mut reached = std::collections::HashMap::new();
        let mut queue = Vec::new();
        assert!(!flood(
            [0, 0, 0],
            ([0, 0, 0], [30_000, 24, 30_000]),
            |_| None,
            |_| true,
            &mut reached,
            &mut queue,
        ));
    }
}
