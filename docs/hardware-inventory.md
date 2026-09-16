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

## Ubuntu host (HP Z4) — NOT YET CAPTURED

**Status: outstanding.** Everything below is unknown.

| Item | Status |
|---|---|
| `ddcutil` installed and version | `unknown` |
| `i2c-dev` loaded, `/dev/i2c-*` permissions without sudo | `unknown` |
| Which displays `ddcutil detect` finds | `unknown` |
| Whether ddcutil reports the same serials as Windows does | `unknown` |
| Which monitor-side port the Ubuntu cable occupies on each panel | `unknown` |
| Input code that selects Ubuntu, per panel | `unknown` |

To capture it, run `scripts/ubuntu-preflight.sh` on the Z4. It is read-only: it
never runs `setvcp` and never changes system configuration. Its output replaces
the synthetic fixtures in `tests/fixtures/ddcutil/`, which currently exist only
so the parser has something to test against.

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
