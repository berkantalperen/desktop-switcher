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
use switcher_desktop::{DesktopDisplay, DesktopManager, SavedDisplayMode};

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

/// Device paths that must never be detached.
///
/// The internal laptop panel is the recovery display. The monitor backend can
/// recognise it because Windows reaches it over WMI rather than DDC/CI, so
/// that knowledge is passed down rather than guessed at here.
pub fn protected_paths(detected: &[DetectedMonitor]) -> Vec<String> {
    detected
        .iter()
        .filter(|m| matches!(m.identity.transport, Transport::Wmi))
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
        .filter(|d| d.matches_backend_id(backend_id))
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

/// Resolve a monitor argument to its backend id and a human label.
fn target_monitor(app: &App, monitor: Option<&str>) -> Result<(String, String)> {
    let config = app
        .config
        .as_ref()
        .ok_or_else(|| anyhow!("no configuration yet. Run `desktop-switcher configure` first."))?;

    match monitor {
        Some(name) => {
            let m = config.monitor(name).ok_or_else(|| {
                anyhow!(
                    "`{name}` is not a configured monitor. Configured: {}",
                    config
                        .monitors
                        .iter()
                        .map(|m| m.logical_id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            Ok((m.backend_id.clone(), m.logical_id.clone()))
        }
        None => match config.monitors.as_slice() {
            [one] => Ok((one.backend_id.clone(), one.logical_id.clone())),
            [] => bail!("no monitors are configured"),
            many => bail!(
                "several monitors are configured ({}); name one with --monitor",
                many.iter()
                    .map(|m| m.logical_id.as_str())
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

    let Some((key, saved)) = saved else {
        bail!(
            "`{label}` is detached, but this tool has no saved layout for it, so it \
             cannot restore the previous resolution and position. Reattach it from \
             Windows display settings instead."
        );
    };

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

    if report.moved.is_empty() && report.skipped.is_empty() {
        println!("No windows were on `{label}`.");
        return Ok(0);
    }
    println!("Moved {} window(s) off `{label}`:", report.moved.len());
    for title in &report.moved {
        ui::bullet(title);
    }
    if !report.skipped.is_empty() {
        println!("\nCould not move {} window(s):", report.skipped.len());
        for title in &report.skipped {
            ui::bullet(title);
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// side effects applied around a `switch`
// ---------------------------------------------------------------------------

/// Apply the outbound layer-2 action for one monitor, before its input moves.
///
/// Failures here are reported but never abort the switch: changing the input
/// is what the user asked for, and tidying the desktop is a convenience.
pub fn apply_away(app: &App, backend_id: &str, label: &str, release_it: bool) {
    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let Ok(display) = display_for(manager.as_ref(), backend_id) else {
        return;
    };
    if !display.is_attached {
        return;
    }

    if release_it {
        match manager.detach(&display) {
            Ok(saved) => {
                let mut state = ReleasedState::load(&app.state_dir);
                state.released.insert(backend_id.to_string(), saved);
                let _ = state.save(&app.state_dir);
                println!("  {label:<10} released from this desktop");
            }
            Err(e) => ui::warn(format!("could not release `{label}`: {e}")),
        }
        return;
    }

    match manager.sweep_windows_off(&display) {
        Ok(report) if report.moved.is_empty() => {}
        Ok(report) => println!(
            "  {label:<10} moved {} window(s) off it",
            report.moved.len()
        ),
        Err(e) => ui::warn(format!("could not sweep windows off `{label}`: {e}")),
    }
}

/// Reattach a monitor this computer had released, after its input comes back.
pub fn apply_home(app: &App, backend_id: &str, label: &str) {
    let mut state = ReleasedState::load(&app.state_dir);
    let Some((key, saved)) = state
        .released
        .iter()
        .find(|(k, _)| k.as_str() == backend_id)
        .map(|(k, v)| (k.clone(), *v))
    else {
        return; // never released by us, so nothing to restore
    };

    let detected = app.backend.discover().unwrap_or_default();
    let manager = manager(&detected);
    let Ok(display) = display_for(manager.as_ref(), backend_id) else {
        return;
    };
    if display.is_attached {
        state.released.remove(&key);
        let _ = state.save(&app.state_dir);
        return;
    }

    match manager.attach(&display, saved) {
        Ok(()) => {
            state.released.remove(&key);
            let _ = state.save(&app.state_dir);
            println!("  {label:<10} reattached to this desktop");
        }
        Err(e) => ui::warn(format!("could not reattach `{label}`: {e}")),
    }
}
