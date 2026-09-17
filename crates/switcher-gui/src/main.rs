//! A small editor and launcher for the switcher.
//!
//! It owns no logic of its own. Everything it *does* it does by invoking the
//! CLI, so there is one implementation of the rules and the GUI cannot drift
//! from it or bypass a safety check. What it owns is the configuration file.
//!
//! Like the rest of the project, it knows about monitors and inputs and
//! nothing else. Which computer is on which input is the user's business, and
//! the place they record it is the name they give an action.

// Release builds have no console; debug builds keep one so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;
use switcher_core::actions::{Action, ActionStep};
use switcher_core::config::{self, Config, MonitorInput};
use switcher_core::types::InputCode;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 640.0])
            .with_min_inner_size([600.0, 440.0])
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
    Monitors,
    Actions,
    Settings,
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
            tab: Tab::Monitors,
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
            // Validation runs inside save, so a duplicate hotkey or an action
            // naming a monitor that no longer exists is reported here.
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
                ui.selectable_value(&mut self.tab, Tab::Monitors, "Monitors");
                ui.selectable_value(&mut self.tab, Tab::Actions, "Actions");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
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
            .min_height(110.0)
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
                        .max_height(150.0)
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
                Tab::Monitors => self.show_monitors(ui),
                Tab::Actions => self.show_actions(ui),
                Tab::Settings => self.show_settings(ui),
            });
        });
    }
}

impl SwitcherApp {
    fn show_no_config(&mut self, ui: &mut egui::Ui) {
        ui.heading("No configuration yet");
        ui.add_space(8.0);
        ui.label(
            "This editor changes an existing configuration. Discovering which monitors \
             exist and what inputs they offer happens in the command line:",
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

    fn show_monitors(&mut self, ui: &mut egui::Ui) {
        let mut pending: Vec<(String, Vec<String>)> = Vec::new();
        let mut dirty = false;

        ui.heading("Monitors");
        ui.label("Click an input to switch that monitor to it.");
        ui.add_space(8.0);

        {
            let Some(config) = &mut self.config else {
                return;
            };
            if config.monitors.is_empty() {
                ui.label("No monitors are configured. Run `desktop-switcher configure`.");
                return;
            }

            for (index, monitor) in config.monitors.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&monitor.label).strong());
                        ui.label(
                            egui::RichText::new(format!("· {}", monitor.key))
                                .weak()
                                .small(),
                        );
                    });

