# Smoketest Bridge Lifecycle

This document outlines how the smoketest harness establishes, uses, and tears down the persistent
bridge between Hotki and the UI runtime. The bridge exposes high-level control endpoints that the
smoketests drive to replay shortcuts and inspect world state.

## Initialization

- `HotkiSession::spawn` generates a unique Unix-domain socket under the system temporary directory
  and exports it via the `HOTKI_CONTROL_SOCKET` environment variable before launching the Hotki UI.
- The UI runtime reads `HOTKI_CONTROL_SOCKET` during startup and binds the smoketest bridge listener
  to the provided path, replacing any stale socket from previous runs.
- After the process starts, `HotkiSession::spawn` constructs a fresh `BridgeDriver`, pointing it at the
  session's server socket, and calls `BridgeDriver::ensure_ready`. The handshake (`BridgeRequest::Ping`)
  returns the server's idle-timer snapshot plus any pending UI notifications; `ensure_ready` asserts the
  timer is disarmed and the pending list is empty before a smoketest case begins.

## Runtime Usage

- All bridge commands now flow through an explicit `BridgeDriver` handle. Each session owns its driver,
  eliminating the process-wide `OnceLock<Mutex<Option<BridgeClient>>>` singleton.
- `BridgeClient` resends requests automatically when reads fail with `BrokenPipe`, `ConnectionReset`,
  or `ConnectionAborted`, performing exponential backoff between reconnection attempts. This protects
  the harness from transient bridge restarts while keeping call semantics the same for callers.
- Smoketest helpers call `BridgeDriver::ensure_ready` when they expect the bridge to reconnect or when
  reconnection loops are required while the UI publishes bindings or other runtime state.

## Sequencing and ACKs

- Every bridge command carries a monotonically increasing `command_id` and a millisecond timestamp
  emitted by the smoketest driver. The driver fails immediately if responses arrive out of order.
- The UI issues an explicit `BridgeResponse::Ack` for each command before any side effects run. Acks
  include the current queue depth so the harness can observe inflight pressure.
- The smoketest driver enforces a `config::BRIDGE.ack_timeout_ms` budget (750 ms by default). If the
  ACK is not observed within that window the command is aborted and the test fails fast.
- After acknowledging, the UI processes one command at a time and buffers additional commands until
  the active command completes. Final responses reuse the same command ID so the driver can associate
  results with the original request.

## Shutdown

- `HotkiSession::shutdown` first attempts to close the UI via its `BridgeDriver`. If the bridge was
  never initialized, `BridgeDriver::shutdown` propagates `DriverError::NotInitialized`, allowing the
  session to fall back to a direct MRPC shutdown path until the bridge becomes mandatory.
- Whether the shutdown succeeds or fails, the harness drops the session's driver so subsequent sessions
  start from a clean state.
- The `HOTKI_CONTROL_SOCKET` path is removed during cleanup to avoid collisions when the next session
  spawns.

## Troubleshooting

- To diagnose early bridge failures, export `SMOKETEST_LOG_BINDINGS=1` before running the harness; the
  binding polls include elapsed timings and remaining chords.
- If reconnects keep failing, inspect the bridge socket path reported in the error and confirm that no
  other process is holding the Unix socket open.
