//! Managing *this* computer's desktop around a monitor that is being sent to
//! the other computer.
//!
//! This exists because of a property of the hardware: these monitors keep
//! hot-plug detect asserted on every connected input, so Windows never
//! notices the screen has gone. That is exactly what makes a switch instant
//! and leaves the window layout untouched — and it is also why a window
//! sitting on that display becomes unreachable until the monitor comes back.
//!
//! Three behaviours, chosen by configuration:
//!
//! * **keep** (default) — do nothing at all. Fast, and windows can strand.
//! * **keep + sweep** — leave the display attached, but move windows off it
//!   first. Nothing strands, nothing about the desktop topology changes.
//! * **drop** — detach the display until it returns. The OS relocates the
//!   windows itself, at the cost of a reflow each way.
//!
//! Detaching is the only operation here that can leave someone unable to see
//! anything, so it refuses to act unless another usable display will remain.

use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod displayconfig;
#[cfg(windows)]
mod windows_impl;

#[cfg(windows)]
pub use windows_impl::WindowsDesktop;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DesktopError {
    #[error("desktop management is not available on this platform")]
    Unsupported,
    #[error("no display on this computer matches `{0}`")]
    DisplayNotFound(String),
    #[error("refusing to detach `{display}`: {reason}")]
    RefusedUnsafe { display: String, reason: String },
    #[error("{operation} failed: {detail}")]
    OsCall { operation: String, detail: String },
    #[error("no saved layout for `{0}`, so it cannot be restored")]
    NoSavedLayout(String),
}

/// A rectangle in virtual-desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn center(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }
}

/// One display as this computer's desktop sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopDisplay {
    /// GDI adapter name, e.g. `\\.\DISPLAY5`. Addresses the display to the OS.
    pub gdi_name: String,
    /// Device interface path, e.g. `\\?\DISPLAY#AOC2702#5&23e...&0&UID4613`.
    /// Shares a prefix with the id the PowerToys backend reports, which is how
    /// a monitor is matched between the two layers.
    pub device_path: String,
    pub friendly_name: Option<String>,
    pub rect: Rect,
    pub is_primary: bool,
    pub is_attached: bool,
    /// Internal laptop panel. Never detached: it is the recovery display.
    pub is_internal: bool,
}

impl DesktopDisplay {
    /// Whether this display is the one a monitor backend calls `backend_id`.
    ///
    /// Compared by prefix because the device interface path carries a trailing
    /// GUID that the backend's id does not.
    pub fn matches_backend_id(&self, backend_id: &str) -> bool {
        let normalise = |s: &str| s.trim().to_ascii_uppercase().replace('/', "\\");
        let mine = normalise(&self.device_path);
        let theirs = normalise(backend_id);
        if mine.is_empty() || theirs.is_empty() {
            return false;
        }
        mine.starts_with(&theirs) || theirs.starts_with(&mine)
    }
}

/// Enough of a display's mode to put it back exactly where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDisplayMode {
    pub width: u32,
    pub height: u32,
    pub pos_x: i32,
    pub pos_y: i32,
    pub refresh_hz: u32,
    pub bits_per_pixel: u32,
}

/// A window a sweep moved, and where it was before.
///
/// Identified by title rather than by window handle: a handle is only
/// meaningful inside the process that read it, and sweep and restore are
/// separate runs of the program. A title can be ambiguous or can change, so
/// restoring is best-effort and says which windows it could not find.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedWindow {
    pub title: String,
    /// Where it was before the sweep, in virtual-desktop coordinates.
    pub from: Rect,
    /// Whether it was maximized, so restoring can maximize it again.
    #[serde(default)]
    pub was_maximized: bool,
}

/// What a sweep actually moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub moved: Vec<MovedWindow>,
    /// Windows found on the display that were deliberately left alone.
    pub skipped: Vec<String>,
}

impl SweepReport {
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.skipped.is_empty()
    }
}

