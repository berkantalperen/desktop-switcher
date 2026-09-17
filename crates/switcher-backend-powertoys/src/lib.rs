//! Windows backend: a process adapter over `PowerToys.PowerDisplay.Cli.exe`.
//!
//! Monitors are always addressed by `--monitor-id`, never by `--monitor-number`.
//! Numbers are reassigned when displays sleep, when the dock re-enumerates, or
//! when a cable is replugged, and an off-by-one there means writing an input
//! code to the wrong screen.
//!
//! Identity is corroborated with the EDID serial read from the registry,
//! because the PowerToys id is derived from the DevicePath and therefore
//! follows the GPU output port rather than the panel plugged into it.

pub mod parse;

#[cfg(windows)]
mod edid_source;

use std::path::{Path, PathBuf};
use std::time::Duration;

use switcher_core::backend::{BackendError, BackendHealth, HealthFinding, MonitorBackend};
use switcher_core::proc::{self, CommandRun};
use switcher_core::types::{
    DetectedMonitor, InputCapabilities, InputCode, InputReading, InputSourceOption, MonitorHandle,
    MonitorIdentity, Transport, WriteOutcome,
};

/// The CLI version this adapter was developed and captured fixtures against.
pub const TESTED_VERSION: &str = "0.101.2362.0";

/// CLI versions this adapter has actually been exercised against.
pub const TESTED_VERSIONS: &[&str] = &[TESTED_VERSION];

pub const EXE_NAME: &str = "PowerToys.PowerDisplay.Cli";

/// How many times to attempt a write the monitor has explicitly rejected.
///
/// These panels intermittently answer a perfectly good `setvcp` with
/// "hardware write failed"; the same command a few seconds later succeeds.
/// Three attempts turns that from a visible failure into a slightly slow
/// switch.
const WRITE_ATTEMPTS: usize = 3;
const WRITE_RETRY_DELAY: Duration = Duration::from_millis(400);

/// Whether the tool is telling us the write never reached the monitor.
///
/// Only these failures are safe to repeat. Anything vaguer could mean the
/// write was accepted and the reply was lost, and repeating an input switch
/// in that state is how a monitor ends up somewhere nobody asked for.
fn is_definite_write_failure(run: &CommandRun) -> bool {
    let text = format!("{}\n{}", run.stdout, run.stderr).to_lowercase();
    text.contains("hardware write failed") || text.contains("failed to set vcp")
}

const NOT_RUNNING_HINT: &str =
    "Start PowerToys and enable the Power Display module, then try again.";

pub struct PowerDisplayBackend {
    exe: PathBuf,
    timeout: Duration,
}

impl PowerDisplayBackend {
    pub fn new(exe: PathBuf, timeout: Duration) -> Self {
        Self { exe, timeout }
    }

