//! Setting a monitor's input, and reporting honestly what happened.
//!
//! The unit of work is **one monitor, one input code**. That is the only
//! thing the hardware offers, and it is all this layer believes in. Grouping
//! several of them into "the desk setup" is a person's idea and belongs in an
//! action, not here.
//!
//! Two rules shape the rest:
//!
//! 1. A set is absolute, never a step or a cycle. Issuing it twice is
//!    harmless, so a repeated hotkey cannot walk a monitor through its inputs.
//! 2. A zero exit status is not a switched monitor. The report distinguishes
//!    "confirmed by read", "issued but unconfirmed", and "failed", and never
//!    rounds the middle one up.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::backend::MonitorBackend;
use crate::config::Config;
use crate::eventlog::EventLog;
use crate::inventory::{bind, BindingProblem};
use crate::types::{DetectedMonitor, InputCode, InputReading, MonitorHandle, WriteOutcome};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("no monitors are configured. Run `desktop-switcher configure` first.")]
    NoMonitorsConfigured,
    #[error("`{requested}` is not a configured monitor. Configured: {}", known.join(", "))]
    UnknownMonitor {
        requested: String,
        known: Vec<String>,
    },
    #[error("cannot identify `{}`: {problem}", problem.monitor())]
    Binding { problem: BindingProblem },
}

/// One monitor about to be set to one input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSet {
    /// The monitor's configured key.
    pub key: String,
    /// Its human label, for reporting.
    pub label: String,
    pub handle: MonitorHandle,
    pub code: InputCode,
    /// Whether the monitor advertises this input. Advisory: capability
    /// strings are routinely wrong in both directions, so an unadvertised
    /// code is worth a warning and not a refusal.
    pub advertised: bool,
}

/// Resolve "set this monitor to this input" against what is actually attached.
pub fn plan_set_input(
    config: &Config,
    detected: &[DetectedMonitor],
    monitor_name: &str,
    code: InputCode,
) -> Result<PlannedSet, PlanError> {
    if config.monitors.is_empty() {
        return Err(PlanError::NoMonitorsConfigured);
    }
    let Some(cfg) = config.monitor(monitor_name) else {
        return Err(PlanError::UnknownMonitor {
            requested: monitor_name.to_string(),
            known: config.monitors.iter().map(|m| m.key.clone()).collect(),
        });
    };

    let bindings = bind(config, detected);
    if let Some(problem) = bindings
        .problems
        .iter()
        .find(|p| p.monitor() == cfg.key)
        .cloned()
    {
        return Err(PlanError::Binding { problem });
    }
    let binding = bindings
        .get(&cfg.key)
        .expect("a monitor with no binding problem is bound");

    Ok(PlannedSet {
        key: cfg.key.clone(),
        label: cfg.label.clone(),
        handle: binding.handle(),
        code,
        advertised: cfg.input(code).is_some(),
    })
}

#[derive(Debug, Clone, Copy)]
pub struct ExecOptions {
    /// Pause after a write before attempting to read it back.
    pub settle: Duration,
    pub read_back: bool,
    /// Report what would happen without touching the hardware.
    pub dry_run: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            settle: Duration::from_millis(1200),
            read_back: true,
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub key: String,
    pub label: String,
    pub requested: InputCode,
    pub before: Option<InputReading>,
    pub outcome: WriteOutcome,
    pub after: Option<InputReading>,
    /// Set when no write was issued because the monitor was already there.
    pub skipped_already_correct: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyStatus {
    /// Every monitor read back as requested.
    AllConfirmed,
    /// Every write was accepted, but at least one could not be verified.
    AllIssuedSomeUnconfirmed,
    Partial,
    AllFailed,
}

impl fmt::Display for ApplyStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ApplyStatus::AllConfirmed => "confirmed",
            ApplyStatus::AllIssuedSomeUnconfirmed => {
                "issued; could not be confirmed (expected if the input belongs to another computer)"
            }
            ApplyStatus::Partial => "PARTIAL: some monitors failed",
            ApplyStatus::AllFailed => "FAILED: nothing was switched",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub steps: Vec<StepReport>,
    pub dry_run: bool,
}

