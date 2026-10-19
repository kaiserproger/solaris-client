//! Hashes compiled into this launcher release.
//!
//! The content manifest is served over plain HTTP, so nothing it says is
//! trusted: only the archive whose name AND sha256 are pinned here is
//! installed. Publishing a new facts bundle means bumping these constants in a
//! new launcher release (`tools/publish_facts.sh` prints the values).
pub const FACTS_NAME: &str = "solaris-facts-26.1.2-r1";
pub const FACTS_SHA256: &str = "98ee25d831aa30d241e2c07ed1be51946059efe249e09ada0b91e8ca0b3cec37";
