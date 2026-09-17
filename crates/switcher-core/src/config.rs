//! Host-local configuration.
//!
//! The model is deliberately narrow: **monitors and their inputs**. Nothing
//! here knows what is plugged into an input, and nothing should. A monitor
//! input is a fact about the panel; "that one is the laptop" is a fact about
//! one person's desk, and the place to record it is the *name they give an
//! action*, not the vocabulary of the tool.
//!
//! That keeps the tool usable on any machine with any number of computers
//! attached, and means a rearranged desk is a rename rather than a
//! reinstall.
//!
//! Configuration is not portable between computers: backend ids and the
//! physical wiring differ per host, so each machine keeps its own file.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::actions::{Action, ActionStep};
use crate::types::{Evidence, InputCode};

/// Schema 2 dropped the idea of a "destination computer"; see the module note.
pub const SCHEMA_VERSION: u32 = 2;

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

/// One input on one monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInput {
    pub input_code: InputCode,
    /// What to call it. Defaults to the monitor's own name for the input
    /// (`HDMI-1`), but this is the natural place for "Laptop" or "Desktop" —
    /// a label chosen by the person, not a concept the tool understands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How well this input is known to work. `reported` means the monitor
    /// listed it; only a real switch that someone watched makes it
    /// `user-confirmed`.
    #[serde(default)]
    pub verification: Evidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl MonitorInput {
    pub fn reported(code: InputCode, label: Option<String>) -> Self {
        Self {
            input_code: code,
            label,
            verification: Evidence::Reported,
            note: None,
        }
    }

    /// The name to show, falling back to the MCCS name and then the raw code.
    pub fn display_name(&self) -> String {
        match &self.label {
            Some(l) if !l.trim().is_empty() => l.clone(),
            _ => self
                .input_code
                .standard_label()
                .map(String::from)
                .unwrap_or_else(|| self.input_code.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// Stable identity, used to name this monitor on the command line and in
    /// actions. The EDID serial when the panel publishes one, because that
    /// follows the panel rather than the port.
    pub key: String,
    /// What to call it, e.g. `AOC 27P2DG5 (left)`. Free text.
    pub label: String,
    /// Backend handle recorded at configure time; a port-scoped identifier.
    pub backend_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The inputs this monitor offers.
    #[serde(default)]
    pub inputs: Vec<MonitorInput>,
}

impl MonitorConfig {
    pub fn input(&self, code: InputCode) -> Option<&MonitorInput> {
        self.inputs.iter().find(|i| i.input_code == code)
    }

    pub fn input_mut(&mut self, code: InputCode) -> Option<&mut MonitorInput> {
        self.inputs.iter_mut().find(|i| i.input_code == code)
    }

    /// Match a user-typed name against this monitor's key, label or serial.
    pub fn matches(&self, name: &str) -> bool {
        let name = name.trim();
        self.key.eq_ignore_ascii_case(name)
            || self.label.eq_ignore_ascii_case(name)
            || self
                .serial
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    /// This machine's name. Display only; filled from the system hostname.
    pub host: String,
    pub backend: BackendKind,
    /// Explicit path to the backend executable. Discovery is attempted when
    /// absent, but a pinned path is preferred for shortcuts, which run with a
    /// different environment than a shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_path: Option<PathBuf>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Pause after a write before attempting a read-back.
    #[serde(default = "default_settle_ms")]
    pub settle_ms: u64,
    /// Repeats of the same request inside this window are ignored, so a held
    /// shortcut key does not issue a burst of writes.
    #[serde(default = "default_dedupe_ms")]
    pub dedupe_ms: u64,
    #[serde(default)]
    pub monitors: Vec<MonitorConfig>,
    /// Named sequences of steps, each optionally bound to a hotkey. This is
    /// where a person records what a combination of inputs *means* to them.
    #[serde(default)]
    pub actions: Vec<Action>,
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
    pub fn new(host: impl Into<String>, backend: BackendKind) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            host: host.into(),
            backend,
            tool_path: None,
            timeout_seconds: default_timeout_seconds(),
            settle_ms: default_settle_ms(),
            dedupe_ms: default_dedupe_ms(),
            monitors: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// Find a monitor by key, label or serial.
    pub fn monitor(&self, name: &str) -> Option<&MonitorConfig> {
        self.monitors.iter().find(|m| m.matches(name))
    }

    pub fn monitor_mut(&mut self, name: &str) -> Option<&mut MonitorConfig> {
        self.monitors.iter_mut().find(|m| m.matches(name))
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ConfigError::Invalid(format!(
                "schema_version is {}, this build understands {SCHEMA_VERSION}",
                self.schema_version
            )));
        }

        let mut seen_keys: BTreeMap<String, ()> = BTreeMap::new();
        let mut seen_backend: BTreeMap<&String, ()> = BTreeMap::new();
        for monitor in &self.monitors {
            if monitor.key.trim().is_empty() {
                return Err(ConfigError::Invalid("a monitor has no key".into()));
            }
            if seen_keys
                .insert(monitor.key.to_ascii_lowercase(), ())
                .is_some()
            {
                return Err(ConfigError::Invalid(format!(
                    "two monitors share the key `{}`",
                    monitor.key
                )));
            }
            if seen_backend.insert(&monitor.backend_id, ()).is_some() {
                return Err(ConfigError::Invalid(format!(
                    "two monitors share the connection `{}`; re-run `configure`",
                    monitor.backend_id
                )));
            }
            let mut codes = Vec::new();
            for input in &monitor.inputs {
                if codes.contains(&input.input_code) {
                    return Err(ConfigError::Invalid(format!(
                        "`{}` lists input {} twice",
                        monitor.label, input.input_code
                    )));
                }
                codes.push(input.input_code);
            }
        }

        // An action naming a monitor that no longer exists would fail at the
        // worst moment — when a hotkey is pressed — so catch it on save.
        for action in &self.actions {
            for step in &action.steps {
                if let Some(name) = step.monitor_name() {
                    if !name.is_empty() && self.monitor(name).is_none() {
                        return Err(ConfigError::Invalid(format!(
                            "action `{}` refers to monitor `{name}`, which is not configured",
                            action.name
                        )));
                    }
                }
            }
        }

        crate::actions::validate_all(&self.actions).map_err(ConfigError::Invalid)?;
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
        let config = Self::from_toml(&text).map_err(|detail| ConfigError::Parse {
            path: path.to_path_buf(),
            detail,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Parse, upgrading an older schema on the way through.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
        let version = value
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .unwrap_or(SCHEMA_VERSION as i64);

        if version == 1 {
            return migrate_v1(&value);
        }
        value.try_into::<Config>().map_err(|e| e.to_string())
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
        // Write-then-rename, so an interrupted save cannot truncate a working
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

/// Upgrade a schema-1 file, which was organised around "destination
/// computers".
///
/// Each destination becomes an *action* of the same name. That is the whole
/// conceptual shift made concrete: the tool stops believing in computers, and
/// what used to be built-in vocabulary becomes a label the user owns and can
/// rename or delete.
fn migrate_v1(value: &toml::Value) -> Result<Config, String> {
    let host = value
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("this-computer")
        .to_string();
    let backend = value
        .get("backend")
        .cloned()
        .map(|b| b.try_into::<BackendKind>())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(BackendKind::Fake);

    let mut config = Config::new(host, backend);
    for key in ["timeout_seconds", "settle_ms", "dedupe_ms"] {
        if let Some(n) = value.get(key).and_then(|v| v.as_integer()) {
            match key {
                "timeout_seconds" => config.timeout_seconds = n as u64,
                "settle_ms" => config.settle_ms = n as u64,
                _ => config.dedupe_ms = n as u64,
            }
        }
    }
    if let Some(p) = value.get("tool_path").and_then(|v| v.as_str()) {
        config.tool_path = Some(PathBuf::from(p));
    }

    // destination name -> steps, so the old groupings survive as actions.
    let mut grouped: BTreeMap<String, Vec<ActionStep>> = BTreeMap::new();

    let monitors = value
        .get("monitors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for raw in monitors {
        let logical_id = raw
            .get("logical_id")
            .and_then(|v| v.as_str())
            .unwrap_or("monitor");
        let serial = raw
            .get("serial")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let backend_id = raw
            .get("backend_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let model = raw
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let name = raw
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(logical_id);

        // Prefer the serial as the key, since it follows the panel.
        let key = serial.clone().unwrap_or_else(|| backend_id.clone());

        let mut inputs = Vec::new();
        if let Some(destinations) = raw.get("destinations").and_then(|v| v.as_table()) {
            for (destination, mapping) in destinations {
                let Some(code) = mapping
                    .get("input_code")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<InputCode>().ok())
                else {
                    continue;
                };
                let verification = mapping
                    .get("verification")
                    .cloned()
                    .and_then(|v| v.try_into::<Evidence>().ok())
                    .unwrap_or_default();

                inputs.push(MonitorInput {
                    input_code: code,
                    // The old destination name becomes the input's label:
                    // still the user's word, no longer the tool's concept.
                    label: Some(destination.clone()),
                    verification,
                    note: mapping
                        .get("note")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });

                grouped
                    .entry(destination.clone())
                    .or_default()
                    .push(ActionStep::SetInput {
                        monitor: key.clone(),
                        code,
                    });
            }
        }
        inputs.sort_by_key(|i| i.input_code);

        config.monitors.push(MonitorConfig {
            key,
            label: name.to_string(),
            backend_id,
            serial,
            model,
            inputs,
        });
    }

    for (name, steps) in grouped {
        config.actions.push(Action {
            name,
            hotkey: None,
            steps,
        });
    }

    // Carry over any actions the file already had.
    if let Some(existing) = value.get("actions").and_then(|v| v.as_array()) {
        for raw in existing {
            if let Ok(action) = raw.clone().try_into::<Action>() {
                if !config
                    .actions
                    .iter()
                    .any(|a| a.name.eq_ignore_ascii_case(&action.name))
                {
                    config.actions.push(action);
                }
            }
        }
    }

    Ok(config)
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

impl fmt::Display for MonitorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.label, self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(code: &str, label: &str) -> MonitorInput {
        MonitorInput {
            input_code: code.parse().unwrap(),
            label: Some(label.into()),
            verification: Evidence::UserConfirmed,
            note: None,
        }
    }

    fn monitor(key: &str, backend: &str) -> MonitorConfig {
        MonitorConfig {
            key: key.into(),
            label: format!("AOC 27P2DG5 ({key})"),
            backend_id: backend.into(),
            serial: Some(key.into()),
            model: Some("27P2DG5".into()),
            inputs: vec![input("0x11", "HDMI-1"), input("0x0F", "DisplayPort-1")],
        }
    }

    fn sample() -> Config {
        let mut c = Config::new("test-host", BackendKind::Fake);
        c.monitors.push(monitor("SN-LEFT", "port-a"));
        c.monitors.push(monitor("SN-RIGHT", "port-b"));
        c
    }

    #[test]
    fn round_trips_through_toml() {
        let c = sample();
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), c);
        // Codes stay hex rather than being reformatted into decimal.
        assert!(text.contains("0x11"), "{text}");
    }

    #[test]
    fn nothing_in_the_schema_names_a_computer() {
        let text = toml::to_string_pretty(&sample()).unwrap();
        for forbidden in ["destination", "self_destination", "windows", "ubuntu"] {
            assert!(
                !text.to_lowercase().contains(forbidden),
                "schema still mentions `{forbidden}`:\n{text}"
            );
        }
    }

    #[test]
    fn monitors_can_be_named_by_key_label_or_serial() {
        let c = sample();
        assert!(c.monitor("SN-LEFT").is_some());
        assert!(c.monitor("sn-left").is_some());
        assert!(c.monitor("AOC 27P2DG5 (SN-LEFT)").is_some());
        assert!(c.monitor("nope").is_none());
    }

    #[test]
    fn duplicate_keys_and_connections_are_rejected() {
        let mut c = sample();
        c.monitors[1].key = "SN-LEFT".into();
        assert!(c.validate().is_err());

        let mut c = sample();
        c.monitors[1].backend_id = "port-a".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn a_monitor_cannot_list_the_same_input_twice() {
        let mut c = sample();
        c.monitors[0].inputs.push(input("0x11", "HDMI again"));
        assert!(c.validate().unwrap_err().to_string().contains("twice"));
    }

    #[test]
    fn an_action_naming_an_unknown_monitor_is_rejected_on_save() {
        // Better here than when a hotkey is pressed.
        let mut c = sample();
        c.actions.push(Action {
            name: "Broken".into(),
            hotkey: None,
            steps: vec![ActionStep::SetInput {
                monitor: "SN-GONE".into(),
                code: "0x11".parse().unwrap(),
            }],
        });
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("SN-GONE"), "{err}");
    }

    #[test]
    fn input_names_fall_back_sensibly() {
        let labelled = input("0x11", "Laptop");
        assert_eq!(labelled.display_name(), "Laptop");

        let unlabelled = MonitorInput::reported("0x0F".parse().unwrap(), None);
        assert_eq!(unlabelled.display_name(), "DisplayPort-1");

        let unknown = MonitorInput::reported("0x42".parse().unwrap(), None);
        assert_eq!(unknown.display_name(), "0x42");
    }

    /// The old schema was organised around destination computers. Upgrading
    /// has to keep the verified codes — they cost a human watching a screen —
    /// and turn each destination into an action the user now owns.
    #[test]
    fn a_schema_1_file_upgrades_into_monitors_and_actions() {
        let v1 = r#"
schema_version = 1
host = "BERKANT16X"
backend = "powertoys-cli"
self_destination = "windows"
timeout_seconds = 20
settle_ms = 900

[[monitors]]
logical_id = "main"
name = "AOC 27P2DG5"
backend_id = "\\?\\DISPLAY#AOC2702#UID4613"
serial = "ASFPA9A001108"
model = "27P2DG5"

[monitors.destinations.windows]
input_code = "0x11"
verification = "user-confirmed"

[monitors.destinations.ubuntu]
input_code = "0x0F"
verification = "user-confirmed"
"#;
        let config = Config::from_toml(v1).unwrap();
        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert_eq!(config.timeout_seconds, 20);
        assert_eq!(config.settle_ms, 900);

        // The panel is keyed by serial, and keeps both verified inputs.
        assert_eq!(config.monitors.len(), 1);
        let monitor = &config.monitors[0];
        assert_eq!(monitor.key, "ASFPA9A001108");
        assert_eq!(monitor.inputs.len(), 2);
        assert!(monitor
            .inputs
            .iter()
            .all(|i| i.verification == Evidence::UserConfirmed));

        // Each old destination survives as an action the user can rename.
        let names: Vec<&str> = config.actions.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"windows"), "{names:?}");
        assert!(names.contains(&"ubuntu"), "{names:?}");

        config.validate().unwrap();
    }

    #[test]
    fn an_upgraded_config_still_validates_and_saves() {
        let v1 = r#"
schema_version = 1
host = "h"
backend = "fake"
self_destination = "a"

[[monitors]]
logical_id = "one"
name = "One"
backend_id = "b1"
serial = "S1"

[monitors.destinations.a]
input_code = "0x11"
verification = "user-confirmed"
"#;
        let config = Config::from_toml(v1).unwrap();
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), config);
    }

    #[test]
    fn an_unknown_future_schema_is_refused_rather_than_guessed_at() {
        let mut c = sample();
        c.schema_version = 99;
        assert!(c.validate().is_err());
    }
}
