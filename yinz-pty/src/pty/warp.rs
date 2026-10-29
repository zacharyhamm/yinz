//! Pane and tab transfer, adoption and recovery.

use super::{send_pane_error_to_screen, Pty};
use crate::global_async_runtime::get_tokio_runtime as async_runtime;
use crate::os_input_output::ReleasedPane;
use crate::screen::ScreenInstruction;
use crate::terminal_bytes::TerminalBytes;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use yinz_bus::pty::{get_default_shell, terminal_exit_instruction, AdoptedPane, AdoptedTabPanes};
use yinz_bus::warp::ReleasedPaneForWarp;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{WarpedPane, WarpedTab};
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::{SessionName, TerminalId};
use yinz_types::pane_id::PaneId;

impl Pty {
    fn check_held_can_be_warped(&self, id: TerminalId) -> Result<()> {
        if self.reruns.contains_key(&id) || self.pending_remote_spawns.contains(&id) {
            return Err(anyhow!(
                "terminal_{id} is restarting; retry after it has started"
            ));
        }
        Ok(())
    }

    /// An exited child's reader may still have final output for Screen. Join it
    /// without cancelling; on timeout, keep the handle and let a later warp retry.
    pub(super) fn drain_held_reader_for_warp(
        &mut self,
        id: TerminalId,
        deadline: Instant,
    ) -> Result<()> {
        let err_context = || format!("failed to drain terminal_{id} for transfer");
        self.check_held_can_be_warped(id)
            .with_context(err_context)?;
        refuse_if_nobody_is_waiting(deadline).with_context(err_context)?;
        if let Some(handle) = self.task_handles.get_mut(&id) {
            let joined = async_runtime()
                .block_on(async { tokio::time::timeout_at(deadline.into(), handle).await })
                .with_context(|| format!("terminal_{id} output is still draining; retry"))?;
            self.task_handles.remove(&id);
            joined
                .with_context(|| format!("terminal_{id} output reader failed during transfer"))?;
        }
        Ok(())
    }

    /// Whether this pane could be released for a warp, without releasing it.
    ///
    /// Separate from the release so a whole tab can be asked before any of it is
    /// let go — the panes ahead of a refusal would otherwise already be gone by
    /// the time it arrived.
    pub(crate) fn check_terminal_can_be_warped(&self, terminal_id: TerminalId) -> Result<()> {
        self.bus
            .os_input
            .as_ref()
            .context("no OS I/O interface found")
            .and_then(|os_input| os_input.check_terminal_can_be_warped(terminal_id))
    }

    /// Let go of a pane so another session can take it, without killing anything.
    ///
    /// Everything `close_pane` does *except* the two things that end a process:
    /// no `kill` of the child, and `release_terminal_for_warp` in place of
    /// `clear_terminal_id`, which would terminate the supervisor. Aborting the
    /// reader task stops consuming output; releasing the transport then gives up
    /// this server's master — the supervisor keeps its own and drains into the ring.
    ///
    /// The reader is stopped *before* the release, not after, and for a remote
    /// pane that ordering is load-bearing rather than tidy. The broker replays
    /// from the byte count this side reports, and the pane's screen is
    /// serialized separately by Screen; a byte has to land in exactly one of the
    /// two. With the reader still running, the count sampled inside the release
    /// could grow between being read and the screen being dumped, and the
    /// overlap would be replayed on top of itself.
    ///
    /// Stopping the reader is also why [`Self::check_terminal_can_be_warped`] runs first
    /// and is a separate question rather than something the release discovers as
    /// it goes: a refusal after the abort would leave a pane whose output is no
    /// longer being read. Everything knowable is therefore knowable *before*.
    pub(crate) fn release_terminal_for_warp(
        &mut self,
        terminal_id: TerminalId,
        deadline: Instant,
    ) -> Result<ReleasedPane> {
        let err_context = || format!("failed to release pane {terminal_id} for a warp");
        // Asked before the reader is touched, which is the whole point of it
        // being a separate question: everything the release below can refuse for
        // is knowable now, and refusing now leaves the pane exactly as it was.
        // A refusal after aborting the reader would leave the pane without its
        // output task, so all known refusals must precede the abort.
        self.check_terminal_can_be_warped(terminal_id)
            .with_context(err_context)?;
        refuse_if_nobody_is_waiting(deadline).with_context(err_context)?;
        self.stop_readers_for_warp(&[terminal_id]);
        // Measured *after* the readers have stopped, because stopping them is
        // not free — up to `READER_SHUTDOWN_TIMEOUT` each — and a budget taken
        // before it would hand the backend time the caller has already spent.
        let budget = deadline.saturating_duration_since(Instant::now());
        let released = self
            .bus
            .os_input
            .as_ref()
            .context("no OS I/O interface found")
            .and_then(|os_input| os_input.release_terminal_for_warp(terminal_id, budget))
            .with_context(err_context)?;

        self.forget_released_terminal(terminal_id);
        Ok(released)
    }

