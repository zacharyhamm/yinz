//! Zellij program-wide constants.

use crate::home::find_default_config_dir;
use directories::ProjectDirs;
use lazy_static::lazy_static;
use std::{path::PathBuf, sync::OnceLock};
use zellij_types::internal_ids::SessionName;

pub const ZELLIJ_CONFIG_FILE_ENV: &str = "ZELLIJ_CONFIG_FILE";
pub const ZELLIJ_CONFIG_DIR_ENV: &str = "ZELLIJ_CONFIG_DIR";
pub const ZELLIJ_LAYOUT_DIR_ENV: &str = "ZELLIJ_LAYOUT_DIR";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The folder under a data dir that holds the installed builtin plugin wasm, one
/// subfolder per `VERSION`: `<data dir>/builtins/<VERSION>/<name>.wasm`. See
/// `home::builtin_plugin_dir`. `xtask install` writes it; `resolve_wasm_bytes` reads it.
pub const BUILTIN_PLUGIN_DIR_NAME: &str = "builtins";
pub const DEFAULT_SCROLL_BUFFER_SIZE: usize = 10_000;
pub static SCROLL_BUFFER_SIZE: OnceLock<usize> = OnceLock::new();
pub static DEBUG_MODE: OnceLock<bool> = OnceLock::new();

pub const SYSTEM_DEFAULT_CONFIG_DIR: &str = "/etc/zellij";
pub const SYSTEM_DEFAULT_DATA_DIR_PREFIX: &str = system_default_data_dir();

pub const CLIENT_SERVER_CONTRACT_VERSION: usize = 1;

/// Legacy flat metadata path. Read current metadata through `SessionCheckpoint`.
pub fn session_info_cache_file_name(session_name: &SessionName) -> PathBuf {
    session_info_folder_for_session(session_name).join("session-metadata.kdl")
}

/// Logical resurrection path passed over IPC; the server resolves its checkpoint when loading.
pub fn session_layout_cache_file_name(session_name: &SessionName) -> PathBuf {
    session_info_folder_for_session(session_name).join("session-layout.kdl")
}

pub fn session_info_folder_for_session(session_name: &SessionName) -> PathBuf {
    ZELLIJ_SESSION_INFO_CACHE_DIR.join(session_name)
}

pub fn create_config_and_cache_folders() {
    if let Err(e) = std::fs::create_dir_all(ZELLIJ_CACHE_DIR.as_path()) {
        log::error!("Failed to create cache dir: {:?}", e);
    }
    if let Some(config_dir) = find_default_config_dir() {
        if let Err(e) = std::fs::create_dir_all(config_dir.as_path()) {
            log::error!("Failed to create config dir: {:?}", e);
        }
    }
    // while session_info is a child of cache currently, it won't necessarily always be this way,
    // and so it's explicitly created here
    if let Err(e) = std::fs::create_dir_all(ZELLIJ_SESSION_INFO_CACHE_DIR.as_path()) {
        log::error!("Failed to create session_info cache dir: {:?}", e);
    }
    prune_empty_session_info_folders();
}

fn prune_empty_session_info_folders() {
    let Ok(entries) = std::fs::read_dir(&*ZELLIJ_SESSION_INFO_CACHE_DIR) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let is_empty = std::fs::read_dir(&path)
            .ok()
            .is_some_and(|mut iter| iter.next().is_none());
        if is_empty {
            if let Err(e) = std::fs::remove_dir(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::debug!("Failed to prune empty session folder {:?}: {:?}", path, e);
                }
            }
        }
    }
}

const fn system_default_data_dir() -> &'static str {
    if let Some(data_dir) = std::option_env!("PREFIX") {
        data_dir
    } else {
        "/usr"
    }
}

