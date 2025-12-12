use std::{collections::VecDeque, future::Future, io, path::PathBuf, pin::Pin};

use hotki_server::smoketest_bridge::{
    BridgeCommand, BridgeCommandId, BridgeEvent, BridgeReply, BridgeResponse, now_millis,
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::{broadcast, mpsc, oneshot},
};

use crate::control::{ControlMsg, TestCommand};

/// Spawn the UI-side listener that proxies smoketest bridge requests.
pub async fn init_test_bridge(
    path: PathBuf,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    events: broadcast::Sender<BridgeEvent>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    if let Err(err) = fs::remove_file(&path).await
        && err.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(?err, socket = %path.display(), "failed to remove stale test bridge socket");
    }
    let listener = UnixListener::bind(&path)?;
    let cleanup_path = path.clone();
    tokio::spawn(async move {
        if let Err(err) = run_test_bridge(listener, tx_ctrl_runtime, events).await {
            tracing::debug!(?err, "smoketest bridge listener exited");
        }
        if let Err(err) = fs::remove_file(&cleanup_path).await
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(?err, "failed to remove smoketest bridge socket on shutdown");
        }
    });
    Ok(())
}

/// Accept incoming bridge clients and spawn per-connection handlers.
async fn run_test_bridge(
    listener: UnixListener,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    events: broadcast::Sender<BridgeEvent>,
) -> io::Result<()> {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let tx = tx_ctrl_runtime.clone();
                let rx = events.subscribe();
                tokio::spawn(async move {
                    if let Err(err) = handle_test_bridge_client(stream, tx, rx).await {
                        tracing::debug!(?err, "smoketest bridge client disconnected");
                    }
                });
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                continue;
            }
            Err(err) => {
                return Err(err);
            }
        }
    }
}

/// Future that resolves with the command id and final bridge response.
type ProcessingFuture = Pin<Box<dyn Future<Output = (BridgeCommandId, BridgeResponse)> + Send>>;

/// Process commands from a single smoketest bridge client connection.
async fn handle_test_bridge_client(
    stream: UnixStream,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    mut event_rx: broadcast::Receiver<BridgeEvent>,
) -> io::Result<()> {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);
    let mut lines = reader.lines();

    let mut pending: VecDeque<BridgeCommand> = VecDeque::new();
    let mut processing: Option<ProcessingFuture> = None;
    let mut expected_command: BridgeCommandId = 0;
    let mut next_event_id: BridgeCommandId = 1 << 63;

    loop {
        tokio::select! {
            maybe_line = lines.next_line() => {
                match maybe_line? {
                    Some(line) => {
                        handle_bridge_line(
                            line,
                            &mut writer,
                            &mut pending,
                            &mut processing,
                            &tx_ctrl_runtime,
                            &mut expected_command,
                        ).await?;
                    }
                    None => break,
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        let reply = BridgeReply {
                            command_id: next_event_id,
                            timestamp_ms: now_millis(),
                            response: BridgeResponse::Event { event },
                        };
                        write_bridge_reply(&mut writer, reply).await?;
                        next_event_id = next_event_id.wrapping_add(1);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            result = async {
                if let Some(fut) = processing.as_mut() {
                    Some(fut.await)
                } else {
                    None
                }
            }, if processing.is_some() => {
                if let Some((command_id, response)) = result {
                    let reply = BridgeReply {
                        command_id,
                        timestamp_ms: now_millis(),
                        response,
                    };
                    write_bridge_reply(&mut writer, reply).await?;
                    processing = None;
                    drive_queue(&mut pending, &mut processing, &tx_ctrl_runtime, &mut writer).await?;
                }
            }
        }
    }

    writer.flush().await?;
    Ok(())
}

/// Process a single inbound bridge line: validate sequence, enqueue, and ACK.
async fn handle_bridge_line(
    line: String,
    writer: &mut BufWriter<OwnedWriteHalf>,
    pending: &mut VecDeque<BridgeCommand>,
    processing: &mut Option<ProcessingFuture>,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    expected_command: &mut BridgeCommandId,
) -> io::Result<()> {
    if line.trim().is_empty() {
        return Ok(());
    }

    let command: BridgeCommand = match serde_json::from_str(&line) {
        Ok(cmd) => cmd,
        Err(err) => {
            let reply = BridgeReply {
                command_id: *expected_command,
                timestamp_ms: now_millis(),
                response: BridgeResponse::Err {
                    message: format!("invalid request: {}", err),
                },
            };
            write_bridge_reply(writer, reply).await?;
            return Ok(());
        }
    };

    if command.command_id != *expected_command {
        let reply = BridgeReply {
            command_id: command.command_id,
            timestamp_ms: now_millis(),
            response: BridgeResponse::Err {
                message: format!(
                    "unexpected command id: expected {}, got {}",
                    *expected_command, command.command_id
                ),
            },
        };
        write_bridge_reply(writer, reply).await?;
        return Ok(());
    }

    let next = (*expected_command).wrapping_add(1);
    *expected_command = next;
    let command_id = command.command_id;
    pending.push_back(command);

    let queued = pending.len() + if processing.is_some() { 1 } else { 0 };
    let ack = BridgeReply {
        command_id,
        timestamp_ms: now_millis(),
        response: BridgeResponse::Ack { queued },
    };
    write_bridge_reply(writer, ack).await?;

    drive_queue(pending, processing, tx_ctrl_runtime, writer).await
}

/// Drive the queued commands, ensuring only one runtime request executes at a time.
async fn drive_queue(
    pending: &mut VecDeque<BridgeCommand>,
    processing: &mut Option<ProcessingFuture>,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    writer: &mut BufWriter<OwnedWriteHalf>,
) -> io::Result<()> {
    while processing.is_none() {
        let Some(command) = pending.pop_front() else {
            break;
        };

        let BridgeCommand {
            command_id,
            request,
            ..
        } = command;

        let (reply_tx, reply_rx) = oneshot::channel::<BridgeResponse>();
        if tx_ctrl_runtime
            .send(ControlMsg::Test(TestCommand {
                command_id,
                req: request,
                respond_to: reply_tx,
            }))
            .is_err()
        {
            let reply = BridgeReply {
                command_id,
                timestamp_ms: now_millis(),
                response: BridgeResponse::Err {
                    message: "runtime control channel closed".to_string(),
                },
            };
            write_bridge_reply(writer, reply).await?;
            continue;
        }

        let fut = Box::pin(async move {
            let response = match reply_rx.await {
                Ok(resp) => resp,
                Err(_canceled) => BridgeResponse::Err {
                    message: "runtime dropped bridge response".to_string(),
                },
            };
            (command_id, response)
        });
        *processing = Some(fut);
    }
    Ok(())
}

/// Serialize a bridge reply to the client stream.
async fn write_bridge_reply(
    writer: &mut BufWriter<OwnedWriteHalf>,
    reply: BridgeReply,
) -> io::Result<()> {
    let encoded = serde_json::to_string(&reply).map_err(io::Error::other)?;
    writer.write_all(encoded.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}
