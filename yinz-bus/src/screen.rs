use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::TerminalId;

use crate::loading_indication::LoadingIndication;
use crate::plugins::{DumpSessionLayoutResponse, PluginId, PluginRenderAsset};
use crate::pty::{AdoptedTabPanes, ClientTabIndexOrPaneId, ReplacedPane, VteBytes};
use crate::route::NotificationEnd;
use crate::warp::ReleasedPaneForWarp;
use crate::ClientId;
use crate::PaneId;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};
use yinz_config::screen_config::ScreenConfig;
use yinz_host_protocol::notifications::PendingNotification;
use yinz_ipc::ipc::PixelDimensions;
use yinz_ipc::nested_session::NestedSessionMessage;
use yinz_types::agent::AgentInfo;
use yinz_types::diagnostics::ScreenContext;
use yinz_types::geometry::{Direction, FloatingPaneCoordinates, Resize, ResizeStrategy};
use yinz_types::input::actions::Action;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::MouseEvent;
use yinz_types::pane_size::Size;
use yinz_types::position::Position;
use yinz_types::style::HostTerminalThemeMode;
use {
    yinz_types::data::CommandOrPlugin, yinz_types::data::ContextMenuItem,
    yinz_types::data::EventType, yinz_types::data::GetFocusedPaneInfoResponse,
    yinz_types::data::InputMode, yinz_types::data::LayoutInfo, yinz_types::data::LayoutWithError,
    yinz_types::data::ListPanesResponse, yinz_types::data::ListTabsResponse,
    yinz_types::data::ModeInfo, yinz_types::data::NewPanePlacement,
    yinz_types::data::NotificationInfo, yinz_types::data::OverlayPlacement,
    yinz_types::data::PaneInfo, yinz_types::data::PaneScrollbackResponse,
    yinz_types::data::PluginPermission, yinz_types::data::RegexHighlight,
    yinz_types::data::SessionInfo, yinz_types::data::TabInfo, yinz_types::data::WarpPlacement,
    yinz_types::data::WarpedPane, yinz_types::data::WarpedTab,
};
use {yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction};
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::layout::SwapFloatingLayout,
    yinz_types::input::layout::SwapTiledLayout, yinz_types::input::layout::TabLayoutInfo,
    yinz_types::input::layout::TiledPaneLayout,
};

/// Project probes finish outside the screen thread and are applied by version.
pub type AgentEventProjectMemberships = BTreeMap<String, Result<Option<Vec<PaneId>>, String>>;

