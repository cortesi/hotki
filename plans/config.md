# Compact, Composable Configuration

Hotki should remain a Luau program with an ordered menu builder. That model fits dynamic focus
matching, loops, conditional bindings, nested modes, and composite actions better than a static
table schema. The current format is verbose for a different reason: every one-effect binding needs
an action closure, the root is registered through mutable host state, and a growing config cannot
use Luau's module system.

This plan keeps closures as the one execution model while making the common case compact:

```luau
local a = hotki.actions
local wezterm = require("./apps/wezterm")

local GLOBAL = { global = true, hidden = true }
local STAY = { stay = true }

return function(menu, ctx)
    menu:bind("shift+cmd+p", "Run Application", a.select({
        items = hotki.applications,
        on_select = function(select_ctx, item)
            select_ctx:open(item.data.path)
        end,
    }))

    if ctx.hud then
        menu:bind("esc", "Exit", a.exit, GLOBAL)
    end

    menu:submenu("shift+cmd+m", "music", function(music)
        music:bind("k", "vol up", a.hold(a.change_volume(5)), STAY)
        music:bind("j", "vol down", a.hold(a.change_volume(-5)), STAY)
        music:bind("l", "next", a.shell("spotify next"))
        music:bind("m", "mute", a.mute("toggle"))
    end, { capture = true })

    if ctx:app_matches("WezTerm") then
        wezterm(menu, ctx)
    end
end
```

The action helpers above return ordinary `Action` closures. Direct closures remain the escape hatch
for conditionals, multiple ordered effects, and runtime calculations. Required files return normal
Luau values; Hotki does not invent role-specific imports or a second module protocol.

## Design Decisions

1. A behavior entry file returns its root `ModeRenderer`; `hotki.root(...)` is removed.
2. `MenuBuilder` remains imperative and ordered. It is already the right abstraction for dynamic
   menu construction and duplicate-chord diagnostics.
3. `Action` remains exactly `(ctx: ActionContext) -> ()` inside the runtime.
4. `hotki.actions` is an immutable, typed, pure-Luau library whose values and factories return
   `Action` closures. It does not add action userdata or another Rust dispatch path.
5. Direct action closures and every existing `ActionContext` method remain supported.
6. `hotki.actions.hold(action, opts?)` is compact sugar for a closure that calls
   `ctx:until_keyup(action, opts)`. The lower-level method remains available for selective repeat
   inside composite actions.
7. Filesystem-backed configs may use normal `require` with explicit relative requests beginning
   with `./` or `../`. Reject bare names such as `require("util")`; they look package-like but
   Ruau resolves them from the config root rather than the requesting module's directory.
8. Resolution is rooted at the entry file's directory and uses Ruau's `FilesystemSource`; module
   names cannot lexically escape that root. The config directory's contents are trusted, so
   in-root symlinks are followed.
9. Prefer `.luau` modules in docs. Accept the resolver's standard unambiguous `.luau`, `.lua`, and
   `init` candidates instead of maintaining a Hotki-specific resolver. Direct `init` requests,
   ambiguous file/directory candidates, aliases, and `.luaurc` configuration are not supported.
10. The strict checker validates the complete reachable module graph with the same module source
    and global declarations used at runtime. The checked module identities form the execution
    allowlist: a computed request may load only a module already present in that graph.
11. Seal source loading after the entry module returns. A later `require` from a renderer or action
    may return an already-cached module, but an uncached request fails before resolution, file I/O,
    or compilation.
12. Reload creates a fresh VM and module graph. This change does not add file watching or partial
    module reload.
13. `style.luau` remains a separate optional style overlay, not a behavior module convention.
14. This is a clean runtime break. Do not retain `hotki.root`, add a global `action`, or add
    compatibility aliases and migration shims. The checker should emit a targeted migration
    diagnostic for `hotki.root`: return the renderer from `config.luau`.
15. Ruau is inside the project boundary. Prefer a small, general Ruau API improvement over source
    rewriting or Hotki-only VM and resolver workarounds, and validate both repositories together.
16. Prepare and type-check the complete source graph once, then load that exact prepared graph into
    the retained runtime. Do not independently re-read and recompile it for execution.

The initial action library should mirror effect-producing `ActionContext` operations:

