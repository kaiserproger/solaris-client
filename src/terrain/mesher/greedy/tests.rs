use super::*;

fn quad(x: i32, z: i32) -> [Vertex; 4] {
    let x = x as f32;
    let z = z as f32;
    let positions = [
        [x, 1., z],
        [x, 1., z + 1.],
        [x + 1., 1., z + 1.],
        [x + 1., 1., z],
    ];
    let uv = [[0.125, 0.25], [0.125, 0.5], [0.25, 0.5], [0.25, 0.25]];
    std::array::from_fn(|i| Vertex {
        position: positions[i],
        color: [0.7; 3],
        uv: uv[i],
        overlay_uv: [-1.; 2],
        overlay_tint: [1.; 3],
        alpha: 1.,
        texture_layer: 3,
        light: [15., 0.],
        normal: [0., 1., 0.],
    })
}

#[test]
fn greedy_uniform_rectangle_preserves_area_winding_and_repeated_texture() {
    for rotation in 0..4 {
        let mut greedy = Greedy::new();
        for z in 0..3 {
            for x in 0..2 {
                let mut vertices = quad(x, z);
                let original_uv = vertices.map(|v| v.uv);
                for i in 0..4 {
                    vertices[i].uv = original_uv[(i + rotation) % 4];
                }
                assert!(greedy.push(vertices, [x, 0, z], 1));
            }
        }
        let mut out = MeshGeometry::default();
        greedy.append(&mut out, 1_000_000).unwrap();
        assert_eq!(out.vertices.len(), 4);
        assert_eq!(out.indices[0].len(), 6);
        let a = glam::Vec3::from_array(out.vertices[0].position);
        let b = glam::Vec3::from_array(out.vertices[1].position);
        let c = glam::Vec3::from_array(out.vertices[2].position);
        assert_eq!((b - a).cross(c - a), glam::Vec3::Y * 6.);
        assert!(out
            .vertices
            .iter()
            .all(|v| v.overlay_tint == [0.125, 0.25, -1.]));
        // Compare every original unit face's interior with the repeated atlas UV
        // reconstructed from the merged quad's affine interpolation.
        for z in 0..3 {
            for x in 0..2 {
                let original = quad(x, z);
                for (s, t) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let weights = [(1. - s) * (1. - t), (1. - s) * t, s * t, s * (1. - t)];
                    let original_uv: [f32; 2] = std::array::from_fn(|axis| {
                        (0..4)
                            .map(|i| original[(i + rotation) % 4].uv[axis] * weights[i])
                            .sum()
                    });
                    let s = (x as f32 + s) / 2.;
                    let t = (z as f32 + t) / 3.;
                    let weights = [(1. - s) * (1. - t), (1. - s) * t, s * t, s * (1. - t)];
                    for (axis, expected) in original_uv.into_iter().enumerate() {
                        let repeated: f32 =
                            (0..4).map(|i| out.vertices[i].uv[axis] * weights[i]).sum();
                        let v = &out.vertices[0];
                        let actual = v.overlay_uv[axis]
                            + (repeated - repeated.floor()) * v.overlay_tint[axis];
                        assert!(
                            (actual - expected).abs() < 0.00001,
                            "rotation {rotation}, axis {axis}: {actual} != {expected}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn greedy_preserves_light_ao_tint_texture_and_orientation_boundaries() {
    for difference in 0..5 {
        let mut greedy = Greedy::new();
        assert!(greedy.push(quad(0, 0), [0, 0, 0], 1));
        let mut next = quad(1, 0);
        for vertex in &mut next {
            match difference {
                0 => vertex.color = [0.6; 3],
                1 => vertex.light = [14., 0.],
                2 => vertex.texture_layer = 4,
                3 => vertex.uv[0] += 0.125,
                _ => {}
            }
        }
        if difference == 4 {
            next.rotate_left(1);
        }
        assert!(greedy.push(next, [1, 0, 0], 1));
        let mut out = MeshGeometry::default();
        greedy.append(&mut out, 1_000_000).unwrap();
        assert_eq!(out.indices[0].len(), 12, "difference {difference}");
    }
    let mut greedy = Greedy::new();
    let mut gradient = quad(0, 0);
    gradient[2].light[0] = 14.;
    assert!(!greedy.push(gradient, [0, 0, 0], 1));
    gradient = quad(0, 0);
    gradient[2].color[0] = 0.5;
    assert!(!greedy.push(gradient, [0, 0, 0], 1));
    gradient = quad(0, 0);
    gradient[2].position[0] -= 0.5;
    assert!(!greedy.push(gradient, [0, 0, 0], 1));
}

#[test]
fn greedy_merges_both_tangent_axes_on_every_cardinal_face() {
    for face in 0..6 {
        let tangent = axes(face);
        let axis = normal_axis(face);
        let mut greedy = Greedy::new();
        for v in 0..3 {
            for u in 0..2 {
                let mut vertices = quad(u, v);
                let mut cell = [0; 3];
                cell[tangent[0]] = u;
                cell[tangent[1]] = v;
                for vertex in &mut vertices {
                    let old = vertex.position;
                    vertex.position[axis] = (face % 2) as f32;
                    vertex.position[tangent[0]] = old[0];
                    vertex.position[tangent[1]] = old[2];
                    vertex.normal = super::super::NORMALS[face].map(|v| v as f32);
                }
                assert!(greedy.push(vertices, cell, face));
            }
        }
        let mut out = MeshGeometry::default();
        greedy.append(&mut out, 1_000_000).unwrap();
        assert_eq!(out.indices[0].len(), 6, "face {face}");
        for (i, &t) in tangent.iter().enumerate() {
            let min = out
                .vertices
                .iter()
                .map(|v| v.position[t])
                .fold(f32::INFINITY, f32::min);
            let max = out
                .vertices
                .iter()
                .map(|v| v.position[t])
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!((min, max), (0., [2., 3.][i]));
        }
        assert!(out
            .vertices
            .iter()
            .all(|v| v.position[axis] == (face % 2) as f32));
    }
}
