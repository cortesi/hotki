# World: Window State Service 

This revision eliminates reliance on deprecated/fragile keys for Spaces, corrects focus
derivation to use AX, adds optional display/z‑order data, introduces adaptive polling,
and formalizes permission handling.&#x20;

### Preamble: Context, Structure, Threads, and Low‑Level Calls

* Project structure (relevant crates) remains as proposed. We continue to use:

  * `mac-winops` for CG and AX glue (enumeration, AX helpers, focus watcher).
  * `hotki-engine` owns the focus watcher and the world service.
  * `hotki-server` hosts Tao + MRPC.

* Process split and threading unchanged:

  * Tao/AppKit main thread for NS/AX observer installation; Tokio runtime for actor and
    timers.
  * macOS‑only.

* Low‑level macOS primitives:

  * **Enumeration + attrs (CG):** `CGWindowListCopyWindowInfo` with option
    `.excludeDesktopElements` by default. Keys used:
    `kCGWindowOwnerPID`, `kCGWindowNumber`, `kCGWindowOwnerName`, `kCGWindowName`,
    `kCGWindowLayer`, `kCGWindowBounds`, `kCGWindowIsOnscreen`. CG enumerations are
    expensive; we adapt polling accordingly. ([Apple Developer][8])
  * **Focus/title (AX):** `AXUIElementCreateSystemWide`, `AXFocusedApplication`,
    `AXFocusedWindow`, `AXTitle`, and targeted observers as an optional mode. ([Apple Developer][3])
  * **Workspace/Spaces:** We **do not** depend on `kCGWindowWorkspace` (deprecated).
    Instead we derive `on_active_space` from `kCGWindowIsOnscreen`. ([Apple Developer][1])
  * **Display mapping:** Compute `display_id` by intersecting window bounds with display
    frames (`NSScreen`/`CGGetActiveDisplayList`). CG doesn’t provide a screen ID. ([Stack Overflow][4])

### Goals and Non‑Goals

* **Goals**

  * Maintain an in‑memory map of current onscreen windows with stable identity
    `{pid, window_id}` and useful derived fields.
  * Track: `app`, `title`, `pid`, `window_id`, `pos` (x, y, w, h), `layer`, `z`
    (front‑to‑back index), `on_active_space`, `display_id`, `focused` (AX‑derived), plus
    Hotki metadata.
  * Provide APIs to:

    * List current windows and fetch by key.
    * Subscribe to added/removed/updated and focus changes, with **server‑side filters**
      (by pid/app/display/layer/visibility).
    * Attach/detach transient `WindowMeta`, GC’ed when a window disappears.

* **Non‑Goals (initial phase)**

  * No persistent metadata across restarts.
  * No global per‑app AX observers for create/destroy in v1 (optional feature gate).

### Data Model

* **Key types**

  * `WindowKey { pid: i32, id: u32 }` (`Eq + Hash + Copy`).

* **World snapshot entity**

  * `WorldWindow`:

    * System fields (CG + derived): `app`, `title` (AX preferred, CG fallback),
      `pid`, `id`, `pos: Option<Pos>`, `layer: i32`, `z: u32`,
      `on_active_space: bool`, `display_id: Option<DisplayId>`.
    * Focus field: `focused: bool` (AX‑derived).
    * Hotki fields: `meta: Vec<WindowMeta>`, `last_seen: Instant`, `seen_seq: u64`.

* **In‑memory store**

  * Single‑writer `HashMap<WindowKey, WorldWindow>`, plus `focused: Option<WindowKey>`.
  * `Capabilities` struct exposed (accessibility/screen‑recording: Granted/Denied).

### Public API (crate: `hotki-world`)

* **Types**

  * `WorldHandle` (cheap clone).
  * `WorldEvent`:

    * `Added(WorldWindow)`
    * `Removed(WindowKey)`
    * `Updated(WindowKey, WindowDelta)`
    * `MetaAdded(WindowKey, WindowMeta)` / `MetaRemoved(WindowKey, WindowMeta)`
    * `FocusChanged(Option<WindowKey>)`
  * `WindowDelta` (changed fields only).

