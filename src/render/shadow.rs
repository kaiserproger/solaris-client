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

pub(super) struct Shadow {
    _map: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub opaque_depth: wgpu::Texture,
    pub bind: wgpu::BindGroup,
    pub pipeline: wgpu::RenderPipeline,
    pub instances: wgpu::Buffer,
    pub chunks: Vec<[i32; 3]>,
    origins: Vec<[f32; 4]>,
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
            origins: Vec::with_capacity(mesh::MAX_CHUNKS + 2),
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
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        chunks: &BTreeMap<[i32; 3], ResidentChunk>,
        scene: &Scene,
    ) {
        self.chunks.clear();
        self.origins.clear();
        self.origins.push([0., 0., 0., 1.]);
        let origin = relative(scene.origin, scene.camera_position);
        self.origins.push([origin.x, origin.y, origin.z, 1.]);
        // Include off-camera casters: camera-frustum culling would make shadows pop.
        for (&key, chunk) in chunks {
            let offset = relative(chunk.origin, scene.camera_position);
            let center = offset
                + (Vec3::from_array(chunk.bounds_min) + Vec3::from_array(chunk.bounds_max)) * 0.5;
            if chunk.mesh.indices.is_some()
                && chunk.visibility.is_some()
                && center.length_squared() < (RADIUS + 32.).powi(2)
            {
                self.chunks.push(key);
                self.origins.push([offset.x, offset.y, offset.z, 1.]);
            }
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.origins));
    }
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
