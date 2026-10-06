use std::{collections::BTreeMap, path::PathBuf};

use prost::Message;
use yinz_types::{
    data::{ClientId, Event, EventType, PluginCommand},
    input::actions::Action,
    internal_ids::PluginId,
};

use crate::{
    action::{KeybindPipePayload, OptionalPayload, ProtobufAction, ProtobufActionName},
    event::{ProtobufEvent, ProtobufEventPayload, ProtobufEventType},
    plugin_command::ProtobufPluginCommand,
};

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
            plugin: Some("yinz:agents".into()),
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
fn client_grid_reset_roundtrips_and_rejects_malformed_payloads() {
    for id in [0, 7, u16::MAX] {
        let event = Event::ClientGridReset(ClientId::new(id));
        let protobuf = ProtobufEvent::try_from(event.clone()).unwrap();
        let decoded = ProtobufEvent::decode(protobuf.encode_to_vec().as_slice()).unwrap();
        assert_eq!(Event::try_from(decoded).unwrap(), event);
    }
    assert_eq!(
        EventType::try_from(ProtobufEventType::ClientGridReset).unwrap(),
        EventType::ClientGridReset
    );
    assert_eq!(
        ProtobufEventType::try_from(EventType::ClientGridReset).unwrap(),
        ProtobufEventType::ClientGridReset
    );
    for payload in [
        None,
        Some(ProtobufEventPayload::VisiblePayload(true)),
        Some(ProtobufEventPayload::ClientGridResetPayload(65536)),
    ] {
        assert!(Event::try_from(ProtobufEvent {
            name: ProtobufEventType::ClientGridReset as i32,
            payload
        })
        .is_err());
    }
}
