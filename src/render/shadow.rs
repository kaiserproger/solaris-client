use super::*;

pub(super) const SIZE: u32 = 2048;
pub(super) const RADIUS: f32 = 80.;

pub(super) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Fancy shadow and opaque depth"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

/// One `DrawIndexedIndirect` argument. Matches the `DrawIndexed` layout the
/// main-pass `mdi.wgsl` compute shader writes, reused here so both passes
/// share a single argument shape.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ShadowDraw {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// CPU-prepacked indirect arguments in the direct caster walk's order.
/// Both materials use the same alpha-tested pipeline, so one batch suffices.
/// Casters are not culled by the camera frustum (off-camera casters must not
/// pop), only by the light's orthographic box and light-facing opaque groups.
pub(super) struct ShadowBatch {
    pub draws: Vec<ShadowDraw>,
    pub buffer: wgpu::Buffer,
    /// Per-material logical chunk-range draws the batch submits.
    pub logical_draws: [u64; 2],
    /// Total shadow-pass terrain indices submitted (both materials).
    pub submitted_indices: u64,
}

impl ShadowBatch {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            // One entry per (chunk, material) plus the fixture's two slots.
            draws: Vec::with_capacity(mesh::MAX_CHUNKS * 2 + 2),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow caster indirect arguments"),
                size: ((mesh::MAX_CHUNKS * 2 + 2) * std::mem::size_of::<ShadowDraw>()) as u64,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            logical_draws: [0; 2],
            submitted_indices: 0,
        }
    }
    pub(super) fn reserved_bytes(&self) -> u64 {
        self.buffer.size()
    }
    fn clear(&mut self) {
        self.draws.clear();
        self.logical_draws = [0; 2];
        self.submitted_indices = 0;
    }
}

/// One shadow-pass index range of a caster section (or the fixture).
pub(super) struct ShadowRun {
    /// `None` is the fixture mesh (instance slot 1).
    pub key: Option<[i32; 3]>,
    pub instance: u32,
    pub material: usize,
    pub range: std::ops::Range<u32>,
}

pub(super) struct Shadow {
    _map: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub opaque_depth: wgpu::Texture,
    pub bind: wgpu::BindGroup,
    pub pipeline: wgpu::RenderPipeline,
    pub instances: wgpu::Buffer,
    pub chunks: Vec<[i32; 3]>,
    /// Caster index ranges in draw order: the single source of truth for the
    /// direct walk and the indirect batch.
    pub runs: Vec<ShadowRun>,
    origins: Vec<[f32; 4]>,
    /// CPU-prepacked indirect arguments mirroring the direct caster walk.
    /// `None`-armed (no-op) unless the renderer constructed it for native MDI.
    pub batch: Option<ShadowBatch>,
}

impl Shadow {
    pub fn new(
        device: &wgpu::Device,
        camera: &wgpu::BindGroupLayout,
        atlas: &wgpu::BindGroupLayout,
    ) -> Self {
        let map = Self::depth(device, SIZE, SIZE, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let view = map.create_view(&Default::default());
        let opaque_depth = Self::depth(device, 1, 1, wgpu::TextureUsages::COPY_DST);
        let bind = Self::binding(device, &view, &opaque_depth);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fancy alpha-tested directional shadow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow caster layout"),
            bind_group_layouts: &[camera, atlas],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x2,4=>Float32x3,5=>Float32,6=>Uint32,8=>Float32x2,9=>Float32x3];
        let instance_attributes = wgpu::vertex_attr_array![7=>Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sun moon alpha-tested shadow map"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("shadow_vertex"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::api::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("shadow_fragment"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.,
                    clamp: 0.,
                },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow caster origins"),
            size: ((mesh::MAX_CHUNKS + 2) * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            _map: map,
            view,
            opaque_depth,
            bind,
            pipeline,
            instances,
            chunks: Vec::with_capacity(mesh::MAX_CHUNKS),
            runs: Vec::with_capacity(mesh::MAX_CHUNKS * 2 + 2),
            origins: Vec::with_capacity(mesh::MAX_CHUNKS + 2),
            batch: None,
        }
    }
    fn depth(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Fancy depth texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }
    fn binding(
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        depth: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("soft shadow comparison"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fancy scene textures"),
            layout: &layout(device),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &depth.create_view(&Default::default()),
                    ),
                },
            ],
        })
    }
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.opaque_depth = Self::depth(device, width, height, wgpu::TextureUsages::COPY_DST);
        self.bind = Self::binding(device, &self.view, &self.opaque_depth);
    }
    /// `light` points from the scene toward the sun/moon; `matrix` is the
    /// camera-relative orthographic light projection used by the shadow pass.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        chunks: &BTreeMap<[i32; 3], ResidentChunk>,
        scene: &Scene,
        fixture: Option<&ResidentMesh>,
        matrix: Mat4,
        light: Vec3,
    ) {
        self.chunks.clear();
        self.runs.clear();
        self.origins.clear();
        self.origins.push([0., 0., 0., 1.]);
        let origin = relative(scene.origin, scene.camera_position);
        self.origins.push([origin.x, origin.y, origin.z, 1.]);
        let armed = self.batch.is_some();
        if let Some(batch) = self.batch.as_mut() {
            batch.clear();
        }
        let vertex_stride = std::mem::size_of::<crate::api::Vertex>() as u64;
        // Casters need not be inside the camera frustum (that would make
        // shadows pop) but must reach the light's orthographic box.
        for (&key, chunk) in chunks {
            let offset = relative(chunk.origin, scene.camera_position);
            let min = Vec3::from_array(chunk.bounds_min) + offset;
            let max = Vec3::from_array(chunk.bounds_max) + offset;
            if chunk.mesh.indices.is_none()
                || chunk.visibility.is_none()
                || ((min + max) * 0.5).length_squared() >= (RADIUS + 32.).powi(2)
                || outside_light_box(&matrix, min, max)
            {
                continue;
            }
            let first = self.runs.len();
            let instance = self.origins.len() as u32;
            for material in 0..2 {
                let runs = &mut self.runs;
                let mut push = |range| {
                    runs.push(ShadowRun {
                        key: Some(key),
                        instance,
                        material,
                        range,
                    })
                };
                if material == 0 {
                    // Opaque faces turned away from the light never hold the
                    // nearest depth of a closed solid surface.
                    chunk
                        .mesh
                        .light_facing_runs(0, light.to_array(), &mut push);
                } else if !chunk.mesh.ranges[1].is_empty() {
                    push(chunk.mesh.ranges[1].clone());
                }
            }
            if self.runs.len() > first {
                self.chunks.push(key);
                self.origins.push([offset.x, offset.y, offset.z, 1.]);
            }
        }
        if let Some(mesh) = fixture.filter(|mesh| mesh.indices.is_some()) {
            for material in 0..2 {
                if !mesh.ranges[material].is_empty() {
                    self.runs.push(ShadowRun {
                        key: None,
                        instance: 1,
                        material,
                        range: mesh.ranges[material].clone(),
                    });
                }
            }
        }
        if armed {
            let batch = self.batch.as_mut().expect("shadow batch armed");
            for run in &self.runs {
                let mesh = match run.key {
                    Some(key) => &chunks[&key].mesh,
                    None => fixture.expect("fixture run without fixture"),
                };
                push_shadow_draw(mesh, run, vertex_stride, batch);
            }
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.origins));
        if let Some(batch) = self.batch.as_ref()
            && !batch.draws.is_empty()
        {
            queue.write_buffer(&batch.buffer, 0, bytemuck::cast_slice(&batch.draws));
        }
    }
}

