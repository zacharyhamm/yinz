// Clippy allows kept as debt — see REVIEW_CHECKLIST.md.
#![allow(clippy::too_many_arguments)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::useless_format)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::needless_borrow)]
use crate::background_jobs::write_session_state_to_disk;
use crate::background_jobs::BackgroundJob;
use crate::global_async_runtime::get_tokio_runtime as async_runtime;
use crate::os_input_output::{AsyncReader, NullAsyncReader, ServerOsApi};
use crate::route::NotificationEnd;
use crate::terminal_bytes::TerminalBytes;
use crate::{
    panes::PaneId,
    plugins::{DumpSessionLayoutResponse, PluginId, PluginInstruction},
    screen::{ScreenInstruction, TabOverrideResult},
    session_layout_metadata::SessionLayoutMetadata,
    thread_bus::{Bus, ThreadSenders},
    ClientId, ServerInstruction,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
};
use zellij_types::internal_ids::ProcessId;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;
use zellij_types::internal_ids::TerminalId;

use crate::pty_host::client::{ChildRunState, ReleasedPane};
use crate::pty_host::remote::protocol::ChildSignal;
use tokio::task::JoinHandle;
use zellij_runtime::errors::prelude::*;
use zellij_types::diagnostics::ContextType;
use zellij_types::diagnostics::PtyContext;
use zellij_types::pane_size::Size;
use zellij_types::{agent::AgentInfo, command::PaneTarget, geometry::FloatingPaneCoordinates};
use {
    zellij_session::session_serialization, zellij_types::data::CommandOrPlugin,
    zellij_types::data::Event, zellij_types::data::GetPaneCwdResponse,
    zellij_types::data::GetPaneGitProbeResponse, zellij_types::data::GetPanePidResponse,
    zellij_types::data::GetPaneRunningCommandResponse, zellij_types::data::NewPanePlacement,
    zellij_types::data::OriginatingPlugin, zellij_types::data::OverlayPlacement,
    zellij_types::data::PaneGitProbe, zellij_types::data::SessionInfo,
    zellij_types::data::WarpPlacement, zellij_types::data::WarpedPane,
    zellij_types::data::WarpedTab, zellij_types::input::command::OpenFilePayload,
    zellij_types::input::command::RunCommand, zellij_types::input::command::TerminalAction,
    zellij_types::input::layout::FloatingPaneLayout, zellij_types::input::layout::Layout,
    zellij_types::input::layout::Run, zellij_types::input::layout::RunPluginOrAlias,
    zellij_types::input::layout::SwapFloatingLayout, zellij_types::input::layout::SwapTiledLayout,
    zellij_types::input::layout::TabLayoutInfo, zellij_types::input::layout::TiledPaneLayout,
};

pub type VteBytes = Vec<u8>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientTabIndexOrPaneId {
    ClientId(ClientId),
    ClientIdNoFocus(ClientId),
    TabIndex(TabId),
    TabIndexNoFocus(TabId),
    PaneId(PaneId),
    /// Beside `PaneId`, or - when no tab holds it - in the tab `ClientId` is looking at.
    ///
    /// What a plugin means by "near me". A plugin loaded while no client was attached has no
    /// pane of its own yet: its `AddPlugin` is still parked in
    /// `pending_events_waiting_for_client`, so nothing it opens can be placed beside it. The
    /// pane it asked for must still land somewhere rather than be forgotten and killed.
    PaneIdOrClient(PaneId, ClientId),
}

#[allow(clippy::too_many_arguments)]
fn new_terminal_screen_instruction(
    pane_id: PaneId,
    pane_title: Option<String>,
    hold_for_command: Option<RunCommand>,
    invoked_with: Option<Run>,
    new_pane_placement: NewPanePlacement,
    start_suppressed: bool,
    client_or_tab_index: ClientTabIndexOrPaneId,
    completion_tx: Option<NotificationEnd>,
    set_blocking: bool,
) -> ScreenInstruction {
    ScreenInstruction::NewPane(
        pane_id,
        pane_title,
        hold_for_command,
        invoked_with,
        new_pane_placement,
        start_suppressed,
        client_or_tab_index,
        completion_tx,
        set_blocking,
    )
}

fn finish_terminal_spawn(
    pty: &mut Pty,
    pending: PendingTerminalSpawn,
    result: Result<(TerminalId, bool)>,
) -> Result<()> {
    let err_context = || {
        format!(
            "failed to spawn terminal for {:?}",
            pending.client_or_tab_index
        )
    };
    let show_spawn_error = pending
        .run_command
        .as_ref()
        .is_some_and(|command| command.shows_start_failure());
    match result.with_context(err_context) {
        Ok((pid, starts_held)) => {
            let hold_for_command = starts_held.then(|| pending.run_command.clone()).flatten();
            if let Some(originating_plugin) = pending
                .run_command
                .and_then(|command| command.originating_plugin)
            {
                pty.originating_plugins
                    .insert(pid, originating_plugin.clone());
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::Update(vec![(
                        Some(originating_plugin.plugin_id),
                        Some(originating_plugin.client_id),
                        Event::CommandPaneOpened(pid, originating_plugin.context),
                    )]))
                    .with_context(err_context)?;
            }
            if let Some(originating_plugin) = pending
                .open_file_payload
                .and_then(|payload| payload.originating_plugin)
            {
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::Update(vec![(
                        Some(originating_plugin.plugin_id),
                        Some(originating_plugin.client_id),
                        Event::EditPaneOpened(pid, originating_plugin.context),
                    )]))
                    .with_context(err_context)?;
            }
            pty.bus
                .senders
                .send_to_screen(new_terminal_screen_instruction(
                    PaneId::Terminal(pid),
                    pending.pane_title,
                    hold_for_command,
                    pending.invoked_with,
                    pending.new_pane_placement,
                    pending.start_suppressed,
                    pending.client_or_tab_index,
                    pending.completion_tx,
                    pending.set_blocking,
                ))
                .with_context(err_context)?;
        },
        Err(err) => match pane_start_failure(&err) {
            Some((terminal_id, message)) if show_spawn_error => {
                pty.bus
                    .senders
                    .send_to_screen(new_terminal_screen_instruction(
                        PaneId::Terminal(terminal_id),
                        pending.pane_title,
                        None,
                        pending.invoked_with,
                        pending.new_pane_placement,
                        pending.start_suppressed,
                        pending.client_or_tab_index,
                        pending.completion_tx,
                        pending.set_blocking,
                    ))
                    .with_context(err_context)?;
                if let Some(run_command) = pending.run_command {
                    if let Some(originating_plugin) = run_command.originating_plugin.clone() {
                        pty.bus
                            .senders
                            .send_to_plugin(PluginInstruction::Update(vec![(
                                Some(originating_plugin.plugin_id),
                                Some(originating_plugin.client_id),
                                Event::CommandPaneExited(
                                    terminal_id,
                                    Some(2),
                                    originating_plugin.context,
                                ),
                            )]))
                            .with_context(err_context)?;
                    }
                    send_pane_error_to_screen(
                        pty.bus.senders.clone(),
                        terminal_id,
                        message,
                        run_command,
                    )
                    .with_context(err_context)?;
                }
            },
            Some((terminal_id, _)) => {
                Err::<(), _>(anyhow!("{err:?}"))
                    .context("failed to spawn terminal")
                    .non_fatal();
                pty.close_pane(PaneId::Terminal(terminal_id))
                    .with_context(err_context)?;
            },
            None => Err::<(), _>(err).non_fatal(),
        },
    }
    Ok(())
}

fn finish_in_place_spawn(
    pty: &mut Pty,
    pending: PendingInPlaceSpawn,
    result: Result<(TerminalId, bool)>,
) -> Result<()> {
    let err_context = || {
        format!(
            "failed to spawn terminal for {:?}",
            pending.client_or_tab_index
        )
    };
    let show_spawn_error = pending
        .run_command
        .as_ref()
        .is_some_and(|command| command.shows_start_failure());
    match result.with_context(err_context) {
        Ok((pid, starts_held)) => pty
            .bus
            .senders
            .send_to_screen(ScreenInstruction::ReplacePane(
                PaneId::Terminal(pid),
                starts_held.then_some(pending.run_command).flatten(),
                pending.pane_title,
                pending.invoked_with,
                pending.close_replaced_pane,
                pending.client_or_tab_index,
                pending.completion_tx,
            ))
            .with_context(err_context)?,
        Err(err) => match pane_start_failure(&err) {
            Some((terminal_id, message)) if show_spawn_error => {
                pty.bus
                    .senders
                    .send_to_screen(ScreenInstruction::ReplacePane(
                        PaneId::Terminal(terminal_id),
                        None,
                        pending.pane_title,
                        pending.invoked_with,
                        pending.close_replaced_pane,
                        pending.client_or_tab_index,
                        pending.completion_tx,
                    ))
                    .with_context(err_context)?;
                if let Some(run_command) = pending.run_command {
                    send_pane_error_to_screen(
                        pty.bus.senders.clone(),
                        terminal_id,
                        message,
                        run_command,
                    )
                    .with_context(err_context)?;
                }
            },
            Some((terminal_id, _)) => {
                Err::<(), _>(anyhow!("{err:?}"))
                    .context("failed to spawn terminal")
                    .non_fatal();
                pty.close_pane(PaneId::Terminal(terminal_id))
                    .with_context(err_context)?;
            },
            None => Err::<(), _>(err).non_fatal(),
        },
    }
    Ok(())
}

fn finish_command_rerun(
    pty: &mut Pty,
    pane_id: PaneId,
    run_command: RunCommand,
    result: Result<()>,
) -> Result<()> {
    let err_context = || format!("failed to rerun command in pane {pane_id:?}");
    if let Err(err) = result.with_context(err_context) {
        match pane_start_failure(&err) {
            Some((terminal_id, message)) if run_command.shows_start_failure() => {
                if let Some(originating_plugin) = run_command.originating_plugin.clone() {
                    pty.bus
                        .senders
                        .send_to_plugin(PluginInstruction::Update(vec![(
                            Some(originating_plugin.plugin_id),
                            Some(originating_plugin.client_id),
                            Event::CommandPaneExited(
                                terminal_id,
                                Some(2),
                                originating_plugin.context,
                            ),
                        )]))
                        .with_context(err_context)?;
                }
                send_pane_error_to_screen(
                    pty.bus.senders.clone(),
                    terminal_id,
                    message,
                    run_command,
                )
                .with_context(err_context)?;
            },
            Some(_) => {},
            None => Err::<(), _>(err).non_fatal(),
        }
    }
    Ok(())
}

/// What `Pty::adopt_terminal` hands back so Screen can build the pane.
#[derive(Debug, Clone)]
pub struct AdoptedPane {
    pub terminal_id: TerminalId,
    pub cmd: RunCommand,
    /// Output the supervisor caught while nobody was attached. Replayed into the
    /// pane after the transferred screen and before any live bytes, so the
    /// hand-off costs nothing.
    pub buffered: Vec<u8>,
    /// `Some` when the child is already gone, so the pane can arrive held rather
    /// than pretending to run.
    pub child_exit_status: Option<Option<i32>>,
}

/// Every supervisor in an adopted tab, in the order its layout expects them.
///
/// There is no room here for a pane that failed to attach. A tab's layout places
/// panes by position, so a missing one does not leave a hole — it shifts every
/// pane after it into the wrong slot. `Pty::adopt_tab` therefore takes the whole
/// tab or none of it, releasing anything it had already attached before giving
/// up, which puts the tab back exactly where a rollback can find it.
#[derive(Debug, Clone)]
pub struct AdoptedTabPanes {
    pub tiled: Vec<AdoptedPane>,
    pub floating: Vec<AdoptedPane>,
}

/// One adopted terminal, waiting for the tab that will hold it.
#[derive(Debug, Clone)]
pub struct AdoptedTabTerminal {
    pub terminal_id: TerminalId,
    pub cmd: RunCommand,
    pub agent: Option<AgentInfo>,
    /// `Some` when the child has already exited, so the pane arrives held
    /// showing the status it exited with rather than pretending to run.
    pub child_exit_status: Option<Option<i32>>,
    /// The pane's screen as it was in the session it left, followed by whatever
    /// its supervisor caught while nobody was attached.
    ///
    /// Replayed by the pty thread rather than by Screen, and that is the whole
    /// reason it travels back here. Screen builds a warped tab by asking for a
    /// `NewTab`, which goes out through the plugin thread and only comes back as
    /// an `ApplyLayout` much later — so bytes Screen sent itself would arrive
    /// before the panes existed, and be dropped. Sending them from here puts
    /// them behind that `ApplyLayout` on the same channel, which is exactly
    /// where a normal pane's first output sits.
    pub replay: Vec<u8>,
}

impl AdoptedTabTerminal {
    fn hold_for_command(&self) -> Option<RunCommand> {
        self.child_exit_status.map(|_| self.cmd.clone())
    }
}

