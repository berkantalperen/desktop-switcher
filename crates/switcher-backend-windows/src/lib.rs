//! Windows backend: talks to monitors through Windows' own DDC/CI API.
//!
//! Nothing to install and nothing to keep running. It replaces a backend that
//! drove PowerToys' Power Display CLI, which used this same API underneath but
//! hid any monitor whose capabilities string it could not read. That string is
//! a long, multi-part transfer, and a cable can fail to carry it while every
//! short command a switch needs gets through: a panel sat on the desk, fully
//! switchable, and was reported as not attached.
//!
//! So the rule here is that **discovery never talks DDC**. A monitor is listed
//! because Windows' display topology says it is there, and identified by the
//! EDID Windows cached when it was plugged in. Whether it answers a particular
//! request is reported on that request, and never decides whether it exists.

pub mod topology;

#[cfg(windows)]
mod ddc;
#[cfg(windows)]
mod edid_source;

use std::time::Duration;

use switcher_core::backend::{BackendError, BackendHealth, MonitorBackend};
use switcher_core::types::{
    DetectedMonitor, InputCapabilities, InputCode, InputReading, MonitorHandle, WriteOutcome,
};

/// How many times a write is sent when Windows reports it failed.
///
/// Only a reported failure is retried. Repeating an absolute input is harmless
/// by design, and DDC over a converter cable drops the odd transaction.
const WRITE_ATTEMPTS: usize = 3;
/// Reads are retried the same way; they change nothing.
const READ_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(400);

/// How long a write waits for Windows to finish re-detecting its displays
/// before believing a missing or mirrored display is real: about five seconds.
/// Reads never wait, so a hotkey is never slowed by a read-back.
const SETTLE_ATTEMPTS: usize = 20;
const SETTLE_INTERVAL: Duration = Duration::from_millis(250);

pub struct WindowsBackend {
    attempts: usize,
    retry_delay: Duration,
}

