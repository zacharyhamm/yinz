use super::*;
use serde_json::json;
use yinz_types::{data::HttpVerb, internal_ids::TerminalId};

#[test]
fn compact_command_bytes_preserve_exact_data_and_typed_fields() {
    let bytes = vec![0, 128, 255, b'x'];
    let pane = yinz_types::data::PaneId::Terminal(TerminalId::new(5));
    let actions = [
        Action::Write {
            key_with_modifier: None,
            bytes: bytes.clone(),
            is_kitty_keyboard_protocol: false,
        },
        Action::WriteToPaneId {
            bytes: bytes.clone(),
            pane_id: pane,
        },
        Action::PaneNameInput {
            input: bytes.clone(),
        },
        Action::TabNameInput {
            input: bytes.clone(),
        },
        Action::SearchInput {
            input: bytes.clone(),
        },
        Action::RenameTerminalPane {
            pane_id: TerminalId::new(5),
            name: bytes.clone(),
        },
        Action::RenamePluginPane {
            pane_id: yinz_types::internal_ids::PluginId::new(5),
            name: bytes.clone(),
        },
        Action::RenameTab {
            tab_index: yinz_types::internal_ids::TabNumber::new(1),
            name: bytes.clone(),
        },
        Action::RenamePaneByPaneId {
            pane_id: Some(pane),
            name: bytes.clone(),
        },
    ];
    let mut commands = vec![
        PluginCommand::Write(bytes.clone()),
        PluginCommand::WriteToPaneId(bytes.clone(), pane),
        PluginCommand::WebRequest(
            "https://example.invalid".into(),
            HttpVerb::Post,
            Default::default(),
            bytes.clone(),
            Default::default(),
            None,
            None,
        ),
        PluginCommand::FsWrite {
            path: "/data/out".into(),
            contents: bytes.clone(),
            append: false,
            create_dirs: false,
        },
    ];
    for action in actions {
        commands.push(PluginCommand::RunAction(action.clone(), Default::default()));
        commands.push(PluginCommand::RunAction(
            Action::SkipConfirm {
                action: Box::new(action),
            },
            Default::default(),
        ));
    }
    for command in commands {
        let mut wire = serde_json::to_value(&command).unwrap();
        *command_field(&mut wire).unwrap() = crate::binary::encode(&bytes).unwrap();
        assert_eq!(decode_command(wire).unwrap(), command);
        assert_eq!(
            decode_command(serde_json::to_value(&command).unwrap()).unwrap(),
            command
        );
    }
}

#[test]
fn compact_bytes_reject_malformed_markers_and_keep_generic_json_ordinary() {
    for contents in [
        json!({"$yinz-bytes": "!"}),
        json!({"$yinz-bytes": "AA==", "extra": true}),
        json!({"$yinz-bytes": 4}),
    ] {
        assert!(decode_command(json!({"Write": contents})).is_err());
    }
    let mut generic = json!({"DataApply": {"mutations": [{"value": {"$yinz-bytes": "AA=="}}]}});
    let original = generic.clone();
    assert!(command_field(&mut generic).is_none());
    assert_eq!(generic, original);
    let command = decode_command(json!({"FsWrite": {"path":"/data/out", "contents":"λ", "append":false, "create_dirs":false}})).unwrap();
    assert!(
        matches!(command, PluginCommand::FsWrite { contents, .. } if contents == "λ".as_bytes())
    );
}
