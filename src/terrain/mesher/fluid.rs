use super::{add, append_quad, greedy::Greedy, quantize, Rules, Samples, NORMALS};
use crate::{
    api::{MeshGeometry, RenderLayer, Vertex},
    protocol::{
        content::{FluidKind, FluidState, TextureRegion},
        models::{
            fluids::{self, FluidSample},
            shapes::rectangles_cover,
        },
        Result,
    },
};

fn sample(samples: &Samples<'_>, position: [i32; 3]) -> Result<FluidSample> {
    let state = samples.catalog.state(samples.state(position)?)?;
    Ok(FluidSample {
        fluid: state.fluid,
        blocks_motion: state.fact.solid,
    })
}
fn same(samples: &Samples<'_>, position: [i32; 3], fluid: FluidState) -> Result<bool> {
    Ok(sample(samples, position)?
        .fluid
        .is_some_and(|f| f.kind == fluid.kind))
}
fn occluded(samples: &Samples<'_>, state: u32, face: usize, height: f32) -> Result<bool> {
    let target = &samples.catalog.state(state)?.fact.faces[face ^ 1];
    if face == 1 && height < 1. {
        return Ok(false);
    }
    let rectangle = if face <= 1 {
        [0., 0., 1., 1.]
    } else {
        [0., 0., 1., f64::from(height)]
    };
    Ok(rectangles_cover(target, &[rectangle]))
}
fn light(samples: &mut Samples<'_>, position: [i32; 3]) -> Result<[f32; 2]> {
    let own = samples.light(position)?;
    let above = samples.light(add(position, [0, 1, 0]))?;
    Ok(if own == [-1.; 2] || above == [-1.; 2] {
        [-1.; 2]
    } else {
        std::array::from_fn(|i| own[i].max(above[i]))
    })
}
fn uv(sprite: TextureRegion, coord: [f32; 2]) -> [f32; 2] {
    [
        sprite.uv[0] + (sprite.uv[2] - sprite.uv[0]) * coord[0],
        sprite.uv[1] + (sprite.uv[3] - sprite.uv[1]) * coord[1],
    ]
}
fn vertices(
    points: [[f32; 3]; 4],
    coords: [[f32; 2]; 4],
    origin: [f32; 3],
    sprite: TextureRegion,
    color: [f32; 3],
    light: [f32; 2],
    normal: [f32; 3],
) -> [Vertex; 4] {
    std::array::from_fn(|i| Vertex {
        position: std::array::from_fn(|axis| points[i][axis] + origin[axis]),
        color,
        uv: uv(sprite, coords[i]),
        overlay_uv: [-1.; 2],
        overlay_tint: [1.; 3],
        alpha: 1.,
        texture_layer: sprite.layer,
        light,
        normal,
    })
}
/// WATER-only tag (length 4, real direction preserved): lava falls through as
/// unit normals (class 1). Side faces derive from the cardinal face normal;
/// top/bottom stay axis normals.
fn tag(fluid: FluidState, normal: [f32; 3]) -> [f32; 3] {
    if fluid.kind == FluidKind::Water {
        normal.map(|v| v * 4.)
    } else {
        normal
    }
}
pub(super) fn emit(
    samples: &mut Samples<'_>,
    state: u32,
    position: [i32; 3],
    origin: [f32; 3],
    fluid: FluidState,
    rules: Rules,
    geometry: &mut MeshGeometry,
    flat_tops: &mut Greedy,
    byte_limit: usize,
) -> Result<()> {
    let textures = samples.catalog.fluids[if fluid.kind == FluidKind::Water { 0 } else { 1 }];
    let tint = samples.tint(textures.tint, position, rules.biome_blend_radius)?;
    let mut heights = fluids::corner_heights(position, fluid.kind, |p| sample(samples, p))?;
    let above = add(position, [0, 1, 0]);
    let below = add(position, [0, -1, 0]);
    let bottom = !same(samples, below, fluid)?
        && !occluded(samples, state, 1, 1.)?
        && !occluded(samples, samples.state(below)?, 0, 0.8888889)?;
    let bottom_height = if bottom { 0.001 } else { 0. };
    if !same(samples, above, fluid)?
        && !occluded(
            samples,
            samples.state(above)?,
            1,
            heights.into_iter().fold(1., f32::min),
        )?
    {
        for height in &mut heights {
            *height -= 0.001;
        }
        let flow = fluids::flow(
            position,
            fluid,
            |p| sample(samples, p),
            |p, face| {
                let neighbor = samples.catalog.state(samples.state(p)?)?;
                Ok(if neighbor.fluid.is_some_and(|f| f.kind == fluid.kind) {
                    false
                } else if face == 1 {
                    true
                } else {
                    !neighbor.fact.is_ice_block && neighbor.fact.sturdy_faces[face as usize]
                })
            },
        )?;
        let (sprite, coords) = if flow[0] == 0. && flow[2] == 0. {
            (textures.still, [[0., 0.], [0., 1.], [1., 1.], [1., 0.]])
        } else {
            let angle =
                crate::player::math::atan2(flow[2], flow[0]) as f32 - std::f32::consts::FRAC_PI_2;
            let s = crate::player::math::sin(f64::from(angle)) * 0.25;
            let c = crate::player::math::cos(f64::from(angle)) * 0.25;
            (
                textures.flow,
                [
                    [0.5 - c - s, 0.5 - c + s],
                    [0.5 - c + s, 0.5 + c + s],
                    [0.5 + c + s, 0.5 + c - s],
                    [0.5 + c - s, 0.5 - c - s],
                ],
            )
        };
        // FluidState's own predicate (not a guessed FlowingFluid override): the
        // top is double-sided iff the 3x3 ABOVE layer contains a non-solid,
        // different-fluid cell. Original 26.1.2 javap retained under .analysis.
        let mut backward = false;
        for x in -1..=1 {
            for z in -1..=1 {
                let neighbor = samples
                    .catalog
                    .state(samples.state(add(above, [x, 0, z]))?)?;
                backward |= !neighbor.fluid.is_some_and(|f| f.kind == fluid.kind)
                    && !neighbor.fact.solid_render;
            }
        }
        let lit = light(samples, position)?;
        let top = vertices(
            [
                [0., heights[0], 0.],
                [0., heights[1], 1.],
                [1., heights[2], 1.],
                [1., heights[3], 0.],
            ],
            coords,
            origin,
            sprite,
            quantize(tint, rules.directional_shade[1]),
            lit,
            tag(fluid, [0., 1., 0.]),
        );
        // Flat still tops of equal light/tint merge into tiled rectangles; the
        // water shader works per fragment, so no vertex detail is lost.
        if textures.layer != RenderLayer::Translucent
            || !flat_tops.push_sided(top, origin.map(|v| v as i32), 1, backward)
        {
            append_quad(geometry, top, textures.layer, backward, byte_limit)?;
        }
    }
    if bottom {
        let lit = light(samples, below)?;
        append_quad(
            geometry,
            vertices(
                [
                    [0., bottom_height, 0.],
                    [1., bottom_height, 0.],
                    [1., bottom_height, 1.],
                    [0., bottom_height, 1.],
                ],
                [[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
                origin,
                textures.still,
                quantize(tint, rules.directional_shade[0]),
                lit,
                tag(fluid, [0., -1., 0.]),
            ),
            textures.layer,
            false,
            byte_limit,
        )?;
    }
    let lit = light(samples, position)?;
    for face in [2usize, 3, 4, 5] {
        let p = add(position, NORMALS[face]);
        let neighbor_id = samples.state(p)?;
        if same(samples, p, fluid)? || occluded(samples, state, face ^ 1, 1.)? {
            continue;
        }
        let (a, b, start, end) = match face {
            2 => (heights[0], heights[3], [0., 0.001], [1., 0.001]),
            3 => (heights[2], heights[1], [1., 0.999], [0., 0.999]),
            4 => (heights[1], heights[0], [0.001, 1.], [0.001, 0.]),
            _ => (heights[3], heights[2], [0.999, 0.], [0.999, 1.]),
        };
        if occluded(samples, neighbor_id, face, a.max(b))? {
            continue;
        }
        let overlay = textures
            .overlay
            .filter(|_| samples.catalog.states[neighbor_id as usize].fluid_overlay);
        let sprite = overlay.unwrap_or(textures.flow);
        let shade =
            rules.directional_shade[1] * rules.directional_shade[if face <= 3 { 2 } else { 4 }];
        append_quad(
            geometry,
            vertices(
                [
                    [start[0], a, start[1]],
                    [end[0], b, end[1]],
                    [end[0], bottom_height, end[1]],
                    [start[0], bottom_height, start[1]],
                ],
                [
                    [0., (1. - a) * 0.5],
                    [0.5, (1. - b) * 0.5],
                    [0.5, 0.5],
                    [0., 0.5],
                ],
                origin,
                sprite,
                quantize(tint, shade),
                lit,
                tag(fluid, NORMALS[face].map(|v| v as f32)),
            ),
            textures.layer,
            overlay.is_none(),
            byte_limit,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_tags_preserve_all_face_directions_and_lava_stays_unit() {
        for kind in [FluidKind::Water, FluidKind::Lava] {
            let fluid = FluidState {
                kind,
                amount: 8,
                falling: false,
                waterlogged: false,
            };
            let expected = if kind == FluidKind::Water { 4. } else { 1. };
            for normal in NORMALS.map(|normal| normal.map(|component| component as f32)) {
                let tagged = tag(fluid, normal);
                let magnitude = tagged.iter().map(|v| v * v).sum::<f32>().sqrt();
                assert_eq!(magnitude, expected);
                assert_eq!(tagged.map(|v| v / magnitude), normal);
            }
        }
    }
}
