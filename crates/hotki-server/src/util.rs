use crossbeam_channel::Receiver;
use std::thread;
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Bridge a crossbeam channel to a Tokio unbounded channel using a dedicated OS
/// thread.
///
/// Semantics
/// - Single consumer: the returned `UnboundedReceiver<T>` has single‑consumer
///   semantics. Only one task may receive from it.
/// - Backpressure: the Tokio side is unbounded. If the receiver is slow, items
///   accumulate in memory. The bridge never blocks on forwarding; it only
///   blocks when waiting for the next item from the crossbeam receiver.
/// - Closure and drop: the bridge thread forwards until either side closes. If
///   the Tokio receiver is dropped, forwarding fails and the thread exits. If
///   all crossbeam senders are dropped, `recv` returns an error, the thread
///   exits, and the Tokio receiver is closed after all forwarded items are
///   observed.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel as cb;
    use std::time::Duration;
    use tokio::time::timeout;

    async fn run_bridge_roundtrip(cap: usize, n: u32) {
        let (tx, rx) = cb::bounded::<u32>(cap);
        let mut rx_tokio = bridge_crossbeam_to_tokio(rx);

        thread::spawn(move || {
            for i in 0..n {
                tx.send(i).unwrap();
            }
            // sender drops here
        });

        let mut seen = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let v = timeout(Duration::from_secs(2), rx_tokio.recv())
                .await
                .expect("timed out waiting for item")
                .expect("channel closed before receiving all items");
            seen.push(v);
        }

        // After the last item, the bridge thread should notice the closed
        // crossbeam channel, drop its sender, and the tokio receiver should
        // eventually close. We expect `None` within the timeout.
        let tail = timeout(Duration::from_secs(2), rx_tokio.recv())
            .await
            .expect("timed out waiting for channel closure");
        assert!(
            tail.is_none(),
            "tokio receiver did not close after senders dropped"
        );

        // Basic order/coverage check
        assert_eq!(seen.len() as u32, n);
        for (i, v) in seen.iter().enumerate() {
            assert_eq!(*v, i as u32);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_delivers_and_closes_bounded0() {
        run_bridge_roundtrip(0, 16).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_delivers_and_closes_bounded4() {
        run_bridge_roundtrip(4, 32).await;
    }
}
