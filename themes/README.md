# Theme Files

Hotki themes are `*.luau` files that `return` a single table with `hud`, `notify`, and/or
`selector` sections.

Example:

```luau
return {
    hud = {
        bg = "#101010",
        title_fg = "#d0d0d0",
    },
}
```

Place custom themes in `themes/` next to `config.luau`, for example `~/.hotki/themes/my-theme.luau`,
then activate them from your config:

```luau
themes:use("my-theme")
```
