# Hotki Smoketests 2.0

Deterministic, event-driven, world-centric smoketests. Tests only consume hotki-world.
No second data channel; hotki-world is the single assertion surface.

> **Targets**
>
> * **Reliability:** flake <=0.5% over 20 runs; <=1% in 100-run stress for critical cases.
> * **Clarity/LOC:** replace ad-hoc loops with shared primitives; cut touched test LOC by 20-35%.
> * **Single Source:** tests never call AX/CG directly; hotki-world must expose every probe we need.
> * **Coverage:** model high-value placement/focus cases via Mimic Window Harness; keep CI friendly.
> * **Budgets:** every case logs `Budget { setup, action, settle }` so slow paths are tunable.

> **Design Principles**
>
> * One world: every read/write flows through `hotki-world`.
> * Event-first settling: prefer event->confirm loops over polling.
> * Deterministic boundaries: fresh world per case; assert quiescence on teardown.
> * Mutating ops stay on the main thread; waits pump AppKit via `world.pump_main_until`.
> * Assertions run on scaled integer pixels with helper-derived epsilons.
> * Artifacts capture recent events and environment so regressions self-diagnose.
> * Minimal public test surface: only vetted helpers exported from smoketest runner.
> * Reproducible realism: mimic windows come from `hotki-tester` captures; no third-party apps.

> **Strengths Locked In**
>
> * Single channel: tests assert via hotki-world; raw AX/CG stay behind feature gates.
> * Main-thread serialization with explicit `world.pump_main_until` keeps flakes down.
> * Event ring buffers with `lost_count` plus artifact tails guard against silent overflow.
> * Scaled-pixel assertions with default eps clean up Retina rounding issues.
> * Per-case budgets land in logs so slow paths can be tuned without global timeout creep.
> * Importer heuristics and preview bridge `hotki-tester` captures into deterministic mimic runs.
> * The registry runner replaces the orchestrator, collapsing subprocess plumbing safely.

1. Stage One: Codify Testing Principles
1. [x] Write `docs/testing-principles.md` covering world-only flows, runloop pumping, reset contract,
       budgets, skip semantics, and canonical environment probes.
2. [x] Add a "Do Not Do" page to `testing-principles.md`: no direct AX/CG in tests, no sleeping or
       manual polling, no third-party apps in CI, mutating ops serialized on the main thread.
3. [x] Write `docs/mimic-scenarios.md` to describe mimic structure, quirks, and capture lifecycle,
       and cross-link importer heuristics guidance.
4. [x] Link both docs from `README.md`, `DEV.md`, and `crates/smoketest/README.md` contributor
       sections.
5. [x] Document a "Message Style" subsection with the one-line failure template and ensure helpers
       emit `case=<name> scale=<n> eps=<px> expected=<x,y,w,h> got=<x,y,w,h>` and
       `delta=<dx,dy,dw,dh> artifacts=<paths...>`.
6. [x] Ban async test attributes in world tests: document the rule and add lint coverage for
       `#[tokio::test]` (and friends) in `crates/hotki-world/tests`.
*Acceptance:* Docs spell out the main-thread guarantee, skip policy, and message template, and CI lints
             for banned imports plus async test attributes in `crates/hotki-world/tests`.

2. Stage Two: Extend World Frame Introspection
1. [x] Add `test-introspection` feature gating raw AX/CG fields while keeping the authoritative rect
       always available.
2. [x] Introduce `RectPx`, `FrameKind`, and `Frames` (with display/space/scale/mode) in
       `crates/hotki-world/src`, re-export via `lib.rs`, and log AX↔CG deltas in diagnostics.
3. [x] Embed the "Authoritative frame rules" policy block in docs and module comments so behavior is
       explicit when AX and CG disagree or windows change modes.
4. [x] Implement `WindowMode { Normal | Minimized | Hidden | Fullscreen | Tiled }` on `Frames` and
       surface `WorldHandle::authoritative_eps(display_id) -> i32` for helpers and diagnostics.
5. [x] Provide `WorldHandle::pump_main_until` plus a sync `TestHarness` helper that advances the
       runloop until a deadline instead of relying on async executors.
