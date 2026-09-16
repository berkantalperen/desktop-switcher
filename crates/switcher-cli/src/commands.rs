//! Command implementations.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use switcher_core::backend::Severity;
use switcher_core::config::{Config, DestinationMapping, MonitorConfig};
use switcher_core::eventlog;
use switcher_core::inventory::{self, Binding};
use switcher_core::switch::{self, ExecOptions, LastRequest, SwitchGuard, SwitchReport};
use switcher_core::types::{DetectedMonitor, Evidence, InputCode, InputReading};

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
    ui::field(
        "tool path",
        health.tool_path.as_deref().unwrap_or("(not resolved)"),
    );
    ui::field(
        "tool version",
        health.tool_version.as_deref().unwrap_or("(unknown)"),
    );

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
            println!("         Run `desktop-switcher configure` to identify the monitors");
            println!("         and record verified input codes.");
            blockers += 1;
        }
        Some(config) => {
            ui::field("host", &config.host);
            ui::field("this computer is", &config.self_destination);
            ui::field("destinations", config.destinations().join(", "));
            ui::field("monitors", config.monitors.len().to_string());

            println!();
            for monitor in &config.monitors {
                for destination in config.destinations() {
                    match monitor.mapping(&destination) {
                        Some(m) if m.verification.is_trusted_for_switching() => println!(
                            "  [ok] {} -> {destination}: {} ({})",
                            monitor.logical_id, m.input_code, m.verification
                        ),
                        Some(m) => println!(
                            "  [warn] {} -> {destination}: {} is only {}; switching will refuse it",
                            monitor.logical_id, m.input_code, m.verification
                        ),
                        None => println!(
                            "  [warn] {} -> {destination}: not configured",
                            monitor.logical_id
                        ),
                    }
                }
            }

            // Binding is the check that actually gates a switch.
            ui::heading("Identification");
            match app.backend.discover() {
                Err(e) => {
                    println!("  [BLOCKER] Could not enumerate displays: {e}");
                    blockers += 1;
                }
                Ok(detected) => {
                    let duplicates = inventory::duplicate_serials(&detected);
                    if !duplicates.is_empty() {
                        println!(
                            "  [BLOCKER] Serial(s) {} appear on more than one display.",
                            duplicates.join(", ")
                        );
                        println!("         Every binding that relies on them is ambiguous.");
                        blockers += 1;
                    }
                    let bindings = inventory::bind(config, &detected);
                    for binding in &bindings.bound {
                        println!(
                            "  [ok] {} -> {}",
                            binding.logical_id(),
                            binding.detected.identity.describe()
                        );
                        for note in &binding.notes {
                            for line in textwrap(note, 72) {
                                println!("         {line}");
                            }
                        }
                    }
                    for problem in &bindings.problems {
                        println!("  [BLOCKER] {problem}");
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
        println!("  No blockers. `switch` should work.");
    } else {
        println!("  {blockers} blocker(s). `switch` will refuse until they are resolved.");
    }
    println!("\n  Event log: {}", app.log.path().display());

    Ok(if blockers == 0 { 0 } else { 2 })
}

// ---------------------------------------------------------------------------
// monitors
// ---------------------------------------------------------------------------

pub fn monitors(app: &App) -> Result<i32> {
    let detected = app.backend.discover().context("enumerating displays")?;

    ui::heading(&format!("Displays seen by {}", app.backend.name()));
    if detected.is_empty() {
        println!("  (none)");
        return Ok(1);
    }

    for monitor in &detected {
        let id = &monitor.identity;
        let logical = app
            .config
            .as_ref()
            .and_then(|c| {
                c.monitors
                    .iter()
                    .find(|m| m.serial.is_some() && m.serial == id.serial)
                    .or_else(|| c.monitors.iter().find(|m| m.backend_id == id.backend_id))
            })
            .map(|m| m.logical_id.clone());

        println!();
        println!(
            "{}{}",
            id.model.as_deref().unwrap_or("(unnamed display)"),
            logical
                .map(|l| format!("  [configured as `{l}`]"))
                .unwrap_or_default()
        );
        ui::field(
            "manufacturer",
            id.manufacturer.as_deref().unwrap_or("(unknown)"),
        );
        ui::field(
            "serial",
            id.serial
                .as_deref()
                .unwrap_or("(none published — cannot be told apart from an identical panel)"),
        );
        ui::field("connection", &id.backend_id);
        ui::field("transport", id.transport.to_string());
        ui::field(
            "input switching",
            if id.transport.supports_input_switching() {
                "supported"
            } else {
                "not available over this transport"
            },
        );
        if let Some(n) = id.discovery_index {
            ui::field(
                "discovery index",
                format!("{n}  (changes between runs; never used to address a write)"),
            );
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

// ---------------------------------------------------------------------------
// inspect
// ---------------------------------------------------------------------------

pub fn inspect(app: &App, target: Option<&str>) -> Result<i32> {
    let detected = app.backend.discover().context("enumerating displays")?;

    let selected: Vec<&DetectedMonitor> = match target {
        None => detected
            .iter()
            .filter(|d| d.identity.transport.supports_input_switching())
            .collect(),
        Some(name) => {
            let matched = resolve_detected(app, &detected, name)?;
            vec![matched]
        }
    };

    if selected.is_empty() {
        println!("No display supports input switching.");
        return Ok(1);
    }

    for monitor in selected {
        let identity = &monitor.identity;
        let logical = app.config.as_ref().and_then(|c| {
            c.monitors
                .iter()
                .find(|m| m.serial.is_some() && m.serial == identity.serial)
                .or_else(|| {
                    c.monitors
                        .iter()
                        .find(|m| m.backend_id == identity.backend_id)
                })
        });

        ui::heading(&format!(
            "{}{}",
            logical
                .map(|m| format!("{} — ", m.logical_id))
                .unwrap_or_default(),
            identity.describe()
        ));
        ui::field("connection", &identity.backend_id);

        // What the monitor claims. A claim, and labelled as one.
        let handle = monitor.handle();
        match app.backend.input_capabilities(&handle) {
            Ok(caps) if caps.feature_present && !caps.options.is_empty() => {
                let rendered: Vec<String> = caps.options.iter().map(|o| o.to_string()).collect();
                ui::field("reported input codes", rendered.join(", "));
                ui::field_cont(format!(
                    "[{}] the monitor's own claim; it may list inputs that",
                    Evidence::Reported
                ));
                ui::field_cont("do not exist or omit ones that do");
                if let Some(raw) = &caps.raw_capabilities {
                    ui::field_cont(format!("raw: {raw}"));
                }
            }
            Ok(_) => ui::field("reported input codes", "monitor did not advertise VCP 0x60"),
            Err(e) => ui::field("reported input codes", format!("unavailable: {e}")),
        }

        // What it reads right now.
        match app.backend.read_input(&handle) {
            Ok(InputReading::Value(code)) => {
                ui::field(
                    "last read current",
                    format!("{}", InputReading::Value(code)),
                );
                ui::field_cont(format!("[{}]", Evidence::ReadConfirmed));
            }
            Ok(other) => ui::field("last read current", other.to_string()),
            Err(e) => ui::field("last read current", format!("unavailable: {e}")),
        }

        // What we have actually proven by writing.
        let write_tested = logical
            .map(|m| {
                m.destinations
                    .values()
                    .any(|d| d.verification >= Evidence::WriteConfirmed)
            })
            .unwrap_or(false);
        ui::field(
            "write tested",
            if write_tested {
                "yes — at least one mapping was confirmed by a real switch"
            } else {
                "no — no input code on this monitor has been proven by writing"
            },
        );

        // What is configured.
        match logical {
            None => ui::field("configured mapping", "not configured"),
            Some(cfg) if cfg.destinations.is_empty() => {
                ui::field("configured mapping", "no destinations recorded")
            }
            Some(cfg) => {
                let mut first = true;
                for (destination, mapping) in &cfg.destinations {
                    let line = format!(
                        "{destination} -> {} [{}{}]",
                        mapping.input_code,
                        mapping.verification,
                        mapping
                            .verified_on
                            .as_deref()
                            .map(|d| format!(", {d}"))
                            .unwrap_or_default()
                    );
                    if first {
                        ui::field("configured mapping", line);
                        first = false;
                    } else {
                        ui::field_cont(line);
                    }
                }
            }
        }
    }

    println!();
    println!("Nothing above was written. To prove a code, use:");
    println!("  desktop-switcher test-input --monitor <id> --code 0xNN --destination <name>");
    Ok(0)
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub fn status(app: &App) -> Result<i32> {
    let config = require_config(app)?;
    let detected = app.backend.discover().context("enumerating displays")?;
    let bindings = inventory::bind(config, &detected);

    ui::heading("Observed now");
    let mut readings = Vec::new();
    for binding in &bindings.bound {
        let reading = app
            .backend
            .read_input(&binding.handle())
            .unwrap_or_else(|e| InputReading::ReadFailed {
                detail: e.to_string(),
            });
        let destination = match &reading {
            InputReading::Value(code) => matching_destination(binding, *code),
            _ => None,
        };
        println!(
            "  {:<10} {}{}",
            binding.logical_id(),
            reading,
            destination
                .map(|d| format!("  = {d}"))
                .unwrap_or_else(|| "  = not a configured destination".to_string())
        );
        readings.push((binding.logical_id().to_string(), reading));
    }
    for problem in &bindings.problems {
        println!("  {problem}");
    }

    ui::heading("Last requested by this tool");
    println!("  This is intent, not proof of what the monitors show. Inputs can also");
    println!("  be changed with the monitor buttons, or by the other computer.\n");
    let last = LastRequest::load(&app.state_dir);
    match last.destination {
        Some(d) => println!("  {d}"),
        None => println!("  (nothing recorded yet)"),
    }

    if !bindings.problems.is_empty() {
        return Ok(1);
    }
    Ok(0)
}

fn matching_destination(binding: &Binding<'_>, code: InputCode) -> Option<String> {
    let matches: Vec<&String> = binding
        .monitor
        .destinations
        .iter()
        .filter(|(_, m)| m.input_code == code)
        .map(|(name, _)| name)
        .collect();
    match matches.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// switch / toggle
// ---------------------------------------------------------------------------

pub fn switch(app: &App, destination: &str, dry_run: bool, force: bool) -> Result<i32> {
    let config = require_config(app)?;

    if !dry_run && !force {
        let last = LastRequest::load(&app.state_dir);
        if last.is_duplicate_of(destination, Duration::from_millis(config.dedupe_ms)) {
            println!("Already requested `{destination}` a moment ago; ignoring the repeat.");
            println!("Use --force to issue it anyway.");
            return Ok(0);
        }
    }

    // Serialise against another shortcut press or a second instance.
    let _guard = if dry_run {
        None
    } else {
        match SwitchGuard::acquire(&app.state_dir, LOCK_STALE_AFTER) {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!(
                    "Two switches must not drive the same monitors at once. Try again in a moment."
                );
                return Ok(1);
            }
        }
    };

    let detected = app.backend.discover().context("enumerating displays")?;
    let plan = match switch::plan(config, &detected, destination) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Refusing to switch.\n");
            eprintln!("{e}");
            ui::eprint_recovery_note();
            return Ok(2);
        }
    };

    for note in &plan.notes {
        ui::warn(note);
    }

    if dry_run {
        ui::heading(&format!("Dry run: switch to `{destination}`"));
        for write in &plan.writes {
            println!(
                "  {:<10} would write {} to {}",
                write.logical_id, write.code, write.handle.backend_id
            );
        }
        println!("\nNothing was written.");
        return Ok(0);
    }

    let opts = ExecOptions {
        settle: Duration::from_millis(config.settle_ms),
        read_back: true,
        dry_run: false,
    };
    let report = switch::execute(&plan, app.backend.as_ref(), opts, &app.log);
    if !LastRequest::record(&app.state_dir, destination) {
        ui::warn(format!(
            "could not record this request under {}; the repeat-press guard will not work",
            app.state_dir.display()
        ));
    }

    print_switch_report(&report);
    Ok(report.exit_code())
}

pub fn toggle(app: &App, dry_run: bool) -> Result<i32> {
    let config = require_config(app)?;
    let detected = app.backend.discover().context("enumerating displays")?;
    let bindings = inventory::bind(config, &detected);

    if !bindings.is_complete() {
        eprintln!("Refusing to toggle: not every monitor could be identified.\n");
        for problem in &bindings.problems {
            eprintln!("  - {problem}");
        }
        return Ok(2);
    }

    let mut readings = Vec::new();
    for binding in &bindings.bound {
        let reading = app
            .backend
            .read_input(&binding.handle())
            .unwrap_or_else(|e| InputReading::ReadFailed {
                detail: e.to_string(),
            });
        readings.push((binding.logical_id().to_string(), reading));
    }

    match switch::infer_toggle_target(config, &readings) {
        Ok(target) => {
            println!("Current state is unambiguous; switching to `{target}`.");
            switch(app, &target, dry_run, false)
        }
        Err(reason) => {
            eprintln!("Refusing to toggle.\n");
            eprintln!("  {reason}");
            eprintln!("\nCurrent readings:");
            for (id, reading) in &readings {
                eprintln!("  {id:<10} {reading}");
            }
            Ok(2)
        }
    }
}

fn print_switch_report(report: &SwitchReport) {
    ui::heading(&format!("Switch to `{}`", report.destination));
    for step in &report.steps {
        let detail = if step.skipped_already_correct {
            "already on the requested input; no write issued".to_string()
        } else {
            step.outcome.to_string()
        };
        println!("  {:<10} {} -> {}", step.logical_id, step.requested, detail);
        if let Some(after) = &step.after {
            if !matches!(after, InputReading::Value(_)) {
                println!("  {:<10}   read-back: {after}", "");
            }
        }
    }
    println!("\n  {}", report.status());

    if report.exit_code() != 0 {
        ui::print_recovery_note();
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
        bail!("No display supports input switching, so there is nothing to configure. Run `desktop-switcher doctor` first.");
    }

    let duplicates = inventory::duplicate_serials(&detected);
    if !duplicates.is_empty() {
        bail!(
            "Serial(s) {} appear on more than one display. These panels cannot be told apart \
             reliably, so configuring them would produce bindings that silently address the \
             wrong screen. Resolve this before continuing.",
            duplicates.join(", ")
        );
    }

    ui::heading("Configure");
    println!("Nothing is written to a monitor by this command except where it");
    println!("asks you first, one monitor at a time.");
    println!("\nPress Enter to accept any suggestion in [brackets].");

    let existing = app.config.clone();

    // Two names, and they are the words typed at the command line. Asking
    // separately for a "host name" and a "destination name" only invited
    // people to give this computer the other computer's name.
    ui::heading("Naming the two computers");
    println!("These are the words you will type to switch, as in");
    println!("`desktop-switcher switch <name>`. Short and lowercase is easiest.\n");

    let default_self = existing
        .as_ref()
        .map(|c| c.self_destination.clone())
        .unwrap_or_else(|| if cfg!(windows) { "windows" } else { "ubuntu" }.to_string());
    let self_destination = ui::ask(
        &format!(
            "  Name for THIS computer, the one you are typing on ({})",
            system_hostname()
        ),
        Some(&default_self),
    )?;

    let default_other = existing
        .as_ref()
        .and_then(|c| {
            c.destinations()
                .into_iter()
                .find(|d| *d != self_destination)
        })
        .unwrap_or_else(|| {
            if self_destination == "windows" {
                "ubuntu"
            } else {
                "windows"
            }
            .to_string()
        });
    let other_destination = ui::ask("  Name for the OTHER computer", Some(&default_other))?;

    if other_destination == self_destination {
        bail!("Both computers cannot be called `{self_destination}`. Run configure again and give them different names.");
    }
    println!("\n  `switch {self_destination}` will bring the monitors here.");
    println!("  `switch {other_destination}` will send them to the other computer.");

    // Only used for display and logs, so there is no reason to ask.
    let host = system_hostname();

    let mut config = Config::new(host, app.backend_kind, self_destination.clone());
    if let Some(prev) = &existing {
        config.timeout_seconds = prev.timeout_seconds;
        config.settle_ms = prev.settle_ms;
        config.dedupe_ms = prev.dedupe_ms;
    }

    println!(
        "\nIdentifying displays. There are {} to place.",
        switchable.len()
    );
    println!("Each is listed with its serial number, which is printed on the back of");
    println!("the monitor and is the only thing that tells two identical panels apart.\n");

    let mut used_ids: Vec<String> = Vec::new();
    for monitor in &switchable {
        let identity = &monitor.identity;
        println!("---");
        println!("  {}", identity.describe());
        println!("  connection: {}", identity.backend_id);
        if let Ok(InputReading::Value(code)) = app.backend.read_input(&monitor.handle()) {
            println!("  currently showing input: {}", InputReading::Value(code));
        }

        // A display that is only ever cabled to one computer does not need
        // managing, and configuring it would make every switch to the other
        // computer refuse on its behalf. Skipping leaves it alone; `plan`
        // still reports it as attached-but-not-configured.
        if !ui::confirm("  Manage this display with desktop-switcher?")? {
            println!("  Skipped. It will be left exactly as it is.");
            continue;
        }

        // With two identical panels the useful label is where they sit; with
        // one, asking for a "physical position" is just puzzling.
        let only_one = switchable.len() == 1;
        let suggested = if only_one {
            "main"
        } else if used_ids.is_empty() {
            "left"
        } else {
            "right"
        };
        let question = if only_one {
            "  A short label for this display, used as `--monitor <label>`"
        } else {
            "  Where does this display physically sit (e.g. left, right)?"
        };
        let logical_id = loop {
            let answer = ui::ask(question, Some(suggested))?;
            if used_ids.contains(&answer) {
                println!("  (`{answer}` is already used)");
                continue;
            }
            break answer;
        };
        used_ids.push(logical_id.clone());

        let mut monitor_config = MonitorConfig {
            logical_id: logical_id.clone(),
            name: format!(
                "{} {}",
                identity.manufacturer.as_deref().unwrap_or("Display"),
                identity.model.as_deref().unwrap_or("")
            )
            .trim()
            .to_string(),
            backend_id: identity.backend_id.clone(),
            serial: identity.serial.clone(),
            model: identity.model.clone(),
            destinations: Default::default(),
        };

        // One mapping can always be established without writing anything:
        // read the current input, and have the user say which computer that
        // input is actually showing. It does not have to be this one --
        // being told "that is the other computer" is just as good, and is the
        // normal case when configuring from the machine that is idle.
        match app.backend.read_input(&monitor.handle()) {
            Ok(InputReading::Value(code)) => {
                println!(
                    "\n  This display currently reports input {}.",
                    InputReading::Value(code)
                );
                let choice = ui::choose(
                    "  Which computer is it actually showing right now?",
                    &[
                        format!("{self_destination} (this computer)"),
                        format!("{other_destination} (the other computer)"),
                        "Something else, or I cannot tell".to_string(),
                    ],
                )?;
                match choice {
                    0 | 1 => {
                        let destination = if choice == 0 {
                            self_destination.clone()
                        } else {
                            other_destination.clone()
                        };
                        monitor_config.destinations.insert(
                            destination.clone(),
                            DestinationMapping {
                                input_code: code,
                                verification: Evidence::UserConfirmed,
                                verified_on: Some(eventlog::today()),
                                note: Some(
                                    "Read from the monitor while the user confirmed which computer it was displaying."
                                        .into(),
                                ),
                            },
                        );
                        println!(
                            "  Recorded: {destination} -> {code} (user-confirmed, no write needed)"
                        );
                    }
                    _ => println!("  Nothing recorded for this display yet."),
                }
            }
            Ok(other) => println!("\n  Could not read the current input: {other}"),
            Err(e) => println!("\n  Could not read the current input: {e}"),
        }

        // Whatever is still missing can only come from a real switch.
        let missing: Vec<String> = [&self_destination, &other_destination]
            .into_iter()
            .filter(|d| !monitor_config.destinations.contains_key(*d))
            .cloned()
            .collect();
        for destination in &missing {
            println!("\n  The input code for `{destination}` can only be established by switching");
            println!("  this monitor and watching what happens. That is a separate, deliberate");
            println!("  step:");
            println!(
                "    desktop-switcher test-input --monitor {logical_id} --code 0xNN --destination {destination}"
            );
        }
        if let Ok(caps) = app.backend.input_capabilities(&monitor.handle()) {
            if !caps.options.is_empty() {
                let rendered: Vec<String> = caps.options.iter().map(|o| o.to_string()).collect();
                println!("  This monitor claims to support: {}", rendered.join(", "));
                println!("  (a claim, not proof — some listed inputs may not exist)");
            }
        }

        config.monitors.push(monitor_config);
    }

    if config.monitors.is_empty() {
        bail!(
            "No display was selected for management, so there is nothing to save. \
             The existing configuration, if any, was left untouched."
        );
    }

    config.validate()?;
    config.save(&app.config_path)?;
    app.config = Some(config);

    println!("\nSaved to {}", app.config_path.display());
    println!("\nNext: run `desktop-switcher doctor`, which lists exactly which");
    println!("mappings are still missing, then use `test-input` to establish each");
    println!("one. `switch` refuses to run until every mapping is user-confirmed.");
    Ok(0)
}

// ---------------------------------------------------------------------------
// test-input: the Stage B verification tool
// ---------------------------------------------------------------------------

pub fn test_input(
    app: &mut App,
    monitor_name: &str,
    code_text: &str,
    destination: Option<&str>,
) -> Result<i32> {
    let code: InputCode = code_text
        .parse()
        .map_err(|e| anyhow!("{e}"))
        .context("parsing --code")?;

    let detected = app.backend.discover().context("enumerating displays")?;
    let target = resolve_detected(app, &detected, monitor_name)?.clone();
    let handle = target.handle();

    ui::heading("Single-monitor input test");
    ui::field("monitor", target.identity.describe());
    ui::field("connection", &target.identity.backend_id);
    ui::field("about to write", format!("VCP 0x60 = {code}"));

    match app.backend.input_capabilities(&handle) {
        Ok(caps) => {
            let rendered: Vec<String> = caps.options.iter().map(|o| o.to_string()).collect();
            ui::field("monitor claims", rendered.join(", "));
            if !caps.advertises(code) && !caps.options.is_empty() {
                println!();
                ui::warn(format!(
                    "{code} is NOT in this monitor's advertised list. That can still be correct \
                     (capability strings are often wrong), but it is more likely to do nothing \
                     or blank the screen."
                ));
            }
        }
        Err(e) => ui::field("monitor claims", format!("unavailable: {e}")),
    }

    let before = app.backend.read_input(&handle);
    match &before {
        Ok(reading) => ui::field("current input", reading.to_string()),
        Err(e) => ui::field("current input", format!("unavailable: {e}")),
    }

    println!("\nBefore continuing, make sure you can recover without this computer:");
    ui::print_recovery_note();

    println!();
    if !ui::confirm(&format!(
        "Write {code} to `{}` now?",
        target.identity.describe()
    ))? {
        println!("Nothing was written.");
        return Ok(0);
    }

    app.log.append(
        "test-input.write",
        &[
            ("monitor", target.identity.backend_id.clone()),
            ("code", code.to_string()),
        ],
    );

    let outcome = app.backend.set_input(&handle, code).unwrap_or_else(|e| {
        switcher_core::types::WriteOutcome::Failed {
            detail: e.to_string(),
        }
    });
    println!("\n  write outcome: {outcome}");

    std::thread::sleep(Duration::from_millis(
        app.config.as_ref().map(|c| c.settle_ms).unwrap_or(1200),
    ));

    match app.backend.read_input(&handle) {
        Ok(reading) => println!("  read-back:     {reading}"),
        Err(e) => println!("  read-back:     unavailable ({e})"),
    }
    println!(
        "\n  A read-back that fails here is expected if the monitor switched to the\n  \
         other computer: it stops answering this one."
    );

    // Only a human can settle what actually happened.
    println!();
    let observed = ui::choose(
        "What is that monitor physically showing now?",
        &[
            "The other computer".to_string(),
            "Still this computer, unchanged".to_string(),
            "Nothing — blank or 'no signal'".to_string(),
            "Something else".to_string(),
        ],
    )?;

    match observed {
        0 => println!("\n  Good: {code} selects the port the other computer is plugged into."),
        1 => {
            println!("\n  The write did not take effect. The monitor may not support this code,");
            println!("  or it may map to a port with nothing attached.");
            return Ok(1);
        }
        2 => {
            println!("\n  {code} selects a port with no live source.");
            println!("  Recover with the monitor's buttons, then try a different code.");
            return Ok(1);
        }
        _ => {
            println!("\n  Recording nothing, since the result is unclear.");
            return Ok(1);
        }
    }

    let Some(destination) = destination else {
        println!("\n  Not recorded: pass --destination <name> to save this as a verified mapping.");
        return Ok(0);
    };

    let Some(config) = app.config.as_mut() else {
        println!("\n  Not recorded: there is no configuration yet. Run `configure` first.");
        return Ok(0);
    };

    let Some(entry) = config.monitors.iter_mut().find(|m| {
        (m.serial.is_some() && m.serial == target.identity.serial)
            || m.backend_id == target.identity.backend_id
    }) else {
        println!(
            "\n  Not recorded: this display is not in the configuration. Run `configure` first."
        );
        return Ok(0);
    };

    entry.destinations.insert(
        destination.to_string(),
        DestinationMapping {
            input_code: code,
            verification: Evidence::UserConfirmed,
            verified_on: Some(eventlog::today()),
            note: Some("Confirmed by watching the physical monitor switch.".into()),
        },
    );
    let logical_id = entry.logical_id.clone();
    let config = config.clone();
    config.save(&app.config_path)?;

    println!("\n  Recorded: {logical_id} -> {destination} = {code} (user-confirmed)");
    println!("  Saved to {}", app.config_path.display());
    println!("\n  That monitor is now on the other computer. To bring it back, run");
    println!("  the switch command from that computer, or use the monitor's buttons.");
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

/// Resolve a user-supplied name to exactly one present display.
///
/// Accepts a configured logical id, a serial, or a backend id. Refuses rather
/// than picking one when the name is ambiguous.
fn resolve_detected<'a>(
    app: &App,
    detected: &'a [DetectedMonitor],
    name: &str,
) -> Result<&'a DetectedMonitor> {
    // A configured logical id is resolved through the binding rules, so it
    // gets the same serial corroboration a switch would.
    if let Some(config) = &app.config {
        if let Some(cfg) = config.monitor(name) {
            let bindings = inventory::bind(config, detected);
            if let Some(binding) = bindings.get(&cfg.logical_id) {
                let id = binding.detected.identity.backend_id.clone();
                return detected
                    .iter()
                    .find(|d| d.identity.backend_id == id)
                    .ok_or_else(|| anyhow!("`{name}` vanished between discovery and binding"));
            }
            if let Some(problem) = bindings
                .problems
                .iter()
                .find(|p| p.logical_id() == cfg.logical_id)
            {
                bail!("{problem}");
            }
        }
    }

    let matches: Vec<&DetectedMonitor> = detected
        .iter()
        .filter(|d| d.identity.backend_id == name || d.identity.serial.as_deref() == Some(name))
        .collect();

    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!(
            "no display matches `{name}`. Run `desktop-switcher monitors` to see what is attached."
        ),
        many => bail!(
            "`{name}` matches {} displays; refusing to guess which one you mean.",
            many.len()
        ),
    }
}

/// This machine's hostname, used only for display and logs.
///
/// Deliberately not a prompt: it is not something anyone should have to
/// invent, and asking for it right beside the switch names invited answering
/// it with the *other* computer's name.
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
}
