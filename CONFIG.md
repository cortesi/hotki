# Hotki Configuration Reference

Configuration files are [Rhai](https://rhai.rs/) scripts. Hotki looks for `~/.hotki/config.rhai`
by default, or use `--config <path>` to specify an alternative. Validate your config without
starting the UI with:

```bash
hotki check --config ~/.hotki/config.rhai
hotki check # uses the default resolution policy
```

Split configuration across multiple files using `import "foo"` to load `foo.rhai` relative to
your config directory. Import paths must be relative with no `..` segments, and symlinks outside
the config directory are rejected.

---

## Introduction: Modes, Closures, and Re-rendering

Hotki treats your configuration as a **program** that renders menus on demand.

### Modes are closures

A “mode” is a Rhai closure with the shape:

```rhai
|m, ctx| { ... }
```

When Hotki needs to know “what keys are active right now?”, it **executes the active mode
closures** to produce a list of bindings. There is no static, precomputed tree: your closure
logic (including `if` statements) decides what exists.

### The mode stack

At runtime, Hotki keeps a **stack of modes**:

- The **root** mode is always at the bottom of the stack.
- Entering a mode (a `m.mode(...)` binding) pushes a new mode closure onto the stack.
- `ctx.depth` is the current stack depth (0 at root, 1 in the first child mode, etc).

### Re-rendering when state changes

Mode closures are re-evaluated frequently. In the current implementation, Hotki re-renders the
stack (and rebinds OS hotkeys / updates the HUD) at least when:

- Focus context changes (`ctx.app`, `ctx.title`, `ctx.pid`).
- A bound key is pressed (after actions/handlers run).
- The config is loaded/reloaded.
- Theme state changes (e.g. `action.theme_*`).
- HUD visibility changes (`ctx.hud`) via navigation actions (`action.show_root`, `action.hide_hud`,
  `action.pop`, `action.exit`, and auto-exit).

Because render closures can run often, they should stay lightweight and side-effect-free; use
`action.*` for effects.

### Auto-pop and orphaning

While rendering, Hotki may adjust the stack to keep it consistent with what your closures render:

- A **non-root** mode that renders **zero bindings** is automatically popped.
- A mode entered via a chord is popped if its entry binding disappears or now points at a
  different mode closure (this is what makes `if/else`-selected submenus work reliably).

---

## Entry Point

Every config must register a single root mode:

```rhai
hotki.mode(|m, ctx| {
  // root bindings
});
```

- `m` is a `ModeBuilder` used to declare bindings and sub-modes.
- `ctx` is a `ModeCtx` (focused app/title, HUD visibility, stack depth).

Modes are **re-rendered** whenever context changes (focus/title, HUD visibility, theme, etc), so
use `if` statements for conditional bindings.

---

## Global Functions

| Function | Parameters | Description |
|----------|------------|-------------|
| `theme(name)` | `String` | Set the active theme by registered name |
| `Style(map)` | `Map` | Construct a `Style` object (validates the map) |
| `selector_item(label, data)` | `String, Dynamic` | Build a selector item map (`#{ label, sublabel, data }`) |
| `get_applications()` | – | Return selector items for installed applications |

Themes are stored in a registry exposed as the global `themes` variable.

### Theme Registry (`themes`)

Built-in themes are embedded at compile time and pre-registered. If your config file is
`/path/to/config.rhai`, Hotki will also load `*.rhai` theme files from `/path/to/themes/` before
evaluating the config script, and these can override built-ins by name.

You can add, overwrite, list, and remove themes:

```rhai
themes.list()                 // ["charcoal", "dark-blue", "default", ...] (sorted)
themes.get("dark-blue")       // Style (error if missing)
themes.register("my", themes.default_.set("hud.opacity", 0.9))
themes.remove("solarized-light") // cannot remove "default"
```

Convenience getters are provided for built-ins (hyphens become underscores):
Note that `default` is a reserved keyword in Rhai, so the default theme getter is `default_`.

```rhai
themes.default_        // "default"
themes.dark_blue       // "dark-blue"
themes.solarized_dark  // "solarized-dark"
```

### Theme Files (`themes/*.rhai`)

Theme files are plain Rhai scripts that must evaluate to a single map with `hud` and/or `notify`
sections (the same shape you would pass to `m.style(#{ ... })`).

Theme names come from the filename stem, e.g. `dark-blue.rhai` → `"dark-blue"`.

#### Troubleshooting

- **Where are themes loaded from?** From `themes/` next to your `config.rhai` (defaults to
  `~/.hotki/themes/`), plus the embedded built-ins.
- **Which one wins?** Theme files override embedded built-ins by name; `themes.register(...)` in
  `config.rhai` can override both.
- **Common mistakes:** forgetting to return a final `#{ ... }` map, using unknown fields (schema is
  strict), or using integers where `*.0` floats are required.

---

## ModeBuilder (`m`)

### Bindings

#### Single binding

```rhai
m.bind(chord, desc, target) -> BindingRef
```

Where `target` is one of:
- `action.*` (primitive action or compound action via `action.run`)
- `|m, ctx| { ... }` (a child mode closure)
- `action.selector(#{ ... })` (interactive selector popup)

#### Batch bindings

```rhai
m.bind(array) -> BindingsRef
```

Pass an array of `[chord, desc, target]` tuples to create multiple bindings at once. Each `target`
can be an action, `action.run` (compound action), a selector config, or a mode closure:

```rhai
m.bind([
  ["c", "Clear", action.clear_notifications],
  ["r", "Reload", action.reload_config],
  ["s", "Settings", |sub, ctx| {
    sub.bind([
      ["t", "Theme", action.theme_next],
    ]);
  }],
]).stay();  // modifiers apply to all bindings
```

### Modes

#### Closure form

```rhai
m.mode(chord, title, |m, ctx| { ... }) -> BindingRef
```

Shorthand for `m.bind(chord, title, |m, ctx| { ... })`.

#### Inline bindings form

```rhai
m.mode(chord, title, array) -> BindingRef
```

For simple modes that only contain a flat list of bindings, pass an array directly instead of a
closure. Each element can be an action, `action.run` (compound action), or a nested mode closure:

```rhai
m.mode("h", "Hotki", [
  ["c", "Clear", action.clear_notifications],
  ["r", "Reload", action.reload_config],
  ["s", "Settings", |sub, ctx| {
    // nested mode with its own logic
    sub.bind("t", "Theme", action.theme_next);
  }],
]);
```

This is equivalent to:

```rhai
m.mode("h", "Hotki", |sub, ctx| {
  sub.bind([
    ["c", "Clear", action.clear_notifications],
    ["r", "Reload", action.reload_config],
    ["s", "Settings", |sub, ctx| { ... }],
  ]);
});
```

### Mode-Level Modifiers (inside a mode closure)

```rhai
m.capture();            // swallow unbound keys while HUD is visible
m.style(#{ ... });      // merge map into this mode's style (inherited by children)
m.style(Style(#{ ... })); // merge a Style object into this mode's style
```

---

## Binding Modifiers

### BindingRef (single binding)

Returned by `m.bind(chord, desc, target)` and `m.mode(chord, title, ...)`. All modifiers return
`BindingRef` for chaining.

| Modifier | Valid On | Description |
|----------|----------|-------------|
| `.hidden()` | bindings + mode entries | Active but hidden from HUD |
| `.stay()` | bindings + compound actions | Suppress auto-exit after execution |
| `.repeat()` | bindings | Hold-to-repeat (shell/relay/volume only) |
| `.repeat_ms(delay, interval)` | bindings | Repeat with custom timings (ms) |
| `.global()` | bindings | Inherit into child modes (not allowed on mode entries) |
| `.style(map)` | bindings + mode entries | Per-binding HUD row style override |
| `.capture()` | mode entries | Enable capture-all in the entered mode |

### BindingsRef (batch bindings)

Returned by `m.bind(array)`. Supports the same modifiers as `BindingRef`, applied to all bindings
in the batch:

```rhai
m.bind([
  ["h", "Left", action.relay("left")],
  ["l", "Right", action.relay("right")],
]).stay().global();  // both bindings get stay + global
```

Notes:
- `.global()` is rejected on mode entries to keep orphan detection simple.
- `.repeat()`/`.repeat_ms()` implicitly set `.stay()`.
- There is no built-in `.hud_only()`; use `if ctx.hud { ... }` in the render closure.

---

## Actions (`action.*`)

### Shell

```rhai
action.shell("echo hello")
action.shell("make build").notify(success, error)
action.shell("echo quiet").silent()
```

### Relay

```rhai
action.relay("cmd+c")
action.relay("shift+tab")
```

### Navigation

| Action | Description |
|--------|-------------|
| `action.pop` | Pop one mode frame (if now at root, hide HUD) |
| `action.exit` | Clear stack to root and hide HUD |
| `action.show_root` | Clear stack to root and show HUD |
| `action.hide_hud` | Hide HUD (keep stack position) |

### Selector (interactive)

`action.selector(#{ ... })` builds a selector popup binding. When triggered, Hotki:
- Hides the current HUD
- Suspends the current mode bindings
- Captures all keyboard input until the selector is dismissed

Keyboard shortcuts while the selector is open:

| Key | Action |
|-----|--------|
| Printable keys | Append to query |
| Backspace | Delete last character |
| Up / Ctrl-P | Move selection up |
| Down / Ctrl-N | Move selection down |
| Enter | Confirm selection |
| Escape | Cancel |
| Ctrl-U | Clear query |

Config map fields:
- `title` (String, default `""`)
- `placeholder` (String, default `"Search..."`)
- `items` (Array or closure returning Array)
- `on_select` (closure): `|ctx, item, query| { ... }`
- `on_cancel` (optional closure): `|ctx| { ... }`
- `max_visible` (Int, default `10`)

Selector items can be:
- A string (shorthand): `"Safari"` → `#{ label: "Safari", data: "Safari" }`
- A map: `#{ label: "...", sublabel: "...", data: ... }`

Built-in helpers:
- `get_applications()` returns an array of selector item maps for standard macOS app directories.

Example:

```rhai
m.bind("shift+cmd+0", "Run Application", action.selector(#{
  title: "Run Application",
  placeholder: "Search apps...",
  items: || get_applications(),
  on_select: |ctx, item, _query| {
    ctx.exec(action.shell("open '" + item.data + "'"))
  },
}));
```

### Config / UI

| Action | Description |
|--------|-------------|
| `action.reload_config` | Reload configuration |
| `action.clear_notifications` | Dismiss all notifications |
| `action.show_details(toggle)` | Control details window |

### Themes

| Action | Description |
|--------|-------------|
| `action.theme_next` | Next theme (cycles `themes.list()` order) |
| `action.theme_prev` | Previous theme (cycles `themes.list()` order) |
| `action.theme_set(name)` | Set theme by name |

### Volume

| Action | Parameters | Description |
|--------|------------|-------------|
| `action.set_volume(level)` | `0..=100` | Set absolute volume |
| `action.change_volume(delta)` | `-100..=100` | Adjust volume by delta |
| `action.mute(toggle)` | `on/off/toggle` | Control mute state |

---

## Compound Actions (`action.run`)

Use `action.run` for compound actions (multiple effects, logic, conditional dispatch).

```rhai
m.bind("x", "Complex", action.run(|ctx| {
  ctx.exec(action.shell("echo hello").silent());
  ctx.notify(success, "Done", "Completed");
  ctx.stay(); // suppress auto-exit
}));
```

`ActionCtx` fields:
- `ctx.app`, `ctx.title`, `ctx.pid`
- `ctx.hud` (bool), `ctx.depth` (stack depth)

`ActionCtx` methods:
- `ctx.exec(action)`
- `ctx.notify(kind, title, body)`
- `ctx.stay()`
- `ctx.push(mode_closure, title?)`
- `ctx.pop()`, `ctx.exit()`, `ctx.show_root()`

---

## Render Context (`ModeCtx`)

Available in mode closures as `ctx`:
- `ctx.app`, `ctx.title`, `ctx.pid`
- `ctx.hud` (bool), `ctx.depth` (stack depth)

String helpers:

```rhai
if ctx.app.matches("Safari|Chrome") { ... }
if ctx.title.matches(".*\\.md$") { ... }
```

---

## Constants

### Toggle

`on`, `off`, `toggle`

### NotifyKind

`ignore`, `info`, `warn`, `error`, `success`

### Positions

HUD: `center`, `n`, `ne`, `e`, `se`, `s`, `sw`, `w`, `nw`  
Notifications: `left`, `right`

### HUD Mode

`hud`, `mini`, `hide`

### FontWeight

`thin`, `light`, `regular`, `medium`, `semibold`, `bold`, `extrabold`, `black`

---

## Behavior Notes

- **Auto-exit**: after executing an action, Hotki clears to root + hides HUD unless the
  binding (or compound action via `ctx.stay()`) requests `.stay()`.
- **Duplicate chords**: within a single rendered mode, the first binding wins and later duplicates
  are ignored with a warning; use `if/else` for mutually exclusive chord assignments.