    /// Where PowerToys installs the CLI, in preference order.
    pub fn candidate_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for var in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                dirs.push(base.join("PowerToys").join("WinUI3Apps"));
                dirs.push(base.join("PowerToys"));
            }
        }
        dirs
    }

    /// Resolve the executable, preferring an explicit configured path.
    ///
    /// The path is never assumed to be on `PATH`: PowerToys does not put it
    /// there, and a shortcut runs with a different environment than a shell.
    pub fn locate(explicit: Option<&Path>) -> Result<PathBuf, BackendError> {
        if let Some(p) = explicit {
            return if p.is_file() {
                Ok(p.to_path_buf())
            } else {
                Err(BackendError::ToolNotFound {
                    tool: p.display().to_string(),
                    hint: "The `tool_path` in config.toml does not point at a file.".into(),
                })
            };
        }
        proc::find_executable(EXE_NAME, &Self::candidate_dirs()).ok_or_else(|| {
            BackendError::ToolNotFound {
                tool: EXE_NAME.to_string(),
                hint: format!(
                    "Looked in PATH and {}. Install PowerToys, or set `tool_path` in config.toml.",
                    Self::candidate_dirs()
                        .iter()
                        .map(|d| d.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        })
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    fn run(&self, args: &[&str]) -> Result<CommandRun, BackendError> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Ok(proc::run(&self.exe, &owned, self.timeout)?)
    }

    /// Run and require a zero exit status, classifying failures usefully.
    fn run_ok(&self, args: &[&str]) -> Result<CommandRun, BackendError> {
        let run = self.run(args)?;
        if run.success() {
            return Ok(run);
        }
        Err(classify_failure(&run))
    }

    pub fn version(&self) -> Result<String, BackendError> {
        Ok(self.run_ok(&["--version"])?.stdout.trim().to_string())
    }
}

fn classify_failure(run: &CommandRun) -> BackendError {
    let stderr = run.stderr.trim().to_string();
    let lower = stderr.to_lowercase();
    if lower.contains("no monitor found") {
        return BackendError::MonitorNotFound { detail: stderr };
    }
    if lower.contains("not running") || lower.contains("power display") && lower.contains("enable")
    {
        return BackendError::ToolNotReady {
            tool: "PowerToys Power Display".into(),
            detail: format!("{stderr}. {NOT_RUNNING_HINT}"),
        };
    }
    BackendError::CommandFailed {
        command: run.display_command(),
        status: run.status,
        stderr,
    }
}

fn transport_from_method(method: &str) -> Transport {
    match method {
        "DDC/CI" => Transport::DdcCi,
        "WMI" => Transport::Wmi,
        other => Transport::Other(other.to_string()),
    }
}

impl MonitorBackend for PowerDisplayBackend {
    fn name(&self) -> &str {
        "powertoys-cli"
    }

    fn health(&self) -> BackendHealth {
        let mut findings = Vec::new();
        let mut version = None;

        match self.version() {
            Ok(v) => {
                if TESTED_VERSIONS.contains(&v.as_str()) {
                    findings.push(HealthFinding::info(format!(
                        "PowerToys Power Display CLI {v} (a version this build was tested against)"
                    )));
                } else {
                    findings.push(HealthFinding::warn(
                        format!(
                            "PowerToys Power Display CLI is {v}; this build was tested against {}.",
                            TESTED_VERSIONS.join(", ")
                        ),
                        "Output parsing is strict, so a changed format will fail loudly rather \
                         than silently. Re-capture the fixtures in tests/fixtures/powertoys/ if \
                         anything breaks.",
                    ));
                }
                version = Some(v);
            }
            Err(e) => findings.push(HealthFinding::blocker(
                format!("Could not run the Power Display CLI: {e}"),
                NOT_RUNNING_HINT,
            )),
        }

        match self.discover() {
            Ok(monitors) => {
                let ddc: Vec<&DetectedMonitor> = monitors
                    .iter()
                    .filter(|m| m.identity.transport.supports_input_switching())
                    .collect();
                findings.push(HealthFinding::info(format!(
                    "{} display(s) enumerated, {} reachable over DDC/CI",
                    monitors.len(),
                    ddc.len()
                )));
                if ddc.is_empty() {
                    findings.push(HealthFinding::blocker(
                        "No display is reachable over DDC/CI, so no input can be switched.",
                        "Check that DDC/CI is enabled in each monitor's on-screen menu, and that \
                         the monitors are connected by DisplayPort or HDMI rather than through an \
                         adapter that blocks DDC.",
                    ));
                }
                let without_serial: Vec<&&DetectedMonitor> =
                    ddc.iter().filter(|m| m.identity.serial.is_none()).collect();
                if !without_serial.is_empty() {
                    findings.push(HealthFinding::warn(
                        format!(
                            "{} DDC/CI display(s) published no EDID serial.",
                            without_serial.len()
                        ),
                        "Those displays can only be bound by connection id, so swapping their \
                         cables would silently rebind them. Re-run `configure` after any cabling \
                         change.",
                    ));
                }
            }
            Err(e) => findings.push(HealthFinding::blocker(
                format!("Could not enumerate displays: {e}"),
                NOT_RUNNING_HINT,
            )),
        }

        BackendHealth {
            backend: self.name().to_string(),
            tool_path: Some(self.exe.display().to_string()),
            tool_version: version,
            findings,
        }
    }

    fn discover(&self) -> Result<Vec<DetectedMonitor>, BackendError> {
        let run = self.run_ok(&["list"])?;
        let rows = parse::parse_list(&run.stdout).map_err(|e| BackendError::UnexpectedOutput {
            command: run.display_command(),
            detail: e.to_string(),
            raw: run.stdout.clone(),
        })?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let edid = lookup_edid(&row.id);
                DetectedMonitor {
                    identity: MonitorIdentity {
                        backend_id: row.id.clone(),
                        discovery_index: Some(row.number),
                        manufacturer: edid.as_ref().and_then(|e| e.manufacturer.clone()),
                        model: row
                            .name
                            .clone()
                            .or_else(|| edid.as_ref().and_then(|e| e.model_name.clone())),
                        serial: edid.as_ref().and_then(|e| e.best_serial()),
                        transport: transport_from_method(&row.method),
                    },
                    raw: row.raw,
                }
            })
            .collect())
    }

    fn input_capabilities(
        &self,
        monitor: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError> {
        let run = self.run_ok(&[
            "capabilities",
            "--monitor-id",
            &monitor.backend_id,
            "--setting",
            "input-source",
        ])?;
        let parsed =
            parse::parse_capabilities(&run.stdout).map_err(|e| BackendError::UnexpectedOutput {
                command: run.display_command(),
                detail: e.to_string(),
                raw: run.stdout.clone(),
            })?;

        Ok(InputCapabilities {
            feature_present: parsed.feature_present,
            options: parsed
                .options
                .into_iter()
                .map(|(code, label)| InputSourceOption {
                    code,
                    label: (!label.is_empty()).then_some(label),
                })
                .collect(),
            raw_capabilities: parsed.raw_capabilities,
            raw_output: run.stdout,
        })
    }

    fn read_input(&self, monitor: &MonitorHandle) -> Result<InputReading, BackendError> {
        let run = self.run(&[
            "get",
            "--monitor-id",
            &monitor.backend_id,
            "--setting",
            "input-source",
        ])?;

        if !run.success() {
            let err = classify_failure(&run);
            // A monitor that cannot be addressed at all is a caller error.
            // Anything else is a read that did not work this time, which the
            // domain layer interprets in context.
            if matches!(err, BackendError::MonitorNotFound { .. }) {
                return Err(err);
            }
            return Ok(InputReading::ReadFailed {
                detail: err.to_string(),
            });
        }

        let parsed = parse::parse_get(&run.stdout).map_err(|e| BackendError::UnexpectedOutput {
            command: run.display_command(),
            detail: e.to_string(),
            raw: run.stdout.clone(),
        })?;

        Ok(match parsed.input_source {
            Some(code) => InputReading::Value(code),
            None => InputReading::Unsupported,
        })
    }

    fn set_input(
        &self,
        monitor: &MonitorHandle,
        value: InputCode,
    ) -> Result<WriteOutcome, BackendError> {
        let code = value.to_string();
        let mut last: Option<BackendError> = None;

        for attempt in 1..=WRITE_ATTEMPTS {
            let run = self.run(&[
                "set",
                "--monitor-id",
                &monitor.backend_id,
                "--input-source",
                &code,
            ])?;

            if run.success() {
                // Deliberately not "switched": exit zero means the DDC write
                // was dispatched, nothing more.
                return Ok(WriteOutcome::Accepted);
            }

            let error = classify_failure(&run);
            if matches!(error, BackendError::MonitorNotFound { .. }) {
                return Err(error);
            }
            // Retrying a write that might have been accepted is the one thing
            // this project will not do, because a second input switch during
            // re-enumeration compounds the disconnect. This branch is the
            // exception: the tool has told us the write did not reach the
            // monitor, so nothing is in flight to compound.
            if !is_definite_write_failure(&run) {
                return Ok(WriteOutcome::Failed {
                    detail: error.to_string(),
                });
            }
            last = Some(error);
            if attempt < WRITE_ATTEMPTS {
                std::thread::sleep(WRITE_RETRY_DELAY);
            }
        }

        Ok(WriteOutcome::Failed {
            detail: format!(
                "{} (after {WRITE_ATTEMPTS} attempts)",
                last.map(|e| e.to_string())
                    .unwrap_or_else(|| "write failed".into())
            ),
        })
    }
}

