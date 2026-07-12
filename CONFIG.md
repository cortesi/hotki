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
local GLOBAL = { global = true, hidden = true }

return function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", a.pop, GLOBAL)
    end

    menu:submenu("shift+cmd+0", "Main", function(root)
        root:bind("r", "Reload", a.reload_config)
        root:bind("a", "Run Application", a.select({
            title = "Run Application",
            placeholder = "Search apps...",
            items = hotki.applications,
            on_select = function(select_ctx, item)
                select_ctx:open(item.data.path)
            end,
        }))
        root:bind("n", "Report", function(action_ctx)
            action_ctx:notify("info", "Hotki", "Starting work")
            action_ctx:shell("open https://example.com")
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

menu:bind("p", "Pop", a.pop)
menu:bind("s", "Shell", a.shell("open -a Finder"))
menu:bind("v", "Volume up", a.hold(a.change_volume(5)), { stay = true })
menu:bind("m", "Mute", a.mute("toggle"))
menu:bind("d", "Details", a.show_details("toggle"))
```

The table covers `pop`, `exit`, `show_root`, `hide_hud`, `reload_config`,
`clear_notifications`, `stay`, `notify`, `push`, `shell`, `open`, `relay`, `show_details`,
`set_volume`, `change_volume`, `mute`, `hold`, and `select`.

Direct closures remain the composition mechanism. Effects queue while a handler runs and execute
in source order after it returns.

## Menu and Context

A `ModeRenderer` receives `(menu, ctx)` and builds bindings in order:

- `menu:bind(chord, desc, action, opts?)`
- `menu:submenu(chord, title, render, opts?)`
- `menu:capture()`

Binding options are `global`, `hidden`, and `stay`. Submenu options add `capture`.

`ModeContext` and `ActionContext` expose `app`, `title`, `pid`, `hud`, `depth`,
`app_matches(pattern)`, and `title_matches(pattern)`. `ActionContext` also exposes the effect
methods mirrored by `hotki.actions`; use it directly for composite or conditional behavior.

<!-- hotki-luau: fragment -->
```luau
menu:bind("n", "Conditional notification", function(ctx)
    if ctx:app_matches("Finder") then
        ctx:notify("info", "Finder", ctx.title)
    else
        ctx:notify("warn", "Other application", ctx.app)
    end
end)
```

## Modules

Filesystem-backed configs may use ordinary `require` with an explicit relative request. A module
can return any normal Luau value: a renderer, action factory, helper table, or data.

<!-- hotki-luau: module -->
```luau
-- apps/finder.luau
local a = hotki.actions

return function(menu: MenuBuilder)
    menu:bind("n", "New Finder window", a.relay("cmd+n"))
end
```

<!-- hotki-luau: fragment -->
```luau
local finder = require("./apps/finder")

return function(menu, ctx)
    if ctx:app_matches("Finder") then
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

Selectors accept a static list or a provider function. String lists and records shaped as
`{ label, sublabel?, data }` are supported. Providers receive `ModeContext`; selection and cancel
callbacks receive `ActionContext`.

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

`hotki.root` was removed. The checker reports a targeted migration message: return the renderer
from `config.luau`.

## Documentation Fence Convention

Complete standalone entries use `<!-- hotki-luau: config -->` immediately before a `luau` fence.
`cargo xtask luau` extracts these fences into `tmp/` and strict-checks them. Non-standalone samples
must be marked `fragment` or `module`; they are illustrative and are not treated as entry roots.
