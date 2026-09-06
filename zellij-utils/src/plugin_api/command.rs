pub use super::generated_api::api::command::{
    pane_target::Kind as ProtobufPaneTargetKind, Command as ProtobufCommand,
    FirecrackerTarget as ProtobufFirecrackerTarget, PaneTarget as ProtobufPaneTarget,
};
use zellij_types::command::{CommandToRun, PaneTarget};

use std::convert::TryFrom;
use std::path::{Path, PathBuf};

impl From<&PaneTarget> for ProtobufPaneTarget {
    fn from(target: &PaneTarget) -> Self {
        let kind = match target {
            PaneTarget::Local => ProtobufPaneTargetKind::Local(true),
            PaneTarget::Ssh { host } => ProtobufPaneTargetKind::SshHost(host.clone()),
            PaneTarget::Firecracker {
                vm,
                image,
                worktree,
            } => ProtobufPaneTargetKind::Firecracker(ProtobufFirecrackerTarget {
                vm: vm.clone(),
                image: image.clone(),
                worktree: worktree.as_ref().map(|path| path.display().to_string()),
            }),
        };
        ProtobufPaneTarget { kind: Some(kind) }
    }
}

impl TryFrom<ProtobufPaneTarget> for PaneTarget {
    type Error = &'static str;
    fn try_from(target: ProtobufPaneTarget) -> Result<Self, &'static str> {
        Ok(match target.kind {
            None | Some(ProtobufPaneTargetKind::Local(_)) => PaneTarget::Local,
            Some(ProtobufPaneTargetKind::SshHost(host)) => PaneTarget::Ssh { host },
            Some(ProtobufPaneTargetKind::Firecracker(firecracker)) => PaneTarget::Firecracker {
                vm: firecracker.vm,
                image: firecracker.image,
                worktree: firecracker.worktree.map(PathBuf::from),
            },
        })
    }
}

/// The flat spelling of a target every pre-`PaneTarget` message carries, so
/// a plugin built against the older API still sees the fields it knows.
#[derive(Default)]
pub struct FlatPaneTarget {
    pub remote_host: Option<String>,
    pub firecracker_vm: Option<String>,
    pub firecracker_image: Option<String>,
    pub firecracker_worktree: Option<String>,
}

impl From<&PaneTarget> for FlatPaneTarget {
    fn from(target: &PaneTarget) -> Self {
        match target {
            PaneTarget::Local => FlatPaneTarget::default(),
            PaneTarget::Ssh { host } => FlatPaneTarget {
                remote_host: Some(host.clone()),
                ..Default::default()
            },
            PaneTarget::Firecracker {
                vm,
                image,
                worktree,
            } => FlatPaneTarget {
                remote_host: None,
                firecracker_vm: Some(vm.clone()),
                firecracker_image: image.clone(),
                firecracker_worktree: worktree.as_ref().map(|path| path.display().to_string()),
            },
        }
    }
}

/// Read a target from a message that carries both spellings: the structured
/// `target` wins when it is set, otherwise the flat fields a plugin built
/// before `PaneTarget` still fills.
pub fn pane_target_from_wire(
    target: Option<ProtobufPaneTarget>,
    flat: FlatPaneTarget,
) -> Result<PaneTarget, &'static str> {
    match target {
        Some(target) => PaneTarget::try_from(target),
        None => PaneTarget::from_parts(
            flat.remote_host.as_deref(),
            flat.firecracker_vm.as_deref(),
            flat.firecracker_image.as_deref(),
            flat.firecracker_worktree.as_deref().map(Path::new),
        )
        .map_err(|_| "invalid pane target"),
    }
}

impl TryFrom<ProtobufCommand> for CommandToRun {
    type Error = &'static str;
    fn try_from(protobuf_command: ProtobufCommand) -> Result<Self, &'static str> {
        let path = PathBuf::from(protobuf_command.path);
        let args = protobuf_command.args;
        let cwd = protobuf_command.cwd.map(PathBuf::from);
        let remote_default_shell = protobuf_command.remote_default_shell;
        let target = pane_target_from_wire(
            protobuf_command.target,
            FlatPaneTarget {
                remote_host: protobuf_command.remote_host,
                firecracker_vm: protobuf_command.firecracker_vm,
                firecracker_image: protobuf_command.firecracker_image,
                firecracker_worktree: protobuf_command.firecracker_worktree,
            },
        )?;
        Ok(CommandToRun {
            path,
            args,
            cwd,
            target,
            remote_default_shell,
        })
    }
}

impl TryFrom<CommandToRun> for ProtobufCommand {
    type Error = &'static str;
    fn try_from(command_to_run: CommandToRun) -> Result<Self, &'static str> {
        let flat = FlatPaneTarget::from(&command_to_run.target);
        Ok(ProtobufCommand {
            path: command_to_run.path.display().to_string(),
            args: command_to_run.args,
            cwd: command_to_run.cwd.map(|c| c.display().to_string()),
            remote_host: flat.remote_host,
            remote_default_shell: command_to_run.remote_default_shell,
            firecracker_vm: flat.firecracker_vm,
            firecracker_image: flat.firecracker_image,
            firecracker_worktree: flat.firecracker_worktree,
            target: Some(ProtobufPaneTarget::from(&command_to_run.target)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin's target has to survive the protobuf hop — this is the wire
    /// format a plugin's `OpenCommandPane` request travels over to reach
    /// `zellij_exports.rs`, which forwards it into `RunCommandAction`.
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

    /// A plugin built before `PaneTarget` fills only the flat fields; they
    /// still describe the target.
    #[test]
    fn the_flat_fields_still_describe_the_target_when_the_structured_one_is_unset() {
        let protobuf_command = ProtobufCommand {
            path: "/usr/bin/cargo".to_owned(),
            firecracker_vm: Some("review".to_owned()),
            firecracker_image: Some("rust-dev".to_owned()),
            firecracker_worktree: Some("/work".to_owned()),
            ..Default::default()
        };
        let command_to_run = CommandToRun::try_from(protobuf_command).unwrap();
        assert_eq!(
            command_to_run.target,
            PaneTarget::Firecracker {
                vm: "review".to_owned(),
                image: Some("rust-dev".to_owned()),
                worktree: Some(PathBuf::from("/work")),
            }
        );
        let ssh = ProtobufCommand {
            path: "/usr/bin/htop".to_owned(),
            remote_host: Some("build-box".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            CommandToRun::try_from(ssh).unwrap().target,
            PaneTarget::Ssh {
                host: "build-box".to_owned()
            }
        );
        let both = ProtobufCommand {
            path: "/usr/bin/htop".to_owned(),
            remote_host: Some("build-box".to_owned()),
            firecracker_vm: Some("review".to_owned()),
            ..Default::default()
        };
        assert!(CommandToRun::try_from(both).is_err());
    }
}
