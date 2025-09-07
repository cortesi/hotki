use std::sync::{Mutex, OnceLock};

use crate::{config, runtime};

static CONN: OnceLock<Mutex<Option<hotki_server::Connection>>> = OnceLock::new();

fn conn_slot() -> &'static Mutex<Option<hotki_server::Connection>> {
    CONN.get_or_init(|| Mutex::new(None))
}

/// Initialize a shared MRPC connection to the hotki-server at `socket_path`.
pub fn init(socket_path: &str) -> bool {
    let res =
        runtime::block_on(async { hotki_server::Connection::connect_unix(socket_path).await });
    match res {
        Ok(Ok(c)) => {
            if let Ok(mut g) = conn_slot().lock() {
                *g = Some(c);
                return true;
            }
            false
        }
        _ => false,
    }
}

/// Returns true if a connection is available for RPC driving.
pub fn is_ready() -> bool {
    conn_slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

/// Inject a single key press (down + small delay + up) via MRPC.
pub fn inject_key(seq: &str) -> bool {
    let mut guard = match conn_slot().lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let conn = match guard.as_mut() {
        Some(c) => c,
        None => return false,
    };
    // Canonicalize ident to engine format (e.g., cmd+shift+0)
    let ident = mac_keycode::Chord::parse(seq)
        .map(|c| c.to_string())
        .unwrap_or_else(|| seq.to_string());
    // Drive down -> delay -> up
    let ok = runtime::block_on(async { conn.inject_key_down(&ident).await }).is_ok();
    std::thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
    let ok2 = runtime::block_on(async { conn.inject_key_up(&ident).await }).is_ok();
    ok && ok2
}

/// Inject a sequence of key presses with UI delays.
pub fn inject_sequence(sequences: &[&str]) -> bool {
    for s in sequences {
        if !inject_key(s) {
            return false;
        }
        std::thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
    }
    true
}

/// Return current bindings if connected.
pub fn get_bindings() -> Option<Vec<String>> {
    let mut guard = match conn_slot().lock() {
        Ok(g) => g,
        Err(_) => return None,
    };
    let conn = guard.as_mut()?;
    match runtime::block_on(async { conn.get_bindings().await }) {
        Ok(Ok(v)) => Some(v),
        _ => None,
    }
}

/// Wait until a specific identifier is present in the current bindings.
pub fn wait_for_ident(ident: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if let Some(binds) = get_bindings()
            && binds.iter().any(|b| b == ident)
        {
            return true;
        }
        std::thread::sleep(config::ms(config::RETRY_DELAY_MS));
    }
    false
}

/// Quick liveness probe against the backend via a lightweight RPC.
/// Returns false if the connection is not ready or the RPC fails.
pub fn check_alive() -> bool {
    let mut guard = match conn_slot().lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let conn = match guard.as_mut() {
        Some(c) => c,
        None => return false,
    };
    matches!(
        runtime::block_on(async { conn.get_depth().await }),
        Ok(Ok(_))
    )
}
