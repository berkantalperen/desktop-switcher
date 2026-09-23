# Test matrix

Three tiers. The first two run unattended; the third must not.

## Tier 1 — automated, no hardware

`cargo test --workspace`: 174 tests at the time of writing, passing on Windows
and on Linux (where five Windows-only tests are compiled out).

They cover, among other things:

| Property | Where |
|---|---|
| A bare decimal input code is rejected as ambiguous | `switcher-core::types` |
| Only a person watching can make an input trusted; the word "confirmed" is never used for a read-back | `switcher-core::types` |
| `0x60` is read from the `vcp(...)` section only, never from elsewhere in a capability string | `switcher-core::mccs` |
| Malformed, truncated and oversized capability strings fail loudly rather than parsing to an empty list | `switcher-core::mccs` |
| A monitor with no usable EDID serial yields `None`, not a fabricated identifier | `switcher-core::edid` |
| A hung backend process is killed and reported as a timeout | `switcher-core::proc` |
| Arguments are passed as an array, so a serial containing a `;` cannot become a second command | `switcher-core::proc` |
| Reordered discovery does not change which monitor a configured entry binds to | `switcher-core::inventory` |
| Duplicate serials, missing monitors and moved cables all block the switch instead of resolving to a guess | `switcher-core::inventory` |
| The internal laptop panel is never a switch target | `switcher-core::inventory` |
| Each monitor is written its own code; nothing is batched | `switcher-core::switch` |
| A read that already matches never suppresses the write, and a stale read cannot block a recovery | `switcher-core::switch` |
| A write that is accepted but ignored is not reported as success | `switcher-core::switch` |
| One monitor failing yields a partial report, with no rollback attempted | `switcher-core::switch` |
| A stale lock file does not wedge the tool; the last-request record survives a round trip to disk | `switcher-core::switch` |
| A configuration naming the PowerToys backend loads as the Windows backend and saves under the new name | `switcher-core::config` |
| A display is listed without anything being asked of it, and without an EDID | `switcher-backend-windows::topology` |
| Ids match exactly: `...UID4165` never addresses `...UID41650` | `switcher-backend-windows::topology` |
| Mirrored displays and duplicate ids are refused; the built-in panel and detached displays are never addressed | `switcher-backend-windows::topology` |
| A reshuffle is waited out; a desktop that stays mirrored, or a structural refusal, is still refused | `switcher-backend-windows::topology` |
| A zero reply is a failed read, not an input; real AOC capabilities yield their advertised inputs | `switcher-backend-windows::topology` |
| A read-back is never recorded as evidence, and switching to a configured input prints nothing needing a person | `switcher-cli` integration tests |
| The interface never names a computer | `switcher-cli` integration tests |
| Read-only commands never create or write a configuration | `switcher-cli` integration tests |

## Tier 2 — parser fixtures

Real captured output, byte for byte, in `tests/fixtures/`.

- `ddcutil/` — the Linux backend's parser is tested against these. See the
  README in that directory for how to capture new ones.
- `powertoys/` — captured from PowerToys 0.101.2362.0 on 2026-09-16. No code
  reads these any more; they are kept as the evidence behind
  [hardware-inventory.md](hardware-inventory.md).

## Tier 3 — hardware, human present

**Never run these unattended, and never in CI.** Each one can leave a screen
dark until someone presses buttons on the monitor or wakes a computer.

Before starting, confirm recovery works: a laptop's built-in panel is
available, you can reach each monitor's on-screen menu by hand, and the
computer on each input you will switch to has its screen **on** — a monitor
sent to a blanked computer goes to sleep.

### Current cabling, 2026-09-23

Both AOC 27P2DG5 panels are cabled to both computers, identically: the laptop
on HDMI-1 (`0x11`), the workstation on DisplayPort-1 (`0x0F`).

| # | Case | Pass condition | Status |
|---|---|---|---|
| 16 | Recabled codes re-verified by eye | Each monitor, each code, switch watched | **passed 2026-09-23** — all four; the left panel's laptop code proved to be `0x11`, not the `0x0F` its register had reported for a week |
| 17 | Windows backend discovers a panel PowerToys hid | Listed, bound, read | **passed 2026-09-23** — left panel's capabilities fail over its converter cable every time; short reads and writes work |
| 7 | Both monitors, each direction, from each computer | Both switch; per-monitor result reported | **passed 2026-09-23** — from the Z4 (ddcutil) and from the laptop (Windows backend), both directions, watched |
| 9 | Repeated same request | The same code is written again; no cycling, no visible change | **passed 2026-09-23** — left panel already on `0x11`, `set 0x11` wrote and nothing changed on screen |
| 4b | DDC reachable from the non-displaying computer | The other computer can read and write the monitor | **passed 2026-09-23** — an *awake* panel answered both computers whatever input it showed; the laptop pulled the center panel back from the Z4 on its own |
| 15 | Switching to a computer whose screens are off | Reported honestly; recovery documented | **passed 2026-09-23** — panels went dark and ignored reads *and* writes from both computers; waking the Z4's screens woke them. See troubleshooting, "The screen went dark". |
| 18 | Windows re-detection between the steps of an action | The second step waits and succeeds | **passed 2026-09-23** after a fix — the first run refused on a transiently "mirrored" desktop |
| 8 | Mixed initial inputs | An action converges both monitors on the requested inputs | not run |
| 10 | Monitor order changes (sleep, dock re-enumeration) | No write lands on the wrong display | not run |
| 11 | Laptop sleep / wake / dock reconnect | Re-enumeration is safe, or fails clearly | not run |
| 12 | `/dev/i2c` denied on Linux | Actionable error; no root requirement | not run |

### Earlier cabling, 2026-09-16

Only one panel (`ASFPA9A001108`) was cabled to both computers, and the tool was
organised around destination computers and the PowerToys backend.

| # | Case | Status |
|---|---|---|
| 1 | Read-only discovery on Windows | passed — both panels by distinct serial, internal panel excluded |
| 2 | Read-only discovery on Ubuntu | passed — cabled panel identified, invalid displays flagged |
| 3 | Serials agree across the two hosts | passed — `ASFPA9A001108` from both, byte-identical capability strings |
| 4 | Capabilities on both hosts | passed — feature `0x60` shown and labelled as claims |
| 5, 6, 6c | Single monitor, each direction, and a full round trip | passed — issued from the Z4 and watched |
| 6b | A manual OSD change is visible to the tool | passed |

## Procedure: finding which code is which

1. Pick **one** monitor and **one** code. Prefer one where you know a computer
   with its screen **on** is plugged into that socket.
2. Make sure you can recover: the monitor's buttons, or another computer that
   can reach it.
3. Put this procedure somewhere that is not on the monitor you are testing.
4. `desktop-switcher set <monitor> 0xNN`, and **look at the monitor**. The
   tool's own output cannot tell you whether it switched; a read-back only
   repeats what the monitor stored.
5. Note what it showed, and label that input accordingly in `config.toml`.
6. Switch it back, and repeat for the next code.

The most reliable proof is a switch you watched *arrive*: send the monitor to a
different input first, then to the code under test. A monitor already on the
input you ask for gives you nothing to watch.

### What to record for each hardware run

- The exact command, its exit code and full output.
- What the screens physically did.
- Whether each computer could still reach the monitor afterwards.
- Whether switching one monitor affected the other's enumeration.
- Any recovery step needed.

The event log (`desktop-switcher doctor` prints its path) records commands and
outcomes, but cannot record what you saw.
