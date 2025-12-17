# Interactive Selector Mode

This plan adds a new interactive selector mode: a fuzzy-search popup that lets users filter and
select from a list of options. The initial use case is a Spotlight-style "run application" launcher,
but the design is generic for any list selection.

---

## Concept

When a selector action is triggered, a popup window appears with:
- A text input field at the top (initially focused)
- A filtered list of options below, ranked by fuzzy match score
- Keyboard navigation to select an option
- Enter to confirm, Escape to dismiss

Each option has:
- **Display text**: shown in the list
- **Auxiliary data**: hidden metadata passed to the callback on selection

When the user selects an item, a user-provided handler fires with the selection details. The
handler receives `ActionCtx` plus `(item, query)` and can queue effects via `ctx.exec(...)`,
`ctx.notify(...)`, and navigation requests.

---

## Specification

### 1. SelectorItem

A single selectable option:

```rust
pub struct SelectorItem {
    /// Primary text displayed in the list.
    pub label: String,
    /// Optional secondary text (smaller, dimmed) shown below/beside label.
    pub sublabel: Option<String>,
    /// Arbitrary auxiliary data passed to the callback on selection.
    ///
    /// Stored as Rhai `Dynamic` for flexibility.
    pub data: Dynamic,
}
```

In Rhai, items are constructed via a map:
```rhai
#{ label: "Safari", sublabel: "/Applications/Safari.app", data: "/Applications/Safari.app" }
```

Or shorthand for label-only:
```rhai
"Safari"  // expands to #{ label: "Safari", data: "Safari" }
```

### 2. SelectorConfig

Configuration for a selector instance:

```rust
pub struct SelectorConfig {
    /// Window title shown in the selector header
    pub title: String,
    /// Placeholder text shown in empty input field
    pub placeholder: String,
    /// Items for the selector.
    ///
    /// For large lists, prefer a lazy provider so we don't store/copy large arrays in
    /// rendered bindings.
    pub items: SelectorItems,
    /// Callback invoked on selection.
    ///
    /// Signature: `|ctx, item, query| { ... }`
    pub on_select: HandlerRef,
    /// Optional callback invoked on cancel (Escape); defaults to no-op.
    pub on_cancel: Option<HandlerRef>,
    /// Max items to display at once (default: 10)
    pub max_visible: usize,
    /// Enable vim-style navigation (Escape to enter nav mode, j/k to move)
    pub vim_mode: bool,
}

pub enum SelectorItems {
    /// Static item list.
    Static(Vec<SelectorItem>),
    /// Lazy item provider evaluated when the selector is opened.
    ///
    /// Signature: `|ctx| -> Array`
    Provider(FnPtr),
}
```

**Provider Error Handling:**

When a `Provider` FnPtr is evaluated and throws a Rhai exception (e.g., `get_applications()` fails
to scan directories):
- Log the error via the standard notification mechanism.
- Open the selector with an empty item list rather than crashing.
- Optionally display a subtle error indicator in the selector UI (e.g., "Failed to load items").

### 3. Dynamic Binding Extension

Selectors should not be added to the primitive `Action` enum. `Action` is currently
`Serialize/Deserialize + Eq + Hash` and intentionally stays free of runtime-only values like
Rhai closures (`FnPtr`) and `Dynamic` payloads.

Instead, add a new `BindingKind` variant in `crates/config/src/dynamic/types.rs`:

```rust
pub enum BindingKind {
    // ... existing variants ...
    /// Open an interactive selector popup.
    Selector(SelectorConfig),
}
```

### 4. Selector State (Runtime)

The engine runtime holds optional selector state:

