use std::{
    fs,
    path::{Path, PathBuf},
};

const BANNED_ATTRS: &[&str] = &[
    "#[tokio::test",
    "#[tokio::main",
    "#[async_std::test",
    "#[actix_rt::test",
    "#[futures::test",
];

const BANNED_IMPORTS: &[&str] = &[
    "use tokio::test",
    "use tokio::main",
    "use async_std::test",
    "use actix_rt::test",
    "use futures::test",
];

#[test]
fn forbid_async_test_attributes_in_hotki_world() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    gather_rs_files(&root, &mut files);

    let mut violations = Vec::new();
    for path in files {
        if let Ok(content) = fs::read_to_string(&path) {
            check_patterns(&content, &path, BANNED_ATTRS, &mut violations);
            check_patterns(&content, &path, BANNED_IMPORTS, &mut violations);
        }
    }

    if !violations.is_empty() {
        panic!(
            "async test attributes/imports are banned in hotki-world/tests:\n{}",
            violations.join("\n")
        );
    }
}

fn gather_rs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                gather_rs_files(&path, files);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                if path.file_name().and_then(|name| name.to_str()) == Some("lint_async_attrs.rs") {
                    continue;
                }
                files.push(path);
            }
        }
    }
}

fn check_patterns(content: &str, path: &Path, patterns: &[&str], violations: &mut Vec<String>) {
    for pattern in patterns {
        if content.contains(pattern) {
            violations.push(format!("{}: contains '{}'", path.display(), pattern));
        }
    }
}
