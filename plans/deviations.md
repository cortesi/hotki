# Deviations from `plans/config.md`

This file records intentional deviations from the spec in `plans/config.md` as the implementation
progresses.

## Stage 2

- `DynamicConfig` stores the compiled Rhai `AST` and the original source/path so we can safely call
  stored `FnPtr` closures at render/dispatch time and format errors with excerpts.
- `DynamicConfig` stores `base_theme` + `user_style` separately (instead of a single pre-merged
  `base_style`) so `action.theme_*` and `action.user_style` can be applied dynamically.
- `ActionCtx.push(mode_ref)` without an explicit title currently has no reliable “declared title”
  fallback (it records `None`).

## Stage 3

- `render_stack` returns a `RenderOutput` wrapper so render-time warnings (duplicate chords) can be
  surfaced to the engine for notification delivery.

## Stage 6

- `hotki-server` `set_config_path` temporarily returned a msgpack-encoded
  `config::Config::default()` to keep the existing UI compiling; this was removed in Stage 9 when
  the static-config UI path was deleted.

## Stage 7

- `hotki-protocol::HudState.style` is a full `Style` (HUD + notification config + resolved theme)
  rather than a HUD-only `HudStyle`, so notifications can be styled without additional messages.

## Stage 10

- Built-in theme names remain hyphenated (`dark-blue`, `solarized-dark`, `solarized-light`) as
  implemented in `crates/config/src/themes`, even though `plans/config.md` lists underscore
  variants.
- The UI smoketests no longer fail when no focus-change bridge events are observed during HUD
  activation; focus changes are not guaranteed (the HUD is intentionally non-activating), so we log
  instead. Focus-driven rerendering is covered by engine integration tests.
