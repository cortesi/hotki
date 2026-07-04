# Developer Notes

## Contributor Docs

- [Testing Principles](docs/testing-principles.md) – relay/HUD guidance, budgets, skip semantics,
  and message format rules.

## Validation

- Always run `cargo xtask tidy` after changes, and fix all reported issues.
- For UI‑path changes, run: `cargo run --bin smoketest -- all`.

## Eguidev Automation

Hotki can be launched with an embedded eguidev MCP runtime for interactive and
scripted egui inspection. Install `edev` from the sibling
`~/git/public/eguidev` checkout, then use the repo-local `.edev.toml`:

```bash
edev fixtures
edev fixture hotki.basic.default
edev smoke
edev mcp
```

The eguidev launch command builds `hotki` with `--features devtools` and runs
the checked-in `examples/eguidev-demo.luau` config with `--disable-event-tap`.
Use eguidev input for egui-native windows such as Details and Permissions. HUD,
selector, and notifications are server-driven surfaces; drive those through
fixtures/server actions and inspect the rendered widgets. Synthetic input
(clicks, key presses, etc.) reaches immediate viewports automatically through
eguidev's egui plugin; no app-side wiring is required.

Details has tab-specific fixtures for ordinary-window inspection:
`hotki.details.config`, `hotki.details.logs`, and `hotki.details.about`.

Treat `edev smoke` as a local macOS automation suite for now. Do not add it to
CI until a macOS runner has the eguidev checkout, `edev`, and the required
windowing permissions configured explicitly.
