//! Zellij program-wide constants.

use crate::home::find_default_config_dir;
use directories::ProjectDirs;
use include_dir::{include_dir, Dir};
use lazy_static::lazy_static;
use std::{path::PathBuf, sync::OnceLock};
use uuid::Uuid;

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

pub static ZELLIJ_DEFAULT_THEMES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/themes");

pub const CLIENT_SERVER_CONTRACT_VERSION: usize = 1;

/// Legacy flat metadata path. Read current metadata through `SessionCheckpoint`.
pub fn session_info_cache_file_name(session_name: &str) -> PathBuf {
    session_info_folder_for_session(session_name).join("session-metadata.kdl")
}

/// Logical resurrection path passed over IPC; the server resolves its checkpoint when loading.
pub fn session_layout_cache_file_name(session_name: &str) -> PathBuf {
    session_info_folder_for_session(session_name).join("session-layout.kdl")
}

pub fn session_info_folder_for_session(session_name: &str) -> PathBuf {
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
    pub static ref ZELLIJ_CACHE_DIR: PathBuf = ZELLIJ_PROJ_DIR.cache_dir().to_path_buf();
    pub static ref ZELLIJ_SESSION_CACHE_DIR: PathBuf = ZELLIJ_PROJ_DIR
        .cache_dir()
        .to_path_buf()
        .join(format!("{}", Uuid::new_v4()));
    pub static ref ZELLIJ_PLUGIN_PERMISSIONS_CACHE: PathBuf =
        ZELLIJ_CACHE_DIR.join("permissions.kdl");
    pub static ref ZELLIJ_SESSION_INFO_CACHE_DIR: PathBuf = ZELLIJ_CACHE_DIR
        .join(CLIENT_SERVER_CONTRACT_DIR.clone())
        .join("session_info");
    pub static ref ZELLIJ_SEEN_RELEASE_NOTES_CACHE_FILE: PathBuf =
        ZELLIJ_CACHE_DIR.join(VERSION).join("seen_release_notes");
}

pub const FEATURES: &[&str] = &[];

pub const BUILTIN_PLUGIN_NAMES: &[&str] = &[
    "compact-bar",
    "status-bar",
    "tab-bar",
    "strider",
    "session-manager",
    "configuration",
    "plugin-manager",
    "about",
    "multiple-select",
    "layout-manager",
    "link",
    "file-finder",
    "quit-confirmation",
    "shared-test",
    "hocus",
    "hocus-agentd",
    "hocus-agents",
    "hocus-new-agent",
    "hocus-store",
];

/// Builtin plugins written in Lua, as (name, source).
///
/// A wasm builtin is found by name on disk (see `resolve_wasm_bytes`); a Lua plugin has
/// no build step and no bytes to look up, so its source is simply compiled in here. Being in
/// this table is what makes `zellij:<name>` resolve to Lua - see `PluginConfig::runtime`.
///
/// Kept separate from `BUILTIN_PLUGIN_NAMES` rather than merged into it because everything
/// that reads that list is looking for something to load wasm for.
pub const BUILTIN_LUA_PLUGINS: &[(&str, &str)] = &[
    (
        "agent-exit",
        include_str!("../assets/plugins/agent-exit.lua"),
    ),
    (
        "context-menu",
        include_str!("../assets/plugins/context-menu.lua"),
    ),
    (
        "agent-policy",
        include_str!("../assets/plugins/agent-policy.lua"),
    ),
];

/// The source of a builtin Lua plugin, if `name` names one.
pub fn builtin_lua_plugin(name: &str) -> Option<&'static str> {
    BUILTIN_LUA_PLUGINS
        .iter()
        .find(|(plugin_name, _)| *plugin_name == name)
        .map(|(_, source)| *source)
}

/// The checkout's freshly built debug plugins, baked in at compile time from this crate's
/// manifest dir. Searched *before* any installed copy: a `cargo x run` or `cargo test` on the
/// same `VERSION` as an installed zellij must see the plugins it just built, not last week's
/// install. Only debug binaries look here; on an installed system the folder does not exist
/// and is skipped.
#[cfg(all(not(target_family = "wasm"), debug_assertions))]
pub const BUILTIN_PLUGIN_DEBUG_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../target/wasm32-wasip1/debug");

