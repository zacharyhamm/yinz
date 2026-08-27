use zellij_utils::errors::prelude::*;

use super::{GuestModalOutcome, HoldForCommand, InitialTitle, TabOverrideResult};
use crate::panes::PaneId;
use crate::plugins::{DumpSessionLayoutResponse, PluginId, PluginRenderAsset};
use crate::pty::{AdoptedTabPanes, ClientTabIndexOrPaneId, VteBytes};
use crate::pty_host::client::ReleasedPane;
use crate::route::NotificationEnd;
use crate::ui::loading_indication::LoadingIndication;
use crate::ClientId;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use zellij_utils::data::{
    CommandOrPlugin, ContextMenuItem, Direction, EventType, FloatingPaneCoordinates,
    GetFocusedPaneInfoResponse, HostTerminalThemeMode, InputMode, KeyWithModifier, LayoutInfo,
    LayoutWithError, ListPanesResponse, ListTabsResponse, ModeInfo, NewPanePlacement,
    NotificationInfo, OverlayPlacement, PaneInfo, PaneScrollbackResponse, PluginPermission,
    RegexHighlight, Resize, ResizeStrategy, SessionInfo, Styling, TabInfo, WarpPlacement,
    WarpedPane, WarpedTab,
};
use zellij_utils::errors::ScreenContext;
use zellij_utils::input::actions::Action;
use zellij_utils::input::command::{RunCommand, TerminalAction};
use zellij_utils::input::context_menu::ContextMenu;
use zellij_utils::input::keybinds::Keybinds;
use zellij_utils::input::layout::{
    FloatingPaneLayout, Run, RunPluginOrAlias, SwapFloatingLayout, SwapTiledLayout, TabLayoutInfo,
    TiledPaneLayout,
};
use zellij_utils::input::mouse::MouseEvent;
use zellij_utils::input::options::{Clipboard, NestedSessionHandling, PaneFrameStyle};
use zellij_utils::ipc::PixelDimensions;
use zellij_utils::nested_session::NestedSessionMessage;
use zellij_utils::pane_size::Size;
use zellij_utils::position::Position;

