use crossbeam_channel::Receiver;
use std::thread;
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Bridge a crossbeam receiver into a Tokio unbounded receiver using a dedicated OS thread.
///
/// - Blocks on the crossbeam `recv` in a spawned thread.
/// - Forwards items into a Tokio `mpsc::unbounded_channel`.
/// - Stops when either side closes.
pub fn bridge_crossbeam_to_tokio<T: Send + 'static>(rx: Receiver<T>) -> UnboundedReceiver<T> {
    let (tx_tokio, rx_tokio) = mpsc::unbounded_channel();

    thread::spawn(move || {
        while let Ok(item) = rx.recv() {
            if tx_tokio.send(item).is_err() {
                break;
            }
        }
    });

    rx_tokio
}
