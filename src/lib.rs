pub mod api;
pub mod audio;
pub mod automation;
mod content_paths;
pub mod entities;
pub mod environment;
mod far;
mod frame_trace;
mod headless;
mod hotbar_wheel;
pub mod inventory;
mod launch;
mod loader;
mod local_config;
mod minecraft;
pub mod motion;
pub mod player;
pub mod profile_textures;
pub mod protocol;
pub mod render;
pub mod session;
mod screenshot;
mod singleplayer;
pub mod terrain;
pub mod ui;
#[cfg(test)]
mod ui_hotfix_tests;

use api::{Command, InputIntent, Request, Shared, SharedState};
use automation::{Config as AutomationConfig, Server};
use render::{GraphicsPreset, Renderer};
use serde_json::json;
use session::Session;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, atomic::Ordering, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

fn toggle_fullscreen_key(session: &mut Session, code: KeyCode, pressed: bool) -> bool {
    if code != KeyCode::F11 || !pressed {
        return false;
    }
    let fullscreen = session.native_setting("fullscreen")
        .and_then(serde_json::Value::as_bool).unwrap_or(false);
    if let Err(error) = session.apply_client_setting("fullscreen", &json!(!fullscreen)) {
        eprintln!("cannot toggle fullscreen: {error}");
    }
    true
}

fn native_item_key(
    code: KeyCode,
    pressed: bool,
    modifiers: ui::Modifiers,
    inventory_open: bool,
) -> Option<InputIntent> {
    if code == KeyCode::KeyQ && !inventory_open {
        return Some(InputIntent::Key { key: "drop".into(), pressed });
    }
    if !pressed {
        return None;
    }
    if inventory_open {
        let key = match code {
            KeyCode::Digit1 => "1",
            KeyCode::Digit2 => "2",
            KeyCode::Digit3 => "3",
            KeyCode::Digit4 => "4",
            KeyCode::Digit5 => "5",
            KeyCode::Digit6 => "6",
            KeyCode::Digit7 => "7",
            KeyCode::Digit8 => "8",
            KeyCode::Digit9 => "9",
            KeyCode::KeyF => "f",
            KeyCode::KeyQ => "q",
            _ => return None,
        };
        return Some(InputIntent::Ui {
            intent: ui::UiIntent::Key { key: key.into(), pressed, modifiers },
        });
    }
    let slot = match code {
        KeyCode::Digit1 => Some(0),
        KeyCode::Digit2 => Some(1),
        KeyCode::Digit3 => Some(2),
        KeyCode::Digit4 => Some(3),
        KeyCode::Digit5 => Some(4),
        KeyCode::Digit6 => Some(5),
        KeyCode::Digit7 => Some(6),
        KeyCode::Digit8 => Some(7),
        KeyCode::Digit9 => Some(8),
        _ => None,
    };
    if let Some(slot) = slot {
        return Some(InputIntent::Hotbar { slot });
    }
    None
}

struct Options {
    automation: bool,
    bind: std::net::SocketAddr,
    artifacts: PathBuf,
    headless: Option<bool>,
    runtime_dir: PathBuf,
    profile: String,
    server: Option<String>,
    direct_join: bool,
    username: Option<String>,
    width: u32,
    height: u32,
    fullscreen: bool,
}

/// Resolves an artifact/runtime directory: absolute paths as given, relative ones
/// under the client's data root (`--data-dir`, `$XDG_DATA_HOME/solaris-client`,
/// `~/.local/share/solaris-client`). Never a compile-time path: release builds run
/// on machines without the build checkout.
fn artifact_root(path: &Path, data_root: &Path) -> Result<PathBuf, String> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("artifact_dir_parent_traversal".into());
    }
    let requested = if path.is_absolute() {
        path.to_owned()
    } else {
        data_root.join(path)
    };
    std::fs::create_dir_all(&requested).map_err(|e| format!("{}: {e}", requested.display()))?;
    requested.canonicalize().map_err(|e| e.to_string())
}

pub fn cli(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|arg| arg.starts_with('-') && arg != "--help" && arg != "-h") {
        return launch::play(args);
    }
    match args.next().as_deref() {
        Some("doctor") => {
            let json_output = match args.next().as_deref() {
                Some("--json") => true,
                None => false,
                _ => return Err("usage: solaris-client doctor [--json]".into()),
            };
            if args.next().is_some() {
                return Err("unexpected_doctor_argument".into());
            }
            let evidence = render::doctor()?;
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&evidence).map_err(|e| e.to_string())?
                );
            } else {
                println!(
                    "Solaris GPU doctor\n{}",
                    serde_json::to_string_pretty(&evidence).map_err(|e| e.to_string())?
                );
            }
            Ok(())
        }
        Some("mcp") => {
            if args.next().as_deref() != Some("attach") {
                return Err(
                    "usage: solaris-client mcp attach --instance ID --stdio [--descriptor FILE]"
                        .into(),
                );
            }
            let mut descriptor = None;
            let mut instance = None;
            let mut stdio = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--stdio" => stdio = true,
                    "--descriptor" => {
                        descriptor = Some(PathBuf::from(args.next().ok_or("missing_descriptor")?))
                    }
                    "--instance" => instance = Some(args.next().ok_or("missing_instance")?),
                    _ => return Err(format!("unknown_mcp_argument: {arg}")),
                }
            }
            if !stdio {
                return Err("mcp_attach_requires_stdio".into());
            }
            let path = if let Some(path) = descriptor {
                path
            } else {
                let id = instance.ok_or("mcp_attach_requires_instance_or_descriptor")?;
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return Err("invalid_instance_id".into());
                }
                content_paths::default_root()?
                    .join("runtime")
                    .join(format!("{id}.json"))
            };
            automation::attach_stdio(&path)
        }
        Some("play") | None => launch::play(args),
        Some("setup") => launch::setup(args),
        Some("--help") | Some("-h") => {
            launch::print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown_command: {command}")),
    }
}

/// Frame scheduling: with vsync the FIFO swapchain paces frames at the
/// display refresh (a CPU cap below it repeats frames on some refreshes, e.g.
/// 120 fps on a 144 Hz panel), so `max_fps` only caps unsynced presentation.
fn frame_interval(vsync: bool, max_fps: u64) -> Duration {
    match (vsync, max_fps) {
        (true, _) | (false, 0) => Duration::ZERO,
        (false, fps) => Duration::from_secs_f64(1. / fps as f64),
    }
}

fn frame_deadline(timer_paced: bool, next_tick: Instant, next_frame: Instant) -> Instant {
    if timer_paced { next_tick.min(next_frame) } else { next_tick }
}

