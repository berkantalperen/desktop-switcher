# desktop-switcher

One command on whichever computer you are using points both external monitors at
a named computer, over DDC/CI.

```bash
desktop-switcher switch ubuntu
```

Two AOC 27P2DG5 panels are shared between a Windows laptop and an Ubuntu
workstation. Each monitor's input is set explicitly, per monitor, to a code a
human has confirmed.

---

## Status

| Stage | State |
|---|---|
| A — Windows discovery and evidence | **done**, see [docs/hardware-inventory.md](docs/hardware-inventory.md) |
| A — Ubuntu discovery | **done** — the panel is identified by the same serial from both computers |
| B — hardware verification of input codes | **done for the shared monitor** — both codes user-confirmed |
| C — Rust workspace, backends, CLI | **done** — builds, tests and runs natively on both computers; both backends validated against real tool output |
| D — switch transaction | **working on hardware** from the Z4, both directions confirmed by read. The Windows side still needs its own `configure`. |
| E — shortcuts and packaging | documented below, not yet installed |
| F — keyboard/mouse switching | out of scope for v1 |

Gates A and B are met, and the round trip works: `switch ubuntu` then
`switch windows` from the Z4 both moved the panel and confirmed it by
reading the value back. Running the same command twice issues no second
write.

The one caveat is scope, not correctness: only one of the two AOC panels is
currently cabled to both computers, so "both monitors" is one monitor until a
second cable goes into the other panel.

**`switch` will refuse to run until Stage B is complete.** That is deliberate,
not an unfinished edge: no input code becomes usable until someone has watched
it work.

---

## The finding that shaped the design

The two panels use **different input codes for the same computer**:

| Panel | Shows Windows via |
|---|---|
| serial `ASFPA9A001108` | HDMI-1 (`0x11`) |
| serial `ASFPA9A001109` | DisplayPort-1 (`0x0F`) |

"Switch both monitors to Windows" is therefore two different writes, not one
value broadcast to two displays. Every monitor is handled and reported
individually.

---

## Safety model

The rules the code actually enforces, each with tests behind it:

- **Set, never cycle.** A switch writes an absolute input code for a named
  destination. Nothing increments or steps through inputs, so pressing the same
  shortcut twice is a no-op rather than a surprise.
- **Identity before addressing.** Monitors are bound by EDID serial and
  corroborated by connection id. Discovery-time numbers are never used to
  address a write, because they move when a display sleeps or a dock
  re-enumerates. If a monitor cannot be identified uniquely, the tool refuses.
- **Only user-confirmed codes are switchable.** Evidence is tracked as
  `unknown` → `reported` → `read-confirmed` → `write-confirmed` →
  `user-confirmed`. A capability listing is a claim, and claims do not authorise
  writes.
- **Exit zero is not a switched monitor.** Outcomes are reported as
  `confirmed by read`, `issued, visual state unconfirmed`, or `failed`. The
  middle one is never rounded up.
- **Losing contact after switching away is expected, not a failure.** A monitor
  that has moved to the other computer stops answering this one. That is
  distinguished from a genuine read failure.
- **Partial results are reported as partial.** No rollback is attempted: with
  connectivity unknown, "undoing" a write is just another blind write.
- **Writes are never retried.** The first may already have been accepted, and
  repeating it during re-enumeration compounds the disconnect.
- **Read-only means read-only.** `doctor`, `monitors`, `inspect` and `status`
  cannot write. Tests assert it.
- **Unrecognised tool output is fatal.** If PowerToys or ddcutil changes its
  format, parsing fails loudly rather than guessing at a monitor id.

---

## Requirements

Both are external runtime dependencies. The binary is not self-contained.

