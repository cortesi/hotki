use mrpc::Value;
use thiserror::Error;

use crate::MsgToUI;

/// Errors from encoding/decoding UI messages.
#[derive(Debug, Error)]
pub enum Error {
    /// The provided value was not a binary payload.
    #[error("expected binary message payload, got {0:?}")]
    InvalidValueType(Value),
    /// Deserialization via rmp_serde failed.
    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),
    /// Serialization via rmp_serde failed.
    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),
}

/// Encode a `MsgToUI` message into an `mrpc::Value` as a binary payload.
pub fn msg_to_value(msg: &MsgToUI) -> Result<Value, Error> {
    let bytes = rmp_serde::to_vec_named(msg)?;
    Ok(Value::Binary(bytes))
}

/// Decode an `mrpc::Value` (binary) back into a `MsgToUI`.
///
/// # Errors
/// Returns an error if the binary payload cannot be decoded into a valid
/// `MsgToUI` message using `rmp_serde`.
pub fn value_to_msg(value: Value) -> Result<MsgToUI, Error> {
    match value {
        Value::Binary(bytes) => {
            let msg: MsgToUI = rmp_serde::from_slice(&bytes)?;
            Ok(msg)
        }
        other => Err(Error::InvalidValueType(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cursor, NotifyKind, Toggle};

    #[test]
    fn roundtrip_all_msg_variants() {
        let samples: Vec<MsgToUI> = vec![
            MsgToUI::HotkeyTriggered("cmd-h".to_string()),
            MsgToUI::HudUpdate {
                cursor: Cursor::new(vec![1, 2], false),
            },
            MsgToUI::Notify {
                kind: NotifyKind::Info,
                title: "Title".to_string(),
                text: "Body".to_string(),
            },
            MsgToUI::ReloadConfig,
            MsgToUI::ClearNotifications,
            MsgToUI::ShowDetails(Toggle::Toggle),
            MsgToUI::ThemeNext,
            MsgToUI::ThemePrev,
            MsgToUI::ThemeSet("night".into()),
            MsgToUI::UserStyle(Toggle::Toggle),
            MsgToUI::UserStyle(Toggle::On),
            MsgToUI::Log {
                level: "info".into(),
                target: "test".into(),
                message: "hello".into(),
            },
            MsgToUI::Heartbeat(123456),
            MsgToUI::World(crate::WorldStreamMsg::ResyncRecommended),
            MsgToUI::World(crate::WorldStreamMsg::FocusChanged(Some(crate::App {
                app: "X".into(),
                title: "Y".into(),
                pid: 1,
            }))),
        ];

        for msg in samples {
            let val = msg_to_value(&msg).expect("encode");
            let back = value_to_msg(val).expect("decode");
            assert_eq!(format!("{:?}", msg), format!("{:?}", back));
        }
    }
}