    /// Cancel these panes' readers and wait until they are actually gone.
    ///
    /// Every abort is issued before any join, so a tab's readers stop in
    /// parallel rather than one 500ms window after another. See
    /// [`wait_for_reader_to_stop`] for why the join has to happen at all.
    pub(super) fn stop_readers_for_warp(&mut self, terminal_ids: &[TerminalId]) {
        let handles: Vec<(TerminalId, JoinHandle<()>)> = terminal_ids
            .iter()
            .filter_map(|terminal_id| {
                self.task_handles
                    .remove(terminal_id)
                    .map(|handle| (*terminal_id, handle))
            })
            .collect();
        for (_, handle) in &handles {
            handle.abort();
        }
        for (terminal_id, handle) in handles {
            wait_for_reader_to_stop(terminal_id, handle);
        }
    }

    /// Forget everything this session knew about a pane it has handed away.
    ///
    /// Removed, emphatically not killed: the process is the point.
    fn forget_released_terminal(&mut self, terminal_id: TerminalId) {
        let _ = self.id_to_child_pid.remove(&terminal_id);
        self.pane_activity_flags.remove(&terminal_id);
        self.forget_terminal_cwd(terminal_id);
        self.terminal_cmds.remove(&terminal_id);
        self.forget_terminal_foreground(terminal_id);
        self.forget_terminal_host(terminal_id);
        // The plugin that spawned this pane lives in *this* session, so it does
        // not follow the pane out.
        self.originating_plugins.remove(&terminal_id);
        self.terminal_launch_commands.remove(&terminal_id);
    }

    /// Take a supervisor over from another session (or from a server that died)
    /// and give this session a pane for it.
    ///
    /// Returns the fresh terminal id, the command it is running, whatever the
    /// supervisor buffered while nobody held it, and its exit status if the child
    /// is already gone.
    ///
    /// Takes the whole `WarpedPane` rather than a socket path because a remote
    /// pane has no socket path to take: its process is picked up by resuming it
    /// on the broker that has been holding it. Everything after the two backend
    /// calls is identical, which is the point — the pane that arrives is an
    /// ordinary pane either way.
    pub(crate) fn adopt_terminal(&mut self, pane: &WarpedPane) -> Result<AdoptedPane> {
        if let Some(held) = &pane.held {
            if pane.remote.is_some()
                || !pane.control_socket.as_os_str().is_empty()
                || pane.group.is_some()
            {
                return Err(anyhow!("held terminal also specifies a process handoff"));
            }
            let terminal_id = self.reserve_terminal_id()?;
            let mut cmd = held.command.clone();
            cmd.originating_plugin = None;
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::UpdatePaneHost(
                    terminal_id,
                    cmd.display_host(),
                ))
                .context("failed to report held terminal host")
                .non_fatal();
            return Ok(AdoptedPane {
                terminal_id,
                cmd,
                buffered: vec![],
                child_exit_status: (!held.first_run).then_some(held.exit_status),
            });
        }
        let err_context = || match &pane.remote {
            Some(remote) => format!(
                "failed to adopt the pane running on {} as {}",
                remote.host, remote.pane_token
            ),
            None => format!(
                "failed to adopt the pane supervised at {}",
                pane.control_socket.display()
            ),
        };

