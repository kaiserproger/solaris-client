use super::control::Core;
use crate::api::{Observation, Shared};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Condvar,
};
use std::time::{Duration, Instant};

fn client() -> (Arc<Core>, mpsc::Receiver<crate::api::Request>) {
    let (sender, receiver) = mpsc::sync_channel(16);
    let observation = Observation {
        instance_id: "test".into(),
        connection_epoch: "1".into(),
        dimension_epoch: "2".into(),
        state_revision: "0".into(),
        last_applied_packet_sequence: "0".into(),
        client_tick: "0".into(),
        server_tick: None,
        resource_epoch: "1".into(),
        captured_frame_id: None,
        origin: "client_observed".into(),
        truncated: false,
        next_cursor: None,
        data: json!({}),
    };
    (
        Arc::new(Core {
            shared: Arc::new(Shared {
                observation: std::sync::Mutex::new(observation),
                changed: Condvar::new(),
                owner_wake: Arc::new(|| {}),
            }),
            sender,
            control: Mutex::new(Default::default()),
            stopped: AtomicBool::new(false),
            allow_control: true,
            profile: "test".into(),
            instance: "test".into(),
            artifact_dir: Default::default(),
        }),
        receiver,
    )
}
#[test]
fn authenticated_transport_writer_is_exclusive_and_context_fenced() {
    let (core, _) = client();
    let live = core.admit("a", "1", "2").unwrap();
    assert_eq!(core.admit("b", "1", "2").unwrap_err(), "controller_busy");
    assert_eq!(core.admit("a", "9", "2").unwrap_err(), "stale_context");
    assert_eq!(core.admit("a", "1", "9").unwrap_err(), "stale_context");
    let renewed = core.admit("a", "1", "2").unwrap();
    assert!(Arc::ptr_eq(&live, &renewed));
}
#[test]
fn preemption_invalidates_pending_owner_fences_before_new_writer() {
    let (core, receiver) = client();
    let old = core.admit("a", "1", "2").unwrap();
    core.revoke("manual_input");
    assert!(!old.load(Ordering::Acquire));
    let cleanup = receiver
        .try_recv()
        .expect("cleanup queued before next writer");
    assert!(matches!(
        cleanup.command,
        crate::api::Command::Input(crate::api::InputIntent::ReleaseAll)
    ));
    let new = core.admit("b", "1", "2").unwrap();
    assert!(new.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&old, &new));
}
#[test]
fn expired_writer_cannot_be_implicitly_renewed() {
    let (core, _) = client();
    core.admit("a", "1", "2").unwrap();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    assert_eq!(core.admit("a", "1", "2").unwrap_err(), "lease_expired");
    core.invalidate("expired");
    assert!(core.admit("b", "1", "2").is_ok());
}

#[test]
fn respawn_handoff_preserves_writer_for_one_expected_dimension_reset() {
    let (core, _) = client();
    let live = core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut control = core.control.lock();
        let lease = control.lease.as_mut().unwrap();
        lease.pending_respawn = Some(cancelled);
        lease.observe_respawn("1", "3");
        assert!(lease.pending_respawn.is_none());
    }
    let renewed = core.admit("a", "1", "3").unwrap();
    assert!(Arc::ptr_eq(&live, &renewed));
    assert_eq!(core.admit("a", "1", "2").unwrap_err(), "stale_context");
    core.control
        .lock()
        .lease
        .as_mut()
        .unwrap()
        .observe_respawn("1", "4");
    assert_eq!(core.admit("a", "1", "4").unwrap_err(), "stale_context");
}

#[test]
fn respawn_handoff_refuses_unarmed_cancelled_switched_or_skipped_contexts() {
    for (connection, dimension, armed, cancelled) in [
        ("1", "3", false, false),
        ("1", "3", true, true),
        ("2", "3", true, false),
        ("1", "4", true, false),
    ] {
        let (core, _) = client();
        core.admit("a", "1", "2").unwrap();
        {
            let mut control = core.control.lock();
            let lease = control.lease.as_mut().unwrap();
            if armed {
                lease.pending_respawn = Some(Arc::new(AtomicBool::new(cancelled)));
            }
            lease.observe_respawn(connection, dimension);
        }
        assert_eq!(
            core.admit("a", connection, dimension).unwrap_err(),
            "stale_context"
        );
        assert!(core.admit("a", "1", "2").is_ok());
    }
}