/// Pointer capture follows gameplay input on every owner turn, not only on
/// screen transitions: grab whenever the game owns input and no grab is held
/// (a failed X11 grab is retried next turn), release as soon as it does not.
/// `Some(true)` grab, `Some(false)` release, `None` keep.
fn pointer_grab_change(game_input: bool, grabbed: bool) -> Option<bool> {
    (game_input != grabbed).then_some(game_input)
}

fn owns_native_text(chat: bool, shell: bool, dialog: bool, field: bool) -> bool {
    chat || shell || dialog || field
}

struct App {
    options: Options,
    session: Session,
    shared: SharedState,
    sender: mpsc::SyncSender<Request>,
    requests: mpsc::Receiver<Request>,
    automation: Option<Server>,
    controller_revision: u64,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    focused: bool,
    occluded: bool,
    cursor_locked: bool,
    last_cursor: Option<(f64, f64)>,
    input_only_wakeup: bool,
    pointer_inside: bool,
    debug_overlay: bool,
    shift_held: bool,
    native_modifiers: crate::ui::Modifiers,
    ime_composing: bool,
    hotbar_wheel: hotbar_wheel::HotbarWheel,
    next_tick: Instant,
    tick_interval: Duration,
    next_frame: Instant,
    advance_cpu_ms: f64,
    drain_cpu_ms: f64,
    publish_cpu_ms: f64,
    tasks_cpu_ms: f64,
    sync_cpu_ms: f64,
    tick_cpu_ms: f64,
    events_cpu_ms: f64,
    publish_skipped: u64,
    frame_trace: Option<frame_trace::FrameTrace>,
    look_events: u32,
    captured_frame: Option<String>,
    pending_capture: Option<Request>,
    screenshot_replies: Vec<(PathBuf, mpsc::Receiver<Result<serde_json::Value, String>>)>,
    screenshot_captures: std::collections::VecDeque<Request>,
    pending_tasks: Vec<(Request, minecraft::Task)>,
    shutdown: bool,
    error: Option<String>,
}

