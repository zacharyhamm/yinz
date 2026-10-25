//! `Tab`s holds multiple panes. It tracks their coordinates (x/y) and size,
//! as well as how they should be resized

mod clipboard;
mod copy_command;
mod layout;
mod mouse_handler;
mod navigation;
mod overlays;
mod pane_creation;
mod pane_lifecycle;
mod pane_tabs;
mod render;
mod scrollback;
mod selection;
mod stack_lists;
mod swap_layouts;
mod tab_groups;
mod terminal_io;

// The pane-layer items that `crate::tab` users name, under the paths they had when they lived in
// this module.
pub use yinz_panes::layout_applier::arrangement_layout;
pub use yinz_panes::pane::{
    AdjustedInput, GuestChoiceIndicator, Pane, PaneEdge, MIN_TERMINAL_HEIGHT,
};

use crate::plugins::PluginId;

use crate::ui::damage::CellRect;

use copy_command::CopyCommand;
pub use mouse_handler::{MouseEffect, MouseHandler, PaneResizeState, ResizeDragTarget};

use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use yinz_runtime::errors::prelude::*;
use yinz_types::agent::AgentInfo;
use yinz_types::data::NewPanePlacement;
use yinz_types::data::OverlayPlacement;
use yinz_types::data::PaneInfo;
use yinz_types::data::RegexHighlight;
use yinz_types::data::StyledText;
use yinz_types::geometry::Direction;
use yinz_types::geometry::ResizeStrategy;
use yinz_types::input::command::RunCommand;
use yinz_types::input::options::DEFAULT_WORD_SEPARATORS;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::TerminalId;
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::MouseEvent;
use yinz_types::multiplayer::AttachedClients;
use yinz_types::position::Position;
use yinz_types::style::Style;
use yinz_types::text::clean_string_from_control_and_linebreak;

use crate::background_jobs::BackgroundJob;
use crate::pane_groups::PaneGroups;
use crate::pty_writer::PtyWriteInstruction;
use crate::screen::ScreenInstruction;
use crate::ui::hint_text::held_hint_variants;
use crate::ui::hint_text::hover_hint_variants;
use crate::ui::hint_text::resize_hint_variants;
use crate::ui::hint_text::suspended_hint_variants;
use crate::ui::hint_text::HintExitStatus;
use crate::ui::loading_indication::LoadingIndication;
use swap_layouts::SwapLayouts;
use yinz_panes::layout_applier::LayoutApplier;
use yinz_panes::pane::pane_info_for_pane;
use yinz_panes::resize_pty;

use self::clipboard::ClipboardProvider;
use crate::os_input_output::ServerOsApi;
use crate::output::Output;
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::nested_session_modal::GuestModalShortcuts;
use crate::panes::sixel::SixelImageStore;
use crate::panes::FloatingPanes;
use crate::panes::LinkHandler;
use crate::panes::OverlayCarry;
use crate::panes::OverlayPanes;
use crate::panes::PaneId;
use crate::panes::TiledPanes;
use crate::plugins::PluginInstruction;
use crate::pty::ClientTabIndexOrPaneId;
use crate::pty::PtyInstruction;
use crate::pty::VteBytes;
use crate::route::NotificationEnd;
use crate::thread_bus::ThreadSenders;
use crate::ClientId;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use std::str;
use std::time::Instant;
use yinz_types::data::Event;
use yinz_types::data::InputMode;
use yinz_types::data::ModeInfo;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::layout::Run;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::input::layout::SwapFloatingLayout;
use yinz_types::input::layout::SwapTiledLayout;
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::pane_size::Offset;
use yinz_types::pane_size::PaneGeom;
use yinz_types::pane_size::Size;
use yinz_types::pane_size::SizeInPixels;
use yinz_types::pane_size::Viewport;
use yinz_types::style::Palette;
use yinz_types::style::Styling;

const MAX_PENDING_VTE_EVENTS: usize = 7000;

/// The pane holding a suppressed pane's place in the grid.
///
/// Closing it is what brings the suppressed pane back, which is the whole reason
/// the association is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandIn {
    pub pane_id: PaneId,
    pub is_scrollback_editor: bool,
    /// The stand-in only borrows the slot (`ReplacedPane::Lend`): an explicit close of the
    /// stand-in brings the suppressed pane back instead of closing it as well.
    pub lent: bool,
}

/// A pane that has been taken out of the layout but not closed: it is not
/// rendered and cannot be focused, but its process is still running and it can
/// still be found by id.
pub struct SuppressedPane {
    pub pane: Box<dyn Pane>,
    /// `Some` => a stand-in holds this pane's place in the grid, and closing that
    /// stand-in brings this pane back. `None` => parked under its own id, and only
    /// an explicit action takes it out again (a plugin's `show_self`, selecting a
    /// stack-list member).
    pub restored_by: Option<StandIn>,
}

/// Everything a container needs when it crosses tabs, including panes hidden by replacements.
pub struct OverlayTransfer {
    pane_tabs: BTreeMap<yinz_types::pane_tabs::PaneTabsId, pane_tabs::PaneTabs>,
    container: crate::panes::OverlayPane,
    suppressed: SuppressedPanes,
    respawns: HashMap<PaneId, (RunCommand, bool)>,
    pending_output: HashMap<TerminalId, Vec<VteBytes>>,
    waiting_resize: HashSet<TerminalId>,
}
impl OverlayTransfer {
    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.container
            .pane_ids()
            .chain(self.suppressed.keys().copied())
    }
}

impl SuppressedPane {
    fn parked(pane: Box<dyn Pane>) -> Self {
        SuppressedPane {
            pane,
            restored_by: None,
        }
    }
    fn behind(
        pane: Box<dyn Pane>,
        stand_in: PaneId,
        is_scrollback_editor: bool,
        lent: bool,
    ) -> Self {
        SuppressedPane {
            pane,
            restored_by: Some(StandIn {
                pane_id: stand_in,
                is_scrollback_editor,
                lent,
            }),
        }
    }
}

/// Keyed by the suppressed pane's **own** id.
///
/// Invariant: `map[id].pane.pid() == id`. The pane that suppressed it, if any, is
/// in `restored_by` rather than in the key - a pane is only ever in here once, and
/// is always found by the id it has everywhere else.
pub type SuppressedPanes = HashMap<PaneId, SuppressedPane>;

pub type StackListId = usize;

#[derive(Debug, Clone)]
pub struct StackList {
    pub members: Vec<PaneId>,
    pub visible: PaneId,
}

impl StackList {
    pub fn rank_of(&self, pane_id: &PaneId) -> Option<usize> {
        self.members.iter().position(|p| p == pane_id)
    }
}

enum StackListRemoval {
    NotRemoved,
    DissolvedInPlace,
    Removed(Option<Box<dyn Pane>>),
}

/// What [`Tab::accept_extracted_pane`] did with a pane moved in from another tab.
///
/// `NoRoom` carries the pane back out so the caller can put it where it came from -
/// dropping it would destroy it silently. See `accept_extracted_pane` for why.
pub enum AcceptedPane {
    /// The pane is in this tab.
    Accepted,
    /// This tab has no room; the pane is handed back, still intact.
    NoRoom(Box<dyn Pane>),
    /// The pane was consumed by the add and then lost inside it - it no longer exists
    /// and cannot be handed back. Only reachable when the pty thread is already gone.
    LostInAdd,
}

/// Where a pane extracted from one tab belongs when another tab accepts it.
///
/// Read from the source tab with [`Tab::placement_for_extraction`] *before*
/// [`Tab::extract_pane`], which destroys the wrapper holding an overlay's kind and
/// coordinates. "Not floating" is not "tiled": since overlays moved out of the floating
/// collection they answer `false` to `pane_id_is_floating`, so a caller that asks only that
/// question lands a modal in the destination as an ordinary tiled pane, silently stripped of
/// the one property it cannot do without.
#[derive(Debug, Clone)]
pub enum ExtractedPanePlacement {
    Tiled,
    Floating,
    /// Use destination geometry for a group extracted from a temporary tiled tab.
    FloatingDefault {
        min_height: usize,
    },
    /// [`OverlayCarry`] is the in-memory half of the overlay's wrapper - whether it was set
    /// aside, and whether where it sits is the user's doing. It rides here rather than in
    /// `OverlayPlacement` because that struct crosses the protobuf boundary and neither bit is
    /// placement.
    Overlay(OverlayPlacement, OverlayCarry),
}

enum BufferedTabInstruction {
    RequestPaneRedraw(TerminalId),
    SetPaneSelectable(PaneId, bool),
    HandlePtyBytes(TerminalId, VteBytes),
    HoldPane(PaneId, Option<i32>, bool, RunCommand), // Option<i32> is the exit status, bool is is_first_run
}

