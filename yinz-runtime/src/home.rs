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

pub fn layout_dir_or_default(
    configured: Option<PathBuf>,
    config_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    configured
        .or_else(|| get_layout_dir(config_dir.or_else(find_default_config_dir)))
        .map(|dir| {
            std::env::current_dir()
                .map(|cwd| cwd.join(&dir))
                .unwrap_or(dir)
        })
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
#[path = "unit/layout_dir_tests.rs"]
mod layout_dir_tests;

#[cfg(test)]
#[path = "unit/home.rs"]
mod tests;
