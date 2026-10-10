use super::*;
use crate::client_server_contract::client_server_contract as proto;
use yinz_types::input::{
    command::{RunCommand, RunCommandAction},
    layout::{FloatingPaneLayout, Run, TiledPaneLayout},
};

#[test]
fn bubblewrap_roots_are_validated_in_every_command_decoder() {
    for root in ["/repo", "", "relative/repo"] {
        let valid = root == "/repo";
        let command = proto::RunCommandAction {
            command: "claude".into(),
            target: Some(proto::PaneTarget {
                kind: Some(proto::pane_target::Kind::BubblewrapRoot(root.into())),
            }),
            ..Default::default()
        };
        let decoded = run_command_from_protobuf(command.clone());
        assert_eq!(decoded.is_some(), valid);
        if let Some(decoded) = decoded {
            assert_eq!(
                decoded.target.bubblewrap_root(),
                Some(std::path::Path::new(root))
            );
            assert_eq!(
                run_command_from_protobuf(run_command_to_protobuf(&decoded)),
                Some(decoded)
            );
        }
        assert_eq!(RunCommandAction::try_from(command.clone()).is_ok(), valid);
        let action = proto::Action {
            action_type: Some(proto::action::ActionType::Run(proto::RunAction {
                command: Some(command.clone()),
                ..Default::default()
            })),
        };
        assert_eq!(
            yinz_types::input::actions::Action::try_from(action).is_ok(),
            valid
        );
        let run = proto::Run {
            run_type: Some(proto::run::RunType::Command(command.clone())),
        };
        assert_eq!(Run::try_from(run.clone()).is_ok(), valid);
        assert_eq!(
            TiledPaneLayout::try_from(proto::TiledPaneLayout {
                run: Some(run.clone()),
                ..Default::default()
            })
            .is_ok(),
            valid
        );
        assert_eq!(
            FloatingPaneLayout::try_from(proto::FloatingPaneLayout {
                run: Some(run),
                ..Default::default()
            })
            .is_ok(),
            valid
        );
        assert_eq!(
            held_from_protobuf(proto::HeldTerminal {
                command: Some(command.clone()),
                ..Default::default()
            })
            .is_ok(),
            valid
        );
        assert_eq!(
            remote_handoff_from_protobuf(proto::RemotePaneHandoff {
                cmd: Some(command),
                ..Default::default()
            })
            .is_some(),
            valid
        );
    }
    let local = RunCommand::default();
    assert_eq!(
        run_command_from_protobuf(run_command_to_protobuf(&local)),
        Some(local)
    );
}
