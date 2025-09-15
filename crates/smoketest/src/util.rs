use std::{env, path::PathBuf};

pub fn resolve_hotki_bin() -> Option<PathBuf> {
    if let Ok(p) = env::var("HOTKI_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("hotki")))
        .filter(|p| p.exists())
}
