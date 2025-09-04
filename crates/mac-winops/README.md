# mac-winops

Window operations for Hotki on macOS:

- Native fullscreen (AXFullScreen) and non‑native “maximize to visible frame”.
- Grid snapping and movement of the focused window.
- Focus watching (foreground app and title).

Requirements

- macOS only. Uses public AX/CG/AppKit APIs (no private frameworks).
- Accessibility permission required for window operations.
- Some operations must run on the AppKit main thread; the server posts a Tao user event and drains a main‑thread queue.
