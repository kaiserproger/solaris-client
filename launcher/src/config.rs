//! Launcher state persisted in `<root>/launcher.json`, plus the client's own
//! `client-settings.json` (render distance / fullscreen live there so the
//! in-game Options screen and the launcher share the persisted values).
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const DEFAULT_MANIFEST: &str = "http://135.106.208.217:8080/static/solaris-content.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub username: String,
    #[serde(default)]
    pub skin_path: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub profiles: Vec<Profile>,
    pub selected_profile: usize,
    pub servers: Vec<Server>,
    pub selected_server: usize,
    pub manifest_url: String,
    #[serde(deserialize_with = "appearance_or_default")]
    pub appearance: Appearance,
    #[serde(deserialize_with = "window_or_default")]
    pub window: WindowSettings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme_id: String,
    pub favorites: Vec<String>,
    pub background_enabled: bool,
    pub background_path: String,
    pub reduced_motion: bool,
    #[serde(deserialize_with = "custom_themes_or_default")]
    pub custom_themes: Vec<crate::theme::CustomTheme>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme_id: "graphite".into(),
            favorites: Vec::new(),
            background_enabled: false,
            background_path: String::new(),
            reduced_motion: false,
            custom_themes: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub width: f32,
    pub height: f32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 820.0,
            height: 530.0,
        }
    }
}

fn appearance_or_default<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Appearance, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn custom_themes_or_default<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<crate::theme::CustomTheme>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let serde_json::Value::Array(values) = value else {
        return Ok(Vec::new());
    };
    // A damaged optional entry must not discard other palettes, accessibility
    // settings, or the background. Semantic validation remains in normalize.
    Ok(values
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

fn window_or_default<'de, D: serde::Deserializer<'de>>(d: D) -> Result<WindowSettings, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: vec![Profile {
                username: "Player".into(),
                skin_path: None,
            }],
            selected_profile: 0,
            servers: vec![Server {
                name: "Solaris test".into(),
                address: "135.106.208.217:25565".into(),
            }],
            selected_server: 0,
            manifest_url: DEFAULT_MANIFEST.into(),
            appearance: Appearance::default(),
            window: WindowSettings::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        let mut config: Config = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        config.normalize();
        config
    }

    /// Keep selections valid and the lists non-empty whatever the file held.
    pub fn normalize(&mut self) {
        let defaults = Config::default();
        if self.profiles.is_empty() {
            self.profiles = defaults.profiles;
        }
        if self.servers.is_empty() {
            self.servers = defaults.servers;
        }
        self.selected_profile = self.selected_profile.min(self.profiles.len() - 1);
        self.selected_server = self.selected_server.min(self.servers.len() - 1);
        if self.manifest_url.trim().is_empty() {
            self.manifest_url = DEFAULT_MANIFEST.into();
        }
        if !self.window.width.is_finite() || !self.window.height.is_finite() {
            self.window = WindowSettings::default();
        }
        self.window.width = self.window.width.clamp(640.0, 2560.0);
        self.window.height = self.window.height.clamp(460.0, 1600.0);
        self.appearance.favorites.sort();
        self.appearance.favorites.dedup();
        self.appearance
            .custom_themes
            .retain(|t| t.validate().is_ok());
        let mut ids = std::collections::HashSet::new();
        self.appearance
            .custom_themes
            .retain(|t| ids.insert(t.id.clone()));
        self.appearance.custom_themes.truncate(64);
        self.appearance.theme_id =
            crate::theme::resolve(&self.appearance.theme_id, &self.appearance.custom_themes).id;
        self.appearance.favorites.retain(|id| {
            crate::theme_presets::contains_id(id)
                || self.appearance.custom_themes.iter().any(|t| &t.id == id)
        });
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        save_json(path, self)
    }

    pub fn username(&self) -> &str {
        &self.profiles[self.selected_profile].username
    }

    pub fn address(&self) -> &str {
        &self.servers[self.selected_server].address
    }
}

/// Minecraft offline names: 3–16 of `[A-Za-z0-9_]`.
pub fn valid_username(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `host`, `host:port` or `[v6]:port` with a non-empty host.
pub fn valid_address(address: &str) -> bool {
    let a = address.trim();
    if a.is_empty() || a.contains(char::is_whitespace) {
        return false;
    }
    let (host, port) = if let Some(rest) = a.strip_prefix('[') {
        let Some((host, tail)) = rest.split_once(']') else {
            return false;
        };
        match tail {
            "" => (host, None),
            _ => match tail.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None => return false,
            },
        }
    } else {
        match a.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return false,
            None => (a, None),
        }
    };
    !host.is_empty() && port.is_none_or(|p| p.parse::<u16>().is_ok_and(|p| p > 0))
}

