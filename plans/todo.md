# Hotki Smoketests 2.0

Deterministic, event-driven, world-centric smoketests. Tests only consume hotki-world. No second
data channel; hotki-world is the single assertion surface.

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

1. Stage One: Immediate Stabilization
1. [x] Replace `wait_for_idents` polling in UI demos with an event-driven binding watcher that keeps
       resending activation until the nested chords arrive and the HUD stays frontmost.
2. [x] Introduce a reusable focus guard that loops raise + SmartRaise until world and AX agree, then
       retrofit fullscreen and placement suites to consume it before and after their actions.
3. [x] Add an explicit repeater shutdown that drains shell tasks, clears observers, and verifies
       volume restoration before world teardown across all repeat suites.
4. [x] Eliminate the lingering `dead_code` warnings in `crates/smoketest/src/config.rs` by pruning or
       wiring through the unused helper configuration fields.

2. Stage Two: Coverage Pack and Capture Importers
1. [ ] Implement `--repeat`, `--stress`, and `--seed` flags in `cli.rs`, piping the realized seed into
       mimic harnesses and artifacts (`seed=<n> scenario=<slug>`).
2. [ ] Extend `hotki-tester` with `smoke import` that applies documented heuristics when generating
       mimic stubs (`AxStaleAfterMove(T)`, `RaiseCyclesToSibling`, `IgnoreMoveIfMinimized`).
3. [ ] Create `docs/importer-heuristics.md` describing capture-to-quirk mappings with succinct
       examples for each rule.
4. [ ] Add `smoke preview` to visualize capture timelines (`t=+42ms FocusedWindowChanged -> W#12`) and
       verify they match deterministic mimic playback.
5. [ ] Introduce an `assume!` macro that prints `SKIP: <case> -- <reason>` and takes an explicit case
       name, using canonical environment probes (`world.has_multiple_displays()`, etc.) so skip logic
       stays centralized.
6. [ ] Ensure mimic randomness keys off the runner seed, record it alongside the scenario slug, and
       embed the failure message template fields in artifacts.
7. [ ] Document skip budgets, stress thresholds, and seed handling inline with scenarios and in the
       testing principles.
*Acceptance:* At least six mimic scenarios run green with standardized names, `smoke preview`
             timelines align with importer captures, and skips leverage the shared probes while
             logging `seed` metadata.

3. Stage Three: Guardrails and Validation
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
