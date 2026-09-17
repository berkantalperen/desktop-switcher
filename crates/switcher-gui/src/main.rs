//! A small editor for the things that are tedious to configure by hand:
//! what each layer does on a switch, and the named actions bound to hotkeys.
//!
//! It deliberately owns no logic of its own. Everything it *does* — switching,
//! sweeping, running an action — it does by invoking the CLI, so there is one
//! implementation of the rules and the GUI cannot drift away from it or
//! bypass a safety check. What it owns is the configuration file.

// Release builds have no console; debug builds keep one so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use switcher_core::actions::{Action, ActionStep};
use switcher_core::config::{self, AwayAction, Config, HomeAction};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 620.0])
            .with_min_inner_size([560.0, 420.0])
            .with_title("Desktop Switcher"),
        ..Default::default()
    };
    eframe::run_native(
        "Desktop Switcher",
        options,
        Box::new(|_cc| Ok(Box::new(SwitcherApp::new()))),
    )
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Switch,
    Layers,
    Actions,
}

/// The result of a CLI invocation, delivered back to the UI thread.
struct CommandResult {
    label: String,
    success: bool,
    output: String,
}

struct SwitcherApp {
    config_path: PathBuf,
    config: Option<Config>,
    /// Why the configuration could not be loaded, if it could not.
    load_error: Option<String>,
    cli: Option<PathBuf>,
    tab: Tab,
    dirty: bool,
    status: String,
    output: String,
    running: Option<String>,
    tx: Sender<CommandResult>,
    rx: Receiver<CommandResult>,
}

impl SwitcherApp {
    fn new() -> Self {
        let (tx, rx) = channel();
        let config_path = config::default_config_path().unwrap_or_default();
        let (config, load_error) = match Config::load(&config_path) {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Self {
            config_path,
            config,
            load_error,
            cli: find_cli(),
            tab: Tab::Switch,
            dirty: false,
            status: String::new(),
            output: String::new(),
            running: None,
            tx,
            rx,
        }
    }

    /// Run the CLI off the UI thread, so a switch does not freeze the window
    /// for the settle delay.
    fn run_cli(&mut self, label: &str, args: Vec<String>) {
        let Some(cli) = self.cli.clone() else {
            self.status = "desktop-switcher was not found next to this program or on PATH".into();
            return;
        };
        if self.running.is_some() {
            self.status = "Still running the previous command.".into();
            return;
        }
        self.running = Some(label.to_string());
        self.status = format!("Running: {label}…");
        self.output.clear();

        let tx = self.tx.clone();
        let label = label.to_string();
        std::thread::spawn(move || {
            let result = std::process::Command::new(&cli).args(&args).output();
            let message = match result {
                Ok(out) => CommandResult {
                    label,
                    success: out.status.success(),
                    output: format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    ),
                },
                Err(e) => CommandResult {
                    label,
                    success: false,
                    output: format!("could not start desktop-switcher: {e}"),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn save(&mut self) {
        let Some(config) = &self.config else { return };
        match config.save(&self.config_path) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("Saved to {}", self.config_path.display());
            }
            // Validation runs inside save, so this is where a duplicate
            // hotkey or an empty action gets reported.
            Err(e) => self.status = format!("Not saved: {e}"),
        }
    }

    fn reload(&mut self) {
        match Config::load(&self.config_path) {
            Ok(c) => {
                self.config = Some(c);
                self.load_error = None;
                self.dirty = false;
                self.status = "Reloaded from disk.".into();
            }
            Err(e) => {
                self.load_error = Some(e.to_string());
                self.status = format!("Could not reload: {e}");
            }
        }
    }
}

/// Find the CLI next to this executable first, then on PATH.
fn find_cli() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "desktop-switcher.exe"
    } else {
        "desktop-switcher"
    };
    if let Ok(mut here) = std::env::current_exe() {
        here.pop();
        let candidate = here.join(exe_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe_name))
        .find(|p| p.is_file())
}

impl eframe::App for SwitcherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(result) = self.rx.try_recv() {
            self.running = None;
            self.status = if result.success {
                format!("{} finished.", result.label)
            } else {
                format!("{} failed.", result.label)
            };
            self.output = result.output;
        }
        // A command is in flight on another thread, so keep repainting until
        // its result arrives rather than waiting for the next mouse move.
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Switch, "Switch");
                ui.selectable_value(&mut self.tab, Tab::Layers, "Layers");
                ui.selectable_value(&mut self.tab, Tab::Actions, "Actions");
                ui.separator();
                if ui.button("Reload").clicked() {
                    self.reload();
                }
                let can_save = self.dirty && self.config.is_some();
                if ui
                    .add_enabled(can_save, egui::Button::new("Save"))
                    .clicked()
                {
                    self.save();
                }
                if self.dirty {
                    ui.colored_label(egui::Color32::from_rgb(200, 150, 60), "unsaved changes");
                }
            });
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("status")
            .min_height(120.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if self.running.is_some() {
                        ui.spinner();
                    }
                    ui.label(&self.status);
                });
                if !self.output.is_empty() {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.monospace(&self.output);
                        });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.config.is_none() {
                self.show_no_config(ui);
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::Switch => self.show_switch(ui),
                Tab::Layers => self.show_layers(ui),
                Tab::Actions => self.show_actions(ui),
            });
        });
    }
}