/// Instructions that can be sent to the [`Screen`].
///
/// `IntoStaticStr` yields the variant name, which is what `From<&ScreenInstruction> for
/// ScreenContext` maps through; `VariantNames` lets `unit/instruction_context_tests.rs` prove
/// that mapping is total.
#[derive(Debug, Clone, strum_macros::IntoStaticStr, strum_macros::VariantNames)]
pub enum ScreenInstruction {
    SubscribePaneOutput(
        PluginId,
        ClientId,
        Option<BTreeMap<String, String>>,
        Option<u64>,
    ),
    // None removes every generation and client on complete plugin unload/reload.
    UnsubscribePaneOutput(PluginId, Option<u64>),
    PtyBytes(TerminalId, VteBytes),
    RemotePaneReady(TerminalId),
    PluginBytes(Vec<PluginRenderAsset>),
    Render,
    RenderToClients,
    RenderWriterReady,
    NewPane(
        PaneId,
        Option<InitialTitle>,
        HoldForCommand,
        Option<Run>, // invoked with
        NewPanePlacement,
        bool, // start suppressed
        ClientTabIndexOrPaneId,
        Option<NotificationEnd>, // completion signal
        bool,                    // set_blocking
    ),
    OpenInPlaceEditor(PaneId, ClientTabIndexOrPaneId),
    TogglePaneEmbedOrFloating(ClientId, Option<NotificationEnd>),
    ToggleFloatingPanes(ClientId, Option<TerminalAction>, Option<NotificationEnd>),
    ShowFloatingPanes {
        client_id: ClientId,
        tab_id: Option<TabId>,
        completion: Option<NotificationEnd>,
    },
    HideFloatingPanes {
        client_id: ClientId,
        tab_id: Option<TabId>,
        completion: Option<NotificationEnd>,
    },
    AreFloatingPanesVisible {
        client_id: ClientId,
        tab_id: Option<TabId>,
        completion: Option<NotificationEnd>,
    },
    /// Give an open palette or context menu first refusal before a resolved keybinding runs.
    /// Ordinary writes are intercepted on the screen thread instead. `completion`'s
    /// `stdout_message` carries the answer back as `"true"`/`"false"`.
    ContextMenuHandleKeyIfOpen {
        key: Option<KeyWithModifier>,
        toggle_command_palette: bool,
        open_agents: bool,
        client_id: ClientId,
        completion: Option<NotificationEnd>,
    },
    PasteFromClient(String, ClientId, Option<NotificationEnd>),
    WriteCharacter(
        Option<KeyWithModifier>,
        Vec<u8>,
        bool,
        ClientId,
        Option<NotificationEnd>,
    ), // bool ->
    // is_kitty_keyboard_protocol
    Resize(ClientId, ResizeStrategy, Option<NotificationEnd>),
    SwitchFocus(ClientId, Option<NotificationEnd>),
    FocusNextPane(ClientId, Option<NotificationEnd>),
    FocusPreviousPane(ClientId, Option<NotificationEnd>),
    FocusLastPane(ClientId, Option<NotificationEnd>),
    MoveFocusLeft(ClientId, Option<NotificationEnd>),
    MoveFocusLeftOrPreviousTab(ClientId, Option<NotificationEnd>),
    MoveFocusDown(ClientId, Option<NotificationEnd>),
    MoveFocusUp(ClientId, Option<NotificationEnd>),
    MoveFocusRight(ClientId, Option<NotificationEnd>),
    MoveFocusRightOrNextTab(ClientId, Option<NotificationEnd>),
    MovePane(ClientId, Option<NotificationEnd>),
    MovePaneBackwards(ClientId, Option<NotificationEnd>),
    MovePaneUp(ClientId, Option<NotificationEnd>),
    MovePaneDown(ClientId, Option<NotificationEnd>),
    MovePaneRight(ClientId, Option<NotificationEnd>),
    MovePaneLeft(ClientId, Option<NotificationEnd>),
    Exit,
    ClearScreen(ClientId, Option<NotificationEnd>),
    DumpScreen(
        Option<String>,
        ClientId,
        bool,
        Option<PaneId>,
        Option<NotificationEnd>,
        Option<ClientId>, // cli_client_id - used to send output to the CLI client's STDOUT
        bool,             // ansi - preserve ANSI styling in the dump output
    ),
    DumpLayout(Option<PathBuf>, ClientId, Option<NotificationEnd>), // PathBuf is the default configured
    // shell
    SaveSession(ClientId, Option<NotificationEnd>),
    DumpLayoutToPlugin {
        plugin_id: PluginId,
        tab_index: Option<TabId>,
        response_channel: crossbeam::channel::Sender<DumpSessionLayoutResponse>,
        trace: crate::plugins::LayoutQueryTrace,
    },
    GetCellPixelSize {
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Option<yinz_types::pane_size::SizeInPixels>>,
    },
    ReportCellPixelSize(PluginId, ClientId),
    GetFocusedPaneInfo {
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<GetFocusedPaneInfoResponse>,
    },
    GetPaneInfo {
        pane_id: PaneId,
        response_channel: crossbeam::channel::Sender<Option<PaneInfo>>,
    },
    GetTabInfo {
        tab_id: TabId,
        response_channel: crossbeam::channel::Sender<Option<TabInfo>>,
    },
    EditScrollback(ClientId, bool, Option<NotificationEnd>),
    GetPaneScrollback {
        pane_id: PaneId,
        client_id: ClientId,
        get_full_scrollback: bool,
        response_channel: crossbeam::channel::Sender<PaneScrollbackResponse>,
    },
    ScrollUp(ClientId, Option<NotificationEnd>),
    ScrollUpAt(Position, ClientId, Option<NotificationEnd>),
    ScrollDown(ClientId, Option<NotificationEnd>),
    ScrollDownAt(Position, ClientId, Option<NotificationEnd>),
    ScrollToBottom(ClientId, Option<NotificationEnd>),
    ScrollToTop(ClientId, Option<NotificationEnd>),
    ScrollToPreviousPrompt(ClientId, Option<NotificationEnd>),
    ScrollToNextPrompt(ClientId, Option<NotificationEnd>),
    SelectCommandAtScrollPosition(ClientId, Option<NotificationEnd>),
    CopyLastCommandOutput(ClientId, Option<NotificationEnd>),
    MoveTextSelection(Direction, ClientId, Option<NotificationEnd>),
    SwapTextSelectionEnds(ClientId, Option<NotificationEnd>),
    MoveNotificationSelection(Direction, ClientId, Option<NotificationEnd>),
    ActivateNotification(ClientId, Option<NotificationEnd>),
    ActivateLastNotification(ClientId, Option<NotificationEnd>),
    ToggleNotificationDetail(ClientId, Option<NotificationEnd>),
    /// The feedback flash `CopyLastCommandOutput` painted has run its course.
    ClearCommandOutputFlash(PaneId),
    PageScrollUp(ClientId, Option<NotificationEnd>),
    PageScrollDown(ClientId, Option<NotificationEnd>),
    HalfPageScrollUp(ClientId, Option<NotificationEnd>),
    HalfPageScrollDown(ClientId, Option<NotificationEnd>),
    ClearScroll(ClientId),
    CloseFocusedPane(ClientId, Option<NotificationEnd>),
    ToggleActiveTerminalFullscreen(ClientId, Option<NotificationEnd>),
    ToggleActiveTerminalNoUiFullscreen(ClientId, Option<NotificationEnd>),
    TogglePaneFrames(Option<NotificationEnd>),
    ToggleCommandPalette(ClientId, Option<NotificationEnd>),
    OpenAgents(ClientId, Option<NotificationEnd>),
    CommandPalette(crate::command_palette::Instruction),
    SetPaneFrameStyle(PaneFrameStyle, Option<NotificationEnd>),
    SetSelectable(PaneId, bool),
    ShowPluginCursor(PluginId, ClientId, Option<(usize, usize)>),
    CloseSelf(PluginId, ClientId),
    RetirePicker {
        finder: PluginId,
        pane: Option<yinz_types::internal_ids::TerminalId>,
        client: ClientId,
    },
    ClosePane(
        PaneId,
        Option<ClientId>,
        Option<NotificationEnd>,
        Option<i32>,
    ), // i32 -> optional exit
    // status
    HoldPane(PaneId, Option<i32>, RunCommand),
    /// Restore a transferred hold without firing exit hooks or queued respawns.
    RestoreHeldPane(PaneId, yinz_types::data::HeldTerminal),
    /// The pane's root process stopped (`true`) or started running again
    /// (`false`). Not a kind of hold: the pane keeps its screen contents and its
    /// pty, it just cannot be woken from inside itself.
    SetPaneSuspended(PaneId, bool),
    UpdatePaneName(Vec<u8>, ClientId, Option<NotificationEnd>),
    /// Where a terminal pane's process is, as pty last saw it. `None` forgets
    /// it, for a pane that has gone.
    ///
    /// Held here so every pane manifest carries it. `CwdChanged` announces the
    /// move, but an announcement is only heard once: a plugin that was not
    /// loaded yet, or that missed the event, had no second way to find out and
    /// stayed wrong about the pane for as long as it lived.
    UpdatePaneCwd(TerminalId, Option<PathBuf>),
    /// A terminal pane's foreground command, as pty last saw it. Empty forgets it: the
    /// pane's shell is in the foreground, or the pane has gone.
    ///
    /// Held here so `agent list` can name an agent someone started by hand in a shell,
    /// which has no agent record until it reports.
    UpdatePaneForegroundCommand(TerminalId, Vec<String>, Option<String>),
    /// The machine a pane's process runs on, for panes that are not local.
    /// Sent once when the pane is created and once when it closes.
    UpdatePaneHost(TerminalId, Option<String>),
    /// A decoration to compose in front of a pane's title, or `None` to drop it.
    ///
    /// Separate from every rename instruction on purpose: this does not touch the
    /// pane's name, so a rename cannot erase it and it cannot erase a rename. See
    /// `Pane::set_title_prefix`.
    SetPaneTitlePrefix(PaneId, Option<String>),
    SetPaneSubtitle(PaneId, Option<String>),
    /// A default caption for a pane, as opposed to a name: see `WarpedPane::caption`.
    SetPaneCaption(PaneId, String),
    UndoRenamePane(ClientId, Option<NotificationEnd>),
    NewTab(
        Option<PathBuf>,
        Option<TerminalAction>,
        Option<TiledPaneLayout>,
        Vec<FloatingPaneLayout>,
        Option<String>,
        // `None` falls back to `Screen::default_layout`'s swap layouts.
        (
            Option<Vec<SwapTiledLayout>>,
            Option<Vec<SwapFloatingLayout>>,
        ),
        Option<Vec<CommandOrPlugin>>, // initial_panes
        bool,                         // block_on_first_terminal
        bool,                         // should_change_focus_to_new_tab
        ClientId,
        Option<NotificationEnd>, // completion signal
    ),
    /// Apply layout to tab with given stable ID.
    ///
    /// The tab ID parameter (TabId) is a stable identifier (not position) from the
    /// NewTab → ApplyLayout async flow that passes IDs between threads.
    ApplyLayout(
        TiledPaneLayout,
        Vec<FloatingPaneLayout>,
        Vec<(TerminalId, HoldForCommand)>, // new pane pids
        Vec<(TerminalId, HoldForCommand)>, // new floating pane pids
        Vec<(TerminalId, RunCommand, Option<AgentInfo>)>, // all adopted terminals, including those without agents
        HashMap<RunPluginOrAlias, Vec<PluginId>>,
        TabId, // tab_id - stable identifier from NewTab instruction
        bool,  // should change focus to new tab
        ClientId,
        Option<NotificationEnd>,               // regular completion signal
        Option<(TerminalId, NotificationEnd)>, // blocking_terminal (terminal_id, completion_tx)
    ),
    LayoutSpawnFailed(TabId, Option<crate::route::AgentTabPlacement>),
    SwitchTabNext(ClientId, Option<NotificationEnd>),
    SwitchTabPrev(ClientId, Option<NotificationEnd>),
    ToggleActiveSyncTab(ClientId, Option<NotificationEnd>),
    CloseTab(ClientId, Option<NotificationEnd>),
    TabGroup(
        yinz_types::input::actions::Action,
        ClientId,
        Option<NotificationEnd>,
    ),
    GoToTab(TabNumber, Option<ClientId>, Option<NotificationEnd>), // this Option is a hacky workaround, please do not copy this behaviour
    GoToTabName(
        String,
        Option<TerminalAction>, // default_shell
        bool,
        Option<ClientId>,
        Option<NotificationEnd>,
    ),
    ToggleTab(ClientId, Option<NotificationEnd>),
    UpdateTabName(Vec<u8>, ClientId, Option<NotificationEnd>),
    UndoRenameTab(ClientId, Option<NotificationEnd>),
    MoveTabLeft(ClientId, Option<NotificationEnd>),
    MoveTabRight(ClientId, Option<NotificationEnd>),
    GoToTabWithId(TabId, Option<ClientId>, Option<NotificationEnd>),
    CloseTabWithId(TabId, Option<NotificationEnd>),
    RenameTabWithId(TabId, Vec<u8>, Option<NotificationEnd>),
    ArrangeProjectPanes(Vec<yinz_types::data::ProjectPaneGroup>, ClientId),
    SetTabProjects(BTreeMap<TabId, String>),
    SetProjectContext(Vec<String>, Vec<(PaneId, String)>, BTreeMap<String, String>),
    BreakPanesToTabWithId {
        pane_ids: Vec<PaneId>,
        tab_id: TabId,
        should_change_focus_to_target_tab: bool,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    },
    RecomputeTabSize(ClientId, Size),
    TerminalPixelDimensions(ClientId, PixelDimensions),
    TerminalBackgroundColor(String),
    TerminalForegroundColor(String),
    TerminalColorRegisters(Vec<(usize, String)>),
    SetKittyGraphicsSupport {
        client_id: ClientId,
        supported: bool,
    },
    SetSixelSupport {
        client_id: ClientId,
        supported: bool,
    },
    /// Whether this client's host terminal presents each frame atomically (DECSET 2026).
    SetSynchronizedOutputSupport {
        client_id: ClientId,
        supported: bool,
    },
    /// A pane's Grid intercepted an app-in-pane whitelisted query; Screen
    /// assigns a token, queues the forward, and dispatches to the client.
    /// `query` carries the classified form so Screen can match on it
    /// directly (for cache-fallback synthesis) without re-parsing bytes.
    ForwardHostQuery {
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    },
    NestedSessionMessageFromPane {
        pane_id: PaneId,
        message: NestedSessionMessage,
    },
    NestedGuestPingTick {
        pane_id: PaneId,
    },
    NestedSessionMessageFromHost {
        client_id: ClientId,
        message: NestedSessionMessage,
    },
    GuestModalChoice {
        client_id: ClientId,
        pane_id: PaneId,
        outcome: GuestModalOutcome,
    },
    /// The client observed the host's reply to a previously forwarded
    /// query (closed by the Primary-DA barrier or the 500 ms timeout).
    /// The reply bytes are written verbatim to the originating pane.
    ForwardedReplyFromHost {
        token: u32,
        reply_bytes: Vec<u8>,
    },
    /// Internal: a forwarded reply (or its cache-fallback synthesis,
    /// or a locally-answered query like `ColorPaletteMode`) is ready
    /// to be delivered to the originating pane. Routed through the
    /// owning Tab so that the bytes are written to PTY *and* any
    /// PTY input that arrived while the pane was forward-paused is
    /// drained and re-fed through vte in stream order. This preserves
    /// query/reply ordering even when sync replies (DA1, DSR, DECQRM)
    /// would otherwise race ahead of an async host round-trip.
    ResumePaneAfterForward {
        pane_id: PaneId,
        reply_bytes: Vec<u8>,
    },
    /// The host terminal reported a color-palette theme mode (DSR 997 reply
    /// to `CSI ? 996 n`, or unsolicited notification while `CSI ? 2031 h`
    /// is enabled). Triggers auto-theme switch (when configured), the
    /// `HostTerminalThemeChanged` plugin event, and per-pane DSR forwarding
    /// for panes that opted in via `CSI ? 2031 h`.
    HostTerminalThemeChanged(HostTerminalThemeMode),
    PaletteThemeSaved {
        client: ClientId,
        request: uuid::Uuid,
        result: Result<(), String>,
    },
    /// Manual theme actions issued via the CLI (e.g. `yinz action set-dark-theme`)
    /// or a keybinding. They share the same convergence point as
    /// `HostTerminalThemeChanged`, but additionally surface a CLI-friendly error
    /// via `NotificationEnd` if `theme_dark` and `theme_light` are not both set
    /// (the auto-switch gate). "Last one wins": these compete naively with
    /// terminal-driven notifications via the dedupe in
    /// `update_host_terminal_theme_mode`.
    SetDarkTheme(Option<NotificationEnd>),
    SetLightTheme(Option<NotificationEnd>),
    ToggleTheme(Option<NotificationEnd>),
    ChangeMode(
        InputMode,
        Option<InputMode>,
        ClientId,
        Option<NotificationEnd>,
    ),
    ChangeModeForAllClients(InputMode, Option<InputMode>, Option<NotificationEnd>),
    MouseEvent(MouseEvent, ClientId, Option<NotificationEnd>),
    Copy(ClientId, Option<NotificationEnd>),
    AddClient(
        ClientId,
        Size,              // client viewport size
        Option<TabNumber>, // tab number to focus
        Option<PaneId>,    // pane to focus
    ),
    RemoveClient(ClientId),
    UpdateSearch(Vec<u8>, ClientId, Option<NotificationEnd>),
    SearchDown(ClientId, Option<NotificationEnd>),
    SearchUp(ClientId, Option<NotificationEnd>),
    SearchToggleCaseSensitivity(ClientId, Option<NotificationEnd>),
    SearchToggleWholeWord(ClientId, Option<NotificationEnd>),
    SearchToggleWrap(ClientId, Option<NotificationEnd>),
    AddRedPaneFrameColorOverride(Vec<PaneId>, Option<String>), // Option<String> => optional error text
    ClearPaneFrameColorOverride(Vec<PaneId>),
    SetTabBellFlash(TabId, bool), // tab_id, is_flashing
    /// The host terminal of `ClientId` gained (`true`) or lost (`false`) keyboard focus.
    HostTerminalFocusChanged(ClientId, bool),
    /// The whitelisted environment of a client's host terminal (`HOST_TERMINAL_ENV_VARS`), from
    /// which the notification protocol the host speaks is resolved.
    SetClientHostTerminalEnv(ClientId, BTreeMap<String, String>),
    /// Desktop notifications a pane asked for, to be rendered per client in the protocol that
    /// client's host speaks.
    ForwardDesktopNotifications {
        pane_id: TerminalId,
        notifications: Vec<PendingNotification>,
    },
    PreviousSwapLayout(ClientId, Option<NotificationEnd>),
    NextSwapLayout(ClientId, Option<NotificationEnd>),
    OverrideLayout(
        Option<PathBuf>,        // cwd (applies to all tabs)
        Option<TerminalAction>, // default_shell (applies to all tabs)
        Vec<TabLayoutInfo>,     // layouts for each tab to override
        bool,                   // retain_existing_terminal_panes
        bool,                   // retain_existing_plugin_panes
        bool,                   // apply_only_to_focused_tab
        ClientId,
        Option<NotificationEnd>,
    ),
    OverrideLayoutComplete(
        Vec<TabOverrideResult>, // results for each tab
        Vec<TabId>,             // tabs whose terminals could not be spawned
        Vec<TabId>,             // surplus tabs to close only after successful application
        bool,                   // retain_existing_terminal_panes
        bool,                   // retain_existing_plugin_panes
        ClientId,
        Option<NotificationEnd>,
    ),
    QueryTabNames(ClientId, Option<NotificationEnd>),
    NewTiledPluginPane(
        RunPluginOrAlias,
        Option<String>,
        bool,
        Option<PathBuf>,
        ClientId,
        Option<NotificationEnd>,
        Option<TabId>, // tab_id
        bool,
    ), // Option<String> is
    // optional pane title, bool is skip cache, Option<PathBuf> is an optional cwd
    NewFloatingPluginPane(
        RunPluginOrAlias,
        Option<String>,
        bool,
        Option<PathBuf>,
        // floating or overlay, carried whole so a modal's kind survives the trip
        NewPanePlacement,
        ClientId,
        Option<NotificationEnd>,
        Option<TabId>, // tab_id
        bool,
    ), // Option<String> is an
    // optional pane title, bool
    // is skip cache, Option<PathBuf> is an optional cwd
    NewInPlacePluginPane(
        RunPluginOrAlias,
        Option<String>,
        PaneId,
        bool,
        bool,
        ClientId,
        Option<NotificationEnd>,
        Option<TabId>, // tab_id
        bool,
    ), // Option<String> is an
    // optional pane title, first bool is skip cache, second bool is close_replaced_pane
    StartOrReloadPluginPane(
        RunPluginOrAlias,
        Option<String>,
        ClientId,
        Option<NotificationEnd>,
    ),
    AddPlugin(
        Option<bool>, // should_float
        bool,         // should be opened in place
        ReplacedPane, // what becomes of the pane it is opened in place of
        RunPluginOrAlias,
        Option<String>, // pane title
        Option<TabId>,  // tab index
        PluginId,       // plugin id
        Option<PaneId>,
        Option<PathBuf>, // cwd
        bool,            // start suppressed
        Option<FloatingPaneCoordinates>,
        Option<bool>, // should focus plugin
        Option<ClientId>,
        Option<NotificationEnd>, // completion signal
        // the overlay placement the launcher stated outright, rather than the one derived from
        // the plugin's own `overlay`/`modal` configuration
        Option<OverlayPlacement>,
    ),
    /// The plugin runs without a pane: it declares none. Any pane record for it is removed
    /// without unloading it.
    PluginPaneless {
        plugin_id: PluginId,
        run: Box<RunPluginOrAlias>,
        /// The launch that started it, already filled in: the screen answers it once it records
        /// the plugin as headless, so whatever the launcher does next finds it running. This
        /// instruction comes from the plugin thread and the launcher's next one from a client,
        /// on another channel, so only the screen can order the two.
        completion: Option<NotificationEnd>,
    },
    /// The plugin's declaration waits for its repository to resolve (a git plugin), which can
    /// take a while. Until `AddPlugin`, `PluginPaneless`, `PluginFailed` or `PluginUnloaded`
    /// says more, the screen knows it as loading: a launch of the same plugin finds this one
    /// instead of loading another, and closing it unloads it. The screen answers `completion`,
    /// the launch that started it, once it records the plugin.
    PluginResolving {
        plugin_id: PluginId,
        run: Box<RunPluginOrAlias>,
        completion: Option<NotificationEnd>,
    },
    /// The plugin's load failed, or it crashed, and it has no pane to show the failure in: it is
    /// not running, though it can be reloaded. A launch of the same plugin starts another.
    PluginFailed(PluginId),
    /// Client attachment restored an available instance; forget its previous failure.
    PluginRecovered(PluginId),
    /// The plugin was unloaded; forget everything the screen keeps for it.
    PluginUnloaded(PluginId),
    /// A pane held for a plugin that will not have one there (a multipane child that declares no
    /// pane or fails to load) is given up. A multipane member leaves its container, which keeps
    /// the sibling; any other pane closes as `ClosePane` closes it. Either way the pty frees what
    /// the slot held.
    GiveUpSlot(PaneId),
    UpdatePluginLocation(PluginId, yinz_types::input::layout::RunPluginLocation),
    UpdatePluginLoadingStage(PluginId, LoadingIndication), // u32 - plugin_id
    StartPluginLoadingIndication(PluginId, LoadingIndication), // u32 - plugin_id
    /// Start and stop taking loading-indicator frames from the shared animation clock for this
    /// plugin. The dots themselves are derived from elapsed time, so these only say *whether* the
    /// plugin is still loading.
    StartPluginLoadingAnimation(PluginId),
    StopPluginLoadingAnimation(PluginId), // u32 - plugin id
    RequestStateUpdateForPlugins,
    LaunchOrFocusPlugin(
        RunPluginOrAlias,
        bool,
        bool,
        bool,
        bool,
        Option<PaneId>,
        bool,
        ClientId,
        Option<NotificationEnd>,
        Option<TabId>, // tab_id
    ), // bools are: should_float, move_to_focused_tab, should_open_in_place, close_replaced_pane, Option<PaneId> is the pane id to replace, bool following it is skip_cache
    LaunchPlugin(
        RunPluginOrAlias,
        bool,
        bool,
        bool,
        Option<PaneId>,
        bool,
        Option<PathBuf>,
        ClientId,
        Option<NotificationEnd>,
        Option<TabId>, // tab_id
        bool,
        // stated by the launcher rather than derived from the plugin's own configuration
        Option<OverlayPlacement>,
    ), // bools are: should_float, should_open_in_place, close_replaced_pane, Option<PaneId> is the pane id to replace, Option<PathBuf> is an optional cwd, bool after is skip_cache
    SuppressPane(PaneId, ClientId),
    UnsuppressPane(PaneId, bool), // bool -> should float if hidden
    UnsuppressOrExpandPane(PaneId, bool), // bool -> should float if hidden
    FocusPaneWithId(PaneId, bool, bool, ClientId, Option<NotificationEnd>), // bools:
    // should_float_if_hidden,
    // should_be_in_place_if_hidden
    RenamePane(PaneId, Vec<u8>, Option<NotificationEnd>),
    RenameActivePane(Vec<u8>, ClientId, Option<NotificationEnd>),
    RenameTab(TabNumber, Vec<u8>, Option<NotificationEnd>),
    CancelPluginPermission(u64),
    RequestPluginPermissions(
        PluginId, // plugin_id
        PluginPermission,
    ),
    BreakPane(Option<TerminalAction>, ClientId, Option<NotificationEnd>),
    BreakPaneRight(ClientId, Option<NotificationEnd>),
    BreakPaneLeft(ClientId, Option<NotificationEnd>),
    /// Move a pane, running process and all, to another session.
    ///
    /// Every refusal happens here, before anything is released — a pane whose
    /// warp is going to fail should never leave its tab.
    WarpPane {
        supports_companions: bool,
        supports_held: bool,
        pane_id: Option<PaneId>,
        to_session: SessionName,
        placement: WarpPlacement,
        client_id: ClientId,
        response_channel: crate::warp::PaneWarpReply,
        /// When the caller stops waiting. See [`WarpTab`](Self::WarpTab).
        deadline: Instant,
    },
    /// The last [`PaneWarpHold`](crate::warp::PaneWarpHold) for a pane warp dropped.
    PaneWarpFinished,
    /// Queued behind the instructions that place an adopted pane. Checks that the
    /// pane exists, and releases the warp hold only after placement has run.
    AdoptedPaneFinished(crate::warp::AdoptedPaneFinish),
    /// The pty thread has let a pane's supervisor go. Serializing the pane *here*
    /// rather than when the warp was requested is what makes the screen that
    /// travels match the one the user was looking at: by now the reader task is
    /// gone, so every `PtyBytes` it ever emitted is queued ahead of this and has
    /// already been applied.
    PaneReleasedForWarp {
        terminal_id: TerminalId,
        to_session: SessionName,
        placement: WarpPlacement,
        client_id: ClientId,
        released: Result<ReleasedPaneForWarp, String>,
    },
    /// Nudge an adopted pane's process to repaint by briefly changing its size.
    RequestPaneRedraw(TerminalId),
    /// The pty thread has taken a supervisor over and this session now owns
    /// `terminal_id`. All that is left is to give it a pane and paint it.
    AdoptedPaneReady {
        hold: Option<crate::warp::PaneWarpHold>,
        /// The adopter's `placed` channel. See [`PtyInstruction::AdoptTerminal`].
        placed: crossbeam::channel::Sender<Result<()>>,
        terminal_id: TerminalId,
        payload: Box<WarpedPane>,
        placement: WarpPlacement,
        client_id: ClientId,
        cmd: RunCommand,
        buffered: Vec<u8>,
        child_exit_status: Option<Option<i32>>,
    },
    /// Move a whole tab, arrangement and running processes and all, to another
    /// session. The tab analogue of `WarpPane`, with the same rule: every
    /// refusal happens here, before a single supervisor is released.
    WarpTab {
        supports_companions: bool,
        supports_held: bool,
        tab_id: Option<TabId>,
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Result<Box<WarpedTab>, String>>,
        /// The moment the caller stops waiting for `response_channel`.
        ///
        /// Passed down to the pty thread rather than kept here, because the
        /// part of a warp that can outrun the caller is the release, not
        /// anything Screen does. What Screen keeps back is a slice of it for
        /// its own serialization work — see `TAB_WARP_SCREEN_RESERVE`.
        deadline: Instant,
    },
    /// The pty thread has let go of every supervisor in a tab. Serializing the
    /// tab happens here for the same reason it does for a single pane: by now
    /// the reader tasks are gone, so all their output is queued ahead of this.
    TabReleasedForWarp {
        tab_index: TabId,
        released: Vec<(TerminalId, Result<ReleasedPaneForWarp, String>)>,
    },
    /// The pty thread has taken a tab's supervisors over and this session now
    /// owns their terminal ids. All that is left is to build the tab around them.
    AdoptedTabReady {
        is_rollback: bool,
        response_channel: Option<crossbeam::channel::Sender<Result<()>>>,
        hold: Option<crate::warp::PaneWarpHold>,
        placement: Option<yinz_types::data::WarpPlacement>,
        payload: Box<WarpedTab>,
        adopted: Box<AdoptedTabPanes>,
        client_id: ClientId,
    },
    UpdateSessionInfos(
        BTreeMap<SessionName, SessionInfo>, // String is the session name
        BTreeMap<SessionName, Duration>,    // resurrectable sessions - <name, created>
    ),
    ReplacePane(
        PaneId,
        HoldForCommand,
        Option<InitialTitle>,
        Option<Run>,
        ReplacedPane,
        ClientTabIndexOrPaneId,
        Option<NotificationEnd>, // completion signal
    ),
    /// A plugin asked to open another plugin in place of `pane_id`, which lends it its slot.
    /// The screen refuses a target that holds no slot before anything is loaded, and hands
    /// the rest to the plugin thread, which refuses an instance that already exists.
    OpenPluginPaneInPlaceOfPaneId {
        pane_id: PaneId,
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        cwd: PathBuf,
        client_id: ClientId,
        completion: NotificationEnd,
    },
    SerializeLayoutForResurrection,
    RenameSession(SessionName, ClientId, Option<NotificationEnd>), // String -> new name
    Notify(
        String,
        Option<PaneId>,
        Option<NotificationClick>,
        Option<NotificationEnd>,
    ), // message, click target, what to tell the raising plugin on click, completion
    PluginNotify {
        text: String,
        pane_id: Option<PaneId>,
        on_click: Option<NotificationClick>,
        plugin_id: PluginId,
        completion: Option<NotificationEnd>,
    },
    NotificationPolicy(Option<PluginId>, Option<String>),
    NotificationPolicyUnavailable {
        plugin_id: PluginId,
        reason: String,
        disable: bool,
    },
    BindNotificationRequest {
        id: NotificationId,
        plugin_id: PluginId,
        instance_id: crate::plugins::PluginInstanceId,
    },
    ResolveNotification {
        id: NotificationId,
        decision: yinz_types::notifications::NotificationDecision,
        plugin_id: PluginId,
        instance_id: crate::plugins::PluginInstanceId,
    },
    DismissNotification(NotificationId), // u64 -> the notification's id
    AnimationTick,
    /// A context menu's hook plugin did not answer in time; show the configured items.
    ContextMenuTimedOut(ContextMenuRequestId, ClientId), // u64 -> the menu's request id
    /// The hook plugin's answer for the menu identified by the request id.
    SetContextMenu {
        request_id: ContextMenuRequestId,
        items: Vec<ContextMenuItem>,
        /// The plugin that answered, so an item's echoed context goes back to it and to nothing
        /// else. `None` when the plugin thread answered from the configuration - no plugin was
        /// involved, and configured items carry no context to echo.
        plugin_id: Option<PluginId>,
        client_id: ClientId,
    },
    /// Run a context menu item's actions against the pane its menu was opened over.
    ///
    /// Enqueued by `Screen::activate_context_menu_item` rather than run inline: a focus-changing
    /// instruction already sitting in the queue behind the click that activated the item would
    /// otherwise be processed *after* an inline focus restoration and *before* the actions - which
    /// are dispatched from a spawned thread - putting the two on either side of it.
    ActivateCommandPaletteAction {
        label: String,
        pane_id: PaneId,
        tab_id: TabId,
        action: Action,
        client_id: ClientId,
    },
    MoveCommandPalettePane {
        pane_id: PaneId,
        tab_id: TabId,
        direction: Option<Direction>,
        client_id: ClientId,
    },
    ActivateContextMenuItem {
        pane_id: PaneId,
        actions: Vec<Action>,
        client_id: ClientId,
    },
    /// Dismiss this client's context menu, if it has one.
    ///
    /// Sent from `route.rs` for a key bound to something other than writing to the pane: those
    /// actions become their own screen instructions and never reach `WriteCharacter`, where the
    /// menu otherwise gets out of the way. Without this a tab switch or a focus move leaves a menu
    /// painted over the state it was describing, still offering to act on the pane it was opened
    /// over.
    CloseContextMenu(ClientId),
    ResetAgentEscape(ClientId),
    ListClientsMetadata(Option<PathBuf>, ClientId, Option<NotificationEnd>), // Option<PathBuf> - default shell
    ListPanes {
        show_all: bool,
        response_channel: crossbeam::channel::Sender<ListPanesResponse>,
    },
    ListNotifications {
        response_channel: crossbeam::channel::Sender<Vec<NotificationInfo>>,
    },
    ListTabs {
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<ListTabsResponse>,
    },
    GetCurrentTabInfo {
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Option<TabInfo>>,
    },
    /// A config reload. Everything the screen takes from the options travels in `config`;
    /// the keybinds are separate because they go into the per-client mode info, not the screen.
    Reconfigure {
        client_id: ClientId,
        keybinds: Keybinds,
        config: ScreenConfig,
    },
    RerunCommandPane(TerminalId, Option<NotificationEnd>), // u32 - terminal pane id
    RestartAgentPane {
        pane_id: PaneId,
        prompt: Option<String>,
        session: yinz_types::agent::control::AgentRestartSession,
        originating_plugin: yinz_types::data::OriginatingPlugin,
        response:
            std::sync::mpsc::Sender<Result<yinz_types::agent::control::RestartedAgent, String>>,
    },
    RunCommandInPane(TerminalId, RunCommand, bool), // u32 - terminal pane id, bool - keep_scrollback;
    // runs a *different* command in an existing command pane
    ResizePaneWithId(ResizeStrategy, PaneId),
    EditScrollbackForPaneWithId(PaneId, Option<NotificationEnd>),
    WriteToPaneId(Vec<u8>, PaneId, Option<NotificationEnd>),
    Paste(Vec<u8>, Option<PaneId>, ClientId, Option<NotificationEnd>),
    SetPaneColor(
        PaneId,
        Option<String>,
        Option<String>,
        Option<NotificationEnd>,
    ),
    WriteKeyToPaneId(
        Option<KeyWithModifier>,
        Vec<u8>,
        bool, // is_kitty_keyboard_protocol
        PaneId,
        Option<NotificationEnd>,
    ),
    CopyTextToClipboard(String, PluginId), // String - text to copy, u32 - plugin_id
    MovePaneWithPaneId(PaneId),
    MovePaneWithPaneIdInDirection(PaneId, Direction),
    ClearScreenForPaneId(PaneId),
    ScrollUpInPaneId(PaneId),
    ScrollDownInPaneId(PaneId),
    ScrollToTopInPaneId(PaneId),
    ScrollToBottomInPaneId(PaneId),
    PageScrollUpInPaneId(PaneId),
    PageScrollDownInPaneId(PaneId),
    TogglePaneIdFullscreen(PaneId),
    TogglePaneEmbedOrEjectForPaneId(PaneId),
    CloseTabWithIndex(TabPosition),
    BreakPanesToNewTab {
        pane_ids: Vec<PaneId>,
        default_shell: Option<TerminalAction>,
        should_change_focus_to_new_tab: bool,
        new_tab_name: Option<String>,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    },
    BreakPanesToTabWithIndex {
        pane_ids: Vec<PaneId>,
        tab_index: TabPosition,
        should_change_focus_to_new_tab: bool,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    },
    ListClientsToPlugin(PluginId, ClientId, Option<u64>),
    TogglePanePinned(ClientId, Option<NotificationEnd>),
    SetFloatingPanePinned(PaneId, bool),
    StackPanes(Vec<PaneId>, ClientId, Option<NotificationEnd>),
    PaneTabsAction(i8, ClientId, Option<NotificationEnd>),
    PaneTabs {
        command: yinz_types::pane_tabs::PaneTabsCommand,
        client_id: ClientId,
        response: crossbeam::channel::Sender<yinz_types::pane_tabs::PaneTabsResponse>,
    },
    OverlayTabCwd {
        id: yinz_types::pane_tabs::PaneTabsId,
        cwd: Option<PathBuf>,
        caller_cwd: Option<PathBuf>,
        caller_terminal_pane_id: Option<TerminalId>,
    },
    PaneTabLoaded {
        id: yinz_types::pane_tabs::PaneTabsId,
        index: usize,
        generation: u64,
        result: Result<(PluginId, RunPluginOrAlias), String>,
        client_id: ClientId,
    },
    Multipane {
        operation: yinz_types::multipane::MultipaneOperation,
        client_id: ClientId,
        response: crossbeam::channel::Sender<yinz_types::multipane::MultipaneResponse>,
    },
    /// The `bool` says the placement is a user gesture made through the plugin, and so may
    /// reclaim a position the user dragged, though not a size they resized - see
    /// `OverlayPanes::change_pane_coordinates`.
    ChangeOverlayCoordinates(
        Vec<(PaneId, FloatingPaneCoordinates)>,
        bool,
        Option<NotificationEnd>,
    ),
    ChangeFloatingPanesCoordinates(
        Vec<(PaneId, FloatingPaneCoordinates)>,
        Option<NotificationEnd>,
    ),
    TogglePaneBorderless(PaneId, Option<NotificationEnd>),
    SetPaneBorderless(PaneId, bool, Option<NotificationEnd>),
    AddHighlightPaneFrameColorOverride(Vec<PaneId>, Option<String>),
    /// A bell flash: the highlight colour, easing itself back out rather than waiting to be
    /// cleared. Distinct from the plugin highlight above, which stays up until something takes it
    /// down and so must not fade.
    FlashPaneFrameColor(Vec<PaneId>), // Option<String> => optional
    // message
    GroupAndUngroupPanes(Vec<PaneId>, Vec<PaneId>, bool, ClientId), // panes_to_group, panes_to_ungroup, bool -> for all clients
    HighlightAndUnhighlightPanes(Vec<PaneId>, Vec<PaneId>, ClientId), // panes_to_highlight, panes_to_unhighlight
    /// Show a client a pane and mark it, without moving that client's keyboard
    /// focus. `Option<String>` is a label drawn in place of the pane's title.
    RevealPaneWithId(PaneId, Option<String>, ClientId),
    FloatMultiplePanes(Vec<PaneId>, ClientId),
    EmbedMultiplePanes(Vec<PaneId>, ClientId),
    TogglePaneInGroup(ClientId, Option<NotificationEnd>),
    ToggleGroupMarking(ClientId, Option<NotificationEnd>),
    SetMouseSelectionSupport(PaneId, bool),
    InterceptKeyPresses(PluginId, ClientId),
    /// Drop `client_id`'s intercept if it belongs to this plugin. Plugin-scoped because the
    /// unload path sends it for every plugin, and one client has at most one intercepting
    /// plugin at a time: an unscoped clear would drop another plugin's.
    ClearKeyPressesIntercepts(PluginId, ClientId),
    ReplacePaneWithExistingPane(PaneId, PaneId, bool, Option<NotificationEnd>), // bool -> suppress_replaced_pane
    AddWatcherClient(ClientId, Size),
    RemoveWatcherClient(ClientId),
    SetFollowedClient(ClientId),
    WatcherTerminalResize(ClientId, Size),
    ClearMouseHelpText(ClientId),
    UpdateAvailableLayouts {
        generation: u64,
        layouts: Vec<LayoutInfo>,
        errors: Vec<LayoutWithError>,
    },
    SetPluginRegexHighlights {
        pane_id: PaneId,
        plugin_id: PluginId,
        highlights: Vec<RegexHighlight>,
    },
    ClearPluginHighlights {
        pane_id: PaneId,
        plugin_id: PluginId,
    },
    ClearAllPluginHighlights(PluginId), // plugin_id — clears across all panes
    SubscribeToPaneRenders {
        client_id: ClientId,
        pane_ids: Vec<yinz_types::data::PaneId>,
        scrollback: Option<usize>,
        ansi: bool,
    },
    NotifyPaneClosedToSubscribers {
        pane_id: yinz_types::data::PaneId,
    },
    InvalidateAgentEventProjects,
    AgentEventProjects(u64, AgentEventProjectMemberships),
    SubscribeAgentEvents(ClientId, yinz_ipc::ipc::EventSubscription),
    SetCliEventSubscribersPresent(bool),
    AgentControl(yinz_types::agent::control::AgentRequest, ClientId),
    AgentControlReply(
        yinz_types::agent::control::AgentRequest,
        std::sync::mpsc::Sender<yinz_types::agent::control::AgentResponse>,
    ),
    ResolveAgentTab {
        placement: yinz_types::agent::control::AgentPlacement,
        invoked_with: Option<Run>,
        client_id: ClientId,
        reply: std::sync::mpsc::Sender<Result<crate::route::AgentTabPlacement, String>>,
    },
    CleanupAgentTab(
        crate::route::AgentTabPlacement,
        std::sync::mpsc::Sender<bool>,
    ),
    AgentSpawned(yinz_types::agent::SpawnedAgent),
    AgentReport(yinz_types::agent::report::AgentReport, std::time::Instant),
    RestoreWarpedAgentInput(PaneId, yinz_types::data::WarpedAgentInput),
    SetPaneAgent(PaneId, Option<AgentInfo>, Option<NotificationEnd>),
    AgentWorktreeResolved {
        cwd: PathBuf,
        generation: u64,
        root: Option<PathBuf>,
    },
    /// Set (or clear) the run id on a pane's existing agent record, leaving every field the
    /// agent itself reports alone. See `AgentInfo::run_id`.
    SetPaneAgentRun(PaneId, Option<String>, Option<NotificationEnd>),
    /// What a plugin wants said, and decided, when the session is about to end. Keyed by the
    /// plugin's id; see `PluginCommand::SetQuitNotice`.
    SetQuitNotice(PluginId, Option<String>, bool),
    /// Raise the quit confirmation as a modal, carrying whatever the plugins asked to have said.
    ///
    /// The one place the confirmation is built. `route.rs` sends this rather than the
    /// `LaunchPlugin` it used to build itself, because the notices live on the screen thread.
    ConfirmQuit(ClientId, Option<NotificationEnd>),
    DesktopNotificationResponse(Vec<u8>, ClientId),
    UpdateBackgroundPluginSubscriptions(PluginId, ClientId, HashSet<EventType>),
    ClearHintTextCache,
    BroadcastModeUpdate(ModeInfo, Option<ClientId>), // ModeInfo, optional specific client_id (None = all clients)
    // Pane-targeting CLI variants
    ScrollUpWithPaneId(PaneId, Option<NotificationEnd>),
    ScrollDownWithPaneId(PaneId, Option<NotificationEnd>),
    ScrollToTopWithPaneId(PaneId, Option<NotificationEnd>),
    ScrollToBottomWithPaneId(PaneId, Option<NotificationEnd>),
    PageScrollUpWithPaneId(PaneId, Option<NotificationEnd>),
    PageScrollDownWithPaneId(PaneId, Option<NotificationEnd>),
    HalfPageScrollUpWithPaneId(PaneId, Option<NotificationEnd>),
    HalfPageScrollDownWithPaneId(PaneId, Option<NotificationEnd>),
    ResizeWithPaneId(PaneId, ResizeStrategy, Option<NotificationEnd>),
    MovePaneWithPaneIdCli(PaneId, Option<Direction>, Option<NotificationEnd>),
    MovePaneBackwardsWithPaneId(PaneId, Option<NotificationEnd>),
    ClearScreenWithPaneId(PaneId, Option<NotificationEnd>),
    EditScrollbackWithPaneId(PaneId, bool, Option<NotificationEnd>),
    ToggleFullscreenWithPaneId(PaneId, Option<NotificationEnd>),
    ToggleNoUiFullscreenWithPaneId(PaneId, Option<NotificationEnd>),
    TogglePaneEmbedOrFloatingWithPaneId(PaneId, Option<NotificationEnd>),
    CloseFocusWithPaneId(PaneId, Option<NotificationEnd>),
    RenamePaneWithPaneId(PaneId, Vec<u8>, Option<NotificationEnd>),
    UndoRenamePaneWithPaneId(PaneId, Option<NotificationEnd>),
    TogglePanePinnedWithPaneId(PaneId, Option<NotificationEnd>),
    // Tab-targeting CLI variants
    UndoRenameTabWithTabId(TabId, Option<NotificationEnd>),
    ToggleActiveSyncTabWithTabId(TabId, Option<NotificationEnd>),
    ToggleFloatingPanesWithTabId(TabId, Option<TerminalAction>, Option<NotificationEnd>),
    PreviousSwapLayoutWithTabId(TabId, Option<NotificationEnd>),
    NextSwapLayoutWithTabId(TabId, Option<NotificationEnd>),
    MoveTabWithTabId(TabId, Direction, Option<NotificationEnd>),
    FocusHostSession(ClientId, Option<NotificationEnd>),
    FocusGuestSession(ClientId, Option<NotificationEnd>),
    ToggleHostFullscreen(ClientId, Option<NotificationEnd>),
}

