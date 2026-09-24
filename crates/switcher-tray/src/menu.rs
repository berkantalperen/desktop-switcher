//! What the tray menu offers, built from the configuration.
//!
//! Pure, so the rules are tested on every platform, and shared by the Windows
//! and Linux trays: both show exactly the same menu. The model keeps an item's
//! label and its detail (a hotkey, an input code) apart, because each platform
//! lays them out and escapes them differently; `windows_text` and `dbus_text`
//! do that last step.
//!
//! The menu is rebuilt from the configuration each time it opens, so a change
//! made in the GUI shows up on the next click with nothing to restart.

use switcher_core::config::{Config, MonitorConfig, MonitorInput};

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the CLI with these arguments: an argument list, never a shell
    /// string. `describe` names it in a notification if it fails.
    Cli {
        args: Vec<String>,
        describe: String,
    },
    OpenSettings,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Item {
        id: u32,
        label: String,
        /// Secondary text: an action's hotkey, an input's code.
        detail: Option<String>,
        command: Command,
    },
    /// Shown greyed out, to say why the menu is short.
    Note {
        label: String,
    },
    Submenu {
        label: String,
        entries: Vec<Entry>,
    },
    Separator,
}

/// The whole menu. `config` is the loaded configuration, or why it could not
/// be loaded.
pub fn build(config: Result<&Config, &str>) -> Vec<Entry> {
    let mut ids = 0u32;
    let mut next_id = move || {
        ids += 1;
        ids
    };
    let mut entries = Vec::new();

    match config {
        Err(why) => entries.push(Entry::Note { label: why.into() }),
        Ok(config) => {
            for action in &config.actions {
                entries.push(Entry::Item {
                    id: next_id(),
                    label: action.name.clone(),
                    detail: action.hotkey.as_deref().map(pretty_hotkey),
                    command: Command::Cli {
                        args: vec!["run-action".into(), action.name.clone()],
                        describe: action.name.clone(),
                    },
                });
            }

            let monitors: Vec<Entry> = config
                .monitors
                .iter()
                .filter_map(|m| monitor_submenu(m, &mut next_id))
                .collect();
            if !monitors.is_empty() {
                if !entries.is_empty() {
                    entries.push(Entry::Separator);
                }
                entries.extend(monitors);
            }

            if entries.is_empty() {
                entries.push(Entry::Note {
                    label: "Nothing configured yet".into(),
                });
            }
        }
    }

    entries.push(Entry::Separator);
    entries.push(Entry::Item {
        id: next_id(),
        label: "Settings…".into(),
        detail: None,
        command: Command::OpenSettings,
    });
    entries.push(Entry::Item {
        id: next_id(),
        label: "Quit".into(),
        detail: None,
        command: Command::Quit,
    });
    entries
}

fn monitor_submenu(monitor: &MonitorConfig, next_id: &mut impl FnMut() -> u32) -> Option<Entry> {
    let inputs = inputs_to_offer(monitor);
    if inputs.is_empty() {
        return None;
    }
    let entries = inputs
        .into_iter()
        .map(|input| Entry::Item {
            id: next_id(),
            label: input.display_name(),
            detail: Some(input.input_code.to_string()),
            command: Command::Cli {
                args: vec![
                    "set".into(),
                    monitor.key.clone(),
                    input.input_code.to_string(),
                ],
                describe: format!("{}: {}", monitor.label, input.display_name()),
            },
        })
        .collect();
    Some(Entry::Submenu {
        label: monitor.label.clone(),
        entries,
    })
}

/// The inputs worth a click: the ones someone named.
///
/// `configure` records every input a monitor claims, and capability lists
/// routinely claim sockets that do not exist. A menu offering them is a menu
/// for putting a monitor to sleep on an empty input with one click. An input
/// someone renamed from its standard name ("HDMI-1" to "Laptop") is one they
/// know. With nothing renamed, every input is offered, so a fresh setup still
/// has something to click.
pub fn inputs_to_offer(monitor: &MonitorConfig) -> Vec<&MonitorInput> {
    let named: Vec<&MonitorInput> = monitor.inputs.iter().filter(|i| is_named(i)).collect();
    if named.is_empty() {
        monitor.inputs.iter().collect()
    } else {
        named
    }
}

fn is_named(input: &MonitorInput) -> bool {
    match input.label.as_deref().map(str::trim) {
        None | Some("") => false,
        Some(label) => !input
            .input_code
            .standard_label()
            .is_some_and(|standard| standard.eq_ignore_ascii_case(label)),
    }
}

