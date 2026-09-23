//! `desktop-switcher` — change a monitor's input from the command line.
//!
//! The vocabulary is monitors and inputs, and nothing else. This tool does
//! not know or care what is plugged into an input; that meaning belongs to
//! the person using it, and lives in the name they give an action.
//!
//! The command surface is split so that nothing read-only can write:
//! `doctor`, `monitors`, `displays` and `status` never touch VCP 0x60.

mod commands;
mod desktop;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use switcher_backend_ddcutil::DdcutilBackend;
use switcher_backend_windows::WindowsBackend;
use switcher_core::backend::MonitorBackend;
use switcher_core::config::{self, BackendKind, Config};
use switcher_core::eventlog::EventLog;
use switcher_core::fake::{FakeBackend, FakeMonitor};

#[derive(Parser, Debug)]
#[command(
    name = "desktop-switcher",
    version,
    about = "Change which input a monitor is showing.",
    long_about = "Change which input a monitor is showing.\n\n\
                  An input is set absolutely, never stepped or cycled, so running the\n\
                  same command twice is safe and never walks a monitor through its\n\
                  inputs."
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
    /// they have left displays mirrored instead of extended. `sweep` and
    /// `restore-windows` solve the stranded-window problem without touching
    /// topology and need no flag.
    #[arg(long, global = true)]
    experimental: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BackendChoice {
    /// Windows' own DDC/CI API. Nothing to install.
    Windows,
    /// ddcutil (Linux).
    Ddcutil,
    /// In-memory monitors. Touches no hardware; for trying the commands out.
    Fake,
}

impl From<BackendChoice> for BackendKind {
    fn from(c: BackendChoice) -> Self {
        match c {
            BackendChoice::Windows => BackendKind::Windows,
            BackendChoice::Ddcutil => BackendKind::DdcutilCli,
            BackendChoice::Fake => BackendKind::Fake,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List every monitor, its inputs, and which one it is showing.
    Monitors,

    /// Set a monitor to one of its inputs.
    ///
    /// The code is the monitor's own VCP 0x60 value, written with an explicit
    /// `0x` prefix. `monitors` lists them.
    Set {
        /// Monitor name, key or serial, as shown by `monitors`.
        monitor: String,
        /// Input code, e.g. `0x11`.
        code: String,
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Ignore the repeat-press guard.
        #[arg(long)]
        force: bool,
    },

    /// Flip a monitor between two inputs.
    ///
    /// Sets the second input if the monitor reports the first, and the first
    /// otherwise. Only ever one of the two.
    Toggle {
        /// Monitor name, key or serial, as shown by `monitors`.
        monitor: String,
        /// The first input, e.g. `0x11`. Also used when the monitor reports
        /// anything else, or cannot be read.
        first: String,
        /// The second input, e.g. `0x0F`.
        second: String,
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Ignore the repeat-press guard.
        #[arg(long)]
        force: bool,
    },

    /// Record which monitors exist and what inputs they offer.
    Configure,

    /// Read-only checks of the backend, permissions and configuration.
    Doctor,

    /// What each monitor is showing, alongside the last thing requested.
    Status,

    /// List the actions you have configured.
    Actions,

    /// Run a named action.
    RunAction {
        /// The action name, as shown by `actions`.
        name: String,
    },

    /// List displays as this computer's desktop sees them.
    Displays,

    /// Move windows off a monitor, without changing anything about the display.
    Sweep { monitor: Option<String> },

    /// Put swept windows back where they were.
    RestoreWindows { monitor: Option<String> },

    /// Detach a monitor from this computer's desktop (experimental).
    Release { monitor: Option<String> },

    /// Reattach a monitor this computer released (experimental).
    Claim { monitor: Option<String> },

    /// Make a monitor this computer's primary display (experimental).
    Primary { monitor: Option<String> },
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
        BackendKind::Windows
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
        BackendKind::Windows => Box::new(WindowsBackend::new()),
        BackendKind::DdcutilCli => {
            let exe = DdcutilBackend::locate(tool_path)?;
            Box::new(DdcutilBackend::new(exe, timeout))
        }
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

    // A missing configuration is normal before `configure` has run, so it is
    // not an error here; the commands that need one say so themselves.
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

    // Overridable so tests and portable installs do not share the lock and
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
        Command::Monitors => commands::monitors(&app),
        Command::Set {
            monitor,
            code,
            dry_run,
            force,
        } => commands::set_input(&app, &monitor, &code, dry_run, force),
        Command::Toggle {
            monitor,
            first,
            second,
            dry_run,
            force,
        } => {
            let first = first.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
            let second = second.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
            commands::toggle_input(&app, &monitor, [first, second], dry_run, force)
        }
        Command::Configure => commands::configure(&mut app),
        Command::Doctor => commands::doctor(&app),
        Command::Status => commands::status(&app),
        Command::Actions => commands::actions(&app),
        Command::RunAction { name } => commands::run_action(&app, &name),
        Command::Displays => desktop::displays(&app),
        Command::Sweep { monitor } => desktop::sweep(&app, monitor.as_deref()),
        Command::RestoreWindows { monitor } => desktop::restore_windows(&app, monitor.as_deref()),
        Command::Release { monitor } => desktop::release(&app, monitor.as_deref()),
        Command::Claim { monitor } => desktop::claim(&app, monitor.as_deref()),
        Command::Primary { monitor } => desktop::set_primary(&app, monitor.as_deref()),
    }
}
