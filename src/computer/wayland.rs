use super::{DesktopKey, DesktopSource, Frame, PointerButton, WindowInfo};
use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use parking_lot::Mutex;
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
    sync::Arc,
    thread,
};

type Latest = Arc<Mutex<Result<Option<Frame>, String>>>;
struct WaylandSource {
    latest: Latest,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
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
    Ok(vec![WindowInfo {
        id: 0,
        title: "Choose a NEW window in the system permission dialog".into(),
    }])
}
pub fn open(id: u64) -> Result<Box<dyn DesktopSource>, String> {
    list_windows()?;
    if id != 0 { return Err("Select NEW window for fresh system consent".into()); }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let latest: Latest = Arc::new(Mutex::new(Ok(None)));
    let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
    let output = latest.clone();
    thread::Builder::new().name("desktop-portal".into()).spawn(move || {
        if !matches!(cancelled.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)) {
            return;
        }
        let result = runtime.block_on(async {
            // ashpd::new() caches a process-global connection whose executor
            // belonged to the first capture's now-dropped Tokio runtime.
            let connection = portal_connection().await?;
            let result = async {
            let cast = Screencast::with_connection(connection.clone()).await.map_err(|e|format!("ScreenCast proxy: {e}"))?;
            let session = cast.create_session(Default::default()).await.map_err(|e|format!("CreateSession: {e}"))?;
            let run = async {
                cast.select_sources(&session, SelectSourcesOptions::default().set_sources(Some(SourceType::Window.into()))
                    .set_multiple(false).set_cursor_mode(CursorMode::Embedded).set_persist_mode(PersistMode::DoNot))
                    .await.map_err(|e|format!("SelectSources: {e}"))?.response().map_err(|e|format!("SelectSources: {e}"))?;
                let selected = cast.start(&session, None, Default::default()).await.map_err(|e|format!("Start consent dialog: {e}"))?.response().map_err(|e|format!("Start consent dialog: {e}"))?;
                let stream = selected.streams().first().ok_or("Portal returned no window")?;
                let node = stream.pipe_wire_node_id();
                let (width,height) = stream.size().ok_or("Portal did not supply capture dimensions")?;
                if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) { return Err("Desktop window dimensions exceed 8192".into()); }
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
                let (ended_tx, ended_rx) = tokio::sync::oneshot::channel();
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
                let result = ended_rx.await.unwrap_or_else(|_|Err("Capture reader stopped".into()));
                drop(child);
                let _ = reader.join();
                result
            };
            // Cancellation while the OS consent dialog is open must close that session too.
            let result = tokio::select! { result = run => result, _ = cancelled => Ok(()) };
            let closed = session.close().await.map_err(|e|format!("Close desktop session: {e}"));
            result.and(closed)
            }.await;
            let disconnected = connection.close().await.map_err(|e|format!("Close desktop connection: {e}"));
            result.and(disconnected)
        });
        if let Err(error) = result { *output.lock() = Err(error); }
    }).map_err(|e|e.to_string())?;
    Ok(Box::new(WaylandSource {
        latest,
        cancel: Some(cancel),
    }))
}
impl DesktopSource for WaylandSource {
    fn control_granted(&self) -> bool {
        false
    }
    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        self.latest
            .lock()
            .as_mut()
            .map(Option::take)
            .map_err(|e| e.clone())
    }
    fn send_pointer(&mut self, _: f64, _: f64) -> Result<(), String> {
        Err("Portal capture is view-only; launch an app in the monitor for control".into())
    }
    fn send_button(&mut self, _: PointerButton, _: bool) -> Result<(), String> {
        Err("Portal capture is view-only; launch an app in the monitor for control".into())
    }
    fn send_scroll(&mut self, _: f64, _: f64) -> Result<(), String> {
        Err("Portal capture is view-only; launch an app in the monitor for control".into())
    }
    fn send_key(&mut self, _: DesktopKey, _: bool) -> Result<(), String> {
        Err("Portal capture is view-only; launch an app in the monitor for control".into())
    }
    fn close(&mut self) {
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


async fn portal_connection() -> Result<ashpd::zbus::Connection, String> {
    ashpd::zbus::Connection::session().await.map_err(|e|format!("Connect desktop portal: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "run under dbus-run-session for an isolated real bus"]
    fn reopening_capture_uses_a_live_connection_after_runtime_drop() {
        let connect = || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                let connection = portal_connection().await.unwrap();
                let name = connection.unique_name().unwrap().to_string();
                let bus = ashpd::zbus::fdo::DBusProxy::new(&connection).await.unwrap();
                bus.get_id().await.unwrap();
                drop(bus);
                connection.close().await.unwrap();
                name
            })
        };
        assert_ne!(connect(), connect());
    }
}
