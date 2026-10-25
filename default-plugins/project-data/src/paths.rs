//! One string per folder.
//!
//! Folders are map keys all over agent — the probed repositories, the project
//! roots — and every one of those maps is looked up by exact
//! string. `/work/agent/` and `/work/agent` are the same folder and would be two
//! keys, so an agent could sit inside a project the registry knows and still be
//! told it has none. Everything that takes a folder from outside normalizes it
//! here first.

use std::path::{Component, Path};

/// An absolute path with no `.` or `..`, rebuilt from its parts.
///
/// Rebuilding is what does the work: it takes off a last separator and joins
/// repeated ones. Purely lexical — no symlink is resolved and the filesystem is
/// never touched, because this runs in a plugin against paths that may name a
/// folder on the other side of a session.
pub fn normalize(path: &str) -> Result<String, String> {
    let path = Path::new(path);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err("path must be absolute and contain no . or .. components".to_owned());
    }
    Ok(path
        .components()
        .collect::<std::path::PathBuf>()
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
#[path = "unit/paths.rs"]
mod tests;
