# Developer Notes

## Contributor Docs

- [Testing Principles](docs/testing-principles.md) – relay/HUD guidance, budgets, skip semantics,
  and message format rules.

## Validation

- Always run `cargo xtask tidy` after changes, and fix all reported issues.
- For UI‑path changes, run: `cargo run --bin smoketest -- all`.
