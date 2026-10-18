use crate::api::{EntityMaterial, Observation, Request, Scene, SkyKind};
use glam::{Mat4, Vec3};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::BufWriter,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Instant,
};
mod arena;
mod frame;
mod material;
mod mdi;
mod mesh;
mod metrics;
mod outline;
mod pipelines;
mod post;
mod shadow;
mod progress;
mod residency;
mod sky;
mod timing;
mod visibility;
#[cfg(test)]
mod shader_tests;
use arena::Arena;
use mesh::{ResidentMesh, Stream};
use pipelines::Pipelines;
use visibility::{Frustum, relative};
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

// Pinned Minecraft RGBA8 textures/targets multiply and blend display-encoded RGB.
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub fn load_texture(path: &std::path::Path) -> Result<(Arc<Vec<u8>>, u32, u32), String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("texture_open_failed {}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    if reader.info().width > 4096 || reader.info().height > 4096 {
        return Err("texture_dimension_limit".into());
    }
    let size = reader
        .output_buffer_size()
        .ok_or("texture_buffer_size_overflow")?;
    if size > 64 * 1024 * 1024 {
        return Err("texture_byte_limit".into());
    }
    let mut bytes = vec![0; size];
    let output = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
    let mut rgba = Vec::with_capacity((output.width * output.height * 4) as usize);
    match output.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(&bytes[..output.buffer_size()]),
        png::ColorType::Rgb => {
            for pixel in bytes[..output.buffer_size()].chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
        }
        png::ColorType::Grayscale => {
            for value in &bytes[..output.buffer_size()] {
                rgba.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in bytes[..output.buffer_size()].chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => return Err("unexpanded_texture_palette".into()),
    }
    Ok((Arc::new(rgba), output.width, output.height))
}

fn adapter_evidence(adapter: &wgpu::Adapter) -> Value {
    let info = adapter.get_info();
    let limits = adapter.limits();
    json!({"name":info.name,"vendor":info.vendor,"device":info.device,
        "device_type":format!("{:?}",info.device_type),"backend":format!("{:?}",info.backend),
        "driver":info.driver,"driver_info":info.driver_info,"features":format!("{:?}",adapter.features()),
        "limits":{"max_texture_dimension_2d":limits.max_texture_dimension_2d,
            "max_buffer_size":limits.max_buffer_size,"max_storage_buffer_binding_size":limits.max_storage_buffer_binding_size,
            "max_bind_groups":limits.max_bind_groups,"max_compute_workgroups_per_dimension":limits.max_compute_workgroups_per_dimension},
        "limits_debug":format!("{limits:?}"),"tier":"portable_indexed",
        "timestamp_query_available":adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY),
        "indirect_first_instance_available":adapter.features().contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)})
}

fn gpu_instance() -> wgpu::Instance {
    // Serialize native loader/ICD negotiation, not rendering or device work.
    // Instances stay distinct: sharing an EGL context across constructor
    // threads can fail with BadAccess even when Vulkan is ultimately selected.
    static INITIALIZE: parking_lot::Mutex<()> = parking_lot::const_mutex(());
    let _guard = INITIALIZE.lock();
    wgpu::Instance::new(&wgpu::InstanceDescriptor::default())
}

pub fn doctor() -> Result<Value, String> {
    let instance = gpu_instance();
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());
    let evidence: Vec<_> = adapters.iter().map(adapter_evidence).collect();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| format!("gpu_adapter_unavailable: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Solaris doctor device"),
        ..Default::default()
    }))
    .map_err(|e| format!("gpu_device_unavailable: {e}"))?;
    let source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("doctor source"),
        contents: &[1, 2, 3, 4],
        usage: wgpu::BufferUsages::COPY_SRC,
    });
    let target = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("doctor readback"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&source, 0, &target, 0, 4);
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    target
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .map_err(|e| format!("gpu_completion_failed: {e}"))?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let verified = &*target.slice(..).get_mapped_range() == [1, 2, 3, 4];
    target.unmap();
    if !verified {
        return Err("gpu_roundtrip_mismatch".into());
    }
    Ok(
        json!({"schema":1,"wgpu":"27.0.1","winit":"0.30.12","adapters":evidence,
        "selected":adapter_evidence(&adapter),"device_creation":"passed","gpu_buffer_roundtrip":"passed",
        "native_window_verified":false,"onscreen_presentation_verified":false,
        "gpu_timestamp_metrics":"not_enabled","renderer":"portable_indexed_depth"}),
    )
}

struct Targets {
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    blit: wgpu::BindGroup,
}

