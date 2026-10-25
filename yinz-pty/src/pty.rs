use crate::global_async_runtime::get_tokio_runtime as async_runtime;
use crate::os_input_output::{AsyncReader, NullAsyncReader, TerminalProcesses};
use crate::plugins::{PluginId, PluginInstruction};
use crate::pty_host::client::ChildRunState;
use crate::pty_host::remote::protocol::ChildSignal;
use crate::route::NotificationEnd;
use crate::screen::ScreenInstruction;
use crate::terminal_bytes::TerminalBytes;
use crate::thread_bus::{Bus, ThreadSenders};
use crate::ClientId;
use layouts::SpawnedPane;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;
use yinz_runtime::errors::prelude::*;
use yinz_types::command::PaneTarget;
use yinz_types::data::{Event, NewPanePlacement, OriginatingPlugin};
use yinz_types::input::command::{OpenFilePayload, RunCommand, TerminalAction};
use yinz_types::input::layout::Run;
use yinz_types::internal_ids::{ProcessId, TabId, TerminalId};
use yinz_types::pane_id::PaneId;

mod layouts;
mod metadata;
mod thread_main;
mod warp;
pub use thread_main::pty_thread_main;

pub use yinz_bus::pty::*;

/// A single pane's terminal has been spawned, or has failed to: put it on
/// screen and tell the plugin that asked for it.
fn finish_pane_spawn(
    pty: &mut Pty,
    mut pending: PendingPaneSpawn,
    result: Result<(TerminalId, bool)>,
) -> Result<()> {
    let client_or_tab_index = pending.client_or_tab_index;
    let err_context = move || format!("failed to spawn terminal for {client_or_tab_index:?}");
    match result.with_context(err_context) {
        Ok((terminal_id, starts_held)) => {
            if let Some(completion) = pending.completion_tx.as_mut() {
                completion.set_affected_pane_id(PaneId::Terminal(terminal_id));
            }
            let hold_for_command = starts_held.then(|| pending.run_command.clone()).flatten();
            pty.announce_pane_opened(
                terminal_id,
                pending.run_command.as_ref(),
                pending.open_file_payload.as_ref(),
            )
            .with_context(err_context)?;
            pty.bus
                .senders
                .send_to_screen(pending.screen_instruction(terminal_id, hold_for_command))
                .with_context(err_context)
        },
        Err(err) => {
            if let Some(completion) = pending
                .completion_tx
                .as_mut()
                .filter(|c| c.wait_for_agent_start)
            {
                completion.set_error_message(format!("agent process failed to start: {err:#}"));
                if let Some((terminal_id, _)) = pane_start_failure(&err) {
                    completion.set_affected_pane_id(PaneId::Terminal(terminal_id));
                }
            }
            let run_command = pending.run_command.clone();
            report_pane_start_failure(pty, err, run_command, Some(pending), err_context)
        },
    }
}

fn finish_command_rerun(
    pty: &mut Pty,
    pane_id: PaneId,
    run_command: RunCommand,
    result: Result<()>,
) -> Result<()> {
    let err_context = move || format!("failed to rerun command in pane {pane_id:?}");
    match result.with_context(err_context) {
        Ok(()) => Ok(()),
        Err(err) => report_pane_start_failure(pty, err, Some(run_command), None, err_context),
    }
}

/// A pane's command could not start. When the command shows start failures,
/// the reason is written into the pane and the plugin that asked for it is
/// told the command exited; `placeholder` is the pane to put up for that
/// message when Screen has not seen one yet (a spawn, as opposed to a rerun).
/// Otherwise a pane Screen has not seen is released, and one it has is left
/// as it was.
fn report_pane_start_failure(
    pty: &mut Pty,
    err: anyhow::Error,
    run_command: Option<RunCommand>,
    placeholder: Option<PendingPaneSpawn>,
    err_context: impl Fn() -> String + Copy,
) -> Result<()> {
    let shows_start_failure = run_command
        .as_ref()
        .is_some_and(|command| command.shows_start_failure());
    match pane_start_failure(&err) {
        Some((terminal_id, message)) if shows_start_failure => {
            if let Some(placeholder) = placeholder {
                pty.bus
                    .senders
                    .send_to_screen(placeholder.screen_instruction(terminal_id, None))
                    .with_context(err_context)?;
            }
            let Some(run_command) = run_command else {
                return Ok(());
            };
            if let Some(originating_plugin) = run_command.originating_plugin.clone() {
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::for_plugin_event(
                        originating_plugin.plugin_id,
                        originating_plugin.client_id,
                        originating_plugin.generation,
                        Event::CommandPaneExited(terminal_id, Some(2), originating_plugin.context),
                    ))
                    .with_context(err_context)?;
            }
            send_pane_error_to_screen(pty.bus.senders.clone(), terminal_id, message, run_command)
                .with_context(err_context)?;
        },
        Some((terminal_id, _)) => {
            if placeholder.is_some() {
                Err::<(), _>(anyhow!("{err:?}"))
                    .context("failed to spawn terminal")
                    .non_fatal();
                pty.close_pane(PaneId::Terminal(terminal_id))
                    .with_context(err_context)?;
            }
        },
        None => Err::<(), _>(err).non_fatal(),
    }
    Ok(())
}

/// A pane whose terminal is being spawned: what Screen and the plugin that
/// asked for it need once its id is known.
#[derive(Clone, Debug)]
struct PendingPaneSpawn {
    run_command: Option<RunCommand>,
    open_file_payload: Option<OpenFilePayload>,
    pane_title: Option<String>,
    invoked_with: Option<Run>,
    client_or_tab_index: ClientTabIndexOrPaneId,
    completion_tx: Option<NotificationEnd>,
    placement: PanePlacement,
}

/// Where a finished spawn goes on screen.
#[derive(Clone, Debug)]
enum PanePlacement {
    /// A pane of its own: `ScreenInstruction::NewPane`.
    New {
        new_pane_placement: NewPanePlacement,
        start_suppressed: bool,
        set_blocking: bool,
    },
    /// The pane it was asked to take over: `ScreenInstruction::ReplacePane`.
    InPlace { replaced: ReplacedPane },
}

impl PendingPaneSpawn {
    /// The instruction that puts the pane on screen - the running pane, or the
    /// placeholder that shows why it could not start.
    fn screen_instruction(
        self,
        terminal_id: TerminalId,
        hold_for_command: Option<RunCommand>,
    ) -> ScreenInstruction {
        match self.placement {
            PanePlacement::New {
                new_pane_placement,
                start_suppressed,
                set_blocking,
            } => ScreenInstruction::NewPane(
                PaneId::Terminal(terminal_id),
                self.pane_title,
                hold_for_command,
                self.invoked_with,
                new_pane_placement,
                start_suppressed,
                self.client_or_tab_index,
                self.completion_tx,
                set_blocking,
            ),
            PanePlacement::InPlace { replaced } => ScreenInstruction::ReplacePane(
                PaneId::Terminal(terminal_id),
                hold_for_command,
                self.pane_title,
                self.invoked_with,
                replaced,
                self.client_or_tab_index,
                self.completion_tx,
            ),
        }
    }
}

/// What a spawn worker was asked to do.
enum SpawnJob {
    Terminal(PendingRemoteSpawn),
    Rerun(Box<PendingRerun>),
}

