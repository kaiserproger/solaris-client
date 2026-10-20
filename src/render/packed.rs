//! Compact 20-byte terrain vertex and its sprite-rectangle table.
//!
//! The mesher produces full `api::Vertex` values (80 bytes). Resident terrain
//! stores only what the terrain shaders need, decoded in `shader.wgsl`
//! (`decode_terrain`). Every terrain vertex carries section-relative geometry,
//! so the layout is lossless for the data the mesher emits:
//!
//! | bytes  | field                                                              |
//! |--------|--------------------------------------------------------------------|
//! | 0..6   | `position` x,y,z as u16: `(p + POSITION_BIAS) * POSITION_SCALE`      |
//! | 6..8   | `flags`: face(3) class(3) tiled(1) untextured(1) weight(8)          |
//! | 8..12  | `color` rgb + `alpha`, unorm8 (mesher colors are already 1/255 steps) |
//! | 12..14 | `light` sky, block as `level * 16` (sixteenth precision), 255 = unlit |
//! | 14..16 | `layer` (texture array layer; the sprite id when tiled)              |
//! | 16..20 | `uv` u16 pair: atlas-normalised, or 8.8 fixed tile coordinates       |
//!
//! `face` indexes the six axis normals; `class` is the shader tag that the
//! full vertex carried in `normal`'s length (0 none, 1 ordinary, 2 rooted
//! vegetation, 3 leaves, 4 water) and `weight` the rooted-sway amplitude
//! (`0..=128`). Greedy-tiled quads (`overlay_tint.z < 0`) reference a
//! `(layer, origin, size)` sprite rectangle in `TileTable`, read by the vertex
//! shader from a small texture.
use std::collections::HashMap;

use crate::api::Vertex;

pub(super) const POSITION_SCALE: f32 = 2048.;
pub(super) const POSITION_BIAS: f32 = 8.;
pub(super) const LIGHT_SCALE: f32 = 16.;
pub(super) const UNLIT: u8 = 255;
pub(super) const WEIGHT_SCALE: f32 = 128.;
/// Atlas uv scale: a power of two so every multiple of 1/1024 (the 1024 px
/// atlas pages) is exact; range `[0, 2)`.
pub(super) const UV_SCALE: f32 = 32768.;
const UV_LIMIT: f32 = 65535. / UV_SCALE;
pub(super) const TILE_UV_SCALE: f32 = 256.;
pub(super) const TILE_UV_BIAS: f32 = 64.;
pub(super) const FACE_NORMALS: [[f32; 3]; 6] = [
    [0., -1., 0.],
    [0., 1., 0.],
    [0., 0., -1.],
    [0., 0., 1.],
    [-1., 0., 0.],
    [1., 0., 0.],
];

const FLAG_CLASS_SHIFT: u16 = 3;
const FLAG_TILED: u16 = 1 << 6;
const FLAG_UNTEXTURED: u16 = 1 << 7;
const FLAG_WEIGHT_SHIFT: u16 = 8;

/// Tile table texture: two RGBA32F texels per sprite rectangle.
pub(super) const TILE_TEXTURE_WIDTH: u32 = 1024;
pub(super) const TILE_TEXTURE_HEIGHT: u32 = 64;
pub(super) const MAX_TILES: usize = (TILE_TEXTURE_WIDTH * TILE_TEXTURE_HEIGHT / 2) as usize;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct TerrainVertex {
    pub position: [u16; 3],
    pub flags: u16,
    pub color: [u8; 4],
    pub light: [u8; 2],
    pub layer: u16,
    pub uv: [u16; 2],
}

/// Append-only sprite rectangles referenced by tiled vertices. Entries are
/// never freed while meshes may reference them; identical rectangles share one.
#[derive(Debug, Default)]
pub(super) struct TileTable {
    ids: HashMap<[u32; 5], u16>,
    /// Two texels per rectangle: `[origin.xy, size.xy]`, `[layer, 0, 0, 0]`.
    texels: Vec<[f32; 4]>,
    uploaded: usize,
}

