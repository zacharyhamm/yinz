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
mod tests {
    use super::*;

    #[test]
    fn normalization_takes_off_a_last_separator_and_joins_repeated_ones() {
        assert_eq!(normalize("/work/agent/").unwrap(), "/work/agent");
        assert_eq!(normalize("/work//agent").unwrap(), "/work/agent");
        assert_eq!(normalize("/work/agent").unwrap(), "/work/agent");
    }

    #[test]
    fn the_filesystem_root_is_left_alone() {
        assert_eq!(normalize("/").unwrap(), "/");
    }

    #[test]
    fn a_relative_or_traversing_path_is_refused() {
        assert!(normalize("work/agent").is_err());
        assert!(normalize("/work/../agent").is_err());
        assert!(normalize("").is_err());
    }

    /// `..` has to be refused because resolving it needs the filesystem, but a
    /// `.` names the folder it sits in and `Components` drops it for us.
    #[test]
    fn a_here_segment_is_simply_dropped() {
        assert_eq!(normalize("/work/./agent").unwrap(), "/work/agent");
    }
}
