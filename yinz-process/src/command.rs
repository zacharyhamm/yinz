use nix::unistd::{access, AccessFlags};
use std::{
    env,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::input::command::RunCommand;

/// Resolve a portable agent launcher using the Yinz binary on the execution host.
pub fn resolve_agent_launcher(
    host_bin: &Path,
    program: &mut PathBuf,
    args: &mut [String],
) -> Result<()> {
    if program == Path::new("yinz")
        && matches!(args, [agent, launcher, ..] if agent == "agent" && matches!(launcher.as_str(), "codex" | "codex-untracked" | "claude" | "muse"))
    {
        *program = host_bin.to_path_buf();
    } else {
        let index = if program == Path::new("env") {
            // Generated env wrappers contain assignments followed by the command.
            args.iter().position(|arg| !arg.contains('='))
        } else {
            None
        };
        if let Some(index) = index {
            let (assignments, rest) = args.split_at_mut(index + 1);
            let executable = &mut assignments[index];
            let mut path = PathBuf::from(&*executable);
            resolve_agent_launcher(host_bin, &mut path, rest)?;
            *executable = path
                .into_os_string()
                .into_string()
                .map_err(|_| anyhow!("the agent launcher path is not valid UTF-8"))?;
        }
    }
    Ok(())
}

/// The program and arguments to exec for `cmd` on this machine.
///
/// A Bubblewrap target is entered here and nowhere else: the pane's command stays the one
/// that was asked for, and only the exec goes through this binary's internal launcher,
/// which builds the sandbox and replaces itself with it.
pub fn exec_program_and_args(cmd: &RunCommand) -> Result<(PathBuf, Vec<String>)> {
    let Some(root) = cmd.target.bubblewrap_root() else {
        return Ok((cmd.command.clone(), cmd.args.clone()));
    };
    let launcher = yinz_runtime::own_binary::server_binary()
        .context("find this Yinz binary for the Bubblewrap launcher")?;
    let mut args = vec![
        yinz_types::agent::BUBBLEWRAP_RUN_FLAG.to_owned(),
        root.to_string_lossy().into_owned(),
        "--".to_owned(),
        cmd.command.to_string_lossy().into_owned(),
    ];
    args.extend(cmd.args.iter().cloned());
    Ok((launcher.clone(), args))
}

/// Whether `cmd` can be exec'd, as far as this machine can tell before trying. A Bubblewrap
/// command resolves on the sandbox's `PATH`, which the server checked when it resolved the
/// spawn; here it only needs its launcher.
pub fn can_exec(cmd: &RunCommand, search_path: Option<&std::ffi::OsStr>) -> bool {
    cmd.target.is_bubblewrap() || resolve_command_with_path(cmd, search_path).is_some()
}

fn find_executable(candidate: &std::path::Path) -> Option<PathBuf> {
    if candidate.is_file() && access(candidate, AccessFlags::X_OK).is_ok() {
        return Some(candidate.to_path_buf());
    }
    None
}

/// Resolve a command as the child will: explicit paths from its working directory,
/// bare names through PATH. Relative PATH entries also start at the child's cwd.
///
/// Resolved against the `PATH` the child will actually get.
///
/// A remote pane's child is given a `PATH` composed from the far machine's passwd
/// entry rather than the supervisor's own, and `Command` execs against the
/// environment it was handed. Searching this process's `PATH` would answer a
/// question about the wrong one - rejecting a command that would have exec'd, or
/// admitting one that will not.
///
/// `None` means the child inherits this process's `PATH`, which is every local
/// pane.
pub fn resolve_command_with_path(
    cmd: &RunCommand,
    search_path: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let command = &cmd.command;
    let inherited;
    let search_path = match search_path {
        Some(search_path) => Some(search_path),
        None => {
            inherited = env::var_os("PATH");
            inherited.as_deref()
        },
    };
    if command.is_absolute() {
        return find_executable(command);
    }
    // Both spawn paths retain the inherited cwd when the requested directory
    // disappeared. Keep their preflight check in agreement with that fallback.
    let cwd = std::path::absolute(
        cmd.cwd
            .as_deref()
            .filter(|cwd| cwd.is_dir())
            .unwrap_or(Path::new(".")),
    )
    .ok();
    if command.as_os_str().as_bytes().contains(&b'/') {
        return find_executable(&cwd?.join(command));
    }
    if let Some(paths) = search_path {
        for path in env::split_paths(paths) {
            let path = if path.is_absolute() {
                path
            } else if let Some(cwd) = &cwd {
                cwd.join(path)
            } else {
                continue;
            };
            if let Some(resolved) = find_executable(&path.join(command)) {
                return Some(resolved);
            }
        }
    }
    None
}

pub fn command_exists(cmd: &RunCommand) -> bool {
    can_exec(cmd, None)
}

#[cfg(test)]
#[path = "unit/command_tests.rs"]
mod tests;