/// The checkout's release plugins, the last resort for a binary run straight out of `target/`
/// without a `cargo x install`. Baked in at compile time like the debug dir, and skipped in
/// the same way where it does not exist. Note that a same-`VERSION` install is preferred over
/// this folder - see `PluginConfig::resolve_wasm_bytes` for the whole order.
#[cfg(not(target_family = "wasm"))]
pub const BUILTIN_PLUGIN_RELEASE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/wasm32-wasip1/release"
);

#[cfg(unix)]
pub fn is_ipc_socket(file_type: &std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    file_type.is_socket()
}

#[cfg(not(unix))]
pub fn is_ipc_socket(file_type: &std::fs::FileType) -> bool {
    file_type.is_file()
}

/// How long to keep trying to connect to a session socket.
///
/// A server whose listen backlog is full accepts no new connection and refuses
/// none either: a blocking `connect` to it never returns, and a non-blocking
/// one reports `EAGAIN` for as long as the condition lasts. That is how a
/// single wedged session hung `zellij`, `zellij ls`, `zellij attach` and even
/// `zellij kill-session` — for every unrelated session on the machine, since
/// all of them sweep the socket directory. One server observed here had leaked
/// 65,538 connections and 131,085 threads over three days, and its backlog had
/// been full ever since.
///
/// Long enough that a healthy server briefly behind on `accept` — a burst of
/// hook clients, say — is waited for rather than declared wedged, and short
/// enough that a genuinely wedged one costs a person half a second once.
pub const IPC_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// How long to wait between connect attempts while the backlog is full.
#[cfg(unix)]
const IPC_CONNECT_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Fill in a `sockaddr_un` for `path`.
#[cfg(unix)]
pub(crate) fn unix_socket_address(
    path: &std::path::Path,
) -> std::io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // Safety: `sockaddr_un` is a plain C struct, valid all-zero.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    // One byte reserved for the terminating nul the zeroing already provides.
    if bytes.len() >= addr.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "socket path is too long for a unix socket: {}",
                path.display()
            ),
        ));
    }
    for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let addr_len =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    #[cfg(any(
        target_vendor = "apple",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    {
        addr.sun_len = addr_len as u8;
    }
    Ok((addr, addr_len))
}

/// Connect to an IPC socket, giving up after `IPC_CONNECT_TIMEOUT` if the peer
/// never accepts.
///
/// Done with raw syscalls rather than through `interprocess`, which cannot
/// express this case. Its `ConnectWaitMode::Timeout` connects non-blocking and
/// then polls for `POLLOUT` — but a unix socket that answered `EAGAIN` was
/// never queued at all, and polling an unconnected one returns `POLLOUT` (with
/// `POLLHUP`, and `SO_ERROR` clear) straight away. So it reports success and
/// hands back a stream that is not connected to anything, which is a worse
/// failure than the hang: it moves the error to whoever writes to it first.
///
/// `EAGAIN` here means precisely "the backlog is full", and for a unix socket
/// retrying it on the same descriptor is fine — nothing is queued to conflict
/// with. So the retry loop below waits out a server that is briefly behind on
/// `accept`, and gives up on one that is never coming back.
///
/// The deadline lives here rather than at the probe call site so that *every*
/// connect is bounded, `kill_session` included — the remedy a person is told to
/// run for a wedged session must not itself hang on it. The probe's own
/// `PROBE_TIMEOUT` covers only the reply, because it is applied to the stream
/// this function returns: it can do nothing about a connect that never returns
/// one.
/// A non-blocking `AF_UNIX` stream socket with the descriptor flags std would
/// have given it.
///
/// `std::os::unix::net::UnixStream` cannot connect non-blocking, which is the
/// one thing `ipc_connect` needs, so the socket has to be made by hand — but
/// "by hand" must not mean "with fewer guarantees". Two matter here:
///
/// - **close-on-exec.** std sets it on every socket it creates. Without it,
///   every IPC connection a client holds open is inherited by any process
///   forked while it is open — a `zellij run` child, a plugin's command — which
///   keeps the connection alive after the client is gone, and is precisely the
///   sort of leak the server's connection cap exists to survive.
/// - **portability.** `SOCK_NONBLOCK` and `SOCK_CLOEXEC` are not part of
///   `socket(2)` on apple targets; there the flags are set with `fcntl` after
///   the fact, exactly as std does. Where the kernel does take them, they are
///   passed atomically so that no fork can slip in between `socket` and
///   `fcntl`.
///
/// Also as in std: `SO_NOSIGPIPE` on apple, so that writing to a server that
/// has gone away comes back as `EPIPE` rather than killing the process.
#[cfg(unix)]
pub(crate) fn nonblocking_unix_stream_socket() -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd, OwnedFd};

    // The cfg list is std's own (`library/std/src/sys/pal/unix/net.rs`).
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "solaris",
    ))]
    let socket_type = libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    #[cfg(not(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "solaris",
    )))]
    let socket_type = libc::SOCK_STREAM;

    // Safety: a socket(2) call with constant arguments; the descriptor it
    // returns is handed to `OwnedFd` before anything can go wrong with it.
    let raw = unsafe { libc::socket(libc::AF_UNIX, socket_type, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Safety: `raw` is a fresh descriptor this function just created and has
    // not shared; `OwnedFd` becomes its sole owner and will close it, including
    // on every early return below.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    #[cfg(not(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "solaris",
    )))]
    {
        use std::os::fd::AsRawFd;
        // Safety for the block: plain fcntl/setsockopt calls on a live
        // descriptor this function owns.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[cfg(target_vendor = "apple")]
        {
            let one: libc::c_int = 1;
            if unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_NOSIGPIPE,
                    &one as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }

    Ok(fd)
}

