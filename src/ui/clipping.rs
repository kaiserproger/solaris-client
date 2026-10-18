//! CPU scissoring for the shared UI geometry pass. Fixed polygon scratch; no per-triangle heap.
use crate::{
    api::{MeshGeometry, RenderLayer, Vertex},
    protocol::Result,
};
pub fn clip(
    mesh: &mut MeshGeometry,
    vertex_start: usize,
    index_start: usize,
    [x, y, w, h]: [f32; 4],
) -> Result<()> {
    if mesh.vertices.len() > 2_000_000 {
        return Err("ui_vertex_limit".into());
    }
    let bounds = [x, x + w, y, y + h];
    if mesh.vertices[vertex_start..].iter().all(|v| {
        v.position[0] >= bounds[0]
            && v.position[0] <= bounds[1]
            && v.position[1] >= bounds[2]
            && v.position[1] <= bounds[3]
    }) {
        return Ok(());
    }
    let layer = RenderLayer::Translucent as usize;
    let end = mesh.indices[layer].len();
    for offset in (index_start..end).step_by(3) {
        let indices = [
            mesh.indices[layer][offset],
            mesh.indices[layer][offset + 1],
            mesh.indices[layer][offset + 2],
        ];
        let first = mesh.vertices[indices[0] as usize];
        let mut polygon = [first; 8];
        for i in 0..3 {
            polygon[i] = mesh.vertices[indices[i] as usize];
        }
        if polygon[..3].iter().all(|v| {
            v.position[0] >= bounds[0]
                && v.position[0] <= bounds[1]
                && v.position[1] >= bounds[2]
                && v.position[1] <= bounds[3]
        }) {
            mesh.indices[layer].extend(indices);
            continue;
        }
        let mut count = 3;
        for side in 0..4 {
            if count == 0 {
                break;
            }
            let axis = side / 2;
            let limit = bounds[side];
            let inside = |v: &Vertex| {
                if side % 2 == 0 {
                    v.position[axis] >= limit
                } else {
                    v.position[axis] <= limit
                }
            };
            let mut output = [first; 8];
            let mut n = 0;
            for i in 0..count {
                let a = polygon[i];
                let b = polygon[(i + 1) % count];
                let ai = inside(&a);
                let bi = inside(&b);
                if ai {
                    output[n] = a;
                    n += 1;
                }
                if ai != bi {
                    let t = (limit - a.position[axis]) / (b.position[axis] - a.position[axis]);
                    output[n] = interpolate(a, b, t);
                    n += 1;
                }
            }
            polygon = output;
            count = n;
        }
        if count < 3 {
            continue;
        }
        if mesh.vertices.len() + count > 2_000_000 {
            return Err("ui_vertex_limit".into());
        }
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&polygon[..count]);
        for i in 1..count - 1 {
            mesh.indices[layer].extend([base, base + i as u32, base + i as u32 + 1]);
        }
    }
    let replacement_len = mesh.indices[layer].len() - end;
    mesh.indices[layer].copy_within(end.., index_start);
    mesh.indices[layer].truncate(index_start + replacement_len);
    Ok(())
}
fn interpolate(a: Vertex, b: Vertex, t: f32) -> Vertex {
    fn mix<const N: usize>(a: [f32; N], b: [f32; N], t: f32) -> [f32; N] {
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
    }
    Vertex {
        position: mix(a.position, b.position, t),
        color: mix(a.color, b.color, t),
        uv: mix(a.uv, b.uv, t),
        overlay_uv: mix(a.overlay_uv, b.overlay_uv, t),
        overlay_tint: mix(a.overlay_tint, b.overlay_tint, t),
        alpha: a.alpha + (b.alpha - a.alpha) * t,
        texture_layer: a.texture_layer,
        light: [-1.; 2],
        normal: [0.; 3],
    }
}
