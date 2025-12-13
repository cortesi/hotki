#[cfg(test)]
mod tests {
    use crate::*; // bring Config and helpers into scope

    #[test]
    fn tag_submenu_plain_string_parses() {
        let ron = r#"(
            keys: [],
            style: (hud: (tag_submenu: ">>")),
        )"#;
        let cfg: Config = loader::load_from_str(ron, None).unwrap();
        let loc = Cursor::default();
        assert_eq!(cfg.hud(&loc).tag_submenu, ">>");
    }

    #[test]
    fn keys_allow_chord_strings() {
        // A set of chord specs that should be accepted by parsing
        let chords = vec![
            "shift+cmd+0",
            "cmd+space",
            "cmd+tab",
            "opt+/",
            "ctrl+-",
            "cmd+=",
            "cmd+[",
            "cmd+]",
            "cmd+;",
            "cmd+,",
            "cmd+.",
            "cmd+`",
            "esc",
            "enter",
            "ret",
            "left",
            "right",
            "up",
            "down",
            "pgup",
            "pgdn",
        ];

        // Build a RON config with one entry per chord
        let mut items = String::new();
        for (i, c) in chords.iter().enumerate() {
            if i > 0 {
                items.push_str(",\n");
            }
            items.push_str(&format!("(\"{}\", \"Desc{}\", exit)", c, i));
        }
        let ron = format!("(keys: [\n{}\n])", items);

        let cfg: Config = loader::load_from_str(&ron, None).unwrap();
        // Ensure we got the same number of key entries back
        assert_eq!(cfg.keys.key_objects().count(), chords.len());
    }

    #[test]
    fn unknown_mode_attr_key_fails() {
        // misspelled match_app => match_ap should error
        let ron = r#"(
            keys: [
                ("a", "Test", exit, (match_ap: "Safari")),
            ],
        )"#;
        let res = loader::load_from_str(ron, None);
        assert!(res.is_err());
    }

    #[test]
    fn unknown_shell_modifier_key_fails() {
        // misspelled ok_notify => ok_nofity should error
        let ron = r#"(
            keys: [
                ("s", "Shell", shell("echo hi", (ok_nofity: info))),
            ],
        )"#;
        let res = loader::load_from_str(ron, None);
        assert!(res.is_err());
    }

    #[test]
    fn server_tunables_parse() {
        let ron = r#"(
            keys: [],
            server: (exit_if_no_clients: true),
        )"#;
        let cfg: Config = loader::load_from_str(ron, None).unwrap();
        assert!(cfg.server().exit_if_no_clients);
    }
}
