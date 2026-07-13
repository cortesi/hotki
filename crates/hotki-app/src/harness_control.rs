//! App-owned control socket used by process-owning local harnesses.

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    thread::{self, JoinHandle},
};

use hotki_protocol::NotifyKind;
use tokio::sync::mpsc::UnboundedSender;

use crate::runtime::ControlMsg;

/// UI presentation state that a local harness can wait to observe after painting.
#[derive(Debug, PartialEq, Eq)]
pub enum PresentationExpectation {
    /// The HUD is visible.
    Hud,
    /// The selector is visible with the exact query.
    Selector(String),
    /// A notification of this kind is visible.
    Notification(NotifyKind),
}

/// One pending request for a painted UI state.
pub struct PresentationRequest {
    /// State that must be rendered before acknowledgement.
    pub(crate) expectation: PresentationExpectation,
    /// Completion channel returned to the socket-serving thread.
    pub(crate) rendered: SyncSender<()>,
}

/// Spawn a listener that accepts presentation barriers and app-local shutdown.
pub fn spawn(
    path: &Path,
    tx_ctrl: UnboundedSender<ControlMsg>,
) -> io::Result<(JoinHandle<()>, Receiver<PresentationRequest>)> {
    prepare_socket_path(path)?;
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let path = path.to_path_buf();
    let (tx_present, rx_present) = mpsc::channel();
    let task = thread::Builder::new()
        .name("hotki-harness-control".to_string())
        .spawn(move || serve(&listener, &path, &tx_ctrl, &tx_present))?;
    Ok((task, rx_present))
}

/// Refuse to replace anything except a stale Unix socket.
fn prepare_socket_path(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("harness control path is not a socket: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Serve requests until shutdown is delivered or the app drops its presentation receiver.
fn serve(
    listener: &UnixListener,
    path: &Path,
    tx_ctrl: &UnboundedSender<ControlMsg>,
    tx_present: &Sender<PresentationRequest>,
) {
    loop {
        let result = listener
            .accept()
            .and_then(|(stream, _)| handle_request(stream, tx_ctrl, tx_present));
        match result {
            Ok(true) => break,
            Ok(false) => {}
            Err(error) => tracing::warn!(?error, "harness control request failed"),
        }
    }
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(?error, path = %path.display(), "failed to remove harness control socket");
    }
}

/// Handle one line-oriented command and return whether the listener should stop.
fn handle_request(
    mut stream: UnixStream,
    tx_ctrl: &UnboundedSender<ControlMsg>,
    tx_present: &Sender<PresentationRequest>,
) -> io::Result<bool> {
    let mut command = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut command)?;
    let command = command.trim_end();
    if command == "shutdown" {
        tx_ctrl
            .send(ControlMsg::Shutdown)
            .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))?;
        stream.write_all(b"ok\n")?;
        return Ok(true);
    }

    let Some(expectation) = parse_presentation(command) else {
        stream.write_all(b"error: unknown command\n")?;
        return Ok(false);
    };
    let (tx_rendered, rx_rendered) = mpsc::sync_channel(0);
    tx_present
        .send(PresentationRequest {
            expectation,
            rendered: tx_rendered,
        })
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))?;
    thread::Builder::new()
        .name("hotki-harness-presentation".to_string())
        .spawn(move || {
            let result = rx_rendered
                .recv()
                .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
                .and_then(|()| stream.write_all(b"ok\n"));
            if let Err(error) = result {
                tracing::debug!(?error, "harness presentation request ended");
            }
        })?;
    Ok(false)
}

/// Parse a presentation barrier command from the private harness protocol.
fn parse_presentation(command: &str) -> Option<PresentationExpectation> {
    match command {
        "present hud" => Some(PresentationExpectation::Hud),
        "present notification info" => {
            Some(PresentationExpectation::Notification(NotifyKind::Info))
        }
        "present notification warn" => {
            Some(PresentationExpectation::Notification(NotifyKind::Warn))
        }
        "present notification error" => {
            Some(PresentationExpectation::Notification(NotifyKind::Error))
        }
        "present notification success" => {
            Some(PresentationExpectation::Notification(NotifyKind::Success))
        }
        _ => command
            .strip_prefix("present selector ")
            .map(|query| PresentationExpectation::Selector(query.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn shutdown_command_reaches_app_control_channel() {
        let nonce = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let path =
            PathBuf::from("tmp").join(format!("harness-control-{}-{nonce}.sock", process::id()));
        let (tx, mut rx) = unbounded_channel();
        let (task, _requests) = spawn(&path, tx).expect("spawn harness control");
        let mut stream = UnixStream::connect(&path).expect("connect harness control");

        stream.write_all(b"shutdown\n").expect("write command");
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .expect("read response");

        assert_eq!(response, "ok\n");
        assert!(matches!(rx.try_recv(), Ok(ControlMsg::Shutdown)));
        task.join().expect("join harness control");
        assert!(!path.exists());
    }

    #[test]
    fn presentation_command_waits_for_ui_acknowledgement() {
        let nonce = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("tmp").join(format!(
            "harness-presentation-{}-{nonce}.sock",
            process::id()
        ));
        let (tx, mut rx_ctrl) = unbounded_channel();
        let (task, requests) = spawn(&path, tx).expect("spawn harness control");
        let client_path = path.clone();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(client_path).expect("connect presentation");
            stream
                .write_all(b"present selector cal\n")
                .expect("write presentation command");
            let mut response = String::new();
            BufReader::new(stream)
                .read_line(&mut response)
                .expect("read presentation response");
            response
        });

        let request = requests.recv().expect("receive presentation request");
        assert_eq!(
            request.expectation,
            PresentationExpectation::Selector("cal".to_string())
        );
        request.rendered.send(()).expect("acknowledge rendered UI");
        assert_eq!(client.join().expect("join presentation client"), "ok\n");

        let mut stream = UnixStream::connect(&path).expect("connect shutdown");
        stream.write_all(b"shutdown\n").expect("write shutdown");
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .expect("read shutdown response");
        assert_eq!(response, "ok\n");
        assert!(matches!(rx_ctrl.try_recv(), Ok(ControlMsg::Shutdown)));
        task.join().expect("join harness control");
        assert!(!path.exists());
    }

    #[test]
    fn pending_presentation_does_not_block_shutdown() {
        let nonce = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let path =
            PathBuf::from("tmp").join(format!("harness-pending-{}-{nonce}.sock", process::id()));
        let (tx, mut rx_ctrl) = unbounded_channel();
        let (task, requests) = spawn(&path, tx).expect("spawn harness control");
        let mut presentation = UnixStream::connect(&path).expect("connect presentation");
        presentation
            .write_all(b"present hud\n")
            .expect("write presentation command");
        let pending = requests.recv().expect("receive pending presentation");

        let mut shutdown = UnixStream::connect(&path).expect("connect shutdown");
        shutdown.write_all(b"shutdown\n").expect("write shutdown");
        let mut response = String::new();
        BufReader::new(shutdown)
            .read_line(&mut response)
            .expect("read shutdown response");

        assert_eq!(response, "ok\n");
        assert!(matches!(rx_ctrl.try_recv(), Ok(ControlMsg::Shutdown)));
        drop(pending);
        task.join().expect("join harness control");
        assert!(!path.exists());
    }
}
