//! Far-terrain pass: column-sample tiles drawn behind the real chunks.
//!
//! Tiles live in fixed slots of one storage buffer. Each upload also meshes
//! the tile on the CPU into at most [`MAX_FACES`] face words (tops merged
//! along x where height and colour repeat, walls only toward lower or missing
//! neighbours, skirts on the tile edge) written to the slot's region of a
//! second buffer; the vertex shader pulls faces and samples (`far.wgsl`) and
//! each visible slot is one draw of six vertices per face. The pass runs right
//! after the sky with its own projection (far plane past the far distance)
//! and a cleared depth buffer; the opaque pass clears depth again, so real
//! chunks always draw over it. Fragments over loaded chunk columns are
//! discarded (seam) and the rest take the loaded terrain's fog: Fancy fog
//! by alpha into the sky drawn behind, otherwise toward the fog colour.
//!
//! Budgets: at most `CAPACITY` resident tiles (`CAPACITY * 5.2 KiB` of GPU
//! memory) and `UPLOADS_PER_FRAME` tile writes per frame.
use crate::api::{FarScene, Scene};
use crate::far::{EDGE, SAMPLES, WORDS};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

use super::{DEPTH, post::HDR, visibility::Frustum};

/// Resident tile slots; the 2 km quadtree needs about 2900, 8 km about 4300.
const CAPACITY: u32 = 4096;
const UPLOADS_PER_FRAME: usize = 128;
/// Loaded-chunk mask side, in chunk columns, centred on the camera chunk.
const MASK: u32 = 128;
/// Faces one tile can need: every top unmerged, one wall per adjacent pair
/// of samples (only the higher one walls) and a skirt per edge sample.
pub(super) const MAX_FACES: usize = SAMPLES + 2 * EDGE * (EDGE - 1) + 4 * EDGE;
/// Skirt depth below a tile-edge sample, scaled up for coarse levels.
const SKIRT: f32 = 24.;
const NEAR: f32 = 4.;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    view_projection: [f32; 16],
    camera: [f32; 4],
    mask: [i32; 4],
    /// Environmental fog start/end, daylight, linear output.
    fog: [f32; 4],
    /// Render-distance fog start/end, fog alpha, aerial coefficient.
    edge: [f32; 4],
    fog_color: [f32; 4],
    /// Camera block x, z: slot origins subtract it exactly in integers.
    anchor: [i32; 4],
    /// Camera position inside its block (x, z).
    fraction: [f32; 4],
}

/// Per-slot tile placement the vertex shader reads.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SlotInfo {
    origin: [i32; 2],
    level: u32,
    _pad: u32,
}

struct Slot {
    index: u32,
    /// Uploaded tile revision; `None` until the first upload lands.
    revision: Option<u64>,
    level: u8,
    origin: [i64; 2],
    height: [i32; 2],
    /// Face words in the slot's region.
    faces: u32,
}

/// Counters of the latest prepared frame.
#[derive(Default, Clone, Copy)]
pub(super) struct FarStats {
    pub resident_tiles: u64,
    pub visible_tiles: u64,
    pub uploads: u64,
    pub vertices: u64,
}

pub(super) struct FarTerrain {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    pool: wgpu::Buffer,
    faces: wgpu::Buffer,
    slot_info: wgpu::Buffer,
    mask: wgpu::Texture,
    mask_bytes: Vec<u8>,
    /// The mask bytes last written to the texture; `None` before the first.
    /// Compared by content: one column finishing its fade while another
    /// starts keeps the count but must still re-upload.
    mask_uploaded: Option<Vec<u8>>,
    slots: HashMap<(u8, i32, i32), Slot>,
    free: Vec<u32>,
    scene_revision: Option<u64>,
    /// This frame's visible slots: (slot index, face count).
    visible: Vec<(u32, u32)>,
    mesh_scratch: Vec<u32>,
    pub stats: FarStats,
    /// Compute generation for program-sharing servers.
    pub generator: super::far_generate::FarGenerator,
}

