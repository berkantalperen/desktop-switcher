//! Command implementations.
//!
//! The vocabulary is monitors and inputs. Nothing here knows or asks what is
//! plugged into an input — that meaning lives in the name a person gives an
//! action.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use switcher_core::actions::ActionStep;
use switcher_core::backend::Severity;
use switcher_core::config::{Config, MonitorConfig, MonitorInput};
use switcher_core::eventlog;
use switcher_core::inventory;
use switcher_core::switch::{self, ApplyReport, ExecOptions, LastRequest, SwitchGuard};
use switcher_core::types::{DetectedMonitor, Evidence, InputCode, InputReading, MonitorIdentity};

use crate::desktop;
use crate::ui;
use crate::App;

const LOCK_STALE_AFTER: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

pub fn doctor(app: &App) -> Result<i32> {
    ui::heading("Backend");
    let health = app.backend.health();
    ui::field("backend", &health.backend);
    // A backend built on an OS API has no tool to report, and saying
    // "(not resolved)" would read as a problem.
    if let Some(path) = health.tool_path.as_deref() {
        ui::field("tool path", path);
    }
    if let Some(version) = health.tool_version.as_deref() {
        ui::field("tool version", version);
    }

    println!();
    let mut blockers = 0;
    for finding in &health.findings {
        println!("  [{}] {}", finding.severity, finding.message);
        if let Some(remedy) = &finding.remedy {
            for line in textwrap(remedy, 72) {
                println!("         {line}");
            }
        }
        if finding.severity == Severity::Blocker {
            blockers += 1;
        }
    }

    ui::heading("Configuration");
    ui::field("path", app.config_path.display().to_string());
    match &app.config {
        None => {
            println!("\n  [BLOCKER] No configuration yet.");
            println!("         Run `desktop-switcher configure`.");
            blockers += 1;
        }
        Some(config) => {
            ui::field("host", &config.host);
            ui::field("monitors", config.monitors.len().to_string());
            ui::field("actions", config.actions.len().to_string());

            match app.backend.discover() {
                Err(e) => {
                    println!("\n  [BLOCKER] Could not enumerate displays: {e}");
                    blockers += 1;
                }
                Ok(detected) => {
                    let duplicates = inventory::duplicate_serials(&detected);
                    if !duplicates.is_empty() {
                        println!(
                            "\n  [BLOCKER] Serial(s) {} appear on more than one display; \
                             every binding that relies on them is ambiguous.",
                            duplicates.join(", ")
                        );
                        blockers += 1;
                    }
                    let bindings = inventory::bind(config, &detected);
                    println!();
                    for binding in &bindings.bound {
                        println!(
                            "  [ok] {} -> {}",
                            binding.key(),
                            binding.detected.identity.describe()
                        );
                    }
                    // A backend may not list a monitor it cannot talk to, so
                    // "unplugged" and "here but unreachable" can arrive
                    // identically. The desktop layer can tell them apart, and
                    // the difference decides what the person should do next.
                    let at_os_level = desktop::manager(&detected).displays().unwrap_or_default();
                    for problem in &bindings.problems {
                        let place = if problem.is_not_found() {
                            config
                                .monitor(problem.monitor())
                                .map(|m| whereabouts(&at_os_level, &m.backend_id))
                                .unwrap_or(Whereabouts::Absent)
                        } else {
                            Whereabouts::Absent
                        };
                        let text = match place {
                            Whereabouts::Attached => problem
                                .as_present_but_silent()
                                .unwrap_or_else(|| problem.clone())
                                .to_string(),
                            Whereabouts::Detached => format!(
                                "{problem} It is plugged in but turned off in this computer's \
                                 display settings; turn it on in Settings > System > Display."
                            ),
                            Whereabouts::Absent => problem.to_string(),
                        };
                        let mut lines = textwrap(&text, 66).into_iter();
                        println!("  [BLOCKER] {}", lines.next().unwrap_or_default());
                        for line in lines {
                            println!("            {line}");
                        }
                        blockers += 1;
                    }
                    for extra in &bindings.unclaimed {
                        println!(
                            "  [info] {} is attached but not configured",
                            extra.identity.describe()
                        );
                    }
                }
            }
        }
    }

    ui::heading("Summary");
    if blockers == 0 {
        println!("  No blockers.");
    } else {
        println!("  {blockers} blocker(s).");
    }
    println!("\n  Event log: {}", app.log.path().display());
    Ok(if blockers == 0 { 0 } else { 2 })
}

