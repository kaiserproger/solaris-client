//! Far-terrain pass: column-sample tiles drawn behind the real chunks.
//!
//! Tiles live in fixed slots of one storage buffer; the vertex shader pulls
//! samples and builds tops, walls and edge skirts (`far.wgsl`), so a tile
//! update is one 2 KiB buffer write and there is no CPU meshing. The pass runs
//! right after the sky with its own projection (far plane past the far
//! distance) and a cleared depth buffer; the opaque pass clears depth again,
//! so real chunks always draw over it. Fragments over loaded chunk columns are
//! discarded (seam) and the far edge fades into the sky by alpha.
//!
//! Budgets: at most `CAPACITY` resident tiles (`CAPACITY * 2 KiB` of GPU
//! memory) and `UPLOADS_PER_FRAME` tile writes per frame.
use crate::api::{FarScene, Scene};
use crate::far::{EDGE, SAMPLES, WORDS};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

use super::{post::HDR, visibility::Frustum, DEPTH};

/// Resident tile slots; the 8 km quadtree needs about 1100.
const CAPACITY: u32 = 2048;
const UPLOADS_PER_FRAME: usize = 128;
/// Loaded-chunk mask side, in chunk columns, centred on the camera chunk.
const MASK: u32 = 128;
const VERTICES_PER_TILE: u32 = SAMPLES as u32 * 30;
/// Skirt depth below a tile-edge sample, scaled up for coarse levels.
const SKIRT: f32 = 24.;
const NEAR: f32 = 4.;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    view_projection: [f32; 16],
    camera: [f32; 4],
    mask: [i32; 4],
    fog: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    origin: [f32; 2],
    slot: u32,
    level: u32,
}

struct Slot {
    index: u32,
    /// Uploaded tile revision; `None` until the first upload lands.
    revision: Option<u64>,
    level: u8,
    origin: [i64; 2],
    height: [i32; 2],
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
    instances: wgpu::Buffer,
    mask: wgpu::Texture,
    mask_bytes: Vec<u8>,
    mask_key: Option<([i32; 2], usize)>,
    slots: HashMap<(u8, i32, i32), Slot>,
    free: Vec<u32>,
    scene_revision: Option<u64>,
    instance_data: Vec<Instance>,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
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
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far terrain visible tiles"),
            size: u64::from(CAPACITY) * std::mem::size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask_bytes = vec![0u8; (MASK * MASK) as usize];
        // Written on the first prepared frame (`mask_key` starts empty).
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
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32x2],
                }],
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
            instances,
            mask,
            mask_bytes,
            mask_key: None,
            slots: HashMap::new(),
            free: (0..CAPACITY).rev().collect(),
            scene_revision: None,
            instance_data: Vec::new(),
            stats: FarStats::default(),
            generator: super::far_generate::FarGenerator::new(compute),
        }
    }

    /// Sync tiles, cull them and write this frame's uniform. Returns whether
    /// the pass has anything to draw.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        scene: &Scene,
        view: Mat4,
        fov_y: f32,
        aspect: f32,
        linear_output: bool,
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
            slot.revision = Some(tile.revision);
            slot.height = heights(&tile.data);
            uploads += 1;
        }
        let camera = scene.camera_position;
        let projection = Mat4::perspective_rh(fov_y, aspect, NEAR, far.distance * 1.5 + 512.);
        let view_projection = projection * view;
        let frustum = Frustum::new(view_projection);
        self.instance_data.clear();
        for slot in self.slots.values() {
            if slot.revision.is_none() {
                continue;
            }
            let size = (EDGE as f32) * f32::from(1u16 << slot.level);
            let origin = [
                (slot.origin[0] as f64 - camera[0]) as f32,
                (slot.origin[1] as f64 - camera[2]) as f32,
            ];
            let low = slot.height[0] as f32 - camera[1] as f32 - SKIRT * size / 16.;
            let high = slot.height[1] as f32 + 1. - camera[1] as f32;
            if !frustum.intersects(
                Vec3::new(origin[0], low, origin[1]),
                Vec3::new(origin[0] + size, high, origin[1] + size),
            ) {
                continue;
            }
            self.instance_data.push(Instance {
                origin,
                slot: slot.index,
                level: u32::from(slot.level),
            });
        }
        if !self.instance_data.is_empty() {
            queue.write_buffer(
                &self.instances,
                0,
                bytemuck::cast_slice(&self.instance_data),
            );
        }
        let camera_chunk = [camera[0], camera[2]].map(|v| (v / 16.).floor() as i32);
        self.update_mask(queue, camera_chunk, resident_columns);
        let daylight = scene.lightmap.as_ref().map_or(1., |lightmap| {
            // Lightmap texel (block 0, sky 15): the open-sky brightness.
            let texel = &lightmap.rgba[(15 * 16) * 4..(15 * 16) * 4 + 3];
            texel.iter().map(|v| f32::from(*v)).fold(0., f32::max) / 255.
        });
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
                far.distance * 0.75,
                far.distance,
                daylight,
                f32::from(u8::from(linear_output)),
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.stats.resident_tiles = self.slots.len() as u64;
        self.stats.visible_tiles = self.instance_data.len() as u64;
        self.stats.uploads = uploads as u64;
        self.stats.vertices = self.instance_data.len() as u64 * u64::from(VERTICES_PER_TILE);
        !self.instance_data.is_empty()
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..VERTICES_PER_TILE, 0..self.instance_data.len() as u32);
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
                },
            );
        }
    }

    fn release_all(&mut self) {
        for (_, slot) in self.slots.drain() {
            self.free.push(slot.index);
        }
        self.scene_revision = None;
        self.instance_data.clear();
    }

    fn update_mask(
        &mut self,
        queue: &wgpu::Queue,
        camera_chunk: [i32; 2],
        columns: impl Iterator<Item = [i32; 2]>,
    ) {
        let half = MASK as i32 / 2;
        self.mask_bytes.fill(0);
        let mut count = 0;
        for [x, z] in columns {
            let (u, v) = (x - camera_chunk[0] + half, z - camera_chunk[1] + half);
            if (0..MASK as i32).contains(&u) && (0..MASK as i32).contains(&v) {
                let texel = &mut self.mask_bytes[(v as u32 * MASK + u as u32) as usize];
                count += usize::from(*texel == 0);
                *texel = 255;
            }
        }
        if self.mask_key == Some((camera_chunk, count)) {
            return;
        }
        self.mask_key = Some((camera_chunk, count));
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
            "gpu_pool_bytes": u64::from(CAPACITY) * WORDS as u64 * 4,
            "generation": self.generator.metrics(),
        })
    }
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
    if range[0] > range[1] {
        [0, 0]
    } else {
        range
    }
}