impl TileTable {
    fn intern(&mut self, layer: u32, origin: [f32; 2], size: [f32; 2]) -> Result<u16, String> {
        let key = [
            layer,
            origin[0].to_bits(),
            origin[1].to_bits(),
            size[0].to_bits(),
            size[1].to_bits(),
        ];
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        let id = self.texels.len() / 2;
        if id >= MAX_TILES {
            return Err("terrain_tile_table_exhausted".into());
        }
        self.texels.push([origin[0], origin[1], size[0], size[1]]);
        self.texels.push([layer as f32, 0., 0., 0.]);
        self.ids.insert(key, id as u16);
        Ok(id as u16)
    }

    #[cfg(test)]
    fn entry(&self, id: u16) -> (u32, [f32; 2], [f32; 2]) {
        let rect = self.texels[usize::from(id) * 2];
        let layer = self.texels[usize::from(id) * 2 + 1][0];
        (layer as u32, [rect[0], rect[1]], [rect[2], rect[3]])
    }

    /// Writes the rows touched since the last flush; returns bytes written.
    pub fn flush(&mut self, queue: &wgpu::Queue, texture: &wgpu::Texture) -> u64 {
        if self.uploaded == self.texels.len() {
            return 0;
        }
        let width = TILE_TEXTURE_WIDTH as usize;
        let first_row = self.uploaded / width;
        let end_row = self.texels.len().div_ceil(width);
        let mut rows = vec![[0f32; 4]; (end_row - first_row) * width];
        rows[..self.texels.len() - first_row * width]
            .copy_from_slice(&self.texels[first_row * width..]);
        let bytes: &[u8] = bytemuck::cast_slice(&rows);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: first_row as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TILE_TEXTURE_WIDTH * 16),
                rows_per_image: Some((end_row - first_row) as u32),
            },
            wgpu::Extent3d {
                width: TILE_TEXTURE_WIDTH,
                height: (end_row - first_row) as u32,
                depth_or_array_layers: 1,
            },
        );
        self.uploaded = self.texels.len();
        bytes.len() as u64
    }
}

fn unorm8(value: f32) -> u8 {
    (value * 255.).round().clamp(0., 255.) as u8
}

fn fixed(value: f32, scale: f32, bias: f32, what: &str) -> Result<u16, String> {
    let scaled = ((value + bias) * scale).round();
    if !(0. ..=65535.).contains(&scaled) {
        return Err(format!(
            "terrain_vertex_{what}_outside_packed_range value={value}"
        ));
    }
    Ok(scaled as u16)
}

/// Face index, shader class and rooted weight of a mesher-tagged normal.
fn pack_normal(normal: [f32; 3]) -> Result<(u16, u16, u16), String> {
    let length = normal.iter().map(|c| c * c).sum::<f32>().sqrt();
    let class = length.round();
    if !length.is_finite() || !(0. ..=4.).contains(&class) {
        return Err(format!(
            "terrain_vertex_normal_class_unpackable normal={normal:?}"
        ));
    }
    if class == 0. {
        return Ok((0, 0, 0));
    }
    let direction = normal.map(|c| c / length);
    let Some(face) = FACE_NORMALS
        .iter()
        .position(|axis| (0..3).all(|i| (axis[i] - direction[i]).abs() < 1e-3))
    else {
        return Err(format!(
            "terrain_vertex_normal_not_axis_aligned normal={normal:?}"
        ));
    };
    let weight = if class == 2. {
        ((length - 2.) * 5.).clamp(0., 1.)
    } else {
        0.
    };
    Ok((
        face as u16,
        class as u16,
        (weight * WEIGHT_SCALE).round() as u16,
    ))
}

