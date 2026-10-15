//!
//! # This module contain everything you'll need to access local system paths
//! containing configuration and layouts

use crate::consts::{SYSTEM_DEFAULT_CONFIG_DIR, YINZ_PROJ_DIR};

use std::{path::Component, path::Path, path::PathBuf};

use crate::home_unix as platform;

#[cfg(not(test))]
/// Goes through a predefined list and checks for an already
/// existing config directory, returns the first match
pub fn find_default_config_dir() -> Option<PathBuf> {
    default_config_dirs()
        .into_iter()
        .filter(|p| p.is_some())
        .find(|p| p.clone().unwrap().exists())
        .flatten()
}

#[cfg(test)]
pub fn find_default_config_dir() -> Option<PathBuf> {
    None
}

/// Order in which config directories are checked
pub fn default_config_dirs() -> Vec<Option<PathBuf>> {
    vec![
        home_config_dir(),
        Some(xdg_config_dir()),
        Some(Path::new(SYSTEM_DEFAULT_CONFIG_DIR).to_path_buf()),
    ]
}

/// Looks for an existing dir, uses that, else returns a
/// dir matching the config spec.
pub fn get_default_data_dir() -> PathBuf {
    [xdg_data_dir(), platform::system_data_dir()]
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(xdg_data_dir)
}

pub fn xdg_config_dir() -> PathBuf {
    YINZ_PROJ_DIR.config_dir().to_owned()
}

pub fn xdg_data_dir() -> PathBuf {
    YINZ_PROJ_DIR.data_dir().to_owned()
}

pub fn home_config_dir() -> Option<PathBuf> {
    platform::home_config_dir()
}

pub fn try_create_home_config_dir() {
    platform::try_create_home_config_dir()
}

pub fn system_data_dir() -> PathBuf {
    platform::system_data_dir()
}

pub fn get_layout_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join("layouts"))
}

pub fn default_layout_dir() -> Option<PathBuf> {
    find_default_config_dir().map(|dir| dir.join("layouts"))
}

/// The drop-in folder beside the config file: every `*.kdl` in it is layered over the config,
/// in file-name order, and it is where yinz itself writes what it is asked to persist
/// (`yinz-managed.kdl`). Computed only, never created - see `get_user_plugin_dir`.
pub fn get_config_overlay_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join(crate::consts::CONFIG_OVERLAY_DIR_NAME))
}

pub fn default_config_overlay_dir() -> Option<PathBuf> {
    get_config_overlay_dir(find_default_config_dir())
}

pub fn get_theme_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join("themes"))
}

pub fn default_theme_dir() -> Option<PathBuf> {
    find_default_config_dir().map(|dir| dir.join("themes"))
}

/// The folder holding the user's own plugins.
/// Like `get_layout_dir` this only computes a path - it deliberately creates nothing. A `get_*`
/// that makes a directory is a trap, and an empty folder in every user's config dir would be
/// one more thing to explain for a feature most sessions never touch.
pub fn get_user_plugin_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join("plugins"))
}

pub fn default_user_plugin_dir() -> Option<PathBuf> {
    find_default_config_dir().map(|dir| dir.join("plugins"))
}

/// An absolute path, without touching the filesystem: a relative one is joined onto the process's
/// cwd, and `.` / `..` are then folded away lexically.
///
/// Lexical rather than `canonicalize` because the folder this is used for need not exist - see
/// `get_user_plugin_dir` - and canonicalizing a path that is not there fails. No symlink is
/// resolved, so this answers "which path did the user mean" rather than "which file is that".
pub fn absolute_lexical(path: &Path) -> PathBuf {
    let path = match path.is_absolute() {
        true => path.to_path_buf(),
        // Nothing better to say if the cwd is unreadable: the path stays as it was, which is
        // exactly what every reader of it would have done with it anyway.
        false => match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => return path.to_path_buf(),
        },
    };

    let mut folded = PathBuf::new();
    for part in path.components() {
        match part {
            // A `.` names the folder it sits in, so it simply goes.
            Component::CurDir => {},
            // `pop` on a bare root is a no-op, so `/..` stays `/` - as it does on the filesystem.
            Component::ParentDir => {
                folded.pop();
            },
            part => folded.push(part),
        }
    }
    folded
}