// ---------------------------------------------------------------------------
// monitors — the main view
// ---------------------------------------------------------------------------

pub fn monitors(app: &App) -> Result<i32> {
    let detected = app.backend.discover().context("enumerating displays")?;
    let switchable: Vec<&DetectedMonitor> = detected
        .iter()
        .filter(|d| d.identity.transport.supports_input_switching())
        .collect();

    ui::heading(&format!("Monitors ({})", app.backend.name()));
    if switchable.is_empty() && app.config.is_none() {
        println!("  No monitor on this computer can have its input switched.");
        return Ok(1);
    }

    for monitor in &switchable {
        let identity = &monitor.identity;
        let configured = app
            .config
            .as_ref()
            .and_then(|c| find_configured(c, identity));

        println!();
        println!(
            "{}",
            configured
                .map(|m| m.label.clone())
                .unwrap_or_else(|| identity.describe())
        );
        match configured {
            Some(cfg) => ui::field("name it by", &cfg.key),
            None => ui::field(
                "not configured",
                "run `configure` to name it and list its inputs",
            ),
        }
        ui::field("connection", &identity.backend_id);

        let current = app.backend.read_input(&monitor.handle()).ok();
        match &current {
            Some(InputReading::Value(code)) => {
                ui::field("current input", InputReading::Value(*code).to_string())
            }
            Some(other) => ui::field("current input", other.to_string()),
            None => ui::field("current input", "could not be read"),
        }

        let inputs = known_inputs(app, monitor, configured);
        if inputs.is_empty() {
            ui::field("inputs", "none reported");
        } else {
            let mut first = true;
            for input in &inputs {
                let marker = if current
                    .as_ref()
                    .and_then(|r| r.value())
                    .is_some_and(|c| c == input.input_code)
                {
                    "  <- now"
                } else {
                    ""
                };
                let line = format!("{}  {}{}", input.input_code, input.display_name(), marker);
                if first {
                    ui::field("inputs", line);
                    first = false;
                } else {
                    ui::field_cont(line);
                }
            }
        }

        if let Some(cfg) = configured {
            ui::field_cont(format!(
                "set with: desktop-switcher set {} 0xNN",
                shell_quote(&cfg.key)
            ));
        }
    }

    // Configured panels that did not answer are still yours, and hiding them
    // makes a monitor look like it stopped existing when it has only stopped
    // talking. Each one says which of the two it is.
    let silent = unanswered(app, &switchable, &detected);
    for (cfg, place) in &silent {
        println!();
        println!("{}", cfg.label);
        ui::field("name it by", &cfg.key);
        ui::field("connection", &cfg.backend_id);
        ui::field(
            "current input",
            match place {
                Whereabouts::Attached => "not answering",
                Whereabouts::Detached => "turned off in display settings",
                Whereabouts::Absent => "not present",
            },
        );

        let mut first = true;
        for input in &cfg.inputs {
            let line = format!("{}  {}", input.input_code, input.display_name());
            if first {
                ui::field("inputs", line);
                first = false;
            } else {
                ui::field_cont(line);
            }
        }

        let why = match place {
            Whereabouts::Attached => {
                "This computer is drawing on it, so the cable is fine, but it did not \
                 answer. A monitor asleep on an input with no picture ignores every \
                 command. Wake the computer on that input, or use the monitor's own \
                 buttons."
            }
            Whereabouts::Detached => {
                "It is plugged in, but turned off in this computer's display settings, \
                 so there is no way to talk to it. Turn it on in Settings > System > \
                 Display."
            }
            Whereabouts::Absent => {
                "Nothing on this computer matches it right now, so no command here can \
                 reach it."
            }
        };
        let mut first = true;
        for line in textwrap(why, 56) {
            if first {
                ui::field("why", line);
                first = false;
            } else {
                ui::field_cont(line);
            }
        }
    }

    let duplicates = inventory::duplicate_serials(&detected);
    if !duplicates.is_empty() {
        println!();
        ui::warn(format!(
            "serial(s) {} appear on more than one display; those panels cannot be told apart",
            duplicates.join(", ")
        ));
    }
    Ok(0)
}

