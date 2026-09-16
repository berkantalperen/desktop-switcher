//! Host-local configuration.
//!
//! Configuration is deliberately *not* portable between the two computers.
//! Backend ids, ports and even which input code means "Windows" differ per
//! host, so each machine keeps its own file.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{Evidence, InputCode};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no configuration file at {0}. Run `desktop-switcher configure` first.")]
    Missing(PathBuf),
    #[error("could not read {path}: {detail}")]
    Read { path: PathBuf, detail: String },
    #[error("could not write {path}: {detail}")]
    Write { path: PathBuf, detail: String },
    #[error("{path} is not valid TOML: {detail}")]
    Parse { path: PathBuf, detail: String },
    #[error("could not serialise configuration: {0}")]
    Serialise(String),
    #[error("could not determine a configuration directory for this user")]
    NoConfigDir,
    #[error("configuration is invalid: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    #[serde(rename = "powertoys-cli")]
    PowerToysCli,
    #[serde(rename = "ddcutil-cli")]
    DdcutilCli,
    /// In-memory backend, for tests and dry runs only.
    Fake,
}

// Two independent layers, deliberately not welded together:
//
//   Layer 1, the panel's input. Physical, one value, shared by both
//   computers. This is what `switch` changes over DDC.
//
//   Layer 2, desktop attachment. Local to one computer. Both computers can
//   have the same monitor attached at once -- which is precisely why a window
//   strands: one computer displays the panel while the other still has it in
//   its desktop and keeps drawing there.
//
// The settings below say what, if anything, layer 1 should do to layer 2.
// Default is nothing at all, and the `release`/`claim`/`sweep` commands drive
// layer 2 on their own.

/// What a `switch` should do to this computer's desktop when a monitor is
/// sent to the other computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AwayAction {
    /// Leave the desktop completely alone. Switching stays instant and
    /// nothing rearranges, but windows on that display become unreachable.
    #[default]
    Nothing,
    /// Move windows off the display but leave it attached. Nothing strands,
    /// and the desktop topology does not change.
    Sweep,
    /// Detach the display from this computer's desktop until it comes back.
    /// The OS relocates the windows itself, at the cost of a reflow each way.
    Release,
}

/// What a `switch` should do when a monitor comes back to this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HomeAction {
    /// Leave the desktop alone.
    #[default]
    Nothing,
    /// Reattach the display if this computer had released it.
    Claim,
}

impl fmt::Display for AwayAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AwayAction::Nothing => "nothing",
            AwayAction::Sweep => "sweep",
            AwayAction::Release => "release",
        })
    }
}

impl fmt::Display for HomeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HomeAction::Nothing => "nothing",
            HomeAction::Claim => "claim",
        })
    }
}

/// Layer-2 side effects a bare `switch` should apply. Both default to
/// nothing, so `switch` only ever changes the input unless asked otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SwitchSideEffects {
    #[serde(default)]
    pub away: AwayAction,
    #[serde(default)]
    pub home: HomeAction,
}

impl SwitchSideEffects {
    /// True when a bare `switch` should touch only the monitor input.
    pub fn is_input_only(&self) -> bool {
        self.away == AwayAction::Nothing && self.home == HomeAction::Nothing
    }
}

/// One verified mapping: "to put this monitor on that computer, write this code".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationMapping {
    pub input_code: InputCode,
    /// How this code was established. Only `user-confirmed` permits switching.
    #[serde(default)]
    pub verification: Evidence,
    /// ISO date the confirmation happened, for staleness review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// Stable human name used on the command line, e.g. `left`.
    pub logical_id: String,
    pub name: String,
    /// Backend handle recorded at configure time (a port-scoped identifier).
    pub backend_id: String,
    /// EDID serial recorded at configure time. This tracks the panel itself
    /// and is what a binding is actually checked against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// destination name -> verified input code.
    #[serde(default)]
    pub destinations: BTreeMap<String, DestinationMapping>,
}

impl MonitorConfig {
    pub fn mapping(&self, destination: &str) -> Option<&DestinationMapping> {
        self.destinations.get(destination)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    /// Free-form name for this computer, e.g. `alienware-windows`.
    pub host: String,
    pub backend: BackendKind,
    /// The destination name that refers to *this* computer. Used to decide
    /// whether a failed read-back is expected (we just switched away) or a
    /// real fault.
    pub self_destination: String,
    /// Explicit path to the backend executable. Discovery is attempted when
    /// this is absent, but a pinned path is preferred for shortcuts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_path: Option<PathBuf>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Pause after each write before attempting a read-back.
    #[serde(default = "default_settle_ms")]
    pub settle_ms: u64,
    /// Repeat presses of the same shortcut inside this window are ignored.
    #[serde(default = "default_dedupe_ms")]
    pub dedupe_ms: u64,
    /// Layer-2 side effects a bare `switch` applies. Defaults to none, so
    /// `switch` only ever changes the monitor input.
    #[serde(default)]
    pub on_switch: SwitchSideEffects,
    /// Logical ids in the order writes should be issued. Absent means
    /// declaration order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_order: Option<Vec<String>>,
    #[serde(default)]
    pub monitors: Vec<MonitorConfig>,
}

fn default_timeout_seconds() -> u64 {
    15
}
fn default_settle_ms() -> u64 {
    1200
}
fn default_dedupe_ms() -> u64 {
    1500
}

impl Config {
    pub fn new(
        host: impl Into<String>,
        backend: BackendKind,
        self_destination: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            host: host.into(),
            backend,
            self_destination: self_destination.into(),
            tool_path: None,
            timeout_seconds: default_timeout_seconds(),
            settle_ms: default_settle_ms(),
            dedupe_ms: default_dedupe_ms(),
            on_switch: SwitchSideEffects::default(),
            switch_order: None,
            monitors: Vec::new(),
        }
    }

