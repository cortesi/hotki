# Hotki Smoketest Principles

Smoketests now cover the mac-native surfaces we still ship in-process: key relay
and the UI layer (HUD + notifications). Window management, placement, and focus
mutation live in an external CLI and are **out of scope** here.

## Scope and Contracts
- Keep cases limited to relay throughput, shell execution, volume changes, HUD,
  and notification rendering.
- Talk to the app only through the smoketest bridge and published helpers. Do
  not reach for AX/CoreGraphics directly.
- Add new probes to the bridge or UI runtime if a case needs extra visibility.

## Determinism
- Avoid sleeps. Prefer bounded loops that wait for bridge events or binding
  gates with explicit deadlines.
- Budget each stage (setup/action/settle) and log the timings. Tight budgets
  make flakiness obvious.
- Use the warn overlay when running interactive cases to avoid stray input.

## Reset and Isolation
- Each case should start the app with a fresh config and shut it down cleanly.
- Tear down bridge connections and temp sockets even on failure paths.
- Keep artifacts under `tmp/smoketest-scratch/run-<ts>/`.

## Skips and Environment
- Gate anything that might be environment-sensitive behind skip probes, but
  avoid case-specific env vars. If a probe is missing, extend the bridge/API
  rather than adding ad-hoc flags.
- Permissions (Accessibility/Input Monitoring) are mandatory unless running in
  fake mode (CI without permissions).

## Message Style
- Emit single-line, structured logs for assertions (key=value pairs). This keeps
  CI output machine-parseable and easy to grep.

## Do Not Do
- No window movement/activation tests here—those belong to the external CLI.
- No helper apps or mimic harnesses; keep everything in-process.
- No direct platform calls from cases; route through shared helpers instead.