        // The supervisor supplies the command during attach, so the callback uses
        // the received command's hold policy.
        let senders = self.bus.senders.clone();
        let quit_cb = Box::new(move |pane_id, exit_status, command: RunCommand| {
            // Deliberately no `CommandPaneExited`/`EditPaneExited`: an
            // originating plugin belongs to the session this pane came from, and
            // its plugin id would name something else entirely here. The
            // supervisor clears it from the command for the same reason.
            let hold_on_close = command.hold_on_close;
            let _ = senders.send_to_screen(terminal_exit_instruction(
                pane_id,
                exit_status,
                command,
                hold_on_close,
            ));
        });

        let adopted = self
            .bus
            .os_input
            .as_ref()
            .context("no OS I/O interface found")
            .and_then(|os_input| match &pane.remote {
                Some(remote) => os_input.adopt_remote_terminal(remote, quit_cb),
                None => os_input.adopt_terminal(&pane.control_socket, quit_cb),
            })
            .with_context(err_context)?;

        let terminal_id = adopted.terminal_id;
        let child_pid = adopted.child_pid;
        self.track_launch(terminal_id, &adopted.cmd);
        let activity_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let terminal_bytes = async_runtime().spawn({
            let err_context = |terminal_id: TerminalId| {
                format!("failed to run async task for terminal {terminal_id}")
            };
            let senders = self.bus.senders.clone();
            let debug_to_file = self.debug_to_file;
            let activity_flag = activity_flag.clone();
            let reader = adopted.reader;
            async move {
                TerminalBytes::new(terminal_id, reader, senders, debug_to_file, activity_flag)
                    .listen()
                    .await
                    .with_context(|| err_context(terminal_id))
                    .fatal();
            }
        });

        self.task_handles.insert(terminal_id, terminal_bytes);
        self.pane_activity_flags.insert(terminal_id, activity_flag);
        self.report_terminal_host(terminal_id);
        // `None` for a pane adopted from another machine, and it must stay out
        // of both maps rather than be given a placeholder. A pid from there
        // names an unrelated local process, so `id_to_child_pid` would arm every
        // signal and `sysinfo` path against it, and `capture_initial_cwd` would
        // read that process's cwd and call it the pane's. The broker answers
        // both questions instead, through `remote_pane_status`.
        if let Some(child_pid) = child_pid {
            self.id_to_child_pid.insert(terminal_id, child_pid);
            // No requested cwd: an adopted pane asked for nothing, and its
            // process has been running long enough that the OS answers reliably.
            self.capture_initial_cwd(terminal_id, child_pid, None);
        }