/// What the host terminal was last told about one client's cursor.
///
/// The host keeps this state until something changes it, so a frame only has to send the parts
/// that moved. That matters most for `shape`: re-sending the cursor-shape CSI restarts the host's
/// blink phase, and on a frame cadence set by an animation rather than by the user that reads as
/// the cursor flickering.
/// Which byte stream a `HostCursorState` describes.
///
/// The watchers' frame is built from the followed client's panes and carries that client's id,
/// but it is a second stream, delivered to other terminals, with its own occlusion - the session's
/// toasts are drawn into the regular clients' frame only. So it remembers what it told the host
/// separately: sharing the entry let each pass conclude "nothing changed" from what the other one
/// said, and leave a real client's cursor hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CursorMemoKey {
    /// The frame this client's own terminal receives.
    Client(ClientId),
    /// The frame every watcher receives, named by the client they are following.
    Watchers(ClientId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostCursorState {
    position: Option<(usize, usize)>,
    shape: Option<String>,
    visible: bool,
}

/// Which pane a `Tab` entry point is about: the one a client has focused, or one named by id.
#[derive(Clone, Copy, Debug)]
enum PaneSubject {
    Active(ClientId),
    Pane(PaneId),
}

/// Where a pane is asked to go: the next (or previous) slot in its layer, or one step in a
/// direction.
#[derive(Clone, Copy, Debug)]
enum PaneMove {
    Next { backwards: bool },
    Toward(Direction),
}

/// Where a tab's last client was focused in each layer when it left.
#[derive(Debug, Clone, Copy, Default)]
struct DepartedFocus {
    overlay: Option<PaneId>,
    floating: Option<PaneId>,
    tiled: Option<PaneId>,
}

pub struct Tab {
    /// Only this tab owns the strong reference; delayed launches hold weak references.
    pub agent_tab_lifetime: std::sync::Arc<()>,
    pub agent_tab_cleanup_allowed: bool,
    /// Keep an empty default layout alive until its reserved agent pane arrives.
    pub agent_tab_waiting_for_pane: bool,
    pending_agent_placements: Vec<(PaneId, NotificationEnd)>,
    pub agent_worktrees: HashMap<PaneId, PathBuf>,
    pub agent_double_escape: bool,
    pub max_agent_width: usize,
    pending_render_damage: Vec<CellRect>,
    pub pending_agent_escapes: HashMap<(ClientId, PaneId), Instant>,
    pub id: TabId,
    pub position: TabPosition,
    pub name: String,
    pub prev_name: String,
    /// The "Tab #N" a user gets back by clearing the name they gave.
    default_name: String,
    /// Whether the user chose this tab's name. Only this keeps a project or a lent pane title
    /// off the tab: a name a layout gave it is just a fallback. A rename in this session sets
    /// it, and so does a name typed in a command, a resurrected session's record of a rename,
    /// and a warp carrying one from the session it left.
    user_named: bool,
    /// `user_named` as it was when rename mode began, for the undo that ends it.
    prev_user_named: bool,
    pub size: Size,
    tiled_panes: TiledPanes,
    floating_panes: FloatingPanes,
    // panes drawn over the whole session surface, above this tab's own panes and chrome - a
    // sibling of the two above rather than a special case of the floating layer, so that "the
    // floating layer is down" can never mean "nothing on it is focused".
    overlay_panes: OverlayPanes,
    suppressed_panes: SuppressedPanes,
    pane_tabs: BTreeMap<yinz_types::pane_tabs::PaneTabsId, pane_tabs::PaneTabs>,
    stacked_pane_list: Rc<RefCell<bool>>,
    reserved_top_rows: Rc<RefCell<HashMap<PaneId, usize>>>,
    stack_lists: HashMap<StackListId, StackList>,
    stack_list_of_member: HashMap<PaneId, StackListId>,
    next_stack_list_id: StackListId,
    stack_list_session_is_mirrored: bool,
    max_panes: Option<usize>,
    viewport: Rc<RefCell<Viewport>>, // includes all non-UI panes
    display_area: Rc<RefCell<Size>>, // includes all panes (including eg. the status bar and tab bar in the default layout)
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    os_api: Box<dyn ServerOsApi>,
    pub senders: ThreadSenders,
    synchronize_is_active: bool,
    should_clear_display_before_rendering: bool,
    mode_info: Rc<RefCell<HashMap<ClientId, ModeInfo>>>,
    default_mode_info: ModeInfo,
    pub style: Style,
    connected_clients: Rc<RefCell<HashSet<ClientId>>>,
    pane_frame_style: PaneFrameStyle,
    auto_layout: bool,
    tab_group_count: usize,
    pending_vte_events: HashMap<TerminalId, Vec<VteBytes>>,
    pub selecting_with_mouse_in_pane: Option<PaneId>, // this is only pub for the tests
    pane_being_resized_with_mouse: Option<PaneResizeState>,
    link_handler: Rc<RefCell<LinkHandler>>,
    clipboard_provider: ClipboardProvider,
    /// The pane the layout marked `focus=true`, where a client joining an otherwise
    /// empty tab lands. A property of the layout, not of any client: it survives
    /// clients detaching, and is only checked against the panes still in the tab.
    focus_pane_id: Option<PaneId>,
    /// Where the tab's last client was focused when it left. A client joining the empty tab
    /// lands there, before the layout's choice: a client that reattaches gets a new id, and
    /// would otherwise lose its place.
    departed_focus: DepartedFocus,
    copy_on_select: bool,
    terminal_emulator_colors: Rc<RefCell<Palette>>,
    terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
    pids_waiting_resize: HashSet<TerminalId>, // TerminalId is the terminal_id
    // Replacement commands for panes whose current command is still running: a
    // `RunCommandInPane` that raced the pane's own exit waits here and is started by
    // `hold_pane` (last write wins). bool is keep_scrollback.
    pending_command_respawns: HashMap<PaneId, (RunCommand, bool)>,
    /// What the host terminal was last told about each cursor, so a frame says only what changed.
    /// Dropped rather than updated wherever the next frame has to place the cursor from scratch -
    /// see `render_cursor`. Keyed by stream rather than by client, for the reason `CursorMemoKey`
    /// gives.
    host_cursor_state: HashMap<CursorMemoKey, HostCursorState>,
    is_pending: bool, // a pending tab is one that is still being loaded or otherwise waiting
    pending_instructions: Vec<BufferedTabInstruction>, // instructions that came while the tab was
    // pending and need to be re-applied
    swap_layouts: SwapLayouts,
    default_shell: PathBuf,
    default_editor: Option<PathBuf>,
    debug: bool,
    arrow_fonts: bool,
    kitty_host_support: Option<KittyHostSupport>,
    sixel_host_support: Option<bool>,
    background_opacity: u8,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    explicitly_disable_kitty_keyboard_protocol: bool,
    mouse_hover_pane_id: HashMap<ClientId, PaneId>,
    // the pane a keyboard text selection was started on, so that it is the pane the selection is
    // moved, copied and finished on even if focus moved elsewhere in the meantime
    keyboard_text_selection: HashMap<ClientId, PaneId>,
    dimmed_clients: HashSet<ClientId>,
    plugin_hover_pane_id: HashMap<ClientId, PaneId>,
    mouse_last_pane_id: HashMap<ClientId, PaneId>,
    mouse_help_text_visible: HashMap<ClientId, bool>,
    last_mouse_activity_time: HashMap<ClientId, Instant>,
    last_hint_text: HashMap<ClientId, BTreeMap<usize, StyledText>>,
    last_active_pane_scroll: HashMap<ClientId, Option<(usize, usize)>>,
    current_pane_group: Rc<RefCell<PaneGroups>>,
    advanced_mouse_actions: bool,
    mouse_scroll_resize: bool,
    mouse_hover_effects: bool,
    focus_follows_mouse: bool,
    mouse_click_through: bool,
    pub context_menu_on_right_click: bool,
    pub context_menu_override_agent_mouse: bool,
    osc133_command_selection: bool,
    word_separators: String,
    currently_marking_pane_group: Rc<RefCell<HashMap<ClientId, bool>>>,
    connected_clients_in_app: Rc<RefCell<AttachedClients>>,
    pub panes_with_pending_bell: HashSet<PaneId>,
    pub tab_has_pending_bell: bool,
    pub tab_bell_flash: bool, // currently in mid-notification-flash
    pub tab_bell_ring: bool,  // need to send ANSI BEL to the controlling terminal
    /// What `visible` last said about this tab; floating-surface toggles only report their own
    /// visibility to plugins while the tab itself is on screen.
    tab_visible: bool,
}

/// Whether a pane launched with this `Run` is an overlay by its own configuration - a plugin
/// with `overlay`/`modal` set. Overlay-ness has to survive callers that pass their own
/// `FloatingPaneCoordinates` (which suppresses the flag `AddPlugin` would otherwise synthesize)
/// and configs written before the flags existed.
fn run_requests_overlay(run: &Option<Run>) -> bool {
    matches!(run, Some(Run::Plugin(run_plugin)) if run_plugin.requests_overlay())
}

/// The placement a pane launched with this `Run` asked for by its own configuration, if any.
///
/// Kept separate from `run_requests_overlay` because the *kind* matters: a modal is an overlay
/// that must not have focus taken from it, and until overlays had a collection to store that in,
/// modal-ness was re-read from this configuration at every decision point.
fn run_overlay_placement(run: &Option<Run>) -> Option<OverlayPlacement> {
    match run {
        Some(Run::Plugin(run_plugin)) => run_plugin.overlay_placement(),
        _ => None,
    }
}

/// The overlay placement a pane arriving by one of the coordinate-carrying paths asks for.
///
/// Overlay-ness is a placement now, so the only thing left to resolve is the compatibility shim:
/// a pane whose plugin configuration still spells it as `overlay`/`modal` keys. Coordinates say
/// *where* on the surface and never *whether*, so they are carried through untouched.
fn overlay_placement_for(
    coordinates: &Option<FloatingPaneCoordinates>,
    invoked_with: &Option<Run>,
) -> Option<OverlayPlacement> {
    run_overlay_placement(invoked_with).map(|placement| OverlayPlacement {
        coordinates: coordinates.clone().or(placement.coordinates.clone()),
        ..placement
    })
}

/// Whether a pane launched with this `Run` places its own overlay, and so must not be painted
/// until it has been placed. See [`OverlayCarry::awaiting_placement`].
///
/// Read at a pane's *birth* only. An overlay moving between tabs carries its `OverlayCarry`
/// across (`accept_extracted_pane`), and re-deriving this there would blank a pane that was placed
/// long ago every time the user switched tabs.
fn run_awaits_placement(run: &Option<Run>) -> bool {
    matches!(run, Some(Run::Plugin(run_plugin)) if run_plugin.awaits_placement())
}

impl Tab {
    pub fn new(
        id: TabId,
        position: TabPosition,
        name: String,
        display_area: Size,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        stacked_resize: Rc<RefCell<bool>>,
        stacked_pane_list: Rc<RefCell<bool>>,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        os_api: Box<dyn ServerOsApi>,
        senders: ThreadSenders,
        max_panes: Option<usize>,
        style: Style,
        default_mode_info: ModeInfo,
        pane_frame_style: PaneFrameStyle,
        auto_layout: bool,
        connected_clients_in_app: Rc<RefCell<AttachedClients>>,
        session_is_mirrored: bool,
        client_id: Option<ClientId>,
        copy_options: CopyOptions,
        terminal_emulator_colors: Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
        swap_layouts: (Vec<SwapTiledLayout>, Vec<SwapFloatingLayout>),
        default_shell: PathBuf,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        explicitly_disable_kitty_keyboard_protocol: bool,
        default_editor: Option<PathBuf>,
        current_pane_group: Rc<RefCell<PaneGroups>>,
        currently_marking_pane_group: Rc<RefCell<HashMap<ClientId, bool>>>,
        advanced_mouse_actions: bool,
        mouse_scroll_resize: bool,
        mouse_hover_effects: bool,
        focus_follows_mouse: bool,
        mouse_click_through: bool,
        context_menu_on_right_click: bool,
        context_menu_override_agent_mouse: bool,
    ) -> Self {
        let default_name = format!("Tab #{}", id.get() + 1);
        let name = if name.is_empty() {
            default_name.clone()
        } else {
            name
        };

        let mut connected_clients = HashSet::new();
        if let Some(client_id) = client_id {
            connected_clients.insert(client_id);
        }
        let initial_size = display_area;
        let viewport: Viewport = display_area.into();
        let viewport = Rc::new(RefCell::new(viewport));
        let display_area = Rc::new(RefCell::new(display_area));
        let connected_clients = Rc::new(RefCell::new(connected_clients));
        let mode_info = Rc::new(RefCell::new(HashMap::new()));
        let reserved_top_rows: Rc<RefCell<HashMap<PaneId, usize>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let fullscreen_covers_ui = Rc::new(RefCell::new(false));

        let tiled_panes = TiledPanes::new(
            display_area.clone(),
            viewport.clone(),
            connected_clients.clone(),
            connected_clients_in_app.clone(),
            mode_info.clone(),
            character_cell_size.clone(),
            stacked_resize.clone(),
            reserved_top_rows.clone(),
            fullscreen_covers_ui.clone(),
            session_is_mirrored,
            pane_frame_style,
            default_mode_info.clone(),
            style,
            os_api.clone(),
            senders.clone(),
        );
        let floating_panes = FloatingPanes::new(
            display_area.clone(),
            viewport.clone(),
            connected_clients.clone(),
            connected_clients_in_app.clone(),
            mode_info.clone(),
            character_cell_size.clone(),
            fullscreen_covers_ui.clone(),
            pane_frame_style,
            session_is_mirrored,
            default_mode_info.clone(),
            style,
            os_api.clone(),
            senders.clone(),
        );
        let overlay_panes = OverlayPanes::new(
            display_area.clone(),
            viewport.clone(),
            connected_clients.clone(),
            connected_clients_in_app.clone(),
            mode_info.clone(),
            character_cell_size.clone(),
            session_is_mirrored,
            default_mode_info.clone(),
            style,
            os_api.clone(),
            senders.clone(),
        );

        let clipboard_provider = match copy_options.command {
            Some(command) => ClipboardProvider::Command(CopyCommand::new(command)),
            None => ClipboardProvider::Osc52(copy_options.clipboard),
        };
        let swap_layouts = SwapLayouts::new(swap_layouts, display_area.clone());

        Tab {
            agent_worktrees: HashMap::new(),
            max_agent_width: 0,
            pending_render_damage: Vec::new(),
            agent_double_escape: false,
            pending_agent_escapes: HashMap::new(),
            id,
            position,
            tiled_panes,
            floating_panes,
            overlay_panes,
            suppressed_panes: HashMap::new(),
            pane_tabs: BTreeMap::new(),
            stacked_pane_list,
            reserved_top_rows,
            stack_lists: HashMap::new(),
            stack_list_of_member: HashMap::new(),
            next_stack_list_id: 0,
            stack_list_session_is_mirrored: session_is_mirrored,
            name: name.clone(),
            prev_name: name,
            default_name,
            user_named: false,
            prev_user_named: false,
            size: initial_size,
            max_panes,
            viewport,
            display_area,
            character_cell_size,
            sixel_image_store,
            kitty_image_store,
            synchronize_is_active: false,
            os_api,
            senders,
            should_clear_display_before_rendering: false,
            style,
            mode_info,
            default_mode_info,
            pane_frame_style,
            auto_layout,
            tab_group_count: 1,
            pending_vte_events: HashMap::new(),
            connected_clients,
            selecting_with_mouse_in_pane: None,
            pane_being_resized_with_mouse: None,
            link_handler: Rc::new(RefCell::new(LinkHandler::new())),
            clipboard_provider,
            focus_pane_id: None,
            departed_focus: DepartedFocus::default(),
            copy_on_select: copy_options.copy_on_select,
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            pids_waiting_resize: HashSet::new(),
            pending_command_respawns: HashMap::new(),
            host_cursor_state: HashMap::new(),
            agent_tab_lifetime: std::sync::Arc::new(()),
            agent_tab_cleanup_allowed: false,
            agent_tab_waiting_for_pane: false,
            pending_agent_placements: vec![],
            is_pending: true, // will be switched to false once the layout is applied
            pending_instructions: vec![],
            swap_layouts,
            default_shell,
            debug,
            arrow_fonts,
            kitty_host_support: None,
            sixel_host_support: None,
            background_opacity: yinz_types::input::options::DEFAULT_BACKGROUND_OPACITY,
            styled_underlines,
            osc8_hyperlinks,
            explicitly_disable_kitty_keyboard_protocol,
            default_editor,
            mouse_hover_pane_id: HashMap::new(),
            keyboard_text_selection: HashMap::new(),
            plugin_hover_pane_id: HashMap::new(),
            mouse_last_pane_id: HashMap::new(),
            mouse_help_text_visible: HashMap::new(),
            last_mouse_activity_time: HashMap::new(),
            last_hint_text: HashMap::new(),
            last_active_pane_scroll: HashMap::new(),
            current_pane_group,
            currently_marking_pane_group,
            advanced_mouse_actions,
            mouse_scroll_resize,
            mouse_hover_effects,
            focus_follows_mouse,
            mouse_click_through,
            context_menu_on_right_click,
            context_menu_override_agent_mouse,
            osc133_command_selection: true,
            word_separators: DEFAULT_WORD_SEPARATORS.to_owned(),
            connected_clients_in_app,
            panes_with_pending_bell: HashSet::new(),
            tab_has_pending_bell: false,
            tab_bell_flash: false,
            tab_bell_ring: false,
            dimmed_clients: HashSet::new(),
            tab_visible: true,
        }
    }

    pub fn set_client_dimmed(&mut self, client_id: ClientId, dimmed: bool) {
        if dimmed {
            self.dimmed_clients.insert(client_id);
        } else {
            self.dimmed_clients.remove(&client_id);
        }
        self.tiled_panes.set_client_dimmed(client_id, dimmed);
        self.floating_panes.set_client_dimmed(client_id, dimmed);
        self.overlay_panes.set_client_dimmed(client_id, dimmed);
    }

    pub fn request_pane_redraw(&mut self, terminal_id: TerminalId) -> Result<()> {
        if self.is_pending {
            self.pending_instructions
                .push(BufferedTabInstruction::RequestPaneRedraw(terminal_id));
            return Ok(());
        }
        let Some(pane) = self.get_pane_with_id(PaneId::Terminal(terminal_id)) else {
            return Ok(());
        };
        let columns = pane.get_content_columns() as u16;
        let rows = pane.get_content_rows() as u16;
        if pane.is_held() || pane.exited() || columns == 0 || rows == 0 {
            return Ok(());
        }
        // An unchanged winsize does not trigger SIGWINCH. Flush the resize batch
        // so it cannot coalesce this nudge with the exact size restored below.
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .context("failed to flush resizes before pane redraw")?;
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ResizePty(
                terminal_id,
                if columns == 1 { 2 } else { columns - 1 },
                rows,
                None,
                None,
            ))
            .context("failed to nudge pane size for redraw")?;
        resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
            .context("failed to restore pane size after redraw nudge")
    }

    pub fn apply_buffered_instructions(&mut self) -> Result<()> {
        let buffered_instructions: Vec<BufferedTabInstruction> =
            std::mem::take(&mut self.pending_instructions);
        for buffered_instruction in buffered_instructions {
            match buffered_instruction {
                BufferedTabInstruction::RequestPaneRedraw(terminal_id) => {
                    // Finish the whole layout event, including group relocation, first.
                    self.senders
                        .send_to_screen(ScreenInstruction::RequestPaneRedraw(terminal_id))
                        .context("failed to requeue adopted pane redraw")
                        .non_fatal();
                },
                BufferedTabInstruction::SetPaneSelectable(pane_id, selectable) => {
                    self.set_pane_selectable(pane_id, selectable);
                },
                BufferedTabInstruction::HandlePtyBytes(terminal_id, bytes) => {
                    self.handle_pty_bytes(terminal_id, bytes)?;
                },
                BufferedTabInstruction::HoldPane(
                    terminal_id,
                    exit_status,
                    is_first_run,
                    run_command,
                ) => {
                    if let Some((terminal_pane_id, run_command, keep_scrollback)) =
                        self.hold_pane(terminal_id, exit_status, is_first_run, run_command)
                    {
                        self.senders
                            .send_to_screen(ScreenInstruction::RunCommandInPane(
                                terminal_pane_id,
                                run_command,
                                keep_scrollback,
                            ))
                            .context("failed to requeue buffered pane respawn")?;
                    }
                },
            }
        }
        Ok(())
    }
    pub fn rename_session(&mut self, new_session_name: SessionName) -> Result<()> {
        {
            let mode_infos = &mut self.mode_info.borrow_mut();
            for mode_info in mode_infos.values_mut() {
                mode_info.session_name = Some(new_session_name.clone());
            }
            self.default_mode_info.session_name = Some(new_session_name);
        }
        self.update_input_modes()
    }
    pub fn update_input_modes(&mut self) -> Result<()> {
        // this updates only this tab's plugins with the client's input mode
        let mode_infos = self.mode_info.borrow();
        let mut plugin_updates = vec![];
        let currently_marking_pane_group = self.currently_marking_pane_group.borrow();
        let tab_plugin_ids = self.get_plugin_ids_including_suppressed();
        for client_id in self.connected_clients.borrow().iter() {
            let mut mode_info = mode_infos
                .get(client_id)
                .unwrap_or(&self.default_mode_info)
                .clone();
            mode_info.shell = Some(self.default_shell.clone());
            mode_info.editor = self.default_editor.clone();
            mode_info.currently_marking_pane_group =
                currently_marking_pane_group.get(client_id).copied();
            mode_info.pane_frame_style = Some(self.pane_frame_style);
            for plugin_id in &tab_plugin_ids {
                plugin_updates.push((
                    Some(*plugin_id),
                    Some(*client_id),
                    Event::ModeUpdate(mode_info.clone()),
                ));
            }
        }
        if !plugin_updates.is_empty() {
            self.senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates))
                .with_context(|| "failed to update plugins with mode info".to_string())?;
        }
        Ok(())
    }
    fn resolve_hint_text(&self, client_id: ClientId) -> BTreeMap<usize, StyledText> {
        let focused_pane_id = self.get_active_pane_id(client_id);
        let hovered_pane_id = self.mouse_hover_pane_id.get(&client_id).copied();
        if let Some(hovered_pane_id) = hovered_pane_id
            && Some(hovered_pane_id) != focused_pane_id
        {
            return hover_hint_variants();
        }
        if let Some(focused_pane) = self.get_active_pane(client_id) {
            if focused_pane.is_suspended() {
                return suspended_hint_variants();
            }
            if focused_pane.is_held() {
                let exited = focused_pane.exited();
                let is_first_run = !exited;
                let exit_status = if exited {
                    Some(match focused_pane.exit_status() {
                        Some(exit_code) => HintExitStatus::Code(exit_code),
                        None => HintExitStatus::Exited,
                    })
                } else {
                    None
                };
                return held_hint_variants(is_first_run, exit_status);
            }
        }
        let resize_help_visible = self
            .mouse_help_text_visible
            .get(&client_id)
            .copied()
            .unwrap_or(false);
        if resize_help_visible {
            // A modal is sized by the plugin that asked the question, so nothing here may offer to
            // resize it. This surface never knew about overlays - `selectable_pane_count` below
            // counts only the other two collections - so the focused pane has to be asked directly.
            if focused_pane_id.is_some_and(|pane_id| {
                self.pane_is_overlay(pane_id) && !self.overlay_panes.is_resizable(&pane_id)
            }) {
                return BTreeMap::new();
            }
            let selectable_pane_count = self.get_selectable_tiled_panes().count()
                + self.get_selectable_floating_panes().count();
            if selectable_pane_count > 1 && !self.is_fullscreen_active() {
                let focused_pane_is_floating = self.floating_panes.panes_are_visible();
                return resize_hint_variants(focused_pane_is_floating, self.mouse_scroll_resize);
            }
        }
        BTreeMap::new()
    }
    pub fn emit_hint_text_if_changed(&mut self) -> Result<()> {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        let mut plugin_updates = vec![];
        for client_id in connected_clients {
            let hint_text = self.resolve_hint_text(client_id);
            if self.last_hint_text.get(&client_id) != Some(&hint_text) {
                self.last_hint_text.insert(client_id, hint_text.clone());
                plugin_updates.push((None, Some(client_id), Event::HintText(hint_text)));
            }
        }
        if !plugin_updates.is_empty() {
            self.senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates))
                .with_context(|| "failed to update plugins with hint text".to_string())?;
        }
        Ok(())
    }
    pub fn clear_hint_text_cache(&mut self) {
        self.last_hint_text.clear();
    }
    fn get_selectable_tiled_content_panes(
        &self,
    ) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.get_selectable_tiled_panes()
            .filter(|(_, p)| !p.borderless())
    }
    /// How wide a tab name may be, decorations included, whichever of the three names won.
    /// Matches `MAX_TAB_NAME_WIDTH` in the tab bars, which cap every tab name whatever its
    /// source; staying inside the same figure here means the bars have nothing left to cut, and
    /// the decorations - the pane id in front, the exit marker behind - survive.
    ///
    /// The bars charge their own "(FULLSCREEN)" and "[!]" markers to that same budget, so a tab
    /// wearing one of those *and* an exit marker can still lose the tail of the latter. That is a
    /// corner it is not worth threading the bars' markers back through the server for.
    const MAX_TAB_NAME_WIDTH: usize = 40;

    /// How many columns are left for the name itself, once this tab's decorations are charged.
    ///
    /// The decorations are what a reader cannot reconstruct - `[5] ` says which pane
    /// `--pane-id` reaches, `[ EXIT CODE: 2 ]` how it ended - so they are taken off the top and
    /// the name is cut to what remains. See [`Screen::plugin_tab_names`], which does the cutting
    /// for all three names in one place.
    pub fn tab_name_budget(&self) -> usize {
        Self::MAX_TAB_NAME_WIDTH
            .saturating_sub(self.lone_pane_id_marker().width())
            .saturating_sub(self.exit_marker().width())
    }

    pub fn single_pane_titles(&self) -> bool {
        self.pane_frame_style.draws_titles()
            && self.get_selectable_tiled_content_panes().count() == 1
    }
    /// Whether something other than the tab's own name may name it: true until the user
    /// renames it in this session, whatever name it was born with.
    pub fn tab_name_is_automatic(&self) -> bool {
        !self.user_named
    }
    pub fn is_user_named(&self) -> bool {
        self.user_named
    }
    /// For a tab born with a name the user chose - carried by a warp from the session it left,
    /// typed in a command, restored by resurrection. It is where the tab starts, so an undo
    /// has nothing of it to revert.
    pub fn set_user_named(&mut self, user_named: bool) {
        self.user_named = user_named;
        self.prev_user_named = user_named;
    }
    /// A name the user asked for. An empty one hands the tab back to automatic naming,
    /// under the "Tab #N" it started with, rather than leaving it blank.
    pub fn rename_by_user(&mut self, name: String) {
        self.agent_tab_cleanup_allowed = false;
        if name.is_empty() {
            self.name = self.default_name.clone();
            self.user_named = false;
        } else {
            self.name = name;
            self.user_named = true;
        }
    }
    /// Rename mode is beginning: remember what an undo goes back to.
    pub fn start_rename(&mut self) {
        self.agent_tab_cleanup_allowed = false;
        self.prev_name = self.name.clone();
        self.prev_user_named = self.user_named;
    }
    /// A keystroke in rename mode. The tab is the user's from the first one, so the bar
    /// shows what is being typed rather than the project it would otherwise wear.
    pub fn edit_name_by_user(&mut self, edit: impl FnOnce(&mut String)) {
        self.agent_tab_cleanup_allowed = false;
        edit(&mut self.name);
        self.user_named = true;
    }
    /// Rename mode ended without an undo. A name typed away to nothing gives naming back.
    pub fn finish_rename(&mut self) {
        if self.name.is_empty() {
            self.rename_by_user(String::new());
        }
    }
    /// Returns whether anything changed.
    pub fn undo_rename(&mut self) -> bool {
        self.agent_tab_cleanup_allowed = false;
        let changed = self.name != self.prev_name || self.user_named != self.prev_user_named;
        self.name = self.prev_name.clone();
        self.user_named = self.prev_user_named;
        changed
    }
    /// What a lone pane is lending its automatically named tab, or `None` when
    /// it has no explicit title, checkout, or known running program.
    ///
    /// The exit marker is not part of the loan. It describes the pane's fate
    /// rather than its identity, so it belongs to whichever name ends up on the
    /// tab — see [`Tab::exit_marker`].
    pub fn lent_pane_title(&self) -> Option<String> {
        if !self.single_pane_titles() || !self.tab_name_is_automatic() {
            return None;
        }
        let (_, pane) = self.get_selectable_tiled_content_panes().next()?;
        if !pane.can_lend_title() {
            return None;
        }
        // Whole, however long it runs. The cut happens once in `Screen::plugin_tab_names`,
        // against `tab_name_budget`, where this title and the other two names meet the same
        // decorations - doing it here as well would charge the markers twice to the one name
        // that already runs long enough to feel it.
        Some(pane.display_title())
    }

    /// The `[5] ` a lone pane's tab leads its name with - the same label the pane's own frame
    /// carries (`TerminalPane::framed_title`), for the same reason: the id is how the pane is
    /// addressed from outside, and a tab down to one pane is read off the tab bar more often
    /// than off the frame.
    ///
    /// Like [`Tab::exit_marker`] it is not a name and does not compete with the three that are:
    /// it goes on whichever of them won. Empty for a tab holding more than one pane, and for a
    /// lone *plugin* pane, which its frame does not label either.
    ///
    /// A stack is one content pane, so a tab holding one wears the id of the member on screen -
    /// which is the member `--pane-id` would have to name to reach what the tab is showing, and
    /// changes as the stack is stepped through.
    pub fn lone_pane_id_marker(&self) -> String {
        if !self.single_pane_titles() {
            return String::new();
        }
        match self.get_selectable_tiled_content_panes().next() {
            Some((PaneId::Terminal(id), _)) => format!("[{}] ", id),
            _ => String::new(),
        }
    }

    /// The " [ EXITED ] " a lone pane that has stopped adds to its tab's name.
    ///
    /// Separate from the name it lends because it outlives the loan: a pane that
    /// set no title lends nothing and has still exited, and the tab must say so
    /// whether it is wearing a project's name, the user's, or its own.
    pub fn exit_marker(&self) -> String {
        if !self.single_pane_titles() {
            return String::new();
        }
        let Some((_, pane)) = self.get_selectable_tiled_content_panes().next() else {
            return String::new();
        };
        if !(pane.is_held() && pane.exited()) {
            return String::new();
        }
        match pane.exit_status() {
            Some(exit_code) => format!(" [ EXIT CODE: {} ] ", exit_code),
            None => " [ EXITED ] ".to_string(),
        }
    }

    fn resolve_active_pane_scroll(&self, _client_id: ClientId) -> Option<(usize, usize)> {
        if !self.single_pane_titles() {
            return None;
        }
        let (_, pane) = self.get_selectable_tiled_content_panes().next()?;
        let (position, length) = pane.scroll_position();
        if position > 0 || length > 0 {
            Some((position, length))
        } else {
            None
        }
    }
    pub fn emit_active_pane_scroll_if_changed(&mut self) -> Result<()> {
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        let mut plugin_updates = vec![];
        for client_id in connected_clients {
            let active_pane_scroll = self.resolve_active_pane_scroll(client_id);
            if self.last_active_pane_scroll.get(&client_id) != Some(&active_pane_scroll) {
                self.last_active_pane_scroll
                    .insert(client_id, active_pane_scroll);
                plugin_updates.push((
                    None,
                    Some(client_id),
                    Event::ActivePaneScroll(active_pane_scroll),
                ));
            }
        }
        if !plugin_updates.is_empty() {
            self.senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates))
                .with_context(|| "failed to update plugins with active pane scroll".to_string())?;
        }
        Ok(())
    }
    pub fn add_client(&mut self, client_id: ClientId, mode_info: Option<ModeInfo>) -> Result<()> {
        self.agent_tab_cleanup_allowed = false;
        // A client arriving in a tab that already has clients does not go through
        // `visible(true)`, so start its cursor state from scratch.
        self.forget_host_cursor_state(client_id);
        // The departed client's place is for a client with none of its own here. One coming back
        // from another tab keeps its own focus in every layer, including an overlay it dismissed.
        // Read before the overlay below can give it one.
        let has_own_place = self.tiled_panes.get_active_pane_id(client_id).is_some()
            || self.floating_panes.active_pane_id(client_id).is_some()
            || self.overlay_panes.active_pane_id(client_id).is_some();
        let departed = if self.connected_clients.borrow().is_empty() && !has_own_place {
            self.departed_focus
        } else {
            DepartedFocus::default()
        };
        // an overlay is drawn for every client, so a client that joins while one is open must be
        // focused on it - otherwise it sees the dialog and types into the pane behind it. A modal
        // first: it is the one pane that must own the keyboard and cannot learn that it doesn't.
        // Otherwise whatever another client is on - which is `None` for a dismissible overlay that
        // was deliberately defocused, and that stays defocused. This runs before the branch below
        // because the tab's first client returns early when there are no tiled panes.
        if let Some(overlay_to_focus) = self
            .overlay_panes
            .topmost_modal_pane_id()
            .or_else(|| self.overlay_panes.first_active_pane_id())
            .or(departed
                .overlay
                .filter(|pane_id| self.overlay_panes.panes_contain(pane_id)))
        {
            self.overlay_panes
                .focus_pane_for_joining_client(overlay_to_focus, client_id);
        }
        let other_clients_exist_in_tab = { !self.connected_clients.borrow().is_empty() };
        // Focus first, registration second, and in that order on purpose: the pane a joining
        // client is put on is read from the clients already here
        // (`TiledPanes::first_active_pane_id` asks `connected_clients` who is first), so a
        // client registered before that question is asked can be the one answering it - and it
        // has no focused pane yet.
        if other_clients_exist_in_tab {
            if let Some(first_active_tiled_pane_id) = self.tiled_panes.first_active_pane_id() {
                self.tiled_panes
                    .focus_pane_if_client_not_focused(first_active_tiled_pane_id, client_id);
            }
        } else {
            let pane_ids: Vec<PaneId> = self.tiled_panes.pane_ids().copied().collect();
            let focus_pane_id = [departed.tiled, self.focus_pane_id]
                .into_iter()
                .flatten()
                .find(|focus_pane_id| {
                    pane_ids.contains(focus_pane_id)
                        && !self.tiled_panes.panes_to_hide_contains(*focus_pane_id)
                })
                .or_else(|| {
                    self.tiled_panes
                        .get_panes()
                        .filter(|(id, pane)| {
                            pane.selectable() && !self.tiled_panes.panes_to_hide_contains(**id)
                        })
                        .min_by_key(|(id, pane)| {
                            let geom = pane.current_geom();
                            (geom.y, geom.x, **id)
                        })
                        .map(|(id, _)| *id)
                });
            if let Some(focus_pane_id) = focus_pane_id {
                self.tiled_panes
                    .focus_pane_if_client_not_focused(focus_pane_id, client_id);
            }
        }
        match departed
            .floating
            .filter(|pane_id| self.floating_panes.panes_contain(pane_id))
        {
            Some(pane_id) => self.floating_panes.focus_pane(pane_id, client_id),
            None => self.floating_panes.focus_pane_for_joining_client(client_id),
        }
        // Unconditional. A tab with no tiled panes is an ordinary state here - floating panes, an
        // overlay, a pane suppressed behind a hook plugin, a tab still applying its layout - and
        // this used to return `Ok(())` for one without registering the client at all. Callers
        // cannot tell that from success and have already written `Screen::active_tab_ids`, so the
        // client ended up naming a tab that did not count it: `render_tiled_panes_and_chrome`
        // draws for `connected_clients` and returns early on an empty set, so it received no
        // frames, and `close_tab_by_id` re-homes only the clients `drain_connected_clients` hands
        // back, so the next sweep left it naming a tab that no longer existed. Nothing repairs
        // either state short of a reattach. A client with no pane to be focused on simply has
        // none; `TiledPanes::insert_pane` focuses it when a pane arrives.
        self.connected_clients.borrow_mut().insert(client_id);
        self.mode_info.borrow_mut().insert(
            client_id,
            mode_info.unwrap_or_else(|| self.default_mode_info.clone()),
        );
        for pane_id in self.get_all_pane_ids() {
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.add_client(client_id);
            }
        }
        self.ensure_selectable_focus(client_id)?;
        self.set_force_render();
        Ok(())
    }

    /// Keep valid focus, or reveal a selectable destination in a retained tab.
    pub fn ensure_selectable_focus(&mut self, client_id: ClientId) -> Result<()> {
        if self.is_pending {
            return Ok(());
        }
        let focused_is_selectable = self.get_active_pane_id(client_id).is_some_and(|id| {
            self.get_pane_with_id(id)
                .is_some_and(|pane| pane.selectable())
                && self.has_non_suppressed_pane_with_pid(&id)
                && !self.tiled_panes.panes_to_hide_contains(id)
        });
        if !focused_is_selectable {
            let candidate = self
                .tiled_panes
                .get_panes()
                .filter(|(id, pane)| {
                    pane.selectable() && !self.tiled_panes.panes_to_hide_contains(**id)
                })
                .map(|(id, _)| *id)
                .min()
                .or_else(|| {
                    self.floating_panes
                        .get_panes()
                        .filter(|(_, pane)| pane.selectable())
                        .map(|(id, _)| *id)
                        .min()
                })
                .or_else(|| {
                    self.tiled_panes
                        .get_panes()
                        .filter(|(_, pane)| pane.selectable())
                        .map(|(id, _)| *id)
                        .min()
                })
                .or_else(|| self.overlay_panes.selectable_tab_content_pane_id());
            if let Some(id) = candidate {
                if self.floating_panes.get_pane(id).is_some()
                    && self.tiled_panes.fullscreen_covers_ui()
                {
                    self.tiled_panes.unset_fullscreen();
                }
                self.focus_pane_with_id(id, false, false, client_id)?;
                self.set_force_render();
                self.set_should_clear_display_before_rendering();
            }
        }
        Ok(())
    }

    fn repair_focus_after_pane_removal(&mut self) {
        let clients: Vec<_> = self.connected_clients.borrow().iter().copied().collect();
        for client in clients {
            self.ensure_selectable_focus(client)
                .context("failed to repair focus after pane removal")
                .non_fatal();
        }
    }

    pub fn change_mode_info(&mut self, mode_info: ModeInfo, client_id: ClientId) {
        self.mode_info.borrow_mut().insert(client_id, mode_info);
    }

    pub fn add_multiple_clients(
        &mut self,
        client_ids_to_mode_infos: Vec<(ClientId, ModeInfo)>,
    ) -> Result<()> {
        for (client_id, client_mode_info) in client_ids_to_mode_infos {
            self.add_client(client_id, None)
                .context("failed to add clients")?;
            self.mode_info
                .borrow_mut()
                .insert(client_id, client_mode_info);
        }
        Ok(())
    }
    pub fn remove_client(&mut self, client_id: ClientId) {
        let was_connected = self.connected_clients.borrow().contains(&client_id);
        let was_dimmed = self.dimmed_clients.contains(&client_id);
        let changed: Vec<_> = self
            .pane_tabs
            .iter_mut()
            .filter_map(|(id, tabs)| {
                let focused = tabs.info.bar_focused.contains(&client_id);
                tabs.info.bar_focused.retain(|client| *client != client_id);
                let viewed = tabs.views.remove(&client_id).is_some();
                (focused || viewed).then_some(*id)
            })
            .collect();
        for id in changed {
            self.refresh_pane_tabs(id).non_fatal();
        }
        // The entry goes, as `drain_single_client` does: `add_client` supplies a fresh
        // `ModeInfo` on reattach, and one left behind per detach lived as long as the tab.
        self.mode_info.borrow_mut().remove(&client_id);
        self.connected_clients.borrow_mut().remove(&client_id);
        self.mouse_help_text_visible.remove(&client_id);
        self.mouse_last_pane_id.remove(&client_id);
        self.last_mouse_activity_time.remove(&client_id);
        self.forget_host_cursor_state(client_id);
        if was_connected || was_dimmed {
            self.set_client_dimmed(client_id, false);
            self.set_force_render();
        }
    }
    pub fn drain_connected_clients(
        &mut self,
        clients_to_drain: Option<Vec<ClientId>>,
    ) -> Vec<(ClientId, ModeInfo)> {
        // None => all clients
        let mut client_ids_to_mode_infos = vec![];
        let clients_to_drain = clients_to_drain
            .unwrap_or_else(|| self.connected_clients.borrow().iter().copied().collect());
        for client_id in clients_to_drain {
            client_ids_to_mode_infos.push(self.drain_single_client(client_id));
        }
        client_ids_to_mode_infos
    }
    pub fn drain_single_client(&mut self, client_id: ClientId) -> (ClientId, ModeInfo) {
        self.remember_departing_focus(client_id);
        let client_mode_info = self
            .mode_info
            .borrow_mut()
            .remove(&client_id)
            .unwrap_or_else(|| self.default_mode_info.clone());
        self.connected_clients.borrow_mut().remove(&client_id);
        self.forget_host_cursor_state(client_id);
        (client_id, client_mode_info)
    }
    pub fn has_no_connected_clients(&self) -> bool {
        self.connected_clients.borrow().is_empty()
    }
    /// Whether this tab counts `client_id` among the clients it renders for and routes input
    /// to. The other half of `Screen::active_tab_ids`: a client that names this tab and is not
    /// in here gets no output and no input handling.
    pub fn has_connected_client(&self, client_id: &ClientId) -> bool {
        self.connected_clients.borrow().contains(client_id)
    }
    pub fn pane_id_is_floating(&self, pane_id: &PaneId) -> bool {
        self.floating_panes.panes_contain(pane_id)
    }
    pub fn toggle_pane_embed_or_floating(&mut self, client_id: ClientId) -> Result<()> {
        if self
            .get_active_pane_id(client_id)
            .is_some_and(|id| self.multipane_info(id).is_some())
        {
            return Err(anyhow!("cannot embed a multipane child"));
        }
        let err_context =
            || format!("failed to toggle embedded/floating pane for client {client_id}");
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        if self.floating_panes.panes_are_visible() {
            if let Some(focused_floating_pane_id) = self.floating_panes.active_pane_id(client_id)
                && self.tiled_panes.has_room_for_new_pane()
            {
                let floating_pane_to_embed = self
                        .extract_pane(focused_floating_pane_id, true)
                        .with_context(|| format!(
                        "failed to find floating pane (ID: {focused_floating_pane_id:?}) to embed for client {client_id}",
                    ))
                        .with_context(err_context)?;
                self.hide_floating_panes();
                self.add_tiled_pane(
                    floating_pane_to_embed,
                    focused_floating_pane_id,
                    false,
                    Some(client_id),
                )?;
            }
        } else if let Some(focused_pane_id) = self.tiled_panes.focused_pane_id(client_id) {
            if self.get_selectable_tiled_panes().count() <= 1
                && !self.pane_is_stack_list_member(&focused_pane_id)
            {
                // don't close the only pane on screen...
                return Ok(());
            }
            if let Some(embedded_pane_to_float) = self.extract_pane(focused_pane_id, true) {
                self.show_floating_panes();
                self.add_floating_pane(
                    embedded_pane_to_float,
                    focused_pane_id,
                    None,
                    true,
                    Some(client_id),
                )?;
            }
        }
        Ok(())
    }
    pub fn toggle_pane_embed_or_floating_for_pane_id(
        &mut self,
        pane_id: PaneId,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        if self.multipane_info(pane_id).is_some() {
            return Err(anyhow!("cannot embed a multipane child"));
        }
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        let is_floating = self.floating_panes.panes_contain(&pane_id);
        if is_floating {
            if !self.tiled_panes.has_room_for_new_pane() {
                return Err(anyhow!("not enough room to embed pane {pane_id:?}"));
            }
        } else if self.tiled_panes.panes_contain(&pane_id) {
            if self.get_selectable_tiled_panes().count() <= 1
                && !self.pane_is_stack_list_member(&pane_id)
            {
                return Err(anyhow!("cannot float the last tiled pane"));
            }
        } else if !self.pane_is_hidden_stack_list_member(&pane_id) {
            return Err(anyhow!("pane {pane_id:?} not found"));
        }
        let pane = self.extract_pane(pane_id, true).with_context(|| {
            format!("failed to find pane {pane_id:?} to toggle embedded/floating")
        })?;
        if is_floating {
            self.add_tiled_pane(pane, pane_id, false, client_id)?;
        } else {
            self.add_floating_pane(pane, pane_id, None, true, client_id)?;
        }
        Ok(())
    }
    pub fn toggle_floating_panes(
        &mut self,
        client_id: Option<ClientId>,
        default_shell: Option<TerminalAction>,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let last_selectable_floating_pane_id =
            self.floating_panes.last_selectable_floating_pane_id();
        if last_selectable_floating_pane_id.is_some() && self.floating_panes.all_panes_are_visible()
        {
            self.hide_floating_panes();
            self.set_force_render();
        } else {
            self.show_floating_panes();
            match last_selectable_floating_pane_id {
                Some(last_selectable_floating_pane_id) => match client_id {
                    Some(client_id) => {
                        if !self.floating_panes.active_panes_contain(&client_id) {
                            self.floating_panes
                                .focus_pane(last_selectable_floating_pane_id, client_id);
                        }
                    },
                    None => {
                        self.floating_panes
                            .focus_pane_for_all_clients(last_selectable_floating_pane_id);
                    },
                },
                None => {
                    let name = None;
                    let client_id_or_tab_index = match client_id {
                        Some(client_id) => ClientTabIndexOrPaneId::ClientId(client_id),
                        None => ClientTabIndexOrPaneId::TabIndex(self.id),
                    };
                    let should_start_suppressed = false;
                    let instruction = PtyInstruction::SpawnTerminal(
                        default_shell,
                        name,
                        NewPanePlacement::Floating(None),
                        should_start_suppressed,
                        client_id_or_tab_index,
                        completion_tx,
                        false, // set_blocking
                    );
                    self.senders
                        .send_to_pty(instruction)
                        .with_context(|| "failed to open a floating pane for client".to_string())?;
                },
            }
            self.floating_panes.set_force_render();
        }
        self.set_force_render();
        Ok(())
    }

    pub fn get_active_pane(&self, client_id: ClientId) -> Option<&dyn Pane> {
        self.get_active_pane_id(client_id)
            .and_then(|pane_id| self.get_pane_with_id(pane_id))
    }
    pub fn active_pane_is_scrolled(&self, client_id: ClientId) -> bool {
        self.get_active_pane(client_id)
            .map(|pane| pane.is_scrolled())
            .unwrap_or(false)
    }
    /// This tab's pane with `pane_id`, wherever it is standing: tiled, floating, an overlay,
    /// or suppressed under something else.
    ///
    /// Suppressed included on purpose - a pane set aside is still a pane, and a mark aimed at
    /// it must land so that it is right when the pane comes back. A pane id lives in exactly
    /// one layer, so the order the layers are asked in does not matter. For a pane that has
    /// to be on screen, see `rendered_pane`.
    pub fn get_pane_with_id(&self, pane_id: PaneId) -> Option<&dyn Pane> {
        self.rendered_pane(pane_id)
            .or_else(|| self.suppressed_panes.get(&pane_id).map(|p| &*p.pane))
    }
    /// As `get_pane_with_id`, for changing the pane.
    pub fn get_pane_with_id_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn Pane>> {
        self.floating_panes
            .get_pane_mut(pane_id)
            .or_else(|| self.overlay_panes.get_pane_mut(pane_id))
            .or_else(|| self.tiled_panes.get_pane_mut(pane_id))
            .or_else(|| {
                self.suppressed_panes
                    .get_mut(&pane_id)
                    .map(|suppressed| &mut suppressed.pane)
            })
    }
    /// The pane with `pane_id` if it is on screen - tiled, floating or an overlay - and `None`
    /// for one suppressed under something else. What a frame, a cursor or a mouse gesture asks
    /// for: a pane nobody can see has none of those.
    pub fn rendered_pane(&self, pane_id: PaneId) -> Option<&dyn Pane> {
        self.floating_panes
            .get_pane(pane_id)
            .map(Box::as_ref)
            .or_else(|| self.overlay_panes.get_pane(pane_id).map(Box::as_ref))
            .or_else(|| self.tiled_panes.get_pane(pane_id).map(Box::as_ref))
    }
    /// As `rendered_pane`, for changing the pane.
    pub fn rendered_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn Pane>> {
        self.floating_panes
            .get_pane_mut(pane_id)
            .or_else(|| self.overlay_panes.get_pane_mut(pane_id))
            .or_else(|| self.tiled_panes.get_pane_mut(pane_id))
    }
    pub fn get_active_pane_mut(&mut self, client_id: ClientId) -> Option<&mut Box<dyn Pane>> {
        self.get_active_pane_id(client_id)
            .and_then(|active_pane_id| self.rendered_pane_mut(active_pane_id))
    }
    pub fn get_active_pane_or_floating_pane_mut(
        &mut self,
        client_id: ClientId,
    ) -> Option<&mut Box<dyn Pane>> {
        self.get_active_pane_mut(client_id)
    }
    /// Where this client's keyboard goes.
    ///
    /// An overlay comes first, and nothing takes focus from it while it exists. There is
    /// deliberately no layer-visibility test: an overlay is not on the floating layer, and it was
    /// the exception carved into that test - and every path that forgot the exception - that this
    /// collection exists to remove. `overlays_are_rendered` is still consulted so that focus and
    /// what is drawn cannot disagree; the one way it is false for a live overlay is that every
    /// overlay here has been set aside by `hide_pane`, which drops their focus on the way out - so
    /// this filter is a second lock on a door that is already shut, not the thing holding it.
    pub fn get_active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.overlay_panes
            .active_pane_id(client_id)
            .filter(|_| self.overlays_are_rendered())
            .or_else(|| {
                self.floating_panes
                    .get_active_pane_id(client_id)
                    .filter(|_| self.floating_panes.panes_are_visible())
            })
            .or_else(|| self.tiled_panes.get_active_pane_id(client_id))
    }
    /// Share the reference client's tiled focus without changing floating visibility or overlay
    /// focus, before enabling session mirroring.
    pub fn reconcile_tiled_focus(&mut self, reference_client_id: ClientId) {
        if let Some(pane_id) = self.tiled_panes.get_active_pane_id(reference_client_id) {
            self.tiled_panes.focus_pane_for_all_clients(pane_id);
        }
    }
    fn get_active_terminal_id(&self, client_id: ClientId) -> Option<TerminalId> {
        if let Some(PaneId::Terminal(pid)) = self.get_active_pane_id(client_id) {
            Some(pid)
        } else {
            None
        }
    }
    /// Tells a terminal pane that the host terminal gained or lost focus, if the application in
    /// it asked for focus events (DECSET 1004). Plugin panes are left alone.
    pub fn send_host_focus_event_to_pane(&mut self, pane_id: PaneId, focused: bool) {
        let PaneId::Terminal(terminal_id) = pane_id else {
            return;
        };
        let focus_event = self.get_pane_with_id(pane_id).and_then(|pane| {
            if focused {
                pane.focus_event()
            } else {
                pane.unfocus_event()
            }
        });
        if let Some(focus_event) = focus_event {
            self.os_api
                .write_to_tty_stdin(terminal_id, focus_event.as_bytes())
                .with_context(|| format!("failed to send host focus event to pane {}", terminal_id))
                .non_fatal();
        }
    }
    pub fn check_and_handle_bell_notifications(
        &mut self,
        is_active_tab: bool,
    ) -> (Vec<PaneId>, bool) {
        let mut newly_notified_panes = vec![];
        let mut tab_bell_newly_set = false;

        let focused_pane_ids: HashSet<PaneId> = self
            .connected_clients
            .borrow()
            .iter()
            .filter_map(|c_id| self.get_active_pane_id(*c_id))
            .collect();

        // Collect ringing pane IDs first (immutable borrow)
        let ringing_panes: Vec<PaneId> = self
            .tiled_panes
            .get_panes()
            .chain(self.floating_panes.get_panes())
            .chain(self.overlay_panes.get_panes())
            .chain(self.suppressed_stack_list_members())
            .filter(|(_, pane)| pane.has_bell())
            .map(|(pane_id, _)| *pane_id)
            .collect();

        for pane_id in ringing_panes {
            // Consume the bell from the pane
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.consume_bell();
            }
            let is_focused = focused_pane_ids.contains(&pane_id);
            if !is_focused && !self.panes_with_pending_bell.contains(&pane_id) {
                if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                    pane.set_bell_notification(true);
                }
                self.panes_with_pending_bell.insert(pane_id);
                newly_notified_panes.push(pane_id);
            }
            if !self.tab_bell_ring {
                self.tab_bell_ring = true;
                tab_bell_newly_set = true;
            }
            if !is_active_tab {
                self.tab_has_pending_bell = true;
            }
        }
        (newly_notified_panes, tab_bell_newly_set)
    }
    pub fn clear_bell_notification_for_pane(&mut self, pane_id: PaneId) {
        self.panes_with_pending_bell.remove(&pane_id);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_bell_notification(false);
            pane.clear_pane_frame_color_override(None);
        }
        self.tab_has_pending_bell = false;
    }
    pub fn clear_tab_bell_notification(&mut self) {
        self.tab_has_pending_bell = false;
    }
    pub fn clear_tab_bell_ring(&mut self) {
        self.tab_bell_ring = false;
    }
    /// Checks if any pane in the tab has a pending bell, consumes all such bells, and returns
    /// whether any were found. Does not update notification state (used when visual_bell is
    /// disabled but ANSI BEL forwarding is still desired).
    pub fn check_and_consume_bells_without_visual_notification(&mut self) -> bool {
        // the same chain as `check_and_handle_bell_notifications`, overlay link included: with
        // `visual_bell` off this is the *only* thing that sees a bell, so an overlay missing here
        // is a BEL that is neither forwarded to the client nor consumed off the pane
        let ringing_panes: Vec<PaneId> = self
            .tiled_panes
            .get_panes()
            .chain(self.floating_panes.get_panes())
            .chain(self.overlay_panes.get_panes())
            .chain(self.suppressed_stack_list_members())
            .filter(|(_, pane)| pane.has_bell())
            .map(|(pane_id, _)| *pane_id)
            .collect();
        let had_bell = !ringing_panes.is_empty();
        for pane_id in ringing_panes {
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.consume_bell();
            }
        }
        had_bell
    }
    pub fn has_terminal_pid(&self, pid: TerminalId) -> bool {
        self.tiled_panes.panes_contain(&PaneId::Terminal(pid))
            || self.floating_panes.panes_contain(&PaneId::Terminal(pid))
            || self.overlay_panes.panes_contain(&PaneId::Terminal(pid))
            || self.suppressed_panes.contains_key(&PaneId::Terminal(pid))
    }
    pub fn has_plugin(&self, plugin_id: PluginId) -> bool {
        self.tiled_panes.panes_contain(&PaneId::Plugin(plugin_id))
            || self
                .floating_panes
                .panes_contain(&PaneId::Plugin(plugin_id))
            || self.overlay_panes.panes_contain(&PaneId::Plugin(plugin_id))
            || self
                .suppressed_panes
                .contains_key(&PaneId::Plugin(plugin_id))
    }
    pub fn set_pane_color(
        &mut self,
        pane_id: PaneId,
        fg: Option<String>,
        bg: Option<String>,
    ) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_pane_default_colors(fg, bg);
        }
        Ok(())
    }
    /// Replace `pane_id`'s agent record, answering whether it changed. Unchanged is the common
    /// case - a hook reporting `busy` on every tool call - and costs no manifest.
    pub fn set_pane_agent(&mut self, pane_id: PaneId, agent: Option<AgentInfo>) -> bool {
        if agent.is_none() {
            self.pending_agent_escapes
                .retain(|(_, pane), _| *pane != pane_id);
        }
        self.get_pane_with_id_mut(pane_id)
            .map(|pane| pane.set_agent_info(agent))
            .unwrap_or(false)
    }
    /// Whether any pane in this tab needs human input, including hidden panes.
    pub fn has_waiting_agent(&self) -> bool {
        self.tiled_panes
            .get_panes()
            .chain(self.floating_panes.get_panes())
            .chain(self.overlay_panes.get_panes())
            .map(|(_, pane)| pane)
            .chain(
                self.suppressed_panes
                    .values()
                    .map(|suppressed| &suppressed.pane),
            )
            .any(|pane| {
                pane.agent_info()
                    .is_some_and(AgentInfo::is_waiting_on_a_person)
            })
    }
    /// Whether `pane_id` is an agent pane: it has a record, whoever wrote it, or it held one
    /// at some point. The second half is what makes a clean exit fire `agent_exit` - the
    /// agent's own `SessionEnd` hook clears the record a moment before the process goes.
    pub fn pane_is_agent(&self, pane_id: PaneId) -> bool {
        self.get_pane_with_id(pane_id)
            .is_some_and(|pane| pane.agent_info().is_some() || pane.was_an_agent())
    }
    /// `pane_id`'s agent record, wherever the pane is - the screen reads it before a change so
    /// that a hook can be told what the record moved from.
    pub fn pane_last_agent_cwd(&self, pane_id: PaneId) -> Option<String> {
        self.get_pane_with_id(pane_id)
            .and_then(|pane| pane.last_agent_cwd().map(str::to_owned))
    }
    pub fn pane_last_agent_kind(&self, pane_id: PaneId) -> Option<String> {
        self.get_pane_with_id(pane_id)
            .and_then(|pane| pane.last_agent_kind().map(str::to_owned))
    }
    pub fn pane_agent_info(&self, pane_id: PaneId) -> Option<AgentInfo> {
        self.get_pane_with_id(pane_id)
            .and_then(|pane| pane.agent_info().cloned())
    }
    /// The command `pane_id` runs, for a command pane; `None` for a shell, an editor or a
    /// plugin pane. What tells a command pane's exit apart from any other close.
    pub fn pane_run_command(&self, pane_id: PaneId) -> Option<RunCommand> {
        self.get_pane_with_id(pane_id)
            .and_then(|pane| match pane.invoked_with() {
                Some(Run::Command(run_command)) => Some(run_command.clone()),
                _ => None,
            })
    }
    pub fn has_pane_with_pid(&self, pid: &PaneId) -> bool {
        self.tiled_panes.panes_contain(pid)
            || self.floating_panes.panes_contain(pid)
            || self.overlay_panes.panes_contain(pid)
            || self.suppressed_panes.contains_key(pid)
    }
    pub fn has_non_suppressed_pane_with_pid(&self, pid: &PaneId) -> bool {
        self.tiled_panes.panes_contain(pid)
            || self.floating_panes.panes_contain(pid)
            || self.overlay_panes.panes_contain(pid)
    }
    pub fn set_pane_is_nested_guest(&mut self, pane_id: PaneId, is_nested_guest: bool) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_is_nested_guest(is_nested_guest);
        }
    }
    pub fn is_pane_nested_guest(&self, pane_id: PaneId) -> bool {
        if let Some(pane) = self.get_pane_with_id(pane_id) {
            pane.is_nested_guest()
        } else {
            false
        }
    }
    pub fn nested_guest_pane_ids(&self) -> Vec<PaneId> {
        self.get_all_pane_ids()
            .into_iter()
            .filter(|pane_id| self.is_pane_nested_guest(*pane_id))
            .collect()
    }
    pub fn set_guest_modal_on_pane(&mut self, pane_id: PaneId, client_ids: &[ClientId]) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_guest_modal(client_ids);
        }
    }
    pub fn clear_guest_modal_on_pane(&mut self, pane_id: PaneId, client_id: ClientId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_guest_modal(client_id);
        }
    }
    pub fn clear_all_guest_modals_on_pane(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_all_guest_modals();
        }
    }
    pub fn set_guest_choice_indicator_on_pane(
        &mut self,
        pane_id: PaneId,
        client_id: ClientId,
        indicator: Option<GuestChoiceIndicator>,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_guest_choice_indicator(client_id, indicator);
        }
    }
    pub fn clear_all_guest_choice_indicators_on_pane(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_all_guest_choice_indicators();
        }
    }
    pub fn pane_has_guest_modal_for_client(&self, pane_id: PaneId, client_id: ClientId) -> bool {
        if let Some(pane) = self.get_pane_with_id(pane_id) {
            pane.guest_modal_selection(client_id).is_some()
        } else {
            false
        }
    }
    /// Note where a leaving client was, if it is the last client here, for the next to join.
    /// Every tab is asked, so one the client is not in keeps what it has.
    pub fn remember_departing_focus(&mut self, client_id: ClientId) {
        let last_here = {
            let clients = self.connected_clients.borrow();
            clients.len() == 1 && clients.contains(&client_id)
        };
        if last_here {
            self.departed_focus = DepartedFocus {
                overlay: self.overlay_panes.active_pane_id(client_id),
                floating: self.floating_panes.get_active_pane_id(client_id),
                tiled: self.tiled_panes.get_active_pane_id(client_id),
            };
        }
    }
    pub fn remove_client_pane_state(&mut self, client_id: ClientId) {
        self.tiled_panes.forget_client(client_id);
        self.floating_panes.forget_client(client_id);
        self.overlay_panes.forget_client(client_id);
        self.forget_host_cursor_state(client_id);
        self.invalidate_boundaries(client_id);
        if let Some(pane_id) = self.mouse_hover_pane_id.remove(&client_id)
            && !self.mouse_hover_pane_id.values().any(|id| *id == pane_id)
            && let Some(pane) = self.get_pane_with_id_mut(pane_id)
        {
            pane.set_hover_position(None);
        }
        self.plugin_hover_pane_id.remove(&client_id);
        self.keyboard_text_selection.remove(&client_id);
        self.last_hint_text.remove(&client_id);
        self.last_active_pane_scroll.remove(&client_id);
        self.pending_agent_escapes
            .retain(|(client, _), _| *client != client_id);
        for pane_id in self.get_all_pane_ids() {
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.remove_client(client_id);
            }
        }
    }
    pub fn clear_guest_modal_for_client_on_all_panes(&mut self, client_id: ClientId) {
        for pane_id in self.get_all_pane_ids() {
            self.clear_guest_modal_on_pane(pane_id, client_id);
        }
    }
    pub fn clear_guest_choice_indicator_for_client_on_all_panes(&mut self, client_id: ClientId) {
        for pane_id in self.get_all_pane_ids() {
            self.set_guest_choice_indicator_on_pane(pane_id, client_id, None);
        }
    }
    pub fn set_guest_session_name_on_pane(
        &mut self,
        pane_id: PaneId,
        session_name: Option<SessionName>,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_guest_session_name(session_name);
        }
    }
    pub fn set_guest_modal_shortcuts_on_pane(
        &mut self,
        pane_id: PaneId,
        shortcuts: GuestModalShortcuts,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_guest_modal_shortcuts(shortcuts);
        }
    }

    pub fn active_terminal_is_mid_frame(&self, client_id: ClientId) -> Option<bool> {
        let active_pane_id = self.get_active_pane_id(client_id)?;
        let active_terminal = self.rendered_pane(active_pane_id)?;
        Some(active_terminal.is_mid_frame())
    }
    pub fn get_active_terminal_cursor_position(
        &self,
        client_id: ClientId,
    ) -> Option<(usize, usize, bool)> {
        // (x, y, is_cursor_visible)
        let active_pane_id = self.get_active_pane_id(client_id)?;
        let active_terminal = self.rendered_pane(active_pane_id)?;
        active_terminal.cursor_coordinates(Some(client_id)).map(
            |(x_in_terminal, y_in_terminal, is_visible)| {
                let x = active_terminal.render_geom().x + x_in_terminal;
                let y = active_terminal.y() + y_in_terminal;
                (x, y, is_visible)
            },
        )
    }
    /// A fullscreen keystroke that landed on an overlay is the overlay's, and is answered here
    /// rather than falling through to another collection.
    ///
    /// This used to refuse outright - `OverlayPanes` had no expansion of its own, and before even
    /// the refusal the by-id paths logged "no tiled pane with id" while the active-pane paths
    /// silently fullscreened the tiled pane *behind* the overlay the user was looking at. An
    /// overlay can now be expanded over the screen (`OverlayPanes::set_fullscreen`), so the
    /// keystroke is dispatched instead of dropped. A **modal** is still refused, for the reason
    /// it cannot be resized: it is sized to the question it is asking.
    ///
    /// `true` => the keystroke was the overlays', answered or refused, and no other collection
    /// may see it.
    fn overlay_takes_fullscreen(&mut self, pane_id: PaneId, covers_ui: bool) -> bool {
        if !self.overlay_panes.panes_contain(&pane_id) {
            return false;
        }
        // what the repaint below is about is the expansion *changing*, which is not the same as
        // the toggle answering `true`: a press naming an overlay that may not be expanded - a
        // modal, a hidden one, one still awaiting its placement - puts the expansion that was
        // there back and then refuses, and the screenful it gave up has to be repainted all the
        // same. Without this that press left a ghost of it over the panes underneath.
        let was_expanded = self.overlay_panes.fullscreen_is_active();
        let toggled = if covers_ui {
            self.overlay_panes.toggle_pane_no_ui_fullscreen(pane_id)
        } else {
            self.overlay_panes.toggle_pane_fullscreen(pane_id)
        };
        if toggled || was_expanded {
            // the overlay is painted last, over a tab that has just had a screenful of it taken
            // away, so everything it was covering has to be asked for again
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
        true
    }
    /// The same, for the collection's own idea of which pane this client's keystroke is about -
    /// the expanded overlay if there is one, so a second press always collapses it.
    ///
    /// A client whose keyboard is *not* on an overlay may only ever **collapse** an expansion,
    /// never begin one and never switch between the two variants: an overlay is expanded over the
    /// tab, so the other clients on it are looking at a screenful of someone else's pane, and the
    /// keystroke that gets it off their screen must exist. What must not is a bystander re-sizing
    /// an expansion they never asked for - so once it is put back, the next press falls through
    /// to the layers underneath and fullscreens the pane this client is actually on.
    fn active_overlay_takes_fullscreen(&mut self, client_id: ClientId, covers_ui: bool) -> bool {
        let on_an_overlay = self
            .get_active_pane_id(client_id)
            .is_some_and(|pane_id| self.overlay_panes.panes_contain(&pane_id));
        // as in `overlay_takes_fullscreen`: the repaint is owed to the expansion changing, not to
        // the keystroke being answered
        let was_expanded = self.overlay_panes.fullscreen_is_active();
        let claimed = if on_an_overlay {
            if covers_ui {
                self.overlay_panes
                    .toggle_active_pane_no_ui_fullscreen(client_id)
            } else {
                self.overlay_panes.toggle_active_pane_fullscreen(client_id)
            }
        } else {
            self.overlay_panes.unset_fullscreen()
        };
        if claimed || was_expanded {
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
        on_an_overlay || claimed
    }
    pub fn toggle_active_pane_fullscreen(&mut self, client_id: ClientId) {
        if self.active_overlay_takes_fullscreen(client_id, false) {
            return;
        }
        if self.floating_panes.fullscreen_is_active()
            || (self.floating_panes.panes_are_visible() && self.floating_panes.has_active_panes())
        {
            self.floating_panes.toggle_active_pane_fullscreen(client_id);
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
            return;
        }
        if self.floating_panes.panes_are_visible() {
            return;
        }
        self.dissolve_stack_lists_for_classic_mutation();
        self.tiled_panes.toggle_active_pane_fullscreen(client_id);
    }
    pub fn toggle_pane_fullscreen(&mut self, pane_id: PaneId) {
        if self.overlay_takes_fullscreen(pane_id, false) {
            return;
        }
        if self.floating_panes.panes_contain(&pane_id) {
            self.floating_panes.toggle_pane_fullscreen(pane_id);
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
            return;
        }
        if self.pane_is_hidden_stack_list_member(&pane_id) {
            self.make_hidden_stack_list_member_visible(pane_id);
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if self.tiled_panes.panes_contain(&pane_id) {
            self.tiled_panes.toggle_pane_fullscreen(pane_id);
        } else {
            log::error!("No tiled pane with id: {:?} found", pane_id);
        }
    }
    pub fn toggle_active_pane_no_ui_fullscreen(&mut self, client_id: ClientId) {
        if self.active_overlay_takes_fullscreen(client_id, true) {
            return;
        }
        if self.floating_panes.fullscreen_is_active()
            || (self.floating_panes.panes_are_visible() && self.floating_panes.has_active_panes())
        {
            self.floating_panes
                .toggle_active_pane_no_ui_fullscreen(client_id);
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
            return;
        }
        if self.floating_panes.panes_are_visible() {
            return;
        }
        self.dissolve_stack_lists_for_classic_mutation();
        self.tiled_panes
            .toggle_active_pane_no_ui_fullscreen(client_id);
    }
    pub fn toggle_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) {
        if self.overlay_takes_fullscreen(pane_id, true) {
            return;
        }
        if self.floating_panes.panes_contain(&pane_id) {
            self.floating_panes.toggle_pane_no_ui_fullscreen(pane_id);
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
            return;
        }
        if self.pane_is_hidden_stack_list_member(&pane_id) {
            self.make_hidden_stack_list_member_visible(pane_id);
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if self.tiled_panes.panes_contain(&pane_id) {
            self.tiled_panes.toggle_pane_no_ui_fullscreen(pane_id);
        } else {
            log::error!("No tiled pane with id: {:?} found", pane_id);
        }
    }
    /// Put back *everything* that is expanded: an overlay over the screen, and the fullscreen of
    /// whichever layer underneath it has one.
    ///
    /// Not a dispatch, unlike the four toggles above - this is the "there must be nothing
    /// fullscreen after this" statement its callers make before they relayout, so an overlay
    /// collapsing must not stand in for the tiled pane they were asking about. The two split
    /// paths guard on `tiled_panes.fullscreen_is_active()` and then split into a layout that is
    /// still collapsed to one pane if this returned early.
    pub fn unset_fullscreen(&mut self) {
        if self.overlay_panes.unset_fullscreen() {
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
        if self.floating_panes.fullscreen_is_active() {
            self.floating_panes.unset_fullscreen();
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
            return;
        }
        if self.floating_panes.panes_are_visible() {
            return;
        }
        self.dissolve_stack_lists_for_classic_mutation();
        self.tiled_panes.unset_fullscreen();
    }
    pub fn is_fullscreen_active(&self) -> bool {
        self.tiled_panes.fullscreen_is_active() || self.floating_panes.fullscreen_is_active()
    }
    /// Whether *something* is drawn over the tab bar and the status bar.
    ///
    /// The overlays are in here, unlike in `is_fullscreen_active` above: this is a question about
    /// what is painted, which an expanded overlay is as much a part of as a fullscreen tiled pane.
    /// The readers that mean "the tiled layout is collapsed" ask `tiled_panes` directly, and the
    /// one that means "this guest pane's own fullscreen" asks `tiled_or_floating_fullscreen`
    /// below - which is why this pair is at present read only by the tests that assert it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fullscreen_covers_ui(&self) -> bool {
        self.tiled_panes.fullscreen_covers_ui()
            || self.floating_panes.fullscreen_covers_ui()
            || self.overlay_panes.fullscreen_covers_ui()
    }
    /// The pane a fullscreen is drawn for, whichever collection holds it - the tab-wide question,
    /// as `fullscreen_covers_ui` above is. See its note on who reads them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fullscreen_pane_id(&self) -> Option<PaneId> {
        self.overlay_panes
            .fullscreen_pane_id()
            .or_else(|| self.tiled_panes.fullscreen_pane_id())
            .or_else(|| self.floating_panes.fullscreen_pane_id())
    }
    /// The tiled or floating pane that is fullscreen, and whether *its* fullscreen covers the
    /// chrome - asked of the collection that owns it rather than of the tab-wide disjunctions
    /// above, which mix the layers and would answer about one pane's fullscreen with another's.
    ///
    /// This is what a **nested guest**'s fullscreen state is about. A guest is a terminal pane and
    /// never an overlay, so an expanded overlay is neither a fullscreen the guest protocol should
    /// report nor one it should displace - and an overlay drawn over a fullscreen guest does not
    /// make the guest any less fullscreen.
    pub fn tiled_or_floating_fullscreen(&self) -> Option<(PaneId, bool)> {
        self.tiled_panes
            .fullscreen_pane_id()
            .map(|pane_id| (pane_id, self.tiled_panes.fullscreen_covers_ui()))
            .or_else(|| {
                self.floating_panes
                    .fullscreen_pane_id()
                    .map(|pane_id| (pane_id, self.floating_panes.fullscreen_covers_ui()))
            })
    }
    pub fn are_floating_panes_visible(&self) -> bool {
        self.floating_panes.all_panes_are_visible()
    }
    pub fn focus_pane_left_fullscreen(&mut self, client_id: ClientId) -> bool {
        if !self.is_fullscreen_active() {
            return false;
        }

        self.tiled_panes.focus_pane_left_fullscreen(client_id)
    }
    pub fn focus_pane_right_fullscreen(&mut self, client_id: ClientId) -> bool {
        if !self.is_fullscreen_active() {
            return false;
        }

        self.tiled_panes.focus_pane_right_fullscreen(client_id)
    }
    pub fn focus_pane_up_fullscreen(&mut self, client_id: ClientId) -> bool {
        if !self.is_fullscreen_active() {
            return false;
        }

        self.tiled_panes.focus_pane_up_fullscreen(client_id)
    }
    pub fn focus_pane_down_fullscreen(&mut self, client_id: ClientId) -> bool {
        if !self.is_fullscreen_active() {
            return false;
        }

        self.tiled_panes.focus_pane_down_fullscreen(client_id)
    }
    pub fn switch_next_pane_fullscreen(&mut self, client_id: ClientId) {
        if !self.is_fullscreen_active() {
            return;
        }
        self.tiled_panes.switch_next_pane_fullscreen(client_id);
    }
    pub fn switch_prev_pane_fullscreen(&mut self, client_id: ClientId) {
        if !self.is_fullscreen_active() {
            return;
        }
        self.tiled_panes.switch_prev_pane_fullscreen(client_id);
    }
    pub fn switch_last_pane_fullscreen(&mut self, client_id: ClientId) {
        if !self.is_fullscreen_active() {
            return;
        }
        self.tiled_panes.switch_last_pane_fullscreen(client_id);
    }
    pub fn invalidate_boundaries(&mut self, client_id: ClientId) {
        self.tiled_panes.invalidate_boundaries(client_id);
    }
    pub fn prepare_margin_replay(&mut self, damage: &[CellRect]) {
        self.pending_render_damage.extend_from_slice(damage);
    }
    pub fn set_force_render(&mut self) {
        let display_area = *self.display_area.borrow();
        self.pending_render_damage.clear();
        self.pending_render_damage.push(CellRect {
            x: 0,
            y: 0,
            cols: display_area.cols,
            rows: display_area.rows,
        });
        self.tiled_panes.set_force_render();
        self.floating_panes.set_force_render();
        self.overlay_panes.set_force_render();
        for member in self.stack_list_of_member.keys() {
            if let Some(pane) = self.suppressed_panes.get_mut(member).map(|s| &mut s.pane) {
                pane.set_should_render(true);
                pane.render_full_viewport();
            }
        }
    }

    /// Repaint only the panes overlapping `damage`.
    ///
    /// The narrowed twin of `set_force_render` above, for the cells an overlay - a toast - stopped
    /// covering as it moved. Suppressed stack members are deliberately left out: they are not on
    /// screen, so nothing can have been uncovered over them.
    pub fn force_render_panes_intersecting(&mut self, damage: &[CellRect]) {
        if damage.is_empty() {
            return;
        }
        self.pending_render_damage.extend_from_slice(damage);
        self.tiled_panes.force_render_panes_intersecting(damage);
        self.floating_panes.force_render_panes_intersecting(damage);
        self.overlay_panes.force_render_panes_intersecting(damage);
    }

    pub fn should_clear_display_before_rendering(&self) -> bool {
        self.should_clear_display_before_rendering
    }
    pub fn set_should_clear_display_before_rendering(&mut self) {
        self.should_clear_display_before_rendering = true;
        self.floating_panes.set_force_render(); // we do this to make sure pinned panes are
                                                // rendered even if their surface is not visible
                                                // the overlays draw after the clear is emitted (see
                                                // `render_floating_panes_and_overlays`), so an overlay that is already on screen and
                                                // clean would contribute nothing to the frame that wipes it - leaving a modal invisible
                                                // while it still holds the keyboard
        self.overlay_panes.set_force_render();
    }
    pub fn is_sync_panes_active(&self) -> bool {
        self.synchronize_is_active
    }
    pub fn toggle_sync_panes_is_active(&mut self) {
        self.synchronize_is_active = !self.synchronize_is_active;
    }
    pub fn mark_active_pane_for_rerender(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_mut(client_id) {
            active_pane.set_should_render(true);
        }
    }
    fn update_active_panes_in_pty_thread(&self) -> Result<()> {
        // this is a bit hacky and we should ideally not keep this state in two different places at
        // some point
        let connected_clients: Vec<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        for client_id in connected_clients {
            self.senders
                .send_to_pty(PtyInstruction::UpdateActivePane(
                    self.get_active_pane_id(client_id),
                    client_id,
                ))
                .with_context(|| format!("failed to update active pane for client {client_id}"))?;
        }
        Ok(())
    }

    pub fn get_tiled_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.tiled_panes.get_panes()
    }
    pub fn get_floating_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.floating_panes.get_panes()
    }
    /// Resolve a visible selection to its list and every replacement behind it.
    pub fn warp_pane_ids(&self, selected: Option<PaneId>) -> Result<Vec<PaneId>, String> {
        let mut ids = match selected {
            Some(id) => {
                if !self
                    .get_tiled_panes()
                    .chain(self.get_floating_panes())
                    .any(|(p, _)| *p == id)
                {
                    return Err("only visible panes can initiate a warp".into());
                }
                self.stack_list_id_of_member(&id)
                    .and_then(|id| self.stack_lists.get(&id))
                    .map(|list| list.members.clone())
                    .unwrap_or_else(|| vec![id])
            },
            None => self
                .get_tiled_panes()
                .chain(self.get_floating_panes())
                .map(|(id, _)| *id)
                .chain(self.suppressed_panes.iter().filter_map(|(id, pane)| {
                    (matches!(id, PaneId::Terminal(_)) && pane.restored_by.is_none()).then_some(*id)
                }))
                .collect(),
        };
        loop {
            let before = ids.len();
            for list in self.stack_lists.values() {
                if ids.contains(&list.visible) {
                    for id in &list.members {
                        if !ids.contains(id) {
                            ids.push(*id);
                        }
                    }
                }
            }
            for (id, pane) in &self.suppressed_panes {
                if pane
                    .restored_by
                    .is_some_and(|cover| ids.contains(&cover.pane_id))
                    && !ids.contains(id)
                {
                    ids.push(*id);
                }
            }
            if before == ids.len() {
                break;
            }
        }
        Ok(ids)
    }

    pub fn warp_relationships(&self, slots: &[PaneId]) -> yinz_types::data::WarpRelationships {
        use yinz_types::data::{WarpHiddenPane, WarpRelationships, WarpStackList};
        let index = |id: PaneId| slots.iter().position(|p| *p == id).map(|i| i as u32);
        WarpRelationships {
            hidden: slots
                .iter()
                .enumerate()
                .filter_map(|(slot, id)| {
                    self.suppressed_panes.get(id).map(|pane| WarpHiddenPane {
                        pane: slot as u32,
                        restored_by: pane.restored_by.and_then(|cover| index(cover.pane_id)),
                        is_scrollback_editor: pane
                            .restored_by
                            .is_some_and(|cover| cover.is_scrollback_editor),
                    })
                })
                .collect(),
            stacks: self
                .stack_lists
                .values()
                .filter_map(|list| {
                    index(list.visible).map(|selected| WarpStackList {
                        members: list.members.iter().filter_map(|id| index(*id)).collect(),
                        selected,
                    })
                })
                .collect(),
        }
    }

    /// Remove a transfer as a unit: neither covers nor list members promote a replacement.
    pub fn extract_warp_panes(&mut self, ids: &[PaneId]) -> Vec<Box<dyn Pane>> {
        self.prepare_arrangement();
        let lists: Vec<_> = self
            .stack_lists
            .iter()
            .filter(|(_, list)| ids.contains(&list.visible))
            .map(|(id, list)| (*id, list.members.clone()))
            .collect();
        for (id, members) in lists {
            self.drop_stack_list_ledger(id, &members);
        }
        ids.iter()
            .filter_map(|id| self.extract_pane(*id, true))
            .collect()
    }

    pub fn restore_warp_relationships(
        &mut self,
        slots: &[PaneId],
        relationships: &yinz_types::data::WarpRelationships,
    ) -> Result<()> {
        relationships
            .validate(slots.len())
            .map_err(|e| anyhow!("{e}"))?;
        if slots.iter().any(|id| self.get_pane_with_id(*id).is_none()) {
            return Err(anyhow!(
                "a transferred pane is missing during relationship restoration"
            ));
        }
        self.prepare_arrangement();
        for hidden in &relationships.hidden {
            let id = slots[hidden.pane as usize];
            let pane = self
                .extract_pane(id, true)
                .context("cannot hide transferred companion")?;
            self.suppressed_panes.insert(
                id,
                SuppressedPane {
                    pane,
                    // A warp does not carry `lent`: a lent slot arrives as a plain suppression.
                    restored_by: hidden.restored_by.map(|cover| StandIn {
                        pane_id: slots[cover as usize],
                        is_scrollback_editor: hidden.is_scrollback_editor,
                        lent: false,
                    }),
                },
            );
        }
        let stacks: Vec<_> = relationships
            .stacks
            .iter()
            .map(|stack| StackList {
                members: stack.members.iter().map(|id| slots[*id as usize]).collect(),
                visible: slots[stack.selected as usize],
            })
            .collect();
        // Members are already parked; only rebuild the ledger. Extracting them again
        // would discard the replacement links on hidden editors.
        for stack in stacks {
            let id = self.next_stack_list_id;
            self.next_stack_list_id += 1;
            for member in &stack.members {
                self.stack_list_of_member.insert(*member, id);
            }
            self.stack_lists.insert(id, stack);
            self.refresh_or_dissolve_stack_list(id);
        }
        self.set_force_render();
        Ok(())
    }

    pub fn get_suppressed_panes(&self) -> impl Iterator<Item = (&PaneId, &SuppressedPane)> {
        self.suppressed_panes.iter()
    }
    fn get_selectable_tiled_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.get_tiled_panes().filter(|(_, p)| p.selectable())
    }
    fn get_selectable_floating_panes(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.get_floating_panes().filter(|(_, p)| p.selectable())
    }
    pub fn get_selectable_tiled_panes_count(&self) -> usize {
        self.get_selectable_tiled_panes().count()
    }
    pub fn get_selectable_floating_panes_count(&self) -> usize {
        self.get_selectable_floating_panes().count()
    }
    pub fn get_visible_selectable_floating_panes_count(&self) -> usize {
        if self.are_floating_panes_visible() {
            self.get_selectable_floating_panes().count()
        } else {
            0
        }
    }
    fn get_next_terminal_position(&self) -> usize {
        let tiled_panes_count = self
            .tiled_panes
            .get_panes()
            .filter(|(k, _)| match k {
                PaneId::Plugin(_) => false,
                PaneId::Terminal(_) => true,
            })
            .count();
        let floating_panes_count = self
            .floating_panes
            .get_panes()
            .filter(|(k, _)| match k {
                PaneId::Plugin(_) => false,
                PaneId::Terminal(_) => true,
            })
            .count();
        let hidden_stack_list_members_count = self
            .suppressed_stack_list_members()
            .filter(|(pane_id, _)| matches!(pane_id, PaneId::Terminal(_)))
            .count();
        tiled_panes_count + floating_panes_count + hidden_stack_list_members_count + 1
    }
    /// Recover one parked terminal only when explicit tab navigation has no selectable target.
    pub fn recover_suppressed_terminal_for_navigation(
        &mut self,
        client_id: ClientId,
    ) -> Result<bool> {
        if self.is_pending || self.has_selectable_panes() {
            return Ok(false);
        }
        let pane_id = self
            .suppressed_panes
            .iter()
            .filter_map(|(id, suppressed)| {
                let pane = &suppressed.pane;
                (matches!(id, PaneId::Terminal(_))
                    && suppressed.restored_by.is_none()
                    && pane.selectable()
                    && !pane.is_held()
                    && !pane.exited()
                    && !run_requests_overlay(pane.invoked_with())
                    && !self.pane_is_stack_list_member(id)
                    && self.pane_tabs_of(*id).is_none())
                .then_some(*id)
            })
            .min();
        let Some(pane_id) = pane_id else {
            return Ok(false);
        };
        // Focus takes ownership out of suppression, so establish capacity before calling it.
        self.floating_panes
            .find_room_for_new_pane()
            .context("no room to recover hidden terminal; leaving it suppressed")?;
        self.focus_pane_with_id(pane_id, true, false, client_id)
            .context("failed to focus recovered terminal")?;
        Ok(true)
    }

    pub fn has_selectable_panes(&self) -> bool {
        let selectable_tiled_panes = self.tiled_panes.get_panes().filter(|(_, p)| p.selectable());
        let selectable_floating_panes = self
            .floating_panes
            .get_panes()
            .filter(|(_, p)| p.selectable());
        selectable_tiled_panes.count() > 0
            || selectable_floating_panes.count() > 0
            || self.overlay_panes.has_selectable_tab_content()
    }
    /// Retain hidden live work as well as selectable panes. Screen settlement handles the
    /// separate case where no tab provides a usable destination.
    pub fn should_retain_tab(&self) -> bool {
        self.has_selectable_panes() || self.has_live_terminal()
    }
    /// Whether a terminal in this tab - shown or hidden - is still running. Hidden terminals
    /// retain their tab while another usable tab remains, but do not keep the session alive.
    pub fn has_live_terminal(&self) -> bool {
        self.get_all_pane_ids().iter().any(|id| {
            matches!(id, PaneId::Terminal(_))
                && self
                    .get_pane_with_id(*id)
                    .is_some_and(|pane| !pane.exited())
        })
    }

    pub fn set_pane_subtitle(&mut self, pane_id: PaneId, subtitle: Option<String>) -> Result<bool> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let subtitle = subtitle
                .map(|text| yinz_types::text::clean_string_from_control_and_linebreak(&text))
                .filter(|text| !text.is_empty());
            if pane.subtitle() == subtitle.as_deref() {
                return Ok(false);
            }
            let presence_changed = pane.subtitle().is_some() != subtitle.is_some();
            pane.set_subtitle(subtitle);
            if !presence_changed {
                return Ok(false);
            }
            self.tiled_panes.reapply_pane_frames();
            self.floating_panes.set_pane_frames()?;
            self.resize_pty_all_panes()?;
            self.set_force_render();
            return Ok(true);
        }
        Ok(false)
    }

    pub fn resize_pty_all_panes(&mut self) -> Result<()> {
        let err_context = || format!("failed to resize PTYs of all panes in tab {}", self.id);
        self.tiled_panes
            .resize_pty_all_panes()
            .with_context(err_context)?;
        self.floating_panes
            .resize_pty_all_panes(&mut self.os_api)
            .with_context(err_context)?;
        self.overlay_panes
            .resize_pty_all_panes()
            .with_context(err_context)?;
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.character_cell_size_possibly_changed();
            resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                .with_context(err_context)?;
        }
        Ok(())
    }
    pub fn resize_whole_tab(&mut self, new_screen_size: Size) -> Result<()> {
        self.resize_whole_tab_to(new_screen_size)
    }
    pub fn set_tab_group_count(&mut self, count: usize) -> Result<()> {
        self.tab_group_count = count;
        self.sync_tab_bar_height()
    }
    pub fn resize_whole_tab_to(&mut self, display_area: Size) -> Result<()> {
        // a floating pane outside the tab's viewport (over the tab bar, say) is not laid out by
        // the solver at all - it is taken out and put back once the tab has been resized
        let floating_panes_outside_viewport = self.floating_panes.geoms_outside_viewport();
        self.resize_whole_tab_locally(display_area)?;
        self.floating_panes
            .restore_geoms_outside_viewport(floating_panes_outside_viewport);
        Ok(())
    }
    fn resize_whole_tab_locally(&mut self, new_screen_size: Size) -> Result<()> {
        let err_context = || format!("failed to resize whole tab (id {})", self.id);
        self.size = new_screen_size;
        // If a tiled pane is fullscreen, exit fullscreen first so that *all*
        // tiled panes (including the currently hidden ones) participate in the
        // resize/relayout. We re-enter fullscreen on the same pane after the
        // resize so the user-visible state is preserved. Without this, hidden
        // panes retain stale geometry from before the resize and the layout
        // solver fails when fullscreen is later toggled off.
        let fullscreen_pane_to_restore = self.tiled_panes.fullscreen_pane_id();
        let fullscreen_covered_ui = self.tiled_panes.fullscreen_covers_ui();
        if fullscreen_pane_to_restore.is_some() {
            self.tiled_panes.unset_fullscreen();
        }
        let floating_fullscreen_pane_to_restore = self.floating_panes.fullscreen_pane_id();
        let floating_fullscreen_covered_ui = self.floating_panes.fullscreen_covers_ui();
        if floating_fullscreen_pane_to_restore.is_some() {
            self.floating_panes.unset_fullscreen();
        }
        self.floating_panes.resize(new_screen_size);
        // we need to do this explicitly because floating_panes.resize does not do this
        self.floating_panes
            .resize_pty_all_panes(&mut self.os_api)
            .with_context(err_context)?;
        self.tiled_panes.resize(new_screen_size);
        // after `tiled_panes.resize`, which is what updates the shared `display_area` cell the
        // surface is computed from. `FloatingPanes::reapply_overlay_coordinates` used to run from
        // inside `floating_panes.resize` above, and so re-resolved every overlay against the
        // surface as it was *before* the resize - a 60%-wide overlay kept its old width.
        self.overlay_panes.reapply_coordinates();
        if self.auto_layout && !self.swap_layouts.is_floating_damaged() {
            // we do this only for floating panes, because the constraint system takes care of the
            // tiled panes
            self.swap_layouts.set_is_floating_damaged();
            let _ = self.relayout_floating_panes(false);
        }
        if self.auto_layout
            && !self.swap_layouts.is_tiled_damaged()
            && fullscreen_pane_to_restore.is_none()
        {
            self.swap_layouts.set_is_tiled_damaged();
            let _ = self.relayout_tiled_panes(false);
        }
        // The screen's ResizeCache flushes once chrome and pane geometry have both settled.
        let group_count = self.visible_tab_group_count();
        self.tiled_panes.resize_tab_bar(group_count).non_fatal();
        self.set_should_clear_display_before_rendering();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::StartCachingResizes)
            .context("keep pane resizes batched until the screen event ends")?;
        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            &mut self.tiled_panes,
            self.pane_frame_style,
        );
        if let Some(pane_id) = fullscreen_pane_to_restore
            && self.tiled_panes.panes_contain(&pane_id)
        {
            if fullscreen_covered_ui {
                self.tiled_panes.toggle_pane_no_ui_fullscreen(pane_id);
            } else {
                self.tiled_panes.toggle_pane_fullscreen(pane_id);
            }
        }
        if let Some(pane_id) = floating_fullscreen_pane_to_restore
            && self.floating_panes.panes_contain(&pane_id)
        {
            if floating_fullscreen_covered_ui {
                self.floating_panes.toggle_pane_no_ui_fullscreen(pane_id);
            } else {
                self.floating_panes.toggle_pane_fullscreen(pane_id);
            }
        }
        // `reapply_coordinates` above re-derived any expansion against `self.viewport` as it was
        // *before* `LayoutApplier::offset_viewport` recomputed it. Called here rather than beside
        // that recompute because the viewport is only final now: the fullscreen restores above
        // are the last thing that can move it.
        self.refresh_overlay_expansion();
        self.resize_all_stack_list_hidden_members();
        Ok(())
    }
    /// Re-derive an overlay expansion against `self.viewport` as it is now.
    ///
    /// `OverlayPanes::set_pane_frames` is `refresh_fullscreen`'s one caller, so every path that
    /// recomputes the viewport - all of them through `LayoutApplier::offset_viewport`, which
    /// knows nothing about overlays - has to end here. Without it an overlay expanded over the
    /// *viewport*, rather than over the whole surface, keeps the rectangle the old chrome left:
    /// the tab bar or the status bar is turned on or off underneath it, or a layout is applied,
    /// and it is still drawn against a viewport that no longer exists.
    fn refresh_overlay_expansion(&mut self) {
        if self.overlay_panes.fullscreen_is_active() {
            self.overlay_panes.set_pane_frames().non_fatal();
        }
    }
    /// Resize a pane in its own layer. One core for the by-client and by-id entry points, so
    /// that they cannot disagree about the rules:
    ///
    /// - an overlay is dispatched first. The layers below are a two-way floating/tiled split,
    ///   and without this the request fell through to the tiled grid and resized whatever the
    ///   overlay was covering. `OverlayPanes` refuses a modal on its own, and writes the new
    ///   size back into the overlay's coordinates rather than onto its geometry - an overlay's
    ///   geometry stays a function of its coordinates and the surface;
    /// - a floating resize is refused while a floating pane is fullscreen: that pane has no size
    ///   of its own to change, and the others are hidden under it. The guard is the floating
    ///   layer's own, so it reaches neither an overlay, which is resizable by ctrl-drag in this
    ///   state either way, nor a tiled pane named by id, whose grid the fullscreen does not
    ///   touch. Otherwise the resize marks the floating layout damaged and forces a render, so
    ///   the panes under it repaint and no "garbage" is left behind by a decrease; the tiled
    ///   grid was not touched, so it is not marked;
    /// - a tiled resize dissolves the stack lists first, being a classic mutation of the grid
    ///   (a hidden list member is only in the grid once its list is dissolved), and marks the
    ///   tiled layout damaged on success, which is what stops the next swap-layout request from
    ///   discarding the layout the user just adjusted. A pane that cannot be resized because a
    ///   neighbour is fixed is flashed rather than reported.
    fn resize_pane_in_layer(
        &mut self,
        subject: PaneSubject,
        strategy: ResizeStrategy,
    ) -> Result<()> {
        let err_context = || "unable to resize pane".to_string();
        let pane_id = match subject {
            PaneSubject::Active(client_id) => self.get_active_pane_id(client_id),
            PaneSubject::Pane(pane_id) => Some(pane_id),
        };
        let Some(pane_id) = pane_id else {
            return Ok(());
        };
        if self.overlay_panes.panes_contain(&pane_id) {
            let resized = self
                .overlay_panes
                .resize_pane_with_id(pane_id, strategy)
                .with_context(err_context)?;
            if resized {
                self.set_force_render();
            }
            return Ok(());
        }
        if self.floating_panes.panes_contain(&pane_id) {
            // a fullscreen floating pane has no size of its own to change, and the other floating
            // panes are hidden under it; the tiled grid beneath is touched by neither, so a tiled
            // pane named by id still resizes
            if self.floating_panes.fullscreen_is_active() {
                return Ok(());
            }
            let successfully_resized = self
                .floating_panes
                .resize_pane_with_id(strategy, pane_id)
                .with_context(err_context)?;
            if successfully_resized {
                self.swap_layouts.set_is_floating_damaged();
                self.set_force_render();
            }
            return Ok(());
        }
        if self.tiled_panes.panes_contain(&pane_id)
            || self.pane_is_hidden_stack_list_member(&pane_id)
        {
            self.dissolve_stack_lists_for_classic_mutation();
        }
        if self.tiled_panes.panes_contain(&pane_id) {
            // By subject, not by id: a keyboard resize of an undirected strategy is a stacked
            // resize when `stacked_resize` is on, which `TiledPanes::resize_active_pane` does
            // and the by-id manager call never has.
            let outcome = match subject {
                PaneSubject::Active(client_id) => {
                    self.tiled_panes.resize_active_pane(client_id, &strategy)
                },
                PaneSubject::Pane(pane_id) => self
                    .tiled_panes
                    .resize_pane_with_id(strategy, pane_id, None)
                    .map(|_| ()),
            };
            match outcome {
                Ok(()) => self.swap_layouts.set_is_tiled_damaged(),
                Err(err) => match err.downcast_ref::<YinzError>() {
                    Some(YinzError::CantResizeFixedPanes { pane_ids }) => {
                        self.senders
                            .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                                pane_ids.clone(),
                                "FIXED!".into(),
                            ))
                            .with_context(err_context)?;
                    },
                    _ => Err::<(), _>(err).non_fatal(),
                },
            }
            self.update_agent_pane_widths().with_context(err_context)?;
        } else if self.suppressed_panes.contains_key(&pane_id) {
            log::error!("Cannot resize suppressed panes");
        }
        Ok(())
    }
    pub fn resize(&mut self, client_id: ClientId, strategy: ResizeStrategy) -> Result<()> {
        self.resize_pane_in_layer(PaneSubject::Active(client_id), strategy)
    }

    fn close_down_to_max_terminals(&mut self) -> Result<()> {
        if let Some(max_panes) = self.max_panes {
            self.dissolve_stack_lists_for_classic_mutation();
            let terminals = self.get_tiled_pane_ids();
            for &pid in terminals.iter().skip(max_panes.saturating_sub(1)) {
                self.senders
                    .send_to_pty(PtyInstruction::ClosePane(pid, None))
                    .context("failed to close down to max terminals")?;
                self.close_pane(pid, false, None);
            }
        }
        Ok(())
    }
    pub fn get_tiled_pane_ids(&self) -> Vec<PaneId> {
        self.get_tiled_panes().map(|(&pid, _)| pid).collect()
    }
    pub fn kitty_visible_pane_ids(&self) -> HashSet<PaneId> {
        let mut pane_ids: HashSet<PaneId> =
            self.tiled_panes.rendered_pane_ids().into_iter().collect();
        pane_ids.extend(self.floating_panes.rendered_pane_ids());
        pane_ids.extend(self.overlay_panes.rendered_pane_ids());
        pane_ids
    }
    pub fn get_all_pane_ids(&self) -> Vec<PaneId> {
        let mut static_and_floating_pane_ids = self.get_static_and_floating_pane_ids();
        let mut suppressed_pane_ids = self.suppressed_panes.keys().copied().collect();
        static_and_floating_pane_ids.append(&mut suppressed_pane_ids);
        static_and_floating_pane_ids
    }
    /// Every overlay this tab holds, hidden ones included.
    ///
    /// Hidden ones on purpose: an overlay that has been set aside still has to hear the events it
    /// will act on when it comes back - a hidden agent learns which tab to reappear on from the
    /// `TabUpdate`s it receives while hidden.
    pub fn overlay_plugin_ids(&self) -> Vec<PluginId> {
        self.overlay_panes
            .pane_ids()
            .filter_map(|pane_id| match pane_id {
                PaneId::Plugin(pid) => Some(*pid),
                _ => None,
            })
            .collect()
    }
    pub fn get_static_and_floating_pane_ids(&self) -> Vec<PaneId> {
        self.tiled_panes
            .pane_ids()
            .chain(self.floating_panes.pane_ids())
            .chain(self.overlay_panes.pane_ids())
            .copied()
            .collect()
    }
    /// The panes a tab travels as: the ones with a slot in the tiled or floating layout.
    ///
    /// Overlays are deliberately absent. A terminal overlay's overlay-ness cannot be re-derived at
    /// the far end - there is no plugin configuration to read it back out of, which is the same
    /// reason `placement_for_extraction` has to be asked before a pane leaves - so an overlay is
    /// not warped at all. A caller that swept one out of the tab anyway would leave nothing for
    /// the tab close to hand to the pty thread, and the overlay's child would be left attached to
    /// nobody.
    pub fn tiled_and_floating_pane_ids(&self) -> Vec<PaneId> {
        self.tiled_panes
            .pane_ids()
            .chain(self.floating_panes.pane_ids())
            .copied()
            .collect()
    }
    pub fn get_plugin_ids(&self) -> Vec<PluginId> {
        self.get_static_and_floating_pane_ids()
            .into_iter()
            .filter_map(|pane_id| match pane_id {
                PaneId::Plugin(pid) => Some(pid),
                _ => None,
            })
            .collect()
    }
    /// Hidden plugins still need events to know when to show themselves again.
    pub fn get_plugin_ids_including_suppressed(&self) -> Vec<PluginId> {
        let mut plugin_ids = self.get_plugin_ids();
        for pane_id in self.suppressed_panes.keys() {
            if let PaneId::Plugin(plugin_id) = pane_id
                && !plugin_ids.contains(plugin_id)
            {
                plugin_ids.push(*plugin_id);
            }
        }
        plugin_ids
    }
    pub fn get_pane_info(&self, pane_id: PaneId) -> Option<PaneInfo> {
        let current_pane_group: HashMap<ClientId, Vec<PaneId>> =
            { self.current_pane_group.borrow().clone_inner() };

        // Check tiled panes
        if let Some(pane) = self.tiled_panes.get_pane(pane_id) {
            let mut info = pane_info_for_pane(&pane_id, pane.as_ref(), &current_pane_group);
            // Note: is_focused will be false since we don't have a specific client_id context
            // Plugins calling this API would need to compare the pane_id with their own focused pane
            info.is_focused = false;
            info.is_fullscreen = self.tiled_panes.fullscreen_is_active();
            info.is_floating = false;
            info.is_suppressed = false;
            info.is_stack_list_member = self.pane_is_stack_list_member(&pane_id);
            return Some(info);
        }

        // Check floating panes
        if let Some(pane) = self.floating_panes.get_pane(pane_id) {
            let mut info = pane_info_for_pane(&pane_id, pane.as_ref(), &current_pane_group);
            info.is_focused = false;
            info.is_fullscreen = self.floating_panes.fullscreen_pane_id() == Some(pane_id);
            info.is_floating = true;
            info.is_suppressed = false;
            info.is_stack_list_member = self.pane_is_stack_list_member(&pane_id);
            return Some(info);
        }

        // Check overlay panes - the collection's own singular API rather than the fields spelled
        // again here, so that the two cannot drift apart. It is what knows that an expanded
        // overlay reports the rectangle it will come back to.
        if let Some(mut info) = self
            .overlay_panes
            .pane_info_of(&pane_id, &current_pane_group)
        {
            info.is_focused = false;
            return Some(info);
        }

        // Check suppressed panes
        if let Some(suppressed) = self.suppressed_panes.get(&pane_id) {
            let mut info =
                pane_info_for_pane(&pane_id, suppressed.pane.as_ref(), &current_pane_group);
            info.is_focused = false;
            info.is_fullscreen = false;
            info.is_floating = false;
            info.is_suppressed = true;
            info.is_stack_list_member = self.pane_is_stack_list_member(&pane_id);
            return Some(info);
        }

        None
    }
    pub fn set_pane_selectable(&mut self, id: PaneId, selectable: bool) {
        if self.is_pending {
            self.pending_instructions
                .push(BufferedTabInstruction::SetPaneSelectable(id, selectable));
            return;
        }
        if let Some(pane) = self.tiled_panes.get_pane_mut(id) {
            pane.set_selectable(selectable);
            if !selectable {
                self.tiled_panes.move_clients_out_of_pane(id);
            }
        } else if let Some(pane) = self.floating_panes.get_pane_mut(id) {
            pane.set_selectable(selectable);
            if !selectable {
                self.floating_panes.move_clients_out_of_pane(id);
                if !self.floating_panes.has_selectable_panes() {
                    self.hide_floating_panes();
                }
            }
        } else if let Some(pane) = self.overlay_panes.get_mut(&id) {
            pane.set_selectable(selectable);
            if !selectable {
                self.overlay_panes.move_clients_out_of_pane(id);
            }
        }
        // we do this here because if there is a non-selectable pane on the edge, we consider it
        // outside the viewport (a ui-pane, eg. the status-bar and tab-bar) and need to adjust for it
        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            &mut self.tiled_panes,
            self.pane_frame_style,
        );
        self.refresh_overlay_expansion();
    }

    /// Whether this tab has been told it is on screen. The other half of [`Tab::visible`]: a tab
    /// a client was moved into without this being set renders for nobody. Only the tests ask -
    /// production code sets it and reads it off `Screen::active_tab_ids` instead.
    pub fn is_visible(&self) -> bool {
        self.tab_visible
    }
    pub fn visible(&mut self, visible: bool) -> Result<()> {
        // Floating panes as well as tiled: a floating plugin goes off screen
        // with the tab exactly as a tiled one does, and for a transient popup
        // this is the only word it gets that nobody is looking at it any more
        // (`PaneUpdate` only reaches plugins in a client's *active* tab).
        // Suppressed panes are deliberately still left out - `suppress_pane`
        // extracts them from both collections, and a hidden plugin is not on
        // screen to begin with. A hidden *overlay* is the same statement about
        // the same thing, so it is filtered here rather than extracted: agent
        // set aside with Esc must not be told the tab it is not being drawn on
        // just became visible. The same goes for a floating plugin while the
        // floating surface is down: it is told about the surface by
        // `show_floating_panes`/`hide_floating_panes`, and a tab coming back
        // with its surface hidden must not tell it that it is visible.
        self.tab_visible = visible;
        // The cursor is one host-wide resource that every tab writes to, but the memo of what the
        // host was last told is per tab. A tab coming into view has no idea what the tab it
        // replaced left behind, so it places the cursor from scratch on its first frame.
        if visible {
            self.host_cursor_state.clear();
        }
        let floating_panes_are_shown = self.floating_panes.panes_are_shown();
        let visible_overlays: Vec<PaneId> = self.overlay_panes.stack_layer_ids();
        let tiled_pids = self.tiled_panes.pane_ids().filter_map(|p| match p {
            PaneId::Plugin(pid) => Some(*pid),
            _ => None,
        });
        let floating_pids = self.floating_panes.pane_ids().filter_map(|p| match p {
            PaneId::Plugin(pid) if !visible || floating_panes_are_shown => Some(*pid),
            _ => None,
        });
        let overlay_pids = visible_overlays.iter().filter_map(|p| match p {
            PaneId::Plugin(pid) => Some(*pid),
            _ => None,
        });
        let mut plugin_updates = vec![];
        for pid in tiled_pids.chain(floating_pids).chain(overlay_pids) {
            if self.pane_tabs_of(PaneId::Plugin(pid)).is_none() {
                plugin_updates.push((Some(pid), None, Event::Visible(visible)));
            }
        }
        self.senders
            .send_to_plugin(PluginInstruction::Update(plugin_updates))
            .with_context(|| format!("failed to set visibility of tab to {visible}"))?;
        self.sync_pane_tabs()?;
        if !visible {
            self.mouse_hover_pane_id.clear();
            self.plugin_hover_pane_id.clear();
        }
        Ok(())
    }

    pub fn clear_mouse_help_text(&mut self, client_id: ClientId) {
        self.mouse_help_text_visible.insert(client_id, false);
    }

    pub fn update_active_pane_name(&mut self, buf: Vec<u8>, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to update name of active pane to '{buf:?}' for client {client_id}");
        let s = str::from_utf8(&buf).with_context(err_context)?;
        self.get_active_pane_mut(client_id)
            .with_context(|| format!("no active pane found for client {client_id}"))
            .map(|active_pane| {
                let to_update = match s {
                    "\0" => s, // pane name terminator
                    "\u{007F}" | "\u{0008}" => {
                        // delete and backspace keys
                        s
                    },
                    _ => &clean_string_from_control_and_linebreak(s),
                };
                active_pane.update_name(to_update);
            })?;
        if let Some(id) = self.get_active_pane_id(client_id) {
            self.update_pane_tab_label(id)?;
        }
        Ok(())
    }

    /// Rename a pane, reporting whether the name actually moved.
    ///
    /// The `bool` is what lets the caller skip the repaint and the session-state
    /// report for a rename that changes nothing. A plugin that reasserts its own
    /// title every render — agent did, ten times a second while streaming a
    /// reply — would otherwise have the screen rebuild every tab's pane manifest
    /// and push `PaneUpdate`/`TabUpdate` at every subscribed plugin each time,
    /// landing that traffic in the same queues the user's keystrokes travel.
    pub fn rename_pane(&mut self, buf: Vec<u8>, pane_id: PaneId) -> Result<bool> {
        let err_context = || {
            format!(
                "failed to update name of active pane to '{buf:?}' for pane_id {:?}",
                pane_id
            )
        };
        let pane = self
            .get_pane_with_id_mut(pane_id)
            .with_context(err_context)?;
        // `custom_title` is empty for a pane carrying no name of its own, which
        // is also what an empty `buf` asks for — so this covers undoing a rename
        // as well as restating one.
        if pane.custom_title().unwrap_or_default().as_bytes() == buf.as_slice() {
            return Ok(false);
        }
        pane.rename(buf);
        self.update_pane_tab_label(pane_id)?;
        Ok(true)
    }

    pub fn rename_active_pane(&mut self, buf: Vec<u8>, client_id: ClientId) -> Result<()> {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let id = active_pane.pid();
            active_pane.rename(buf);
            self.update_pane_tab_label(id)?;
        }
        Ok(())
    }

    pub fn undo_active_rename_pane(&mut self, client_id: ClientId) -> Result<()> {
        if let Some(active_terminal_id) = self.get_active_terminal_id(client_id) {
            // by id rather than by which collection the layer's visibility suggests: the active
            // terminal can be an overlay, which is on neither the floating layer nor the tiled one
            let active_terminal = self
                .get_pane_with_id_mut(PaneId::Terminal(active_terminal_id))
                .with_context(|| {
                    format!("failed to undo rename of active pane for client {client_id}")
                })?;

            active_terminal.load_pane_name();
        }
        Ok(())
    }

    pub fn is_position_inside_viewport(&self, point: &Position) -> Result<bool> {
        let line = point.line();
        let column = point.column();
        let line: usize = line.try_into().with_context(|| {
            format!("failed to determine if position {point:?} is inside viewport")
        })?;

        let viewport = self.viewport.borrow();
        Ok(line >= viewport.y
            && column >= viewport.x
            && line < viewport.y + viewport.rows
            && column < viewport.x + viewport.cols)
    }

    pub fn set_pane_frames(&mut self, pane_frame_style: PaneFrameStyle) {
        self.tiled_panes.set_pane_frames(pane_frame_style);
        self.floating_panes.set_pane_frame_style(pane_frame_style);
        self.pane_frame_style = pane_frame_style;
        self.set_should_clear_display_before_rendering();
        self.set_force_render();
    }
    pub fn panes_to_hide_count(&self) -> usize {
        self.tiled_panes.panes_to_hide_count()
    }

    pub fn is_pending(&self) -> bool {
        self.is_pending
    }

    pub fn add_red_pane_frame_color_override(
        &mut self,
        pane_id: PaneId,
        error_text: Option<String>,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.add_red_pane_frame_color_override(error_text);
        }
    }
    pub fn add_highlight_pane_frame_color_override(
        &mut self,
        pane_id: PaneId,
        error_text: Option<String>,
        client_id: Option<ClientId>,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.add_highlight_pane_frame_color_override(error_text, client_id);
        }
    }
    /// Whether `pane_id`'s frame override has finished fading, so the caller can stop asking for
    /// frames on its behalf. A pane that is not here has nothing fading.
    pub fn frame_color_override_is_faded_out(&self, pane_id: PaneId, now: Instant) -> bool {
        self.rendered_pane(pane_id)
            .map(|pane| pane.frame_color_override_strength_is_settled(now))
            .unwrap_or(true)
    }

    /// Mark `pane_id`'s frame and boundaries for repaint without touching its contents.
    pub fn set_pane_frame_should_render(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.rendered_pane_mut(pane_id) {
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
        }
    }

    /// Start a bell flash on `pane_id`. See `Pane::flash_pane_frame_color_override`.
    ///
    /// Returns whether the pane was found, so the caller knows whether to keep asking for frames
    /// while the flash fades.
    pub fn flash_pane_frame_color_override(&mut self, pane_id: PaneId, now: Instant) -> bool {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.flash_pane_frame_color_override(now);
            pane.set_should_render(true);
            pane.set_should_render_boundaries(true);
            true
        } else {
            false
        }
    }
    pub fn clear_pane_frame_color_override(
        &mut self,
        pane_id: PaneId,
        client_id: Option<ClientId>,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_pane_frame_color_override(client_id);
        }
    }
    pub fn set_plugin_regex_highlights_for_pane(
        &mut self,
        pane_id: PaneId,
        plugin_id: PluginId,
        highlights: Vec<RegexHighlight>,
        style: &Style,
    ) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_plugin_regex_highlights(plugin_id, highlights, style);
        }
    }

    pub fn clear_all_plugin_highlights(&mut self, plugin_id: PluginId) {
        for pane_id in self.get_all_pane_ids() {
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.clear_plugin_highlights(plugin_id);
            }
        }
    }

    pub fn clear_plugin_highlights_for_pane(&mut self, pane_id: PaneId, plugin_id: PluginId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_plugin_highlights(plugin_id);
        }
    }

    pub fn update_plugin_loading_stage(
        &mut self,
        pid: PluginId,
        loading_indication: LoadingIndication,
    ) {
        if loading_indication.is_error() || loading_indication.ended {
            self.pane_tab_load_state(
                pid,
                if loading_indication.is_error() {
                    yinz_types::pane_tabs::PaneTabLoadState::Failed(
                        "Plugin failed to load; focus the tab bar and press r to retry".into(),
                    )
                } else {
                    yinz_types::pane_tabs::PaneTabLoadState::Ready
                },
            )
            .non_fatal();
        }
        if loading_indication.is_error() {
            // Failed initialization may never send geometry; show the diagnostic anyway.
            self.overlay_panes.finish_placement(PaneId::Plugin(pid));
        }
        if let Some(plugin_pane) = self
            .get_pane_with_id_mut(PaneId::Plugin(pid))
            .and_then(|pane| pane.plugin_control_mut())
        {
            plugin_pane.update_loading_indication(loading_indication);
        }
    }
    pub fn show_plugin_cursor(
        &mut self,
        pid: PluginId,
        client_id: ClientId,
        cursor_position: Option<(usize, usize)>,
    ) {
        if let Some(plugin_pane) = self.get_pane_with_id_mut(PaneId::Plugin(pid)) {
            plugin_pane.show_cursor(client_id, cursor_position);
        }
    }
    pub fn start_plugin_loading_indication(
        &mut self,
        pid: PluginId,
        loading_indication: LoadingIndication,
    ) {
        if let Some(plugin_pane) = self
            .get_pane_with_id_mut(PaneId::Plugin(pid))
            .and_then(|pane| pane.plugin_control_mut())
        {
            plugin_pane.start_loading_indication(loading_indication);
        }
    }
    pub fn progress_plugin_loading_offset(&mut self, pid: PluginId) {
        if let Some(plugin_pane) = self
            .get_pane_with_id_mut(PaneId::Plugin(pid))
            .and_then(|pane| pane.plugin_control_mut())
        {
            plugin_pane.progress_animation_offset();
        }
    }
    pub fn show_floating_panes(&mut self) {
        // this function is to be preferred to directly invoking floating_panes.toggle_show_panes(true)
        let were_shown = self.floating_panes.panes_are_shown();
        self.floating_panes.toggle_show_panes(true);
        self.tiled_panes.unfocus_all_panes();
        self.set_force_render();
        if !were_shown {
            self.notify_floating_plugins_of_visibility(true);
        }
    }

    pub fn hide_floating_panes(&mut self) {
        // this function is to be preferred to directly invoking
        // floating_panes.toggle_show_panes(false)
        let were_shown = self.floating_panes.panes_are_shown();
        if self.floating_panes.fullscreen_is_active() {
            self.floating_panes.unset_fullscreen();
        }
        self.floating_panes.toggle_show_panes(false);
        self.tiled_panes.focus_all_panes();
        self.set_force_render();
        if were_shown {
            let tiled_clients: Vec<_> = self
                .connected_clients
                .borrow()
                .iter()
                .copied()
                .filter(|client_id| {
                    self.get_active_pane_id(*client_id)
                        .is_some_and(|pane_id| self.tiled_panes.panes_contain(&pane_id))
                })
                .collect();
            for client_id in tiled_clients {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
            self.notify_floating_plugins_of_visibility(false);
        }
    }

    /// Tell the plugins on the floating surface that the surface was raised or lowered. Only
    /// the panes in `floating_panes` are concerned: overlays are drawn on their own layer and
    /// suppressed panes are not on screen either way. Nothing is sent while the tab itself is
    /// hidden - `visible(true)` reports the state the surface is in when the tab returns.
    fn notify_floating_plugins_of_visibility(&mut self, visible: bool) {
        if !self.tab_visible {
            return;
        }
        let plugin_updates: Vec<_> = self
            .floating_panes
            .pane_ids()
            .filter(|id| self.pane_tabs_of(**id).is_none())
            .filter_map(|p| match p {
                PaneId::Plugin(pid) => Some((Some(*pid), None, Event::Visible(visible))),
                _ => None,
            })
            .collect();
        self.sync_pane_tabs().non_fatal();
        if plugin_updates.is_empty() {
            return;
        }
        self.senders
            .send_to_plugin(PluginInstruction::Update(plugin_updates))
            .with_context(|| format!("failed to set visibility of floating panes to {visible}"))
            .non_fatal();
    }

    pub fn show_floating_panes_atomic(&mut self, mut completion: Option<NotificationEnd>) {
        if self.floating_panes.all_panes_are_visible() {
            if let Some(c) = completion.as_mut() {
                c.set_exit_status(2);
            }
        } else {
            match self.floating_panes.last_selectable_floating_pane_id() {
                Some(last_selectable_floating_pane_id) => {
                    self.show_floating_panes();
                    self.floating_panes
                        .focus_pane_for_all_clients(last_selectable_floating_pane_id);
                    if let Some(c) = completion.as_mut() {
                        c.set_exit_status(0);
                    }
                },
                None => {
                    // No selectable floating panes exist — surface must not be shown
                    if let Some(c) = completion.as_mut() {
                        c.set_exit_status(1);
                    }
                },
            }
        }
        drop(completion);
    }

    pub fn hide_floating_panes_atomic(&mut self, mut completion: Option<NotificationEnd>) {
        if !self.floating_panes.all_panes_are_visible() {
            if let Some(c) = completion.as_mut() {
                c.set_exit_status(2);
            }
        } else {
            self.hide_floating_panes();
            if let Some(c) = completion.as_mut() {
                c.set_exit_status(0);
            }
        }
        drop(completion);
    }

    pub fn find_plugin(&self, run_plugin_or_alias: &RunPluginOrAlias) -> Option<PaneId> {
        self.tiled_panes
            .get_plugin_pane_id(run_plugin_or_alias)
            .or_else(|| self.floating_panes.get_plugin_pane_id(run_plugin_or_alias))
            .or_else(|| self.overlay_panes.get_plugin_pane_id(run_plugin_or_alias))
            .or_else(|| {
                self.suppressed_panes
                    .iter()
                    .find(|(_id, suppressed)| {
                        run_plugin_or_alias.is_equivalent_to_run(suppressed.pane.invoked_with())
                    })
                    .map(|(pane_id, _)| *pane_id)
            })
            .map(|pane| {
                self.pane_tabs_of(pane)
                    .map(|id| self.pane_tabs[&id].info.pane_id)
                    .unwrap_or(pane)
            })
    }

    pub fn can_replace_active_pane(&self, client_id: ClientId) -> bool {
        self.get_active_pane_id(client_id)
            .is_some_and(|pane_id| self.has_non_suppressed_pane_with_pid(&pane_id))
    }

    pub fn focus_pane_with_id(
        &mut self,
        pane_id: PaneId,
        should_float: bool,
        should_be_in_place: bool,
        client_id: ClientId,
    ) -> Result<()> {
        if let Some(id) = self.pane_tabs_of(pane_id)
            && self.pane_tabs[&id].info.pane_id != pane_id
            && let Some(index) = self.pane_tabs[&id]
                .info
                .tabs
                .iter()
                .position(|slot| slot.pane_id() == Some(pane_id))
        {
            self.select_pane_tab(id, index, client_id)?;
            if self.pane_tabs[&id].info.pane_id != pane_id {
                return Ok(());
            }
        }
        // Revealing a suppressed pane explicitly makes it selectable below. Hidden stack
        // members retain their selectability when swapped into the visible slot.
        if self
            .get_pane_with_id(pane_id)
            .is_some_and(|pane| !pane.selectable())
            && (!self.suppressed_panes.contains_key(&pane_id)
                || self.pane_is_hidden_stack_list_member(&pane_id))
        {
            return Err(anyhow!(
                "Cannot focus pane {pane_id:?}: pane is not selectable"
            ));
        }

        // Resolve the replacement before a dismissible overlay can give up focus.
        let pane_id_to_replace = if !should_float
            && should_be_in_place
            && self.suppressed_panes.contains_key(&pane_id)
            && !self.pane_is_hidden_stack_list_member(&pane_id)
        {
            Some(
                self.get_active_pane_id(client_id)
                    .filter(|id| self.has_non_suppressed_pane_with_pid(id))
                    .with_context(|| {
                        format!("no active pane to replace for client {client_id}; pane {pane_id:?} remains suppressed")
                    })?,
            )
        } else {
            None
        };

        // A pane named by id is the pane the user just asked for, so this takes the *opening*
        // shape of the rule rather than the moving one that `lower_layer_may_take_focus` applies
        // to the focus-move family: a dismissible overlay gives the keyboard up so it can dismiss
        // itself, a modal keeps it until it goes, and either way the focus underneath still lands
        // where it was asked to - exactly as `add_floating_pane` does.
        //
        // A replacement keeps the focus of its slot. Otherwise, defocus only for a pane this
        // tab holds outside the overlay collection: focusing another overlay or naming an
        // absent pane should dismiss nothing.
        if pane_id_to_replace.is_none()
            && !self.overlay_panes.panes_contain(&pane_id)
            && self.get_pane_with_id(pane_id).is_some()
        {
            self.defocus_overlay_pane(client_id, !self.floating_panes.panes_contain(&pane_id));
        }
        // A modal keeping the keyboard still learns where the user now wants to be, and hands
        // the keyboard there when it goes rather than back to where it was opened from.
        if pane_id_to_replace.is_none() && self.get_pane_with_id(pane_id).is_some() {
            self.overlay_panes.return_from_modal_to(client_id, pane_id);
        }
        if self.focus_hidden_stack_list_member(pane_id, client_id) {
            return Ok(());
        }
        self.tiled_panes
            .focus_pane_if_exists(pane_id, client_id)
            .map(|_| self.hide_floating_panes())
            .or_else(|_| {
                self.show_overlay_pane(pane_id);
                self.overlay_panes.focus_pane_if_exists(pane_id, client_id)
            })
            .or_else(|_| {
                let focused_floating_pane =
                    self.floating_panes.focus_pane_if_exists(pane_id, client_id);
                if focused_floating_pane.is_ok() {
                    self.show_floating_panes();
                };
                focused_floating_pane
            })
            .or_else(|_| {
                match self
                    .suppressed_panes
                    .remove(&pane_id)
                    .map(|suppressed| suppressed.pane)
                {
                    Some(mut pane) => {
                        pane.set_selectable(true);
                        if should_float {
                            // A modal parked in the suppressed panes comes back out as one:
                            // `add_floating_pane` reads the placement off the pane itself and
                            // sends it to the overlay collection, so it does not bring the
                            // floating layer up with it.
                            if !run_requests_overlay(pane.invoked_with()) {
                                self.show_floating_panes();
                            }
                            self.add_floating_pane(pane, pane_id, None, true, Some(client_id))
                        } else if let Some(pane_id_to_replace) = pane_id_to_replace {
                            let replaced_pane =
                                if self.overlay_panes.panes_contain(&pane_id_to_replace) {
                                    Some(
                                        self.overlay_panes
                                            .replace_pane(pane_id_to_replace, pane)
                                            .context("failed to replace active overlay pane")?,
                                    )
                                } else if self.floating_panes.panes_contain(&pane_id_to_replace) {
                                    Some(
                                        self.floating_panes
                                            .replace_pane(pane_id_to_replace, pane)
                                            .context("failed to replace active floating pane")?,
                                    )
                                } else {
                                    self.tiled_panes.replace_pane(pane_id_to_replace, pane)
                                };
                            if let Some(replaced_pane) = replaced_pane {
                                let is_scrollback_editor = false;
                                let lent = false;
                                self.track_stack_list_member_id_swap(replaced_pane.pid(), pane_id);
                                self.suppress_pane_behind(
                                    replaced_pane,
                                    pane_id,
                                    is_scrollback_editor,
                                    lent,
                                );
                            } else {
                                log::error!("Could not find pane to replace, aborting.");
                            }
                            Ok(())
                        } else {
                            self.hide_floating_panes();
                            self.add_tiled_pane(pane, pane_id, false, Some(client_id))
                        }
                    },
                    None => Ok(()),
                }
            })
    }

    pub fn pane_infos(&self) -> Vec<PaneInfo> {
        let mut pane_info = vec![];
        let current_pane_group = { self.current_pane_group.borrow().clone_inner() };
        let mut tiled_pane_info = self.tiled_panes.pane_info(&current_pane_group);
        // Preserve traversal order in the manifest. Coordinate sorting interleaves
        // nested columns and would make repeated project arrangement swap panes.
        tiled_pane_info.sort_by_key(|info| {
            (
                self.tiled_panes
                    .get_pane(info.id)
                    .and_then(|pane| pane.position_and_size().logical_position)
                    .unwrap_or(usize::MAX),
                info.id,
            )
        });
        let mut floating_pane_info = self.floating_panes.pane_info(&current_pane_group);
        let mut overlay_pane_info = self.overlay_panes.pane_info(&current_pane_group);
        pane_info.append(&mut tiled_pane_info);
        pane_info.append(&mut floating_pane_info);
        pane_info.append(&mut overlay_pane_info);
        for SuppressedPane { pane, .. } in self.suppressed_panes.values() {
            let mut pane_info_for_suppressed_pane =
                pane_info_for_pane(&pane.pid(), pane.as_ref(), &current_pane_group);
            pane_info_for_suppressed_pane.is_floating = false;
            pane_info_for_suppressed_pane.is_suppressed = true;
            pane_info_for_suppressed_pane.is_focused = false;
            pane_info_for_suppressed_pane.is_fullscreen = false;
            pane_info.push(pane_info_for_suppressed_pane);
        }
        for info in &mut pane_info {
            info.is_stack_list_member = self.pane_is_stack_list_member(&info.id);
        }
        pane_info
    }

    pub fn resize_pane_with_id(&mut self, strategy: ResizeStrategy, pane_id: PaneId) -> Result<()> {
        self.resize_pane_in_layer(PaneSubject::Pane(pane_id), strategy)
    }
    pub fn update_agent_theme_mapping(
        &mut self,
        mapping: yinz_types::agent::theme::AgentThemeMapping,
    ) {
        self.style.agent_theme_mapping = mapping;
        self.floating_panes.update_agent_theme_mapping(mapping);
        self.overlay_panes.update_agent_theme_mapping(mapping);
        self.tiled_panes.update_agent_theme_mapping(mapping);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_agent_theme_mapping(mapping);
        }
    }
    pub fn update_terminal_theme(&mut self, theme: Option<yinz_types::style::TerminalTheme>) {
        self.style.terminal = theme;
        self.set_force_render();
        self.floating_panes.update_terminal_theme(theme);
        self.overlay_panes.update_terminal_theme(theme);
        self.tiled_panes.update_terminal_theme(theme);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_terminal_theme(theme);
        }
    }
    pub fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
        self.set_force_render();
        // The tab's `default_mode_info` is what `update_input_modes`
        // falls back to when no per-client entry exists in
        // `self.mode_info`. Without this update, plugins on a
        // freshly-connected client (one that has never manually
        // changed mode) receive `Event::ModeUpdate` carrying the old
        // style and skip their re-render.
        self.default_mode_info.update_theme(theme);
        self.floating_panes.update_pane_themes(theme);
        self.overlay_panes.update_pane_themes(theme);
        self.tiled_panes.update_pane_themes(theme);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_theme(theme);
        }
    }
    pub fn update_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
        self.floating_panes
            .update_pane_rounded_corners(rounded_corners);
        self.overlay_panes
            .update_pane_rounded_corners(rounded_corners);
        self.tiled_panes
            .update_pane_rounded_corners(rounded_corners);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_rounded_corners(rounded_corners);
        }
    }
    pub fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        self.arrow_fonts = should_support_arrow_fonts;
        self.floating_panes
            .update_pane_arrow_fonts(should_support_arrow_fonts);
        self.overlay_panes
            .update_pane_arrow_fonts(should_support_arrow_fonts);
        self.tiled_panes
            .update_pane_arrow_fonts(should_support_arrow_fonts);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_arrow_fonts(should_support_arrow_fonts);
        }
    }
    /// Whether every client shares one focus. Read at the next focus change and frame.
    pub fn update_session_is_mirrored(&mut self, session_is_mirrored: bool) {
        self.stack_list_session_is_mirrored = session_is_mirrored;
        self.tiled_panes
            .set_session_is_mirrored(session_is_mirrored);
        self.floating_panes
            .set_session_is_mirrored(session_is_mirrored);
        self.overlay_panes
            .set_session_is_mirrored(session_is_mirrored);
    }
    pub fn update_render_options(&mut self, styled_underlines: bool, osc8_hyperlinks: bool) {
        self.styled_underlines = styled_underlines;
        self.osc8_hyperlinks = osc8_hyperlinks;
        self.floating_panes
            .update_pane_render_options(styled_underlines, osc8_hyperlinks);
        self.overlay_panes
            .update_pane_render_options(styled_underlines, osc8_hyperlinks);
        self.tiled_panes
            .update_pane_render_options(styled_underlines, osc8_hyperlinks);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_render_options(styled_underlines, osc8_hyperlinks);
        }
    }
    pub fn update_kitty_keyboard_protocol_disabled(&mut self, disabled: bool) {
        self.explicitly_disable_kitty_keyboard_protocol = disabled;
        self.floating_panes
            .update_pane_kitty_keyboard_protocol_disabled(disabled);
        self.overlay_panes
            .update_pane_kitty_keyboard_protocol_disabled(disabled);
        self.tiled_panes
            .update_pane_kitty_keyboard_protocol_disabled(disabled);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_kitty_keyboard_protocol_disabled(disabled);
        }
    }
    pub fn update_kitty_host_support(&mut self, supported: KittyHostSupport) {
        self.kitty_host_support = Some(supported);
        self.floating_panes
            .update_pane_kitty_host_support(supported);
        self.overlay_panes.update_pane_kitty_host_support(supported);
        self.tiled_panes.update_pane_kitty_host_support(supported);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_kitty_host_support(supported);
        }
    }
    pub fn update_sixel_host_support(&mut self, supported: bool) {
        self.sixel_host_support = Some(supported);
        self.floating_panes
            .update_pane_sixel_host_support(supported);
        self.overlay_panes.update_pane_sixel_host_support(supported);
        self.tiled_panes.update_pane_sixel_host_support(supported);
        for pane in self.suppressed_panes.values_mut().map(|s| &mut s.pane) {
            pane.update_sixel_host_support(supported);
        }
    }
    pub fn update_default_shell(&mut self, mut default_shell: Option<PathBuf>) {
        if let Some(default_shell) = default_shell.take() {
            self.default_shell = default_shell;
        }
    }
    pub fn update_default_editor(&mut self, mut default_editor: Option<PathBuf>) {
        if let Some(default_editor) = default_editor.take() {
            self.default_editor = Some(default_editor);
        }
    }
    pub fn update_copy_options(&mut self, copy_options: &CopyOptions) {
        self.clipboard_provider = match &copy_options.command {
            Some(command) => ClipboardProvider::Command(CopyCommand::new(command.clone())),
            None => ClipboardProvider::Osc52(copy_options.clipboard),
        };
        self.copy_on_select = copy_options.copy_on_select;
    }
    pub fn update_auto_layout(&mut self, auto_layout: bool) {
        self.auto_layout = auto_layout;
    }
    pub fn update_advanced_mouse_actions(&mut self, advanced_mouse_actions: bool) {
        self.advanced_mouse_actions = advanced_mouse_actions;
    }
    pub fn update_mouse_scroll_resize(&mut self, mouse_scroll_resize: bool) {
        self.mouse_scroll_resize = mouse_scroll_resize;
    }
    pub fn update_mouse_hover_effects(&mut self, mouse_hover_effects: bool) {
        self.mouse_hover_effects = mouse_hover_effects;
    }
    pub fn update_focus_follows_mouse(&mut self, focus_follows_mouse: bool) {
        self.focus_follows_mouse = focus_follows_mouse;
    }
    pub fn update_context_menu_on_right_click(&mut self, context_menu_on_right_click: bool) {
        self.context_menu_on_right_click = context_menu_on_right_click;
    }
    pub fn update_context_menu_override_agent_mouse(
        &mut self,
        context_menu_override_agent_mouse: bool,
    ) {
        self.context_menu_override_agent_mouse = context_menu_override_agent_mouse;
    }
    pub fn update_mouse_click_through(&mut self, mouse_click_through: bool) {
        self.mouse_click_through = mouse_click_through;
    }
    pub fn update_selection_options(
        &mut self,
        osc133_command_selection: bool,
        word_separators: String,
    ) {
        self.osc133_command_selection = osc133_command_selection;
        self.word_separators = word_separators;
    }
    pub fn clear_mouse_hover_state(&mut self) {
        self.mouse_hover_pane_id.clear();
        self.plugin_hover_pane_id.clear();
        self.mouse_last_pane_id.clear();
        self.mouse_help_text_visible.clear();
    }

    pub fn toggle_pane_pinned(&mut self, client_id: ClientId) {
        if let Some(pane) = self.get_active_pane_mut(client_id) {
            pane.toggle_pinned();
            self.set_force_render();
        }
    }
    pub fn set_floating_pane_pinned(&mut self, pane_id: PaneId, should_be_pinned: bool) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_pinned(should_be_pinned);
            self.set_force_render();
        }
    }
    pub fn has_room_for_stack(&mut self, root_pane_id: PaneId, stack_size: usize) -> bool {
        if self.floating_panes.panes_contain(&root_pane_id)
            || self.overlay_panes.panes_contain(&root_pane_id)
            || self.suppressed_panes.contains_key(&root_pane_id)
        {
            log::error!("Root pane of stack cannot be floating, an overlay or suppressed");
            return false;
        }
        self.dissolve_stack_lists_for_classic_mutation();
        // the overlay arm above is load-bearing rather than tidiness: the size check below
        // measures through `get_pane_with_id`, which resolves overlays, so a tall enough overlay
        // would answer `true` for a pane that is not in the tiled grid at all. `Screen::stack_panes`
        // consults this *before* extracting the members, and by the time the tiled stacking found
        // no root the extracted panes would already be out of their tabs with nowhere to go.
        if self.pane_is_stacked(root_pane_id) {
            let room_left_in_stack = self
                .tiled_panes
                .room_left_in_stack_of_pane_id(&root_pane_id)
                .unwrap_or(0);
            stack_size <= room_left_in_stack
        } else {
            self.get_pane_with_id(root_pane_id)
                .map(|p| p.position_and_size().rows.as_usize() >= stack_size + MIN_TERMINAL_HEIGHT)
                .unwrap_or(false)
        }
    }
    pub fn set_tiled_panes_damaged(&mut self) {
        self.swap_layouts.set_is_tiled_damaged();
    }
    pub fn stack_panes(&mut self, root_pane_id: PaneId, mut panes_to_stack: Vec<Box<dyn Pane>>) {
        if panes_to_stack.is_empty() {
            // nothing to do
            return;
        }
        self.dissolve_stack_lists_for_classic_mutation();
        self.swap_layouts.set_is_tiled_damaged();
        if self.pane_is_stacked(root_pane_id) {
            for pane in self
                .tiled_panes
                .add_panes_to_stack(&root_pane_id, panes_to_stack)
            {
                log::error!("No room in the stack for pane {:?}, hiding it", pane.pid());
                self.park_pane(pane);
            }
        } else {
            // + 1 for the root pane
            let mut stack_geoms = self
                .tiled_panes
                .stack_panes(root_pane_id, panes_to_stack.len() + 1);
            if stack_geoms.is_empty() {
                log::error!("Failed to find room for stacked panes");
                // these panes were extracted from their tabs before we were called, so this is
                // the last thing holding them: dropping the vec would close them out from under
                // their still-running processes. The stacked-root arm above parks on failure for
                // exactly the same reason.
                for pane in panes_to_stack.drain(..) {
                    self.park_pane(pane);
                }
                return;
            }
            self.tiled_panes
                .set_geom_for_pane_with_id(&root_pane_id, stack_geoms.remove(0));
            let mut focused_pane_id_in_stack = None;
            for mut pane in panes_to_stack.drain(..) {
                let pane_id = pane.pid();
                let stack_geom = stack_geoms.remove(0);
                pane.set_geom(stack_geom);
                self.tiled_panes.add_pane_with_existing_geom(pane_id, pane);
                if self.tiled_panes.pane_id_is_focused(&pane_id) {
                    focused_pane_id_in_stack = Some(pane_id);
                }
            }
            // if we had a focused pane in the stack, we expand it
            if let Some(focused_pane_id_in_stack) = focused_pane_id_in_stack {
                self.tiled_panes
                    .expand_pane_in_stack(focused_pane_id_in_stack);
            } else if self.tiled_panes.pane_id_is_focused(&root_pane_id) {
                self.tiled_panes.expand_pane_in_stack(root_pane_id);
            } else {
                // Nothing in the new stack is focused, so nothing expanded it and nothing
                // reapplied the frames: the root was collapsed to a single row above and
                // every member kept the offset it arrived with, and no pty heard of
                // either. `expand_pane_in_stack` reaches `reapply_pane_frames`; this is
                // the same for a stack nobody is looking at.
                self.tiled_panes
                    .reapply_pane_frames_in_stack_of(&root_pane_id);
            }
        }
    }
    /// Move or resize overlays where they already are, as one placement.
    ///
    /// The kind is not a parameter and cannot change: an overlay was born dismissible or modal and
    /// stays so, which is why `OverlayPanes::change_pane_coordinates` touches only the geometry.
    /// A pane that is not an overlay is refused rather than promoted - overlay-ness is decided at
    /// birth, and `change_floating_pane_coordinates` is what moves a floating pane.
    ///
    /// Conflicting placements naming members of the same container are rejected.
    pub fn change_overlay_coordinates_batch(
        &mut self,
        batch: Vec<(PaneId, FloatingPaneCoordinates)>,
        override_user_placement: bool,
    ) -> Result<()> {
        if let Some((pane_id, _)) = batch
            .iter()
            .find(|(pane_id, _)| !self.overlay_panes.panes_contain(pane_id))
        {
            return Err(anyhow!(
                "pane {pane_id:?} is not an overlay - use change_floating_pane_coordinates"
            ));
        }
        self.overlay_panes
            .change_pane_coordinates_batch(batch, override_user_placement)?;
        self.set_force_render();
        Ok(())
    }
    /// Move or resize a pane on the tab's floating layer.
    ///
    /// A pane cannot *become* an overlay here. Overlay-ness is decided when the pane is born, so
    /// there is no promotion to compensate for: the compensation that used to live here - noting
    /// the floating layer's visibility before a pane opened onto it, so that a pane which turned
    /// itself into an overlay a moment later could put back the reveal its own birth had caused -
    /// went with it.
    ///
    /// An overlay asking to be moved is answered where it already is, so that a plugin resizing
    /// itself does not have to know which collection it landed in. `change_overlay_coordinates`
    /// is the direct route.
    pub fn change_floating_pane_coordinates(
        &mut self,
        pane_id: &PaneId,
        floating_pane_coordinates: FloatingPaneCoordinates,
    ) -> Result<()> {
        // already an overlay: coordinates change, overlay-ness does not
        if self.overlay_panes.panes_contain(pane_id) {
            self.overlay_panes.change_pane_coordinates(
                *pane_id,
                floating_pane_coordinates,
                false,
            )?;
            self.set_force_render();
            return Ok(());
        }
        if !self.floating_panes.panes_contain(pane_id) {
            // if these panes are not floating, we make them floating (assuming doing so wouldn't
            // be removing the last selectable tiled pane in the tab, which would close it)
            if (self.tiled_panes.panes_contain(pane_id)
                && self.get_selectable_tiled_panes().count() <= 1)
                || self.suppressed_panes.contains_key(pane_id)
            {
                if let Some(pane) = self.extract_pane(*pane_id, true) {
                    self.add_floating_pane(pane, *pane_id, None, false, None)?;
                }
                // `add_floating_pane` sends an overlay-by-configuration pane straight to the
                // overlay collection, so it may already have landed there
                if self.overlay_panes.panes_contain(pane_id) {
                    self.overlay_panes.change_pane_coordinates(
                        *pane_id,
                        floating_pane_coordinates,
                        false,
                    )?;
                    self.set_force_render();
                    return Ok(());
                }
            }
        }
        self.floating_panes
            .change_pane_coordinates(*pane_id, floating_pane_coordinates)?;
        self.set_force_render();
        self.swap_layouts.set_is_floating_damaged();
        Ok(())
    }
    pub fn toggle_pane_borderless(&mut self, pane_id: &PaneId) -> Result<()> {
        if let Some(pane) = self.floating_panes.get_pane_mut(*pane_id) {
            let is_borderless = pane.borderless();
            if is_borderless {
                pane.set_content_offset(Offset::frame(1));
            } else {
                pane.set_content_offset(Offset::default());
            }
            pane.set_borderless(!is_borderless);
            self.set_force_render();
            return Ok(());
        }

        if let Some(pane) = self.tiled_panes.get_pane_mut(*pane_id) {
            let is_borderless = pane.borderless();
            if is_borderless {
                pane.set_content_offset(Offset::frame(1));
            } else {
                pane.set_content_offset(Offset::default());
            }
            pane.set_borderless(!is_borderless);
            self.set_force_render();
            return Ok(());
        }

        if let Some(pane) = self.suppressed_panes.get_mut(pane_id) {
            let is_borderless = pane.pane.borderless();
            if is_borderless {
                pane.pane.set_content_offset(Offset::frame(1));
            } else {
                pane.pane.set_content_offset(Offset::default());
            }
            pane.pane.set_borderless(!is_borderless);
            return Ok(());
        }

        // an overlay does its own frame and offset arithmetic, in `set_pane_frames`
        if self.overlay_panes.toggle_borderless(pane_id) {
            self.set_force_render();
            return Ok(());
        }

        log::error!("Pane with id {:?} not found", pane_id);
        Ok(())
    }
    pub fn set_pane_borderless(&mut self, pane_id: &PaneId, borderless: bool) -> Result<()> {
        // Try floating panes first
        if let Some(pane) = self.floating_panes.get_pane_mut(*pane_id) {
            if borderless {
                pane.set_content_offset(Offset::default()); // Borderless: no offset
            } else {
                pane.set_content_offset(Offset::frame(1)); // Bordered: 1-char offset
            }
            pane.set_borderless(borderless);
            self.set_force_render();
            return Ok(());
        }

        // Try tiled panes
        if let Some(pane) = self.tiled_panes.get_pane_mut(*pane_id) {
            if borderless {
                pane.set_content_offset(Offset::default());
            } else {
                pane.set_content_offset(Offset::frame(1));
            }
            pane.set_borderless(borderless);
            self.set_force_render();
            return Ok(());
        }

        // Try suppressed panes
        if let Some(pane) = self.suppressed_panes.get_mut(pane_id) {
            if borderless {
                pane.pane.set_content_offset(Offset::default());
            } else {
                pane.pane.set_content_offset(Offset::frame(1));
            }
            pane.pane.set_borderless(borderless);
            return Ok(());
        }

        // Try overlays, which do their own frame and offset arithmetic in `set_pane_frames`
        if self.overlay_panes.set_borderless(pane_id, borderless) {
            self.set_force_render();
            return Ok(());
        }

        log::error!("Pane with id {:?} not found", pane_id);
        Ok(())
    }
    pub fn get_viewport(&self) -> Viewport {
        *self.viewport.borrow()
    }
    pub fn get_display_area(&self) -> Size {
        *self.display_area.borrow()
    }
    pub fn get_client_input_mode(&self, client_id: ClientId) -> Option<InputMode> {
        self.mode_info.borrow().get(&client_id).map(|m| m.mode)
    }
    pub fn query_mouse_hover_pane_id(&self) -> HashMap<ClientId, PaneId> {
        self.mouse_hover_pane_id.clone()
    }

    fn pane_is_stacked(&self, pane_id: PaneId) -> bool {
        self.get_pane_with_id(pane_id)
            .map(|p| p.position_and_size().stacked.is_some())
            .unwrap_or(false)
    }

    pub fn resize_by_pane_id(&mut self, pane_id: PaneId, strategy: ResizeStrategy) {
        self.resize_pane_with_id(strategy, pane_id).non_fatal();
    }
    pub fn move_pane_by_pane_id(&mut self, pane_id: PaneId, direction: Option<Direction>) {
        let movement = direction.map_or(PaneMove::Next { backwards: false }, PaneMove::Toward);
        self.move_pane_in_layer(PaneSubject::Pane(pane_id), movement);
    }
    pub fn move_pane_backwards_by_pane_id(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Next { backwards: true },
        );
    }
    pub fn clear_screen_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        self.clear_screen_for_pane_id(pane_id);
        Ok(())
    }
    pub fn toggle_fullscreen_by_pane_id(&mut self, pane_id: PaneId) {
        self.toggle_pane_fullscreen(pane_id);
    }
    pub fn toggle_no_ui_fullscreen_by_pane_id(&mut self, pane_id: PaneId) {
        self.toggle_pane_no_ui_fullscreen(pane_id);
    }
    pub fn rename_pane_by_pane_id(&mut self, pane_id: PaneId, name: Vec<u8>) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.rename(name);
            self.update_pane_tab_label(pane_id)?;
        }
        Ok(())
    }
    pub fn undo_rename_pane_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.load_pane_name();
            self.update_pane_tab_label(pane_id).non_fatal();
        }
    }
    pub fn toggle_pane_pinned_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.toggle_pinned();
            self.set_force_render();
        }
    }
}

