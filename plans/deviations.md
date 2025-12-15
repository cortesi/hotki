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
