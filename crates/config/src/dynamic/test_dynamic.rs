#[cfg(test)]
mod tests {
    use mac_keycode::Chord;
    use rhai::{Array, Dynamic};

    use crate::{
        Action, Error,
        dynamic::{
            Binding, BindingKind, DynamicConfig, Effect, ModeCtx, ModeFrame, NavRequest,
            RenderedState, dsl::ModeBuilder, execute_handler, load_dynamic_config_from_string,
            render_stack,
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
            style: None,
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
        let base_style = cfg.base_style(None);
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
        let base_style = cfg.base_style(None);
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
        let base_style = cfg.base_style(None);
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
        let base_style = cfg.base_style(None);
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
        let source = r##"theme("default");

hotki.mode(|m, ctx| {
  m.style(#{ hud: #{ bg: "#0000ff" } });
  m.style(#{ hud: #{ font_size: 18.0 } });

  m.mode("a", "child", |m, ctx| {
    m.style(#{ hud: #{ bg: "#00ff00", opacity: 0.8 } });
    m.bind("x", "x", action.shell("true"))
      .style(#{ key_bg: "#ff0000" });
  });
});
"##;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let ctx_root = base_ctx("TestApp", true, 0);
        let out_root = render_stack(&cfg, &mut stack, &ctx_root, &base_style).expect("render root");
        let entry = find_binding(&out_root.rendered, "a").clone();
        push_mode(&mut stack, &entry);

        let ctx_child = base_ctx("TestApp", true, 1);
        let out_child =
            render_stack(&cfg, &mut stack, &ctx_child, &base_style).expect("render child");

        assert_eq!(out_child.rendered.style.hud.bg, (0, 255, 0));
        assert_eq!(out_child.rendered.style.hud.font_size, 18.0);
        assert_eq!(out_child.rendered.style.hud.opacity, 0.8);

        let row = out_child
            .rendered
            .hud_rows
            .iter()
            .find(|r| r.chord.to_string() == "x")
            .expect("x row");
        let style = row.style.expect("binding style override should be present");
        assert_eq!(style.key_bg, (255, 0, 0));
    }

    #[test]
    fn multiple_m_style_calls_merge_left_to_right() {
        let source = r#"
theme("default");

hotki.mode(|m, _ctx| {
  m.style(#{ hud: #{ font_size: 18.0, opacity: 0.9 } });
  m.style(#{ hud: #{ font_size: 20.0 } });
  m.bind("a", "a", action.shell("true"));
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", true, 0);
        let out = render_stack(&cfg, &mut stack, &ctx, &base_style).expect("render");

        assert_eq!(out.rendered.style.hud.font_size, 20.0);
        assert_eq!(out.rendered.style.hud.opacity, 0.9);
    }

    #[test]
    fn m_style_accepts_style_objects() {
        let source = r#"
theme("default");

hotki.mode(|m, _ctx| {
  m.style(Style(#{ hud: #{ font_size: 22.0 } }));
  m.bind("a", "a", action.shell("true"));
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        let base_style = cfg.base_style(None);
        let mut stack = vec![root_frame(&cfg)];
        let ctx = base_ctx("TestApp", true, 0);
        let out = render_stack(&cfg, &mut stack, &ctx, &base_style).expect("render");

        assert_eq!(out.rendered.style.hud.font_size, 22.0);
    }

    #[test]
    fn rhai_style_constructor_getters_set_and_merge() {
        let source = r#"
hotki.mode(|_m, _ctx| {
  let s = Style(#{ hud: #{ font_size: 18.0 }, notify: #{ timeout: 5.0 } });
  let s2 = s
    .set("hud", #{ opacity: 0.9 })
    .set("notify.timeout", 2.0);

  let merged = s2.merge(Style(#{ hud: #{ font_size: 20.0 } }));

  [merged.hud.font_size, merged.hud.opacity, merged.notify.timeout]
});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");

        let builder = ModeBuilder::new();
        let ctx = base_ctx("TestApp", false, 0);
        let result = cfg
            .root()
            .func
            .as_ref()
            .expect("expected closure")
            .call::<Dynamic>(&cfg.engine, &cfg.ast, (builder, ctx))
            .expect("call root");

        let arr: Array = result.cast();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].clone_cast::<f64>(), 20.0);
        let opacity = arr[1].clone_cast::<f64>();
        assert!(
            (opacity - 0.9).abs() < 1e-6,
            "expected opacity ~ 0.9, got {opacity}"
        );
        assert_eq!(arr[2].clone_cast::<f64>(), 2.0);
    }

    #[test]
    fn style_constructor_rejects_unknown_fields() {
        let source = r#"
hotki.mode(|_m, _ctx| {
  Style(#{ hud: #{ nope: 1 } });
});
"#;
        let err = match load_dynamic_config_from_string(source.to_string(), None) {
            Ok(_cfg) => panic!("expected error"),
            Err(err) => err,
        };
        match err {
            Error::Validation { message, .. } => assert!(
                message.contains("invalid style map"),
                "unexpected message: {message}"
            ),
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn themes_list_get_register_remove_and_theme_select() {
        let source = r#"
// Builtins are pre-registered.
let names = themes.list();

// Add a custom theme and select it.
themes.register("my-dark", themes.default_.set("hud.font_size", 18.0));
theme("my-dark");

hotki.mode(|_m, _ctx| names);
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        assert_eq!(cfg.active_theme(), "my-dark");

        let style = cfg.base_style(None);
        assert_eq!(style.hud.font_size, 18.0);

        let builder = ModeBuilder::new();
        let ctx = base_ctx("TestApp", false, 0);
        let result = cfg
            .root()
            .func
            .as_ref()
            .expect("expected closure")
            .call::<Dynamic>(&cfg.engine, &cfg.ast, (builder, ctx))
            .expect("call root");

        let names: Array = result.cast();
        assert!(names.len() >= 5, "expected builtin themes in registry");
        assert_eq!(
            names[0].clone_cast::<String>(),
            "charcoal",
            "expected sorted theme names"
        );
    }

    #[test]
    fn theme_errors_on_unknown_name() {
        let source = r#"
theme("does-not-exist");
hotki.mode(|_m, _ctx| {});
"#;
        let err = match load_dynamic_config_from_string(source.to_string(), None) {
            Ok(_cfg) => panic!("expected error"),
            Err(err) => err,
        };
        match err {
            Error::Validation { message, .. } => assert!(
                message.contains("unknown theme"),
                "unexpected message: {message}"
            ),
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn themes_remove_forbids_default_and_falls_back_when_active_removed() {
        let source = r#"
themes.register("tmp", themes.default_);
theme("tmp");
themes.remove("tmp");
hotki.mode(|_m, _ctx| {});
"#;
        let cfg = load_dynamic_config_from_string(source.to_string(), None).expect("load cfg");
        assert_eq!(cfg.active_theme(), "default");

        let source = r#"
themes.remove("default");
hotki.mode(|_m, _ctx| {});
"#;
        let err = match load_dynamic_config_from_string(source.to_string(), None) {
            Ok(_cfg) => panic!("expected error"),
            Err(err) => err,
        };
        match err {
            Error::Validation { message, .. } => assert!(
                message.contains("cannot remove 'default'"),
                "unexpected message: {message}"
            ),
            other => panic!("expected validation error, got {other:?}"),
        }
    }
}
