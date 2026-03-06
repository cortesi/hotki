use std::{
    env, fs,
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, OnceLock, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hotki_protocol::{
    DisplaysSnapshot, FontWeight, HudRow, HudState, HudStyle, Mode, NotifyConfig, NotifyPos,
    NotifyTheme, NotifyWindowStyle, Offset, Pos, SelectorStyle, Style,
    rpc::{InjectKind, ServerStatusLite},
};
use hotki_server::smoketest_bridge::{
    BridgeCommand, BridgeCommandId, BridgeReply, BridgeRequest, BridgeResponse, BridgeTimestampMs,
    control_socket_path,
};
use mac_keycode::Chord;
use parking_lot::Mutex as ParkingMutex;
use tracing::debug;

use super::{BridgeClient, BridgeEvent, DriverError};
use crate::tmp_paths;

fn sample_style() -> Style {
    let window = NotifyWindowStyle {
        bg: (0, 0, 0),
        title_fg: (255, 255, 255),
        body_fg: (255, 255, 255),
        title_font_size: 14.0,
        title_font_weight: FontWeight::Regular,
        body_font_size: 12.0,
        body_font_weight: FontWeight::Regular,
        icon: None,
    };
    Style {
        hud: HudStyle {
            mode: Mode::Hud,
            pos: Pos::Center,
            offset: Offset::default(),
            font_size: 14.0,
            title_font_weight: FontWeight::Regular,
            key_font_size: 14.0,
            key_font_weight: FontWeight::Regular,
            tag_font_size: 14.0,
            tag_font_weight: FontWeight::Regular,
            title_fg: (255, 255, 255),
            bg: (0, 0, 0),
            key_fg: (255, 255, 255),
            key_bg: (0, 0, 0),
            mod_fg: (255, 255, 255),
            mod_font_weight: FontWeight::Regular,
            mod_bg: (0, 0, 0),
            tag_fg: (255, 255, 255),
            opacity: 1.0,
            key_radius: 6.0,
            key_pad_x: 6.0,
            key_pad_y: 6.0,
            radius: 10.0,
            tag_submenu: "…".to_string(),
        },
        notify: NotifyConfig {
            width: 400.0,
            pos: NotifyPos::Right,
            opacity: 1.0,
            timeout: 2.0,
            buffer: 10,
            radius: 10.0,
            theme: NotifyTheme {
                info: window.clone(),
                warn: window.clone(),
                error: window.clone(),
                success: window,
            },
        },
        selector: SelectorStyle::default(),
    }
}

fn unique_control_socket() -> String {
    tmp_paths::unique_socket_path("smoketest-bridge-tests", "hotki-bridge-test")
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn bridge_test_lock() -> &'static ParkingMutex<()> {
    static LOCK: OnceLock<ParkingMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| ParkingMutex::new(()))
}

#[test]
fn inject_key_requires_initialization() {
    let _guard = bridge_test_lock().lock();
    let mut client = BridgeClient::new(unique_control_socket());
    let err = client.inject_key("cmd+shift+9").unwrap_err();
    assert!(matches!(err, DriverError::NotInitialized));
}

#[test]
fn inject_key_waits_for_binding_event() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let control_path = control_socket_path(&path);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();

    let server_path = control_path.clone();
    let handle = thread::spawn(move || {
        if let Err(err) = fs::remove_file(&server_path)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("failed to remove socket: {err}");
        }
        let listener = UnixListener::bind(&server_path).unwrap();
        ready_tx.send(()).unwrap();

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_handshake(&mut writer, cmd.command_id, 1);
            send_custom_hud_event(
                &mut writer,
                cmd.command_id + 10,
                vec![HudRow {
                    chord: Chord::parse("h").unwrap(),
                    desc: "Help".into(),
                    is_mode: false,
                    style: None,
                }],
            );
            assert!(try_read_command(&mut reader, 100).is_none());

            event_rx.recv().unwrap();
            send_custom_hud_event(
                &mut writer,
                cmd.command_id + 11,
                vec![HudRow {
                    chord: Chord::parse("cmd+b").unwrap(),
                    desc: "Binding".into(),
                    is_mode: false,
                    style: None,
                }],
            );

            let down = read_command(&mut reader);
            match &down.request {
                BridgeRequest::InjectKey {
                    ident,
                    kind,
                    repeat,
                } => {
                    assert_eq!(ident, "cmd+b");
                    assert!(matches!(kind, InjectKind::Down));
                    assert!(!repeat);
                }
                other => panic!("expected InjectKey down, got {:?}", other),
            }
            send_ack(&mut writer, down.command_id, 1);
            send_ok(&mut writer, down.command_id);

            let up = read_command(&mut reader);
            match &up.request {
                BridgeRequest::InjectKey {
                    ident,
                    kind,
                    repeat,
                } => {
                    assert_eq!(ident, "cmd+b");
                    assert!(matches!(kind, InjectKind::Up));
                    assert!(!repeat);
                }
                other => panic!("expected InjectKey up, got {:?}", other),
            }
            send_ack(&mut writer, up.command_id, 1);
            send_ok(&mut writer, up.command_id);
        }
    });

    ready_rx.recv().unwrap();
    let mut client = BridgeClient::new(path);
    client.ensure_ready(1_000).unwrap();
    let client = Arc::new(ParkingMutex::new(client));

    let injector_client = Arc::clone(&client);
    let injector = thread::spawn(move || injector_client.lock().inject_key("cmd+b"));
    thread::sleep(Duration::from_millis(50));
    event_tx.send(()).unwrap();
    injector.join().unwrap().unwrap();
    client.lock().reset();
    handle.join().unwrap();
    fs::remove_file(&control_path).ok();
}

