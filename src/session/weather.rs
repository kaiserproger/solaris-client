//! Batched precipitation in received motion-blocking columns.
use crate::{
    api::{DynamicGeometry, EntityMaterial, Vertex},
    protocol::{Result, content::Content, world::World},
};

pub(super) fn append(
    world: &World,
    content: &Content,
    camera: [f64; 3],
    origin: [f64; 3],
    rain: f32,
    ticks: f64,
    mesh: &mut DynamicGeometry,
) -> Result<()> {
    if rain <= 0. {
        return Ok(());
    }
    let rain_texture = content.texture("minecraft:environment/rain")
        .ok_or("missing mandatory environment texture minecraft:environment/rain")?;
    let snow_texture = content.texture("minecraft:environment/snow")
        .ok_or("missing mandatory environment texture minecraft:environment/snow")?;
    let center = camera.map(|v| v.floor() as i32);
    let bottom = camera[1] - 5.;
    let top = camera[1] + 5.;
    for dz in -5i32..=5 {
        for dx in -5i32..=5 {
            let x = center[0] + dx;
            let z = center[2] + dz;
            let Some(height) = world.precipitation_height(x, z, content)? else { continue };
            let low = bottom.max(f64::from(height));
            if low >= top { continue; }
            let Some(snow) = world.precipitation_is_snow([x, height.max(center[1]), z], content)? else { continue };
            let texture = if snow { snow_texture } else { rain_texture };
            let px = f64::from(x) + 0.5;
            let pz = f64::from(z) + 0.5;
            let vx = px - camera[0];
            let vz = pz - camera[2];
            let distance = (vx * vx + vz * vz).sqrt();
            let alpha = rain * (1. - (distance as f32 / 8.).min(1.)) * if snow { 0.75 } else { 0.6 };
            if alpha <= 0. { continue; }
            let (sx, sz) = if distance > 0.001 { (-vz / distance * 0.5, vx / distance * 0.5) } else { (0.5, 0.) };
            let seed = (x.wrapping_mul(3121) ^ z.wrapping_mul(4187)).rem_euclid(100) as f64 / 100.;
            let phase = (ticks * if snow { 0.025 } else { 0.2 } + seed).rem_euclid(1.);
            let length = if snow { 2.0 } else { 3.0 };
            let mut y = (low / length).floor() * length - phase * length;
            while y < top {
                let y0 = y.max(low);
                let y1 = (y + length).min(top);
                if y1 > y0 {
                    let v0 = texture.uv[1] + (texture.uv[3] - texture.uv[1]) * ((y0 - y) / length) as f32;
                    let v1 = texture.uv[1] + (texture.uv[3] - texture.uv[1]) * ((y1 - y) / length) as f32;
                    let light = world.light_levels([x, y0.floor() as i32, z]);
                    let light = [light[0].map_or(15., f32::from), light[1].map_or(0., f32::from)];
                    let first = mesh.vertices.len() as u32;
                    for (position, uv) in [
                        ([px - sx, y0, pz - sz], [texture.uv[0], v1]),
                        ([px + sx, y0, pz + sz], [texture.uv[2], v1]),
                        ([px + sx, y1, pz + sz], [texture.uv[2], v0]),
                        ([px - sx, y1, pz - sz], [texture.uv[0], v0]),
                    ] {
                        mesh.vertices.push(Vertex {
                            position: std::array::from_fn(|i| (position[i] - origin[i]) as f32),
                            color: [1.; 3], uv, overlay_uv: [0.; 2], overlay_tint: [0.; 3],
                            alpha, texture_layer: texture.layer, light, normal: [0., 1., 0.],
                        });
                    }
                    mesh.indices[EntityMaterial::Precipitation as usize].extend_from_slice(&[
                        first, first + 1, first + 2, first + 2, first + 3, first,
                    ]);
                }
                y += length;
            }
        }
    }
    Ok(())
}
