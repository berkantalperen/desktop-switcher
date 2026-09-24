# Hardware inventory

Evidence captured from the machines the tool was built against; nothing is
inferred from model numbers. The sections below are a **dated log, oldest
first**, kept because each design rule in the README traces back to something
observed here. Status lines inside an older section were true when written and
may since have been overtaken; where it matters, they say so.

## Current setup (2026-09-24)

| | Laptop (BERKANT16X) | Workstation (HP Z4 G4) |
|---|---|---|
| OS | Windows 11 Home 10.0.26200 | Ubuntu 26.04.1, GNOME 50.1 (Wayland) |
| Backend | `windows` (dxva2, built in) | `ddcutil-cli`, ddcutil 2.2.5 |
| Hotkeys | the tray registers them | GNOME custom shortcuts |
| Tray | notification area | top bar, via Ubuntu's AppIndicator extension |

Two AOC 27P2DG5 panels, each cabled to both computers identically; every code
below was confirmed by someone watching the switch:

| Panel | Serial | Laptop | Workstation |
|---|---|---|---|
| left | `ASFPA9A001109` | HDMI-1 `0x11` (through an HDMI↔DP cable) | DisplayPort-1 `0x0F` (`card1-DP-4`) |
| center | `ASFPA9A001108` | HDMI-1 `0x11` | DisplayPort-1 `0x0F` (`card1-DP-1`) |

The facts that shaped the design, each detailed below:

- The input register reports what was last **stored**, not what is shown — one
  panel read `0x0F` for a week while showing HDMI. So a read-back is never
  confirmation, and a write is never skipped because the read already matches.
- A panel on an input with no picture **sleeps and ignores DDC/CI entirely**.
- An awake panel answers **both** computers, whichever it is showing.
- The converter cable carries short DDC/CI exchanges but not the long
  capabilities string. PowerToys hid that panel for it, which is why the
  Windows backend now talks to Windows' API directly and discovery never talks
  DDC.
- Windows briefly shows displays as missing or mirrored after a panel changes
  input, so writes wait for the topology to settle.
- Two panels showing the same computer can need different codes, so every
  monitor is its own mapping.

---

Each claim below is tagged with how strongly it was known at the time:

| Tag | Meaning |
|---|---|
| `reported` | The monitor said so in its capabilities string. Often wrong. |
| `read-confirmed` | We read the value back over DDC/CI. |
| `write-confirmed` | We wrote it and read it back. |
| `user-confirmed` | A human watched the physical result. |
| `unknown` | Not established. |

---

## Windows host — captured 2026-09-16

At the time the Windows backend drove PowerToys' Power Display CLI, and the
monitors were cabled differently; both have since changed (see 2026-09-23).

| | |
|---|---|
| OS | Windows 11 Home Single Language 10.0.26200 |
| PowerToys | 0.101.2362.0 (`read-confirmed`) |
| Power Display module | running (`PowerToys.PowerDisplay` process present) |
| CLI path | `C:\Users\berka\AppData\Local\PowerToys\WinUI3Apps\PowerToys.PowerDisplay.Cli.exe` |
| Rust toolchain | 1.98.1 stable, `x86_64-pc-windows-msvc` |

The CLI is **not** on `PATH`. The path above was found by searching the
PowerToys install directory, and the backend does the same search at runtime.

### Displays

| Logical | Model (EDID) | Serial | Connection id | Transport | Input now |
|---|---|---|---|---|---|
| *(internal panel)* | BOE NE16NZH | none published | `\\?\DISPLAY#BOE0CBF#4&34b9e9a7&0&UID8388688` | WMI | n/a |
| *(unassigned)* | AOC 27P2DG5 | `ASFPA9A001108` | `\\?\DISPLAY#AOC2702#5&23e00778&0&UID4613` | DDC/CI | HDMI-1 `0x11` (`read-confirmed`) |
| *(unassigned)* | AOC 27P2DG5 | `ASFPA9A001109` | `\\?\DISPLAY#AOC2702#5&23e00778&0&UID4612` | DDC/CI | DisplayPort-1 `0x0F` (`read-confirmed`) |

Notes on identity:

- **The two AOC panels publish distinct EDID serial numbers.** This is the
  single most useful finding for Gate A: the ambiguity problem the plan worried
  about does not exist on this host. Bindings are made against the serial and
  corroborated with the connection id.
- Serials are read from the Windows device registry
  (`HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY\...\Device Parameters\EDID`),
  which is readable without elevation. PowerToys' `list` does not print them.
