//! Yinz program-wide constants.

use crate::home::find_default_config_dir;
use directories::ProjectDirs;
use lazy_static::lazy_static;
use std::{path::PathBuf, sync::OnceLock};
use yinz_types::internal_ids::SessionName;

pub const YINZ_CONFIG_FILE_ENV: &str = "YINZ_CONFIG_FILE";
pub const YINZ_CONFIG_DIR_ENV: &str = "YINZ_CONFIG_DIR";
pub const YINZ_LAYOUT_DIR_ENV: &str = "YINZ_LAYOUT_DIR";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_SCROLL_BUFFER_SIZE: usize = 10_000;
pub static SCROLL_BUFFER_SIZE: OnceLock<usize> = OnceLock::new();
pub static DEBUG_MODE: OnceLock<bool> = OnceLock::new();

pub const SYSTEM_DEFAULT_CONFIG_DIR: &str = "/etc/yinz";
pub const SYSTEM_DEFAULT_DATA_DIR_PREFIX: &str = system_default_data_dir();

/// The client/server wire contract. Local session sockets live under `cv<version>` in
/// [`YINZ_SOCK_DIR`], so bumping this splits the socket directory: builds on either side of the
/// bump can no longer list, attach to, or warp panes into each other's sessions.
///
/// Bump it only for changes an older peer cannot skip: framing, removed or renumbered protobuf
/// tags, or a changed meaning for an existing message. Gate additive messages on a capability
/// the peer advertises instead, as `CliAssets::supports_session_requests` does.
pub const CLIENT_SERVER_CONTRACT_VERSION: usize = 1;

/// Version of the SSH bootstrap: the `remote-session` and `remote-sessions` command lines and
/// their JSON replies. It is separate from the client/server contract, which stays put across
/// additive IPC changes and so cannot tell a remote whose bootstrap predates these flags from a
/// current one.
///
/// Bump it whenever either side would misread the other's bootstrap: a new required flag, a
/// changed reply, or changed startup semantics.
pub const REMOTE_BOOTSTRAP_VERSION: usize = 1;

/// Maximum protobuf frame length, excluding its four-byte length prefix. `yinz_ipc` re-exports
/// it for the socket transport; the plugin compilation worker bounds its requests and responses
/// with it without linking that transport.
pub const MAX_IPC_FRAME_LEN: usize = 64 * 1024 * 1024;

/// Embedded Scheme plugins, also used to validate builtin tags in configuration.
pub const BUILTIN_PLUGIN_NAMES: &[&str] = &[
    "compact-bar",
    "tab-bar",
    "status-bar",
    "strider",
    "configuration",
    "file-finder",
    "new-agent",
    "agent-service",
    "project-store",
    "multiple-select",
    "link",
    "context-menu",
    "agent-policy",
    "agent-opencode",
    "agent-exit",
    "quit-confirmation",
    "session-palette",
    "layout-palette",
    "plugin-palette",
    "configuration-palette",
    "shared-test",
    "repl",
];

/// The per-file source budget of a script plugin: configuration applies it when it reads one,
/// and `yinz_plugin_authoring::limits` before a source reaches the compilation worker, the
/// loader or the checker. It lives here so authoring need not link configuration.
pub const MAX_PLUGIN_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// Legacy flat metadata path. Read current metadata through `SessionCheckpoint`.
pub fn session_info_cache_file_name(session_name: &SessionName) -> PathBuf {
    session_info_folder_for_session(session_name).join("session-metadata.kdl")
}

/// Logical resurrection path passed over IPC; the server resolves its checkpoint when loading.
pub fn session_layout_cache_file_name(session_name: &SessionName) -> PathBuf {
    session_info_folder_for_session(session_name).join("session-layout.kdl")
}

pub fn session_info_folder_for_session(session_name: &SessionName) -> PathBuf {
    YINZ_SESSION_INFO_CACHE_DIR.join(session_name)
}

pub fn create_config_and_cache_folders() {
    if let Err(e) = std::fs::create_dir_all(YINZ_CACHE_DIR.as_path()) {
        log::error!("Failed to create cache dir: {:?}", e);
    }
    if let Some(config_dir) = find_default_config_dir()
        && let Err(e) = std::fs::create_dir_all(config_dir.as_path())
    {
        log::error!("Failed to create config dir: {:?}", e);
    }
    // while session_info is a child of cache currently, it won't necessarily always be this way,
    // and so it's explicitly created here
    if let Err(e) = std::fs::create_dir_all(YINZ_SESSION_INFO_CACHE_DIR.as_path()) {
        log::error!("Failed to create session_info cache dir: {:?}", e);
    }
    prune_empty_session_info_folders();
}

