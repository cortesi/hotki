# Project Review

This document captures the highest-value improvements and issues found across the workspace.
The emphasis is on structural refactorings, simplifications, and places where hidden structure
can be excavated to reduce LOC and long-term maintenance cost.

Validation context: `cargo test --all` passed, and `cargo clippy -q --all --all-targets
--all-features --tests --examples` completed cleanly. The items below are therefore mostly
about correctness risks, overgrown modules, duplicated models, and API cleanup rather than
current red tests or lint failures.

1. Stage One: Correctness And High-Risk Design Faults

These items combine concrete bugs with architectural hot spots that are already too
error-prone.

1. [x] Fix the dead fast-startup poll in `crates/hotki-server/src/client.rs:193-207` and
       `crates/hotki-server/src/client.rs:246-275`.
Refs: the fast readiness poll in `try_connect_with_retries()` only runs when `self.server` is
set, but `self.server` is only populated after a successful connection.
Why: this is a real correctness bug, and it also obscures the intended client/server lifecycle.
Refactor direction: store the spawned `ServerProcess` before entering the retry path, or pass an
explicit `spawned: bool` into the readiness routine.

2. [x] Split `Engine::handle_key_event()` into a pure dispatch planner plus effect execution.
Refs: `crates/hotki-engine/src/lib.rs:596-865`.
Why: this function currently mixes selector input, binding dispatch, handler execution, manual
lock dropping, UI notifications, auto-exit, and final rebinds. It is the highest-risk engine
maintenance point.
Refactor direction: introduce a `DispatchPlan` or `DispatchSession` that computes
`SelectorUpdate`, `OpenSelector`, `ApplyHandler`, `ApplyAction`, `Nav`, or `Noop` first, then
executes side effects afterward.

3. [x] Extract a pure runtime refresh/reconcile step from
       `crates/hotki-engine/src/lib.rs:356-565`.
Refs: `rebind_and_refresh()` mixes config rendering, root-stack repair, warning delivery,
binding diffing, repeater cleanup, capture mode updates, and HUD publication.
Why: the function hides the actual engine state model, and the same root-frame construction is
duplicated again at `:372`, `:537`, `:1114`, and `:1201`.
Refactor direction: add `root_frame(...)` plus `RuntimeState::reconcile(...) -> RefreshPlan`.

4. [x] Break `HotkeyService` into focused server components instead of one mixed controller.
Refs: `crates/hotki-server/src/ipc/service.rs:50-240` and
`crates/hotki-server/src/ipc/service.rs:243-420`.
Why: the current type owns engine bootstrapping, event fanout, heartbeat, world forwarding,
shutdown, MRPC request parsing, and response encoding.
Refactor direction: extract `event_bus`, `world_forwarder`, and `rpc_methods` modules, leaving
`HotkeyService` as composition glue.

2. Stage Two: Highest-Yield LOC Reduction

These changes should cut the most code while also clarifying internal structure.

1. [x] Collapse the duplicated `bind`/modifier overloads in
       `crates/config/src/dynamic/dsl.rs`.
Refs: `crates/config/src/dynamic/dsl.rs:649`, `:803`, and `:950`.
Why: binding construction and modifier mutation are duplicated across `Action`, `HandlerRef`,
`SelectorConfig`, `FnPtr`, and array forms, then duplicated again for `BindingRef` and
`BindingsRef`.
Refactor direction: introduce one internal `BindingSpec` parser plus a shared
`mutate_bindings(indices, op)` helper.

2. [x] Unify binding-style parsing, which currently exists twice with near-identical schemas.
Refs: `crates/config/src/dynamic/dsl.rs:570-646` and
`crates/config/src/dynamic/render.rs:37-92` plus `:309-352`.
Why: both modules define a `RawBindingStyle` shape and both rebuild `RawHud`/`RawStyle`
overlays field by field.
Refactor direction: move this into a single `binding_style` module with
`parse_binding_style(map)` and `to_raw_overlay(style)`.

