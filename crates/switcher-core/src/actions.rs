//! User-defined actions: a name, a sequence of steps, and an optional hotkey.
//!
//! The two layers stay separate everywhere else in this project, which is
//! right for correctness but tedious in daily use: "everything to the desk
//! machine" is one intention and several commands. An action is where a
//! person composes those into the thing they actually mean, names it in their
//! own words, and binds a key to it.
//!
//! Steps run in order and the outcome of each is reported. A failing step
//! stops the rest by default, because the later steps usually assume the
//! earlier ones happened — sweeping a monitor is pointless if the switch that
//! was meant to free it never occurred.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::types::InputCode;

/// One thing an action does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ActionStep {
    /// Set one monitor to one of its inputs.
    ///
    /// The unit of work is a single monitor, because that is the only thing
    /// the hardware actually offers. "Everything to the desktop" is a person's
    /// idea, and it is expressed by putting several of these in one action.
    SetInput { monitor: String, code: InputCode },
    /// Flip one monitor between two inputs: to the second if it reports the
    /// first, otherwise to the first.
    ///
    /// Only ever one of the two, so one key can alternate a monitor between
    /// the two computers on it without ever landing on a third input. See
    /// [`crate::switch::toggle_target`] for how the monitor's report is used.
    ToggleInput {
        monitor: String,
        between: [InputCode; 2],
    },
    /// Move windows off a monitor without detaching it.
    Sweep {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<String>,
    },
    /// Put swept windows back where they were.
    RestoreWindows {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<String>,
    },
    /// Detach a monitor from this desktop (layer 2, experimental).
    Release {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<String>,
    },
    /// Reattach a monitor this computer released (layer 2, experimental).
    Claim {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<String>,
    },
    /// Make a monitor the primary display (layer 2, experimental).
    Primary {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<String>,
    },
    /// Run any program. Arguments are an explicit list, never a shell string,
    /// so nothing here is parsed by a shell.
    Run {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl ActionStep {
    /// Whether this step needs the `--experimental` opt-in.
    pub fn is_experimental(&self) -> bool {
        matches!(
            self,
            ActionStep::Release { .. } | ActionStep::Claim { .. } | ActionStep::Primary { .. }
        )
    }

    /// The monitor this step names, if it names one.
    ///
    /// Used to check an action against the configured monitors before it is
    /// saved, rather than when a hotkey is pressed.
    pub fn monitor_name(&self) -> Option<&str> {
        match self {
            ActionStep::SetInput { monitor, .. } | ActionStep::ToggleInput { monitor, .. } => {
                Some(monitor)
            }
            ActionStep::Sweep { monitor }
            | ActionStep::RestoreWindows { monitor }
            | ActionStep::Release { monitor }
            | ActionStep::Claim { monitor }
            | ActionStep::Primary { monitor } => monitor.as_deref(),
            ActionStep::Run { .. } => None,
        }
    }

    pub fn summary(&self) -> String {
        let target = |m: &Option<String>| m.clone().unwrap_or_else(|| "the only monitor".into());
        match self {
            ActionStep::SetInput { monitor, code } => format!("set {monitor} to {code}"),
            ActionStep::ToggleInput { monitor, between } => {
                format!("toggle {monitor} between {} and {}", between[0], between[1])
            }
            ActionStep::Sweep { monitor } => format!("sweep windows off {}", target(monitor)),
            ActionStep::RestoreWindows { monitor } => {
                format!("restore windows onto {}", target(monitor))
            }
            ActionStep::Release { monitor } => format!("release {}", target(monitor)),
            ActionStep::Claim { monitor } => format!("claim {}", target(monitor)),
            ActionStep::Primary { monitor } => format!("make {} primary", target(monitor)),
            ActionStep::Run { command, args } => {
                if args.is_empty() {
                    format!("run {command}")
                } else {
                    format!("run {command} {}", args.join(" "))
                }
            }
        }
    }
}

impl fmt::Display for ActionStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

/// A named sequence of steps, optionally bound to a hotkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub name: String,
    /// Hotkey in the form the shortcut installers understand, e.g.
    /// `CTRL+ALT+2`. Absent means the action exists but is not bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    #[serde(default)]
    pub steps: Vec<ActionStep>,
}