- The EDID model name is **27P2DG5**; the capabilities string reports
  `model(27P2Q)`. Both names refer to these panels. The project plan referred to
  them as 27P2Q.
- Both AOCs share the adapter instance `5&23e00778&0` and differ only in the
  `UID` suffix, i.e. two outputs of the same adapter.
- The laptop panel appears over WMI, not DDC/CI, and is excluded from switching
  automatically. It stays available as the Windows recovery display.

### Reported input sources

Both AOC panels report exactly the same list (`reported`):

```
0x60 Input Source: VGA-1 (0x01), DVI-1 (0x03), HDMI-1 (0x11), DisplayPort-1 (0x0F)
```

Raw capability string, identical on both panels, MCCS 2.2:

```
(vcp(02 04 05 08 10 12 14(01 05 06 08 0B) 16 18 1A 52 60(01 03 11 0F ) 62 86(02 05)
C8 C9 CC(01 02 03 04 05 06 07 09 0A 0B 0C 0D 0E 12 14 16 1E) B6 DF C6
DC(00 0B 0C 0D 0E 0F 10) D6(01 04) ED F8)prot(monitor)type(LCD)
cmds(01 02 03 07 0C F3)mccs_ver(2.2)asset_eep(64)mpu_ver(005)model(27P2Q)mswhql(1))
```

This is a claim by the monitor. It does not establish that all four inputs
exist physically, nor which one any computer is plugged into.

### The finding that shapes the design

**The two panels are on different inputs for the same computer.**

- `ASFPA9A001108` shows Windows over **HDMI-1 (`0x11`)**
- `ASFPA9A001109` shows Windows over **DisplayPort-1 (`0x0F`)**

So "switch both monitors to Windows" is not one value written to two monitors.
It is two different values. Any design that batches a single input code across
both displays would be wrong on this hardware. The tool therefore issues one
write per monitor, with that monitor's own code, and reports each separately.

### Raw evidence

Verbatim CLI output is preserved in `tests/fixtures/powertoys/`, with stdout,
stderr and exit status captured separately. No code reads it any more; it is
kept as evidence.

---

## Ubuntu host (HP Z4) — partially captured 2026-09-16

Captured over SSH with the read-only preflight script plus direct sysfs reads,
before `ddcutil` was installed; the subsections after "ddcutil — installed and
working" follow on from there.

| | |
|---|---|
| Hostname | `berkant-HP-Z4-G4` |
| OS | Ubuntu 26.04.1 LTS (Resolute Raccoon) |
| Kernel | 7.0.0-31-generic |
| GPU | NVIDIA Quadro M2000 (GM206GL), `21:00.0` |
| GPU driver | proprietary `nvidia` 580.178.04 |
| User | `berkant`, in the `sudo` group |
| `ddcutil` | **not installed** (`read-confirmed`) |

### Graphics outputs

The Quadro M2000 presents four DisplayPort connectors and **no HDMI**:

| Connector | Status |
|---|---|
| `card1-DP-1` | disconnected |
| `card1-DP-2` | disconnected |
| `card1-DP-3` | disconnected |
| `card1-DP-4` | **connected** |

**Only one display was linked to the Z4** then. See "The cabling question"
below; it was settled by recabling on 2026-09-23.

EDID is not exposed through sysfs — every connector reports `edid=0B`, which is
normal for the proprietary NVIDIA driver. Identity on this host therefore has to
come from `ddcutil detect`, not from sysfs as it does on Windows.

### I²C / DDC readiness

Good news, and one blocker:

- `i2c-dev` is **built into this kernel**, not a module. No `modprobe` is
  needed, and the preflight script's advice to load it does not apply here.
- The GPU's DDC lines **are** exposed to userspace, which is the thing that most
  often stops ddcutil working with the proprietary NVIDIA driver:

  | Device | Adapter |
  |---|---|
  | `/dev/i2c-0` | SMBus I801 adapter at `0000:00:1f.4` (chipset, not a display) |
  | `/dev/i2c-1` … `/dev/i2c-5` | `NVIDIA i2c adapter N at 21:00.0` |

- **Permissions block access.** All are `crw------- root root`, so nothing can
  reach them without `sudo`. The `ddcutil` package ships a udev rule and an
  `i2c` group that fix this properly; there is no `i2c` group on the host yet.

### ddcutil — installed and working

