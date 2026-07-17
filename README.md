# Hotki

Hotki is a macOS-only hotkey application configured with Luau.

Behavior lives at `~/.hotki/config.luau`. Styling lives in optional sibling
`~/.hotki/style.luau`, which overlays Hotki's embedded default style.

Features:

- Global hotkeys and nested HUD menus.
- Luau config with typed API docs.
- App and custom-item selectors.
- Direct process, shell, open, focused/targeted relay, and volume actions.
- Native notifications and style overlays.

```bash
hotki check --config ~/.hotki/config.luau
hotki style --default
hotki api
hotki api --surface config --filter Actions
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
    end, { capture = true })
end
```

Use `a.exec({ program = "/absolute/path", args = { ... } })` for literal process arguments.
Keep `a.shell(command)` for intentional shell language such as pipelines and expansion;
`a.launch_application()` and `menu:with(defaults)` cover common selector and option boilerplate.

Target a running application without activating it by exact AppKit localized name:

<!-- hotki-luau: fragment -->
```luau
local youtube_music = a.relay_to_app("YouTube Music")
root:bind("p", "YouTube Music Play/Pause", youtube_music("space"))
```

Targeted relays fail closed with a warning when the exact name is missing or ambiguous. They use
ordinary application shortcuts, so browser extensions such as Vimium may need a site exclusion.

## Secure Input and Diagnostics

Some password fields and terminal applications enable macOS Secure Event Input. While it is
active, macOS pauses Hotki's physical hotkeys; Hotki leaves its server, configuration, and hotkey
registrations running and resumes automatically when Secure Input ends. The main window and tray
show `Hotkeys paused by Secure Input` when registered hotkeys are affected.

Hotki may show the application associated with Secure Input. This owner is a best-effort snapshot
from the current macOS session and can be missing or change before the notice is rendered. It is
never used to decide whether hotkeys are blocked.

For a support report, open **Logs** from the main window and choose **Copy Diagnostics**. The report
contains runtime state, permissions, input-health counters, binding and notification counts, and a
short allowlisted health-log tail. It omits configuration paths and contents, binding identifiers,
notification text, typed text, and raw key events.