impl Action {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hotkey: None,
            steps: Vec::new(),
        }
    }

    /// A filesystem- and identifier-safe form of the name.
    ///
    /// Used for shortcut filenames and GNOME keybinding slots, so it must be
    /// stable and must never be empty.
    pub fn slug(&self) -> String {
        let mut slug = String::new();
        let mut last_dash = true;
        for ch in self.name.chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch.to_ascii_lowercase());
                last_dash = false;
            } else if !last_dash {
                slug.push('-');
                last_dash = true;
            }
        }
        let trimmed = slug.trim_matches('-').to_string();
        if trimmed.is_empty() {
            "action".to_string()
        } else {
            trimmed
        }
    }

    pub fn needs_experimental(&self) -> bool {
        self.steps.iter().any(ActionStep::is_experimental)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("an action needs a name".into());
        }
        if self.steps.is_empty() {
            return Err(format!(
                "`{}` has no steps, so it would do nothing",
                self.name
            ));
        }
        for step in &self.steps {
            if let ActionStep::SetInput { monitor, .. } = step {
                if monitor.trim().is_empty() {
                    return Err(format!(
                        "`{}` has a set-input step with no monitor",
                        self.name
                    ));
                }
            }
            if let ActionStep::ToggleInput { monitor, between } = step {
                if monitor.trim().is_empty() {
                    return Err(format!("`{}` has a toggle step with no monitor", self.name));
                }
                if between[0] == between[1] {
                    return Err(format!(
                        "`{}` toggles {monitor} between {} and itself, so it would never \
                         change anything",
                        self.name, between[0]
                    ));
                }
            }
            if let ActionStep::Run { command, .. } = step {
                if command.trim().is_empty() {
                    return Err(format!("`{}` has a run step with no command", self.name));
                }
            }
        }
        Ok(())
    }
}

