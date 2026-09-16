//! Terminal output and prompting.
//!
//! Prompts refuse to run when stdin is not a terminal. A keyboard shortcut
//! invokes this binary with no console attached, and a question nobody can see
//! must not be answered by an EOF that reads as "yes".

use std::io::{self, IsTerminal, Write};

pub fn heading(text: &str) {
    println!("\n{text}");
    println!("{}", "-".repeat(text.chars().count()));
}

pub fn field(label: &str, value: impl AsRef<str>) {
    println!("  {:<22} {}", label, value.as_ref());
}

/// A continuation line aligned under a [`field`] value.
pub fn field_cont(value: impl AsRef<str>) {
    println!("  {:<22} {}", "", value.as_ref());
}

pub fn warn(text: impl AsRef<str>) {
    eprintln!("warning: {}", text.as_ref());
}

#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("this command needs an interactive terminal, but stdin is not one. Run it from a shell rather than from a shortcut.")]
    NotATerminal,
    #[error("input ended before the question was answered")]
    Eof,
    #[error("could not read from the terminal: {0}")]
    Io(String),
}

fn read_line(question: &str) -> Result<String, PromptError> {
    if !io::stdin().is_terminal() {
        return Err(PromptError::NotATerminal);
    }
    print!("{question}");
    io::stdout()
        .flush()
        .map_err(|e| PromptError::Io(e.to_string()))?;

    let mut buf = String::new();
    let n = io::stdin()
        .read_line(&mut buf)
        .map_err(|e| PromptError::Io(e.to_string()))?;
    if n == 0 {
        return Err(PromptError::Eof);
    }
    Ok(buf.trim().to_string())
}

/// Free-text answer. Empty input returns `default` when one is given.
pub fn ask(question: &str, default: Option<&str>) -> Result<String, PromptError> {
    let suffix = match default {
        Some(d) => format!(" [{d}]: "),
        None => ": ".to_string(),
    };
    loop {
        let answer = read_line(&format!("{question}{suffix}"))?;
        if !answer.is_empty() {
            return Ok(answer);
        }
        if let Some(d) = default {
            return Ok(d.to_string());
        }
        println!("  (an answer is required)");
    }
}

/// Yes/no. There is no default: every caller of this is about to change
/// something physical, so silence is not consent.
pub fn confirm(question: &str) -> Result<bool, PromptError> {
    loop {
        let answer = read_line(&format!("{question} (yes/no): "))?.to_lowercase();
        match answer.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("  (please answer yes or no)"),
        }
    }
}

/// Pick one of `options`, returning its index.
pub fn choose(question: &str, options: &[String]) -> Result<usize, PromptError> {
    println!("{question}");
    for (i, option) in options.iter().enumerate() {
        println!("  {}) {option}", i + 1);
    }
    loop {
        let answer = read_line("Choice: ")?;
        match answer.parse::<usize>() {
            Ok(n) if n >= 1 && n <= options.len() => return Ok(n - 1),
            _ => println!("  (enter a number between 1 and {})", options.len()),
        }
    }
}

/// Printed before any first write to a monitor, and in every failure report.
pub const RECOVERY_NOTE: &str = "\
If a screen goes blank or stops responding:
  - Use the monitor's own buttons: open the on-screen menu and pick the input
    by hand. This always works and needs no computer.
  - On this laptop, the built-in panel stays available; Windows will fall back
    to it if the external displays disappear.
  - Run `desktop-switcher switch <destination>` again once a screen is back.
    Every switch sets an absolute input, so repeating it is safe.";

pub fn print_recovery_note() {
    println!("\n{RECOVERY_NOTE}");
}

/// The same note on stderr, so it stays attached to a refusal or a failure
/// even when the two streams are redirected separately.
pub fn eprint_recovery_note() {
    eprintln!("\n{RECOVERY_NOTE}");
}