/// What a restore managed to put back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored: Vec<String>,
    /// Windows that could not be found again — closed, renamed, or opened on
    /// another virtual desktop since the sweep.
    pub missing: Vec<String>,
}

/// Operations on this computer's desktop.
///
/// Every method must be a no-op on a platform that cannot implement it, rather
/// than an error that blocks a switch: the monitor switch is the thing the
/// user asked for, and desktop tidying is a convenience on top.
pub trait DesktopManager {
    fn name(&self) -> &str;

    fn displays(&self) -> Result<Vec<DesktopDisplay>, DesktopError>;

    /// Move windows off `display` onto somewhere still visible.
    fn sweep_windows_off(&self, display: &DesktopDisplay) -> Result<SweepReport, DesktopError>;

    /// Put previously swept windows back where they were.
    fn restore_windows(&self, windows: &[MovedWindow]) -> Result<RestoreReport, DesktopError>;

    /// Make `display` the primary one.
    ///
    /// Needed because Windows refuses to detach the primary display, so
    /// releasing one that happens to be primary means promoting another
    /// first. Implementations must reposition the rest of the desktop to
    /// match, since the primary display defines the origin.
    fn set_primary(&self, display: &DesktopDisplay) -> Result<(), DesktopError>;

    /// Detach `display` from the desktop, returning what is needed to restore
    /// it. Must refuse when no other usable display would remain.
    fn detach(&self, display: &DesktopDisplay) -> Result<SavedDisplayMode, DesktopError>;

    /// Reattach `display` using a mode previously returned by [`Self::detach`].
    fn attach(&self, display: &DesktopDisplay, saved: SavedDisplayMode)
        -> Result<(), DesktopError>;
}

/// Finds nothing and does nothing. Used on platforms without an
/// implementation, so the calling code needs no `cfg` of its own.
pub struct NoopDesktop;

impl DesktopManager for NoopDesktop {
    fn name(&self) -> &str {
        "none"
    }

    fn displays(&self) -> Result<Vec<DesktopDisplay>, DesktopError> {
        Ok(Vec::new())
    }

    fn sweep_windows_off(&self, _display: &DesktopDisplay) -> Result<SweepReport, DesktopError> {
        Ok(SweepReport::default())
    }

    fn restore_windows(&self, _windows: &[MovedWindow]) -> Result<RestoreReport, DesktopError> {
        Ok(RestoreReport::default())
    }

    fn set_primary(&self, _display: &DesktopDisplay) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn detach(&self, _display: &DesktopDisplay) -> Result<SavedDisplayMode, DesktopError> {
        Err(DesktopError::Unsupported)
    }

    fn attach(
        &self,
        _display: &DesktopDisplay,
        _saved: SavedDisplayMode,
    ) -> Result<(), DesktopError> {
        Err(DesktopError::Unsupported)
    }
}

/// The desktop manager for the platform this was built for.
pub fn for_this_platform() -> Box<dyn DesktopManager> {
    #[cfg(windows)]
    {
        Box::new(WindowsDesktop::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(NoopDesktop)
    }
}

/// Pick the display a swept window should move to.
///
/// Prefers the primary display, then the largest remaining one, and never
/// returns the display being vacated.
pub fn sweep_target<'a>(
    displays: &'a [DesktopDisplay],
    leaving: &DesktopDisplay,
) -> Option<&'a DesktopDisplay> {
    let candidates: Vec<&DesktopDisplay> = displays
        .iter()
        .filter(|d| d.is_attached && d.gdi_name != leaving.gdi_name && !d.rect.is_empty())
        .collect();

    candidates
        .iter()
        .find(|d| d.is_primary)
        .or_else(|| {
            candidates
                .iter()
                .max_by_key(|d| i64::from(d.rect.width()) * i64::from(d.rect.height()))
        })
        .copied()
}

