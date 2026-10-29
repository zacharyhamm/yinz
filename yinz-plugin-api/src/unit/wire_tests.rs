use std::{collections::BTreeMap, path::PathBuf};

use prost::Message;
use yinz_types::{
    data::{Event, EventType, PluginCommand},
    input::actions::Action,
    internal_ids::PluginId,
};

use crate::{
    action::{KeybindPipePayload, OptionalPayload, ProtobufAction, ProtobufActionName},
    event::{ProtobufEvent, ProtobufEventPayload, ProtobufEventType},
    plugin_command::ProtobufPluginCommand,
};

#[test]
fn pane_responses_reject_unknown_wire_pane_types() {
    use crate::plugin_command::{
        spawn_agent_response, ProtobufOpenPaneInNewTabResponse, ProtobufOpenTerminalResponse,
        ProtobufPaneId, ProtobufSpawnAgentResponse,
    };
    use yinz_types::data::{OpenPaneInNewTabResponse, OpenTerminalResponse, SpawnAgentResponse};

    for pane_type in [-1, 2, i32::MAX] {
        let pane_id = ProtobufPaneId { pane_type, id: 1 };
        let wire = ProtobufOpenTerminalResponse {
            pane_id: Some(pane_id.clone()),
        };
        let decoded =
            ProtobufOpenTerminalResponse::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(
            OpenTerminalResponse::try_from(decoded).unwrap_err(),
            "Failed to convert PaneId"
        );
        assert_eq!(
            OpenPaneInNewTabResponse::try_from(ProtobufOpenPaneInNewTabResponse {
                pane_id: Some(pane_id.clone()),
                tab_id: Some(1),
            })
            .unwrap_err(),
            "Failed to convert PaneId"
        );
        assert_eq!(
            SpawnAgentResponse::try_from(ProtobufSpawnAgentResponse {
                result: Some(spawn_agent_response::Result::PaneId(pane_id)),
                ..Default::default()
            })
            .unwrap_err(),
            "Failed to convert PaneId"
        );
    }
}

#[test]
fn session_name_byte_limit_round_trips_and_is_optional_for_older_hosts() {
    use crate::plugin_ids::ProtobufPluginIds;
    use yinz_types::data::PluginIds;

    for max_session_name_bytes in [None, Some(0), Some(80)] {
        let wire = ProtobufPluginIds {
            max_session_name_bytes,
            ..Default::default()
        };
        let decoded = ProtobufPluginIds::decode(wire.encode_to_vec().as_slice()).unwrap();
        let ids = PluginIds::try_from(decoded).unwrap();
        assert_eq!(
            ids.max_session_name_bytes,
            max_session_name_bytes.map(|n| n as usize)
        );
        assert_eq!(ProtobufPluginIds::try_from(ids).unwrap(), wire);
    }
}

#[test]
fn keybind_pipe_preserves_fields_in_actions_and_rebound_keys() {
    for args in [
        None,
        Some(BTreeMap::new()),
        Some(BTreeMap::from([("cwd".into(), "value".into())])),
    ] {
        let action = Action::KeybindPipe {
            name: Some("message".into()),
            payload: Some("quotes `and`\nnewlines".into()),
            args: args.clone(),
            plugin: Some("yinz:configuration".into()),
            plugin_id: Some(PluginId::new(u32::MAX)),
            configuration: args,
            launch_new: true,
            skip_cache: true,
            floating: Some(false),
            in_place: Some(true),
            cwd: Some(PathBuf::from("relative/é/../folder")),
            pane_title: Some("title".into()),
        };
        let protobuf = ProtobufAction::try_from(action.clone()).unwrap();
        let decoded = ProtobufAction::decode(protobuf.encode_to_vec().as_slice()).unwrap();
        assert_eq!(Action::try_from(decoded).unwrap(), action);

        let command = PluginCommand::RunAction(action.clone(), BTreeMap::new());
        let protobuf = ProtobufPluginCommand::try_from(command.clone()).unwrap();
        let decoded = ProtobufPluginCommand::decode(protobuf.encode_to_vec().as_slice()).unwrap();
        assert_eq!(PluginCommand::try_from(decoded).unwrap(), command);

        let command = PluginCommand::RebindKeys {
            keys_to_rebind: vec![(
                yinz_types::data::InputMode::Normal,
                yinz_types::keys::KeyWithModifier::new(yinz_types::data::BareKey::Char('x')),
                vec![action],
            )],
            keys_to_unbind: vec![],
            write_config_to_disk: false,
        };
        let protobuf = ProtobufPluginCommand::try_from(command.clone()).unwrap();
        let decoded = ProtobufPluginCommand::decode(protobuf.encode_to_vec().as_slice()).unwrap();
        assert_eq!(PluginCommand::try_from(decoded).unwrap(), command);
    }
}

