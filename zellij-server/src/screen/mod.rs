// Clippy allows are intentional for this large file; they suppress lints that
// would otherwise churn the branch. Keep with comment or fix incrementally.
// See REVIEW_CHECKLIST.md "Many clippy allows hide warnings".
#![allow(clippy::large_enum_variant)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::manual_pattern_char_comparison)]
#![allow(clippy::type_complexity)]
#![allow(clippy::unwrap_or_default)]
#![allow(clippy::iter_kv_map)]
#![allow(clippy::single_match)]
#![allow(clippy::for_kv_map)]
#![allow(clippy::unnecessary_unwrap)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::unnecessary_get_then_check)]
#![allow(clippy::unnecessary_sort_by)]
#![allow(clippy::useless_format)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::needless_option_take)]
#![allow(clippy::needless_return)]
#![allow(clippy::option_map_unit_fn)]
#![allow(clippy::unnecessary_map_or)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::manual_filter)]
#![allow(clippy::redundant_pattern_matching)]
#![allow(clippy::iter_next_slice)]
#![allow(clippy::unnecessary_cast)]
//! Things related to [`Screen`]s.
//!
//! # Tab Identification
//!
//! Tabs have two distinct identifiers:
//!
//! - **ID** (`tab.id`): Stable, unique identifier that never changes after creation.
//!   Used as BTreeMap key and for internal tracking. Monotonically increasing.
//!
//! - **Position** (`tab.position`): Current display order (0-based index in tab bar).
//!   Changes when tabs are moved or closed. Used for user-facing operations.
//!
//! # Terminology Convention
//!
//! - **"id"**: Always means stable identifier
//! - **"position"**: Always means 0-based display order
//! - **"index"**: Synonym for "position" (used in public/plugin APIs)
//!
//! Examples:
//! - `close_tab_by_id(5)` - Closes tab with stable ID 5
//! - `CloseTabWithIndex(2)` - Closes tab at position 2 (3rd tab visually)
//! - `get_tab_by_position(0)` - Gets first tab in display order
//! - `PluginInstruction::NewTab(tab_id, ...)` - Uses ID for async communication
//!
//! # Key Data Structures
//!
//! - `tabs: BTreeMap<usize, Tab>`: Keyed by tab.id (stable identifier)
//! - `active_tab_ids: BTreeMap<ClientId, usize>`: Maps clients to active tab ID
//! - `tab_history: BTreeMap<ClientId, Vec<usize>>`: History of tab IDs per client

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::rc::Rc;
use std::str;
use std::time::{Duration, Instant};

use crate::route::NotificationEnd;

use crate::pty_host::client::ReleasedPane;
use crate::ui::notification::{
    lerp_origin, notification_chunks, notification_height, notification_lines,
    notification_origins, notification_reveal, notification_reveal_bounds, notification_width,
};
use log::{debug, warn};
use zellij_utils::data::{
    CommandOrPlugin, Direction, EventType, FloatingPaneCoordinates, GetFocusedPaneInfoResponse,
    GlobalPaneEdge, GlobalPanePlacement, HostTerminalThemeMode, KeyWithModifier, LayoutInfo,
    LayoutWithError, ListPanesResponse, ListTabsResponse, NewPanePlacement, NotificationInfo,
    OverlayPlacement, PaneInfo, PaneScrollbackResponse, PluginPermission, RegexHighlight,
    RemotePaneHandoff, Resize, ResizeStrategy, SessionInfo, Styling, TabInfo, ThemeHue,
    WarpPlacement, WarpedPane, WarpedTab, WebSharing,
};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::command::{OpenFilePayload, RunCommand};
use zellij_utils::input::config::Config;
use zellij_utils::input::keybinds::Keybinds;
use zellij_utils::input::mouse::{MouseEvent, MouseEventType};
use zellij_utils::input::options::{
    Clipboard, NestedSessionHandling, NotificationPosition, PaneFrameStyle,
    DEFAULT_NOTIFICATION_TIMEOUT_SECS, DEFAULT_WORD_SEPARATORS,
};
use zellij_utils::ipc::{MobileStatePayload, ServerToClientMsg};
use zellij_utils::pane_size::{Dimension, PaneGeom, Size, SizeInPixels, Viewport};
use zellij_utils::shared::{clean_string_from_control_and_linebreak, detect_theme_hue};
use zellij_utils::{
    consts::{session_info_folder_for_session, ZELLIJ_SOCK_DIR},
    envs::set_session_name,
    input::command::TerminalAction,
    input::layout::{
        FloatingPaneLayout, Layout, PercentOrFixed, Run, RunPluginOrAlias, SwapFloatingLayout,
        SwapTiledLayout, TabLayoutInfo, TiledPaneLayout,
    },
    input::parse_keys,
    position::Position,
};

use crate::background_jobs::BackgroundJob;
use crate::os_input_output::ResizeCache;
use crate::pane_groups::PaneGroups;
use crate::panes::alacritty_functions::xparse_color;
use crate::panes::link_handler::LinkHandler;
use crate::panes::terminal_character::AnsiCode;
use crate::panes::terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END};
use crate::session_layout_metadata::{PaneLayoutMetadata, SessionLayoutMetadata};

use crate::{
    global_panes::{unowned_strip_rows, AllocatedGlobalPanes, GlobalPaneManager, GlobalPaneStrip},
    nested_guest::NestedGuestTracker,
    output::{CharacterChunk, HostKittyState, Output},
    panes::kitty_graphics::{KittyHostSupport, KittyImageStore},
    panes::sixel::SixelImageStore,
    panes::{PaneFactory, PaneId, EMPTY_TERMINAL_CHARACTER},
    plugins::{DumpSessionLayoutResponse, PluginId, PluginInstruction, PluginRenderAsset},
    pty::{
        get_default_shell, AdoptedTabPanes, ClientTabIndexOrPaneId, GlobalPaneSpawnContext,
        GlobalSplitAnchor, PtyInstruction, VteBytes,
    },
    pty_writer::PtyWriteInstruction,
    tab::{AdjustedInput, Pane, SuppressedPanes, Tab},
    thread_bus::Bus,
    ui::loading_indication::LoadingIndication,
    ClientId, ServerInstruction,
};
use zellij_utils::{
    data::{Event, InputMode, ModeInfo, Palette, PaletteColor, PluginCapabilities, Style},
    errors::{ContextType, ScreenContext},
    input::get_mode_info,
    ipc::{ClientAttributes, PixelDimensions},
    nested_session::{self, NestedSessionMessage},
};

use crate::mobile_web::MobileWebPrefs;

mod break_panes;
mod input_modes;
mod mouse;
mod nested;
mod plugin_state;
mod render;
mod subscriptions;
mod warp_ops;

/// Parses a namespaced OSC 99 response and extracts the original pane ID
/// and un-namespaced response bytes.
///
/// Input bytes (the termwiz OperatingSystemCommand payload after stripping "99;"):
/// e.g. b"i=p42.mynotif" or b"i=p42.mynotif:p=close;some_data"
///
/// Returns Some((terminal_id, full_osc_bytes)) where full_osc_bytes is
/// the complete reconstructed OSC 99 sequence with original identifier,
/// ready to write to the pane's PTY.
/// Denormalizes a namespaced OSC 99 response.
///
/// Parses the namespaced `i=p<N>[r][q].<original_id>` format and returns:
/// - `pane_id`: the terminal pane that originated the notification
/// - `app_wants_report`: `r` flag — app originally requested `a=report`
/// - `is_query`: `q` flag — this was a capability query (`p=?`)
/// - `restored_response_bytes`: the response with the original `i=` value restored
pub(crate) fn denormalize_notification_response(
    payload: &[u8],
) -> Option<(u32, bool, bool, Vec<u8>)> {
    let payload_str = str::from_utf8(payload).ok()?;

    // Split into metadata and response payload on first ';'
    let (metadata, response_payload) = match payload_str.find(';') {
        Some(idx) => (
            payload_str.get(..idx).unwrap_or_default(),
            payload_str.get(idx..).unwrap_or_default(),
        ),
        None => (payload_str, ""),
    };

    // Find the i= key in colon-separated metadata
    let mut terminal_id = None;
    let mut app_wants_report = false;
    let mut is_query = false;
    let mut restored_parts = Vec::new();

    for kv in metadata.split(':') {
        if let Some(namespaced_value) = kv.strip_prefix("i=p") {
            // Parse "p<N>[r][q].<original_id>"
            if let Some(dot_pos) = namespaced_value.find('.') {
                let flags_part = namespaced_value.get(..dot_pos).unwrap_or_default();
                let original_id = namespaced_value.get(dot_pos + 1..).unwrap_or_default();
                let pane_id_str = flags_part.trim_end_matches(['r', 'q']);
                let flag_chars = flags_part.get(pane_id_str.len()..).unwrap_or_default();
                if let Ok(pid) = pane_id_str.parse::<u32>() {
                    terminal_id = Some(pid);
                    app_wants_report = flag_chars.contains('r');
                    is_query = flag_chars.contains('q');
                    // Empty original_id means the app never sent an i= key;
                    // don't inject one into the response
                    if !original_id.is_empty() {
                        restored_parts.push(format!("i={}", original_id));
                    }
                    continue;
                }
            }
        }
        restored_parts.push(kv.to_string());
    }

    let terminal_id = terminal_id?;
    let restored_metadata = restored_parts.join(":");
    let full_response = format!("\x1b]99;{}{}\x1b\\", restored_metadata, response_payload);
    Some((
        terminal_id,
        app_wants_report,
        is_query,
        full_response.into_bytes(),
    ))
}

/// Get the active tab and call a closure on it
///
/// If no active tab can be found, an error is logged instead.
///
/// # Parameters
///
/// - screen: An instance of `Screen` to operate on
/// - client_id: The client_id, usually taken from the `ScreenInstruction` that's being processed
/// - closure: A closure satisfying `|tab: &mut Tab| -> ()` OR `|tab: &mut Tab| -> Result<T>` (see
///   '?' below)
/// - ?: A literal "?", to append a `?` to the closure when it returns a `Result` type. This
///   argument is optional and not needed when the closure returns `()`
macro_rules! active_tab {
    ($screen:ident, $client_id:ident, $closure:expr) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
                // This could be made more ergonomic by declaring the type of 'active_tab' in the
                // closure, known as "Type Ascription". Then we could hint the type here and forego the
                // "&mut Tab" in all the closures below...
                // See: https://github.com/rust-lang/rust/issues/23416
                $closure(active_tab);
            },
            Err(err) => Err::<(), _>(err).non_fatal(),
        };
    };
    // Same as above, but with an added `?` for when the close returns a `Result` type.
    ($screen:ident, $client_id:ident, $closure:expr, ?) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
            $closure(active_tab)?;
            },
            Err(err) => Err::<(), _>(err).non_fatal(),
        };
    };
}
pub(crate) use active_tab;

/// The global pane edge you reach by moving in `direction`.
fn global_pane_edge_for_direction(direction: Direction) -> GlobalPaneEdge {
    match direction {
        Direction::Left => GlobalPaneEdge::Left,
        Direction::Right => GlobalPaneEdge::Right,
        Direction::Up => GlobalPaneEdge::Top,
        Direction::Down => GlobalPaneEdge::Bottom,
    }
}

/// The direction that leads from a global pane on `edge` back into the tab area.
fn direction_away_from_edge(edge: GlobalPaneEdge) -> Direction {
    match edge {
        GlobalPaneEdge::Left => Direction::Right,
        GlobalPaneEdge::Right => Direction::Left,
        GlobalPaneEdge::Top => Direction::Down,
        GlobalPaneEdge::Bottom => Direction::Up,
    }
}

macro_rules! active_tab_and_connected_client_id {
    ($screen:ident, $client_id:ident, $closure:expr) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
                $closure(active_tab, $client_id);
            },
            Err(_) => {
                if let Some(client_id) = $screen.get_first_client_id() {
                    match $screen.get_active_tab_mut(client_id) {
                        Ok(active_tab) => {
                            $closure(active_tab, client_id);
                        },
                        Err(err) => Err::<(), _>(err).non_fatal(),
                    }
                } else {
                    log::error!("No client ids in screen found");
                };
            },
        }
    };
    // Same as above, but with an added `?` for when the closure returns a `Result` type.
    ($screen:ident, $client_id:ident, $closure:expr, ?) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
                $closure(active_tab, $client_id).non_fatal();
            },
            Err(_) => {
                if let Some(client_id) = $screen.get_first_client_id() {
                    match $screen.get_active_tab_mut(client_id) {
                        Ok(active_tab) => {
                            $closure(active_tab, client_id)?;
                        },
                        Err(err) => Err::<(), _>(err).non_fatal(),
                    }
                } else {
                    log::error!("No client ids in screen found");
                };
            },
        }
    };
}

macro_rules! active_tab_and_connected_client_id_with_first_tab_fallback {
    ($screen:ident, $client_id:ident, $closure:expr) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
                $closure(active_tab, Some($client_id));
            },
            Err(_) => {
                if let Some(client_id) = $screen.get_first_client_id() {
                    match $screen.get_active_tab_mut(client_id) {
                        Ok(active_tab) => {
                            $closure(active_tab, Some(client_id));
                        },
                        Err(err) => Err::<(), _>(err).non_fatal(),
                    }
                } else {
                    match $screen.first_tab_mut() {
                        Some(first_tab) => {
                            $closure(first_tab, None);
                        },
                        None => {
                            log::error!("Not tabs found!");
                        },
                    }
                };
            },
        }
    };
    // Same as above, but with an added `?` for when the closure returns a `Result` type.
    ($screen:ident, $client_id:ident, $closure:expr, ?) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
                $closure(active_tab, Some($client_id)).non_fatal();
            },
            Err(_) => {
                if let Some(client_id) = $screen.get_first_client_id() {
                    match $screen.get_active_tab_mut(client_id) {
                        Ok(active_tab) => {
                            $closure(active_tab, Some(client_id))?;
                        },
                        Err(err) => Err::<(), _>(err).non_fatal(),
                    }
                } else {
                    match $screen.first_tab_mut() {
                        Some(first_tab) => {
                            $closure(first_tab, None)?;
                        },
                        None => {
                            log::error!("Not tabs found!");
                        },
                    }
                };
            },
        }
    };
}

type InitialTitle = String;
type HoldForCommand = Option<RunCommand>;

/// Result of overriding a single tab's layout
#[derive(Debug, Clone)]
pub struct TabOverrideResult {
    pub tab_index: usize,
    pub tab_name: Option<String>,
    pub tiled_layout: TiledPaneLayout,
    pub floating_layouts: Vec<FloatingPaneLayout>,
    pub swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
    pub swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
    pub new_terminal_pids: Vec<(u32, HoldForCommand)>,
    pub new_floating_pane_pids: Vec<(u32, HoldForCommand)>,
    pub plugin_ids: HashMap<RunPluginOrAlias, Vec<u32>>,
}

/// Instructions that can be sent to the [`Screen`].
#[derive(Debug, Clone)]
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
    NewGlobalPane {
        pane_id: PaneId,
        initial_pane_title: Option<InitialTitle>,
        hold_for_command: HoldForCommand,
        invoked_with: Option<Run>,
        placement: GlobalPanePlacement,
        borderless: bool,
        initially_hidden: bool,
        should_focus: bool,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
        set_blocking: bool,
        declaration_order: Option<usize>,
        split: Option<GlobalSplitAnchor>,
    },
    /// Launch a plugin as a split of `anchor`'s global pane, or reveal (and optionally focus) an
    /// existing global pane for that plugin - the plugin-facing sibling of LaunchOrFocusPlugin's
    /// global branch, anchored to the calling plugin's own pane.
    LaunchOrFocusGlobalPaneSplit {
        run_plugin: RunPluginOrAlias,
        anchor: PaneId,
        before: bool,
        length: Option<PercentOrFixed>,
        should_focus: bool,
        skip_cache: bool,
        client_id: ClientId,
    },
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
    ProgressPluginLoadingOffset(u32),                 // u32 - plugin id
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
        Option<GlobalPanePlacement>,
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
        Option<GlobalPanePlacement>,
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
    TickNotificationAnimations,
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
    AddHighlightPaneFrameColorOverride(Vec<PaneId>, Option<String>), // Option<String> => optional
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
    SetGlobalPanePlacement(PaneId, GlobalPanePlacement, Option<NotificationEnd>),
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
        match *screen_instruction {
            ScreenInstruction::PtyBytes(..) => ScreenContext::HandlePtyBytes,
            ScreenInstruction::PluginBytes(..) => ScreenContext::PluginBytes,
            ScreenInstruction::Render => ScreenContext::Render,
            ScreenInstruction::RenderToClients => ScreenContext::RenderToClients,
            ScreenInstruction::NewPane(..) => ScreenContext::NewPane,
            ScreenInstruction::NewGlobalPane { .. } => ScreenContext::NewPane,
            ScreenInstruction::LaunchOrFocusGlobalPaneSplit { .. } => {
                ScreenContext::LaunchOrFocusPlugin
            },
            ScreenInstruction::OpenInPlaceEditor(..) => ScreenContext::OpenInPlaceEditor,
            ScreenInstruction::TogglePaneEmbedOrFloating(..) => {
                ScreenContext::TogglePaneEmbedOrFloating
            },
            ScreenInstruction::ToggleFloatingPanes(..) => ScreenContext::ToggleFloatingPanes,
            ScreenInstruction::ShowFloatingPanes { .. } => ScreenContext::ShowFloatingPanes,
            ScreenInstruction::HideFloatingPanes { .. } => ScreenContext::HideFloatingPanes,
            ScreenInstruction::AreFloatingPanesVisible { .. } => {
                ScreenContext::AreFloatingPanesVisible
            },
            ScreenInstruction::WriteCharacter(..) => ScreenContext::WriteCharacter,
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
            ScreenInstruction::SwitchFocus(..) => ScreenContext::SwitchFocus,
            ScreenInstruction::FocusNextPane(..) => ScreenContext::FocusNextPane,
            ScreenInstruction::FocusPreviousPane(..) => ScreenContext::FocusPreviousPane,
            ScreenInstruction::FocusLastPane(..) => ScreenContext::FocusLastPane,
            ScreenInstruction::MoveFocusLeft(..) => ScreenContext::MoveFocusLeft,
            ScreenInstruction::MoveFocusLeftOrPreviousTab(..) => {
                ScreenContext::MoveFocusLeftOrPreviousTab
            },
            ScreenInstruction::MoveFocusDown(..) => ScreenContext::MoveFocusDown,
            ScreenInstruction::MoveFocusUp(..) => ScreenContext::MoveFocusUp,
            ScreenInstruction::MoveFocusRight(..) => ScreenContext::MoveFocusRight,
            ScreenInstruction::MoveFocusRightOrNextTab(..) => {
                ScreenContext::MoveFocusRightOrNextTab
            },
            ScreenInstruction::MovePane(..) => ScreenContext::MovePane,
            ScreenInstruction::MovePaneBackwards(..) => ScreenContext::MovePaneBackwards,
            ScreenInstruction::MovePaneDown(..) => ScreenContext::MovePaneDown,
            ScreenInstruction::MovePaneUp(..) => ScreenContext::MovePaneUp,
            ScreenInstruction::MovePaneRight(..) => ScreenContext::MovePaneRight,
            ScreenInstruction::MovePaneLeft(..) => ScreenContext::MovePaneLeft,
            ScreenInstruction::Exit => ScreenContext::Exit,
            ScreenInstruction::ClearScreen(..) => ScreenContext::ClearScreen,
            ScreenInstruction::DumpScreen(..) => ScreenContext::DumpScreen,
            ScreenInstruction::DumpLayout(..) => ScreenContext::DumpLayout,
            ScreenInstruction::SaveSession(..) => ScreenContext::SaveSession,
            ScreenInstruction::DumpLayoutToPlugin { .. } => ScreenContext::DumpLayoutToPlugin,
            ScreenInstruction::GetFocusedPaneInfo { .. } => ScreenContext::GetFocusedPaneInfo,
            ScreenInstruction::GetPaneInfo { .. } => ScreenContext::GetPaneInfo,
            ScreenInstruction::GetTabInfo { .. } => ScreenContext::GetTabInfo,
            ScreenInstruction::EditScrollback(..) => ScreenContext::EditScrollback,
            ScreenInstruction::GetPaneScrollback { .. } => ScreenContext::GetPaneScrollback,
            ScreenInstruction::ScrollUp(..) => ScreenContext::ScrollUp,
            ScreenInstruction::ScrollDown(..) => ScreenContext::ScrollDown,
            ScreenInstruction::ScrollToBottom(..) => ScreenContext::ScrollToBottom,
            ScreenInstruction::ScrollToTop(..) => ScreenContext::ScrollToTop,
            ScreenInstruction::PageScrollUp(..) => ScreenContext::PageScrollUp,
            ScreenInstruction::PageScrollDown(..) => ScreenContext::PageScrollDown,
            ScreenInstruction::HalfPageScrollUp(..) => ScreenContext::HalfPageScrollUp,
            ScreenInstruction::HalfPageScrollDown(..) => ScreenContext::HalfPageScrollDown,
            ScreenInstruction::ClearScroll(..) => ScreenContext::ClearScroll,
            ScreenInstruction::CloseFocusedPane(..) => ScreenContext::CloseFocusedPane,
            ScreenInstruction::ToggleActiveTerminalFullscreen(..) => {
                ScreenContext::ToggleActiveTerminalFullscreen
            },
            ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(..) => {
                ScreenContext::ToggleActiveTerminalNoUiFullscreen
            },
            ScreenInstruction::TogglePaneFrames(..) => ScreenContext::TogglePaneFrames,
            ScreenInstruction::SetPaneFrameStyle(..) => ScreenContext::SetPaneFrameStyle,
            ScreenInstruction::SetSelectable(..) => ScreenContext::SetSelectable,
            ScreenInstruction::ShowPluginCursor(..) => ScreenContext::ShowPluginCursor,
            ScreenInstruction::ClosePane(..) => ScreenContext::ClosePane,
            ScreenInstruction::HoldPane(..) => ScreenContext::HoldPane,
            ScreenInstruction::UpdatePaneName(..) => ScreenContext::UpdatePaneName,
            ScreenInstruction::UpdatePaneCwd(..) => ScreenContext::UpdatePaneCwd,
            ScreenInstruction::UpdatePaneHost(..) => ScreenContext::UpdatePaneHost,
            ScreenInstruction::UndoRenamePane(..) => ScreenContext::UndoRenamePane,
            ScreenInstruction::NewTab(..) => ScreenContext::NewTab,
            ScreenInstruction::ApplyLayout(..) => ScreenContext::ApplyLayout,
            ScreenInstruction::SwitchTabNext(..) => ScreenContext::SwitchTabNext,
            ScreenInstruction::SwitchTabPrev(..) => ScreenContext::SwitchTabPrev,
            ScreenInstruction::CloseTab(..) => ScreenContext::CloseTab,
            ScreenInstruction::GoToTab(..) => ScreenContext::GoToTab,
            ScreenInstruction::GoToTabName(..) => ScreenContext::GoToTabName,
            ScreenInstruction::UpdateTabName(..) => ScreenContext::UpdateTabName,
            ScreenInstruction::UndoRenameTab(..) => ScreenContext::UndoRenameTab,
            ScreenInstruction::MoveTabLeft(..) => ScreenContext::MoveTabLeft,
            ScreenInstruction::MoveTabRight(..) => ScreenContext::MoveTabRight,
            ScreenInstruction::GoToTabWithId(..) => ScreenContext::GoToTabWithId,
            ScreenInstruction::CloseTabWithId(..) => ScreenContext::CloseTabWithId,
            ScreenInstruction::RenameTabWithId(..) => ScreenContext::RenameTabWithId,
            ScreenInstruction::SetTabProjects(..) => ScreenContext::SetTabProjects,
            ScreenInstruction::BreakPanesToTabWithId { .. } => ScreenContext::BreakPanesToTabWithId,
            ScreenInstruction::RecomputeTabSize(..) => ScreenContext::RecomputeTabSize,
            ScreenInstruction::TerminalPixelDimensions(..) => {
                ScreenContext::TerminalPixelDimensions
            },
            ScreenInstruction::TerminalBackgroundColor(..) => {
                ScreenContext::TerminalBackgroundColor
            },
            ScreenInstruction::TerminalForegroundColor(..) => {
                ScreenContext::TerminalForegroundColor
            },
            ScreenInstruction::TerminalColorRegisters(..) => ScreenContext::TerminalColorRegisters,
            ScreenInstruction::SetKittyGraphicsSupport { .. } => {
                ScreenContext::SetKittyGraphicsSupport
            },
            ScreenInstruction::SetSixelSupport { .. } => ScreenContext::SetSixelSupport,
            ScreenInstruction::ForwardHostQuery { .. } => ScreenContext::ForwardHostQuery,
            ScreenInstruction::NestedSessionMessageFromPane { .. } => {
                ScreenContext::NestedSessionMessageFromPane
            },
            ScreenInstruction::NestedGuestPingTick { .. } => ScreenContext::NestedGuestPingTick,
            ScreenInstruction::NestedSessionMessageFromHost { .. } => {
                ScreenContext::NestedSessionMessageFromHost
            },
            ScreenInstruction::GuestModalChoice { .. } => ScreenContext::GuestModalChoice,
            ScreenInstruction::ForwardedReplyFromHost { .. } => {
                ScreenContext::ForwardedReplyFromHost
            },
            ScreenInstruction::ResumePaneAfterForward { .. } => {
                ScreenContext::ResumePaneAfterForward
            },
            ScreenInstruction::HostTerminalThemeChanged(..) => {
                ScreenContext::HostTerminalThemeChanged
            },
            ScreenInstruction::SetDarkTheme(..) => ScreenContext::SetDarkTheme,
            ScreenInstruction::SetLightTheme(..) => ScreenContext::SetLightTheme,
            ScreenInstruction::ToggleTheme(..) => ScreenContext::ToggleTheme,
            ScreenInstruction::ChangeMode(..) => ScreenContext::ChangeMode,
            ScreenInstruction::ChangeModeForAllClients(..) => {
                ScreenContext::ChangeModeForAllClients
            },
            ScreenInstruction::ToggleActiveSyncTab(..) => ScreenContext::ToggleActiveSyncTab,
            ScreenInstruction::ScrollUpAt(..) => ScreenContext::ScrollUpAt,
            ScreenInstruction::ScrollDownAt(..) => ScreenContext::ScrollDownAt,
            ScreenInstruction::MouseEvent(..) => ScreenContext::MouseEvent,
            ScreenInstruction::Copy(..) => ScreenContext::Copy,
            ScreenInstruction::ToggleTab(..) => ScreenContext::ToggleTab,
            ScreenInstruction::AddClient(..) => ScreenContext::AddClient,
            ScreenInstruction::RemoveClient(..) => ScreenContext::RemoveClient,
            ScreenInstruction::UpdateSearch(..) => ScreenContext::UpdateSearch,
            ScreenInstruction::SearchDown(..) => ScreenContext::SearchDown,
            ScreenInstruction::SearchUp(..) => ScreenContext::SearchUp,
            ScreenInstruction::SearchToggleCaseSensitivity(..) => {
                ScreenContext::SearchToggleCaseSensitivity
            },
            ScreenInstruction::SearchToggleWholeWord(..) => ScreenContext::SearchToggleWholeWord,
            ScreenInstruction::SearchToggleWrap(..) => ScreenContext::SearchToggleWrap,
            ScreenInstruction::AddRedPaneFrameColorOverride(..) => {
                ScreenContext::AddRedPaneFrameColorOverride
            },
            ScreenInstruction::ClearPaneFrameColorOverride(..) => {
                ScreenContext::ClearPaneFrameColorOverride
            },
            ScreenInstruction::SetTabBellFlash(..) => ScreenContext::SetTabBellFlash,
            ScreenInstruction::PreviousSwapLayout(..) => ScreenContext::PreviousSwapLayout,
            ScreenInstruction::NextSwapLayout(..) => ScreenContext::NextSwapLayout,
            ScreenInstruction::OverrideLayout(..) => ScreenContext::OverrideLayout,
            ScreenInstruction::OverrideLayoutComplete(..) => ScreenContext::OverrideLayoutComplete,
            ScreenInstruction::QueryTabNames(..) => ScreenContext::QueryTabNames,
            ScreenInstruction::NewTiledPluginPane(..) => ScreenContext::NewTiledPluginPane,
            ScreenInstruction::NewFloatingPluginPane(..) => ScreenContext::NewFloatingPluginPane,
            ScreenInstruction::StartOrReloadPluginPane(..) => {
                ScreenContext::StartOrReloadPluginPane
            },
            ScreenInstruction::AddPlugin(..) => ScreenContext::AddPlugin,
            ScreenInstruction::UpdatePluginLoadingStage(..) => {
                ScreenContext::UpdatePluginLoadingStage
            },
            ScreenInstruction::ProgressPluginLoadingOffset(..) => {
                ScreenContext::ProgressPluginLoadingOffset
            },
            ScreenInstruction::StartPluginLoadingIndication(..) => {
                ScreenContext::StartPluginLoadingIndication
            },
            ScreenInstruction::RequestStateUpdateForPlugins => {
                ScreenContext::RequestStateUpdateForPlugins
            },
            ScreenInstruction::LaunchOrFocusPlugin(..) => ScreenContext::LaunchOrFocusPlugin,
            ScreenInstruction::LaunchPlugin(..) => ScreenContext::LaunchPlugin,
            ScreenInstruction::SuppressPane(..) => ScreenContext::SuppressPane,
            ScreenInstruction::UnsuppressPane(..) => ScreenContext::UnsuppressPane,
            ScreenInstruction::UnsuppressOrExpandPane(..) => ScreenContext::UnsuppressOrExpandPane,
            ScreenInstruction::FocusPaneWithId(..) => ScreenContext::FocusPaneWithId,
            ScreenInstruction::RenamePane(..) => ScreenContext::RenamePane,
            ScreenInstruction::RenameActivePane(..) => ScreenContext::RenameActivePane,
            ScreenInstruction::RenameTab(..) => ScreenContext::RenameTab,
            ScreenInstruction::RequestPluginPermissions(..) => {
                ScreenContext::RequestPluginPermissions
            },
            ScreenInstruction::BreakPane(..) => ScreenContext::BreakPane,
            ScreenInstruction::BreakPaneRight(..) => ScreenContext::BreakPaneRight,
            ScreenInstruction::BreakPaneLeft(..) => ScreenContext::BreakPaneLeft,
            ScreenInstruction::WarpPane { .. } => ScreenContext::WarpPane,
            ScreenInstruction::WarpTab { .. } => ScreenContext::WarpTab,
            ScreenInstruction::TabReleasedForWarp { .. } => ScreenContext::TabReleasedForWarp,
            ScreenInstruction::AdoptedTabReady { .. } => ScreenContext::AdoptedTabReady,
            ScreenInstruction::PaneReleasedForWarp { .. } => ScreenContext::PaneReleasedForWarp,
            ScreenInstruction::AdoptedPaneReady { .. } => ScreenContext::AdoptedPaneReady,
            ScreenInstruction::UpdateSessionInfos(..) => ScreenContext::UpdateSessionInfos,
            ScreenInstruction::ReplacePane(..) => ScreenContext::ReplacePane,
            ScreenInstruction::NewInPlacePluginPane(..) => ScreenContext::NewInPlacePluginPane,
            ScreenInstruction::SerializeLayoutForResurrection => {
                ScreenContext::SerializeLayoutForResurrection
            },
            ScreenInstruction::RenameSession(..) => ScreenContext::RenameSession,
            ScreenInstruction::Notify(..) => ScreenContext::Notify,
            ScreenInstruction::DismissNotification(..) => ScreenContext::DismissNotification,
            ScreenInstruction::TickNotificationAnimations => {
                ScreenContext::TickNotificationAnimations
            },
            ScreenInstruction::ListClientsMetadata(..) => ScreenContext::ListClientsMetadata,
            ScreenInstruction::ListPanes { .. } => ScreenContext::ListPanes,
            ScreenInstruction::ListNotifications { .. } => ScreenContext::ListNotifications,
            ScreenInstruction::ListTabs { .. } => ScreenContext::ListTabs,
            ScreenInstruction::GetCurrentTabInfo { .. } => ScreenContext::GetCurrentTabInfo,
            ScreenInstruction::Reconfigure { .. } => ScreenContext::Reconfigure,
            ScreenInstruction::RerunCommandPane { .. } => ScreenContext::RerunCommandPane,
            ScreenInstruction::ResizePaneWithId(..) => ScreenContext::ResizePaneWithId,
            ScreenInstruction::EditScrollbackForPaneWithId(..) => {
                ScreenContext::EditScrollbackForPaneWithId
            },
            ScreenInstruction::WriteToPaneId(..) => ScreenContext::WriteToPaneId,
            ScreenInstruction::Paste(..) => ScreenContext::Paste,
            ScreenInstruction::SetPaneColor(..) => ScreenContext::SetPaneColor,
            ScreenInstruction::WriteKeyToPaneId(..) => ScreenContext::WriteKeyToPaneId,
            ScreenInstruction::CopyTextToClipboard(..) => ScreenContext::CopyTextToClipboard,
            ScreenInstruction::MovePaneWithPaneId(..) => ScreenContext::MovePaneWithPaneId,
            ScreenInstruction::MovePaneWithPaneIdInDirection(..) => {
                ScreenContext::MovePaneWithPaneIdInDirection
            },
            ScreenInstruction::ClearScreenForPaneId(..) => ScreenContext::ClearScreenForPaneId,
            ScreenInstruction::ScrollUpInPaneId(..) => ScreenContext::ScrollUpInPaneId,
            ScreenInstruction::ScrollDownInPaneId(..) => ScreenContext::ScrollDownInPaneId,
            ScreenInstruction::ScrollToTopInPaneId(..) => ScreenContext::ScrollToTopInPaneId,
            ScreenInstruction::ScrollToBottomInPaneId(..) => ScreenContext::ScrollToBottomInPaneId,
            ScreenInstruction::PageScrollUpInPaneId(..) => ScreenContext::PageScrollUpInPaneId,
            ScreenInstruction::PageScrollDownInPaneId(..) => ScreenContext::PageScrollDownInPaneId,
            ScreenInstruction::TogglePaneIdFullscreen(..) => ScreenContext::TogglePaneIdFullscreen,
            ScreenInstruction::SetMobileRenderPreferences { .. } => {
                ScreenContext::SetMobileRenderPreferences
            },
            ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(..) => {
                ScreenContext::TogglePaneEmbedOrEjectForPaneId
            },
            ScreenInstruction::CloseTabWithIndex(..) => ScreenContext::CloseTabWithIndex,
            ScreenInstruction::BreakPanesToNewTab { .. } => ScreenContext::BreakPanesToNewTab,
            ScreenInstruction::BreakPanesToTabWithIndex { .. } => {
                ScreenContext::BreakPanesToTabWithIndex
            },
            ScreenInstruction::ListClientsToPlugin(..) => ScreenContext::ListClientsToPlugin,
            ScreenInstruction::TogglePanePinned(..) => ScreenContext::TogglePanePinned,
            ScreenInstruction::SetFloatingPanePinned(..) => ScreenContext::SetFloatingPanePinned,
            ScreenInstruction::StackPanes(..) => ScreenContext::StackPanes,
            ScreenInstruction::ChangeOverlayCoordinates(..) => {
                ScreenContext::ChangeOverlayCoordinates
            },
            ScreenInstruction::ChangeFloatingPanesCoordinates(..) => {
                ScreenContext::ChangeFloatingPanesCoordinates
            },
            ScreenInstruction::TogglePaneBorderless(..) => ScreenContext::TogglePaneBorderless,
            ScreenInstruction::SetPaneBorderless(..) => ScreenContext::SetPaneBorderless,
            ScreenInstruction::AddHighlightPaneFrameColorOverride(..) => {
                ScreenContext::AddHighlightPaneFrameColorOverride
            },
            ScreenInstruction::GroupAndUngroupPanes(..) => ScreenContext::GroupAndUngroupPanes,
            ScreenInstruction::HighlightAndUnhighlightPanes(..) => {
                ScreenContext::HighlightAndUnhighlightPanes
            },
            ScreenInstruction::RevealPaneWithId(..) => ScreenContext::RevealPaneWithId,
            ScreenInstruction::FloatMultiplePanes(..) => ScreenContext::FloatMultiplePanes,
            ScreenInstruction::EmbedMultiplePanes(..) => ScreenContext::EmbedMultiplePanes,
            ScreenInstruction::TogglePaneInGroup(..) => ScreenContext::TogglePaneInGroup,
            ScreenInstruction::ToggleGroupMarking(..) => ScreenContext::ToggleGroupMarking,
            ScreenInstruction::SessionSharingStatusChange(..) => {
                ScreenContext::SessionSharingStatusChange
            },
            ScreenInstruction::SetMouseSelectionSupport(..) => {
                ScreenContext::SetMouseSelectionSupport
            },
            ScreenInstruction::InterceptKeyPresses(..) => ScreenContext::InterceptKeyPresses,
            ScreenInstruction::ClearKeyPressesIntercepts(..) => {
                ScreenContext::ClearKeyPressesIntercepts
            },
            ScreenInstruction::ReplacePaneWithExistingPane(..) => {
                ScreenContext::ReplacePaneWithExistingPane
            },
            ScreenInstruction::AddWatcherClient(..) => ScreenContext::AddWatcherClient,
            ScreenInstruction::RemoveWatcherClient(..) => ScreenContext::RemoveWatcherClient,
            ScreenInstruction::SetFollowedClient(..) => ScreenContext::SetFollowedClient,
            ScreenInstruction::WatcherTerminalResize(..) => ScreenContext::WatcherTerminalResize,
            ScreenInstruction::ClearMouseHelpText(..) => ScreenContext::ClearMouseHelpText,
            ScreenInstruction::UpdateAvailableLayouts(..) => ScreenContext::UpdateAvailableLayouts,
            ScreenInstruction::SetPluginRegexHighlights { .. } => {
                ScreenContext::SetPluginRegexHighlights
            },
            ScreenInstruction::ClearPluginHighlights { .. } => ScreenContext::ClearPluginHighlights,
            ScreenInstruction::ClearAllPluginHighlights(..) => ScreenContext::ClearPluginHighlights,
            ScreenInstruction::DesktopNotificationResponse(..) => {
                ScreenContext::DesktopNotificationResponse
            },
            ScreenInstruction::SubscribeToPaneRenders { .. } => {
                ScreenContext::SubscribeToPaneRenders
            },
            ScreenInstruction::NotifyPaneClosedToSubscribers { .. } => {
                ScreenContext::NotifyPaneClosedToSubscribers
            },
            ScreenInstruction::UpdateBackgroundPluginSubscriptions(..) => {
                ScreenContext::UpdateBackgroundPluginSubscriptions
            },
            ScreenInstruction::ClearHintTextCache => ScreenContext::ClearHintTextCache,
            ScreenInstruction::BroadcastModeUpdate(..) => ScreenContext::BroadcastModeUpdate,
            // Pane-targeting CLI variants
            ScreenInstruction::ScrollUpWithPaneId(..) => ScreenContext::ScrollUpWithPaneId,
            ScreenInstruction::ScrollDownWithPaneId(..) => ScreenContext::ScrollDownWithPaneId,
            ScreenInstruction::ScrollToTopWithPaneId(..) => ScreenContext::ScrollToTopWithPaneId,
            ScreenInstruction::ScrollToBottomWithPaneId(..) => {
                ScreenContext::ScrollToBottomWithPaneId
            },
            ScreenInstruction::PageScrollUpWithPaneId(..) => ScreenContext::PageScrollUpWithPaneId,
            ScreenInstruction::PageScrollDownWithPaneId(..) => {
                ScreenContext::PageScrollDownWithPaneId
            },
            ScreenInstruction::HalfPageScrollUpWithPaneId(..) => {
                ScreenContext::HalfPageScrollUpWithPaneId
            },
            ScreenInstruction::HalfPageScrollDownWithPaneId(..) => {
                ScreenContext::HalfPageScrollDownWithPaneId
            },
            ScreenInstruction::ResizeWithPaneId(..) => ScreenContext::ResizeWithPaneId,
            ScreenInstruction::SetGlobalPanePlacement(..) => ScreenContext::ResizeWithPaneId,
            ScreenInstruction::MovePaneWithPaneIdCli(..) => ScreenContext::MovePaneWithPaneIdCli,
            ScreenInstruction::MovePaneBackwardsWithPaneId(..) => {
                ScreenContext::MovePaneBackwardsWithPaneId
            },
            ScreenInstruction::ClearScreenWithPaneId(..) => ScreenContext::ClearScreenWithPaneId,
            ScreenInstruction::EditScrollbackWithPaneId(..) => {
                ScreenContext::EditScrollbackWithPaneId
            },
            ScreenInstruction::ToggleFullscreenWithPaneId(..) => {
                ScreenContext::ToggleFullscreenWithPaneId
            },
            ScreenInstruction::ToggleNoUiFullscreenWithPaneId(..) => {
                ScreenContext::ToggleNoUiFullscreenWithPaneId
            },
            ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(..) => {
                ScreenContext::TogglePaneEmbedOrFloatingWithPaneId
            },
            ScreenInstruction::CloseFocusWithPaneId(..) => ScreenContext::CloseFocusWithPaneId,
            ScreenInstruction::RenamePaneWithPaneId(..) => ScreenContext::RenamePaneWithPaneId,
            ScreenInstruction::UndoRenamePaneWithPaneId(..) => {
                ScreenContext::UndoRenamePaneWithPaneId
            },
            ScreenInstruction::TogglePanePinnedWithPaneId(..) => {
                ScreenContext::TogglePanePinnedWithPaneId
            },
            // Tab-targeting CLI variants
            ScreenInstruction::UndoRenameTabWithTabId(..) => ScreenContext::UndoRenameTabWithTabId,
            ScreenInstruction::ToggleActiveSyncTabWithTabId(..) => {
                ScreenContext::ToggleActiveSyncTabWithTabId
            },
            ScreenInstruction::ToggleFloatingPanesWithTabId(..) => {
                ScreenContext::ToggleFloatingPanesWithTabId
            },
            ScreenInstruction::PreviousSwapLayoutWithTabId(..) => {
                ScreenContext::PreviousSwapLayoutWithTabId
            },
            ScreenInstruction::NextSwapLayoutWithTabId(..) => {
                ScreenContext::NextSwapLayoutWithTabId
            },
            ScreenInstruction::MoveTabWithTabId(..) => ScreenContext::MoveTabWithTabId,
            ScreenInstruction::SetSoftKeyboard { .. } => ScreenContext::SetSoftKeyboard,
            ScreenInstruction::FocusHostSession(..) => ScreenContext::FocusHostSession,
            ScreenInstruction::FocusGuestSession(..) => ScreenContext::FocusGuestSession,
            ScreenInstruction::ToggleHostFullscreen(..) => ScreenContext::ToggleHostFullscreen,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CopyOptions {
    pub command: Option<String>,
    pub clipboard: Clipboard,
    pub copy_on_select: bool,
}

impl CopyOptions {
    pub(crate) fn new(
        copy_command: Option<String>,
        copy_clipboard: Clipboard,
        copy_on_select: bool,
    ) -> Self {
        Self {
            command: copy_command,
            clipboard: copy_clipboard,
            copy_on_select,
        }
    }

    #[cfg(test)]
    pub(crate) fn default() -> Self {
        Self {
            command: None,
            clipboard: Clipboard::default(),
            copy_on_select: true,
        }
    }
}

// We use this to delay rendering when a new tab opens so that we make sure all plugins
// (representing portions of the UI) have been fully loaded before the tab is first rendered (with
// a sensible timeout of 100ms)
#[derive(Debug, Clone)]
pub struct RenderBlocker {
    blocking_plugins: HashMap<u32, Instant>,
    #[cfg_attr(test, allow(dead_code))]
    timeout_ms: u64,
}

impl RenderBlocker {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            blocking_plugins: HashMap::new(),
            timeout_ms,
        }
    }

    pub fn register_blocking_plugin(&mut self, plugin_id: u32) {
        self.blocking_plugins.insert(plugin_id, Instant::now());
    }

    pub fn remove_blocking_plugin(&mut self, plugin_id: u32) {
        self.blocking_plugins.remove(&plugin_id);
    }

    #[cfg(test)]
    pub fn can_render(&mut self) -> bool {
        // we want the tests to be more deterministic and so we always render without any
        // optimizations
        true
    }

    #[cfg(not(test))]
    pub fn can_render(&mut self) -> bool {
        let ret = if self.blocking_plugins.is_empty() {
            true
        } else {
            let timeout = Duration::from_millis(self.timeout_ms);
            let now = Instant::now();

            self.blocking_plugins
                .values()
                .all(|&registered_at| now.duration_since(registered_at) >= timeout)
        };
        if ret {
            self.blocking_plugins.clear();
        }
        ret
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WatcherState {
    size: Size,
    should_force_render: bool,
}

impl WatcherState {
    pub fn new(size: Size) -> Self {
        WatcherState {
            size,
            should_force_render: true,
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    pub fn should_force_render(&self) -> bool {
        self.should_force_render
    }

    pub fn clear_force_render(&mut self) {
        self.should_force_render = false;
    }

    pub fn set_force_render(&mut self) {
        self.should_force_render = true;
    }
}

struct PaneRenderSubscription {
    pane_ids: HashSet<zellij_utils::data::PaneId>,
    previous_viewports: HashMap<zellij_utils::data::PaneId, Vec<String>>,
    ansi: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NestedGuestChoice {
    Zoom,
    Descend,
    Dismissed,
}

impl NestedGuestChoice {
    fn enters_passthrough(&self) -> bool {
        matches!(self, NestedGuestChoice::Zoom | NestedGuestChoice::Descend)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestModalOutcome {
    Zoom,
    Descend,
}

fn format_guest_modal_shortcut(keys: &[KeyWithModifier]) -> Vec<String> {
    keys.iter().map(|key| key.to_string()).collect()
}

/// A [`Screen`] holds multiple [`Tab`]s, each one holding multiple [`panes`](crate::client::panes).
/// It only directly controls which tab is active, delegating the rest to the individual `Tab`.
/// Everything about a tab warp that could be settled before anything was
/// released.
///
/// Held between `check_tab_can_be_warped` and `take_released_tab_for_warp`,
/// which is the whole reason it exists: the arrangement is the fallible part,
/// and working it out first means a tab that cannot be rebuilt is refused while
/// it is still a tab rather than after its supervisors have been let go.
///
/// The pane lists are in the order the receiving session will fill its slots,
/// carrying the two presentation details that do not change between here and the
/// release. Contents are not among them — they are read afterwards, once the
/// screen has stopped moving.
#[derive(Debug, Clone)]
pub(crate) struct PreparedTabWarp {
    name: Option<String>,
    tiled_layout: TiledPaneLayout,
    floating_layout: Vec<FloatingPaneLayout>,
    tiled_panes: Vec<PaneId>,
    floating_panes: Vec<PaneId>,
    hide_floating_panes: bool,
}

/// Where a warped pane's process can be picked up, whichever machine it is on.
///
/// The three `WarpedPane` fields that say "attach here", pulled out so the pane
/// and tab builders share one answer to the question. Exactly one of the two
/// shapes is meaningful: a local supervisor socket, or a broker and a token.
#[derive(Debug, Clone, Default)]
pub(crate) struct WarpAddress {
    host_id: String,
    control_socket: PathBuf,
    remote: Option<RemotePaneHandoff>,
}

/// What [`Screen::break_multiple_panes_to_tab_with_id`] actually did.
///
/// The plugin API reports a move as `Some(tab_id)` and a failure as `None`, and it must
/// not lie in either direction: a silent no-op reported as success is what makes an
/// agent reason about a session state that never happened. Callers get enough detail
/// here to log something actionable, and [`Self::affected_tab_id`] gives them the one
/// value the wire format can carry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BreakPanesToTabOutcome {
    /// `None` when there is no tab with the requested id. Nothing was extracted and the
    /// session is unchanged.
    pub destination_tab_id: Option<usize>,
    /// Panes extracted from another tab and now in the destination.
    pub moved: Vec<PaneId>,
    /// Panes that were already in the destination and were left alone.
    pub already_in_destination: Vec<PaneId>,
    /// Requested panes that are in no tab in this session.
    pub not_found: Vec<PaneId>,
    /// Panes the destination had no room for; each was put back in its source tab.
    pub returned_to_source: Vec<PaneId>,
    /// Panes that could be re-homed nowhere and were torn down properly rather than
    /// dropped. Should always be empty; non-empty means a bug, but not a leaked process.
    pub closed: Vec<PaneId>,
}

impl BreakPanesToTabOutcome {
    /// Whether every requested pane is now in the destination tab.
    ///
    /// A partial move counts as a failure. The panes that did move stay where they
    /// landed, but the caller is told "no", so it re-reads the session rather than
    /// trusting a half-applied result.
    pub fn succeeded(&self) -> bool {
        self.destination_tab_id.is_some()
            && self.not_found.is_empty()
            && self.returned_to_source.is_empty()
            && self.closed.is_empty()
            && !(self.moved.is_empty() && self.already_in_destination.is_empty())
    }
    /// The value the plugin API reports back: `Some(tab_id)` only on a real move.
    pub fn affected_tab_id(&self) -> Option<usize> {
        if self.succeeded() {
            self.destination_tab_id
        } else {
            None
        }
    }
}

pub(crate) struct Screen {
    /// A Bus for sending and receiving messages with the other threads.
    pub bus: Bus<ScreenInstruction>,
    /// An optional maximal amount of panes allowed per [`Tab`] in this [`Screen`] instance.
    max_panes: Option<usize>,
    /// A map between this [`Screen`]'s tabs and their ID/key.
    tabs: BTreeMap<usize, Tab>,
    /// Where each terminal pane's process is, as pty last reported it. Stamped
    /// onto every pane manifest so a plugin that missed the announcement — or
    /// loaded after it — is put right by the next update rather than never.
    terminal_cwds: HashMap<u32, PathBuf>,
    /// The machine each remote pane runs on. Absent for local panes, which is
    /// nearly all of them.
    terminal_hosts: HashMap<u32, String>,
    pub(crate) global_panes: GlobalPaneManager,
    /// The name each tab last showed a plugin, so a change to one that no other
    /// event reports — a lone pane retitling itself, a project arriving, a
    /// collision forming or clearing — still redraws the tab bar.
    last_plugin_tab_names: HashMap<usize, String>,
    /// The project each tab is working on, as the last `SetTabProjects` had it.
    ///
    /// Only a plugin knows what a project is, so this is told to us rather than
    /// worked out. Held raw — without the `#1`/`#2` a collision earns — because
    /// the suffixes depend on tab order, which changes without anyone sending us
    /// anything.
    tab_projects: HashMap<usize, String>,
    pixel_dimensions: PixelDimensions,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    stacked_resize: Rc<RefCell<bool>>,
    stacked_pane_list: Rc<RefCell<bool>>,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    kitty_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    sixel_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    client_kitty_host_state: Rc<RefCell<HashMap<ClientId, HostKittyState>>>,
    terminal_emulator_colors: Rc<RefCell<Palette>>,
    terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
    connected_clients: Rc<RefCell<HashMap<ClientId, bool>>>, // bool -> is_web_client
    /// The indices of this [`Screen`]'s active [`Tab`]s.
    active_tab_ids: BTreeMap<ClientId, usize>,
    client_sizes: HashMap<ClientId, Size>,
    /// The size the session was built with, and the only size there is until the first client
    /// reports its own. Upstream's `Screen::size` was seeded here and so was always available to
    /// lay out against; `client_sizes` starts empty, and a render tick that lands before the first
    /// `RecomputeTabSize` would otherwise have `surface_size` fall back to a zero size - which
    /// `allocate_global_panes` rejects, killing the screen thread on a frame nobody could see.
    initial_size: Size,
    /// The `unowned_strip_rows` last painted for each client. `Output` re-emits every chunk it is
    /// given and `Output::is_dirty` counts them, so filling these unconditionally would send a frame
    /// on every render tick (and force every watcher into a full repaint) - this is the diff.
    unowned_strip_fill: HashMap<ClientId, Vec<(usize, usize, usize)>>,
    /// The floating pane geometries each tab was last rendered with. The global panes paint
    /// clipped against these, so the cells a floating pane covers are dropped from their chunks
    /// and have no grid diff of their own - once the pane moves off them, only a full repaint
    /// replaces the floating pane's contents with the global pane's.
    last_rendered_floating_stacks: BTreeMap<usize, Vec<PaneGeom>>,
    global_last_active_tab_id: usize,
    tab_history: BTreeMap<ClientId, Vec<usize>>,
    pane_history: BTreeMap<ClientId, Vec<PaneId>>,
    mode_info: BTreeMap<ClientId, ModeInfo>,
    default_mode_info: ModeInfo, // TODO: restructure ModeInfo to prevent this duplication
    style: Style,
    pane_frame_style: PaneFrameStyle,
    /// Live toast notifications, newest first (index 0 sits in the configured corner).
    notifications: Vec<Notification>,
    notification_mouse_captures: HashSet<ClientId>,
    /// Every toast received by this live session. Dismissal only affects `notifications` above.
    /// ponytail: intentionally unbounded because the feature promises "all" notifications;
    /// add a configurable cap only if long-lived sessions show meaningful memory pressure.
    notification_history: Vec<NotificationInfo>,
    next_notification_id: u64,
    notification_position: NotificationPosition,
    notification_timeout_secs: u64,
    /// The notification stack as it was just before the last time it changed (a toast arrived or
    /// one was finally removed after sliding out) - used to animate the survivors gliding from
    /// their old stacked position to their new one. See `render_notifications`.
    notification_stack_before_change: Vec<Notification>,
    /// `0.0` while the stack is still at `notification_stack_before_change`'s layout, `1.0` once
    /// it has fully settled at `notifications`' layout.
    notification_displacement_progress: f32,
    auto_layout: bool,
    session_serialization: bool,
    serialize_pane_viewport: bool,
    scrollback_lines_to_serialize: Option<usize>,
    /// Warps waiting on the pty thread, keyed by the terminal id being released.
    ///
    /// A warp takes two hops — ask pty to let the supervisor go, then serialize
    /// the pane once it has — and whoever is waiting on the far end has to still
    /// be reachable from the second hop.
    pending_warps: HashMap<u32, crossbeam::channel::Sender<Result<Box<WarpedPane>, String>>>,
    /// The same idea as `pending_warps`, keyed by tab id rather than terminal id
    /// because a tab's release is answered once for the whole tab — and carrying
    /// the arrangement, which was worked out before the release so that a tab
    /// that could not be rebuilt was never taken apart.
    pending_tab_warps: HashMap<
        usize,
        (
            crossbeam::channel::Sender<Result<Box<WarpedTab>, String>>,
            PreparedTabWarp,
        ),
    >,
    session_is_mirrored: bool,
    copy_options: CopyOptions,
    debug: bool,
    session_name: String,
    peer_sessions_cache: BTreeMap<String, SessionInfo>, // String is the session name, can
    // also be this session
    resurrectable_sessions_cache: BTreeMap<String, Duration>, // String is the session name,
    // duration is its creation time
    default_layout: Box<Layout>,
    default_shell: PathBuf,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    arrow_fonts: bool,
    #[cfg_attr(test, allow(dead_code))]
    layout_dir: Option<PathBuf>,
    #[cfg_attr(test, allow(dead_code))]
    default_layout_name: Option<String>,
    explicitly_disable_kitty_keyboard_protocol: bool,
    support_kitty_graphics_protocol: bool,
    default_editor: Option<PathBuf>,
    web_clients_allowed: bool,
    web_sharing: WebSharing,
    current_pane_group: Rc<RefCell<PaneGroups>>,
    advanced_mouse_actions: bool,
    osc133_command_selection: bool,
    word_separators: String,
    mouse_scroll_resize: bool,
    mouse_hover_effects: bool,
    visual_bell: bool,
    focus_follows_mouse: bool,
    mouse_click_through: bool,
    currently_marking_pane_group: Rc<RefCell<HashMap<ClientId, bool>>>,
    // the below are the configured values - the ones that will be set if and when the web server
    // is brought online
    web_server_ip: IpAddr,
    web_server_port: u16,
    render_blocker: RenderBlocker,
    watcher_clients: HashMap<ClientId, WatcherState>,
    followed_client_id: Option<ClientId>,
    cached_layouts: Vec<LayoutInfo>,
    cached_layout_errors: Vec<LayoutWithError>,
    pane_render_subscribers: HashMap<ClientId, PaneRenderSubscription>,
    background_plugin_subscriptions: HashMap<(PluginId, ClientId), HashSet<EventType>>,
    next_forward_token: u32,
    pending_forwarded_queries: HashMap<u32, PendingForwardEntry>,
    forward_queue: VecDeque<PendingForward>,
    forward_in_flight_token: Option<u32>,
    pending_clipboard_forwards: HashMap<u32, PendingForwardEntry>,
    clipboard_forward_queue: VecDeque<PendingForward>,
    clipboard_forward_in_flight_token: Option<u32>,
    paste_buffer_read_enabled: bool,
    nested_guest_tracker: NestedGuestTracker,
    nested_ancestry: Vec<String>,
    nested_via_client_id: Option<ClientId>,
    nested_fullscreen_panes: HashSet<PaneId>,
    own_fullscreen_requested: bool,
    reported_guest_fullscreen: HashMap<PaneId, bool>,
    host_fullscreen: bool,
    dimmed_clients: HashSet<ClientId>,
    nested_guest_choices: HashMap<(ClientId, PaneId), NestedGuestChoice>,
    guest_ascend_keys: HashMap<PaneId, Vec<KeyWithModifier>>,
    host_descend_keys: Vec<KeyWithModifier>,
    host_descended: bool,
    host_terminal_theme_mode: Option<HostTerminalThemeMode>,
    host_theme_dark_styling: Option<Styling>,
    host_theme_light_styling: Option<Styling>,
    nested_session_handling: NestedSessionHandling,
    last_mobile_state_sent: HashMap<ClientId, MobileStatePayload>,
    pane_output_activity: HashMap<PaneId, Instant>,
    layout_global_panes_started: bool,
    suspended_global_focus: HashMap<ClientId, (usize, PaneId)>,
    /// A layout-declared global pane with `focus=true` that spawned before any client attached -
    /// the focus lands on the first client to arrive instead of being dropped.
    pending_global_pane_focus: Option<PaneId>,
    mobile_web_prefs: HashMap<ClientId, MobileWebPrefs>,
}

/// Per-tick step for a toast's slide in/out, sized so the whole animation takes a handful of
/// `NOTIFICATION_ANIMATION_TICK_MS` ticks (background_jobs.rs) - a few hundred milliseconds.
const NOTIFICATION_HORIZONTAL_PROGRESS_STEP: f32 = 0.2;
/// Per-tick step for the shared displacement lerp (see `render_notifications`), same rationale.
const NOTIFICATION_DISPLACEMENT_STEP: f32 = 0.2;

/// A pending forward waiting to be dispatched once the current in-flight
/// forward's barrier reply (or timeout) arrives.
/// A live toast notification. See [`crate::ui::notification`] for how these are drawn.
#[derive(Debug, Clone)]
struct Notification {
    id: u64,
    text: String,
    pane_id: Option<PaneId>,
    /// `0.0` = hidden at the anchored edge (just inserted), `1.0` = fully slid into place.
    /// Advances toward `0.0` instead once `dismissing` is set, and the notification is dropped
    /// from the stack once it gets there. See `notification_reveal`.
    horizontal_progress: f32,
    /// Set once dismissal has been requested (by the timeout or otherwise); the notification stays
    /// in the stack; sliding out, until `horizontal_progress` reaches `0.0`.
    dismissing: bool,
}

impl Notification {
    fn new(id: u64, text: String, pane_id: Option<PaneId>) -> Self {
        Notification {
            id,
            text,
            pane_id,
            horizontal_progress: 0.0,
            dismissing: false,
        }
    }
}

struct NotificationLayout<'a> {
    notification: &'a Notification,
    lines: Vec<String>,
    x: usize,
    y: usize,
    width: usize,
    revealed_x: usize,
    visible_cols: usize,
}

#[derive(Debug, Clone)]
struct PendingForward {
    token: u32,
    pane_id: PaneId,
    query: crate::host_query::HostQuery,
}

/// A forward currently in flight (dispatched to the client, waiting
/// for a reply). Retains the `HostQuery` classification so we can
/// fall back to cache-synthesized replies when the live reply is
/// empty (no client attached, client-side timeout, etc.) without
/// re-parsing byte strings.
#[derive(Debug, Clone)]
struct PendingForwardEntry {
    pane_id: PaneId,
    query: crate::host_query::HostQuery,
}

/// Reserved sentinel token for Zellij's own startup batch of host
/// queries (pixel dims, bg/fg, sync-output, palette registers). The
/// existing fire-and-forget startup plumbing is unchanged today; the
/// sentinel exists so a future migration can route the startup batch
/// through the same forwarded-query mechanism as per-pane queries
/// while keeping its replies routable to `Screen`'s cached state
/// rather than to a pane's pty. Reserved value 0 is excluded from
/// `next_forward_token` allocation by the wrap-skip in
/// `forward_host_query`.
const STARTUP_SENTINEL_TOKEN: u32 = 0;

/// Server-side deadline for a forwarded host query. If no
/// `ForwardedReplyFromHost` arrives within this window, the server
/// synthesizes an empty reply for itself so the in-flight slot
/// releases and queued forwards continue to drain.
///
/// This recovers the worst-case where the connected client doesn't
/// understand `ServerToClientMsg::ForwardQueryToHost` (an old client
/// against a new server) — without it the slot would deadlock and
/// every app-issued host query would hang. The window is generous
/// relative to the 500 ms client-side deadline so a well-behaved new
/// client always replies first; only the old-client and
/// network-pathological cases ever see this fire.
const SERVER_FORWARD_TIMEOUT_MS: u64 = 1000;

const SERVER_CLIPBOARD_FORWARD_TIMEOUT_MS: u64 = 35_000;

impl Screen {
    /// Creates and returns a new [`Screen`].
    pub fn new(
        bus: Bus<ScreenInstruction>,
        client_attributes: &ClientAttributes,
        max_panes: Option<usize>,
        mode_info: ModeInfo,
        pane_frame_style: PaneFrameStyle,
        auto_layout: bool,
        session_is_mirrored: bool,
        copy_options: CopyOptions,
        debug: bool,
        default_layout: Box<Layout>,
        default_layout_name: Option<String>,
        default_shell: PathBuf,
        session_serialization: bool,
        serialize_pane_viewport: bool,
        scrollback_lines_to_serialize: Option<usize>,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        arrow_fonts: bool,
        layout_dir: Option<PathBuf>,
        explicitly_disable_kitty_keyboard_protocol: bool,
        support_kitty_graphics_protocol: bool,
        stacked_resize: bool,
        stacked_pane_list: bool,
        default_editor: Option<PathBuf>,
        web_clients_allowed: bool,
        web_sharing: WebSharing,
        advanced_mouse_actions: bool,
        osc133_command_selection: bool,
        word_separators: String,
        mouse_scroll_resize: bool,
        mouse_hover_effects: bool,
        visual_bell: bool,
        focus_follows_mouse: bool,
        mouse_click_through: bool,
        web_server_ip: IpAddr,
        web_server_port: u16,
        nested_session_handling: NestedSessionHandling,
        notification_position: NotificationPosition,
        notification_timeout_secs: u64,
    ) -> Self {
        let session_name = mode_info.session_name.clone().unwrap_or_default();
        let session_info = SessionInfo::new(session_name.clone());
        let mut peer_sessions_cache = BTreeMap::new();
        let resurrectable_sessions_cache = BTreeMap::new();
        peer_sessions_cache.insert(session_name.clone(), session_info);
        let current_pane_group = PaneGroups::new(bus.senders.clone());
        let character_cell_size = Rc::new(RefCell::new(None));
        let global_panes = GlobalPaneManager::new(
            bus.senders.clone(),
            character_cell_size.clone(),
            pane_frame_style,
        );
        Screen {
            bus,
            max_panes,
            pixel_dimensions: Default::default(),
            character_cell_size,
            stacked_resize: Rc::new(RefCell::new(stacked_resize)),
            stacked_pane_list: Rc::new(RefCell::new(stacked_pane_list)),
            sixel_image_store: Rc::new(RefCell::new(SixelImageStore::default())),
            kitty_image_store: Rc::new(RefCell::new(KittyImageStore::default())),
            kitty_host_capabilities: Rc::new(RefCell::new(HashMap::new())),
            sixel_host_capabilities: Rc::new(RefCell::new(HashMap::new())),
            client_kitty_host_state: Rc::new(RefCell::new(HashMap::new())),
            style: client_attributes.style,
            connected_clients: Rc::new(RefCell::new(HashMap::new())),
            active_tab_ids: BTreeMap::new(),
            client_sizes: HashMap::new(),
            initial_size: client_attributes.size,
            unowned_strip_fill: HashMap::new(),
            last_rendered_floating_stacks: BTreeMap::new(),
            global_last_active_tab_id: 0,
            tabs: BTreeMap::new(),
            terminal_cwds: HashMap::new(),
            terminal_hosts: HashMap::new(),
            global_panes,
            last_plugin_tab_names: HashMap::new(),
            tab_projects: HashMap::new(),
            terminal_emulator_colors: Rc::new(RefCell::new(Palette::default())),
            terminal_emulator_color_codes: Rc::new(RefCell::new(HashMap::new())),
            tab_history: BTreeMap::new(),
            pane_history: BTreeMap::new(),
            mode_info: BTreeMap::new(),
            default_mode_info: mode_info,
            pane_frame_style,
            notifications: vec![],
            notification_mouse_captures: HashSet::new(),
            notification_history: vec![],
            next_notification_id: 0,
            notification_position,
            notification_timeout_secs,
            notification_stack_before_change: vec![],
            notification_displacement_progress: 1.0,
            auto_layout,
            session_is_mirrored,
            copy_options,
            debug,
            session_name,
            peer_sessions_cache,
            default_layout,
            default_layout_name,
            default_shell,
            session_serialization,
            serialize_pane_viewport,
            scrollback_lines_to_serialize,
            pending_warps: HashMap::new(),
            pending_tab_warps: HashMap::new(),
            styled_underlines,
            osc8_hyperlinks,
            arrow_fonts,
            resurrectable_sessions_cache,
            layout_dir,
            explicitly_disable_kitty_keyboard_protocol,
            support_kitty_graphics_protocol,
            default_editor,
            web_clients_allowed,
            web_sharing,
            current_pane_group: Rc::new(RefCell::new(current_pane_group)),
            currently_marking_pane_group: Rc::new(RefCell::new(HashMap::new())),
            advanced_mouse_actions,
            osc133_command_selection,
            word_separators,
            mouse_scroll_resize,
            mouse_hover_effects,
            visual_bell,
            focus_follows_mouse,
            mouse_click_through,
            web_server_ip,
            web_server_port,
            render_blocker: RenderBlocker::new(100),
            watcher_clients: HashMap::new(),
            followed_client_id: None,
            cached_layouts: vec![],
            cached_layout_errors: vec![],
            pane_render_subscribers: HashMap::new(),
            background_plugin_subscriptions: HashMap::new(),
            next_forward_token: 1, // 0 is reserved as the startup sentinel
            pending_forwarded_queries: HashMap::new(),
            forward_queue: VecDeque::new(),
            forward_in_flight_token: None,
            pending_clipboard_forwards: HashMap::new(),
            clipboard_forward_queue: VecDeque::new(),
            clipboard_forward_in_flight_token: None,
            paste_buffer_read_enabled: false,
            nested_guest_tracker: NestedGuestTracker::default(),
            nested_ancestry: vec![],
            nested_via_client_id: None,
            nested_fullscreen_panes: HashSet::new(),
            own_fullscreen_requested: false,
            reported_guest_fullscreen: HashMap::new(),
            host_fullscreen: false,
            dimmed_clients: HashSet::new(),
            nested_guest_choices: HashMap::new(),
            guest_ascend_keys: HashMap::new(),
            host_descend_keys: vec![],
            host_descended: false,
            host_terminal_theme_mode: None,
            host_theme_dark_styling: None,
            host_theme_light_styling: None,
            nested_session_handling,
            last_mobile_state_sent: HashMap::new(),
            pane_output_activity: HashMap::new(),
            layout_global_panes_started: false,
            suspended_global_focus: HashMap::new(),
            pending_global_pane_focus: None,
            mobile_web_prefs: HashMap::new(),
        }
    }

    fn client_is_web(&self, client_id: ClientId) -> bool {
        self.connected_clients
            .borrow()
            .get(&client_id)
            .copied()
            .unwrap_or(false)
    }

    fn get_new_tab_id(&self) -> usize {
        if let Some(id) = self.tabs.keys().last() {
            *id + 1
        } else {
            0
        }
    }

    /// Gets a tab by its stable ID (BTreeMap key).
    ///
    /// Use this when you have a tab ID from active_tab_ids, tab_history, or tab.id.
    fn get_tab_by_id(&self, id: usize) -> Option<&Tab> {
        self.tabs.get(&id)
    }

    /// Gets a mutable tab by its stable ID (BTreeMap key).
    fn get_tab_by_id_mut(&mut self, id: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(&id)
    }

    /// Gets a mutable tab by its display position (0-based).
    fn get_tab_by_position_mut(&mut self, position: usize) -> Option<&mut Tab> {
        self.tabs.values_mut().find(|t| t.position == position)
    }

    /// Gets any one tab, for operations that need *a* tab rather than a specific one.
    ///
    /// This is the lowest-id tab, which is not necessarily the leftmost one. Callers
    /// that want the leftmost tab want `get_tab_by_position_mut(0)`.
    pub fn first_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.values_mut().next()
    }

    /// Gets the stable ID of the tab at the given display position.
    ///
    /// Use this to convert position → ID for BTreeMap lookups.
    fn get_tab_id_at_position(&self, position: usize) -> Option<usize> {
        self.tabs
            .values()
            .find(|t| t.position == position)
            .map(|t| t.id)
    }

    /// Gets the display position of the tab with the given ID.
    ///
    /// Use this to convert ID → position for user-facing operations.
    fn get_tab_position_by_id(&self, id: usize) -> Option<usize> {
        self.tabs.get(&id).map(|t| t.position)
    }

    fn move_clients_from_closed_tab(
        &mut self,
        client_ids_and_mode_infos: Vec<(ClientId, ModeInfo)>,
    ) -> Result<()> {
        let err_context = || "failed to move clients from closed tab".to_string();

        if self.tabs.is_empty() {
            Err::<(), _>(anyhow!(
                "No tabs left, cannot move clients: {:?} from closed tab",
                client_ids_and_mode_infos
            ))
            .with_context(err_context)
            .non_fatal();

            return Ok(());
        }
        let first_tab_index = *self
            .tabs
            .keys()
            .next()
            .context("screen contained no tabs")
            .with_context(err_context)?;
        for (client_id, client_mode_info) in client_ids_and_mode_infos {
            let client_tab_history = self.tab_history.entry(client_id).or_default();
            if let Some(client_previous_tab) = client_tab_history.pop() {
                if let Some(client_active_tab) = self.tabs.get_mut(&client_previous_tab) {
                    self.active_tab_ids.insert(client_id, client_previous_tab);
                    client_active_tab
                        .add_client(client_id, Some(client_mode_info))
                        .with_context(err_context)?;
                    continue;
                }
            }
            self.active_tab_ids.insert(client_id, first_tab_index);
            self.tabs
                .get_mut(&first_tab_index)
                .with_context(err_context)?
                .add_client(client_id, Some(client_mode_info))
                .with_context(err_context)?;
        }
        Ok(())
    }

    fn move_suppressed_panes_from_closed_tab(
        &mut self,
        suppressed_panes: SuppressedPanes,
    ) -> Result<()> {
        // TODO: this is not entirely accurate, these also sometimes contain a pane who's
        // scrollback is being edited - in this case we need to close it or to move it to the
        // appropriate tab
        let err_context = || "Failed to move suppressed panes from closed tab";
        let first_tab_index = *self
            .tabs
            .keys()
            .next()
            .context("screen contains no tabs")
            .with_context(err_context)?;
        self.tabs
            .get_mut(&first_tab_index)
            .with_context(err_context)?
            .add_suppressed_panes(suppressed_panes);
        Ok(())
    }

    fn move_clients_between_tabs(
        &mut self,
        source_tab_index: usize,
        destination_tab_index: usize,
        update_mode_infos: bool,
        clients_to_move: Option<Vec<ClientId>>,
    ) -> Result<()> {
        let err_context = || {
            format!(
                "failed to move clients from tab {source_tab_index} to tab {destination_tab_index}"
            )
        };

        // None ==> move all clients
        let drained_clients = self
            .get_tab_by_id_mut(source_tab_index)
            .map(|t| t.drain_connected_clients(clients_to_move));

        if let Some(client_mode_info_in_source_tab) = drained_clients {
            let destination_tab = self
                .get_tab_by_id_mut(destination_tab_index)
                .context("failed to get destination tab by index")
                .with_context(err_context)?;
            destination_tab
                .add_multiple_clients(client_mode_info_in_source_tab)
                .with_context(err_context)?;
            if update_mode_infos {
                destination_tab
                    .update_input_modes()
                    .with_context(err_context)?;
            }
            destination_tab.set_force_render();
            if destination_tab.has_stack_lists() {
                destination_tab.set_should_clear_display_before_rendering();
            }
            destination_tab.visible(true).with_context(err_context)?;
        }
        Ok(())
    }

    fn update_client_tab_focus(&mut self, client_id: ClientId, new_tab_index: usize) {
        match self.active_tab_ids.remove(&client_id) {
            Some(old_active_index) => {
                self.active_tab_ids.insert(client_id, new_tab_index);
                if old_active_index != new_tab_index {
                    self.repaint_after_overlay_in(old_active_index, client_id);
                }
                let client_tab_history = self.tab_history.entry(client_id).or_default();
                client_tab_history.retain(|&e| e != new_tab_index);
                client_tab_history.push(old_active_index);
            },
            None => {
                self.active_tab_ids.insert(client_id, new_tab_index);
            },
        }
        self.clear_bell_for_focused_pane(client_id);
    }

    /// Repaint the surface a client is about to stop seeing `tab_id` from, if that tab was
    /// painting a floating pane outside its own viewport.
    ///
    /// A floating pane can be placed - or dragged - onto the global panes, outside the viewport of
    /// every tab, its own included. The tab the client leaves stops rendering the moment its last
    /// client is gone, which is right: the pane belongs to that tab and nobody looking elsewhere
    /// should see it. But nothing else owns those cells either. The tab being entered repaints
    /// only its own centre, and the global panes and the strip rows no pane owns are both diffed -
    /// neither changed, so neither emits anything, and the pane stays on the host terminal over a
    /// tab it is not in.
    fn repaint_after_overlay_in(&mut self, tab_id: usize, client_id: ClientId) {
        if !self
            .tabs
            .get(&tab_id)
            .map(|tab| tab.has_floating_panes_outside_viewport())
            .unwrap_or(false)
        {
            return;
        }
        self.global_panes.set_force_render();
        self.unowned_strip_fill.remove(&client_id);
    }

    /// A helper function to switch to a new tab at specified position.
    fn switch_active_tab(
        &mut self,
        new_tab_pos: usize,
        should_change_pane_focus: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || {
            format!(
            "Failed to switch to active tab at position {new_tab_pos} for client id: {client_id:?}"
        )
        };

        if let Some(new_tab) = self.tabs.values().find(|t| t.position == new_tab_pos) {
            match self.get_active_tab(client_id) {
                Ok(current_tab) => {
                    // If new active tab is same as the current one, do nothing.
                    if current_tab.position == new_tab_pos {
                        return Ok(());
                    }

                    let current_tab_index = current_tab.id;
                    let new_tab_index = new_tab.id;
                    let affected_client_ids: Vec<ClientId> = if self.session_is_mirrored {
                        self.connected_clients
                            .borrow()
                            .iter()
                            .map(|(c, _i)| *c)
                            .collect()
                    } else {
                        vec![client_id]
                    };
                    let old_focused_panes: Vec<(ClientId, Option<PaneId>)> = affected_client_ids
                        .iter()
                        .map(|c| (*c, self.get_active_pane_id(c)))
                        .collect();
                    // global panes are not owned by any tab, so a client focused on one keeps
                    // that focus across the switch
                    if self.session_is_mirrored {
                        self.move_clients_between_tabs(
                            current_tab_index,
                            new_tab_index,
                            update_mode_infos,
                            None,
                        )
                        .with_context(err_context)?;
                        let all_connected_clients: Vec<ClientId> = self
                            .connected_clients
                            .borrow()
                            .iter()
                            .map(|(c, _i)| *c)
                            .collect();
                        for client_id in all_connected_clients {
                            self.update_client_tab_focus(client_id, new_tab_index);
                            match (
                                should_change_pane_focus,
                                self.get_tab_by_id_mut(new_tab_index),
                            ) {
                                (Some(direction), Some(new_tab)) => {
                                    new_tab.focus_pane_on_edge(direction, client_id);
                                },
                                _ => {},
                            }
                        }
                    } else {
                        self.move_clients_between_tabs(
                            current_tab_index,
                            new_tab_index,
                            update_mode_infos,
                            Some(vec![client_id]),
                        )
                        .with_context(err_context)?;
                        match (
                            should_change_pane_focus,
                            self.get_tab_by_id_mut(new_tab_index),
                        ) {
                            (Some(direction), Some(new_tab)) => {
                                new_tab.focus_pane_on_edge(direction, client_id);
                            },
                            _ => {},
                        }
                        self.update_client_tab_focus(client_id, new_tab_index);
                    }

                    if let Some(current_tab) = self.get_tab_by_id_mut(current_tab_index) {
                        if current_tab.has_no_connected_clients() {
                            current_tab.visible(false).with_context(err_context)?;
                        }
                    } else {
                        Err::<(), _>(anyhow!("Tab index {:?} not found", current_tab_index))
                            .with_context(err_context)
                            .non_fatal();
                    }

                    // Clear tab bell notification for the newly active tab
                    if let Some(new_tab) = self.tabs.get_mut(&new_tab_index) {
                        let tab_id = new_tab.id;
                        new_tab.clear_tab_bell_notification();
                        let _ = self
                            .bus
                            .senders
                            .send_to_background_jobs(BackgroundJob::StopFlashTabBell(tab_id));
                    }

                    self.recompute_tab_size(current_tab_index)
                        .with_context(err_context)?;
                    self.recompute_tab_size(new_tab_index)
                        .with_context(err_context)?;

                    self.log_and_report_session_state()
                        .with_context(err_context)?;
                    for (affected_client_id, old_pane_id) in old_focused_panes {
                        let new_pane_id = self.get_active_pane_id(&affected_client_id);
                        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                            self.report_key_passthrough_state(affected_client_id, old, new);
                        }
                        let _ = self.sync_scroll_mode_on_focus(affected_client_id);
                    }
                    return self.render(None).with_context(err_context);
                },
                Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
            }
        }
        Ok(())
    }

    /// A helper function to switch to a new tab with specified name. Return true if tab [name] has
    /// been created, else false.
    fn switch_active_tab_name(&mut self, name: String, client_id: ClientId) -> Result<bool> {
        match self.tabs.values().find(|t| t.name == name) {
            Some(new_tab) => {
                self.switch_active_tab(new_tab.position, None, true, client_id)?;
                Ok(true)
            },
            None => Ok(false),
        }
    }

    /// Sets this [`Screen`]'s active [`Tab`] to the next tab.
    pub fn switch_tab_next(
        &mut self,
        should_change_pane_focus: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || format!("failed to switch to next tab for client {client_id}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        if let Some(client_id) = client_id {
            match self.get_active_tab(client_id) {
                Ok(active_tab) => {
                    let active_tab_pos = active_tab.position;
                    let new_tab_pos = (active_tab_pos + 1) % self.tabs.len();
                    return self.switch_active_tab(
                        new_tab_pos,
                        should_change_pane_focus,
                        update_mode_infos,
                        client_id,
                    );
                },
                Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
            }
        }
        Ok(())
    }

    /// Sets this [`Screen`]'s active [`Tab`] to the previous tab.
    pub fn switch_tab_prev(
        &mut self,
        should_change_pane_focus: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || format!("failed to switch to previous tab for client {client_id}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        if let Some(client_id) = client_id {
            match self.get_active_tab(client_id) {
                Ok(active_tab) => {
                    let active_tab_pos = active_tab.position;
                    let new_tab_pos = if active_tab_pos == 0 {
                        self.tabs.len() - 1
                    } else {
                        active_tab_pos - 1
                    };

                    return self.switch_active_tab(
                        new_tab_pos,
                        should_change_pane_focus,
                        update_mode_infos,
                        client_id,
                    );
                },
                Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
            }
        }
        Ok(())
    }

    pub fn go_to_tab(&mut self, tab_index: usize, client_id: ClientId) -> Result<()> {
        self.switch_active_tab(tab_index.saturating_sub(1), None, true, client_id)
    }

    pub fn go_to_tab_name(&mut self, name: String, client_id: ClientId) -> Result<bool> {
        self.switch_active_tab_name(name, client_id)
    }

    fn close_tab_by_id(&mut self, tab_id: usize) -> Result<()> {
        let err_context = || format!("failed to close tab at index {tab_id:?}");

        let mut tab_to_close = self.tabs.remove(&tab_id).with_context(err_context)?;
        self.last_rendered_floating_stacks.remove(&tab_id);
        let mut pane_ids = tab_to_close.get_all_pane_ids();

        // here we extract the suppressed panes (these are background panes that don't care which
        // tab they are in, and in the future we should probably make them global to screen rather
        // than to each tab) and move them to another tab if there is one
        let suppressed_panes = tab_to_close.extract_suppressed_panes();
        for suppressed_pane_id in suppressed_panes.keys() {
            pane_ids.retain(|p| p != suppressed_pane_id);
        }

        let _ = self.bus.senders.send_to_plugin(PluginInstruction::Update(
            pane_ids
                .iter()
                .copied()
                .map(|p_id| (None, None, Event::PaneClosed(p_id.into())))
                .collect(),
        ));

        // Notify pane render subscribers of each closed pane
        for p_id in &pane_ids {
            self.notify_pane_closed_to_subscribers((*p_id).into());
        }

        // below we don't check the result of sending the CloseTab instruction to the pty thread
        // because this might be happening when the app is closing, at which point the pty thread
        // has already closed and this would result in an error
        self.bus
            .senders
            .send_to_pty(PtyInstruction::CloseTab(pane_ids))
            .with_context(err_context)?;
        if self.tabs.is_empty() {
            self.active_tab_ids.clear();
            self.bus
                .senders
                .send_to_server(ServerInstruction::Render(None))
                .with_context(err_context)
        } else {
            let client_mode_infos_in_closed_tab = tab_to_close.drain_connected_clients(None);
            self.move_clients_from_closed_tab(client_mode_infos_in_closed_tab)
                .with_context(err_context)?;
            self.move_suppressed_panes_from_closed_tab(suppressed_panes)
                .with_context(err_context)?;
            let visible_tab_indices: HashSet<usize> =
                self.active_tab_ids.values().copied().collect();
            for t in self.tabs.values_mut() {
                if visible_tab_indices.contains(&t.id) {
                    t.set_force_render();
                    t.visible(true).with_context(err_context)?;
                }
                if t.position > tab_to_close.position {
                    t.position -= 1;
                }
            }
            for tab_id in visible_tab_indices {
                self.recompute_tab_size(tab_id).with_context(err_context)?;
            }
            self.log_and_report_session_state()
                .with_context(err_context)?;
            self.render(None).with_context(err_context)
        }
    }

    // Closes the client_id's focused tab
    pub fn close_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to close tab for client {client_id:?}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        match client_id {
            Some(client_id) => {
                let active_tab_index = *self
                    .active_tab_ids
                    .get(&client_id)
                    .with_context(err_context)?;
                self.close_tab_by_id(active_tab_index)
                    .with_context(err_context)
            },
            None => Ok(()),
        }
    }

    /// Where the global panes sit for a tab of `size`, and the center that tab gets.
    fn global_layout_for(&self, size: Size) -> Result<AllocatedGlobalPanes> {
        self.global_panes
            .tab_layout(Viewport {
                x: 0,
                y: 0,
                cols: size.cols,
                rows: size.rows,
            })
            .map_err(|error| anyhow!(error))
    }

    /// The top/bottom ui strips (tab bar, status bar) a left/right global pane must leave to the
    /// tab, so they can span the full screen width beside it.
    ///
    /// A global pane is one grid shared by every tab, so this cannot follow the tab being
    /// rendered: it is the tallest chrome any tab declares, which no tab's chrome can then be
    /// covered by. The cost is that a tab with less chrome than that has rows in the strip's columns
    /// that neither its chrome nor the shortened band reaches - `unowned_strip_rows` names them and
    /// `Screen::fill_unowned_strip_rows` paints them.
    fn max_chrome_rows(&self) -> (usize, usize) {
        self.tabs
            .values()
            .map(|tab| tab.chrome_rows())
            .reduce(|acc, rows| (acc.0.max(rows.0), acc.1.max(rows.1)))
            .unwrap_or((0, 0))
    }

    /// Every tab shares the smallest interactive client bounds, regardless of which tab each
    /// client is viewing. Watchers and fit-disabled mobile clients are already handled by
    /// `session_size`.
    fn tab_surface_size(&self, _tab_id: usize) -> Option<Size> {
        self.session_size()
    }

    /// How much of a watcher's viewport the followed client's frame covers, so
    /// `serialize_with_size` clears only what is beyond it.
    ///
    /// That frame is the whole tab surface, global pane strips included - `Tab::size` is just the
    /// center between them, and padding from there would clear the strips on every frame the panes
    /// themselves did not redraw.
    fn followed_frame_size(&self, tab_id: usize) -> Option<Size> {
        if !self.tabs.contains_key(&tab_id) {
            return None;
        }
        Some(
            self.tab_surface_size(tab_id)
                .unwrap_or_else(|| self.surface_size()),
        )
    }

    fn resize_global_panes_to(&mut self, size: Size) -> Result<Viewport> {
        self.global_panes.set_chrome_rows(self.max_chrome_rows());
        self.global_panes
            .resize(Viewport {
                x: 0,
                y: 0,
                cols: size.cols,
                rows: size.rows,
            })
            .map_err(|error| anyhow!(error))
    }

    fn reflow_global_panes(&mut self) -> Result<()> {
        let surface = self.surface_size();
        self.resize_global_panes_to(surface)?;
        for tab_id in self.tabs.keys().copied().collect::<Vec<_>>() {
            self.reflow_tab(tab_id)?;
        }
        Ok(())
    }

    /// Lay tab `tab_id` out over the shared session surface, beside the global panes.
    ///
    /// With no interactive client there is no size to derive, so the tab keeps its last layout.
    fn reflow_tab(&mut self, tab_id: usize) -> Result<()> {
        let Some(tab_size) = self.tab_surface_size(tab_id) else {
            return Ok(());
        };
        let center = self.global_layout_for(tab_size)?.center;
        let center_size = Size {
            cols: center.cols,
            rows: center.rows,
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.get_display_area() != center_size
                || tab.get_display_area_origin() != (center.x, center.y)
            {
                tab.resize_whole_tab_to(center)?;
                tab.set_force_render();
            }
        }
        Ok(())
    }

    /// The strips a tab must leave to the global panes for the supplied layout.
    /// One strip per edge: the union of that edge's pane geometries. Panes sharing an edge split
    /// one strip among themselves, and everything tab-side - boundary growth
    /// (`expand_viewport_over_global_panes`), the unowned-row fill (`unowned_strip_rows`), ui pane
    /// widening - reasons about the strip as a single rectangle. Per-pane strips here would make
    /// each same-edge pane's span look "unowned" from its sibling's strip and get blanked over.
    fn strips_from_layout(&self, layout: &AllocatedGlobalPanes) -> Vec<GlobalPaneStrip> {
        let mut strips: Vec<GlobalPaneStrip> = vec![];
        for (pane, placement, hidden) in self.global_panes.panes() {
            if hidden {
                continue;
            }
            let Some(geom) = layout.panes.get(&pane.pid().into()) else {
                continue;
            };
            match strips.iter_mut().find(|strip| strip.edge == placement.edge) {
                Some(strip) => {
                    let right = (strip.geom.x + strip.geom.cols.as_usize())
                        .max(geom.x + geom.cols.as_usize());
                    let bottom = (strip.geom.y + strip.geom.rows.as_usize())
                        .max(geom.y + geom.rows.as_usize());
                    strip.geom.x = strip.geom.x.min(geom.x);
                    strip.geom.y = strip.geom.y.min(geom.y);
                    strip.geom.cols = Dimension::fixed(right - strip.geom.x);
                    strip.geom.rows = Dimension::fixed(bottom - strip.geom.y);
                },
                None => strips.push(GlobalPaneStrip {
                    geom: *geom,
                    edge: placement.edge,
                }),
            }
        }
        strips
    }

    /// Paint the strip cells tab `tab_id` leaves unowned (see `unowned_strip_rows`) for the clients
    /// looking at it, so the previous tab's chrome does not linger beside the sidebar.
    ///
    /// Only for clients whose fill actually changed, unless `force` - `Output` re-emits every chunk
    /// it is handed and counts them towards `is_dirty`, so painting unconditionally would send a
    /// frame on every render tick.
    fn fill_unowned_strip_rows(
        &mut self,
        output: &mut Output,
        tab_id: usize,
        layout: &AllocatedGlobalPanes,
        client_ids: &[ClientId],
        force: bool,
    ) -> Result<()> {
        let strips = self.strips_from_layout(layout);
        let Some(tab) = self.tabs.get(&tab_id) else {
            return Ok(());
        };
        let rects = unowned_strip_rows(layout.center, &strips, tab.chrome_rows());
        let stale: Vec<ClientId> = client_ids
            .iter()
            .copied()
            .filter(|client_id| force || self.unowned_strip_fill.get(client_id) != Some(&rects))
            .collect();
        for client_id in &stale {
            self.unowned_strip_fill.insert(*client_id, rects.clone());
        }
        if rects.is_empty() || stale.is_empty() {
            return Ok(());
        }
        let chunks: Vec<CharacterChunk> = rects
            .iter()
            .map(|(x, y, cols)| CharacterChunk::new(vec![EMPTY_TERMINAL_CHARACTER; *cols], *x, *y))
            .collect();
        output
            .add_character_chunks_to_multiple_clients(chunks, stale.into_iter(), None)
            .context("failed to fill the strip rows no tab owns")
    }

    /// Lay out live notifications once for both rendering and mouse hit-testing.
    ///
    /// Called once every frame while any notification is live, rather than diffed like
    /// `fill_unowned_strip_rows` - a pane that repaints would overwrite the toast's cells, and
    /// re-emitting after every pane is both correct and cheap. The floating pane stack is cleared
    /// first so `Output` doesn't clip these chunks behind a floating pane sitting in that corner.
    ///
    /// Every toast in the stack shares one box width - the widest any of them wants - so the text
    /// can only be wrapped once that width is settled. One consequence: a new wide toast re-wraps,
    /// and can change the height of, every other live toast on the frame it arrives. Nothing caches
    /// the result, which is what keeps that honest; the wrapping is noise beside the chunk building
    /// already happening here, so it does not want a cache that would go stale on a resize.
    ///
    /// Entering/exiting toasts slide in from - and back out to - the anchored corner
    /// (`notification_reveal`, driven by each toast's own `horizontal_progress`), and while
    /// `notification_displacement_progress` is still settling, survivors are lerped from where
    /// they sat in `notification_stack_before_change` to where they sit now. Both the "before" and
    /// "after" stacks are laid out fresh for whichever `viewport` this call was given - this can
    /// run once per distinct client viewport in a frame - rather than trusting a stored row number
    /// that might belong to a different-sized viewport.
    fn notification_layout(&self, viewport: Viewport) -> Vec<NotificationLayout<'_>> {
        let Some(width) = self
            .notifications
            .iter()
            .filter_map(|notification| notification_width(&notification.text, viewport.cols))
            .max()
        else {
            return vec![];
        };
        let wrapped: Vec<Vec<String>> = self
            .notifications
            .iter()
            .map(|notification| notification_lines(&notification.text, width))
            .collect();
        let heights: Vec<usize> = wrapped
            .iter()
            .map(|lines| notification_height(lines.len()))
            .collect();
        let origins = notification_origins(viewport, self.notification_position, &heights, width);
        let before_origins = if self.notification_displacement_progress < 1.0 {
            let before_heights: Vec<usize> = self
                .notification_stack_before_change
                .iter()
                .map(|notification| {
                    notification_height(notification_lines(&notification.text, width).len())
                })
                .collect();
            notification_origins(viewport, self.notification_position, &before_heights, width)
        } else {
            vec![]
        };

        self.notifications
            .iter()
            .zip(wrapped)
            .zip(origins)
            .map(|((notification, lines), (x, y))| {
                let (x, y) = if self.notification_displacement_progress < 1.0 {
                    self.notification_stack_before_change
                        .iter()
                        .position(|before| before.id == notification.id)
                        .and_then(|index| before_origins.get(index))
                        .map(|&before_origin| {
                            lerp_origin(
                                before_origin,
                                (x, y),
                                self.notification_displacement_progress,
                            )
                        })
                        .unwrap_or((x, y))
                } else {
                    (x, y)
                };
                let (revealed_x, visible_cols) = notification_reveal_bounds(
                    x,
                    width,
                    self.notification_position,
                    notification.horizontal_progress,
                );
                NotificationLayout {
                    notification,
                    lines,
                    x,
                    y,
                    width,
                    revealed_x,
                    visible_cols,
                }
            })
            .collect()
    }

    fn render_notifications(
        &self,
        output: &mut Output,
        viewport: Viewport,
        client_ids: &[ClientId],
    ) -> Result<()> {
        if self.notifications.is_empty() || client_ids.is_empty() {
            return Ok(());
        }
        let layouts = self.notification_layout(viewport);

        let previous_stack = output.set_floating_panes_stack(None);
        let result = (|| -> Result<()> {
            for layout in layouts {
                let chunks = notification_chunks(
                    &layout.lines,
                    layout.x,
                    layout.y,
                    layout.width,
                    &self.style,
                );
                let (chunks, _) = notification_reveal(
                    &chunks,
                    layout.x,
                    layout.width,
                    self.notification_position,
                    layout.notification.horizontal_progress,
                );
                output.add_character_chunks_to_multiple_clients(
                    chunks,
                    client_ids.iter().copied(),
                    None,
                )?;
            }
            Ok(())
        })();
        output.set_floating_panes_stack(previous_stack);
        result.context("failed to render notifications")
    }

    fn pane_notification_at(
        &self,
        viewport: Viewport,
        position: Position,
    ) -> Option<(u64, PaneId)> {
        let line = usize::try_from(position.line()).ok()?;
        self.notification_layout(viewport)
            .into_iter()
            .rev()
            .find(|layout| {
                layout.visible_cols > 0
                    && (layout.revealed_x..layout.revealed_x + layout.visible_cols)
                        .contains(&position.column())
                    && (layout.y..layout.y + notification_height(layout.lines.len()))
                        .contains(&line)
            })
            .and_then(|layout| {
                layout
                    .notification
                    .pane_id
                    .map(|pane_id| (layout.notification.id, pane_id))
            })
    }

    fn dismiss_notification(&mut self, id: u64) -> Result<()> {
        if let Some(notification) = self.notifications.iter_mut().find(|n| n.id == id) {
            if !notification.dismissing {
                notification.dismissing = true;
                self.bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::AnimateNotifications)?;
            }
        }
        Ok(())
    }

    fn handle_pane_notification_click(
        &mut self,
        event: &MouseEvent,
        viewport: Viewport,
        client_id: ClientId,
    ) -> bool {
        if event.event_type != MouseEventType::Press
            || !event.left
            || event.right
            || event.middle
            || event.shift
            || event.alt
            || event.ctrl
        {
            return false;
        }
        let Some((notification_id, pane_id)) = self.pane_notification_at(viewport, event.position)
        else {
            return false;
        };

        self.dismiss_notification(notification_id).non_fatal();
        let pane_exists = self.global_panes.pane(pane_id).is_some()
            || self
                .tabs
                .values()
                .any(|tab| tab.has_pane_with_pid(&pane_id));
        if pane_exists {
            if self.get_active_pane_id(&client_id) != Some(pane_id) {
                self.focus_pane_with_id(pane_id, true, false, client_id)
                    .non_fatal();
            }
            self.clear_bell_for_pane_id(pane_id, client_id);
            self.reconcile_single_pane_focus(client_id);
            self.sync_scroll_mode_on_focus(client_id).non_fatal();
            self.log_and_report_session_state().non_fatal();
        }
        self.notify_plugins_of_input(client_id);
        self.render(None).non_fatal();
        true
    }

    /// One tick of the toast slide/displacement animation: advances every notification's
    /// `horizontal_progress` and the shared `notification_displacement_progress`, finally removing
    /// any toast that has finished sliding out. Returns whether anything is still mid-animation, so
    /// the caller knows whether to keep the animation clock ticking.
    fn advance_notification_animations(&mut self) -> bool {
        for notification in self.notifications.iter_mut() {
            let step = if notification.dismissing {
                -NOTIFICATION_HORIZONTAL_PROGRESS_STEP
            } else {
                NOTIFICATION_HORIZONTAL_PROGRESS_STEP
            };
            notification.horizontal_progress =
                (notification.horizontal_progress + step).clamp(0.0, 1.0);
        }

        let was_showing = self.notifications.len();
        if self
            .notifications
            .iter()
            .any(|n| n.dismissing && n.horizontal_progress <= 0.0)
        {
            // the stack is about to shrink - remember the fuller layout so the survivors glide to
            // their new position instead of jumping into the gap
            self.notification_stack_before_change = self.notifications.clone();
            self.notification_displacement_progress = 0.0;
            self.notifications
                .retain(|n| !(n.dismissing && n.horizontal_progress <= 0.0));
        }
        if self.notifications.len() != was_showing {
            // nothing under the toast is dirty, so without forcing a repaint its cells would keep
            // the glyphs it just gave up
            for tab in self.tabs.values_mut() {
                tab.set_force_render();
            }
            self.global_panes.set_force_render();
        }

        if self.notification_displacement_progress < 1.0 {
            self.notification_displacement_progress =
                (self.notification_displacement_progress + NOTIFICATION_DISPLACEMENT_STEP).min(1.0);
        }

        self.notifications
            .iter()
            .any(|n| n.dismissing || n.horizontal_progress < 1.0)
            || self.notification_displacement_progress < 1.0
    }

    /// Move the global panes to where they sit for `client_id`'s tab, so anything reading their
    /// geometry for that client - hit testing, relative mouse positions, a border drag - sees the
    /// positions that client is actually looking at rather than the last tab rendered. Returns the
    /// layout they were moved to.
    fn position_global_panes_for_client(
        &mut self,
        client_id: ClientId,
    ) -> Option<AllocatedGlobalPanes> {
        let tab_id = self.active_tab_ids.get(&client_id).copied()?;
        let tab_size = self
            .tab_surface_size(tab_id)
            .unwrap_or_else(|| self.surface_size());
        let layout = self.global_layout_for(tab_size).ok()?;
        self.global_panes.set_positions(&layout);
        Some(layout)
    }

    fn with_pane<R>(&mut self, pane_id: PaneId, f: impl FnOnce(&mut dyn Pane) -> R) -> Option<R> {
        if let Some(pane) = self.global_panes.pane_mut(pane_id) {
            return Some(f(pane));
        }
        for tab in self.tabs.values_mut() {
            if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                return Some(f(pane.as_mut()));
            }
        }
        None
    }

    #[cfg(test)]
    fn global_pane_placement(&self, pane_id: PaneId) -> Option<GlobalPanePlacement> {
        self.global_panes.placement(pane_id)
    }

    fn reject_global_for_tab_operation(&self, pane_id: PaneId) -> Result<()> {
        if self.global_panes.pane(pane_id).is_some() {
            Err(anyhow!("global pane has no tab"))
        } else {
            Ok(())
        }
    }

    fn reject_global_for_tab_operation_with_completion(
        &self,
        pane_id: PaneId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> bool {
        if self.reject_global_for_tab_operation(pane_id).is_err() {
            let error = "global pane has no tab";
            log::error!("{error}");
            if let Some(completion_tx) = completion_tx.as_mut() {
                completion_tx.set_exit_status(1);
                completion_tx.set_error_message(error.to_owned());
            }
            true
        } else {
            false
        }
    }

    fn reject_focused_global_for_tab_operation_with_completion(
        &self,
        client_id: ClientId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> bool {
        self.global_panes
            .focused_pane_id(client_id)
            .map(|pane_id| {
                self.reject_global_for_tab_operation_with_completion(pane_id, completion_tx)
            })
            .unwrap_or(false)
    }

    fn active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.global_panes.focused_pane_id(client_id).or_else(|| {
            self.get_active_tab(client_id)
                .ok()
                .and_then(|tab| tab.get_active_pane_id(client_id))
        })
    }

    fn focus_global_pane(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        self.global_panes
            .focus_pane(pane_id, client_id)
            .map_err(|error| anyhow!(error))?;
        if let Ok(tab) = self.get_active_tab_mut(client_id) {
            tab.set_force_render();
        }
        self.global_panes.set_force_render();
        let _ = self.sync_scroll_mode_on_focus(client_id);
        Ok(())
    }

    fn defocus_global_pane(&mut self, client_id: ClientId) {
        if self.global_panes.focused_pane_id(client_id).is_none() {
            return;
        }
        self.global_panes.defocus_pane(client_id);
        if let Ok(tab) = self.get_active_tab_mut(client_id) {
            tab.set_force_render();
        }
        self.global_panes.set_force_render();
        let _ = self.sync_scroll_mode_on_focus(client_id);
    }

    fn activate_overlay(&mut self, pane_id: PaneId) {
        let Some((tab_id, tab)) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.pane_is_overlay(pane_id))
        else {
            return;
        };
        let tab_id = *tab_id;
        let clients: Vec<_> = self
            .active_tab_ids
            .iter()
            .filter_map(|(client_id, active_tab_id)| {
                (*active_tab_id == tab_id && tab.get_active_pane_id(*client_id) == Some(pane_id))
                    .then_some(*client_id)
            })
            .collect();
        for client_id in clients {
            if let Some(global_pane_id) = self.global_panes.focused_pane_id(client_id) {
                self.suspended_global_focus
                    .insert(client_id, (tab_id, global_pane_id));
                self.defocus_global_pane(client_id);
            }
        }
    }

    fn take_global_focus_after_overlay_close(
        &mut self,
        pane_id: PaneId,
    ) -> Vec<(ClientId, PaneId)> {
        let Some((tab_id, tab)) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.pane_is_overlay(pane_id))
        else {
            return vec![];
        };
        let tab_id = *tab_id;
        let mut restore = vec![];
        self.suspended_global_focus
            .retain(|client_id, (saved_tab, global_pane)| {
                if *saved_tab != tab_id {
                    return true;
                }
                if tab.get_active_pane_id(*client_id) == Some(pane_id) {
                    restore.push((*client_id, *global_pane));
                }
                false
            });
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.set_should_clear_display_before_rendering();
        }
        self.global_panes.set_force_render();
        restore
    }

    fn restore_global_focus(&mut self, focus: Vec<(ClientId, PaneId)>) {
        for (client_id, pane_id) in focus {
            self.focus_global_pane(pane_id, client_id).non_fatal();
        }
    }

    /// The global pane to focus next when cycling, or `None` to hand the cycle back to the tab.
    /// Global panes form a segment appended to the end of the active tab's pane cycle, so the full
    /// loop is `tab panes... -> global panes... -> tab panes...`.
    fn next_global_pane_in_cycle(&mut self, client_id: ClientId, forward: bool) -> Option<PaneId> {
        let global_pane_ids = self.global_panes.focusable_pane_ids();
        if global_pane_ids.is_empty() {
            return None;
        }
        match self.global_panes.focused_pane_id(client_id) {
            Some(focused_pane_id) => {
                let position = global_pane_ids
                    .iter()
                    .position(|pane_id| *pane_id == focused_pane_id)?;
                if forward {
                    global_pane_ids.get(position + 1).copied()
                } else {
                    position
                        .checked_sub(1)
                        .and_then(|position| global_pane_ids.get(position).copied())
                }
            },
            None => {
                let tab = self.get_active_tab_mut(client_id).ok()?;
                // a tab with no selectable panes of its own is always "at the end of its cycle"
                let at_cycle_end = !tab.has_selectable_tiled_panes()
                    || if forward {
                        tab.is_last_pane_in_cycle(client_id)
                    } else {
                        tab.is_first_pane_in_cycle(client_id)
                    };
                if !at_cycle_end {
                    return None;
                }
                if forward {
                    global_pane_ids.first().copied()
                } else {
                    global_pane_ids.last().copied()
                }
            },
        }
    }

    fn cycle_focus(&mut self, client_id: ClientId, forward: bool) {
        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };
        let Some(client_id) = client_id else {
            log::error!("No client ids in screen found");
            return;
        };
        match self.next_global_pane_in_cycle(client_id, forward) {
            Some(pane_id) => {
                if let Err(error) = self.focus_global_pane(pane_id, client_id) {
                    log::error!("{error}");
                }
            },
            None => {
                // either we were never on a global pane, or we just fell off the end of the global
                // segment - both mean "advance the tab", which also closes the loop back to its
                // first pane
                self.defocus_global_pane(client_id);
                if let Ok(tab) = self.get_active_tab_mut(client_id) {
                    if forward {
                        tab.focus_next_pane(client_id);
                    } else {
                        tab.focus_previous_pane(client_id);
                    }
                }
            },
        }
    }

    /// Focus the global pane sitting on the edge we're moving towards, if there is one. With
    /// several panes splitting that edge's strip, the one whose span overlaps the client's active
    /// tab pane wins, so the move lands where the eye expects.
    fn enter_global_pane_in_direction(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> bool {
        let edge = global_pane_edge_for_direction(direction);
        let cross_axis_pos = self
            .get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane(client_id))
            .map(|pane| {
                let geom = pane.position_and_size();
                match edge {
                    GlobalPaneEdge::Left | GlobalPaneEdge::Right => {
                        geom.y + geom.rows.as_usize() / 2
                    },
                    GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => {
                        geom.x + geom.cols.as_usize() / 2
                    },
                }
            })
            .unwrap_or(0);
        let Some(pane_id) = self.global_panes.pane_on_edge_nearest(edge, cross_axis_pos) else {
            return false;
        };
        if let Err(error) = self.focus_global_pane(pane_id, client_id) {
            log::error!("{error}");
            return false;
        }
        true
    }

    /// Whether a modal in the client's active tab holds the keyboard. A directional focus move
    /// is then a no-op - never an escalation to a global pane, another tab, or the host session.
    fn active_tab_modal_holds_focus(&self, client_id: ClientId) -> bool {
        self.get_active_tab(client_id)
            .map(|tab| !tab.lower_layer_may_take_focus(client_id))
            .unwrap_or(false)
    }

    /// Handle a directional focus move made from a focused global pane. Returns `true` when the
    /// client is on a global pane, in which case the caller must not fall through to the tab.
    fn global_pane_handles_direction(&mut self, direction: Direction, client_id: ClientId) -> bool {
        let Some(focused_pane_id) = self.global_panes.focused_pane_id(client_id) else {
            return false;
        };
        if let Some(edge) = self
            .global_panes
            .placement(focused_pane_id)
            .map(|placement| placement.edge)
        {
            let along_strip = match edge {
                GlobalPaneEdge::Left | GlobalPaneEdge::Right => {
                    matches!(direction, Direction::Up | Direction::Down)
                },
                GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => {
                    matches!(direction, Direction::Left | Direction::Right)
                },
            };
            if along_strip {
                // moving within the strip lands on the same-edge neighbor; off the strip's end,
                // fall through to the perpendicular hop below, same as before splits existed
                let towards_strip_end = matches!(direction, Direction::Down | Direction::Right);
                if let Some(neighbor_id) = self
                    .global_panes
                    .neighbor_in_strip(focused_pane_id, towards_strip_end)
                {
                    if let Err(error) = self.focus_global_pane(neighbor_id, client_id) {
                        log::error!("{error}");
                    }
                    return true;
                }
            }
            if global_pane_edge_for_direction(direction) == edge {
                // moving further outwards - this is the session boundary, same as a tab edge
                self.bubble_focus_to_host(client_id, direction);
            } else if direction == direction_away_from_edge(edge) {
                self.defocus_global_pane(client_id);
                if let Ok(tab) = self.get_active_tab_mut(client_id) {
                    tab.focus_pane_on_edge(direction, client_id);
                }
            } else {
                // perpendicular - move to the global pane on that edge if there is one
                self.enter_global_pane_in_direction(direction, client_id);
            }
        }
        true
    }

    /// Resize the global pane the client's active tab pane borders, if the resize was made towards
    /// it. Moving that shared border is the same gesture as moving one between two tab panes, only
    /// the pane on the other side of it is global - so growing the tab pane shrinks the global one.
    /// Returns whether the resize was handled here.
    fn resize_global_pane_from_adjacent_tab_pane(
        &mut self,
        client_id: ClientId,
        strategy: ResizeStrategy,
    ) -> Result<bool> {
        // a directionless resize has no border to point at, so it stays tab-local
        let Some(direction) = strategy.direction else {
            return Ok(false);
        };
        let edge = global_pane_edge_for_direction(direction);
        let cross_axis_pos = self
            .get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane(client_id))
            .map(|pane| {
                let geom = pane.position_and_size();
                match edge {
                    GlobalPaneEdge::Left | GlobalPaneEdge::Right => {
                        geom.y + geom.rows.as_usize() / 2
                    },
                    GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => {
                        geom.x + geom.cols.as_usize() / 2
                    },
                }
            })
            .unwrap_or(0);
        let Some(global_pane_id) = self.global_panes.pane_on_edge_nearest(edge, cross_axis_pos)
        else {
            return Ok(false);
        };
        let is_flush_with_global_pane = self
            .get_active_tab(client_id)
            .map(|tab| tab.active_pane_is_flush_with_edge(direction, client_id))
            .unwrap_or(false);
        if !is_flush_with_global_pane {
            return Ok(false);
        }
        let strategy = ResizeStrategy {
            resize: strategy.resize.invert(),
            ..strategy
        };
        self.resize_pane_with_id(strategy, global_pane_id)?;
        Ok(true)
    }

    /// Hide the whole strip `pane_id` belongs to - panes sharing an edge (a hocus-style sidebar
    /// and its agent list, say) hide and show as one unit.
    fn hide_pane(&mut self, pane_id: PaneId) -> Result<()> {
        let edge = self
            .global_panes
            .placement(pane_id)
            .ok_or_else(|| anyhow!("global pane {pane_id:?} not found"))?
            .edge;
        self.global_panes
            .hide_edge(edge)
            .map_err(|error| anyhow!(error))?;
        self.reflow_global_panes()
    }

    /// Show the whole strip `pane_id` belongs to, restoring its split geometry.
    fn show_pane(&mut self, pane_id: PaneId) -> Result<()> {
        let edge = self
            .global_panes
            .placement(pane_id)
            .ok_or_else(|| anyhow!("global pane {pane_id:?} not found"))?
            .edge;
        let surface = self.surface_size();
        self.global_panes
            .show_edge(edge, pane_id, surface.into())
            .map_err(|error| anyhow!(error))?;
        self.reflow_global_panes()
    }

    fn set_global_pane_placement(
        &mut self,
        pane_id: PaneId,
        placement: GlobalPanePlacement,
    ) -> Result<()> {
        let surface = self.surface_size();
        self.global_panes
            .set_placement(pane_id, placement, surface.into())
            .map_err(|error| anyhow!(error))?;
        self.reflow_global_panes()
    }

    fn write_to_pane_id(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        pane_id: PaneId,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<bool> {
        if self.global_panes.pane(pane_id).is_none() {
            for tab in self.tabs.values_mut() {
                if tab.has_pane_with_pid(&pane_id) {
                    return tab.write_to_pane_id(
                        key_with_modifier,
                        raw_input_bytes,
                        raw_input_bytes_are_kitty,
                        pane_id,
                        client_id,
                        completion_tx,
                    );
                }
            }
            return Err(anyhow!("failed to find pane with id {pane_id:?}"));
        }

        let adjusted_input = self.global_panes.pane_mut(pane_id).and_then(|pane| {
            pane.adjust_input_to_terminal(
                key_with_modifier,
                raw_input_bytes,
                raw_input_bytes_are_kitty,
                client_id,
            )
        });
        let mut should_update_ui = false;
        match (pane_id, adjusted_input) {
            (PaneId::Terminal(terminal_id), Some(AdjustedInput::WriteBytesToTerminal(bytes))) => {
                self.bus
                    .senders
                    .send_to_pty_writer(PtyWriteInstruction::Write(
                        bytes,
                        terminal_id,
                        completion_tx,
                    ))?;
            },
            (
                PaneId::Terminal(terminal_id),
                Some(AdjustedInput::ReRunCommandInThisPane(command)),
            ) => {
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ReRunCommandInPane(
                        PaneId::Terminal(terminal_id),
                        command,
                        completion_tx,
                    ))?;
                should_update_ui = true;
            },
            (PaneId::Terminal(_), Some(AdjustedInput::CloseThisPane)) => {
                self.close_global_pane(pane_id, completion_tx)?;
                should_update_ui = true;
            },
            (
                PaneId::Terminal(terminal_id),
                Some(AdjustedInput::DropToShellInThisPane { working_dir }),
            ) => {
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::DropToShellInPane {
                        pane_id: PaneId::Terminal(terminal_id),
                        shell: Some(self.default_shell.clone()),
                        working_dir,
                        completion_tx,
                    })?;
                should_update_ui = true;
            },
            (PaneId::Plugin(plugin_id), Some(AdjustedInput::WriteKeyToPlugin(key))) => {
                self.bus
                    .senders
                    .send_to_plugin(PluginInstruction::Update(vec![(
                        Some(plugin_id),
                        client_id,
                        Event::Key(key),
                    )]))?;
            },
            (PaneId::Plugin(plugin_id), Some(AdjustedInput::WriteBytesToTerminal(bytes))) => {
                self.bus.senders.send_to_plugin(PluginInstruction::Update(
                    parse_keys(&bytes)
                        .into_iter()
                        .map(|key| (Some(plugin_id), client_id, Event::Key(key)))
                        .collect(),
                ))?;
            },
            (
                PaneId::Plugin(plugin_id),
                Some(AdjustedInput::PermissionRequestResult(permissions, status)),
            ) => {
                // As Tab::handle_plugin_input does: while `requesting_permissions` is set the
                // pane renders the prompt over the plugin's output and swallows keys, for every
                // client rather than just the one who answered.
                self.with_pane(PaneId::Plugin(plugin_id), |pane| {
                    pane.request_permissions_from_user(None)
                });
                self.bus
                    .senders
                    .send_to_plugin(PluginInstruction::PermissionRequestResult(
                        plugin_id,
                        client_id,
                        permissions,
                        status,
                        None,
                    ))?;
                should_update_ui = true;
            },
            _ => {},
        }
        Ok(should_update_ui)
    }

    fn paste_to_pane_id(
        &mut self,
        bytes: Vec<u8>,
        pane_id: PaneId,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        self.write_to_pane_id(
            &None,
            BRACKETED_PASTE_BEGIN.to_vec(),
            false,
            pane_id,
            client_id,
            None,
        )?;
        self.write_to_pane_id(&None, bytes, false, pane_id, client_id, None)?;
        self.write_to_pane_id(
            &None,
            BRACKETED_PASTE_END.to_vec(),
            false,
            pane_id,
            client_id,
            completion_tx,
        )?;
        Ok(())
    }

    pub fn set_client_size(&mut self, client_id: ClientId, size: Size) {
        self.client_sizes.insert(client_id, size);
        // A terminal that reports its text area in pixels but not its cell size needs a client
        // size to divide by. That reply reaches us on the route thread while the size comes via
        // AddClient on the server-instruction thread, so it can arrive first - `Screen::size` was
        // seeded in `new` and was always there to divide by, `client_sizes` is not, and the client
        // re-sends only on a font size change. `self.pixel_dimensions` kept the reply, so derive
        // from it now that there is a size.
        //
        // Only when nothing has been derived yet: `text_area_size` belongs to whichever client
        // sent it, so dividing it by another client's - or a newly resized one's - rows and cols
        // would replace a good cell size with a wrong one.
        let cell_size_is_unknown = self.character_cell_size.borrow().is_none();
        if cell_size_is_unknown {
            self.update_pixel_dimensions(client_id, PixelDimensions::default());
        }
    }

    // The size a client contributes to the layout: a mobile web client with "fit" disabled must
    // not shrink anything, so it borrows the size of a client it shares a tab with instead.
    fn effective_client_size(&self, client_id: ClientId) -> Option<Size> {
        if self
            .mobile_web_prefs
            .get(&client_id)
            .map(|prefs| !prefs.fit)
            .unwrap_or(false)
        {
            self.reference_size_for_client(client_id)
        } else {
            self.client_sizes.get(&client_id).copied()
        }
    }

    // The screen is shared by every client, so it can only be as large as the smallest one on
    // each axis. `None` when no client contributes a size.
    fn session_size(&self) -> Option<Size> {
        let connected = self.connected_clients.borrow();
        connected
            .keys()
            .filter(|client_id| !self.watcher_clients.contains_key(client_id))
            .filter_map(|client_id| self.effective_client_size(*client_id))
            .reduce(|acc, size| Size {
                rows: acc.rows.min(size.rows),
                cols: acc.cols.min(size.cols),
            })
    }

    // The single surface every global pane is allocated over: the smallest client viewport, so a
    // global pane stays visible to every client.
    //
    // When no client contributes a size - every regular client has detached and only watchers
    // remain - the last area the panes were laid out over stands in, so the layout is kept rather
    // than collapsed to nothing. The per-client lookup is the remaining fallback, for before the
    // first reflow has happened.
    //
    // Takes `connected_clients` via `session_size`, so bind it to a local before calling anything
    // that borrows it again (`get_first_client_id`, and so `resize_global_panes_to`).
    fn surface_size(&self) -> Size {
        self.session_size()
            .or_else(|| {
                self.global_panes.area().map(|area| Size {
                    rows: area.rows,
                    cols: area.cols,
                })
            })
            .unwrap_or_else(|| self.size_for_client(None))
    }

    // Stands in for the terminal-wide resize the removed `Screen::size` used to model: put every
    // client at `new_screen_size` so `surface_size` resolves to it, then reflow.
    #[cfg(test)]
    fn resize_to_screen(&mut self, new_screen_size: Size) -> Result<()> {
        let client_ids: Vec<ClientId> = self
            .connected_clients
            .borrow()
            .keys()
            .copied()
            .chain(self.client_sizes.keys().copied())
            .collect();
        for client_id in client_ids {
            self.set_client_size(client_id, new_screen_size);
        }
        self.resize_to_clients()
    }

    pub fn resize_to_clients(&mut self) -> Result<()> {
        let err_context = || "failed to resize to client sizes".to_string();
        self.reflow_global_panes().with_context(err_context)?;
        self.log_and_report_session_state()
            .with_context(err_context)?;
        self.render(None).with_context(err_context)
    }

    fn size_for_client(&self, client_id: Option<ClientId>) -> Size {
        client_id
            .and_then(|client_id| self.client_sizes.get(&client_id).copied())
            .or_else(|| {
                self.get_first_client_id()
                    .and_then(|client_id| self.client_sizes.get(&client_id).copied())
            })
            .or_else(|| self.client_sizes.values().next().copied())
            // every client has detached, so nobody contributes a size - the last area the global
            // panes were laid out over is what `surface_size` falls back to, and a tab created
            // here (`zellij action new-tab` against a detached session) has to match it or it is
            // built with no rows to put a pane in and closed on the next render tick
            .or_else(|| {
                self.global_panes.area().map(|area| Size {
                    rows: area.rows,
                    cols: area.cols,
                })
            })
            // and before the first client has reported one, the size the session was built with -
            // never a zero size, which is not a layout anything can be laid out against
            .unwrap_or(self.initial_size)
    }

    fn size_of_tab_of_client(&self, client_id: ClientId) -> Size {
        self.active_tab_ids
            .get(&client_id)
            .and_then(|tab_id| self.tabs.get(tab_id))
            .map(|tab| tab.size)
            .unwrap_or_else(|| self.size_for_client(Some(client_id)))
    }

    pub fn recompute_tab_size(&mut self, tab_id: usize) -> Result<()> {
        self.reflow_tab(tab_id)
            .with_context(|| format!("failed to recompute size for tab {tab_id}"))
    }

    // A fit-disabled mobile client borrows only from a client viewing the same tab; another tab
    // cannot be used as its rendering reference.
    fn shares_tab_with(&self, client_id: ClientId, other_id: ClientId) -> bool {
        if other_id == client_id {
            return false;
        }
        match (
            self.active_tab_ids.get(&client_id),
            self.active_tab_ids.get(&other_id),
        ) {
            (Some(tab_id), Some(other_tab_id)) => tab_id == other_tab_id,
            _ => false,
        }
    }

    fn reference_size_for_client(&self, client_id: ClientId) -> Option<Size> {
        let connected = self.connected_clients.borrow();
        connected
            .keys()
            .filter(|other_id| self.shares_tab_with(client_id, **other_id))
            .filter_map(|other_id| self.client_sizes.get(other_id).copied())
            .max_by_key(|size| size.rows * size.cols)
    }

    fn has_reference_client(&self, client_id: ClientId) -> bool {
        let connected = self.connected_clients.borrow();
        connected
            .keys()
            .any(|other_id| self.shares_tab_with(client_id, *other_id))
    }

    fn tab_id_containing_pane(&self, pane_id: PaneId) -> Option<usize> {
        self.tabs
            .values()
            .find(|tab| tab.has_pane_with_pid(&pane_id))
            .map(|tab| tab.id)
    }

    fn ensure_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) {
        let Some(tab_id) = self.tab_id_containing_pane(pane_id) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        if tab.is_fullscreen_active() {
            if tab.fullscreen_pane_id() == Some(pane_id) {
                if tab.fullscreen_covers_ui() {
                    return;
                }
                tab.toggle_pane_no_ui_fullscreen(pane_id);
                return;
            }
            tab.unset_fullscreen();
        }
        tab.toggle_pane_no_ui_fullscreen(pane_id);
    }

    fn unset_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) {
        let Some(tab_id) = self.tab_id_containing_pane(pane_id) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        if tab.is_fullscreen_active() && tab.fullscreen_pane_id() == Some(pane_id) {
            tab.unset_fullscreen();
        }
    }

    fn active_pane_id_for_client(&mut self, client_id: ClientId) -> Option<PaneId> {
        self.get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane_id_or_first_selectable(client_id))
    }

    pub fn set_mobile_render_preferences(
        &mut self,
        client_id: ClientId,
        single_pane: bool,
        fit: bool,
    ) -> Result<()> {
        let reference_client_connected = self.has_reference_client(client_id);
        let fit = if !reference_client_connected {
            true
        } else {
            fit
        };

        let previous = self.mobile_web_prefs.get(&client_id).copied();
        let previous_fullscreen = previous.and_then(|p| p.fullscreened_pane);

        let mut fullscreened_pane = previous_fullscreen;
        if single_pane {
            let target = self.active_pane_id_for_client(client_id);
            if let Some(pane_id) = target {
                if previous_fullscreen != Some(pane_id) {
                    if let Some(previous_pane) = previous_fullscreen {
                        self.unset_pane_no_ui_fullscreen(previous_pane);
                    }
                }
                self.ensure_pane_no_ui_fullscreen(pane_id);
                fullscreened_pane = Some(pane_id);
            }
        } else if let Some(previous_pane) = previous_fullscreen {
            self.unset_pane_no_ui_fullscreen(previous_pane);
            fullscreened_pane = None;
        }

        self.mobile_web_prefs.insert(
            client_id,
            MobileWebPrefs {
                single_pane,
                fit,
                fullscreened_pane,
            },
        );

        if let Some(&tab_id) = self.active_tab_ids.get(&client_id) {
            self.recompute_tab_size(tab_id)?;
        }
        self.render(None)?;
        Ok(())
    }

    fn revert_fit_disabled_without_reference_client(&mut self) -> Result<()> {
        let reverted: Vec<ClientId> = self
            .mobile_web_prefs
            .iter()
            .filter(|(_client_id, prefs)| !prefs.fit)
            .map(|(client_id, _)| *client_id)
            .filter(|client_id| !self.has_reference_client(*client_id))
            .collect();
        if reverted.is_empty() {
            return Ok(());
        }
        let mut tabs_to_recompute: HashSet<usize> = HashSet::new();
        for client_id in reverted {
            if let Some(prefs) = self.mobile_web_prefs.get_mut(&client_id) {
                prefs.fit = true;
            }
            if let Some(&tab_id) = self.active_tab_ids.get(&client_id) {
                tabs_to_recompute.insert(tab_id);
            }
        }
        for tab_id in tabs_to_recompute {
            self.recompute_tab_size(tab_id)?;
        }
        Ok(())
    }

    fn reconcile_all_single_pane_focus(&mut self) {
        let single_pane_clients: Vec<ClientId> = self
            .mobile_web_prefs
            .iter()
            .filter(|(_client_id, prefs)| prefs.single_pane)
            .map(|(client_id, _)| *client_id)
            .collect();
        for client_id in single_pane_clients {
            self.reconcile_single_pane_focus(client_id);
        }
    }

    fn reconcile_single_pane_focus(&mut self, client_id: ClientId) {
        let is_single_pane = self
            .mobile_web_prefs
            .get(&client_id)
            .map(|prefs| prefs.single_pane)
            .unwrap_or(false);
        if !is_single_pane {
            return;
        }
        let Some(target) = self.active_pane_id_for_client(client_id) else {
            return;
        };
        let previous = self
            .mobile_web_prefs
            .get(&client_id)
            .and_then(|prefs| prefs.fullscreened_pane);

        // Tab-global fullscreen is the source of truth. When the tracked pane is still the focused
        // one but the tab is no longer fullscreen on it, another actor deliberately left
        // fullscreen, so single-pane yields rather than fighting for it. A changed focus target
        // (own tap, own new pane, closed pane) is instead re-asserted against the new pane.
        let target_tab_fullscreen_ok = self
            .tab_id_containing_pane(target)
            .and_then(|tab_id| self.tabs.get(&tab_id))
            .map(|tab| {
                tab.is_fullscreen_active()
                    && tab.fullscreen_pane_id() == Some(target)
                    && tab.fullscreen_covers_ui()
            })
            .unwrap_or(false);

        if previous == Some(target) {
            if target_tab_fullscreen_ok {
                return;
            }
            self.demote_single_pane(client_id);
            return;
        }
        if let Some(previous_pane) = previous {
            if previous_pane != target {
                self.unset_pane_no_ui_fullscreen(previous_pane);
            }
        }
        self.ensure_pane_no_ui_fullscreen(target);
        if let Some(prefs) = self.mobile_web_prefs.get_mut(&client_id) {
            prefs.fullscreened_pane = Some(target);
        }
    }

    // Leaves the tab as the other client left it and drops the client's single-pane preference.
    // Fit is disabled alongside it when the tab is shared, otherwise the tab would immediately be
    // shrunk to this client's size by the min-size rule - trading one intrusion for another.
    fn demote_single_pane(&mut self, client_id: ClientId) {
        let disable_fit = self.has_reference_client(client_id);
        let mut fit_changed = false;
        if let Some(prefs) = self.mobile_web_prefs.get_mut(&client_id) {
            prefs.single_pane = false;
            prefs.fullscreened_pane = None;
            if disable_fit && prefs.fit {
                prefs.fit = false;
                fit_changed = true;
            }
        }
        if fit_changed {
            if let Some(&tab_id) = self.active_tab_ids.get(&client_id) {
                if let Err(e) = self.recompute_tab_size(tab_id) {
                    log::error!("Failed to recompute tab size after single-pane demotion: {e:?}");
                }
            }
        }
    }

    pub fn update_pixel_dimensions(
        &mut self,
        client_id: ClientId,
        pixel_dimensions: PixelDimensions,
    ) {
        self.pixel_dimensions.merge(pixel_dimensions);
        if let Some(character_cell_size) = self.pixel_dimensions.character_cell_size {
            *self.character_cell_size.borrow_mut() = Some(character_cell_size);
        } else if let Some(text_area_size) = self.pixel_dimensions.text_area_size {
            let Some(client_size) = self.client_sizes.get(&client_id).copied() else {
                return;
            };
            if client_size.rows == 0 || client_size.cols == 0 {
                return;
            }
            let character_cell_size = SizeInPixels {
                height: text_area_size.height / client_size.rows,
                width: text_area_size.width / client_size.cols,
            };
            *self.character_cell_size.borrow_mut() = Some(character_cell_size);
        }
    }

    pub fn update_kitty_graphics_support(&mut self, client_id: ClientId, supported: bool) {
        let supported = supported && self.support_kitty_graphics_protocol;
        self.kitty_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        self.push_kitty_host_support_to_tabs();
    }

    fn kitty_host_support_aggregate(&self) -> Option<KittyHostSupport> {
        if !self.support_kitty_graphics_protocol {
            return Some(KittyHostSupport::ProtocolDisabled);
        }
        let capabilities = self.kitty_host_capabilities.borrow();
        if capabilities.is_empty() {
            None
        } else {
            Some(KittyHostSupport::from_host_capability(
                capabilities.values().any(|supported| *supported),
            ))
        }
    }

    fn push_kitty_host_support_to_tabs(&mut self) {
        if let Some(aggregate) = self.kitty_host_support_aggregate() {
            for tab in self.tabs.values_mut() {
                tab.update_kitty_host_support(aggregate);
            }
        }
    }

    pub fn update_sixel_support(&mut self, client_id: ClientId, supported: bool) {
        self.sixel_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        self.push_sixel_host_support_to_tabs();
    }

    fn sixel_host_support_aggregate(&self) -> Option<bool> {
        let capabilities = self.sixel_host_capabilities.borrow();
        if capabilities.is_empty() {
            None
        } else {
            Some(capabilities.values().any(|supported| *supported))
        }
    }

    fn push_sixel_host_support_to_tabs(&mut self) {
        if let Some(aggregate) = self.sixel_host_support_aggregate() {
            for tab in self.tabs.values_mut() {
                tab.update_sixel_host_support(aggregate);
            }
        }
    }

    pub fn update_terminal_background_color(&mut self, background_color_instruction: String) {
        if let Some(AnsiCode::RgbCode((r, g, b))) =
            xparse_color(background_color_instruction.as_bytes())
        {
            let bg_palette_color = PaletteColor::Rgb((r, g, b));
            self.terminal_emulator_colors.borrow_mut().bg = bg_palette_color;
        }
    }

    pub fn update_terminal_foreground_color(&mut self, foreground_color_instruction: String) {
        if let Some(AnsiCode::RgbCode((r, g, b))) =
            xparse_color(foreground_color_instruction.as_bytes())
        {
            let fg_palette_color = PaletteColor::Rgb((r, g, b));
            self.terminal_emulator_colors.borrow_mut().fg = fg_palette_color;
        }
    }

    pub fn update_terminal_color_registers(&mut self, color_registers: Vec<(usize, String)>) {
        let mut terminal_emulator_color_codes = self.terminal_emulator_color_codes.borrow_mut();
        for (color_register, color_sequence) in color_registers {
            terminal_emulator_color_codes.insert(color_register, color_sequence);
        }
    }

    /// Enqueue a whitelisted host-terminal query from pane `pane_id`.
    /// Queries are serialized globally: at most one is in flight to the
    /// client at a time; the rest wait in `forward_queue`. Returns the
    /// allocated token so callers (tests) can observe it if useful.
    pub fn forward_host_query(
        &mut self,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) -> u32 {
        // ColorPaletteMode is answered locally — Zellij already knows
        // the host's mode from its own startup query and from
        // unsolicited DSR 997 updates. Skip the entire forwarding
        // machinery (no token, no in-flight slot, no host round-trip)
        // and write the reply straight to the originating pane's pty.
        if let crate::host_query::HostQuery::ColorPaletteMode = query {
            self.answer_color_palette_mode_query_locally(pane_id);
            return STARTUP_SENTINEL_TOKEN; // sentinel: no real forward happened
        }
        if let crate::host_query::HostQuery::ClipboardContent { .. } = query {
            if !self.paste_buffer_read_enabled {
                let _ = self.resume_pane_after_forward(pane_id, Vec::new());
                return STARTUP_SENTINEL_TOKEN;
            }
            return self.enqueue_clipboard_forward(pane_id, query);
        }
        let token = self.next_forward_token;
        // Skip over the reserved sentinel (0) on wrap; allocate a fresh
        // u32 for every forward.
        self.next_forward_token = self.next_forward_token.wrapping_add(1);
        if self.next_forward_token == STARTUP_SENTINEL_TOKEN {
            self.next_forward_token = 1;
        }
        if self.forward_in_flight_token.is_some() {
            self.forward_queue.push_back(PendingForward {
                token,
                pane_id,
                query,
            });
        } else {
            self.dispatch_forward(token, pane_id, query);
        }
        token
    }

    fn enqueue_clipboard_forward(
        &mut self,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) -> u32 {
        let token = self.next_forward_token;
        self.next_forward_token = self.next_forward_token.wrapping_add(1);
        if self.next_forward_token == STARTUP_SENTINEL_TOKEN {
            self.next_forward_token = 1;
        }
        if self.clipboard_forward_in_flight_token.is_some() {
            self.clipboard_forward_queue.push_back(PendingForward {
                token,
                pane_id,
                query,
            });
        } else {
            self.dispatch_clipboard_forward(token, pane_id, query);
        }
        token
    }

    fn dispatch_clipboard_forward(
        &mut self,
        token: u32,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) {
        let query_bytes = query.to_query_bytes();
        self.pending_clipboard_forwards
            .insert(token, PendingForwardEntry { pane_id, query });
        self.clipboard_forward_in_flight_token = Some(token);
        let _ = self
            .bus
            .senders
            .send_to_server(ServerInstruction::ForwardQueryToHost(
                token,
                query_bytes,
                true,
            ));
        let senders = self.bus.senders.clone();
        crate::global_async_runtime::get_tokio_runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(
                SERVER_CLIPBOARD_FORWARD_TIMEOUT_MS,
            ))
            .await;
            let _ = senders.send_to_screen(ScreenInstruction::ForwardedReplyFromHost {
                token,
                reply_bytes: Vec::new(),
            });
        });
    }

    fn handle_clipboard_reply(&mut self, token: u32, reply_bytes: Vec<u8>) -> Result<()> {
        if let Some(PendingForwardEntry { pane_id, query }) =
            self.pending_clipboard_forwards.remove(&token)
        {
            let payload = if reply_bytes.is_empty() {
                query.empty_reply_bytes()
            } else {
                reply_bytes
            };
            self.resume_pane_after_forward(pane_id, payload)?;
        }
        self.clipboard_forward_in_flight_token = None;
        while let Some(next) = self.clipboard_forward_queue.pop_front() {
            if !self.pane_exists(&next.pane_id) {
                continue;
            }
            self.dispatch_clipboard_forward(next.token, next.pane_id, next.query);
            break;
        }
        Ok(())
    }

    fn pane_exists(&self, pane_id: &PaneId) -> bool {
        self.tabs.values().any(|tab| tab.has_pane_with_pid(pane_id))
    }

    fn answer_color_palette_mode_query_locally(&mut self, pane_id: PaneId) {
        if matches!(pane_id, PaneId::Plugin(_)) {
            return;
        }
        let code: u8 = match self.effective_host_terminal_theme_mode() {
            HostTerminalThemeMode::Dark => 1,
            HostTerminalThemeMode::Light => 2,
        };
        let reply = format!("\u{1b}[?997;{}n", code).into_bytes();
        // Route via Tab so the reply lands on the pane in the correct
        // stream position and any PTY input the app emitted while
        // waiting is replayed.
        let _ = self.resume_pane_after_forward(pane_id, reply);
    }

    fn effective_host_terminal_theme_mode(&self) -> HostTerminalThemeMode {
        self.host_terminal_theme_mode.unwrap_or_else(|| {
            match detect_theme_hue(self.style.colors.text_unselected.background) {
                ThemeHue::Dark => HostTerminalThemeMode::Dark,
                ThemeHue::Light => HostTerminalThemeMode::Light,
            }
        })
    }

    /// Dispatch a forward to the client and mark the slot as in-flight.
    /// Spawns a one-shot timeout task on the global tokio runtime that
    /// synthesizes an empty reply for `token` after
    /// `SERVER_FORWARD_TIMEOUT_MS`. The handler's token-equality guard
    /// makes the timeout a no-op if a real reply arrived first, so no
    /// explicit cancellation is needed.
    fn dispatch_forward(
        &mut self,
        token: u32,
        pane_id: PaneId,
        query: crate::host_query::HostQuery,
    ) {
        let query_bytes = query.to_query_bytes();
        self.pending_forwarded_queries
            .insert(token, PendingForwardEntry { pane_id, query });
        self.forward_in_flight_token = Some(token);
        let _ = self
            .bus
            .senders
            .send_to_server(ServerInstruction::ForwardQueryToHost(
                token,
                query_bytes,
                false,
            ));
        let senders = self.bus.senders.clone();
        crate::global_async_runtime::get_tokio_runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(SERVER_FORWARD_TIMEOUT_MS)).await;
            let _ = senders.send_to_screen(ScreenInstruction::ForwardedReplyFromHost {
                token,
                reply_bytes: Vec::new(),
            });
        });
    }

    /// Handle a host-reply observed by the client for token `token`.
    /// Writes the bytes to the originating pane's pty (if still present)
    /// and releases the in-flight slot so the next queued forward can
    /// dispatch.
    ///
    /// An empty `reply_bytes` is treated as "nobody answered" — either
    /// because no client was attached to forward the query, or the
    /// chosen client's host timed out. In that case we try to answer
    /// from Zellij's cached view of the host (pixel dims, bg/fg,
    /// palette) so the pane still gets a well-formed reply instead of
    /// zero bytes. If the cache has nothing relevant either, the pane
    /// receives an empty write and the app decides what to do.
    pub fn handle_forwarded_reply_from_host(
        &mut self,
        token: u32,
        reply_bytes: Vec<u8>,
    ) -> Result<()> {
        if self.clipboard_forward_in_flight_token == Some(token) {
            return self.handle_clipboard_reply(token, reply_bytes);
        }
        if self.pending_clipboard_forwards.contains_key(&token) {
            log::debug!(
                "Dropping stale clipboard reply (token={}, len={} bytes)",
                token,
                reply_bytes.len(),
            );
            return Ok(());
        }
        // Stale-reply guard. Both the real `ForwardedReplyFromHost`
        // path and the server-side timeout path land here. If a real
        // reply landed first (releasing the slot AND dispatching the
        // next queued forward), the late timeout's empty reply for the
        // already-cleared token must be a no-op — releasing the slot
        // again would clobber the new in-flight forward. The same
        // logic also rejects duplicate replies and replies for tokens
        // belonging to a previously-closed pane.
        if self.forward_in_flight_token != Some(token) {
            log::debug!(
                "Dropping stale forwarded reply (token={}, in-flight={:?}, len={} bytes)",
                token,
                self.forward_in_flight_token,
                reply_bytes.len(),
            );
            return Ok(());
        }
        if let Some(entry) = self.pending_forwarded_queries.remove(&token) {
            let PendingForwardEntry { pane_id, query } = entry;
            match pane_id {
                PaneId::Terminal(_) => {
                    let payload = if reply_bytes.is_empty() {
                        self.synthesize_cached_reply(&query)
                    } else {
                        reply_bytes
                    };
                    // Route via Tab so the reply lands on the pane in
                    // the correct stream position and any PTY input
                    // the app emitted while waiting is replayed.
                    self.resume_pane_after_forward(pane_id, payload)?;
                },
                PaneId::Plugin(_) => {
                    // Plugin panes do not issue whitelisted host queries;
                    // if we reach here the mapping was populated
                    // erroneously. Drop the reply.
                    log::warn!(
                        "Discarding host reply for plugin pane (token={}); plugins do not forward CSI/OSC queries",
                        token
                    );
                },
            }
        }
        // Release the slot and dispatch the next queued forward, if any.
        self.forward_in_flight_token = None;
        if let Some(next) = self.forward_queue.pop_front() {
            self.dispatch_forward(next.token, next.pane_id, next.query);
        }
        Ok(())
    }

    /// Deliver a forwarded reply (or cache-fallback synthesis, or a
    /// locally-answered query payload) to the originating pane via
    /// the owning Tab. The Tab handler writes the bytes to PTY and
    /// then re-feeds any PTY input that was buffered while the pane
    /// was forward-paused, preserving query/reply ordering.
    pub fn resume_pane_after_forward(
        &mut self,
        pane_id: PaneId,
        reply_bytes: Vec<u8>,
    ) -> Result<()> {
        let terminal_id = match pane_id {
            PaneId::Terminal(id) => id,
            PaneId::Plugin(_) => {
                // Plugin panes do not forward queries — dropping is
                // the correct behaviour matching the existing
                // forwarding path's plugin-pane guard.
                return Ok(());
            },
        };
        let mut found = false;
        for tab in self.tabs.values_mut() {
            if tab.has_pane_with_pid(&pane_id) {
                tab.resume_pane_after_forward(terminal_id, reply_bytes.clone())
                    .non_fatal();
                found = true;
                break;
            }
        }
        if !found {
            // No tab owns the pane — the pane was closed between the
            // forward dispatch and the reply arrival. The terminal id
            // is probably defunct; the write goes to the PTY writer
            // anyway because an empty payload is the canonical "host
            // declined" reply some apps rely on, and a non-empty
            // payload may still be deliverable until the pty drains.
            let _ = self
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(reply_bytes, terminal_id, None));
            log::debug!(
                "ResumePaneAfterForward: pane {:?} not in any tab, fell back to direct PTY write",
                pane_id
            );
        }
        Ok(())
    }

    /// Returns a mutable reference to this [`Screen`]'s tabs.
    pub fn get_tabs_mut(&mut self) -> &mut BTreeMap<usize, Tab> {
        &mut self.tabs
    }

    pub fn get_tabs(&self) -> &BTreeMap<usize, Tab> {
        &self.tabs
    }

    /// Returns an immutable reference to this [`Screen`]'s active [`Tab`].
    pub fn get_active_tab(&self, client_id: ClientId) -> Result<&Tab> {
        match self.active_tab_ids.get(&client_id) {
            Some(tab) => self
                .tabs
                .get(tab)
                .ok_or_else(|| anyhow!("active tab {} does not exist", tab)),
            None => Err(anyhow!("active tab not found for client {:?}", client_id)),
        }
    }

    pub fn get_client_input_mode(&self, client_id: ClientId) -> Option<InputMode> {
        self.get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_client_input_mode(client_id))
    }

    pub fn get_first_client_id(&self) -> Option<ClientId> {
        self.active_tab_ids.keys().next().copied()
    }

    fn resolve_tab_and_client(
        &self,
        requested_client_id: ClientId,
        explicit_tab_id: Option<usize>,
    ) -> Option<(usize, ClientId)> {
        let client_id = self
            .active_tab_ids
            .contains_key(&requested_client_id)
            .then_some(requested_client_id)
            .or_else(|| self.get_first_client_id())?;
        explicit_tab_id
            .map(|tab_id| (tab_id, client_id))
            .or_else(|| {
                self.active_tab_ids
                    .get(&client_id)
                    .map(|tab_id| (*tab_id, client_id))
            })
    }

    /// Returns an immutable reference to this [`Screen`]'s previous active [`Tab`].
    /// Consumes the last entry in tab history.
    pub fn get_previous_tab(&mut self, client_id: ClientId) -> Result<Option<&Tab>> {
        Ok(
            match self
                .tab_history
                .get_mut(&client_id)
                .with_context(|| {
                    format!("failed to retrieve tab history for client {client_id:?}")
                })?
                .pop()
            {
                Some(tab) => self.tabs.get(&tab),
                None => None,
            },
        )
    }

    /// Returns a mutable reference to this [`Screen`]'s active [`Tab`].
    pub fn get_active_tab_mut(&mut self, client_id: ClientId) -> Result<&mut Tab> {
        match self.active_tab_ids.get(&client_id) {
            Some(tab) => self
                .tabs
                .get_mut(tab)
                .ok_or_else(|| anyhow!("active tab {} does not exist", tab)),
            None => Err(anyhow!("active tab not found for client {:?}", client_id)),
        }
    }

    /// Clear bell notification for the currently focused pane of the given client.
    /// Also cancels any running flash jobs if applicable.
    pub fn clear_bell_for_focused_pane(&mut self, client_id: ClientId) {
        let tab_id_and_pane_id: Option<(usize, PaneId)> =
            self.get_active_tab_mut(client_id).ok().and_then(|tab| {
                tab.get_active_pane_id(client_id)
                    .map(|pane_id| (tab.id, pane_id))
            });
        if let Some((tab_id, focused_pane_id)) = tab_id_and_pane_id {
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                if tab.panes_with_pending_bell.contains(&focused_pane_id) {
                    let tab_had_bell = tab.tab_has_pending_bell;
                    tab.clear_bell_notification_for_pane(focused_pane_id);
                    let tab_bell_now_cleared = tab_had_bell && !tab.tab_has_pending_bell;
                    let _ =
                        self.bus
                            .senders
                            .send_to_background_jobs(BackgroundJob::StopFlashPaneBell(vec![
                                focused_pane_id,
                            ]));
                    if tab_bell_now_cleared {
                        let _ = self
                            .bus
                            .senders
                            .send_to_background_jobs(BackgroundJob::StopFlashTabBell(tab_id));
                    }
                }
            }
        }
    }

    /// Clear bell notification for a specific pane ID in the given client's active tab.
    pub fn clear_bell_for_pane_id(&mut self, pane_id: PaneId, client_id: ClientId) {
        let tab_id: Option<usize> = self.get_active_tab_mut(client_id).ok().map(|tab| tab.id);
        if let Some(tab_id) = tab_id {
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                if tab.panes_with_pending_bell.contains(&pane_id) {
                    let tab_had_bell = tab.tab_has_pending_bell;
                    tab.clear_bell_notification_for_pane(pane_id);
                    let tab_bell_now_cleared = tab_had_bell && !tab.tab_has_pending_bell;
                    let _ = self
                        .bus
                        .senders
                        .send_to_background_jobs(BackgroundJob::StopFlashPaneBell(vec![pane_id]));
                    if tab_bell_now_cleared {
                        let _ = self
                            .bus
                            .senders
                            .send_to_background_jobs(BackgroundJob::StopFlashTabBell(tab_id));
                    }
                }
            }
        }
    }

    pub fn show_floating_panes_in_tab(
        &mut self,
        client_id: ClientId,
        tab_id: Option<usize>,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let tab = match tab_id {
            Some(id) => self.tabs.get_mut(&id),
            None => self.get_active_tab_mut(client_id).ok(),
        };
        match tab {
            None => {
                let mut completion = completion;
                if let Some(c) = completion.as_mut() {
                    c.set_exit_status(1);
                    c.set_error_message("Tab not found".to_string());
                }
                drop(completion);
            },
            Some(tab) => tab.show_floating_panes_atomic(completion),
        }
        Ok(())
    }

    pub fn hide_floating_panes_in_tab(
        &mut self,
        client_id: ClientId,
        tab_id: Option<usize>,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let tab = match tab_id {
            Some(id) => self.tabs.get_mut(&id),
            None => self.get_active_tab_mut(client_id).ok(),
        };
        match tab {
            None => {
                let mut completion = completion;
                if let Some(c) = completion.as_mut() {
                    c.set_exit_status(1);
                    c.set_error_message("Tab not found".to_string());
                }
                drop(completion);
            },
            Some(tab) => tab.hide_floating_panes_atomic(completion),
        }
        Ok(())
    }

    pub fn are_floating_panes_visible_in_tab(
        &self,
        client_id: ClientId,
        tab_id: Option<usize>,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let tab = match tab_id {
            Some(id) => self.tabs.get(&id),
            None => self.get_active_tab(client_id).ok(),
        };
        let mut completion = completion;
        match tab {
            None => {
                if let Some(c) = completion.as_mut() {
                    c.set_error_message("Tab not found".to_string());
                }
            },
            Some(tab) => {
                if tab.are_floating_panes_visible() {
                    if let Some(c) = completion.as_mut() {
                        c.set_stdout_message("true".to_string());
                    }
                } else {
                    if let Some(c) = completion.as_mut() {
                        c.set_error_message("false".to_string());
                    }
                }
            },
        }
        drop(completion);
        Ok(())
    }

    /// Creates a new [`Tab`] in this [`Screen`]
    pub fn new_tab(
        &mut self,
        tab_id: usize,
        swap_layouts: (Vec<SwapTiledLayout>, Vec<SwapFloatingLayout>),
        tab_name: Option<String>,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new tab for client {client_id:?}",);

        let client_id = client_id.map(|client_id| {
            if self.get_active_tab(client_id).is_ok() {
                client_id
            } else if let Some(first_client_id) = self.get_first_client_id() {
                first_client_id
            } else {
                client_id
            }
        });

        let tab_name = tab_name.unwrap_or_else(String::new);

        let position = self.tabs.len();
        let tab_size = self.size_for_client(client_id);
        let mut tab = Tab::new(
            tab_id,
            position,
            tab_name,
            tab_size,
            self.character_cell_size.clone(),
            self.stacked_resize.clone(),
            self.stacked_pane_list.clone(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.bus
                .os_input
                .as_ref()
                .with_context(err_context)?
                .clone(),
            self.bus.senders.clone(),
            self.max_panes,
            self.style,
            self.default_mode_info.clone(),
            self.pane_frame_style,
            self.auto_layout,
            self.connected_clients.clone(),
            self.session_is_mirrored,
            client_id,
            self.copy_options.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            swap_layouts,
            self.default_shell.clone(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            self.default_editor.clone(),
            self.web_clients_allowed,
            self.web_sharing,
            self.current_pane_group.clone(),
            self.currently_marking_pane_group.clone(),
            self.advanced_mouse_actions,
            self.mouse_scroll_resize,
            self.mouse_hover_effects,
            self.focus_follows_mouse,
            self.mouse_click_through,
            self.web_server_ip,
            self.web_server_port,
        );
        for (client_id, mode_info) in &self.mode_info {
            tab.change_mode_info(mode_info.clone(), *client_id);
        }
        for dimmed_client_id in &self.dimmed_clients {
            tab.set_client_dimmed(*dimmed_client_id, true);
        }
        if let Some(aggregate) = self.kitty_host_support_aggregate() {
            tab.update_kitty_host_support(aggregate);
        }
        if let Some(aggregate) = self.sixel_host_support_aggregate() {
            tab.update_sixel_host_support(aggregate);
        }
        tab.update_selection_options(self.osc133_command_selection, self.word_separators.clone());
        self.tabs.insert(tab_id, tab);
        Ok(())
    }
    pub fn apply_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_terminal_ids: Vec<(u32, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(u32, HoldForCommand)>,
        new_plugin_ids: HashMap<RunPluginOrAlias, Vec<u32>>,
        tab_id: usize,
        should_change_client_focus: bool,
        client_id_and_is_web_client: (ClientId, bool),
        blocking_terminal: Option<(u32, NotificationEnd)>,
    ) -> Result<()> {
        if !self.tabs.contains_key(&tab_id) {
            // TODO: we should prevent this situation with a UI - eg. cannot close tabs with a
            // pending state
            log::error!("Tab with index {tab_id} not found. Cannot apply layout!");
            return Ok(());
        }
        let (client_id, mut is_web_client) = client_id_and_is_web_client;
        let client_id = if self.get_active_tab(client_id).is_ok() {
            if let Some(connected_client_is_web_client) =
                self.connected_clients.borrow().get(&client_id)
            {
                is_web_client = *connected_client_is_web_client;
            }
            client_id
        } else if let Some(first_client_id) = self.get_first_client_id() {
            if let Some(first_client_is_web_client) =
                self.connected_clients.borrow().get(&first_client_id)
            {
                is_web_client = *first_client_is_web_client;
            }
            first_client_id
        } else {
            client_id
        };
        let err_context = || format!("failed to apply layout for tab {tab_id:?}",);

        let passthrough_affected_client_ids: Vec<ClientId> = if should_change_client_focus {
            if self.session_is_mirrored {
                self.connected_clients
                    .borrow()
                    .iter()
                    .map(|(c, _i)| *c)
                    .collect()
            } else {
                vec![client_id]
            }
        } else {
            vec![]
        };
        let passthrough_old_focused_panes: Vec<(ClientId, Option<PaneId>)> =
            passthrough_affected_client_ids
                .iter()
                .map(|c| (*c, self.get_active_pane_id(c)))
                .collect();

        // move the relevant clients out of the current tab and place them in the new one
        let drained_clients = if should_change_client_focus {
            if self.session_is_mirrored {
                let client_mode_infos_in_source_tab = if let Ok(active_tab) =
                    self.get_active_tab_mut(client_id)
                {
                    let client_mode_infos_in_source_tab = active_tab.drain_connected_clients(None);
                    if active_tab.has_no_connected_clients() {
                        active_tab
                            .visible(false)
                            .with_context(err_context)
                            .non_fatal();
                    }
                    Some(client_mode_infos_in_source_tab)
                } else {
                    None
                };
                let all_connected_clients: Vec<ClientId> = self
                    .connected_clients
                    .borrow()
                    .iter()
                    .map(|(c, _i)| *c)
                    .collect();
                for client_id in all_connected_clients {
                    self.update_client_tab_focus(client_id, tab_id);
                }
                client_mode_infos_in_source_tab
            } else if let Ok(active_tab) = self.get_active_tab_mut(client_id) {
                let client_mode_info_in_source_tab =
                    active_tab.drain_connected_clients(Some(vec![client_id]));
                if active_tab.has_no_connected_clients() {
                    active_tab
                        .visible(false)
                        .with_context(err_context)
                        .non_fatal();
                }
                self.update_client_tab_focus(client_id, tab_id);
                Some(client_mode_info_in_source_tab)
            } else {
                None
            }
        } else {
            None
        };

        if !self.active_tab_ids.contains_key(&client_id) {
            self.connected_clients
                .borrow_mut()
                .insert(client_id, is_web_client);
        }

        // apply the layout to the new tab
        self.tabs
            .get_mut(&tab_id)
            .context("couldn't find tab with index {tab_id}")
            .and_then(|tab| {
                tab.apply_layout(
                    layout,
                    floating_panes_layout,
                    new_terminal_ids,
                    new_floating_terminal_ids,
                    new_plugin_ids,
                    client_id,
                    blocking_terminal,
                )?;
                tab.update_input_modes()?;

                if let Some(drained_clients) = drained_clients {
                    tab.visible(true)?;
                    tab.add_multiple_clients(drained_clients)?;
                }
                // the tab keeps the size its viewers gave it here; `recompute_tab_size` below
                // reflows it against the global pane center
                let tab_size = tab.size;
                tab.resize_whole_tab(tab_size).with_context(err_context)?;
                tab.set_force_render();
                Ok(())
            })
            .with_context(err_context)?;

        if !self.active_tab_ids.contains_key(&client_id) {
            // this means this is a new client and we need to add it to our state properly
            self.add_client(client_id, is_web_client)
                .with_context(err_context)?;
        }

        self.recompute_tab_size(tab_id).with_context(err_context)?;

        for (affected_client_id, old_pane_id) in passthrough_old_focused_panes {
            let new_pane_id = self.get_active_pane_id(&affected_client_id);
            if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                self.report_key_passthrough_state(affected_client_id, old, new);
            }
        }

        self.log_and_report_session_state()
            .and_then(|_| self.render(None))
            .with_context(err_context)
    }

    pub fn add_client(&mut self, client_id: ClientId, is_web_client: bool) -> Result<()> {
        let err_context = |tab_index| {
            format!("failed to attach client {client_id} to tab with index {tab_index}")
        };

        // Set followed_client_id to the first regular client if not already set
        if self.followed_client_id.is_none() && !self.watcher_clients.contains_key(&client_id) {
            self.followed_client_id = Some(client_id);
        }

        let mut tab_history = vec![];
        if let Some((_first_client, first_tab_history)) = self.tab_history.iter().next() {
            tab_history = first_tab_history.clone();
        }

        let is_watcher = self.watcher_clients.contains_key(&client_id);
        let attach_to_first_tab_on_tiled_surface = is_web_client && !is_watcher;
        let first_tab_index = self
            .tabs
            .iter()
            .min_by_key(|(_tab_index, tab)| tab.position)
            .map(|(tab_index, _tab)| *tab_index);

        let tab_index = if let Some(first_tab_index) =
            first_tab_index.filter(|_| attach_to_first_tab_on_tiled_surface)
        {
            first_tab_index
        } else if let Some((_first_client, first_active_tab_index)) =
            self.active_tab_ids.iter().next()
        {
            *first_active_tab_index
        } else if self.tabs.contains_key(&self.global_last_active_tab_id) {
            self.global_last_active_tab_id
        } else if self.tabs.contains_key(&0) {
            0
        } else if let Some(tab_index) = self.tabs.keys().next() {
            tab_index.to_owned()
        } else {
            bail!("Can't find a valid tab to attach client to!");
        };

        self.active_tab_ids.insert(client_id, tab_index);
        // a reattaching client id inherits a blank terminal, not the fill the last one was sent
        self.unowned_strip_fill.remove(&client_id);
        self.client_kitty_host_state.borrow_mut().remove(&client_id);
        self.connected_clients
            .borrow_mut()
            .insert(client_id, is_web_client);
        if is_web_client {
            self.kitty_host_capabilities
                .borrow_mut()
                .insert(client_id, false);
            self.push_kitty_host_support_to_tabs();
            self.sixel_host_capabilities
                .borrow_mut()
                .insert(client_id, false);
            self.push_sixel_host_support_to_tabs();
        }
        self.tab_history.insert(client_id, tab_history);
        let tab = self
            .tabs
            .get_mut(&tab_index)
            .with_context(|| err_context(tab_index))?;
        tab.add_client(client_id, None)
            .with_context(|| err_context(tab_index))?;
        if attach_to_first_tab_on_tiled_surface && tab.are_floating_panes_visible() {
            tab.hide_floating_panes();
        }
        // the tab force-renders itself for the new client (see Tab::add_client), but global panes
        // render diff-based and would otherwise emit nothing for a client that hasn't seen them
        self.global_panes.set_force_render();
        // a layout global pane asked for focus before any client existed - this is that client
        if let Some(global_pane_id) = self.pending_global_pane_focus.take() {
            if self.global_panes.pane(global_pane_id).is_some() {
                self.focus_global_pane(global_pane_id, client_id)
                    .non_fatal();
            }
        }
        self.recompute_tab_size(tab_index)
            .with_context(|| err_context(tab_index))?;
        if !self.nested_ancestry.is_empty() || !self.host_descend_keys.is_empty() {
            self.update_all_clients_nesting_mode_info();
        }
        Ok(())
    }

    pub fn remove_client(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to remove client {client_id}");

        self.notification_mouse_captures.remove(&client_id);
        self.set_client_dimmed(client_id, false, None);
        self.suspended_global_focus.remove(&client_id);
        let passthrough_panes: Vec<PaneId> = self
            .nested_guest_choices
            .iter()
            .filter(|((choice_client_id, _), choice)| {
                *choice_client_id == client_id && choice.enters_passthrough()
            })
            .map(|((_, choice_pane_id), _)| *choice_pane_id)
            .collect();
        for pane_id in passthrough_panes {
            let _ = self
                .bus
                .senders
                .send_to_server(ServerInstruction::KeyPassthroughChanged(
                    client_id, pane_id, pane_id, false, None, true,
                ));
        }
        self.nested_guest_choices
            .retain(|(choice_client_id, _), _| *choice_client_id != client_id);
        for tab in self.tabs.values_mut() {
            tab.clear_guest_modal_for_client_on_all_panes(client_id);
            tab.clear_guest_choice_indicator_for_client_on_all_panes(client_id);
        }

        // If the followed client disconnected, find the next regular client
        if Some(client_id) == self.followed_client_id {
            // Try to find another regular (non-watcher) client
            self.followed_client_id = self
                .connected_clients
                .borrow()
                .keys()
                .copied()
                .find(|id| !self.watcher_clients.contains_key(id) && id != &client_id);

            // If no regular client remains but we have watchers, keep the old followed_client_id
            // for terminal rendering (plugins will use their last state)
            if self.followed_client_id.is_none() && !self.watcher_clients.is_empty() {
                self.followed_client_id = Some(client_id); // Keep the disconnected client's ID
            }
        }

        if let Some(prefs) = self.mobile_web_prefs.remove(&client_id) {
            if let Some(pane_id) = prefs.fullscreened_pane {
                self.unset_pane_no_ui_fullscreen(pane_id);
            }
        }

        for tab in self.tabs.values_mut() {
            tab.remove_client(client_id);
            if tab.has_no_connected_clients() {
                tab.visible(false).with_context(err_context)?;
            }
        }

        let previously_active_tab_id = self.active_tab_ids.get(&client_id).copied();
        if let Some(prev_tab_id) = previously_active_tab_id {
            self.global_last_active_tab_id = prev_tab_id;
            self.active_tab_ids.remove(&client_id);
        }
        if self.tab_history.contains_key(&client_id) {
            self.tab_history.remove(&client_id);
        }
        self.connected_clients.borrow_mut().remove(&client_id);
        self.client_kitty_host_state.borrow_mut().remove(&client_id);
        let removed_kitty_capability = self
            .kitty_host_capabilities
            .borrow_mut()
            .remove(&client_id)
            .is_some();
        if removed_kitty_capability {
            self.push_kitty_host_support_to_tabs();
        }
        let removed_sixel_capability = self
            .sixel_host_capabilities
            .borrow_mut()
            .remove(&client_id)
            .is_some();
        if removed_sixel_capability {
            self.push_sixel_host_support_to_tabs();
        }
        self.client_sizes.remove(&client_id);
        self.unowned_strip_fill.remove(&client_id);
        self.pane_render_subscribers.remove(&client_id);
        self.global_panes.defocus_pane(client_id);
        self.global_panes.stop_selecting_with_mouse(client_id);
        self.global_panes.stop_mouse_resize(client_id);
        self.revert_fit_disabled_without_reference_client()
            .with_context(err_context)?;
        if let Some(prev_tab_id) = previously_active_tab_id {
            self.recompute_tab_size(prev_tab_id)
                .with_context(err_context)?;
        }
        if self.nested_via_client_id == Some(client_id) {
            self.nested_via_client_id = None;
            self.nested_ancestry = vec![];
            self.host_descended = false;
            self.host_descend_keys = vec![];
            self.update_all_clients_nesting_mode_info();
            self.broadcast_ancestry_update_to_guests();
        }
        self.log_and_report_session_state().non_fatal();
        Ok(())
    }

    pub fn add_watcher_client(&mut self, client_id: ClientId) -> Result<()> {
        // Initialize with a default size - will be updated when we receive the actual size
        let default_size = Size { rows: 24, cols: 80 }; // Reasonable default
        self.watcher_clients
            .insert(client_id, WatcherState::new(default_size));

        // Force a full render for the new watcher
        // This ensures they get complete state, not just delta
        self.render(None)?;

        Ok(())
    }

    pub fn remove_watcher_client(&mut self, client_id: ClientId) {
        self.watcher_clients.remove(&client_id);
    }

    pub fn set_followed_client(&mut self, client_id: ClientId) -> Result<()> {
        self.followed_client_id = Some(client_id);
        // Trigger re-render with new followed client
        self.render(None)?;
        Ok(())
    }

    pub fn set_watcher_size(&mut self, client_id: ClientId, size: Size) {
        // Update size if this client is a watcher
        if let Some(watcher_state) = self.watcher_clients.get_mut(&client_id) {
            watcher_state.set_size(size);
            watcher_state.set_force_render();
        }
    }

    pub fn update_active_tab_name(&mut self, buf: Vec<u8>, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to update active tabs name for client id: {client_id:?}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        match client_id {
            Some(client_id) => {
                let s = str::from_utf8(&buf)
                    .with_context(|| format!("failed to construct tab name from buf: {buf:?}"))
                    .with_context(err_context)?;
                match self.get_active_tab_mut(client_id) {
                    Ok(active_tab) => {
                        match s {
                            "\0" => {
                                active_tab.name = String::new();
                            },
                            "\u{007F}" | "\u{0008}" => {
                                // delete and backspace keys
                                active_tab.name.pop();
                            },
                            c => {
                                active_tab
                                    .name
                                    .push_str(&clean_string_from_control_and_linebreak(c));
                            },
                        }
                        self.log_and_report_session_state()
                            .with_context(err_context)
                    },
                    Err(err) => {
                        Err::<(), _>(err).with_context(err_context).non_fatal();
                        Ok(())
                    },
                }
            },
            None => Ok(()),
        }
    }
    pub fn undo_active_rename_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to undo active tab rename for client {}", client_id);

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };
        match client_id {
            Some(client_id) => {
                match self.get_active_tab_mut(client_id) {
                    Ok(active_tab) => {
                        if active_tab.name != active_tab.prev_name {
                            active_tab.name = active_tab.prev_name.clone();
                            self.log_and_report_session_state()
                                .context("failed to undo renaming of active tab")?;
                        }
                    },
                    Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
                };
                Ok(())
            },
            None => Ok(()),
        }
    }

    pub fn move_active_tab_to_left(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || "Failed to move active tab left";
        if self.tabs.len() < 2 {
            debug!("cannot move tab to left: only one tab exists");
            return Ok(());
        }
        let Some(client_id) = self.client_id(client_id) else {
            return Ok(());
        };

        match self.get_active_tab(client_id) {
            Ok(active_tab) => {
                let active_tab_pos = active_tab.position;
                if active_tab_pos == 0 {
                    self.rotate_tab_positions_left();
                } else {
                    self.switch_tabs(active_tab_pos, active_tab_pos.saturating_sub(1));
                }
                self.log_and_report_session_state()
                    .context("failed to move tab to left")?;
            },
            Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
        }
        Ok(())
    }

    fn client_id(&mut self, client_id: ClientId) -> Option<u16> {
        if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        }
    }

    /// Switches tabs at two positions, swapping their display order.
    ///
    /// # Arguments
    /// * `active_tab_pos` - Current position of active tab (0-based)
    /// * `other_tab_pos` - Position to swap with (0-based)
    ///
    /// NOTE: this expects positions rather than IDs (see distinction at top of file)
    fn switch_tabs(&mut self, active_tab_pos: usize, other_tab_pos: usize) {
        let Some(active_tab_id) = self
            .tabs
            .values()
            .find(|t| t.position == active_tab_pos)
            .map(|t| t.id)
        else {
            log::error!("Failed to find active tab at position: {}", active_tab_pos);
            return;
        };
        let Some(other_tab_id) = self
            .tabs
            .values()
            .find(|t| t.position == other_tab_pos)
            .map(|t| t.id)
        else {
            log::error!(
                "Failed to find tab to switch to at position: {}",
                other_tab_pos
            );
            return;
        };

        if !self.tabs.contains_key(&active_tab_id) || !self.tabs.contains_key(&other_tab_id) {
            warn!(
                "failed to switch tabs: index {} or {} not found in {:?}",
                active_tab_id,
                other_tab_id,
                self.tabs.keys()
            );
            return;
        }

        // NOTE: Can `expect` here, because we checked that the keys exist above
        let mut active_tab = self
            .tabs
            .remove(&active_tab_id)
            .expect("active tab not found");
        let mut other_tab = self
            .tabs
            .remove(&other_tab_id)
            .expect("other tab not found");

        std::mem::swap(&mut active_tab.position, &mut other_tab.position);

        self.tabs.insert(active_tab_id, active_tab);
        self.tabs.insert(other_tab_id, other_tab);
    }

    fn rotate_tab_positions_left(&mut self) {
        let last_position = self.tabs.len().saturating_sub(1);
        for tab in self.tabs.values_mut() {
            tab.position = if tab.position == 0 {
                last_position
            } else {
                tab.position.saturating_sub(1)
            };
        }
    }

    fn rotate_tab_positions_right(&mut self) {
        let num_tabs = self.tabs.len();
        for tab in self.tabs.values_mut() {
            tab.position = (tab.position + 1) % num_tabs;
        }
    }

    pub fn move_active_tab_to_right(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || "Failed to move active tab right ";
        if self.tabs.len() < 2 {
            debug!("cannot move tab to right: only one tab exists");
            return Ok(());
        }
        let Some(client_id) = self.client_id(client_id) else {
            return Ok(());
        };

        match self.get_active_tab(client_id) {
            Ok(active_tab) => {
                let active_tab_pos = active_tab.position;
                if active_tab_pos == self.tabs.len().saturating_sub(1) {
                    self.rotate_tab_positions_right();
                } else {
                    self.switch_tabs(active_tab_pos, active_tab_pos + 1);
                }
                self.log_and_report_session_state()
                    .context("failed to move tab to the right")?;
            },
            Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
        }
        Ok(())
    }

    pub fn move_tab_by_id(&mut self, tab_id: usize, direction: Direction) -> Result<()> {
        if self.tabs.len() < 2 {
            return Ok(());
        }
        if let Some(tab) = self.tabs.get(&tab_id) {
            let tab_pos = tab.position;
            let num_tabs = self.tabs.len();
            match direction {
                Direction::Left => {
                    if tab_pos == 0 {
                        self.rotate_tab_positions_left();
                    } else {
                        self.switch_tabs(tab_pos, tab_pos.saturating_sub(1));
                    }
                },
                Direction::Right => {
                    if tab_pos == num_tabs.saturating_sub(1) {
                        self.rotate_tab_positions_right();
                    } else {
                        self.switch_tabs(tab_pos, tab_pos + 1);
                    }
                },
                _ => return Ok(()),
            }
            self.log_and_report_session_state()?;
        } else {
            log::error!("Tab with id {} not found", tab_id);
        }
        Ok(())
    }

    /// Move the client's focus one pane in `direction`, escalating to a global pane or the host
    /// session when the tab reports the focus stayed put. A modal holding the keyboard is checked
    /// first: the keypress is then a no-op, never a hop to a global pane or the host.
    pub(crate) fn move_focus_in_direction_or_escalate(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> Result<()> {
        if self.global_pane_handles_direction(direction, client_id) {
            self.sync_scroll_mode_on_focus(client_id)?;
            self.render(None)?;
            return self.log_and_report_session_state();
        }
        if self.active_tab_modal_holds_focus(client_id) {
            return self.log_and_report_session_state();
        }
        let old_pane_id = self.get_active_pane_id(&client_id);
        active_tab_and_connected_client_id!(
            self,
            client_id,
            |tab: &mut Tab, client_id: ClientId| match direction {
                Direction::Left => tab.move_focus_left(client_id),
                Direction::Down => tab.move_focus_down(client_id),
                Direction::Up => tab.move_focus_up(client_id),
                Direction::Right => tab.move_focus_right(client_id),
            },
            ?
        );
        let new_pane_id = self.get_active_pane_id(&client_id);
        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
            if old == new {
                if !self.enter_global_pane_in_direction(direction, client_id) {
                    self.bubble_focus_to_host(client_id, direction);
                }
            } else {
                self.report_key_passthrough_state_with_direction(
                    client_id,
                    old,
                    new,
                    Some(direction),
                );
            }
        }
        self.clear_bell_for_focused_pane(client_id);
        self.add_active_pane_to_group_if_marking(&client_id);
        self.sync_scroll_mode_on_focus(client_id)?;
        self.render(None)?;
        self.log_and_report_session_state()
    }
    pub fn move_focus_left_or_previous_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || {
            format!(
                "failed to move focus left or to previous tab for client {}",
                client_id
            )
        };

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };
        if let Some(client_id) = client_id {
            if self.global_pane_handles_direction(Direction::Left, client_id) {
                return self
                    .log_and_report_session_state()
                    .with_context(err_context);
            }
            // a modal owns the keyboard: the keypress is a no-op, not a hop to another tab,
            // a global pane, or the host session
            if self.active_tab_modal_holds_focus(client_id) {
                return self
                    .log_and_report_session_state()
                    .with_context(err_context);
            }
            let moved = match self.get_active_tab_mut(client_id) {
                Ok(active_tab) => Some(
                    active_tab
                        .move_focus_left(client_id)
                        .with_context(err_context)?,
                ),
                Err(err) => {
                    Err::<(), _>(err).with_context(err_context).non_fatal();
                    None
                },
            };
            // a global pane is inside this session, so it takes precedence over hopping to a tab
            if moved == Some(false)
                && !self.enter_global_pane_in_direction(Direction::Left, client_id)
            {
                if self.tabs.len() == 1 {
                    self.bubble_focus_to_host(client_id, Direction::Left);
                } else {
                    self.switch_tab_prev(Some(Direction::Left), true, client_id)
                        .context("failed to move focus to previous tab")
                        .with_context(err_context)?;
                }
            }
        }
        self.log_and_report_session_state()
            .with_context(err_context)?;
        Ok(())
    }
    pub fn move_focus_right_or_next_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || {
            format!(
                "failed to move focus right or to next tab for client {}",
                client_id
            )
        };

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        if let Some(client_id) = client_id {
            if self.global_pane_handles_direction(Direction::Right, client_id) {
                return self
                    .log_and_report_session_state()
                    .with_context(err_context);
            }
            // a modal owns the keyboard: the keypress is a no-op, not a hop to another tab,
            // a global pane, or the host session
            if self.active_tab_modal_holds_focus(client_id) {
                return self
                    .log_and_report_session_state()
                    .with_context(err_context);
            }
            let moved = match self.get_active_tab_mut(client_id) {
                Ok(active_tab) => Some(
                    active_tab
                        .move_focus_right(client_id)
                        .with_context(err_context)?,
                ),
                Err(err) => {
                    Err::<(), _>(err).with_context(err_context).non_fatal();
                    None
                },
            };
            // a global pane is inside this session, so it takes precedence over hopping to a tab
            if moved == Some(false)
                && !self.enter_global_pane_in_direction(Direction::Right, client_id)
            {
                if self.tabs.len() == 1 {
                    self.bubble_focus_to_host(client_id, Direction::Right);
                } else {
                    self.switch_tab_next(Some(Direction::Right), true, client_id)
                        .context("failed to move focus to next tab")
                        .with_context(err_context)?;
                }
            }
        }
        self.log_and_report_session_state()
            .with_context(err_context)?;
        Ok(())
    }
    pub fn toggle_tab(&mut self, client_id: ClientId) -> Result<()> {
        let tab = self
            .get_previous_tab(client_id)
            .context("failed to toggle tabs")?;
        if let Some(t) = tab {
            let position = t.position;
            self.go_to_tab(position + 1, client_id)
                .context("failed to toggle tabs")?;
        };

        self.log_and_report_session_state()
            .context("failed to toggle tabs")?;
        self.render(None)
    }

    pub fn focus_plugin_pane(
        &mut self,
        run_plugin: &RunPluginOrAlias,
        should_float: bool,
        move_to_focused_tab: bool,
        should_be_in_place: bool,
        client_id: ClientId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> Result<bool> {
        // true => found and focused, false => not
        let err_context = || "failed to focus_plugin_pane".to_string();
        let mut tab_index_and_plugin_pane_id = None;
        let mut plugin_pane_to_move_to_active_tab = None;
        // the tab a pane leaves forgets that it was an overlay - carry the placement over, or the
        // pane lands in the new tab sized against its viewport and painted under the global panes
        let mut overlay_placement = None;
        let focused_tab_index = *self.active_tab_ids.get(&client_id).unwrap_or(&0);
        let all_tabs = self.get_tabs_mut();
        for (tab_index, tab) in all_tabs.iter_mut() {
            if let Some(plugin_pane_id) = tab.find_plugin(&run_plugin) {
                tab_index_and_plugin_pane_id = Some((*tab_index, plugin_pane_id));
                if move_to_focused_tab && focused_tab_index != *tab_index {
                    overlay_placement = tab.overlay_placement_for(plugin_pane_id);
                    plugin_pane_to_move_to_active_tab = tab.extract_pane(plugin_pane_id, true);
                }

                break;
            }
        }
        if let Some(plugin_pane_to_move_to_active_tab) = plugin_pane_to_move_to_active_tab.take() {
            let pane_id = plugin_pane_to_move_to_active_tab.pid();
            let new_active_tab = self.get_active_tab_mut(client_id)?;

            if let Some(placement) = overlay_placement {
                // an overlay is not on the floating layer, so moving one here must not raise it
                new_active_tab.add_overlay_pane(
                    plugin_pane_to_move_to_active_tab,
                    pane_id,
                    placement.kind,
                    placement.coordinates.unwrap_or_default(),
                    true,
                    Some(client_id),
                )?;
            } else if should_float {
                new_active_tab.show_floating_panes();
                new_active_tab.add_floating_pane(
                    plugin_pane_to_move_to_active_tab,
                    pane_id,
                    None,
                    true,
                    Some(client_id),
                )?;
            // TODO: also should_be_in_place
            } else {
                new_active_tab.hide_floating_panes();
                new_active_tab.add_tiled_pane(
                    plugin_pane_to_move_to_active_tab,
                    pane_id,
                    false,
                    Some(client_id),
                )?;
            }
            // Set affected pane ID for CLI client output
            if let Some(ref mut completion) = completion_tx {
                completion.set_affected_pane_id(pane_id);
            }
            return Ok(true);
        }
        match tab_index_and_plugin_pane_id {
            Some((tab_index, plugin_pane_id)) => {
                self.go_to_tab(tab_index + 1, client_id)?;
                self.tabs
                    .get_mut(&tab_index)
                    .with_context(err_context)?
                    .focus_pane_with_id(plugin_pane_id, should_float, should_be_in_place, client_id)
                    .context("failed to focus plugin pane")?;
                self.log_and_report_session_state()
                    .with_context(err_context)?;
                // Set affected pane ID for CLI client output
                if let Some(ref mut completion) = completion_tx {
                    completion.set_affected_pane_id(plugin_pane_id);
                }
                Ok(true)
            },
            None => Ok(false),
        }
    }

    pub fn focus_global_plugin_pane(
        &mut self,
        run_plugin: &RunPluginOrAlias,
        client_id: ClientId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> Result<bool> {
        let Some(plugin_pane_id) = self.global_panes.find_plugin(run_plugin) else {
            return Ok(false);
        };
        self.show_pane(plugin_pane_id)?;
        self.focus_global_pane(plugin_pane_id, client_id)?;
        if let Some(completion) = completion_tx.as_mut() {
            completion.set_affected_pane_id(plugin_pane_id);
        }
        Ok(true)
    }

    /// Build the `Box<dyn Pane>` for a global pane.
    fn build_global_pane_box(
        &mut self,
        pane_id: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<Box<dyn Pane>> {
        Ok(match pane_id {
            PaneId::Terminal(terminal_id) => PaneFactory::terminal(
                terminal_id,
                PaneGeom::default(),
                self.style,
                self.global_panes.terminal_pane_count() + 1,
                initial_pane_title.clone().unwrap_or_default(),
                Rc::new(RefCell::new(LinkHandler::new())),
                self.character_cell_size.clone(),
                self.sixel_image_store.clone(),
                self.kitty_image_store.clone(),
                self.terminal_emulator_colors.clone(),
                self.terminal_emulator_color_codes.clone(),
                initial_pane_title,
                invoked_with,
                self.debug,
                self.arrow_fonts,
                self.styled_underlines,
                self.osc8_hyperlinks,
                self.explicitly_disable_kitty_keyboard_protocol,
                blocking_notification,
            ),
            PaneId::Plugin(plugin_id) => PaneFactory::plugin(
                plugin_id,
                PaneGeom::default(),
                self.bus
                    .senders
                    .to_plugin
                    .as_ref()
                    .context("missing plugin sender for global pane")?
                    .clone(),
                initial_pane_title.clone().unwrap_or_default(),
                String::new(),
                self.sixel_image_store.clone(),
                self.kitty_image_store.clone(),
                self.terminal_emulator_colors.clone(),
                self.terminal_emulator_color_codes.clone(),
                Rc::new(RefCell::new(LinkHandler::new())),
                self.character_cell_size.clone(),
                self.connected_clients.borrow().keys().copied().collect(),
                self.style,
                invoked_with,
                self.debug,
                self.arrow_fonts,
                self.styled_underlines,
            ),
        })
    }

    fn close_global_pane(
        &mut self,
        pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<bool> {
        let displaced: Vec<ClientId> = self
            .global_panes
            .focused_clients()
            .into_iter()
            .filter(|client_id| self.global_panes.focused_pane_id(*client_id) == Some(pane_id))
            .collect();
        let Some((_removed, strip_neighbor)) = self.global_panes.remove(pane_id) else {
            return Ok(false);
        };
        // clients that were on the closed pane land on its strip neighbor, the pane that
        // inherited its cells - matching how closing a tab pane focuses an adjacent one
        if let Some(neighbor_id) = strip_neighbor {
            for client_id in displaced {
                self.focus_global_pane(neighbor_id, client_id).non_fatal();
            }
        }
        let cleanup_result = self
            .bus
            .senders
            .send_to_pty(PtyInstruction::ClosePane(pane_id, completion_tx));
        let reflow_result = self.reflow_global_panes();
        cleanup_result?;
        reflow_result?;
        Ok(true)
    }

    fn spawn_layout_global_panes_once(
        &mut self,
        client_id: ClientId,
        tab_index: usize,
    ) -> Result<usize> {
        if self.layout_global_panes_started {
            return Ok(0);
        }
        self.layout_global_panes_started = true;
        let global_panes = self.default_layout.global_panes.clone();
        for (declaration_order, global_pane) in global_panes.iter().enumerate() {
            let properties = GlobalPaneSpawnContext {
                placement: global_pane.placement,
                borderless: global_pane.borderless,
                initially_hidden: global_pane.hidden,
                should_focus: global_pane.focus,
                declaration_order: Some(declaration_order),
                split: None,
            };
            match global_pane.run.clone() {
                Some(Run::Plugin(run_plugin)) => {
                    self.bus.senders.send_to_plugin(PluginInstruction::Load(
                        Some(false),
                        false,
                        false,
                        None,
                        run_plugin,
                        Some(tab_index),
                        None,
                        client_id,
                        Size::default(),
                        None,
                        None,
                        false,
                        Some(global_pane.focus),
                        None,
                        None,
                        Some(properties),
                        None,
                    ))?;
                },
                run => {
                    let terminal_action = match run {
                        Some(Run::Command(run_command)) => {
                            Some(TerminalAction::RunCommand(run_command))
                        },
                        Some(Run::EditFile(path, line_number, cwd)) => Some(
                            TerminalAction::OpenFile(OpenFilePayload::new(path, line_number, cwd)),
                        ),
                        Some(Run::Cwd(cwd)) => Some(TerminalAction::RunCommand(RunCommand {
                            command: self.default_shell.clone(),
                            cwd: Some(cwd),
                            ..Default::default()
                        })),
                        Some(Run::Plugin(_)) => unreachable!(),
                        None => None,
                    };
                    let client = if global_pane.focus {
                        ClientTabIndexOrPaneId::ClientId(client_id)
                    } else {
                        ClientTabIndexOrPaneId::ClientIdNoFocus(client_id)
                    };
                    self.bus.senders.send_to_pty(PtyInstruction::SpawnTerminal(
                        terminal_action,
                        None,
                        NewPanePlacement::default(),
                        false,
                        client,
                        None,
                        false,
                        Some(properties),
                    ))?;
                },
            }
        }
        Ok(global_panes.len())
    }

    pub fn focus_pane_with_id(
        &mut self,
        pane_id: PaneId,
        should_float_if_hidden: bool,
        should_open_in_place: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || "failed to focus_plugin_pane".to_string();
        if self.global_panes.pane(pane_id).is_some() {
            self.show_pane(pane_id)?;
            self.focus_global_pane(pane_id, client_id)?;
            return Ok(());
        }
        // `go_to_tab` below only releases global focus when the tab actually changes
        self.defocus_global_pane(client_id);
        let tab_index = self
            .tabs
            .iter()
            .find(|(_tab_index, tab)| tab.has_pane_with_pid(&pane_id))
            .map(|(_tab_index, tab)| tab.position);
        match tab_index {
            Some(tab_index) => {
                self.go_to_tab(tab_index + 1, client_id)?;
                self.tabs
                    .iter_mut()
                    .find(|(_, t)| t.position == tab_index)
                    .map(|(_, t)| {
                        t.focus_pane_with_id(
                            pane_id,
                            should_float_if_hidden,
                            should_open_in_place,
                            client_id,
                        )
                    })
                    .with_context(err_context)
                    .non_fatal();
            },
            None => {
                log::error!("Could not find pane with id: {:?}", pane_id);
            },
        };
        Ok(())
    }
    /// Show `client_id` a pane and mark its frame, without moving that client's
    /// keyboard focus.
    ///
    /// The sibling of `focus_pane_with_id` for the caller that wants to point at
    /// a pane rather than hand it the keyboard - a plugin asking "shall I type
    /// into this one?" needs you to see the pane *and* to still be in the plugin
    /// to answer.
    ///
    /// How much of the pane can be shown depends on where the client's focus
    /// already is, because `active_tab_ids` is at once the tab a client sees and
    /// the tab it types into: switching it moves focus, and there is no third
    /// state. The exception is a global pane, which no tab owns - a client
    /// focused on one keeps that focus across a tab switch (see
    /// `switch_active_tab`), and that is the case this trades on. For every
    /// other client the mark is applied where the pane stands and the view is
    /// left alone, which is worth more than a reveal that costs them their
    /// place.
    pub fn reveal_pane_with_id(
        &mut self,
        pane_id: PaneId,
        label: Option<String>,
        client_id: ClientId,
    ) -> Result<()> {
        // First, and whatever else happens below: an unreachable pane still gets
        // marked, so the mark is waiting on it when the client does arrive.
        if self.global_panes.pane(pane_id).is_some() {
            self.with_pane(pane_id, |pane| {
                pane.add_highlight_pane_frame_color_override(label.clone(), Some(client_id))
            });
            // A global pane is in view from every tab; there is nothing to switch to.
            self.show_pane(pane_id)?;
            self.render(None)?;
            self.log_and_report_session_state()?;
            return Ok(());
        }

        let Some((tab_id, tab_position)) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.has_pane_with_pid(&pane_id))
            .map(|(tab_id, tab)| (*tab_id, tab.position))
        else {
            log::error!("Could not find pane with id: {:?}", pane_id);
            return Ok(());
        };

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.add_highlight_pane_frame_color_override(pane_id, label, Some(client_id));
        }

        let pane_is_in_view = self.active_tab_ids.get(&client_id) == Some(&tab_id);
        // Deliberately *not* `defocus_global_pane` first, which is the one line
        // that makes `focus_pane_with_id` a focus operation.
        let can_switch_tabs = self.global_panes.focused_pane_id(client_id).is_some();
        if !pane_is_in_view {
            if !can_switch_tabs {
                self.render(None)?;
                self.log_and_report_session_state()?;
                return Ok(());
            }
            self.go_to_tab(tab_position + 1, client_id)?;
        }

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            // Suppressed, or floating while floating panes are hidden: in the
            // right tab and still not on screen. Unlike the tab-level
            // `focus_pane_with_id` this leaves the tab's own active pane alone,
            // so nothing about where the keyboard goes changes.
            tab.unsuppress_or_expand_pane(pane_id, true);
        }
        self.render(None)?;
        self.log_and_report_session_state()?;
        Ok(())
    }
    pub fn rerun_command_pane_with_id(
        &mut self,
        terminal_pane_id: u32,
        completion_tx: Option<NotificationEnd>,
    ) {
        if let Some(command_to_rerun) = self
            .global_panes
            .pane_mut(PaneId::Terminal(terminal_pane_id))
            .and_then(|pane| pane.rerun())
        {
            self.bus
                .senders
                .send_to_pty(PtyInstruction::ReRunCommandInPane(
                    PaneId::Terminal(terminal_pane_id),
                    command_to_rerun,
                    completion_tx,
                ))
                .non_fatal();
            return;
        }
        let mut found = false;
        for tab in self.tabs.values_mut() {
            if tab.has_pane_with_pid(&PaneId::Terminal(terminal_pane_id)) {
                tab.rerun_terminal_pane_with_id(terminal_pane_id, completion_tx);
                found = true;
                break;
            }
        }
        if !found {
            log::error!(
                "Failed to find terminal pane with id: {} to run",
                terminal_pane_id
            );
        }
    }
    pub fn resize_pane_with_id(&mut self, resize: ResizeStrategy, pane_id: PaneId) -> Result<()> {
        if self.global_panes.pane(pane_id).is_some() {
            let surface = self.surface_size();
            self.global_panes
                .resize_pane(pane_id, resize, surface.into())
                .map_err(|error| anyhow!(error))?;
            return self.reflow_global_panes();
        }
        let mut found = false;
        for tab in self.tabs.values_mut() {
            if tab.has_pane_with_pid(&pane_id) {
                tab.resize_pane_with_id(resize, pane_id).non_fatal();
                found = true;
                break;
            }
        }
        if !found {
            log::error!("Failed to find pane with id: {:?} to resize", pane_id);
        }
        Ok(())
    }
    pub fn replace_pane(
        &mut self,
        new_pane_id: PaneId,
        hold_for_command: HoldForCommand,
        run: Option<Run>,
        pane_title: Option<InitialTitle>,
        close_replaced_pane: bool,
        client_id_tab_index_or_pane_id: ClientTabIndexOrPaneId,
    ) -> Result<()> {
        let suppress_pane = |tab: &mut Tab, pane_id: PaneId, new_pane_id: PaneId| {
            let _ = tab.suppress_pane_and_replace_with_pid(
                pane_id,
                new_pane_id,
                close_replaced_pane,
                run,
                None,
                None,
            );
            if let Some(pane_title) = pane_title {
                let _ = tab.rename_pane(pane_title.as_bytes().to_vec(), new_pane_id);
            }
            if let Some(hold_for_command) = hold_for_command {
                let is_first_run = true;
                tab.hold_pane(new_pane_id, None, is_first_run, hold_for_command)
            }
        };
        match client_id_tab_index_or_pane_id {
            ClientTabIndexOrPaneId::ClientId(client_id)
            | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                active_tab!(self, client_id, |tab: &mut Tab| {
                    match tab.get_active_pane_id(client_id) {
                        Some(pane_id) => {
                            suppress_pane(tab, pane_id, new_pane_id);
                        },
                        None => {
                            log::error!(
                                "Failed to find active pane for client id: {:?}",
                                client_id
                            );
                        },
                    }
                });
            },
            ClientTabIndexOrPaneId::PaneId(pane_id) => {
                let tab_index = self
                    .tabs
                    .iter()
                    .find(|(_tab_index, tab)| tab.has_pane_with_pid(&pane_id))
                    .map(|(_tab_index, tab)| tab.position);
                match tab_index {
                    Some(tab_index) => {
                        if let Some(tab) =
                            self.tabs.iter_mut().find(|(_, t)| t.position == tab_index)
                        {
                            suppress_pane(tab.1, pane_id, new_pane_id);
                        }
                    },
                    None => {
                        log::error!("Could not find pane with id: {:?}", pane_id);
                    },
                };
            },
            ClientTabIndexOrPaneId::TabIndex(_tab_index)
            | ClientTabIndexOrPaneId::TabIndexNoFocus(_tab_index) => {
                log::error!("Cannot replace pane with tab index");
            },
        }
        Ok(())
    }
    pub fn replace_pane_with_existing_pane(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_id_of_existing_pane: PaneId,
        suppress_replaced_pane: bool,
        _completion_tx: Option<NotificationEnd>, // ends here
    ) {
        let Some(tab_index_of_pane_id_to_replace) = self
            .tabs
            .iter()
            .find(|(_tab_index, tab)| tab.has_pane_with_pid(&pane_id_to_replace))
            .map(|(_tab_index, tab)| tab.position)
        else {
            log::error!(
                "Could not find tab with pane_id: {:?} to replace",
                pane_id_to_replace
            );
            return;
        };
        let Some(tab_index_of_existing_pane) = self
            .tabs
            .iter()
            .find(|(_tab_index, tab)| tab.has_pane_with_pid(&pane_id_of_existing_pane))
            .map(|(_tab_index, tab)| tab.position)
        else {
            log::error!(
                "Could not find tab with pane_id: {:?} to be replaced by",
                pane_id_of_existing_pane
            );
            return;
        };
        let Some(extracted_pane_from_other_tab) = self
            .tabs
            .iter_mut()
            .find(|(_, t)| t.position == tab_index_of_existing_pane)
            .and_then(|(_, t)| t.extract_pane(pane_id_of_existing_pane, true))
        else {
            log::error!("Failed to find pane");
            return;
        };
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|(_, t)| t.position == tab_index_of_pane_id_to_replace)
        {
            if suppress_replaced_pane {
                tab.1.suppress_pane_and_replace_with_other_pane(
                    pane_id_to_replace,
                    extracted_pane_from_other_tab,
                    None,
                );
            } else {
                tab.1.close_pane_and_replace_with_other_pane(
                    pane_id_to_replace,
                    extracted_pane_from_other_tab,
                    None,
                );
            }
        }
        let _ = self.log_and_report_session_state();
    }
    pub fn reconfigure(
        &mut self,
        new_keybinds: Keybinds,
        new_default_mode: InputMode,
        theme: Styling,
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
        osc133_command_selection: bool,
        word_separators: String,
        nested_session_handling: NestedSessionHandling,
        dangerously_enable_paste_buffer_read: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let should_support_arrow_fonts = !simplified_ui;
        self.arrow_fonts = should_support_arrow_fonts;

        // global configuration
        self.default_mode_info.update_theme(theme);
        self.default_mode_info
            .update_rounded_corners(rounded_corners);
        // `default_mode_info` is the fallback used by `change_mode` for
        // clients that don't yet have a per-client `mode_info` entry, so its
        // keybinds and base mode must be kept in sync with reconfigures.
        self.default_mode_info.update_keybinds(new_keybinds.clone());
        self.default_mode_info.update_default_mode(new_default_mode);
        self.default_shell = default_shell.clone().unwrap_or_else(|| get_default_shell());
        self.default_editor = default_editor.clone().or_else(|| get_default_editor());
        self.auto_layout = auto_layout;
        self.copy_options.command = copy_command.clone();
        self.copy_options.copy_on_select = copy_on_select;
        self.pane_frame_style = pane_frame_style;
        self.global_panes
            .set_pane_frame_style(pane_frame_style)
            .map_err(|error| anyhow!(error))?;
        self.advanced_mouse_actions = advanced_mouse_actions;
        self.mouse_scroll_resize = mouse_scroll_resize;
        self.mouse_hover_effects = mouse_hover_effects;
        self.visual_bell = visual_bell;
        self.focus_follows_mouse = focus_follows_mouse;
        self.mouse_click_through = mouse_click_through;
        self.osc133_command_selection = osc133_command_selection;
        self.word_separators = word_separators;
        self.nested_session_handling = nested_session_handling;
        self.paste_buffer_read_enabled = dangerously_enable_paste_buffer_read;
        self.default_mode_info
            .update_arrow_fonts(should_support_arrow_fonts);
        self.default_mode_info
            .update_hide_session_name(hide_session_name);
        {
            *self.stacked_resize.borrow_mut() = stacked_resize;
        }
        {
            *self.stacked_pane_list.borrow_mut() = stacked_pane_list;
        }
        if let Some(copy_to_clipboard) = copy_to_clipboard {
            self.copy_options.clipboard = copy_to_clipboard;
        }
        for tab in self.tabs.values_mut() {
            tab.update_theme(theme);
            tab.update_rounded_corners(rounded_corners);
            tab.update_default_shell(default_shell.clone());
            tab.update_default_editor(self.default_editor.clone());
            tab.update_auto_layout(auto_layout);
            tab.update_copy_options(&self.copy_options);
            tab.set_pane_frames(pane_frame_style);
            tab.update_arrow_fonts(should_support_arrow_fonts);
            tab.update_advanced_mouse_actions(advanced_mouse_actions);
            tab.update_mouse_scroll_resize(mouse_scroll_resize);
            tab.update_mouse_hover_effects(mouse_hover_effects);
            tab.update_focus_follows_mouse(focus_follows_mouse);
            tab.update_mouse_click_through(mouse_click_through);
            tab.update_selection_options(osc133_command_selection, self.word_separators.clone());
            tab.sync_stacked_pane_list_mode();
        }
        // Clear hover state when disabled
        if !mouse_hover_effects {
            for tab in self.tabs.values_mut() {
                tab.clear_mouse_hover_state();
            }
        }

        // client specific configuration
        if self.connected_clients_contains(&client_id) {
            let mode_info = self
                .mode_info
                .entry(client_id)
                .or_insert_with(|| self.default_mode_info.clone());
            mode_info.update_keybinds(new_keybinds);
            mode_info.update_default_mode(new_default_mode);
            mode_info.update_theme(theme);
            mode_info.update_arrow_fonts(should_support_arrow_fonts);
            mode_info.update_hide_session_name(hide_session_name);
            for tab in self.tabs.values_mut() {
                tab.change_mode_info(mode_info.clone(), client_id);
                tab.mark_active_pane_for_rerender(client_id);
            }
        }

        // this needs to be done separately at the end because it applies some of the above changes
        // and propagates them to plugins
        for tab in self.tabs.values_mut() {
            tab.update_input_modes()?;
        }
        self.update_global_input_modes(self.active_tab_ids.keys().copied())?;
        self.broadcast_nested_shortcuts();
        Ok(())
    }
    /// Apply a host-reported color-palette theme mode (CSI 2031 / DSR 997).
    ///
    /// This is the Phase 2 entry-point: it
    /// 1. de-duplicates against the last-known mode,
    /// 2. swaps the active palette to the configured `theme_dark` / `theme_light`
    ///    (when both are configured) by reusing the reconfigure propagation,
    /// 3. fans out an `Event::HostTerminalThemeChanged` plugin event,
    /// 4. forwards a `CSI ?997;{1|2}n` DSR onto the pty of every terminal pane
    ///    whose app opted in via `CSI ? 2031 h`.
    pub fn update_host_terminal_theme_mode(&mut self, mode: HostTerminalThemeMode) -> Result<()> {
        let err_context = || "Failed to update host terminal theme mode".to_string();

        // dedupe
        if self.host_terminal_theme_mode == Some(mode) {
            return Ok(());
        }
        self.host_terminal_theme_mode = Some(mode);

        // resolve target styling
        let resolved = match mode {
            HostTerminalThemeMode::Dark => self.host_theme_dark_styling,
            HostTerminalThemeMode::Light => self.host_theme_light_styling,
        };

        // theme propagation when both keys configured and the resolved
        // styling exists. (If only one of theme_dark/theme_light is set,
        // skip auto-switch; the static `theme` stays authoritative.)
        let auto_switch_enabled =
            self.host_theme_dark_styling.is_some() && self.host_theme_light_styling.is_some();
        if auto_switch_enabled {
            if let Some(theme) = resolved {
                self.default_mode_info.update_theme(theme);
                for tab in self.tabs.values_mut() {
                    tab.update_theme(theme);
                }
                // Iterate every connected client (active_tab_ids is the
                // canonical "who is connected" map). Iterating
                // self.mode_info.keys() would skip any client that has
                // never manually changed mode
                let client_ids: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
                let default_for_new = self.default_mode_info.clone();
                for client_id in client_ids {
                    let mode_info = self
                        .mode_info
                        .entry(client_id)
                        .or_insert_with(|| default_for_new.clone());
                    mode_info.update_theme(theme);
                    let mode_info_clone = mode_info.clone();
                    // Push the freshly-themed mode_info into every tab's
                    // per-client mode_info map. `update_input_modes` below
                    // reads from THAT map (not Screen's) when fanning out
                    // ModeUpdate to plugins, so without this step plugins
                    // receive ModeUpdate carrying the *old* style and the
                    // status-bar / tab-bar do not repaint. Also mark the
                    // active pane for rerender so terminal panes refresh
                    // their borders/title using the new palette.
                    for tab in self.tabs.values_mut() {
                        tab.change_mode_info(mode_info_clone.clone(), client_id);
                        tab.mark_active_pane_for_rerender(client_id);
                    }
                }
                for tab in self.tabs.values_mut() {
                    tab.update_input_modes().with_context(err_context)?;
                }
                self.update_global_input_modes(self.active_tab_ids.keys().copied())?;
            } else {
                log::warn!(
                    "host theme auto-switch enabled but resolved styling missing for {:?}",
                    mode
                );
            }
        }

        // fan out the plugin event
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::HostTerminalThemeChanged(mode),
            )]))
            .with_context(err_context)?;

        // forward DSR to opted-in terminal panes
        let mut pty_writes: Vec<(Vec<u8>, u32)> = vec![];
        for tab in self.tabs.values_mut() {
            for pane_id in tab.get_all_pane_ids() {
                if let PaneId::Terminal(terminal_id) = pane_id {
                    if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                        pane.push_color_palette_dsr(mode);
                        for bytes in pane.drain_messages_to_pty() {
                            pty_writes.push((bytes, terminal_id));
                        }
                    }
                }
            }
        }
        for (bytes, terminal_id) in pty_writes {
            let _ = self
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(bytes, terminal_id, None));
        }

        self.render(None)?;
        Ok(())
    }
    /// Apply a manual host-terminal theme mode change requested via the CLI or a
    /// keybinding. Surfaces a clear error to the CLI if the auto-switch gate
    /// (both `theme_dark` and `theme_light` configured) is not satisfied;
    /// otherwise delegates to `update_host_terminal_theme_mode`, which dedupes,
    /// repaints, fans out the plugin event, and forwards DSR to opted-in panes.
    pub fn apply_manual_host_terminal_theme_mode(
        &mut self,
        mode: HostTerminalThemeMode,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> Result<()> {
        let auto_switch_enabled =
            self.host_theme_dark_styling.is_some() && self.host_theme_light_styling.is_some();
        if !auto_switch_enabled {
            if let Some(c) = completion_tx.as_mut() {
                c.set_exit_status(1);
                c.set_error_message(
                    "Manual theme switching requires both `theme_dark` and `theme_light` to be configured."
                        .to_string(),
                );
            }
            return Ok(());
        }
        self.update_host_terminal_theme_mode(mode)
    }
    pub fn toggle_pane_pinned(&mut self, client_id: ClientId) {
        active_tab_and_connected_client_id!(
            self,
            client_id,
            |tab: &mut Tab, client_id: ClientId| {
                tab.toggle_pane_pinned(client_id);
            }
        );
    }
    pub fn set_floating_pane_pinned(&mut self, pane_id: PaneId, should_be_pinned: bool) {
        let mut found = false;
        for tab in self.tabs.values_mut() {
            if tab.has_pane_with_pid(&pane_id) {
                tab.set_floating_pane_pinned(pane_id, should_be_pinned);
                found = true;
                break;
            }
        }
        if !found {
            log::error!(
                "Failed to find pane with id: {:?} to set as pinned",
                pane_id
            );
        }
    }
    pub fn stack_panes(&mut self, mut pane_ids_to_stack: Vec<PaneId>) -> Option<PaneId> {
        // if successful, returns the pane id of the last pane in the stack
        if pane_ids_to_stack.is_empty() {
            log::error!("Got an empty list of pane_ids to stack");
            return None;
        }
        let stack_size = pane_ids_to_stack.len();
        let root_pane_id = pane_ids_to_stack.remove(0);
        let last_pane_id = pane_ids_to_stack.last();
        let Some(root_tab_id) = self
            .tabs
            .iter()
            .find_map(|(tab_id, tab)| {
                if tab.has_pane_with_pid(&root_pane_id) {
                    Some(tab_id)
                } else {
                    None
                }
            })
            .copied()
        else {
            log::error!("Failed to find tab for root_pane_id: {:?}", root_pane_id);
            return None;
        };
        let root_pane_id_is_floating = self
            .tabs
            .get(&root_tab_id)
            .map(|t| t.pane_id_is_floating(&root_pane_id))
            .unwrap_or(false);

        if root_pane_id_is_floating {
            self.tabs.get_mut(&root_tab_id).map(|tab| {
                let _ = tab.toggle_pane_embed_or_floating_for_pane_id(root_pane_id, None);
            });
        }

        let mut panes_to_stack = vec![];
        let target_tab_has_room_for_stack = self
            .tabs
            .get_mut(&root_tab_id)
            .map(|t| t.has_room_for_stack(root_pane_id, stack_size))
            .unwrap_or(false);
        if !target_tab_has_room_for_stack {
            log::error!("No room for stack with root pane id: {:?}", root_pane_id);
            return None;
        }

        for (tab_id, tab) in self.tabs.iter_mut() {
            if tab_id == &root_tab_id {
                // we do this before we extract panes so that the extraction won't trigger a
                // relayout according to the next swapped tiled pane
                tab.set_tiled_panes_damaged();
            }
            for pane_id in &pane_ids_to_stack {
                if tab.has_pane_with_pid(&pane_id) {
                    match tab.extract_pane(*pane_id, false) {
                        Some(pane) => {
                            panes_to_stack.push(pane);
                        },
                        None => {
                            log::error!("Failed to extract pane: {:?}", pane_id);
                        },
                    }
                }
            }
        }
        self.tabs
            .get_mut(&root_tab_id)
            .map(|t| t.stack_panes(root_pane_id, panes_to_stack));
        last_pane_id.copied()
    }
    /// Move or resize overlays where they already are, in the overlay collection.
    ///
    /// No `activate_overlay` sweep afterwards: nothing became an overlay here, so there is no
    /// global-pane focus to suspend that was not already suspended when it was born.
    pub fn change_overlay_coordinates(
        &mut self,
        pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    ) {
        for (pane_id, coordinates) in pane_ids_and_coordinates {
            for (_tab_id, tab) in self.tabs.iter_mut() {
                if tab.has_pane_with_pid(&pane_id) {
                    tab.change_overlay_coordinates(&pane_id, coordinates)
                        .non_fatal();
                    break;
                }
            }
        }
    }
    pub fn change_floating_panes_coordinates(
        &mut self,
        pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    ) {
        let pane_ids: Vec<_> = pane_ids_and_coordinates
            .iter()
            .map(|(pane_id, _)| *pane_id)
            .collect();
        for (pane_id, coordinates) in pane_ids_and_coordinates {
            for (_tab_id, tab) in self.tabs.iter_mut() {
                if tab.has_pane_with_pid(&pane_id) {
                    tab.change_floating_pane_coordinates(&pane_id, coordinates)
                        .non_fatal();
                    break;
                }
            }
        }
        for pane_id in pane_ids {
            self.activate_overlay(pane_id);
        }
    }
    pub fn toggle_pane_borderless(&mut self, pane_id: PaneId) {
        for (_tab_id, tab) in self.tabs.iter_mut() {
            if tab.has_pane_with_pid(&pane_id) {
                tab.toggle_pane_borderless(&pane_id).non_fatal();
                break;
            }
        }
    }
    pub fn set_pane_borderless(&mut self, pane_id: PaneId, borderless: bool) {
        for (_tab_id, tab) in self.tabs.iter_mut() {
            if tab.has_pane_with_pid(&pane_id) {
                tab.set_pane_borderless(&pane_id, borderless).non_fatal();
                break;
            }
        }
    }
    pub fn toggle_pane_in_group(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = "Can't add pane to group";
        let active_tab = self
            .get_active_tab(client_id)
            .with_context(|| err_context)?;
        let active_pane_id = active_tab
            .get_active_pane_id(client_id)
            .with_context(|| err_context)?;
        self.toggle_pane_id_in_group(active_pane_id, &client_id);
        let _ = self.log_and_report_session_state();
        Ok(())
    }
    pub fn toggle_group_marking(&mut self, client_id: ClientId) -> Result<()> {
        let (was_marking_before, marking_pane_group_now) = {
            let mut currently_marking_pane_group = self.currently_marking_pane_group.borrow_mut();
            let previous_value = currently_marking_pane_group
                .remove(&client_id)
                .unwrap_or(false);
            let new_value = !previous_value;
            if new_value {
                currently_marking_pane_group.insert(client_id, true);
            }
            (previous_value, new_value)
        };
        if marking_pane_group_now {
            let active_pane_id = self.get_active_pane_id(&client_id);
            if let Some(active_pane_id) = active_pane_id {
                self.add_pane_id_to_group(active_pane_id, &client_id);
            }
        }
        let value_changed = was_marking_before != marking_pane_group_now;
        if value_changed {
            for tab in self.tabs.values_mut() {
                tab.update_input_modes()?;
            }
            self.update_global_input_modes([client_id])?;
            let _ = self.log_and_report_session_state();
        }
        Ok(())
    }
    fn get_layout_metadata(
        &self,
        default_shell: Option<PathBuf>,
        tab_index: Option<usize>,
    ) -> SessionLayoutMetadata {
        let mut session_layout_metadata = SessionLayoutMetadata::new(self.default_layout.clone());
        if let Some(default_shell) = default_shell {
            session_layout_metadata.update_default_shell(default_shell);
        }
        let first_client_id = self.get_first_client_id();
        let active_tab_index =
            first_client_id.and_then(|client_id| self.active_tab_ids.get(&client_id));

        for (pane, placement, is_hidden) in self.global_panes.panes() {
            let pane_id = pane.pid();
            let focused_clients: Vec<ClientId> = self
                .active_tab_ids
                .keys()
                .filter(|client_id| self.global_panes.focused_pane_id(**client_id) == Some(pane_id))
                .copied()
                .collect();
            let (default_fg, default_bg) = pane.get_pane_default_colors();
            session_layout_metadata.add_global_pane(PaneLayoutMetadata::new_global(
                pane_id,
                pane.position_and_size(),
                pane.borderless(),
                pane.invoked_with().clone(),
                pane.custom_title(),
                !focused_clients.is_empty(),
                if self.serialize_pane_viewport {
                    pane.serialize(self.scrollback_lines_to_serialize)
                } else {
                    None
                },
                focused_clients,
                default_fg,
                default_bg,
                placement,
                is_hidden,
            ));
        }

        // Filter tabs based on optional tab_index parameter
        let tabs_to_process: Vec<_> = self
            .tabs
            .iter()
            .filter(|(idx, _)| tab_index.is_none_or(|target| **idx == target))
            .collect();

        for (tab_index, tab) in tabs_to_process {
            let tab_is_focused = active_tab_index == Some(&tab_index);
            let (tiled_panes, floating_panes, hide_floating_panes) =
                self.tab_panes_for_layout(*tab_index, tab, self.serialize_pane_viewport);
            session_layout_metadata.add_tab(
                tab.name.clone(),
                tab_is_focused,
                hide_floating_panes,
                tiled_panes.into_iter().map(|(_, pane)| pane).collect(),
                floating_panes.into_iter().map(|(_, pane)| pane).collect(),
            );
        }
        session_layout_metadata
    }

    /// One tab's panes as layout metadata, paired with the ids they came from.
    ///
    /// Split out of `get_layout_metadata` so a warp can reuse it, and the reason
    /// to reuse it rather than walk the tab again is everything between here and
    /// the end of the function: a scrollback editor has to be swapped for the
    /// pane it is editing, stack lists have geometry of their own, and their
    /// hidden members have to be put back. Getting any of that wrong produces a
    /// layout that looks plausible and is missing a pane.
    ///
    /// `with_contents` is a parameter rather than a read of
    /// `self.serialize_pane_viewport` because the two callers mean different
    /// things by it: that option governs whether sessions are serialized to disk
    /// in the background, whereas a warp is something the user asked for by name.
    fn tab_panes_for_layout(
        &self,
        tab_index: usize,
        tab: &Tab,
        with_contents: bool,
    ) -> (
        Vec<(PaneId, PaneLayoutMetadata)>,
        Vec<(PaneId, PaneLayoutMetadata)>,
        bool,
    ) {
        {
            let hide_floating_panes = !tab.are_floating_panes_visible();
            // keyed by the stand-in, which is what the loops below are walking:
            // a slot occupied by a scrollback editor serializes as the pane the
            // editor is editing
            let mut scrollback_editor_subjects: HashMap<PaneId, &Box<dyn Pane>> = HashMap::new();
            for (_pane_id, suppressed) in tab.get_suppressed_panes() {
                if let Some(stand_in) = suppressed.restored_by {
                    if stand_in.is_scrollback_editor {
                        scrollback_editor_subjects.insert(stand_in.pane_id, &suppressed.pane);
                    }
                }
            }

            let all_connected_clients: Vec<ClientId> = self
                .connected_clients
                .borrow()
                .iter()
                .map(|(c, _i)| *c)
                .filter(|c| self.active_tab_ids.get(&c) == Some(&tab_index))
                .collect();

            let mut active_pane_ids: HashMap<ClientId, Option<PaneId>> = HashMap::new();
            for connected_client_id in &all_connected_clients {
                active_pane_ids.insert(
                    *connected_client_id,
                    tab.get_active_pane_id(*connected_client_id),
                );
            }

            let stack_list_geoms = tab.stack_list_serialization_geoms();
            let tiled_panes: Vec<(PaneId, PaneLayoutMetadata)> = tab
                .get_tiled_panes()
                .map(|(pane_id, p)| {
                    // here we look to see if this pane triggers any suppressed pane,
                    // and if so we take that suppressed pane - we do this because this
                    // is currently only the case the scrollback editing panes, and
                    // when dumping the layout we want the "real" pane and not the
                    // editor pane
                    let geom_override = stack_list_geoms.get(pane_id).copied();
                    let (pane_id, p) = match scrollback_editor_subjects.remove(pane_id) {
                        Some(subject) => (subject.pid(), subject),
                        None => (*pane_id, p),
                    };
                    (
                        pane_id,
                        p,
                        geom_override.unwrap_or_else(|| p.position_and_size()),
                    )
                })
                .chain(tab.hidden_stack_list_members_for_serialization())
                .map(|(pane_id, p, geom)| {
                    let focused_clients: Vec<ClientId> = active_pane_ids
                        .iter()
                        .filter_map(|(c_id, p_id)| {
                            p_id.and_then(|p_id| if p_id == pane_id { Some(*c_id) } else { None })
                        })
                        .collect();
                    let (default_fg, default_bg) = p.get_pane_default_colors();
                    (
                        pane_id,
                        PaneLayoutMetadata::new(
                            pane_id,
                            geom,
                            p.borderless(),
                            p.invoked_with().clone(),
                            p.custom_title(),
                            !focused_clients.is_empty(),
                            if with_contents {
                                p.serialize(self.scrollback_lines_to_serialize)
                            } else {
                                None
                            },
                            focused_clients,
                            default_fg,
                            default_bg,
                        ),
                    )
                })
                .collect();
            let floating_panes: Vec<(PaneId, PaneLayoutMetadata)> = tab
                .get_floating_panes()
                .map(|(pane_id, p)| {
                    // here we look to see if this pane triggers any suppressed pane,
                    // and if so we take that suppressed pane - we do this because this
                    // is currently only the case the scrollback editing panes, and
                    // when dumping the layout we want the "real" pane and not the
                    // editor pane
                    match scrollback_editor_subjects.remove(pane_id) {
                        Some(subject) => (subject.pid(), subject),
                        None => (*pane_id, p),
                    }
                })
                .map(|(pane_id, p)| {
                    let focused_clients: Vec<ClientId> = active_pane_ids
                        .iter()
                        .filter_map(|(c_id, p_id)| {
                            p_id.and_then(|p_id| if p_id == pane_id { Some(*c_id) } else { None })
                        })
                        .collect();
                    let (default_fg, default_bg) = p.get_pane_default_colors();
                    (
                        pane_id,
                        PaneLayoutMetadata::new(
                            pane_id,
                            p.position_and_size(),
                            false, // floating panes are never borderless
                            p.invoked_with().clone(),
                            p.custom_title(),
                            !focused_clients.is_empty(),
                            if with_contents {
                                p.serialize(self.scrollback_lines_to_serialize)
                            } else {
                                None
                            },
                            focused_clients,
                            default_fg,
                            default_bg,
                        ),
                    )
                })
                .collect();
            (tiled_panes, floating_panes, hide_floating_panes)
        }
    }
    fn handle_plugin_render_assets(
        &mut self,
        plugin_render_assets: Vec<PluginRenderAsset>,
        unblock_cli_pipes: bool,
    ) -> Result<Vec<PluginRenderAsset>> {
        let mut handled_render_assets = vec![];
        let mut unhandled_render_assets = vec![];
        for mut plugin_render_asset in plugin_render_assets {
            let plugin_id = plugin_render_asset.plugin_id;
            let client_id = plugin_render_asset.client_id;
            let mut vte_bytes = Some(std::mem::take(&mut plugin_render_asset.bytes));
            let mut found_plugin = false;

            if self.global_panes.pane(PaneId::Plugin(plugin_id)).is_some() {
                self.global_panes.handle_plugin_bytes(
                    plugin_id,
                    client_id,
                    vte_bytes.take().unwrap(),
                );
                found_plugin = true;
            } else {
                for tab in self.tabs.values_mut() {
                    if tab.has_plugin(plugin_id) {
                        tab.handle_plugin_bytes(plugin_id, client_id, vte_bytes.take().unwrap())
                            .context("failed to process plugin bytes")?;
                        found_plugin = true;
                        break;
                    }
                }
            }
            if found_plugin {
                handled_render_assets.push(plugin_render_asset);
            } else {
                plugin_render_asset.bytes = vte_bytes.take().unwrap();
                unhandled_render_assets.push(plugin_render_asset);
            }
            self.render_blocker.remove_blocking_plugin(plugin_id);
        }
        if unblock_cli_pipes {
            // CLI pipes must be unblocked for all assets - including orphans whose bytes stay
            // pending for a future pane - otherwise whatever is piping into them hangs until (or
            // forever if) their pane shows up. Replays of such pending events pass false here,
            // since their pipes were already unblocked when the bytes first arrived.
            let mut all_render_assets = handled_render_assets;
            all_render_assets.extend(unhandled_render_assets.iter().cloned());
            self.render((!all_render_assets.is_empty()).then_some(all_render_assets))?;
        } else if !handled_render_assets.is_empty() {
            // schedule a render without unblocking - passing the assets to render would
            // unblock pipes that were already unblocked when the bytes first arrived
            self.render(None)?;
        }
        Ok(unhandled_render_assets)
    }
    fn show_plugin_cursor(
        &mut self,
        plugin_id: u32,
        client_id: ClientId,
        cursor_position: Option<(usize, usize)>,
    ) -> bool {
        if let Some(plugin_pane) = self.global_panes.pane_mut(PaneId::Plugin(plugin_id)) {
            plugin_pane.show_cursor(client_id, cursor_position);
            return true;
        }
        for tab in self.tabs.values_mut() {
            if tab.has_plugin(plugin_id) {
                tab.show_plugin_cursor(plugin_id, client_id, cursor_position);
                return true;
            }
        }
        false
    }
    fn update_plugin_loading_stage(
        &mut self,
        pid: u32,
        loading_indication: LoadingIndication,
    ) -> bool {
        if let Some(plugin_pane) = self.global_panes.pane_mut(PaneId::Plugin(pid)) {
            plugin_pane.update_loading_indication(loading_indication);
            return true;
        }
        let all_tabs = self.get_tabs_mut();
        let mut found_plugin = false;
        for tab in all_tabs.values_mut() {
            if tab.has_plugin(pid) {
                found_plugin = true;
                tab.update_plugin_loading_stage(pid, loading_indication);
                break;
            }
        }
        found_plugin
    }
    fn start_plugin_loading_indication(
        &mut self,
        plugin_id: u32,
        loading_indication: LoadingIndication,
    ) -> bool {
        if let Some(plugin_pane) = self.global_panes.pane_mut(PaneId::Plugin(plugin_id)) {
            plugin_pane.start_loading_indication(loading_indication);
            return true;
        }
        for tab in self.tabs.values_mut() {
            if tab.has_plugin(plugin_id) {
                tab.start_plugin_loading_indication(plugin_id, loading_indication);
                return true;
            }
        }
        false
    }
    fn progress_plugin_loading_offset(&mut self, plugin_id: u32) -> bool {
        if let Some(plugin_pane) = self.global_panes.pane_mut(PaneId::Plugin(plugin_id)) {
            plugin_pane.progress_animation_offset();
            return true;
        }
        for tab in self.tabs.values_mut() {
            if tab.has_plugin(plugin_id) {
                tab.progress_plugin_loading_offset(plugin_id);
                return true;
            }
        }
        false
    }
    fn connected_clients_contains(&self, client_id: &ClientId) -> bool {
        self.connected_clients.borrow().contains_key(client_id)
    }
    fn get_client_pane_group(&self, client_id: &ClientId) -> HashSet<PaneId> {
        self.current_pane_group
            .borrow()
            .get_client_pane_group(client_id)
    }
    fn clear_pane_group(&mut self, client_id: &ClientId) {
        self.current_pane_group
            .borrow_mut()
            .clear_pane_group(client_id);
        self.currently_marking_pane_group
            .borrow_mut()
            .remove(client_id);
    }
    fn toggle_pane_id_in_group(&mut self, pane_id: PaneId, client_id: &ClientId) {
        let surface_size = self.size_of_tab_of_client(*client_id);
        {
            let mut pane_groups = self.current_pane_group.borrow_mut();
            pane_groups.toggle_pane_id_in_group(pane_id, surface_size, client_id);
        }
        self.retain_only_existing_panes_in_pane_groups();
    }
    fn add_pane_id_to_group(&mut self, pane_id: PaneId, client_id: &ClientId) {
        let surface_size = self.size_of_tab_of_client(*client_id);
        {
            let mut pane_groups = self.current_pane_group.borrow_mut();
            pane_groups.add_pane_id_to_group(pane_id, surface_size, client_id);
        }
        self.retain_only_existing_panes_in_pane_groups();
    }
    fn add_active_pane_to_group_if_marking(&mut self, client_id: &ClientId) {
        {
            if self
                .currently_marking_pane_group
                .borrow()
                .get(client_id)
                .copied()
                .unwrap_or(false)
            {
                let active_pane_id = self.get_active_pane_id(&client_id);
                if let Some(active_pane_id) = active_pane_id {
                    self.add_pane_id_to_group(active_pane_id, &client_id);
                }
            }
        }
        self.retain_only_existing_panes_in_pane_groups();
    }
    fn get_active_pane_id(&self, client_id: &ClientId) -> Option<PaneId> {
        self.active_pane_id(*client_id)
    }

    fn get_pane_info(&self, pane_id: PaneId) -> Option<PaneInfo> {
        // Search through all tabs to find the pane
        for tab in self.tabs.values() {
            if let Some(pane_info) = tab.get_pane_info(pane_id) {
                return Some(pane_info);
            }
        }
        None
    }

    fn get_tab_info(&self, tab_id: usize) -> Option<TabInfo> {
        // Look up tab by its stable ID
        self.tabs.get(&tab_id).map(|tab| {
            let all_focused_clients: Vec<ClientId> = self
                .active_tab_ids
                .iter()
                .filter(|(_c_id, active_tab_id)| **active_tab_id == tab.id)
                .map(|(c_id, _)| c_id)
                .copied()
                .collect();
            let (active_swap_layout_name, is_swap_layout_dirty) = tab.swap_layout_info();
            let tab_viewport = tab.get_viewport();
            let tab_display_area = tab.get_display_area();
            let selectable_tiled_panes_count = tab.get_selectable_tiled_panes_count();
            let selectable_floating_panes_count = tab.get_selectable_floating_panes_count();

            TabInfo {
                position: tab.position,
                name: tab.name.clone(),
                active: self.active_tab_ids.values().any(|i| i == &tab.id),
                panes_to_hide: tab.panes_to_hide_count(),
                is_fullscreen_active: tab.is_fullscreen_active(),
                is_sync_panes_active: tab.is_sync_panes_active(),
                are_floating_panes_visible: tab.are_floating_panes_visible(),
                other_focused_clients: all_focused_clients,
                active_swap_layout_name,
                is_swap_layout_dirty,
                viewport_rows: tab_viewport.rows,
                viewport_columns: tab_viewport.cols,
                display_area_rows: tab_display_area.rows,
                display_area_columns: tab_display_area.cols,
                selectable_tiled_panes_count,
                selectable_floating_panes_count,
                tab_id: tab.id,
                has_bell_notification: tab.tab_has_pending_bell
                    && !self.active_tab_ids.values().any(|i| i == &tab.id),
                is_flashing_bell: tab.tab_bell_flash
                    && !self.active_tab_ids.values().any(|i| i == &tab.id),
            }
        })
    }

    fn group_and_ungroup_panes(
        &mut self,
        pane_ids_to_group: Vec<PaneId>,
        pane_ids_to_ungroup: Vec<PaneId>,
        for_all_clients: bool,
        client_id: ClientId,
    ) {
        let surface_size = self.size_of_tab_of_client(client_id);
        if for_all_clients {
            {
                let mut current_pane_group = self.current_pane_group.borrow_mut();
                current_pane_group.group_and_ungroup_panes_for_all_clients(
                    pane_ids_to_group,
                    pane_ids_to_ungroup,
                    surface_size,
                );
            }
        } else {
            {
                let mut current_pane_group = self.current_pane_group.borrow_mut();
                current_pane_group.group_and_ungroup_panes(
                    pane_ids_to_group,
                    pane_ids_to_ungroup,
                    surface_size,
                    &client_id,
                );
            }
        }
        self.retain_only_existing_panes_in_pane_groups();
        let _ = self.log_and_report_session_state();
    }
    fn retain_only_existing_panes_in_pane_groups(&mut self) {
        let clients_with_empty_group = {
            let mut clients_with_empty_group = vec![];
            let mut current_pane_group = { self.current_pane_group.borrow().clone_inner() };
            for (client_id, panes_in_group) in current_pane_group.iter_mut() {
                let all_tabs = self.get_tabs();
                panes_in_group.retain(|p_id| {
                    let mut found = false;
                    for tab in all_tabs.values() {
                        if tab.has_pane_with_pid(&p_id) {
                            found = true;
                            break;
                        }
                    }
                    found
                });
                if panes_in_group.is_empty() {
                    clients_with_empty_group.push(*client_id)
                }
            }
            self.current_pane_group
                .borrow_mut()
                .override_groups_with(current_pane_group);
            clients_with_empty_group
        };
        for client_id in &clients_with_empty_group {
            self.currently_marking_pane_group
                .borrow_mut()
                .remove(client_id);
        }
        if !clients_with_empty_group.is_empty() {
            {
                let all_tabs = self.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    let _ = tab.update_input_modes();
                }
            }
            let _ = self.update_global_input_modes(clients_with_empty_group.iter().copied());
        }
    }
    fn update_active_pane_ids(&mut self) {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().keys().copied().collect();
        for client_id in connected_clients {
            if let Some(active_pane_id) = self.get_active_pane_id(&client_id) {
                let active_pane_id: PaneId = active_pane_id;
                let history = self.pane_history.entry(client_id).or_insert_with(|| vec![]);
                history.retain(|e| e != &active_pane_id);
                history.push(active_pane_id);
            }
        }
    }
}

#[cfg(not(test))]
fn get_default_editor() -> Option<PathBuf> {
    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map(|e| PathBuf::from(e))
        .ok()
}

#[cfg(test)]
fn get_default_editor() -> Option<PathBuf> {
    None
}

fn find_already_running_panes(
    tiled_layout: &TiledPaneLayout,
    floating_layouts: &[FloatingPaneLayout],
    active_tab: &Tab,
) -> (Vec<Option<Run>>, Vec<usize>) {
    let mut layout_tiled_instructions = tiled_layout.extract_run_instructions();
    let running_tiled_instructions: Vec<Option<Run>> = active_tab
        .get_tiled_panes()
        .map(|(_, pane)| pane.invoked_with().clone())
        .chain(
            active_tab
                .suppressed_stack_list_members()
                .map(|(_, pane)| pane.invoked_with().clone()),
        )
        .collect();

    let mut tiled_to_ignore = Vec::new();
    for running_instr in running_tiled_instructions {
        if let Some(pos) = layout_tiled_instructions
            .iter()
            .position(|layout_instr| layout_instr == &running_instr)
        {
            layout_tiled_instructions.remove(pos);
            tiled_to_ignore.push(running_instr);
        }
    }

    let mut running_floating_instructions: Vec<Option<Run>> = active_tab
        .get_floating_panes()
        .map(|(_, pane)| pane.invoked_with().clone())
        .collect();

    let mut floating_indices = Vec::new();
    for (idx, floating_layout) in floating_layouts.iter().enumerate() {
        if let Some(pos) = running_floating_instructions
            .iter()
            .position(|instr| instr == &floating_layout.run)
        {
            running_floating_instructions.remove(pos);
            floating_indices.push(idx);
        }
    }

    (tiled_to_ignore, floating_indices)
}

fn replay_pending_plugin_events(
    screen: &mut Screen,
    pending_events: Vec<ScreenInstruction>,
) -> Result<()> {
    for event in pending_events {
        match event {
            ScreenInstruction::PluginBytes(plugin_render_assets) => {
                // pipes were already unblocked when these bytes first arrived
                for unhandled in screen.handle_plugin_render_assets(plugin_render_assets, false)? {
                    log::error!(
                        "Could not replay pending plugin bytes for missing plugin {}",
                        unhandled.plugin_id
                    );
                }
            },
            ScreenInstruction::ShowPluginCursor(plugin_id, client_id, cursor_position) => {
                if !screen.show_plugin_cursor(plugin_id, client_id, cursor_position) {
                    log::error!(
                        "Could not replay pending plugin cursor for missing plugin {plugin_id}"
                    );
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePluginLoadingStage(plugin_id, loading_indication) => {
                if !screen.update_plugin_loading_stage(plugin_id, loading_indication) {
                    log::error!(
                        "Could not replay pending loading stage for missing plugin {plugin_id}"
                    );
                }
                screen.render(None)?;
            },
            ScreenInstruction::StartPluginLoadingIndication(plugin_id, loading_indication) => {
                if !screen.start_plugin_loading_indication(plugin_id, loading_indication) {
                    log::error!(
                        "Could not replay pending loading indication for missing plugin {plugin_id}"
                    );
                }
                screen.render(None)?;
            },
            ScreenInstruction::ProgressPluginLoadingOffset(plugin_id) => {
                if !screen.progress_plugin_loading_offset(plugin_id) {
                    log::error!(
                        "Could not replay pending loading offset for missing plugin {plugin_id}"
                    );
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClosePane(pane_id, _, _, exit_status) => {
                let removed_global_pane = screen.close_global_pane(pane_id, None)?;
                if removed_global_pane {
                    screen.render(None)?;
                } else {
                    let mut found = false;
                    for tab in screen.tabs.values_mut() {
                        if tab.get_all_pane_ids().contains(&pane_id) {
                            tab.close_pane(pane_id, false, exit_status);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        log::error!(
                            "Could not replay pending close for missing plugin pane {pane_id:?}"
                        );
                    }
                }
                screen.clear_nested_guest(pane_id);
                if !removed_global_pane {
                    screen
                        .bus
                        .senders
                        .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
                        .non_fatal();
                }
                screen.log_and_report_session_state().non_fatal();
                screen.retain_only_existing_panes_in_pane_groups();
            },
            ScreenInstruction::HoldPane(pane_id, exit_status, run_command) => {
                let is_first_run = false;
                let mut found = false;
                if let Some(pane) = screen.global_panes.pane_mut(pane_id) {
                    pane.hold(exit_status, is_first_run, run_command.clone());
                    found = true;
                } else {
                    for tab in screen.tabs.values_mut() {
                        if tab.get_all_pane_ids().contains(&pane_id) {
                            tab.hold_pane(pane_id, exit_status, is_first_run, run_command.clone());
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    log::error!(
                        "Could not replay pending hold for missing plugin pane {pane_id:?}"
                    );
                }
                screen.log_and_report_session_state()?;
            },
            unexpected => {
                log::error!(
                    "Ignoring unexpected pending plugin event during inline replay: {unexpected:?}"
                );
            },
        }
    }
    Ok(())
}

// The box is here in order to make the
// NewClient enum smaller
#[allow(clippy::boxed_local)]
pub(crate) fn screen_thread_main(
    bus: Bus<ScreenInstruction>,
    max_panes: Option<usize>,
    client_attributes: ClientAttributes,
    config: Config,
    debug: bool,
    default_layout: Box<Layout>,
) -> Result<()> {
    // Resolve `theme_dark` / `theme_light` to concrete `Styling` from the
    // bundled themes BEFORE `config.options` is moved out below. These
    // populate Screen's auto-switch state at startup; runtime updates
    // continue to flow through `propagate_configuration_changes`.
    let host_theme_dark_styling = config
        .options
        .theme_dark
        .as_ref()
        .and_then(|name| config.themes.get_theme(name).map(|t| t.palette));
    let host_theme_light_styling = config
        .options
        .theme_light
        .as_ref()
        .and_then(|name| config.themes.get_theme(name).map(|t| t.palette));
    if config.options.theme_dark.is_some() && host_theme_dark_styling.is_none() {
        log::warn!(
            "theme_dark='{}' not found in themes; auto-theme switch disabled for dark.",
            config.options.theme_dark.as_deref().unwrap_or("?")
        );
    }
    if config.options.theme_light.is_some() && host_theme_light_styling.is_none() {
        log::warn!(
            "theme_light='{}' not found in themes; auto-theme switch disabled for light.",
            config.options.theme_light.as_deref().unwrap_or("?")
        );
    }

    let config_options = config.options;
    let arrow_fonts = !config_options.simplified_ui.unwrap_or_default();
    let pane_frame_style = PaneFrameStyle::from_options(&config_options);
    let auto_layout = config_options.auto_layout.unwrap_or(true);
    let session_serialization = config_options.session_serialization.unwrap_or(true);
    let serialize_pane_viewport = config_options.serialize_pane_viewport.unwrap_or(false);
    let scrollback_lines_to_serialize = config_options.scrollback_lines_to_serialize;
    let session_is_mirrored = config_options.mirror_session.unwrap_or(false);
    let notification_position = config_options.notification_position.unwrap_or_default();
    let notification_timeout_secs = config_options
        .notification_timeout
        .unwrap_or(DEFAULT_NOTIFICATION_TIMEOUT_SECS);
    let layout_dir = config_options.layout_dir;
    #[cfg(test)]
    let default_shell = config_options
        .default_shell
        .clone()
        .unwrap_or(PathBuf::from("/bin/sh"));
    #[cfg(not(test))]
    let default_shell = config_options
        .default_shell
        .clone()
        .unwrap_or_else(|| get_default_shell());
    let default_editor = config_options
        .scrollback_editor
        .clone()
        .or_else(|| get_default_editor());
    let default_layout_name = config_options
        .default_layout
        .map(|l| format!("{}", l.display()));
    let copy_options = CopyOptions::new(
        config_options.copy_command,
        config_options.copy_clipboard.unwrap_or_default(),
        config_options.copy_on_select.unwrap_or(true),
    );
    let web_server_ip = config_options
        .web_server_ip
        .unwrap_or_else(|| IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    let web_server_port = config_options.web_server_port.unwrap_or(8082);
    let styled_underlines = config_options.styled_underlines.unwrap_or(true);
    let osc8_hyperlinks = config_options.osc8_hyperlinks.unwrap_or(true);
    let explicitly_disable_kitty_keyboard_protocol = config_options
        .support_kitty_keyboard_protocol
        .map(|e| !e) // this is due to the config options wording, if
        // "support_kitty_keyboard_protocol" is true,
        // explicitly_disable_kitty_keyboard_protocol is false and vice versa
        .unwrap_or(false); // by default, we try to support this if the terminal supports it and
                           // the program running inside a pane requests it
    let support_kitty_graphics_protocol = config_options
        .support_kitty_graphics_protocol
        .unwrap_or(true);
    let stacked_resize = config_options.stacked_resize.unwrap_or(true);
    let stacked_pane_list = config_options.stacked_pane_list.unwrap_or(true);
    let web_clients_allowed = config_options
        .web_sharing
        .map(|s| s.web_clients_allowed())
        .unwrap_or(false);
    let web_sharing = config_options.web_sharing.unwrap_or_else(Default::default);
    let advanced_mouse_actions = config_options.advanced_mouse_actions.unwrap_or(true);
    let osc133_command_selection = config_options.osc133_command_selection.unwrap_or(true);
    let word_separators = config_options
        .word_separators
        .clone()
        .unwrap_or_else(|| DEFAULT_WORD_SEPARATORS.to_owned());
    let mouse_scroll_resize = config_options.mouse_scroll_resize.unwrap_or(true);
    let mouse_hover_effects = config_options.mouse_hover_effects.unwrap_or(true);
    let visual_bell = config_options.visual_bell.unwrap_or(true);
    let focus_follows_mouse = config_options.focus_follows_mouse.unwrap_or(false);
    let mouse_click_through = config_options.mouse_click_through.unwrap_or(false);
    let nested_session_handling = config_options.nested_session_handling.unwrap_or_default();
    let dangerously_enable_paste_buffer_read = config_options
        .dangerously_enable_paste_buffer_read
        .unwrap_or(false);

    let thread_senders = bus.senders.clone();
    let mut screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        get_mode_info(
            config_options.default_mode.unwrap_or_default(),
            &client_attributes,
            PluginCapabilities {
                //  ¯\_(ツ)_/¯
                arrow_fonts: !arrow_fonts,
            },
            &config.keybinds,
            config_options.default_mode,
        ),
        pane_frame_style,
        auto_layout,
        session_is_mirrored,
        copy_options,
        debug,
        default_layout,
        default_layout_name,
        default_shell,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        styled_underlines,
        osc8_hyperlinks,
        arrow_fonts,
        layout_dir,
        explicitly_disable_kitty_keyboard_protocol,
        support_kitty_graphics_protocol,
        stacked_resize,
        stacked_pane_list,
        default_editor,
        web_clients_allowed,
        web_sharing,
        advanced_mouse_actions,
        osc133_command_selection,
        word_separators,
        mouse_scroll_resize,
        mouse_hover_effects,
        visual_bell,
        focus_follows_mouse,
        mouse_click_through,
        web_server_ip,
        web_server_port,
        nested_session_handling,
        notification_position,
        notification_timeout_secs,
    );
    screen.host_theme_dark_styling = host_theme_dark_styling;
    screen.host_theme_light_styling = host_theme_light_styling;
    screen.paste_buffer_read_enabled = dangerously_enable_paste_buffer_read;

    let mut pending_tab_ids: HashSet<usize> = HashSet::new();
    let mut pending_tab_switches: HashSet<(usize, ClientId)> = HashSet::new(); // usize is the
                                                                               // tab_index
    let mut pending_events_waiting_for_tab: Vec<ScreenInstruction> = vec![];
    let mut pending_events_waiting_for_client: Vec<ScreenInstruction> = vec![];
    let mut pending_events_waiting_for_pane: HashMap<PaneId, Vec<ScreenInstruction>> =
        HashMap::new();
    let mut keybind_intercepts = HashMap::new();
    loop {
        let (event, mut err_ctx) = screen
            .bus
            .recv()
            .context("failed to receive event on channel")?;
        err_ctx.add_call(ContextType::Screen((&event).into()));
        // here we start caching resizes, so that we'll send them in bulk at the end of each event
        // when this cache is Dropped, for more information, see the comments in PtyWriter
        let _resize_cache = ResizeCache::new(thread_senders.clone());

        match event {
            ScreenInstruction::PtyBytes(pid, vte_bytes) => {
                screen
                    .pane_output_activity
                    .insert(PaneId::Terminal(pid), Instant::now());
                let mut vte_bytes = Some(vte_bytes);
                if screen.global_panes.pane(PaneId::Terminal(pid)).is_some() {
                    screen
                        .global_panes
                        .handle_pty_bytes(pid, vte_bytes.take().unwrap());
                } else {
                    let all_tabs = screen.get_tabs_mut();
                    for tab in all_tabs.values_mut() {
                        if tab.has_terminal_pid(pid) {
                            if let Some(bytes) = vte_bytes.take() {
                                tab.handle_pty_bytes(pid, bytes)
                                    .context("failed to process pty bytes")?;
                            }
                            break;
                        }
                    }
                }
                if let Some(vte_bytes) = vte_bytes {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Terminal(pid))
                        .or_default()
                        .push(ScreenInstruction::PtyBytes(pid, vte_bytes));
                }
                let _ = screen
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::RenderToClients);
            },
            ScreenInstruction::PluginBytes(plugin_render_assets) => {
                for plugin_render_asset in
                    screen.handle_plugin_render_assets(plugin_render_assets, true)?
                {
                    let plugin_id = plugin_render_asset.plugin_id;
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(plugin_id))
                        .or_default()
                        .push(ScreenInstruction::PluginBytes(vec![plugin_render_asset]));
                }
            },
            ScreenInstruction::Render => {
                screen.render(None)?;
            },
            ScreenInstruction::RenderToClients => {
                // render_blocker.can_render() returning true means that either all pending plugins
                // (only those waiting for a new tab layout to be applied!) have been rendered or
                // that a 100ms timeout has been reached (more info in the RenderBlocker comment)
                if screen.render_blocker.can_render() {
                    screen.render_to_clients()?;
                } else {
                    screen.render(None)?;
                }
            },
            ScreenInstruction::NewGlobalPane {
                pane_id,
                initial_pane_title,
                hold_for_command,
                invoked_with,
                placement,
                borderless,
                initially_hidden,
                should_focus,
                client_id,
                mut completion_tx,
                set_blocking,
                declaration_order,
                split,
            } => {
                let pane_id_was_available = screen.global_panes.pane(pane_id).is_none();
                let existing_split_plugin = split.and_then(|_| {
                    invoked_with.as_ref().and_then(|run| match run {
                        Run::Plugin(run_plugin) => screen.global_panes.find_plugin(run_plugin),
                        _ => None,
                    })
                });
                if let Some(existing) = existing_split_plugin {
                    pending_events_waiting_for_pane.remove(&pane_id);
                    screen.show_pane(existing)?;
                    let client_id = client_id
                        .filter(|client_id| screen.active_tab_ids.contains_key(client_id))
                        .or_else(|| screen.get_first_client_id());
                    if let Some(client_id) = client_id.filter(|_| should_focus) {
                        screen.focus_global_pane(existing, client_id)?;
                    }
                    if let Some(completion) = completion_tx.as_mut() {
                        completion.set_affected_pane_id(existing);
                    }
                    if pane_id_was_available {
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
                            .non_fatal();
                    }
                    screen.log_and_report_session_state()?;
                    screen.render(None)?;
                    continue;
                }
                let mut inserted = false;
                let creation_result: Result<()> = (|| {
                    let blocking_notification = if set_blocking {
                        completion_tx.take()
                    } else {
                        None
                    };
                    let mut pane = screen.build_global_pane_box(
                        pane_id,
                        initial_pane_title,
                        invoked_with,
                        blocking_notification,
                    )?;
                    pane.set_borderless(borderless);
                    if let Some(run_command) = hold_for_command {
                        pane.hold(None, true, run_command);
                    }
                    // a split anchor that closed while the pane was spawning falls back to a
                    // plain edge insert with the carried placement
                    let live_anchor = split.filter(|split| {
                        !initially_hidden && screen.global_panes.pane(split.anchor).is_some()
                    });
                    match live_anchor {
                        Some(GlobalSplitAnchor { anchor, before }) => screen
                            .global_panes
                            .insert_split(pane, anchor, before, placement.length)
                            .map_err(|error| anyhow!(error))?,
                        None => screen
                            .global_panes
                            .insert_with_order(pane, placement, initially_hidden, declaration_order)
                            .map_err(|error| anyhow!(error))?,
                    }
                    inserted = true;
                    if should_focus && !initially_hidden {
                        let client_id = client_id
                            .filter(|client_id| screen.active_tab_ids.contains_key(client_id))
                            .or_else(|| screen.get_first_client_id());
                        match client_id {
                            Some(client_id) => {
                                screen.focus_global_pane(pane_id, client_id)?;
                            },
                            // a layout global pane can spawn before the first client finishes
                            // attaching; park the focus for AddClient rather than dropping it
                            None => {
                                screen.pending_global_pane_focus = Some(pane_id);
                            },
                        }
                    }
                    screen.reflow_global_panes()
                })();
                if let Err(error) = creation_result {
                    pending_events_waiting_for_pane.remove(&pane_id);
                    if inserted {
                        screen.global_panes.remove(pane_id);
                        if !screen.global_panes.visible_pane_ids().is_empty() {
                            screen.reflow_global_panes().non_fatal();
                        }
                    }
                    if let Some(completion) = completion_tx.as_mut() {
                        completion.set_exit_status(1);
                        completion.set_error_message(format!(
                            "failed to create global pane {pane_id:?}: {error:#}"
                        ));
                    }
                    if pane_id_was_available {
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
                            .non_fatal();
                    }
                    log::error!("Failed to create global pane {pane_id:?}: {error:#}");
                    continue;
                }
                if let Some(completion) = completion_tx.as_mut() {
                    completion.set_affected_pane_id(pane_id);
                }
                if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pane_id) {
                    if matches!(pane_id, PaneId::Plugin(_)) {
                        replay_pending_plugin_events(&mut screen, pending_events)?;
                    } else {
                        for event in pending_events {
                            screen.bus.senders.send_to_screen(event).non_fatal();
                        }
                    }
                }
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::LaunchOrFocusGlobalPaneSplit {
                run_plugin,
                anchor,
                before,
                length,
                should_focus,
                skip_cache,
                client_id,
            } => {
                if let Some(existing) = screen.global_panes.find_plugin(&run_plugin) {
                    // adopt: the plugin already has a global pane (possibly hidden after a
                    // restart or a strip-wide hide) - reveal its strip rather than split again
                    screen.show_pane(existing)?;
                    if should_focus && screen.active_tab_ids.contains_key(&client_id) {
                        screen.focus_global_pane(existing, client_id)?;
                    }
                    screen.log_and_report_session_state()?;
                    screen.render(None)?;
                } else if screen.global_panes.placement(anchor).is_some() {
                    let tab_index = screen
                        .active_tab_ids
                        .get(&client_id)
                        .copied()
                        .or_else(|| screen.active_tab_ids.values().next().copied())
                        .or_else(|| screen.tabs.keys().next().copied());
                    match tab_index {
                        Some(tab_index) => {
                            // placement details are derived from the anchor at insertion time;
                            // this placement only carries the requested length and a sane
                            // fallback should the anchor close mid-flight
                            let anchor_placement = screen
                                .global_panes
                                .placement(anchor)
                                .expect("checked above");
                            let placement = GlobalPanePlacement {
                                edge: anchor_placement.edge,
                                size: anchor_placement.size,
                                length,
                            };
                            let context = GlobalPaneSpawnContext {
                                placement,
                                borderless: false,
                                initially_hidden: false,
                                should_focus,
                                declaration_order: None,
                                split: Some(GlobalSplitAnchor { anchor, before }),
                            };
                            screen
                                .bus
                                .senders
                                .send_to_pty(PtyInstruction::FillPluginCwd(
                                    Some(false),
                                    false,
                                    false,
                                    None,
                                    run_plugin,
                                    tab_index,
                                    None,
                                    client_id,
                                    Size::default(),
                                    skip_cache,
                                    None,
                                    None,
                                    None,
                                    None,
                                    Some(context),
                                    None,
                                ))?;
                        },
                        None => {
                            log::error!("no tab found to launch a global pane split into");
                        },
                    }
                } else {
                    log::error!(
                        "cannot split global pane {anchor:?}: the caller is not a global pane"
                    );
                }
            },
            ScreenInstruction::NewPane(
                pid,
                initial_pane_title,
                hold_for_command,
                invoked_with,
                new_pane_placement,
                start_suppressed,
                client_or_tab_index,
                mut completion_tx,
                set_blocking,
            ) => {
                completion_tx.as_mut().map(|c| c.set_affected_pane_id(pid));

                let blocking_notification = if set_blocking { completion_tx } else { None };

                match client_or_tab_index {
                    ClientTabIndexOrPaneId::ClientId(client_id)
                    | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                        let should_focus_pane =
                            matches!(client_or_tab_index, ClientTabIndexOrPaneId::ClientId(_));
                        // An ordinary new pane belongs to a tab, never to the global
                        // area: the global strips are session chrome, and a client
                        // happening to have focus in one is not a request to grow it.
                        // The pane is about to land in a tab and take the focus with
                        // it, and a client left on a global pane would be looking
                        // elsewhere when it did - so release that focus first. A no-op
                        // unless a global pane is focused.
                        if should_focus_pane {
                            screen.defocus_global_pane(client_id);
                        }
                        active_tab_and_connected_client_id_with_first_tab_fallback!(screen, client_id, |tab: &mut Tab, client_id: Option<ClientId>| {
                            tab.new_pane(pid,
                               initial_pane_title,
                               invoked_with,
                               start_suppressed,
                               should_focus_pane,
                               new_pane_placement,
                               client_id,
                               blocking_notification
                           )
                        }, ?);
                        if let Some(hold_for_command) = hold_for_command {
                            let is_first_run = true;
                            active_tab_and_connected_client_id_with_first_tab_fallback!(
                                screen,
                                client_id,
                                |tab: &mut Tab, _client_id: Option<ClientId>| tab.hold_pane(
                                    pid,
                                    None,
                                    is_first_run,
                                    hold_for_command
                                )
                            )
                        }
                        if should_focus_pane {
                            screen.sync_scroll_mode_on_focus(client_id)?;
                        }
                    },
                    ClientTabIndexOrPaneId::TabIndex(tab_index)
                    | ClientTabIndexOrPaneId::TabIndexNoFocus(tab_index) => {
                        let should_focus_pane =
                            matches!(client_or_tab_index, ClientTabIndexOrPaneId::TabIndex(_));
                        // Some placements (directional split, stacked without a
                        // target pane) need a client_id to know which pane to
                        // split relative to. Only resolve one when required.
                        let needs_client_id = matches!(
                            new_pane_placement,
                            NewPanePlacement::Tiled {
                                direction: Some(_),
                                ..
                            } | NewPanePlacement::Stacked {
                                pane_id_to_stack_under: None,
                                ..
                            }
                        );
                        let client_id = if needs_client_id {
                            screen
                                .active_tab_ids
                                .iter()
                                .find(|(_, tid)| **tid == tab_index)
                                .map(|(cid, _)| *cid)
                                .or_else(|| screen.active_tab_ids.keys().next().copied())
                        } else {
                            None
                        };
                        if let Some(active_tab) = screen.tabs.get_mut(&tab_index) {
                            active_tab.new_pane(
                                pid,
                                initial_pane_title,
                                invoked_with,
                                start_suppressed,
                                should_focus_pane,
                                new_pane_placement,
                                client_id,
                                blocking_notification,
                            )?;
                            if let Some(hold_for_command) = hold_for_command {
                                let is_first_run = true;
                                active_tab.hold_pane(pid, None, is_first_run, hold_for_command);
                            }
                        } else {
                            log::error!("Tab index not found: {:?}", tab_index);
                        }
                    },
                    ClientTabIndexOrPaneId::PaneId(pane_id) => {
                        // A global pane is in no tab, so it can anchor nothing: the scan
                        // below would find no owning tab and drop the pane on the floor.
                        // The anchor is the only part of the request that cannot be
                        // honoured, so give up the anchor rather than the pane - it lands
                        // in a tab, unanchored, instead of nowhere.
                        let anchor_is_global = screen.global_panes.pane(pane_id).is_some();
                        let new_pane_placement = if anchor_is_global {
                            NewPanePlacement::NoPreference { borderless: None }
                        } else {
                            new_pane_placement
                        };
                        let tab_without_anchor = if anchor_is_global {
                            screen
                                .active_tab_ids
                                .values()
                                .next()
                                .copied()
                                .or_else(|| screen.tabs.keys().next().copied())
                        } else {
                            None
                        };
                        let mut found = false;
                        let all_tabs = screen.get_tabs_mut();
                        let should_focus_pane = false;
                        for (tab_index, tab) in all_tabs.iter_mut() {
                            let is_target = match tab_without_anchor {
                                Some(target) => *tab_index == target,
                                None => tab.has_pane_with_pid(&pane_id),
                            };
                            if is_target {
                                match new_pane_placement {
                                    NewPanePlacement::Tiled {
                                        direction: Some(direction),
                                        borderless,
                                    } => {
                                        if direction == Direction::Left
                                            || direction == Direction::Right
                                        {
                                            tab.vertical_split_of_pane_id(
                                                pid,
                                                initial_pane_title,
                                                invoked_with,
                                                pane_id,
                                                blocking_notification,
                                                borderless,
                                            )?;
                                        } else {
                                            tab.horizontal_split_of_pane_id(
                                                pid,
                                                initial_pane_title,
                                                invoked_with,
                                                pane_id,
                                                blocking_notification,
                                                borderless,
                                            )?;
                                        }
                                    },
                                    _ => {
                                        tab.new_pane(
                                            pid,
                                            initial_pane_title,
                                            invoked_with,
                                            start_suppressed,
                                            should_focus_pane,
                                            new_pane_placement,
                                            None,
                                            blocking_notification,
                                        )?;
                                    },
                                }
                                if let Some(hold_for_command) = hold_for_command {
                                    let is_first_run = true;
                                    tab.hold_pane(pid, None, is_first_run, hold_for_command);
                                }
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            if anchor_is_global {
                                log::error!("no tab found to open a pane in");
                            } else {
                                log::error!(
                                    "Failed to find tab containing pane with id: {:?}",
                                    pane_id
                                );
                            }
                        }
                    },
                };
                screen.activate_overlay(pid);
                if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pid) {
                    for event in pending_events {
                        screen.bus.senders.send_to_screen(event).non_fatal();
                    }
                }
                screen.log_and_report_session_state()?;

                screen.render(None)?;
            },
            ScreenInstruction::OpenInPlaceEditor(pid, client_tab_index_or_pane_id) => {
                match client_tab_index_or_pane_id {
                    ClientTabIndexOrPaneId::ClientId(client_id)
                    | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                        active_tab!(screen, client_id, |tab: &mut Tab| tab
                            .replace_active_pane_with_editor_pane(pid, client_id), ?);
                        screen.log_and_report_session_state()?;
                    },
                    ClientTabIndexOrPaneId::TabIndex(_tab_index)
                    | ClientTabIndexOrPaneId::TabIndexNoFocus(_tab_index) => {
                        log::error!("Cannot OpenInPlaceEditor with a TabIndex");
                    },
                    ClientTabIndexOrPaneId::PaneId(pane_id_to_replace) => {
                        let mut found = false;
                        let all_tabs = screen.get_tabs_mut();
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id_to_replace) {
                                tab.replace_pane_with_editor_pane(pid, pane_id_to_replace)
                                    .non_fatal();
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            log::error!(
                                "Could not find pane with id {:?} to replace",
                                pane_id_to_replace
                            );
                        }
                    },
                }

                if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pid) {
                    for event in pending_events {
                        screen.bus.senders.send_to_screen(event).non_fatal();
                    }
                }

                screen.render(None)?;
            },
            ScreenInstruction::TogglePaneEmbedOrFloating(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_pane_embed_or_floating(client_id), ?);
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::ToggleFloatingPanes(client_id, default_shell, completion_tx) => {
                active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_floating_panes(Some(client_id), default_shell, completion_tx), ?);
                screen.clear_bell_for_focused_pane(client_id);
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.log_and_report_session_state()?;

                screen.render(None)?;
            },
            ScreenInstruction::ShowFloatingPanes {
                client_id,
                tab_id,
                completion,
            } => {
                screen.show_floating_panes_in_tab(client_id, tab_id, completion)?;
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::HideFloatingPanes {
                client_id,
                tab_id,
                completion,
            } => {
                screen.hide_floating_panes_in_tab(client_id, tab_id, completion)?;
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::AreFloatingPanesVisible {
                client_id,
                tab_id,
                completion,
            } => {
                screen.are_floating_panes_visible_in_tab(client_id, tab_id, completion)?;
            },
            ScreenInstruction::WriteCharacter(
                key_with_modifier,
                raw_bytes,
                is_kitty_keyboard_protocol,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                if let Some(plugin_id) = keybind_intercepts.get(&client_id) {
                    if let Some(key_with_modifier) = key_with_modifier {
                        let _ = screen
                            .bus
                            .senders
                            .send_to_plugin(PluginInstruction::Update(vec![(
                                Some(*plugin_id),
                                Some(client_id),
                                Event::InterceptedKeyPress(key_with_modifier),
                            )]));
                        continue;
                    }
                }
                let mut state_changed = false;
                let client_input_mode = screen.get_client_input_mode(client_id);
                if let Some(global_pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    match client_input_mode {
                        Some(InputMode::RenamePane) => {
                            if !(raw_bytes == BRACKETED_PASTE_BEGIN
                                || raw_bytes == BRACKETED_PASTE_END)
                            {
                                screen.with_pane(global_pane_id, |pane| pane.rename(raw_bytes));
                                state_changed = true;
                            }
                        },
                        _ => {
                            state_changed = screen
                                .write_to_pane_id(
                                    &key_with_modifier,
                                    raw_bytes,
                                    is_kitty_keyboard_protocol,
                                    global_pane_id,
                                    Some(client_id),
                                    None,
                                )
                                .unwrap_or(false);
                        },
                    }
                    if state_changed {
                        screen.log_and_report_session_state()?;
                    }
                    screen.render(None)?;
                    continue;
                }
                match client_input_mode {
                    Some(InputMode::RenameTab) => {
                        if !(raw_bytes == BRACKETED_PASTE_BEGIN || raw_bytes == BRACKETED_PASTE_END)
                        {
                            screen.update_active_tab_name(raw_bytes, client_id)?;
                            state_changed = true;
                        }
                    },
                    _ => {
                        active_tab_and_connected_client_id!(
                            screen,
                            client_id,
                            |tab: &mut Tab, client_id: ClientId| {
                                match client_input_mode {
                                    Some(InputMode::EnterSearch) => {
                                        if !(raw_bytes == BRACKETED_PASTE_BEGIN
                                            || raw_bytes == BRACKETED_PASTE_END)
                                        {
                                            if let Err(e) =
                                                tab.update_search_term(raw_bytes, client_id)
                                            {
                                                log::error!("{}", e);
                                            }
                                        }
                                        state_changed = true;
                                    },
                                    Some(InputMode::RenamePane) => {
                                        if !(raw_bytes == BRACKETED_PASTE_BEGIN
                                            || raw_bytes == BRACKETED_PASTE_END)
                                        {
                                            if let Err(e) =
                                                tab.update_active_pane_name(raw_bytes, client_id)
                                            {
                                                log::error!("{}", e);
                                            }
                                            state_changed = true;
                                        }
                                    },
                                    _ => {
                                        let write_result = match tab.is_sync_panes_active() {
                                            true => tab.write_to_terminals_on_current_tab(
                                                &key_with_modifier,
                                                raw_bytes,
                                                is_kitty_keyboard_protocol,
                                                client_id,
                                            ),
                                            false => tab.write_to_active_terminal(
                                                &key_with_modifier,
                                                raw_bytes,
                                                is_kitty_keyboard_protocol,
                                                client_id,
                                            ),
                                        };
                                        if let Ok(true) = write_result {
                                            state_changed = true;
                                        }
                                    },
                                }
                            }
                        );
                    },
                };
                if state_changed {
                    screen.log_and_report_session_state()?;
                }
                screen.render(None)?;
            },
            ScreenInstruction::Resize(
                client_id,
                strategy,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                if let Some(global_pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.resize_pane_with_id(strategy, global_pane_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                if screen.resize_global_pane_from_adjacent_tab_pane(client_id, strategy)? {
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.resize(client_id, strategy),
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SwitchFocus(client_id, _completion_tx) => {
                let old_pane_id = screen.get_active_pane_id(&client_id);
                screen.cycle_focus(client_id, true);
                let new_pane_id = screen.get_active_pane_id(&client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    screen.report_key_passthrough_state(client_id, old, new);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::FocusNextPane(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to change focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to change focus for".to_string());
                    }
                } else {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.cycle_focus(client_id, true);
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        screen.report_key_passthrough_state(client_id, old, new);
                    }
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                }
            },
            ScreenInstruction::FocusPreviousPane(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to change focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to change focus for".to_string());
                    }
                } else {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.cycle_focus(client_id, false);
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        screen.report_key_passthrough_state(client_id, old, new);
                    }
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::FocusLastPane(client_id, _completion_tx) => {
                let old_pane_id = screen.get_active_pane_id(&client_id);
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.focus_last_pane(client_id)
                );
                let new_pane_id = screen.get_active_pane_id(&client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    screen.report_key_passthrough_state(client_id, old, new);
                }
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MoveFocusLeft(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    screen.move_focus_in_direction_or_escalate(Direction::Left, client_id)?;
                }
            },
            ScreenInstruction::MoveFocusLeftOrPreviousTab(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.move_focus_left_or_previous_tab(client_id)?;
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        if old != new {
                            screen.report_key_passthrough_state_with_direction(
                                client_id,
                                old,
                                new,
                                Some(Direction::Left),
                            );
                        }
                    }
                    screen.clear_bell_for_focused_pane(client_id);
                    screen.add_active_pane_to_group_if_marking(&client_id);
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::MoveFocusDown(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    screen.move_focus_in_direction_or_escalate(Direction::Down, client_id)?;
                }
            },
            ScreenInstruction::MoveFocusRight(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    screen.move_focus_in_direction_or_escalate(Direction::Right, client_id)?;
                }
            },
            ScreenInstruction::MoveFocusRightOrNextTab(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.move_focus_right_or_next_tab(client_id)?;
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        if old != new {
                            screen.report_key_passthrough_state_with_direction(
                                client_id,
                                old,
                                new,
                                Some(Direction::Right),
                            );
                        }
                    }
                    screen.clear_bell_for_focused_pane(client_id);
                    screen.add_active_pane_to_group_if_marking(&client_id);
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::MoveFocusUp(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    screen.move_focus_in_direction_or_escalate(Direction::Up, client_id)?;
                }
            },
            ScreenInstruction::ClearScreen(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| pane.clear_screen());
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.clear_active_terminal_screen(
                        client_id,
                    ),
                    ?
                );
                screen.render(None)?;
            },
            ScreenInstruction::DumpScreen(
                file,
                client_id,
                full,
                pane_id,
                completion_tx,
                cli_client_id,
                ansi,
            ) => {
                let global_pane_id = pane_id
                    .filter(|pane_id| screen.global_panes.pane(*pane_id).is_some())
                    .or_else(|| {
                        if pane_id.is_none() {
                            screen.global_panes.focused_pane_id(client_id)
                        } else {
                            None
                        }
                    });
                if let Some(global_pane_id) = global_pane_id {
                    let dump = screen
                        .with_pane(global_pane_id, |pane| {
                            if ansi {
                                pane.dump_screen_with_ansi(full, Some(client_id))
                            } else {
                                pane.dump_screen(full, Some(client_id))
                            }
                        })
                        .unwrap_or_default();
                    if let Some(file_path) = file {
                        if let Some(os_input) = screen.bus.os_input.as_mut() {
                            os_input.write_to_file(dump, Some(file_path))?;
                        }
                        drop(completion_tx);
                    } else {
                        screen.bus.senders.send_to_server(ServerInstruction::Log(
                            vec![dump],
                            cli_client_id.unwrap_or(client_id),
                            completion_tx,
                        ))?;
                    }
                    continue;
                }
                match file {
                    Some(file_path) => {
                        // Write dump to file (existing behavior)
                        match pane_id {
                            Some(pane_id) => {
                                for tab in screen.get_tabs_mut().values_mut() {
                                    if tab.has_pane_with_pid(&pane_id) {
                                        if ansi {
                                            tab.dump_with_ansi_terminal_screen(
                                                Some(file_path.clone()),
                                                pane_id,
                                                full,
                                            )?;
                                        } else {
                                            tab.dump_terminal_screen(
                                                Some(file_path.clone()),
                                                pane_id,
                                                full,
                                            )?;
                                        }
                                        break;
                                    }
                                }
                            },
                            None => {
                                if ansi {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| tab.dump_with_ansi_active_terminal_screen(
                                            Some(file_path.to_string()),
                                            client_id,
                                            full
                                        ),
                                        ?
                                    );
                                } else {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| tab.dump_active_terminal_screen(
                                            Some(file_path.to_string()),
                                            client_id,
                                            full
                                        ),
                                        ?
                                    );
                                }
                            },
                        }
                        screen.render(None)?;
                        drop(completion_tx);
                    },
                    None => {
                        // Dump to STDOUT via Log
                        let dump = match pane_id {
                            Some(pane_id) => {
                                let mut result = String::new();
                                for tab in screen.get_tabs_mut().values_mut() {
                                    if tab.has_pane_with_pid(&pane_id) {
                                        if ansi {
                                            if let Some(dump) = tab
                                                .get_dump_with_ansi_terminal_screen(pane_id, full)
                                            {
                                                result = dump;
                                            }
                                        } else {
                                            if let Some(dump) =
                                                tab.get_dump_terminal_screen(pane_id, full)
                                            {
                                                result = dump;
                                            }
                                        }
                                        break;
                                    }
                                }
                                result
                            },
                            None => {
                                let mut result = String::new();
                                if ansi {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| {
                                            result = tab.get_dump_with_ansi_active_terminal_screen(client_id, full);
                                            Ok::<(), anyhow::Error>(())
                                        },
                                        ?
                                    );
                                } else {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| {
                                            result = tab.get_dump_active_terminal_screen(client_id, full);
                                            Ok::<(), anyhow::Error>(())
                                        },
                                        ?
                                    );
                                }
                                result
                            },
                        };
                        screen.bus.senders.send_to_server(ServerInstruction::Log(
                            vec![dump],
                            cli_client_id.unwrap_or(client_id),
                            completion_tx,
                        ))?;
                    },
                }
            },
            ScreenInstruction::DumpLayout(default_shell, client_id, completion_tx) => {
                let err_context = || "Failed to dump layout".to_string();
                let session_layout_metadata = screen.get_layout_metadata(default_shell, None);
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::DumpLayout(
                        session_layout_metadata,
                        client_id,
                        completion_tx,
                    ))
                    .with_context(err_context)?;
            },
            ScreenInstruction::ListClientsMetadata(default_shell, client_id, completion_tx) => {
                let err_context = || "Failed to dump layout".to_string();
                let session_layout_metadata = screen.get_layout_metadata(default_shell, None);
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::ListClientsMetadata(
                        session_layout_metadata,
                        client_id,
                        completion_tx,
                    ))
                    .with_context(err_context)?;
            },
            ScreenInstruction::ListPanes {
                show_all,
                response_channel,
            } => {
                let err_context = || "Failed to list panes";
                let pane_entries = screen
                    .collect_pane_list(show_all)
                    .with_context(err_context)?;
                let _ = response_channel.send(pane_entries);
            },
            ScreenInstruction::ListNotifications { response_channel } => {
                let _ = response_channel.send(screen.notification_history.clone());
            },
            ScreenInstruction::ListTabs {
                client_id,
                response_channel,
            } => {
                let err_context = || "Failed to list tabs";
                let tab_infos = screen
                    .collect_tab_list(client_id)
                    .with_context(err_context)?;
                let _ = response_channel.send(tab_infos);
            },
            ScreenInstruction::GetCurrentTabInfo {
                client_id,
                response_channel,
            } => {
                let err_context = || "Failed to get current tab info";
                let tab_info = screen
                    .get_current_tab_info(client_id)
                    .with_context(err_context)?;
                let _ = response_channel.send(tab_info);
            },
            ScreenInstruction::DumpLayoutToPlugin {
                plugin_id,
                tab_index,
                response_channel,
            } => {
                let err_context = || "Failed to dump layout".to_string();
                let session_layout_metadata =
                    screen.get_layout_metadata(Some(screen.default_shell.clone()), tab_index);
                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::DumpLayoutToPlugin {
                        session_layout_metadata,
                        plugin_id,
                        response_channel,
                    })
                    .with_context(err_context)
                    .non_fatal();
            },
            ScreenInstruction::GetFocusedPaneInfo {
                client_id,
                response_channel,
            } => {
                let response = match screen.active_tab_ids.get(&client_id) {
                    Some(&focused_tab_index) => match screen.get_active_pane_id(&client_id) {
                        Some(focused_pane_id) => GetFocusedPaneInfoResponse::Ok {
                            tab_index: focused_tab_index,
                            pane_id: focused_pane_id.into(),
                        },
                        None => GetFocusedPaneInfoResponse::Err(format!(
                            "No active pane found for client {:?}",
                            client_id
                        )),
                    },
                    None => GetFocusedPaneInfoResponse::Err(format!(
                        "Client {:?} not found in active_tab_indices",
                        client_id
                    )),
                };
                let _ = response_channel.send(response);
            },
            ScreenInstruction::GetPaneInfo {
                pane_id,
                response_channel,
            } => {
                let pane_info = screen.get_pane_info(pane_id);
                let _ = response_channel.send(pane_info);
            },
            ScreenInstruction::GetTabInfo {
                tab_id,
                response_channel,
            } => {
                let tab_info = screen.get_tab_info(tab_id);
                let _ = response_channel.send(tab_info);
            },
            ScreenInstruction::ListClientsToPlugin(plugin_id, client_id) => {
                let err_context = || "Failed to dump layout".to_string();
                let session_layout_metadata =
                    screen.get_layout_metadata(Some(screen.default_shell.clone()), None);
                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::ListClientsToPlugin(
                        session_layout_metadata,
                        plugin_id,
                        client_id,
                    ))
                    .with_context(err_context)
                    .non_fatal();
            },
            ScreenInstruction::EditScrollback(client_id, ansi, completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| {
                        if ansi {
                            tab.edit_scrollback_raw(client_id, completion_tx)
                        } else {
                            tab.edit_scrollback(client_id, completion_tx)
                        }
                    },
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::GetPaneScrollback {
                pane_id,
                client_id,
                get_full_scrollback,
                response_channel,
            } => {
                let last_output_ms_ago = screen
                    .pane_output_activity
                    .get(&pane_id)
                    .map(|last| Instant::now().saturating_duration_since(*last).as_millis() as u64);
                let pane_contents = screen.with_pane(pane_id, |pane| {
                    pane.pane_contents(Some(client_id), get_full_scrollback, None)
                });
                // Send response back through channel
                let response = match pane_contents {
                    Some(mut contents) => {
                        contents.last_output_ms_ago = last_output_ms_ago;
                        PaneScrollbackResponse::Ok(contents)
                    },
                    None => {
                        log::warn!(
                            "Plugin requested scrollback for pane {:?} but pane was not found",
                            pane_id
                        );
                        PaneScrollbackResponse::Err(format!("Pane {:?} not found", pane_id))
                    },
                };
                if response_channel.send(response).is_err() {
                    // the plugin likely timed out and dropped the receiver
                    log::debug!(
                        "Plugin timed out before pane scrollback response was sent for pane {:?}",
                        pane_id
                    );
                }
            },
            ScreenInstruction::ScrollUp(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| pane.scroll_up(1, client_id));
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.scroll_active_terminal_up(client_id)
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::MovePane(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneBackwards(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_backwards(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneDown(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_down(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneUp(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_up(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneRight(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_right(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneLeft(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_left(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ScrollUpAt(
                point,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .handle_scrollwheel_up(&point, 3, client_id), ?
                );
                screen.render(None)?;
            },
            ScreenInstruction::ScrollDown(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| pane.scroll_down(1, client_id));
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.scroll_active_terminal_down(client_id), ?
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollDownAt(
                point,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .handle_scrollwheel_down(&point, 3, client_id), ?
                );
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToBottom(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| pane.clear_scroll());
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_to_bottom(client_id), ?
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToTop(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| {
                        pane.clear_scroll();
                        if let Some(lines) = pane.get_line_number() {
                            pane.scroll_up(lines, client_id);
                        }
                    });
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_to_top(client_id), ?
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollUp(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_up(pane.rows().max(1).saturating_sub(1), client_id)
                    });
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_up_page(client_id)
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollDown(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_down(pane.get_content_rows(), client_id)
                    });
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_down_page(client_id), ?
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::HalfPageScrollUp(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_up(pane.rows().max(1).saturating_sub(1) / 2, client_id)
                    });
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_up_half_page(client_id)
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::HalfPageScrollDown(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_down(pane.rows().max(1).saturating_sub(1) / 2, client_id)
                    });
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_down_half_page(client_id), ?
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::ClearScroll(client_id) => {
                let was_scrolled = screen.active_pane_is_scrolled(client_id);
                if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.with_pane(pane_id, |pane| pane.clear_scroll());
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render(None)?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .clear_active_terminal_scroll(client_id), ?
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::CloseFocusedPane(client_id, completion_tx) => {
                let old_pane_id = screen.get_active_pane_id(&client_id);
                if let Some(global_pane_id) = screen.global_panes.focused_pane_id(client_id) {
                    screen.close_global_pane(global_pane_id, completion_tx)?;
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.close_focused_pane(client_id, completion_tx), ?
                );
                let new_pane_id = screen.get_active_pane_id(&client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    screen.report_key_passthrough_state(client_id, old, new);
                }
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetSelectable(pid, selectable) => {
                let all_tabs = screen.get_tabs_mut();
                let mut found_plugin = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pid) {
                        tab.set_pane_selectable(pid, selectable);
                        found_plugin = true;
                        break;
                    }
                }
                if !found_plugin {
                    pending_events_waiting_for_tab
                        .push(ScreenInstruction::SetSelectable(pid, selectable));
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ShowPluginCursor(pid, client_id, cursor_position) => {
                if !screen.show_plugin_cursor(pid, client_id, cursor_position) {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::ShowPluginCursor(
                            pid,
                            client_id,
                            cursor_position,
                        ));
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetMouseSelectionSupport(pid, selection_support) => {
                // `with_pane` looks in the global panes before the tabs - a global pane belongs to
                // no tab, so a tab-only search would park this in `pending_events_waiting_for_tab`
                // forever and the plugin's opt-in would never arrive
                if screen
                    .with_pane(pid, |pane| {
                        pane.set_mouse_selection_support(selection_support)
                    })
                    .is_none()
                {
                    pending_events_waiting_for_tab.push(
                        ScreenInstruction::SetMouseSelectionSupport(pid, selection_support),
                    );
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ClosePane(
                id,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                // waiting for it
                exit_status,
            ) => {
                let global_focus_to_restore = screen.take_global_focus_after_overlay_close(id);
                let removed_global_pane = screen.close_global_pane(id, None)?;
                if removed_global_pane {
                    screen.render(None)?;
                } else {
                    match client_id {
                        Some(client_id) => {
                            active_tab!(screen, client_id, |tab: &mut Tab| tab.close_pane(
                                id,
                                false,
                                exit_status
                            ));
                        },
                        None => {
                            let mut found = false;
                            for tab in screen.tabs.values_mut() {
                                if tab.get_all_pane_ids().contains(&id) {
                                    tab.close_pane(id, false, exit_status);
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                pending_events_waiting_for_pane.entry(id).or_default().push(
                                    ScreenInstruction::ClosePane(id, None, None, exit_status),
                                );
                            }
                        },
                    }
                }

                screen.clear_nested_guest(id);
                screen.pane_output_activity.remove(&id);

                // Clean up PTY-side resources (async reader task, child PID mapping,
                // terminal_id_to_raw_fd entry). This is needed because the natural
                // child exit path (quit_cb) only sends ScreenInstruction::ClosePane
                // and never sends PtyInstruction::ClosePane. The handler in Pty is
                // idempotent, so this is safe even if ClosePane was already sent.
                if !removed_global_pane {
                    let _ = screen
                        .bus
                        .senders
                        .send_to_pty(PtyInstruction::ClosePane(id, None));
                }
                screen.restore_global_focus(global_focus_to_restore);

                let connected_client_ids: Vec<ClientId> =
                    screen.active_tab_ids.keys().copied().collect();
                for connected_client_id in connected_client_ids {
                    screen.sync_scroll_mode_on_focus(connected_client_id)?;
                }
                screen.log_and_report_session_state().non_fatal();
                screen.retain_only_existing_panes_in_pane_groups();
            },
            ScreenInstruction::HoldPane(id, exit_status, run_command) => {
                let is_first_run = false;
                let mut found = false;
                if let Some(pane) = screen.global_panes.pane_mut(id) {
                    pane.hold(exit_status, is_first_run, run_command.clone());
                    found = true;
                } else {
                    for tab in screen.tabs.values_mut() {
                        if tab.get_all_pane_ids().contains(&id) {
                            tab.hold_pane(id, exit_status, is_first_run, run_command.clone());
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    pending_events_waiting_for_pane
                        .entry(id)
                        .or_default()
                        .push(ScreenInstruction::HoldPane(id, exit_status, run_command));
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePaneName(
                c,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.update_active_pane_name(c, client_id), ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePaneHost(terminal_id, host) => {
                // A pane's host never changes while it lives, so this arrives
                // once when it is created and once more when it is closed.
                for tab in screen.tabs.values_mut() {
                    if let Some(pane) = tab.get_pane_with_id_mut(PaneId::Terminal(terminal_id)) {
                        pane.set_remote_host(host.clone());
                        break;
                    }
                }
                match host {
                    Some(host) => screen.terminal_hosts.insert(terminal_id, host),
                    None => screen.terminal_hosts.remove(&terminal_id),
                };
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePaneCwd(terminal_id, cwd) => {
                // Only pty says a pane moved, and it only says so on a change,
                // so there is nothing to compare against here. Pruning cannot be
                // done from the manifest instead: pty reports the folder a pane
                // starts in *before* the layout that puts the pane in a tab, so
                // an unknown id is as likely to be arriving as departing.
                match cwd {
                    Some(cwd) => screen.terminal_cwds.insert(terminal_id, cwd),
                    None => screen.terminal_cwds.remove(&terminal_id),
                };
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UndoRenamePane(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.undo_active_rename_pane(client_id), ?
                );
                screen.render(None)?;
            },
            ScreenInstruction::ToggleActiveTerminalFullscreen(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .toggle_active_pane_fullscreen(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .toggle_active_pane_no_ui_fullscreen(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::TogglePaneFrames(
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.pane_frame_style = match screen.pane_frame_style {
                    PaneFrameStyle::Full => PaneFrameStyle::Titles,
                    PaneFrameStyle::Titles => PaneFrameStyle::None,
                    PaneFrameStyle::None => PaneFrameStyle::Full,
                };
                screen
                    .global_panes
                    .set_pane_frame_style(screen.pane_frame_style)
                    .map_err(|error| anyhow!(error))?;
                for tab in screen.tabs.values_mut() {
                    tab.set_pane_frames(screen.pane_frame_style);
                    tab.update_input_modes()?;
                }
                screen.update_global_input_modes(screen.active_tab_ids.keys().copied())?;
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetPaneFrameStyle(pane_frame_style, _completion_tx) => {
                screen.pane_frame_style = pane_frame_style;
                screen
                    .global_panes
                    .set_pane_frame_style(screen.pane_frame_style)
                    .map_err(|error| anyhow!(error))?;
                for tab in screen.tabs.values_mut() {
                    tab.set_pane_frames(screen.pane_frame_style);
                    tab.update_input_modes()?;
                }
                screen.update_global_input_modes(screen.active_tab_ids.keys().copied())?;
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SwitchTabNext(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.switch_tab_next(None, true, client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::SwitchTabPrev(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.switch_tab_prev(None, true, client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::CloseTab(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.close_tab(client_id)?;
                let connected_client_ids: Vec<ClientId> =
                    screen.active_tab_ids.keys().copied().collect();
                for connected_client_id in connected_client_ids {
                    screen.sync_scroll_mode_on_focus(connected_client_id)?;
                }
                screen.render(None)?;
            },
            ScreenInstruction::NewTab(
                cwd,
                default_shell,
                layout,
                floating_panes_layout,
                tab_name,
                (swap_tiled_layouts, swap_floating_layouts),
                initial_panes,
                block_on_first_terminal,
                should_change_focus_to_new_tab,
                (client_id, is_web_client),
                completion_tx,
            ) => {
                let tab_index = screen.get_new_tab_id();
                screen.spawn_layout_global_panes_once(client_id, tab_index)?;
                pending_tab_ids.insert(tab_index);
                let client_id_for_new_tab = if should_change_focus_to_new_tab {
                    Some(client_id)
                } else {
                    None
                };
                // `is_web_client` may be a placeholder when the NewTab action was
                // initiated over the web control channel (which cannot carry the
                // flag); resolve it from the actual connected-client status.
                let is_web_client = is_web_client || screen.client_is_web(client_id);
                let resolved_swap_layouts = (
                    swap_tiled_layouts
                        .unwrap_or_else(|| screen.default_layout.swap_tiled_layouts.clone()),
                    swap_floating_layouts
                        .unwrap_or_else(|| screen.default_layout.swap_floating_layouts.clone()),
                );
                screen.new_tab(
                    tab_index,
                    resolved_swap_layouts,
                    tab_name.clone(),
                    client_id_for_new_tab,
                )?;
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::NewTab(
                        cwd,
                        default_shell,
                        layout,
                        floating_panes_layout,
                        tab_index,
                        initial_panes,
                        block_on_first_terminal,
                        should_change_focus_to_new_tab,
                        (client_id, is_web_client),
                        completion_tx,
                    ))?;
            },
            ScreenInstruction::ApplyLayout(
                layout,
                floating_panes_layout,
                new_pane_pids,
                new_floating_pane_pids,
                new_plugin_ids,
                tab_id,
                should_change_focus_to_new_tab,
                (client_id, is_web_client),
                mut completion_tx,
                blocking_terminal,
            ) => {
                log::info!(
                    "ScreenInstruction::ApplyLayout: applying layout for tab {}",
                    tab_id
                );
                // tab_id is a stable identifier from NewTab instruction
                if let Some(first_terminal_pane) = new_pane_pids.first() {
                    completion_tx
                        .as_mut()
                        .map(|c| c.set_affected_pane_id(PaneId::Terminal(first_terminal_pane.0)));
                } else if let Some(plugin_id) =
                    new_plugin_ids.values().next().and_then(|v| v.first())
                {
                    completion_tx
                        .as_mut()
                        .map(|c| c.set_affected_pane_id(PaneId::Plugin(*plugin_id)));
                }
                // Set the affected tab ID for plugin API return value
                completion_tx
                    .as_mut()
                    .map(|c| c.set_affected_tab_id(tab_id));
                screen.apply_layout(
                    layout,
                    floating_panes_layout,
                    new_pane_pids.clone(),
                    new_floating_pane_pids,
                    new_plugin_ids.clone(),
                    tab_id,
                    should_change_focus_to_new_tab,
                    (client_id, is_web_client),
                    blocking_terminal,
                )?;
                pending_tab_ids.remove(&tab_id);
                if pending_tab_ids.is_empty() {
                    for (tab_index, client_id) in pending_tab_switches.drain() {
                        screen.go_to_tab(tab_index + 1, client_id)?;
                    }
                    if should_change_focus_to_new_tab {
                        // Convert ID → position for go_to_tab (which expects 1-based position)
                        if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
                            screen.go_to_tab(tab_position + 1, client_id)?;
                        }
                    }
                } else if should_change_focus_to_new_tab {
                    let client_id_to_switch = if screen.active_tab_ids.contains_key(&client_id) {
                        Some(client_id)
                    } else {
                        screen.active_tab_ids.keys().next().copied()
                    };
                    if let Some(client_id_to_switch) = client_id_to_switch {
                        // Convert ID → position for pending_tab_switches (which stores positions)
                        if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
                            pending_tab_switches.insert((tab_position, client_id_to_switch));
                        }
                    }
                }

                for plugin_ids in new_plugin_ids.values() {
                    for plugin_id in plugin_ids {
                        let pane_id = PaneId::Plugin(*plugin_id);
                        screen.render_blocker.register_blocking_plugin(*plugin_id);
                        let plugin_exists = screen.global_panes.pane(pane_id).is_some()
                            || screen.tabs.values().any(|tab| tab.has_plugin(*plugin_id));
                        if plugin_exists {
                            if let Some(pending_events) =
                                pending_events_waiting_for_pane.remove(&pane_id)
                            {
                                replay_pending_plugin_events(&mut screen, pending_events)?;
                            }
                        }
                    }
                }

                for event in pending_events_waiting_for_client.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                for event in pending_events_waiting_for_tab.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                screen.render(None)?;
            },
            ScreenInstruction::GoToTab(
                tab_index,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let client_id_to_switch = if client_id.is_none() {
                    None
                } else if screen
                    .active_tab_ids
                    .contains_key(&client_id.expect("This is checked above"))
                {
                    client_id
                } else {
                    screen.active_tab_ids.keys().next().copied()
                };
                match client_id_to_switch {
                    // we must make sure pending_tab_ids is empty because otherwise we cannot be
                    // sure this instruction is applied at the right time (eg. we might have a
                    // pending tab that will become not-pending after this instruction and change
                    // the client focus, which should have happened before this instruction and not
                    // after)
                    Some(client_id) if pending_tab_ids.is_empty() => {
                        screen.go_to_tab(tab_index as usize, client_id)?;
                        screen.render(None)?;
                    },
                    _ => {
                        if let Some(client_id) = client_id {
                            pending_tab_switches.insert((tab_index as usize, client_id));
                        }
                    },
                }
            },
            ScreenInstruction::GoToTabName(
                tab_name,
                default_shell,
                create,
                client_id,
                mut completion_tx,
            ) => {
                let swap_layouts = (
                    screen.default_layout.swap_tiled_layouts.clone(),
                    screen.default_layout.swap_floating_layouts.clone(),
                );
                let client_id = if client_id.is_none() {
                    None
                } else if screen
                    .active_tab_ids
                    .contains_key(&client_id.expect("This is checked above"))
                {
                    client_id
                } else {
                    screen.active_tab_ids.keys().next().copied()
                };
                if let Some(client_id) = client_id {
                    let is_web_client = screen
                        .connected_clients
                        .borrow()
                        .get(&client_id)
                        .copied()
                        .unwrap_or(false);
                    if let Ok(tab_exists) = screen.go_to_tab_name(tab_name.clone(), client_id) {
                        screen.render(None)?;
                        if tab_exists {
                            // Tab already exists - find its ID and set in completion
                            if let Some(existing_tab) =
                                screen.tabs.values().find(|t| t.name == tab_name)
                            {
                                completion_tx
                                    .as_mut()
                                    .map(|c| c.set_affected_tab_id(existing_tab.id));
                            }
                        }
                        if create && !tab_exists {
                            let tab_index = screen.get_new_tab_id();
                            let should_change_focus_to_new_tab = true;
                            screen.new_tab(
                                tab_index,
                                swap_layouts,
                                Some(tab_name),
                                Some(client_id),
                            )?;
                            screen
                                .bus
                                .senders
                                .send_to_plugin(PluginInstruction::NewTab(
                                    None,
                                    default_shell,
                                    None,
                                    vec![],
                                    tab_index,
                                    None,  // initial_panes
                                    false, // block_on_first_terminal
                                    should_change_focus_to_new_tab,
                                    (client_id, is_web_client),
                                    completion_tx,
                                ))?;
                            continue; // so we don't get to the completion signalling below
                        }
                    }
                }
            },
            ScreenInstruction::UpdateTabName(
                c,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.update_active_tab_name(c, client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::UndoRenameTab(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.undo_active_rename_tab(client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::MoveTabLeft(client_id, completion_tx) => {
                if pending_tab_ids.is_empty() {
                    screen.move_active_tab_to_left(client_id)?;
                    screen.render(None)?;
                } else {
                    // Defer execution, forward completion_tx
                    pending_events_waiting_for_tab
                        .push(ScreenInstruction::MoveTabLeft(client_id, completion_tx));
                }
            },
            ScreenInstruction::MoveTabRight(client_id, completion_tx) => {
                if pending_tab_ids.is_empty() {
                    screen.move_active_tab_to_right(client_id)?;
                    screen.render(None)?;
                } else {
                    // Defer execution, forward completion_tx
                    pending_events_waiting_for_tab
                        .push(ScreenInstruction::MoveTabRight(client_id, completion_tx));
                }
            },
            ScreenInstruction::RecomputeTabSize(client_id, new_size) => {
                let previous_surface = screen.surface_size();
                screen.set_client_size(client_id, new_size);
                // the server sends this while setting the session up, before any tab exists
                if screen.tabs.is_empty() {
                    continue;
                }
                // resize_to_clients reflows the global panes and every tab against the new
                // surface size, then reports and renders - which is what render_to_clients would
                // otherwise undo, since it lays the tabs out against the surface on every frame
                screen.resize_to_clients()?;
                if screen.surface_size() == previous_surface {
                    // another, smaller client is pinning the shared surface size, so the layout
                    // does not change - but this client's terminal did, and it needs the
                    // unchanged layout painted over its new viewport
                    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
                        tab.set_force_render();
                    }
                    screen.global_panes.set_force_render();
                    screen.render(None)?;
                }
            },
            ScreenInstruction::TerminalPixelDimensions(client_id, pixel_dimensions) => {
                screen.update_pixel_dimensions(client_id, pixel_dimensions);
            },
            ScreenInstruction::TerminalBackgroundColor(background_color_instruction) => {
                screen.update_terminal_background_color(background_color_instruction);
            },
            ScreenInstruction::TerminalForegroundColor(background_color_instruction) => {
                screen.update_terminal_foreground_color(background_color_instruction);
            },
            ScreenInstruction::TerminalColorRegisters(color_registers) => {
                screen.update_terminal_color_registers(color_registers);
            },
            ScreenInstruction::SetKittyGraphicsSupport {
                client_id,
                supported,
            } => {
                screen.update_kitty_graphics_support(client_id, supported);
            },
            ScreenInstruction::SetSixelSupport {
                client_id,
                supported,
            } => {
                screen.update_sixel_support(client_id, supported);
            },
            ScreenInstruction::ForwardHostQuery { pane_id, query } => {
                screen.forward_host_query(pane_id, query);
            },
            ScreenInstruction::NestedSessionMessageFromPane { pane_id, message } => {
                screen.handle_nested_session_message_from_pane(pane_id, message);
            },
            ScreenInstruction::NestedGuestPingTick { pane_id } => {
                screen.handle_nested_guest_ping_tick(pane_id);
            },
            ScreenInstruction::NestedSessionMessageFromHost { client_id, message } => {
                screen.handle_nested_session_message_from_host(client_id, message);
            },
            ScreenInstruction::GuestModalChoice {
                client_id,
                pane_id,
                outcome,
            } => {
                screen.handle_guest_modal_choice(client_id, pane_id, outcome);
            },
            ScreenInstruction::ForwardedReplyFromHost { token, reply_bytes } => {
                screen.handle_forwarded_reply_from_host(token, reply_bytes)?;
                // The handler's replay of `pending_pty_input` mutates the
                // grid. Without a render scheduling here the clients
                // keep displaying the pre-reply frame
                screen.render(None)?;
            },
            ScreenInstruction::ResumePaneAfterForward {
                pane_id,
                reply_bytes,
            } => {
                screen.resume_pane_after_forward(pane_id, reply_bytes)?;
                // Same rationale as ForwardedReplyFromHost above — the
                // resume's replay paints the grid, so we must schedule
                // a render before yielding back to the screen loop.
                screen.render(None)?;
            },
            ScreenInstruction::HostTerminalThemeChanged(mode) => {
                screen.update_host_terminal_theme_mode(mode)?;
            },
            ScreenInstruction::SetDarkTheme(mut completion_tx) => {
                screen.apply_manual_host_terminal_theme_mode(
                    HostTerminalThemeMode::Dark,
                    &mut completion_tx,
                )?;
            },
            ScreenInstruction::SetLightTheme(mut completion_tx) => {
                screen.apply_manual_host_terminal_theme_mode(
                    HostTerminalThemeMode::Light,
                    &mut completion_tx,
                )?;
            },
            ScreenInstruction::ToggleTheme(mut completion_tx) => {
                let next = match screen.host_terminal_theme_mode {
                    Some(HostTerminalThemeMode::Dark) => HostTerminalThemeMode::Light,
                    Some(HostTerminalThemeMode::Light) => HostTerminalThemeMode::Dark,
                    // No prior mode: treat as Dark and toggle to Light.
                    None => HostTerminalThemeMode::Light,
                };
                screen.apply_manual_host_terminal_theme_mode(next, &mut completion_tx)?;
            },
            ScreenInstruction::ChangeMode(
                input_mode,
                base_mode,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.change_mode(input_mode, base_mode, client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ChangeModeForAllClients(
                input_mode,
                base_mode,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.change_mode_for_all_clients(input_mode, base_mode)?;
                screen.render(None)?;
            },
            ScreenInstruction::ToggleActiveSyncTab(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, _client_id: ClientId| tab.toggle_sync_panes_is_active()
                );
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::MouseEvent(
                event,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.handle_mouse_event(event, client_id);
            },
            ScreenInstruction::Copy(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.copy_selection(client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::Exit => {
                break;
            },
            ScreenInstruction::ToggleTab(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.toggle_tab(client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::AddClient(
                client_id,
                is_web_client,
                client_size,
                tab_position_to_focus,
                pane_id_to_focus,
            ) => {
                screen.set_client_size(client_id, client_size);
                // the new client can shrink the shared surface, so every tab is reflowed - but
                // only the reflow: this handler reports and renders on its own way out.
                // Reject a client whose size leaves nothing to lay out: keeping its size in the
                // shared minimum would make every later render fail for all clients.
                let attach_result = screen
                    .add_client(client_id, is_web_client)
                    .and_then(|_| screen.reflow_global_panes());
                if let Err(error) = attach_result {
                    Err::<(), _>(error)
                        .context("failed to attach client at its reported size")
                        .non_fatal();
                    screen.remove_client(client_id).non_fatal();
                    screen.resize_to_clients().non_fatal();
                    screen
                        .bus
                        .senders
                        .send_to_server(ServerInstruction::ClientExit(client_id, None))
                        .non_fatal();
                    continue;
                }
                let pane_id = pane_id_to_focus.map(|(pane_id, is_plugin)| {
                    if is_plugin {
                        PaneId::Plugin(pane_id)
                    } else {
                        PaneId::Terminal(pane_id)
                    }
                });
                if let Some(pane_id) = pane_id {
                    screen.focus_pane_with_id(pane_id, true, false, client_id)?;
                } else if let Some(tab_position_to_focus) = tab_position_to_focus {
                    screen.go_to_tab(tab_position_to_focus, client_id)?;
                }
                screen.show_guest_modals_for_new_client(client_id);
                let focused_pane_id = screen.get_active_pane_id(&client_id);
                if let Some(focused) = focused_pane_id {
                    screen.report_key_passthrough_state(client_id, focused, focused);
                }
                screen.sync_scroll_mode_on_focus(client_id)?;
                for event in pending_events_waiting_for_client.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }
                screen.log_and_report_session_state()?;

                screen.render(None)?;
            },
            ScreenInstruction::RemoveClient(client_id) => {
                screen.remove_client(client_id).non_fatal();
                // the departing client may have been the one constraining the screen size
                screen.resize_to_clients().non_fatal();
                screen.log_and_report_session_state().non_fatal();
                screen.render(None).non_fatal();
            },
            ScreenInstruction::UpdateSearch(
                c,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.update_search_term(c, client_id), ?
                );
                screen.render(None)?;
            },
            ScreenInstruction::SearchDown(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.search_down(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::SearchUp(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.search_up(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::SearchToggleCaseSensitivity(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .toggle_search_case_sensitivity(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::SearchToggleWrap(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.toggle_search_wrap(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::SearchToggleWholeWord(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.toggle_search_whole_words(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::AddRedPaneFrameColorOverride(pane_ids, error_text) => {
                let all_tabs = screen.get_tabs_mut();
                for pane_id in pane_ids {
                    for tab in all_tabs.values_mut() {
                        if tab.has_pane_with_pid(&pane_id) {
                            tab.add_red_pane_frame_color_override(pane_id, error_text.clone());
                            break;
                        }
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::AddHighlightPaneFrameColorOverride(pane_ids, error_text) => {
                let all_tabs = screen.get_tabs_mut();
                for pane_id in pane_ids {
                    for tab in all_tabs.values_mut() {
                        if tab.has_pane_with_pid(&pane_id) {
                            tab.add_highlight_pane_frame_color_override(
                                pane_id,
                                error_text.clone(),
                                None,
                            );
                            break;
                        }
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClearPaneFrameColorOverride(pane_ids) => {
                let all_tabs = screen.get_tabs_mut();
                for pane_id in pane_ids {
                    for tab in all_tabs.values_mut() {
                        if tab.has_pane_with_pid(&pane_id) {
                            tab.clear_pane_frame_color_override(pane_id, None);
                            break;
                        }
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::SetTabBellFlash(tab_id, is_flashing) => {
                if let Some(tab) = screen.tabs.values_mut().find(|t| t.id == tab_id) {
                    tab.tab_bell_flash = is_flashing;
                    tab.clear_tab_bell_ring();
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::PreviousSwapLayout(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, _client_id: ClientId| tab.previous_swap_layout(),
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::NextSwapLayout(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, _client_id: ClientId| tab.next_swap_layout(),
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::OverrideLayout(
                cwd,
                default_shell,
                mut tab_layouts,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_focused_tab,
                client_id,
                completion_tx,
            ) => {
                // 1. Determine which tabs to close (exist but not in layout)
                let existing_tab_indices: HashSet<usize> = screen.tabs.keys().copied().collect();
                let layout_tab_indices: HashSet<usize> =
                    tab_layouts.iter().map(|tl| tl.tab_index).collect();
                let tabs_to_close: Vec<usize> = existing_tab_indices
                    .difference(&layout_tab_indices)
                    .copied()
                    .collect();

                // 2. Process each tab layout
                let mut processed_tab_layouts = Vec::new();

                if apply_only_to_focused_tab {
                    match screen.get_active_tab_mut(client_id) {
                        Ok(active_tab) => {
                            if tab_layouts.is_empty() {
                                log::error!("No tab layouts found, cannot override.");
                                continue;
                            }
                            let mut tab_layout_info = tab_layouts.remove(0);
                            tab_layout_info.tab_index = active_tab.id;
                            // Set the tab name if provided
                            if let Some(name) = tab_layout_info.tab_name.take() {
                                active_tab.name = name;
                            }

                            // Find already-running panes for this tab
                            let (tiled_to_ignore, floating_indices) = find_already_running_panes(
                                &tab_layout_info.tiled_layout,
                                &tab_layout_info.floating_layouts,
                                active_tab,
                            );

                            // Mark run instructions to ignore (prevents re-spawning)
                            for run_instruction in tiled_to_ignore {
                                tab_layout_info
                                    .tiled_layout
                                    .ignore_run_instruction(run_instruction);
                            }

                            for idx in floating_indices {
                                tab_layout_info
                                    .floating_layouts
                                    .get_mut(idx)
                                    .map(|f| f.already_running = true);
                            }

                            processed_tab_layouts.push(tab_layout_info);
                        },
                        Err(e) => {
                            log::error!("Failed to override layout of active tab: {}", e);
                        },
                    }
                } else {
                    for mut tab_layout_info in tab_layouts {
                        // Get the tab by index
                        let tab = match screen.tabs.get_mut(&tab_layout_info.tab_index) {
                            Some(t) => t,
                            None => {
                                // no corresponding tab exists, we'll create it
                                processed_tab_layouts.push(tab_layout_info);
                                continue;
                            },
                        };

                        // Set the tab name if provided
                        if let Some(name) = tab_layout_info.tab_name.take() {
                            tab.name = name;
                        }

                        // Find already-running panes for this tab
                        let (tiled_to_ignore, floating_indices) = find_already_running_panes(
                            &tab_layout_info.tiled_layout,
                            &tab_layout_info.floating_layouts,
                            tab,
                        );

                        // Mark run instructions to ignore (prevents re-spawning)
                        for run_instruction in tiled_to_ignore {
                            tab_layout_info
                                .tiled_layout
                                .ignore_run_instruction(run_instruction);
                        }

                        for idx in floating_indices {
                            tab_layout_info
                                .floating_layouts
                                .get_mut(idx)
                                .map(|f| f.already_running = true);
                        }

                        processed_tab_layouts.push(tab_layout_info);
                    }
                }

                // 3. Send to plugin thread with all tab layouts
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::OverrideLayout(
                        cwd,
                        default_shell,
                        processed_tab_layouts,
                        retain_existing_terminal_panes,
                        retain_existing_plugin_panes,
                        client_id,
                        completion_tx,
                    ))?;

                // 4. Close tabs that aren't in the layout
                if !apply_only_to_focused_tab {
                    for tab_index in tabs_to_close {
                        screen.close_tab_by_id(tab_index)?;
                    }
                }
            },
            ScreenInstruction::OverrideLayoutComplete(
                tab_results,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
                completion_tx,
            ) => {
                // Process each tab result
                for tab_result in tab_results {
                    if let Some(tab) = screen.tabs.get_mut(&tab_result.tab_index) {
                        if let Err(e) = tab.override_layout(
                            tab_result.tiled_layout,
                            tab_result.floating_layouts,
                            tab_result.swap_tiled_layouts.clone(),
                            tab_result.swap_floating_layouts.clone(),
                            tab_result.new_terminal_pids,
                            tab_result.new_floating_pane_pids,
                            tab_result.plugin_ids,
                            retain_existing_terminal_panes,
                            retain_existing_plugin_panes,
                            client_id,
                            None,
                        ) {
                            log::error!(
                                "Failed to override layout for tab {}: {:?}",
                                tab_result.tab_index,
                                e
                            );
                        }
                    } else {
                        // Tab doesn't exist - create it
                        let swap_layouts = (
                            tab_result.swap_tiled_layouts.clone().unwrap_or_default(),
                            tab_result.swap_floating_layouts.clone().unwrap_or_default(),
                        );
                        if let Err(e) = screen.new_tab(
                            tab_result.tab_index,
                            swap_layouts,
                            tab_result.tab_name.clone(),
                            None,
                        ) {
                            log::error!(
                                "Failed to create new tab {} during override completion: {:?}",
                                tab_result.tab_index,
                                e
                            );
                            continue;
                        }

                        // Now override the newly created tab's layout
                        if let Some(tab) = screen.tabs.get_mut(&tab_result.tab_index) {
                            if let Err(e) = tab.override_layout(
                                tab_result.tiled_layout,
                                tab_result.floating_layouts,
                                tab_result.swap_tiled_layouts,
                                tab_result.swap_floating_layouts,
                                tab_result.new_terminal_pids,
                                tab_result.new_floating_pane_pids,
                                tab_result.plugin_ids,
                                retain_existing_terminal_panes,
                                retain_existing_plugin_panes,
                                client_id,
                                None,
                            ) {
                                log::error!(
                                    "Failed to override layout for new tab {}: {:?}",
                                    tab_result.tab_index,
                                    e
                                );
                            }
                        }
                    }
                }

                for event in pending_events_waiting_for_client.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                for event in pending_events_waiting_for_tab.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                // Single render and log after all tabs are updated (performance optimization)
                screen.log_and_report_session_state()?;
                let _ = screen.render(None);
                drop(completion_tx); // action ends here, notify the action initiator
            },
            ScreenInstruction::QueryTabNames(client_id, completion_tx) => {
                let tab_names = screen
                    .get_tabs_mut()
                    .values()
                    .map(|tab| tab.name.clone())
                    .collect::<Vec<String>>();
                screen.bus.senders.send_to_server(ServerInstruction::Log(
                    tab_names,
                    client_id,
                    completion_tx,
                ))?;
            },
            ScreenInstruction::NewTiledPluginPane(
                run_plugin,
                pane_title,
                skip_cache,
                cwd,
                client_id,
                completion_tx,
                explicit_tab_id,
                no_focus,
            ) => {
                let tab_index = explicit_tab_id
                    .unwrap_or_else(|| *screen.active_tab_ids.values().next().unwrap_or(&1));
                let size = Size::default();
                let should_float = Some(false);
                let should_be_opened_in_place = false;
                let should_focus_plugin = if no_focus { Some(false) } else { None };
                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::FillPluginCwd(
                        should_float,
                        should_be_opened_in_place,
                        false, // close_replaced_pane
                        pane_title,
                        run_plugin,
                        tab_index,
                        None,
                        client_id,
                        size,
                        skip_cache,
                        cwd,
                        should_focus_plugin,
                        None,
                        completion_tx,
                        None,
                        None,
                    ))?;
            },
            ScreenInstruction::NewFloatingPluginPane(
                run_plugin,
                pane_title,
                skip_cache,
                cwd,
                placement,
                client_id,
                completion_tx,
                explicit_tab_id,
                no_focus,
            ) => {
                let resolved_tab_index =
                    explicit_tab_id.or_else(|| screen.active_tab_ids.values().next().copied());
                match resolved_tab_index {
                    Some(tab_index) => {
                        let size = Size::default();
                        let should_float = Some(true);
                        let should_be_opened_in_place = false;
                        let should_focus_plugin = if no_focus { Some(false) } else { None };
                        let overlay_placement = match &placement {
                            NewPanePlacement::Overlay(overlay) => Some(overlay.clone()),
                            _ => None,
                        };
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::FillPluginCwd(
                                should_float,
                                should_be_opened_in_place,
                                false, // close_replaced_pane
                                pane_title,
                                run_plugin,
                                tab_index,
                                None,
                                client_id,
                                size,
                                skip_cache,
                                cwd,
                                should_focus_plugin,
                                placement.floating_pane_coordinates(),
                                completion_tx,
                                None,
                                overlay_placement,
                            ))?;
                    },
                    None => {
                        log::error!(
                            "Could not find an active tab - is there at least 1 connected user?"
                        );
                    },
                }
            },
            ScreenInstruction::NewInPlacePluginPane(
                run_plugin,
                pane_title,
                pane_id_to_replace,
                skip_cache,
                close_replaced_pane,
                client_id,
                completion_tx,
                explicit_tab_id,
                no_focus,
            ) => {
                let resolved_tab_index =
                    explicit_tab_id.or_else(|| screen.active_tab_ids.values().next().copied());
                match resolved_tab_index {
                    Some(tab_index) => {
                        let size = Size::default();
                        let should_float = None;
                        let should_be_in_place = true;
                        let should_focus_plugin = if no_focus { Some(false) } else { None };
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::FillPluginCwd(
                                should_float,
                                should_be_in_place,
                                close_replaced_pane,
                                pane_title,
                                run_plugin,
                                tab_index,
                                Some(pane_id_to_replace),
                                client_id,
                                size,
                                skip_cache,
                                None,
                                should_focus_plugin,
                                None,
                                completion_tx,
                                None,
                                None,
                            ))?;
                    },
                    None => {
                        log::error!(
                            "Could not find an active tab - is there at least 1 connected user?"
                        );
                    },
                }
            },
            ScreenInstruction::StartOrReloadPluginPane(run_plugin, pane_title, completion_tx) => {
                let tab_index = screen.active_tab_ids.values().next().unwrap_or(&1);
                let size = Size::default();
                let should_float = Some(false);

                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::Reload(
                        should_float,
                        pane_title,
                        run_plugin,
                        *tab_index,
                        size,
                        completion_tx,
                    ))?;
            },
            ScreenInstruction::AddPlugin(
                should_float,
                should_be_in_place,
                close_replaced_pane,
                run_plugin_or_alias,
                pane_title,
                tab_index,
                plugin_id,
                pane_id_to_replace,
                cwd,
                start_suppressed,
                floating_pane_coordinates,
                should_focus_plugin,
                client_id,
                mut completion_tx,
                stated_overlay_placement,
            ) => {
                let mut new_pane_placement = NewPanePlacement::default();
                let maybe_should_float = should_float;
                let should_float = maybe_should_float.unwrap_or(false);
                // Two spellings of "this is an overlay" meet here. A launcher that states an
                // `OverlayPlacement` outright wins; the plugin's own `overlay`/`modal`
                // configuration is the compatibility shim for everything written before
                // placements existed, and this is the one place it is read. Coordinates the
                // caller passed are kept either way - they say *where* on the surface, not
                // *whether*.
                let overlay_placement = stated_overlay_placement
                    .clone()
                    .or_else(|| run_plugin_or_alias.overlay_placement())
                    .map(|placement| OverlayPlacement {
                        coordinates: floating_pane_coordinates
                            .clone()
                            .or(placement.coordinates.clone()),
                        ..placement
                    });
                let floating_pane_coordinates = floating_pane_coordinates.or_else(|| {
                    overlay_placement
                        .as_ref()
                        .map(|placement| placement.as_floating_pane_coordinates())
                });
                let should_be_tiled = maybe_should_float.map(|f| !f).unwrap_or(false)
                    && floating_pane_coordinates.is_none();
                if let Some(overlay_placement) = overlay_placement {
                    new_pane_placement = NewPanePlacement::Overlay(overlay_placement);
                } else if floating_pane_coordinates.is_some() || should_float {
                    new_pane_placement = NewPanePlacement::with_floating_pane_coordinates(
                        floating_pane_coordinates.clone(),
                    );
                }
                if should_be_tiled {
                    new_pane_placement = NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    };
                }
                if should_be_in_place {
                    new_pane_placement = NewPanePlacement::with_pane_id_to_replace(
                        pane_id_to_replace.map(|id| id.into()),
                        close_replaced_pane,
                    );
                }
                if screen.active_tab_ids.is_empty() && tab_index.is_none() {
                    pending_events_waiting_for_client.push(ScreenInstruction::AddPlugin(
                        maybe_should_float,
                        should_be_in_place,
                        close_replaced_pane,
                        run_plugin_or_alias,
                        pane_title,
                        tab_index,
                        plugin_id,
                        pane_id_to_replace,
                        cwd,
                        start_suppressed,
                        floating_pane_coordinates,
                        should_focus_plugin,
                        client_id,
                        completion_tx,
                        stated_overlay_placement,
                    ));
                    continue;
                }
                let pane_title = pane_title.unwrap_or_else(|| {
                    format!(
                        "({}) - {}",
                        cwd.map(|cwd| cwd.display().to_string())
                            .unwrap_or(".".to_owned()),
                        run_plugin_or_alias.location_string()
                    )
                });
                let run_plugin = Run::Plugin(run_plugin_or_alias);

                // Set affected pane ID for CLI client output
                if let Some(ref mut completion) = completion_tx {
                    completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
                }

                if should_be_in_place {
                    if let Some(pane_id_to_replace) = pane_id_to_replace {
                        let client_tab_index_or_pane_id =
                            ClientTabIndexOrPaneId::PaneId(pane_id_to_replace);
                        screen.replace_pane(
                            PaneId::Plugin(plugin_id),
                            None,
                            Some(run_plugin),
                            Some(pane_title),
                            close_replaced_pane,
                            client_tab_index_or_pane_id,
                        )?;
                    } else if let Some(client_id) = client_id {
                        let client_tab_index_or_pane_id =
                            ClientTabIndexOrPaneId::ClientId(client_id);
                        screen.replace_pane(
                            PaneId::Plugin(plugin_id),
                            None,
                            Some(run_plugin),
                            Some(pane_title),
                            close_replaced_pane,
                            client_tab_index_or_pane_id,
                        )?;
                    } else {
                        log::error!("Must have pane id to replace or connected client_id if replacing a pane");
                    }
                } else if let Some(client_id) = client_id {
                    active_tab_and_connected_client_id!(screen, client_id, |active_tab: &mut Tab, _client_id: ClientId| {
                        active_tab.new_pane(
                            PaneId::Plugin(plugin_id),
                            Some(pane_title),
                            Some(run_plugin),
                            start_suppressed,
                            should_focus_plugin.unwrap_or(true),
                            new_pane_placement,
                            Some(client_id),
                            None,
                        )
                    }, ?);
                } else if let Some(active_tab) =
                    tab_index.and_then(|tab_index| screen.tabs.get_mut(&tab_index))
                {
                    active_tab.new_pane(
                        PaneId::Plugin(plugin_id),
                        Some(pane_title),
                        Some(run_plugin),
                        start_suppressed,
                        should_focus_plugin.unwrap_or(true),
                        new_pane_placement,
                        None,
                        None,
                    )?;
                } else {
                    log::error!("Tab index not found: {:?}", tab_index);
                }
                let pane_id = PaneId::Plugin(plugin_id);
                screen.activate_overlay(pane_id);
                let plugin_exists = screen.global_panes.pane(pane_id).is_some()
                    || screen.tabs.values().any(|tab| tab.has_plugin(plugin_id));
                if plugin_exists {
                    if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pane_id) {
                        replay_pending_plugin_events(&mut screen, pending_events)?;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePluginLoadingStage(pid, loading_indication) => {
                let found_plugin =
                    screen.update_plugin_loading_stage(pid, loading_indication.clone());
                if !found_plugin {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::UpdatePluginLoadingStage(
                            pid,
                            loading_indication,
                        ));
                }
                screen.render(None)?;
            },
            ScreenInstruction::StartPluginLoadingIndication(pid, loading_indication) => {
                if !screen.start_plugin_loading_indication(pid, loading_indication.clone()) {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::StartPluginLoadingIndication(
                            pid,
                            loading_indication,
                        ));
                }
                screen.render(None)?;
            },
            ScreenInstruction::ProgressPluginLoadingOffset(pid) => {
                if !screen.progress_plugin_loading_offset(pid) {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::ProgressPluginLoadingOffset(pid));
                }
                screen.render(None)?;
            },
            ScreenInstruction::RequestStateUpdateForPlugins => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    tab.update_input_modes()?;
                }
                screen.update_global_input_modes(screen.active_tab_ids.keys().copied())?;
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::LaunchOrFocusPlugin(
                run_plugin,
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                close_replaced_pane,
                pane_id_to_replace,
                skip_cache,
                client_id,
                mut completion_tx,
                explicit_tab_id,
                global_placement,
            ) => {
                if let Some(global_placement) = global_placement {
                    if should_float
                        || move_to_focused_tab
                        || should_open_in_place
                        || close_replaced_pane
                        || pane_id_to_replace.is_some()
                    {
                        if let Some(completion) = completion_tx.as_mut() {
                            completion.set_exit_status(1);
                            completion.set_error_message(
                            "global plugins cannot be floating, moved between tabs, or opened in place"
                                .to_owned(),
                        );
                        }
                    } else {
                        let resolved_tab_and_client =
                            screen.resolve_tab_and_client(client_id, explicit_tab_id);
                        match resolved_tab_and_client {
                            Some((tab_index, client_id)) => {
                                if screen.focus_global_plugin_pane(
                                    &run_plugin,
                                    client_id,
                                    &mut completion_tx,
                                )? {
                                    screen.render(None)?;
                                    screen.log_and_report_session_state()?;
                                } else if screen
                                    .tabs
                                    .values()
                                    .any(|tab| tab.find_plugin(&run_plugin).is_some())
                                {
                                    if let Some(completion) = completion_tx.as_mut() {
                                        completion.set_exit_status(1);
                                        completion.set_error_message(
                                        "plugin already exists as a tab-local pane; it cannot be promoted to global scope"
                                            .to_owned(),
                                    );
                                    }
                                } else {
                                    screen.bus.senders.send_to_pty(
                                        PtyInstruction::FillPluginCwd(
                                            Some(false),
                                            false,
                                            false,
                                            None,
                                            run_plugin,
                                            tab_index,
                                            None,
                                            client_id,
                                            Size::default(),
                                            skip_cache,
                                            None,
                                            None,
                                            None,
                                            completion_tx,
                                            Some(global_placement.into()),
                                            None,
                                        ),
                                    )?;
                                }
                            },
                            None => {
                                if let Some(completion) = completion_tx.as_mut() {
                                    completion.set_exit_status(1);
                                    completion.set_error_message(
                                        "No connected clients found - cannot load or focus plugin"
                                            .to_owned(),
                                    );
                                }
                            },
                        }
                    }
                } else {
                    match pane_id_to_replace {
                        Some(pane_id_to_replace) if should_open_in_place => {
                            let resolved_tab_index = explicit_tab_id
                                .or_else(|| screen.active_tab_ids.values().next().copied());
                            match resolved_tab_index {
                                Some(tab_index) => {
                                    let size = Size::default();
                                    screen.bus.senders.send_to_pty(
                                        PtyInstruction::FillPluginCwd(
                                            Some(should_float),
                                            should_open_in_place,
                                            close_replaced_pane,
                                            None,
                                            run_plugin,
                                            tab_index,
                                            Some(pane_id_to_replace),
                                            client_id,
                                            size,
                                            skip_cache,
                                            None,
                                            None,
                                            None,
                                            completion_tx,
                                            None,
                                            None,
                                        ),
                                    )?;
                                },
                                None => {
                                    log::error!(
                            "Could not find an active tab - is there at least 1 connected user?"
                        );
                                },
                            }
                        },
                        _ => {
                            let resolved_tab_and_client =
                                screen.resolve_tab_and_client(client_id, explicit_tab_id);
                            match resolved_tab_and_client {
                                Some((tab_index, client_id)) => {
                                    if screen.focus_plugin_pane(
                                        &run_plugin,
                                        should_float,
                                        move_to_focused_tab,
                                        should_open_in_place,
                                        client_id,
                                        &mut completion_tx,
                                    )? {
                                        screen.render(None)?;
                                        screen.log_and_report_session_state()?;
                                    } else {
                                        screen.bus.senders.send_to_pty(
                                            PtyInstruction::FillPluginCwd(
                                                Some(should_float),
                                                should_open_in_place,
                                                close_replaced_pane,
                                                None,
                                                run_plugin,
                                                tab_index,
                                                None,
                                                client_id,
                                                Size::default(),
                                                skip_cache,
                                                None,
                                                None,
                                                None,
                                                completion_tx,
                                                None,
                                                None,
                                            ),
                                        )?;
                                    }
                                },
                                None => {
                                    log::error!(
                                        "No connected clients found - cannot load or focus plugin"
                                    )
                                },
                            }
                        },
                    }
                }
            },
            ScreenInstruction::LaunchPlugin(
                run_plugin,
                should_float,
                should_open_in_place,
                close_replaced_pane,
                pane_id_to_replace,
                skip_cache,
                cwd,
                client_id,
                completion_tx,
                explicit_tab_id,
                no_focus,
                global_placement,
                overlay_placement,
            ) => {
                if let Some(global_placement) = global_placement {
                    let mut completion_tx = completion_tx;
                    if should_float
                        || should_open_in_place
                        || close_replaced_pane
                        || pane_id_to_replace.is_some()
                    {
                        if let Some(completion) = completion_tx.as_mut() {
                            completion.set_exit_status(1);
                            completion.set_error_message(
                                "global plugins cannot be floating or opened in place".to_owned(),
                            );
                        }
                    } else {
                        let resolved_tab_and_client =
                            screen.resolve_tab_and_client(client_id, explicit_tab_id);
                        match resolved_tab_and_client {
                            Some((tab_index, client_id)) => {
                                screen
                                    .bus
                                    .senders
                                    .send_to_pty(PtyInstruction::FillPluginCwd(
                                        Some(false),
                                        false,
                                        false,
                                        None,
                                        run_plugin,
                                        tab_index,
                                        None,
                                        client_id,
                                        Size::default(),
                                        skip_cache,
                                        cwd,
                                        if no_focus { Some(false) } else { None },
                                        None,
                                        completion_tx,
                                        Some(global_placement.into()),
                                        None,
                                    ))?;
                            },
                            None => {
                                if let Some(completion) = completion_tx.as_mut() {
                                    completion.set_exit_status(1);
                                    completion.set_error_message(
                                        "No connected clients found - cannot load plugin"
                                            .to_owned(),
                                    );
                                }
                            },
                        }
                    }
                } else {
                    match pane_id_to_replace {
                        Some(pane_id_to_replace) => {
                            let resolved_tab_index = explicit_tab_id
                                .or_else(|| screen.active_tab_ids.values().next().copied());
                            match resolved_tab_index {
                                Some(tab_index) => {
                                    let size = Size::default();
                                    screen.bus.senders.send_to_pty(
                                        PtyInstruction::FillPluginCwd(
                                            Some(should_float),
                                            should_open_in_place,
                                            close_replaced_pane,
                                            None,
                                            run_plugin,
                                            tab_index,
                                            Some(pane_id_to_replace),
                                            client_id,
                                            size,
                                            skip_cache,
                                            cwd,
                                            if no_focus { Some(false) } else { None },
                                            None,
                                            completion_tx,
                                            None,
                                            overlay_placement,
                                        ),
                                    )?;
                                },
                                None => {
                                    log::error!(
                                "Could not find an active tab - is there at least 1 connected user?"
                            );
                                },
                            }
                        },
                        None => {
                            let resolved_tab_and_client =
                                screen.resolve_tab_and_client(client_id, explicit_tab_id);
                            match resolved_tab_and_client {
                                Some((tab_index, client_id)) => {
                                    screen.bus.senders.send_to_pty(
                                        PtyInstruction::FillPluginCwd(
                                            Some(should_float),
                                            should_open_in_place,
                                            close_replaced_pane,
                                            None,
                                            run_plugin,
                                            tab_index,
                                            None,
                                            client_id,
                                            Size::default(),
                                            skip_cache,
                                            cwd,
                                            if no_focus { Some(false) } else { None },
                                            None,
                                            completion_tx,
                                            None,
                                            overlay_placement,
                                        ),
                                    )?;
                                },
                                None => {
                                    log::error!(
                                        "No connected clients found - cannot load or focus plugin"
                                    )
                                },
                            }
                        },
                    }
                }
            },
            ScreenInstruction::SuppressPane(pane_id, client_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.hide_pane(pane_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_non_suppressed_pane_with_pid(&pane_id) {
                        tab.suppress_pane(pane_id, Some(client_id));
                        drop(screen.render(None));
                        break;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UnsuppressPane(pane_id, should_float_if_hidden) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.show_pane(pane_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.unsuppress_pane(pane_id, should_float_if_hidden);
                        drop(screen.render(None));
                        break;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UnsuppressOrExpandPane(pane_id, should_float_if_hidden) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.show_pane(pane_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.unsuppress_or_expand_pane(pane_id, should_float_if_hidden);
                        drop(screen.render(None));
                        break;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::FocusPaneWithId(
                pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
                client_id,
                mut completion_tx,
            ) => {
                let pane_exists = screen.global_panes.pane(pane_id).is_some()
                    || screen
                        .tabs
                        .iter()
                        .any(|(_, tab)| tab.has_pane_with_pid(&pane_id));
                if !pane_exists {
                    if let Some(c) = completion_tx.as_mut() {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                } else {
                    let already_focused = screen
                        .get_active_pane_id(&client_id)
                        .map(|active| active == pane_id)
                        .unwrap_or(false);
                    if already_focused {
                        if let Some(c) = completion_tx.as_mut() {
                            c.set_exit_status(1);
                            c.set_error_message(format!("Pane {:?} is already focused", pane_id));
                        }
                    } else {
                        screen.focus_pane_with_id(
                            pane_id,
                            should_float_if_hidden,
                            should_be_in_place_if_hidden,
                            client_id,
                        )?;
                        screen.clear_bell_for_pane_id(pane_id, client_id);
                        screen.reconcile_single_pane_focus(client_id);
                        screen.sync_scroll_mode_on_focus(client_id)?;
                        screen.log_and_report_session_state()?;
                    }
                }
            },
            ScreenInstruction::RenamePane(
                pane_id,
                new_name,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.rename(new_name));
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        match tab.rename_pane(new_name, pane_id) {
                            Ok(()) => drop(screen.render(None)),
                            Err(e) => log::error!("Failed to rename pane: {:?}", e),
                        }
                        break;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::RenameActivePane(new_name, client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .rename_active_pane(new_name, client_id),
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::RenameTab(
                tab_index,
                new_name,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                // tab_index here is 1-based user input representing display position
                let tab_position = tab_index.saturating_sub(1); // Convert to 0-based

                match screen.get_tab_by_position_mut(tab_position) {
                    Some(tab) => {
                        tab.name = String::from_utf8_lossy(&new_name).to_string();
                    },
                    None => {
                        log::error!("Failed to find tab at position: {}", tab_position);
                    },
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::GoToTabWithId(tab_id, client_id, _completion_tx) => {
                let client_id_to_switch = client_id
                    .and_then(|cid| {
                        if screen.active_tab_ids.contains_key(&cid) {
                            Some(cid)
                        } else {
                            screen.active_tab_ids.keys().next().copied()
                        }
                    })
                    .or_else(|| screen.active_tab_ids.keys().next().copied());

                if let Some(client_id) = client_id_to_switch {
                    // Get the position from the ID
                    if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
                        // switch_active_tab expects 0-based position
                        screen.switch_active_tab(tab_position, None, true, client_id)?;
                        screen.render(None)?;

                        screen
                            .tab_history
                            .entry(client_id)
                            .or_default()
                            .push(tab_id);
                    } else {
                        log::error!("Tab with ID {} not found", tab_id);
                    }
                }
            },
            ScreenInstruction::RenameTabWithId(tab_id, new_name, _completion_tx) => {
                // Use get_tab_by_id_mut() helper method
                if let Some(tab) = screen.get_tab_by_id_mut(tab_id) {
                    tab.name = String::from_utf8_lossy(&new_name).to_string();
                    screen.log_and_report_session_state()?;
                } else {
                    log::error!("Failed to find tab with ID: {}", tab_id);
                }
            },
            ScreenInstruction::SetTabProjects(tab_projects) => {
                // Whole map each time, so a tab that lost its project loses the
                // name with it. Tabs we have never heard of are kept: a plugin can
                // know about a tab before we have finished making it, and the next
                // pass drops whatever is genuinely gone.
                let tab_projects: HashMap<usize, String> = tab_projects.into_iter().collect();
                if tab_projects != screen.tab_projects {
                    screen.tab_projects = tab_projects;
                    if screen.update_plugin_tab_names() {
                        screen.log_and_report_session_state()?;
                    }
                }
            },
            ScreenInstruction::CloseTabWithId(tab_id, _completion_tx) => {
                if screen.get_tab_by_id(tab_id).is_some() {
                    screen.close_tab_by_id(tab_id).non_fatal();
                } else {
                    log::error!("Failed to find tab with ID: {}", tab_id);
                }
            },
            ScreenInstruction::BreakPanesToTabWithId {
                pane_ids,
                tab_id,
                should_change_focus_to_target_tab,
                client_id,
                mut completion_tx,
            } => {
                if pane_ids
                    .iter()
                    .copied()
                    .find(|pane_id| screen.global_panes.pane(*pane_id).is_some())
                    .map(|pane_id| {
                        screen.reject_global_for_tab_operation_with_completion(
                            pane_id,
                            &mut completion_tx,
                        )
                    })
                    .unwrap_or(false)
                {
                    continue;
                }
                // tab_id is a stable id and the callee is id-space, so nothing to convert
                let outcome = screen.break_multiple_panes_to_tab_with_id(
                    pane_ids,
                    tab_id,
                    should_change_focus_to_target_tab,
                    client_id,
                )?;
                match outcome.affected_tab_id() {
                    Some(affected_tab_id) => {
                        completion_tx
                            .as_mut()
                            .map(|c| c.set_affected_tab_id(affected_tab_id));
                        let pane_group = screen.get_client_pane_group(&client_id);
                        if !pane_group.is_empty() {
                            let _ = screen.bus.senders.send_to_background_jobs(
                                BackgroundJob::HighlightPanesWithMessage(
                                    pane_group.iter().copied().collect(),
                                    "BROKEN OUT".to_owned(),
                                ),
                            );
                        }
                        screen.clear_pane_group(&client_id);
                    },
                    // leaving affected_tab_id None is what signals failure to the plugin
                    None => {
                        log::error!("Failed to move panes to tab with id {tab_id}: {outcome:?}")
                    },
                }
            },
            ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission) => {
                let global_plugin_id = PaneId::Plugin(plugin_id);
                if screen.global_panes.pane(global_plugin_id).is_some() {
                    screen.with_pane(global_plugin_id, |pane| {
                        pane.request_permissions_from_user(Some(plugin_permission.clone()))
                    });
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let found = all_tabs.values_mut().any(|tab| {
                    if tab.has_plugin(plugin_id) {
                        tab.request_plugin_permissions(plugin_id, Some(plugin_permission.clone()));
                        true
                    } else {
                        false
                    }
                });

                if !found {
                    log::error!("PluginId '{}' not found - caching request", plugin_id);
                    pending_events_waiting_for_client.push(
                        ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission),
                    );
                }
            },
            ScreenInstruction::WarpPane {
                pane_id,
                to_session,
                placement,
                client_id,
                response_channel,
            } => {
                // Every check that can fail happens before the pty thread is asked
                // for anything: a warp that is going to be refused must leave the
                // pane exactly where it was.
                match screen.check_pane_can_be_warped(pane_id, client_id) {
                    Ok(terminal_id) => {
                        screen.pending_warps.insert(terminal_id, response_channel);
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::ReleaseTerminalForWarp {
                                terminal_id,
                                to_session,
                                placement,
                                client_id,
                            })?;
                    },
                    Err(e) => {
                        let _ = response_channel.send(Err(e));
                    },
                }
            },
            ScreenInstruction::PaneReleasedForWarp {
                terminal_id,
                placement: _,
                to_session: _,
                client_id: _,
                released,
            } => {
                let response_channel = screen.pending_warps.remove(&terminal_id);
                let result = screen.take_released_pane_for_warp(terminal_id, released);
                // Nobody listening means the caller gave up waiting — its own
                // timeout fired while this was still in flight. The pane has
                // been taken out of its tab by now and its process is running
                // with nothing watching it, so the recovery line has to go
                // somewhere: this log is the only place left that it can.
                let stranded = match (response_channel, result) {
                    (Some(response_channel), result) => response_channel.send(result).err(),
                    (None, Ok(payload)) => Some(crossbeam::channel::SendError(Ok(payload))),
                    (None, Err(_)) => None,
                };
                if let Some(crossbeam::channel::SendError(Ok(payload))) = stranded {
                    log::error!(
                        "pane {terminal_id} was released for a warp nobody was waiting on; it \
                         is still running and can be recovered with `{}`",
                        crate::warp::recovery_command(&payload)
                    );
                }
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::AdoptedPaneReady {
                terminal_id,
                payload,
                placement,
                client_id,
                cmd,
                buffered,
                child_exit_status,
            } => {
                screen.place_adopted_pane(
                    terminal_id,
                    *payload,
                    placement,
                    client_id,
                    cmd,
                    buffered,
                    child_exit_status,
                )?;
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::WarpTab {
                tab_index,
                client_id,
                response_channel,
            } => {
                // Every check that can fail happens before the pty thread is
                // asked for anything, so a tab whose warp is going to be refused
                // stays exactly where it was.
                match screen.check_tab_can_be_warped(tab_index, client_id) {
                    Ok((tab_index, terminal_ids, prepared)) => {
                        screen
                            .pending_tab_warps
                            .insert(tab_index, (response_channel, prepared));
                        screen.bus.senders.send_to_pty(
                            PtyInstruction::ReleaseTerminalsForWarp {
                                tab_index,
                                terminal_ids,
                            },
                        )?;
                    },
                    Err(e) => {
                        let _ = response_channel.send(Err(e));
                    },
                }
            },
            ScreenInstruction::TabReleasedForWarp {
                tab_index,
                released,
            } => {
                if let Some((response_channel, prepared)) =
                    screen.pending_tab_warps.remove(&tab_index)
                {
                    let result = screen.take_released_tab_for_warp(tab_index, released, prepared);
                    if let Err(crossbeam::channel::SendError(Ok(payload))) =
                        response_channel.send(result)
                    {
                        // The caller timed out between asking and being
                        // answered. The tab is out of this session by now and
                        // every process in it is running unwatched.
                        log::error!(
                            "tab {tab_index} was released for a warp nobody was waiting on; its \
                             panes are still running and can be recovered with: {}",
                            payload
                                .tiled_panes
                                .iter()
                                .chain(payload.floating_panes.iter())
                                .map(crate::warp::recovery_command)
                                .collect::<Vec<_>>()
                                .join("; ")
                        );
                    }
                } else {
                    // Nobody is waiting, which means the supervisors have been
                    // released with no one to hand them to. The tab is still on
                    // screen and every pane in it is now talking to nothing, so
                    // saying where the processes went in the log is not enough —
                    // each pane has to say it too, or the user is left with a
                    // tab that looks alive and answers nothing.
                    log::error!(
                        "tab {tab_index} was released for a warp nobody was waiting on; its \
                         panes are still running and can be recovered with `zellij pty-host \
                         adopt <id>`"
                    );
                    for (terminal_id, released) in released {
                        if let Ok(released) = released {
                            screen
                                .bus
                                .senders
                                .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                                    terminal_id,
                                    why:
                                        "this tab was moved to another session, but nothing took it"
                                            .to_owned(),
                                    recovery: Some(released.recovery_command()),
                                })
                                .non_fatal();
                        }
                    }
                }
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::AdoptedTabReady {
                payload,
                adopted,
                client_id,
            } => {
                screen.place_adopted_tab(*payload, *adopted, client_id)?;
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::BreakPane(default_shell, client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                let default_layout = screen.default_layout.clone();
                screen.break_pane(default_shell, default_layout, client_id)?;
            },
            ScreenInstruction::BreakPaneRight(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                screen.break_pane_to_new_tab(Direction::Right, client_id)?;
            },
            ScreenInstruction::BreakPaneLeft(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                screen.break_pane_to_new_tab(Direction::Left, client_id)?;
            },
            ScreenInstruction::UpdateSessionInfos(new_session_infos, resurrectable_sessions) => {
                screen.update_session_infos(new_session_infos, resurrectable_sessions)?;
            },
            ScreenInstruction::UpdateAvailableLayouts(layouts, errors) => {
                screen.update_available_layouts(layouts, errors);
            },
            ScreenInstruction::ReplacePane(
                new_pane_id,
                hold_for_command,
                pane_title,
                invoked_with,
                close_replaced_pane,
                client_id_tab_index_or_pane_id,
                mut completion_tx,
            ) => {
                completion_tx
                    .as_mut()
                    .map(|c| c.set_affected_pane_id(new_pane_id));
                screen.replace_pane(
                    new_pane_id,
                    hold_for_command,
                    invoked_with,
                    pane_title,
                    close_replaced_pane,
                    client_id_tab_index_or_pane_id,
                )?;

                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SerializeLayoutForResurrection => {
                if screen.session_serialization {
                    screen.dump_layout_to_hd()?;
                }
            },
            ScreenInstruction::SaveSession(_client_id, completion_tx) => {
                let err_context = || "Failed to save session";

                screen.update_active_pane_ids();
                let pane_manifest = screen.generate_and_report_pane_state()?;
                let tab_infos = screen.generate_and_report_tab_state()?;

                #[cfg(not(test))]
                let (available_layouts, _layout_errors) = Layout::list_available_layouts(
                    screen.layout_dir.clone(),
                    &screen.default_layout_name,
                );
                #[cfg(test)]
                let available_layouts = vec![];

                let creation_time = {
                    let sock_path = ZELLIJ_SOCK_DIR.join(&screen.session_name);
                    std::fs::metadata(&sock_path)
                        .ok()
                        .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                        .and_then(|d| d.elapsed().ok())
                        .map(|d| Duration::from_secs(d.as_secs()))
                        .unwrap_or_default()
                };
                let session_info = SessionInfo {
                    name: screen.session_name.clone(),
                    tabs: tab_infos,
                    panes: pane_manifest,
                    connected_clients: screen.active_tab_ids.keys().len(),
                    is_current_session: true,
                    available_layouts,
                    web_clients_allowed: screen.web_sharing.web_clients_allowed(),
                    web_client_count: screen
                        .connected_clients
                        .borrow()
                        .iter()
                        .filter(|(_client_id, is_web_client)| **is_web_client)
                        .count(),
                    plugins: Default::default(),
                    tab_history: screen.tab_history.clone(),
                    pane_history: screen
                        .pane_history
                        .iter()
                        .map(|(k, v)| (*k, v.iter().map(|v| (*v).into()).collect()))
                        .collect(),
                    creation_time,
                };

                let session_layout_metadata = if screen.session_serialization {
                    screen.get_layout_metadata(Some(screen.default_shell.clone()), None)
                } else {
                    // Create empty metadata if serialization is disabled
                    SessionLayoutMetadata::new(screen.default_layout.clone())
                };

                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::SaveSessionToDisk {
                        session_name: screen.session_name.clone(),
                        session_info,
                        session_layout_metadata,
                        completion_tx,
                    })
                    .with_context(err_context)?;
            },
            ScreenInstruction::Notify(
                text,
                pane_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                // the survivors are about to be pushed away from the corner - remember where they
                // were so `render_notifications` can glide them to their new spot
                screen.notification_stack_before_change = screen.notifications.clone();
                screen.notification_displacement_progress = 0.0;
                let id = screen.next_notification_id;
                screen.next_notification_id += 1;
                let received_at_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                screen.notification_history.push(NotificationInfo {
                    id,
                    message: text.clone(),
                    received_at_unix_ms,
                });
                screen
                    .notifications
                    .insert(0, Notification::new(id, text, pane_id));
                screen
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::DismissNotification(
                        id,
                        screen.notification_timeout_secs,
                    ))?;
                screen
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::AnimateNotifications)?;
                screen.render(None)?;
            },
            ScreenInstruction::DismissNotification(id) => {
                // the toast stays in the stack, sliding back out, until
                // `TickNotificationAnimations` sees its `horizontal_progress` reach zero and
                // actually removes it - see `advance_notification_animations`
                screen.dismiss_notification(id)?;
            },
            ScreenInstruction::TickNotificationAnimations => {
                let still_animating = screen.advance_notification_animations();
                if !still_animating {
                    screen
                        .bus
                        .senders
                        .send_to_background_jobs(BackgroundJob::StopAnimateNotifications)?;
                }
                screen.render(None)?;
            },
            ScreenInstruction::RenameSession(
                name,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                if screen.peer_sessions_cache.contains_key(&name) {
                    let error_text = "A session by this name already exists.";
                    log::error!("{}", error_text);
                    if let Some(os_input) = &mut screen.bus.os_input {
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::LogError {
                                lines: vec![error_text.to_owned()],
                            },
                        );
                    }
                } else if screen.resurrectable_sessions_cache.contains_key(&name) {
                    let error_text =
                        "A resurrectable session by this name exists, cannot use this name.";
                    log::error!("{}", error_text);
                    if let Some(os_input) = &mut screen.bus.os_input {
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::LogError {
                                lines: vec![error_text.to_owned()],
                            },
                        );
                    }
                } else {
                    let err_context = || "Failed to rename session".to_string();
                    let old_session_name = screen.session_name.clone();

                    // update state
                    screen.session_name = name.clone();
                    screen.default_mode_info.session_name = Some(name.clone());
                    for (_client_id, mode_info) in screen.mode_info.iter_mut() {
                        mode_info.session_name = Some(name.clone());
                    }
                    for (_, tab) in screen.tabs.iter_mut() {
                        tab.rename_session(name.clone()).with_context(err_context)?;
                    }

                    // rename socket file
                    let old_socket_file_path = ZELLIJ_SOCK_DIR.join(&old_session_name);
                    let new_socket_file_path = ZELLIJ_SOCK_DIR.join(&name);
                    if let Err(e) = std::fs::rename(old_socket_file_path, new_socket_file_path) {
                        log::error!("Failed to rename ipc socket: {:?}", e);
                    }

                    // rename session_info folder (TODO: make this atomic, right now there is a
                    // chance background_jobs will re-create this folder before it knows the
                    // session was renamed)
                    let old_session_info_folder =
                        session_info_folder_for_session(&old_session_name);
                    let new_session_info_folder = session_info_folder_for_session(&name);
                    if let Err(e) =
                        std::fs::rename(old_session_info_folder, new_session_info_folder)
                    {
                        log::error!("Failed to rename session_info folder: {:?}", e);
                    }

                    // report
                    screen
                        .log_and_report_session_state()
                        .with_context(err_context)?;

                    // set the env variable
                    set_session_name(name.clone());
                    let connected_client_ids: Vec<ClientId> =
                        screen.active_tab_ids.keys().copied().collect();
                    for client_id in connected_client_ids {
                        if let Some(os_input) = &mut screen.bus.os_input {
                            let _ = os_input.send_to_client(
                                client_id,
                                ServerToClientMsg::RenamedSession { name: name.clone() },
                            );
                        }
                    }
                }
            },
            ScreenInstruction::Reconfigure {
                client_id,
                keybinds,
                default_mode,
                theme,
                host_theme_dark,
                host_theme_light,
                simplified_ui,
                default_shell,
                pane_frame_style,
                copy_to_clipboard,
                copy_command,
                copy_on_select,
                auto_layout,
                rounded_corners,
                hide_session_name,
                stacked_resize,
                stacked_pane_list,
                default_editor,
                advanced_mouse_actions,
                mouse_scroll_resize,
                mouse_hover_effects,
                visual_bell,
                focus_follows_mouse,
                mouse_click_through,
                osc133_command_selection,
                word_separators,
                nested_session_handling,
                dangerously_enable_paste_buffer_read,
            } => {
                screen.host_theme_dark_styling = host_theme_dark;
                screen.host_theme_light_styling = host_theme_light;
                screen
                    .reconfigure(
                        keybinds,
                        default_mode,
                        theme,
                        simplified_ui,
                        default_shell,
                        pane_frame_style,
                        copy_command,
                        copy_to_clipboard,
                        copy_on_select,
                        auto_layout,
                        rounded_corners,
                        hide_session_name,
                        stacked_resize,
                        stacked_pane_list,
                        default_editor,
                        advanced_mouse_actions,
                        mouse_scroll_resize,
                        mouse_hover_effects,
                        visual_bell,
                        focus_follows_mouse,
                        mouse_click_through,
                        osc133_command_selection,
                        word_separators,
                        nested_session_handling,
                        dangerously_enable_paste_buffer_read,
                        client_id,
                    )
                    .non_fatal();
            },
            ScreenInstruction::RerunCommandPane(terminal_pane_id, completion_tx) => {
                screen.rerun_command_pane_with_id(terminal_pane_id, completion_tx)
            },
            ScreenInstruction::ResizePaneWithId(resize, pane_id) => {
                screen.resize_pane_with_id(resize, pane_id)?;
            },
            ScreenInstruction::EditScrollbackForPaneWithId(pane_id, completion_tx) => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                            .non_fatal();
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::WriteToPaneId(bytes, pane_id, _completion) => {
                screen
                    .write_to_pane_id(&None, bytes, false, pane_id, None, _completion)
                    .non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::Paste(bytes, pane_id, client_id, _completion) => {
                match pane_id {
                    Some(pane_id) => {
                        screen
                            .paste_to_pane_id(bytes, pane_id, Some(client_id), _completion)
                            .non_fatal();
                    },
                    None => {
                        if let Some(pane_id) = screen.global_panes.focused_pane_id(client_id) {
                            screen
                                .paste_to_pane_id(bytes, pane_id, Some(client_id), _completion)
                                .non_fatal();
                            screen.render(None)?;
                            continue;
                        }
                        active_tab_and_connected_client_id!(
                            screen,
                            client_id,
                            |tab: &mut Tab, _client_id: ClientId| {
                                tab.paste_to_active_terminal(bytes, client_id, _completion)
                                    .non_fatal();
                            }
                        );
                    },
                }
                screen.render(None)?;
            },
            ScreenInstruction::SetPaneColor(pane_id, fg, bg, _completion) => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.set_pane_color(pane_id, fg, bg).non_fatal();
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::WriteKeyToPaneId(
                key_with_modifier,
                bytes,
                is_kitty,
                pane_id,
                _completion,
            ) => {
                screen
                    .write_to_pane_id(
                        &key_with_modifier,
                        bytes,
                        is_kitty,
                        pane_id,
                        None,
                        _completion,
                    )
                    .non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::CopyTextToClipboard(text, plugin_id) => {
                let plugin_pane_id = PaneId::Plugin(plugin_id);
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&plugin_pane_id) {
                        tab.copy_text_to_clipboard(&text)
                            .with_context(|| {
                                format!(
                                    "failed to copy text to clipboard from plugin {}",
                                    plugin_id
                                )
                            })
                            .non_fatal();
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::MovePaneWithPaneId(pane_id) => {
                if let Err(error) = screen.reject_global_for_tab_operation(pane_id) {
                    log::error!("{error}");
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.move_pane(pane_id);
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::MovePaneWithPaneIdInDirection(pane_id, direction) => {
                if let Err(error) = screen.reject_global_for_tab_operation(pane_id) {
                    log::error!("{error}");
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        match direction {
                            Direction::Down => tab.move_pane_down(pane_id),
                            Direction::Up => tab.move_pane_up(pane_id),
                            Direction::Left => tab.move_pane_left(pane_id),
                            Direction::Right => tab.move_pane_right(pane_id),
                        }
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClearScreenForPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.clear_screen());
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.clear_screen_for_pane_id(pane_id);
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::ScrollUpInPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.scroll_up(1, 1));
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_up(terminal_pane_id)?;
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling up"
                            );
                        }
                        break;
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollDownInPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.scroll_down(1, 1));
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_down(terminal_pane_id)?;
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling down"
                            );
                        }
                        break;
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToTopInPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| {
                        pane.clear_scroll();
                        if let Some(lines) = pane.get_line_number() {
                            pane.scroll_up(lines, 1);
                        }
                    });
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_to_top(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling to top"
                            );
                        }
                        break;
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToBottomInPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.clear_scroll());
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_to_bottom(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!("Currently only terminal panes are supported for scrolling to bottom");
                        }
                        break;
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollUpInPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_up(pane.rows().max(1).saturating_sub(1), 1)
                    });
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_page_up(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling"
                            );
                        }
                        break;
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollDownInPaneId(pane_id) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.scroll_down(pane.get_content_rows(), 1));
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_page_down(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling"
                            );
                        }
                        break;
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::TogglePaneIdFullscreen(pane_id) => {
                if let Err(error) = screen.reject_global_for_tab_operation(pane_id) {
                    log::error!("{error}");
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.toggle_pane_fullscreen(pane_id);
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::SetMobileRenderPreferences {
                client_id,
                single_pane,
                fit,
            } => {
                screen.set_mobile_render_preferences(client_id, single_pane, fit)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(pane_id) => {
                if let Err(error) = screen.reject_global_for_tab_operation(pane_id) {
                    log::error!("{error}");
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                            .non_fatal();
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::CloseTabWithIndex(tab_index) => {
                // tab_index here means display position (0-based) per API convention
                // Must map to stable ID for close_tab_by_id()

                if let Some(tab_id) = screen.get_tab_id_at_position(tab_index) {
                    screen.close_tab_by_id(tab_id).non_fatal();
                } else {
                    log::error!("Failed to find tab at position: {}", tab_index);
                }
            },
            ScreenInstruction::BreakPanesToNewTab {
                pane_ids,
                default_shell,
                should_change_focus_to_new_tab,
                new_tab_name,
                client_id,
                mut completion_tx,
            } => {
                if pane_ids
                    .iter()
                    .copied()
                    .find(|pane_id| screen.global_panes.pane(*pane_id).is_some())
                    .map(|pane_id| {
                        screen.reject_global_for_tab_operation_with_completion(
                            pane_id,
                            &mut completion_tx,
                        )
                    })
                    .unwrap_or(false)
                {
                    continue;
                }
                let tab_id = screen.break_multiple_panes_to_new_tab(
                    pane_ids,
                    default_shell,
                    should_change_focus_to_new_tab,
                    new_tab_name,
                    client_id,
                )?;
                // Set affected tab ID for plugin API return value
                completion_tx
                    .as_mut()
                    .map(|c| c.set_affected_tab_id(tab_id));
                // TODO: is this a race?
                let pane_group = screen.get_client_pane_group(&client_id);
                if !pane_group.is_empty() {
                    let _ = screen.bus.senders.send_to_background_jobs(
                        BackgroundJob::HighlightPanesWithMessage(
                            pane_group.iter().copied().collect(),
                            "BROKEN OUT".to_owned(),
                        ),
                    );
                }
                screen.clear_pane_group(&client_id);
            },
            ScreenInstruction::BreakPanesToTabWithIndex {
                pane_ids,
                tab_index,
                should_change_focus_to_new_tab,
                client_id,
                mut completion_tx,
            } => {
                if pane_ids
                    .iter()
                    .copied()
                    .find(|pane_id| screen.global_panes.pane(*pane_id).is_some())
                    .map(|pane_id| {
                        screen.reject_global_for_tab_operation_with_completion(
                            pane_id,
                            &mut completion_tx,
                        )
                    })
                    .unwrap_or(false)
                {
                    continue;
                }
                // tab_index here means display position (0-based) per API convention,
                // and the callee is id-space, so it must be mapped to a stable ID -
                // the same conversion CloseTabWithIndex does
                let Some(tab_id) = screen.get_tab_id_at_position(tab_index) else {
                    log::error!("Failed to find tab at position: {}", tab_index);
                    continue; // affected_tab_id stays None, signalling failure
                };
                let outcome = screen.break_multiple_panes_to_tab_with_id(
                    pane_ids,
                    tab_id,
                    should_change_focus_to_new_tab,
                    client_id,
                )?;
                match outcome.affected_tab_id() {
                    Some(affected_tab_id) => {
                        completion_tx
                            .as_mut()
                            .map(|c| c.set_affected_tab_id(affected_tab_id));
                        let pane_group = screen.get_client_pane_group(&client_id);
                        if !pane_group.is_empty() {
                            let _ = screen.bus.senders.send_to_background_jobs(
                                BackgroundJob::HighlightPanesWithMessage(
                                    pane_group.iter().copied().collect(),
                                    "BROKEN OUT".to_owned(),
                                ),
                            );
                        }
                        screen.clear_pane_group(&client_id);
                    },
                    None => log::error!(
                        "Failed to move panes to tab at position {tab_index} (id {tab_id}): {outcome:?}"
                    ),
                }
            },
            ScreenInstruction::TogglePanePinned(client_id, mut completion_tx) => {
                if screen.reject_focused_global_for_tab_operation_with_completion(
                    client_id,
                    &mut completion_tx,
                ) {
                    continue;
                }
                screen.toggle_pane_pinned(client_id);
            },
            ScreenInstruction::SetFloatingPanePinned(pane_id, should_be_pinned) => {
                if let Err(error) = screen.reject_global_for_tab_operation(pane_id) {
                    log::error!("{error}");
                    continue;
                }
                screen.set_floating_pane_pinned(pane_id, should_be_pinned);
            },
            ScreenInstruction::StackPanes(pane_ids_to_stack, client_id, mut completion_tx) => {
                if pane_ids_to_stack.iter().copied().any(|pane_id| {
                    screen.reject_global_for_tab_operation_with_completion(
                        pane_id,
                        &mut completion_tx,
                    )
                }) {
                    continue;
                }
                if let Some(last_pane_id) = screen.stack_panes(pane_ids_to_stack) {
                    let _ = screen.focus_pane_with_id(last_pane_id, false, false, client_id);
                    let _ = screen.render(None);
                    let pane_group = screen.get_client_pane_group(&client_id);
                    if !pane_group.is_empty() {
                        let _ = screen.bus.senders.send_to_background_jobs(
                            BackgroundJob::HighlightPanesWithMessage(
                                pane_group.iter().copied().collect(),
                                "STACKED".to_owned(),
                            ),
                        );
                    }
                    screen.clear_pane_group(&client_id);
                }
            },
            ScreenInstruction::ChangeFloatingPanesCoordinates(
                pane_ids_and_coordinates,
                mut completion_tx,
            ) => {
                if pane_ids_and_coordinates.iter().any(|(pane_id, _)| {
                    screen.reject_global_for_tab_operation_with_completion(
                        *pane_id,
                        &mut completion_tx,
                    )
                }) {
                    continue;
                }
                screen.change_floating_panes_coordinates(pane_ids_and_coordinates);
                let _ = screen.render(None);
            },
            ScreenInstruction::ChangeOverlayCoordinates(
                pane_ids_and_coordinates,
                mut completion_tx,
            ) => {
                if pane_ids_and_coordinates.iter().any(|(pane_id, _)| {
                    screen.reject_global_for_tab_operation_with_completion(
                        *pane_id,
                        &mut completion_tx,
                    )
                }) {
                    continue;
                }
                screen.change_overlay_coordinates(pane_ids_and_coordinates);
                let _ = screen.render(None);
            },
            ScreenInstruction::TogglePaneBorderless(pane_id, _completion_tx) => {
                screen.toggle_pane_borderless(pane_id);
                let _ = screen.render(None);
            },
            ScreenInstruction::SetPaneBorderless(pane_id, borderless, _completion_tx) => {
                screen.set_pane_borderless(pane_id, borderless);
                let _ = screen.render(None);
            },
            ScreenInstruction::GroupAndUngroupPanes(
                pane_ids_to_group,
                pane_ids_to_ungroup,
                for_all_clients,
                client_id,
            ) => {
                screen.group_and_ungroup_panes(
                    pane_ids_to_group,
                    pane_ids_to_ungroup,
                    for_all_clients,
                    client_id,
                );
                let _ = screen.log_and_report_session_state();
            },
            ScreenInstruction::TogglePaneInGroup(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.toggle_pane_in_group(client_id).non_fatal();
            },
            ScreenInstruction::ToggleGroupMarking(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.toggle_group_marking(client_id).non_fatal();
            },
            ScreenInstruction::SessionSharingStatusChange(web_sharing) => {
                if web_sharing {
                    screen.web_sharing = WebSharing::On;
                } else {
                    screen.web_sharing = WebSharing::Off;
                }

                for tab in screen.tabs.values_mut() {
                    tab.update_web_sharing(screen.web_sharing);
                }
                let _ = screen.log_and_report_session_state();
                let _ = screen.render(None);
            },
            ScreenInstruction::HighlightAndUnhighlightPanes(
                pane_ids_to_highlight,
                pane_ids_to_unhighlight,
                client_id,
            ) => {
                {
                    let all_tabs = screen.get_tabs_mut();
                    for pane_id in pane_ids_to_highlight {
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id) {
                                tab.add_highlight_pane_frame_color_override(
                                    pane_id,
                                    None,
                                    Some(client_id),
                                );
                            }
                        }
                    }
                    for pane_id in pane_ids_to_unhighlight {
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id) {
                                tab.clear_pane_frame_color_override(pane_id, Some(client_id));
                            }
                        }
                    }
                    screen.render(None)?;
                }
                let _ = screen.log_and_report_session_state();
            },
            ScreenInstruction::RevealPaneWithId(pane_id, label, client_id) => {
                screen.reveal_pane_with_id(pane_id, label, client_id)?;
            },
            ScreenInstruction::FloatMultiplePanes(pane_ids_to_float, client_id) => {
                if pane_ids_to_float
                    .iter()
                    .any(|pane_id| screen.global_panes.pane(*pane_id).is_some())
                {
                    log::error!("global pane has no tab");
                    continue;
                }
                {
                    let all_tabs = screen.get_tabs_mut();
                    let mut ejected_panes_in_group = vec![];
                    for pane_id in pane_ids_to_float {
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id) {
                                if !tab.pane_id_is_floating(&pane_id) {
                                    ejected_panes_in_group.push(pane_id);
                                    tab.toggle_pane_embed_or_floating_for_pane_id(
                                        pane_id,
                                        Some(client_id),
                                    )
                                    .non_fatal();
                                }
                                tab.show_floating_panes();
                            }
                        }
                    }
                    screen.render(None)?;
                    if !ejected_panes_in_group.is_empty() {
                        let _ = screen.bus.senders.send_to_background_jobs(
                            BackgroundJob::HighlightPanesWithMessage(
                                ejected_panes_in_group,
                                "EJECTED".to_owned(),
                            ),
                        );
                    }
                }
                let _ = screen.log_and_report_session_state();
            },
            ScreenInstruction::EmbedMultiplePanes(pane_ids_to_float, client_id) => {
                if pane_ids_to_float
                    .iter()
                    .any(|pane_id| screen.global_panes.pane(*pane_id).is_some())
                {
                    log::error!("global pane has no tab");
                    continue;
                }
                {
                    let all_tabs = screen.get_tabs_mut();
                    let mut embedded_panes_in_group = vec![];
                    for pane_id in pane_ids_to_float {
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id) {
                                if tab.pane_id_is_floating(&pane_id) {
                                    embedded_panes_in_group.push(pane_id);
                                    tab.toggle_pane_embed_or_floating_for_pane_id(
                                        pane_id,
                                        Some(client_id),
                                    )
                                    .non_fatal();
                                }
                                tab.hide_floating_panes();
                            }
                        }
                    }
                    screen.render(None)?;
                    if !embedded_panes_in_group.is_empty() {
                        let _ = screen.bus.senders.send_to_background_jobs(
                            BackgroundJob::HighlightPanesWithMessage(
                                embedded_panes_in_group,
                                "EMBEDDED".to_owned(),
                            ),
                        );
                    }
                }
                let _ = screen.log_and_report_session_state();
            },
            ScreenInstruction::InterceptKeyPresses(plugin_id, client_id) => {
                keybind_intercepts.insert(client_id, plugin_id);
            },
            ScreenInstruction::ClearKeyPressesIntercepts(client_id) => {
                keybind_intercepts.remove(&client_id);
            },
            ScreenInstruction::ReplacePaneWithExistingPane(
                old_pane_id,
                new_pane_id,
                suppress_replaced_pane,
                completion_tx,
            ) => screen.replace_pane_with_existing_pane(
                old_pane_id,
                new_pane_id,
                suppress_replaced_pane,
                completion_tx,
            ),
            ScreenInstruction::AddWatcherClient(client_id, size) => {
                screen
                    .add_watcher_client(client_id)
                    .context("failed to add watcher client")?;
                screen.set_watcher_size(client_id, size);
                screen.render(None)?;
            },
            ScreenInstruction::RemoveWatcherClient(client_id) => {
                screen.remove_watcher_client(client_id);
            },
            ScreenInstruction::SetFollowedClient(client_id) => {
                screen
                    .set_followed_client(client_id)
                    .context("failed to set followed client")?;
            },
            ScreenInstruction::WatcherTerminalResize(client_id, size) => {
                screen.set_watcher_size(client_id, size);
                screen.render(None)?;
            },
            ScreenInstruction::ClearMouseHelpText(client_id) => {
                if let Ok(tab) = screen.get_active_tab_mut(client_id) {
                    tab.clear_mouse_help_text(client_id);
                    screen.render(None)?;
                }
            },
            ScreenInstruction::SetPluginRegexHighlights {
                pane_id,
                plugin_id,
                highlights,
            } => {
                let style = screen.style.clone();
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.set_plugin_regex_highlights_for_pane(
                            pane_id, plugin_id, highlights, &style,
                        );
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClearPluginHighlights { pane_id, plugin_id } => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.clear_plugin_highlights_for_pane(pane_id, plugin_id);
                        break;
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClearAllPluginHighlights(plugin_id) => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    tab.clear_all_plugin_highlights(plugin_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::DesktopNotificationResponse(raw_bytes, client_id) => {
                if let Some((terminal_id, app_wants_report, is_query, rewritten_bytes)) =
                    denormalize_notification_response(&raw_bytes)
                {
                    let pane_id = PaneId::Terminal(terminal_id);
                    // Write response to the pane if the app expects it:
                    // capability query answers (q flag) or activation reports (r flag)
                    if app_wants_report || is_query {
                        let all_tabs = screen.get_tabs_mut();
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id) {
                                tab.write_to_pane_id(
                                    &None,
                                    rewritten_bytes,
                                    false,
                                    pane_id,
                                    None,
                                    None,
                                )
                                .non_fatal();
                                break;
                            }
                        }
                    }
                    // Focus the pane on activation click (not on query responses)
                    if !is_query {
                        screen
                            .focus_pane_with_id(pane_id, false, false, client_id)
                            .non_fatal();
                    }
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::SubscribeToPaneRenders {
                client_id,
                pane_ids,
                scrollback,
                ansi,
            } => {
                screen.subscribe_to_pane_renders(client_id, pane_ids, scrollback, ansi);
            },
            ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id } => {
                screen.notify_pane_closed_to_subscribers(pane_id);
            },
            ScreenInstruction::UpdateBackgroundPluginSubscriptions(
                plugin_id,
                client_id,
                subscriptions,
            ) => {
                if subscriptions.is_empty() {
                    screen
                        .background_plugin_subscriptions
                        .remove(&(plugin_id, client_id));
                } else {
                    screen
                        .background_plugin_subscriptions
                        .insert((plugin_id, client_id), subscriptions);
                }
            },
            ScreenInstruction::ClearHintTextCache => {
                for tab in screen.tabs.values_mut() {
                    tab.clear_hint_text_cache();
                }
                screen.render(None)?;
            },
            ScreenInstruction::BroadcastModeUpdate(mode_info, target_client_id) => {
                screen.broadcast_mode_update(mode_info, target_client_id)?;
            },
            // Pane-targeting CLI handlers
            ScreenInstruction::ScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.scroll_up(1, 1));
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.scroll_up_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.scroll_down(1, 1));
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.scroll_down_by_pane_id(pane_id).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToTopWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| {
                        pane.clear_scroll();
                        if let Some(lines) = pane.get_line_number() {
                            pane.scroll_up(lines, 1);
                        }
                    });
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.scroll_to_top_by_pane_id(pane_id).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToBottomWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.clear_scroll());
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.scroll_to_bottom_by_pane_id(pane_id).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_up(pane.rows().max(1).saturating_sub(1), 1)
                    });
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.page_scroll_up_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.scroll_down(pane.get_content_rows(), 1));
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.page_scroll_down_by_pane_id(pane_id).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::HalfPageScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_up(pane.rows().max(1).saturating_sub(1) / 2, 1)
                    });
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.half_page_scroll_up_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::HalfPageScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| {
                        pane.scroll_down(pane.rows().max(1).saturating_sub(1) / 2, 1)
                    });
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.half_page_scroll_down_by_pane_id(pane_id).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ResizeWithPaneId(pane_id, strategy, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.resize_pane_with_id(strategy, pane_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.resize_by_pane_id(pane_id, strategy);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetGlobalPanePlacement(pane_id, placement, mut completion_tx) => {
                if let Err(error) = screen.set_global_pane_placement(pane_id, placement) {
                    log::error!("{error}");
                    if let Some(completion_tx) = completion_tx.as_mut() {
                        completion_tx.set_exit_status(1);
                        completion_tx.set_error_message(error.to_string());
                    }
                } else {
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::MovePaneWithPaneIdCli(pane_id, direction, mut _completion_tx) => {
                if screen
                    .reject_global_for_tab_operation_with_completion(pane_id, &mut _completion_tx)
                {
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.move_pane_by_pane_id(pane_id, direction);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneBackwardsWithPaneId(pane_id, mut _completion_tx) => {
                if screen
                    .reject_global_for_tab_operation_with_completion(pane_id, &mut _completion_tx)
                {
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.move_pane_backwards_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ClearScreenWithPaneId(pane_id, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.clear_screen());
                    screen.render(None)?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.clear_screen_by_pane_id(pane_id).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::EditScrollbackWithPaneId(pane_id, ansi, completion_tx) => {
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        if ansi {
                            tab.edit_scrollback_raw_for_pane_with_id(pane_id, completion_tx)
                                .non_fatal();
                        } else {
                            tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                                .non_fatal();
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::ToggleFullscreenWithPaneId(pane_id, mut _completion_tx) => {
                if screen
                    .reject_global_for_tab_operation_with_completion(pane_id, &mut _completion_tx)
                {
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.toggle_fullscreen_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ToggleNoUiFullscreenWithPaneId(pane_id, mut _completion_tx) => {
                if screen
                    .reject_global_for_tab_operation_with_completion(pane_id, &mut _completion_tx)
                {
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.toggle_no_ui_fullscreen_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(pane_id, mut _completion_tx) => {
                if screen
                    .reject_global_for_tab_operation_with_completion(pane_id, &mut _completion_tx)
                {
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                            .non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::CloseFocusWithPaneId(pane_id, completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.close_global_pane(pane_id, completion_tx)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.close_pane_by_pane_id(pane_id, completion_tx)
                            .non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::RenamePaneWithPaneId(pane_id, name, mut _completion_tx) => {
                if screen.global_panes.pane(pane_id).is_some() {
                    screen.with_pane(pane_id, |pane| pane.rename(name));
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.rename_pane_by_pane_id(pane_id, name).non_fatal();
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UndoRenamePaneWithPaneId(pane_id, mut _completion_tx) => {
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.undo_rename_pane_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::TogglePanePinnedWithPaneId(pane_id, mut _completion_tx) => {
                if screen
                    .reject_global_for_tab_operation_with_completion(pane_id, &mut _completion_tx)
                {
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                let mut found = false;
                for tab in all_tabs.values_mut() {
                    if tab.has_pane_with_pid(&pane_id) {
                        tab.toggle_pane_pinned_by_pane_id(pane_id);
                        found = true;
                        break;
                    }
                }
                if !found {
                    log::error!("Pane with id {:?} not found", pane_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                }
            },
            // Tab-targeting CLI handlers
            ScreenInstruction::UndoRenameTabWithTabId(tab_id, mut _completion_tx) => {
                if let Some(tab) = screen.tabs.get_mut(&tab_id) {
                    if tab.name != tab.prev_name {
                        tab.name = tab.prev_name.clone();
                        screen.log_and_report_session_state()?;
                    }
                } else {
                    log::error!("Tab with id {} not found", tab_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Tab with id {} not found", tab_id));
                    }
                }
                screen.render(None)?;
            },
            ScreenInstruction::ToggleActiveSyncTabWithTabId(tab_id, mut _completion_tx) => {
                if let Some(tab) = screen.tabs.get_mut(&tab_id) {
                    tab.toggle_sync_panes_is_active();
                } else {
                    log::error!("Tab with id {} not found", tab_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Tab with id {} not found", tab_id));
                    }
                }
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::ToggleFloatingPanesWithTabId(
                tab_id,
                default_shell,
                mut completion_tx,
            ) => {
                if let Some(tab) = screen.tabs.get_mut(&tab_id) {
                    // Pass None as client_id so that if a new floating pane must be spawned,
                    // it targets this tab (via TabIndex) rather than the focused tab of some client.
                    tab.toggle_floating_panes(None, default_shell, completion_tx)
                        .non_fatal();
                } else {
                    log::error!("Tab with id {} not found", tab_id);
                    if let Some(ref mut c) = completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Tab with id {} not found", tab_id));
                    }
                    drop(completion_tx);
                }
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::PreviousSwapLayoutWithTabId(tab_id, mut _completion_tx) => {
                if let Some(tab) = screen.tabs.get_mut(&tab_id) {
                    tab.previous_swap_layout().non_fatal();
                } else {
                    log::error!("Tab with id {} not found", tab_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Tab with id {} not found", tab_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::NextSwapLayoutWithTabId(tab_id, mut _completion_tx) => {
                if let Some(tab) = screen.tabs.get_mut(&tab_id) {
                    tab.next_swap_layout().non_fatal();
                } else {
                    log::error!("Tab with id {} not found", tab_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Tab with id {} not found", tab_id));
                    }
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MoveTabWithTabId(tab_id, direction, _completion_tx) => {
                if pending_tab_ids.is_empty() {
                    screen.move_tab_by_id(tab_id, direction)?;
                    screen.render(None)?;
                } else {
                    pending_events_waiting_for_tab.push(ScreenInstruction::MoveTabWithTabId(
                        tab_id,
                        direction,
                        _completion_tx,
                    ));
                }
            },
            ScreenInstruction::SetSoftKeyboard { client_id, on } => {
                if let Some(os_input) = &screen.bus.os_input {
                    let _ = os_input
                        .send_to_client(client_id, ServerToClientMsg::SetSoftKeyboard { on });
                }
            },
            ScreenInstruction::FocusHostSession(client_id, _completion_tx) => {
                let payload = nested_session::encode_payload(&NestedSessionMessage::FocusHost {
                    direction: None,
                });
                let _ = screen.bus.senders.send_to_server(
                    ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                );
            },
            ScreenInstruction::FocusGuestSession(client_id, _completion_tx) => {
                screen.focus_guest_session(client_id);
            },
            ScreenInstruction::ToggleHostFullscreen(client_id, _completion_tx) => {
                screen.own_fullscreen_requested = !screen.own_fullscreen_requested;
                let fullscreen = screen.own_fullscreen_requested;
                let payload =
                    nested_session::encode_payload(&NestedSessionMessage::ToggleHostFullscreen {
                        fullscreen,
                    });
                let _ = screen.bus.senders.send_to_server(
                    ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                );
            },
        }
    }
    Ok(())
}

#[path = "../unit/screen_tests/mod.rs"]
#[cfg(test)]
mod screen_tests;