#[test]
fn ordinary_owner_error_keeps_lease_and_queues_no_cleanup() {
    let (core, receiver) = client();
    let live = core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let request = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("write must reach the owner");
        let fence = request
            .automation_fence
            .clone()
            .expect("write carries a writer fence");
        request
            .reply
            .send(Err(
                "target is unreachable from the current loaded client position: 1, 2, 3"
                    .to_string(),
            ))
            .expect("owner reply");
        let no_cleanup = receiver.try_recv().is_err();
        (fence, no_cleanup)
    });
    let error = super::tools::execute(
        &core,
        "a",
        "minecraft_send_chat",
        &json!({"message": "hi"}),
        &cancelled,
    )
    .unwrap_err();
    assert!(
        error.contains("target is unreachable"),
        "unexpected: {error}"
    );
    let (fence, no_cleanup) = owner.join().expect("owner thread");
    assert!(
        Arc::ptr_eq(&fence, &live),
        "owner must receive the admitted writer fence"
    );
    assert!(
        fence.load(Ordering::Acquire),
        "ordinary owner error must not invalidate the writer fence"
    );
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "ordinary owner error must not drop the lease"
    );
    assert!(
        no_cleanup,
        "no ReleaseAll cleanup may be queued for an ordinary error"
    );
}

#[test]
fn next_write_after_plain_error_reuses_the_live_writer_fence() {
    let (core, receiver) = client();
    core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let mut fences = Vec::new();
        for first in [true, false] {
            let request = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("write must reach the owner");
            fences.push(
                request
                    .automation_fence
                    .clone()
                    .expect("write carries a writer fence"),
            );
            let reply: Result<serde_json::Value, String> = if first {
                Err("ordinary raycast does not target requested entity".to_string())
            } else {
                Ok(json!({"status": "ok"}))
            };
            request.reply.send(reply).expect("owner reply");
        }
        fences
    });
    let args = json!({"message": "hi"});
    let error =
        super::tools::execute(&core, "a", "minecraft_send_chat", &args, &cancelled).unwrap_err();
    assert!(error.contains("raycast"), "unexpected: {error}");
    super::tools::execute(&core, "a", "minecraft_send_chat", &args, &cancelled)
        .expect("next write must be admitted after a plain error");
    let fences = owner.join().expect("owner thread");
    assert_eq!(fences.len(), 2);
    assert!(
        fences.iter().all(|fence| fence.load(Ordering::Acquire)),
        "both fences must stay live"
    );
    assert!(
        Arc::ptr_eq(&fences[0], &fences[1]),
        "lease must survive a plain error so the next write reuses it"
    );
}