/// Packs mesher vertices; `Err` names the first value outside the format.
pub(super) fn pack_vertices(
    vertices: &[Vertex],
    tiles: &mut TileTable,
) -> Result<Vec<TerrainVertex>, String> {
    let mut packed = Vec::with_capacity(vertices.len());
    for vertex in vertices {
        let (face, class, weight) = pack_normal(vertex.normal)?;
        let mut flags = face | class << FLAG_CLASS_SHIFT | weight << FLAG_WEIGHT_SHIFT;
        let tiled = vertex.overlay_tint[2] < 0.;
        let layer;
        let uv;
        if tiled {
            flags |= FLAG_TILED;
            layer = tiles.intern(
                vertex.texture_layer,
                vertex.overlay_uv,
                [vertex.overlay_tint[0], vertex.overlay_tint[1]],
            )?;
            uv = [
                fixed(vertex.uv[0], TILE_UV_SCALE, TILE_UV_BIAS, "tile_uv")?,
                fixed(vertex.uv[1], TILE_UV_SCALE, TILE_UV_BIAS, "tile_uv")?,
            ];
        } else {
            if vertex.overlay_uv != [-1.; 2] {
                return Err("terrain_vertex_overlay_unpackable".into());
            }
            layer = u16::try_from(vertex.texture_layer)
                .map_err(|_| "terrain_vertex_layer_outside_packed_range".to_string())?;
            if vertex.uv.iter().any(|v| *v < 0.) {
                flags |= FLAG_UNTEXTURED;
                uv = [0; 2];
            } else {
                if vertex.uv.iter().any(|v| *v > UV_LIMIT) {
                    return Err(format!(
                        "terrain_vertex_uv_outside_packed_range uv={:?}",
                        vertex.uv
                    ));
                }
                uv = vertex.uv.map(|v| (v * UV_SCALE).round() as u16);
            }
        }
        let light = if vertex.light.iter().all(|level| *level >= 0.) {
            vertex
                .light
                .map(|level| (level * LIGHT_SCALE).round().clamp(0., 240.) as u8)
        } else {
            [UNLIT; 2]
        };
        packed.push(TerrainVertex {
            position: [
                fixed(
                    vertex.position[0],
                    POSITION_SCALE,
                    POSITION_BIAS,
                    "position",
                )?,
                fixed(
                    vertex.position[1],
                    POSITION_SCALE,
                    POSITION_BIAS,
                    "position",
                )?,
                fixed(
                    vertex.position[2],
                    POSITION_SCALE,
                    POSITION_BIAS,
                    "position",
                )?,
            ],
            flags,
            color: [
                unorm8(vertex.color[0]),
                unorm8(vertex.color[1]),
                unorm8(vertex.color[2]),
                unorm8(vertex.alpha),
            ],
            light,
            layer,
            uv,
        });
    }
    Ok(packed)
}

