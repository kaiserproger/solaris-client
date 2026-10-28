//! Fullscreen HDR postprocess for the world scene target.
//!
//! The world target is always [`HDR`]. Off mode renders display-encoded
//! color into it and copies those texels to the UI target unchanged; Fancy
//! mode renders linear HDR and, per preset, runs (in order) a temporal
//! anti-aliasing resolve (High), half-resolution screen-space god rays, a
//! light half-resolution separable bloom, and display-encodes (ACES + sRGB)
//! on the way out. UI draws after this pass, so it never contributes to bloom.
//!
//! All GPU objects are created up front; [`Post::encode`] only opens render
//! passes, so there is no per-frame allocation besides pass descriptors. The
//! per-frame uniform ([`PostFrame`]) is written once before the passes; every
//! pass of one frame reads the same constants, so no `queue.write_buffer`
//! ordering hazard exists between passes.
//!
//! GPU timestamps use the shared [`super::timing::Timing`] slots 13
//! (`bloom_horizontal`), 14 (`bloom_vertical`), 15 (Off copy, Fancy
//! `aces_tonemap`), 21 (`god_rays`) and 22 (`taa_resolve`), so A/B totals stay
//! comparable across modes.

use super::graphics::Features;
use super::timing::Timing;
use super::COLOR;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

pub(super) const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const BLOOM_H_PASS: usize = 13;
const BLOOM_V_PASS: usize = 14;
const TONEMAP_PASS: usize = 15;
const RAYS_PASS: usize = 21;
const TAA_PASS: usize = 22;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    scene_texel: [f32; 2],
    bloom_texel: [f32; 2],
    inverse_vp: [f32; 16],
    previous_vp: [f32; 16],
    camera_delta: [f32; 4],
    ray_color: [f32; 4],
    sun_uv: [f32; 4],
    jitter: [f32; 4],
}

/// Per-frame constants of the temporal and god-ray passes.
pub(super) struct PostFrame {
    /// Unjittered inverse view-projection of this frame.
    pub inverse_view_projection: Mat4,
    /// Unjittered view-projection of the previous frame.
    pub previous_view_projection: Mat4,
    /// Current camera position minus previous camera position (world).
    pub camera_delta: Vec3,
    pub history_valid: bool,
    /// Shaft colour times strength; zero disables the shafts for this frame.
    pub ray_color: [f32; 3],
    pub sun_uv: [f32; 2],
    pub sun_in_front: bool,
    /// This frame's projection jitter in NDC.
    pub jitter_ndc: [f32; 2],
}

pub(super) struct Post {
    pub scene_view: wgpu::TextureView,
    /// Copy source for the High water refraction/SSR scene copy.
    pub scene: wgpu::Texture,
    bloom_a: wgpu::Texture,
    bloom_a_view: wgpu::TextureView,
    bloom_b: wgpu::Texture,
    bloom_b_view: wgpu::TextureView,
    rays: wgpu::Texture,
    rays_view: wgpu::TextureView,
    resolved: [wgpu::Texture; 2],
    resolved_views: [wgpu::TextureView; 2],
    params_buffer: wgpu::Buffer,
    texel: [f32; 4],
    taps: u32,
    taa: bool,
    /// Index of the resolve target the next TAA pass writes.
    write: usize,
    copy: wgpu::RenderPipeline,
    bloom_h: wgpu::RenderPipeline,
    bloom_v: wgpu::RenderPipeline,
    combine: wgpu::RenderPipeline,
    rays_pipeline: wgpu::RenderPipeline,
    taa_pipeline: wgpu::RenderPipeline,
    bind_copy: wgpu::BindGroup,
    /// Indexed by source: 0 = raw scene, 1/2 = resolved[0]/[1].
    bind_h: [wgpu::BindGroup; 3],
    bind_combine: [wgpu::BindGroup; 3],
    bind_v: wgpu::BindGroup,
    bind_rays: wgpu::BindGroup,
    /// Indexed by the resolve target being written; history is the other one.
    bind_taa: [wgpu::BindGroup; 2],
}