        Ok(AdoptedPane {
            terminal_id,
            cmd: adopted.cmd,
            buffered: adopted.buffered,
            child_exit_status: adopted.child_exit_status,
        })
    }

    /// Take over every supervisor in a warped tab, or none of them.
    ///
    /// All-or-nothing because a tab's layout places panes by position: a pane
    /// that failed to attach does not leave a gap in the tab, it shifts every
    /// pane after it into somebody else's slot. Releasing what has already been
    /// attached puts the tab back in exactly the state the sending session's
    /// rollback expects to find it in — released, unowned, and reachable at the
    /// same sockets it sent.
    ///
    /// The queue this leaves behind is drained by `spawn_terminals_for_layout`
    /// once Screen's new-tab instructions come back round to this thread. It can
    /// be a plain field rather than something threaded through those
    /// instructions because there is only one pty thread and its channel is
    /// ordered: nothing can run between the two halves.
    pub(crate) fn adopt_tab(&mut self, payload: &WarpedTab) -> Result<AdoptedTabPanes> {
        payload
            .validate_relationships()
            .map_err(|e| anyhow!("{e}"))?;
        let mut tiled = vec![];
        let mut floating = vec![];
        let mut failed = None;

        for (panes, adopted) in [
            (&payload.tiled_panes, &mut tiled),
            (&payload.floating_panes, &mut floating),
        ] {
            for pane in panes {
                if failed.is_some() {
                    break;
                }
                match self.adopt_terminal(pane) {
                    Ok(pane) => adopted.push(pane),
                    Err(e) => failed = Some((pane.host_id.clone(), e)),
                }
            }
        }

        if let Some((host_id, e)) = failed {
            let host_ids: Vec<String> = payload
                .tiled_panes
                .iter()
                .chain(payload.floating_panes.iter())
                .map(|pane| pane.host_id.to_string())
                .collect();
            for (adopted, pane) in tiled
                .iter()
                .zip(&payload.tiled_panes)
                .chain(floating.iter().zip(&payload.floating_panes))
            {
                self.give_back_adopted(adopted.terminal_id, pane.held.is_some());
            }
            return Err(e)
                .with_context(|| {
                    format!(
                    "could not take the whole tab, so none of it was taken — its panes are still \
                     running and can be recovered with `yinz pty-host adopt <id>`: {}",
                    host_ids.join(", ")
                )
                })
                .with_context(|| {
                    format!("the pane supervised by {host_id} could not be attached")
                });
        }

        Ok(AdoptedTabPanes { tiled, floating })
    }

    /// Undo an adoption that cannot be completed. A held terminal has no
    /// supervisor and is closed; otherwise the supervisor is released so it can
    /// be adopted again.
    pub(crate) fn give_back_adopted(&mut self, terminal_id: TerminalId, held: bool) {
        if held {
            self.close_pane(PaneId::Terminal(terminal_id)).non_fatal();
            return;
        }
        if let Err(e) =
            self.release_terminal_for_warp(terminal_id, Instant::now() + GIVE_BACK_BUDGET)
        {
            // Nothing left to try. The caller logs which supervisor this was so it
            // can be recovered by hand.
            log::error!("could not give back adopted pane {terminal_id}: {e:#}");
        }
    }
}

/// Give up one pane's supervisor for a warp, or say why not.
///
/// The two failures this can report are not the same failure, and telling them
/// apart is the whole point of asking before releasing. A refusal from the check
/// costs nothing: the pane has not been touched, Screen leaves it exactly where
/// it was, and it goes on working. A failure from the release itself is a pane
/// already cut loose — the abort inside it has closed this server's dup of the
/// master — and that one has to be told what happened to it rather than left
/// looking live.
pub(super) fn release_pane_for_warp(
    pty: &mut Pty,
    terminal_id: TerminalId,
    to_session: &SessionName,
    deadline: Instant,
) -> std::result::Result<ReleasedPaneForWarp, String> {
    if let Err(e) = pty.check_terminal_can_be_warped(terminal_id) {
        return Err(format!("{e:#}"));
    }
    if let Err(e) = refuse_if_nobody_is_waiting(deadline) {
        return Err(format!("{e:#}"));
    }
    let run_command = warp_restart_command(pty, terminal_id);
    let released = pty
        .release_terminal_for_warp(terminal_id, deadline)
        .map_err(|e| {
            let why = format!("could not move this pane to '{to_session}': {e:#}");
            strand_pane_after_warp(pty, terminal_id, run_command.clone(), &why, None);
            format!("{e:#}")
        })?;
    Ok(ReleasedPaneForWarp {
        pane: released,
        run_command,
    })
}

/// Say no to a release whose caller has already stopped waiting.
///
/// Asked in the same breath as [`Pty::check_terminal_can_be_warped`] and for
/// the same reason: after this point the pane's reader is aborted and there is
/// no putting it back. A deadline that has passed used to reach the backend as
/// a budget of zero — which a remote pane's socket refuses to be bounded by —
/// with the reader already gone, so the pane was stranded to answer a caller
/// that was not there. Screen deliberately sends an already-passed deadline
/// when the caller left it nothing to spend; this is where that is honoured.
fn refuse_if_nobody_is_waiting(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(anyhow!(
            "the caller stopped waiting before this pane could be released"
        ));
    }
    Ok(())
}