3. [x] Break up `crates/smoketest/src/server_drive.rs`, which currently combines the public
       driver, blocking bridge client, HUD cache, wait logic, fake bridge server helpers, and
       unit tests.
Refs: `crates/smoketest/src/server_drive.rs:75-233`, `:360-966`, and `:978-1615`.
Why: this is the largest single LOC-reduction target in the workspace.
Refactor direction: extract `bridge_client.rs`, `hud_wait.rs`, and `bridge_test_support.rs`,
leaving `server_drive.rs` as a thin orchestration facade.

4. [x] Replace the duplicated smoketest command-switch layer with one transport-neutral bridge
       adapter.
Refs: `crates/hotki-server/src/ipc/service.rs:312-420`,
`crates/hotki/src/connection_driver.rs:266-332`, and
`crates/hotki/src/smoketest_bridge.rs:74-270`.
Why: `SetConfig`, `InjectKey`, `GetBindings`, `GetDepth`, and `Shutdown` are expressed twice,
once as MRPC methods and again as bridge commands.
Refactor direction: move the bridge executor into `hotki-server`, or add a typed adapter over
`hotki_server::Connection` so the bridge becomes transport-only.

5. [x] Extract a shared overlay/window controller for HUD, selector, notification, and details.
Refs: `crates/hotki/src/hud.rs:33-79`, `crates/hotki/src/selector.rs:56-123`,
`crates/hotki/src/notification.rs:54-110`, and `crates/hotki/src/details.rs:42-100`.
Why: each viewport reimplements the same state shape: `DisplayMetrics`, `last_pos`,
`last_size`, visibility resets, and NSWindow post-processing.
Refactor direction: centralize placement caching, display invalidation, viewport commands, and
shared NSWindow setup; leave each module responsible only for size calculation and paint.

6. [x] Flatten selector runtime layering and derive capture bindings from one keymap source.
Refs: `crates/hotki-engine/src/selector.rs:129-259`, `:277-351`, and `:353-410`.
Why: `SelectorState` mostly forwards to `SelectorSession`, which mostly forwards to
`SelectorMatcher`, and the selector keyboard grammar is encoded twice.
Refactor direction: merge `SelectorSession` into `SelectorState`, store `SelectorItem`
directly in the matcher, and define one declarative keymap that drives both action decoding and
capture chords.

7. [x] Unify the shell and relay repeat lifecycles in `crates/hotki-engine/src/repeater.rs`.
Refs: `crates/hotki-engine/src/repeater.rs:264-318` and `:320-430`.
Why: immediate first run, optional ticker setup, callback emission, and stop/replace behavior
are duplicated for shell and relay jobs.
Refactor direction: factor the common scheduling and cancellation path into a `RepeatJob`
abstraction or a small executor enum.

3. Stage Three: API Tending And Boundary Cleanup

These items reduce duplicated models and expose cleaner cross-crate boundaries.

1. [x] Consolidate shared resolved UI/style types into `hotki-protocol`.
Refs: `crates/config/src/types.rs:5-145`, `crates/config/src/style.rs:48-179`, and
`crates/hotki-protocol/src/lib.rs:22-317`.
Why: `Mode`, `FontWeight`, `Pos`, `Offset`, `NotifyPos`, and resolved HUD/notification style
types exist in both crates, then `hotki-engine` manually converts between them at
`crates/hotki-engine/src/lib.rs:1366-1479`.
Refactor direction: let `config` own parsing and overlay logic, but emit the canonical resolved
types from `hotki-protocol`.

2. [x] Narrow the engine-facing surface of `config::dynamic`.
Refs: `crates/config/src/dynamic/mod.rs:31-41`,
`crates/hotki-engine/src/lib.rs:373-452`, and `crates/hotki-engine/src/runtime.rs:1-46`.
Why: `ModeFrame`, `RenderedState`, `BindingKind`, `NavRequest`, `Effect`, and renderer helpers
leak directly into the engine, tightly coupling both crates.
Refactor direction: introduce a smaller engine bridge API in `config::dynamic` and make the
frame/render machinery crate-private.

