# Troubleshooting

Start with `desktop-switcher doctor`. It is read-only, checks the backend,
permissions, configuration and monitor identification, and exits non-zero when
something would block a switch.

## Recovery first

If a screen is blank or unresponsive:

1. **Use the monitor's own buttons.** Open the on-screen menu and select the
   input by hand. This always works and needs no computer. It is the reason
   this tool never takes an action you cannot undo physically.
2. On the laptop, the built-in panel remains available; Windows falls back to it
   when the external displays disappear.
3. Once a screen is back, run `desktop-switcher switch <destination>` again.
   Every switch sets an absolute input, so repeating it is safe and never cycles.

---

## "Refusing to switch"

This is the tool working. It refuses rather than writing a code it cannot
justify. The message names the reason.

### `... is only reported, not user-confirmed`

An input code exists in the configuration but no human has ever watched it work.
`reported` means the monitor listed it in its capabilities, which is a claim,
not evidence — capability strings routinely list inputs a panel does not have.

Fix: verify it with a real switch.

```bash
desktop-switcher test-input --monitor left --code 0x0F --destination ubuntu
```

### `... is not attached: nothing present matches serial ...`

The configured panel is not there. It is off, unplugged, asleep, or connected to
the other computer and not answering this one.

Fix: check `desktop-switcher monitors`. If the display is genuinely gone,
nothing can switch it.

### `... is ambiguous: N displays match`

Two displays report the same serial, so a binding cannot pick one. The tool
will not guess, because a wrong guess writes to the wrong screen.

Fix: this usually means a panel publishes a placeholder serial. Run
`desktop-switcher monitors` to see what is duplicated.

### `Cabling changed, so its verified input codes may point at the wrong physical port`

The panel was found by its serial, but on a different connection than the one
recorded. A cable moved. The verified codes describe which monitor-side port
each computer occupies, and that assumption no longer holds.

Fix: `desktop-switcher configure`, then re-verify the codes with `test-input`.

### `has no input code recorded for ...`

Half the mapping exists. The code for this computer can be established by
reading; the code for the other computer cannot, and needs `test-input`.

---

## Desktop attachment (layer 2)

`switch` changes the monitor's input. Whether *this* computer still draws on
that monitor is a separate thing, and both computers can have it attached at
once — which is why a window can end up on a screen you cannot see.

| Command | What it does | State |
|---|---|---|
| `displays` | what this desktop sees, and what is attached | **working** |
| `sweep` | move windows off a monitor, leave it attached | **working** |
| `primary` | make a monitor the primary display | **unreliable**, see below |
| `release` | detach a monitor from this desktop | **not working**, see below |
| `claim` | reattach a released monitor | untested — nothing has been released |

### `release` fails with DISP_CHANGE_BADMODE

The legacy `ChangeDisplaySettingsEx` detach does not work on the NVIDIA/Intel
driver this was developed against. Two DEVMODE recipes were tried — a fully
zeroed one, and one derived from the live mode with only the size fields
blanked — and both are rejected with `DISP_CHANGE_BADMODE (-2)` even when the
display is not primary.

`SetDisplayConfig` is the modern API for topology changes and is the likely
fix, but it is a separate piece of work and is not implemented.

Until then, use `sweep`: it solves the stranded-window problem without
touching topology at all, and cannot leave you short a screen.

### `primary` fails with DISP_CHANGE_FAILED

Promotion succeeded once and has since been refused in the reverse direction
with `DISP_CHANGE_FAILED (-1)`. The geometry is right — the origin moves and
every other display shifts by the same delta, staged and committed as one
batch — but the driver rejects some arrangements for reasons not yet
established.

If `release` moved your primary and could not put it back, it says so. Fix it
by hand: **Settings → System → Display**, select the display you want, and
tick **"Make this my main display"**. Windows repositions the rest for you.

## Windows

### The Power Display CLI cannot be found

PowerToys does not put it on `PATH`. The backend searches
`%LOCALAPPDATA%\PowerToys\WinUI3Apps`, `%ProgramFiles%\PowerToys\...` and
`PATH`. If your install is elsewhere, pin it:

```toml
tool_path = "D:\\Apps\\PowerToys\\WinUI3Apps\\PowerToys.PowerDisplay.Cli.exe"
```

### A monitor is listed with transport `WMI`

That is the internal laptop panel. WMI carries brightness, not input selection.
It is excluded from switching on purpose and left available for recovery.

### `unexpected output from ...`

PowerToys changed its output format. The adapter refuses to guess, because
misreading a monitor id means writing to the wrong screen.

Fix: re-capture the fixtures in `tests/fixtures/powertoys/`, run
`cargo test -p switcher-backend-powertoys`, and update the parser against the
failures. Add the new version to `TESTED_VERSIONS`.

### A monitor stops responding after a switch

Expected, if it switched to the other computer: it is no longer talking to this
one. The tool reports `issued, visual state unconfirmed` rather than claiming
success. Confirm from the other computer, or by looking at the screen.

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

### `ddcutil detect` finds nothing

- Check DDC/CI is enabled in each monitor's on-screen menu. Some panels ship
  with it off.
- Some adapters and hubs do not pass DDC through.
- Run `scripts/ubuntu-preflight.sh` and read the whole report; it checks the
  kernel module, device permissions and DRM connectors separately, so it can
  tell you which layer is the problem.

### Displays are numbered differently than last time

They will be. `ddcutil` display numbers and I²C bus numbers are discovery-time
addresses, not identities. The tool never uses them to address a write: it
selects by `--sn`/`--model`, and only resolves a bus number when a panel
publishes no serial at all.

---

## Monitors, generally

### Brightness changes work but input switching does not

These are different VCP features. Brightness working proves DDC/CI is alive;
it proves nothing about `0x60`.

If the AOC panels stop responding to DDC entirely, check Eco Mode in the
on-screen menu — setting it to Standard has fixed DDC responsiveness on these
monitors before.

### A code in the capabilities list does nothing

Normal. Capability strings are unreliable: they list inputs that do not exist,
omit ones that do, and sometimes name the wrong port. The tool labels them
`reported` for exactly this reason, and refuses to switch on them.

---

## Diagnostics to include in a bug report

```bash
desktop-switcher doctor
desktop-switcher monitors
desktop-switcher inspect
```

Plus the tail of the event log — `doctor` prints its path. It records commands,
exit statuses and outcomes with timestamps.
