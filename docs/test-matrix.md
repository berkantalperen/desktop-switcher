# Test matrix

Three tiers. The first two run unattended; the third must not.

## Tier 1 — automated, no hardware

`cargo test --workspace`. 127 tests at the time of writing.

They cover, among other things:

| Property | Where |
|---|---|
| A bare decimal input code is rejected as ambiguous | `switcher-core::types` |
| `0x60` is read from the `vcp(...)` section only, never from elsewhere in a capability string | `switcher-core::mccs` |
| Malformed, truncated and oversized capability strings fail loudly rather than parsing to an empty list | `switcher-core::mccs` |
| A panel with no usable EDID serial yields `None`, not a fabricated identifier | `switcher-core::edid` |
| A hung backend process is killed and reported as a timeout | `switcher-core::proc` |
| Arguments are passed as an array, so a serial containing a `;` cannot become a second command | `switcher-core::proc` |
| Reordered discovery does not change which monitor a logical id binds to | `switcher-core::inventory` |
| Duplicate serials, missing monitors and moved cables all block the switch instead of resolving to a guess | `switcher-core::inventory` |
| A missing panel can be restated as present-but-silent, and nothing else can | `switcher-core::inventory` |
| The internal laptop panel is never a switch target | `switcher-core::inventory` |
| Each monitor is written its own code; nothing is batched | `switcher-core::switch` |
| A read that already matches never suppresses the write | `switcher-core::switch` |
| A stale read cannot block a recovery | `switcher-core::switch` |
| Losing DDC contact after switching away is not reported as a failure | `switcher-core::switch` |
| A write that is accepted but ignored is not reported as success | `switcher-core::switch` |
| One monitor failing yields a partial report, with no rollback attempted | `switcher-core::switch` |
| `toggle` refuses on mixed inputs, unreadable monitors, or a manual OSD change to an unmapped input | `switcher-core::switch` |
| A stale lock file does not wedge the tool | `switcher-core::switch` |
| The last-request record survives a round trip to disk | `switcher-core::switch` |
| `doctor`, `monitors` and `inspect` never create or write anything | `switcher-cli` integration tests |
| Interactive commands refuse to run when stdin is not a terminal | `switcher-cli` integration tests |

## Tier 2 — parser fixtures

Real captured output, byte for byte, in `tests/fixtures/`.

- `powertoys/` — **real**, captured from PowerToys 0.101.2362.0 on 2026-09-16,
  with stdout, stderr and exit status recorded separately.
- `ddcutil/` — **synthetic**, pending capture from the HP Z4. See the README in
  that directory for how to replace them.

A test also cross-checks the two independent readings of the same capability
claim — the CLI's parsed table and the raw MCCS string — against each other.

## Tier 3 — hardware, human present

**Never run these unattended, and never in CI.** Each one can leave a screen
blank until someone presses buttons on the monitor.

Before starting, confirm recovery works: the laptop's built-in panel is
available, and you can reach each monitor's on-screen menu by hand.

| # | Case | Pass condition | Status |
|---|---|---|---|
| 1 | Read-only discovery on Windows | Both AOCs identified by distinct serial, internal panel excluded | **passed 2026-09-16** |
| 2 | Read-only discovery on Ubuntu | The cabled AOC identified, invalid displays flagged | **passed 2026-09-16** |
| 3 | Serials agree across the two hosts | The same serial identifies the same panel on both | **passed 2026-09-16** — `ASFPA9A001108` from both, and byte-identical capability strings |
| 4 | Capabilities on both hosts | Feature `0x60` and its values shown, labelled as claims | **passed 2026-09-16** on both |
| 4b | DDC reachable from the non-displaying computer | The idle computer can still read `0x60` | **passed 2026-09-16** — the Z4 reads `0x11` while Windows drives the panel |
| 5 | Single monitor, → Ubuntu | The nominated panel shows Ubuntu; recovery works | **passed 2026-09-16** — `test-input --code 0x0F` issued from the Z4, panel switched, recovered with the monitor buttons |
| 6 | Single monitor, → Windows | Mirror of case 5, issued by the tool | **passed 2026-09-16** — `switch windows` from the Z4, confirmed by read |
| 6c | Full round trip via `switch` | Both directions issued by the tool and verified | **passed 2026-09-16** — `switch ubuntu` then `switch windows`, both `confirmed by read` |
| 6b | Readback tracks a manual OSD change | A change made with the monitor buttons is visible to the tool | **passed 2026-09-16** — five consecutive reads returned `0x11` after a manual switch back |
| 7 | Both monitors, each direction | Both switch; per-monitor result and timing recorded | not run |
| 8 | Mixed initial inputs | An explicit target converges both panels on the requested computer | not run |
| 9 | Repeated same request | The same absolute code is written again; no cycling, no new input | **passed 2026-09-16** on hardware as a no-write skip; the skip was **removed 2026-09-17** after a panel physically on HDMI reported DisplayPort and the skip blocked the way back. Needs a re-run: expect two writes of the same code and no visible change. |
| 10 | Monitor order changes (sleep, dock re-enumeration) | No write lands on the wrong display | not run |
| 11 | Sleep / wake / dock reconnect | Re-enumeration is safe, or fails clearly | not run |
| 12 | PowerToys closed, or `/dev/i2c` denied | Actionable error; no root requirement, no fallback to monitor 1 | not run |
| 13 | Input read lost after switching away | Reported as `issued, visual state unconfirmed`, never as verified | covered by tier 1, not yet on hardware |
| 14 | Manual OSD change, then `toggle` | Refuses and explains, rather than guessing | covered by tier 1, not yet on hardware |
| 15 | One computer powered off | Switching to it is still allowed but reported honestly; recovery documented | not run |

### Procedure for cases 5 and 6

This is how an input code earns `user-confirmed`, and it is the only way.

1. Pick **one** monitor and **one** candidate code. Start with a code the
   monitor advertises, and one where you have reason to think a live computer is
   attached to that port.
2. Make sure you can recover: the other computer is reachable, or you are
   willing to use the monitor's buttons.
3. Run, from the computer that currently owns the monitor:

   ```
   desktop-switcher test-input --monitor left --code 0x0F --destination ubuntu
   ```

4. The tool prints what it is about to do, the recovery instructions, and waits
   for an explicit confirmation. It writes once. It does not retry.
5. **Look at the monitor.** The tool then asks what you actually saw. Only the
   answer "the other computer" records the mapping.
6. Restore the monitor, either from the other computer or with the monitor's own
   buttons.
7. Repeat for the other monitor, then for the other direction.

A read-back that fails at step 4 is expected when the switch worked: the monitor
has stopped talking to the computer that just sent it away. The tool says so
rather than calling it a failure — and equally, it does not call it a success.

### What must be recorded for each hardware run

- The exact command, its exit code, and its full output.
- What the screens physically did.
- Whether the previously-owning computer could still reach the monitor
  afterwards.
- Whether switching one monitor affected the other's enumeration.
- Any recovery step needed.

The tool writes its own event log to the per-user data directory
(`desktop-switcher doctor` prints the path); that log records commands and
outcomes but cannot record what you saw.
