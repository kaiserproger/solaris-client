use super::{
    catalog,
    control::{nonce, Core},
};
use crate::api::{Command, Request};
use serde_json::Value;
use std::{
    path::{Component, Path},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

struct RespawnFence<'a> {
    core: &'a Core,
    cancelled: &'a Arc<AtomicBool>,
}

impl Drop for RespawnFence<'_> {
    fn drop(&mut self) {
        let observed = self
            .core
            .shared
            .observation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(lease) = self.core.control.lock().lease.as_mut() {
            if lease
                .pending_respawn
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(pending, self.cancelled))
            {
                // Complete the authorized handoff even if the owner reply won
                // the race with the watchdog's context-change notification.
                lease.observe_respawn(&observed.connection_epoch, &observed.dimension_epoch);
                lease.pending_respawn = None;
            }
        }
    }
}

pub(super) fn execute(
    core: &Arc<Core>,
    session: &str,
    name: &str,
    args: &Value,
    cancelled: &Arc<AtomicBool>,
) -> Result<Value, String> {
    let definition = catalog::validate(name, args)?;
    if name == "client_capture"
        && !args["name"].as_str().is_some_and(|name| {
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
    {
        return Err("invalid_artifact_name".into());
    }
    if cancelled.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    let observation = core
        .shared
        .observation
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if observation.data["presentation_available"] != true && !catalog::state_supported(name) {
        return Err(format!("unknown_tool: {name}"));
    }
    let readonly = definition["annotations"]["readOnlyHint"] == true;
    let lifecycle = matches!(
        name,
        "minecraft_connect"
            | "minecraft_disconnect"
            | "minecraft_wait_for_play"
            | "minecraft_wait_for_state_change"
    );
    let fence = if readonly {
        None
    } else {
        if observation
            .data
            .pointer("/window/focused")
            .and_then(Value::as_bool)
            == Some(false)
        {
            return Err("window_not_focused".into());
        }
        Some(core.admit(
            session,
            &observation.connection_epoch,
            &observation.dimension_epoch,
        )?)
    };
    let connection = if lifecycle {
        None
    } else {
        Some(observation.connection_epoch.clone())
    };
    let dimension = if lifecycle || name == "minecraft_respawn" {
        None
    } else {
        Some(observation.dimension_epoch.clone())
    };
    drop(observation);
    let command = match name {
        "client_capture" => Command::Capture {
            name: args["name"].as_str().ok_or("name_required")?.into(),
            destination: None,
        },
        "minecraft_screenshot" => {
            let supplied = Path::new(args["path"].as_str().ok_or("path_required")?);
            if supplied
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            {
                return Err("screenshot_path_escape".into());
            }
            let target = if supplied.is_absolute() {
                supplied.to_path_buf()
            } else {
                core.artifact_dir.join(supplied)
            };
            let parent = target.parent().ok_or("screenshot_parent_required")?;
            if !parent.starts_with(&core.artifact_dir)
                || !parent
                    .components()
                    .any(|part| part.as_os_str() == "screenshots")
            {
                return Err("screenshot_requires_artifact_screenshots_directory".into());
            }
            // Create only within the artifact sandbox and reject symlink escapes.
            let mut directory = core.artifact_dir.clone();
            for part in parent
                .strip_prefix(&core.artifact_dir)
                .map_err(|_| "screenshot_path_escape")?
                .components()
            {
                if let Component::Normal(part) = part {
                    directory.push(part);
                    match std::fs::create_dir(&directory) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.to_string()),
                    }
                    if std::fs::symlink_metadata(&directory)
                        .map_err(|e| e.to_string())?
                        .file_type()
                        .is_symlink()
                    {
                        return Err("screenshot_symlink_refused".into());
                    }
                }
            }
            if target.extension().and_then(|s| s.to_str()) != Some("png") {
                return Err("screenshot_png_required".into());
            }
            Command::Capture {
                name: nonce()?,
                destination: Some(target),
            }
        }
        _ => Command::Minecraft {
            name: name.into(),
            arguments: args.clone(),
        },
    };
    // Respawn authorizes one server dimension-epoch reset, not a connection
    // switch. The live writer/cancellation fences remain active across it.
    let _respawn_fence = if name == "minecraft_respawn" {
        let mut control = core.control.lock();
        let lease = control.lease.as_mut().ok_or("lease_expired")?;
        if lease.session != session {
            return Err("controller_busy".into());
        }
        if lease.pending_respawn.is_some() {
            return Err("writer_action_busy".into());
        }
        lease.pending_respawn = Some(cancelled.clone());
        Some(RespawnFence { core, cancelled })
    } else {
        None
    };
    let (reply, receive) = mpsc::sync_channel(1);
    core.sender
        .try_send(Request {
            command,
            reply,
            expected_connection_epoch: connection,
            expected_dimension_epoch: dimension,
            automation_fence: fence,
            cancelled: Some(cancelled.clone()),
        })
        .map_err(|_| "command_queue_full")?;
    core.notify();
    // Safety bound only. The real owner replies from event predicates, client-clock
    // duration, GPU completion, or cancellation; no observer polling worker exists.
    let reply_bound = args["timeout_seconds"].as_f64().unwrap_or(120.).max(120.) + 5.;
    let result = match receive.recv_timeout(Duration::from_secs_f64(reply_bound)) {
        Ok(result) => result,
        Err(_) => {
            // Reply-bound timeout: the transport caller is gone, but the owner
            // task may still sit in `pending_tasks` holding the single writer
            // slot (`writer_action_busy` for every later write). Signal
            // cancellation on the shared flag — event-driven, no polling — so
            // the owner's next event tick cancels the task, drops the request,
            // and frees the writer slot. The lease itself survives (see
            // `revoke_on_owner_error`): the next write is admitted as soon as
            // the owner drains, instead of being fenced by a revoked lease.
            // A late commit still converges via server packets (Petya's
            // phantom-table case); agents must observe before retrying.
            cancelled.store(true, Ordering::Release);
            core.notify();
            return Err("owner_result_unknown".into());
        }
    };
    if let Ok(value) = &result {
        if value["gpu_completion"] == true {
            if let (Some(uri), Some(path)) =
                (value["artifact_uri"].as_str(), value["path"].as_str())
            {
                let mut control = core.control.lock();
                if control.artifacts.len() >= 512 {
                    control.artifacts.pop_first();
                }
                control.artifacts.insert(uri.into(), path.into());
            }
        }
    }
    // Fail-closed but retry-friendly: only genuine session/owner loss may drop the
    // lease (the owner fence is already dead, so the dead `automation_fence` handle
    // and the `controller_lease_revoked` owner refusal are the authoritative
    // signal). Ordinary validation errors, unreachable targets, brittle raycasts,
    // state races (`stale_*`), and wait-predicate timeouts leave the lease and the
    // live writer fence untouched: agents retry those constantly (swarm: Katya's
    // ~40 container clicks, Ilya's post-pickup retries, Zhenya's racing observe),
    // and revoking on them wedges every later write into `controller_busy` until
    // a restart. A reply-bound timeout (`owner_result_unknown`) frees the transport
    // slot but keeps the lease: the owner task still holds the only writer slot
    // (`writer_action_busy` stays fail-closed) and the fence stays live, so the
    // next write is admitted once the owner drains the in-flight task instead of
    // being fenced forever by a revoked lease.
    if let Err(error) = &result
        && !readonly
        && revoke_on_owner_error(error)
    {
        core.revoke("owner_error");
    }
    result
}

/// Which owner-side errors genuinely end the controller session. Revoking on any
/// other error converts one retried click into a stuck `controller_busy` lease.
fn revoke_on_owner_error(error: &str) -> bool {
    // Cancelled/fenced: the transport cancelled the request, the owner refused a
    // dead fence, or the session epochs moved under the request. The writer
    // identity is gone; keeping the lease would fence every later write. The
    // stale match is narrowed to epoch loss: per-click state races such as
    // `stale_container_state` are ordinary retried errors and keep the lease.
    if error == "cancelled"
        || error.starts_with("cancelled: ")
        || error == "controller_lease_revoked"
        || error == "lease_expired"
        || error == "stale_connection_epoch"
        || error == "stale_dimension_epoch"
    {
        return true;
    }
    // Everything else is an ordinary retried validation/state/predicate error
    // (`target is unreachable`, `ordinary raycast ...`, `no visible container`,
    // `container slot is outside menu size`, wait timeouts, reply-bound
    // `owner_result_unknown`, queue-full races, ...): keep the lease. The fence
    // stays live and the owner-side `writer_action_busy` gate still rejects a
    // genuinely concurrent second writer while a task is in flight, so the
    // fail-closed single-writer property is preserved without wedging the lease.
    false
}
