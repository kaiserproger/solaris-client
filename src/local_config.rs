//! Exe-adjacent `client.toml` defaults for player-ready `dist/` launches.
//!
//! No engine behavior changes: this only injects defaults for environment
//! lookups the session/content/audio layers already perform. Explicit
//! process environment always wins; explicit CLI flags always win over this.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Defaults sourced from `<exe-dir>/client.toml`. Missing file = no defaults.
pub struct LocalDefaults {
    pub server: Option<String>,
    pub username: Option<String>,
    pub profile: Option<String>,
    pub windowed: Option<bool>,
}

/// Path-valued `client.toml [resources]` keys mapped to their env var.
const RESOURCE_VARS: &[(&str, &str)] = &[
    ("data_dir", "SOLARIS_CLIENT_DATA_DIR"),
    ("resource_root", "SOLARIS_CLIENT_RESOURCE_ROOT"),
    ("asset_root", "SOLARIS_CLIENT_ASSET_ROOT"),
    ("asset_index", "SOLARIS_CLIENT_ASSET_INDEX"),
    ("collision_shapes", "SOLARIS_CLIENT_COLLISION_SHAPES"),
    ("terrain_facts", "SOLARIS_CLIENT_TERRAIN_FACTS"),
    (
        "block_state_predicates",
        "SOLARIS_CLIENT_BLOCK_STATE_PREDICATES",
    ),
    ("block_use_facts", "SOLARIS_CLIENT_BLOCK_USE_FACTS"),
    ("item_use_facts", "SOLARIS_CLIENT_ITEM_USE_FACTS"),
    ("interaction_facts", "SOLARIS_CLIENT_INTERACTION_FACTS"),
    ("item_properties", "SOLARIS_CLIENT_ITEM_PROPERTIES"),
    ("creative_tabs", "SOLARIS_CLIENT_CREATIVE_TABS"),
    ("placement_facts", "SOLARIS_CLIENT_PLACEMENT_FACTS"),
    (
        "item_placement_facts",
        "SOLARIS_CLIENT_ITEM_PLACEMENT_FACTS",
    ),
    ("action_state_facts", "SOLARIS_CLIENT_ACTION_STATE_FACTS"),
    ("entity_use_static", "SOLARIS_CLIENT_ENTITY_USE_STATIC"),
    ("resource_packs", "SOLARIS_CLIENT_RESOURCE_PACKS"),
];

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Minimal `key = "value"` reader: flat keys plus one `[resources]` section.
/// Unknown keys/sections are ignored so future config stays compatible.
fn parse_config(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let mut value = value.trim();
        if let Some(comment) = value.find(" #") {
            value = value[..comment].trim_end();
        }
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        let full = if section == "resources" {
            format!("resources.{key}")
        } else if section.is_empty() {
            key
        } else {
            continue;
        };
        out.insert(full, value.to_owned());
    }
    out
}

fn resolve(dir: &Path, value: &str) -> String {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return value.to_owned();
    }
    dir.join(path).to_string_lossy().into_owned()
}

/// Read `<exe-dir>/client.toml` and export defaults for unset env vars.
/// Returns connection defaults for the CLI layer to apply when its own
/// flags are absent. Never overrides explicit environment.
pub fn apply_exe_defaults() -> LocalDefaults {
    let Some(dir) = exe_dir() else {
        return LocalDefaults {
            server: None,
            username: None,
            profile: None,
            windowed: None,
        };
    };
    let text = std::fs::read_to_string(dir.join("client.toml")).unwrap_or_default();
    if text.is_empty() {
        return LocalDefaults {
            server: None,
            username: None,
            profile: None,
            windowed: None,
        };
    }
    let config = parse_config(&text);
    for (key, var) in RESOURCE_VARS {
        if std::env::var_os(var).is_some() {
            continue;
        }
        if let Some(value) = config.get(&format!("resources.{key}")) {
            if !value.is_empty() {
                unsafe { std::env::set_var(var, resolve(&dir, value)) };
            }
        }
    }
    // Non-path scalar passthrough (env still wins).
    if std::env::var_os("SOLARIS_CLIENT_LOADER_PERMISSIONS").is_none() {
        if let Some(value) = config.get("resources.loader_permissions") {
            if !value.is_empty() {
                unsafe { std::env::set_var("SOLARIS_CLIENT_LOADER_PERMISSIONS", value) };
            }
        }
    }
    let plain = |key: &str| config.get(key).filter(|v| !v.is_empty()).cloned();
    LocalDefaults {
        server: plain("server"),
        username: plain("username"),
        profile: plain("profile"),
        windowed: plain("windowed").and_then(|v| match v.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }),
    }
}
/// Options use the selected data root unless explicitly overridden.
pub fn client_settings_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SOLARIS_CLIENT_SETTINGS").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(explicit));
    }
    crate::content_paths::selected_root().ok().map(|root| root.join("client-settings.json"))
}