| Host | Needs |
|---|---|
| Windows | [PowerToys](https://learn.microsoft.com/en-us/windows/powertoys/) with the Power Display module enabled and running. Tested against 0.101.2362.0. |
| Ubuntu | [`ddcutil`](https://www.ddcutil.com/) (`sudo apt install ddcutil`), `i2c-dev` loaded, and `/dev/i2c-*` readable without `sudo`. |

Building needs Rust 1.82 or newer.

---

## Build

```bash
cargo build --release
```

The binary lands at `target/release/desktop-switcher` (`.exe` on Windows).
Build on each platform natively; there is no cross-compilation step.

To put it somewhere on `PATH`, so it works from any directory and from a
keyboard shortcut:

```powershell
# Windows: this directory is already on PATH for the current user.
Copy-Item target\release\desktop-switcher.exe "$env:LOCALAPPDATA\Microsoft\WindowsApps\"
```

```bash
# Linux: ~/.local/bin is on PATH on most desktop installs.
install -Dm755 target/release/desktop-switcher ~/.local/bin/desktop-switcher
```

Neither needs elevation, and uninstalling is deleting the file.

---

## Getting started

Nothing here writes to a monitor until step 4, which asks first.

```bash
# 1. Check the backend, permissions and identification. Read-only.
desktop-switcher doctor

# 2. See what this computer can identify. Read-only.
desktop-switcher monitors

# 3. Identify the panels and record the code for THIS computer.
#    Interactive; establishes half the mapping with no writes at all.
desktop-switcher configure

# 4. Establish the code for the OTHER computer. One monitor, one code,
#    one confirmation, and it asks what you physically saw.
desktop-switcher test-input --monitor left --code 0x0F --destination ubuntu

# 5. Once every mapping is user-confirmed:
desktop-switcher switch ubuntu
```

Step 4 is the only way a code becomes usable. See
[docs/test-matrix.md](docs/test-matrix.md) for the full procedure.

---

## Commands

| Command | Writes? | Purpose |
|---|---|---|
| `doctor` | no | Backend, permissions, configuration and identification checks |
| `monitors` | no | Displays this computer can identify right now |
| `inspect [id]` | no | Claims, current reading, write-test status and configured mapping, kept separate |
| `status` | no | Live readings alongside the last destination *requested*, clearly distinguished |
| `configure` | no | Interactive identification and mapping |
| `test-input` | **yes**, after confirmation | Verify one code on one monitor |
| `switch <dest>` | **yes** | Set every monitor to a destination |
| `toggle` | **yes** | Switch to the other computer, only when the current state is unambiguous |

Useful flags: `--dry-run` on `switch`, `--backend fake` to try any command
against in-memory monitors, `--config <path>`, `--tool-path <path>`.

Exit codes: `0` success, `1` partial, `2` refused or total failure.

---

## Keyboard shortcuts

Bind the two explicit destinations separately. `toggle` is available but binding
it is not recommended until the current-state logic has been proven on hardware:
after someone changes an input with the monitor buttons, a toggle has to guess,
and this one refuses instead.

**Windows** — create a shortcut to the binary with `switch windows` as its
argument and assign a hotkey in its properties, or use Task Scheduler for a
global binding. Pin `tool_path` in `config.toml`: a shortcut runs with a
different environment than a shell, and PowerToys is not on `PATH`.

**Ubuntu (GNOME)** — Settings → Keyboard → Keyboard Shortcuts → Custom
Shortcuts, with the command `/usr/local/bin/desktop-switcher switch ubuntu`.
This avoids depending on a global-hotkey library, whose behaviour under Wayland
varies.

---

## Layout

```
crates/
  switcher-core/                 domain logic; no OS calls, no hardware
  switcher-backend-powertoys/    Windows adapter over the PowerToys CLI
  switcher-backend-ddcutil/      Linux adapter over ddcutil
  switcher-cli/                  command surface
docs/
  hardware-inventory.md          real captured evidence, with confidence labels
  test-matrix.md                 what is tested, and the hardware procedure
  troubleshooting.md             failure modes and recovery
scripts/
  ubuntu-preflight.sh            read-only Stage A capture for the Z4
tests/fixtures/                  verbatim tool output the parsers are tested against
examples/config.example.toml     schema, with placeholders that are not valid codes
```

Backends sit behind one narrow trait (`MonitorBackend`), so the rules that
decide whether a write is safe are tested without hardware.

---

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

All three pass. Hardware switching is not safe to run unattended and is not in
CI.

---

## Uninstall

Delete the binary, and remove the per-user configuration and state directories
(`desktop-switcher doctor` prints both paths). Nothing is installed system-wide,
no service is registered, and no elevation is ever required. PowerToys and
`ddcutil` are separate installs and are left alone.