/// Instructions that can be sent to the [`Screen`].
///
/// `IntoStaticStr` yields the variant name, which is what `From<&ScreenInstruction> for
/// ScreenContext` maps through; `VariantNames` lets `unit/instruction_context_tests.rs` prove
/// that mapping is total.
#[derive(Debug, Clone, strum_macros::IntoStaticStr, strum_macros::VariantNames)]
pub enum ScreenInstruction {
    PtyBytes(u32, VteBytes),
    PluginBytes(Vec<PluginRenderAsset>),
    Render,
    RenderToClients,
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
        tab_id: Option<usize>,
        completion: Option<NotificationEnd>,
    },
    HideFloatingPanes {
        client_id: ClientId,
        tab_id: Option<usize>,
        completion: Option<NotificationEnd>,
    },
    AreFloatingPanesVisible {
        client_id: ClientId,
        tab_id: Option<usize>,
        completion: Option<NotificationEnd>,
    },
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
        tab_index: Option<usize>,
        response_channel: crossbeam::channel::Sender<DumpSessionLayoutResponse>,
    },
    GetFocusedPaneInfo {
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<GetFocusedPaneInfoResponse>,
    },
    GetPaneInfo {
        pane_id: PaneId,
        response_channel: crossbeam::channel::Sender<Option<PaneInfo>>,
    },
    GetTabInfo {
        tab_id: usize,
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
    PageScrollUp(ClientId, Option<NotificationEnd>),
    PageScrollDown(ClientId, Option<NotificationEnd>),
    HalfPageScrollUp(ClientId, Option<NotificationEnd>),
    HalfPageScrollDown(ClientId, Option<NotificationEnd>),
    ClearScroll(ClientId),
    CloseFocusedPane(ClientId, Option<NotificationEnd>),
    ToggleActiveTerminalFullscreen(ClientId, Option<NotificationEnd>),
    ToggleActiveTerminalNoUiFullscreen(ClientId, Option<NotificationEnd>),
    TogglePaneFrames(Option<NotificationEnd>),
    SetPaneFrameStyle(PaneFrameStyle, Option<NotificationEnd>),
    SetSelectable(PaneId, bool),
    ShowPluginCursor(u32, ClientId, Option<(usize, usize)>),
    ClosePane(
        PaneId,
        Option<ClientId>,
        Option<NotificationEnd>,
        Option<i32>,
    ), // i32 -> optional exit
    // status
    HoldPane(PaneId, Option<i32>, RunCommand),
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
    UpdatePaneCwd(u32, Option<PathBuf>),
    /// The machine a pane's process runs on, for panes that are not local.
    /// Sent once when the pane is created and once when it closes.
    UpdatePaneHost(u32, Option<String>),
    /// A decoration to compose in front of a pane's title, or `None` to drop it.
    ///
    /// Separate from every rename instruction on purpose: this does not touch the
    /// pane's name, so a rename cannot erase it and it cannot erase a rename. See
    /// `Pane::set_title_prefix`.
    SetPaneTitlePrefix(PaneId, Option<String>),
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
        (ClientId, bool),             // bool -> is_web_client
        Option<NotificationEnd>,      // completion signal
    ),
    /// Apply layout to tab with given stable ID.
    ///
    /// The sixth parameter (usize) is a stable identifier (not position) from the
    /// NewTab → ApplyLayout async flow that passes IDs between threads.
    ApplyLayout(
        TiledPaneLayout,
        Vec<FloatingPaneLayout>,
        Vec<(u32, HoldForCommand)>, // new pane pids
        Vec<(u32, HoldForCommand)>, // new floating pane pids
        HashMap<RunPluginOrAlias, Vec<u32>>,
        usize,                          // tab_id - stable identifier from NewTab instruction
        bool,                           // should change focus to new tab
        (ClientId, bool),               // bool -> is_web_client
        Option<NotificationEnd>,        // regular completion signal
        Option<(u32, NotificationEnd)>, // blocking_terminal (terminal_id, completion_tx)
    ),
    SwitchTabNext(ClientId, Option<NotificationEnd>),
    SwitchTabPrev(ClientId, Option<NotificationEnd>),
    ToggleActiveSyncTab(ClientId, Option<NotificationEnd>),
    CloseTab(ClientId, Option<NotificationEnd>),
    GoToTab(u32, Option<ClientId>, Option<NotificationEnd>), // this Option is a hacky workaround, please do not copy this behaviour
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
    GoToTabWithId(usize, Option<ClientId>, Option<NotificationEnd>),
    CloseTabWithId(usize, Option<NotificationEnd>),
    RenameTabWithId(usize, Vec<u8>, Option<NotificationEnd>),
    SetTabProjects(BTreeMap<usize, String>),
    BreakPanesToTabWithId {
        pane_ids: Vec<PaneId>,
        tab_id: usize,
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
    /// Manual theme actions issued via the CLI (e.g. `zellij action set-dark-theme`)
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
        bool,                // is_web_client
        Size,                // client viewport size
        Option<usize>,       // tab position to focus
        Option<(u32, bool)>, // (pane_id, is_plugin) => pane_id to focus
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
    SetTabBellFlash(usize, bool), // tab_id, is_flashing
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
        Option<usize>, // tab_id
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
        Option<usize>, // tab_id
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
        Option<usize>, // tab_id
        bool,
    ), // Option<String> is an
    // optional pane title, first bool is skip cache, second bool is close_replaced_pane
    StartOrReloadPluginPane(RunPluginOrAlias, Option<String>, Option<NotificationEnd>),
    AddPlugin(
        Option<bool>, // should_float
        bool,         // should be opened in place
        bool,         // close_replaced_pane
        RunPluginOrAlias,
        Option<String>, // pane title
        Option<usize>,  // tab index
        u32,            // plugin id
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
    UpdatePluginLoadingStage(u32, LoadingIndication), // u32 - plugin_id
    StartPluginLoadingIndication(u32, LoadingIndication), // u32 - plugin_id
    /// Start and stop taking loading-indicator frames from the shared animation clock for this
    /// plugin. The dots themselves are derived from elapsed time, so these only say *whether* the
    /// plugin is still loading.
    StartPluginLoadingAnimation(u32),
    StopPluginLoadingAnimation(u32), // u32 - plugin id
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
        Option<usize>, // tab_id
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
        Option<usize>, // tab_id
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
    RenameTab(usize, Vec<u8>, Option<NotificationEnd>),
    RequestPluginPermissions(
        u32, // u32 - plugin_id
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
        pane_id: Option<PaneId>,
        to_session: String,
        placement: WarpPlacement,
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Result<Box<WarpedPane>, String>>,
    },
    /// The pty thread has let a pane's supervisor go. Serializing the pane *here*
    /// rather than when the warp was requested is what makes the screen that
    /// travels match the one the user was looking at: by now the reader task is
    /// gone, so every `PtyBytes` it ever emitted is queued ahead of this and has
    /// already been applied.
    PaneReleasedForWarp {
        terminal_id: u32,
        to_session: String,
        placement: WarpPlacement,
        client_id: ClientId,
        released: Result<ReleasedPane, String>,
    },
    /// The pty thread has taken a supervisor over and this session now owns
    /// `terminal_id`. All that is left is to give it a pane and paint it.
    AdoptedPaneReady {
        terminal_id: u32,
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
        tab_index: Option<usize>,
        client_id: ClientId,
        response_channel: crossbeam::channel::Sender<Result<Box<WarpedTab>, String>>,
    },
    /// The pty thread has let go of every supervisor in a tab. Serializing the
    /// tab happens here for the same reason it does for a single pane: by now
    /// the reader tasks are gone, so all their output is queued ahead of this.
    TabReleasedForWarp {
        tab_index: usize,
        released: Vec<(u32, Result<ReleasedPane, String>)>,
    },
    /// The pty thread has taken a tab's supervisors over and this session now
    /// owns their terminal ids. All that is left is to build the tab around them.
    AdoptedTabReady {
        payload: Box<WarpedTab>,
        adopted: Box<AdoptedTabPanes>,
        client_id: ClientId,
    },
    UpdateSessionInfos(
        BTreeMap<String, SessionInfo>, // String is the session name
        BTreeMap<String, Duration>,    // resurrectable sessions - <name, created>
    ),
    ReplacePane(
        PaneId,
        HoldForCommand,
        Option<InitialTitle>,
        Option<Run>,
        bool, // close replaced pane
        ClientTabIndexOrPaneId,
        Option<NotificationEnd>, // completion signal
    ),
    SerializeLayoutForResurrection,
    RenameSession(String, ClientId, Option<NotificationEnd>), // String -> new name
    Notify(String, Option<PaneId>, Option<NotificationEnd>),  // message, click target, completion
    DismissNotification(u64),                                 // u64 -> the notification's id
    AnimationTick,
    /// A context menu's hook plugin did not answer in time; show the configured items.
    ContextMenuTimedOut(u64, ClientId), // u64 -> the menu's request id
    /// The hook plugin's answer for the menu identified by the request id.
    SetContextMenu {
        request_id: u64,
        items: Vec<ContextMenuItem>,
        /// The plugin that answered, so an item's echoed context goes back to it and to nothing
        /// else. `None` when the plugin thread answered from the configuration - no plugin was
        /// involved, and configured items carry no context to echo.
        plugin_id: Option<u32>,
        client_id: ClientId,
    },
    /// Run a context menu item's actions against the pane its menu was opened over.
    ///
    /// Enqueued by `Screen::activate_context_menu_item` rather than run inline: a focus-changing
    /// instruction already sitting in the queue behind the click that activated the item would
    /// otherwise be processed *after* an inline focus restoration and *before* the actions - which
    /// are dispatched from a spawned thread - putting the two on either side of it.
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
    Reconfigure {
        client_id: ClientId,
        keybinds: Keybinds,
        default_mode: InputMode,
        theme: Styling,
        /// Resolved styling for `theme_dark`. When both this and
        /// `host_theme_light` are `Some`, Screen auto-switches the active
        /// palette in response to host CSI 2031 / DSR 997 notifications.
        host_theme_dark: Option<Styling>,
        /// Resolved styling for `theme_light`. See `host_theme_dark`.
        host_theme_light: Option<Styling>,
        simplified_ui: bool,
        default_shell: Option<PathBuf>,
        pane_frame_style: PaneFrameStyle,
        copy_command: Option<String>,
        copy_to_clipboard: Option<Clipboard>,
        copy_on_select: bool,
        auto_layout: bool,
        rounded_corners: bool,
        hide_session_name: bool,
        stacked_resize: bool,
        stacked_pane_list: bool,
        default_editor: Option<PathBuf>,
        advanced_mouse_actions: bool,
        mouse_scroll_resize: bool,
        mouse_hover_effects: bool,
        visual_bell: bool,
        focus_follows_mouse: bool,
        mouse_click_through: bool,
        context_menu_on_right_click: bool,
        /// The items a right-click offers. Held by `Screen` rather than re-read per click, so a
        /// reload has to hand the new list over or the session keeps the one it started with.
        context_menu: ContextMenu,
        osc133_command_selection: bool,
        word_separators: String,
        nested_session_handling: NestedSessionHandling,
        dangerously_enable_paste_buffer_read: bool,
    },
    RerunCommandPane(u32, Option<NotificationEnd>), // u32 - terminal pane id
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
    CopyTextToClipboard(String, u32), // String - text to copy, u32 - plugin_id
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
    SetMobileRenderPreferences {
        client_id: ClientId,
        single_pane: bool,
        fit: bool,
    },
    TogglePaneEmbedOrEjectForPaneId(PaneId),
    CloseTabWithIndex(usize),
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
        tab_index: usize,
        should_change_focus_to_new_tab: bool,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    },
    ListClientsToPlugin(PluginId, ClientId),
    TogglePanePinned(ClientId, Option<NotificationEnd>),
    SetFloatingPanePinned(PaneId, bool),
    StackPanes(Vec<PaneId>, ClientId, Option<NotificationEnd>),
    ChangeOverlayCoordinates(
        Vec<(PaneId, FloatingPaneCoordinates)>,
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
    SessionSharingStatusChange(bool),
    SetMouseSelectionSupport(PaneId, bool),
    InterceptKeyPresses(PluginId, ClientId),
    ClearKeyPressesIntercepts(ClientId),
    ReplacePaneWithExistingPane(PaneId, PaneId, bool, Option<NotificationEnd>), // bool -> suppress_replaced_pane
    AddWatcherClient(ClientId, Size),
    RemoveWatcherClient(ClientId),
    SetFollowedClient(ClientId),
    WatcherTerminalResize(ClientId, Size),
    ClearMouseHelpText(ClientId),
    UpdateAvailableLayouts(Vec<LayoutInfo>, Vec<LayoutWithError>),
    SetPluginRegexHighlights {
        pane_id: PaneId,
        plugin_id: u32,
        highlights: Vec<RegexHighlight>,
    },
    ClearPluginHighlights {
        pane_id: PaneId,
        plugin_id: u32,
    },
    ClearAllPluginHighlights(u32), // plugin_id — clears across all panes
    SubscribeToPaneRenders {
        client_id: ClientId,
        pane_ids: Vec<zellij_utils::data::PaneId>,
        scrollback: Option<usize>,
        ansi: bool,
    },
    NotifyPaneClosedToSubscribers {
        pane_id: zellij_utils::data::PaneId,
    },
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
    UndoRenameTabWithTabId(usize, Option<NotificationEnd>),
    ToggleActiveSyncTabWithTabId(usize, Option<NotificationEnd>),
    ToggleFloatingPanesWithTabId(usize, Option<TerminalAction>, Option<NotificationEnd>),
    PreviousSwapLayoutWithTabId(usize, Option<NotificationEnd>),
    NextSwapLayoutWithTabId(usize, Option<NotificationEnd>),
    MoveTabWithTabId(usize, Direction, Option<NotificationEnd>),
    SetSoftKeyboard {
        client_id: ClientId,
        on: bool,
    },
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
            ScreenInstruction::PtyBytes(..) => ScreenContext::HandlePtyBytes,
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
