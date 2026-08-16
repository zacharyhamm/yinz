//! The remote end: a small daemon that turns a pty master descriptor into a
//! byte stream, and back.
//!
//! It is deliberately not a new kind of supervisor. It does exactly what a
//! local server does — `pty_host::client::spawn`, then `recv_fd` — so the
//! supervisor on the far side cannot tell it apart from one, and every existing
//! guarantee about pane lifetime keeps holding. All the broker adds is the one
//! thing ssh cannot carry: it owns the descriptor locally and relays it.
//!
//! One connection per pane. ssh opens a channel per connection to a forwarded
//! socket, so this needs no multiplexing of its own; the accept loop is the
//! whole of it.

use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

use zellij_utils::{
    consts::{VERSION, ZELLIJ_SOCK_DIR},
    errors::prelude::*,
};

use super::protocol::{
    self, BrokerToServerMsg, ChildSignal, Frame, ServerToBrokerMsg, MAX_STREAM_PAYLOAD,
    REMOTE_PROTOCOL_VERSION,
};
use crate::pty_host::client::{self as pty_host_client, SpawnParams};
use crate::pty_host::protocol::{check_socket_path_length, SpawnFailureKind, WireWinsize};

/// A socket shared by the several things that need to write to it: the output
/// pump, the child-exit callback, and (from Phase 3) the status ticker.
type Wire = Arc<Mutex<UnixStream>>;

fn send(wire: &Wire, msg: &BrokerToServerMsg) -> Result<()> {
    let mut sock = wire
        .lock()
        .to_anyhow()
        .context("failed to lock a remote pane's wire")?;
    protocol::write_control(&mut *sock, msg)
}

fn send_output(wire: &Wire, bytes: &[u8]) -> Result<()> {
    let mut sock = wire
        .lock()
        .to_anyhow()
        .context("failed to lock a remote pane's wire")?;
    protocol::write_output(&mut *sock, bytes)
}