`ddcutil 2.2.5`. After `sudo apt install ddcutil` and adding the user to the
`i2c` group, all six buses are readable and writable without `sudo`.

`ddcutil detect` finds exactly one display:

| | |
|---|---|
| Display number | 1 (a discovery-time address, not identity) |
| I²C bus | `/dev/i2c-5` = `NVIDIA i2c adapter 9 at 21:00.0` |
| DRM connector | `card1-DP-4` |
| Mfg / Model | AOC / 27P2DG5 |
| **Serial** | **`ASFPA9A001108`** (`read-confirmed`) |
| DDC responsive | yes — `I2C address 0x37 (DDC) responsive: true` |
| VCP version | 2.2 |
| Controller | Mstar, firmware 0.1 |

**The serial matches the one Windows reports for that panel.** This is the
result Gate A actually needed: a monitor can be identified as the same physical
device from both computers. The capability string is byte-identical across the
two hosts as well.

### Two findings that make the project viable

1. **DDC works from the computer that is not being displayed.** The Z4 reads
   `getvcp 60` as `HDMI-1 (0x11)` — the *Windows* input — while Windows is
   actively driving that monitor over HDMI. The panel keeps answering DDC on an
   input it is not currently showing, which is what allows either computer to
   pull the monitor to itself.
2. **The NVIDIA proprietary driver exposes its DDC lines.** `/dev/i2c-1`
   through `/dev/i2c-5` are `NVIDIA i2c adapter N at 21:00.0`. This is the usual
   reason ddcutil fails on this driver stack, and it is not a problem here.

### Cabling, now settled for the connected panel

| Panel | Windows | Ubuntu |
|---|---|---|
| `ASFPA9A001108` | HDMI-1 (`0x11`, `read-confirmed`) | DisplayPort via `card1-DP-4` |
| `ASFPA9A001109` | DisplayPort-1 (`0x0F`, `read-confirmed`) | not cabled (confirmed by the user) |

For panel `ASFPA9A001108`, the Ubuntu input is almost certainly
**DisplayPort-1 (`0x0F`)**: the Z4 is cabled to it over DisplayPort, and the
panel advertises exactly one DP input. That is a strong inference, not
evidence — it stays `reported` until a human watches the switch happen. See
`docs/test-matrix.md`.

### Toolchain on this host

Rust 1.98.1 (user-scoped rustup, no elevation) and gcc 15.2.0. The workspace
builds, passes `cargo fmt --check`, `cargo clippy -D warnings` and its full
test suite natively here, and the release binary is installed at
`~/.local/bin/desktop-switcher`.

`doctor`, `monitors` and `inspect` were run against the real monitor from this
host and produce the same identification, the same capability string and the
same evidence labelling as the Windows side.

Five tests are skipped on Linux relative to Windows; they are the Windows
registry EDID tests, which are `cfg`-gated.

### Verified mapping for `ASFPA9A001108`

Both codes are now `user-confirmed`, and the Z4's configuration is complete:

| Destination | Code | How it was established |
|---|---|---|
| `windows` | `0x11` (HDMI-1) | Read from the monitor while the user confirmed it was displaying Windows. No write. |
| `ubuntu` | `0x0F` (DisplayPort-1) | `test-input` issued the write from the Z4 on 2026-09-16; the user watched the panel switch to Ubuntu. |

`desktop-switcher doctor` on the Z4 reports no blockers.

Two further observations from that test:

- **`setvcp 60 0x0F` works on this hardware.** The panel switched on the first
  write, with no retry.
