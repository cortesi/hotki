# Hotki Server Injection: Checklist

Track progress for enabling deterministic programmatic driving of `hotki-server` from tests. Each stage keeps the project fully operational.

## Stage 0 — Document & Stabilize Public API (docs only)

- [x] Clarify crate-level docs in `crates/hotki-server/src/lib.rs` (per‑process socket, auto‑spawn, idle timeout, parent PID, event/heartbeat, focus watcher contract).
- [x] Add/expand concise docs for public items (`Server`, `Client`, `Connection`, `socket_path_for_pid`).
- [x] Expand `Server` notes (per‑UID+PID socket, parent PID watch, idle shutdown) in `server.rs`.
- [x] Expand `Connection` event stream semantics in `ipc/client.rs`.
- [x] Expand `Client::connect` docs (readiness poll + retries) in `client.rs`.
- [x] Format + lint + unit test (`cargo fmt`, `cargo clippy --fix`, `cargo test --all`).
- [ ] Optional: Smoketest run (`cargo run --bin smoketest -- all`).

## Stage 1 — Protocol: Add Key Injection RPC

- [x] Add `RpcErrorCode::KeyNotBound`.
- [x] Add `HotkeyMethod::InjectKey` (`"inject_key"`).
- [x] Add parameter codecs: `enc_inject_key(ident, kind, repeat)` and `dec_inject_key_params(...)`.
- [x] Implement service handler in `ipc/service.rs`:
  - [x] `ensure_engine_initialized().await`.
  - [x] Resolve ident → id; return `KeyNotBound` if missing.
  - [x] Map kind (`down|up`) and call `Engine::dispatch(id, kind, repeat).await`.
  - [x] Return `Value::Boolean(true)`.
- [x] Add client helpers on `Connection`: `inject_key_down`, `inject_key_up`, `inject_key_repeat`.
- [x] Format + lint + unit test (as above).
- [ ] Optional: Manual sanity (run GUI, connect a client, inject a known binding, observe HUD/logs).

## Stage 2 — Optional Status RPCs (Determinism helpers)

- [x] Add `HotkeyMethod::{GetBindings, GetDepth}`.
- [x] Implement service methods using engine (`bindings_snapshot`, `get_depth`).
- [x] Add client wrappers: `get_bindings()`, `get_depth()`.
- [ ] Validation: ensure `get_bindings()` returns non‑empty after `set_config`.

## Stage 3 — Smoketest RPC Driver (retain HID fallback)

- [x] Add `server_drive.rs` in `crates/smoketest`:
  - [x] `init(socket_path)` to establish shared connection.
  - [x] `inject_key(seq)` → `inject_key_down` + delay + `inject_key_up`.
  - [x] `inject_sequence(&[&str])`.
- [x] Add driver abstraction with env switch `HOTKI_DRIVE=rpc|hid` (default `rpc`), fallback to HID if RPC not ready.
- [ ] Gate readiness via `get_bindings()` when available, else via `HudUpdate`.
- [ ] Validation: `cargo run --bin smoketest -- all` (RPC + HID).

## Stage 4 — Migrate Flaky Tests and Tighten Timeouts

Modifications we apply per test (playbook):
- Use key injection (RPC-first) instead of HID where possible; fall back to HID
  only if RPC is unavailable.
- Add readiness gates: wait_for_ident via get_bindings() before each injected
  step to avoid racing rebinds.
- Hoist all magic numbers to smoketest config constants for easy tuning (polls,
  delays, timeouts, sizes).
- Reduce timeouts and sleeps to the minimum that’s stable once gating is in
  place.
- Make helper/test windows smaller and position them in a screen corner to
  reduce visual disruption.
- Hide the HUD where it’s not needed (style: (hud: (mode: hide))).
- Ensure hard termination: top-level watchdog enforces CLI timeout and
  force-kills tracked processes.

Readiness gating and RPC-first driving checklist (ticking items as we complete them):

- [x] raise (RPC, hidden HUD, small helpers, gated, constants hoisted)
- [x] hide (RPC, hidden HUD, small helpers, gated, constants hoisted)
- [x] focus (hidden HUD, tuned polling, constants hoisted)
- [x] ui
- [x] minui
- [ ] repeat_relay
- [ ] repeat_shell
- [ ] repeat_volume

- [ ] preflight
- [ ] fullscreen
- [ ] screenshots

### Stage 4 Notes (UI + Minui)

- For the UI and Minui smoketests we intentionally keep the HUD visible and anchor it at the bottom‑right corner (`pos: se`).
- We still gate key injection via RPC where available, then drive a short theme cycle and cleanly exit.

## Stage 5 — Polish & Developer Docs

- [ ] Add developer docs in `README.md`/`DEV.md` for connecting and injecting keys (socket conventions, auto‑spawn, examples).
- [ ] Document test env toggles (`HOTKI_DRIVE`, timeouts).
- [ ] Format + lint + unit test.

## Notes (Specs / Risks / Open Questions)

- RPC additions: `inject_key({ ident, kind: "down"|"up", repeat }) -> bool`; later `get_bindings()`, `get_depth()`.
- Concurrency: injection executes in same runtime as OS events; ordering follows arrival.
- Security: RPC path doesn’t post HID events; relies on registered identifiers.
- Risks: ident mismatch (mitigate with `get_bindings()`); binding churn window (consider `Engine::dispatch_ident` later).
