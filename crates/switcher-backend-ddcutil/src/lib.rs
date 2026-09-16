//! Ubuntu backend: a process adapter over the `ddcutil` CLI.
//!
//! **Status: not yet validated against the HP Z4.** The command shapes follow
//! ddcutil's documentation, and the parsers are tested against synthetic
//! fixtures. Run `scripts/ubuntu-preflight.sh` on the real host and replace
//! `tests/fixtures/ddcutil/` before relying on any of it.
//!
//! Two decisions worth stating:
//!
//! * Displays are selected by `--sn`/`--model` whenever the panel publishes a
//!   serial. `--display N` is never used for a write: those numbers, and the
//!   I²C bus numbering behind them, are discovery-time addresses that move.
//! * All calls are serialised through a mutex. Overlapping DDC transactions on
//!   one I²C bus corrupt each other, and nothing here is fast enough for the
//!   parallelism to be worth it.

pub mod parse;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use switcher_core::backend::{BackendError, BackendHealth, HealthFinding, MonitorBackend};
use switcher_core::mccs;
use switcher_core::proc::{self, CommandRun};
use switcher_core::types::{
    DetectedMonitor, InputCapabilities, InputCode, InputReading, InputSourceOption, MonitorHandle,
    MonitorIdentity, Transport, WriteOutcome,
};

pub const EXE_NAME: &str = "ddcutil";

const PERMISSION_HINT: &str =
    "Add yourself to the `i2c` group (`sudo usermod -aG i2c $USER`, then \
     log out and back in), and make sure the `i2c-dev` module is loaded. See \
     https://www.ddcutil.com/i2c_permissions/ . Do not run this tool with sudo.";

pub struct DdcutilBackend {
    exe: PathBuf,
    timeout: Duration,
    /// Serialises DDC traffic; see the module note.
    bus_lock: Mutex<()>,
}

impl DdcutilBackend {
    pub fn new(exe: PathBuf, timeout: Duration) -> Self {
        Self {
            exe,
            timeout,
            bus_lock: Mutex::new(()),
        }
    }

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
        let extra = [
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/bin"),
        ];
        proc::find_executable(EXE_NAME, &extra).ok_or_else(|| BackendError::ToolNotFound {
            tool: EXE_NAME.to_string(),
            hint: "Install it with `sudo apt install ddcutil`, or set `tool_path` in config.toml."
                .into(),
        })
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    fn run(&self, args: &[&str]) -> Result<CommandRun, BackendError> {
        let _guard = self.bus_lock.lock().unwrap_or_else(|e| e.into_inner());
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Ok(proc::run(&self.exe, &owned, self.timeout)?)
    }

    fn run_ok(&self, args: &[&str]) -> Result<CommandRun, BackendError> {
        let run = self.run(args)?;
        if run.success() {
            return Ok(run);
        }
        Err(classify_failure(&run))
    }

    pub fn version(&self) -> Result<String, BackendError> {
        let run = self.run_ok(&["--version"])?;
        Ok(run
            .stdout
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string())
    }

    fn detect(&self) -> Result<Vec<parse::DetectedDisplay>, BackendError> {
        let run = self.run_ok(&["detect"])?;
        parse::parse_detect(&run.stdout).map_err(|e| BackendError::UnexpectedOutput {
            command: run.display_command(),
            detail: e.to_string(),
            raw: run.stdout.clone(),
        })
    }