/// Where a configured monitor the backend did not list actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Whereabouts {
    /// Part of this desktop, yet the backend did not list it.
    Attached,
    /// Plugged in, but turned off in this computer's display settings.
    Detached,
    /// Nothing on this computer matches it.
    Absent,
}

fn whereabouts(at_os_level: &[switcher_desktop::DesktopDisplay], backend_id: &str) -> Whereabouts {
    let matching: Vec<_> = at_os_level
        .iter()
        .filter(|d| d.matches_backend_id(backend_id))
        .collect();
    if matching.iter().any(|d| d.is_attached) {
        Whereabouts::Attached
    } else if matching.is_empty() {
        Whereabouts::Absent
    } else {
        Whereabouts::Detached
    }
}

/// Configured monitors the backend did not list, each with where it really is.
fn unanswered<'a>(
    app: &'a App,
    switchable: &[&DetectedMonitor],
    detected: &[DetectedMonitor],
) -> Vec<(&'a MonitorConfig, Whereabouts)> {
    let Some(config) = app.config.as_ref() else {
        return Vec::new();
    };
    let missing: Vec<&MonitorConfig> = config
        .monitors
        .iter()
        .filter(|cfg| {
            !switchable
                .iter()
                .any(|d| find_configured(config, &d.identity).is_some_and(|c| c.key == cfg.key))
        })
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    let at_os_level = desktop::manager(detected).displays().unwrap_or_default();
    missing
        .into_iter()
        .map(|cfg| (cfg, whereabouts(&at_os_level, &cfg.backend_id)))
        .collect()
}

