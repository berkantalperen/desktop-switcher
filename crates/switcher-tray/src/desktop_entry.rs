//! Linux desktop integration: an app-menu entry for the settings window, a
//! login entry so the top-bar icon starts with the session, and the icon both
//! use.
//!
//! Written by `desktop-switcher-tray --install`, removed by `--uninstall`.
//! Everything goes under the user's own directories and nothing needs root.
//! The entry texts are pure, so they are tested on every platform.

use std::path::Path;

/// The icon's name in the icon theme, and the app-menu entry's file name.
///
/// GNOME matches a window to its entry by the window's Wayland app id, so the
/// GUI sets this same id and its window gets the icon in the dash.
pub const APP_ID: &str = "desktop-switcher";

/// Quote a path for an `Exec=` line. The Desktop Entry spec reserves `"`,
/// `` ` ``, `$` and `\` inside double quotes, each escaped with a backslash.
pub fn exec_quote(path: &Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The app-menu entry for the settings window.
pub fn app_entry(gui: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Desktop Switcher\n\
         Comment=Switch your monitors' inputs\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Utility;Settings;HardwareSettings;\n\
         StartupWMClass={APP_ID}\n",
        exec_quote(gui)
    )
}

/// The login entry for the top-bar icon. Hidden from the app menu: the
/// settings window starts the icon too, so there is one thing to open.
pub fn autostart_entry(tray: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Desktop Switcher tray\n\
         Comment=Top-bar menu for switching your monitors' inputs\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_quote(tray)
    )
}

#[cfg(target_os = "linux")]
pub use install::{install, uninstall};

#[cfg(target_os = "linux")]
mod install {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{app_entry, autostart_entry, APP_ID};

    const ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

    struct Places {
        applications: PathBuf,
        autostart: PathBuf,
        icons: PathBuf,
    }

    fn places() -> io::Result<Places> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("HOME is not set"))?;
        let env_or = |key: &str, fallback: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let data = env_or("XDG_DATA_HOME", ".local/share");
        let config = env_or("XDG_CONFIG_HOME", ".config");
        Ok(Places {
            applications: data.join("applications"),
            autostart: config.join("autostart"),
            icons: data.join("icons/hicolor"),
        })
    }

    fn files(places: &Places) -> (PathBuf, PathBuf, Vec<(u32, PathBuf)>) {
        let icons = ICON_SIZES
            .iter()
            .map(|&s| (s, places.icons.join(format!("{s}x{s}/apps/{APP_ID}.png"))))
            .collect();
        (
            places.applications.join(format!("{APP_ID}.desktop")),
            places.autostart.join(format!("{APP_ID}-tray.desktop")),
            icons,
        )
    }

    fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, bytes)
    }

    /// Write the entries and icons; returns every file written.
    pub fn install(gui: &Path, tray: &Path) -> io::Result<Vec<PathBuf>> {
        let places = places()?;
        let (app, login, icons) = files(&places);
        let mut written = Vec::new();
        for (size, path) in icons {
            write(&path, &switcher_icon::png_file(size))?;
            written.push(path);
        }
        write(&app, app_entry(gui).as_bytes())?;
        written.push(app);
        write(&login, autostart_entry(tray).as_bytes())?;
        written.push(login);
        Ok(written)
    }

    /// Remove everything `install` writes; returns what was there to remove.
    pub fn uninstall() -> io::Result<Vec<PathBuf>> {
        let places = places()?;
        let (app, login, icons) = files(&places);
        let mut removed = Vec::new();
        for path in [app, login]
            .into_iter()
            .chain(icons.into_iter().map(|(_, p)| p))
        {
            match std::fs::remove_file(&path) {
                Ok(()) => removed.push(path),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_plain_path_is_quoted() {
        let p = PathBuf::from("/home/berkant/.local/bin/desktop-switcher-gui");
        assert_eq!(
            exec_quote(&p),
            "\"/home/berkant/.local/bin/desktop-switcher-gui\""
        );
    }

    /// The characters the Desktop Entry spec reserves inside quotes.
    #[test]
    fn reserved_characters_are_escaped() {
        let p = PathBuf::from("/opt/my \"odd\" $dir/`x`\\bin");
        assert_eq!(
            exec_quote(&p),
            "\"/opt/my \\\"odd\\\" \\$dir/\\`x\\`\\\\bin\""
        );
    }

    #[test]
    fn the_app_entry_opens_the_settings_window_with_the_icon() {
        let text = app_entry(Path::new("/usr/bin/desktop-switcher-gui"));
        assert!(text.starts_with("[Desktop Entry]\n"), "{text}");
        assert!(
            text.contains("\nExec=\"/usr/bin/desktop-switcher-gui\"\n"),
            "{text}"
        );
        assert!(text.contains("\nIcon=desktop-switcher\n"), "{text}");
        assert!(
            text.contains("\nStartupWMClass=desktop-switcher\n"),
            "{text}"
        );
        assert!(!text.contains("NoDisplay"), "{text}");
    }

    /// Starts with the session, and stays out of the app menu.
    #[test]
    fn the_login_entry_starts_the_tray_and_hides_itself() {
        let text = autostart_entry(Path::new("/usr/bin/desktop-switcher-tray"));
        assert!(
            text.contains("\nExec=\"/usr/bin/desktop-switcher-tray\"\n"),
            "{text}"
        );
        assert!(
            text.contains("\nX-GNOME-Autostart-enabled=true\n"),
            "{text}"
        );
        assert!(text.contains("\nNoDisplay=true\n"), "{text}");
    }
}
