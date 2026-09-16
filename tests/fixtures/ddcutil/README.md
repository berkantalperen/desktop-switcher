# ddcutil fixtures

Verbatim capture from **ddcutil 2.2.5** on the HP Z4 (`berkant-HP-Z4-G4`,
Ubuntu 26.04.1, NVIDIA Quadro M2000, proprietary driver 580.178.04), taken
2026-09-16. Every file is real tool output, with stdout, stderr and exit status
recorded separately.

Nothing here was produced by a write. Every command is a read.

| Fixture | Command |
|---|---|
| `version` | `ddcutil --version` |
| `detect` | `ddcutil detect` |
| `capabilities` | `ddcutil --sn ASFPA9A001108 capabilities` |
| `capabilities-terse` | `ddcutil --sn ASFPA9A001108 capabilities --terse` |
| `getvcp60` | `ddcutil --sn ASFPA9A001108 getvcp 60` |
| `getvcp60-terse` | `ddcutil --sn ASFPA9A001108 getvcp 60 --terse` |
| `error-display-not-found` | `ddcutil --sn NOPE-NOT-REAL getvcp 60 --terse` |
| `error-unsupported-feature` | `ddcutil --sn ASFPA9A001108 getvcp AA --terse` |

## What these captures settled

Three things the parser was guessing at before, each now pinned by a test:

- **2.2.5 prints `DRM_connector:` with an underscore**, where the documentation
  uses a space. The parser accepts both.
- **`capabilities --terse` prefixes the MCCS string** with
  `Unparsed capabilities string: `, so the line cannot be handed straight to
  the MCCS parser.
- **An unsupported feature exits non-zero with `VCP <code> ERR` on stdout.**
  That is the monitor answering, not the tool failing, and the two are now
  told apart.

Also confirmed, and more important than any of the above: `detect` reports
serial `ASFPA9A001108` — **the same serial Windows reports for that panel** —
and the capability string is byte-identical to the one PowerToys returns. That
is what makes a monitor matchable across the two computers.

Only one display appears, because only one AOC is currently cabled to the Z4.
See `docs/hardware-inventory.md`.

## Re-capturing

Run `scripts/ubuntu-preflight.sh` on the Z4, or the individual commands above.
Keep stdout and stderr separate, and keep the exit status: several tests depend
on the distinction. If a future ddcutil release changes the output, the parser
tests fail rather than the tool silently misreading a monitor.
