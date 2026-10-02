//! Trigger a command
use crate::command::PaneTarget;
use crate::data::OriginatingPlugin;
use crate::geometry::Direction;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum TerminalAction {
    OpenFile(OpenFilePayload),
    RunCommand(RunCommand),
}

impl TerminalAction {
    pub fn change_cwd(&mut self, new_cwd: PathBuf) {
        match self {
            TerminalAction::OpenFile(open_file_payload) => {
                open_file_payload.cwd = Some(new_cwd);
            },
            TerminalAction::RunCommand(run_command) => {
                run_command.cwd = Some(new_cwd);
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenFilePayload {
    pub path: PathBuf,
    pub line_number: Option<usize>,
    pub cwd: Option<PathBuf>,
    pub originating_plugin: Option<OriginatingPlugin>,
}

impl Default for OpenFilePayload {
    fn default() -> Self {
        OpenFilePayload {
            path: PathBuf::new(),
            line_number: None,
            cwd: None,
            originating_plugin: None,
        }
    }
}

impl OpenFilePayload {
    pub fn new(path: PathBuf, line_number: Option<usize>, cwd: Option<PathBuf>) -> Self {
        OpenFilePayload {
            path,
            line_number,
            cwd,
            originating_plugin: None,
        }
    }
    pub fn with_originating_plugin(mut self, originating_plugin: OriginatingPlugin) -> Self {
        self.originating_plugin = Some(originating_plugin);
        self
    }
}

#[derive(Clone, Debug, Deserialize, Default, Serialize, PartialEq, Eq)]
pub struct RunCommand {
    #[serde(alias = "cmd")]
    pub command: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub hold_on_close: bool,
    #[serde(default)]
    pub hold_on_start: bool,
    #[serde(default)]
    pub originating_plugin: Option<OriginatingPlugin>,
    /// Ownership recovered from the supervisor during adoption. This is server
    /// metadata, not a client action argument; old plugin/client ids are discarded.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_owner: Option<String>,
    #[serde(default)]
    pub use_terminal_title: bool,
    /// The machine this command runs on. See `PaneTarget`.
    #[serde(default)]
    pub target: PaneTarget,
    /// Run the default login shell of the machine this pane runs on, and ignore
    /// `command`.
    ///
    /// Only meaningful with a remote `target`: which shell a user has, and
    /// where their home directory is, are facts about *that* machine, and the
    /// machine asking for the pane cannot know either. So nothing is resolved
    /// here — the broker on the far side reads its own passwd entry, exactly as
    /// sshd does for an interactive session. Shipping this side's `$SHELL`
    /// instead would name a path that need not even exist there.
    #[serde(default)]
    pub remote_default_shell: bool,
}

impl std::fmt::Display for RunCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A pane whose shell is chosen on the far machine has no command to
        // print, and an empty string here would reach pane frames and client
        // listings as a blank. The host is the truthful thing to say about it
        // until that machine reports back the argv it actually started.
        if self.is_remote_default_shell() {
            // Named the way the held-pane banner reads it — "Waiting to run:
            // login shell on build-box" — rather than as a bare host.
            return write!(
                f,
                "login shell on {}",
                self.display_host().as_deref().unwrap_or_default()
            );
        }
        let mut command: String = self
            .command
            .as_path()
            .as_os_str()
            .to_string_lossy()
            .to_string();
        let mut args = self.args.iter().peekable();
        while let Some(arg) = args.next() {
            // Filter before joining: data-directory paths can contain spaces, and
            // a stringified command no longer has reliable argument boundaries.
            if arg == "--extension"
                && args
                    .peek()
                    .is_some_and(|value| crate::agent::is_pi_extension_argument(value))
            {
                args.next();
                continue;
            }
            command.push(' ');
            command.push_str(arg);
        }
        write!(f, "{}", command)
    }
}

/// Intermediate representation
#[derive(Clone, Debug, Deserialize, Default, Serialize, PartialEq, Eq)]
pub struct RunCommandAction {
    #[serde(rename = "cmd")]
    pub command: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub direction: Option<Direction>,
    #[serde(default)]
    pub hold_on_close: bool,
    #[serde(default)]
    pub hold_on_start: bool,
    #[serde(default)]
    pub originating_plugin: Option<OriginatingPlugin>,
    #[serde(default)]
    pub use_terminal_title: bool,
    /// See `RunCommand::target`.
    #[serde(default)]
    pub target: PaneTarget,
    /// See `RunCommand::remote_default_shell`.
    #[serde(default)]
    pub remote_default_shell: bool,
}

impl From<RunCommandAction> for RunCommand {
    fn from(action: RunCommandAction) -> Self {
        RunCommand {
            command: action.command,
            args: action.args,
            cwd: action.cwd,
            hold_on_close: action.hold_on_close,
            hold_on_start: action.hold_on_start,
            originating_plugin: action.originating_plugin,
            workflow_owner: None,
            use_terminal_title: action.use_terminal_title,
            target: action.target,
            remote_default_shell: action.remote_default_shell,
        }
    }
}

impl From<RunCommand> for RunCommandAction {
    fn from(run_command: RunCommand) -> Self {
        RunCommandAction {
            command: run_command.command,
            args: run_command.args,
            cwd: run_command.cwd,
            direction: None,
            hold_on_close: run_command.hold_on_close,
            hold_on_start: run_command.hold_on_start,
            originating_plugin: run_command.originating_plugin,
            use_terminal_title: run_command.use_terminal_title,
            target: run_command.target,
            remote_default_shell: run_command.remote_default_shell,
        }
    }
}

impl RunCommandAction {
    pub fn new(mut command: Vec<String>) -> Self {
        if command.is_empty() {
            Default::default()
        } else {
            RunCommandAction {
                command: PathBuf::from(command.remove(0)),
                args: command,
                ..Default::default()
            }
        }
    }
    pub fn populate_originating_plugin(&mut self, originating_plugin: OriginatingPlugin) {
        self.originating_plugin = Some(originating_plugin);
    }
}

impl RunCommand {
    /// Portable ownership survives adoption; plugin/client numbers do not.
    pub fn workflow_owner(&self) -> Option<&str> {
        self.workflow_owner.as_deref().or_else(|| {
            self.originating_plugin
                .as_ref()?
                .context
                .get("swamp_owner")
                .map(String::as_str)
        })
    }
    pub fn new(command: PathBuf) -> Self {
        RunCommand {
            command,
            ..Default::default()
        }
    }
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }
    /// The predicate every consumer should use, rather than reading the flag.
    ///
    /// `remote_default_shell` without a host asks this machine to resolve a
    /// shell it was never told to resolve, and an empty command is how a
    /// mistyped command already arrives. Requiring both keeps such a pane on
    /// the ordinary `CommandNotFound` path instead of quietly logging the user
    /// in somewhere.
    pub fn is_remote_default_shell(&self) -> bool {
        self.remote_default_shell && self.target.is_remote()
    }
    /// `PaneTarget::display` of this command's target.
    pub fn display_host(&self) -> Option<String> {
        self.target.display()
    }
    /// Runs on another machine, so the spawn resolves an endpoint (an ssh
    /// connection, a VM to boot) and happens off the pty thread; a held pane
    /// waits for the user instead.
    pub fn defers_spawn(&self) -> bool {
        self.target.is_remote() && !self.hold_on_start
    }
    /// A start failure is shown in the pane rather than closing it: always for
    /// a remote target, whose failures (an unreachable host, a VM that did not
    /// boot) the user has to read, and otherwise only when asked.
    pub fn shows_start_failure(&self) -> bool {
        self.hold_on_close || self.target.is_remote()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    #[test]
    fn firecracker_panes_display_their_vm_name() {
        let command = RunCommand {
            target: PaneTarget::firecracker("review", None, None).unwrap(),
            ..Default::default()
        };

        assert_eq!(
            command.display_host().as_deref(),
            Some("firecracker:review")
        );
        assert_eq!(
            PaneTarget::parse_display(command.display_host().as_deref()),
            PaneTarget::Firecracker {
                vm: "review".to_owned(),
                image: None,
                worktree: None,
            }
        );
        assert_eq!(
            PaneTarget::parse_display(Some("build-box")),
            PaneTarget::Ssh {
                host: "build-box".to_owned()
            }
        );
        assert_eq!(PaneTarget::parse_display(None), PaneTarget::Local);
    }

    #[test]
    fn a_firecracker_target_carries_every_validation_rule() {
        assert!(PaneTarget::firecracker("bad/name", None, None).is_err());
        assert!(PaneTarget::firecracker("review", Some("bad/image"), None).is_err());
        assert!(PaneTarget::firecracker("review", None, Some(Path::new("relative"))).is_err());
        assert_eq!(
            PaneTarget::firecracker(" review ", Some(" "), Some(Path::new(""))).unwrap(),
            PaneTarget::Firecracker {
                vm: "review".to_owned(),
                image: None,
                worktree: None,
            }
        );
        assert!(PaneTarget::from_parts(Some("build-box"), Some("review"), None, None).is_err());
        assert!(PaneTarget::from_parts(None, None, Some("rust-dev"), None).is_err());
        assert!(PaneTarget::from_parts(Some("build-box"), None, Some("rust-dev"), None).is_err());
        assert_eq!(
            PaneTarget::from_parts(Some(" build-box "), None, None, None).unwrap(),
            PaneTarget::Ssh {
                host: "build-box".to_owned()
            }
        );
        assert_eq!(
            PaneTarget::from_parts(None, None, None, None).unwrap(),
            PaneTarget::Local
        );
    }

    #[test]
    fn a_consumer_target_inherits_image_and_worktree_individually() {
        let base =
            PaneTarget::firecracker("review", Some("rust-dev"), Some(Path::new("/w"))).unwrap();
        let mut local = PaneTarget::Local;
        local.merge_from(&base);
        assert_eq!(local, base);

        let mut other_vm = PaneTarget::firecracker("review-2", None, None).unwrap();
        other_vm.merge_from(&base);
        assert_eq!(
            other_vm,
            PaneTarget::Firecracker {
                vm: "review-2".to_owned(),
                image: Some("rust-dev".to_owned()),
                worktree: Some(PathBuf::from("/w")),
            }
        );

        let mut own_image = PaneTarget::firecracker("review-2", Some("go"), None).unwrap();
        own_image.merge_from(&base);
        assert_eq!(own_image.firecracker_image(), Some("go"));
        assert_eq!(own_image.firecracker_worktree(), Some(Path::new("/w")));

        let mut ssh = PaneTarget::ssh("build-box").unwrap();
        ssh.merge_from(&base);
        assert_eq!(ssh.ssh_host(), Some("build-box"));
    }
}
