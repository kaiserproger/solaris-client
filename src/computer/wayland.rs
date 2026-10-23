use super::{DesktopKey, DesktopSource, Frame, PointerButton, WindowInfo};
use ashpd::desktop::{
    PersistMode,
    remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions},
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use parking_lot::Mutex;
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::{fs::OpenOptionsExt, process::CommandExt}},
    process::{Command, Stdio},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread,
};

type Latest = Arc<Mutex<Result<Option<Frame>, String>>>;
// Portals may reject a new session until the previous session's Close has completed.
// Serialize only backend workers; the render/input thread never waits for D-Bus.
static PORTAL_SESSION: Mutex<()> = Mutex::new(());
enum Input {
    Pointer(f64, f64),
    Button(PointerButton, bool),
    Scroll(f64, f64),
    Key(DesktopKey, bool),
}
struct WaylandSource {
    latest: Latest,
    input: tokio::sync::mpsc::Sender<Input>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    control: Arc<AtomicBool>,
}
struct CaptureChild(std::process::Child);
impl Drop for CaptureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("Desktop capture requires a Wayland session".into());
    }
    let mut windows = choices()?.into_iter().map(|choice| WindowInfo {
        id: choice.id,
        title: choice.title,
    }).collect::<Vec<_>>();
    windows.push(WindowInfo {
        id: 0,
        title: "Choose a NEW window in the system permission dialog".into(),
    });
    Ok(windows)
}
pub fn open(id: u64) -> Result<Box<dyn DesktopSource>, String> {
    list_windows()?;
    let restore_token = if id == 0 { None } else {
        Some(choices()?.into_iter().find(|choice| choice.id == id)
            .ok_or("Saved desktop window no longer exists")?.token)
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let latest: Latest = Arc::new(Mutex::new(Ok(None)));
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
    let output = latest.clone();
    let control = Arc::new(AtomicBool::new(false));
    let granted = control.clone();
    thread::Builder::new().name("desktop-portal".into()).spawn(move || {
        let _session_guard = PORTAL_SESSION.lock();
        if !matches!(cancelled.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)) {
            return;
        }
        let result = runtime.block_on(async {
            let remote = RemoteDesktop::new().await.map_err(|e|e.to_string())?;
            let cast = Screencast::new().await.map_err(|e|e.to_string())?;
            let session = remote.create_session(Default::default()).await.map_err(|e|e.to_string())?;
            let run = async {
                remote.select_devices(&session, SelectDevicesOptions::default().set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked).set_restore_token(restore_token.as_deref()))
                    .await.map_err(|e|e.to_string())?.response().map_err(|e|e.to_string())?;
                cast.select_sources(&session, SelectSourcesOptions::default().set_sources(Some(SourceType::Window.into()))
                    .set_multiple(false).set_cursor_mode(CursorMode::Embedded))
                    .await.map_err(|e|e.to_string())?.response().map_err(|e|e.to_string())?;
                let selected = remote.start(&session, None, Default::default()).await.map_err(|e|e.to_string())?.response().map_err(|e|e.to_string())?;
                // GNOME grants input only when "Allow remote interaction" is on; still show the window.
                let control = selected.devices().contains(DeviceType::Pointer) && selected.devices().contains(DeviceType::Keyboard);
                granted.store(control, Ordering::Release);
                if !control { eprintln!("Desktop computer: view only (enable \"Allow remote interaction\" in the share dialog for control)"); }
                let stream = selected.streams().first().ok_or("Portal returned no window")?;
                let node = stream.pipe_wire_node_id();
                let (width,height) = stream.size().ok_or("Portal did not supply capture dimensions")?;
                if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) { return Err("Desktop window dimensions exceed 8192".into()); }
                if let Some(token) = selected.restore_token()
                    && let Err(error) = save_choice(id, token, width, height) {
                    eprintln!("Desktop consent could not be saved: {error}");
                }
                let fd = cast.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e|e.to_string())?;
                let raw_fd = fd.as_raw_fd();
                let mut command = Command::new("gst-launch-1.0");
                command.args(["-q", "pipewiresrc", &format!("fd={raw_fd}"), &format!("path={node}"), "do-timestamp=true", "!", "videoconvert", "!", "videoscale", "!", &format!("video/x-raw,format=RGBA,width={width},height={height},pixel-aspect-ratio=1/1"), "!", "fdsink", "fd=1", "sync=false"])
                    .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit());
                // Only the child's copy loses CLOEXEC. The portal fd remains private in the parent.
                unsafe { command.pre_exec(move || {
                    if libc::fcntl(raw_fd, libc::F_SETFD, 0) == -1 { return Err(std::io::Error::last_os_error()); }
                    Ok(())
                }); }
                let mut child = CaptureChild(command.spawn().map_err(|e|format!("Cannot start GStreamer PipeWire capture: {e}"))?);
                let mut pipe = child.0.stdout.take().ok_or("Missing GStreamer frame pipe")?;
                let frames = output.clone();
                let (ended_tx, mut ended_rx) = tokio::sync::oneshot::channel();
                let reader = thread::spawn(move || {
                    let size = width as usize * height as usize * 4;
                    let mut rgba = vec![0; size];
                    let result = loop {
                        if let Err(error) = pipe.read_exact(&mut rgba) { break Err(format!("Desktop capture stopped: {error}")); }
                        let mut slot = frames.lock();
                        let previous = slot.as_mut().ok().and_then(Option::take);
                        *slot = Ok(Some(Frame { width:width as u32,height:height as u32,rgba }));
                        rgba = previous.map_or_else(||vec![0;size],|f| f.rgba);
                    };
                    let _ = ended_tx.send(result);
                });
                let result = loop {
                    let input = tokio::select! {
                        end = &mut ended_rx => break end.unwrap_or_else(|_|Err("Capture reader stopped".into())),
                        input = rx.recv() => input,
                    };
                    let state = |pressed| if pressed { KeyState::Pressed } else { KeyState::Released };
                    let result = match input {
                        None => break Ok(()),
                        Some(_) if !control => continue,
                        Some(Input::Pointer(x,y)) => remote.notify_pointer_motion_absolute(&session,node,x,y,Default::default()).await,
                        Some(Input::Button(button,pressed)) => remote.notify_pointer_button(&session,match button { PointerButton::Left=>272,PointerButton::Right=>273,PointerButton::Middle=>274 },state(pressed),Default::default()).await,
                        Some(Input::Scroll(dx,dy)) => remote.notify_pointer_axis(&session,dx*15.,-dy*15.,Default::default()).await,
                        Some(Input::Key(DesktopKey::Code(code),pressed)) => remote.notify_keyboard_keycode(&session,code as i32,state(pressed),Default::default()).await,
                        Some(Input::Key(DesktopKey::Char(ch),pressed)) => remote.notify_keyboard_keysym(&session,if u32::from(ch)<=255 { ch as i32 } else { (u32::from(ch)|0x01000000) as i32 },state(pressed),Default::default()).await,
                    };
                    if let Err(error) = result { break Err(error.to_string()); }
                };
                drop(child);
                let _ = reader.join();
                result
            };
            // Cancellation while the OS consent dialog is open must close that session too.
            let result = tokio::select! { result = run => result, _ = cancelled => Ok(()) };
            let _ = session.close().await;
            result
        });
        if let Err(error) = result { *output.lock() = Err(error); }
    }).map_err(|e|e.to_string())?;
    Ok(Box::new(WaylandSource {
        latest,
        control,
        input: tx,
        cancel: Some(cancel),
    }))
}
impl WaylandSource {
    fn send(&self, input: Input) -> Result<(), String> {
        self.input
            .try_send(input)
            .map_err(|e| format!("Desktop input queue: {e}"))
    }
}
impl DesktopSource for WaylandSource {
    fn control_granted(&self) -> bool {
        self.control.load(Ordering::Acquire)
    }
    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        self.latest
            .lock()
            .as_mut()
            .map(Option::take)
            .map_err(|e| e.clone())
    }
    fn send_pointer(&mut self, x: f64, y: f64) -> Result<(), String> {
        self.send(Input::Pointer(x, y))
    }
    fn send_button(&mut self, b: PointerButton, p: bool) -> Result<(), String> {
        self.send(Input::Button(b, p))
    }
    fn send_scroll(&mut self, x: f64, y: f64) -> Result<(), String> {
        self.send(Input::Scroll(x, y))
    }
    fn send_key(&mut self, k: DesktopKey, p: bool) -> Result<(), String> {
        self.send(Input::Key(k, p))
    }
    fn close(&mut self) {
        self.control.store(false, Ordering::Release);
        *self.latest.lock() = Ok(None);
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}
impl Drop for WaylandSource {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    id: u64,
    title: String,
    token: String,
}

