# Smoketest Structural Cleanup

This plan trims duplication, improves reliability, and clarifies ownership between smoketest and
mac-winops. After finishing each stage, run:
- `timeout 100s cargo test --all --all-features`
- `cargo run --bin smoketest -- all`

1. Stage One: Normalize Helper Windows

Unify helper window plumbing so new flags and fixtures land in one place.

1. [x] Extract shared command assembly from `HelperWindowBuilder::spawn` and
       `spawn_inherit_io` into one helper to keep CLI flags aligned.
2. [x] Move `HelperWindowBuilder`, `ManagedChild`, and the RAII wrapper into a
       dedicated `helper_window` module, then update callers across smoketest.

2. Stage Two: Harden MRPC Driver Reliability

Surface connection failures instead of silently falling back and add coverage.

1. [x] Replace `bool`/`Option` returns in `server_drive` and `ui_interaction`
       with typed `Result` errors, updating callers to bubble failures cleanly.
2. [x] Change `TestContext::ensure_rpc_ready` to return `Result` and plumb the
       error to test call sites so gating stops early.
3. [x] Add focused coverage (mock connection or harness) for MRPC driver error
       paths and watchdog teardown to keep behavior deterministic.
4. [x] Resolve nested binding readiness so submenu chords (`shift+cmd+0`,
       `r`, `g`, focus-nav chords) are observed before injection; smoketest
       cases currently fail with `KeyNotBound`.
5. [x] Decide how to expose binding canonicalization (quoted strings vs parsed
       chords) so `wait_for_ident` and injection agree on identifiers across
       the test suite.

3. Stage Three: Consolidate mac-winops Dependent Helpers

Move generic window polling and geometry helpers where they belong.

1. [x] Move visibility polling helpers from `tests::helpers` into a new
       `mac_winops::wait` module, leaving thin smoketest wrappers if needed.
2. [x] Relocate geometry utilities (`resolve_vf_for_window`, `cell_rect`,
       `find_window_id`) into mac-winops to eliminate duplication.
3. [x] Collapse the remaining `tests::geom` and `tests::helpers` modules into a
       single smoketest `fixtures` module once shared pieces move out.

4. Stage Four: Cut Orchestrator Duplication

Trim copy-paste watchdog logic and improve diagnostics.

1. [x] Refactor `run_subtest_with_watchdog` and friends to share one
       implementation that toggles capture vs inherit behavior.
2. [x] Improve failure propagation so orchestrator surfaces child stderr/stdout
       without concatenation artifacts.
3. [x] Add regression coverage for the watchdog (stub binary exceeding timeout)
       to guard against future regressions.
4. [x] Investigate the recurring `focus-nav` smoketest timeout and land a fix.

5. Stage Five: Outcome and Config Clarity

Finish migrating to the modern reporting types and tame configuration sprawl.

1. [ ] Convert remaining consumers of `Summary`/`FocusOutcome` to `TestOutcome`
       and delete the legacy structs.
2. [ ] Group constants in `config.rs` into structured collections to cut the
       top-level clutter while keeping defaults explicit.
3. [ ] Refresh documentation comments to match the new layout, then rerun
       formatting, tests, and the smoketest all-run.