impl App {
    fn new(options: Options, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let instance = format!(
            "native-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        );
        let presentation = !options
            .headless
            .unwrap_or(options.profile == "offline-test" && options.automation);
        let mut session = Session::new_mode(instance, presentation);
        session.apply_persisted_settings(crate::local_config::load_client_settings());
        if options.fullscreen {
            session.apply_client_setting("fullscreen", &json!(true))?;
        }
        session.set_wake(wake.clone());
        let tick_interval = session.tick_interval();
        let shared = Arc::new(Shared {
            observation: Mutex::new(session.execution_observation()),
            changed: Condvar::new(),
            owner_wake: wake,
        });
        let (sender, requests) = mpsc::sync_channel(256);
        Ok(Self {
            options,
            session,
            shared,
            sender,
            requests,
            automation: None,
            controller_revision: u64::MAX,
            window: None,
            renderer: None,
            focused: false,
            occluded: false,
            cursor_locked: false,
            last_cursor: None,
            input_only_wakeup: false,
            pointer_inside: false,
            debug_overlay: false,
            shift_held: false,
            native_modifiers: crate::ui::Modifiers::default(),
            ime_composing: false,
            hotbar_wheel: hotbar_wheel::HotbarWheel::default(),
            next_tick: Instant::now(),
            tick_interval,
            next_frame: Instant::now(),
            advance_cpu_ms: 0.,
            drain_cpu_ms: 0.,
            publish_cpu_ms: 0.,
            tasks_cpu_ms: 0.,
            sync_cpu_ms: 0.,
            tick_cpu_ms: 0.,
            events_cpu_ms: 0.,
            publish_skipped: 0,
            frame_trace: frame_trace::FrameTrace::from_env(),
            look_events: 0,
            captured_frame: None,
            pending_capture: None,
            screenshot_replies: Vec::new(),
            screenshot_captures: std::collections::VecDeque::new(),
            pending_tasks: Vec::new(),
            shutdown: false,
            error: None,
        })
    }

    fn publish_session(&mut self) {
        let publish_start = Instant::now();
        // Synchronize derived UI state before building the snapshot. An idle
        // publication gate cannot rely on another turn to publish these nodes.
        if self.debug_overlay {
            let render = self.shared.observation.lock().unwrap_or_else(|e| e.into_inner())
                .data["render"].clone();
            let lines = self.session.debug_lines(&render);
            if let Some(ui) = self.session.ui_mut() {
                ui.set_debug_lines(lines);
            }
        } else if let Some(ui) = self.session.ui_mut() {
            ui.set_debug_lines(Vec::new());
        }
        self.session.sync_ui_nodes();
        self.session.note_published();
        let mut observation = self.session.execution_observation();
        observation.captured_frame_id = self.captured_frame.clone();
        observation.data["window"] = self.window_state();
        observation.data["profile"] = json!(self.options.profile);
        observation.data["debug_overlay"] = json!(self.debug_overlay);
        let mut shared = self
            .shared
            .observation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(renderer) = &self.renderer {
            observation.data["render"] = shared.data["render"].take();
            renderer.update_metrics(&mut observation.data["render"]);
        }
        *shared = observation;
        self.shared.changed.notify_all();
        self.publish_cpu_ms += publish_start.elapsed().as_secs_f64() * 1000.;
    }
    /// Change-gated owner publication. Every mutation path already publishes
    /// through `drain_requests` (input/connect/capture/minecraft tools), and
    /// `Session::tick/process_events` report `changed`; rebuilding the
    /// ~10k-node observation on an idle turn is pure waste, so skip it and let
    /// the shared `Condvar` (and automation `state_version`/`wait_*` polls)
    /// observe only real state changes.
    fn publish_session_if_changed(&mut self, changed: bool) {
        if changed || self.session.observation_changed_since_publish() {
            self.publish_session();
        } else {
            self.publish_skipped += 1;
        }
    }
    fn window_state(&self) -> serde_json::Value {
        let size = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1280, 720));
        json!({"focused":self.focused,"occluded":self.occluded,"width":size.width,"height":size.height,
            "cursor_locked":self.cursor_locked,"pointer_inside":self.pointer_inside,
            "native":self.window.is_some(),"offscreen":self.window.is_none(),
            "renderable":self.renderer.is_some()&&size.width>0&&size.height>0&&!self.occluded})
    }
    fn release(&mut self, reason: &str) {
        let tasks = std::mem::take(&mut self.pending_tasks);
        for (request, mut task) in tasks {
            if request.automation_fence.is_some() {
                let _ = task.cancel(&mut self.session);
                let _ = request.reply.send(Err(format!("cancelled: {reason}")));
            } else {
                self.pending_tasks.push((request, task));
            }
        }
        let _ = self.session.input(InputIntent::ReleaseAll);
        if let Some(server) = &self.automation {
            server.revoke_control_owner(reason);
        }
        self.cursor_locked = false;
        if let Some(window) = &self.window {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.publish_session();
    }
    fn capture_screenshot(&mut self) {
        if self.screenshot_replies.len() >= 16 {
            self.screenshot_notice(Err("screenshot queue full".into()));
            return;
        }
        let stamp = match screenshot::timestamp() {
            Ok(stamp) => stamp,
            Err(error) => {
                self.screenshot_notice(Err(error));
                return;
            }
        };
        let path = screenshot::path(&self.options.artifacts, &stamp,
            self.screenshot_replies.iter().map(|(path, _)| path));
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let (reply, receive) = mpsc::sync_channel(1);
        self.screenshot_captures.push_back(Request {
            command: Command::Capture { name, destination: Some(path.clone()) },
            reply,
            expected_connection_epoch: None,
            expected_dimension_epoch: None,
            automation_fence: None,
            cancelled: None,
        });
        self.screenshot_replies.push((path, receive));
    }
    /// Physical F2 requests wait for the renderer; automation captures retain
    /// their existing immediate/busy contract.
    fn schedule_screenshot(&mut self, renderer_ready: bool) {
        if renderer_ready && self.pending_capture.is_none()
            && let Some(request) = self.screenshot_captures.pop_front() {
            self.pending_capture = Some(request);
            self.next_frame = Instant::now();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
    fn screenshot_notice(&mut self, result: Result<serde_json::Value, String>) {
        let message = screenshot::message(&result);
        if result.is_err() {
            eprintln!("{message}");
        }
        if let Some(ui) = self.session.ui_mut()
            && let Err(error) = ui.local_message(&message) {
            eprintln!("screenshot chat: {error}");
        }
        self.publish_session();
    }
    fn manual(&mut self, intent: InputIntent) {
        // Raw look events arrive at mouse polling frequency. Session input
        // records their revision; the next tick/frame publishes it once.
        let defer_publication = matches!(intent, InputIntent::Look { .. });
        if self
            .automation
            .as_ref()
            .is_some_and(|server| server.revoke_control_owner("manual_input"))
        {
            let _ = self.session.input(InputIntent::ReleaseAll);
        }
        let old_screen = self.session.screen();
        if let Err(e) = self.session.input(intent) {
            eprintln!("input: {e}");
        }
        if old_screen != self.session.screen() {
            self.release("screen_transition");
        } else if !defer_publication {
            self.publish_session();
        }
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        eprintln!("client failed: {error}");
        self.release("client_error");
        self.error = Some(error);
        event_loop.exit();
    }
    fn fence(&self, request: &Request) -> Result<(), String> {
        if request
            .cancelled
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Acquire))
        {
            return Err("cancelled".into());
        }
        if request
            .automation_fence
            .as_ref()
            .is_some_and(|fence| !fence.load(Ordering::Acquire))
        {
            return Err("controller_lease_revoked".into());
        }
        let observation = self
            .shared
            .observation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if request
            .expected_connection_epoch
            .as_ref()
            .is_some_and(|epoch| epoch != &observation.connection_epoch)
        {
            return Err("stale_connection_epoch".into());
        }
        if request
            .expected_dimension_epoch
            .as_ref()
            .is_some_and(|epoch| epoch != &observation.dimension_epoch)
        {
            return Err("stale_dimension_epoch".into());
        }
        Ok(())
    }
    fn drain_requests(&mut self) {
        for _ in 0..64 {
            let Ok(request) = self.requests.try_recv() else {
                break;
            };
            if let Err(error) = self.fence(&request) {
                let _ = request.reply.send(Err(error));
                continue;
            }
            match &request.command {
                Command::Capture { .. } => {
                    if self.pending_capture.is_some() {
                        let _ = request.reply.send(Err("capture_busy".into()));
                    } else if self.occluded
                        || self.renderer.is_none()
                        || self.window.as_ref().is_some_and(|w| {
                            w.inner_size().width == 0 || w.inner_size().height == 0
                        })
                    {
                        let _ = request
                            .reply
                            .send(Err("render_target_not_renderable".into()));
                    } else {
                        self.pending_capture = Some(request);
                        self.next_frame = Instant::now();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
                Command::Minecraft { name, arguments }
                    if name == "minecraft_toggle_debug_overlay" =>
                {
                    // F3 overlay flag lives on the App owner (same bool the
                    // physical F3 key flips). Apply + publish so the next
                    // observe carries fresh debug/line/* nodes, then report
                    // the session-part lines immediately for the assertion.
                    let enabled = arguments
                        .get("enabled")
                        .and_then(serde_json::Value::as_bool);
                    match enabled {
                        Some(enabled) => {
                            self.debug_overlay = enabled;
                            self.session.debug_overlay_pending = enabled;
                            self.publish_session();
                            let lines = self.session.debug_overlay_lines_for_mcp();
                            let _ = request.reply.send(Ok(json!({"status": "ok",
                                "debug_overlay": enabled, "lines": lines})));
                        }
                        None => {
                            let _ = request.reply.send(Err("enabled must be a boolean".into()));
                        }
                    }
                }
                Command::Minecraft { name, arguments } => {
                    if request.automation_fence.is_some()
                        && name != "minecraft_disconnect"
                        && self
                            .pending_tasks
                            .iter()
                            .any(|(pending, _)| pending.automation_fence.is_some())
                    {
                        let _ = request.reply.send(Err("writer_action_busy".into()));
                        continue;
                    }
                    if name == "minecraft_connect" && self.options.profile != "offline-test" {
                        let _ = request
                            .reply
                            .send(Err("connect_requires_explicit_offline-test_profile".into()));
                        continue;
                    }
                    match minecraft::start(&mut self.session, name, arguments) {
                        Ok(minecraft::Outcome::Complete(value)) => {
                            let _ = request.reply.send(Ok(value));
                        }
                        Ok(minecraft::Outcome::Pending(task)) => {
                            self.pending_tasks.push((request, task))
                        }
                        Err(error) => {
                            let _ = request.reply.send(Err(error));
                        }
                    }
                    self.publish_session();
                }
                Command::Shutdown => {
                    self.release("shutdown");
                    let _ = self.session.command(Command::Disconnect);
                    let _ = request.reply.send(Ok(json!({"shutdown":"requested"})));
                    self.shutdown = true;
                }
                Command::Input(intent)
                    if !self.focused && !matches!(intent, InputIntent::ReleaseAll) =>
                {
                    let _ = request.reply.send(Err("window_not_focused".into()));
                }
                Command::Connect { .. } if self.options.profile != "offline-test" => {
                    let _ = request
                        .reply
                        .send(Err("connect_requires_explicit_offline-test_profile".into()));
                }
                _ => {
                    let result = self.session.command(request.command);
                    self.publish_session();
                    let _ = request.reply.send(result);
                }
            }
        }
    }

    fn complete_tasks(&mut self) {
        let tasks = std::mem::take(&mut self.pending_tasks);
        for (request, mut task) in tasks {
            if let Err(error) = self.fence(&request) {
                let _ = task.cancel(&mut self.session);
                let _ = request.reply.send(Err(error));
                continue;
            }
            match task.on_event(&mut self.session) {
                Ok(Some(value)) => {
                    let _ = request.reply.send(Ok(value));
                }
                Ok(None) => self.pending_tasks.push((request, task)),
                Err(error) => {
                    let _ = task.cancel(&mut self.session);
                    let _ = request.reply.send(Err(error));
                }
            }
        }
        self.publish_session();
    }

    fn start_automation(&mut self) -> Result<(), String> {
        if self.options.automation {
            let server = automation::start(
                self.shared.clone(),
                self.sender.clone(),
                AutomationConfig {
                    bind: self.options.bind,
                    runtime_dir: self.options.runtime_dir.clone(),
                    artifact_dir: self.options.artifacts.clone(),
                    profile: self.options.profile.clone(),
                    allow_control: true,
                },
            )?;
            eprintln!(
                "automation descriptor: {}",
                server.descriptor_path().display()
            );
            self.automation = Some(server);
        }
        self.session.enable_native_shell();
        if self.options.direct_join {
            let address = self.options.server.as_ref().ok_or("missing_server")?;
            self.session
                .connect(address, self.options.username.as_ref().unwrap())?;
            self.publish_session();
        } else {
            // Without an explicit --server, show the shell. client.toml only
            // prefills Direct Connect; it no longer bypasses the title screen.
            let server = self.options.server.clone();
            let username = self.options.username.clone();
            self.session.ensure_title_ui(server, username)?;
            self.publish_session();
        }
        Ok(())
    }

    fn draw(&mut self) -> Result<(), String> {
        let draw_start = Instant::now();
        let capture = self.pending_capture.take();
        let capture = match capture {
            Some(request) => match self.fence(&request) {
                Ok(()) => Some(request),
                Err(error) => {
                    let _ = request.reply.send(Err(error));
                    None
                }
            },
            None => None,
        };
        let remaining = self.next_tick.saturating_duration_since(Instant::now()).as_secs_f32();
        let prepare_start = Instant::now();
        self.session.prepare_frame(
            (1. - remaining / self.tick_interval.as_secs_f32()).clamp(0., 1.),
        )?;
        let prepare_ms = prepare_start.elapsed().as_secs_f64() * 1000.;
        if let Some(renderer) = &mut self.renderer {
            let mut shared = self
                .shared
                .observation
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if capture.is_some() {
                shared.data["terrain_capture"] = self.session.capture_terrain_metrics();
            }
            renderer.render(self.session.scene(), capture, &shared)?;
            self.session.release_uploaded_terrain(renderer.drain_uploaded_meshes());
            self.session.far_generated(renderer.far_gpu_generation(), renderer.drain_far_generated());
            renderer.update_metrics(&mut shared.data["render"]);
            let epochs = self.session.context_epochs();
            if renderer.spawn_meshes_presented(
                self.session.scene(), epochs, self.session.spawn_meshes_ready(),
            ) {
                self.session.note_renderer_scene_confirmation(
                    epochs.0, epochs.1, self.session.scene().revision,
                );
            }
            self.shared.changed.notify_all();
            let render = &mut shared.data["render"];
            render["cpu_session_prepare_frame_ms"] = json!(prepare_ms);
            render["cpu_client_draw_ms"] = json!(draw_start.elapsed().as_secs_f64() * 1000.);
            render["cpu_client_advance_ms"] = json!(self.advance_cpu_ms);
            render["cpu_client_drain_ms"] = json!(self.drain_cpu_ms);
            render["cpu_client_publish_ms"] = json!(self.publish_cpu_ms);
            render["cpu_client_tasks_ms"] = json!(self.tasks_cpu_ms);
            render["cpu_client_sync_window_ms"] = json!(self.sync_cpu_ms);
            render["client_publish_skipped"] = json!(self.publish_skipped);
            render["cpu_session_tick_ms"] = json!(self.tick_cpu_ms);
            render["cpu_session_events_ms"] = json!(self.events_cpu_ms);
            for (key, value) in session::TICK_DETAIL_NAMES.iter().zip(self.session.take_tick_detail()) {
                render["cpu_session_tick_detail_ms"][*key] = json!(value);
            }
            if let Some(trace) = &mut self.frame_trace {
                trace.record(render, self.look_events);
            }
            self.look_events = 0;
            self.advance_cpu_ms = 0.;
            self.drain_cpu_ms = 0.;
            self.publish_cpu_ms = 0.;
            self.tasks_cpu_ms = 0.;
            self.sync_cpu_ms = 0.;
            self.tick_cpu_ms = 0.;
            self.events_cpu_ms = 0.;
        }
        Ok(())
    }

    fn advance(&mut self) {
        let advance_start = Instant::now();
        let drain_start = Instant::now();
        self.drain_requests();
        self.drain_cpu_ms += drain_start.elapsed().as_secs_f64() * 1000.;
        let mut changed = false;
        if let Some(renderer) = &mut self.renderer {
            if let Some(frame) = renderer.poll_capture() {
                self.captured_frame = Some(frame.to_string());
                // A fresh GPU frame id mutates the published observation.
                changed = true;
            }
        }
        let mut index = 0;
        while index < self.screenshot_replies.len() {
            let result = match self.screenshot_replies[index].1.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => { index += 1; continue; }
                Err(mpsc::TryRecvError::Disconnected) => Err("capture reply channel closed".into()),
            };
            self.screenshot_replies.swap_remove(index);
            self.screenshot_notice(result);
            changed = true;
        }
        self.schedule_screenshot(self.renderer.as_ref().is_some_and(Renderer::capture_ready));
        // tick() drains events itself; calling process_events() here first
        // would rebuild the scene twice per frame (baseline: pure waste when
        // both return unchanged). Pass the already-drained flag in so the
        // first tick skips its duplicate drain.
        let mut events_drained = false;
        if self.next_tick > Instant::now() {
            let events_start = Instant::now();
            changed |= self.session.process_events();
            self.events_cpu_ms += events_start.elapsed().as_secs_f64() * 1000.;
            events_drained = true;
        }
        let now = Instant::now();
        let interval = self.session.tick_interval();
        if interval != self.tick_interval {
            self.tick_interval = interval;
            self.next_tick = now + interval;
        }
        let mut advanced = 0;
        while now >= self.next_tick && advanced < 4 {
            let tick_start = Instant::now();
            changed |= self.session.tick_after_events(events_drained && advanced == 0);
            self.tick_cpu_ms += tick_start.elapsed().as_secs_f64() * 1000.;
            self.next_tick += self.tick_interval;
            advanced += 1;
        }
        if now >= self.next_tick {
            self.next_tick = now + self.tick_interval;
        }
        // No task can have changed state after the publication below when
        // the queue is empty; avoid rebuilding the entire observation twice.
        if self.pending_tasks.is_empty() {
            self.publish_session_if_changed(changed);
        } else {
            let tasks_start = Instant::now();
            self.complete_tasks();
            self.tasks_cpu_ms += tasks_start.elapsed().as_secs_f64() * 1000.;
        }
        self.shutdown |= self.session.take_shutdown_request();
        let sync_start = Instant::now();
        self.sync_native_window();
        self.sync_cpu_ms += sync_start.elapsed().as_secs_f64() * 1000.;
        self.advance_cpu_ms += advance_start.elapsed().as_secs_f64() * 1000.;
    }

    fn frame_interval(&self) -> Duration {
        frame_interval(
            self.session.native_setting("vsync").and_then(serde_json::Value::as_bool).unwrap_or(true),
            self.session.native_setting("max_fps").and_then(serde_json::Value::as_u64).unwrap_or(120),
        )
    }

    fn sync_native_window(&mut self) {
        let Some(window) = &self.window else { return };
        let fullscreen = self.session.native_setting("fullscreen").and_then(serde_json::Value::as_bool).unwrap_or(false);
        if fullscreen != window.fullscreen().is_some() {
            window.set_fullscreen(fullscreen.then_some(winit::window::Fullscreen::Borderless(None)));
        }
        if let Some(ui) = self.session.ui_mut() {
            let ime = ui.ime_state();
            window.set_ime_allowed(ime.allowed);
            if ime.allowed {
                window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(ime.position[0], ime.position[1]),
                    winit::dpi::PhysicalSize::new(ime.size[0], ime.size[1]),
                );
            }
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.set_vsync(self.session.native_setting("vsync").and_then(serde_json::Value::as_bool).unwrap_or(true));
            renderer.set_graphics_preset(self.session.native_setting("graphics_preset").and_then(GraphicsPreset::from_setting).unwrap_or_default());
        }
        let game_input = self.focused && !self.occluded && self.session.phase() == "play"
            && self.session.screen() == "game" && !self.session.chat_open()
            && !self.session.dialog_screen_active();
        match pointer_grab_change(game_input, self.cursor_locked) {
            Some(false) => {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
                self.cursor_locked = false;
            }
            Some(true)
                if window.set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined)).is_ok() =>
            {
                window.set_cursor_visible(false);
                self.cursor_locked = true;
            }
            _ => {}
        }
    }

    fn deadline(&self) -> Instant {
        // Presentation-paced frames are woken by the window/compositor, not
        // a past WaitUntil deadline (which would busy-spin on Wayland).
        // A hidden/zero-size surface has no frame timer either.
        let renderable = !self.occluded && self.window.as_ref().is_some_and(|window| {
            let size = window.inner_size();
            size.width > 0 && size.height > 0
        });
        self.pending_tasks
            .iter()
            .map(|(_, task)| task.deadline())
            .fold(frame_deadline(
                renderable && !self.frame_interval().is_zero(),
                self.next_tick,
                self.next_frame,
            ), Instant::min)
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let title = if self.options.automation {
            "Solaris Native Client — AUTOMATION ENABLED"
        } else {
            "Solaris Native Client"
        };
        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title(title)
                .with_inner_size(winit::dpi::PhysicalSize::new(self.options.width, self.options.height))
                .with_fullscreen(self.options.fullscreen.then_some(winit::window::Fullscreen::Borderless(None))),
        ) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                self.fail(event_loop, e.to_string());
                return;
            }
        };
        self.focused = window.has_focus();
        let size = window.inner_size();
        self.session
            .resize_ui(size.width, size.height, window.scale_factor());
        let renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            self.options.artifacts.clone(),
        )) {
            Ok(renderer) => renderer,
            Err(e) => {
                self.fail(event_loop, format!("renderer_initialization_failed: {e}"));
                return;
            }
        };
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.renderer
            .as_mut()
            .unwrap()
            .set_wake(self.shared.owner_wake.clone());
        self.publish_session();
        if let Err(error) = self.start_automation() {
            self.fail(event_loop, error);
            return;
        }
        self.window.as_ref().unwrap().request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        self.input_only_wakeup = false;
        if self.window.as_ref().is_none_or(|w| w.id() != window_id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.release("window_close");
                let _ = self.session.command(Command::Disconnect);
                event_loop.exit();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if !focused {
                    self.release("focus_lost");
                    self.hotbar_wheel.reset();
                    self.native_modifiers = crate::ui::Modifiers::default();
                    self.shift_held = false;
                    self.ime_composing = false;
                } else {
                    self.publish_session();
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if occluded {
                    self.release("window_occluded");
                } else {
                    self.publish_session();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size);
                }
                if let Some(window) = &self.window {
                    self.session
                        .resize_ui(size.width, size.height, window.scale_factor());
                }
                self.publish_session();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    self.session
                        .resize_ui(size.width, size.height, scale_factor);
                }
                self.publish_session();
            }
            WindowEvent::CursorEntered { .. } | WindowEvent::CursorLeft { .. } => {
                self.pointer_inside = matches!(event, WindowEvent::CursorEntered { .. });
                self.publish_session();
            }
            WindowEvent::CursorMoved { position, .. } if self.focused => {
                // Track logical cursor for click synthesis and container drag;
                // gameplay keeps raw mouse-look capture.
                self.last_cursor = Some((position.x, position.y));
                if self.session.dialog_screen_active() || self.session.screen() == "inventory" {
                    let _ = self.session.command(Command::Input(InputIntent::Ui {
                        intent: crate::ui::UiIntent::Pointer {
                            phase: crate::ui::PointerPhase::Move,
                            x: position.x,
                            y: position.y,
                            button: None,
                            modifiers: crate::ui::Modifiers {
                                shift: self.shift_held,
                                ..Default::default()
                            },
                        },
                    }));
                    self.publish_session();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.native_modifiers = crate::ui::Modifiers {
                    shift: state.shift_key(), control: state.control_key(),
                    alt: state.alt_key(), logo: state.super_key(),
                };
                self.shift_held = state.shift_key();
            }
            WindowEvent::Ime(ime) if self.focused && self.session.text_field_focused() => {
                match ime {
                    winit::event::Ime::Preedit(text, cursor) => {
                        self.ime_composing = !text.is_empty();
                        self.manual(InputIntent::Ui { intent: crate::ui::UiIntent::Composition {
                            text, cursor: cursor.map(|(a, b)| [a, b]),
                        }});
                    }
                    winit::event::Ime::Commit(text) => {
                        self.ime_composing = false;
                        self.manual(InputIntent::Ui { intent: crate::ui::UiIntent::Text { text } });
                    }
                    winit::event::Ime::Disabled => self.ime_composing = false,
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } if self.focused
                && (!event.repeat || self.session.text_field_focused()) => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                if toggle_fullscreen_key(&mut self.session, code, pressed) {
                    self.sync_native_window();
                    self.publish_session();
                    return;
                }
                if code == KeyCode::F5 && pressed && self.session.screen() == "game" {
                    self.session.cycle_camera();
                    return;
                }
                if matches!(code, KeyCode::ShiftLeft | KeyCode::ShiftRight) {
                    self.shift_held = pressed;
                }
                // Shift is a container modifier, not a movement input, while a menu is open.
                if self.session.screen() == "inventory"
                    && matches!(code, KeyCode::ShiftLeft | KeyCode::ShiftRight)
                {
                    return;
                }
                // Dialog screens own Escape/Enter/Tab: forward to UI (fail-closed
                // emergency release still fires first so held inputs never stick).
                if self.session.dialog_screen_active()
                    && !self.session.shell_screen_active()
                    && !self.session.server_dialog_open()
                {
                    if code == KeyCode::Escape && pressed {
                        self.release("emergency_stop");
                    }
                    let key = match code {
                        KeyCode::Escape => Some("ui_cancel"),
                        KeyCode::Enter | KeyCode::NumpadEnter => Some("ui_confirm"),
                        KeyCode::Tab => Some("ui_focus_next"),
                        _ => None,
                    };
                    if let Some(key) = key {
                        if pressed {
                            self.manual(InputIntent::Key {
                                key: key.into(),
                                pressed: true,
                            });
                        }
                        return;
                    }
                    // Non-UI keys on a dialog screen: swallow (no movement while dead).
                    return;
                }
                // Chat overlay owns text input while open: forward printable text
                // (KeyEvent.text, not IME) plus editing/navigation keys to the
                // focused field. Escape closes chat; Enter submits (Game<-Chat).
                // The Title screen reuses the same path for its server/username
                // fields (chat is never open on Title; Escape there is a noop).
                if owns_native_text(
                    self.session.chat_open(),
                    self.session.shell_screen_active(),
                    self.session.server_dialog_open(),
                    self.session.text_field_focused(),
                )
                {
                    // The platform IME owns candidate confirmation/navigation.
                    if self.ime_composing {
                        return;
                    }
                    if code == KeyCode::Escape && pressed {
                        self.manual(InputIntent::Ui {
                            intent: crate::ui::UiIntent::Key {
                                key: "Escape".into(),
                                pressed: true,
                                modifiers: crate::ui::Modifiers::default(),
                            },
                        });
                        return;
                    }
                    let key = match code {
                        KeyCode::KeyA if (self.native_modifiers.control || self.native_modifiers.logo)
                            && !self.native_modifiers.alt => Some("a"),
                        KeyCode::Enter | KeyCode::NumpadEnter => Some("Enter"),
                        KeyCode::Tab => Some("Tab"),
                        KeyCode::Backspace => Some("Backspace"),
                        KeyCode::Delete => Some("Delete"),
                        KeyCode::ArrowLeft => Some("ArrowLeft"),
                        KeyCode::ArrowRight => Some("ArrowRight"),
                        KeyCode::ArrowUp => Some("ArrowUp"),
                        KeyCode::ArrowDown => Some("ArrowDown"),
                        KeyCode::Home => Some("Home"),
                        KeyCode::End => Some("End"),
                        _ => None,
                    };
                    if let Some(key) = key {
                        if pressed {
                            self.manual(InputIntent::Ui {
                                intent: crate::ui::UiIntent::Key {
                                    key: key.into(),
                                    pressed: true,
                                    modifiers: self.native_modifiers,
                                },
                            });
                        }
                        return;
                    }
                    let alt_graph = self.native_modifiers.control && self.native_modifiers.alt
                        && !self.native_modifiers.logo;
                    if pressed && ((!self.native_modifiers.control && !self.native_modifiers.logo
                        && !self.native_modifiers.alt) || alt_graph)
                        && let Some(text) = event.text.as_ref().filter(|t| {
                            !t.is_empty()
                                && t.chars().all(|c| !c.is_control() || c == '\t')
                                && t.as_str() != "\t"
                        })
                    {
                        self.manual(InputIntent::Ui {
                            intent: crate::ui::UiIntent::Text {
                                text: text.to_string(),
                            },
                        });
                    }
                    return;
                }
                if code == KeyCode::Tab && self.session.screen() == "game" {
                    self.manual(InputIntent::Key {
                        key: "player_list".into(),
                        pressed,
                    });
                    return;
                }
                if let Some(intent) = native_item_key(
                    code, pressed, self.native_modifiers, self.session.screen() == "inventory",
                ) {
                    self.manual(intent);
                    return;
                }
                if code == KeyCode::Escape && pressed {
                    self.release("emergency_stop");
                    if self.session.screen() == "inventory" {
                        let _ = self.session.input(InputIntent::Key {
                            key: "inventory".into(),
                            pressed: true,
                        });
                        self.publish_session();
                    } else {
                        // Game -> Pause lives in UiState::key ("Escape"),
                        // reached via the dialog-screen ui_cancel path.
                        self.manual(InputIntent::Key {
                            key: "ui_cancel".into(),
                            pressed: true,
                        });
                    }
                    return;
                }
                // Gameplay motion/drop inputs are inert while an inventory/container owns the UI.
                if self.session.screen() == "inventory"
                    && matches!(
                        code,
                        KeyCode::KeyW
                            | KeyCode::KeyA
                            | KeyCode::KeyS
                            | KeyCode::KeyD
                            | KeyCode::Space
                            | KeyCode::ControlLeft
                            | KeyCode::ControlRight
                            | KeyCode::KeyQ
                    )
                {
                    return;
                }
                // Bare T/Slash/F2/F3: gate on tracked Shift (no modifier
                // snapshot exists on KeyEvent in winit 0.30; Ctrl/Alt/Logo
                // combos on these keys are not bound anyway).
                let bare = !self.shift_held;
                if pressed && bare {
                    match code {
                        KeyCode::KeyT => {
                            let _ = self
                                .session
                                .execute_ui(crate::ui::UiIntent::OpenChat { command: false });
                            self.publish_session();
                            return;
                        }
                        KeyCode::Slash => {
                            let _ = self
                                .session
                                .execute_ui(crate::ui::UiIntent::OpenChat { command: true });
                            self.publish_session();
                            return;
                        }
                        KeyCode::F2 => {
                            self.capture_screenshot();
                            return;
                        }
                        KeyCode::F3 => {
                            self.debug_overlay = !self.debug_overlay;
                            self.session.debug_overlay_pending = self.debug_overlay;
                            self.publish_session();
                            return;
                        }
                        _ => {}
                    }
                }
                let key = match code {
                    KeyCode::KeyW => "forward",
                    KeyCode::KeyS => "backward",
                    KeyCode::KeyA => "left",
                    KeyCode::KeyD => "right",
                    KeyCode::Space => "jump",
                    KeyCode::ShiftLeft | KeyCode::ShiftRight => "sneak",
                    KeyCode::ControlLeft | KeyCode::ControlRight => "sprint",
                    KeyCode::KeyE => "inventory",
                    _ => "",
                };
                if !key.is_empty() {
                    self.manual(InputIntent::Key {
                        key: key.into(),
                        pressed,
                    });
                }
            }
            // Dialogs synthesize a complete click on press. Inventory/container
            // screens preserve pointer edges so cursor transfers and drag gestures work.
            WindowEvent::MouseInput { state, button, .. }
                if self.focused
                    && (self.session.dialog_screen_active()
                        || self.session.screen() == "inventory") =>
            {
                // Options sliders drag across Move events: keep real edges there too.
                let inventory_screen = self.session.screen() == "inventory"
                    || self.session.ui_preserves_pointer_edges();
                self.cursor_locked = false;
                if let Some(window) = &self.window {
                    let _ = window.set_cursor_grab(CursorGrabMode::None);
                    window.set_cursor_visible(true);
                }
                let ui_button = match button {
                    MouseButton::Left => Some(crate::ui::PointerButton::Left),
                    MouseButton::Right => Some(crate::ui::PointerButton::Right),
                    MouseButton::Middle => Some(crate::ui::PointerButton::Middle),
                    _ => None,
                };
                if let (Some(ui_button), Some((x, y))) = (ui_button, self.last_cursor) {
                    let phase = if state == ElementState::Pressed {
                        crate::ui::PointerPhase::Down
                    } else {
                        crate::ui::PointerPhase::Up
                    };
                    let synthesized_up = !inventory_screen && state == ElementState::Pressed;
                    let edges = std::iter::once(if synthesized_up {
                        crate::ui::PointerPhase::Down
                    } else {
                        phase
                    })
                    .chain(synthesized_up.then_some(crate::ui::PointerPhase::Up));
                    for edge in edges {
                        let _ = self.session.command(Command::Input(InputIntent::Ui {
                            intent: crate::ui::UiIntent::Pointer {
                                phase: edge,
                                x,
                                y,
                                button: Some(ui_button),
                                modifiers: crate::ui::Modifiers {
                                    shift: self.shift_held,
                                    ..Default::default()
                                },
                            },
                        }));
                    }
                    self.publish_session();
                }
            }
            WindowEvent::MouseInput { state, button, .. }
                if self.focused && self.session.screen() == "game" =>
            {
                if state == ElementState::Pressed && !self.cursor_locked {
                    let window = self.window.as_ref().unwrap();
                    if window
                        .set_cursor_grab(CursorGrabMode::Locked)
                        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                        .is_ok()
                    {
                        window.set_cursor_visible(false);
                        self.cursor_locked = true;
                    }
                }
                let key = match button {
                    MouseButton::Left => Some("attack"),
                    MouseButton::Right => Some("use"),
                    _ => None,
                };
                if let Some(key) = key {
                    self.manual(InputIntent::Key {
                        key: key.into(),
                        pressed: state == ElementState::Pressed,
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.focused
                    && (self.session.screen() != "game" || self.session.dialog_screen_active()) =>
            {
                if let Some((x, y)) = self.last_cursor {
                    let (delta_x, delta_y) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (x, y),
                        MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                    };
                    self.manual(InputIntent::Ui {
                        intent: crate::ui::UiIntent::Scroll {
                            x,
                            y,
                            delta_x,
                            delta_y,
                            modifiers: crate::ui::Modifiers {
                                shift: self.shift_held,
                                ..Default::default()
                            },
                        },
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.focused
                    && self.session.screen() == "game"
                    && !self.session.dialog_screen_active() =>
            {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => self.hotbar_wheel.push_lines(f64::from(y)),
                    MouseScrollDelta::PixelDelta(p) => self.hotbar_wheel.push_pixels(p.y),
                };
                // Selection is read from the live session, never the published
                // observation snapshot (stale between publications).
                let selected = self.session.inventory.selected_slot;
                let slot = hotbar_wheel::step_slot(selected, notches);
                if slot != selected {
                    self.manual(InputIntent::Hotbar { slot });
                }
            }
            WindowEvent::RedrawRequested => {
                if self.occluded
                    || self.window.as_ref().unwrap().inner_size().width == 0
                    || self.window.as_ref().unwrap().inner_size().height == 0
                {
                    return;
                }
                if let Err(error) = self.draw() {
                    self.fail(event_loop, error);
                    return;
                }
            }
            _ => {}
        }
    }
    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if self.focused && self.cursor_locked {
            if let DeviceEvent::MouseMotion { delta } = event {
                // Client-local Options sensitivity multiplies the 0.12 base scale.
                let scale = 0.12 * self.session.mouse_sensitivity();
                let (yaw, pitch) = self.session.look_angles();
                let yaw = yaw + delta.0 as f32 * scale;
                let pitch = (pitch + delta.1 as f32 * scale).clamp(-89.9, 89.9);
                self.look_events += 1;
                self.manual(InputIntent::Look { yaw, pitch });
            }
        }
    }
    fn new_events(&mut self, _: &ActiveEventLoop, _: winit::event::StartCause) {
        self.input_only_wakeup = true;
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Raw motion is already applied. Drain/rebuild once for an owner
        // event or a tick/task/frame deadline, not at mouse polling frequency.
        if !self.input_only_wakeup || Instant::now() >= self.deadline() {
            self.advance();
        }
        if self.shutdown {
            event_loop.exit();
            return;
        }
        let now = Instant::now();
        if now >= self.next_frame {
            if let Some(window) = &self.window {
                if !self.occluded {
                    window.request_redraw();
                }
            }
            self.next_frame = now + self.frame_interval();
        }
        if let (Some(server), Some(window)) = (&self.automation, &self.window) {
            let revision = server.controller_revision();
            if revision != self.controller_revision {
                let title = match server.controller_label() {
                    Some(label) => format!("Solaris Native Client — Controller: {label}"),
                    None => "Solaris Native Client — AUTOMATION ENABLED (no controller)".into(),
                };
                window.set_title(&title);
                self.controller_revision = revision;
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline()));
    }

    fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
        self.input_only_wakeup = false;
    }
    fn suspended(&mut self, _: &ActiveEventLoop) {
        self.release("suspended");
    }
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.release("exiting");
        let _ = self.session.command(Command::Disconnect);
        self.automation.take();
        // run_app consumes the event loop. Destroy GPU instances (including
        // the completion worker's EGL backend) before its Wayland display,
        // while the window still owns valid native handles.
        self.renderer.take();
        self.window.take();
        self.frame_trace.take();
    }
}

#[cfg(test)]
mod cpu_hotpath_advance_tests {
    // Failing-first contract for the App.advance change gate: idle turns must
    // skip the full observation rebuild, while any session-triple movement
    // (revision/client_tick/packet_sequence) still publishes. Uses the
    // session snapshot helpers so no window, GPU, or network is required.
    use super::*;

    #[test]
    fn publish_gate_skips_idle_turns_and_fires_on_state_movement() {
        let mut session = Session::new_mode("advance-gate".into(), false);
        // Fresh session: gate closed.
        assert!(!session.observation_changed_since_publish());
        // Any of the three triple components opens the gate.
        session.client_tick += 1;
        assert!(session.observation_changed_since_publish(),
            "client_tick movement (wait_ticks) must open the publish gate");
        session.note_published();
        assert!(!session.observation_changed_since_publish(),
            "noted publish must close the gate for the next idle turn");
    }

}

#[cfg(test)]
mod window_input_pacing_tests {
    use super::*;

    #[test]
    fn f11_toggles_and_persists_fullscreen() {
        let mut session = Session::new_mode("fullscreen-key-test".into(), false);
        assert!(!toggle_fullscreen_key(&mut session, KeyCode::F11, false));
        assert!(!toggle_fullscreen_key(&mut session, KeyCode::F10, true));
        assert!(toggle_fullscreen_key(&mut session, KeyCode::F11, true));
        let path = std::env::temp_dir().join(format!("solaris-f11-settings-{}.json", std::process::id()));
        local_config::save_client_settings_to(&session.client_settings_snapshot(), &path);
        let saved = local_config::load_client_settings_from(&path);
        assert_eq!(saved.get("fullscreen"), Some(&json!(true)));
        let _ = std::fs::remove_file(path);
        let mut restored = Session::new_mode("fullscreen-key-restored".into(), false);
        restored.apply_persisted_settings(saved);
        assert_eq!(restored.native_setting("fullscreen"), Some(&json!(true)));
        assert!(toggle_fullscreen_key(&mut restored, KeyCode::F11, true));
        assert_eq!(restored.native_setting("fullscreen"), Some(&json!(false)));
    }

    #[test]
    fn vsync_is_present_paced_and_max_fps_caps_only_unsynced_frames() {
        // 120 fps under FIFO on a 144 Hz panel repeated every sixth refresh.
        assert_eq!(frame_interval(true, 120), Duration::ZERO);
        assert_eq!(frame_interval(true, 0), Duration::ZERO);
        assert_eq!(frame_interval(false, 0), Duration::ZERO);
        assert_eq!(frame_interval(false, 120), Duration::from_secs_f64(1. / 120.));
    }

    #[test]
    fn presentation_paced_or_hidden_surface_does_not_spin_on_frame_deadline() {
        let now = Instant::now();
        let tick = now + Duration::from_millis(50);
        assert_eq!(frame_deadline(false, tick, now), tick);
        assert_eq!(frame_deadline(true, tick, now), now);
        assert_eq!(frame_deadline(!frame_interval(true, 120).is_zero(), tick, now), tick);
        assert_eq!(frame_deadline(!frame_interval(false, 120).is_zero(), tick, now), now);
        assert_eq!(frame_deadline(!frame_interval(false, 0).is_zero(), tick, now), tick);
    }

    #[test]
    fn pointer_grab_retries_every_turn_until_held() {
        // Entering play while the grab cannot be taken yet (X11 grab refused,
        // window not viewable): the old transition-only rule never retried.
        let mut grabbed = false;
        for grab_succeeds in [false, false, true] {
            assert_eq!(pointer_grab_change(true, grabbed), Some(true));
            grabbed = grab_succeeds;
        }
        assert_eq!(pointer_grab_change(true, grabbed), None, "held grab is kept");
        assert_eq!(pointer_grab_change(false, grabbed), Some(false), "menus/focus loss release");
        assert_eq!(pointer_grab_change(false, false), None);
    }

    #[test]
    fn rc1_focused_inventory_text_owns_printable_and_editing_keys() {
        assert!(owns_native_text(false, false, false, true));
        assert!(!owns_native_text(false, false, false, false));
        assert!(owns_native_text(true, false, false, false));
        assert!(owns_native_text(false, true, false, false));
    }

    #[test]
    fn rc1_f2_requests_distinct_vanilla_screenshot_names() {
        let options = Options {
            automation: false,
            bind: "127.0.0.1:0".parse().unwrap(),
            artifacts: std::env::temp_dir(),
            headless: Some(true),
            runtime_dir: std::env::temp_dir(),
            profile: "fixture".into(),
            server: None,
            direct_join: false,
            username: None,
            width: 1280,
            height: 720,
            fullscreen: false,
        };
        let mut app = App::new(options, Arc::new(|| {})).unwrap();
        app.capture_screenshot();
        app.capture_screenshot();
        app.schedule_screenshot(true);
        let first = app.pending_capture.take().unwrap();
        app.schedule_screenshot(false);
        assert!(app.pending_capture.is_none(), "encoding retains the next F2 request");
        assert_eq!(app.screenshot_captures.len(), 1);
        app.schedule_screenshot(true);
        let second = app.pending_capture.take().unwrap();
        let Command::Capture { name: first, .. } = first.command else { panic!() };
        let Command::Capture { name: second, .. } = second.command else { panic!() };
        assert_ne!(first, second, "F2 requests in the same second must not collide");
        assert_eq!(first.len(), 19, "yyyy-MM-dd_HH.mm.ss");
        assert_eq!(first.as_bytes()[10], b'_');
    }
}
