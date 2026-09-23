//! End-to-end tests of the command surface, driven through the real binary
//! against the in-memory backend. No hardware is touched.
//!
//! The property most of these protect: a command either has a fully
//! identified monitor and writes, or it refuses and writes nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_desktop-switcher");

/// A throwaway directory for one test's config and state.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("desktop-switcher-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create sandbox");
        Self { dir }
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    fn write_config(&self, text: &str) {
        std::fs::write(self.config_path(), text).expect("write config");
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(["--backend", "fake", "--config"])
            .arg(self.config_path())
            .args(args)
            .env("DESKTOP_SWITCHER_STATE_DIR", &self.dir)
            .output()
            .expect("run desktop-switcher")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn combined(out: &Output) -> String {
    format!("{}{}", stdout_of(out), stderr_of(out))
}

/// Two monitors, each with two inputs. Nothing names a computer.
fn config() -> &'static str {
    r#"
schema_version = 2
host = "test-host"
backend = "fake"
timeout_seconds = 5
settle_ms = 0
dedupe_ms = 0

[[monitors]]
key = "FAKE-SN-L"
label = "Left panel"
backend_id = "fake-left"
serial = "FAKE-SN-L"
model = "27P2DG5"

[[monitors.inputs]]
input_code = "0x11"
label = "HDMI-1"

[[monitors.inputs]]
input_code = "0x0F"
label = "DisplayPort-1"

[[monitors]]
key = "FAKE-SN-R"
label = "Right panel"
backend_id = "fake-right"
serial = "FAKE-SN-R"
model = "27P2DG5"

[[monitors.inputs]]
input_code = "0x11"
label = "HDMI-1"

[[monitors.inputs]]
input_code = "0x0F"
label = "DisplayPort-1"

[[actions]]
name = "Desk setup"
hotkey = "CTRL+ALT+1"

[[actions.steps]]
kind = "set-input"
monitor = "FAKE-SN-L"
code = "0x0F"

[[actions.steps]]
kind = "set-input"
monitor = "FAKE-SN-R"
code = "0x0F"
"#
}

#[test]
fn monitors_lists_each_panel_and_its_inputs() {
    let sandbox = Sandbox::new("monitors");
    sandbox.write_config(config());
    let out = sandbox.run(&["monitors"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    // Both panels appear separately, each with its inputs.
    assert!(text.contains("Left panel"), "{text}");
    assert!(text.contains("Right panel"), "{text}");
    assert!(text.contains("0x11"), "{text}");
    assert!(text.contains("0x0F"), "{text}");
}

#[test]
fn the_interface_never_names_a_computer() {
    // The whole point of the schema change: this tool describes monitors and
    // inputs, and leaves the meaning of an input to the person.
    let sandbox = Sandbox::new("no-pc-names");
    sandbox.write_config(config());
    for command in [&["monitors"][..], &["status"][..], &["actions"][..]] {
        let text = combined(&sandbox.run(command)).to_lowercase();
        for forbidden in ["windows machine", "ubuntu", "destination"] {
            assert!(
                !text.contains(forbidden),
                "`{command:?}` mentioned `{forbidden}`:\n{text}"
            );
        }
    }
}

#[test]
fn set_changes_the_named_monitor() {
    let sandbox = Sandbox::new("set");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "FAKE-SN-L", "0x0F"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        stdout_of(&out).contains("Left panel"),
        "{}",
        stdout_of(&out)
    );
}

#[test]
fn a_monitor_can_be_named_by_its_label() {
    let sandbox = Sandbox::new("by-label");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "Right panel", "0x0F"]);
    assert!(out.status.success(), "{}", combined(&out));
}

/// The write goes out even when the monitor claims to be on that input
/// already. A read can be stale exactly when it matters — a panel physically on
/// HDMI once reported DisplayPort, the set was skipped, and there was no way
/// back — so a matching read is never a reason to stay quiet.
#[test]
fn setting_the_input_it_is_already_on_still_writes() {
    let sandbox = Sandbox::new("idempotent");
    sandbox.write_config(config());
    // The fake left panel starts on 0x11.
    let out = sandbox.run(&["set", "FAKE-SN-L", "0x11"]);
    assert!(out.status.success(), "{}", combined(&out));
    let text = stdout_of(&out);
    assert!(!text.contains("already on that input"), "{text}");
    assert!(text.contains("stored 0x11"), "{text}");
}

