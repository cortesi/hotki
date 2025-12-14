#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        os::unix::fs::symlink,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use mac_keycode::Chord;

    use crate::{
        Action, Cursor, Error, FontWeight, NotifyKind, NotifyPos, Pos, RhaiRuntime,
        rhai::load_from_str_with_runtime,
    };

    fn load(script: &str) -> Result<crate::Config, Error> {
        // Use the private Rhai loader directly for unit-level coverage.
        load_from_str_with_runtime(script, None).map(|loaded| loaded.config)
    }

    fn load_with_runtime(script: &str) -> (crate::Config, RhaiRuntime) {
        let loaded = load_from_str_with_runtime(script, None).expect("loads");
        let runtime = loaded.runtime.expect("runtime present");
        (loaded.config, runtime)
    }

    fn unique_tmp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let mut dir = env::temp_dir();
        dir.push(format!("hotki-{name}-{}-{nanos}", process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn chord(spec: &str) -> Chord {
        Chord::parse(spec).expect("valid chord")
    }

    fn find_action(cfg: &crate::Config, spec: &str) -> Action {
        let ch = chord(spec);
        cfg.keys
            .keys
            .iter()
            .find(|(c, _, _, _)| c == &ch)
            .map(|(_, _, a, _)| a.clone())
            .expect("binding exists")
    }

    #[test]
    fn minimal_script_loads() {
        let cfg = load(
            r#"
            base_theme("default");
            global.bind("a", "Exit", action.exit);
            "#,
        )
        .unwrap();
        assert_eq!(cfg.keys.key_objects().count(), 1);
    }

    #[test]
    fn mode_api_builds_nested_keys() {
        let cfg = load(
            r#"
            global.mode("shift+cmd+0", "Main", |m| {
              m.bind("a", "Exit", action.exit);
            });
            "#,
        )
        .unwrap();

        let Action::Keys(inner) = find_action(&cfg, "shift+cmd+0") else {
            panic!("expected mode action");
        };
        assert_eq!(inner.key_objects().count(), 1);
    }

    #[test]
    fn fluent_attrs_apply_to_bindings() {
        let cfg =
            load(r#"global.bind("a", "Exit", action.exit).global().hidden().hud_only();"#).unwrap();
        let attrs = cfg
            .attrs_for_key(&Cursor::default(), &chord("a"))
            .expect("attrs");
        assert!(attrs.global());
        assert!(attrs.hide());
        assert!(attrs.hud_only());
    }

    #[test]
    fn actions_are_immutable_in_fluent_calls() {
        let cfg = load(
            r#"
            let base = action.shell("echo hi");
            let silent = base.silent();
            let loud = base.notify(success, error);
            global.bind("a", "Base", base);
            global.bind("b", "Silent", silent);
            global.bind("c", "Loud", loud);
            "#,
        )
        .unwrap();

        let Action::Shell(base_spec) = find_action(&cfg, "a") else {
            panic!("expected shell");
        };
        assert_eq!(base_spec.ok_notify(), NotifyKind::Ignore);
        assert_eq!(base_spec.err_notify(), NotifyKind::Warn);

        let Action::Shell(silent_spec) = find_action(&cfg, "b") else {
            panic!("expected shell");
        };
        assert_eq!(silent_spec.ok_notify(), NotifyKind::Ignore);
        assert_eq!(silent_spec.err_notify(), NotifyKind::Ignore);

        let Action::Shell(loud_spec) = find_action(&cfg, "c") else {
            panic!("expected shell");
        };
        assert_eq!(loud_spec.ok_notify(), NotifyKind::Success);
        assert_eq!(loud_spec.err_notify(), NotifyKind::Error);
    }

    #[test]
    fn action_methods_are_type_checked() {
        let err =
            load(r#"global.bind("a", "Bad", action.exit.notify(success, error));"#).unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));
        assert!(err.pretty().contains("notify is only valid"));
    }

    #[test]
    fn binding_attribute_applicability_is_enforced() {
        let err = load(r#"global.bind("a", "Exit", action.exit).capture();"#).unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));

        let err = load(r#"global.mode("m", "Main", |m| {}).no_exit();"#).unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));

        let err = load(r#"global.bind("a", "Exit", action.exit).style(#{});"#).unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));
    }

    #[test]
    fn duplicate_chords_error() {
        let err = load(
            r#"
            global.bind("a", "One", action.exit);
            global.bind("a", "Two", action.exit);
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));
        assert!(err.pretty().contains("duplicate chord"));
    }

    #[test]
    fn duplicate_chords_are_allowed_when_guarded() {
        let cfg = load(
            r#"
            global.bind("a", "One", action.shell("echo one")).match_app("Foo");
            global.bind("a", "Two", action.shell("echo two")).match_app("Bar");
            "#,
        )
        .unwrap();

        let key = chord("a");
        let (one, _, _) = cfg
            .action(&Cursor::default(), &key, "Foo", "")
            .expect("match Foo");
        let Action::Shell(one) = one else {
            panic!("expected shell");
        };
        assert_eq!(one.command(), "echo one");

        let (two, _, _) = cfg
            .action(&Cursor::default(), &key, "Bar", "")
            .expect("match Bar");
        let Action::Shell(two) = two else {
            panic!("expected shell");
        };
        assert_eq!(two.command(), "echo two");

        assert!(cfg.action(&Cursor::default(), &key, "Baz", "").is_none());
    }

    #[test]
    fn invalid_chords_error() {
        let err = load(r#"global.bind("BAD_KEY", "Bad", action.exit);"#).unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));
        assert!(err.pretty().contains("invalid chord"));
    }

    #[test]
    fn style_and_server_maps_reject_unknown_keys() {
        let err = load(
            r#"
            style(#{
              hud: #{ pos: ne, definitely_not_real: 123 },
            });
            global.bind("a", "Exit", action.exit);
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));

        let err = load(
            r#"
            server(#{ wat: true });
            global.bind("a", "Exit", action.exit);
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Validation { .. }));
    }

    #[test]
    fn server_tunables_roundtrip() {
        let cfg = load(
            r#"
            server(#{ exit_if_no_clients: true });
            global.bind("a", "Exit", action.exit);
            "#,
        )
        .unwrap();
        assert!(cfg.server().exit_if_no_clients);
    }

    #[test]
    fn env_returns_empty_string_when_missing() {
        let cfg = load(
            r#"
            let v = env("__HOTKI_ENV_SHOULD_NOT_EXIST__");
            global.bind("a", "Cmd", action.shell("x" + v));
            "#,
        )
        .unwrap();
        let Action::Shell(spec) = find_action(&cfg, "a") else {
            panic!("expected shell");
        };
        assert_eq!(spec.command(), "x");
    }

    #[test]
    fn style_maps_accept_string_constants_for_enums() {
        let cfg = load(
            r#"
            style(#{
              hud: #{ pos: ne, title_font_weight: medium },
              notify: #{ pos: right },
            });
            global.bind("a", "Exit", action.exit);
            "#,
        )
        .unwrap();

        let loc = Cursor::default();
        assert_eq!(cfg.hud(&loc).pos, Pos::NE);
        assert_eq!(cfg.hud(&loc).title_font_weight, FontWeight::Medium);
        assert_eq!(cfg.notify_config(&loc).pos, NotifyPos::Right);
    }

    #[test]
    fn imports_reject_parent_dir_segments() {
        let dir = unique_tmp_dir("rhai-import-parent");
        let cfg_path = dir.join("config.rhai");
        let err = load_from_str_with_runtime(
            r#"
            import "../evil";
            global.bind("a", "Exit", action.exit);
            "#,
            Some(&cfg_path),
        )
        .err()
        .expect("import should fail");
        assert!(matches!(err, Error::Parse { .. }));
        assert!(err.pretty().contains("parent directory segments"));
    }

    #[test]
    fn imports_cannot_escape_via_symlink() {
        let root = unique_tmp_dir("rhai-import-root");
        let external = unique_tmp_dir("rhai-import-external");

        fs::write(
            external.join("evil.rhai"),
            r#"
            export fn answer() { 42 }
            "#,
        )
        .expect("write module");

        symlink(external.join("evil.rhai"), root.join("evil_link.rhai")).expect("symlink");

        let cfg_path = root.join("config.rhai");
        let err = load_from_str_with_runtime(
            r#"
            import "evil_link";
            global.bind("a", "Exit", action.exit);
            "#,
            Some(&cfg_path),
        )
        .err()
        .expect("import should fail");
        assert!(matches!(err, Error::Parse { .. }));
        assert!(err.pretty().contains("escapes config directory"));
    }

    #[test]
    fn script_actions_can_return_single_action() {
        let (cfg, rt) = load_with_runtime(r#"global.bind("a", "Next", || action.theme_next);"#);
        let Action::Rhai { id } = find_action(&cfg, "a") else {
            panic!("expected rhai action");
        };
        let actions = rt
            .eval_action(id, "Safari", "Window", 123, &Cursor::default())
            .expect("eval");
        assert_eq!(actions, vec![Action::ThemeNext]);
    }

    #[test]
    fn script_actions_can_return_macro_array() {
        let (cfg, rt) = load_with_runtime(
            r#"global.bind("a", "Macro", || [action.theme_next, action.theme_prev]);"#,
        );
        let Action::Rhai { id } = find_action(&cfg, "a") else {
            panic!("expected rhai action");
        };
        let actions = rt
            .eval_action(id, "Safari", "Window", 123, &Cursor::default())
            .expect("eval");
        assert_eq!(actions, vec![Action::ThemeNext, Action::ThemePrev]);
    }

    #[test]
    fn script_actions_can_take_ctx() {
        let (cfg, rt) = load_with_runtime(
            r#"
            global.bind("a", "Ctx", |ctx| {
              if ctx.app.contains("Safari") { action.theme_next } else { action.theme_prev }
            });
            "#,
        );
        let Action::Rhai { id } = find_action(&cfg, "a") else {
            panic!("expected rhai action");
        };
        let actions = rt
            .eval_action(id, "Safari", "Window", 123, &Cursor::default())
            .expect("eval");
        assert_eq!(actions, vec![Action::ThemeNext]);
    }

    #[test]
    fn script_action_return_type_is_enforced() {
        let (cfg, rt) = load_with_runtime(r#"global.bind("a", "Bad", || 42);"#);
        let Action::Rhai { id } = find_action(&cfg, "a") else {
            panic!("expected rhai action");
        };
        let msg = rt
            .eval_action(id, "Safari", "Window", 123, &Cursor::default())
            .expect_err("eval should fail");
        assert!(msg.contains("script action must return Action or [Action]"));
    }

    #[test]
    fn script_action_array_elements_are_type_checked() {
        let (cfg, rt) =
            load_with_runtime(r#"global.bind("a", "Bad", || [action.theme_next, 42]);"#);
        let Action::Rhai { id } = find_action(&cfg, "a") else {
            panic!("expected rhai action");
        };
        let msg = rt
            .eval_action(id, "Safari", "Window", 123, &Cursor::default())
            .expect_err("eval should fail");
        assert!(msg.contains("script action array element must be Action"));
    }

    #[test]
    fn script_actions_can_call_helpers_that_use_constants() {
        let (cfg, rt) = load_with_runtime(
            r#"
            fn spotify(cmd) {
              action.shell("spotify " + cmd).notify(ignore, warn)
            }

            global.bind("a", "Pause", || spotify("pause"));
            "#,
        );
        let Action::Rhai { id } = find_action(&cfg, "a") else {
            panic!("expected rhai action");
        };
        let actions = rt
            .eval_action(id, "Safari", "Window", 123, &Cursor::default())
            .expect("eval");
        let [Action::Shell(crate::ShellSpec::WithMods(cmd, mods))] = actions.as_slice() else {
            panic!("expected single shell action");
        };
        assert_eq!(cmd, "spotify pause");
        assert_eq!(mods.ok_notify, NotifyKind::Ignore);
        assert_eq!(mods.err_notify, NotifyKind::Warn);
    }
}
