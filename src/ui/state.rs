//! Retained paint/hit/accessibility owner. Network effects are typed domain commands only.
use super::{
    edit::EditBuffer,
    items::{ItemContext, ItemModels},
    render::{Painter, TextRenderer},
    ClickMode, Modifiers, PointerButton, PointerPhase, UiIntent,
};
use crate::{
    api::{MeshGeometry, RenderLayer, Vertex},
    inventory::{InventoryStore, MenuKind, UiContext},
    protocol::{content::Content, models::TintKind, Result},
    render::GraphicsPreset,
};
use glam::{Mat4, Vec3};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
const NODE_LIMIT: usize = 512;
const TEXT_LIMIT: usize = 65536;
#[derive(Clone, Debug)]
pub enum ClipboardRequest {
    Get,
    Set(String),
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ImeState {
    pub allowed: bool,
    pub position: [f64; 2],
    pub size: [f64; 2],
}
#[derive(Clone, Debug)]
pub enum WindowEffect {
    Fullscreen(bool),
    UiScale(f64),
    Narrate(String),
}
#[derive(Clone, Debug)]
pub struct CommandEdit {
    pub pos: [i32; 3],
    pub command: String,
    pub mode: i32,
    pub track_output: bool,
    pub conditional: bool,
    pub automatic: bool,
    pub entity_id: Option<i32>,
}
#[derive(Clone, Debug)]
pub struct StructureEdit {
    pub pos: [i32; 3],
    pub update: i32,
    pub mode: i32,
    pub name: String,
    pub offset: [i32; 3],
    pub size: [i32; 3],
    pub mirror: i32,
    pub rotation: i32,
    pub data: String,
    pub integrity: f32,
    pub seed: i64,
    pub ignore_entities: bool,
    pub strict: bool,
    pub show_air: bool,
    pub show_box: bool,
}
#[derive(Clone, Debug)]
pub struct JigsawEdit {
    pub pos: [i32; 3],
    pub name: String,
    pub target: String,
    pub pool: String,
    pub final_state: String,
    pub joint: String,
    pub selection_priority: i32,
    pub placement_priority: i32,
}
#[derive(Clone, Debug)]
pub enum EditorState {
    Command(CommandEdit),
    Structure(StructureEdit),
    Jigsaw(JigsawEdit),
    Book {
        slot: i32,
        pages: Vec<String>,
        title: Option<String>,
        author: Option<String>,
        writable: bool,
    },
    Sign {
        pos: [i32; 3],
        front: bool,
        lines: [String; 4],
    },
}
#[derive(Clone, Debug)]
pub enum UiCommand {
    Loader(Value),
    /// Dialog `custom` action: `custom_click_action(id, payload)`.
    DialogClick {
        id: String,
        payload: Option<crate::protocol::nbt::Nbt>,
    },
    Inventory(UiIntent),
    InventoryGesture(Vec<UiIntent>),
    CreativePick {
        context: UiContext,
        stack: crate::inventory::Stack,
        button: u8,
        mode: ClickMode,
    },
    CreativeClear {
        context: UiContext,
        all: bool,
    },
    CreativeSetStack {
        context: UiContext,
        slot: i16,
        stack: crate::inventory::Stack,
    },
    OpenUrl(String),
    InventoryRename {
        context: UiContext,
        text: String,
        hover_name: String,
    },
    ChatAck {
        offset: i32,
    },
    CommandSuggestion { id: i32, command: String },
    Chat {
        message: String,
        command: bool,
    },
    Sign {
        pos: [i32; 3],
        front: bool,
        lines: [String; 4],
    },
    Book {
        slot: i32,
        pages: Vec<String>,
        title: Option<String>,
    },
    Command(CommandEdit),
    Structure(StructureEdit),
    Jigsaw(JigsawEdit),
    JigsawGenerate {
        pos: [i32; 3],
        levels: i32,
        keep: bool,
    },
    Respawn,
    RequestStatistics,
    SeenAdvancements {
        tab: Option<String>,
    },
    Disconnect,
    Connect {
        address: String,
        username: String,
    },
    Shutdown,
    /// Start the local server for a world folder and join it.
    PlayWorld {
        dir_name: String,
    },
    CancelLocalStart,
    ReloadResources,
    Window(WindowEffect),
    Settings {
        key: String,
        value: Value,
    },
}
#[derive(Default)]
pub struct UiEffects {
    pub commands: Vec<UiCommand>,
    pub release_input: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudState {
    pub health: Option<f32>,
    pub max_health: Option<f32>,
    pub absorption: f32,
    pub food: Option<i32>,
    pub armor: Option<i32>,
    pub air: Option<i32>,
    pub max_air: Option<i32>,
    pub experience: Option<f32>,
    pub level: Option<i32>,
    pub player_tick: Option<i32>,
    pub experience_display_start_tick: Option<i32>,
    pub jump_cooldown: Option<i32>,
    pub riding_health: Option<f32>,
    pub riding_max_health: Option<f32>,
    pub jump: Option<f32>,
    pub attack: Option<f32>,
    /// Darkness vignette strength under the HUD, 0 (none) ..= 1.
    pub vignette: f32,
    pub hardcore: bool,
    pub heart_kind: String,
    pub hunger: bool,
    pub effects: Vec<EffectState>,
    pub players: Vec<PlayerListEntry>,
    pub player_list_visible: bool,
    pub scoreboard_name: Option<String>,
    pub locator: Option<super::hud::LocatorCamera>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct EffectState {
    pub id: String,
    pub amplifier: i32,
    pub ticks: i32,
    pub ambient: bool,
    pub visible: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerListEntry {
    pub id: String,
    pub name: Value,
    pub latency: i32,
    pub game_mode: i32,
}
#[derive(Clone, Debug)]
pub struct UiPresentation {
    pub hud: HudState,
    pub loading: Option<String>,
    pub death: Option<Value>,
    pub can_respawn: bool,
    pub editor: Option<EditorState>,
    pub can_edit_operator_blocks: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    Game,
    Menu,
    Chat,
    Book,
    Sign,
    Command,
    Structure,
    Jigsaw,
    Death,
    Loading,
    Pause,
    Settings,
    Accessibility,
    Advancements,
    Statistics,
    Creative,
    Loader,
    /// Server-driven `show_dialog` screen ([`super::dialog`]).
    ServerDialog,
    Title,
    Multiplayer,
    DirectConnect,
    Disconnected,
    Singleplayer,
    CreateWorld,
    DeleteWorld,
    SingleplayerStarting,
}
#[derive(Clone, Debug, Serialize)]
pub struct SemanticNode {
    pub id: String,
    pub role: &'static str,
    pub label: String,
    pub component: Option<Arc<Value>>,
    pub bounds: [f32; 4],
    pub enabled: bool,
    pub visible: bool,
    /// Paint clip override for nodes only partly inside a scroll viewport.
    #[serde(skip)]
    pub(crate) clip: Option<[f32; 4]>,
    pub deny_reason: Option<&'static str>,
    pub focused: bool,
    pub owner: &'static str,
    #[serde(skip)]
    pub(crate) action: NodeAction,
    #[serde(skip)]
    pub(crate) paint: Paint,
}
#[derive(Clone, Debug, Default)]
pub(crate) enum NodeAction {
    #[default]
    None,
    Slot(i16),
    MenuButton(i32),
    Focus(String),
    Close,
    Send,
    Respawn,
    Disconnect,
    Screen(Screen),
    StatisticsGroup(super::statistics::StatisticsGroup),
    AdvancementTab(String),
    Connect,
    Quit,
    AddServer,
    SaveServer,
    SelectServer(usize),
    EditServer,
    DeleteServer,
    ConfirmDeleteServer,
    CancelDeleteServer,
    JoinServer(usize),
    ServerPage(i32),
    World(usize),
    PlayWorld,
    CancelLocalStart,
    OpenWorldsFolder,
    WorldOption(&'static str),
    CreateWorld,
    DeleteWorld,
    RecipeBook,
    RecipeFilter,
    SlotToggle(i16),
    BeaconEffect {
        primary: bool,
        effect: i32,
    },
    BeaconSubmit,
    Recipe(i32),
    RecipeCategory(String),
    Creative(u32),
    CreativeTab(usize),
    CreativeClear,
    HotbarSave(usize),
    HotbarLoad(usize),
    ComponentClick(Arc<Value>),
    LinkAccept,
    LinkCopy,
    LinkCancel,
    Submit,
    Page(i32),
    Setting(&'static str),
    /// Slider row: activation cycles like `Setting`; pointer/keys/wheel set exact stops.
    Slider(&'static str),
    OptionsTab(usize),
    Rebind(&'static str),
    ResetBinding(&'static str),
    ResetBindings,
    EditorToggle(&'static str),
    Generate,
    Loader(super::loader::LoaderControl),
    Dialog(super::dialog::DialogControl),
}
#[derive(Clone, Debug, Default)]
pub(crate) enum Paint {
    #[default]
    Text,
    HitRegion,
    RichText(Arc<super::shaping::TextLayout>),
    OutlinedText(Arc<super::shaping::TextLayout>, u32, u32),
    Button,
    /// Options category tab (`true` = selected).
    Tab(bool),
    /// Options slider; `t` is the 0..1 stop position, `desc` the hover description.
    Slider {
        t: f32,
        desc: &'static str,
    },
    Toggle {
        on: bool,
        desc: &'static str,
    },
    /// Row split at `delim` into a left label and a right-aligned value.
    Split {
        delim: &'static str,
        desc: &'static str,
        conflict: bool,
    },
    /// Themed translucent panel.
    Frame,
    Hotbar,
    HotbarSelection,
    Field(String),
    Image(String, [f32; 4]),
    TintedImage(String, [f32; 3]),
    Slot(usize),
    PlayerPreview,
    Creative(u32),
    Variants(Arc<Vec<crate::inventory::Stack>>, bool),
    RecipeDisplay(i32),
    TradeItem {
        offer: usize,
        part: u8,
    },
    Map(i32),
    LoaderMap(Vec<[f64; 2]>),
    LoaderPreview(Arc<crate::loader::previews::WorldPreview>),
    Solid([f32; 3], f32),
}
struct PlayerPreview {
    texture: crate::protocol::content::TextureRegion,
    model: crate::entities::PlayerModel,
    quads: Vec<[Vertex; 4]>,
    order: Vec<(usize, f32)>,
}
pub struct UiState {
    pub(crate) text: Arc<TextRenderer>,
    pub(crate) models: ItemModels,
    entity_models: Option<Arc<crate::entities::Models>>,
    player_preview: Option<PlayerPreview>,
    pub(crate) screen: Screen,
    pub(crate) revision: u64,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) scale: f64,
    framebuffer_size: [u32; 2],
    pub(crate) nodes: Vec<SemanticNode>,
    pub(crate) dirty: bool,
    pub(crate) inventory_revision: u64,
    pub(crate) inventory_instance: u64,
    pub(crate) focus: Option<String>,
    pub(crate) fields: BTreeMap<String, EditBuffer>,
    pub(crate) editor: Option<EditorState>,
    pub(crate) loader: Option<super::loader::LoaderView>,
    pub(crate) operator_permission: bool,
    pub(crate) hud: HudState,
    pub(crate) network_hud: super::hud::HudPackets,
    pub(crate) hud_sidebar: Option<Arc<super::hud_view::Sidebar>>,
    pub(crate) hud_level: Option<(i32, Arc<super::hud_view::SidebarText>)>,
    pub(crate) hud_waypoint_styles: BTreeMap<String, Arc<super::hud_locator::WaypointStyle>>,
    pub(crate) loading: Option<String>,
    pub(crate) death: Option<Value>,
    pub(crate) can_respawn: bool,
    pub(crate) clipboard: VecDeque<ClipboardRequest>,
    pub(crate) clipboard_fence: Option<(u64, String)>,
    pub(crate) history: VecDeque<String>,
    pub(crate) history_index: Option<usize>,
    pub(crate) scroll: usize,
    pub(crate) page: usize,
    pub(crate) recipe_open: bool,
    pub(crate) recipe_filter: bool,
    pub(crate) recipe_category: Option<String>,
    pub(crate) settings: BTreeMap<String, Value>,
    pub(crate) rebind: Option<&'static str>,
    pub(crate) tick: u64,
    pub(crate) pointer: [f32; 2],
    pub(crate) drag: Option<PointerButton>,
    pub(crate) drag_slots: Vec<i16>,
    pub(crate) drag_carrying: bool,
    pub(crate) drag_fence: Option<UiContext>,
    last_slot_click: Option<(i16, u8, Instant)>,
    collect_on_release: bool,
    pub(crate) release_pending: bool,
    pub(crate) chat: VecDeque<super::packets::ChatLine>,
    pub(crate) maps: BTreeMap<i32, super::packets::MapState>,
    pub(crate) bosses: BTreeMap<String, super::packets::BossBar>,
    pub(crate) title: Option<Value>,
    pub(crate) subtitle: Option<Value>,
    pub(crate) title_timing: [i32; 3],
    pub(crate) title_started: u64,
    pub(crate) tab_header: Option<Value>,
    pub(crate) tab_footer: Option<Value>,
    pub(crate) advancements: BTreeMap<String, super::packets::AdvancementState>,
    pub(crate) statistics: super::statistics::StatisticsState,
    pub(crate) statistics_group: super::statistics::StatisticsGroup,
    pub(crate) selected_advancement: Option<String>,
    pub(crate) creative_source: Arc<Value>,
    pub(crate) enabled_features: Arc<BTreeSet<String>>,
    pub(crate) creative_tab: usize,
    pub(crate) creative_stacks: BTreeMap<u32, crate::inventory::Stack>,
    pub(crate) saved_hotbars: [Option<Arc<Vec<crate::inventory::Stack>>>; 9],
    pub(crate) book_components: Vec<Arc<Value>>,
    pub(crate) pending_link: Option<String>,
    pub(crate) recipe_stacks: BTreeMap<i32, (Arc<Value>, Vec<crate::inventory::Stack>)>,
    pub(crate) recipe_names: BTreeMap<i32, (Arc<Value>, String)>,
    pub(crate) ghost_presentation: Option<(Value, Vec<(usize, Arc<Vec<crate::inventory::Stack>>)>)>,
    pub(crate) item_context: ItemContext,
    pub(crate) signature_cache: super::chat_tracking::SignatureCache,
    pub(crate) packet_commands: VecDeque<UiCommand>,
    pub(crate) chat_receipts: VecDeque<super::packets::ChatReceipt>,
    pub(crate) last_seen: super::chat_tracking::LastSeenTracker,
    pub(crate) command_completion: super::commands::CommandState,
    pub(crate) chat_expected_index: i32,
    pub(crate) chat_sequence: u64,
    pub(crate) beacon_selection: [Option<i32>; 2],
    beacon_source: [Option<i32>; 2],
    bundle_scroll: [f64; 2],
    bundle_hover: Option<i16>,
    pub(crate) selected_slot: u8,
    pub(crate) selected_name_tick: u64,
    pub(crate) debug_lines: Vec<String>,
    pub(crate) title_error: Option<String>,
    pub(crate) last_server: Option<String>,
    pub(crate) last_username: Option<String>,
    pub(crate) settings_parent: Screen,
    pub(crate) servers: Vec<super::menus::SavedServer>,
    pub(crate) adding_server: bool,
    pub(crate) selected_server: Option<usize>,
    pub(crate) editing_server: Option<usize>,
    pub(crate) deleting_server: bool,
    pub(crate) worlds: Vec<crate::singleplayer::World>,
    pub(crate) selected_world: Option<usize>,
    pub(crate) worlds_error: Option<String>,
    pub(crate) new_world_mode: crate::singleplayer::GameMode,
    pub(crate) new_world_difficulty: crate::singleplayer::Difficulty,
    pub(crate) local_status: String,
    /// Set while the connected world is served by the local singleplayer server.
    pub(crate) singleplayer_active: bool,
    pause_requested: bool,
    pub(crate) disconnect_title: Option<String>,
    pub(crate) options_tab: usize,
    pub(crate) options_footer: [f32; 4],
    pub(crate) slider_drag: Option<String>,
    pub(crate) dialogs: super::dialog::DialogHost,
    pub(crate) anim: super::anim::UiAnim,
}
impl UiState {
    pub fn new(content: &Content) -> Result<Self> {
        Self::new_mode(content, true)
    }
    pub(crate) fn new_state(content: &Content) -> Result<Self> {
        Self::new_mode(content, false)
    }
    fn new_mode(content: &Content, presentation: bool) -> Result<Self> {
        Ok(Self {
            text: Arc::new(if presentation {
                TextRenderer::load(content)?
            } else {
                TextRenderer::load_state(content)?
            }),
            models: ItemModels::new(),
            entity_models: None,
            player_preview: None,
            screen: Screen::Game,
            revision: 1,
            width: 854.,
            height: 480.,
            scale: 1.,
            framebuffer_size: [854, 480],
            nodes: Vec::with_capacity(NODE_LIMIT),
            dirty: true,
            inventory_revision: 0,
            inventory_instance: 0,
            focus: None,
            fields: BTreeMap::new(),
            editor: None,
            loader: None,
            operator_permission: false,
            hud: HudState::default(),
            network_hud: super::hud::HudPackets::default(),
            hud_sidebar: None,
            hud_level: None,
            hud_waypoint_styles: BTreeMap::new(),
            loading: None,
            death: None,
            can_respawn: false,
            clipboard: VecDeque::new(),
            clipboard_fence: None,
            history: VecDeque::new(),
            history_index: None,
            scroll: 0,
            page: 0,
            recipe_open: false,
            recipe_filter: false,
            recipe_category: None,
            settings: BTreeMap::new(),
            rebind: None,
            tick: 0,
            pointer: [0.; 2],
            drag: None,
            drag_slots: Vec::new(),
            drag_carrying: false,
            drag_fence: None,
            release_pending: false,
            chat: VecDeque::new(),
            maps: BTreeMap::new(),
            bosses: BTreeMap::new(),
            title: None,
            subtitle: None,
            title_timing: [10, 70, 20],
            title_started: 0,
            tab_header: None,
            tab_footer: None,
            advancements: BTreeMap::new(),
            statistics: super::statistics::StatisticsState::default(),
            statistics_group: super::statistics::StatisticsGroup::default(),
            selected_advancement: None,
            creative_source: Arc::new(
                serde_json::from_slice(content.source_report("creative_tabs")?)
                    .map_err(|e| e.to_string())?,
            ),
            enabled_features: Arc::new(BTreeSet::new()),
            creative_tab: 0,
            creative_stacks: BTreeMap::new(),
            saved_hotbars: std::array::from_fn(|_| None),
            book_components: Vec::new(),
            pending_link: None,
            recipe_stacks: BTreeMap::new(),
            recipe_names: BTreeMap::new(),
            ghost_presentation: None,
            item_context: ItemContext::default(),
            signature_cache: super::chat_tracking::SignatureCache::default(),
            packet_commands: VecDeque::new(),
            chat_receipts: VecDeque::new(),
            last_seen: super::chat_tracking::LastSeenTracker::default(),
            command_completion: super::commands::CommandState::default(),
            chat_expected_index: 0,
            chat_sequence: 0,
            beacon_selection: [None, None],
            beacon_source: [None, None],
            bundle_scroll: [0.; 2],
            bundle_hover: None,
            last_slot_click: None,
            collect_on_release: false,
            selected_slot: 0,
            selected_name_tick: u64::MAX,
            debug_lines: Vec::new(),
            title_error: None,
            last_server: None,
            last_username: None,
            settings_parent: Screen::Pause,
            servers: Vec::new(),
            adding_server: false,
            selected_server: None,
            editing_server: None,
            deleting_server: false,
            worlds: Vec::new(),
            selected_world: None,
            worlds_error: None,
            new_world_mode: crate::singleplayer::GameMode::Survival,
            new_world_difficulty: crate::singleplayer::Difficulty::Normal,
            local_status: String::new(),
            singleplayer_active: false,
            pause_requested: false,
            disconnect_title: None,
            options_tab: 0,
            options_footer: [0.; 4],
            slider_drag: None,
            dialogs: Default::default(),
            anim: super::anim::UiAnim::default(),
        })
    }
    pub fn set_item_context(&mut self, context: ItemContext) {
        self.item_context = context;
    }
    /// Defaults, ranges, and steps for the client-local Options persisted to
    /// `<exe-dir>/client-settings.json` (see `crate::local_config`).
    pub const DEFAULT_RENDER_DISTANCE: u32 = 8;
    pub const MIN_RENDER_DISTANCE: u32 = 2;
    pub const MAX_RENDER_DISTANCE: u32 = 32;
    pub const DEFAULT_FOV_DEG: f32 = 70.;
    pub const MIN_FOV_DEG: f32 = 30.;
    pub const MAX_FOV_DEG: f32 = 110.;
    pub const FOV_STEP_DEG: f32 = 5.;
    pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 1.;
    pub const MIN_MOUSE_SENSITIVITY: f32 = 0.1;
    pub const MAX_MOUSE_SENSITIVITY: f32 = 3.;
    pub const MOUSE_SENSITIVITY_STEP: f32 = 0.25;
    pub const DEFAULT_DAMAGE_TILT_STRENGTH: f64 = 1.;
    pub const DEFAULT_VOLUME: f32 = 1.;
    pub const MAX_VOLUME: f32 = 1.;
    pub const VOLUME_STEP: f32 = 0.1;
    pub const DEFAULT_UI_SCALE: f64 = 0.;
    /// Settings-screen keys persisted client-locally. Anything else in
    /// `UiState::settings` stays session-only.
    pub const MANAGED_SETTING_KEYS: &[&str] = &[
        "render_distance",
        "fov",
        "mouse_sensitivity",
        "damage_tilt_strength",
        "volume",
        "ui_scale",
        "fullscreen",
        "vsync",
        "max_fps",
        "graphics_preset",
        "key_bindings",
    ];
    fn setting_f64(value: &Value) -> Option<f64> {
        value.as_f64().or_else(|| value.as_u64().map(|v| v as f64))
    }
    /// Clamp `value` into the valid range for a managed key. Returns `None` for
    /// unknown keys or non-numeric values where numbers are required.
    pub fn sanitize_setting_value(key: &str, value: &Value) -> Option<Value> {
        match key {
            "key_bindings" => crate::controls::Bindings::from_value(value).map(|bindings| bindings.value()),
            "fullscreen" | "vsync" => value.as_bool().map(|v| json!(v)),
            "graphics_preset" => {
                GraphicsPreset::from_setting(value).map(|preset| json!(preset.as_str()))
            }
            "max_fps" => value
                .as_u64()
                .map(|v| json!(if v == 0 { 0 } else { v.clamp(30, 260) })),
            "render_distance" => {
                let n = if let Some(n) = value.as_u64() {
                    n
                } else if let Some(f) = Self::setting_f64(value) {
                    if !f.is_finite() {
                        return None;
                    }
                    f.round().clamp(
                        Self::MIN_RENDER_DISTANCE as f64,
                        Self::MAX_RENDER_DISTANCE as f64,
                    ) as u64
                } else {
                    return None;
                };
                Some(json!(n.clamp(
                    Self::MIN_RENDER_DISTANCE as u64,
                    Self::MAX_RENDER_DISTANCE as u64
                )))
            }
            "fov" => {
                let f = Self::setting_f64(value)?;
                if !f.is_finite() {
                    return None;
                }
                Some(json!(
                    f.clamp(Self::MIN_FOV_DEG as f64, Self::MAX_FOV_DEG as f64)
                ))
            }
            "mouse_sensitivity" => {
                let f = Self::setting_f64(value)?;
                if !f.is_finite() {
                    return None;
                }
                Some(json!(f.clamp(
                    Self::MIN_MOUSE_SENSITIVITY as f64,
                    Self::MAX_MOUSE_SENSITIVITY as f64
                )))
            }
            "volume" | "damage_tilt_strength" => {
                let f = Self::setting_f64(value)?;
                if !f.is_finite() {
                    return None;
                }
                Some(json!(f.clamp(0., Self::MAX_VOLUME as f64)))
            }
            "ui_scale" => {
                let f = Self::setting_f64(value)?;
                if f.is_finite() && (0.0..=4.0).contains(&f) && f.fract() == 0. {
                    Some(json!(f))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    /// Default value for a managed key, if any.
    pub fn default_setting_value(key: &str) -> Option<Value> {
        match key {
            "key_bindings" => Some(crate::controls::Bindings::default().value()),
            "fullscreen" => Some(json!(false)),
            "vsync" => Some(json!(true)),
            "graphics_preset" => Some(json!(GraphicsPreset::default().as_str())),
            "max_fps" => Some(json!(120)),
            "render_distance" => Some(json!(Self::DEFAULT_RENDER_DISTANCE)),
            "fov" => Some(json!(Self::DEFAULT_FOV_DEG)),
            "mouse_sensitivity" => Some(json!(Self::DEFAULT_MOUSE_SENSITIVITY)),
            "damage_tilt_strength" => Some(json!(Self::DEFAULT_DAMAGE_TILT_STRENGTH)),
            "volume" => Some(json!(Self::DEFAULT_VOLUME)),
            "ui_scale" => Some(json!(Self::DEFAULT_UI_SCALE)),
            _ => None,
        }
    }
    /// Next value for a managed Options control; numeric unset entries start
    /// with their default. Unmanaged toggles use the generic toggle path.
    pub fn next_setting_value(key: &str, current: Option<&Value>) -> Option<Value> {
        let sane = |v: &Value| Self::sanitize_setting_value(key, v);
        match key {
            "fullscreen" | "vsync" => Some(json!(!current
                .and_then(Value::as_bool)
                .unwrap_or(key != "fullscreen"))),
            "graphics_preset" => {
                let current = current
                    .and_then(GraphicsPreset::from_setting)
                    .unwrap_or_default();
                Some(json!(current.next().as_str()))
            }
            "max_fps" => {
                let current = current.and_then(Value::as_u64).unwrap_or(120);
                Some(json!([30, 60, 90, 120, 144, 165, 240, 260, 0]
                    .into_iter()
                    .find(|&v| v > current)
                    .filter(|_| current != 0)
                    .unwrap_or(if current == 0 { 30 } else { 0 })))
            }
            "render_distance" => {
                let Some(cur) = current.and_then(sane).and_then(|v| v.as_u64()) else {
                    return Some(json!(Self::DEFAULT_RENDER_DISTANCE));
                };
                let cur = cur as u32;
                Some(json!(if cur >= Self::MAX_RENDER_DISTANCE {
                    Self::MIN_RENDER_DISTANCE
                } else {
                    cur + 1
                }))
            }
            "fov" => {
                let Some(cur) = current.and_then(sane).and_then(|v| Self::setting_f64(&v)) else {
                    return Some(json!(Self::DEFAULT_FOV_DEG));
                };
                Some(json!(if cur >= Self::MAX_FOV_DEG as f64 - 1e-6 {
                    30.
                } else {
                    (cur + Self::FOV_STEP_DEG as f64).min(Self::MAX_FOV_DEG as f64)
                }))
            }
            "mouse_sensitivity" => {
                let Some(cur) = current.and_then(sane).and_then(|v| Self::setting_f64(&v)) else {
                    return Some(json!(Self::DEFAULT_MOUSE_SENSITIVITY));
                };
                let stepped = ((cur + Self::MOUSE_SENSITIVITY_STEP as f64) * 100.).round() / 100.;
                Some(json!(if cur >= Self::MAX_MOUSE_SENSITIVITY as f64 - 1e-6 {
                    // Emit the literal decimal (0.1), not the f32 bit pattern, so
                    // persisted JSON round-trips exactly.
                    0.1
                } else {
                    stepped.min(Self::MAX_MOUSE_SENSITIVITY as f64)
                }))
            }
            "volume" | "damage_tilt_strength" => {
                let Some(cur) = current.and_then(sane).and_then(|v| Self::setting_f64(&v)) else {
                    return Some(json!(Self::DEFAULT_VOLUME));
                };
                let stepped = ((cur + Self::VOLUME_STEP as f64) * 10.).round() / 10.;
                Some(json!(if cur >= Self::MAX_VOLUME as f64 - 1e-9 {
                    0.
                } else {
                    stepped.min(Self::MAX_VOLUME as f64)
                }))
            }
            _ => None,
        }
    }
    /// Apply a loaded/pushed managed value (session load path, tests). Sanitizes
    /// into range; unknown keys and bad values are rejected with an error.
    pub fn apply_setting(&mut self, key: &str, value: Value) -> Result<()> {
        let Some(clean) = Self::sanitize_setting_value(key, &value) else {
            return Err(format!("invalid client setting {key}"));
        };
        self.settings.insert(key.into(), clean);
        if key == "ui_scale" {
            let [width, height] = self.framebuffer_size;
            self.resize(width, height, 1.);
        }
        self.invalidate();
        Ok(())
    }
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }
    /// Options.getBackgroundOpacity: chat-only backgrounds retain the caller's default.
    pub fn text_background_opacity(&self, default: f32) -> f32 {
        if self
            .settings
            .get("text_background")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            default
        } else {
            self.settings
                .get("text_background_opacity")
                .and_then(Value::as_f64)
                .unwrap_or(0.5) as f32
        }
    }
    pub fn set_enabled_features(&mut self, features: Arc<BTreeSet<String>>) {
        if self.enabled_features != features {
            self.enabled_features = features;
            self.creative_stacks.clear();
            self.recipe_stacks.clear();
            self.recipe_names.clear();
            self.ghost_presentation = None;
            self.invalidate();
        }
    }
    pub fn text_renderer(&self) -> Arc<TextRenderer> {
        Arc::clone(&self.text)
    }
    pub fn set_entity_models(&mut self, models: Arc<crate::entities::Models>) {
        self.models.set_entity_models(models.clone());
        self.entity_models = Some(models);
        self.player_preview = None;
        self.invalidate();
    }
    pub(crate) fn set_player_preview(
        &mut self,
        content: &Content,
        uuid: Option<&str>,
        textures: Option<crate::entities::ProfileTextures>,
    ) -> Result<()> {
        let Some(models) = &self.entity_models else {
            return Ok(());
        };
        let default = models.default_skin(uuid)?;
        let texture = if let Some(texture) = textures.and_then(|textures| textures.skin) {
            texture
        } else {
            let path = default
                .texture
                .strip_prefix("minecraft:textures/")
                .unwrap_or(&default.texture)
                .trim_end_matches(".png");
            content
                .texture(path)
                .ok_or("player preview skin not activated")?
        };
        let model = textures
            .and_then(|textures| textures.model)
            .unwrap_or(default.model);
        if self.player_preview.as_ref().is_some_and(|preview| {
            preview.model == model
                && preview.texture.layer == texture.layer
                && preview.texture.uv == texture.uv
        }) {
            return Ok(());
        }
        let rig_name = match model {
            crate::entities::PlayerModel::Slim => "minecraft:player_slim#main",
            crate::entities::PlayerModel::Wide => "minecraft:player#main",
        };
        let rig = models.rig(rig_name);
        let mut matrices = Vec::with_capacity(rig.bones.len());
        let mut quads = Vec::new();
        for bone in &rig.bones {
            let matrix = bone
                .parent
                .map(|parent| matrices[parent])
                .unwrap_or(Mat4::IDENTITY)
                * Mat4::from_translation(bone.pivot)
                * Mat4::from_euler(
                    glam::EulerRot::ZYX,
                    bone.rotation.z,
                    bone.rotation.y,
                    bone.rotation.x,
                )
                * Mat4::from_scale(bone.scale);
            matrices.push(matrix);
            for quad in &bone.quads {
                quads.push(quad.map(|corner| {
                    Vertex {
                        position: matrix
                            .transform_point3(Vec3::new(corner[0], corner[1], corner[2]))
                            .to_array(),
                        color: [1.; 3],
                        uv: [
                            texture.uv[0] + corner[3] * (texture.uv[2] - texture.uv[0]),
                            texture.uv[1] + corner[4] * (texture.uv[3] - texture.uv[1]),
                        ],
                        overlay_uv: [-1.; 2],
                        overlay_tint: [1.; 3],
                        alpha: 1.,
                        texture_layer: texture.layer,
                        light: [-1.; 2],
                        normal: [0., 0., 1.],
                    }
                }));
            }
        }
        let order = (0..quads.len()).map(|index| (index, 0.)).collect();
        self.player_preview = Some(PlayerPreview {
            texture,
            model,
            quads,
            order,
        });
        self.invalidate();
        Ok(())
    }
    pub fn context(&self, inventory: &InventoryStore) -> UiContext {
        let mut c = inventory.context();
        c.ui_revision = self.revision;
        c
    }
    pub fn screen(&self) -> Screen {
        self.screen
    }
    pub(crate) fn statistics(&self) -> &super::statistics::StatisticsState {
        &self.statistics
    }
    /// Windowed-proof seam: visible semantic nodes (id/role/label/enabled)
    /// for automation assertions on overlay content and button labels.
    pub fn observed_nodes(&self) -> Vec<Value> {
        self.nodes
            .iter()
            .filter(|n| n.visible)
            .map(|n| {
                serde_json::json!({"id": n.id, "role": n.role, "label": n.label,
                    "bounds": n.bounds.map(|coordinate| coordinate * self.scale as f32),
                    "enabled": n.enabled, "focused": n.focused})
            })
            .collect()
    }
    /// Windowed-proof seam: current managed settings (Options rows) for
    /// applied-change assertions.
    pub fn observed_settings(&self) -> BTreeMap<String, Value> {
        self.settings.clone()
    }
    /// Dialog and loading overlays own keyboard/mouse input while open.
    /// Death/loading refuse Escape-close; other dialogs accept it. Used by Session input
    /// routing so native events never leak into gameplay while the world is not ready.
    pub fn dialog_screen_active(&self) -> bool {
        matches!(
            self.screen,
            Screen::Loading
                | Screen::Death
                | Screen::Pause
                | Screen::Settings
                | Screen::Accessibility
                | Screen::Advancements
                | Screen::Statistics
                | Screen::Title
                | Screen::Multiplayer
                | Screen::DirectConnect
                | Screen::Disconnected
                | Screen::Singleplayer
                | Screen::CreateWorld
                | Screen::DeleteWorld
                | Screen::SingleplayerStarting
                | Screen::ServerDialog
        )
    }
    pub fn has_open_container(&self, inventory: &InventoryStore) -> bool {
        inventory.screen_open
    }
    pub(crate) fn invalidate(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("UI revision exhaustion");
        self.dirty = true;
        self.clipboard_fence = None;
    }
    pub fn resize(&mut self, width: u32, height: u32, scale: f64) {
        if width == 0 || height == 0 || !scale.is_finite() || scale <= 0. {
            return;
        }
        self.framebuffer_size = [width, height];
        let requested = self
            .settings
            .get("ui_scale")
            .and_then(Value::as_f64)
            .unwrap_or(Self::DEFAULT_UI_SCALE) as u32;
        let scale = Self::gui_scale(width, height, requested) as f64;
        self.width = (width as f64 / scale).ceil() as f32;
        self.height = (height as f64 / scale).ceil() as f32;
        self.scale = scale;
        self.invalidate();
    }
    /// Window.calculateScale from the pinned 26.1.2 bytecode: zero is Auto.
    fn gui_scale(width: u32, height: u32, requested: u32) -> u32 {
        let maximum = (width / 320).min(height / 240).max(1);
        if requested == 0 {
            maximum
        } else {
            requested.min(maximum).max(1)
        }
    }
    pub(crate) fn clear_item_models(&mut self) {
        self.models.clear();
        self.player_preview = None;
        self.entity_models = None;
        if let Some(view) = &mut self.loader {
            view.preview_meshes.clear();
        }
        self.invalidate();
    }
    pub fn reset_resources(&mut self, content: &Content) -> Result<()> {
        let text = TextRenderer::load(content)?;
        let labels = self.prepare_map_labels(&text)?;
        self.text = Arc::new(text);
        self.commit_map_labels(labels);
        self.models.clear();
        if let Some(view) = &mut self.loader {
            view.preview_meshes.clear();
        }
        self.hud_sidebar = None;
        self.hud_level = None;
        self.hud_waypoint_styles.clear();
        self.recipe_stacks.clear();
        self.recipe_names.clear();
        self.invalidate();
        self.creative_source = Arc::new(
            serde_json::from_slice(content.source_report("creative_tabs")?)
                .map_err(|e| e.to_string())?,
        );
        self.ghost_presentation = None;
        self.creative_stacks.clear();
        Ok(())
    }
    pub fn set_language(
        &mut self,
        content: &Content,
        language: &str,
        uniform: bool,
        japanese: bool,
    ) -> Result<()> {
        let text = TextRenderer::load_language(content, language, uniform, japanese)?;
        let labels = self.prepare_map_labels(&text)?;
        self.text = Arc::new(text);
        self.commit_map_labels(labels);
        self.hud_sidebar = None;
        self.hud_level = None;
        self.recipe_names.clear();
        self.invalidate();
        Ok(())
    }
    fn prepare_map_labels(
        &self,
        text: &TextRenderer,
    ) -> Result<Vec<(i32, Vec<Option<Arc<super::shaping::TextLayout>>>)>> {
        self.maps
            .iter()
            .map(|(&id, map)| {
                Ok((
                    id,
                    map.decorations
                        .iter()
                        .map(|decoration| {
                            decoration
                                .name
                                .as_ref()
                                .map(|name| text.layout(name, f32::MAX).map(Arc::new))
                                .transpose()
                        })
                        .collect::<Result<_>>()?,
                ))
            })
            .collect()
    }
    fn commit_map_labels(
        &mut self,
        labels: Vec<(i32, Vec<Option<Arc<super::shaping::TextLayout>>>)>,
    ) {
        for (id, labels) in labels {
            for (decoration, layout) in self
                .maps
                .get_mut(&id)
                .expect("prepared map is retained")
                .decorations
                .iter_mut()
                .zip(labels)
            {
                decoration.label_layout = layout;
            }
        }
    }
    pub fn release_all(&mut self) {
        self.drag = None;
        self.drag_slots.clear();
        self.drag_carrying = false;
        self.drag_fence = None;
        self.last_slot_click = None;
        self.collect_on_release = false;
        self.clipboard_fence = None;
        for f in self.fields.values_mut() {
            f.composition.clear();
            f.composition_cursor = None;
        }
        self.release_pending = true;
        self.invalidate();
    }
    pub fn set_focus_active(&mut self, active: bool) {
        if !active {
            self.focus = None;
            self.release_all();
        } else {
            self.invalidate();
        }
    }
    pub fn tick(&mut self, tick: u64, _partial: f32) {
        if self.tick != tick {
            self.tick = tick;
            self.text.set_tick(tick);
            if self.title.is_some() || !self.chat.is_empty() {
                self.dirty = true;
            }
            if self.selected_name_tick != u64::MAX
                && tick.saturating_sub(self.selected_name_tick) == 100
            {
                self.dirty = true;
            }
            self.dialog_tick();
        }
    }
    pub fn take_clipboard_request(&mut self) -> Option<ClipboardRequest> {
        self.clipboard.pop_front()
    }
    pub fn clipboard_insert(&mut self, text: &str) -> Result<()> {
        let (revision, id) = self
            .clipboard_fence
            .take()
            .ok_or("clipboard_request_expired")?;
        if revision != self.revision || self.focus.as_ref() != Some(&id) {
            return Err("clipboard_request_expired".into());
        }
        self.fields
            .get_mut(&id)
            .ok_or("text_focus_required")?
            .insert(text)?;
        self.invalidate();
        Ok(())
    }
    /// Focus routing must not depend on font availability or an IME caret layout.
    pub(crate) fn text_field_focused(&self) -> bool {
        self.focus.as_deref().is_some_and(|id| {
            self.fields.contains_key(id)
                && self.nodes.iter().any(|n| n.id == id && n.visible && n.enabled)
        })
    }
    pub fn ime_state(&self) -> ImeState {
        let Some(id) = &self.focus else {
            return ImeState::default();
        };
        let Some(field) = self.fields.get(id) else {
            return ImeState::default();
        };
        let Some(node) = self
            .nodes
            .iter()
            .find(|n| n.id == *id && n.visible && n.enabled)
        else {
            return ImeState::default();
        };
        let Ok(layout) = self.text.layout(
            &json!({"text":field.display()}),
            (node.bounds[2] - 8.).max(1.),
        ) else {
            return ImeState::default();
        };
        let [x, y] = layout.caret(
            field
                .display_cursor()
                .unwrap_or(field.anchor.min(field.cursor)),
        );
        ImeState {
            allowed: true,
            position: [
                (node.bounds[0] + 4. + x) as f64 * self.scale,
                (node.bounds[1] + 4. + y) as f64 * self.scale,
            ],
            size: [self.scale, 9. * self.scale],
        }
    }
    pub fn set_presentation(&mut self, p: UiPresentation) -> Result<()> {
        if p.hud.effects.len() > 256 || p.hud.players.len() > 4096 {
            return Err("UI presentation limit".into());
        }
        let changed = self.hud != p.hud
            || self.loading != p.loading
            || self.death != p.death
            || self.can_respawn != p.can_respawn
            || self.operator_permission != p.can_edit_operator_blocks;
        if self.hud.scoreboard_name != p.hud.scoreboard_name {
            self.hud_sidebar = None;
        }
        self.hud = p.hud;
        self.loading = p.loading;
        self.death = p.death;
        self.can_respawn = p.can_respawn;
        self.operator_permission = p.can_edit_operator_blocks;
        if let Some(editor) = p.editor {
            self.open_editor(editor, p.can_edit_operator_blocks)?;
        }
        if self.loading.is_some() {
            self.transition(Screen::Loading);
        } else if self.death.is_some() || self.can_respawn {
            self.transition(Screen::Death);
        } else if matches!(self.screen, Screen::Loading | Screen::Death) {
            self.transition(Screen::Game);
            self.dialog_after_blocking_screen()?;
        }
        if changed {
            self.invalidate();
        }
        Ok(())
    }
    /// F3 debug overlay content. Owned by App (lib.rs publish path), not the
    /// session presentation flow, so `set_presentation` stays untouched.
    pub fn set_debug_lines(&mut self, lines: Vec<String>) {
        if self.debug_lines != lines {
            self.debug_lines = lines;
            self.invalidate();
        }
    }
    pub fn open_editor(&mut self, editor: EditorState, operator_permission: bool) -> Result<()> {
        if matches!(
            editor,
            EditorState::Command(_) | EditorState::Structure(_) | EditorState::Jigsaw(_)
        ) && !operator_permission
        {
            return Err("permission_denied".into());
        }
        self.fields.clear();
        self.book_components.clear();
        self.page = 0;
        self.operator_permission = operator_permission;
        match &editor {
            EditorState::Sign { lines, .. } => {
                for (i, line) in lines.iter().enumerate() {
                    self.field(&format!("sign/line/{i}"), line.clone(), 384, false)?;
                }
                self.transition(Screen::Sign);
                self.focus = Some("sign/line/0".into());
            }
            EditorState::Book {
                pages,
                title,
                writable,
                ..
            } => {
                if pages.is_empty() || pages.len() > 100 {
                    return Err("book_page_limit".into());
                }
                for (i, page) in pages.iter().enumerate() {
                    self.field(
                        &format!("book/page/{i}"),
                        page.clone(),
                        if *writable { 1024 } else { TEXT_LIMIT },
                        true,
                    )?;
                }
                self.field("book/title", title.clone().unwrap_or_default(), 128, false)?;
                self.transition(Screen::Book);
                if *writable {
                    self.focus = Some("book/page/0".into());
                }
            }
            EditorState::Command(c) => {
                self.field("command/command", c.command.clone(), 32500, false)?;
                self.transition(Screen::Command);
                self.focus = Some("command/command".into());
            }
            EditorState::Structure(s) => {
                self.field("structure/name", s.name.clone(), 128, false)?;
                self.field("structure/data", s.data.clone(), 128, false)?;
                for (axis, values) in [("offset", s.offset), ("size", s.size)] {
                    for (i, n) in values.into_iter().enumerate() {
                        self.field(&format!("structure/{axis}/{i}"), n.to_string(), 12, false)?;
                    }
                }
                self.field("structure/integrity", s.integrity.to_string(), 16, false)?;
                self.field("structure/seed", s.seed.to_string(), 24, false)?;
                self.transition(Screen::Structure);
                self.focus = Some("structure/name".into());
            }
            EditorState::Jigsaw(j) => {
                for (k, v) in [
                    ("name", &j.name),
                    ("target", &j.target),
                    ("pool", &j.pool),
                    ("final_state", &j.final_state),
                ] {
                    self.field(&format!("jigsaw/{k}"), v.clone(), 1024, false)?;
                }
                self.field(
                    "jigsaw/selection_priority",
                    j.selection_priority.to_string(),
                    12,
                    false,
                )?;
                self.field(
                    "jigsaw/placement_priority",
                    j.placement_priority.to_string(),
                    12,
                    false,
                )?;
                self.field("jigsaw/levels", "0".into(), 2, false)?;
                self.transition(Screen::Jigsaw);
                self.focus = Some("jigsaw/name".into());
            }
        }
        self.editor = Some(editor);
        self.invalidate();
        Ok(())
    }
    pub(crate) fn field(
        &mut self,
        id: &str,
        text: String,
        limit: usize,
        multiline: bool,
    ) -> Result<()> {
        self.fields
            .insert(id.into(), EditBuffer::new(text, limit, multiline)?);
        Ok(())
    }
    pub(crate) fn transition(&mut self, screen: Screen) {
        if self.screen != screen {
            let old = self.screen;
            self.anim_screen_changed(old);
            if !self.text.can_render()
                && matches!(
                    self.screen,
                    Screen::Pause | Screen::Statistics | Screen::Advancements
                )
            {
                self.nodes.clear();
            }
            self.screen = screen;
            self.focus = None;
            self.scroll = 0;
            self.slider_drag = None;
            self.release_all();
        }
    }
    /// Enter the title shell and prefill its Direct Connect buffers from
    /// explicit/last-used values, else loopback/Player. The title itself has
    /// no text fields; Multiplayer owns server selection and address editing.
    pub(crate) fn enter_title(
        &mut self,
        server: Option<&str>,
        username: Option<&str>,
    ) -> Result<()> {
        let server = server
            .map(str::to_owned)
            .or_else(|| self.last_server.clone())
            .unwrap_or_else(|| "127.0.0.1:25565".into());
        let username = username
            .map(str::to_owned)
            .or_else(|| self.last_username.clone())
            .unwrap_or_else(|| "Player".into());
        self.last_server = Some(server.clone());
        self.last_username = Some(username.clone());
        self.field("title/server", server, 253, false)?;
        self.field("title/username", username, 16, false)?;
        self.transition(Screen::Title);
        self.servers = super::menus::load_servers();
        self.invalidate();
        Ok(())
    }
    pub(crate) fn set_title_error(&mut self, error: String) {
        self.title_error = Some(error);
        self.invalidate();
    }
    /// Windowed-proof seam: Title field text + error line for observation.
    pub fn title_field(&self, id: &str) -> Option<String> {
        self.fields.get(id).map(|f| f.text.clone())
    }
    pub fn title_error_text(&self) -> Option<String> {
        self.title_error.clone()
    }
    pub(crate) fn title_defaults(&self) -> (String, String) {
        (
            self.last_server
                .clone()
                .unwrap_or_else(|| "127.0.0.1:25565".into()),
            self.last_username
                .clone()
                .unwrap_or_else(|| "Player".into()),
        )
    }
    pub(crate) fn synchronize(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<()> {
        if self.inventory_instance != inventory.screen_instance {
            self.inventory_instance = inventory.screen_instance;
            self.release_all();
            self.beacon_source = [None, None];
            self.beacon_selection = [None, None];
            self.pending_link = None;
            if let Some((pos, front)) = inventory.sign_editor {
                if !matches!(self.editor,Some(EditorState::Sign{pos:p,front:f,..}) if p==pos && f==front)
                {
                    return Err("sign_text_not_observed".into());
                }
            } else if inventory.screen_open {
                self.editor = None;
                self.transition(if inventory.game_mode == 1 && inventory.active.is_none() {
                    Screen::Creative
                } else {
                    Screen::Menu
                });
            } else if matches!(self.screen, Screen::Menu | Screen::Creative | Screen::Sign) {
                self.editor = None;
                self.transition(Screen::Game);
            }
        }
        if self.selected_slot != inventory.selected_slot {
            let old = self.selected_slot;
            self.selected_slot = inventory.selected_slot;
            self.anim_hotbar_changed(old);
            self.selected_name_tick = self.tick;
            self.invalidate();
        }
        if self.inventory_revision != inventory.revision {
            self.inventory_revision = inventory.revision;
            self.recipe_stacks
                .retain(|id, _| inventory.recipes.contains_key(id));
            self.recipe_names
                .retain(|id, _| inventory.recipes.contains_key(id));
            self.invalidate();
            if inventory.menu().kind == MenuKind::Beacon {
                let source = [
                    inventory.menu().data.get(&1).copied(),
                    inventory.menu().data.get(&2).copied(),
                ];
                if self.beacon_source != source {
                    let count = content.registry_entries("minecraft:mob_effect")?.len();
                    self.beacon_source = source;
                    self.beacon_selection = source.map(|value| {
                        value
                            .filter(|id| *id > 0 && *id as usize <= count)
                            .map(|id| id - 1)
                    });
                }
            }
        }
        if self.dirty
            && (self.text.can_render()
                || matches!(
                    self.screen,
                    Screen::Pause | Screen::Statistics | Screen::Advancements
                ))
        {
            self.rebuild(inventory, content)?;
            self.dirty = false;
        }
        Ok(())
    }
    /// Test-only direct rebuild: the windowed proof drivers rebuild with real
    /// fonts/GPU present; state-only unit tests (no fonts, `can_render() ==
    /// false`) drive the same path explicitly after `set_presentation`.
    #[cfg(test)]
    pub(crate) fn rebuild_for_test(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<()> {
        self.rebuild(inventory, content)?;
        self.dirty = false;
        Ok(())
    }
    fn validate(&self, c: &UiContext, inventory: &InventoryStore) -> Result<()> {
        inventory.validate(c)?;
        if c.ui_revision != self.revision {
            return Err("stale_ui_revision".into());
        }
        Ok(())
    }
    pub fn semantic_tree(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<Value> {
        self.synchronize(inventory, content)?;
        Ok(
            json!({"context":self.context(inventory),"screen":self.screen,"loader_view":self.loader_observation(),"nodes":self.nodes,"truncated":false}),
        )
    }
    fn target(&self, id: &str) -> Result<&SemanticNode> {
        let n = self
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or("unknown_ui_target")?;
        if !n.visible {
            return Err("hidden_ui_target".into());
        }
        if !n.enabled {
            return Err(n.deny_reason.unwrap_or("disabled_ui_target").into());
        }
        Ok(n)
    }
    pub fn execute(
        &mut self,
        intent: UiIntent,
        inventory: &mut InventoryStore,
        content: &Content,
    ) -> Result<UiEffects> {
        self.synchronize(inventory, content)?;
        let mut effects = UiEffects::default();
        effects.release_input = std::mem::take(&mut self.release_pending);
        if self.pending_link.is_some() {
            match &intent {
                UiIntent::Activate { element_id, .. }
                    if matches!(
                        element_id.as_str(),
                        "link/accept" | "link/copy" | "link/cancel"
                    ) => {}
                UiIntent::Pointer { .. } => {}
                UiIntent::Key { key, .. } if matches!(key.as_str(), "Escape" | "Tab" | "Enter") => {
                }
                _ => return Err("link_confirmation_required".into()),
            }
        }
        self.execute_inner(intent, inventory, content, &mut effects)?;
        self.synchronize(inventory, content)?;
        effects.release_input |= std::mem::take(&mut self.release_pending);
        if self.singleplayer_active {
            let pause = if !self.pause_requested && self.screen == Screen::Pause {
                Some(true)
            } else if self.pause_requested && self.screen == Screen::Game {
                Some(false)
            } else {
                None
            };
            if let Some(paused) = pause {
                self.pause_requested = paused;
                effects.commands.push(UiCommand::Chat {
                    message: if paused {
                        "solaris_pause on"
                    } else {
                        "solaris_pause off"
                    }
                    .into(),
                    command: true,
                });
            }
        }
        Ok(effects)
    }
    pub(super) fn execute_inner(
        &mut self,
        intent: UiIntent,
        inventory: &mut InventoryStore,
        content: &Content,
        effects: &mut UiEffects,
    ) -> Result<()> {
        match intent {
            UiIntent::Activate {
                context,
                element_id,
            } => {
                self.validate(&context, inventory)?;
                let action = self.target(&element_id)?.action.clone();
                self.activate(action, Modifiers::default(), inventory, content, effects)?;
            }
            UiIntent::Focus {
                context,
                element_id,
            } => {
                self.validate(&context, inventory)?;
                self.target(&element_id)?;
                if matches!(self.target(&element_id)?.action, NodeAction::None) {
                    return Err("target_not_focusable".into());
                }
                self.focus = Some(element_id);
                self.narrate_focus(effects)?;
                self.invalidate();
            }
            UiIntent::ScrollTarget {
                context,
                element_id,
                delta_x,
                delta_y,
            } => {
                self.validate(&context, inventory)?;
                let action = self.target(&element_id)?.action.clone();
                self.scroll_target(action, delta_x, delta_y, inventory, content, effects)?;
            }
            UiIntent::OpenInventory => {
                if self.screen != Screen::Game {
                    return Err("screen_blocks_inventory".into());
                }
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::OpenInventory));
            }
            UiIntent::OpenPause => {
                if self.screen != Screen::Game {
                    return Err("screen_blocks_pause".into());
                }
                self.transition(Screen::Pause);
            }
            UiIntent::Close { context } => {
                self.validate(&context, inventory)?;
                if self.screen == Screen::Title {
                    return Ok(());
                }
                if self.screen == Screen::SingleplayerStarting {
                    effects.commands.push(UiCommand::CancelLocalStart);
                    return Ok(());
                }
                if self.close_menu() {
                    return Ok(());
                }
                self.update_bundle_hover(None, inventory, effects);
                if self.screen == Screen::Advancements {
                    effects
                        .commands
                        .push(UiCommand::SeenAdvancements { tab: None });
                    self.transition(Screen::Pause);
                    return Ok(());
                }
                if self.screen == Screen::Statistics {
                    self.transition(Screen::Pause);
                    return Ok(());
                }
                if self.screen == Screen::Loader {
                    // Native modal close is local UI only. Session cleanup keeps the live Loader view.
                    self.transition(Screen::Game);
                } else if matches!(self.editor, Some(EditorState::Sign { .. })) {
                    self.submit_editor(effects)?;
                } else {
                    if inventory.screen_open {
                        effects
                            .commands
                            .push(UiCommand::Inventory(UiIntent::Close { context }));
                    }
                    self.editor = None;
                    self.transition(Screen::Game);
                }
            }
            UiIntent::ClickSlot {
                context,
                slot,
                button,
                mode,
            } => {
                self.validate(&context, inventory)?;
                let closed_player_menu = self.screen == Screen::Game
                    && inventory.menu().id == 0
                    && matches!(mode, ClickMode::Swap | ClickMode::QuickMove);
                if !matches!(self.screen, Screen::Menu | Screen::Creative) && !closed_player_menu {
                    return Err("container_not_visible".into());
                }
                if slot != -999 {
                    if closed_player_menu {
                        inventory
                            .menu()
                            .slots
                            .get(usize::try_from(slot).map_err(|_| "negative slot")?)
                            .ok_or("container slot bound")?;
                    } else {
                        self.target(&format!("slot/{slot}"))?;
                    }
                }
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::ClickSlot {
                        context,
                        slot,
                        button,
                        mode,
                    }));
            }
            UiIntent::MenuButton { context, button } => {
                self.validate(&context, inventory)?;
                self.target(&format!("menu/button/{button}"))?;
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::MenuButton {
                        context,
                        button,
                    }));
            }
            UiIntent::Beacon {
                context,
                primary,
                secondary,
            } => {
                self.validate(&context, inventory)?;
                if self.screen != Screen::Menu || inventory.menu().kind != MenuKind::Beacon {
                    return Err("beacon_not_visible".into());
                }
                self.target("beacon/confirm")?;
                for (kind, effect) in [(true, primary), (false, secondary)] {
                    if let Some(effect) = effect {
                        self.target(&format!(
                            "beacon/{}/{effect}",
                            if kind { "primary" } else { "secondary" }
                        ))?;
                    }
                }
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::Beacon {
                        context,
                        primary,
                        secondary,
                    }));
            }
            UiIntent::SlotToggle {
                context,
                slot,
                enabled,
            } => {
                self.validate(&context, inventory)?;
                self.target(&format!("crafter/toggle/{slot}"))?;
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::SlotToggle {
                        context,
                        slot,
                        enabled,
                    }));
            }
            UiIntent::Rename { context, text } => {
                self.validate(&context, inventory)?;
                if inventory.menu().kind != MenuKind::Anvil || self.screen != Screen::Menu {
                    return Err("anvil_not_visible".into());
                }
                let field = self
                    .fields
                    .get_mut("anvil/name")
                    .ok_or("anvil_not_visible")?;
                field.select(0, field.text.len())?;
                field.insert(&text)?;
                let hover_name = self.anvil_hover_name(inventory, content)?;
                effects.commands.push(UiCommand::InventoryRename {
                    context,
                    text,
                    hover_name,
                });
                self.invalidate();
            }
            UiIntent::PlaceRecipe {
                context,
                display_id,
                craft_all,
            } => {
                self.validate(&context, inventory)?;
                self.target(&format!("recipe/{display_id}"))?;
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::PlaceRecipe {
                        context,
                        display_id,
                        craft_all,
                    }));
            }
            UiIntent::RecipeBook {
                context,
                open,
                filter,
            } => {
                self.validate(&context, inventory)?;
                if !matches!(self.screen, Screen::Menu | Screen::Creative)
                    || inventory.menu().kind.recipe_book().is_none()
                {
                    return Err("recipe_book_unavailable".into());
                }
                self.recipe_open = open;
                self.recipe_filter = filter;
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::RecipeBook {
                        context,
                        open,
                        filter,
                    }));
                self.invalidate();
            }
            UiIntent::BundleSelect {
                context,
                slot,
                index,
            } => {
                self.validate(&context, inventory)?;
                if !matches!(self.screen, Screen::Menu | Screen::Creative) {
                    return Err("container_not_visible".into());
                }
                self.target(&format!("slot/{slot}"))?;
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::BundleSelect {
                        context,
                        slot,
                        index,
                    }));
            }
            UiIntent::CreativeSlot {
                context,
                slot,
                item_id,
                count,
            } => {
                self.validate(&context, inventory)?;
                if self.screen != Screen::Creative || inventory.game_mode != 1 {
                    return Err("creative_permission_required".into());
                }
                if !content.item_names.contains_key(&item_id) && count != 0 {
                    return Err("unknown_item".into());
                }
                effects
                    .commands
                    .push(UiCommand::Inventory(UiIntent::CreativeSlot {
                        context,
                        slot,
                        item_id,
                        count,
                    }));
            }
            UiIntent::OpenChat { command } => {
                if self.screen != Screen::Game {
                    return Err("screen_blocks_chat".into());
                }
                self.field(
                    "chat/input",
                    if command { "/".into() } else { String::new() },
                    256,
                    false,
                )?;
                self.transition(Screen::Chat);
                self.focus = Some("chat/input".into());
                self.invalidate();
            }
            UiIntent::SendChat { text } => {
                if self.screen != Screen::Chat {
                    return Err("chat_not_visible".into());
                }
                if text.len() > 1024
                    || text.chars().count() > 256
                    || text.chars().any(char::is_control)
                {
                    return Err("chat_limit".into());
                }
                let text = text.trim().to_owned();
                if text.is_empty() {
                    return Err("empty_chat".into());
                }
                let command = text.starts_with('/');
                effects.commands.push(UiCommand::Chat {
                    message: if command {
                        text[1..].into()
                    } else {
                        text.clone()
                    },
                    command,
                });
                if self.history.back() != Some(&text) {
                    if self.history.len() == 100 {
                        self.history.pop_front();
                    }
                    self.history.push_back(text);
                }
                self.history_index = None;
                self.transition(Screen::Game);
            }
            UiIntent::EditSignLine {
                context,
                line,
                text,
            } => {
                self.validate(&context, inventory)?;
                if self.screen != Screen::Sign || line >= 4 {
                    return Err("sign_not_visible".into());
                }
                let id = format!("sign/line/{line}");
                self.replace_field(&id, text)?;
            }
            UiIntent::SubmitSign { context } => {
                self.validate(&context, inventory)?;
                if self.screen != Screen::Sign {
                    return Err("sign_not_visible".into());
                }
                self.submit_editor(effects)?;
            }
            UiIntent::EditBookPage {
                context,
                page,
                text,
            } => {
                self.validate(&context, inventory)?;
                if !matches!(self.editor, Some(EditorState::Book { writable: true, .. }))
                    || self.screen != Screen::Book
                    || page >= 100
                {
                    return Err("book_not_editable".into());
                }
                let id = format!("book/page/{page}");
                if !self.fields.contains_key(&id) {
                    let count = self
                        .fields
                        .keys()
                        .filter(|k| k.starts_with("book/page/"))
                        .count();
                    if usize::from(page) != count {
                        return Err("book_page_gap".into());
                    }
                    self.field(&id, String::new(), 1024, true)?;
                }
                self.replace_field(&id, text)?;
            }
            UiIntent::SubmitBook { context, title } => {
                self.validate(&context, inventory)?;
                if self.screen != Screen::Book {
                    return Err("book_not_visible".into());
                }
                if let Some(title) = title {
                    self.replace_field("book/title", title)?;
                }
                self.submit_editor(effects)?;
            }
            UiIntent::Pointer {
                phase,
                x,
                y,
                button,
                modifiers,
            } => {
                if !x.is_finite() || !y.is_finite() {
                    return Err("invalid_pointer".into());
                }
                self.pointer = [x as f32 / self.scale as f32, y as f32 / self.scale as f32];
                if matches!(self.screen, Screen::Chat | Screen::Book | Screen::Creative) {
                    self.dirty = true;
                }
                let target = self
                    .nodes
                    .iter()
                    .rev()
                    .find(|n| {
                        n.visible
                            && !matches!(n.action, NodeAction::None)
                            && contains(n.bounds, self.pointer)
                    })
                    .map(|n| (n.id.clone(), n.action.clone(), n.enabled));
                let bundle = target.as_ref().and_then(|(_, action, enabled)| {
                    if let NodeAction::Slot(slot) = action {
                        if *enabled
                            && self
                                .bundle_stack(inventory, *slot)
                                .is_some_and(|stack| stack.bundle_visible_items() > 0)
                        {
                            return Some(*slot);
                        }
                    }
                    None
                });
                self.update_bundle_hover(bundle, inventory, effects);
                match phase {
                    PointerPhase::Down => {
                        let previous_click = self.last_slot_click.take();
                        self.collect_on_release = false;
                        self.drag = button;
                        self.drag_slots.clear();
                        self.drag_carrying = inventory.cursor.as_ref().is_some_and(|s| !s.empty());
                        self.drag_fence = Some(self.context(inventory));
                        if let Some((id, action, enabled)) = target {
                            if !enabled {
                                return Err("disabled_ui_target".into());
                            }
                            if let NodeAction::Focus(_) = action {
                                let pos = self.text_hit(&id, self.pointer)?;
                                let field =
                                    self.fields.get_mut(&id).ok_or("text_focus_required")?;
                                if modifiers.shift {
                                    field.cursor = pos;
                                } else {
                                    field.select(pos, pos)?;
                                }
                                self.focus = Some(id);
                                self.invalidate();
                            } else if let NodeAction::Slot(slot) = action {
                                self.drag_slots.push(slot);
                                let button = match button {
                                    Some(PointerButton::Right) => 1,
                                    Some(PointerButton::Middle) => 2,
                                    _ => 0,
                                };
                                let now = Instant::now();
                                self.collect_on_release = button == 0
                                    && !modifiers.shift
                                    && self.drag_carrying
                                    && previous_click.is_some_and(|(old_slot, old_button, time)| {
                                        old_slot == slot && old_button == button
                                            && now.duration_since(time) < Duration::from_millis(250)
                                    });
                                self.last_slot_click = Some((slot, button, now));
                                let mode = if modifiers.shift {
                                    ClickMode::QuickMove
                                } else if button == 2 {
                                    ClickMode::Clone
                                } else {
                                    ClickMode::Pickup
                                };
                                if !self.drag_carrying || modifiers.shift || button == 2 {
                                    self.execute_inner(
                                        UiIntent::ClickSlot {
                                            context: self.context(inventory),
                                            slot,
                                            button,
                                            mode,
                                        },
                                        inventory,
                                        content,
                                        effects,
                                    )?;
                                    self.drag_slots.clear();
                                }
                            } else if let NodeAction::Creative(key) = action {
                                let stack = self
                                    .creative_stacks
                                    .get(&key)
                                    .ok_or("creative_preset_not_retained")?;
                                effects.commands.push(UiCommand::CreativePick {
                                    context: self.context(inventory),
                                    stack: stack.clone(),
                                    button: if matches!(button, Some(PointerButton::Right)) {
                                        1
                                    } else {
                                        0
                                    },
                                    mode: if matches!(button, Some(PointerButton::Middle)) {
                                        ClickMode::Clone
                                    } else if modifiers.shift {
                                        ClickMode::QuickMove
                                    } else {
                                        ClickMode::Pickup
                                    },
                                });
                            } else if let NodeAction::Dialog(super::dialog::DialogControl::Slide(
                                _,
                            )) = action
                            {
                                self.focus = Some(id.clone());
                                self.slider_drag = Some(id.clone());
                                self.dialog_slider_pointer(&id);
                            } else if let NodeAction::Slider(key) = action {
                                if let Some(bounds) =
                                    self.nodes.iter().find(|n| n.id == id).map(|n| n.bounds)
                                {
                                    self.focus = Some(id.clone());
                                    self.slider_drag = Some(id);
                                    if let Some(pos) =
                                        super::options::pos_at(key, bounds, self.pointer[0])
                                    {
                                        self.slide_to(key, pos, effects);
                                    }
                                }
                            } else {
                                self.activate(action, modifiers, inventory, content, effects)?;
                            }
                        } else if self.pending_link.is_none()
                            && matches!(self.screen, Screen::Menu | Screen::Creative)
                        {
                            self.execute_inner(
                                UiIntent::ClickSlot {
                                    context: self.context(inventory),
                                    slot: -999,
                                    button: if matches!(button, Some(PointerButton::Right)) {
                                        1
                                    } else {
                                        0
                                    },
                                    mode: ClickMode::Pickup,
                                },
                                inventory,
                                content,
                                effects,
                            )?;
                        }
                    }
                    PointerPhase::Move => {
                        if let Some(id) = self.slider_drag.clone() {
                            if self.screen == Screen::ServerDialog {
                                self.dialog_slider_pointer(&id);
                            } else if let Some((key, bounds)) =
                                self.nodes.iter().find(|n| n.id == id).and_then(|n| {
                                    match n.action {
                                        NodeAction::Slider(key) => Some((key, n.bounds)),
                                        _ => None,
                                    }
                                })
                            {
                                if let Some(pos) =
                                    super::options::pos_at(key, bounds, self.pointer[0])
                                {
                                    self.slide_to(key, pos, effects);
                                }
                            }
                        } else if self.drag.is_some() {
                            if let Some(id) = self.focus.clone() {
                                if self.fields.contains_key(&id) {
                                    let pos = self.text_hit(&id, self.pointer)?;
                                    self.fields.get_mut(&id).unwrap().cursor = pos;
                                    self.invalidate();
                                }
                            }
                            if let Some((_, NodeAction::Slot(slot), true)) = target {
                                if !self.drag_slots.contains(&slot) {
                                    self.drag_slots.push(slot);
                                }
                            }
                        }
                    }
                    PointerPhase::Up => {
                        if !self.drag_slots.is_empty() && self.drag_carrying {
                            let fence = self.drag_fence.clone().ok_or("drag_context_missing")?;
                            self.validate(&fence, inventory)?;
                            let b = if matches!(self.drag, Some(PointerButton::Right)) {
                                1
                            } else {
                                0
                            };
                            if self.drag_slots.len() == 1 {
                                self.execute_inner(
                                    UiIntent::ClickSlot {
                                        context: fence,
                                        slot: self.drag_slots[0],
                                        button: b,
                                        mode: if self.collect_on_release
                                            && matches!(target, Some((_, NodeAction::Slot(slot), true)) if slot == self.drag_slots[0])
                                        {
                                            ClickMode::PickupAll
                                        } else {
                                            ClickMode::Pickup
                                        },
                                    },
                                    inventory,
                                    content,
                                    effects,
                                )?;
                            } else {
                                let mut gesture = Vec::with_capacity(self.drag_slots.len() + 2);
                                for (slot, button) in std::iter::once((-999, b * 4))
                                    .chain(self.drag_slots.iter().map(|s| (*s, b * 4 + 1)))
                                    .chain(std::iter::once((-999, b * 4 + 2)))
                                {
                                    self.target(&format!("slot/{slot}")).or_else(|error| {
                                        if slot == -999 {
                                            Ok(&self.nodes[0])
                                        } else {
                                            Err(error)
                                        }
                                    })?;
                                    gesture.push(UiIntent::ClickSlot {
                                        context: fence.clone(),
                                        slot,
                                        button,
                                        mode: ClickMode::QuickCraft,
                                    });
                                }
                                effects.commands.push(UiCommand::InventoryGesture(gesture));
                            }
                        }
                        self.slider_drag = None;
                        self.drag = None;
                        self.drag_slots.clear();
                        self.drag_fence = None;
                        self.collect_on_release = false;
                    }
                }
            }
            UiIntent::Scroll {
                x,
                y,
                delta_x,
                delta_y,
                ..
            } => {
                if !x.is_finite() || !y.is_finite() {
                    return Err("invalid_pointer".into());
                }
                let p = [x as f32 / self.scale as f32, y as f32 / self.scale as f32];
                let action = self
                    .nodes
                    .iter()
                    .rev()
                    .find(|node| node.visible && contains(node.bounds, p))
                    .ok_or("scroll_target_required")?
                    .action
                    .clone();
                self.scroll_target(action, delta_x, delta_y, inventory, content, effects)?;
            }
            UiIntent::Key {
                key,
                pressed,
                modifiers,
            } => {
                if pressed {
                    self.key(&key, modifiers, inventory, content, effects)?;
                }
            }
            UiIntent::Text { text } => {
                let id = self.focus.clone().ok_or("text_focus_required")?;
                self.target(&id)?;
                self.fields
                    .get_mut(&id)
                    .ok_or("text_focus_required")?
                    .insert(&text)?;
                self.on_field_change(&id, inventory, content, effects)?;
                self.invalidate();
            }
            UiIntent::Composition { text, cursor } => {
                let id = self.focus.clone().ok_or("text_focus_required")?;
                self.target(&id)?;
                self.fields
                    .get_mut(&id)
                    .ok_or("text_focus_required")?
                    .compose(text, cursor)?;
                self.invalidate();
            }
            UiIntent::SelectText { start, end } => {
                let id = self.focus.clone().ok_or("text_focus_required")?;
                self.target(&id)?;
                self.fields
                    .get_mut(&id)
                    .ok_or("text_focus_required")?
                    .select(start, end)?;
                self.invalidate();
            }
        }
        Ok(())
    }
    fn replace_field(&mut self, id: &str, text: String) -> Result<()> {
        let f = self.fields.get_mut(id).ok_or("unknown_text_field")?;
        f.select(0, f.text.len())?;
        f.insert(&text)?;
        self.invalidate();
        Ok(())
    }
    fn on_field_change(
        &self,
        id: &str,
        inventory: &InventoryStore,
        content: &Content,
        effects: &mut UiEffects,
    ) -> Result<()> {
        if id == "anvil/name" {
            effects.commands.push(UiCommand::InventoryRename {
                context: self.context(inventory),
                text: self.fields[id].text.clone(),
                hover_name: self.anvil_hover_name(inventory, content)?,
            });
        }
        Ok(())
    }
    fn anvil_hover_name(&self, inventory: &InventoryStore, content: &Content) -> Result<String> {
        inventory
            .menu()
            .slots
            .first()
            .and_then(Option::as_ref)
            .filter(|stack| !stack.empty())
            .map(|stack| {
                self.text
                    .language
                    .plain(super::text::item_name_component(content, stack)?.as_ref())
            })
            .transpose()
            .map(|name| name.unwrap_or_default())
    }
    fn bundle_stack<'a>(
        &self,
        inventory: &'a InventoryStore,
        slot: i16,
    ) -> Option<&'a crate::inventory::Stack> {
        inventory
            .menu()
            .slots
            .get(usize::try_from(slot).ok()?)?
            .as_ref()
            .filter(|stack| stack.components.contains_key("minecraft:bundle_contents"))
    }
    fn update_bundle_hover(
        &mut self,
        slot: Option<i16>,
        inventory: &InventoryStore,
        effects: &mut UiEffects,
    ) {
        if self.bundle_hover != slot {
            if let Some(old) = self.bundle_hover {
                if self
                    .bundle_stack(inventory, old)
                    .is_some_and(|stack| stack.bundle_selected_index() != -1)
                {
                    effects
                        .commands
                        .push(UiCommand::Inventory(UiIntent::BundleSelect {
                            context: self.context(inventory),
                            slot: old,
                            index: -1,
                        }));
                }
            }
            self.bundle_hover = slot;
            self.bundle_scroll = [0.; 2];
        }
    }
    fn scroll_target(
        &mut self,
        action: NodeAction,
        delta_x: f32,
        delta_y: f32,
        inventory: &mut InventoryStore,
        content: &Content,
        effects: &mut UiEffects,
    ) -> Result<()> {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return Err("invalid_scroll".into());
        }
        if let NodeAction::Slot(slot) = action {
            if let Some(stack) = self.bundle_stack(inventory, slot) {
                let visible = stack.bundle_visible_items();
                if visible > 0 {
                    let selected = stack.bundle_selected_index();
                    let mut integral = [0.; 2];
                    for (axis, delta) in [f64::from(delta_x), f64::from(delta_y)]
                        .into_iter()
                        .enumerate()
                    {
                        let sign = if delta == 0. { 0. } else { delta.signum() };
                        if self.bundle_scroll[axis] != 0.
                            && sign != self.bundle_scroll[axis].signum()
                        {
                            self.bundle_scroll[axis] = 0.;
                        }
                        self.bundle_scroll[axis] += delta;
                        integral[axis] = self.bundle_scroll[axis].trunc();
                    }
                    if integral == [0.; 2] {
                        return Ok(());
                    }
                    for axis in 0..2 {
                        self.bundle_scroll[axis] -= integral[axis];
                    }
                    let scroll = if integral[1] != 0. {
                        integral[1]
                    } else {
                        -integral[0]
                    };
                    let index = (selected - scroll.signum() as i32)
                        .max(-1)
                        .rem_euclid(visible as i32);
                    return self.execute_inner(
                        UiIntent::BundleSelect {
                            context: self.context(inventory),
                            slot,
                            index,
                        },
                        inventory,
                        content,
                        effects,
                    );
                }
            }
        }
        if let NodeAction::Slider(key) = action {
            // One stop per wheel notch; wheel up raises the value.
            let notches = delta_y.signum() as i32 * delta_y.abs().ceil().max(1.) as i32;
            if delta_y != 0. {
                self.slide_by(key, notches, effects);
            }
            return Ok(());
        }
        self.scroll_by(delta_y)
    }
    fn scroll_by(&mut self, delta: f32) -> Result<()> {
        if !delta.is_finite() {
            return Err("invalid_scroll".into());
        }
        let dialog_cap = (self.screen == Screen::ServerDialog).then_some(self.dialogs.max_scroll);
        self.scroll = if delta > 0. {
            self.scroll.saturating_sub(delta.ceil() as usize)
        } else {
            self.scroll.saturating_add((-delta).ceil() as usize).min(
                if self.screen == Screen::Advancements {
                    usize::MAX
                } else {
                    dialog_cap.unwrap_or(4096)
                },
            )
        };
        if self.screen == Screen::Advancements {
            let root = self.selected_advancement_root();
            let count = self
                .advancements
                .iter()
                .filter(|(id, a)| {
                    a.display.is_some() && root.is_some() && self.advancement_root_for(id) == root
                })
                .count();
            let (_, visible_rows) = self.advancement_list_viewport();
            self.scroll = self.scroll.min(count.saturating_sub(visible_rows));
        }
        self.invalidate();
        Ok(())
    }
    fn key(
        &mut self,
        key: &str,
        m: Modifiers,
        inventory: &mut InventoryStore,
        content: &Content,
        e: &mut UiEffects,
    ) -> Result<()> {
        if key == "F1" && self.screen == Screen::Game {
            let hidden = self.setting("hide_hud").and_then(Value::as_bool).unwrap_or(false);
            self.settings.insert("hide_hud".into(), json!(!hidden));
            self.invalidate();
            return Ok(());
        }
        if key == "Escape" {
            if self.pending_link.take().is_some() {
                self.invalidate();
                return Ok(());
            }
            if self.screen == Screen::SingleplayerStarting {
                e.commands.push(UiCommand::CancelLocalStart);
                return Ok(());
            }
            if self.screen == Screen::ServerDialog {
                return self.dialog_escape(e);
            }
            if self.screen == Screen::Game {
                self.transition(Screen::Pause);
            } else if self.screen == Screen::Title {
                // Title owns its input: Escape stays put (Quit button exits).
                return Ok(());
            } else if self.close_menu() {
                return Ok(());
            } else if self.screen == Screen::Death || self.screen == Screen::Loading {
                return Err("screen_cannot_close".into());
            } else {
                self.execute_inner(
                    UiIntent::Close {
                        context: self.context(inventory),
                    },
                    inventory,
                    content,
                    e,
                )?;
            }
            return Ok(());
        }
        if key == "Tab" {
            if self.screen == Screen::Chat && self.pending_link.is_none()
                && self.focus.as_deref() == Some("chat/input")
            {
                return self.complete_chat(m.shift, e);
            }
            let ids: Vec<_> = self
                .nodes
                .iter()
                .filter(|n| n.visible && n.enabled && !matches!(n.action, NodeAction::None))
                .map(|n| n.id.clone())
                .collect();
            if !ids.is_empty() {
                let old = self
                    .focus
                    .as_ref()
                    .and_then(|f| ids.iter().position(|i| i == f));
                let next = if m.shift {
                    old.unwrap_or(0).wrapping_add(ids.len() - 1) % ids.len()
                } else {
                    old.map_or(0, |i| (i + 1) % ids.len())
                };
                self.focus = Some(ids[next].clone());
                self.narrate_focus(e)?;
                self.invalidate();
            }
            return Ok(());
        }
        if key == "Enter" {
            if matches!(self.screen, Screen::CreateWorld)
                && self
                    .focus
                    .as_ref()
                    .is_some_and(|id| self.fields.contains_key(id))
            {
                return self.activate(NodeAction::CreateWorld, m, inventory, content, e);
            }
            if self.screen == Screen::DirectConnect
                && self
                    .focus
                    .as_ref()
                    .is_some_and(|id| self.fields.contains_key(id))
            {
                return self.activate(
                    if self.adding_server {
                        NodeAction::SaveServer
                    } else {
                        NodeAction::Connect
                    },
                    m,
                    inventory,
                    content,
                    e,
                );
            }
            if self.screen == Screen::Chat
                && self.pending_link.is_none()
                && self.focus.as_deref() == Some("chat/input")
            {
                let text = self.fields["chat/input"].text.clone();
                return self.execute_inner(UiIntent::SendChat { text }, inventory, content, e);
            }
            if let Some(id) = &self.focus {
                if !self.fields.get(id).is_some_and(|f| f.multiline) {
                    let a = self.target(id)?.action.clone();
                    if !matches!(a, NodeAction::Focus(_)) {
                        return self.activate(a, m, inventory, content, e);
                    }
                }
            }
        }
        if self.screen == Screen::ServerDialog && self.dialog_slider_key(key) {
            return Ok(());
        }
        if let Some(slider) = self.focus.as_deref().and_then(|id| self.slider_key(id)) {
            let step = if m.shift { 5 } else { 1 };
            match key {
                "ArrowLeft" | "ArrowDown" => self.slide_by(slider, -step, e),
                "ArrowRight" | "ArrowUp" => self.slide_by(slider, step, e),
                "Home" => self.slide_to(slider, 0, e),
                "End" => {
                    if let Some(n) = super::options::slider_positions(slider) {
                        self.slide_to(slider, n - 1, e);
                    }
                }
                _ => {}
            }
            if matches!(
                key,
                "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Home" | "End"
            ) {
                return Ok(());
            }
        }
        if let Some(id) = self.focus.clone() {
            if self.fields.contains_key(&id) {
                if m.control || m.logo {
                    match key {
                        "a" | "A" => {
                            let f = self.fields.get_mut(&id).unwrap();
                            f.select(0, f.text.len())?;
                        }
                        "c" | "C" | "x" | "X" => {
                            if self.clipboard.len() >= 4 {
                                return Err("clipboard_queue_full".into());
                            }
                            let f = self.fields.get_mut(&id).unwrap();
                            self.clipboard
                                .push_back(ClipboardRequest::Set(f.selection().into()));
                            if key.eq_ignore_ascii_case("x") {
                                f.insert("")?;
                            }
                        }
                        "v" | "V" => {
                            if self.clipboard.len() >= 4 {
                                return Err("clipboard_queue_full".into());
                            }
                            self.clipboard.push_back(ClipboardRequest::Get);
                            self.clipboard_fence = Some((self.revision, id));
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                let f = self.fields.get_mut(&id).unwrap();
                match key {
                    "ArrowLeft" => f.move_cursor(false, m.shift, m.control),
                    "ArrowRight" => f.move_cursor(true, m.shift, m.control),
                    "Home" => {
                        f.cursor = 0;
                        if !m.shift {
                            f.anchor = 0;
                        }
                    }
                    "End" => {
                        f.cursor = f.text.len();
                        if !m.shift {
                            f.anchor = f.cursor;
                        }
                    }
                    "Backspace" => f.delete(false, m.control)?,
                    "Delete" => f.delete(true, m.control)?,
                    "Enter" if f.multiline => f.insert("\n")?,
                    "ArrowUp" | "ArrowDown" if self.screen == Screen::Chat => {
                        if !self.history.is_empty() {
                            let i = if key == "ArrowUp" {
                                self.history_index
                                    .unwrap_or(self.history.len())
                                    .saturating_sub(1)
                            } else {
                                self.history_index
                                    .unwrap_or(self.history.len())
                                    .saturating_add(1)
                                    .min(self.history.len())
                            };
                            self.history_index = Some(i);
                            let value = self.history.get(i).cloned().unwrap_or_default();
                            f.select(0, f.text.len())?;
                            f.insert(&value)?;
                        }
                    }
                    _ => {}
                }
                self.on_field_change(&id, inventory, content, e)?;
                self.invalidate();
                return Ok(());
            }
        }
        if matches!(self.screen, Screen::Menu | Screen::Creative) {
            if let Some((id, action, enabled)) = self
                .nodes
                .iter()
                .rev()
                .find(|node| {
                    node.visible
                        && !matches!(node.action, NodeAction::None)
                        && contains(node.bounds, self.pointer)
                })
                .map(|node| (node.id.clone(), node.action.clone(), node.enabled))
            {
                if !enabled {
                    return Err("disabled_ui_target".into());
                }
                let (button, mode) = match key {
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                        (key.as_bytes()[0] - b'1', ClickMode::Swap)
                    }
                    "f" | "F" => (40, ClickMode::Swap),
                    "q" | "Q" => (u8::from(m.control), ClickMode::Throw),
                    _ => return Ok(()),
                };
                if mode == ClickMode::Swap
                    && inventory.cursor.as_ref().is_some_and(|stack| !stack.empty())
                {
                    return Ok(());
                }
                self.target(&id)?;
                match action {
                    NodeAction::Slot(slot) => {
                        return self.execute_inner(
                            UiIntent::ClickSlot {
                                context: self.context(inventory),
                                slot,
                                button,
                                mode,
                            },
                            inventory,
                            content,
                            e,
                        );
                    }
                    NodeAction::Creative(key) => {
                        if button == 40 {
                            return Err("creative_palette_offhand_unavailable".into());
                        }
                        let stack = self
                            .creative_stacks
                            .get(&key)
                            .ok_or("creative_preset_not_retained")?;
                        e.commands.push(UiCommand::CreativePick {
                            context: self.context(inventory),
                            stack: stack.clone(),
                            button,
                            mode,
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
    fn activate(
        &mut self,
        a: NodeAction,
        m: Modifiers,
        inventory: &mut InventoryStore,
        content: &Content,
        e: &mut UiEffects,
    ) -> Result<()> {
        match a {
            NodeAction::None => return Err("target_not_interactive".into()),
            NodeAction::Loader(control) => self.activate_loader(control, e)?,
            NodeAction::Dialog(control) => self.activate_dialog(control, e)?,
            NodeAction::Focus(id) => {
                self.focus = Some(id);
                self.invalidate();
            }
            NodeAction::Slot(slot) => self.execute_inner(
                UiIntent::ClickSlot {
                    context: self.context(inventory),
                    slot,
                    button: 0,
                    mode: if m.shift {
                        ClickMode::QuickMove
                    } else {
                        ClickMode::Pickup
                    },
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::MenuButton(button) => self.execute_inner(
                UiIntent::MenuButton {
                    context: self.context(inventory),
                    button,
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::Close => self.execute_inner(
                UiIntent::Close {
                    context: self.context(inventory),
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::Send => self.execute_inner(
                UiIntent::SendChat {
                    text: self.fields["chat/input"].text.clone(),
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::Respawn => {
                // Fail-closed: death screen must never emit a no-op respawn. The
                // dead-player flag (health<=0, surfaced as can_respawn) plus the
                // Death screen must agree; the death message is display-only, so
                // zero-health admission without a separate combat message still
                // respawns. The Session send path double-checks health<=0 before
                // touching the network.
                if self.screen != Screen::Death {
                    return Err("respawn_requires_death_screen".into());
                }
                if !self.can_respawn {
                    return Err("respawn_forbidden".into());
                }
                e.commands.push(UiCommand::Respawn);
                self.release_all();
            }
            NodeAction::Disconnect => {
                e.commands.push(UiCommand::Disconnect);
                self.release_all();
            }
            NodeAction::Connect => {
                if self.screen != Screen::DirectConnect {
                    return Err("connect_requires_title_screen".into());
                }
                let address = self
                    .fields
                    .get("title/server")
                    .map(|f| f.text.trim().to_owned())
                    .unwrap_or_default();
                let username = self
                    .fields
                    .get("title/username")
                    .map(|f| f.text.trim().to_owned())
                    .unwrap_or_default();
                if address.is_empty() {
                    return Err("title_server_required".into());
                }
                if username.is_empty()
                    || username.len() > 16
                    || !username
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err("offline username must contain 1..16 ASCII alphanumeric/underscore characters".into());
                }
                self.last_server = Some(address.clone());
                self.last_username = Some(username.clone());
                e.commands.push(UiCommand::Connect { address, username });
                self.release_all();
            }
            NodeAction::World(_)
            | NodeAction::PlayWorld
            | NodeAction::CancelLocalStart
            | NodeAction::OpenWorldsFolder
            | NodeAction::WorldOption(_)
            | NodeAction::CreateWorld
            | NodeAction::DeleteWorld => self.activate_world_action(a, e)?,
            NodeAction::AddServer => {
                self.adding_server = true;
                self.editing_server = None;
                self.fields.remove("server/name");
                self.title_error = None;
                self.transition(Screen::DirectConnect);
                self.focus = Some("title/server".into());
            }
            NodeAction::SaveServer => {
                if let Err(error) = self.save_server() {
                    self.title_error = Some(error);
                    self.invalidate();
                }
            }
            NodeAction::SelectServer(index) => {
                if index >= self.servers.len() {
                    return Err("server_not_found".into());
                }
                self.selected_server = Some(index);
                self.focus = Some(format!("server/{index}"));
                self.invalidate();
            }
            NodeAction::EditServer => {
                let index = self.selected_server.ok_or("no_server_selected")?;
                let server = self.servers.get(index).ok_or("server_not_found")?.clone();
                self.field("server/name", server.name, 64, false)?;
                self.field("title/server", server.address, 253, false)?;
                self.adding_server = true;
                self.editing_server = Some(index);
                self.title_error = None;
                self.transition(Screen::DirectConnect);
                self.focus = Some("server/name".into());
            }
            NodeAction::DeleteServer => {
                self.selected_server.and_then(|i| self.servers.get(i)).ok_or("no_server_selected")?;
                self.deleting_server = true;
                self.title_error = None;
                self.invalidate();
            }
            NodeAction::CancelDeleteServer => {
                self.deleting_server = false;
                self.title_error = None;
                self.invalidate();
            }
            NodeAction::ConfirmDeleteServer => {
                if !self.deleting_server {
                    return Err("server_delete_confirmation_required".into());
                }
                let index = self.selected_server.ok_or("no_server_selected")?;
                let mut servers = self.servers.clone();
                if index >= servers.len() {
                    return Err("server_not_found".into());
                }
                servers.remove(index);
                let result = crate::local_config::servers_path().ok_or_else(|| "Cannot locate local server list".to_owned())
                    .and_then(|path| super::menus::write_servers(&path, &servers));
                match result {
                    Ok(()) => {
                        self.servers = servers;
                        self.selected_server = if self.servers.is_empty() { None } else { Some(index.min(self.servers.len() - 1)) };
                        self.deleting_server = false;
                        self.title_error = None;
                    }
                    Err(error) => self.title_error = Some(error),
                }
                self.invalidate();
            }
            NodeAction::JoinServer(index) => {
                let address = self
                    .servers
                    .get(index)
                    .ok_or("server_not_found")?
                    .address
                    .clone();
                self.field("title/server", address, 253, false)?;
                self.adding_server = false;
                self.transition(Screen::DirectConnect);
                self.activate(NodeAction::Connect, m, inventory, content, e)?;
            }
            NodeAction::ServerPage(delta) => {
                self.page = self.page.saturating_add_signed(delta as isize);
                self.invalidate();
            }
            NodeAction::Quit => {
                e.commands.push(UiCommand::Shutdown);
                self.release_all();
            }
            NodeAction::Screen(s) => {
                if matches!(s, Screen::Settings | Screen::Accessibility) && self.screen != s {
                    if !self.is_options_screen() {
                        self.settings_parent = self.screen;
                    }
                    if s == Screen::Accessibility {
                        self.options_tab = super::options::INTERFACE_TAB;
                    }
                }
                if s == Screen::DirectConnect {
                    self.adding_server = false;
                    self.editing_server = None;
                    self.title_error = None;
                }
                if s == Screen::Singleplayer {
                    self.enter_singleplayer();
                    return Ok(());
                }
                if matches!(s, Screen::CreateWorld | Screen::DeleteWorld) {
                    self.title_error = None;
                }
                if self.screen == Screen::Advancements && s != Screen::Advancements {
                    e.commands.push(UiCommand::SeenAdvancements { tab: None });
                }
                if s == Screen::Advancements && self.screen != Screen::Advancements {
                    if let Some(root) = self.selected_advancement_root().map(str::to_owned) {
                        self.select_advancement_tab(&root)?;
                        e.commands
                            .push(UiCommand::SeenAdvancements { tab: Some(root) });
                    }
                }
                if s == Screen::Statistics && self.screen != Screen::Statistics {
                    self.statistics.begin_request();
                    self.statistics_group = super::statistics::StatisticsGroup::default();
                    e.commands.push(UiCommand::RequestStatistics);
                }
                self.transition(s);
            }
            NodeAction::AdvancementTab(id) => {
                if self.screen != Screen::Advancements {
                    return Err("advancement_tab_requires_advancements_screen".into());
                }
                self.select_advancement_tab(&id)?;
                e.commands
                    .push(UiCommand::SeenAdvancements { tab: Some(id) });
            }
            NodeAction::StatisticsGroup(group) => {
                if self.screen != Screen::Statistics {
                    return Err("statistics_requires_statistics_screen".into());
                }
                if self.statistics_group != group {
                    self.statistics_group = group;
                    self.scroll = 0;
                    self.invalidate();
                }
            }
            NodeAction::RecipeBook => self.execute_inner(
                UiIntent::RecipeBook {
                    context: self.context(inventory),
                    open: !self.recipe_open,
                    filter: self.recipe_filter,
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::RecipeFilter => self.execute_inner(
                UiIntent::RecipeBook {
                    context: self.context(inventory),
                    open: true,
                    filter: !self.recipe_filter,
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::SlotToggle(slot) => {
                let enabled = inventory
                    .menu()
                    .data
                    .get(&slot)
                    .copied()
                    .ok_or("crafter_state_not_observed")?
                    == 1;
                self.execute_inner(
                    UiIntent::SlotToggle {
                        context: self.context(inventory),
                        slot,
                        enabled,
                    },
                    inventory,
                    content,
                    e,
                )?;
            }
            NodeAction::BeaconEffect { primary, effect } => {
                self.beacon_selection[usize::from(!primary)] = Some(effect);
                self.invalidate();
            }
            NodeAction::BeaconSubmit => self.execute_inner(
                UiIntent::Beacon {
                    context: self.context(inventory),
                    primary: self.beacon_selection[0],
                    secondary: self.beacon_selection[1],
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::Recipe(display_id) => self.execute_inner(
                UiIntent::PlaceRecipe {
                    context: self.context(inventory),
                    display_id,
                    craft_all: m.shift,
                },
                inventory,
                content,
                e,
            )?,
            NodeAction::RecipeCategory(category) => {
                self.recipe_category = if category.is_empty() {
                    None
                } else {
                    Some(category)
                };
                self.scroll = 0;
                self.invalidate();
            }
            NodeAction::Creative(key) => {
                let stack = self
                    .creative_stacks
                    .get(&key)
                    .ok_or("creative_preset_not_retained")?;
                e.commands.push(UiCommand::CreativePick {
                    context: self.context(inventory),
                    stack: stack.clone(),
                    button: 0,
                    mode: if m.shift {
                        ClickMode::QuickMove
                    } else {
                        ClickMode::Pickup
                    },
                });
            }
            NodeAction::CreativeTab(tab) => {
                let tabs = self.creative_source["tabs"]
                    .as_array()
                    .ok_or("creative_tabs_required")?;
                let selected = tabs.get(tab).ok_or("creative_tab_not_observed")?;
                if selected["id"] == "minecraft:op_blocks" && !self.operator_permission {
                    return Err("permission_denied".into());
                }
                inventory.set_creative_inventory_tab(selected["type"] == "INVENTORY");
                self.creative_tab = tab;
                self.creative_stacks.clear();
                self.scroll = 0;
                self.invalidate();
            }
            NodeAction::CreativeClear => {
                e.commands.push(UiCommand::CreativeClear {
                    context: self.context(inventory),
                    all: m.shift,
                });
            }
            NodeAction::HotbarSave(index) => self.save_hotbar(index, inventory, content, e)?,
            NodeAction::HotbarLoad(index) => self.load_hotbar(index, inventory, e)?,
            NodeAction::ComponentClick(event) => {
                self.click_component(&event, inventory, content, e)?
            }
            NodeAction::LinkAccept => {
                let url = self
                    .pending_link
                    .take()
                    .ok_or("link_confirmation_not_visible")?;
                e.commands.push(UiCommand::OpenUrl(url));
                self.invalidate();
            }
            NodeAction::LinkCopy => {
                let url = self
                    .pending_link
                    .take()
                    .ok_or("link_confirmation_not_visible")?;
                self.clipboard.push_back(ClipboardRequest::Set(url));
                self.invalidate();
            }
            NodeAction::LinkCancel => {
                self.pending_link = None;
                self.invalidate();
            }
            NodeAction::Submit => self.submit_editor(e)?,
            NodeAction::Page(delta) => {
                let count = self
                    .fields
                    .keys()
                    .filter(|k| k.starts_with("book/page/"))
                    .count();
                let next = if delta < 0 {
                    self.page.saturating_sub(1)
                } else {
                    self.page + 1
                };
                if next >= count {
                    if matches!(self.editor, Some(EditorState::Book { writable: true, .. }))
                        && next < 100
                    {
                        self.field(&format!("book/page/{next}"), String::new(), 1024, true)?;
                    } else {
                        return Err("book_page_bound".into());
                    }
                }
                self.page = next;
                self.focus = Some(format!("book/page/{next}"));
                self.invalidate();
            }
            NodeAction::Setting(key) | NodeAction::Slider(key) => {
                let value = if key == "ui_scale" {
                    let v = self
                        .settings
                        .get(key)
                        .and_then(Value::as_f64)
                        .unwrap_or(Self::DEFAULT_UI_SCALE);
                    json!(if v >= 4. { 0. } else { v + 1. })
                } else if let Some(next) = Self::next_setting_value(key, self.settings.get(key)) {
                    next
                } else if key == "narrator" {
                    json!((self.settings.get(key).and_then(Value::as_u64).unwrap_or(0) + 1) % 4)
                } else if key == "text_background_opacity" {
                    let v = self
                        .settings
                        .get(key)
                        .and_then(Value::as_f64)
                        .unwrap_or(0.5);
                    json!(if v >= 1. { 0. } else { (v + 0.05).min(1.) })
                } else {
                    json!(!self
                        .settings
                        .get(key)
                        .and_then(Value::as_bool)
                        .unwrap_or(matches!(key, "text_background" | "keep_jigsaws")))
                };
                self.commit_setting(key, value, e);
            }
            NodeAction::OptionsTab(tab) => self.select_options_tab(tab),
            NodeAction::Rebind(id) => {
                self.rebind = Some(id);
                self.invalidate();
            }
            NodeAction::ResetBinding(id) => {
                let mut bindings = self.settings.get("key_bindings")
                    .and_then(crate::controls::Bindings::from_value).unwrap_or_default();
                bindings.reset(id);
                self.commit_setting("key_bindings", bindings.value(), e);
            }
            NodeAction::ResetBindings => {
                self.commit_setting("key_bindings", crate::controls::Bindings::default().value(), e);
            }
            NodeAction::EditorToggle(key) => {
                self.toggle_editor(key)?;
                if matches!(key, "save" | "load" | "detect") {
                    self.submit_editor(e)?;
                }
                self.invalidate();
            }
            NodeAction::Generate => {
                self.submit_editor(e)?;
                if let Some(EditorState::Jigsaw(j)) = &self.editor {
                    let levels = self.fields["jigsaw/levels"]
                        .text
                        .parse::<i32>()
                        .map_err(|_| "invalid_jigsaw_levels")?;
                    if !(0..=7).contains(&levels) {
                        return Err("invalid_jigsaw_levels".into());
                    }
                    e.commands.push(UiCommand::JigsawGenerate {
                        pos: j.pos,
                        levels,
                        keep: self
                            .settings
                            .get("keep_jigsaws")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                    });
                }
            }
        }
        Ok(())
    }
    fn toggle_editor(&mut self, key: &str) -> Result<()> {
        match self.editor.as_mut() {
            Some(EditorState::Command(c)) => match key {
                "mode" => c.mode = (c.mode + 1) % 3,
                "track_output" => c.track_output = !c.track_output,
                "conditional" => c.conditional = !c.conditional,
                "automatic" => c.automatic = !c.automatic,
                _ => return Err("unknown_editor_control".into()),
            },
            Some(EditorState::Structure(s)) => match key {
                "mode" => s.mode = if s.mode >= 2 { 0 } else { s.mode + 1 },
                "mirror" => s.mirror = (s.mirror + 1) % 3,
                "rotation" => s.rotation = (s.rotation + 1) % 4,
                "ignore_entities" => s.ignore_entities = !s.ignore_entities,
                "strict" => s.strict = !s.strict,
                "show_air" => s.show_air = !s.show_air,
                "show_box" => s.show_box = !s.show_box,
                "save" => s.update = 1,
                "load" => s.update = 2,
                "detect" => s.update = 3,
                _ => return Err("unknown_editor_control".into()),
            },
            Some(EditorState::Jigsaw(j)) if key == "joint" => {
                j.joint = if j.joint == "aligned" {
                    "rollable".into()
                } else {
                    "aligned".into()
                }
            }
            _ => return Err("editor_not_visible".into()),
        }
        Ok(())
    }
    fn submit_editor(&mut self, e: &mut UiEffects) -> Result<()> {
        let text = |id: &str| -> Result<String> {
            Ok(self
                .fields
                .get(id)
                .ok_or("missing_editor_field")?
                .text
                .clone())
        };
        match self.editor.as_ref().ok_or("editor_not_visible")? {
            EditorState::Sign { pos, front, .. } => {
                let mut lines = std::array::from_fn(|_| String::new());
                for (i, line) in lines.iter_mut().enumerate() {
                    *line = text(&format!("sign/line/{i}"))?;
                }
                e.commands.push(UiCommand::Sign {
                    pos: *pos,
                    front: *front,
                    lines,
                });
                self.editor = None;
                self.transition(Screen::Game);
            }
            EditorState::Book { slot, writable, .. } => {
                if !writable {
                    return Err("book_not_editable".into());
                }
                let count = self
                    .fields
                    .keys()
                    .filter(|k| k.starts_with("book/page/"))
                    .count();
                let mut pages = Vec::with_capacity(count);
                for i in 0..count {
                    pages.push(text(&format!("book/page/{i}"))?);
                }
                while pages.len() > 1 && pages.last().is_some_and(String::is_empty) {
                    pages.pop();
                }
                let title = text("book/title")?;
                let title = if title.trim().is_empty() {
                    None
                } else {
                    if title.chars().count() > 32 {
                        return Err("book_title_limit".into());
                    }
                    Some(title.trim().into())
                };
                e.commands.push(UiCommand::Book {
                    slot: *slot,
                    pages,
                    title,
                });
                self.editor = None;
                self.transition(Screen::Game);
            }
            EditorState::Command(c) => {
                if !self.operator_permission {
                    return Err("permission_denied".into());
                }
                let mut c = c.clone();
                c.command = text("command/command")?;
                e.commands.push(UiCommand::Command(c));
                self.transition(Screen::Game);
            }
            EditorState::Structure(s) => {
                if !self.operator_permission {
                    return Err("permission_denied".into());
                }
                let mut s = s.clone();
                s.name = text("structure/name")?;
                s.data = text("structure/data")?;
                for i in 0..3 {
                    s.offset[i] =
                        super::operator::coordinate(&text(&format!("structure/offset/{i}"))?);
                    s.size[i] = super::operator::coordinate(&text(&format!("structure/size/{i}"))?);
                }
                s.integrity = super::operator::integrity(&text("structure/integrity")?);
                s.seed = super::operator::seed(&text("structure/seed")?);
                e.commands.push(UiCommand::Structure(s));
                self.transition(Screen::Game);
            }
            EditorState::Jigsaw(j) => {
                if !self.operator_permission {
                    return Err("permission_denied".into());
                }
                let mut j = j.clone();
                j.name = text("jigsaw/name")?;
                j.target = text("jigsaw/target")?;
                j.pool = text("jigsaw/pool")?;
                for id in [&j.name, &j.target, &j.pool] {
                    if !valid_identifier(id) {
                        return Err("invalid_jigsaw_identifier".into());
                    }
                }
                j.final_state = text("jigsaw/final_state")?;
                j.selection_priority = text("jigsaw/selection_priority")?
                    .parse::<i32>()
                    .map_err(|_| "invalid_jigsaw_priority")?;
                j.placement_priority = text("jigsaw/placement_priority")?
                    .parse::<i32>()
                    .map_err(|_| "invalid_jigsaw_priority")?;
                e.commands.push(UiCommand::Jigsaw(j));
            }
        }
        Ok(())
    }
    pub fn render(
        &mut self,
        content: &Content,
        inventory: &InventoryStore,
        out: &mut MeshGeometry,
    ) -> Result<()> {
        self.synchronize(inventory, content)?;
        self.prepare_loader_previews(content)?;
        self.anim_register_nodes();
        let transform = Mat4::from_scale(Vec3::new(self.scale as f32, self.scale as f32, 1.));
        self.paint_vignette(content, out, transform)?;
        let nodes = std::mem::take(&mut self.nodes);
        let exit = self.anim.exit.take();
        let result = self.render_nodes(content, inventory, out, transform, &nodes, exit.as_ref());
        self.nodes = nodes;
        self.anim.exit = exit;
        result
    }
    /// The darkness vignette lies under the whole HUD. Its dedicated overlay
    /// partition multiplies the frame by `1 - texel * colour`, so the quad
    /// colour carries the strength.
    fn paint_vignette(&self, content: &Content, out: &mut MeshGeometry, transform: Mat4) -> Result<()> {
        let strength = self.hud.vignette.clamp(0., 1.);
        if strength < 1. / 512.
            || self.shell_screen_active()
            || self.settings.get("hide_hud").and_then(Value::as_bool) == Some(true)
        {
            return Ok(());
        }
        let layer = RenderLayer::Cutout as usize;
        std::mem::swap(&mut out.indices[layer], &mut out.overlay_blends[0]);
        let result = Painter {
            content,
            out,
            transform,
            light: [-1.; 2],
            render_layer: RenderLayer::Cutout,
        }
        .image(
            "minecraft:misc/vignette",
            [0., 0., self.width, self.height],
            [0., 0., 1., 1.],
            [strength; 3],
            1.,
        );
        std::mem::swap(&mut out.indices[layer], &mut out.overlay_blends[0]);
        result
    }
    fn render_nodes(
        &mut self,
        content: &Content,
        inventory: &InventoryStore,
        out: &mut MeshGeometry,
        transform: Mat4,
        nodes: &[SemanticNode],
        exit: Option<&super::anim::Exit>,
    ) -> Result<()> {
        let leaving = exit
            .into_iter()
            .flat_map(|e| e.nodes.iter().map(move |n| (n, Some(e.started))));
        for (node, exit_started) in leaving.chain(nodes.iter().map(|n| (n, None))) {
            if !node.visible {
                continue;
            }
            let fx = match exit_started {
                Some(started) => self.fx_exit(node, started),
                None => self.fx_enter(node),
            };
            if fx.alpha <= 0.004 {
                continue;
            }
            // Hovered buttons lift two pixels, pressed ones settle back: whole
            // GUI pixels keep the pixel look crisp while the eased factor steps.
            let lifts = node.enabled && matches!(node.paint, Paint::Button | Paint::Tab(_));
            let fx = if lifts {
                super::anim::Fx {
                    dy: fx.dy + (fx.press - fx.hover * 2.).round(),
                    ..fx
                }
            } else {
                fx
            };
            let [x, y, w, h] = node.bounds;
            let vertex_start = out.vertices.len();
            let index_start = out.indices[RenderLayer::Translucent as usize].len();
            let mut p = Painter {
                content,
                out,
                transform,
                light: [-1.; 2],
                render_layer: RenderLayer::Translucent,
            };
            match &node.paint {
                Paint::Text | Paint::HitRegion => {}
                Paint::LoaderPreview(preview) => {
                    p.solid(node.bounds, [0.06, 0.09, 0.13], 0.95);
                    let mesh = &self
                        .loader
                        .as_ref()
                        .ok_or("loader preview view missing")?
                        .preview_meshes[&preview.id];
                    crate::loader::previews::WorldPreview::append_cached(
                        mesh,
                        node.bounds,
                        transform,
                        p.out,
                    );
                }
                Paint::RichText(layout) => {
                    self.text.append_layout(
                        content,
                        layout,
                        transform * Mat4::from_translation(Vec3::new(x, y, 0.)),
                        0xffffffff,
                        None,
                        p.out,
                    )?;
                }
                Paint::OutlinedText(layout, color, outline) => {
                    self.text.append_ui_outlined_layout(
                        content,
                        layout,
                        transform * Mat4::from_translation(Vec3::new(x, y, 0.)),
                        *color,
                        *outline,
                        p.out,
                    )?;
                }
                Paint::TintedImage(name, color) => {
                    p.image(name, node.bounds, [0., 0., 1., 1.], *color, 1.)?;
                }
                Paint::Button => {
                    let selected = node.focused;
                    p.themed_button(node.bounds, node.enabled, selected, fx.hover, fx.press, 1.);
                }
                Paint::Tab(_)
                | Paint::Slider { .. }
                | Paint::Toggle { .. }
                | Paint::Split { .. }
                | Paint::Frame
                | Paint::Hotbar
                | Paint::HotbarSelection => {
                    self.paint_widget(node, &fx, content, transform, p.out)?;
                }
                Paint::Solid(color, a) => {
                    if node.id == "screen/background" {
                        self.paint_backdrop(&mut p, node.bounds, self.shell_screen_active());
                    } else {
                        p.solid(node.bounds, *color, *a);
                    }
                }
                Paint::Image(name, uv) => {
                    let region = content
                        .texture(name)
                        .ok_or_else(|| format!("missing verified GUI texture {name}"))?;
                    let source = if uv[2] > 1. || uv[3] > 1. {
                        [
                            uv[0] / region.width as f32,
                            uv[1] / region.height as f32,
                            uv[2] / region.width as f32,
                            uv[3] / region.height as f32,
                        ]
                    } else {
                        *uv
                    };
                    // Container textures take the warm theme tint (slot recesses
                    // stay visible as darker cells).
                    let tint = if node.id.starts_with("menu/background") {
                        [0.46, 0.33, 0.23]
                    } else {
                        [1.; 3]
                    };
                    p.texture(region, node.bounds, source, tint, 1., 0.);
                }
                Paint::Field(id) => {
                    // Single-line fields centre their text vertically; tall
                    // (multi-line editor) fields keep the top inset.
                    let ty = if h <= 28. {
                        y + ((h - 9.) / 2.).floor()
                    } else {
                        y + 4.
                    };
                    p.pixel_panel(
                        node.bounds,
                        super::theme::TRACK,
                        0.86,
                        super::theme::mix(
                            super::theme::RIM,
                            super::theme::ACCENT,
                            if node.focused { 1. } else { fx.hover * 0.6 },
                        ),
                        1.,
                    );
                    let f = &self.fields[id];
                    let layout = self
                        .text
                        .layout(&json!({"text":f.display()}), (w - 8.).max(1.))?;
                    if node.focused {
                        if let Some([start, end]) = f.display_selection() {
                            for glyph in layout
                                .glyphs
                                .iter()
                                .filter(|g| g.bytes[0] < end && g.bytes[1] > start)
                            {
                                p.solid(
                                    [
                                        x + 6. + glyph.position[0],
                                        ty + glyph.position[1],
                                        glyph.advance,
                                        9.,
                                    ],
                                    [0.75, 0.42, 0.10],
                                    0.7,
                                );
                            }
                        }
                    }
                    self.text.append_layout(
                        content,
                        &layout,
                        transform * Mat4::from_translation(Vec3::new(x + 6., ty, 0.)),
                        0xffffffff,
                        None,
                        p.out,
                    )?;
                    if node.focused {
                        if let Some(cursor) = f.display_cursor() {
                            let [cx, cy] = layout.caret(cursor);
                            p.solid([x + 6. + cx, ty + cy, 1., 9.], super::theme::ACCENT_HI, 1.);
                        }
                        if !f.composition.is_empty() {
                            let start = f.anchor.min(f.cursor);
                            for glyph in layout.glyphs.iter().filter(|g| {
                                g.bytes[0] >= start && g.bytes[1] <= start + f.composition.len()
                            }) {
                                p.solid(
                                    [
                                        x + 6. + glyph.position[0],
                                        ty + 8. + glyph.position[1],
                                        glyph.advance,
                                        1.,
                                    ],
                                    [1.; 3],
                                    1.,
                                );
                            }
                        }
                    }
                }
                Paint::PlayerPreview => {
                    if let Some(preview) = &mut self.player_preview {
                        append_player_preview(preview, node.bounds, self.pointer, transform, p.out);
                    }
                }
                Paint::Slot(index) => {
                    if fx.hover > 0.01 {
                        p.solid(node.bounds, super::theme::ACCENT, 0.30 * fx.hover);
                    }
                    let slots = if node.id.starts_with("hud/") {
                        &inventory.player.slots
                    } else {
                        &inventory.menu().slots
                    };
                    if let Some(Some(s)) = slots.get(*index) {
                        if !s.empty() {
                            let model = self.models.icon_model(content, s, &self.item_context);
                            let icon = [x + 1., y + 1., w - 2., h - 2.];
                            append_item(model, icon, transform, p.out)?;
                            if s.count > 1 {
                                let layout = self
                                    .text
                                    .layout(&json!({"text":s.count.to_string(),"shadow_color":0xff3f3f3fu32}), 24.)?;
                                self.text.append_layout(
                                    content,
                                    &layout,
                                    transform
                                        * Mat4::from_translation(Vec3::new(
                                            icon[0] + 17. - layout.width,
                                            icon[1] + 9.,
                                            0.,
                                        )),
                                    0xffffffff,
                                    None,
                                    p.out,
                                )?;
                            }
                        }
                    }
                }
                Paint::Creative(id) => {
                    if !self.creative_stacks.contains_key(id) {
                        return Err("creative_preset_not_retained".into());
                    }
                    let stack = &self.creative_stacks[id];
                    let model = self.models.icon_model(content, stack, &self.item_context);
                    append_item(model, node.bounds, transform, p.out)?;
                }
                Paint::TradeItem { offer, part } => {
                    let trade = inventory
                        .menu()
                        .trades
                        .as_ref()
                        .and_then(|t| t.offers.get(*offer))
                        .ok_or("trade_not_observed")?;
                    let stack = match part {
                        0 => &trade.buy_a.stack,
                        1 => {
                            &trade
                                .buy_b
                                .as_ref()
                                .ok_or("trade_second_cost_missing")?
                                .stack
                        }
                        _ => &trade.sell,
                    };
                    let model = self.models.icon_model(content, stack, &self.item_context);
                    append_item(model, node.bounds, transform, p.out)?;
                }
                Paint::RecipeDisplay(id) => {
                    let recipe = inventory.recipes.get(id).ok_or("recipe_not_observed")?;
                    if !self
                        .recipe_stacks
                        .get(id)
                        .is_some_and(|(display, _)| Arc::ptr_eq(display, &recipe.display))
                    {
                        let variants = super::display::display_variants(
                            &recipe.display["result"],
                            "result",
                            &recipe.display_stacks,
                            content,
                        )?;
                        if self.recipe_stacks.len() >= 512 {
                            self.recipe_stacks.clear();
                        }
                        self.recipe_stacks
                            .insert(*id, (Arc::clone(&recipe.display), variants));
                    }
                    let variants = &self.recipe_stacks[id].1;
                    if !variants.is_empty() {
                        let stack = &variants[(self.tick / 20) as usize % variants.len()];
                        let model = self.models.icon_model(content, stack, &self.item_context);
                        append_item(model, node.bounds, transform, p.out)?;
                    }
                }
                Paint::Variants(variants, ghost) => {
                    if *ghost {
                        p.solid(node.bounds, [1., 0., 0.], 48. / 255.);
                    }
                    if !variants.is_empty() {
                        let stack = &variants[(self.tick / 20) as usize % variants.len()];
                        let model = self.models.icon_model(content, stack, &self.item_context);
                        append_item(model, node.bounds, transform, p.out)?;
                    }
                    if *ghost {
                        p.solid(node.bounds, [1.; 3], 48. / 255.);
                    }
                }
                Paint::Map(id) => {
                    let map = self.maps.get(id).ok_or("map_not_observed")?;
                    map.append_gui_geometry(
                        content,
                        &self.text,
                        transform
                            * Mat4::from_translation(Vec3::new(x, y, 0.))
                            * Mat4::from_scale(Vec3::new(w / 128., h / 128., 1.)),
                        false,
                        p.out,
                    )?;
                }
                Paint::LoaderMap(points) => {
                    p.solid(node.bounds, [0.06, 0.09, 0.13], 0.95);
                    p.solid(
                        [x + 8., y + h / 2., (w - 16.).max(0.), 1.],
                        [0.16, 0.20, 0.25],
                        1.,
                    );
                    p.solid(
                        [x + w / 2., y + 8., 1., (h - 16.).max(0.)],
                        [0.16, 0.20, 0.25],
                        1.,
                    );
                    let min_x = points.iter().map(|v| v[0]).fold(f64::INFINITY, f64::min);
                    let max_x = points
                        .iter()
                        .map(|v| v[0])
                        .fold(f64::NEG_INFINITY, f64::max);
                    let min_z = points.iter().map(|v| v[1]).fold(f64::INFINITY, f64::min);
                    let max_z = points
                        .iter()
                        .map(|v| v[1])
                        .fold(f64::NEG_INFINITY, f64::max);
                    let scale = ((w - 32.).max(0.) as f64 / (max_x - min_x).max(1.))
                        .min((h - 32.).max(0.) as f64 / (max_z - min_z).max(1.));
                    for (i, point) in points.iter().enumerate() {
                        let px = x + w / 2. + ((point[0] - (min_x + max_x) / 2.) * scale) as f32;
                        let pz = y + h / 2. + ((point[1] - (min_z + max_z) / 2.) * scale) as f32;
                        p.solid([px - 2., pz - 2., 5., 5.], [1., 0.82, 0.38], 1.);
                        let label = self
                            .text
                            .layout(&json!({"text":(i + 1).to_string()}), 32.)?;
                        self.text.append_layout(
                            content,
                            &label,
                            transform
                                * Mat4::from_translation(Vec3::new(
                                    px - label.width / 2.,
                                    pz - 12.,
                                    0.,
                                )),
                            0xffffffff,
                            None,
                            p.out,
                        )?;
                    }
                }
            }
            let label = if matches!(
                node.paint,
                Paint::Slot(_)
                    | Paint::PlayerPreview
                    | Paint::Field(_)
                    | Paint::Creative(_)
                    | Paint::Map(_)
                    | Paint::LoaderPreview(_)
                    | Paint::LoaderMap(_)
                    | Paint::TradeItem { .. }
                    | Paint::RecipeDisplay(_)
                    | Paint::Variants(_, _)
                    | Paint::RichText(_)
                    | Paint::OutlinedText(_, _, _)
                    | Paint::TintedImage(_, _)
                    | Paint::HitRegion
                    | Paint::Tab(_)
                    | Paint::Slider { .. }
                    | Paint::Toggle { .. }
                    | Paint::Split { .. }
                    | Paint::Frame
                    | Paint::Hotbar
                    | Paint::HotbarSelection
            ) {
                ""
            } else {
                node.label.as_str()
            };
            if !label.is_empty() {
                let menu = self.screen.profile() != super::anim::Profile::None;
                let offset = if matches!(node.paint, Paint::Field(_) | Paint::Button) {
                    4.
                } else {
                    0.
                };
                let wordmark = node.id == "title/name";
                let heading = matches!(node.id.as_str(), "death/title" | "pause/title");
                let panel_title = node.id.ends_with("/title") && menu;
                let text_scale = if wordmark {
                    4.
                } else if heading {
                    2.
                } else {
                    1.
                };
                let plain = json!({"text":label});
                let layout = self.text.layout(
                    node.component.as_deref().unwrap_or(&plain),
                    ((w - offset * 2.) / text_scale).max(1.),
                )?;
                let centered = wordmark
                    || heading
                    || matches!(node.paint, Paint::Button)
                    || node.id == "dialog/title"
                    || node.id.starts_with("dialog/body/")
                    || matches!(
                        node.id.as_str(),
                        "title/subtitle"
                            | "death/message"
                            | "loading/status"
                            | "hud/selected_name"
                            | "options/title"
                            | "connect/title"
                            | "multiplayer/title"
                            | "multiplayer/empty"
                            | "disconnect/title"
                            | "disconnect/reason"
                            | "statistics/title"
                    );
                let text_position = if centered {
                    [
                        (x + (w - layout.width * text_scale) * 0.5).floor(),
                        (y + (h - layout.height * text_scale) * 0.5).floor(),
                    ]
                } else {
                    [x + offset, y + offset]
                };
                let color = if !node.enabled {
                    if menu {
                        super::theme::TEXT_OFF
                    } else {
                        0xff888888
                    }
                } else if !menu {
                    0xffffffff
                } else if wordmark || heading || panel_title {
                    super::theme::HEADING
                } else if matches!(node.paint, Paint::Button) {
                    if fx.hover > 0.4 {
                        0xffffe2a8
                    } else {
                        super::theme::TEXT
                    }
                } else if node.id == "title/error" {
                    0xffff8466
                } else if node.id == "title/subtitle" {
                    0xffd9a566
                } else if node.id == "title/version" {
                    super::theme::TEXT_DIM
                } else {
                    super::theme::TEXT
                };
                let place = |dx: f32, dy: f32| {
                    transform
                        * Mat4::from_translation(Vec3::new(
                            text_position[0] + dx,
                            text_position[1] + dy,
                            0.,
                        ))
                        * Mat4::from_scale(Vec3::new(text_scale, text_scale, 1.))
                };
                if wordmark {
                    // Chunky pixel drop shadow in deep ember.
                    self.text.append_layout(
                        content,
                        &layout,
                        place(2., 2.),
                        0xff5a2608,
                        None,
                        p.out,
                    )?;
                }
                self.text
                    .append_layout(content, &layout, place(0., 0.), color, None, p.out)?;
            }
            let scale = self.scale as f32;
            // Themed controls draw a glow ring and may lift: clip loosely.
            let pad = if matches!(
                node.paint,
                Paint::Button
                    | Paint::Tab(_)
                    | Paint::Slider { .. }
                    | Paint::Toggle { .. }
                    | Paint::Split { .. }
                    | Paint::Frame
                    | Paint::Field(_)
            ) {
                4.
            } else {
                0.
            };
            let [cx, cy, cw, ch] = node.clip.unwrap_or(node.bounds);
            super::clipping::clip(
                p.out,
                vertex_start,
                index_start,
                [
                    (cx - pad) * scale,
                    (cy - pad) * scale,
                    (cw + pad * 2.) * scale,
                    (ch + pad * 2.) * scale,
                ],
            )?;
            if let Paint::Slider { desc, .. }
            | Paint::Toggle { desc, .. }
            | Paint::Split { desc, .. } = &node.paint
            {
                // Outside the row's own clip rect: the shared options footer.
                self.footer_text(&mut p, node, &fx, desc, content, transform)?;
            }
            let (dx, dy) = (fx.dx * scale, fx.dy * scale);
            if fx.alpha < 1. || dx != 0. || dy != 0. {
                for v in &mut p.out.vertices[vertex_start..] {
                    v.position[0] += dx;
                    v.position[1] += dy;
                    v.alpha *= fx.alpha;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn add(
        &mut self,
        id: String,
        role: &'static str,
        label: String,
        bounds: [f32; 4],
        enabled: bool,
        deny: Option<&'static str>,
        action: NodeAction,
        paint: Paint,
    ) -> Result<()> {
        if self.nodes.len() >= NODE_LIMIT || label.len() > TEXT_LIMIT {
            return Err("retained_ui_limit".into());
        }
        let visible = bounds[0] >= 0.
            && bounds[1] >= 0.
            && bounds[0] + bounds[2] <= self.width
            && bounds[1] + bounds[3] <= self.height;
        let focused = self.focus.as_ref() == Some(&id);
        self.nodes.push(SemanticNode {
            id,
            role,
            label,
            component: None,
            clip: None,
            bounds,
            enabled,
            visible,
            deny_reason: deny,
            focused,
            owner: "minecraft",
            action,
            paint,
        });
        Ok(())
    }
    fn text_hit(&self, id: &str, p: [f32; 2]) -> Result<usize> {
        let node = self.target(id)?;
        let field = self.fields.get(id).ok_or("text_focus_required")?;
        let layout = self
            .text
            .layout(&json!({"text":field.text}), (node.bounds[2] - 8.).max(1.))?;
        Ok(layout.hit([p[0] - node.bounds[0] - 4., p[1] - node.bounds[1] - 4.]))
    }
    /// Test seam: drive a `NodeAction` without a built semantic tree
    /// (state-only tests have no fonts, so `target()` fails closed).
    #[cfg(test)]
    pub(crate) fn activate_for_test(
        &mut self,
        action: NodeAction,
        inventory: &mut InventoryStore,
        content: &Content,
    ) -> Result<UiEffects> {
        let mut effects = UiEffects::default();
        self.activate(
            action,
            Modifiers::default(),
            inventory,
            content,
            &mut effects,
        )?;
        Ok(effects)
    }
}
pub(super) fn contains([x, y, w, h]: [f32; 4], [px, py]: [f32; 2]) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}
fn valid_identifier(id: &str) -> bool {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    !ns.is_empty()
        && !path.is_empty()
        && ns
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c))
        && path
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_./-".contains(c))
}
fn append_item(
    model: &crate::protocol::models::BakedModel,
    [x, y, w, h]: [f32; 4],
    parent: Mat4,
    out: &mut MeshGeometry,
) -> Result<()> {
    let d = model.display.get("gui").copied().unwrap_or_default();
    let r = d.rotation.map(f32::to_radians);
    let transform = parent
        * Mat4::from_translation(Vec3::new(x + w / 2., y + h / 2., 0.))
        * Mat4::from_scale(Vec3::new(w, -h, 1.))
        * Mat4::from_translation(Vec3::from_array(d.translation) / 16.)
        * Mat4::from_rotation_y(r[1])
        * Mat4::from_rotation_x(r[0])
        * Mat4::from_rotation_z(r[2])
        * Mat4::from_scale(Vec3::from_array(d.scale))
        * Mat4::from_translation(Vec3::splat(-0.5));
    if model.quads.len() > 65536 {
        return Err("item_geometry_limit".into());
    }
    for q in &model.quads {
        let color = match q.tint {
            TintKind::Constant(c) => c,
            TintKind::None => [1.; 3],
            _ => return Err("unresolved_item_tint".into()),
        };
        let base = out.vertices.len() as u32;
        for i in 0..4 {
            out.vertices.push(Vertex {
                position: transform
                    .transform_point3(Vec3::from_array(q.positions[i]))
                    .to_array(),
                color,
                uv: q.uv[i],
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                alpha: q.alpha,
                texture_layer: q.texture_layer,
                light: [-1.; 2],
                normal: [0.; 3],
            });
        }
        out.indices[RenderLayer::Translucent as usize].extend([
            base,
            base + 1,
            base + 2,
            base,
            base + 2,
            base + 3,
        ]);
    }
    Ok(())
}

fn append_player_preview(
    preview: &mut PlayerPreview,
    [x, y, width, height]: [f32; 4],
    pointer: [f32; 2],
    parent: Mat4,
    out: &mut MeshGeometry,
) {
    let yaw = ((x + width * 0.5 - pointer[0]) / 40.).atan();
    let pitch = ((y + height * 0.4 - pointer[1]) / 40.).atan();
    let rotation = Mat4::from_rotation_x(pitch * 0.25)
        * Mat4::from_rotation_y(std::f32::consts::PI + yaw * 0.5);
    for (index, depth) in &mut preview.order {
        let center = preview.quads[*index]
            .iter()
            .map(|vertex| Vec3::from_array(vertex.position))
            .sum::<Vec3>()
            * 0.25;
        *depth = rotation.transform_point3(center).z;
    }
    preview.order.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
    let scale = (height / 2.2).min(width / 1.2);
    let matrix = parent
        * Mat4::from_translation(Vec3::new(x + width * 0.5, y + height * 0.3, 0.))
        * Mat4::from_scale(Vec3::splat(scale))
        * rotation;
    for (index, _) in &preview.order {
        let base = out.vertices.len() as u32;
        out.vertices
            .extend(preview.quads[*index].iter().map(|vertex| {
                let mut vertex = *vertex;
                vertex.position = matrix
                    .transform_point3(Vec3::from_array(vertex.position))
                    .to_array();
                vertex
            }));
        out.indices[RenderLayer::Translucent as usize].extend([
            base,
            base + 1,
            base + 2,
            base,
            base + 2,
            base + 3,
        ]);
    }
}
#[cfg(test)]
mod client_settings_tests {
    use super::*;
    #[test]
    fn player_preview_preserves_activated_skin_uvs_and_projects_inside_recess() {
        use bytemuck::Zeroable;
        let quad = [
            [-0.25, -0.5, 0.],
            [0.25, -0.5, 0.],
            [0.25, 1.5, 0.],
            [-0.25, 1.5, 0.],
        ]
        .map(|position| Vertex {
            position,
            uv: [0.2, 0.3],
            texture_layer: 7,
            ..Vertex::zeroed()
        });
        let mut preview = PlayerPreview {
            texture: crate::protocol::content::TextureRegion {
                uv: [0., 0., 1., 1.],
                layer: 7,
                width: 64,
                height: 64,
            },
            model: crate::entities::PlayerModel::Wide,
            quads: vec![quad],
            order: vec![(0, 0.)],
        };
        let mut geometry = MeshGeometry::default();
        append_player_preview(
            &mut preview,
            [26., 8., 51., 70.],
            [51.5, 36.],
            Mat4::IDENTITY,
            &mut geometry,
        );
        assert_eq!(geometry.vertices.len(), 4);
        assert_eq!(geometry.indices[RenderLayer::Translucent as usize].len(), 6);
        for vertex in &geometry.vertices {
            assert!((26. ..=77.).contains(&vertex.position[0]));
            assert!((8. ..=78.).contains(&vertex.position[1]));
            assert_eq!(vertex.uv, [0.2, 0.3]);
            assert_eq!(vertex.texture_layer, 7);
        }
    }
    #[test]
    fn auto_gui_scale_uses_vanilla_minimum_viewport_and_option_applies() {
        assert_eq!(UiState::gui_scale(1280, 720, 0), 3);
        assert_eq!(UiState::gui_scale(1920, 1080, 0), 4);
        assert_eq!(UiState::gui_scale(854, 480, 0), 2);
        assert_eq!(UiState::gui_scale(1280, 720, 4), 3);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        let mut ui = UiState::new_state(&content).unwrap();
        ui.resize(1280, 720, 1.);
        assert_eq!((ui.width, ui.height, ui.scale), (427., 240., 3.));
        ui.apply_setting("ui_scale", json!(2.)).unwrap();
        assert_eq!((ui.width, ui.height, ui.scale), (640., 360., 2.));
        ui.apply_setting("ui_scale", json!(0.)).unwrap();
        assert_eq!(ui.scale, 3.);
    }
    #[test]
    fn entering_title_preserves_last_connection_defaults() {
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        let mut ui = UiState::new_state(&content).unwrap();
        ui.enter_title(Some("localhost:31415"), Some("RejoinPlayer"))
            .unwrap();
        assert_eq!(
            ui.title_defaults(),
            ("localhost:31415".into(), "RejoinPlayer".into())
        );
        ui.enter_title(None, None).unwrap();
        assert_eq!(
            ui.title_field("title/server").as_deref(),
            Some("localhost:31415")
        );
        assert_eq!(
            ui.title_field("title/username").as_deref(),
            Some("RejoinPlayer")
        );
    }
    #[test]
    fn cycles_wrap_and_clamp() {
        // Unset yields defaults first.
        assert_eq!(
            UiState::next_setting_value("render_distance", None),
            Some(json!(UiState::DEFAULT_RENDER_DISTANCE))
        );
        assert_eq!(
            UiState::next_setting_value("fov", None),
            Some(json!(UiState::DEFAULT_FOV_DEG))
        );
        assert_eq!(
            UiState::next_setting_value("mouse_sensitivity", None),
            Some(json!(UiState::DEFAULT_MOUSE_SENSITIVITY))
        );
        assert_eq!(
            UiState::next_setting_value("volume", None),
            Some(json!(UiState::DEFAULT_VOLUME))
        );
        // Increments.
        assert_eq!(
            UiState::next_setting_value("render_distance", Some(&json!(8))),
            Some(json!(9))
        );
        assert_eq!(
            UiState::next_setting_value("fov", Some(&json!(70.))),
            Some(json!(75.))
        );
        assert_eq!(
            UiState::next_setting_value("mouse_sensitivity", Some(&json!(1.))),
            Some(json!(1.25))
        );
        assert_eq!(
            UiState::next_setting_value("volume", Some(&json!(0.9))),
            Some(json!(1.0))
        );
        // Maximums wrap.
        assert_eq!(
            UiState::next_setting_value(
                "render_distance",
                Some(&json!(UiState::MAX_RENDER_DISTANCE))
            ),
            Some(json!(UiState::MIN_RENDER_DISTANCE))
        );
        assert_eq!(
            UiState::next_setting_value("fov", Some(&json!(110.0))),
            Some(json!(30.))
        );
        assert_eq!(
            UiState::next_setting_value("mouse_sensitivity", Some(&json!(3.0))),
            Some(json!(0.1))
        );
        assert_eq!(
            UiState::next_setting_value("volume", Some(&json!(1.0))),
            Some(json!(0.))
        );
        // Unknown keys keep legacy behavior (no cycle value).
        assert_eq!(
            UiState::next_setting_value("hide_hud", Some(&json!(true))),
            None
        );
        assert_eq!(UiState::next_setting_value("hide_hud", None), None);
        // Graphics preset cycles Low -> Medium -> High -> Low; unset is Low.
        assert_eq!(
            UiState::next_setting_value("graphics_preset", None),
            Some(json!("medium"))
        );
        assert_eq!(
            UiState::next_setting_value("graphics_preset", Some(&json!("low"))),
            Some(json!("medium"))
        );
        assert_eq!(
            UiState::next_setting_value("graphics_preset", Some(&json!("medium"))),
            Some(json!("high"))
        );
        assert_eq!(
            UiState::next_setting_value("graphics_preset", Some(&json!("high"))),
            Some(json!("low"))
        );
    }
    #[test]
    fn sanitize_clamps_ranges() {
        let num = |key: &str, v: serde_json::Value| {
            UiState::sanitize_setting_value(key, &v)
                .unwrap()
                .as_f64()
                .unwrap()
        };
        assert!((num("fov", json!(500.)) - 110.).abs() < 1e-9);
        assert!((num("fov", json!(-5.)) - 30.).abs() < 1e-9);
        assert!((num("volume", json!(2.)) - 1.).abs() < 1e-9);
        assert!((num("volume", json!(-1.)) - 0.).abs() < 1e-9);
        assert!((num("mouse_sensitivity", json!(99.)) - 3.).abs() < 1e-9);
        assert_eq!(
            UiState::sanitize_setting_value("render_distance", &json!(100)),
            Some(json!(UiState::MAX_RENDER_DISTANCE))
        );
        assert_eq!(
            UiState::sanitize_setting_value("render_distance", &json!(0)),
            Some(json!(UiState::MIN_RENDER_DISTANCE))
        );
        assert!(UiState::sanitize_setting_value("fov", &json!("wide")).is_none());
        assert!(UiState::sanitize_setting_value("bogus", &json!(1)).is_none());
        assert_eq!(
            UiState::sanitize_setting_value("graphics_preset", &json!("HIGH")),
            Some(json!("high"))
        );
        assert_eq!(
            UiState::default_setting_value("graphics_preset"),
            Some(json!("low"))
        );
        assert!(UiState::sanitize_setting_value("graphics_preset", &json!("ultra")).is_none());
        assert!(UiState::sanitize_setting_value("graphics_preset", &json!(true)).is_none());
    }
}
#[cfg(test)]
mod vignette_tests {
    use super::*;
    use crate::protocol::content::TextureRegion;

    /// The HUD darkness vignette is one window-sized `misc/vignette` quad on
    /// the dedicated multiply partition, whose colour is the strength.
    /// No darkness, a hidden HUD or a shell screen paint nothing.
    #[test]
    fn vignette_is_one_full_window_quad_on_the_multiply_layer() {
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs": []}"#.as_slice());
        let region = TextureRegion { uv: [0.25, 0.5, 0.75, 1.], layer: 3, width: 256, height: 256 };
        content.insert_test_texture("minecraft:misc/vignette", region);
        let mut ui = UiState::new_state(&content).unwrap();
        // 1920x1080 divides evenly by every GUI scale, so the quad ends exactly at the edge.
        ui.resize(1920, 1080, 1.);
        let transform = Mat4::from_scale(Vec3::new(ui.scale as f32, ui.scale as f32, 1.));
        let paint = |ui: &UiState| {
            let mut out = MeshGeometry::default();
            ui.paint_vignette(&content, &mut out, transform).unwrap();
            out
        };
        ui.hud.vignette = 0.6;
        let out = paint(&ui);
        let layer = &out.overlay_blends[0];
        assert_eq!(layer.len(), 6);
        assert!(out.indices.iter().all(Vec::is_empty));
        assert!(out.overlay_blends[1].is_empty());
        let corners: Vec<_> = layer.iter().map(|&i| out.vertices[i as usize]).collect();
        for vertex in &corners {
            assert_eq!(vertex.color, [0.6; 3]);
            assert_eq!(vertex.texture_layer, 3);
        }
        let extent = |axis: usize| {
            corners.iter().map(|v| v.position[axis]).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)))
        };
        assert_eq!(extent(0), (0., 1920.));
        assert_eq!(extent(1), (0., 1080.));
        let us = corners.iter().map(|v| v.uv[0]).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        assert_eq!(us, (0.25, 0.75));

        ui.hud.vignette = 0.;
        assert!(paint(&ui).vertices.is_empty());
        ui.hud.vignette = 0.6;
        ui.settings.insert("hide_hud".into(), json!(true));
        assert!(paint(&ui).vertices.is_empty());
        ui.settings.remove("hide_hud");
        ui.transition(Screen::Title);
        assert!(paint(&ui).vertices.is_empty());
    }
}
