//! Source-specific living and player presentation; derived clips are activated as content.
mod clips;
mod modern;
mod motion;
pub(crate) use modern::AxolotlAnimation;
pub(crate) use motion::{BodyRule, MotionFacts, WalkRule};
mod state;
pub(crate) use clips::AnimationDefinition;
pub(crate) use state::{Avatar, Modern, State, TickInput, Timer};
