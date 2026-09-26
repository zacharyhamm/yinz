use nix::unistd::{access, AccessFlags};
use std::{
    env,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use zellij_runtime::errors::prelude::*;
use zellij_types::input::command::RunCommand;

/// Resolve the portable Codex wrapper using the Zellij binary on the execution host.
pub fn resolve_codex_wrapper(
    host_bin: &Path,
    program: &mut PathBuf,
    args: &mut [String],
) -> Result<()> {
    if program == Path::new("zellij")
        && matches!(args, [hocus, codex, ..] if hocus == "hocus" && codex == "codex")
    {
        *program = host_bin.to_path_buf();
    } else if program == Path::new("env") {
        // Generated env wrappers contain assignments followed by the command.
        if let Some(index) = args.iter().position(|arg| !arg.contains('=')) {
            let (assignments, rest) = args.split_at_mut(index + 1);
            let executable = &mut assignments[index];
            let mut path = PathBuf::from(&*executable);
            resolve_codex_wrapper(host_bin, &mut path, rest)?;
            *executable = path
                .into_os_string()
                .into_string()
                .map_err(|_| anyhow!("the Codex launcher path is not valid UTF-8"))?;
        }
    }
    Ok(())
}

fn find_executable(candidate: &std::path::Path) -> Option<PathBuf> {
    if candidate.is_file() && access(candidate, AccessFlags::X_OK).is_ok() {
        return Some(candidate.to_path_buf());
    }
    None
}

/// Resolve a command as the child will: explicit paths from its working directory,
/// bare names through PATH. Relative PATH entries also start at the child's cwd.
pub fn resolve_command(cmd: &RunCommand) -> Option<PathBuf> {
    resolve_command_with_path(cmd, None)
}

/// As `resolve_command`, but against the `PATH` the child will actually get.
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
    resolve_command(cmd).is_some()
}

#[cfg(test)]
#[path = "unit/command_tests.rs"]
mod tests;
