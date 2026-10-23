//! One packet-applied session authority shared by native and offscreen application owners.
mod actions;
mod block_effects;
mod block_motion;
mod local_server;
#[cfg(test)]
mod entity_aim_tests;
#[cfg(test)]
mod mining_start_tests;
mod observation;
mod packets;
mod ui_packets;

use crate::{
    api::{Command, InputIntent, Observation, Scene, Vertex},
    audio::Audio,
    entities::Store as EntityStore,
    environment::Environment,
    inventory::{InventoryStore, PacketCommand},
    player::Player,
    protocol::{
        Result, Writer,
        content::Content,
        network::{Event, Network, Phase},
        world::World,
    },
    terrain::Terrain,
    ui::UiState,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, RwLock, mpsc},
    thread,
    time::Instant,
};
type Wake = Arc<dyn Fn() + Send + Sync>;
struct PredictedBlock {
    received: u32,
    sequence: i32,
}
struct Loaded {
    content: Content,
    network: Network,
    models: Arc<crate::entities::Models>,
}

pub(crate) const TICK_DETAIL_NAMES: [&str; 11] = [
    "events", "environment", "player_environment_context", "world_border",
    "block_effects", "player", "entities_presentation", "entities_block_sources",
    "ui_synchronize", "commands_effects_atlas", "refresh_scene",
];

#[derive(Default)]
pub(crate) struct ScreenAudioStats {
    pub packets: u64,
    /// Opus frames the mixer decoded for this screen, and the loudest sample seen.
    pub decoded_frames: u64,
    pub peak: f32,
}
/// Owner-side counters of the shared source's audio capture.
#[derive(Default)]
pub(crate) struct ScreenAudioOwner {
    pub capturing: bool,
    pub packets_sent: u64,
    pub unavailable: bool,
}