/// The re-runs a pane has in flight and waiting: the generation of the one
/// off-thread now, and the ones asked for since, run in order after it. Any
/// rerun of a pane with an entry here is queued, local or not, so two commands
/// can never stream into one pane.
#[derive(Debug, Default)]
struct RerunState {
    generation: u64,
    queued: VecDeque<(RunCommand, Option<NotificationEnd>)>,
}

/// A coding agent's hook event from a remote pane, arrived on the pane's
/// broker channel. Queue state, lifecycle observation, and plugin delivery
/// together, the way `agent-report` does for a local pane.
fn handle_remote_agent_report(
    senders: &ThreadSenders,
    terminal_id: TerminalId,
    kind: &str,
    json: String,
) -> Result<()> {
    use yinz_runtime::agent::report::ReportTrace;
    use yinz_types::agent::report::{AgentReport, AgentReportState};
    use {yinz_types::agent, yinz_types::agent::HookEvent, yinz_types::agent::HookOutcome};
    let accepted_at = Instant::now();
    if json.len() > agent::HOOK_BROADCAST_MAX_BYTES {
        return Err(anyhow!("agent hook event exceeds the broadcast limit"));
    }
    let event: HookEvent = serde_json::from_str(&json).context("invalid remote agent report")?;
    let state = match agent::hook_outcome(kind, &event) {
        HookOutcome::Ended => AgentReportState::Clear,
        HookOutcome::Reported(agent) => AgentReportState::Set(agent),
        HookOutcome::Nothing => AgentReportState::Unchanged,
    };
    let trace = ReportTrace::new(accepted_at);
    senders
        .send_to_screen(ScreenInstruction::AgentReport(
            AgentReport {
                report_id: trace.report_id,
                pane_id: PaneId::Terminal(terminal_id),
                state,
                event: Some(agent::AgentEvent::from_hook(
                    kind,
                    terminal_id,
                    &event,
                    json,
                )),
                trace: trace.enabled,
                process_id: trace.process_id,
                kind: kind.to_owned(),
            },
            accepted_at,
        ))
        .context("failed to queue remote agent report")
}

/// A spawn worker could not hand its result back to the pty thread, so the pane it
/// started has to be closed by the worker itself: nobody else knows the id.
const LOST_SPAWN_HANDBACK: &str =
    "a remote pane was started but could not be handed back to the pty thread; \
     the pane has been dropped";

fn terminal_safe(text: &str) -> String {
    text.chars().flat_map(char::escape_default).collect()
}

/// What a deferred spawn is waiting on before its command can start.
fn resolving_status(target: &PaneTarget) -> &'static str {
    match target {
        PaneTarget::Bubblewrap { .. } => "checking the Bubblewrap environment",
        PaneTarget::Ssh { .. } | PaneTarget::Local => "connecting to the remote host",
    }
}

/// What a deferred spawn does once its target is resolved.
fn starting_status(target: &PaneTarget) -> &'static str {
    match target {
        PaneTarget::Bubblewrap { .. } => "entering the Bubblewrap environment",
        PaneTarget::Ssh { .. } | PaneTarget::Local => "starting the remote command",
    }
}

fn remote_loading_screen(command: &RunCommand, status: &str) -> Vec<u8> {
    let status = terminal_safe(status);
    let body = match &command.target {
        PaneTarget::Ssh { host } => format!(
            "\u{1b}[H\u{1b}[2J\u{1b}[1mConnecting to remote host\u{1b}[0m\r\n\r\nHost: {}\r\n\r\nStatus: {}",
            terminal_safe(host),
            status,
        ),
        PaneTarget::Bubblewrap { root } => format!(
            "\u{1b}[H\u{1b}[2J\u{1b}[1mStarting Bubblewrap environment\u{1b}[0m\r\n\r\nCheckout: {}\r\n\r\nStatus: {}",
            terminal_safe(&root.display().to_string()),
            status,
        ),
        PaneTarget::Local => return vec![],
    };
    body.into_bytes()
}

fn send_remote_status(
    senders: &ThreadSenders,
    terminal_id: TerminalId,
    command: &RunCommand,
    status: &str,
) -> Result<()> {
    senders.send_to_screen(ScreenInstruction::PtyBytes(
        terminal_id,
        remote_loading_screen(command, status),
    ))
}

/// Resolve where one command runs and start it there, off the pty thread.
fn spawn_remote_terminal(
    os_input: &dyn TerminalProcesses,
    senders: &ThreadSenders,
    pending: &PendingRemoteSpawn,
    default_editor: Option<PathBuf>,
) -> Result<SpawnedTerminal> {
    let terminal_action = pending.terminal_action.clone();
    let command = match &terminal_action {
        TerminalAction::RunCommand(command) => command.clone(),
        TerminalAction::OpenFile(_) => return Err(anyhow!("a remote spawn must run a command")),
    };
    let quit_cb = terminal_quit_callback(senders.clone(), &terminal_action);
    let mut report_progress =
        |phase: &str| send_remote_status(senders, pending.terminal_id, &command, phase).non_fatal();
    os_input
        .resolve_spawn(
            pending.terminal_id,
            terminal_action,
            default_editor,
            &mut report_progress,
        )
        .and_then(|resolved| {
            send_remote_status(
                senders,
                pending.terminal_id,
                &command,
                starting_status(&command.target),
            )
            .non_fatal();
            os_input.spawn_resolved(resolved, quit_cb)
        })
        .map(|(terminal_id, reader, child_pid)| SpawnedTerminal {
            terminal_id,
            reader,
            child_pid,
        })
}

/// Do a spawn job's off-thread work. Returns what to hand back and the ids of
/// every terminal the job reserved, so a handback that fails can close them.
fn run_spawn_job(
    os_input: &dyn TerminalProcesses,
    senders: &ThreadSenders,
    job: SpawnJob,
    default_editor: Option<PathBuf>,
) -> (PendingSpawn, Vec<TerminalId>) {
    match job {
        SpawnJob::Terminal(pending) => {
            let spawned = spawn_remote_terminal(os_input, senders, &pending, default_editor);
            let started = vec![pending.terminal_id];
            (
                PendingSpawn::Terminal(pending, OneShot::new(spawned)),
                started,
            )
        },
        SpawnJob::Rerun(pending) => {
            let spawned = match pending.pane_id {
                PaneId::Terminal(terminal_id) => {
                    let quit_cb = terminal_quit_callback(
                        senders.clone(),
                        &TerminalAction::RunCommand(pending.run_command.clone()),
                    );
                    let mut report_progress = |phase: &str| {
                        send_remote_status(senders, terminal_id, &pending.run_command, phase)
                            .non_fatal()
                    };
                    os_input
                        .resolve_rerun(
                            terminal_id,
                            pending.run_command.clone(),
                            &mut report_progress,
                        )
                        .and_then(|resolved| {
                            send_remote_status(
                                senders,
                                terminal_id,
                                &pending.run_command,
                                starting_status(&pending.run_command.target),
                            )
                            .non_fatal();
                            os_input.rerun_resolved(resolved, quit_cb)
                        })
                        .map(|(reader, child_pid)| SpawnedRerun { reader, child_pid })
                },
                PaneId::Plugin(_) => Err(anyhow!("cannot respawn plugin panes")),
            };
            let started = match (&spawned, pending.pane_id) {
                (Ok(_), PaneId::Terminal(terminal_id)) => vec![terminal_id],
                _ => vec![],
            };
            (PendingSpawn::Rerun(pending, OneShot::new(spawned)), started)
        },
    }
}

