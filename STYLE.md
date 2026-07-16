# Style

Hotki has one embedded default style. Users can add an optional sibling
`~/.hotki/style.luau` next to `~/.hotki/config.luau`; that file returns a partial style table
merged over the default.

Dump the default style source:

```bash
hotki style --default
```

Dump the effective style for the resolved config plus sibling `style.luau`:

```bash
hotki style --config ~/.hotki/config.luau
```

Validate both behavior and style:

```bash
hotki check --config ~/.hotki/config.luau
```

Minimal `style.luau`:

```luau
return {
    hud = {
        pos = "ne",
        bg = "#1a1a1a",
        key_bg = "#2a2a2a",
        pressed = {
            min_duration_ms = 120,
            bg = "#20295f",
        },
    },
    notify = {
        pos = "right",
        timeout = 3.5,
    },
}
```

The style API surface is defined by
[`hotki_style.d.luau`](./crates/config/luau/hotki_style.d.luau):

```bash
hotki api --surface style
```

`style.luau` is intentionally standalone. It has style value types, but it does not have config
globals such as `hotki` or behavior imports.

`hud.pressed` is a partial overlay for visible handler rows declared with `stay = true`. Its
`min_duration_ms` keeps quick taps visible after release; `0` makes feedback follow only the
physical held state. Values above 2000 ms are rejected. The section also accepts `bg`, `title_fg`,
`key_fg`, `key_bg`, `mod_fg`, `mod_bg`, and `tag_fg` color overrides.
