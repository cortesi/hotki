# Hotki Testing Principles

Hotki tests should exercise the shipped macOS paths while keeping failures
diagnosable. The main suites are Rust unit and integration tests, the smoketest
binary, and the local eguidev automation scripts.

## Scope

- Use Rust tests for pure logic, protocol round trips, worker lifecycle, and
  engine behavior that can run without driving windows.
- Use `cargo run --bin smoketest -- all` for end-to-end server and UI smoke
  coverage after UI-facing changes.
- Use eguidev scripts for local inspection of egui-native viewports such as the
  main window, logs, and Permissions.
- Do not add direct CoreGraphics or Accessibility probes to individual test
  cases when an existing bridge or helper can expose the same state.

## Determinism

- Prefer event-driven waits, channel notifications, or observable state changes.
- Keep artifacts under `tmp/` so repeated local runs do not dirty the tree.
- Give full test and smoketest commands extended timeouts; these suites can take
  more than 60 seconds on macOS.
- Avoid case-specific environment variables. Prefer function parameters,
  fixtures, or configuration structs when a test needs a different behavior.

## Permissions

Hotki is a real macOS event-tap application. Tests that need Accessibility or
Input Monitoring should either run only when those permissions are present or
exercise code through the repository's synthetic RPC and devtools paths. There
is no fake permission mode in the shipped app.

## Message Style

Emit concise, structured assertion output from test harnesses. Logs should make
the failing case, observed state, and expected state easy to grep from long
smoketest runs.
