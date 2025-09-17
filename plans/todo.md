# Space-Scoped Window Tracking

Hotki currently enumerates every window across all Mission Control spaces each sweep, slowing
smoketests and inflating world reconciliation costs. We want the world tracker to focus on the
active space, adopt windows when we land on a new space, and refuse operations on windows that
aren't on our current space.

1. Stage One: Capture Baseline And Requirements
1. [x] Instrument the smoketest (and optionally the new `space_probe`) to capture sweep timings,
   window counts, and active-space sets so we can quantify the slowdown.
2. [x] Review collected traces to confirm the slowdown is tied to off-space enumeration and agree
   on target timings for "fast" smoketests.

2. Stage Two: mac-winops Targeted Enumeration
3. [x] Extend `mac_winops::ops::WinOps` with an API that filters to the active space set by default
   and exposes a `list_windows_for_spaces` helper for on-demand adoption.
4. [x] Update `RealWinOps` / `MockWinOps` implementations plus unit tests so the new API surfaces
   work without pulling in off-space windows unless explicitly requested.

3. Stage Three: World Space-Adoption Lifecycle
5. [ ] Teach `WorldState` to track the currently active space ids and retain per-space caches so the
   live snapshot only includes windows for the foreground space(s).
6. [ ] When the active space set changes, enumerate just the newly activated spaces, adopt their
   windows into the world, and drop windows for spaces we leave.
7. [ ] On re-entering a space, diff the cached snapshot against a fresh enumeration to reap windows
   that closed while we were away without sweeping every other space.

4. Stage Four: Enforce Space Guardrails Downstream
8. [x] Guard engine/server window manipulation APIs so we refuse place/move/raise requests when
   `on_active_space` is false, returning structured errors and telemetry.
9. [x] Update protocol/client runtime handling to keep requests scoped to active-space windows and
   cover the new guardrails with tests.

5. Stage Five: Validation And Perf Regression Tests
10. [x] Expand smoketests to simulate multi-space navigation, asserting adoption/reaping behavior
    and ensuring runtime stays under the agreed performance budget.
11. [x] Refresh world/unit/integration tests to cover the new lifecycle and guardrail logic end to
    end.

6. Stage Six: Release Hygiene
12. [ ] Run `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests
    --examples 2>&1` and resolve warnings.
13. [ ] Execute `cargo test --all` and `cargo run --bin smoketest -- all` with extended timeouts.
14. [ ] Document the space-scoped tracking strategy and performance expectations in
    `plans/world.md` or `DEV.md`.
