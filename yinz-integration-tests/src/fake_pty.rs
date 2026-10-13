use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use yinz_server::os_input_output::AsyncReader;
use yinz_server::panes::PaneId;
use yinz_types::internal_ids::TerminalId;
use {yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction};

pub(crate) type QuitCb = Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>;

pub(crate) const FAKE_PID_BASE: u32 = 100_000;

pub(crate) struct FakePtyState {
    warp_socket: Option<PathBuf>,
    refuse_next_adoption: bool,
    pub terminal_action: Option<TerminalAction>,
    pub output_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    pub stdin: Vec<u8>,
    pub size: Option<(u16, u16)>,
    pub pixel_size: Option<(u16, u16)>,
    pub size_history: Vec<(u16, u16)>,
    pub quit_cb: Option<QuitCb>,
    pub exited: bool,
    pub echo: bool,
    pub stdin_tap: Option<crossbeam::channel::Sender<Vec<u8>>>,
    /// The pane's own child process, as sysinfo would report it: the command the
    /// pane was spawned with. A real `get_cwds` answers this for every pane, so a
    /// fake that answered nothing left the whole command path dark.
    pub process_command: Option<Vec<String>>,
    /// What holds the tty foreground, as `tcgetpgrp` plus sysinfo would report it.
    /// `None` is "nothing but the pane's own process", which is what a shell
    /// sitting at its prompt looks like.
    pub foreground_command: Option<Vec<String>>,
}

impl FakePtyState {
    fn run_command(&self) -> RunCommand {
        match &self.terminal_action {
            Some(TerminalAction::RunCommand(run_command)) => run_command.clone(),
            _ => RunCommand::default(),
        }
    }

    fn describe(&self) -> String {
        format!(
            "size={:?} exited={} stdin_bytes={}",
            self.size,
            self.exited,
            self.stdin.len()
        )
    }
}

#[derive(Default)]
pub(crate) struct FakePtyRegistry {
    next_terminal_id: TerminalId,
    pub fake_pty_states: HashMap<TerminalId, FakePtyState>,
    pub spawn_queue: VecDeque<TerminalId>,
    /// Terminals this session took over from another one. Kept apart from
    /// `spawn_queue` because nothing was spawned: the whole claim a warp makes is
    /// that the process was already running and went on running.
    pub adopted_queue: VecDeque<TerminalId>,
    /// How many panes this session let go of, and took up, by the *remote* road.
    ///
    /// Counted because a warp that took the wrong road still works in this
    /// harness: the pane's state travels either way, so everything else a test
    /// can ask about the pane that arrived — its command, its host, its
    /// scrollback — reads the same whether it went by a supervisor socket or a
    /// broker token. These are the only witnesses to which branch actually ran,
    /// and without them a remote pane quietly sent down the local path would
    /// pass every assertion.
    ///
    /// Per-session rather than process-global, so two tests warping at once
    /// cannot see each other's, and so a test can say which *end* did what.
    pub remote_releases: usize,
    pub remote_adoptions: usize,
    /// Panes this session will refuse to release, and how it refuses.
    ///
    /// `Unwarpable` is the answer a real backend gives before anything has been
    /// touched — no supervisor, or one that never bound a socket — and is what a
    /// test uses to check that a refused warp leaves a working pane. `Unreleasable`
    /// is the harder one: the pre-flight says yes and the release then fails, which
    /// is the shape of a remote broker that will not confirm, and the only case
    /// where a pane really is cut loose.
    pub warp_failures: HashMap<TerminalId, WarpFailure>,
}

/// How a test makes a release go wrong. See `FakePtyRegistry::warp_failures`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WarpFailure {
    /// Refused by the pre-flight check, before anything is released.
    Unwarpable,
    /// Passes the pre-flight check and then fails to release.
    Unreleasable,
    /// Refuse one attach, then allow rollback to take the process back.
    UnadoptableOnce,
}

/// Supervisors released by one session and not yet adopted by another.
///
/// Process-global, because the two sessions in a warp test have a registry each
/// — exactly as two real servers have a pty host each — and the pane has to
/// exist in neither for the moment it is between them. Keyed by control socket
/// path, which is the address the receiving session is handed and the only thing
/// it is told about the pane.
fn detached_supervisors() -> &'static Mutex<HashMap<PathBuf, FakePtyState>> {
    static DETACHED_SUPERVISORS: OnceLock<Mutex<HashMap<PathBuf, FakePtyState>>> = OnceLock::new();
    DETACHED_SUPERVISORS.get_or_init(Default::default)
}

