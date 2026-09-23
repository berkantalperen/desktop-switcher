//! A notification-area icon for desktop-switcher.
//!
//! One click opens a menu of your actions and each monitor's named inputs.
//! Choosing one runs the CLI, exactly as a hotkey does. It says nothing when a
//! switch works, because the screens changing is the answer, and shows a
//! notification with the reason when one does not.
//!
//! Windows only for now. GNOME has no notification area without an extension,
//! and hotkeys cover Linux.

#![windows_subsystem = "windows"]
// Elsewhere only the tests use the menu, icon and notice code.
#![cfg_attr(not(windows), allow(dead_code))]

mod hotkey;
mod menu;
mod notice;
#[cfg(windows)]
mod win;

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    {
        std::process::ExitCode::from(win::run() as u8)
    }
    #[cfg(not(windows))]
    {
        eprintln!(
            "desktop-switcher-tray only runs on Windows for now. On Linux, bind your \
             actions to hotkeys with scripts/install-gnome-shortcuts.sh."
        );
        std::process::ExitCode::from(1)
    }
}
