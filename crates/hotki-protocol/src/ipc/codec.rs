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
    use mac_keycode::Chord;

    use super::*;
    use crate::{
        DisplaysSnapshot, FontWeight, HudRow, HudState, HudStyle, Mode, NotifyConfig, NotifyKind,
        NotifyPos, NotifyTheme, NotifyWindowStyle, Offset, Pos, SelectorItemSnapshot,
        SelectorSnapshot, SelectorStyle, Style, Toggle,
    };

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

    #[test]
    fn roundtrip_all_msg_variants() {
        let style = sample_style();
        let hud = HudState {
            visible: true,
            rows: vec![HudRow {
                chord: Chord::parse("cmd+k").unwrap(),
                desc: "Test".to_string(),
                is_mode: false,
                style: None,
            }],
            depth: 0,
            breadcrumbs: Vec::new(),
            style,
            capture: false,
        };
        let samples: Vec<MsgToUI> = vec![
            MsgToUI::HotkeyTriggered("cmd-h".to_string()),
            MsgToUI::HudUpdate {
                hud: Box::new(hud),
                displays: DisplaysSnapshot::default(),
            },
            MsgToUI::SelectorUpdate(SelectorSnapshot {
                title: "Selector".to_string(),
                placeholder: "Search...".to_string(),
                query: "sa".to_string(),
                items: vec![SelectorItemSnapshot {
                    label: "Safari".to_string(),
                    sublabel: None,
                    label_match_indices: vec![0, 1],
                }],
                selected: 0,
                total_matches: 1,
            }),
            MsgToUI::SelectorHide,
            MsgToUI::Notify {
                kind: NotifyKind::Info,
                title: "Title".to_string(),
                text: "Body".to_string(),
            },
            MsgToUI::ClearNotifications,
            MsgToUI::ShowDetails(Toggle::Toggle),
            MsgToUI::Log {
                level: "info".into(),
                target: "test".into(),
                message: "hello".into(),
            },
            MsgToUI::Heartbeat(123456),
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