impl SwitcherApp {
    fn show_no_config(&mut self, ui: &mut egui::Ui) {
        ui.heading("No configuration yet");
        ui.add_space(8.0);
        ui.label(
            "This editor changes an existing configuration; it does not identify \
             monitors or verify input codes. Those need a human watching the screens, \
             so they live in the command line.",
        );
        ui.add_space(8.0);
        ui.monospace("desktop-switcher configure");
        ui.add_space(8.0);
        if let Some(e) = &self.load_error {
            ui.label(format!("Looked in {}", self.config_path.display()));
            ui.colored_label(egui::Color32::from_rgb(200, 120, 120), e);
        }
        ui.add_space(8.0);
        if ui.button("Check again").clicked() {
            self.reload();
        }
    }

    fn show_switch(&mut self, ui: &mut egui::Ui) {
        let Some(config) = &self.config else { return };
        let destinations = config.destinations();
        let self_destination = config.self_destination.clone();
        let monitors: Vec<String> = config
            .monitors
            .iter()
            .map(|m| m.logical_id.clone())
            .collect();

        ui.heading("Layer 1 — which computer the monitors show");
        ui.add_space(6.0);
        let mut pending: Vec<(String, Vec<String>)> = Vec::new();
        ui.horizontal_wrapped(|ui| {
            for destination in &destinations {
                let label = if *destination == self_destination {
                    format!("Bring here ({destination})")
                } else {
                    format!("Send to {destination}")
                };
                if ui.button(label).clicked() {
                    pending.push((
                        format!("switch {destination}"),
                        vec!["switch".into(), destination.clone()],
                    ));
                }
            }
        });

        ui.add_space(14.0);
        ui.heading("Layer 2 — this computer's desktop");
        ui.label(
            "Both computers can have a monitor attached at once, which is why a window \
             can strand on a screen showing the other machine. Sweeping moves those \
             windows back without changing anything about the display layout.",
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if monitors.is_empty() {
                if ui.button("Sweep windows off").clicked() {
                    pending.push(("sweep".into(), vec!["sweep".into()]));
                }
            } else {
                for monitor in &monitors {
                    if ui.button(format!("Sweep windows off {monitor}")).clicked() {
                        pending.push((
                            format!("sweep {monitor}"),
                            vec!["sweep".into(), monitor.clone()],
                        ));
                    }
                }
            }
        });

        ui.add_space(14.0);
        ui.heading("Look, don't touch");
        ui.horizontal_wrapped(|ui| {
            for (label, arg) in [
                ("Status", "status"),
                ("Displays", "displays"),
                ("Monitors", "monitors"),
                ("Doctor", "doctor"),
            ] {
                if ui.button(label).clicked() {
                    pending.push((arg.to_string(), vec![arg.to_string()]));
                }
            }
        });

