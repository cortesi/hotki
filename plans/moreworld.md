**Overview**
- hotki-world becomes the single source of truth for window and focus state.
- We will transition gradually where possible, and switch abruptly where necessary to avoid split‑brain behavior.
- Keep write-side window operations in mac-winops (raise, place, fullscreen, move, hide). After such actions, nudge the world with `hint_refresh()` to accelerate reconciliation.
- Provide a push channel from the server to the UI for world events.
- No bounded SLA; deliver best‑effort timeliness with practical mitigations (immediate first reconcile, refresh hints, and debouncing).

---

## Step 1 — Strengthen hotki-world API

Tasks
- [x] Immediate first reconcile: on spawn, reset the first timer so a reconcile runs right away (no initial delay).
- [x] Add getters:
  - [x] `WorldHandle::focused_window() -> Option<WorldWindow>`
  - [x] `WorldHandle::focused_context() -> Option<(String /*app*/, String /*title*/, i32 /*pid*/)>`
- [x] Add `subscribe_with_snapshot() -> (broadcast::Receiver<WorldEvent>, Vec<WorldWindow>, Option<WindowKey>)` to seed clients atomically.
- [x] Make event buffer size configurable: `WorldCfg { events_buffer: usize }` (defaults to 256). Document drop semantics.
- [x] Document focus rules (AX‑preferred, CG fallback), debounce behavior, and display mapping.

Acceptance
- [x] A `FocusChanged` event is produced at startup when a focused window exists (with immediate first reconcile).
- [x] `focused_context()` returns `(app,title,pid)` matching the focused `WorldWindow`.
- [x] Snapshot available within ~`poll_ms_min` (best effort, typically ≤ 50 ms with defaults).

---

## Step 2 — Add server push channel for world events to UI

Tasks
- [x] Define encoding for `WorldEvent` (Added/Removed/Updated/FocusChanged) in `hotki-protocol`/server using MRPC values (`MsgToUI::World(WorldStreamMsg)` with `WorldWindowLite`).
- [x] Reuse existing notify channel with a new envelope type (`MsgToUI::World`).
- [x] Implement subscription inside server: bridge `WorldHandle::subscribe()` to push events to connected UI clients.
- [x] Auto‑start world stream on first client connect.
- [x] Rate limiting/backpressure:
  - [ ] Coalesce bursts on the server if the UI stalls; ensure final state reaches the UI.
  - [x] On overflow, send a synthetic “ResyncRecommended” and let UI refetch snapshot.

Acceptance
- [x] UI receives `FocusChanged` within one world polling cycle of the source change (best effort).
- [x] Under heavy churn, UI either stays current or receives a resync hint and can call a snapshot method.

---

## Step 3 — Engine: begin consuming world (gradual)

Tasks
- [ ] Introduce `FocusContext` cache in Engine, updated by `WorldEvent::FocusChanged`.
- [ ] Seed cache at init via `world.focused_context()` or the topmost by `z` from `world.snapshot()`.
- [ ] Replace reads incrementally:
  - [ ] Raise: use `world.snapshot()` (sorted by `z`) for matching; fallback to `winops.list_windows()` only if snapshot is empty during early startup.
  - [ ] Place: choose pid from `last_target_pid` or `world.focused_context()`; fallback to current CG frontmost only if world has not produced a snapshot yet.
  - [ ] PlaceMove: derive frontmost-for-pid from snapshot: prefer `focused==true` for that pid; else smallest `z` for that pid. Fallback to CG only if necessary during early startup.
  - [ ] Hide logging: log using `world.focused_context()` and topmost-by-`z` from snapshot.
- [ ] Sync-on-dispatch: when enabled, use `world.hint_refresh()` and proceed with cached context; avoid `poll_now`.
- [ ] After every action that likely changes focus/geometry, call `world.hint_refresh()`; optionally schedule a second hint ~60 ms later to catch animations.

Acceptance
- [ ] Engine behaviors (Raise/Place/Move/Hide) match pre‑migration results in tests.
- [ ] During early startup (before first world tick), fallbacks prevent user-visible regressions.

---

## Step 4 — Cut over Engine to world (abrupt)

Tasks
- [ ] Remove `mac_winops::focus::{FocusWatcher, FocusSnapshot, poll_now}` from Engine.
- [ ] Remove CG read paths for frontmost/snapshots from Engine (keep only for diagnostics in tools/tests where appropriate).
- [ ] Ensure all focus-dependent code paths rely solely on world cache/snapshot.

Acceptance
- [ ] No Engine references to `mac_winops::focus` remain.
- [ ] All tests pass relying on world only; smoketest indicates no missed focus transitions.

---

## Step 5 — Server adjustments and polish

Tasks
- [ ] Maintain NS main-thread plumbing in `mac-winops` (set_main_proxy, post_user_event, install_ns_workspace_observer, drain_main_ops). No change required.
- [ ] Expose optional RPCs if helpful to UI tooling (e.g., `get_world_snapshot`).
- [ ] Confirm push channel stability under reconnects; on reconnect, UI gets a snapshot then resumes streaming.

Acceptance
- [ ] Server compiles without FocusWatcher dependencies and streams world events to UI as designed.

---

## Step 6 — Tests, smoketests, and observability

Tasks
- [ ] Update Engine tests to build world state via `mac_winops::ops::MockWinOps` and a real `World` actor.
- [ ] Convert tests that injected `FocusSnapshot` to react to `WorldEvent::FocusChanged` and snapshots.
- [ ] Add smoketest: rapidly alternate focus between two apps; assert UI and Engine rebinding keeps up.
- [ ] Add logs/metrics:
  - [ ] Log time from action → `hint_refresh` → next reconcile → event received by Engine.
  - [ ] World status logging at INFO on permission issues; document expected effects (e.g., redacted titles without Screen Recording).

Acceptance
- [ ] All tests pass locally and in CI.
- [ ] Smoketest green with no missed focus changes.

---

## Step 7 — Cleanup

Tasks
- [ ] Remove any remaining dead code paths or utility wrappers used during migration.
- [ ] Update crate-level docs for Engine/Server to state hotki-world is the source of truth for window state.
- [ ] Review `hotki-shots` and other tools; keep direct CG calls where they are purely diagnostic.

Acceptance
- [ ] Codebase contains a single read source for focus/window state: hotki-world.
- [ ] Documentation reflects the new architecture.