impl WindowsBackend {
    pub fn new() -> Self {
        Self {
            attempts: WRITE_ATTEMPTS.max(READ_ATTEMPTS),
            retry_delay: RETRY_DELAY,
        }
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MonitorBackend for WindowsBackend {
    fn name(&self) -> &str {
        "windows"
    }

    fn health(&self) -> BackendHealth {
        imp::health(self)
    }

    fn discover(&self) -> Result<Vec<DetectedMonitor>, BackendError> {
        imp::discover()
    }

    fn input_capabilities(
        &self,
        monitor: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError> {
        imp::input_capabilities(self, monitor)
    }

    fn read_input(&self, monitor: &MonitorHandle) -> Result<InputReading, BackendError> {
        imp::read_input(self, monitor)
    }

    fn set_input(
        &self,
        monitor: &MonitorHandle,
        value: InputCode,
    ) -> Result<WriteOutcome, BackendError> {
        imp::set_input(self, monitor, value)
    }
}

#[cfg(windows)]
mod imp {
    use switcher_core::backend::{BackendError, BackendHealth, HealthFinding};
    use switcher_core::types::{
        DetectedMonitor, InputCapabilities, InputCode, InputReading, MonitorHandle, WriteOutcome,
    };
    use switcher_desktop::Connector;

    use crate::ddc::{self, PhysicalMonitors};
    use crate::edid_source;
    use crate::topology::{self, Refusal, TopologyEntry, INPUT_SOURCE, VCP_NOT_SUPPORTED};
    use crate::{WindowsBackend, SETTLE_ATTEMPTS, SETTLE_INTERVAL};

    fn topology_now() -> Result<Vec<TopologyEntry>, BackendError> {
        let displays = switcher_desktop::for_this_platform()
            .displays()
            .map_err(|e| BackendError::Io(format!("reading the display topology: {e}")))?;
        Ok(displays
            .into_iter()
            .map(|d| TopologyEntry {
                internal: d.connector == Some(Connector::Internal),
                attached: d.is_attached,
                device_path: d.device_path,
                gdi_name: d.gdi_name,
            })
            .collect())
    }

    pub fn discover() -> Result<Vec<DetectedMonitor>, BackendError> {
        let entries = topology_now()?;
        Ok(entries
            .iter()
            .filter(|e| e.attached)
            .enumerate()
            .map(|(i, e)| {
                let edid = edid_source::read_for_device_path(&e.device_path);
                DetectedMonitor {
                    identity: topology::identity_for(e, i as u32 + 1, edid.as_ref()),
                    raw: format!("{} on {}", e.device_path, e.gdi_name),
                }
            })
            .collect())
    }

    /// Open the one physical monitor for a handle, or refuse.
    ///
    /// With `settle`, a missing or mirrored display is re-checked for a few
    /// seconds first: an action's second step can run while Windows is still
    /// re-detecting after its first.
    fn open(handle: &MonitorHandle, settle: bool) -> Result<PhysicalMonitors, BackendError> {
        let attempts = if settle { SETTLE_ATTEMPTS } else { 1 };
        // A failure to read the topology at all is not a refusal to wait out;
        // it is kept aside, and the probe answers with something structural
        // so that `settle` stops at once.
        let mut failed: Option<BackendError> = None;
        let located = topology::settle(
            attempts,
            || match topology_now() {
                Ok(entries) => topology::locate(&entries, &handle.backend_id).cloned(),
                Err(e) => {
                    failed = Some(e);
                    Err(Refusal::Internal)
                }
            },
            || std::thread::sleep(SETTLE_INTERVAL),
        );
        if let Some(e) = failed {
            return Err(e);
        }
        let entry = located.map_err(|r| r.into_error(&handle.backend_id))?;

        // The binding layer matched this id to a serial moments ago. Check it
        // still holds, so a replugged cable can never redirect a write.
        if let Some(want) = handle.serial.as_deref() {
            let have =
                edid_source::read_for_device_path(&entry.device_path).and_then(|e| e.best_serial());
            if let Some(have) = have {
                if !have.eq_ignore_ascii_case(want) {
                    return Err(BackendError::MonitorNotFound {
                        detail: format!(
                            "`{}` now reports serial {have}, not {want}; refusing to write to it",
                            handle.backend_id
                        ),
                    });
                }
            }
        }

        let monitors = ddc::physical_monitors_for(&entry.gdi_name).map_err(|e| {
            BackendError::MonitorNotFound {
                detail: format!("`{}`: {e}", handle.backend_id),
            }
        })?;
        if monitors.only().is_none() {
            return Err(BackendError::Unsupported {
                detail: format!(
                    "Windows reports {} physical monitors behind `{}`, and gives no way to \
                     tell which is which; refusing to guess",
                    monitors.len(),
                    handle.backend_id
                ),
            });
        }
        Ok(monitors)
    }

    fn retry<T>(
        backend: &WindowsBackend,
        mut attempt: impl FnMut() -> Result<T, ddc::DdcError>,
    ) -> Result<T, ddc::DdcError> {
        let mut last = None;
        for n in 0..backend.attempts {
            if n > 0 {
                std::thread::sleep(backend.retry_delay);
            }
            match attempt() {
                Ok(v) => return Ok(v),
                // A monitor that says it has no such feature will say so again.
                Err(e) if e.code == VCP_NOT_SUPPORTED => return Err(e),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("at least one attempt"))
    }

    pub fn read_input(
        backend: &WindowsBackend,
        handle: &MonitorHandle,
    ) -> Result<InputReading, BackendError> {
        let monitors = open(handle, false)?;
        let monitor = monitors.only().expect("open() guarantees one");
        Ok(
            match retry(backend, || ddc::get_vcp(monitor, INPUT_SOURCE)) {
                Ok(value) => topology::reading_from_reply(value),
                Err(e) if e.code == VCP_NOT_SUPPORTED => InputReading::Unsupported,
                Err(e) => InputReading::ReadFailed {
                    detail: format!(
                        "{e}. A monitor does this while asleep: on an input with nothing \
                         plugged in, or whose computer has turned its screen off."
                    ),
                },
            },
        )
    }

    pub fn set_input(
        backend: &WindowsBackend,
        handle: &MonitorHandle,
        value: InputCode,
    ) -> Result<WriteOutcome, BackendError> {
        let monitors = open(handle, true)?;
        let monitor = monitors.only().expect("open() guarantees one");
        Ok(
            match retry(backend, || {
                ddc::set_vcp(monitor, INPUT_SOURCE, value.0 as u32)
            }) {
                Ok(()) => WriteOutcome::Accepted,
                Err(e) => WriteOutcome::Failed {
                    detail: format!("{e} (after {} attempts)", backend.attempts),
                },
            },
        )
    }

    pub fn input_capabilities(
        backend: &WindowsBackend,
        handle: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError> {
        let monitors = open(handle, true)?;
        let monitor = monitors.only().expect("open() guarantees one");
        let caps = retry(backend, || ddc::capabilities(monitor)).map_err(|e| {
            BackendError::Io(format!(
                "`{}` did not return its capabilities: {e}. This is a long transfer that some \
                 cables and adapters cannot carry; switching inputs does not depend on it.",
                handle.backend_id
            ))
        })?;
        topology::capabilities_from(&caps).map_err(|detail| BackendError::UnexpectedOutput {
            command: "CapabilitiesRequestAndCapabilitiesReply".into(),
            detail,
            raw: caps,
        })
    }

    pub fn health(backend: &WindowsBackend) -> BackendHealth {
        let mut findings = vec![HealthFinding::info(
            "Talks to monitors through Windows' own DDC/CI API; nothing to install or keep \
             running.",
        )];
        match discover() {
            Err(e) => findings.push(HealthFinding::blocker(
                format!("Could not read the display topology: {e}"),
                "This is a Windows API failure, not a monitor problem. Try again, and reboot \
                 if it persists.",
            )),
            Ok(found) => {
                let external: Vec<&DetectedMonitor> = found
                    .iter()
                    .filter(|m| m.identity.transport.supports_input_switching())
                    .collect();
                let silent: Vec<String> = external
                    .iter()
                    .filter(|m| {
                        !matches!(read_input(backend, &m.handle()), Ok(InputReading::Value(_)))
                    })
                    .map(|m| m.identity.describe())
                    .collect();
                findings.push(HealthFinding::info(format!(
                    "{} display(s), {} external; {} answered just now",
                    found.len(),
                    external.len(),
                    external.len() - silent.len()
                )));
                if !silent.is_empty() {
                    findings.push(HealthFinding::warn(
                        format!("Not answering right now: {}", silent.join(", ")),
                        "A monitor goes quiet while asleep, on an input with nothing plugged \
                         in or whose computer has turned its screen off. It wakes when a \
                         picture arrives on that input, not before: a sleeping monitor \
                         ignores writes as well as reads. Its own buttons always work.",
                    ));
                }
            }
        }
        BackendHealth {
            backend: "windows".into(),
            tool_path: None,
            tool_version: None,
            findings,
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use switcher_core::backend::{BackendError, BackendHealth, HealthFinding};
    use switcher_core::types::{
        DetectedMonitor, InputCapabilities, InputCode, InputReading, MonitorHandle, WriteOutcome,
    };

    use crate::WindowsBackend;

    fn unsupported() -> BackendError {
        BackendError::Unsupported {
            detail: "the windows backend only runs on Windows; use ddcutil here".into(),
        }
    }

    pub fn health(_: &WindowsBackend) -> BackendHealth {
        BackendHealth {
            backend: "windows".into(),
            tool_path: None,
            tool_version: None,
            findings: vec![HealthFinding::blocker(
                "The windows backend only runs on Windows.",
                "Use `--backend ddcutil` on Linux.",
            )],
        }
    }

    pub fn discover() -> Result<Vec<DetectedMonitor>, BackendError> {
        Err(unsupported())
    }

    pub fn input_capabilities(
        _: &WindowsBackend,
        _: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError> {
        Err(unsupported())
    }

    pub fn read_input(_: &WindowsBackend, _: &MonitorHandle) -> Result<InputReading, BackendError> {
        Err(unsupported())
    }

    pub fn set_input(
        _: &WindowsBackend,
        _: &MonitorHandle,
        _: InputCode,
    ) -> Result<WriteOutcome, BackendError> {
        Err(unsupported())
    }
}
