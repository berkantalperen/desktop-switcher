//! The switch transaction: validate, then write, then report honestly.
//!
//! Two rules drive the shape of this module.
//!
//! 1. A switch is a *set-target* operation. Every write is an absolute input
//!    code for a named destination, so issuing it twice is harmless. Nothing
//!    here ever increments or cycles VCP 0x60.
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
    #[error("unknown destination `{requested}`. Configured destinations: {}", known.join(", "))]
    UnknownDestination {
        requested: String,
        known: Vec<String>,
    },
    #[error("no monitors are configured. Run `desktop-switcher configure` first.")]
    NoMonitorsConfigured,
    #[error("cannot identify every configured monitor:\n{}",
            problems.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n"))]
    Binding { problems: Vec<BindingProblem> },
    #[error("`{logical_id}` has no input code recorded for `{destination}`. Run `desktop-switcher configure`, or test one monitor with `desktop-switcher inspect`.")]
    MissingMapping {
        logical_id: String,
        destination: String,
    },
    #[error("`{logical_id}` -> `{destination}` is only {evidence}, not user-confirmed. Verify it with a human watching before switching unattended.")]
    UntrustedMapping {
        logical_id: String,
        destination: String,
        evidence: String,
    },
}

/// One write this switch intends to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWrite {
    pub logical_id: String,
    pub name: String,
    pub handle: MonitorHandle,
    pub code: InputCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchPlan {
    pub destination: String,
    /// True when the destination is another computer, so losing DDC contact
    /// after the write is the expected outcome rather than a fault.
    pub switching_away: bool,
    pub writes: Vec<PlannedWrite>,
    pub notes: Vec<String>,
}

