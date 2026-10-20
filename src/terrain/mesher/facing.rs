//! Class partition of a section's opaque and cutout quads.
//!
//! Every terrain pipeline back-face culls, so an axis-aligned quad whose
//! outward plane the camera is behind never produces a pixel. Ordering each
//! layer's quads by facing lets the renderer skip those index ranges on the
//! CPU instead of paying vertex fetch, shading and setup for them on the GPU.
//! Rooted waving vegetation (shader class 2, recognised by its tagged normal
//! length) gets its own group so distant sections can skip it.
use crate::api::{FACE_GROUPS, FaceGroups, MeshGeometry, VEGETATION_GROUP, Vertex};

/// Tolerance for "all four corners lie on one axis plane".
const PLANAR: f32 = 1e-4;
const ALWAYS: usize = FACE_GROUPS - 1;
/// Shader class 2 tags `normal` with a length of 2.0..=2.2 (see `ShaderClass`).
const VEGETATION_NORMAL: std::ops::RangeInclusive<f32> = 1.9..=2.3;
/// Group base per position axis: `-Y,+Y`, `-Z,+Z`, `-X,+X`.
const AXIS_BASE: [usize; 3] = [4, 0, 2];

/// Facing group and plane coordinate of one two-triangle quad, or `None` when
/// it is not an axis-aligned plane (cross plants, rotated models, degenerate).
fn classify(vertices: &[Vertex], quad: &[u32]) -> Option<(usize, f32)> {
    let vegetation = vertices.get(quad[0] as usize).is_some_and(|v| {
        VEGETATION_NORMAL.contains(&v.normal.iter().map(|c| c * c).sum::<f32>().sqrt())
    });
    if vegetation {
        return Some((VEGETATION_GROUP, 0.));
    }
    let at = |i: u32| vertices.get(i as usize).map(|v| v.position);
    let (a, b, c) = (at(quad[0])?, at(quad[1])?, at(quad[2])?);
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let normal = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let axis = (0..3).fold(0, |best, i| {
        if normal[i].abs() > normal[best].abs() {
            i
        } else {
            best
        }
    });
    if normal[axis] == 0. || !normal[axis].is_finite() {
        return None;
    }
    for &index in &quad[3..] {
        if (at(index)?[axis] - a[axis]).abs() > PLANAR {
            return None;
        }
    }
    // The first triangle already pins its own plane; the second must agree.
    if (b[axis] - a[axis]).abs() > PLANAR || (c[axis] - a[axis]).abs() > PLANAR {
        return None;
    }
    Some((AXIS_BASE[axis] + usize::from(normal[axis] > 0.), a[axis]))
}

/// Stably reorders `indices` by facing group and returns the partition.
/// Returns `None` (layer left untouched) when the list is not whole quads.
fn partition(vertices: &[Vertex], indices: &mut Vec<u32>) -> Option<FaceGroups> {
    if indices.is_empty() || !indices.len().is_multiple_of(6) {
        return None;
    }
    let mut groups: [Vec<u32>; FACE_GROUPS] = std::array::from_fn(|_| Vec::new());
    let mut planes: [Option<f32>; 6] = [None; 6];
    for quad in indices.chunks_exact(6) {
        let group = match classify(vertices, quad) {
            Some((VEGETATION_GROUP, _)) => VEGETATION_GROUP,
            Some((group, plane)) => {
                let slot = &mut planes[group];
                // Positive normals face the camera from above their plane, so
                // the lowest plane decides; negative ones the highest.
                *slot = Some(match *slot {
                    None => plane,
                    Some(old) if group % 2 == 1 => old.min(plane),
                    Some(old) => old.max(plane),
                });
                group
            }
            None => ALWAYS,
        };
        groups[group].extend_from_slice(quad);
    }
    let mut ends = [0u32; FACE_GROUPS];
    let mut ordered = Vec::with_capacity(indices.len());
    for (group, list) in groups.iter().enumerate() {
        ordered.extend_from_slice(list);
        ends[group] = ordered.len() as u32;
    }
    *indices = ordered;
    Some(FaceGroups {
        ends,
        planes: planes.map(|plane| plane.unwrap_or(0.)),
    })
}

/// Partitions the opaque and cutout layers of a finished section.
pub(super) fn apply(geometry: &mut MeshGeometry) {
    for layer in 0..2 {
        geometry.faces[layer] = partition(&geometry.vertices, &mut geometry.indices[layer]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(vertices: &mut Vec<Vertex>, corners: [[f32; 3]; 4]) -> Vec<u32> {
        let first = vertices.len() as u32;
        for position in corners {
            vertices.push(Vertex {
                position,
                ..bytemuck::Zeroable::zeroed()
            });
        }
        vec![first, first + 1, first + 2, first, first + 2, first + 3]
    }

    #[test]
    fn groups_follow_winding_normals_and_planes() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        // +Y at y=3 (CCW seen from above), -Y at y=1, +X at x=5, diagonal,
        // then a tagged (class 2) vegetation quad.
        indices.extend(quad(
            &mut vertices,
            [[0., 3., 0.], [0., 3., 1.], [1., 3., 1.], [1., 3., 0.]],
        ));
        indices.extend(quad(
            &mut vertices,
            [[0., 1., 0.], [1., 1., 0.], [1., 1., 1.], [0., 1., 1.]],
        ));
        indices.extend(quad(
            &mut vertices,
            [[5., 0., 0.], [5., 1., 0.], [5., 1., 1.], [5., 0., 1.]],
        ));
        indices.extend(quad(
            &mut vertices,
            [[0., 0., 0.], [1., 0., 1.], [1., 1., 1.], [0., 1., 0.]],
        ));
        indices.extend(quad(
            &mut vertices,
            [[0., 2., 0.], [0., 2., 1.], [1., 2., 1.], [1., 2., 0.]],
        ));
        indices.extend(quad(
            &mut vertices,
            [[0., 0., 0.], [1., 0., 1.], [1., 1., 1.], [0., 1., 0.]],
        ));
        for vertex in &mut vertices[20..24] {
            vertex.normal = [0., 2.1, 0.];
        }
        let faces = partition(&vertices, &mut indices).expect("whole quads");
        // Two +Y quads (y=3 and y=2): group 1 holds both, minimum plane wins.
        assert_eq!(faces.ends, [6, 18, 18, 18, 18, 24, 30, 36]);
        assert_eq!(faces.planes[1], 2.);
        assert_eq!(faces.planes[0], 1.);
        assert_eq!(faces.planes[5], 5.);
        assert_eq!(indices.len(), 36);
    }

    #[test]
    fn ragged_lists_stay_whole() {
        let mut vertices = Vec::new();
        let mut indices = quad(
            &mut vertices,
            [[0., 3., 0.], [0., 3., 1.], [1., 3., 1.], [1., 3., 0.]],
        );
        indices.push(0);
        let before = indices.clone();
        assert!(partition(&vertices, &mut indices).is_none());
        assert_eq!(indices, before);
    }
}