* **Constructors**

  * `World::spawn(winops: Arc<dyn WinOps>, cfg: WorldCfg) -> WorldHandle`

    * `WorldCfg` includes `poll_ms_min`, `poll_ms_max`, and feature toggles (e.g., `ax_watch_frontmost: bool`).

* **Queries / Streams**

  * `snapshot() -> Vec<WorldWindow>`
  * `get(key) -> Option<WorldWindow>`
  * `subscribe(filter: WorldFilter) -> Receiver<WorldEvent>`
  * `focused() -> Option<WindowKey>`
  * `capabilities() -> Capabilities`

* **Metadata ops**

  * `add_meta/remove_meta/clear_meta`.

* **Hints**

  * `send_focus_hint(FocusSnapshot)` (from engine) to trigger immediate reconcile.

### Actor Loop, Timers, and Hints

* **Startup sequence**

  1. Query permission state (Accessibility + Screen Recording) and publish `Capabilities`.
     Titles fall back gracefully if Screen Recording is denied; focus becomes
     “best‑effort” if Accessibility is denied. ([Apple Support][5])
  2. Initial CG enumeration builds the map; emit `Added` for each.
  3. Seed focus from the engine’s `FocusWatcher` (AX). If missing, use frontmost PID and
     pick its topmost layer‑0 window by CG order.

* **Refresh policy**

  * **Adaptive polling** with Tokio:

    * Start with a short interval (e.g., 100–150 ms) when recent deltas are frequent,
      back off towards 500–1000 ms when stable. Bound by `cfg`.
    * Immediate refresh on `FocusSnapshot` or when the frontmost app changes
      (NSWorkspace hint).
    * Coalesce identical deltas within 50 ms.
    * Record `z` as the enumeration index from CG’s front‑to‑back list. ([Apple Developer][11])

* **Reconciliation algorithm**

  * `enumerated = winops.list_windows()` (CG snapshot).
  * Build `key_set` from `enumerated`.
  * Removals: for keys in store but not in `key_set`, emit `Removed`, drop metadata.
  * Inserts: for keys in `key_set` not present, build `WorldWindow` and emit `Added`.
  * Updates: compute `WindowDelta` for `title` (AX preferred), `pos`, `layer`, `z`,
    `on_active_space`, `display_id`.
  * **Focused pointer: AX‑only.** Map `AXFocusedWindow` to `WindowKey`; if mapping fails,
    derive via frontmost PID + topmost layer‑0 CG window; emit `FocusChanged` on change. ([Apple Developer][3])

### Integration Points

* **Engine (`hotki-engine`)**

  * Owns a single `FocusWatcher` and forwards `FocusSnapshot`s to World via
    `send_focus_hint`.
  * Re‑exports `world_snapshot()` and `world_events()` for read‑only consumption.

* **Server (`hotki-server`)**

  * No direct ownership. Can request snapshots or subscribe to events (with filters) for
    diagnostics.

### Permissions, Degradation, and UX

* **Accessibility (AX):** If denied, world still runs on CG, but `focused` becomes
  best‑effort and AX‑only fields degrade. Expose this in `capabilities()`. ([Apple Support][7])

* **Screen Recording:** If denied, CG titles (`kCGWindowName`) are often missing. Prefer
  AX titles for focus app; otherwise leave empty. Surface a warning and advice to grant
  permission in Settings. ([Apple Support][5], [Stack Overflow][6])

### API Sketch (Rust)

```rust
// crates/hotki-world/src/lib.rs (selected changes)

#[derive(Clone, Debug)]
pub struct WorldWindow {
    pub app: String,
    pub title: String,              // AX preferred, CG fallback (may be empty)
    pub pid: i32,
    pub id: WindowId,
    pub pos: Option<mac_winops::Pos>,
    pub layer: i32,
    pub z: u32,                     // front-to-back index from CG
    pub on_active_space: bool,      // from kCGWindowIsOnscreen
    pub display_id: Option<DisplayId>,
    pub focused: bool,              // AX-derived
    pub meta: Vec<WindowMeta>,
    pub last_seen: Instant,
    pub seen_seq: u64,
}

pub struct Capabilities {
    pub accessibility: PermissionState,     // Granted | Denied | Unknown
    pub screen_recording: PermissionState,  // Granted | Denied | Unknown
}

pub struct WorldCfg {
    pub poll_ms_min: u64,           // e.g., 100
    pub poll_ms_max: u64,           // e.g., 1000
    pub ax_watch_frontmost: bool,   // optional per-frontmost-app observers
}

pub enum WorldEvent {
    Added(WorldWindow),
    Removed(WindowKey),
    Updated(WindowKey, WindowDelta),
    MetaAdded(WindowKey, WindowMeta),
    MetaRemoved(WindowKey, WindowMeta),
    FocusChanged(Option<WindowKey>),
}
```

