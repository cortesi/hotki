//! Integration coverage for test-world event delivery.

use std::time::Duration;

use crate::{
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

#[tokio::test]
async fn same_window_title_change_emits_focus_event() {
    let world = TestWorld::new();
    let key = WindowKey { pid: 42, id: 7 };
    let mut cursor = world.subscribe();
    let window = |title: &str| WorldWindow {
        app: "TestApp".into(),
        title: title.into(),
        pid: key.pid,
        id: key.id,
        display_id: Some(1),
        focused: true,
    };

    world.set_snapshot(vec![window("First")], Some(key));
    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    world.next_event_until(&mut cursor, deadline).await;

    world.set_snapshot(vec![window("Second")], Some(key));
    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    let event = world.next_event_until(&mut cursor, deadline).await;

    assert!(matches!(
        event,
        Some(WorldEvent::FocusChanged(FocusChange {
            key: Some(event_key),
            focus: Some(hotki_protocol::FocusSnapshot { title, .. }),
        })) if event_key == key && title == "Second"
    ));
    assert_eq!(
        focused_snapshot(&world).await.map(|focus| focus.title),
        Some("Second".into())
    );
}
