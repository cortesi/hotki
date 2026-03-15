# Themes

Built-in theme names match `themes/*.luau` file stems:

- `default`
- `charcoal`
- `dark-blue`
- `solarized-dark`
- `solarized-light`

Theme files return a `StyleOverlay` table. The checked-in themes in [`themes/`](./themes) are also
the format user themes should follow.

Select a built-in theme:

```luau
themes:use("dark-blue")
```

Derive a custom theme from a built-in:

```luau
local large = themes:get("default")
large.hud = large.hud or {}
large.hud.font_size = 18

themes:register("large-default", large)
themes:use("large-default")
```

Custom themes in `~/.hotki/themes/*.luau` override built-ins by name.
