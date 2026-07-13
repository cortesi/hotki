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
