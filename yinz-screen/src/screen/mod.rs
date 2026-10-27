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
//! - `tabs: BTreeMap<TabId, Tab>`: Keyed by tab.id (stable identifier)
//! - `active_tab_ids: BTreeMap<ClientId, TabId>`: Maps clients to active tab ID
//! - `tab_history: BTreeMap<ClientId, Vec<TabId>>`: History of tab IDs per client

mod agent_control;
mod agent_events;
mod agent_reports;
mod multipanes;
mod pane_tabs;
mod pending_events;

use crate::ui::loading_indication::DOT_PERIOD;
use crate::ui::notification::{ENTER_DURATION, ENTER_EASING};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::str;
use std::time::{Duration, Instant};
use yinz_types::animation::{Tween, FRAME_PERIOD};
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::TerminalId;
use yinz_types::multiplayer::AttachedClients;
use yinz_types::units::ElapsedSeconds;

use crate::route::NotificationEnd;

use log::{debug, warn};
use yinz_config::screen_config::ScreenConfig;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::input::hooks::HookPoint;
use yinz_types::keys::KeyWithModifier;
use yinz_types::pane_size::{Size, SizeInPixels};
use yinz_types::position::Position;
use yinz_types::style::{HostTerminalThemeMode, Styling, ThemeHue};
use yinz_types::text::clean_string_from_control_and_linebreak;
use yinz_types::{
    agent::{AgentInfo, AgentState},
    geometry::{Direction, FloatingPaneCoordinates, ResizeStrategy},
};
use {
    yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem,
    yinz_types::data::EventType, yinz_types::data::LayoutInfo, yinz_types::data::LayoutWithError,
    yinz_types::data::NotificationInfo, yinz_types::data::OverlayPlacement,
    yinz_types::data::PaneInfo, yinz_types::data::PluginTag, yinz_types::data::RemotePaneHandoff,
    yinz_types::data::SessionInfo, yinz_types::data::TabInfo, yinz_types::data::WarpedTab,
};
use {yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction};
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Layout,
    yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginLocation,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::layout::SwapFloatingLayout,
    yinz_types::input::layout::SwapTiledLayout, yinz_types::input::layout::TiledPaneLayout,
};
use {
    yinz_types::input::options::HostNotificationProtocol,
    yinz_types::input::options::NestedSessionHandling,
    yinz_types::input::options::NotificationPosition, yinz_types::input::options::PaneFrameStyle,
};

use crate::background_jobs::BackgroundJob;
use crate::notifications::NotificationProtocol;
use crate::output::Output;
use crate::pane_groups::PaneGroups;
use crate::panes::grid::{namespace_notification_id, PendingNotification};
use crate::panes::link_handler::LinkHandler;
use crate::panes::terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END};
use crate::session_layout_metadata::{PaneLayoutMetadata, SessionLayoutMetadata};

use crate::{
    nested_guest::NestedGuestTracker,
    output::HostKittyState,
    panes::kitty_graphics::KittyImageStore,
    panes::sixel::SixelImageStore,
    panes::PaneId,
    plugins::{PluginId, PluginInstruction, PluginRenderAsset},
    pty::{ClientTabIndexOrPaneId, PtyInstruction, ReplacedPane},
    tab::{Pane, Tab},
    thread_bus::Bus,
    ui::loading_indication::LoadingIndication,
    ClientId, ServerInstruction,
};
use yinz_types::style::{Palette, Style};
use {
    yinz_ipc::ipc::ClientAttributes, yinz_ipc::ipc::PixelDimensions, yinz_types::data::Event,
    yinz_types::data::InputMode, yinz_types::data::ModeInfo,
};

mod agent_placement;
mod agent_worktrees;
mod arrange_projects;
mod break_panes;
mod capabilities;
mod clients;
mod command_palette;
mod configuration;
mod context_menu;
mod headless_targets;
mod hooks;
mod host_query;
mod input_modes;
mod mouse;
mod nested;
mod notification_browser;
mod notification_policy;
mod notifications;
mod panes;
mod permission_modal;
mod plugin_state;
mod plugins;
mod render;
mod session;
mod sizing;
mod subscriptions;
mod tab_groups;
mod tab_layout;
mod tabs;
mod thread_main;
mod warp_ops;

pub(crate) use crate::tab::CopyOptions;
#[cfg(test)]
pub(crate) use thread_main::apply_pane_agent_report;
pub use thread_main::screen_thread_main;
pub use yinz_bus::screen::ScreenInstruction;
#[cfg(test)]
pub(crate) use yinz_bus::screen::TabOverrideResult;
pub(crate) use yinz_bus::screen::{
    GuestModalOutcome, HoldForCommand, InitialTitle, NotificationClick,
};

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
    ($screen:ident, $client_id:ident, $closure:expr_2021) => {
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
    ($screen:ident, $client_id:ident, $closure:expr_2021, ?) => {
        match $screen.get_active_tab_mut($client_id) {
            Ok(active_tab) => {
            $closure(active_tab)?;
            },
            Err(err) => Err::<(), _>(err).non_fatal(),
        };
    };
}
pub(crate) use active_tab;

