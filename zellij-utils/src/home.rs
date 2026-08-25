//!
//! # This module contain everything you'll need to access local system paths
//! containing configuration and layouts

use crate::consts::{SYSTEM_DEFAULT_CONFIG_DIR, ZELLIJ_PROJ_DIR};

use std::{path::Path, path::PathBuf};

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
pub(crate) fn default_config_dirs() -> Vec<Option<PathBuf>> {
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
    ZELLIJ_PROJ_DIR.config_dir().to_owned()
}

pub fn xdg_data_dir() -> PathBuf {
    ZELLIJ_PROJ_DIR.data_dir().to_owned()
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

pub fn get_theme_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join("themes"))
}

pub fn default_theme_dir() -> Option<PathBuf> {
    find_default_config_dir().map(|dir| dir.join("themes"))
}

/// The folder holding the user's own plugins, as opposed to the two that already exist: the
/// wasm cache under the *data* dir (`<data_dir>/plugins`), and `builtin_plugins_dir`, which
/// overrides where the shipped builtins are read from. This one is for plugins the user wrote.
///
/// Like `get_layout_dir` this only computes a path - it deliberately creates nothing. A `get_*`
/// that makes a directory is a trap, and an empty folder in every user's config dir would be
/// one more thing to explain for a feature most sessions never touch.
pub fn get_user_plugin_dir(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    config_dir.map(|dir| dir.join("plugins"))
}

pub fn default_user_plugin_dir() -> Option<PathBuf> {
    find_default_config_dir().map(|dir| dir.join("plugins"))
}

/// The user's plugin folder for a session started with `config_dir`: the `--config-dir` or
/// `ZELLIJ_CONFIG_DIR` value when there is one, and the first conventional config dir otherwise.
///
/// This precedence, rather than the bare `default_user_plugin_dir`, is what `zellij setup --check`
/// reports (see `setup.rs`), so anything that tells a plugin where its folder is has to use it
/// too - otherwise a session started with `--config-dir` loads plugins from one folder while
/// `setup --check` names another.
pub fn user_plugin_dir_or_default(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    get_user_plugin_dir(config_dir.or_else(find_default_config_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_plugin_dir_hangs_off_the_config_dir() {
        assert_eq!(
            get_user_plugin_dir(Some(PathBuf::from("/etc/zellij"))),
            Some(PathBuf::from("/etc/zellij/plugins")),
        );
        assert_eq!(get_user_plugin_dir(None), None);
    }

    /// The whole point of the helper: a session started with `--config-dir` must not be handed
    /// the conventional config dir's plugin folder, which is what the argument-less
    /// `default_user_plugin_dir` would give it.
    #[test]
    fn a_supplied_config_dir_beats_the_conventional_ones() {
        assert_eq!(
            user_plugin_dir_or_default(Some(PathBuf::from("/etc/zellij"))),
            Some(PathBuf::from("/etc/zellij/plugins")),
        );
    }

    /// `find_default_config_dir` is `None` under `cfg(test)`, so with nothing supplied there is
    /// nothing to fall back to and the answer is honestly "no folder" rather than a guess.
    #[test]
    fn no_config_dir_anywhere_resolves_to_no_plugin_dir() {
        assert_eq!(user_plugin_dir_or_default(None), None);
    }

    /// It must not be `<data_dir>/plugins`, which is the wasm cache and a different folder
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
