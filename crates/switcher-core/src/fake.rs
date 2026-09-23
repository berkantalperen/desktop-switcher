//! An in-memory backend, so every domain rule can be exercised — including
//! the failure modes that are unsafe or impossible to stage on real hardware.

use std::sync::Mutex;

use crate::backend::{BackendError, BackendHealth, HealthFinding, MonitorBackend};
use crate::types::{
    DetectedMonitor, InputCapabilities, InputCode, InputReading, InputSourceOption, MonitorHandle,
    MonitorIdentity, Transport, WriteOutcome,
};

/// How a fake monitor misbehaves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FakeBehavior {
    #[default]
    Normal,
    /// Accepts the write, then stops answering — what really happens when a
    /// monitor is switched to another computer.
    SilentAfterWrite,
    /// The write command itself reports failure.
    WriteFails(String),
    /// Reads fail even though the link is believed up.
    ReadFails(String),
    /// Monitor does not implement feature 0x60.
    NoInputFeature,
    /// Accepts the write but never actually changes input.
    IgnoresWrites,
}

#[derive(Debug, Clone)]
pub struct FakeMonitor {
    pub identity: MonitorIdentity,
    pub current: Option<InputCode>,
    pub advertised: Vec<InputCode>,
    pub behavior: FakeBehavior,
    /// Set once this monitor has been written to and gone quiet.
    pub silent: bool,
    /// An input this monitor claims regardless of the one it is really on.
    ///
    /// Real panels do this: one physically showing HDMI answered reads with
    /// DisplayPort while another computer was driving it.
    pub misreports: Option<InputCode>,
}

impl FakeMonitor {
    pub fn new(backend_id: &str, serial: Option<&str>, current: u8) -> Self {
        Self {
            identity: MonitorIdentity {
                backend_id: backend_id.to_string(),
                discovery_index: None,
                manufacturer: Some("AOC".into()),
                model: Some("27P2DG5".into()),
                serial: serial.map(String::from),
                transport: Transport::DdcCi,
            },
            current: Some(InputCode(current)),
            advertised: vec![
                InputCode(0x01),
                InputCode(0x03),
                InputCode(0x11),
                InputCode(0x0F),
            ],
            behavior: FakeBehavior::Normal,
            silent: false,
            misreports: None,
        }
    }

    pub fn internal_panel(backend_id: &str) -> Self {
        let mut m = Self::new(backend_id, Some("PANEL"), 0x00);
        m.identity.manufacturer = Some("BOE".into());
        m.identity.model = Some("NE16NZH".into());
        m.identity.transport = Transport::Internal;
        m.current = None;
        m.advertised.clear();
        m
    }

    pub fn with_behavior(mut self, behavior: FakeBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    /// Make reads answer with `code` whatever the monitor is actually on.
    pub fn reporting_input(mut self, code: InputCode) -> Self {
        self.misreports = Some(code);
        self
    }

    pub fn with_index(mut self, index: u32) -> Self {
        self.identity.discovery_index = Some(index);
        self
    }
}

#[derive(Debug, Default)]
struct FakeState {
    monitors: Vec<FakeMonitor>,
    writes: Vec<(String, InputCode)>,
    reads: Vec<String>,
}

pub struct FakeBackend {
    state: Mutex<FakeState>,
    discover_error: Option<BackendError>,
}

impl FakeBackend {
    pub fn new(monitors: Vec<FakeMonitor>) -> Self {
        Self {
            state: Mutex::new(FakeState {
                monitors,
                ..Default::default()
            }),
            discover_error: None,
        }
    }

    pub fn failing_discovery(error: BackendError) -> Self {
        Self {
            state: Mutex::new(FakeState::default()),
            discover_error: Some(error),
        }
    }

    /// Every write issued, in order. Used to assert that read-only commands
    /// never write, and that a switch touches each monitor exactly once.
    pub fn writes(&self) -> Vec<(String, InputCode)> {
        self.state.lock().unwrap().writes.clone()
    }

    pub fn reads(&self) -> Vec<String> {
        self.state.lock().unwrap().reads.clone()
    }

    pub fn current_input(&self, backend_id: &str) -> Option<InputCode> {
        self.state
            .lock()
            .unwrap()
            .monitors
            .iter()
            .find(|m| m.identity.backend_id == backend_id)
            .and_then(|m| m.current)
    }