```luau
type Actions = {
    pop: Action,
    exit: Action,
    show_root: Action,
    hide_hud: Action,
    reload_config: Action,
    clear_notifications: Action,
    stay: Action,
    notify: (kind: NotifyKind, title: string, body: string) -> Action,
    push: (render: ModeRenderer, title: string?) -> Action,
    shell: (cmd: string, opts: ShellOptions?) -> Action,
    open: (target: string) -> Action,
    relay: (spec: string) -> Action,
    show_details: (toggle: Toggle) -> Action,
    set_volume: (level: number) -> Action,
    change_volume: (delta: number) -> Action,
    mute: (toggle: Toggle) -> Action,
    hold: (action: Action, opts: RepeatOptions?) -> Action,
    select: <T>(spec: SelectorSpec<T>) -> Action,
}
```

Do not add sequence, conditional, mapping, or fallback combinators. Ordinary Luau closures already
express those operations without growing a second language.

`actions.stay` means "suppress automatic mode exit." Keep it for parity with `ActionContext`, but
prefer binding options for static stay behavior and direct closures for conditional stay behavior.

## Alternatives Intentionally Rejected

- Do not replace `MenuBuilder` with nested action tables. An additional schema would duplicate
  Luau's functions and control flow, need its own parser and errors, and risk obscuring binding
  order.
- Do not add `menu:shell`, `menu:relay`, and one menu method per effect. That duplicates the action
  API on every builder and couples binding construction to effect execution.
- Do not accept string commands such as `"reload"` or `{ shell = ... }` as a second `Action` type.
  They would restore the native action-object split that closure-first actions just removed.
- Do not inject short globals such as `a`, `bind`, or `shell`. A config can choose local aliases,
  while the host exposes one organized `hotki` namespace.
- Do not restore Hotki-specific `import_mode`, `import_items`, or `import_handler` helpers. Use the
  standard module graph and ordinary returned values.

## 1. Stage One: Prove the New Boundaries

Validate the risky integration points before changing the checked public surface.

1. [x] Capture the current `hotki api --surface config` output and representative config metrics
       under `tmp/`, including nonblank lines and one-effect closure counts in
       `examples/complete.luau` and `examples/cortesi.luau`.
2. [x] Prototype root-module execution through `RetainedRuntime::step_root_with_context` and the
       existing `CallbackContext`. Decode the protected call as `Function`, create the `ModeRef`,
       synchronize the callback registry, and only then unload the entry root.
3. [x] Prove that nil, table, multiple, and otherwise invalid root returns produce a located
       `config.luau must return a ModeRenderer` error. Pin callback synchronization and root unload
       on both success and every failure path.
4. [x] Prototype the pure-Luau action library through Ruau's native-module support-chunk seam, which
       runs before `VmConfig::untrusted` seals the retained VM. Freeze the returned actions table
       explicitly and prove config code cannot replace the table or its fields.
5. [x] Prove every `Actions` member returns a normal Luau function accepted by `menu:bind`, with no
       new action userdata or `BindingKind` variant.
6. [x] Add a general Ruau native-module binding that installs a support chunk's returned value as a
       declared library member before sandboxing. Use it for `hotki.actions` instead of adding
       Hotki-only trusted-VM mutation or native action factories.
7. [x] Prototype one shared `RootOverlaySource` plus `ruau::fs::FilesystemSource` constructor for
       runtime and checking. Match Ruau's requester-identity and collision-rejection pattern; cover
       nested relative require, cache reuse, cycles, missing and ambiguous modules, and root escape.
8. [x] Prototype a virtual checked entry that contextually constrains the real entry export to
       `ModeRenderer` without rewriting user source or shifting user diagnostics. Add a general
       Ruau expected-export primitive only if the virtual entry cannot preserve inference or errors.
9. [x] Strict-check the headline example verbatim, including the unannotated `a.select` callback
       with `hotki.applications`. If inference fails, settle the selector type shape before Stage
       Three rather than adding misleading annotations after the API is implemented.
10. [x] Wrap the module source narrowly enough to retain every loaded source under Ruau's traceback
        display name. Promote that wrapper into Ruau only if the focused prototype shows it is a
        reusable source capability rather than Hotki diagnostic bookkeeping.
11. [x] Record graph-load gas, retained heap, render time, and one-effect dispatch cost against the
        current 4,000,000 gas, 32 MiB heap, and 5 ms key-processing limits. Set a numeric reload
        budget from the complete and multi-file example graphs.
12. [x] Run focused Ruau crate tests for every new primitive, then run focused `config` crate tests
       against the local Ruau tree and remove temporary harnesses not promoted into permanent tests.
13. [x] Keep Ruau and Hotki changes in separate reviewable batches. Do not commit either repository
       without explicit user approval.
14. [x] Remove any temporary harnesses that are not promoted into permanent tests after recording
       the settled API and measurements in this plan.

