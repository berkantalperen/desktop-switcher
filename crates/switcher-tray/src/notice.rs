//! What to tell someone when a click did not do what they asked.
//!
//! Silence on success: the screens changing is the confirmation, and a toast
//! for every switch would be noise. On failure, one line that says why, lifted
//! from the CLI's own report.

/// A notification to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Room Windows gives a notification, in characters, leaving the terminator.
const TITLE_MAX: usize = 63;
const BODY_MAX: usize = 255;

/// A notice for a finished CLI run, or `None` when there is nothing to say.
///
/// Exit codes are the CLI's: `0` done, `1` partial, `2` refused or failed.
pub fn for_run(describe: &str, exit_code: Option<i32>, output: &str) -> Option<Notice> {
    let title = match exit_code {
        Some(0) => return None,
        Some(1) => format!("{describe}: only partly done"),
        Some(_) => format!("{describe}: did not switch"),
        None => format!("{describe}: stopped unexpectedly"),
    };
    Some(Notice {
        title: clip(&title, TITLE_MAX),
        body: clip(&reason(output), BODY_MAX),
    })
}

/// The notice when the CLI could not be started at all.
pub fn cannot_start(describe: &str, why: &str) -> Notice {
    Notice {
        title: clip(&format!("{describe}: could not run"), TITLE_MAX),
        body: clip(why, BODY_MAX),
    }
}

/// The notice when one or more hotkeys could not be registered. Windows
/// only: on Linux GNOME owns the hotkeys.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn hotkeys_unavailable(problems: &[String]) -> Notice {
    let title = match problems.len() {
        1 => "A hotkey is not available".to_string(),
        n => format!("{n} hotkeys are not available"),
    };
    Notice {
        title: clip(&title, TITLE_MAX),
        body: clip(&problems.join("; "), BODY_MAX),
    }
}

/// The one line of a CLI report that says why.
fn reason(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    // A step that failed: `aoc-left   0x11 -> failed: <why>`.
    if let Some(line) = lines.iter().find(|l| l.contains("-> failed:")) {
        let (head, why) = line.split_once("-> failed:").expect("checked above");
        let monitor = head.split_whitespace().next().unwrap_or("");
        return format!("{monitor}: {}", why.trim());
    }
    // A refusal: the reason is on the next line.
    if let Some(i) = lines.iter().position(|l| l.starts_with("Refusing")) {
        if let Some(why) = lines.get(i + 1) {
            return why.to_string();
        }
    }
    if let Some(line) = lines.iter().find(|l| l.starts_with("error:")) {
        return line.trim_start_matches("error:").trim().to_string();
    }
    lines
        .last()
        .map(|l| l.to_string())
        .unwrap_or_else(|| "No details were reported.".into())
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from the first run of the Laptop action through the Windows
    /// backend, which refused on a desktop Windows briefly showed as mirrored.
    const MIRRORED: &str = r"Laptop
------

[1/2] set ASFPA9A001108 to 0x11
  aoc-center                 0x11 -> issued, visual state unconfirmed

[2/2] set ASFPA9A001109 to 0x11
  aoc-left                   0x11 -> failed: `\\?\DISPLAY#AOC2702#4&34b9e9a7&0&UID4165` is mirrored with 1 other display(s). Windows returns mirrored monitors in an order nothing ties to a particular display, so a write could land on the wrong one.

  FAILED: nothing was switched

If a screen goes blank or stops responding:
  - Use the monitor's own buttons: open its on-screen menu and pick the input

Stopped: step 2 of 2 failed, so the rest were not run.";

    /// Verbatim: a monitor PowerToys could not see.
    const REFUSED: &str = "Refusing to change the input.

cannot identify `ASFPA9A001109`: `ASFPA9A001109` is not attached: nothing present matches serial ASFPA9A001109

If a screen goes blank or stops responding:
  - Use the monitor's own buttons";

    #[test]
    fn success_says_nothing() {
        assert_eq!(for_run("Laptop", Some(0), "anything"), None);
    }

    #[test]
    fn a_failed_step_names_the_monitor_and_the_reason() {
        let n = for_run("Laptop", Some(2), MIRRORED).unwrap();
        assert_eq!(n.title, "Laptop: did not switch");
        assert!(n.body.starts_with("aoc-left: `"), "{}", n.body);
        assert!(n.body.contains("is mirrored"), "{}", n.body);
    }

    #[test]
    fn a_refusal_gives_its_reason_not_the_word_refusing() {
        let n = for_run("aoc-left: Laptop", Some(2), REFUSED).unwrap();
        assert!(n.body.starts_with("cannot identify"), "{}", n.body);
    }

    #[test]
    fn a_partial_result_is_called_partial() {
        let n = for_run("Workstation", Some(1), MIRRORED).unwrap();
        assert_eq!(n.title, "Workstation: only partly done");
    }

    #[test]
    fn a_cli_error_line_is_used_without_its_prefix() {
        let n = for_run("X", Some(2), "error: loading configuration: bad TOML").unwrap();
        assert_eq!(n.body, "loading configuration: bad TOML");
    }

    #[test]
    fn empty_output_still_says_something() {
        let n = for_run("X", None, "").unwrap();
        assert_eq!(n.title, "X: stopped unexpectedly");
        assert!(!n.body.is_empty());
    }

    #[test]
    fn long_text_fits_what_windows_allows() {
        let n = for_run(&"a".repeat(200), Some(2), &"b".repeat(1000)).unwrap();
        assert!(n.title.chars().count() <= TITLE_MAX);
        assert!(n.body.chars().count() <= BODY_MAX);
        assert!(n.body.ends_with('…'));
    }

    #[test]
    fn unavailable_hotkeys_are_named() {
        let one = hotkeys_unavailable(&["F24 for “Toggle center” is taken".into()]);
        assert_eq!(one.title, "A hotkey is not available");
        assert!(one.body.contains("F24"));
        let two = hotkeys_unavailable(&["a".into(), "b".into()]);
        assert_eq!(two.title, "2 hotkeys are not available");
        assert_eq!(two.body, "a; b");
    }
}
