use crate::os_input_output::{command_exists, AsyncReader};
use crate::panes::PaneId;

use nix::{
    fcntl::{fcntl, FcntlArg, OFlag},
    pty::{openpty, OpenptyResult, Winsize},
    sys::{
        signal::{kill, Signal},
        termios,
        wait::{waitpid, WaitPidFlag, WaitStatus},
    },
    unistd,
};
use tokio::io::unix::AsyncFd;

use libc::{self, ioctl, TIOCSWINSZ};
use signal_hook::consts::*;

use std::{
    collections::BTreeMap,
    fs::File,
    io,
    os::fd::{BorrowedFd, FromRawFd, IntoRawFd},
    os::unix::{
        io::{AsRawFd, RawFd},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use zellij_utils::{errors::prelude::*, input::command::RunCommand};

use crate::pty_host::remote::{
    conn::RemoteGitProbe, protocol::ChildSignal, RemoteConn, RemoteStatus,
};
use crate::pty_host::{
    client::{
        self as pty_host_client, ChildRunState, ChildStateCb, PtyHostHandle, ReleasedPane,
        ReleasedRemotePane, SpawnParams,
    },
    protocol::{WireTermios, WireWinsize},
};

pub use async_trait::async_trait;

/// The longest one round trip to a broker may take, whatever budget is left.
///
/// Shorter than `warp::WARP_TIMEOUT`, and deliberately: this is one leg of a
/// warp that the pty thread is blocked on, and the leg after it — the far
/// session attaching — needs budget left over. A broker that has not answered in
/// this long is not about to.
///
/// A ceiling only. What a release actually gets is the smaller of this and what
/// remains of the caller's deadline, which is the number that matters when a
/// tab is releasing several panes at once — see
/// `Self::release_remote_terminal_for_warp`.
const REMOTE_RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const REMOTE_TERMINATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// An `AsyncReader` that wraps a `RawFd` using epoll via `AsyncFd`.
///
/// Construction sets O_NONBLOCK but defers `AsyncFd` registration to the first
/// `read()` call, because `AsyncFd::new()` requires a live Tokio reactor and
/// `spawn_terminal` runs on the plain PTY thread (outside the runtime).
struct RawFdAsyncReader {
    /// Holds the file before reactor registration; `None` after promotion.
    pending: Option<File>,
    /// Populated on first `read()` inside the Tokio runtime.
    async_fd: Option<AsyncFd<File>>,
}

impl RawFdAsyncReader {
    fn new(fd: RawFd) -> io::Result<Self> {
        // Set O_NONBLOCK so AsyncFd can use epoll correctly
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        let flags = fcntl(borrowed_fd, FcntlArg::F_GETFL)
            .map_err(|e| io::Error::from_raw_os_error(e as i32))?;
        let mut oflags = OFlag::from_bits_truncate(flags);
        oflags.insert(OFlag::O_NONBLOCK);
        fcntl(borrowed_fd, FcntlArg::F_SETFL(oflags))
            .map_err(|e| io::Error::from_raw_os_error(e as i32))?;

        let file = unsafe { File::from_raw_fd(fd) };
        Ok(Self {
            pending: Some(file),
            async_fd: None,
        })
    }

    /// Lazily register with the Tokio reactor on first use.
    fn get_async_fd(&mut self) -> io::Result<&mut AsyncFd<File>> {
        if self.async_fd.is_none() {
            let file = self
                .pending
                .take()
                .expect("RawFdAsyncReader used after init");
            self.async_fd = Some(AsyncFd::new(file)?);
        }
        Ok(self.async_fd.as_mut().unwrap())
    }
}

#[async_trait]
impl AsyncReader for RawFdAsyncReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let async_fd = self.get_async_fd()?;
        loop {
            let mut guard = async_fd.readable().await?;
            match guard.try_io(|inner| {
                let fd = inner.get_ref().as_raw_fd();
                let ret =
                    unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if ret < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(ret as usize)
                }
            }) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

fn set_terminal_size_using_fd(
    fd: RawFd,
    columns: u16,
    rows: u16,
    width_in_pixels: Option<u16>,
    height_in_pixels: Option<u16>,
) {
    // TODO: do this with the nix ioctl
    let ws_xpixel = width_in_pixels.unwrap_or(0);
    let ws_ypixel = height_in_pixels.unwrap_or(0);
    let winsize = Winsize {
        ws_col: columns,
        ws_row: rows,
        ws_xpixel,
        ws_ypixel,
    };
    // TIOCGWINSZ is an u32, but the second argument to ioctl is u64 on
    // some platforms. When checked on Linux, clippy will complain about
    // useless conversion.
    #[allow(clippy::useless_conversion)]
    unsafe {
        ioctl(fd, TIOCSWINSZ.into(), &winsize)
    };
}

/// Handle some signals for the child process. This will loop until the child
/// process exits.
fn handle_command_exit(mut child: Child) -> Result<Option<i32>> {
    let id = child.id();
    let err_context = || {
        format!(
            "failed to handle signals and command exit for child process pid {}",
            id
        )
    };

    // returns the exit status, if any
    let mut should_exit = false;
    let mut attempts = 3;
    let mut signals =
        signal_hook::iterator::Signals::new([SIGINT, SIGTERM]).with_context(err_context)?;
    'handle_exit: loop {
        // test whether the child process has exited
        match child.try_wait() {
            Ok(Some(status)) => {
                // if the child process has exited, break outside of the loop
                // and exit this function
                // TODO: handle errors?
                break 'handle_exit Ok(status.code());
            },
            Ok(None) => {
                thread::sleep(Duration::from_millis(10));
            },
            Err(e) => panic!("error attempting to wait: {}", e),
        }

        if !should_exit {
            for signal in signals.pending() {
                if signal == SIGINT || signal == SIGTERM {
                    should_exit = true;
                }
            }
        } else if attempts > 0 {
            // let's try nicely first...
            attempts -= 1;
            kill(
                unistd::Pid::from_raw(child.id() as i32),
                Some(Signal::SIGTERM),
            )
            .with_context(err_context)?;
            continue;
        } else {
            // when I say whoa, I mean WHOA!
            let _ = child.kill();
            break 'handle_exit Ok(None);
        }
    }
}

/// Block until the child exits, reporting every stop and continue on the way.
///
/// `nix`'s `waitpid` reaps, so the caller's `Child` must not be waited on again;
/// `handle_command_exit`'s `try_wait` will simply see `ECHILD` and fall through.
fn wait_reporting_stops(
    child_pid: u32,
    terminal_id: u32,
    child_state_cb: Option<&ChildStateCb>,
) -> Option<i32> {
    let pid = unistd::Pid::from_raw(child_pid as i32);
    let flags = WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED;
    loop {
        let state = match waitpid(pid, Some(flags)) {
            Ok(WaitStatus::Exited(_, code)) => return Some(code),
            Ok(WaitStatus::Signaled(_, _, _)) => return None,
            Ok(WaitStatus::Stopped(_, _)) => ChildRunState::Stopped,
            Ok(WaitStatus::Continued(_)) => ChildRunState::Continued,
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            // Nothing left to wait for. Returning is the only safe answer: a
            // waiter that spun here would leave the pane open forever.
            Err(_) => return None,
        };
        if let Some(cb) = child_state_cb {
            cb(terminal_id, state);
        }
    }
}

fn handle_openpty(
    open_pty_res: OpenptyResult,
    cmd: RunCommand,
    quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    child_state_cb: Option<ChildStateCb>,
    terminal_id: u32,
) -> Result<(RawFd, RawFd)> {
    let err_context = |cmd: &RunCommand| {
        format!(
            "failed to open PTY for command '{}'",
            cmd.command.to_string_lossy()
        )
    };

    // primary side of pty and child fd
    let pid_primary = open_pty_res.master.into_raw_fd();
    let pid_secondary = open_pty_res.slave.into_raw_fd();

    if !command_exists(&cmd) {
        return Err(ZellijError::CommandNotFound {
            terminal_id,
            command: cmd.command.to_string_lossy().to_string(),
        })
        .with_context(|| err_context(&cmd));
    }

    let child = unsafe {
        let cmd = cmd.clone();
        let command = &mut Command::new(cmd.command);
        if let Some(current_dir) = cmd.cwd {
            if current_dir.exists() && current_dir.is_dir() {
                command.current_dir(current_dir);
            } else {
                log::error!(
                    "Failed to set CWD for new pane. '{}' does not exist or is not a folder",
                    current_dir.display()
                );
            }
        }
        command
            .args(&cmd.args)
            // The server may itself be running in a supervised pane. This
            // in-process fallback has no stable supervisor reference of its own.
            .env_remove(crate::pty_host::PANE_REF_ENV_KEY)
            .env("ZELLIJ_PANE_ID", format!("{}", terminal_id))
            .pre_exec(move || -> io::Result<()> {
                if libc::login_tty(pid_secondary) != 0 {
                    panic!("failed to set controlling terminal");
                }
                close_fds::close_open_fds(3, &[]);
                Ok(())
            })
            .spawn()
            .expect("failed to spawn")
    };

    let child_id = child.id();
    thread::spawn(move || {
        // `Child::wait` is `waitpid` without `WUNTRACED`, so it cannot tell a
        // stopped child from a busy one — and a `^Z`'d command pane looks
        // exactly like a hang. Wait for the stops too, and only then hand the
        // now-reaped child to `handle_command_exit` for its signal handling.
        let exit_status = wait_reporting_stops(child.id(), terminal_id, child_state_cb.as_ref());
        handle_command_exit(child)
            .with_context(|| err_context(&cmd))
            .fatal();
        let _ = unistd::close(pid_secondary);
        quit_cb(PaneId::Terminal(terminal_id), exit_status, cmd);
    });

    Ok((pid_primary, child_id as RawFd))
}

/// Spawns a new terminal from the parent terminal with [`termios`](termios::Termios)
/// `orig_termios`.
fn handle_terminal(
    cmd: RunCommand,
    failover_cmd: Option<RunCommand>,
    orig_termios: Option<termios::Termios>,
    quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    child_state_cb: Option<ChildStateCb>,
    terminal_id: u32,
) -> Result<(RawFd, RawFd)> {
    let err_context = || "failed to spawn child terminal".to_string();

    // Create a pipe to allow the child the communicate the shell's pid to its
    // parent.
    match openpty(None, &orig_termios) {
        Ok(open_pty_res) => handle_openpty(open_pty_res, cmd, quit_cb, child_state_cb, terminal_id),
        Err(e) => match failover_cmd {
            Some(failover_cmd) => handle_terminal(
                failover_cmd,
                None,
                orig_termios,
                quit_cb,
                child_state_cb,
                terminal_id,
            )
            .with_context(err_context),
            None => Err::<(i32, i32), _>(e)
                .context("failed to start pty")
                .with_context(err_context)
                .to_log(),
        },
    }
}

/// How a pane's pty is reached.
///
/// Deliberately an enum rather than an `Option<RawFd>`: a pane whose process
/// lives on another machine has no local descriptor, and every operation that
/// used to dereference one has to answer for that case. Keeping the arms
/// exhaustive means a new transport is a compile error at each site that must
/// handle it, rather than a silent fall-through to "no fd found" — or, worse, to
/// a descriptor or pid that belongs to something else entirely.
pub(crate) enum PaneTransport {
    /// The id is allocated but nothing is spawned yet (`reserve_terminal_id`).
    /// A pane sits here between being reserved and being started, and `hold_on_start`
    /// panes sit here indefinitely.
    Reserved,
    /// A pty master this server holds directly, whether it came from an
    /// in-process `openpty` or over `SCM_RIGHTS` from a local supervisor.
    Local(RawFd),
    /// A pty owned by a broker on another machine. There is no descriptor and no
    /// usable pid on this side; every operation is a frame on the connection.
    Remote(Arc<RemoteConn>),
}

impl PaneTransport {
    /// The local master descriptor, if this pane has one.
    ///
    /// Only for operations that are genuinely fd-shaped and have no meaning
    /// otherwise. Prefer matching exhaustively where a non-local pane needs its
    /// own answer rather than "no fd".
    fn local_fd(&self) -> Option<RawFd> {
        match self {
            PaneTransport::Reserved | PaneTransport::Remote(_) => None,
            PaneTransport::Local(fd) => Some(*fd),
        }
    }

    fn remote(&self) -> Option<&Arc<RemoteConn>> {
        match self {
            PaneTransport::Reserved | PaneTransport::Local(_) => None,
            PaneTransport::Remote(conn) => Some(conn),
        }
    }
}

/// The Unix PTY backend. Manages native PTY file descriptors and signals.
#[derive(Clone)]
pub(crate) struct UnixPtyBackend {
    orig_termios: Arc<Mutex<Option<termios::Termios>>>,
    terminal_id_to_transport: Arc<Mutex<BTreeMap<u32, PaneTransport>>>,
    next_terminal_id_counter: Arc<AtomicU32>,
    /// Whether panes are spawned under a detachable pty-host supervisor. Set once
    /// per session from config; see `ServerOsApi::set_pty_host_enabled`.
    pty_host_enabled: Arc<AtomicBool>,
    /// The binary to re-exec as a supervisor, resolved once here rather than per
    /// pane: `current_exe()` starts failing after the binary is replaced on disk
    /// (a package upgrade mid-session), and resolving it repeatedly would turn
    /// that into "no new panes" instead of one stale path.
    host_bin: Option<PathBuf>,
    /// The user's terminal settings, in wire form. Sampled here because
    /// `UnixPtyBackend::new()` runs before the server daemonizes and loses its
    /// controlling terminal — a supervisor could never re-derive them. Converted
    /// once rather than per spawn.
    wire_termios: Option<WireTermios>,
    terminal_id_to_host: Arc<Mutex<BTreeMap<u32, PtyHostHandle>>>,
    /// Told when any pane's root process stops or resumes. Registered once by the
    /// pty thread rather than passed per pane — see `ChildStateCb`.
    child_state_cb: Arc<Mutex<Option<ChildStateCb>>>,
}

/// Try to write as many bytes from `buf` as possible to `fd` without blocking.
///
/// Loops on successful short writes and EINTR to drain as much as the kernel
/// will accept. On EAGAIN (fd buffer full), stops and returns how many bytes
/// were written so far (which may be 0). The caller is expected to re-queue
/// any unwritten remainder.
fn try_write_to_fd(fd: RawFd, buf: &[u8]) -> Result<usize> {
    let mut written = 0;
    while written < buf.len() {
        match unistd::write(unsafe { BorrowedFd::borrow_raw(fd) }, &buf[written..]) {
            Ok(0) => break, // fd returned 0 on non-empty buf; treat like EAGAIN
            Ok(n) => written += n,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(nix::errno::Errno::EAGAIN) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(written)
}

impl UnixPtyBackend {
    pub fn new() -> Result<Self, io::Error> {
        let current_termios = termios::tcgetattr(io::stdin()).ok();
        if current_termios.is_none() {
            log::warn!("Starting a server without a controlling terminal, using the default termios configuration.");
        }
        let wire_termios = current_termios.as_ref().map(WireTermios::from_termios);
        let host_bin = match std::env::current_exe() {
            Ok(path) => Some(path),
            Err(e) => {
                log::warn!("Could not resolve our own binary ({e}); panes will not be detachable.");
                None
            },
        };
        Ok(Self {
            orig_termios: Arc::new(Mutex::new(current_termios)),
            terminal_id_to_transport: Arc::new(Mutex::new(BTreeMap::new())),
            next_terminal_id_counter: Arc::new(AtomicU32::new(0)),
            pty_host_enabled: Arc::new(AtomicBool::new(false)),
            host_bin,
            wire_termios,
            terminal_id_to_host: Arc::new(Mutex::new(BTreeMap::new())),
            child_state_cb: Arc::new(Mutex::new(None)),
        })
    }

    /// Called once at session init, when config is finally available. Env
    /// overrides win, because the config path needs a live session and is
    /// therefore useless for bisecting a startup problem.
    pub fn set_pty_host_enabled(&self, enabled: bool) {
        let enabled = if std::env::var_os("ZELLIJ_DISABLE_PTY_HOST").is_some() {
            false
        } else if std::env::var_os("ZELLIJ_ENABLE_PTY_HOST").is_some() {
            true
        } else {
            enabled
        };
        self.pty_host_enabled.store(enabled, Ordering::SeqCst);
    }

    /// Shut down the supervisor for a pane, if it has one.
    ///
    /// The lock is released before `terminate()` runs: that does a blocking write
    /// and we are usually on the pty thread, which must not stall behind another
    /// pane's teardown.
    fn terminate_host(&self, terminal_id: u32) {
        let handle = self
            .terminal_id_to_host
            .lock()
            .ok()
            .and_then(|mut hosts| hosts.remove(&terminal_id));
        if let Some(mut handle) = handle {
            handle.terminate();
        }
    }

    /// Terminate every supervisor this server owns. Belt and braces for session
    /// shutdown: `clear_terminal_id` already covers each pane individually, but a
    /// leaked supervisor is a leaked user process, so sweep before exiting.
    pub fn shutdown_pty_hosts(&self) {
        let handles: Vec<PtyHostHandle> = match self.terminal_id_to_host.lock() {
            Ok(mut hosts) => std::mem::take(&mut *hosts).into_values().collect(),
            Err(_) => return,
        };
        for mut handle in handles {
            handle.terminate();
        }
    }

    fn spawn_via_pty_host(
        &self,
        host_bin: &std::path::Path,
        cmd: &RunCommand,
        failover_cmd: &Option<RunCommand>,
        terminal_id: u32,
    ) -> Result<pty_host_client::PendingHost> {
        // Kept here rather than left to the supervisor so that a mistyped command
        // still produces a synchronous `ZellijError::CommandNotFound` — which is
        // what `pane_start_failure` in pty.rs shows the user inside the pane
        // rather than dying on — without paying for a process spawn.
        if !command_exists(cmd) {
            return Err(ZellijError::CommandNotFound {
                terminal_id,
                command: cmd.command.to_string_lossy().to_string(),
            }
            .into());
        }
        pty_host_client::spawn(SpawnParams {
            host_bin,
            terminal_id,
            session_name: zellij_utils::envs::get_session_name().ok(),
            cmd: cmd.clone(),
            failover_cmd: failover_cmd.clone(),
            termios: self.wire_termios.clone(),
            // Left unset on purpose: today's `openpty(None, ..)` means a pane
            // starts at 0x0 until the first TIOCSWINSZ, and matching that keeps
            // this change pure plumbing.
            initial_size: None,
            // A local pane execs its command under its own name, as it always
            // has. Only the remote broker asks for a login shell's argv[0].
            arg0: None,
            // A local pane inherits this server's environment, which is where its
            // TERM and locale have always come from. Only a remote pane needs one
            // composed for it, because only a remote pane cannot inherit ours.
            env: vec![],
            login_session: false,
            // This server listens for the stops job control makes possible and
            // can send `SIGCONT` back, so a pane that suspends is recoverable.
            job_control: true,
        })
    }

    /// Register the one callback that hears about every pane's suspend/resume.
    pub fn set_child_state_callback(&self, cb: ChildStateCb) {
        if let Ok(mut guard) = self.child_state_cb.lock() {
            *guard = Some(cb);
        }
    }

    fn child_state_cb(&self) -> Option<ChildStateCb> {
        self.child_state_cb.lock().ok()?.clone()
    }

    pub fn spawn_terminal(
        &self,
        cmd: RunCommand,
        failover_cmd: Option<RunCommand>,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        terminal_id: u32,
    ) -> Result<(Box<dyn AsyncReader>, RawFd)> {
        // A re-run reuses the terminal_id, so an earlier supervisor for it would
        // otherwise be orphaned with nobody left holding its control channel.
        self.terminate_host(terminal_id);

        if self.pty_host_enabled.load(Ordering::Relaxed) {
            if let Some(host_bin) = self.host_bin.as_ref() {
                match self.spawn_via_pty_host(host_bin, &cmd, &failover_cmd, terminal_id) {
                    Ok(pending) => {
                        let spawned = pending.finish(quit_cb, self.child_state_cb());
                        let master_fd = spawned.master.into_raw_fd();
                        self.terminal_id_to_transport
                            .lock()
                            .to_anyhow()?
                            .insert(terminal_id, PaneTransport::Local(master_fd));
                        self.terminal_id_to_host
                            .lock()
                            .to_anyhow()?
                            .insert(terminal_id, spawned.handle);
                        let async_reader = Box::new(
                            RawFdAsyncReader::new(master_fd)
                                .map_err(|e| anyhow!("failed to create async reader: {}", e))?,
                        ) as Box<dyn AsyncReader>;
                        return Ok((async_reader, spawned.child_pid as RawFd));
                    },
                    Err(e) => {
                        if e.downcast_ref::<ZellijError>().is_some() {
                            // CommandNotFound and friends are real answers, not
                            // supervisor trouble; do not paper over them.
                            return Err(e);
                        }
                        // Anything else — an upgraded binary, a protocol mismatch,
                        // a supervisor that would not start — should cost the user
                        // detachability, not the pane.
                        log::warn!(
                            "could not start a pty-host for pane {terminal_id} ({e:#}); \
                             falling back to an in-process pty"
                        );
                    },
                }
            }
        }

        let orig_termios = self
            .orig_termios
            .lock()
            .to_anyhow()
            .context("failed to lock orig_termios")?;
        let (pid_primary, child_fd) = handle_terminal(
            cmd,
            failover_cmd,
            orig_termios.clone(),
            quit_cb,
            self.child_state_cb(),
            terminal_id,
        )?;
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .insert(terminal_id, PaneTransport::Local(pid_primary));
        let async_reader = Box::new(
            RawFdAsyncReader::new(pid_primary)
                .map_err(|e| anyhow::anyhow!("failed to create async reader: {}", e))?,
        ) as Box<dyn AsyncReader>;
        Ok((async_reader, child_fd))
    }

    pub fn set_terminal_size(
        &self,
        terminal_id: u32,
        cols: u16,
        rows: u16,
        width_in_pixels: Option<u16>,
        height_in_pixels: Option<u16>,
    ) -> Result<()> {
        let err_context = || {
            format!(
                "failed to set terminal id {} to size ({}, {})",
                terminal_id, rows, cols
            )
        };
        let transports = self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()
            .with_context(err_context)?;
        match transports.get(&terminal_id) {
            Some(PaneTransport::Local(fd)) => {
                if cols > 0 && rows > 0 {
                    set_terminal_size_using_fd(*fd, cols, rows, width_in_pixels, height_in_pixels);
                }
            },
            Some(PaneTransport::Remote(conn)) => {
                if cols > 0 && rows > 0 {
                    let conn = conn.clone();
                    // The map lock is dropped before the send: a slow link must
                    // not hold every other pane's resize behind it.
                    drop(transports);
                    conn.resize(WireWinsize {
                        cols,
                        rows,
                        width_in_pixels: width_in_pixels.unwrap_or(0),
                        height_in_pixels: height_in_pixels.unwrap_or(0),
                    })
                    .with_context(err_context)
                    .non_fatal();
                }
            },
            Some(PaneTransport::Reserved) | None => {
                Err::<(), _>(anyhow!("failed to find terminal fd for id {terminal_id}"))
                    .with_context(err_context)
                    .non_fatal();
            },
        }
        Ok(())
    }

    pub fn write_to_tty_stdin(&self, terminal_id: u32, buf: &[u8]) -> Result<usize> {
        let err_context = || format!("failed to write to stdin of TTY ID {}", terminal_id);

        // Resolved to a target before any I/O so the map lock is not held across
        // a write. Both arms honour the same contract `pty_writer` relies on:
        // the return is how many bytes were *accepted*, and a short count means
        // "ask again", never "written".
        enum Target {
            Local(RawFd),
            Remote(Arc<RemoteConn>),
        }
        let target = match self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()
            .with_context(err_context)?
            .get(&terminal_id)
        {
            Some(PaneTransport::Local(fd)) => Target::Local(*fd),
            Some(PaneTransport::Remote(conn)) => Target::Remote(conn.clone()),
            Some(PaneTransport::Reserved) | None => {
                return Err(anyhow!("could not find raw file descriptor")).with_context(err_context)
            },
        };

        match target {
            Target::Local(fd) => try_write_to_fd(fd, buf).with_context(err_context),
            Target::Remote(conn) => conn.write_input(buf).with_context(err_context),
        }
    }

    pub fn tcdrain(&self, terminal_id: u32) -> Result<()> {
        let err_context = || format!("failed to tcdrain to TTY ID {}", terminal_id);

        match self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()
            .with_context(err_context)?
            .get(&terminal_id)
        {
            Some(PaneTransport::Local(fd)) => {
                termios::tcdrain(unsafe { BorrowedFd::borrow_raw(*fd) }).with_context(err_context)
            },
            // "Wait until the kernel has handed every queued byte to the
            // terminal" does not survive the trip: the queue that matters is on
            // the far side, and answering would cost a round trip on the pty
            // thread to tell the caller something it cannot act on. Nothing
            // calls this today; a remote pane says "drained" rather than
            // pretending to a synchronisation it cannot provide.
            Some(PaneTransport::Remote(_)) => Ok(()),
            Some(PaneTransport::Reserved) | None => {
                Err(anyhow!("could not find raw file descriptor")).with_context(err_context)
            },
        }
    }

    pub fn tcgetpgrp(&self, terminal_id: u32) -> Option<i32> {
        let fd = self
            .terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .local_fd()?;
        unistd::tcgetpgrp(unsafe { BorrowedFd::borrow_raw(fd) })
            .ok()
            .map(|pgid| pgid.as_raw())
    }

    pub fn kill(&self, pid: u32) -> Result<()> {
        let _ = kill(unistd::Pid::from_raw(pid as i32), Some(Signal::SIGHUP));
        Ok(())
    }

    pub fn force_kill(&self, pid: u32) -> Result<()> {
        let _ = kill(unistd::Pid::from_raw(pid as i32), Some(Signal::SIGKILL));
        Ok(())
    }

    pub fn send_sigint(&self, pid: u32) -> Result<()> {
        let _ = kill(unistd::Pid::from_raw(pid as i32), Some(Signal::SIGINT));
        Ok(())
    }

    /// Continue a stopped pane, the way a shell's `fg` does.
    ///
    /// Aimed at the *process group*, not the pid: `login_tty` made the child a
    /// session leader with `pgid == pid`, and `^Z` stopped everything in its
    /// foreground group, not just the leader. Continuing only the leader would
    /// leave the rest of a pipeline asleep. The bare pid is a fallback for the
    /// case where the child never got a group of its own.
    pub fn send_sigcont(&self, pid: u32) -> Result<()> {
        let group = unistd::Pid::from_raw(-(pid as i32));
        if kill(group, Some(Signal::SIGCONT)).is_err() {
            let _ = kill(unistd::Pid::from_raw(pid as i32), Some(Signal::SIGCONT));
        }
        Ok(())
    }

    /// Adopt an already-connected remote pane: record its transport and hand
    /// back the reader `TerminalBytes` will drive.
    ///
    /// The handshake and `Spawn` exchange happen before this — by the time a
    /// pane exists, the far end has already confirmed the child is running, so
    /// there is no window in which a pane is on screen but unspawned.
    pub fn register_remote_terminal(
        &self,
        terminal_id: u32,
        conn: Arc<RemoteConn>,
        read_socket: std::os::unix::net::UnixStream,
        quit_cb: crate::pty_host::client::QuitCb,
        cmd: RunCommand,
    ) -> Result<Box<dyn AsyncReader>> {
        let reader = Box::new(
            crate::pty_host::remote::RemoteReader::new(conn.clone(), read_socket)
                .map_err(|e| anyhow!("failed to create a remote async reader: {e}"))?
                // The remote counterpart of the supervisor's reader thread: the
                // one thing that will tell the pty thread this pane has ended.
                .on_exit(terminal_id, cmd, quit_cb),
        ) as Box<dyn AsyncReader>;
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .insert(terminal_id, PaneTransport::Remote(conn));
        Ok(reader)
    }

    /// Whether this pane's process lives on another machine.
    ///
    /// The question every pid-shaped operation has to ask first: `id_to_child_pid`
    /// holds nothing usable for a remote pane, and a pid from another machine
    /// names an unrelated local process rather than nothing at all.
    pub fn is_remote(&self, terminal_id: u32) -> bool {
        self.terminal_id_to_transport
            .lock()
            .ok()
            .and_then(|t| t.get(&terminal_id).map(|t| t.remote().is_some()))
            .unwrap_or(false)
    }

    /// The last thing the broker reported about a remote pane, or `None` for a
    /// local one.
    ///
    /// Cached rather than queried: the caller is `update_and_report_cwds` on the
    /// pty thread, and a round trip over a link that may be dead is not
    /// something that thread can afford.
    pub fn remote_status(&self, terminal_id: u32) -> Option<RemoteStatus> {
        self.terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .map(|conn| conn.status())
    }

    /// Every pane whose process lives on another machine.
    pub fn remote_pane_ids(&self) -> Vec<u32> {
        self.terminal_id_to_transport
            .lock()
            .map(|t| {
                t.iter()
                    .filter(|(_, transport)| transport.remote().is_some())
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What git last said about a remote pane's working directory.
    pub fn remote_git_probe(&self, terminal_id: u32) -> Option<RemoteGitProbe> {
        let conn = self
            .terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .cloned()?;
        let mut probe = conn.git_probe()?;
        if let Some(host) = conn.cmd().display_host() {
            probe.host = host;
        }
        Some(probe)
    }

    /// The machine a pane runs on, or `None` if it runs on this one.
    pub fn remote_host(&self, terminal_id: u32) -> Option<String> {
        self.terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .and_then(|conn| conn.cmd().display_host())
    }

    pub fn remote_command(&self, terminal_id: u32) -> Option<RunCommand> {
        self.terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .map(|conn| conn.cmd().clone())
    }

    /// Deliver a signal to a remote pane's child, by name rather than by pid.
    pub fn signal_remote_child(&self, terminal_id: u32, signal: ChildSignal) -> Result<()> {
        let conn = self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .get(&terminal_id)
            .and_then(|t| t.remote().cloned())
            .ok_or_else(|| anyhow!("pane {terminal_id} is not a remote pane"))?;
        conn.signal_child(signal)
    }

    pub fn reserve_terminal_id(&self, terminal_id: u32) {
        self.terminal_id_to_transport
            .lock()
            .unwrap()
            .insert(terminal_id, PaneTransport::Reserved);
    }

    /// Give a reservation back without terminating anything.
    ///
    /// Deliberately not `clear_terminal_id`, which is the teardown funnel and
    /// would end whatever the id names. This is for an adoption that failed
    /// before it recorded a transport: the pane it was reaching for is still
    /// running somewhere, and must stay that way for the caller's rollback.
    pub fn forget_reserved_terminal_id(&self, terminal_id: u32) {
        self.terminal_id_to_transport
            .lock()
            .unwrap()
            .remove(&terminal_id);
    }

    pub fn clear_terminal_id(&self, terminal_id: u32) -> Result<()> {
        // The single funnel for pane teardown: `Pty::close_pane` calls this, and
        // every path that closes a pane — ClosePane, CloseTab, spawn failures,
        // `Drop for Pty` at session shutdown — goes through it. Terminating here
        // means no changes are needed in pty.rs or screen.rs.
        self.terminate_host(terminal_id);
        let removed = self
            .terminal_id_to_transport
            .lock()
            .unwrap()
            .remove(&terminal_id);
        // A remote pane's supervisor is on the far machine, so `terminate_host`
        // above cannot reach it. Without this the pane would leave a live
        // process and a live broker behind on every ordinary close.
        if let Some(PaneTransport::Remote(conn)) = removed {
            conn.terminate(REMOTE_TERMINATION_TIMEOUT)
                .with_context(|| format!("failed to terminate remote pane {terminal_id}"))?;
        }
        Ok(())
    }

    pub fn next_terminal_id(&self) -> Option<u32> {
        Some(
            self.next_terminal_id_counter
                .fetch_add(1, Ordering::Relaxed),
        )
    }

    /// Give a pane's supervisor up without killing it, so another server can take
    /// it over. `clear_terminal_id` minus the `terminate_host`.
    ///
    /// The master dup is deliberately not closed here: it belongs to the reader
    /// task's `RawFdAsyncReader`, which closes it when the caller aborts that
    /// task. Closing it here as well would race that drop into a double close of
    /// a descriptor another pane may by then have been given.
    pub fn release_terminal_for_warp(
        &self,
        terminal_id: u32,
        budget: Duration,
    ) -> Result<ReleasedPane> {
        // A remote pane is asked about first, because the miss below is not the
        // truth about one: its supervisor exists, it is simply on the far side
        // of a broker and was never going to be in this map.
        //
        // Bound to a `let` rather than matched in place, and not for style: the
        // guard on `terminal_id_to_transport` is a temporary of the expression
        // it is created in, and as an `if let` scrutinee it would live through
        // the whole body — including the release below, which takes the same
        // lock again to remove the pane. `std::sync::Mutex` is not reentrant,
        // so that was a deadlock on the pty thread for every remote pane ever
        // released. The `let` ends the guard's statement, and its life, here.
        let remote = self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .get(&terminal_id)
            .and_then(|transport| transport.remote())
            .cloned();
        if let Some(conn) = remote {
            return self.release_remote_terminal_for_warp(terminal_id, conn, budget);
        }

        let handle = self
            .terminal_id_to_host
            .lock()
            .to_anyhow()?
            .remove(&terminal_id)
            .ok_or_else(|| {
                anyhow!(
                    "pane {terminal_id} has no pty-host supervisor, so it cannot be moved to \
                     another session — detachable panes are disabled, or this pane's supervisor \
                     failed to start and it fell back to an in-process pty"
                )
            })?;
        let released = handle.release();
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .remove(&terminal_id);
        Ok(ReleasedPane::Local(released))
    }

    /// Whether [`Self::release_terminal_for_warp`] would work, asked while the
    /// pane is still whole.
    ///
    /// Deliberately every refusal that the release and `Screen::warp_address`
    /// between them can produce, in the same words, and deliberately nothing
    /// else: the release aborts the pane's reader task before it gets far enough
    /// to refuse, so a "no" raised there leaves a pane on screen with no process
    /// left to talk to. Asked here it costs the user an error message and nothing
    /// more.
    ///
    /// A remote pane mostly can only answer yes. Its supervisor is on the far
    /// side of a broker reached by ssh and a token, and whether that broker will
    /// confirm the handoff is not knowable without asking it — which is the
    /// release itself. That residual case is why callers still have to cope with
    /// a release that fails.
    ///
    /// The one thing that *is* knowable here is whether there is currently a
    /// path to that broker at all. A pane whose transport has been lost is being
    /// reconnected by its reader right now, and the release would abort that
    /// reader — killing a recovery that was working — only to fail on the same
    /// broken socket a moment later, leaving the pane stranded. Refusing while
    /// the pane is still whole costs the user a message and a retry; not
    /// refusing costs them the pane.
    pub fn check_terminal_can_be_warped(&self, terminal_id: u32) -> Result<()> {
        let remote = self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .get(&terminal_id)
            .and_then(|transport| transport.remote())
            .cloned();
        if let Some(conn) = remote {
            if conn.cmd().firecracker_vm.is_some() {
                return Err(anyhow!(
                    "Firecracker panes belong to their originating session and cannot be moved between sessions"
                ));
            }
            if conn.transport_lost() {
                return Err(anyhow!(
                    "the connection to {} is down and this pane is reconnecting, so it cannot \
                     be moved yet — try again once it is back",
                    conn.host()
                ));
            }
            return Ok(());
        }

        let hosts = self.terminal_id_to_host.lock().to_anyhow()?;
        let handle = hosts.get(&terminal_id).ok_or_else(|| {
            anyhow!(
                "pane {terminal_id} has no pty-host supervisor, so it cannot be moved to \
                 another session — detachable panes are disabled, or this pane's supervisor \
                 failed to start and it fell back to an in-process pty"
            )
        })?;
        if handle.control_socket.is_none() {
            return Err(anyhow!(
                "the supervisor for this pane ({}) never bound a socket, so no other session \
                 can reach it — this usually means the runtime directory path is too long",
                handle.host_id
            ));
        }
        Ok(())
    }

    /// Hand a remote pane back to its broker for someone else to resume.
    ///
    /// `received` is sampled *before* the release rather than after, and that
    /// ordering is load-bearing. The broker replays from this number, and the
    /// pane's screen is serialized separately from the grid this side already
    /// filled; a byte has to be in exactly one of the two. The caller has
    /// already stopped the reader, so nothing can be counted between the sample
    /// and the release, and everything the pane produces after it is buffered on
    /// the broker for the replay.
    ///
    /// The pane leaves the transport table whether or not the release succeeded.
    /// A failure here means the broker did not confirm, so the pane may or may
    /// not be free — but this session has stopped reading it either way, and
    /// keeping a connection nothing drives would leave a pane that looks live
    /// and is not.
    fn release_remote_terminal_for_warp(
        &self,
        terminal_id: u32,
        conn: Arc<RemoteConn>,
        budget: Duration,
    ) -> Result<ReleasedPane> {
        let received = conn.received();
        // Whichever is shorter. `REMOTE_RELEASE_TIMEOUT` is what one round trip
        // to a broker should ever need; the budget is what the caller is still
        // prepared to wait. Spending longer than the second would answer a
        // question nobody is listening for any more.
        let released = conn
            .release_for_warp(budget.min(REMOTE_RELEASE_TIMEOUT))
            .with_context(|| format!("failed to hand remote pane {terminal_id} over"));
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .remove(&terminal_id);
        released?;
        Ok(ReleasedPane::Remote(ReleasedRemotePane {
            host: conn.host().to_owned(),
            pane_token: conn.pane_token().to_owned(),
            received,
            cmd: conn.cmd().clone(),
        }))
    }

    /// Take a detached supervisor over into this session under a fresh terminal
    /// id — ids are per-server, so whatever the pane was called in the session it
    /// came from means nothing here.
    pub fn adopt_terminal(
        &self,
        control_socket: &std::path::Path,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        let terminal_id = self
            .next_terminal_id()
            .ok_or_else(|| anyhow!("failed to allocate a terminal id"))?;
        self.reserve_terminal_id(terminal_id);
        match self.adopt_into_terminal_id(control_socket, terminal_id, quit_cb) {
            Ok(adopted) => Ok(adopted),
            Err(e) => {
                // Undo the reservation, but *not* through `clear_terminal_id` —
                // that terminates the supervisor, and a failed adoption must leave
                // it running for the next caller.
                self.terminal_id_to_transport
                    .lock()
                    .to_anyhow()?
                    .remove(&terminal_id);
                Err(e)
            },
        }
    }

    fn adopt_into_terminal_id(
        &self,
        control_socket: &std::path::Path,
        terminal_id: u32,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        let pending = pty_host_client::attach(pty_host_client::AttachParams {
            control_socket,
            terminal_id,
            session_name: zellij_utils::envs::get_session_name().ok(),
        })?;
        let attached = pending.finish(quit_cb, self.child_state_cb());
        let master_fd = attached.master.into_raw_fd();
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .insert(terminal_id, PaneTransport::Local(master_fd));
        self.terminal_id_to_host
            .lock()
            .to_anyhow()?
            .insert(terminal_id, attached.handle);
        let reader = Box::new(
            RawFdAsyncReader::new(master_fd)
                .map_err(|e| anyhow!("failed to create async reader: {}", e))?,
        ) as Box<dyn AsyncReader>;
        Ok(AdoptedTerminal {
            terminal_id,
            reader,
            child_pid: Some(attached.child_pid),
            cmd: attached.cmd,
            buffered: attached.buffered,
            child_exit_status: attached.status.child_exit_status,
        })
    }
}

/// A supervisor this server has just taken over, and everything the caller needs
/// to give it a pane.
pub struct AdoptedTerminal {
    pub terminal_id: u32,
    pub reader: Box<dyn AsyncReader>,
    /// `None` for a pane adopted from another machine. Its child's pid is
    /// meaningful only there, and recording one here would put it in
    /// `id_to_child_pid`, where every consumer would act on it locally.
    pub child_pid: Option<u32>,
    /// The command the supervisor is running, for rebuilding a `quit_cb` that
    /// holds or closes the pane the way the original did.
    pub cmd: RunCommand,
    /// Output produced while nobody was attached, to be replayed into the pane
    /// before any live bytes.
    pub buffered: Vec<u8>,
    /// `Some` if the child has already exited — the pane should arrive held.
    pub child_exit_status: Option<Option<i32>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    use nix::sys::termios;
    use std::io::Read;

    /// Verify that `try_write_to_fd` writes as many bytes as the kernel will
    /// accept in one pass and returns a partial count (not an error) when the
    /// PTY buffer fills up.
    ///
    /// A concurrent reader drains the slave side so some bytes are accepted.
    /// The key assertion: the function returns Ok(n) where n <= buf.len(),
    /// and the caller (PtyWriter) is responsible for re-queuing the rest.
    #[test]
    fn try_write_to_fd_returns_partial_on_full_buffer() {
        let pty = openpty(None, &None).expect("openpty failed");
        let master_fd = pty.master.into_raw_fd();
        let slave_fd = pty.slave.into_raw_fd();
        let borrowed_master = unsafe { BorrowedFd::borrow_raw(master_fd) };
        let borrowed_slave = unsafe { BorrowedFd::borrow_raw(slave_fd) };

        let mut attrs = termios::tcgetattr(borrowed_slave).expect("tcgetattr failed");
        termios::cfmakeraw(&mut attrs);
        termios::tcsetattr(borrowed_slave, termios::SetArg::TCSANOW, &attrs)
            .expect("tcsetattr failed");

        // O_NONBLOCK so write() returns EAGAIN instead of blocking
        let flags = fcntl(borrowed_master, FcntlArg::F_GETFL).expect("F_GETFL");
        let mut oflags = OFlag::from_bits_truncate(flags);
        oflags.insert(OFlag::O_NONBLOCK);
        fcntl(borrowed_master, FcntlArg::F_SETFL(oflags)).expect("F_SETFL");

        // Fill most of the buffer, leaving some space
        let chunk = vec![0x42u8; 1024];
        let mut total_filled = 0;
        loop {
            match super::try_write_to_fd(master_fd, &chunk) {
                Ok(0) => break,
                Ok(n) => total_filled += n,
                Err(e) => panic!("unexpected error filling buffer: {e}"),
            }
        }
        assert!(
            total_filled > 0,
            "should have written some bytes to fill buffer"
        );

        // Read a small amount from the slave to free partial space
        let mut drain = vec![0u8; 512];
        let slave_file = unsafe { std::fs::File::from_raw_fd(slave_fd) };
        let mut slave_reader = std::io::BufReader::new(&slave_file);
        let drained = slave_reader.read(&mut drain).expect("slave read failed");
        assert!(drained > 0, "should have drained some bytes");
        // Prevent File from closing the slave fd — we close it manually below
        std::mem::forget(slave_file);

        // Now write more than the freed space — should get a partial write
        let size = 128 * 1024;
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        let written = super::try_write_to_fd(master_fd, &data)
            .expect("try_write_to_fd should not error on EAGAIN");

        assert!(
            written > 0 && written < size,
            "expected partial write, got {written}/{size}",
        );

        unsafe {
            libc::close(master_fd);
            libc::close(slave_fd);
        }
    }

    /// Verify that `try_write_to_fd` returns Ok(0) — not an error — when the
    /// fd is completely full and cannot accept any bytes at all.
    #[test]
    fn try_write_to_fd_returns_zero_on_stuck_pty() {
        let pty = openpty(None, &None).expect("openpty failed");
        let master_fd = pty.master.into_raw_fd();
        let slave_fd = pty.slave.into_raw_fd();
        let borrowed_master = unsafe { BorrowedFd::borrow_raw(master_fd) };
        let borrowed_slave = unsafe { BorrowedFd::borrow_raw(slave_fd) };

        let mut attrs = termios::tcgetattr(borrowed_slave).expect("tcgetattr failed");
        termios::cfmakeraw(&mut attrs);
        termios::tcsetattr(borrowed_slave, termios::SetArg::TCSANOW, &attrs)
            .expect("tcsetattr failed");

        let flags = fcntl(borrowed_master, FcntlArg::F_GETFL).expect("F_GETFL");
        let mut oflags = OFlag::from_bits_truncate(flags);
        oflags.insert(OFlag::O_NONBLOCK);
        fcntl(borrowed_master, FcntlArg::F_SETFL(oflags)).expect("F_SETFL");

        // Fill the buffer completely — keep writing until we get Ok(0)
        let fill = vec![0x42u8; 1024];
        loop {
            match super::try_write_to_fd(master_fd, &fill) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => panic!("unexpected error filling buffer: {e}"),
            }
        }

        // Now the buffer is full — next write should return Ok(0)
        let written = super::try_write_to_fd(master_fd, &[0x01, 0x02, 0x03])
            .expect("try_write_to_fd should not error on EAGAIN");

        assert_eq!(written, 0, "expected zero bytes written on full buffer");

        unsafe {
            libc::close(master_fd);
            libc::close(slave_fd);
        }
    }

    /// The real backend, releasing a remote pane, on the thread that would
    /// deadlock.
    ///
    /// The lookup that finds the pane's connection and the release that removes
    /// it take the same lock, and a guard kept alive across the two — which an
    /// `if let` scrutinee did — blocked the pty thread for good. The peer here
    /// hangs up at once, so a release that returns at all returns in
    /// milliseconds; one that has not returned in five seconds is the deadlock.
    #[test]
    fn releasing_a_remote_pane_does_not_deadlock_on_the_transport_table() {
        use std::os::unix::net::UnixStream;

        let backend = UnixPtyBackend::new().expect("a backend");
        let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
        drop(theirs);
        let conn = RemoteConn::new(
            ours,
            "build-box".to_owned(),
            "tok".to_owned(),
            RunCommand::default(),
            None,
        )
        .expect("a remote connection");
        backend
            .terminal_id_to_transport
            .lock()
            .unwrap()
            .insert(7, PaneTransport::Remote(Arc::new(conn)));

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let releasing = backend.clone();
        std::thread::spawn(move || {
            let outcome = releasing.release_terminal_for_warp(7, Duration::from_secs(1));
            let _ = done_tx.send(outcome.map(|_| ()).map_err(|e| format!("{e:#}")));
        });

        // Whether the broker "confirmed" is the peer's business, and the peer
        // is gone; what is asserted is that the call came back at all.
        let _ = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the release deadlocked on the transport table");
        assert!(
            !backend
                .terminal_id_to_transport
                .lock()
                .unwrap()
                .contains_key(&7),
            "a released pane leaves the transport table whether or not the broker confirmed"
        );
    }
}
