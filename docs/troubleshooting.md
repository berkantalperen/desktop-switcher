# Troubleshooting

Start with `desktop-switcher doctor`. It is read-only, checks the backend,
permissions, configuration and monitor identification, and exits non-zero when
something would block a switch.

## Recovery first

If a screen is dark or not responding:

1. **Use the monitor's own buttons.** Open its on-screen menu and pick the
   input by hand. This always works and needs no computer, which is why this
   tool never takes an action you cannot undo physically.
2. **Wake the computer on the input the monitor is set to.** Touch its keyboard
   or mouse. A monitor asleep for lack of a picture wakes the moment one
   arrives.
3. On a laptop, the built-in panel stays available; the desktop falls back to
   it if the external displays disappear.
4. Once a screen is back, run the same command again. Every switch sets an
   absolute input, so repeating it is safe and never cycles.

---

## The screen went dark after a switch

The monitor moved to an input with **no picture**: nothing is plugged in there,
or the computer there has turned its screen off (idle timeout, lock screen,
sleep). After a few seconds without a picture the monitor goes to sleep, and a
sleeping monitor **ignores DDC/CI completely — reads and writes alike.**

Established on the hardware this was built against, 2026-09-23:

- A panel sent to a workstation whose GNOME session had idled out and blanked
  its outputs went dark and stopped answering both computers.
- A write sent to it while it slept was accepted by Windows and did nothing; the
  register afterwards still held the old value.
- Turning the workstation's screens back on woke the panel within seconds, and
  both computers could switch it again.

So the fix is to put a picture on that input — wake that computer — or to use
the monitor's buttons. No command can do it for you while the monitor sleeps.

What this looks like from the tool:

```
monitors:  current input   read failed: ... a DDC/CI message had an invalid
                           value in its command field (0xC0262589) ...
set:       issued, but the monitor did not answer afterwards ...
```

`0xC0262589` (`ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND`) is what Windows
returns for a sleeping monitor. Note that a monitor that is awake answered DDC/CI
from **both** computers here, whichever input it was showing — so on this
hardware, silence means asleep, not "busy with the other computer".

If the computers are yours and can reach each other, an action can wake the
other machine's screens before switching to it, with a `run` step. The tool
itself deliberately knows nothing about computers, so this lives in your
action, not in the tool.

---

## Messages from `set` and `run-action`

### warning: `0xNN is not in this monitor's advertised input list`

The code is not among the inputs configured for this monitor — often a typo.
The write still goes out, because capability lists are wrong often enough that
an unlisted code can be right.

### `monitor stored 0x0F; it does not report what it displays`

The write succeeded and the monitor read the value back. That is all DDC/CI can
tell anyone. VCP 0x60 is a register: reading it returns what was last stored,
and a monitor that accepts a value and declines to act on it reads back exactly
like one that switched. One held `0x0F` for several minutes while showing VGA.

Believe the screen, not the read-back.

### `issued, but the monitor did not answer afterwards`

For a moment after a monitor changes input, the computer re-detects it, and a
read in that window finds nothing. That is normal, and the switch usually
worked. If the screen went dark instead, see above.

### `... is not attached: nothing present matches serial ...`

The configured monitor is not there: off, unplugged, or on Windows, turned off
in Display settings. Check `desktop-switcher monitors`.

### `... is connected but not part of the desktop` (Windows)

Windows knows the monitor is plugged in, but it is not part of the desktop, so
there is no way to talk to it. Turn it back on in **Settings → System →
Display**.

### `... is mirrored with N other display(s)` (Windows)

The desktop is set to duplicate rather than extend, so the mirrored displays
share one Windows handle, and Windows returns their physical monitors in an
order nothing ties to a particular display. A write could land on the wrong
screen, so the tool refuses.

Right after a monitor changes input, Windows can briefly show displays as
mirrored while it re-detects them. A write waits a few seconds for that to
settle before believing it; if you still see this message, the desktop really is
mirrored. Set it to **Extend** in Display settings.

### `... is ambiguous: N displays match`

