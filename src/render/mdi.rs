use super::*;
use std::cell::Cell;

// wgpu 27 folded the old MULTI_DRAW_INDIRECT flag into the baseline API.
// COUNT additionally guarantees native (not loop-emulated) multi-draw.
pub(super) const FEATURES: wgpu::Features =
    wgpu::Features::MULTI_DRAW_INDIRECT_COUNT.union(wgpu::Features::INDIRECT_FIRST_INSTANCE);

pub(super) fn supported(features: wgpu::Features, enabled: bool) -> bool {
    enabled && features.contains(FEATURES)
}

/// Hi-Z needs one write-only `r32float` storage texture per pyramid level and
/// nothing else beyond the baseline compute the MDI path already requires. A
/// device that cannot do that keeps frustum-only culling.
pub(super) fn hiz_supported(
    r32float_storage: bool,
    max_storage_textures: u32,
    enabled: bool,
) -> bool {
    enabled && r32float_storage && max_storage_textures >= 1
}

/// Last-frame visibility is kept per hashed section slot. A collision only
/// merges two sections' histories: history decides *which phase* draws a
/// section, never *whether* a visible section is drawn.
pub(super) const HISTORY_SLOTS: usize = 1 << 16;
const COUNTER_WORDS: usize = 8;
pub(super) const COUNTER_BYTES: u64 = (2 * COUNTER_WORDS * 4) as u64;
/// Depth-space slack added to the pyramid's farthest depth before a box counts
/// as hidden: covers corner-projection and rasteriser interpolation error
/// (a few ulp of a 32-bit depth near 1.0).
const DEPTH_MARGIN: f32 = 2e-6;
const COUNTER_RING: usize = 3;