fn choices_path() -> Result<std::path::PathBuf, String> {
    let root = std::env::var_os("SOLARIS_CLIENT_ROOT").map(std::path::PathBuf::from)
        .map(Ok).unwrap_or_else(crate::content_paths::default_root)?;
    Ok(root.join("desktop-windows.json"))
}

fn choices() -> Result<Vec<Choice>, String> {
    let path = choices_path()?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Cannot read saved desktop choices: {e}")),
    };
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 { return Err("Saved desktop choices exceed byte limit".into()); }
    let entries: Vec<Choice> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if entries.len() > 32 || entries.iter().any(|c| c.id == 0 || c.token.len() > 4096) {
        return Err("Invalid saved desktop choices".into());
    }
    Ok(entries)
}

fn save_choice(id: u64, token: &str, width: i32, height: i32) -> Result<(), String> {
    use std::io::Write;
    if token.len() > 4096 { return Err("Portal restore token exceeds byte limit".into()); }
    let mut entries = choices()?;
    if let Some(choice) = entries.iter_mut().find(|choice| choice.id == id) {
        // Portal tokens are single-use: replace the consumed token after every start.
        choice.token = token.into();
    } else {
        let id = entries.iter().map(|choice| choice.id).max().unwrap_or(0)
            .checked_add(1).ok_or("Saved desktop choice id overflow")?;
        if entries.len() == 32 { entries.remove(0); }
        entries.push(Choice { id, title: format!("Saved window {id} ({width}×{height})"), token: token.into() });
    }
    let bytes = serde_json::to_vec(&entries).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 { return Err("Saved desktop choices exceed byte limit".into()); }
    let path = choices_path()?;
    std::fs::create_dir_all(path.parent().ok_or("Missing desktop data directory")?).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(&temporary).map_err(|e| e.to_string())?;
    let result = file.write_all(&bytes).and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temporary, &path)).map_err(|e| e.to_string());
    if result.is_err() { let _ = std::fs::remove_file(temporary); }
    result
}
