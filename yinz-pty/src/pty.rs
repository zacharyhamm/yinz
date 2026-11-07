use crate::global_async_runtime::get_tokio_runtime as async_runtime;
use crate::os_input_output::{ChildRunState, ChildSignal, TerminalProcesses};
use crate::plugins::{PluginId, PluginInstruction};
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
use yinz_bus::pty_writer::PtyWriteInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::command::PaneTarget;
use yinz_types::data::{Event, NewPanePlacement, OriginatingPlugin};
use yinz_types::input::command::{OpenFilePayload, RunCommand, TerminalAction};
use yinz_types::input::layout::Run;
use yinz_types::internal_ids::{ProcessId, TabId, TerminalId};
use yinz_types::pane_id::PaneId;
use yinz_types::pane_size::TerminalSize;

mod layouts;
mod metadata;
mod migration;
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
    Terminal(PendingTerminalSpawn),
    Rerun(Box<PendingRerun>),
}

/// A pane's active launch generation and the reruns queued behind it. Initial
/// starts and reruns share this queue, so two launches cannot stream into one pane.
#[derive(Debug, Default)]
struct RerunState {
    generation: u64,
    queued: VecDeque<(RunCommand, Option<NotificationEnd>)>,
}

/// Placement and cancellation belong to one launch generation. Keep cancellation
/// alive after acknowledging placement, while target resolution may still run.
struct PendingPlacement {
    generation: u64,
    sender: Option<std::sync::mpsc::Sender<TerminalSize>>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
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
    "a pane was started but could not be handed back to the pty thread; \
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
    if command.target == PaneTarget::Local {
        return Ok(());
    }
    senders.send_to_screen(ScreenInstruction::PtyBytes(
        terminal_id,
        remote_loading_screen(command, status),
    ))
}