#[test]
fn ensure_init_times_out_for_missing_socket() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let mut client = BridgeClient::new(path.clone());
    let err = client.ensure_ready(25).unwrap_err();
    match err {
        DriverError::InitTimeout { socket_path, .. } => {
            assert_eq!(socket_path, control_socket_path(&path))
        }
        other => panic!("expected InitTimeout, got {:?}", other),
    }
}

#[test]
fn check_alive_without_connection_reports_error() {
    let _guard = bridge_test_lock().lock();
    let mut client = BridgeClient::new(unique_control_socket());
    let err = client.check_alive().unwrap_err();
    assert!(matches!(err, DriverError::NotInitialized));
}

#[test]
fn control_socket_path_appends_suffix() {
    let _guard = bridge_test_lock().lock();
    let key = "HOTKI_CONTROL_SOCKET";
    let restore = env::var_os(key);
    unsafe {
        env::remove_var(key);
    }
    let path = tmp_paths::named_path("smoketest-bridge-tests", "hotki.sock")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(control_socket_path(&path), format!("{path}.bridge"));
    match restore {
        Some(value) => unsafe {
            env::set_var(key, value);
        },
        None => unsafe {
            env::remove_var(key);
        },
    }
}

fn ts() -> BridgeTimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn read_command(reader: &mut BufReader<UnixStream>) -> BridgeCommand {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(
        !line.trim().is_empty(),
        "unexpected EOF while reading bridge command"
    );
    serde_json::from_str(&line).unwrap()
}

fn try_read_command(reader: &mut BufReader<UnixStream>, timeout_ms: u64) -> Option<BridgeCommand> {
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
        .unwrap();
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => {
            if let Err(err) = reader.get_ref().set_read_timeout(None) {
                debug!(?err, "failed to clear test bridge read timeout");
            }
            None
        }
        Ok(_) => {
            if let Err(err) = reader.get_ref().set_read_timeout(None) {
                debug!(?err, "failed to clear test bridge read timeout");
            }
            Some(serde_json::from_str(&line).unwrap())
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut => {
            if let Err(err) = reader.get_ref().set_read_timeout(None) {
                debug!(?err, "failed to clear test bridge read timeout");
            }
            None
        }
        Err(err) => panic!("unexpected bridge read error: {err}"),
    }
}

fn send_reply(writer: &mut UnixStream, reply: &BridgeReply) {
    let mut bytes = serde_json::to_vec(reply).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).unwrap();
    writer.flush().unwrap();
}

fn send_ack(writer: &mut UnixStream, command_id: BridgeCommandId, queued: usize) {
    let reply = BridgeReply {
        command_id,
        timestamp_ms: ts(),
        response: BridgeResponse::Ack { queued },
    };
    send_reply(writer, &reply);
}

fn send_handshake(writer: &mut UnixStream, command_id: BridgeCommandId, clients: usize) {
    let response = BridgeResponse::Handshake {
        idle_timer: ServerStatusLite {
            idle_timeout_secs: 60,
            idle_timer_armed: false,
            idle_deadline_ms: None,
            clients_connected: clients,
        },
        notifications: Vec::new(),
    };
    let reply = BridgeReply {
        command_id,
        timestamp_ms: ts(),
        response,
    };
    send_reply(writer, &reply);
}