6. [x] Add `frames`, `display_scale`, and `authoritative_eps` APIs on `WorldHandle`/`WorldView` and
       ensure tests only use those accessors.
7. [x] Replace async frame stability tests with synchronous harness-based ones that exercise
       minimized, hidden, and fullscreen/tiled windows, asserting `place()` errors when modes disallow
       placement without `PlaceOptions`.
*Acceptance:* Unit tests cover minimized/hidden/fullscreen modes, `place()` errors in forbidden modes,
             and `authoritative_eps` defaults match the documented policy while runloop pumping avoids
             starvation.

3. Stage Three: Event Loop, Reset, and Artifacts
1. [x] Replace broadcast receivers with a 256-capacity ring buffer per subscription that drops the
       oldest entry on overflow and increments a `lost_count` counter.
2. [x] Extend `WorldHandle`/`WorldView` for filtered subscribe and `next_event_until`, returning an
       `EventCursor` that carries a monotonic index and `lost_count`.
3. [x] Implement `reset`, `is_quiescent`, and `quiescence_report` returning counts for
       `active_ax_observers`, `pending_main_ops`, `mimic_windows`, and `subscriptions`, forcibly
       closing mimic windows and unsubscribing all streams before draining main ops.
4. [x] Add `capture_failure_artifacts` emitting `.world.txt`, `.frames.txt`, cropped PNGs using the
       authoritative rect (with expected vs actual overlays and scale), the last ~50 events with
       timestamps/window ids, `display_id`, `space_id`, scale, and `world_commit`.
5. [x] Add unit tests covering overflow (`lost_count` surfaces), leaking subscriptions, mimic cleanup
       on reset, and verifying artifact crops/overlays appear.
6. [x] Propagate `lost_count` to helper wait APIs so they fail loudly with the standardized message
       when events drop during waits.
*Acceptance:* Flooding the ring buffer increments `lost_count` and makes helper waits fail with the
             template message, and reset reports mimic leaks while artifacts include cropped overlays.

4. Stage Four: Mimic Window Harness in hotki-world
1. [x] Move winit helper logic from `crates/smoketest/src/winhelper.rs` into a `mimic` module gated
       behind a `world-mimic` feature that stays disabled in release builds.
2. [x] Define `MimicSpec`, `MimicScenario`, `Quirk`, and `spawn_mimic`/`kill_mimic` APIs, tagging each
       window with `{ scenario_slug, window_label, quirks[] }` for artifacts and enforcing naming
       conventions.
3. [x] Document quirk semantics precisely: `AxRounding` perturbs raw AX only, `DelayApplyMove` delays
       authoritative updates, `IgnoreMoveIfMinimized` defers placement, and `RaiseCyclesToSibling`
       respects `KeepFrontWindow`.
4. [x] Introduce `RaiseStrategy`, `MinimizedPolicy`, and `PlaceOptions` in `hotki-world`, require tests
       to pick a strategy, and ensure mimics honor `RaiseStrategy::KeepFrontWindow`.
5. [x] Implement quirks as specified and emit diagnostics that include
       `scenario_slug/window_label/quirks[]`.
6. [x] Add integration tests validating each individual quirk plus a composite raise-cycle case that
       exercises `RaiseStrategy::KeepFrontWindow`.
7. [x] Update build scripts and CI presets so `world-mimic` is on for dev/tests, off for release, and
       document the required profiles.
*Acceptance:* Quirk tests (including the composite raise-cycle) pass with artifacts showing the naming
             scheme and `RaiseStrategy` behavior, and feature gates align with CI profiles.

5. Stage Five: Rebuild Smoketest Runner and Helpers
1. [x] Delete `orchestrator.rs`; replace it with a registry-driven `run_case` loop that owns
       `{ name, info, main_thread, extra_timeout_ms, budget }` entries.
2. [x] Introduce a `Budget` struct and log both configured budgets and actual
       `{setup_ms, action_ms, settle_ms}` per run into artifacts.
3. [x] Implement `wait_for_events_or` that pumps the main thread via `world.pump_main_until`, uses
       event cursors for ordering, and fails with the standardized message when `lost_count` increases.