### Actionable Execution Checklist

#### Stage 1: Crate Skeleton

- [x] Create `crates/hotki-world` with `edition = 2024`.
- [x] Add dependencies: `tokio`, `mac-winops`.
- [x] Define public types: `WorldHandle`, `WorldEvent`, `WorldCfg`, `Capabilities`, `WorldWindow`.
- [x] Expose `World::spawn(winops: Arc<dyn WinOps>, cfg: WorldCfg) -> WorldHandle`.

#### Stage 2: Actor + Storage

- [x] Implement single‑writer actor with `mpsc::UnboundedReceiver<Command>`.
- [x] Add `broadcast::Sender<WorldEvent>` for subscribers.
- [x] Implement in‑memory `HashMap<WindowKey, WorldWindow>` store.
- [x] Track `focused: Option<WindowKey>` and `seen_seq: u64`.
- [x] Implement queries: `snapshot`, `get`, `subscribe`, `focused`, `capabilities`.

#### Stage 3: Polling + Hints (Adaptive)

- [x] Implement adaptive interval bounded by `cfg.poll_ms_min` and `cfg.poll_ms_max`.
- [x] Trigger immediate reconcile on hint (`hint_refresh`); engine wiring follows in Stage 6.
- [x] Debounce identical deltas within ~50 ms.
- [x] Record `z` as CG enumeration index.

#### Stage 4: Focus + Title Precedence

- [x] Use AX for `focused` window and preferred title.
- [x] Fallback to CG title when Screen Recording is denied; degrade gracefully when AX is denied.
- [x] Map `AXFocusedWindow` to `WindowKey`; fallback: frontmost PID + topmost layer‑0 CG window.

#### Stage 5: Display + Z‑Order + Active‑Space

- [x] Compute `display_id` by intersecting window bounds with active display frames.
- [x] Set `on_active_space` from the OnScreen CG snapshot (effectively `kCGWindowIsOnscreen`).
- [x] Ensure `z` reflects CG front‑to‑back order for layer‑0 windows.

#### Stage 6: Engine Wiring

- [x] Integrate `hotki-engine` `FocusWatcher`; forward a refresh hint on snapshots.
- [x] Re‑export `world_snapshot()` and `world_events()` for read‑only consumption.

#### Stage 7: Diagnostics + Capabilities

- [x] Implement `world.status()` with last tick duration, window counts, debounce metrics, and current poll interval.
- [x] Detect and expose `Capabilities { accessibility, screen_recording }`.
- [x] Surface user‑visible warnings when permissions are missing.

#### Stage 8: Tests

- [x] Provide mock `WinOps` for CG/AX.
- [x] Test startup: additions, z‑order, and `on_active_space` flags.
- [ ] Test AX focus and title precedence (with/without permissions granted).
- [ ] Test multi‑display `display_id` mapping.
- [ ] Test debounce behavior on repetitive move/resize/title changes.

#### Backlog

- [ ] AX observers for create/destroy on frontmost app and “hot” apps.
- [ ] IPC relay for a diagnostics “Windows” view.
- [ ] Richer `WindowMeta` (color, notes, TTLs).

### Risks and Mitigations (revised)

* **CG polling overhead.** Adaptive cadence + burst‑on‑hint reduce CPU. CG calls are
  documented as relatively expensive; profile under realistic loads. ([Apple Developer][8])

* **Permissions missing.** Degrade gracefully; expose `Capabilities`; provide UX to
  guide users to grant Screen Recording and Accessibility. ([Apple Support][5])

* **Spaces metadata.** We avoid deprecated `kCGWindowWorkspace`; rely on
  `kCGWindowIsOnscreen` for active‑Space presence. ([Apple Developer][1])

