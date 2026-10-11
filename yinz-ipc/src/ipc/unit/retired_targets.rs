use super::*;
use crate::client_server_contract::client_server_contract as proto;
use prost::Message;
use yinz_types::input::{
    actions::Action,
    command::RunCommandAction,
    layout::{FloatingPaneLayout, Run, TiledPaneLayout},
};

#[test]
fn retired_targets_are_rejected_in_every_command_decoder() {
    for supported in [
        vec![],
        vec![0x08, 1],
        vec![0x12, 1, b'h'],
        vec![0x22, 2, b'/', b'r'],
    ] {
        for retired in [vec![0x1a, 0], vec![0x1a, 3, 0x0a, 1, b'v']] {
            for retired_first in [false, true] {
                let bytes = if retired_first {
                    [retired.clone(), supported.clone()].concat()
                } else {
                    [supported.clone(), retired.clone()].concat()
                };
                let target = proto::PaneTarget::decode(bytes.as_slice()).unwrap();
                assert_eq!(
                    pane_target_from_protobuf(Some(target.clone()))
                        .unwrap_err()
                        .to_string(),
                    "Firecracker execution is no longer supported."
                );
                let command = proto::RunCommandAction {
                    command: "claude".into(),
                    target: Some(target),
                    ..Default::default()
                };
                assert!(run_command_from_protobuf(command.clone()).is_none());
                assert!(RunCommandAction::try_from(command.clone()).is_err());
                assert!(Action::try_from(proto::Action {
                    action_type: Some(proto::action::ActionType::Run(proto::RunAction {
                        command: Some(command.clone()),
                        ..Default::default()
                    })),
                })
                .is_err());
                let run = proto::Run {
                    run_type: Some(proto::run::RunType::Command(command.clone())),
                };
                assert!(Run::try_from(run.clone()).is_err());
                assert!(TiledPaneLayout::try_from(proto::TiledPaneLayout {
                    run: Some(run.clone()),
                    ..Default::default()
                })
                .is_err());
                assert!(FloatingPaneLayout::try_from(proto::FloatingPaneLayout {
                    run: Some(run),
                    ..Default::default()
                })
                .is_err());
                assert!(held_from_protobuf(proto::HeldTerminal {
                    command: Some(command.clone()),
                    ..Default::default()
                })
                .is_err());
                assert!(remote_handoff_from_protobuf(proto::RemotePaneHandoff {
                    cmd: Some(command),
                    ..Default::default()
                })
                .is_none());
            }
        }
    }
}

#[test]
fn supported_targets_round_trip_without_retired_markers() {
    use yinz_types::command::PaneTarget;
    for target in [
        PaneTarget::Local,
        PaneTarget::ssh("build-box").unwrap(),
        PaneTarget::bubblewrap(std::path::Path::new("/repo")).unwrap(),
    ] {
        let wire = pane_target_to_protobuf(&target);
        assert!(wire.firecracker.is_none());
        let decoded = proto::PaneTarget::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(pane_target_from_protobuf(Some(decoded)).unwrap(), target);
    }
    assert_eq!(pane_target_from_protobuf(None).unwrap(), PaneTarget::Local);
    assert_eq!(
        pane_target_from_protobuf(Some(proto::PaneTarget::default())).unwrap(),
        PaneTarget::Local
    );
}
