use super::*;

// wgpu 27 folded the old MULTI_DRAW_INDIRECT flag into the baseline API.
// COUNT additionally guarantees native (not loop-emulated) multi-draw.
pub(super) const FEATURES: wgpu::Features =
    wgpu::Features::MULTI_DRAW_INDIRECT_COUNT.union(wgpu::Features::INDIRECT_FIRST_INSTANCE);

pub(super) fn supported(features: wgpu::Features, enabled: bool) -> bool {
    enabled && features.contains(FEATURES)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Candidate {
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
    ) -> Option<Self> {
        if range.is_empty() {
            return None;
        }
        let vertices = mesh.vertices?;
        let indices = mesh.indices?;
        let stride = std::mem::size_of::<TerrainVertex>() as u64;
        assert_eq!(vertices.offset % stride, 0, "unaligned terrain base vertex");
        Some(Self {
            min: [min.x, min.y, min.z, 0.],
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
    count: [u32; 4],
}

pub(super) struct Indirect {
    pub candidates: [Vec<Candidate>; 2],
    inputs: [wgpu::Buffer; 2],
    outputs: [wgpu::Buffer; 2],
    uniforms: [wgpu::Buffer; 2],
    bindings: [wgpu::BindGroup; 2],
    pipeline: wgpu::ComputePipeline,
}

impl Indirect {
    pub fn new(device: &wgpu::Device) -> Self {
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("conservative terrain frustum culling"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mdi.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("terrain GPU frustum culling"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        let bindings = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain culling inputs and indirect output"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: inputs[i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: outputs[i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniforms[i].as_entire_binding(),
                    },
                ],
            })
        });
        Self {
            candidates: std::array::from_fn(|_| Vec::with_capacity(mesh::MAX_CHUNKS)),
            inputs,
            outputs,
            uniforms,
            bindings,
            pipeline,
        }
    }

    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        matrix: Mat4,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
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
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("terrain frustum culling"),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
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
                    count: [count, 0, 0, 0],
                }),
            );
            pass.set_bind_group(0, &self.bindings[material], &[]);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
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
        self.inputs
            .iter()
            .chain(&self.outputs)
            .chain(&self.uniforms)
            .map(wgpu::Buffer::size)
            .sum()
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
        let candidate = Candidate::new(&mesh, 6..12, 17, Vec3::ZERO, Vec3::ONE).unwrap();
        assert_eq!(candidate.draw, [6, 38, 13, 17]);
        assert!(Candidate::new(&mesh, 12..12, 17, Vec3::ZERO, Vec3::ONE).is_none());
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
        mdi.encode(&renderer.queue, &mut encoder, matrix, None);
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
