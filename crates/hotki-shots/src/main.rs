use std::{
    cmp, env,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::{self, Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionOnScreenOnly, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
};

#[derive(Parser, Debug)]
#[command(
    name = "hotki-shots",
    about = "Capture Hotki HUD and notifications as PNGs",
    version
)]
struct Cli {
    /// Theme name to apply before capturing (optional)
    #[arg(long)]
    theme: Option<String>,
    /// Output directory for PNG files
    #[arg(long)]
    dir: PathBuf,
    /// Timeout in milliseconds for HUD readiness and waits
    #[arg(long, default_value_t = 10_000)]
    timeout: u64,
    /// Enable logging for the spawned hotki process
    #[arg(long, default_value_t = false)]
    logs: bool,
}

fn resolve_hotki_bin() -> Option<PathBuf> {
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

fn used_config_path(theme: &Option<String>) -> io::Result<PathBuf> {
    let cwd = env::current_dir()?;
    let cfg_path = cwd.join("examples/test.rhai");
    if theme.is_none() {
        return Ok(cfg_path);
    }
    let name = theme.as_ref().unwrap();
    match fs::read_to_string(&cfg_path) {
        Ok(s) => {
            let re = regex::Regex::new("theme\\(\\s*\"[^\"]*\"\\s*\\)\\s*;").unwrap();
            let out = if re.is_match(&s) {
                re.replace(&s, format!("theme(\"{}\");", name)).to_string()
            } else {
                format!("theme(\"{}\");\n{}", name, s)
            };
            let tmp = env::temp_dir().join(format!(
                "hotki-shots-{}-{}.rhai",
                process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::write(&tmp, out)?;
            Ok(tmp)
        }
        Err(_) => Ok(cfg_path),
    }
}

fn spawn_hotki(bin: &Path, cfg: &Path, logs: bool) -> io::Result<Child> {
    let mut cmd = Command::new(bin);
    if logs {
        // Respect parent's RUST_LOG if set; otherwise default to info for server logs
        if env::var_os("RUST_LOG").is_none() {
            cmd.env("RUST_LOG", "info");
        }
    }
    cmd.arg("--config").arg(cfg);
    cmd.spawn()
}

fn socket_path_for_pid(pid: u32) -> String {
    hotki_server::socket_path_for_pid(pid)
}

fn wait_for_hud(rt: &tokio::runtime::Runtime, sock: &str, hotki_pid: u32, timeout_ms: u64) -> bool {
    // Connect client with retry
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut client = loop {
        let res = rt.block_on(async {
            hotki_server::Client::new_with_socket(sock)
                .with_connect_only()
                .connect()
                .await
        });
        match res {
            Ok(c) => break c,
            Err(_) => {
                if Instant::now() >= deadline {
                    return false;
                }
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
    };

    if let Ok(conn) = client.connection() {
        // Send activation chord a few times while we wait
        inject_key(rt, conn, "shift+cmd+0");
        let poll = Duration::from_millis(200);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = cmp::min(left, poll);
            match rt.block_on(async { tokio::time::timeout(chunk, conn.recv_event()).await }) {
                Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { hud, .. })) => {
                    if hud.visible {
                        return true;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => {}
            }
            if find_window_by_pid_title(hotki_pid, "Hotki HUD").is_some() {
                return true;
            }
            inject_key(rt, conn, "shift+cmd+0");
        }
    }
    false
}

fn inject_key(rt: &tokio::runtime::Runtime, conn: &mut hotki_server::Connection, ident: &str) {
    let ident = mac_keycode::Chord::parse(ident)
        .map(|c| c.to_string())
        .unwrap_or_else(|| ident.to_string());
    let _ = rt.block_on(async { conn.inject_key_down(&ident).await });
    thread::sleep(Duration::from_millis(60));
    let _ = rt.block_on(async { conn.inject_key_up(&ident).await });
}

fn capture_window_by_id(pid: u32, title: &str, dir: &Path, name: &str) -> bool {
    let Some(win_id) = find_window_by_pid_title(pid, title).or_else(|| find_window_by_title(title))
    else {
        eprintln!(
            "WARN: window not found for capture (pid={}, title={})",
            pid, title
        );
        return false;
    };

    let sanitized = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = dir.join(format!("{}.png", sanitized));

    // Capture by window id
    let status = Command::new("screencapture")
        .args([
            OsStr::new("-x"),
            OsStr::new("-o"),
            OsStr::new("-l"),
            OsString::from(win_id.to_string()).as_os_str(),
            path.as_os_str(),
        ])
        .status();
    if matches!(status, Ok(s) if s.success()) {
        return true;
    }

    eprintln!(
        "WARN: screencapture failed (pid={}, title={}, window_id={})",
        pid, title, win_id
    );
    false
}

fn wait_for_window_by_pid_title(pid: u32, title: &str, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if find_window_by_pid_title(pid, title).is_some() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn inject_until_window(
    rt: &tokio::runtime::Runtime,
    conn: &mut hotki_server::Connection,
    pid: u32,
    window_title: &str,
    ident: &str,
    timeout_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        inject_key(rt, conn, ident);
        if wait_for_window_by_pid_title(pid, window_title, 600) {
            return true;
        }
    }
    false
}

fn main() {
    let cli = Cli::parse();

    let hotki_bin = match resolve_hotki_bin() {
        Some(p) => p,
        None => {
            eprintln!(
                "ERROR: could not locate 'hotki' binary (set HOTKI_BIN or cargo build --bin hotki)"
            );
            process::exit(2);
        }
    };

    let cfg_path = match used_config_path(&cli.theme) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: unable to read or write config: {}", e);
            process::exit(2);
        }
    };

    let mut child = match spawn_hotki(&hotki_bin, &cfg_path, cli.logs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: failed to spawn hotki: {}", e);
            process::exit(2);
        }
    };
    let hotki_pid = child.id();
    let sock = socket_path_for_pid(hotki_pid);

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ERROR: failed to create Tokio runtime: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            process::exit(2);
        }
    };

    if !wait_for_hud(&rt, &sock, hotki_pid, cli.timeout) {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("ERROR: HUD did not appear within {} ms", cli.timeout);
        process::exit(2);
    }

    let _ = fs::create_dir_all(&cli.dir);
    let mut client = match rt.block_on(async {
        hotki_server::Client::new_with_socket(&sock)
            .with_connect_only()
            .connect()
            .await
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: failed to connect to server: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            process::exit(2);
        }
    };
    let conn = match client.connection() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: failed to get connection: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            process::exit(2);
        }
    };

    // Capture HUD first
    let mut failed = Vec::new();
    let hud_ok = capture_window_by_id(hotki_pid, "Hotki HUD", &cli.dir, "hud");
    if !hud_ok {
        failed.push("hud");
    }

    // Trigger notifications via chords and capture
    let gap = Duration::from_millis(160);
    for (k, name) in [
        ("t", None::<&str>),
        ("s", Some("notify_success")),
        ("i", Some("notify_info")),
        ("w", Some("notify_warning")),
        ("e", Some("notify_error")),
    ] {
        inject_key(&rt, conn, k);
        thread::sleep(gap);
        if let Some(n) = name {
            thread::sleep(Duration::from_millis(120));
            let ok = capture_window_by_id(hotki_pid, "Hotki Notification", &cli.dir, n);
            if !ok {
                failed.push(n);
            }
        }
    }

    // Selector capture: open selector demo, type a query, and screenshot.
    if inject_until_window(&rt, conn, hotki_pid, "Hotki Selector", "p", cli.timeout) {
        for k in ["c", "a", "l"] {
            inject_key(&rt, conn, k);
        }
        thread::sleep(Duration::from_millis(250));
        let ok = capture_window_by_id(hotki_pid, "Hotki Selector", &cli.dir, "selector");
        if !ok {
            failed.push("selector");
        }
        inject_key(&rt, conn, "esc");
    } else {
        failed.push("selector");
    }

    // Exit HUD and shutdown server
    inject_key(&rt, conn, "shift+cmd+0");
    let _ = rt.block_on(async move { client.shutdown_server().await });
    let _ = child.kill();
    let _ = child.wait();

    if !failed.is_empty() {
        eprintln!(
            "ERROR: failed to capture screenshots: {}",
            failed.join(", ")
        );
        process::exit(1);
    }

    println!(
        "screenshots: OK (hud_seen={}, dir={})",
        hud_ok,
        cli.dir.display()
    );
}

fn find_window_by_pid_title(pid: u32, title: &str) -> Option<u32> {
    find_window_impl(Some(pid as i32), title)
}

fn find_window_by_title(title: &str) -> Option<u32> {
    find_window_impl(None, title)
}

fn find_window_impl(pid: Option<i32>, title: &str) -> Option<u32> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let arr: CFArray = copy_window_info(options, kCGNullWindowID)?;
    let key_owner_pid = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let key_name = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    let key_number = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };

    for raw in arr.iter() {
        let dict_ptr = *raw;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };
        let owner_pid = dict_value_i32(&dict, &key_owner_pid)?;
        if let Some(pid) = pid
            && owner_pid != pid
        {
            continue;
        }
        let name = dict_value_string(&dict, &key_name).unwrap_or_default();
        if name != title {
            continue;
        }
        return dict_value_u32(&dict, &key_number);
    }

    None
}

fn dict_value_string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    dict.find(key)
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
}

fn dict_value_i32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i32> {
    dict.find(key)
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n: CFNumber| n.to_i64())
        .map(|n| n as i32)
}

fn dict_value_u32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    dict_value_i32(dict, key).map(|v| v as u32)
}
