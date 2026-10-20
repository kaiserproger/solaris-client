use super::material::{Descriptor, MATERIALS};
use super::packed::TerrainVertex;
use super::{COLOR, DEPTH};
use crate::api::{EntityMaterial, Vertex};

/// Packed terrain vertex attributes (see `packed.rs` for the byte layout).
pub(super) const TERRAIN_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Uint16x4, 1 => Unorm8x4, 2 => Uint8x4, 3 => Uint16x2
];

pub(super) struct Pipelines {
    pub world: [wgpu::RenderPipeline; 3],
    pub dynamic: Vec<wgpu::RenderPipeline>,
    pub dynamic_index: [usize; EntityMaterial::COUNT],
    pub dynamic_order: [usize; EntityMaterial::COUNT],
    pub overlay: wgpu::RenderPipeline,
    pub sky: wgpu::RenderPipeline,
    pub celestials: wgpu::RenderPipeline,
    pub dark_disc: wgpu::RenderPipeline,
    pub stars: wgpu::RenderPipeline,
    pub sunrise: wgpu::RenderPipeline,
    pub blit: wgpu::RenderPipeline,
    pub camera_layout: wgpu::BindGroupLayout,
    pub atlas_layout: wgpu::BindGroupLayout,
}
impl Pipelines {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Solaris authored material/sky shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader.wgsl").into()),
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shared world environment"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shared packed texture array"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let fancy_layout = super::shadow::layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("one world UI content layout"),
            bind_group_layouts: &[&camera_layout, &atlas_layout, &fancy_layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x2,4=>Float32x3,5=>Float32,6=>Uint32,8=>Float32x2,9=>Float32x3];
        let instances = wgpu::vertex_attr_array![7=>Float32x4];
        let world_buffers = [
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            },
            wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instances,
            },
        ];
        let terrain_buffers = [
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &TERRAIN_ATTRIBUTES,
            },
            wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instances,
            },
        ];
        let alpha = wgpu::BlendState::ALPHA_BLENDING;
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let world = std::array::from_fn(|i| {
            create(
                device,
                &shader,
                Some(&layout),
                "terrain_vertex",
                ["opaque_fragment", "cutout_fragment", "translucent_fragment"][i],
                &terrain_buffers,
                super::post::HDR,
                if i == 2 { Some(alpha) } else { None },
                Some(true),
                Some(wgpu::Face::Back),
            )
        });
        let mut dynamic = Vec::new();
        let mut dynamic_index = [0; EntityMaterial::COUNT];
        for material in 0..EntityMaterial::COUNT {
            if let Some(previous) =
                (0..material).find(|&i| MATERIALS[i].raster_equivalent(MATERIALS[material]))
            {
                dynamic_index[material] = dynamic_index[previous];
            } else {
                dynamic_index[material] = dynamic.len();
                dynamic.push(create_source(
                    device,
                    &shader,
                    &layout,
                    &world_buffers,
                    MATERIALS[material],
                ));
            }
        }
        let mut dynamic_order = std::array::from_fn(|i| i);
        dynamic_order.sort_unstable_by_key(|&i| (MATERIALS[i].sort_key, i));
        let overlay = create(
            device,
            &shader,
            Some(&layout),
            "overlay_vertex",
            "overlay_fragment",
            &world_buffers[..1],
            COLOR,
            Some(alpha),
            None,
            None,
        );
        let sky = create(
            device,
            &shader,
            Some(&layout),
            "blit_vertex",
            "sky_fragment",
            &[],
            super::post::HDR,
            None,
            None,
            None,
        );
        let celestials = create(
            device,
            &shader,
            Some(&layout),
            "blit_vertex",
            "celestial_fragment",
            &[],
            super::post::HDR,
            Some(additive),
            None,
            None,
        );
        let dark_disc = create(
            device,
            &shader,
            Some(&layout),
            "blit_vertex",
            "dark_disc_fragment",
            &[],
            super::post::HDR,
            Some(alpha),
            None,
            None,
        );
        let star_attributes = wgpu::vertex_attr_array![0=>Float32x3];
        let stars = create(
            device,
            &shader,
            Some(&layout),
            "star_vertex",
            "star_fragment",
            &[wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &star_attributes,
            }],
            super::post::HDR,
            Some(additive),
            None,
            None,
        );
        let sunrise = create(
            device,
            &shader,
            Some(&layout),
            "scene_vertex",
            "overlay_fragment",
            &world_buffers,
            super::post::HDR,
            Some(alpha),
            None,
            None,
        );
        let blit = create(
            device,
            &shader,
            None,
            "blit_vertex",
            if surface_format.is_srgb() {
                "blit_srgb_fragment"
            } else {
                "blit_fragment"
            },
            &[],
            surface_format,
            None,
            None,
            None,
        );
        Self {
            world,
            dynamic,
            dynamic_index,
            dynamic_order,
            overlay,
            sky,
            celestials,
            dark_disc,
            stars,
            sunrise,
            blit,
            camera_layout,
            atlas_layout,
        }
    }
}
fn create(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: Option<&wgpu::PipelineLayout>,
    vertex: &str,
    fragment: &str,
    buffers: &[wgpu::VertexBufferLayout<'_>],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    depth: Option<bool>,
    cull: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: depth.map(|write| wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: write,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}

fn create_source(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    buffers: &[wgpu::VertexBufferLayout<'_>],
    descriptor: Descriptor,
) -> wgpu::RenderPipeline {
    let constants = descriptor.shader.constants();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pinned Source RenderType specialization"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("source_vertex"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("source_fragment"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: if descriptor.shader.kind == 11 {
                    COLOR
                } else {
                    super::post::HDR
                },
                blend: descriptor.blend_state(),
                write_mask: if descriptor.color_write {
                    wgpu::ColorWrites::ALL
                } else {
                    wgpu::ColorWrites::empty()
                },
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: descriptor.cull.then_some(wgpu::Face::Back),
            ..Default::default()
        },
        // Disabled Source depth testing is raster-equivalent to ALWAYS/no-write.
        // Keeping the attachment format permits these draws in the shared world pass.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: descriptor.write_depth,
            depth_compare: match descriptor.depth {
                0 => wgpu::CompareFunction::Always,
                1 => wgpu::CompareFunction::LessEqual,
                2 => wgpu::CompareFunction::Equal,
                _ => unreachable!(),
            },
            stencil: Default::default(),
            bias: if descriptor.bias {
                wgpu::DepthBiasState {
                    constant: -10,
                    slope_scale: -1.,
                    clamp: 0.,
                }
            } else {
                Default::default()
            },
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}
