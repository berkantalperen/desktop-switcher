//! Layer 2: this computer's desktop.
//!
//! Separate from switching the monitor's input, and driven by its own
//! commands. A monitor can be displayed by the other computer while this one
//! still has it attached — that is the normal state, and it is why a window
//! can end up somewhere you cannot see it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use switcher_core::types::{DetectedMonitor, Transport};
use switcher_desktop::{DesktopDisplay, DesktopManager, MovedWindow, SavedDisplayMode};

use crate::ui;
use crate::App;

/// Displays this computer has released, and how to put each one back.
///
/// Persisted because a release and its matching claim are separate runs of
/// the program; without this, a detached display could not be restored to the
/// resolution and position it had.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleasedState {
    #[serde(default)]
    pub released: BTreeMap<String, SavedDisplayMode>,
    /// Displays that were the Windows primary when they were released.
    ///
    /// Windows refuses to detach the primary display, so releasing one means
    /// handing the role to another screen first. Remembering that lets
    /// `claim` give it back rather than silently leaving the desktop
    /// rearranged.
    #[serde(default)]
    pub was_primary: Vec<String>,
}

impl ReleasedState {
    fn path(dir: &Path) -> PathBuf {
        dir.join("released-displays.toml")
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(Self::path(dir), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Where windows were before the last sweep, so it can be undone.
///
/// Keyed by monitor, because sweeping two monitors and restoring one should
/// only put back what came off that one. Kept on disk because sweep and
/// restore are separate runs of the program.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SweptState {
    #[serde(default)]
    pub swept: BTreeMap<String, Vec<MovedWindow>>,
}

impl SweptState {
    fn path(dir: &Path) -> PathBuf {
        dir.join("swept-windows.toml")
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(Self::path(dir), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Device paths that must never be detached.
///
/// The internal laptop panel is the recovery display. The desktop layer also
/// recognises it from the graphics driver on its own; this passes down what
/// the monitor backend knows as a second, independent source.
pub fn protected_paths(detected: &[DetectedMonitor]) -> Vec<String> {
    detected
        .iter()
        .filter(|m| matches!(m.identity.transport, Transport::Internal))
        .map(|m| m.identity.backend_id.clone())
        .collect()
}

pub fn manager(detected: &[DetectedMonitor]) -> Box<dyn DesktopManager> {
    let protected = protected_paths(detected);
    #[cfg(windows)]
    {
        Box::new(switcher_desktop::WindowsDesktop::protecting(protected))
    }
    #[cfg(not(windows))]
    {
        let _ = protected;
        Box::new(switcher_desktop::NoopDesktop)
    }
}

/// Find the desktop display corresponding to a configured monitor.
pub fn display_for(manager: &dyn DesktopManager, backend_id: &str) -> Result<DesktopDisplay> {
    let displays = manager.displays()?;
    if displays.is_empty() {
        bail!(
            "this build cannot manage desktop attachment on this platform \
             (`{}` backend); `release`, `claim` and `sweep` are Windows-only for now",
            manager.name()
        );
    }
    let matches: Vec<&DesktopDisplay> = displays
        .iter()
        .filter(|d| d.matches_backend_id(backend_id) || d.gdi_name == backend_id)
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(anyhow!(
            "no display on this computer matches `{backend_id}`. \
             It may already be released; run `desktop-switcher displays` to see."
        )),
        many => Err(anyhow!(
            "`{backend_id}` matches {} displays; refusing to guess",
            many.len()
        )),
    }
}

/// Refuse a topology change unless the user has explicitly opted in.
///
/// These three commands are the only ones in the project that have caused
/// real damage: on the development hardware they have left two monitors
/// mirrored instead of extended, and reported a detached display as attached
/// so it could not be reattached. `sweep` covers the problem they were built
/// for, without touching topology, so the safe default is off.
fn require_experimental(app: &App, command: &str) -> Result<()> {
    if app.experimental {
        return Ok(());
    }
    bail!(
        "`{command}` changes the Windows display topology and is not reliable yet.\n\n\
         On this hardware it has left monitors mirrored instead of extended, and has \
         reported a detached display as attached. Recovering needs Settings > System > \
         Display.\n\n\
         If you want the stranded-window problem solved, use `desktop-switcher sweep` \
         instead: it moves windows off a monitor without changing topology and cannot \
         cost you a screen.\n\n\
         To run it anyway, pass --experimental."
    )
}

/// Resolve a monitor argument to its backend id and a human label.
fn target_monitor(app: &App, monitor: Option<&str>) -> Result<(String, String)> {
    let config = app
        .config
        .as_ref()
        .ok_or_else(|| anyhow!("no configuration yet. Run `desktop-switcher configure` first."))?;

    match monitor {
        Some(name) => match config.monitor(name) {
            Some(m) => Ok((m.backend_id.clone(), m.label.clone())),
            // Not a configured logical id, so take it as a raw selector: a
            // device path or a GDI name. A display this tool does not manage
            // can still get stranded by a topology change, and refusing to
            // name it would leave no way to put it back.
            None => Ok((name.to_string(), name.to_string())),
        },
        None => match config.monitors.as_slice() {
            [one] => Ok((one.backend_id.clone(), one.label.clone())),
            [] => bail!("no monitors are configured"),
            many => bail!(
                "several monitors are configured ({}); name one with --monitor",
                many.iter()
                    .map(|m| m.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

pub fn displays(app: &App) -> Result<i32> {
    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let displays = manager.displays()?;

    ui::heading(&format!("Desktop displays ({} backend)", manager.name()));
    if displays.is_empty() {
        println!("  Desktop attachment is not available on this platform.");
        return Ok(1);
    }

    let released = ReleasedState::load(&app.state_dir);
    for d in &displays {
        println!();
        println!(
            "{}{}",
            d.friendly_name.as_deref().unwrap_or(&d.gdi_name),
            if d.is_primary { "  [primary]" } else { "" }
        );
        ui::field("gdi name", &d.gdi_name);
        ui::field("device path", &d.device_path);
        if let Some(connector) = d.connector {
            // Describes this computer's end of the cable. Deliberately not
            // turned into a monitor input code; see `Connector`.
            ui::field("wired via", connector.to_string());
        }
        ui::field(
            "attached",
            if d.is_attached {
                "yes — this computer draws on it"
            } else {
                "no — not part of this desktop"
            },
        );
        if d.is_attached {
            ui::field(
                "area",
                format!(
                    "{}x{} at ({}, {})",
                    d.rect.width(),
                    d.rect.height(),
                    d.rect.left,
                    d.rect.top
                ),
            );
        }
        // Two attached displays sharing an area are mirrored, not extended.
        // Windows' own Screen API collapses them into one entry, which makes
        // a mirrored pair look exactly like a missing monitor.
        let mirrors: Vec<&str> = displays
            .iter()
            .filter(|o| {
                o.is_attached
                    && d.is_attached
                    && o.device_path != d.device_path
                    && o.rect == d.rect
                    && !o.rect.is_empty()
            })
            .map(|o| o.friendly_name.as_deref().unwrap_or(&o.gdi_name))
            .collect();
        if !mirrors.is_empty() {
            ui::field(
                "mirrored with",
                format!(
                    "{} — same area, so they show the same thing",
                    mirrors.join(", ")
                ),
            );
        }
        if d.is_internal {
            ui::field("protected", "built-in panel; never detached");
        }
        if released.released.keys().any(|k| d.matches_backend_id(k)) {
            ui::field("note", "released by this tool; `claim` will restore it");
        }
    }
    Ok(0)
}

/// Make a monitor the primary display.
///
/// Part of layer 2 in its own right, and the operation `release` needs first
/// when the display it is detaching happens to be primary.
pub fn set_primary(app: &App, monitor: Option<&str>) -> Result<i32> {
    require_experimental(app, "primary")?;
    let (backend_id, label) = target_monitor(app, monitor)?;
    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let display = display_for(manager.as_ref(), &backend_id)?;

    if display.is_primary {
        println!("`{label}` is already the primary display. Nothing to do.");
        return Ok(0);
    }
    manager.set_primary(&display)?;
    app.log.append(
        "desktop.set-primary",
        &[
            ("monitor", label.clone()),
            ("gdi", display.gdi_name.clone()),
        ],
    );
    println!("`{label}` is now the primary display.");
    Ok(0)
}

pub fn release(app: &App, monitor: Option<&str>) -> Result<i32> {
    require_experimental(app, "release")?;
    let (backend_id, label) = target_monitor(app, monitor)?;
    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let display = display_for(manager.as_ref(), &backend_id)?;

    if !display.is_attached {
        println!("`{label}` is already detached from this desktop. Nothing to do.");
        return Ok(0);
    }

    // Windows will not detach the primary display, so hand the role over
    // first and remember that we did.
    let was_primary = display.is_primary;
    let display = if was_primary {
        let all = manager.displays()?;
        let Some(promote) = switcher_desktop::promotion_target(&all, &display) else {
            bail!(
                "`{label}` is the primary display and there is no other attached display \
                 to hand that role to, so it cannot be released."
            );
        };
        println!(
            "`{label}` is the primary display; making {} primary first.",
            promote
                .friendly_name
                .as_deref()
                .unwrap_or(&promote.gdi_name)
        );
        manager.set_primary(promote)?;
        // Geometry moved when the origin moved, so re-read it.
        display_for(manager.as_ref(), &backend_id)?
    } else {
        display
    };

    // Promotion has already changed the desktop. If the detach now fails,
    // undo it rather than leave a half-finished rearrangement behind: the
    // user asked to release a monitor, not to have their primary display
    // quietly moved somewhere else and left there.
    let saved = match manager.detach(&display) {
        Ok(saved) => saved,
        Err(detach_error) if was_primary => {
            let rollback = display_for(manager.as_ref(), &backend_id)
                .and_then(|d| manager.set_primary(&d).map_err(Into::into));
            return match rollback {
                Ok(()) => Err(anyhow!(
                    "{detach_error}\n\nThe primary display was moved in order to attempt this \
                     and has been put back, so nothing has changed overall."
                )),
                Err(rollback_error) => Err(anyhow!(
                    "{detach_error}\n\nWorse: `{label}` had already been demoted from primary \
                     to attempt the detach, and restoring it failed too ({rollback_error}). \
                     Your desktop arrangement has changed. Put it back in Settings > System > \
                     Display: select the display you want and tick \"Make this my main \
                     display\"."
                )),
            };
        }
        Err(detach_error) => return Err(detach_error.into()),
    };

    let mut state = ReleasedState::load(&app.state_dir);
    state.released.insert(backend_id.clone(), saved);
    if was_primary && !state.was_primary.contains(&backend_id) {
        state.was_primary.push(backend_id.clone());
    }
    state.save(&app.state_dir)?;

    app.log.append(
        "desktop.release",
        &[
            ("monitor", label.clone()),
            ("gdi", display.gdi_name.clone()),
        ],
    );
    println!("Released `{label}` from this desktop.");
    println!("Windows that were on it have been relocated by the OS.");
    println!("Run `desktop-switcher claim {label}` to bring it back.");
    Ok(0)
}

pub fn claim(app: &App, monitor: Option<&str>) -> Result<i32> {
    require_experimental(app, "claim")?;
    let (backend_id, label) = target_monitor(app, monitor)?;
    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let display = display_for(manager.as_ref(), &backend_id)?;

    if display.is_attached {
        println!("`{label}` is already attached to this desktop. Nothing to do.");
        return Ok(0);
    }

    let mut state = ReleasedState::load(&app.state_dir);
    let saved = state
        .released
        .iter()
        .find(|(k, _)| display.matches_backend_id(k))
        .map(|(k, v)| (k.clone(), *v));

    // No saved layout is not a reason to refuse. Attaching the display is the
    // part that matters; Windows will pick a resolution and position, and
    // having it back in the wrong place beats not having it back.
    let (key, saved) = saved.unwrap_or_else(|| {
        println!("No saved layout for `{label}`; Windows will choose its size and position.");
        (
            backend_id.clone(),
            SavedDisplayMode {
                width: 0,
                height: 0,
                pos_x: 0,
                pos_y: 0,
                refresh_hz: 0,
                bits_per_pixel: 0,
            },
        )
    });

    manager.attach(&display, saved)?;
    state.released.remove(&key);

    // Give the primary role back if releasing had taken it away.
    let restore_primary = state.was_primary.contains(&key);
    if restore_primary {
        // Re-read: the display only exists in the desktop again now.
        match display_for(manager.as_ref(), &backend_id)
            .and_then(|d| manager.set_primary(&d).map_err(Into::into))
        {
            Ok(()) => println!("Restored `{label}` as the primary display."),
            Err(e) => ui::warn(format!(
                "reattached `{label}` but could not make it primary again: {e}"
            )),
        }
        state.was_primary.retain(|p| *p != key);
    }
    state.save(&app.state_dir)?;

    app.log.append(
        "desktop.claim",
        &[
            ("monitor", label.clone()),
            ("gdi", display.gdi_name.clone()),
        ],
    );
    println!(
        "Claimed `{label}`: restored at {}x{} and ({}, {}).",
        saved.width, saved.height, saved.pos_x, saved.pos_y
    );
    Ok(0)
}

pub fn sweep(app: &App, monitor: Option<&str>) -> Result<i32> {
    let (backend_id, label) = target_monitor(app, monitor)?;
    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let display = display_for(manager.as_ref(), &backend_id)?;

    if !display.is_attached {
        println!("`{label}` is not attached to this desktop, so nothing can be on it.");
        return Ok(0);
    }

    let report = manager.sweep_windows_off(&display)?;
    app.log.append(
        "desktop.sweep",
        &[
            ("monitor", label.clone()),
            ("moved", report.moved.len().to_string()),
        ],
    );

    // Remember where they came from so the move can be undone. Only replace
    // a previous record when something actually moved, or a second sweep
    // would wipe the positions the first one saved.
    if !report.moved.is_empty() {
        let mut state = SweptState::load(&app.state_dir);
        state.swept.insert(backend_id.clone(), report.moved.clone());
        state.save(&app.state_dir)?;
    }

    if report.moved.is_empty() && report.skipped.is_empty() {
        println!("No windows were on `{label}`.");
        return Ok(0);
    }
    println!("Moved {} window(s) off `{label}`:", report.moved.len());
    for window in &report.moved {
        ui::bullet(&window.title);
    }
    println!("\nPut them back with: desktop-switcher restore-windows");
    if !report.skipped.is_empty() {
        println!("\nCould not move {} window(s):", report.skipped.len());
        for title in &report.skipped {
            ui::bullet(title);
        }
    }
    Ok(0)
}

/// Put windows back where a sweep found them.
///
/// Best effort by design: windows are matched by title, because a window
/// handle means nothing outside the process that read it. A window that has
/// since been closed or renamed is reported as missing rather than silently
/// skipped.
pub fn restore_windows(app: &App, monitor: Option<&str>) -> Result<i32> {
    let mut state = SweptState::load(&app.state_dir);
    if state.swept.is_empty() {
        println!("Nothing has been swept, so there is nothing to put back.");
        return Ok(0);
    }

    // Naming a monitor restores only what came off that one.
    let keys: Vec<String> = match monitor {
        Some(_) => {
            let (backend_id, _) = target_monitor(app, monitor)?;
            if !state.swept.contains_key(&backend_id) {
                println!("Nothing was swept off that monitor.");
                return Ok(0);
            }
            vec![backend_id]
        }
        None => state.swept.keys().cloned().collect(),
    };

    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let mut restored_any = false;

    for key in keys {
        let Some(windows) = state.swept.get(&key).cloned() else {
            continue;
        };
        let report = manager.restore_windows(&windows)?;
        app.log.append(
            "desktop.restore",
            &[
                ("monitor", key.clone()),
                ("restored", report.restored.len().to_string()),
                ("missing", report.missing.len().to_string()),
            ],
        );

        if !report.restored.is_empty() {
            restored_any = true;
            println!("Put {} window(s) back:", report.restored.len());
            for title in &report.restored {
                ui::bullet(title);
            }
        }
        if !report.missing.is_empty() {
            println!("\nCould not find {} window(s):", report.missing.len());
            for title in &report.missing {
                ui::bullet(title);
            }
            println!("  (closed, renamed, or on another virtual desktop)");
        }
        state.swept.remove(&key);
    }

    state.save(&app.state_dir)?;
    if !restored_any {
        println!("None of the swept windows could be found.");
        return Ok(1);
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
