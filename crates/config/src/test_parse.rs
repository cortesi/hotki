#[cfg(test)]
mod tests {
    use crate::{Config, Cursor, Error, rhai};

    fn load(script: &str) -> Result<Config, Error> {
        // Use the internal Rhai loader for config-level coverage.
        Ok(rhai::load_from_str_with_runtime(script, None)?.config)
    }

    #[test]
    fn tag_submenu_plain_string_parses() {
        let cfg = load(r#"style(#{ hud: #{ tag_submenu: ">>" } });"#).unwrap();
        let loc = Cursor::default();
        assert_eq!(cfg.hud(&loc).tag_submenu, ">>");
    }

    #[test]
    fn keys_allow_chord_strings() {
        // A set of chord specs that should be accepted by parsing.
        let chords = [
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

        // Parse each chord spec in isolation so aliases that canonicalize to the same
        // chord (e.g. enter/ret/return) don't trip duplicate validation.
        for (i, chord) in chords.iter().enumerate() {
            let script = format!(r#"global.bind("{}", "Desc{}", exit);"#, chord, i);
            let cfg = load(&script).unwrap();
            assert_eq!(cfg.keys.key_objects().count(), 1);
        }
    }

    #[test]
    fn server_tunables_parse() {
        let cfg = load(r#"server(#{ exit_if_no_clients: true });"#).unwrap();
        assert!(cfg.server().exit_if_no_clients);
    }
}