/// Configured inputs when we have them, otherwise whatever the monitor says.
fn known_inputs(
    app: &App,
    monitor: &DetectedMonitor,
    configured: Option<&MonitorConfig>,
) -> Vec<MonitorInput> {
    match configured {
        Some(cfg) if !cfg.inputs.is_empty() => cfg.inputs.clone(),
        _ => app
            .backend
            .input_capabilities(&monitor.handle())
            .map(|caps| {
                caps.options
                    .iter()
                    .map(|o| MonitorInput::reported(o.code, o.label.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn find_configured<'a>(
    config: &'a Config,
    identity: &MonitorIdentity,
) -> Option<&'a MonitorConfig> {
    config
        .monitors
        .iter()
        .find(|m| m.serial.is_some() && m.serial == identity.serial)
        .or_else(|| {
            config
                .monitors
                .iter()
                .find(|m| m.backend_id == identity.backend_id)
        })
}

fn shell_quote(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// set — the core command
// ---------------------------------------------------------------------------

pub fn set_input(
    app: &App,
    monitor: &str,
    code_text: &str,
    dry_run: bool,
    force: bool,
) -> Result<i32> {
    let config = require_config(app)?;
    let code: InputCode = code_text.parse().map_err(|e| anyhow!("{e}"))?;
    let request = format!("set {monitor} {code}");

    if !dry_run && !force {
        let last = LastRequest::load(&app.state_dir);
        if last.is_duplicate_of(&request, Duration::from_millis(config.dedupe_ms)) {
            println!("Already requested that a moment ago; ignoring the repeat.");
            return Ok(0);
        }
    }

    // Held only for a one-off request. Inside an action the whole sequence is
    // already guarded, and taking it again here would deadlock against it.
    let _guard = if dry_run || force {
        None
    } else {
        match SwitchGuard::acquire(&app.state_dir, LOCK_STALE_AFTER) {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("error: {e}");
                return Ok(1);
            }
        }
    };

    let detected = app.backend.discover().context("enumerating displays")?;
    let plan = match switch::plan_set_input(config, &detected, monitor, code) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Refusing to change the input.\n\n{e}");
            ui::eprint_recovery_note();
            return Ok(2);
        }
    };

    if !plan.advertised {
        ui::warn(format!(
            "{code} is not in this monitor's advertised input list. That can still be \
             correct, since capability strings are often wrong, but it may do nothing \
             or blank the screen."
        ));
    }

    // No warning about whether anyone has watched this input work. This runs
    // from hotkeys, where nobody reads output and nobody can be asked
    // anything, and a warning that nothing short of editing the config can
    // clear is just noise. What happens when an input has no picture behind
    // it is documented instead.

    let opts = ExecOptions {
        settle: Duration::from_millis(config.settle_ms),
        read_back: true,
        dry_run,
    };
    let step = switch::apply_one(&plan, app.backend.as_ref(), opts, &app.log);
    if !dry_run {
        LastRequest::record(&app.state_dir, &request);
        learn_from(app, monitor, code, &step);
    }

    let report = ApplyReport {
        steps: vec![step],
        dry_run,
    };
    print_report(&report);
    Ok(report.exit_code())
}

/// Flip a monitor between two inputs, then set it exactly as `set` would.
///
/// The decision is the only new thing here: which of the two to set, from
/// what the monitor reports. Everything after that — binding, the write, the
/// read-back, the report — is `set_input`, so a toggle is held to the same
/// rules as any other switch.
pub fn toggle_input(
    app: &App,
    monitor: &str,
    between: [InputCode; 2],
    dry_run: bool,
    force: bool,
) -> Result<i32> {
    let config = require_config(app)?;
    let detected = app.backend.discover().context("enumerating displays")?;
    let plan = match switch::plan_set_input(config, &detected, monitor, between[0]) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Refusing to change the input.\n\n{e}");
            ui::eprint_recovery_note();
            return Ok(2);
        }
    };
    let reported = app
        .backend
        .read_input(&plan.handle)
        .ok()
        .and_then(|reading| reading.value());
    let target = switch::toggle_target(reported, between);
    println!(
        "  {} reports {}; toggling to {target}",
        plan.label,
        reported.map_or_else(|| "nothing readable".to_string(), |c| c.to_string())
    );
    set_input(app, monitor, &target.to_string(), dry_run, force)
}

/// Record the one thing a switch can teach us about an input unattended.
///
/// A monitor that goes silent right after a write has moved somewhere that is
/// not this computer — possibly an input with nothing on it — and that is worth
/// remembering before someone sends it there again by reflex.
///
/// A monitor that reads the value back teaches us nothing, and is deliberately
/// not recorded. The read returns what the monitor stored, and "stored it and
/// moved to a live input" looks identical to "stored it and stayed where it
/// was". One panel was recorded as `write-confirmed` on DVI, a socket it does
/// not have, which then silenced the warning about unverified inputs. Only a
/// person watching the screen can raise an input's evidence.
///
/// This is best effort: failing to record it must never turn a successful
/// switch into an error.
fn learn_from(app: &App, monitor: &str, code: InputCode, step: &switch::StepReport) {
    let Some(config) = app.config.as_ref() else {
        return;
    };
    let Some(existing) = config.monitor(monitor).and_then(|m| m.input(code)).cloned() else {
        return;
    };

    let vanished = matches!(
        step.after,
        Some(InputReading::UnavailableAfterSwitch { .. })
    );

    let mut updated = existing.clone();
    if vanished {
        updated.note = Some(format!(
            "On {}, the monitor stopped answering after this was selected. That is \
             normal if another computer is on it, and is what it looks like when \
             nothing is.",
            eventlog::today()
        ));
    } else {
        return;
    }
    if updated == existing {
        return;
    }

    let mut next = config.clone();
    if let Some(m) = next.monitor_mut(monitor) {
        if let Some(slot) = m.input_mut(code) {
            *slot = updated;
        }
    }
    if let Err(e) = next.save(&app.config_path) {
        ui::warn(format!("could not record what that switch showed: {e}"));
    }
}

