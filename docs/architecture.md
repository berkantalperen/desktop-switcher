# Architecture

How desktop-switcher is put together, and why. Read this before changing it.
The README covers using it; [hardware-inventory.md](hardware-inventory.md) is
the evidence behind the rules; [test-matrix.md](test-matrix.md) says how each
rule is checked.

## The problem, in one paragraph

Monitors cabled to more than one computer can be told over DDC/CI to show a
different input: write a code to VCP feature `0x60`. That is the whole
mechanism, and it is unreliable in specific ways. The register reports what
was last *stored*, not what is displayed. A monitor with no picture sleeps and
ignores DDC entirely. Capability lists are wrong. OS display ids follow the
cable, not the monitor. The code is small; most of it exists to handle those
facts safely, so that a hotkey never sends a screen somewhere nobody can get it
back from.

## Two ideas everything follows from

**Monitors and inputs, never computers.** The tool knows a monitor (by EDID
serial) and its input codes. What an input *means* ("Laptop") is a label the
user gives it, and what a set of switches means ("everything to the
workstation") is an *action* they name. The first schema was organised around
destination computers; it was replaced because it hard-wired one desk's
layout into the vocabulary. Old files are upgraded on load
(`switcher-core::config`). Tests assert the interface never names a computer.

**Shortcut-first.** The switching path runs from a hotkey with no console and
nobody watching. So nothing in it prompts, asks for confirmation, or prints a
warning a person would have to act on. What can go wrong is refused, waited
out, or documented. A "confirm it worked" step was considered and rejected for
this reason. The CLI's prompts refuse to run without a terminal, so an EOF can
never read as "yes".

## Programs

```
 hotkey (Windows: tray registers it)        GNOME custom shortcut (Linux)
          |                                           |
          v                                           v
  desktop-switcher-tray  --spawns-->  desktop-switcher run-action <name>  <--  terminal
  desktop-switcher-gui   --spawns-->          (the CLI)
                                                 |
                                   switcher-core rules, then a backend
                                                 |
                              Windows dxva2 API  |  ddcutil (Linux)
                                                 v
                                          the monitor (VCP 0x60)
```

- **`desktop-switcher`** (the CLI) is the only implementation of the rules.
- **`desktop-switcher-gui`** edits `config.toml` and runs actions by spawning
  the CLI. It owns the configuration file and no switching logic.
- **`desktop-switcher-tray`** shows a menu built from the configuration and
  runs the CLI for each item. On Windows it also registers the hotkeys. On
  Linux GNOME binds them, because a Wayland client cannot grab keys.

Keeping the GUI and tray as thin callers of the CLI means there is one place a
safety check can live, and nothing can bypass it.

## Crates

| Crate | What it owns |
|---|---|
| `switcher-core` | All domain rules; no OS calls. Types (`InputCode` refuses bare decimals), configuration and schema upgrades, EDID and MCCS parsing, binding configured monitors to present ones (`inventory`), setting and toggling an input and reporting it honestly (`switch`), actions, the event log, the lock, and the `MonitorBackend` trait with an in-memory `fake` backend. |
| `switcher-backend-windows` | Windows' Monitor Configuration API. `topology` (pure) decides which physical monitor a display is and refuses mirrored, detached, duplicate or built-in ones; `ddc` (unsafe) only exchanges VCP values; `edid_source` reads serials from the registry. |
| `switcher-backend-ddcutil` | Linux, by running `ddcutil` with argument arrays (never a shell). Selects monitors by `--sn`/`--model`, never by display number, and serialises calls, since overlapping I²C transactions corrupt each other. Parsers are tested against captured output in `tests/fixtures/ddcutil/`. |
| `switcher-desktop` | "Layer 2": this computer's desktop around a monitor that has gone to the other computer. Listing displays and sweeping windows work; detaching (`release`/`claim`) is experimental. Windows only; a no-op elsewhere. |
| `switcher-cli` | The command surface. Read-only commands (`doctor`, `monitors`, `status`, `displays`) cannot write, and tests assert it. |
| `switcher-gui` | eframe/egui configuration editor. |
| `switcher-tray` | The tray. `menu` is a platform-neutral model rendered by `win` (Win32 `Shell_NotifyIcon`) and `linux` (StatusNotifierItem over D-Bus, via `ksni`). `hotkey` parses `CTRL+ALT+1`-style strings; `notice` turns a failed CLI run into one line; `desktop_entry` writes the Linux app-menu and login entries. |
| `switcher-icon` | The icon, drawn in code at any size, encoded as PNG and ICO with no dependencies. Build scripts embed it in each Windows executable; the tray and GUI draw it at runtime. No image file is kept in the repository. |

## What happens on a switch

`desktop-switcher run-action "Toggle left"`, from a hotkey:

1. **Load** the configuration (upgrading an old schema in memory).
2. **Lock and dedupe.** A cross-process lock (`switch.lock` in the state
   directory) stops two hotkeys, or a hotkey and the GUI, driving monitors at
   once; a lock left by a killed process is reclaimed. The same request inside
   `dedupe_ms` is ignored, so a held key does not fire a burst of writes.
3. **Discover and bind.** The backend lists what is present; `inventory` binds
   each configured monitor to exactly one present display by serial,
   corroborated by connection. No match, two matches, or a moved cable refuses
   the step. Discovery order is never used to address anything.
4. **Run the steps in order**, stopping at the first failure:
   - `set-input` writes the code. **Always**, even when the monitor claims to
     be on it already: that claim is least trustworthy exactly when it matters.
   - `toggle-input` reads the register and writes the second code if it
     reports the first, the first otherwise. The read chooses; it never skips
     the write.
   - `run`, `sweep`, `restore-windows` run a program or move windows.
5. **Write** (Windows): wait for the display topology to settle (Windows shows
   panels missing or mirrored for a second or two after one changes input),
   check the serial again, write, and retry only if the OS reports failure.
   Writes are absolute, so a retry cannot land somewhere new.
6. **Read back** after `settle_ms`, and report it as "monitor stored 0x0F; it
   does not report what it displays". A read-back is never called confirmation
   and never recorded as evidence.
7. **Log and exit.** The event log records what was asked, run and returned.
   Exit code `0` done, `1` partial, `2` refused or failed. The tray is silent
   on `0` and otherwise shows the one line from the report that says why.

## Configuration and state

One file per computer: connection ids differ between machines, and so can the
codes, since a code names the monitor's socket rather than the computer.

| | Windows | Linux |
|---|---|---|
| Configuration | `%APPDATA%\desktop-switcher\config\config.toml` | `~/.config/desktop-switcher/config.toml` |
| State (event log, lock, last request) | `%LOCALAPPDATA%\desktop-switcher\data\` | `~/.local/share/desktop-switcher/` |

`desktop-switcher doctor` prints both. The schema is documented in
[examples/config.example.toml](../examples/config.example.toml). `configure`
writes monitors; the GUI writes everything. The tray re-reads the file every
time its menu opens and polls it every two seconds for hotkey changes. GNOME
shortcuts are written by `install-gnome-shortcuts.sh`, so they change only when
it is run again.

Each input carries a `verification` level (`reported`, `user-confirmed`, …).
It is informational: switching never depends on it, and only a person can
raise it. Labels matter more. The tray offers only inputs someone has named,
because an unnamed one is usually a socket the monitor merely claims, and
switching to an empty socket puts the screen to sleep.

## Platform integration

**Windows.** `scripts/install-windows.ps1` copies the three executables to
`%LOCALAPPDATA%\Programs\Desktop Switcher`, copies the CLI into
`%LOCALAPPDATA%\Microsoft\WindowsApps` (already on `PATH`), creates one
Start Menu entry, and starts the tray at login through the per-user `Run` key.
It closes a running tray with `WM_CLOSE` (killing it leaves a ghost icon) and
verifies each copy landed, since Windows will not overwrite a running
executable. The tray is a single instance, and re-registers its icon when
Explorer restarts (`TaskbarCreated`).

**Linux.** `scripts/install-linux.sh` copies the executables to `~/.local/bin`
and runs `desktop-switcher-tray --install`. That writes an app-menu entry, a
login entry and hicolor icons, all under the user's home directory. The script
then starts the tray and runs `install-gnome-shortcuts.sh`. The tray holds the
D-Bus name `io.github.berkantalperen.DesktopSwitcherTray`; that makes it a
single instance and is how the installer finds it to stop it (`pkill -x` cannot
match: Linux truncates process names to 15 characters). GNOME shows the icon
only with the AppIndicator extension, which Ubuntu enables by default.

The GNOME shortcut script edits one `gsettings` list shared with every other
custom shortcut. It must write a valid path list: a malformed entry crashes
`gsd-media-keys` and takes every GNOME shortcut down with it. That happened
once; the script now keeps only entries shaped like paths.

## Decisions that were tried and reversed

Worth knowing before proposing them again:

| Tried | Why it went |
|---|---|
| Driving PowerToys' Power Display CLI on Windows | It hid any monitor whose capabilities string it could not read, and a converter cable fails that long transfer while every short command works. The native backend lists monitors from the OS topology and never talks DDC to discover. |
| Skipping a write when the read-back already matches | A panel on HDMI reported DisplayPort for a week; the skipped "redundant" write left only its buttons as a way back. |
| Upgrading an input's verification from a read-back | It recorded inputs as confirmed that nobody had watched, and silenced a warning. |
| A warning when switching to an input nobody confirmed | Nobody can act on a warning from a hotkey. Removed with the rule above. |
| Refusing a write when Windows shows a mirrored desktop | Windows shows that transiently during re-detection after every input change. Writes now wait for it to settle, and refuse only if it persists. |
| Detaching displays around a switch (`release`/`claim`) | Left both external displays mirrored on the hardware it was built on. Kept behind `--experimental`; `sweep` solves the stranded-window problem without touching topology. |
| Per-action Start Menu shortcuts carrying hotkeys | Slow, and Explorer held the keys so nothing else could. The tray registers them. |
| A destination-computer schema | See "Monitors and inputs, never computers". |

## Making changes

- **A new step kind:** add a variant to `ActionStep` in
  `switcher-core::actions` with its validation, run it in
  `switcher-cli::commands`, give it an editor in the GUI, and document it in
  `examples/config.example.toml`. The tray needs nothing: it runs actions by
  name.
- **A new backend:** implement `MonitorBackend`. Discovery must report
  identity (serial, model) without guessing; `set_input` must address the
  monitor it was handed and nothing else. Add a `BackendKind`, and fixtures of
  the real tool's output if it wraps one.
- **A new hotkey key name:** `switcher-tray::hotkey` for Windows, and the
  translation in `scripts/install-gnome-shortcuts.sh` for GNOME.
- **The icon:** `switcher-icon::pixels`. Everything else is generated from it.

Before committing, run fmt, clippy and the tests on **both** Windows and Linux
(see the README's Development section): much of the code is `cfg`-gated, and
dead-code lints differ per platform. Never run a hardware switch unattended;
[test-matrix.md](test-matrix.md) has the procedure.
