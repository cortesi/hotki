#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use mac_keycode::Chord;

    use crate::{
        Action, Error, Style, load_dynamic_config,
        script::{
            ActionRepeatPermission, Binding, BindingKind, Effect, LoadedConfig, ModeCtx, ModeFrame,
            NavRequest, RenderedState, RepeatSpec, SelectorItems,
            config::{SCRIPT_GAS_LIMIT, SCRIPT_MEMORY_LIMIT},
            handler::{execute_handler, execute_handler_with_permission, execute_selector_handler},
            load_dynamic_config_from_string, render_stack,
        },
    };

    fn test_dir(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("config-script-{name}-{id}"));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale tmp dir");
        }
        fs::create_dir_all(&root).expect("create tmp dir");
        root
    }

    fn base_ctx(app: &str, hud: bool, depth: i64) -> ModeCtx {
        ModeCtx {
            app: app.to_string(),
            title: String::new(),
            pid: 0,
            hud,
            depth,
        }
    }

    fn root_frame(cfg: &LoadedConfig) -> ModeFrame {
        ModeFrame {
            title: "root".to_string(),
            closure: cfg.root(),
            entered_via: None,
            rendered: Vec::new(),
            capture: false,
        }
    }

    fn find_binding<'a>(rendered: &'a RenderedState, ident: &str) -> &'a Binding {
        let chord = Chord::parse(ident).expect("test chord must parse");
        rendered
            .bindings
            .iter()
            .find_map(|(candidate, binding)| {
                if *candidate == chord {
                    Some(binding)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("missing binding ident '{ident}'"))
    }

    fn push_mode(stack: &mut Vec<ModeFrame>, binding: &Binding) {
        let BindingKind::Mode(mode) = binding.kind.clone() else {
            panic!("binding is not a mode entry: {:?}", binding.kind);
        };
        stack.push(ModeFrame {
            title: binding.desc.clone(),
            closure: mode,
            entered_via: binding.mode_id.map(|id| (binding.chord.clone(), id)),
            rendered: Vec::new(),
            capture: binding.mode_capture,
        });
    }

    fn assert_heap_plateaus(baseline_heap: usize, retained_heap: usize) {
        assert!(
            retained_heap <= baseline_heap + 4 * 1024 * 1024,
            "retained heap should plateau; baseline={baseline_heap}, retained={retained_heap}"
        );
    }

    #[test]
    fn config_load_render_and_dispatch_stay_within_budgets() {
        const LOAD_BUDGET: Duration = Duration::from_secs(2);
        const ENTRY_BUDGET: Duration = Duration::from_millis(5);
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

        for relative in ["examples/complete.luau", "examples/cortesi/config.luau"] {
            let path = workspace.join(relative);
            let started = Instant::now();
            let mut config = load_dynamic_config(&path).expect("load measured config");
            let load_time = started.elapsed();
            let heap = config.runtime.heap_used_bytes();
            assert!(
                load_time < LOAD_BUDGET,
                "{relative} load took {load_time:?}"
            );
            assert!(config.entry_gas < SCRIPT_GAS_LIMIT);
            assert!(config.validation_gas < SCRIPT_GAS_LIMIT);
            assert!(heap < SCRIPT_MEMORY_LIMIT);

            let style = config.base_style();
            let ctx = base_ctx("Finder", false, 0);
            let started = Instant::now();
            for _ in 0..100 {
                let mut stack = vec![root_frame(&config)];
                render_stack(&mut config, &mut stack, &ctx, &style)
                    .expect("render measured config");
            }
            let render_time = started.elapsed() / 100;
            assert!(
                render_time < ENTRY_BUDGET,
                "{relative} average render took {render_time:?}"
            );
            eprintln!(
                "{relative}: load={load_time:?} entry_gas={} validation_gas={} heap={heap} render={render_time:?}",
                config.entry_gas, config.validation_gas
            );
        }

        let mut config = load_dynamic_config_from_string(
            "local a = hotki.actions\nreturn function(menu) menu:bind('a', 'stay', a.stay) end",
            None,
        )
        .expect("load dispatch probe");
        let style = config.base_style();
        let ctx = base_ctx("Finder", false, 0);
        let mut stack = vec![root_frame(&config)];
        let rendered =
            render_stack(&mut config, &mut stack, &ctx, &style).expect("render dispatch probe");
        let BindingKind::Handler(handler) = &find_binding(&rendered.rendered, "a").kind else {
            panic!("probe binding is not an action");
        };
        let handler = handler.clone();
        let started = Instant::now();
        for _ in 0..1_000 {
            execute_handler(&mut config, &handler, &ctx).expect("dispatch one-effect action");
        }
        let dispatch_time = started.elapsed() / 1_000;
        assert!(
            dispatch_time < ENTRY_BUDGET,
            "average one-effect dispatch took {dispatch_time:?}"
        );
        assert!(config.runtime.gas_spent() < SCRIPT_GAS_LIMIT);
        eprintln!(
            "one-effect dispatch: average={dispatch_time:?} gas={}",
            config.runtime.gas_spent()
        );
    }

    #[test]
    fn cortesi_application_routes_render_one_expected_menu_each() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cortesi/config.luau");
        let mut config = load_dynamic_config(&path).expect("load Cortesi config");
        let style = config.base_style();

        for (app, route, child_chords) in [
            ("WezTerm", "wezterm", ["r", "s", "t"].as_slice()),
            ("Brave", "brave", ["b"].as_slice()),
            ("Obsidian", "obsidian", ["c", "f", "s", "u", "w"].as_slice()),
        ] {
            let ctx = base_ctx(app, false, 0);
            let mut stack = vec![root_frame(&config)];
            let rendered = render_stack(&mut config, &mut stack, &ctx, &style)
                .expect("render Cortesi application route");
            let route_bindings: Vec<_> = rendered
                .rendered
                .bindings
                .iter()
                .filter(|(_, binding)| {
                    matches!(binding.desc.as_str(), "wezterm" | "brave" | "obsidian")
                })
                .collect();
            assert_eq!(route_bindings.len(), 1, "unexpected routes for {app}");
            let route_binding = &route_bindings[0].1;
            assert_eq!(route_binding.desc, route);
            push_mode(&mut stack, route_binding);

            let child = render_stack(&mut config, &mut stack, &ctx, &style)
                .expect("render Cortesi application submenu");
            for chord in child_chords {
                assert!(
                    child
                        .rendered
                        .bindings
                        .iter()
                        .any(|(candidate, _)| candidate == &Chord::parse(chord).unwrap()),
                    "missing {chord} in {app} route"
                );
            }
        }
    }

    #[test]
    fn path_backed_runtime_errors_report_root_file_excerpt() {
        let root = test_dir("root-runtime-error");
        let root_path = root.join("hotki.luau");
        let root_source = r#"
local missing = nil
missing()

return function(menu, ctx)
end
"#;
        fs::write(&root_path, root_source).expect("write root config");

        let err = match load_dynamic_config_from_string(root_source, Some(root_path.clone())) {
            Ok(_) => panic!("expected imported config load to fail"),
            Err(err) => err,
        };

        match err {
            Error::Validation {
                path,
                line,
                excerpt,
                ..
            } => {
                let path = path.expect("expected root path");
                assert_eq!(path, root_path);
                assert!(line.is_some(), "expected root source line");
                let excerpt = excerpt.expect("expected root source excerpt");
                assert!(
                    excerpt.contains("missing()"),
                    "unexpected excerpt: {excerpt}"
                );
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn entry_module_must_return_exactly_one_renderer() {
        for source in ["return nil", "return {}", "return function() end, true"] {
            let error = match load_dynamic_config_from_string(source, None) {
                Ok(_) => panic!("invalid root return should fail: {source}"),
                Err(error) => error,
            };
            assert!(
                error
                    .pretty()
                    .contains("config.luau must return a ModeRenderer"),
                "unexpected error for {source}: {}",
                error.pretty()
            );
        }
    }

    #[test]
    fn cached_modules_remain_available_after_entry_evaluation() {
        let root = test_dir("cached-late-require");
        let path = root.join("config.luau");
        fs::write(root.join("action.luau"), "return hotki.actions.pop")
            .expect("write action module");
        let source = r#"
local action = require("./action")
return function(menu, ctx)
    local cached = require("./action")
    menu:bind("a", "cached", cached)
end
"#;
        fs::write(&path, source).expect("write root config");

        let mut config = load_dynamic_config_from_string(source, Some(path)).expect("load config");
        let base_style = config.base_style();
        let mut stack = vec![root_frame(&config)];
        let rendered = render_stack(
            &mut config,
            &mut stack,
            &base_ctx("", false, 0),
            &base_style,
        )
        .expect("cached require renders");
        assert_eq!(find_binding(&rendered.rendered, "a").desc, "cached");
    }

    #[test]
    fn uncached_modules_cannot_load_after_entry_evaluation() {
        let root = test_dir("uncached-late-require");
        let path = root.join("config.luau");
        fs::write(root.join("action.luau"), "return hotki.actions.pop")
            .expect("write action module");
        let source = r#"
return function(menu, ctx)
    local action = require("./action")
    menu:bind("a", "uncached", action)
end
"#;
        fs::write(&path, source).expect("write root config");

        let error = match load_dynamic_config_from_string(source, Some(path)) {
            Ok(_) => panic!("late uncached require should fail validation render"),
            Err(error) => error,
        };
        assert!(
            error.pretty().contains("module source is sealed"),
            "unexpected error: {}",
            error.pretty()
        );
    }

    #[test]
    fn chord_parse_errors_include_location() {
        let source = r#"
return function(menu, ctx)
    menu:bind("cmd+bogus", "bad", function(actx)
        actx:shell("true")
    end)
end
"#;
        let err = match load_dynamic_config_from_string(source, None) {
            Ok(_) => panic!("expected config load to fail"),
            Err(err) => err,
        };
        match err {
            Error::Validation {
                line,
                excerpt,
                message,
                ..
            } => {
                assert!(
                    message.contains("invalid chord string"),
                    "unexpected message: {message}"
                );
                assert!(line.is_some(), "expected a source line");
                assert!(excerpt.is_some(), "expected an excerpt");
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_chord_warns_and_first_wins() {
        let source = r#"
return function(menu, ctx)
    menu:bind("a", "first", function(actx)
        actx:shell("true")
    end)
    menu:bind("a", "second", function(actx)
        actx:shell("true")
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", false, 0);
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");

        assert_eq!(out.warnings.len(), 1);
        assert!(matches!(
            out.warnings[0],
            Effect::Notify {
                kind: crate::NotifyKind::Warn,
                ..
            }
        ));
        assert_eq!(find_binding(&out.rendered, "a").desc, "first");
    }

    #[test]
    fn menu_with_merges_defaults_without_mutating_shared_order() {
        let source = r#"
return function(menu, ctx)
    local inherited = menu:with({ hidden = true, global = true, stay = true })
    inherited:bind("a", "overrides", function(actx)
    end, { hidden = false, stay = false })
    inherited:submenu("b", "child", function(child, inner)
        child:bind("x", "child binding", function(actx)
        end)
    end, { hidden = false, global = false, capture = true })
    local nested = inherited:with({ stay = false })
    nested:bind("c", "nested", function(actx)
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let root = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", false, 0),
            &base_style,
        )
        .expect("render root");

        let a = find_binding(&root.rendered, "a");
        assert!(!a.flags.hidden);
        assert!(a.flags.global);
        assert!(!a.flags.stay);

        let b = find_binding(&root.rendered, "b").clone();
        assert!(!b.flags.hidden);
        assert!(!b.flags.global);
        assert!(b.flags.stay);
        assert!(b.mode_capture);

        let c = find_binding(&root.rendered, "c");
        assert!(c.flags.hidden);
        assert!(c.flags.global);
        assert!(!c.flags.stay);

        push_mode(&mut stack, &b);
        let child = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", true, 1),
            &base_style,
        )
        .expect("render child");
        let x = find_binding(&child.rendered, "x");
        assert!(!x.flags.hidden);
        assert!(!x.flags.global);
        assert!(!x.flags.stay);
        assert!(child.rendered.capture);
    }

    #[test]
    fn menu_with_preserves_unknown_field_and_receiver_diagnostics() {
        for source in [
            r#"
return function(menu, ctx)
    menu:with({ repeat = true })
end
"#,
            r#"
return function(menu, ctx)
    local value = {}
    value:with({ stay = true })
end
"#,
        ] {
            let err = match load_dynamic_config_from_string(source, None) {
                Ok(_) => panic!("invalid menu:with use should fail"),
                Err(err) => err,
            };
            let pretty = err.pretty();
            assert!(
                pretty.contains("with") || pretty.contains("repeat"),
                "{pretty}"
            );
        }
    }

    #[test]
    fn auto_pop_truncates_empty_child_modes() {
        let source = r#"
return function(menu, ctx)
    menu:submenu("a", "child", function(child, inner)
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", false, 0),
            &base_style,
        )
        .expect("render root");
        let mode_entry = find_binding(&out.rendered, "a").clone();
        push_mode(&mut stack, &mode_entry);
        let _ = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", true, 1),
            &base_style,
        )
        .expect("render child");
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn orphan_detection_pops_when_mode_identity_changes() {
        let source = r#"
return function(menu, ctx)
    if ctx:app_matches("A") then
        menu:submenu("a", "child-a", function(child, inner)
            child:bind("x", "x", function(actx)
                actx:shell("true")
            end)
        end)
    else
        menu:submenu("a", "child-b", function(child, inner)
            child:bind("y", "y", function(actx)
                actx:shell("true")
            end)
        end)
    end
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];

        let out_a = render_stack(&mut cfg, &mut stack, &base_ctx("A", true, 0), &base_style)
            .expect("render A");
        let entry = find_binding(&out_a.rendered, "a").clone();
        push_mode(&mut stack, &entry);

        let _ = render_stack(&mut cfg, &mut stack, &base_ctx("B", true, 1), &base_style)
            .expect("render B");
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn path_backed_conditional_submenu_branch_renders() {
        let root = test_dir("conditional-submenu-branch");
        let root_path = root.join("hotki.luau");
        let source = r#"
return function(menu, ctx)
    if ctx:app_matches("A") then
        menu:submenu("a", "child-a", function(child, inner)
            child:bind("x", "x", function(actx)
                actx:shell("true")
            end)
        end)
    elseif ctx:app_matches("B") then
        menu:submenu("b", "child-b", function(child, inner)
            child:bind("y", "y", function(actx)
                actx:shell("true")
            end)
        end)
    end
end
"#;
        fs::write(&root_path, source).expect("write root config");
        let mut cfg = load_dynamic_config_from_string(source, Some(root_path)).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];

        let out = render_stack(&mut cfg, &mut stack, &base_ctx("A", true, 0), &base_style)
            .expect("render A");

        assert_eq!(find_binding(&out.rendered, "a").desc, "child-a");
    }

    #[test]
    fn handler_effects_preserve_enqueue_order() {
        let source = r#"
return function(menu, ctx)
    menu:bind("h", "handler", function(actx)
        actx:shell("echo one")
        actx:notify("info", "Test", "middle")
        actx:shell("echo two")
        actx:pop()
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", true, 0);
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let binding = find_binding(&out.rendered, "h");
        let BindingKind::Handler(handler) = &binding.kind else {
            panic!("expected handler binding");
        };

        let result = execute_handler(&mut cfg, handler, &ctx).expect("execute handler");
        assert_eq!(result.effects.len(), 4);
        match &result.effects[0] {
            Effect::Exec(Action::Shell(spec)) => assert_eq!(spec.command(), "echo one"),
            other => panic!("unexpected first effect: {other:?}"),
        }
        match &result.effects[1] {
            Effect::Notify { kind, title, body } => {
                assert_eq!(kind, &crate::NotifyKind::Info);
                assert_eq!(title, "Test");
                assert_eq!(body, "middle");
            }
            other => panic!("unexpected second effect: {other:?}"),
        }
        match &result.effects[2] {
            Effect::Exec(Action::Shell(spec)) => assert_eq!(spec.command(), "echo two"),
            other => panic!("unexpected third effect: {other:?}"),
        }
        assert!(matches!(result.effects[3], Effect::Nav(_)));
    }

    #[test]
    fn action_library_matches_direct_context_calls_and_is_immutable() {
        let source = r#"
local a = hotki.actions
if pcall(function() hotki.actions = {} end) then
    error("hotki table accepted replacement")
end
if pcall(function() a.pop = function() end end) then
    error("actions table accepted mutation")
end

local selector_spec = {
    items = { "before" },
    on_select = function(ctx, item, query) end,
}
local select_action = a.select(selector_spec)
selector_spec.items[1] = "after"

return function(menu, ctx)
    menu:bind("a", "helper", a.pop)
    menu:bind("shift+a", "direct", function(c) c:pop() end)
    menu:bind("b", "helper", a.exit)
    menu:bind("shift+b", "direct", function(c) c:exit() end)
    menu:bind("c", "helper", a.show_root)
    menu:bind("shift+c", "direct", function(c) c:show_root() end)
    menu:bind("d", "helper", a.hide_hud)
    menu:bind("shift+d", "direct", function(c) c:hide_hud() end)
    menu:bind("e", "helper", a.reload_config)
    menu:bind("shift+e", "direct", function(c) c:reload_config() end)
    menu:bind("f", "helper", a.clear_notifications)
    menu:bind("shift+f", "direct", function(c) c:clear_notifications() end)
    menu:bind("g", "helper", a.stay)
    menu:bind("shift+g", "direct", function(c) c:stay() end)
    menu:bind("h", "helper", a.notify("info", "title", "body"))
    menu:bind("shift+h", "direct", function(c) c:notify("info", "title", "body") end)
    menu:bind("i", "helper", a.shell("echo hi", { ok_notify = "success" }))
    menu:bind("shift+i", "direct", function(c)
        c:shell("echo hi", { ok_notify = "success" })
    end)
    menu:bind("j", "helper", a.open("https://example.com"))
    menu:bind("shift+j", "direct", function(c) c:open("https://example.com") end)
    menu:bind("k", "helper", a.relay("cmd+c"))
    menu:bind("shift+k", "direct", function(c) c:relay("cmd+c") end)
    menu:bind("l", "helper", a.show_details("toggle"))
    menu:bind("shift+l", "direct", function(c) c:show_details("toggle") end)
    menu:bind("m", "helper", a.set_volume(50))
    menu:bind("shift+m", "direct", function(c) c:set_volume(50) end)
    menu:bind("n", "helper", a.change_volume(-5))
    menu:bind("shift+n", "direct", function(c) c:change_volume(-5) end)
    menu:bind("o", "helper", a.mute("toggle"))
    menu:bind("shift+o", "direct", function(c) c:mute("toggle") end)
    menu:bind("p", "push", a.push(function() end, "child"))
    menu:bind("q", "hold", a.hold(a.change_volume(5), {
        delay_ms = 10,
        interval_ms = 20,
    }))
    menu:bind("r", "select", select_action)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load action library");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");

        for chord in 'a'..='o' {
            let helper = chord.to_string();
            let direct = format!("shift+{chord}");
            let BindingKind::Handler(helper) = find_binding(&out.rendered, &helper).kind.clone()
            else {
                panic!("expected helper handler for {chord}");
            };
            let BindingKind::Handler(direct) = find_binding(&out.rendered, &direct).kind.clone()
            else {
                panic!("expected direct handler for {chord}");
            };
            let helper = execute_handler(&mut cfg, &helper, &ctx).expect("run helper");
            let direct = execute_handler(&mut cfg, &direct, &ctx).expect("run direct");
            assert_eq!(
                format!("{:?}", helper.effects),
                format!("{:?}", direct.effects)
            );
            assert_eq!(helper.stay, direct.stay);
        }

        let BindingKind::Handler(push) = find_binding(&out.rendered, "p").kind.clone() else {
            panic!("expected push handler");
        };
        let pushed = execute_handler(&mut cfg, &push, &ctx).expect("run push helper");
        assert!(matches!(
            &pushed.effects[0],
            Effect::Nav(NavRequest::Push { title, .. })
                if title.as_deref() == Some("child")
        ));

        let BindingKind::Handler(hold) = find_binding(&out.rendered, "q").kind.clone() else {
            panic!("expected hold handler");
        };
        let held = execute_handler(&mut cfg, &hold, &ctx).expect("run hold helper");
        assert!(matches!(
            &held.effects[0],
            Effect::UntilKeyUp {
                repeat: Some(RepeatSpec {
                    delay_ms: Some(10),
                    interval_ms: Some(20),
                }),
                ..
            }
        ));
        for permission in [
            ActionRepeatPermission::Keyless,
            ActionRepeatPermission::RepeatedAction,
        ] {
            let error = execute_handler_with_permission(&mut cfg, &hold, &ctx, permission)
                .expect_err("hold should preserve until-keyup permission checks");
            assert!(
                error.pretty().contains("until_keyup"),
                "unexpected hold permission error: {}",
                error.pretty()
            );
        }

        let BindingKind::Handler(select) = find_binding(&out.rendered, "r").kind.clone() else {
            panic!("expected select handler");
        };
        let selected = execute_handler(&mut cfg, &select, &ctx).expect("run select helper");
        let Effect::Select(selector) = &selected.effects[0] else {
            panic!("expected select effect: {:?}", selected.effects);
        };
        let SelectorItems::Static(items) = &selector.items else {
            panic!("expected static selector items");
        };
        assert_eq!(items[0].label, "after");
    }

    #[test]
    fn new_action_helpers_preserve_capture_and_direct_call_parity() {
        let source = r#"
local a = hotki.actions
local exec_options = {
    program = "printf",
    args = { "%s", "before" },
    ok_notify = "success",
}
local exec_action = a.exec(exec_options)
exec_options.args[2] = "after"
local with_separator = a.relay_with("cmd+shift++")
local without_separator = a.relay_with("cmd+shift")
local youtube_music = a.relay_to_app("YouTube Music")
local launch_options = {
    title = "Apps",
    placeholder = "Find apps",
    max_visible = 4,
}
local launch_action = a.launch_application(launch_options)
launch_options.title = "Changed"

return function(menu, ctx)
    menu:bind("a", "exec helper", exec_action)
    menu:bind("shift+a", "exec direct", function(c)
        c:exec({
            program = "printf",
            args = { "%s", "after" },
            ok_notify = "success",
        })
    end)
    menu:bind("b", "relay helper", with_separator("6"))
    menu:bind("shift+b", "relay direct", function(c) c:relay("cmd+shift+6") end)
    menu:bind("c", "relay helper without separator", without_separator("++7"))
    menu:bind("d", "launch", launch_action)
    menu:bind("e", "targeted relay helper", youtube_music("shift+="))
    menu:bind("shift+e", "targeted relay direct", function(c)
        c:relay_to_app("YouTube Music", "shift+=")
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load action helpers");
        let style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &style).expect("render helpers");

        let BindingKind::Handler(exec_helper) = find_binding(&out.rendered, "a").kind.clone()
        else {
            panic!("expected exec helper handler");
        };
        let BindingKind::Handler(exec_direct) = find_binding(&out.rendered, "shift+a").kind.clone()
        else {
            panic!("expected direct exec handler");
        };
        let helper = execute_handler(&mut cfg, &exec_helper, &ctx).expect("run exec helper");
        let direct = execute_handler(&mut cfg, &exec_direct, &ctx).expect("run direct exec");
        assert_eq!(
            format!("{:?}", helper.effects),
            format!("{:?}", direct.effects)
        );
        match &helper.effects[0] {
            Effect::Exec(Action::Exec(spec)) => {
                assert_eq!(spec.program, "printf");
                assert_eq!(spec.args.as_ref().expect("args"), &["%s", "after"]);
                assert_eq!(spec.ok_notify, crate::NotifyKind::Success);
                assert_eq!(spec.err_notify, crate::NotifyKind::Warn);
            }
            other => panic!("unexpected exec effect: {other:?}"),
        }

        for (helper_chord, expected) in [("b", "cmd+shift+6"), ("c", "cmd+shift+7")] {
            let BindingKind::Handler(handler) =
                find_binding(&out.rendered, helper_chord).kind.clone()
            else {
                panic!("expected relay helper handler");
            };
            let result = execute_handler(&mut cfg, &handler, &ctx).expect("run relay helper");
            assert!(matches!(
                &result.effects[0],
                Effect::Exec(Action::Relay(spec))
                    if spec == &crate::RelaySpec::focused(expected)
            ));
        }

        for chord in ["e", "shift+e"] {
            let BindingKind::Handler(handler) = find_binding(&out.rendered, chord).kind.clone()
            else {
                panic!("expected targeted relay handler");
            };
            let result = execute_handler(&mut cfg, &handler, &ctx).expect("run targeted relay");
            assert!(matches!(
                &result.effects[0],
                Effect::Exec(Action::Relay(spec))
                    if spec == &crate::RelaySpec::application("YouTube Music", "shift+=")
            ));
        }

        let BindingKind::Handler(launch) = find_binding(&out.rendered, "d").kind.clone() else {
            panic!("expected launcher handler");
        };
        let result = execute_handler(&mut cfg, &launch, &ctx).expect("run launcher");
        let Effect::Select(selector) = &result.effects[0] else {
            panic!("expected selector effect: {:?}", result.effects);
        };
        assert_eq!(selector.title, "Changed");
        assert_eq!(selector.placeholder, "Find apps");
        assert_eq!(selector.max_visible, 4);
        assert!(matches!(selector.items, SelectorItems::Provider(_)));
    }

    #[test]
    fn targeted_relay_rejects_an_empty_application_name() {
        let source = r#"
return function(menu, ctx)
    menu:bind("a", "bad target", function(c)
        c:relay_to_app("", "space")
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load config");
        let style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &style).expect("render config");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "a").kind.clone() else {
            panic!("expected handler");
        };

        let error = execute_handler(&mut cfg, &handler, &ctx).expect_err("empty name rejected");
        assert!(error.pretty().contains("app_name must not be empty"));
    }

    #[test]
    fn renderer_helpers_are_ordered_contextual_and_immutable() {
        let source = r#"
local r = hotki.renderers
if pcall(function() r.combine = function() end end) then
    error("renderers table accepted mutation")
end

local function base(menu, ctx)
    menu:bind("a", "base", function(c) end)
end

return r.combine(
    base,
    r.when_app("Finder", function(menu, ctx)
        menu:bind("b", "exact", function(c) end)
    end),
    r.when_app_matches("Find", function(menu, ctx)
        menu:bind("b", "regex", function(c) end)
    end)
)
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load renderers");
        let style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let finder = render_stack(&mut cfg, &mut stack, &base_ctx("Finder", true, 0), &style)
            .expect("render Finder");
        assert_eq!(finder.rendered.bindings.len(), 2);
        assert_eq!(finder.rendered.bindings[0].1.desc, "base");
        assert_eq!(finder.rendered.bindings[1].1.desc, "exact");
        assert_eq!(finder.warnings.len(), 1, "all matching routes should run");
        assert!(finder.rendered.bindings[0].1.pos.is_some());

        let other = render_stack(&mut cfg, &mut stack, &base_ctx("Safari", true, 0), &style)
            .expect("rerender Safari");
        assert_eq!(other.rendered.bindings.len(), 1);
        assert_eq!(other.rendered.bindings[0].1.desc, "base");
        assert!(other.warnings.is_empty());
    }

    #[test]
    fn exec_parser_is_strict_and_effects_keep_source_order() {
        let source = r#"
return function(menu, ctx)
    menu:bind("e", "exec", function(actx)
        actx:exec({
            program = "/usr/bin/printf",
            args = { "%s", "hello" },
            cwd = ".",
            ok_notify = "success",
            err_notify = "error",
        })
        actx:notify("info", "middle", "body")
        actx:pop()
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load exec config");
        let style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &style).expect("render exec config");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "e").kind.clone() else {
            panic!("expected exec handler");
        };
        let result = execute_handler(&mut cfg, &handler, &ctx).expect("execute exec handler");
        assert_eq!(result.effects.len(), 3);
        match &result.effects[0] {
            Effect::Exec(Action::Exec(spec)) => {
                assert_eq!(spec.program, "/usr/bin/printf");
                assert_eq!(spec.args.as_ref().expect("args"), &["%s", "hello"]);
                assert_eq!(spec.cwd.as_deref(), Some("."));
                assert_eq!(spec.ok_notify, crate::NotifyKind::Success);
                assert_eq!(spec.err_notify, crate::NotifyKind::Error);
            }
            other => panic!("unexpected first effect: {other:?}"),
        }
        assert!(matches!(result.effects[1], Effect::Notify { .. }));
        assert!(matches!(result.effects[2], Effect::Nav(_)));

        let unknown = r#"
return function(menu, ctx)
    menu:bind("e", "exec", function(actx)
        actx:exec({ program = "true", unknown = true })
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(unknown, None).expect("load unknown config");
        let style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &style).expect("render unknown config");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "e").kind.clone() else {
            panic!("expected unknown-field handler");
        };
        let error = execute_handler(&mut cfg, &handler, &ctx).expect_err("unknown field rejected");
        assert!(
            error.pretty().contains("unknown"),
            "unexpected error: {}",
            error.pretty()
        );
    }

    #[test]
    fn replaced_render_callbacks_are_released_and_stale_handles_fail_closed() {
        let source = r#"
return function(menu, ctx)
    menu:bind("h", "handler", function(actx)
        actx:notify("info", "live", "")
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];

        let first = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("first render");
        drop(first);
        let second = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("second render");
        drop(second);

        let invalidated = cfg.runtime.invalidate();
        assert_eq!(
            invalidated.roots, 0,
            "config root chunk is unloaded after load"
        );
        assert_eq!(
            invalidated.functions, 2,
            "only the root renderer and current binding remain retained"
        );

        let error = render_stack(&mut cfg, &mut stack, &ctx, &base_style)
            .expect_err("invalidated renderer must be stale");
        assert!(
            error.to_string().contains("stale retained Function handle"),
            "unexpected stale-handle error: {error}"
        );
    }

    #[test]
    fn callback_contexts_are_isolated_between_configs() {
        let source = |title: &str| {
            format!(
                r#"
return function(menu, ctx)
    menu:bind("h", "handler", function(actx)
        actx:notify("info", "{title}", "")
    end)
end
"#
            )
        };
        let mut left =
            load_dynamic_config_from_string(&source("left"), None).expect("load left cfg");
        let mut right =
            load_dynamic_config_from_string(&source("right"), None).expect("load right cfg");
        let ctx = base_ctx("TestApp", true, 0);
        let mut left_stack = vec![root_frame(&left)];
        let mut right_stack = vec![root_frame(&right)];
        let left_rendered =
            render_stack(&mut left, &mut left_stack, &ctx, &Style::default()).expect("render left");
        let right_rendered = render_stack(&mut right, &mut right_stack, &ctx, &Style::default())
            .expect("render right");
        let BindingKind::Handler(left_handler) =
            find_binding(&left_rendered.rendered, "h").kind.clone()
        else {
            panic!("expected left handler");
        };
        let BindingKind::Handler(right_handler) =
            find_binding(&right_rendered.rendered, "h").kind.clone()
        else {
            panic!("expected right handler");
        };

        let error = execute_handler(&mut right, &left_handler, &ctx)
            .expect_err("foreign callback must not resolve in another config");
        assert!(
            error.to_string().contains("cross-VM host registry pin"),
            "unexpected isolation error: {error}"
        );

        let left_result = execute_handler(&mut left, &left_handler, &ctx).expect("run left");
        let right_result = execute_handler(&mut right, &right_handler, &ctx).expect("run right");
        assert!(matches!(
            &left_result.effects[0],
            Effect::Notify { title, .. } if title == "left"
        ));
        assert!(matches!(
            &right_result.effects[0],
            Effect::Notify { title, .. } if title == "right"
        ));
    }

    #[test]
    fn handler_failure_does_not_poison_later_events() {
        let source = r#"
return function(menu, ctx)
    menu:bind("b", "bad", function(actx)
        error("expected failure")
    end)
    menu:bind("g", "good", function(actx)
        actx:notify("success", "recovered", "")
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let rendered =
            render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render handlers");
        let BindingKind::Handler(bad) = find_binding(&rendered.rendered, "b").kind.clone() else {
            panic!("expected bad handler");
        };
        let BindingKind::Handler(good) = find_binding(&rendered.rendered, "g").kind.clone() else {
            panic!("expected good handler");
        };

        let error = execute_handler(&mut cfg, &bad, &ctx).expect_err("bad handler fails");
        assert!(error.to_string().contains("expected failure"));
        let result = execute_handler(&mut cfg, &good, &ctx).expect("later handler recovers");
        assert!(matches!(
            &result.effects[0],
            Effect::Notify { title, .. } if title == "recovered"
        ));
    }

    #[test]
    fn removed_action_global_fails_normally() {
        let source = r#"
return function(menu, ctx)
    menu:bind("h", "handler", action.reload_config)
end
"#;
        let err = match load_dynamic_config_from_string(source, None) {
            Ok(_) => panic!("load should fail"),
            Err(err) => err,
        };
        let pretty = err.pretty();
        assert!(pretty.contains("action"), "unexpected error: {pretty}");
    }

    #[test]
    fn removed_ctx_exec_fails_normally() {
        let source = r#"
return function(menu, ctx)
    menu:bind("h", "handler", function(actx)
        actx:exec(function(inner)
        end)
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", true, 0);
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = &find_binding(&out.rendered, "h").kind else {
            panic!("expected handler binding");
        };

        let err = execute_handler(&mut cfg, handler, &ctx).expect_err("execute should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("exec"), "unexpected error: {pretty}");
    }

    #[test]
    fn selector_action_queues_selector_effect() {
        let source = r#"
return function(menu, ctx)
    menu:bind("a", "Selector", function(actx)
        actx:select({
            title = "Run",
            placeholder = "Search...",
            items = {
                "Safari",
                { label = "Chrome", sublabel = "/Applications/Chrome.app", data = 123 },
            },
            on_select = function(select_ctx, item, query)
            end,
        })
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", false, 0),
            &base_style,
        )
        .expect("render");
        let binding = find_binding(&out.rendered, "a");
        let BindingKind::Handler(handler) = &binding.kind else {
            panic!("expected handler binding");
        };
        let result = execute_handler(&mut cfg, handler, &base_ctx("TestApp", false, 0))
            .expect("execute selector action");
        let Effect::Select(selector) = &result.effects[0] else {
            panic!("expected selector effect: {:?}", result.effects);
        };
        assert_eq!(selector.title, "Run");
        assert_eq!(selector.placeholder, "Search...");
        let SelectorItems::Static(items) = &selector.items else {
            panic!("expected static items");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Safari");
        assert_eq!(
            items[1].sublabel.as_deref(),
            Some("/Applications/Chrome.app")
        );
    }

    #[test]
    fn binding_options_reject_repeat_field() {
        let source = r#"
return function(menu, ctx)
    menu:submenu("a", "child", function(child, inner)
        child:bind("x", "x", function(actx)
            actx:shell("true")
        end)
    end, {
        global = true,
        capture = true,
        ["repeat"] = {
            delay_ms = 1,
            interval_ms = 2,
        },
    })
end
"#;
        let err = match load_dynamic_config_from_string(source, None) {
            Ok(_) => panic!("load should fail"),
            Err(err) => err,
        };
        let pretty = err.pretty();
        assert!(pretty.contains("repeat"), "unexpected error: {pretty}");
    }

    #[test]
    fn until_keyup_queues_repeated_action_effect() {
        let source = r#"
return function(menu, ctx)
    menu:bind("r", "repeat", function(actx)
        actx:until_keyup(function(repeat_ctx)
            repeat_ctx:shell("echo tick")
        end, {
            delay_ms = 125,
            interval_ms = 250,
        })
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "r").kind.clone() else {
            panic!("expected handler binding");
        };

        let result = execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        let Effect::UntilKeyUp { repeat, .. } = &result.effects[0] else {
            panic!("expected until-keyup effect: {:?}", result.effects);
        };
        let repeat = (*repeat).expect("repeat spec");
        assert_eq!(repeat.delay_ms, Some(125));
        assert_eq!(repeat.interval_ms, Some(250));
    }

    #[test]
    fn until_keyup_rejects_second_request_in_same_action() {
        let source = r#"
return function(menu, ctx)
    menu:bind("r", "repeat", function(actx)
        actx:until_keyup(function(repeat_ctx)
            repeat_ctx:shell("echo one")
        end)
        actx:until_keyup(function(repeat_ctx)
            repeat_ctx:shell("echo two")
        end)
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "r").kind.clone() else {
            panic!("expected handler binding");
        };

        let err = execute_handler(&mut cfg, &handler, &ctx).expect_err("execute should fail");
        let pretty = err.pretty();
        assert!(
            pretty.contains("only be called once"),
            "unexpected error: {pretty}"
        );
    }

    #[test]
    fn action_context_rejects_use_after_handler_returns() {
        let source = r#"
local stored: ActionContext? = nil

return function(menu, ctx)
    menu:bind("s", "store", function(actx)
        stored = actx
    end)
    menu:bind("u", "use", function(actx)
        if stored ~= nil then
            stored:notify("info", "late", "")
        end
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(store) = find_binding(&out.rendered, "s").kind.clone() else {
            panic!("expected store handler");
        };
        let BindingKind::Handler(use_stored) = find_binding(&out.rendered, "u").kind.clone() else {
            panic!("expected use handler");
        };

        execute_handler(&mut cfg, &store, &ctx).expect("store context");
        let err = execute_handler(&mut cfg, &use_stored, &ctx).expect_err("execute should fail");
        let pretty = err.pretty();
        assert!(
            pretty.contains("no longer valid"),
            "unexpected error: {pretty}"
        );
    }

    #[test]
    fn selector_handler_rejects_until_keyup_without_held_key() {
        let source = r#"
return function(menu, ctx)
    menu:bind("s", "selector", function(actx)
        actx:select({
            items = { "One" },
            on_select = function(select_ctx, item, query)
                select_ctx:until_keyup(function(repeat_ctx)
                    repeat_ctx:shell("echo tick")
                end)
            end,
        })
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "s").kind.clone() else {
            panic!("expected handler binding");
        };
        let result = execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        let Effect::Select(selector) = &result.effects[0] else {
            panic!("expected selector effect: {:?}", result.effects);
        };
        let SelectorItems::Static(items) = &selector.items else {
            panic!("expected static selector items");
        };
        let item = items[0].clone();
        let on_select = selector.on_select.clone();

        let err = execute_selector_handler(&mut cfg, &on_select, &ctx, &item, "")
            .expect_err("selector handler should fail");
        let pretty = err.pretty();
        assert!(
            pretty.contains("requires a held triggering key"),
            "unexpected error: {pretty}"
        );
    }

    #[test]
    fn repeated_action_rejects_nested_until_keyup() {
        let source = r#"
return function(menu, ctx)
    menu:bind("r", "repeat", function(actx)
        actx:until_keyup(function(repeat_ctx)
            repeat_ctx:shell("echo tick")
        end)
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", true, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "r").kind.clone() else {
            panic!("expected handler binding");
        };

        let err = execute_handler_with_permission(
            &mut cfg,
            &handler,
            &ctx,
            ActionRepeatPermission::RepeatedAction,
        )
        .expect_err("execute should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("nested"), "unexpected error: {pretty}");
    }

    #[test]
    fn submenu_volume_actions_accept_integer_number_literals() {
        let source = r#"
return function(menu, ctx)
    menu:submenu("m", "music", function(music, inner)
        music:bind("k", "vol up", function(actx)
            actx:change_volume(5)
        end)
        music:bind("j", "vol down", function(actx)
            actx:change_volume(-5)
        end)
        music:bind("1", "set volume", function(actx)
            actx:set_volume(50)
        end)
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let mut stack = vec![root_frame(&cfg)];
        let root = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", false, 0),
            &base_style,
        )
        .expect("render root");
        let binding = find_binding(&root.rendered, "m").clone();
        push_mode(&mut stack, &binding);

        let ctx = base_ctx("TestApp", true, 1);
        let child = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render child");
        assert_handler_execs(
            &mut cfg,
            &child.rendered,
            "k",
            &ctx,
            &Action::ChangeVolume(5),
        );
        assert_handler_execs(
            &mut cfg,
            &child.rendered,
            "j",
            &ctx,
            &Action::ChangeVolume(-5),
        );
        assert_handler_execs(&mut cfg, &child.rendered, "1", &ctx, &Action::SetVolume(50));
    }

    #[test]
    fn render_uses_resolved_base_style_without_row_overrides() {
        let source = r##"
return function(menu, ctx)
    menu:submenu("a", "child", function(child, inner)
        child:bind("x", "x", function(actx)
            actx:shell("true")
        end)
    end)
end
"##;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let mut base_style = cfg.base_style();
        base_style.hud.bg = (1, 2, 3);
        let mut stack = vec![root_frame(&cfg)];
        let out_root = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", true, 0),
            &base_style,
        )
        .expect("render root");
        let entry = find_binding(&out_root.rendered, "a").clone();
        push_mode(&mut stack, &entry);
        let out_child = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", true, 1),
            &base_style,
        )
        .expect("render child");

        assert_eq!(out_child.rendered.style.hud.bg, (1, 2, 3));
        let row = out_child
            .rendered
            .hud_rows
            .iter()
            .find(|row| row.chord.to_string() == "x")
            .expect("x row");
        assert_eq!(row.style, None);
    }

    #[test]
    fn execution_budget_resets_between_renders() {
        let source = r#"
return function(menu, ctx)
    local total = 0
    for i = 1, 20000 do
        total = total + i
    end

    if total > 0 then
        menu:bind("a", "loop", function(actx)
            actx:shell("true")
        end)
    end
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", false, 0);

        for _ in 0..32 {
            let mut stack = vec![root_frame(&cfg)];
            let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
            assert_eq!(find_binding(&out.rendered, "a").desc, "loop");
        }
    }

    #[test]
    fn handler_execution_reclaims_temporary_selector_tables_between_calls() {
        let source = r#"
local function synthetic_applications()
    local items = {}
    local payload = string.rep("x", 2048)
    for i = 1, 768 do
        items[i] = {
            label = "Application " .. i,
            sublabel = payload .. i,
            data = { path = payload .. i },
        }
    end
    return items
end

return function(menu, ctx)
    menu:bind("a", "Selector", function(actx)
        actx:select({
            title = "Run Application",
            items = synthetic_applications(),
            on_select = function(select_ctx, item, query)
            end,
        })
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", false, 0);
        let mut stack = vec![root_frame(&cfg)];

        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "a").kind.clone() else {
            panic!("expected handler binding");
        };
        let result = execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        assert_selector_len(&result.effects, 768);
        let baseline_heap = cfg.runtime.heap_used_bytes();

        for _ in 0..24 {
            let result = execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
            assert_selector_len(&result.effects, 768);
        }

        let retained_heap = cfg.runtime.heap_used_bytes();
        assert_heap_plateaus(baseline_heap, retained_heap);
    }

    #[test]
    fn handler_execution_reclaims_temporary_tables_between_calls() {
        let source = r#"
return function(menu, ctx)
    menu:bind("h", "Handler", function(actx)
        local scratch = {}
        local payload = string.rep("x", 2048)
        for i = 1, 768 do
            scratch[i] = { path = payload .. i }
        end
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", false, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "h").kind.clone() else {
            panic!("expected handler binding");
        };

        execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        let baseline_heap = cfg.runtime.heap_used_bytes();

        for _ in 0..24 {
            execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        }

        let retained_heap = cfg.runtime.heap_used_bytes();
        assert_heap_plateaus(baseline_heap, retained_heap);
    }

    #[test]
    fn selector_provider_reclaims_temporary_tables_between_resolves() {
        let source = r#"
local function synthetic_applications()
    local items = {}
    local payload = string.rep("x", 2048)
    for i = 1, 512 do
        items[i] = {
            label = "Application " .. i,
            sublabel = payload .. i,
            data = { path = payload .. i },
        }
    end
    return items
end

return function(menu, ctx)
    menu:bind("a", "Selector", function(actx)
        actx:select({
            title = "Run Application",
            items = function(inner)
                return synthetic_applications()
            end,
            on_select = function(select_ctx, item, query)
            end,
        })
    end)
end
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", false, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Handler(handler) = find_binding(&out.rendered, "a").kind.clone() else {
            panic!("expected handler binding");
        };
        let result = execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        let Effect::Select(selector) = result.effects[0].clone() else {
            panic!("expected selector effect: {:?}", result.effects);
        };

        let mut items = selector
            .resolve_items(&mut cfg, &ctx)
            .expect("resolve items");
        assert_eq!(items.len(), 512);
        let baseline_heap = cfg.runtime.heap_used_bytes();

        for _ in 0..24 {
            items = selector
                .resolve_items(&mut cfg, &ctx)
                .expect("resolve items");
            assert_eq!(items.len(), 512);
        }
        let retained_heap = cfg.runtime.heap_used_bytes();
        assert_heap_plateaus(baseline_heap, retained_heap);
    }

    fn assert_handler_execs(
        cfg: &mut LoadedConfig,
        rendered: &RenderedState,
        ident: &str,
        ctx: &ModeCtx,
        expected: &Action,
    ) {
        let BindingKind::Handler(handler) = &find_binding(rendered, ident).kind else {
            panic!("expected handler binding");
        };
        let result = execute_handler(cfg, handler, ctx).expect("execute handler");
        assert!(
            matches!(&result.effects[0], Effect::Exec(action) if action == expected),
            "unexpected effects: {:?}",
            result.effects
        );
    }

    fn assert_selector_len(effects: &[Effect], expected_len: usize) {
        let Effect::Select(selector) = &effects[0] else {
            panic!("expected selector effect: {effects:?}");
        };
        let SelectorItems::Static(items) = &selector.items else {
            panic!("expected static selector items");
        };
        assert_eq!(items.len(), expected_len);
    }
}
