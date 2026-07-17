# Configuration

Hotki loads behavior from `~/.hotki/config.luau`. The entry file is a strict Luau module that
returns exactly one `ModeRenderer`. Optional styling stays separate in sibling
`~/.hotki/style.luau`.

The checked contract lives in
[`hotki_config.d.luau`](./crates/config/luau/hotki_config.d.luau). Inspect it with
`hotki api --surface config`, or narrow it with `--filter`.

## Entry Config

Use `hotki.actions` for one-effect handlers and ordinary closures when an action needs control
flow or several ordered effects.

<!-- hotki-luau: config -->
```luau
local a = hotki.actions

return function(menu, ctx)
    local global = menu:with({ global = true, hidden = true })
    if ctx.hud then
        global:bind("esc", "Back", a.pop)
    end

    menu:submenu("shift+cmd+0", "Main", function(root)
        root:bind("r", "Reload", a.reload_config)
        root:bind("a", "Run Application", a.launch_application())
        root:bind("n", "Report", function(action_ctx)
            action_ctx:notify("info", "Hotki", "Starting work")
            action_ctx:exec({
                program = "/usr/bin/open",
                args = { "https://example.com" },
            })
        end)
    end, { capture = true })
end
```

Hotki constrains the returned function contextually, so the common root, submenu, action, and
selector callback parameters do not need annotations. A missing, non-function, or multiple root
return fails with `config.luau must return a ModeRenderer`.

## Actions

`hotki.actions` is an immutable pure-Luau table. Constants and factories return the same `Action`
functions accepted by `menu:bind`:

<!-- hotki-luau: fragment -->
```luau
local a = hotki.actions

local stay = menu:with({ stay = true })

menu:bind("p", "Pop", a.pop)
menu:bind("s", "Shell", a.shell("open -a Finder"))
stay:bind("v", "Volume up", a.hold(a.change_volume(5)))
stay:bind("b", "Volume down", a.hold(a.change_volume(-5)))
menu:bind("m", "Mute", a.mute("toggle"))
menu:bind("u", "Unmute", a.mute("off"))
menu:bind("0", "Set volume 50%", a.set_volume(50))
menu:bind("d", "Hotki", a.show_main_window("toggle"))
```

The table covers `pop`, `exit`, `show_root`, `hide_hud`, `reload_config`,
`clear_notifications`, `stay`, `notify`, `push`, `shell`, `exec`, `open`, `relay`,
`relay_to_app`, `relay_with`, `launch_application`, `show_main_window`, `set_volume`,
`change_volume`, `mute`, `hold`, and `select`.

Wrap `change_volume` in `a.hold` for a held control; it defaults to a 250 ms initial delay and a
150 ms minimum interval. `set_volume` sets an exact level, `change_volume` applies exact deltas,
and `mute("on")`/`mute("off")` preserve explicit mute state control.

Direct closures remain the composition mechanism. Effects queue while a handler runs and execute
in source order after it returns.

`a.exec(spec)` and `ctx:exec(spec)` start a program directly with literal arguments. Use an
absolute program path for tools installed outside the standard GUI application environment, and
use `cwd` for a directory relative to the config entry. `a.shell(command, options?)` and
`ctx:shell(command, options?)` remain the escape hatch for shell syntax such as pipelines,
expansion, and conditionals. Hotki owns the complete process group: cancellation stops every child,
and children may not outlive a normally completed parent process.

### Targeted relays

`a.relay(spec)` and `ctx:relay(spec)` deliver an ordinary chord through the global HID stream to
the focused application. Use `a.relay_to_app(app_name)` or
`ctx:relay_to_app(app_name, spec)` to deliver in the background without activating the target:

<!-- hotki-luau: fragment -->
```luau
local youtube_music = hotki.actions.relay_to_app("YouTube Music")

menu:bind("p", "Play/Pause", youtube_music("space"))
```

`app_name` is an exact, case-sensitive AppKit localized name of a running application. It is not a
bundle ID. By contrast, `ctx.window.app`, `when_app`, and `WindowContext:app_matches` use the
CoreGraphics window-owner name. Those names normally agree, but localization or application
metadata can make them diverge; copying `ctx.window.app` into `relay_to_app` can therefore produce
a not-running warning. Handle `ctx.window == nil` before copying it.

Hotki resolves the name when the gesture starts and pins that process through repeat and key-up.
It never launches or activates the application and never falls back to the focused application.
For modified chords, process-scoped delivery carries modifier flags on the main key but does not
send separate modifier transitions to the background application.
No matching process produces `Application "NAME" is not running`; multiple distinct matching
processes produce `Application "NAME" is ambiguous: N running matches`. Both warnings are
fail-closed and post no key event.

Targeted relays send ordinary application keys, not global media keys. Browser extensions and site
shortcut policy still apply: for example, disable Vimium for
`music.youtube.com` when it intercepts a YouTube Music chord. Keep `relay_with` for focused prefix
composition; destination selection stays explicit in `relay_to_app`.

## Menu and Context

A `ModeRenderer` receives `(menu, ctx)` and builds bindings in order:

- `menu:bind(chord, desc, action, opts?)`
- `menu:submenu(chord, title, render, opts?)`
- `menu:with(defaults)`
- `menu:capture()`

Binding options are `global`, `hidden`, and `stay`. Submenu options add `capture`.
`with` returns a derived builder sharing the same ordered output. Its defaults apply to bindings on
that view, including submenu entry bindings, but do not propagate into submenu contents; explicit
fields override only the corresponding default.