/// A read-back is the monitor quoting its own register, so it must never be
/// written into the configuration as evidence about the input.
///
/// A real panel stored 0x03 — DVI, a socket it does not have — stayed awake
/// on the input it was already showing, read 0x03 back, and got recorded as
/// `write-confirmed`.
#[test]
fn a_read_back_is_never_recorded_as_evidence() {
    let sandbox = Sandbox::new("no-learning");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "FAKE-SN-L", "0x0F"]);
    assert!(out.status.success(), "{}", combined(&out));

    let saved = std::fs::read_to_string(sandbox.config_path()).expect("read config");
    assert!(!saved.contains("write-confirmed"), "{saved}");
    assert!(!saved.contains("read-confirmed"), "{saved}");
}

/// This runs from hotkeys. Switching to a configured input says nothing it
/// would need a person to act on — no warning about whether anyone has
/// watched the input work, however it is recorded in the configuration.
#[test]
fn switching_to_a_configured_input_is_quiet() {
    let sandbox = Sandbox::new("quiet");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "FAKE-SN-L", "0x0F"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        !combined(&out).to_lowercase().contains("warning"),
        "{}",
        combined(&out)
    );
}

#[test]
fn a_bare_decimal_code_is_rejected_as_ambiguous() {
    let sandbox = Sandbox::new("bad-code");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "FAKE-SN-L", "11"]);
    assert!(!out.status.success());
    assert!(combined(&out).contains("0x"), "{}", combined(&out));
}

#[test]
fn an_unknown_monitor_lists_the_configured_ones() {
    let sandbox = Sandbox::new("unknown-monitor");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "nope", "0x11"]);
    assert_eq!(out.status.code(), Some(2), "{}", combined(&out));
    assert!(stderr_of(&out).contains("FAKE-SN-L"), "{}", combined(&out));
}

