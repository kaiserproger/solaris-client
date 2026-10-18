use crate::api::{Command, InputIntent, Request, SharedState};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub(super) fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("OS randomness unavailable: {e}"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(result)
}
pub(super) struct Lease {
    pub session: String,
    pub controller: String,
    pub expires: Instant,
    pub live: Arc<AtomicBool>,
    pub connection: String,
    pub dimension: String,
    pub pending_respawn: Option<Arc<AtomicBool>>,
}
impl Lease {
    pub fn observe_respawn(&mut self, connection: &str, dimension: &str) {
        let expected = self.connection == connection
            && self.dimension != dimension
            && self
                .pending_respawn
                .as_ref()
                .is_some_and(|cancelled| !cancelled.load(Ordering::Acquire))
            && self
                .dimension
                .parse::<u64>()
                .ok()
                .zip(dimension.parse::<u64>().ok())
                .is_some_and(|(old, new)| old.checked_add(1) == Some(new));
        if expected {
            self.dimension.clear();
            self.dimension.push_str(dimension);
            self.pending_respawn = None;
        }
    }
}
#[derive(Default)]
pub(super) struct Control {
    pub lease: Option<Lease>,
    pub releasing: usize,
    pub last_release: Option<String>,
    pub controller_revision: u64,
    pub artifacts: std::collections::BTreeMap<String, std::path::PathBuf>,
}
pub(super) struct Core {
    pub shared: SharedState,
    pub sender: mpsc::SyncSender<Request>,
    pub control: Mutex<Control>,
    pub stopped: AtomicBool,
    pub allow_control: bool,
    pub profile: String,
    pub instance: String,
    pub artifact_dir: std::path::PathBuf,
}
impl Core {
    pub fn tools(&self) -> Vec<&'static Value> {
        let presentation = self
            .shared
            .observation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data["presentation_available"]
            .as_bool()
            .unwrap_or(false);
        super::catalog::tools()
            .iter()
            .filter(|tool| {
                presentation || super::catalog::state_supported(tool["name"].as_str().unwrap())
            })
            .collect()
    }
    pub fn notify(&self) {
        let _guard = self
            .shared
            .observation
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.shared.changed.notify_all();
        (self.shared.owner_wake)();
    }
    /// Drops the current lease and kills its fence. Caller holds the control lock.
    fn take_lease(c: &mut Control, reason: &str) -> bool {
        if let Some(lease) = c.lease.take() {
            c.last_release = Some(reason.into());
            c.controller_revision = c.controller_revision.wrapping_add(1);
            lease.live.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }
    /// Queues the ReleaseAll that must precede any next writer. The caller holds the
    /// control lock, so no admit can interleave; a full owner queue keeps `releasing`
    /// raised until a helper thread has delivered it.
    fn queue_release_all(self: &Arc<Self>, c: &mut Control) {
        let (reply, _receive) = mpsc::sync_channel(1);
        let request = Request {
            command: Command::Input(InputIntent::ReleaseAll),
            reply,
            expected_connection_epoch: None,
            expected_dimension_epoch: None,
            automation_fence: None,
            cancelled: None,
        };
        if let Err(mpsc::TrySendError::Full(request)) = self.sender.try_send(request) {
            c.releasing += 1;
            let core = self.clone();
            std::thread::spawn(move || {
                let _ = core.sender.send(request);
                core.control.lock().releasing -= 1;
                core.notify();
            });
        }
    }
    pub fn invalidate(&self, reason: &str) -> bool {
        Self::take_lease(&mut self.control.lock(), reason)
    }
    pub fn revoke(self: &Arc<Self>, reason: &str) {
        self.revoke_lease(reason, None);
    }
    /// Revokes the current lease, but when `expected` is given only if that exact
    /// lease (fence identity) is still current: a deferred revoke decided from an
    /// earlier check must never hit a newer writer admitted in between.
    pub(super) fn revoke_lease(self: &Arc<Self>, reason: &str, expected: Option<&Arc<AtomicBool>>) {
        {
            let mut c = self.control.lock();
            let current = match (&c.lease, expected) {
                (Some(lease), Some(expected)) => Arc::ptr_eq(&lease.live, expected),
                (Some(_), None) => true,
                (None, _) => false,
            };
            if current {
                Self::take_lease(&mut c, reason);
                self.queue_release_all(&mut c);
            }
        }
        self.notify();
    }
    pub fn controller(&self) -> Value {
        let c = self.control.lock();
        match &c.lease {
            Some(l) if l.expires > Instant::now() => {
                json!({"active":true,"controller":l.controller,
                "remaining_ms":l.expires.saturating_duration_since(Instant::now()).as_millis(),
                "scopes":["observe","control","capture"]})
            }
            _ => json!({"active":false,"last_release_reason":c.last_release}),
        }
    }
    /// Canonical Java arguments carry no private control token. The authenticated
    /// transport session is the writer identity; context/live fences stay owner-side.
    pub fn admit(
        self: &Arc<Self>,
        session: &str,
        connection: &str,
        dimension: &str,
    ) -> Result<Arc<AtomicBool>, String> {
        if !self.allow_control {
            return Err("control_not_locally_authorized".into());
        }
        let mut c = self.control.lock();
        if c.releasing != 0 {
            return Err("controller_releasing".into());
        }
        // A dead session can never renew: its TTL lapsed with no writer
        // action, or its fence was invalidated by an event (revoke,
        // expiry sweeper, manual preemption, session teardown). Reclaim the
        // slot for the live session deterministically on this admit path —
        // no polling thread — so a new MCP session is never fenced forever
        // by a stuck/dead predecessor (swarm: Katya's 20-minute
        // `controller_busy` across fresh sessions). The old fence stays
        // dead, so the owner keeps refusing its in-flight tasks with
        // `controller_lease_revoked` while the new writer proceeds. The
        // predecessor's held inputs are released first, in owner order, so
        // they cannot leak to the new writer.
        let reclaim = c.lease.as_ref().is_some_and(|lease| {
            lease.session != session
                && (lease.expires <= Instant::now() || !lease.live.load(Ordering::Acquire))
        });
        if reclaim {
            if let Some(lease) = c.lease.take() {
                lease.live.store(false, Ordering::Release);
            }
            c.last_release = Some("lease_reclaimed".into());
            self.queue_release_all(&mut c);
            if c.releasing != 0 {
                return Err("controller_releasing".into());
            }
        }
        if let Some(lease) = c.lease.as_mut() {
            if lease.session != session {
                return Err("controller_busy".into());
            }
            if lease.expires <= Instant::now() || !lease.live.load(Ordering::Acquire) {
                return Err("lease_expired".into());
            }
            if lease.connection != connection || lease.dimension != dimension {
                return Err("stale_context".into());
            }
            lease.expires = Instant::now() + Duration::from_secs(120);
            return Ok(lease.live.clone());
        }
        let live = Arc::new(AtomicBool::new(true));
        c.lease = Some(Lease {
            session: session.into(),
            controller: session.into(),
            expires: Instant::now() + Duration::from_secs(120),
            live: live.clone(),
            connection: connection.into(),
            dimension: dimension.into(),
            pending_respawn: None,
        });
        c.controller_revision = c.controller_revision.wrapping_add(1);
        Ok(live)
    }
    /// Watchdog decision under the control lock: the revocation reason plus the fence
    /// of the lease that was checked, or the next expiry deadline.
    pub(super) fn lease_verdict(
        c: &mut Control,
        connection: &str,
        dimension: &str,
    ) -> (Option<(&'static str, Arc<AtomicBool>)>, Option<Instant>) {
        if let Some(lease) = c.lease.as_mut() {
            lease.observe_respawn(connection, dimension);
        }
        match &c.lease {
            Some(l) if l.expires <= Instant::now() => {
                (Some(("lease_expired", l.live.clone())), None)
            }
            Some(l) if l.connection != connection || l.dimension != dimension => {
                (Some(("context_changed", l.live.clone())), None)
            }
            Some(l) => (None, Some(l.expires)),
            None => (None, None),
        }
    }
}
pub(super) fn watchdog(core: Arc<Core>) {
    let mut observed = core
        .shared
        .observation
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    while !core.stopped.load(Ordering::Acquire) {
        let (revoke, deadline) = {
            let mut c = core.control.lock();
            Core::lease_verdict(
                &mut c,
                &observed.connection_epoch,
                &observed.dimension_epoch,
            )
        };
        if let Some((reason, checked)) = revoke {
            drop(observed);
            core.revoke_lease(reason, Some(&checked));
            observed = core
                .shared
                .observation
                .lock()
                .unwrap_or_else(|e| e.into_inner());
        } else if let Some(deadline) = deadline {
            observed = core
                .shared
                .changed
                .wait_timeout(observed, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        } else {
            observed = core
                .shared
                .changed
                .wait(observed)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}