pub(super) fn section_slot(key: [i32; 3]) -> u32 {
    let mut hash = (key[0] as u32).wrapping_mul(0x9E37_79B1)
        ^ (key[1] as u32).wrapping_mul(0x85EB_CA6B)
        ^ (key[2] as u32).wrapping_mul(0xC2B2_AE35);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 13;
    hash & (HISTORY_SLOTS as u32 - 1)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Candidate {
    // w lanes carry the history slot (min) and are otherwise unused.
    min: [f32; 4],
    max: [f32; 4],
    // index_count, first_index, base_vertex (i32 bits), first_instance.
    draw: [u32; 4],
}

impl Candidate {
    pub fn new(
        mesh: &ResidentMesh,
        range: std::ops::Range<u32>,
        instance: u32,
        min: Vec3,
        max: Vec3,
        slot: u32,
    ) -> Option<Self> {
        if range.is_empty() {
            return None;
        }
        let vertices = mesh.vertices?;
        let indices = mesh.indices?;
        let stride = std::mem::size_of::<TerrainVertex>() as u64;
        assert_eq!(vertices.offset % stride, 0, "unaligned terrain base vertex");
        Some(Self {
            min: [min.x, min.y, min.z, f32::from_bits(slot)],
            max: [max.x, max.y, max.z, 0.],
            draw: [
                range.end - range.start,
                (indices.offset / 4) as u32 + range.start,
                (vertices.offset / stride) as u32,
                instance,
            ],
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CullUniform {
    planes: [[f32; 4]; 6],
    // candidate count, counter base word, history slot mask.
    count: [u32; 4],
    view_projection: [[f32; 4]; 4],
    // depth width, depth height, pyramid levels, depth margin.
    screen: [f32; 4],
}

/// Max-depth pyramid built from the scene depth buffer. Level 0 is half the
/// depth resolution; levels run down to 1x1.
struct Pyramid {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    levels: u32,
    steps: Vec<BuildStep>,
}

struct BuildStep {
    bind: wgpu::BindGroup,
    groups: [u32; 2],
}

struct CounterSlot {
    buffer: wgpu::Buffer,
    pending: Option<(u64, mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>)>,
}

pub(super) struct Indirect {
    pub candidates: [Vec<Candidate>; 2],
    inputs: [wgpu::Buffer; 2],
    outputs: [wgpu::Buffer; 2],
    uniforms: [wgpu::Buffer; 2],
    history: [wgpu::Buffer; 2],
    counters: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    bindings: [[wgpu::BindGroup; 2]; 2],
    frustum_pipeline: wgpu::ComputePipeline,
    phase1_pipeline: wgpu::ComputePipeline,
    phase2_pipeline: wgpu::ComputePipeline,
    from_depth: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    pyramid: Pyramid,
    hiz: bool,
    /// Diagnostic (`SOLARIS_CLIENT_HIZ=cold`): forget last frame's visibility
    /// every frame, so every section takes the phase-2 path as after the
    /// worst possible quick turn. Pixels must still be identical.
    cold: bool,
    size: [u32; 2],
    /// `history[parity]` is last frame's verdict, `history[1 - parity]` this
    /// frame's output. Flipped once per two-phase frame.
    parity: Cell<usize>,
    ring: Vec<CounterSlot>,
    active: Option<usize>,
    latest_frame: u64,
    pub latest: Value,
}

impl Indirect {
    pub fn new(
        device: &wgpu::Device,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        hiz: bool,
    ) -> Self {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let inputs = std::array::from_fn(|_| {
            buffer(
                "terrain coarse candidates",
                (mesh::MAX_CHUNKS * std::mem::size_of::<Candidate>()) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            )
        });
        let outputs = std::array::from_fn(|_| {
            buffer(
                "terrain culled indexed indirect arguments",
                (mesh::MAX_CHUNKS * 20) as u64,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_SRC,
            )
        });
        let uniforms = std::array::from_fn(|_| {
            buffer(
                "terrain frustum planes",
                std::mem::size_of::<CullUniform>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            )
        });
        let history = std::array::from_fn(|_| {
            buffer(
                "terrain hi-z section history",
                (HISTORY_SLOTS * 4) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            )
        });
        let counters = buffer(
            "terrain culling counters",
            COUNTER_BYTES,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let ring = (0..COUNTER_RING)
            .map(|_| CounterSlot {
                buffer: buffer(
                    "terrain culling counter readback",
                    COUNTER_BYTES,
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                ),
                pending: None,
            })
            .collect();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("conservative terrain frustum and hi-z culling"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mdi.wgsl").into()),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain culling layout"),
            entries: &[
                storage(0, true),
                storage(1, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(3, true),
                storage(4, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(6, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain culling pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let cull = |label, entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let frustum_pipeline = cull("terrain GPU frustum culling", "main");
        let phase1_pipeline = cull("terrain GPU phase 1 culling", "phase1");
        let phase2_pipeline = cull("terrain GPU phase 2 hi-z culling", "phase2");
        let hiz_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hi-z max-depth pyramid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hiz.wgsl").into()),
        });
        let build = |label, entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: &hiz_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let from_depth = build("hi-z level 0 from depth", "from_depth");
        let reduce = build("hi-z reduce", "reduce");
        let pyramid = Self::pyramid(device, &from_depth, &reduce, depth_view, width, height, hiz);
        let bindings = Self::bind(
            device, &layout, &inputs, &outputs, &uniforms, &history, &pyramid, &counters,
        );
        Self {
            candidates: std::array::from_fn(|_| Vec::with_capacity(mesh::MAX_CHUNKS)),
            inputs,
            outputs,
            uniforms,
            history,
            counters,
            layout,
            bindings,
            frustum_pipeline,
            phase1_pipeline,
            phase2_pipeline,
            from_depth,
            reduce,
            pyramid,
            hiz,
            cold: false,
            size: [width, height],
            parity: Cell::new(0),
            ring,
            active: None,
            latest_frame: 0,
            latest: Value::Null,
        }
    }

    /// Rebuild the pyramid and every bind group that references it. History
    /// is meaningless across a resolution change but harmless: it only picks
    /// which phase draws a section.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        self.pyramid = Self::pyramid(
            device,
            &self.from_depth,
            &self.reduce,
            depth_view,
            width,
            height,
            self.hiz,
        );
        self.bindings = Self::bind(
            device,
            &self.layout,
            &self.inputs,
            &self.outputs,
            &self.uniforms,
            &self.history,
            &self.pyramid,
            &self.counters,
        );
        self.size = [width, height];
    }

    #[allow(clippy::too_many_arguments)]
    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        inputs: &[wgpu::Buffer; 2],
        outputs: &[wgpu::Buffer; 2],
        uniforms: &[wgpu::Buffer; 2],
        history: &[wgpu::Buffer; 2],
        pyramid: &Pyramid,
        counters: &wgpu::Buffer,
    ) -> [[wgpu::BindGroup; 2]; 2] {
        std::array::from_fn(|material| {
            std::array::from_fn(|parity| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain culling inputs, history and indirect output"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: inputs[material].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: outputs[material].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: uniforms[material].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: history[parity].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: history[1 - parity].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(&pyramid.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: counters.as_entire_binding(),
                        },
                    ],
                })
            })
        })
    }

    fn pyramid(
        device: &wgpu::Device,
        from_depth: &wgpu::ComputePipeline,
        reduce: &wgpu::ComputePipeline,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        enabled: bool,
    ) -> Pyramid {
        // Disabled Hi-Z keeps a 1x1 placeholder so the culling layout is uniform.
        let base = if enabled {
            [(width / 2).max(1), (height / 2).max(1)]
        } else {
            [1, 1]
        };
        let levels = 32 - base[0].max(base[1]).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hi-z max-depth pyramid"),
            size: wgpu::Extent3d {
                width: base[0],
                height: base[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut steps = Vec::new();
        if enabled {
            let level_view = |level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("hi-z level"),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            };
            for level in 0..levels {
                let (pipeline, source) = if level == 0 {
                    (from_depth, None)
                } else {
                    (reduce, Some(level_view(level - 1)))
                };
                let destination = level_view(level);
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("hi-z build step"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                source.as_ref().unwrap_or(depth_view),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&destination),
                        },
                    ],
                });
                steps.push(BuildStep {
                    bind,
                    groups: [
                        (base[0] >> level).max(1).div_ceil(8),
                        (base[1] >> level).max(1).div_ceil(8),
                    ],
                });
            }
        }
        Pyramid {
            _texture: texture,
            view,
            levels,
            steps,
        }
    }

    pub fn set_cold_history(&mut self, cold: bool) {
        self.cold = cold;
    }
    /// True when the two-phase occlusion path is active.
    pub fn hiz(&self) -> bool {
        self.hiz
    }

    /// Pick this frame's counter readback slot. Call once per encoded frame.
    pub fn begin_frame(&mut self) {
        self.active = self.ring.iter().position(|slot| slot.pending.is_none());
    }

    /// Upload candidates and per-material culling parameters (once per frame).
    pub fn upload(&self, queue: &wgpu::Queue, matrix: Mat4) {
        let rows = matrix.transpose();
        let planes = [
            rows.w_axis + rows.x_axis,
            rows.w_axis - rows.x_axis,
            rows.w_axis + rows.y_axis,
            rows.w_axis - rows.y_axis,
            rows.z_axis,
            rows.w_axis - rows.z_axis,
        ]
        .map(|plane| plane.to_array());
        for material in 0..2 {
            let count = self.candidates[material].len() as u32;
            if count == 0 {
                continue;
            }
            queue.write_buffer(
                &self.inputs[material],
                0,
                bytemuck::cast_slice(&self.candidates[material]),
            );
            queue.write_buffer(
                &self.uniforms[material],
                0,
                bytemuck::bytes_of(&CullUniform {
                    planes,
                    count: [
                        count,
                        (material * COUNTER_WORDS) as u32,
                        HISTORY_SLOTS as u32 - 1,
                        0,
                    ],
                    view_projection: matrix.to_cols_array_2d(),
                    screen: [
                        self.size[0] as f32,
                        self.size[1] as f32,
                        self.pyramid.levels as f32,
                        DEPTH_MARGIN,
                    ],
                }),
            );
        }
    }

    fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>, pipeline: &wgpu::ComputePipeline) {
        pass.set_pipeline(pipeline);
        for material in 0..2 {
            let count = self.candidates[material].len() as u32;
            if count == 0 {
                continue;
            }
            pass.set_bind_group(0, &self.bindings[material][self.parity.get()], &[]);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
    }

    /// Frustum-only culling (Hi-Z disabled): one pass, every argument written.
    pub fn encode_frustum(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        encoder.clear_buffer(&self.counters, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("terrain frustum culling"),
                timestamp_writes,
            });
            self.dispatch(&mut pass, &self.frustum_pipeline);
        }
        self.copy_ring(encoder);
    }

    /// Phase 1: frustum && visible last frame. Draw these before the pyramid.
    pub fn encode_phase1(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        encoder.clear_buffer(&self.counters, 0, None);
        encoder.clear_buffer(&self.history[1 - self.parity.get()], 0, None);
        if self.cold {
            encoder.clear_buffer(&self.history[self.parity.get()], 0, None);
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("terrain phase 1 culling"),
            timestamp_writes,
        });
        self.dispatch(&mut pass, &self.phase1_pipeline);
    }

    /// Reduce the scene depth written by phase 1 into the max-depth pyramid.
    pub fn encode_pyramid(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hi-z pyramid build"),
            timestamp_writes,
        });
        for (level, step) in self.pyramid.steps.iter().enumerate() {
            pass.set_pipeline(if level == 0 {
                &self.from_depth
            } else {
                &self.reduce
            });
            pass.set_bind_group(0, &step.bind, &[]);
            pass.dispatch_workgroups(step.groups[0], step.groups[1], 1);
        }
    }

    /// Phase 2: re-test every candidate against the fresh pyramid, rewrite the
    /// indirect arguments so only sections not drawn in phase 1 remain, and
    /// publish the verdict as next frame's history.
    pub fn encode_phase2(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("terrain phase 2 hi-z culling"),
                timestamp_writes,
            });
            self.dispatch(&mut pass, &self.phase2_pipeline);
        }
        self.copy_ring(encoder);
        self.parity.set(1 - self.parity.get());
    }

    fn copy_ring(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(slot) = self.active {
            encoder.copy_buffer_to_buffer(
                &self.counters,
                0,
                &self.ring[slot].buffer,
                0,
                COUNTER_BYTES,
            );
        }
    }

    /// Copy this frame's counters into `destination` (capture exact-frame path).
    pub fn copy_counters(&self, encoder: &mut wgpu::CommandEncoder, destination: &wgpu::Buffer) {
        encoder.copy_buffer_to_buffer(&self.counters, 0, destination, 0, COUNTER_BYTES);
    }

    pub fn submitted(&mut self, frame: u64) {
        if let Some(index) = self.active.take() {
            let (tx, rx) = mpsc::sync_channel(1);
            self.ring[index]
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            self.ring[index].pending = Some((frame, rx));
        }
    }

    pub fn collect(&mut self) {
        for slot in &mut self.ring {
            let Some((frame, rx)) = &slot.pending else {
                continue;
            };
            let Ok(result) = rx.try_recv() else { continue };
            let frame = *frame;
            if result.is_ok() {
                if frame >= self.latest_frame {
                    let mapped = slot.buffer.slice(..).get_mapped_range();
                    let words: &[u32] = bytemuck::cast_slice(&mapped);
                    self.latest_frame = frame;
                    self.latest = counters_json(words, self.hiz, frame);
                }
                slot.buffer.unmap();
            }
            slot.pending = None;
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, material: usize) -> bool {
        let count = self.candidates[material].len() as u32;
        if count == 0 {
            return false;
        }
        pass.multi_draw_indexed_indirect(&self.outputs[material], 0, count);
        true
    }

    pub fn reserved_bytes(&self) -> u64 {
        let pyramid = (0..self.pyramid.levels)
            .map(|level| {
                let base = if self.hiz {
                    [(self.size[0] / 2).max(1), (self.size[1] / 2).max(1)]
                } else {
                    [1, 1]
                };
                u64::from((base[0] >> level).max(1)) * u64::from((base[1] >> level).max(1)) * 4
            })
            .sum::<u64>();
        self.inputs
            .iter()
            .chain(&self.outputs)
            .chain(&self.uniforms)
            .chain(&self.history)
            .map(wgpu::Buffer::size)
            .sum::<u64>()
            + pyramid
    }

    pub fn upload_bytes(&self) -> u64 {
        self.candidates
            .iter()
            .filter(|candidates| !candidates.is_empty())
            .map(|candidates| {
                (std::mem::size_of_val(candidates.as_slice()) + std::mem::size_of::<CullUniform>())
                    as u64
            })
            .sum()
    }
}