impl From<&ScreenInstruction> for ScreenContext {
    fn from(screen_instruction: &ScreenInstruction) -> Self {
        // The two enums are kept name-for-name deliberately, so all but a handful of the 250-odd
        // variants resolve by name. `unit/instruction_context_tests.rs` fails the test run if a
        // new instruction is added without either a matching `ScreenContext` variant or an arm
        // here, which is a stronger guarantee than the exhaustive match this replaced: it also
        // catches the two names silently drifting apart.
        match screen_instruction {
            ScreenInstruction::PaletteThemeSaved { .. } => ScreenContext::ToggleCommandPalette,
            ScreenInstruction::CommandPalette(..) => ScreenContext::ToggleCommandPalette,
            // The one instruction whose context is chosen by payload rather than by name.
            ScreenInstruction::Resize(.., strategy, _) => match strategy {
                ResizeStrategy {
                    resize: Resize::Increase,
                    direction,
                    ..
                } => match direction {
                    Some(Direction::Left) => ScreenContext::ResizeIncreaseLeft,
                    Some(Direction::Down) => ScreenContext::ResizeIncreaseDown,
                    Some(Direction::Up) => ScreenContext::ResizeIncreaseUp,
                    Some(Direction::Right) => ScreenContext::ResizeIncreaseRight,
                    None => ScreenContext::ResizeIncreaseAll,
                },
                ResizeStrategy {
                    resize: Resize::Decrease,
                    direction,
                    ..
                } => match direction {
                    Some(Direction::Left) => ScreenContext::ResizeDecreaseLeft,
                    Some(Direction::Down) => ScreenContext::ResizeDecreaseDown,
                    Some(Direction::Up) => ScreenContext::ResizeDecreaseUp,
                    Some(Direction::Right) => ScreenContext::ResizeDecreaseRight,
                    None => ScreenContext::ResizeDecreaseAll,
                },
            },
            // Instructions whose context is named differently from the instruction itself.
            ScreenInstruction::SubscribePaneOutput(..)
            | ScreenInstruction::UnsubscribePaneOutput(..)
            | ScreenInstruction::PtyBytes(..)
            | ScreenInstruction::RemotePaneReady(..) => ScreenContext::HandlePtyBytes,
            ScreenInstruction::RenderWriterReady => ScreenContext::RenderToClients,
            ScreenInstruction::AgentWorktreeResolved { .. } => ScreenContext::RenderToClients,
            ScreenInstruction::ClearAllPluginHighlights(..) => ScreenContext::ClearPluginHighlights,
            other => {
                let name: &'static str = other.into();
                ScreenContext::from_str(name).unwrap_or_else(|_| {
                    // the guard tests make this unreachable in a tested build; in a release build
                    // a missing name should not take the session down over what is only an
                    // error-context label.
                    debug_assert!(false, "no ScreenContext variant named {name}");
                    ScreenContext::Exit
                })
            },
        }
    }
}

