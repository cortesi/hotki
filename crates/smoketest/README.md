# Smoketest Runner

The smoketest crate exercises the shipped HUD, mini HUD, display placement, and notification paths
through a synthetic RPC-driven app session. The live case registry is the source of truth; list it
with:

```bash
cargo run --bin smoketest -- list
```

## Running Smoketests

```bash
cargo run --bin smoketest -- all
```

Use `seq` to run selected cases in order, for example:

```bash
cargo run --bin smoketest -- seq hud notifications
```

Pass `--debug` or `--trace` before the subcommand to increase tracing output.

`--run-budget` sets the complete wall-clock allowance for each case, including app startup, RPC
readiness, actions, waits, and cleanup. For a slower machine or cold launch:

```bash
cargo run --bin smoketest -- --run-budget 30000 all
```