/// Exact GPU-side culling result for one encoded frame. `phase1`/`phase2` are
/// logical draw ranges and indices with non-zero instance count after culling;
/// `final_visible` = phase1 + phase2 - late_hidden is what an immediately
/// fresh pyramid would keep; `frustum_visible` = phase1 + phase2 + rejected is
/// what frustum-only culling would draw. Without Hi-Z everything lands in
/// `phase1`.
pub(super) fn counters_json(words: &[u32], hiz: bool, frame: u64) -> Value {
    let material = |m: usize| {
        let w = &words[m * COUNTER_WORDS..(m + 1) * COUNTER_WORDS];
        let (p1, p2, hidden, late) = ((w[0], w[1]), (w[2], w[3]), (w[4], w[5]), (w[6], w[7]));
        json!({
            "phase1_draws": p1.0, "phase1_indices": p1.1,
            "phase2_draws": p2.0, "phase2_indices": p2.1,
            "hiz_rejected_draws": hidden.0, "hiz_rejected_indices": hidden.1,
            "late_hidden_draws": late.0, "late_hidden_indices": late.1,
            "final_visible_draws": (p1.0 + p2.0).saturating_sub(late.0),
            "final_visible_indices": (p1.1 + p2.1).saturating_sub(late.1),
            "drawn_draws": p1.0 + p2.0,
            "drawn_indices": p1.1 + p2.1,
            "frustum_visible_draws": p1.0 + p2.0 + hidden.0,
            "frustum_visible_indices": p1.1 + p2.1 + hidden.1,
        })
    };
    json!({"frame_id": frame.to_string(), "hiz": hiz,
        "opaque": material(0), "cutout": material(1)})
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indirect_arguments_use_global_arena_offsets_and_first_instance() {
        let stride = std::mem::size_of::<TerrainVertex>() as u64;
        let mesh = ResidentMesh {
            vertices: Some(arena::Allocation {
                offset: stride * 13,
                size: stride * 4,
            }),
            indices: Some(arena::Allocation {
                offset: 128,
                size: 48,
            }),
            ranges: [0..6, 6..12, 12..12],
            faces: [None, None],
            transparent: Default::default(),
        };
        let candidate = Candidate::new(&mesh, 6..12, 17, Vec3::ZERO, Vec3::ONE, 0xABC).unwrap();
        assert_eq!(candidate.draw, [6, 38, 13, 17]);
        assert_eq!(candidate.min[3].to_bits(), 0xABC);
        assert!(Candidate::new(&mesh, 12..12, 17, Vec3::ZERO, Vec3::ONE, 0).is_none());
    }

    #[test]
    fn mdi_requires_both_features_and_respects_explicit_disable() {
        let all =
            wgpu::Features::MULTI_DRAW_INDIRECT_COUNT | wgpu::Features::INDIRECT_FIRST_INSTANCE;
        assert!(supported(all, true));
        assert!(!supported(all, false));
        assert!(!supported(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT, true));
        assert!(!supported(wgpu::Features::INDIRECT_FIRST_INSTANCE, true));
    }
}