```rust
pub struct SelectorState {
    /// Configuration for this selector instance (callbacks, settings)
    pub config: SelectorConfig,
    /// Nucleo-based matcher (owns items, handles filtering/ranking)
    pub matcher: SelectorMatcher,
    /// Current input text
    pub query: String,
    /// Currently highlighted index in matched results (0-based)
    pub selected: usize,
    /// Vim mode: true if in navigation mode (after Escape), false if typing
    pub nav_mode: bool,
}

pub struct SelectorMatcher {
    /// Nucleo matcher worker.
    nucleo: Nucleo<SelectorCandidate>,
    /// Local matcher scratch used for computing highlight indices for visible items.
    highlight_matcher: nucleo::Matcher,
}

pub struct SelectorCandidate {
    /// Stable identity to keep selection stable across snapshot updates.
    pub id: u64,
    pub item: SelectorItem,
}
```

### 5. Fuzzy Matching

Use the `nucleo` crate (not `nucleo-matcher`) for high-quality fuzzy matching with background
threading. This choice is deliberate: nucleo provides lock-free item injection and snapshot-based
results, which enables future features like dynamic item sources and filesystem browsing without
architectural changes.

**Why nucleo over nucleo-matcher:**
- `Injector` allows streaming items in without blocking (e.g., files discovered during traversal)
- Background threadpool keeps UI responsive even with thousands of items
- Snapshot model provides instant partial results while matching continues
- `notify` callback integrates naturally with egui's repaint model
- Multi-pattern support enables richer query syntax in the future

**Matching behavior:**
- Match over `label` only.
- Use a single matching column whose haystack is `label`.
- Ranking is score-based; nucleo's tie-break prefers shorter total haystack length, then injected
  order.
- Smart-case + smart normalization via `CaseMatching::Smart` and `Normalization::Smart`.

**Highlighting:**
- `Snapshot` does not expose match indices. Compute match indices for the visible N items only
  using `Pattern::indices` (from `nucleo_matcher`) against `item.matcher_columns[0]`.
- Use `matcher_columns` from the `Item` returned by `Snapshot::matched_items()`. These are already
  `Utf32String`, avoiding re-conversion of the label on every frame.
- Prefer sending **codepoint indices** (`Vec<u32>`) in the protocol to avoid UTF-8 byte indexing
  hazards; the UI can convert indices to byte ranges as needed.

**Integration pattern:**
```rust
// On selector open
let nucleo = Nucleo::new(config, notify_repaint, None, 1); // 1 column
let injector = nucleo.injector();
for candidate in candidates {
    injector.push(candidate, |candidate, cols| {
        cols[0] = candidate.item.label.as_str().into();
    });
}

// Each frame
nucleo.pattern.reparse(
    0,
    &query,
    CaseMatching::Smart,
    Normalization::Smart,
    append,
);
let status = nucleo.tick(10); // 10ms timeout
let snapshot = nucleo.snapshot();
// Render snapshot.matched_items(0..max_visible)
```

### 6. Keyboard Handling

**Normal mode (typing):**
| Key | Action |
|-----|--------|
| Any printable | Append to query, re-filter |
| Backspace | Delete last char, re-filter |
| Up / Ctrl-P | Move selection up |
| Down / Ctrl-N | Move selection down |
| Enter | Confirm selection, invoke callback |
| Escape | Cancel selector (or enter nav mode if vim_mode) |
| Ctrl-U | Clear query |

**Vim nav mode (after Escape in vim_mode):**
| Key | Action |
|-----|--------|
| j / Down | Move selection down |
| k / Up | Move selection up |
| Enter | Confirm selection |
| Escape (again) | Cancel selector |
| i / a / any printable | Return to typing mode |

### 7. Callback Signature

The `on_select` handler is a normal Hotki handler and should not extend `ActionCtx`.
Instead, pass the selection as arguments:

- `on_select(ctx, item, query)` where:
  - `ctx` is `ActionCtx`
  - `item` is a Rhai map: `#{ label, sublabel, data }`
  - `query` is the final query string

The handler queues effects (actions, notifications, navigation) like any other handler.