pub struct Session {
    instance_id: String,
    presentation: bool,
    network: Option<Network>,
    loading: Option<mpsc::Receiver<Result<Loaded>>>,
    wake: Arc<RwLock<Wake>>,
    pub(crate) phase: String,
    error: Option<String>,
    address: Option<String>,
    username: Option<String>,
    pub(crate) content: Option<Content>,
    pub(crate) world: World,
    pub(crate) player: Player,
    pub(crate) inventory: InventoryStore,
    pub(crate) entities: EntityStore,
    pub(crate) ui: Option<UiState>,
    ui_viewport: Option<(u32, u32, f64)>,
    /// Monotonic epoch of the UI animation clock (frame time, never sleeps).
    ui_clock: std::time::Instant,
    loader: crate::loader::Loader,
    pub(crate) screen_relay: crate::computer::relay::Relay,
    /// Per-screen audio counters for observation (viewer side, owner's own monitor).
    pub(crate) screen_audio_owner: ScreenAudioOwner,
    pub(crate) screen_audio: std::collections::BTreeMap<crate::computer::relay::Id, ScreenAudioStats>,
    pub(crate) voice_net: crate::voice::Net,
    environment: Environment,
    terrain: Option<Terrain>,
    scene: Scene,
    presentation_partial: f32,
    camera_mode: u8,
    renderer_scene_confirmation: Option<(u64, u64, u64)>,
    animation_patches: Vec<crate::api::AtlasPatch>,
    audio: Option<Audio>,
    connection_epoch: u64,
    dimension_epoch: u64,
    resource_epoch: u64,
    revision: u64,
    scene_state_revision: u64,
    /// Last (revision, client_tick, packet_sequence) triple already pushed to
    /// the shared owner observation. `observation_changed_since_publish()`
    /// lets App skip the full world/observation serialization on idle turns
    /// without touching automation `state_version`/`wait_*` semantics.
    last_published_snapshot: (u64, u64, u64),
    /// Per-subsystem tick accumulator, drained by App.draw into
    /// `render["cpu_session_tick_detail_ms"]` for Xvfb profiling.
    tick_detail_ms: [f64; 11],
    state_version: u64,
    health_version: u64,
    container_packet_version: u64,
    packet_sequence: u64,
    pub(crate) client_tick: u64,
    keys: BTreeSet<String>,
    registries: BTreeMap<String, Vec<(String, Value)>>,
    registry_tags: BTreeMap<String, BTreeMap<String, Vec<i32>>>,
    features: Arc<BTreeSet<String>>,
    dimension: Option<String>,
    flat_world: bool,
    predicted_blocks: BTreeMap<[i32; 3], PredictedBlock>,
    received_chunks: BTreeMap<[i32; 2], u64>,
    received_blocks: BTreeMap<[i32; 3], u64>,
    /// Server ClientboundBlockDestruction (Play id 5) keyed by breaker entity
    /// id. Stage 0..=9 renders cracks; stage >= 10 removes the entry (vanilla
    /// drops progress on negative or >= 10 / missing entries). Evict-oldest
    /// bound at 4096 entries.
    destruction: BTreeMap<i32, ([i32; 3], u8)>,
    block_effects: block_effects::BlockEffects,
    block_motion: block_motion::BlockMotions,
    commands: Vec<PacketCommand>,
    events: VecDeque<Value>,
    packet_counts: BTreeMap<i32, u64>,
    ignored_presentation: BTreeMap<i32, u64>,
    status: Option<Value>,
    server_frozen: bool,
    step_ticks: i32,
    /// Client-local Options (Options screen, `client-settings.json`). Render
    /// distance is in chunks, FOV in degrees, sensitivity multiplies the 0.12
    /// base look scale, volume is the Master gain. All defaults live on UiState.
    render_distance: u32,
    fov_deg: f32,
    mouse_sensitivity: f32,
    damage_tilt_strength: f64,
    /// Vanilla Brightness option fed to the lightmap gamma (0 Moody .. 1 Bright).
    gamma: f32,
    volume: f32,
    presentation_settings: BTreeMap<String, Value>,
    /// Singleplayer: the running local server, a start in flight, first-join
    /// world settings still to apply, and the background stopper.
    local_server: Option<crate::singleplayer::Server>,
    local_start: Option<local_server::Start>,
    local_setup: Option<local_server::Setup>,
    local_stopper: local_server::Stopper,
    /// Native shell lifecycle is opt-in for windowed clients; headless probes
    /// keep their existing protocol-only disconnect behavior.
    title_allowed: bool,
    /// F3 overlay mirror: the App owner holds the real flag (physical F3 key
    /// + `minecraft_toggle_debug_overlay` intercept); the session mirrors it
    /// here so `minecraft_observation` (which the session builds) can publish
    /// `debug_overlay` for automation predicates.
    pub(crate) debug_overlay_pending: bool,
}
impl Session {
    pub fn new(instance_id: String) -> Self {
        Self::new_mode(instance_id, true)
    }
    pub(crate) fn new_mode(instance_id: String, presentation: bool) -> Self {
        let mut player = Player::new();
        player.position = [4.5, 2., 7.5];
        player.yaw = 180.;
        player.pitch = 8.;
        let mut out = Self {
            instance_id,
            presentation,
            network: None,
            loading: None,
            wake: Arc::new(RwLock::new(Arc::new(|| {}))),
            phase: "fixture".into(),
            error: None,
            address: None,
            username: None,
            content: None,
            world: World::default(),
            player,
            inventory: InventoryStore::default(),
            entities: EntityStore::new(),
            ui: None,
            ui_viewport: None,
            ui_clock: std::time::Instant::now(),
            loader: crate::loader::Loader::default(),
            screen_relay: Default::default(),
            screen_audio: Default::default(),
            screen_audio_owner: Default::default(),
            voice_net: Default::default(),
            environment: Environment::new(),
            terrain: None,
            scene: Scene::default(),
            presentation_partial: 0.,
            camera_mode: 0,
            renderer_scene_confirmation: None,
            animation_patches: Vec::new(),
            audio: presentation.then(Audio::new),
            connection_epoch: 0,
            dimension_epoch: 0,
            resource_epoch: 0,
            revision: 1,
            scene_state_revision: 0,
            last_published_snapshot: (1, 0, 0),
            tick_detail_ms: [0.; 11],
            state_version: 1,
            health_version: 0,
            container_packet_version: 0,
            packet_sequence: 0,
            client_tick: 0,
            keys: BTreeSet::new(),
            registries: BTreeMap::new(),
            registry_tags: BTreeMap::new(),
            features: Arc::new(BTreeSet::new()),
            dimension: None,
            flat_world: false,
            predicted_blocks: BTreeMap::new(),
            received_chunks: BTreeMap::new(),
            received_blocks: BTreeMap::new(),
            destruction: BTreeMap::new(),
            block_effects: block_effects::BlockEffects::default(),
            block_motion: block_motion::BlockMotions::default(),
            commands: Vec::with_capacity(32),
            events: VecDeque::with_capacity(64),
            packet_counts: BTreeMap::new(),
            ignored_presentation: BTreeMap::new(),
            status: None,
            server_frozen: false,
            step_ticks: 0,
            title_allowed: false,
            debug_overlay_pending: false,
            render_distance: UiState::DEFAULT_RENDER_DISTANCE,
            fov_deg: UiState::DEFAULT_FOV_DEG,
            mouse_sensitivity: UiState::DEFAULT_MOUSE_SENSITIVITY,
            damage_tilt_strength: UiState::DEFAULT_DAMAGE_TILT_STRENGTH,
            gamma: UiState::DEFAULT_GAMMA as f32,
            volume: UiState::DEFAULT_VOLUME,
            presentation_settings: BTreeMap::new(),
            local_server: None,
            local_start: None,
            local_setup: None,
            local_stopper: local_server::Stopper::default(),
        };
        out.fixture();
        out.camera();
        out.scene.fov_deg = out.fov_deg;
        out
    }
    pub fn set_wake(&mut self, wake: Wake) {
        *self.wake.write().expect("session wake lock poisoned") = wake;
    }
    fn wake_callback(&self) -> Wake {
        let wake = self.wake.clone();
        Arc::new(move || {
            let callback = wake.read().expect("session wake lock poisoned").clone();
            callback();
        })
    }
    pub fn phase(&self) -> &'static str {
        match self.phase.as_str() {
            "fixture" => "fixture",
            "status" => "status",
            "loading_resources" => "loading_resources",
            "login" => "login",
            "configuration" => "configuration",
            "play" => "play",
            "error" => "error",
            _ => "disconnected",
        }
    }
    pub fn screen(&self) -> &'static str {
        if let Some(ui) = &self.ui {
            match ui.screen() {
                crate::ui::Screen::Loading => return "loading",
                crate::ui::Screen::Loader => return "loader",
                crate::ui::Screen::ServerDialog => return "dialog",
                crate::ui::Screen::Title => return "title",
                crate::ui::Screen::Multiplayer => return "multiplayer",
                crate::ui::Screen::DirectConnect => return "direct_connect",
                crate::ui::Screen::Singleplayer => return "singleplayer",
                crate::ui::Screen::CreateWorld => return "create_world",
                crate::ui::Screen::DeleteWorld => return "delete_world",
                crate::ui::Screen::SingleplayerStarting => return "singleplayer_starting",
                crate::ui::Screen::Disconnected => return "disconnected",
                crate::ui::Screen::Pause => return "pause",
                crate::ui::Screen::Settings | crate::ui::Screen::Accessibility => return "options",
                _ => {}
            }
        }
        if self.inventory.screen_open { "inventory" } else { "game" }
    }
    pub fn enable_native_shell(&mut self) {
        self.title_allowed = true;
    }
    pub fn shell_screen_active(&self) -> bool {
        self.ui.as_ref().is_some_and(UiState::shell_screen_active)
    }
    pub fn native_setting(&self, key: &str) -> Option<&Value> {
        self.presentation_settings.get(key)
    }
    /// True while a dialog or loading overlay owns native input. Screen labels
    /// expose the loading gate so the UI cannot be mistaken for playable terrain.
    pub fn dialog_screen_active(&self) -> bool {
        self.ui.as_ref().is_some_and(|ui| ui.dialog_screen_active())
    }
    /// True while a server-driven dialog owns native input (text fields included).
    pub fn server_dialog_open(&self) -> bool {
        self.ui.as_ref().is_some_and(UiState::server_dialog_open)
    }
    /// True while the chat overlay owns native text input.
    pub fn chat_open(&self) -> bool {
        self.ui
            .as_ref()
            .is_some_and(|ui| ui.screen() == crate::ui::Screen::Chat)
    }
    /// True while a text field owns the keyboard on a menu/inventory/editor screen.
    pub fn text_input_active(&self) -> bool {
        self.ui.as_ref().is_some_and(UiState::text_input_active)
    }
    /// True on the creative "Saved Hotbars" tab (digits load/save hotbars).
    pub fn creative_hotbar_tab(&self) -> bool {
        self.ui.as_ref().is_some_and(UiState::creative_hotbar_tab)
    }
    pub fn context_epochs(&self) -> (u64, u64) {
        (self.connection_epoch, self.dimension_epoch)
    }
    pub fn state_version(&self) -> u64 {
        self.state_version
    }
    /// True when revision/client_tick/packet_sequence moved since the last
    /// owner publication; App uses it to skip the full observation rebuild
    /// on idle turns. `state_version` keeps its automation wait semantics.
    pub fn observation_changed_since_publish(&self) -> bool {
        (self.revision, self.client_tick, self.packet_sequence) != self.last_published_snapshot
    }
    pub fn note_published(&mut self) {
        self.last_published_snapshot = (self.revision, self.client_tick, self.packet_sequence);
    }
    /// Drain per-subsystem tick timings for App.draw profiling output.
    pub(crate) fn take_tick_detail(&mut self) -> [f64; 11] {
        std::mem::take(&mut self.tick_detail_ms)
    }
    pub fn health_version(&self) -> u64 {
        self.health_version
    }
    pub fn container_packet_version(&self) -> u64 {
        self.container_packet_version
    }
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub(crate) fn release_uploaded_terrain(
        &mut self, uploaded: impl IntoIterator<Item = ([i32; 3], u64)>,
    ) {
        if let Some(terrain) = &mut self.terrain {
            terrain.release_uploaded(&mut self.scene.chunks, uploaded);
        }
    }
    pub fn ui_mut(&mut self) -> Option<&mut UiState> {
        self.ui.as_mut()
    }
    /// Windowed-proof seam: re-sync the semantic tree after the app updates
    /// F3 debug lines (lib.rs), so the next observe carries fresh nodes.
    pub fn sync_ui_nodes(&mut self) {
        if let (Some(ui), Some(content)) = (self.ui.as_mut(), self.content.as_ref()) {
            let _ = ui.synchronize(&self.inventory, content);
        }
    }
    /// Client-local Options accessors for the native renderer/input layer.
    pub fn render_distance(&self) -> u32 {
        self.render_distance
    }
    pub fn fov_deg(&self) -> f32 {
        self.fov_deg
    }
    pub fn mouse_sensitivity(&self) -> f32 {
        self.mouse_sensitivity
    }
    pub fn volume(&self) -> f32 {
        self.volume
    }
    pub fn received_block(&self, position: [i32; 3]) -> Option<u32> {
        self.predicted_blocks
            .get(&position)
            .map(|block| block.received)
            .or_else(|| self.world.block(position))
    }
    pub fn received_block_revision(&self, position: [i32; 3]) -> u64 {
        self.received_blocks
            .get(&position)
            .copied()
            .unwrap_or(0)
            .max(
                self.received_chunks
                    .get(&[position[0].div_euclid(16), position[2].div_euclid(16)])
                    .copied()
                    .unwrap_or(0),
            )
    }
    pub fn command(&mut self, command: Command) -> Result<Value> {
        match command {
            Command::Input(intent) => self.input(intent),
            Command::Connect { address, username } => self.connect(&address, &username),
            Command::Disconnect => {
                self.disconnect();
                Ok(json!({"phase":"disconnected"}))
            }
            _ => Err("capture/tasks/shutdown belong to application owner".into()),
        }
    }
    pub fn connect(&mut self, address: &str, username: &str) -> Result<Value> {
        if username.is_empty()
            || username.len() > 16
            || !username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(
                "offline username must contain 1..16 ASCII alphanumeric/underscore characters"
                    .into(),
            );
        }
        // Retain the current font/atlas owner for the loading overlay while the
        // replacement connection resources load on the bootstrap worker.
        let previous_ui = self.ui.take();
        let previous_content = self.content.take();
        let was_title = self.title_allowed;
        self.title_allowed = false;
        self.disconnect();
        self.connection_epoch += 1;
        self.dimension_epoch += 1;
        // Bootstrap failed -> fail() must return to Title; success clears it.
        self.title_allowed = was_title;
        self.client_tick = 0;
        self.phase = "loading_resources".into();
        self.error = None;
        self.address = Some(address.into());
        self.username = Some(username.into());
        self.player = Player::new();
        self.environment = Environment::new();
        self.entities.clear_connection();
        self.registries.clear();
        self.registry_tags.clear();
        self.status = None;
        self.packet_counts.clear();
        self.ignored_presentation.clear();
        self.events.clear();
        self.server_frozen = false;
        self.step_ticks = 0;
        self.content = None;
        self.ui = None;
        let (sender, receiver) = mpsc::sync_channel(1);
        let endpoint = address.to_owned();
        let username = username.to_owned();
        // Sanitized Options radius (chunks); the worker echoes it in every
        // Configuration ClientInformation instead of the old hardcoded 4.
        let view_distance = self.render_distance.clamp(
            UiState::MIN_RENDER_DISTANCE,
            UiState::MAX_RENDER_DISTANCE,
        ) as u8;
        let wake = self.wake_callback();
        let presentation = self.presentation;
        thread::Builder::new()
            .name("session-bootstrap".into())
            .spawn(move || {
                let result = (|| {
                    eprintln!("session startup: loading content");
                    let content = if presentation {
                        Content::load()?
                    } else {
                        Content::load_state()?
                    };
                    eprintln!("session startup: loading entity metadata");
                    let models = Arc::new(if presentation {
                        crate::entities::Models::load(&content)?
                    } else {
                        crate::entities::Models::load_state(&content)?
                    });
                    eprintln!("session startup: connecting network");
                    let network = Network::connect(endpoint, username, view_distance, wake.clone())?;
                    Ok(Loaded {
                        content,
                        network,
                        models,
                    })
                })();
                let _ = sender.send(result);
                wake();
            })
            .map_err(|e| e.to_string())?;
        self.loading = Some(receiver);
        self.scene = Scene::default();
        if let (Some(mut ui), Some(content)) = (previous_ui, previous_content) {
            ui.clear_session();
            ui.release_all();
            ui.clear_item_models();
            if self.presentation {
                self.install_atlas(&content);
            }
            self.content = Some(content);
            self.ui = Some(ui);
            self.refresh_scene()?;
        }
        self.changed_state();
        Ok(
            json!({"phase":"dispatched","target":address,"identity_mode":"explicit_offline","connection_epoch":self.connection_epoch.to_string()}),
        )
    }
    pub fn disconnect(&mut self) {
        self.release_local_server();
        // Preserve the pre-disconnect error (if any) for the Title screen, plus
        // the title prefill state across the content reload below. An ordinary
        // manual disconnect carries no error, so label it (the Title proof
        // asserts the reason line is shown).
        let reason = self
            .error
            .clone()
            .or_else(|| Some("Disconnected by user.".into()));
        let last = self.connection_defaults();
        let viewport = self.ui_viewport;
        self.keys.clear();
        self.commands.clear();
        self.network = None;
        self.loading = None;
        self.loader.reset();
        self.screen_relay = Default::default();
        self.screen_audio.clear();
        let content = self.content.take();
        if let Some(audio) = &self.audio {
            audio.stop();
        }
        self.phase = "disconnected".into();
        self.address = None;
        self.player = Player::new();
        self.inventory.reset();
        if let Some(ui) = &mut self.ui {
            ui.clear_session();
            ui.release_all();
            ui.clear_item_models();
        }
        self.world.clear();
        self.entities.clear_connection();
        self.predicted_blocks.clear();
        self.received_chunks.clear();
        self.received_blocks.clear();
        self.destruction.clear();
        self.block_effects.clear();
        self.block_motion.clear();
        self.dimension = None;
        if let Some(terrain) = &mut self.terrain {
            terrain.reset(&mut self.scene.chunks);
        }
        self.scene.vertices.clear();
        self.scene.indices.clear();
        self.scene.dynamic = Default::default();
        self.scene.entity_presentation = Default::default();
        self.scene.overlay = Default::default();
        self.scene.revision += 1;
        if self.title_allowed {
            // Title mode reuses the session's loaded Content: a fresh
            // Content::load() here ran on the window thread (~2 s on Linux,
            // longer on Windows) on every disconnect and Connection Lost, so
            // Windows marked the client "Not responding". The next connect
            // reloads content on its bootstrap worker anyway. Explicit
            // --server harness paths never set the flag.
            match self.install_title_ui(last.as_ref(), viewport, content) {
                Ok(_) => {
                    if let Some(reason) = reason
                        && let Some(ui) = &mut self.ui
                    {
                        ui.set_title_error(reason);
                    }
                }
                Err(error) => {
                    self.error = Some(error);
                    self.phase = "error".into();
                }
            }
        }
        self.changed_state();
    }
    fn changed_state(&mut self) {
        self.revision += 1;
        self.state_version += 1;
    }
    fn event(&mut self, value: Value) {
        if self.events.len() == 64 {
            self.events.pop_front();
        }
        self.events.push_back(value);
    }
    fn connection_defaults(&self) -> Option<(String, String)> {
        self.address.clone().zip(self.username.clone())
            .or_else(|| self.ui.as_ref().map(UiState::title_defaults))
    }
    fn fail(&mut self, error: String) {
        if let Some(report) = self.local_server_report() {
            return self.fail_local(report);
        }
        self.event(json!({"type":"incompatibility_or_transport_error","error":error,"packet_sequence":self.packet_sequence.to_string()}));
        eprintln!("session failed: {error}");
        self.error = Some(error.clone());
        self.disconnect();
        self.phase = "error".into();
        if let Some(ui) = &mut self.ui
            && self.title_allowed
        {
            ui.show_disconnected(error);
        }
        self.changed_state();
    }
    /// Title-mode UI install: `UiState` over `content` (startup passes `None`
    /// and pays one `Content::load()`), no network; entity models stay
    /// deferred until `connect()` reloads everything on success.
    fn install_title_ui(
        &mut self,
        last: Option<&(String, String)>,
        viewport: Option<(u32, u32, f64)>,
        content: Option<Content>,
    ) -> Result<Value> {
        let content = match content {
            Some(content) => content,
            None if self.presentation => Content::load()?,
            None => Content::load_state()?,
        };
        let mut ui = if self.presentation {
            UiState::new(&content)?
        } else {
            UiState::new_state(&content)?
        };
        if let Some((width, height, scale)) = viewport.or(self.ui_viewport) {
            ui.resize(width, height, scale);
        }
        match last {
            Some((server, username)) => ui.enter_title(Some(server), Some(username))?,
            None => {
                let (server, username) = ui.title_defaults();
                ui.enter_title(Some(&server), Some(&username))?;
            }
        }
        self.resource_epoch += 1;
        if self.presentation {
            self.install_atlas(&content);
        }
        self.content = Some(content);
        self.ui = Some(ui);
        self.apply_persisted_settings(crate::local_config::load_client_settings());
        Ok(json!({"phase": "disconnected", "screen": "title"}))
    }
    /// App startup entry for title mode (`--server` absent): builds the real
    /// content UI, prefills from `client.toml`/explicit values, and arms
    /// `title_allowed` so later `disconnect()`/`fail()` return here.
    pub fn ensure_title_ui(
        &mut self,
        server: Option<String>,
        username: Option<String>,
    ) -> Result<Value> {
        self.title_allowed = true;
        let viewport = self.ui_viewport;
        let last = match (&server, &username) {
            (Some(server), Some(username)) => Some((server.clone(), username.clone())),
            _ => self.ui.as_ref().map(|ui| ui.title_defaults()),
        };
        let out = self.install_title_ui(last.as_ref(), viewport, None)?;
        // Explicit prefill wins over last-used for this showing only.
        if let Some(ui) = &mut self.ui
            && (server.is_some() || username.is_some())
        {
            let (default_server, default_username) = ui.title_defaults();
            ui.enter_title(
                Some(server.as_deref().unwrap_or(&default_server)),
                Some(username.as_deref().unwrap_or(&default_username)),
            )?;
        }
        self.phase = "disconnected".into();
        self.changed_state();
        Ok(out)
    }
    fn send(&self, id: i32, payload: Writer) -> Result<()> {
        self.network
            .as_ref()
            .ok_or("not connected")?
            .send(id, payload.0)
    }
    fn send_loader(&self, outgoing: crate::loader::Outgoing) -> Result<()> {
        let id = match self.phase.as_str() {
            "configuration" => 0x02,
            "play" => 0x16,
            _ => return Err("Loader payload requires live Configuration or Play".into()),
        };
        let mut payload = Writer::default();
        payload.string(outgoing.channel);
        payload.0.extend(outgoing.payload);
        self.send(id, payload)
    }
    pub(crate) fn screen_dimension(&self) -> Option<u8> {
        match self.dimension.as_deref()? {
            "minecraft:overworld" => Some(0),
            "minecraft:the_nether" => Some(1),
            "minecraft:the_end" => Some(2),
            _ => None,
        }
    }
    pub(crate) fn register_screens(&mut self) -> Result<()> {
        if !self.screen_relay.registered && self.phase == "play" {
            self.send_loader(crate::loader::Outgoing {
                channel: "minecraft:register",
                payload: crate::computer::relay::CHANNEL.as_bytes().to_vec(),
            })?;
            self.screen_relay.registered = true;
        }
        Ok(())
    }
    /// HUD microphone indicator; `None` hides it.
    pub(crate) fn set_voice_mic(&mut self, mic: Option<(&'static str, [f32; 3])>) {
        if let Some(ui) = &mut self.ui {
            ui.set_voice_mic(mic);
        }
    }
    pub(crate) fn reset_voice(&mut self) {
        self.voice_net = Default::default();
        if let Some(audio) = &self.audio {
            audio.voice().clear();
        }
    }
    pub(crate) fn audio(&self) -> Option<&Audio> {
        self.audio.as_ref()
    }
    pub(crate) fn register_voice(&mut self) -> Result<()> {
        if !self.voice_net.registered && self.phase == "play" {
            self.send_loader(crate::loader::Outgoing {
                channel: "minecraft:register",
                payload: crate::voice::CHANNEL.as_bytes().to_vec(),
            })?;
            self.voice_net.registered = true;
        }
        Ok(())
    }
    pub(crate) fn send_voice(&self, payload: Vec<u8>) -> Result<()> {
        if self.phase != "play" || payload.len() > 32767 {
            return Err("voice payload requires live Play and bounded size".into());
        }
        self.send_loader(crate::loader::Outgoing { channel: crate::voice::CHANNEL, payload })
    }
    /// Decode a relayed voice payload; state changes are applied, audio goes to the mixer.
    pub(crate) fn receive_voice(&mut self, payload: &[u8]) -> Result<()> {
        match crate::voice::parse(payload)? {
            crate::voice::Incoming::State { enabled, range } => {
                self.voice_net.enabled = enabled;
                if let Some(audio) = &self.audio {
                    audio.voice().set_range(f32::from(range));
                }
            }
            crate::voice::Incoming::Audio { speaker, position, seq, opus } => {
                if self.voice_net.listening
                    && let Some(audio) = &self.audio
                {
                    audio.voice().push(speaker, position, None, seq, opus)?;
                }
            }
        }
        Ok(())
    }
    /// Hear one screen's Opus packet at the monitor rectangle's centre (silent beyond 32 blocks).
    pub(crate) fn play_screen_audio(&mut self, screen: crate::computer::relay::Id, seq: u32, opus: &[u8]) -> Result<()> {
        let Some(audio) = &self.audio else { return Ok(()) };
        let centre = screen.face.centre().map(|v| v as f32);
        audio.voice().push(screen.speaker(), centre, Some(crate::computer::relay::AUDIO_RANGE), seq, opus)?;
        let stats = self.screen_audio.entry(screen).or_default();
        stats.packets += 1;
        if let Some((frames, peak)) = audio.voice().speaker_stats(screen.speaker()) {
            stats.decoded_frames = frames;
            stats.peak = stats.peak.max(peak);
        }
        Ok(())
    }
    /// Observation of monitor audio: what this client captured and what it heard.
    pub(crate) fn screen_audio_observation(&self) -> Value {
        let heard: Vec<_> = self.screen_audio.iter().map(|(id, stats)| {
            json!({"block": id.face.block, "side": id.face.side, "packets": stats.packets,
                "decoded_frames": stats.decoded_frames, "peak": stats.peak})
        }).collect();
        json!({"capturing": self.screen_audio_owner.capturing, "packets_sent": self.screen_audio_owner.packets_sent,
            "unavailable": self.screen_audio_owner.unavailable, "heard": heard})
    }
    /// Deliver relayed screen audio queued by the relay to the mixer.
    pub(crate) fn pump_screen_audio(&mut self) {
        let dimension = self.screen_dimension();
        while let Some(packet) = self.screen_relay.audio.pop_front() {
            if Some(packet.screen.dimension) != dimension { continue; }
            if let Err(error) = self.play_screen_audio(packet.screen, packet.seq, &packet.opus) {
                self.event(json!({"type":"screen_audio_rejected","error":error}));
            }
        }
    }
    pub(crate) fn send_screen(&self, payload: Vec<u8>) -> Result<()> {
        if self.phase != "play" || payload.len() > 32767 {
            return Err("screen payload requires live Play and bounded size".into());
        }
        self.send_loader(crate::loader::Outgoing { channel: crate::computer::relay::CHANNEL, payload })
    }
    pub(crate) fn resize_ui(&mut self, width: u32, height: u32, scale: f64) {
        if width == 0 || height == 0 || !scale.is_finite() || scale <= 0. {
            return;
        }
        let viewport = (width, height, scale);
        if self.ui_viewport == Some(viewport) {
            return;
        }
        self.ui_viewport = Some(viewport);
        if let Some(ui) = &mut self.ui {
            ui.resize(width, height, scale);
        }
        self.changed_state();
    }

    pub(crate) fn request_loader_view(&mut self, key: &str) -> Result<Value> {
        let live = self.phase == "play" && self.network.is_some() && self.player.pose_known;
        let outgoing = self.loader.request_view(key, live)?;
        self.send_loader(outgoing)?;
        self.event(json!({"type":"loader_view_request_dispatched","view_key":key,"origin":"ordinary_client_custom_payload"}));
        Ok(
            json!({"status":"dispatched","view_key":key,"server_observed":false,"completion":"await received Loader view in minecraft_observe"}),
        )
    }
    pub(super) fn loader_action(&mut self, message: Value) -> Result<()> {
        let live = self.phase == "play"
            && self.network.is_some()
            && self.player.pose_known
            && self
                .ui
                .as_ref()
                .is_some_and(|ui| ui.screen() == crate::ui::Screen::Loader);
        let outgoing = self.loader.action(message, live)?;
        self.send_loader(outgoing)
    }
    fn flush_commands(&mut self) -> Result<()> {
        let network = self.network.as_ref().ok_or("not connected")?;
        for command in self.commands.drain(..) {
            network.send(command.id, command.payload.0)?;
        }
        Ok(())
    }
    pub fn process_events(&mut self) -> bool {
        let mut changed = self.poll_local_server();
        if let Some(result) = self.loading.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.loading = None;
            let result = result.and_then(|loaded| {
                eprintln!("session startup: activating content and UI");
                self.entities.activate_models(loaded.models.clone());
                self.player.set_visual_models(Arc::clone(&loaded.models));
                // Font/FreeType resources are owner-thread local, not transferable.
                let mut ui = if self.presentation {
                    UiState::new(&loaded.content)?
                } else {
                    UiState::new_state(&loaded.content)?
                };
                self.entities.set_text_renderer(ui.text_renderer());
                ui.set_entity_models(loaded.models);
                if let Some((width, height, scale)) = self.ui_viewport {
                    ui.resize(width, height, scale);
                }
                self.resource_epoch += 1;
                if self.presentation {
                    self.install_atlas(&loaded.content);
                }
                self.content = Some(loaded.content);
                ui.singleplayer_active = self.local_server.is_some();
                self.ui = Some(ui);
                // Re-apply persisted client-local Options onto the fresh UI/session
                // state (fresh UiState starts with an empty settings map), then
                // re-publish the persisted radius: the worker was spawned with
                // the pre-activation value (bootstrap loads before activation).
                self.apply_persisted_settings(crate::local_config::load_client_settings());
                self.network = Some(loaded.network);
                if let Some(network) = self.network.as_ref() {
                    network.set_view_distance(
                        self.render_distance
                            .clamp(
                                UiState::MIN_RENDER_DISTANCE,
                                UiState::MAX_RENDER_DISTANCE,
                            ) as u8,
                    );
                }
                if self.presentation && self.terrain.is_none() {
                    self.terrain = Some(Terrain::new(self.wake_callback())?);
                }
                self.phase = "status".into();
                // Stay in title mode across an ordinary Title Connect: a later
                // manual disconnect returns to the Title screen (the windowed
                // Title proof asserts this round-trip).
                Ok(())
            });
            if let Err(error) = result {
                self.fail(error);
            }
            self.changed_state();
            changed = true;
        }
        for index in 0..128 {
            let event = match self.network.as_ref().map(Network::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.fail("network event stream closed".into());
                    changed = true;
                    break;
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
            };
            changed = true;
            match event {
                Event::Status(status) => {
                    eprintln!("session startup: login");
                    self.status = Some(status);
                    self.phase = "login".into();
                }
                Event::Error(error) => {
                    self.fail(error);
                    break;
                }
                Event::Packet(phase, id, body) => {
                    self.packet_sequence += 1;
                    *self.packet_counts.entry(id).or_default() += 1;
                    if let Err(error) = self.apply(phase, id, &body) {
                        self.fail(format!(
                            "{phase:?} packet 0x{id:02x} sequence {}: {error}",
                            self.packet_sequence
                        ));
                        break;
                    }
                }
            }
            self.changed_state();
            if index == 127 {
                (self.wake_callback())();
            }
        }
        if self.phase == "play"
            || self
                .ui
                .as_ref()
                .is_some_and(UiState::shell_screen_active)
        {
            match self.refresh_scene() {
                Ok(value) => changed |= value,
                Err(error) => {
                    self.fail(error);
                    changed = true;
                }
            }
        }
        self.camera();
        changed
    }
    pub fn tick(&mut self) -> bool {
        self.tick_after_events(false)
    }
    /// Tick entry when the owner already drained network/loading events this
    /// frame (App.advance passes `events_drained=true` for the first tick so
    /// `process_events` + its `refresh_scene` + camera rebuild run once, not
    /// twice). Direct callers keep calling `tick()`; behavior is identical.
    /// Never skips `refresh_scene_mode(animation_only=false)` on state ticks:
    /// terrain/environment/HUD stay on the tick path, animation-only rebuilds
    /// stay in `prepare_frame`.
    pub fn tick_after_events(&mut self, events_drained: bool) -> bool {
        let events_start = Instant::now();
        let mut changed = if events_drained { false } else { self.process_events() };
        self.tick_detail_ms[0] += events_start.elapsed().as_secs_f64() * 1000.;
        if self.phase == "fixture" {
            self.client_tick += 1;
            return true;
        }
        if self.phase != "play" || !self.player.pose_known {
            return changed;
        }
        if self.server_frozen && self.step_ticks <= 0 {
            return changed;
        }
        if self.step_ticks > 0 {
            self.step_ticks -= 1;
        }
        let previous = self.client_tick;
        self.client_tick += 1;
        let result = (|| {
            let mut start = Instant::now();
            self.environment.tick(self.client_tick);
            self.environment.publish_world(&mut self.world)?;
            self.tick_detail_ms[1] += start.elapsed().as_secs_f64() * 1000.;
            start = Instant::now();
            self.player.update_environment_context(&self.world)?;
            self.tick_detail_ms[2] += start.elapsed().as_secs_f64() * 1000.;
            if self
                .world
                .chunk([
                    (self.player.position[0].floor() as i32).div_euclid(16),
                    (self.player.position[2].floor() as i32).div_euclid(16),
                ])
                .is_none()
            {
                return Ok(());
            }
            start = Instant::now();
            self.world.border.tick();
            self.tick_detail_ms[3] += start.elapsed().as_secs_f64() * 1000.;
            let content = self.content.as_ref().ok_or("content unavailable")?;
            // Block effects stay on the 20Hz game tick (visual only); kept
            // after the chunk gate exactly as before, only timed now.
            start = Instant::now();
            self.block_effects.tick(&self.world, content);
            self.tick_detail_ms[4] += start.elapsed().as_secs_f64() * 1000.;
            start = Instant::now();
            self.player.tick(
                &self.world,
                content,
                &self.inventory,
                &self.entities,
                &self.keys,
                self.client_tick,
                &mut self.commands,
            )?;
            self.tick_detail_ms[5] += start.elapsed().as_secs_f64() * 1000.;
            start = Instant::now();
            self.entities.set_look_target(
                (self.player.pose_known && self.player.game_mode != 3)
                    .then(|| self.player.eye_position()),
            );
            self.entities.tick_presentation(
                self.client_tick,
                false,
                &self.world,
                content,
                self.environment.game_time(),
            )?;
            self.tick_detail_ms[6] += start.elapsed().as_secs_f64() * 1000.;
            start = Instant::now();
            self.entities.tick_block_sources(
                &self.world,
                content,
                self.environment.game_time(),
                &[self.player.position],
                false,
            )?;
            self.tick_detail_ms[7] += start.elapsed().as_secs_f64() * 1000.;
            start = Instant::now();
            self.inventory.experience_level = self.player.experience_level;
            if let Some(ui) = &mut self.ui {
                ui.synchronize(&self.inventory, content)?;
                ui.tick(self.client_tick, 0.);
            }
            self.tick_detail_ms[8] += start.elapsed().as_secs_f64() * 1000.;
            start = Instant::now();
            self.flush_commands()?;
            self.consume_player_effects()?;
            self.animate_atlas(previous)?;
            self.tick_detail_ms[9] += start.elapsed().as_secs_f64() * 1000.;
            self.revision += 1;
            start = Instant::now();
            self.refresh_scene()?;
            self.tick_detail_ms[10] += start.elapsed().as_secs_f64() * 1000.;
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
        }
        self.camera();
        if let Some(audio) = &self.audio {
            if let Err(error) = audio.listener(
                self.scene.camera_position,
                self.player.yaw,
                self.player.pitch,
            ) {
                self.event(json!({"type":"audio_listener_error","error":error}));
            }
        }
        changed = true;
        changed
    }
    fn camera(&mut self) {
        let view = self.player.visual_view(self.presentation_partial);
        let yaw = self.player.yaw;
        self.scene.camera_position = view.eye;
        self.scene.yaw = yaw;
        self.scene.pitch = self.player.pitch;
        if self.camera_mode == 0 {
            self.scene.camera_position[0] += f64::from(yaw.to_radians().cos() * view.bob[0]);
            self.scene.camera_position[2] += f64::from(yaw.to_radians().sin() * view.bob[0]);
            self.scene.camera_position[1] += f64::from(view.bob[1]) - f64::from(view.dip);
            self.scene.pitch += view.pitch_bob;
        } else {
            let sign = if self.camera_mode == 1 { -1. } else { 1. };
            let direction = self.player.look_direction().map(|v| v * sign);
            let distance = self.content.as_ref().map_or(4., |content| {
                crate::player::camera_distance(&self.world, content, view.eye, direction)
            });
            self.scene.camera_position =
                std::array::from_fn(|i| view.eye[i] + direction[i] * distance);
            if self.camera_mode == 2 {
                self.scene.yaw += 180.;
                self.scene.pitch = -self.scene.pitch;
            }
        }
        self.scene.roll_deg = if self.camera_mode == 0 { view.roll_deg } else { 0. };
        self.scene.fov_deg = self.fov_deg * view.fov;
        self.scene.hurt_tilt_deg = self.player.hurt_tilt_deg(
            self.client_tick,
            self.presentation_partial,
            self.damage_tilt_strength,
        );
        self.scene.hurt_direction = self.player.hurt_direction;
    }

    pub(crate) fn cycle_camera(&mut self) {
        self.camera_mode = (self.camera_mode + 1) % 3;
        self.revision += 1;
        self.camera();
    }

    pub(crate) fn look_angles(&self) -> (f32, f32) {
        (self.player.yaw, self.player.pitch)
    }

    /// Per-frame UI animation: advance the frame clock and re-emit the retained
    /// overlay only while something is actually moving (hover easing, panel
    /// enter/leave, slider/switch easing). An idle menu costs nothing here.
    /// The title screen additionally sways its live backdrop camera.
    fn animate_ui(&mut self) {
        if !self.presentation {
            return;
        }
        let now = self.ui_clock.elapsed().as_secs_f64();
        let (Some(ui), Some(content)) = (self.ui.as_mut(), self.content.as_ref()) else {
            return;
        };
        if ui.synchronize(&self.inventory, content).is_err() {
            return;
        }
        let result = if ui.advance_animations(now) {
            self.scene.overlay.vertices.clear();
            for indices in &mut self.scene.overlay.indices {
                indices.clear();
            }
            ui.render(content, &self.inventory, &mut self.scene.overlay)
        } else {
            Ok(())
        };
        if self.dimension.is_none() && ui.shell_screen_active() {
            // Slow +-12 degree pan (about 40 s period) over the dimmed scene.
            self.scene.yaw = self.player.yaw + 12. * (now * 0.157).sin() as f32;
        }
        if let Err(error) = result {
            self.fail(error);
        }
    }

    /// Screens whose controls need real pointer press/release edges (slider drag).
    pub(crate) fn ui_preserves_pointer_edges(&self) -> bool {
        self.ui.as_ref().is_some_and(UiState::is_options_screen)
    }

    pub(crate) fn prepare_frame(&mut self, partial: f32) -> Result<()> {
        self.animate_ui();
        if self.presentation && self.phase == "play" && self.player.pose_known {
            self.presentation_partial = if self.server_frozen { 0. } else { partial.clamp(0., 1.) };
            self.player
                .visual_animate(self.client_tick as f64 + f64::from(self.presentation_partial));
            if let Some(terrain) = &mut self.terrain {
                let now = (self.client_tick as f64 + f64::from(self.presentation_partial)) * 0.05;
                self.block_motion.step(terrain, &self.world, now)?;
            }
            // Animated geometry samples the same tick pair as the camera.
            self.refresh_scene_mode(true)?;
        }
        Ok(())
    }
    fn activate_loader_content(&mut self) -> Result<()> {
        let content = self
            .content
            .as_mut()
            .ok_or("Loader activation requires loaded content")?;
        content
            .activate_loader_resources(self.loader.verified_paths(), self.loader.item_entries(), self.loader.block_entries())?;
        self.loader.validate_previews(content, self.presentation)?;
        self.resource_epoch += 1;
        self.player.clear_visual_resources();
        if let Some(ui) = &mut self.ui {
            ui.clear_item_models();
        }
        if self.presentation {
            let content = self
                .content
                .take()
                .ok_or("Loader content missing after activation")?;
            self.install_atlas(&content);
            self.content = Some(content);
        }
        Ok(())
    }
    fn clear_loader_content(&mut self) -> Result<()> {
        if let Some(content) = &mut self.content {
            content.clear_loader_resources()?;
        }
        self.resource_epoch += 1;
        self.player.clear_visual_resources();
        if let Some(ui) = &mut self.ui {
            ui.clear_item_models();
        }
        if self.presentation {
            if let Some(content) = self.content.take() {
                self.install_atlas(&content);
                self.content = Some(content);
            }
        }
        Ok(())
    }
    fn install_atlas(&mut self, content: &Content) {
        self.scene.texture_rgba = Some(content.rgba.clone());
        self.scene.texture_mips = Some(content.mips.clone());
        self.scene.texture_width = content.width;
        self.scene.texture_height = content.height;
        self.scene.texture_layers = content.texture_layers;
        self.scene.texture_revision = self.resource_epoch;
        self.scene.texture_patches.clear();
    }
    fn animate_atlas(&mut self, previous: u64) -> Result<()> {
        if !self.presentation {
            return Ok(());
        }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        content.animation_patches(previous, self.client_tick, &mut self.animation_patches)?;
        for patch in self.animation_patches.drain(..) {
            if let Some(old) = self.scene.texture_patches.iter_mut().find(|old| {
                old.mip_level == patch.mip_level
                    && old.layer == patch.layer
                    && old.origin == patch.origin
            }) {
                *old = patch;
            } else {
                self.scene.texture_patches.push(patch);
            }
        }
        Ok(())
    }
    fn fixture(&mut self) {
        for x in 0..10 {
            for z in 0..10 {
                fixture_cube(
                    &mut self.scene,
                    [x, -1, z],
                    if (x + z) % 2 == 0 {
                        [0.34, 0.46, 0.3]
                    } else {
                        [0.43, 0.55, 0.38]
                    },
                );
            }
        }
        for (position, color) in [
            ([3, 0, 3], [0.8, 0.38, 0.16]),
            ([5, 0, 4], [0.25, 0.42, 0.78]),
            ([5, 1, 4], [0.25, 0.42, 0.78]),
            ([7, 0, 2], [0.7, 0.65, 0.2]),
        ] {
            fixture_cube(&mut self.scene, position, color);
        }
        self.scene.revision = 1;
    }
}
fn fixture_cube(scene: &mut Scene, position: [i32; 3], color: [f32; 3]) {
    const FACES: [[[f32; 3]; 4]; 6] = [
        [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
        [[0., 1., 1.], [1., 1., 1.], [1., 1., 0.], [0., 1., 0.]],
        [[1., 0., 0.], [0., 0., 0.], [0., 1., 0.], [1., 1., 0.]],
        [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
        [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
        [[1., 0., 1.], [1., 0., 0.], [1., 1., 0.], [1., 1., 1.]],
    ];
    for (face, corners) in FACES.iter().enumerate() {
        let start = scene.vertices.len() as u32;
        for corner in corners {
            scene.vertices.push(Vertex {
                position: std::array::from_fn(|axis| position[axis] as f32 + corner[axis]),
                color,
                uv: [0., 0.],
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                alpha: 1.,
                texture_layer: 0,
                light: [-1.; 2],
                normal: match face {
                    0 => [0., -1., 0.],
                    1 => [0., 1., 0.],
                    2 => [0., 0., -1.],
                    3 => [0., 0., 1.],
                    4 => [-1., 0., 0.],
                    _ => [1., 0., 0.],
                },
            });
        }
        scene
            .indices
            .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
    }
}

#[cfg(test)]
mod title_connection_tests {
    use super::*;
    #[test]
    fn game_escape_reaches_pause_and_second_escape_resumes() {
        let mut session = Session::new_mode("escape-routing".into(), false);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.content = Some(content);
        session.phase = "play".into();
        session.input(InputIntent::Key { key: "forward".into(), pressed: true }).unwrap();
        session.input(InputIntent::Key { key: "ui_cancel".into(), pressed: true }).unwrap();
        assert_eq!(session.screen(), "pause");
        assert!(session.keys.is_empty(), "opening pause must release movement");
        session.input(InputIntent::Key { key: "ui_cancel".into(), pressed: true }).unwrap();
        assert_eq!(session.screen(), "game");
    }
    #[test]
    fn disconnect_clears_death_before_installing_shell() {
        let mut session = Session::new_mode("disconnect-death".into(), false);
        session.player.health = Some(0.);
        session.player.death_message = Some(json!({"text":"fell"}));
        session.disconnect();
        assert_eq!(session.player.health, None);
        assert_eq!(session.player.death_message, None);
    }

    #[test]
    fn settings_snapshot_preserves_gui_scale_before_ui_installation() {
        let mut session = Session::new_mode("shell-settings".into(), false);
        session.apply_persisted_settings(BTreeMap::from([("ui_scale".into(), json!(2.))]));
        assert!(session.ui.is_none());
        assert_eq!(session.client_settings_snapshot().get("ui_scale"), Some(&json!(2.)));
    }

    #[test]
    fn disconnect_prefill_uses_live_connection_not_replaced_ui_defaults() {
        let mut session = Session::new_mode("title-prefill".into(), false);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.address = Some("localhost:31415".into());
        session.username = Some("RejoinPlayer".into());
        assert_eq!(
            session.connection_defaults(),
            Some(("localhost:31415".into(), "RejoinPlayer".into()))
        );
    }

    #[test]
    fn title_disconnect_reuses_loaded_content_without_reloading() {
        // Regression: every disconnect/Connection Lost re-ran Content::load()
        // on the window thread (~2 s) and Windows ghosted the client.
        let mut session = Session::new_mode("title-reuse".into(), false);
        session.enable_native_shell();
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        let atlas = Arc::clone(&content.rgba);
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.content = Some(content);
        session.phase = "play".into();
        session.disconnect();
        assert_eq!(session.phase(), "disconnected");
        assert_eq!(session.screen(), "title");
        assert!(Arc::ptr_eq(&session.content.as_ref().unwrap().rgba, &atlas));
    }
}

#[cfg(test)]
mod camera_animation_tests {
    use super::*;

    #[test]
    fn f5_camera_cycles_without_changing_player_look_or_interaction_eye() {
        let mut session = Session::new_mode("camera-cycle".into(), false);
        session.player.position = [12., 64., -3.];
        session.player.yaw = 37.;
        session.player.pitch = 15.;
        session.camera();
        let first = session.scene.camera_position;
        let physical_eye = session.player.eye_position();
        session.cycle_camera();
        assert_eq!(session.camera_mode, 1);
        assert_ne!(session.scene.camera_position, first);
        let rear = session.scene.camera_position;
        session.cycle_camera();
        assert_eq!(session.camera_mode, 2);
        assert_eq!(session.scene.yaw, 217.);
        assert_eq!(session.scene.pitch, -15.);
        for (i, coordinate) in first.into_iter().enumerate() {
            assert!((rear[i] + session.scene.camera_position[i] - coordinate * 2.).abs() < 1e-6);
        }
        assert_eq!(session.look_angles(), (37., 15.));
        assert_eq!(session.player.eye_position(), physical_eye);
        session.cycle_camera();
        assert_eq!(session.camera_mode, 0);
        assert_eq!(session.scene.camera_position, first);
    }
}

#[cfg(test)]
mod cpu_hotpath_tests {
    use super::*;

    #[test]
    fn fixture_tick_still_advances_client_tick_and_reports_changed() {
        let mut session = Session::new_mode("cpu-fixture".into(), false);
        let tick = session.client_tick;
        assert!(session.tick(), "fixture tick must report changed");
        assert_eq!(session.client_tick, tick + 1, "block effects stay on the 20Hz tick");
    }

    #[test]
    fn frozen_server_tick_skips_game_tick_without_advancing_client_tick() {
        let mut session = Session::new_mode("cpu-frozen".into(), false);
        session.phase = "play".into();
        session.player.pose_known = true;
        session.server_frozen = true;
        session.step_ticks = 0;
        let tick = session.client_tick;
        // Frozen server: the whole 20Hz game tick (block effects included)
        // is skipped until steps arrive.
        assert!(!session.tick(), "frozen idle tick must report unchanged");
        assert_eq!(session.client_tick, tick, "frozen server must not advance game ticks");
        // Drained-first entry agrees with the direct entry when frozen.
        assert!(!session.tick_after_events(true), "drained frozen tick must also report unchanged");
        assert_eq!(session.client_tick, tick);
    }

    #[test]
    fn publish_snapshot_gate_tracks_revision_tick_and_packet_sequence() {
        let mut session = Session::new_mode("cpu-publish".into(), false);
        // Fresh session matches its initial snapshot: no publish needed.
        assert!(
            !session.observation_changed_since_publish(),
            "fresh session must not report unpublished changes"
        );
        session.revision += 1;
        assert!(
            session.observation_changed_since_publish(),
            "revision bump must require publication"
        );
        session.note_published();
        assert!(
            !session.observation_changed_since_publish(),
            "noted publication must clear the gate"
        );
        session.client_tick += 1;
        assert!(
            session.observation_changed_since_publish(),
            "client tick must require publication for wait_ticks predicates"
        );
    }

    #[test]
    fn tick_detail_accumulates_named_subsystems() {
        let mut session = Session::new_mode("cpu-detail".into(), false);
        session.phase = "play".into();
        session.player.pose_known = true;
        session.world = World::new(0, 16);
        session.environment.activate_dimension("minecraft:overworld", &json!({
            "min_y": 0, "height": 16, "has_skylight": true, "has_ceiling": false,
            "attributes": {},
        }), session.client_tick).unwrap();
        // Missing chunk short-circuits the game tick after environment work,
        // so detail must still carry those stages without a content install.
        session.tick_after_events(true);
        let detail = session.take_tick_detail();
        assert!(detail[1] > 0., "environment work must be measured");
        assert!(detail[2] > 0., "player context work must be measured");
        assert_eq!(session.take_tick_detail(), [0.; 11], "take must drain accumulators");
    }

    #[test]
    fn tick_after_events_skips_duplicate_drain_but_matches_tick() {
        // Non-play phases return the drained flag unchanged through both
        // entries; the drain-skipping path must not alter the result.
        let mut drained = Session::new_mode("cpu-drain-a".into(), false);
        drained.phase = "disconnected".into();
        let mut direct = Session::new_mode("cpu-drain-b".into(), false);
        direct.phase = "disconnected".into();
        assert_eq!(
            drained.tick_after_events(true),
            direct.tick(),
            "drain-skipped tick must match direct tick when idle"
        );
    }

    #[test]
    fn cpu_publish_snapshot_contains_current_debug_nodes_without_another_turn() {
        use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap};
        let options = crate::Options {
            automation: false, bind: "127.0.0.1:0".parse().unwrap(),
            artifacts: ".analysis/clientcpu-regression".into(), headless: Some(true),
            runtime_dir: ".analysis/clientcpu-regression/runtime".into(),
            profile: "fixture".into(), server: None, direct_join: false, username: None,
            width: 1280, height: 720, fullscreen: false,
        };
        let mut app = crate::App::new(options, Arc::new(|| {})).unwrap();
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        content.insert_test_font("minecraft:default", &json!({"providers":[]}));
        content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
        app.session.ui = Some(UiState::new(&content).unwrap());
        app.session.content = Some(content);
        app.session.phase = "play".into();
        app.session.dimension = Some("minecraft:overworld".into());
        app.session.player.pose_known = true;
        app.session.world = World::new(0, 16);
        let uniform = |size| Palette { bits: 0, values: vec![0], words: Vec::new(), size };
        for x in -1..=1 {
            for z in -1..=1 {
                app.session.world.insert([x, z], Chunk {
                    sections: vec![Arc::new(Section {
                        blocks: uniform(4096), biomes: uniform(64), non_empty_block_count: 0,
                    })],
                    sky: BTreeMap::from([(1, Arc::new(vec![0xff; 2048]))]),
                    block: BTreeMap::from([(1, Arc::new(vec![0; 2048]))]),
                    block_entities: crate::protocol::world::empty_block_entities(),
                    surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                    surface_heightmap_received: false,
                }).unwrap();
            }
        }
        app.publish_session();
        let has_debug = |app: &crate::App| {
            let observed = app.session.minecraft_observation();
            observed["ui_nodes"].as_array().unwrap().iter()
                .any(|node| node["id"] == "debug/line/0")
        };
        app.debug_overlay = true;
        app.publish_session();
        assert!(has_debug(&app), "the enabling publication itself must contain fresh nodes");
        app.publish_session_if_changed(false);
        assert!(has_debug(&app), "idle skipping must not conceal the fresh nodes");
        app.debug_overlay = false;
        app.publish_session();
        assert!(!has_debug(&app), "the disabling publication must remove old debug nodes");
    }
}

#[cfg(test)]
#[test]
fn cpu_hotpath_execution_context_is_bounded_and_preserves_fences() {
    let mut session = Session::new_mode("execution-context".into(), false);
    session.connection_epoch = 7;
    session.dimension_epoch = 8;
    session.resource_epoch = 9;
    session.revision = 10;
    session.packet_sequence = 11;
    session.client_tick = 12;
    let compact = session.execution_observation();
    let full = session.observation();
    assert_eq!(compact.connection_epoch, full.connection_epoch);
    assert_eq!(compact.dimension_epoch, full.dimension_epoch);
    assert_eq!(compact.resource_epoch, full.resource_epoch);
    assert_eq!(compact.state_revision, full.state_revision);
    assert_eq!(compact.last_applied_packet_sequence, full.last_applied_packet_sequence);
    assert_eq!(compact.client_tick, full.client_tick);
    assert_eq!(compact.data["snapshot_kind"], json!("execution_context"));
    assert_eq!(compact.data["presentation_available"], json!(false));
    assert_eq!(compact.data.as_object().unwrap().len(), 5);
    for heavy in ["entities", "inventory", "loaded_blocks", "minecraft", "terrain"] {
        assert!(compact.data.get(heavy).is_none(), "{heavy} must stay on-demand");
    }
    assert!(full.data["entities"].is_object(), "explicit full snapshots remain available");
    assert!(session.minecraft_observation().is_object(), "canonical queries still run on the real owner");
}