    pub fn monitor(&self, logical_id: &str) -> Option<&MonitorConfig> {
        self.monitors.iter().find(|m| m.logical_id == logical_id)
    }

    pub fn monitor_mut(&mut self, logical_id: &str) -> Option<&mut MonitorConfig> {
        self.monitors
            .iter_mut()
            .find(|m| m.logical_id == logical_id)
    }

    /// Every destination named by any monitor, sorted.
    pub fn destinations(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .monitors
            .iter()
            .flat_map(|m| m.destinations.keys().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Monitors in the order writes should be issued.
    pub fn ordered_monitors(&self) -> Vec<&MonitorConfig> {
        match &self.switch_order {
            None => self.monitors.iter().collect(),
            Some(order) => {
                let mut out: Vec<&MonitorConfig> = Vec::new();
                for id in order {
                    if let Some(m) = self.monitor(id) {
                        out.push(m);
                    }
                }
                for m in &self.monitors {
                    if !out.iter().any(|x| x.logical_id == m.logical_id) {
                        out.push(m);
                    }
                }
                out
            }
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ConfigError::Invalid(format!(
                "schema_version is {}, this build understands {SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        if self.host.trim().is_empty() {
            return Err(ConfigError::Invalid("host must not be empty".into()));
        }
        if self.self_destination.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "self_destination must name the destination that means this computer".into(),
            ));
        }

        let mut seen_logical = BTreeMap::new();
        let mut seen_backend = BTreeMap::new();
        for m in &self.monitors {
            if m.logical_id.trim().is_empty() {
                return Err(ConfigError::Invalid("logical_id must not be empty".into()));
            }
            if seen_logical.insert(&m.logical_id, ()).is_some() {
                return Err(ConfigError::Invalid(format!(
                    "duplicate logical_id `{}`",
                    m.logical_id
                )));
            }
            if seen_backend.insert(&m.backend_id, ()).is_some() {
                return Err(ConfigError::Invalid(format!(
                    "two monitors share backend_id `{}`; re-run `configure`",
                    m.backend_id
                )));
            }
        }

        // Two entries claiming the same panel would make every binding ambiguous.
        let serials: Vec<&String> = self
            .monitors
            .iter()
            .filter_map(|m| m.serial.as_ref())
            .collect();
        for (i, a) in serials.iter().enumerate() {
            if serials[i + 1..].contains(a) {
                return Err(ConfigError::Invalid(format!(
                    "two monitors are configured with the same serial `{a}`; re-run `configure`"
                )));
            }
        }

        if let Some(order) = &self.switch_order {
            for id in order {
                if self.monitor(id).is_none() {
                    return Err(ConfigError::Invalid(format!(
                        "switch_order names unknown monitor `{id}`"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::Missing(path.to_path_buf()));
        }
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
        let config: Config = toml::from_str(&text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ConfigError::Write {
                path: dir.to_path_buf(),
                detail: e.to_string(),
            })?;
        }
        let text =
            toml::to_string_pretty(self).map_err(|e| ConfigError::Serialise(e.to_string()))?;
        // Write-then-rename so an interrupted save cannot truncate a working
        // configuration into an unusable one.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| ConfigError::Write {
            path: tmp.clone(),
            detail: e.to_string(),
        })?;
        std::fs::rename(&tmp, path).map_err(|e| ConfigError::Write {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
        Ok(())
    }
}

/// Per-user config directory for this application.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    directories::ProjectDirs::from("", "", "desktop-switcher")
        .map(|d| d.config_dir().to_path_buf())
        .ok_or(ConfigError::NoConfigDir)
}

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn default_state_dir() -> Result<PathBuf, ConfigError> {
    directories::ProjectDirs::from("", "", "desktop-switcher")
        .map(|d| d.data_local_dir().to_path_buf())
        .ok_or(ConfigError::NoConfigDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(code: &str, ev: Evidence) -> DestinationMapping {
        DestinationMapping {
            input_code: code.parse().unwrap(),
            verification: ev,
            verified_on: None,
            note: None,
        }
    }

    fn monitor(id: &str, backend: &str, serial: &str, win: &str, ubu: &str) -> MonitorConfig {
        let mut destinations = BTreeMap::new();
        destinations.insert("windows".into(), mapping(win, Evidence::UserConfirmed));
        destinations.insert("ubuntu".into(), mapping(ubu, Evidence::UserConfirmed));
        MonitorConfig {
            logical_id: id.into(),
            name: format!("AOC 27P2DG5 - {id}"),
            backend_id: backend.into(),
            serial: Some(serial.into()),
            model: Some("27P2DG5".into()),
            destinations,
        }
    }

    fn sample() -> Config {
        let mut c = Config::new("alienware-windows", BackendKind::PowerToysCli, "windows");
        c.monitors
            .push(monitor("left", "id-left", "SN-L", "0x11", "0x0F"));
        c.monitors
            .push(monitor("right", "id-right", "SN-R", "0x0F", "0x11"));
        c
    }

    #[test]
    fn round_trips_through_toml() {
        let c = sample();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(c, back);
        // Codes must survive as hex, not be reformatted into decimal.
        assert!(text.contains("0x11"), "{text}");
    }

    #[test]
    fn rejects_duplicate_logical_ids() {
        let mut c = sample();
        c.monitors[1].logical_id = "left".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_two_monitors_claiming_one_panel() {
        let mut c = sample();
        c.monitors[1].serial = Some("SN-L".into());
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("same serial"), "{err}");
    }

    #[test]
    fn rejects_duplicate_backend_ids() {
        let mut c = sample();
        c.monitors[1].backend_id = "id-left".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let mut c = sample();
        c.schema_version = 99;
        assert!(c.validate().is_err());
    }

    #[test]
    fn switch_order_controls_sequence_and_keeps_unlisted_monitors() {
        let mut c = sample();
        c.switch_order = Some(vec!["right".into()]);
        let ids: Vec<&str> = c
            .ordered_monitors()
            .iter()
            .map(|m| m.logical_id.as_str())
            .collect();
        assert_eq!(ids, vec!["right", "left"]);
    }

    #[test]
    fn switch_order_naming_an_unknown_monitor_is_rejected() {
        let mut c = sample();
        c.switch_order = Some(vec!["middle".into()]);
        assert!(c.validate().is_err());
    }

    #[test]
    fn the_default_policy_leaves_the_desktop_completely_alone() {
        let effects = SwitchSideEffects::default();
        assert!(effects.is_input_only());
        assert_eq!(effects.away, AwayAction::Nothing);
        assert_eq!(effects.home, HomeAction::Nothing);
    }

    #[test]
    fn the_two_layers_are_configured_independently() {
        // Releasing on the way out without claiming on the way back is a
        // legitimate combination, so neither field may imply the other.
        let release_only = SwitchSideEffects {
            away: AwayAction::Release,
            home: HomeAction::Nothing,
        };
        assert!(!release_only.is_input_only());

        let claim_only = SwitchSideEffects {
            away: AwayAction::Nothing,
            home: HomeAction::Claim,
        };
        assert!(!claim_only.is_input_only());
    }

    #[test]
    fn side_effects_round_trip_through_toml() {
        let mut c = sample();
        c.on_switch = SwitchSideEffects {
            away: AwayAction::Release,
            home: HomeAction::Claim,
        };
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("away = \"release\""), "{text}");
        assert!(text.contains("home = \"claim\""), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
    }

    #[test]
    fn every_away_action_has_a_distinct_name() {
        for (action, word) in [
            (AwayAction::Nothing, "nothing"),
            (AwayAction::Sweep, "sweep"),
            (AwayAction::Release, "release"),
        ] {
            assert_eq!(action.to_string(), word);
        }
        assert_eq!(HomeAction::Claim.to_string(), "claim");
    }

    #[test]
    fn a_config_without_the_setting_still_loads_and_changes_nothing() {
        // Files written before this setting existed must keep working, and
        // must not suddenly start rearranging someone's desktop.
        let toml_text = r#"
schema_version = 1
host = "h"
backend = "fake"
self_destination = "windows"
"#;
        let config: Config = toml::from_str(toml_text).unwrap();
        assert!(config.on_switch.is_input_only());
    }

    #[test]
    fn destinations_are_collected_across_monitors() {
        assert_eq!(sample().destinations(), vec!["ubuntu", "windows"]);
    }

    #[test]
    fn bare_decimal_input_code_in_toml_is_rejected() {
        let toml_text = r#"
schema_version = 1
host = "h"
backend = "powertoys-cli"
self_destination = "windows"

[[monitors]]
logical_id = "left"
name = "left"
backend_id = "b"

[monitors.destinations.windows]
input_code = "11"
verification = "user-confirmed"
"#;
        let err = toml::from_str::<Config>(toml_text).unwrap_err().to_string();
        assert!(err.contains("0x"), "{err}");
    }
}