/// The tiled and floating halves of a tab's adopted terminals, in the order
/// their slots are filled.
pub type AdoptedTabTerminals = (Vec<AdoptedTabTerminal>, Vec<AdoptedTabTerminal>);

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PendingTerminalSpawn {
    run_command: Option<RunCommand>,
    pane_title: Option<String>,
    open_file_payload: Option<OpenFilePayload>,
    invoked_with: Option<Run>,
    new_pane_placement: NewPanePlacement,
    start_suppressed: bool,
    client_or_tab_index: ClientTabIndexOrPaneId,
    completion_tx: Option<NotificationEnd>,
    set_blocking: bool,
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PendingInPlaceSpawn {
    run_command: Option<RunCommand>,
    pane_title: Option<String>,
    invoked_with: Option<Run>,
    close_replaced_pane: bool,
    client_or_tab_index: ClientTabIndexOrPaneId,
    completion_tx: Option<NotificationEnd>,
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PendingLayoutSpawn {
    layout: TiledPaneLayout,
    floating_panes_layout: Vec<FloatingPaneLayout>,
    default_shell: TerminalAction,
    plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
    adopted_panes: Option<AdoptedTabTerminals>,
    run_instructions: Vec<Option<Run>>,
    floating_run_instructions: Vec<Option<Run>>,
    tab_index: TabId,
    block_on_first_terminal: bool,
    should_change_focus_to_new_tab: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PendingLayoutOverride {
    cwd: Option<PathBuf>,
    default_shell: Option<TerminalAction>,
    tabs: Vec<(TabLayoutInfo, HashMap<RunPluginOrAlias, Vec<PluginId>>)>,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
}

#[doc(hidden)]
pub struct SpawnedTerminal {
    terminal_id: TerminalId,
    reader: Box<dyn AsyncReader>,
    child_pid: Option<ProcessId>,
}

impl std::fmt::Debug for SpawnedTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnedTerminal")
            .field("terminal_id", &self.terminal_id)
            .field("child_pid", &self.child_pid)
            .finish_non_exhaustive()
    }
}

#[doc(hidden)]
pub struct SpawnedRerun {
    reader: Box<dyn AsyncReader>,
    child_pid: Option<ProcessId>,
}

impl std::fmt::Debug for SpawnedRerun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnedRerun")
            .field("child_pid", &self.child_pid)
            .finish_non_exhaustive()
    }
}

#[doc(hidden)]
pub struct OneShot<T>(Arc<std::sync::Mutex<Option<T>>>);

impl<T> OneShot<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(value))))
    }

    fn take(self) -> Result<T> {
        self.0
            .lock()
            .to_anyhow()?
            .take()
            .context("pty worker result was already consumed")
    }
}

impl<T> Clone for OneShot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> std::fmt::Debug for OneShot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OneShot(..)")
    }
}

/// A re-run of a command in an existing pane, while its remote spawn is off
/// the pty thread. Everything the completion needs was captured before the
/// worker started, so the pty thread never has to read pane state that may
/// have changed underneath a boot.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PendingRerun {
    pane_id: PaneId,
    run_command: RunCommand,
    /// Held until the rerun finishes; dropping it then is what notifies the
    /// waiter, so nothing reads it.
    _completion_tx: Option<NotificationEnd>,
    /// Which request this is; a completion whose generation the pane no longer
    /// carries belongs to a pane that was closed meanwhile.
    generation: u64,
    previous_host: Option<String>,
    requested_cwd: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct PendingRemoteSpawn {
    terminal_id: TerminalId,
    terminal_action: TerminalAction,
}

/// A pane a layout has spawned but not yet handed to Screen: its terminal id,
/// whether it starts held, the command it holds for, and the reader its bytes
/// will arrive on (`None` for a pane whose spawn was deferred).
type SpawnedPane = (
    TerminalId,
    bool,
    Option<RunCommand>,
    Option<Result<Box<dyn AsyncReader>>>,
);

/// What a spawn worker was asked to do.
enum SpawnJob {
    Terminal(PendingRemoteSpawn),
    Rerun(Box<PendingRerun>),
}

/// What a spawn worker hands back: the job, plus its result in a `OneShot` so
/// a non-`Clone` reader can ride a `PtyInstruction`.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub enum PendingSpawn {
    Terminal(PendingRemoteSpawn, OneShot<Result<SpawnedTerminal>>),
    Rerun(Box<PendingRerun>, OneShot<Result<SpawnedRerun>>),
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
/// broker channel. Sets the pane's agent record the way `agent-report` does
/// for a local pane, and hands the event to the plugins on the hook pipe.
fn handle_remote_agent_report(
    senders: &ThreadSenders,
    terminal_id: TerminalId,
    kind: &str,
    json: String,
) -> Result<()> {
    use {zellij_types::agent, zellij_types::agent::HookEvent, zellij_types::agent::HookOutcome};
    if json.len() > agent::HOOK_BROADCAST_MAX_BYTES {
        return Err(anyhow!("agent hook event exceeds the 64 KiB limit"));
    }
    let event: HookEvent = serde_json::from_str(&json).context("invalid remote agent report")?;
    match agent::hook_outcome(kind, &event) {
        HookOutcome::Ended => senders.send_to_screen(ScreenInstruction::SetPaneAgent(
            PaneId::Terminal(terminal_id),
            None,
            None,
        ))?,
        HookOutcome::Reported(agent) => senders.send_to_screen(ScreenInstruction::SetPaneAgent(
            PaneId::Terminal(terminal_id),
            Some(agent),
            None,
        ))?,
        HookOutcome::Nothing => {},
    }
    // The same event a local `agent-report --hook-json` sends, read the same way.
    senders.send_to_plugin(PluginInstruction::Update(vec![(
        None,
        None,
        Event::AgentEvent(agent::AgentEvent::from_hook(
            kind,
            terminal_id,
            &event,
            json,
        )),
    )]))
}

/// A spawn worker could not hand its result back to the pty thread, so the pane it
/// started has to be closed by the worker itself: nobody else knows the id.
const LOST_SPAWN_HANDBACK: &str =
    "a remote pane was started but could not be handed back to the pty thread; \
     the pane has been dropped";

fn terminal_safe(text: &str) -> String {
    text.chars().flat_map(char::escape_default).collect()
}

fn remote_loading_screen(command: &RunCommand, status: &str) -> Vec<u8> {
    let status = terminal_safe(status);
    let body = match &command.target {
        PaneTarget::Firecracker {
            vm,
            image,
            worktree,
        } => format!(
            "\u{1b}[H\u{1b}[2J\u{1b}[1mStarting Firecracker VM\u{1b}[0m\r\n\r\nVM: {}\r\nImage: {}\r\nWorktree: {}\r\n\r\nStatus: {}",
            terminal_safe(vm),
            image.as_deref().map(terminal_safe).unwrap_or_else(|| "default".to_owned()),
            worktree
                .as_ref()
                .map(|path| terminal_safe(&path.display().to_string()))
                .unwrap_or_else(|| "none".to_owned()),
            status,
        ),
        PaneTarget::Ssh { host } => format!(
            "\u{1b}[H\u{1b}[2J\u{1b}[1mConnecting to remote host\u{1b}[0m\r\n\r\nHost: {}\r\n\r\nStatus: {}",
            terminal_safe(host),
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
    os_input: &dyn ServerOsApi,
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
                if command.target.is_firecracker() {
                    "connecting to the guest"
                } else {
                    "starting the remote command"
                },
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
    os_input: &dyn ServerOsApi,
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
                                if pending.run_command.target.is_firecracker() {
                                    "connecting to the guest"
                                } else {
                                    "starting the remote command"
                                },
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
        if let PaneId::Terminal(terminal_id) = pane_id {
            if let Some(plugin) = edit_plugin.as_ref() {
                let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
                    Some(plugin.plugin_id),
                    Some(plugin.client_id),
                    Event::EditPaneExited(terminal_id, exit_status, plugin.context.clone()),
                )]));
            }
        }
        if hold_on_close {
            let _ =
                senders.send_to_screen(ScreenInstruction::HoldPane(pane_id, exit_status, command));
        } else {
            let _ = senders.send_to_screen(ScreenInstruction::ClosePane(
                pane_id,
                None,
                None,
                exit_status,
            ));
        }
        if let PaneId::Terminal(terminal_id) = pane_id {
            if let Some(plugin) = command_plugin.as_ref() {
                let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
                    Some(plugin.plugin_id),
                    Some(plugin.client_id),
                    Event::CommandPaneExited(terminal_id, exit_status, plugin.context.clone()),
                )]));
            }
        }
    })
}

/// Instructions related to PTYs (pseudoterminals).
#[derive(Clone, Debug, strum_macros::IntoStaticStr, strum_macros::VariantNames)]
pub enum PtyInstruction {
    SpawnTerminal(
        Option<TerminalAction>,
        Option<String>,
        NewPanePlacement,
        bool, // start suppressed
        ClientTabIndexOrPaneId,
        Option<NotificationEnd>, // completion signal
        bool,                    // set_blocking
    ), // bool (if Some) is
    // should_float, String is an optional pane name
    /// A spawn worker is done: the remote terminals a spawn, layout or rerun
    /// asked for are resolved and started (or failed), and the pty thread can
    /// finish the job the way it would have had the spawn been local.
    SpawnResolved(PendingSpawn),
    OpenInPlaceEditor(
        PathBuf,
        Option<usize>,
        ClientTabIndexOrPaneId,
        Option<NotificationEnd>,
    ), // Option<usize> is the optional line number
    UpdateActivePane(Option<PaneId>, ClientId),
    GoToTab(TabNumber, ClientId),
    NewTab(
        Option<PathBuf>,
        Option<TerminalAction>,
        Option<TiledPaneLayout>,
        Vec<FloatingPaneLayout>,
        TabId,                                    // tab_index
        HashMap<RunPluginOrAlias, Vec<PluginId>>, // plugin_ids
        Option<Vec<CommandOrPlugin>>,             // initial_panes
        bool,                                     // block_on_first_terminal
        bool,                                     // should change focus to new tab
        ClientId,
        Option<NotificationEnd>, // completion signal
    ), // the String is the tab name
    OverrideLayout(
        Option<PathBuf>,                                                // CWD
        Option<TerminalAction>,                                         // Default Shell
        Vec<(TabLayoutInfo, HashMap<RunPluginOrAlias, Vec<PluginId>>)>, // (layout, plugin_ids) per tab
        bool, // retain_existing_terminal_panes
        bool, // retain_existing_plugin_panes
        ClientId,
        Option<NotificationEnd>,
    ),
    ClosePane(PaneId, Option<NotificationEnd>),
    CloseTab(Vec<PaneId>),
    /// Give a pane's supervisor up so another session can take it, keeping the
    /// child alive. The opposite of `ClosePane` in the one way that matters.
    ///
    /// The reply goes back to Screen as `ScreenInstruction::PaneReleasedForWarp`
    /// rather than down a response channel, because Screen must not block on this
    /// thread — and because by the time it lands, the reader task is gone and
    /// every `PtyBytes` it ever emitted is already queued ahead of it, so what
    /// Screen serializes next is exactly what the user was looking at.
    ReleaseTerminalForWarp {
        terminal_id: TerminalId,
        to_session: String,
        placement: WarpPlacement,
        client_id: ClientId,
        /// When the caller stops waiting. See
        /// [`ReleaseTerminalsForWarp`](Self::ReleaseTerminalsForWarp).
        deadline: Instant,
    },
    /// Take a detached supervisor over into this session and give it a pane.
    AdoptTerminal {
        payload: Box<WarpedPane>,
        placement: WarpPlacement,
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Result<()>>,
    },
    /// Give up every supervisor in a tab at once.
    ///
    /// Batched rather than looped by the caller because the reply is what Screen
    /// waits on before serializing the tab: N separate replies would mean N
    /// chances to serialize a tab that is only half released.
    ReleaseTerminalsForWarp {
        tab_index: TabId,
        terminal_ids: Vec<TerminalId>,
        /// The moment the warp's caller stops waiting for an answer.
        ///
        /// Carried rather than re-derived because a release that outlives the
        /// caller is worse than a release that fails: by the time it lands,
        /// Screen has nobody to hand the tab to and every process in it is
        /// running unwatched. A remote pane's release is a round trip over ssh
        /// with a budget of its own, and until this was plumbed through, that
        /// budget was a constant unrelated to the caller's — so a tab with two
        /// remote panes spent the caller's entire ten seconds before the first
        /// word came back. Now every leg answers to the same clock.
        deadline: Instant,
    },
    /// Say in a pane that its warp cut it loose, and hold it.
    ///
    /// For the cases Screen is the one to notice — a tab released for a warp
    /// nobody was waiting on, most of all. The message has to be written on this
    /// thread because holding a pane means naming the command it was running,
    /// and that is one of the things Screen hands *to* the pty thread rather
    /// than one of the things it keeps.
    StrandPaneAfterWarp {
        terminal_id: TerminalId,
        why: String,
        recovery: Option<String>,
    },
    /// Take a whole tab's supervisors over into this session.
    AdoptTab {
        payload: Box<WarpedTab>,
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Result<()>>,
    },
    /// Set aside a tab's adopted terminals for the `NewTab` that will place them.
    ///
    /// Sent by Screen immediately before that `NewTab` goes out through the
    /// plugin thread, which is what makes the pairing safe: this is enqueued
    /// first, so it is always here by the time the tab comes back round, and it
    /// is keyed so that a tab the user opens in the meantime cannot take it.
    ClaimAdoptedTab {
        tab_index: TabId,
        terminals: AdoptedTabTerminals,
    },
    ReRunCommandInPane(PaneId, RunCommand, Option<NotificationEnd>),
    DropToShellInPane {
        pane_id: PaneId,
        shell: Option<PathBuf>,
        working_dir: Option<PathBuf>,
        completion_tx: Option<NotificationEnd>,
    },
    SpawnInPlaceTerminal(
        Option<TerminalAction>,
        Option<String>,
        bool, // close replaced pane
        ClientTabIndexOrPaneId,
        Option<NotificationEnd>, // completion signal
    ), // String is an optional pane name
    DumpLayout(SessionLayoutMetadata, ClientId, Option<NotificationEnd>),
    DumpLayoutToPlugin {
        session_layout_metadata: SessionLayoutMetadata,
        plugin_id: PluginId,
        response_channel: crossbeam::channel::Sender<DumpSessionLayoutResponse>,
    },
    LogLayoutToHd(SessionLayoutMetadata),
    SaveSessionToDisk {
        session_name: String,
        session_info: SessionInfo,
        session_layout_metadata: SessionLayoutMetadata,
        completion_tx: Option<NotificationEnd>,
    },
    FillPluginCwd(
        Option<bool>,   // should float
        bool,           // should be opened in place
        bool,           // close_replaced_pane
        Option<String>, // pane title
        RunPluginOrAlias,
        TabId,          // tab index
        Option<PaneId>, // pane id to replace if this is to be opened "in-place"
        ClientId,
        Size,
        bool,            // skip cache
        Option<PathBuf>, // if Some, will not fill cwd but just forward the message
        Option<bool>,    // should focus plugin
        Option<FloatingPaneCoordinates>,
        Option<NotificationEnd>,
        // the overlay placement the launcher stated outright, rather than the one derived from
        // the plugin's own `overlay`/`modal` configuration
        Option<OverlayPlacement>,
    ),
    ListClientsMetadata(SessionLayoutMetadata, ClientId, Option<NotificationEnd>),
    Reconfigure {
        client_id: ClientId,
        default_editor: Option<PathBuf>,
        post_command_discovery_hook: Option<String>,
    },
    ListClientsToPlugin(SessionLayoutMetadata, PluginId, ClientId),
    ReportPluginCwd(PluginId, PathBuf),
    SendSigintToPaneId(PaneId),
    SendSigkillToPaneId(PaneId),
    /// Continue a pane whose root process was stopped by `^Z`.
    ResumeSuspendedPane(PaneId),
    GetPanePid {
        pane_id: PaneId,
        response_channel: crossbeam::channel::Sender<GetPanePidResponse>,
    },
    GetPaneRunningCommand {
        pane_id: PaneId,
        response_channel: crossbeam::channel::Sender<GetPaneRunningCommandResponse>,
    },
    GetPaneCwd {
        pane_id: PaneId,
        response_channel: crossbeam::channel::Sender<GetPaneCwdResponse>,
    },
    GetPaneGitProbe {
        pane_id: PaneId,
        response_channel: crossbeam::channel::Sender<GetPaneGitProbeResponse>,
    },
    UpdateAndReportCwds,
    NotifyCwdFromOsc7(TerminalId, PathBuf),
    Exit,
}

