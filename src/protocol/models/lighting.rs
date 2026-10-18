//! Pinned BlockModelLighter face sampling and partial-face interpolation.
use super::super::Result;
use super::BakedQuad;
#[derive(Clone, Copy, Debug)]
pub struct LightSample {
    pub shade_brightness: f32,
    pub view_blocking: bool,
    pub light_dampening: u8,
    pub solid_render: bool,
    pub collision_full: bool,
    /// Sky, block levels; smooth coordinates preserve sixteenth-level precision.
    /// `[-1.; 2]` means the received light channels are not yet known.
    pub light: [f32; 2],
}
#[derive(Clone, Copy, Debug)]
pub struct VertexLighting {
    pub ao: f32,
    pub light: [f32; 2],
}
const NORMALS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];
// Original AdjacencyInfo order matters when two view-blocking sides suppress a diagonal.
const SIDES: [[usize; 4]; 6] = [
    [4, 5, 2, 3],
    [5, 4, 2, 3],
    [1, 0, 5, 4],
    [4, 5, 0, 1],
    [1, 0, 2, 3],
    [0, 1, 2, 3],
];
const CORNERS: [(usize, usize); 4] = [(0, 3), (0, 2), (1, 2), (1, 3)];
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn smooth(mut a: [f32; 2], mut b: [f32; 2], mut c: [f32; 2], center: [f32; 2]) -> [f32; 2] {
    if [a, b, c, center].contains(&[-1.; 2]) {
        return [-1.; 2];
    }
    if center[0] > 2. || center[1] > 2. {
        for value in [&mut a, &mut b, &mut c] {
            for i in 0..2 {
                if value[i] < 1. {
                    value[i] = center[i];
                }
            }
        }
    }
    std::array::from_fn(|i| {
        ((a[i] * 16. + b[i] * 16. + c[i] * 16. + center[i] * 16.) / 4.).floor() / 16.
    })
}
pub fn quad_lighting(
    quad: &BakedQuad,
    position: [i32; 3],
    ambient_occlusion: bool,
    mut sample: impl FnMut([i32; 3]) -> Result<LightSample>,
) -> Result<[VertexLighting; 4]> {
    let face = usize::from(quad.face);
    if face >= 6 {
        return Err("quad light face bound".into());
    }
    let normal = NORMALS[face];
    let axis = normal.iter().position(|v| *v != 0).unwrap();
    let min = std::array::from_fn::<_, 3, _>(|i| {
        quad.positions
            .iter()
            .map(|p| p[i])
            .fold(f32::INFINITY, f32::min)
    });
    let max = std::array::from_fn::<_, 3, _>(|i| {
        quad.positions
            .iter()
            .map(|p| p[i])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    let current = sample(position)?;
    let boundary = if normal[axis] < 0 {
        min[axis] < 0.0001
    } else {
        max[axis] > 0.9999
    };
    // Rotated variants (y quarter turns) leave ~1e-7 roundoff on the normal axis; an exact
    // equality would demote a full sunlit cube face to a partial one that samples its own
    // opaque cell and neighbours (near-black faces).
    let cubic = max[axis] - min[axis] < 0.0001 && (boundary || current.collision_full);
    let front = sample(add(position, normal))?;
    if !ambient_occlusion || !quad.ambient_occlusion {
        return Ok([VertexLighting {
            ao: 1.,
            light: if cubic { front.light } else { current.light },
        }; 4]);
    }
    let base = if cubic {
        add(position, normal)
    } else {
        position
    };
    let center_brightness = if cubic {
        front.shade_brightness
    } else {
        current.shade_brightness
    };
    let center_light = if cubic || !front.solid_render {
        front.light
    } else {
        current.light
    };
    let mut sides = [current; 4];
    let mut diagonal_open = [false; 4];
    for i in 0..4 {
        let direction = NORMALS[SIDES[face][i]];
        sides[i] = sample(add(base, direction))?;
        let check = sample(add(add(base, direction), normal))?;
        diagonal_open[i] = !check.view_blocking || check.light_dampening == 0;
    }
    let mut corners = [VertexLighting {
        ao: 0.,
        light: [0.; 2],
    }; 4];
    for (index, (a, b)) in CORNERS.into_iter().enumerate() {
        let diagonal = if diagonal_open[a] || diagonal_open[b] {
            sample(add(
                add(base, NORMALS[SIDES[face][a]]),
                NORMALS[SIDES[face][b]],
            ))?
        } else {
            sides[a]
        };
        corners[index] = VertexLighting {
            ao: (sides[a].shade_brightness
                + sides[b].shade_brightness
                + diagonal.shade_brightness
                + center_brightness)
                * 0.25,
            light: smooth(sides[a].light, sides[b].light, diagonal.light, center_light),
        };
    }
    let weight = |point: [f32; 3], direction: [i32; 3]| {
        let axis = direction.iter().position(|v| *v != 0).unwrap();
        let coordinate = if point[axis] <= (min[axis] + max[axis]) * 0.5 {
            min[axis]
        } else {
            max[axis]
        };
        // Rotating baked vertices can move a cube edge by a float epsilon.
        // Interpolation coordinates remain on the unit face, not beyond it.
        let coordinate = coordinate.clamp(0., 1.);
        if direction[axis] > 0 {
            coordinate
        } else {
            1. - coordinate
        }
    };
    Ok(std::array::from_fn(|vertex| {
        let point = quad.positions[vertex];
        let u = weight(point, NORMALS[SIDES[face][0]]);
        let v = weight(point, NORMALS[SIDES[face][2]]);
        let weights = [u * (1. - v), u * v, (1. - u) * v, (1. - u) * (1. - v)];
        let ao = corners
            .iter()
            .zip(weights)
            .map(|(c, w)| c.ao * w)
            .sum::<f32>();
        let light = if corners
            .iter()
            .zip(weights)
            .any(|(corner, weight)| weight > 0. && corner.light == [-1.; 2])
        {
            [-1.; 2]
        } else {
            std::array::from_fn(|channel| {
                (corners
                    .iter()
                    .zip(weights)
                    .map(|(c, w)| c.light[channel] * 16. * w)
                    .sum::<f32>())
                .floor()
                    / 16.
            })
        };
        VertexLighting {
            ao: (ao.clamp(0., 1.) * 255.).floor() / 255.,
            light,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::super::TintKind;
    use super::*;
    use crate::api::RenderLayer;
    fn quad() -> BakedQuad {
        BakedQuad {
            positions: super::super::face_positions(1, [0.; 3], [1.; 3]),
            uv: [[0.; 2]; 4],
            tint: TintKind::None,
            tint_index: None,
            cullface: Some(1),
            face: 1,
            shade: true,
            ambient_occlusion: true,
            layer: RenderLayer::Opaque,
            alpha: 1.,
            texture_layer: 0,
        }
    }
    fn air() -> LightSample {
        LightSample {
            shade_brightness: 1.,
            view_blocking: false,
            light_dampening: 0,
            solid_render: false,
            collision_full: false,
            light: [15., 0.],
        }
    }
    #[test]
    fn ambient_occlusion_darkens_only_the_obstructed_corner() {
        let vertices = quad_lighting(&quad(), [0; 3], true, |p| {
            let mut value = air();
            if p == [1, 1, -1] {
                value.shade_brightness = 0.2;
            }
            Ok(value)
        })
        .unwrap();
        let near = quad()
            .positions
            .iter()
            .position(|p| p[0] == 1. && p[2] == 0.)
            .unwrap();
        assert_eq!(vertices[near].ao, 0.8);
        assert!(
            vertices
                .iter()
                .enumerate()
                .all(|(i, v)| i == near || v.ao == 1.)
        );
    }
    #[test]
    fn source_zero_channel_replacement_is_not_applied_in_dark_regions() {
        assert_eq!(smooth([0.; 2], [0.; 2], [0.; 2], [2., 1.]), [0.5, 0.25]);
        assert_eq!(smooth([0.; 2], [0.; 2], [0.; 2], [15., 0.]), [15., 0.]);
    }
    #[test]
    fn quarter_turn_roundoff_keeps_interpolated_light_in_range() {
        let mut face = quad();
        for point in &mut face.positions {
            point[0] = if point[0] == 0. {
                -f32::EPSILON
            } else {
                1. + f32::EPSILON
            };
        }
        let vertices = quad_lighting(&face, [0; 3], true, |p| {
            let mut value = air();
            value.light = if p == [-1, 1, 1] { [4., 0.] } else { [0.; 2] };
            Ok(value)
        })
        .unwrap();
        for vertex in vertices {
            assert!(
                vertex
                    .light
                    .into_iter()
                    .all(|level| (0. ..=15.).contains(&level)),
                "{vertex:?}"
            );
        }
    }
    /// Rotated stone variants carry float roundoff on the face-normal axis: a
    /// sunlit outer stone face must still read its open front neighbour.
    #[test]
    fn rotated_cube_face_with_normal_axis_roundoff_samples_the_open_neighbor() {
        let stone = LightSample {
            shade_brightness: 0.2,
            view_blocking: true,
            light_dampening: 15,
            solid_render: true,
            collision_full: true,
            light: [0.; 2],
        };
        for roundoff in [0., f32::EPSILON, 2.9802322e-8, -5.9604645e-8] {
            let mut face = quad();
            face.positions[0][1] = 1. + roundoff;
            face.positions[2][1] = 1. - roundoff;
            let vertices = quad_lighting(&face, [0; 3], true, |p| {
                Ok(if p == [0; 3] { stone } else { air() })
            })
            .unwrap();
            for vertex in vertices {
                assert_eq!(vertex.light, [15., 0.], "roundoff {roundoff}: {vertex:?}");
                assert_eq!(vertex.ao, 1., "roundoff {roundoff}: {vertex:?}");
            }
        }
    }
    #[test]
    fn unknown_corner_light_propagates_only_to_vertices_that_use_it() {
        let face = quad();
        let vertices = quad_lighting(&face, [0; 3], true, |p| {
            let mut value = air();
            value.light = if p == [-1, 1, 1] { [-1.; 2] } else { [0.; 2] };
            Ok(value)
        })
        .unwrap();
        for (position, vertex) in face.positions.iter().zip(vertices) {
            assert_eq!(
                vertex.light,
                if position[0] == 0. && position[2] == 1. {
                    [-1.; 2]
                } else {
                    [0.; 2]
                }
            );
        }
        assert_eq!(smooth([-1.; 2], [15.; 2], [15.; 2], [15.; 2]), [-1.; 2]);
    }
}