macro_rules! active_tab_and_connected_client_id {
    ($screen:ident, $client_id:ident, $closure:expr_2021) => {
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
    ($screen:ident, $client_id:ident, $closure:expr_2021, ?) => {
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
    ($screen:ident, $client_id:ident, $closure:expr_2021) => {
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
    ($screen:ident, $client_id:ident, $closure:expr_2021, ?) => {
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

// We use this to delay rendering when a new tab opens so that we make sure all plugins
// (representing portions of the UI) have been fully loaded before the tab is first rendered (with
// a sensible timeout of 100ms)
#[derive(Debug, Clone)]
pub struct RenderBlocker {
    blocking_plugins: HashMap<PluginId, Instant>,
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

    pub fn register_blocking_plugin(&mut self, plugin_id: PluginId) {
        self.blocking_plugins.insert(plugin_id, Instant::now());
    }

    pub fn remove_blocking_plugin(&mut self, plugin_id: PluginId) {
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

/// What one plugin wants said, and decided, when the session is about to end.
///
/// See `PluginCommand::SetQuitNotice`. The two halves are independent: a plugin may have
/// something worth saying about a quit without wanting to stand in the way of one.
#[derive(Debug, Clone, Default)]
pub(crate) struct QuitNotice {
    /// One line for the quit confirmation to show, if this plugin has anything to say.
    pub notice: Option<String>,
    /// Whether closing the session's last pane should confirm rather than exit.
    pub guard: bool,
}

impl QuitNotice {
    /// Whether this is worth keeping at all. A plugin that says nothing and guards nothing has
    /// withdrawn, and its entry goes rather than sitting there empty.
    fn is_empty(&self) -> bool {
        self.notice.is_none() && !self.guard
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WatcherState {
    size: Size,
    should_force_render: bool,
    /// Whether the followed client's cursor fell outside this watcher's size on the last frame.
    /// `serialize_with_size` hides a cropped cursor in this watcher's stream alone, which the
    /// tab's memo knows nothing about - so the frame that brings the cursor back into range reads
    /// this and corrects the memo before placing the cursor (see `Screen::render_to_clients`).
    ///
    /// Plain bookkeeping, and deliberately not a reason to repaint: the cursor is the only thing
    /// the size cropping makes stateful across frames. Chunk clipping and the padding sweep are
    /// recomputed every frame, and `content_size` is the shared session surface rather than
    /// anything per-watcher - so the crop direction needs nothing either, serialization re-emitting
    /// `?25l` for as long as the cursor is out of range.
    cursor_was_cropped: bool,
}

impl WatcherState {
    pub fn new(size: Size) -> Self {
        WatcherState {
            size,
            should_force_render: true,
            cursor_was_cropped: false,
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

    pub fn cursor_was_cropped(&self) -> bool {
        self.cursor_was_cropped
    }

    pub fn set_cursor_was_cropped(&mut self, cursor_was_cropped: bool) {
        self.cursor_was_cropped = cursor_was_cropped;
    }
}

struct PaneRenderSubscription {
    pane_ids: HashSet<yinz_types::data::PaneId>,
    previous_viewports: HashMap<yinz_types::data::PaneId, Vec<String>>,
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

fn format_guest_modal_shortcut(keys: &[KeyWithModifier]) -> Vec<String> {
    keys.iter().map(|key| key.to_string()).collect()
}

/// A [`Screen`] holds multiple [`Tab`]s, each one holding multiple [`panes`](crate::panes).
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
    relationships: Option<yinz_types::data::WarpRelationships>,
    slots: Vec<PaneId>,
    pane_group: bool,
    name: Option<String>,
    user_named: bool,
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
    held: Option<yinz_types::data::HeldTerminal>,
    host_id: SupervisorId,
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
    pub destination_tab_id: Option<TabId>,
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
    pub fn affected_tab_id(&self) -> Option<TabId> {
        if self.succeeded() {
            self.destination_tab_id
        } else {
            None
        }
    }
}

/// Who decides the session's dark/light appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThemePolicy {
    /// The host terminal's reports (CSI 2031 / DSR 997) are followed.
    Auto,
    /// Pinned by `explicit_theme_hue` or a manual `SetDarkTheme` /
    /// `SetLightTheme`; host reports are recorded but not applied.
    Pinned(HostTerminalThemeMode),
}

/// The channel a warp's caller waits on for the serialized pane or tab, or for the reason it
/// could not be produced.
type WarpReplySender<T> = crossbeam::channel::Sender<Result<Box<T>, String>>;

/// A parked pane warp's caller, and what is needed to take the pane back if that
/// caller is gone by the time it is released.
pub(crate) struct ParkedPaneWarp {
    pub(crate) response_channel: yinz_bus::warp::PaneWarpReply,
    pub(crate) hold: yinz_bus::warp::PaneWarpHold,
    pub(crate) placement: yinz_types::data::WarpPlacement,
    pub(crate) client_id: ClientId,
}

/// Where a warped-in group of panes goes once its tab exists: the placement asked for, the tab
/// it is relative to, the client that asked, and the channel told when the placement is done.
type PendingGroupPlacement = (
    yinz_types::data::WarpPlacement,
    Option<TabId>,
    ClientId,
    Option<crossbeam::channel::Sender<Result<()>>>,
    Option<yinz_bus::warp::PaneWarpHold>,
);

/// Each Deno isolate has its own registration; Wasm subscribers have no generation.
type PaneOutputSubscriber = (PluginId, ClientId, Option<u64>);

/// The panes of a tab as a layout lists them, each with the metadata it is serialized from.
type LayoutPanes = Vec<(PaneId, PaneLayoutMetadata)>;

pub(crate) struct Screen {
    instance_id: uuid::Uuid,
    agent_events: agent_events::AgentEvents,
    agent_worktrees: agent_worktrees::Worktrees,
    agent_controls: Vec<agent_control::Pending>,
    agent_draft_clears: HashMap<PaneId, agent_control::DraftClear>,
    agent_submissions: HashMap<PaneId, std::collections::VecDeque<agent_control::Submission>>,
    agent_launches: HashMap<PaneId, yinz_types::agent::SpawnedAgent>,
    warped_agent_inputs: HashMap<PaneId, (yinz_types::data::WarpedAgentInput, u64)>,
    agent_lifecycle: HashMap<PaneId, agent_control::Lifecycle>,
    agent_dialog_inputs: HashMap<PaneId, agent_control::DialogInput>,
    agent_clear_inputs: HashMap<PaneId, agent_control::ClearInput>,
    agent_worker_waits: HashMap<PaneId, agent_control::WorkerWaits>,
    claude_permission_waits: HashMap<PaneId, agent_control::PermissionWait>,
    overlay_tab_plugins: Vec<yinz_types::pane_tabs::PaneTabDescriptor>,
    plugin_aliases: yinz_types::input::plugins::PluginAliases,
    pending_overlay_tabs: Option<plugins::PendingOverlayTabs>,
    overlay_tab_completions: Vec<(yinz_types::pane_tabs::PaneTabsId, usize, NotificationEnd)>,
    reported_pane_tabs: Vec<yinz_types::pane_tabs::PaneTabsInfo>,
    /// A Bus for sending and receiving messages with the other threads.
    pub bus: Bus<ScreenInstruction>,
    /// An optional maximal amount of panes allowed per [`Tab`] in this [`Screen`] instance.
    max_panes: Option<usize>,
    /// A map between this [`Screen`]'s tabs and their ID/key.
    tabs: BTreeMap<TabId, Tab>,
    // A closed last tab must not be mistaken for startup.
    has_created_tab: bool,
    /// Where each terminal pane's process is, as pty last reported it. Stamped
    /// onto every pane manifest so a plugin that missed the announcement — or
    /// loaded after it — is put right by the next update rather than never.
    terminal_cwds: HashMap<TerminalId, PathBuf>,
    /// Each terminal pane's foreground command, as pty last reported it, while a program
    /// other than the pane's shell is in the foreground.
    pub(crate) terminal_foreground_cmds: HashMap<TerminalId, Vec<String>>,
    /// The machine each remote pane runs on. Absent for local panes, which is
    /// nearly all of them.
    terminal_hosts: HashMap<TerminalId, String>,
    /// The name each tab last showed a plugin, so a change to one that no other
    /// event reports — a lone pane retitling itself, a project arriving, a
    /// collision forming or clearing — still redraws the tab bar.
    last_plugin_tab_names: HashMap<TabId, String>,
    /// The project each tab is working on, as the last `SetTabProjects` had it.
    ///
    /// Only a plugin knows what a project is, so this is told to us rather than
    /// worked out. Held raw — without the `#1`/`#2` a collision earns — because
    /// the suffixes depend on tab order, which changes without anyone sending us
    /// anything.
    arrangement_projects: HashMap<TabId, String>,
    pending_arrangement: Option<arrange_projects::PendingArrangement>,
    tab_projects: HashMap<TabId, String>,
    projects: std::collections::BTreeSet<String>,
    project_sources: BTreeMap<String, String>,
    pane_projects: HashMap<PaneId, String>,
    client_projects: HashMap<ClientId, command_palette::ClientProject>,
    last_project: yinz_runtime::last_project::LastProject,
    pixel_dimensions: PixelDimensions,
    client_pixel_dimensions: HashMap<ClientId, PixelDimensions>,
    client_cell_pixel_sizes: HashMap<ClientId, Option<SizeInPixels>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    stacked_resize: Rc<RefCell<bool>>,
    stacked_pane_list: Rc<RefCell<bool>>,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    kitty_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    /// What each client's probe answered, before the `support_kitty_graphics_protocol` option is
    /// applied to it: `kitty_host_capabilities` holds the masked answer, and a reload that turns
    /// the option back on has to recover the raw one.
    kitty_probe_answers: HashMap<ClientId, bool>,
    sixel_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    /// Whether each client's host terminal presents a frame atomically (DECSET 2026 synchronised
    /// output). A client absent from this map has not said, and is treated as not atomic - the
    /// worst case is the cursor handling we had before it was asked.
    synchronized_output_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    client_kitty_host_state: Rc<RefCell<HashMap<ClientId, HostKittyState>>>,
    terminal_emulator_colors: Rc<RefCell<Palette>>,
    terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
    /// The attached clients and the multiplayer color each holds. Shared with every tab.
    connected_clients: Rc<RefCell<AttachedClients>>,
    /// The indices of this [`Screen`]'s active [`Tab`]s.
    active_tab_ids: BTreeMap<ClientId, TabId>,
    client_sizes: HashMap<ClientId, Size>,
    /// The surface every tab was last laid out over, for when no client contributes a size at all:
    /// before the first client reports one, and after every client has detached - a
    /// `yinz action new-tab` still has to build a tab somebody can see when they reattach.
    ///
    /// Seeded with the size the session was built with. Upstream's `Screen::size` was always
    /// available to lay out against; `client_sizes` starts empty, and a render tick that lands
    /// before the first `RecomputeTabSize` would otherwise have nothing to lay a tab out against.
    last_surface_size: Size,
    /// The `unowned_strip_rows` last painted for each client. `Output` re-emits every chunk it is
    /// given and `Output::is_dirty` counts them, so filling these unconditionally would send a frame
    /// on every render tick (and force every watcher into a full repaint) - this is the diff.
    unowned_strip_fill: HashMap<ClientId, Vec<(usize, usize, usize)>>,
    /// The cells each client's toasts painted last frame, so the next one can work out which of
    /// them a moving toast stopped covering. Keyed by client rather than by render group because
    /// a client that switches tab or resizes gets different rects, and that difference *is* the
    /// damage. See `prepare_notification_damage`.
    last_notification_fill: HashMap<ClientId, Vec<crate::ui::damage::CellRect>>,
    /// Panes whose frame colour is currently easing back out of a bell flash. Kept so the frames
    /// get repainted while the colour moves - a pane whose contents are idle has nothing else to
    /// make it redraw - and so the animation clock knows when the last one is done.
    fading_frame_panes: HashSet<PaneId>,
    /// Plugins whose loading indicator is taking frames from the shared clock. The dots themselves
    /// are derived from how long the load has been running, so this only records *which* plugins
    /// are still loading.
    animating_plugin_loads: HashSet<PluginId>,
    /// The period last requested of the shared animation clock, `None` while it is stopped. Kept
    /// so `sync_animation_clock` is a no-op on the overwhelming majority of ticks rather than a
    /// message on the bus sixty times a second.
    animation_clock: Option<Duration>,
    global_last_active_tab_id: TabId,
    tab_history: BTreeMap<ClientId, Vec<TabId>>,
    tab_groups: BTreeMap<u32, tab_groups::TabGroup>,
    tab_group_ids: HashMap<TabId, u32>,
    warped_tab_groups: HashMap<(String, u32), u32>,
    next_tab_group_id: u64,
    pane_history: BTreeMap<ClientId, Vec<PaneId>>,
    mode_info: BTreeMap<ClientId, ModeInfo>,
    // Defaults for new clients and readers without a per-client mode entry.
    default_mode_info: ModeInfo,
    style: Style,
    pane_frame_style: PaneFrameStyle,
    max_agent_width: usize,
    /// Live toast notifications, newest first (index 0 sits in the configured corner).
    notifications: Vec<Notification>,
    notification_mouse_captures: HashSet<ClientId>,
    /// The open right-click context menu per client, if any.
    ///
    /// Per client rather than per session, and on `Screen` rather than on `Tab`, for the same two
    /// reasons the notification stack is: it is drawn after the per-tab loop in
    /// `render_to_clients`, where tab-owned state would be invisible, and it has to be torn down
    /// in `remove_client` alongside `notification_mouse_captures`. See `screen::context_menu`.
    context_menus: HashMap<ClientId, ContextMenuState>,
    command_palettes: HashMap<ClientId, command_palette::CommandPaletteState>,
    permission_modals: Vec<permission_modal::PermissionModal>,
    permission_last_client: Option<ClientId>,
    permission_mouse_captures: HashSet<ClientId>,
    command_palette_mouse_captures: HashSet<ClientId>,
    palette_providers: command_palette::plugins::Registry,
    /// Correlates a plugin's answer with the menu that asked for it. Monotonic per session.
    next_context_menu_request_id: ContextMenuRequestId,
    /// The items a right-click menu offers before its hook plugin has had its say.
    ///
    /// Held here, rather than only in the plugin thread, because the screen needs them for two
    /// things the plugin is not involved in: the fallback when the hook does not answer in time,
    /// and putting an item's actions back when it does.
    context_menu: ContextMenu,
    /// Every toast received by this live session. Dismissal only affects `notifications` above.
    /// ponytail: intentionally unbounded because the feature promises "all" notifications;
    /// add a configurable cap only if long-lived sessions show meaningful memory pressure.
    notification_history: Vec<NotificationRecord>,
    notification_policy: notification_policy::NotificationPolicy,
    /// Toasts committed before any client was ever connected, in arrival order: the session's
    /// first layout can raise one while its first client is still being registered. They are
    /// shown to that first client. `None` from then on, so a later attach never replays a toast
    /// committed while the session was detached; see `commit_notifications`.
    toasts_before_first_client: Option<Vec<NotificationId>>,
    /// The open notification browser per client in `Notifications` mode. See
    /// `screen::notification_browser`.
    notification_browsers: HashMap<ClientId, notification_browser::NotificationBrowser>,
    next_notification_id: NotificationId,
    notification_position: NotificationPosition,
    notification_timeout_secs: ElapsedSeconds,
    /// The floor a spawned agent pane's equal share of a tab must clear, columns by rows;
    /// zero in either turns the rule off. See `agent_placement`.
    agent_pane_min_size: Size,
    auto_layout: bool,
    session_serialization: bool,
    /// `Options::agent_double_escape`, pushed to every tab at the top of each screen-thread
    /// loop turn and replaced by `Reconfigure`.
    pub(super) agent_double_escape: bool,
    serialize_pane_viewport: bool,
    scrollback_lines_to_serialize: Option<usize>,
    /// Warps waiting on the pty thread, keyed by the terminal id being released.
    ///
    /// A warp takes two hops — ask pty to let the supervisor go, then serialize
    /// the pane once it has — and whoever is waiting on the far end has to still
    /// be reachable from the second hop.
    ///
    /// The `Instant` is the last moment the second hop may *start*: the
    /// caller's deadline less `PANE_WARP_SCREEN_RESERVE`, the same number the
    /// pty thread was given. It is kept because serializing the pane is also
    /// what closes it: by the time a send could report that nobody is
    /// listening, the pane is already gone and its process unwatched. Comparing
    /// against this before starting is the only chance to notice in time.
    pending_warps: HashMap<TerminalId, (ParkedPaneWarp, std::time::Instant)>,
    /// Live `PaneWarpHold`s. While any exists, an emptied session keeps its last
    /// tab and clients, because the pane may still come back.
    in_flight_pane_warps: usize,
    /// The same idea as `pending_warps`, keyed by tab id rather than terminal id
    /// because a tab's release is answered once for the whole tab — and carrying
    /// the arrangement, which was worked out before the release so that a tab
    /// that could not be rebuilt was never taken apart.
    ///
    /// The `Instant` is the tab-sized version of the one above: the caller's
    /// deadline less `TAB_WARP_SCREEN_RESERVE`. Not the caller's deadline
    /// itself — a release landing inside the reserve would pass that check and
    /// still have the tab closed under a caller that had gone by the time the
    /// serialization finished, which is exactly what the reserve exists to rule
    /// out.
    pending_pane_groups: HashMap<TabId, (ParkedPaneWarp, PreparedTabWarp, std::time::Instant)>,
    pending_group_placements: HashMap<TabId, PendingGroupPlacement>,
    pending_warp_relationships: HashMap<TabId, yinz_types::data::WarpRelationships>,
    /// Connections that left before attaching while a tab's layout was still on its way. A
    /// layout applied with nobody attached registers the client it was opened for, and one of
    /// these would never leave: its id is never handed out again. Emptied once no tab is pending.
    departed_before_layout: HashSet<ClientId>,
    pending_tab_warps: HashMap<
        TabId,
        (
            WarpReplySender<WarpedTab>,
            PreparedTabWarp,
            std::time::Instant,
        ),
    >,
    session_is_mirrored: bool,
    copy_options: CopyOptions,
    debug: bool,
    session_name: SessionName,
    config_file_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    peer_sessions_cache: BTreeMap<SessionName, SessionInfo>, // String is the session name, can
    // also be this session
    resurrectable_sessions_cache: BTreeMap<SessionName, Duration>, // String is the session name,
    // duration is its creation time
    default_layout: Box<Layout>,
    default_shell: PathBuf,
    /// Same shell as `default_shell`, in the `route_action`-ready shape (command plus
    /// `default_cwd`), for menu-triggered pane creation to route with the real configured
    /// shell instead of falling back to `crate::pty::get_default_shell()`.
    default_shell_action: Option<TerminalAction>,
    styled_underlines: bool,
    background_opacity: u8,
    osc8_hyperlinks: bool,
    arrow_fonts: bool,
    #[cfg_attr(test, allow(dead_code))]
    layout_dir: Option<PathBuf>,
    #[cfg_attr(test, allow(dead_code))]
    default_layout_name: Option<String>,
    layout_watcher: Option<plugin_state::LayoutWatcher>,
    layout_watcher_generation: u64,
    explicitly_disable_kitty_keyboard_protocol: bool,
    support_kitty_graphics_protocol: bool,
    default_editor: Option<PathBuf>,
    current_pane_group: Rc<RefCell<PaneGroups>>,
    advanced_mouse_actions: bool,
    osc133_command_selection: bool,
    word_separators: String,
    mouse_scroll_resize: bool,
    mouse_hover_effects: bool,
    visual_bell: bool,
    pane_animations: bool,
    focus_follows_mouse: bool,
    mouse_click_through: bool,
    context_menu_on_right_click: bool,
    context_menu_override_agent_mouse: bool,
    currently_marking_pane_group: Rc<RefCell<HashMap<ClientId, bool>>>,
    render_blocker: RenderBlocker,
    watcher_clients: HashMap<ClientId, WatcherState>,
    followed_client_id: Option<ClientId>,
    departed_followed_tab_id: Option<TabId>,
    cached_layouts: Vec<LayoutInfo>,
    cached_layout_errors: Vec<LayoutWithError>,
    pane_render_subscribers: HashMap<ClientId, PaneRenderSubscription>,
    /// General render requests invalidate all snapshots; output invalidates individual panes.
    pane_subscriber_snapshots_dirty: bool,
    pane_subscriber_dirty_panes: HashSet<PaneId>,
    deferred_render_tabs: HashSet<TabId>,
    render_writer_ready: OnceCell<yinz_bus::os_input_output::RenderReady>,
    deferred_subscriber_renders: bool,
    /// What each plugin wants said, and decided, when the session is about to end.
    ///
    /// Keyed by plugin id and dropped when that plugin's pane closes, so a plugin that goes away
    /// cannot leave a guard behind that nothing can lift. See `PluginCommand::SetQuitNotice`.
    quit_notices: BTreeMap<PluginId, QuitNotice>,
    /// Retained after loading so replacement terminals hold on natural exit.
    pub(super) replacement_tab: Option<TabId>,
    /// Waiting to relocate the emptied tabs and, if still needed, ask for confirmation.
    pub(super) replacement_pending: bool,
    /// Explicit close requests waiting for a usable destination for hidden work.
    pub(super) deferred_tab_closes: HashSet<TabId>,
    /// Whether a `yinz events` client is listening, so pane and tab state is reported even
    /// when no plugin asked for it. Set by the plugin thread, which keeps the subscribers.
    pub(super) cli_event_subscribers_present: bool,
    background_plugin_subscriptions: HashMap<(PluginId, ClientId), HashSet<EventType>>,
    /// Running plugins that declare no pane, with what each was loaded from. They never get a
    /// pane record; `launch_or_focus_plugin` matches against them so a second launch does not
    /// start another instance.
    pub(super) paneless_plugins: HashMap<PluginId, RunPluginOrAlias>,
    /// Those of `paneless_plugins` whose load failed or that crashed: they run no more, so a
    /// launch replaces one instead of finding it.
    pub(super) failed_paneless_plugins: HashSet<PluginId>,
    /// Plugins loading whose declaration waits for their repository (git plugins), with what
    /// each was loaded from. They have no pane record yet; a launch finds one instead of
    /// loading another.
    pub(super) resolving_plugins: HashMap<PluginId, RunPluginOrAlias>,
    /// Plugins the plugin thread has unloaded. Plugin ids are never reused, so an `AddPlugin`
    /// that arrives for one of these is a late answer to a load that is already gone.
    pub(super) unloaded_plugins: HashSet<PluginId>,
    next_forward_token: u32,
    pending_forwarded_queries: HashMap<u32, PendingForwardEntry>,
    forward_queue: VecDeque<PendingForward>,
    forward_in_flight_token: Option<u32>,
    pending_clipboard_forwards: HashMap<u32, PendingForwardEntry>,
    clipboard_forward_queue: VecDeque<PendingForward>,
    clipboard_forward_in_flight_token: Option<u32>,
    paste_buffer_read_enabled: bool,
    nested_guest_tracker: NestedGuestTracker,
    nested_ancestry: Vec<SessionName>,
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
    /// The mode the session is currently painted in (`None` until resolved).
    host_terminal_theme_mode: Option<HostTerminalThemeMode>,
    /// The last mode the host terminal reported, recorded even while a pin
    /// suppresses it so that unpinning can restore it.
    last_host_reported_theme_mode: Option<HostTerminalThemeMode>,
    theme_policy: ThemePolicy,
    /// The `explicit_theme_hue` the session was last configured with, so a
    /// reconfigure can tell a changed pin from an unchanged one.
    configured_explicit_theme_hue: Option<ThemeHue>,
    themes: BTreeMap<String, Styling>,
    terminal_themes: BTreeMap<String, yinz_types::style::TerminalTheme>,
    theme_name: Option<String>,
    theme_dark_name: Option<String>,
    theme_light_name: Option<String>,
    theme_preview: Option<command_palette::themes::Preview>,
    host_theme_dark_styling: Option<Styling>,
    host_theme_light_styling: Option<Styling>,
    nested_session_handling: NestedSessionHandling,
    pane_output_activity: HashMap<PaneId, Instant>,
    /// Session-lifetime tombstones, relying on terminal and plugin IDs not being reused.
    /// Absent panes with these IDs are already closed; unknown IDs may be awaiting creation.
    destroyed_panes: HashSet<PaneId>,
    pane_output_subscriptions: HashMap<PaneOutputSubscriber, BTreeMap<String, String>>,

    /// Whether each client's host terminal currently has keyboard focus, as reported through
    /// DECSET 1004. A client that never reported counts as focused.
    client_host_focused: HashMap<ClientId, bool>,
    /// The notification protocol resolved for each client from `host_notification_protocol`
    /// and its host terminal environment.
    client_notification_protocols: HashMap<ClientId, NotificationProtocol>,
    /// The configured `host_notification_protocol`, re-resolved for every client on reconfigure.
    host_notification_protocol: HostNotificationProtocol,
    /// Each client's whitelisted host terminal environment, kept so a reconfigure can re-resolve.
    client_host_terminal_env: HashMap<ClientId, BTreeMap<String, String>>,
    last_forwarded_osc7: HashMap<ClientId, Option<String>>,
    /// Pins what `frame_now` answers, so a test can step an animation frame by frame instead of
    /// sleeping. Never set outside tests.
    #[cfg(test)]
    pinned_now: Option<Instant>,
}

/// A live toast notification. See [`crate::ui::notification`] for how these are drawn.
///
/// Both animations are wall-clock [`Tween`]s sampled once per frame by `render_to_clients`, never
/// accumulated per tick: a late, dropped or doubled frame changes how much motion is *seen* and
/// never where a toast is or when it finishes.
/// The one place a frame reads the clock.
///
/// Split out so a test can pin it: production always answers `Instant::now()`, and a test that
/// wants to step an animation frame by frame sets `pinned_now` instead of sleeping.
impl Screen {
    pub(crate) fn set_pane_animations(&mut self, enabled: bool) {
        self.pane_animations = enabled;
        if !enabled {
            for tab in self.tabs.values_mut() {
                tab.settle_overlay_ripples();
            }
        }
    }

    pub(crate) fn frame_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(pinned) = self.pinned_now {
            return pinned;
        }
        Instant::now()
    }

    /// How often the shared clock should wake at `now`, or `None` if nothing on screen wants a
    /// frame any more.
    ///
    /// Derived from the animated state itself rather than from a registration list, which is what
    /// stops the clock getting stuck on and burning wakeups over an idle session.
    ///
    /// A toast sliding or a bell fading is continuous motion and needs [`FRAME_PERIOD`].
    /// Waiting borders pulse slowly enough to use 50ms frames.
    /// The plugin loading dots are a four-character
    /// "still working" pulse that changes once every [`DOT_PERIOD`], and holding the clock at 60fps
    /// for them means a full `render_to_clients` - global panes, every tab's layout, every pane's
    /// composition - sixty times a second for the entire time any plugin is loading, which on a
    /// plugin that never finishes is forever.
    ///
    /// The aggregate is computed here rather than requested by each surface on purpose: a plugin
    /// load starting while a toast is mid-slide must not be able to drop the clock to the dot
    /// period underneath it.
    pub(crate) fn animation_clock_period(&self, now: Instant) -> Option<Duration> {
        if self.notifications_are_animating(now)
            || !self.fading_frame_panes.is_empty()
            || self.tabs.values().any(|tab| tab.overlay_ripple_is_live())
        {
            Some(FRAME_PERIOD)
        } else if self.pane_animations
            && self
                .active_tab_ids
                .values()
                .copied()
                .chain(self.watched_tab_id())
                .any(|id| {
                    self.tabs.get(&id).is_some_and(|tab| {
                        tab.has_visible_waiting_pulse(
                            self.followed_client_id
                                .filter(|_| self.watched_tab_id() == Some(id)),
                        )
                    })
                })
        {
            Some(Duration::from_millis(50))
        } else if !self.animating_plugin_loads.is_empty()
            || self
                .tabs
                .values()
                .any(|tab| !tab.pending_agent_escapes.is_empty())
        {
            Some(DOT_PERIOD)
        } else {
            None
        }
    }

    /// Whether anything on screen still wants a frame. Production asks
    /// [`Screen::animation_clock_period`] the sharper question - *how often* - so this is the
    /// tests' way of asserting the clock stops.
    #[cfg(test)]
    pub(crate) fn animations_are_live(&self, now: Instant) -> bool {
        self.animation_clock_period(now).is_some()
    }

    /// Tell the shared clock what it should be doing, if that has changed since we last said so.
    ///
    /// Called after anything that starts or ends an animation, and on every tick - so a cadence
    /// that becomes coarser, or an animation that ends, is picked up within one period without
    /// every surface having to know about every other one.
    pub(crate) fn sync_animation_clock(&mut self, now: Instant) -> Result<()> {
        let wanted = self.animation_clock_period(now);
        if wanted == self.animation_clock {
            return Ok(());
        }
        match wanted {
            Some(period) => self
                .bus
                .senders
                .send_to_background_jobs(BackgroundJob::StartAnimationClock(period))?,
            None => self
                .bus
                .senders
                .send_to_background_jobs(BackgroundJob::StopAnimationClock)?,
        }
        self.animation_clock = wanted;
        Ok(())
    }

    /// One wakeup from the shared animation clock. Nothing is advanced here: every animation is
    /// sampled inside `render_to_clients` at the instant the frame is composited, so a tick that
    /// arrives late - or never - costs a frame and never a position.
    ///
    /// The frame is asked for unconditionally, including on the tick that finds everything settled.
    /// That last one is the frame that actually *paints* the settled state: renders are debounced
    /// by `REPAINT_DELAY_MS`, so the frame a still-animating tick asks for is composited up to that
    /// long before the deadline it was aiming at. Without a tick past the deadline asking for one
    /// more, a dismissing toast is left painted a sliver short of gone - and never reaped, because
    /// `reap_settled_notifications` only runs inside a frame - until unrelated output happens to
    /// repaint.
    pub(crate) fn on_animation_tick(&mut self, now: Instant) -> Result<()> {
        for tab in self.tabs.values_mut() {
            tab.expire_pending_agent_escapes(now);
        }
        self.render()?;
        self.sync_animation_clock(now)
    }

    /// Redraw the loading indicator of every plugin still loading. The pane itself skips the write
    /// on the frames where the derived dot count has not changed, which is most of them.
    pub(crate) fn advance_plugin_loading_animations(&mut self) {
        if self.animating_plugin_loads.is_empty() {
            return;
        }
        for plugin_id in self
            .animating_plugin_loads
            .iter()
            .copied()
            .collect::<Vec<_>>()
        {
            self.progress_plugin_loading_offset(plugin_id);
        }
    }

    /// Keep the frames of panes with a fading bell repainting, and forget the ones that have
    /// finished. A pane sitting idle produces no output of its own, so without this the flash
    /// would freeze at whatever strength the last frame happened to catch it at.
    pub(crate) fn advance_frame_color_fades(&mut self, now: Instant) {
        if self.fading_frame_panes.is_empty() {
            return;
        }
        let fading: Vec<PaneId> = self.fading_frame_panes.iter().copied().collect();
        for pane_id in fading {
            let Some(tab) = self.tab_containing_pane_mut(pane_id) else {
                // the pane went away mid-flash; nothing left to fade
                self.fading_frame_panes.remove(&pane_id);
                continue;
            };
            if tab.frame_color_override_is_faded_out(pane_id, now) {
                self.fading_frame_panes.remove(&pane_id);
                continue;
            }
            tab.set_pane_frame_should_render(pane_id);
        }
    }
}

/// A notification this session received, kept after its toast is gone so the notification browser
/// can list it and activate it later.
#[derive(Debug, Clone)]
struct NotificationRecord {
    info: NotificationInfo,
    pane_id: Option<PaneId>,
    on_click: Option<NotificationClick>,
    pending: bool,
    /// Keep the complete forwarded OSC 99 identifier, including its report flag.
    forwarded_id: Option<String>,
}

#[derive(Debug, Clone)]
struct Notification {
    id: NotificationId,
    text: String,
    pane_id: Option<PaneId>,
    /// Set only by a plugin that asked to hear about the click; see [`NotificationClick`].
    on_click: Option<NotificationClick>,
    /// `0.0` = hidden at the anchored edge, `1.0` = fully slid into place. Re-aimed at `0.0` on
    /// dismissal, and the toast is dropped from the stack once it gets there. Drives the colour
    /// fade as well as the column reveal, so the two cannot disagree about how far in it is.
    reveal: Tween,
    /// How many rows this toast is displaced from where the *current* stack says it belongs.
    /// Always tweens to `0.0`; a stack change adds the number of rows the slot moved and re-aims
    /// from there, so the painted row stays put while the target underneath it changes.
    ///
    /// A displacement rather than a position, because the settled position depends on the
    /// viewport and clients do not share one - see `notification_layout`.
    y_offset: Tween,
}

impl Notification {
    fn new(
        id: NotificationId,
        text: String,
        pane_id: Option<PaneId>,
        on_click: Option<NotificationClick>,
        now: Instant,
    ) -> Self {
        Notification {
            id,
            text,
            pane_id,
            on_click,
            reveal: Tween::new(0.0, 1.0, ENTER_DURATION, ENTER_EASING, now),
            y_offset: Tween::settled(0.0),
        }
    }

    /// Derived rather than stored: `reveal`'s target is the single source of truth for "is this on
    /// its way out", so the two can never disagree.
    fn is_dismissing(&self) -> bool {
        self.reveal.target() <= 0.0
    }

    /// Finished sliding out - ready to be dropped from the stack.
    fn is_gone(&self, now: Instant) -> bool {
        self.is_dismissing() && self.reveal.is_settled(now)
    }

    fn is_settled(&self, now: Instant) -> bool {
        self.reveal.is_settled(now) && self.y_offset.is_settled(now)
    }
}

/// An open context menu, from the right-click that asked for it to the item that closes it.
///
/// The pane it was opened over keeps focus for the menu's whole life, which is the property that
/// makes `Copy` and `CloseFocus` mean what they look like they mean - see `ui::context_menu`.
#[derive(Debug, Clone)]
pub(crate) struct ContextMenuState {
    /// Matched against the `request_id` in a plugin's answer. An answer for a menu that has since
    /// closed - or been replaced by a newer one - names a stale id and is dropped.
    request_id: ContextMenuRequestId,
    /// The clicked cell in surface coordinates. The menu is anchored here every frame rather than
    /// storing a resolved box, because a client's viewport can change under an open menu.
    anchor: Position,
    /// The pane the click landed in. Every item's actions run against it.
    pane_id: PaneId,
    /// What the hook plugin was told, kept so an item's echoed context can carry it back.
    context: ContextMenuContext,
    /// The plugin whose answer this menu is showing, if one answered. An item's echoed context is
    /// addressed to it alone - a menu's items are that plugin's business and nobody else's.
    plugin_id: Option<PluginId>,
    items: ContextMenuItems,
    /// Index into `items`, once they are `Ready`. Separators and disabled rows are never here.
    selected: Option<usize>,
}

/// Whether a menu yet knows what it says.
#[derive(Debug, Clone)]
pub(crate) enum ContextMenuItems {
    /// The hook has been fired and nothing is drawn yet.
    ///
    /// Drawing the configured items now and replacing them a frame later would make the list
    /// change under a pointer already on its way to a row, so a menu waits instead - bounded by
    /// `BackgroundJob::ContextMenuTimeout`, which promotes it to `configured` if the plugin never
    /// answers. That timer is why an unbound or wedged hook cannot make right-click do nothing.
    AwaitingPlugin { configured: Vec<ContextMenuItem> },
    /// The list the menu shows for the rest of its life, from whichever of the plugin and the
    /// timeout got there first. Nothing replaces it: the moment a menu is drawn it is also
    /// clickable, and a list that changes under a press the user has already made would run an
    /// item they never aimed at.
    Ready(Vec<ContextMenuItem>),
}

impl ContextMenuItems {
    /// The items to draw and hit-test: none at all while the plugin is still being waited on.
    fn visible(&self) -> &[ContextMenuItem] {
        match self {
            ContextMenuItems::AwaitingPlugin { .. } => &[],
            ContextMenuItems::Ready(items) => items,
        }
    }
}

struct NotificationLayout<'a> {
    notification: &'a Notification,
    lines: Vec<String>,
    footer: Option<String>,
    x: usize,
    y: usize,
    width: usize,
    /// The toast's sampled reveal for this frame: `0.0` hidden, `1.0` fully in place. Drives both
    /// the column clip below and the colour fade, sampled once so they cannot disagree.
    reveal: f32,
    revealed_x: usize,
    visible_cols: usize,
}

impl NotificationLayout<'_> {
    fn height(&self) -> usize {
        crate::ui::notification::notification_height(
            self.lines.len() + usize::from(self.footer.is_some()),
        )
    }
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

/// Reserved sentinel token for Yinz's own startup batch of host
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
        mut mode_info: ModeInfo,
        debug: bool,
        default_layout: Box<Layout>,
        config: ScreenConfig,
    ) -> Self {
        // No `..`: a field added to `ScreenConfig` has to say here what startup does with it.
        let ScreenConfig {
            overlay_tab_plugins,
            plugin_aliases,
            // Painted from `client_attributes.style`, which the first client resolved against its
            // command line as well as the config; see the field's doc. Same for the two chrome
            // flags.
            agent_theme_mapping: _,
            terminal_theme: _,
            terminal_themes,
            theme: _,
            rounded_corners: _,
            hide_session_name: _,
            // Applied by the screen thread once the screen exists.
            explicit_theme_hue: _,
            // Went into `mode_info`.
            default_mode: _,
            themes,
            theme_name,
            theme_dark_name,
            theme_light_name,
            host_theme_dark,
            host_theme_light,
            pane_frame_style,
            max_agent_width,
            tab_groups,
            simplified_ui,
            pane_animations,
            styled_underlines,
            background_opacity,
            osc8_hyperlinks,
            default_shell,
            default_cwd,
            default_editor,
            default_layout_name,
            layout_dir,
            auto_layout,
            stacked_resize,
            stacked_pane_list,
            agent_pane_min_size,
            agent_double_escape,
            advanced_mouse_actions,
            mouse_scroll_resize,
            mouse_hover_effects,
            focus_follows_mouse,
            mouse_click_through,
            context_menu_on_right_click,
            context_menu_override_agent_mouse,
            context_menu,
            osc133_command_selection,
            word_separators,
            copy_command,
            copy_clipboard,
            copy_on_select,
            support_kitty_keyboard_protocol,
            support_kitty_graphics_protocol,
            host_notification_protocol,
            nested_session_handling,
            dangerously_enable_paste_buffer_read,
            visual_bell,
            session_is_mirrored,
            session_serialization,
            serialize_pane_viewport,
            scrollback_lines_to_serialize,
            notification_position,
            notification_timeout_secs,
        } = config;
        mode_info.max_agent_width = Some(max_agent_width);
        mode_info.tab_groups = Some(tab_groups);
        let arrow_fonts = !simplified_ui;
        let explicitly_disable_kitty_keyboard_protocol = !support_kitty_keyboard_protocol;
        let default_shell_action = default_shell.clone().map(|shell| {
            TerminalAction::RunCommand(RunCommand {
                command: shell,
                cwd: default_cwd,
                use_terminal_title: true,
                ..Default::default()
            })
        });
        let default_shell = resolve_default_shell(default_shell);
        let default_editor = default_editor.or_else(get_default_editor);
        let copy_options = CopyOptions::new(
            copy_command,
            copy_clipboard.unwrap_or_default(),
            copy_on_select,
        );
        let session_name = mode_info.session_name.clone().unwrap_or_default();
        let session_info = SessionInfo::new(session_name.clone());
        let mut peer_sessions_cache = BTreeMap::new();
        let resurrectable_sessions_cache = BTreeMap::new();
        peer_sessions_cache.insert(session_name.clone(), session_info);
        let current_pane_group = PaneGroups::new(bus.senders.clone());
        let character_cell_size = Rc::new(RefCell::new(None));
        Screen {
            instance_id: uuid::Uuid::new_v4(),
            agent_events: Default::default(),
            agent_worktrees: Default::default(),
            agent_controls: Vec::new(),
            agent_draft_clears: HashMap::new(),
            agent_submissions: HashMap::new(),
            agent_launches: HashMap::new(),
            warped_agent_inputs: HashMap::new(),
            agent_lifecycle: HashMap::new(),
            agent_dialog_inputs: HashMap::new(),
            agent_clear_inputs: HashMap::new(),
            agent_worker_waits: HashMap::new(),
            claude_permission_waits: HashMap::new(),
            overlay_tab_plugins,
            plugin_aliases,
            pending_overlay_tabs: None,
            overlay_tab_completions: vec![],
            reported_pane_tabs: vec![],
            bus,
            max_panes,
            pixel_dimensions: Default::default(),
            client_pixel_dimensions: HashMap::new(),
            client_cell_pixel_sizes: HashMap::new(),
            character_cell_size,
            stacked_resize: Rc::new(RefCell::new(stacked_resize)),
            stacked_pane_list: Rc::new(RefCell::new(stacked_pane_list)),
            sixel_image_store: Rc::new(RefCell::new(SixelImageStore::default())),
            kitty_image_store: Rc::new(RefCell::new(KittyImageStore::default())),
            kitty_host_capabilities: Rc::new(RefCell::new(HashMap::new())),
            kitty_probe_answers: HashMap::new(),
            sixel_host_capabilities: Rc::new(RefCell::new(HashMap::new())),
            synchronized_output_capabilities: Rc::new(RefCell::new(HashMap::new())),
            client_kitty_host_state: Rc::new(RefCell::new(HashMap::new())),
            style: client_attributes.style,
            connected_clients: Rc::new(RefCell::new(AttachedClients::default())),
            active_tab_ids: BTreeMap::new(),
            client_sizes: HashMap::new(),
            last_surface_size: client_attributes.size,
            unowned_strip_fill: HashMap::new(),
            last_notification_fill: HashMap::new(),
            fading_frame_panes: HashSet::new(),
            animating_plugin_loads: HashSet::new(),
            animation_clock: None,
            global_last_active_tab_id: TabId::new(0),
            tabs: BTreeMap::new(),
            has_created_tab: false,
            terminal_cwds: HashMap::new(),
            terminal_foreground_cmds: HashMap::new(),
            terminal_hosts: HashMap::new(),
            last_plugin_tab_names: HashMap::new(),
            arrangement_projects: HashMap::new(),
            pending_arrangement: None,
            tab_projects: HashMap::new(),
            projects: Default::default(),
            project_sources: BTreeMap::new(),
            pane_projects: HashMap::new(),
            client_projects: HashMap::new(),
            last_project: Default::default(),
            terminal_emulator_colors: Rc::new(RefCell::new(Palette::default())),
            terminal_emulator_color_codes: Rc::new(RefCell::new(HashMap::new())),
            tab_history: BTreeMap::new(),
            tab_groups: BTreeMap::new(),
            tab_group_ids: HashMap::new(),
            warped_tab_groups: HashMap::new(),
            next_tab_group_id: 1,
            pane_history: BTreeMap::new(),
            mode_info: BTreeMap::new(),
            default_mode_info: mode_info,
            pane_frame_style,
            max_agent_width,
            notifications: vec![],
            notification_mouse_captures: HashSet::new(),
            context_menus: HashMap::new(),
            command_palettes: HashMap::new(),
            permission_modals: vec![],
            permission_last_client: None,
            permission_mouse_captures: HashSet::new(),
            command_palette_mouse_captures: HashSet::new(),
            palette_providers: command_palette::plugins::Registry::default(),
            next_context_menu_request_id: ContextMenuRequestId::new(0),
            context_menu,
            notification_history: vec![],
            notification_policy: Default::default(),
            toasts_before_first_client: Some(vec![]),
            notification_browsers: HashMap::new(),
            next_notification_id: NotificationId::new(0),
            notification_position,
            notification_timeout_secs,
            agent_pane_min_size,
            auto_layout,
            session_is_mirrored,
            copy_options,
            debug,
            session_name,
            config_file_path: None,
            config_dir: None,
            peer_sessions_cache,
            default_layout,
            default_layout_name,
            default_shell,
            default_shell_action,
            session_serialization,
            agent_double_escape,
            serialize_pane_viewport,
            scrollback_lines_to_serialize,
            in_flight_pane_warps: 0,
            pending_warps: HashMap::new(),
            pending_tab_warps: HashMap::new(),
            pending_warp_relationships: HashMap::new(),
            departed_before_layout: HashSet::new(),
            pending_pane_groups: HashMap::new(),
            pending_group_placements: HashMap::new(),
            styled_underlines,
            background_opacity,
            osc8_hyperlinks,
            arrow_fonts,
            resurrectable_sessions_cache,
            layout_dir,
            layout_watcher: None,
            layout_watcher_generation: 0,
            explicitly_disable_kitty_keyboard_protocol,
            support_kitty_graphics_protocol,
            default_editor,
            current_pane_group: Rc::new(RefCell::new(current_pane_group)),
            currently_marking_pane_group: Rc::new(RefCell::new(HashMap::new())),
            advanced_mouse_actions,
            osc133_command_selection,
            word_separators,
            mouse_scroll_resize,
            mouse_hover_effects,
            visual_bell,
            pane_animations,
            focus_follows_mouse,
            mouse_click_through,
            context_menu_on_right_click,
            context_menu_override_agent_mouse,
            render_blocker: RenderBlocker::new(100),
            watcher_clients: HashMap::new(),
            followed_client_id: None,
            departed_followed_tab_id: None,
            cached_layouts: vec![],
            cached_layout_errors: vec![],
            pane_render_subscribers: HashMap::new(),
            pane_subscriber_snapshots_dirty: true,
            pane_subscriber_dirty_panes: HashSet::new(),
            deferred_render_tabs: HashSet::new(),
            render_writer_ready: OnceCell::new(),
            deferred_subscriber_renders: false,
            quit_notices: BTreeMap::new(),
            replacement_tab: None,
            replacement_pending: false,
            deferred_tab_closes: HashSet::new(),
            cli_event_subscribers_present: false,
            background_plugin_subscriptions: HashMap::new(),
            paneless_plugins: HashMap::new(),
            failed_paneless_plugins: HashSet::new(),
            resolving_plugins: HashMap::new(),
            unloaded_plugins: HashSet::new(),
            next_forward_token: 1, // 0 is reserved as the startup sentinel
            pending_forwarded_queries: HashMap::new(),
            forward_queue: VecDeque::new(),
            forward_in_flight_token: None,
            pending_clipboard_forwards: HashMap::new(),
            clipboard_forward_queue: VecDeque::new(),
            clipboard_forward_in_flight_token: None,
            paste_buffer_read_enabled: dangerously_enable_paste_buffer_read,
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
            last_host_reported_theme_mode: None,
            theme_policy: ThemePolicy::Auto,
            configured_explicit_theme_hue: None,
            themes,
            theme_name,
            theme_dark_name,
            theme_light_name,
            theme_preview: None,
            terminal_themes,
            host_theme_dark_styling: host_theme_dark,
            host_theme_light_styling: host_theme_light,
            nested_session_handling,
            pane_output_activity: HashMap::new(),
            destroyed_panes: HashSet::new(),
            pane_output_subscriptions: HashMap::new(),
            client_host_focused: HashMap::new(),
            client_notification_protocols: HashMap::new(),
            host_notification_protocol,
            client_host_terminal_env: HashMap::new(),
            last_forwarded_osc7: HashMap::new(),
            #[cfg(test)]
            pinned_now: None,
        }
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
        match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => tab.write_to_pane_id(
                key_with_modifier,
                raw_input_bytes,
                raw_input_bytes_are_kitty,
                pane_id,
                client_id,
                completion_tx,
            ),
            None => Err(anyhow!("failed to find pane with id {pane_id:?}")),
        }
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
    pub fn get_tabs_mut(&mut self) -> &mut BTreeMap<TabId, Tab> {
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

    /// The client a "focused tab/pane" action is really for: `client_id` when it is attached,
    /// otherwise the first client there is - the same fallback
    /// `active_tab_and_connected_client_id!` makes silently, spelled out for the callers that
    /// have to know *which* client was picked.
    pub fn client_to_act_for(&self, client_id: ClientId) -> Option<ClientId> {
        match self.get_active_tab(client_id).is_ok() {
            true => Some(client_id),
            false => self.get_first_client_id(),
        }
    }

    /// True when a "focused tab/pane" action has nobody to act for: `client_id` is not attached
    /// and there is no other client either, so the fallback that
    /// `active_tab_and_connected_client_id!` makes silently would find nothing.
    pub fn has_no_client_to_act_for(&self, client_id: ClientId) -> bool {
        self.get_active_tab(client_id).is_err() && self.get_first_client_id().is_none()
    }

    /// Reports `message` back to whoever issued the action (the CLI, for `yinz action ...`)
    /// and completes `completion_tx` so the caller stops waiting.
    pub fn report_no_client_to_act_for(
        &self,
        client_id: ClientId,
        message: String,
        completion_tx: Option<NotificationEnd>,
    ) {
        self.bus
            .senders
            .send_to_server(ServerInstruction::LogError(
                vec![message],
                client_id,
                completion_tx,
            ))
            .context("failed to report that no client is attached")
            .non_fatal();
    }

    fn resolve_tab_and_client(
        &self,
        requested_client_id: ClientId,
        explicit_tab_id: Option<TabId>,
    ) -> Option<(TabId, ClientId)> {
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
        let group = self.group_for_client(Some(client_id));
        let history = self
            .tab_history
            .get_mut(&client_id)
            .context("missing tab history")?;
        let previous = history.iter().rposition(|id| {
            self.default_mode_info.tab_groups == Some(false)
                || self.tab_group_ids.get(id) == Some(&group)
        });
        Ok(previous.and_then(|index| self.tabs.get(&history.remove(index))))
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
        let tab_id_and_pane_id: Option<(TabId, PaneId)> =
            self.get_active_tab_mut(client_id).ok().and_then(|tab| {
                tab.get_active_pane_id(client_id)
                    .map(|pane_id| (tab.id, pane_id))
            });
        if let Some((tab_id, focused_pane_id)) = tab_id_and_pane_id
            && let Some(tab) = self.tabs.get_mut(&tab_id)
            && tab.panes_with_pending_bell.contains(&focused_pane_id)
        {
            let tab_had_bell = tab.tab_has_pending_bell;
            tab.clear_bell_notification_for_pane(focused_pane_id);
            let tab_bell_now_cleared = tab_had_bell && !tab.tab_has_pending_bell;
            let _ = self
                .bus
                .senders
                .send_to_background_jobs(BackgroundJob::StopFlashPaneBell(vec![focused_pane_id]));
            if tab_bell_now_cleared {
                let _ = self
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::StopFlashTabBell(tab_id));
            }
        }
    }

    /// Clear bell notification for a specific pane ID in the given client's active tab.
    pub fn clear_bell_for_pane_id(&mut self, pane_id: PaneId, client_id: ClientId) {
        let tab_id: Option<TabId> = self.get_active_tab_mut(client_id).ok().map(|tab| tab.id);
        if let Some(tab_id) = tab_id
            && let Some(tab) = self.tabs.get_mut(&tab_id)
            && tab.panes_with_pending_bell.contains(&pane_id)
        {
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

    pub fn show_floating_panes_in_tab(
        &mut self,
        client_id: ClientId,
        tab_id: Option<TabId>,
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
        tab_id: Option<TabId>,
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
        tab_id: Option<TabId>,
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
        tab_id: TabId,
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

        let tab_name = tab_name.unwrap_or_default();

        let position = TabPosition::new(self.tabs.len());
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
            self.current_pane_group.clone(),
            self.currently_marking_pane_group.clone(),
            self.advanced_mouse_actions,
            self.mouse_scroll_resize,
            self.mouse_hover_effects,
            self.focus_follows_mouse,
            self.mouse_click_through,
            self.context_menu_on_right_click,
            self.context_menu_override_agent_mouse,
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
        tab.max_agent_width = self.max_agent_width;
        tab.update_background_opacity(self.background_opacity);
        tab.update_selection_options(self.osc133_command_selection, self.word_separators.clone());
        self.tabs.insert(tab_id, tab);
        self.assign_tab_group(tab_id, client_id);
        self.has_created_tab = true;
        Ok(())
    }
    /// Whether the name a new tab was asked for is the user's, so it holds off the project.
    ///
    /// With no layout, a name was typed in a command (`new-tab --name`, a keybinding, a
    /// plugin), so it is. A layout says whose its name is: its own - a fallback - unless a
    /// command filled it in or it records a rename, as a resurrected session's does.
    pub(crate) fn tab_name_is_users(
        tab_name: Option<&str>,
        layout: Option<&TiledPaneLayout>,
    ) -> bool {
        tab_name.is_some_and(|name| !name.is_empty())
            && layout.is_none_or(|layout| layout.user_named)
    }
    /// Returns the committed departed source whose pending plugin output must be discarded.
    pub fn apply_layout(
        &mut self,
        layout: TiledPaneLayout,
        mut floating_panes_layout: Vec<FloatingPaneLayout>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        mut new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        mut new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        tab_id: TabId,
        should_change_client_focus: bool,
        client_id: ClientId,
        mut blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> Result<Option<ClientId>> {
        let departed = self.departed_follow_source();
        let applied = (|| {
            if !self.tabs.contains_key(&tab_id) {
                let message = format!("Tab with index {tab_id} not found. Cannot apply layout!");
                return Err(anyhow!(message));
            }
            let client_id = if self.get_active_tab(client_id).is_ok() {
                client_id
            } else if let Some(first_client_id) = self.get_first_client_id() {
                first_client_id
            } else {
                client_id
            };
            let err_context = || format!("failed to apply layout for tab {tab_id:?}",);

            let warp_slots = if self.pending_warp_relationships.contains_key(&tab_id) {
                let mut plugins = new_plugin_ids.clone();
                let mut tiled = new_terminal_ids.iter();
                let mut floating = new_floating_terminal_ids.iter();
                let mut slots = vec![];
                for (runs, terminals) in [
                    (layout.extract_run_instructions(), &mut tiled),
                    (
                        floating_panes_layout
                            .iter()
                            .map(|pane| pane.run.clone())
                            .collect(),
                        &mut floating,
                    ),
                ] {
                    for run in runs {
                        let id = match run {
                            Some(Run::Plugin(plugin)) => plugins
                                .get_mut(&plugin)
                                .and_then(|ids| ids.pop())
                                .map(PaneId::Plugin),
                            _ => terminals.next().map(|(id, _)| PaneId::Terminal(*id)),
                        }
                        .context("missing warp layout slot")?;
                        slots.push(id);
                    }
                }
                Some(slots)
            } else {
                None
            };

            let mut hidden_companions = vec![];
            if let (Some(slots), Some(relationships)) = (
                warp_slots.as_ref(),
                self.pending_warp_relationships.get(&tab_id),
            ) {
                relationships
                    .validate(slots.len())
                    .map_err(|e| anyhow!("{e}"))?;
                let tiled_slots = layout.extract_run_instructions().len();
                if relationships
                    .hidden
                    .iter()
                    .any(|hidden| (hidden.pane as usize) < tiled_slots)
                {
                    return Err(anyhow!("hidden warp companion occupies a tiled slot"));
                }
                let hidden_ids: HashSet<_> = relationships
                    .hidden
                    .iter()
                    .map(|hidden| slots[hidden.pane as usize])
                    .collect();
                let mut terminals = new_floating_terminal_ids.iter();
                let mut visible_layout = vec![];
                for (pane_layout, id) in
                    floating_panes_layout.into_iter().zip(&slots[tiled_slots..])
                {
                    let held_command = match id {
                        PaneId::Terminal(_) => terminals
                            .next()
                            .context("missing warp terminal slot")?
                            .1
                            .clone(),
                        PaneId::Plugin(_) => None,
                    };
                    if hidden_ids.contains(id) {
                        hidden_companions.push((*id, pane_layout, held_command));
                    } else {
                        visible_layout.push(pane_layout);
                    }
                }
                floating_panes_layout = visible_layout;
                new_floating_terminal_ids
                    .retain(|(id, _)| !hidden_ids.contains(&PaneId::Terminal(*id)));
                for ids in new_plugin_ids.values_mut() {
                    // Keep pop order for repeated configurations across tiled and floating slots.
                    ids.retain(|id| !hidden_ids.contains(&PaneId::Plugin(*id)));
                }
            }

            // Reject the layout before moving clients away from their working tab.
            self.tabs
                .get_mut(&tab_id)
                .context("layout tab is missing")?
                .validate_layout(
                    &layout,
                    &floating_panes_layout,
                    &new_terminal_ids,
                    &new_floating_terminal_ids,
                    &new_plugin_ids,
                )
                .with_context(err_context)?;

            // Companions must exist before apply_layout replays buffered output. Create them
            // before moving clients too, since parking a companion can fail on its resize.
            let tab = self
                .tabs
                .get_mut(&tab_id)
                .context("layout tab is missing")?;
            for (id, pane_layout, held_command) in hidden_companions {
                tab.create_warp_companion(id, pane_layout, held_command)
                    .with_context(err_context)?;
            }

            let client_departed = self.departed_before_layout.contains(&client_id);
            let mut transition = (should_change_client_focus && !client_departed)
                .then(|| self.begin_tab_transition(client_id, tab_id));

            if !client_departed && !self.active_tab_ids.contains_key(&client_id) {
                self.connected_clients.borrow_mut().insert(client_id);
                self.show_toasts_held_for_the_first_client()
                    .with_context(err_context)?;
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
                        blocking_terminal.take(),
                    )?;
                    // The layout now owns its panes. Notification or reflow failures must not
                    // send them through rejected-layout cleanup and destroy a committed tab.
                    if let (Some(slots), Some(relationships)) = (
                        warp_slots.as_ref(),
                        self.pending_warp_relationships.get(&tab_id),
                    ) {
                        tab.restore_warp_relationships(slots, relationships)
                            .non_fatal();
                    }
                    Ok(())
                })
                .with_context(err_context)?;

            if let Some(transition) = &mut transition {
                self.finish_tab_transition(transition, None)
                    .with_context(err_context)
                    .non_fatal();
            }
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                // after the drained clients have joined, so the plugins in this tab hear
                // about their modes too and not only about the client that opened it
                tab.update_input_modes().non_fatal();
                // the tab keeps the size its viewers gave it here; `recompute_tab_size` below
                // reflows it against the session surface
                let tab_size = tab.size;
                tab.resize_whole_tab(tab_size)
                    .with_context(err_context)
                    .non_fatal();
                tab.set_force_render();
            }
            if client_departed && let Some(tab) = self.tabs.get_mut(&tab_id) {
                tab.remove_client_pane_state(client_id);
                tab.remove_client(client_id);
            }
            let mut retired_source = None;
            if !client_departed && !self.active_tab_ids.contains_key(&client_id) {
                // Capture the departed source before the layout provisionally connects this id.
                self.register_client_transaction(client_id, departed, None, false)
                    .map(|source| retired_source = source)
                    .with_context(err_context)
                    .non_fatal();
            }

            if let (Some(slots), Some(relationships)) =
                (warp_slots, self.pending_warp_relationships.remove(&tab_id))
            {
                self.finish_warp_group(tab_id, &slots, &relationships)
                    .non_fatal();
            }
            self.recompute_tab_size(tab_id)
                .with_context(err_context)
                .non_fatal();

            for (affected_client_id, old_pane_id) in
                transition.into_iter().flat_map(|t| t.old_focus)
            {
                let new_pane_id = self.get_active_pane_id(&affected_client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    self.report_key_passthrough_state(affected_client_id, old, new);
                }
            }

            // Populate chrome-only destinations before rendering can reap empty tabs.
            self.resume_pending_arrangement().non_fatal();
            self.log_and_report_session_state()
                .and_then(|_| self.render())
                .with_context(err_context)
                .non_fatal();
            Ok(retired_source)
        })();
        if !self.has_pending_tabs() {
            self.departed_before_layout.clear();
        }
        if let Err(error) = &applied
            && let Some((_, completion)) = blocking_terminal.as_mut()
        {
            completion.set_exit_status(1);
            completion.set_error_message(format!("{error:#}"));
        }
        if applied.is_ok() {
            self.finish_group_restore();
        }
        applied
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
                let finish_empty_rename = s.is_empty()
                    && self
                        .mode_info
                        .get(&client_id)
                        .unwrap_or(&self.default_mode_info)
                        .mode
                        != InputMode::RenameTab;
                match self.get_active_tab_mut(client_id) {
                    Ok(active_tab) => {
                        active_tab.edit_name_by_user(|name| match s {
                            "\0" => {
                                name.clear();
                            },
                            "\u{007F}" | "\u{0008}" => {
                                // delete and backspace keys
                                name.pop();
                            },
                            c => {
                                name.push_str(&clean_string_from_control_and_linebreak(c));
                            },
                        });
                        if finish_empty_rename {
                            active_tab.finish_rename();
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
                        if active_tab.undo_rename() {
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
        if self.tab_groups_enabled() && self.tab_groups.len() > 1 {
            if let Some(client) = self.client_id(client_id)
                && let Some(tab) = self.active_tab_ids.get(&client).copied()
            {
                self.move_group_tab(tab, false)?;
            }
            return Ok(());
        }
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
                if active_tab_pos == TabPosition::new(0) {
                    self.rotate_tab_positions_left();
                } else {
                    self.switch_tabs(
                        active_tab_pos,
                        TabPosition::new(active_tab_pos.get().saturating_sub(1)),
                    );
                }
                self.log_and_report_session_state()
                    .context("failed to move tab to left")?;
            },
            Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
        }
        Ok(())
    }

    fn client_id(&mut self, client_id: ClientId) -> Option<ClientId> {
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
    fn switch_tabs(&mut self, active_tab_pos: TabPosition, other_tab_pos: TabPosition) {
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
        let last_position = TabPosition::new(self.tabs.len().saturating_sub(1));
        for tab in self.tabs.values_mut() {
            tab.position = if tab.position == TabPosition::new(0) {
                last_position
            } else {
                TabPosition::new(tab.position.get().saturating_sub(1))
            };
        }
    }

    fn rotate_tab_positions_right(&mut self) {
        let num_tabs = self.tabs.len();
        for tab in self.tabs.values_mut() {
            tab.position = TabPosition::new((tab.position.get() + 1) % num_tabs);
        }
    }

    pub fn move_active_tab_to_right(&mut self, client_id: ClientId) -> Result<()> {
        if self.tab_groups_enabled() && self.tab_groups.len() > 1 {
            if let Some(client) = self.client_id(client_id)
                && let Some(tab) = self.active_tab_ids.get(&client).copied()
            {
                self.move_group_tab(tab, true)?;
            }
            return Ok(());
        }
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
                if active_tab_pos.get() == self.tabs.len().saturating_sub(1) {
                    self.rotate_tab_positions_right();
                } else {
                    self.switch_tabs(active_tab_pos, TabPosition::new(active_tab_pos.get() + 1));
                }
                self.log_and_report_session_state()
                    .context("failed to move tab to the right")?;
            },
            Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
        }
        Ok(())
    }

    pub fn move_tab_by_id(&mut self, tab_id: TabId, direction: Direction) -> Result<()> {
        if self.tab_groups_enabled()
            && self.tab_groups.len() > 1
            && matches!(direction, Direction::Left | Direction::Right)
        {
            return self.move_group_tab(tab_id, direction == Direction::Right);
        }
        if self.tabs.len() < 2 {
            return Ok(());
        }
        if let Some(tab) = self.tabs.get(&tab_id) {
            let tab_pos = tab.position;
            let num_tabs = self.tabs.len();
            match direction {
                Direction::Left => {
                    if tab_pos == TabPosition::new(0) {
                        self.rotate_tab_positions_left();
                    } else {
                        self.switch_tabs(
                            tab_pos,
                            TabPosition::new(tab_pos.get().saturating_sub(1)),
                        );
                    }
                },
                Direction::Right => {
                    if tab_pos.get() == num_tabs.saturating_sub(1) {
                        self.rotate_tab_positions_right();
                    } else {
                        self.switch_tabs(tab_pos, TabPosition::new(tab_pos.get() + 1));
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

    /// Move the client's focus one pane in `direction`, escalating to the host session when the
    /// tab reports the focus stayed put. An overlay holding the keyboard is checked once the tab
    /// has had its turn - a multipane's halves may trade the keyboard, and a dismissible overlay
    /// gives it up to a pane the move lands on - and a move that went nowhere under it is a
    /// no-op, never a hop to the host, as `move_focus_{left_or_previous,right_or_next}_tab` treat
    /// the same move. The modal test stays beside the overlay one: `Tab::overlay_holds_focus`
    /// counts an overlay only while one is drawn, and a modal owns the keyboard regardless.
    pub(crate) fn move_focus_in_direction_or_escalate(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> Result<()> {
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
        if old_pane_id == new_pane_id
            && (self.active_tab_modal_holds_focus(client_id)
                || self.active_tab_overlay_holds_focus(client_id))
        {
            return self.log_and_report_session_state();
        }
        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
            if old == new {
                self.bubble_focus_to_host(client_id, direction);
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
        self.render()?;
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
            // A move that went nowhere while an overlay holds the keyboard is a no-op, not a hop
            // to another tab or the host session: a modal owns the keyboard, and a dismissible
            // overlay is what the user is looking at - agent would carry its panel into the new
            // tab. Across a multipane's divider the tab has already moved
            // (`Tab::move_focus_to_overlay_sibling`).
            if moved == Some(false) && !self.active_tab_overlay_holds_focus(client_id) {
                if self.navigation_tabs(self.active_tab_ids[&client_id]).len() == 1 {
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
            // A move that went nowhere while an overlay holds the keyboard is a no-op, not a hop
            // to another tab or the host session: a modal owns the keyboard, and a dismissible
            // overlay is what the user is looking at - agent would carry its panel into the new
            // tab. Across a multipane's divider the tab has already moved
            // (`Tab::move_focus_to_overlay_sibling`).
            if moved == Some(false) && !self.active_tab_overlay_holds_focus(client_id) {
                if self.navigation_tabs(self.active_tab_ids[&client_id]).len() == 1 {
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
            self.navigate_to_tab(position, None, true, client_id)
                .context("failed to toggle tabs")?;
        };

        self.log_and_report_session_state()
            .context("failed to toggle tabs")?;
        self.render()
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
        let mut tab_id_and_plugin_pane_id = None;
        let mut plugin_pane_to_move_to_active_tab = None;
        let mut overlay_to_move = None;
        // A tab *id*, as `self.tabs` and `active_tab_ids` are both keyed - never a position. The
        // two part company as soon as a tab is closed or moved, and reading one as the other is
        // what used to walk the client off its own tab on the way to a reveal.
        //
        // Resolved, and checked against `self.tabs`, *before* anything is extracted: a pane taken
        // out of its tab with nowhere to put it is dropped on the `?` below, and a dropped plugin
        // pane never reaches `PluginInstruction::Unload` - the instance outlives the pane with
        // nothing left holding it. Production cannot get here without an active tab
        // (`resolve_tab_and_client` only hands over a client that has one), but that invariant is
        // worth keeping next to the extraction rather than one call frame away.
        let focused_tab_id = self
            .active_tab_ids
            .get(&client_id)
            .copied()
            .filter(|tab_id| self.tabs.contains_key(tab_id));
        let can_replace_active_pane = focused_tab_id
            .and_then(|tab_id| self.tabs.get(&tab_id))
            .is_some_and(|tab| tab.can_replace_active_pane(client_id));
        let all_tabs = self.get_tabs_mut();
        for (tab_id, tab) in all_tabs.iter_mut() {
            if let Some(plugin_pane_id) = tab.find_plugin(run_plugin) {
                tab_id_and_plugin_pane_id = Some((*tab_id, plugin_pane_id));
                if move_to_focused_tab && focused_tab_id.is_some_and(|focused| focused != *tab_id) {
                    if tab.pane_is_overlay(plugin_pane_id) {
                        overlay_to_move = Some(plugin_pane_id);
                        break;
                    }
                    if !should_float && should_be_in_place && !can_replace_active_pane {
                        return Err(anyhow!("no active pane to replace for client {client_id}"));
                    }
                    plugin_pane_to_move_to_active_tab = tab.extract_pane(plugin_pane_id, true);
                }

                break;
            }
        }
        if let Some(plugin_pane_id) = overlay_to_move {
            let destination = focused_tab_id.with_context(err_context)?;
            self.break_multiple_panes_to_tab_with_id(
                vec![plugin_pane_id],
                destination,
                false,
                client_id,
            )?;
            let tab = self.tabs.get_mut(&destination).with_context(err_context)?;
            tab.show_overlay_pane(plugin_pane_id);
            tab_id_and_plugin_pane_id = Some((destination, plugin_pane_id));
        }
        if let Some(plugin_pane_to_move_to_active_tab) = plugin_pane_to_move_to_active_tab.take() {
            let pane_id = plugin_pane_to_move_to_active_tab.pid();
            let new_active_tab = self.get_active_tab_mut(client_id)?;

            if should_float {
                new_active_tab.show_floating_panes();
                new_active_tab.add_floating_pane(
                    plugin_pane_to_move_to_active_tab,
                    pane_id,
                    None,
                    true,
                    Some(client_id),
                )?;
            } else if should_be_in_place {
                new_active_tab.park_pane(plugin_pane_to_move_to_active_tab);
                new_active_tab.focus_pane_with_id(pane_id, false, true, client_id)?;
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
            if let Some(completion) = completion_tx {
                completion.set_affected_pane_id(pane_id);
            }
            return Ok(true);
        }
        match tab_id_and_plugin_pane_id {
            Some((tab_id, plugin_pane_id)) => {
                // `tab_id` is a `self.tabs` key - a stable id - and `go_to_tab` takes a 1-based
                // display *position*. The two agree only until the first tab is closed or moved:
                // `close_tab_by_id` renumbers positions and leaves ids alone. Handing the id over
                // as a position took the client to whichever tab happened to sit at that number
                // and then revealed the plugin on the tab it had just left - "revealing agent
                // just switches tabs" - and did nothing at all when the id ran past the end of
                // the tab bar, which is why it only misbehaved sometimes.
                //
                // Converting is also what makes the already-focused case free, and it is the
                // common one: every `LaunchOrFocusPlugin { move_to_focused_tab }` that found its
                // pane in the client's own tab arrives here. `switch_active_tab` returns early
                // when handed the position the client is already on, so a reveal in place needs
                // no branch of its own - and cannot grow one that drifts from this.
                let tab_position = self
                    .get_tab_position_by_id(tab_id)
                    .with_context(err_context)?;
                self.switch_active_tab(tab_position, None, true, client_id)?;
                self.tabs
                    .get_mut(&tab_id)
                    .with_context(err_context)?
                    .focus_pane_with_id(plugin_pane_id, should_float, should_be_in_place, client_id)
                    .context("failed to focus plugin pane")?;
                self.log_and_report_session_state()
                    .with_context(err_context)?;
                // Set affected pane ID for CLI client output
                if let Some(completion) = completion_tx {
                    completion.set_affected_pane_id(plugin_pane_id);
                }
                Ok(true)
            },
            None => Ok(false),
        }
    }

    /// Run `f` against `pane_id` wherever it lives.
    pub(super) fn with_pane<R>(
        &mut self,
        pane_id: PaneId,
        f: impl FnOnce(&mut dyn Pane) -> R,
    ) -> Option<R> {
        for tab in self.tabs.values_mut() {
            if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                return Some(f(pane.as_mut()));
            }
        }
        None
    }

    /// Whether a modal in the client's active tab holds the keyboard. A directional focus move
    /// is then a no-op - never an escalation to another tab or the host session.
    pub(super) fn active_tab_modal_holds_focus(&self, client_id: ClientId) -> bool {
        self.get_active_tab(client_id)
            .map(|tab| !tab.lower_layer_may_take_focus(client_id))
            .unwrap_or(false)
    }

    /// Whether any overlay in the client's active tab holds the keyboard. A focus move that went
    /// nowhere is then not a reason to switch tabs.
    pub(super) fn active_tab_overlay_holds_focus(&self, client_id: ClientId) -> bool {
        self.get_active_tab(client_id)
            .map(|tab| tab.overlay_holds_focus(client_id))
            .unwrap_or(false)
    }

    pub(super) fn active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane_id(client_id))
    }

    pub(super) fn cycle_focus(&mut self, client_id: ClientId, forward: bool) {
        let Some(client_id) = self.client_to_act_for(client_id) else {
            log::error!("No client ids in screen found");
            return;
        };
        if let Ok(tab) = self.get_active_tab_mut(client_id) {
            if forward {
                tab.focus_next_pane(client_id);
            } else {
                tab.focus_previous_pane(client_id);
            }
        }
    }

    /// Repaint the tab an overlay is closing on: an overlay is drawn over the tab's own panes, so
    /// the cells it vacates have no other writer.
    pub(super) fn repaint_tab_of_closing_overlay(&mut self, pane_id: PaneId) {
        let Some(tab_id) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.pane_is_overlay(pane_id))
            .map(|(tab_id, _)| *tab_id)
        else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.set_should_clear_display_before_rendering();
        }
    }

    pub fn focus_pane_with_id(
        &mut self,
        pane_id: PaneId,
        should_float_if_hidden: bool,
        should_open_in_place: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || format!("failed to focus pane {pane_id:?}");
        let tab_index = self.tab_position_containing_pane(pane_id);
        match tab_index {
            Some(tab_index) => {
                self.switch_active_tab(tab_index, None, true, client_id)?;
                // The tab's own result is what gets logged. Mapping over the lookup used to
                // produce an `Option<Result>` whose *outer* layer was the only thing
                // `non_fatal` ever looked at, so a tab that failed to focus the pane was
                // reported as success.
                match self.tabs.iter_mut().find(|(_, t)| t.position == tab_index) {
                    Some((_, tab)) => tab
                        .focus_pane_with_id(
                            pane_id,
                            should_float_if_hidden,
                            should_open_in_place,
                            client_id,
                        )
                        .with_context(err_context)
                        .non_fatal(),
                    None => log::error!(
                        "Tab at position {tab_index} containing pane {pane_id:?} disappeared before it could be focused"
                    ),
                }
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
    /// state. So the mark is applied where the pane stands and the view is left
    /// alone, which is worth more than a reveal that costs the client its place.
    pub fn reveal_pane_with_id(
        &mut self,
        pane_id: PaneId,
        label: Option<String>,
        client_id: ClientId,
    ) -> Result<()> {
        let Some(tab_id) = self.tab_containing_pane(pane_id).map(|tab| tab.id) else {
            log::error!("Could not find pane with id: {:?}", pane_id);
            return Ok(());
        };

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.add_highlight_pane_frame_color_override(pane_id, label, Some(client_id));
        }

        // Deliberately not a focus operation: a pane in another tab is only revealed once the
        // client is looking at that tab, and nothing here moves the keyboard.
        if self.active_tab_ids.get(&client_id) != Some(&tab_id) {
            self.render()?;
            self.log_and_report_session_state()?;
            return Ok(());
        }

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            // Suppressed, or floating while floating panes are hidden: in the
            // right tab and still not on screen. Unlike the tab-level
            // `focus_pane_with_id` this leaves the tab's own active pane alone,
            // so nothing about where the keyboard goes changes.
            tab.unsuppress_or_expand_pane(pane_id, true);
        }
        self.render()?;
        self.log_and_report_session_state()?;
        Ok(())
    }
    /// Resolve the acting client once so pane destruction and cleanup use the same identity.
    pub(crate) fn close_focused_pane(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<Option<PaneId>> {
        let Some(client_id) = self.client_to_act_for(client_id) else {
            log::error!("No client ids in screen found");
            return Ok(None);
        };
        let closing = self.get_active_pane_id(&client_id);
        if let Some(id) = closing {
            self.close_pane_explicitly(id, completion_tx);
        }
        Ok(closing)
    }

    fn close_self_instruction(&self, plugin: PluginId, client: ClientId) -> ScreenInstruction {
        let id = PaneId::Plugin(plugin);
        let composed = self.tab_containing_pane(id).is_some_and(|tab| {
            tab.multipane_info(id).is_some()
                && tab.stands_in_for_a_pane(id).is_none()
                && tab.pane_tabs_of(id).is_none()
        });
        ScreenInstruction::ClosePane(id, composed.then_some(client), None, None)
    }

    pub(crate) fn cleanup_closed_pane(&mut self, id: PaneId) {
        self.forget_notification_assemblies(id);
        self.retire_agent_event_pane(id, "pane_closed");
        self.destroyed_panes.insert(id);
        self.invalidate_agent_lifecycle(id, "pane_closed");
        self.agent_launches.remove(&id);
        self.close_context_menus_for_pane(id);
        if let PaneId::Plugin(plugin_id) = id {
            self.drop_quit_notice(plugin_id);
        }
        self.clear_nested_guest(id);
        self.pane_output_activity.remove(&id);
    }

    pub(crate) fn close_pane_explicitly(
        &mut self,
        id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) {
        self.repaint_tab_of_closing_overlay(id);
        let closed = self
            .tab_containing_pane_mut(id)
            .map(|tab| tab.close_pane_explicitly(id, completion_tx))
            .unwrap_or_default();
        for id in closed {
            self.cleanup_closed_pane(id);
        }
        self.retain_only_existing_panes_in_pane_groups();
    }
    /// Record what a plugin wants said, and decided, about a quit. `None`/`false` withdraws.
    pub(crate) fn set_quit_notice(
        &mut self,
        plugin_id: PluginId,
        notice: Option<String>,
        guard: bool,
    ) {
        let entry = QuitNotice { notice, guard };
        match entry.is_empty() {
            true => {
                self.quit_notices.remove(&plugin_id);
            },
            false => {
                self.quit_notices.insert(plugin_id, entry);
            },
        }
    }

    /// Forget what a plugin said about a quit, because it unloaded or its pane has gone.
    ///
    /// Without this a plugin that crashed or was closed mid-reply would leave a guard behind
    /// that nothing could lift, and the session could never close its last pane again.
    pub(crate) fn drop_quit_notice(&mut self, plugin_id: PluginId) {
        self.quit_notices.remove(&plugin_id);
    }

    /// Whether any plugin has asked for the session's last pane closing to be confirmed.
    pub(crate) fn quit_is_guarded(&self) -> bool {
        self.quit_notices.values().any(|notice| notice.guard)
    }

    /// Everything the plugins asked to have said about a quit, one per line, in plugin id order
    /// so the same set of notices always reads the same way.
    fn quit_notice_text(&self) -> Option<String> {
        let lines: Vec<&str> = self
            .quit_notices
            .values()
            .filter_map(|notice| notice.notice.as_deref())
            .collect();
        match lines.is_empty() {
            true => None,
            false => Some(lines.join("\n")),
        }
    }

    /// Raise the quit confirmation as a modal, carrying whatever the plugins asked to have said.
    ///
    /// The one place the confirmation is built: `Ctrl-q` reaches it through
    /// `ScreenInstruction::ConfirmQuit`, and the last pane closing reaches it from the render
    /// sweep. The plugin answers by closing itself and sending `Quit`, which carries a pane id
    /// and so falls through `route.rs` to the immediate exit rather than reopening this.
    ///
    /// The configuration is left empty when no plugin had anything to say, so a plain quit is
    /// the same instruction it has always been.
    pub(crate) fn confirm_quit(
        &mut self,
        client_id: ClientId,
        on_last_pane: bool,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to confirm quit for client {client_id}");
        let mut configuration = BTreeMap::new();
        if let Some(notice) = self.quit_notice_text() {
            configuration.insert("notice".to_owned(), notice);
        }
        if on_last_pane {
            configuration.insert("reason".to_owned(), "last_pane".to_owned());
        }
        let plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::Yinz(PluginTag::new("quit-confirmation")),
            configuration: PluginUserConfiguration::new(configuration),
            ..Default::default()
        });
        self.bus
            .senders
            .send_to_screen(ScreenInstruction::LaunchPlugin(
                plugin,
                true,
                false,
                false,
                None,
                false,
                None,
                client_id,
                completion,
                None,
                false,
                // the one pane the user has to answer: stated here rather than spelled as a
                // `modal` key in the plugin's configuration for the server to read back out
                Some(OverlayPlacement::modal(None)),
            ))
            .with_context(err_context)
    }

    pub fn rerun_command_pane_with_id(
        &mut self,
        terminal_pane_id: TerminalId,
        completion_tx: Option<NotificationEnd>,
    ) {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        self.refresh_agent_events();
        // Read around the re-run rather than inside it: the pane resets its own record to
        // `busy` when it restarts, and an exited agent coming back is an `agent_start`.
        let previous = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id));
        let accepted = match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => tab.rerun_terminal_pane_with_id(terminal_pane_id, completion_tx),
            None => {
                log::error!(
                    "Failed to find terminal pane with id: {} to run",
                    terminal_pane_id
                );
                return;
            },
        };
        if !accepted {
            return;
        }
        // Retire the cached identity; refreshing after respawn would read the new agent.
        self.retire_agent_event_pane(pane_id, "pane_replaced");
        self.invalidate_agent_lifecycle(pane_id, "rerun");
        let current = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id));
        self.fire_agent_state_hooks(pane_id, previous.as_ref(), current.as_ref());
    }
    pub(super) fn restart_agent_pane(
        &mut self,
        pane_id: PaneId,
        prompt: Option<&str>,
        session: yinz_types::agent::control::AgentRestartSession,
        originating_plugin: Option<yinz_types::data::OriginatingPlugin>,
    ) -> Result<yinz_types::agent::control::RestartedAgent, String> {
        use yinz_types::agent::{
            control::AgentRestartSession, control::RestartedAgent, AgentSource,
        };
        let PaneId::Terminal(terminal_id) = pane_id else {
            return Err("target must be a terminal pane".into());
        };
        let pane = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.get_pane_with_id(pane_id))
            .ok_or("target pane does not exist")?;
        let mut command = pane.agent_restart_command(session, prompt)?;
        let kind = pane
            .last_agent_kind()
            .ok_or("target has no remembered agent kind")?
            .to_owned();
        let cwd = command
            .cwd
            .as_ref()
            .map(|cwd| cwd.display().to_string())
            .or_else(|| pane.last_agent_cwd().map(str::to_owned));
        let session_id = (session == AgentRestartSession::Resume)
            .then(|| pane.last_agent_session_id().map(str::to_owned))
            .flatten();
        let previous = pane.agent_info().cloned();
        if let Some(origin) = originating_plugin {
            command.originating_plugin = Some(origin);
        }
        command.hold_on_close = true;
        let result = RestartedAgent {
            pane_id,
            session_id: session_id.clone(),
            command: std::iter::once(command.command.display().to_string())
                .chain(command.args.iter().cloned())
                .collect(),
        };
        self.refresh_agent_events();
        let tab = self
            .tab_containing_pane_mut(pane_id)
            .ok_or("target pane does not exist")?;
        if !tab.run_command_in_pane_with_id(terminal_id, command, false) {
            return Err("agent pane could not accept the restart".into());
        }
        if session == AgentRestartSession::Fresh
            && let Some(pane) = tab.get_pane_with_id_mut(pane_id)
        {
            pane.clear_agent_session_id();
        }
        let current = AgentInfo {
            kind,
            cwd,
            session_id,
            state: AgentState::Busy,
            source: AgentSource::Spawned,
            detail: None,
            waiting_on: None,
            run_id: None,
            wait_needs_a_person: false,
        };
        tab.set_pane_agent(pane_id, Some(current.clone()));
        self.retire_agent_event_pane(pane_id, "pane_replaced");
        self.invalidate_agent_lifecycle(pane_id, "agent_restarted");
        self.fire_agent_state_hooks(pane_id, previous.as_ref(), Some(&current));
        Ok(result)
    }
    /// Run a *different* command in an existing command pane, reusing its pane id.
    pub fn run_command_in_pane_with_id(
        &mut self,
        terminal_pane_id: TerminalId,
        run_command: RunCommand,
        keep_scrollback: bool,
    ) {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        self.refresh_agent_events();
        // Read around the re-run rather than inside it: the pane resets its own record to
        // `busy` when it restarts, and an exited agent coming back is an `agent_start`.
        let previous = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id));
        let accepted = match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => {
                tab.run_command_in_pane_with_id(terminal_pane_id, run_command, keep_scrollback)
            },
            None => {
                log::error!(
                    "Failed to find terminal pane with id: {} to run a command in",
                    terminal_pane_id
                );
                return;
            },
        };
        if !accepted {
            return;
        }
        // Retire the cached identity; refreshing after respawn would read the new agent.
        self.retire_agent_event_pane(pane_id, "pane_replaced");
        self.invalidate_agent_lifecycle(pane_id, "command_replaced");
        let current = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_agent_info(pane_id));
        self.fire_agent_state_hooks(pane_id, previous.as_ref(), current.as_ref());
    }
    pub fn resize_pane_with_id(&mut self, resize: ResizeStrategy, pane_id: PaneId) -> Result<()> {
        match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => tab.resize_pane_with_id(resize, pane_id).non_fatal(),
            None => log::error!("Failed to find pane with id: {:?} to resize", pane_id),
        }
        Ok(())
    }
    /// Open `new_pane_id` in place of the pane `client_id_tab_index_or_pane_id` names.
    ///
    /// Returns whether it was placed. A pane that could not be - its target is missing,
    /// suppressed or names no pane - is forgotten, which closes its process or unloads its
    /// plugin, and `completion_tx` carries the reason.
    pub fn replace_pane(
        &mut self,
        new_pane_id: PaneId,
        hold_for_command: HoldForCommand,
        run: Option<Run>,
        pane_title: Option<InitialTitle>,
        replaced: ReplacedPane,
        client_id_tab_index_or_pane_id: ClientTabIndexOrPaneId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> bool {
        let place = |tab: &mut Tab, pane_id: PaneId| -> Result<()> {
            tab.suppress_pane_and_replace_with_pid(
                pane_id,
                new_pane_id,
                replaced,
                run,
                None,
                None,
            )?;
            if let Some(pane_title) = pane_title {
                let _ = tab.rename_pane(pane_title.as_bytes().to_vec(), new_pane_id);
            }
            if let Some(hold_for_command) = hold_for_command {
                let is_first_run = true;
                let _ = tab.hold_pane(new_pane_id, None, is_first_run, hold_for_command);
            }
            Ok(())
        };
        let placed = match client_id_tab_index_or_pane_id {
            ClientTabIndexOrPaneId::ClientId(client_id)
            | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => self
                .get_active_tab_mut(client_id)
                .and_then(|tab| match tab.get_active_pane_id(client_id) {
                    Some(pane_id) => place(tab, pane_id),
                    None => Err(anyhow!("no active pane for client {client_id}")),
                }),
            // No client fallback here on purpose: replacing a pane the caller does not have
            // is not a placement question, it is a request with no subject.
            ClientTabIndexOrPaneId::PaneId(pane_id)
            | ClientTabIndexOrPaneId::PaneIdOrClient(pane_id, _) => {
                match self.tab_containing_pane_mut(pane_id) {
                    Some(tab) => place(tab, pane_id),
                    None => Err(anyhow!("pane {pane_id:?} to replace not found")),
                }
            },
            ClientTabIndexOrPaneId::TabIndex(_tab_index)
            | ClientTabIndexOrPaneId::TabIndexNoFocus(_tab_index)
            | ClientTabIndexOrPaneId::TabIndexForClient(_tab_index, _) => {
                Err(anyhow!("cannot replace a pane by tab index"))
            },
        };
        match placed {
            Ok(()) => true,
            Err(error) => {
                let error = format!("{error:#}");
                if let Some(completion) = completion_tx.as_mut() {
                    // A caller may have named the pane before placing it; it is gone now.
                    completion.clear_affected_pane_id();
                    completion.set_exit_status(1);
                    completion.set_error_message(error.clone());
                }
                self.forget_unplaced_pane(new_pane_id, error);
                false
            },
        }
    }
    /// Settle a [`ClientTabIndexOrPaneId::PaneIdOrClient`] against the tabs there are now.
    ///
    /// "Beside that pane, or wherever this client is looking" is what a plugin means by "near
    /// me", and the second half is the half that matters: a plugin loaded while no client was
    /// attached has no pane of its own yet - its `AddPlugin` is still parked in
    /// `pending_events_waiting_for_client` - and without a fallback the pane it opened would
    /// find no tab, be forgotten, and have the process the pty thread already started closed
    /// behind its back. `ClientIdNoFocus` rather than `ClientId` because the `PaneId` placement
    /// this stands in for does not steal focus either.
    ///
    /// Every other spelling passes through untouched.
    pub fn resolve_pane_placement(&self, target: ClientTabIndexOrPaneId) -> ClientTabIndexOrPaneId {
        match target {
            ClientTabIndexOrPaneId::PaneIdOrClient(pane_id, client_id) => {
                if self.tab_containing_pane(pane_id).is_some() {
                    ClientTabIndexOrPaneId::PaneId(pane_id)
                } else {
                    ClientTabIndexOrPaneId::ClientIdNoFocus(client_id)
                }
            },
            other => other,
        }
    }
    /// A pane the pty thread already started has nowhere to go: the tab or
    /// pane it was meant for closed while it was being spawned, which for a
    /// remote pane can take a long time. Nothing owns it now, so the pty
    /// thread is asked to close it; otherwise its process, transport and reader
    /// would run until the session ends.
    pub fn forget_unplaced_pane(&mut self, pane_id: PaneId, why: String) {
        self.cleanup_closed_pane(pane_id);
        log::error!("dropping pane {pane_id:?}: {why}");
        self.bus
            .senders
            .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
            .with_context(|| format!("failed to close unplaced pane {pane_id:?}"))
            .non_fatal();
    }
    /// Move an existing pane into `pane_id_to_replace`'s slot, suppressing or closing the pane
    /// that was there.
    ///
    /// Refused, with nothing moved, when the target holds no slot - it is suppressed or gone -
    /// or is the pane being moved: the collections consume the pane they are handed, so a
    /// replacement with nowhere to go would be dropped with its process still running.
    pub fn replace_pane_with_existing_pane(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_id_of_existing_pane: PaneId,
        suppress_replaced_pane: bool,
        mut completion_tx: Option<NotificationEnd>, // ends here
    ) -> Result<()> {
        let replaced = self.move_existing_pane_into_slot(
            pane_id_to_replace,
            pane_id_of_existing_pane,
            suppress_replaced_pane,
        );
        if let Err(error) = &replaced
            && let Some(completion) = completion_tx.as_mut()
        {
            completion.set_exit_status(1);
            completion.set_error_message(format!("{error:#}"));
        }
        let _ = self.log_and_report_session_state();
        replaced
    }
    fn move_existing_pane_into_slot(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_id_of_existing_pane: PaneId,
        suppress_replaced_pane: bool,
    ) -> Result<()> {
        if pane_id_to_replace == pane_id_of_existing_pane {
            bail!("cannot replace pane {pane_id_to_replace:?} with itself");
        }
        let tab_of_pane_to_replace = self
            .tab_id_containing_pane(pane_id_to_replace)
            .with_context(|| format!("pane {pane_id_to_replace:?} to replace not found"))?;
        let tab_of_existing_pane = self
            .tab_id_containing_pane(pane_id_of_existing_pane)
            .with_context(|| {
                format!("pane {pane_id_of_existing_pane:?} to replace it with not found")
            })?;
        {
            let tab = self
                .tabs
                .get(&tab_of_pane_to_replace)
                .context("tab of the pane to replace disappeared")?;
            if !tab.has_non_suppressed_pane_with_pid(&pane_id_to_replace) {
                return Err(tab.no_slot_error(pane_id_to_replace));
            }
        }
        let (extracted_pane_from_other_tab, carried_respawn, retains_tab) = self
            .tabs
            .get_mut(&tab_of_existing_pane)
            .and_then(|t| {
                let retains_tab = t
                    .get_tiled_panes()
                    .chain(t.get_floating_panes())
                    .any(|(id, _)| *id == pane_id_of_existing_pane);
                let carried_respawn = t.take_pending_respawn(pane_id_of_existing_pane);
                t.extract_pane(pane_id_of_existing_pane, true)
                    .map(|pane| (pane, carried_respawn, retains_tab))
            })
            .with_context(|| format!("failed to take pane {pane_id_of_existing_pane:?}"))?;
        let tab = self
            .tabs
            .get_mut(&tab_of_pane_to_replace)
            .context("tab of the pane to replace disappeared")?;
        // Taking the pane can take its whole container with it, the target included.
        if !tab.has_non_suppressed_pane_with_pid(&pane_id_to_replace) {
            let error = tab.no_slot_error(pane_id_to_replace);
            if let Some(origin) = self.tabs.get_mut(&tab_of_existing_pane) {
                origin.park_pane(extracted_pane_from_other_tab);
                if let Some(entry) = carried_respawn {
                    origin.restore_pending_respawn(pane_id_of_existing_pane, entry);
                }
            }
            return Err(error.context("moving the pane took the target with it; it is parked"));
        }
        if suppress_replaced_pane {
            tab.suppress_pane_and_replace_with_other_pane(
                pane_id_to_replace,
                extracted_pane_from_other_tab,
                None,
            );
        } else {
            tab.close_pane_and_replace_with_other_pane(
                pane_id_to_replace,
                extracted_pane_from_other_tab,
                None,
            );
        }
        if retains_tab {
            tab.retain_tab_for_multipane(pane_id_of_existing_pane);
        }
        if let Some(entry) = carried_respawn {
            tab.restore_pending_respawn(pane_id_of_existing_pane, entry);
        }
        if !suppress_replaced_pane
            && self.tab_containing_pane(pane_id_to_replace).is_none()
            && let PaneId::Plugin(plugin_id) = pane_id_to_replace
        {
            self.drop_quit_notice(plugin_id);
        }
        Ok(())
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
        if self.selection_has_multipane(&pane_ids_to_stack) {
            return None;
        }
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

        if root_pane_id_is_floating && let Some(tab) = self.tabs.get_mut(&root_tab_id) {
            let _ = tab.toggle_pane_embed_or_floating_for_pane_id(root_pane_id, None);
        }

        let mut panes_to_stack = vec![];
        let mut pending_respawns_to_restore: Vec<(PaneId, (RunCommand, bool))> = vec![];
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
                if tab.has_pane_with_pid(pane_id) {
                    let carried_respawn = tab.take_pending_respawn(*pane_id);
                    match tab.extract_pane(*pane_id, false) {
                        Some(pane) => {
                            panes_to_stack.push(pane);
                            if let Some(entry) = carried_respawn {
                                pending_respawns_to_restore.push((*pane_id, entry));
                            }
                        },
                        None => {
                            log::error!("Failed to extract pane: {:?}", pane_id);
                            if let Some(entry) = carried_respawn {
                                // extraction failed after all - put it back where it came from
                                tab.restore_pending_respawn(*pane_id, entry);
                            }
                        },
                    }
                }
            }
        }
        if let Some(t) = self.tabs.get_mut(&root_tab_id) {
            t.stack_panes(root_pane_id, panes_to_stack)
        }
        if let Some(dest) = self.tabs.get_mut(&root_tab_id) {
            for (pane_id, entry) in pending_respawns_to_restore {
                dest.restore_pending_respawn(pane_id, entry);
            }
        }
        last_pane_id.copied()
    }
    /// Move or resize overlays where they already are, in the overlay collection.
    pub fn change_overlay_coordinates(
        &mut self,
        pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
        override_user_placement: bool,
    ) -> Vec<(PaneId, FloatingPaneCoordinates)> {
        if let Err(error) = self.validate_multipane_coordinates(&pane_ids_and_coordinates) {
            Err::<(), _>(error).non_fatal();
            return vec![];
        }
        let mut pending = vec![];
        let mut per_tab: BTreeMap<TabId, Vec<(PaneId, FloatingPaneCoordinates)>> = BTreeMap::new();
        for (pane_id, coordinates) in pane_ids_and_coordinates {
            if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                per_tab
                    .entry(tab.id)
                    .or_default()
                    .push((pane_id, coordinates));
            } else {
                pending.push((pane_id, coordinates));
            }
        }
        for (tab_id, batch) in per_tab {
            if let Some(tab) = self.get_tab_by_id_mut(tab_id) {
                tab.change_overlay_coordinates_batch(batch, override_user_placement)
                    .non_fatal();
            }
        }
        pending
    }
    fn validate_multipane_coordinates(
        &self,
        coordinates: &[(PaneId, FloatingPaneCoordinates)],
    ) -> Result<()> {
        let mut placements = std::collections::HashMap::new();
        for (pane, coordinates) in coordinates {
            if let Some(info) = self
                .tab_containing_pane(*pane)
                .and_then(|tab| tab.multipane_info(*pane))
                && placements
                    .insert(info.id, coordinates)
                    .is_some_and(|previous| previous != coordinates)
            {
                return Err(anyhow!("conflicting placements for the same multipane"));
            }
        }
        Ok(())
    }
    pub fn change_floating_panes_coordinates(
        &mut self,
        pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    ) {
        if let Err(error) = self.validate_multipane_coordinates(&pane_ids_and_coordinates) {
            Err::<(), _>(error).non_fatal();
            return;
        }
        let mut applied = std::collections::HashSet::new();
        for (pane_id, coordinates) in pane_ids_and_coordinates {
            if let Some(info) = self
                .tab_containing_pane(pane_id)
                .and_then(|tab| tab.multipane_info(pane_id))
                && !applied.insert(info.id)
            {
                continue;
            }
            if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                tab.change_floating_pane_coordinates(&pane_id, coordinates)
                    .non_fatal();
            }
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
    /// Record which machine a pane runs on. Usually this arrives once when a pane is created
    /// and once more when it is closed, but a workflow step can respawn a pane's command on a
    /// different remote host, so it can also arrive again mid-life with a changed - or
    /// newly-absent - host.
    ///
    /// Unlike `UpdatePaneCwd`, which pty only sends on a change, there is something to compare
    /// against here and not every arrival is one: a respawn re-reports the host the pane
    /// already had, and a local pane's close retracts a host it never announced. Reporting
    /// either would cost the session a whole-manifest broadcast to say nothing.
    ///
    pub fn set_pane_host(&mut self, terminal_id: TerminalId, host: Option<String>) -> Result<()> {
        self.with_pane(PaneId::Terminal(terminal_id), |pane| {
            pane.set_remote_host(host.clone())
        });
        let changed = match host {
            Some(host) => self.terminal_hosts.insert(terminal_id, host.clone()) != Some(host),
            None => self.terminal_hosts.remove(&terminal_id).is_some(),
        };
        if changed {
            self.render()?;
            self.log_and_report_session_state()?;
        }
        Ok(())
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
            let _ = self.log_and_report_session_state();
        }
        Ok(())
    }
    fn get_layout_metadata(
        &self,
        default_shell: Option<PathBuf>,
        tab_index: Option<TabId>,
    ) -> SessionLayoutMetadata {
        let mut session_layout_metadata = SessionLayoutMetadata::new(self.default_layout.clone());
        if let Some(default_shell) = default_shell {
            session_layout_metadata.update_default_shell(default_shell);
        }
        let first_client_id = self.get_first_client_id();
        let active_tab_index =
            first_client_id.and_then(|client_id| self.active_tab_ids.get(&client_id));

        // Filter tabs based on optional tab_index parameter. `self.tabs` is keyed by
        // creation id, so walk them by position instead: a dumped or resurrected
        // layout should keep the order the user put the tabs in, not the order they
        // opened them in.
        let mut tabs_to_process: Vec<_> = self
            .tabs
            .iter()
            .filter(|(idx, _)| tab_index.is_none_or(|target| **idx == target))
            .collect();
        tabs_to_process.sort_by_key(|(_, tab)| tab.position);

        for (tab_index, tab) in tabs_to_process {
            let tab_is_focused = active_tab_index == Some(tab_index);
            let (tiled_panes, floating_panes, hide_floating_panes) =
                self.tab_panes_for_layout(*tab_index, tab, self.serialize_pane_viewport, false);
            session_layout_metadata.add_tab(
                tab.name.clone(),
                tab_is_focused,
                hide_floating_panes,
                tab.is_user_named(),
                tiled_panes.into_iter().map(|(_, pane)| pane).collect(),
                floating_panes.into_iter().map(|(_, pane)| pane).collect(),
            );
            // Persist the fallback, so derived project names do not replace it on restore.
            let group = self.tab_group_ids.get(tab_index).and_then(|id| {
                self.tab_groups
                    .get(id)
                    .map(|group| yinz_types::tab_bar::TabGroupInfo {
                        id: *id,
                        name: group.name.clone(),
                    })
            });
            let focused = group.as_ref().is_some_and(|group| {
                first_client_id
                    .and_then(|client| self.preferred_group_tab(group.id, client, false))
                    .or_else(|| {
                        self.tab_groups
                            .get(&group.id)?
                            .preferred
                            .filter(|id| self.tabs.contains_key(id))
                    })
                    == Some(*tab_index)
            });
            session_layout_metadata.set_last_tab_group(group, focused);
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
        tab_index: TabId,
        tab: &Tab,
        with_contents: bool,
        for_warp: bool,
    ) -> (LayoutPanes, LayoutPanes, bool) {
        {
            let hide_floating_panes = !tab.are_floating_panes_visible();
            // keyed by the stand-in, which is what the loops below are walking:
            // a slot occupied by a scrollback editor serializes as the pane the
            // editor is editing
            let mut scrollback_editor_subjects: HashMap<PaneId, &dyn Pane> = HashMap::new();
            for (_pane_id, suppressed) in tab.get_suppressed_panes() {
                if let Some(stand_in) = suppressed.restored_by
                    && stand_in.is_scrollback_editor
                    && !for_warp
                {
                    scrollback_editor_subjects.insert(stand_in.pane_id, suppressed.pane.as_ref());
                }
            }

            let all_connected_clients: Vec<ClientId> = self
                .connected_clients
                .borrow()
                .iter()
                .copied()
                .filter(|c| self.active_tab_ids.get(c) == Some(&tab_index))
                .collect();

            let mut active_pane_ids: HashMap<ClientId, Option<PaneId>> = HashMap::new();
            for connected_client_id in &all_connected_clients {
                active_pane_ids.insert(
                    *connected_client_id,
                    tab.get_active_pane_id(*connected_client_id),
                );
            }

            let stack_list_geoms = if for_warp {
                HashMap::new()
            } else {
                tab.stack_list_serialization_geoms()
            };
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
                        None => (*pane_id, p.as_ref()),
                    };
                    (
                        pane_id,
                        p,
                        geom_override.unwrap_or_else(|| p.position_and_size()),
                    )
                })
                .chain(if for_warp {
                    vec![]
                } else {
                    tab.hidden_stack_list_members_for_serialization(false)
                })
                .map(|(pane_id, p, mut geom)| {
                    if p.is_builtin_tab_bar() && !geom.is_stacked() {
                        geom.rows.constraint = p.configured_rows().constraint;
                    }
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
                        )
                        .with_pane_tabs(p.pane_tabs()),
                    )
                })
                .collect();
            let floating_panes: Vec<(PaneId, PaneLayoutMetadata)> = if for_warp {
                vec![]
            } else {
                tab.hidden_stack_list_members_for_serialization(true)
            }
            .into_iter()
            .chain(tab.get_floating_panes().map(|(pane_id, p)| {
                // here we look to see if this pane triggers any suppressed pane,
                // and if so we take that suppressed pane - we do this because this
                // is currently only the case the scrollback editing panes, and
                // when dumping the layout we want the "real" pane and not the
                // editor pane
                match scrollback_editor_subjects.remove(pane_id) {
                    Some(subject) => (subject.pid(), subject, p.position_and_size()),
                    None => (*pane_id, p.as_ref(), p.position_and_size()),
                }
            }))
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
                    )
                    .with_pane_tabs(p.pane_tabs()),
                )
            })
            .collect();
            (tiled_panes, floating_panes, hide_floating_panes)
        }
    }
    fn accepts_plugin_output(&self, client_id: ClientId) -> bool {
        self.connected_clients.borrow().contains(&client_id)
            || self.client_sizes.contains_key(&client_id)
            || self.followed_client_id == Some(client_id)
    }

    fn handle_plugin_render_assets(
        &mut self,
        plugin_render_assets: Vec<PluginRenderAsset>,
    ) -> Result<Vec<PluginRenderAsset>> {
        let mut handled_render_assets = vec![];
        let mut unhandled_render_assets = vec![];
        for mut plugin_render_asset in plugin_render_assets {
            let plugin_id = plugin_render_asset.plugin_id;
            let client_id = plugin_render_asset.client_id;
            self.render_blocker.remove_blocking_plugin(plugin_id);
            if !self.accepts_plugin_output(client_id) {
                continue;
            }
            let mut vte_bytes = Some(std::mem::take(&mut plugin_render_asset.bytes));
            let mut found_plugin = false;

            {
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
                self.mark_subscriber_pane_dirty(PaneId::Plugin(plugin_id));
                handled_render_assets.push(plugin_render_asset);
            } else {
                plugin_render_asset.bytes = vte_bytes.take().unwrap();
                unhandled_render_assets.push(plugin_render_asset);
            }
        }
        if !handled_render_assets.is_empty() {
            self.schedule_render()?;
        }
        Ok(unhandled_render_assets)
    }
    fn show_plugin_cursor(
        &mut self,
        plugin_id: PluginId,
        client_id: ClientId,
        cursor_position: Option<(usize, usize)>,
    ) -> bool {
        if !self.accepts_plugin_output(client_id) {
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
    fn update_plugin_location(&mut self, plugin_id: PluginId, location: RunPluginLocation) {
        if let Some(run) = self.paneless_plugins.get_mut(&plugin_id) {
            let plugin = match run {
                RunPluginOrAlias::RunPlugin(plugin) => Some(plugin),
                RunPluginOrAlias::Alias(alias) => alias.run_plugin.as_mut(),
            };
            if let Some(plugin) = plugin {
                plugin.location = location.clone();
            }
        }
        for tab in self.tabs.values_mut() {
            if let Some(pane) = tab
                .get_pane_with_id_mut(PaneId::Plugin(plugin_id))
                .and_then(|pane| pane.plugin_control_mut())
            {
                pane.update_plugin_location(location);
                break;
            }
        }
    }

    fn update_plugin_loading_stage(
        &mut self,
        pid: PluginId,
        loading_indication: LoadingIndication,
    ) -> bool {
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
        plugin_id: PluginId,
        loading_indication: LoadingIndication,
    ) -> bool {
        for tab in self.tabs.values_mut() {
            if tab.has_plugin(plugin_id) {
                tab.start_plugin_loading_indication(plugin_id, loading_indication);
                return true;
            }
        }
        false
    }
    fn progress_plugin_loading_offset(&mut self, plugin_id: PluginId) -> bool {
        for tab in self.tabs.values_mut() {
            if tab.has_plugin(plugin_id) {
                tab.progress_plugin_loading_offset(plugin_id);
                return true;
            }
        }
        false
    }
    fn connected_clients_contains(&self, client_id: &ClientId) -> bool {
        self.connected_clients.borrow().contains(client_id)
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
                let active_pane_id = self.get_active_pane_id(client_id);
                if let Some(active_pane_id) = active_pane_id {
                    self.add_pane_id_to_group(active_pane_id, client_id);
                }
            }
        }
        self.retain_only_existing_panes_in_pane_groups();
    }
    fn get_active_pane_id(&self, client_id: &ClientId) -> Option<PaneId> {
        self.active_pane_id(*client_id)
    }

    /// The host terminal of `client_id` gained or lost focus. Only transitions are forwarded,
    /// and only to the client's active pane, which is left alone while another non-watcher
    /// client whose host is focused is sitting on the same pane.
    pub fn host_terminal_focus_changed(&mut self, client_id: ClientId, focused: bool) {
        let was_focused = self.client_host_is_focused(&client_id);
        self.client_host_focused.insert(client_id, focused);
        if was_focused == focused {
            return;
        }
        let Some(pane_id) = self.get_active_pane_id(&client_id) else {
            return;
        };
        if self.pane_is_held_focused_by_another_client(&client_id, pane_id) {
            return;
        }
        if let Ok(tab) = self.get_active_tab_mut(client_id) {
            tab.send_host_focus_event_to_pane(pane_id, focused);
        }
    }

    fn pane_is_held_focused_by_another_client(
        &self,
        client_id: &ClientId,
        pane_id: PaneId,
    ) -> bool {
        self.active_tab_ids
            .keys()
            .filter(|other_client_id| {
                *other_client_id != client_id && !self.watcher_clients.contains_key(other_client_id)
            })
            .any(|other_client_id| {
                self.client_host_is_focused(other_client_id)
                    && self.get_active_pane_id(other_client_id) == Some(pane_id)
            })
    }

    fn client_host_is_focused(&self, client_id: &ClientId) -> bool {
        self.client_host_focused
            .get(client_id)
            .copied()
            .unwrap_or(true)
    }

    pub fn set_client_host_terminal_env(
        &mut self,
        client_id: ClientId,
        host_terminal_env: BTreeMap<String, String>,
    ) {
        self.client_notification_protocols.insert(
            client_id,
            NotificationProtocol::resolve(self.host_notification_protocol, &host_terminal_env),
        );
        self.client_host_terminal_env
            .insert(client_id, host_terminal_env);
    }

    /// Applies a (re)configured `host_notification_protocol`, re-resolving every client that
    /// reported its host environment.
    pub fn set_host_notification_protocol(
        &mut self,
        host_notification_protocol: HostNotificationProtocol,
    ) {
        self.host_notification_protocol = host_notification_protocol;
        let client_ids: Vec<ClientId> = self.client_host_terminal_env.keys().copied().collect();
        for client_id in client_ids {
            let host_terminal_env = self
                .client_host_terminal_env
                .get(&client_id)
                .cloned()
                .unwrap_or_default();
            self.client_notification_protocols.insert(
                client_id,
                NotificationProtocol::resolve(host_notification_protocol, &host_terminal_env),
            );
        }
    }

    fn notification_protocol_for_client(&self, client_id: &ClientId) -> NotificationProtocol {
        self.client_notification_protocols
            .get(client_id)
            .copied()
            .unwrap_or_else(|| {
                NotificationProtocol::resolve(self.host_notification_protocol, &BTreeMap::new())
            })
    }

    /// Sends the notifications a pane asked for to every connected client, each in the protocol
    /// its host speaks: an OSC 99 host gets OSC 99 passed through with the identifier namespaced
    /// to the pane, everything else gets the assembled title and body rendered by its protocol.
    /// All clients in the app are told, not just the ones viewing the pane's tab.
    fn deliver_desktop_notifications(
        &mut self,
        notifications: Vec<PendingNotification>,
        pane_id: TerminalId,
    ) {
        let all_clients: Vec<ClientId> = self.connected_clients.borrow().iter().copied().collect();
        if all_clients.is_empty() || notifications.is_empty() {
            return;
        }
        let mut output = Output::default();
        let client_set: HashSet<ClientId> = all_clients.iter().copied().collect();
        output.add_clients(&client_set, Rc::new(RefCell::new(LinkHandler::new())), None);
        let mut emitted_anything = false;
        for client_id in &all_clients {
            let protocol = self.notification_protocol_for_client(client_id);
            for notification in &notifications {
                let raw = match (protocol, notification) {
                    (
                        NotificationProtocol::Osc99,
                        PendingNotification::Osc99 {
                            payload,
                            terminator,
                            wants_report,
                            ..
                        },
                    ) => {
                        let (metadata, rest) = match payload.find(';') {
                            Some(idx) => (
                                payload.get(..idx).unwrap_or_default(),
                                payload.get(idx..).unwrap_or_default(),
                            ),
                            None => (payload.as_str(), ""),
                        };
                        let namespaced_metadata =
                            namespace_notification_id(metadata, pane_id, *wants_report);
                        Some(format!(
                            "\u{1b}]99;{}{}{}",
                            namespaced_metadata, rest, terminator
                        ))
                    },
                    (_, PendingNotification::Osc99 { display, .. }) => display
                        .as_ref()
                        .and_then(|(title, body)| protocol.render(title, body)),
                    _ => {
                        let (title, body) = notification.title_and_body();
                        protocol.render(&title, &body)
                    },
                };
                if let Some(raw) = raw {
                    emitted_anything = true;
                    output.add_post_vte_instruction_to_multiple_clients(
                        std::iter::once(*client_id),
                        &raw,
                    );
                }
            }
        }
        if !emitted_anything {
            return;
        }
        match output.serialize() {
            Ok(serialized_output) if !serialized_output.is_empty() => {
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::Render(Some(serialized_output)))
                    .context("failed to forward desktop notifications to host terminals")
                    .non_fatal();
            },
            Ok(_) => {},
            Err(e) => {
                Err::<(), _>(e)
                    .context("failed to serialize desktop notifications for host terminals")
                    .non_fatal();
            },
        }
    }

    fn report_codex_plan_prompt(
        &mut self,
        notification: &PendingNotification,
        terminal_id: TerminalId,
    ) -> Result<()> {
        use yinz_types::agent::report::{AgentReport, AgentReportState};
        use yinz_types::agent::{hook_outcome, AgentEvent, AgentSource, HookEvent, HookOutcome};

        // Codex supplies no structured plan-prompt event. This exact TUI notification text
        // must follow upstream changes; disabled Codex notifications produce no wait here.
        let (title, body) = notification.title_and_body();
        if title != "Plan mode prompt: Implement this plan?"
            && body != "Plan mode prompt: Implement this plan?"
        {
            return Ok(());
        }
        let pane_id = PaneId::Terminal(terminal_id);
        let Some(agent) = self
            .get_pane_info(pane_id)
            .and_then(|info| info.agent)
            .filter(|agent| {
                agent.kind == "codex"
                    && agent.source == AgentSource::Reported
                    && agent.state == AgentState::Idle
            })
        else {
            return Ok(());
        };
        let Some(session_id) = agent.session_id.filter(|id| !id.is_empty()) else {
            return Ok(());
        };
        // ponytail: local dismissal has no signal; the next native hook clears this wait.
        let hook = HookEvent {
            hook_event_name: "Elicitation".into(),
            session_id,
            cwd: agent.cwd,
            message: Some("approve the plan".into()),
            ..Default::default()
        };
        let HookOutcome::Reported(agent) = hook_outcome("codex", &hook) else {
            return Ok(());
        };
        let raw = serde_json::to_string(&hook).context("serialize a Codex plan prompt")?;
        thread_main::apply_queued_agent_report(
            self,
            &AgentReport {
                report_id: uuid::Uuid::new_v4().to_string(),
                pane_id,
                state: AgentReportState::Set(agent),
                event: Some(AgentEvent::from_hook("codex", terminal_id, &hook, raw)),
                trace: false,
                process_id: yinz_types::internal_ids::ProcessId::new(std::process::id()),
                kind: "codex".into(),
            },
            Instant::now(),
        )?;
        Ok(())
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

    /// Each client's multiplayer color slot, for `TabInfo::other_focused_client_slots`.
    pub(crate) fn color_slots(
        &self,
        clients: &[ClientId],
    ) -> Vec<Option<yinz_types::multiplayer::ColorSlot>> {
        let attached = self.connected_clients.borrow();
        clients
            .iter()
            .map(|client| attached.color_slot(client))
            .collect()
    }

    fn get_tab_info(&self, tab_id: TabId, tab_names: &HashMap<TabId, String>) -> Option<TabInfo> {
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
                group: self.tab_group_info(tab.id),
                position: tab.position,
                name: tab_names[&tab.id].clone(),
                active: self.active_tab_ids.values().any(|i| i == &tab.id),
                panes_to_hide: tab.panes_to_hide_count(),
                is_fullscreen_active: tab.is_fullscreen_active(),
                is_sync_panes_active: tab.is_sync_panes_active(),
                are_floating_panes_visible: tab.are_floating_panes_visible(),
                other_focused_client_slots: self.color_slots(&all_focused_clients),
                other_focused_clients: all_focused_clients,
                active_swap_layout_name,
                is_swap_layout_dirty,
                viewport_rows: yinz_types::position::RowCount::new(tab_viewport.rows),
                viewport_columns: yinz_types::position::ColumnCount::new(tab_viewport.cols),
                display_area_rows: yinz_types::position::RowCount::new(tab_display_area.rows),
                display_area_columns: yinz_types::position::ColumnCount::new(tab_display_area.cols),
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
        }
    }
    fn update_active_pane_ids(&mut self) {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            if let Some(active_pane_id) = self.get_active_pane_id(&client_id) {
                let active_pane_id: PaneId = active_pane_id;
                let history = self.pane_history.entry(client_id).or_default();
                history.retain(|e| e != &active_pane_id);
                history.push(active_pane_id);
            }
        }
    }
}

