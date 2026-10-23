//! Nested Wayland compositor for monitors. Wayland forbids injecting input into
//! other desktop windows, so apps are launched INTO a monitor instead: they connect
//! to a private `solaris-monitor-<n>` socket, render into wl_shm buffers that are
//! composited to RGBA here, and receive pointer/keyboard events directly.
use super::{DesktopKey, DesktopSource, Frame, PointerButton, audio::{self, AudioSource}};
use parking_lot::{Condvar, Mutex};
use smithay::{
    backend::{
        input::{Axis, AxisSource, ButtonState, KeyState},
        renderer::utils::{RendererSurfaceStateUserData, SurfaceView, on_commit_buffer_handler},
    },
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy, Window,
        WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords, space::SpaceElement,
    },
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, Keycode, XkbContext, xkb},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, Focus, MotionEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, channel, generic::Generic},
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_shm, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, TraversalAction, get_parent,
            is_sync_subsurface, with_states, with_surface_tree_upward,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            decoration::{XdgDecorationHandler, XdgDecorationState},
        },
        shm::{ShmHandler, ShmState, with_buffer_contents},
        socket::ListeningSocketSource,
    },
};
use std::{
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

/// Installed-app candidates used when the `monitor_apps` setting is absent.
const DEFAULT_APPS: &[&[&str]] = &[
    &["foot"],
    &["weston-terminal"],
    &["ptyxis", "-s"],
    &["kgx"],
    &["gnome-text-editor", "-s"],
    &["gnome-calculator"],
];
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const KEY_LEFTSHIFT: u32 = 42;
/// xkb keycodes are evdev codes offset by 8.
const XKB_OFFSET: u32 = 8;

/// Monitor apps as (picker name, argv). `monitor_apps` is an array whose items
/// are argv arrays or whitespace-separated command strings; when absent or not an
/// array, the installed defaults are offered.
pub(crate) fn apps(setting: Option<&serde_json::Value>) -> Vec<(String, Vec<String>)> {
    let commands: Vec<Vec<String>> = match setting.and_then(serde_json::Value::as_array) {
        Some(items) => items.iter().filter_map(argv_from_json).collect(),
        None => DEFAULT_APPS
            .iter()
            .filter(|argv| find_program(argv[0]).is_some())
            .map(|argv| argv.iter().map(|arg| (*arg).to_owned()).collect())
            .collect(),
    };
    commands
        .into_iter()
        .map(|argv| {
            let name = Path::new(&argv[0]).file_name().map_or_else(
                || argv[0].clone(),
                |name| name.to_string_lossy().into_owned(),
            );
            (name, argv)
        })
        .collect()
}

fn argv_from_json(item: &serde_json::Value) -> Option<Vec<String>> {
    let argv: Vec<String> = match item {
        serde_json::Value::String(command) => {
            command.split_whitespace().map(str::to_owned).collect()
        }
        serde_json::Value::Array(args) => args
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    (!argv.is_empty() && !argv[0].is_empty()).then_some(argv)
}

fn find_program(program: &str) -> Option<PathBuf> {
    let executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        let path = PathBuf::from(program);
        return executable(&path).then_some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| executable(path))
}

/// Surface size for a monitor rectangle of `aspect` = width / height: about
/// 1280x720 worth of pixels, so a 16:9 monitor gets exactly 1280x720.
fn monitor_size(aspect: f64) -> Size<i32, Logical> {
    let aspect = if aspect.is_finite() && aspect > 0. {
        aspect.clamp(1. / 16., 16.)
    } else {
        16. / 9.
    };
    let width = (1280. * 720. * aspect).sqrt();
    Size::from((width.round() as i32, (width / aspect).round() as i32))
}

/// Starts a private compositor sized for the monitor and launches `argv` into it.
pub(crate) fn launch(argv: &[String], aspect: f64) -> Result<Box<dyn DesktopSource>, String> {
    Ok(Box::new(Nested::launch(argv, aspect)?))
}

enum Cmd {
    Pointer(f64, f64),
    Button(u32, bool),
    Scroll(f64, f64),
    Key(DesktopKey, bool),
    FrameTaken,
    Stop,
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
    ready: Condvar,
    app_exited: AtomicBool,
}
#[derive(Default)]
struct Slot {
    frame: Option<Frame>,
    ended: Option<String>,
}
impl Shared {
    fn publish(&self, frame: Frame) {
        self.slot.lock().frame = Some(frame);
        self.ready.notify_all();
    }
    fn end(&self, reason: String) {
        self.slot.lock().ended.get_or_insert(reason);
        self.ready.notify_all();
    }
}

struct Nested {
    commands: channel::Sender<Cmd>,
    shared: Arc<Shared>,
    compositor: Option<thread::JoinHandle<()>>,
    /// Process-group leader of the launched app.
    app: Option<i32>,
    /// The app's only audio output; dropped after the app is killed.
    sink: Option<audio::NullSink>,
}
impl Nested {
    fn launch(argv: &[String], aspect: f64) -> Result<Self, String> {
        let program = argv.first().ok_or("Monitor app command is empty")?;
        let size = monitor_size(aspect);
        let shared = Arc::new(Shared::default());
        let (commands, receiver) = channel::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_shared = shared.clone();
        let compositor = thread::Builder::new()
            .name("monitor-compositor".into())
            .spawn(move || {
                let reason = match serve(size, thread_shared.clone(), receiver, &ready_tx) {
                    Ok(()) => "Monitor compositor stopped".to_owned(),
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.clone()));
                        error
                    }
                };
                thread_shared.end(reason);
            })
            .map_err(|error| format!("Monitor compositor thread: {error}"))?;
        let mut nested = Self {
            commands,
            shared,
            compositor: Some(compositor),
            app: None,
            sink: audio::NullSink::create().ok(),
        };
        let socket = ready_rx
            .recv()
            .map_err(|_| "Monitor compositor failed to start".to_owned())??;
        // Own D-Bus session: single-instance apps must not hand the window to an
        // instance already running on the real desktop.
        let mut command = match find_program("dbus-run-session") {
            Some(dbus) => {
                let mut command = Command::new(dbus);
                command.arg("--").args(argv);
                command
            }
            None => {
                let mut command = Command::new(program);
                command
                    .args(&argv[1..])
                    .env_remove("DBUS_SESSION_BUS_ADDRESS");
                command
            }
        };
        for (key, value) in nested.sink.iter().flat_map(|sink| sink.env()) {
            command.env(key, value);
        }
        command
            .env("WAYLAND_DISPLAY", &socket)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("DESKTOP_STARTUP_ID")
            .env_remove("XDG_ACTIVATION_TOKEN")
            // No dmabuf here: GL and GTK renderers must fall back to wl_shm.
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("GSK_RENDERER", "cairo")
            .env("GDK_BACKEND", "wayland")
            .env("QT_QPA_PLATFORM", "wayland")
            .env("SDL_VIDEODRIVER", "wayland")
            .env("NO_AT_BRIDGE", "1")
            .env("GTK_A11Y", "none")
            .stdin(Stdio::null())
            .process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| format!("Launch {program}: {error}"))?;
        nested.app = Some(child.id() as i32);
        let shared = nested.shared.clone();
        let name = program.clone();
        thread::Builder::new()
            .name("monitor-app-wait".into())
            .spawn(move || {
                let status = child.wait();
                shared.app_exited.store(true, Ordering::Release);
                shared.end(match status {
                    Ok(status) => format!("{name} exited ({status})"),
                    Err(error) => format!("{name}: {error}"),
                });
            })
            .map_err(|error| format!("Monitor app wait thread: {error}"))?;
        Ok(nested)
    }
    fn send(&self, command: Cmd) -> Result<(), String> {
        self.commands.send(command).map_err(|_| {
            self.shared
                .slot
                .lock()
                .ended
                .clone()
                .unwrap_or_else(|| "Monitor compositor stopped".into())
        })
    }
    fn take_frame(&mut self, slot: &mut Slot) -> Result<Option<Frame>, String> {
        if let Some(frame) = slot.frame.take() {
            // Frame callbacks follow consumption, so apps draw at the game's pace.
            let _ = self.commands.send(Cmd::FrameTaken);
            return Ok(Some(frame));
        }
        slot.ended.clone().map_or(Ok(None), Err)
    }
}
impl DesktopSource for Nested {
    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        let shared = self.shared.clone();
        let mut slot = shared.slot.lock();
        self.take_frame(&mut slot)
    }
    fn control_granted(&self) -> bool {
        true
    }
    fn audio(&mut self) -> Option<Box<dyn AudioSource>> {
        self.sink.as_ref()?.capture().ok()
    }
    fn private_audio(&self) -> bool {
        self.sink.is_some()
    }
    fn send_pointer(&mut self, x: f64, y: f64) -> Result<(), String> {
        self.send(Cmd::Pointer(x, y))
    }
    fn send_button(&mut self, button: PointerButton, pressed: bool) -> Result<(), String> {
        let code = match button {
            PointerButton::Left => BTN_LEFT,
            PointerButton::Right => BTN_RIGHT,
            PointerButton::Middle => BTN_MIDDLE,
        };
        self.send(Cmd::Button(code, pressed))
    }
    fn send_scroll(&mut self, dx: f64, dy: f64) -> Result<(), String> {
        self.send(Cmd::Scroll(dx, dy))
    }
    fn send_key(&mut self, key: DesktopKey, pressed: bool) -> Result<(), String> {
        self.send(Cmd::Key(key, pressed))
    }
    fn close(&mut self) {
        if let Some(group) = self.app.take()
            && !self.shared.app_exited.load(Ordering::Acquire)
        {
            // SAFETY: plain syscall on the process group created for the app.
            unsafe { libc::killpg(group, libc::SIGKILL) };
        }
        let _ = self.commands.send(Cmd::Stop);
        if let Some(compositor) = self.compositor.take() {
            let _ = compositor.join();
        }
        self.sink = None;
        self.shared.slot.lock().frame = None;
        self.shared.end("Monitor app closed".into());
    }
}
impl Drop for Nested {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Default)]
struct ClientState {
    compositor: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

fn serve(
    size: Size<i32, Logical>,
    shared: Arc<Shared>,
    commands: channel::Channel<Cmd>,
    ready: &mpsc::Sender<Result<String, String>>,
) -> Result<(), String> {
    let mut event_loop: EventLoop<State> =
        EventLoop::try_new().map_err(|error| format!("Monitor event loop: {error}"))?;
    let display: Display<State> =
        Display::new().map_err(|error| format!("Monitor Wayland display: {error}"))?;
    let dh = display.handle();
    let socket = (0..64)
        .find_map(|n| ListeningSocketSource::with_name(&format!("solaris-monitor-{n}")).ok())
        .ok_or("No free solaris-monitor-<n> Wayland socket in XDG_RUNTIME_DIR")?;
    let name = socket.socket_name().to_string_lossy().into_owned();
    let handle = event_loop.handle();
    handle
        .insert_source(socket, |stream, _, state| {
            if let Err(error) = state
                .dh
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                eprintln!("monitor compositor: rejected client: {error}");
            }
        })
        .map_err(|error| error.error.to_string())?;
    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // SAFETY: the display is never dropped while the event loop owns it.
                unsafe { display.get_mut().dispatch_clients(state)? };
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| error.error.to_string())?;
    handle
        .insert_source(commands, |event, _, state| match event {
            channel::Event::Msg(command) => state.command(command),
            channel::Event::Closed => state.signal.stop(),
        })
        .map_err(|error| error.error.to_string())?;
    let mut state = State::new(dh, size, shared, event_loop.get_signal())?;
    let _ = ready.send(Ok(name));
    event_loop
        .run(None, &mut state, State::after_dispatch)
        .map_err(|error| format!("Monitor event loop: {error}"))
}

