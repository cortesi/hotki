# Hotki Configuration Reference

Configuration files are [Rhai](https://rhai.rs/) scripts, typically at
`~/.hotki/config.rhai`.

## File Resolution

1. `--config <path>` if provided
2. `~/.hotki/config.rhai` if it exists
3. Error with hint to copy `examples/complete.rhai`

Validate without starting UI:

```bash
hotki check --config /path/to/config.rhai
hotki check  # uses default resolution
```

## Imports

```rust
import "foo"  // loads foo.rhai relative to config directory
```

- Paths must be relative (no absolute paths, no `..` segments)
- Symlinks outside config directory are rejected

## Entry Point

The `global` variable is a `Mode` representing the root. All bindings are
created from this.

## Mode Methods

| Method | Description |
|--------|-------------|
| `mode.bind(chord, desc, action)` | Create a leaf binding |
| `mode.mode(chord, desc, \|m\| { ... })` | Create a sub-mode |

Chord strings: `shift+cmd+0`, `cmd+c`, `esc`, etc.

## Binding Modifiers

**Valid on both `.bind()` and `.mode()`:**

| Modifier | Description |
|----------|-------------|
| `.global()` | Active in this mode and all sub-modes |
| `.hidden()` | Works but hidden from HUD |
| `.hud_only()` | Only activates while HUD is visible |
| `.match_app(pattern)` | Regex filter by focused app name |
| `.match_title(pattern)` | Regex filter by focused window title |

**Only valid on `.bind()`:**

| Modifier | Description |
|----------|-------------|
| `.no_exit()` | Stay in current mode after action |
| `.repeat()` | Enable hold-to-repeat with defaults |
| `.repeat_ms(delay, interval)` | Hold-to-repeat with custom timing (ms) |

**Only valid on `.mode()`:**

| Modifier | Description |
|----------|-------------|
| `.capture()` | Capture all unbound keys in this mode |
| `.style(map)` | Per-mode style overlay |

Duplicate chords within a mode require `match_app` or `match_title` guards.

## Actions

All actions are in the `action` namespace.

### Shell Commands

```rust
action.shell("echo hello")
action.shell("make build").notify(success, error)
action.shell("echo quiet").silent()
```

| Method | Description |
|--------|-------------|
| `.notify(ok, err)` | Notification on success/failure |
| `.silent()` | Suppress all notifications |

### Key Relay

```rust
action.relay("cmd+c")
action.relay("shift+tab")
```

### Mode Navigation

| Action | Description |
|--------|-------------|
| `action.pop` | Return to previous mode |
| `action.exit` | Exit to root |

### Volume Control

| Action | Description |
|--------|-------------|
| `action.set_volume(level)` | Set absolute volume (0–100) |
| `action.change_volume(delta)` | Adjust by delta (-100 to +100) |
| `action.mute(toggle)` | Control mute state |

### Theme Control

| Action | Description |
|--------|-------------|
| `action.theme_next` | Next theme |
| `action.theme_prev` | Previous theme |
| `action.theme_set(name)` | Set theme by name |

Themes: `default`, `charcoal`, `dark-blue`, `solarized-dark`, `solarized-light`

### UI Control

| Action | Description |
|--------|-------------|
| `action.show_details(toggle)` | Control details window |
| `action.show_hud_root` | Display root-level HUD |
| `action.user_style(toggle)` | Enable/disable user style overlay |
| `action.clear_notifications` | Clear notifications |
| `action.reload_config` | Reload configuration |

### Action Fluent Methods

| Method | Description |
|--------|-------------|
| `action.clone()` | Clone an action (all actions are immutable) |

## Script Actions

Bind a closure instead of an `Action`. Must return an `Action` or `[Action, ...]`.

```rust
// Zero-argument closure
m.bind("p", "Play", || action.shell("spotify pause"));

// Context-aware closure
m.bind("o", "Open", |ctx| {
  if ctx.app.contains("Safari") {
    action.shell("open ~/logs/safari.log")
  } else {
    action.shell("open ~/logs/system.log")
  }
});

// Macro: array of actions executed in sequence
m.bind("s", "Save+Beep", || [
  action.relay("cmd+s"),
  action.shell("afplay /System/Library/Sounds/Pop.aiff").silent(),
]);
```

### ActionCtx Properties

| Property | Type | Description |
|----------|------|-------------|
| `ctx.app` | String | Focused app name |
| `ctx.title` | String | Focused window title |
| `ctx.pid` | i64 | Focused app PID |
| `ctx.depth` | i64 | Current mode depth (0 = root) |
| `ctx.path` | Array | Cursor indices from root |

Script actions are sandboxed with conservative limits. I/O is only possible via
built-in action constructors.

## Top-Level Functions

| Function | Description |
|----------|-------------|
| `base_theme(name)` | Set the base theme |
| `style(map)` | Set user style overlay |
| `server(map)` | Set server tunables |
| `env(var)` | Get environment variable (empty string if unset) |

## Constants

### Toggle Values

| Value | Description |
|-------|-------------|
| `on` | Enable |
| `off` | Disable |
| `toggle` | Flip current state |

### Notification Kinds

`ignore`, `info`, `warn`, `error`, `success`

### HUD Positions

`center`, `n`, `ne`, `e`, `se`, `s`, `sw`, `w`, `nw`

### Notification Positions

`left`, `right`

### HUD Display Modes

`hud_full`, `hud_mini`, `hud_hide`

### Font Weights

`thin`, `extralight`, `light`, `regular`, `medium`, `semibold`, `bold`,
`extrabold`, `black`