Example Rhai:
```rhai
action.selector(#{
    title: "Run Application",
    placeholder: "Search apps...",
    items: || get_applications(),  // lazy provider (recommended)
    on_select: |ctx, item, query| {
        // Note: action.shell takes a full command string; quote as appropriate.
        ctx.exec(action.shell("open '" + item.data + "'"))
    },
})
```

### 8. Protocol Extension

Add selector state to the UI protocol:

```rust
pub enum MsgToUI {
    // ... existing variants ...
    /// Show/update selector popup
    SelectorUpdate(SelectorSnapshot),
    /// Hide selector popup
    SelectorHide,
}

pub struct SelectorSnapshot {
    pub title: String,
    pub placeholder: String,
    pub query: String,
    pub items: Vec<SelectorItemSnapshot>,
    pub selected: usize,
    pub nav_mode: bool,
}

pub struct SelectorItemSnapshot {
    pub label: String,
    pub sublabel: Option<String>,
    /// Codepoint indices in `label` to highlight.
    pub label_match_indices: Vec<u32>,
}
```

### 9. UI Component

A new viewport similar to `Hud`:
- Centered on screen (or configurable position)
- Fixed width (~400-500px), height adapts to content
- Transparent background with blur (like HUD)
- Components:
  - Title bar (optional, shows `config.title`)
  - Text input field with cursor
  - Scrollable list of filtered items
  - Selected item highlighted
  - Match characters highlighted in results

### 10. DSL Registration

Register in Rhai:

```rust
// action.selector(config_map) -> SelectorConfig (used as a binding target)
engine.register_fn("selector", |_: ActionNamespace, config: Map| -> SelectorConfig { ... });

// Helper for building item arrays
engine.register_fn("selector_item", |label: &str, data: Dynamic| -> Map { ... });
```

### 11. Application Launcher Helper

Provide a built-in Rhai function for the primary use case:

```rhai
// Returns array of SelectorItems for installed applications
fn get_applications() -> Array

// Usage:
action.selector(#{
    title: "Run Application",
    placeholder: "Search apps...",
    items: || get_applications(),
    on_select: |ctx, item, _query| {
        // Note: action.shell takes a full command string; quote as appropriate.
        ctx.exec(action.shell("open '" + item.data + "'"))
    },
})
```

`get_applications()` scans:
- `/Applications`
- `/System/Applications`
- `~/Applications`
- `/Applications/Utilities`

Returns items with:
- `label`: App name (e.g., "Safari")
- `sublabel`: Full path (e.g., "/Applications/Safari.app")
- `data`: Full path (for `open` command)

---

## Implementation Plan

### Stage 1: Dependencies and Core Types

1. [ ] Add `nucleo` dependency for fuzzy matching (not `nucleo-matcher`).
2. [ ] Define `SelectorItem`, `SelectorItems`, `SelectorConfig` in
       `crates/config/src/dynamic/selector.rs`.
3. [ ] Add `BindingKind::Selector(SelectorConfig)` in `crates/config/src/dynamic/types.rs`.
4. [ ] Add selector state field to `crates/hotki-engine/src/runtime.rs` (initially `None`).

### Stage 2: Protocol Extension

1. [ ] Define `SelectorItemSnapshot` and `SelectorSnapshot` in `crates/hotki-protocol/src/lib.rs`.
2. [ ] Add `MsgToUI::SelectorUpdate(SelectorSnapshot)` variant.
3. [ ] Add `MsgToUI::SelectorHide` variant.
4. [ ] Build `SelectorSnapshot` in the engine (do not attempt a cross-crate `From` impl).

### Stage 3: Nucleo Integration

1. [ ] Create `crates/hotki-engine/src/selector.rs` module.
2. [ ] Implement `SelectorMatcher` struct wrapping `Nucleo<SelectorCandidate>`:
   - Use 1 matching column; haystack is the label.
   - `notify` callback triggers egui repaint request.
