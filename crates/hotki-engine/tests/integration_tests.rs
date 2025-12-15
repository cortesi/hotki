use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hotki_engine::test_support::{
    capture_all_active, create_test_engine_with_relay, recv_until, run_engine_test,
    set_on_relay_repeat, set_world_focus, write_test_config,
};
use hotki_protocol::MsgToUI;
use tokio::time::{sleep, timeout};

#[test]
fn focus_change_triggers_rerender() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            hotki.mode(|m, ctx| {
              if ctx.app.matches("Safari") {
                m.bind("a", "a", action.shell("true"));
              }
            });
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "Other", "Window", 1).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        assert!(
            engine
                .bindings_snapshot()
                .await
                .iter()
                .all(|(ident, _)| ident != "a"),
            "binding should be absent for non-Safari app"
        );

        set_world_focus(world.as_ref(), "Safari", "Window", 1).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        assert!(
            engine
                .bindings_snapshot()
                .await
                .iter()
                .any(|(ident, _)| ident == "a"),
            "binding should appear for Safari app"
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn mode_entry_and_pop_updates_depth() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            hotki.mode(|m, ctx| {
              m.mode("cmd+k", "menu", |m, ctx| {
                m.bind("a", "back", action.pop);
              });
            });
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        while rx.try_recv().is_ok() {}

        assert_eq!(engine.get_depth().await, 0);

        let cmd_k = engine
            .resolve_id_for_ident("cmd+k")
            .await
            .expect("id for cmd+k");
        engine
            .dispatch(cmd_k, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch cmd+k");
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        assert_eq!(engine.get_depth().await, 1);

        let a = engine.resolve_id_for_ident("a").await.expect("id for a");
        engine
            .dispatch(a, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch a");
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        assert_eq!(engine.get_depth().await, 0);

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn repeat_relay_ticks() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            hotki.mode(|m, ctx| {
              m.bind("a", "repeat", action.relay("b")).repeat_ms(100, 100);
            });
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        while rx.try_recv().is_ok() {}

        let count = Arc::new(AtomicUsize::new(0));
        let count2 = count.clone();
        set_on_relay_repeat(
            &engine,
            Arc::new(move |_id| {
                count2.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let a = engine.resolve_id_for_ident("a").await.expect("id for a");
        engine
            .dispatch(a, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch a down");

        // Wait until we observe at least one software repeat tick.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while count.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            sleep(Duration::from_millis(25)).await;
        }
        assert!(
            count.load(Ordering::SeqCst) > 0,
            "expected at least one relay repeat tick"
        );

        engine
            .dispatch(a, mac_hotkey::EventKind::KeyUp, false)
            .await
            .expect("dispatch a up");

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn capture_mode_sets_capture_all() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            hotki.mode(|m, ctx| {
              m.mode("cmd+k", "cap", |m, ctx| {
                m.capture();
                m.bind("a", "back", action.pop);
              });
            });
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        while rx.try_recv().is_ok() {}

        let cmd_k = engine
            .resolve_id_for_ident("cmd+k")
            .await
            .expect("id for cmd+k");
        engine
            .dispatch(cmd_k, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch cmd+k");
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;

        assert!(
            capture_all_active(&engine).await,
            "capture_all should be active when HUD is visible and capture mode is set"
        );

        let a = engine.resolve_id_for_ident("a").await.expect("id for a");
        engine
            .dispatch(a, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch a");
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;

        assert!(
            !capture_all_active(&engine).await,
            "capture_all should be disabled after popping to root and hiding HUD"
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn reload_config_action_does_not_deadlock() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            hotki.mode(|m, ctx| {
              m.bind("r", "reload", action.reload_config);
            });
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        while rx.try_recv().is_ok() {}

        let r = engine.resolve_id_for_ident("r").await.expect("id for r");
        let outcome = timeout(
            Duration::from_millis(2_000),
            engine.dispatch(r, mac_hotkey::EventKind::KeyDown, false),
        )
        .await;
        assert!(
            outcome.is_ok(),
            "reload_config dispatch timed out (possible deadlock)"
        );
        outcome.unwrap().expect("dispatch ok");

        let _ignored = fs::remove_file(&path);
    });
}