/// Server lists always belong to the selected data root, independent of settings overrides.
pub(crate) fn servers_path() -> Option<PathBuf> {
    crate::content_paths::selected_root().ok().map(|root| root.join("servers.json"))
}
/// Load persisted managed settings. Missing file/keys/parse errors yield an
/// empty map; callers fill defaults via `UiState::default_setting_value`.
pub fn load_client_settings() -> BTreeMap<String, serde_json::Value> {
    let Some(path) = client_settings_path() else {
        return BTreeMap::new();
    };
    load_client_settings_from(&path)
}
/// Best-effort atomic-ish save of managed settings (write + rename).
pub fn save_client_settings(settings: &BTreeMap<String, serde_json::Value>) {
    let Some(path) = client_settings_path() else {
        return;
    };
    save_client_settings_to(settings, &path);
}
/// Test seam: load from an explicit path (same tolerant map semantics).
pub fn load_client_settings_from(path: &std::path::Path) -> BTreeMap<String, serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if text.trim().is_empty() {
        return BTreeMap::new();
    }
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let serde_json::Value::Object(map) = parsed else {
        return BTreeMap::new();
    };
    let mut out: BTreeMap<String, serde_json::Value> = map
        .into_iter()
        .filter(|(key, _)| {
            key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && key.len() <= 64
        })
        .collect();
    // One-way legacy migration: bool `fancy_shaders` -> string `graphics_preset`.
    if !out.contains_key("graphics_preset")
        && let Some(legacy) = out.remove("fancy_shaders")
    {
        let preset = if legacy.as_bool().unwrap_or(true) {
            "medium"
        } else {
            "low"
        };
        out.insert("graphics_preset".into(), serde_json::json!(preset));
    } else {
        out.remove("fancy_shaders");
    }
    out
}
/// Test seam: save to an explicit path (same managed-keys-only filter).
pub fn save_client_settings_to(
    settings: &BTreeMap<String, serde_json::Value>,
    path: &std::path::Path,
) {
    let kept: BTreeMap<&str, serde_json::Value> = settings
        .iter()
        .filter_map(|(key, value)| {
            crate::ui::UiState::sanitize_setting_value(key, value)
                .map(|clean| (key.as_str(), clean))
        })
        .collect();
    let Ok(text) = serde_json::to_string_pretty(&kept) else {
        return;
    };
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty())
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_selected_data_root_and_explicit_override() {
        const CHILD: &str = "SOLARIS_SETTINGS_PATH_TEST_CHILD";
        if let Some(case) = std::env::var_os(CHILD) {
            let root = if case == "explicit-root" {
                PathBuf::from("/tmp/selected-client-root")
            } else {
                crate::content_paths::default_root().unwrap()
            };
            crate::content_paths::apply_defaults(&root);
            let expected = if case == "settings-override" {
                PathBuf::from("/tmp/custom-settings.json")
            } else {
                root.join("client-settings.json")
            };
            assert_eq!(client_settings_path(), Some(expected));
            assert_eq!(servers_path(), Some(root.join("servers.json")));
            return;
        }
        for case in ["explicit-root", "platform-root", "settings-override"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "local_config::tests::settings_follow_selected_data_root_and_explicit_override"])
                .env(CHILD, case)
                .env("HOME", "/tmp/client-test-home")
                .env("XDG_DATA_HOME", "/tmp/client-test-xdg")
                .env("APPDATA", "/tmp/client-test-appdata")
                .env_remove("SOLARIS_CLIENT_SETTINGS")
                .env_remove("SOLARIS_CLIENT_ROOT");
            if case == "settings-override" {
                child.env("SOLARIS_CLIENT_SETTINGS", "/tmp/custom-settings.json");
            }
            let output = child.output().unwrap();
            assert!(output.status.success(), "{case}: {}", String::from_utf8_lossy(&output.stdout));
        }
    }

    #[test]
    fn parses_flat_and_section_keys() {
        let config = parse_config(
            "# comment\nserver = \"127.0.0.1:25565\"\nusername='Player'\n[resources]\ndata_dir = \"data\"\n",
        );
        assert_eq!(config["server"], "127.0.0.1:25565");
        assert_eq!(config["username"], "Player");
        assert_eq!(config["resources.data_dir"], "data");
    }

    #[test]
    fn ignores_unknown_sections_and_garbage() {
        let config = parse_config("[bogus]\nfoo = \"x\"\nnot a kv\n[resources]\nbad-key = \"y\"\n");
        assert!(config.is_empty());
    }

    #[test]
    fn settings_round_trip_keeps_managed_keys_only() {
        let dir = std::env::temp_dir().join(format!(
            "solaris-settings-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client-settings.json");
        let mut settings = BTreeMap::new();
        settings.insert("fov".into(), serde_json::json!(90.));
        settings.insert("volume".into(), serde_json::json!(0.5));
        settings.insert("render_distance".into(), serde_json::json!(12));
        settings.insert("damage_tilt_strength".into(), serde_json::json!(0.5));
        settings.insert("fullscreen".into(), serde_json::json!(true));
        settings.insert("vsync".into(), serde_json::json!(false));
        settings.insert("graphics_preset".into(), serde_json::json!("low"));
        settings.insert("max_fps".into(), serde_json::json!(120));
        settings.insert("chat_key".into(), serde_json::json!("not persisted"));
        save_client_settings_to(&settings, &path);
        let loaded = load_client_settings_from(&path);
        assert_eq!(loaded["fov"], serde_json::json!(90.));
        assert_eq!(loaded["volume"], serde_json::json!(0.5));
        assert_eq!(loaded["render_distance"], serde_json::json!(12));
        assert_eq!(loaded["damage_tilt_strength"], serde_json::json!(0.5));
        assert_eq!(loaded.get("fullscreen"), Some(&serde_json::json!(true)));
        assert_eq!(loaded.get("vsync"), Some(&serde_json::json!(false)));
        assert_eq!(loaded.get("graphics_preset"), Some(&serde_json::json!("low")));
        assert_eq!(loaded.get("max_fps"), Some(&serde_json::json!(120)));
        assert!(!loaded.contains_key("chat_key"));
        // Corrupt file degrades to empty, not an error.
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_client_settings_from(&path).is_empty());
        // Missing file degrades to empty.
        assert!(load_client_settings_from(&dir.join("absent.json")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_fancy_shaders_migrates_to_graphics_preset() {
        let dir = std::env::temp_dir().join(format!(
            "solaris-settings-migrate-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client-settings.json");
        std::fs::write(&path, r#"{"fancy_shaders":false,"volume":0.5}"#).unwrap();
        let loaded = load_client_settings_from(&path);
        assert_eq!(loaded.get("graphics_preset"), Some(&serde_json::json!("low")));
        assert!(!loaded.contains_key("fancy_shaders"));
        std::fs::write(&path, r#"{"fancy_shaders":true}"#).unwrap();
        let loaded = load_client_settings_from(&path);
        assert_eq!(loaded.get("graphics_preset"), Some(&serde_json::json!("medium")));
        std::fs::write(&path, r#"{"fancy_shaders":false,"graphics_preset":"high"}"#).unwrap();
        let loaded = load_client_settings_from(&path);
        assert_eq!(loaded.get("graphics_preset"), Some(&serde_json::json!("high")));
        assert!(!loaded.contains_key("fancy_shaders"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