/// Structural invariants that must hold after every operation on a `Tab`.
///
/// Written for the modals/overlays refactor: the property test in
/// `unit/screen_tests/overlays.rs` drives a random sequence of operations and calls this after
/// each one, and the characterization tests call it at their assertion points. Invariants that
/// only become true once overlays have their own collection are noted and added there.
impl Tab {
    pub fn assert_invariants(&self) {
        // 1. A pane lives in exactly one collection.
        let mut seen: std::collections::HashMap<PaneId, &'static str> =
            std::collections::HashMap::new();
        let mut claim = |pane_id: PaneId, collection: &'static str| {
            if let Some(previous) = seen.insert(pane_id, collection) {
                panic!("{pane_id:?} is in both {previous} and {collection}");
            }
        };
        for pane_id in self.tiled_panes.pane_ids() {
            claim(*pane_id, "tiled_panes");
        }
        for pane_id in self.floating_panes.pane_ids() {
            claim(*pane_id, "floating_panes");
        }
        for pane_id in self.overlay_panes.pane_ids() {
            claim(*pane_id, "overlay_panes");
        }
        for pane_id in self.suppressed_panes.keys() {
            claim(*pane_id, "suppressed_panes");
        }

        // 2. No overlay pane id appears in `floating_panes`. Invariant 1 already forbids a pane
        //    being in two collections; this says the specific thing the refactor is for, so that
        //    the guarantee does not quietly become "some other collection" later.
        for pane_id in self.overlay_panes.pane_ids() {
            assert!(
                !self.floating_panes.panes_contain(pane_id),
                "{pane_id:?} is an overlay but is carried by the floating panes"
            );
        }

