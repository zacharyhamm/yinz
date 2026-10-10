use prost::Message;
use yinz_types::{
    command::{CommandToRun, PaneTarget},
    input::command::RunCommandAction,
};

use crate::{action::ProtobufRunCommandAction, command::ProtobufCommand};

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
                RunCommandAction::try_from(
                    ProtobufRunCommandAction::decode(bytes.as_slice()).unwrap(),
                )
                .unwrap_err()
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