Two displays report the same serial, so the tool cannot tell which is which and
will not guess. This usually means a monitor publishes a placeholder serial.
`desktop-switcher monitors` shows what is duplicated.

### `Cabling changed, so its verified input codes may point at the wrong physical port`

The monitor was found by its serial, but on a different connection than the one
recorded. A cable moved, and the recorded codes describe which of the monitor's
sockets each computer used — which may no longer be true.

Fix: run `desktop-switcher configure` to record the new connection, then
switch to each input once and look, since the codes may have moved with the
cable. Update the labels in `config.toml` if they did.

---

## Desktop attachment (layer 2)

Changing a monitor's input and whether *this* computer still draws on that
monitor are separate things. Both computers can have a monitor attached at once,
which is how a window ends up on a screen you cannot see.

| Command | What it does | State |
|---|---|---|
| `displays` | what this desktop sees, attached or not, and each connector | working |
| `sweep` | move windows off a monitor, leave the display alone | working |
| `restore-windows` | put swept windows back | working |
| `release` / `claim` / `primary` | detach, reattach, make primary | **experimental**, behind `--experimental` |

The topology commands use `SetDisplayConfig`. On the hardware this was built
against they have left both external displays mirrored at the origin instead of
extended, and Windows' own `Screen` API collapses a mirrored pair into one
entry, so it looks exactly like a missing monitor. `displays` detects mirroring
and says so.

The legacy `ChangeDisplaySettingsEx` detach, tried first, is rejected outright
by that driver with `DISP_CHANGE_BADMODE`.

Use `sweep`: it solves the stranded-window problem without touching topology,
and cannot leave you short a screen. If a topology command leaves your primary
display wrong, fix it in **Settings → System → Display** with **Make this my
main display**.

---

## Windows

### A monitor answers slowly, or only sometimes

Short DDC/CI exchanges — reading or setting an input — are retried a few times
when Windows reports them failed, because a converter cable or adapter can drop
the odd transaction. The monitor's capabilities string is a much longer
transfer, and a marginal cable can fail it every time while switching works
perfectly. That only affects `configure`'s list of suggested inputs, never
switching; `configure` says the monitor "reported no input list", and you add
the codes by hand.

### A monitor is listed as `built-in panel`

That is the laptop's own screen. It has no inputs to switch, is never a target,
and is never detached: it is the display that is always there to recover with.
It is recognised from what the graphics driver reports, not from a guess.

### Upgrading from the PowerToys backend

Configurations that say `backend = "powertoys-cli"` load unchanged and use the
built-in Windows backend; the next save writes `backend = "windows"`. Monitor
ids are the same format, so every binding carries over. PowerToys is no longer
needed for this tool.

### A hotkey does nothing

The tray icon registers the hotkeys, so it has to be running: opening Desktop
Switcher from the Start Menu starts it. If a combination is held by another
program, the tray shows "A hotkey is not available" once, naming it, and keeps
trying every couple of seconds; pick a different combination, or close the
program holding it.

Older versions carried hotkeys on per-action Start Menu shortcuts. The
installer removes those, since while they exist Explorer holds their keys and
the tray cannot have them.

### Ghost icons in the notification area

A tray icon whose program was killed stays drawn until the mouse passes over
it. The installer asks the running tray to close rather than killing it, so
this should not happen on upgrade; hovering clears any left over.

### `An Application Control policy has blocked this file` (os error 4551)

Smart App Control. It blocks unsigned executables it has no reputation for, and
every rebuild is a new hash, so a binary that ran a minute ago is blocked after
the next `cargo build`. It hits the installed copy, the copy on `PATH` and the
one in `target/`, and makes the CLI integration tests fail, since they spawn the
built binary. `cargo test -p switcher-core` spawns nothing and still exercises
every rule.

Turning Smart App Control off (Windows Security → App & browser control)
cannot be undone without reinstalling Windows, so it is a real decision about
the machine. Signing the binaries is the other way out.

### `LNK1104: cannot open file ... .exe` while building

