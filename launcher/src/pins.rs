//! Hashes compiled into this launcher release.
//!
//! The content manifest is served over plain HTTP, so nothing it says is
//! trusted: only the archive whose name AND sha256 are pinned here is
//! installed. Publishing a new facts bundle means bumping these constants in a
//! new launcher release (`tools/publish_facts.sh` prints the values).
pub const FACTS_NAME: &str = "solaris-facts-26.3-r1";
pub const FACTS_SHA256: &str = "3abdd5a9defb8f74c31911ba722e7e8add7daafeea56cfe9abba47a393ed9573";

/// Singleplayer server content cache (`tools/publish_content.sh` prints the values).
pub const CONTENT_NAME: &str = "solaris-content-26.3-r3";
pub const CONTENT_SHA256: &str = "3b9309aef07cce26f39eba25b770749bad7c54b3ae589b7bd2eb7bf96c4bb90f";
