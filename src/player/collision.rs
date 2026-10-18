//! Shape collision and shape-aware picking; all coordinates stay f64 and block floors use Euclidean semantics.
use crate::protocol::{Result, content::Content, world::World};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
impl Bounds {
    pub fn new(raw: [f64; 6]) -> Self {
        Self {
            min: [raw[0], raw[1], raw[2]],
            max: [raw[3], raw[4], raw[5]],
        }
    }
    pub fn moved(self, delta: [f64; 3]) -> Self {
        Self {
            min: std::array::from_fn(|i| self.min[i] + delta[i]),
            max: std::array::from_fn(|i| self.max[i] + delta[i]),
        }
    }
    pub fn expanded(self, delta: [f64; 3]) -> Self {
        Self {
            min: std::array::from_fn(|i| self.min[i] + delta[i].min(0.)),
            max: std::array::from_fn(|i| self.max[i] + delta[i].max(0.)),
        }
    }
    pub fn intersects(self, other: Self) -> bool {
        (0..3).all(|i| self.max[i] > other.min[i] && self.min[i] < other.max[i])
    }
    pub fn deflated(self, amount: f64) -> Self {
        Self {
            min: self.min.map(|x| x + amount),
            max: self.max.map(|x| x - amount),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Movement {
    pub from: [f64; 3],
    pub to: [f64; 3],
    pub axis_order: Option<[f64; 3]>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct InsideVisit {
    pub position: [i32; 3],
    pub state: u32,
    pub step: usize,
    pub affects_motion: bool,
    pub block_contact: bool,
    pub fluid_contact: bool,
}

#[derive(Default, Debug)]
pub(super) struct InsideBlockState {
    pub powder_snow: bool,
    pub frozen_ticks: u32,
    pub fire_ticks: i32,
}

/// Deferred source effects are deduplicated within each step and applied in enum
/// order. Server-only runBefore/runAfter damage/world mutation is not predicted.
pub(super) fn apply_inside_effects(
    content: &Content,
    visits: &[InsideVisit],
    state: &mut InsideBlockState,
    can_freeze: bool,
    fire_immune: bool,
) -> Result<()> {
    state.powder_snow = false;
    let mut last_step = None;
    let mut mask = 0u8;
    for visit in visits {
        if last_step.is_some_and(|step| step != visit.step) {
            apply_effect_step(state, mask, can_freeze, fire_immune);
            mask = 0;
        }
        last_step = Some(visit.step);
        if visit.block_contact {
            match content.name(visit.state) {
                Some("minecraft:powder_snow") => mask |= 1 | 16,
                Some("minecraft:fire" | "minecraft:soul_fire") => mask |= 2 | 4,
                _ => {}
            }
        }
        if let Some(fluid) = content
            .info(visit.state)?
            .fluid
            .filter(|_| visit.fluid_contact)
        {
            match fluid.kind {
                crate::protocol::content::FluidKind::Water => mask |= 16,
                crate::protocol::content::FluidKind::Lava => mask |= 2 | 8,
            }
        }
    }
    apply_effect_step(state, mask, can_freeze, fire_immune);
    Ok(())
}

fn apply_effect_step(state: &mut InsideBlockState, mask: u8, can_freeze: bool, fire_immune: bool) {
    if mask & 1 != 0 {
        state.powder_snow = true;
        if can_freeze {
            state.frozen_ticks = (state.frozen_ticks + 1).min(140);
        }
    }
    if mask & 2 != 0 {
        state.frozen_ticks = 0;
    }
    if mask & 4 != 0 && !fire_immune {
        if state.fire_ticks < 0 {
            state.fire_ticks += 1;
        }
        if state.fire_ticks >= 0 {
            state.fire_ticks = state.fire_ticks.max(160);
            state.frozen_ticks = 0;
        }
    }
    if mask & 8 != 0 && !fire_immune {
        state.fire_ticks = state.fire_ticks.max(300);
        state.frozen_ticks = 0;
    }
    if mask & 16 != 0 {
        state.fire_ticks = state.fire_ticks.min(0);
    }
}

fn clip_segment(origin: [f64; 3], delta: [f64; 3], bounds: Bounds) -> Option<f64> {
    let mut nearest = 1.;
    let mut hit = false;
    for axis in 0..3 {
        if delta[axis].abs() <= 1e-7 {
            continue;
        }
        let face = if delta[axis] > 0. {
            bounds.min[axis]
        } else {
            bounds.max[axis]
        };
        let t = (face - origin[axis]) / delta[axis];
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        let x = origin[a] + t * delta[a];
        let y = origin[b] + t * delta[b];
        if t > 0.
            && t < nearest
            && x > bounds.min[a] - 1e-7
            && x < bounds.max[a] + 1e-7
            && y > bounds.min[b] - 1e-7
            && y < bounds.max[b] + 1e-7
        {
            nearest = t;
            hit = true;
        }
    }
    hit.then_some(nearest)
}

fn collided_along(body: Bounds, delta: [f64; 3], shape: Bounds) -> bool {
    let center: [f64; 3] = std::array::from_fn(|i| (body.min[i] + body.max[i]) * 0.5);
    let half: [f64; 3] = std::array::from_fn(|i| (body.max[i] - body.min[i]) * 0.5 - 1e-7);
    let inflated = Bounds {
        min: std::array::from_fn(|i| shape.min[i] - half[i]),
        max: std::array::from_fn(|i| shape.max[i] + half[i]),
    };
    let contains =
        |point: [f64; 3]| (0..3).all(|i| point[i] >= inflated.min[i] && point[i] < inflated.max[i]);
    contains(center)
        || contains(std::array::from_fn(|i| center[i] + delta[i]))
        || clip_segment(center, delta, inflated).is_some()
}

fn axis_order(delta: [f64; 3]) -> [usize; 3] {
    if delta[0].abs() < delta[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    }
}

fn visit_corners(
    a: [i32; 3],
    b: [i32; 3],
    delta: [f64; 3],
    mut visit: impl FnMut([i32; 3]) -> bool,
) -> bool {
    let lo: [i32; 3] = std::array::from_fn(|i| a[i].min(b[i]));
    let hi: [i32; 3] = std::array::from_fn(|i| a[i].max(b[i]));
    let start: [i32; 3] = std::array::from_fn(|i| if delta[i] < 0. { hi[i] } else { lo[i] });
    let sign: [i32; 3] = delta.map(|d| if d < 0. { -1 } else { 1 });
    let order = axis_order(delta);
    // BlockPos's directional iterator advances the third (least priority) axis fastest.
    for outer in 0..=hi[order[0]] - lo[order[0]] {
        for middle in 0..=hi[order[1]] - lo[order[1]] {
            for inner in 0..=hi[order[2]] - lo[order[2]] {
                let mut p = start;
                p[order[0]] += outer * sign[order[0]];
                p[order[1]] += middle * sign[order[1]];
                p[order[2]] += inner * sign[order[2]];
                if !visit(p) {
                    return false;
                }
            }
        }
    }
    true
}

/// Pinned BlockGetter traversal: leading-corner DDA, directional volume visits,
/// strict Z/Y/X tie breaking, per-traversal deduplication and endpoint visit.
pub(super) fn swept_inside(
    from: [f64; 3],
    to: [f64; 3],
    end: Bounds,
    seen: &mut Vec<[i32; 3]>,
    mut visit: impl FnMut([i32; 3], usize) -> bool,
) -> bool {
    let delta: [f64; 3] = std::array::from_fn(|i| to[i] - from[i]);
    seen.clear();
    if delta.iter().map(|d| d * d).sum::<f64>() < f64::from(1e-5f32 * 1e-5f32) {
        // betweenClosed uses X fastest, then Y, then Z, unlike directional visits.
        for z in end.min[2].floor() as i32..=end.max[2].floor() as i32 {
            for y in end.min[1].floor() as i32..=end.max[1].floor() as i32 {
                for x in end.min[0].floor() as i32..=end.max[0].floor() as i32 {
                    if !visit([x, y, z], 0) {
                        return false;
                    }
                }
            }
        }
        return true;
    }
    let start = end.moved(delta.map(|d| -d));
    if !visit_corners(
        start.min.map(|d| d.floor() as i32),
        start.max.map(|d| d.floor() as i32),
        delta,
        |p| {
            seen.push(p);
            visit(p, 0)
        },
    ) {
        return false;
    }
    let size: [f64; 3] = std::array::from_fn(|i| end.max[i] - end.min[i]);
    let sign: [i32; 3] = delta.map(|d| if d < 0. { -1 } else { 1 });
    let abs = delta.map(f64::abs);
    // Source selects the corner opposite the least-aligned axis, not simply max.
    let corner = if abs[0] <= abs[1] && abs[0] <= abs[2] {
        [-sign[0], -sign[2], sign[1]]
    } else if abs[1] <= abs[2] {
        [sign[2], -sign[1], -sign[0]]
    } else {
        [-sign[1], sign[0], -sign[2]]
    };
    let tip: [f64; 3] = std::array::from_fn(|i| {
        (end.min[i] + end.max[i]) * 0.5 + size[i] * 0.5 * f64::from(corner[i])
    });
    let origin: [f64; 3] = std::array::from_fn(|i| tip[i] - delta[i]);
    let mut cell = origin.map(|d| d.floor() as i32);
    let step: [i32; 3] = delta.map(|d| {
        if d > 0. {
            1
        } else if d < 0. {
            -1
        } else {
            0
        }
    });
    let interval: [f64; 3] = std::array::from_fn(|i| {
        if step[i] == 0 {
            f64::MAX
        } else {
            f64::from(step[i]) / delta[i]
        }
    });
    let mut next: [f64; 3] = std::array::from_fn(|i| {
        interval[i]
            * if step[i] > 0 {
                1. - (origin[i] - origin[i].floor())
            } else {
                origin[i] - origin[i].floor()
            }
    });
    let mut count = 0;
    while next.iter().any(|t| *t <= 1.) {
        let axis = if next[0] < next[1] {
            if next[0] < next[2] { 0 } else { 2 }
        } else if next[1] < next[2] {
            1
        } else {
            2
        };
        cell[axis] += step[axis];
        next[axis] += interval[axis];
        let block = Bounds {
            min: cell.map(f64::from),
            max: cell.map(|i| f64::from(i) + 1.),
        };
        let Some(t) = clip_segment(origin, delta, block) else {
            continue;
        };
        count += 1;
        let hit: [f64; 3] = std::array::from_fn(|i| {
            (origin[i] + delta[i] * t).clamp(
                block.min[i] + f64::from(1e-5f32),
                block.max[i] - f64::from(1e-5f32),
            )
        });
        let other: [i32; 3] =
            std::array::from_fn(|i| (hit[i] - size[i] * f64::from(corner[i])).floor() as i32);
        if !visit_corners(cell, other, delta, |p| {
            if seen.contains(&p) {
                return true;
            }
            seen.push(p);
            visit(p, count)
        }) {
            return false;
        }
    }
    visit_corners(
        end.min.map(|d| d.floor() as i32),
        end.max.map(|d| d.floor() as i32),
        delta,
        |p| {
            if seen.contains(&p) {
                return true;
            }
            seen.push(p);
            visit(p, count + 1)
        },
    )
}

pub(super) fn collect_inside(
    world: &World,
    content: &Content,
    movements: &[Movement],
    final_bounds: Bounds,
    final_position: [f64; 3],
    context: Context,
    seen: &mut Vec<[i32; 3]>,
    out: &mut Vec<InsideVisit>,
) -> Result<()> {
    out.clear();
    let mut error = None;
    for movement in movements {
        let mut from = movement.from;
        let delta: [f64; 3] = std::array::from_fn(|i| movement.to[i] - from[i]);
        let order = movement
            .axis_order
            .filter(|_| delta.iter().any(|d| *d != 0.))
            .map(axis_order);
        let mut remaining = 16usize;
        let mut segment = |from: [f64; 3], to: [f64; 3], limit: usize| {
            let end = final_bounds
                .moved(std::array::from_fn(|i| to[i] - final_position[i]))
                .deflated(f64::from(1e-5f32));
            let body = final_bounds.moved(std::array::from_fn(|i| from[i] - final_position[i]));
            let travel: [f64; 3] = std::array::from_fn(|i| to[i] - from[i]);
            let long = (0..3).map(|i| (to[i] - from[i]).powi(2)).sum::<f64>()
                > (1. - f64::from(1e-5f32)).powi(2);
            let mut used = 1;
            swept_inside(from, to, end, seen, |position, step| {
                if step >= limit {
                    return false;
                }
                used = step + 1;
                let contact = (|| -> Result<Option<InsideVisit>> {
                    let state = world.block(position).ok_or("unloaded_inside_block")?;
                    if matches!(
                        content.name(state),
                        Some("minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
                    ) {
                        return Ok(None);
                    }
                    let offset = position.map(f64::from);
                    let block = Bounds {
                        min: offset,
                        max: offset.map(|p| p + 1.),
                    };
                    let block_contact = match content.inside_collision(state)? {
                        Some(shapes) => shapes.iter().any(|raw| {
                            *raw == [0., 0., 0., 1., 1., 1.]
                                || collided_along(body, travel, Bounds::new(*raw).moved(offset))
                        }),
                        None if content.name(state) == Some("minecraft:powder_snow") => {
                            // Source returns contextual collision when nonempty,
                            // otherwise Shapes.block. Only a >2.5 fall makes a partial box.
                            context.fall_distance <= 2.5
                                || collided_along(
                                    body,
                                    travel,
                                    Bounds::new([0., 0., 0., 1., 0.9, 1.]).moved(offset),
                                )
                        }
                        None => return Err("unknown_contextual_inside_shape".into()),
                    };
                    let fluid_contact = if let Some(fluid) = content.info(state)?.fluid {
                        let height = super::Player::fluid_height(world, content, position, fluid)?;
                        collided_along(
                            body,
                            travel,
                            Bounds::new([0., 0., 0., 1., height, 1.]).moved(offset),
                        )
                    } else {
                        false
                    };
                    Ok((block_contact || fluid_contact).then_some(InsideVisit {
                        position,
                        state,
                        step,
                        affects_motion: long || end.intersects(block),
                        block_contact,
                        fluid_contact,
                    }))
                })();
                match contact {
                    Ok(Some(visit)) if !out.iter().any(|v| v.position == position) => {
                        out.push(visit)
                    }
                    Ok(_) => {}
                    Err(reason) => {
                        error = Some(reason);
                        return false;
                    }
                }
                true
            });
            used
        };
        if let Some(order) = order {
            for axis in order {
                if delta[axis] == 0. {
                    continue;
                }
                let mut to = from;
                to[axis] += delta[axis];
                remaining = remaining.saturating_sub(segment(from, to, remaining));
                from = to;
            }
        } else {
            remaining = remaining.saturating_sub(segment(from, movement.to, remaining));
        }
        if remaining == 0 {
            segment(movement.to, movement.to, 1);
        }
        if let Some(error) = error {
            return Err(error);
        }
    }
    Ok(())
}

/// EntityCollisionContext uses the entity's Y, not the swept box's bottom.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub bottom: f64,
    pub descending: bool,
    pub fall_distance: f64,
    pub walks_powder_snow: bool,
}

pub(crate) fn climbable(world: &World, content: &Content, position: [f64; 3]) -> Result<bool> {
    let p = position.map(|value| value.floor() as i32);
    let state = world.block(p).ok_or("unloaded_climb_neighborhood")?;
    if content.block_has_tag(state, "minecraft:climbable")? {
        return Ok(true);
    }
    if !content.block_has_tag(state, "minecraft:trapdoors")? {
        return Ok(false);
    }
    let props = content
        .state_properties(state)
        .ok_or("missing_block_properties")?;
    let lower = world
        .block([p[0], p[1] - 1, p[2]])
        .ok_or("unloaded_climb_neighborhood")?;
    Ok(props.get("open").is_some_and(|value| value == "true")
        && content.name(lower) == Some("minecraft:ladder")
        && content
            .state_properties(lower)
            .and_then(|props| props.get("facing"))
            == props.get("facing"))
}

pub fn gather(
    world: &World,
    content: &Content,
    entities: &crate::entities::Store,
    tick: u64,
    bounds: Bounds,
    out: &mut Vec<Bounds>,
) -> Result<()> {
    gather_with_context(world, content, entities, tick, bounds, None, out)
}

pub fn gather_with_context(
    world: &World,
    content: &Content,
    entities: &crate::entities::Store,
    tick: u64,
    bounds: Bounds,
    context: Option<Context>,
    out: &mut Vec<Bounds>,
) -> Result<()> {
    out.clear();
    // Shapes can extend outside their cell (fences/walls); include the adjacent cells.
    let lo = bounds.min.map(|v| v.floor() as i32 - 1);
    let hi = bounds.max.map(|v| v.floor() as i32 + 1);
    let cells = (0..3)
        .try_fold(1usize, |n, i| {
            n.checked_mul((i64::from(hi[i]) - i64::from(lo[i]) + 1) as usize)
        })
        .ok_or("collision_scan_bound")?;
    if cells > 4096 {
        return Err("collision_scan_bound".into());
    }
    for y in lo[1]..=hi[1] {
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                let p = [x, y, z];
                if y < world.min_y || y >= world.min_y + world.height {
                    continue;
                }
                let state = world.block(p).ok_or("unloaded_collision_neighborhood")?;
                if entities.append_block_collision(p, world, content, tick, out)? {
                    continue;
                }
                if matches!(
                    content.name(state),
                    Some("minecraft:powder_snow" | "minecraft:scaffolding")
                ) {
                    let Some(context) = context else { continue };
                    append_contextual(content, state, p, bounds, context, out)?;
                    continue;
                }
                let shapes = content
                    .collision_boxes(state)
                    .ok_or("unknown_collision_shape")?;
                let offset = content.block_offset(state, p);
                for raw in shapes {
                    let shape = Bounds::new(*raw)
                        .moved(std::array::from_fn(|i| f64::from(p[i]) + offset[i]));
                    if shape.intersects(bounds) {
                        if out.len() == 16384 {
                            return Err("collision_shape_bound".into());
                        }
                        out.push(shape);
                    }
                }
            }
        }
    }
    Ok(())
}

fn append_contextual(
    content: &Content,
    state: u32,
    position: [i32; 3],
    area: Bounds,
    context: Context,
    out: &mut Vec<Bounds>,
) -> Result<()> {
    let mut append = |raw| {
        let shape = Bounds::new(raw).moved(position.map(f64::from));
        if shape.intersects(area) {
            out.push(shape);
        }
    };
    // EntityCollisionContext.isAbove uses the widened float epsilon.
    let above = |top: f64| context.bottom > f64::from(position[1]) + top - f64::from(1e-5f32);
    match content.name(state) {
        Some("minecraft:powder_snow") => {
            if context.fall_distance > 2.5 {
                append([0., 0., 0., 1., 0.9, 1.]);
            } else if context.walks_powder_snow && !context.descending && above(1.) {
                append([0., 0., 0., 1., 1., 1.]);
            }
        }
        Some("minecraft:scaffolding") => {
            if above(1.) && !context.descending {
                append([0., 0.875, 0., 1., 1., 1.]);
                for (x, z) in [(0., 0.), (0.875, 0.), (0., 0.875), (0.875, 0.875)] {
                    append([x, 0., z, x + 0.125, 1., z + 0.125]);
                }
            } else {
                let props = content
                    .state_properties(state)
                    .ok_or("missing_scaffolding_properties")?;
                let distance = props
                    .get("distance")
                    .ok_or("missing_scaffolding_distance")?;
                let bottom = props.get("bottom").ok_or("missing_scaffolding_bottom")?;
                if distance != "0" && bottom == "true" && above(0.) {
                    append([0., 0., 0., 1., 0.125, 1.]);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn can_fall_at_least(bounds: Bounds, colliders: &[Bounds], x: f64, z: f64, depth: f64) -> bool {
    let below = Bounds {
        min: [
            bounds.min[0] + 1e-7 + x,
            bounds.min[1] - depth - 1e-7,
            bounds.min[2] + 1e-7 + z,
        ],
        max: [
            bounds.max[0] - 1e-7 + x,
            bounds.min[1],
            bounds.max[2] - 1e-7 + z,
        ],
    };
    !colliders.iter().any(|shape| below.intersects(*shape))
}

pub fn clip_axis(bounds: Bounds, colliders: &[Bounds], axis: usize, mut delta: f64) -> f64 {
    if delta.abs() < 1e-7 {
        return 0.;
    }
    let a = (axis + 1) % 3;
    let b = (axis + 2) % 3;
    for shape in colliders {
        if bounds.max[a] <= shape.min[a]
            || bounds.min[a] >= shape.max[a]
            || bounds.max[b] <= shape.min[b]
            || bounds.min[b] >= shape.max[b]
        {
            continue;
        }
        if delta > 0. && bounds.max[axis] <= shape.min[axis] + 1e-7 {
            delta = delta.min((shape.min[axis] - bounds.max[axis]).max(0.));
        } else if delta < 0. && bounds.min[axis] >= shape.max[axis] - 1e-7 {
            delta = delta.max((shape.max[axis] - bounds.min[axis]).min(0.));
        }
    }
    delta
}

pub fn collide(mut bounds: Bounds, colliders: &[Bounds], wanted: [f64; 3]) -> [f64; 3] {
    let mut moved = wanted;
    moved[1] = clip_axis(bounds, colliders, 1, moved[1]);
    bounds = bounds.moved([0., moved[1], 0.]);
    // Target Entity.collideWithShapes resolves the greater horizontal component first.
    let order = if wanted[0].abs() < wanted[2].abs() {
        [2, 0]
    } else {
        [0, 2]
    };
    for axis in order {
        moved[axis] = clip_axis(bounds, colliders, axis, moved[axis]);
        let mut d = [0.; 3];
        d[axis] = moved[axis];
        bounds = bounds.moved(d);
    }
    moved
}

pub fn with_step(
    bounds: Bounds,
    colliders: &[Bounds],
    wanted: [f64; 3],
    grounded: bool,
    step: f32,
    heights: &mut Vec<f32>,
) -> [f64; 3] {
    let baseline = collide(bounds, colliders, wanted);
    let down = wanted[1] < 0. && wanted[1] != baseline[1];
    if step <= 0. || !(grounded || down) || (wanted[0] == baseline[0] && wanted[2] == baseline[2]) {
        return baseline;
    }
    let base = if down {
        bounds.moved([0., baseline[1], 0.])
    } else {
        bounds
    };
    heights.clear();
    for shape in colliders {
        for y in [shape.min[1], shape.max[1]] {
            let height = (y - base.min[1]) as f32;
            if height >= 0.
                && height <= step
                && height != baseline[1] as f32
                && !heights.contains(&height)
            {
                heights.push(height);
            }
        }
    }
    heights.sort_unstable_by(f32::total_cmp);
    let baseline_horizontal = baseline[0] * baseline[0] + baseline[2] * baseline[2];
    for height in heights.iter().copied() {
        let mut candidate = collide(base, colliders, [wanted[0], f64::from(height), wanted[2]]);
        if candidate[0] * candidate[0] + candidate[2] * candidate[2] > baseline_horizontal {
            candidate[1] -= bounds.min[1] - base.min[1];
            return candidate;
        }
    }
    baseline
}

pub fn ray_box(
    origin: [f64; 3],
    direction: [f64; 3],
    bounds: Bounds,
    reach: f64,
) -> Option<(f64, i32, bool)> {
    let inside = (0..3).all(|i| origin[i] >= bounds.min[i] && origin[i] <= bounds.max[i]);
    let mut near = 0f64;
    let mut far = reach;
    let mut face = 1;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-12 {
            if origin[axis] < bounds.min[axis] || origin[axis] > bounds.max[axis] {
                return None;
            }
            continue;
        }
        let mut a = (bounds.min[axis] - origin[axis]) / direction[axis];
        let mut b = (bounds.max[axis] - origin[axis]) / direction[axis];
        let entry = match (axis, direction[axis] > 0.) {
            (0, true) => 4,
            (0, false) => 5,
            (1, true) => 0,
            (1, false) => 1,
            (2, true) => 2,
            _ => 3,
        };
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if a > near {
            near = a;
            face = entry;
        }
        far = far.min(b);
        if near > far {
            return None;
        }
    }
    if far < 0. || near > reach {
        None
    } else {
        Some((near, face, inside))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(x: f64, y: f64, z: f64) -> Bounds {
        Bounds {
            min: [x - 0.3, y, z - 0.3],
            max: [x + 0.3, y + 1.8, z + 0.3],
        }
    }
    #[test]
    fn swept_inside_crosses_intermediate_blocks_without_visiting_diagonal_void() {
        let mut seen = Vec::new();
        let mut visits = Vec::new();
        let end = body(4.5, 0., 4.5).deflated(f64::from(1e-5f32));
        swept_inside([0.5, 0., 0.5], [4.5, 0., 4.5], end, &mut seen, |p, step| {
            visits.push((p, step));
            true
        });
        assert!(visits.iter().any(|(p, step)| *p == [2, 0, 2] && *step > 0));
        assert!(!visits.iter().any(|(p, _)| *p == [0, 0, 4]));
        assert!(visits.iter().any(|(p, _)| *p == [4, 1, 4]));
        for (index, (p, _)) in visits.iter().enumerate() {
            assert!(!visits[..index].iter().any(|(prior, _)| prior == p));
        }
    }

    #[test]
    fn stationary_inside_deflation_excludes_touching_blocks_and_negative_boundary() {
        let mut seen = Vec::new();
        let mut visits = Vec::new();
        let bounds = Bounds::new([-1., 0., -1., 0., 1., 0.]).deflated(f64::from(1e-5f32));
        swept_inside(
            [-0.5, 0., -0.5],
            [-0.5, 0., -0.5],
            bounds,
            &mut seen,
            |p, step| {
                visits.push((p, step));
                true
            },
        );
        assert_eq!(visits, vec![([-1, 0, -1], 0)]);
    }

    #[test]
    fn deferred_effect_order_extinguishes_same_step_ignition_but_next_step_reignites() {
        let mut state = InsideBlockState {
            frozen_ticks: 139,
            fire_ticks: -1,
            ..InsideBlockState::default()
        };
        apply_effect_step(&mut state, 1 | 2 | 4 | 8 | 16, true, false);
        assert_eq!(state.frozen_ticks, 0);
        assert_eq!(state.fire_ticks, 0);
        assert!(state.powder_snow);
        apply_effect_step(&mut state, 8, true, false);
        assert_eq!(state.fire_ticks, 300);
        state.frozen_ticks = 139;
        apply_effect_step(&mut state, 1, true, true);
        apply_effect_step(&mut state, 1, true, true);
        assert_eq!(state.frozen_ticks, 140);
        apply_effect_step(&mut state, 2 | 8, true, true);
        assert_eq!(state.frozen_ticks, 0);
        assert_eq!(state.fire_ticks, 300);
    }
    #[test]
    fn negative_boundary_partial_shape_does_not_act_as_full_cube() {
        let slab = Bounds::new([-1., 0., -1., 0., 0.5, 0.]);
        assert_eq!(
            collide(body(-0.5, 1., -0.5), &[slab], [0., -2., 0.]),
            [0., -0.5, 0.]
        );
        assert_eq!(
            collide(body(-1.5, 0.6, -0.5), &[slab], [1., 0., 0.]),
            [1., 0., 0.]
        );
    }
    #[test]
    fn step_requires_headroom_and_improves_horizontal_progress() {
        let slab = Bounds::new([1., 0., 0., 2., 0.5, 1.]);
        let start = body(0.5, 0., 0.5);
        let mut heights = Vec::new();
        assert_eq!(
            with_step(start, &[slab], [0.8, -0.08, 0.], true, 0.6, &mut heights),
            [0.8, 0.5, 0.]
        );
        let ceiling = Bounds::new([0., 1.8, 0., 2., 2.8, 1.]);
        assert!(
            with_step(
                start,
                &[slab, ceiling],
                [0.8, -0.08, 0.],
                true,
                0.6,
                &mut heights
            )[0] < 0.8
        );
    }
    #[test]
    fn shape_raycast_rejects_empty_part_and_preserves_negative_face() {
        let slab = Bounds::new([-1., 0., -1., 0., 0.5, 0.]);
        assert_eq!(
            ray_box([-2., 0.25, -0.5], [1., 0., 0.], slab, 4.),
            Some((1., 4, false))
        );
        assert_eq!(ray_box([-2., 0.75, -0.5], [1., 0., 0.], slab, 4.), None);
        assert_eq!(
            ray_box([-0.5, 0.25, -0.5], [1., 0., 0.], slab, 4.),
            Some((0., 1, true))
        );
    }
}
