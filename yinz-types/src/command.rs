//! Command payloads and target validation, independent of host process execution.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a pane's process runs.
///
/// Carried alongside the command rather than as a new `Run` variant: it is an
/// attribute of *where* the command runs rather than of what it is. `Local` is
/// the default, so every existing layout and serialized session parses
/// unchanged.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneTarget {
    /// This machine.
    #[default]
    Local,
    /// A machine reached over ssh; `host` is the `--remote-host` argument
    /// (an ssh alias, `user@host`, or an `ssh://` URL).
    Ssh { host: String },
    /// This machine, inside the Bubblewrap environment prepared for `root`: a primary
    /// checkout. The command stays the one asked for;
    /// the sandbox is applied where the process is exec'd.
    Bubblewrap { root: PathBuf },
}

impl PaneTarget {
    /// An ssh target. The host is trimmed; a blank host is refused, because it
    /// would silently become a local pane somewhere down the line.
    pub fn ssh(host: &str) -> Result<Self, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err("remote_host must not be empty".to_owned());
        }
        Ok(PaneTarget::Ssh {
            host: host.to_owned(),
        })
    }

    /// A Bubblewrap target for the prepared environment at `root`, an absolute path.
    pub fn bubblewrap(root: &Path) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err(format!(
                "bubblewrap must be an absolute path, got {}",
                root.display()
            ));
        }
        Ok(PaneTarget::Bubblewrap {
            root: root.to_path_buf(),
        })
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PaneTarget::Local)
    }
    /// Runs on another machine, over ssh. A Bubblewrap pane is neither
    /// `Local` nor remote: its cwd, pid and Git checkout are this machine's.
    pub fn is_remote(&self) -> bool {
        matches!(self, PaneTarget::Ssh { .. })
    }
    pub fn is_bubblewrap(&self) -> bool {
        matches!(self, PaneTarget::Bubblewrap { .. })
    }
    /// The prepared environment root of a `Bubblewrap` target.
    pub fn bubblewrap_root(&self) -> Option<&Path> {
        match self {
            PaneTarget::Bubblewrap { root } => Some(root),
            _ => None,
        }
    }
    /// The ssh host of an `Ssh` target.
    pub fn ssh_host(&self) -> Option<&str> {
        match self {
            PaneTarget::Ssh { host } => Some(host),
            _ => None,
        }
    }
    /// What to show a user for this target: `None` for a local pane, the ssh
    /// host for an SSH pane.
    pub fn display(&self) -> Option<String> {
        match self {
            PaneTarget::Local | PaneTarget::Bubblewrap { .. } => None,
            PaneTarget::Ssh { host } => Some(host.clone()),
        }
    }

    /// The inverse of `display`, for readers that only have the string.
    pub fn parse_display(host: Option<&str>) -> Self {
        match host {
            None => PaneTarget::Local,
            Some(host) => PaneTarget::Ssh {
                host: host.to_owned(),
            },
        }
    }

    /// A local target inherits the template target.
    pub fn merge_from(&mut self, base: &PaneTarget) {
        if self.is_local() {
            *self = base.clone();
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct CommandToRun {
    pub path: PathBuf,
    pub args: Vec<String>,
    /// Where to start the command. Locally, `/host`, `/data`, `/cache` and `/tmp`
    /// name the plugin mounts and a relative path is joined onto the plugin's
    /// cwd. With a remote `target`, the path is interpreted on that machine
    /// exactly as written, and `None` means the login directory there.
    pub cwd: Option<PathBuf>,
    /// The machine the command runs on.
    pub target: PaneTarget,
    /// Ignore `path` and run the login shell of the machine named by
    /// `target`, resolved there. See `RunCommand::remote_default_shell`:
    /// which shell a user has is a fact about that machine, so a plugin has
    /// nothing useful to put in `path` and says so with this instead.
    pub remote_default_shell: bool,
}

/// `CommandToRun` as it arrives from serde callers (TypeScript plugins, JSON): either
/// a `target`, or the flat `remote_host` key the API accepted
/// before `PaneTarget`, which still describe one.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CommandToRunWire {
    path: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    target: Option<PaneTarget>,
    remote_host: Option<String>,
    remote_default_shell: bool,
}

impl<'de> Deserialize<'de> for CommandToRun {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CommandToRunWire::deserialize(deserializer)?;
        let target = match wire.target {
            Some(target) => target,
            None => wire
                .remote_host
                .as_deref()
                .map(PaneTarget::ssh)
                .transpose()
                .map_err(serde::de::Error::custom)?
                .unwrap_or_default(),
        };
        Ok(CommandToRun {
            path: wire.path,
            args: wire.args,
            cwd: wire.cwd,
            target,
            remote_default_shell: wire.remote_default_shell,
        })
    }
}

/// Git setup Yinz performs before opening a command pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitCheckout {
    Current,
    /// The branch, in the checkout the request came from. Whether an existing
    /// branch is reused - switched to, or run in whichever worktree holds it,
    /// which git allows only one of - is the *caller's* to say; without that,
    /// the host cuts the next free name instead.
    NewBranch {
        branch: String,
        base: String,
    },
    /// The branch, in a worktree of its own at `path`. Reuse is again the
    /// caller's to ask for: with it, a worktree already at `path` is used as it
    /// stands and failing that the one already holding `branch` is; without it,
    /// a taken name means the next free one, for the branch and the worktree
    /// together.
    NewWorktree {
        branch: String,
        base: String,
        path: PathBuf,
    },
}

impl CommandToRun {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        CommandToRun {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        }
    }
    pub fn new_with_args<P: AsRef<Path>, A: AsRef<str>>(path: P, args: Vec<A>) -> Self {
        CommandToRun {
            path: path.as_ref().to_path_buf(),
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            ..Default::default()
        }
    }
}
