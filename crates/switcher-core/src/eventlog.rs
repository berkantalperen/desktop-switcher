//! Append-only local event log.
//!
//! Records what was asked for, what was run, and what came back, so a switch
//! that went wrong can be reconstructed afterwards. Monitor serials are
//! recorded because they are the identifiers the tool acts on; nothing else
//! about the user or their machine is.

use std::io::Write;
use std::path::{Path, PathBuf};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Current UTC time as an RFC 3339 string.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".to_string())
}

/// Today as `YYYY-MM-DD`, for stamping verification dates.
pub fn today() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

#[derive(Debug, Clone)]
pub struct EventLog {
    path: PathBuf,
    enabled: bool,
}

impl EventLog {
    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            enabled: true,
        }
    }

    /// A log that silently discards everything, for dry runs and tests.
    pub fn disabled() -> Self {
        Self {
            path: PathBuf::new(),
            enabled: false,
        }
    }

    pub fn in_dir(dir: &Path) -> Self {
        Self::at(dir.join("events.log"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one event. Logging failures never abort a switch; the switch is
    /// the thing the user asked for, and a full disk should not block recovery.
    pub fn append(&self, kind: &str, fields: &[(&str, String)]) {
        if !self.enabled {
            return;
        }
        let mut line = format!("{} {kind}", now_rfc3339());
        for (k, v) in fields {
            line.push(' ');
            line.push_str(k);
            line.push('=');
            if v.is_empty() || v.contains(' ') || v.contains('"') {
                line.push_str(&format!("{:?}", v));
            } else {
                line.push_str(v);
            }
        }
        line.push('\n');

        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// The last `n` lines, newest last.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines
            .iter()
            .rev()
            .take(n)
            .rev()
            .map(|s| s.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_tails_events() {
        let dir = std::env::temp_dir().join(format!("ds-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = EventLog::in_dir(&dir);
        log.append("switch.begin", &[("destination", "ubuntu".into())]);
        log.append(
            "switch.step",
            &[("monitor", "left".into()), ("detail", "a b".into())],
        );
        let lines = log.tail(10);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("switch.begin destination=ubuntu"));
        // Values with spaces are quoted so the log stays parseable.
        assert!(lines[1].contains("detail=\"a b\""), "{}", lines[1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_log_writes_nothing_and_does_not_panic() {
        let log = EventLog::disabled();
        log.append("switch.begin", &[("destination", "ubuntu".into())]);
        assert!(log.tail(5).is_empty());
    }

    #[test]
    fn timestamps_look_like_rfc3339() {
        let t = now_rfc3339();
        assert!(t.contains('T') && t.ends_with('Z'), "{t}");
        assert_eq!(today().len(), 10);
    }
}
