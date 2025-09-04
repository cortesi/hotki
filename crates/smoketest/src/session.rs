use std::{
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};

use crate::SmkError;

pub(crate) struct HotkiSession {
    child: Child,
    sock: String,
}

impl HotkiSession {
    pub(crate) fn launch_with_config(
        hotki_bin: &Path,
        cfg_path: &Path,
        with_logs: bool,
    ) -> Result<HotkiSession, SmkError> {
        let mut cmd = Command::new(hotki_bin);
        if with_logs {
            cmd.env(
                "RUST_LOG",
                "info,hotki=info,hotki_server=info,hotki_engine=info,mac_hotkey=info,mac_focus_watcher=info,mrpc::connection=off",
            );
        }
        let child = cmd
            .arg(cfg_path)
            .spawn()
            .map_err(|e| SmkError::SpawnFailed(e.to_string()))?;
        let sock = hotki_server::socket_path_for_pid(child.id());
        Ok(HotkiSession { child, sock })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn socket_path(&self) -> &str {
        &self.sock
    }

    pub(crate) fn wait_for_hud(&self, timeout_ms: u64) -> (bool, u64) {
        // Try to connect and wait for HudUpdate indicating HUD visible.
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return (false, 0),
        };
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let start = Instant::now();

        // Connect with retry
        let mut attempts = 0;
        let mut client = loop {
            match rt.block_on(async {
                hotki_server::Client::new_with_socket(self.socket_path())
                    .with_connect_only()
                    .connect()
                    .await
            }) {
                Ok(c) => break c,
                Err(_) => {
                    attempts += 1;
                    if Instant::now() >= deadline {
                        return (false, start.elapsed().as_millis() as u64);
                    }
                    let delay = if attempts <= 3 { 200 } else { 50 };
                    std::thread::sleep(Duration::from_millis(delay));
                    continue;
                }
            }
        };

        // Borrow connection
        let conn = match client.connection() {
            Ok(c) => c,
            Err(_) => return (false, start.elapsed().as_millis() as u64),
        };

        // Send activation chord periodically until HUD visible
        let relayer = relaykey::RelayKey::new_unlabeled();
        let mut last_sent = None;
        if let Some(ch) = mac_keycode::Chord::parse("shift+cmd+0") {
            let pid = 0;
            relayer.key_down(pid, ch.clone(), false);
            std::thread::sleep(Duration::from_millis(80));
            relayer.key_up(pid, ch);
            last_sent = Some(Instant::now());
        }

        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = std::cmp::min(left, Duration::from_millis(300));
            let res = rt.block_on(async { tokio::time::timeout(chunk, conn.recv_event()).await });
            match res {
                Ok(Ok(msg)) => match msg {
                    hotki_protocol::MsgToUI::HudUpdate { cursor, .. } => {
                        let depth = cursor.depth();
                        let visible = cursor.viewing_root || depth > 0;
                        if visible {
                            return (true, start.elapsed().as_millis() as u64);
                        }
                    }
                    _ => {}
                },
                Ok(Err(_)) => break,
                Err(_) => {}
            }
            if let Some(last) = last_sent {
                if last.elapsed() >= Duration::from_millis(1000) {
                    if let Some(ch) = mac_keycode::Chord::parse("shift+cmd+0") {
                        let pid = 0;
                        relayer.key_down(pid, ch.clone(), false);
                        std::thread::sleep(Duration::from_millis(80));
                        relayer.key_up(pid, ch);
                    }
                    last_sent = Some(Instant::now());
                }
            }
        }
        (false, start.elapsed().as_millis() as u64)
    }

    pub(crate) fn shutdown(&self) {
        if let Ok(rt) = tokio::runtime::Runtime::new() {
            let sock = self.sock.clone();
            rt.block_on(async move {
                if let Ok(mut c) = hotki_server::Client::new_with_socket(&sock)
                    .with_connect_only()
                    .connect()
                    .await
                {
                    let _ = c.shutdown_server().await;
                }
            });
        }
    }

    pub(crate) fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
