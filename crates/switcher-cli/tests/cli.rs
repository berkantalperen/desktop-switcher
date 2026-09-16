//! End-to-end tests of the command surface, driven through the real binary
//! against the in-memory backend. No hardware is touched.
//!
//! The property most of these protect is the same one: a command either has a
//! fully verified plan and writes, or it refuses and writes nothing.

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

/// The fake backend's two panels need different codes for the same computer,
/// matching the real hardware.
fn verified_config() -> &'static str {
    r#"
schema_version = 1
host = "test-host"
backend = "fake"
self_destination = "windows"
timeout_seconds = 5
settle_ms = 0
dedupe_ms = 0

[[monitors]]
logical_id = "left"
name = "AOC left"
backend_id = "fake-left"
serial = "FAKE-SN-L"
model = "27P2DG5"

[monitors.destinations.windows]
input_code = "0x11"
verification = "user-confirmed"

[monitors.destinations.ubuntu]
input_code = "0x0F"
verification = "user-confirmed"

[[monitors]]
logical_id = "right"
name = "AOC right"
backend_id = "fake-right"
serial = "FAKE-SN-R"
model = "27P2DG5"

[monitors.destinations.windows]
input_code = "0x0F"
verification = "user-confirmed"

[monitors.destinations.ubuntu]
input_code = "0x11"
verification = "user-confirmed"
"#
}

