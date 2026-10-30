//! Wireframe around the block under the crosshair: the edges of the union of
//! the block's selection boxes, drawn as thin translucent black screen-space
//! lines through the shared `Lines` render type.
use crate::{
    api::{DynamicGeometry, EntityMaterial, Vertex},
    protocol::{Result, content::Content},
};

/// Black at 40 % opacity, 2.5 px wide at the reference 1920-pixel window.
const COLOR: [f32; 3] = [0.; 3];
const ALPHA: f32 = 0.4;
const WIDTH: f32 = 2.5;

type Segment = ([f64; 3], [f64; 3]);

/// Outer edges of the union of axis-aligned boxes `[min x,y,z, max x,y,z]`.
/// An edge between two grid cells belongs to the shape's outline when the four
/// cells around it are an outside corner (1 solid), an inside corner (3 solid)
/// or a diagonal pair; flat faces (2 adjacent) and interiors (0 or 4) are not
/// edges. Collinear neighbours are merged into one segment.
pub(super) fn union_edges(boxes: &[[f64; 6]]) -> Vec<Segment> {
    let boxes: Vec<_> = boxes
        .iter()
        .filter(|b| (0..3).all(|a| b[a] < b[a + 3]))
        .collect();
    if boxes.is_empty() {
        return Vec::new();
    }
    let coords: [Vec<f64>; 3] = std::array::from_fn(|axis| {
        let mut values: Vec<f64> = boxes.iter().flat_map(|b| [b[axis], b[axis + 3]]).collect();
        values.sort_by(f64::total_cmp);
        values.dedup();
        values
    });
    let cells: [usize; 3] = std::array::from_fn(|axis| coords[axis].len() - 1);
    let index = |c: [usize; 3]| (c[0] * cells[1] + c[1]) * cells[2] + c[2];
    let mut solid = vec![false; cells[0] * cells[1] * cells[2]];
    for cell in (0..cells[0]).flat_map(|x| (0..cells[1]).flat_map(move |y| (0..cells[2]).map(move |z| [x, y, z]))) {
        let center: [f64; 3] = std::array::from_fn(|a| (coords[a][cell[a]] + coords[a][cell[a] + 1]) / 2.);
        solid[index(cell)] = boxes
            .iter()
            .any(|b| (0..3).all(|a| b[a] < center[a] && center[a] < b[a + 3]));
    }
    let is_solid = |c: [isize; 3]| {
        (0..3).all(|a| c[a] >= 0 && (c[a] as usize) < cells[a]) && solid[index(c.map(|v| v as usize))]
    };
    let mut segments = Vec::new();
    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        for ib in 0..coords[b].len() {
            for ic in 0..coords[c].len() {
                let mut open: Option<usize> = None;
                for s in 0..=cells[axis] {
                    let edge = s < cells[axis] && {
                        let at = |db: isize, dc: isize| {
                            let mut cell = [0isize; 3];
                            cell[axis] = s as isize;
                            cell[b] = ib as isize + db;
                            cell[c] = ic as isize + dc;
                            is_solid(cell)
                        };
                        let quad = [at(-1, -1), at(0, -1), at(-1, 0), at(0, 0)];
                        match quad.iter().filter(|&&v| v).count() {
                            1 | 3 => true,
                            2 => quad[0] == quad[3],
                            _ => false,
                        }
                    };
                    match (edge, open) {
                        (true, None) => open = Some(s),
                        (false, Some(start)) => {
                            let mut from = [0.; 3];
                            from[axis] = coords[axis][start];
                            from[b] = coords[b][ib];
                            from[c] = coords[c][ic];
                            let mut to = from;
                            to[axis] = coords[axis][s];
                            segments.push((from, to));
                            open = None;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    segments
}

/// Append the outline of `state` at `position`. Positions are relative to the
/// scene origin like every other dynamic vertex.
pub(super) fn append(
    content: &Content,
    state: u32,
    position: [i32; 3],
    origin: [f64; 3],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let boxes = content
        .selection_boxes(state)
        .ok_or("unknown_selection_shape")?;
    let offset = content.block_offset(state, position);
    let base: [f64; 3] = std::array::from_fn(|i| f64::from(position[i]) + offset[i] - origin[i]);
    for (from, to) in union_edges(boxes) {
        let p = std::array::from_fn::<f32, 3, _>(|i| (base[i] + from[i]) as f32);
        let q = std::array::from_fn::<f32, 3, _>(|i| (base[i] + to[i]) as f32);
        let length = (0..3).map(|i| (q[i] - p[i]).powi(2)).sum::<f32>().sqrt();
        if length <= 0. {
            continue;
        }
        let normal = std::array::from_fn(|i| (q[i] - p[i]) / length);
        let first = u32::try_from(out.vertices.len()).map_err(|_| "outline index bound")?;
        for (point, side) in [(p, 1.), (p, -1.), (q, 1.), (q, -1.)] {
            out.vertices.push(Vertex {
                position: point,
                color: COLOR,
                alpha: ALPHA,
                uv: [WIDTH, 0.],
                overlay_uv: [side, 0.],
                overlay_tint: [1.; 3],
                texture_layer: 0,
                light: [-1.; 2],
                normal,
            });
        }
        out.indices[EntityMaterial::Lines as usize].extend([
            first,
            first + 1,
            first + 2,
            first + 3,
            first + 2,
            first + 1,
        ]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_length(edges: &[Segment]) -> f64 {
        edges
            .iter()
            .map(|(a, b)| (0..3).map(|i| (b[i] - a[i]).abs()).sum::<f64>())
            .sum()
    }

    #[test]
    fn single_box_is_twelve_edges() {
        let edges = union_edges(&[[0., 0., 0., 1., 1., 1.]]);
        assert_eq!(edges.len(), 12);
        assert_eq!(total_length(&edges), 12.);
    }

    #[test]
    fn stacked_boxes_drop_the_seam_and_merge_collinear_edges() {
        let edges = union_edges(&[[0., 0., 0., 1., 0.5, 1.], [0., 0.5, 0., 1., 1., 1.]]);
        assert_eq!(edges.len(), 12, "{edges:?}");
        assert_eq!(total_length(&edges), 12.);
    }

    #[test]
    fn stepped_union_keeps_the_inside_corner() {
        // L-shaped footprint, one block tall.
        let edges = union_edges(&[[0., 0., 0., 1., 1., 1.], [1., 0., 0., 2., 1., 0.5]]);
        // Prism over the L polygon: 6 + 6 around the faces plus 6 verticals.
        assert_eq!(total_length(&edges), 18.);
    }

    #[test]
    fn empty_and_degenerate_boxes_have_no_edges() {
        assert!(union_edges(&[]).is_empty());
        assert!(union_edges(&[[0., 0., 0., 0., 1., 1.]]).is_empty());
    }

    #[test]
    fn outline_vertices_are_black_thin_lines() {
        let content = Content::test_cube();
        let mut out = DynamicGeometry::default();
        append(&content, 1, [3, 4, 5], [0.; 3], &mut out).unwrap();
        assert_eq!(out.vertices.len(), 12 * 4);
        assert_eq!(out.indices[EntityMaterial::Lines as usize].len(), 12 * 6);
        assert!(out.vertices.iter().all(|v| v.alpha == ALPHA && v.color == [0.; 3]));
        assert!(out.vertices.iter().all(|v| v.texture_layer == 0), "line vertices must use a valid atlas layer");
        let max = out.vertices.iter().fold([f32::MIN; 3], |m, v| std::array::from_fn(|i| m[i].max(v.position[i])));
        assert_eq!(max, [4., 5., 6.]);
    }
}