/// Check a whole set of actions for problems that only show up together.
pub fn validate_all(actions: &[Action]) -> Result<(), String> {
    for action in actions {
        action.validate()?;
    }
    for (i, a) in actions.iter().enumerate() {
        for b in &actions[i + 1..] {
            if a.name.eq_ignore_ascii_case(&b.name) {
                return Err(format!("two actions are both called `{}`", a.name));
            }
            if a.slug() == b.slug() {
                return Err(format!(
                    "`{}` and `{}` reduce to the same short name `{}`, which shortcut files \
                     cannot tell apart",
                    a.name,
                    b.name,
                    a.slug()
                ));
            }
            match (&a.hotkey, &b.hotkey) {
                (Some(x), Some(y)) if normalise_hotkey(x) == normalise_hotkey(y) => {
                    return Err(format!(
                        "`{}` and `{}` are both bound to {x}",
                        a.name, b.name
                    ))
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Compare hotkeys ignoring case and modifier order, so `Ctrl+Alt+1` and
/// `ALT+CTRL+1` are recognised as the same binding rather than silently
/// fighting each other.
pub fn normalise_hotkey(hotkey: &str) -> String {
    let mut parts: Vec<String> = hotkey
        .split('+')
        .map(|p| p.trim().to_ascii_uppercase())
        .filter(|p| !p.is_empty())
        .collect();
    parts.sort();
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_input(monitor: &str, code: &str) -> ActionStep {
        ActionStep::SetInput {
            monitor: monitor.into(),
            code: code.parse().unwrap(),
        }
    }

    fn action(name: &str, hotkey: Option<&str>) -> Action {
        Action {
            name: name.into(),
            hotkey: hotkey.map(String::from),
            steps: vec![set_input("SN-1", "0x11")],
        }
    }

    /// Actions always live inside the configuration table, never as a
    /// top-level TOML array, so the round trip is tested the same way.
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Wrapper {
        actions: Vec<Action>,
    }

    #[test]
    fn steps_round_trip_through_toml() {
        let wrapper = Wrapper {
            actions: vec![Action {
                name: "Desk setup".into(),
                hotkey: Some("CTRL+ALT+2".into()),
                steps: vec![
                    set_input("SN-1", "0x11"),
                    ActionStep::Sweep {
                        monitor: Some("main".into()),
                    },
                    ActionStep::Run {
                        command: "notepad.exe".into(),
                        args: vec!["notes.txt".into()],
                    },
                ],
            }],
        };
        let text = toml::to_string_pretty(&wrapper).unwrap();
        assert!(text.contains("kind = \"set-input\""), "{text}");
        assert_eq!(toml::from_str::<Wrapper>(&text).unwrap(), wrapper);
    }

    /// Written the way a person would write it in config.toml.
    #[test]
    fn a_toggle_reads_from_the_configuration() {
        let text = r#"
[[actions]]
name = "Toggle center"
hotkey = "F24"

[[actions.steps]]
kind = "toggle-input"
monitor = "ASFPA9A001108"
between = ["0x11", "0x0F"]
"#;
        let wrapper: Wrapper = toml::from_str(text).unwrap();
        let step = &wrapper.actions[0].steps[0];
        assert_eq!(
            step,
            &ActionStep::ToggleInput {
                monitor: "ASFPA9A001108".into(),
                between: [InputCode(0x11), InputCode(0x0F)],
            }
        );
        assert_eq!(step.monitor_name(), Some("ASFPA9A001108"));
        assert_eq!(step.summary(), "toggle ASFPA9A001108 between 0x11 and 0x0F");
        let again = toml::to_string_pretty(&wrapper).unwrap();
        assert_eq!(toml::from_str::<Wrapper>(&again).unwrap(), wrapper);
    }

    #[test]
    fn a_toggle_between_an_input_and_itself_is_rejected() {
        let mut a = action("Pointless", None);
        a.steps = vec![ActionStep::ToggleInput {
            monitor: "SN-1".into(),
            between: [InputCode(0x11), InputCode(0x11)],
        }];
        assert!(a.validate().unwrap_err().contains("itself"));
    }

    #[test]
    fn an_action_with_no_steps_is_rejected() {
        let mut a = action("Empty", None);
        a.steps.clear();
        assert!(a.validate().unwrap_err().contains("no steps"));
    }

    #[test]
    fn a_switch_without_a_destination_is_rejected() {
        let a = Action {
            name: "Broken".into(),
            hotkey: None,
            steps: vec![set_input("  ", "0x11")],
        };
        assert!(a.validate().is_err());
    }

    #[test]
    fn duplicate_names_are_rejected_case_insensitively() {
        let actions = vec![action("Go", None), action("go", None)];
        assert!(validate_all(&actions).unwrap_err().contains("both called"));
    }

    #[test]
    fn two_actions_cannot_share_a_hotkey_even_written_differently() {
        // Windows would silently let one win, so catch it here instead.
        let actions = vec![
            action("First", Some("CTRL+ALT+1")),
            action("Second", Some("Alt+Ctrl+1")),
        ];
        assert!(validate_all(&actions).unwrap_err().contains("bound to"));
    }

    #[test]
    fn different_hotkeys_are_fine() {
        let actions = vec![
            action("First", Some("CTRL+ALT+1")),
            action("Second", Some("CTRL+ALT+2")),
        ];
        assert!(validate_all(&actions).is_ok());
    }

    #[test]
    fn names_that_collapse_to_the_same_slug_are_rejected() {
        // Both would want the same shortcut filename.
        let actions = vec![action("Go Desk", None), action("go/desk!", None)];
        assert!(validate_all(&actions).unwrap_err().contains("short name"));
    }

    #[test]
    fn slugs_are_safe_and_never_empty() {
        assert_eq!(Action::new("Desk setup").slug(), "desk-setup");
        assert_eq!(Action::new("  Desk!!  ").slug(), "desk");
        assert_eq!(Action::new("///").slug(), "action");
        assert_eq!(Action::new("Türkçe ada").slug(), "t-rk-e-ada");
    }

    #[test]
    fn experimental_steps_are_flagged_so_they_can_be_gated() {
        let safe = Action {
            name: "Safe".into(),
            hotkey: None,
            steps: vec![
                set_input("SN-1", "0x11"),
                ActionStep::Sweep { monitor: None },
            ],
        };
        assert!(!safe.needs_experimental());

        let risky = Action {
            name: "Risky".into(),
            hotkey: None,
            steps: vec![
                set_input("SN-1", "0x11"),
                ActionStep::Release { monitor: None },
            ],
        };
        assert!(risky.needs_experimental());
    }

    #[test]
    fn steps_describe_themselves_readably() {
        assert_eq!(set_input("SN-1", "0x11").summary(), "set SN-1 to 0x11");
        assert_eq!(
            ActionStep::Sweep {
                monitor: Some("main".into())
            }
            .summary(),
            "sweep windows off main"
        );
        assert_eq!(
            ActionStep::Run {
                command: "x.exe".into(),
                args: vec!["-a".into()]
            }
            .summary(),
            "run x.exe -a"
        );
    }

    #[test]
    fn hotkey_comparison_ignores_order_and_case() {
        assert_eq!(
            normalise_hotkey("ctrl+alt+1"),
            normalise_hotkey("ALT+CTRL+1")
        );
        assert_ne!(
            normalise_hotkey("CTRL+ALT+1"),
            normalise_hotkey("CTRL+ALT+2")
        );
    }
}