fn terminal_quit_callback(
    senders: ThreadSenders,
    terminal_action: &TerminalAction,
) -> Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send> {
    let (hold_on_close, command_plugin, edit_plugin) = match terminal_action {
        TerminalAction::RunCommand(command) => (
            command.hold_on_close,
            command.originating_plugin.clone(),
            None,
        ),
        TerminalAction::OpenFile(payload) => (false, None, payload.originating_plugin.clone()),
    };
    Box::new(move |pane_id, exit_status, command| {
        if let PaneId::Terminal(terminal_id) = pane_id
            && let Some(plugin) = edit_plugin.as_ref()
        {
            let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
                plugin.plugin_id,
                plugin.client_id,
                plugin.generation,
                Event::EditPaneExited(terminal_id, exit_status, plugin.context.clone()),
            ));
        }
        let _ = senders.send_to_screen(terminal_exit_instruction(
            pane_id,
            exit_status,
            command,
            hold_on_close,
        ));
        if let PaneId::Terminal(terminal_id) = pane_id
            && let Some(plugin) = command_plugin.as_ref()
        {
            let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
                plugin.plugin_id,
                plugin.client_id,
                plugin.generation,
                Event::CommandPaneExited(terminal_id, exit_status, plugin.context.clone()),
            ));
        }
    })
}

pub struct Pty {
    pub(crate) active_panes: HashMap<ClientId, PaneId>,
    pub(crate) bus: Bus<PtyInstruction>,
    pub(crate) id_to_child_pid: HashMap<TerminalId, ProcessId>, // terminal_id => child pid
    originating_plugins: HashMap<TerminalId, OriginatingPlugin>,
    /// Retained for restart commands and for reading argv inside Bubblewrap sandboxes.
    terminal_launch_commands: HashMap<TerminalId, RunCommand>,
    debug_to_file: bool,
    task_handles: HashMap<TerminalId, JoinHandle<()>>, // terminal_id to join-handle
    default_editor: Option<PathBuf>,
    post_command_discovery_hook: Option<String>,
    plugin_cwds: HashMap<PluginId, PathBuf>, // plugin_id -> cwd
    terminal_cwds: HashMap<TerminalId, PathBuf>, // terminal_id -> cwd
    pane_activity_flags: HashMap<TerminalId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    terminal_cmds: HashMap<TerminalId, Vec<String>>,
    terminal_reported_commands: HashMap<TerminalId, (Vec<String>, Vec<String>)>,
    /// Terminals adopted for a tab that is about to be built, waiting for the
    /// `NewTab` that will place them, keyed by the tab they belong to.
    ///
    /// Keyed rather than kept as a single slot because the `NewTab` this is
    /// waiting for goes out through the plugin thread and comes back, and any
    /// tab the user opens in the meantime arrives on this channel too. An
    /// unkeyed queue would hand a warped tab's panes to whichever `NewTab`
    /// happened to be first, which is a race a user could lose a tab to.
    ///
    /// Screen claims an entry before sending that `NewTab`, so it is always here
    /// by the time the tab comes round: the claim is enqueued first, and the
    /// plugin thread cannot forward a `NewTab` it has not yet been sent.
    adopted_tabs: HashMap<TabId, AdoptedTabTerminals>,
    /// Threads resolving and starting remote panes, so `Drop` need not wait
    /// for a boot; reaped as new ones start.
    spawn_workers: Vec<std::thread::JoinHandle<()>>,
    /// Remote panes already placed on screen whose worker has not handed a
    /// transport back yet. Closing the pane removes its id, so a late result
    /// can only be torn down, never bring the pane back.
    pending_remote_spawns: HashSet<TerminalId>,
    /// Agent control must report process failure, not success from a placeholder's ID.
    pending_agent_spawns: HashMap<TerminalId, PendingPaneSpawn>,
    /// Per pane, the rerun off-thread now and the ones waiting behind it.
    reruns: HashMap<TerminalId, RerunState>,
    next_rerun_generation: u64,
}

impl Pty {
    pub fn new(
        bus: Bus<PtyInstruction>,
        debug_to_file: bool,
        default_editor: Option<PathBuf>,
        post_command_discovery_hook: Option<String>,
    ) -> Self {
        Pty {
            active_panes: HashMap::new(),
            bus,
            id_to_child_pid: HashMap::new(),
            debug_to_file,
            task_handles: HashMap::new(),
            default_editor,
            originating_plugins: HashMap::new(),
            terminal_launch_commands: HashMap::new(),
            post_command_discovery_hook,
            plugin_cwds: HashMap::new(),
            terminal_cwds: HashMap::new(),
            pane_activity_flags: HashMap::new(),
            terminal_cmds: HashMap::new(),
            terminal_reported_commands: HashMap::new(),
            adopted_tabs: HashMap::new(),
            spawn_workers: vec![],
            pending_remote_spawns: HashSet::new(),
            pending_agent_spawns: HashMap::new(),
            reruns: HashMap::new(),
            next_rerun_generation: 0,
        }
    }
    pub(crate) fn get_default_terminal(
        &self,
        cwd: Option<PathBuf>,
        default_shell: Option<TerminalAction>,
    ) -> TerminalAction {
        match default_shell {
            Some(mut default_shell) => {
                if let Some(cwd) = cwd {
                    match default_shell {
                        TerminalAction::RunCommand(ref mut command) => {
                            command.cwd = Some(cwd);
                        },
                        TerminalAction::OpenFile(ref mut payload) => {
                            match payload.cwd.as_mut() {
                                Some(edit_cwd) => {
                                    *edit_cwd = cwd.join(&edit_cwd);
                                },
                                None => {
                                    let _ = payload.cwd.insert(cwd.clone());
                                },
                            };
                        },
                    }
                }
                default_shell
            },
            None => {
                let shell = get_default_shell();
                TerminalAction::RunCommand(RunCommand {
                    args: vec![],
                    command: shell,
                    cwd, // note: this might also be filled by the calling function, eg. spawn_terminal
                    hold_on_close: false,
                    hold_on_start: false,
                    ..Default::default()
                })
            },
        }
    }
    pub(crate) fn spawn_terminal(
        &mut self,
        terminal_action: Option<TerminalAction>,
        client_or_tab_index: ClientTabIndexOrPaneId,
    ) -> Result<(TerminalId, bool)> {
        let terminal_action = self.prepare_terminal_action(terminal_action, client_or_tab_index);
        self.spawn_prepared_terminal(terminal_action, client_or_tab_index)
    }