/// Unpackable-by-default `Vertex::zeroed()` has a zero (valid-overlay) uv; tests
/// that need inert geometry use this instead.
#[cfg(test)]
pub(super) fn blank_vertices(count: usize) -> Vec<Vertex> {
    use bytemuck::Zeroable;
    vec![
        Vertex {
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            ..Vertex::zeroed()
        };
        count
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_is_twenty_bytes_in_the_documented_layout() {
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 20);
        let vertex = TerrainVertex {
            position: [1, 2, 3],
            flags: 4,
            color: [5, 6, 7, 8],
            light: [9, 10],
            layer: 0x0b0c,
            uv: [0x0d0e, 0x0f10],
        };
        let bytes = bytemuck::bytes_of(&vertex);
        assert_eq!(&bytes[0..6], [1, 0, 2, 0, 3, 0]);
        assert_eq!(&bytes[6..8], [4, 0]);
        assert_eq!(&bytes[8..12], [5, 6, 7, 8]);
        assert_eq!(&bytes[12..16], [9, 10, 0x0c, 0x0b]);
        assert_eq!(&bytes[16..20], [0x0e, 0x0d, 0x10, 0x0f]);
    }

    fn sample() -> Vertex {
        Vertex {
            position: [3.0625, 15.9375, 0.5],
            color: [61. / 255., 140. / 255., 1.],
            uv: [0.25, 0.8125],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            alpha: 1.,
            texture_layer: 7,
            light: [13.25, 4.0625],
            normal: [0., 1., 0.],
        }
    }

    fn pack_one(vertex: Vertex, tiles: &mut TileTable) -> TerrainVertex {
        pack_vertices(&[vertex], tiles).unwrap()[0]
    }

    #[test]
    fn mesher_quantities_round_trip_exactly() {
        let mut tiles = TileTable::default();
        let packed = pack_one(sample(), &mut tiles);
        let decoded = |q: u16| f32::from(q) / POSITION_SCALE - POSITION_BIAS;
        assert_eq!(
            packed.position.map(decoded),
            [3.0625, 15.9375, 0.5],
            "sixteenth positions are exact"
        );
        assert_eq!(packed.color, [61, 140, 255, 255]);
        assert_eq!(
            packed.light.map(|l| f32::from(l) / LIGHT_SCALE),
            [13.25, 4.0625]
        );
        assert_eq!(packed.layer, 7);
        let uv = packed.uv.map(|q| f32::from(q) / UV_SCALE);
        assert_eq!(
            uv,
            [0.25, 0.8125],
            "sprite uvs are multiples of 1/1024: exact"
        );
        assert_eq!(packed.flags, 1 | 1 << FLAG_CLASS_SHIFT);
    }

    #[test]
    fn class_face_and_rooted_weight_survive() {
        let mut tiles = TileTable::default();
        for (class, magnitude) in [(1u16, 1.), (3, 3.), (4, 4.)] {
            for (face, normal) in FACE_NORMALS.iter().enumerate() {
                let tagged = normal.map(|c| c * magnitude);
                let packed = pack_one(
                    Vertex {
                        normal: tagged,
                        ..sample()
                    },
                    &mut tiles,
                );
                assert_eq!(packed.flags & 7, face as u16);
                assert_eq!((packed.flags >> FLAG_CLASS_SHIFT) & 7, class);
                assert_eq!(packed.flags >> FLAG_WEIGHT_SHIFT, 0);
            }
        }
        for height in [0f32, 0.0625, 0.5, 1.] {
            let tagged = [0., 2. + 0.2 * height, 0.];
            let packed = pack_one(
                Vertex {
                    normal: tagged,
                    ..sample()
                },
                &mut tiles,
            );
            assert_eq!((packed.flags >> FLAG_CLASS_SHIFT) & 7, 2);
            let weight = f32::from(packed.flags >> FLAG_WEIGHT_SHIFT) / WEIGHT_SCALE;
            assert!((weight - height).abs() < 1e-6, "{height} -> {weight}");
        }
        let zero = pack_one(
            Vertex {
                normal: [0.; 3],
                ..sample()
            },
            &mut tiles,
        );
        assert_eq!(zero.flags & 0x7f, 0);
    }

    #[test]
    fn tiled_quads_share_one_table_entry_per_rectangle() {
        let mut tiles = TileTable::default();
        let tiled = |uv: [f32; 2]| Vertex {
            uv,
            overlay_uv: [0.125, 0.25],
            overlay_tint: [0.0625, 0.0625, -1.],
            texture_layer: 3,
            ..sample()
        };
        let a = pack_one(tiled([0., 0.]), &mut tiles);
        let b = pack_one(tiled([16., -15.]), &mut tiles);
        assert_eq!(a.layer, b.layer);
        assert_eq!(tiles.entry(a.layer), (3, [0.125, 0.25], [0.0625, 0.0625]));
        assert_eq!(a.flags & FLAG_TILED, FLAG_TILED);
        let decode = |q: u16| f32::from(q) / TILE_UV_SCALE - TILE_UV_BIAS;
        assert_eq!(b.uv.map(decode), [16., -15.]);
        let other = pack_one(
            Vertex {
                overlay_uv: [0.5, 0.25],
                ..tiled([0., 0.])
            },
            &mut tiles,
        );
        assert_ne!(other.layer, a.layer);
    }

    #[test]
    fn unlit_untextured_and_out_of_range_inputs() {
        let mut tiles = TileTable::default();
        let unlit = pack_one(
            Vertex {
                light: [-1.; 2],
                ..sample()
            },
            &mut tiles,
        );
        assert_eq!(unlit.light, [UNLIT; 2]);
        let bare = pack_one(
            Vertex {
                uv: [-1.; 2],
                ..sample()
            },
            &mut tiles,
        );
        assert_eq!(bare.flags & FLAG_UNTEXTURED, FLAG_UNTEXTURED);
        for bad in [
            Vertex {
                position: [24., 0., 0.],
                ..sample()
            },
            Vertex {
                position: [-8.5, 0., 0.],
                ..sample()
            },
            Vertex {
                normal: [0.7, 0.7, 0.],
                ..sample()
            },
            Vertex {
                overlay_uv: [0.5, 0.5],
                ..sample()
            },
            Vertex {
                uv: [2., 0.],
                ..sample()
            },
        ] {
            assert!(pack_vertices(&[bad], &mut tiles).is_err(), "{bad:?}");
        }
        assert!(pack_vertices(&blank_vertices(1), &mut tiles).is_ok());
    }
}