struct State {
    dh: DisplayHandle,
    signal: LoopSignal,
    shared: Arc<Shared>,
    compositor: CompositorState,
    xdg: XdgShellState,
    shm: ShmState,
    _outputs: OutputManagerState,
    _decorations: XdgDecorationState,
    data_device: DataDeviceState,
    seat_state: SeatState<State>,
    seat: Seat<State>,
    output: Output,
    popups: PopupManager,
    /// Bottom to top; the first is the main window, configured to the monitor size.
    windows: Vec<Window>,
    size: Size<i32, Logical>,
    start: Instant,
    dirty: bool,
    cursor_position: Option<Point<i32, Logical>>,
}
impl State {
    fn new(
        dh: DisplayHandle,
        size: Size<i32, Logical>,
        shared: Arc<Shared>,
        signal: LoopSignal,
    ) -> Result<Self, String> {
        let compositor = CompositorState::new::<Self>(&dh);
        let xdg = XdgShellState::new::<Self>(&dh);
        let shm = ShmState::new::<Self>(&dh, []);
        let outputs = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let decorations = XdgDecorationState::new::<Self>(&dh);
        let data_device = DataDeviceState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "solaris-monitor");
        seat.add_keyboard(Default::default(), 400, 25)
            .map_err(|error| format!("Monitor keyboard keymap: {error}"))?;
        seat.add_pointer();
        let output = Output::new(
            "solaris-monitor".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Solaris".into(),
                model: "Monitor".into(),
            },
        );
        let _ = output.create_global::<Self>(&dh);
        let mode = OutputMode {
            size: (size.w, size.h).into(),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        output.set_preferred(mode);
        Ok(Self {
            dh,
            signal,
            shared,
            compositor,
            xdg,
            shm,
            _outputs: outputs,
            _decorations: decorations,
            data_device,
            seat_state,
            seat,
            output,
            popups: PopupManager::default(),
            windows: Vec::new(),
            size,
            start: Instant::now(),
            dirty: false,
            cursor_position: None,
        })
    }
    fn time(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }
    fn after_dispatch(&mut self) {
        if std::mem::take(&mut self.dirty)
            && let Some(frame) = self.compose()
        {
            self.shared.publish(frame);
        }
        self.popups.cleanup();
        let _ = self.dh.flush_clients();
    }
    fn window_index(&self, surface: &WlSurface) -> Option<usize> {
        self.windows
            .iter()
            .position(|window| window.toplevel().is_some_and(|t| t.wl_surface() == surface))
    }
    fn frame_size(&self) -> Size<i32, Logical> {
        match self.windows.first().map(Window::geometry) {
            Some(geometry) if geometry.size.w > 0 && geometry.size.h > 0 => {
                Size::from((geometry.size.w.min(4096), geometry.size.h.min(4096)))
            }
            _ => self.size,
        }
    }
    /// Frame-space position of a window's surface origin: the main window's
    /// geometry fills the frame; later windows (dialogs) are centered over it.
    fn origin(&self, index: usize) -> Point<i32, Logical> {
        let geometry = self.windows[index].geometry();
        if index == 0 {
            return Point::from((-geometry.loc.x, -geometry.loc.y));
        }
        let frame = self.frame_size();
        Point::from((
            (frame.w - geometry.size.w) / 2 - geometry.loc.x,
            (frame.h - geometry.size.h) / 2 - geometry.loc.y,
        ))
    }
    fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(usize, WlSurface, Point<f64, Logical>)> {
        (0..self.windows.len()).rev().find_map(|index| {
            let origin = self.origin(index);
            self.windows[index]
                .surface_under(point - origin.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, location)| (index, surface, (location + origin).to_f64()))
        })
    }
    fn configure_main(toplevel: &ToplevelSurface, size: Size<i32, Logical>) {
        toplevel.with_pending_state(|state| {
            state.size = Some(size);
            state.bounds = Some(size);
            state.states.set(xdg_toplevel::State::Maximized);
        });
    }
    /// Keyboard focus and the activated state follow the topmost window.
    fn focus(&mut self, index: Option<usize>) {
        let focus = index
            .and_then(|index| self.windows.get(index))
            .and_then(Window::toplevel);
        let surface = focus.map(|toplevel| toplevel.wl_surface().clone());
        for window in &self.windows {
            let active = window.toplevel().map(ToplevelSurface::wl_surface) == surface.as_ref();
            if window.set_activated(active)
                && let Some(toplevel) = window.toplevel()
                && toplevel.is_initial_configure_sent()
            {
                toplevel.send_pending_configure();
            }
        }
        let keyboard = self.seat.get_keyboard().expect("monitor seat keyboard");
        if !keyboard.is_grabbed() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }
    fn command(&mut self, command: Cmd) {
        let time = self.time();
        let pointer = self.seat.get_pointer().expect("monitor seat pointer");
        match command {
            Cmd::Pointer(x, y) => {
                let location = Point::from((x, y));
                let cursor = Point::from((x.floor() as i32, y.floor() as i32));
                if self.cursor_position != Some(cursor) {
                    self.cursor_position = Some(cursor);
                    self.dirty = true;
                }
                let under = self
                    .surface_under(location)
                    .map(|(_, surface, at)| (surface, at));
                let serial = SERIAL_COUNTER.next_serial();
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location,
                        serial,
                        time,
                    },
                );
                pointer.frame(self);
            }
            Cmd::Button(button, pressed) => {
                if pressed && !pointer.is_grabbed() {
                    let index = self
                        .surface_under(pointer.current_location())
                        .map(|(i, ..)| i);
                    if index.is_some() {
                        self.focus(index);
                    }
                }
                let state = if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                let serial = SERIAL_COUNTER.next_serial();
                pointer.button(
                    self,
                    &ButtonEvent {
                        serial,
                        time,
                        button,
                        state,
                    },
                );
                pointer.frame(self);
            }
            Cmd::Scroll(dx, dy) => {
                // Game wheel deltas are lines with +y up; Wayland axes are +down.
                let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
                for (axis, lines) in [(Axis::Horizontal, dx), (Axis::Vertical, -dy)] {
                    if lines != 0. {
                        frame = frame
                            .value(axis, lines * 15.)
                            .v120(axis, (lines * 120.).round() as i32);
                    }
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            Cmd::Key(key, pressed) => self.key(key, pressed),
            Cmd::FrameTaken => {
                let output = self.output.clone();
                let now = self.start.elapsed();
                for window in &self.windows {
                    window.send_frame(&output, now, None, |_, _| Some(output.clone()));
                }
            }
            Cmd::Stop => self.signal.stop(),
        }
    }
    fn key(&mut self, key: DesktopKey, pressed: bool) {
        let keyboard = self.seat.get_keyboard().expect("monitor seat keyboard");
        let (code, shift) = match key {
            DesktopKey::Code(code) => (code, false),
            // Characters without a physical key in this keymap cannot be typed.
            DesktopKey::Char(ch) => {
                match keyboard.with_xkb_state(self, |context| char_key(&context, ch)) {
                    Some(found) => found,
                    None => return,
                }
            }
        };
        let mut input = |code: u32, pressed: bool| {
            let state = if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            };
            let serial = SERIAL_COUNTER.next_serial();
            let time = self.time();
            keyboard.input::<(), _>(
                self,
                Keycode::new(code + XKB_OFFSET),
                state,
                serial,
                time,
                |_, _, _| FilterResult::Forward,
            );
        };
        if shift && pressed {
            input(KEY_LEFTSHIFT, true);
        }
        input(code, pressed);
        if shift && !pressed {
            input(KEY_LEFTSHIFT, false);
        }
    }
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Some(index) = find_popup_root_surface(&kind)
            .ok()
            .and_then(|root| self.window_index(&root))
        else {
            return;
        };
        // Positioner coordinates are relative to the parent's window geometry.
        let parent = self.origin(index)
            + self.windows[index].geometry().loc
            + get_popup_toplevel_coords(&kind);
        let target = Rectangle::new(Point::from((-parent.x, -parent.y)), self.frame_size());
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
    /// Composites every window and popup bottom to top into an opaque RGBA frame.
    fn compose(&self) -> Option<Frame> {
        let main = self.windows.first()?.toplevel()?.wl_surface().clone();
        let mapped = with_states(&main, |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .is_some_and(|data| data.lock().unwrap().buffer().is_some())
        });
        if !mapped {
            return None;
        }
        let size = self.frame_size();
        let mut canvas = Canvas {
            width: size.w,
            height: size.h,
            rgba: [0, 0, 0, 255].repeat(size.w as usize * size.h as usize),
        };
        for (index, window) in self.windows.iter().enumerate() {
            let Some(surface) = window.toplevel().map(ToplevelSurface::wl_surface) else {
                continue;
            };
            let origin = self.origin(index);
            canvas.draw_tree(surface, origin);
            for (popup, location) in PopupManager::popups_for_surface(surface) {
                let at = origin + window.geometry().loc + location - popup.geometry().loc;
                canvas.draw_tree(popup.wl_surface(), at);
            }
        }
        if let Some(at) = self.cursor_position {
            canvas.draw_pointer(at);
        }
        Some(Frame {
            width: size.w as u32,
            height: size.h as u32,
            rgba: canvas.rgba,
        })
    }
}