impl From<&PtyInstruction> for PtyContext {
    fn from(pty_instruction: &PtyInstruction) -> Self {
        // The two enums are kept name-for-name; `instruction_context_tests` fails the test run if
        // a variant is added without either a matching `PtyContext` or an arm here.
        let name: &'static str = pty_instruction.into();
        PtyContext::from_str(name).unwrap_or_else(|_| {
            debug_assert!(false, "no PtyContext variant named {name}");
            PtyContext::Exit
        })
    }
}

pub(crate) struct Pty {
    pub active_panes: HashMap<ClientId, PaneId>,
    pub bus: Bus<PtyInstruction>,
    pub id_to_child_pid: HashMap<TerminalId, ProcessId>, // terminal_id => child pid
    originating_plugins: HashMap<TerminalId, OriginatingPlugin>,
    debug_to_file: bool,
    task_handles: HashMap<TerminalId, JoinHandle<()>>, // terminal_id to join-handle
    default_editor: Option<PathBuf>,
    post_command_discovery_hook: Option<String>,
    plugin_cwds: HashMap<PluginId, PathBuf>, // plugin_id -> cwd
    terminal_cwds: HashMap<TerminalId, PathBuf>, // terminal_id -> cwd
    pane_activity_flags: HashMap<TerminalId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    terminal_cmds: HashMap<TerminalId, Vec<String>>,
    terminal_foreground_cmds: HashMap<TerminalId, Vec<String>>,
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
    /// Per pane, the rerun off-thread now and the ones waiting behind it.
    reruns: HashMap<TerminalId, RerunState>,
    next_rerun_generation: u64,
}

