#[cfg(test)]
mod tests {
    use mac_keycode::Chord;

    use crate::{
        Action, Error,
        dynamic::{
            Binding, BindingKind, DynamicConfig, Effect, ModeCtx, ModeFrame, NavRequest,
            RenderedState, execute_handler, load_dynamic_config_from_string, render_stack,
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
            .find_map(|(ch, b)| if *ch == chord { Some(b) } else { None })
            .unwrap_or_else(|| panic!("missing binding ident '{}'", ident))
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
            style: binding.mode_style.clone(),
            capture: binding.mode_capture,
        });
    }

    #[test]
    fn chord_parse_errors_include_location() {
        let source = r#"hotki.mode(|m, ctx| {
  m.bind("cmd+bogus", "bad", action.shell("true"));
});
"#;
        let err = match load_dynamic_config_from_string(source.to_string(), None) {
            Ok(_cfg) => panic!("expected chord parse error"),
            Err(err) => err,
        };
        match err {
            Error::Validation {
                line: Some(_),
                col: Some(_),
                excerpt: Some(excerpt),
                message,
                ..
            } => {
                assert!(
                    message.contains("invalid chord string"),
                    "unexpected message: {message}"
                );
                assert!(
                    excerpt.contains("cmd+bogus"),
                    "excerpt should mention the chord:\n{excerpt}"
                );
                assert!(
                    excerpt.contains('^'),
                    "excerpt should include a caret:\n{excerpt}"
                );
            }
            other => panic!("expected validation error with location, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_chord_warns_and_first_wins() {
        let source = r#"hotki.mode(|m, ctx| {
  m.bind("a", "first", action.shell("true"));
  m.bind("a", "second", action.shell("true"));
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None, false);
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", false, 0);
        let out = render_stack(&cfg, &mut stack, &ctx, &base_style).expect("render");

        assert_eq!(
            out.warnings.len(),
            1,
            "expected one duplicate-chord warning"
        );
        assert!(matches!(
            out.warnings[0],
            Effect::Notify {
                kind: crate::NotifyKind::Warn,
                ..
            }
        ));

        let binding = find_binding(&out.rendered, "a");
        assert_eq!(binding.desc, "first");
        assert_eq!(stack[0].rendered.len(), 1);
    }

    #[test]
    fn auto_pop_truncates_empty_child_modes() {
        let source = r#"hotki.mode(|m, ctx| {
  m.mode("a", "child", |m, ctx| { });
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None, false);
        let mut stack = vec![root_frame(&cfg)];
        let ctx_root = base_ctx("TestApp", false, 0);
        let out = render_stack(&cfg, &mut stack, &ctx_root, &base_style).expect("render root");

        let mode_entry = find_binding(&out.rendered, "a").clone();
        push_mode(&mut stack, &mode_entry);
        assert_eq!(stack.len(), 2);

        let ctx_child = base_ctx("TestApp", true, 1);
        let _out2 = render_stack(&cfg, &mut stack, &ctx_child, &base_style).expect("render stack");
        assert_eq!(stack.len(), 1, "empty child mode should auto-pop");
    }

    #[test]
    fn orphan_detection_pops_when_mode_identity_changes() {
        let source = r#"hotki.mode(|m, ctx| {
  if ctx.app.matches("A") {
    m.mode("a", "child-a", |m, ctx| { m.bind("x", "x", action.shell("true")); });
  } else {
    m.mode("a", "child-b", |m, ctx| { m.bind("y", "y", action.shell("true")); });
  }
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None, false);
        let mut stack = vec![root_frame(&cfg)];

        let ctx_a = base_ctx("A", true, 0);
        let out_a = render_stack(&cfg, &mut stack, &ctx_a, &base_style).expect("render A");
        let mode_entry = find_binding(&out_a.rendered, "a").clone();
        push_mode(&mut stack, &mode_entry);
        assert_eq!(stack.len(), 2);

        let ctx_b = base_ctx("B", true, 1);
        let _out_b = render_stack(&cfg, &mut stack, &ctx_b, &base_style).expect("render B");
        assert_eq!(
            stack.len(),
            1,
            "child should be popped when parent 'a' points to a different mode"
        );
    }

    #[test]
    fn handler_effects_preserve_enqueue_order() {
        let source = r#"hotki.mode(|m, ctx| {
  m.bind("h", "handler", handler(|ctx| {
    ctx.exec(action.shell("echo one"));
    ctx.notify(info, "Test", "middle");
    ctx.exec(action.shell("echo two"));
    ctx.pop();
  }));
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None, false);
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", true, 0);
        let out = render_stack(&cfg, &mut stack, &ctx, &base_style).expect("render");
        let binding = find_binding(&out.rendered, "h");
        let BindingKind::Handler(handler) = &binding.kind else {
            panic!("expected handler binding, got {:?}", binding.kind);
        };

        let result = execute_handler(&cfg, handler, &ctx).expect("execute handler");

        assert_eq!(result.effects.len(), 3);
        match &result.effects[0] {
            Effect::Exec(Action::Shell(spec)) => assert_eq!(spec.command(), "echo one"),
            other => panic!("expected shell exec first, got {other:?}"),
        }
        match &result.effects[1] {
            Effect::Notify { kind, title, body } => {
                assert_eq!(*kind, crate::NotifyKind::Info);
                assert_eq!(title, "Test");
                assert_eq!(body, "middle");
            }
            other => panic!("expected notify second, got {other:?}"),
        }
        match &result.effects[2] {
            Effect::Exec(Action::Shell(spec)) => assert_eq!(spec.command(), "echo two"),
            other => panic!("expected shell exec third, got {other:?}"),
        }

        assert!(matches!(result.nav, Some(NavRequest::Pop)));
    }

    #[test]
    fn style_inheritance_layers_mode_overlays_and_binding_overrides() {
        let source = r##"base_theme("default");

hotki.mode(|m, ctx| {
  m.style(#{ hud: #{ bg: "#0000ff" } });

  m.mode("a", "child", |m, ctx| {
    m.bind("x", "x", action.shell("true"))
      .style(#{ key_bg: "#ff0000" });
  }).style(#{ hud: #{ bg: "#00ff00" } });
});
"##;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None, true);
        let mut stack = vec![root_frame(&cfg)];
        let ctx_root = base_ctx("TestApp", true, 0);
        let out_root = render_stack(&cfg, &mut stack, &ctx_root, &base_style).expect("render root");
        let entry = find_binding(&out_root.rendered, "a").clone();
        push_mode(&mut stack, &entry);

        let ctx_child = base_ctx("TestApp", true, 1);
        let out_child =
            render_stack(&cfg, &mut stack, &ctx_child, &base_style).expect("render child");

        assert_eq!(out_child.rendered.style.hud.bg, (0, 255, 0));

        let row = out_child
            .rendered
            .hud_rows
            .iter()
            .find(|r| r.chord.to_string() == "x")
            .expect("x row");
        let style = row.style.expect("binding style override should be present");
        assert_eq!(style.key_bg, (255, 0, 0));
    }
}