#[test]
fn gpu_culling_matches_cpu_planes_and_overwrites_every_argument() {
    let renderer = pollster::block_on(Renderer::new_offscreen(
        32,
        32,
        PathBuf::from(".analysis/clientcpu-regression"),
    ))
    .unwrap();
    let Some(mut mdi) = renderer.mdi else {
        eprintln!("MDI hardware unavailable");
        return;
    };
    let cases = [
        ([-0.5, -0.5, 0.1], [0.5, 0.5, 0.9]),
        ([2., 0., 0.1], [3., 0.5, 0.9]),
        ([-0.5, -0.5, -2.], [0.5, 0.5, -1.]),
        ([1.005, 0., 0.1], [1.005, 0.5, 0.9]),
        ([f32::NAN, 0., 0.], [1., 1., 1.]),
        ([2., 0., 0.], [1., 1., 1.]),
    ];
    let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("indirect test readback"),
        size: cases.len() as u64 * 20,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    for matrix in [
        Mat4::IDENTITY,
        Mat4::from_translation(Vec3::new(-4., 0., 0.)),
    ] {
        mdi.candidates[0].clear();
        for (i, (min, max)) in cases.iter().enumerate() {
            mdi.candidates[0].push(Candidate {
                min: [min[0], min[1], min[2], 0.],
                max: [max[0], max[1], max[2], 0.],
                draw: [6, i as u32 * 6, i as u32 * 4, i as u32 + 2],
            });
        }
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        mdi.upload(&renderer.queue, matrix);
        mdi.encode_frustum(&mut encoder, None);
        encoder.copy_buffer_to_buffer(&mdi.outputs[0], 0, &readback, 0, readback.size());
        let submission = renderer.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
        renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        {
            let mapped = readback.slice(..).get_mapped_range();
            let args: &[u32] = bytemuck::cast_slice(&mapped);
            let frustum = Frustum::new(matrix);
            for (i, (min, max)) in cases.iter().enumerate() {
                let visible = frustum.intersects(Vec3::from_array(*min), Vec3::from_array(*max));
                assert_eq!(
                    &args[i * 5..i * 5 + 5],
                    &[
                        6,
                        u32::from(visible),
                        i as u32 * 6,
                        i as u32 * 4,
                        i as u32 + 2,
                    ],
                    "candidate {i}"
                );
            }
        }
        readback.unmap();
    }
}