impl ApplyReport {
    pub fn status(&self) -> ApplyStatus {
        let failed = self.steps.iter().filter(|s| s.outcome.is_failure()).count();
        if !self.steps.is_empty() && failed == self.steps.len() {
            return ApplyStatus::AllFailed;
        }
        if failed > 0 {
            return ApplyStatus::Partial;
        }
        if self
            .steps
            .iter()
            .all(|s| matches!(s.outcome, WriteOutcome::ConfirmedByRead(_)))
        {
            ApplyStatus::AllConfirmed
        } else {
            ApplyStatus::AllIssuedSomeUnconfirmed
        }
    }

    /// Exit code: 0 confirmed or issued, 1 partial, 2 total failure.
    pub fn exit_code(&self) -> i32 {
        match self.status() {
            ApplyStatus::AllConfirmed | ApplyStatus::AllIssuedSomeUnconfirmed => 0,
            ApplyStatus::Partial => 1,
            ApplyStatus::AllFailed => 2,
        }
    }
}

/// Set one monitor's input.
///
/// No rollback and no retry live here. A write is never repeated on this
/// layer: it may already have been accepted, and repeating an input switch
/// during re-enumeration compounds the disconnect. A backend that can *prove*
/// its write never reached the panel may retry internally, which is a
/// different and safe thing.
pub fn apply_one(
    plan: &PlannedSet,
    backend: &dyn MonitorBackend,
    opts: ExecOptions,
    log: &EventLog,
) -> StepReport {
    let before = read_best_effort(backend, &plan.handle);

    if opts.dry_run {
        return StepReport {
            key: plan.key.clone(),
            label: plan.label.clone(),
            requested: plan.code,
            before,
            outcome: WriteOutcome::Unknown {
                detail: "dry run: no write issued".into(),
            },
            after: None,
            skipped_already_correct: false,
        };
    }

    // Already there: the set is a no-op, so skip the write entirely. This is
    // what makes pressing the same hotkey twice safe rather than a cycle.
    if before.as_ref().and_then(|r| r.value()) == Some(plan.code) {
        log.append(
            "input.skip",
            &[
                ("monitor", plan.key.clone()),
                ("code", plan.code.to_string()),
                ("reason", "already-on-requested-input".into()),
            ],
        );
        return StepReport {
            key: plan.key.clone(),
            label: plan.label.clone(),
            requested: plan.code,
            before,
            outcome: WriteOutcome::ConfirmedByRead(plan.code),
            after: None,
            skipped_already_correct: true,
        };
    }

    let outcome = match backend.set_input(&plan.handle, plan.code) {
        Ok(o) => o,
        Err(e) => WriteOutcome::Failed {
            detail: e.to_string(),
        },
    };
    log.append(
        "input.write",
        &[
            ("monitor", plan.key.clone()),
            ("id", plan.handle.backend_id.clone()),
            ("code", plan.code.to_string()),
            ("outcome", outcome.to_string()),
        ],
    );

    let mut after = None;
    let mut final_outcome = outcome;
    if opts.read_back && !final_outcome.is_failure() {
        if !opts.settle.is_zero() {
            std::thread::sleep(opts.settle);
        }
        let reading = read_after_write(backend, &plan.handle);
        final_outcome = match &reading {
            InputReading::Value(v) if *v == plan.code => WriteOutcome::ConfirmedByRead(*v),
            InputReading::Value(v) => WriteOutcome::Unknown {
                detail: format!("read back {v}, expected {}", plan.code),
            },
            // Losing contact right after the write is the normal outcome when
            // the input we selected belongs to another computer: the monitor
            // stops answering this one.
            InputReading::UnavailableAfterSwitch { .. } => WriteOutcome::Accepted,
            InputReading::ReadFailed { detail } => WriteOutcome::Unknown {
                detail: format!("write accepted, read-back failed: {detail}"),
            },
            InputReading::Unsupported => WriteOutcome::Accepted,
        };
        after = Some(reading);
    }

    StepReport {
        key: plan.key.clone(),
        label: plan.label.clone(),
        requested: plan.code,
        before,
        outcome: final_outcome,
        after,
        skipped_already_correct: false,
    }
}

