use crate::os_input_output::AsyncReader;
use crate::route::NotificationEnd;
use crate::screen::ScreenInstruction;
use crate::{
    plugins::{DumpSessionLayoutResponse, PluginId},
    session_layout_metadata::SessionLayoutMetadata,
    ClientId, PaneId,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use std::{collections::HashMap, path::PathBuf};
use zellij_types::internal_ids::ProcessId;
use zellij_types::internal_ids::SessionName;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;
use zellij_types::internal_ids::TerminalId;

use zellij_runtime::errors::prelude::*;
use zellij_types::diagnostics::PtyContext;
use zellij_types::pane_size::Size;
use zellij_types::{agent::AgentInfo, geometry::FloatingPaneCoordinates};
use {
    zellij_types::data::CommandOrPlugin, zellij_types::data::GetPaneCwdResponse,
    zellij_types::data::GetPaneGitProbeResponse, zellij_types::data::GetPanePidResponse,
    zellij_types::data::GetPaneRunningCommandResponse, zellij_types::data::NewPanePlacement,
    zellij_types::data::OverlayPlacement, zellij_types::data::SessionInfo,
    zellij_types::data::WarpPlacement, zellij_types::data::WarpedPane,
    zellij_types::data::WarpedTab, zellij_types::input::command::RunCommand,
    zellij_types::input::command::TerminalAction, zellij_types::input::layout::FloatingPaneLayout,
    zellij_types::input::layout::RunPluginOrAlias, zellij_types::input::layout::TabLayoutInfo,
    zellij_types::input::layout::TiledPaneLayout,
};

pub type VteBytes = Vec<u8>;

/// Build the screen update using the caller's exit policy (editors always close).
pub fn terminal_exit_instruction(
    pane_id: PaneId,
    exit_status: Option<i32>,
    command: RunCommand,
    hold_on_close: bool,
) -> ScreenInstruction {
    if hold_on_close {
        ScreenInstruction::HoldPane(pane_id, exit_status, command)
    } else {
        ScreenInstruction::ClosePane(pane_id, None, None, exit_status)
    }
}

/// A running child has no exit status; an exited child may lack a numeric status.
pub fn held_command_on_exit(
    command: &RunCommand,
    child_exit_status: Option<Option<i32>>,
) -> Option<RunCommand> {
    child_exit_status
        .filter(|_| command.hold_on_close)
        .map(|_| command.clone())
}

/// The shell a pane runs when neither the command nor the config names one: `$SHELL`, or
/// `/bin/sh` when that is unset.
pub fn get_default_shell() -> PathBuf {
    PathBuf::from(std::env::var("SHELL").unwrap_or_else(|_| {
        log::warn!("Cannot read SHELL env, falling back to use /bin/sh");
        "/bin/sh".to_string()
    }))
}

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
    pub held: Option<zellij_types::data::HeldTerminal>,
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
    /// The default caption the pane was wearing (`WarpedPane::caption`), applied once the pane
    /// exists, for the same reason as `replay`.
    pub caption: Option<String>,
}

impl AdoptedTabTerminal {
    /// The command to hold the pane for on arrival: only a pane whose child has
    /// exited *and* that would have been held in its old session. One that would
    /// have closed there is closed here too (`Pty::apply_layout`).
    pub fn hold_for_command(&self) -> Option<RunCommand> {
        match &self.held {
            Some(held) => held.first_run.then(|| self.cmd.clone()),
            None => held_command_on_exit(&self.cmd, self.child_exit_status),
        }
    }
}

/// The tiled and floating halves of a tab's adopted terminals, in the order
/// their slots are filled.
pub type AdoptedTabTerminals = (Vec<AdoptedTabTerminal>, Vec<AdoptedTabTerminal>);

#[doc(hidden)]
pub struct SpawnedTerminal {
    pub terminal_id: TerminalId,
    pub reader: Box<dyn AsyncReader>,
    pub child_pid: Option<ProcessId>,
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
    pub reader: Box<dyn AsyncReader>,
    pub child_pid: Option<ProcessId>,
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
    pub fn new(value: T) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Some(value))))
    }

    pub fn take(self) -> Result<T> {
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
    pub pane_id: PaneId,
    pub run_command: RunCommand,
    /// Held until the rerun finishes; dropping it then is what notifies the
    /// waiter, so nothing reads it.
    pub _completion_tx: Option<NotificationEnd>,
    /// Which request this is; a completion whose generation the pane no longer
    /// carries belongs to a pane that was closed meanwhile.
    pub generation: u64,
    pub previous_host: Option<String>,
    pub requested_cwd: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct PendingRemoteSpawn {
    pub terminal_id: TerminalId,
    pub terminal_action: TerminalAction,
}

/// What a spawn worker hands back: the job, plus its result in a `OneShot` so
/// a non-`Clone` reader can ride a `PtyInstruction`.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub enum PendingSpawn {
    Terminal(PendingRemoteSpawn, OneShot<Result<SpawnedTerminal>>),
    Rerun(Box<PendingRerun>, OneShot<Result<SpawnedRerun>>),
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
        Option<PathBuf>,        // CWD
        Option<TerminalAction>, // Default Shell
        Vec<(
            TabId,
            TabLayoutInfo,
            HashMap<RunPluginOrAlias, Vec<PluginId>>,
        )>, // (resolved ID, layout, plugin_ids) per tab
        bool,                   // retain_existing_terminal_panes
        bool,                   // retain_existing_plugin_panes
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
        held: Option<zellij_types::data::HeldTerminal>,
        terminal_id: TerminalId,
        to_session: SessionName,
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
        held: std::collections::HashMap<TerminalId, zellij_types::data::HeldTerminal>,
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
        /// The session's configured default shell, as `Tab::default_shell` holds it.
        shell: PathBuf,
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
        session_name: SessionName,
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
    ListClientsToPlugin(SessionLayoutMetadata, PluginId, ClientId, Option<u64>),
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
        refresh_token: Option<String>,
        pane_id: PaneId,
        cwd: String,
        response_channel: crossbeam::channel::Sender<GetPaneGitProbeResponse>,
    },
    WorkflowRemotePanes(crossbeam::channel::Sender<Vec<serde_json::Value>>),
    WorkflowSignal {
        terminal_id: TerminalId,
        owner: String,
        process: Option<ProcessId>,
        supervisor: String,
        kill: bool,
        reply: crossbeam::channel::Sender<std::result::Result<(), String>>,
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
