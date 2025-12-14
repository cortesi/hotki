# Hotki Configuration Reference

Hotki configuration files are written in [Rhai](https://rhai.rs/), a simple
scripting language. Configuration files typically use the `.rhai` extension
and are loaded from `~/.hotki/config.rhai` by default.

## Actions

Actions are operations that hotki executes when a bound key is pressed. All
actions are accessed through the `action` namespace.

### Shell Commands

Execute a shell command.

```rhai
action.shell("echo hello")
```

**Modifiers:**

- `.notify(ok_kind, err_kind)` - Display notifications based on command
  success/failure. Kinds: `ignore`, `info`, `warn`, `error`, `success`.
- `.silent()` - Suppress all notifications (equivalent to `.notify(ignore, ignore)`).

```rhai
// Show success notification on success, error notification on failure
action.shell("make build").notify(success, error)

// Never show notifications
action.shell("echo quiet").silent()
```

### Key Relay

Send keystrokes to the focused application.

```rhai
action.relay("cmd+c")     // Send Cmd+C
action.relay("shift+tab") // Send Shift+Tab
```

### Mode Navigation

Control navigation through the modal key hierarchy.

| Action | Description |
|--------|-------------|
| `action.pop` | Return to the previous mode (go up one level) |
| `action.exit` | Exit all modes and return to root |

```rhai
m.bind("esc", "Back", action.pop);
m.bind("q", "Quit", action.exit);
```

### Volume Control

Control system volume.

| Action | Description |
|--------|-------------|
| `action.set_volume(level)` | Set absolute volume (0-100) |
| `action.change_volume(delta)` | Adjust volume by delta (-100 to +100) |
| `action.mute(toggle)` | Control mute state |

```rhai
action.set_volume(50)      // Set to 50%
action.change_volume(10)   // Increase by 10
action.change_volume(-10)  // Decrease by 10
action.mute(on)            // Mute
action.mute(off)           // Unmute
action.mute(toggle)        // Toggle mute state
```

### Theme Control

Control the visual theme.

| Action | Description |
|--------|-------------|
| `action.theme_next` | Switch to the next theme |
| `action.theme_prev` | Switch to the previous theme |
| `action.theme_set(name)` | Set a specific theme by name |

```rhai
action.theme_next
action.theme_prev
action.theme_set("dark-blue")
```

Available themes: `default`, `charcoal`, `dark-blue`, `solarized-dark`,
`solarized-light`.

### UI Control

Control the hotki user interface.

| Action | Description |
|--------|-------------|
| `action.show_details(toggle)` | Control the details window |
| `action.show_hud_root` | Display the root-level HUD |
| `action.user_style(toggle)` | Enable/disable user style overlay |
| `action.clear_notifications` | Clear all on-screen notifications |
| `action.reload_config` | Reload the configuration file |

```rhai
action.show_details(toggle)   // Toggle details window
action.show_details(on)       // Show details
action.show_details(off)      // Hide details

action.user_style(toggle)     // Toggle user styling
action.clear_notifications    // Clear notifications
action.reload_config          // Reload config file
```

### Script Actions (Closures)

Actions can be computed at runtime using closures. These are useful for
conditional logic or returning multiple actions.

```rhai
// Simple closure returning an action
m.bind("p", "Play/Pause", || action.shell("spotify pause"));

// Context-aware action (receives ctx with app, title, pid, depth, path)
m.bind("o", "Open", |ctx| {
  if ctx.app.contains("Safari") {
    action.shell("open -a Safari ~/logs/safari.log")
  } else {
    action.shell("open ~/logs/system.log")
  }
});

// Macro action: return an array of actions executed in sequence
m.bind("s", "Save + Beep", || [
  action.relay("cmd+s"),
  action.shell("afplay /System/Library/Sounds/Pop.aiff").silent(),
]);
```

### Toggle Values

Several actions accept a toggle parameter:

| Value | Description |
|-------|-------------|
| `on` | Enable |
| `off` | Disable |
| `toggle` | Flip the current state |

### Notification Kinds

Used with `.notify()` modifier on shell actions:

| Kind | Description |
|------|-------------|
| `ignore` | No notification |
| `info` | Informational notification |
| `warn` | Warning notification |
| `error` | Error notification |
| `success` | Success notification |