4. [x] Rebuild `assert_frame_matches` to operate on scaled integer pixels, prefer authoritative frames,
       include raw AX/CG deltas when available, and emit single-line diffs matching the message
       template.
5. [x] Port high-value smoketests to the new runner, removing direct `mac_winops` usage, relying on
       mimic scenarios, and naming cases like `place.minimized.defer`.
6. [x] Keep the exported helper API ≤12 functions, document each in registry metadata, and add the case
       naming and failure message style guide to contributor docs.
7. [x] After each `run_case`, call `world.reset()` and fail if `!world.is_quiescent()`, including
       `quiescence_report()` and artifact paths in the error.
8. [x] Update the mimic runtime pump to honor helper wakeups instead of polling with a zero timeout.
9. [x] Ensure `process_apply_ready` preserves `apply_after` until the window is available.
10. [x] Reset or isolate the mimic event loop between helpers to avoid reusing an exited loop state.
11. [x] Avoid calling `elwt.exit()` during normal helper shutdown so the shared loop stays reusable.
12. [ ] Explicitly close helper NSWindows on shutdown and wait for AppKit confirmation before teardown.
*Acceptance:* Runner artifacts include configured vs actual budgets, waits fail loudly on `lost_count`
             changes, quiescence failures surface diagnostic counts, and refactored cases run solely
             through the new helpers.

1. Unported Smoketests
1. [ ] repeat-relay
2. [ ] repeat-shell
3. [ ] repeat-volume
4. [ ] raise
5. [ ] focus-nav
6. [ ] focus-tracking
7. [ ] hide
8. [ ] place (legacy grid cycle)
9. [ ] place-async
10. [ ] place-animated
11. [ ] place-term
12. [ ] place-increments
13. [ ] place-fake
14. [ ] place-fallback
15. [ ] place-smg
16. [ ] place-flex
17. [ ] place-skip
18. [ ] place-move-min
19. [ ] place-move-nonresizable
20. [ ] place-minimized
21. [ ] place-zoomed
22. [ ] ui
23. [ ] minui
24. [ ] fullscreen
25. [ ] world-status
26. [ ] world-ax
27. [ ] world-spaces

2. Post-Port Cleanup
1. [ ] Remove `crates/smoketest/src/tests` once all scenarios live under `cases/`.
2. [ ] Delete `crates/smoketest/src/test_runner.rs` after migrating remaining call sites.
3. [ ] Replace per-command handlers in `main.rs` with suite-driven case dispatch only.
4. [ ] Trim CLI enums (`Commands`, `SeqTest`) to map directly onto the registry cases.
5. [ ] Drop legacy helper plumbing (`run_case`, watchdog wrappers) once unused by the CLI.
6. [ ] Update docs and configs to remove references to the legacy smoketest harness.

6. Stage Six: Coverage Pack and Capture Importers
1. [ ] Seed 6-10 mimic scenarios under `crates/smoketest/src/cases` with budgets, standardized skip
       reasons, and consistent `scenario_slug/window_label` labels mirrored in artifacts.
2. [ ] Implement `--repeat`, `--stress`, and `--seed` flags in `cli.rs`, piping the realized seed into
       mimic harnesses and artifacts (`seed=<n> scenario=<slug>`).
3. [ ] Extend `hotki-tester` with `smoke import` that applies documented heuristics when generating
       mimic stubs (`AxStaleAfterMove(T)`, `RaiseCyclesToSibling`, `IgnoreMoveIfMinimized`).
4. [ ] Create `docs/importer-heuristics.md` describing capture-to-quirk mappings with succinct
       examples for each rule.
5. [ ] Add `smoke preview` to visualize capture timelines (`t=+42ms FocusedWindowChanged -> W#12`) and
       verify they match deterministic mimic playback.
6. [ ] Introduce an `assume!` macro that prints `SKIP: <case> -- <reason>` and takes an explicit case
       name, using canonical environment probes (`world.has_multiple_displays()`, etc.) so skip logic
       stays centralized.
7. [ ] Ensure mimic randomness keys off the runner seed, record it alongside the scenario slug, and
       embed the failure message template fields in artifacts.
