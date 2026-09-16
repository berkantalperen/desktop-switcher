# Hardware inventory

Stage A evidence. Everything here was captured from the machines themselves;
nothing is inferred from monitor model numbers or from the project plan.

Each claim is tagged with how strongly it is known:

| Tag | Meaning |
|---|---|
| `reported` | The monitor said so in its capabilities string. Often wrong. |
| `read-confirmed` | We read the value back over DDC/CI. |
| `write-confirmed` | We wrote it and read it back. |
| `user-confirmed` | A human watched the physical result. |
| `unknown` | Not established. |

---

## Windows host — captured 2026-09-16

**Status: complete for this host.**

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
stderr and exit status captured separately. The parser tests run against those
files, so a future PowerToys release that changes the output format breaks the
tests rather than silently misreading a monitor id.

---

## Ubuntu host (HP Z4) — partially captured 2026-09-16

**Status: system inspected, DDC not yet reachable.** Captured over SSH with the
read-only preflight script plus direct sysfs reads. No write of any kind was
made to this host.

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

**Only one display is linked to the Z4.** See "The cabling question" below;
this is the most consequential open item in the project.

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

### Outstanding on this host

| Item | Status |
|---|---|
| Input code that selects Ubuntu, per panel | `reported` — inferred, not yet proven by a switch |
| Behaviour of `setvcp` on this hardware | `unknown` — no write has been issued |

### The cabling question

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

## Outstanding unknowns on both hosts

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
