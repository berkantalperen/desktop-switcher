//! The narrow seam between domain logic and the two CLI tools.

use std::fmt;
use std::time::Duration;

use crate::proc::ProcError;
use crate::types::{
    DetectedMonitor, InputCapabilities, InputCode, InputReading, MonitorHandle, WriteOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    #[error("{tool} was not found. {hint}")]
    ToolNotFound { tool: String, hint: String },
    #[error("{tool} is installed but not usable right now: {detail}")]
    ToolNotReady { tool: String, detail: String },
    #[error("permission denied: {detail}")]
    PermissionDenied { detail: String },
    #[error("`{command}` timed out after {}s", after.as_secs_f32())]
    Timeout { command: String, after: Duration },
    #[error("`{command}` failed (exit {status:?}): {stderr}")]
    CommandFailed {
        command: String,
        status: Option<i32>,
        stderr: String,
    },
    /// The tool ran, but printed something this adapter does not understand.
    /// This is deliberately fatal: guessing at changed output is how you write
    /// the right value to the wrong monitor.
    #[error("unexpected output from `{command}`: {detail}")]
    UnexpectedOutput {
        command: String,
        detail: String,
        raw: String,
    },
    #[error("no monitor matched: {detail}")]
    MonitorNotFound { detail: String },
    #[error("{detail}")]
    Unsupported { detail: String },
    #[error("{0}")]
    Io(String),
}

impl BackendError {
    /// Whether retrying could plausibly help.
    ///
    /// Only ever consulted on read paths. An input-switch write is never
    /// retried on this basis: the first write may already have been accepted,
    /// and repeating it during re-enumeration compounds the disconnect.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            BackendError::Timeout { .. } | BackendError::CommandFailed { .. }
        )
    }

    pub fn raw_evidence(&self) -> Option<&str> {
        match self {
            BackendError::UnexpectedOutput { raw, .. } => Some(raw),
            _ => None,
        }
    }
}

impl From<ProcError> for BackendError {
    fn from(e: ProcError) -> Self {
        match e {
            ProcError::NotFound(p) => BackendError::ToolNotFound {
                tool: p.display().to_string(),
                hint: "Check the install, or pass an explicit path in config.toml.".to_string(),
            },
            ProcError::Timeout { program, timeout } => BackendError::Timeout {
                command: program,
                after: timeout,
            },
            other => BackendError::Io(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    /// Stops the backend from being usable at all.
    Blocker,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Info => "ok",
            Severity::Warning => "warn",
            Severity::Blocker => "BLOCKER",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFinding {
    pub severity: Severity,
    pub message: String,
    /// What the user should do about it, when there is something to do.
    pub remedy: Option<String>,
}

impl HealthFinding {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            message: message.into(),
            remedy: None,
        }
    }
    pub fn warn(message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            remedy: Some(remedy.into()),
        }
    }
    pub fn blocker(message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            severity: Severity::Blocker,
            message: message.into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// Read-only report on whether this backend can do anything at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendHealth {
    pub backend: String,
    pub tool_path: Option<String>,
    pub tool_version: Option<String>,
    pub findings: Vec<HealthFinding>,
}

impl BackendHealth {
    pub fn is_usable(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|f| f.severity == Severity::Blocker)
    }
}

/// A source of monitor information and input-source control.
///
/// Implementations must never fall back to a different monitor than the one
/// asked for. If a handle cannot be resolved exactly, return
/// [`BackendError::MonitorNotFound`].
pub trait MonitorBackend {
    fn name(&self) -> &str;

    /// Read-only checks: is the tool present, the right version, permitted to
    /// talk to the hardware. Must not write any VCP value.
    fn health(&self) -> BackendHealth;

    /// Enumerate displays. Must not write any VCP value.
    fn discover(&self) -> Result<Vec<DetectedMonitor>, BackendError>;

    /// What the monitor claims about feature 0x60. Must not write.
    fn input_capabilities(
        &self,
        monitor: &MonitorHandle,
    ) -> Result<InputCapabilities, BackendError>;

    /// Current value of feature 0x60. Must not write.
    fn read_input(&self, monitor: &MonitorHandle) -> Result<InputReading, BackendError>;

    /// Write feature 0x60. The only method in this trait that changes anything.
    fn set_input(
        &self,
        monitor: &MonitorHandle,
        value: InputCode,
    ) -> Result<WriteOutcome, BackendError>;
}