fn print_report(report: &ApplyReport) {
    for step in &report.steps {
        let detail = if report.dry_run {
            "would be set (dry run)".to_string()
        } else {
            step.outcome.to_string()
        };
        println!("  {:<26} {} -> {}", step.label, step.requested, detail);
        if let Some(after) = &step.after {
            if !matches!(after, InputReading::Value(_)) {
                println!("  {:<26}   read-back: {after}", "");
            }
        }
    }
    if !report.dry_run {
        println!("\n  {}", report.status());
        if report.exit_code() != 0 {
            ui::eprint_recovery_note();
        }
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub fn status(app: &App) -> Result<i32> {
    let config = require_config(app)?;
    let detected = app.backend.discover().context("enumerating displays")?;
    let bindings = inventory::bind(config, &detected);

    ui::heading("Observed now");
    for binding in &bindings.bound {
        let reading = app
            .backend
            .read_input(&binding.handle())
            .unwrap_or_else(|e| InputReading::ReadFailed {
                detail: e.to_string(),
            });
        let named = reading
            .value()
            .and_then(|c| binding.monitor.input(c))
            .map(|i| format!("  ({})", i.display_name()))
            .unwrap_or_default();
        println!("  {:<26} {reading}{named}", binding.monitor.label);
    }
    for problem in &bindings.problems {
        println!("  {problem}");
    }

    ui::heading("Last requested by this tool");
    println!("  This is intent, not proof. Inputs can also be changed with the monitor");
    println!("  buttons, or by any other computer attached to them.\n");
    match LastRequest::load(&app.state_dir).request {
        Some(r) => println!("  {r}"),
        None => println!("  (nothing recorded yet)"),
    }

    Ok(if bindings.problems.is_empty() { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

pub fn actions(app: &App) -> Result<i32> {
    let config = require_config(app)?;
    ui::heading("Actions");
    if config.actions.is_empty() {
        println!("  None yet. Add them in the GUI, or by hand in");
        println!("  {}", app.config_path.display());
        return Ok(0);
    }
    for action in &config.actions {
        println!();
        println!(
            "{}{}",
            action.name,
            action
                .hotkey
                .as_deref()
                .map(|h| format!("   [{h}]"))
                .unwrap_or_else(|| "   (no hotkey)".into())
        );
        for step in &action.steps {
            ui::bullet(step.summary());
        }
        if action.needs_experimental() {
            ui::field_cont("needs --experimental to run");
        }
    }
    println!("\nRun one with: desktop-switcher run-action \"<name>\"");
    Ok(0)
}

/// Execute a named action, stopping at the first failure.
///
/// Later steps generally assume the earlier ones happened, so carrying on
/// after a failure would produce a state nobody asked for.
pub fn run_action(app: &App, name: &str) -> Result<i32> {
    let config = require_config(app)?;
    let action = config
        .action(name)
        .ok_or_else(|| {
            anyhow!(
                "no action called `{name}`. Configured: {}",
                if config.actions.is_empty() {
                    "none".to_string()
                } else {
                    config
                        .actions
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )
        })?
        .clone();

    action.validate().map_err(|e| anyhow!("{e}"))?;
    if action.needs_experimental() && !app.experimental {
        bail!(
            "`{}` includes a display-topology step, which is not reliable yet. \
             Pass --experimental to run it anyway.",
            action.name
        );
    }

    // Deduplicate and lock the action as a whole, so a held hotkey cannot
    // replay it and two actions cannot interleave on the same DDC lines.
    let last = LastRequest::load(&app.state_dir);
    if last.is_duplicate_of(&action.name, Duration::from_millis(config.dedupe_ms)) {
        println!("`{}` ran a moment ago; ignoring the repeat.", action.name);
        return Ok(0);
    }
    let _guard = match SwitchGuard::acquire(&app.state_dir, LOCK_STALE_AFTER) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return Ok(1);
        }
    };
    LastRequest::record(&app.state_dir, &action.name);

    app.log.append(
        "action.begin",
        &[
            ("name", action.name.clone()),
            ("steps", action.steps.len().to_string()),
        ],
    );
    ui::heading(&action.name);

    for (index, step) in action.steps.iter().enumerate() {
        println!(
            "\n[{}/{}] {}",
            index + 1,
            action.steps.len(),
            step.summary()
        );
        let code = run_step(app, step)?;
        if code != 0 {
            app.log.append(
                "action.stop",
                &[("name", action.name.clone()), ("step", step.summary())],
            );
            eprintln!(
                "\nStopped: step {} of {} failed, so the rest were not run.",
                index + 1,
                action.steps.len()
            );
            return Ok(code);
        }
    }

    app.log
        .append("action.end", &[("name", action.name.clone())]);
    println!("\nDone.");
    Ok(0)
}

fn run_step(app: &App, step: &ActionStep) -> Result<i32> {
    match step {
        // `force` because the action already holds the lock and has been
        // deduplicated; taking either again here would fight itself.
        ActionStep::SetInput { monitor, code } => {
            set_input(app, monitor, &code.to_string(), false, true)
        }
        ActionStep::ToggleInput { monitor, between } => {
            toggle_input(app, monitor, *between, false, true)
        }
        ActionStep::Sweep { monitor } => desktop::sweep(app, monitor.as_deref()),
        ActionStep::RestoreWindows { monitor } => desktop::restore_windows(app, monitor.as_deref()),
        ActionStep::Release { monitor } => desktop::release(app, monitor.as_deref()),
        ActionStep::Claim { monitor } => desktop::claim(app, monitor.as_deref()),
        ActionStep::Primary { monitor } => desktop::set_primary(app, monitor.as_deref()),
        ActionStep::Run { command, args } => {
            // Started, not awaited: these launch programs, and blocking a
            // hotkey until the user closes one would be wrong.
            match std::process::Command::new(command).args(args).spawn() {
                Ok(child) => {
                    println!("  started (pid {})", child.id());
                    Ok(0)
                }
                Err(e) => {
                    eprintln!("  could not start `{command}`: {e}");
                    Ok(1)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// configure
// ---------------------------------------------------------------------------

pub fn configure(app: &mut App) -> Result<i32> {
    let detected = app.backend.discover().context("enumerating displays")?;
    let switchable: Vec<&DetectedMonitor> = detected
        .iter()
        .filter(|d| d.identity.transport.supports_input_switching())
        .collect();

    if switchable.is_empty() {
        bail!("No monitor on this computer can have its input switched. Run `doctor` first.");
    }
    let duplicates = inventory::duplicate_serials(&detected);
    if !duplicates.is_empty() {
        bail!(
            "Serial(s) {} appear on more than one display. Those panels cannot be told \
             apart, so binding them would address the wrong screen.",
            duplicates.join(", ")
        );
    }

    ui::heading("Configure");
    println!("This records which monitors exist and what inputs they offer.");
    println!("It writes nothing to a monitor.");
    println!("\nPress Enter to accept any suggestion in [brackets].");

    let existing = app.config.clone();
    let mut config = Config::new(system_hostname(), app.backend_kind);
    if let Some(prev) = &existing {
        config.timeout_seconds = prev.timeout_seconds;
        config.settle_ms = prev.settle_ms;
        config.dedupe_ms = prev.dedupe_ms;
        config.tool_path = prev.tool_path.clone();
        config.actions = prev.actions.clone();
    }

    for monitor in &switchable {
        let identity = &monitor.identity;
        let previous = existing.as_ref().and_then(|c| find_configured(c, identity));

        println!("\n---");
        println!("  {}", identity.describe());
        println!("  connection: {}", identity.backend_id);
        if let Ok(InputReading::Value(code)) = app.backend.read_input(&monitor.handle()) {
            println!("  currently on: {}", InputReading::Value(code));
        }

        if !ui::confirm("  Manage this monitor?")? {
            println!("  Skipped.");
            continue;
        }

        let suggested = previous
            .map(|p| p.label.clone())
            .unwrap_or_else(|| identity.describe());
        let label = ui::ask("  A name for it", Some(&suggested))?;

        // Start from what the monitor advertises, keeping any labels the user
        // already chose for those inputs.
        let mut inputs: Vec<MonitorInput> = Vec::new();
        match app.backend.input_capabilities(&monitor.handle()) {
            Ok(caps) if !caps.options.is_empty() => {
                for option in &caps.options {
                    let kept = previous.and_then(|p| p.input(option.code));
                    inputs.push(MonitorInput {
                        input_code: option.code,
                        label: kept
                            .and_then(|k| k.label.clone())
                            .or_else(|| option.label.clone()),
                        verification: kept.map(|k| k.verification).unwrap_or(Evidence::Reported),
                        note: kept.and_then(|k| k.note.clone()),
                    });
                }
                println!("  Inputs it reports:");
                for input in &inputs {
                    println!("    {}  {}", input.input_code, input.display_name());
                }
                println!("  (a claim by the monitor — some listed inputs may not exist)");
            }
            _ => {
                println!("  It reported no input list; codes can be added by hand later.");
                if let Some(p) = previous {
                    inputs = p.inputs.clone();
                }
            }
        }

        let key = identity
            .serial
            .clone()
            .unwrap_or_else(|| identity.backend_id.clone());

        config.monitors.push(MonitorConfig {
            key,
            label,
            backend_id: identity.backend_id.clone(),
            serial: identity.serial.clone(),
            model: identity.model.clone(),
            inputs,
        });
    }

    if config.monitors.is_empty() {
        bail!("No monitor was selected, so there is nothing to save.");
    }

    // Drop actions that refer to a monitor skipped this time round, rather
    // than saving a configuration that fails when a hotkey is pressed.
    let monitors = config.monitors.clone();
    config.actions.retain(|action| {
        action.steps.iter().all(|s| match s.monitor_name() {
            None => true,
            Some(name) => name.is_empty() || monitors.iter().any(|m| m.matches(name)),
        })
    });

    config.save(&app.config_path)?;
    app.config = Some(config);

    println!("\nSaved to {}", app.config_path.display());
    println!("\nNext: `desktop-switcher monitors` lists the inputs, and");
    println!("`desktop-switcher set <monitor> 0xNN` changes one.");
    Ok(0)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn require_config(app: &App) -> Result<&Config> {
    app.config.as_ref().ok_or_else(|| {
        anyhow!(
            "no configuration at {}. Run `desktop-switcher configure` first.",
            app.config_path.display()
        )
    })
}

/// This machine's hostname, used only for display. Never prompted for: it is
/// not something anyone should have to invent.
fn system_hostname() -> String {
    let from_env = if cfg!(windows) {
        std::env::var("COMPUTERNAME").ok()
    } else {
        std::env::var("HOSTNAME").ok()
    };
    from_env
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this-computer".to_string())
}

/// Wrap text for the indented remedy lines in `doctor`.
fn textwrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textwrap_breaks_on_word_boundaries() {
        let lines = textwrap("one two three four five six seven", 12);
        assert!(lines.iter().all(|l| l.chars().count() <= 12), "{lines:?}");
        assert_eq!(lines.join(" "), "one two three four five six seven");
    }

    #[test]
    fn textwrap_handles_empty_and_single_long_words() {
        assert!(textwrap("", 10).is_empty());
        assert_eq!(
            textwrap("supercalifragilistic", 5),
            vec!["supercalifragilistic"]
        );
    }

    #[test]
    fn keys_with_spaces_are_quoted_in_suggested_commands() {
        assert_eq!(shell_quote("SN-1"), "SN-1");
        assert_eq!(shell_quote("Left monitor"), "\"Left monitor\"");
    }
}
