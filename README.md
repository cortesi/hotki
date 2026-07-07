# Hotki

Hotki is a macOS-only hotkey application with a Luau-based configuration runtime.

Behavior lives at `~/.hotki/config.luau`. Styling lives in optional sibling
`~/.hotki/style.luau`, which returns a partial style table merged over Hotki's embedded default.
The checked-in Luau declaration files define both surfaces, and the CLI can print them directly:

```bash
hotki api
hotki api --surface style
hotki api --surface all --markdown
hotki api --filter selector
```

Useful entry points:

- [CONFIG.md](./CONFIG.md): Luau config structure, imports, selectors, and validation.
- [STYLE.md](./STYLE.md): `style.luau`, the embedded default style, and style dumping.

Examples:

- `examples/complete.luau`
- `examples/style.luau`
- `examples/cortesi.luau`
- `examples/match.luau`
- `examples/selector.luau`
- `examples/selector-custom.luau`
- `examples/test.luau`

Validate a config and its sibling style file:

```bash
hotki check --config ~/.hotki/config.luau
```

Dump the embedded default style source:

```bash
hotki style --default
```

Minimal `config.luau`:

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
