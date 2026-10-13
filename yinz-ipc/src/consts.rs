/// Persistent claims are kept separate from discoverable session sockets.
pub const SESSION_OWNERSHIP_DIR: &str = ".session-locks";

#[cfg(unix)]
pub fn is_ipc_socket(file_type: &std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    file_type.is_socket()
}

#[cfg(not(unix))]
pub fn is_ipc_socket(file_type: &std::fs::FileType) -> bool {
    file_type.is_file()
}

/// The name prefix of the pty broker's socket, `pty-broker-v<N>.sock`.
///
/// The broker listens in the session socket directory but is not a session. It
/// carries panes across server restarts, so a sweep that probed it as a session
/// would list it as unresponsive and invite `kill-session` on it.
pub const PTY_BROKER_SOCKET_PREFIX: &str = "pty-broker-v";

/// The pty broker's socket name for a protocol version.
pub fn pty_broker_socket_name(protocol_version: impl std::fmt::Display) -> String {
    format!("{PTY_BROKER_SOCKET_PREFIX}{protocol_version}.sock")
}

/// Whether a file name in the session socket directory belongs to a session.
pub fn is_session_socket_name(name: &str) -> bool {
    !(name.starts_with(PTY_BROKER_SOCKET_PREFIX) && name.ends_with(".sock"))
}

/// How long to keep trying to connect to a session socket.
///
/// A server whose listen backlog is full accepts no new connection and refuses
/// none either: a blocking `connect` to it never returns, and a non-blocking
/// one reports `EAGAIN` for as long as the condition lasts. That is how a
/// single wedged session hung `yinz`, `yinz ls`, `yinz attach` and even
/// `yinz kill-session` — for every unrelated session on the machine, since
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

/// Whether this kernel refuses a connection outright when the listener's backlog
/// is full.
///
/// Linux queues the connect and answers `EAGAIN`, which says exactly "the
/// backlog is full" and nothing else. The BSDs return `ECONNREFUSED` — the same
/// errno they give for a socket file whose server is gone — so on those the
/// answer carries no information on its own and can only be retried, never
/// believed. Left `false` on Linux so nothing there changes.
#[cfg(unix)]
pub(crate) const A_FULL_BACKLOG_IS_REFUSED: bool = cfg!(any(
    target_vendor = "apple",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
));

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
///   forked while it is open — a `yinz run` child, a plugin's command — which
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
            // A full backlog, reported the way the BSDs report it. Indistinguishable
            // at the errno level from a socket whose server is gone, so it is
            // retried rather than trusted: a server merely behind on `accept`
            // takes the connection within the deadline, and one that is really
            // gone goes on refusing and is reported as refused below — which is
            // what lets the next owner reclaim its stale socket. Deliberately *not*
            // rewritten as `TimedOut` on the way out: that would make a stale
            // socket read as an unresponsive server, and `yinz -s <name>`
            // refuses to build over one of those, so a crashed session's name
            // would be unusable until someone deleted the file by hand.
            std::io::ErrorKind::ConnectionRefused
                if A_FULL_BACKLOG_IS_REFUSED && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(IPC_CONNECT_RETRY_INTERVAL);
            },
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