#[test]
fn a_monitor_that_is_not_attached_is_refused() {
    let sandbox = Sandbox::new("missing-monitor");
    sandbox.write_config(&config().replace("FAKE-SN-R", "SN-NOT-PRESENT"));
    let out = sandbox.run(&["set", "SN-NOT-PRESENT", "0x0F"]);
    assert_eq!(out.status.code(), Some(2), "{}", combined(&out));
    assert!(
        stderr_of(&out).contains("not attached"),
        "{}",
        combined(&out)
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let sandbox = Sandbox::new("dry-run");
    sandbox.write_config(config());
    let out = sandbox.run(&["set", "FAKE-SN-L", "0x0F", "--dry-run"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(stdout_of(&out).contains("dry run"), "{}", stdout_of(&out));
}

#[test]
fn repeated_requests_within_the_dedupe_window_are_ignored() {
    let sandbox = Sandbox::new("dedupe");
    sandbox.write_config(&config().replace("dedupe_ms = 0", "dedupe_ms = 60000"));

    let first = sandbox.run(&["set", "FAKE-SN-L", "0x0F"]);
    assert!(first.status.success(), "{}", combined(&first));

    let second = sandbox.run(&["set", "FAKE-SN-L", "0x0F"]);
    assert!(
        stdout_of(&second).contains("ignoring the repeat"),
        "{}",
        combined(&second)
    );

    let forced = sandbox.run(&["set", "FAKE-SN-L", "0x0F", "--force"]);
    assert!(!stdout_of(&forced).contains("ignoring the repeat"));
}

#[test]
fn actions_are_listed_with_their_steps_and_hotkeys() {
    let sandbox = Sandbox::new("actions");
    sandbox.write_config(config());
    let out = sandbox.run(&["actions"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("Desk setup"), "{text}");
    assert!(text.contains("CTRL+ALT+1"), "{text}");
    assert!(text.contains("set FAKE-SN-L to 0x0F"), "{text}");
}

#[test]
fn an_action_runs_each_of_its_steps() {
    let sandbox = Sandbox::new("run-action");
    sandbox.write_config(config());
    let out = sandbox.run(&["run-action", "Desk setup"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("[1/2]"), "{text}");
    assert!(text.contains("[2/2]"), "{text}");
    assert!(text.contains("Done."), "{text}");
}

#[test]
fn an_action_can_be_named_case_insensitively() {
    let sandbox = Sandbox::new("action-case");
    sandbox.write_config(config());
    let out = sandbox.run(&["run-action", "desk SETUP"]);
    assert!(out.status.success(), "{}", combined(&out));
}

#[test]
fn an_unknown_action_lists_the_configured_ones() {
    let sandbox = Sandbox::new("unknown-action");
    sandbox.write_config(config());
    let out = sandbox.run(&["run-action", "nope"]);
    assert!(!out.status.success());
    assert!(combined(&out).contains("Desk setup"), "{}", combined(&out));
}

#[test]
fn an_action_with_an_experimental_step_refuses_without_the_flag() {
    let sandbox = Sandbox::new("experimental-action");
    sandbox.write_config(&format!(
        "{}\n[[actions.steps]]\nkind = \"release\"\nmonitor = \"FAKE-SN-L\"\n",
        config()
    ));
    let out = sandbox.run(&["run-action", "Desk setup"]);
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("--experimental"),
        "{}",
        combined(&out)
    );
}

#[test]
fn an_action_naming_an_unknown_monitor_is_rejected_when_the_config_loads() {
    // Better than failing when a hotkey is pressed.
    let sandbox = Sandbox::new("action-bad-monitor");
    sandbox.write_config(&config().replace(r#"monitor = "FAKE-SN-R""#, r#"monitor = "GONE""#));
    let out = sandbox.run(&["actions"]);
    assert!(!out.status.success());
    assert!(combined(&out).contains("GONE"), "{}", combined(&out));
}

#[test]
fn a_schema_1_configuration_is_upgraded_rather_than_rejected() {
    let sandbox = Sandbox::new("migrate");
    sandbox.write_config(
        r#"
schema_version = 1
host = "old-host"
backend = "fake"
self_destination = "windows"

[[monitors]]
logical_id = "left"
name = "Left panel"
backend_id = "fake-left"
serial = "FAKE-SN-L"

[monitors.destinations.windows]
input_code = "0x11"
verification = "user-confirmed"

[monitors.destinations.ubuntu]
input_code = "0x0F"
verification = "user-confirmed"
"#,
    );
    let out = sandbox.run(&["actions"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    // The old destinations survive as actions the user now owns and can rename.
    assert!(text.contains("windows"), "{text}");
    assert!(text.contains("ubuntu"), "{text}");
    assert!(text.contains("set FAKE-SN-L to"), "{text}");
}

#[test]
fn status_separates_live_readings_from_last_requested_intent() {
    let sandbox = Sandbox::new("status");
    sandbox.write_config(config());
    let out = sandbox.run(&["status"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("Observed now"), "{text}");
    assert!(text.contains("Last requested by this tool"), "{text}");
    assert!(
        text.contains("intent, not proof"),
        "intent must not be presented as state: {text}"
    );
}

#[test]
fn set_without_a_configuration_refuses_and_says_what_to_do() {
    let sandbox = Sandbox::new("no-config");
    let out = sandbox.run(&["set", "anything", "0x11"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("configure"), "{}", combined(&out));
}

#[test]
fn doctor_is_clean_with_a_valid_configuration() {
    let sandbox = Sandbox::new("doctor");
    sandbox.write_config(config());
    let out = sandbox.run(&["doctor"]);
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    assert!(stdout_of(&out).contains("No blockers"));
}

#[test]
fn release_refuses_without_the_experimental_flag() {
    let sandbox = Sandbox::new("release-gated");
    sandbox.write_config(config());
    let out = sandbox.run(&["release", "FAKE-SN-L"]);
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("--experimental"),
        "{}",
        combined(&out)
    );
}

#[test]
fn restore_windows_without_a_sweep_says_so_rather_than_failing() {
    let sandbox = Sandbox::new("restore-empty");
    sandbox.write_config(config());
    let out = sandbox.run(&["restore-windows"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        stdout_of(&out).contains("nothing to put back"),
        "{}",
        stdout_of(&out)
    );
}

#[test]
fn read_only_commands_never_create_a_configuration() {
    let sandbox = Sandbox::new("no-side-effects");
    for command in [&["monitors"][..], &["doctor"][..], &["displays"][..]] {
        let _ = sandbox.run(command);
        assert!(
            !Path::new(&sandbox.config_path()).exists(),
            "{command:?} created a config file"
        );
    }
}