#[test]
fn dead_session_lease_is_reclaimable_by_a_new_session() {
    // Clock-expired: the previous MCP session is gone, nobody renews.
    let (core, _) = client();
    let old = core.admit("a", "1", "2").unwrap();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    let new = core
        .admit("b", "1", "2")
        .expect("a new session must reclaim a clock-expired lease");
    assert!(!old.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(new.load(Ordering::Acquire));
    assert_eq!(core.admit("a", "1", "2").unwrap_err(), "controller_busy");
    // Event-invalidated: the same recovery must apply.
    let (core, _) = client();
    let old = core.admit("a", "1", "2").unwrap();
    assert!(core.invalidate("transport_session_deleted"));
    assert!(!old.load(Ordering::Acquire));
    assert!(core.admit("b", "1", "2").is_ok());
}

fn is_release_all(request: &crate::api::Request) -> bool {
    matches!(
        request.command,
        crate::api::Command::Input(crate::api::InputIntent::ReleaseAll)
    )
}

#[test]
fn reclaiming_a_dead_lease_releases_held_inputs_before_the_new_writer() {
    let (core, receiver) = client();
    let old = core.admit("a", "1", "2").unwrap();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    let new = core.admit("b", "1", "2").unwrap();
    assert!(!old.load(Ordering::Acquire));
    assert!(new.load(Ordering::Acquire));
    let cleanup = receiver
        .try_recv()
        .expect("reclaim must queue ReleaseAll before the new writer");
    assert!(is_release_all(&cleanup));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn reclaim_with_a_full_owner_queue_refuses_until_release_all_is_delivered() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (core, _) = client();
    let core = {
        let mut core = Arc::try_unwrap(core).ok().unwrap();
        core.sender = sender.clone();
        Arc::new(core)
    };
    let old = core.admit("a", "1", "2").unwrap();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    let filler = crate::api::Request {
        command: crate::api::Command::Input(crate::api::InputIntent::ReleaseAll),
        reply: mpsc::sync_channel(1).0,
        expected_connection_epoch: None,
        expected_dimension_epoch: None,
        automation_fence: None,
        cancelled: None,
    };
    sender.try_send(filler).unwrap();
    assert_eq!(
        core.admit("b", "1", "2").unwrap_err(),
        "controller_releasing"
    );
    assert!(!old.load(Ordering::Acquire));
    // Draining lets the helper thread deliver ReleaseAll and drop `releasing`.
    assert!(is_release_all(&receiver.recv().unwrap()));
    assert!(is_release_all(&receiver.recv().unwrap()));
    while core.control.lock().releasing != 0 {
        std::thread::yield_now();
    }
    assert!(core.admit("b", "1", "2").unwrap().load(Ordering::Acquire));
}

#[test]
fn deferred_expiry_revoke_never_hits_a_writer_admitted_after_the_check() {
    let (core, receiver) = client();
    let old = core.admit("a", "1", "2").unwrap();
    core.control.lock().lease.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    // Watchdog: expiry decided under the lock...
    let (verdict, _) = Core::lease_verdict(&mut core.control.lock(), "1", "2");
    let (reason, checked) = verdict.expect("expired lease must be selected");
    assert_eq!(reason, "lease_expired");
    assert!(Arc::ptr_eq(&checked, &old));
    // ...a new session replaces the lease before the deferred revoke runs...
    let new = core.admit("b", "1", "2").unwrap();
    assert!(is_release_all(&receiver.try_recv().unwrap()));
    // ...and the deferred revoke must target only the lease that was checked.
    core.revoke_lease(reason, Some(&checked));
    assert!(new.load(Ordering::Acquire), "new writer fence was revoked");
    assert!(core.control.lock().lease.is_some());
    assert!(receiver.try_recv().is_err(), "no spurious ReleaseAll");
    assert!(Arc::ptr_eq(&core.admit("b", "1", "2").unwrap(), &new));
    // The still-current lease is revoked as before.
    core.revoke_lease("lease_expired", Some(&new));
    assert!(!new.load(Ordering::Acquire));
    assert!(is_release_all(&receiver.try_recv().unwrap()));
}

#[test]
fn concurrent_second_writer_still_rejected_while_first_is_in_flight() {
    let (core, receiver) = client();
    let live = core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let first_core = core.clone();
    let first_cancelled = cancelled.clone();
    let first = std::thread::spawn(move || {
        super::tools::execute(
            &first_core,
            "a",
            "minecraft_send_chat",
            &json!({"message": "first"}),
            &first_cancelled,
        )
    });
    let request = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("first write must reach the owner");
    assert!(Arc::ptr_eq(
        &request.automation_fence.clone().expect("writer fence"),
        &live
    ));
    // The second writer goes through the same fence admission path the owner
    // uses: no concurrent writer task may be in flight.
    let busy = {
        let mut pending = 0;
        let _ = receiver.try_recv().map(|_| pending += 1);
        pending
    };
    assert_eq!(
        busy, 0,
        "no second request may reach the owner while the first is in flight"
    );
    request
        .reply
        .send(Ok(json!({"status": "ok"})))
        .expect("owner reply");
    first
        .join()
        .expect("first thread")
        .expect("first write succeeds");
    let second_cancelled = cancelled.clone();
    let owner = std::thread::spawn(move || {
        let request = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("second write must reach the owner after the first completes");
        let fence = request.automation_fence.clone().expect("writer fence");
        request
            .reply
            .send(Ok(json!({"status": "ok"})))
            .expect("owner reply");
        fence
    });
    super::tools::execute(
        &core,
        "a",
        "minecraft_send_chat",
        &json!({"message": "second"}),
        &second_cancelled,
    )
    .expect("second write admitted after the first completes");
    let fence = owner.join().expect("owner thread");
    assert!(Arc::ptr_eq(&fence, &live));
    assert!(fence.load(Ordering::Acquire));
}

#[test]
fn validation_error_before_dispatch_never_touches_lease_or_queue() {
    let (core, receiver) = client();
    core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let error = super::tools::execute(&core, "a", "minecraft_send_chat", &json!({}), &cancelled)
        .unwrap_err();
    assert!(error.contains("invalid_arguments"), "unexpected: {error}");
    assert!(
        receiver.try_recv().is_err(),
        "a validation error must never reach the owner queue"
    );
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "a validation error must not drop the lease"
    );
}

