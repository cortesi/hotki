use std::{
    env,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use tao::{
    event::Event,
    event_loop::{ControlFlow, EventLoop},
    platform::macos::{ActivationPolicy, EventLoopExtMacOS},
};

use tracing::{debug, error, info, trace};

use crate::ipc::IPCServer;
use crate::{Error, Result, default_socket_path};

/// Default idle timeout in seconds after client disconnects.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 5;

/// A hotkey server that manages the event loop and IPC communication
pub struct Server {
    socket_path: String,
    idle_timeout_secs: u64,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Create a new hotkey server with default configuration
    pub fn new() -> Self {
        // Allow environment override; fallback to default.
        let idle_timeout_secs = env::var("HOTKI_SERVER_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS);
        Self {
            socket_path: default_socket_path().to_string(),
            idle_timeout_secs,
        }
    }

    /// Set the socket path for IPC communication
    pub fn with_socket_path(mut self, path: impl Into<String>) -> Self {
        self.socket_path = path.into();
        self
    }

    /// Override the idle timeout in seconds (after client disconnects).
    pub fn with_idle_timeout_secs(mut self, secs: u64) -> Self {
        self.idle_timeout_secs = secs;
        self
    }

    /// Run the server
    ///
    /// This will:
    /// 1. Create a tao event loop on the current thread (must be main thread on macOS)
    /// 2. Create a mac_hotkey::Manager
    /// 3. Start an IPC server in a background thread
    /// 4. Run the event loop until shutdown is requested
    ///
    /// The server will automatically shut down when:
    /// - The IPC client disconnects
    /// - An error occurs in the IPC server
    /// - The event loop is explicitly terminated
    pub fn run(self) -> Result<()> {
        info!("Starting hotkey server on socket: {}", self.socket_path);

        // Create the tao event loop (must be on main thread for macOS)
        let mut event_loop = EventLoop::new();
        let proxy = event_loop.create_proxy();
        // Keep a clone for posting wakeups from other threads
        let proxy_for_ipc = proxy.clone();
        mac_winops::focus::set_main_proxy(proxy);

        // Set activation policy to Accessory to prevent dock icon
        event_loop.set_activation_policy(ActivationPolicy::Accessory);

        // Create the mac_hotkey manager
        debug!("Creating mac_hotkey::Manager");
        let manager = mac_hotkey::Manager::new().map_err(|e| {
            Error::HotkeyOperation(format!("Failed to create mac_hotkey::Manager: {e}"))
        })?;
        debug!("mac_hotkey::Manager created successfully");

        // Create shutdown coordination
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        // Create the IPC server; pass shutdown flag so RPC can trigger exit
        let ipc_server = IPCServer::new(
            &self.socket_path,
            manager,
            shutdown_requested.clone(),
            proxy_for_ipc.clone(),
        );
        let shutdown_requested_clone = shutdown_requested.clone();
        let ipc_wakeup = proxy_for_ipc.clone();

        // Track when client disconnects to start idle countdown
        let client_disconnected = Arc::new(AtomicBool::new(false));
        let client_disconnected_clone = client_disconnected.clone();

        // Spawn IPC server in background thread
        let server_thread = thread::spawn(move || {
            // Create a single-threaded tokio runtime for the IPC server
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    error!("Failed to create tokio runtime: {}", e);
                    shutdown_requested_clone.store(true, Ordering::SeqCst);
                    // Wake the Tao loop to observe shutdown
                    let _ = ipc_wakeup.send_event(());
                    return;
                }
            };

            trace!("IPC server thread started, waiting for client connection...");

            // Run the IPC server
            runtime.block_on(async {
                if let Err(e) = ipc_server.run().await {
                    error!("IPC server error: {}", e);
                }
            });

            info!("IPC server thread ending, client disconnected");
            // Mark that client has disconnected to start idle countdown
            client_disconnected_clone.store(true, Ordering::SeqCst);
            // Wake the Tao loop so it can start/advance the idle timer immediately
            let _ = proxy_for_ipc.send_event(());
            // Don't immediately request shutdown - let idle timeout handle it
            // shutdown_requested_clone.store(true, Ordering::SeqCst);
        });

        // If a parent PID is provided (standard when auto-spawned by the UI),
        // watch it and request shutdown immediately when it exits. This makes
        // the backend die as soon as the frontend goes away for any reason.
        if let Ok(ppid_str) = env::var("HOTKI_PARENT_PID") {
            if let Ok(ppid) = ppid_str.parse::<libc::pid_t>() {
                let shutdown_for_parent = shutdown_requested.clone();
                thread::spawn(move || {
                    // Try kqueue EVFILT_PROC NOTE_EXIT for precise exit detection.
                    unsafe {
                        let kq = libc::kqueue();
                        if kq >= 0 {
                            let mut kev: libc::kevent = std::mem::zeroed();
                            // Configure event: watch specific PID for exit, one-shot.
                            kev.ident = ppid as usize;
                            kev.filter = libc::EVFILT_PROC;
                            kev.flags = libc::EV_ADD | libc::EV_ONESHOT;
                            kev.fflags = libc::NOTE_EXIT;
                            kev.data = 0;
                            kev.udata = std::ptr::null_mut();

                            let res = libc::kevent(
                                kq,
                                &kev as *const libc::kevent,
                                1,
                                std::ptr::null_mut(),
                                0,
                                std::ptr::null(),
                            );
                            if res == 0 {
                                // Wait for the event to fire.
                                let mut out: libc::kevent = std::mem::zeroed();
                                let _ = libc::kevent(
                                    kq,
                                    std::ptr::null(),
                                    0,
                                    &mut out as *mut libc::kevent,
                                    1,
                                    std::ptr::null(),
                                );
                                // Parent exited; request shutdown.
                                shutdown_for_parent.store(true, Ordering::SeqCst);
                                let _ = mac_winops::focus::post_user_event();
                                let _ = libc::close(kq);
                                return;
                            } else {
                                // Registration failed; fall back to polling below.
                                let _ = libc::close(kq);
                            }
                        }
                    }

                    // Fallback: poll with kill(ppid, 0) at short intervals.
                    loop {
                        // kill == 0 -> process exists; ESRCH -> doesn't exist
                        let alive = unsafe { libc::kill(ppid, 0) } == 0
                            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                        if !alive {
                            shutdown_for_parent.store(true, Ordering::SeqCst);
                            let _ = mac_winops::focus::post_user_event();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
            } else {
                tracing::warn!("HOTKI_PARENT_PID present but invalid: {:?}", ppid_str);
            }
        }

        // NSWorkspace observer installation is triggered post-handshake via UserEvent

        // Run the event loop on the main thread without periodic polling; we
        // exclusively use user events (via EventLoopProxy) and explicit
        // WaitUntil deadlines for the idle timer.
        trace!("Starting tao event loop...");

        // Track an idle shutdown deadline once the client disconnects.
        // None means no disconnect or timer canceled by activity.
        let mut idle_deadline: Option<Instant> = None;
        // Keep the IPC thread handle so we can join on exit.
        let mut server_thread = Some(server_thread);
        // Ensure we only log the shutdown transition once.
        let mut shutdown_logged = false;

        event_loop.run(move |event, _, control_flow| {
            // Default to waiting until the next concrete event.
            *control_flow = ControlFlow::Wait;

            // Check for shutdown
            if shutdown_requested.load(Ordering::SeqCst) {
                if !shutdown_logged {
                    // Log once when we first observe the transition to shutdown.
                    tracing::debug!("Shutdown requested, exiting event loop");
                    shutdown_logged = true;
                }
                *control_flow = ControlFlow::Exit;
                return;
            }

            // Handle client disconnect-driven idle timeout using a monotonic clock.
            if client_disconnected.load(Ordering::SeqCst) {
                match idle_deadline {
                    None => {
                        // Arm the idle timer on first observation of disconnect
                        let when = Instant::now() + Duration::from_secs(self.idle_timeout_secs);
                        idle_deadline = Some(when);
                        info!(
                            "Client disconnected, starting {}s idle timer",
                            self.idle_timeout_secs
                        );
                        *control_flow = ControlFlow::WaitUntil(when);
                    }
                    Some(when) => {
                        if Instant::now() >= when {
                            info!(
                                "Idle timeout reached ({}s since client disconnect), shutting down",
                                self.idle_timeout_secs
                            );
                            *control_flow = ControlFlow::Exit;
                            return;
                        } else {
                            // Keep sleeping until the existing deadline, unless other events arrive.
                            *control_flow = ControlFlow::WaitUntil(when);
                        }
                    }
                }
            } else {
                // No disconnect or it was canceled; ensure no idle timer is pending.
                idle_deadline = None;
            }

            // Process events (most are handled internally by tao/mac-hotkey)
            match event {
                Event::NewEvents(_) | Event::MainEventsCleared | Event::RedrawEventsCleared => {
                    // These events fire frequently, ignore them
                }
                Event::UserEvent(()) => {
                    if let Err(e) = mac_winops::focus::install_ns_workspace_observer() {
                        error!("Failed to install NSWorkspace observer: {}", e);
                    }
                    // Run any queued main-thread operations (e.g., non-native fullscreen)
                    mac_winops::drain_main_ops();
                    // User events indicate client activity - reset disconnect timer if set
                    if client_disconnected.load(Ordering::SeqCst) {
                        client_disconnected.store(false, Ordering::SeqCst);
                        idle_deadline = None;
                        info!("Client reconnected, canceling idle timer");
                    }
                }
                Event::LoopDestroyed => {
                    // Join the IPC server thread and emit a single success line.
                    if let Some(h) = server_thread.take() {
                        if let Err(e) = h.join() {
                            error!("IPC server thread join failed: {:?}", e);
                        } else {
                            info!("Shutdown complete");
                        }
                    } else {
                        info!("Shutdown complete");
                    }
                }
                _ => {
                    // Log other events at trace level for debugging
                    trace!("Event loop received: {:?}", event);
                }
            }
        });

        // The event loop runs forever and only exits when control flow is set to Exit
        // This Ok(()) is technically unreachable but required by the function signature
        #[allow(unreachable_code)]
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_with_methods() {
        // Test with_socket_path
        let server = Server::new().with_socket_path("/custom/path.sock");
        assert_eq!(server.socket_path, "/custom/path.sock");

        // Test chaining from new
        let server = Server::new()
            .with_socket_path("/initial/path.sock")
            .with_socket_path("/another/path.sock");
        assert_eq!(server.socket_path, "/another/path.sock");
    }

    #[test]
    fn test_server_default() {
        let server = Server::default();
        assert_eq!(server.socket_path, default_socket_path());
    }
}
