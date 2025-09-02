//! Shared event tagging helpers used across crates.
//!
//! We tag injected events with a process-unique marker value in the
//! `EventSourceUserData` field so our taps can ignore them.

/// 'hotk' in ASCII bytes: 0x68 0x6f 0x74 0x6b -> 1752468299
pub const HOTK_TAG: i64 = 1_752_468_299;
