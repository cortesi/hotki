# Hotki Smoketest Testing Principles

Hotki smoketests exercise deterministic, world-driven scenarios against a controlled helper window.
Every probe flows through `hotki-world` APIs or explicit smoketest helpers so that CoreGraphics and AX
usage stays centralized. This document captures the principles that keep runs reproducible, reliable,
and debuggable.

## World-Only Flows
- Treat `hotki-world` as the single source of truth. Reach for `smoketest::world` helpers before
  touching platform APIs directly.
- If a probe is missing, extend the world surface (or its test utilities) instead of calling AX/CG in
  a smoketest. This keeps the data channel unified and preserves determinism.
- When helper processes need hints, exchange them via the world or smoketest RPC rather than side
  channels.

## Runloop Pumping
- Mutating operations must execute on the macOS main thread. Use
  `WorldHandle::pump_main_until(..)` or `mac_winops::drain_main_ops()` immediately after commands to
  settle AppKit state.
- Avoid manual sleeps. Prefer event-driven loops (`pump_main_until`, `recv_event_until`) that observe
  completion signals.
- When waiting on asynchronous deltas, always pair a deadline budget with a runloop pump to avoid
  starving the dispatcher.

## Reset Contract
- Each case owns a fresh world. Call `world.reset()` (or respawn helpers) before reusing handles so
  state from a prior case cannot leak.
- Teardown must assert quiescence: helper processes are killed, subscriptions drained, and world
  queues emptied. Capture artifacts whenever residual events remain.
- Keep helper names unique per case via `config::test_title(..)`; this avoids cross-case focus
  collisions.

## Budgets
- Every case reports `Budget { setup, action, settle }` in logs. Honour those buckets in new helpers
  so slow paths can be tuned without inflating global timeouts.
- Failing assertions should include the elapsed budget segment to spotlight timeouts vs. mismatches.
- Prefer fine-grained budgets (per placement step) over long global sleeps; this yields better
  diagnostics and tighter CI predictability.

## Skip Semantics
- Gate environment-sensitive cases with the shared `assume!` macro. Record skips as
  `SKIP: <case> -- <reason>` and exit early with `Ok(())` so stats remain accurate.
- Keep skip probes centralized. Use canonical helpers such as `world::list_windows`,
  `world::ensure_frontmost`, or `server_drive::wait_for_ident` instead of ad-hoc environment checks.
- Never sprinkle test-specific environment variables. If a scenario needs configuration, extend the
  smoketest CLI surface.

## Canonical Environment Probes
- `smoketest::world::list_windows()` – authoritative snapshot for display/window counts.
- `smoketest::world::ensure_frontmost(..)` – deterministic focus handoff without reaching into AX.
- `smoketest::server_drive::wait_for_focused_title(..)` – confirm backend focus updates over MRPC.
- `mac_winops::screen::visible_frame_containing_point(..)` – translate helper frames into display
  coordinates when budgeting placements.
- If you need a new probe (e.g., multiple-display detection, screen scale, mission-control state),
  add it to `hotki-world` or the smoketest helper modules so all cases stay on the same contract.

## Message Style
Emit failures as a single structured line so CI logs stay machine-parseable:

```
case=<name> scale=<n> eps=<px> expected=<x,y,w,h> got=<x,y,w,h> delta=<dx,dy,dw,dh> artifacts=<paths>
```

- `case` should identify the scenario plus any sub-key (for example `place[col=1,row=2]`).
- `scale` is the display backing scale (normally `1` or `2`).
- `eps` is the comparison tolerance in pixels.
- `delta` is the signed difference (`actual - expected`).
- `artifacts` lists comma-separated relative paths or `[]` when none exist.
- Prefer helper functions (see `tests::fixtures::frame_failure_line`) over open-coded formats so new
  cases inherit the template automatically.

## Do Not Do
- **No direct AX/CG in tests.** Extend `hotki-world` or smoketest helpers; do not import platform
  modules inside case code.
- **No manual sleeping or polling loops.** Use runloop pumps and bounded event waits instead of
  `std::thread::sleep` or busy loops.
- **No third-party apps in CI.** Mimic captures and helper windows are the only sanctioned surfaces.
- **Mutating ops stay on the main thread.** Use world commands and helper shims that marshal through
  the main AppKit thread; never spawn ad-hoc background runloops for window mutation.
- **No async test attributes in `hotki-world/tests`.** Use `run_async_test` plus explicit runtime
  helpers—the lint guard will fail the build if `#[tokio::test]` (or similar) slips in.