lazy_static! {
    pub static ref CLIENT_SERVER_CONTRACT_DIR: String =
        format!("cv{}", CLIENT_SERVER_CONTRACT_VERSION);
    pub static ref ZELLIJ_PROJ_DIR: ProjectDirs =
        ProjectDirs::from("org", "Zellij Contributors", "Zellij").unwrap();
    pub static ref ZELLIJ_CACHE_DIR: PathBuf = std::env::var_os("ZELLIJ_TEST_CACHE_ROOT")
        .map(|root| PathBuf::from(root)
            .join(std::process::id().to_string())
            .join("cache"))
        .unwrap_or_else(|| ZELLIJ_PROJ_DIR.cache_dir().to_path_buf());
    pub static ref ZELLIJ_PLUGIN_PERMISSIONS_CACHE: PathBuf =
        ZELLIJ_CACHE_DIR.join("permissions.kdl");
    pub static ref ZELLIJ_SESSION_INFO_CACHE_DIR: PathBuf = ZELLIJ_CACHE_DIR
        .join(CLIENT_SERVER_CONTRACT_DIR.clone())
        .join("session_info");
    pub static ref ZELLIJ_SEEN_RELEASE_NOTES_CACHE_FILE: PathBuf =
        ZELLIJ_CACHE_DIR.join(VERSION).join("seen_release_notes");
}

#[cfg(unix)]
pub use unix_only::*;

#[cfg(unix)]
mod unix_only {
    use super::*;
    use crate::envs;
    pub use crate::shared::set_permissions;
    use lazy_static::lazy_static;
    use nix::unistd::Uid;
    use std::env::temp_dir;

    // Maximum sockaddr_un.sun_path length: 104 on macOS/BSD, 108 on Linux/Android/Solaris.
    #[cfg(target_os = "macos")]
    pub const ZELLIJ_SOCK_MAX_LENGTH: usize = 104;
    #[cfg(not(target_os = "macos"))]
    pub const ZELLIJ_SOCK_MAX_LENGTH: usize = 108;

    lazy_static! {
        static ref UID: Uid = Uid::current();
        pub static ref ZELLIJ_TMP_DIR: PathBuf = temp_dir().join(format!("zellij-{}", *UID));
        pub static ref ZELLIJ_TMP_LOG_DIR: PathBuf = ZELLIJ_TMP_DIR.join("zellij-log");
        pub static ref ZELLIJ_TMP_LOG_FILE: PathBuf = ZELLIJ_TMP_LOG_DIR.join("zellij.log");
        pub static ref ZELLIJ_SOCK_DIR: PathBuf = {
            let mut ipc_dir = envs::get_socket_dir().map_or_else(
                |_| {
                    ZELLIJ_PROJ_DIR
                        .runtime_dir()
                        .map_or_else(|| ZELLIJ_TMP_DIR.clone(), |p| p.to_owned())
                },
                PathBuf::from,
            );
            ipc_dir.push(CLIENT_SERVER_CONTRACT_DIR.clone());
            ipc_dir
        };
    }
}

#[cfg(not(unix))]
pub use not_unix::*;

#[cfg(not(unix))]
mod not_unix {
    use super::*;
    use crate::envs;
    pub use crate::shared::set_permissions;
    use lazy_static::lazy_static;
    use std::env::temp_dir;

    fn canonicalize_path(path: PathBuf) -> PathBuf {
        path
    }

    pub const ZELLIJ_SOCK_MAX_LENGTH: usize = 256;

    lazy_static! {
        pub static ref ZELLIJ_TMP_DIR: PathBuf = {
            let tmp_dir = canonicalize_path(temp_dir());
            tmp_dir.join("zellij")
        };
        pub static ref ZELLIJ_TMP_LOG_DIR: PathBuf = ZELLIJ_TMP_DIR.join("zellij-log");
        pub static ref ZELLIJ_TMP_LOG_FILE: PathBuf = ZELLIJ_TMP_LOG_DIR.join("zellij.log");
        pub static ref ZELLIJ_SOCK_DIR: PathBuf = {
            let mut ipc_dir = canonicalize_path(envs::get_socket_dir().map_or_else(
                |_| {
                    ZELLIJ_PROJ_DIR
                        .runtime_dir()
                        .map_or_else(|| ZELLIJ_TMP_DIR.clone(), |p| p.to_owned())
                },
                PathBuf::from,
            ));
            ipc_dir.push(CLIENT_SERVER_CONTRACT_DIR.clone());
            ipc_dir
        };
    }
}

pub const CONFIG_OVERLAY_DIR_NAME: &str = "config.d";

#[cfg(test)]
#[path = "unit/cache_dir_tests.rs"]
mod tests;