#[test]
fn keybind_pipe_accepts_legacy_empty_actions() {
    let legacy = ProtobufAction {
        name: ProtobufActionName::KeybindPipe as i32,
        optional_payload: None,
    };
    let current = ProtobufAction {
        optional_payload: Some(OptionalPayload::KeybindPipePayload(
            KeybindPipePayload::default(),
        )),
        ..legacy.clone()
    };
    assert_eq!(
        Action::try_from(legacy).unwrap(),
        Action::try_from(current).unwrap()
    );
    assert!(Action::try_from(ProtobufAction {
        name: ProtobufActionName::KeybindPipe as i32,
        optional_payload: Some(OptionalPayload::NotifyPayload("wrong payload".into())),
    })
    .is_err());
}

#[test]
fn change_host_folder_preserves_utf8_paths() {
    for path in ["/tmp/é/文件", "relative/../folder", ""] {
        let command = PluginCommand::ChangeHostFolder(PathBuf::from(path));
        let protobuf = ProtobufPluginCommand::try_from(command.clone()).unwrap();
        assert_eq!(PluginCommand::try_from(protobuf).unwrap(), command);
    }
}

#[cfg(unix)]
#[test]
fn host_folder_and_keybind_pipe_reject_non_utf8_paths() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let path = PathBuf::from(OsString::from_vec(b"/tmp/invalid-\xff".to_vec()));
    assert_eq!(
        ProtobufPluginCommand::try_from(PluginCommand::ChangeHostFolder(path.clone())).unwrap_err(),
        "ChangeHostFolder path is not valid UTF-8"
    );
    let mut action = Action::try_from(ProtobufAction {
        name: ProtobufActionName::KeybindPipe as i32,
        optional_payload: None,
    })
    .unwrap();
    if let Action::KeybindPipe { cwd, .. } = &mut action {
        *cwd = Some(path);
    }
    assert_eq!(
        ProtobufAction::try_from(action).unwrap_err(),
        "KeybindPipe cwd is not valid UTF-8"
    );
}

#[test]
fn picker_and_instance_info_commands_round_trip() {
    use yinz_types::{
        internal_ids::TerminalId,
        pick::{PickCommand, PickKind, PickOptions, PickResult},
    };
    for command in [
        PluginCommand::GetPluginInfo(None),
        PluginCommand::GetPluginInfo(Some(PluginId::new(7))),
        PluginCommand::GetPluginIdsForUrlWithConfiguration {
            url: "reader".into(),
            configuration: BTreeMap::from([("fit".into(), "width".into())]),
        },
        PluginCommand::Pick(PickCommand::Start {
            request_id: "pick:1".into(),
            options: PickOptions {
                kind: PickKind::File,
                roots: Some(vec!["/a\nb".into()]),
                extensions: Some(vec!["PDF".into()]),
                hidden: Some(true),
                recursive: Some(false),
                timeout_ms: Some(50),
            },
        }),
        PluginCommand::Pick(PickCommand::Cancel {
            request_id: "pick:1".into(),
            timeout: true,
        }),
        PluginCommand::Pick(PickCommand::Attach {
            pane_id: TerminalId::new(7),
        }),
        PluginCommand::Pick(PickCommand::Complete {
            result: Ok(Some("/a\nb.pdf".into())),
        }),
        PluginCommand::Pick(PickCommand::Complete { result: Ok(None) }),
        PluginCommand::Pick(PickCommand::Complete {
            result: Err("listing failed".into()),
        }),
    ] {
        let wire = ProtobufPluginCommand::try_from(command.clone()).unwrap();
        let decoded = ProtobufPluginCommand::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(PluginCommand::try_from(decoded).unwrap(), command);
    }
    for result in [
        Ok(Some("/a\nb.pdf".into())),
        Ok(None),
        Err("failure".into()),
    ] {
        let event = Event::PickResult(PickResult {
            request_id: "pick:1".into(),
            result,
        });
        let wire = ProtobufEvent::try_from(event.clone()).unwrap();
        assert_eq!(
            Event::try_from(ProtobufEvent::decode(wire.encode_to_vec().as_slice()).unwrap())
                .unwrap(),
            event
        );
    }
}

