//! Singleplayer session glue: start the local Solaris server for a world, join
//! it, apply first-join world settings, report a crashed server, and stop the
//! server (which checkpoints the world) whenever the connection ends.
use super::*;
use crate::singleplayer::{self, Cancel, Launch, Server, WorldMeta};
use std::thread::JoinHandle;

pub(super) enum Msg {
    Progress(String),
    Done(std::result::Result<(Server, WorldMeta), String>),
}

pub(super) struct Start {
    rx: mpsc::Receiver<Msg>,
    cancel: Cancel,
    dir_name: String,
    username: String,
}

/// First-join settings the server cannot take from its config file.
pub(super) struct Setup {
    dir_name: String,
    meta: WorldMeta,
}

/// Stops servers off the UI thread, one after another, and always finishes
/// the last stop before the session goes away so the world is checkpointed.
/// A stop that fails (forced kill, non-zero exit) is reported through `failures`.
pub(super) struct Stopper {
    handle: Option<JoinHandle<()>>,
    report: mpsc::Sender<String>,
    failures: mpsc::Receiver<String>,
}

impl Default for Stopper {
    fn default() -> Self {
        let (report, failures) = mpsc::channel();
        Self {
            handle: None,
            report,
            failures,
        }
    }
}

impl Stopper {
    fn stop(&mut self, mut server: Server) {
        let previous = self.handle.take();
        let report = self.report.clone();
        self.handle = thread::Builder::new()
            .name("singleplayer-stop".into())
            .spawn(move || {
                if let Some(previous) = previous {
                    let _ = previous.join();
                }
                if let Err(failure) = server.stop() {
                    eprintln!("singleplayer: {failure}");
                    let _ = report.send(failure);
                }
            })
            .ok();
    }
}

impl Drop for Stopper {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Session {
    /// Press Play: start the world's server in the background, then join it.
    pub fn start_local_world(&mut self, dir_name: &str) -> Result<()> {
        if self.local_start.is_some() {
            return Err("a singleplayer world is already starting".into());
        }
        let username = self
            .ui
            .as_ref()
            .map(|ui| ui.title_defaults().1)
            .unwrap_or_else(|| "Player".into());
        let root = singleplayer::data_root()?;
        let view_distance = self
            .render_distance
            .clamp(UiState::MIN_RENDER_DISTANCE, UiState::MAX_RENDER_DISTANCE)
            as u8;
        let previous_stop = self.local_stopper.handle.take();
        let (cancel, events) = singleplayer::start_channel();
        let (tx, rx) = mpsc::channel();
        let wake = self.wake_callback();
        let thread_cancel = cancel.clone();
        let (world, user) = (dir_name.to_owned(), username.clone());
        thread::Builder::new()
            .name("singleplayer-start".into())
            .spawn(move || {
                let send = |msg| {
                    let _ = tx.send(msg);
                    wake();
                };
                if let Some(previous) = previous_stop {
                    send(Msg::Progress("Saving the previous world".into()));
                    let _ = previous.join();
                }
                let progress = |text: String| send(Msg::Progress(text));
                let result = singleplayer::start(
                    &Launch {
                        root: &root,
                        world_dir: &world,
                        username: &user,
                        view_distance,
                    },
                    &thread_cancel,
                    events,
                    &progress,
                );
                send(Msg::Done(result));
            })
            .map_err(|e| e.to_string())?;
        self.local_start = Some(Start {
            rx,
            cancel,
            dir_name: dir_name.to_owned(),
            username,
        });
        self.changed_state();
        Ok(())
    }

    pub fn cancel_local_start(&mut self) {
        if let Some(start) = &self.local_start {
            start.cancel.cancel();
        }
    }

    /// Hand the running server to the stopper (graceful stop + checkpoint).
    pub(super) fn release_local_server(&mut self) {
        // A start still in flight winds down by itself: its result is dropped
        // with the receiver, and dropping a `Server` stops it gracefully.
        if let Some(start) = self.local_start.take() {
            start.cancel.cancel();
        }
        self.local_setup = None;
        if let Some(server) = self.local_server.take() {
            self.local_stopper.stop(server);
        }
        if let Some(ui) = &mut self.ui {
            ui.singleplayer_active = false;
        }
    }

    /// Crash report when the local server is gone. Right after a dropped
    /// connection the exit may still be in flight, so give it a moment.
    pub(super) fn local_server_report(&mut self) -> Option<String> {
        let server = self.local_server.as_mut()?;
        server.exited_within(std::time::Duration::from_millis(1500))
    }

    /// Called from `process_events`: start progress, crash watch, first-join setup.
    pub(super) fn poll_local_server(&mut self) -> bool {
        let mut changed = false;
        while let Some(msg) = self
            .local_start
            .as_ref()
            .and_then(|start| start.rx.try_recv().ok())
        {
            changed = true;
            match msg {
                Msg::Progress(text) => {
                    if let Some(ui) = &mut self.ui {
                        ui.set_local_status(text);
                    }
                }
                Msg::Done(result) => {
                    let start = self.local_start.take().expect("start in progress");
                    self.finish_local_start(start, result);
                }
            }
        }
        if let Some(report) = self.local_server.as_mut().and_then(Server::exited) {
            changed = true;
            self.fail_local(report);
        } else if self.local_start.is_none()
            && self.local_server.is_none()
            && let Some(report) = self.local_stopper.failures.try_recv().ok()
        {
            // An explicit Save & Quit whose server had to be killed or exited badly.
            changed = true;
            self.fail_local(report);
        }
        if self.phase == "play"
            && self.player.pose_known
            && let Some(setup) = self.local_setup.take()
        {
            changed = true;
            if let Err(error) = self.apply_world_setup(&setup) {
                self.event(json!({"type":"singleplayer_setup_failed","error":error}));
            }
        }
        changed
    }

