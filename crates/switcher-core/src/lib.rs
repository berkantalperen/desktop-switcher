//! Pure domain logic for desktop-switcher.
//!
//! Nothing in this crate talks to a monitor. Everything that does lives behind
//! the [`backend::MonitorBackend`] trait, so the domain rules that decide
//! whether a write is safe can be tested without hardware attached.

pub mod actions;
pub mod backend;
pub mod config;
pub mod edid;
pub mod eventlog;
pub mod fake;
pub mod inventory;
pub mod mccs;
pub mod proc;
pub mod switch;
pub mod types;

pub use actions::{Action, ActionStep};
pub use backend::{BackendError, BackendHealth, MonitorBackend};
pub use types::{
    DetectedMonitor, Evidence, InputCapabilities, InputCode, InputReading, InputSourceOption,
    MonitorHandle, MonitorIdentity, Transport, WriteOutcome,
};
