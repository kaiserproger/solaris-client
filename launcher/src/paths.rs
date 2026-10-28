//! Data-root discovery and the on-disk layout shared with `solaris-client`.
//!
//! ```text
//! <root>/
//!   facts/ data/vanilla/                Solaris data bundle (see `bootstrap`)
//!   resources/assets/                   extracted from the official client.jar
//!   assets/asset-index.json, objects/   Mojang asset index + sound/lang objects
//!   mojang/versions/<ver>/              official version json + client.jar
//!   cache/                              downloaded bundle archives
//!   logs/launcher.log
//!   launcher.json, installed.json, client-settings.json
//!   servers/content/<ver>/              singleplayer server content bundle (`--singleplayer-runtime`)
//!   servers/ worlds/                    singleplayer (see the client's `singleplayer` module)
//! ```
use std::path::{Path, PathBuf};

pub const GAME_VERSION: &str = "26.3";

/// `--data-dir` wins; otherwise use the shared client data-root convention.
pub fn data_root(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    solaris_paths::data_root(explicit)
}

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn join(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
    pub fn version_dir(&self) -> PathBuf {
        self.root.join("mojang/versions").join(GAME_VERSION)
    }
    pub fn client_jar(&self) -> PathBuf {
        self.version_dir().join("client.jar")
    }
    /// The local server's content cache (`vanilla_data_dir`), as the client's
    /// `singleplayer` module expects it.
    pub fn server_content(&self) -> PathBuf {
        self.root.join("servers/content").join(GAME_VERSION)
    }
    pub fn version_json(&self) -> PathBuf {
        self.version_dir().join(format!("{GAME_VERSION}.json"))
    }
    pub fn asset_index(&self) -> PathBuf {
        self.root.join("assets/asset-index.json")
    }
    pub fn asset_object(&self, hash: &str) -> PathBuf {
        self.root.join("assets/objects").join(&hash[..2]).join(hash)
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn log_file(&self) -> PathBuf {
        self.root.join("logs/launcher.log")
    }
    pub fn config_file(&self) -> PathBuf {
        self.root.join("launcher.json")
    }
    pub fn installed_file(&self) -> PathBuf {
        self.root.join("installed.json")
    }
    pub fn client_settings(&self) -> PathBuf {
        self.root.join("client-settings.json")
    }
}

/// Files and directories the client refuses to start without (mirrors the
/// client's `content_paths` table). `true` = directory.
pub const REQUIRED: &[(&str, bool)] = &[
    ("facts/block_collision_shapes_26_3.bin", false),
    ("facts/terrain-facts.json", false),
    ("facts/block-state-predicates.json", false),
    ("facts/block-use-classes.json", false),
    ("facts/item-use-owners.json", false),
    ("facts/interaction-facts.json", false),
    ("facts/item-properties.json", false),
    ("facts/creative-facts.json", false),
    ("facts/placement-state-facts.json", false),
    ("facts/item-placement-facts.json", false),
    ("facts/action-state-facts.json", false),
    ("facts/entity-use-static.json", false),
    ("assets/asset-index.json", false),
    ("assets/objects", true),
    ("data/vanilla/reports/blocks.json", false),
    ("data/vanilla/reports/block_mining.json", false),
    ("data/vanilla/reports/block_light.json", false),
    ("data/vanilla/reports/registries.json", false),
    ("resources/assets/minecraft", true),
];

/// Names of required content that is missing; empty = layout complete.
pub fn missing(root: &Path) -> Vec<&'static str> {
    REQUIRED
        .iter()
        .filter(|(rel, dir)| {
            let p = root.join(rel);
            !(if *dir { p.is_dir() } else { p.is_file() })
        })
        .map(|(rel, _)| *rel)
        .collect()
}
