# Hotki Configuration Reference

Configuration files are [Rhai](https://rhai.rs/) scripts. Hotki looks for
`~/.hotki/config.rhai` by default, or use `--config <path>` to specify an
alternative. Validate your config without starting the UI with `hotki check`.

Split configuration across multiple files using `import "foo"` to load
`foo.rhai` relative to your config directory. Import paths must be relative
with no `..` segments, and symlinks outside the config directory are rejected.

The entry point is the `global` variable, a [`Mode`](#mode) representing the root
from which all bindings are created.

## API

- [Global Functions](#global-functions)
- [Binding Modifiers](#binding-modifiers)
- [Actions](#actions)
  - [Shell Commands](#shell-commands)
  - [Key Relay](#key-relay)
  - [Mode Navigation](#mode-navigation)
  - [Volume Control](#volume-control)
  - [Theme Control](#theme-control)
  - [UI Control](#ui-control)
- [Constants](#constants)
- [Types](#types)

### Global Functions

| Function | Parameters | Returns | Description |
|----------|------------|---------|-------------|
| `base_theme(name)` | `String` | — | Set the base theme |
| `style(map)` | `Map` | — | Set user style overlay |
| `server(map)` | `Map` | — | Set server tunables |
| `env(var)` | `String` | `String` | Get environment variable (empty string if unset) |

#### base_theme

```rust
base_theme("charcoal");
```

Themes: `default`, `charcoal`, `dark-blue`, `solarized-dark`, `solarized-light`

#### style

```rust
style(#{
  hud: #{ pos: ne, bg: "#1a1a1a", opacity: 0.95 },
  notify: #{ pos: right, timeout: 3.5 },
});
```

Position values: [`Pos`](#pos) for HUD, [`NotifyPos`](#notifypos) for notifications.
See `examples/complete.rhai` for all style options.

#### server

```rust
server(#{
  exit_if_no_clients: true,  // Auto-shutdown when no UI clients connected
});
```

#### env

```rust
let home = env("HOME");
action.shell(`open ${env("HOME")}/Documents`)
```

### Binding Modifiers

All modifiers return [`Binding`](#binding) for chaining.

**Valid on both `.bind()` and `.mode()`:**

| Modifier | Parameters | Description |
|----------|------------|-------------|
| `.global()` | — | Active in this mode and all sub-modes |
| `.hidden()` | — | Works but hidden from HUD |
| `.hud_only()` | — | Only activates while HUD is visible |
| `.match_app(pattern)` | `String` | Regex filter by focused app name |
| `.match_title(pattern)` | `String` | Regex filter by focused window title |

**Only valid on `.bind()`:**

| Modifier | Parameters | Description |
|----------|------------|-------------|
| `.no_exit()` | — | Stay in current mode after action |
| `.repeat()` | — | Enable hold-to-repeat with defaults |
| `.repeat_ms(delay, interval)` | `i64`, `i64` | Hold-to-repeat with custom timing (ms) |

**Only valid on `.mode()`:**

| Modifier | Parameters | Description |
|----------|------------|-------------|
| `.capture()` | — | Capture all unbound keys in this mode |
| `.style(map)` | `Map` | Per-mode style overlay |

Duplicate chords within a mode require `match_app` or `match_title` guards.

### Actions

Actions are created via the `action` namespace. Bindings accept either a bare
[`Action`](#action) or a closure that returns an [`Action`](#action) (or array of
actions). Closures with a parameter receive an [`ActionCtx`](#actionctx). Script
closures are sandboxed; I/O is only possible via action constructors.

```rust
m.bind("c", "Copy", action.relay("cmd+c"));                    // bare action
m.bind("p", "Play", || action.shell("spotify pause"));         // closure
m.bind("o", "Open", |ctx| {                                    // closure with context
  if ctx.app.contains("Safari") { action.shell("open ~/safari.log") }
  else { action.shell("open ~/system.log") }
});
m.bind("s", "Save+Beep", || [                                  // action sequence
  action.relay("cmd+s"),
  action.shell("afplay /System/Library/Sounds/Pop.aiff").silent(),
]);
```

#### Shell Commands

```rust
action.shell("echo hello")
action.shell("make build").notify(success, error)
action.shell("echo quiet").silent()
```

| Method | Parameters | Description |
|--------|------------|-------------|
| `action.shell(cmd)` | `String` | Execute shell command |
| `.notify(ok, err)` | [`NotifyKind`](#notifykind), [`NotifyKind`](#notifykind) | Notification on success/failure |
| `.silent()` | — | Suppress all notifications |

#### Key Relay

```rust
action.relay("cmd+c")
action.relay("shift+tab")
```

| Method | Parameters | Description |
|--------|------------|-------------|
| `action.relay(chord)` | `String` | Send keystroke to active app |

#### Mode Navigation

| Action | Parameters | Description |
|--------|------------|-------------|
| `action.pop` | — | Return to previous mode |
| `action.exit` | — | Exit to root |

#### Volume Control

| Action | Parameters | Description |
|--------|------------|-------------|
| `action.set_volume(level)` | `i64` (0–100) | Set absolute volume |
| `action.change_volume(delta)` | `i64` (-100–+100) | Adjust by delta |
| `action.mute(toggle)` | [`Toggle`](#toggle) | Control mute state |

#### Theme Control

| Action | Parameters | Description |
|--------|------------|-------------|
| `action.theme_next` | — | Next theme |
| `action.theme_prev` | — | Previous theme |
| `action.theme_set(name)` | `String` | Set theme by name |

Themes: `default`, `charcoal`, `dark-blue`, `solarized-dark`, `solarized-light`

#### UI Control

| Action | Parameters | Description |
|--------|------------|-------------|
| `action.show_details(toggle)` | [`Toggle`](#toggle) | Control details window |
| `action.show_hud_root` | — | Display root-level HUD |
| `action.user_style(toggle)` | [`Toggle`](#toggle) | Enable/disable user style overlay |
| `action.clear_notifications` | — | Clear notifications |
| `action.reload_config` | — | Reload configuration |

#### Action Fluent Methods

| Method | Parameters | Returns | Description |
|--------|------------|---------|-------------|
| `.clone()` | — | [`Action`](#action) | Clone an action (all actions are immutable) |

### Constants

#### Toggle

| Value | Description |
|-------|-------------|
| `on` | Enable |
| `off` | Disable |
| `toggle` | Flip current state |

#### NotifyKind

`ignore`, `info`, `warn`, `error`, `success`

#### Pos

HUD position: `center`, `n`, `ne`, `e`, `se`, `s`, `sw`, `w`, `nw`

#### NotifyPos

Notification position: `left`, `right`

#### HudMode

`hud_full`, `hud_mini`, `hud_hide`

#### FontWeight

`thin`, `extralight`, `light`, `regular`, `medium`, `semibold`, `bold`,
`extrabold`, `black`

### Types

#### Mode

A container for key bindings. The `global` variable is the root `Mode`. Sub-modes
are created via `mode.mode()`.

| Method | Parameters | Returns | Description |
|--------|------------|---------|-------------|
| `.bind(chord, desc, action)` | `String`, `String`, [`Action`](#action) | [`Binding`](#binding) | Create a leaf binding |
| `.mode(chord, desc, \|m\| { ... })` | `String`, `String`, `Fn(Mode)` | [`Binding`](#binding) | Create a sub-mode |

Chord strings: `shift+cmd+0`, `cmd+c`, `esc`, etc.

#### Binding

Returned by `Mode.bind()` and `Mode.mode()`. Supports chained modifiers—see
[Binding Modifiers](#binding-modifiers).

#### Action

An executable action created via the `action` namespace. Immutable; use
`.clone()` to duplicate. See [Actions](#actions) for all constructors.

#### ActionCtx

Context passed to script action closures that take a parameter.

| Property | Type | Description |
|----------|------|-------------|
| `ctx.app` | `String` | Focused app name |
| `ctx.title` | `String` | Focused window title |
| `ctx.pid` | `i64` | Focused app PID |
| `ctx.depth` | `i64` | Current mode depth (0 = root) |
| `ctx.path` | `Array` | Cursor indices from root |
