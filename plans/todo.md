# Hotki Reliability Roadmap — Clean Slate (Sep 14, 2025)

We’ve condensed all completed work into a succinct summary and rebuilt the remaining plan into fresh, 0‑indexed stages. Treat this file as the single source of truth for next actions. All checklist items include exact touch points and validation guidance so a fresh agent can proceed with zero external context.

**What’s Done (succinct)**
- Placement stability: choose initial write order from cached settable bits; skip unsettable writes; structured clamp flags; safe‑park preflight near global (0,0); explicit local→global conversion; grid math tested.
- AX observability: per‑PID AXObserver with runloop source; rich AxEvent (Created/Destroyed/Focused/Moved/Resized/TitleChanged); throttled AxEvent→Hint integration; world “hint refresh” fast‑path.
- AX read isolation: per‑PID read pool with bounded concurrency and stale‑drop; pool behavior covered by tests.
- Focus/raise robustness: prefer AXRaise with NS app activation fallback; private window id via `_AXUIElementGetWindow` when available; title fallback; all AX mutations verified on AppKit main thread.
- Coalescing: enqueue‑time per‑window/pid coalescer for placement ops (older intents dropped before drain).
- Diagnostics: periodic world snapshot dump flag; once‑per‑bundle dedupe for non‑settable warnings.

Guardrails & Validation
- Platform: macOS only. Accessibility permission must be granted to the invoking terminal.
- AX mutations: main thread only.
- Lint/format: `cargo clippy -q --fix --all-targets --all-features --allow-dirty --tests --examples 2>&1` then `cargo +nightly fmt --all`.
- Tests (bounded): `timeout 100s cargo test --all --all-features`.
- Smoketest: `cargo run --bin smoketest -- all`.

---

0. Stage Zero — Coalescing Finalization

Goal: Eliminate residual rubber‑banding and bound latency during bursts.

1. [x] Deadline‑aware draining.  
   Change: Implemented time‑boxed batch draining with in‑batch coalescing for placement ops. `drain_main_ops` now:
   - Collects ops for ~30 ms or until the queue is empty.
   - Preserves FIFO order for non‑placement ops in the batch.
   - Applies only the latest placement per `WindowId` or `pid` using last‑writer order.  
   Touch: `crates/mac-winops/src/lib.rs` (new deadline batcher, in‑batch coalescing, execution helper).  
   Tests: Added unit test `deadline_batching_coalesces_to_single_apply` in `crates/mac-winops/src/lib.rs` using test‑only indirection to count placement applies without touching AX.  
   Validation: `timeout 100s cargo test --all --all-features`.

2. [x] Cross‑type stale‑drop at drain time.  
   Change: During the batch drain, if any id‑specific placement maps to the same `pid` as a focused placement present in the batch, drop the focused one (id‑specific wins).  
   Touch: `crates/mac-winops/src/lib.rs` (added batch‑time drop logic).  
   Tests: `cross_type_stale_drop_prefers_id_over_focused` uses a test‑only id→pid map to assert only the id‑specific placement applies.

   [x] 2.1 Best‑effort id→pid resolver.  
   Change: `resolve_pid_for_id(WindowId) -> Option<pid>` tries, in order: test‑only map (for hermetic tests), CoreGraphics `list_windows` (fast path), and non‑test builds fall back to AX window resolution. If resolution fails, keeps both ops (no drop).

---

1. Stage One — Smoketest Helpers (Window Types)

Goal: Realistic window behaviors to exercise verification and settling.

1. [ ] Async window: delay `setFrame:` ~50 ms; ensures polling handles delayed application.  
   Touch: `crates/smoketest/src/winhelper.rs` (+ AX hooks if needed).  
   Tests: `place` converges within 250 ms.

2. [ ] Animated window: tween to target over ~120 ms; ignore mid‑animation reads.  
   Touch: same.  
   Tests: `place` passes with temporary eps relaxation.

