# ddcutil fixtures

**These files are SYNTHETIC.** They are shaped from ddcutil's documented
output, not captured from the HP Z4. They exist so the parser has something to
test against before hardware access exists.

They are not evidence about the AOC monitors, and nothing in them should be
copied into a configuration as an input code.

Replace them with real capture as soon as `scripts/ubuntu-preflight.sh` has run
on the Z4:

1. Run the preflight script on the Ubuntu host.
2. Split its output into `detect.txt`, `capabilities-<display>.txt` and
   `getvcp60-<display>.txt`.
3. Point the tests in `crates/switcher-backend-ddcutil/src/parse.rs` at the
   real files and delete the `.synthetic.` ones.
4. Re-run `cargo test`. Any failure is the parser learning what ddcutil
   actually prints on this machine.