        for (label, args) in pending {
            self.run_cli(&label, args);
        }
    }

    fn show_layers(&mut self, ui: &mut egui::Ui) {
        let Some(config) = &mut self.config else {
            return;
        };
        ui.heading("What a plain switch should also do");
        ui.label(
            "These are off by default, so `switch` only changes the monitor input. \
             An action can still do more without changing this.",
        );
        ui.add_space(10.0);

        ui.label("When a monitor is sent to the other computer:");
        let before = config.on_switch;
        ui.radio_value(
            &mut config.on_switch.away,
            AwayAction::Nothing,
            "Nothing — fastest, and windows on it become unreachable",
        );
        ui.radio_value(
            &mut config.on_switch.away,
            AwayAction::Sweep,
            "Sweep — move windows off it, leave it attached",
        );
        ui.radio_value(
            &mut config.on_switch.away,
            AwayAction::Release,
            "Release — detach it from this desktop (experimental, needs --experimental)",
        );

        ui.add_space(12.0);
        ui.label("When a monitor comes back to this computer:");
        ui.radio_value(&mut config.on_switch.home, HomeAction::Nothing, "Nothing");
        ui.radio_value(
            &mut config.on_switch.home,
            HomeAction::Claim,
            "Claim — reattach it if this computer had released it (experimental)",
        );

        if config.on_switch != before {
            self.dirty = true;
        }

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Release and Claim change the Windows display topology and are not reliable \
                 yet — they have left monitors mirrored instead of extended. They refuse to \
                 run unless --experimental is passed. Sweep solves the same problem safely.",
            )
            .color(egui::Color32::from_rgb(200, 150, 60)),
        );
    }

    fn show_actions(&mut self, ui: &mut egui::Ui) {
        let (destinations, monitors) = {
            let Some(config) = &self.config else { return };
            (
                config.destinations(),
                config
                    .monitors
                    .iter()
                    .map(|m| m.logical_id.clone())
                    .collect::<Vec<_>>(),
            )
        };

        ui.heading("Actions");
        ui.label(
            "A named sequence of steps, optionally bound to a hotkey. Steps run in order \
             and stop at the first failure.",
        );
        ui.add_space(8.0);

        let mut remove_action: Option<usize> = None;
        let mut run_action: Option<String> = None;
        let mut dirty = false;

        let Some(config) = &mut self.config else {
            return;
        };

        for (index, action) in config.actions.iter_mut().enumerate() {
            let header = if action.name.trim().is_empty() {
                format!("(unnamed action {})", index + 1)
            } else {
                action.name.clone()
            };
            egui::CollapsingHeader::new(header)
                .id_salt(index)
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new(format!("meta{index}"))
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            dirty |= ui.text_edit_singleline(&mut action.name).changed();
                            ui.end_row();

                            ui.label("Hotkey");
                            let mut hotkey = action.hotkey.clone().unwrap_or_default();
                            if ui.text_edit_singleline(&mut hotkey).changed() {
                                action.hotkey = (!hotkey.trim().is_empty()).then(|| hotkey.clone());
                                dirty = true;
                            }
                            ui.end_row();
                        });
                    ui.small(
                        "Hotkey looks like CTRL+ALT+2. Leave it empty to keep the action unbound.",
                    );

                    ui.add_space(6.0);
                    ui.label("Steps:");
                    let mut remove_step: Option<usize> = None;
                    for (step_index, step) in action.steps.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            dirty |=
                                step_editor(ui, index, step_index, step, &destinations, &monitors);
                            if ui.button("✕").on_hover_text("Remove this step").clicked() {
                                remove_step = Some(step_index);
                            }
                        });
                    }
                    if let Some(i) = remove_step {
                        action.steps.remove(i);
                        dirty = true;
                    }

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("+ Switch").clicked() {
                            action.steps.push(ActionStep::Switch {
                                destination: destinations.first().cloned().unwrap_or_default(),
                            });
                            dirty = true;
                        }
                        if ui.button("+ Sweep").clicked() {
                            action.steps.push(ActionStep::Sweep {
                                monitor: monitors.first().cloned(),
                            });
                            dirty = true;
                        }
                        if ui.button("+ Run a command").clicked() {
                            action.steps.push(ActionStep::Run {
                                command: String::new(),
                                args: Vec::new(),
                            });
                            dirty = true;
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Run now").clicked() {
                            run_action = Some(action.name.clone());
                        }
                        if ui.button("Delete action").clicked() {
                            remove_action = Some(index);
                        }
                        if action.needs_experimental() {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 150, 60),
                                "contains an experimental step",
                            );
                        }
                    });
                });
            ui.add_space(4.0);
        }

        if let Some(i) = remove_action {
            config.actions.remove(i);
            dirty = true;
        }

        ui.add_space(8.0);
        if ui.button("+ New action").clicked() {
            let mut action = Action::new(format!("Action {}", config.actions.len() + 1));
            if let Some(first) = destinations.first() {
                action.steps.push(ActionStep::Switch {
                    destination: first.clone(),
                });
            }
            config.actions.push(action);
            dirty = true;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label("Hotkeys are registered with the operating system by the installer scripts:");
        ui.monospace(r".\scripts\install-windows-shortcuts.ps1");
        ui.monospace("./scripts/install-gnome-shortcuts.sh");
        ui.small("Save first — the installers read the saved configuration.");

        if dirty {
            self.dirty = true;
        }
        if let Some(name) = run_action {
            self.run_cli(
                &format!("run-action {name}"),
                vec!["run-action".into(), name],
            );
        }
    }
}

