use super::{Result, append_quad};
use crate::api::{MeshGeometry, RenderLayer, Vertex};
use std::sync::LazyLock;

pub(super) fn enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("SOLARIS_CLIENT_GREEDY").is_none_or(|v| v != "0"));
    *ENABLED
}

struct Face {
    vertices: [Vertex; 4],
    cell: [usize; 2],
}

/// Only opaque full-cell faces with constant corner lighting qualify. Keeping
/// smooth-lit gradients out of this path preserves the original triangulation.
pub(super) struct Greedy {
    planes: [Vec<Face>; 96],
}

fn axes(face: usize) -> [usize; 2] {
    match face / 2 {
        0 => [0, 2],
        1 => [0, 1],
        _ => [1, 2],
    }
}
fn normal_axis(face: usize) -> usize {
    [1, 1, 2, 2, 0, 0][face]
}

fn eligible(vertices: &[Vertex; 4], cell: [i32; 3], face: usize) -> bool {
    let axis = normal_axis(face);
    let tangent = axes(face);
    let plane = cell[axis] as f32 + (face % 2) as f32;
    let first = &vertices[0];
    if first.alpha != 1. || first.overlay_uv != [-1.; 2] {
        return false;
    }
    let uv_min: [f32; 2] = std::array::from_fn(|a| {
        vertices
            .iter()
            .map(|v| v.uv[a])
            .fold(f32::INFINITY, f32::min)
    });
    let uv_max: [f32; 2] = std::array::from_fn(|a| {
        vertices
            .iter()
            .map(|v| v.uv[a])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    if (0..2).any(|a| !uv_min[a].is_finite() || !uv_max[a].is_finite() || uv_min[a] >= uv_max[a]) {
        return false;
    }
    let mut corners = 0u8;
    for vertex in vertices {
        if vertex.color != first.color
            || vertex.light != first.light
            || vertex.normal != first.normal
            || vertex.alpha != first.alpha
            || vertex.texture_layer != first.texture_layer
            || vertex.overlay_uv != first.overlay_uv
            || vertex.overlay_tint != first.overlay_tint
            || (vertex.position[axis] - plane).abs() > 0.00001
        {
            return false;
        }
        let mut corner = 0;
        for (a, &t) in tangent.iter().enumerate() {
            let coordinate = vertex.position[t] - cell[t] as f32;
            if coordinate.abs() <= 0.00001 {
                // Lower corner.
            } else if (coordinate - 1.).abs() <= 0.00001 {
                corner |= 1 << a;
            } else {
                return false;
            }
        }
        if (0..2).any(|a| vertex.uv[a] != uv_min[a] && vertex.uv[a] != uv_max[a]) {
            return false;
        }
        corners |= 1 << corner;
    }
    // UVs must also form an affine rectangle, not a folded or degenerate mapping.
    if corners != 15 {
        return false;
    }
    let edge_a = glam::Vec2::from_array(vertices[1].uv) - glam::Vec2::from_array(vertices[0].uv);
    let edge_b = glam::Vec2::from_array(vertices[3].uv) - glam::Vec2::from_array(vertices[0].uv);
    edge_a.perp_dot(edge_b) != 0.
        && (glam::Vec2::from_array(vertices[0].uv) + glam::Vec2::from_array(vertices[2].uv)
            - glam::Vec2::from_array(vertices[1].uv)
            - glam::Vec2::from_array(vertices[3].uv))
        .length_squared()
            < 1e-12
}

fn same(a: &Face, b: &Face, tangent: [usize; 2]) -> bool {
    a.vertices.iter().zip(&b.vertices).all(|(av, bv)| {
        av.color == bv.color
            && av.light == bv.light
            && av.uv == bv.uv
            && av.normal == bv.normal
            && av.texture_layer == bv.texture_layer
            && av.alpha == bv.alpha
            && av.overlay_uv == bv.overlay_uv
            && av.overlay_tint == bv.overlay_tint
            && tangent.iter().enumerate().all(|(i, &axis)| {
                (av.position[axis] - a.cell[i] as f32).round()
                    == (bv.position[axis] - b.cell[i] as f32).round()
            })
    })
}

impl Greedy {
    pub fn new() -> Self {
        Self {
            planes: std::array::from_fn(|_| Vec::new()),
        }
    }

    pub fn push(&mut self, vertices: [Vertex; 4], cell: [i32; 3], face: usize) -> bool {
        if !eligible(&vertices, cell, face) {
            return false;
        }
        let tangent = axes(face);
        self.planes[face * 16 + cell[normal_axis(face)] as usize].push(Face {
            vertices,
            cell: tangent.map(|axis| cell[axis] as usize),
        });
        true
    }

    pub fn append(self, out: &mut MeshGeometry, byte_limit: usize) -> Result<()> {
        for (plane, faces) in self.planes.into_iter().enumerate() {
            if faces.is_empty() {
                continue;
            }
            let tangent = axes(plane / 16);
            let mut mask = [None; 256];
            // Multiple model layers at one face must not be swallowed by a mask.
            for (index, face) in faces.iter().enumerate() {
                let slot = face.cell[1] * 16 + face.cell[0];
                if let Some(previous) = mask[slot].replace(index) {
                    append_quad(
                        out,
                        faces[previous].vertices,
                        RenderLayer::Opaque,
                        false,
                        byte_limit,
                    )?;
                }
            }
            for v in 0..16 {
                for u in 0..16 {
                    let Some(index) = mask[v * 16 + u] else {
                        continue;
                    };
                    let face = &faces[index];
                    let matches = |slot: usize| {
                        mask[slot].is_some_and(|other| same(face, &faces[other], tangent))
                    };
                    let mut width = 1;
                    while u + width < 16 && matches(v * 16 + u + width) {
                        width += 1;
                    }
                    let mut height = 1;
                    while v + height < 16 && (0..width).all(|x| matches((v + height) * 16 + u + x))
                    {
                        height += 1;
                    }
                    for y in 0..height {
                        for x in 0..width {
                            mask[(v + y) * 16 + u + x] = None;
                        }
                    }
                    let mut vertices = face.vertices;
                    if width > 1 || height > 1 {
                        let min: [f32; 2] = std::array::from_fn(|a| {
                            vertices
                                .iter()
                                .map(|v| v.uv[a])
                                .fold(f32::INFINITY, f32::min)
                        });
                        let max: [f32; 2] = std::array::from_fn(|a| {
                            vertices
                                .iter()
                                .map(|v| v.uv[a])
                                .fold(f32::NEG_INFINITY, f32::max)
                        });
                        for vertex in &mut vertices {
                            let old = vertex.position;
                            for (a, size) in [width, height].into_iter().enumerate() {
                                let axis = tangent[a];
                                let local = (old[axis] - face.cell[a] as f32).round();
                                vertex.position[axis] = face.cell[a] as f32 + local * size as f32;
                            }
                            for uv_axis in 0..2 {
                                let mut local_uv = (vertex.uv[uv_axis] - min[uv_axis])
                                    / (max[uv_axis] - min[uv_axis]);
                                for &axis in &tangent {
                                    let other = face
                                        .vertices
                                        .iter()
                                        .find(|other| {
                                            (other.position[axis] - old[axis]).abs() > 0.5
                                                && (other.position[tangent[0] + tangent[1] - axis]
                                                    - old[tangent[0] + tangent[1] - axis])
                                                    .abs()
                                                    < 0.00001
                                        })
                                        .expect("eligible quad has each tangent edge");
                                    let slope = (other.uv[uv_axis] - vertex.uv[uv_axis])
                                        / (other.position[axis] - old[axis])
                                        / (max[uv_axis] - min[uv_axis]);
                                    local_uv += slope * (vertex.position[axis] - old[axis]);
                                }
                                vertex.uv[uv_axis] = local_uv;
                            }
                            vertex.overlay_uv = min;
                            vertex.overlay_tint = [max[0] - min[0], max[1] - min[1], -1.];
                        }
                    }
                    append_quad(out, vertices, RenderLayer::Opaque, false, byte_limit)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
