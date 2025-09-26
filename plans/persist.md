# Persistent Bridge Reliability

This plan sequences the work needed to leverage the new persistent UI bridge to tighten
synchronisation between hotki, hotki-world, and the smoketest harness. Stages are ordered
from highest to lowest impact on reliability.

0. Stage Zero: Harden Existing Bridge Implementation (must-do first)

1. [ ] Export `HOTKI_CONTROL_SOCKET` from `HotkiSession::spawn` so the UI always binds the expected bridge path.
2. [ ] Add automatic reconnection/backoff in `BridgeClient` when reads return `BrokenPipe` or `ConnectionReset`.
3. [ ] Cover the bridge handshake with an integration test that boots the UI runtime and ensures `BridgeRequest::Ping` succeeds before any smoketest case runs.
4. [ ] Document the bridge lifecycle (init, shutdown, error handling) in `docs/testing.md` and reference it from the smoketest README.
5. [ ] Restore propagation of `DriverError::NotInitialized` from `server_drive::shutdown` (or guarantee bridge handshake during session spawn) so teardown either reuses the bridge or explicitly falls back until the bridge is mandatory.

1. Stage One: Enforce Sequenced, Acknowledged Bridge Traffic (highest value)

1. [ ] Attach monotonic command IDs and timestamps to every `BridgeRequest`/`BridgeResponse`.
2. [ ] Buffer outstanding requests on the UI side and require explicit ACKs before processing new ones.
3. [ ] Teach the smoketest driver to fail fast when sequence gaps or delayed ACKs exceed thresholds.

2. Stage Two: Surface Live State Snapshots Through the Bridge

1. [ ] Stream world-focus and HUD updates over the bridge so tests can assert UI/world parity without polling.
2. [ ] Add a `wait_for_world_seq` bridge call that blocks until hotki-world reaches a target reconcile ID.
3. [ ] Update flaky smoketests to replace sleep-based waits with the new synchronisation primitives.

3. Stage Three: Harden Startup and Shutdown Paths

1. [ ] Extend the bridge handshake to include server idle-timer state and pending notifications.
2. [ ] Inject pre-shutdown drain hooks so the UI flushes world events before acknowledging `Shutdown`.
3. [ ] Add harness assertions that no stray bridge messages arrive after shutdown completes.

4. Stage Four: Expand Diagnostics and Guard Rails (lower value)

1. [ ] Capture bridge command/response latency metrics and emit structured logs for post-mortem triage.
2. [ ] Provide a `dump_bridge_state` request for on-demand inspection during flaky test failures.
3. [ ] Document the bridge protocol and expected sequencing guarantees in `docs/testing.md`.