#[test]
fn keybind_events_discard_only_malformed_bindings() {
    use crate::event::{ProtobufInputMode, ProtobufInputModeKeybinds, ProtobufKeyBind};
    use crate::key::{ProtobufKey, ProtobufMainKey};
    use yinz_types::data::{InputMode, ModeInfo};
    use yinz_types::keys::{BareKey, KeyWithModifier};

    let key = |c| KeyWithModifier::new(BareKey::Char(c));
    let binding = |c, actions: Vec<Action>| ProtobufKeyBind {
        key: Some(key(c).try_into().unwrap()),
        action: actions.into_iter().map(|a| a.try_into().unwrap()).collect(),
    };
    let mut mixed_actions = binding('b', vec![Action::Quit, Action::NoOp]);
    mixed_actions.action.insert(
        1,
        ProtobufAction {
            name: i32::MAX,
            optional_payload: None,
        },
    );
    let invalid_keys = [
        None,
        Some(ProtobufKey::default()),
        Some(ProtobufKey {
            main_key: Some(ProtobufMainKey::Key(i32::MAX)),
            ..Default::default()
        }),
        Some(ProtobufKey {
            main_key: Some(ProtobufMainKey::Char(-1)),
            ..Default::default()
        }),
    ];
    for invalid_key in invalid_keys {
        let wire_keybinds = vec![
            ProtobufInputModeKeybinds {
                mode: ProtobufInputMode::Normal as i32,
                key_bind: vec![
                    binding('a', vec![Action::Quit]),
                    ProtobufKeyBind {
                        key: invalid_key,
                        action: vec![],
                    },
                    mixed_actions.clone(),
                    binding('c', vec![]),
                ],
            },
            ProtobufInputModeKeybinds {
                mode: i32::MAX,
                key_bind: vec![binding('d', vec![])],
            },
            ProtobufInputModeKeybinds {
                mode: ProtobufInputMode::Locked as i32,
                key_bind: vec![],
            },
            ProtobufInputModeKeybinds {
                mode: ProtobufInputMode::Pane as i32,
                key_bind: vec![binding('e', vec![Action::NoOp])],
            },
        ];
        let expected = vec![
            (
                InputMode::Normal,
                vec![
                    (key('a'), vec![Action::Quit]),
                    (key('b'), vec![Action::Quit, Action::NoOp]),
                    (key('c'), vec![]),
                ],
            ),
            (InputMode::Locked, vec![]),
            (InputMode::Pane, vec![(key('e'), vec![Action::NoOp])]),
        ];
        for event in [
            Event::InitialKeybinds(vec![]),
            Event::ModeUpdate(ModeInfo::default()),
        ] {
            let mut wire = ProtobufEvent::try_from(event).unwrap();
            match wire.payload.as_mut().unwrap() {
                ProtobufEventPayload::InitialKeybindsPayload(payload) => {
                    payload.keybinds = wire_keybinds.clone()
                },
                ProtobufEventPayload::ModeUpdatePayload(payload) => {
                    payload.keybinds = wire_keybinds.clone()
                },
                _ => unreachable!(),
            }
            let wire = ProtobufEvent::decode(wire.encode_to_vec().as_slice()).unwrap();
            let decoded = Event::try_from(wire).unwrap();
            let actual = match &decoded {
                Event::InitialKeybinds(keybinds) => keybinds,
                Event::ModeUpdate(mode_info) => &mode_info.keybinds,
                _ => unreachable!(),
            };
            assert_eq!(actual, &expected);
            let wire = ProtobufEvent::try_from(decoded.clone()).unwrap();
            assert_eq!(
                Event::try_from(ProtobufEvent::decode(wire.encode_to_vec().as_slice()).unwrap())
                    .unwrap(),
                decoded
            );
        }
    }
}

#[test]
fn cell_pixel_geometry_roundtrips_and_rejects_invalid_dimensions() {
    use crate::event::ProtobufCellPixelSize;
    use crate::plugin_command::ProtobufGetCellPixelSizeResponse;
    use yinz_types::pane_size::SizeInPixels;

    for size in [
        None,
        Some(SizeInPixels {
            width: 9,
            height: 18,
        }),
    ] {
        let event = Event::CellPixelSizeChanged(size);
        let protobuf = ProtobufEvent::try_from(event.clone()).unwrap();
        assert_eq!(
            Event::try_from(ProtobufEvent::decode(protobuf.encode_to_vec().as_slice()).unwrap())
                .unwrap(),
            event
        );
        let response = ProtobufGetCellPixelSizeResponse {
            size: size.map(|size| size.try_into().unwrap()),
        };
        let decoded =
            ProtobufGetCellPixelSizeResponse::decode(response.encode_to_vec().as_slice()).unwrap();
        assert_eq!(
            decoded
                .size
                .map(|size| SizeInPixels::try_from(size).unwrap()),
            size
        );
    }
    for (width, height) in [(0, 18), (9, 0)] {
        assert!(SizeInPixels::try_from(ProtobufCellPixelSize { width, height }).is_err());
    }
    assert_eq!(
        EventType::try_from(ProtobufEventType::CellPixelSizeChanged).unwrap(),
        EventType::CellPixelSizeChanged
    );
    assert_eq!(
        ProtobufEventType::try_from(EventType::CellPixelSizeChanged).unwrap(),
        ProtobufEventType::CellPixelSizeChanged
    );
    let command = PluginCommand::GetCellPixelSize;
    assert_eq!(
        PluginCommand::try_from(ProtobufPluginCommand::try_from(command.clone()).unwrap()).unwrap(),
        command
    );
}
