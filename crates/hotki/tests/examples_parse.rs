//! Integration tests for parsing example RON configuration files.

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    #[test]
    fn parse_all_example_rons() {
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
            if path.extension().and_then(|s| s.to_str()) != Some("ron") {
                continue;
            }
            found += 1;
            let content = fs::read_to_string(&path).expect("read ron file");
            let parsed = config::load_from_str(&content, None);
            let fname = path.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                parsed.is_ok(),
                "failed to parse {}: {:?}",
                fname,
                parsed.err()
            );
        }
        assert!(found > 0, "no .ron files found in examples");
    }
}
