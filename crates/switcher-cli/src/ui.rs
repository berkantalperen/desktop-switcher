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

pub fn bullet(text: impl AsRef<str>) {
    println!("  - {}", text.as_ref());
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

/// Recovery instructions, printed with every refusal and failure.
pub const RECOVERY_NOTE: &str = "\
If a screen goes blank or stops responding:
  - Use the monitor's own buttons: open its on-screen menu and pick the input
    by hand. This always works and needs no computer at all.
  - A laptop's built-in panel stays available, so the desktop falls back to it
    if the external displays disappear.
  - Run `desktop-switcher set <monitor> 0xNN` again once a screen is back. An
    input is set absolutely, so repeating it is safe.";

/// On stderr, so it stays attached to a refusal or failure even when the two
/// streams are redirected separately.
pub fn eprint_recovery_note() {
    eprintln!("\n{RECOVERY_NOTE}");
}