/// The user's plugin folder for a session: the configured `user_plugin_dir` when there is one, and
/// otherwise the `plugins` folder of the session's config dir - the `--config-dir` or
/// `YINZ_CONFIG_DIR` value when there is one, and the first conventional config dir otherwise.
///
/// This precedence, rather than the bare `default_user_plugin_dir`, is what `yinz setup --check`
/// reports (see `setup.rs`), so anything that tells a plugin where its folder is has to use it
/// too - otherwise a session started with `--config-dir` loads plugins from one folder while
/// `setup --check` names another.
///
/// The answer is always absolute. Neither option is normalized when it is parsed - and must not be,
/// since a config write-back would then rewrite the user's own relative path - but a *relative*
/// folder is resolved by the server so every plugin receives the same absolute path.
pub fn user_plugin_dir_or_default(
    configured: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    configured
        .or_else(|| get_user_plugin_dir(config_dir.or_else(find_default_config_dir)))
        .map(|dir| absolute_lexical(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_plugin_dir_hangs_off_the_config_dir() {
        assert_eq!(
            get_user_plugin_dir(Some(PathBuf::from("/etc/yinz"))),
            Some(PathBuf::from("/etc/yinz/plugins")),
        );
        assert_eq!(get_user_plugin_dir(None), None);
    }

    /// The whole point of the helper: a session started with `--config-dir` must not be handed
    /// the conventional config dir's plugin folder, which is what the argument-less
    /// `default_user_plugin_dir` would give it.
    #[test]
    fn a_supplied_config_dir_beats_the_conventional_ones() {
        assert_eq!(
            user_plugin_dir_or_default(None, Some(PathBuf::from("/etc/yinz"))),
            Some(PathBuf::from("/etc/yinz/plugins")),
        );
    }

    /// And a configured folder beats both, wherever the config dir happens to be.
    #[test]
    fn a_configured_folder_beats_the_config_dir() {
        assert_eq!(
            user_plugin_dir_or_default(
                Some(PathBuf::from("/srv/plugins")),
                Some(PathBuf::from("/etc/yinz")),
            ),
            Some(PathBuf::from("/srv/plugins")),
        );
    }

    /// `find_default_config_dir` is `None` under `cfg(test)`, so with nothing supplied there is
    /// nothing to fall back to and the answer is honestly "no folder" rather than a guess.
    #[test]
    fn no_config_dir_anywhere_resolves_to_no_plugin_dir() {
        assert_eq!(user_plugin_dir_or_default(None, None), None);
    }

    /// The reason the helper absolutizes at all: a plugin is handed this path and has no cwd to
    /// join it onto, so a relative folder has to be resolved here or not at all.
    #[test]
    fn a_relative_configured_folder_is_resolved_against_the_cwd() {
        let cwd = std::env::current_dir().expect("a cwd");
        assert_eq!(
            user_plugin_dir_or_default(Some(PathBuf::from("plugins")), None),
            Some(cwd.join("plugins")),
        );
    }

    #[test]
    fn a_relative_config_dir_yields_an_absolute_plugin_folder() {
        let cwd = std::env::current_dir().expect("a cwd");
        assert_eq!(
            user_plugin_dir_or_default(None, Some(PathBuf::from("relcfg"))),
            Some(cwd.join("relcfg/plugins")),
        );
    }

    #[test]
    fn absolutizing_folds_away_here_and_parent_segments() {
        assert_eq!(
            absolute_lexical(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(absolute_lexical(Path::new("/a/./b")), PathBuf::from("/a/b"));
        assert_eq!(absolute_lexical(Path::new("/a/b/")), PathBuf::from("/a/b"));
    }

    /// As on the filesystem: the root is its own parent.
    #[test]
    fn the_root_has_no_parent_to_climb_to() {
        assert_eq!(absolute_lexical(Path::new("/..")), PathBuf::from("/"));
        assert_eq!(absolute_lexical(Path::new("/")), PathBuf::from("/"));
    }

    #[test]
    fn a_relative_path_climbs_out_of_the_cwd_lexically() {
        let cwd = std::env::current_dir().expect("a cwd");
        let parent = cwd.parent().unwrap_or(&cwd);
        assert_eq!(
            absolute_lexical(Path::new("../plugins")),
            parent.join("plugins")
        );
    }

    /// It must remain under the configuration directory, independently of data storage
    /// entirely - the two are told apart only by which of these functions produced them.
    #[test]
    fn the_user_plugin_dir_is_not_created_by_asking_for_it() {
        let config_dir = tempfile::tempdir().expect("a temp dir");
        let plugin_dir =
            get_user_plugin_dir(Some(config_dir.path().to_path_buf())).expect("a dir was supplied");
        assert!(
            !plugin_dir.exists(),
            "asking for the path must not make the folder"
        );
    }
}