3. [ ] Implement `SelectorMatcher::new(items: Vec<SelectorItem>, notify: impl Fn())`:
   - Create `Nucleo` with 1 column.
   - Assign stable `id`s and inject candidates via `Injector::push()`.
4. [ ] Implement `SelectorMatcher::update_pattern(&mut self, query: &str)`:
   - Call `pattern.reparse()` for column 0, using `append` when safe.
5. [ ] Implement `SelectorMatcher::tick(&mut self) -> Status`:
   - Call `nucleo.tick(10)` with 10ms timeout.
6. [ ] Implement `SelectorMatcher::matched_items(&self, range) -> impl Iterator<Item = MatchedItem>`:
   - Returns items from snapshot and computes match indices for visible items only.
7. [ ] Add unit tests for matching behavior:
   - Empty query returns all items in original order.
   - Exact prefix match ranks highest.
   - Substring matches work.
   - Case insensitivity (smart case).

### Stage 4: Selector State Management

1. [ ] Implement `SelectorState::new(config: SelectorConfig, notify: impl Fn()) -> Self`:
   - Creates `SelectorMatcher` with resolved items.
   - Initializes query, selected index, nav_mode.
2. [ ] Implement `SelectorState::handle_input(key: Key) -> SelectorEvent`:
   - Returns `SelectorEvent::Update` (state changed), `Select(usize)`, `Cancel`, or `None`.
   - On text input: update query, call `matcher.update_pattern()`.
3. [ ] Implement `SelectorState::tick(&mut self) -> bool`:
   - Calls `matcher.tick()`, returns true if snapshot changed.
4. [ ] Implement keyboard handling per specification (normal mode + vim nav mode).
5. [ ] Add unit tests for keyboard state machine.

### Stage 5: Engine Integration

1. [ ] In key dispatch, handle `BindingKind::Selector`:
   - Resolve items (evaluate provider if needed) and create `SelectorState`.
   - Enable capture-all while selector is active.
   - Send `MsgToUI::SelectorUpdate`.
2. [ ] Route key events to selector when active (before normal mode handling).
3. [ ] On `SelectorEvent::Select(idx)`:
   - Extract selected item.
   - Execute `on_select(ctx, item, query)` handler.
   - Close selector, send `MsgToUI::SelectorHide`.
4. [ ] On `SelectorEvent::Cancel`:
   - Execute `on_cancel` handler if present.
   - Close selector, send `MsgToUI::SelectorHide`.
5. [ ] On `SelectorEvent::Update`:
   - Send `MsgToUI::SelectorUpdate` with new snapshot.

### Stage 6: UI Implementation

1. [ ] Create `crates/hotki/src/selector.rs` module.
2. [ ] Implement `SelectorWindow` struct with:
   - `ViewportId` for the selector viewport.
   - `state: Option<SelectorSnapshot>` mirroring engine state.
3. [ ] Implement rendering:
   - Title bar with `title`.
   - Text input showing `query` with cursor.
   - Scrollable list of items with selection highlight.
   - Match character highlighting in labels.
4. [ ] Handle viewport positioning (centered on active display).
5. [ ] Apply HUD-style theming (transparent, blur, rounded corners).
6. [ ] Wire into `HotkiApp`:
   - Handle `MsgToUI::SelectorUpdate` to show/update.
   - Handle `MsgToUI::SelectorHide` to dismiss.
   - Render query/selection from snapshots (no local input state).

### Stage 7: DSL Registration

1. [ ] In `crates/config/src/dynamic/dsl.rs`, register:
   - `action.selector(config_map)` function (returns `SelectorConfig`).
   - `selector_item(label, data)` helper.
2. [ ] Implement config map parsing:
   - Parse `title`, `placeholder`, `items`, `on_select`, `on_cancel`, `max_visible`, `vim_mode`.
   - Accept `items` as either an array or a `|ctx| -> Array` provider.
   - Accept `on_select`/`on_cancel` as closures (wrap into `HandlerRef`).
   - Validate required fields, provide defaults.