- **Readback tracks reality, including manual changes.** After the user
  returned the panel to Windows with the monitor's own buttons, five
  consecutive reads all reported `0x11`. *Overtaken on 2026-09-23:* the other
  panel of the same model held a stale value for a week, so no read-back is
  trusted as confirmation (see the README's safety model).

The "destination" names above (`windows`, `ubuntu`) are from the first
configuration schema, which was organised around computers. Schema 2 knows
only monitors and inputs; an old file's destinations are upgraded into actions
and input labels on load.

### The cabling question (settled 2026-09-23)

Windows sees panel `...108` on **HDMI-1** and panel `...109` on
**DisplayPort-1**. The Z4 can only drive DisplayPort. Each AOC has a single DP
input. So panel `...109`, whose DP input is already occupied by Windows, cannot
also be cabled to the Z4.

That is consistent with exactly one Z4 connector being live, and the likely
physical reality is:

| Panel | Windows | Ubuntu |
|---|---|---|
| `ASFPA9A001108` | HDMI | DisplayPort (`card1-DP-4`) |
| `ASFPA9A001109` | DisplayPort | **not connected** |

One detail supports this rather than a merely-sleeping link: panel `...108` is
currently displaying Windows over HDMI, yet the Z4 still sees its DP input as
connected. So these panels keep hot-plug detect asserted on an input that is not
selected — meaning a second cabled panel would also have shown as connected.

**If this is right, only one of the two monitors can currently be switched
between the computers**, and the project's two-monitor goal needs a second cable
into panel `...109` — which would mean a DisplayPort→HDMI or →DVI conversion,
since that panel's DP is taken and the M2000 has no HDMI output.

This needs physical confirmation by counting cables at the back of the monitors;
it is not something either computer can settle on its own.

---

## Unknowns as of 2026-09-16

All resolved or made moot by the 2026-09-23 section below.

1. **Physical left/right placement.** Nothing yet maps `ASFPA9A001108` and
   `ASFPA9A001109` to the physical left and right monitors. `configure` asks a
   human. Two ways to tell them apart today: the serial is printed on the label
   on the back of each panel, and — conveniently — the two are currently on
   different inputs, so each monitor's own on-screen menu shows which is which
   (one reads HDMI, the other DisplayPort).
2. **Cable path on Windows.** Whether the AOCs are connected directly to the
   laptop or through the HP G6 dock is not established. Both share one adapter
   instance, which is consistent with either.
3. **No input-switch write has been performed.** No code in this repository has
   ever written VCP `0x60` to these monitors. Everything above is from reads.
   The mapping for Ubuntu cannot be established without a controlled write; see
   `docs/test-matrix.md` for that procedure.
4. **Behaviour when Power Display is not running** is untested. The adapter
   classifies such a failure and reports it, but the exact message PowerToys
   emits has not been observed.

---

## Recabled, both panels on both hosts — captured 2026-09-23

Supersedes the unknowns above where they overlap: the left/right question is
settled by label (`ASFPA9A001109` is the left panel), writes have been performed
and watched many times, and PowerToys is no longer involved.

**Cabling.** Both panels are now cabled to both computers, identically, and
every mapping was confirmed by watching the switch happen:

| Panel | Laptop | Workstation (Z4) |
|---|---|---|
| `ASFPA9A001108` (center) | HDMI-1, `0x11` | DisplayPort-1, `0x0F` |
| `ASFPA9A001109` (left) | HDMI-1, `0x11`, through an HDMI↔DP cable | DisplayPort-1, `0x0F` |

On the Z4 the panels are `card1-DP-1` (center) and `card1-DP-4` (left). On the
laptop both sit on the adapter instance `4&34b9e9a7&0`, the same one as the
built-in panel, and the driver reports **DisplayPort** for both — including the
one plugged into the monitor's HDMI socket. The driver describes its own end of
the cable.

**The left panel's register lied for a week.** At the start of the session it
read `0x0F` while physically on HDMI showing the laptop; `0x0F` had simply been
the last value written to it.

**The converter cable carries short DDC/CI exchanges but not the capabilities
string.** Through Windows' API directly, the left panel answers VCP `0x60` reads
and writes; `GetCapabilitiesStringLength` succeeds with length 0, every time.
PowerToys hid the panel entirely for that reason — the trigger for replacing it
with the built-in Windows backend. The center panel returns its full 302-byte
string.

**Awake panels answered both computers.** The Z4 read both panels while both
showed the laptop, and the laptop read and wrote the center panel while it
showed the Z4. Silence meant asleep, not "busy with the other computer".

**A panel on an input with no picture sleeps, and then ignores everything.**
With the Z4's GNOME session idled out (5-minute idle delay, locked, both outputs
at `dpms=Off`, Mutter `PowerSaveMode` 3), panels switched to it went dark.
Reads from the laptop returned `0xC0262589`; the Z4's ddcutil reported
`Invalid display`; a `SetVCPFeature` from the laptop succeeded at the API and
changed nothing, the register afterwards still holding `0x0F`. Setting Mutter's
`PowerSaveMode` to 0 over SSH woke the Z4's outputs, and the panel woke with
them and answered both computers again.

**Windows re-detects a panel when it changes input.** For a second or two after
a panel arrives on the laptop's input, `QueryDisplayConfig` can omit it or show
the pair as sharing one source (mirrored). One action's second step hit that
window and refused; the backend now waits for the topology to settle before a
write.