fn texture(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    height: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR,
        usage: usage
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

impl Post {
    pub(super) fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        depth: &wgpu::TextureView,
        features: &Features,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let bloom_width = (width / 2).max(1);
        let bloom_height = (height / 2).max(1);
        let hdr = features.hdr;
        let taps = if hdr { features.god_ray_taps } else { 0 };
        let taa = hdr && features.taa;

        let scene = texture(
            device,
            "post scene HDR",
            width,
            height,
            wgpu::TextureUsages::COPY_SRC,
        );
        let scene_view = scene.create_view(&wgpu::TextureViewDescriptor::default());
        let none = wgpu::TextureUsages::empty();
        let bloom_a = texture(
            device,
            "post bloom half HDR",
            bloom_width,
            bloom_height,
            none,
        );
        let bloom_a_view = bloom_a.create_view(&wgpu::TextureViewDescriptor::default());
        let bloom_b = texture(
            device,
            "post bloom half HDR ping",
            bloom_width,
            bloom_height,
            none,
        );
        let bloom_b_view = bloom_b.create_view(&wgpu::TextureViewDescriptor::default());
        // Feature-off targets are 1x1 placeholders so every bind group is valid.
        let (rays_width, rays_height) = if taps > 0 {
            (bloom_width, bloom_height)
        } else {
            (1, 1)
        };
        let rays = texture(
            device,
            "post god rays half HDR",
            rays_width,
            rays_height,
            none,
        );
        let rays_view = rays.create_view(&wgpu::TextureViewDescriptor::default());
        let (resolve_width, resolve_height) = if taa { (width, height) } else { (1, 1) };
        let resolved = [
            texture(
                device,
                "post TAA resolve A",
                resolve_width,
                resolve_height,
                none,
            ),
            texture(
                device,
                "post TAA resolve B",
                resolve_width,
                resolve_height,
                none,
            ),
        ];
        let resolved_views = [
            resolved[0].create_view(&wgpu::TextureViewDescriptor::default()),
            resolved[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

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

        let texel = [
            1.0 / width as f32,
            1.0 / height as f32,
            1.0 / bloom_width as f32,
            1.0 / bloom_height as f32,
        ];
        let params = Params {
            scene_texel: [texel[0], texel[1]],
            bloom_texel: [texel[2], texel[3]],
            ..bytemuck::Zeroable::zeroed()
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post frame params"),
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
            label: Some("post scene+bloom+temporal bindings"),
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
                float_2d(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                float_2d(7),
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
        let rays_pipeline = fullscreen("fs_rays", HDR);
        let taa_pipeline = fullscreen("fs_taa", HDR);

        // A texture bound in a pass must never alias that pass's attachment,
        // even on bindings the stage ignores. No post pass renders into the raw
        // scene, so each stage parks every binding it does not read on it.
        let bind_with = |label: &str,
                         scene_source: &wgpu::TextureView,
                         bloom_h: &wgpu::TextureView,
                         bloom_v: &wgpu::TextureView,
                         history: &wgpu::TextureView,
                         shafts: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(scene_source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(bloom_h),
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
                        resource: wgpu::BindingResource::TextureView(bloom_v),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(history),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(shafts),
                    },
                ],
            })
        };
        let sources = [&scene_view, &resolved_views[0], &resolved_views[1]];
        let bind_copy = bind_with(
            "post copy bindings",
            &scene_view,
            &scene_view,
            &scene_view,
            &scene_view,
            &scene_view,
        );
        let bind_h = std::array::from_fn(|i| {
            bind_with(
                "post bloom horizontal bindings",
                sources[i],
                &scene_view,
                &scene_view,
                &scene_view,
                &scene_view,
            )
        });
        let bind_v = bind_with(
            "post bloom vertical bindings",
            &scene_view,
            &bloom_a_view,
            &scene_view,
            &scene_view,
            &scene_view,
        );
        let bind_combine = std::array::from_fn(|i| {
            bind_with(
                "post combine bindings",
                sources[i],
                &scene_view,
                &bloom_b_view,
                &scene_view,
                &rays_view,
            )
        });
        let bind_rays = bind_with(
            "post god ray bindings",
            &scene_view,
            &scene_view,
            &scene_view,
            &scene_view,
            &scene_view,
        );
        let bind_taa = [
            bind_with(
                "post TAA bindings write A",
                &scene_view,
                &scene_view,
                &scene_view,
                &resolved_views[1],
                &scene_view,
            ),
            bind_with(
                "post TAA bindings write B",
                &scene_view,
                &scene_view,
                &scene_view,
                &resolved_views[0],
                &scene_view,
            ),
        ];

        Self {
            scene_view,
            scene,
            bloom_a,
            bloom_a_view,
            bloom_b,
            bloom_b_view,
            rays,
            rays_view,
            resolved,
            resolved_views,
            params_buffer,
            texel,
            taps,
            taa,
            write: 0,
            copy,
            bloom_h,
            bloom_v,
            combine,
            rays_pipeline,
            taa_pipeline,
            bind_copy,
            bind_h,
            bind_combine,
            bind_v,
            bind_rays,
            bind_taa,
        }
    }

    pub(super) fn taa(&self) -> bool {
        self.taa
    }

    /// Resident bytes of every post target (scene HDR, bloom, rays, TAA history).
    pub(super) fn target_bytes(&self) -> u64 {
        [
            &self.scene,
            &self.bloom_a,
            &self.bloom_b,
            &self.rays,
            &self.resolved[0],
            &self.resolved[1],
        ]
        .iter()
        .map(|t| u64::from(t.width()) * u64::from(t.height()) * 8)
        .sum()
    }

    /// Uploads this frame's constants; call once before [`Post::encode`].
    pub(super) fn update(&self, queue: &wgpu::Queue, frame: &PostFrame) {
        let params = Params {
            scene_texel: [self.texel[0], self.texel[1]],
            bloom_texel: [self.texel[2], self.texel[3]],
            inverse_vp: frame.inverse_view_projection.to_cols_array(),
            previous_vp: frame.previous_view_projection.to_cols_array(),
            camera_delta: [
                frame.camera_delta.x,
                frame.camera_delta.y,
                frame.camera_delta.z,
                f32::from(u8::from(frame.history_valid)),
            ],
            ray_color: [
                frame.ray_color[0],
                frame.ray_color[1],
                frame.ray_color[2],
                self.taps as f32,
            ],
            sun_uv: [
                frame.sun_uv[0],
                frame.sun_uv[1],
                f32::from(u8::from(frame.sun_in_front)),
                0.,
            ],
            jitter: [frame.jitter_ndc[0], frame.jitter_ndc[1], 0., 0.],
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    fn fullscreen(
        encoder: &mut wgpu::CommandEncoder,
        label: &'static str,
        target: &wgpu::TextureView,
        clear: bool,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
        pipeline: &wgpu::RenderPipeline,
        bind: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Records the passes of one frame; returns the number of fullscreen draws.
    pub(super) fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        hdr: bool,
        timing: &Timing,
    ) -> u64 {
        debug_assert_eq!(self.scene.format(), HDR);
        debug_assert_eq!(self.bloom_a.format(), HDR);
        debug_assert_eq!(self.bloom_b.format(), HDR);
        if !hdr {
            Self::fullscreen(
                encoder,
                "post copy",
                output,
                true,
                timing.writes(TONEMAP_PASS),
                &self.copy,
                &self.bind_copy,
            );
            return 1;
        }
        let mut draws = 0;
        let mut source = 0;
        if self.taa {
            let write = self.write;
            Self::fullscreen(
                encoder,
                "post taa resolve",
                &self.resolved_views[write],
                true,
                timing.writes(TAA_PASS),
                &self.taa_pipeline,
                &self.bind_taa[write],
            );
            source = 1 + write;
            self.write = 1 - write;
            draws += 1;
        }
        if self.taps > 0 {
            Self::fullscreen(
                encoder,
                "post god rays",
                &self.rays_view,
                true,
                timing.writes(RAYS_PASS),
                &self.rays_pipeline,
                &self.bind_rays,
            );
            draws += 1;
        }
        Self::fullscreen(
            encoder,
            "post bloom horizontal",
            &self.bloom_a_view,
            true,
            timing.writes(BLOOM_H_PASS),
            &self.bloom_h,
            &self.bind_h[source],
        );
        Self::fullscreen(
            encoder,
            "post bloom vertical",
            &self.bloom_b_view,
            true,
            timing.writes(BLOOM_V_PASS),
            &self.bloom_v,
            &self.bind_v,
        );
        Self::fullscreen(
            encoder,
            "post combine tonemap",
            output,
            true,
            timing.writes(TONEMAP_PASS),
            &self.combine,
            &self.bind_combine[source],
        );
        draws + 3
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

    /// The Params uniform must match the WGSL struct byte for byte:
    /// 2+2 floats, two mat4, three vec4 (std140: mat4 at 16-byte offsets).
    #[test]
    fn params_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<super::Params>(), 16 + 64 + 64 + 16 * 4);
        assert_eq!(std::mem::size_of::<super::Params>() % 16, 0);
    }
}
