# Place Module Reliability Overhaul

The `mac-winops::place` module drives every grid-based move in Hotki via
`main_thread_ops`. Its multi-stage pipeline repeats across focused, id-based,
and directional placements, leaving us with divergent logic, opaque fallbacks,
and almost no automated coverage beyond simple helpers. We need to tighten
observability first, then consolidate the implementation into reusable
components that we can test in isolation, before hardening fallbacks and
integrating the improvements across the crate.

1. Stage One: Establish Observability and Guard Rails

1. [x] Add module-level documentation summarizing the placement pipeline,
       invariants, and how callers interact through `main_thread_ops`.
2. [x] Expand unit coverage for pure helpers (`grid_guess_cell_by_pos`,
       `needs_safe_park`, `skip_reason_for_role_subrole`) to lock down current
       behaviour.
3. [x] Introduce structured tracing (attempt order, settle durations,
       fallback usage) and surface counters so we can baseline failure modes
       before large refactors.

2. Stage Two: Extract a Shared Placement Engine

1. [x] Introduce a `PlacementContext` struct that captures the shared inputs
       (AX element, target rect, visible frame, attempt options).
2. [x] Move the multi-attempt pipeline into a single `PlacementEngine::execute`
       function that returns detailed outcomes for verification and logging.
3. [x] Rebuild `place_grid`, `place_grid_focused`, `place_grid_focused_opts`,
       and `place_move_grid` on top of the new engine to eliminate duplicated
       branches.
4. [x] Collapse the duplicate `PlaceAttemptOptions` definitions and re-export
       a single configuration surface for callers and tests.

3. Stage Three: Harden Fallback Behaviour

1. [x] Make settle timing, epsilon thresholds, and retry limits configurable
       via the new engine so we can tune per-call and in tests.
2. [x] Revisit safe-park and shrink→move→grow heuristics, turning the fixed
       booleans (`force_smg`, etc.) into explicit decision hooks.
3. [x] Extend error reporting to include attempt timelines, clamp diagnostics,
       and the visible-frame context we verified against.

4. Stage Four: Build a Deterministic Test Harness

1. [ ] Introduce an `AxAdapter` trait that wraps the current accessibility
       calls, with an in-memory fake to simulate window responses.
2. [ ] Add scenario tests covering success, axis nudges, fallback activation,
       and failure-to-settle paths through the new engine.
3. [ ] Layer property-style tests over grid math and clamping behaviour to
       catch regressions across different grid sizes and screen origins.
4. [ ] Extend the smoketest binary to exercise representative placement flows
       (focused, id-based, move) using the fake adapter when CI lacks GUI
       access.

5. Stage Five: Share Components and Update Integrations

1. [ ] Audit `fullscreen`, `raise`, and other window ops for opportunities to
       reuse the new adapter, normalization, and fallback utilities.
2. [ ] Expose explicit placement APIs in `main_thread_ops`/`ops.rs` that accept
       the new options surface so callers outside Hotki can opt in incrementally.
3. [ ] Update crate-level documentation and DEV.md to describe the placement
       engine, adapter pattern, and new testing strategy.
