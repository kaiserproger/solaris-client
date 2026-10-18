//! Opt-in, local-only MCP automation; all effects go through the window/session owner.
mod catalog;
mod control;
#[cfg(test)]
mod tests;
mod tools;
mod transport;

pub use transport::start;
pub use transport::{Config, Server, attach_stdio};
