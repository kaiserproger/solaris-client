//! Player screenshot names are independent of automation artifact names.
use std::path::{Path, PathBuf};

pub(crate) fn path<'a>(
    directory: &Path,
    stamp: &str,
    pending: impl Iterator<Item = &'a PathBuf> + Clone,
) -> PathBuf {
    let mut suffix = 0u32;
    loop {
        let name = if suffix == 0 { format!("{stamp}.png") } else { format!("{stamp}_{suffix}.png") };
        let path = directory.join(name);
        if !path.exists() && !pending.clone().any(|existing| existing == &path) {
            return path;
        }
        suffix += 1;
    }
}

/// Format the player's local wall clock, matching vanilla's screenshot names.
pub(crate) fn timestamp() -> Result<String, String> {
    #[cfg(unix)]
    {
        let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?.as_secs();
        let seconds: libc::time_t = seconds.try_into().map_err(|_| "screenshot clock out of range")?;
        let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
        // SAFETY: both pointers refer to valid storage; localtime_r initializes
        // the output on success and does not use shared tm storage.
        let local = unsafe {
            if libc::localtime_r(&seconds, local.as_mut_ptr()).is_null() {
                return Err("Cannot read local screenshot time".into());
            }
            local.assume_init()
        };
        Ok(format!("{:04}-{:02}-{:02}_{:02}.{:02}.{:02}",
            local.tm_year + 1900, local.tm_mon + 1, local.tm_mday,
            local.tm_hour, local.tm_min, local.tm_sec))
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct LocalTime { year: u16, month: u16, weekday: u16, day: u16,
            hour: u16, minute: u16, second: u16, millis: u16 }
        #[link(name = "kernel32")]
        unsafe extern "system" { fn GetLocalTime(time: *mut LocalTime); }
        let mut local = std::mem::MaybeUninit::<LocalTime>::uninit();
        // SAFETY: GetLocalTime writes one SYSTEMTIME into correctly laid-out storage.
        let local = unsafe { GetLocalTime(local.as_mut_ptr()); local.assume_init() };
        Ok(format!("{:04}-{:02}-{:02}_{:02}.{:02}.{:02}",
            local.year, local.month, local.day, local.hour, local.minute, local.second))
    }
}

pub(crate) fn message(result: &Result<serde_json::Value, String>) -> String {
    match result {
        Ok(value) => format!("Saved screenshot as {}", value["file_name"].as_str().unwrap_or("PNG")),
        Err(error) => format!("Couldn't save screenshot: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc1_screenshot_timestamp_and_collision_suffixes_preserve_existing_files() {
        let dir = std::env::temp_dir().join(format!("solaris-screenshot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let stamp = "2026-10-29_12.34.56";
        let first = path(&dir, stamp, [].iter());
        assert_eq!(first.file_name().unwrap(), "2026-10-29_12.34.56.png");
        std::fs::write(&first, b"original").unwrap();
        let second = path(&dir, stamp, [].iter());
        assert_eq!(second.file_name().unwrap(), "2026-10-29_12.34.56_1.png");
        std::fs::write(&second, b"second").unwrap();
        let third = path(&dir, stamp, [].iter());
        assert_eq!(third.file_name().unwrap(), "2026-10-29_12.34.56_2.png");
        let fourth = path(&dir, stamp, [third].iter());
        assert_eq!(fourth.file_name().unwrap(), "2026-10-29_12.34.56_3.png");
        assert_eq!(std::fs::read(&first).unwrap(), b"original");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn screenshot_feedback_reports_success_and_failure() {
        assert_eq!(message(&Ok(serde_json::json!({"file_name":"2026-10-29_12.34.56.png"}))),
            "Saved screenshot as 2026-10-29_12.34.56.png");
        assert_eq!(message(&Err("disk full".into())), "Couldn't save screenshot: disk full");
        let stamp = timestamp().unwrap();
        assert_eq!(stamp.len(), 19);
        assert_eq!(stamp.as_bytes()[10], b'_');
    }
}
