//! The Linux side: an icon in the top bar, through the StatusNotifierItem
//! protocol that GNOME shows (with Ubuntu's AppIndicator extension, which is
//! on by default) and KDE shows natively.
//!
//! Everything a click does goes through `desktop-switcher`, exactly as a
//! hotkey does. Hotkeys themselves stay with GNOME: under Wayland an
//! application cannot grab keys, so `install-gnome-shortcuts.sh` binds them.

use std::collections::HashMap;
use std::process::Command as Process;
use std::time::Duration;

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use crate::menu::{self, Command, Entry};
use crate::notice::{self, Notice};
use crate::shared;

/// Owned for as long as the tray runs, so a second copy — from the app menu,
/// or the settings window starting it — can tell and leave quietly.
const BUS_NAME: &str = "io.github.berkantalperen.DesktopSwitcherTray";
/// How often the configuration is checked for changes, so the menu follows
/// edits even if the desktop never says the menu is about to open.
const CONFIG_POLL: Duration = Duration::from_secs(2);
/// How long to wait at login for the desktop to start showing status icons
/// before saying that it does not.
const HOST_GRACE: Duration = Duration::from_secs(30);

struct SwitcherTray {
    entries: Vec<Entry>,
}

fn current_menu() -> Vec<Entry> {
    match shared::load_config() {
        Ok(config) => menu::build(Ok(&config)),
        Err(why) => menu::build(Err(&why)),
    }
}

impl ksni::Tray for SwitcherTray {
    // A click on the icon opens the menu, as it does on Windows.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        crate::desktop_entry::APP_ID.into()
    }

    fn title(&self) -> String {
        "Desktop Switcher".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [16, 22, 24, 32, 48, 64].into_iter().map(icon).collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Desktop Switcher".into(),
            ..Default::default()
        }
    }

    /// Read the configuration fresh each time the menu opens.
    fn menu_about_to_show(&mut self) {
        self.entries = current_menu();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        items(&self.entries)
    }

    /// Keep running: the top bar comes back after a shell restart or when
    /// the extension is turned on again.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        true
    }
}

/// The drawing, as the protocol wants it: ARGB, most significant byte first.
fn icon(size: u32) -> ksni::Icon {
    let rgba = switcher_icon::pixels(size);
    let data = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect();
    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

fn items(entries: &[Entry]) -> Vec<MenuItem<SwitcherTray>> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Item {
                label,
                detail,
                command,
                ..
            } => {
                let command = command.clone();
                StandardItem {
                    label: menu::dbus_text(label, detail.as_deref()),
                    activate: Box::new(move |_: &mut SwitcherTray| perform(command.clone())),
                    ..Default::default()
                }
                .into()
            }
            Entry::Note { label } => StandardItem {
                label: menu::dbus_text(label, None),
                enabled: false,
                ..Default::default()
            }
            .into(),
            Entry::Submenu { label, entries } => SubMenu {
                label: menu::dbus_text(label, None),
                submenu: items(entries),
                ..Default::default()
            }
            .into(),
            Entry::Separator => MenuItem::Separator,
        })
        .collect()
}

fn perform(command: Command) {
    match command {
        Command::Quit => std::process::exit(0),
        Command::OpenSettings => {
            let started = shared::sibling("desktop-switcher-gui")
                .ok_or_else(|| "desktop-switcher-gui was not found".to_string())
                .and_then(|gui| Process::new(gui).spawn().map_err(|e| e.to_string()));
            match started {
                // Waited on so it does not linger as a zombie once closed.
                Ok(mut child) => {
                    std::thread::spawn(move || child.wait());
                }
                Err(why) => notify(&notice::cannot_start("Settings", &why)),
            }
        }
        // Off the menu's thread: a switch takes a moment, and the menu must
        // not hang while it does.
        Command::Cli { args, describe } => {
            std::thread::spawn(move || {
                let Some(cli) = shared::sibling("desktop-switcher") else {
                    notify(&notice::cannot_start(
                        &describe,
                        "desktop-switcher was not found",
                    ));
                    return;
                };
                let found = match Process::new(cli).args(&args).output() {
                    Ok(out) => {
                        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                        text.push('\n');
                        text.push_str(&String::from_utf8_lossy(&out.stderr));
                        notice::for_run(&describe, out.status.code(), &text)
                    }
                    Err(e) => Some(notice::cannot_start(&describe, &e.to_string())),
                };
                if let Some(found) = found {
                    notify(&found);
                }
            });
        }
    }
}

