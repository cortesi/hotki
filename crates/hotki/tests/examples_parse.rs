//! Integration tests for parsing example configuration files.

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    #[test]
    fn parse_all_example_configs() {
        // Locate the examples directory from the workspace root
        let examples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent() // crates
            .and_then(|p| p.parent()) // workspace root
            .expect("workspace root")
            .join("examples");

        let mut found = 0usize;
        for entry in fs::read_dir(&examples_dir).expect("read examples dir") {
            let entry = entry.unwrap();
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());
            if ext != Some("rhai") {
                continue;
            }
            found += 1;
            let parsed = config::load_from_path(&path);
            let fname = path.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                parsed.is_ok(),
                "failed to parse {}: {:?}",
                fname,
                parsed.err()
            );
        }
        assert!(found > 0, "no config files found in examples");
    }
}