fn prune_empty_session_info_folders() {
    let Ok(entries) = std::fs::read_dir(&*YINZ_SESSION_INFO_CACHE_DIR) else {
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
        if is_empty
            && let Err(e) = std::fs::remove_dir(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!("Failed to prune empty session folder {:?}: {:?}", path, e);
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
    pub static ref YINZ_PROJ_DIR: ProjectDirs =
        ProjectDirs::from("org", "Yinz Contributors", "Yinz").unwrap();
    pub static ref YINZ_CACHE_DIR: PathBuf = std::env::var_os("YINZ_TEST_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(
            || std::env::var_os("YINZ_TEST_CACHE_ROOT").map(|root| PathBuf::from(root)
                .join(std::process::id().to_string())
                .join("cache"))
        )
        .unwrap_or_else(|| YINZ_PROJ_DIR.cache_dir().to_path_buf());
    pub static ref YINZ_PLUGIN_PERMISSIONS_CACHE: PathBuf = YINZ_CACHE_DIR.join("permissions.kdl");
    pub static ref YINZ_SESSION_INFO_CACHE_DIR: PathBuf = YINZ_CACHE_DIR
        .join(CLIENT_SERVER_CONTRACT_DIR.clone())
        .join("session_info");
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
    pub const YINZ_SOCK_MAX_LENGTH: usize = 104;
    #[cfg(not(target_os = "macos"))]
    pub const YINZ_SOCK_MAX_LENGTH: usize = 108;

    lazy_static! {
        static ref UID: Uid = Uid::current();
        pub static ref YINZ_TMP_DIR: PathBuf = temp_dir().join(format!("yinz-{}", *UID));
        pub static ref YINZ_TMP_LOG_DIR: PathBuf = YINZ_TMP_DIR.join("yinz-log");
        pub static ref YINZ_TMP_LOG_FILE: PathBuf = YINZ_TMP_LOG_DIR.join("yinz.log");
        /// The folder plugins see as `/tmp`. A child of `YINZ_TMP_DIR` rather than that folder
        /// itself, which also holds the server log, pane dumps and agent markers.
        pub static ref YINZ_PLUGIN_TMP_DIR: PathBuf = YINZ_TMP_DIR.join("plugin-tmp");
        pub static ref YINZ_SOCK_DIR: PathBuf = {
            let mut ipc_dir = envs::get_socket_dir().map_or_else(
                |_| {
                    let dirs = directories::BaseDirs::new().expect(
                        "Could not determine the home directory for Yinz sockets; set YINZ_SOCKET_DIR",
                    );
                    // Keep the fallback short enough for Unix sockets, including on macOS.
                    dirs
                        .runtime_dir()
                        .map_or_else(|| dirs.home_dir().join(".yinz/run"), |p| p.join("yinz"))
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

    pub const YINZ_SOCK_MAX_LENGTH: usize = 256;

    lazy_static! {
        pub static ref YINZ_TMP_DIR: PathBuf = {
            let tmp_dir = canonicalize_path(temp_dir());
            tmp_dir.join("yinz")
        };
        pub static ref YINZ_TMP_LOG_DIR: PathBuf = YINZ_TMP_DIR.join("yinz-log");
        pub static ref YINZ_TMP_LOG_FILE: PathBuf = YINZ_TMP_LOG_DIR.join("yinz.log");
        /// The folder plugins see as `/tmp`. A child of `YINZ_TMP_DIR` rather than that folder
        /// itself, which also holds the server log, pane dumps and agent markers.
        pub static ref YINZ_PLUGIN_TMP_DIR: PathBuf = YINZ_TMP_DIR.join("plugin-tmp");
        pub static ref YINZ_SOCK_DIR: PathBuf = {
            let mut ipc_dir = canonicalize_path(envs::get_socket_dir().map_or_else(
                |_| {
                    YINZ_PROJ_DIR
                        .runtime_dir()
                        .map_or_else(|| YINZ_TMP_DIR.clone(), |p| p.to_owned())
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

#[cfg(all(test, unix))]
#[path = "unit/socket_dir_tests.rs"]
mod socket_dir_tests;
