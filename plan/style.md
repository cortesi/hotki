# Unified Styles and Themes Specification

## Executive Summary

This document specifies the unification of themes and styles in hotki. The key changes
are:

1. **Theme registry** — users can register, remove, and list themes; builtins pre-registered
2. **Unify theme and style types** — themes are Style objects stored in the registry
3. **Make `m.style()` merge** instead of replace, enabling incremental customization
4. **Introduce a Style object** with type constraints and utility methods
5. **Replace `base_theme()` with `theme("name")`** — sets active theme by registered name
6. **Remove global `style()`** — customizations go on modes via `m.style()`

## Current State Analysis

### Current Architecture

**Themes** (`themes/mod.rs`):
- 5 builtin themes: `default`, `charcoal`, `dark-blue`, `solarized-dark`, `solarized-light`
- Stored as `Style` objects in `OnceLock<HashMap<String, Style>>`
- Not directly accessible from Rhai scripts
- Selected via `base_theme("name")` global function

**Styles**:
- `Style` struct: fully resolved style with concrete values (`(u8, u8, u8)` for colors)
- `RawStyle` struct: partial style with optional fields (`Maybe<String>` for colors)
- Style overlay chain: theme → global_styles → mode_style → binding_style

**Style Application Points**:
1. `base_theme("name")` - sets which theme to load
2. `style(#{...})` - global style overlay (replaces, doesn't merge)
3. `m.style(#{...})` - mode-level style overlay
4. `binding.style(#{...})` or `binding.style(|ctx| ...)` - binding-level style

### Current Problems

1. **Themes not accessible**: Users cannot reference themes in scripts
2. **No style merging**: `style()` replaces the entire global style rather than merging
3. **Type disconnect**: Themes are `Style`, style() takes `RawStyle` (maps)
4. **Loose typing**: Style maps have no schema validation beyond field names

## Proposed Design

### 1. The Style Object

Introduce a `Style` Rhai type that wraps style data and provides methods for
manipulation. **Style objects are immutable**—all methods return a new Style
rather than modifying in place.

```rust
/// Rhai-exposed Style object wrapping partial style data.
#[derive(Clone)]
pub struct RhaiStyle {
    raw: RawStyle,
}
```

**Construction**:
```rhai
// From a theme
let s = themes.default_;

// From a map (validates structure)
let s = Style(#{ hud: #{ font_size: 18.0 } });

// Clone an existing style
let s2 = s.clone();
```

**Property getters** (read-only access to style data):
```rhai
// Section access - returns map
let hud = s.hud;           // #{ font_size: 14.0, bg: "#2d2d2d", ... }
let notify = s.notify;     // #{ timeout: 3.0, bg: "#2d2d2d", ... }

// Nested property access
let size = s.hud.font_size;      // 14.0
let bg = s.hud.bg;               // "#2d2d2d"
let timeout = s.notify.timeout;  // 3.0
```

**set() method** (returns new Style with path updated):
```rhai
// Set a single field
let s2 = s.set("hud.font_size", 18.0);
let s3 = s.set("notify.timeout", 5.0);

// Set a section (merges map into section)
let s2 = s.set("hud", #{ font_size: 18.0, opacity: 0.95 });

// Chainable
let s2 = themes.default_
  .set("hud.font_size", 18.0)
  .set("hud.opacity", 0.95)
  .set("notify.timeout", 5.0);
```

**merge() method** (full style merge):
```rhai
// Merge with another style
let merged = s.merge(other_style);

// Merge with a map
let merged = s.merge(#{ hud: #{ opacity: 0.9 } });
```

**Example: round-trip modification**:
```rhai
// Get a value, compute new value, set it
let s = themes.default_;
let s2 = s.set("hud.font_size", s.hud.font_size * 1.2);

// Copy property between themes
let accent = themes.solarized_dark.hud.key_bg;
let s2 = themes.charcoal.set("hud.key_bg", accent);
```

### 2. Theme Registry

Themes are managed through a registry. Builtin themes are pre-registered; users can
register, overwrite, and remove themes.

```rhai
// Query the registry
themes.list()              // ["charcoal", "dark-blue", "default", ...] (alphabetical)
themes.get("dark-blue")    // Style object (error if not found)

// Modify the registry
themes.register("my-dark", themes.default_.set("hud.bg", "#1a1a2e"));
themes.register("default", themes.default_.set("hud.font_size", 18.0));  // overwrite builtin
themes.remove("solarized-light");  // remove from registry

// Convenience accessors (read-only, hyphens become underscores)
themes.default_        // "default" (note: `default` is reserved in Rhai)
themes.charcoal        // equivalent to themes.get("charcoal")
themes.dark_blue       // equivalent to themes.get("dark-blue")
```

**Notes:**
- `themes.list()` returns names in alphabetical order
- `action.theme_next`/`action.theme_prev` cycle through registered themes alphabetically
- Style objects are immutable — `themes.get()` returns a value, not a reference
- Overwriting builtins is allowed
- Removing a theme that is currently active falls back to "default"

**Example: minimal theme set**
```rhai
// Remove themes you don't want in rotation
themes.remove("solarized-light");
themes.remove("solarized-dark");

// Register custom themes
themes.register("my-dark", themes.charcoal.set("hud.opacity", 0.95));

// Now action.theme_next cycles: charcoal, dark-blue, default, my-dark
```

Implementation:
- Theme registry stored in `DynamicConfigScriptState`
- Builtins registered at initialization
- `register(name, Style)` adds/overwrites entry
- `remove(name)` removes entry (error if "default" or not found?)
- `list()` returns sorted keys
- `get(name)` returns clone of Style

### 3. Style Layering

Styles are applied in layers:

```
theme layer      ← set via theme(), switched via action.theme_*
  └── root mode  ← m.style() customizations
        └── child modes  ← inherit parent, add own m.style()
```

**Theme layer**: The base style, set via `theme("name")` global function. The theme
must be registered. This is what `action.theme_next`, `action.theme_prev`, and
`action.theme_set` operate on.

```rhai
// Set a builtin theme
theme("dark-blue");

// Or register and use a custom theme
themes.register("my-dark", themes.default_.set("hud.font_size", 18.0));
theme("my-dark");
```

**Mode-level m.style()**: Customizations on top of the theme. Multiple calls merge
left-to-right within each mode:

```rhai
hotki.mode(|m, ctx| {
  // Customize on top of theme
  m.style(#{ hud: #{ opacity: 0.9 } });
  m.style(#{ notify: #{ timeout: 2.0 } });  // Merges with above
});
```

**Style inheritance**: Child modes inherit their parent's merged style, then apply
their own overlays on top:

```rhai
hotki.mode(|m, ctx| {
  m.style(#{ hud: #{ font_size: 18.0 } });

  m.mode("w", "Window", |sub, ctx| {
    // Inherits parent's font_size, adds opacity
    sub.style(#{ hud: #{ opacity: 0.8 } });
  });
});
```

### 4. Type Constraints via the Style Object

The `RhaiStyle` type provides schema validation:

```rust
impl RhaiStyle {
    /// Construct from a Rhai map, validating all fields
    pub fn from_map(map: Map) -> Result<Self, Box<EvalAltResult>> {
        // Deserialize via serde, catching unknown fields
        let raw: RawStyle = from_dynamic(&Dynamic::from_map(map))?;
        Ok(Self { raw })
    }

    /// Merge another style on top of this one
    pub fn merge(&self, other: &RhaiStyle) -> Self {
        Self {
            raw: self.raw.merge(&other.raw),
        }
    }
}
```

The existing `#[serde(deny_unknown_fields)]` on `RawStyle`, `RawHud`, and `RawNotify`
already provides field validation. The Style object surfaces these errors clearly.

### 5. Simplified API Surface

After this change, the style API becomes:

| Context | Method | Behavior |
|---------|--------|----------|
| Global | `theme("name")` | Set active theme by name |
| Global | `themes.register("name", Style)` | Add/overwrite theme in registry |
| Global | `themes.remove("name")` | Remove theme from registry |
| Global | `themes.list()` | Get registered theme names (alphabetical) |
| Global | `themes.get("name")` | Get Style by name |
| Mode | `m.style(style_or_map)` | Merge into mode-level style |
| Binding | `binding.style(map)` | Set binding row colors |
| Binding | `binding.style(\|ctx\| map)` | Dynamic binding row colors |

**Removed**:
- `base_theme("name")` — use `theme("name")` instead
- `style(...)` global function — use `m.style(...)` on modes

### 6. Example Configurations

**Before** (current):
```rhai
base_theme("dark-blue");
style(#{
  hud: #{ font_size: 18.0 },
});

hotki.mode(|m, ctx| {
  m.mode("w", "Window", |sub, ctx| {
    // No easy way to switch theme just for this mode
  }).style(#{ hud: #{ mode: mini } });
});
```

**After** (proposed):
```rhai
// Set base theme (switchable via action.theme_*)
theme("dark-blue");

hotki.mode(|m, ctx| {
  // Customizations on top of theme
  m.style(#{ hud: #{ font_size: 18.0 } });

  m.mode("w", "Window", |sub, ctx| {
    sub.style(#{ hud: #{ mode: mini } });
  });
});
```

**Power user** (custom theme registry):
```rhai
// Register a custom theme
themes.register("my-dark", themes.default_
  .set("hud.bg", "#1a1a2e")
  .set("hud.key_bg", "#16213e")
  .set("notify.timeout", 2.0));

// Remove themes you don't want in rotation
themes.remove("solarized-light");
themes.remove("solarized-dark");

// Set the custom theme
theme("my-dark");

hotki.mode(|m, ctx| {
  m.mode("a", "App Mode", |sub, ctx| {
    // Mode-specific customization on top of theme
    sub.style(#{ hud: #{ opacity: 0.85 } });
  });
});
```

---

## Implementation Plan

### Stage 1: Clean Up Legacy Code and Add Merging Infrastructure

**Goal**: Remove legacy `user_style` and `style()`, replace `base_theme()` with `theme()`.
Add merging support to `RawStyle` for use by mode-level styling.

**Note**: This stage removes `user_style`, `style()`, and all supporting code (the `enabled`
flag pattern, single-style replacement semantics). The theme layer is preserved but
accessed via `theme()` which accepts Style objects only.

- [x] Add `RawStyle::merge(&self, other: &RawStyle) -> RawStyle` method
- [x] Add `merge_maybe<T>()` helper for combining nested `Maybe<T>` fields
- [x] Add `RawHud::merge()` with field-level merging
- [x] Add `RawNotify::merge()` with field-level merging
- [x] Remove `user_style` field from `DynamicConfigScriptState`
- [x] Remove `style()` global function registration
- [x] Remove `base_theme()` function registration
- [x] Remove `user_style_enabled` flag and related code paths
- [x] Add tests for RawStyle merging (empty+empty, value+empty, nested override)

---

### Stage 2: Introduce RhaiStyle Type

**Goal**: Create a Rhai-exposed Style type with property getters, `set()`, and `merge()`.

- [x] Add `RhaiStyle` struct wrapping `RawStyle`
- [x] Implement `RhaiStyle::from_raw()` constructor
- [x] Register `RhaiStyle` as Rhai type named "Style"
- [x] Register `Style(map)` constructor function
- [x] Register `clone()` method
- [x] Register property getters for `hud` and `notify` (return maps)
- [x] Register `set("path", value)` method (single field or section map)
- [x] Register `merge(Style)` and `merge(map)` methods
- [x] Add tests for Style() constructor, property getters, set(), merge()

---

### Stage 3: Add Theme Registry and theme() Function

**Goal**: Implement the theme registry with register/remove/list/get methods, and add
the `theme("name")` global function.

- [x] Add `Style::to_raw(&self) -> RawStyle` method
- [x] Create theme registry (`HashMap<String, RawStyle>`) in `DynamicConfigScriptState`
- [x] Pre-populate registry with builtin themes at initialization
- [x] Create `ThemesNamespace` struct
- [x] Register `themes.get(name)` returning Style (error if not found)
- [x] Register `themes.list()` returning sorted array of theme names
- [x] Register `themes.register(name, Style)` to add/overwrite themes
- [x] Register `themes.remove(name)` to remove themes (error if "default"?)
- [x] Register convenience getters (`themes.default_`, `themes.charcoal`, etc.)
- [x] Expose `themes` as global variable
- [x] Register `theme("name")` global function to set active theme by name
- [x] Store active theme name in `DynamicConfigScriptState`
- [x] Update `DynamicConfig::base_style()` to look up theme from registry
- [x] Handle removal of active theme (fallback to "default")
- [x] Add tests for registry operations
- [x] Add tests for `theme("name")` setting base theme

---

### Stage 4: Mode-Level Style Merging

**Goal**: Support multiple `m.style()` calls per mode that merge together.

- [x] Change `ModeBuildState::style` from `Option<StyleOverlay>` to `styles: Vec<RawStyle>`
- [x] Update `ModeBuilder::finish()` to merge accumulated styles
- [x] Update `m.style(map)` to push instead of replace
- [x] Add `m.style(RhaiStyle)` overload
- [x] Add tests for multiple m.style() calls merging
- [x] Add tests for mode inheriting and extending parent style

---

### Stage 5: Update Examples and Documentation

**Goal**: Update all examples to use the new `theme()` and `m.style()` API.

- [x] Update `examples/complete.rhai` with new style API
- [x] Update `examples/cortesi.rhai` with new style API
- [x] Verify all examples parse without error
- [x] Run full smoketest suite

---

## Migration Guide

### For Users

**Before** (removed):
```rhai
base_theme("dark-blue");
style(#{ hud: #{ font_size: 18.0 } });

hotki.mode(|m, ctx| {
  // ...
});
```

**After**:
```rhai
theme("dark-blue");

hotki.mode(|m, ctx| {
  m.style(#{ hud: #{ font_size: 18.0 } });
  // ...
});
```

Or with a custom registered theme:
```rhai
themes.register("my-dark", themes.dark_blue.set("hud.font_size", 18.0));
theme("my-dark");

hotki.mode(|m, ctx| {
  // ...
});
```

### Behavior Changes

1. **`base_theme("name")` is replaced by `theme("name")`**. The API is similar but
   `theme()` requires the theme to be registered (builtins are pre-registered).

2. **`style()` global function is removed**. Use `m.style(...)` on modes for customizations.
   The theme layer is set via `theme()`, mode customizations via `m.style()`.

3. **Theme registry is user-controllable**. Use `themes.register()` to add custom themes,
   `themes.remove()` to remove themes from rotation, including builtins.

4. **Multiple `m.style()` calls now merge** instead of replacing. If you relied on
   replacement behavior, use a single `m.style()` call with all fields.

5. **Theme names with hyphens** are accessed with underscores via convenience getters:
   `themes.dark_blue` not `themes.dark-blue`. Use `themes.get("dark-blue")` for the
   original name, or just use `theme("dark-blue")` directly.

---

## Testing Strategy

### Unit Tests

1. **RawStyle merging** (`test_merge.rs`):
   - Empty + Empty = Empty
   - Value + Empty = Value
   - Empty + Value = Value
   - Value + Value = Later Value (override)
   - Nested field merging

2. **RhaiStyle operations** (`test_dynamic.rs`):
   - Construction from map
   - Clone operation
   - Merge with Style
   - Merge with map
   - Property getters (s.hud, s.hud.font_size, etc.)
   - set("path", value) for fields and sections

3. **Theme registry and theme()** (`test_dynamic.rs`):
   - Builtins pre-registered and accessible
   - `themes.get()` works with valid names
   - `themes.get()` errors on invalid theme name
   - `themes.list()` returns sorted theme names
   - `themes.register()` adds/overwrites themes
   - `themes.remove()` removes themes
   - `theme("name")` sets active theme
   - `theme("invalid")` errors on unregistered name
   - Removing active theme falls back to "default"
   - `action.theme_*` cycles through registered themes alphabetically

4. **Mode style merging** (`test_dynamic.rs`):
   - Multiple m.style() calls merge
   - Mode inherits and extends parent style
   - Mode styles layer on top of theme

### Integration Tests

1. **Smoketest** (`cargo run --bin smoketest -- all`):
   - All UI scenarios with new style API
   - Theme switching works
   - Mode-specific themes work

2. **Example configs**:
   - All examples parse without error
   - Visual verification of styling

---

## Open Questions

1. **Should binding.style() also accept Style objects?**
   Current binding styles are a subset (just colors). Could extend in future.
   Defer to keep scope manageable.