    fn finish_local_start(
        &mut self,
        start: Start,
        result: std::result::Result<(Server, WorldMeta), String>,
    ) {
        let cancelled = start.cancel.cancelled();
        let (server, meta) = match result {
            Ok(ok) if !cancelled => ok,
            other => {
                // Dropping a live server here stops it gracefully.
                let error = other.err().filter(|e| e != "Cancelled");
                if let Some(ui) = &mut self.ui {
                    ui.local_start_ended(error);
                }
                self.changed_state();
                return;
            }
        };
        let address = format!("127.0.0.1:{}", server.port);
        match self.connect(&address, &start.username) {
            Ok(_) => {
                self.local_setup = meta.setup_pending.then_some(Setup {
                    dir_name: start.dir_name,
                    meta,
                });
                self.local_server = Some(server);
            }
            Err(error) => {
                self.local_stopper.stop(server);
                if let Some(ui) = &mut self.ui {
                    ui.local_start_ended(Some(error));
                }
            }
        }
    }

    /// Ordinary operator commands: the server has no per-world mode config.
    fn apply_world_setup(&mut self, setup: &Setup) -> Result<()> {
        self.send_chat(&format!("gamemode {}", setup.meta.game_mode.name()), true)?;
        self.send_chat(
            &format!("difficulty {}", setup.meta.difficulty.name()),
            true,
        )?;
        let root = singleplayer::data_root()?;
        let mut meta = setup.meta.clone();
        meta.setup_pending = false;
        singleplayer::write_meta(
            &singleplayer::worlds_dir(&root).join(&setup.dir_name),
            &meta,
        )
    }

    /// Show a server failure on the disconnect screen and tear the session down.
    pub(super) fn fail_local(&mut self, report: String) {
        self.event(json!({"type":"singleplayer_server_stopped","report":report}));
        eprintln!("singleplayer: {report}");
        self.error = Some(report.clone());
        self.disconnect();
        self.phase = "error".into();
        if let Some(ui) = &mut self.ui
            && self.title_allowed
        {
            ui.show_disconnected_titled(Some("Singleplayer Server Stopped"), report);
        }
        self.changed_state();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("solaris-sp-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn disconnect_stops_the_server_and_dropping_the_session_waits_for_its_checkpoint() {
        let root = temp_root("stop");
        // The "checkpoint" takes a moment: only a stop that waits sees the file.
        let server = singleplayer::spawn_fake_server(
            &root,
            "trap 'sleep 0.3; echo done > \"$PWD/checkpoint\"; exit 0' TERM\n\
             echo 'Solaris is listening addr=127.0.0.1:41240'\n\
             while :; do sleep 1 & wait $!; done",
        )
        .unwrap();
        let mut session = Session::new_mode("local-stop".into(), false);
        session.local_server = Some(server);
        session.disconnect();
        assert!(session.local_server.is_none());
        drop(session);
        assert_eq!(
            std::fs::read_to_string(root.join("checkpoint")).unwrap().trim(),
            "done"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_crashed_server_ends_the_session_with_its_log_tail() {
        let root = temp_root("crash");
        let server = singleplayer::spawn_fake_server(
            &root,
            "echo 'Solaris is listening addr=127.0.0.1:41241'\necho 'panic: tick failed' >&2\nexit 7",
        )
        .unwrap();
        let mut session = Session::new_mode("local-crash".into(), false);
        session.local_server = Some(server);
        // The script exits on its own; poll until the exit becomes visible.
        for _ in 0..1_000_000 {
            if session.phase() == "error" {
                break;
            }
            session.poll_local_server();
            std::thread::yield_now();
        }
        assert_eq!(session.phase(), "error");
        let error = session.error.clone().expect("crash reason is recorded");
        assert!(error.contains("stopped unexpectedly"), "{error}");
        assert!(error.contains("panic: tick failed"), "{error}");
        assert!(session.local_server.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_failed_save_and_quit_shows_the_server_error_screen() {
        let root = temp_root("badsave");
        let server = singleplayer::spawn_fake_server(
            &root,
            "echo 'Solaris is listening addr=127.0.0.1:41242'\n\
             trap 'echo disk full >&2; exit 5' TERM\n\
             while :; do sleep 1 & wait $!; done",
        )
        .unwrap();
        let mut session = Session::new_mode("local-badsave".into(), false);
        session.local_server = Some(server);
        session.disconnect();
        assert!(session.local_server.is_none());
        // The stop runs on its own thread; poll until its failure is reported.
        for _ in 0..10_000_000 {
            if session.phase() == "error" {
                break;
            }
            session.poll_local_server();
            std::thread::yield_now();
        }
        assert_eq!(session.phase(), "error");
        let error = session.error.clone().expect("stop failure is recorded");
        assert!(error.contains("may not be fully saved"), "{error}");
        assert!(error.contains("disk full"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }
}