        // 3. Nothing the floating layer draws is an overlay. The transition half of this - that
        //    opening or focusing an overlay never *changes* the layer's visibility - is a property
        //    of a step rather than of a state, and is asserted around each overlay operation by
        //    the property test in `unit/screen_tests/overlays.rs`. (The layer can legitimately be
        //    flagged visible with nothing on it: the user can just toggle it.)
        for pane_id in self.floating_panes.rendered_pane_ids() {
            assert!(
                !self.overlay_panes.panes_contain(&pane_id),
                "{pane_id:?} is an overlay but the floating layer is drawing it"
            );
        }

        // 4. The occlusion stack names only live panes, and a pane's z-index is its position in
        //    that stack - `FloatingPanes::stack_layer_ids` and `render` must agree, or a pane is
        //    clipped against its own geometry and erases itself.
        let stack = self.floating_panes.rendered_pane_ids();
        for (index, pane_id) in stack.iter().enumerate() {
            assert!(
                self.floating_panes.panes_contain(pane_id),
                "{pane_id:?} is in the occlusion stack but not in the floating panes"
            );
            assert_eq!(
                self.floating_panes.get_pane_z_index(*pane_id),
                Some(index),
                "{pane_id:?} is at position {index} of the occlusion stack but reports a \
                 different z-index"
            );
        }

