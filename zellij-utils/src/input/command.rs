//! Trigger a command
use crate::data::{Direction, OriginatingPlugin};
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
    #[serde(default)]
    pub use_terminal_title: bool,
    /// The machine this command runs on, if not this one.
    ///
    /// Carried alongside the command rather than as a new `Run` variant: a
    /// variant would ripple into `Run::merge`, the KDL property whitelist and
    /// three arms of session serialization, all to express something that is
    /// really an attribute of *where* the command runs rather than of what it
    /// is. `serde(default)` keeps every existing layout and serialized session
    /// parsing unchanged.
    #[serde(default)]
    pub remote_host: Option<String>,
    /// Run the default login shell of the machine this pane runs on, and ignore
    /// `command`.
    ///
    /// Only meaningful together with `remote_host`: which shell a user has, and
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
                self.remote_host.as_deref().unwrap_or_default()
            );
        }
        let mut command: String = self
            .command
            .as_path()
            .as_os_str()
            .to_string_lossy()
            .to_string();
        for arg in &self.args {
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
    /// See `RunCommand::remote_host`.
    #[serde(default)]
    pub remote_host: Option<String>,
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
            use_terminal_title: action.use_terminal_title,
            remote_host: action.remote_host,
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
            remote_host: run_command.remote_host,
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
        self.remote_default_shell && self.remote_host.is_some()
    }
}