#[test]
fn monitors_lists_displays_and_flags_the_unswitchable_one() {
    let sandbox = Sandbox::new("monitors");
    let out = sandbox.run(&["monitors"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("FAKE-SN-L"), "{text}");
    assert!(text.contains("FAKE-SN-R"), "{text}");
    // The internal panel must be shown but marked unusable for switching.
    assert!(text.contains("not available over this transport"), "{text}");
}

#[test]
fn switch_without_a_configuration_refuses_and_says_what_to_do() {
    let sandbox = Sandbox::new("no-config");
    let out = sandbox.run(&["switch", "ubuntu"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("configure"), "{}", combined(&out));
}

#[test]
fn doctor_reports_blockers_without_writing_anything() {
    let sandbox = Sandbox::new("doctor-empty");
    let out = sandbox.run(&["doctor"]);
    // Exit 2 because there is no configuration yet.
    assert_eq!(out.status.code(), Some(2), "{}", combined(&out));
    assert!(stdout_of(&out).contains("No configuration yet"));
}

#[test]
fn doctor_is_clean_once_every_mapping_is_verified() {
    let sandbox = Sandbox::new("doctor-ok");
    sandbox.write_config(verified_config());
    let out = sandbox.run(&["doctor"]);
    assert_eq!(out.status.code(), Some(0), "{}", combined(&out));
    assert!(stdout_of(&out).contains("No blockers"));
}

#[test]
fn a_dry_run_reports_the_plan_and_writes_nothing() {
    let sandbox = Sandbox::new("dry-run");
    sandbox.write_config(verified_config());
    let out = sandbox.run(&["switch", "ubuntu", "--dry-run"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("Nothing was written"), "{text}");
    // Each panel gets its own code, not one shared value.
    assert!(text.contains("would write 0x0F"), "{text}");
    assert!(text.contains("would write 0x11"), "{text}");
}

#[test]
fn switching_to_the_current_destination_is_a_confirmed_no_op() {
    let sandbox = Sandbox::new("idempotent");
    sandbox.write_config(verified_config());
    // The fake panels start on the Windows codes.
    let out = sandbox.run(&["switch", "windows"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        text.contains("already on the requested input"),
        "should not re-issue a write: {text}"
    );
}

#[test]
fn an_unverified_mapping_blocks_the_switch() {
    let sandbox = Sandbox::new("unverified");
    sandbox.write_config(&verified_config().replace(
        r#"input_code = "0x0F"
verification = "user-confirmed""#,
        r#"input_code = "0x0F"
verification = "reported""#,
    ));
    let out = sandbox.run(&["switch", "ubuntu"]);
    assert_eq!(out.status.code(), Some(2), "{}", combined(&out));
    let text = stderr_of(&out);
    assert!(text.contains("not user-confirmed"), "{text}");
    // A refusal must still tell the user how to recover.
    assert!(text.contains("monitor's own buttons"), "{text}");
}

#[test]
fn a_monitor_that_is_not_attached_blocks_the_switch() {
    let sandbox = Sandbox::new("missing-monitor");
    sandbox.write_config(&verified_config().replace("FAKE-SN-R", "SN-THAT-IS-NOT-PRESENT"));
    let out = sandbox.run(&["switch", "ubuntu"]);
    assert_eq!(out.status.code(), Some(2), "{}", combined(&out));
    assert!(
        stderr_of(&out).contains("not attached"),
        "{}",
        combined(&out)
    );
}

#[test]
fn a_panel_moved_to_another_port_blocks_the_switch() {
    let sandbox = Sandbox::new("port-changed");
    sandbox.write_config(&verified_config().replace(
        r#"backend_id = "fake-right""#,
        r#"backend_id = "fake-some-other-port""#,
    ));
    let out = sandbox.run(&["switch", "ubuntu"]);
    assert_eq!(out.status.code(), Some(2), "{}", combined(&out));
    let text = stderr_of(&out);
    assert!(text.contains("Cabling changed"), "{text}");
    assert!(text.contains("configure"), "{text}");
}

#[test]
fn an_unknown_destination_lists_the_configured_ones() {
    let sandbox = Sandbox::new("unknown-dest");
    sandbox.write_config(verified_config());
    let out = sandbox.run(&["switch", "macos"]);
    assert_eq!(out.status.code(), Some(2));
    let text = stderr_of(&out);
    assert!(
        text.contains("windows") && text.contains("ubuntu"),
        "{text}"
    );
}

#[test]
fn repeated_presses_within_the_dedupe_window_are_ignored() {
    let sandbox = Sandbox::new("dedupe");
    sandbox.write_config(&verified_config().replace("dedupe_ms = 0", "dedupe_ms = 60000"));

    let first = sandbox.run(&["switch", "ubuntu"]);
    assert!(first.status.success(), "{}", combined(&first));

    let second = sandbox.run(&["switch", "ubuntu"]);
    assert!(
        stdout_of(&second).contains("ignoring the repeat"),
        "{}",
        combined(&second)
    );

    // --force gets through the guard.
    let forced = sandbox.run(&["switch", "ubuntu", "--force"]);
    assert!(!stdout_of(&forced).contains("ignoring the repeat"));
}

#[test]
fn status_separates_live_readings_from_last_requested_intent() {
    let sandbox = Sandbox::new("status");
    sandbox.write_config(verified_config());
    let out = sandbox.run(&["status"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("Observed now"), "{text}");
    assert!(text.contains("Last requested by this tool"), "{text}");
    assert!(
        text.contains("not proof of what the monitors show"),
        "intent must not be presented as state: {text}"
    );
}

#[test]
fn inspect_labels_claims_as_claims() {
    let sandbox = Sandbox::new("inspect");
    sandbox.write_config(verified_config());
    let out = sandbox.run(&["inspect"]);
    let text = stdout_of(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(text.contains("reported input codes"), "{text}");
    assert!(text.contains("the monitor's own claim"), "{text}");
    assert!(text.contains("last read current"), "{text}");
    assert!(text.contains("write tested"), "{text}");
    assert!(text.contains("configured mapping"), "{text}");
}

#[test]
fn interactive_commands_refuse_to_run_without_a_terminal() {
    let sandbox = Sandbox::new("no-tty");
    sandbox.write_config(verified_config());
    // stdin is not a terminal here, which is exactly the shortcut case.
    let out = sandbox.run(&["test-input", "--monitor", "left", "--code", "0x0F"]);
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("interactive terminal"),
        "{}",
        combined(&out)
    );
}

#[test]
fn a_bare_decimal_input_code_is_rejected_on_the_command_line() {
    let sandbox = Sandbox::new("bad-code");
    sandbox.write_config(verified_config());
    let out = sandbox.run(&["test-input", "--monitor", "left", "--code", "11"]);
    assert!(!out.status.success());
    assert!(combined(&out).contains("0x"), "{}", combined(&out));
}

#[test]
fn config_is_never_created_as_a_side_effect_of_a_read_only_command() {
    let sandbox = Sandbox::new("no-side-effects");
    for command in [&["monitors"][..], &["doctor"][..], &["inspect"][..]] {
        let _ = sandbox.run(command);
        assert!(
            !Path::new(&sandbox.config_path()).exists(),
            "{command:?} created a config file"
        );
    }
}
