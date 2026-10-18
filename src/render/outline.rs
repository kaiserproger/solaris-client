use super::{COLOR, DEPTH};

struct Targets {
    _mask: wgpu::Texture,
    _swap: wgpu::Texture,
    _depth: wgpu::Texture,
    mask: wgpu::TextureView,
    swap: wgpu::TextureView,
    depth: wgpu::TextureView,
    mask_input: wgpu::BindGroup,
    swap_input: wgpu::BindGroup,
}

pub(super) struct Outline {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipelines: [wgpu::RenderPipeline; 5],
    targets: Targets,
}

impl Outline {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline post input"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("source outline bilinear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("authored source silhouette and outline post chain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("outline post layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipelines = std::array::from_fn(|i| {
            let fragment = ["silhouette", "horizontal", "vertical", "blit", "blit"][i];
            let compose = i == 4;
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(
                    [
                        "outline silhouette",
                        "outline horizontal blur",
                        "outline vertical blur",
                        "outline identity blit",
                        "outline RGB composite",
                    ][i],
                ),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR,
                        blend: compose.then_some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: if compose {
                            wgpu::ColorWrites::RED
                                | wgpu::ColorWrites::GREEN
                                | wgpu::ColorWrites::BLUE
                        } else {
                            wgpu::ColorWrites::ALL
                        },
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        });
        let targets = Self::targets(device, &layout, &sampler, width, height);
        Self {
            layout,
            sampler,
            pipelines,
            targets,
        }
    }

    fn targets(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        width: u32,
        height: u32,
    ) -> Targets {
        let create = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let mask_texture = create("actual entity outline mask", COLOR, usage);
        let swap_texture = create("entity outline post swap", COLOR, usage);
        let depth_texture = create(
            "isolated outline depth",
            DEPTH,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let mask = mask_texture.create_view(&Default::default());
        let swap = swap_texture.create_view(&Default::default());
        let depth = depth_texture.create_view(&Default::default());
        let input = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("outline target input"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        };
        let mask_input = input(&mask);
        let swap_input = input(&swap);
        Targets {
            _mask: mask_texture,
            _swap: swap_texture,
            _depth: depth_texture,
            mask,
            swap,
            depth,
            mask_input,
            swap_input,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.targets = Self::targets(device, &self.layout, &self.sampler, width, height);
    }

    pub fn mask_pass<'a>(
        &'a self,
        encoder: &'a mut wgpu::CommandEncoder,
        timing: &'a super::timing::Timing,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("actual entity outline mask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.targets.mask,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timing.writes(4),
            occlusion_query_set: None,
        })
    }

    pub fn post(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        main: &wgpu::TextureView,
        timing: &super::timing::Timing,
    ) {
        // The verified chain alternates mask -> swap -> mask -> swap -> mask.
        // Its final RGB-only blend targets main color, never the mask's source.
        for i in 0..5 {
            let (output, input) = match i {
                0 | 2 => (&self.targets.swap, &self.targets.mask_input),
                1 | 3 => (&self.targets.mask, &self.targets.swap_input),
                4 => (main, &self.targets.mask_input),
                _ => unreachable!(),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(
                    [
                        "outline silhouette",
                        "outline horizontal blur",
                        "outline vertical blur",
                        "outline identity blit",
                        "outline RGB composite",
                    ][i],
                ),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if i == 4 {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timing.writes(i + 5),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipelines[i]);
            pass.set_bind_group(0, input, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
