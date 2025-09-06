use crate::MsgToUI;
use mrpc::Value;
use thiserror::Error;

/// Errors from encoding/decoding UI messages.
#[derive(Debug, Error)]
pub enum Error {
    #[error("expected binary message payload, got {0:?}")]
    InvalidValueType(Value),
    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),
}

/// Encode a `MsgToUI` message into an `mrpc::Value` as a binary payload.
pub fn msg_to_value(msg: &MsgToUI) -> Value {
    let bytes = rmp_serde::to_vec_named(msg).unwrap_or_default();
    Value::Binary(bytes)
}

/// Decode an `mrpc::Value` (binary) back into a `MsgToUI`.
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
        ];

        for msg in samples {
            let val = msg_to_value(&msg);
            let back = value_to_msg(val).expect("decode");
            assert_eq!(format!("{:?}", msg), format!("{:?}", back));
        }
    }
}
