# Mimic Scenarios

Mimic scenarios replay captured desktop traces through the smoketest harness so we can assert world
behaviour without driving live third-party apps. Each scenario consists of a capture bundle, a
canonical manifest, and optional importer heuristics to reconcile platform quirks.

## Structure
- **Capture bundle** – Raw payload recorded by `hotki-tester`, including timeline events, CG window
  snapshots, and accessibility deltas. The bundle lives under `smoketest/mimic/<slug>/` alongside its
  manifest.
- **Manifest** – Describes the scenario slug, default budgets, display topology, and any required
  helper overlays. Manifests provide stable identifiers so CI can run subsets or stress loops.
- **Replay script** – The smoketest runner feeds events into `hotki-world` via the mimic harness.
  Tests interact with the world APIs exactly as they would against live windows, keeping a single
  assertion surface.

## Quirks

Stage Four introduces a dedicated mimic harness with deterministic quirks that mirror common
application behaviours. Each mimic window advertises its quirks in structured logs via the
`scenario_slug/window_label/quirks[]` tag.

- **AxRounding** – The helper reports CoreGraphics frames verbatim but publishes Accessibility frames
  rounded toward zero. Tests can assert that authoritative reconciliation prefers CG data while AX
  retains the historical rounding artefacts.
- **DelayApplyMove** – The helper defers applying requested frames until an additional main-thread
  pump occurs (`delay_apply_ms = 160`). This simulates apps that coalesce move requests before
  updating geometry.
- **IgnoreMoveIfMinimized** – Placement is skipped while the window remains miniaturized. Tests must
  restore the window (or choose `MinimizedPolicy::AutoUnminimize`) before expecting geometry changes.
- **RaiseCyclesToSibling** – Before yielding focus, the helper cycles through its sibling window so
  `RaiseStrategy::KeepFrontWindow` scenarios keep the original frontmost window ahead of the mimic
  under test.

Legacy documentation about pixel scaling, lost frames, and mode transitions still applies when the
`world-mimic` feature is enabled. The harness continues to log scaled-pixel comparisons, track
`lost_count`, and respect the authoritative reconciliation rules described in
`docs/testing-principles.md`.

## Capture Lifecycle
1. Record a live interaction with `hotki-tester capture --slug <name>`.
2. Review the capture locally using `smoke preview` to ensure the timeline matches expectations.
3. Normalize the bundle (trim transient metadata, scrub user PII) and commit it under
   `smoketest/mimic/<slug>`.
4. Document the scenario: purpose, key windows, budgets, and any required skip conditions.
5. Wire the scenario into the smoketest registry so CI executes it by default.

## Importer Heuristics
Some applications require heuristics to reconcile AX and CG behaviour. The importer layer applies
those rules while materializing mimic bundles. See `docs/importer-heuristics.md` for the current
policy table and implementation guidance. Cross-link heuristics from each scenario so future maintainers
understand why a rule exists and when it is safe to remove.

## Related Reading
- [Testing Principles](./testing-principles.md)
- [Importer Heuristics](./importer-heuristics.md)
- [`crates/smoketest` README](../crates/smoketest/README.md)
