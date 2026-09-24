# desktop-switcher

Switch which input your monitors are showing, from the keyboard, over DDC/CI.

```bash
desktop-switcher set ASFPA9A001108 0x0F     # one monitor, one input
desktop-switcher run-action Workstation     # several monitors, one keypress
```

Built for monitors shared between computers: the same screens, cabled to a
laptop and a workstation, flipped between them with one hotkey on either
machine. It knows about **monitors and their inputs**, and nothing about the
computers behind them. What an input *means* — "Laptop", "Workstation" — is a
name you give it.

Runs natively on Windows and Linux. On Windows there is nothing else to
install.

---

## Requirements

| Host | Needs |
|---|---|
| Windows 10/11 | Nothing. Monitors are driven through Windows' own DDC/CI API. |
| Linux | [`ddcutil`](https://www.ddcutil.com/) (`sudo apt install ddcutil`), the `i2c-dev` module loaded, and `/dev/i2c-*` readable without `sudo`. |

DDC/CI must be enabled in each monitor's own menu; most ship with it on.
Building needs Rust 1.82 or newer.

---

## Install

Download the archive for your system from
[Releases](https://github.com/berkantalperen/desktop-switcher/releases),
unpack it, and run the install script inside it. Or build from source and run
the same scripts from the repository:

```powershell
# Windows, after `cargo build --release`. Installs the CLI, the GUI and the
# tray icon, with one Start Menu entry; starts the tray, which carries your
# hotkeys, and has it start at login.
.\scripts\install-windows.ps1
```

```bash
# Linux, after `cargo build --release`. Installs the CLI, the GUI and the
# top-bar icon into ~/.local/bin, adds an app-menu entry, has the icon start
# with your session, and binds GNOME shortcuts for your actions' hotkeys.
./scripts/install-linux.sh
```

Both scripts stop a running copy first and check every copy landed. Windows
silently refuses to overwrite a running executable, which leaves an old binary
reading your configuration with old rules; that is confusing enough to be worth
a script. Nothing needs elevation or sudo, and running a script again updates
the install.

---

## Getting started

```bash
desktop-switcher doctor      # read-only: backend, permissions, identification
desktop-switcher monitors    # read-only: every monitor, its inputs, what it shows
desktop-switcher configure   # name the monitors and record their inputs
desktop-switcher set <monitor> 0x0F     # switch one, and watch the screen
```

A monitor can be named by its label, or by the key `monitors` prints (its EDID
serial). Input codes are the monitor's own VCP 0x60 values, always written with
`0x`; `monitors` lists the ones each monitor claims.

Then compose **actions** — a named list of steps with an optional hotkey — in
the GUI (`desktop-switcher-gui`) or in `config.toml`. An action can set
several monitors, flip one between two inputs, move windows, or run any
program. On Windows the hotkeys take effect a couple of seconds after you save;
on GNOME, run `install-gnome-shortcuts.sh` again.

For example, one key that flips a monitor between the two computers on it:

```toml
[[actions]]
name = "Toggle center"
hotkey = "F24"

[[actions.steps]]
kind = "toggle-input"
monitor = "ASFPA9A001108"
between = ["0x11", "0x0F"]
```

A toggle sets the second input when the monitor reports the first, and the
first otherwise — only ever one of the two. What the monitor reports is the
value it last stored; if something changed the input without updating that,
one press sets the input it is already on and the next press toggles.

### Finding which code is which

Capability lists are claims, and routinely list inputs a monitor does not have.
The only way to learn which code shows which computer is to `set` it once and
look at the screen — the monitor's own read-back cannot tell you (see the safety
model). Give each input you find a label in `config.toml` ("Laptop",
"Workstation"), and build your actions from those.

Try codes where you know a computer with its screen **on** is plugged in: a
monitor sent to an input with no picture goes to sleep (below).

---

## When a screen goes dark

A monitor sent to an input with **no picture** goes to sleep after a few
seconds: nothing plugged in there, or the computer there has turned its screen
off. **A sleeping monitor ignores DDC/CI entirely** — reads and writes alike —
so no command from any computer can bring it back. It wakes on its own when a
picture arrives on the input it is set to.

So:

- **The monitor's own buttons always work.** Open its menu and pick an input.
- **Waking the computer on that input works too.** Touch its keyboard or mouse,
  and the monitor wakes with it.
- Switching to a computer that has blanked its screens gives you dark screens
  until you use that computer, exactly like a hardware KVM.

The one case with no software way out: sending your monitors to a computer
that is asleep, then wanting them back *without touching that computer*. Use
the buttons.

---

## Safety model

The rules the code enforces, each with tests behind it:

- **Set, never cycle.** A switch writes an absolute input code. Nothing steps
  through inputs, so pressing a hotkey twice asks for the same thing twice
  rather than landing somewhere new.
- **The write always goes out,** even when the monitor claims to be on that
  input already. The read is least trustworthy exactly when it matters: a panel
  physically on HDMI once reported DisplayPort, and skipping the "redundant"
  write left no way back but its buttons.
- **A read-back is not confirmation.** VCP 0x60 returns the value a monitor last
  *stored*, not the input it is *displaying* — one held `0x0F` for minutes while
  showing VGA. A matching read is reported as "monitor stored 0x0F; it does not
  report what it displays". The word "confirmed" belongs to a person who looked.
- **Nothing is learned from a read-back.** It is never recorded as evidence
  about an input.
- **Switching never asks anything.** It runs from hotkeys, so it neither
  prompts nor prints warnings that would need a person to act on them; what
  can go wrong is refused, waited out, or documented.
- **Identity before addressing.** Monitors are bound by EDID serial and
  corroborated by connection. Discovery order is never used to address a write,
  because it moves when a display sleeps or re-enumerates. On Windows the serial
  is checked again immediately before every write, so a moved cable cannot
  redirect one.
- **Refuse rather than guess.** Two displays with one serial, or (on Windows) a
  desktop set to *mirror* rather than extend, cannot be told apart reliably, so the tool
  refuses to write to them and says why.
- **On Windows, discovery never talks to the monitor.** A monitor is listed
  because the OS says it is plugged in. Whether it answers a particular request is reported on
  that request and never decides whether it exists. (PowerToys, which the
  Windows backend used to drive, hid any monitor whose capabilities string it
  could not read — a long transfer some cables cannot carry even when every
  short command a switch needs gets through.)
- **Windows' re-detection is waited out, not refused.** For a second or two
  after a monitor changes input, Windows' view of the displays is wrong — a
  panel missing, or two shown as mirrored. A write waits for it to settle. Reads
  do not, so a hotkey is never slowed by one.
- **Writes are retried only when the OS reports them failed,** and are safe to
  repeat because they are absolute.
- **Partial results are reported as partial,** and an action stops at the first
  failed step. Nothing is "rolled back": with the monitor's state unknown, an
  undo is just another blind write.
- **Read-only commands stay read-only.** `doctor`, `monitors`, `status` and
  `displays` cannot write, and tests assert it.
- **The built-in laptop panel is never a target and never detached.** It is
  recognised from the graphics driver itself, not from any one backend's say-so.

---

## Commands

| Command | Writes? | Purpose |
|---|---|---|
| `monitors` | no | Every monitor, its inputs, and what it reports it is showing |
| `set <monitor> <code>` | **yes** | Set one monitor to one input (`--dry-run` to only plan it) |
| `toggle <monitor> <a> <b>` | **yes** | Set `b` if the monitor reports `a`, and `a` otherwise |
| `run-action <name>` | **yes** | Run a configured action |
| `actions` | no | List configured actions and their hotkeys |
| `configure` | config only | Record which monitors exist and what inputs they offer |
| `doctor` | no | Backend, permissions, configuration and identification checks |
| `status` | no | What each monitor reports, alongside the last thing requested |
| `displays` | no | This computer's desktop: attached displays, connectors, layout |
| `sweep [monitor]` | windows only | Move windows off a monitor, leaving the display alone |
| `restore-windows` | windows only | Put swept windows back |
| `release` / `claim` / `primary` | topology | Detach, reattach, or make primary. **Experimental**, behind `--experimental`: on the hardware this was built against they have left displays mirrored. |

Global flags: `--config <path>`, `--backend windows|ddcutil|fake` (`fake` is
in-memory monitors, for trying commands with no hardware), `--tool-path` for
`ddcutil`. Exit codes: `0` success, `1` partial, `2` refused or failed.

The GUI owns no logic: everything it *does* it does by running the CLI, so there
is one implementation of the rules and the GUI cannot skip a check. What it owns
is the configuration file.

---

## Tray icon

`desktop-switcher-tray` puts a small monitor icon in the Windows notification
area, or at the right of the GNOME top bar. One click opens a menu of your
actions, with their hotkeys, and a submenu per monitor listing its inputs.
Choosing one runs the CLI exactly as a hotkey does, without a console window.

- It re-reads the configuration every time the menu opens, so changes made in
  the GUI appear on the next click.
- A monitor's submenu lists only the inputs you have named. Inputs still
  carrying their standard name ("VGA-1") are usually sockets the monitor merely
  claims, and one click would put it to sleep on an empty one; with nothing
  named, every input is listed.
- It is silent when a switch works. When one does not, it shows a notification
  with the reason, taken from the CLI's report.
- Only one runs at a time, and opening Desktop Switcher from the Start Menu or
  app menu starts it if you quit it.

**Windows:** it registers your actions' hotkeys (below). To keep it visible,
drag it out of the `^` overflow onto the taskbar, or turn it on under
**Settings → Personalization → Taskbar → Other system tray icons**.

**Linux:** it is a StatusNotifierItem, the kind of icon KDE, Ubuntu's GNOME
and most other desktops show. Stock GNOME shows none without the *AppIndicator
and KStatusNotifierItem Support* extension (Ubuntu ships it on); without it the
icon has nowhere to appear, and the hotkeys still work. GNOME, not the icon,
owns the hotkeys.

---

## Keyboard shortcuts

Write hotkeys like `CTRL+ALT+1`, `F24` or `SHIFT+F24`: any of Ctrl, Alt, Shift
and Win, plus one key — a letter, digit, F1–F24, a numpad digit, or a named key
such as `PAGEDOWN` or `SPACE`.

**Windows:** the tray registers them itself. They fire instantly, need no
shortcut files, and follow the configuration: an edit takes effect within a
couple of seconds of saving. If another program already holds a combination,
the tray says so once and keeps trying, so it picks the key up when the other
program lets go.

**GNOME:** `./scripts/install-gnome-shortcuts.sh` (which `install-linux.sh`
runs) binds each hotkey as a custom shortcut that runs `run-action`; run it
again after changing hotkeys, and `--uninstall` removes them. Under
Wayland an application cannot reliably grab keys for itself, so GNOME has to do
it.

---

## Layout

```
crates/
  switcher-core/              domain rules; no OS calls, no hardware
  switcher-backend-windows/   Windows' DDC/CI API (dxva2), EDID from the registry
  switcher-backend-ddcutil/   Linux, over ddcutil
  switcher-desktop/           this computer's desktop: topology, window sweeps
  switcher-cli/               command surface
  switcher-gui/               configuration editor that drives the CLI
  switcher-tray/              tray / top-bar menu that drives the CLI
  switcher-icon/              the app icon, drawn in code, as .ico and .png
docs/
  architecture.md             how it fits together, and why
  hardware-inventory.md       captured evidence from the machines it was built on
  test-matrix.md              what is tested, and the hardware procedure
  troubleshooting.md          failure modes and recovery
scripts/                      installers and hotkey binders
tests/fixtures/               verbatim tool output, for parsers and as evidence
examples/config.example.toml  the configuration schema
```

Backends sit behind one narrow trait (`MonitorBackend`), so the rules that decide
whether a write is safe are tested without hardware.
[docs/architecture.md](docs/architecture.md) explains how the pieces fit, what
a switch does step by step, and which approaches were tried and reversed;
[AGENTS.md](AGENTS.md) is the short version for anyone about to change the code.

---

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

All three pass on Windows and Linux. Switching real monitors is not safe to run
unattended and is not in CI; [docs/test-matrix.md](docs/test-matrix.md) has the
procedure for doing it by hand.

---

## Uninstall

On Linux, `./scripts/install-linux.sh --uninstall` removes the binaries, the
app-menu and autostart entries, the icons and the shortcuts. On Windows, delete
the binaries, the Start Menu entry and the `Desktop Switcher Tray` value under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Either way your
configuration and state are left alone (`desktop-switcher doctor` prints where
they are). Nothing is installed system-wide, no service is registered, and no
elevation is ever needed.
