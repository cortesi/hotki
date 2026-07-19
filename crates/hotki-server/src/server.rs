use std::{
    io, mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    ptr,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tao::{
    event::Event,
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::macos::{ActivationPolicy, EventLoopExtMacOS},
};
use tracing::{debug, error, info, trace};

use crate::{
    Error, Result, default_socket_path,
    ipc::{IPCServer, IdleTimerState},
    loop_wake::{self, WakeEvent},
    shutdown::{ShutdownCoordinator, ShutdownReason},
};

/// Default idle timeout in seconds after client disconnects.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 5;
/// Poll interval used while checking shutdown during a kqueue wait.
const PARENT_WATCH_SHUTDOWN_INTERVAL: Duration = Duration::from_millis(200);
/// Poll interval used by the process-existence fallback.
const PARENT_WATCH_FALLBACK_INTERVAL: Duration = Duration::from_millis(100);

/// Terminal result from the kqueue-backed parent watcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParentWatchOutcome {
    /// The watched process emitted an exit event.
    ParentExited,
    /// Another server lane requested shutdown.
    ShutdownRequested,
    /// Kqueue setup or waiting failed and polling should take over.
    KqueueUnavailable,
}

/// Request coordinated shutdown once an armed idle deadline has elapsed.
fn request_idle_shutdown_if_due(
    shutdown: &ShutdownCoordinator,
    deadline: Instant,
    now: Instant,
) -> bool {
    if now < deadline {
        return false;
    }
    shutdown.request(ShutdownReason::IdleExpired);
    true
}

/// Wait for the parent process to exit while observing cooperative shutdown.
fn watch_parent_with_kqueue(
    pid: libc::pid_t,
    shutdown: &ShutdownCoordinator,
) -> ParentWatchOutcome {
    // SAFETY: `kqueue` takes no arguments and returns a new owned file descriptor on success.
    let descriptor = unsafe { libc::kqueue() };
    if descriptor < 0 {
        return ParentWatchOutcome::KqueueUnavailable;
    }
    // SAFETY: `descriptor` was just returned by `kqueue` and ownership has not been transferred.
    let kqueue = unsafe { OwnedFd::from_raw_fd(descriptor) };

    // SAFETY: `kevent` is a plain C data structure for which all-zero is a valid baseline.
    let mut registration = unsafe { mem::zeroed::<libc::kevent>() };
    registration.ident = pid as usize;
    registration.filter = libc::EVFILT_PROC;
    registration.flags = libc::EV_ADD | libc::EV_ONESHOT;
    registration.fflags = libc::NOTE_EXIT;
    registration.udata = ptr::null_mut();

    // SAFETY: the descriptor is open, `registration` points to one initialized event, and the
    // output list is null because this call only submits the registration.
    let registered = unsafe {
        libc::kevent(
            kqueue.as_raw_fd(),
            &raw const registration,
            1,
            ptr::null_mut(),
            0,
            ptr::null(),
        ) == 0
    };
    if !registered {
        return ParentWatchOutcome::KqueueUnavailable;
    }

    loop {
        if shutdown.is_requested() {
            return ParentWatchOutcome::ShutdownRequested;
        }

        let timeout = libc::timespec {
            tv_sec: PARENT_WATCH_SHUTDOWN_INTERVAL.as_secs() as libc::time_t,
            tv_nsec: PARENT_WATCH_SHUTDOWN_INTERVAL.subsec_nanos() as libc::c_long,
        };
        // SAFETY: `event` is initialized storage for one result, `timeout` lives for the call,
        // and `kqueue` remains open for the duration of the wait.
        let mut event = unsafe { mem::zeroed::<libc::kevent>() };
        // SAFETY: all pointers reference initialized storage with the counts supplied, and the
        // changelist is null because the process filter was registered above.
        let result = unsafe {
            libc::kevent(
                kqueue.as_raw_fd(),
                ptr::null(),
                0,
                &raw mut event,
                1,
                &raw const timeout,
            )
        };

        if result > 0 {
            return ParentWatchOutcome::ParentExited;
        }
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return ParentWatchOutcome::KqueueUnavailable;
        }
    }
}