    /// Build the selector arguments that address exactly one display.
    ///
    /// Never falls back to `--display 1`: an unresolvable handle is an error.
    fn selector(&self, handle: &MonitorHandle) -> Result<Vec<String>, BackendError> {
        if let Some(serial) = handle.serial.as_deref().filter(|s| !s.is_empty()) {
            let mut args = vec!["--sn".to_string(), serial.to_string()];
            if let Some(model) = handle.model.as_deref().filter(|m| !m.is_empty()) {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            return Ok(args);
        }

        // No serial to bind to, so resolve the port id to a bus number now
        // rather than trusting one recorded earlier.
        let displays = self.detect()?;
        let matches: Vec<&parse::DetectedDisplay> = displays
            .iter()
            .filter(|d| !d.invalid && d.stable_id() == handle.backend_id)
            .collect();

        match matches.as_slice() {
            [one] => {
                let bus = one
                    .i2c_bus
                    .as_deref()
                    .and_then(bus_number)
                    .ok_or_else(|| BackendError::MonitorNotFound {
                        detail: format!(
                            "{} has no I2C bus number and no serial, so it cannot be addressed safely",
                            handle.backend_id
                        ),
                    })?;
                Ok(vec!["--bus".to_string(), bus.to_string()])
            }
            [] => Err(BackendError::MonitorNotFound {
                detail: format!("no display is currently at {}", handle.backend_id),
            }),
            many => Err(BackendError::MonitorNotFound {
                detail: format!(
                    "{} displays report the connection {}; refusing to guess",
                    many.len(),
                    handle.backend_id
                ),
            }),
        }
    }
}

/// `/dev/i2c-5` -> `5`.
fn bus_number(dev: &str) -> Option<u32> {
    dev.rsplit_once("i2c-")?.1.trim().parse().ok()
}

fn classify_failure(run: &CommandRun) -> BackendError {
    let combined = format!("{}\n{}", run.stdout.trim(), run.stderr.trim());
    let lower = combined.to_lowercase();

    if lower.contains("permission denied") || lower.contains("eacces") {
        return BackendError::PermissionDenied {
            detail: format!("{}. {PERMISSION_HINT}", combined.trim()),
        };
    }
    if lower.contains("no such file") && lower.contains("i2c") {
        return BackendError::PermissionDenied {
            detail: format!(
                "{}. The i2c-dev module may not be loaded (`sudo modprobe i2c-dev`).",
                combined.trim()
            ),
        };
    }
    if lower.contains("display not found")
        || lower.contains("no display found")
        || lower.contains("invalid display")
    {
        return BackendError::MonitorNotFound {
            detail: combined.trim().to_string(),
        };
    }
    if lower.contains("unsupported feature") || lower.contains("feature not supported") {
        return BackendError::Unsupported {
            detail: combined.trim().to_string(),
        };
    }
    BackendError::CommandFailed {
        command: run.display_command(),
        status: run.status,
        stderr: combined.trim().to_string(),
    }
}

impl MonitorBackend for DdcutilBackend {
    fn name(&self) -> &str {
        "ddcutil-cli"
    }

