//! Integration coverage for test-world event delivery.

use std::time::Duration;

use crate::{FocusChange, TestWorld, WindowKey, WorldEvent, WorldView, WorldWindow};

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

    let snap = world.snapshot();
    let focused = world.focused();
    assert_eq!(snap.len(), 1);
    assert_eq!(focused, Some(key));
    assert_eq!(
        world.focus_snapshot(),
        Some(hotki_protocol::FocusSnapshot {
            id: key.id,
            app: "TestApp".into(),
            title: "TestTitle".into(),
            pid: key.pid,
            display_id: None,
        })
    );
    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    let event = world.next_event_until(&mut cursor, deadline).await;
    assert!(
        matches!(
            event,
            Some(WorldEvent::FocusChanged(FocusChange::Focused(
                hotki_protocol::FocusSnapshot { pid: 42, .. }
            )))
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
        Some(WorldEvent::FocusChanged(FocusChange::Focused(
            hotki_protocol::FocusSnapshot { id: 7, title, .. }
        ))) if title == "Second"
    ));
    assert_eq!(
        world.focus_snapshot().map(|focus| focus.title),
        Some("Second".into())
    );
}

#[tokio::test]
async fn clearing_focus_emits_an_explicit_clear() {
    let world = TestWorld::new();
    let key = WindowKey { pid: 42, id: 7 };
    let window = WorldWindow {
        app: "TestApp".into(),
        title: "TestTitle".into(),
        pid: key.pid,
        id: key.id,
        display_id: None,
        focused: true,
    };
    world.set_snapshot(vec![window], Some(key));
    let mut cursor = world.subscribe();

    world.set_snapshot(Vec::new(), None);

    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
    assert_eq!(
        world.next_event_until(&mut cursor, deadline).await,
        Some(WorldEvent::FocusChanged(FocusChange::Cleared))
    );
}

#[test]
#[should_panic(expected = "absent from the supplied snapshot")]
fn focused_key_must_exist_in_test_snapshot() {
    TestWorld::new().set_snapshot(Vec::new(), Some(WindowKey { pid: 42, id: 7 }));
}
