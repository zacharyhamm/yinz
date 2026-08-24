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
use std::net::IpAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::str;
use std::time::{Duration, Instant};

use crate::route::NotificationEnd;

use log::{debug, warn};
use zellij_utils::data::{
    Direction, EventType, FloatingPaneCoordinates, GlobalPaneEdge, HostTerminalThemeMode,
    KeyWithModifier, LayoutInfo, LayoutWithError, NewPanePlacement, NotificationInfo, PaneInfo,
    RemotePaneHandoff, ResizeStrategy, SessionInfo, Styling, TabInfo, WarpedPane, WarpedTab,
    WebSharing,
};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::command::{OpenFilePayload, RunCommand};
use zellij_utils::input::keybinds::Keybinds;
use zellij_utils::input::options::{
    Clipboard, NestedSessionHandling, NotificationPosition, PaneFrameStyle,
};
use zellij_utils::ipc::MobileStatePayload;
use zellij_utils::pane_size::{PaneGeom, Size, SizeInPixels};
use zellij_utils::shared::clean_string_from_control_and_linebreak;
use zellij_utils::{
    input::command::TerminalAction,
    input::layout::{
        FloatingPaneLayout, Layout, Run, RunPluginOrAlias, SwapFloatingLayout, SwapTiledLayout,
        TiledPaneLayout,
    },
    input::parse_keys,
};

use crate::background_jobs::BackgroundJob;
use crate::pane_groups::PaneGroups;
use crate::panes::link_handler::LinkHandler;
use crate::panes::terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END};
use crate::session_layout_metadata::{PaneLayoutMetadata, SessionLayoutMetadata};

use crate::{
    global_panes::GlobalPaneManager,
    nested_guest::NestedGuestTracker,
    output::HostKittyState,
    panes::kitty_graphics::KittyImageStore,
    panes::sixel::SixelImageStore,
    panes::{PaneFactory, PaneId},
    plugins::{PluginId, PluginInstruction, PluginRenderAsset},
    pty::{get_default_shell, ClientTabIndexOrPaneId, GlobalPaneSpawnContext, PtyInstruction},
    pty_writer::PtyWriteInstruction,
    tab::{AdjustedInput, Pane, Tab},
    thread_bus::Bus,
    ui::loading_indication::LoadingIndication,
    ClientId, ServerInstruction,
};
use zellij_utils::{
    data::{Event, InputMode, ModeInfo, Palette, Style},
    ipc::{ClientAttributes, PixelDimensions},
};

use crate::mobile_web::MobileWebPrefs;

mod break_panes;
mod capabilities;
mod global_pane_focus;
mod global_pane_layout;
mod host_query;
mod input_modes;
mod instruction;
mod mouse;
mod nested;
mod notifications;
mod plugin_state;
mod render;
mod sizing;
mod subscriptions;
mod tabs;
mod thread_main;
mod warp_ops;

pub use instruction::ScreenInstruction;
pub(crate) use thread_main::screen_thread_main;

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
pub(crate) use active_tab_and_connected_client_id;

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
pub(crate) use active_tab_and_connected_client_id_with_first_tab_fallback;

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
            return match self.tab_containing_pane_mut(pane_id) {
                Some(tab) => tab.write_to_pane_id(
                    key_with_modifier,
                    raw_input_bytes,
                    raw_input_bytes_are_kitty,
                    pane_id,
                    client_id,
                    completion_tx,
                ),
                None => Err(anyhow!("failed to find pane with id {pane_id:?}")),
            };
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
            (
                PaneId::Terminal(terminal_id),
                Some(AdjustedInput::ResumeSuspendedProcessInThisPane),
            ) => {
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ResumeSuspendedPane(PaneId::Terminal(
                        terminal_id,
                    )))?;
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

    /// Returns a mutable reference to this [`Screen`]'s tabs.
    pub fn get_tabs_mut(&mut self) -> &mut BTreeMap<usize, Tab> {
        &mut self.tabs
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
        let tab_index = self.tab_position_containing_pane(pane_id);
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
            .tab_containing_pane(pane_id)
            .map(|tab| (tab.id, tab.position))
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
        match self.tab_containing_pane_mut(PaneId::Terminal(terminal_pane_id)) {
            Some(tab) => tab.rerun_terminal_pane_with_id(terminal_pane_id, completion_tx),
            None => log::error!(
                "Failed to find terminal pane with id: {} to run",
                terminal_pane_id
            ),
        }
    }
    pub fn resize_pane_with_id(&mut self, resize: ResizeStrategy, pane_id: PaneId) -> Result<()> {
        if self.global_panes.pane(pane_id).is_some() {
            let surface = self.surface_size();
            self.global_panes
                .resize_pane(pane_id, resize, surface.into())?;
            return self.reflow_global_panes();
        }
        match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => tab.resize_pane_with_id(resize, pane_id).non_fatal(),
            None => log::error!("Failed to find pane with id: {:?} to resize", pane_id),
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
                let tab_index = self.tab_position_containing_pane(pane_id);
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
        let Some(tab_index_of_pane_id_to_replace) =
            self.tab_position_containing_pane(pane_id_to_replace)
        else {
            log::error!(
                "Could not find tab with pane_id: {:?} to replace",
                pane_id_to_replace
            );
            return;
        };
        let Some(tab_index_of_existing_pane) =
            self.tab_position_containing_pane(pane_id_of_existing_pane)
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
        self.global_panes.set_pane_frame_style(pane_frame_style)?;
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
        match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => tab.set_floating_pane_pinned(pane_id, should_be_pinned),
            None => log::error!(
                "Failed to find pane with id: {:?} to set as pinned",
                pane_id
            ),
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
        let Some(root_tab_id) = self.tab_id_containing_pane(root_pane_id) else {
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
            if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                tab.change_overlay_coordinates(&pane_id, coordinates)
                    .non_fatal();
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
            if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                tab.change_floating_pane_coordinates(&pane_id, coordinates)
                    .non_fatal();
            }
        }
        for pane_id in pane_ids {
            self.activate_overlay(pane_id);
        }
    }
    pub fn toggle_pane_borderless(&mut self, pane_id: PaneId) {
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.toggle_pane_borderless(&pane_id).non_fatal();
        }
    }
    pub fn set_pane_borderless(&mut self, pane_id: PaneId, borderless: bool) {
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.set_pane_borderless(&pane_id, borderless).non_fatal();
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
                panes_in_group.retain(|p_id| self.tab_containing_pane(*p_id).is_some());
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

#[path = "../unit/screen_tests/mod.rs"]
#[cfg(test)]
mod screen_tests;
