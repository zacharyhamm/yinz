use super::*;

/// A plugin's target has to survive the protobuf hop — this is the wire
/// format a plugin's `OpenCommandPane` request travels over to reach
/// `yinz_exports.rs`, which forwards it into `RunCommandAction`.
#[test]
fn remote_host_survives_the_round_trip_through_the_protobuf_command() {
    let command_to_run = CommandToRun {
        path: PathBuf::from("/usr/bin/htop"),
        target: PaneTarget::Ssh {
            host: "user@example.com".to_owned(),
        },
        ..Default::default()
    };
    let protobuf_command = ProtobufCommand::try_from(command_to_run.clone()).unwrap();
    assert_eq!(
        protobuf_command.remote_host,
        Some("user@example.com".to_owned())
    );
    let round_tripped = CommandToRun::try_from(protobuf_command).unwrap();
    assert_eq!(round_tripped.target, command_to_run.target);
}

#[test]
fn a_local_command_carries_no_remote_host() {
    let command_to_run = CommandToRun::new("/bin/sh");
    let protobuf_command = ProtobufCommand::try_from(command_to_run).unwrap();
    assert_eq!(protobuf_command.remote_host, None);
    assert!(!protobuf_command.remote_default_shell);
}

/// A pane that wants the far machine's login shell crosses this boundary
/// with no command at all — the flag is the entire request, and losing it
/// would turn the pane into an attempt to exec an empty path.
#[test]
fn a_remote_default_shell_survives_the_round_trip_with_no_command() {
    let command_to_run = CommandToRun {
        path: PathBuf::new(),
        args: vec![],
        cwd: None,
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        remote_default_shell: true,
    };
    let protobuf_command = ProtobufCommand::try_from(command_to_run.clone()).unwrap();
    assert!(protobuf_command.remote_default_shell);
    let round_tripped = CommandToRun::try_from(protobuf_command).unwrap();
    assert!(round_tripped.remote_default_shell);
    assert_eq!(round_tripped.path, PathBuf::new());
    assert_eq!(round_tripped.target, command_to_run.target);
}
#[test]
fn a_flat_ssh_host_describes_the_target() {
    let command = ProtobufCommand {
        path: "/usr/bin/htop".to_owned(),
        remote_host: Some("build-box".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        CommandToRun::try_from(command).unwrap().target,
        PaneTarget::ssh("build-box").unwrap()
    );
}
