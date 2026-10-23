//! Presentation-only block animations: placement pop-in and door/trapdoor
//! swing. The animated block is meshed as air (Terrain::hide_block) and drawn
//! through the terrain mesher's moving-block path, so lighting/AO match the
//! section mesh. Frame-time based; never touches collision, raycasts or packets.
//! Falling block entities use the same moving-block path.
use crate::{
    api::DynamicGeometry,
    motion,
    protocol::{content::Content, world::World, Result},
    terrain::Terrain,
};
use glam::{Mat4, Vec3};

const MAX_ACTIVE: usize = 24;
const RANGE: f64 = 40.;
pub(super) const POP_SECONDS: f64 = 0.11;
pub(super) const SWING_SECONDS: f64 = 0.2;
/// A section that never settles (starved queue) must not pin a block hidden.
const SETTLE_TIMEOUT: f64 = 1.;

type Box3 = ([f32; 3], [f32; 3]);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    Pop,
    /// Rigid swing: `angle` maps the old pose onto the new one about the axis
    /// through `pivot`.
    Hinge {
        pivot: [f32; 3],
        axis: usize,
        angle: f32,
    },
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Overlay holds the start pose until the section mesh stops showing the block.
    Hiding(f64),
    Playing(f64),
    /// Overlay holds the end pose until the section mesh shows the block again.
    Revealing(f64),
}
struct Motion {
    position: [i32; 3],
    state: u32,
    kind: Kind,
    phase: Phase,
}
#[derive(Default)]
pub(super) struct BlockMotions {
    active: Vec<Motion>,
}

/// Local-space transform of the overlay at `progress` (0 = old pose, 1 = new).
pub(super) fn pose(kind: Kind, progress: f32, age: f32) -> Mat4 {
    match kind {
        Kind::Pop => {
            let scale = motion::pop_scale(age, POP_SECONDS as f32);
            Mat4::from_translation(Vec3::new(0.5, 0., 0.5))
                * Mat4::from_scale(Vec3::splat(scale))
                * Mat4::from_translation(Vec3::new(-0.5, 0., -0.5))
        }
        Kind::Hinge { pivot, axis, angle } => {
            let pivot = Vec3::from_array(pivot);
            let mut direction = Vec3::ZERO;
            direction[axis] = 1.;
            Mat4::from_translation(pivot)
                * Mat4::from_axis_angle(direction, -angle * (1. - progress))
                * Mat4::from_translation(-pivot)
        }
    }
}

/// Find the single axis-aligned 90 degree rotation that carries box `a` onto
/// box `b`. The hinge post is the candidate pivot farthest from the block
/// centre (a door leaf swings about its post, not about the block middle).
pub(super) fn solve_hinge(a: Box3, b: Box3) -> Option<Kind> {
    const TOLERANCE: f32 = 0.01;
    let centre = |bounds: Box3, i: usize| (bounds.0[i] + bounds.1[i]) / 2.;
    let half = |bounds: Box3, i: usize| (bounds.1[i] - bounds.0[i]) / 2.;
    let mut best: Option<(f32, Kind)> = None;
    // (axis, u, v) is a right-handed cycle: +90 degrees maps u onto v.
    for (axis, u, v) in [(1, 2, 0), (0, 1, 2), (2, 0, 1)] {
        let aligned = (centre(a, axis) - centre(b, axis)).abs() < TOLERANCE
            && (half(a, axis) - half(b, axis)).abs() < TOLERANCE;
        let swapped = (half(a, u) - half(b, v)).abs() < TOLERANCE
            && (half(a, v) - half(b, u)).abs() < TOLERANCE;
        let unchanged = (0..3).all(|i| {
            (centre(a, i) - centre(b, i)).abs() < TOLERANCE
                && (half(a, i) - half(b, i)).abs() < TOLERANCE
        });
        if !aligned || !swapped || unchanged {
            continue;
        }
        let (cu, cv, nu, nv) = (centre(a, u), centre(a, v), centre(b, u), centre(b, v));
        for s in [1f32, -1.] {
            let pu = (cu + nu + s * (cv - nv)) / 2.;
            let pv = (cv + nv + s * (nu - cu)) / 2.;
            if !(-0.05..=1.05).contains(&pu) || !(-0.05..=1.05).contains(&pv) {
                continue;
            }
            let mut pivot = [0.5; 3];
            pivot[u] = pu;
            pivot[v] = pv;
            let reach = (pu - 0.5).abs().max((pv - 0.5).abs());
            if best.is_none_or(|(far, _)| reach > far) {
                best = Some((
                    reach,
                    Kind::Hinge {
                        pivot,
                        axis,
                        angle: s * std::f32::consts::FRAC_PI_2,
                    },
                ));
            }
        }
    }
    best.map(|(_, kind)| kind)
}

