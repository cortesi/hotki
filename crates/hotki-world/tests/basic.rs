//! Integration coverage for test-world event delivery.

use std::time::Duration;

use hotki_world::{
    FocusChange, TestWorld, WindowKey, WorldEvent, WorldView, WorldWindow,
    focus_snapshot_for_change, focused_snapshot,
};

#[tokio::test]
async fn testworld_snapshot_and_focus() {
    let world = TestWorld::new();
    let key = WindowKey { pid: 42, id: 7 };
    let mut cursor = world.subscribe();
    world.set_snapshot(
        vec![WorldWindow {
            app: "TestApp".into(),
            title: "TestTitle".into(),
            pid: key.pid,
            id: key.id,
            display_id: None,
            focused: true,
        }],
        Some(key),
    );

    let snap = world.snapshot().await;
    let focused = world.focused().await;
    assert_eq!(snap.len(), 1);
    assert_eq!(focused, Some(key));
    assert_eq!(
        focused_snapshot(&world).await,
        Some(hotki_protocol::FocusSnapshot {
            app: "TestApp".into(),
            title: "TestTitle".into(),
            pid: key.pid,
            display_id: None,
        })
    );
    assert_eq!(
        focus_snapshot_for_change(
            &world,
            &FocusChange {
                key: Some(key),
                focus: None,
            },
        )
        .await
        .map(|focus| focus.pid),
        Some(key.pid)
    );

    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    let event = world.next_event_until(&mut cursor, deadline).await;
    assert!(
        matches!(
            event,
            Some(WorldEvent::FocusChanged(FocusChange {
                focus: Some(hotki_protocol::FocusSnapshot { pid: 42, .. }),
                ..
            }))
        ),
        "focus change event should be observed"
    );
}