## 2. Stage Two: Make the Entry File Return Its Root

Remove registration state and make `config.luau` behave like an ordinary Luau module.

1. [x] Change the public contract so the entry module returns `ModeRenderer`; remove `root` from the
       anonymous `declare hotki` block and remove the `hotki.root` runtime function.
2. [x] Extend `reject_removed_config_surface` in `crates/config/src/check.rs` with a targeted
       `hotki.root was removed; return the renderer from config.luau` diagnostic. Keep this solely
       as checker guidance, with no declaration or runtime compatibility alias.
3. [x] Execute the entry through the retained-runtime path proven in Stage One, validate its single
       returned function, promote it through `CallbackRef`, and store it as the root `ModeRef`.
4. [x] Remove `RuntimeState.root`, `HotkiRoot`, duplicate-root checks, and loader code that reads a
       root out of shared mutable state.
5. [x] Keep `load_dynamic_config_from_string` useful for focused tests by accepting a returned root
       renderer and leaving `require` unavailable when no filesystem origin is supplied.
6. [x] Keep this intermediate stage statically strict by annotating returned root parameters as
       `MenuBuilder` and `ModeContext`. Runtime loading must reject the wrong export type;
       contextual root inference and static export conformance land with graph checking in Stage
       Four.
7. [x] Update runtime tests for root execution errors, root validation renders, retained closure
       lifetime, reload replacement, and garbage collection at the entrypoint boundary.
8. [x] Convert all repository configs, fixtures, embedded test sources, README snippets, and
       `CONFIG.md` examples from registration to annotated returned roots.
9. [x] Remove unused submenu callback parameters where strict Luau already permits shorter
       functions. Keep explicit root parameter types until Stage Four provides contextual inference.
10. [x] Search for `hotki.root` at the end of this stage and keep only the targeted checker test and
        intentional negative runtime tests. All repository configs and tests must be green while
        local action factories still exist.
11. [x] Run `cargo test -p config`, `cargo xtask luau`, and filtered API CLI checks before moving
       to action-library work.

## 3. Stage Three: Add the Typed Action Library

Make one-effect bindings one line without changing closure-first execution semantics.

1. [x] Add the `Actions` type and `actions: Actions` field to the anonymous `declare hotki` block.
       Keep `Action` itself a function type and retain the complete `ActionContext` surface.
2. [x] Put the action library source in one focused config module and install its frozen return
       value through the Ruau source-defined library binding proven in Stage One.
3. [x] Implement constant actions for `pop`, `exit`, `show_root`, `hide_hud`, `reload_config`,
       `clear_notifications`, and `stay`.
4. [x] Implement factories for `notify`, `push`, `shell`, `open`, `relay`, `show_details`,
       `set_volume`, `change_volume`, `mute`, `hold`, and generic `select`.
5. [x] Have every helper call the corresponding `ActionContext` method. Do not duplicate argument
       validation or construct Rust `Action` values in the helper layer.
6. [x] Add table-driven parity tests that exercise every action helper through a real binding and
       prove its effects match the equivalent handwritten closure.
7. [x] Add nested-mutation tests proving the host-provided `hotki` and `hotki.actions` tables and
       constant members are immutable after sandboxing. Keep config-owned factory arguments
       mutable by reference, matching equivalent handwritten closures, and test that behavior.
8. [x] Keep composite examples as direct closures so docs teach that helpers are convenience, not a
       second execution model.
9. [x] Replace config-local `shell`, `relay`, volume, mute, repeat, pop, and exit factories in
       checked examples with `local a = hotki.actions` and the standard helpers.
10. [x] Pin `a.hold` to the same permission rules as `ctx:until_keyup`: held-key activations work,
        while selector callbacks and repeated actions fail with the existing runtime errors.
11. [x] Prove `a.select` defers provider and callback stashing until action dispatch, matching the
        equivalent handwritten closure rather than capturing VM handles at factory creation.
12. [x] Add action-library filtered API tests and ensure `hotki api --filter Actions` returns the
        entire helper table without unrelated style declarations.
13. [x] Run focused checker, runtime, repeat, selector, and engine integration tests before adding
        filesystem modules.

## 4. Stage Four: Enable the Standard Luau Module Graph

Let larger configs split by application or concern without reviving role-specific imports.

1. [x] Use `ruau::fs::FilesystemSource` through Hotki's existing `ruau` dependency and build one
       bounded source rooted at the canonical entry directory. Add a direct crate dependency only
       if the implementation uses `ruau-fs` types not re-exported by `ruau`.
