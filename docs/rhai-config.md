# Rhai configuration

Hotki's user configuration is a Rhai script at `~/.hotki/config.rhai` that builds the in-memory
`config::Config` structure.

This document describes the DSL and execution semantics, including Phase 2 script actions.

## File locations and resolution

Resolution policy:

1. If `--config <path>` is provided, use that file.
2. Else use `~/.hotki/config.rhai` if it exists.
3. Else error with a hint to copy `examples/complete.rhai`.

### Imports

Rhai modules can be imported from the config directory:

- `import "foo"` loads `foo.rhai` relative to the config file's directory.
- Import paths must be relative (no absolute paths and no `..` path segments).
- Symlinks that point outside the config directory are rejected.

You can validate any config file without starting the UI:

```bash
hotki check --config /path/to/config.rhai
hotki check  # uses the default resolution policy
```

## DSL reference

### Top-level functions

- `base_theme(name: &str) -> ()`
- `style(map) -> ()` where `map` is a Rhai object map that deserializes into `raw::RawStyle`
- `server(map) -> ()` where `map` deserializes into `raw::RawServerTunables`
- `env(var: &str) -> String` returns the environment variable value or `""` when unset

### Entry point: `global`

Hotki injects a `global` value of type `Mode`. All bindings are created from this root mode.

### `Mode` methods

- `mode.bind(chord: &str, desc: &str, action: Action) -> Binding`
- `mode.mode(chord: &str, desc: &str, |m| { ... }) -> Binding`

Chord strings use the same syntax as older Hotki releases (e.g. `shift+cmd+0`, `cmd+c`, `esc`).

### `Binding` fluent methods

Valid on both `.bind()` and `.mode()`:

- `.global()`
- `.hidden()`
- `.hud_only()`
- `.match_app(pattern: &str)`
- `.match_title(pattern: &str)`

Only valid on `.bind()` (error on `.mode()`):

- `.no_exit()`
- `.repeat()`
- `.repeat_ms(delay: i64, interval: i64)`

Only valid on `.mode()` (error on `.bind()`):

- `.capture()`
- `.style(map)` (per-mode style overlay)

Notes:

- Duplicate chords within the same mode are a load-time error.
- Empty modes are allowed: `m.mode("x", "Empty", |sub| {})`.

### `Action` constructors and methods

Constructors:

- `shell(cmd: &str) -> Action`
- `relay(spec: &str) -> Action`
- `show_details(t: Toggle) -> Action`
- `theme_set(name: &str) -> Action`
- `set_volume(level: i64) -> Action` (0–100)
- `change_volume(delta: i64) -> Action` (-100–100)
- `mute(t: Toggle) -> Action`
- `user_style(t: Toggle) -> Action`

Fluent methods:

- `action.clone() -> Action` (all actions)
- `shell(...).notify(ok: NotifyKind, err: NotifyKind) -> Action`
- `shell(...).silent() -> Action` (equivalent to `.notify(ignore, ignore)`)

Action values are immutable: fluent methods return a new `Action` and do not modify the original.

### Injected constants

Toggles:

- `on`, `off`, `toggle`

Notification kinds:

- `ignore`, `info`, `warn`, `error`, `success`

HUD positions:

- `center`, `n`, `ne`, `e`, `se`, `s`, `sw`, `w`, `nw`

Notification positions:

- `left`, `right`

HUD display modes:

- `hud_full`, `hud_mini`, `hud_hide`

Font weights:

- `thin`, `extralight`, `light`, `regular`, `medium`, `semibold`, `bold`, `extrabold`, `black`

Zero-arg actions (as values):

- `pop`, `exit`, `reload_config`, `clear_notifications`, `theme_next`, `theme_prev`, `show_hud_root`

## Script actions (Phase 2)

In addition to passing a built-in `Action` value to `bind`, you can pass a Rhai function/closure.
The callable is evaluated at runtime when the hotkey triggers and must return:

- an `Action`, or
- an array of actions, `[Action, ...]`, for a "macro" sequence executed left-to-right.

Callables can take zero arguments (`|| ...`) or a single `ActionCtx` argument (`|ctx| ...`).

### `ActionCtx`

The runtime passes a small context object to help compute actions:

- `ctx.app: String` — focused app name
- `ctx.title: String` — focused window title
- `ctx.pid: i64` — focused app PID
- `ctx.depth: i64` — current mode depth (0 = root)
- `ctx.path: Array` — cursor indices from the root

### Semantics and safety

- Returning `()` (unit) or any non-`Action` type is an error.
- Script actions cannot synthesize nested key trees (`Action::Keys`) or return other script actions.
- `.repeat()`/`.repeat_ms()` and `.no_exit()` are still configured on the binding; the resolved
  action(s) must be compatible with the requested attributes.
- Runtime execution is sandboxed with conservative Rhai limits (max operations/call depth); I/O is
  only possible via the built-in `Action` constructors (e.g. `shell`, `relay`).