/// The hook points a change of agent record fires, in the order they fire.
///
/// `agent_start` is a record appearing where there was none, or where the last one had exited,
/// or a new session id in the same pane (an agent restarted where another one had been: the
/// record's `session_id` exists so that a restart can be told from a pane that merely went
/// quiet). `agent_waiting` is the record moving to `waiting` from anything else - a second
/// report of `waiting` with a new detail is the same wait, not a new one. Clearing the record
/// and moving to `exited` fire nothing here: exit is `agent_exit`'s.
pub(crate) fn agent_hook_edges(
    previous: Option<&AgentInfo>,
    current: Option<&AgentInfo>,
) -> Vec<HookPoint> {
    let Some(current) = current else {
        return vec![];
    };
    let mut edges = vec![];
    if current.state != AgentState::Exited {
        let started = match previous {
            None => true,
            Some(previous) => {
                previous.state == AgentState::Exited
                    || matches!(
                        (previous.session_id.as_ref(), current.session_id.as_ref()),
                        (Some(before), Some(after)) if before != after
                    )
            },
        };
        if started {
            edges.push(HookPoint::AgentStart);
        }
    }
    if current.state == AgentState::Waiting
        && previous.map(|previous| previous.state) != Some(AgentState::Waiting)
    {
        edges.push(HookPoint::AgentWaiting);
    }
    // The turn ended: from working, or from a wait that was answered. Not from nothing - a
    // record that appears idle is `agent_start`'s - and not from `exited`, which is a
    // restart and `agent_start`'s too.
    if current.state == AgentState::Idle
        && matches!(
            previous.map(|previous| previous.state),
            Some(AgentState::Busy) | Some(AgentState::Waiting)
        )
    {
        edges.push(HookPoint::AgentIdle);
    }
    edges
}
/// The shell a pane gets when the config names none. Pinned under test so that snapshots do
/// not depend on the machine.
#[cfg(test)]
fn resolve_default_shell(configured: Option<PathBuf>) -> PathBuf {
    configured.unwrap_or_else(|| PathBuf::from("/bin/sh"))
}
#[cfg(not(test))]
fn resolve_default_shell(configured: Option<PathBuf>) -> PathBuf {
    configured.unwrap_or_else(crate::pty::get_default_shell)
}