    /// Simulate the user changing input with the monitor's own buttons.
    pub fn set_physically(&self, backend_id: &str, code: u8) {
        if let Some(m) = self
            .state
            .lock()
            .unwrap()
            .monitors
            .iter_mut()
            .find(|m| m.identity.backend_id == backend_id)
        {
            m.current = Some(InputCode(code));
            m.silent = false;
        }
    }

    /// Simulate unplugging a display.
    pub fn detach(&self, backend_id: &str) {
        self.state
            .lock()
            .unwrap()
            .monitors
            .retain(|m| m.identity.backend_id != backend_id);
    }
}

fn find_index(state: &FakeState, handle: &MonitorHandle) -> Result<usize, BackendError> {
    state
        .monitors
        .iter()
        .position(|m| m.identity.backend_id == handle.backend_id)
        .ok_or_else(|| BackendError::MonitorNotFound {
            detail: format!("no fake monitor with id {}", handle.backend_id),
        })
}

impl MonitorBackend for FakeBackend {
    fn name(&self) -> &str {
        "fake"
    }

    fn health(&self) -> BackendHealth {
        BackendHealth {
            backend: "fake".into(),
            tool_path: None,
            tool_version: Some("in-memory".into()),
            findings: vec![HealthFinding::info(
                "Fake backend: no hardware is touched by any command.",
            )],
        }
    }

    fn discover(&self) -> Result<Vec<DetectedMonitor>, BackendError> {
        if let Some(e) = &self.discover_error {
            return Err(e.clone());
        }
        let state = self.state.lock().unwrap();
        Ok(state
            .monitors
            .iter()
            .map(|m| DetectedMonitor {
                identity: m.identity.clone(),
                raw: format!("fake://{}", m.identity.backend_id),
            })
            .collect())
    }

    fn input_capabilities(
        &self,
        handle: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError> {
        let state = self.state.lock().unwrap();
        let idx = find_index(&state, handle)?;
        let m = &state.monitors[idx];
        let present = m.behavior != FakeBehavior::NoInputFeature && !m.advertised.is_empty();
        Ok(InputCapabilities {
            feature_present: present,
            options: if present {
                m.advertised
                    .iter()
                    .map(|c| InputSourceOption {
                        code: *c,
                        label: c.standard_label().map(String::from),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            raw_capabilities: present.then(|| "(vcp(60(01 03 11 0F)))".to_string()),
            raw_output: format!("fake capabilities for {}", handle.backend_id),
        })
    }

    fn read_input(&self, handle: &MonitorHandle) -> Result<InputReading, BackendError> {
        let mut state = self.state.lock().unwrap();
        let idx = find_index(&state, handle)?;
        state.reads.push(handle.backend_id.clone());
        let m = &state.monitors[idx];

        if m.behavior == FakeBehavior::NoInputFeature {
            return Ok(InputReading::Unsupported);
        }
        if m.silent {
            return Ok(InputReading::UnavailableAfterSwitch {
                detail: "monitor stopped answering after switching away".into(),
            });
        }
        if let FakeBehavior::ReadFails(detail) = &m.behavior {
            return Ok(InputReading::ReadFailed {
                detail: detail.clone(),
            });
        }
        if let Some(claimed) = m.misreports {
            return Ok(InputReading::Value(claimed));
        }
        match m.current {
            Some(c) => Ok(InputReading::Value(c)),
            None => Ok(InputReading::Unsupported),
        }
    }

    fn set_input(
        &self,
        handle: &MonitorHandle,
        value: InputCode,
    ) -> Result<WriteOutcome, BackendError> {
        let mut state = self.state.lock().unwrap();
        let idx = find_index(&state, handle)?;
        state.writes.push((handle.backend_id.clone(), value));
        let m = &mut state.monitors[idx];

        match m.behavior.clone() {
            FakeBehavior::NoInputFeature => Err(BackendError::Unsupported {
                detail: "monitor does not implement VCP 0x60".into(),
            }),
            FakeBehavior::WriteFails(detail) => Ok(WriteOutcome::Failed { detail }),
            FakeBehavior::IgnoresWrites => Ok(WriteOutcome::Accepted),
            FakeBehavior::SilentAfterWrite => {
                m.current = Some(value);
                m.silent = true;
                Ok(WriteOutcome::Accepted)
            }
            // Reads are broken but writes are not: the switch lands, and we
            // simply cannot confirm it.
            FakeBehavior::Normal | FakeBehavior::ReadFails(_) => {
                m.current = Some(value);
                Ok(WriteOutcome::Accepted)
            }
        }
    }
}
