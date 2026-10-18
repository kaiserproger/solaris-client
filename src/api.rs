use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{mpsc, Arc, Condvar, Mutex},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputIntent {
    Key {
        key: String,
        pressed: bool,
    },
    Look {
        yaw: f32,
        pitch: f32,
    },
    Hotbar {
        slot: u8,
    },
    ReleaseAll,
    Ui {
        intent: crate::ui::UiIntent,
    },
    ReloadResources,
    /// F3 debug overlay toggle through the ordinary input owner (the windowed
    /// playthrough drives the same flag the physical F3 key flips).
    DebugOverlay {
        enabled: bool,
    },
}
#[derive(Debug)]
pub enum Command {
    Input(InputIntent),
    Connect {
        address: String,
        username: String,
    },
    Minecraft {
        name: String,
        arguments: Value,
    },
    Disconnect,
    Capture {
        name: String,
        destination: Option<std::path::PathBuf>,
    },
    Shutdown,
}
pub struct Request {
    pub command: Command,
    pub reply: mpsc::SyncSender<Result<Value, String>>,
    pub expected_connection_epoch: Option<String>,
    pub expected_dimension_epoch: Option<String>,
    pub automation_fence: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub cancelled: Option<Arc<std::sync::atomic::AtomicBool>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub instance_id: String,
    pub connection_epoch: String,
    pub dimension_epoch: String,
    pub state_revision: String,
    pub last_applied_packet_sequence: String,
    pub client_tick: String,
    pub server_tick: Option<String>,
    pub resource_epoch: String,
    pub captured_frame_id: Option<String>,
    pub origin: String,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub data: Value,
}
pub struct Shared {
    pub observation: Mutex<Observation>,
    pub changed: Condvar,
    pub owner_wake: Arc<dyn Fn() + Send + Sync>,
}
pub type SharedState = Arc<Shared>;
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub uv: [f32; 2],
    pub overlay_uv: [f32; 2],
    pub overlay_tint: [f32; 3],
    pub alpha: f32,
    pub texture_layer: u32,
    // Sky/block levels in 0..=15. [-1, -1] is explicitly unlit (UI/fixtures).
    pub light: [f32; 2],
    // Source GPU-visible normal; line materials carry their source direction.
    pub normal: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum RenderLayer {
    Opaque = 0,
    Cutout = 1,
    Translucent = 2,
}

#[derive(Clone, Debug, Default)]
pub struct MeshGeometry {
    pub vertices: Vec<Vertex>,
    pub indices: [Vec<u32>; 3],
}

// Entity RenderTypes are not terrain's three chunk layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum EntityMaterial {
    ArmorCutoutNoCull,
    ArmorEntityGlint,
    ArmorTranslucent,
    BannerPattern,
    BeaconBeamOpaque,
    BeaconBeamTranslucent,
    BlockScreenEffect,
    EntityTranslucentEmissive,
    BreezeWind,
    ArmorDecalCutoutNoCull,
    Crumbling,
    CutoutMovingBlock,
    DebugFilledBox,
    DebugPoint,
    DebugQuads,
    DebugTriangleFan,
    DragonRaysDepth,
    DragonRays,
    EndCrystalBeam,
    EndGateway,
    EndPortal,
    EnergySwirl,
    EntityCutoutCull,
    EntityCutoutDissolve,
    EntityCutoutZOffset,
    EntityCutout,
    EntityGlint,
    EntityShadow,
    EntitySolidZOffsetForward,
    EntitySolid,
    EntityTranslucentCullItemTarget,
    EntityTranslucent,
    Eyes,
    FireScreenEffect,
    GlintTranslucent,
    Glint,
    ItemCutout,
    ItemTranslucent,
    Leash,
    Lightning,
    LinesTranslucent,
    Lines,
    Outline,
    OutlineCull,
    SecondaryBlockOutline,
    SolidMovingBlock,
    TextBackgroundSeeThrough,
    TextBackground,
    TextIntensityPolygonOffset,
    TextIntensitySeeThrough,
    TextIntensity,
    TextPolygonOffset,
    TextSeeThrough,
    Text,
    TranslucentMovingBlock,
    WaterMask,
}