fn send_custom_hud_event(writer: &mut UnixStream, event_id: BridgeCommandId, rows: Vec<HudRow>) {
    let hud = HudState {
        visible: true,
        rows,
        depth: 1,
        breadcrumbs: vec!["Test".into()],
        style: sample_style(),
        capture: false,
    };
    let response = BridgeResponse::Event {
        event: Box::new(BridgeEvent::Hud {
            hud: Box::new(hud),
            displays: DisplaysSnapshot::default(),
        }),
    };
    let reply = BridgeReply {
        command_id: event_id,
        timestamp_ms: ts(),
        response,
    };
    send_reply(writer, &reply);
}

fn send_hud_event(writer: &mut UnixStream, event_id: BridgeCommandId) {
    send_custom_hud_event(
        writer,
        event_id,
        vec![HudRow {
            chord: Chord::parse("k").unwrap(),
            desc: "Key".into(),
            is_mode: false,
            style: None,
        }],
    );
}

fn send_ok(writer: &mut UnixStream, command_id: BridgeCommandId) {
    let reply = BridgeReply {
        command_id,
        timestamp_ms: ts(),
        response: BridgeResponse::Ok,
    };
    send_reply(writer, &reply);
}

fn send_err(writer: &mut UnixStream, command_id: BridgeCommandId, message: &str) {
    let reply = BridgeReply {
        command_id,
        timestamp_ms: ts(),
        response: BridgeResponse::Err {
            message: message.to_string(),
        },
    };
    send_reply(writer, &reply);
}

#[test]
fn ensure_init_retries_failed_handshake() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let control_path = control_socket_path(&path);
    let (ready_tx, ready_rx) = mpsc::channel();

    let server_path = control_path.clone();
    let handle = thread::spawn(move || {
        if let Err(err) = fs::remove_file(&server_path)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("failed to remove socket: {err}");
        }
        let listener = UnixListener::bind(&server_path).unwrap();
        ready_tx.send(()).unwrap();

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            let reply = BridgeReply {
                command_id: cmd.command_id,
                timestamp_ms: ts(),
                response: BridgeResponse::Err {
                    message: "handshake failed".into(),
                },
            };
            send_reply(&mut writer, &reply);
        }

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_handshake(&mut writer, cmd.command_id, 7);
            thread::sleep(Duration::from_millis(50));
        }
    });

    ready_rx.recv().unwrap();
    let mut client = BridgeClient::new(path);
    client.ensure_ready(1_000).unwrap();

    let clients = client
        .handshake()
        .unwrap()
        .map(|h| h.idle_timer.clients_connected)
        .unwrap();
    assert_eq!(clients, 7);

    client.reset();
    handle.join().unwrap();
    fs::remove_file(&control_path).ok();
}

#[test]
fn wait_for_idents_tracks_hud_events() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let control_path = control_socket_path(&path);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();

    let server_path = control_path.clone();
    let handle = thread::spawn(move || {
        if let Err(err) = fs::remove_file(&server_path)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("failed to remove socket: {err}");
        }
        let listener = UnixListener::bind(&server_path).unwrap();
        ready_tx.send(()).unwrap();

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_handshake(&mut writer, cmd.command_id, 1);
            send_custom_hud_event(
                &mut writer,
                cmd.command_id + 10,
                vec![HudRow {
                    chord: Chord::parse("h").unwrap(),
                    desc: "Help".into(),
                    is_mode: false,
                    style: None,
                }],
            );

            event_rx.recv().unwrap();
            send_custom_hud_event(
                &mut writer,
                cmd.command_id + 11,
                vec![HudRow {
                    chord: Chord::parse("cmd+b").unwrap(),
                    desc: "Binding".into(),
                    is_mode: false,
                    style: None,
                }],
            );

            if let Some(cmd) = try_read_command(&mut reader, 200) {
                panic!(
                    "unexpected bridge command after HUD event: {:?}",
                    cmd.request
                );
            }
        }
    });

    ready_rx.recv().unwrap();
    let mut client = BridgeClient::new(path);
    client.ensure_ready(1_000).unwrap();

    let notifier = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        event_tx.send(()).unwrap();
    });

    client.wait_for_idents(&["cmd+b"], 1_000).unwrap();
    notifier.join().unwrap();

    client.reset();
    handle.join().unwrap();
    fs::remove_file(&control_path).ok();
}