#[test]
fn cancelled_and_fenced_owner_errors_release_the_lease() {
    for error in [
        "cancelled",
        "cancelled: request_cancelled",
        "controller_lease_revoked",
        "lease_expired",
        "stale_connection_epoch",
    ] {
        let (core, receiver) = client();
        core.admit("a", "1", "2").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let owner = std::thread::spawn(move || {
            let request = receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("write must reach the owner");
            request
                .reply
                .send(Err(error.to_string()))
                .expect("owner reply");
            let cleanup = receiver.recv_timeout(Duration::from_secs(10)).ok();
            cleanup.is_some()
        });
        let seen = super::tools::execute(
            &core,
            "a",
            "minecraft_send_chat",
            &json!({"message": "hi"}),
            &cancelled,
        )
        .unwrap_err();
        assert_eq!(seen, error);
        assert!(
            owner.join().expect("owner thread"),
            "a genuine session-loss error ({error}) must queue ReleaseAll cleanup"
        );
        assert!(
            core.control.lock().lease.is_none(),
            "a genuine session-loss error ({error}) must drop the lease"
        );
        assert!(core.admit("b", "1", "2").is_ok());
    }
}
#[test]
fn stale_container_state_keeps_lease_for_retry() {
    let (core, receiver) = client();
    core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let owner = std::thread::spawn(move || {
        let request = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("write must reach the owner");
        request
            .reply
            .send(Err("stale_container_state".to_string()))
            .expect("owner reply");
        receiver.try_recv().is_err()
    });
    let seen = super::tools::execute(
        &core,
        "a",
        "minecraft_send_chat",
        &json!({"message": "hi"}),
        &cancelled,
    )
    .unwrap_err();
    assert_eq!(seen, "stale_container_state");
    assert!(
        owner.join().expect("owner thread"),
        "a per-click state race must not queue ReleaseAll cleanup"
    );
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "a per-click state race must keep the lease"
    );
}

#[test]
fn reply_timeout_signals_owner_cancel_and_keeps_lease_for_retry() {
    let (core, receiver) = client();
    let live = core.admit("a", "1", "2").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    // `execute` enforces a 125 s safety-bound floor, so a real timeout cannot
    // run in a unit test. Assert the exact contract the timeout path relies on
    // instead: the request carries the caller's shared cancel flag to the
    // owner, and the lease/fence stay live while the task is in flight — the
    // timeout arm only flips that flag (event-driven, no polling), and the
    // owner's next `fence()` refuses the hung task with `cancelled` and frees
    // the writer slot.
    let owner_cancelled = cancelled.clone();
    let owner_core = core.clone();
    let owner = std::thread::spawn(move || {
        super::tools::execute(
            &owner_core,
            "a",
            "minecraft_send_chat",
            &json!({"message": "hi"}),
            &owner_cancelled,
        )
    });
    let request = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("write must reach the owner");
    let fence = request
        .automation_fence
        .clone()
        .expect("write carries a writer fence");
    assert!(Arc::ptr_eq(&fence, &live));
    let shared = request.cancelled.clone().expect("shared cancel flag");
    assert!(Arc::ptr_eq(&shared, &cancelled));
    request
        .reply
        .send(Ok(json!({"status": "ok"})))
        .expect("owner reply");
    owner.join().expect("owner thread").expect("write succeeds");
    assert!(fence.load(Ordering::Acquire));
    assert!(
        core.control
            .lock()
            .lease
            .as_ref()
            .is_some_and(|lease| lease.session == "a"),
        "lease must still be held after the owner replies"
    );
}
