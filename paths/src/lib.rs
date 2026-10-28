//! Platform data-root discovery shared by the Solaris client and launcher.
use std::{ffi::OsString, path::PathBuf};

const APP_NAME: &str = "solaris-client";

fn default_root_for(
    platform: &str,
    home: Option<OsString>,
    xdg_data_home: Option<OsString>,
    app_data: Option<OsString>,
) -> Result<PathBuf, String> {
    match platform {
        "windows" => {
            if let Some(app_data) = app_data.filter(|value| !value.is_empty()) {
                return Ok(PathBuf::from(app_data).join(APP_NAME));
            }
        }
        "macos" => {
            if let Some(home) = home.as_ref().filter(|value| !value.is_empty()) {
                return Ok(PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join(APP_NAME));
            }
        }
        _ => {}
    }
    if let Some(data) = xdg_data_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(data).join(APP_NAME));
    }
    home.filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".local/share").join(APP_NAME))
        .ok_or_else(|| "data directory unavailable: set HOME/XDG_DATA_HOME or pass --data-dir".into())
}

/// `explicit` wins; otherwise use the platform's conventional per-user data root.
pub fn data_root(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    explicit.map(Ok).unwrap_or_else(|| {
        default_root_for(
            std::env::consts::OS,
            std::env::var_os("HOME"),
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("APPDATA"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_uses_application_support() {
        assert_eq!(
            default_root_for("macos", Some("/Users/player".into()), None, None).unwrap(),
            PathBuf::from("/Users/player/Library/Application Support/solaris-client"),
        );
    }
}