/// An address no two releases share, so a test running several warps at once
/// cannot hand one pane's supervisor to the wrong session.
fn next_control_socket() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    PathBuf::from(format!(
        "/fake-pty-host/warp-{}.sock",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Where a released remote pane is parked, keyed by the token rather than by a
/// socket path.
///
/// A path only because the parking map is one; the value is never a filesystem
/// path and the prefix is there to make that obvious in a failure message.
fn remote_parking_key(pane_token: &str) -> PathBuf {
    PathBuf::from(format!("/fake-broker/{pane_token}"))
}

impl FakePtyRegistry {
    fn describe_all(&self) -> String {
        let mut fake_pty_state_lines: Vec<String> = self
            .fake_pty_states
            .iter()
            .map(|(terminal_id, fake_pty_state)| {
                format!("terminal {}: {}", terminal_id, fake_pty_state.describe())
            })
            .collect();
        fake_pty_state_lines.sort();
        format!(
            "{}\nspawn queue: {:?}",
            fake_pty_state_lines.join("\n"),
            self.spawn_queue
        )
    }
}

#[derive(Default)]
struct RegistryWithChangeSignal {
    fake_pty_registry: Mutex<FakePtyRegistry>,
    change_signal: Condvar,
}

#[derive(Clone, Default)]
pub struct SharedPtys {
    inner: Arc<RegistryWithChangeSignal>,
}

impl SharedPtys {
    fn lock_registry(&self) -> MutexGuard<'_, FakePtyRegistry> {
        self.inner.fake_pty_registry.lock().unwrap()
    }

    pub(crate) fn read<T>(&self, reader: impl FnOnce(&FakePtyRegistry) -> T) -> T {
        reader(&self.lock_registry())
    }

    pub(crate) fn mutate<T>(&self, mutator: impl FnOnce(&mut FakePtyRegistry) -> T) -> T {
        let result = mutator(&mut self.lock_registry());
        self.inner.change_signal.notify_all();
        result
    }

    pub(crate) fn next_terminal_id(&self) -> TerminalId {
        self.mutate(|fake_pty_registry| {
            let terminal_id = fake_pty_registry.next_terminal_id;
            fake_pty_registry.next_terminal_id = TerminalId::new(
                terminal_id
                    .get()
                    .checked_add(1)
                    .expect("fake terminal IDs exhausted"),
            );
            terminal_id
        })
    }

    pub(crate) fn register(
        &self,
        terminal_id: TerminalId,
        terminal_action: Option<TerminalAction>,
        quit_cb: Option<QuitCb>,
    ) -> Box<dyn AsyncReader> {
        let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel();
        let process_command = match &terminal_action {
            Some(TerminalAction::RunCommand(run_command)) => Some(
                std::iter::once(run_command.command.to_string_lossy().into_owned())
                    .chain(run_command.args.iter().cloned())
                    .collect(),
            ),
            _ => None,
        };
        let spawned = terminal_action.is_some();
        self.mutate(|fake_pty_registry| {
            fake_pty_registry.fake_pty_states.insert(
                terminal_id,
                FakePtyState {
                    warp_socket: None,
                    refuse_next_adoption: false,
                    terminal_action,
                    output_tx: Some(output_tx),
                    stdin: Vec::new(),
                    size: None,
                    pixel_size: None,
                    size_history: Vec::new(),
                    quit_cb,
                    exited: false,
                    echo: true,
                    stdin_tap: None,
                    process_command,
                    foreground_command: None,
                },
            );
            if spawned {
                fake_pty_registry.spawn_queue.push_back(terminal_id);
            }
        });
        Box::new(FakeAsyncReader {
            output_rx,
            pending: VecDeque::new(),
        })
    }

    /// Hand a pane's supervisor over without killing it.
    ///
    /// The state moves out of this session's registry into the detached one,
    /// which is what makes it stop being this session's pane while leaving the
    /// fake process exactly as it was — the stdin it has been sent, the sizes it
    /// has been told about, the command it was spawned with, whether it has
    /// exited. That everything survives *unchanged* is the whole claim a warp
    /// makes, so the fake models it by moving the state rather than rebuilding it.
    ///
    /// `output_tx` travels with it and is replaced on the far side. Dropping it
    /// would be an EOF to the reader, but `Pty::release_terminal_for_warp` aborts
    /// the reader task before this returns, so there is nobody left to hear it.
    /// Whether this pane could be released, asked before anything is let go.
    ///
    /// The real backends answer this out of the maps the release would use, so
    /// the fake answers it out of the registry the release would use: a pane it
    /// has no state for cannot be handed over. `WarpFailure::Unwarpable` is a
    /// test saying "answer no", which is the branch that must leave the pane
    /// working.
    pub(crate) fn check_can_be_warped(
        &self,
        terminal_id: TerminalId,
    ) -> std::result::Result<(), String> {
        self.read(|registry| {
            if registry.warp_failures.get(&terminal_id) == Some(&WarpFailure::Unwarpable) {
                return Err(format!(
                    "pane {terminal_id} has no pty-host supervisor, so it cannot be moved to \
                     another session"
                ));
            }
            if !registry.fake_pty_states.contains_key(&terminal_id) {
                return Err(format!("no fake pty for terminal id {terminal_id}"));
            }
            Ok(())
        })
    }

    /// Whether a test has asked this pane's release to fail after the check
    /// passed.
    pub(crate) fn release_is_rigged_to_fail(&self, terminal_id: TerminalId) -> bool {
        self.read(|registry| {
            registry.warp_failures.get(&terminal_id) == Some(&WarpFailure::Unreleasable)
        })
    }

    /// Make a pane's warp fail, from a test.
    pub fn fail_warp_of(&self, terminal_id: TerminalId, failure: WarpFailure) {
        self.mutate(|registry| registry.warp_failures.insert(terminal_id, failure));
    }

    pub(crate) fn release_for_warp(&self, terminal_id: TerminalId) -> Option<PathBuf> {
        let mut fake_pty_state = self.mutate(|registry| {
            let mut state = registry.fake_pty_states.remove(&terminal_id)?;
            state.refuse_next_adoption |=
                registry.warp_failures.remove(&terminal_id) == Some(WarpFailure::UnadoptableOnce);
            Some(state)
        })?;
        let control_socket = fake_pty_state
            .warp_socket
            .get_or_insert_with(next_control_socket)
            .clone();
        detached_supervisors()
            .lock()
            .unwrap()
            .insert(control_socket.clone(), fake_pty_state);
        Some(control_socket)
    }

    /// The machine a pane's command names, if it is not this one.
    ///
    /// The fake runs no processes anywhere, let alone on another host, so a
    /// "remote" pane here is one whose `RunCommand` says it is. That is enough
    /// for the thing worth testing: which of the two addressing shapes a warp
    /// builds, and whether the receiving session can follow it back.
    pub(crate) fn remote_host_of(&self, terminal_id: TerminalId) -> Option<String> {
        self.read(|registry| {
            registry
                .fake_pty_states
                .get(&terminal_id)
                .and_then(|state| state.run_command().target.display())
        })
    }

    /// The command a pane was spawned with, for the warp payload to carry.
    pub(crate) fn run_command_of(&self, terminal_id: TerminalId) -> Option<RunCommand> {
        self.read(|registry| {
            registry
                .fake_pty_states
                .get(&terminal_id)
                .map(|state| state.run_command())
        })
    }

    /// Park a remote pane under its broker token instead of a socket path.
    ///
    /// The same parking spot as `release_for_warp`, keyed differently — which is
    /// exactly the real difference. A remote pane's state does not move to a
    /// local supervisor socket; it stays where it is, and what travels is the
    /// name the broker knows it by.
    pub(crate) fn release_remote_for_warp(&self, terminal_id: TerminalId) -> Option<String> {
        let fake_pty_state = self.mutate(|fake_pty_registry| {
            let fake_pty_state = fake_pty_registry.fake_pty_states.remove(&terminal_id)?;
            fake_pty_registry.remote_releases += 1;
            Some(fake_pty_state)
        })?;
        let pane_token = format!("fake-token-{terminal_id}");
        detached_supervisors()
            .lock()
            .unwrap()
            .insert(remote_parking_key(&pane_token), fake_pty_state);
        Some(pane_token)
    }

    pub(crate) fn adopt_remote_for_warp(
        &self,
        pane_token: &str,
        quit_cb: QuitCb,
    ) -> Option<AdoptedFakePty> {
        let adopted = self.adopt_parked(&remote_parking_key(pane_token), quit_cb)?;
        self.mutate(|fake_pty_registry| fake_pty_registry.remote_adoptions += 1);
        Some(adopted)
    }

    /// How many panes this session released, and adopted, as remote ones.
    pub(crate) fn remote_warp_counts(&self) -> (usize, usize) {
        self.read(|fake_pty_registry| {
            (
                fake_pty_registry.remote_releases,
                fake_pty_registry.remote_adoptions,
            )
        })
    }

    /// Take a detached supervisor over into this session under a fresh id.
    ///
    /// The new id is this session's — ids are per-server and the receiving one
    /// knows nothing of the sender's numbering — but everything the id names is
    /// the same fake process it was before.
    pub(crate) fn adopt_for_warp(
        &self,
        control_socket: &Path,
        quit_cb: QuitCb,
    ) -> Option<AdoptedFakePty> {
        self.adopt_parked(control_socket, quit_cb)
    }

    fn adopt_parked(&self, key: &Path, quit_cb: QuitCb) -> Option<AdoptedFakePty> {
        let mut fake_pty_state = {
            let mut detached = detached_supervisors().lock().unwrap();
            if std::mem::take(&mut detached.get_mut(key)?.refuse_next_adoption) {
                return None;
            }
            detached.remove(key)?
        };
        let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel();
        let run_command = fake_pty_state.run_command();
        let child_exit_status = if fake_pty_state.exited {
            Some(None)
        } else {
            None
        };
        fake_pty_state.output_tx = Some(output_tx);
        fake_pty_state.quit_cb = Some(quit_cb);
        let terminal_id = self.next_terminal_id();
        // A child that exited while nobody was attached is reported again to the
        // adopting server, exactly as a real supervisor does after `AttachAck`
        // (`pty_host::host::serve_attach`). From another thread, as the real
        // report arrives on the reader thread rather than inside the adoption.
        let already_exited = if fake_pty_state.exited {
            fake_pty_state
                .quit_cb
                .take()
                .map(|quit_cb| (quit_cb, fake_pty_state.run_command()))
        } else {
            None
        };
        self.mutate(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .insert(terminal_id, fake_pty_state);
            fake_pty_registry.adopted_queue.push_back(terminal_id);
        });
        if let Some((quit_cb, run_command)) = already_exited {
            std::thread::spawn(move || {
                quit_cb(PaneId::Terminal(terminal_id), None, run_command);
            });
        }
        Some(AdoptedFakePty {
            terminal_id,
            reader: Box::new(FakeAsyncReader {
                output_rx,
                pending: VecDeque::new(),
            }),
            run_command,
            child_exit_status,
        })
    }

    /// The next terminal this session adopted, waiting for one if need be.
    pub(crate) fn expect_adoption(&self) -> TerminalId {
        self.wait_for("a pane to be adopted from another session", |registry| {
            registry.adopted_queue.pop_front()
        })
    }

    pub(crate) fn rerun(
        &self,
        terminal_id: TerminalId,
        run_command: RunCommand,
        quit_cb: QuitCb,
    ) -> Box<dyn AsyncReader> {
        let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel();
        self.mutate(|fake_pty_registry| {
            let fake_pty_state = fake_pty_registry
                .fake_pty_states
                .get_mut(&terminal_id)
                .expect("re-run for unknown terminal id");
            let was_reserved = fake_pty_state.terminal_action.is_none();
            fake_pty_state.process_command = Some(
                std::iter::once(run_command.command.to_string_lossy().into_owned())
                    .chain(run_command.args.iter().cloned())
                    .collect(),
            );
            fake_pty_state.foreground_command = None;
            fake_pty_state.terminal_action = Some(TerminalAction::RunCommand(run_command));
            fake_pty_state.output_tx = Some(output_tx);
            fake_pty_state.quit_cb = Some(quit_cb);
            fake_pty_state.exited = false;
            // Reserving an ID is not a spawn. Report its first actual command here.
            if was_reserved {
                fake_pty_registry.spawn_queue.push_back(terminal_id);
            }
        });
        Box::new(FakeAsyncReader {
            output_rx,
            pending: VecDeque::new(),
        })
    }

    pub(crate) fn exit_terminal(&self, terminal_id: TerminalId, exit_status: Option<i32>) {
        let newly_exited = self.mutate(|fake_pty_registry| {
            let fake_pty_state = fake_pty_registry.fake_pty_states.get_mut(&terminal_id)?;
            if fake_pty_state.exited {
                return None;
            }
            fake_pty_state.exited = true;
            fake_pty_state.output_tx.take();
            Some((fake_pty_state.quit_cb.take(), fake_pty_state.run_command()))
        });
        if let Some((Some(quit_cb), run_command)) = newly_exited {
            quit_cb(PaneId::Terminal(terminal_id), exit_status, run_command);
        }
    }

    pub(crate) fn remove(&self, terminal_id: TerminalId) {
        self.mutate(|fake_pty_registry| {
            fake_pty_registry.fake_pty_states.remove(&terminal_id);
        });
    }

    pub(crate) fn set_pixel_size(&self, terminal_id: TerminalId, pixels: Option<(u16, u16)>) {
        self.mutate(|registry| {
            if let Some(terminal) = registry.fake_pty_states.get_mut(&terminal_id) {
                terminal.pixel_size = pixels;
            }
        });
    }

    pub(crate) fn set_size(&self, terminal_id: TerminalId, cols: u16, rows: u16) {
        self.mutate(|fake_pty_registry| {
            if let Some(fake_pty_state) = fake_pty_registry.fake_pty_states.get_mut(&terminal_id) {
                // like a real pty, setting the same size again is a no-op that will not make the
                // program inside the terminal react (the kernel only sends a SIGWINCH if the
                // winsize actually changed)
                if fake_pty_state.size != Some((cols, rows)) {
                    fake_pty_state.size_history.push((cols, rows));
                }
                fake_pty_state.size = Some((cols, rows));
            }
        });
    }

    pub(crate) fn set_stdin_tap(
        &self,
        terminal_id: TerminalId,
        sender: crossbeam::channel::Sender<Vec<u8>>,
    ) {
        self.mutate(|fake_pty_registry| {
            if let Some(fake_pty_state) = fake_pty_registry.fake_pty_states.get_mut(&terminal_id) {
                fake_pty_state.stdin_tap = Some(sender);
            }
        });
    }

    /// What each pane's own process is, keyed by terminal id — the fake's answer
    /// to the sysinfo half of `get_cwds`.
    pub(crate) fn process_commands(&self) -> HashMap<TerminalId, Vec<String>> {
        self.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .iter()
                .filter_map(|(terminal_id, fake_pty_state)| {
                    Some((*terminal_id, fake_pty_state.process_command.clone()?))
                })
                .collect()
        })
    }

    /// What holds each pane's tty foreground, keyed by terminal id — the fake's
    /// answer to `get_foreground_cmds`.
    pub(crate) fn foreground_commands(&self) -> HashMap<TerminalId, Vec<String>> {
        self.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .iter()
                .filter_map(|(terminal_id, fake_pty_state)| {
                    Some((*terminal_id, fake_pty_state.foreground_command.clone()?))
                })
                .collect()
        })
    }

    pub(crate) fn set_foreground_command(
        &self,
        terminal_id: TerminalId,
        command: Option<Vec<String>>,
    ) {
        self.mutate(|fake_pty_registry| {
            if let Some(fake_pty_state) = fake_pty_registry.fake_pty_states.get_mut(&terminal_id) {
                fake_pty_state.foreground_command = command;
            }
        });
    }

    pub(crate) fn write_output(&self, terminal_id: TerminalId, bytes: &[u8]) {
        self.read(|fake_pty_registry| {
            if let Some(fake_pty_state) = fake_pty_registry.fake_pty_states.get(&terminal_id) {
                if let Some(output_tx) = fake_pty_state.output_tx.as_ref() {
                    let _ = output_tx.send(bytes.to_vec());
                }
            }
        });
    }

    pub(crate) fn append_stdin(&self, terminal_id: TerminalId, bytes: &[u8]) -> bool {
        self.mutate(|fake_pty_registry| {
            match fake_pty_registry.fake_pty_states.get_mut(&terminal_id) {
                Some(fake_pty_state) => {
                    fake_pty_state.stdin.extend_from_slice(bytes);
                    if fake_pty_state.echo {
                        if let Some(output_tx) = fake_pty_state.output_tx.as_ref() {
                            let _ = output_tx.send(bytes.to_vec());
                        }
                    }
                    if let Some(stdin_tap) = fake_pty_state.stdin_tap.as_ref() {
                        let _ = stdin_tap.send(bytes.to_vec());
                    }
                    true
                },
                None => false,
            }
        })
    }

    pub(crate) fn wait_for<T>(
        &self,
        what: &str,
        mut condition: impl FnMut(&mut FakePtyRegistry) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + crate::default_timeout();
        let mut fake_pty_registry = self.lock_registry();
        loop {
            if let Some(result) = condition(&mut fake_pty_registry) {
                return result;
            }
            let now = Instant::now();
            if now >= deadline {
                panic!(
                    "timed out waiting for: {}\n{}{}\n=== yinz log tail ({}) ===\n{}",
                    what,
                    fake_pty_registry.describe_all(),
                    crate::test_env::panics_so_far(),
                    crate::test_env::log_file_path().display(),
                    crate::test_env::log_tail(40),
                );
            }
            let (guard, _) = self
                .inner
                .change_signal
                .wait_timeout(fake_pty_registry, deadline - now)
                .unwrap();
            fake_pty_registry = guard;
        }
    }

    pub(crate) fn wait_for_change(
        &self,
        terminal_id: TerminalId,
        last_seen: (u16, u16),
    ) -> Option<(u16, u16)> {
        let mut fake_pty_registry = self.lock_registry();
        loop {
            {
                let fake_pty_state = fake_pty_registry.fake_pty_states.get(&terminal_id)?;
                if let Some((cols, rows)) = fake_pty_state.size {
                    if (cols, rows) != last_seen {
                        return Some((cols, rows));
                    }
                }
            }
            let (guard, _) = self
                .inner
                .change_signal
                .wait_timeout(fake_pty_registry, Duration::from_millis(200))
                .unwrap();
            fake_pty_registry = guard;
        }
    }
}

