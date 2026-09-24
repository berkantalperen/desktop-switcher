//! A tray icon for desktop-switcher: the notification area on Windows, the
//! top bar on Linux.
//!
//! One click opens a menu of your actions and each monitor's named inputs.
//! Choosing one runs the CLI, exactly as a hotkey does. It says nothing when a
//! switch works, because the screens changing is the answer, and shows a
//! notification with the reason when one does not.
//!
//! On Windows it also registers the hotkeys. On Linux GNOME does that, since
//! under Wayland an application cannot grab keys for itself.
//!
//! On Linux, `--install` adds an app-menu entry for the settings window and
//! has the icon start with the session; `--uninstall` removes them.

#![windows_subsystem = "windows"]
// Elsewhere only the tests use the shared menu, icon and notice code.
#![cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod desktop_entry;
#[cfg_attr(not(windows), allow(dead_code))]
mod hotkey;
#[cfg(target_os = "linux")]
mod linux;
mod menu;
mod notice;
mod shared;
#[cfg(windows)]
mod win;

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(windows)]
    {
        ExitCode::from(win::run() as u8)
    }
    #[cfg(target_os = "linux")]
    {
        match std::env::args().nth(1).as_deref() {
            Some("--install") => install(),
            Some("--uninstall") => uninstall(),
            Some(other) => {
                eprintln!("unknown argument `{other}`; expected --install or --uninstall");
                ExitCode::from(2)
            }
            None => ExitCode::from(linux::run() as u8),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        eprintln!("desktop-switcher-tray runs on Windows and Linux only.");
        ExitCode::from(1)
    }
}

#[cfg(target_os = "linux")]
fn install() -> ExitCode {
    let tray = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("could not tell where this program is: {e}");
            return ExitCode::from(1);
        }
    };
    let Some(gui) = shared::sibling("desktop-switcher-gui") else {
        eprintln!("desktop-switcher-gui was not found next to this program or on PATH");
        return ExitCode::from(1);
    };
    match desktop_entry::install(&gui, &tray) {
        Ok(written) => {
            for path in written {
                println!("  wrote {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("could not install the desktop entries: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(target_os = "linux")]
fn uninstall() -> ExitCode {
    match desktop_entry::uninstall() {
        Ok(removed) => {
            for path in removed {
                println!("  removed {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("could not remove the desktop entries: {e}");
            ExitCode::from(1)
        }
    }
}