/// Resolve where one command runs and start it there, off the pty thread.
fn spawn_placed_terminal(
    os_input: &dyn TerminalProcesses,
    senders: &ThreadSenders,
    pending: &PendingTerminalSpawn,
    default_editor: Option<PathBuf>,
    placement: &std::sync::mpsc::Receiver<TerminalSize>,
    cancelled: &std::sync::atomic::AtomicBool,
    quit_cb: TerminalQuitCallback,
) -> Result<SpawnedTerminal> {
    let terminal_action = pending.terminal_action.clone();
    let mut report_progress = |phase: &str| {
        if let TerminalAction::RunCommand(command) = &terminal_action {
            send_remote_status(senders, pending.terminal_id, command, phase).non_fatal();
        }
    };
    os_input
        .resolve_spawn(
            pending.terminal_id,
            terminal_action.clone(),
            default_editor,
            &mut report_progress,
        )
        .and_then(|mut resolved| {
            resolved.initial_size = Some(placement.recv().context("pane closed before placement")?);
            anyhow::ensure!(
                !cancelled.load(std::sync::atomic::Ordering::Acquire),
                "pane closed before command startup"
            );
            if let TerminalAction::RunCommand(command) = &terminal_action {
                send_remote_status(
                    senders,
                    pending.terminal_id,
                    command,
                    starting_status(&command.target),
                )
                .non_fatal();
            }
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
    placement: std::sync::mpsc::Receiver<TerminalSize>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    quit_cb: TerminalQuitCallback,
) -> (PendingSpawn, Vec<TerminalId>) {
    match job {
        SpawnJob::Terminal(pending) => {
            let spawned = spawn_placed_terminal(
                os_input,
                senders,
                &pending,
                default_editor,
                &placement,
                &cancelled,
                quit_cb,
            );
            let started = vec![pending.terminal_id];
            (
                PendingSpawn::Terminal(pending, OneShot::new(spawned)),
                started,
            )
        },
        SpawnJob::Rerun(pending) => {
            let spawned = match pending.pane_id {
                PaneId::Terminal(terminal_id) => {
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
                        .and_then(|mut resolved| {
                            resolved.initial_size = Some(
                                placement
                                    .recv()
                                    .context("pane closed before rerun placement")?,
                            );
                            anyhow::ensure!(
                                !cancelled.load(std::sync::atomic::Ordering::Acquire),
                                "pane closed before command restart"
                            );
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

type TerminalQuitCallback = Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>;

/// A child can exit inside spawn_resolved. Its notifications must wait until
/// SpawnResolved is enqueued, so Screen's ClosePane cannot overtake startup.
struct SpawnExitGate {
    callback: TerminalQuitCallback,
    handback: Option<bool>,
    pending: Option<(PaneId, Option<i32>, RunCommand)>,
}

impl SpawnExitGate {
    fn callback(gate: &Arc<std::sync::Mutex<Self>>) -> TerminalQuitCallback {
        let gate = gate.clone();
        Box::new(move |pane, status, command| {
            let result = (|| -> Result<()> {
                let mut gate = gate.lock().to_anyhow()?;
                match gate.handback {
                    Some(true) => (gate.callback)(pane, status, command),
                    Some(false) => {},
                    None => gate.pending = Some((pane, status, command)),
                }
                Ok(())
            })();
            result
                .context("defer terminal exit until spawn handback")
                .non_fatal();
        })
    }

    fn release(&mut self, handed_back: bool) {
        self.handback = Some(handed_back);
        if let Some((pane, status, command)) = self.pending.take()
            && handed_back
        {
            (self.callback)(pane, status, command);
        }
    }
}

fn terminal_quit_callback(
    senders: ThreadSenders,
    terminal_action: &TerminalAction,
) -> TerminalQuitCallback {
    let (hold_on_close, command_plugin, edit_plugin) = match terminal_action {
        TerminalAction::RunCommand(command) => (
            command.hold_on_close,
            command.originating_plugin.clone(),
            None,
        ),
        TerminalAction::OpenFile(payload) => (false, None, payload.originating_plugin.clone()),
    };
    let restoring_agent = matches!(terminal_action, TerminalAction::RunCommand(command) if command.resume_agent.is_some());
    Box::new(move |pane_id, exit_status, command| {
        // Says only what is known: this exit may follow hours of work in the resumed session.
        if restoring_agent
            && let Some(status) = exit_status.filter(|status| *status != 0)
            && let PaneId::Terminal(id) = pane_id
        {
            let _ = senders.send_to_screen(ScreenInstruction::PtyBytes(
                id,
                format!("\r\nResumed agent exited with status {status}.\r\n").into_bytes(),
            ));
        }

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
    /// Supplied by the server so snapshot decoding stays outside the PTY dependency boundary.
    validate_terminal_states: fn(&[&str]) -> Result<()>,
    task_handles: HashMap<TerminalId, JoinHandle<()>>, // terminal_id to join-handle
    reader_controls: HashMap<TerminalId, crate::terminal_bytes::ReaderControl>,
    migration_readers_paused: bool,
    warp_recovery: HashMap<TerminalId, yinz_types::data::WarpReplay>,
    migration_owned: Option<(String, Vec<(TerminalId, bool)>)>,
    migration_recovery_snapshot: Option<(
        PathBuf,
        yinz_types::session_migration::SessionMigrationSnapshot,
    )>,
    default_editor: Option<PathBuf>,
    post_command_discovery_hook: Option<String>,
    plugin_cwds: HashMap<PluginId, PathBuf>, // plugin_id -> cwd
    /// Once a layout has been reported, a session gone clean again still has to replace it:
    /// otherwise periodic checkpoints keep rewriting the panes and agents it used to have.
    layout_reported: bool,
    terminal_cwds: HashMap<TerminalId, PathBuf>, // terminal_id -> cwd
    pane_activity_flags: HashMap<TerminalId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    terminal_cmds: HashMap<TerminalId, Vec<String>>,
    /// Missing entries have not reported yet; stored `None` has reported no agent.
    terminal_detected_themes:
        HashMap<TerminalId, Option<yinz_types::agent::theme::DetectedAgentTheme>>,
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
    /// Workers resolve targets without blocking other panes, then wait for placement.
    /// Reaped as new launches start; closing a pane cancels its placement wait.
    spawn_workers: Vec<std::thread::JoinHandle<()>>,
    /// Reserved panes whose worker has not handed a
    /// transport back yet. Closing the pane removes its id, so a late result
    /// can only be torn down, never bring the pane back.
    pending_terminal_spawns: HashMap<TerminalId, u64>,
    /// Placement replies and cancellation for each unresolved launch.
    awaiting_placement: HashMap<TerminalId, PendingPlacement>,
    /// Actions that wait for process startup as well as placement.
    pending_start_completions: HashMap<TerminalId, NotificationEnd>,
    /// Scrollback dumps opened in an in-place editor, keyed by that editor's pane. Each is
    /// deleted when its pane closes or fails to start. A pane handed to another session is
    /// forgotten here without deleting its file, and the session that takes it claims the dump
    /// from the editor's command (`claim_scrollback_dump`).
    scrollback_dumps: HashMap<TerminalId, PathBuf>,
    /// Failed launches keep reruns queued until the writer discards old input.
    pending_write_cancellations: HashMap<TerminalId, u64>,
    /// Per pane, the rerun off-thread now and the ones waiting behind it.
    reruns: HashMap<TerminalId, RerunState>,
    next_rerun_generation: u64,
}

impl Pty {
    fn start_terminal_reader(
        &mut self,
        terminal_id: TerminalId,
        reader: Box<dyn crate::os_input_output::AsyncReader>,
    ) {
        self.start_terminal_reader_paused(terminal_id, reader, false);
    }

    fn start_terminal_reader_paused(
        &mut self,
        terminal_id: TerminalId,
        reader: Box<dyn crate::os_input_output::AsyncReader>,
        paused: bool,
    ) {
        let activity_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut bytes = TerminalBytes::new(
            terminal_id,
            reader,
            self.bus.senders.clone(),
            self.debug_to_file,
            activity_flag.clone(),
        );
        let control = bytes.control();
        if paused || self.migration_readers_paused {
            control.pause();
        }
        self.reader_controls.insert(terminal_id, control);
        let handle = async_runtime().spawn(async move {
            bytes
                .listen()
                .await
                .with_context(|| format!("read terminal {terminal_id} output"))
                .fatal();
        });
        self.task_handles.insert(terminal_id, handle);
        self.pane_activity_flags.insert(terminal_id, activity_flag);
    }

    pub fn new(
        bus: Bus<PtyInstruction>,
        debug_to_file: bool,
        default_editor: Option<PathBuf>,
        post_command_discovery_hook: Option<String>,
        validate_terminal_states: fn(&[&str]) -> Result<()>,
    ) -> Self {
        Pty {
            active_panes: HashMap::new(),
            bus,
            id_to_child_pid: HashMap::new(),
            debug_to_file,
            validate_terminal_states,
            task_handles: HashMap::new(),
            reader_controls: HashMap::new(),
            migration_readers_paused: false,
            warp_recovery: HashMap::new(),
            migration_owned: None,
            migration_recovery_snapshot: None,
            default_editor,
            originating_plugins: HashMap::new(),
            terminal_launch_commands: HashMap::new(),
            post_command_discovery_hook,
            plugin_cwds: HashMap::new(),
            layout_reported: false,
            terminal_cwds: HashMap::new(),
            pane_activity_flags: HashMap::new(),
            terminal_cmds: HashMap::new(),
            terminal_detected_themes: HashMap::new(),
            terminal_reported_commands: HashMap::new(),
            adopted_tabs: HashMap::new(),
            spawn_workers: vec![],
            pending_terminal_spawns: HashMap::new(),
            awaiting_placement: HashMap::new(),
            pending_start_completions: HashMap::new(),
            scrollback_dumps: HashMap::new(),
            pending_write_cancellations: HashMap::new(),
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
    fn defer_spawn(&mut self, mut job: SpawnJob) -> Result<()> {
        let os_input = self
            .bus
            .os_input
            .as_ref()
            .context("no OS I/O interface found")?
            .box_clone();
        let default_editor = self.default_editor.clone();
        let senders = self.bus.senders.clone();
        let (placement_tx, placement_rx) = std::sync::mpsc::channel();
        let (terminal_id, generation) = match &mut job {
            SpawnJob::Terminal(pending) => {
                pending.generation = self.next_rerun_generation;
                self.next_rerun_generation = self.next_rerun_generation.wrapping_add(1);
                (pending.terminal_id, pending.generation)
            },
            SpawnJob::Rerun(pending) => {
                let PaneId::Terminal(id) = pending.pane_id else {
                    anyhow::bail!("cannot rerun a plugin");
                };
                (id, pending.generation)
            },
        };
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.awaiting_placement.insert(
            terminal_id,
            PendingPlacement {
                generation,
                sender: Some(placement_tx),
                cancelled: cancelled.clone(),
            },
        );
        if let Err(error) =
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::ConfirmTerminalPlacement(
                    terminal_id,
                    generation,
                ))
        {
            self.close_pane(PaneId::Terminal(terminal_id))
                .context("release unplaced terminal")?;
            return Err(error).context("request terminal placement dimensions");
        }
        match &job {
            SpawnJob::Terminal(pending) => {
                self.pending_terminal_spawns
                    .insert(pending.terminal_id, pending.generation);
                self.reruns.insert(
                    pending.terminal_id,
                    RerunState {
                        generation: pending.generation,
                        queued: VecDeque::new(),
                    },
                );
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
        let action = match &job {
            SpawnJob::Terminal(pending) => pending.terminal_action.clone(),
            SpawnJob::Rerun(pending) => TerminalAction::RunCommand(pending.run_command.clone()),
        };
        let exit_gate = Arc::new(std::sync::Mutex::new(SpawnExitGate {
            callback: terminal_quit_callback(senders.clone(), &action),
            handback: None,
            pending: None,
        }));
        self.reap_spawn_workers();
        self.spawn_workers.push(std::thread::spawn(move || {
            let (resolved, started) = run_spawn_job(
                &*os_input,
                &senders,
                job,
                default_editor,
                placement_rx,
                cancelled,
                SpawnExitGate::callback(&exit_gate),
            );
            let handed_back = senders
                .send_to_pty(PtyInstruction::SpawnResolved(resolved))
                .is_ok();
            match exit_gate
                .lock()
                .to_anyhow()
                .context("release spawn exit callback")
            {
                Ok(mut gate) => gate.release(handed_back),
                Err(error) => Err::<(), _>(error).non_fatal(),
            }
            if !handed_back {
                Err::<(), _>(anyhow!(LOST_SPAWN_HANDBACK)).non_fatal();
                for terminal_id in started {
                    os_input.clear_terminal_id(terminal_id).non_fatal();
                }
            }
        }));
        Ok(())
    }

    /// Queue behind an active launch, or resolve this rerun on a worker and
    /// wait for Screen's current content dimensions before executing it.
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
        self.track_launch(terminal_id, &run_command);
        let _ = self.task_handles.remove(&terminal_id);
        self.reader_controls.remove(&terminal_id);
        self.warp_recovery.remove(&terminal_id);
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

    fn handle_terminal_spawned(
        &mut self,
        pending: PendingTerminalSpawn,
        spawned: OneShot<Result<SpawnedTerminal>>,
    ) -> Result<()> {
        let terminal_id = pending.terminal_id;
        let spawned = spawned.take()?;
        if self.pending_terminal_spawns.get(&terminal_id) != Some(&pending.generation) {
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
        self.pending_terminal_spawns.remove(&terminal_id);
        self.awaiting_placement.remove(&terminal_id);
        let mut queued = self
            .reruns
            .remove(&terminal_id)
            .map(|state| state.queued)
            .unwrap_or_default();
        let mut keep_pane = true;
        let mut completion = self.pending_start_completions.remove(&terminal_id);
        let command = match &pending.terminal_action {
            TerminalAction::RunCommand(command) => Some(command.clone()),
            TerminalAction::OpenFile(_) => None,
        };
        if let Err(error) = &spawned
            && let Some(completion) = completion.as_mut()
        {
            completion.set_error_message(format!("command failed to start: {error:#}"));
        }
        match spawned {
            Ok(spawned) => {
                if command.as_ref().is_some_and(RunCommand::defers_spawn) {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::RemotePaneReady(terminal_id))?;
                }
                self.start_spawned_terminal(&pending.terminal_action, spawned);
                match &pending.terminal_action {
                    TerminalAction::RunCommand(command) if !command.defers_spawn() => {
                        self.announce_pane_opened(terminal_id, Some(command), None)?;
                    },
                    TerminalAction::OpenFile(payload) => {
                        self.announce_pane_opened(terminal_id, None, Some(payload))?;
                    },
                    _ => {},
                }
            },
            Err(error) => {
                let show_failure = command
                    .as_ref()
                    .is_some_and(|command| command.shows_start_failure() || command.defers_spawn());
                if show_failure {
                    self.cancel_failed_startup_writes(
                        terminal_id,
                        pending.generation,
                        std::mem::take(&mut queued),
                    )?;
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::PtyBytes(
                            terminal_id,
                            b"\x1b[3J\x1b[2J\x1b[H".to_vec(),
                        ))?;
                    let command = command.context("a command pane must have a command")?;
                    if let Some(plugin) = command.originating_plugin.as_ref() {
                        self.bus
                            .senders
                            .send_to_plugin(PluginInstruction::for_plugin_event(
                                plugin.plugin_id,
                                plugin.client_id,
                                plugin.generation,
                                Event::CommandPaneExited(
                                    terminal_id,
                                    Some(2),
                                    plugin.context.clone(),
                                ),
                            ))?;
                    }
                    let message = pane_start_failure(&error)
                        .map(|(_, message)| message)
                        .unwrap_or_else(|| format!("{error:#}"));
                    send_pane_error_to_screen(
                        self.bus.senders.clone(),
                        terminal_id,
                        message,
                        command,
                    )?;
                } else {
                    Err::<(), _>(error)
                        .context("failed to start placed terminal")
                        .non_fatal();
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::ClosePane(
                            PaneId::Terminal(terminal_id),
                            None,
                            None,
                            None,
                        ))?;
                    self.close_pane(PaneId::Terminal(terminal_id))?;
                    keep_pane = false;
                }
            },
        }
        if keep_pane && !self.pending_write_cancellations.contains_key(&terminal_id) {
            self.run_queued_reruns(PaneId::Terminal(terminal_id), queued)?;
        }
        Ok(())
    }

    /// Release the failed reservation and retain retries until the writer has
    /// discarded its buffered input and completed the pending write requests.
    fn cancel_failed_startup_writes(
        &mut self,
        terminal_id: TerminalId,
        generation: u64,
        queued: VecDeque<(RunCommand, Option<NotificationEnd>)>,
    ) -> Result<()> {
        self.bus
            .os_input
            .as_ref()
            .context("no OS I/O interface found")?
            .clear_terminal_id(terminal_id)
            .context("release failed terminal startup")
            .non_fatal();
        self.pending_write_cancellations
            .insert(terminal_id, generation);
        self.reruns
            .insert(terminal_id, RerunState { generation, queued });
        self.bus
            .senders
            .send_to_pty_writer(PtyWriteInstruction::CancelWrites {
                terminal_id,
                generation,
            })
            .context("cancel writes for failed terminal startup")
    }

    fn startup_writes_cancelled(&mut self, terminal_id: TerminalId, generation: u64) -> Result<()> {
        if self.pending_write_cancellations.get(&terminal_id) != Some(&generation) {
            return Ok(());
        }
        self.pending_write_cancellations.remove(&terminal_id);
        let queued = self
            .reruns
            .remove(&terminal_id)
            .map(|state| state.queued)
            .unwrap_or_default();
        self.run_queued_reruns(PaneId::Terminal(terminal_id), queued)
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
        self.awaiting_placement.remove(&terminal_id);
        let mut queued = self
            .reruns
            .remove(&terminal_id)
            .map(|state| state.queued)
            .unwrap_or_default();
        let spawned = spawned.take()?;
        if spawned.is_err() {
            self.cancel_failed_startup_writes(
                terminal_id,
                pending.generation,
                std::mem::take(&mut queued),
            )?;
        }
        if pending.run_command.defers_spawn() {
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
        }
        let result = self.finish_rerun(
            pending.pane_id,
            pending.previous_host,
            pending.requested_cwd,
            spawned,
        );
        finish_command_rerun(self, pending.pane_id, pending.run_command, result)?;
        if !self.pending_write_cancellations.contains_key(&terminal_id) {
            self.run_queued_reruns(pending.pane_id, queued)?;
        }
        Ok(())
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

    fn terminal_placed(
        &mut self,
        terminal_id: TerminalId,
        generation: u64,
        placement: Option<(TerminalSize, TabId)>,
    ) -> Result<()> {
        if self
            .awaiting_placement
            .get(&terminal_id)
            .map(|placement| placement.generation)
            != Some(generation)
        {
            return Ok(());
        }
        let Some((size, tab_id)) = placement.filter(|(size, _)| size.is_positive()) else {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::ClosePane(
                    PaneId::Terminal(terminal_id),
                    None,
                    None,
                    None,
                ))
                .context("close rejected terminal placement")?;
            return self.close_pane(PaneId::Terminal(terminal_id));
        };
        if let Some(completion) = self.pending_start_completions.get_mut(&terminal_id) {
            completion.complete_pane_placement(tab_id);
        }
        if let Some(sender) = self
            .awaiting_placement
            .get_mut(&terminal_id)
            .and_then(|placement| placement.sender.take())
        {
            // A failed resolution may already have dropped its receiver.
            let _ = sender.send(size);
        }
        Ok(())
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
        self.start_terminal_reader(terminal_id, reader);
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

    /// Reserve and place a pane, then let its worker start with the assigned size.
    fn spawn_pane(
        &mut self,
        terminal_action: Option<TerminalAction>,
        mut pending: PendingPaneSpawn,
    ) -> Result<()> {
        let terminal_action =
            self.prepare_terminal_action(terminal_action, pending.client_or_tab_index);
        let terminal_id = self.reserve_terminal_id()?;
        let starts_held = matches!(&terminal_action, TerminalAction::RunCommand(command) if command.hold_on_start);
        let completes_after_start = !starts_held
            && match &terminal_action {
                TerminalAction::RunCommand(command) => !command.defers_spawn(),
                TerminalAction::OpenFile(_) => true,
            };
        let wait_for_start = !starts_held
            && pending
                .completion_tx
                .as_ref()
                .is_some_and(|completion| completes_after_start || completion.wait_for_agent_start);
        if completes_after_start {
            // The pane must exist for placement, but local opened callbacks still mean
            // its process is running. The action retains both plugin reply targets.
            pending.run_command = None;
            pending.open_file_payload = None;
        }
        if wait_for_start && let Some(completion) = pending.completion_tx.take() {
            let (placement, startup) = completion.split_for_startup();
            pending.completion_tx = Some(placement);
            self.pending_start_completions.insert(terminal_id, startup);
        }
        finish_pane_spawn(self, pending, Ok((terminal_id, starts_held)))?;
        if !starts_held {
            self.defer_spawn(SpawnJob::Terminal(PendingTerminalSpawn {
                terminal_id,
                terminal_action,
                generation: 0,
            }))?;
        }
        Ok(())
    }

    fn apply_run_instruction(
        &mut self,
        run_instruction: Option<Run>,
        default_shell: TerminalAction,
        deferred_spawns: &mut Vec<PendingTerminalSpawn>,
    ) -> Result<Option<SpawnedPane>> {
        let (action, command) = match run_instruction {
            Some(Run::Plugin(_)) => return Ok(None),
            Some(Run::Command(mut command)) => {
                if command.cwd.is_none()
                    && !command.target.is_remote()
                    && let TerminalAction::RunCommand(default) = &default_shell
                {
                    command.cwd = default.cwd.clone();
                }
                (TerminalAction::RunCommand(command.clone()), Some(command))
            },
            Some(Run::Cwd(cwd)) => (
                self.get_default_terminal(Some(cwd), Some(default_shell)),
                None,
            ),
            Some(Run::EditFile(path, line, cwd)) => (
                TerminalAction::OpenFile(OpenFilePayload::new(path, line, cwd)),
                None,
            ),
            None => (default_shell, None),
        };
        let terminal_id = self.reserve_terminal_id()?;
        let held = matches!(&action, TerminalAction::RunCommand(command) if command.hold_on_start);
        if !held {
            deferred_spawns.push(PendingTerminalSpawn {
                terminal_id,
                terminal_action: action,
                generation: 0,
            });
        }
        Ok(Some((terminal_id, held, command, None)))
    }
    /// Record what a pane's command says about the pane itself, each time one starts in it.
    fn track_launch(&mut self, terminal_id: TerminalId, command: &RunCommand) {
        self.claim_scrollback_dump(terminal_id, command);
        self.terminal_launch_commands
            .insert(terminal_id, command.clone());
    }

    /// Take ownership of the scrollback dump an editor command was opened on.
    ///
    /// A dump moves with its editor's command, so a pane adopted from another session (or
    /// rerun after being stranded) names its own dump; from here this session deletes it when
    /// the pane closes. The session it came from forgot it when it let the pane go.
    pub(crate) fn claim_scrollback_dump(&mut self, terminal_id: TerminalId, command: &RunCommand) {
        if let Some(dump) = yinz_bus::scrollback_dump::scrollback_dump_in_command(command) {
            self.scrollback_dumps.insert(terminal_id, dump);
        }
    }

    /// Close a held pane that has moved to another session. Its dump went with it, so it is
    /// forgotten rather than deleted.
    pub(crate) fn close_departed_pane(&mut self, id: PaneId) -> Result<()> {
        if let PaneId::Terminal(terminal_id) = id {
            self.scrollback_dumps.remove(&terminal_id);
        }
        self.close_pane(id)
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

    /// Open a scrollback dump in an editor pane that replaces the target pane in place.
    ///
    /// From here the dump belongs to that editor pane: it is deleted when the pane closes,
    /// including after a failed start, or right away if the pane cannot be set up at all.
    pub(crate) fn open_scrollback_editor(
        &mut self,
        scrollback_dump: PathBuf,
        line_number: Option<usize>,
        client_tab_index_or_pane_id: ClientTabIndexOrPaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || "failed to open in-place editor for client".to_string();

        let terminal_action = self.prepare_terminal_action(
            Some(TerminalAction::OpenFile(OpenFilePayload::new(
                scrollback_dump.clone(),
                line_number,
                None,
            ))),
            client_tab_index_or_pane_id,
        );
        let terminal_id = match self.reserve_terminal_id() {
            Ok(terminal_id) => terminal_id,
            Err(error) => {
                yinz_bus::scrollback_dump::remove_scrollback_dump(&scrollback_dump).non_fatal();
                return Err(error).with_context(err_context);
            },
        };
        self.scrollback_dumps.insert(terminal_id, scrollback_dump);
        if let Some(completion) = completion_tx {
            self.pending_start_completions
                .insert(terminal_id, completion);
        }
        let opened = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::OpenInPlaceEditor(
                PaneId::Terminal(terminal_id),
                client_tab_index_or_pane_id,
            ))
            .and_then(|()| {
                self.defer_spawn(SpawnJob::Terminal(PendingTerminalSpawn {
                    terminal_id,
                    terminal_action,
                    generation: 0,
                }))
            })
            .with_context(err_context);
        if opened.is_err() {
            self.remove_scrollback_dump(terminal_id);
        }
        opened
    }

    /// Delete the scrollback dump the editor in this pane was opened on, if it has one.
    fn remove_scrollback_dump(&mut self, terminal_id: TerminalId) {
        if let Some(path) = self.scrollback_dumps.remove(&terminal_id) {
            yinz_bus::scrollback_dump::remove_scrollback_dump(&path).non_fatal();
        }
    }

    pub(crate) fn close_pane(&mut self, id: PaneId) -> Result<()> {
        let err_context = || format!("failed to close for pane {id:?}");
        match id {
            PaneId::Terminal(id) => {
                self.pending_terminal_spawns.remove(&id);
                self.pending_write_cancellations.remove(&id);
                if let Some(placement) = self.awaiting_placement.remove(&id) {
                    placement
                        .cancelled
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                if let Some(mut completion) = self.pending_start_completions.remove(&id) {
                    completion.set_error_message("pane closed during command startup".into());
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
                self.reader_controls.remove(&id);
                // Signal before forgetting the pid: the helper needs it, and for
                // a remote pane there is no pid to forget in the first place.
                self.signal_pane_child(id, ChildSignal::Hup);
                self.id_to_child_pid.remove(&id);
                self.terminal_launch_commands.remove(&id);
                self.remove_scrollback_dump(id);
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
                self.reader_controls.remove(&id);
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
        self.start_terminal_reader(id, reader);
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
        let child_ids: HashSet<TerminalId> = self
            .id_to_child_pid
            .keys()
            .chain(self.pending_terminal_spawns.keys())
            .chain(self.awaiting_placement.keys())
            .copied()
            .collect();
        for id in child_ids {
            self.close_pane(PaneId::Terminal(id))
                .with_context(|| format!("failed to close pane {id}"))
                .non_fatal();
        }
        // Editors whose process this session no longer tracks still leave no dump behind.
        for path in std::mem::take(&mut self.scrollback_dumps).into_values() {
            yinz_bus::scrollback_dump::remove_scrollback_dump(&path).non_fatal();
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
