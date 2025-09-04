use std::collections::HashSet;

use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use super::{
    ax::{AXState, ax_is_trusted},
    cg::front_app_title_pid,
    event::FocusEvent,
};

/// Start the background CG/AX watcher thread that emits [`FocusEvent`]s.
pub fn start_watcher(tx: UnboundedSender<FocusEvent>) {
    std::thread::spawn(move || {
        // Skip known system apps that don't support AX observation
        const SKIP_APPS: &[&str] = &["WindowManager", "Dock", "Control Center", "Spotlight"];

        let mut last_app = String::new();
        let mut last_title = String::new();
        let mut last_pid: i32 = -1;
        let mut ax = AXState::default();
        let mut warned_apps = HashSet::<String>::new();
        debug!("Watcher thread started");
        loop {
            let (app, title, pid) = front_app_title_pid();
            if app != last_app {
                debug!("App changed: '{}' -> '{}' (pid={})", last_app, app, pid);
                last_app = app.clone();
                let _ = tx.send(FocusEvent::AppChanged {
                    title: app.clone(),
                    pid,
                });
            }
            if title != last_title {
                last_title = title.clone();
                let _ = tx.send(FocusEvent::TitleChanged { title, pid });
            }

            // Try to (re)attach AX observer when pid changes and we are trusted
            if pid != last_pid {
                last_pid = pid;

                let should_skip = SKIP_APPS.iter().any(|&skip_app| app == skip_app);

                if ax_is_trusted() && pid > 0 && !should_skip {
                    match ax.attach(pid, tx.clone()) {
                        Ok(()) => debug!("AX attached to app '{}'", app),
                        Err(e) => {
                            // Only log once per app to reduce noise; downgrade -25204 to debug
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
            }
            // If permission just granted and we haven't attached yet, try attach
            if !ax.have_source && ax_is_trusted() && last_pid > 0 {
                let should_skip = SKIP_APPS.iter().any(|&skip_app| last_app == skip_app);

                if !should_skip {
                    match ax.attach(last_pid, tx.clone()) {
                        Ok(()) => debug!("AX attached to app '{}' (permission granted)", last_app),
                        Err(e) => {
                            // Only log once per app; downgrade -25204 to debug
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

            // Service AX notifications if observer is attached
            ax.pump_runloop_once();

            // Keep polling cadence low-latency for app switches
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}
