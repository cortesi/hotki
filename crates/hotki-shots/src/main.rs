use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::Parser;

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
    if let Ok(p) = std::env::var("HOTKI_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("hotki")))
        .filter(|p| p.exists())
}

fn used_config_path(theme: &Option<String>) -> std::io::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let cfg_path = cwd.join("examples/test.ron");
    if theme.is_none() {
        return Ok(cfg_path);
    }
    let name = theme.as_ref().unwrap();
    match fs::read_to_string(&cfg_path) {
        Ok(s) => {
            let mut out = String::new();
            if s.contains("base_theme:") {
                let re = regex::Regex::new("base_theme\\s*:\\s*\"[^\"]*\"").unwrap();
                out = re
                    .replace(&s, format!("base_theme: \"{}\"", name))
                    .to_string();
            } else if let Some(pos) = s.find('(') {
                let (head, tail) = s.split_at(pos + 1);
                out.push_str(head);
                out.push('\n');
                out.push_str(&format!("    base_theme: \"{}\",\n", name));
                out.push_str(tail);
            } else {
                out = s;
            }
            let tmp = std::env::temp_dir().join(format!(
                "hotki-shots-{}-{}.ron",
                std::process::id(),
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

fn spawn_hotki(bin: &Path, cfg: &Path, logs: bool) -> std::io::Result<Child> {
    let mut cmd = Command::new(bin);
    if logs {
        // Respect parent's RUST_LOG if set; otherwise default to info for server logs
        if std::env::var_os("RUST_LOG").is_none() {
            cmd.env("RUST_LOG", "info");
        }
    }
    cmd.arg(cfg);
    cmd.spawn()
}

fn socket_path_for_pid(pid: u32) -> String {
    hotki_server::socket_path_for_pid(pid)
}

fn wait_for_hud(sock: &str, hotki_pid: u32, timeout_ms: u64) -> bool {
    // Connect client with retry
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => return false,
    };
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
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
    };

    if let Ok(conn) = client.connection() {
        // Send activation chord a few times while we wait
        inject_key(&rt, conn, "shift+cmd+0");
        let poll = Duration::from_millis(200);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = std::cmp::min(left, poll);
            match rt.block_on(async { tokio::time::timeout(chunk, conn.recv_event()).await }) {
                Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { cursor })) => {
                    let visible = cursor.viewing_root || cursor.depth() > 0;
                    if visible {
                        return true;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => {}
            }
            // Side-check using CG list
            if mac_winops::list_windows()
                .into_iter()
                .any(|w| w.pid == hotki_pid as i32 && w.title == "Hotki HUD")
            {
                return true;
            }
            inject_key(&rt, conn, "shift+cmd+0");
        }
    }
    false
}

fn inject_key(rt: &tokio::runtime::Runtime, conn: &mut hotki_server::Connection, ident: &str) {
    let ident = mac_keycode::Chord::parse(ident)
        .map(|c| c.to_string())
        .unwrap_or_else(|| ident.to_string());
    let _ = rt.block_on(async { conn.inject_key_down(&ident).await });
    std::thread::sleep(Duration::from_millis(60));
    let _ = rt.block_on(async { conn.inject_key_up(&ident).await });
}

type Rect = (i32, i32, i32, i32);
fn find_window_by_title(pid: u32, title: &str) -> Option<(u32, Option<Rect>)> {
    mac_winops::list_windows()
        .into_iter()
        .find(|w| w.pid == pid as i32 && w.title == title)
        .map(|w| {
            let rect = w.pos.map(|pos| (pos.x, pos.y, pos.width, pos.height));
            (w.id, rect)
        })
}

fn capture_window_by_id_or_rect(pid: u32, title: &str, dir: &Path, name: &str) -> bool {
    if let Some((win_id, rect_opt)) = find_window_by_title(pid, title) {
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
        // First, try capture by window id
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
        if let Some((x, y, w, h)) = rect_opt {
            let rect_arg = format!("{},{},{},{}", x, y, w, h);
            let status = Command::new("screencapture")
                .args([
                    OsStr::new("-x"),
                    OsStr::new("-R"),
                    OsStr::new(&rect_arg),
                    path.as_os_str(),
                ])
                .status();
            return matches!(status, Ok(s) if s.success());
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
            std::process::exit(2);
        }
    };

    let cfg_path = match used_config_path(&cli.theme) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ERROR: unable to read or write config: {}", e);
            std::process::exit(2);
        }
    };

    let mut child = match spawn_hotki(&hotki_bin, &cfg_path, cli.logs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: failed to spawn hotki: {}", e);
            std::process::exit(2);
        }
    };
    let hotki_pid = child.id();
    let sock = socket_path_for_pid(hotki_pid);

    if !wait_for_hud(&sock, hotki_pid, cli.timeout) {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("ERROR: HUD did not appear within {} ms", cli.timeout);
        std::process::exit(2);
    }

    let _ = fs::create_dir_all(&cli.dir);

    // Connect once for key injection
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ERROR: failed to create Tokio runtime: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            std::process::exit(2);
        }
    };
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
            std::process::exit(2);
        }
    };
    let conn = match client.connection() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: failed to get connection: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            std::process::exit(2);
        }
    };

    // Capture HUD first
    let hud_ok = capture_window_by_id_or_rect(hotki_pid, "Hotki HUD", &cli.dir, "hud");

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
        std::thread::sleep(gap);
        if let Some(n) = name {
            std::thread::sleep(Duration::from_millis(120));
            let _ = capture_window_by_id_or_rect(hotki_pid, "Hotki Notification", &cli.dir, n);
        }
    }

    // Exit HUD and shutdown server
    inject_key(&rt, conn, "shift+cmd+0");
    let _ = rt.block_on(async move { client.shutdown_server().await });
    let _ = child.kill();
    let _ = child.wait();

    println!(
        "screenshots: OK (hud_seen={}, dir={})",
        hud_ok,
        cli.dir.display()
    );
}
