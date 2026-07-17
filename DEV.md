# Developer Notes

## Contributor Docs

- [Testing Principles](docs/testing-principles.md) – relay/HUD guidance, budgets, skip semantics,
  and message format rules.

## Validation

- Always run `cargo xtask tidy` after changes, and fix all reported issues.
- Run `cargo xtask luau` after changing config declarations, examples, or marked Markdown Luau
  fences. It validates top-level examples, nested `config.luau` graphs, and complete documentation
  entries; helper modules are checked through their entry graph rather than as roots.
- Run `cargo xtask edev` after changing `.edev.toml` or the app's package, binary, feature, flags,
  or fixture config. `cargo xtask tidy` includes the same check.
- Run `cargo xtask test` for the automated gate: Luau validation, Rust tests, and the native
  smoketest.
- For UI‑path changes, run: `cargo run --bin smoketest -- all`.

## Eguidev Automation

Hotki can be launched with an embedded eguidev MCP runtime for interactive and
scripted egui inspection. Install `edev` from crates.io, then use the repo-local
`.edev.toml`:

```bash
edev fixtures
edev fixture hotki.basic.default
edev smoke
edev mcp
```

The eguidev launch command builds `hotki-app` with `--features devtools` and runs
the checked-in `examples/eguidev-demo.luau` config with `--disable-event-tap`.
Style is loaded from an optional `style.luau` sibling of the active config; keep eguidev fixtures
aligned with that split when adding style-sensitive checks.
Use eguidev input for egui-native windows such as the main window, logs, and Permissions. HUD,
selector, and notifications are server-driven surfaces; drive those through
fixtures/server actions and inspect the rendered widgets. Synthetic input
(clicks, key presses, etc.) reaches immediate viewports automatically through
eguidev's egui plugin; no app-side wiring is required.

The main window has state-specific fixtures for ready-empty, populated, permission-required, and
invalid-config layouts. `hotki.logs` opens the dedicated diagnostic window.
Use `edev fixtures` for the complete inventory generated from the app's live fixture catalog.

Treat `edev smoke` as a local macOS automation suite for now. Do not add it to
CI until a macOS runner has `edev` and the required windowing permissions
configured explicitly.