/// One step's editor. Returns whether anything changed.
fn step_editor(
    ui: &mut egui::Ui,
    action_index: usize,
    step_index: usize,
    step: &mut ActionStep,
    destinations: &[String],
    monitors: &[String],
) -> bool {
    let mut changed = false;
    let id = format!("step{action_index}-{step_index}");

    // Changing the kind replaces the step, so the previous one's fields do
    // not linger invisibly in the saved file.
    let mut kind = step_kind(step);
    egui::ComboBox::from_id_salt(format!("{id}-kind"))
        .selected_text(kind_label(kind))
        .width(150.0)
        .show_ui(ui, |ui| {
            for option in [
                StepKind::Switch,
                StepKind::Sweep,
                StepKind::Run,
                StepKind::Release,
                StepKind::Claim,
                StepKind::Primary,
            ] {
                if ui
                    .selectable_value(&mut kind, option, kind_label(option))
                    .clicked()
                {
                    changed = true;
                }
            }
        });
    if kind != step_kind(step) {
        *step = default_step(kind, destinations, monitors);
        changed = true;
    }

    match step {
        ActionStep::Switch { destination } => {
            if destinations.is_empty() {
                changed |= ui.text_edit_singleline(destination).changed();
            } else {
                egui::ComboBox::from_id_salt(format!("{id}-dest"))
                    .selected_text(destination.clone())
                    .show_ui(ui, |ui| {
                        for option in destinations {
                            if ui
                                .selectable_value(destination, option.clone(), option)
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            }
        }
        ActionStep::Sweep { monitor }
        | ActionStep::Release { monitor }
        | ActionStep::Claim { monitor }
        | ActionStep::Primary { monitor } => {
            let mut selected = monitor.clone().unwrap_or_default();
            egui::ComboBox::from_id_salt(format!("{id}-mon"))
                .selected_text(if selected.is_empty() {
                    "(the only monitor)".to_string()
                } else {
                    selected.clone()
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut selected, String::new(), "(the only monitor)")
                        .clicked()
                    {
                        changed = true;
                    }
                    for option in monitors {
                        if ui
                            .selectable_value(&mut selected, option.clone(), option)
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                });
            let next = (!selected.is_empty()).then_some(selected);
            if *monitor != next {
                *monitor = next;
                changed = true;
            }
        }
        ActionStep::Run { command, args } => {
            ui.label("command");
            changed |= ui
                .add(egui::TextEdit::singleline(command).desired_width(200.0))
                .changed();
            ui.label("args");
            let mut joined = args.join(" ");
            if ui
                .add(egui::TextEdit::singleline(&mut joined).desired_width(160.0))
                .changed()
            {
                // Split on whitespace: arguments are passed as a list, never
                // handed to a shell, so quoting rules would be misleading.
                *args = joined.split_whitespace().map(String::from).collect();
                changed = true;
            }
        }
    }
    changed
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum StepKind {
    Switch,
    Sweep,
    Release,
    Claim,
    Primary,
    Run,
}

fn step_kind(step: &ActionStep) -> StepKind {
    match step {
        ActionStep::Switch { .. } => StepKind::Switch,
        ActionStep::Sweep { .. } => StepKind::Sweep,
        ActionStep::Release { .. } => StepKind::Release,
        ActionStep::Claim { .. } => StepKind::Claim,
        ActionStep::Primary { .. } => StepKind::Primary,
        ActionStep::Run { .. } => StepKind::Run,
    }
}

fn kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Switch => "Switch input",
        StepKind::Sweep => "Sweep windows",
        StepKind::Release => "Release (exp.)",
        StepKind::Claim => "Claim (exp.)",
        StepKind::Primary => "Make primary (exp.)",
        StepKind::Run => "Run a command",
    }
}

fn default_step(kind: StepKind, destinations: &[String], monitors: &[String]) -> ActionStep {
    let monitor = monitors.first().cloned();
    match kind {
        StepKind::Switch => ActionStep::Switch {
            destination: destinations.first().cloned().unwrap_or_default(),
        },
        StepKind::Sweep => ActionStep::Sweep { monitor },
        StepKind::Release => ActionStep::Release { monitor },
        StepKind::Claim => ActionStep::Claim { monitor },
        StepKind::Primary => ActionStep::Primary { monitor },
        StepKind::Run => ActionStep::Run {
            command: String::new(),
            args: Vec::new(),
        },
    }
}