3. [ ] Implement `SelectorItem` parsing from Rhai maps and strings.
4. [ ] Add DSL unit tests.
5. [ ] Update `ModeBuilder.bind(...)` parsing to accept `SelectorConfig` as the third element.

### Stage 8: Application Launcher Helper

1. [ ] Create `crates/config/src/dynamic/apps.rs` module.
2. [ ] Implement `scan_applications() -> Vec<SelectorItem>`:
   - Scan standard macOS application directories.
   - Extract app name from bundle.
   - Cache results (invalidate on config reload).
3. [ ] Register `get_applications()` function in Rhai DSL.
4. [ ] Add example configuration in `examples/selector.rhai`.

### Stage 9: Styling Integration

1. [ ] Add selector-specific style fields to `RawStyle`/`Style`:
   - `selector_bg`, `selector_input_bg`, `selector_item_bg`
   - `selector_item_selected_bg`, `selector_match_fg`
   - `selector_border`, `selector_shadow`
2. [ ] Apply theme styles in selector rendering.
3. [ ] Update built-in themes with selector colors.

### Stage 10: Documentation and Examples

1. [ ] Add selector documentation to `CONFIG.md`.
2. [ ] Add selector testers to `examples/test.rhai` (follow our standard entry chord `shift+cmd+0`).
3. [ ] Create `examples/selector.rhai` with app launcher example (activated via `shift+cmd+0`).
4. [ ] Create `examples/selector-custom.rhai` showing custom item lists (activated via `shift+cmd+0`).
5. [ ] Document keyboard shortcuts and vim mode.

### Stage 11: Validation

1. [ ] Run `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests --examples 2>&1`.
2. [ ] Run `cargo test --all`.
3. [ ] Run `cargo run --bin smoketest -- all`.
4. [ ] Run `cargo fmt --all`.
5. [ ] Manual testing:
   - App launcher with various search queries.
   - Keyboard navigation (arrows, vim mode).
   - Selection callback execution.
   - Cancel behavior.
   - Empty results handling.
   - Theme consistency.

---

## Future Extensibility (enabled by nucleo)

The choice of `nucleo` over `nucleo-matcher` is strategic. These future features require minimal
architectural changes:

**Dynamic item sources:**
```rust
// Future: items populated asynchronously
let injector = selector.matcher.injector();
tokio::spawn(async move {
    for entry in walkdir::WalkDir::new("/") {
        let item = SelectorItem {
            label: entry.path().display().to_string(),
            sublabel: None,
            data: entry.path().display().to_string().into(),
        };
        injector.push(SelectorCandidate { id: next_id(), item }, |cand, cols| {
            cols[0] = cand.item.label.as_str().into();
        });
    }
});
// UI updates live as items stream in
```

**Filesystem browser:**
- Inject directory entries on-demand as user navigates
- Results appear incrementally, never blocks
- Could add column for file type, size, etc.

**Command palette:**
- Inject commands from multiple sources (built-in, plugins, recent)
- Columns for command name, keybinding, source

---

## Open Questions

1. **Async item loading**: Should we support lazy/async item sources for large lists (e.g., file
   browser)? Recommendation: defer to future enhancement; start with synchronous items. The `nucleo`
   architecture (with `Injector`) is already designed for this - items can be streamed in from
   background tasks and results update live.

2. **Multi-select**: Should we support selecting multiple items? Recommendation: defer; single
   selection covers primary use cases.

3. **Item icons**: Should items support icons (e.g., app icons)? Recommendation: add `icon: Option<PathBuf>`
   to `SelectorItem` but defer rendering to a follow-up.

4. **Preview pane**: Should we show a preview of the selected item? Recommendation: defer; not
   needed for app launcher use case.

5. **Theming granularity**: How much selector styling should be configurable? Recommendation: start
   with basic colors matching HUD style; expand based on user feedback.