#[cfg(test)]
mod hiz_tests {
    use super::*;

    const EDGE: u32 = 64;

    fn offscreen() -> Option<Renderer> {
        let renderer = pollster::block_on(Renderer::new_offscreen(
            32,
            32,
            PathBuf::from(".analysis/clientcpu-regression"),
        ))
        .unwrap();
        if renderer.mdi.as_ref().is_some_and(Indirect::hiz) {
            Some(renderer)
        } else {
            eprintln!("Hi-Z hardware unavailable");
            None
        }
    }

    fn depth_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hi-z test depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    /// Depth-only pipeline whose fullscreen triangle sits at a fixed depth.
    fn plane_pipeline(device: &wgpu::Device, depth: f32) -> wgpu::RenderPipeline {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hi-z test occluder plane"),
            source: wgpu::ShaderSource::Wgsl(
                "override PLANE: f32;
                 @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                     let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
                     return vec4<f32>(p * 2.0 - 1.0, PLANE, 1.0);
                 }"
                .into(),
            ),
        });
        let constants = [("PLANE", f64::from(depth))];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hi-z test occluder plane"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    zero_initialize_workgroup_memory: true,
                },
                buffers: &[],
            },
            fragment: None,
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        })
    }

    fn readback(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hi-z test readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn read_words(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: wgpu::CommandEncoder,
        buffers: &[&wgpu::Buffer],
    ) -> Vec<Vec<u32>> {
        let submission = queue.submit([encoder.finish()]);
        let receivers: Vec<_> = buffers
            .iter()
            .map(|buffer| {
                let (tx, rx) = mpsc::sync_channel(1);
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        tx.send(result).unwrap();
                    });
                rx
            })
            .collect();
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        buffers
            .iter()
            .zip(receivers)
            .map(|(buffer, rx)| {
                rx.recv().unwrap().unwrap();
                let words = bytemuck::cast_slice(&buffer.slice(..).get_mapped_range()).to_vec();
                buffer.unmap();
                words
            })
            .collect()
    }

    /// Phase-1 arguments, phase-2 arguments and exact counters of one frame.
    struct Frame {
        phase1: Vec<[u32; 5]>,
        phase2: Vec<[u32; 5]>,
        counters: Value,
    }

    #[allow(clippy::too_many_arguments)]
    fn two_phase_frame(
        renderer: &Renderer,
        mdi: &mut Indirect,
        depth: &wgpu::Texture,
        plane: Option<&wgpu::RenderPipeline>,
        scissor: [u32; 4],
        matrix: Mat4,
        count: usize,
    ) -> Frame {
        let device = &renderer.device;
        let args = readback(device, count as u64 * 20);
        let args2 = readback(device, count as u64 * 20);
        let counters = readback(device, COUNTER_BYTES);
        mdi.begin_frame();
        mdi.upload(&renderer.queue, matrix);
        let mut encoder = device.create_command_encoder(&Default::default());
        mdi.encode_phase1(&mut encoder, None);
        encoder.copy_buffer_to_buffer(&mdi.outputs[0], 0, &args, 0, args.size());
        {
            let view = depth.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hi-z test occluder"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(plane) = plane {
                pass.set_pipeline(plane);
                pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                pass.draw(0..3, 0..1);
            }
        }
        mdi.encode_pyramid(&mut encoder, None);
        mdi.encode_phase2(&mut encoder, None);
        encoder.copy_buffer_to_buffer(&mdi.outputs[0], 0, &args2, 0, args2.size());
        mdi.copy_counters(&mut encoder, &counters);
        let mut words = read_words(
            device,
            &renderer.queue,
            encoder,
            &[&args, &args2, &counters],
        );
        let counters = counters_json(&words.pop().unwrap(), true, 0);
        let split = |words: Vec<u32>| -> Vec<[u32; 5]> {
            words
                .chunks_exact(5)
                .map(|w| w.try_into().unwrap())
                .collect()
        };
        let phase2 = split(words.pop().unwrap());
        let phase1 = split(words.pop().unwrap());
        Frame {
            phase1,
            phase2,
            counters,
        }
    }

    fn candidate(i: usize, min: [f32; 3], max: [f32; 3]) -> Candidate {
        Candidate {
            min: [min[0], min[1], min[2], f32::from_bits(i as u32 * 7 + 1)],
            max: [max[0], max[1], max[2], 0.],
            draw: [6, i as u32 * 6, i as u32 * 4, i as u32 + 2],
        }
    }

    /// An occluder plane at 10 blocks covering the left half of the screen,
    /// sections behind it vanish, everything else keeps its arguments, and a
    /// section hidden last frame that becomes visible is drawn in the same
    /// frame (phase 2), never one frame late.
    #[test]
    fn hiz_occluder_plane_zeroes_hidden_sections_and_keeps_visible_untouched() {
        let Some(renderer) = offscreen() else { return };
        let device = &renderer.device;
        let matrix = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1., 0.05, 2048.);
        let plane_depth = {
            let clip = matrix * glam::Vec4::new(0., 0., -10., 1.);
            clip.z / clip.w
        };
        let depth = depth_target(device, EDGE, EDGE);
        let mut mdi = Indirect::new(
            device,
            &depth.create_view(&Default::default()),
            EDGE,
            EDGE,
            true,
        );
        let plane = plane_pipeline(device, plane_depth);
        let cases: [([f32; 3], [f32; 3], bool); 9] = [
            // (min, max, hidden by the left-half plane at z=-10)
            ([-6., -1., -30.], [-3., 1., -28.], true),
            ([3., -1., -30.], [6., 1., -28.], false),
            ([-1.5, -1., -6.], [-0.5, 1., -5.], false),
            ([-6., -1., -30.], [6., 1., -28.], false),
            ([f32::NAN, -1., -30.], [-3., 1., -28.], false),
            ([-1., -1., -1.], [1., 1., 1.], false),
            ([-10., -2., -100.], [-5., 2., -90.], true),
            ([100., -1., -20.], [110., 1., -19.], false),
            ([-9., -1., -20.], [-0.5, 1., -19.], true),
        ];
        let outside = 7;
        mdi.candidates[0].clear();
        for (i, (min, max, _)) in cases.iter().enumerate() {
            mdi.candidates[0].push(candidate(i, *min, *max));
        }
        let draw =
            |i: usize, on: bool| [6, u32::from(on), i as u32 * 6, i as u32 * 4, i as u32 + 2];
        let visible = |i: usize| i != outside && !cases[i].2;
        let frame = |mdi: &mut Indirect, with_plane: bool| {
            two_phase_frame(
                &renderer,
                mdi,
                &depth,
                with_plane.then_some(&plane),
                [0, 0, EDGE / 2, EDGE],
                matrix,
                cases.len(),
            )
        };

        // Frame 1: no history, so phase 1 draws nothing; phase 2 draws exactly
        // the sections the fresh pyramid cannot hide.
        let one = frame(&mut mdi, true);
        for i in 0..cases.len() {
            assert_eq!(one.phase1[i], draw(i, false), "frame 1 phase 1 section {i}");
            assert_eq!(
                one.phase2[i],
                draw(i, visible(i)),
                "frame 1 phase 2 section {i}"
            );
        }
        let hidden = cases.iter().filter(|case| case.2).count() as u64;
        let shown = (0..cases.len()).filter(|&i| visible(i)).count() as u64;
        assert_eq!(one.counters["opaque"]["phase1_draws"], 0);
        assert_eq!(one.counters["opaque"]["phase2_draws"], shown);
        assert_eq!(one.counters["opaque"]["hiz_rejected_draws"], hidden);
        assert_eq!(one.counters["opaque"]["late_hidden_draws"], 0);
        assert_eq!(one.counters["opaque"]["phase2_indices"], shown * 6);

        // Frame 2: last frame's visible set is drawn in phase 1, nothing is
        // left for phase 2, hidden sections stay zero in both.
        let two = frame(&mut mdi, true);
        for i in 0..cases.len() {
            assert_eq!(
                two.phase1[i],
                draw(i, visible(i)),
                "frame 2 phase 1 section {i}"
            );
            assert_eq!(two.phase2[i], draw(i, false), "frame 2 phase 2 section {i}");
        }
        assert_eq!(two.counters["opaque"]["final_visible_draws"], shown);

        // Frame 3: occluder gone. Phase 1 still draws the old set; the
        // previously hidden sections appear in phase 2 of this same frame.
        let three = frame(&mut mdi, false);
        for i in 0..cases.len() {
            assert_eq!(
                three.phase1[i],
                draw(i, visible(i)),
                "frame 3 phase 1 section {i}"
            );
            let newly = cases[i].2;
            assert_eq!(
                three.phase2[i],
                draw(i, newly),
                "frame 3 phase 2 section {i}"
            );
        }
        assert_eq!(three.counters["opaque"]["phase2_draws"], hidden);

        // Frame 4: occluder back. Hidden sections are drawn in phase 1 (stale
        // history) but flagged late-hidden; phase 2 draws nothing.
        let four = frame(&mut mdi, true);
        for i in 0..cases.len() {
            assert_eq!(
                four.phase2[i],
                draw(i, false),
                "frame 4 phase 2 section {i}"
            );
            assert_eq!(
                four.phase1[i],
                draw(i, i != outside),
                "frame 4 phase 1 section {i}"
            );
        }
        assert_eq!(four.counters["opaque"]["late_hidden_draws"], hidden);
        assert_eq!(four.counters["opaque"]["final_visible_draws"], shown);
    }

    /// A pixel in the odd tail row/column must reach the top of the pyramid,
    /// or a box covering only that pixel could be culled wrongly.
    #[test]
    fn hiz_pyramid_keeps_odd_tail_pixels_in_every_level() {
        let Some(renderer) = offscreen() else { return };
        let device = &renderer.device;
        let (width, height) = (37, 21);
        let depth = depth_target(device, width, height);
        let mdi = Indirect::new(
            device,
            &depth.create_view(&Default::default()),
            width,
            height,
            true,
        );
        // Read the 1x1 top level back.
        let levels = mdi.pyramid.levels;
        assert_eq!(levels, 5, "18x10 base needs 5 levels");
        let base = plane_pipeline(device, 0.25);
        let tail = plane_pipeline(device, 0.75);
        let top = readback(device, 4);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let view = depth.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hi-z odd tail"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.5),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&base);
            pass.draw(0..3, 0..1);
            // The very last pixel (odd tail in both axes).
            pass.set_pipeline(&tail);
            pass.set_scissor_rect(width - 1, height - 1, 1, 1);
            pass.draw(0..3, 0..1);
        }
        mdi.encode_pyramid(&mut encoder, None);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &mdi.pyramid._texture,
                mip_level: levels - 1,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &top,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let words = read_words(device, &renderer.queue, encoder, &[&top]);
        assert_eq!(f32::from_bits(words[0][0]), 0.75);
    }
}
