//! Source-specific living and player presentation.
mod modern;
mod motion;
pub(crate) use modern::AxolotlAnimation;
pub(crate) use motion::{BodyRule, MotionFacts, WalkRule};
mod state;
pub(crate) use state::{Avatar, Modern, State, TickInput, Timer};