pub type ClientSettings = BTreeMap<String, serde_json::Value>;

pub fn load_client_settings(path: &Path) -> ClientSettings {
    read_client_settings(path).unwrap_or_default()
}

fn read_client_settings(path: &Path) -> anyhow::Result<ClientSettings> {
    use anyhow::Context;
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Parsing Minecraft settings"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ClientSettings::new()),
        Err(error) => Err(error).context("Reading Minecraft settings"),
    }
}

/// Merge only into a successfully read object. A missing file may be created;
/// corrupt or unreadable settings must never be replaced by display defaults.
pub fn update_client_setting(
    path: &Path,
    key: &str,
    value: serde_json::Value,
) -> anyhow::Result<ClientSettings> {
    let mut settings = read_client_settings(path)?;
    settings.insert(key.into(), value);
    save_client_settings(path, &settings)?;
    Ok(settings)
}

pub fn save_client_settings(path: &Path, settings: &ClientSettings) -> anyhow::Result<()> {
    save_json(path, settings)
}

fn save_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    use anyhow::Context;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).context("Creating settings directory")?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?).context("Writing settings")?;
    std::fs::rename(&tmp, path).context("Replacing settings file")
}

pub fn render_distance(settings: &ClientSettings) -> u32 {
    settings
        .get("render_distance")
        .and_then(|v| v.as_u64())
        .map_or(8, |v| v.clamp(2, 16) as u32)
}

pub fn fullscreen(settings: &ClientSettings) -> bool {
    settings
        .get("fullscreen")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_receives_graphite_appearance_without_losing_profiles() {
        let config: Config =
            serde_json::from_str(r#"{"profiles":[{"username":"Legacy"}]}"#).unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(config.username(), "Legacy");
        assert_eq!(value["appearance"]["theme_id"], "graphite");
        assert_eq!(value["appearance"]["background_enabled"], false);
    }

    #[test]
    fn usernames() {
        assert!(valid_username("Player_1"));
        assert!(!valid_username("ab"));
        assert!(!valid_username("has space"));
        assert!(!valid_username("seventeen_chars_xx"));
    }

    #[test]
    fn one_structurally_broken_theme_does_not_reset_other_appearance_settings() {
        let valid = serde_json::json!({
            "id":"custom-good", "name":"Good palette",
            "background":"#101010", "surface":"#202020", "text":"#FFFFFF", "accent":"#E0A060"
        });
        let mut invalid = valid.clone();
        invalid.as_object_mut().unwrap().remove("accent");
        let mut config: Config = serde_json::from_value(serde_json::json!({
            "appearance": {
                "theme_id":"custom-good", "favorites":["custom-good"],
                "background_enabled":true, "background_path":"local-background.png",
                "reduced_motion":true, "custom_themes":[valid, invalid]
            }
        }))
        .unwrap();
        config.normalize();
        assert!(config.appearance.reduced_motion);
        assert!(config.appearance.background_enabled);
        assert_eq!(config.appearance.background_path, "local-background.png");
        assert_eq!(config.appearance.custom_themes.len(), 1);
        assert_eq!(config.appearance.theme_id, "custom-good");
        assert_eq!(config.appearance.favorites, ["custom-good"]);
    }

    #[test]
    fn addresses() {
        assert!(valid_address("135.106.208.217:25565"));
        assert!(valid_address("example.com"));
        assert!(valid_address("[::1]:25565"));
        assert!(!valid_address(""));
        assert!(!valid_address("host:0"));
        assert!(!valid_address("host:notaport"));
        assert!(!valid_address("a b"));
    }

    #[test]
    fn broken_config_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("sl-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launcher.json");
        std::fs::write(&path, r#"{"profiles":[],"selected_server":9}"#).unwrap();
        let config = Config::load(&path);
        assert_eq!(config.username(), "Player");
        assert_eq!(config.selected_server, 0);
        assert_eq!(config.address(), "135.106.208.217:25565");
        let _ = std::fs::remove_dir_all(dir);
    }
}