/// Whether detaching `display` would leave the desktop usable.
pub fn detach_is_safe(displays: &[DesktopDisplay], display: &DesktopDisplay) -> Result<(), String> {
    if display.is_internal {
        return Err(
            "it is the built-in panel, which is kept available as the recovery display".into(),
        );
    }
    // Checked before the primary rule, because being the only screen is the
    // more fundamental problem and deserves the clearer message.
    let others = displays
        .iter()
        .filter(|d| d.is_attached && d.gdi_name != display.gdi_name && !d.rect.is_empty())
        .count();
    if others == 0 {
        return Err(
            "it is the only attached display, so detaching it would leave no screen at all".into(),
        );
    }
    // Being the primary display is not refused here: Windows will not detach
    // it, but another display can be promoted first. That there is something
    // to promote is exactly what the check above established.
    Ok(())
}

/// Which display should become primary so `leaving` can be detached.
///
/// Windows refuses to detach the primary display, so releasing one that
/// happens to be primary means handing the role to something else first.
/// Returns `None` when `leaving` is not primary and nothing needs to change.
pub fn promotion_target<'a>(
    displays: &'a [DesktopDisplay],
    leaving: &DesktopDisplay,
) -> Option<&'a DesktopDisplay> {
    if !leaving.is_primary {
        return None;
    }
    displays
        .iter()
        .filter(|d| d.is_attached && d.gdi_name != leaving.gdi_name && !d.rect.is_empty())
        .max_by_key(|d| i64::from(d.rect.width()) * i64::from(d.rect.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(name: &str, path: &str, rect: Rect, primary: bool) -> DesktopDisplay {
        DesktopDisplay {
            gdi_name: name.into(),
            device_path: path.into(),
            friendly_name: None,
            rect,
            is_primary: primary,
            is_attached: true,
            is_internal: false,
        }
    }

    fn rect(left: i32, top: i32, w: i32, h: i32) -> Rect {
        Rect {
            left,
            top,
            right: left + w,
            bottom: top + h,
        }
    }

    #[test]
    fn matches_a_backend_id_despite_the_trailing_guid() {
        let d = display(
            "\\\\.\\DISPLAY5",
            "\\\\?\\DISPLAY#AOC2702#5&23e00778&0&UID4613#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
            rect(0, 0, 1920, 1080),
            true,
        );
        assert!(d.matches_backend_id("\\\\?\\DISPLAY#AOC2702#5&23e00778&0&UID4613"));
        // The sibling panel differs only in the UID, and must not match.
        assert!(!d.matches_backend_id("\\\\?\\DISPLAY#AOC2702#5&23e00778&0&UID4612"));
    }

    #[test]
    fn matching_is_case_insensitive_but_not_loose() {
        let d = display(
            "\\\\.\\DISPLAY5",
            "\\\\?\\display#aoc2702#5&23e00778&0&uid4613",
            rect(0, 0, 1920, 1080),
            true,
        );
        assert!(d.matches_backend_id("\\\\?\\DISPLAY#AOC2702#5&23E00778&0&UID4613"));
        assert!(!d.matches_backend_id(""));
        assert!(!d.matches_backend_id("\\\\?\\DISPLAY#BOE0CBF#4&34b9e9a7&0&UID8388688"));
    }

    #[test]
    fn sweep_prefers_the_primary_display() {
        let leaving = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), false);
        let displays = vec![
            leaving.clone(),
            display("\\\\.\\DISPLAY1", "p1", rect(1920, 0, 1707, 1067), true),
            display("\\\\.\\DISPLAY6", "p6", rect(-1920, 0, 1920, 1080), false),
        ];
        let target = sweep_target(&displays, &leaving).unwrap();
        assert_eq!(target.gdi_name, "\\\\.\\DISPLAY1");
    }

    #[test]
    fn sweep_falls_back_to_the_largest_remaining_display() {
        let leaving = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        let displays = vec![
            leaving.clone(),
            display("\\\\.\\DISPLAY1", "p1", rect(1920, 0, 1280, 720), false),
            display("\\\\.\\DISPLAY6", "p6", rect(-1920, 0, 1920, 1080), false),
        ];
        assert_eq!(
            sweep_target(&displays, &leaving).unwrap().gdi_name,
            "\\\\.\\DISPLAY6"
        );
    }

    #[test]
    fn sweep_has_nowhere_to_go_when_it_is_the_only_display() {
        let only = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        assert!(sweep_target(std::slice::from_ref(&only), &only).is_none());
    }

    #[test]
    fn detaching_the_last_display_is_refused() {
        let only = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        let err = detach_is_safe(std::slice::from_ref(&only), &only).unwrap_err();
        assert!(err.contains("no screen at all"), "{err}");
    }

    #[test]
    fn detaching_the_built_in_panel_is_refused() {
        let mut panel = display("\\\\.\\DISPLAY1", "p1", rect(0, 0, 1707, 1067), true);
        panel.is_internal = true;
        let other = display("\\\\.\\DISPLAY5", "p5", rect(1707, 0, 1920, 1080), false);
        let displays = vec![panel.clone(), other];
        let err = detach_is_safe(&displays, &panel).unwrap_err();
        assert!(err.contains("recovery display"), "{err}");
    }

    #[test]
    fn detaching_the_primary_is_allowed_because_another_can_be_promoted() {
        let primary = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        let other = display("\\\\.\\DISPLAY1", "p1", rect(1920, 0, 2560, 1600), false);
        let displays = vec![primary.clone(), other];
        assert!(detach_is_safe(&displays, &primary).is_ok());
        assert_eq!(
            promotion_target(&displays, &primary).unwrap().gdi_name,
            "\\\\.\\DISPLAY1"
        );
    }

    #[test]
    fn nothing_is_promoted_when_the_departing_display_is_not_primary() {
        let leaving = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), false);
        let displays = vec![
            leaving.clone(),
            display("\\\\.\\DISPLAY1", "p1", rect(1920, 0, 2560, 1600), true),
        ];
        assert!(promotion_target(&displays, &leaving).is_none());
    }

    #[test]
    fn promotion_picks_the_largest_remaining_display() {
        let primary = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        let displays = vec![
            primary.clone(),
            display("\\\\.\\DISPLAY6", "p6", rect(-1920, 0, 1920, 1080), false),
            display("\\\\.\\DISPLAY1", "p1", rect(1920, 0, 2560, 1600), false),
        ];
        assert_eq!(
            promotion_target(&displays, &primary).unwrap().gdi_name,
            "\\\\.\\DISPLAY1"
        );
    }

    #[test]
    fn detaching_is_allowed_when_another_display_remains() {
        let leaving = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), false);
        let displays = vec![
            leaving.clone(),
            display("\\\\.\\DISPLAY1", "p1", rect(1920, 0, 1707, 1067), true),
        ];
        assert!(detach_is_safe(&displays, &leaving).is_ok());
    }

    #[test]
    fn an_already_detached_display_does_not_count_as_a_survivor() {
        let leaving = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        let mut gone = display("\\\\.\\DISPLAY6", "p6", rect(0, 0, 0, 0), false);
        gone.is_attached = false;
        let displays = vec![leaving.clone(), gone];
        assert!(detach_is_safe(&displays, &leaving).is_err());
    }

    #[test]
    fn the_noop_manager_never_pretends_to_have_detached_anything() {
        let d = display("\\\\.\\DISPLAY5", "p5", rect(0, 0, 1920, 1080), true);
        let noop = NoopDesktop;
        assert!(noop.displays().unwrap().is_empty());
        assert!(noop.sweep_windows_off(&d).unwrap().is_empty());
        assert_eq!(noop.detach(&d).unwrap_err(), DesktopError::Unsupported);
    }

    #[test]
    fn rect_geometry() {
        let r = rect(100, 50, 800, 600);
        assert_eq!((r.width(), r.height()), (800, 600));
        assert_eq!(r.center(), (500, 350));
        assert!(r.contains(100, 50));
        assert!(!r.contains(900, 50));
        assert!(rect(0, 0, 0, 0).is_empty());
    }
}