    fn prepare_terminal_action(
        &mut self,
        terminal_action: Option<TerminalAction>,
        client_or_tab_index: ClientTabIndexOrPaneId,
    ) -> TerminalAction {
        match client_or_tab_index {
            ClientTabIndexOrPaneId::ClientId(client_id)
            | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                let mut terminal_action =
                    terminal_action.unwrap_or_else(|| self.get_default_terminal(None, None));
                self.fill_cwd(&mut terminal_action, client_id);
                terminal_action
            },
            ClientTabIndexOrPaneId::TabIndex(_)
            | ClientTabIndexOrPaneId::TabIndexNoFocus(_)
            | ClientTabIndexOrPaneId::TabIndexForClient(_, _) => {
                terminal_action.unwrap_or_else(|| self.get_default_terminal(None, None))
            },
            ClientTabIndexOrPaneId::PaneId(pane_id)
            | ClientTabIndexOrPaneId::PaneIdOrClient(pane_id, _) => {
                let mut terminal_action =
                    terminal_action.unwrap_or_else(|| self.get_default_terminal(None, None));
                self.fill_cwd_from_pane_id(&mut terminal_action, &pane_id);
                terminal_action
            },
        }
    }

    /// Hand a spawn to a worker thread. Reaching an SSH host and starting it there can take minutes, and
    /// the pty thread has every other pane's input to serve meanwhile; the
    /// worker sends `SpawnResolved` back when it is done.
    fn defer_spawn(&mut self, job: SpawnJob) -> Result<()> {
        let os_input = self
            .bus
            .os_input
            .as_ref()
            .context("no OS I/O interface found")?
            .box_clone();
        let default_editor = self.default_editor.clone();
        let senders = self.bus.senders.clone();
        match &job {
            SpawnJob::Terminal(pending) => {
                self.pending_remote_spawns.insert(pending.terminal_id);
                if let TerminalAction::RunCommand(command) = &pending.terminal_action {
                    send_remote_status(
                        &senders,
                        pending.terminal_id,
                        command,
                        resolving_status(&command.target),
                    )?;
                }
            },
            SpawnJob::Rerun(pending) => {
                if let PaneId::Terminal(terminal_id) = pending.pane_id {
                    send_remote_status(
                        &senders,
                        terminal_id,
                        &pending.run_command,
                        resolving_status(&pending.run_command.target),
                    )?;
                }
            },
        }
        self.reap_spawn_workers();
        self.spawn_workers.push(std::thread::spawn(move || {
            let (resolved, started) = run_spawn_job(&*os_input, &senders, job, default_editor);
            if senders
                .send_to_pty(PtyInstruction::SpawnResolved(resolved))
                .is_err()
            {
                Err::<(), _>(anyhow!(LOST_SPAWN_HANDBACK)).non_fatal();
                for terminal_id in started {
                    os_input.clear_terminal_id(terminal_id).non_fatal();
                }
            }
        }));
        Ok(())
    }

    /// Route a rerun. A pane with a rerun in flight queues this one behind it,
    /// whatever its target, so two commands never stream into one pane; a
    /// remote command goes to a worker; a local one is handed back to the
    /// caller to run on this thread. `Some` is that handback.
    fn request_rerun(
        &mut self,
        pane_id: PaneId,
        mut run_command: RunCommand,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<Option<(RunCommand, Option<NotificationEnd>)>> {
        run_command.hold_on_start = false;
        let PaneId::Terminal(terminal_id) = pane_id else {
            return Ok(Some((run_command, completion_tx)));
        };
        if let Some(state) = self.reruns.get_mut(&terminal_id) {
            state.queued.push_back((run_command, completion_tx));
            return Ok(None);
        }
        // The bookkeeping `rerun_command_in_pane` does before spawning, done
        // here for the same reason: what the pane knew is captured before
        // anything about it changes.
        match run_command.originating_plugin.as_ref() {
            Some(plugin) => {
                self.originating_plugins.insert(terminal_id, plugin.clone());
            },
            None => {
                run_command.originating_plugin =
                    self.originating_plugins.get(&terminal_id).cloned();
            },
        }
        // Return local commands with their origin too, so a failed rerun keeps its reply target.
        if !run_command.defers_spawn() {
            return Ok(Some((run_command, completion_tx)));
        }
        self.track_launch(terminal_id, &run_command);
        let _ = self.task_handles.remove(&terminal_id);
        let _ = self.id_to_child_pid.remove(&terminal_id);
        let generation = self.next_rerun_generation;
        self.next_rerun_generation = self.next_rerun_generation.wrapping_add(1);
        let pending = PendingRerun {
            pane_id,
            previous_host: self.host_of_pane(&pane_id),
            requested_cwd: run_command.cwd.clone(),
            run_command,
            _completion_tx: completion_tx,
            generation,
        };
        self.reruns.insert(
            terminal_id,
            RerunState {
                generation,
                queued: VecDeque::new(),
            },
        );
        self.defer_spawn(SpawnJob::Rerun(Box::new(pending)))?;
        Ok(None)
    }

    fn handle_remote_spawned(
        &mut self,
        pending: PendingRemoteSpawn,
        spawned: OneShot<Result<SpawnedTerminal>>,
    ) -> Result<()> {
        let terminal_id = pending.terminal_id;
        let spawned = spawned.take()?;
        if !self.pending_remote_spawns.remove(&terminal_id) {
            if spawned.is_ok() {
                self.bus
                    .os_input
                    .as_ref()
                    .context("no OS I/O interface found")?
                    .clear_terminal_id(terminal_id)
                    .with_context(|| {
                        format!("failed to tear down the late spawn of closed pane {terminal_id}")
                    })
                    .non_fatal();
            }
            return Ok(());
        }
        if let Some(agent) = self.pending_agent_spawns.remove(&terminal_id) {
            let result = match spawned {
                Ok(spawned) => {
                    // Clear buffered loading messages before the reader can send command output.
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::PtyBytes(
                            terminal_id,
                            b"\x1b[3J\x1b[2J\x1b[H".to_vec(),
                        ))
                        .context("clear agent startup progress")?;
                    self.start_spawned_terminal(&pending.terminal_action, spawned);
                    Ok((terminal_id, false))
                },
                Err(error) => {
                    if pane_start_failure(&error).is_none() {
                        self.close_pane(PaneId::Terminal(terminal_id))
                            .context("release failed agent startup reservation")
                            .non_fatal();
                    }
                    Err(error)
                },
            };
            return finish_pane_spawn(self, agent, result);
        }
        let TerminalAction::RunCommand(command) = pending.terminal_action.clone() else {
            return Err(anyhow!("a remote spawn must run a command"));
        };
        match spawned {
            Ok(spawned) => {
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::RemotePaneReady(terminal_id))?;
                self.start_spawned_terminal(&pending.terminal_action, spawned);
            },
            Err(error) => {
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::PtyBytes(
                        terminal_id,
                        b"\x1b[3J\x1b[2J\x1b[H".to_vec(),
                    ))?;
                if let Some(originating_plugin) = command.originating_plugin.as_ref() {
                    self.bus
                        .senders
                        .send_to_plugin(PluginInstruction::for_plugin_event(
                            originating_plugin.plugin_id,
                            originating_plugin.client_id,
                            originating_plugin.generation,
                            Event::CommandPaneExited(
                                terminal_id,
                                Some(2),
                                originating_plugin.context.clone(),
                            ),
                        ))?;
                }
                let message = pane_start_failure(&error)
                    .map(|(_, message)| message)
                    .unwrap_or_else(|| format!("{error:#}"));
                send_pane_error_to_screen(self.bus.senders.clone(), terminal_id, message, command)?;
            },
        }
        Ok(())
    }

    /// The second half of a deferred rerun, once the worker has the command
    /// running.
    ///
    /// A missing or different generation means the pane was closed while the
    /// spawn was off-thread: `close_pane` dropped its entry, but the worker had
    /// by then registered the transport and started the command, so the id
    /// must still go through the teardown funnel or both outlive the pane.
    /// Terminal ids come from a counter and are never reused, so a stale spawn
    /// cannot name anyone else's pane.
    ///
    /// A stale completion never fails this handler: the pty thread takes its
    /// errors fatally, and there is nothing left to report a failure to, so a
    /// spawn that failed on its way back to a closed pane is logged and
    /// dropped.
    pub(crate) fn handle_rerun_spawned(
        &mut self,
        pending: PendingRerun,
        spawned: OneShot<Result<SpawnedRerun>>,
    ) -> Result<()> {
        let PaneId::Terminal(terminal_id) = pending.pane_id else {
            return Ok(());
        };
        let current = self.reruns.get(&terminal_id).map(|state| state.generation);
        if current != Some(pending.generation) {
            match spawned.take().and_then(|spawned| spawned) {
                Ok(_) => self
                    .bus
                    .os_input
                    .as_ref()
                    .context("no OS I/O interface found")
                    .and_then(|os_input| os_input.clear_terminal_id(terminal_id))
                    .with_context(|| {
                        format!("failed to tear down the late rerun of closed pane {terminal_id}")
                    })
                    .non_fatal(),
                Err(err) => Err::<(), _>(err)
                    .with_context(|| {
                        format!(
                            "the late rerun of closed pane {terminal_id} could not be completed"
                        )
                    })
                    .non_fatal(),
            }
            return Ok(());
        }
        let queued = self
            .reruns
            .remove(&terminal_id)
            .map(|state| state.queued)
            .unwrap_or_default();
        let spawned = spawned.take()?;
        if spawned.is_ok() {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::RemotePaneReady(terminal_id))?;
        } else {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::PtyBytes(
                    terminal_id,
                    b"\x1b[3J\x1b[2J\x1b[H".to_vec(),
                ))?;
        }
        let result = self.finish_rerun(
            pending.pane_id,
            pending.previous_host,
            pending.requested_cwd,
            spawned,
        );
        finish_command_rerun(self, pending.pane_id, pending.run_command, result)?;
        self.run_queued_reruns(pending.pane_id, queued)
    }

    /// Run the reruns that queued up behind one that just finished, in order,
    /// until the queue is empty or one of them goes off-thread (which takes the
    /// rest of the queue with it).
    fn run_queued_reruns(
        &mut self,
        pane_id: PaneId,
        mut queued: VecDeque<(RunCommand, Option<NotificationEnd>)>,
    ) -> Result<()> {
        let PaneId::Terminal(terminal_id) = pane_id else {
            return Ok(());
        };
        while let Some((run_command, completion_tx)) = queued.pop_front() {
            match self.request_rerun(pane_id, run_command, completion_tx)? {
                Some((run_command, _completion_tx)) => {
                    let result = self.rerun_command_in_pane(pane_id, run_command.clone());
                    finish_command_rerun(self, pane_id, run_command, result)?;
                },
                None => {
                    if let Some(state) = self.reruns.get_mut(&terminal_id) {
                        state.queued.append(&mut queued);
                    }
                    return Ok(());
                },
            }
        }
        Ok(())
    }

    fn reap_spawn_workers(&mut self) {
        let mut pending = vec![];
        for worker in self.spawn_workers.drain(..) {
            if worker.is_finished() {
                if worker.join().is_err() {
                    Err::<(), _>(anyhow!("a spawn worker panicked")).non_fatal();
                }
            } else {
                pending.push(worker);
            }
        }
        self.spawn_workers = pending;
    }

    fn reserve_terminal_id(&mut self) -> Result<TerminalId> {
        self.bus
            .os_input
            .as_mut()
            .context("no OS I/O interface found")?
            .reserve_terminal_id()
    }

    fn spawn_prepared_terminal(
        &mut self,
        terminal_action: TerminalAction,
        client_or_tab_index: ClientTabIndexOrPaneId,
    ) -> Result<(TerminalId, bool)> {
        // bool is starts_held
        let err_context = || format!("failed to spawn terminal for {:?}", client_or_tab_index);
        let hold_on_start = matches!(
            &terminal_action,
            TerminalAction::RunCommand(command) if command.hold_on_start
        );

        if hold_on_start {
            // we don't actually open a terminal in this case, just wait for the user to run it
            let starts_held = hold_on_start;
            let terminal_id = self
                .bus
                .os_input
                .as_mut()
                .context("couldn't get mutable reference to OS interface")
                .and_then(|os_input| os_input.reserve_terminal_id())
                .with_context(err_context)?;
            return Ok((terminal_id, starts_held));
        }

        let quit_cb = terminal_quit_callback(self.bus.senders.clone(), &terminal_action);
        let spawned = self
            .bus
            .os_input
            .as_mut()
            .context("no OS I/O interface found")
            .and_then(|os_input| {
                os_input.spawn_terminal(
                    terminal_action.clone(),
                    quit_cb,
                    self.default_editor.clone(),
                )
            })
            .map(|(terminal_id, reader, child_pid)| SpawnedTerminal {
                terminal_id,
                reader,
                child_pid,
            })
            .with_context(err_context)?;
        let terminal_id = spawned.terminal_id;
        self.start_spawned_terminal(&terminal_action, spawned);

        let starts_held = false;
        Ok((terminal_id, starts_held))
    }

    fn start_spawned_terminal(
        &mut self,
        terminal_action: &TerminalAction,
        spawned: SpawnedTerminal,
    ) {
        // `fill_cwd` has already resolved this, so it is the answer when the
        // child has not forked far enough to be asked.
        let requested_cwd = Self::action_cwd(terminal_action);
        let SpawnedTerminal {
            terminal_id,
            reader,
            child_pid,
        } = spawned;
        if let TerminalAction::RunCommand(command) = terminal_action {
            self.track_launch(terminal_id, command);
        }
        let activity_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let terminal_bytes = async_runtime().spawn({
            let err_context = |terminal_id: TerminalId| {
                format!("failed to run async task for terminal {terminal_id}")
            };
            let senders = self.bus.senders.clone();
            let debug_to_file = self.debug_to_file;
            let activity_flag = activity_flag.clone();
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
        if let Some(child_pid) = child_pid {
            self.id_to_child_pid.insert(terminal_id, child_pid);
            self.capture_initial_cwd(terminal_id, child_pid, requested_cwd);
        }
    }

    /// Register a command pane as its plugin's, so a later rerun keeps the
    /// plugin's context, and tell the plugin it is open.
    fn inform_originating_plugin_of_open(
        &mut self,
        terminal_id: TerminalId,
        originating_plugin: OriginatingPlugin,
    ) -> Result<()> {
        self.originating_plugins
            .insert(terminal_id, originating_plugin.clone());
        let update_event = Event::CommandPaneOpened(terminal_id, originating_plugin.context);
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                originating_plugin.plugin_id,
                originating_plugin.client_id,
                originating_plugin.generation,
                update_event,
            ))
    }

    /// Tell the plugin that asked for this pane that it is open: a command pane
    /// gets `CommandPaneOpened` (and is registered as the plugin's), an edit
    /// pane gets `EditPaneOpened`. A pane nobody asked for is told to no one.
    fn announce_pane_opened(
        &mut self,
        terminal_id: TerminalId,
        run_command: Option<&RunCommand>,
        open_file_payload: Option<&OpenFilePayload>,
    ) -> Result<()> {
        if let Some(originating_plugin) =
            run_command.and_then(|command| command.originating_plugin.clone())
        {
            self.inform_originating_plugin_of_open(terminal_id, originating_plugin)?;
        }
        if let Some(originating_plugin) =
            open_file_payload.and_then(|payload| payload.originating_plugin.clone())
        {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::for_plugin_event(
                    originating_plugin.plugin_id,
                    originating_plugin.client_id,
                    originating_plugin.generation,
                    Event::EditPaneOpened(terminal_id, originating_plugin.context),
                ))?;
        }
        Ok(())
    }

    /// Spawn one pane's terminal - now, or through a worker when the command
    /// runs elsewhere - and finish the spawn once its id is known.
    fn spawn_pane(
        &mut self,
        terminal_action: Option<TerminalAction>,
        mut pending: PendingPaneSpawn,
    ) -> Result<()> {
        let client_or_tab_index = pending.client_or_tab_index;
        if pending
            .run_command
            .as_ref()
            .is_some_and(RunCommand::defers_spawn)
        {
            let terminal_action =
                self.prepare_terminal_action(terminal_action, client_or_tab_index);
            let terminal_id = self.reserve_terminal_id()?;
            let wait_for_start = pending
                .completion_tx
                .as_ref()
                .is_some_and(|c| c.wait_for_agent_start);
            if wait_for_start {
                if let Some(completion) = pending.completion_tx.as_mut() {
                    completion.set_affected_pane_id(PaneId::Terminal(terminal_id));
                }
                self.pending_agent_spawns.insert(terminal_id, pending);
            } else {
                finish_pane_spawn(self, pending, Ok((terminal_id, false)))?;
            }
            let deferred = self.defer_spawn(SpawnJob::Terminal(PendingRemoteSpawn {
                terminal_id,
                terminal_action,
            }));
            if let Err(error) = deferred {
                if let Some(mut agent) = self.pending_agent_spawns.remove(&terminal_id) {
                    if let Some(completion) = agent.completion_tx.as_mut() {
                        completion
                            .set_error_message(format!("cannot dispatch agent startup: {error:#}"));
                    }
                    self.close_pane(PaneId::Terminal(terminal_id))
                        .context("release undispatched agent reservation")
                        .non_fatal();
                }
                return Err(error);
            }
            return Ok(());
        }
        let result = self.spawn_terminal(terminal_action, client_or_tab_index);
        finish_pane_spawn(self, pending, result)
    }

    fn apply_run_instruction(
        &mut self,
        run_instruction: Option<Run>,
        default_shell: TerminalAction,
        deferred_spawns: &mut Vec<PendingRemoteSpawn>,
    ) -> Result<Option<SpawnedPane>> {
        let err_context = || "failed to apply run instruction".to_string();
        let quit_cb = Box::new({
            let senders = self.bus.senders.clone();
            move |pane_id, exit_status, _command| {
                let _ = senders.send_to_screen(ScreenInstruction::ClosePane(
                    pane_id,
                    None,
                    None,
                    exit_status,
                ));
            }
        });

        match run_instruction {
            Some(Run::Command(mut command)) => {
                let starts_held = command.hold_on_start;
                // The default shell's cwd is a path on *this* machine; a command
                // that runs elsewhere must not inherit it.
                if command.cwd.is_none()
                    && !command.target.is_remote()
                    && let TerminalAction::RunCommand(cmd) = default_shell
                {
                    command.cwd = cmd.cwd;
                }
                let cmd = TerminalAction::RunCommand(command.clone());
                if starts_held {
                    // we don't actually open a terminal in this case, just wait for the user to run it
                    match self
                        .bus
                        .os_input
                        .as_mut()
                        .context("no OS I/O interface found")
                        .with_context(err_context)?
                        .reserve_terminal_id()
                    {
                        Ok(terminal_id) => {
                            Ok(Some((
                                terminal_id,
                                starts_held,
                                Some(command.clone()),
                                Some(Ok(Box::new(NullAsyncReader) as Box<dyn AsyncReader>)), // placeholder, never used for held panes
                            )))
                        },
                        Err(e) => Err(e),
                    }
                } else if command.defers_spawn() {
                    let terminal_id = self.reserve_terminal_id()?;
                    deferred_spawns.push(PendingRemoteSpawn {
                        terminal_id,
                        terminal_action: cmd,
                    });
                    Ok(Some((terminal_id, false, Some(command), None)))
                } else {
                    let quit_cb = terminal_quit_callback(self.bus.senders.clone(), &cmd);
                    let spawn_result = self
                        .bus
                        .os_input
                        .as_mut()
                        .context("no OS I/O interface found")
                        .and_then(|os_input| {
                            os_input.spawn_terminal(
                                cmd.clone(),
                                quit_cb,
                                self.default_editor.clone(),
                            )
                        })
                        .with_context(err_context);
                    match spawn_result {
                        Ok((terminal_id, reader, child_pid)) => {
                            if let Some(child_pid) = child_pid {
                                self.id_to_child_pid.insert(terminal_id, child_pid);
                                self.capture_initial_cwd(
                                    terminal_id,
                                    child_pid,
                                    command.cwd.clone(),
                                );
                            }
                            Ok(Some((
                                terminal_id,
                                starts_held,
                                Some(command.clone()),
                                Some(Ok(reader)),
                            )))
                        },
                        Err(err) => match pane_start_failure(&err) {
                            Some((terminal_id, _)) => Ok(Some((
                                terminal_id,
                                starts_held,
                                Some(command.clone()),
                                Some(Err(err)),
                            ))),
                            None => Err(err),
                        },
                    }
                }
            },
            Some(Run::Cwd(cwd)) => {
                let starts_held = false; // we do not hold Cwd panes
                let shell = self.get_default_terminal(Some(cwd), Some(default_shell.clone()));
                let requested_cwd = Self::action_cwd(&shell);
                match self
                    .bus
                    .os_input
                    .as_mut()
                    .context("no OS I/O interface found")
                    .with_context(err_context)?
                    .spawn_terminal(shell, quit_cb, self.default_editor.clone())
                    .with_context(err_context)
                {
                    Ok((terminal_id, reader, child_pid)) => {
                        if let Some(child_pid) = child_pid {
                            self.id_to_child_pid.insert(terminal_id, child_pid);
                            self.capture_initial_cwd(terminal_id, child_pid, requested_cwd);
                        }
                        Ok(Some((terminal_id, starts_held, None, Some(Ok(reader)))))
                    },
                    Err(err) => match pane_start_failure(&err) {
                        Some((terminal_id, _)) => {
                            Ok(Some((terminal_id, starts_held, None, Some(Err(err)))))
                        },
                        None => Err(err),
                    },
                }
            },
            Some(Run::EditFile(path_to_file, line_number, cwd)) => {
                let starts_held = false; // we do not hold edit panes (for now?)
                match self
                    .bus
                    .os_input
                    .as_mut()
                    .context("no OS I/O interface found")
                    .with_context(err_context)?
                    .spawn_terminal(
                        TerminalAction::OpenFile(OpenFilePayload::new(
                            path_to_file,
                            line_number,
                            cwd.clone(),
                        )),
                        quit_cb,
                        self.default_editor.clone(),
                    )
                    .with_context(err_context)
                {
                    Ok((terminal_id, reader, child_pid)) => {
                        if let Some(child_pid) = child_pid {
                            self.id_to_child_pid.insert(terminal_id, child_pid);
                            self.capture_initial_cwd(terminal_id, child_pid, cwd);
                        }
                        Ok(Some((terminal_id, starts_held, None, Some(Ok(reader)))))
                    },
                    Err(err) => match pane_start_failure(&err) {
                        Some((terminal_id, _)) => {
                            Ok(Some((terminal_id, starts_held, None, Some(Err(err)))))
                        },
                        None => Err(err),
                    },
                }
            },
            None => {
                let starts_held = false;
                match self
                    .bus
                    .os_input
                    .as_mut()
                    .context("no OS I/O interface found")
                    .with_context(err_context)?
                    .spawn_terminal(default_shell.clone(), quit_cb, self.default_editor.clone())
                    .with_context(err_context)
                {
                    Ok((terminal_id, reader, child_pid)) => {
                        if let Some(child_pid) = child_pid {
                            self.id_to_child_pid.insert(terminal_id, child_pid);
                            self.capture_initial_cwd(
                                terminal_id,
                                child_pid,
                                Self::action_cwd(&default_shell),
                            );
                        }
                        Ok(Some((terminal_id, starts_held, None, Some(Ok(reader)))))
                    },
                    Err(err) => match pane_start_failure(&err) {
                        Some((terminal_id, _)) => {
                            Ok(Some((terminal_id, starts_held, None, Some(Err(err)))))
                        },
                        None => Err(err),
                    },
                }
            },
            // Investigate moving plugin loading to here.
            Some(Run::Plugin(_)) => Ok(None),
        }
    }
    /// Record what a pane's command says about the pane itself, each time one starts in it.
    fn track_launch(&mut self, terminal_id: TerminalId, command: &RunCommand) {
        self.terminal_launch_commands
            .insert(terminal_id, command.clone());
    }

    /// The remote check comes first, and deliberately so. For a remote pane
    /// `id_to_child_pid` holds nothing usable, and a pid from another machine is
    /// worse than no pid at all: pid spaces are per-machine, so it names some
    /// unrelated *local* process, and a `SIGKILL` meant for a pane lands on it.
    /// Asking "is this pane remote" before "what is its pid" is what keeps that
    /// from being possible.
    fn signal_pane_child(&self, terminal_id: TerminalId, signal: ChildSignal) {
        let err_context = || format!("failed to send {signal:?} to pane {terminal_id}");
        let os_input = match self.bus.os_input.as_ref() {
            Some(os_input) => os_input,
            None => return,
        };
        if os_input.is_remote_pane(terminal_id) {
            os_input
                .signal_remote_child(terminal_id, signal)
                .with_context(err_context)
                .non_fatal();
            return;
        }
        match self.id_to_child_pid.get(&terminal_id) {
            Some(&child_pid) => {
                let result = match signal {
                    ChildSignal::Hup => os_input.kill(child_pid),
                    ChildSignal::Int => os_input.send_sigint(child_pid),
                    ChildSignal::Kill => os_input.force_kill(child_pid),
                    ChildSignal::Cont => os_input.send_sigcont(child_pid),
                };
                result.with_context(err_context).non_fatal();
            },
            None => {
                log::warn!("Terminal pane {terminal_id} not found or not running");
            },
        }
    }

    pub(crate) fn close_pane(&mut self, id: PaneId) -> Result<()> {
        let err_context = || format!("failed to close for pane {id:?}");
        match id {
            PaneId::Terminal(id) => {
                self.pending_remote_spawns.remove(&id);
                if let Some(mut agent) = self.pending_agent_spawns.remove(&id)
                    && let Some(completion) = agent.completion_tx.as_mut()
                {
                    completion.set_error_message("pane closed during agent startup".into());
                }
                // Dropping a queued rerun's completion notifies its waiter; the
                // pane it asked about is gone.
                self.reruns.remove(&id);
                // Abort and move on. The reader stops at its next await point;
                // nothing here reuses its socket, so there is nothing to wait
                // for (the warp path, which does reuse it, waits in
                // `stop_readers_for_warp`).
                if let Some(handle) = self.task_handles.remove(&id) {
                    handle.abort();
                }
                // Signal before forgetting the pid: the helper needs it, and for
                // a remote pane there is no pid to forget in the first place.
                self.signal_pane_child(id, ChildSignal::Hup);
                self.id_to_child_pid.remove(&id);
                self.terminal_launch_commands.remove(&id);
                self.pane_activity_flags.remove(&id);
                self.forget_terminal_cwd(id);
                self.forget_terminal_host(id);
                self.terminal_cmds.remove(&id);
                self.forget_terminal_foreground(id);
                let teardown = self
                    .bus
                    .os_input
                    .as_ref()
                    .context("no OS I/O interface found")
                    .and_then(|os_input| os_input.clear_terminal_id(id))
                    .with_context(err_context);
                teardown.non_fatal();
            },
            PaneId::Plugin(pid) => drop(
                self.bus
                    .senders
                    .send_to_plugin(PluginInstruction::Unload(pid)),
            ),
        }
        Ok(())
    }
    pub(crate) fn close_tab(&mut self, ids: Vec<PaneId>) -> Result<()> {
        for id in ids {
            self.close_pane(id)
                .with_context(|| format!("failed to close tab for pane {id:?}"))?;
        }
        Ok(())
    }

    pub(crate) fn set_active_pane(&mut self, pane_id: Option<PaneId>, client_id: ClientId) {
        if let Some(pane_id) = pane_id {
            self.active_panes.insert(client_id, pane_id);
        }
    }
    /// A departed client is focused nowhere: its pane no longer gives new panes their cwd, and
    /// it is no longer among the clients a cwd or command change concerns.
    pub(crate) fn remove_client(&mut self, client_id: ClientId) {
        self.active_panes.remove(&client_id);
    }
    pub(crate) fn rerun_command_in_pane(
        &mut self,
        pane_id: PaneId,
        mut run_command: RunCommand,
    ) -> Result<()> {
        let err_context = || format!("failed to rerun command in pane {:?}", pane_id);
        match pane_id {
            PaneId::Terminal(id) => {
                match run_command.originating_plugin.as_ref() {
                    // A per-step context set by the caller wins over the pane's original one,
                    // and becomes the answer for later reruns of this pane too.
                    Some(originating_plugin) => {
                        self.originating_plugins
                            .insert(id, originating_plugin.clone());
                    },
                    None => {
                        if let Some(originating_plugins) = self.originating_plugins.get(&id) {
                            run_command.originating_plugin = Some(originating_plugins.clone());
                        }
                    },
                }
                self.track_launch(id, &run_command);
                let _ = self.task_handles.remove(&id); // if all is well, this shouldn't be here
                let _ = self.id_to_child_pid.remove(&id); // if all is wlel, this shouldn't be here
                let previous_host = self.host_of_pane(&PaneId::Terminal(id));
                let requested_cwd = run_command.cwd.clone();
                let rerun = self
                    .bus
                    .os_input
                    .as_mut()
                    .context("no OS I/O interface found")
                    .and_then(|os_input| {
                        os_input.re_run_command_in_terminal(
                            id,
                            run_command.clone(),
                            terminal_quit_callback(
                                self.bus.senders.clone(),
                                &TerminalAction::RunCommand(run_command),
                            ),
                        )
                    })
                    .map(|(reader, child_pid)| SpawnedRerun { reader, child_pid });
                self.finish_rerun(pane_id, previous_host, requested_cwd, rerun)
                    .with_context(err_context)
            },
            _ => Err(anyhow!("cannot respawn plugin panes")).with_context(err_context),
        }
    }

    fn finish_rerun(
        &mut self,
        pane_id: PaneId,
        previous_host: Option<String>,
        requested_cwd: Option<PathBuf>,
        spawned: Result<SpawnedRerun>,
    ) -> Result<()> {
        let PaneId::Terminal(id) = pane_id else {
            return Err(anyhow!("cannot respawn plugin panes"));
        };
        if previous_host != self.host_of_pane(&pane_id) {
            self.forget_terminal_cwd(id);
        }
        self.re_report_terminal_host(id);
        let SpawnedRerun { reader, child_pid } = spawned?;
        let activity_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let terminal_bytes = async_runtime().spawn({
            let senders = self.bus.senders.clone();
            let debug_to_file = self.debug_to_file;
            let activity_flag = activity_flag.clone();
            async move {
                TerminalBytes::new(id, reader, senders, debug_to_file, activity_flag)
                    .listen()
                    .await
                    .with_context(|| format!("failed to run async task for pane {pane_id:?}"))
                    .fatal();
            }
        });
        self.task_handles.insert(id, terminal_bytes);
        self.pane_activity_flags.insert(id, activity_flag);
        if let Some(child_pid) = child_pid {
            self.id_to_child_pid.insert(id, child_pid);
            self.capture_initial_cwd(id, child_pid, requested_cwd);
        }
        if let Some(plugin) = self.originating_plugins.get(&id) {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::for_plugin_event(
                    plugin.plugin_id,
                    plugin.client_id,
                    plugin.generation,
                    Event::CommandPaneReRun(id, plugin.context.clone()),
                ))?;
        }
        Ok(())
    }

    pub(crate) fn reconfigure(
        &mut self,
        default_editor: Option<PathBuf>,
        post_command_discovery_hook: Option<String>,
    ) {
        self.default_editor = default_editor;
        self.post_command_discovery_hook = post_command_discovery_hook;
    }

    pub(crate) fn send_sigint_to_pane(&self, pane_id: PaneId) {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                self.signal_pane_child(terminal_id, ChildSignal::Int);
            },
            PaneId::Plugin(plugin_id) => {
                log::warn!("Cannot send SIGINT to plugin pane {}", plugin_id);
            },
        }
    }

    /// Tell the OS layer where to send suspend/resume news.
    ///
    /// Registered once for the whole session rather than per pane: unlike
    /// `quit_cb` this carries no per-pane policy, so one closure serves every
    /// pane and no spawn signature has to grow a parameter for it.
    pub(crate) fn register_child_state_callback(&self) {
        let Some(os_input) = self.bus.os_input.as_ref() else {
            return;
        };
        let senders = self.bus.senders.clone();
        os_input.set_child_state_callback(Arc::new(move |terminal_id, state| {
            let suspended = matches!(state, ChildRunState::Stopped);
            let _ = senders.send_to_screen(ScreenInstruction::SetPaneSuspended(
                PaneId::Terminal(terminal_id),
                suspended,
            ));
        }));
    }

    /// Tell the OS layer where a remote pane's agent hook reports go: the same
    /// place `yinz action agent-report` takes a local pane's, minus the
    /// action. Registered once, like the child state callback.
    pub(crate) fn register_agent_report_callback(&self) {
        let Some(os_input) = self.bus.os_input.as_ref() else {
            return;
        };
        let senders = self.bus.senders.clone();
        os_input.set_agent_report_callback(Arc::new(move |terminal_id, kind, json| {
            handle_remote_agent_report(&senders, terminal_id, &kind, json)
                .with_context(|| format!("failed to apply an agent report from pane {terminal_id}"))
                .non_fatal();
        }));
    }

    /// Wake a pane whose root process was stopped by `^Z`.
    ///
    /// The pane's own `SIGCONT` can never come from inside it: the process is
    /// stopped, and a command pane has no shell above it to run `fg`.
    pub(crate) fn resume_suspended_pane(&self, pane_id: PaneId) {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                self.signal_pane_child(terminal_id, ChildSignal::Cont);
            },
            PaneId::Plugin(plugin_id) => {
                log::warn!("Cannot resume plugin pane {}", plugin_id);
            },
        }
    }

    pub(crate) fn send_sigkill_to_pane(&self, pane_id: PaneId) {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                self.signal_pane_child(terminal_id, ChildSignal::Kill);
            },
            PaneId::Plugin(plugin_id) => {
                log::warn!("Cannot send SIGKILL to plugin pane {}", plugin_id);
            },
        }
    }
}