        let overlay_stack = self.overlay_panes.rendered_pane_ids();
        let mut remaining = overlay_stack.as_slice();
        // A container owns one occlusion layer, even when it renders two children.
        for (index, geom) in self.overlay_panes.layers().iter().enumerate() {
            let first = *remaining
                .first()
                .expect("overlay layer has no rendered pane");
            let members = self.overlay_panes.members(&first);
            assert!(!members.is_empty(), "{first:?} has no overlay container");
            assert!(
                remaining.starts_with(&members),
                "overlay members are not adjacent in render order"
            );
            for pane_id in &members {
                assert!(self.overlay_panes.panes_contain(pane_id));
                assert_eq!(
                    self.overlay_panes.get_pane_z_index(*pane_id),
                    Some(index),
                    "{pane_id:?} does not report its container's z-index"
                );
                assert_eq!(self.overlay_panes.outer_geom(*pane_id), Some(*geom));
            }
            remaining = &remaining[members.len()..];
        }
        assert!(
            remaining.is_empty(),
            "rendered overlay panes have no occlusion layer"
        );

        // 5. An expansion names a pane that is here, on screen and expandable, and no *other*
        //    overlay is holding a geometry override. The second half is what makes "the way back
        //    is exact" a property of the collection rather than of each test: an override is the
        //    one geometry not derived from an overlay's coordinates, so a path that expands and
        //    forgets to collapse is caught here rather than by the rectangle it leaves behind.
        let fullscreen_overlays = self.overlay_panes.fullscreen_pane_ids();
        if let Some(pane_id) = self.overlay_panes.fullscreen_pane_id() {
            assert!(
                self.overlay_panes.panes_contain(&pane_id),
                "{pane_id:?} is the expanded overlay but is not in the overlay panes"
            );
            assert!(
                !self.overlay_panes.is_hidden(&pane_id),
                "{pane_id:?} is expanded over the screen but is also set aside"
            );
            assert!(
                !self.overlay_panes.is_modal(&pane_id),
                "{pane_id:?} is a modal, which may not be expanded over the screen"
            );
            assert!(
                fullscreen_overlays.contains(&pane_id),
                "{pane_id:?} anchors the expansion but is not one of the panes it draws"
            );
        }
        for (pane_id, pane) in self.overlay_panes.get_panes() {
            assert_eq!(
                pane.geom_override().is_some(),
                fullscreen_overlays.contains(pane_id),
                "{pane_id:?} holds a geometry override iff it is expanded over the screen"
            );
        }

