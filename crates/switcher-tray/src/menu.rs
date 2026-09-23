//! What the tray menu offers, built from the configuration.
//!
//! Pure, so the rules are tested on every platform. The menu is rebuilt from
//! the configuration each time it opens, so a change made in the GUI shows up
//! on the next click with nothing to restart.

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
        text: String,
        command: Command,
    },
    /// Shown greyed out, to say why the menu is short.
    Note {
        text: String,
    },
    Submenu {
        text: String,
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
        Err(why) => entries.push(Entry::Note {
            text: menu_text(why),
        }),
        Ok(config) => {
            for action in &config.actions {
                let mut text = menu_text(&action.name);
                if let Some(hotkey) = action.hotkey.as_deref() {
                    text.push('\t');
                    text.push_str(&pretty_hotkey(hotkey));
                }
                entries.push(Entry::Item {
                    id: next_id(),
                    text,
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
                    text: "Nothing configured yet".into(),
                });
            }
        }
    }

    entries.push(Entry::Separator);
    entries.push(Entry::Item {
        id: next_id(),
        text: "Settings…".into(),
        command: Command::OpenSettings,
    });
    entries.push(Entry::Item {
        id: next_id(),
        text: "Quit".into(),
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
            text: format!("{}\t{}", menu_text(&input.display_name()), input.input_code),
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
        text: menu_text(&monitor.label),
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

/// Text safe to hand to a Windows menu: `&` would otherwise underline the
/// next letter and vanish, and a tab would split the item into columns.
pub fn menu_text(s: &str) -> String {
    s.replace('&', "&&").replace('\t', " ")
}

/// The command behind an id, searching submenus too.
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
        let mut laptop = Action::new("Laptop");
        laptop.hotkey = Some("CTRL+ALT+1".into());
        let mut workstation = Action::new("Workstation");
        workstation.hotkey = Some("CTRL+ALT+2".into());
        c.actions = vec![laptop, workstation];
        c
    }

    fn texts(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item { text, .. } | Entry::Note { text } | Entry::Submenu { text, .. } => {
                    text.clone()
                }
                Entry::Separator => "---".into(),
            })
            .collect()
    }

    #[test]
    fn actions_come_first_with_their_hotkeys_then_monitors_then_settings() {
        let menu = build(Ok(&desk()));
        assert_eq!(
            texts(&menu),
            vec![
                "Laptop\tCtrl+Alt+1",
                "Workstation\tCtrl+Alt+2",
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
                args: vec!["run-action".into(), "Workstation".into()],
                describe: "Workstation".into(),
            }
        );
    }

    /// A menu that offers VGA and DVI is a menu for putting a monitor to sleep
    /// on a socket with nothing in it.
    #[test]
    fn a_monitor_offers_only_the_inputs_someone_named() {
        let menu = build(Ok(&desk()));
        let Entry::Submenu { entries, .. } = &menu[3] else {
            panic!("not a submenu: {:?}", menu[3]);
        };
        assert_eq!(texts(entries), vec!["Laptop\t0x11", "Workstation\t0x0F"]);
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
        assert!(!texts(&menu).contains(&"aoc-center".to_string()));
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
            texts(&menu),
            vec!["No configuration yet", "---", "Settings…", "Quit"]
        );
    }

    #[test]
    fn an_empty_configuration_says_so() {
        let c = Config::new("laptop", BackendKind::Fake);
        assert_eq!(texts(&build(Ok(&c)))[0], "Nothing configured yet");
    }

    #[test]
    fn hotkeys_read_as_people_write_them() {
        assert_eq!(pretty_hotkey("CTRL+ALT+1"), "Ctrl+Alt+1");
        assert_eq!(pretty_hotkey("ctrl+shift+F12"), "Ctrl+Shift+F12");
        assert_eq!(pretty_hotkey("WIN+d"), "Win+D");
    }

    /// Windows eats a lone `&` as an accelerator marker.
    #[test]
    fn names_are_escaped_for_a_windows_menu() {
        assert_eq!(menu_text("Mail & chat"), "Mail && chat");
        assert_eq!(menu_text("a\tb"), "a b");
    }
}
