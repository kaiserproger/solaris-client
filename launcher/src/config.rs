//! Launcher state persisted in `<root>/launcher.json`, plus the client's own
//! `client-settings.json` (render distance / fullscreen live there so the
//! in-game Options screen and the launcher never disagree).
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const DEFAULT_MANIFEST: &str = "http://135.106.208.217:8080/static/solaris-content.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub username: String,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: vec![Profile {
                username: "Player".into(),
            }],
            selected_profile: 0,
            servers: vec![Server {
                name: "Solaris test".into(),
                address: "135.106.208.217:25565".into(),
            }],
            selected_server: 0,
            manifest_url: DEFAULT_MANIFEST.into(),
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
    }

    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
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
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(map) => Some(map.into_iter().collect()),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn save_client_settings(path: &Path, settings: &ClientSettings) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(settings) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

pub fn render_distance(settings: &ClientSettings) -> u32 {
    settings
        .get("render_distance")
        .and_then(|v| v.as_u64())
        .map_or(8, |v| v.clamp(2, 16) as u32)
}

pub fn fullscreen(settings: &ClientSettings) -> bool {
    settings.get("fullscreen").and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert!(valid_username("Player_1"));
        assert!(!valid_username("ab"));
        assert!(!valid_username("has space"));
        assert!(!valid_username("seventeen_chars_xx"));
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