3. [ ] Min‑size window: enforce ≥800×600 via `constrainFrameRect:`.  
   Touch: same.  
   Tests: grid target shrinks to constraints; verification passes.

4. [ ] Tabbed window: NSWindow tabbing on; some writes ignored until focus.  
   Touch: same.  
   Tests: raise+place sequencing converges.

5. [ ] Non‑movable panel: refuse position writes; size‑only settable.  
   Touch: same.  
   Tests: “skip unsettable” path yields success via size‑only.

6. [ ] Sheet‑attached parent: placement is skipped by role/subrole guard.  
   Touch: same.  
   Tests: scenario logs “skipped” and does not retry.

---

2. Stage Two — Test Stabilization & Coverage

Goal: Make `smoketest -- all` consistently green and harden edge cases.

1. [ ] Orchestrator readiness: stabilize `all` runs for `place`/`place‑minimized` with explicit readiness waits and tuned key‑send delays.  
   Touch: `crates/smoketest/src/orchestrator.rs`.  
   Tests: flake rate → 0 locally.

2. [ ] Multi‑display geometry: displays to the left of primary (negative origin) trigger safe‑park then converge.  
   Touch: `crates/smoketest` harness.  
   Tests: assert safe‑park log precedes normal placement; no `BadCoordinateSpace`.

3. [ ] Synthetic churn & stress: zoom/minimize/title churn + 10 Hz placements for 5 s; bound settle and no deadlocks/panics.  
   Touch: `crates/smoketest` (CLI + helpers).  
   Tests: enforce time and liveness budgets.

---

3. Stage Three — Verification & Tuning

Goal: Adapt settle tolerance to animated windows and bound effort.

1. [ ] Animated‑aware eps: temporarily relax eps to ~3–4 pt while rect changes between polls; restore baseline afterwards.  
   Touch: `crates/mac-winops/src/place.rs` (detect animation in settle loop).  
   Tests: animated helper passes; non‑animated unaffected.

2. [ ] Cap total placement effort: at most two order attempts plus one fallback within ≤800 ms total.  
   Touch: `crates/mac-winops/src/place.rs`.  
   Tests: enforced budget with logs.

---

4. Stage Four — Observers

Goal: Validate AX observer pipeline end‑to‑end and lifecycle safety.

1. [ ] Observer integration: with observers enabled, create/close windows; world snapshot updates within one hint cycle.  
   Touch: `crates/smoketest` (flagged test).  
   Tests: event‑to‑hint timing within budget.

2. [ ] Observer lifecycle & memory: repeated app launch/quit; observers detach; CF objects released (debug build leak check).  
   Touch: `crates/smoketest`.  
   Tests: no leaks, stable teardown.

---

5. Stage Five — Diagnostics & Telemetry

Goal: Set expectations, quiet logs, and capture minimal metrics (opt‑in).

1. [ ] Document Spaces caveats & recommendations.  
   Touch: `README.md` (separate Spaces behavior; current scope).  
   Tests: docs build; guidance is clear.

2. [ ] Logging gates & dedupe: consolidate noisy warnings behind targeted gates and extend once‑per‑bundle dedupe beyond non‑settable attributes.  
   Touch: `crates/mac-winops` logging.  
   Tests: repeated scenarios emit at most one warning per bundle.

3. [ ] Basic telemetry counters (opt‑in): counts for `settle_time_ms`, `attempts`, `order_used`, `fallback_used`, and final `outcome`.  
   Touch: `crates/mac-winops` (feature‑gated if needed).  
   Tests: counters increment and reset correctly.

4. [ ] AX setter call counters for unit tests: test‑only `AtomicUsize` hooks for `ax_set_point/size` with clear/query API.  
   Touch: `crates/mac-winops/src/ax.rs`; unit in `place.rs`.  
   Tests: verifies “skip unsettable” reduces AX writes.

---

Notes
- Paths match the current tree (`crates/mac-winops`, `crates/hotki-world`, `crates/smoketest`, …).
- All steps assume macOS with Accessibility permission granted to the invoking terminal.