pub type InitialTitle = String;

pub type HoldForCommand = Option<RunCommand>;

/// Result of overriding a single tab's layout
#[derive(Debug, Clone)]
pub struct TabOverrideResult {
    pub tab_index: TabId,
    pub tab_name: Option<String>,
    pub tiled_layout: TiledPaneLayout,
    pub floating_layouts: Vec<FloatingPaneLayout>,
    pub swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
    pub swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
    pub new_terminal_pids: Vec<(TerminalId, HoldForCommand)>,
    pub new_floating_pane_pids: Vec<(TerminalId, HoldForCommand)>,
    pub plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestModalOutcome {
    Zoom,
    Descend,
}

/// What to tell a toast's own plugin when somebody clicks it, and which plugin to tell.
///
/// Focusing the toast's `pane_id` says *where* the click meant; this says *what*. A plugin cannot
/// otherwise tell a click on its toast from any other route into its pane, and the two mean
/// different things - see `handle_pane_notification_click`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationClick {
    pub plugin_id: PluginId,
    /// The client whose instance raised the toast - the key it is stored under in the plugin map,
    /// not whoever is acting on it now. A per-client plugin has an instance per client and only
    /// this one knows what the toast was about, so the click has to be able to name it however
    /// long the toast stands and whoever ends up clicking it.
    pub client_id: ClientId,
    pub message: String,
    pub payload: String,
}
