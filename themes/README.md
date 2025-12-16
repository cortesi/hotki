# Themes

Hotki themes are `*.rhai` files that evaluate to a single map with `hud` and/or `notify` sections.
These maps are deserialized into Hotki's `RawStyle` overlay type.

## Built-in themes

The built-in themes live in this directory **in the repo**, and are embedded into the binary at
compile time.

## User themes

To customize a theme:

1. Copy a built-in theme into your config directory's `themes/` folder (defaults to
   `~/.hotki/themes/` when your config is `~/.hotki/config.rhai`).
2. Edit it and select it from your `config.rhai`:

```rhai
theme("my-theme-name");
```

Theme names are derived from the filename stem, e.g. `dark-blue.rhai` → `"dark-blue"`.

