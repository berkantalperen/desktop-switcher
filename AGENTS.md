# Working on desktop-switcher

Orientation for anyone, human or agent, about to change this code.

1. [README.md](README.md): what it does and the safety model.
2. [docs/architecture.md](docs/architecture.md): how the crates fit, what a
   switch does step by step, and decisions already tried and reversed.
3. [docs/hardware-inventory.md](docs/hardware-inventory.md): the observed
   hardware behaviour the rules are built on. Start at "Current setup".
4. [docs/test-matrix.md](docs/test-matrix.md): what is tested, and how to test
   on hardware.

## Rules that are not up for quiet revision

Each exists because its absence broke something on real hardware. Change one
only on purpose, with the reason in the commit and the docs.

- The vocabulary is **monitors and inputs**. No code, output or config key
  names a computer.
- **Nothing in the switching path is interactive**: no prompts, confirmations
  or warnings aimed at a person. It runs from hotkeys.
- **The write always goes out**; a read-back never skips it, never counts as
  confirmation, and is never recorded as evidence.
- Monitors are addressed by **EDID serial**, never by discovery order or
  display number. If a monitor cannot be identified uniquely, refuse.
- The GUI and tray **run the CLI**. Switching logic lives in `switcher-core`
  and `switcher-cli` only.
- Read-only commands stay read-only.

## Checks

On **both** Windows and Linux, since much of the code is `cfg`-gated:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

Never switch real monitors unattended or in CI. A wrong switch can leave a
screen dark until someone presses its buttons. Hardware checks need a person
watching the screens.

## Conventions

- Comments explain *why*, in plain sentences; match the surrounding density.
- Tests are named as sentences (`a_toggle_goes_to_the_other_input`), and
  failures seen on hardware get a test with the verbatim output as a fixture.
- Keep docs in step with behaviour in the same change: README for users,
  architecture for structure and reasoning, troubleshooting for failure modes,
  test-matrix for what was verified and when.
- Commits carry no AI co-author or "generated with" trailers.