impl FarTerrain {
    pub fn new(device: &wgpu::Device, compute: bool) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("far terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("far.wgsl").into()),
        });
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("far terrain"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(3),
                storage(4),
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far terrain uniform"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pool = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far terrain tile pool"),
            size: u64::from(CAPACITY) * (WORDS as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let faces = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far terrain tile faces"),
            size: u64::from(CAPACITY) * (MAX_FACES as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let slot_info = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far terrain slot placement"),
            size: u64::from(CAPACITY) * std::mem::size_of::<SlotInfo>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask_bytes = vec![0u8; (MASK * MASK) as usize];
        // Written on the first prepared frame (`mask_uploaded` starts empty).
        let mask = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("far terrain loaded chunk mask"),
            size: wgpu::Extent3d {
                width: MASK,
                height: MASK,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mask_view = mask.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("far terrain"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pool.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: faces.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: slot_info.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("far terrain"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("far terrain"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("far_vertex"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[("MAX_FACES", MAX_FACES as f64)],
                    ..Default::default()
                },
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("far_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            bind,
            uniform,
            pool,
            faces,
            slot_info,
            mask,
            mask_bytes,
            mask_uploaded: None,
            slots: HashMap::new(),
            free: (0..CAPACITY).rev().collect(),
            scene_revision: None,
            visible: Vec::new(),
            mesh_scratch: Vec::with_capacity(MAX_FACES),
            stats: FarStats::default(),
            generator: super::far_generate::FarGenerator::new(compute),
        }
    }

    /// Sync tiles, cull them and write this frame's uniform. Returns whether
    /// the pass has anything to draw. `linear_output` is the Fancy path;
    /// `aerial` its aerial-perspective coefficient (shader.wgsl `fogged`).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        scene: &Scene,
        view: Mat4,
        fov_y: f32,
        aspect: f32,
        linear_output: bool,
        aerial: f32,
        resident_columns: impl Iterator<Item = [i32; 2]>,
    ) -> bool {
        let far = &scene.far;
        self.stats = FarStats::default();
        if far.distance <= 0. {
            self.release_all();
            return false;
        }
        self.sync(far);
        let mut uploads = 0;
        for tile in &far.tiles {
            if uploads == UPLOADS_PER_FRAME {
                break;
            }
            let Some(slot) = self.slots.get_mut(&(tile.level, tile.x, tile.z)) else {
                continue;
            };
            if slot.revision == Some(tile.revision) {
                continue;
            }
            queue.write_buffer(
                &self.pool,
                u64::from(slot.index) * WORDS as u64 * 4,
                bytemuck::cast_slice(tile.data.as_slice()),
            );
            mesh(&tile.data, &mut self.mesh_scratch);
            if !self.mesh_scratch.is_empty() {
                queue.write_buffer(
                    &self.faces,
                    u64::from(slot.index) * MAX_FACES as u64 * 4,
                    bytemuck::cast_slice(&self.mesh_scratch),
                );
            }
            queue.write_buffer(
                &self.slot_info,
                u64::from(slot.index) * std::mem::size_of::<SlotInfo>() as u64,
                bytemuck::bytes_of(&SlotInfo {
                    origin: slot.origin.map(|v| v as i32),
                    level: u32::from(slot.level),
                    _pad: 0,
                }),
            );
            slot.faces = self.mesh_scratch.len() as u32;
            slot.revision = Some(tile.revision);
            slot.height = heights(&tile.data);
            uploads += 1;
        }
        let camera = scene.camera_position;
        let projection = Mat4::perspective_rh(fov_y, aspect, NEAR, far.distance * 1.5 + 512.);
        let view_projection = projection * view;
        let frustum = Frustum::new(view_projection);
        let env = &scene.environment;
        // Fancy fog dissolves into the sky: a tile wholly past a full-strength
        // fog end is invisible. Both fog distances exceed the horizontal one.
        let fogged_out = if linear_output && env.fog_alpha >= 1. {
            [
                (env.fog_start < env.fog_end).then_some(env.fog_end),
                (env.render_fog_start < env.render_fog_end).then_some(env.render_fog_end),
            ]
            .into_iter()
            .flatten()
            .reduce(f32::min)
        } else {
            None
        };
        self.visible.clear();
        let mut faces = 0u64;
        for slot in self.slots.values() {
            if slot.revision.is_none() || slot.faces == 0 {
                continue;
            }
            let size = (EDGE as f32) * f32::from(1u16 << slot.level);
            let origin = [
                (slot.origin[0] as f64 - camera[0]) as f32,
                (slot.origin[1] as f64 - camera[2]) as f32,
            ];
            let low = slot.height[0] as f32 - camera[1] as f32 - SKIRT * size / 16.;
            let high = slot.height[1] as f32 + 1. - camera[1] as f32;
            let nearest = [origin[0], origin[1]].map(|v| v.max(-(v + size)).max(0.));
            if fogged_out.is_some_and(|end| nearest[0].hypot(nearest[1]) >= end)
                || !frustum.intersects(
                    Vec3::new(origin[0], low, origin[1]),
                    Vec3::new(origin[0] + size, high, origin[1] + size),
                )
            {
                continue;
            }
            faces += u64::from(slot.faces);
            self.visible.push((slot.index, slot.faces));
        }
        let camera_chunk = [camera[0], camera[2]].map(|v| (v / 16.).floor() as i32);
        self.update_mask(queue, camera_chunk, resident_columns);
        let daylight = scene.lightmap.as_ref().map_or(1., |lightmap| {
            // Lightmap texel (block 0, sky 15): the open-sky brightness.
            let texel = &lightmap.rgba[(15 * 16) * 4..(15 * 16) * 4 + 3];
            texel.iter().map(|v| f32::from(*v)).fold(0., f32::max) / 255.
        });
        let anchor = [camera[0].floor(), camera[2].floor()];
        let uniform = Uniform {
            view_projection: view_projection.to_cols_array(),
            camera: [
                camera[1] as f32,
                (camera[0] - f64::from(camera_chunk[0]) * 16.) as f32,
                (camera[2] - f64::from(camera_chunk[1]) * 16.) as f32,
                SKIRT,
            ],
            mask: [MASK as i32 / 2, MASK as i32 / 2, MASK as i32, 1],
            fog: [
                env.fog_start,
                env.fog_end,
                daylight,
                f32::from(u8::from(linear_output)),
            ],
            edge: [
                env.render_fog_start,
                env.render_fog_end,
                env.fog_alpha,
                aerial,
            ],
            fog_color: [env.fog_color[0], env.fog_color[1], env.fog_color[2], 1.],
            anchor: [anchor[0] as i32, anchor[1] as i32, 0, 0],
            fraction: [
                (camera[0] - anchor[0]) as f32,
                (camera[2] - anchor[1]) as f32,
                0.,
                0.,
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.stats.resident_tiles = self.slots.len() as u64;
        self.stats.visible_tiles = self.visible.len() as u64;
        self.stats.uploads = uploads as u64;
        self.stats.vertices = faces * 6;
        !self.visible.is_empty()
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        // Direct draws: every backend offsets `vertex_index` by the first
        // vertex, which selects the slot's face region.
        for &(slot, faces) in &self.visible {
            let first = slot * MAX_FACES as u32 * 6;
            pass.draw(first..first + faces * 6, 0..1);
        }
    }

    /// Bind slots to the scene's tiles when the list changed.
    fn sync(&mut self, far: &FarScene) {
        if self.scene_revision == Some(far.revision) {
            return;
        }
        self.scene_revision = Some(far.revision);
        let wanted: HashMap<(u8, i32, i32), ()> = far
            .tiles
            .iter()
            .map(|tile| ((tile.level, tile.x, tile.z), ()))
            .collect();
        let stale: Vec<_> = self
            .slots
            .keys()
            .filter(|key| !wanted.contains_key(key))
            .copied()
            .collect();
        for key in stale {
            if let Some(slot) = self.slots.remove(&key) {
                self.free.push(slot.index);
            }
        }
        // Nearest first: when slots run out, the farthest tiles wait.
        for tile in &far.tiles {
            let key = (tile.level, tile.x, tile.z);
            if self.slots.contains_key(&key) {
                continue;
            }
            let Some(index) = self.free.pop() else {
                break;
            };
            let size = EDGE as i64 * (1 << tile.level);
            self.slots.insert(
                key,
                Slot {
                    index,
                    revision: None,
                    level: tile.level,
                    origin: [i64::from(tile.x) * size, i64::from(tile.z) * size],
                    height: [0, 0],
                    faces: 0,
                },
            );
        }
    }

    fn release_all(&mut self) {
        for (_, slot) in self.slots.drain() {
            self.free.push(slot.index);
        }
        self.scene_revision = None;
        self.visible.clear();
    }

    fn update_mask(
        &mut self,
        queue: &wgpu::Queue,
        camera_chunk: [i32; 2],
        columns: impl Iterator<Item = [i32; 2]>,
    ) {
        let half = MASK as i32 / 2;
        self.mask_bytes.fill(0);
        for [x, z] in columns {
            let (u, v) = (x - camera_chunk[0] + half, z - camera_chunk[1] + half);
            if (0..MASK as i32).contains(&u) && (0..MASK as i32).contains(&v) {
                self.mask_bytes[(v as u32 * MASK + u as u32) as usize] = 255;
            }
        }
        if self.mask_uploaded.as_ref() == Some(&self.mask_bytes) {
            return;
        }
        self.mask_uploaded
            .get_or_insert_with(Vec::new)
            .clone_from(&self.mask_bytes);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.mask,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.mask_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(MASK),
                rows_per_image: Some(MASK),
            },
            wgpu::Extent3d {
                width: MASK,
                height: MASK,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn metrics(&self) -> serde_json::Value {
        serde_json::json!({
            "resident_tiles": self.stats.resident_tiles,
            "visible_tiles": self.stats.visible_tiles,
            "uploads": self.stats.uploads,
            "vertices": self.stats.vertices,
            "gpu_pool_bytes": u64::from(CAPACITY) * (WORDS + MAX_FACES) as u64 * 4,
            "generation": self.generator.metrics(),
        })
    }
}

/// Face word: sample `i` (bits 0-3), `j` (4-7), face (8-10: 0 top, 1 +x,
/// 2 -x, 3 +z, 4 -z) and run length - 1 along +x (11-14, tops only).
fn face(i: usize, j: usize, face: u32, run: usize) -> u32 {
    i as u32 | (j as u32) << 4 | face << 8 | (run as u32 - 1) << 11
}

/// One tile's faces into `out` (at most [`MAX_FACES`]): tops merged along x
/// while height and colour repeat, a wall toward each lower or missing
/// neighbour inside the tile and a skirt toward every neighbour outside it
/// (`far.wgsl` derives the wall bottoms the same way).
fn mesh(data: &[u32; WORDS], out: &mut Vec<u32>) {
    out.clear();
    let sample = |i: usize, j: usize| {
        let index = (j * EDGE + i) * 2;
        let (height, source) = crate::far::unpack(data[index]);
        (source != crate::far::Source::Missing).then_some((height, data[index + 1]))
    };
    for j in 0..EDGE {
        let mut i = 0;
        while i < EDGE {
            let Some(top) = sample(i, j) else {
                i += 1;
                continue;
            };
            let mut run = 1;
            while i + run < EDGE && sample(i + run, j) == Some(top) {
                run += 1;
            }
            out.push(face(i, j, 0, run));
            i += run;
        }
    }
    for j in 0..EDGE {
        for i in 0..EDGE {
            let Some((height, _)) = sample(i, j) else {
                continue;
            };
            for (side, di, dj) in [(1, 1, 0), (2, -1, 0), (3, 0, 1), (4, 0, -1)] {
                let (ni, nj) = (i as isize + di, j as isize + dj);
                let inside = (0..EDGE as isize).contains(&ni) && (0..EDGE as isize).contains(&nj);
                let wall =
                    !inside || sample(ni as usize, nj as usize).is_none_or(|(low, _)| low < height);
                if wall {
                    out.push(face(i, j, side, 1));
                }
            }
        }
    }
    debug_assert!(out.len() <= MAX_FACES);
}

/// Lowest and highest present sample top in a tile.
fn heights(data: &[u32; WORDS]) -> [i32; 2] {
    let mut range = [i32::MAX, i32::MIN];
    for sample in data.chunks_exact(2) {
        let (height, source) = crate::far::unpack(sample[0]);
        if source != crate::far::Source::Missing {
            range = [range[0].min(height), range[1].max(height)];
        }
    }
    if range[0] > range[1] { [0, 0] } else { range }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::far::{Source, pack};

    fn tile(height: impl Fn(usize, usize) -> Option<i32>) -> [u32; WORDS] {
        let mut data = [0u32; WORDS];
        for j in 0..EDGE {
            for i in 0..EDGE {
                if let Some(h) = height(i, j) {
                    data[(j * EDGE + i) * 2] = pack(h, Source::Stored);
                    data[(j * EDGE + i) * 2 + 1] = 0xff00_ff00;
                }
            }
        }
        data
    }

    fn count(faces: &[u32], side: u32) -> usize {
        faces.iter().filter(|f| (*f >> 8) & 7 == side).count()
    }

    #[test]
    fn flat_tile_merges_rows_and_keeps_only_edge_skirts() {
        let mut faces = Vec::new();
        mesh(&tile(|_, _| Some(64)), &mut faces);
        assert_eq!(count(&faces, 0), EDGE, "one full-width top per row");
        assert!(
            faces
                .iter()
                .filter(|f| (*f >> 8) & 7 == 0)
                .all(|f| (f >> 11) + 1 == EDGE as u32)
        );
        assert_eq!(faces.len(), EDGE + 4 * EDGE, "tops plus the 64 edge skirts");
    }

    #[test]
    fn walls_face_only_lower_or_missing_neighbours() {
        // A step down along +x at column 8 and a hole at (5, 5).
        let mut faces = Vec::new();
        mesh(
            &tile(|i, j| ((i, j) != (5, 5)).then_some(if i < 8 { 70 } else { 60 })),
            &mut faces,
        );
        let walls: Vec<(u32, u32, u32)> = faces
            .iter()
            .filter(|f| (*f >> 8) & 7 != 0)
            .map(|f| (f & 15, (f >> 4) & 15, (f >> 8) & 7))
            .collect();
        for j in 0..EDGE as u32 {
            assert!(walls.contains(&(7, j, 1)), "higher side of the step walls");
            assert!(!walls.contains(&(8, j, 2)), "lower side does not");
        }
        for wall in [(4, 5, 1), (6, 5, 2), (5, 4, 3), (5, 6, 4)] {
            assert!(walls.contains(&wall), "{wall:?} walls toward the hole");
        }
        assert_eq!(
            walls.len(),
            EDGE + 4 + 4 * EDGE,
            "step, hole and edge skirts only"
        );
    }

    #[test]
    fn worst_case_tile_fits_the_face_region() {
        let mut faces = Vec::new();
        mesh(
            &tile(|i, j| Some(if (i + j) % 2 == 0 { 100 } else { 0 })),
            &mut faces,
        );
        assert!(faces.len() <= MAX_FACES, "{} faces", faces.len());
        assert_eq!(count(&faces, 0), SAMPLES, "alternating heights never merge");
    }
}
