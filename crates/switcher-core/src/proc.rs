//! Child-process helper for the CLI-wrapping backends.
//!
//! Arguments are always passed as an explicit array. Nothing is concatenated
//! into a shell string, so a monitor name or serial containing a quote or a
//! semicolon cannot turn into a second command.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcError {
    #[error("executable not found: {0}")]
    NotFound(PathBuf),
    #[error("could not launch `{program}`: {detail}")]
    Launch { program: String, detail: String },
    #[error("`{program}` did not finish within {}s and was killed", timeout.as_secs_f32())]
    Timeout { program: String, timeout: Duration },
    #[error("could not read output of `{program}`: {detail}")]
    Output { program: String, detail: String },
}

/// A completed child process, with everything needed to diagnose it later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRun {
    pub program: String,
    pub args: Vec<String>,
    /// `None` when the process was terminated by a signal.
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

impl CommandRun {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// A shell-safe rendering for logs and error messages. Never executed.
    pub fn display_command(&self) -> String {
        let mut out = String::from(&self.program);
        for a in &self.args {
            out.push(' ');
            if a.contains(' ') {
                out.push('"');
                out.push_str(a);
                out.push('"');
            } else {
                out.push_str(a);
            }
        }
        out
    }
}

#[cfg(windows)]
fn configure_platform(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    // Keep a console window from flashing when invoked from a shortcut.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_platform(_cmd: &mut Command) {}

/// Run `program` with `args`, killing it if it outruns `timeout`.
pub fn run(program: &Path, args: &[String], timeout: Duration) -> Result<CommandRun, ProcError> {
    let program_name = program.display().to_string();

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_platform(&mut cmd);

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ProcError::NotFound(program.to_path_buf())
        } else {
            ProcError::Launch {
                program: program_name.clone(),
                detail: e.to_string(),
            }
        }
    })?;

    // Drain both pipes on their own threads: a child that fills one pipe while
    // we block on the other would deadlock.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || drain(&mut out_pipe));
    let err_handle = std::thread::spawn(move || drain(&mut err_pipe));

    let deadline = started + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(ProcError::Launch {
                    program: program_name,
                    detail: e.to_string(),
                });
            }
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    let Some(status) = status else {
        return Err(ProcError::Timeout {
            program: program_name,
            timeout,
        });
    };

    Ok(CommandRun {
        program: program_name,
        args: args.to_vec(),
        status: status.code(),
        stdout,
        stderr,
        duration: started.elapsed(),
    })
}

fn drain<R: Read>(pipe: &mut Option<R>) -> String {
    let Some(reader) = pipe.as_mut() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).replace("\r\n", "\n")
}

/// Look for `name` on `PATH`, plus any extra directories the caller suggests.
pub fn find_executable(name: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) { &[".exe", ""] } else { &[""] };

    for dir in extra_dirs {
        for ext in candidates {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in candidates {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> (PathBuf, Vec<String>) {
        if cfg!(windows) {
            (PathBuf::from("cmd"), vec!["/C".to_string()])
        } else {
            (PathBuf::from("sh"), vec!["-c".to_string()])
        }
    }

    #[test]
    fn missing_executable_is_reported_as_not_found() {
        let err = run(
            Path::new("definitely-not-a-real-binary-xyzzy"),
            &[],
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(matches!(err, ProcError::NotFound(_)));
    }

    #[test]
    fn captures_stdout_and_exit_code() {
        let (prog, mut args) = shell();
        args.push("echo hello".to_string());
        let run = run(&prog, &args, Duration::from_secs(20)).unwrap();
        assert_eq!(run.status, Some(0));
        assert!(run.success());
        assert!(run.stdout.contains("hello"));
    }

    #[test]
    fn nonzero_exit_is_preserved_not_swallowed() {
        let (prog, mut args) = shell();
        args.push("exit 3".to_string());
        let run = run(&prog, &args, Duration::from_secs(20)).unwrap();
        assert_eq!(run.status, Some(3));
        assert!(!run.success());
    }

    #[test]
    fn a_hung_child_is_killed_and_reported_as_timeout() {
        // Spawn the slow program directly rather than through a shell: killing
        // a shell does not kill its children, and an orphaned grandchild can
        // keep holding files open long after the test ends.
        let (prog, args) = if cfg!(windows) {
            (
                PathBuf::from("ping"),
                vec!["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        } else {
            (PathBuf::from("sleep"), vec!["30".to_string()])
        };
        let err = run(&prog, &args, Duration::from_millis(400)).unwrap_err();
        assert!(matches!(err, ProcError::Timeout { .. }), "got {err:?}");
    }

    #[test]
    fn display_command_quotes_but_never_executes() {
        let run = CommandRun {
            program: "ddcutil".into(),
            args: vec!["--sn".into(), "A B; rm -rf /".into()],
            status: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            duration: Duration::ZERO,
        };
        assert_eq!(run.display_command(), "ddcutil --sn \"A B; rm -rf /\"");
    }
}