pub(crate) fn pty_thread_main(mut pty: Pty, layout: Box<Layout>) -> Result<()> {
    pty.register_child_state_callback();
    pty.register_agent_report_callback();
    loop {
        let (event, mut err_ctx) = pty.bus.recv().expect("failed to receive event on channel");
        zellij_runtime::errors::add_call(&mut err_ctx, ContextType::Pty((&event).into()));
        match event {
            PtyInstruction::SpawnTerminal(
                terminal_action,
                name,
                new_pane_placement,
                start_suppressed,
                client_or_tab_index,
                completion_tx,
                set_blocking,
            ) => {
                let (run_command, pane_title, open_file_payload) = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => (
                        Some(run_command.clone()),
                        if name.is_some() {
                            // User explicitly provided a name — use it regardless
                            // of use_terminal_title
                            name
                        } else if run_command.is_remote_default_shell() {
                            // The host, until the remote shell sets a title
                            // of its own — which it will, exactly as a local
                            // shell does.
                            run_command.display_host()
                        } else if run_command.use_terminal_title {
                            None
                        } else {
                            Some(run_command.to_string())
                        },
                        None,
                    ),
                    Some(TerminalAction::OpenFile(open_file_payload)) => {
                        (None, name, Some(open_file_payload.clone()))
                    },
                    _ => (None, name, None),
                };
                let invoked_with = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => {
                        Some(Run::Command(run_command.clone()))
                    },
                    Some(TerminalAction::OpenFile(payload)) => Some(Run::EditFile(
                        payload.path.clone(),
                        payload.line_number,
                        payload.cwd.clone(),
                    )),
                    _ => None,
                };
                let pending = PendingTerminalSpawn {
                    run_command: run_command.clone(),
                    pane_title,
                    open_file_payload,
                    invoked_with,
                    new_pane_placement,
                    start_suppressed,
                    client_or_tab_index,
                    completion_tx,
                    set_blocking,
                };
                if run_command.as_ref().is_some_and(RunCommand::defers_spawn) {
                    let terminal_action =
                        pty.prepare_terminal_action(terminal_action, client_or_tab_index);
                    let terminal_id = pty.reserve_terminal_id()?;
                    finish_terminal_spawn(&mut pty, pending, Ok((terminal_id, false)))?;
                    pty.defer_spawn(SpawnJob::Terminal(PendingRemoteSpawn {
                        terminal_id,
                        terminal_action,
                    }))?;
                    continue;
                }
                let result = pty.spawn_terminal(terminal_action, client_or_tab_index);
                finish_terminal_spawn(&mut pty, pending, result)?;
            },
            PtyInstruction::SpawnResolved(resolved) => match resolved {
                PendingSpawn::Terminal(pending, spawned) => {
                    pty.handle_remote_spawned(pending, spawned)?;
                },
                PendingSpawn::Rerun(pending, spawned) => {
                    pty.handle_rerun_spawned(*pending, spawned)?;
                },
            },
            PtyInstruction::SpawnInPlaceTerminal(
                terminal_action,
                name,
                close_replaced_pane,
                client_id_tab_index_or_pane_id,
                completion_tx,
            ) => {
                let (run_command, pane_title) = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => (
                        Some(run_command.clone()),
                        if run_command.is_remote_default_shell() {
                            // See the sibling site above: the host stands in
                            // until the remote shell titles the pane itself.
                            Some(
                                name.or_else(|| run_command.display_host())
                                    .unwrap_or_else(|| run_command.to_string()),
                            )
                        } else if run_command.use_terminal_title {
                            None
                        } else {
                            Some(name.unwrap_or_else(|| run_command.to_string()))
                        },
                    ),
                    _ => (None, name),
                };
                let invoked_with = match &terminal_action {
                    Some(TerminalAction::RunCommand(run_command)) => {
                        Some(Run::Command(run_command.clone()))
                    },
                    Some(TerminalAction::OpenFile(payload)) => Some(Run::EditFile(
                        payload.path.clone(),
                        payload.line_number,
                        payload.cwd.clone(),
                    )),
                    _ => None,
                };
                let pending = PendingInPlaceSpawn {
                    run_command: run_command.clone(),
                    pane_title,
                    invoked_with,
                    close_replaced_pane,
                    client_or_tab_index: client_id_tab_index_or_pane_id,
                    completion_tx,
                };
                if run_command.as_ref().is_some_and(RunCommand::defers_spawn) {
                    let terminal_action = pty
                        .prepare_terminal_action(terminal_action, client_id_tab_index_or_pane_id);
                    let terminal_id = pty.reserve_terminal_id()?;
                    finish_in_place_spawn(&mut pty, pending, Ok((terminal_id, false)))?;
                    pty.defer_spawn(SpawnJob::Terminal(PendingRemoteSpawn {
                        terminal_id,
                        terminal_action,
                    }))?;
                    continue;
                }
                let result = pty.spawn_terminal(terminal_action, client_id_tab_index_or_pane_id);
                finish_in_place_spawn(&mut pty, pending, result)?;
            },
            PtyInstruction::OpenInPlaceEditor(
                temp_file,
                line_number,
                client_tab_index_or_pane_id,
                _completion_tx,
            ) => {
                let err_context = || "failed to open in-place editor for client".to_string();

                match pty.spawn_terminal(
                    Some(TerminalAction::OpenFile(OpenFilePayload::new(
                        temp_file,
                        line_number,
                        None,
                    ))),
                    client_tab_index_or_pane_id,
                ) {
                    Ok((pid, _starts_held)) => {
                        pty.bus
                            .senders
                            .send_to_screen(ScreenInstruction::OpenInPlaceEditor(
                                PaneId::Terminal(pid),
                                client_tab_index_or_pane_id,
                            ))
                            .with_context(err_context)?;
                    },
                    Err(e) => {
                        Err::<(), _>(e).with_context(err_context).non_fatal();
                    },
                }
            },
            PtyInstruction::UpdateActivePane(pane_id, client_id) => {
                pty.set_active_pane(pane_id, client_id);
            },
            PtyInstruction::GoToTab(tab_index, client_id) => {
                pty.bus
                    .senders
                    .send_to_screen(ScreenInstruction::GoToTab(tab_index, Some(client_id), None))
                    .with_context(|| {
                        format!("failed to move client {} to tab {}", client_id, tab_index)
                    })?;
            },
            PtyInstruction::NewTab(
                cwd,
                terminal_action,
                tab_layout,
                floating_panes_layout,
                tab_index,
                plugin_ids,
                initial_panes,
                block_on_first_terminal,
                should_change_focus_to_new_tab,
                client_id,
                completion_tx,
            ) => {
                let err_context = || "failed to open new tab";
                log::info!(
                    "PtyInstruction::NewTab: spawning terminals for tab {}",
                    tab_index
                );

                let floating_panes_layout = if floating_panes_layout.is_empty() {
                    layout.new_tab().1
                } else {
                    floating_panes_layout
                };
                pty.spawn_terminals_for_layout(
                    cwd,
                    tab_layout.unwrap_or_else(|| layout.new_tab().0),
                    floating_panes_layout,
                    terminal_action.clone(),
                    plugin_ids,
                    initial_panes,
                    tab_index,
                    block_on_first_terminal,
                    should_change_focus_to_new_tab,
                    client_id,
                    completion_tx,
                )
                .with_context(err_context)?;
            },
            PtyInstruction::OverrideLayout(
                cwd,
                default_shell,
                tab_layouts_with_plugin_ids,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
                completion_tx,
            ) => {
                let pending = PendingLayoutOverride {
                    cwd,
                    default_shell,
                    tabs: tab_layouts_with_plugin_ids,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    client_id,
                    completion_tx,
                };
                pty.finish_layout_override(pending)?;
            },
            PtyInstruction::ClosePane(id, _completion_tx) => {
                pty.close_pane(id)
                    .and_then(|_| {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::UnblockInputThread)
                    })
                    .with_context(|| format!("failed to close pane {:?}", id))?;
            },
            PtyInstruction::CloseTab(ids) => {
                pty.close_tab(ids)
                    .and_then(|_| {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::UnblockInputThread)
                    })
                    .context("failed to close tabs")?;
            },
            PtyInstruction::ReleaseTerminalForWarp {
                terminal_id,
                to_session,
                placement,
                client_id,
                deadline,
            } => {
                let released = release_pane_for_warp(&mut pty, terminal_id, &to_session, deadline);
                // Screen hears about this either way: it has a pane sitting in
                // limbo and has to either send it on or put it back.
                let _ = pty
                    .bus
                    .senders
                    .send_to_screen(ScreenInstruction::PaneReleasedForWarp {
                        terminal_id,
                        to_session,
                        placement,
                        client_id,
                        released,
                    });
            },
            PtyInstruction::AdoptTerminal {
                payload,
                placement,
                client_id,
                response_channel,
            } => {
                // Matched rather than mapped so the payload — which carries the
                // pane's whole scrollback — moves into the Screen instruction
                // instead of being cloned.
                let result = match pty.adopt_terminal(&payload) {
                    Ok(adopted) => {
                        let _ =
                            pty.bus
                                .senders
                                .send_to_screen(ScreenInstruction::AdoptedPaneReady {
                                    terminal_id: adopted.terminal_id,
                                    payload,
                                    placement,
                                    client_id,
                                    cmd: adopted.cmd,
                                    buffered: adopted.buffered,
                                    child_exit_status: adopted.child_exit_status,
                                });
                        Ok(())
                    },
                    Err(e) => Err(e),
                };
                let _ = response_channel.send(result);
            },
            PtyInstruction::ReleaseTerminalsForWarp {
                tab_index,
                terminal_ids,
                deadline,
            } => {
                let released = release_tab_for_warp(&mut pty, terminal_ids, deadline);
                // Screen hears about this either way: it has a tab full of panes
                // in limbo and has to either send them on or put them back.
                let _ = pty
                    .bus
                    .senders
                    .send_to_screen(ScreenInstruction::TabReleasedForWarp {
                        tab_index,
                        released,
                    });
            },
            PtyInstruction::StrandPaneAfterWarp {
                terminal_id,
                why,
                recovery,
            } => {
                strand_pane_after_warp(&pty, terminal_id, &why, recovery);
            },
            PtyInstruction::AdoptTab {
                payload,
                client_id,
                response_channel,
            } => {
                let result = match pty.adopt_tab(&payload) {
                    Ok(adopted) => {
                        let _ =
                            pty.bus
                                .senders
                                .send_to_screen(ScreenInstruction::AdoptedTabReady {
                                    payload,
                                    adopted: Box::new(adopted),
                                    client_id,
                                });
                        Ok(())
                    },
                    Err(e) => Err(e),
                };
                let _ = response_channel.send(result);
            },
            PtyInstruction::ClaimAdoptedTab {
                tab_index,
                terminals,
            } => {
                pty.adopted_tabs.insert(tab_index, terminals);
            },
            PtyInstruction::ReRunCommandInPane(pane_id, run_command, completion_tx) => {
                if let Some((run_command, _completion_tx)) =
                    pty.request_rerun(pane_id, run_command, completion_tx)?
                {
                    let result = pty.rerun_command_in_pane(pane_id, run_command.clone());
                    finish_command_rerun(&mut pty, pane_id, run_command, result)?;
                }
            },
            PtyInstruction::DropToShellInPane {
                pane_id,
                shell,
                working_dir,
                completion_tx: _completion_tx,
            } => {
                let err_context = || format!("failed to rerun command in pane {:?}", pane_id);

                // TODO: get configured default_shell from screen/tab as an option and default to
                // this otherwise (also look for a place that turns get_default_shell into a
                // RunCommand, we might have done this before)
                let run_command = RunCommand {
                    command: shell.unwrap_or_else(get_default_shell),
                    hold_on_close: false,
                    hold_on_start: false,
                    cwd: working_dir,
                    ..Default::default()
                };
                // Behind any rerun the pane already has in flight, like every
                // other rerun; a shell is local, so otherwise it runs here.
                let Some((run_command, _)) = pty.request_rerun(pane_id, run_command, None)? else {
                    continue;
                };
                match pty
                    .rerun_command_in_pane(pane_id, run_command.clone())
                    .with_context(err_context)
                {
                    Ok(..) => {},
                    Err(err) => match pane_start_failure(&err) {
                        Some((terminal_id, message)) => {
                            if run_command.hold_on_close {
                                send_pane_error_to_screen(
                                    pty.bus.senders.clone(),
                                    terminal_id,
                                    message,
                                    run_command,
                                )
                                .with_context(err_context)?;
                            }
                        },
                        None => Err::<(), _>(err).non_fatal(),
                    },
                }
            },
            PtyInstruction::DumpLayout(mut session_layout_metadata, client_id, completion_tx) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                match session_serialization::serialize_session_layout(
                    session_layout_metadata.into(),
                ) {
                    Ok((kdl_layout, _pane_contents)) => {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::Log(
                                vec![kdl_layout],
                                client_id,
                                completion_tx,
                            ))
                            .with_context(err_context)
                            .non_fatal();
                    },
                    Err(e) => {
                        pty.bus
                            .senders
                            .send_to_server(ServerInstruction::Log(
                                vec![e.to_owned()],
                                client_id,
                                completion_tx,
                            ))
                            .with_context(err_context)
                            .non_fatal();
                    },
                }
            },
            PtyInstruction::ListClientsMetadata(
                mut session_layout_metadata,
                client_id,
                completion_tx,
            ) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                pty.bus
                    .senders
                    .send_to_server(ServerInstruction::Log(
                        vec![format!(
                            "{}",
                            session_layout_metadata.list_clients_metadata(),
                        )],
                        client_id,
                        completion_tx,
                    ))
                    .with_context(err_context)
                    .non_fatal();
            },
            PtyInstruction::DumpLayoutToPlugin {
                mut session_layout_metadata,
                plugin_id,
                response_channel,
            } => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::DumpLayoutToPlugin {
                        session_layout_metadata,
                        plugin_id,
                        response_channel,
                    })
                    .with_context(err_context)
                    .non_fatal();
            },
            PtyInstruction::ListClientsToPlugin(
                mut session_layout_metadata,
                plugin_id,
                client_id,
            ) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                pty.bus
                    .senders
                    .send_to_plugin(PluginInstruction::ListClientsToPlugin(
                        session_layout_metadata,
                        plugin_id,
                        client_id,
                    ))
                    .with_context(err_context)
                    .non_fatal();
            },
            PtyInstruction::ReportPluginCwd(plugin_id, cwd) => {
                pty.plugin_cwds.insert(plugin_id, cwd);
            },
            PtyInstruction::LogLayoutToHd(mut session_layout_metadata) => {
                let err_context = || "Failed to dump layout".to_string();
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                if session_layout_metadata.is_dirty() {
                    match session_serialization::serialize_session_layout(
                        session_layout_metadata.into(),
                    ) {
                        Ok(kdl_layout_and_pane_contents) => {
                            pty.bus
                                .senders
                                .send_to_background_jobs(BackgroundJob::ReportLayoutInfo(
                                    kdl_layout_and_pane_contents,
                                ))
                                .with_context(err_context)?;
                        },
                        Err(e) => {
                            log::error!("Failed to log layout to HD: {}", e);
                        },
                    }
                }
            },
            PtyInstruction::SaveSessionToDisk {
                session_name,
                session_info,
                mut session_layout_metadata,
                completion_tx: _completion_tx, // Dropped at end to signal completion
            } => {
                pty.populate_session_layout_metadata(&mut session_layout_metadata);
                match session_serialization::serialize_session_layout(
                    session_layout_metadata.into(),
                ) {
                    Ok(kdl_and_files) => {
                        match write_session_state_to_disk(
                            session_name,
                            session_info,
                            kdl_and_files.clone(),
                        ) {
                            Ok(()) => {
                                let timestamp_millis = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64;
                                let _ = pty.bus.senders.send_to_plugin(
                                    PluginInstruction::UpdateSessionSaveTime(timestamp_millis),
                                );
                            },
                            Err(error) => Err::<(), _>(error).non_fatal(),
                        }

                        let _ = pty.bus.senders.send_to_background_jobs(
                            BackgroundJob::ReportLayoutInfo(kdl_and_files),
                        );
                    },
                    Err(e) => {
                        log::error!("Failed to serialize layout: {}", e);
                    },
                };
            },
            PtyInstruction::FillPluginCwd(
                should_float,
                should_be_open_in_place,
                close_replaced_pane,
                pane_title,
                run,
                tab_index,
                pane_id_to_replace,
                client_id,
                size,
                skip_cache,
                cwd,
                should_focus_plugin,
                floating_pane_coordinates,
                completion_tx,
                overlay_placement,
            ) => {
                pty.fill_plugin_cwd(
                    should_float,
                    should_be_open_in_place,
                    close_replaced_pane,
                    pane_title,
                    run,
                    tab_index,
                    pane_id_to_replace,
                    client_id,
                    size,
                    skip_cache,
                    cwd,
                    should_focus_plugin,
                    floating_pane_coordinates,
                    completion_tx,
                    overlay_placement,
                )?;
            },
            PtyInstruction::Reconfigure {
                default_editor,
                post_command_discovery_hook,
                client_id: _,
            } => {
                pty.reconfigure(default_editor, post_command_discovery_hook);
            },
            PtyInstruction::SendSigintToPaneId(pane_id) => {
                pty.send_sigint_to_pane(pane_id);
            },
            PtyInstruction::SendSigkillToPaneId(pane_id) => {
                pty.send_sigkill_to_pane(pane_id);
            },
            PtyInstruction::ResumeSuspendedPane(pane_id) => {
                pty.resume_suspended_pane(pane_id);
            },
            PtyInstruction::GetPanePid {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_pid(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPaneRunningCommand {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_running_command(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPaneCwd {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_cwd(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::GetPaneGitProbe {
                pane_id,
                response_channel,
            } => {
                let response = pty.get_pane_git_probe(pane_id);
                let _ = response_channel.send(response);
            },
            PtyInstruction::UpdateAndReportCwds => {
                pty.update_and_report_cwds();
            },
            PtyInstruction::NotifyCwdFromOsc7(terminal_id, path) => {
                pty.notify_cwd_from_osc7(terminal_id, path);
            },
            PtyInstruction::Exit => break,
        }
    }
    Ok(())
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
            post_command_discovery_hook,
            plugin_cwds: HashMap::new(),
            terminal_cwds: HashMap::new(),
            pane_activity_flags: HashMap::new(),
            terminal_cmds: HashMap::new(),
            terminal_foreground_cmds: HashMap::new(),
            adopted_tabs: HashMap::new(),
            spawn_workers: vec![],
            pending_remote_spawns: HashSet::new(),
            reruns: HashMap::new(),
            next_rerun_generation: 0,
        }
    }
    pub fn get_default_terminal(
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
    /// Which machine a pane's cwd would be a path on, if it is a terminal pane.
    ///
    /// `None` means this one. Used to keep a cwd from being inherited across a
    /// machine boundary, where it names a directory that need not exist.
    fn host_of_pane(&self, pane_id: &PaneId) -> Option<String> {
        match pane_id {
            PaneId::Terminal(id) => self
                .bus
                .os_input
                .as_ref()
                .and_then(|input| input.remote_pane_host(*id)),
            PaneId::Plugin(_) => None,
        }
    }
    /// Whether a cwd from `source` can be handed to `terminal_action`.
    ///
    /// Only if both run on the same machine. Inheriting a path across a machine
    /// boundary is inheriting a path that is not there — and for a pane that
    /// asked for a login shell, "not there" is exactly the wrong answer: with no
    /// cwd it lands in the remote home directory, which is where ssh puts you.
    fn cwd_may_be_inherited(&self, terminal_action: &TerminalAction, source: &PaneId) -> bool {
        let target_host = match terminal_action {
            TerminalAction::RunCommand(run_command) => run_command.display_host(),
            TerminalAction::OpenFile(_) => None,
        };
        target_host.as_deref() == self.host_of_pane(source).as_deref()
    }
    fn fill_cwd(&self, terminal_action: &mut TerminalAction, client_id: ClientId) {
        if let Some(active_pane) = self.active_panes.get(&client_id) {
            if !self.cwd_may_be_inherited(terminal_action, active_pane) {
                return;
            }
        }
        let cwd = match terminal_action {
            TerminalAction::RunCommand(run_command) => &mut run_command.cwd,
            TerminalAction::OpenFile(payload) => &mut payload.cwd,
        };
        if cwd.is_none() {
            *cwd = self
                .active_panes
                .get(&client_id)
                .and_then(|pane| match pane {
                    PaneId::Plugin(plugin_id) => self.plugin_cwds.get(plugin_id).cloned(),
                    PaneId::Terminal(id) => {
                        // Try to get CWD from OS, fall back to cached value
                        self.id_to_child_pid
                            .get(id)
                            .and_then(|&pid| {
                                self.bus
                                    .os_input
                                    .as_ref()
                                    .and_then(|input| input.get_cwd(pid))
                            })
                            .or_else(|| self.terminal_cwds.get(id).cloned())
                    },
                })
        };
    }
    fn fill_cwd_from_pane_id(&self, terminal_action: &mut TerminalAction, pane_id: &PaneId) {
        if !self.cwd_may_be_inherited(terminal_action, pane_id) {
            return;
        }
        let cwd = match terminal_action {
            TerminalAction::RunCommand(run_command) => &mut run_command.cwd,
            TerminalAction::OpenFile(payload) => &mut payload.cwd,
        };
        if cwd.is_none() {
            *cwd = match pane_id {
                PaneId::Terminal(terminal_pane_id) => {
                    // Try to get CWD from OS, fall back to cached value
                    self.id_to_child_pid
                        .get(terminal_pane_id)
                        .and_then(|&pid| {
                            self.bus
                                .os_input
                                .as_ref()
                                .and_then(|input| input.get_cwd(pid))
                        })
                        .or_else(|| self.terminal_cwds.get(terminal_pane_id).cloned())
                },
                PaneId::Plugin(plugin_id) => self.plugin_cwds.get(plugin_id).cloned(),
            };
        };
    }
    pub fn spawn_terminal(
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
            ClientTabIndexOrPaneId::TabIndex(_) | ClientTabIndexOrPaneId::TabIndexNoFocus(_) => {
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

    /// Hand a spawn to a worker thread. Resolving where a command runs (a VM to
    /// boot, an ssh host to reach) and starting it there can take minutes, and
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
                        if command.target.is_firecracker() {
                            "waiting for the Firecracker provider"
                        } else {
                            "connecting to the remote host"
                        },
                    )?;
                }
            },
            SpawnJob::Rerun(pending) => {
                if let PaneId::Terminal(terminal_id) = pending.pane_id {
                    send_remote_status(
                        &senders,
                        terminal_id,
                        &pending.run_command,
                        if pending.run_command.target.is_firecracker() {
                            "waiting for the Firecracker provider"
                        } else {
                            "connecting to the remote host"
                        },
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
        let PaneId::Terminal(terminal_id) = pane_id else {
            return Ok(Some((run_command, completion_tx)));
        };
        if let Some(state) = self.reruns.get_mut(&terminal_id) {
            state.queued.push_back((run_command, completion_tx));
            return Ok(None);
        }
        if !run_command.defers_spawn() {
            return Ok(Some((run_command, completion_tx)));
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
                        .send_to_plugin(PluginInstruction::Update(vec![(
                            Some(originating_plugin.plugin_id),
                            Some(originating_plugin.client_id),
                            Event::CommandPaneExited(
                                terminal_id,
                                Some(2),
                                originating_plugin.context.clone(),
                            ),
                        )]))?;
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

    fn discard_deferred_spawns(&mut self, pending: &[PendingRemoteSpawn]) {
        for spawn in pending {
            self.close_pane(PaneId::Terminal(spawn.terminal_id))
                .with_context(|| {
                    format!(
                        "failed to close reserved pane {} of a failed layout",
                        spawn.terminal_id
                    )
                })
                .non_fatal();
        }
    }

    /// Release the panes a layout spawned but never handed to Screen.
    ///
    /// Screen only learns of a layout's panes through `ApplyLayout` or
    /// `OverrideLayoutComplete`. A layout that fails before one of those is sent
    /// still owns every child it started, and no `ClosePane` will ever arrive for
    /// them, so they must be closed here or they outlive the layout that asked
    /// for them.
    fn discard_layout_panes(&mut self, terminal_ids: impl IntoIterator<Item = TerminalId>) {
        for terminal_id in terminal_ids {
            self.close_pane(PaneId::Terminal(terminal_id))
                .with_context(|| format!("failed to close pane {terminal_id} of a failed layout"))
                .non_fatal();
        }
    }

    /// Give up on a layout: close every pane it spawned, deferred or already
    /// running, so a failure part-way through leaves nothing behind.
    fn abandon_layout(
        &mut self,
        tiled: &[SpawnedPane],
        floating: &[SpawnedPane],
        deferred: &[PendingRemoteSpawn],
    ) {
        // A deferred pane holds a slot in both lists - close it once, through the
        // deferred path, which names it in its own error context.
        let deferred_ids: HashSet<TerminalId> =
            deferred.iter().map(|spawn| spawn.terminal_id).collect();
        let spawned: Vec<TerminalId> = tiled
            .iter()
            .chain(floating)
            .map(|(terminal_id, ..)| *terminal_id)
            .filter(|terminal_id| !deferred_ids.contains(terminal_id))
            .collect();
        self.discard_layout_panes(spawned);
        self.discard_deferred_spawns(deferred);
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
    pub fn spawn_terminals_for_layout(
        &mut self,
        cwd: Option<PathBuf>,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        default_shell: Option<TerminalAction>,
        plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        initial_panes: Option<Vec<CommandOrPlugin>>,
        tab_index: TabId,
        block_on_first_terminal: bool,
        should_change_focus_to_new_tab: bool,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let mut default_shell =
            default_shell.unwrap_or_else(|| self.get_default_terminal(cwd, None));
        self.fill_cwd(&mut default_shell, client_id);

        // Match initial_panes commands to empty slots in the layout
        let mut layout = layout;
        if let Some(ref initial_panes_vec) = initial_panes {
            for initial_pane in initial_panes_vec.iter() {
                if let CommandOrPlugin::Command(run_command_action) = initial_pane {
                    let run_command: RunCommand = run_command_action.clone().into();
                    if !layout.replace_next_empty_slot_with_run(Run::Command(run_command)) {
                        log::warn!("More initial_panes provided than empty slots available");
                        break;
                    }
                } else if let CommandOrPlugin::File(file_to_open) = initial_pane {
                    if !layout.replace_next_empty_slot_with_run(Run::EditFile(
                        file_to_open.path.clone(),
                        file_to_open.line_number,
                        file_to_open.cwd.clone(),
                    )) {
                        log::warn!("More initial_panes provided than empty slots available");
                        break;
                    }
                }
                // Skip CommandOrPlugin::Plugin entries (already handled by plugin thread)
            }
        }

        // A tab warped in from another session brings its panes with it. Their
        // supervisors are already attached and already have reader tasks, so
        // there is nothing here to spawn: an empty instruction list is what says
        // so, and the ids come from the queue instead, further down.
        //
        // Nothing else about the layout changes. Its plugin nodes were resolved
        // into `plugin_ids` by the plugin thread on the way here, exactly as they
        // are for any other new tab — which is how a warped tab gets its plugin
        // panes back.
        let adopted_panes = self.adopted_tabs.remove(&tab_index);

        let extracted_run_instructions = match &adopted_panes {
            Some(_) => vec![],
            None => layout.extract_run_instructions(),
        };
        let extracted_floating_run_instructions: Vec<Option<Run>> = match &adopted_panes {
            Some(_) => vec![],
            None => floating_panes_layout
                .iter()
                .filter(|f| !f.already_running)
                .map(|f| f.run.clone())
                .collect(),
        };
        let pending = PendingLayoutSpawn {
            layout,
            floating_panes_layout,
            default_shell,
            plugin_ids,
            adopted_panes,
            run_instructions: extracted_run_instructions,
            floating_run_instructions: extracted_floating_run_instructions,
            tab_index,
            block_on_first_terminal,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        };
        self.finish_layout_spawn(pending)
    }

    fn finish_layout_spawn(&mut self, pending: PendingLayoutSpawn) -> Result<()> {
        let err_context = || "failed to spawn terminals for layout for".to_string();
        let PendingLayoutSpawn {
            layout,
            floating_panes_layout,
            default_shell,
            plugin_ids,
            adopted_panes,
            run_instructions: extracted_run_instructions,
            floating_run_instructions: extracted_floating_run_instructions,
            tab_index,
            block_on_first_terminal,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        } = pending;
        let mut new_pane_pids: Vec<(
            TerminalId,
            bool,
            Option<RunCommand>,
            Option<Result<Box<dyn AsyncReader>>>,
        )> = vec![];

        let mut new_floating_panes_pids: Vec<(
            TerminalId,
            bool,
            Option<RunCommand>,
            Option<Result<Box<dyn AsyncReader>>>,
        )> = vec![];

        let mut originating_plugins_to_inform = vec![];
        let mut deferred_spawns = vec![];

        for run_instruction in extracted_run_instructions {
            let originating_plugin = run_instruction.as_ref().and_then(|r| {
                if let Run::Command(run_command) = r {
                    run_command.originating_plugin.clone()
                } else {
                    None
                }
            });
            let mut terminal_id = None;
            let applied = match self.apply_run_instruction(
                run_instruction,
                default_shell.clone(),
                &mut deferred_spawns,
            ) {
                Ok(applied) => applied,
                Err(error) => {
                    self.abandon_layout(&new_pane_pids, &new_floating_panes_pids, &deferred_spawns);
                    return Err(error);
                },
            };
            if let Some(new_pane_data) = applied {
                terminal_id = Some(new_pane_data.0);
                new_pane_pids.push(new_pane_data);
            }
            if let (Some(originating_plugin), Some(terminal_id)) = (originating_plugin, terminal_id)
            {
                originating_plugins_to_inform.push((terminal_id, originating_plugin));
            }
        }
        for run_instruction in extracted_floating_run_instructions {
            let originating_plugin = run_instruction.as_ref().and_then(|r| {
                if let Run::Command(run_command) = r {
                    run_command.originating_plugin.clone()
                } else {
                    None
                }
            });
            let mut terminal_id = None;
            let applied = match self.apply_run_instruction(
                run_instruction,
                default_shell.clone(),
                &mut deferred_spawns,
            ) {
                Ok(applied) => applied,
                Err(error) => {
                    self.abandon_layout(&new_pane_pids, &new_floating_panes_pids, &deferred_spawns);
                    return Err(error);
                },
            };
            if let Some(new_pane_data) = applied {
                terminal_id = Some(new_pane_data.0);
                new_floating_panes_pids.push(new_pane_data);
            }
            if let (Some(originating_plugin), Some(terminal_id)) = (originating_plugin, terminal_id)
            {
                originating_plugins_to_inform.push((terminal_id, originating_plugin));
            }
        }

        // Option<RunCommand> should only be Some if the pane starts held
        let spawned_ids = |pids: &Vec<(
            TerminalId,
            bool,
            Option<RunCommand>,
            Option<Result<Box<dyn AsyncReader>>>,
        )>| {
            pids.iter()
                .map(|(terminal_id, starts_held, run_command, _)| {
                    if *starts_held {
                        (*terminal_id, run_command.clone())
                    } else {
                        (*terminal_id, None)
                    }
                })
                .collect::<Vec<_>>()
        };
        let adopted_ids = |terminals: &[AdoptedTabTerminal]| {
            terminals
                .iter()
                .map(|terminal| (terminal.terminal_id, terminal.hold_for_command()))
                .collect::<Vec<_>>()
        };
        let (new_tab_pane_ids, new_tab_floating_pane_ids) = match &adopted_panes {
            Some((tiled, floating)) => (adopted_ids(tiled), adopted_ids(floating)),
            None => (
                spawned_ids(&new_pane_pids),
                spawned_ids(&new_floating_panes_pids),
            ),
        };
        let adopted_agents = match &adopted_panes {
            Some((tiled, floating)) => tiled
                .iter()
                .chain(floating)
                .filter_map(|terminal| {
                    terminal
                        .agent
                        .clone()
                        .map(|agent| (terminal.terminal_id, agent))
                })
                .collect(),
            None => vec![],
        };

        // Track the first terminal_id if blocking is requested
        let first_initial_pane_terminal_id = if block_on_first_terminal && !new_pane_pids.is_empty()
        {
            Some(new_pane_pids[0].0)
        } else {
            None
        };

        // Prepare blocking_terminal for ApplyLayout
        let (direct_completion_tx, blocking_terminal) =
            if let Some(terminal_id) = first_initial_pane_terminal_id {
                (None, completion_tx.map(|tx| (terminal_id, tx)))
            } else {
                (completion_tx, None)
            };

        log::info!(
            "spawn_terminals_for_layout: {} tiled + {} floating panes created, sending ApplyLayout",
            new_tab_pane_ids.len(),
            new_tab_floating_pane_ids.len()
        );
        if let Err(error) = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::ApplyLayout(
                layout,
                floating_panes_layout,
                new_tab_pane_ids.clone(),
                new_tab_floating_pane_ids.clone(),
                adopted_agents,
                plugin_ids,
                tab_index,
                should_change_focus_to_new_tab,
                client_id,
                direct_completion_tx,
                blocking_terminal,
            ))
        {
            self.abandon_layout(&new_pane_pids, &new_floating_panes_pids, &deferred_spawns);
            return Err(error).with_context(err_context);
        }

        // Behind the `ApplyLayout` on the same channel, so the panes exist by
        // the time these land. An adopted pane whose child is already gone is
        // then held, showing the status it exited with.
        if let Some((tiled, floating)) = adopted_panes {
            for terminal in tiled.into_iter().chain(floating) {
                if !terminal.replay.is_empty() {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::PtyBytes(
                            terminal.terminal_id,
                            terminal.replay,
                        ))
                        .with_context(err_context)?;
                }
                if let Some(exit_status) = terminal.child_exit_status {
                    self.bus
                        .senders
                        .send_to_screen(ScreenInstruction::HoldPane(
                            PaneId::Terminal(terminal.terminal_id),
                            exit_status,
                            terminal.cmd,
                        ))
                        .with_context(err_context)?;
                }
            }
        }

        let mut terminals_to_start = vec![];

        terminals_to_start.append(&mut new_pane_pids);
        terminals_to_start.append(&mut new_floating_panes_pids);
        for (terminal_id, starts_held, run_command, reader_result) in terminals_to_start {
            if starts_held {
                // we do not run a command or start listening for bytes on held panes
                continue;
            }
            let Some(reader_result) = reader_result else {
                continue;
            };
            match reader_result {
                Ok(reader) => {
                    let activity_flag =
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let terminal_bytes = async_runtime().spawn({
                        let senders = self.bus.senders.clone();
                        let debug_to_file = self.debug_to_file;
                        let activity_flag = activity_flag.clone();
                        async move {
                            TerminalBytes::new(
                                terminal_id,
                                reader,
                                senders,
                                debug_to_file,
                                activity_flag,
                            )
                            .listen()
                            .await
                            .context("failed to spawn terminals for layout")
                            .fatal();
                        }
                    });
                    self.task_handles.insert(terminal_id, terminal_bytes);
                    self.pane_activity_flags.insert(terminal_id, activity_flag);
                    self.report_terminal_host(terminal_id);
                },
                Err(err) => match run_command {
                    Some(run_command) => {
                        if run_command.shows_start_failure() {
                            let message = pane_start_failure(&err)
                                .map(|(_, message)| message)
                                .unwrap_or_else(|| format!("{err:#}"));
                            send_pane_error_to_screen(
                                self.bus.senders.clone(),
                                terminal_id,
                                message,
                                run_command.clone(),
                            )
                            .with_context(err_context)?;
                        } else {
                            self.close_pane(PaneId::Terminal(terminal_id))
                                .with_context(err_context)?;
                        }
                    },
                    None => {
                        self.close_pane(PaneId::Terminal(terminal_id))
                            .with_context(err_context)?;
                    },
                },
            }
        }
        for (terminal_id, originating_plugin) in originating_plugins_to_inform {
            self.inform_originating_plugin_of_open(terminal_id, originating_plugin);
        }
        for deferred in deferred_spawns {
            self.defer_spawn(SpawnJob::Terminal(deferred))?;
        }
        Ok(())
    }
    fn finish_layout_override(&mut self, pending: PendingLayoutOverride) -> Result<()> {
        let err_context = || "failed to override layout";
        let mut all_tab_results = vec![];
        let mut deferred_spawns = vec![];
        for (tab, plugin_ids) in pending.tabs {
            let tab_index = tab.tab_index;
            match self.spawn_terminals_for_layout_override(
                pending.cwd.clone(),
                tab.tiled_layout,
                tab.floating_layouts,
                pending.default_shell.clone(),
                plugin_ids,
                tab_index,
                tab.tab_name,
                pending.client_id,
                tab.swap_tiled_layouts,
                tab.swap_floating_layouts,
                pending.retain_existing_terminal_panes,
                pending.retain_existing_plugin_panes,
            ) {
                Ok((tab_result, mut tab_deferred_spawns)) => {
                    all_tab_results.push(tab_result);
                    deferred_spawns.append(&mut tab_deferred_spawns);
                },
                Err(error) => Err::<(), _>(error)
                    .with_context(|| format!("failed to spawn terminals for tab {tab_index}"))
                    .non_fatal(),
            }
        }
        // The results carry the panes away with them, so note the ids while they
        // are still here: if the send fails, Screen never hears of these panes and
        // this is the last chance to close them.
        let spawned_ids: Vec<TerminalId> = all_tab_results
            .iter()
            .flat_map(|tab_result| {
                tab_result
                    .new_terminal_pids
                    .iter()
                    .chain(&tab_result.new_floating_pane_pids)
            })
            .map(|(terminal_id, _)| *terminal_id)
            .collect();
        if let Err(error) =
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::OverrideLayoutComplete(
                    all_tab_results,
                    pending.retain_existing_terminal_panes,
                    pending.retain_existing_plugin_panes,
                    pending.client_id,
                    pending.completion_tx,
                ))
        {
            let deferred_ids: HashSet<TerminalId> = deferred_spawns
                .iter()
                .map(|spawn| spawn.terminal_id)
                .collect();
            self.discard_layout_panes(
                spawned_ids
                    .into_iter()
                    .filter(|terminal_id| !deferred_ids.contains(terminal_id)),
            );
            self.discard_deferred_spawns(&deferred_spawns);
            return Err(error).with_context(err_context);
        }
        for deferred in deferred_spawns {
            self.defer_spawn(SpawnJob::Terminal(deferred))?;
        }
        Ok(())
    }

    pub fn spawn_terminals_for_layout_override(
        &mut self,
        cwd: Option<PathBuf>,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        default_shell: Option<TerminalAction>,
        plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        tab_index: TabId,
        tab_name: Option<String>,
        client_id: ClientId,
        swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
        swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
        _retain_existing_terminal_panes: bool,
        _retain_existing_plugin_panes: bool,
    ) -> Result<(TabOverrideResult, Vec<PendingRemoteSpawn>)> {
        let err_context = || "failed to spawn terminals for layout for".to_string();

        let mut default_shell =
            default_shell.unwrap_or_else(|| self.get_default_terminal(cwd, None));
        self.fill_cwd(&mut default_shell, client_id);

        let extracted_run_instructions = layout.extract_run_instructions();
        let extracted_floating_run_instructions = floating_panes_layout
            .iter()
            .filter(|f| !f.already_running)
            .map(|f| f.run.clone());
        let mut new_pane_pids: Vec<(
            TerminalId,
            bool,
            Option<RunCommand>,
            Option<Result<Box<dyn AsyncReader>>>,
        )> = vec![];

        let mut new_floating_panes_pids: Vec<(
            TerminalId,
            bool,
            Option<RunCommand>,
            Option<Result<Box<dyn AsyncReader>>>,
        )> = vec![];

        let mut originating_plugins_to_inform = vec![];
        let mut deferred_spawns = vec![];

        for run_instruction in extracted_run_instructions {
            let originating_plugin = run_instruction.as_ref().and_then(|r| {
                if let Run::Command(run_command) = r {
                    run_command.originating_plugin.clone()
                } else {
                    None
                }
            });
            let mut terminal_id = None;
            let applied = match self.apply_run_instruction(
                run_instruction,
                default_shell.clone(),
                &mut deferred_spawns,
            ) {
                Ok(applied) => applied,
                Err(error) => {
                    self.abandon_layout(&new_pane_pids, &new_floating_panes_pids, &deferred_spawns);
                    return Err(error);
                },
            };
            if let Some(new_pane_data) = applied {
                terminal_id = Some(new_pane_data.0);
                new_pane_pids.push(new_pane_data);
            }
            if let (Some(originating_plugin), Some(terminal_id)) = (originating_plugin, terminal_id)
            {
                originating_plugins_to_inform.push((terminal_id, originating_plugin));
            }
        }
        for run_instruction in extracted_floating_run_instructions {
            let originating_plugin = run_instruction.as_ref().and_then(|r| {
                if let Run::Command(run_command) = r {
                    run_command.originating_plugin.clone()
                } else {
                    None
                }
            });
            let mut terminal_id = None;
            let applied = match self.apply_run_instruction(
                run_instruction,
                default_shell.clone(),
                &mut deferred_spawns,
            ) {
                Ok(applied) => applied,
                Err(error) => {
                    self.abandon_layout(&new_pane_pids, &new_floating_panes_pids, &deferred_spawns);
                    return Err(error);
                },
            };
            if let Some(new_pane_data) = applied {
                terminal_id = Some(new_pane_data.0);
                new_floating_panes_pids.push(new_pane_data);
            }
            if let (Some(originating_plugin), Some(terminal_id)) = (originating_plugin, terminal_id)
            {
                originating_plugins_to_inform.push((terminal_id, originating_plugin));
            }
        }

        // Option<RunCommand> should only be Some if the pane starts held
        let new_tab_pane_ids: Vec<(TerminalId, Option<RunCommand>)> = new_pane_pids
            .iter()
            .map(|(terminal_id, starts_held, run_command, _)| {
                if *starts_held {
                    (*terminal_id, run_command.clone())
                } else {
                    (*terminal_id, None)
                }
            })
            .collect();
        let new_tab_floating_pane_ids: Vec<(TerminalId, Option<RunCommand>)> =
            new_floating_panes_pids
                .iter()
                .map(|(terminal_id, starts_held, run_command, _)| {
                    if *starts_held {
                        (*terminal_id, run_command.clone())
                    } else {
                        (*terminal_id, None)
                    }
                })
                .collect();

        let tab_result = TabOverrideResult {
            tab_index,
            tab_name,
            tiled_layout: layout,
            floating_layouts: floating_panes_layout,
            swap_tiled_layouts,
            swap_floating_layouts,
            new_terminal_pids: new_tab_pane_ids,
            new_floating_pane_pids: new_tab_floating_pane_ids,
            plugin_ids,
        };

        let mut terminals_to_start = vec![];

        terminals_to_start.append(&mut new_pane_pids);
        terminals_to_start.append(&mut new_floating_panes_pids);
        for (terminal_id, starts_held, run_command, reader_result) in terminals_to_start {
            if starts_held {
                // we do not run a command or start listening for bytes on held panes
                continue;
            }
            let Some(reader_result) = reader_result else {
                continue;
            };
            match reader_result {
                Ok(reader) => {
                    let activity_flag =
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let terminal_bytes = async_runtime().spawn({
                        let senders = self.bus.senders.clone();
                        let debug_to_file = self.debug_to_file;
                        let activity_flag = activity_flag.clone();
                        async move {
                            TerminalBytes::new(
                                terminal_id,
                                reader,
                                senders,
                                debug_to_file,
                                activity_flag,
                            )
                            .listen()
                            .await
                            .context("failed to spawn terminals for layout")
                            .fatal();
                        }
                    });
                    self.task_handles.insert(terminal_id, terminal_bytes);
                    self.pane_activity_flags.insert(terminal_id, activity_flag);
                    self.report_terminal_host(terminal_id);
                },
                Err(err) => match run_command {
                    Some(run_command) => {
                        if run_command.shows_start_failure() {
                            let message = pane_start_failure(&err)
                                .map(|(_, message)| message)
                                .unwrap_or_else(|| format!("{err:#}"));
                            send_pane_error_to_screen(
                                self.bus.senders.clone(),
                                terminal_id,
                                message,
                                run_command.clone(),
                            )
                            .with_context(err_context)?;
                        } else {
                            self.close_pane(PaneId::Terminal(terminal_id))
                                .with_context(err_context)?;
                        }
                    },
                    None => {
                        self.close_pane(PaneId::Terminal(terminal_id))
                            .with_context(err_context)?;
                    },
                },
            }
        }
        for (terminal_id, originating_plugin) in originating_plugins_to_inform {
            self.inform_originating_plugin_of_open(terminal_id, originating_plugin);
        }
        Ok((tab_result, deferred_spawns))
    }
    fn inform_originating_plugin_of_open(
        &mut self,
        terminal_id: TerminalId,
        originating_plugin: OriginatingPlugin,
    ) {
        self.originating_plugins
            .insert(terminal_id, originating_plugin.clone());
        let update_event = Event::CommandPaneOpened(terminal_id, originating_plugin.context);
        let _ = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                Some(originating_plugin.plugin_id),
                Some(originating_plugin.client_id),
                update_event,
            )]));
    }

    fn apply_run_instruction(
        &mut self,
        run_instruction: Option<Run>,
        default_shell: TerminalAction,
        deferred_spawns: &mut Vec<PendingRemoteSpawn>,
    ) -> Result<
        Option<(
            TerminalId,
            bool,
            Option<RunCommand>,
            Option<Result<Box<dyn AsyncReader>>>,
        )>,
    > {
        // terminal_id,
        // starts_held,
        // command
        // successfully opened
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
                if command.cwd.is_none() && command.target.is_local() {
                    if let TerminalAction::RunCommand(cmd) = default_shell {
                        command.cwd = cmd.cwd;
                    }
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
    /// Deliver a signal to a pane's child, wherever that child lives.
    ///
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

    pub fn close_pane(&mut self, id: PaneId) -> Result<()> {
        let err_context = || format!("failed to close for pane {id:?}");
        match id {
            PaneId::Terminal(id) => {
                self.pending_remote_spawns.remove(&id);
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
                self.pane_activity_flags.remove(&id);
                self.forget_terminal_cwd(id);
                self.forget_terminal_host(id);
                self.terminal_cmds.remove(&id);
                self.terminal_foreground_cmds.remove(&id);
                self.bus
                    .os_input
                    .as_ref()
                    .context("no OS I/O interface found")
                    .and_then(|os_input| os_input.clear_terminal_id(id))
                    .with_context(err_context)
                    .non_fatal();
            },
            PaneId::Plugin(pid) => drop(
                self.bus
                    .senders
                    .send_to_plugin(PluginInstruction::Unload(pid)),
            ),
        }
        Ok(())
    }
    pub fn close_tab(&mut self, ids: Vec<PaneId>) -> Result<()> {
        for id in ids {
            self.close_pane(id)
                .with_context(|| format!("failed to close tab for pane {id:?}"))?;
        }
        Ok(())
    }

    /// Whether this pane could be released for a warp, without releasing it.
    ///
    /// Separate from the release so a whole tab can be asked before any of it is
    /// let go — the panes ahead of a refusal would otherwise already be gone by
    /// the time it arrived.
    pub fn check_terminal_can_be_warped(&self, terminal_id: TerminalId) -> Result<()> {
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
    /// reader task drops its `RawFdAsyncReader`, which closes this server's dup of
    /// the master — the supervisor keeps its own and starts draining into the ring.
    ///
    /// The reader is stopped *before* the release, not after, and for a remote
    /// pane that ordering is load-bearing rather than tidy. The broker replays
    /// from the byte count this side reports, and the pane's screen is
    /// serialized separately by Screen; a byte has to land in exactly one of the
    /// two. With the reader still running, the count sampled inside the release
    /// could grow between being read and the screen being dumped, and the
    /// overlap would be replayed on top of itself.
    ///
    /// It costs the local path nothing. There, aborting the task drops the
    /// `RawFdAsyncReader` and closes this server's dup of the master, which the
    /// supervisor has been waiting for either way.
    ///
    /// That drop is also why [`Self::check_terminal_can_be_warped`] runs first
    /// and is a separate question rather than something the release discovers as
    /// it goes: once the abort has happened there is no dup left to reopen, so a
    /// refusal raised after it would leave a pane on screen with no process on
    /// the other end. Everything knowable is therefore knowable *before*.
    pub fn release_terminal_for_warp(
        &mut self,
        terminal_id: TerminalId,
        deadline: Instant,
    ) -> Result<ReleasedPane> {
        let err_context = || format!("failed to release pane {terminal_id} for a warp");
        // Asked before the reader is touched, which is the whole point of it
        // being a separate question: everything the release below can refuse for
        // is knowable now, and refusing now leaves the pane exactly as it was.
        // Once the abort has run there is no way back — the `RawFdAsyncReader`
        // drops with it and takes this server's dup of the master with it — so a
        // refusal from that side would cost the user a working pane.
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
    fn stop_readers_for_warp(&mut self, terminal_ids: &[TerminalId]) {
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
        self.terminal_foreground_cmds.remove(&terminal_id);
        self.forget_terminal_host(terminal_id);
        // The plugin that spawned this pane lives in *this* session, so it does
        // not follow the pane out.
        self.originating_plugins.remove(&terminal_id);
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
    pub fn adopt_terminal(&mut self, pane: &WarpedPane) -> Result<AdoptedPane> {
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

        // Built before the attach, because `adopt_terminal` hands it to the
        // reader thread. `hold_on_close` is the only thing that varies, and it
        // comes from the supervisor — which we have not asked yet — so this
        // closure reads it from a cell the attach fills in. Simpler: attach
        // first is impossible, so the callback consults the command it is given.
        let senders = self.bus.senders.clone();
        let quit_cb = Box::new(move |pane_id, exit_status, command: RunCommand| {
            // Deliberately no `CommandPaneExited`/`EditPaneExited`: an
            // originating plugin belongs to the session this pane came from, and
            // its plugin id would name something else entirely here. The
            // supervisor clears it from the command for the same reason.
            if command.hold_on_close {
                let _ = senders.send_to_screen(ScreenInstruction::HoldPane(
                    pane_id,
                    exit_status,
                    command,
                ));
            } else {
                let _ = senders.send_to_screen(ScreenInstruction::ClosePane(
                    pane_id,
                    None,
                    None,
                    exit_status,
                ));
            }
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
    pub fn adopt_tab(&mut self, payload: &WarpedTab) -> Result<AdoptedTabPanes> {
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
                .map(|pane| pane.host_id.clone())
                .collect();
            for adopted in tiled.iter().chain(floating.iter()) {
                if let Err(e) = self.release_terminal_for_warp(
                    adopted.terminal_id,
                    Instant::now() + GIVE_BACK_BUDGET,
                ) {
                    // Nothing left to try. Say which supervisors are now stranded
                    // so they can be recovered by hand.
                    log::error!(
                        "could not give back an adopted pane after a failed tab warp: {e:#}"
                    );
                }
            }
            return Err(e)
                .with_context(|| {
                    format!(
                    "could not take the whole tab, so none of it was taken — its panes are still \
                     running and can be recovered with `zellij pty-host adopt <id>`: {}",
                    host_ids.join(", ")
                )
                })
                .with_context(|| {
                    format!("the pane supervised by {host_id} could not be attached")
                });
        }

        Ok(AdoptedTabPanes { tiled, floating })
    }

    pub fn set_active_pane(&mut self, pane_id: Option<PaneId>, client_id: ClientId) {
        if let Some(pane_id) = pane_id {
            self.active_panes.insert(client_id, pane_id);
        }
    }
    pub fn rerun_command_in_pane(
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
                .send_to_plugin(PluginInstruction::Update(vec![(
                    Some(plugin.plugin_id),
                    Some(plugin.client_id),
                    Event::CommandPaneReRun(id, plugin.context.clone()),
                )]))?;
        }
        Ok(())
    }
    pub fn populate_session_layout_metadata(
        &mut self,
        session_layout_metadata: &mut SessionLayoutMetadata,
    ) {
        let terminal_ids = session_layout_metadata.all_terminal_ids();
        let mut terminal_ids_to_commands: HashMap<TerminalId, Vec<String>> = HashMap::new();
        let mut terminal_ids_to_cwds: HashMap<TerminalId, PathBuf> = HashMap::new();

        let pids: Vec<_> = terminal_ids
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(&id))
            .copied()
            .collect();
        let (pids_to_cwds, pids_to_cmds) = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_cwds(pids))
            .unwrap_or_default();
        let panes: Vec<(TerminalId, ProcessId)> = terminal_ids
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id).map(|pid| (*id, *pid)))
            .collect();
        let foreground_cmds = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_foreground_cmds(&panes, &self.post_command_discovery_hook))
            .unwrap_or_default();

        for terminal_id in terminal_ids {
            let process_id = self.id_to_child_pid.get(&terminal_id);
            let cwd = process_id.and_then(|pid| pids_to_cwds.get(pid));
            let cmd_sysinfo = process_id.and_then(|pid| pids_to_cmds.get(pid));
            let cmd_foreground = foreground_cmds.get(&terminal_id);
            if let Some(cmd) = cmd_foreground {
                terminal_ids_to_commands.insert(terminal_id, cmd.clone());
            } else if let Some(cmd) = cmd_sysinfo {
                terminal_ids_to_commands.insert(terminal_id, cmd.clone());
            }
            if let Some(cwd) = cwd {
                terminal_ids_to_cwds.insert(terminal_id, cwd.clone());
            }
        }
        session_layout_metadata.update_default_shell(get_default_shell());
        session_layout_metadata.update_terminal_commands(terminal_ids_to_commands);
        session_layout_metadata.update_terminal_cwds(terminal_ids_to_cwds);
        session_layout_metadata.update_default_editor(&self.default_editor);
        session_layout_metadata.detect_editor_panes();
    }
    pub fn fill_plugin_cwd(
        &self,
        should_float: Option<bool>,
        should_open_in_place: bool, // should be opened in place
        close_replaced_pane: bool,  // close_replaced_pane
        pane_title: Option<String>, // pane title
        mut run: RunPluginOrAlias,
        tab_index: TabId,                   // tab index
        pane_id_to_replace: Option<PaneId>, // pane id to replace if this is to be opened "in-place"
        client_id: ClientId,
        size: Size,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        should_focus_plugin: Option<bool>,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        completion_tx: Option<NotificationEnd>,
        overlay_placement: Option<OverlayPlacement>,
    ) -> Result<()> {
        let get_focused_cwd = || {
            self.active_panes
                .get(&client_id)
                .and_then(|pane| match pane {
                    PaneId::Plugin(plugin_id) => self.plugin_cwds.get(plugin_id).cloned(),
                    PaneId::Terminal(id) => {
                        // Try to get CWD from OS, fall back to cached value
                        self.id_to_child_pid
                            .get(id)
                            .and_then(|&pid| {
                                self.bus
                                    .os_input
                                    .as_ref()
                                    .and_then(|input| input.get_cwd(pid))
                            })
                            .or_else(|| self.terminal_cwds.get(id).cloned())
                    },
                })
        };

        let cwd = cwd.or_else(get_focused_cwd);
        let focused_plugin_id = self
            .active_panes
            .get(&client_id)
            .and_then(|pane| match pane {
                PaneId::Plugin(plugin_id) => Some(*plugin_id),
                _ => None,
            });

        if let RunPluginOrAlias::Alias(alias) = &mut run {
            let cwd = get_focused_cwd();
            alias.set_caller_cwd_if_not_set(cwd);
            let pane_id = self
                .active_panes
                .get(&client_id)
                .and_then(|pane| match pane {
                    PaneId::Terminal(id) => Some(*id),
                    PaneId::Plugin(_) => None,
                });
            alias.set_caller_terminal_pane_id_if_not_set(pane_id);
        }
        self.bus.senders.send_to_plugin(PluginInstruction::Load(
            should_float,
            should_open_in_place,
            close_replaced_pane,
            pane_title,
            run,
            Some(tab_index),
            pane_id_to_replace,
            client_id,
            size,
            cwd,
            focused_plugin_id,
            skip_cache,
            should_focus_plugin,
            floating_pane_coordinates,
            completion_tx,
            overlay_placement,
        ))?;
        Ok(())
    }
    /// The folder an action asks to start in, taken before it is spawned and
    /// consumed.
    fn action_cwd(terminal_action: &TerminalAction) -> Option<PathBuf> {
        match terminal_action {
            TerminalAction::RunCommand(run_command) => run_command.cwd.clone(),
            TerminalAction::OpenFile(payload) => payload.cwd.clone(),
        }
    }

    /// The clients this pane is the active one for, who a cwd change concerns.
    fn focused_clients(&self, pane_id: PaneId) -> Vec<ClientId> {
        self.active_panes
            .iter()
            .filter(|(_, active_pane)| *active_pane == &pane_id)
            .map(|(client_id, _)| *client_id)
            .collect()
    }

    /// Record where a pane is, and say so when it has moved.
    ///
    /// The single writer of `terminal_cwds`. Everything that learns a pane's
    /// folder — the initial capture, the ticker, OSC 7 — goes through here, so
    /// there is one place that decides a change has happened and one place that
    /// announces it.
    fn set_terminal_cwd(&mut self, terminal_id: TerminalId, cwd: PathBuf) {
        if self.terminal_cwds.get(&terminal_id) == Some(&cwd) {
            return;
        }
        let pane_id = PaneId::Terminal(terminal_id);
        let focused_client_ids = self.focused_clients(pane_id);
        let _ = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::CwdChanged(
                    pane_id,
                    cwd.clone(),
                    focused_client_ids.into_iter().collect(),
                ),
            )]));
        let _ = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::UpdatePaneCwd(
                terminal_id,
                Some(cwd.clone()),
            ));
        self.terminal_cwds.insert(terminal_id, cwd);
    }

    /// Drop what we knew about a pane that has gone.
    ///
    /// Terminal ids are handed out again after a pane closes, so a folder left
    /// behind here is not merely wasted space — it is the wrong answer for
    /// whichever pane inherits the id.
    /// Tell Screen which machine a pane runs on, so it can reach the plugins
    /// that decide what a pane *is* — two panes in the same folder on different
    /// machines are in different folders, and anything keying on the path alone
    /// would merge them.
    fn report_terminal_host(&mut self, terminal_id: TerminalId) {
        let host = self
            .bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.remote_pane_host(terminal_id));
        if host.is_some() {
            // The worktree rides with the host because it is the same fact about the
            // same transport, discovered at the same moment: a Firecracker pane that
            // joined a VM asked for no worktree, and what it got is only knowable once
            // the provider has answered.
            let worktree = self.remote_worktree(terminal_id);
            let _ = self
                .bus
                .senders
                .send_to_screen(ScreenInstruction::UpdatePaneHost(
                    terminal_id,
                    host,
                    worktree,
                ));
        }
    }

    fn remote_worktree(&self, terminal_id: TerminalId) -> Option<std::path::PathBuf> {
        self.bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.remote_pane_worktree(terminal_id))
    }

    /// The respawn counterpart of `report_terminal_host`. A pane's host is fixed for the life
    /// of one command, but a workflow step can respawn the pane on a different machine — or on
    /// none — so unlike the first report this one has to send `None` as well, or a
    /// remote-to-local rerun leaves the pane's reported host stale. One update rather than a
    /// `forget` followed by a `report`, so a plugin watching pane hosts never sees a spurious
    /// host-less pane in between.
    fn re_report_terminal_host(&mut self, terminal_id: TerminalId) {
        let host = self
            .bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.remote_pane_host(terminal_id));
        let worktree = self.remote_worktree(terminal_id);
        let _ = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::UpdatePaneHost(
                terminal_id,
                host,
                worktree,
            ));
    }

    fn forget_terminal_host(&mut self, terminal_id: TerminalId) {
        let _ = self
            .bus
            .senders
            .send_to_screen(ScreenInstruction::UpdatePaneHost(terminal_id, None, None));
    }

    fn forget_terminal_cwd(&mut self, terminal_id: TerminalId) {
        if self.terminal_cwds.remove(&terminal_id).is_some() {
            let _ = self
                .bus
                .senders
                .send_to_screen(ScreenInstruction::UpdatePaneCwd(terminal_id, None));
        }
    }

    /// The folder a pane starts in.
    ///
    /// This used to be recorded silently, and the ticker below only reports a
    /// cwd that *differs* from the recorded one — so a pane whose process never
    /// left the folder it started in announced its folder to nobody, ever. It
    /// was a race, not a certainty: when the child had not forked yet there was
    /// nothing to record, and the ticker then reported it after all. That is why
    /// the requested cwd is a fallback here rather than an afterthought — the
    /// caller has already resolved it, and it is better than nothing to go on.
    fn capture_initial_cwd(
        &mut self,
        terminal_id: TerminalId,
        child_pid: ProcessId,
        requested_cwd: Option<PathBuf>,
    ) {
        let cwd = self
            .bus
            .os_input
            .as_ref()
            .and_then(|os_input| os_input.get_cwd(child_pid))
            .or(requested_cwd);
        if let Some(cwd) = cwd {
            self.set_terminal_cwd(terminal_id, cwd);
        }
    }

    pub fn update_and_report_cwds(&mut self) {
        use std::sync::atomic::Ordering;

        // Local panes are gated on having produced output, because inspecting
        // them costs a `sysinfo` refresh per pane. Remote panes are not: their
        // information is pushed up by the broker on its own ticker and is
        // already sitting in a local cache, so re-reading it costs nothing —
        // which also means a silent remote agent, one sitting at its prompt
        // producing nothing, stays visible where a local one would go stale.
        let active_local: Vec<TerminalId> = self
            .id_to_child_pid
            .keys()
            .copied()
            .filter(|id| {
                self.pane_activity_flags
                    .get(id)
                    .map(|f| f.swap(false, Ordering::Relaxed))
                    .unwrap_or(false)
            })
            .collect();
        let remote_ids: Vec<TerminalId> = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.remote_pane_ids())
            .unwrap_or_default();

        if active_local.is_empty() && remote_ids.is_empty() {
            return;
        }

        let pids: Vec<_> = active_local
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id))
            .copied()
            .collect();

        let (pids_to_cwds, pids_to_cmds) = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_cwds(pids))
            .unwrap_or_default();

        for terminal_id in &active_local {
            let process_id = self.id_to_child_pid.get(terminal_id).copied();
            let cwd = process_id.and_then(|pid| pids_to_cwds.get(&pid).cloned());
            let cmd = process_id.and_then(|pid| pids_to_cmds.get(&pid).cloned());

            if let Some(cwd) = cwd {
                self.set_terminal_cwd(*terminal_id, cwd);
            }
            if let Some(cmd) = cmd {
                self.terminal_cmds.insert(*terminal_id, cmd);
            }
        }

        let panes: Vec<(TerminalId, ProcessId)> = active_local
            .iter()
            .filter_map(|id| self.id_to_child_pid.get(id).map(|pid| (*id, *pid)))
            .collect();
        let mut foreground_cmds = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| os_input.get_foreground_cmds(&panes, &self.post_command_discovery_hook))
            .unwrap_or_default();

        // The same three facts for a remote pane, from the broker's report
        // rather than from this machine's process table. Everything downstream
        // is identical, which is what lets agent detection — pure string
        // matching over argv — work on a remote pane with no changes at all.
        for terminal_id in &remote_ids {
            let status = match self
                .bus
                .os_input
                .as_ref()
                .and_then(|os_input| os_input.remote_pane_status(*terminal_id))
            {
                Some(status) => status,
                None => continue,
            };
            if let Some(cwd) = status.cwd {
                self.set_terminal_cwd(*terminal_id, PathBuf::from(cwd));
            }
            if let Some(shell_argv) = status.shell_argv {
                self.terminal_cmds.insert(*terminal_id, shell_argv);
            }
            if let Some(foreground) = status.foreground_argv {
                foreground_cmds.insert(*terminal_id, foreground);
            }
        }

        let active_terminal_ids: Vec<TerminalId> = active_local
            .iter()
            .copied()
            .chain(remote_ids.iter().copied())
            .collect();

        for terminal_id in &active_terminal_ids {
            let foreground_cmd: Vec<String> = foreground_cmds
                .get(terminal_id)
                .cloned()
                .unwrap_or_default();

            if self.terminal_foreground_cmds.get(terminal_id) != Some(&foreground_cmd) {
                let pane_id = PaneId::Terminal(*terminal_id);
                let focused_client_ids: Vec<ClientId> = self
                    .active_panes
                    .iter()
                    .filter(|(_, active_pane)| *active_pane == &pane_id)
                    .map(|(client_id, _)| *client_id)
                    .collect();
                let (command, is_foreground) = if foreground_cmd.is_empty() {
                    let shell_cmd = self
                        .terminal_cmds
                        .get(terminal_id)
                        .cloned()
                        .unwrap_or_default();
                    (shell_cmd, false)
                } else {
                    (foreground_cmd.clone(), true)
                };
                let _ = self
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::Update(vec![(
                        None,
                        None,
                        Event::CommandChanged(
                            pane_id,
                            command,
                            is_foreground,
                            focused_client_ids.into_iter().collect(),
                        ),
                    )]));
                self.terminal_foreground_cmds
                    .insert(*terminal_id, foreground_cmd);
            }
        }
    }

    pub fn reconfigure(
        &mut self,
        default_editor: Option<PathBuf>,
        post_command_discovery_hook: Option<String>,
    ) {
        self.default_editor = default_editor;
        self.post_command_discovery_hook = post_command_discovery_hook;
    }

    pub fn notify_cwd_from_osc7(&mut self, terminal_id: TerminalId, path: PathBuf) {
        use std::sync::atomic::Ordering;

        self.set_terminal_cwd(terminal_id, path);
        if let Some(flag) = self.pane_activity_flags.get(&terminal_id) {
            flag.store(false, Ordering::Relaxed);
        }
    }

    pub fn send_sigint_to_pane(&self, pane_id: PaneId) {
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
    pub fn register_child_state_callback(&self) {
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
    /// place `zellij action agent-report` takes a local pane's, minus the
    /// action. Registered once, like the child state callback.
    pub fn register_agent_report_callback(&self) {
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
    pub fn resume_suspended_pane(&self, pane_id: PaneId) {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                self.signal_pane_child(terminal_id, ChildSignal::Cont);
            },
            PaneId::Plugin(plugin_id) => {
                log::warn!("Cannot resume plugin pane {}", plugin_id);
            },
        }
    }

    pub fn send_sigkill_to_pane(&self, pane_id: PaneId) {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                self.signal_pane_child(terminal_id, ChildSignal::Kill);
            },
            PaneId::Plugin(plugin_id) => {
                log::warn!("Cannot send SIGKILL to plugin pane {}", plugin_id);
            },
        }
    }

    pub fn get_pane_pid(&self, pane_id: PaneId) -> GetPanePidResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                // A remote pane's pid is meaningful only on its own machine.
                // Handing it to a plugin invites exactly the mistake this whole
                // boundary exists to prevent — the plugin would `kill` or `stat`
                // whatever local process happens to hold that number.
                if self
                    .bus
                    .os_input
                    .as_ref()
                    .is_some_and(|os_input| os_input.is_remote_pane(terminal_id))
                {
                    return GetPanePidResponse::Err(format!(
                        "Terminal pane {terminal_id} runs on another machine, so it has no pid \
                         meaningful on this one"
                    ));
                }
                if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                    GetPanePidResponse::Ok(child_pid)
                } else {
                    GetPanePidResponse::Err(format!(
                        "Terminal pane {} not found or not running",
                        terminal_id
                    ))
                }
            },
            PaneId::Plugin(plugin_id) => {
                GetPanePidResponse::Err(format!("Cannot get PID for plugin pane {}", plugin_id))
            },
        }
    }
    pub fn get_pane_running_command(&self, pane_id: PaneId) -> GetPaneRunningCommandResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                // `hocus-agentd` calls this directly to repair panes the
                // activity-flag poll missed, so a remote pane has to answer it
                // too — otherwise a remote agent would only ever be classified
                // from its title.
                if let Some(status) = self
                    .bus
                    .os_input
                    .as_ref()
                    .and_then(|os_input| os_input.remote_pane_status(terminal_id))
                {
                    return match status.foreground_argv.or(status.shell_argv) {
                        Some(argv) => GetPaneRunningCommandResponse::Ok(argv),
                        None => GetPaneRunningCommandResponse::Err(format!(
                            "the machine hosting terminal pane {terminal_id} has not reported \
                             its running command yet"
                        )),
                    };
                }
                if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                    // Query OS for current running command
                    if let Some(os_input) = self.bus.os_input.as_ref() {
                        // First, try to get the foreground command (e.g., nvim running in bash)
                        let foreground_cmds = os_input.get_foreground_cmds(
                            &[(terminal_id, child_pid)],
                            &self.post_command_discovery_hook,
                        );
                        let cmd_foreground = foreground_cmds.get(&terminal_id);

                        // If no foreground command, fall back to the shell itself
                        let (_cwds, cmds) = os_input.get_cwds(vec![child_pid]);
                        let cmd_sysinfo = cmds.get(&child_pid);

                        if let Some(command_args) = cmd_foreground {
                            GetPaneRunningCommandResponse::Ok(command_args.clone())
                        } else if let Some(command_args) = cmd_sysinfo {
                            GetPaneRunningCommandResponse::Ok(command_args.clone())
                        } else {
                            GetPaneRunningCommandResponse::Err(format!(
                                "Could not retrieve running command for terminal pane {}",
                                terminal_id
                            ))
                        }
                    } else {
                        GetPaneRunningCommandResponse::Err("OS input not available".to_string())
                    }
                } else {
                    GetPaneRunningCommandResponse::Err(format!(
                        "Terminal pane {} not found or not running",
                        terminal_id
                    ))
                }
            },
            PaneId::Plugin(plugin_id) => GetPaneRunningCommandResponse::Err(format!(
                "Cannot get running command for plugin pane {}",
                plugin_id
            )),
        }
    }
    pub fn get_pane_cwd(&self, pane_id: PaneId) -> GetPaneCwdResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                // A remote pane's cwd is a path on *its* machine. Reported as-is:
                // rewriting it would be a lie, and the plugins that consume it
                // pair it with the host when deciding which project it belongs
                // to.
                if let Some(status) = self
                    .bus
                    .os_input
                    .as_ref()
                    .and_then(|os_input| os_input.remote_pane_status(terminal_id))
                {
                    return match status.cwd {
                        Some(cwd) => GetPaneCwdResponse::Ok(PathBuf::from(cwd)),
                        None => GetPaneCwdResponse::Err(format!(
                            "the machine hosting terminal pane {terminal_id} has not reported \
                             its working directory yet"
                        )),
                    };
                }
                if let Some(&child_pid) = self.id_to_child_pid.get(&terminal_id) {
                    // Query OS for current working directory
                    if let Some(os_input) = self.bus.os_input.as_ref() {
                        let (cwds, _cmds) = os_input.get_cwds(vec![child_pid]);
                        if let Some(cwd) = cwds.get(&child_pid) {
                            GetPaneCwdResponse::Ok(cwd.clone())
                        } else {
                            GetPaneCwdResponse::Err(format!(
                                "Could not retrieve CWD for terminal pane {}",
                                terminal_id
                            ))
                        }
                    } else {
                        GetPaneCwdResponse::Err("OS input not available".to_string())
                    }
                } else {
                    GetPaneCwdResponse::Err(format!(
                        "Terminal pane {} not found or not running",
                        terminal_id
                    ))
                }
            },
            PaneId::Plugin(plugin_id) => {
                GetPaneCwdResponse::Err(format!("Cannot get CWD for plugin pane {}", plugin_id))
            },
        }
    }

    /// What Git said about a remote pane's folder, as the far machine sees it.
    ///
    /// Answered from the cache the broker pushes up, never by running Git here:
    /// the folder is a path on another machine, and this is the pty thread,
    /// which every pane in the session is waiting on. A local pane is an `Err`
    /// for the same reason — there is nothing cached to give, and spawning two
    /// `git` processes to make one would block that thread. Plugins branch on
    /// `PaneInfo::host` and run Git themselves for a local pane.
    pub fn get_pane_git_probe(&self, pane_id: PaneId) -> GetPaneGitProbeResponse {
        match pane_id {
            PaneId::Terminal(terminal_id) => {
                match self
                    .bus
                    .os_input
                    .as_ref()
                    .and_then(|os_input| os_input.remote_pane_git_probe(terminal_id))
                {
                    Some(probe) => GetPaneGitProbeResponse::Ok(PaneGitProbe {
                        host: probe.host,
                        cwd: probe.cwd,
                        exit_code: probe.exit_code,
                        metadata: probe.metadata,
                        config: probe.config,
                    }),
                    None => GetPaneGitProbeResponse::Err(format!(
                        "terminal pane {terminal_id} is not remote, or its machine has not \
                         reported a git probe yet"
                    )),
                }
            },
            PaneId::Plugin(plugin_id) => GetPaneGitProbeResponse::Err(format!(
                "cannot get a git probe for plugin pane {plugin_id}"
            )),
        }
    }
}