2. [x] Overlay the explicit entry source on that filesystem source so arbitrary entry filenames,
       in-memory test sources, and child-module resolution share one module identity model.
3. [x] Install the module source on the runtime VM so `require` is present only for
       filesystem-backed configs.
4. [x] Reject config requests that do not begin with `./` or `../`, including bare names and
       aliases. Report the rejected request and requesting module path before source resolution.
5. [x] Treat checked module identities as the runtime allowlist during entry evaluation. Reject a
       computed request outside the prepared graph before resolving, reading, or compiling it.
6. [x] Seal module source loading after the entry module returns. Permit late `require` calls only
       when the requested module is already in the runtime cache; reject uncached requests from
       renderers and actions before resolver access, file I/O, or compilation.
7. [x] Replace root-only `check_bytes` and standalone compilation with strict prepared-graph
       validation. Use the Stage One virtual entry for contextual `ModeRenderer` inference, then
       load the exact `PreparedGraphScript` through `RetainedRuntime::load_prepared`.
8. [x] Register the Hotki declaration surface with the checker so every child module sees the same
       typed `hotki` global without prepending declaration text to each source.
9. [x] Remove root-line-offset machinery once the checker no longer concatenates API declarations
       or a type wrapper with user source. Remove the explicit root parameter annotations added in
       Stage Two after the virtual entry proves contextual inference.
10. [x] Cache all graph sources for runtime excerpts and render module-qualified checker diagnostics
       in dependency order with their filesystem display names.
11. [x] Report the number of checked behavior modules in `LuauCheckReport` and the human CLI
        output. Keep the CLI human-oriented rather than adding a generic machine envelope.
12. [x] On reload, build and validate a fresh `DynamicConfig` first. Only after success stop future
       repeated-action ticks, swap the config and runtime state together, and drop the old config,
       whose `Drop` synchronizes callbacks and invalidates the old runtime. Let in-flight callbacks
       finish against the old config or fail closed through Ruau's cross-VM registry ownership.
13. [x] Add checker and runtime tests for nested relative requests, bare-name rejection,
        shared-module caching, returned helper tables and renderers, cycles, missing and ambiguous
        modules, child errors, lexical root escapes, symlink behavior, and computed requests both
        inside and outside the checked graph.
14. [x] Prove a renderer or action can reuse a module cached during entry evaluation without
        source access, while an uncached late request fails before resolver access and leaves no
        newly compiled or cached module behind.
15. [x] Test that a failed reload leaves the previous config active and its retained callbacks
        usable. Test factory-produced submenu renderers with stable chords and changed captures to
        pin the intended `ModeId` and stale-child behavior.
16. [x] Confirm `style.luau` resolution and validation remain independent of behavior modules and
        continue to use the entry file's sibling path.
17. [x] Record whether graph loading stays within the existing gas and heap limits and the numeric
        reload budget. Document that module top-level code runs once during each load or reload.
18. [x] Run focused graph, loader, diagnostics, render, action, and reload tests at the end of the
        stage.

## 5. Stage Five: Teach and Exercise the Compact Format

Make real configs demonstrate the intended shape rather than carrying local compatibility helpers.

1. [x] Rewrite `examples/complete.luau` as a concise returned root that covers every action
       helper, a composite direct closure, a dynamic condition, hold repeat, selector callbacks,
       and options.
2. [x] Split the large Cortesi example into a directory with `config.luau` as its entry plus modules
       for reusable actions and application-specific menus. Compare total nonblank lines with the
       Stage One baseline.
3. [x] Keep one module returning a renderer, one returning an action factory, and one returning data
       so the examples cover normal Luau composition rather than prescribed Hotki roles.
4. [x] Rewrite `CONFIG.md` around entry returns, `hotki.actions`, direct closures, explicit
       relative requests, root containment, graph-bounded loading, cache-only late `require`,
       reload semantics, and child-module diagnostics.
5. [x] Tighten README examples by relying on inferred callback types and omitting unused parameters;
       keep explicit annotations only where they teach a public generic type.
6. [x] Update `DEV.md`, smoketest configs, screenshot fixtures, engine integration sources, and test
       fixtures to the final format with no local action-builder boilerplate.
7. [x] Extend `cargo xtask luau` and `check_validates_all_workspace_examples` to discover entry
       configs and validate multi-file graphs instead of treating every `.luau` file as a root.
8. [x] Define a Markdown fence convention for complete entry configs versus fragments or modules.
       Extract tagged fences into `tmp/` and validate each through the corresponding strict path.
