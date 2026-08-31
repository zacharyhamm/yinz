pub use super::generated_api::api::command::Command as ProtobufCommand;
use crate::data::CommandToRun;

use std::convert::TryFrom;
use std::path::PathBuf;

impl TryFrom<ProtobufCommand> for CommandToRun {
    type Error = &'static str;
    fn try_from(protobuf_command: ProtobufCommand) -> Result<Self, &'static str> {
        let path = PathBuf::from(protobuf_command.path);
        let args = protobuf_command.args;
        let cwd = protobuf_command.cwd.map(PathBuf::from);
        let remote_host = protobuf_command.remote_host;
        let remote_default_shell = protobuf_command.remote_default_shell;
        let firecracker_vm = protobuf_command.firecracker_vm;
        Ok(CommandToRun {
            path,
            args,
            cwd,
            remote_host,
            remote_default_shell,
            firecracker_vm,
        })
    }
}

impl TryFrom<CommandToRun> for ProtobufCommand {
    type Error = &'static str;
    fn try_from(command_to_run: CommandToRun) -> Result<Self, &'static str> {
        Ok(ProtobufCommand {
            path: command_to_run.path.display().to_string(),
            args: command_to_run.args,
            cwd: command_to_run.cwd.map(|c| c.display().to_string()),
            remote_host: command_to_run.remote_host,
            remote_default_shell: command_to_run.remote_default_shell,
            firecracker_vm: command_to_run.firecracker_vm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin's `remote_host` has to survive the protobuf hop — this is the wire
    /// format a plugin's `OpenCommandPane` request travels over to reach
    /// `zellij_exports.rs`, which forwards it into `RunCommandAction`.
    #[test]
    fn remote_host_survives_the_round_trip_through_the_protobuf_command() {
        let command_to_run = CommandToRun {
            path: PathBuf::from("/usr/bin/htop"),
            args: vec![],
            cwd: None,
            remote_host: Some("user@example.com".to_owned()),
            ..Default::default()
        };
        let protobuf_command = ProtobufCommand::try_from(command_to_run.clone()).unwrap();
        assert_eq!(
            protobuf_command.remote_host,
            Some("user@example.com".to_owned())
        );
        let round_tripped = CommandToRun::try_from(protobuf_command).unwrap();
        assert_eq!(round_tripped.remote_host, command_to_run.remote_host);
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
            remote_host: Some("build-box".to_owned()),
            remote_default_shell: true,
            firecracker_vm: None,
        };
        let protobuf_command = ProtobufCommand::try_from(command_to_run.clone()).unwrap();
        assert!(protobuf_command.remote_default_shell);
        let round_tripped = CommandToRun::try_from(protobuf_command).unwrap();
        assert!(round_tripped.remote_default_shell);
        assert_eq!(round_tripped.path, PathBuf::new());
        assert_eq!(round_tripped.remote_host, command_to_run.remote_host);
    }
}