3. [x] Split `hotki-protocol` into focused modules instead of one root file handling style,
       display geometry, event transport, channel helpers, heartbeat tuning, and RPC types.
Refs: `crates/hotki-protocol/src/lib.rs:11-317` and `:465-590`.
Why: the seams are already clear, but every change currently lands in one giant file.
Refactor direction: create `style`, `display`, `ui`, and `ipc`/`rpc` modules and re-export
intentionally.

4. [x] Reduce the `WorldView` trait to a smaller core and introduce one named focus snapshot
       type shared across world and protocol.
Refs: `crates/hotki-world/src/lib.rs:170-223`, `:241-281`, and
`crates/hotki-protocol/src/lib.rs:11-20`.
Why: `WorldView` exposes many overlapping projections over the same snapshot, most returning
fresh owned data; focus state is represented as `WorldWindow`, `FocusChange`, `App`, and also
as raw `(String, String, i32)` tuples.
Refactor direction: keep `snapshot()`, `focused()`, `displays()`, `status()`, and
`subscribe()` in the trait, and move derived helpers to free functions or extension methods
around a shared `FocusSnapshot`.

5. [x] Remove the one-to-one duplication between `MsgToUI` and `AppEvent`.
Refs: `crates/hotki/src/app.rs:15-54`, `crates/hotki/src/connection_driver.rs:335-403`, and
`crates/hotki-protocol/src/lib.rs:532-587`.
Why: the UI transport layer is translated almost verbatim, which creates a maintenance fork
every time the protocol changes.
Refactor direction: send `MsgToUI` directly to the UI thread and keep only a small local enum
for app-only commands such as `Shutdown` and `ShowPermissionsHelp`.

4. Stage Four: Secondary Cleanup

These are smaller but still worthwhile changes once the main seams above are opened.

1. [ ] Replace the manual raw-style overlay boilerplate in `crates/config/src/raw.rs` with a
       generated or table-driven overlay mechanism.
Refs: `crates/config/src/raw.rs:221-277`, `:388-431`, `:438-528`, and `:544-552`.
Why: `RawNotify`, `RawHud`, `RawSelector`, and `RawStyle` all hand-roll `merge` and
`into_*_over` logic.
Refactor direction: extract generic field-wise overlay helpers or a macro/derive for overlay
structs.

2. [ ] Simplify `KeyBindingManager`'s internal representation.
Refs: `crates/hotki-engine/src/key_binding.rs:19-34`, `:65-154`, and `:177-191`.
Why: one concept is spread across `id_map`, `chord_map`, `inv_map`, and `last_bound`, which
forces extra cloning and bookkeeping.
Refactor direction: keep one `HashMap<String, BindingRegistration>` plus a reverse
`HashMap<u32, String>`.

3. [ ] Deduplicate low-value helpers in `Engine::apply_action()`.
Refs: `crates/hotki-engine/src/lib.rs:920-1103`.
Why: `RepeatSpec` translation is repeated for shell, relay, and volume actions; AppleScript
construction is repeated for volume branches; theme stepping is spread across three similar
arms.
Refactor direction: extract helpers such as `repeat_spec(...)`, `start_shell_action(...)`, and
`set_theme_by(...)`.

4. [ ] Replace the hardcoded logging crate list with a single source of truth.
Refs: `crates/logging/src/lib.rs:18-34`.
Why: `OUR_CRATES` must be updated manually as the workspace changes, which guarantees drift.
Refactor direction: derive this list from workspace metadata at build time, or expose a small
builder API so each binary declares its own additions explicitly.

5. [ ] Bring smoketest temporary-path handling back in line with repo policy.
Refs: `crates/smoketest/src/session.rs:185-194` and
`crates/smoketest/src/suite.rs:640-657`.
Why: bridge sockets are allocated under `env::temp_dir()` instead of `./tmp/`, which makes test
artifacts harder to inspect and breaks the repo-local scratch-space convention.
Refactor direction: allocate control sockets under `tmp/` with per-run subdirectories.
