use super::{Face, Frame, fit};
use wgpu::util::DeviceExt;

pub(crate) struct Texture {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertices: wgpu::Buffer,
    size: [u32; 2],
    pub face: Face,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 4],
    uv: [f32; 2],
    image: [f32; 4],
}
impl Texture {
    pub fn new(device: &wgpu::Device, face: Face, size: [u32; 2], shared: Option<&Self>) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("client-local desktop"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (layout, sampler, pipeline) = if let Some(shared) = shared {
            (shared.layout.clone(), shared.sampler.clone(), shared.pipeline.clone())
        } else {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("desktop texture"),
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
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("desktop face"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("desktop face"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("desktop face"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 40,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x2,2=>Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
            (layout, sampler, pipeline)
        };
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("desktop frame"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("desktop face vertices"),
            contents: bytemuck::cast_slice(
                &[Vertex {
                    position: [0.; 4],
                    uv: [0.; 2],
                    image: [0.; 4],
                }; 6],
            ),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            texture,
            bind,
            pipeline,
            vertices,
            layout,
            sampler,
            size,
            face,
        }
    }
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn upload(&self, queue: &wgpu::Queue, frame: &Frame) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
    }
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        camera: [f64; 3],
        matrix: glam::Mat4,
    ) {
        let vertices = face_vertices(self.face, self.size, camera, matrix);
        queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("desktop on monitor face"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..6, 0..1);
    }
}
fn face_vertices(face: Face, size: [u32; 2], camera: [f64; 3], matrix: glam::Mat4) -> [Vertex; 6] {
    let (origin, right, down) = face.basis();
    let bounds = [f64::from(face.width), f64::from(face.height)];
    let extent = fit(size, face.width, face.height);
    let image = [
        ((bounds[0] - extent[0]) * 0.5) as f32,
        ((bounds[1] - extent[1]) * 0.5) as f32,
        extent[0] as f32, extent[1] as f32,
    ];
    [[0., 0.], [0., 1.], [1., 1.], [0., 0.], [1., 1.], [1., 0.]].map(|corner| {
        let uv = [corner[0] * bounds[0], corner[1] * bounds[1]];
        let point = std::array::from_fn(|i| (origin[i] - camera[i] + right[i] * uv[0] + down[i] * uv[1]) as f32);
        Vertex {
            position: (matrix * glam::Vec3::from_array(point).extend(1.)).to_array(),
            uv: uv.map(|value| value as f32),
            image,
        }
    })
}
const SHADER:&str = "
struct Out { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) image:vec4<f32> }
@vertex fn vs(@location(0) position:vec4<f32>, @location(1) uv:vec2<f32>, @location(2) image:vec4<f32>)->Out { var o:Out; o.position=position; o.uv=uv; o.image=image; return o; }
@group(0) @binding(0) var image:texture_2d<f32>;
@group(0) @binding(1) var sample_image:sampler;
@fragment fn fs(i:Out)->@location(0) vec4<f32> {
    let picture_uv = (i.uv-i.image.xy)/i.image.zw;
    let color = textureSample(image,sample_image,clamp(picture_uv,vec2<f32>(0.),vec2<f32>(1.)));
    let bounds = 2.*i.image.xy+i.image.zw;
    let edge = min(i.uv,bounds-i.uv);
    if (any(edge<vec2<f32>(0.0625))) { return vec4<f32>(0.055,0.065,0.075,1.); }
    if (any(picture_uv<vec2<f32>(0.)) || any(picture_uv>vec2<f32>(1.))) { return vec4<f32>(0.,0.,0.,1.); }
    return vec4<f32>(color.rgb,1.);
}
";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_surface_shader_validates_without_nonuniform_derivatives() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::empty()
        ).validate(&module).unwrap();
    }
    #[test]
    fn a_single_quad_covers_the_entire_rectangle_including_letterbox() {
        for side in 2..=5 {
            let face = Face { block: [10, 20, 30], side, width: 4, height: 2 };
            let vertices = face_vertices(face, [1920, 1080], [0.; 3], glam::Mat4::IDENTITY);
            let (origin, right, down) = face.basis();
            for vertex in vertices {
                assert!(vertex.uv[0] == 0. || vertex.uv[0] == 4.);
                assert!(vertex.uv[1] == 0. || vertex.uv[1] == 2.);
                for axis in 0..3 {
                    let expected = origin[axis] + right[axis] * f64::from(vertex.uv[0]) + down[axis] * f64::from(vertex.uv[1]);
                    assert!((f64::from(vertex.position[axis]) - expected).abs() < 0.00001);
                }
                assert!((2. * vertex.image[0] + vertex.image[2] - 4.).abs() < 0.00001);
                assert!((2. * vertex.image[1] + vertex.image[3] - 2.).abs() < 0.00001);
            }
        }
    }
}