impl Drop for Pty {
    /// Close every local pane and every Firecracker pane on the way out. A
    /// plain ssh pane is deliberately not on the list: its broker keeps the
    /// process, and `zellij pty-host adopt --host` can pick it up later. A
    /// Firecracker pane's VM belongs to this session and is stopped with it,
    /// so the process is over either way; terminating it first lets the
    /// broker report the exit instead of the VM simply vanishing under it.
    fn drop(&mut self) {
        let firecracker_ids = self
            .bus
            .os_input
            .as_ref()
            .map(|os_input| {
                os_input
                    .remote_pane_ids()
                    .into_iter()
                    .filter(|id| os_input.is_firecracker_pane(*id))
                    .collect::<Vec<TerminalId>>()
            })
            .unwrap_or_default();
        let child_ids: HashSet<TerminalId> = self
            .id_to_child_pid
            .keys()
            .copied()
            .chain(firecracker_ids)
            .collect();
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
    match err.downcast_ref::<ZellijError>()? {
        ZellijError::CommandNotFound {
            terminal_id,
            command,
        } => Some((
            *terminal_id,
            format!(
                "Command not found: {command}\n\rIf you were including arguments as part of \
                 the command, try including them as 'args' instead."
            ),
        )),
        ZellijError::PaneStartFailed {
            terminal_id,
            message,
        } => Some((*terminal_id, message.clone())),
        _ => None,
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
fn release_pane_for_warp(
    pty: &mut Pty,
    terminal_id: TerminalId,
    to_session: &str,
    deadline: Instant,
) -> std::result::Result<ReleasedPane, String> {
    if let Err(e) = pty.check_terminal_can_be_warped(terminal_id) {
        return Err(format!("{e:#}"));
    }
    if let Err(e) = refuse_if_nobody_is_waiting(deadline) {
        return Err(format!("{e:#}"));
    }
    pty.release_terminal_for_warp(terminal_id, deadline)
        .map_err(|e| {
            let why = format!("could not move this pane to '{to_session}': {e:#}");
            strand_pane_after_warp(pty, terminal_id, &why, None);
            format!("{e:#}")
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
const READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

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
fn release_tab_for_warp(
    pty: &mut Pty,
    terminal_ids: Vec<TerminalId>,
    deadline: Instant,
) -> Vec<(TerminalId, std::result::Result<ReleasedPane, String>)> {
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

    // Every reader stopped before any socket is written to. A remote pane's
    // reader shares the connection the release is about to speak on, and it
    // shares it with *this* pane's release only — but stopping them all up
    // front costs one wait instead of one per pane.
    pty.stop_readers_for_warp(&terminal_ids);

    let budget = deadline.saturating_duration_since(Instant::now());
    let released: Vec<(TerminalId, std::result::Result<ReleasedPane, String>)> = {
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
            terminal_ids
                .iter()
                .zip(threads)
                .map(|(terminal_id, thread)| {
                    // A panicked release is a failed release. Reporting it as
                    // one puts the pane through the stranding below, which is
                    // the only way its process gets named to the user at all.
                    let result = thread
                        .join()
                        .unwrap_or_else(|_| Err(format!("releasing pane {terminal_id} panicked")));
                    (*terminal_id, result)
                })
                .collect()
        })
    };

    for (terminal_id, result) in &released {
        if result.is_ok() {
            pty.forget_released_terminal(*terminal_id);
        }
    }
    // A release that fails *after* every pane passed the check takes the whole
    // tab down with it: the ones that succeeded are orphaned, the one that
    // failed is severed, and none of them has a process on the other end any
    // more. Each says so, and the orphans say how to get theirs back.
    if released.iter().any(|(_, result)| result.is_err()) {
        for (terminal_id, result) in &released {
            match result {
                Ok(released) => strand_pane_after_warp(
                    pty,
                    *terminal_id,
                    "this tab could not be moved, and this pane had already been handed over",
                    Some(released.recovery_command()),
                ),
                Err(e) => strand_pane_after_warp(
                    pty,
                    *terminal_id,
                    &format!("this tab could not be moved: {e}"),
                    None,
                ),
            }
        }
    }
    released
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
fn strand_pane_after_warp(pty: &Pty, terminal_id: TerminalId, why: &str, recovery: Option<String>) {
    let mut message = format!("\r\n\u{1b}[31m── {why} ──\u{1b}[0m\r\n");
    if let Some(recovery) = recovery {
        message.push_str(&format!(
            "\u{1b}[33mThe process is still running — recover it with `{recovery}`\u{1b}[0m\r\n"
        ));
    }
    // The pane's own command, so `<ENTER>` on the held pane runs what was in it
    // rather than something invented here. `hold_on_close` is deliberately left
    // false: this hold is not the pane's command policy, it is a one-off.
    let argv = pty
        .terminal_cmds
        .get(&terminal_id)
        .cloned()
        .unwrap_or_default();
    let mut argv = argv.into_iter();
    let run_command = RunCommand {
        command: argv
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(get_default_shell),
        args: argv.collect(),
        cwd: pty.terminal_cwds.get(&terminal_id).cloned(),
        ..Default::default()
    };
    send_pane_error_to_screen(pty.bus.senders.clone(), terminal_id, message, run_command)
        .non_fatal();
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

pub fn get_default_shell() -> PathBuf {
    PathBuf::from(std::env::var("SHELL").unwrap_or_else(|_| {
        log::warn!("Cannot read SHELL env, falling back to use /bin/sh");
        "/bin/sh".to_string()
    }))
}

#[cfg(test)]
#[path = "./unit/pty_tests.rs"]
mod pty_tests;