* **Identity reuse.** Keep `seen_seq`; consider surfacing an `epoch` for clients.

### Acceptance Criteria (Checklist)

- [ ] Crate builds; all unit/integration tests pass; no panics.
- [ ] Under a running server, World tracks windows and logs deltas and z‑order.
- [ ] Adaptive polling visibly backs off when idle and speeds up on hints.
- [ ] With missing permissions, diagnostics are clear and functionality gracefully degrades.

### Validation Steps (Checklist)

- [x] Run `cargo clippy -q --fix --all-targets --all-features --allow-dirty --tests --examples`.
- [x] Run `cargo fmt --all`.
- [x] Run `cargo test --all`.
- [ ] Local smoke with and without permissions; confirm `Capabilities`, titles, and focus behavior.
- [ ] Multi‑display smoke; verify `display_id` mapping and z‑order correctness.

---

### Quick index to key sources cited

* CG window list basics and cost; options and order: Apple docs. ([Apple Developer][8])
* `kCGWindowIsOnscreen` presence flag: Apple docs. ([Apple Developer][13])
* `kCGWindowWorkspace` deprecated: Apple docs; observed community notes. ([Apple Developer][1], [Keyboard Maestro Discourse][2])
* No display ID in CG window dictionaries; compute via bounds: discussion. ([Stack Overflow][4])
* Screen Recording permission affects CG metadata (e.g., titles): Apple support + dev
  guidance. ([Apple Support][5], [Stack Overflow][6])
* AX focus/title and notification model; per‑app observers: Apple docs. ([Apple Developer][3])
* `OnScreenOnly` excludes minimized windows: community reference. ([GitHub][9])

---

[1]: https://developer.apple.com/documentation/coregraphics/kcgwindowworkspace?utm_source=chatgpt.com "kCGWindowWorkspace | Apple Developer Documentation"
[2]: https://forum.keyboardmaestro.com/t/how-to-list-all-windows-of-one-app-that-are-open-in-all-desktops/23986?page=2&utm_source=chatgpt.com "How to List All Windows of One App That Are Open in All ..."
[3]: https://developer.apple.com/documentation/applicationservices/1462085-axuielementcopyattributevalue?utm_source=chatgpt.com "AXUIElementCopyAttributeValue(_:_:_:)"
[4]: https://stackoverflow.com/questions/19475578/cgwindowlistcopywindowinfo-multiple-screens-and-changing-properties?utm_source=chatgpt.com "CGWindowListCopyWindowInfo: multiple screens and ..."
[5]: https://support.apple.com/guide/mac-help/control-access-screen-system-audio-recording-mchld6aa7d23/mac?utm_source=chatgpt.com "Control access to screen and system audio recording on Mac"
[6]: https://stackoverflow.com/questions/56597221/detecting-screen-recording-settings-on-macos-catalina?utm_source=chatgpt.com "Detecting screen recording settings on macOS Catalina"
[7]: https://support.apple.com/guide/mac-help/allow-accessibility-apps-to-access-your-mac-mh43185/mac?utm_source=chatgpt.com "Allow accessibility apps to access your Mac"
[8]: https://developer.apple.com/documentation/coregraphics/cgwindowlistcopywindowinfo%28_%3A_%3A%29?utm_source=chatgpt.com "CGWindowListCopyWindowInfo(_:_:)"
[9]: https://github.com/lwouis/alt-tab-macos/issues/11?utm_source=chatgpt.com "Handle minimized windows #11 - lwouis/alt-tab-macos"
[10]: https://developer.apple.com/documentation/applicationservices/axnotificationconstants_h?utm_source=chatgpt.com "AXNotificationConstants.h | Apple Developer Documentation"
[11]: https://developer.apple.com/documentation/coregraphics/cgwindowlistoption/optiononscreenonly?utm_source=chatgpt.com "optionOnScreenOnly | Apple Developer Documentation"
[12]: https://developer.apple.com/documentation/applicationservices/1462089-axobserveraddnotification?utm_source=chatgpt.com "AXObserverAddNotification(_:_:_:_:)"
[13]: https://developer.apple.com/documentation/coregraphics/kcgwindowisonscreen?utm_source=chatgpt.com "kCGWindowIsOnscreen | Apple Developer Documentation"
