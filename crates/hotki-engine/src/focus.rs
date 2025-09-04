use std::sync::{Arc, Mutex};

use mac_winops::focus::FocusEvent;

/// Represents the current focus state
#[derive(Clone, Debug)]
pub struct FocusState {
    pub app: String,
    pub title: String,
    pub pid: i32,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            app: String::new(),
            title: String::new(),
            pid: -1,
        }
    }
}

/// Handles focus events and maintains current application context
#[derive(Clone)]
pub struct FocusHandler {
    state: Arc<Mutex<FocusState>>,
}

impl FocusHandler {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FocusState::default())),
        }
    }

    /// Handle a focus event and update internal state
    pub fn handle_event(&self, event: FocusEvent) {
        let mut state = self.state.lock().unwrap();
        match event {
            FocusEvent::AppChanged {
                title: app_name,
                pid,
            } => {
                state.app = app_name;
                state.pid = pid;
            }
            FocusEvent::TitleChanged {
                title: window_title,
                pid,
            } => {
                state.title = window_title;
                state.pid = pid;
            }
        }
    }

    /// Get the current application context
    pub fn get_focus_state(&self) -> FocusState {
        self.state.lock().unwrap().clone()
    }

    /// Get the current process ID
    pub fn get_pid(&self) -> i32 {
        self.state.lock().unwrap().pid
    }

    /// Directly set the current process ID (for tools/tests)
    pub fn set_pid_for_tools(&self, pid: i32) {
        self.state.lock().unwrap().pid = pid;
    }
}

impl Default for FocusHandler {
    fn default() -> Self {
        Self::new()
    }
}