/// Apply several sets in order, independently.
///
/// One monitor failing does not abort the others: leaving half a desk on the
/// wrong input is worse than finishing the job. Nothing is rolled back —
/// with connectivity unknown, "undoing" a write is just another blind write.
pub fn apply_many(
    plans: &[PlannedSet],
    backend: &dyn MonitorBackend,
    opts: ExecOptions,
    log: &EventLog,
) -> ApplyReport {
    ApplyReport {
        steps: plans
            .iter()
            .map(|p| apply_one(p, backend, opts, log))
            .collect(),
        dry_run: opts.dry_run,
    }
}

fn read_best_effort(backend: &dyn MonitorBackend, handle: &MonitorHandle) -> Option<InputReading> {
    match backend.read_input(handle) {
        Ok(r) => Some(r),
        Err(e) => Some(InputReading::ReadFailed {
            detail: e.to_string(),
        }),
    }
}

/// A read that fails immediately after a write is reported as
/// "unavailable after switch" rather than as a fault: we cannot know whether
/// the input we just selected belongs to another machine, and treating the
/// silence as an error would call a successful switch a failure.
fn read_after_write(backend: &dyn MonitorBackend, handle: &MonitorHandle) -> InputReading {
    match backend.read_input(handle) {
        Ok(InputReading::ReadFailed { detail }) => InputReading::UnavailableAfterSwitch { detail },
        Ok(r) => r,
        Err(e) => InputReading::UnavailableAfterSwitch {
            detail: e.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Serialising concurrent changes
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("another change is already running (lock held at {path})")]
    Busy { path: String },
    #[error("could not create lock at {path}: {detail}")]
    Io { path: String, detail: String },
}

/// A cross-process lock, so two hotkeys or a hotkey and the GUI cannot drive
/// the same DDC interfaces at once.
#[derive(Debug)]
pub struct SwitchGuard {
    path: PathBuf,
}

impl SwitchGuard {
    pub fn acquire(dir: &Path, stale_after: Duration) -> Result<Self, GuardError> {
        std::fs::create_dir_all(dir).map_err(|e| GuardError::Io {
            path: dir.display().to_string(),
            detail: e.to_string(),
        })?;
        let path = dir.join("switch.lock");

        // A lock left behind by a killed process must not wedge the tool.
        if let Ok(meta) = std::fs::metadata(&path) {
            let stale = meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .map(|age| age > stale_after)
                .unwrap_or(true);
            if stale {
                let _ = std::fs::remove_file(&path);
            }
        }

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Self { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(GuardError::Busy {
                path: path.display().to_string(),
            }),
            Err(e) => Err(GuardError::Io {
                path: path.display().to_string(),
                detail: e.to_string(),
            }),
        }
    }
}

impl Drop for SwitchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Last request, tracked separately from observed state
// ---------------------------------------------------------------------------

/// What this tool last asked for.
///
/// Deliberately not called "current state": inputs can be changed with the
/// monitor's own buttons at any time, and by any other computer attached to
/// it. This is intent, and it is reported as intent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRequest {
    /// The action name, or a description of the one-off request.
    pub request: Option<String>,
    /// Milliseconds since the Unix epoch. `u64`, not `u128`: TOML integers
    /// are 64-bit, and a `u128` silently fails to serialise.
    pub at_unix_ms: Option<u64>,
}