#[cfg(not(test))]
fn get_default_editor() -> Option<PathBuf> {
    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map(PathBuf::from)
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
        let event = match event {
            ScreenInstruction::CloseSelf(plugin, client) => {
                screen.close_self_instruction(plugin, client)
            },
            ScreenInstruction::ClosePane(id, None, None, status) => {
                match screen.replacement_exit_command(id) {
                    Some(command) => ScreenInstruction::HoldPane(id, status, command),
                    None => ScreenInstruction::ClosePane(id, None, None, status),
                }
            },
            event => event,
        };
        match event {
            event @ ScreenInstruction::RequestPluginPermissions(..) => {
                screen.bus.senders.send_to_screen(event)?;
            },
            ScreenInstruction::AgentReport(report, accepted_at) => {
                thread_main::apply_queued_agent_report(screen, &report, accepted_at)?;
            },
            ScreenInstruction::ClosePane(id, _, _, _)
                if screen.destroyed_panes.contains(&id)
                    && screen.tab_containing_pane(id).is_none() => {},
            ScreenInstruction::ClosePane(id, Some(_), completion_tx, _)
            | ScreenInstruction::ClosePane(id, _, completion_tx @ Some(_), _) => {
                // Close before replaying later events: a queued close_self must not restore
                // the source that this explicit close is meant to destroy.
                screen.close_pane_explicitly(id, completion_tx);
                let client_ids: Vec<_> = screen.active_tab_ids.keys().copied().collect();
                for client_id in client_ids {
                    screen.sync_scroll_mode_on_focus(client_id)?;
                }
                screen.render()?;
            },
            ScreenInstruction::PluginBytes(plugin_render_assets) => {
                // pipes were already unblocked when these bytes first arrived
                for unhandled in screen.handle_plugin_render_assets(plugin_render_assets)? {
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
                screen.render()?;
            },
            ScreenInstruction::PaneTabs {
                command,
                client_id,
                response,
            } => {
                let result = screen
                    .pane_tabs_command(command, client_id)
                    .map_err(|error| error.to_string());
                let _ = response.send(result);
                screen.render()?;
            },
            ScreenInstruction::OverlayTabCwd {
                id,
                cwd,
                caller_cwd,
                caller_terminal_pane_id,
            } => {
                screen.overlay_tab_cwd(id, cwd, caller_cwd, caller_terminal_pane_id)?;
            },
            ScreenInstruction::PaneTabLoaded {
                id,
                index,
                generation,
                result,
                client_id: _,
            } => {
                screen.pane_tab_loaded(id, index, generation, result)?;
                screen.render()?;
            },
            ScreenInstruction::Multipane {
                operation,
                client_id,
                response,
            } => {
                let result = screen
                    .multipane_command(operation, client_id)
                    .map_err(|error| error.to_string());
                let _ = response.send(result);
            },
            ScreenInstruction::ChangeOverlayCoordinates(
                coordinates,
                override_user_placement,
                _,
            ) => {
                screen.change_overlay_coordinates(coordinates, override_user_placement);
                screen.render()?;
            },
            ScreenInstruction::UpdatePluginLoadingStage(plugin_id, loading_indication) => {
                if !screen.update_plugin_loading_stage(plugin_id, loading_indication) {
                    log::error!(
                        "Could not replay pending loading stage for missing plugin {plugin_id}"
                    );
                }
                screen.render()?;
            },
            ScreenInstruction::StartPluginLoadingIndication(plugin_id, loading_indication) => {
                if !screen.start_plugin_loading_indication(plugin_id, loading_indication) {
                    log::error!(
                        "Could not replay pending loading indication for missing plugin {plugin_id}"
                    );
                }
                screen.render()?;
            },
            ScreenInstruction::ClosePane(pane_id, client_id, completion_tx, exit_status) => {
                // Buffered only by the pty's "the child exited" road, which is the one that
                // means a command pane's exit; the guard is the same as the live handler's.
                if client_id.is_none()
                    && completion_tx.is_none()
                    && let Some(run_command) = screen
                        .tab_containing_pane(pane_id)
                        .and_then(|tab| tab.pane_run_command(pane_id))
                {
                    screen.fire_pane_exit_hook(pane_id, exit_status, &run_command, false);
                }
                if let PaneId::Plugin(plugin_id) = pane_id {
                    screen.drop_quit_notice(plugin_id);
                }
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
                } else if screen.tab_containing_pane(pane_id).is_none() {
                    screen.cleanup_closed_pane(pane_id);
                }
                screen.clear_nested_guest(pane_id);
                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
                    .non_fatal();
                screen.log_and_report_session_state().non_fatal();
                screen.retain_only_existing_panes_in_pane_groups();
            },
            ScreenInstruction::RestoreHeldPane(pane_id, held) => {
                screen.restore_held_pane(pane_id, &held);
                screen.log_and_report_session_state().non_fatal();
            },
            ScreenInstruction::HoldPane(pane_id, exit_status, run_command) => {
                screen.invalidate_agent_lifecycle(pane_id, "held");
                let is_first_run = false;
                let mut found = false;
                let mut pane_is_agent = false;
                let mut queued_respawn = None;
                for tab in screen.tabs.values_mut() {
                    if tab.get_all_pane_ids().contains(&pane_id) {
                        queued_respawn =
                            tab.hold_pane(pane_id, exit_status, is_first_run, run_command.clone());
                        found = true;
                        pane_is_agent = tab.pane_is_agent(pane_id);
                        break;
                    }
                }
                if found {
                    screen.fire_pane_exit_hook(pane_id, exit_status, &run_command, true);
                    screen.fire_agent_exit_hook(pane_id, exit_status, &run_command, pane_is_agent);
                    if let Some((terminal_pane_id, run_command, keep_scrollback)) = queued_respawn {
                        screen.run_command_in_pane_with_id(
                            terminal_pane_id,
                            run_command,
                            keep_scrollback,
                        );
                    }
                }
                if !found {
                    log::error!(
                        "Could not replay pending hold for missing plugin pane {pane_id:?}"
                    );
                }
                screen.log_and_report_session_state()?;
                screen.render()?;
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
