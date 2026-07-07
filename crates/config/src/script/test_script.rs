#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use mac_keycode::Chord;

    use crate::{
        Action, Error,
        script::{
            Binding, BindingKind, DynamicConfig, Effect, ModeCtx, ModeFrame, NavRequest,
            RenderedState, SelectorItems, handler::execute_handler,
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

    fn root_frame(cfg: &DynamicConfig) -> ModeFrame {
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
    fn imported_runtime_errors_report_imported_file_excerpt() {
        let root = test_dir("imported-runtime-error");
        let root_path = root.join("hotki.luau");
        let child_path = root.join("child.luau");
        fs::write(
            &child_path,
            r#"
local missing = nil
missing()
return function(menu, ctx)
end
"#,
        )
        .expect("write child config");
        let root_source = r#"
local child = hotki.import_mode("child")

hotki.root(function(menu, ctx)
    menu:submenu("a", "Child", child)
end)
"#;
        fs::write(&root_path, root_source).expect("write root config");

        let err = match load_dynamic_config_from_string(root_source, Some(root_path)) {
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
                let path = path.expect("expected imported path");
                assert_eq!(
                    fs::canonicalize(path).expect("canonicalize error path"),
                    fs::canonicalize(child_path).expect("canonicalize child path")
                );
                assert!(line.is_some(), "expected imported source line");
                let excerpt = excerpt.expect("expected imported source excerpt");
                assert!(
                    excerpt.contains("missing()"),
                    "unexpected excerpt: {excerpt}"
                );
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn chord_parse_errors_include_location() {
        let source = r#"
hotki.root(function(menu, ctx)
    menu:bind("cmd+bogus", "bad", action.shell("true"))
end)
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
hotki.root(function(menu, ctx)
    menu:bind("a", "first", action.shell("true"))
    menu:bind("a", "second", action.shell("true"))
end)
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
    fn auto_pop_truncates_empty_child_modes() {
        let source = r#"
hotki.root(function(menu, ctx)
    menu:submenu("a", "child", function(child, inner)
    end)
end)
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
hotki.root(function(menu, ctx)
    if ctx:app_matches("A") then
        menu:submenu("a", "child-a", function(child, inner)
            child:bind("x", "x", action.shell("true"))
        end)
    else
        menu:submenu("a", "child-b", function(child, inner)
            child:bind("y", "y", action.shell("true"))
        end)
    end
end)
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
hotki.root(function(menu, ctx)
    if ctx:app_matches("A") then
        menu:submenu("a", "child-a", function(child, inner)
            child:bind("x", "x", action.shell("true"))
        end)
    elseif ctx:app_matches("B") then
        menu:submenu("b", "child-b", function(child, inner)
            child:bind("y", "y", action.shell("true"))
        end)
    end
end)
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
hotki.root(function(menu, ctx)
    menu:bind("h", "handler", action.run(function(actx)
        actx:exec(action.shell("echo one"))
        actx:notify("info", "Test", "middle")
        actx:exec(action.shell("echo two"))
        actx:pop()
    end))
end)
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
        assert_eq!(result.effects.len(), 3);
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
        assert!(matches!(result.nav, Some(NavRequest::Pop)));
    }

    #[test]
    fn selector_action_builds_selector_binding() {
        let source = r#"
hotki.root(function(menu, ctx)
    menu:bind("a", "Selector", action.selector({
        title = "Run",
        placeholder = "Search...",
        items = {
            "Safari",
            { label = "Chrome", sublabel = "/Applications/Chrome.app", data = 123 },
        },
        on_select = function(actx, item, query)
        end,
    }))
end)
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
        let BindingKind::Selector(selector) = &binding.kind else {
            panic!("expected selector binding");
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
    fn submenu_flatten_options_accept_integer_number_fields() {
        let source = r#"
hotki.root(function(menu, ctx)
    menu:submenu("a", "child", function(child, inner)
        child:bind("x", "x", action.shell("true"))
    end, {
        global = true,
        capture = true,
        ["repeat"] = {
            delay_ms = 1,
            interval_ms = 2,
        },
    })
end)
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
        assert!(binding.flags.global);
        assert!(binding.mode_capture);
        let repeat = binding.flags.repeat.expect("repeat options");
        assert_eq!(repeat.delay_ms, Some(1));
        assert_eq!(repeat.interval_ms, Some(2));
    }

    #[test]
    fn submenu_volume_actions_accept_integer_number_literals() {
        let source = r#"
hotki.root(function(menu, ctx)
    menu:submenu("m", "music", function(music, inner)
        music:bind("k", "vol up", action.change_volume(5))
        music:bind("j", "vol down", action.change_volume(-5))
        music:bind("1", "set volume", action.set_volume(50))
    end)
end)
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

        let child = render_stack(
            &mut cfg,
            &mut stack,
            &base_ctx("TestApp", true, 1),
            &base_style,
        )
        .expect("render child");
        assert!(matches!(
            find_binding(&child.rendered, "k").kind,
            BindingKind::Action(Action::ChangeVolume(5))
        ));
        assert!(matches!(
            find_binding(&child.rendered, "j").kind,
            BindingKind::Action(Action::ChangeVolume(-5))
        ));
        assert!(matches!(
            find_binding(&child.rendered, "1").kind,
            BindingKind::Action(Action::SetVolume(50))
        ));
    }

    #[test]
    fn render_uses_resolved_base_style_without_row_overrides() {
        let source = r##"
hotki.root(function(menu, ctx)
    menu:submenu("a", "child", function(child, inner)
        child:bind("x", "x", action.shell("true"))
    end)
end)
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
hotki.root(function(menu, ctx)
    local total = 0
    for i = 1, 20000 do
        total = total + i
    end

    if total > 0 then
        menu:bind("a", "loop", action.shell("true"))
    end
end)
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
    fn render_stack_reclaims_temporary_selector_tables_between_renders() {
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

hotki.root(function(menu, ctx)
    menu:bind("a", "Selector", action.selector({
        title = "Run Application",
        items = synthetic_applications(),
        on_select = function(actx, item, query)
        end,
    }))
end)
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", false, 0);
        let mut stack = vec![root_frame(&cfg)];

        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        assert_eq!(find_binding(&out.rendered, "a").desc, "Selector");
        let baseline_heap = cfg.vm.heap_used_bytes();

        for _ in 0..24 {
            let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
            assert_eq!(find_binding(&out.rendered, "a").desc, "Selector");
        }

        let retained_heap = cfg.vm.heap_used_bytes();
        assert_heap_plateaus(baseline_heap, retained_heap);
    }

    #[test]
    fn handler_execution_reclaims_temporary_tables_between_calls() {
        let source = r#"
hotki.root(function(menu, ctx)
    menu:bind("h", "Handler", action.run(function(actx)
        local scratch = {}
        local payload = string.rep("x", 2048)
        for i = 1, 768 do
            scratch[i] = { path = payload .. i }
        end
    end))
end)
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
        let baseline_heap = cfg.vm.heap_used_bytes();

        for _ in 0..24 {
            execute_handler(&mut cfg, &handler, &ctx).expect("execute handler");
        }

        let retained_heap = cfg.vm.heap_used_bytes();
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

hotki.root(function(menu, ctx)
    menu:bind("a", "Selector", action.selector({
        title = "Run Application",
        items = function(inner)
            return synthetic_applications()
        end,
        on_select = function(actx, item, query)
        end,
    }))
end)
"#;
        let mut cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style();
        let ctx = base_ctx("TestApp", false, 0);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(&mut cfg, &mut stack, &ctx, &base_style).expect("render");
        let BindingKind::Selector(selector) = find_binding(&out.rendered, "a").kind.clone() else {
            panic!("expected selector binding");
        };

        let mut items = selector
            .resolve_items(&mut cfg, &ctx)
            .expect("resolve items");
        assert_eq!(items.len(), 512);
        let baseline_heap = cfg.vm.heap_used_bytes();

        for _ in 0..24 {
            items = selector
                .resolve_items(&mut cfg, &ctx)
                .expect("resolve items");
            assert_eq!(items.len(), 512);
        }
        cfg.collect_entrypoint_garbage();

        let retained_heap = cfg.vm.heap_used_bytes();
        assert_heap_plateaus(baseline_heap, retained_heap);
    }
}