/// What a session gets back for taking a detached supervisor over.
pub(crate) struct AdoptedFakePty {
    pub terminal_id: TerminalId,
    pub reader: Box<dyn AsyncReader>,
    pub run_command: RunCommand,
    pub child_exit_status: Option<Option<i32>>,
}

struct FakeAsyncReader {
    output_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    pending: VecDeque<u8>,
}

#[async_trait]
impl AsyncReader for FakeAsyncReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        if self.pending.is_empty() {
            match self.output_rx.recv().await {
                Some(bytes) => self.pending.extend(bytes),
                None => return Ok(0),
            }
        }
        let mut written = 0;
        while written < buf.len() {
            match self.pending.pop_front() {
                Some(byte) => {
                    buf[written] = byte;
                    written += 1;
                },
                None => break,
            }
        }
        Ok(written)
    }
}

#[derive(Clone)]
pub struct FakePtyHandle {
    pub(crate) terminal_id: TerminalId,
    pub(crate) shared_ptys: SharedPtys,
}

impl FakePtyHandle {
    pub fn wait_for_command(&self, command: &str) {
        self.shared_ptys.wait_for(command, |registry| {
            registry
                .fake_pty_states
                .get(&self.terminal_id)
                .filter(|state| state.run_command().command.to_string_lossy() == command)
                .map(|_| ())
        });
    }