#[cfg(windows)]
fn lookup_edid(device_path: &str) -> Option<switcher_core::edid::EdidInfo> {
    edid_source::read_for_device_path(device_path)
}

#[cfg(not(windows))]
fn lookup_edid(_device_path: &str) -> Option<switcher_core::edid::EdidInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_configured_tool_path_is_reported_clearly() {
        let err = PowerDisplayBackend::locate(Some(Path::new("Z:/nope/missing.exe"))).unwrap_err();
        assert!(err.to_string().contains("tool_path"), "{err}");
    }

    #[test]
    fn unknown_monitor_errors_are_classified_not_lumped_together() {
        let run = CommandRun {
            program: "cli".into(),
            args: vec!["get".into()],
            status: Some(1),
            stdout: String::new(),
            stderr: "Error: no monitor found with id NO-SUCH-MONITOR".into(),
            duration: Duration::ZERO,
        };
        assert!(matches!(
            classify_failure(&run),
            BackendError::MonitorNotFound { .. }
        ));
    }

    #[test]
    fn other_failures_keep_their_stderr_and_exit_code() {
        let run = CommandRun {
            program: "cli".into(),
            args: vec!["set".into()],
            status: Some(7),
            stdout: String::new(),
            stderr: "Error: ddc write timed out".into(),
            duration: Duration::ZERO,
        };
        match classify_failure(&run) {
            BackendError::CommandFailed { status, stderr, .. } => {
                assert_eq!(status, Some(7));
                assert!(stderr.contains("timed out"));
            }
            other => panic!("unexpected classification: {other:?}"),
        }
    }

    /// Captured from the event log on 2026-09-17: the same switch that
    /// worked seconds earlier and seconds later.
    #[test]
    fn a_rejected_hardware_write_is_recognised_as_safe_to_retry() {
        let run = CommandRun {
            program: "cli".into(),
            args: vec!["set".into()],
            status: Some(5),
            stdout: String::new(),
            stderr: "Error: hardware write failed\n  monitor: Monitor 3 (27P2DG5)\n  \
                     diagnostic: Failed to set VCP 0x60"
                .into(),
            duration: Duration::ZERO,
        };
        assert!(is_definite_write_failure(&run));
    }

    #[test]
    fn a_vague_failure_is_not_retried() {
        // Could mean the write landed and the reply was lost, so repeating it
        // could switch a monitor twice.
        let run = CommandRun {
            program: "cli".into(),
            args: vec!["set".into()],
            status: Some(1),
            stdout: String::new(),
            stderr: "Error: timed out waiting for the display".into(),
            duration: Duration::ZERO,
        };
        assert!(!is_definite_write_failure(&run));
    }

    #[test]
    fn transports_map_to_switchability() {
        assert!(transport_from_method("DDC/CI").supports_input_switching());
        assert!(!transport_from_method("WMI").supports_input_switching());
        assert!(!transport_from_method("Something").supports_input_switching());
    }
}
