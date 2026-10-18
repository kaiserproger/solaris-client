//! Canonical Java-compatible Minecraft tools, executed by the ordinary client owner.
mod entities;
mod inventory;
mod lifecycle;
mod navigation;

use crate::{protocol::Result, session::Session};
use serde_json::Value;
use std::time::Instant;

pub(crate) enum Outcome<T> {
    Complete(Value),
    Pending(T),
}

pub(crate) enum Task {
    Inventory(inventory::Task),
    Entities(entities::Task),
    Lifecycle(lifecycle::Task),
    Navigation(navigation::Task),
}

pub(crate) fn start(session: &mut Session, name: &str, arguments: &Value) -> Result<Outcome<Task>> {
    if name == "minecraft_observe" {
        return Ok(Outcome::Complete(session.minecraft_observation()));
    }
    if name == "minecraft_request_loader_view" {
        let key = arguments["view_key"]
            .as_str()
            .ok_or("invalid_arguments: view_key required")?;
        return session.request_loader_view(key).map(Outcome::Complete);
    }
    if let Some(outcome) = inventory::start(session, name, arguments)? {
        return Ok(match outcome {
            Outcome::Complete(value) => Outcome::Complete(value),
            Outcome::Pending(task) => Outcome::Pending(Task::Inventory(task)),
        });
    }
    if let Some(outcome) = entities::start(session, name, arguments)? {
        return Ok(match outcome {
            Outcome::Complete(value) => Outcome::Complete(value),
            Outcome::Pending(task) => Outcome::Pending(Task::Entities(task)),
        });
    }
    if let Some(outcome) = navigation::start(session, name, arguments)? {
        return Ok(match outcome {
            Outcome::Complete(value) => Outcome::Complete(value),
            Outcome::Pending(task) => Outcome::Pending(Task::Navigation(task)),
        });
    }
    if let Some(outcome) = lifecycle::start(session, name, arguments)? {
        return Ok(match outcome {
            Outcome::Complete(value) => Outcome::Complete(value),
            Outcome::Pending(task) => Outcome::Pending(Task::Lifecycle(task)),
        });
    }
    Err(format!("unknown Minecraft tool {name}"))
}

impl Task {
    /// The application invokes this only after an actual client state/tick event.
    pub(crate) fn on_event(&mut self, session: &mut Session) -> Result<Option<Value>> {
        match self {
            Self::Inventory(task) => task.on_event(session),
            Self::Entities(task) => task.on_event(session),
            Self::Lifecycle(task) => task.on_event(session),
            Self::Navigation(task) => task.on_event(session),
        }
    }
    pub(crate) fn deadline(&self) -> Instant {
        match self {
            Self::Inventory(task) => task.deadline(),
            Self::Entities(task) => task.deadline(),
            Self::Lifecycle(task) => task.deadline(),
            Self::Navigation(task) => task.deadline(),
        }
    }
    pub(crate) fn cancel(&mut self, session: &mut Session) -> Result<()> {
        match self {
            Self::Inventory(task) => task.cancel(session),
            Self::Entities(task) => task.cancel(session),
            Self::Lifecycle(task) => task.cancel(session),
            Self::Navigation(task) => task.cancel(session),
        }
    }
}
