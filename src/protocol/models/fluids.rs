//! FluidRenderer weighted corners and FlowingFluid flow, independent of world storage.
use super::super::{
    Result,
    content::{FluidKind, FluidState},
};
#[derive(Clone, Copy, Debug)]
pub struct FluidSample {
    pub fluid: Option<FluidState>,
    pub blocks_motion: bool,
}
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn height(
    position: [i32; 3],
    kind: FluidKind,
    sample: &mut impl FnMut([i32; 3]) -> Result<FluidSample>,
) -> Result<f32> {
    let value = sample(position)?;
    if let Some(fluid) = value.fluid.filter(|f| f.kind == kind) {
        if sample(add(position, [0, 1, 0]))?
            .fluid
            .is_some_and(|f| f.kind == kind)
        {
            return Ok(1.);
        }
        return Ok(fluid.height());
    }
    Ok(if value.blocks_motion { -1. } else { 0. })
}
/// NW, SW, SE, NE heights, matching the original top quad's vertex order.
pub fn corner_heights(
    position: [i32; 3],
    kind: FluidKind,
    mut sample: impl FnMut([i32; 3]) -> Result<FluidSample>,
) -> Result<[f32; 4]> {
    let center = height(position, kind, &mut sample)?;
    if center >= 1. {
        return Ok([1.; 4]);
    }
    let north = height(add(position, [0, 0, -1]), kind, &mut sample)?;
    let south = height(add(position, [0, 0, 1]), kind, &mut sample)?;
    let west = height(add(position, [-1, 0, 0]), kind, &mut sample)?;
    let east = height(add(position, [1, 0, 0]), kind, &mut sample)?;
    let mut out = [0.; 4];
    for (index, (a, b, offset)) in [
        (north, west, [-1, 0, -1]),
        (south, west, [-1, 0, 1]),
        (south, east, [1, 0, 1]),
        (north, east, [1, 0, -1]),
    ]
    .into_iter()
    .enumerate()
    {
        if a >= 1. || b >= 1. {
            out[index] = 1.;
            continue;
        }
        let diagonal = if a > 0. || b > 0. {
            height(add(position, offset), kind, &mut sample)?
        } else {
            -1.
        };
        if diagonal >= 1. {
            out[index] = 1.;
            continue;
        }
        let (mut total, mut weight) = (0f32, 0f32);
        // Original accumulation order: diagonal, center, second side, first side.
        for value in [diagonal, center, b, a] {
            if value >= 0. {
                let w = if value >= 0.8 { 10. } else { 1. };
                total += value * w;
                weight += w;
            }
        }
        if weight == 0. {
            return Err("fluid corner has no owning fluid sample".into());
        }
        out[index] = total / weight;
    }
    Ok(out)
}
fn normalize(vector: [f64; 3]) -> [f64; 3] {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length < 1.0e-4 {
        [0.; 3]
    } else {
        vector.map(|v| v / length)
    }
}
/// `solid_face` implements the original fluid's solid-face query, not opaque-cube inference.
pub fn flow(
    position: [i32; 3],
    fluid: FluidState,
    mut sample: impl FnMut([i32; 3]) -> Result<FluidSample>,
    mut solid_face: impl FnMut([i32; 3], u8) -> Result<bool>,
) -> Result<[f64; 3]> {
    let mut vector = [0f64; 3];
    let own = fluid.height();
    for (_, offset) in [
        (2, [0, 0, -1]),
        (5, [1, 0, 0]),
        (3, [0, 0, 1]),
        (4, [-1, 0, 0]),
    ] {
        let neighbor_position = add(position, offset);
        let neighbor = sample(neighbor_position)?;
        if neighbor.fluid.is_some_and(|f| f.kind != fluid.kind) {
            continue;
        }
        let neighbor_height = neighbor.fluid.map_or(0., FluidState::height);
        let difference = if neighbor_height == 0. {
            if neighbor.blocks_motion {
                0.
            } else {
                let below = sample(add(neighbor_position, [0, -1, 0]))?;
                match below.fluid.filter(|f| f.kind == fluid.kind) {
                    Some(below) if below.height() > 0. => own - (below.height() - 0.8888889),
                    _ => 0.,
                }
            }
        } else {
            own - neighbor_height
        };
        vector[0] += f64::from(offset[0]) * f64::from(difference);
        vector[2] += f64::from(offset[2]) * f64::from(difference);
    }
    if fluid.falling {
        for (direction, offset) in [
            (2, [0, 0, -1]),
            (5, [1, 0, 0]),
            (3, [0, 0, 1]),
            (4, [-1, 0, 0]),
        ] {
            let neighbor = add(position, offset);
            if solid_face(neighbor, direction)? || solid_face(add(neighbor, [0, 1, 0]), direction)?
            {
                vector = normalize(vector);
                vector[1] -= 6.;
                break;
            }
        }
    }
    Ok(normalize(vector))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn water(amount: u8) -> FluidSample {
        FluidSample {
            fluid: Some(FluidState {
                kind: FluidKind::Water,
                amount,
                falling: false,
                waterlogged: false,
            }),
            blocks_motion: false,
        }
    }
    #[test]
    fn weighted_source_corner_does_not_average_to_a_thin_sheet() {
        let heights = corner_heights([0; 3], FluidKind::Water, |p| {
            Ok(if p == [0; 3] {
                water(8)
            } else {
                FluidSample {
                    fluid: None,
                    blocks_motion: false,
                }
            })
        })
        .unwrap();
        for h in heights {
            assert!((h - (20. / 27.)).abs() < 1e-6);
        }
    }
    #[test]
    fn above_diagonal_fluid_lifts_only_its_shared_corner() {
        let heights = corner_heights([0; 3], FluidKind::Water, |p| {
            Ok(
                if p == [0; 3] || p == [-1, 0, 0] || p == [-1, 0, -1] || p == [-1, 1, -1] {
                    water(8)
                } else {
                    FluidSample {
                        fluid: None,
                        blocks_motion: false,
                    }
                },
            )
        })
        .unwrap();
        assert_eq!(heights[0], 1.);
        assert!(heights[1..].iter().all(|h| *h < 1.));
    }
}