impl EntityMaterial {
    pub const COUNT: usize = Self::WaterMask as usize + 1;
    pub const ALL: [Self; Self::COUNT] = [
        Self::ArmorCutoutNoCull,
        Self::ArmorEntityGlint,
        Self::ArmorTranslucent,
        Self::BannerPattern,
        Self::BeaconBeamOpaque,
        Self::BeaconBeamTranslucent,
        Self::BlockScreenEffect,
        Self::EntityTranslucentEmissive,
        Self::BreezeWind,
        Self::ArmorDecalCutoutNoCull,
        Self::Crumbling,
        Self::CutoutMovingBlock,
        Self::DebugFilledBox,
        Self::DebugPoint,
        Self::DebugQuads,
        Self::DebugTriangleFan,
        Self::DragonRaysDepth,
        Self::DragonRays,
        Self::EndCrystalBeam,
        Self::EndGateway,
        Self::EndPortal,
        Self::EnergySwirl,
        Self::EntityCutoutCull,
        Self::EntityCutoutDissolve,
        Self::EntityCutoutZOffset,
        Self::EntityCutout,
        Self::EntityGlint,
        Self::EntityShadow,
        Self::EntitySolidZOffsetForward,
        Self::EntitySolid,
        Self::EntityTranslucentCullItemTarget,
        Self::EntityTranslucent,
        Self::Eyes,
        Self::FireScreenEffect,
        Self::GlintTranslucent,
        Self::Glint,
        Self::ItemCutout,
        Self::ItemTranslucent,
        Self::Leash,
        Self::Lightning,
        Self::LinesTranslucent,
        Self::Lines,
        Self::Outline,
        Self::OutlineCull,
        Self::SecondaryBlockOutline,
        Self::SolidMovingBlock,
        Self::TextBackgroundSeeThrough,
        Self::TextBackground,
        Self::TextIntensityPolygonOffset,
        Self::TextIntensitySeeThrough,
        Self::TextIntensity,
        Self::TextPolygonOffset,
        Self::TextSeeThrough,
        Self::Text,
        Self::TranslucentMovingBlock,
        Self::WaterMask,
    ];
}

#[derive(Clone, Debug)]
pub struct DynamicGeometry {
    pub vertices: Vec<Vertex>,
    pub indices: [Vec<u32>; EntityMaterial::COUNT],
}

