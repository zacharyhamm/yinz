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
    /// A named Firecracker VM owned by this session.
    Firecracker {
        /// The VM's name, unique within the session.
        vm: String,
        /// Immutable image selected for the VM, or the provider's default.
        image: Option<String>,
        /// Canonical host worktree mirrored into the VM, or the pane's cwd.
        worktree: Option<PathBuf>,
    },
}

/// The prefix `PaneTarget::display` puts before a Firecracker VM's name.
///
/// `validate_firecracker_vm_name` never admits a `:`, and `validate_ssh_host`
/// rejects hosts that start with it, so the encoding is unambiguous.
pub const FIRECRACKER_HOST_PREFIX: &str = "firecracker:";

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

    /// A Firecracker target, carrying every rule the layout, keybind, CLI and
    /// plugin spellings share: the VM and image names use the charset
    /// `validate_firecracker_vm_name` admits, blank image and worktree values
    /// mean "not given", and a worktree is an absolute path.
    pub fn firecracker(
        vm: &str,
        image: Option<&str>,
        worktree: Option<&Path>,
    ) -> Result<Self, String> {
        let vm = vm.trim();
        validate_firecracker_vm_name(vm)?;
        let image = match image.map(str::trim).filter(|image| !image.is_empty()) {
            Some(image) => {
                validate_firecracker_vm_name(image)
                    .map_err(|error| error.replacen("VM name", "image name", 1))?;
                Some(image.to_owned())
            },
            None => None,
        };
        let worktree = match worktree.filter(|path| !path.as_os_str().is_empty()) {
            Some(path) if path.is_absolute() => Some(path.to_path_buf()),
            Some(path) => {
                return Err(format!(
                    "firecracker_worktree must be an absolute path, got {}",
                    path.display()
                ))
            },
            None => None,
        };
        Ok(PaneTarget::Firecracker {
            vm: vm.to_owned(),
            image,
            worktree,
        })
    }

    /// Build a target from the loose optional spellings every parser starts
    /// from: `None`/`None` is local, an ssh host and a VM name together are
    /// refused, and an image or worktree without a VM is refused too.
    pub fn from_parts(
        remote_host: Option<&str>,
        firecracker_vm: Option<&str>,
        firecracker_image: Option<&str>,
        firecracker_worktree: Option<&Path>,
    ) -> Result<Self, String> {
        let remote_host = remote_host.map(str::trim).filter(|host| !host.is_empty());
        let firecracker_vm = firecracker_vm.map(str::trim).filter(|vm| !vm.is_empty());
        let has_image = firecracker_image
            .map(str::trim)
            .is_some_and(|image| !image.is_empty());
        let has_worktree = firecracker_worktree.is_some_and(|path| !path.as_os_str().is_empty());
        match (remote_host, firecracker_vm) {
            (Some(_), Some(_)) => {
                Err("a pane cannot use both firecracker_vm and remote_host".to_owned())
            },
            (None, Some(vm)) => {
                PaneTarget::firecracker(vm, firecracker_image, firecracker_worktree)
            },
            (_, None) if has_image || has_worktree => Err(match (has_image, has_worktree) {
                (true, true) => {
                    "firecracker_image and firecracker_worktree need a firecracker_vm".to_owned()
                },
                (true, false) => "firecracker_image needs a firecracker_vm".to_owned(),
                _ => "firecracker_worktree needs a firecracker_vm".to_owned(),
            }),
            (Some(host), None) => PaneTarget::ssh(host),
            (None, None) => Ok(PaneTarget::Local),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PaneTarget::Local)
    }
    /// Runs on another machine, over ssh or in a VM.
    pub fn is_remote(&self) -> bool {
        !self.is_local()
    }
    pub fn is_firecracker(&self) -> bool {
        matches!(self, PaneTarget::Firecracker { .. })
    }
    /// The ssh host of an `Ssh` target.
    pub fn ssh_host(&self) -> Option<&str> {
        match self {
            PaneTarget::Ssh { host } => Some(host),
            _ => None,
        }
    }
    /// The VM name of a `Firecracker` target.
    pub fn firecracker_vm(&self) -> Option<&str> {
        match self {
            PaneTarget::Firecracker { vm, .. } => Some(vm),
            _ => None,
        }
    }
    pub fn firecracker_image(&self) -> Option<&str> {
        match self {
            PaneTarget::Firecracker { image, .. } => image.as_deref(),
            _ => None,
        }
    }
    pub fn firecracker_worktree(&self) -> Option<&Path> {
        match self {
            PaneTarget::Firecracker { worktree, .. } => worktree.as_deref(),
            _ => None,
        }
    }
    /// What to show a user for this target: `None` for a local pane, the ssh
    /// host, or `firecracker:<vm>`. The VM's ssh endpoint is a provider detail
    /// and never appears here.
    pub fn display(&self) -> Option<String> {
        match self {
            PaneTarget::Local => None,
            PaneTarget::Ssh { host } => Some(host.clone()),
            PaneTarget::Firecracker { vm, .. } => Some(format!("{FIRECRACKER_HOST_PREFIX}{vm}")),
        }
    }

    /// The inverse of `display`, for readers that only have the string.
    pub fn parse_display(host: Option<&str>) -> Self {
        match host {
            None => PaneTarget::Local,
            Some(host) => match host.strip_prefix(FIRECRACKER_HOST_PREFIX) {
                Some(vm) => PaneTarget::Firecracker {
                    vm: vm.to_owned(),
                    image: None,
                    worktree: None,
                },
                None => PaneTarget::Ssh {
                    host: host.to_owned(),
                },
            },
        }
    }

    /// Fill what this target leaves unsaid from `base`, the way a layout's
    /// pane inherits from its template: a local target takes the base
    /// wholesale, and a Firecracker target without an image or a worktree takes
    /// each from a Firecracker base individually.
    pub fn merge_from(&mut self, base: &PaneTarget) {
        match (self, base) {
            (this @ PaneTarget::Local, base) => *this = base.clone(),
            (
                PaneTarget::Firecracker {
                    image, worktree, ..
                },
                PaneTarget::Firecracker {
                    image: base_image,
                    worktree: base_worktree,
                    ..
                },
            ) => {
                if image.is_none() {
                    image.clone_from(base_image);
                }
                if worktree.is_none() {
                    worktree.clone_from(base_worktree);
                }
            },
            _ => {},
        }
    }
}

