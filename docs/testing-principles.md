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
  queues emptied. Emit structured logs whenever residual events remain.
- Keep helper names unique per case via `config::test_title(..)`; this avoids cross-case focus
  collisions.

## Budgets
- Every case reports `Budget { setup, action, settle }` in logs. Honour those buckets in new helpers
  so slow paths can be tuned without inflating global timeouts.
- Failing assertions should include the elapsed budget segment to spotlight timeouts vs. mismatches.
- Prefer fine-grained budgets (per placement step) over long global sleeps; this yields better
  diagnostics and tighter CI predictability.

## Case Naming
- Registry entries follow a dotted hierarchy: `<domain>.<scenario>.<variant>` (for example
  `place.minimized.defer`). Keep segments short and deterministic so log keys and CLI invocations
  stay discoverable.
- Reuse the registry slug when spawning mimic windows. Helper titles adopt the
  `[{slug}::{window_label}]` convention so diagnostics line up without additional parsing.
- If a case owns multiple helpers, add a fourth segment (for example
  `focus.swap.sibling.primary`) or annotate individual failure lines via the `case=<>` field.
- Document every helper used by a case inside the registry metadata. The registry enforces the
  ≤12 helper contract and feeds contributor docs automatically.

## Skip Semantics
- Gate environment-sensitive cases with the shared `assume!` macro. Record skips as
  `SKIP: <case> -- <reason>` and exit early with `Ok(())` so stats remain accurate.
- Keep skip probes centralized. Use canonical helpers such as `world::list_windows`,
  `world::ensure_frontmost`, or `smoketest::server_drive::BridgeDriver::wait_for_idents` instead of
  ad-hoc environment checks.
- Never sprinkle test-specific environment variables. If a scenario needs configuration, extend the
  smoketest CLI surface.

## Canonical Environment Probes
- `smoketest::world::list_windows()` – authoritative snapshot for display/window counts.
- `smoketest::world::ensure_frontmost(..)` – deterministic focus handoff without reaching into AX.
- `smoketest::server_drive::BridgeDriver::wait_for_world_seq(..)` – confirm backend focus updates over
  MRPC.
- `WorldView::displays()` / `WorldHandle::displays_snapshot()` – translate helper frames into display
  coordinates and `global_top` when budgeting placements.
- If you need a new probe (e.g., multiple-display detection, screen scale, mission-control state),
  add it to `hotki-world` or the smoketest helper modules so all cases stay on the same contract.

## Authoritative Frames
- `hotki-world` reconciles CoreGraphics and Accessibility rectangles per window. Use
  `WorldView::frames_snapshot`/`frames` to inspect the resolved [`WindowMode`], backing scale, and
  authoritative rectangle.
- Normal and hidden windows prefer CoreGraphics geometry and fall back to Accessibility only when
  CG bounds are missing. Fullscreen and tiled windows also prefer CoreGraphics to avoid split-view
  disagreements.
- Minimized windows reuse the last visible rectangle observed before the minimize event. The cache
  is surfaced as `authoritative_kind = Cached` so tests can assert the provenance explicitly.
- `WorldView::display_scale` and `WorldView::authoritative_eps` expose the reconciled scale and the
  default pixel epsilon. Tests should query these helpers instead of hard-coding `1` or `2`.
- Raw AX/CG rectangles are available for diagnostics, but production code should continue to rely
  on the authoritative rectangle exclusively.

## Message Style
Emit failures as a single structured line so CI logs stay machine-parseable:

```
case=<name> scale=<n> eps=<px> expected=<x,y,w,h> got=<x,y,w,h> delta=<dx,dy,dw,dh>
```

- `case` should identify the scenario plus any sub-key (for example `place[col=1,row=2]`).
- `scale` is the display backing scale (normally `1` or `2`).
- `eps` is the comparison tolerance in pixels.
- `delta` is the signed difference (`actual - expected`).
- Add context via `event`-scoped log fields instead of writing side-channel files.
- Prefer helper functions (see `helpers::assert_frame_matches`) over open-coded formats so new cases
  inherit the template automatically.

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