struct Capture {
    buffer: wgpu::Buffer,
    mapped: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    request: Request,
    width: u32,
    height: u32,
    row_bytes: u32,
    path: PathBuf,
    metadata: Value,
    frame: u64,
}

struct ResidentChunk {
    revision: u64,
    origin: [f64; 3],
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    /// 6x6 face connectivity (`from * 6 + to`); copied from `ChunkDraw`.
    /// Stale revisions keep their old mask until the replacement commits.
    visibility_mask: u64,
    fade_duration_ms: Option<u64>,
    uploaded_time_ms: Option<u64>,
    visibility: Option<f32>,
    mesh: ResidentMesh,
}
struct RetiredTexture {
    frame: u64,
    _texture: wgpu::Texture,
    _bind: wgpu::BindGroup,
    bytes: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Chunk([i32; 3]),
    Dynamic(usize),
}
impl Source {
    fn bucket(self) -> usize {
        match self {
            Self::Dynamic(material) => material,
            Self::Chunk(_) => EntityMaterial::COUNT,
        }
    }
}
struct Visible {
    key: [i32; 3],
    instance: u32,
}
#[derive(Clone, Copy)]
struct TransparentDraw {
    source: Source,
    instance: u32,
    first: u32,
    count: u32,
    distance: f32,
    order: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_projection: [f32; 16],
    inverse_view_projection: [f32; 16],
    fog: [f32; 4],
    sky: [f32; 4],
    viewport: [f32; 4],
    celestial: [f32; 4],
    sun_uv: [f32; 4],
    moon_uv: [f32; 4],
    end_uv: [f32; 4],
    layers: [f32; 4],
    sunrise: [f32; 4],
    star: [f32; 4],
    end_flash: [f32; 4],
    end_flash_uv: [f32; 4],
    color_space: [f32; 4],
    light_direction_0: [f32; 4],
    light_direction_1: [f32; 4],
    portal_uv_0: [f32; 4],
    portal_uv_1: [f32; 4],
    material_globals: [f32; 4],
    projection_fog: [f32; 4],
    shadow_projection: [f32; 16],
    fancy: [f32; 4],
    sun_direction: [f32; 4],
    world_origin: [f32; 4],
}
#[derive(Default)]
struct FrameStats {
    terrain_upload_bytes: u64,
    terrain_uploaded_chunks: u64,
    terrain_freed_chunks: u64,
    texture_upload_bytes: u64,
    visible_chunks: u64,
    culled_chunks: u64,
    unknown_fade_policy_sections: u64,
    draw_calls: u64,
    submitted_indices: u64,
    entity_submitted_indices: u64,
    upload_ms: f64,
    visibility_ms: f64,
    draw_encode_ms: f64,
    surface_acquire_ms: f64,
    present_call_ms: f64,
    present_thread_cpu_ms: Option<f64>,
}

pub struct Renderer {
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    present_modes: Vec<wgpu::PresentMode>,
    targets: Targets,
    pipelines: Pipelines,
    outline: outline::Outline,
    post: post::Post,
    shadow: shadow::Shadow,
    fancy_shaders: bool,
    atlas_texture: wgpu::Texture,
    atlas_bind: wgpu::BindGroup,
    lightmap_texture: wgpu::Texture,
    entity_overlay_texture: wgpu::Texture,
    lightmap_revision: Option<u64>,
    texture_revision: Option<u64>,
    texture_activation_frame: Option<u64>,
    texture_bytes: u64,
    texture_layers: u32,
    applied_patches: BTreeMap<(u32, u32, [u32; 2], [u32; 2]), u64>,
    retired_textures: Vec<RetiredTexture>,
    vertex_arena: Arena,
    index_arena: Arena,
    mdi: Option<mdi::Indirect>,
    chunks: BTreeMap<[i32; 3], ResidentChunk>,
    staged_chunks: Vec<([i32; 3], ResidentChunk)>,
    /// Successful terrain uploads awaiting parent drain after submitted-frame
    /// success. Pushed only after `upload_geometry` + resident mapping commit;
    /// drained via `drain_uploaded_meshes` (no per-frame allocation).
    uploaded_meshes: Vec<([i32; 3], u64)>,
    fixture: Option<ResidentMesh>,
    dynamic: Stream<{ EntityMaterial::COUNT }>,
    overlay: Stream,
    sunrise: Stream,
    sunrise_geometry: crate::api::MeshGeometry,
    stars: wgpu::Buffer,
    star_vertices: u32,
    instances: wgpu::Buffer,
    instance_data: Vec<[f32; 4]>,
    visible: Vec<Visible>,
    transparent: Vec<TransparentDraw>,
    transparent_ranges: [std::ops::Range<usize>; EntityMaterial::COUNT + 1],
    /// Reused incoming/expanded-face masks and breadth-first queue.
    occlusion_reached: std::collections::HashMap<[i32; 3], u16>,
    occlusion_queue: Vec<([i32; 3], u8)>,
    camera: wgpu::Buffer,
    camera_bind: wgpu::BindGroup,
    mesh_revision: Option<u64>,
    generation: Option<[String; 3]>,
    reference_mode: bool,
    frame_id: u64,
    spawn_ready_frame: Option<(u64, u64, u64)>,
    monotonic_origin: Instant,
    visibility_time_ms: u64,
    completed_frame: Arc<AtomicU64>,
    capabilities: Value,
    timing: timing::Timing,
    capture: Option<Capture>,
    encoding: bool,
    encoded_tx: mpsc::Sender<(u64, Request, Result<Value, String>)>,
    encoded_rx: mpsc::Receiver<(u64, Request, Result<Value, String>)>,
    gpu_errors: mpsc::Receiver<String>,
    gpu_failed: Arc<std::sync::atomic::AtomicBool>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    progress: progress::Progress,
    artifact_dir: PathBuf,
    last_frame_ms: f64,
    frame_interval_ms: Option<f64>,
    previous_frame: Option<Instant>,
    stats: FrameStats,
}