/// What giving a just-adopted pane back may spend on each one.
///
/// This runs when a tab was adopted part-way and then failed, inside the
/// adopting side's own warp budget, once per pane already taken. It is
/// deliberately mean: the pane's process survives on its supervisor either way,
/// and the only thing a longer wait buys is a tidier handback — paid for by the
/// caller's remaining budget, which the rest of the rollback still needs.
const GIVE_BACK_BUDGET: Duration = Duration::from_secs(1);

/// How long to wait for a pane's reader to actually stop before releasing it.
///
/// Short, because it is measured against a task that has already been told to
/// cancel and only has to reach its next await point to do it. Bounded at all
/// because the pty thread is what does the waiting, and a reader wedged in a
/// blocking call it cannot be cancelled out of must not take the session with
/// it.
pub(super) const READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// Wait for an already-cancelled reader to actually be gone, before its
/// connection is handed to anyone else. Only the warp/release path needs this;
/// an ordinary close just aborts the reader and lets it stop on its own.
///
/// `JoinHandle::abort` asks; it does not deliver. The task keeps running until
/// it reaches an await point, and for a remote pane that window is long enough
/// to matter: the reader's socket is a `try_clone` of the very socket the
/// release is about to write `Release` down and read `Released` back from, so a
/// reader still on its feet can consume the ack and leave the release waiting
/// out its whole budget before reporting that the host never confirmed. The
/// release also flips that socket to blocking, and `O_NONBLOCK` lives on the
/// open file description the clone shares — so a reader that outlives the abort
/// finds its own reads blocking a runtime worker.
///
/// The real ssh tests get this right by construction: `release_for_warp` there
/// joins the pump thread before releasing. This is that join, in production.
fn wait_for_reader_to_stop(terminal_id: TerminalId, handle: JoinHandle<()>) {
    let joined = async_runtime()
        .block_on(async { tokio::time::timeout(READER_SHUTDOWN_TIMEOUT, handle).await });
    // A cancelled task reports `JoinError::is_cancelled`, which is the expected
    // outcome and not worth a word. Only the timeout is: it means the release
    // below is about to share a socket with a reader that is still reading.
    if joined.is_err() {
        log::warn!(
            "pane {terminal_id}'s reader did not stop within {READER_SHUTDOWN_TIMEOUT:?} of \
             being cancelled; continuing teardown anyway"
        );
    }
}

pub(super) fn release_tab_with_held_for_warp(
    pty: &mut Pty,
    mut terminal_ids: Vec<TerminalId>,
    held: HashMap<TerminalId, yinz_types::data::HeldTerminal>,
    deadline: Instant,
) -> Vec<(TerminalId, std::result::Result<ReleasedPaneForWarp, String>)> {
    // Revalidate before releasing any live terminal. Screen has frozen restart input
    // for these held panes; asynchronous spawns must also have finished.
    let refusal = terminal_ids
        .iter()
        .find_map(|id| {
            let check = if held.contains_key(id) {
                pty.check_held_can_be_warped(*id)
            } else {
                pty.check_terminal_can_be_warped(*id)
            };
            check
                .err()
                .map(|e| (Some(*id), format!("terminal_{id} cannot be moved: {e:#}")))
        })
        .or_else(|| {
            refuse_if_nobody_is_waiting(deadline)
                .err()
                .map(|e| (None, format!("{e:#}")))
        })
        .or_else(|| {
            terminal_ids
                .iter()
                .filter(|id| held.contains_key(id))
                .find_map(|id| {
                    pty.drain_held_reader_for_warp(*id, deadline)
                        .err()
                        .map(|e| (Some(*id), format!("{e:#}")))
                })
        });
    if let Some((culprit, reason)) = refusal {
        // Screen enriches the first refusal with that pane's title and state.
        terminal_ids.sort_by_key(|id| Some(*id) != culprit);
        return terminal_ids
            .into_iter()
            .map(|id| (id, Err(reason.clone())))
            .collect();
    }
    let live = terminal_ids
        .iter()
        .filter(|id| !held.contains_key(id))
        .copied()
        .collect();
    let mut released: HashMap<_, _> = release_tab_for_warp(pty, live, deadline)
        .into_iter()
        .collect();
    terminal_ids
        .into_iter()
        .map(|id| {
            let result = match held.get(&id) {
                Some(held) => Ok(ReleasedPaneForWarp {
                    run_command: held.command.clone(),
                    pane: ReleasedPane::Held(held.clone()),
                }),
                None => released
                    .remove(&id)
                    .unwrap_or_else(|| Err(format!("terminal_{id} was not released"))),
            };
            (id, result)
        })
        .collect()
}