#[test]
fn inject_key_advances_sequence_after_bridge_error() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let control_path = control_socket_path(&path);
    let (ready_tx, ready_rx) = mpsc::channel();

    let server_path = control_path.clone();
    let handle = thread::spawn(move || {
        if let Err(err) = fs::remove_file(&server_path)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("failed to remove socket: {err}");
        }
        let listener = UnixListener::bind(&server_path).unwrap();
        ready_tx.send(()).unwrap();

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_handshake(&mut writer, cmd.command_id, 1);
            send_custom_hud_event(
                &mut writer,
                cmd.command_id + 10,
                vec![HudRow {
                    chord: Chord::parse("cmd+b").unwrap(),
                    desc: "Binding".into(),
                    is_mode: false,
                    style: None,
                }],
            );

            let first_down = read_command(&mut reader);
            assert_eq!(first_down.command_id, 1);
            send_ack(&mut writer, first_down.command_id, 1);
            send_err(&mut writer, first_down.command_id, "KeyNotBound: cmd+b");

            let second_down = read_command(&mut reader);
            assert_eq!(second_down.command_id, 2);
            send_ack(&mut writer, second_down.command_id, 1);
            send_ok(&mut writer, second_down.command_id);

            let up = read_command(&mut reader);
            assert_eq!(up.command_id, 3);
            send_ack(&mut writer, up.command_id, 1);
            send_ok(&mut writer, up.command_id);
        }
    });

    ready_rx.recv().unwrap();
    let mut client = BridgeClient::new(path);
    client.ensure_ready(1_000).unwrap();
    client.inject_key("cmd+b").unwrap();

    client.reset();
    handle.join().unwrap();
    fs::remove_file(&control_path).ok();
}

#[test]
fn reconnect_refreshes_handshake_and_clears_cache() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let control_path = control_socket_path(&path);
    let (ready_tx, ready_rx) = mpsc::channel();

    let server_path = control_path.clone();
    let handle = thread::spawn(move || {
        if let Err(err) = fs::remove_file(&server_path)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("failed to remove socket: {err}");
        }
        let listener = UnixListener::bind(&server_path).unwrap();
        ready_tx.send(()).unwrap();

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_hud_event(&mut writer, 1 << 32);
            send_handshake(&mut writer, cmd.command_id, 1);
        }

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_handshake(&mut writer, cmd.command_id, 2);

            let depth_cmd = read_command(&mut reader);
            assert!(matches!(depth_cmd.request, BridgeRequest::GetDepth));
            send_ack(&mut writer, depth_cmd.command_id, 1);
            let reply = BridgeReply {
                command_id: depth_cmd.command_id,
                timestamp_ms: ts(),
                response: BridgeResponse::Depth { depth: 2 },
            };
            send_reply(&mut writer, &reply);
            thread::sleep(Duration::from_millis(50));
        }
    });

    ready_rx.recv().unwrap();
    let mut client = BridgeClient::new(path);
    client.ensure_ready(1_000).unwrap();

    let hud_before = client.latest_hud().unwrap();
    assert!(hud_before.is_some());

    let depth = client.get_depth().unwrap();
    assert_eq!(depth, 2);

    let hud_after = client.latest_hud().unwrap();
    assert!(hud_after.is_none());

    let clients = client
        .handshake()
        .unwrap()
        .map(|h| h.idle_timer.clients_connected)
        .unwrap();
    assert_eq!(clients, 2);

    let buffered = client.event_buffer_len().unwrap();
    assert_eq!(buffered, 0);

    client.reset();
    handle.join().unwrap();
    fs::remove_file(&control_path).ok();
}

#[test]
fn shutdown_flags_post_shutdown_events() {
    let _guard = bridge_test_lock().lock();
    let path = unique_control_socket();
    let control_path = control_socket_path(&path);
    let (ready_tx, ready_rx) = mpsc::channel();

    let server_path = control_path.clone();
    let handle = thread::spawn(move || {
        if let Err(err) = fs::remove_file(&server_path)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("failed to remove socket: {err}");
        }
        let listener = UnixListener::bind(&server_path).unwrap();
        ready_tx.send(()).unwrap();

        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let cmd = read_command(&mut reader);
            assert!(matches!(cmd.request, BridgeRequest::Ping));
            send_ack(&mut writer, cmd.command_id, 1);
            send_handshake(&mut writer, cmd.command_id, 3);

            let shutdown_cmd = read_command(&mut reader);
            assert!(matches!(shutdown_cmd.request, BridgeRequest::Shutdown));
            send_ack(&mut writer, shutdown_cmd.command_id, 1);
            send_hud_event(&mut writer, shutdown_cmd.command_id + 100);
            send_ok(&mut writer, shutdown_cmd.command_id);
        }
    });

    ready_rx.recv().unwrap();
    let mut client = BridgeClient::new(path);
    client.ensure_ready(1_000).unwrap();

    let err = client.shutdown().unwrap_err();
    assert!(matches!(err, DriverError::PostShutdownMessage { .. }));

    client.reset();
    handle.join().unwrap();
    fs::remove_file(&control_path).ok();
}