fn model_box(content: &Content, state: u32) -> Option<Box3> {
    let model = content.model_for(state).ok()??;
    let mut bounds: Option<Box3> = None;
    model.for_each_quad([0; 3], |quad| {
        for p in quad.positions {
            let (min, max) = bounds.get_or_insert((p, p));
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
    });
    bounds
}

fn open_flag(content: &Content, state: u32) -> Option<bool> {
    Some(content.state_properties(state)?.get("open")? == "true")
}

/// Classify a world change into an animation, or `None` for an ordinary snap.
pub(super) fn classify(content: &Content, old: Option<u32>, new: u32) -> Option<Kind> {
    let new_info = content.info(new).ok()?;
    if new_info.is_air || new_info.fluid.is_some() || new_info.model.is_none() {
        return None;
    }
    let old = old?;
    let old_info = content.info(old).ok()?;
    if old_info.block == new_info.block {
        let name = content.name(new)?;
        if !(name.ends_with("_door") || name.ends_with("_trapdoor")) {
            return None;
        }
        if open_flag(content, old)? == open_flag(content, new)? {
            return None;
        }
        return solve_hinge(model_box(content, old)?, model_box(content, new)?);
    }
    (old_info.is_air || old_info.fluid.is_some()).then_some(Kind::Pop)
}

impl BlockMotions {
    pub(super) fn clear(&mut self) {
        self.active.clear();
    }
    pub(super) fn len(&self) -> usize {
        self.active.len()
    }
    /// A block just changed in the world. Cancels any animation for the same cell.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn changed(
        &mut self,
        terrain: &mut Terrain,
        world: &World,
        content: &Content,
        camera: [f64; 3],
        now: f64,
        position: [i32; 3],
        old: Option<u32>,
        new: u32,
    ) -> Result<()> {
        if let Some(index) = self.active.iter().position(|m| m.position == position) {
            self.active.swap_remove(index);
            terrain.unhide_block(world, position)?;
        }
        let near = (0..3).all(|i| (f64::from(position[i]) + 0.5 - camera[i]).abs() < RANGE);
        if !near || self.active.len() >= MAX_ACTIVE {
            return Ok(());
        }
        let Some(kind) = classify(content, old, new) else {
            return Ok(());
        };
        terrain.hide_block(world, position)?;
        self.active.push(Motion {
            position,
            state: new,
            kind,
            phase: Phase::Hiding(now),
        });
        Ok(())
    }
    /// Advance phases once per frame; mesh settling decides hand-over moments.
    pub(super) fn step(&mut self, terrain: &mut Terrain, world: &World, now: f64) -> Result<()> {
        let mut index = 0;
        while index < self.active.len() {
            let motion = &mut self.active[index];
            let done = match motion.phase {
                Phase::Hiding(since) => {
                    if terrain.block_settled(motion.position) {
                        motion.phase = Phase::Playing(now);
                    } else if now - since > SETTLE_TIMEOUT {
                        terrain.unhide_block(world, motion.position)?;
                        self.active.swap_remove(index);
                        continue;
                    }
                    false
                }
                Phase::Playing(since) => {
                    if now - since >= duration(motion.kind) {
                        terrain.unhide_block(world, motion.position)?;
                        motion.phase = Phase::Revealing(now);
                    }
                    false
                }
                Phase::Revealing(since) => {
                    terrain.block_settled(motion.position) || now - since > SETTLE_TIMEOUT
                }
            };
            if done {
                self.active.swap_remove(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }
    pub(super) fn append(
        &self,
        terrain: &Terrain,
        world: &World,
        origin: [f64; 3],
        now: f64,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        for motion in &self.active {
            let (progress, age) = match motion.phase {
                Phase::Hiding(_) => (0., 0.),
                Phase::Playing(since) => {
                    let age = (now - since).max(0.);
                    (
                        motion::ease_in_out_cubic((age / duration(motion.kind)) as f32),
                        age as f32,
                    )
                }
                Phase::Revealing(_) => (1., duration(motion.kind) as f32),
            };
            let at = Vec3::from_array(std::array::from_fn(|i| {
                (f64::from(motion.position[i]) - origin[i]) as f32
            }));
            let transform = Mat4::from_translation(at) * pose(motion.kind, progress, age);
            terrain.append_moving_block(world, motion.state, motion.position, transform, out)?;
        }
        Ok(())
    }
}

/// FallingBlockRenderer: the block model sits at the entity's feet (-0.5 on x/z),
/// lit from the cell holding the box top. A copy of the block already standing
/// in the entity's own cell is skipped, as on the landing tick.
pub(super) fn append_falling(
    terrain: &Terrain,
    world: &World,
    falling: &[(u32, [f64; 3])],
    origin: [f64; 3],
    out: &mut DynamicGeometry,
) -> Result<()> {
    for &(state, [x, y, z]) in falling {
        let cell = [x, y, z].map(|v| v.floor() as i32);
        if world.block(cell) == Some(state) {
            continue;
        }
        let at = Vec3::new(
            (x - 0.5 - origin[0]) as f32,
            (y - origin[1]) as f32,
            (z - 0.5 - origin[2]) as f32,
        );
        let light_cell = [cell[0], (y + 0.98).floor() as i32, cell[2]];
        terrain.append_moving_block(world, state, light_cell, Mat4::from_translation(at), out)?;
    }
    Ok(())
}

fn duration(kind: Kind) -> f64 {
    match kind {
        Kind::Pop => POP_SECONDS,
        Kind::Hinge { .. } => SWING_SECONDS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) {
        assert!((a - b).length() < 1e-4, "{a:?} != {b:?}");
    }
    // Vanilla door leaf: 3/16 thick slab on the south edge, then the same leaf
    // swung open onto the west edge.
    const CLOSED: Box3 = ([0., 0., 0.8125], [1., 1., 1.]);
    const OPEN: Box3 = ([0., 0., 0.], [0.1875, 1., 1.]);

    #[test]
    fn door_hinge_solves_to_the_hinge_post_and_maps_old_box_onto_new() {
        let Some(Kind::Hinge { pivot, axis, angle }) = solve_hinge(CLOSED, OPEN) else {
            panic!("door must solve");
        };
        assert_eq!(axis, 1);
        assert!((pivot[0] - 0.09375).abs() < 1e-4 && (pivot[2] - 0.90625).abs() < 1e-4);
        assert!((angle.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        // Applying the full rotation to the old box corners lands inside the new box.
        let full = Mat4::from_translation(Vec3::from_array(pivot))
            * Mat4::from_axis_angle(Vec3::Y, angle)
            * Mat4::from_translation(-Vec3::from_array(pivot));
        for x in [CLOSED.0[0], CLOSED.1[0]] {
            for z in [CLOSED.0[2], CLOSED.1[2]] {
                let p = full.transform_point3(Vec3::new(x, 0.5, z));
                assert!(p.x >= -1e-4 && p.x <= 0.1875 + 1e-4, "{p:?}");
                assert!(p.z >= -1e-4 && p.z <= 1. + 1e-4, "{p:?}");
            }
        }
    }

    #[test]
    fn pose_starts_at_old_pose_and_ends_at_identity() {
        let kind = solve_hinge(CLOSED, OPEN).unwrap();
        assert_eq!(pose(kind, 1., 0.), Mat4::IDENTITY);
        let start = pose(kind, 0., 0.);
        // The new (open) leaf rotated back by the full angle covers the old closed box.
        let p = start.transform_point3(Vec3::new(0.09375, 0.5, 0.0));
        assert!(p.z > 0.8 && p.x > 0.4, "{p:?}");
        let mid = pose(kind, 0.5, 0.);
        assert_ne!(mid, start);
        assert_ne!(mid, Mat4::IDENTITY);
    }

    #[test]
    fn trapdoor_hinges_on_its_edge_about_a_horizontal_axis() {
        let closed: Box3 = ([0., 0., 0.], [1., 0.1875, 1.]);
        let open: Box3 = ([0., 0., 0.8125], [1., 1., 1.]);
        let Some(Kind::Hinge { pivot, axis, .. }) = solve_hinge(closed, open) else {
            panic!("trapdoor must solve");
        };
        assert_eq!(axis, 0);
        assert!((pivot[1] - 0.09375).abs() < 1e-4 && (pivot[2] - 0.90625).abs() < 1e-4);
    }

    #[test]
    fn non_rigid_or_unchanged_shapes_do_not_animate() {
        assert!(solve_hinge(CLOSED, CLOSED).is_none());
        let wide: Box3 = ([0., 0., 0.], [1., 1., 1.]);
        assert!(solve_hinge(CLOSED, wide).is_none());
        // Fence-gate-like frame: union boxes differ in height as well.
        let tall: Box3 = ([0., 0., 0.], [0.1875, 0.5, 1.]);
        assert!(solve_hinge(CLOSED, tall).is_none());
    }

    #[test]
    fn pop_pose_grows_from_the_floor_centre_and_lands_on_identity() {
        let start = pose(Kind::Pop, 0., 0.);
        close(
            start.transform_point3(Vec3::new(0.5, 0., 0.5)),
            Vec3::new(0.5, 0., 0.5),
        );
        let corner = start.transform_point3(Vec3::new(1., 1., 1.));
        assert!(corner.x < 0.9 && corner.y < 0.8);
        assert_eq!(pose(Kind::Pop, 1., POP_SECONDS as f32), Mat4::IDENTITY);
    }
}