/// The charset and length a Firecracker VM or image name must satisfy.
pub fn validate_firecracker_vm_name(name: &str) -> Result<(), String> {
    if (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid Firecracker VM name {name:?}: use 1-64 letters, digits, '.', '_' or '-'"
        ))
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

/// `CommandToRun` as it arrives from serde callers (Lua plugins, JSON): either
/// a `target`, or the flat `remote_host`/`firecracker_*` keys the API accepted
/// before `PaneTarget`, which still describe one.
#[derive(Default, Deserialize)]
#[serde(default)]
struct CommandToRunWire {
    path: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    target: Option<PaneTarget>,
    remote_host: Option<String>,
    firecracker_vm: Option<String>,
    firecracker_image: Option<String>,
    firecracker_worktree: Option<PathBuf>,
    remote_default_shell: bool,
}

impl<'de> Deserialize<'de> for CommandToRun {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CommandToRunWire::deserialize(deserializer)?;
        let target = match wire.target {
            Some(target) => target,
            None => PaneTarget::from_parts(
                wire.remote_host.as_deref(),
                wire.firecracker_vm.as_deref(),
                wire.firecracker_image.as_deref(),
                wire.firecracker_worktree.as_deref(),
            )
            .map_err(serde::de::Error::custom)?,
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

/// Git setup Zellij performs before opening a command pane.
///
/// `New` in both branch variants means *the one named*, created when it is not
/// there already: a branch or a worktree that exists is reused rather than
/// refused, and `base` then has nothing to say - an existing branch has its own
/// history. See `prepare_git_checkout_in` in the server for the exact order the
/// two kinds of reuse are tried in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitCheckout {
    Current,
    /// The branch, in the checkout the request came from. Reuse means switching
    /// to it, or - when another worktree holds it, which git allows only one of
    /// - running in that worktree instead.
    NewBranch {
        branch: String,
        base: String,
    },
    /// The branch, in a worktree of its own at `path`. A worktree already at
    /// `path` is used as it stands; failing that, the worktree already holding
    /// `branch` is.
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