/// Finds the keycode (evdev) and shift level typing `ch` in the active layout.
fn char_key(context: &XkbContext<'_>, ch: char) -> Option<(u32, bool)> {
    let xkb = context.xkb().lock().unwrap();
    let layout = xkb.active_layout();
    // SAFETY: the keymap reference does not outlive the locked Xkb.
    let keymap = unsafe { xkb.keymap() };
    let target = xkb::utf32_to_keysym(u32::from(ch));
    (XKB_OFFSET..=255).find_map(|code| {
        (0..2).find_map(|level| {
            keymap
                .key_get_syms_by_level(Keycode::new(code), layout.0, level)
                .contains(&target)
                .then_some((code - XKB_OFFSET, level == 1))
        })
    })
}

struct Canvas {
    width: i32,
    height: i32,
    rgba: Vec<u8>,
}
impl Canvas {
    /// Small high-contrast pointer marker, independent of the host cursor theme.
    fn draw_pointer(&mut self, at: Point<i32, Logical>) {
        for y in 0..18 {
            for x in 0..=y / 2 {
                let (px, py) = (at.x + x, at.y + y);
                if px < 0 || py < 0 || px >= self.width || py >= self.height { continue; }
                let edge = x == 0 || x == y / 2 || y == 17;
                let shade = if edge { 0 } else { 255 };
                let offset = 4 * (py * self.width + px) as usize;
                self.rgba[offset..offset + 4].copy_from_slice(&[shade, shade, shade, 255]);
            }
        }
    }
    fn draw_tree(&mut self, surface: &WlSurface, at: Point<i32, Logical>) {
        with_surface_tree_upward(
            surface,
            at,
            |_, states, location| {
                let view = states
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .and_then(|data| data.lock().unwrap().view());
                match view {
                    Some(view) => TraversalAction::DoChildren(*location + view.offset),
                    None => TraversalAction::SkipChildren,
                }
            },
            |_, states, location| {
                let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() else {
                    return;
                };
                let data = data.lock().unwrap();
                if let (Some(view), Some(buffer)) = (data.view(), data.buffer()) {
                    self.blit(buffer, view, data.buffer_scale(), *location + view.offset);
                }
            },
            |_, _, _| true,
        );
    }
    /// Nearest-neighbour copy of a wl_shm buffer, premultiplied-alpha "over".
    fn blit(&mut self, buffer: &WlBuffer, view: SurfaceView, scale: i32, at: Point<i32, Logical>) {
        let _ = with_buffer_contents(buffer, |ptr, len, data| {
            // Byte offsets of R, G, B and optional A in a little-endian pixel.
            let (r, g, b, a) = match data.format {
                wl_shm::Format::Argb8888 => (2, 1, 0, Some(3)),
                wl_shm::Format::Xrgb8888 => (2, 1, 0, None),
                wl_shm::Format::Abgr8888 => (0, 1, 2, Some(3)),
                wl_shm::Format::Xbgr8888 => (0, 1, 2, None),
                _ => return,
            };
            let (dst_w, dst_h) = (view.dst.w, view.dst.h);
            let (x0, x1) = (at.x.max(0), (at.x + dst_w).min(self.width));
            let (y0, y1) = (at.y.max(0), (at.y + dst_h).min(self.height));
            if x0 >= x1 || y0 >= y1 || data.width <= 0 || data.height <= 0 {
                return;
            }
            // SAFETY: smithay maps the pool for `len` bytes for the closure's duration.
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
            let scale = f64::from(scale.max(1));
            let sample = |d: i32, start: f64, extent: f64, dst: i32, limit: i32| {
                let logical = start + (f64::from(d) + 0.5) * extent / f64::from(dst);
                ((logical * scale) as i32).clamp(0, limit - 1) as usize
            };
            let columns: Vec<usize> = (x0..x1)
                .map(|x| 4 * sample(x - at.x, view.src.loc.x, view.src.size.w, dst_w, data.width))
                .collect();
            for y in y0..y1 {
                let row = data.offset as usize
                    + data.stride as usize
                        * sample(
                            y - at.y,
                            view.src.loc.y,
                            view.src.size.h,
                            dst_h,
                            data.height,
                        );
                let start = 4 * (y * self.width + x0) as usize;
                let end = 4 * (y * self.width + x1) as usize;
                for (out, column) in self.rgba[start..end].chunks_exact_mut(4).zip(&columns) {
                    let Some(px) = bytes.get(row + column..row + column + 4) else {
                        return;
                    };
                    let alpha = a.map_or(255, |a| px[a]);
                    let inverse = 255 - u32::from(alpha);
                    for (channel, source) in [(0, px[r]), (1, px[g]), (2, px[b])] {
                        let below = u32::from(out[channel]) * inverse / 255;
                        out[channel] = (u32::from(source) + below).min(255) as u8;
                    }
                }
            }
        });
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("monitor client data")
            .compositor
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_index(&root).map(|index| &self.windows[index]) {
                window.on_commit();
                window.output_enter(&self.output, window.bbox());
            }
        }
        if let Some(index) = self.window_index(surface)
            && let Some(toplevel) = self.windows[index].toplevel()
            && !toplevel.is_initial_configure_sent()
        {
            toplevel.send_configure();
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            let _ = popup.send_configure();
        }
        self.dirty = true;
    }
}
impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _: &WlBuffer) {}
}
impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}
impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }
    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let size = self.size;
        toplevel.with_pending_state(|state| {
            state.bounds = Some(size);
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        if self.windows.is_empty() {
            Self::configure_main(&toplevel, size);
        }
        self.windows.push(Window::new_wayland_window(toplevel));
        self.focus(Some(self.windows.len() - 1));
    }
    fn toplevel_destroyed(&mut self, toplevel: ToplevelSurface) {
        let Some(index) = self.window_index(toplevel.wl_surface()) else {
            return;
        };
        self.windows.remove(index);
        if index == 0
            && let Some(main) = self.windows.first().and_then(Window::toplevel)
        {
            Self::configure_main(main, self.size);
            main.send_pending_configure();
        }
        self.focus(self.windows.len().checked_sub(1));
        self.dirty = true;
    }
    fn new_popup(&mut self, popup: PopupSurface, positioner: PositionerState) {
        popup.with_pending_state(|state| state.geometry = positioner.get_geometry());
        self.unconstrain_popup(&popup);
        let _ = self.popups.track_popup(PopupKind::Xdg(popup));
    }
    fn reposition_request(&mut self, popup: PopupSurface, positioner: PositionerState, token: u32) {
        popup.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&popup);
        popup.send_repositioned(token);
    }
    fn popup_destroyed(&mut self, _: PopupSurface) {
        self.dirty = true;
    }
    fn grab(&mut self, popup: PopupSurface, _: WlSeat, serial: Serial) {
        let kind = PopupKind::Xdg(popup);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let seat = self.seat.clone();
        let Ok(mut grab) = self.popups.grab_popup::<Self>(root, kind, &seat, serial) else {
            return;
        };
        let keyboard = seat.get_keyboard().expect("monitor seat keyboard");
        let pointer = seat.get_pointer().expect("monitor seat pointer");
        let previous = grab.previous_serial().unwrap_or(serial);
        if keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(previous)) {
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        keyboard.set_focus(self, grab.current_grab(), serial);
        keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        let previous = grab.previous_serial().unwrap_or_else(|| grab.serial());
        if pointer.is_grabbed() && !(pointer.has_grab(serial) || pointer.has_grab(previous)) {
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
    }
}
/// Monitors draw no decorations, and clients are told not to draw their own.
impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        server_side(&toplevel);
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _: DecorationMode) {
        server_side(&toplevel);
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        server_side(&toplevel);
    }
}
fn server_side(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
    if toplevel.is_initial_configure_sent() {
        toplevel.send_pending_configure();
    }
}
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|surface| self.dh.get_client(surface.id()).ok());
        set_data_device_focus(&self.dh, seat, client);
    }
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
}
impl SelectionHandler for State {
    type SelectionUserData = ();
}
impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}
impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}
impl OutputHandler for State {}
delegate_compositor!(State);
delegate_shm!(State);
delegate_xdg_shell!(State);
delegate_xdg_decoration!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn pointer_marker_is_visible_and_clipped_to_frame() {
        let mut canvas = Canvas { width: 24, height: 24, rgba: vec![100; 24 * 24 * 4] };
        canvas.draw_pointer((2, 2).into());
        assert_eq!(&canvas.rgba[4 * (2 * 24 + 2)..][..4], &[0, 0, 0, 255]);
        assert_eq!(&canvas.rgba[4 * (8 * 24 + 3)..][..4], &[255, 255, 255, 255]);
        for at in [(23, 23), (-8, -17), (24, 24), (-100, -100)] {
            canvas.draw_pointer(at.into());
        }
        assert_eq!(canvas.rgba.len(), 24 * 24 * 4);
    }
    #[test]
    fn monitor_apps_setting_accepts_strings_and_argv_arrays() {
        let setting = serde_json::json!([
            "foot --server",
            ["/usr/bin/gnome-text-editor", "-s"],
            "",
            3,
            []
        ]);
        assert_eq!(
            apps(Some(&setting)),
            vec![
                (
                    "foot".to_owned(),
                    vec!["foot".to_owned(), "--server".to_owned()]
                ),
                (
                    "gnome-text-editor".to_owned(),
                    vec!["/usr/bin/gnome-text-editor".to_owned(), "-s".to_owned()]
                ),
            ]
        );
        assert!(apps(Some(&serde_json::json!([]))).is_empty());
        for (_, argv) in apps(None) {
            assert!(
                find_program(&argv[0]).is_some(),
                "default {argv:?} is installed"
            );
        }
    }

    #[test]
    fn monitor_size_keeps_aspect_and_pixel_budget() {
        assert_eq!(monitor_size(16. / 9.), Size::from((1280, 720)));
        let wide = monitor_size(15.875 / 0.875);
        assert!(wide.w <= 4096 && wide.h >= 200, "{wide:?}");
        assert_eq!(monitor_size(f64::NAN), Size::from((1280, 720)));
    }

    fn wait_frame(nested: &mut Nested, deadline: Instant) -> Option<Frame> {
        let shared = nested.shared.clone();
        let mut slot = shared.slot.lock();
        loop {
            match nested.take_frame(&mut slot) {
                Ok(Some(frame)) => return Some(frame),
                Ok(None) => {}
                Err(error) => panic!("monitor app ended: {error}"),
            }
            if shared.ready.wait_until(&mut slot, deadline).timed_out() {
                return None;
            }
        }
    }

    fn save(frame: &Frame, name: &str) -> PathBuf {
        let dir = std::env::var_os("SOLARIS_NESTED_TEST_OUT")
            .map_or_else(std::env::temp_dir, PathBuf::from);
        let path = dir.join(name);
        let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut encoder = png::Encoder::new(file, frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&frame.rgba)
            .unwrap();
        path
    }

    /// Real end-to-end proof: run with `-- --ignored` (optionally
    /// SOLARIS_NESTED_TEST_APP="ptyxis -s"). Never touches the real desktop: the
    /// app sees only the nested socket and its own D-Bus session.
    #[test]
    #[ignore = "launches a real Wayland app into the nested compositor"]
    fn real_app_renders_and_receives_typed_keys() {
        let argv: Vec<String> = match std::env::var("SOLARIS_NESTED_TEST_APP") {
            Ok(command) => command.split_whitespace().map(str::to_owned).collect(),
            Err(_) => {
                apps(None)
                    .into_iter()
                    .next()
                    .expect("an installed monitor app")
                    .1
            }
        };
        let mut nested = Nested::launch(&argv, 16. / 9.).expect("launch");
        let first = wait_frame(&mut nested, Instant::now() + Duration::from_secs(60))
            .expect("first frame from the app");
        assert_eq!((first.width, first.height), (1280, 720));
        assert_eq!(first.rgba.len(), 1280 * 720 * 4);
        // Settle: keep consuming frames until the app idles for a second, and mask
        // pixels that change on their own (cursor blink, startup animation).
        let mut baseline = first;
        let mut unstable = vec![false; baseline.rgba.len() / 4];
        let settle = Instant::now() + Duration::from_secs(15);
        while let Some(frame) = wait_frame(
            &mut nested,
            (Instant::now() + Duration::from_secs(1)).min(settle),
        ) {
            for (i, flag) in unstable.iter_mut().enumerate() {
                *flag |= frame.rgba[4 * i..4 * i + 4] != baseline.rgba[4 * i..4 * i + 4];
            }
            baseline = frame;
        }
        let distinct: std::collections::HashSet<&[u8]> = baseline.rgba.chunks_exact(4).collect();
        assert!(
            distinct.len() > 8,
            "app drew real content ({} colours)",
            distinct.len()
        );
        println!(
            "baseline: {}",
            save(&baseline, "solaris-nested-before.png").display()
        );
        nested.send_pointer(640., 360.).unwrap();
        nested.send_button(PointerButton::Left, true).unwrap();
        nested.send_button(PointerButton::Left, false).unwrap();
        // "echo hi\n" as evdev codes, the same path physical game keys take.
        for code in [18, 46, 35, 24, 57, 35, 23, 28] {
            nested.send_key(DesktopKey::Code(code), true).unwrap();
            nested.send_key(DesktopKey::Code(code), false).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut best = 0;
        let mut typed = None;
        while let Some(frame) = wait_frame(&mut nested, deadline) {
            assert_eq!(
                (frame.width, frame.height),
                (baseline.width, baseline.height)
            );
            let changed = (0..unstable.len())
                .filter(|&i| {
                    !unstable[i] && frame.rgba[4 * i..4 * i + 4] != baseline.rgba[4 * i..4 * i + 4]
                })
                .count();
            if changed > best {
                best = changed;
                println!(
                    "typed: {changed} stable pixels changed: {}",
                    save(&frame, "solaris-nested-after.png").display()
                );
            }
            if changed >= 300 {
                typed = Some(frame);
                break;
            }
        }
        let typed = typed.unwrap_or_else(|| panic!("typing changed only {best} stable pixels"));
        // Right-click opens a context menu: an xdg_popup with a grab, composited on top.
        nested.send_button(PointerButton::Right, true).unwrap();
        nested.send_button(PointerButton::Right, false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut menu = 0;
        while let Some(frame) = wait_frame(&mut nested, deadline) {
            menu = frame
                .rgba
                .chunks_exact(4)
                .zip(typed.rgba.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            if menu >= 5000 {
                println!(
                    "popup: {menu} pixels changed: {}",
                    save(&frame, "solaris-nested-popup.png").display()
                );
                break;
            }
        }
        assert!(menu >= 5000, "context menu changed only {menu} pixels");
        nested.close();
        assert!(nested.next_frame().is_err());
    }
}
