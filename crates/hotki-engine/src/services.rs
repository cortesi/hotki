use std::sync::Arc;

use hotki_world::WorldView;
use mac_winops::ops::WinOps;

use crate::{notification::NotificationDispatcher, relay::RelayHandler, repeater::Repeater};

/// Groups long‑lived engine services to reduce top‑level `Engine` fields
/// and make dependencies explicit at construction sites.
#[derive(Clone)]
pub struct Services {
    pub relay: RelayHandler,
    pub notifier: NotificationDispatcher,
    pub repeater: Repeater,
    pub winops: Arc<dyn WinOps>,
    pub world: Arc<dyn WorldView>,
}
