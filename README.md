# Hotki

Hotki is a macOS-only hotkey application with a Luau-based configuration runtime.

Configuration lives at `~/.hotki/config.luau`. The precise scripting contract is the checked-in
[`hotki.d.luau`](./crates/config/luau/hotki.d.luau) file, and the CLI can print it directly:

```bash
hotki api
hotki api --markdown
hotki api --filter selector
```

Useful entry points:

- [CONFIG.md](./CONFIG.md): Luau config structure, multi-file imports, selectors, and themes.
- [THEMES.md](./THEMES.md): Built-in themes and screenshots.
- [themes/README.md](./themes/README.md): Writing custom `themes/*.luau` files.

Examples:

- `examples/complete.luau`
- `examples/cortesi.luau`
- `examples/match.luau`
- `examples/selector.luau`
- `examples/selector-custom.luau`
- `examples/test.luau`

Validate a config:

```bash
hotki check --config ~/.hotki/config.luau
```

Minimal example:

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