/// Serve one pane on an accepted connection.
///
/// Returns when the connection or the child is done; the caller drops the
/// supervisor handle, which terminates the pane.
pub fn serve_connection(socket: UnixStream, host_bin: &std::path::Path) -> Result<()> {
    socket
        .set_nonblocking(false)
        .context("failed to put a broker connection into blocking mode")?;
    let mut reader = socket
        .try_clone()
        .context("failed to clone a broker connection")?;
    let wire: Wire = Arc::new(Mutex::new(socket));

    // --- handshake ---
    let hello =
        read_control(&mut reader)?.ok_or_else(|| anyhow!("peer closed before saying hello"))?;
    match hello {
        ServerToBrokerMsg::Hello {
            protocol_version, ..
        } if protocol_version == REMOTE_PROTOCOL_VERSION => {},
        ServerToBrokerMsg::Hello {
            protocol_version,
            zellij_version,
        } => {
            // Name both sides. A remote box on a different build is the normal
            // case, and "connection refused" would send the user hunting the
            // network rather than the version.
            return Err(anyhow!(
                "remote pane protocol mismatch: this broker speaks v{REMOTE_PROTOCOL_VERSION} \
                 (zellij {VERSION}), the server speaks v{protocol_version} (zellij \
                 {zellij_version}). Install matching zellij versions on both machines."
            ));
        },
        other => return Err(anyhow!("expected a hello, got {other:?}")),
    }
    send(
        &wire,
        &BrokerToServerMsg::HelloAck {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
        },
    )?;

    // --- spawn ---
    let (cmd, failover_cmd, initial_size) = match read_control(&mut reader)?
        .ok_or_else(|| anyhow!("peer closed before asking for a pane"))?
    {
        ServerToBrokerMsg::Spawn {
            cmd,
            failover_cmd,
            initial_size,
        } => (cmd, failover_cmd, initial_size),
        other => return Err(anyhow!("expected a spawn, got {other:?}")),
    };

    let pending = match pty_host_client::spawn(SpawnParams {
        host_bin,
        terminal_id: 0,
        session_name: None,
        cmd: cmd.clone(),
        failover_cmd,
        // Never sent across a machine boundary — the flag words and control
        // character indices are platform constants. See the note on `Spawn`.
        termios: None,
        initial_size,
    }) {
        Ok(pending) => pending,
        Err(e) => {
            // `CommandNotFound` has to survive the trip: eight sites in pty.rs
            // downcast for it, and the local pre-flight `command_exists` check
            // that normally produces it is meaningless for a command that lives
            // on *this* machine. Nothing is lost — the supervisor re-checks
            // before forking and reports `SpawnFailureKind::CommandNotFound`,
            // which `client::spawn` turns back into this error. The check simply
            // happens on the machine that has the command.
            let kind = match e.downcast_ref::<ZellijError>() {
                Some(ZellijError::CommandNotFound { .. }) => SpawnFailureKind::CommandNotFound,
                _ => SpawnFailureKind::Other,
            };
            send(
                &wire,
                &BrokerToServerMsg::SpawnFailed {
                    kind,
                    message: format!("{e:#}"),
                },
            )?;
            return Err(e);
        },
    };

    // The supervisor reports the child's exit through this callback; forward it
    // as the one message that means the pane is over.
    let exit_wire = wire.clone();
    let spawned = pending.finish(Box::new(move |_pane_id, status, _cmd| {
        send(&exit_wire, &BrokerToServerMsg::ChildExited { status })
            .context("failed to report a remote child's exit")
            .non_fatal();
    }));

    let master_fd = spawned.master.as_raw_fd();
    let handle = spawned.handle;
    let child_pid = spawned.child_pid;
    send(&wire, &BrokerToServerMsg::Spawned)?;

    // --- pump ---
    // Output on its own thread, control and input on this one. Two blocking
    // readers is simpler than one poll loop and, with a connection per pane,
    // costs nothing worth saving.
    // `master` stays owned here, and both pumps borrow the raw descriptor. If
    // the output thread owned it, its exit would close the descriptor while
    // `pump_control` might still be writing to that number — and by then the
    // number could belong to another pane's pty.
    let master = spawned.master;
    let output_wire = wire.clone();
    let output = thread::Builder::new()
        .name("remote-pane-output".to_owned())
        .spawn(move || {
            let mut master_read = borrow_fd(master_fd);
            let mut buf = [0u8; MAX_STREAM_PAYLOAD];
            loop {
                match master_read.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if send_output(&output_wire, &buf[..n]).is_err() {
                            break;
                        }
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .context("failed to start a remote pane's output pump")?;

    let result = pump_control(&mut reader, master_fd, child_pid);
    let mut handle = handle;
    match &result {
        // An explicit terminate is the *only* thing that ends a supervisor —
        // dropping the handle deliberately does not. Without this call every
        // closed remote pane would leave its supervisor and its child running
        // on this machine forever.
        Ok(Outcome::Terminated) => handle.terminate(),
        // A connection that merely ended is not a teardown. The supervisor
        // holding the pane is exactly what lets the pane be picked back up, so
        // it is left running on purpose; `zellij pty-host list`/`kill` on this
        // machine can reclaim one that is never reconnected to.
        Ok(Outcome::Disconnected) | Err(_) => {
            log::info!(
                "a remote pane's connection ended without a terminate; leaving its \
                 supervisor running so the pane can be picked back up"
            );
        },
    }
    let _ = output.join();
    drop(master);
    result.map(|_| ())
}

/// A `File` view of a descriptor owned by someone else. Never closes it.
fn borrow_fd(fd: std::os::unix::io::RawFd) -> std::mem::ManuallyDrop<std::fs::File> {
    use std::os::unix::io::FromRawFd;
    std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn read_control(reader: &mut UnixStream) -> Result<Option<ServerToBrokerMsg>> {
    match protocol::read_frame(reader)? {
        None => Ok(None),
        Some(frame @ Frame::Control(_)) => frame.decode_control().map(Some),
        Some(other) => Err(anyhow!("expected a control frame, got {other:?}")),
    }
}

/// Why the control pump stopped, which decides the supervisor's fate.
enum Outcome {
    /// The server asked for the pane to end.
    Terminated,
    /// The connection went away without saying why.
    Disconnected,
}

fn pump_control(
    reader: &mut UnixStream,
    master_fd: std::os::unix::io::RawFd,
    child_pid: u32,
) -> Result<Outcome> {
    let mut master_write = borrow_fd(master_fd);
    loop {
        let frame = match protocol::read_frame(reader)? {
            Some(frame) => frame,
            None => return Ok(Outcome::Disconnected),
        };
        match frame {
            Frame::Input(bytes) => {
                // Blocking on purpose: if the pty is full, not reading the
                // socket is precisely the backpressure the far side needs. ssh's
                // window closes, and its outbound queue starts short-counting.
                master_write
                    .write_all(&bytes)
                    .context("failed to write input to a remote pane")?;
            },
            Frame::Output(_) => {
                return Err(anyhow!("a server sent an output frame to a broker"));
            },
            Frame::Control(_) => {
                let msg: ServerToBrokerMsg = frame.decode_control()?;
                match msg {
                    ServerToBrokerMsg::Resize(WireWinsize {
                        cols,
                        rows,
                        width_in_pixels,
                        height_in_pixels,
                    }) => {
                        set_winsize(master_fd, cols, rows, width_in_pixels, height_in_pixels);
                    },
                    ServerToBrokerMsg::SignalChild(signal) => {
                        signal_child(child_pid, signal);
                    },
                    ServerToBrokerMsg::Terminate => return Ok(Outcome::Terminated),
                    // A second hello or spawn on a live pane means the peer is
                    // confused, but the pane is fine; say so and carry on.
                    other => {
                        log::warn!("unexpected message on a live remote pane: {other:?}");
                    },
                }
            },
        }
    }
}

fn set_winsize(fd: std::os::unix::io::RawFd, cols: u16, rows: u16, xpixel: u16, ypixel: u16) {
    let winsize = nix::pty::Winsize {
        ws_col: cols,
        ws_row: rows,
        ws_xpixel: xpixel,
        ws_ypixel: ypixel,
    };
    #[allow(clippy::useless_conversion)]
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ.into(), &winsize)
    };
}

/// Resolve a symbolic signal against the child this broker owns.
///
/// The pid never crosses the wire in either direction, which is the point: a pid
/// from the server's machine would name an unrelated process here, and this
/// child's pid would name an unrelated process there.
fn signal_child(child_pid: u32, signal: ChildSignal) {
    let pid = nix::unistd::Pid::from_raw(child_pid as i32);
    let sig = match signal {
        ChildSignal::Hup => nix::sys::signal::Signal::SIGHUP,
        ChildSignal::Int => nix::sys::signal::Signal::SIGINT,
        ChildSignal::Kill => nix::sys::signal::Signal::SIGKILL,
    };
    let _ = nix::sys::signal::kill(pid, Some(sig));
}

/// The argv flag that turns this binary into a broker, mirroring
/// `PTY_HOST_FLAG`. Recognised only as argv\[1\], so `main` can branch before
/// clap and the logger.
pub const PTY_BROKER_FLAG: &str = "--pty-broker";

pub fn broker_invocation() -> Option<String> {
    broker_invocation_from_args(std::env::args())
}

fn broker_invocation_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter();
    let _bin = args.next();
    if args.next().as_deref() != Some(PTY_BROKER_FLAG) {
        return None;
    }
    Some(args.next().unwrap_or_default())
}

/// Where this machine's broker listens.
///
/// Computed here and never on the connecting side: `ZELLIJ_SOCK_DIR` resolves
/// differently per platform, and the connecting machine cannot know which
/// applies. `ensure` prints this back over ssh for exactly that reason.
pub fn broker_socket_path() -> std::path::PathBuf {
    ZELLIJ_SOCK_DIR.join("pty-broker.sock")
}

/// `zellij --pty-broker ensure` — make sure a broker is up and say where.
///
/// Prints one JSON line and exits. Output is the contract with `ssh::bootstrap`,
/// so it goes to stdout and nothing else may.
pub fn ensure() -> Result<()> {
    let socket = broker_socket_path();
    check_socket_path_length(&socket)?;

    if UnixStream::connect(&socket).is_err() {
        // Either nothing is listening or the file is a leftover from a crash.
        let _ = std::fs::remove_file(&socket);
        spawn_detached_broker(&socket)?;
        await_socket(&socket)?;
    }

    println!(
        "{}",
        serde_json::json!({
            "socket": socket.to_string_lossy(),
            "protocol_version": REMOTE_PROTOCOL_VERSION,
            "zellij_version": VERSION,
        })
    );
    Ok(())
}

fn spawn_detached_broker(socket: &std::path::Path) -> Result<()> {
    let exe = std::env::current_exe().context("could not resolve our own binary")?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(PTY_BROKER_FLAG)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        // Its own session, so it outlives the ssh that started it. Without this
        // the broker — and every pane on this machine — would die with the
        // bootstrap connection.
        command.pre_exec(|| {
            let _ = nix::unistd::setsid();
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("failed to start a broker listening on {}", socket.display()))?;
    Ok(())
}

fn await_socket(socket: &std::path::Path) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(anyhow!(
        "a broker was started but never began listening on {}",
        socket.display()
    ))
}

/// `zellij --pty-broker serve` — the daemon itself.
pub fn serve_forever() -> Result<()> {
    let socket = broker_socket_path();
    check_socket_path_length(&socket)?;
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Owner only: this socket starts processes as us.
        let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
    }
    let host_bin = std::env::current_exe().context("could not resolve our own binary")?;
    serve(listener, host_bin)
}

/// Accept panes until the listener dies.
pub fn serve(listener: UnixListener, host_bin: std::path::PathBuf) -> Result<()> {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("broker accept failed: {e}");
                continue;
            },
        };
        let host_bin = host_bin.clone();
        thread::Builder::new()
            .name("remote-pane".to_owned())
            .spawn(move || {
                if let Err(e) = serve_connection(stream, &host_bin) {
                    log::warn!("remote pane ended: {e:#}");
                }
            })
            .context("failed to start a remote pane thread")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_broker_tests.rs"]
mod pty_host_remote_broker_tests;