                    if monitor.inputs.is_empty() {
                        ui.label("No inputs recorded. Re-run `configure`.");
                    } else {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for input in &monitor.inputs {
                                let text =
                                    format!("{}  ({})", input.display_name(), input.input_code);
                                if ui.button(text).clicked() {
                                    pending.push((
                                        format!("set {} {}", monitor.key, input.input_code),
                                        vec![
                                            "set".into(),
                                            monitor.key.clone(),
                                            input.input_code.to_string(),
                                        ],
                                    ));
                                }
                            }
                        });
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("Sweep windows off")
                            .on_hover_text(
                                "Move windows off this monitor without changing the display layout",
                            )
                            .clicked()
                        {
                            pending.push((
                                format!("sweep {}", monitor.key),
                                vec!["sweep".into(), monitor.key.clone()],
                            ));
                        }
                        if ui
                            .button("Restore windows")
                            .on_hover_text("Put swept windows back where they were")
                            .clicked()
                        {
                            pending.push((
                                format!("restore {}", monitor.key),
                                vec!["restore-windows".into(), monitor.key.clone()],
                            ));
                        }
                    });

                    // Renaming lives here rather than in a settings screen:
                    // it is the one place the person can say what an input
                    // actually is to them.
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new("Rename")
                        .id_salt(index)
                        .show(ui, |ui| {
                            egui::Grid::new(format!("names{index}"))
                                .num_columns(2)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    ui.label("Monitor");
                                    dirty |= ui.text_edit_singleline(&mut monitor.label).changed();
                                    ui.end_row();
                                    for input in monitor.inputs.iter_mut() {
                                        ui.label(input.input_code.to_string());
                                        let mut name = input.label.clone().unwrap_or_default();
                                        if ui.text_edit_singleline(&mut name).changed() {
                                            input.label =
                                                (!name.trim().is_empty()).then(|| name.clone());
                                            dirty = true;
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                });
                ui.add_space(6.0);
            }
        }

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (label, arg) in [
                ("Status", "status"),
                ("Re-read hardware", "monitors"),
                ("Displays", "displays"),
                ("Doctor", "doctor"),
            ] {
                if ui.button(label).clicked() {
                    pending.push((arg.to_string(), vec![arg.to_string()]));
                }
            }
        });
        ui.small(
            "Adding or removing a monitor needs `desktop-switcher configure`, which reads \
             the inputs from the hardware.",
        );

        if dirty {
            self.dirty = true;
        }
        for (label, args) in pending {
            self.run_cli(&label, args);
        }
    }

    fn show_settings(&mut self, ui: &mut egui::Ui) {
        let Some(config) = &mut self.config else {
            return;
        };
        let mut dirty = false;

        ui.heading("Settings");
        ui.add_space(8.0);
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Settle delay (ms)");
                dirty |= ui
                    .add(egui::DragValue::new(&mut config.settle_ms).range(0..=5000))
                    .on_hover_text(
                        "How long to wait after a write before reading the input back. \
                         Too short and the monitor has not settled; too long and a hotkey \
                         feels slow.",
                    )
                    .changed();
                ui.end_row();

                ui.label("Ignore repeats within (ms)");
                dirty |= ui
                    .add(egui::DragValue::new(&mut config.dedupe_ms).range(0..=10000))
                    .on_hover_text(
                        "A held hotkey would otherwise issue a burst of writes. Zero \
                         disables the guard.",
                    )
                    .changed();
                ui.end_row();

                ui.label("Command timeout (s)");
                dirty |= ui
                    .add(egui::DragValue::new(&mut config.timeout_seconds).range(1..=120))
                    .changed();
                ui.end_row();
            });

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Configuration file").strong());
        ui.monospace(self.config_path.display().to_string());
        ui.small(format!(
            "Host: {} · backend: {:?}",
            config.host, config.backend
        ));

        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(
                "Release, Claim and Make-primary change the Windows display topology and \
                 are not reliable yet — they have left monitors mirrored instead of \
                 extended. They refuse to run unless --experimental is passed. Sweep and \
                 Restore windows solve the same problem safely.",
            )
            .color(egui::Color32::from_rgb(200, 150, 60)),
        );

        if dirty {
            self.dirty = true;
        }
    }

    fn show_actions(&mut self, ui: &mut egui::Ui) {
        // Monitors and their inputs, so a step can be picked rather than typed.
        let monitors: Vec<(String, String, Vec<MonitorInput>)> = {
            let Some(config) = &self.config else { return };
            config
                .monitors
                .iter()
                .map(|m| (m.key.clone(), m.label.clone(), m.inputs.clone()))
                .collect()
        };

        ui.heading("Actions");
        ui.label(
            "A named sequence of steps, optionally bound to a hotkey. Steps run in order \
             and stop at the first failure. Name them however you think of them.",
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
                .id_salt(1000 + index)
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
                    ui.small("Hotkey looks like CTRL+ALT+2. Leave it empty to keep it unbound.");

                    ui.add_space(6.0);
                    ui.label("Steps:");
                    let mut remove_step: Option<usize> = None;
                    for (step_index, step) in action.steps.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            dirty |= step_editor(ui, index, step_index, step, &monitors);
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
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("+ Set input").clicked() {
                            action.steps.push(default_set_input(&monitors));
                            dirty = true;
                        }
                        if ui.button("+ Sweep").clicked() {
                            action.steps.push(ActionStep::Sweep {
                                monitor: monitors.first().map(|m| m.0.clone()),
                            });
                            dirty = true;
                        }
                        if ui.button("+ Restore windows").clicked() {
                            action.steps.push(ActionStep::RestoreWindows {
                                monitor: monitors.first().map(|m| m.0.clone()),
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
            action.steps.push(default_set_input(&monitors));
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

type MonitorChoice = (String, String, Vec<MonitorInput>);

fn default_set_input(monitors: &[MonitorChoice]) -> ActionStep {
    let (key, _, inputs) = monitors.first().cloned().unwrap_or_default();
    ActionStep::SetInput {
        monitor: key,
        code: inputs
            .first()
            .map(|i| i.input_code)
            .unwrap_or(InputCode(0x11)),
    }
}

/// One step's editor. Returns whether anything changed.
fn step_editor(
    ui: &mut egui::Ui,
    action_index: usize,
    step_index: usize,
    step: &mut ActionStep,
    monitors: &[MonitorChoice],
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
                StepKind::SetInput,
                StepKind::Sweep,
                StepKind::RestoreWindows,
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
        *step = default_step(kind, monitors);
        changed = true;
    }

    match step {
        ActionStep::SetInput { monitor, code } => {
            let label_for = |key: &str| {
                monitors
                    .iter()
                    .find(|(k, _, _)| k == key)
                    .map(|(_, l, _)| l.clone())
                    .unwrap_or_else(|| key.to_string())
            };
            egui::ComboBox::from_id_salt(format!("{id}-mon"))
                .selected_text(label_for(monitor))
                .width(200.0)
                .show_ui(ui, |ui| {
                    for (key, label, _) in monitors {
                        if ui.selectable_value(monitor, key.clone(), label).clicked() {
                            changed = true;
                        }
                    }
                });

            let inputs = monitors
                .iter()
                .find(|(k, _, _)| k == monitor)
                .map(|(_, _, i)| i.clone())
                .unwrap_or_default();
            let current = inputs
                .iter()
                .find(|i| i.input_code == *code)
                .map(|i| format!("{}  ({})", i.display_name(), i.input_code))
                .unwrap_or_else(|| code.to_string());

            egui::ComboBox::from_id_salt(format!("{id}-code"))
                .selected_text(current)
                .width(190.0)
                .show_ui(ui, |ui| {
                    for input in &inputs {
                        let text = format!("{}  ({})", input.display_name(), input.input_code);
                        if ui.selectable_value(code, input.input_code, text).clicked() {
                            changed = true;
                        }
                    }
                });
        }
        ActionStep::Sweep { monitor }
        | ActionStep::RestoreWindows { monitor }
        | ActionStep::Release { monitor }
        | ActionStep::Claim { monitor }
        | ActionStep::Primary { monitor } => {
            let mut selected = monitor.clone().unwrap_or_default();
            let shown = monitors
                .iter()
                .find(|(k, _, _)| *k == selected)
                .map(|(_, l, _)| l.clone())
                .unwrap_or_else(|| "(the only monitor)".to_string());
            egui::ComboBox::from_id_salt(format!("{id}-mon"))
                .selected_text(shown)
                .width(220.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut selected, String::new(), "(the only monitor)")
                        .clicked()
                    {
                        changed = true;
                    }
                    for (key, label, _) in monitors {
                        if ui
                            .selectable_value(&mut selected, key.clone(), label)
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
                .add(egui::TextEdit::singleline(&mut joined).desired_width(150.0))
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
    SetInput,
    Sweep,
    RestoreWindows,
    Release,
    Claim,
    Primary,
    Run,
}

fn step_kind(step: &ActionStep) -> StepKind {
    match step {
        ActionStep::SetInput { .. } => StepKind::SetInput,
        ActionStep::Sweep { .. } => StepKind::Sweep,
        ActionStep::RestoreWindows { .. } => StepKind::RestoreWindows,
        ActionStep::Release { .. } => StepKind::Release,
        ActionStep::Claim { .. } => StepKind::Claim,
        ActionStep::Primary { .. } => StepKind::Primary,
        ActionStep::Run { .. } => StepKind::Run,
    }
}

fn kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::SetInput => "Set input",
        StepKind::Sweep => "Sweep windows",
        StepKind::RestoreWindows => "Restore windows",
        StepKind::Release => "Release (exp.)",
        StepKind::Claim => "Claim (exp.)",
        StepKind::Primary => "Make primary (exp.)",
        StepKind::Run => "Run a command",
    }
}

fn default_step(kind: StepKind, monitors: &[MonitorChoice]) -> ActionStep {
    let monitor = monitors.first().map(|m| m.0.clone());
    match kind {
        StepKind::SetInput => default_set_input(monitors),
        StepKind::Sweep => ActionStep::Sweep { monitor },
        StepKind::RestoreWindows => ActionStep::RestoreWindows { monitor },
        StepKind::Release => ActionStep::Release { monitor },
        StepKind::Claim => ActionStep::Claim { monitor },
        StepKind::Primary => ActionStep::Primary { monitor },
        StepKind::Run => ActionStep::Run {
            command: String::new(),
            args: Vec::new(),
        },
    }
}
