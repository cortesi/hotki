use hotki_protocol::MsgToUI;
use tokio::sync::mpsc::unbounded_channel;
use tracing::info;
use tracing_subscriber::prelude::*;

#[test]
fn forwards_logs_when_sink_set_and_stops_after_clear() {
    // Set up a channel to receive forwarded logs
    let (tx, mut rx) = unbounded_channel::<MsgToUI>();
    log_forward::set_sink(tx);

    // Build a subscriber with the forwarding layer only
    let subscriber = tracing_subscriber::registry().with(log_forward::layer());

    tracing::subscriber::with_default(subscriber, || {
        info!(target: "test_forward", "hello world");

        // Expect one forwarded message
        match rx.try_recv() {
            Ok(MsgToUI::Log { message, .. }) => {
                assert!(message.contains("hello world"));
            }
            other => panic!("expected forwarded log, got: {:?}", other),
        }

        // Clearing the sink should stop forwarding
        log_forward::clear_sink();
        info!(target: "test_forward", "this should not forward");

        // No additional messages should be received
        assert!(rx.try_recv().is_err());
    });
}
