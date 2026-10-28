use super::graphics::MAX_CASCADES;
use super::*;

pub(super) const SIZE: u32 = 2048;
/// Medium's single cascade (the original shadow box).
pub(super) const SINGLE_RADIUS: f32 = 80.;
/// High's three cascades: crisp near the player, 144 blocks of coverage.
pub(super) const CASCADE_RADII: [f32; MAX_CASCADES] = [18., 52., 144.];
const LIGHT_DISTANCE: f32 = 192.;

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType, dimension: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

/// Group 2 of every world pipeline: shadow cascades (array), opaque depth copy,
/// opaque colour copy (High water refraction/SSR) and the sampler for it.
pub(super) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Fancy shadow, opaque depth and colour"),
        entries: &[
            texture_entry(
                0,
                wgpu::TextureSampleType::Depth,
                wgpu::TextureViewDimension::D2Array,
            ),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            texture_entry(
                2,
                wgpu::TextureSampleType::Depth,
                wgpu::TextureViewDimension::D2,
            ),
            texture_entry(
                3,
                wgpu::TextureSampleType::Float { filterable: true },
                wgpu::TextureViewDimension::D2,
            ),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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

/// CPU-prepacked indirect arguments in the direct caster walk's order,
/// cascade-major: `ranges[c]` indexes the draws of cascade `c`.
/// Both materials use the same alpha-tested pipeline, so one batch per
/// cascade suffices. Casters are not culled by the camera frustum (off-camera
/// casters must not pop), only by each cascade's orthographic box and
/// light-facing opaque groups.
pub(super) struct ShadowBatch {
    pub draws: Vec<ShadowDraw>,
    pub ranges: [std::ops::Range<u32>; MAX_CASCADES],
    pub buffer: wgpu::Buffer,
    /// Per-material logical chunk-range draws the batch submits.
    pub logical_draws: [u64; 2],
    /// Total shadow-pass terrain indices submitted (both materials, all cascades).
    pub submitted_indices: u64,
}

impl ShadowBatch {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        // One entry per (chunk, material) plus the fixture's two slots, per cascade.
        let capacity = (mesh::MAX_CHUNKS * 2 + 2) * MAX_CASCADES;
        Self {
            draws: Vec::with_capacity(capacity),
            ranges: std::array::from_fn(|_| 0..0),
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow caster indirect arguments"),
                size: (capacity * std::mem::size_of::<ShadowDraw>()) as u64,
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
        self.ranges = std::array::from_fn(|_| 0..0);
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

/// Orthographic light matrices for this frame, camera-relative.
#[derive(Clone, Copy, Debug)]
pub(super) struct Cascades {
    pub count: u32,
    pub radii: [f32; MAX_CASCADES],
    pub matrices: [Mat4; MAX_CASCADES],
}

impl Cascades {
    pub(super) const NONE: Self = Self {
        count: 0,
        radii: [0.; MAX_CASCADES],
        matrices: [Mat4::IDENTITY; MAX_CASCADES],
    };
}

pub(super) struct Shadow {
    map: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// One depth attachment view per cascade layer.
    pub layers: Vec<wgpu::TextureView>,
    pub opaque_depth: wgpu::Texture,
    pub opaque_depth_view: wgpu::TextureView,
    pub scene_copy: wgpu::Texture,
    scene_copy_view: wgpu::TextureView,
    size: (u32, u32),
    pub bind: wgpu::BindGroup,
    /// Per-cascade copies of the camera uniform whose `shadow_projection` is
    /// that cascade's matrix: the shadow vertex stage reads only that field.
    pub cameras: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// Dynamic (entity) casters, full `Vertex` layout.
    pub pipeline: wgpu::RenderPipeline,
    /// Terrain and fixture casters, packed `TerrainVertex` layout.
    pub terrain_pipeline: wgpu::RenderPipeline,
    pub instances: wgpu::Buffer,
    /// Caster index ranges in draw order, cascade-major: the single source of
    /// truth for the direct walk and the indirect batch.
    pub runs: Vec<ShadowRun>,
    /// `runs[run_ranges[c]]` are cascade `c`'s casters.
    pub run_ranges: [std::ops::Range<usize>; MAX_CASCADES],
    origins: Vec<[f32; 4]>,
    /// (key, instance, cascade bit mask) of every chunk reaching any cascade.
    casters: Vec<([i32; 3], u32, u8)>,
    /// CPU-prepacked indirect arguments mirroring the direct caster walk.
    /// `None`-armed (no-op) unless the renderer constructed it for native MDI.
    pub batch: Option<ShadowBatch>,
}

fn depth_texture(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    height: u32,
    layers: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

impl Shadow {
    pub fn new(
        device: &wgpu::Device,
        camera: &wgpu::BindGroupLayout,
        atlas: &wgpu::BindGroupLayout,
    ) -> Self {
        let (map, view, layers) = Self::map(device, 0);
        let opaque_depth = depth_texture(device, "Fancy depth texture", 1, 1, 1, wgpu::TextureUsages::COPY_DST);
        let opaque_depth_view = opaque_depth.create_view(&Default::default());
        let (scene_copy, scene_copy_view) = Self::scene_copy(device, 1, 1);
        let bind = Self::binding(device, &view, &opaque_depth_view, &scene_copy_view);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fancy alpha-tested directional shadow"),
            source: wgpu::ShaderSource::Wgsl(super::graphics::main_shader_source().into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow caster layout"),
            bind_group_layouts: &[camera, atlas],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x2,4=>Float32x3,5=>Float32,6=>Uint32,8=>Float32x2,9=>Float32x3];
        let instance_attributes = wgpu::vertex_attr_array![7=>Float32x4];
        let build = |label: &'static str, entry: &'static str, vertex: wgpu::VertexBufferLayout<'_>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &[
                        vertex,
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
            })
        };
        let pipeline = build(
            "sun moon alpha-tested shadow map",
            "shadow_vertex",
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::api::Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            },
        );
        let terrain_pipeline = build(
            "sun moon alpha-tested packed terrain shadow map",
            "shadow_terrain_vertex",
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &super::pipelines::TERRAIN_ATTRIBUTES,
            },
        );
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow caster origins"),
            size: ((mesh::MAX_CHUNKS + 2) * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cameras = (0..MAX_CASCADES)
            .map(|_| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("shadow cascade camera"),
                    size: std::mem::size_of::<CameraUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("shadow cascade camera binding"),
                    layout: camera,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                (buffer, bind)
            })
            .collect();
        Self {
            map,
            view,
            layers,
            opaque_depth,
            opaque_depth_view,
            scene_copy,
            scene_copy_view,
            size: (1, 1),
            bind,
            cameras,
            pipeline,
            terrain_pipeline,
            instances,
            runs: Vec::with_capacity(mesh::MAX_CHUNKS * 2 + 2),
            run_ranges: std::array::from_fn(|_| 0..0),
            origins: Vec::with_capacity(mesh::MAX_CHUNKS + 2),
            casters: Vec::with_capacity(mesh::MAX_CHUNKS),
            batch: None,
        }
    }
    /// Resident bytes of the shadow cascade array, opaque depth copy and scene
    /// colour copy (placeholders when the preset has the feature off).
    pub fn target_bytes(&self) -> u64 {
        let map = &self.map;
        let shadow = u64::from(map.width()) * u64::from(map.height())
            * u64::from(map.depth_or_array_layers()) * 4;
        let depth = u64::from(self.opaque_depth.width()) * u64::from(self.opaque_depth.height()) * 4;
        let copy = u64::from(self.scene_copy.width()) * u64::from(self.scene_copy.height()) * 8;
        shadow + depth + copy
    }
    fn map(
        device: &wgpu::Device,
        cascades: u32,
    ) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
        let size = if cascades == 0 { 1 } else { SIZE };
        let layers = cascades.max(1);
        let map = depth_texture(
            device,
            "Fancy shadow cascades",
            size,
            size,
            layers,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let view = map.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_views = (0..layers)
            .map(|layer| {
                map.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        (map, view, layer_views)
    }
    fn scene_copy(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("opaque scene colour copy"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: super::post::HDR,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }
    fn binding(
        device: &wgpu::Device,
        map: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("soft shadow comparison"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("opaque scene copy sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
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
                    resource: wgpu::BindingResource::TextureView(map),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&compare),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&linear),
                },
            ],
        })
    }
    /// (Re)allocates the cascade array and the optional scene-colour copy for
    /// a preset, then rebinds. Cheap placeholders when a feature is off.
    pub fn configure(
        &mut self,
        device: &wgpu::Device,
        cascades: u32,
        scene_copy: bool,
        width: u32,
        height: u32,
    ) {
        let (map, view, layers) = Self::map(device, cascades);
        self.map = map;
        self.view = view;
        self.layers = layers;
        self.size = (width.max(1), height.max(1));
        self.opaque_depth = depth_texture(
            device,
            "Fancy depth texture",
            self.size.0,
            self.size.1,
            1,
            wgpu::TextureUsages::COPY_DST,
        );
        self.opaque_depth_view = self.opaque_depth.create_view(&Default::default());
        let (copy_width, copy_height) = if scene_copy { self.size } else { (1, 1) };
        let (copy, copy_view) = Self::scene_copy(device, copy_width, copy_height);
        self.scene_copy = copy;
        self.scene_copy_view = copy_view;
        self.bind = Self::binding(device, &self.view, &self.opaque_depth_view, &self.scene_copy_view);
    }
    /// `light` points from the scene toward the sun/moon; `cascades` are the
    /// camera-relative orthographic light projections used by the shadow passes.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        chunks: &BTreeMap<[i32; 3], ResidentChunk>,
        scene: &Scene,
        fixture: Option<&ResidentMesh>,
        cascades: &Cascades,
        light: Vec3,
    ) {
        self.runs.clear();
        self.run_ranges = std::array::from_fn(|_| 0..0);
        self.origins.clear();
        self.casters.clear();
        self.origins.push([0., 0., 0., 1.]);
        let origin = relative(scene.origin, scene.camera_position);
        self.origins.push([origin.x, origin.y, origin.z, 1.]);
        let armed = self.batch.is_some();
        if let Some(batch) = self.batch.as_mut() {
            batch.clear();
        }
        let vertex_stride = std::mem::size_of::<TerrainVertex>() as u64;
        let count = cascades.count as usize;
        // Casters need not be inside the camera frustum (that would make
        // shadows pop) but must reach a cascade's orthographic box.
        for (&key, chunk) in chunks {
            if chunk.mesh.indices.is_none() || chunk.visibility.is_none() {
                continue;
            }
            let offset = relative(chunk.origin, scene.camera_position);
            let min = Vec3::from_array(chunk.bounds_min) + offset;
            let max = Vec3::from_array(chunk.bounds_max) + offset;
            let centre = ((min + max) * 0.5).length_squared();
            let mut mask = 0u8;
            for cascade in 0..count {
                if centre < (cascades.radii[cascade] + 32.).powi(2)
                    && !outside_light_box(&cascades.matrices[cascade], min, max)
                {
                    mask |= 1 << cascade;
                }
            }
            if mask != 0 {
                let instance = self.origins.len() as u32;
                self.origins.push([offset.x, offset.y, offset.z, 1.]);
                self.casters.push((key, instance, mask));
            }
        }
        for cascade in 0..count {
            let first = self.runs.len();
            for &(key, instance, mask) in &self.casters {
                if mask & (1 << cascade) == 0 {
                    continue;
                }
                let chunk = &chunks[&key];
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
            self.run_ranges[cascade] = first..self.runs.len();
        }
        if armed {
            let batch = self.batch.as_mut().expect("shadow batch armed");
            for cascade in 0..count {
                let first = batch.draws.len() as u32;
                for run in &self.runs[self.run_ranges[cascade].clone()] {
                    let mesh = match run.key {
                        Some(key) => &chunks[&key].mesh,
                        None => fixture.expect("fixture run without fixture"),
                    };
                    push_shadow_draw(mesh, run, vertex_stride, batch);
                }
                batch.ranges[cascade] = first..batch.draws.len() as u32;
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

/// Cascade radii for a cascade count (1 = Medium's original 80-block box).
pub(super) fn radii(count: u32) -> [f32; MAX_CASCADES] {
    match count {
        0 => [0.; MAX_CASCADES],
        1 => [SINGLE_RADIUS, 0., 0.],
        _ => CASCADE_RADII,
    }
}

/// Light-space orthographic projection of one cascade, centred on the camera
/// and texel-snapped in *world* space: the camera-relative centre is shifted
/// so the world position of the box centre is a whole number of texels along
/// the light's right/up axes. Shadow edges therefore do not swim as the
/// camera moves (`camera_world` is f64 so a far-from-origin camera still snaps).
fn cascade_matrix(light: Vec3, radius: f32, camera_world: [f64; 3]) -> Mat4 {
    let up = if light.y.abs() > 0.98 { Vec3::Z } else { Vec3::Y };
    let forward = -light;
    let right = forward.cross(up).normalize();
    let up = right.cross(forward);
    let texel = f64::from(2. * radius / SIZE as f32);
    let along = |axis: Vec3| {
        let world = camera_world[0] * f64::from(axis.x)
            + camera_world[1] * f64::from(axis.y)
            + camera_world[2] * f64::from(axis.z);
        world.rem_euclid(texel) as f32
    };
    let centre = -(along(right) * right + along(up) * up);
    Mat4::orthographic_rh(-radius, radius, -radius, radius, 1., 2. * LIGHT_DISTANCE)
        * Mat4::look_at_rh(centre + light * LIGHT_DISTANCE, centre, up)
}

pub(super) fn cascades(
    sun_angle: f32,
    moon_angle: f32,
    camera_world: [f64; 3],
    count: u32,
) -> (Cascades, Vec3, f32) {
    let (light, strength) = sky::dominant_light(sun_angle, moon_angle);
    let radii = radii(count);
    let mut matrices = [Mat4::IDENTITY; MAX_CASCADES];
    for cascade in 0..count as usize {
        matrices[cascade] = cascade_matrix(light, radii[cascade], camera_world);
    }
    (
        Cascades {
            count,
            radii,
            matrices,
        },
        light,
        strength,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shadow_cascades_contain_camera_and_switch_to_moon() {
        for angle in [0., 45., 90., 180., 270., 359.] {
            for count in [1, 3] {
                let (cascades, light, intensity) =
                    cascades(angle, angle + 180., [123.4, 70.2, -987.6], count);
                assert!((light.length() - 1.).abs() < 1e-5);
                assert!(light.y >= -1e-5);
                assert!(intensity > 0.);
                for cascade in 0..count as usize {
                    let matrix = cascades.matrices[cascade];
                    assert!(matrix.is_finite());
                    // The camera (relative origin) lies within the box, shifted by
                    // less than one texel from its centre by world snapping.
                    let centre = matrix.project_point3(Vec3::ZERO);
                    let texel_ndc = 2. / SIZE as f32;
                    assert!(
                        centre.x.abs() <= texel_ndc * 1.01 && centre.y.abs() <= texel_ndc * 1.01,
                        "cascade {cascade} at {angle}: {centre:?}"
                    );
                    assert!((0. ..1.).contains(&centre.z));
                }
            }
        }
    }

    #[test]
    fn cascades_nest_and_cover_more_with_each_level() {
        let (cascades, _, _) = cascades(60., 240., [0.; 3], 3);
        assert_eq!(cascades.radii, CASCADE_RADII);
        assert!(cascades.radii.windows(2).all(|pair| pair[0] < pair[1]));
        // Texel density falls monotonically with the cascade index.
        let density: Vec<f32> = cascades.radii.iter().map(|r| SIZE as f32 / (2. * r)).collect();
        assert!(density.windows(2).all(|pair| pair[0] > pair[1]));
        let single = radii(1);
        assert_eq!(single, [SINGLE_RADIUS, 0., 0.]);
        assert_eq!(radii(0), [0.; MAX_CASCADES]);
    }

    /// Texel snapping: the world-space centre of every cascade lands on a
    /// texel boundary of the light's right/up grid for any camera position.
    #[test]
    fn cascade_centre_is_texel_snapped_in_world_space() {
        let (light, _) = sky::dominant_light(50., 230.);
        for camera in [[0.3, 64., 0.7], [1.0e6 + 0.123, 70., -2.5e5 - 0.77], [-33.9, 10., 12.2]] {
            for radius in CASCADE_RADII {
                let matrix = cascade_matrix(light, radius, camera);
                let texel = f64::from(2. * radius / SIZE as f32);
                // World position of the box centre = camera + relative centre.
                // The relative centre projects to NDC (0,0): invert to find it.
                let inverse = matrix.inverse();
                let centre = inverse.project_point3(Vec3::new(0., 0., 0.5));
                let up = if light.y.abs() > 0.98 { Vec3::Z } else { Vec3::Y };
                let right = (-light).cross(up).normalize();
                let up = right.cross(-light);
                for axis in [right, up] {
                    let world = camera[0] * f64::from(axis.x)
                        + camera[1] * f64::from(axis.y)
                        + camera[2] * f64::from(axis.z)
                        + f64::from(centre.dot(axis));
                    let cell = (world / texel).round();
                    assert!(
                        (world - cell * texel).abs() < texel * 0.02 + world.abs() * 1e-7,
                        "radius {radius}: {world} is {} texels off the grid",
                        (world / texel - cell).abs()
                    );
                }
            }
        }
    }
}
