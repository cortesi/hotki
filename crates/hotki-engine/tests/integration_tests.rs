//! Integration coverage for engine dispatch, mode, and selector flows.

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant as StdInstant},
};

use hotki_protocol::MsgToUI;
use tokio::time::{advance, timeout};

use crate::test_support::{
    capture_all_active, create_test_engine_with_relay, recv_until, run_engine_test,
    run_engine_test_paused, set_on_relay_repeat, set_world_focus, set_world_focus_window,
    write_test_config,
};

#[test]
fn focus_change_triggers_rerender() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              local window = ctx.window
              if window ~= nil and window:app_matches("Safari") then
                menu:bind("a", "a", function(actx)
                  actx:shell("true")
                end)
              end
            end
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
fn selector_callback_keeps_opening_window_and_rebinds_with_closing_focus() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;
        set_world_focus_window(world.as_ref(), "Opening", "First", 11, 101).await;
        timeout(Duration::from_millis(200), async {
            loop {
                if engine
                    .current_focus_snapshot()
                    .is_some_and(|focus| focus.id == 101)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("engine should observe opening focus");

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              local window = ctx.window
              if window ~= nil and window.id == 202 then
                menu:bind("n", "new focus", function(actx) end)
              else
                menu:bind("a", "select", function(actx)
                  actx:select({
                    items = { "Only" },
                    on_select = function(select_ctx, item, query)
                      local opening = select_ctx.window
                      if opening == nil then error("selector lost opening window") end
                      select_ctx:notify("info", "Selector window", tostring(opening.id))
                    end,
                  })
                end)
              end
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set selector config");
        while rx.try_recv().is_ok() {}

        dispatch_ident(&engine, "a").await;
        let opened = recv_selector_update(&mut rx, 500)
            .await
            .expect("selector should open");
        assert_eq!(opened.items.len(), 1);

        set_world_focus_window(world.as_ref(), "Closing", "Second", 22, 202).await;
        timeout(Duration::from_millis(200), async {
            loop {
                if engine
                    .current_focus_snapshot()
                    .is_some_and(|focus| focus.id == 202)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("engine should observe closing focus");
        dispatch_ident(&engine, "return").await;

        assert_eq!(
            recv_notify_text(&mut rx, 500, "Selector window")
                .await
                .as_deref(),
            Some("101"),
            "selector callback should retain the opening window",
        );
        assert!(
            engine.resolve_id_for_ident("n").await.is_some(),
            "selector close should rebind with the closing activation focus"
        );

        let _ignored = fs::remove_file(path);
    });
}

#[test]
fn mode_entry_and_pop_updates_depth() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              menu:submenu("cmd+k", "menu", function(child, inner)
                child:bind("a", "back", function(actx)
                  actx:pop()
                end)
              end)
            end
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
    run_engine_test_paused(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              menu:bind("a", "repeat", function(actx)
                actx:until_keyup(function(repeat_ctx)
                  repeat_ctx:relay("b")
                end, {
                  delay_ms = 100,
                  interval_ms = 100,
                })
              end)
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        while rx.try_recv().is_ok() {}
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }

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

        tokio::task::yield_now().await;
        advance(Duration::from_millis(250)).await;
        for _ in 0..3 {
            tokio::task::yield_now().await;
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
fn hold_relay_ticks_and_stops_after_key_up() {
    run_engine_test_paused(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            local a = hotki.actions
            return function(menu)
              menu:bind("a", "relay", a.hold(a.relay("cmd+b")))
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        drain_ui(&mut rx);

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
            .expect("dispatch relay down");
        tokio::task::yield_now().await;
        advance(Duration::from_millis(500)).await;
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert!(
            count.load(Ordering::SeqCst) > 0,
            "held relay should produce repeat ticks"
        );

        engine
            .dispatch(a, mac_hotkey::EventKind::KeyUp, false)
            .await
            .expect("dispatch relay up");
        let count_after_release = count.load(Ordering::SeqCst);
        advance(Duration::from_millis(500)).await;
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            count_after_release,
            "released relay should stop repeating"
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn repeat_change_volume_ticks() {
    run_engine_test_paused(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              menu:bind("a", "repeat", function(actx)
                actx:until_keyup(function(repeat_ctx)
                  repeat_ctx:change_volume(5)
                  repeat_ctx:notify("info", "volume tick", "")
                end, {
                  delay_ms = 100,
                  interval_ms = 100,
                })
              end)
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        while rx.try_recv().is_ok() {}
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }

        let a = engine.resolve_id_for_ident("a").await.expect("id for a");
        engine
            .dispatch(a, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch a down");

        let mut ticks = 0usize;
        let started = StdInstant::now();
        while ticks < 2 && started.elapsed() < Duration::from_secs(2) {
            advance(Duration::from_millis(100)).await;
            for _ in 0..3 {
                tokio::task::yield_now().await;
            }
            while let Ok(msg) = rx.try_recv() {
                if let MsgToUI::Notify { title, .. } = msg
                    && title == "volume tick"
                {
                    ticks += 1;
                }
            }
        }
        assert!(
            ticks >= 2,
            "expected immediate and repeated change-volume ticks"
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
            return function(menu, ctx)
              menu:submenu("cmd+k", "cap", function(child, inner)
                child:capture()
                child:bind("a", "back", function(actx)
                  actx:pop()
                end)
              end)
            end
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
            return function(menu, ctx)
              menu:bind("r", "reload", function(actx)
                actx:reload_config()
              end)
            end
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

#[test]
fn failed_reload_keeps_previous_callbacks_active() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            local a = hotki.actions
            return function(menu)
              menu:bind("a", "old callback", a.notify("info", "Active", "old"))
              menu:bind("r", "reload", a.reload_config)
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        drain_ui(&mut rx);

        fs::write(&path, "return 42").expect("replace config with invalid root");
        dispatch_ident(&engine, "r").await;
        assert!(
            recv_notify_text(&mut rx, 500, "Config").await.is_some(),
            "failed reload should report its validation error"
        );

        dispatch_ident(&engine, "a").await;
        assert_eq!(
            recv_notify_text(&mut rx, 500, "Active").await.as_deref(),
            Some("old")
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn reload_discards_factory_submenu_with_changed_capture() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;
        let config = |label: &str, capture: bool| {
            format!(
                r#"
                local a = hotki.actions
                local function make_child(value: string): ModeRenderer
                  return function(menu)
                    menu:bind("n", value, a.notify("info", "Version", value))
                  end
                end
                local child = make_child("{label}")

                return function(menu)
                  menu:submenu("m", "mode", child, {{ capture = {capture} }})
                  menu:bind("r", "reload", a.reload_config, {{ global = true }})
                end
                "#
            )
        };
        let path = write_test_config(&config("old", true));
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        drain_ui(&mut rx);
        dispatch_ident(&engine, "m").await;
        assert_eq!(engine.get_depth().await, 1);

        fs::write(&path, config("new", false)).expect("write replacement config");
        dispatch_ident(&engine, "r").await;
        assert_eq!(
            engine.get_depth().await,
            0,
            "reload must discard the old child"
        );

        dispatch_ident(&engine, "m").await;
        dispatch_ident(&engine, "n").await;
        assert_eq!(
            recv_notify_text(&mut rx, 500, "Version").await.as_deref(),
            Some("new")
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn selector_select_runs_handler_with_item_and_query() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              menu:bind("cmd+k", "pick", function(actx)
                actx:select({
                  title = "Pick",
                  placeholder = "Filter",
                  items = { "Alpha", "Beta" },
                  on_select = function(select_ctx, item, query)
                    select_ctx:notify("info", "Selected", item.label .. ":" .. query)
                  end,
                  on_cancel = function(cancel_ctx)
                    cancel_ctx:notify("info", "Canceled", "cancel")
                  end,
                })
              end)
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        drain_ui(&mut rx);

        dispatch_ident(&engine, "cmd+k").await;
        let opened = recv_selector_update(&mut rx, 500)
            .await
            .expect("selector should open");
        assert_eq!(opened.title, "Pick");
        assert_eq!(opened.items.len(), 2);

        dispatch_ident(&engine, "a").await;
        let filtered = recv_selector_update(&mut rx, 500)
            .await
            .expect("selector should update query");
        assert_eq!(filtered.query, "a");

        dispatch_ident(&engine, "return").await;
        assert!(
            recv_until(&mut rx, 500, |m| matches!(m, MsgToUI::SelectorHide)).await,
            "selector should hide after selection"
        );
        assert_eq!(
            recv_notify_text(&mut rx, 500, "Selected").await.as_deref(),
            Some("Alpha:a")
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn selector_cancel_runs_cancel_handler() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              menu:bind("cmd+k", "pick", function(actx)
                actx:select({
                  items = { "Alpha" },
                  on_select = function(select_ctx, item, query)
                    select_ctx:notify("info", "Selected", item.label)
                  end,
                  on_cancel = function(cancel_ctx)
                    cancel_ctx:notify("info", "Canceled", "cancel")
                  end,
                })
              end)
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        drain_ui(&mut rx);

        dispatch_ident(&engine, "cmd+k").await;
        let _ = recv_selector_update(&mut rx, 500)
            .await
            .expect("selector should open");

        dispatch_ident(&engine, "escape").await;
        assert!(
            recv_until(&mut rx, 500, |m| matches!(m, MsgToUI::SelectorHide)).await,
            "selector should hide after cancel"
        );
        assert_eq!(
            recv_notify_text(&mut rx, 500, "Canceled").await.as_deref(),
            Some("cancel")
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn render_recovery_truncates_bad_child_mode_to_root() {
    run_engine_test(async move {
        let (engine, mut rx, world) = create_test_engine_with_relay(false).await;

        let path = write_test_config(
            r#"
            return function(menu, ctx)
              menu:submenu("cmd+k", "bad", function(child, inner)
                error("child render failed")
              end)
              menu:bind("x", "ok", function(actx)
                actx:shell("true")
              end)
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");

        set_world_focus(world.as_ref(), "TestApp", "Window", 123).await;
        let _ = recv_until(&mut rx, 200, |m| matches!(m, MsgToUI::HudUpdate { .. })).await;
        drain_ui(&mut rx);

        dispatch_ident(&engine, "cmd+k").await;

        assert_eq!(engine.get_depth().await, 0);
        assert!(
            recv_until(&mut rx, 500, |m| matches!(
                m,
                MsgToUI::Notify { title, .. } if title == "Config"
            ))
            .await,
            "render recovery should report the child render error"
        );

        let _ignored = fs::remove_file(&path);
    });
}

#[test]
fn unbound_key_up_is_noop() {
    run_engine_test(async move {
        let (engine, mut rx, _world) = create_test_engine_with_relay(false).await;

        engine
            .dispatch(999_999, mac_hotkey::EventKind::KeyUp, false)
            .await
            .expect("unbound key-up should be ignored");
        assert!(rx.try_recv().is_err());
    });
}

async fn dispatch_ident(engine: &crate::Engine, ident: &str) {
    let id = engine
        .resolve_id_for_ident(ident)
        .await
        .unwrap_or_else(|| panic!("id for {ident}"));
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .unwrap_or_else(|err| panic!("dispatch {ident}: {err}"));
}

fn drain_ui(rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>) {
    while rx.try_recv().is_ok() {}
}

async fn recv_selector_update(
    rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>,
    timeout_ms: u64,
) -> Option<hotki_protocol::SelectorSnapshot> {
    timeout(Duration::from_millis(timeout_ms), async {
        while let Some(msg) = rx.recv().await {
            if let MsgToUI::SelectorUpdate(snapshot) = msg {
                return Some(snapshot);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

async fn recv_notify_text(
    rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>,
    timeout_ms: u64,
    expected_title: &str,
) -> Option<String> {
    timeout(Duration::from_millis(timeout_ms), async {
        while let Some(msg) = rx.recv().await {
            if let MsgToUI::Notify { title, text, .. } = msg
                && title == expected_title
            {
                return Some(text);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}