/// Validate a requested destination into a concrete, fully-identified plan.
///
/// Every refusal here is a refusal to write. Nothing is attempted partially
/// because one monitor checked out.
pub fn plan(
    config: &Config,
    detected: &[DetectedMonitor],
    destination: &str,
) -> Result<SwitchPlan, PlanError> {
    if config.monitors.is_empty() {
        return Err(PlanError::NoMonitorsConfigured);
    }
    let known = config.destinations();
    if !known.iter().any(|d| d == destination) {
        return Err(PlanError::UnknownDestination {
            requested: destination.to_string(),
            known,
        });
    }

    let bindings = bind(config, detected);
    if !bindings.is_complete() {
        return Err(PlanError::Binding {
            problems: bindings.problems,
        });
    }

    let mut notes: Vec<String> = bindings
        .bound
        .iter()
        .flat_map(|b| b.notes.clone())
        .collect();
    for extra in &bindings.unclaimed {
        notes.push(format!(
            "{} is attached but not configured; it will not be switched.",
            extra.identity.describe()
        ));
    }

    let mut writes = Vec::new();
    for cfg in config.ordered_monitors() {
        let binding = bindings
            .get(&cfg.logical_id)
            .expect("every configured monitor is bound at this point");

        let Some(mapping) = cfg.mapping(destination) else {
            return Err(PlanError::MissingMapping {
                logical_id: cfg.logical_id.clone(),
                destination: destination.to_string(),
            });
        };
        if !mapping.verification.is_trusted_for_switching() {
            return Err(PlanError::UntrustedMapping {
                logical_id: cfg.logical_id.clone(),
                destination: destination.to_string(),
                evidence: mapping.verification.to_string(),
            });
        }
        writes.push(PlannedWrite {
            logical_id: cfg.logical_id.clone(),
            name: cfg.name.clone(),
            handle: binding.handle(),
            code: mapping.input_code,
        });
    }

    Ok(SwitchPlan {
        destination: destination.to_string(),
        switching_away: destination != config.self_destination,
        writes,
        notes,
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
    pub logical_id: String,
    pub name: String,
    pub requested: InputCode,
    pub before: Option<InputReading>,
    pub outcome: WriteOutcome,
    pub after: Option<InputReading>,
    /// Set when the write was skipped because the monitor was already there.
    pub skipped_already_correct: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchStatus {
    /// Every monitor read back as requested.
    AllConfirmed,
    /// Every write was accepted, but at least one could not be verified.
    AllIssuedSomeUnconfirmed,
    /// Some monitors failed and some did not.
    Partial,
    AllFailed,
}

impl fmt::Display for SwitchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SwitchStatus::AllConfirmed => "all monitors confirmed",
            SwitchStatus::AllIssuedSomeUnconfirmed => {
                "all writes issued; some could not be confirmed"
            }
            SwitchStatus::Partial => "PARTIAL: some monitors failed",
            SwitchStatus::AllFailed => "FAILED: no monitor was switched",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchReport {
    pub destination: String,
    pub steps: Vec<StepReport>,
    pub dry_run: bool,
}

impl SwitchReport {
    pub fn status(&self) -> SwitchStatus {
        let failed = self.steps.iter().filter(|s| s.outcome.is_failure()).count();
        if failed == self.steps.len() && !self.steps.is_empty() {
            return SwitchStatus::AllFailed;
        }
        if failed > 0 {
            return SwitchStatus::Partial;
        }
        if self
            .steps
            .iter()
            .all(|s| matches!(s.outcome, WriteOutcome::ConfirmedByRead(_)))
        {
            SwitchStatus::AllConfirmed
        } else {
            SwitchStatus::AllIssuedSomeUnconfirmed
        }
    }

    /// Exit code: 0 confirmed or issued, 1 partial, 2 total failure.
    pub fn exit_code(&self) -> i32 {
        match self.status() {
            SwitchStatus::AllConfirmed | SwitchStatus::AllIssuedSomeUnconfirmed => 0,
            SwitchStatus::Partial => 1,
            SwitchStatus::AllFailed => 2,
        }
    }
}

/// Execute a validated plan.
///
/// Monitors are handled one at a time and independently: a failure on the
/// first does not abort the second, because leaving one screen on each
/// computer is worse than finishing the job. No rollback is attempted — with
/// connectivity unknown, "undoing" a write is just another blind write.
pub fn execute(
    plan: &SwitchPlan,
    backend: &dyn MonitorBackend,
    opts: ExecOptions,
    log: &EventLog,
) -> SwitchReport {
    log.append(
        "switch.begin",
        &[
            ("destination", plan.destination.clone()),
            ("monitors", plan.writes.len().to_string()),
            ("dry_run", opts.dry_run.to_string()),
        ],
    );

    let mut steps = Vec::new();
    for write in &plan.writes {
        let before = read_best_effort(backend, &write.handle);

        if opts.dry_run {
            steps.push(StepReport {
                logical_id: write.logical_id.clone(),
                name: write.name.clone(),
                requested: write.code,
                before,
                outcome: WriteOutcome::Unknown {
                    detail: "dry run: no write issued".into(),
                },
                after: None,
                skipped_already_correct: false,
            });
            continue;
        }

        // Already on the requested input: the set is a no-op, so skip the
        // write entirely. This is what makes pressing the same shortcut twice
        // safe rather than a cycle.
        if before.as_ref().and_then(|r| r.value()) == Some(write.code) {
            log.append(
                "switch.skip",
                &[
                    ("monitor", write.logical_id.clone()),
                    ("code", write.code.to_string()),
                    ("reason", "already-on-requested-input".into()),
                ],
            );
            steps.push(StepReport {
                logical_id: write.logical_id.clone(),
                name: write.name.clone(),
                requested: write.code,
                before,
                outcome: WriteOutcome::ConfirmedByRead(write.code),
                after: None,
                skipped_already_correct: true,
            });
            continue;
        }

        let outcome = match backend.set_input(&write.handle, write.code) {
            Ok(o) => o,
            Err(e) => WriteOutcome::Failed {
                detail: e.to_string(),
            },
        };
        log.append(
            "switch.write",
            &[
                ("monitor", write.logical_id.clone()),
                ("id", write.handle.backend_id.clone()),
                ("code", write.code.to_string()),
                ("outcome", outcome.to_string()),
            ],
        );

        // A write is never retried. It may already have been accepted, and
        // repeating it while the link re-enumerates compounds the disconnect.
        let mut after = None;
        let mut final_outcome = outcome;
        if opts.read_back && !final_outcome.is_failure() {
            if !opts.settle.is_zero() {
                std::thread::sleep(opts.settle);
            }
            let reading = read_after_write(backend, &write.handle, plan.switching_away);
            final_outcome = match &reading {
                InputReading::Value(v) if *v == write.code => WriteOutcome::ConfirmedByRead(*v),
                InputReading::Value(v) => WriteOutcome::Unknown {
                    detail: format!("read back {v}, expected {}", write.code),
                },
                // Losing contact after switching a monitor to the other
                // computer is the normal case, not a failure.
                InputReading::UnavailableAfterSwitch { .. } => WriteOutcome::Accepted,
                InputReading::ReadFailed { detail } => WriteOutcome::Unknown {
                    detail: format!("write accepted, read-back failed: {detail}"),
                },
                InputReading::Unsupported => WriteOutcome::Accepted,
            };
            after = Some(reading);
        }

        steps.push(StepReport {
            logical_id: write.logical_id.clone(),
            name: write.name.clone(),
            requested: write.code,
            before,
            outcome: final_outcome,
            after,
            skipped_already_correct: false,
        });
    }

    let report = SwitchReport {
        destination: plan.destination.clone(),
        steps,
        dry_run: opts.dry_run,
    };
    log.append(
        "switch.end",
        &[
            ("destination", report.destination.clone()),
            ("status", report.status().to_string()),
        ],
    );
    report
}

fn read_best_effort(backend: &dyn MonitorBackend, handle: &MonitorHandle) -> Option<InputReading> {
    match backend.read_input(handle) {
        Ok(r) => Some(r),
        Err(e) => Some(InputReading::ReadFailed {
            detail: e.to_string(),
        }),
    }
}

fn read_after_write(
    backend: &dyn MonitorBackend,
    handle: &MonitorHandle,
    switching_away: bool,
) -> InputReading {
    match backend.read_input(handle) {
        Ok(InputReading::ReadFailed { detail }) if switching_away => {
            InputReading::UnavailableAfterSwitch { detail }
        }
        Ok(r) => r,
        Err(e) if switching_away => InputReading::UnavailableAfterSwitch {
            detail: e.to_string(),
        },
        Err(e) => InputReading::ReadFailed {
            detail: e.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Serialising concurrent switches
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("another switch is already running (lock held at {path})")]
    Busy { path: String },
    #[error("could not create lock at {path}: {detail}")]
    Io { path: String, detail: String },
}

/// A cross-process lock so two shortcuts, or a shortcut and a tray icon,
/// cannot drive the same DDC interfaces at once.
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
// Last *requested* destination, tracked separately from observed state
// ---------------------------------------------------------------------------

/// What this tool last asked for.
///
/// Deliberately not called "current state": the user can change inputs with
/// the monitor's own buttons at any time, and the other computer can issue its
/// own switch. This is intent, and it is reported as intent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRequest {
    pub destination: Option<String>,
    /// Milliseconds since the Unix epoch.
    ///
    /// `u64`, not `u128`: TOML integers are 64-bit, and a `u128` here fails to
    /// serialise, which silently left this file empty and defeated the
    /// duplicate-press guard entirely.
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

    /// Persist the request. Returns whether it was actually written, so a
    /// silent failure cannot quietly disable the duplicate-press guard.
    pub fn record(dir: &Path, destination: &str) -> bool {
        let value = Self {
            destination: Some(destination.to_string()),
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

    /// Whether this request is a duplicate of one just made, e.g. a shortcut
    /// key repeating because it was held down.
    pub fn is_duplicate_of(&self, destination: &str, window: Duration) -> bool {
        let (Some(last), Some(at)) = (self.destination.as_deref(), self.at_unix_ms) else {
            return false;
        };
        if last != destination {
            return false;
        }
        // Strictly less-than, so a zero window disables deduplication instead
        // of swallowing every request that lands in the same millisecond.
        u128::from(now_unix_ms().saturating_sub(at)) < window.as_millis()
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Decide the other named destination, for `toggle`.
///
/// Returns `Err` with an explanation whenever the live state is not
/// trustworthy enough to infer intent — mixed inputs, unreadable monitors, or
/// a current input that matches no configured destination.
pub fn infer_toggle_target(
    config: &Config,
    observed: &[(String, InputReading)],
) -> Result<String, String> {
    let destinations = config.destinations();
    if destinations.len() != 2 {
        return Err(format!(
            "toggle needs exactly two configured destinations, found {}",
            destinations.len()
        ));
    }

    let mut current: Option<String> = None;
    for (logical_id, reading) in observed {
        let InputReading::Value(code) = reading else {
            return Err(format!(
                "`{logical_id}` could not be read ({reading}), so the current computer is unknown. \
                 Use `switch <destination>` explicitly."
            ));
        };
        let cfg = config
            .monitor(logical_id)
            .ok_or_else(|| format!("`{logical_id}` is not configured"))?;
        let matched: Vec<&String> = cfg
            .destinations
            .iter()
            .filter(|(_, m)| m.input_code == *code)
            .map(|(name, _)| name)
            .collect();
        let [one] = matched.as_slice() else {
            return Err(format!(
                "`{logical_id}` is on {code}, which matches no single configured destination. \
                 Use `switch <destination>` explicitly."
            ));
        };
        match &current {
            None => current = Some((*one).clone()),
            Some(prev) if prev == *one => {}
            Some(prev) => {
                return Err(format!(
                    "monitors disagree: one is on `{prev}`, `{logical_id}` is on `{one}`. \
                     Use `switch <destination>` explicitly."
                ))
            }
        }
    }

    let current = current.ok_or_else(|| "no monitors were read".to_string())?;
    destinations
        .into_iter()
        .find(|d| *d != current)
        .ok_or_else(|| "could not determine the other destination".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendKind, DestinationMapping, MonitorConfig};
    use crate::fake::{FakeBackend, FakeBehavior, FakeMonitor};
    use crate::types::Evidence;
    use std::collections::BTreeMap;

    const WIN: u8 = 0x11;
    const UBU: u8 = 0x0F;

    fn monitor_cfg(id: &str, backend_id: &str, serial: &str, win: u8, ubu: u8) -> MonitorConfig {
        let mut destinations = BTreeMap::new();
        for (name, code) in [("windows", win), ("ubuntu", ubu)] {
            destinations.insert(
                name.to_string(),
                DestinationMapping {
                    input_code: InputCode(code),
                    verification: Evidence::UserConfirmed,
                    verified_on: Some("2026-09-16".into()),
                    note: None,
                },
            );
        }
        MonitorConfig {
            logical_id: id.into(),
            name: format!("AOC 27P2DG5 - {id}"),
            backend_id: backend_id.into(),
            serial: Some(serial.into()),
            model: Some("27P2DG5".into()),
            destinations,
        }
    }

    /// Mirrors the real machine: the two panels use *different* codes for the
    /// same computer, so a single batched write would be wrong.
    fn config() -> Config {
        let mut c = Config::new("alienware-windows", BackendKind::Fake, "windows");
        c.monitors = vec![
            monitor_cfg("left", "port-a", "SN-L", WIN, UBU),
            monitor_cfg("right", "port-b", "SN-R", UBU, WIN),
        ];
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

    fn run(cfg: &Config, be: &FakeBackend, dest: &str) -> SwitchReport {
        let detected = be.discover().unwrap();
        let plan = plan(cfg, &detected, dest).unwrap();
        execute(&plan, be, fast(), &EventLog::disabled())
    }

    #[test]
    fn switching_writes_each_monitors_own_code() {
        let cfg = config();
        let be = backend(WIN, UBU);
        let report = run(&cfg, &be, "ubuntu");
        assert_eq!(report.status(), SwitchStatus::AllConfirmed);
        // left -> 0x0F, right -> 0x11: different values, same destination.
        assert_eq!(
            be.writes(),
            vec![
                ("port-a".to_string(), InputCode(UBU)),
                ("port-b".to_string(), InputCode(WIN)),
            ]
        );
    }

    #[test]
    fn requesting_the_current_destination_writes_nothing_and_does_not_cycle() {
        let cfg = config();
        // Already showing Windows: left on 0x11, right on 0x0F.
        let be = backend(WIN, UBU);
        let report = run(&cfg, &be, "windows");
        assert!(be.writes().is_empty(), "no write should be issued");
        assert!(report.steps.iter().all(|s| s.skipped_already_correct));
        assert_eq!(report.status(), SwitchStatus::AllConfirmed);
    }

    #[test]
    fn repeating_a_switch_is_idempotent() {
        let cfg = config();
        let be = backend(WIN, UBU);
        run(&cfg, &be, "ubuntu");
        let first = be.writes().len();
        run(&cfg, &be, "ubuntu");
        assert_eq!(be.writes().len(), first, "second run must be a no-op");
        assert_eq!(be.current_input("port-a"), Some(InputCode(UBU)));
    }

    #[test]
    fn mixed_inputs_still_converge_on_the_requested_destination() {
        let cfg = config();
        // left already on ubuntu, right still on windows.
        let be = backend(UBU, UBU);
        let report = run(&cfg, &be, "ubuntu");
        assert_eq!(report.status(), SwitchStatus::AllConfirmed);
        assert_eq!(be.current_input("port-a"), Some(InputCode(UBU)));
        assert_eq!(be.current_input("port-b"), Some(InputCode(WIN)));
    }

    #[test]
    fn losing_contact_after_switching_away_is_not_reported_as_failure() {
        let cfg = config();
        let be = FakeBackend::new(vec![
            FakeMonitor::new("port-a", Some("SN-L"), WIN)
                .with_behavior(FakeBehavior::SilentAfterWrite),
            FakeMonitor::new("port-b", Some("SN-R"), UBU)
                .with_behavior(FakeBehavior::SilentAfterWrite),
        ]);
        let report = run(&cfg, &be, "ubuntu");
        assert_eq!(report.status(), SwitchStatus::AllIssuedSomeUnconfirmed);
        for step in &report.steps {
            assert_eq!(step.outcome, WriteOutcome::Accepted);
            assert!(matches!(
                step.after,
                Some(InputReading::UnavailableAfterSwitch { .. })
            ));
            // The honest phrasing, not a claim of success.
            assert!(step.outcome.to_string().contains("unconfirmed"));
        }
    }

    #[test]
    fn one_monitor_failing_is_reported_as_partial_not_all_or_nothing() {
        let cfg = config();
        let be = FakeBackend::new(vec![
            FakeMonitor::new("port-a", Some("SN-L"), WIN),
            FakeMonitor::new("port-b", Some("SN-R"), UBU)
                .with_behavior(FakeBehavior::WriteFails("i2c write failed".into())),
        ]);
        let report = run(&cfg, &be, "ubuntu");
        assert_eq!(report.status(), SwitchStatus::Partial);
        assert_eq!(report.exit_code(), 1);
        assert!(!report.steps[0].outcome.is_failure());
        assert!(report.steps[1].outcome.is_failure());
        // The monitor that worked stays switched: no rollback is attempted.
        assert_eq!(be.current_input("port-a"), Some(InputCode(UBU)));
    }

    #[test]
    fn a_write_that_is_silently_ignored_is_not_called_success() {
        let cfg = config();
        let be = FakeBackend::new(vec![
            FakeMonitor::new("port-a", Some("SN-L"), WIN)
                .with_behavior(FakeBehavior::IgnoresWrites),
            FakeMonitor::new("port-b", Some("SN-R"), UBU),
        ]);
        let report = run(&cfg, &be, "ubuntu");
        assert!(matches!(
            report.steps[0].outcome,
            WriteOutcome::Unknown { .. }
        ));
        assert_ne!(report.status(), SwitchStatus::AllConfirmed);
    }

    #[test]
    fn a_disconnected_monitor_blocks_the_plan_before_any_write() {
        let cfg = config();
        let be = backend(WIN, UBU);
        be.detach("port-b");
        let detected = be.discover().unwrap();
        let err = plan(&cfg, &detected, "ubuntu").unwrap_err();
        assert!(matches!(err, PlanError::Binding { .. }));
        assert!(be.writes().is_empty(), "nothing may be written on refusal");
    }

    #[test]
    fn an_unverified_mapping_refuses_to_switch() {
        let mut cfg = config();
        cfg.monitor_mut("right")
            .unwrap()
            .destinations
            .get_mut("ubuntu")
            .unwrap()
            .verification = Evidence::Reported;
        let be = backend(WIN, UBU);
        let detected = be.discover().unwrap();
        let err = plan(&cfg, &detected, "ubuntu").unwrap_err();
        assert!(matches!(err, PlanError::UntrustedMapping { .. }));
        assert!(err.to_string().contains("user-confirmed") || err.to_string().contains("reported"));
        assert!(be.writes().is_empty());
    }

    #[test]
    fn a_missing_mapping_refuses_to_switch() {
        let mut cfg = config();
        cfg.monitor_mut("left")
            .unwrap()
            .destinations
            .remove("ubuntu");
        let be = backend(WIN, UBU);
        let detected = be.discover().unwrap();
        assert!(matches!(
            plan(&cfg, &detected, "ubuntu").unwrap_err(),
            PlanError::MissingMapping { .. }
        ));
        assert!(be.writes().is_empty());
    }

    #[test]
    fn unknown_destination_lists_the_real_ones() {
        let cfg = config();
        let be = backend(WIN, UBU);
        let detected = be.discover().unwrap();
        let err = plan(&cfg, &detected, "macos").unwrap_err();
        assert!(err.to_string().contains("windows"), "{err}");
    }

    #[test]
    fn dry_run_touches_nothing() {
        let cfg = config();
        let be = backend(WIN, UBU);
        let detected = be.discover().unwrap();
        let p = plan(&cfg, &detected, "ubuntu").unwrap();
        let opts = ExecOptions {
            settle: Duration::ZERO,
            dry_run: true,
            ..Default::default()
        };
        execute(&p, &be, opts, &EventLog::disabled());
        assert!(be.writes().is_empty());
    }

    #[test]
    fn switch_order_is_honoured() {
        let mut cfg = config();
        cfg.switch_order = Some(vec!["right".into(), "left".into()]);
        let be = backend(WIN, UBU);
        run(&cfg, &be, "ubuntu");
        assert_eq!(be.writes()[0].0, "port-b");
        assert_eq!(be.writes()[1].0, "port-a");
    }

    #[test]
    fn an_unconfigured_extra_display_is_noted_but_not_switched() {
        let cfg = config();
        let be = FakeBackend::new(vec![
            FakeMonitor::new("port-a", Some("SN-L"), WIN),
            FakeMonitor::new("port-b", Some("SN-R"), UBU),
            FakeMonitor::new("port-x", Some("SN-X"), WIN),
        ]);
        let detected = be.discover().unwrap();
        let p = plan(&cfg, &detected, "ubuntu").unwrap();
        assert_eq!(p.writes.len(), 2);
        assert!(p.notes.iter().any(|n| n.contains("not configured")));
        execute(&p, &be, fast(), &EventLog::disabled());
        assert!(be.writes().iter().all(|(id, _)| id != "port-x"));
    }

    // -- toggle -------------------------------------------------------------

    #[test]
    fn toggle_picks_the_other_computer_when_state_is_clear() {
        let cfg = config();
        let observed = vec![
            ("left".to_string(), InputReading::Value(InputCode(WIN))),
            ("right".to_string(), InputReading::Value(InputCode(UBU))),
        ];
        assert_eq!(infer_toggle_target(&cfg, &observed).unwrap(), "ubuntu");
    }

    #[test]
    fn toggle_refuses_when_monitors_disagree() {
        let cfg = config();
        let observed = vec![
            ("left".to_string(), InputReading::Value(InputCode(WIN))),
            ("right".to_string(), InputReading::Value(InputCode(WIN))),
        ];
        let err = infer_toggle_target(&cfg, &observed).unwrap_err();
        assert!(err.contains("disagree"), "{err}");
    }

    #[test]
    fn toggle_refuses_when_a_monitor_cannot_be_read() {
        let cfg = config();
        let observed = vec![
            ("left".to_string(), InputReading::Value(InputCode(WIN))),
            (
                "right".to_string(),
                InputReading::ReadFailed {
                    detail: "no response".into(),
                },
            ),
        ];
        assert!(infer_toggle_target(&cfg, &observed).is_err());
    }

    #[test]
    fn toggle_refuses_after_a_manual_osd_change_to_an_unmapped_input() {
        let cfg = config();
        // Someone pressed the monitor buttons and chose VGA.
        let observed = vec![
            ("left".to_string(), InputReading::Value(InputCode(0x01))),
            ("right".to_string(), InputReading::Value(InputCode(UBU))),
        ];
        let err = infer_toggle_target(&cfg, &observed).unwrap_err();
        assert!(
            err.contains("matches no single configured destination"),
            "{err}"
        );
    }

    // -- guard and last-request --------------------------------------------

    #[test]
    fn the_guard_serialises_concurrent_switches() {
        let dir = std::env::temp_dir().join(format!("ds-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = SwitchGuard::acquire(&dir, Duration::from_secs(60)).unwrap();
        assert!(matches!(
            SwitchGuard::acquire(&dir, Duration::from_secs(60)),
            Err(GuardError::Busy { .. })
        ));
        drop(first);
        // Released on drop, so the next invocation proceeds.
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
        // Regression: this was serialised as u128, which TOML cannot hold, so
        // the file was never written and every repeat press got through.
        let dir = std::env::temp_dir().join(format!("ds-last-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            LastRequest::record(&dir, "ubuntu"),
            "recording the last request must actually write the file"
        );
        let loaded = LastRequest::load(&dir);
        assert_eq!(loaded.destination.as_deref(), Some("ubuntu"));
        assert!(loaded.at_unix_ms.is_some());
        assert!(loaded.is_duplicate_of("ubuntu", Duration::from_secs(60)));
        assert!(!loaded.is_duplicate_of("windows", Duration::from_secs(60)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_shortcut_presses_are_deduplicated() {
        let last = LastRequest {
            destination: Some("ubuntu".into()),
            at_unix_ms: Some(now_unix_ms()),
        };
        assert!(last.is_duplicate_of("ubuntu", Duration::from_millis(1500)));
        assert!(!last.is_duplicate_of("windows", Duration::from_millis(1500)));
        assert!(!last.is_duplicate_of("ubuntu", Duration::ZERO));
        assert!(!LastRequest::default().is_duplicate_of("ubuntu", Duration::from_secs(5)));
    }
}
