use std::collections::HashSet;

use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use super::{
    FocusSnapshot,
    ax::{AXState, AxEvent, ax_is_trusted},
};

// Legacy FocusEvent watcher removed.

/// Start the background CG/AX watcher thread that emits coarser `FocusSnapshot`s.
pub fn start_watcher_snapshots(tx: UnboundedSender<FocusSnapshot>) {
    std::thread::spawn(move || {
        // Skip known system apps that don't support AX observation
        const SKIP_APPS: &[&str] = &["WindowManager", "Dock", "Control Center", "Spotlight"];

        let mut last_app = String::new();
        let mut last_title = String::new();
        let mut last_pid: i32 = -1;

        let mut ax = AXState::default();
        let mut warned_apps = HashSet::<String>::new();
        let (tx_ax, mut rx_ax) = tokio::sync::mpsc::unbounded_channel::<AxEvent>();

        debug!("Snapshot watcher thread started");
        loop {
            let (app, mut title, pid) = match crate::frontmost_window() {
                Some(w) => (w.app, w.title, w.pid),
                None => (String::new(), String::new(), -1),
            };

            let app_changed = app != last_app || pid != last_pid;

            if app_changed {
                // Attach/detach AX for new pid
                let should_skip = SKIP_APPS.iter().any(|&skip_app| app == skip_app);
                if ax_is_trusted() && pid > 0 && !should_skip {
                    match ax.attach(pid, tx_ax.clone()) {
                        Ok(()) => debug!("AX attached to app '{}'", app),
                        Err(e) => {
                            if !warned_apps.contains(&app) {
                                match e {
                                    super::ax::Error::AddNotificationAlreadyRegistered => {
                                        debug!("AX attach non-fatal for app '{}': {}", app, e)
                                    }
                                    _ => warn!("AX attach failed for app '{}': {}", app, e),
                                }
                                warned_apps.insert(app.clone());
                            }
                        }
                    }
                } else {
                    ax.detach();
                }

                // Debounce briefly to allow AX to surface an updated title
                let start = std::time::Instant::now();
                let debounce = std::time::Duration::from_millis(40);
                while start.elapsed() < debounce {
                    // Drain AX events to capture a fresher title
                    while let Ok(ev) = rx_ax.try_recv() {
                        match ev {
                            AxEvent::TitleChanged { title: t, pid: p } => {
                                if p == pid {
                                    title = t;
                                }
                            }
                        }
                    }
                    ax.pump_runloop_once();
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            } else {
                // On title-only changes, update quickly from AX if available
                while let Ok(ev) = rx_ax.try_recv() {
                    match ev {
                        AxEvent::TitleChanged { title: t, pid: p } => {
                            if p == pid {
                                title = t;
                            }
                        }
                    }
                }
                ax.pump_runloop_once();
            }

            // Emit snapshot only if anything changed
            if app != last_app || title != last_title || pid != last_pid {
                let _ = tx.send(FocusSnapshot {
                    app: app.clone(),
                    title: title.clone(),
                    pid,
                });
                last_app = app;
                last_title = title;
                last_pid = pid;
            }

            // If permission just granted and we haven't attached yet, try attach
            if !ax.have_source && ax_is_trusted() && last_pid > 0 {
                let should_skip = SKIP_APPS.iter().any(|&skip_app| last_app == skip_app);
                if !should_skip {
                    match ax.attach(last_pid, tx_ax.clone()) {
                        Ok(()) => debug!("AX attached to app '{}' (permission granted)", last_app),
                        Err(e) => {
                            if !warned_apps.contains(&last_app) {
                                match e {
                                    super::ax::Error::AddNotification(-25204) => {
                                        debug!(
                                            "AX attach non-fatal post-permission for app '{}': {}",
                                            last_app, e
                                        )
                                    }
                                    _ => warn!(
                                        "AX attach failed post-permission for app '{}': {}",
                                        last_app, e
                                    ),
                                }
                                warned_apps.insert(last_app.clone());
                            }
                        }
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}
