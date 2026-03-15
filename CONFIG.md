# Configuration

Hotki configs are Luau scripts loaded from `~/.hotki/config.luau` by default.

The exact API surface is defined by [`crates/config/luau/hotki.d.luau`](./crates/config/luau/hotki.d.luau).
Use `hotki api` when you want the checked-in contract instead of prose.

## Root Config

A config registers exactly one root renderer:

```luau
themes:use("default")

hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", action.pop, {
            global = true,
            hidden = true,
        })
    end

    menu:submenu("shift+cmd+0", "Main", function(root, inner)
        root:bind("r", "Reload", action.reload_config)
        root:bind("t", "Next Theme", action.theme_next, { stay = true })
        root:bind("a", "Run Application", action.selector({
            title = "Run Application",
            items = hotki.applications,
            on_select = function(actx, item, query)
                actx:exec(action.open(item.data.path))
            end,
        }))
    end, {
        capture = true,
    })
end)
```

## Core Tables

- `hotki`: root registration, multi-file imports, and app discovery.
- `action`: primitive actions plus `action.run(...)` and `action.selector(...)`.
- `themes`: built-in and user theme registry access.

## Menu API

Mode renderers receive `(menu, ctx)`.

- `menu:bind(chord, desc, action, opts?)`
- `menu:bind_many(entries)`
- `menu:submenu(chord, title, render, opts?)`
- `menu:style(overlay)`
- `menu:capture()`

Binding options are plain tables:

```luau
menu:bind("r", "Repeat Relay", action.relay("cmd+c"), {
    stay = true,
    global = false,
    hidden = false,
    ["repeat"] = {
        delay_ms = 200,
        interval_ms = 300,
    },
    style = {
        key_bg = "#ff0000",
    },
})
```

## Contexts

`ModeContext` and `ActionContext` expose:

- `app`, `title`, `pid`, `hud`, `depth`
- `ctx:app_matches(pattern)`
- `ctx:title_matches(pattern)`

`ActionContext` additionally exposes:

- `ctx:exec(action)`
- `ctx:notify(kind, title, body)`
- `ctx:stay()`
- `ctx:push(render, title?)`
- `ctx:pop()`
- `ctx:exit()`
- `ctx:show_root()`

## Multi-file Configs

Hotki supports typed, role-specific imports instead of general `require(...)`:

```luau
local app_selector = hotki.import_mode("common/app-selector")
local local_theme = hotki.import_style("themes/local")
```

Available helpers:

- `hotki.import_mode(path)`
- `hotki.import_items(path)`
- `hotki.import_handler(path)`
- `hotki.import_style(path)`

Rules:

- Paths are relative to the config directory.
- Absolute paths and `..` traversal are rejected.
- Files use the `.luau` extension.
- Imported files are cached by canonical path and validated by role.

## Selectors

Selectors accept either a static item list or a provider function:

```luau
menu:bind("a", "Run Application", action.selector({
    title = "Run Application",
    placeholder = "Search apps...",
    items = hotki.applications,
    on_select = function(actx, item, query)
        actx:exec(action.open(item.data.path))
    end,
}))
```

Static items use `{ label, sublabel?, data }` records.

## Themes

- Built-in themes live in `themes/*.luau`.
- User themes load from `themes/` next to the active config.
- User themes override built-ins by name.
- Script-registered themes override both.

Examples:

```luau
themes:use("dark-blue")

local custom = themes:get("default")
custom.hud = custom.hud or {}
custom.hud.font_size = 18
themes:register("large-default", custom)
themes:use("large-default")
```

Theme files are plain Luau scripts that `return` a `StyleOverlay` table.

## Validation

```bash
hotki check --config ~/.hotki/config.luau
hotki api --filter action
```