8. [ ] Document skip budgets, stress thresholds, and seed handling inline with scenarios and in the
       testing principles.
*Acceptance:* At least six mimic scenarios run green with standardized names, `smoke preview`
             timelines align with importer captures, and skips leverage the shared probes while
             logging `seed` metadata.

7. Stage Seven: Guardrails and Validation
1. [ ] Add a CI guard that blocks any platform module import (`ax`, `appkit`, `coregraphics`, `mac_`)
       under `crates/smoketest`.
2. [ ] Add CI checks banning `std::thread::sleep` in smoketest code and async test attributes in
       `crates/hotki-world/tests`.
3. [ ] Extend documentation to cover `test-introspection` defaults and feature expectations in the
       crate-level docs.
4. [ ] Add `smoketest stats --runs N` reporting pass %, mean, p95, and flake rate per case, and surface
       the table in CLI output.
5. [ ] Update the smoketest CLI to emit a final summary table: passed / failed / skipped (reason) with
       per-case timings.
6. [ ] Update the PR template with LOC delta, helper surface counts, stats invocation guidance, and an
       artifact checklist.
7. [ ] `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests --examples`.
8. [ ] `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml`.
9. [ ] Run `cargo test --all` and `cargo run --bin smoketest -- all` (extended timeout) to confirm
       stability goals.
*Acceptance:* CI fails fast on stray platform imports, sleeps, or async test attrs; stats output and
             summary tables surface the required metrics; and final tooling runs stay green.

```rust
pub struct Frames {
    pub authoritative: RectPx,
    #[cfg(feature = "test-introspection")] pub ax: Option<RectPx>,
    #[cfg(feature = "test-introspection")] pub cg: Option<RectPx>,
    pub display_id: u64,
    pub space_id: u64,
    pub scale: f32,
    pub mode: WindowMode,
}
```

```rust
pub enum RaiseStrategy { None, AppActivate, KeepFrontWindow }
pub enum MinimizedPolicy { DeferUntilUnminimized, AutoUnminimize }

pub struct PlaceOptions {
    pub raise: RaiseStrategy,
    pub minimized: MinimizedPolicy,
    pub animate: bool,
}
```

```rust
fn reconcile_authoritative(
    ax: Option<RectPx>,
    cg: Option<RectPx>,
    mode: WindowMode,
    scale: f32,
) -> RectPx {
    match mode {
        WindowMode::Minimized => last_known_unminimized_frame(),
        WindowMode::Fullscreen | WindowMode::Tiled => system_layout_frame(),
        _ => match (ax, cg) {
            (_, Some(cg)) => cg,
            (Some(ax), None) => ax,
            (None, None) => RectPx { x: 0, y: 0, w: 0, h: 0 },
        },
    }
}
```

```rust
let before = cur.lost_count;
let ok = confirm();
if ok && cur.lost_count > before {
    bail!(
        "events lost during wait (lost_count={}): see artifacts",
        cur.lost_count
    );
}
```

```rust
pub fn default_eps(scale: f32) -> i32 {
    if scale >= 1.5 { 1 } else { 0 }
}
```

```rust
#[macro_export]
macro_rules! assume {
    ($cond:expr, $reason:expr, $case:expr) => {
        if !$cond {
            println!("SKIP: {} -- {}", $case, $reason);
            return Ok(());
        }
    };
}
```

## Risk Register

* **Runloop starvation in tests.** Mitigation: always use `pump_main_until` in waits; ban async tests
  without the harness.
* **Event overflow hides a flake.** Mitigation: enforce `lost_count == 0` on success paths; otherwise
  fail loudly.
* **Ambiguous authoritative behavior across modes.** Mitigation: codify the rules and unit test each
  mode.
* **Mimic leaks between cases.** Mitigation: `reset()` force-kills mimics; quiescence asserts zero
  `mimic_windows`.
* **Accidental platform usage in tests.** Mitigation: CI grep and publish a "don't" list in docs.

## Bottom Line

Pin down authoritative reconciliation, raise strategy selection, post-wait lost-event checks, and
cropped artifacts. These changes eliminate the remaining flake sources while keeping the surface
world-centric.
