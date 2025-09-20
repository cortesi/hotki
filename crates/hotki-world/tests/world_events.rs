use hotki_world::{FocusChange, TestWorld, World, WorldEvent, WorldView};

#[test]
fn event_ring_overflow_tracks_lost_count() {
    let world = TestWorld::new();
    let mut cursor = world.subscribe();
    for _ in 0..300 {
        world.push_event(WorldEvent::FocusChanged(FocusChange::default()));
    }
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(20);
        let _ = world.next_event_until(&mut cursor, deadline).await;
    });
    assert!(
        cursor.lost_count > 0,
        "lost_count should increment on overflow"
    );
}

#[test]
fn reset_clears_subscriptions() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let world = World::spawn_noop();
        let _cursor = world.subscribe();
        let report = world.quiescence_report();
        assert_eq!(report.subscriptions, 1);
        let after = world.reset();
        assert_eq!(after.subscriptions, 0);
        assert!(world.is_quiescent());
    });
}
