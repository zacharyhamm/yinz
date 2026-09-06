use std::{env, path::PathBuf};
use zellij_types::input::command::RunCommand;

fn find_executable(candidate: &std::path::Path) -> Option<PathBuf> {
    if candidate.exists() && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    None
}

/// Resolve a command to its absolute path, searching the working directory,
/// then this process's PATH.
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
    match cmd.cwd.as_ref() {
        Some(cwd) => {
            if let Some(resolved) = find_executable(&cwd.join(command)) {
                return Some(resolved);
            }
        },
        None => {
            if let Some(resolved) = find_executable(command) {
                return Some(resolved);
            }
        },
    }
    if let Some(paths) = search_path {
        for path in env::split_paths(paths) {
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
