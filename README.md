# Hotki

Hotki is a macOS-only hotkey application configured with Luau.

Behavior lives at `~/.hotki/config.luau`. Styling lives in optional sibling
`~/.hotki/style.luau`, which overlays Hotki's embedded default style.

Features:

- Global hotkeys and nested HUD menus.
- Luau config with typed API docs.
- App and custom-item selectors.
- Shell, open, relay, media, and volume actions.
- Native notifications and style overlays.

```bash
hotki check --config ~/.hotki/config.luau
hotki style --default
hotki api
hotki api --surface style
hotki api --surface all --markdown
```

`cargo xtask install` installs `Hotki.app` to `/Applications` and links the
embedded CLI at `~/.local/bin/hotki`.

## Screenshots

Generated from Hotki's embedded default style with `cargo xtask screenshots`.

### HUD

![Hotki HUD screenshot with the default style](./assets/screenshots/hud.png)

### Selector

![Hotki selector screenshot with the default style](./assets/screenshots/selector.png)

### Notifications

![Hotki success notification screenshot with the default style](./assets/screenshots/notify_success.png)

![Hotki info notification screenshot with the default style](./assets/screenshots/notify_info.png)

![Hotki warning notification screenshot with the default style](./assets/screenshots/notify_warning.png)

![Hotki error notification screenshot with the default style](./assets/screenshots/notify_error.png)

Docs:

- [CONFIG.md](./CONFIG.md): Luau config structure, selectors, and validation.
- [STYLE.md](./STYLE.md): `style.luau`, the embedded default style, and style dumping.
- [examples/](./examples/): example configs and style overlays.

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
            on_select = function(
                actx: ActionContext,
                item: SelectorItem<ApplicationInfo>,
                query: string
            )
                actx:open(item.data.path)
            end,
        }))
    end, {
        capture = true,
    })
end)
```