/// `CTRL+ALT+1` as a menu shows it: `Ctrl+Alt+1`.
pub fn pretty_hotkey(hotkey: &str) -> String {
    hotkey
        .split('+')
        .map(|part| {
            let part = part.trim();
            match part.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => "Ctrl".to_string(),
                "ALT" => "Alt".to_string(),
                "SHIFT" => "Shift".to_string(),
                "WIN" | "SUPER" | "META" => "Win".to_string(),
                other if other.chars().count() == 1 => other.to_string(),
                _ => {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => {
                            first.to_uppercase().collect::<String>()
                                + &chars.as_str().to_lowercase()
                        }
                        None => String::new(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// An item as a Windows menu shows it: the detail right-aligned after a tab,
/// and `&` doubled, because Windows takes a lone one as an accelerator marker
/// and hides it. A tab inside the text would start a new column, so it goes.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_text(label: &str, detail: Option<&str>) -> String {
    let clean = |s: &str| s.replace('&', "&&").replace('\t', " ");
    match detail {
        Some(d) => format!("{}\t{}", clean(label), clean(d)),
        None => clean(label),
    }
}

/// An item as a D-Bus menu (GNOME, KDE) shows it: `_` doubled, because there
/// it marks the accelerator, and the detail in brackets after the label,
/// since these menus have no column to right-align it in.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn dbus_text(label: &str, detail: Option<&str>) -> String {
    let clean = |s: &str| s.replace('_', "__");
    match detail {
        Some(d) => format!("{}   ({})", clean(label), clean(d)),
        None => clean(label),
    }
}

/// The command behind an id, searching submenus too. Windows needs it; the
/// Linux menu carries each command in its item instead.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn find(entries: &[Entry], id: u32) -> Option<&Command> {
    entries.iter().find_map(|entry| match entry {
        Entry::Item {
            id: item, command, ..
        } if *item == id => Some(command),
        Entry::Submenu { entries, .. } => find(entries, id),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use switcher_core::actions::Action;
    use switcher_core::config::BackendKind;
    use switcher_core::types::{Evidence, InputCode};

    fn input(code: u8, label: &str) -> MonitorInput {
        MonitorInput {
            input_code: InputCode(code),
            label: Some(label.into()),
            verification: Evidence::Reported,
            note: None,
        }
    }

    fn monitor(key: &str, label: &str, inputs: Vec<MonitorInput>) -> MonitorConfig {
        MonitorConfig {
            key: key.into(),
            label: label.into(),
            backend_id: format!("id-{key}"),
            serial: Some(key.into()),
            model: None,
            inputs,
        }
    }

    /// The configuration on the desk this was built at.
    fn desk() -> Config {
        let mut c = Config::new("laptop", BackendKind::Fake);
        let inputs = || {
            vec![
                input(0x01, "VGA-1"),
                input(0x03, "DVI-1"),
                input(0x11, "Laptop"),
                input(0x0F, "Workstation"),
            ]
        };
        c.monitors = vec![
            monitor("SN-L", "aoc-left", inputs()),
            monitor("SN-C", "aoc-center", inputs()),
        ];
        let mut left = Action::new("Toggle left");
        left.hotkey = Some("CTRL+ALT+1".into());
        let mut center = Action::new("Toggle center");
        center.hotkey = Some("CTRL+ALT+2".into());
        c.actions = vec![left, center, Action::new("Laptop")];
        c
    }

    /// Each entry as `label` or `label | detail`, for comparing whole menus.
    fn shown(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item {
                    label,
                    detail: Some(d),
                    ..
                } => format!("{label} | {d}"),
                Entry::Item { label, .. }
                | Entry::Note { label }
                | Entry::Submenu { label, .. } => label.clone(),
                Entry::Separator => "---".into(),
            })
            .collect()
    }

    #[test]
    fn actions_come_first_with_their_hotkeys_then_monitors_then_settings() {
        let menu = build(Ok(&desk()));
        assert_eq!(
            shown(&menu),
            vec![
                "Toggle left | Ctrl+Alt+1",
                "Toggle center | Ctrl+Alt+2",
                "Laptop",
                "---",
                "aoc-left",
                "aoc-center",
                "---",
                "Settings…",
                "Quit",
            ]
        );
    }

    #[test]
    fn an_action_runs_through_the_cli_by_name() {
        let menu = build(Ok(&desk()));
        let Entry::Item { command, .. } = &menu[1] else {
            panic!("not an item: {:?}", menu[1]);
        };
        assert_eq!(
            command,
            &Command::Cli {
                args: vec!["run-action".into(), "Toggle center".into()],
                describe: "Toggle center".into(),
            }
        );
    }

    /// A menu that offers VGA and DVI is a menu for putting a monitor to sleep
    /// on a socket with nothing in it.
    #[test]
    fn a_monitor_offers_only_the_inputs_someone_named() {
        let menu = build(Ok(&desk()));
        let Entry::Submenu { entries, .. } = &menu[4] else {
            panic!("not a submenu: {:?}", menu[4]);
        };
        assert_eq!(shown(entries), vec!["Laptop | 0x11", "Workstation | 0x0F"]);
        let Entry::Item { command, .. } = &entries[1] else {
            panic!()
        };
        assert_eq!(
            command,
            &Command::Cli {
                args: vec!["set".into(), "SN-L".into(), "0x0F".into()],
                describe: "aoc-left: Workstation".into(),
            }
        );
    }

    /// Standard names, in any case, are what `configure` wrote, not a choice.
    #[test]
    fn with_nothing_named_every_input_is_offered() {
        let m = monitor(
            "SN",
            "panel",
            vec![input(0x11, "hdmi-1"), input(0x0F, "DisplayPort-1")],
        );
        assert_eq!(inputs_to_offer(&m).len(), 2);
    }

    #[test]
    fn any_name_other_than_the_standard_one_counts_as_chosen() {
        let m = monitor(
            "SN",
            "panel",
            vec![input(0x11, "HDMI-1"), input(0x0F, "dp-1")],
        );
        let offered: Vec<u8> = inputs_to_offer(&m).iter().map(|i| i.input_code.0).collect();
        assert_eq!(offered, vec![0x0F]);
    }

    #[test]
    fn a_monitor_with_no_inputs_has_no_submenu() {
        let mut c = desk();
        c.monitors[1].inputs.clear();
        let menu = build(Ok(&c));
        assert!(!shown(&menu).contains(&"aoc-center".to_string()));
    }

    #[test]
    fn every_id_is_unique_and_findable() {
        let menu = build(Ok(&desk()));
        let mut seen = Vec::new();
        fn collect(entries: &[Entry], seen: &mut Vec<u32>) {
            for e in entries {
                match e {
                    Entry::Item { id, .. } => seen.push(*id),
                    Entry::Submenu { entries, .. } => collect(entries, seen),
                    _ => {}
                }
            }
        }
        collect(&menu, &mut seen);
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len());
        assert!(seen.iter().all(|&id| id != 0 && find(&menu, id).is_some()));
        assert_eq!(find(&menu, 999), None);
    }

    #[test]
    fn a_missing_configuration_still_offers_settings_and_quit() {
        let menu = build(Err("No configuration yet"));
        assert_eq!(
            shown(&menu),
            vec!["No configuration yet", "---", "Settings…", "Quit"]
        );
    }

    #[test]
    fn an_empty_configuration_says_so() {
        let c = Config::new("laptop", BackendKind::Fake);
        assert_eq!(shown(&build(Ok(&c)))[0], "Nothing configured yet");
    }

    #[test]
    fn hotkeys_read_as_people_write_them() {
        assert_eq!(pretty_hotkey("CTRL+ALT+1"), "Ctrl+Alt+1");
        assert_eq!(pretty_hotkey("ctrl+shift+F12"), "Ctrl+Shift+F12");
        assert_eq!(pretty_hotkey("WIN+d"), "Win+D");
    }

    /// Windows eats a lone `&` as an accelerator marker, and a tab starts a
    /// new column.
    #[test]
    fn windows_text_escapes_and_aligns() {
        assert_eq!(
            windows_text("Mail & chat", Some("Ctrl+M")),
            "Mail && chat\tCtrl+M"
        );
        assert_eq!(windows_text("a\tb", None), "a b");
    }

    /// In a D-Bus menu `_` marks the accelerator; a name like `left_panel`
    /// would otherwise lose its underscore and underline a letter.
    #[test]
    fn dbus_text_escapes_and_brackets_the_detail() {
        assert_eq!(
            dbus_text("Toggle left", Some("Ctrl+Alt+1")),
            "Toggle left   (Ctrl+Alt+1)"
        );
        assert_eq!(dbus_text("left_panel", None), "left__panel");
    }
}
