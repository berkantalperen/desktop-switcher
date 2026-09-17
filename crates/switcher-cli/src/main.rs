//! `desktop-switcher` — point both monitors at a named computer.
//!
//! Command surface is split so that nothing read-only can write:
//! `doctor`, `monitors`, `inspect` and `status` never touch VCP 0x60.
//! `configure` and `test-input` write only after an interactive confirmation.
//! `switch` and `toggle` write only from mappings a human has confirmed.

mod commands;
mod desktop;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use switcher_backend_ddcutil::DdcutilBackend;
use switcher_backend_powertoys::PowerDisplayBackend;
use switcher_core::backend::MonitorBackend;
use switcher_core::config::{self, BackendKind, Config};
use switcher_core::eventlog::EventLog;
use switcher_core::fake::{FakeBackend, FakeMonitor};

#[derive(Parser, Debug)]
#[command(
    name = "desktop-switcher",
    version,
    about = "Switch both external monitors to a named computer over DDC/CI.",
    long_about = "Switch both external monitors to a named computer over DDC/CI.\n\n\
                  Every switch sets an absolute input for a named destination, so running\n\
                  the same command twice is safe and never cycles through inputs."
)]
struct Cli {
    /// Path to config.toml (defaults to the per-user config directory).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Override which backend to use.
    #[arg(long, global = true, value_enum)]
    backend: Option<BackendChoice>,

    /// Override the path to the backend executable.
    #[arg(long, global = true, value_name = "PATH")]
    tool_path: Option<PathBuf>,

