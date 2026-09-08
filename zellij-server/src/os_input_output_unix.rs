use crate::os_input_output::{command_exists, AsyncReader};
use crate::panes::PaneId;
use zellij_types::internal_ids::ProcessId;
use zellij_types::internal_ids::TerminalId;

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

use zellij_runtime::errors::prelude::*;
use zellij_types::input::command::RunCommand;

use crate::pty_host::remote::{
    conn::RemoteGitProbe, protocol::ChildSignal, RemoteConn, RemoteStatus,
};
use crate::pty_host::{
    client::{
        self as pty_host_client, AgentReportCb, ChildRunState, ChildStateCb, PtyHostHandle,
        ReleasedPane, ReleasedRemotePane, SpawnParams,
    },
    host::STOPPED_POLL_INTERVAL,
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
    let id = ProcessId::new(child.id());
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
            kill(crate::pty_host::process_pid(id)?, Some(Signal::SIGTERM))
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
///
/// The mirror of `pty_host::host::spawn_child_waiter`, polling while stopped for
/// the same reason: on XNU nothing wakes a blocking wait for a continue, so the
/// pane would never learn it had resumed. See that function for the detail.
fn wait_reporting_stops(
    child_pid: ProcessId,
    terminal_id: TerminalId,
    child_state_cb: Option<&ChildStateCb>,
) -> Option<i32> {
    let pid = crate::pty_host::process_pid(child_pid).ok()?;
    let flags = WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED;
    let mut stopped = false;
    loop {
        let flags = if stopped {
            flags | WaitPidFlag::WNOHANG
        } else {
            flags
        };
        let state = match waitpid(pid, Some(flags)) {
            Ok(WaitStatus::Exited(_, code)) => return Some(code),
            Ok(WaitStatus::Signaled(_, _, _)) => return None,
            Ok(WaitStatus::Stopped(_, _)) => ChildRunState::Stopped,
            Ok(WaitStatus::Continued(_)) => ChildRunState::Continued,
            // Only reachable while polling, and the only arm that sleeps.
            Ok(WaitStatus::StillAlive) => {
                std::thread::sleep(STOPPED_POLL_INTERVAL);
                continue;
            },
            // The ptrace statuses, which exist only on Linux — so on every
            // other platform the arms above are already exhaustive.
            #[allow(unreachable_patterns)]
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            // Nothing left to wait for. Returning is the only safe answer: a
            // waiter that spun here would leave the pane open forever.
            Err(_) => return None,
        };
        stopped = matches!(state, ChildRunState::Stopped);
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
    terminal_id: TerminalId,
) -> Result<(RawFd, ProcessId)> {
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
            // in-process fallback has no stable supervisor reference of its own,
            // and no broker token either: a server running inside a remote pane
            // would otherwise hand every pane the outer pane's token, and a hook
            // reads that before the pane id.
            .env_remove(crate::pty_host::PANE_REF_ENV_KEY)
            .env_remove(crate::pty_host::remote::broker::PANE_TOKEN_ENV)
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

    let child_id = ProcessId::new(child.id());
    thread::spawn(move || {
        // `Child::wait` is `waitpid` without `WUNTRACED`, so it cannot tell a
        // stopped child from a busy one — and a `^Z`'d command pane looks
        // exactly like a hang. Wait for the stops too, and only then hand the
        // now-reaped child to `handle_command_exit` for its signal handling.
        let exit_status = wait_reporting_stops(child_id, terminal_id, child_state_cb.as_ref());
        handle_command_exit(child)
            .with_context(|| err_context(&cmd))
            .fatal();
        let _ = unistd::close(pid_secondary);
        quit_cb(PaneId::Terminal(terminal_id), exit_status, cmd);
    });

    Ok((pid_primary, child_id))
}

/// Spawns a new terminal from the parent terminal with [`termios`](termios::Termios)
/// `orig_termios`.
fn handle_terminal(
    cmd: RunCommand,
    failover_cmd: Option<RunCommand>,
    orig_termios: Option<termios::Termios>,
    quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    child_state_cb: Option<ChildStateCb>,
    terminal_id: TerminalId,
) -> Result<(RawFd, ProcessId)> {
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
            None => Err::<(i32, ProcessId), _>(e)
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
    terminal_id_to_transport: Arc<Mutex<BTreeMap<TerminalId, PaneTransport>>>,
    next_terminal_id_counter: Arc<AtomicU32>,
    /// Whether panes are spawned under a detachable pty-host supervisor. Set once
    /// per session from config; see `ServerOsApi::set_pty_host_enabled`.
    pty_host_enabled: Arc<AtomicBool>,
    /// The binary to re-exec as a supervisor, resolved once here rather than per
    /// pane: `current_exe()` starts failing after the binary is replaced on disk
    /// (a package upgrade mid-session), and resolving it repeatedly would turn
    /// that into "no new panes" instead of one stale path.
    host_bin: Option<PathBuf>,
    /// Original client settings (or stdin's for direct server startup), converted
    /// once for local supervisors, which have no controlling terminal to sample.
    wire_termios: Option<WireTermios>,
    terminal_id_to_host: Arc<Mutex<BTreeMap<TerminalId, PtyHostHandle>>>,
    /// Told when any pane's root process stops or resumes. Registered once by the
    /// pty thread rather than passed per pane — see `ChildStateCb`.
    child_state_cb: Arc<Mutex<Option<ChildStateCb>>>,
    /// Told when a coding agent's hook in any remote pane reports an event.
    /// Registered once, like `child_state_cb`; handed to every remote reader.
    agent_report_cb: Arc<Mutex<Option<AgentReportCb>>>,
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

/// Resolve before a package upgrade unlinks the running executable. Shared by
/// supervisors, hook payloads and Lua helpers that re-exec this server's binary.
pub(crate) fn server_binary() -> Option<&'static PathBuf> {
    static BINARY: std::sync::LazyLock<Option<PathBuf>> =
        std::sync::LazyLock::new(|| match std::env::current_exe() {
            Ok(path) => Some(path),
            Err(e) => {
                log::warn!("Could not resolve our own binary ({e}); re-execution is unavailable.");
                None
            },
        });
    BINARY.as_ref()
}

impl UnixPtyBackend {
    pub fn new() -> Result<Self, io::Error> {
        Self::with_termios(termios::tcgetattr(io::stdin()).ok())
    }

    /// The client terminal settings this backend hands to every local pane.
    #[cfg(test)]
    pub(crate) fn configured_termios(&self) -> Option<WireTermios> {
        self.wire_termios.clone()
    }

    pub fn with_termios(current_termios: Option<termios::Termios>) -> Result<Self, io::Error> {
        if current_termios.is_none() {
            log::warn!("Starting a server without a controlling terminal, using the default termios configuration.");
        }
        let wire_termios = current_termios.as_ref().map(WireTermios::from_termios);
        let host_bin = server_binary().cloned();
        Ok(Self {
            orig_termios: Arc::new(Mutex::new(current_termios)),
            terminal_id_to_transport: Arc::new(Mutex::new(BTreeMap::new())),
            next_terminal_id_counter: Arc::new(AtomicU32::new(0)),
            pty_host_enabled: Arc::new(AtomicBool::new(false)),
            host_bin,
            wire_termios,
            terminal_id_to_host: Arc::new(Mutex::new(BTreeMap::new())),
            child_state_cb: Arc::new(Mutex::new(None)),
            agent_report_cb: Arc::new(Mutex::new(None)),
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
    fn terminate_host(&self, terminal_id: TerminalId) {
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
        terminal_id: TerminalId,
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
            session_name: zellij_runtime::envs::get_session_name().ok(),
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

    /// Register the one callback that hears every remote pane's agent reports.
    pub fn set_agent_report_callback(&self, cb: AgentReportCb) {
        if let Ok(mut guard) = self.agent_report_cb.lock() {
            *guard = Some(cb);
        }
    }

    fn agent_report_cb(&self) -> Option<AgentReportCb> {
        self.agent_report_cb.lock().ok()?.clone()
    }

    pub fn spawn_terminal(
        &self,
        cmd: RunCommand,
        failover_cmd: Option<RunCommand>,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        terminal_id: TerminalId,
    ) -> Result<(Box<dyn AsyncReader>, ProcessId)> {
        // A re-run reuses the terminal_id, so an earlier supervisor for it would
        // otherwise be orphaned with nobody left holding its control channel.
        self.terminate_host(terminal_id);

        if self.pty_host_enabled.load(Ordering::Relaxed) {
            if let Some(host_bin) = self.host_bin.as_ref() {
                match self.spawn_via_pty_host(host_bin, &cmd, &failover_cmd, terminal_id) {
                    Ok(pending) => {
                        let spawned =
                            pending.finish(terminal_exit_callback(quit_cb), self.child_state_cb());
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
                        return Ok((async_reader, spawned.child_pid));
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
        let (pid_primary, child_pid) = handle_terminal(
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
        Ok((async_reader, child_pid))
    }

    pub fn set_terminal_size(
        &self,
        terminal_id: TerminalId,
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

    pub fn write_to_tty_stdin(&self, terminal_id: TerminalId, buf: &[u8]) -> Result<usize> {
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
            // The pane is already visible while its remote transport starts;
            // input typed into the loading screen is intentionally discarded.
            Some(PaneTransport::Reserved) => return Ok(buf.len()),
            None => {
                return Err(anyhow!("could not find raw file descriptor")).with_context(err_context)
            },
        };

        match target {
            Target::Local(fd) => try_write_to_fd(fd, buf).with_context(err_context),
            Target::Remote(conn) => conn.write_input(buf).with_context(err_context),
        }
    }

    pub fn tcdrain(&self, terminal_id: TerminalId) -> Result<()> {
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
            Some(PaneTransport::Reserved) => Ok(()),
            None => Err(anyhow!("could not find raw file descriptor")).with_context(err_context),
        }
    }

    pub fn tcgetpgrp(&self, terminal_id: TerminalId) -> Option<i32> {
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

    pub fn kill(&self, pid: ProcessId) -> Result<()> {
        let _ = kill(crate::pty_host::process_pid(pid)?, Some(Signal::SIGHUP));
        Ok(())
    }

    pub fn force_kill(&self, pid: ProcessId) -> Result<()> {
        let _ = kill(crate::pty_host::process_pid(pid)?, Some(Signal::SIGKILL));
        Ok(())
    }

    pub fn send_sigint(&self, pid: ProcessId) -> Result<()> {
        let _ = kill(crate::pty_host::process_pid(pid)?, Some(Signal::SIGINT));
        Ok(())
    }

    /// Continue a stopped pane, the way a shell's `fg` does.
    ///
    /// Aimed at the *process group*, not the pid: `login_tty` made the child a
    /// session leader with `pgid == pid`, and `^Z` stopped everything in its
    /// foreground group, not just the leader. Continuing only the leader would
    /// leave the rest of a pipeline asleep. The bare pid is a fallback for the
    /// case where the child never got a group of its own.
    pub fn send_sigcont(&self, pid: ProcessId) -> Result<()> {
        let pid = crate::pty_host::process_pid(pid)?;
        let group = unistd::Pid::from_raw(-pid.as_raw());
        if kill(group, Some(Signal::SIGCONT)).is_err() {
            let _ = kill(pid, Some(Signal::SIGCONT));
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
        terminal_id: TerminalId,
        conn: Arc<RemoteConn>,
        read_socket: std::os::unix::net::UnixStream,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        cmd: RunCommand,
    ) -> Result<Box<dyn AsyncReader>> {
        let mut reader = crate::pty_host::remote::RemoteReader::new(conn.clone(), read_socket)
            .map_err(|e| anyhow!("failed to create a remote async reader: {e}"))?
            // The remote counterpart of the supervisor's reader thread: the
            // one thing that will tell the pty thread this pane has ended.
            .on_exit(terminal_id, cmd, terminal_exit_callback(quit_cb));
        if let Some(cb) = self.agent_report_cb() {
            reader = reader.on_agent_report(cb);
        }
        let reader = Box::new(reader) as Box<dyn AsyncReader>;
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
    pub fn is_remote(&self, terminal_id: TerminalId) -> bool {
        self.terminal_id_to_transport
            .lock()
            .ok()
            .and_then(|t| t.get(&terminal_id).map(|t| t.remote().is_some()))
            .unwrap_or(false)
    }

    /// Whether a pane runs in a Firecracker guest rather than on an ordinary
    /// remote host. Takes the transport lock, so ask only when the answer is
    /// needed, not on every chunk of output.
    pub fn is_firecracker(&self, terminal_id: TerminalId) -> bool {
        self.terminal_id_to_transport
            .lock()
            .ok()
            .and_then(|t| {
                t.get(&terminal_id)
                    .and_then(|t| t.remote())
                    .map(|conn| conn.cmd().target.is_firecracker())
            })
            .unwrap_or(false)
    }

    /// The last thing the broker reported about a remote pane, or `None` for a
    /// local one.
    ///
    /// Cached rather than queried: the caller is `update_and_report_cwds` on the
    /// pty thread, and a round trip over a link that may be dead is not
    /// something that thread can afford.
    pub fn remote_status(&self, terminal_id: TerminalId) -> Option<RemoteStatus> {
        self.terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .map(|conn| conn.status())
    }

    pub fn workflow_remote_panes(&self) -> Vec<serde_json::Value> {
        let Ok(transports) = self.terminal_id_to_transport.lock() else {
            return vec![];
        };
        transports
            .iter()
            .filter_map(|(id, transport)| {
                let conn = transport.remote()?;
                Some(serde_json::json!({
                    "terminal_id": id,
                    "host_id": format!("remote:{}:{}", conn.host(), conn.pane_token()),
                    "remote_host": conn.host(), "pane_token": conn.pane_token(),
                    "owner": conn.cmd().workflow_owner(),
                }))
            })
            .collect()
    }

    /// Every pane whose process lives on another machine.
    pub fn remote_pane_ids(&self) -> Vec<TerminalId> {
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
    pub fn remote_git_probe(
        &self,
        terminal_id: TerminalId,
        cwd: &str,
        refresh_token: Option<&str>,
    ) -> Result<Option<RemoteGitProbe>, String> {
        let conn = self
            .terminal_id_to_transport
            .lock()
            .map_err(|e| e.to_string())?
            .get(&terminal_id)
            .and_then(|transport| transport.remote())
            .cloned()
            .ok_or_else(|| format!("terminal pane {terminal_id} is not remote"))?;
        let mut probe = conn
            .git_probe_with_refresh(cwd, refresh_token)
            .map_err(|e| e.to_string())?;
        if let (Some(probe), Some(host)) = (&mut probe, conn.cmd().display_host()) {
            probe.host = host;
        }
        Ok(probe)
    }

    /// The machine a pane runs on, or `None` if it runs on this one.
    pub fn remote_host(&self, terminal_id: TerminalId) -> Option<String> {
        self.terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .and_then(|conn| conn.cmd().display_host())
    }

    /// The host worktree a Firecracker pane's VM mirrors, off the transport's own
    /// command — which is the one `resolve_target` resolved, so it carries the worktree
    /// the provider named even when the caller asked for none.
    pub fn remote_worktree(&self, terminal_id: TerminalId) -> Option<PathBuf> {
        self.terminal_id_to_transport
            .lock()
            .ok()?
            .get(&terminal_id)?
            .remote()
            .and_then(|conn| conn.cmd().target.firecracker_worktree())
            .map(std::path::Path::to_path_buf)
    }

    /// Deliver a signal to a remote pane's child, by name rather than by pid.
    pub fn signal_remote_child(&self, terminal_id: TerminalId, signal: ChildSignal) -> Result<()> {
        let conn = self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .get(&terminal_id)
            .and_then(|t| t.remote().cloned())
            .ok_or_else(|| anyhow!("pane {terminal_id} is not a remote pane"))?;
        conn.signal_child(signal)
    }

    pub fn reserve_terminal_id(&self, terminal_id: TerminalId) {
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
    pub fn forget_reserved_terminal_id(&self, terminal_id: TerminalId) {
        self.terminal_id_to_transport
            .lock()
            .unwrap()
            .remove(&terminal_id);
    }

    pub fn clear_terminal_id(&self, terminal_id: TerminalId) -> Result<()> {
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
            conn.terminate()
                .with_context(|| format!("failed to terminate remote pane {terminal_id}"))?;
        }
        Ok(())
    }

    pub fn next_terminal_id(&self) -> Option<TerminalId> {
        Some(TerminalId::new(
            self.next_terminal_id_counter
                .fetch_add(1, Ordering::Relaxed),
        ))
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
        terminal_id: TerminalId,
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
    pub fn check_terminal_can_be_warped(&self, terminal_id: TerminalId) -> Result<()> {
        let remote = self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .get(&terminal_id)
            .and_then(|transport| transport.remote())
            .cloned();
        if let Some(conn) = remote {
            if conn.cmd().target.is_firecracker() {
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
        terminal_id: TerminalId,
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
        terminal_id: TerminalId,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        let pending = pty_host_client::attach(pty_host_client::AttachParams {
            control_socket,
            terminal_id,
            session_name: zellij_runtime::envs::get_session_name().ok(),
        })?;
        let attached = pending.finish(terminal_exit_callback(quit_cb), self.child_state_cb());
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
    pub terminal_id: TerminalId,
    pub reader: Box<dyn AsyncReader>,
    /// `None` for a pane adopted from another machine. Its child's pid is
    /// meaningful only there, and recording one here would put it in
    /// `id_to_child_pid`, where every consumer would act on it locally.
    pub child_pid: Option<ProcessId>,
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

    /// Not a test: the entry point `original_termios_reaches_both_local_pane_paths` re-execs
    /// this binary at, to run a real supervisor over the production fd-3 handoff. In every
    /// other run - which is every run of the suite - the variable is unset and this is a
    /// deliberate no-op, so it can never go red on its own.
    #[test]
    fn termios_supervisor_entry() {
        if std::env::var_os("ZELLIJ_TEST_TERMIOS_SUPERVISOR").is_some() {
            crate::pty_host::run_pty_host("termios-test/0".to_owned());
        }
        // Otherwise this is the suite proper, and the test below is what exercises this path.
    }

    #[tokio::test]
    async fn original_termios_reaches_both_local_pane_paths() {
        use termios::{InputFlags, LocalFlags, OutputFlags, SetArg, SpecialCharacterIndices};
        let reference = openpty(None, None).unwrap();
        let mut cooked = termios::tcgetattr(&reference.slave).unwrap();
        cooked.input_flags.insert(InputFlags::ICRNL);
        cooked.local_flags.insert(LocalFlags::ICANON);
        cooked.local_flags.remove(LocalFlags::ECHO);
        cooked.output_flags.remove(OutputFlags::OPOST);
        cooked.control_chars[SpecialCharacterIndices::VERASE as usize] = 8;
        termios::tcsetattr(&reference.slave, SetArg::TCSANOW, &cooked).unwrap();
        let snapshot = WireTermios::from_termios(&termios::tcgetattr(&reference.slave).unwrap());
        let mut raw = cooked.clone();
        termios::cfmakeraw(&mut raw);
        raw.local_flags.insert(LocalFlags::ISIG);
        termios::tcsetattr(&reference.slave, SetArg::TCSANOW, &raw).unwrap();
        assert!(!termios::tcgetattr(&reference.slave)
            .unwrap()
            .input_flags
            .contains(InputFlags::ICRNL));
        let json = serde_json::to_string(&Some(snapshot.clone())).unwrap();
        let decoded: Option<WireTermios> = serde_json::from_str(&json).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let supervisor = dir.path().join("supervisor");
        let exe = std::env::current_exe().unwrap();
        // Re-exec the test binary as a real supervisor, with the production fd-3 handoff.
        zellij_test_support::write_warm_script(&supervisor, &format!(
            "ZELLIJ_TEST_TERMIOS_SUPERVISOR=1 exec '{}' --exact os_input_output_unix::tests::termios_supervisor_entry --nocapture\n",
            exe.display().to_string().replace('\'', "'\"'\"'")
        ));
        for detachable in [false, true] {
            let mut backend =
                UnixPtyBackend::with_termios(decoded.as_ref().map(WireTermios::to_termios))
                    .unwrap();
            backend.host_bin = Some(supervisor.clone());
            backend.pty_host_enabled.store(detachable, Ordering::SeqCst);
            let id = TerminalId::new(0);
            let (mut reader, pid) = backend
                .spawn_terminal(
                    RunCommand {
                        command: PathBuf::from("/bin/sh"),
                        args: vec![
                            "-c".to_owned(),
                            "printf 'ready\\n'; IFS= read -r answer; printf '%s\\n' \"$answer\""
                                .to_owned(),
                        ],
                        ..Default::default()
                    },
                    None,
                    Box::new(|_, _, _| {}),
                    id,
                )
                .unwrap();
            assert_eq!(
                backend
                    .terminal_id_to_host
                    .lock()
                    .unwrap()
                    .contains_key(&id),
                detachable,
                "the detachable path must not silently fall back"
            );
            let master = match backend
                .terminal_id_to_transport
                .lock()
                .unwrap()
                .get(&id)
                .unwrap()
            {
                PaneTransport::Local(fd) => *fd,
                _ => panic!("expected local PTY"),
            };
            let applied = termios::tcgetattr(unsafe { BorrowedFd::borrow_raw(master) }).unwrap();
            assert_eq!(WireTermios::from_termios(&applied), snapshot);
            let outcome = tokio::time::timeout(Duration::from_secs(10), async {
                let mut output = Vec::new();
                let mut buf = [0; 128];
                while !output.ends_with(b"ready\n") {
                    let n = reader.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0, "prompt exited before readiness");
                    output.extend_from_slice(&buf[..n]);
                }
                backend.write_to_tty_stdin(id, b"y\r").unwrap();
                output.clear();
                while !output.ends_with(b"y\n") {
                    let n = reader.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0, "prompt exited without reading the answer");
                    output.extend_from_slice(&buf[..n]);
                }
            })
            .await;
            backend.kill(pid).unwrap();
            backend.clear_terminal_id(id).unwrap();
            assert!(
                outcome.is_ok(),
                "Enter did not complete the line read (detachable={detachable})"
            );
        }
    }

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

        // Which bytes were accepted matters as much as how many: the caller re-queues
        // `&data[written..]`, so the kernel must hold exactly the prefix. Drain the slave
        // and check what arrived after the leftover fill is that prefix.
        let slave_flags = fcntl(borrowed_slave, FcntlArg::F_GETFL).expect("F_GETFL");
        let mut slave_oflags = OFlag::from_bits_truncate(slave_flags);
        slave_oflags.insert(OFlag::O_NONBLOCK);
        fcntl(borrowed_slave, FcntlArg::F_SETFL(slave_oflags)).expect("F_SETFL");
        let mut read_back = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match nix::unistd::read(borrowed_slave, &mut buf) {
                Ok(0) => break,
                Ok(n) => read_back.extend_from_slice(&buf[..n]),
                Err(nix::errno::Errno::EAGAIN) => break,
                Err(nix::errno::Errno::EINTR) => continue,
                Err(e) => panic!("draining the slave failed: {e}"),
            }
        }
        assert!(
            read_back.ends_with(&data[..written]),
            "the {written} accepted bytes are not the prefix of the buffer"
        );
        assert!(
            read_back.len() >= written,
            "read back {} bytes, fewer than the {written} reported written",
            read_back.len()
        );

        unsafe {
            libc::close(master_fd);
            libc::close(slave_fd);
        }
    }

    /// Only EAGAIN and a zero-length write mean "stop here and re-queue the rest". Any
    /// other errno is how `PtyWriter` learns a pane's fd is gone, and swallowing it would
    /// leave the pane's writes silently dropped forever.
    #[test]
    fn try_write_to_fd_propagates_an_error_that_is_not_eagain() {
        // A descriptor open for reading only fails every write with EBADF, and stays
        // owned for the whole call - no closed-fd-number reuse to worry about.
        let read_only = std::fs::File::open("/dev/null").expect("open /dev/null");
        let error = super::try_write_to_fd(read_only.as_raw_fd(), b"gone")
            .expect_err("a write to a read-only fd must be an error, not a short write");
        assert!(error.to_string().contains("EBADF"), "{error}");
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
            .insert(TerminalId::new(7), PaneTransport::Remote(Arc::new(conn)));

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let releasing = backend.clone();
        std::thread::spawn(move || {
            let outcome =
                releasing.release_terminal_for_warp(TerminalId::new(7), Duration::from_secs(1));
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
                .contains_key(&TerminalId::new(7)),
            "a released pane leaves the transport table whether or not the broker confirmed"
        );
    }
}

/// Adapt process terminal IDs at the server boundary.
fn terminal_exit_callback(
    callback: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
) -> crate::pty_host::client::QuitCb {
    Box::new(move |terminal_id, status, command| {
        callback(PaneId::Terminal(terminal_id), status, command);
    })
}

#[cfg(test)]
mod callback_tests {
    use super::*;

    #[test]
    fn terminal_exit_callback_preserves_id_status_and_command() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let callback = terminal_exit_callback(Box::new(move |id, status, command| {
            sender.send((id, status, command)).unwrap();
        }));
        let command = RunCommand {
            command: "/usr/bin/true".into(),
            args: vec!["argument".into()],
            cwd: Some("/tmp".into()),
            ..Default::default()
        };
        for status in [Some(17), None] {
            callback(TerminalId::new(42), status, command.clone());
            assert_eq!(
                receiver.recv().unwrap(),
                (
                    PaneId::Terminal(TerminalId::new(42)),
                    status,
                    command.clone()
                )
            );
        }
    }
}
