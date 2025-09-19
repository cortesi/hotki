# Hotki World Review

Focus is on robustness, correctness, and sound structure for the world actor and AX read pool.
Items assume the macOS-only deployment target.

1. AX Read Pool Hardening

Sustainability of pooled AX readers is critical when the world service restarts.

1. [x] Rework `ax_read_pool::init` to rebind `world_tx` (or expose a reset path) so hint refresh
        nudges reach respawned worlds; add a regression test that spawns, drops, then respawns the
        world and asserts hints still arrive. `crates/hotki-world/src/ax_read_pool.rs:260`
2. [x] Introduce eviction or invalidation for cached titles/props to avoid unbounded growth and
        stale data, ideally keyed off window removals or an age-based TTL, with a soak test that
        cycles many synthetic windows. `crates/hotki-world/src/ax_read_pool.rs:44-68`

2. Event Pipeline & Reconcile

Window reconciliation should stay cheap while providing rich change signals downstream.

1. [x] Refactor suspect eviction to reuse a single confirmation snapshot rather than calling
        `list_windows_for_spaces` per missing window; cover the change with a perf-focused test or
        benchmark. `crates/hotki-world/src/lib.rs:1337-1354`
        Covered by `world_eviction::confirmation_snapshot_reused_across_suspects`.
2. [x] Replace the placeholder `WindowDelta` with field-level diffs and populate them when
        emitting `WorldEvent::Updated`, updating coalescing and downstream consumers accordingly.
        `crates/hotki-world/src/lib.rs:453-455` `crates/hotki-world/src/lib.rs:1260-1302`
        Exercised via `world_basic::updated_event_includes_field_deltas`.
3. [x] Either repurpose `last_emit` to enforce throttling or remove it and adjust
        `WorldStatus::debounce_cache` so the metric reflects real coalescing state; add coverage
        that exercises the metric. `crates/hotki-world/src/lib.rs:960-1360`
        Verified with `world_basic::status_exposes_debounce_cache_size`.

3. Test Coverage

We should lock down command selection and focus logic with end-to-end style tests.

1. [x] Add integration tests that exercise `placement_guard_reason` across placement and move
        paths, ensuring commands only skip windows when AX roles demand it.
        `crates/hotki-world/src/lib.rs:1528-1536` `crates/hotki-world/src/lib.rs:2007-2026`
        Covered by `world_command_guards::placement_guard_skips_guarded_roles` and
        `world_command_guards::placement_guard_allows_standard_windows`.
2. [x] Extend tests around `RaiseIntent` to verify regex matching, cycling order, and off-space
        fallbacks using `TestWorld`. `crates/hotki-world/src/lib.rs:1702-1760`
        Verified with `world_command_guards::raise_intent_cycles_and_handles_off_space`.
