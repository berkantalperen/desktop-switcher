//! What the Windows and Linux trays have in common beyond the menu: where the
//! configuration comes from, and how the other programs are found.

use std::path::PathBuf;

use switcher_core::config::{self, Config, ConfigError};

/// The configuration, read fresh each time so the menu is never stale, or a
/// short reason it could not be read, fit to show as a menu line.
pub fn load_config() -> Result<Config, String> {
    let path = config::default_config_path().map_err(|e| e.to_string())?;
    match Config::load(&path) {
        Ok(c) => Ok(c),
        Err(ConfigError::Missing(_)) => Err("No configuration yet".into()),
        Err(e) => {
            let text = format!("Configuration problem: {e}");
            Err(text.chars().take(80).collect())
        }
    }
}

/// When the configuration file last changed, to notice edits cheaply.
pub fn config_stamp() -> Option<std::time::SystemTime> {
    config::default_config_path()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// Another of this project's programs, by base name (`desktop-switcher`,
/// `desktop-switcher-gui`): next to this one first, then on PATH.
pub fn sibling(base: &str) -> Option<PathBuf> {
    let name = format!("{base}{}", std::env::consts::EXE_SUFFIX);
    if let Ok(mut here) = std::env::current_exe() {
        here.pop();
        let candidate = here.join(&name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file())
}
