//! IPC server implementation for hotkey manager

use std::{
    fs,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use mrpc::Server as MrpcServer;
use tokio::{select, time::sleep};
use tracing::{debug, trace};

use super::{IdleTimerState, service::HotkeyService};
use crate::{Error, Result};

/// IPC server
pub struct IPCServer {
    socket_path: String,
    service: HotkeyService,
}

impl IPCServer {
    /// Create a new IPC server
    pub fn new(
        socket_path: impl Into<String>,
        manager: mac_hotkey::Manager,
        shutdown: Arc<AtomicBool>,
        _proxy: tao::event_loop::EventLoopProxy<()>,
        idle_state: Arc<IdleTimerState>,
    ) -> Self {
        let service = HotkeyService::new(Arc::new(manager), shutdown, idle_state);

        Self {
            socket_path: socket_path.into(),
            service,
        }
    }

    /// Run the server
    pub async fn run(self) -> Result<()> {
        trace!("Starting MRPC server on socket: {}", self.socket_path);

        // Ensure the parent directory exists with conservative permissions (0700).
        // The path is controlled by our crate (default under a per-user runtime dir),
        // but we also support explicit paths via CLI.
        if let Some(parent) = Path::new(&self.socket_path).parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = fs::create_dir_all(parent);
            if let Ok(meta) = fs::metadata(parent) {
                let mut perms = meta.permissions();
                // 0o700: user-only access
                perms.set_mode(0o700);
                let _ = fs::set_permissions(parent, perms);
            }
        }

        // Guard unlink of any pre-existing path to avoid removing non-socket files or
        // sockets owned by other users. This defends against symlink tricks in
        // world-writable directories.
        validate_or_unlink_existing_socket(&self.socket_path)?;

        // Create server with our service
        let service = self.service.clone();
        let server = MrpcServer::from_fn(move || service.clone());

        // Start hotkey dispatcher early; focus watcher will be started
        // on first set_mode via HotkeyService to avoid redundant installs.
        self.service.start_hotkey_dispatcher();

        // Listen on Unix socket
        let server = server
            .unix(&self.socket_path)
            .await
            .map_err(|e| Error::Ipc(format!("Failed to bind to socket: {}", e)))?;

        trace!("MRPC server listening, waiting for client connections...");

        // Run the server until shutdown is requested. Dropping the run future
        // will close the listener and active connections gracefully.
        let shutdown = self.service.shutdown_flag();
        select! {
            res = server.run() => {
                res.map_err(|e| Error::Ipc(format!("Server error: {}", e)))?;
            }
            _ = async {
                // Poll the shutdown flag; wake up periodically.
                while !shutdown.load(Ordering::SeqCst) {
                    sleep(Duration::from_millis(50)).await;
                }
            } => {
                debug!("Shutdown flag set; stopping MRPC server");
                // server.run() future is dropped here, closing the socket and tasks
            }
        }

        Ok(())
    }
}

impl Drop for IPCServer {
    fn drop(&mut self) {
        // Best-effort cleanup: only unlink if it still points to a socket owned by us.
        let _ = validate_or_unlink_existing_socket(&self.socket_path);
    }
}

/// Validate that an existing path is a Unix domain socket owned by the current
/// user. If so, unlink it to make room for a new bind. If the path does not
/// exist, this is a no-op. If the path exists but is not a socket (or is not
/// owned by us), return an error and do not unlink.
fn validate_or_unlink_existing_socket(path: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Ipc(format!(
            "Failed to lstat existing path '{}': {}",
            path, e
        ))),
        Ok(meta) => {
            let ft = meta.file_type();
            if !ft.is_socket() {
                return Err(Error::Ipc(format!(
                    "Refusing to remove non-socket at '{}': {:?}",
                    path, ft
                )));
            }
            let uid = unsafe { libc::getuid() } as u32;
            if meta.uid() != uid {
                return Err(Error::Ipc(format!(
                    "Socket at '{}' not owned by current user (uid {} != {})",
                    path,
                    meta.uid(),
                    uid
                )));
            }
            fs::remove_file(path).map_err(|e| {
                Error::Ipc(format!(
                    "Failed to remove pre-existing socket '{}': {}",
                    path, e
                ))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    fn tmpdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let unique = format!(
            "hotki-test-{}-{}",
            unsafe { libc::getuid() },
            std::process::id()
        );
        p.push(unique);
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn guard_allows_absent_path() {
        let d = tmpdir();
        let sock = d.join("nope.sock");
        // Should be Ok even if path is absent
        let res = validate_or_unlink_existing_socket(sock.to_str().unwrap());
        assert!(res.is_ok());
        // Nothing created
        assert!(!sock.exists());
    }

    #[test]
    fn guard_refuses_regular_file() {
        let d = tmpdir();
        let p = d.join("regular.txt");
        fs::write(&p, b"hi").unwrap();
        let res = validate_or_unlink_existing_socket(p.to_str().unwrap());
        assert!(res.is_err());
        // Must not delete arbitrary files
        assert!(p.exists());
    }

    #[test]
    fn guard_refuses_symlink() {
        use std::os::unix::fs::symlink;
        let d = tmpdir();
        let target = d.join("target.txt");
        fs::write(&target, b"hi").unwrap();
        let link = d.join("link.sock");
        symlink(&target, &link).unwrap();
        let res = validate_or_unlink_existing_socket(link.to_str().unwrap());
        assert!(res.is_err());
        // Symlink should remain untouched
        assert!(link.exists());
    }

    #[test]
    fn guard_unlinks_owned_socket() {
        let d = tmpdir();
        let sock = d.join("owned.sock");
        let _listener = UnixListener::bind(&sock).unwrap();
        // Path now exists and is a socket; should be removed
        validate_or_unlink_existing_socket(sock.to_str().unwrap()).unwrap();
        assert!(!sock.exists());
    }
}
