# Hotki Engine

`hotki-engine` drives key handling and window orchestration for the desktop app. It
is intentionally world-centric: the engine treats `hotki-world` as the only
authority for focus, window identity, and placement eligibility.

## Single-Source-of-Truth Policy

- Window state and focus context **always** originate from `hotki-world`
  snapshots. Consumers must wait for a world snapshot (or the helper APIs on
  `WorldView`) before selecting a target.
- Placement/move requests must provide a `WorldWindowId` returned by
  `hotki-world`. The engine may not rely on Accessibility/CG helpers such as
  `frontmost_window_for_pid` or the focused variants of `request_place_*`.
- A workspace-level Clippy guard denies any direct use of
  `mac_winops::request_place_grid_focused*`, `mac_winops::place_grid_focused*`,
  and `mac_winops::window::frontmost_window_for_pid` outside `hotki-world`.
  Crates that legitimately need them (tests, winops internals) opt in with an
  explicit allowance so production code cannot regress.
- Integration coverage exercises pid collisions to confirm the engine routes
  placements through the world-selected window, preventing accidental fallbacks
  to "whatever AX reports".

Follow these guard rails whenever adding new actions: resolve targets through
`hotki-world`, pass the resulting `WorldWindowId` into mac-winops APIs, and rely
on world refresh hints (instead of synchronous AX lookups) to keep the snapshot
fresh.
