//! IPC server implementation for hotkey manager

use std::{
    env, fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use mrpc::Server as MrpcServer;
use tokio::{select, time::sleep};
use tracing::{debug, trace};

use super::service::HotkeyService;
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
    ) -> Self {
        let mut builder = HotkeyService::builder(Arc::new(manager), shutdown);
        if let Ok(v) = env::var("HOTKI_MAX_INFLIGHT_PER_ID")
            && let Ok(n) = v.parse::<usize>()
        {
            builder = builder.max_in_flight_per_id(n);
        }
        let service = builder.build();

        Self {
            socket_path: socket_path.into(),
            service,
        }
    }

    /// Run the server
    pub async fn run(self) -> Result<()> {
        trace!("Starting MRPC server on socket: {}", self.socket_path);

        // Remove existing socket file if it exists
        let _ = fs::remove_file(&self.socket_path);

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
        // Clean up socket file
        let _ = fs::remove_file(&self.socket_path);
    }
}
