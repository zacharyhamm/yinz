pub use super::generated_api::api::command::{
    pane_target::Kind as ProtobufPaneTargetKind, Command as ProtobufCommand,
    PaneTarget as ProtobufPaneTarget,
};
use yinz_types::command::{CommandToRun, PaneTarget};

use std::convert::TryFrom;
use std::path::{Path, PathBuf};

impl From<&PaneTarget> for ProtobufPaneTarget {
    fn from(target: &PaneTarget) -> Self {
        let kind = match target {
            PaneTarget::Local => ProtobufPaneTargetKind::Local(true),
            PaneTarget::Ssh { host } => ProtobufPaneTargetKind::SshHost(host.clone()),
            PaneTarget::Bubblewrap { root } => {
                ProtobufPaneTargetKind::BubblewrapRoot(root.display().to_string())
            },
        };
        ProtobufPaneTarget {
            kind: Some(kind),
            firecracker: None,
        }
    }
}

impl TryFrom<ProtobufPaneTarget> for PaneTarget {
    type Error = &'static str;
    fn try_from(target: ProtobufPaneTarget) -> Result<Self, &'static str> {
        if target.firecracker.is_some() {
            return Err("Firecracker execution is no longer supported.");
        }
        Ok(match target.kind {
            None | Some(ProtobufPaneTargetKind::Local(_)) => PaneTarget::Local,
            Some(ProtobufPaneTargetKind::SshHost(host)) => PaneTarget::Ssh { host },
            Some(ProtobufPaneTargetKind::BubblewrapRoot(root)) => {
                PaneTarget::bubblewrap(Path::new(&root)).map_err(|_| "invalid pane target")?
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
            // The flat spelling predates Bubblewrap; a plugin that reads only it sees a
            // pane on this machine, which is where the pane is.
            PaneTarget::Local | PaneTarget::Bubblewrap { .. } => FlatPaneTarget::default(),
            PaneTarget::Ssh { host } => FlatPaneTarget {
                remote_host: Some(host.clone()),
                ..Default::default()
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
    if flat.firecracker_vm.is_some()
        || flat.firecracker_image.is_some()
        || flat.firecracker_worktree.is_some()
    {
        return Err("Firecracker execution is no longer supported.");
    }
    match target {
        Some(target) => PaneTarget::try_from(target),
        None => flat
            .remote_host
            .as_deref()
            .map(PaneTarget::ssh)
            .transpose()
            .map(|target| target.unwrap_or_default())
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

            target: Some(ProtobufPaneTarget::from(&command_to_run.target)),
            firecracker_vm: None,
            firecracker_image: None,
            firecracker_worktree: None,
        })
    }
}

#[cfg(test)]
#[path = "unit/command_tests.rs"]
mod tests;