impl LastRequest {
    fn path(dir: &Path) -> PathBuf {
        dir.join("last-request.toml")
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Returns whether it was actually written, so a silent failure cannot
    /// quietly disable the duplicate-press guard.
    pub fn record(dir: &Path, request: &str) -> bool {
        let value = Self {
            request: Some(request.to_string()),
            at_unix_ms: Some(now_unix_ms()),
        };
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        match toml::to_string_pretty(&value) {
            Ok(text) => std::fs::write(Self::path(dir), text).is_ok(),
            Err(_) => false,
        }
    }

    /// Whether this request repeats one just made, e.g. a shortcut key held
    /// down. A zero window disables the guard rather than swallowing
    /// everything in the same millisecond.
    pub fn is_duplicate_of(&self, request: &str, window: Duration) -> bool {
        let (Some(last), Some(at)) = (self.request.as_deref(), self.at_unix_ms) else {
            return false;
        };
        if last != request {
            return false;
        }
        u128::from(now_unix_ms().saturating_sub(at)) < window.as_millis()
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendKind, MonitorConfig, MonitorInput};
    use crate::fake::{FakeBackend, FakeBehavior, FakeMonitor};

    const HDMI: u8 = 0x11;
    const DP: u8 = 0x0F;

    fn monitor_cfg(key: &str, backend_id: &str) -> MonitorConfig {
        MonitorConfig {
            key: key.into(),
            label: format!("AOC 27P2DG5 ({key})"),
            backend_id: backend_id.into(),
            serial: Some(key.into()),
            model: Some("27P2DG5".into()),
            inputs: vec![
                MonitorInput::reported(InputCode(HDMI), None),
                MonitorInput::reported(InputCode(DP), None),
            ],
        }
    }

    fn config() -> Config {
        let mut c = Config::new("test-host", BackendKind::Fake);
        c.monitors.push(monitor_cfg("SN-L", "port-a"));
        c.monitors.push(monitor_cfg("SN-R", "port-b"));
        c
    }

    fn backend(left: u8, right: u8) -> FakeBackend {
        FakeBackend::new(vec![
            FakeMonitor::new("port-a", Some("SN-L"), left),
            FakeMonitor::new("port-b", Some("SN-R"), right),
            FakeMonitor::internal_panel("panel"),
        ])
    }

    fn fast() -> ExecOptions {
        ExecOptions {
            settle: Duration::ZERO,
            ..Default::default()
        }
    }

    fn set(cfg: &Config, be: &FakeBackend, monitor: &str, code: u8) -> StepReport {
        let detected = be.discover().unwrap();
        let plan = plan_set_input(cfg, &detected, monitor, InputCode(code)).unwrap();
        apply_one(&plan, be, fast(), &EventLog::disabled())
    }

    #[test]
    fn sets_the_named_monitor_and_only_that_one() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        let report = set(&cfg, &be, "SN-L", DP);

        assert_eq!(report.outcome, WriteOutcome::ConfirmedByRead(InputCode(DP)));
        assert_eq!(be.writes(), vec![("port-a".to_string(), InputCode(DP))]);
        assert_eq!(be.current_input("port-b"), Some(InputCode(HDMI)));
    }