/// Conservative test: does the camera-relative box lie wholly outside the
/// light's orthographic x/y extent (with a few texels of margin)?
fn outside_light_box(matrix: &Mat4, min: Vec3, max: Vec3) -> bool {
    const MARGIN: f32 = 0.02;
    let mut below = [true; 2];
    let mut above = [true; 2];
    for corner in 0..8u32 {
        let point = Vec3::new(
            if corner & 1 == 0 { min.x } else { max.x },
            if corner & 2 == 0 { min.y } else { max.y },
            if corner & 4 == 0 { min.z } else { max.z },
        );
        let ndc = matrix.project_point3(point);
        for axis in 0..2 {
            below[axis] &= ndc[axis] < -1. - MARGIN;
            above[axis] &= ndc[axis] > 1. + MARGIN;
        }
    }
    below.iter().chain(&above).any(|&outside| outside)
}

/// Append the indirect argument of one caster run, preserving the exact
/// per-section instance index (`null 0 + dynamic 1 + chunks i+2`).
fn push_shadow_draw(mesh: &ResidentMesh, run: &ShadowRun, vertex_stride: u64, batch: &mut ShadowBatch) {
    let (Some(vertices), Some(indices)) = (mesh.vertices, mesh.indices) else {
        return;
    };
    debug_assert_eq!(vertices.offset % vertex_stride, 0, "unaligned shadow base vertex");
    let base_vertex = (vertices.offset / vertex_stride) as i32;
    batch.draws.push(ShadowDraw {
        index_count: run.range.end - run.range.start,
        instance_count: 1,
        first_index: (indices.offset / 4) as u32 + run.range.start,
        base_vertex,
        first_instance: run.instance,
    });
    batch.logical_draws[run.material] += 1;
    batch.submitted_indices += u64::from(run.range.end - run.range.start);
}

pub(super) fn projection(sun_angle: f32, moon_angle: f32) -> (Mat4, Vec3, f32) {
    let (light, strength) = sky::dominant_light(sun_angle, moon_angle);
    let up = if light.y.abs() > 0.98 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let matrix = Mat4::orthographic_rh(-RADIUS, RADIUS, -RADIUS, RADIUS, 1., 384.)
        * Mat4::look_at_rh(light * 192., Vec3::ZERO, up);
    (matrix, light, strength)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fancy_shadow_projection_contains_camera_and_switches_to_moon() {
        for angle in [0., 45., 90., 180., 270., 359.] {
            let (matrix, light, intensity) = projection(angle, angle + 180.);
            assert!(matrix.is_finite());
            assert!((light.length() - 1.).abs() < 1e-5);
            let center = matrix.project_point3(Vec3::ZERO);
            assert!(
                center.x.abs() < 1e-5 && center.y.abs() < 1e-5 && (0. ..1.).contains(&center.z)
            );
            assert!(light.y >= -1e-5);
            assert!(intensity > 0.);
        }
    }
}
