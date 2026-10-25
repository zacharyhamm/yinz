use crate::os_input_output::command_exists;
use crate::PaneId;
use yinz_bus::os_input_output::{AdoptedTerminal, AsyncReader};
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TerminalId;

use nix::{
    fcntl::{fcntl, FcntlArg, OFlag},
    pty::{OpenptyResult, Winsize},
    sys::{
        signal::{kill, Signal},
        termios,
        wait::{waitpid, WaitPidFlag, WaitStatus},
    },
    unistd,
};
use tokio::io::unix::AsyncFd;

use std::{
    collections::BTreeMap,
    io,
    os::fd::{BorrowedFd, OwnedFd},
    os::unix::{
        io::{AsRawFd, RawFd},
        process::CommandExt,
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use yinz_runtime::errors::prelude::*;
use yinz_types::input::command::RunCommand;

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

/// An `AsyncReader` sharing the master descriptor with the transport and writes.
///
/// Construction sets O_NONBLOCK but defers `AsyncFd` registration to the first
/// `read()` call, because `AsyncFd::new()` requires a live Tokio reactor and
/// `spawn_terminal` runs on the plain PTY thread (outside the runtime).
struct RawFdAsyncReader {
    /// Holds the descriptor before reactor registration; `None` after promotion.
    pending: Option<Arc<OwnedFd>>,
    /// Populated on first `read()` inside the Tokio runtime.
    async_fd: Option<AsyncFd<Arc<OwnedFd>>>,
}

impl RawFdAsyncReader {
    fn new(fd: Arc<OwnedFd>) -> io::Result<Self> {
        // Set O_NONBLOCK so AsyncFd can use epoll correctly
        let flags =
            fcntl(&fd, FcntlArg::F_GETFL).map_err(|e| io::Error::from_raw_os_error(e as i32))?;
        let mut oflags = OFlag::from_bits_truncate(flags);
        oflags.insert(OFlag::O_NONBLOCK);
        fcntl(&fd, FcntlArg::F_SETFL(oflags))
            .map_err(|e| io::Error::from_raw_os_error(e as i32))?;

        Ok(Self {
            pending: Some(fd),
            async_fd: None,
        })
    }

    /// Lazily register with the Tokio reactor on first use.
    fn get_async_fd(&mut self) -> io::Result<&mut AsyncFd<Arc<OwnedFd>>> {
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

/// Block until the child exits, reporting every stop and continue on the way.
///
/// `nix`'s `waitpid` reaps, so the caller's `Child` must not be waited on again.
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
) -> Result<(OwnedFd, ProcessId)> {
    let err_context = |cmd: &RunCommand| {
        format!(
            "failed to open PTY for command '{}'",
            cmd.command.to_string_lossy()
        )
    };

    // primary side of pty and child fd
    let master = open_pty_res.master;
    let slave = open_pty_res.slave;
    let pid_secondary = slave.as_raw_fd();

    if !command_exists(&cmd) {
        return Err(YinzError::CommandNotFound {
            terminal_id,
            command: cmd.command.to_string_lossy().to_string(),
        })
        .with_context(|| err_context(&cmd));
    }

    let mut command =
        yinz_process::command::prepare_command(&cmd).with_context(|| err_context(&cmd))?;
    let child = unsafe {
        command
            // The server may itself be running in a supervised pane. This
            // in-process fallback has no stable supervisor reference of its own,
            // and no broker token either: a server running inside a remote pane
            // would otherwise hand every pane the outer pane's token, and a hook
            // reads that before the pane id.
            .env_remove(crate::pty_host::PANE_REF_ENV_KEY)
            .env_remove(crate::pty_host::remote::broker::PANE_TOKEN_ENV)
            // Structured launches set this again through their env wrapper.
            .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
            .env("YINZ_PANE_ID", format!("{}", terminal_id))
            .pre_exec(move || crate::pty_host::terminal::prepare_child(pid_secondary, false))
            .spawn()
            .map_err(|err| YinzError::PaneStartFailed {
                terminal_id,
                message: format!("failed to spawn command '{}': {err}", cmd.command.display()),
            })
            .with_context(|| err_context(&cmd))?
    };

    let child_id = ProcessId::new(child.id());
    thread::spawn(move || {
        // `Child::wait` is `waitpid` without `WUNTRACED`, so it cannot tell a
        // stopped child from a busy one — and a `^Z`'d command pane looks
        // exactly like a hang. This waiter reaps the child and owns its exit status.
        let exit_status = wait_reporting_stops(child_id, terminal_id, child_state_cb.as_ref());
        drop(slave);
        quit_cb(PaneId::Terminal(terminal_id), exit_status, cmd);
    });

    Ok((master, child_id))
}

/// Spawns a new terminal from the parent terminal with [`termios`](termios::Termios)
/// `orig_termios`.
fn handle_terminal(
    cmd: RunCommand,
    orig_termios: Option<termios::Termios>,
    quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    child_state_cb: Option<ChildStateCb>,
    terminal_id: TerminalId,
) -> Result<(OwnedFd, ProcessId)> {
    let open_pty_res = crate::pty_host::terminal::open(None, orig_termios.as_ref())
        .with_context(|| format!("failed to open PTY for command '{}'", cmd.command.display()))
        .context("failed to start pty")
        .context("failed to spawn child terminal")
        .to_log()?;
    handle_openpty(open_pty_res, cmd, quit_cb, child_state_cb, terminal_id)
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
    /// Shared with the reader and in-flight writes so aborting a reader cannot
    /// leave a recycled descriptor number in the transport table.
    Local(Arc<OwnedFd>),
    /// A pty owned by a broker on another machine. There is no descriptor and no
    /// usable pid on this side; every operation is a frame on the connection.
    Remote(Arc<RemoteConn>),
}

impl PaneTransport {
    /// An owning reference to the local master descriptor, if this pane has one.
    ///
    /// Only for operations that are genuinely fd-shaped and have no meaning
    /// otherwise. Prefer matching exhaustively where a non-local pane needs its
    /// own answer rather than "no fd".
    fn local_fd(&self) -> Option<Arc<OwnedFd>> {
        match self {
            PaneTransport::Reserved | PaneTransport::Remote(_) => None,
            PaneTransport::Local(fd) => Some(fd.clone()),
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
    // Serialize ownership changes through publication of a renamed session.
    ownership: Arc<Mutex<()>>,
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
    /// Populated only during an acknowledged shutdown, then consumed by its waiter.
    shutdown_processes: Arc<Mutex<Option<Vec<ProcessId>>>>,
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

use yinz_runtime::own_binary::server_binary;

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
            ownership: Arc::new(Mutex::new(())),
            orig_termios: Arc::new(Mutex::new(current_termios)),
            terminal_id_to_transport: Arc::new(Mutex::new(BTreeMap::new())),
            next_terminal_id_counter: Arc::new(AtomicU32::new(0)),
            pty_host_enabled: Arc::new(AtomicBool::new(false)),
            host_bin,
            wire_termios,
            terminal_id_to_host: Arc::new(Mutex::new(BTreeMap::new())),
            shutdown_processes: Arc::new(Mutex::new(None)),
            child_state_cb: Arc::new(Mutex::new(None)),
            agent_report_cb: Arc::new(Mutex::new(None)),
        })
    }

    /// Called once at session init, when config is finally available. Env
    /// overrides win, because the config path needs a live session and is
    /// therefore useless for bisecting a startup problem.
    pub fn set_pty_host_enabled(&self, enabled: bool) {
        let enabled = if std::env::var_os("YINZ_DISABLE_PTY_HOST").is_some() {
            false
        } else if std::env::var_os("YINZ_ENABLE_PTY_HOST").is_some() {
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
            self.record_shutdown_processes(&handle).non_fatal();
            handle.terminate();
        }
    }

    /// Track only this shutdown, including panes still being spawned during teardown.
    pub fn begin_confirmed_pty_host_shutdown(&self) -> Result<()> {
        let hosts = self.terminal_id_to_host.lock().to_anyhow()?;
        *self.shutdown_processes.lock().to_anyhow()? = Some(
            hosts
                .values()
                .flat_map(|host| [host.host_pid, host.child_pid])
                .collect(),
        );
        Ok(())
    }

    fn record_shutdown_processes(&self, handle: &PtyHostHandle) -> Result<()> {
        if let Some(processes) = self.shutdown_processes.lock().to_anyhow()?.as_mut() {
            processes.extend([handle.host_pid, handle.child_pid]);
        }
        Ok(())
    }

    pub fn confirm_pty_host_shutdown(&self) -> Result<()> {
        let mut pending = self
            .shutdown_processes
            .lock()
            .to_anyhow()?
            .take()
            .unwrap_or_default();
        pending.sort_unstable();
        pending.dedup();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !pending.is_empty() {
            pending.retain(|pid| {
                crate::pty_host::process_pid(*pid)
                    .map(|pid| nix::sys::signal::kill(pid, None) != Err(nix::errno::Errno::ESRCH))
                    .unwrap_or(true)
            });
            if pending.is_empty() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                bail!("owned pane teardown unconfirmed for processes {pending:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Terminate every supervisor this server still owns after pane teardown.
    pub fn shutdown_pty_hosts(&self) {
        let Ok(_ownership) = self.ownership.lock() else {
            return;
        };
        let handles: Vec<PtyHostHandle> = match self.terminal_id_to_host.lock() {
            Ok(mut hosts) => std::mem::take(&mut *hosts).into_values().collect(),
            Err(_) => return,
        };
        for mut handle in handles {
            self.record_shutdown_processes(&handle).non_fatal();
            handle.terminate();
        }
    }

    pub fn rename_pty_host_session(
        &self,
        old: &yinz_types::internal_ids::SessionName,
        new: &yinz_types::internal_ids::SessionName,
    ) -> Result<()> {
        self.rename_pty_host_session_in(&crate::pty_host::protocol::socket_dir(), old, new)
    }

    fn rename_pty_host_session_in(
        &self,
        dir: &std::path::Path,
        old: &yinz_types::internal_ids::SessionName,
        new: &yinz_types::internal_ids::SessionName,
    ) -> Result<()> {
        let _ownership = self.ownership.lock().to_anyhow()?;
        let owned = self
            .terminal_id_to_host
            .lock()
            .to_anyhow()?
            .values()
            .map(|handle| {
                handle.control_socket.clone().with_context(|| {
                    format!("supervisor {} has no socket for relabeling", handle.host_id)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        crate::pty_host::cli::relabel_session_in(dir, &owned, old, new)?;
        // Spawn and adoption read this under the same lock. Publishing later on
        // the screen thread would leave a gap for a pane with the old name.
        yinz_runtime::envs::set_session_name(new.clone());
        Ok(())
    }

    fn spawn_via_pty_host(
        &self,
        host_bin: &std::path::Path,
        cmd: &RunCommand,
        terminal_id: TerminalId,
    ) -> Result<pty_host_client::PendingHost> {
        // Kept here rather than left to the supervisor so that a mistyped command
        // still produces a synchronous `YinzError::CommandNotFound` — which is
        // what `pane_start_failure` in pty.rs shows the user inside the pane
        // rather than dying on — without paying for a process spawn.
        if !command_exists(cmd) {
            return Err(YinzError::CommandNotFound {
                terminal_id,
                command: cmd.command.to_string_lossy().to_string(),
            }
            .into());
        }
        pty_host_client::spawn(SpawnParams {
            host_bin,
            terminal_id,
            session_name: yinz_runtime::envs::get_session_name().ok(),
            cmd: cmd.clone(),
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
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        terminal_id: TerminalId,
    ) -> Result<(Box<dyn AsyncReader>, ProcessId)> {
        let _ownership = self.ownership.lock().to_anyhow()?;
        // A re-run reuses the terminal_id, so an earlier supervisor for it would
        // otherwise be orphaned with nobody left holding its control channel.
        self.terminate_host(terminal_id);

        if self.pty_host_enabled.load(Ordering::Relaxed)
            && let Some(host_bin) = self.host_bin.as_ref()
        {
            match self.spawn_via_pty_host(host_bin, &cmd, terminal_id) {
                Ok(pending) => {
                    let spawned =
                        pending.finish(terminal_exit_callback(quit_cb), self.child_state_cb());
                    let master_fd = Arc::new(spawned.master);
                    let async_reader = Box::new(
                        RawFdAsyncReader::new(master_fd.clone())
                            .map_err(|e| anyhow!("failed to create async reader: {}", e))?,
                    ) as Box<dyn AsyncReader>;
                    self.terminal_id_to_transport
                        .lock()
                        .to_anyhow()?
                        .insert(terminal_id, PaneTransport::Local(master_fd));
                    self.terminal_id_to_host
                        .lock()
                        .to_anyhow()?
                        .insert(terminal_id, spawned.handle);
                    return Ok((async_reader, spawned.child_pid));
                },
                Err(e) => {
                    if e.downcast_ref::<YinzError>().is_some() {
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

        let orig_termios = self
            .orig_termios
            .lock()
            .to_anyhow()
            .context("failed to lock orig_termios")?;
        let (pid_primary, child_pid) = handle_terminal(
            cmd,
            orig_termios.clone(),
            quit_cb,
            self.child_state_cb(),
            terminal_id,
        )?;
        let pid_primary = Arc::new(pid_primary);
        let async_reader = Box::new(
            RawFdAsyncReader::new(pid_primary.clone())
                .map_err(|e| anyhow::anyhow!("failed to create async reader: {}", e))?,
        ) as Box<dyn AsyncReader>;
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .insert(terminal_id, PaneTransport::Local(pid_primary));
        Ok((async_reader, child_pid))
    }

    /// Tell the pane's pty its new size, whichever transport holds it.
    ///
    /// Return transport failures, including panes with no pty yet (reserved or unknown).
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
                    crate::pty_host::terminal::resize(
                        fd.as_raw_fd(),
                        &Winsize {
                            ws_col: cols,
                            ws_row: rows,
                            ws_xpixel: width_in_pixels.unwrap_or(0),
                            ws_ypixel: height_in_pixels.unwrap_or(0),
                        },
                    )
                    .with_context(err_context)?;
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
                    .with_context(err_context)?;
                }
            },
            Some(PaneTransport::Reserved) | None => {
                return Err(anyhow!("failed to find terminal fd for id {terminal_id}"))
                    .with_context(err_context);
            },
        }
        Ok(())
    }

    pub fn retain_agent_theme(
        &self,
        terminal_id: TerminalId,
        event: yinz_types::agent::theme::AgentThemeEvent,
    ) -> Result<()> {
        if let Some(host) = self
            .terminal_id_to_host
            .lock()
            .to_anyhow()
            .context("lock pane supervisors for palette update")?
            .get_mut(&terminal_id)
        {
            host.retain_agent_theme(event)
                .context("update supervised pane palette")?;
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
            Local(Arc<OwnedFd>),
            Remote(Arc<RemoteConn>),
        }
        let target = match self
            .terminal_id_to_transport
            .lock()
            .to_anyhow()
            .with_context(err_context)?
            .get(&terminal_id)
        {
            Some(PaneTransport::Local(fd)) => Target::Local(fd.clone()),
            Some(PaneTransport::Remote(conn)) => Target::Remote(conn.clone()),
            // The pane is already visible while its remote transport starts;
            // input typed into the loading screen is intentionally discarded.
            Some(PaneTransport::Reserved) => return Ok(buf.len()),
            None => {
                return Err(anyhow!("could not find raw file descriptor")).with_context(err_context)
            },
        };

        match target {
            Target::Local(fd) => try_write_to_fd(fd.as_raw_fd(), buf).with_context(err_context),
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
            Some(PaneTransport::Local(fd)) => termios::tcdrain(fd).with_context(err_context),
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
        unistd::tcgetpgrp(fd).ok().map(|pgid| pgid.as_raw())
    }

    pub fn kill(&self, pid: ProcessId) -> Result<()> {
        let _ = kill(crate::pty_host::process_pid(pid)?, Some(Signal::SIGHUP));
        Ok(())
    }

    pub fn force_kill(&self, pid: ProcessId) -> Result<()> {
        crate::pty_host::terminal::signal_group_or_leader(pid, Signal::SIGKILL)
    }

    pub fn send_sigint(&self, pid: ProcessId) -> Result<()> {
        crate::pty_host::terminal::signal_group_or_leader(pid, Signal::SIGINT)
    }

    /// Continue a stopped pane, the way a shell's `fg` does.
    ///
    /// Aimed at the *process group*, not the pid: `login_tty` made the child a
    /// session leader with `pgid == pid`, and `^Z` stopped everything in its
    /// foreground group, not just the leader. Continuing only the leader would
    /// leave the rest of a pipeline asleep. The bare pid is a fallback for the
    /// case where the child never got a group of its own.
    pub fn send_sigcont(&self, pid: ProcessId) -> Result<()> {
        crate::pty_host::terminal::signal_group_or_leader(pid, Signal::SIGCONT)
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
        let _ownership = self.ownership.lock().to_anyhow()?;
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
    /// The reader and transport share ownership of the master. Removing the
    /// transport closes it only after all in-flight reads and writes release it.
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

        let _ownership = self.ownership.lock().to_anyhow()?;
        let handle = self
            .terminal_id_to_host
            .lock()
            .to_anyhow()?
            .remove(&terminal_id)
            .ok_or_else(|| {
                anyhow!(
                    "terminal_{terminal_id} has no transferable pty-host supervisor; its process cannot be safely moved"
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
                "terminal_{terminal_id} has no transferable pty-host supervisor; its process cannot be safely moved"
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
        let _ownership = self.ownership.lock().to_anyhow()?;
        let pending = pty_host_client::attach(pty_host_client::AttachParams {
            control_socket,
            terminal_id,
            session_name: yinz_runtime::envs::get_session_name().ok(),
        })?;
        let attached = pending.finish(terminal_exit_callback(quit_cb), self.child_state_cb());
        let master_fd = Arc::new(attached.master);
        let reader = Box::new(
            RawFdAsyncReader::new(master_fd.clone())
                .map_err(|e| anyhow!("failed to create async reader: {}", e))?,
        ) as Box<dyn AsyncReader>;
        self.terminal_id_to_transport
            .lock()
            .to_anyhow()?
            .insert(terminal_id, PaneTransport::Local(master_fd));
        self.terminal_id_to_host
            .lock()
            .to_anyhow()?
            .insert(terminal_id, attached.handle);
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

#[cfg(test)]
#[path = "unit/pty_descriptor_tests.rs"]
mod pty_descriptor_tests;

#[cfg(test)]
#[path = "unit/os_input_output_unix_tests.rs"]
mod tests;

/// Adapt process terminal IDs at the server boundary.
fn terminal_exit_callback(
    callback: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
) -> crate::pty_host::client::QuitCb {
    Box::new(move |terminal_id, status, command| {
        callback(PaneId::Terminal(terminal_id), status, command);
    })
}

#[cfg(test)]
#[path = "unit/terminal_exit_callback_tests.rs"]
mod callback_tests;