/// Return whether `pid` still names a process visible to this user.
fn process_is_alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal zero performs a process-existence query without delivering a signal.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Fall back to process polling when kqueue setup or waiting fails.
fn watch_parent_by_polling(pid: libc::pid_t, shutdown: &ShutdownCoordinator) {
    while !shutdown.is_requested() {
        if !process_is_alive(pid) {
            shutdown.request(ShutdownReason::ParentExited);
            return;
        }
        thread::sleep(PARENT_WATCH_FALLBACK_INTERVAL);
    }
}

/// Spawns a background thread that monitors a parent process PID.
/// When the parent process exits, it requests server shutdown.
/// If shutdown is requested by other means, the thread terminates gracefully.
fn spawn_parent_watcher(ppid: libc::pid_t, shutdown: ShutdownCoordinator) -> JoinHandle<()> {
    thread::spawn(move || match watch_parent_with_kqueue(ppid, &shutdown) {
        ParentWatchOutcome::ParentExited => {
            shutdown.request(ShutdownReason::ParentExited);
        }
        ParentWatchOutcome::ShutdownRequested => {}
        ParentWatchOutcome::KqueueUnavailable => {
            watch_parent_by_polling(ppid, &shutdown);
        }
    })
}

/// A hotkey server that manages the Tao event loop and MRPC IPC.
///
/// Notes
/// - Default socket path is per‑UID+PID; override with `with_socket_path`.
/// - When auto‑spawned by the UI, the server is handed the parent's PID via
///   `with_parent_pid` and requests shutdown as soon as the parent exits.
/// - After the last client disconnects, the server starts an idle timer
///   (configurable via `with_idle_timeout_secs`) and exits when it fires.
pub struct Server {
    socket_path: String,
    idle_timeout_secs: u64,
    parent_pid: Option<libc::pid_t>,
    event_tap_enabled: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Create a new hotkey server with default configuration
    pub fn new() -> Self {
        Self {
            socket_path: default_socket_path().to_string(),
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
            parent_pid: None,
            event_tap_enabled: true,
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

    /// Watch the given parent PID and shut down when it exits.
    pub fn with_parent_pid(mut self, pid: libc::pid_t) -> Self {
        self.parent_pid = Some(pid);
        self
    }

    /// Run without observing physical keyboard events.
    pub fn without_event_tap(mut self) -> Self {
        self.event_tap_enabled = false;
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
        let mut event_loop = EventLoopBuilder::<WakeEvent>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        loop_wake::set_main_proxy(proxy);

        // Set activation policy to Accessory to prevent dock icon
        event_loop.set_activation_policy(ActivationPolicy::Accessory);

        // Create the mac_hotkey manager
        debug!("Creating mac_hotkey::Manager");
        let manager = if self.event_tap_enabled {
            mac_hotkey::Manager::new().map_err(Error::from)?
        } else {
            mac_hotkey::Manager::without_event_tap()
        };
        debug!("mac_hotkey::Manager created successfully");

        // Create shutdown coordination
        let shutdown = ShutdownCoordinator::new();
        let idle_state = Arc::new(IdleTimerState::new(self.idle_timeout_secs));
        // Create the IPC server with the coordinator used by RPC shutdown.
        let ipc_server = IPCServer::new(
            &self.socket_path,
            manager,
            shutdown.clone(),
            idle_state.clone(),
        );
        let shutdown_for_ipc = shutdown.clone();

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
                    shutdown_for_ipc.request(ShutdownReason::IpcRuntimeFailed);
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
            shutdown_for_ipc.request(ShutdownReason::IpcStopped);

            info!("IPC server thread ending");
        });

        // If a parent PID is provided (standard when auto-spawned by the UI),
        // watch it and request shutdown immediately when it exits. This makes
        // the backend die as soon as the frontend goes away for any reason.
        let mut parent_watcher_thread = None;
        if let Some(ppid) = self.parent_pid {
            parent_watcher_thread = Some(spawn_parent_watcher(ppid, shutdown.clone()));
        }

        // NSWorkspace observer installation is triggered post-handshake via UserEvent

        // Run the event loop on the main thread without periodic polling; we
        // exclusively use user events (via EventLoopProxy) and explicit
        // WaitUntil deadlines for the idle timer.
        trace!("Starting tao event loop...");

        // Track an idle shutdown deadline once the client disconnects.
        // None means no disconnect or timer canceled by activity.
        let mut idle_deadline: Option<Instant> = None;
        let mut client_disconnected = false;
        // Keep the IPC thread handle so we can join on exit.
        let mut server_thread = Some(server_thread);
        // Ensure we only log the shutdown transition once.
        let mut shutdown_logged = false;

        let idle_state_for_loop = idle_state;
        event_loop.run(move |event, _, control_flow| {
            // Default to waiting until the next concrete event.
            *control_flow = ControlFlow::Wait;

            if matches!(event, Event::LoopDestroyed) {
                shutdown.request(ShutdownReason::EventLoopDestroyed);
                if let Some(h) = parent_watcher_thread.take()
                    && let Err(e) = h.join()
                {
                    error!("Parent watcher thread join failed: {:?}", e);
                }
                if let Some(h) = server_thread.take() {
                    if let Err(e) = h.join() {
                        error!("IPC server thread join failed: {:?}", e);
                    } else {
                        info!("Shutdown complete");
                    }
                } else {
                    info!("Shutdown complete");
                }
                idle_state_for_loop.disarm();
                return;
            }

            // Check for shutdown
            if shutdown.is_requested() {
                if !shutdown_logged {
                    // Log once when we first observe the transition to shutdown.
                    tracing::debug!("Shutdown requested, exiting event loop");
                    shutdown_logged = true;
                }
                *control_flow = ControlFlow::Exit;
                return;
            }

            // Handle client disconnect-driven idle timeout using a monotonic clock.
            if client_disconnected {
                match idle_deadline {
                    None => {
                        // Arm the idle timer on first observation of disconnect
                        let when = Instant::now() + Duration::from_secs(self.idle_timeout_secs);
                        idle_deadline = Some(when);
                        info!(
                            "Client disconnected, starting {}s idle timer",
                            self.idle_timeout_secs
                        );
                        idle_state_for_loop.arm(when);
                        *control_flow = ControlFlow::WaitUntil(when);
                    }
                    Some(when) => {
                        if request_idle_shutdown_if_due(&shutdown, when, Instant::now()) {
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
                if idle_deadline.is_some() {
                    idle_state_for_loop.disarm();
                }
                idle_deadline = None;
            }

            // Process events (most are handled internally by tao/mac-hotkey)
            match event {
                Event::NewEvents(_) | Event::MainEventsCleared | Event::RedrawEventsCleared => {
                    // These events fire frequently, ignore them
                }
                Event::UserEvent(WakeEvent::ClientConnected) => {
                    if client_disconnected {
                        client_disconnected = false;
                        idle_state_for_loop.disarm();
                        idle_deadline = None;
                        info!("Client reconnected, canceling idle timer");
                    }
                }
                Event::UserEvent(WakeEvent::ClientDisconnected) => {
                    client_disconnected = true;
                }
                Event::UserEvent(WakeEvent::Shutdown) => {}
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
    use std::{process, sync::mpsc};

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

    #[test]
    fn idle_expiry_requests_shutdown_before_event_loop_exit() {
        let shutdown = ShutdownCoordinator::new();
        let now = Instant::now();

        assert!(!request_idle_shutdown_if_due(
            &shutdown,
            now + Duration::from_secs(1),
            now
        ));
        assert!(!shutdown.is_requested());
        assert!(request_idle_shutdown_if_due(&shutdown, now, now));
        assert!(shutdown.is_requested());
    }

    #[test]
    fn test_spawn_parent_watcher_shutdown_cooperation() {
        let shutdown = ShutdownCoordinator::new();

        // Spawn watching our own process ID (which is definitely alive)
        let handle = spawn_parent_watcher(process::id() as libc::pid_t, shutdown.clone());

        // Sleep briefly to let the thread start
        thread::sleep(Duration::from_millis(50));

        // Request shutdown and see if the thread exits cooperatively!
        shutdown.request(ShutdownReason::Test);

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        });

        // Wait up to 1 second for the thread to join
        assert!(
            rx.recv_timeout(Duration::from_secs(1)).is_ok(),
            "Parent watcher thread failed to exit cooperatively on shutdown!"
        );
    }
}