impl Drop for Pty {
    // SSH brokers keep their processes available for later adoption.
    fn drop(&mut self) {
        let child_ids: HashSet<TerminalId> = self.id_to_child_pid.keys().copied().collect();
        for id in child_ids {
            self.close_pane(PaneId::Terminal(id))
                .with_context(|| format!("failed to close pane {id}"))
                .non_fatal();
        }
    }
}

/// A pane that could not be started, told apart from a failure that should end
/// the session — and where to put the message.
///
/// Everything the pty thread can show the user instead of dying on comes through
/// here: a command that is not there, and a pane on another machine that could
/// not be opened at all. Both name the terminal id they were reserved under, and
/// anything else is a failure this thread has no pane to blame it on.
fn pane_start_failure(err: &anyhow::Error) -> Option<(TerminalId, String)> {
    match err.downcast_ref::<YinzError>()? {
        YinzError::CommandNotFound {
            terminal_id,
            command,
        } => Some((
            *terminal_id,
            format!(
                "Command not found: {command}\n\rIf you were including arguments as part of \
                 the command, try including them as 'args' instead."
            ),
        )),
        YinzError::PaneStartFailed {
            terminal_id,
            message,
        } => Some((*terminal_id, message.clone())),
        _ => None,
    }
}

fn send_pane_error_to_screen(
    senders: ThreadSenders,
    terminal_id: TerminalId,
    message: String,
    run_command: RunCommand,
) -> Result<()> {
    let err_context = || format!("failed to report a failed start for terminal {terminal_id}");
    senders
        .send_to_screen(ScreenInstruction::PtyBytes(
            terminal_id,
            message.into_bytes(),
        ))
        .with_context(err_context)?;
    senders
        .send_to_screen(ScreenInstruction::HoldPane(
            PaneId::Terminal(terminal_id),
            Some(2),
            run_command.clone(),
        ))
        .with_context(err_context)?;
    Ok(())
}

#[cfg(test)]
#[path = "./unit/pty_tests.rs"]
mod pty_tests;
