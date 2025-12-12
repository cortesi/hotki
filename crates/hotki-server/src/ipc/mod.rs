use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod client;
pub use hotki_protocol::rpc;
mod server;
mod service;

pub use client::Connection;
pub(crate) use server::IPCServer;

/// Shared idle timer state exposed to both the Tao event loop and IPC service.
#[derive(Debug)]
pub(crate) struct IdleTimerState {
    timeout_secs: u64,
    armed: AtomicBool,
    deadline_ms: AtomicU64,
}

impl IdleTimerState {
    /// Create a new idle timer state tracker with the configured timeout.
    pub(crate) fn new(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            armed: AtomicBool::new(false),
            deadline_ms: AtomicU64::new(0),
        }
    }

    /// Return a snapshot of the current idle timer state.
    pub(crate) fn snapshot(&self) -> IdleTimerSnapshot {
        let armed = self.armed.load(Ordering::SeqCst);
        let raw_deadline = self.deadline_ms.load(Ordering::SeqCst);
        IdleTimerSnapshot {
            timeout_secs: self.timeout_secs,
            armed,
            deadline_ms: if armed && raw_deadline > 0 {
                Some(raw_deadline)
            } else {
                None
            },
        }
    }

    /// Mark the idle timer as armed with the supplied deadline.
    pub(crate) fn arm(&self, deadline: Instant) {
        let encoded = encode_deadline(deadline);
        self.deadline_ms.store(encoded, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Clear the idle timer state.
    pub(crate) fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.deadline_ms.store(0, Ordering::SeqCst);
    }
}

/// Immutable snapshot of the idle timer state.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IdleTimerSnapshot {
    /// Configured timeout in seconds.
    pub timeout_secs: u64,
    /// True when the timer is currently armed.
    pub armed: bool,
    /// Optional wall-clock deadline in milliseconds since the Unix epoch.
    pub deadline_ms: Option<u64>,
}

fn encode_deadline(deadline: Instant) -> u64 {
    let now_instant = Instant::now();
    // Saturate to u64::MAX if conversion overflows.
    let absolute = if deadline <= now_instant {
        SystemTime::now()
    } else {
        let delta = deadline.duration_since(now_instant);
        match SystemTime::now().checked_add(delta) {
            Some(ts) => ts,
            None => SystemTime::UNIX_EPOCH + Duration::from_secs(u64::MAX),
        }
    };
    absolute
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use hotki_protocol::rpc::{HotkeyNotification, InjectKeyReq, InjectKind};
    use mrpc::Value;

    use super::*;

    #[test]
    fn notify_name_is_notify() {
        assert_eq!(HotkeyNotification::Notify.as_str(), "notify");
    }

    #[test]
    fn set_config_roundtrip() {
        let cfg = config::Config::default();
        let v = client::enc_set_config(&cfg).expect("encode");
        let dec = service::dec_set_config_param(&v).expect("decode");
        // Default roundtrip should preserve style key font size default, etc.
        assert_eq!(
            format!("{:?}", cfg.hud(&config::Cursor::default()).mode),
            format!("{:?}", dec.hud(&config::Cursor::default()).mode)
        );
    }

    #[test]
    fn set_config_invalid_type_error_code() {
        let err =
            service::dec_set_config_param(&Value::String("oops".into())).expect_err("should error");
        match err {
            mrpc::RpcError::Service(se) => {
                assert_eq!(se.name, crate::error::RpcErrorCode::InvalidType.to_string());
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn set_config_invalid_binary_error_code() {
        let err =
            service::dec_set_config_param(&Value::Binary(vec![1, 2, 3])).expect_err("should error");
        match err {
            mrpc::RpcError::Service(se) => {
                assert_eq!(
                    se.name,
                    crate::error::RpcErrorCode::InvalidConfig.to_string()
                );
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn inject_key_roundtrip() {
        let req = InjectKeyReq {
            ident: "shift+cmd+0".into(),
            kind: InjectKind::Down,
            repeat: false,
        };
        let v = client::enc_inject_key(&req).expect("encode");
        let dec = service::dec_inject_key_param(&v).expect("decode inject");
        assert_eq!(req, dec);
    }
}