    /// Allow the display-topology commands (`release`, `claim`, `primary`).
    ///
    /// They are not reliable yet: on the hardware this was developed against
    /// they have left displays mirrored instead of extended, and reported a
    /// detached display as attached. `sweep` solves the stranded-window
    /// problem without touching topology and needs no flag.
    #[arg(long, global = true)]
    experimental: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BackendChoice {
    /// PowerToys Power Display CLI (Windows).
    Powertoys,
    /// ddcutil (Linux).
    Ddcutil,
    /// In-memory monitors. Touches no hardware; for trying the commands out.
    Fake,
}

impl From<BackendChoice> for BackendKind {
    fn from(c: BackendChoice) -> Self {
        match c {
            BackendChoice::Powertoys => BackendKind::PowerToysCli,
            BackendChoice::Ddcutil => BackendKind::DdcutilCli,
            BackendChoice::Fake => BackendKind::Fake,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Read-only checks of the backend, permissions and configuration.
    Doctor,

    /// List the displays this computer can identify right now.
    Monitors,

    /// Show what a monitor claims, what it currently reads, and what is configured.
    Inspect {
        /// Logical id, e.g. `left`. Omit for every display.
        target: Option<String>,
    },

    /// Interactively identify the monitors and record verified input codes.
    Configure,

    /// Switch every configured monitor to a destination, e.g. `windows`.
    ///
    /// Changes the monitor input only. To also change what this computer's
    /// desktop does with the monitor, add --release, --sweep or --claim, or
    /// set `[on_switch]` in the configuration.
    Switch {
        /// Destination name as configured, e.g. `windows` or `ubuntu`.
        destination: String,
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Ignore the repeat-press guard.
        #[arg(long)]
        force: bool,
        /// Also detach departing monitors from this computer's desktop.
        #[arg(long, conflicts_with = "sweep")]
        release: bool,
        /// Also move windows off departing monitors, leaving them attached.
        #[arg(long)]
        sweep: bool,
        /// Also reattach monitors this computer had previously released.
        #[arg(long)]
        claim: bool,
    },

    /// List displays as this computer's desktop sees them.
    Displays,

    /// Detach a monitor from this computer's desktop until you claim it back.
    ///
    /// Does not touch the monitor input. The other computer keeps displaying
    /// whatever it was displaying.
    Release {
        /// Logical id. Optional when only one monitor is configured.
        monitor: Option<String>,
    },

    /// Reattach a monitor this computer had released.
    Claim { monitor: Option<String> },

    /// Move windows off a monitor without detaching it.
    Sweep { monitor: Option<String> },

    /// Make a monitor this computer's primary display.
    Primary { monitor: Option<String> },

    /// Live readings, alongside the last destination this tool requested.
    Status,

    /// Switch to the other computer, only when the current state is unambiguous.
    Toggle {
        #[arg(long)]
        dry_run: bool,
        /// Also detach departing monitors from this computer's desktop.
        #[arg(long, conflicts_with = "sweep")]
        release: bool,
        /// Also move windows off departing monitors, leaving them attached.
        #[arg(long)]
        sweep: bool,
        /// Also reattach monitors this computer had previously released.
        #[arg(long)]
        claim: bool,
    },

    /// Write one input code to one monitor, with confirmation. For Stage B
    /// hardware verification; this is how a mapping earns `user-confirmed`.
    TestInput {
        /// Logical id from the config, or a backend id from `monitors`.
        #[arg(long)]
        monitor: String,
        /// Input code to write, with an explicit 0x prefix, e.g. `0x11`.
        #[arg(long)]
        code: String,
        /// Record the result against this destination after you confirm it.
        #[arg(long)]
        destination: Option<String>,
    },
}

/// Everything a command needs: resolved paths, config, and a live backend.
pub struct App {
    pub config_path: PathBuf,
    pub config: Option<Config>,
    pub backend: Box<dyn MonitorBackend>,
    pub backend_kind: BackendKind,
    pub state_dir: PathBuf,
    pub log: EventLog,
    /// Whether the unreliable display-topology commands are permitted.
    pub experimental: bool,
}

fn default_backend_kind() -> BackendKind {
    if cfg!(windows) {
        BackendKind::PowerToysCli
    } else {
        BackendKind::DdcutilCli
    }
}

fn build_backend(
    kind: BackendKind,
    tool_path: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<Box<dyn MonitorBackend>> {
    Ok(match kind {
        BackendKind::PowerToysCli => {
            let exe = PowerDisplayBackend::locate(tool_path)?;
            Box::new(PowerDisplayBackend::new(exe, timeout))
        }
        BackendKind::DdcutilCli => {
            let exe = DdcutilBackend::locate(tool_path)?;
            Box::new(DdcutilBackend::new(exe, timeout))
        }
        // Two panels that need *different* codes for the same computer, which
        // is what the real hardware does.
        BackendKind::Fake => Box::new(FakeBackend::new(vec![
            FakeMonitor::new("fake-left", Some("FAKE-SN-L"), 0x11).with_index(1),
            FakeMonitor::new("fake-right", Some("FAKE-SN-R"), 0x0F).with_index(2),
            FakeMonitor::internal_panel("fake-internal"),
        ])),
    })
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::from(2)
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();

    let config_path = match &cli.config {
        Some(p) => p.clone(),
        None => config::default_config_path().context("locating the config directory")?,
    };

    // A missing config is normal before `configure` has run, so it is not an
    // error here; the commands that need one say so themselves.
    let config = match Config::load(&config_path) {
        Ok(c) => Some(c),
        Err(config::ConfigError::Missing(_)) => None,
        Err(e) => return Err(e).context("loading configuration"),
    };

    let backend_kind = cli
        .backend
        .map(BackendKind::from)
        .or(config.as_ref().map(|c| c.backend))
        .unwrap_or_else(default_backend_kind);

    let timeout = Duration::from_secs(config.as_ref().map(|c| c.timeout_seconds).unwrap_or(15));
    let tool_path = cli
        .tool_path
        .clone()
        .or_else(|| config.as_ref().and_then(|c| c.tool_path.clone()));

    let backend = build_backend(backend_kind, tool_path.as_deref(), timeout)?;

    // Overridable so tests (and portable installs) do not share the lock and
    // last-request files with a real installation.
    let state_dir = match std::env::var_os("DESKTOP_SWITCHER_STATE_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => config::default_state_dir().context("locating the state directory")?,
    };
    let log = EventLog::in_dir(&state_dir);

    let mut app = App {
        config_path,
        config,
        backend,
        backend_kind,
        state_dir,
        log,
        experimental: cli.experimental,
    };

    match cli.command {
        Command::Doctor => commands::doctor(&app),
        Command::Monitors => commands::monitors(&app),
        Command::Inspect { target } => commands::inspect(&app, target.as_deref()),
        Command::Configure => commands::configure(&mut app),
        Command::Switch {
            destination,
            dry_run,
            force,
            release,
            sweep,
            claim,
        } => commands::switch(
            &app,
            &destination,
            dry_run,
            force,
            commands::SwitchFlags {
                release,
                sweep,
                claim,
            },
        ),
        Command::Displays => desktop::displays(&app),
        Command::Release { monitor } => desktop::release(&app, monitor.as_deref()),
        Command::Claim { monitor } => desktop::claim(&app, monitor.as_deref()),
        Command::Sweep { monitor } => desktop::sweep(&app, monitor.as_deref()),
        Command::Primary { monitor } => desktop::set_primary(&app, monitor.as_deref()),
        Command::Status => commands::status(&app),
        Command::Toggle {
            dry_run,
            release,
            sweep,
            claim,
        } => commands::toggle(
            &app,
            dry_run,
            commands::SwitchFlags {
                release,
                sweep,
                claim,
            },
        ),
        Command::TestInput {
            monitor,
            code,
            destination,
        } => commands::test_input(&mut app, &monitor, &code, destination.as_deref()),
    }
}