fn present_mode(vsync: bool, supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::Fifo
    } else if supported.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else if supported.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else {
        wgpu::PresentMode::AutoNoVsync
    }
}

impl Renderer {
    pub async fn new(window: Arc<Window>, artifact_dir: PathBuf) -> Result<Self, String> {
        let instance = gpu_instance();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| e.to_string())?;
        let size = window.inner_size();
        Self::initialize(
            Some(surface),
            adapter,
            size.width.max(1),
            size.height.max(1),
            artifact_dir,
        )
        .await
    }

    /// Real GPU rendering without a window system or a presentation surface.
    /// Scene preparation, passes, frame fences and capture are shared with native.
    pub async fn new_offscreen(
        width: u32,
        height: u32,
        artifact_dir: PathBuf,
    ) -> Result<Self, String> {
        let instance = gpu_instance();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: None,
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("offscreen_gpu_adapter_unavailable: {error}"))?;
        Self::initialize(None, adapter, width, height, artifact_dir).await
    }

    async fn initialize(
        surface: Option<wgpu::Surface<'static>>,
        adapter: wgpu::Adapter,
        width: u32,
        height: u32,
        artifact_dir: PathBuf,
    ) -> Result<Self, String> {
        if width == 0
            || height == 0
            || width > adapter.limits().max_texture_dimension_2d
            || height > adapter.limits().max_texture_dimension_2d
            || u64::from(width) * u64::from(height) * 36 + 16 * 1024 * 1024 > 256 * 1024 * 1024
        {
            return Err("render_target_dimensions_or_memory_budget_exceeded".into());
        }
        let mut capabilities = adapter_evidence(&adapter);
        let downlevel = adapter.get_downlevel_capabilities();
        let timestamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY)
            && downlevel
                .flags
                .contains(wgpu::DownlevelFlags::NONBLOCKING_QUERY_RESOLVE);
        let mdi_enabled = mdi::supported(adapter.features(),
            std::env::var_os("SOLARIS_CLIENT_MDI").is_none_or(|value| value != "0"))
            && downlevel.flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS | wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        let mut features = if timestamps {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        if mdi_enabled { features |= mdi::FEATURES; }
        capabilities["multi_draw_indirect_enabled"] = json!(mdi_enabled);
        capabilities["enabled_features"] = json!(format!("{features:?}"));
        capabilities["timestamp_query_enabled"] = json!(timestamps);
        capabilities["timestamp_query_nonblocking"] = json!(
            downlevel
                .flags
                .contains(wgpu::DownlevelFlags::NONBLOCKING_QUERY_RESOLVE)
        );
        let color_features = adapter.get_texture_format_features(COLOR);
        let depth_features = adapter.get_texture_format_features(DEPTH);
        if !color_features.allowed_usages.contains(
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        ) || !depth_features
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
        {
            return Err("gpu_missing_required_portable_target_formats".into());
        }
        let mut limits = wgpu::Limits::default().using_resolution(adapter.limits());
        limits.max_buffer_size = adapter.limits().max_buffer_size.min(mesh::VERTEX_BUDGET);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Solaris window GPU"),
                required_features: features,
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let (gpu_error_tx, gpu_errors) = mpsc::sync_channel(8);
        let progress_errors = gpu_error_tx.clone();
        let gpu_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lost_failed = gpu_failed.clone();
        let validation_failed = gpu_failed.clone();
        let lost_tx = gpu_error_tx.clone();
        device.set_device_lost_callback(move |reason, message| {
            lost_failed.store(true, Ordering::Release);
            let _ = lost_tx.try_send(format!("gpu_device_lost {reason:?}: {message}"));
        });
        device.on_uncaptured_error(Arc::new(move |error| {
            validation_failed.store(true, Ordering::Release);
            let _ = gpu_error_tx.try_send(format!("gpu_validation_failure: {error}"));
        }));
        let present_modes = surface.as_ref().map_or_else(Vec::new, |surface|
            surface.get_capabilities(&adapter).present_modes);
        capabilities["surface_present_modes"] = json!(present_modes.iter()
            .map(|mode| format!("{mode:?}")).collect::<Vec<_>>());
        let config = if let Some(surface) = &surface {
            let mut config = surface
                .get_default_config(&adapter, width, height)
                .ok_or("surface_configuration_unavailable")?;
            if let Some(format) = surface
                .get_capabilities(&adapter)
                .formats
                .into_iter()
                .find(|format| !format.is_srgb())
            {
                config.format = format;
            }
            config.present_mode = wgpu::PresentMode::Fifo;
            config.desired_maximum_frame_latency = 2;
            surface.configure(&device, &config);
            config
        } else {
            wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: COLOR,
                width,
                height,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: Vec::new(),
            }
        };
        let pipelines = Pipelines::new(&device, config.format);
        let lightmap_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("causal sixteen by sixteen world lightmap"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let entity_overlay_texture = Self::entity_overlay(&device, &queue);
        let (atlas_texture, atlas_bind) = Self::atlas(
            &device,
            &queue,
            &pipelines.atlas_layout,
            &lightmap_texture,
            &entity_overlay_texture,
            &[255; 4],
            &[],
            1,
            1,
            1,
        );
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera environment"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera binding"),
            layout: &pipelines.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera relative chunk instances"),
            size: ((mesh::MAX_CHUNKS + 2) * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let star_data = sky::stars();
        let star_vertices = star_data.len() as u32;
        let stars = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("persistent seeded star geometry"),
            contents: bytemuck::cast_slice(&star_data),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let targets = Self::targets(&device, &config, &pipelines.blit);
        let outline = outline::Outline::new(&device, config.width, config.height);
        let post = post::Post::new(&device, config.width, config.height);
        let mut shadow = shadow::Shadow::new(&device, &pipelines.camera_layout, &pipelines.atlas_layout);
        shadow.resize(&device, config.width, config.height);
        let dynamic = Stream::new(&device);
        let overlay = Stream::new(&device);
        let sunrise = Stream::new(&device);
        let timing = timing::Timing::new(&device, &queue, timestamps);
        let mdi = mdi_enabled.then(|| mdi::Indirect::new(&device));
        let (encoded_tx, encoded_rx) = mpsc::channel();
        if let Ok(error) = gpu_errors.try_recv() {
            return Err(error);
        }
        let progress =
            progress::Progress::new(device.clone(), progress_errors, gpu_failed.clone())?;
        Ok(Self {
            surface,
            device,
            queue,
            config,
            present_modes,
            targets,
            pipelines,
            outline,
            post,
            shadow,
            fancy_shaders: true,
            atlas_texture,
            atlas_bind,
            texture_revision: None,
            texture_bytes: 4,
            texture_layers: 1,
            retired_textures: Vec::new(),
            texture_activation_frame: None,
            applied_patches: BTreeMap::new(),
            lightmap_texture,
            entity_overlay_texture,
            lightmap_revision: None,
            vertex_arena: Arena::new(
                mesh::VERTEX_BUDGET,
                4 * 1024 * 1024,
                wgpu::BufferUsages::VERTEX,
                "terrain_vertex_arena",
            ),
            index_arena: Arena::new(
                mesh::INDEX_BUDGET,
                2 * 1024 * 1024,
                wgpu::BufferUsages::INDEX,
                "terrain_index_arena",
            ),
            mdi,
            chunks: BTreeMap::new(),
            fixture: None,
            staged_chunks: Vec::new(),
            uploaded_meshes: Vec::new(),
            dynamic,
            overlay,
            sunrise,
            sunrise_geometry: Default::default(),
            stars,
            star_vertices,
            instances,
            instance_data: Vec::with_capacity(mesh::MAX_CHUNKS + 2),
            visible: Vec::with_capacity(mesh::MAX_CHUNKS),
            transparent: Vec::new(),
            transparent_ranges: std::array::from_fn(|_| 0..0),
            occlusion_reached: std::collections::HashMap::with_capacity(mesh::MAX_CHUNKS / 4),
            occlusion_queue: Vec::with_capacity(1024),
            camera,
            camera_bind,
            mesh_revision: None,
            generation: None,
            reference_mode: false,
            frame_id: 0,
            spawn_ready_frame: None,
            completed_frame: Arc::new(AtomicU64::new(0)),
            capabilities,
            timing,
            monotonic_origin: Instant::now(),
            visibility_time_ms: 0,
            capture: None,
            encoding: false,
            encoded_tx,
            encoded_rx,
            gpu_errors,
            gpu_failed,
            artifact_dir,
            wake: None,
            progress,
            last_frame_ms: 0.,
            frame_interval_ms: None,
            previous_frame: None,
            stats: Default::default(),
        })
    }

    pub fn set_reference_mode(&mut self, reference: bool) {
        if self.reference_mode != reference {
            self.reference_mode = reference;
        }
    }

    /// Exact `(key, revision)` acks for terrain uploads that committed GPU
    /// bytes and resident mapping. Parent drains after `render` reports a
    /// successful submission and releases the matching CPU payload via
    /// `Terrain::release_uploaded`; stale acks never match a replacement
    /// there. No per-frame allocation: borrows `self` through the drain.
    /// Reference-mode deliberate full uploads omit acks so the retained
    /// source survives per-frame rewrites.
    pub(crate) fn drain_uploaded_meshes(
        &mut self,
    ) -> impl Iterator<Item = ([i32; 3], u64)> + '_ {
        self.uploaded_meshes.drain(..)
    }

    pub fn set_fancy_shaders(&mut self, fancy: bool) {
        self.fancy_shaders = fancy;
    }

    pub fn set_vsync(&mut self, enabled: bool) {
        let mode = present_mode(enabled, &self.present_modes);
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            if let Some(surface) = &self.surface {
                surface.configure(&self.device, &self.config);
            }
        }
    }

    /// Atlas activation is acknowledged only after its actual submitted frame
    /// completes; neither a CPU catalog nor queued texture writes are an ACK.
    pub fn resource_revision(&self) -> Option<u64> {
        if self.gpu_failed.load(Ordering::Acquire) {
            return None;
        }
        self.texture_activation_frame
            .filter(|frame| *frame <= self.completed_frame.load(Ordering::Acquire))
            .and(self.texture_revision)
    }

    /// Call before driving an event-only loop. GPU/map/PNG completions wake the
    /// same owner; a timer is not required to poll asynchronous capture work.
    pub fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.progress.set_wake(wake.clone());
        self.wake = Some(wake);
    }

    fn entity_overlay(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
        let mut pixels = [0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let rgba = if y < 8 {
                    [255, 0, 0, 178]
                } else {
                    [255, 255, 255, ((1. - x as f32 / 15. * 0.75) * 255.) as u8]
                };
                pixels[(y * 16 + x) * 4..(y * 16 + x + 1) * 4].copy_from_slice(&rgba);
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pinned Source hurt white overlay palette"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(64),
                rows_per_image: Some(16),
            },
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
        );
        texture
    }

    fn atlas(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        lightmap: &wgpu::Texture,
        entity_overlay: &wgpu::Texture,
        pixels: &[u8],
        mips: &[crate::api::AtlasMip],
        width: u32,
        height: u32,
        layers: u32,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("verified shared texture array"),
            size,
            mip_level_count: 1 + mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        for mip in mips {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip.level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                mip.rgba.as_slice(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mip.width * 4),
                    rows_per_image: Some(mip.height),
                },
                wgpu::Extent3d {
                    width: mip.width,
                    height: mip.height,
                    depth_or_array_layers: layers,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest texel, linear mip packed material sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            // Pinned GlSampler NEAREST maps to GL_NEAREST_MIPMAP_LINEAR (9986).
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Chunk rendering reconstructs nearest texel coverage in terrain.fsh's
        // sampleNearest, using this dedicated LINEAR sampler (LevelRenderer).
        let terrain_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("source chunk linear texel and mip sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let lightmap_view = lightmap.create_view(&Default::default());
        let entity_overlay_view = entity_overlay.create_view(&Default::default());
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clamped linear lightmap vertex sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shared texture array binding"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&lightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&lightmap_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&terrain_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&entity_overlay_view),
                },
            ],
        });
        (texture, bind)
    }

    fn targets(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        pipeline: &wgpu::RenderPipeline,
    ) -> Targets {
        let size = wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("actual scene capture/present target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        let blit = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("actual frame blit"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&color_view),
            }],
        });
        Targets {
            color,
            color_view,
            _depth: depth,
            depth_view,
            blit,
        }
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.targets = Self::targets(&self.device, &self.config, &self.pipelines.blit);
        self.post = post::Post::new(&self.device, self.config.width, self.config.height);
        self.shadow.resize(&self.device, self.config.width, self.config.height);
        self.outline
            .resize(&self.device, self.config.width, self.config.height);
    }

    pub fn render(
        &mut self,
        scene: &Scene,
        capture: Option<Request>,
        observation: &Observation,
    ) -> Result<(), String> {
        let start = Instant::now();
        if let Some(request) = &capture {
            let cancelled = request
                .automation_fence
                .as_ref()
                .is_some_and(|f| !f.load(Ordering::Acquire));
            let stale = request
                .expected_connection_epoch
                .as_ref()
                .is_some_and(|e| e != &observation.connection_epoch)
                || request
                    .expected_dimension_epoch
                    .as_ref()
                    .is_some_and(|e| e != &observation.dimension_epoch);
            if cancelled || stale {
                let request = capture.unwrap();
                let _ = request.reply.send(Err(if cancelled {
                    "capture_cancelled"
                } else {
                    "stale_capture_context"
                }
                .into()));
                return Ok(());
            }
        }
        self.collect_completed();
        if let Ok(error) = self.gpu_errors.try_recv() {
            if let Some(request) = capture {
                let _ = request.reply.send(Err(error.clone()));
            }
            return Err(error);
        }
        if self
            .frame_id
            .saturating_sub(self.completed_frame.load(Ordering::Acquire))
            >= 3
        {
            if let Some(request) = capture {
                let _ = request.reply.send(Err("gpu_frames_in_flight_limit".into()));
            }
            return Ok(());
        }
        self.stats = FrameStats::default();
        let acquisition = Instant::now();
        let frame = match self
            .surface
            .as_ref()
            .map(wgpu::Surface::get_current_texture)
        {
            None => None,
            Some(Ok(frame)) => Some(frame),
            Some(Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated)) => {
                self.surface
                    .as_ref()
                    .unwrap()
                    .configure(&self.device, &self.config);
                if let Some(request) = capture {
                    let _ = request
                        .reply
                        .send(Err("surface_reconfigured_retry_capture".into()));
                }
                return Ok(());
            }
            Some(Err(wgpu::SurfaceError::Timeout)) => {
                if let Some(request) = capture {
                    let _ = request.reply.send(Err("window_not_presentable".into()));
                }
                return Ok(());
            }
            Some(Err(e)) => {
                if let Some(request) = capture {
                    let _ = request.reply.send(Err(format!("surface_failure: {e}")));
                }
                return Err(format!("surface_failure: {e}"));
            }
        };
        if self.surface.is_some() {
            self.stats.surface_acquire_ms = acquisition.elapsed().as_secs_f64() * 1000.;
        }
        let mut encoder = match self.encode_scene(scene, observation, frame.as_ref()) {
            Ok(encoder) => encoder,
            Err(error) => {
                if let Some(request) = capture {
                    let _ = request.reply.send(Err(error.clone()));
                }
                return Err(error);
            }
        };
        self.frame_id += 1;
        let mut new_capture = None;
        if let Some(request) = capture {
            let (name, destination) = match &request.command {
                crate::api::Command::Capture { name, destination } => {
                    (name.clone(), destination.clone())
                }
                _ => unreachable!(),
            };
            let valid = !name.is_empty()
                && name.len() <= 100
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
                && name != "."
                && name != "..";
            if !valid {
                let _ = request.reply.send(Err("invalid_artifact_name".into()));
            } else if self.capture.is_some() || self.encoding {
                let _ = request.reply.send(Err("capture_busy".into()));
            } else {
                let width = self.config.width;
                let height = self.config.height;
                let row_bytes = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                    * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                let size = u64::from(row_bytes) * u64::from(height);
                if size > 64 * 1024 * 1024 {
                    let _ = request.reply.send(Err("capture_byte_limit".into()));
                } else {
                    let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("actual target readback"),
                        size,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.targets.color,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(row_bytes),
                                rows_per_image: Some(height),
                            },
                        },
                        wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                    );
                    let metadata = json!({"frame_id":self.frame_id.to_string(),"state_revision":observation.state_revision,
                        "connection_epoch":observation.connection_epoch,"dimension_epoch":observation.dimension_epoch,
                        "resource_epoch":observation.resource_epoch,"width":width,"height":height,
                        "render_target":if self.surface.is_some() {"actual_window_scene_target"} else {"actual_offscreen_scene_target"},
                        "gpu_completion":true,"present_requested":self.surface.is_some(),
                        "display_scanout_verified":false,"offscreen_only":self.surface.is_none(),"window":observation.data.get("window"),
                        "scene_revision":scene.revision.to_string(),"origin":scene.origin,"camera_position":scene.camera_position,
                        "yaw":scene.yaw,"pitch":scene.pitch});
                    let mut metadata = metadata;
                    // A complete preceding frame, including client-owner work
                    // outside Renderer::render. Its own frame_id fences the sample.
                    metadata["previous_render_metrics"] = observation.data["render"].clone();
                    metadata["render_mode"] = json!(if self.reference_mode {
                        "reference"
                    } else {
                        "optimized"
                    });
                    let passes = self.timing.pass_mask();
                    metadata["passes"] = Value::Array(
                        timing::PASSES
                            .iter()
                            .enumerate()
                            .filter(|(index, _)| passes & (1 << index) != 0)
                            .map(|(_, name)| json!(name))
                            .collect(),
                    );
                    metadata["texture_revision"] = json!(scene.texture_revision.to_string());
                    metadata["lightmap_revision"] =
                        json!(self.lightmap_revision.map(|revision| revision.to_string()));
                    metadata["entity_extraction"] = json!(scene.entity_presentation);
                    let emitted_indices =
                        scene.dynamic.indices.iter().map(Vec::len).sum::<usize>() as u64;
                    let submitted_entities = (self.stats.entity_submitted_indices
                        == emitted_indices)
                        .then_some(scene.entity_presentation.submitted);
                    metadata["entity_frame_counts"] = json!({
                        "extracted":scene.entity_presentation.submitted,
                        "submitted":submitted_entities,"gpu_completed":submitted_entities,
                        "visible_pixels_proven":false});
                    metadata["entity_geometry"] = json!({
                        "extracted_vertices":scene.dynamic.vertices.len(),
                        "extracted_indices":scene.dynamic.indices.iter().map(Vec::len).sum::<usize>(),
                        "submitted_indices":self.stats.entity_submitted_indices,
                        "gpu_completed_indices":self.stats.entity_submitted_indices,
                        "completion_scope":"this captured frame submission; not visible-pixel count"});
                    metadata["terrain_extraction"] = observation
                        .data
                        .get("terrain_capture")
                        .cloned()
                        .unwrap_or(Value::Null);
                    metadata["terrain_submission"] = self.section_upload_receipts();
                    metadata["texture_animation_sequence"] = json!(
                        self.applied_patches
                            .values()
                            .max()
                            .copied()
                            .unwrap_or(0)
                            .to_string()
                    );
                    metadata["gpu_timestamps"] = self.timing.latest.clone();
                    metadata["submitted_frame_interval_ms"] = json!(self.frame_interval_ms);
                    metadata["frame_interval_sample_frame_id"] = json!(self.frame_id.saturating_sub(1).to_string());
                    metadata["environment"] = json!({"sky_kind":format!("{:?}",scene.environment.sky_kind),
                        "sun_angle":scene.environment.sun_angle,"moon_angle":scene.environment.moon_angle,
                        "star_angle":scene.environment.star_angle,"star_brightness":scene.environment.star_brightness,
                        "rain":scene.environment.rain,"thunder":scene.environment.thunder,"sky_color":scene.environment.sky_color,
                        "fog_color":scene.environment.fog_color,"fog_start":scene.environment.fog_start,"fog_end":scene.environment.fog_end,
                        "sky_fog_end":scene.environment.sky_fog_end,"render_fog_start":scene.environment.render_fog_start,
                        "render_fog_end":scene.environment.render_fog_end,"fog_alpha":scene.environment.fog_alpha,
                        "time_of_day":scene.environment.time_of_day.to_string()});
                    new_capture = Some((
                        buffer,
                        request,
                        width,
                        height,
                        row_bytes,
                        destination
                            .unwrap_or_else(|| self.artifact_dir.join(format!("{name}.png"))),
                        metadata,
                    ));
                }
            }
        }
        let submission = self.queue.submit([encoder.finish()]);
        if self.texture_activation_frame.is_none() {
            self.texture_activation_frame = Some(self.frame_id);
        }
        let completed = self.completed_frame.clone();
        let submitted_frame = self.frame_id;
        let wake = self.wake.clone();
        self.queue.on_submitted_work_done(move || {
            completed.fetch_max(submitted_frame, Ordering::Release);
            if let Some(wake) = wake {
                wake();
            }
        });
        self.timing.submitted(self.frame_id);
        if let Some(frame) = frame {
            let presentation = Instant::now();
            let cpu = timing::thread_cpu_seconds();
            frame.present();
            self.stats.present_call_ms = presentation.elapsed().as_secs_f64() * 1000.;
            self.stats.present_thread_cpu_ms = cpu.zip(timing::thread_cpu_seconds())
                .map(|(start, end)| (end - start).max(0.) * 1000.);
        }
        let now = Instant::now();
        self.frame_interval_ms = self
            .previous_frame
            .map(|old| now.duration_since(old).as_secs_f64() * 1000.);
        self.previous_frame = Some(now);
        if let Some((buffer, request, width, height, row_bytes, path, metadata)) = new_capture {
            let (tx, mapped) = mpsc::sync_channel(1);
            let wake = self.wake.clone();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                    if let Some(wake) = wake {
                        wake();
                    }
                });
            self.capture = Some(Capture {
                buffer,
                mapped,
                request,
                width,
                height,
                row_bytes,
                path,
                metadata,
                frame: self.frame_id,
            });
        }
        self.progress.submitted(submission)?;
        self.last_frame_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    pub fn poll_capture(&mut self) -> Option<u64> {
        self.collect_completed();
        let mapped = self
            .capture
            .as_ref()
            .and_then(|capture| capture.mapped.try_recv().ok());
        if let Some(result) = mapped {
            let capture = self.capture.take().unwrap();
            if let Err(e) = result {
                let _ = capture
                    .request
                    .reply
                    .send(Err(format!("gpu_readback_failed: {e}")));
            } else {
                self.encoding = true;
                let tx = self.encoded_tx.clone();
                let wake = self.wake.clone();
                std::thread::spawn(move || {
                    let result = (|| -> Result<Value, String> {
                        if capture
                            .request
                            .automation_fence
                            .as_ref()
                            .is_some_and(|fence| !fence.load(std::sync::atomic::Ordering::Acquire))
                        {
                            return Err("capture_cancelled".into());
                        }
                        let mapped = capture.buffer.slice(..).get_mapped_range();
                        let mut pixels =
                            Vec::with_capacity((capture.width * capture.height * 4) as usize);
                        for row in mapped.chunks(capture.row_bytes as usize) {
                            pixels.extend_from_slice(&row[..(capture.width * 4) as usize]);
                        }
                        drop(mapped);
                        capture.buffer.unmap();
                        let mut options = OpenOptions::new();
                        options.write(true).create_new(true);
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::OpenOptionsExt;
                            options.mode(0o600);
                        }
                        let file = options
                            .open(&capture.path)
                            .map_err(|e| format!("artifact_create_failed: {e}"))?;
                        let mut encoder =
                            png::Encoder::new(BufWriter::new(file), capture.width, capture.height);
                        encoder.set_color(png::ColorType::Rgba);
                        encoder.set_depth(png::BitDepth::Eight);
                        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
                        writer
                            .write_image_data(&pixels)
                            .map_err(|e| e.to_string())?;
                        writer.finish().map_err(|e| e.to_string())?;
                        if capture
                            .request
                            .automation_fence
                            .as_ref()
                            .is_some_and(|f| !f.load(Ordering::Acquire))
                        {
                            let _ = std::fs::remove_file(&capture.path);
                            return Err("capture_cancelled".into());
                        }
                        let mut metadata = capture.metadata;
                        let artifact_name = match &capture.request.command {
                            crate::api::Command::Capture { name, .. } => name,
                            _ => unreachable!(),
                        };
                        metadata["artifact_uri"] =
                            json!(format!("solaris-client://artifacts/{artifact_name}.png"));
                        metadata["file_name"] =
                            json!(capture.path.file_name().unwrap().to_string_lossy());
                        metadata["path"] = json!(capture.path);
                        Ok(metadata)
                    })();
                    let _ = tx.send((capture.frame, capture.request, result));
                    if let Some(wake) = wake {
                        wake();
                    }
                });
            }
        }
        if let Ok((frame, request, result)) = self.encoded_rx.try_recv() {
            self.encoding = false;
            let successful = result.is_ok();
            let _ = request.reply.send(result);
            if successful {
                return Some(frame);
            }
        }
        None
    }
}