/// Give up a whole tab's supervisors for a warp, or say why not.
///
/// Every pane is asked before any pane is let go. Releasing them one at a time
/// and stopping at the first refusal would answer "no" with the panes ahead of
/// it already handed away and nothing to give them back to — so a tab that
/// cannot go is refused while it is still whole, and every pane in it is
/// reported with the one reason.
///
/// The releases themselves then run *concurrently*, which is not an
/// optimisation. A local release is a single non-blocking write and costs
/// nothing either way; a remote one is a request and an acknowledgement over
/// ssh, and doing those in turn made a tab's cost the sum of its panes' round
/// trips. Two remote panes was already the whole of `warp::WARP_TIMEOUT`, so a
/// tab like that could not be moved at all — it reported "timed out preparing
/// the tab to be moved" while this thread was still waiting on the first pane.
/// Run together they cost one round trip, and each answers to the same
/// `deadline` rather than to a budget of its own.
pub(super) fn release_tab_for_warp(
    pty: &mut Pty,
    terminal_ids: Vec<TerminalId>,
    deadline: Instant,
) -> Vec<(TerminalId, std::result::Result<ReleasedPaneForWarp, String>)> {
    let refusal = terminal_ids.iter().find_map(|terminal_id| {
        pty.check_terminal_can_be_warped(*terminal_id)
            .err()
            .map(|e| format!("pane {terminal_id} cannot be moved: {e:#}"))
    });
    let refusal = refusal.or_else(|| {
        refuse_if_nobody_is_waiting(deadline)
            .err()
            .map(|e| format!("{e:#}"))
    });
    if let Some(reason) = refusal {
        return terminal_ids
            .into_iter()
            .map(|terminal_id| (terminal_id, Err(reason.clone())))
            .collect();
    }

    let commands: Vec<_> = terminal_ids
        .iter()
        .map(|id| (*id, warp_restart_command(pty, *id)))
        .collect();

    // Every reader stopped before any socket is written to. A remote pane's
    // reader shares the connection the release is about to speak on, and it
    // shares it with *this* pane's release only — but stopping them all up
    // front costs one wait instead of one per pane.
    pty.stop_readers_for_warp(&terminal_ids);

    let budget = deadline.saturating_duration_since(Instant::now());
    let released: Vec<_> = {
        let Some(os_input) = pty.bus.os_input.as_deref() else {
            return terminal_ids
                .iter()
                .map(|terminal_id| (*terminal_id, Err("no OS I/O interface found".to_owned())))
                .collect();
        };
        std::thread::scope(|scope| {
            let threads: Vec<_> = terminal_ids
                .iter()
                .map(|terminal_id| {
                    let terminal_id = *terminal_id;
                    scope.spawn(move || {
                        os_input
                            .release_terminal_for_warp(terminal_id, budget)
                            .with_context(|| {
                                format!("failed to release pane {terminal_id} for a warp")
                            })
                            .map_err(|e| format!("{e:#}"))
                    })
                })
                .collect();
            commands
                .into_iter()
                .zip(threads)
                .map(|((terminal_id, run_command), thread)| {
                    // A panicked release is a failed release. Reporting it as
                    // one puts the pane through the stranding below, which is
                    // the only way its process gets named to the user at all.
                    let result = thread
                        .join()
                        .unwrap_or_else(|_| Err(format!("releasing pane {terminal_id} panicked")));
                    (terminal_id, run_command, result)
                })
                .collect()
        })
    };

    for (terminal_id, _, result) in &released {
        if result.is_ok() {
            pty.forget_released_terminal(*terminal_id);
        }
    }
    // A release that fails *after* every pane passed the check takes the whole
    // tab down with it: the ones that succeeded are orphaned, the one that
    // failed is severed, and none of them has a process on the other end any
    // more. Each says so, and the orphans say how to get theirs back.
    if released.iter().any(|(_, _, result)| result.is_err()) {
        for (terminal_id, run_command, result) in &released {
            match result {
                Ok(released) => strand_pane_after_warp(
                    pty,
                    *terminal_id,
                    run_command.clone(),
                    "this tab could not be moved, and this pane had already been handed over",
                    Some(released.recovery_command()),
                ),
                Err(e) => strand_pane_after_warp(
                    pty,
                    *terminal_id,
                    run_command.clone(),
                    &format!("this tab could not be moved: {e}"),
                    None,
                ),
            }
        }
    }
    released
        .into_iter()
        .map(|(terminal_id, run_command, result)| {
            (
                terminal_id,
                result.map(|pane| ReleasedPaneForWarp { pane, run_command }),
            )
        })
        .collect()
}

