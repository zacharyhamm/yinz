use prost::Message;
use yinz_types::{
    command::{CommandToRun, PaneTarget},
    data::NewPanePlacement,
    input::{actions::Action, command::RunCommandAction},
};

use crate::{
    action::{
        NewBlockingPanePayload, OptionalPayload, ProtobufAction, ProtobufActionName,
        ProtobufRunCommandAction,
    },
    command::ProtobufCommand,
};

#[test]
fn retired_wire_targets_are_rejected_before_target_precedence() {
    // Original wire keys: Command target=9, RunCommandAction target=12;
    // PaneTarget local=1, ssh_host=2, retired Firecracker message=3.
    for (target_key, flat_keys) in [(0x4a, [0x32, 0x3a, 0x42]), (0x62, [0x4a, 0x52, 0x5a])] {
        let reject = |bytes: Vec<u8>| {
            let error = if target_key == 0x4a {
                CommandToRun::try_from(ProtobufCommand::decode(bytes.as_slice()).unwrap())
                    .unwrap_err()
            } else {
                let command = ProtobufRunCommandAction::decode(bytes.as_slice()).unwrap();
                let wire = ProtobufAction {
                    name: ProtobufActionName::NewBlockingPane as i32,
                    optional_payload: Some(OptionalPayload::NewBlockingPanePayload(
                        NewBlockingPanePayload {
                            placement: Some(NewPanePlacement::default().try_into().unwrap()),
                            command: Some(command.clone()),
                            ..Default::default()
                        },
                    )),
                };
                let decoded = ProtobufAction::decode(wire.encode_to_vec().as_slice()).unwrap();
                assert_eq!(
                    Action::try_from(decoded).unwrap_err(),
                    "Firecracker execution is no longer supported.",
                    "{bytes:?}"
                );
                RunCommandAction::try_from(command).unwrap_err()
            };
            assert_eq!(
                error, "Firecracker execution is no longer supported.",
                "{bytes:?}"
            );
        };
        for explicit in [vec![], vec![0x08, 1], vec![0x12, 1, b'h']] {
            for retired in [vec![0x1a, 0], vec![0x1a, 3, 0x0a, 1, b'v']] {
                for retired_first in [false, true] {
                    let target = if retired_first {
                        [retired.clone(), explicit.clone()].concat()
                    } else {
                        [explicit.clone(), retired.clone()].concat()
                    };
                    let mut bytes = vec![target_key, target.len() as u8];
                    bytes.extend(target);
                    reject(bytes);
                }
            }
            for key in flat_keys {
                for value in [vec![], vec![b'v']] {
                    let mut bytes = vec![key, value.len() as u8];
                    bytes.extend(value);
                    if !explicit.is_empty() {
                        bytes.extend([target_key, explicit.len() as u8]);
                        bytes.extend(&explicit);
                    }
                    reject(bytes);
                }
            }
        }
    }
}

#[test]
fn blocking_panes_preserve_supported_and_absent_commands() {
    for target in [
        None,
        Some(PaneTarget::Local),
        Some(PaneTarget::ssh("build-box").unwrap()),
    ] {
        let action = Action::NewBlockingPane {
            placement: NewPanePlacement::default(),
            pane_name: None,
            command: target.map(|target| RunCommandAction {
                command: "/usr/bin/true".into(),
                target,
                ..Default::default()
            }),
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        };
        let wire = ProtobufAction::try_from(action.clone()).unwrap();
        let decoded = ProtobufAction::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(Action::try_from(decoded).unwrap(), action);
    }
}

#[test]
fn supported_targets_round_trip_without_retired_markers() {
    for target in [PaneTarget::Local, PaneTarget::ssh("build-box").unwrap()] {
        let command = CommandToRun {
            target: target.clone(),
            ..Default::default()
        };
        let wire = ProtobufCommand::try_from(command.clone()).unwrap();
        assert!(wire.firecracker_vm.is_none());
        assert!(wire.firecracker_image.is_none());
        assert!(wire.firecracker_worktree.is_none());
        assert!(wire.target.as_ref().unwrap().firecracker.is_none());
        let decoded = ProtobufCommand::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(CommandToRun::try_from(decoded).unwrap(), command);
        let legacy = ProtobufCommand {
            target: None,
            ..wire
        };
        assert_eq!(CommandToRun::try_from(legacy).unwrap(), command);

        let action = RunCommandAction {
            target,
            ..Default::default()
        };
        let wire = ProtobufRunCommandAction::try_from(action.clone()).unwrap();
        assert!(wire.firecracker_vm.is_none());
        assert!(wire.firecracker_image.is_none());
        assert!(wire.firecracker_worktree.is_none());
        assert!(wire.target.as_ref().unwrap().firecracker.is_none());
        let decoded = ProtobufRunCommandAction::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(RunCommandAction::try_from(decoded).unwrap(), action);
    }
}

#[test]
fn retired_agent_spawn_fields_are_rejected() {
    use crate::generated_api::api::plugin_command::{
        plugin_command::Payload, AgentSpawnSpec, CommandName, PluginCommand, SpawnAgentPayload,
    };
    use yinz_types::data::PluginCommand as NativeCommand;

    // Original AgentSpawnSpec string fields: firecracker=14, firecracker_image=15.
    for key in [0x72, 0x7a] {
        for value in [vec![], vec![b'v']] {
            for guards in [vec![], vec![0x32, 1, b'b', 0x40, 1], vec![0x90, 1, 1]] {
                let bytes = [vec![key, value.len() as u8], value.clone(), guards].concat();
                let spec = AgentSpawnSpec::decode(bytes.as_slice()).unwrap();
                assert_eq!(
                    yinz_types::agent::AgentSpawnSpec::try_from(spec.clone()).unwrap_err(),
                    "Firecracker execution is no longer supported."
                );
                let wire = PluginCommand {
                    name: CommandName::SpawnAgent as i32,
                    payload: Some(Payload::SpawnAgentPayload(SpawnAgentPayload {
                        spec: Some(spec),
                        ..Default::default()
                    })),
                };
                let decoded = PluginCommand::decode(wire.encode_to_vec().as_slice()).unwrap();
                assert_eq!(
                    NativeCommand::try_from(decoded).unwrap_err(),
                    "Firecracker execution is no longer supported."
                );
            }
        }
    }
}

#[test]
fn supported_agent_specs_round_trip_without_retired_markers() {
    use crate::generated_api::api::plugin_command::AgentSpawnSpec as WireSpec;
    use yinz_types::agent::AgentSpawnSpec;

    for spec in [
        AgentSpawnSpec::default(),
        AgentSpawnSpec {
            branch: Some("feature".into()),
            worktree: Some(true),
            ..Default::default()
        },
        AgentSpawnSpec {
            allow_outside_worktree: Some(true),
            bubblewrap: true,
            ..Default::default()
        },
    ] {
        let wire = WireSpec::from(spec.clone());
        assert!(wire.firecracker.is_none());
        assert!(wire.firecracker_image.is_none());
        let decoded = WireSpec::decode(wire.encode_to_vec().as_slice()).unwrap();
        assert_eq!(AgentSpawnSpec::try_from(decoded).unwrap(), spec);
    }
}
