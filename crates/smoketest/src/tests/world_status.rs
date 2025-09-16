//! World status smoketest.

use std::{
    env, thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    runtime,
    session::HotkiSession,
    util::resolve_hotki_bin,
};

/// Verify that world status reports healthy permissions and polling parameters.
pub fn run_world_status_test(timeout_ms: u64, _logs: bool) -> Result<()> {
    let bin = resolve_hotki_bin().ok_or(Error::HotkiBinNotFound)?;
    // Use default test config for server
    let cwd = env::current_dir()?;
    let cfg_path = cwd.join(config::PATHS.test_config);
    if !cfg_path.exists() {
        return Err(Error::MissingConfig(cfg_path));
    }

    let mut session = HotkiSession::launch_with_config(&bin, &cfg_path, _logs)?;

    // Connect client with retries until timeout
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut client = loop {
        match runtime::block_on(async {
            hotki_server::Client::new_with_socket(session.socket_path())
                .with_connect_only()
                .connect()
                .await
        }) {
            Ok(Ok(c)) => break c,
            _ => {
                if Instant::now() >= deadline {
                    return Err(Error::IpcDisconnected {
                        during: "world-status connect",
                    });
                }
                thread::sleep(Duration::from_millis(config::RETRY.fast_delay_ms));
            }
        }
    };
    let conn = client.connection().map_err(|_| Error::IpcDisconnected {
        during: "world-status conn",
    })?;

    // Poll world status a few times to let world tick at least once
    let mut ok = false;
    for _ in 0..10 {
        match runtime::block_on(async { conn.get_world_status().await }) {
            Ok(Ok(ws)) => {
                if ws.accessibility == 1
                    && ws.screen_recording == 1
                    && ws.current_poll_ms >= 10
                    && ws.current_poll_ms <= 5000
                {
                    ok = true;
                    break;
                }
            }
            Ok(Err(e)) => return Err(Error::InvalidState(e.to_string())),
            Err(e) => return Err(e),
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !ok {
        return Err(Error::InvalidState(
            "world-status acceptance conditions not met (check permissions)".into(),
        ));
    }

    // Cleanly shutdown server
    let _res = runtime::block_on(async { client.shutdown_server().await });
    session.kill_and_wait();
    Ok(())
}
