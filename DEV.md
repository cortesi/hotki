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

## Concurrency and Locking

Hotki runs an async runtime (Tokio) and several high‑frequency, synchronous hot paths. Use the following rules to choose locking primitives consistently across the workspace:

- Use `parking_lot::Mutex`/`parking_lot::RwLock` for synchronous, short critical sections on hot paths. These are faster and non‑poisoning. Never hold them across an `.await`.
- Use `tokio::sync::{Mutex,RwLock}` for state that may be mutated while awaiting or shared across async tasks. Keep hold times short; move expensive work out of the critical section.
- Avoid mixing lock types in public APIs. If an API is async‑facing, prefer `tokio::sync` types at the boundary and keep `parking_lot` internal. For purely sync modules, keep `parking_lot` throughout.

Recommended patterns

- Sync hot path (e.g., key tracking, small maps):
  ```rust
  use std::collections::{HashMap, HashSet};
  use std::sync::Arc;
  use parking_lot::Mutex;

  struct KeyState {
      held: Arc<Mutex<HashSet<String>>>,
      repeat_ok: Arc<Mutex<HashSet<String>>>,
  }
  ```

- Async state owned by an engine or service:
  ```rust
  use std::sync::Arc;
  use tokio::sync::{Mutex, RwLock};

  struct Engine {
      state: Arc<Mutex<State>>,        // mutated from async handlers
      config: Arc<RwLock<Config>>,     // read‑mostly, async readers
  }
  ```

Migration notes

- Adding the dependency: `cargo add parking_lot`
- Converting from `std::sync::Mutex<T>` → `parking_lot::Mutex<T>`:
  - Update the `use` to `parking_lot::Mutex`.
  - Replace `lock().unwrap()` with `lock()` (no poisoning in `parking_lot`).
  - Ensure no `.await` occurs while the guard is held; if needed, clone/move data out first.

Validation

- For UI‑path changes, run: `cargo run --bin smoketest -- all`.
- Always run: `cargo clippy -q --fix --all-targets --all-features --allow-dirty --tests --examples` and `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml`.