`ModeContext` and `ActionContext` expose `window`, `hud`, and `depth`. `window` is either `nil` or
an immutable `WindowContext` with `id`, `pid`, `app`, `title`, optional `display_id`,
`app_matches(pattern)`, and `title_matches(pattern)`. All fields describe the same window captured
for the activation; this is a snapshot, not a live handle. Opening a transient menu starts a menu
session, and every nested renderer and action retains that opening window until the menu exits.
Focus changes caused by Hotki's HUD therefore do not replace the target. Outside a menu session,
each activation captures the current focused window. No focused window is a normal state.
`ActionContext` also exposes the effect methods mirrored by `hotki.actions`; use it directly for
composite or conditional behavior.

<!-- hotki-luau: fragment -->
```luau
menu:bind("n", "Conditional notification", function(ctx)
    local window = ctx.window
    if window ~= nil and window:app_matches("Finder") then
        ctx:notify("info", "Finder", window.title)
    elseif window ~= nil then
        ctx:notify("warn", "Other application", window.app)
    else
        ctx:notify("warn", "Window", "No focused window is available")
    end
end)
```

`hotki.renderers` provides pure composition for application-specific modules. `combine` invokes
every renderer in source order, `when_app` uses exact equality, and `when_app_matches` uses the
same regular-expression matching as `WindowContext:app_matches`. Both application filters skip
their renderer when `ctx.window == nil`:

<!-- hotki-luau: fragment -->
```luau
local r = hotki.renderers
local finder = require("./apps/finder")

return r.combine(
    function(menu, _ctx)
        menu:bind("r", "Reload", hotki.actions.reload_config)
    end,
    r.when_app("Finder", finder),
    r.when_app_matches("Brave", require("./apps/brave"))
)
```

## Modules

Filesystem-backed configs may use ordinary `require` with an explicit relative request. A module
can return any normal Luau value: a renderer, action factory, helper table, or data.

<!-- hotki-luau: module -->
```luau
-- apps/finder.luau
local a = hotki.actions

return function(menu, _ctx)
    menu:bind("n", "New Finder window", a.relay("cmd+n"))
end
```

<!-- hotki-luau: fragment -->
```luau
local finder = require("./apps/finder")

return function(menu, ctx)
    local window = ctx.window
    if window ~= nil and window:app_matches("Finder") then
        finder(menu)
    end
end
```

Requests must begin with `./` or `../`. Bare package-like names, aliases, direct `init` requests,
ambiguous file/directory candidates, and lexical escapes from the entry directory are rejected.
The standard resolver accepts unambiguous `.luau`, `.lua`, and directory `init` candidates.
Symlinks placed inside the trusted config directory are followed.

Hotki checks the complete reachable graph and then activates that exact graph. A computed request
may resolve only to a checked module. Module source bytes and bytecode are prepared before the VM
runs, so activation does not reread or compile dependencies. After the entry returns, `require`
may reuse an already cached export, but cannot load a new module from a renderer or action.

Module top-level code runs once for each fresh load or reload. Reload builds and validates a new VM
and graph before replacing the active config. If loading fails, the old config and callbacks remain
active.

## Selectors

`a.launch_application(options?)` is the common application selector. It supplies
`hotki.applications`, opens the selected application path, and defaults its title and placeholder
to `Run Application` and `Search apps...`:

<!-- hotki-luau: fragment -->
```luau
menu:bind("a", "Run Application", hotki.actions.launch_application())
```

Use `a.select(spec)` when a selector needs a different provider or callback. Selectors accept a
static list or a provider function. String lists and records shaped as
`{ label, sublabel?, data }` are supported. Providers receive `ModeContext`; selection and cancel
callbacks receive `ActionContext`.

The provider and terminal callbacks retain the window that opened the selector. Their `hud` and
`depth` values reflect selector close time. If the selector returns to an existing menu session,
Hotki keeps that menu's opening window; otherwise it rebinds using the window captured for the
closing key activation.

<!-- hotki-luau: fragment -->
```luau
menu:bind("a", "Run Application", hotki.actions.select({
    items = hotki.applications,
    on_select = function(ctx, item)
        ctx:open(item.data.path)
    end,
}))
```

Use explicit `SelectorItem<T>` or provider annotations when defining a reusable public generic
helper; callback annotations are unnecessary in the common inline form.

## Window-relative commands

External tools that operate on the originating window must receive `ctx.window.id` explicitly.
Do not rely on the ambient selected window: the HUD may own that state when the child process
starts. If the captured window disappears, the tool reports the stale target; Hotki does not fall
back to a different window.

<!-- hotki-luau: fragment -->
```luau
menu:bind("f", "Toggle fullscreen", function(ctx)
    local window = ctx.window
    if window == nil then
        ctx:notify("warn", "Window", "No focused window is available")
        return
    end
    ctx:exec({
        program = "/opt/homebrew/bin/yabai",
        args = { "-m", "window", tostring(window.id), "--toggle", "zoom-fullscreen" },
    })
end)
```

## Style

`config.luau` does not contain style declarations. Put global overrides in sibling `style.luau`;
see [STYLE.md](./STYLE.md). Behavior modules do not change style lookup: Hotki checks only the
entry file's sibling style.

## Validation and Diagnostics

```bash
hotki check --config ~/.hotki/config.luau
hotki api --surface config --filter Actions
hotki api --surface config --filter ModeRenderer
```

A successful check reports both graph size and style presence:

```text
OK (modules: 5, style: false)
```

Child syntax and type errors name the child file and include a source excerpt. Typical policy
errors are direct:

```text
module request 'util' must begin with ./ or ../
module 'late' is outside the checked config graph
module source is sealed after config entry evaluation
```

## Documentation Fence Convention

Complete standalone entries use `<!-- hotki-luau: config -->` immediately before a `luau` fence.
`cargo xtask luau` extracts these fences into `tmp/` and strict-checks them. Non-standalone samples
must be marked `fragment` or `module`; they are illustrative and are not treated as entry roots.
