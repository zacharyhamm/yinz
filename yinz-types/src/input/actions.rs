//! Definition of the actions that can be bound to keys.
use crate::internal_ids::SessionName;

pub use super::command::{OpenFilePayload, RunCommandAction};
use super::layout::{
    FloatingPaneLayout, RunPluginOrAlias, SwapFloatingLayout, SwapTiledLayout, TabLayoutInfo,
    TiledPaneLayout,
};
use crate::data::InputMode;
use crate::data::{
    CommandOrPlugin, LayoutInfo, NewPanePlacement, OriginatingPlugin, PaneId, UnblockCondition,
    WarpPlacement, WarpedPane, WarpedTab,
};
use crate::geometry::FloatingPaneCoordinates;
use crate::input::options::{OnForceClose, PaneFrameStyle};
use crate::internal_ids::TabId;
use crate::internal_ids::TabNumber;
use crate::keys::KeyWithModifier;
use crate::mouse::MouseEvent;
use crate::{
    agent::AgentInfo,
    geometry::{Direction, Resize},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use std::path::PathBuf;
use std::str::FromStr;

use crate::position::Position;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ResizeDirection {
    Left,
    Right,
    Up,
    Down,
    Increase,
    Decrease,
}

impl FromStr for ResizeDirection {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Left" | "left" => Ok(ResizeDirection::Left),
            "Right" | "right" => Ok(ResizeDirection::Right),
            "Up" | "up" => Ok(ResizeDirection::Up),
            "Down" | "down" => Ok(ResizeDirection::Down),
            "Increase" | "increase" | "+" => Ok(ResizeDirection::Increase),
            "Decrease" | "decrease" | "-" => Ok(ResizeDirection::Decrease),
            _ => Err(format!(
                "Failed to parse ResizeDirection. Unknown ResizeDirection: {}",
                s
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum SearchDirection {
    #[default]
    Down,
    Up,
}

impl FromStr for SearchDirection {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Down" | "down" => Ok(SearchDirection::Down),
            "Up" | "up" => Ok(SearchDirection::Up),
            _ => Err(format!(
                "Failed to parse SearchDirection. Unknown SearchDirection: {}",
                s
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum SearchOption {
    #[default]
    CaseSensitivity,
    WholeWord,
    Wrap,
}

impl FromStr for SearchOption {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CaseSensitivity" | "casesensitivity" | "Casesensitivity" => {
                Ok(SearchOption::CaseSensitivity)
            },
            "WholeWord" | "wholeword" | "Wholeword" => Ok(SearchOption::WholeWord),
            "Wrap" | "wrap" => Ok(SearchOption::Wrap),
            _ => Err(format!(
                "Failed to parse SearchOption. Unknown SearchOption: {}",
                s
            )),
        }
    }
}

// As these actions are bound to the default config, please
// do take care when refactoring - or renaming.
// They might need to be adjusted in the default config
// as well `../../assets/config/default.yaml`
/// Actions that can be bound to keys.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    strum_macros::Display,
    strum_macros::EnumString,
    strum_macros::EnumIter,
    strum_macros::EnumDiscriminants,
    Default,
)]
#[strum(ascii_case_insensitive)]
#[strum_discriminants(name(ActionKind))]
#[strum_discriminants(derive(strum_macros::EnumIter))]
pub enum Action {
    /// Quit Yinz.
    Quit,
    /// Write to the terminal.
    Write {
        key_with_modifier: Option<KeyWithModifier>,
        bytes: Vec<u8>,
        is_kitty_keyboard_protocol: bool,
    },
    /// Write Characters to the terminal.
    WriteChars {
        chars: String,
    },
    /// Write to a specific pane by ID.
    WriteToPaneId {
        bytes: Vec<u8>,
        pane_id: PaneId,
    },
    /// Write Characters to a specific pane by ID.
    WriteCharsToPaneId {
        chars: String,
        pane_id: PaneId,
    },
    /// Paste text using bracketed paste mode, optionally to a specific pane.
    Paste {
        chars: String,
        pane_id: Option<PaneId>,
    },
    /// Switch to the specified input mode.
    SwitchToMode {
        input_mode: InputMode,
    },
    /// Switch all connected clients to the specified input mode.
    SwitchModeForAllClients {
        input_mode: InputMode,
    },
    /// Shrink/enlarge focused pane at specified border
    Resize {
        resize: Resize,
        direction: Option<Direction>,
    },
    /// Switch focus to next pane in specified direction.
    FocusNextPane,
    FocusPreviousPane,
    /// Switch focus to the last focused pane.
    FocusLastPane,
    FocusPaneTabs,
    PreviousPaneTab,
    NextPaneTab,

    /// Move the focus pane in specified direction.
    SwitchFocus,
    MoveFocus {
        direction: Direction,
    },
    /// Tries to move the focus pane in specified direction.
    /// If there is no pane in the direction, move to previous/next Tab.
    MoveFocusOrTab {
        direction: Direction,
    },
    MovePane {
        direction: Option<Direction>,
    },
    MovePaneBackwards,
    /// Clear all buffers of a current screen
    ClearScreen,
    /// Dumps the screen to a file or STDOUT
    DumpScreen {
        file_path: Option<String>,
        include_scrollback: bool,
        pane_id: Option<PaneId>,
        ansi: bool,
    },
    /// Dumps
    DumpLayout,
    /// Save the current session state to disk
    SaveSession,
    EditScrollback {
        ansi: bool,
    },
    /// Scroll up in focus pane.
    ScrollUp,
    /// Scroll up at point
    ScrollUpAt {
        position: Position,
    },
    /// Scroll down in focus pane.
    ScrollDown,
    /// Scroll down at point
    ScrollDownAt {
        position: Position,
    },
    /// Scroll down to bottom in focus pane.
    ScrollToBottom,
    /// Scroll up to top in focus pane.
    ScrollToTop,
    /// Scroll the focused pane so the previous shell prompt (OSC 133) is at the top.
    ScrollToPreviousPrompt,
    /// Scroll the focused pane so the next shell prompt (OSC 133) is at the top.
    ScrollToNextPrompt,
    /// Select the OSC 133 command (and its output) at the top of the focused pane's viewport.
    SelectCommandAtScrollPosition,
    /// Copy the output of the last completed OSC 133 command in the focused pane.
    CopyLastCommandOutput,
    /// Extend the active text selection in the focused pane.
    MoveTextSelection {
        direction: Direction,
    },
    /// Swap the anchor and the moving end of the active text selection, so that subsequent
    /// `MoveTextSelection`s move what used to be the anchor.
    SwapTextSelectionEnds,
    /// Move the selection in the notification browser (`Notifications` mode). Only `Up` and
    /// `Down` move it; `Left` and `Right` are accepted and do nothing.
    MoveNotificationSelection {
        direction: Direction,
    },
    /// Activate the notification selected in the browser exactly as clicking its toast would, and
    /// leave `Notifications` mode.
    ActivateNotification,
    /// Activate the newest notification in session history and leave the notification browser.
    ActivateLastNotification,
    /// Show or hide the full, unelided text of the notification selected in the browser.
    ToggleNotificationDetail,
    /// Scroll up one page in focus pane.
    PageScrollUp,
    /// Scroll down one page in focus pane.
    PageScrollDown,
    /// Scroll up half page in focus pane.
    HalfPageScrollUp,
    /// Scroll down half page in focus pane.
    HalfPageScrollDown,
    /// Toggle between fullscreen focus pane and normal layout.
    ToggleFocusFullscreen,
    ToggleFocusNoUiFullscreen,
    /// Toggle frames around panes in the UI
    TogglePaneFrames,
    /// Toggle the client-local command palette prototype.
    ToggleCommandPalette,
    SetPaneFrameStyle(PaneFrameStyle),
    /// Toggle between sending text commands to all panes on the current tab and normal mode.
    ToggleActiveSyncTab,
    /// Open a new pane in the specified direction (relative to focus).
    /// If no direction is specified, will try to use the biggest available space.
    NewPane {
        direction: Option<Direction>,
        pane_name: Option<String>,
        start_suppressed: bool,
    },
    /// Returns: Created pane ID (format: terminal_<id>)
    NewBlockingPane {
        placement: NewPanePlacement,
        pane_name: Option<String>,
        command: Option<RunCommandAction>,
        unblock_condition: Option<UnblockCondition>,
        near_current_pane: bool,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    /// Open the file in a new pane using the default editor
    /// Returns: Created pane ID (format: terminal_<id>)
    EditFile {
        payload: OpenFilePayload,
        direction: Option<Direction>,
        floating: bool,
        in_place: bool,
        close_replaced_pane: bool,
        start_suppressed: bool,
        /// Where the editor pane goes when `floating` is set. See `NewFloatingPane::placement`.
        placement: NewPanePlacement,
        near_current_pane: bool,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    /// Open a new floating pane
    /// Returns: Created pane ID (format: terminal_<id> or plugin_<id>)
    NewFloatingPane {
        command: Option<RunCommandAction>,
        pane_name: Option<String>,
        /// Where on the surface: `Floating` for the tab's floating layer, `Overlay` for the
        /// session surface above the tab's own panes. Carried whole rather than as coordinates
        /// with a flag on them, so a modal's *kind* survives the trip to the server.
        placement: NewPanePlacement,
        near_current_pane: bool,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    /// Open a new tiled (embedded, non-floating) pane
    /// Returns: Created pane ID (format: terminal_<id> or plugin_<id>)
    NewTiledPane {
        direction: Option<Direction>,
        command: Option<RunCommandAction>,
        pane_name: Option<String>,
        near_current_pane: bool,
        no_focus: bool,
        borderless: Option<bool>,
        tab_id: Option<TabId>,
    },
    /// Open a new pane in place of the focused one, suppressing it instead
    /// Returns: Created pane ID (format: terminal_<id> or plugin_<id>)
    NewInPlacePane {
        command: Option<RunCommandAction>,
        pane_name: Option<String>,
        near_current_pane: bool,
        no_focus: bool,
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
        tab_id: Option<TabId>,
    },
    /// Returns: Created pane ID (format: terminal_<id> or plugin_<id>)
    NewStackedPane {
        command: Option<RunCommandAction>,
        pane_name: Option<String>,
        near_current_pane: bool,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    /// Embed focused pane in tab if floating or float focused pane if embedded
    TogglePaneEmbedOrFloating,
    /// Toggle the visibility of all floating panes (if any) in the current Tab
    ToggleFloatingPanes,
    /// Show all floating panes in the specified tab (or active tab if tab_id is None)
    ShowFloatingPanes {
        tab_id: Option<TabId>,
    },
    /// Hide all floating panes in the specified tab (or active tab if tab_id is None)
    HideFloatingPanes {
        tab_id: Option<TabId>,
    },
    /// Check if floating panes are visible in the specified tab (or active tab if tab_id is None)
    AreFloatingPanesVisible {
        tab_id: Option<TabId>,
    },
    /// Close the focus pane.
    CloseFocus,
    PaneNameInput {
        input: Vec<u8>,
    },
    UndoRenamePane,
    /// Create a new tab, optionally with a specified tab layout.
    NewTab {
        /// Explicit KDL layout reference, retained when saving keybindings.
        #[serde(default)]
        layout_source: Option<PathBuf>,
        tiled_layout: Option<TiledPaneLayout>,
        floating_layouts: Vec<FloatingPaneLayout>,
        swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
        swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
        tab_name: Option<String>,
        should_change_focus_to_new_tab: bool,
        cwd: Option<PathBuf>,
        initial_panes: Option<Vec<CommandOrPlugin>>,
        first_pane_unblock_condition: Option<UnblockCondition>,
    },
    /// Create and focus a group with a fresh tab.
    NewTabGroup,
    /// CLI creation with an optional stored name.
    NewNamedTabGroup {
        name: Option<String>,
    },
    GoToTabGroupName {
        name: String,
        create: bool,
    },
    RenameCurrentTabGroup {
        name: String,
    },
    CloseCurrentTabGroup,
    ListTabGroups {
        output_json: bool,
    },
    CurrentTabGroupInfo {
        output_json: bool,
    },

    GoToNextTabGroup,
    MovePaneToNextTabGroup,
    MovePaneToPreviousTabGroup,
    GoToPreviousTabGroup,
    GoToTabInGroup {
        index: TabNumber,
    },
    SwitchTabGroup {
        id: u32,
    },
    RenameTabGroup {
        id: u32,
        name: String,
    },
    CloseTabGroup {
        id: u32,
    },
    /// Do nothing.
    #[default]
    NoOp,
    /// Go to the next tab.
    GoToNextTab,
    /// Go to the previous tab.
    GoToPreviousTab,
    /// Close the current tab.
    CloseTab,
    GoToTab {
        index: TabNumber,
    },
    GoToTabName {
        name: String,
        create: bool,
    },
    ToggleTab,
    TabNameInput {
        input: Vec<u8>,
    },
    UndoRenameTab,
    MoveTab {
        direction: Direction,
    },
    /// Run specified command in new pane.
    Run {
        command: RunCommandAction,
        near_current_pane: bool,
        no_focus: bool,
    },
    /// Set pane default foreground/background color
    SetPaneColor {
        pane_id: PaneId,
        fg: Option<String>,
        bg: Option<String>,
    },
    /// Set what agent runs in a pane and what it is doing; `None` clears the record
    SetPaneAgent {
        pane_id: PaneId,
        agent: Option<AgentInfo>,
    },
    /// One event from a coding agent's own hooks, on its way to the plugins that subscribed to
    /// `agent_event` and to `yinz events`. Sent beside `SetPaneAgent` by `agent-report
    /// --hook-json`, and alone when the event says nothing about the state.
    AgentEvent(crate::agent::AgentEvent),
    /// Detach session and exit
    Detach,
    /// Switch the host-terminal theme mode to dark (uses configured `theme_dark`).
    SetDarkTheme,
    /// Switch the host-terminal theme mode to light (uses configured `theme_light`).
    SetLightTheme,
    /// Toggle between dark and light host-terminal theme modes.
    ToggleTheme,
    /// Switch to a different session
    SwitchSession {
        name: SessionName,
        tab_position: Option<TabNumber>,
        #[serde(default, with = "crate::pane_id::legacy_tuple")]
        pane_id: Option<PaneId>,
        layout: Option<LayoutInfo>,
        cwd: Option<PathBuf>,
    },
    /// Move a pane, running process and all, to another session.
    ///
    /// Handled by the session the pane is *leaving*: it releases the pane's
    /// pty-host supervisor without killing it and sends the other session an
    /// `AdoptPane`.
    WarpPane {
        /// `None` means whichever pane is focused.
        pane_id: Option<PaneId>,
        to_session: SessionName,
        placement: WarpPlacement,
    },
    /// Take a pane over, from a session warping it here or from a supervisor that
    /// outlived its server.
    ///
    /// Handled by the session the pane is *arriving in*, and sent by three
    /// different callers: a warp's source, `yinz pty-host adopt`, and a warp
    /// rolling itself back.
    AdoptPane {
        payload: Box<WarpedPane>,
        placement: WarpPlacement,
    },
    /// Move a whole tab, arrangement and running processes and all, to another
    /// session.
    ///
    /// The tab analogue of `WarpPane`, and handled the same way: by the session
    /// the tab is *leaving*, which releases every supervisor in it and sends the
    /// other session an `AdoptTab`.
    WarpTab {
        /// `None` means whichever tab is focused. This is a tab *id* rather than
        /// a position, matching `Screen::tabs`.
        tab_id: Option<TabId>,
        to_session: SessionName,
    },
    /// Take a whole tab over from a session warping it here.
    ///
    /// Handled by the session the tab is *arriving in*. There is no placement to
    /// choose: a tab arrives as a tab.
    AdoptTab {
        payload: Box<WarpedTab>,
    },
    /// Ask another session to send us one of its tabs.
    ///
    /// Handled by the session the tab is *arriving in*, which turns straight
    /// around and sends the source a `WarpTab` naming itself — so every check,
    /// the release and the rollback happen on the side that owns the tab. There
    /// is no `AdoptPane` equivalent of this because the pane feature only offers
    /// the pull to plugins, which can name their own session; a CLI client
    /// cannot, so it needs the receiving server to do it.
    WarpTabFrom {
        from_session: SessionName,
        tab_id: TabId,
    },
    /// Returns: Plugin pane ID (format: plugin_<id>) when creating or focusing plugin
    LaunchOrFocusPlugin {
        plugin: RunPluginOrAlias,
        should_float: bool,
        move_to_focused_tab: bool,
        should_open_in_place: bool,
        close_replaced_pane: bool,
        skip_cache: bool,
        tab_id: Option<TabId>,
    },
    /// Returns: Plugin pane ID (format: plugin_<id>)
    LaunchPlugin {
        plugin: RunPluginOrAlias,
        should_float: bool,
        should_open_in_place: bool,
        close_replaced_pane: bool,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    MouseEvent {
        event: MouseEvent,
    },
    Copy,
    /// Confirm a prompt
    Confirm,
    /// Deny a prompt
    Deny,
    /// Confirm an action that invokes a prompt automatically
    SkipConfirm {
        action: Box<Action>,
    },
    /// Search for String
    SearchInput {
        input: Vec<u8>,
    },
    /// Search for something
    Search {
        direction: SearchDirection,
    },
    /// Toggle case sensitivity of search
    SearchToggleOption {
        option: SearchOption,
    },
    ToggleMouseMode,
    PreviousSwapLayout,
    NextSwapLayout,
    /// Override the layout of the active tab
    OverrideLayout {
        tabs: Vec<TabLayoutInfo>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        apply_only_to_active_tab: bool,
    },
    /// Query all tab names
    QueryTabNames,
    /// Open a new tiled (embedded, non-floating) plugin pane
    /// Returns: Created pane ID (format: plugin_<id>)
    NewTiledPluginPane {
        plugin: RunPluginOrAlias,
        pane_name: Option<String>,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    /// Returns: Created pane ID (format: plugin_<id>)
    NewFloatingPluginPane {
        plugin: RunPluginOrAlias,
        pane_name: Option<String>,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        /// See `NewFloatingPane::placement`.
        placement: NewPanePlacement,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    /// Returns: Created pane ID (format: plugin_<id>)
    NewInPlacePluginPane {
        plugin: RunPluginOrAlias,
        pane_name: Option<String>,
        skip_cache: bool,
        close_replaced_pane: bool,
        no_focus: bool,
        tab_id: Option<TabId>,
    },
    StartOrReloadPlugin {
        plugin: RunPluginOrAlias,
    },
    CloseTerminalPane {
        pane_id: crate::internal_ids::TerminalId,
    },
    ClosePluginPane {
        pane_id: crate::internal_ids::PluginId,
    },
    FocusTerminalPaneWithId {
        pane_id: crate::internal_ids::TerminalId,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
    },
    FocusPluginPaneWithId {
        pane_id: crate::internal_ids::PluginId,
        should_float_if_hidden: bool,
        should_be_in_place_if_hidden: bool,
    },
    RenameTerminalPane {
        pane_id: crate::internal_ids::TerminalId,
        name: Vec<u8>,
    },
    RenamePluginPane {
        pane_id: crate::internal_ids::PluginId,
        name: Vec<u8>,
    },
    RenameTab {
        tab_index: TabNumber,
        name: Vec<u8>,
    },
    GoToTabById {
        id: TabId,
    },
    CloseTabById {
        id: TabId,
    },
    RenameTabById {
        id: TabId,
        name: String,
    },
    ArrangePanesByProject,
    BreakPane,
    BreakPaneRight,
    BreakPaneLeft,
    FocusHostSession,
    FocusGuestSession,
    ToggleHostFullscreen,
    RenameSession {
        name: SessionName,
    },
    /// Apply a KDL config fragment to every attached client's running configuration, and
    /// optionally persist it. What `PluginCommand::Reconfigure` does for a plugin, for a shell.
    Reconfigure {
        config: String,
        write_to_disk: bool,
    },
    /// Show a self-dismissing notification in the configured screen corner
    Notify(String),
    CliPipe {
        #[serde(default)]
        focus_plugin: bool,
        pipe_id: String,
        name: Option<String>,
        payload: Option<String>,
        args: Option<BTreeMap<String, String>>,
        plugin: Option<String>,
        configuration: Option<BTreeMap<String, String>>,
        launch_new: bool,
        skip_cache: bool,
        floating: Option<bool>,
        in_place: Option<bool>,
        cwd: Option<PathBuf>,
        pane_title: Option<String>,
    },
    KeybindPipe {
        name: Option<String>,
        payload: Option<String>,
        args: Option<BTreeMap<String, String>>,
        plugin: Option<String>,
        plugin_id: Option<crate::internal_ids::PluginId>, // supercedes plugin if present
        configuration: Option<BTreeMap<String, String>>,
        launch_new: bool,
        skip_cache: bool,
        floating: Option<bool>,
        in_place: Option<bool>,
        cwd: Option<PathBuf>,
        pane_title: Option<String>,
    },
    ListClients,
    ListPanes {
        show_tab: bool,
        show_command: bool,
        show_state: bool,
        show_geometry: bool,
        show_all: bool,
        output_json: bool,
    },
    ListTabs {
        show_state: bool,
        show_dimensions: bool,
        show_panes: bool,
        show_layout: bool,
        show_all: bool,
        output_json: bool,
    },
    CurrentTabInfo {
        output_json: bool,
    },
    TogglePanePinned,
    StackPanes {
        pane_ids: Vec<PaneId>,
    },
    ChangeFloatingPaneCoordinates {
        pane_id: PaneId,
        coordinates: FloatingPaneCoordinates,
    },
    TogglePaneBorderless {
        pane_id: PaneId,
    },
    SetPaneBorderless {
        pane_id: PaneId,
        borderless: bool,
    },
    TogglePaneInGroup,
    ToggleGroupMarking,
    // Pane-targeting CLI-only variants
    ScrollUpByPaneId {
        pane_id: PaneId,
    },
    ScrollDownByPaneId {
        pane_id: PaneId,
    },
    ScrollToTopByPaneId {
        pane_id: PaneId,
    },
    ScrollToBottomByPaneId {
        pane_id: PaneId,
    },
    PageScrollUpByPaneId {
        pane_id: PaneId,
    },
    PageScrollDownByPaneId {
        pane_id: PaneId,
    },
    HalfPageScrollUpByPaneId {
        pane_id: PaneId,
    },
    HalfPageScrollDownByPaneId {
        pane_id: PaneId,
    },
    ResizeByPaneId {
        pane_id: PaneId,
        resize: Resize,
        direction: Option<Direction>,
    },
    MovePaneByPaneId {
        pane_id: PaneId,
        direction: Option<Direction>,
    },
    MovePaneBackwardsByPaneId {
        pane_id: PaneId,
    },
    ClearScreenByPaneId {
        pane_id: PaneId,
    },
    EditScrollbackByPaneId {
        pane_id: PaneId,
        ansi: bool,
    },
    ToggleFocusFullscreenByPaneId {
        pane_id: PaneId,
    },
    ToggleFocusNoUiFullscreenByPaneId {
        pane_id: PaneId,
    },
    TogglePaneEmbedOrFloatingByPaneId {
        pane_id: PaneId,
    },
    CloseFocusByPaneId {
        pane_id: PaneId,
    },
    RenamePaneByPaneId {
        pane_id: Option<PaneId>,
        name: Vec<u8>,
    },
    UndoRenamePaneByPaneId {
        pane_id: PaneId,
    },
    TogglePanePinnedByPaneId {
        pane_id: PaneId,
    },
    FocusPaneByPaneId {
        pane_id: PaneId,
    },
    // Tab-targeting CLI-only variants
    UndoRenameTabByTabId {
        id: TabId,
    },
    ToggleActiveSyncTabByTabId {
        id: TabId,
    },
    ToggleFloatingPanesByTabId {
        id: TabId,
    },
    PreviousSwapLayoutByTabId {
        id: TabId,
    },
    NextSwapLayoutByTabId {
        id: TabId,
    },
    MoveTabByTabId {
        id: TabId,
        direction: Direction,
    },
}

/// Both `warp-pane` and `pty-host adopt` spell placement the same way: two
/// mutually exclusive flags over a new-tab default. clap enforces the exclusion,
/// so this only has to pick.
pub fn warp_placement(focused_tab: bool, floating: bool) -> WarpPlacement {
    if floating {
        WarpPlacement::Floating
    } else if focused_tab {
        WarpPlacement::FocusedTab
    } else {
        WarpPlacement::NewTab
    }
}

/// What one agent hook event asks the session to do: set the pane's record, and hand the
/// event itself to the plugins.
///
/// Both, from the one process a hook may spend. A record is deliberately smaller than an event
/// — the agent's own final message on a `Stop`, the prompt behind it on a `UserPromptSubmit`,
/// the permission mode on every one of them, none of which belong on a manifest broadcast to
/// every plugin on every pane update — so the event goes out beside it, whole up to
/// `agent::HOOK_BROADCAST_MAX_BYTES` and pruned above that, and whoever wants more than a
/// record holds reads it there.
/// What `yinz action agent-report --hook-json` does with one event: set the pane's record
/// where the event says something about the state, and send the event itself, typed, to the
/// plugins that subscribed to it. `json` is the event exactly as it arrived.
pub fn hook_report_actions(
    kind: &str,
    pane_id: PaneId,
    event: &crate::agent::HookEvent,
    json: String,
) -> Vec<Action> {
    let mut actions = Vec::with_capacity(2);
    match crate::agent::hook_outcome(kind, event) {
        crate::agent::HookOutcome::Ended => actions.push(Action::SetPaneAgent {
            pane_id,
            agent: None,
        }),
        crate::agent::HookOutcome::Reported(agent) => actions.push(Action::SetPaneAgent {
            pane_id,
            agent: Some(agent),
        }),
        // Nothing to report leaves the record as it was — but the event still goes out,
        // because "says nothing about the state" is not "says nothing".
        crate::agent::HookOutcome::Nothing => {},
    }
    // Whole, unless whole is unreasonable: `tool_input` and `tool_response` have no bound in
    // Claude Code's schema, and this event is copied to every plugin subscribed to it. The cap
    // is `agent::HOOK_BROADCAST_MAX_BYTES`, kept by `AgentEvent::from_hook`. A typed event
    // rather than a pipe: it reaches subscribers only, is gated like the rest of the session's
    // state, and says which agent wrote it - the payload alone never does.
    let PaneId::Terminal(terminal_id) = pane_id else {
        // A hook never fires in a plugin pane, so there is nothing to say for one.
        return actions;
    };
    actions.push(Action::AgentEvent(crate::agent::AgentEvent::from_hook(
        kind,
        terminal_id,
        event,
        json,
    )));
    actions
}

impl Action {
    /// Whether this action puts text on the clipboard, and so must leave the status bar's "text
    /// copied" hint standing rather than clearing it with an `InputReceived`. This is the keyboard
    /// counterpart of `MouseEffect::leave_clipboard_message`: a key bound to `Copy` alongside
    /// anything else (`bind "Enter" { Copy; SwitchToMode "Normal"; }`) would otherwise have the
    /// trailing action wipe the message the copy just produced.
    pub fn leaves_clipboard_message(&self) -> bool {
        match self {
            Action::Copy | Action::CopyLastCommandOutput => true,
            Action::SkipConfirm { action } => action.leaves_clipboard_message(),
            _ => false,
        }
    }

    /// Checks that two Action are match except their mutable attributes.
    pub fn shallow_eq(&self, other_action: &Action) -> bool {
        match (self, other_action) {
            (Action::NewTab { .. }, Action::NewTab { .. }) => true,
            (Action::LaunchOrFocusPlugin { .. }, Action::LaunchOrFocusPlugin { .. }) => true,
            (Action::LaunchPlugin { .. }, Action::LaunchPlugin { .. }) => true,
            (Action::OverrideLayout { .. }, Action::OverrideLayout { .. }) => true,
            _ => self == other_action,
        }
    }

    pub fn populate_originating_plugin(&mut self, originating_plugin: OriginatingPlugin) {
        match self {
            Action::NewBlockingPane { command, .. }
            | Action::NewFloatingPane { command, .. }
            | Action::NewTiledPane { command, .. }
            | Action::NewInPlacePane { command, .. }
            | Action::NewStackedPane { command, .. } => {
                if let Some(c) = command.as_mut() {
                    c.populate_originating_plugin(originating_plugin)
                }
            },
            Action::Run { command, .. } => {
                command.populate_originating_plugin(originating_plugin);
            },
            Action::EditFile { payload, .. } => {
                payload.originating_plugin = Some(originating_plugin);
            },
            Action::NewTab { initial_panes, .. } => {
                if let Some(initial_panes) = initial_panes.as_mut() {
                    for pane in initial_panes.iter_mut() {
                        if let CommandOrPlugin::Command(run_command) = pane {
                            run_command.populate_originating_plugin(originating_plugin.clone());
                        }
                    }
                }
            },
            _ => {},
        }
    }
    pub fn launches_plugin(&self, plugin_url: &str) -> bool {
        match self {
            Action::LaunchPlugin { plugin, .. } => plugin.location_string() == plugin_url,
            Action::LaunchOrFocusPlugin { plugin, .. } => plugin.location_string() == plugin_url,
            _ => false,
        }
    }
    pub fn is_mouse_action(&self) -> bool {
        if let Action::MouseEvent { .. } = self {
            return true;
        }
        false
    }
}

impl From<OnForceClose> for Action {
    fn from(ofc: OnForceClose) -> Action {
        match ofc {
            OnForceClose::Quit => Action::Quit,
            OnForceClose::Detach => Action::Detach,
        }
    }
}
