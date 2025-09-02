# Developer Notes

## Smoketest

Smoketest provides a small, macOS‑only end‑to‑end check for relay repeat
behaviour.

- Purpose: Exercise the key relay + repeat ticker without running the full app.
- Command: `cargo run --bin smoketest -- repeat-relay --time 2000`
  - Output example: `12 repeats`
- Global flag: `--logs` enables Tracing logs (respects `RUST_LOG`), e.g.:
  - `RUST_LOG=hotkey_manager=trace cargo run --bin smoketest -- --logs repeat-relay --time 2000`
- What it does: Opens a tiny window, activates the app, dummies a keydown for
  the Right Arrow, runs the software repeat ticker for the given duration, then
  stops and prints the number of repeat ticks observed.
- Notes:
  - System key repeat settings influence the count (initial delay + interval).
  - Requires macOS Accessibility/Input Monitoring permissions to post events.
  - The default key is Right Arrow to avoid typing into terminals; the test
    brings itself to the foreground.

Additional command:

- `repeat-shell`: Repeats a shell command and counts actual invocations.
  - Example: `cargo run --bin smoketest -- repeat-shell --time 2000`
  - Implementation: The test command appends to a unique temp file on each
    invocation; the tool reads the file to count invocations and reports
    repeats (total minus the initial run).

- `repeat-volume`: Sets volume to 0, repeats a +1 volume change, and measures
  the resulting volume.
  - Example: `cargo run --bin smoketest -- repeat-volume --time 2000`
  - The final volume minus one (initial run) is reported as repeats.

Test runner:

- `all`: Runs repeat tests (1s each; volume 2s, expect ≥3 repeats) and UI demos (ui + miniui) that verify the HUD appears and a short theme cycle works.
  - Example: `cargo run --bin smoketest -- all`
  - Prints per-test counts, runs UI checks, and exits non‑zero on failure.
