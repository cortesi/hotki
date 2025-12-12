use hotki_protocol::NotifyKind;
use hotki_server::smoketest_bridge::{BridgeCommandId, BridgeRequest, BridgeResponse};
use tokio::sync::oneshot;

/// Control messages routed to the runtime event loop.
#[derive(Debug)]
pub enum ControlMsg {
    /// Reload from disk using `config_path`.
    Reload,
    /// Gracefully shut down the UI and exit the process.
    Shutdown,
    /// Request a theme switch by name (handled on the live Config).
    SwitchTheme(String),
    /// Open the in-app permissions help view.
    OpenPermissionsHelp,
    /// Forward a user-facing notice into the app UI.
    Notice {
        /// Notice severity kind.
        kind: NotifyKind,
        /// Notice title text.
        title: String,
        /// Notice body text.
        text: String,
    },
    /// Internal test bridge command (smoketest harness).
    Test(TestCommand),
}

/// Request/response pair used to service smoketest bridge commands.
#[derive(Debug)]
pub struct TestCommand {
    /// Identifier for the command being serviced.
    pub(crate) command_id: BridgeCommandId,
    /// The bridge request submitted by the smoketest harness.
    pub(crate) req: BridgeRequest,
    /// Channel used to deliver the bridge response back to the harness.
    pub(crate) respond_to: oneshot::Sender<BridgeResponse>,
}