/// Tell the user, in the pane itself, that a warp cut it loose.
///
/// A pane whose release failed part-way through — or one that was released for a
/// warp nobody ended up taking — cannot be put back: the abort that starts a
/// release drops this server's dup of the pty master, and there is no second
/// copy to reopen. What is left to decide is only whether the user finds out.
/// Left alone the pane sits there looking live, swallowing keystrokes and never
/// updating, which is the worst of the available outcomes.
///
/// So it is held, through the same funnel a pane that failed to start goes
/// through. That is an honest description of the state — there is no process on
/// the other end any more — and it comes with the way out already wired up:
/// `<ENTER>` re-runs, `<Ctrl-c>` closes. `recovery` names the still-running
/// process for the user who would rather have *that* one back than a fresh one.
pub(super) fn strand_pane_after_warp(
    pty: &Pty,
    terminal_id: TerminalId,
    run_command: RunCommand,
    why: &str,
    recovery: Option<String>,
) {
    let mut message = format!("\r\n\u{1b}[31m── {why} ──\u{1b}[0m\r\n");
    if let Some(recovery) = recovery {
        message.push_str(&format!(
            "\u{1b}[33mThe process is still running — recover it with `{recovery}`\u{1b}[0m\r\n"
        ));
    }
    send_pane_error_to_screen(pty.bus.senders.clone(), terminal_id, message, run_command)
        .non_fatal();
}

pub(super) fn warp_restart_command(pty: &Pty, terminal_id: TerminalId) -> RunCommand {
    // Observed argv can start with `-bash`; only the launch command retains login-shell intent.
    if let Some(command) = pty.terminal_launch_commands.get(&terminal_id) {
        return RunCommand {
            cwd: pty
                .terminal_cwds
                .get(&terminal_id)
                .cloned()
                .or_else(|| command.cwd.clone()),
            hold_on_start: false,
            hold_on_close: false,
            ..command.clone()
        };
    }
    let argv = pty
        .terminal_cmds
        .get(&terminal_id)
        .cloned()
        .unwrap_or_default();
    let mut argv = argv.into_iter();
    RunCommand {
        command: argv
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(get_default_shell),
        args: argv.collect(),
        cwd: pty.terminal_cwds.get(&terminal_id).cloned(),
        ..Default::default()
    }
}