Something is holding a freshly linked executable open — usually an antivirus
scan of the new file, or a still-running instance. Wait a moment and build
again; `install-windows.ps1` stops a running GUI for the same reason.

---

## Ubuntu

### `permission denied` on `/dev/i2c-*`

Do **not** fix this by running the tool with `sudo`.

```bash
sudo usermod -aG i2c "$USER"      # then log out and back in
```

If `/dev/i2c-*` does not exist at all, the kernel module is not loaded:

```bash
sudo modprobe i2c-dev
echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf   # persist across boots
```

See <https://www.ddcutil.com/i2c_permissions/>.

### `ddcutil detect` finds nothing, or says `Invalid display`

- Check DDC/CI is enabled in each monitor's on-screen menu.
- A monitor asleep on an input with no picture shows up as `Invalid display`;
  see "The screen went dark" above.
- Some adapters and hubs do not pass DDC through.
- Run `scripts/ubuntu-preflight.sh`; it checks the kernel module, device
  permissions and connectors separately, so it can tell you which layer is the
  problem.

### Displays are numbered differently than last time

They will be. `ddcutil` display numbers and I²C bus numbers are discovery-time
addresses, not identities, and change when a monitor is hot-plugged. The tool
never uses them to address a write: it selects by serial and model, and only
resolves a bus number when a monitor publishes no serial at all.

### `unexpected output from ...`

`ddcutil` printed something the adapter does not understand, probably after an
upgrade. It refuses to guess, because misreading a monitor id means writing to
the wrong screen. Capture the new output into `tests/fixtures/ddcutil/`, run
`cargo test -p switcher-backend-ddcutil`, and update the parser against the
failures.

### No icon in the top bar

The icon is a StatusNotifierItem, and stock GNOME has nowhere to show one.
Check that the *AppIndicator and KStatusNotifierItem Support* extension is
enabled (`gnome-extensions list --enabled | grep -i appindicator`; Ubuntu ships
it as `ubuntu-appindicators@ubuntu.com`). The icon waits for it and appears as
soon as it is on; after 30 seconds without one it shows a notification saying
so. The hotkeys belong to GNOME and work either way.

To check the icon is running: `gdbus call --session --dest org.freedesktop.DBus
--object-path /org/freedesktop/DBus --method
org.freedesktop.DBus.GetConnectionUnixProcessID
io.github.berkantalperen.DesktopSwitcherTray` prints its process id, or an
error if it is not running. `install-linux.sh` starts it, and it starts with
every session after that.

### Some GNOME shortcuts stopped working

An earlier `install-gnome-shortcuts.sh` could leave a malformed entry in
GNOME's custom-shortcut list, which crashes the service behind every GNOME
shortcut, not just ours. Running the current script repairs the list; then
restart the service with
`systemctl --user restart org.gnome.SettingsDaemon.MediaKeys.target`.

---

## Monitors, generally

### Brightness changes work but input switching does not

Different VCP features. Brightness working proves DDC/CI is alive; it proves
nothing about `0x60`.

If the AOC panels stop responding to DDC entirely, check Eco Mode in the
on-screen menu — setting it to Standard has fixed DDC responsiveness on these
monitors before.

### A code in the capabilities list does nothing

Normal. Capability strings list inputs that do not exist, omit ones that do, and
sometimes name the wrong socket. `configure` records them as `reported` for
exactly this reason. Switch to each once and look at the screen; give the ones
that work a label, and the tray offers only those.

### The same computer needs a different code on each monitor

Also normal. The code names the monitor's socket, not the computer, and nothing
makes two monitors cabled differently agree. On the desk this was built on, the
same laptop was once HDMI (`0x11`) on one panel and DisplayPort (`0x0F`) on the
other. Every monitor is its own mapping, and an action sets each one explicitly.

---

## Diagnostics to include in a bug report

```bash
desktop-switcher doctor
desktop-switcher monitors
desktop-switcher displays     # Windows
```

Plus the tail of the event log (`doctor` prints its path). It records commands,
exit statuses and outcomes with timestamps.
