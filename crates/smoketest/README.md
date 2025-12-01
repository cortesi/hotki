# Smoketest Runner (relay + UI only)

Window-operation scenarios have been removed alongside the built-in macOS
window ops. The smoketest crate now exercises:

- Repeat throughput (`repeat-relay`, `repeat-shell`, `repeat-volume`).
- UI demos (`ui.demo.standard`, `ui.demo.mini`) that exercise HUD + notification styling.

## Running Smoketests

```bash
cargo run --manifest-path crates/smoketest/Cargo.toml -- all
```

Use `seq` to run specific slugs (e.g. `repeat-shell ui.demo.standard`). Pass
`--logs` to surface tracing output during a run.
