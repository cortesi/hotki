#[cfg(test)]
mod tests {
    use mac_keycode::Chord;

    use crate::{
        Action, Error,
        script::{
            Binding, BindingKind, DynamicConfig, Effect, ModeCtx, ModeFrame, NavRequest,
            RenderedState, SelectorItems, handler::execute_handler,
            load_dynamic_config_from_string, render_stack,
        },
    };

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
            style: None,
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
            style: None,
            capture: binding.mode_capture,
        });
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
        let cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", false, 0);
        let out = render_stack(&cfg, &mut stack, &ctx, &base_style).expect("render");

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
        let cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(
            &cfg,
            &mut stack,
            &base_ctx("TestApp", false, 0),
            &base_style,
        )
        .expect("render root");
        let mode_entry = find_binding(&out.rendered, "a").clone();
        push_mode(&mut stack, &mode_entry);
        let _ = render_stack(&cfg, &mut stack, &base_ctx("TestApp", true, 1), &base_style)
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
        let cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];

        let out_a =
            render_stack(&cfg, &mut stack, &base_ctx("A", true, 0), &base_style).expect("render A");
        let entry = find_binding(&out_a.rendered, "a").clone();
        push_mode(&mut stack, &entry);

        let _ =
            render_stack(&cfg, &mut stack, &base_ctx("B", true, 1), &base_style).expect("render B");
        assert_eq!(stack.len(), 1);
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
        let cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", true, 0);
        let out = render_stack(&cfg, &mut stack, &ctx, &base_style).expect("render");
        let binding = find_binding(&out.rendered, "h");
        let BindingKind::Handler(handler) = &binding.kind else {
            panic!("expected handler binding");
        };

        let result = execute_handler(&cfg, handler, &ctx).expect("execute handler");
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
        let cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let out = render_stack(
            &cfg,
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
    fn style_inheritance_layers_mode_overlays_and_binding_overrides() {
        let source = r##"
themes:use("default")

hotki.root(function(menu, ctx)
    menu:style({ hud = { bg = "#0000ff" } })
    menu:style({ hud = { font_size = 18 } })

    menu:submenu("a", "child", function(child, inner)
        child:style({ hud = { bg = "#00ff00", opacity = 0.8 } })
        child:bind("x", "x", action.shell("true"), {
            style = { key_bg = "#ff0000" },
        })
    end)
end)
"##;
        let cfg = load_dynamic_config_from_string(source, None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let out_root = render_stack(&cfg, &mut stack, &base_ctx("TestApp", true, 0), &base_style)
            .expect("render root");
        let entry = find_binding(&out_root.rendered, "a").clone();
        push_mode(&mut stack, &entry);
        let out_child = render_stack(&cfg, &mut stack, &base_ctx("TestApp", true, 1), &base_style)
            .expect("render child");

        assert_eq!(out_child.rendered.style.hud.bg, (0, 255, 0));
        assert_eq!(out_child.rendered.style.hud.font_size, 18.0);
        let row = out_child
            .rendered
            .hud_rows
            .iter()
            .find(|row| row.chord.to_string() == "x")
            .expect("x row");
        let style = row.style.expect("binding style");
        assert_eq!(style.key_bg, (255, 0, 0));
    }
}
