# Configuration

Hotki behavior configs are Luau scripts loaded from `~/.hotki/config.luau` by default.
Styling is separate: place optional global style overrides in sibling `~/.hotki/style.luau`.

> [!NOTE]
> The behavior config and optional style file are statically checked under Luau strict mode during
> validation. There is no need to add `--!strict` annotations.

The config API surface is defined by [`hotki_core.d.luau`](./crates/config/luau/hotki_core.d.luau)
and [`hotki_config.d.luau`](./crates/config/luau/hotki_config.d.luau). Use `hotki api` when you
want the checked-in contract instead of prose.

## Root Config

A config registers exactly one root renderer:

```luau
hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", action.pop, {
            global = true,
            hidden = true,
        })
    end

    menu:submenu("shift+cmd+0", "Main", function(root, inner)
        root:bind("r", "Reload", action.reload_config)
        root:bind("a", "Run Application", action.selector({
            title = "Run Application",
            items = hotki.applications,
            on_select = function(actx: ActionContext, item: SelectorItem<ApplicationInfo>, query: string)
                actx:exec(action.open(item.data.path))
            end,
        }))
    end, {
        capture = true,
    })
end)
```

## Core Tables

- `hotki`: root registration and app discovery.
- `action`: primitive actions plus `action.run(...)` and `action.selector(...)`.

## Menu API

Mode renderers receive `(menu, ctx)`.

- `menu:bind(chord, desc, action, opts?)`
- `menu:bind_many(entries)`
- `menu:submenu(chord, title, render, opts?)`
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
})
```

Submenu options accept the same behavior flags plus `capture`.

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

## Single-file Configs

Hotki behavior config is one `config.luau` file. Use local functions, local tables, and Luau
types to organize larger configs:

```luau
local function notify(title: string): Action
    return action.run(function(ctx)
        ctx:notify("info", title, "done")
    end)
end

local utility_items: { SelectorItem<string> } = {
    { label = "Calendar", data = "calendar" },
    { label = "Calculator", data = "calculator" },
}
```

Config `require` calls are not supported. Keep shared behavior in the root file unless Hotki adds a
normal Ruau module system later.

## Selectors

Selectors accept either a static item list or a provider function:

```luau
menu:bind("a", "Run Application", action.selector({
    title = "Run Application",
    placeholder = "Search apps...",
    items = hotki.applications,
    on_select = function(actx: ActionContext, item: SelectorItem<ApplicationInfo>, query: string)
        actx:exec(action.open(item.data.path))
    end,
}))
```

Static items use either string arrays or `{ label, sublabel?, data }` records. Provider functions
receive `ModeContext` and return the same kind of list.

## Style

`config.luau` does not contain style declarations. Put global style overrides in sibling
`style.luau`; see [STYLE.md](./STYLE.md).

Removed style and theme APIs now fail validation with migration-oriented diagnostics:

- `themes`
- `action.theme_next`, `action.theme_prev`, `action.theme_set`
- `menu:style(...)`
- binding option `style`
- `hotki.import_style(...)`

## Validation

```bash
hotki check --config ~/.hotki/config.luau
hotki api --surface config --filter action
hotki api --surface style
```