    /// The command received by the fake process, including its latest rerun.
    pub fn run_command(&self) -> RunCommand {
        self.shared_ptys.run_command_of(self.terminal_id).unwrap()
    }

    pub fn terminal_id(&self) -> TerminalId {
        self.terminal_id
    }

    pub fn terminal_action(&self) -> Option<TerminalAction> {
        self.shared_ptys.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&self.terminal_id)
                .and_then(|fake_pty_state| fake_pty_state.terminal_action.clone())
        })
    }

    /// Make this pane's warp fail, in one of the two ways a real one can.
    ///
    /// See [`WarpFailure`]: the difference between them is the difference
    /// between a pane that goes on working and one that has been cut loose, and
    /// it is the thing worth testing.
    pub fn fail_warp(&self, failure: WarpFailure) {
        self.shared_ptys.fail_warp_of(self.terminal_id, failure);
    }

    pub fn disable_echo(&self) {
        self.shared_ptys.mutate(|fake_pty_registry| {
            if let Some(fake_pty_state) =
                fake_pty_registry.fake_pty_states.get_mut(&self.terminal_id)
            {
                fake_pty_state.echo = false;
            }
        });
    }

    pub fn output(&self, bytes: &[u8]) {
        self.shared_ptys.read(|fake_pty_registry| {
            let fake_pty_state = fake_pty_registry
                .fake_pty_states
                .get(&self.terminal_id)
                .unwrap_or_else(|| panic!("unknown terminal id {}", self.terminal_id));
            let output_tx = fake_pty_state
                .output_tx
                .as_ref()
                .unwrap_or_else(|| panic!("terminal {} already exited", self.terminal_id));
            let _ = output_tx.send(bytes.to_vec());
        })
    }

    pub fn try_output(&self, bytes: &[u8]) {
        self.shared_ptys.write_output(self.terminal_id, bytes);
    }

    /// Put a program in this pane's tty foreground, the way typing its name at a
    /// shell prompt would — which is what agent detection reads.
    ///
    /// A byte of output goes with it, and has to: the server only re-reads a
    /// pane's foreground command on a tick where that pane produced output
    /// (`pane_activity_flags` in `Pty::update_and_report_cwds`), so a silent
    /// change is a change nothing ever notices. The bytes are an SGR reset, which
    /// the grid parses and draws nothing for.
    pub fn run_foreground(&self, command: &[&str]) {
        self.shared_ptys.set_foreground_command(
            self.terminal_id,
            Some(command.iter().map(|word| (*word).to_owned()).collect()),
        );
        self.stir();
    }

    /// Give the tty foreground back to the pane's own process — what the shell
    /// prompt returning after a program exits looks like from the outside.
    ///
    /// Emits output for the same reason `run_foreground` does.
    pub fn return_to_shell(&self) {
        self.shared_ptys
            .set_foreground_command(self.terminal_id, None);
        self.stir();
    }

    /// A byte that changes nothing on screen, purely to raise the pane's activity
    /// flag so the server's next tick looks at it.
    fn stir(&self) {
        self.shared_ptys.write_output(self.terminal_id, b"\x1b[0m");
    }

    pub fn tap_stdin(&self) -> crossbeam::channel::Receiver<Vec<u8>> {
        let (sender, receiver) = crossbeam::channel::unbounded();
        self.shared_ptys.set_stdin_tap(self.terminal_id, sender);
        receiver
    }

    pub fn exit(&self, exit_status: Option<i32>) {
        self.shared_ptys
            .exit_terminal(self.terminal_id, exit_status);
    }

    pub fn is_exited(&self) -> bool {
        self.shared_ptys.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&self.terminal_id)
                .map(|fake_pty_state| fake_pty_state.exited)
                .unwrap_or(true)
        })
    }

    pub fn stdin_bytes(&self) -> Vec<u8> {
        self.shared_ptys.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&self.terminal_id)
                .map(|fake_pty_state| fake_pty_state.stdin.clone())
                .unwrap_or_default()
        })
    }

    pub fn wait_for_stdin(&self, what: &str, predicate: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let terminal_id = self.terminal_id;
        self.shared_ptys.wait_for(what, move |fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&terminal_id)
                .and_then(|fake_pty_state| {
                    if predicate(&fake_pty_state.stdin) {
                        Some(fake_pty_state.stdin.clone())
                    } else {
                        None
                    }
                })
        })
    }

    pub fn size(&self) -> Option<(u16, u16)> {
        self.shared_ptys.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&self.terminal_id)
                .and_then(|fake_pty_state| fake_pty_state.size)
        })
    }

    pub fn size_history(&self) -> Vec<(u16, u16)> {
        self.shared_ptys.read(|fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&self.terminal_id)
                .map(|fake_pty_state| fake_pty_state.size_history.clone())
                .unwrap_or_default()
        })
    }

    pub fn wait_for_cell_size(&self, width: u16, height: u16) {
        self.shared_ptys
            .wait_for("host cell dimensions applied to PTY", |registry| {
                let terminal = registry.fake_pty_states.get(&self.terminal_id)?;
                let (cols, rows) = terminal.size?;
                (terminal.pixel_size == Some((cols * width, rows * height))).then_some(())
            });
    }

    pub fn wait_for_size(&self, what: &str, predicate: impl Fn(u16, u16) -> bool) -> (u16, u16) {
        let terminal_id = self.terminal_id;
        self.shared_ptys.wait_for(what, move |fake_pty_registry| {
            fake_pty_registry
                .fake_pty_states
                .get(&terminal_id)
                .and_then(|fake_pty_state| fake_pty_state.size)
                .filter(|(cols, rows)| predicate(*cols, *rows))
        })
    }

    pub fn wait_for_size_change(&self, last_seen: (u16, u16)) -> Option<(u16, u16)> {
        self.shared_ptys
            .wait_for_change(self.terminal_id, last_seen)
    }
}
