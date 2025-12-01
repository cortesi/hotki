# Hotki Engine

`hotki-engine` drives key handling, relays, repeat timing, and HUD/notification
updates for the desktop app. Window management (raise/hide/fullscreen/place/etc.)
has been removed from the engine; those behaviours now belong in an external
window CLI that Hotki can invoke via `shell(...)` bindings.

## Single-Source-of-Truth Policy

- Focus context and display snapshots originate from `hotki-world` snapshots.
  Consumers wait for a world snapshot (or the helper APIs on `WorldView`) before
  relying on app/title/pid context.
- The engine never calls platform window APIs directly; its only macOS-native
  surface is keystroke relaying. Any future window interactions should flow
  through the external CLI via `shell(...)` bindings.