    #[test]
    fn a_monitor_can_be_named_by_key_or_label() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        let detected = be.discover().unwrap();
        assert!(plan_set_input(&cfg, &detected, "SN-L", InputCode(DP)).is_ok());
        assert!(plan_set_input(&cfg, &detected, "AOC 27P2DG5 (SN-L)", InputCode(DP)).is_ok());
    }

    #[test]
    fn setting_the_input_it_is_already_on_writes_nothing() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        let report = set(&cfg, &be, "SN-L", HDMI);
        assert!(be.writes().is_empty(), "no write should be issued");
        assert!(report.skipped_already_correct);
    }

    #[test]
    fn repeating_a_set_is_idempotent() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        set(&cfg, &be, "SN-L", DP);
        let first = be.writes().len();
        set(&cfg, &be, "SN-L", DP);
        assert_eq!(be.writes().len(), first, "second run must be a no-op");
        assert_eq!(be.current_input("port-a"), Some(InputCode(DP)));
    }

    #[test]
    fn losing_contact_after_the_write_is_not_a_failure() {
        let cfg = config();
        let be = FakeBackend::new(vec![FakeMonitor::new("port-a", Some("SN-L"), HDMI)
            .with_behavior(FakeBehavior::SilentAfterWrite)]);
        let report = set(&cfg, &be, "SN-L", DP);
        assert_eq!(report.outcome, WriteOutcome::Accepted);
        assert!(report.outcome.to_string().contains("unconfirmed"));
    }

    #[test]
    fn a_write_that_is_silently_ignored_is_not_called_success() {
        let cfg = config();
        let be = FakeBackend::new(vec![FakeMonitor::new("port-a", Some("SN-L"), HDMI)
            .with_behavior(FakeBehavior::IgnoresWrites)]);
        let report = set(&cfg, &be, "SN-L", DP);
        assert!(matches!(report.outcome, WriteOutcome::Unknown { .. }));
    }

    #[test]
    fn a_failing_write_is_reported_as_failed() {
        let cfg = config();
        let be = FakeBackend::new(vec![FakeMonitor::new("port-a", Some("SN-L"), HDMI)
            .with_behavior(FakeBehavior::WriteFails("i2c write failed".into()))]);
        let report = set(&cfg, &be, "SN-L", DP);
        assert!(report.outcome.is_failure());
    }

    #[test]
    fn one_monitor_failing_leaves_the_others_done() {
        let cfg = config();
        let be = FakeBackend::new(vec![
            FakeMonitor::new("port-a", Some("SN-L"), HDMI),
            FakeMonitor::new("port-b", Some("SN-R"), HDMI)
                .with_behavior(FakeBehavior::WriteFails("nope".into())),
        ]);
        let detected = be.discover().unwrap();
        let plans = vec![
            plan_set_input(&cfg, &detected, "SN-L", InputCode(DP)).unwrap(),
            plan_set_input(&cfg, &detected, "SN-R", InputCode(DP)).unwrap(),
        ];
        let report = apply_many(&plans, &be, fast(), &EventLog::disabled());

        assert_eq!(report.status(), ApplyStatus::Partial);
        assert_eq!(report.exit_code(), 1);
        // The one that worked stays switched: no rollback is attempted.
        assert_eq!(be.current_input("port-a"), Some(InputCode(DP)));
    }

    #[test]
    fn a_disconnected_monitor_is_refused_before_any_write() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        be.detach("port-b");
        let detected = be.discover().unwrap();
        let err = plan_set_input(&cfg, &detected, "SN-R", InputCode(DP)).unwrap_err();
        assert!(matches!(err, PlanError::Binding { .. }));
        assert!(be.writes().is_empty(), "nothing may be written on refusal");
    }

    #[test]
    fn an_unknown_monitor_lists_the_configured_ones() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        let detected = be.discover().unwrap();
        let err = plan_set_input(&cfg, &detected, "nope", InputCode(DP)).unwrap_err();
        assert!(err.to_string().contains("SN-L"), "{err}");
    }

    #[test]
    fn an_unadvertised_code_is_flagged_but_not_refused() {
        // Capability strings are wrong in both directions, so this is the
        // user's call to make, not the tool's.
        let cfg = config();
        let be = backend(HDMI, HDMI);
        let detected = be.discover().unwrap();
        let plan = plan_set_input(&cfg, &detected, "SN-L", InputCode(0x03)).unwrap();
        assert!(!plan.advertised);
    }

    #[test]
    fn dry_run_touches_nothing() {
        let cfg = config();
        let be = backend(HDMI, HDMI);
        let detected = be.discover().unwrap();
        let plan = plan_set_input(&cfg, &detected, "SN-L", InputCode(DP)).unwrap();
        let opts = ExecOptions {
            settle: Duration::ZERO,
            dry_run: true,
            ..Default::default()
        };
        apply_one(&plan, &be, opts, &EventLog::disabled());
        assert!(be.writes().is_empty());
    }

    #[test]
    fn nothing_in_this_module_needs_to_know_what_is_plugged_in() {
        // The planner takes a monitor and a code. If a computer name ever
        // becomes necessary here, the design has regressed.
        let cfg = config();
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(!text.contains("destination"), "{text}");
    }

    // -- guard and last-request --------------------------------------------

    #[test]
    fn the_guard_serialises_concurrent_changes() {
        let dir = std::env::temp_dir().join(format!("ds-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = SwitchGuard::acquire(&dir, Duration::from_secs(60)).unwrap();
        assert!(matches!(
            SwitchGuard::acquire(&dir, Duration::from_secs(60)),
            Err(GuardError::Busy { .. })
        ));
        drop(first);
        assert!(SwitchGuard::acquire(&dir, Duration::from_secs(60)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_lock_does_not_wedge_the_tool() {
        let dir = std::env::temp_dir().join(format!("ds-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let leaked = SwitchGuard::acquire(&dir, Duration::from_secs(60)).unwrap();
        std::mem::forget(leaked); // simulate a killed process
        assert!(SwitchGuard::acquire(&dir, Duration::ZERO).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_request_survives_a_round_trip_to_disk() {
        let dir = std::env::temp_dir().join(format!("ds-last-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(LastRequest::record(&dir, "Desk setup"));
        let loaded = LastRequest::load(&dir);
        assert_eq!(loaded.request.as_deref(), Some("Desk setup"));
        assert!(loaded.is_duplicate_of("Desk setup", Duration::from_secs(60)));
        assert!(!loaded.is_duplicate_of("Something else", Duration::from_secs(60)));
        assert!(!loaded.is_duplicate_of("Desk setup", Duration::ZERO));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