impl Default for DynamicGeometry {
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            indices: std::array::from_fn(|_| Vec::new()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChunkDraw {
    pub revision: u64,
    pub origin: [f64; 3],
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// 6x6 face connectivity bits (`from * 6 + to`); open section sets all
    /// 36 bits, fully opaque section sets none. Missing sections are open.
    pub visibility: u64,
    /// CPU upload payload: `Some` while the committed revision still needs a
    /// GPU upload, `None` once that exact revision is uploaded and the CPU
    /// copy released. Metadata, bounds, visibility and revision stay resident
    /// regardless; the renderer keeps the GPU copy and its compact sorting
    /// topology. A released draw with pending scheduling remeshes on demand.
    pub geometry: Option<Arc<MeshGeometry>>,
    /// Source compile policy in milliseconds; unknown policy uploads but never draws.
    pub fade_duration_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkyKind {
    #[default]
    None,
    Overworld,
    End,
}

#[derive(Clone, Copy, Debug)]
pub struct SkyTexture {
    pub uv: [f32; 4],
    pub layer: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ShaderSprite {
    pub uv: [f32; 4],
    pub layer: u32,
}

#[derive(Clone, Debug)]
pub struct WorldEnvironment {
    // Vanilla RGBA8 uses display-encoded colors; authored C02 fixtures use linear RGB.
    pub display_encoded: bool,
    pub light_directions: Option<[[f32; 3]; 2]>,
    pub sky_color: [f32; 3],
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_end: f32,
    pub sky_fog_end: f32,
    pub render_fog_start: f32,
    pub render_fog_end: f32,
    pub fog_alpha: f32,
    pub time_of_day: u64,
    pub rain: f32,
    pub thunder: f32,
    pub sky_kind: SkyKind,
    pub dark_sky_disc: bool,
    // 26.1.2 camera environment attributes, in degrees; these are not derived
    // from a renderer wall clock or assumed 24000-tick legacy day.
    pub sun_angle: f32,
    pub moon_angle: f32,
    pub star_angle: f32,
    pub star_brightness: f32,
    pub sunrise_color: [f32; 4],
    pub sun_texture: Option<SkyTexture>,
    pub moon_texture: Option<SkyTexture>,
    pub end_sky_texture: Option<SkyTexture>,
    pub end_flash_texture: Option<SkyTexture>,
    pub end_flash_intensity: f32,
    pub end_flash_x_angle: f32,
    pub end_flash_y_angle: f32,
}

impl Default for WorldEnvironment {
    fn default() -> Self {
        Self {
            display_encoded: false,
            light_directions: None,
            sky_color: [0.4, 0.6, 0.8],
            fog_color: [0.6, 0.7, 0.8],
            fog_start: 48.,
            fog_end: 128.,
            sky_fog_end: 128.,
            render_fog_start: 48.,
            render_fog_end: 128.,
            fog_alpha: 1.,
            time_of_day: 6000,
            rain: 0.,
            thunder: 0.,
            sky_kind: SkyKind::None,
            dark_sky_disc: false,
            sun_angle: 0.,
            moon_angle: 0.,
            star_angle: 0.,
            star_brightness: 0.,
            sunrise_color: [0.; 4],
            sun_texture: None,
            moon_texture: None,
            end_sky_texture: None,
            end_flash_texture: None,
            end_flash_intensity: 0.,
            end_flash_x_angle: 0.,
            end_flash_y_angle: 0.,
        }
    }
}
#[derive(Clone, Debug)]
pub struct AtlasPatch {
    pub mip_level: u32,
    pub layer: u32,
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub rgba: Arc<Vec<u8>>,
    pub sequence: u64,
}

#[derive(Clone, Debug)]
pub struct AtlasMip {
    pub level: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct LightmapTexture {
    // Identity changes only when the causal 16x16 RGBA lightmap bytes change.
    pub revision: u64,
    pub rgba: Arc<[u8; 1024]>,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub revision: u64,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub origin: [f64; 3],
    pub camera_position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Client-local field-of-view in degrees (Options > FOV, default 70, clamped
    /// 30..=110 by the session; 0/unset means 70 for `Scene::default()` literals).
    pub fov_deg: f32,
    /// GameRenderer.bobHurt camera-space roll, conjugated by the received hurt yaw.
    /// Presentation only: these never change the player's look or movement packets.
    pub hurt_tilt_deg: f32,
    pub hurt_direction: f32,
    /// Subtle strafe lean in degrees about the view axis (positive = clockwise);
    /// presentation only, composed with the hurt tilt in camera space.
    pub roll_deg: f32,
    pub texture_rgba: Option<Arc<Vec<u8>>>,
    pub texture_mips: Option<Arc<Vec<AtlasMip>>>,
    pub texture_width: u32,
    pub texture_height: u32,
    pub texture_revision: u64,
    pub texture_layers: u32,
    pub lightmap: Option<LightmapTexture>,
    pub portal_sprites: Option<[ShaderSprite; 2]>,
    pub shader_game_time: Option<f32>,
    // Newest patch per animated rectangle, retained across skipped render frames.
    // Bootstrap/reload identity is texture_revision; animation never changes it.
    pub texture_patches: Vec<AtlasPatch>,
    pub chunks: BTreeMap<[i32; 3], ChunkDraw>,
    pub dynamic: DynamicGeometry,
    pub entity_presentation: crate::entities::PresentationStats,
    /// Physical-pixel UI geometry, composited in painter order without depth.
    pub overlay: MeshGeometry,
    pub environment: WorldEnvironment,
}