9. [x] Add CLI examples for `hotki check`, `hotki api --filter Actions`, child-module errors,
       rejected bare names, out-of-graph computed requests, and uncached late requests.
10. [x] Search for `hotki.root`, local copies of standard action factories, and stale statements
        that configs are single-file; keep only intentional negative tests.

## 6. Stage Six: Tend the Internal API and Reduce Complexity

Use the new format to remove loader and host abstractions that no longer express the design.

1. [x] Run `ruskel config --private` and inspect the loader, host module, checker, source-cache,
       action-library, and runtime types as one API rather than as isolated implementation files.
2. [x] Rename the remaining shared runtime state around its actual purpose after root registration
       disappears. If it only caches applications, replace `RuntimeState` with a focused cache type.
3. [x] Keep module-source creation in one constructor shared by checking and loading; do not grow
       parallel path-resolution or source-cache implementations.
4. [x] Make root execution, protected-call error conversion, and source lookup use the same narrow
       helpers as child module execution where their contracts match.
5. [x] Audit public `config` crate exports with `ruskel config`. Keep module plumbing private and do
       not expose Ruau resolver, VM, or stashed-closure details through the crate API.
6. [x] Confirm `BindingKind` still has only handler and mode variants and `Effect` remains the only
       handler-output path. Remove any prototype abstraction that violates that invariant.
7. [x] Re-run the Stage One performance probes on the final design and investigate regressions in
       load time, retained VM memory, render churn, or key dispatch before broad validation.

## 7. Stage Seven: Full Validation and Review

Prove the new format across static checking, runtime behavior, UI fixtures, and public
documentation.

1. [x] Run `cargo xtask luau`, including module graphs and extracted Markdown fences.
2. [x] In the Ruau repository, run focused tests for every changed crate plus its repository-native
       format and clippy gates before validating Hotki against that exact local tree.
3. [x] Run focused `hotki check` commands against the complete and multi-file Cortesi examples.
4. [x] Run `hotki api --surface config`, `--filter Actions`, and `--filter ModeRenderer` and inspect
       the output for a compact, coherent public contract.
5. [x] Run `cargo test --all` with an extended timeout.
6. [x] Run
       `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests
       --examples 2>&1` and fix every warning without adding lint allowances.
7. [x] Run `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml` as the final code
       formatting step.
8. [x] Run `cargo run --bin smoketest -- all` with an extended timeout because the real runtime and
       UI fixture configs changed.
9. [x] Run `cargo xtask screenshots` and inspect regenerated output for config-driven UI drift.
10. [x] Run `git diff --check` in every changed repository, review each diff for unrelated changes,
        and update this live checklist with work discovered during validation.
11. [x] Present the settled Ruau and Hotki batches for user review and do not commit either without
        explicit approval.

## Acceptance Criteria

- The entry `config.luau` returns one `ModeRenderer`; no root registration state remains.
- One-effect bindings use `hotki.actions` in a single statement without local wrapper factories.
- Composite and conditional actions remain ordinary closures over `ActionContext`.
- `Action` is still a function type, and runtime binding storage still has one handler path.
- `hotki.actions` is immutable and adds no action userdata, action payload objects, or Rust dispatch
  branch.
- Unused render contexts may be omitted, and selector callback types infer correctly in common use.
- Filesystem-backed configs can require root-contained modules with normal Luau return values by
  using explicit `./` or `../` requests; bare names and aliases are rejected clearly.
- Static checking and runtime loading use the same resolver and prepared graph. Runtime loading
  cannot read or compile a module outside the checked graph.
- Once entry evaluation finishes, renderers and actions can reuse cached modules but cannot trigger
  resolution, file I/O, or compilation through an uncached `require`.
- Removed `hotki.root` use receives a targeted checker migration diagnostic, with no declaration or
  runtime compatibility shim.
- Missing, ambiguous, cyclic, escaping, and invalid modules fail clearly before activation.
- Reload swaps the entire validated graph atomically and cannot retain closures from the old VM.
- A failed reload leaves the previous config and its retained callbacks active.
- `style.luau` behavior is unchanged.
- Any Ruau change is general, documented, covered in Ruau itself, and proven through Hotki as a real
  downstream consumer; Hotki does not carry a private fork-shaped workaround.
- Repository examples contain no copied standard action factories and materially reduce common-case
  ceremony without hiding complex behavior.
- Luau declarations, examples, Markdown fences, Rust tests, clippy, formatting, smoketests, and
  screenshots all pass on the final tree.