    fn health(&self) -> BackendHealth {
        let mut findings = Vec::new();
        let mut version = None;

        match self.version() {
            Ok(v) => {
                findings.push(HealthFinding::info(format!("ddcutil reports: {v}")));
                version = Some(v);
            }
            Err(e) => findings.push(HealthFinding::blocker(
                format!("Could not run ddcutil: {e}"),
                "Install it with `sudo apt install ddcutil`.",
            )),
        }

        // These checks are Linux-only and read-only.
        #[cfg(unix)]
        {
            let i2c_devices: Vec<PathBuf> = std::fs::read_dir("/dev")
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.starts_with("i2c-"))
                        })
                        .collect()
                })
                .unwrap_or_default();

            if i2c_devices.is_empty() {
                findings.push(HealthFinding::blocker(
                    "No /dev/i2c-* devices exist.",
                    "Load the kernel module with `sudo modprobe i2c-dev`, and make it permanent \
                     with `echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf`.",
                ));
            } else {
                let unwritable: Vec<&PathBuf> = i2c_devices
                    .iter()
                    .filter(|p| {
                        std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(p)
                            .is_err()
                    })
                    .collect();
                if unwritable.len() == i2c_devices.len() {
                    findings.push(HealthFinding::blocker(
                        format!(
                            "None of the {} /dev/i2c-* devices are accessible to this user.",
                            i2c_devices.len()
                        ),
                        PERMISSION_HINT,
                    ));
                } else if !unwritable.is_empty() {
                    findings.push(HealthFinding::info(format!(
                        "{} of {} /dev/i2c-* devices are accessible to this user.",
                        i2c_devices.len() - unwritable.len(),
                        i2c_devices.len()
                    )));
                } else {
                    findings.push(HealthFinding::info(format!(
                        "All {} /dev/i2c-* devices are accessible without sudo.",
                        i2c_devices.len()
                    )));
                }
            }
        }

        match self.discover() {
            Ok(monitors) => {
                let ddc = monitors
                    .iter()
                    .filter(|m| m.identity.transport.supports_input_switching())
                    .count();
                findings.push(HealthFinding::info(format!(
                    "{} display(s) detected, {ddc} usable over DDC/CI",
                    monitors.len()
                )));
                if ddc == 0 {
                    findings.push(HealthFinding::blocker(
                        "No display is usable over DDC/CI.",
                        "Enable DDC/CI in each monitor's on-screen menu, and check permissions.",
                    ));
                }
                let without_serial = monitors
                    .iter()
                    .filter(|m| {
                        m.identity.transport.supports_input_switching()
                            && m.identity.serial.is_none()
                    })
                    .count();
                if without_serial > 0 {
                    findings.push(HealthFinding::warn(
                        format!("{without_serial} display(s) published no serial number."),
                        "Identical panels without serials cannot be told apart reliably. \
                         `configure` will ask you to identify them physically.",
                    ));
                }
            }
            Err(e) => findings.push(HealthFinding::blocker(
                format!("Could not detect displays: {e}"),
                PERMISSION_HINT,
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
        Ok(self
            .detect()?
            .into_iter()
            .map(|d| DetectedMonitor {
                identity: MonitorIdentity {
                    backend_id: d.stable_id(),
                    discovery_index: d.display_number,
                    manufacturer: d.manufacturer.clone(),
                    model: d.model.clone(),
                    serial: d.serial.clone().filter(|s| !s.is_empty()),
                    transport: if d.invalid {
                        Transport::Other("no DDC support".into())
                    } else {
                        Transport::DdcCi
                    },
                },
                raw: d.raw.clone(),
            })
            .collect())
    }

    fn input_capabilities(
        &self,
        monitor: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError> {
        let selector = self.selector(monitor)?;
        let mut args: Vec<&str> = selector.iter().map(String::as_str).collect();
        args.push("capabilities");

        let run = self.run_ok(&args)?;
        let parsed =
            parse::parse_capabilities(&run.stdout).map_err(|e| BackendError::UnexpectedOutput {
                command: run.display_command(),
                detail: e.to_string(),
                raw: run.stdout.clone(),
            })?;

        // Ask again for the raw MCCS string. It is parsed by the shared,
        // better-tested parser, and disagreement between the two is worth
        // seeing rather than hiding.
        let mut terse_args: Vec<&str> = selector.iter().map(String::as_str).collect();
        terse_args.extend_from_slice(&["capabilities", "--terse"]);
        let raw_capabilities = self
            .run_ok(&terse_args)
            .ok()
            .map(|r| r.stdout.trim().to_string())
            .filter(|s| s.starts_with('('));

        let mut options: Vec<InputSourceOption> = parsed
            .options
            .into_iter()
            .map(|(code, label)| InputSourceOption {
                code,
                label: (!label.is_empty()).then_some(label),
            })
            .collect();

        if options.is_empty() {
            if let Some(raw) = raw_capabilities.as_deref() {
                if let Ok(Some(values)) = mccs::input_source_values(raw) {
                    options = values
                        .into_iter()
                        .map(|v| InputSourceOption {
                            code: InputCode(v),
                            label: InputCode(v).standard_label().map(String::from),
                        })
                        .collect();
                }
            }
        }

        Ok(InputCapabilities {
            feature_present: parsed.feature_present || !options.is_empty(),
            options,
            raw_capabilities,
            raw_output: run.stdout,
        })
    }

    fn read_input(&self, monitor: &MonitorHandle) -> Result<InputReading, BackendError> {
        let selector = self.selector(monitor)?;
        let mut args: Vec<&str> = selector.iter().map(String::as_str).collect();
        args.extend_from_slice(&["getvcp", "60", "--terse"]);

        let run = self.run(&args)?;
        if !run.success() {
            return match classify_failure(&run) {
                e @ BackendError::MonitorNotFound { .. } => Err(e),
                BackendError::Unsupported { .. } => Ok(InputReading::Unsupported),
                other => Ok(InputReading::ReadFailed {
                    detail: other.to_string(),
                }),
            };
        }

        match parse::parse_getvcp_input(&run.stdout) {
            Ok(code) => Ok(InputReading::Value(code)),
            Err(parse::ParseError::NotInputSource(_)) => Ok(InputReading::Unsupported),
            Err(e) => Err(BackendError::UnexpectedOutput {
                command: run.display_command(),
                detail: e.to_string(),
                raw: run.stdout.clone(),
            }),
        }
    }

    fn set_input(
        &self,
        monitor: &MonitorHandle,
        value: InputCode,
    ) -> Result<WriteOutcome, BackendError> {
        let selector = self.selector(monitor)?;
        let code = format!("{:#04x}", value.get());
        let mut args: Vec<&str> = selector.iter().map(String::as_str).collect();
        args.extend_from_slice(&["setvcp", "60", &code]);

        let run = self.run(&args)?;
        if run.success() {
            // ddcutil returning 0 means the write went out. Whether the panel
            // changed input is not knowable from here; the domain layer says so.
            return Ok(WriteOutcome::Accepted);
        }
        match classify_failure(&run) {
            e @ BackendError::MonitorNotFound { .. } => Err(e),
            other => Ok(WriteOutcome::Failed {
                detail: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bus_numbers() {
        assert_eq!(bus_number("/dev/i2c-5"), Some(5));
        assert_eq!(bus_number("/dev/i2c-12"), Some(12));
        assert_eq!(bus_number("i2c-0"), Some(0));
        assert_eq!(bus_number("/dev/ttyS0"), None);
    }

    #[test]
    fn permission_errors_are_classified_with_a_non_sudo_remedy() {
        let run = CommandRun {
            program: "ddcutil".into(),
            args: vec!["detect".into()],
            status: Some(1),
            stdout: String::new(),
            stderr: "Error opening /dev/i2c-5: Permission denied".into(),
            duration: Duration::ZERO,
        };
        match classify_failure(&run) {
            BackendError::PermissionDenied { detail } => {
                assert!(detail.contains("i2c"));
                // The remedy must not be "run it as root".
                assert!(detail.contains("Do not run this tool with sudo"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_displays_are_classified_separately_from_generic_failures() {
        let run = CommandRun {
            program: "ddcutil".into(),
            args: vec!["getvcp".into()],
            status: Some(1),
            stdout: "Display not found".into(),
            stderr: String::new(),
            duration: Duration::ZERO,
        };
        assert!(matches!(
            classify_failure(&run),
            BackendError::MonitorNotFound { .. }
        ));
    }

    #[test]
    fn unknown_failures_keep_their_exit_code_and_text() {
        let run = CommandRun {
            program: "ddcutil".into(),
            args: vec!["setvcp".into()],
            status: Some(4),
            stdout: String::new(),
            stderr: "DDC communication failed".into(),
            duration: Duration::ZERO,
        };
        match classify_failure(&run) {
            BackendError::CommandFailed { status, stderr, .. } => {
                assert_eq!(status, Some(4));
                assert!(stderr.contains("DDC communication failed"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn a_serial_bearing_handle_is_addressed_by_identity_not_by_number() {
        let backend = DdcutilBackend::new(PathBuf::from("/nonexistent"), Duration::from_secs(1));
        let handle = MonitorHandle {
            backend_id: "card1-DP-1".into(),
            discovery_index: Some(1),
            model: Some("27P2Q".into()),
            serial: Some("ASFPA9A001108".into()),
        };
        let args = backend.selector(&handle).unwrap();
        assert_eq!(args, vec!["--sn", "ASFPA9A001108", "--model", "27P2Q"]);
        // Crucially, the display number never appears.
        assert!(!args.iter().any(|a| a == "--display"));
    }

    #[test]
    fn input_codes_are_formatted_as_ddcutil_expects() {
        assert_eq!(format!("{:#04x}", 0x0Fu8), "0x0f");
        assert_eq!(format!("{:#04x}", 0x11u8), "0x11");
        assert_eq!(format!("{:#04x}", 0x01u8), "0x01");
    }
}