#[cfg(unix)]
pub fn ipc_connect(path: &std::path::Path) -> std::io::Result<interprocess::local_socket::Stream> {
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream;

    let (addr, addr_len) = unix_socket_address(path)?;
    let fd = nonblocking_unix_stream_socket()?;

    let deadline = std::time::Instant::now() + IPC_CONNECT_TIMEOUT;
    loop {
        // Safety: `fd` is a live unix stream socket and `addr` is a
        // `sockaddr_un` of the length given alongside it.
        let rc = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                &addr as *const _ as *const libc::sockaddr,
                addr_len,
            )
        };
        if rc == 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        match err.kind() {
            std::io::ErrorKind::Interrupted => continue,
            std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    // Deliberately surfaced as `TimedOut` rather than
                    // `WouldBlock`: to a caller this is a server that did not
                    // answer in time, and `sessions::classify_probe_connect_error`
                    // must read it as unresponsive — never as absent, which is
                    // the answer that would let a new server unlink a live
                    // server's socket.
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "the server at {} did not accept the connection within \
                             {IPC_CONNECT_TIMEOUT:?}; its backlog is full",
                            path.display()
                        ),
                    ));
                }
                std::thread::sleep(IPC_CONNECT_RETRY_INTERVAL);
            },
            _ => return Err(err),
        }
    }

    // Every caller expects blocking reads and writes; the socket was only made
    // non-blocking so that the connect above could be bounded.
    // Safety: `fd` is a live descriptor owned here.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Safety: as above; clearing the flag this function itself set.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Safety: `fd` is a connected unix stream socket, and ownership of the
    // descriptor moves to the `UnixStream` here.
    let stream = unsafe { UnixStream::from_raw_fd(fd.into_raw_fd()) };
    Ok(interprocess::os::unix::uds_local_socket::Stream::from(stream).into())
}

#[cfg(unix)]
pub fn ipc_bind(path: &std::path::Path) -> std::io::Result<interprocess::local_socket::Listener> {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    let fs_name = path.to_fs_name::<GenericFilePath>()?;
    ListenerOptions::new().name(fs_name).create_sync()
}

#[cfg(unix)]
pub fn ipc_bind_async(
    path: &std::path::Path,
) -> std::io::Result<interprocess::local_socket::tokio::Listener> {
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    let fs_name = path.to_fs_name::<GenericFilePath>()?;
    ListenerOptions::new().name(fs_name).create_tokio()
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
