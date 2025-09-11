# World: Window State Service — Project Plan

Hotki’s world service (crate: `hotki-world`) is the single source of truth for on‑screen windows and focused context on macOS. The engine consumes this cache to drive actions and bindings; the server forwards events and exposes snapshots/RPCs to the UI. This plan lists only remaining, forward‑looking work with checkable tasks.

Assumptions
- macOS‑only.
- Read‑side state comes from `hotki-world`; write‑side ops remain in `mac-winops`.

## Milestones

- [ ] M1: Engine cutover to world‑only focus/context (no `FocusWatcher`).
- [ ] M2: Server forwarder resiliency and resync semantics validated.
- [ ] M3: Tests/smoketests migrated; reconnect and rapid‑focus scenarios green.
- [ ] M4: Observability polish (`WorldStatus`), UI/dev tooling, and cleanup.

## Engine Cutover (world‑only)

- [x] Remove `FocusWatcher`/`FocusSnapshot` dependencies from `hotki-engine`.
  - [x] Drop `Engine::new_with_proxy` and watcher plumbing.
  - [x] Remove `on_focus_snapshot` and `current_snapshot` usages.
  - [x] Ensure `current_context_tuple` and `current_pid_world_first` rely only on world focus cache (`subscribe_with_context`, `context_for_key`).
- [x] Eliminate CG fallbacks in engine.
  - [x] Replace uses of `frontmost_window[_for_pid]` and CG‑derived focus with world context.
  - [x] Decide and implement Raise behavior when snapshot is empty at early startup:
    - [x] Pick policy: no‑op + log OR one‑time CG fallback for first N ticks.
    - [x] Implement and document chosen policy.
- [x] Refactor `Repeater` pid source to world.
  - [x] Provide pid via world‑backed cache (e.g., `Arc<Mutex<Option<i32>>>`).
  - [x] Update relay handoff to follow world focus changes.
- [x] Keep `hint_refresh` on actions; avoid sync reads at dispatch (`sync_focus_on_dispatch` stays enabled).

Acceptance
- [x] No engine references to `mac_winops::focus::*` remain.
- [x] Engine actions (Raise/Place/Move/Hide/Fullscreen/Focus) rely solely on world context.
- [ ] Existing smoketests pass without CG fallbacks for focus reads.

## Server & Protocol

- [ ] World forwarder lifecycle
  - [ ] Ensure single forwarder instance; guard against duplicates across reconnects.
  - [ ] Reconnect test: client reconnects → receives `get_world_snapshot` → resumes streaming.
- [ ] Resync/backpressure handling
  - [ ] On `ResyncRecommended`, UI requests snapshot; document expected behavior.
  - [ ] Add test covering lag → resync → steady‑state.
- [ ] (Optional) Subscription filters to reduce event volume: design API and defer implementation until needed.

Acceptance
- [ ] Reconnect path validated: snapshot consistency + continued events.
- [ ] Resync semantics validated under induced lag.

## UI

- [ ] Developer “Windows” inspector (snapshot view via `get_world_snapshot`).
- [ ] Permissions pane/indicator surfaces `get_world_status` (AX/Screen Recording) state.

Acceptance
- [ ] Inspector shows windows sorted by `z`, including `focused` and `display_id` when present.
- [ ] Permissions warnings visible in UI when missing.

## Tests & Smoketests

- [x] Convert engine tests to world‑only focus.
  - [x] Replace `on_focus_snapshot` seeding with `MockWinOps` + `world.hint_refresh()`.
  - [x] Remove `FocusSnapshot` from tests.
- [ ] Smoketest: rapid focus alternation between two apps.
  - [ ] Assert world emits `FocusChanged` for each switch.
  - [ ] Assert engine’s world focus cache keeps up (no missed transitions).
  - [ ] UI remains responsive.
- [ ] Smoketest: reconnect continuity.
  - [ ] Disconnect/reconnect; assert snapshot then stream resumes.

Acceptance
- [ ] All tests pass locally and in CI.
- [ ] Smoketests green for rapid focus and reconnect scenarios.

## Observability

- [ ] Extend `WorldStatus`/`get_world_status` (if useful now):
  - [ ] Count of dropped `WorldEvent`s (lagged receivers).
  - [ ] Last coalesce flush time and current coalesce set size.
- [ ] Ensure permission warnings (AX/Screen Recording) are logged once and surfaced in UI help.

Acceptance
- [ ] Extended fields appear in RPC and decode cleanly in client.
- [ ] Logs/UI clearly indicate missing permissions.

## Cleanup

- [x] Remove dead code from watcher/CG era in engine and tests.
- [ ] Update crate‑level docs (engine/server) to state `hotki-world` as authoritative read path.
- [ ] Audit tools/smoketests: retain direct CG reads only for diagnostics; document exceptions.

Acceptance
- [ ] Codebase contains a single read source for focus/window state: `hotki-world`.
- [ ] Documentation reflects the new architecture.

## Enhancements (Backlog)

- [ ] `WorldEvent::Updated` carries changed fields (reduce UI fetches); plumb to protocol/UI if needed.
- [ ] `WindowMeta` attach/detach APIs; consider server plumbing if consumers emerge.
- [ ] Subscription filters (`subscribe(filter)`) by pid/app/display/visibility for diagnostics at scale.
- [ ] Optional AX observers for hot apps behind a feature flag.

## Non‑Goals

- Cross‑platform support.
- Persisting metadata across restarts.
