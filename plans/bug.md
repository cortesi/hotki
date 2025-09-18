# Smoketest Raise Instability

## Summary
- `cargo run --bin smoketest -- raise` still fails: the helpers appear via AX,
  but the CG frontmost check never reports the requested helper title before
  the watchdog fires.
- We instrumented the smoketest (`probe_focus_state`) and helper window to log
  CG/AX focus state and winit focus events; the logs show that CG frequently
  reports external windows (e.g., `API redesign proposal`) or the opposite
  helper immediately after our raise completes.
- `ensure_frontmost_by_title` was expanded (multi-poll loop, repeated
  re-raises, additional logging, stronger app activation) yet CG focus still
  drifts back after each attempt.
- Switching the smoketest bindings to direct global chords (`ctrl+alt+1/2`)
  removed the HUD/menu sequence from the equation, but the raise still fails in
  the same way.

## Detailed Observations
- Engine path: we now call `ensure_frontmost_by_title` directly (7 attempts,
  80 ms per poll) before falling back to an activation, but CG focus still
  flips to another window right after the helper raises.
- `ensure_frontmost_by_title` logs show repeated successes on the CG match but
  persistent AX mismatches, followed by CG focus loss within ≤40 ms. We also
  observed CG returning completely unrelated windows (e.g., Notes) even though
  the helper is frontmost momentarily.
- Smoketest probes reveal that both helpers remain AX-visible and unminimized;
  however, CG consistently reports the wrong helper/title when the assertion
  runs. Removing the HUD/`shift+cmd+0` menu did not change this behaviour.
- Helper focus logs indicate winit delivers `WindowEvent::Focused(true)` to the
  target helper immediately after `raise`, so AppKit believes the helper is
  frontmost even when CG reports otherwise.
- Filtering out empty titles from `frontmost_app_window` stopped us from
  targeting HUD overlays, but we still see external-app windows winning the
  race, implying the focus loss comes from outside the helper process.

## Hypothesis
- The underlying bug is now likely outside the helper: CG focus appears to be
  reclaimed by whichever CG window was previously frontmost (sometimes a Notes
  window) even though our helper receives AX focus and winit focus events.
- Possibilities: we need a CG-level user event (e.g., synthetic click) after
  the raise, or we must explicitly demote/hide the prior frontmost CG window
  before/after the raise.
- AX mismatch suggests `AXFocusedWindow` is not updating fast enough; we may
  need to force an `AXPerformAction` on the app element or wait for AX to settle
  before checking CG, rather than relying on CG polling alone.

## Next Steps
1. Prototype a CG-level focus nudge: synthesize a click in the helper window’s
   frame (or use `CGWarpMouseCursorPosition` + `CGEventPost`), then re-run the
   smoketest to see if CG focus stabilizes.
2. Explore explicitly hiding/miniaturizing the previously frontmost window via
   `AXPerformAction` on the old app before raising the target helper.
3. Add an AX settle wait in `ensure_frontmost_by_title` (wait for
   `AXFocusedWindow`/`AXMain` equality) and only then poll CG, to confirm the
   mismatch is timing related.
4. Reconfirm with a reduced repro outside the smoketest (simple script invoking
   `ensure_frontmost_by_title`) to see if the behaviour is reproducible without
   the helper harness.

## Status
- Codebase now includes: expanded `ensure_frontmost_by_title`, additional
  logging, helper focus probes, and the smoketest binding rewrite. Despite
  these, `cargo run --bin smoketest -- raise` still fails due to CG focus never
  stabilizing on the requested helper.

## New Findings (September 17, 2025)
- Added a synthetic click nudge inside `ensure_frontmost_by_title`; winit focus
  reports remain stable but CG continues to jump to unrelated processes (e.g.
  Brave tabs, temporary helper windows, even stray `tmp` titles) immediately
  after each raise.
- Instrumented `raise_window` with a direct `CGSOrderWindow` call. The call
  consistently fails with error code `1000`, indicating the SkyLight request is
  being rejected even on the TAO main thread; no CoreGraphics-level reorder
  occurs.
- The smoketest now logs repeated "No IPC activity within heartbeat timeout"
  warnings because the watchdog expires while the focus loops keep re-raising
  and re-activating the same PID without ever convincing CG to hold the helper
  frontmost.
- Post-stabilization nudges fire, but the subsequent CG poll still reports the
  wrong front window within ≤120 ms. AX visibility and frames remain correct,
  so the regression remains isolated to CG focus arbitration.
- Removing the redundant `request_activate_pid` calls from the recovery path
  reduced some churn but did not change the outcome; CG immediately replaces
  the helper with whichever window previously held focus.

## Owner Notes
- Any future modifications to `mac-winops::raise::raise_window` must keep the
  main-thread constraints in mind; direct CGS calls outside AppKit continue to
  fail with code `1000`.
- Re-running `cargo fmt` after each change ensures formatting stays consistent.

# Focus-Tracking Smoketest Timeout

## Summary
- Root cause: when the engine’s world subscription lags the tokio broadcast stream, `RecvError::Lagged` was treated as fatal; the task exited and the engine stopped receiving `FocusChanged`, leaving the HUD bound to the prior window until the smoketest watchdog fired.
- Added a resubscribe loop plus logging in `spawn_world_focus_subscription`. The engine now reapplies the world focus context after every lag, confirming the helper’s context and keeping the HUD in sync.
- The new warn log (`World focus subscription lagged; resubscribing`) quantifies backlog pressure (typically 55–65 skipped events during initial world sweeps) but recovery is automatic.
- Standalone `cargo run --bin smoketest -- focus-tracking` now passes consistently (<1.4 s); aggregated `smoketest all` still encounters unrelated layout flakes (`focus-nav`, `place-*`) yet the focus-tracking case succeeds when rerun in isolation immediately afterwards.

## Root Cause Analysis
- Broadcast lag is common during the first world snapshot: the actor emits hundreds of `Added/Updated` events. The engine’s subscriber was reading sequentially and, upon lagging, hit `RecvError::Lagged`.
- The previous `Err(_) => break` handling dropped the subscription task entirely, so subsequent helper focus events never reached `apply_world_focus_context`.
- Trace logging incidentally slowed the producer enough to avoid lag, explaining why enabling extra logs “fixed” the issue before.

## Instrumentation
- Added warn-level telemetry with the number of skipped events when resubscribing; this confirmed the hypothesis during failing runs (e.g., skipped≈58 before the watchdog in pre-fix logs).
- Verified via debug logging that `Engine: world focus context updated` now fires for the helper immediately after each resubscribe, even under heavy event load.

## Resolution
- `spawn_world_focus_subscription` now loops indefinitely: on `Lagged` it re-enters `world.subscribe_with_context`, reapplies the seeded context, and continues listening; on `Closed` it logs and exits gracefully.
- No changes required to world buffering yet, but the instrumentation keeps this hotspot visible for future tuning.

## Status (2025-09-18)
- Focus-tracking smoketest: 3/3 standalone runs succeed post-fix, each completing in ≈1.3 s with occasional lag warnings but no hangs.
- `smoketest all` currently fails in other suites (`focus-nav`, placement helpers) because the live desktop layout does not match scripted expectations; focus-tracking no longer blocks the run and can be re-executed successfully immediately after.
- Repeated `Failed to send event to channel: channel closed` errors traced to the MRPC client dropping its receiver during shutdown; logging now downgrades these to debug-level "Dropping notify" messages once the receiver is closed.