        // 6. Every client's active pane exists and is selectable.
        let connected_clients: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        for client_id in connected_clients {
            if let Some(pane_id) = self.get_active_pane_id(client_id) {
                let pane = self.get_pane_with_id(pane_id).unwrap_or_else(|| {
                    panic!("client {client_id} is focused on {pane_id:?}, which does not exist")
                });
                assert!(
                    pane.selectable(),
                    "client {client_id} is focused on {pane_id:?}, which is not selectable"
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "./unit/tab_tests/mod.rs"]
mod tab_tests;

#[cfg(test)]
#[path = "./unit/tab_integration_tests/mod.rs"]
mod tab_integration_tests;

#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub command: Option<String>,
    pub clipboard: yinz_types::input::options::Clipboard,
    pub copy_on_select: bool,
}

impl CopyOptions {
    pub fn new(
        copy_command: Option<String>,
        copy_clipboard: yinz_types::input::options::Clipboard,
        copy_on_select: bool,
    ) -> Self {
        Self {
            command: copy_command,
            clipboard: copy_clipboard,
            copy_on_select,
        }
    }
}

/// The options a test tab copies with: no command, the default clipboard, copy on select.
#[cfg(test)]
impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            command: None,
            clipboard: yinz_types::input::options::Clipboard::default(),
            copy_on_select: true,
        }
    }
}
