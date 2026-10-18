//! Fullscreen HDR postprocess for the world scene target.
//!
//! The world target is always [`HDR`]. Off mode renders display-encoded
//! color into it and copies those texels to the UI target unchanged; Fancy
//! mode renders linear HDR, resolves a light half-resolution separable
//! bloom, and display-encodes (ACES + sRGB) on the way out. UI draws after
//! this pass, so it never contributes to bloom.
//!
//! All GPU objects are created up front; [`Post::encode`] only opens render
//! passes, so there is no per-frame allocation besides pass descriptors. One
//! resolution uniform is written once at creation; per-pass constants (blur
//! direction, copy vs. combine) are baked into separate fragment entry points
//! so no `queue.write_buffer` ordering hazard exists between passes.
//!
//! GPU timestamps use the shared [`super::timing::Timing`] slots 13
//! (`bloom_horizontal`), 14 (`bloom_vertical`) and 15 (Off copy, Fancy
//! `aces_tonemap`), so A/B totals stay comparable across modes.

use super::timing::Timing;
use super::COLOR;
use wgpu::util::DeviceExt;

pub(super) const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const BLOOM_H_PASS: usize = 13;
const BLOOM_V_PASS: usize = 14;
const TONEMAP_PASS: usize = 15;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    scene_texel: [f32; 2],
    bloom_texel: [f32; 2],
}

pub(super) struct Post {
    pub scene_view: wgpu::TextureView,
    scene: wgpu::Texture,
    bloom_a: wgpu::Texture,
    bloom_a_view: wgpu::TextureView,
    bloom_b: wgpu::Texture,
    bloom_b_view: wgpu::TextureView,
    copy: wgpu::RenderPipeline,
    bloom_h: wgpu::RenderPipeline,
    bloom_v: wgpu::RenderPipeline,
    combine: wgpu::RenderPipeline,
    bind_copy: wgpu::BindGroup,
    bind_h: wgpu::BindGroup,
    bind_v: wgpu::BindGroup,
    bind_combine: wgpu::BindGroup,
}

impl Post {
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let bloom_width = (width / 2).max(1);
        let bloom_height = (height / 2).max(1);

        let scene = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post scene HDR"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let scene_view = scene.create_view(&wgpu::TextureViewDescriptor::default());

        let bloom_desc = wgpu::TextureDescriptor {
            label: Some("post bloom half HDR"),
            size: wgpu::Extent3d {
                width: bloom_width,
                height: bloom_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let bloom_a = device.create_texture(&bloom_desc);
        let bloom_a_view = bloom_a.create_view(&wgpu::TextureViewDescriptor::default());
        let bloom_b = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post bloom half HDR ping"),
            ..bloom_desc
        });
        let bloom_b_view = bloom_b.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post bloom sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        let params = Params {
            scene_texel: [1.0 / width as f32, 1.0 / height as f32],
            bloom_texel: [1.0 / bloom_width as f32, 1.0 / bloom_height as f32],
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post resolution params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let float_2d = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post scene+bloom bindings"),
            entries: &[
                float_2d(0),
                float_2d(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                float_2d(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post fullscreen layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Solaris HDR postprocess"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let fullscreen = |fragment: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fragment),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview: None,
                cache: None,
            })
        };

        let copy = fullscreen("fs_copy", COLOR);
        let bloom_h = fullscreen("fs_bloom_h", HDR);
        let bloom_v = fullscreen("fs_bloom_v", HDR);
        let combine = fullscreen("fs_combine", COLOR);

        let bind_with = |label: &str, b1: &wgpu::TextureView, b4: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&scene_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(b1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(b4),
                    },
                ],
            })
        };
        // A texture bound in a pass must never alias that pass's attachment,
        // even on bindings the stage ignores: h writes bloom A, v writes
        // bloom B, so each stage parks its unused bloom slots on the scene.
        let bind_copy = bind_with("post copy bindings", &scene_view, &scene_view);
        let bind_h = bind_with("post bloom horizontal bindings", &scene_view, &scene_view);
        let bind_v = bind_with("post bloom vertical bindings", &bloom_a_view, &scene_view);
        let bind_combine = bind_with("post combine bindings", &bloom_a_view, &bloom_b_view);

        Self {
            scene_view,
            scene,
            bloom_a,
            bloom_a_view,
            bloom_b,
            bloom_b_view,
            copy,
            bloom_h,
            bloom_v,
            combine,
            bind_copy,
            bind_h,
            bind_v,
            bind_combine,
        }
    }

    pub(super) fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        fancy: bool,
        timing: &Timing,
    ) {
        debug_assert_eq!(self.scene.format(), HDR);
        debug_assert_eq!(self.bloom_a.format(), HDR);
        debug_assert_eq!(self.bloom_b.format(), HDR);
        if !fancy {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timing.writes(TONEMAP_PASS),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.copy);
            pass.set_bind_group(0, &self.bind_copy, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post bloom horizontal"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.bloom_a_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timing.writes(BLOOM_H_PASS),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.bloom_h);
            pass.set_bind_group(0, &self.bind_h, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post bloom vertical"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.bloom_b_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timing.writes(BLOOM_V_PASS),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.bloom_v);
            pass.set_bind_group(0, &self.bind_v, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post combine tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timing.writes(TONEMAP_PASS),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.combine);
            pass.set_bind_group(0, &self.bind_combine, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    /// CPU mirror of the Narkowicz ACES fit in post.wgsl (`aces_tonemap`).
    fn aces_fit(x: f32) -> f32 {
        let x = x.max(0.);
        ((x * (x * 2.51 + 0.03)) / (x * (x * 2.43 + 0.59) + 0.14)).clamp(0., 1.)
    }

    /// CPU mirror of the HDR bloom gate in post.wgsl (`bloom_bright`).
    fn bloom_weight(luma: f32) -> f32 {
        ((luma - 1.0) / 0.5).clamp(0.0, 1.0)
    }

    #[test]
    fn aces_is_monotonic_and_finite() {
        assert_eq!(aces_fit(0.0), 0.0);
        let mut prev = 0.0;
        let mut x = 0.0;
        while x <= 10.0 {
            let y = aces_fit(x);
            assert!(y.is_finite(), "aces not finite at {x}");
            assert!(
                (0. ..=1.).contains(&y),
                "aces out of display range at {x}: {y}"
            );
            assert!(y >= prev, "aces decreased at {x}");
            prev = y;
            x += 0.01;
        }
        assert_eq!(aces_fit(-1.), 0.);
        assert_eq!(aces_fit(10.), 1.);
        assert!(
            (aces_fit(0.18) - 0.2668989).abs() < 1e-5,
            "ACES mid-gray must retain the fitted curve"
        );
    }

    #[test]
    fn bloom_threshold_passes_hdr_only() {
        assert_eq!(bloom_weight(0.0), 0.0);
        assert_eq!(bloom_weight(0.8), 0.0);
        assert_eq!(bloom_weight(1.0), 0.0);
        assert!(bloom_weight(1.2) > 0.0);
        assert_eq!(bloom_weight(1.5), 1.0);
        assert_eq!(bloom_weight(100.0), 1.0);
    }
}
