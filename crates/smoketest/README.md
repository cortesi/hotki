# Smoketest Runner

The `smoketest` crate drives Hotki's deterministic world harness. Tests spawn helper windows, replay
mimic captures, and assert results exclusively through `hotki-world` APIs.

## Contributor Docs

- [Testing Principles](../../docs/testing-principles.md) – world-only flows, runloop pumping, reset
  guarantees, budgets, skip semantics, message format, and "Do Not Do" policy.
- [Mimic Scenarios](../../docs/mimic-scenarios.md) – capture lifecycle, bundle structure, quirks, and
  importer heuristics.

## Running Smoketests

```bash
cargo run --bin smoketest -- all
```

Pass `--logs` to surface tracing output or target a single scenario via its slug.
Each CLI subcommand maps directly to a registered slug (e.g. `repeat-shell`,
`place-animated`), and `seq` accepts the same names to run ad-hoc subsets.