/// A desktop notification (org.freedesktop.Notifications).
fn notify(notice: &Notice) {
    let sent = Connection::session().and_then(|bus| {
        let hints: HashMap<&str, Value> = HashMap::new();
        bus.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "Desktop Switcher",
                0u32,
                crate::desktop_entry::APP_ID,
                notice.title.as_str(),
                notice.body.as_str(),
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )
    });
    if let Err(e) = sent {
        eprintln!("{}: {} (could not notify: {e})", notice.title, notice.body);
    }
}

/// Whether some other process on the bus owns `name`.
fn bus_has(bus: &Connection, name: &str) -> bool {
    bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(name,),
    )
    .ok()
    .and_then(|reply| reply.body().deserialize::<bool>().ok())
    .unwrap_or(false)
}

/// Claim the tray's bus name. `None` means another copy already runs.
fn claim(bus: &Connection) -> zbus::Result<Option<()>> {
    // DBUS_NAME_FLAG_DO_NOT_QUEUE; reply 1 is "primary owner", 4 "already".
    let reply = bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "RequestName",
        &(BUS_NAME, 4u32),
    )?;
    let code: u32 = reply.body().deserialize()?;
    Ok(matches!(code, 1 | 4).then_some(()))
}

pub fn run() -> i32 {
    let bus = match Connection::session() {
        Ok(bus) => bus,
        Err(e) => {
            eprintln!("desktop-switcher-tray: no session bus: {e}");
            return 1;
        }
    };
    match claim(&bus) {
        Ok(Some(())) => {}
        Ok(None) => return 0,
        Err(e) => {
            eprintln!("desktop-switcher-tray: could not claim {BUS_NAME}: {e}");
            return 1;
        }
    }

    // At login this can run before the top bar is ready to show icons, so
    // register regardless and appear when it is.
    let tray = SwitcherTray {
        entries: current_menu(),
    };
    let handle = match tray.assume_sni_available(true).spawn() {
        Ok(handle) => handle,
        Err(e) => {
            notify(&Notice {
                title: "Desktop Switcher: no top-bar icon".into(),
                body: e.to_string(),
            });
            return 1;
        }
    };

    // Say so once if nothing on this desktop shows status icons at all,
    // rather than leaving an icon that silently never appears.
    std::thread::spawn(|| {
        std::thread::sleep(HOST_GRACE);
        if let Ok(bus) = Connection::session() {
            if !bus_has(&bus, "org.kde.StatusNotifierWatcher") {
                notify(&Notice {
                    title: "Desktop Switcher cannot show its icon".into(),
                    body: "Nothing on this desktop shows status icons. On GNOME, turn on \
                           the AppIndicator extension in the Extensions app."
                        .into(),
                });
            }
        }
    });

    let mut stamp = shared::config_stamp();
    loop {
        std::thread::sleep(CONFIG_POLL);
        let now = shared::config_stamp();
        if now != stamp {
            stamp = now;
            if handle
                .update(|tray| tray.entries = current_menu())
                .is_none()
            {
                return 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The protocol wants ARGB with the most significant byte first.
    #[test]
    fn icons_are_argb_big_endian() {
        let size = 32;
        let rgba = switcher_icon::pixels(size);
        let argb = icon(size);
        assert_eq!(argb.data.len(), rgba.len());
        let i = ((12 * size + 8) * 4) as usize;
        assert_eq!(
            &argb.data[i..i + 4],
            &[rgba[i + 3], rgba[i], rgba[i + 1], rgba[i + 2]]
        );
    }
}
