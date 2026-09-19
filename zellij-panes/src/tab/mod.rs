//! `Tab`s holds multiple panes. It tracks their coordinates (x/y) and size,
//! as well as how they should be resized

mod clipboard;
mod copy_command;
mod layout_applier;
pub use layout_applier::arrangement_layout;
mod mouse_handler;
mod swap_layouts;

use crate::plugins::PluginId;
use crate::ui::damage::CellRect;
use copy_command::CopyCommand;
pub use mouse_handler::{MouseEffect, MouseHandler, PaneEdge, PaneResizeState, ResizeDragTarget};
use std::env::temp_dir;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;
use uuid::Uuid;
use zellij_runtime::errors::prelude::*;
use zellij_types::agent::AgentInfo;
use zellij_types::data::PaneContents;
use zellij_types::geometry::{Direction, ResizeStrategy};
use zellij_types::input::command::RunCommand;
use zellij_types::input::options::DEFAULT_WORD_SEPARATORS;
use zellij_types::internal_ids::SessionName;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabPosition;
use zellij_types::internal_ids::TerminalId;
use zellij_types::keys::KeyWithModifier;
use zellij_types::mouse::MouseEvent;
use zellij_types::position::Position;
use zellij_types::style::Style;
use zellij_types::text::clean_string_from_control_and_linebreak;
use {
    zellij_types::data::NewPanePlacement, zellij_types::data::OverlayKind,
    zellij_types::data::OverlayPlacement, zellij_types::data::PaneInfo,
    zellij_types::data::PermissionStatus, zellij_types::data::PermissionType,
    zellij_types::data::PluginPermission, zellij_types::data::RegexHighlight,
    zellij_types::data::StyledText,
};

use crate::background_jobs::BackgroundJob;
use crate::pane_groups::PaneGroups;
use crate::pty_writer::PtyWriteInstruction;
use crate::screen::{GuestModalOutcome, ScreenInstruction};
use crate::ui::hint_text::{
    held_hint_variants, hover_hint_variants, resize_hint_variants, suspended_hint_variants,
    HintExitStatus,
};
use crate::ui::{
    loading_indication::LoadingIndication, pane_boundaries_frame::FrameParams,
    pane_contents_and_ui::PaneContentsAndUi,
};
use layout_applier::LayoutApplier;
use swap_layouts::SwapLayouts;

use self::clipboard::ClipboardProvider;
use crate::route::NotificationEnd;
use crate::{
    os_input_output::ServerOsApi,
    output::{CharacterChunk, FloatingPanesStack, KittyImageChunk, Output, SixelImageChunk},
    panes::floating_panes::floating_pane_grid::half_size_middle_geom,
    panes::grid::PendingNotification,
    panes::kitty_graphics::{KittyHostSupport, KittyImageStore},
    panes::nested_session_modal::GuestModalShortcuts,
    panes::sixel::SixelImageStore,
    panes::{FloatingPanes, OverlayCarry, OverlayPanes, PaneFactory, TiledPanes},
    panes::{LinkHandler, PaneId, PluginPane, TerminalPane, EMPTY_TERMINAL_CHARACTER},
    plugins::PluginInstruction,
    pty::{ClientTabIndexOrPaneId, PtyInstruction, VteBytes},
    thread_bus::ThreadSenders,
    ClientId, ServerInstruction,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    str,
};
use zellij_types::style::{Palette, Styling};
use zellij_types::{
    geometry::{FloatingPaneCoordinates, PercentOrFixed, SplitDirection},
    pane_size::{Dimension, Offset, PaneGeom, Size, SizeInPixels, Viewport},
};
use {
    zellij_ipc::nested_session::NestedSessionMessage, zellij_types::data::Event,
    zellij_types::data::InputMode, zellij_types::data::ModeInfo, zellij_types::data::PaletteColor,
    zellij_types::input::command::TerminalAction, zellij_types::input::layout::FloatingPaneLayout,
    zellij_types::input::layout::Run, zellij_types::input::layout::RunPluginOrAlias,
    zellij_types::input::layout::SwapFloatingLayout, zellij_types::input::layout::SwapTiledLayout,
    zellij_types::input::layout::TiledPaneLayout, zellij_types::input::options::PaneFrameStyle,
};

#[macro_export]
macro_rules! resize_pty {
    ($pane:expr, $os_input:expr, $senders:expr, $character_cell_size:expr) => {{
        let (width_in_pixels, height_in_pixels) = {
            let character_cell_size = $character_cell_size.borrow();
            match *character_cell_size {
                Some(size_in_pixels) => {
                    let width_in_pixels =
                        (size_in_pixels.width * $pane.get_content_columns()) as u16;
                    let height_in_pixels =
                        (size_in_pixels.height * $pane.get_content_rows()) as u16;
                    (Some(width_in_pixels), Some(height_in_pixels))
                },
                None => (None, None),
            }
        };
        match $pane.pid() {
            PaneId::Terminal(ref pid) => {
                use $crate::PtyWriteInstruction;
                let err_context = || "Failed to send resize pty instruction".to_string();
                $senders
                    .send_to_pty_writer(PtyWriteInstruction::ResizePty(
                        *pid,
                        $pane.get_content_columns() as u16,
                        $pane.get_content_rows() as u16,
                        width_in_pixels,
                        height_in_pixels,
                    ))
                    .with_context(err_context)
            },
            PaneId::Plugin(ref pid) => {
                let err_context = || format!("failed to resize plugin {pid}");
                $senders
                    .send_to_plugin(PluginInstruction::Resize(
                        *pid,
                        $pane.get_content_columns(),
                        $pane.get_content_rows(),
                    ))
                    .with_context(err_context)
            },
        }
    }};
}

// FIXME: This should be replaced by `RESIZE_PERCENT` at some point
pub const MIN_TERMINAL_HEIGHT: usize = 5;
pub const MIN_TERMINAL_WIDTH: usize = 5;

const MAX_PENDING_VTE_EVENTS: usize = 7000;

type HoldForCommand = Option<RunCommand>;

/// The pane holding a suppressed pane's place in the grid.
///
/// Closing it is what brings the suppressed pane back, which is the whole reason
/// the association is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandIn {
    pub pane_id: PaneId,
    pub is_scrollback_editor: bool,
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

impl SuppressedPane {
    fn parked(pane: Box<dyn Pane>) -> Self {
        SuppressedPane {
            pane,
            restored_by: None,
        }
    }
    fn behind(pane: Box<dyn Pane>, stand_in: PaneId, is_scrollback_editor: bool) -> Self {
        SuppressedPane {
            pane,
            restored_by: Some(StandIn {
                pane_id: stand_in,
                is_scrollback_editor,
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

pub struct Tab {
    pub agent_double_escape: bool,
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
    pending_vte_events: HashMap<TerminalId, Vec<VteBytes>>,
    pub selecting_with_mouse_in_pane: Option<PaneId>, // this is only pub for the tests
    pane_being_resized_with_mouse: Option<PaneResizeState>,
    link_handler: Rc<RefCell<LinkHandler>>,
    clipboard_provider: ClipboardProvider,
    // TODO: used only to focus the pane when the layout is loaded
    // it seems that optimization is possible using `active_panes`
    focus_pane_id: Option<PaneId>,
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
    osc133_command_selection: bool,
    word_separators: String,
    currently_marking_pane_group: Rc<RefCell<HashMap<ClientId, bool>>>,
    connected_clients_in_app: Rc<RefCell<HashSet<ClientId>>>,
    pub panes_with_pending_bell: HashSet<PaneId>,
    pub tab_has_pending_bell: bool,
    pub tab_bell_flash: bool, // currently in mid-notification-flash
    pub tab_bell_ring: bool,  // need to send ANSI BEL to the controlling terminal
    /// What `visible` last said about this tab; floating-surface toggles only report their own
    /// visibility to plugins while the tab itself is on screen.
    tab_visible: bool,
}

/// One rendered frame of a pane: its character chunks, the raw vte output to forward to the
/// client (if any), and its sixel and kitty image chunks.
pub type RenderedPane = (
    Vec<CharacterChunk>,
    Option<String>,
    Vec<SixelImageChunk>,
    Vec<KittyImageChunk>,
);

// FIXME: Use a struct that has a pane_type enum, to reduce all of the duplication
pub trait Pane {
    fn x(&self) -> usize;
    fn y(&self) -> usize;
    fn rows(&self) -> usize;
    fn cols(&self) -> usize;
    fn get_content_x(&self) -> usize;
    fn get_content_y(&self) -> usize;
    fn get_content_columns(&self) -> usize;
    fn get_content_rows(&self) -> usize;
    fn reset_size_and_position_override(&mut self);
    fn set_geom(&mut self, position_and_size: PaneGeom);
    fn set_geom_override(&mut self, pane_geom: PaneGeom);
    fn character_cell_size_possibly_changed(&mut self) {}
    fn handle_pty_bytes(&mut self, _bytes: VteBytes) {}
    fn handle_plugin_bytes(&mut self, _client_id: ClientId, _bytes: VteBytes) {}
    fn show_cursor(&mut self, _client_id: ClientId, _cursor_position: Option<(usize, usize)>) {}
    /// Returns the cursor position and whether it is visible.
    /// The position is returned unconditionally (as long as the cursor is within
    /// bounds) so that the host terminal can position the cursor for IME even
    /// when the app has hidden it. The bool is true when the cursor is visible.
    fn cursor_coordinates(&self, _client_id: Option<ClientId>) -> Option<(usize, usize, bool)>;
    fn is_mid_frame(&self) -> bool {
        false
    }
    fn adjust_input_to_terminal(
        &mut self,
        _key_with_modifier: &Option<KeyWithModifier>,
        _raw_input_bytes: Vec<u8>,
        _raw_input_bytes_are_kitty: bool,
        _client_id: Option<ClientId>,
    ) -> Option<AdjustedInput> {
        None
    }
    fn position_and_size(&self) -> PaneGeom;
    fn current_geom(&self) -> PaneGeom;
    fn geom_override(&self) -> Option<PaneGeom>;
    fn should_render(&self) -> bool;
    fn set_should_render(&mut self, should_render: bool);
    fn set_should_render_boundaries(&mut self, _should_render: bool) {}
    fn selectable(&self) -> bool;
    fn set_selectable(&mut self, selectable: bool);
    fn request_permissions_from_user(&mut self, _permissions: Option<PluginPermission>) {}
    fn render(&mut self, client_id: Option<ClientId>) -> Result<Option<RenderedPane>>;
    /// Clients whose next Esc confirms input to this pane; only their frame shows the hint.
    fn set_pending_agent_escape_clients(&mut self, _clients: HashSet<ClientId>) {}

    fn render_frame(
        &mut self,
        client_id: ClientId,
        frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>>; // TODO: better
    fn render_fake_cursor(
        &mut self,
        cursor_color: PaletteColor,
        text_color: PaletteColor,
    ) -> Option<String>;
    fn render_terminal_title(&mut self, _input_mode: InputMode) -> String;
    fn update_name(&mut self, name: &str);
    fn pid(&self) -> PaneId;
    fn reduce_height(&mut self, percent: f64);
    fn increase_height(&mut self, percent: f64);
    fn reduce_width(&mut self, percent: f64);
    fn increase_width(&mut self, percent: f64);
    fn push_down(&mut self, count: usize);
    fn push_right(&mut self, count: usize);
    fn pull_left(&mut self, count: usize);
    fn pull_up(&mut self, count: usize);
    fn clear_screen(&mut self);
    fn dump_screen(&self, _full: bool, _client_id: Option<ClientId>) -> String {
        "".to_owned()
    }
    fn dump_screen_with_ansi(&self, _full: bool, _client_id: Option<ClientId>) -> String {
        "".to_owned()
    }
    fn scroll_up(&mut self, count: usize, client_id: ClientId);
    fn scroll_down(&mut self, count: usize, client_id: ClientId);
    fn scroll_left(&mut self, _count: usize, _client_id: ClientId) {}
    fn scroll_right(&mut self, _count: usize, _client_id: ClientId) {}
    fn scroll_to_previous_prompt(&mut self, _client_id: ClientId) {}
    fn scroll_to_next_prompt(&mut self, _client_id: ClientId) {}
    fn select_command_at_scroll_position(&mut self, _client_id: ClientId) {}
    /// The output of the last completed OSC 133 command, also starting its feedback flash.
    fn copy_last_command_output(&mut self) -> Option<String> {
        None
    }
    fn clear_command_output_flash(&mut self) {}
    fn clear_scroll(&mut self);
    fn is_scrolled(&self) -> bool;
    fn active_at(&self) -> Instant;
    fn set_active_at(&mut self, instant: Instant);
    fn set_frame(&mut self, frame: bool);
    fn set_content_offset(&mut self, offset: Offset);
    fn get_content_offset(&self) -> Offset {
        Offset::default()
    }
    fn cursor_shape_csi(&self) -> String {
        "\u{1b}[0 q".to_string() // default to non blinking block
    }
    fn contains(&self, position: &Position) -> bool {
        match self.geom_override() {
            Some(position_and_size) => position_and_size.contains(position),
            None => self.position_and_size().contains(position),
        }
    }
    fn start_selection(&mut self, _start: &Position, _client_id: ClientId) {}
    fn update_selection(&mut self, _position: &Position, _client_id: ClientId) {}
    fn end_selection(&mut self, _end: &Position, _client_id: ClientId) {}
    fn reset_selection(&mut self, _client_id: Option<ClientId>) {}
    fn start_text_selection(&mut self, _client_id: ClientId) {}
    fn move_text_selection(&mut self, _direction: Direction, _client_id: ClientId) {}
    fn swap_text_selection_ends(&mut self, _client_id: ClientId) {}
    fn remember_copied_text_selection(&mut self, _client_id: ClientId) {}
    fn finish_text_selection(&mut self, _client_id: ClientId) {}
    fn supports_mouse_selection(&self) -> bool {
        true
    }
    fn get_selected_text(&self, _client_id: ClientId) -> Option<String> {
        None
    }
    fn set_pane_default_colors(&mut self, _fg: Option<String>, _bg: Option<String>) {}
    fn get_pane_default_colors(&self) -> (Option<String>, Option<String>) {
        (None, None)
    }
    /// The agent running in this pane, as the session knows it. Only terminal panes have one.
    fn agent_info(&self) -> Option<&AgentInfo> {
        None
    }
    /// Replace the pane's agent record; answers whether it changed.
    fn set_agent_info(&mut self, _agent: Option<AgentInfo>) -> bool {
        false
    }
    /// Whether this pane has ever held an agent record. Sticky, unlike the record itself,
    /// which a `SessionEnd` hook clears just before the agent's process exits.
    fn last_agent_cwd(&self) -> Option<&str> {
        None
    }
    fn last_agent_kind(&self) -> Option<&str> {
        None
    }
    fn was_an_agent(&self) -> bool {
        false
    }
    /// Whether this pane is kin of the focused one - one of a set some plugin named through
    /// `PluginCommand::SetRelatedPanes` - so its frame is drawn with a gentle tint.
    ///
    /// State on the pane rather than a set threaded through the render, for the same reason
    /// `agent_attention_since` is: the frame is decided per pane, and a pane that closes
    /// takes its answer with it. Only terminal panes carry one; agents are terminals.
    fn has_related_highlight(&self) -> bool {
        false
    }
    /// Mark or unmark this pane as kin of the focused one. Answers whether it changed, so a
    /// caller setting a whole session's worth can tell whether anything needs redrawing.
    fn set_related_highlight(&mut self, _related: bool) -> bool {
        false
    }
    /// When this pane's agent entered a wait a person has to answer, while nobody has yet.
    /// `None` once it has been answered, and for every pane that is not an unanswered wait.
    fn agent_attention_since(&self) -> Option<Instant> {
        None
    }
    /// The wait has been noticed: somebody typed here, or focused the pane. Answers whether
    /// there was anything to take down, so a caller can stop asking for frames.
    ///
    /// Deliberately not a change to the agent record. The record is what the *agent* said,
    /// and a person answering the prompt is not the agent saying anything - the next hook
    /// will say it, and until then the row and the manifest are right to read `waiting`.
    fn acknowledge_agent_attention(&mut self) -> bool {
        false
    }

    fn right_boundary_x_coords(&self) -> usize {
        self.x() + self.cols()
    }
    fn right_boundary_x_content_coords(&self) -> usize {
        self.get_content_x() + self.get_content_columns()
    }
    fn bottom_boundary_y_coords(&self) -> usize {
        self.y() + self.rows()
    }
    fn bottom_boundary_y_content_coords(&self) -> usize {
        self.get_content_y() + self.get_content_rows()
    }
    fn is_right_of(&self, other: &dyn Pane) -> bool {
        self.x() > other.x()
    }
    fn is_directly_right_of(&self, other: &dyn Pane) -> bool {
        self.x() == other.x() + other.cols()
    }
    fn is_left_of(&self, other: &dyn Pane) -> bool {
        self.x() < other.x()
    }
    fn is_directly_left_of(&self, other: &dyn Pane) -> bool {
        self.x() + self.cols() == other.x()
    }
    fn is_below(&self, other: &dyn Pane) -> bool {
        self.y() > other.y()
    }
    fn is_directly_below(&self, other: &dyn Pane) -> bool {
        self.y() == other.y() + other.rows()
    }
    fn is_above(&self, other: &dyn Pane) -> bool {
        self.y() < other.y()
    }
    fn is_directly_above(&self, other: &dyn Pane) -> bool {
        self.y() + self.rows() == other.y()
    }
    fn horizontally_overlaps_with(&self, other: &dyn Pane) -> bool {
        (self.y() >= other.y() && self.y() < (other.y() + other.rows()))
            || ((self.y() + self.rows()) <= (other.y() + other.rows())
                && (self.y() + self.rows()) > other.y())
            || (self.y() <= other.y() && (self.y() + self.rows() >= (other.y() + other.rows())))
            || (other.y() <= self.y() && (other.y() + other.rows() >= (self.y() + self.rows())))
    }
    fn get_horizontal_overlap_with(&self, other: &dyn Pane) -> usize {
        std::cmp::min(self.y() + self.rows(), other.y() + other.rows())
            - std::cmp::max(self.y(), other.y())
    }
    fn vertically_overlaps_with(&self, other: &dyn Pane) -> bool {
        (self.x() >= other.x() && self.x() < (other.x() + other.cols()))
            || ((self.x() + self.cols()) <= (other.x() + other.cols())
                && (self.x() + self.cols()) > other.x())
            || (self.x() <= other.x() && (self.x() + self.cols() >= (other.x() + other.cols())))
            || (other.x() <= self.x() && (other.x() + other.cols() >= (self.x() + self.cols())))
    }
    fn get_vertical_overlap_with(&self, other: &dyn Pane) -> usize {
        std::cmp::min(self.x() + self.cols(), other.x() + other.cols())
            - std::cmp::max(self.x(), other.x())
    }
    fn can_reduce_height_by(&self, reduce_by: usize) -> bool {
        self.rows() > reduce_by && self.rows() - reduce_by >= self.min_height()
    }
    fn can_reduce_width_by(&self, reduce_by: usize) -> bool {
        self.cols() > reduce_by && self.cols() - reduce_by >= self.min_width()
    }
    fn min_width(&self) -> usize {
        MIN_TERMINAL_WIDTH
    }
    fn min_height(&self) -> usize {
        MIN_TERMINAL_HEIGHT
    }
    fn drain_messages_to_pty(&mut self) -> Vec<Vec<u8>> {
        // TODO: this is only relevant to terminal panes
        // we should probably refactor away from this trait at some point
        vec![]
    }
    fn drain_forwarded_queries(&mut self) -> Vec<crate::host_query::HostQuery> {
        // Only terminal panes forward whitelisted queries to the host;
        // plugin panes have no such concept.
        vec![]
    }
    fn drain_nested_session_messages(&mut self) -> Vec<NestedSessionMessage> {
        vec![]
    }
    fn is_nested_guest(&self) -> bool {
        false
    }
    fn set_is_nested_guest(&mut self, _is_nested_guest: bool) {}
    fn set_guest_modal(&mut self, _client_ids: &[ClientId]) {}
    fn clear_guest_modal(&mut self, _client_id: ClientId) {}
    fn clear_all_guest_modals(&mut self) {}
    fn guest_modal_selection(&self, _client_id: ClientId) -> Option<usize> {
        None
    }
    fn has_guest_modal_for_any_client(&self) -> bool {
        false
    }
    fn set_guest_choice_indicator(
        &mut self,
        _client_id: ClientId,
        _indicator: Option<GuestChoiceIndicator>,
    ) {
    }
    fn guest_choice_indicator(&self, _client_id: ClientId) -> Option<GuestChoiceIndicator> {
        None
    }
    fn clear_all_guest_choice_indicators(&mut self) {}
    fn set_guest_session_name(&mut self, _session_name: Option<SessionName>) {}
    fn guest_session_name(&self) -> Option<SessionName> {
        None
    }
    fn set_guest_modal_shortcuts(&mut self, _shortcuts: GuestModalShortcuts) {}
    fn guest_modal_shortcuts(&self) -> GuestModalShortcuts {
        GuestModalShortcuts::default()
    }
    /// Mark this pane as awaiting a host-terminal reply for a forward
    /// it just dispatched. While paused, vte byte feeding is buffered
    /// so that the eventual host reply lands on the pane's stdin in
    /// the same stream position the original query occupied. Default
    /// no-op for non-terminal panes (plugins do not forward queries).
    fn arm_forward_pause(&mut self) {}
    /// Clear the forward-pause flag set by `arm_forward_pause`, returning
    /// `true` if the pane was previously paused. Default no-op.
    fn clear_forward_pause(&mut self) -> bool {
        false
    }
    /// Drain and return any PTY bytes that arrived while the pane was
    /// forward-paused. Tab calls this on resume and re-feeds the bytes
    /// through `handle_pty_bytes`. Default empty for non-terminal panes.
    fn drain_pending_pty_input(&mut self) -> Vec<u8> {
        vec![]
    }
    /// Whether this pane is currently waiting for a host-forward reply.
    fn is_forward_paused(&self) -> bool {
        false
    }
    /// Push a CSI ?997 DSR notification (host color-palette theme mode)
    /// onto this pane's pending pty-write queue, but only if the pane's
    /// underlying app opted in via `CSI ? 2031 h`.
    /// Only relevant to terminal panes, plugin panes receive this through
    /// `Event::HostTerminalThemeChanged`
    fn push_color_palette_dsr(&mut self, _mode: zellij_types::style::HostTerminalThemeMode) {}
    fn drain_clipboard_update(&mut self) -> Option<String> {
        None
    }
    fn drain_desktop_notifications(&mut self) -> Vec<PendingNotification> {
        vec![]
    }
    fn drain_osc7_cwd(&mut self) -> Option<std::path::PathBuf> {
        None
    }
    fn render_full_viewport(&mut self) {}
    fn relative_position(&self, position_on_screen: &Position) -> Position {
        position_on_screen.relative_to(self.get_content_y(), self.get_content_x())
    }
    fn position_is_on_frame(&self, position: &Position) -> bool {
        if !self.contains(position) {
            return false;
        }
        if (self.x()..self.get_content_x()).contains(&position.column()) {
            // position is on left border
            return true;
        }
        if (self.get_content_x() + self.get_content_columns()..(self.x() + self.cols()))
            .contains(&position.column())
        {
            // position is on right border
            return true;
        }
        if (self.y() as isize..self.get_content_y() as isize).contains(&position.line()) {
            // position is on top border
            return true;
        }
        if ((self.get_content_y() + self.get_content_rows()) as isize
            ..(self.y() + self.rows()) as isize)
            .contains(&position.line())
        {
            // position is on bottom border
            return true;
        }
        false
    }
    fn get_edge_at_position(&self, position: &Position) -> Option<PaneEdge> {
        if !self.contains(position) {
            return None;
        }

        let pos_col = position.column();
        let pos_line = position.line();
        let pane_x = self.x();
        let pane_y = self.y();
        let pane_cols = self.cols();
        let pane_rows = self.rows();

        let mid_col = pane_x + pane_cols / 2;
        let mid_line = pane_y + pane_rows / 2;

        let is_left = pos_col < mid_col;
        let is_top = (pos_line as usize) < mid_line;

        Some(match (is_top, is_left) {
            (true, true) => PaneEdge::TopLeft,
            (true, false) => PaneEdge::TopRight,
            (false, true) => PaneEdge::BottomLeft,
            (false, false) => PaneEdge::BottomRight,
        })
    }
    // TODO: get rid of this in favor of intercept_mouse_event_on_frame
    fn intercept_left_mouse_click(&mut self, _position: &Position, _client_id: ClientId) -> bool {
        false
    }
    fn intercept_mouse_event_on_frame(
        &mut self,
        _event: &MouseEvent,
        _client_id: ClientId,
    ) -> bool {
        false
    }
    fn store_pane_name(&mut self);
    fn load_pane_name(&mut self);
    fn set_borderless(&mut self, borderless: bool);
    fn borderless(&self) -> bool;
    fn set_exclude_from_sync(&mut self, exclude_from_sync: bool);
    fn exclude_from_sync(&self) -> bool;

    // TODO: this should probably be merged with the mouse_right_click
    fn handle_right_click(&mut self, _to: &Position, _client_id: ClientId) {}
    fn mouse_event(&self, _event: &MouseEvent, _client_id: ClientId) -> Option<String> {
        None
    }
    fn mouse_left_click(&self, _position: &Position, _is_held: bool) -> Option<String> {
        None
    }
    fn mouse_left_click_release(&self, _position: &Position) -> Option<String> {
        None
    }
    fn mouse_right_click(&self, _position: &Position, _is_held: bool) -> Option<String> {
        None
    }
    fn mouse_right_click_release(&self, _position: &Position) -> Option<String> {
        None
    }
    fn mouse_middle_click(&self, _position: &Position, _is_held: bool) -> Option<String> {
        None
    }
    fn mouse_middle_click_release(&self, _position: &Position) -> Option<String> {
        None
    }
    fn mouse_scroll_up(&self, _position: &Position) -> Option<String> {
        None
    }
    fn mouse_scroll_down(&self, _position: &Position) -> Option<String> {
        None
    }
    fn focus_event(&self) -> Option<String> {
        None
    }
    fn unfocus_event(&self) -> Option<String> {
        None
    }
    fn get_line_number(&self) -> Option<usize> {
        None
    }
    fn update_search_term(&mut self, _needle: &str) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn search_down(&mut self) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn search_up(&mut self) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn toggle_search_case_sensitivity(&mut self) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn toggle_search_whole_words(&mut self) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn toggle_search_wrap(&mut self) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn clear_search(&mut self) {
        // No-op by default (only terminal-panes currently have search capability)
    }
    fn is_alternate_mode_active(&self) -> bool {
        // False by default (only terminal-panes support alternate mode)
        false
    }
    fn hold(&mut self, _exit_status: Option<i32>, _is_first_run: bool, _run_command: RunCommand) {
        // No-op by default, only terminal panes support holding
    }
    /// The pane's root process was stopped, or started running again.
    fn set_suspended(&mut self, _suspended: bool) {
        // No-op by default, only terminal panes have a root process
    }
    fn has_bell(&self) -> bool {
        false
    }
    fn consume_bell(&mut self) {}
    fn set_bell_notification(&mut self, _val: bool) {}
    fn get_bell_notification(&self) -> bool {
        false
    }
    fn add_red_pane_frame_color_override(&mut self, _error_text: Option<String>);
    fn add_highlight_pane_frame_color_override(
        &mut self,
        _text: Option<String>,
        _client_id: Option<ClientId>,
    ) {
    }
    /// Put up a bell flash: the same highlight colour, but easing back out on its own rather than
    /// waiting to be cleared. Every client sees it, because a bell is a fact about the pane.
    fn flash_pane_frame_color_override(&mut self, _now: Instant) {}
    /// Whether a fading frame override has reached the end of its fade. `true` when there is
    /// nothing fading, so a caller can stop asking for frames.
    fn frame_color_override_strength_is_settled(&self, _now: Instant) -> bool {
        true
    }
    /// Take the override down. `None` clears whatever is there; a client only
    /// clears an override that was put up for that same client.
    fn clear_pane_frame_color_override(&mut self, _client_id: Option<ClientId>);
    /// The override colour as `client_id` sees it - `None` when there is no
    /// override, or when the one there is drawn for somebody else.
    fn frame_color_override(&self, _client_id: ClientId) -> Option<PaletteColor>;
    /// How strongly `frame_color_override` applies right now: `1.0` unless it is fading out.
    ///
    /// Separate from the colour rather than folded into it because only the *consumer* knows what
    /// a fading override should blend towards - see `ui::pane_contents_and_ui::frame_color`.
    ///
    /// This reads the clock itself rather than taking the frame's instant, which is the one place
    /// the "sample once per frame" rule is relaxed. Threading `now` down to here would mean
    /// changing every frame- and boundary-drawing signature in the render for a value that is a
    /// display detail and at most a fraction of a millisecond stale. The bell fade is driven and
    /// tested through `FrameColorOverride::strength(now)`, which does take an instant.
    fn frame_color_override_strength(&self, _client_id: ClientId) -> f32 {
        1.0
    }
    fn invoked_with(&self) -> &Option<Run>;
    fn set_notification_end(&mut self, _notification_end: Option<NotificationEnd>) {}
    fn set_title(&mut self, title: String);
    fn update_loading_indication(&mut self, _loading_indication: LoadingIndication) {} // only relevant for plugins
    fn start_loading_indication(&mut self, _loading_indication: LoadingIndication) {} // only relevant for plugins
    fn progress_animation_offset(&mut self) {} // only relevant for plugins
    fn current_title(&self) -> String;
    fn set_remote_host(&mut self, _host: Option<String>) {} // only relevant for terminal panes
    /// A decoration to compose in front of the title, whatever the title is.
    ///
    /// Set by whoever knows something durable about the pane that its own title
    /// cannot be trusted to keep — hocus-agentd puts an agent's checkout here.
    /// Only relevant for terminal panes.
    fn set_title_prefix(&mut self, _prefix: Option<String>) {}
    fn set_subtitle(&mut self, _subtitle: Option<String>) {}
    fn subtitle(&self) -> Option<&str> {
        None
    }
    fn set_subtitle_enabled(&mut self, _enabled: bool) {}
    fn subtitle_enabled(&self) -> bool {
        true
    }
    /// Extra content inset, computed from the current geometry so shrinking a pane
    /// never spends its last content row on a subtitle. The base frame offset is
    /// kept separate because stack members copy it between panes.
    fn subtitle_rows(&self) -> usize {
        let offset = self.get_content_offset();
        usize::from(
            self.subtitle_enabled()
                && self.subtitle().is_some()
                && !self.borderless()
                && offset.top > 0
                && self.rows().saturating_sub(offset.top + offset.bottom) > 1
                && self.cols() > offset.left + offset.right
                && !(self.current_geom().is_stacked() && self.current_geom().rows.is_fixed()),
        )
    }

    /// `current_title()`, prefixed with where the pane is: the remote hostname
    /// for a pane running on another host, and whatever `set_title_prefix` last
    /// said. Use this for anything a human looks at; `current_title()` stays the
    /// raw title for session serialization / layouts / plugins.
    fn display_title(&self) -> String {
        self.current_title()
    }
    fn stack_list_entry_label(&self) -> String {
        self.display_title()
    }
    fn custom_title(&self) -> Option<String>;
    fn has_explicit_title(&self) -> bool {
        false
    }
    fn scroll_position(&self) -> (usize, usize) {
        (0, 0)
    }
    fn is_held(&self) -> bool {
        false
    }
    /// The pane's root process is stopped and can only be woken from out here.
    fn is_suspended(&self) -> bool {
        false
    }
    fn exited(&self) -> bool {
        false
    }
    fn exit_status(&self) -> Option<i32> {
        None
    }
    fn rename(&mut self, _buf: Vec<u8>) {}
    fn serialize(&self, _scrollback_lines_to_serialize: Option<usize>) -> Option<String> {
        None
    }
    fn restart_error(&self) -> Option<String> {
        None
    }
    fn rerun(&mut self) -> Option<RunCommand> {
        self.respawn(None, false)
    } // only relevant to terminal panes
    /// Restart a held command pane, either with its own command (`None`) or with a
    /// `replacement`. Returns the command to spawn, or `None` when the pane is not a held
    /// command pane. Only relevant to terminal panes.
    fn respawn(
        &mut self,
        _replacement: Option<RunCommand>,
        _keep_scrollback: bool,
    ) -> Option<RunCommand> {
        None
    }
    fn update_theme(&mut self, _theme: Styling) {}
    fn set_selection_options(&mut self, _osc133_command_selection: bool, _word_separators: &str) {}
    fn update_arrow_fonts(&mut self, _should_support_arrow_fonts: bool) {}
    fn update_render_options(&mut self, _styled_underlines: bool, _osc8_hyperlinks: bool) {}
    fn update_kitty_keyboard_protocol_disabled(&mut self, _disabled: bool) {}
    fn update_kitty_host_support(&mut self, _supported: KittyHostSupport) {}
    fn update_sixel_host_support(&mut self, _supported: bool) {}
    fn update_rounded_corners(&mut self, _rounded_corners: bool) {}
    fn set_should_be_suppressed(&mut self, _should_be_suppressed: bool) {}
    fn query_should_be_suppressed(&self) -> bool {
        false
    }
    fn drain_fake_cursors(&mut self) -> Option<HashSet<(usize, usize)>> {
        None
    }
    fn toggle_pinned(&mut self) {}
    fn set_pinned(&mut self, _should_be_pinned: bool) {}
    fn reset_logical_position(&mut self) {}
    fn set_mouse_selection_support(&mut self, _selection_support: bool) {}
    fn pane_contents(
        &self,
        client_id: Option<ClientId>,
        _get_full_scrollback: bool,
        _max_scrollback_lines: Option<usize>,
    ) -> PaneContents;
    fn pane_contents_with_ansi(
        &self,
        _client_id: Option<ClientId>,
        _get_full_scrollback: bool,
        _max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        Default::default()
    }
    fn update_exit_status(&mut self, _exit_status: i32) {}
    fn set_plugin_regex_highlights(
        &mut self,
        _plugin_id: PluginId,
        _highlights: Vec<RegexHighlight>,
        _style: &Style,
    ) {
    }
    fn clear_plugin_highlights(&mut self, _plugin_id: PluginId) {}
    fn set_hover_position(&mut self, _position: Option<Position>) -> bool {
        false
    }
    fn cached_hover_tooltip(&self) -> Option<String> {
        None
    }
    fn plugin_highlight_at(
        &self,
        _position: &Position,
    ) -> Option<(PluginId, String, String, BTreeMap<String, String>)> {
        None
    }
    fn terminal_emulator_wants_mouse(&self) -> bool {
        false
    }
    /// The word and the logical line the cursor is over, as text.
    ///
    /// One call rather than two because both walk the same grid rows and a context menu wants
    /// both or neither. `None` for a pane with no character grid to ask - a plugin pane, where
    /// "the word under the cursor" has no meaning the pane could answer for.
    fn text_around_position(&self, _position: &Position) -> Option<TextAroundPosition> {
        None
    }
}

/// What a terminal pane can say about the cell under a pointer. Both fields are independently
/// absent: a click past the end of a line has no word but still has a line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextAroundPosition {
    pub word: Option<String>,
    pub line: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestChoiceIndicator {
    Descended,
    Dismissed,
}

#[derive(Clone, Debug)]
pub enum AdjustedInput {
    WriteBytesToTerminal(Vec<u8>),
    /// The pane is held and was asked to run its command again. The tab hands this to the
    /// screen (`ScreenInstruction::RerunCommandPane`) rather than to the pty: the screen's
    /// re-run is the one that fires `agent_start` for an exited agent coming back.
    ReRunCommandInThisPane,
    /// Send `SIGCONT` to a pane whose root process was stopped by `^Z`. Nothing
    /// inside the pane can do this: the process is stopped, and a command pane
    /// has no shell above it to run `fg`.
    ResumeSuspendedProcessInThisPane,
    PermissionRequestResult(Vec<PermissionType>, PermissionStatus),
    CloseThisPane,
    DropToShellInThisPane {
        working_dir: Option<PathBuf>,
    },
    WriteKeyToPlugin(KeyWithModifier),
    GuestModalSelectionChanged,
    GuestModalZoom,
    GuestModalDescend,
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
        kind: placement.kind,
        coordinates: coordinates.clone().or(placement.coordinates),
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

pub fn get_next_terminal_position(
    tiled_panes: &TiledPanes,
    floating_panes: &FloatingPanes,
) -> usize {
    let tiled_panes_count = tiled_panes
        .get_panes()
        .filter(|(k, _)| match k {
            PaneId::Plugin(_) => false,
            PaneId::Terminal(_) => true,
        })
        .count();
    let floating_panes_count = floating_panes
        .get_panes()
        .filter(|(k, _)| match k {
            PaneId::Plugin(_) => false,
            PaneId::Terminal(_) => true,
        })
        .count();
    tiled_panes_count + floating_panes_count + 1
}

impl Tab {
    // FIXME: Still too many arguments for clippy to be happy...
    #[allow(clippy::too_many_arguments)]
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
        connected_clients_in_app: Rc<RefCell<HashSet<ClientId>>>,
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
            agent_double_escape: false,
            pending_agent_escapes: HashMap::new(),
            id,
            position,
            tiled_panes,
            floating_panes,
            overlay_panes,
            suppressed_panes: HashMap::new(),
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
            pending_vte_events: HashMap::new(),
            connected_clients,
            selecting_with_mouse_in_pane: None,
            pane_being_resized_with_mouse: None,
            link_handler: Rc::new(RefCell::new(LinkHandler::new())),
            clipboard_provider,
            focus_pane_id: None,
            copy_on_select: copy_options.copy_on_select,
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            pids_waiting_resize: HashSet::new(),
            pending_command_respawns: HashMap::new(),
            host_cursor_state: HashMap::new(),
            is_pending: true, // will be switched to false once the layout is applied
            pending_instructions: vec![],
            swap_layouts,
            default_shell,
            debug,
            arrow_fonts,
            kitty_host_support: None,
            sixel_host_support: None,
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

    pub fn stacked_pane_list_is_active(&self) -> bool {
        *self.stacked_pane_list.borrow()
    }
    pub fn pane_is_stack_list_member(&self, pane_id: &PaneId) -> bool {
        self.stack_list_of_member.contains_key(pane_id)
    }
    pub fn has_stack_lists(&self) -> bool {
        !self.stack_lists.is_empty()
    }
    fn stack_list_id_of_member(&self, pane_id: &PaneId) -> Option<StackListId> {
        self.stack_list_of_member.get(pane_id).copied()
    }
    pub fn pane_is_hidden_stack_list_member(&self, pane_id: &PaneId) -> bool {
        self.stack_list_id_of_member(pane_id)
            .and_then(|stack_list_id| self.stack_lists.get(&stack_list_id))
            .map(|list| list.visible != *pane_id)
            .unwrap_or(false)
    }
    pub fn focus_hidden_stack_list_member(&mut self, pane_id: PaneId, client_id: ClientId) -> bool {
        self.swap_in_hidden_stack_list_member(pane_id, Some(client_id))
    }
    fn swap_in_hidden_stack_list_member(
        &mut self,
        pane_id: PaneId,
        client_id: Option<ClientId>,
    ) -> bool {
        if !self.pane_is_hidden_stack_list_member(&pane_id) {
            return false;
        }
        match self.stack_list_id_of_member(&pane_id) {
            Some(stack_list_id) => {
                self.select_stack_list_member(stack_list_id, pane_id, client_id);
                true
            },
            None => false,
        }
    }
    fn floating_stack_headers_visible(&self, list: &StackList) -> bool {
        self.floating_panes.fullscreen_pane_id().is_none()
            && self
                .floating_panes
                .get_pane(list.visible)
                .map(|pane| {
                    let bottom_frame = usize::from(!pane.borderless());
                    pane.rows() > list.members.len() + 1 + bottom_frame
                })
                .unwrap_or(false)
    }
    fn stack_list_member_at_point(&self, pane_id: PaneId, point: &Position) -> PaneId {
        let Some(stack_list_id) = self.stack_list_id_of_member(&pane_id) else {
            return pane_id;
        };
        let Some(list) = self.stack_lists.get(&stack_list_id) else {
            return pane_id;
        };
        if self.floating_panes.panes_contain(&list.visible)
            && !self.floating_stack_headers_visible(list)
        {
            return list.visible;
        }
        let Some(pane) = self
            .tiled_panes
            .get_pane(pane_id)
            .or_else(|| self.floating_panes.get_pane(pane_id))
        else {
            return pane_id;
        };
        let geom = pane.current_geom();
        if point.line() < geom.y as isize {
            return pane_id;
        }
        let rank = (point.line() - geom.y as isize) as usize;
        list.members.get(rank).copied().unwrap_or(pane_id)
    }
    pub fn suppressed_stack_list_members(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.suppressed_panes
            .iter()
            .filter(|(pane_id, _)| self.stack_list_of_member.contains_key(pane_id))
            .map(|(pane_id, suppressed)| (pane_id, &suppressed.pane))
    }
    pub fn stack_list_serialization_geoms(&self) -> HashMap<PaneId, PaneGeom> {
        let mut synthetic_geoms = HashMap::new();
        if self.stack_lists.is_empty() {
            return synthetic_geoms;
        }
        let mut next_synthetic_stack_id = self
            .tiled_panes
            .get_panes()
            .filter_map(|(_, pane)| pane.position_and_size().stacked)
            .max()
            .map(|highest_stack_id| highest_stack_id + 1)
            .unwrap_or(0);
        for list in self.stack_lists.values() {
            let Some(visible_pane) = self.tiled_panes.get_pane(list.visible) else {
                continue;
            };
            let full_rect = visible_pane.position_and_size();
            let collapsed_member_count = list.members.len().saturating_sub(1);
            let mut running_y = full_rect.y;
            for member in &list.members {
                let mut member_geom = full_rect;
                member_geom.stacked = Some(next_synthetic_stack_id);
                member_geom.y = running_y;
                if *member == list.visible {
                    member_geom.rows.set_inner(
                        full_rect
                            .rows
                            .as_usize()
                            .saturating_sub(collapsed_member_count),
                    );
                } else {
                    member_geom.rows = Dimension::fixed(1);
                    member_geom.logical_position =
                        self.suppressed_panes.get(member).and_then(|suppressed| {
                            suppressed.pane.position_and_size().logical_position
                        });
                }
                running_y += member_geom.rows.as_usize();
                synthetic_geoms.insert(*member, member_geom);
            }
            next_synthetic_stack_id += 1;
        }
        synthetic_geoms
    }
    pub fn hidden_stack_list_members_for_serialization(
        &self,
        floating: bool,
    ) -> Vec<(PaneId, &dyn Pane, PaneGeom)> {
        let synthetic_geoms = self.stack_list_serialization_geoms();
        let mut hidden_members = vec![];
        for list in self.stack_lists.values() {
            let floating_pane = self.floating_panes.get_pane(list.visible);
            if floating_pane.is_some() != floating {
                continue;
            }
            for member in &list.members {
                if *member == list.visible {
                    continue;
                }
                let Some(member_geom) = floating_pane
                    .map(|pane| pane.position_and_size())
                    .or_else(|| synthetic_geoms.get(member).copied())
                else {
                    continue;
                };
                // A member with a scrollback editor open on it *is* the editor -
                // `track_stack_list_member_id_swap` made the editor's id the member
                // id. What belongs in a layout is the pane being edited, which is
                // suppressed behind it.
                let resolved_pane = self
                    .scrollback_editor_subject(*member)
                    .or_else(|| self.suppressed_panes.get(member));
                if let Some(suppressed) = resolved_pane {
                    hidden_members.push((
                        suppressed.pane.pid(),
                        suppressed.pane.as_ref(),
                        member_geom,
                    ));
                }
            }
        }
        hidden_members
    }
    /// The pane, if any, whose place in the grid `stand_in` is *currently*
    /// holding, and which closing `stand_in` would therefore swap back in.
    ///
    /// Not the same question as `hidden_behind`: a stand-in that has itself been
    /// suppressed - a stack-list member with a scrollback editor open on it, which
    /// the user then clicked away from - holds nobody's place while it is off
    /// screen. Swapping into a slot it does not have would drop the pane.
    fn stands_in_for_a_pane(&self, stand_in: PaneId) -> Option<PaneId> {
        if !self.tiled_panes.panes_contain(&stand_in)
            && !self.floating_panes.panes_contain(&stand_in)
            && !self.overlay_panes.panes_contain(&stand_in)
        {
            return None;
        }
        self.hidden_behind(stand_in)
    }
    /// The pane, if any, that `stand_in` is holding the place of.
    fn hidden_behind(&self, stand_in: PaneId) -> Option<PaneId> {
        self.suppressed_panes
            .iter()
            .find(|(_, suppressed)| {
                suppressed
                    .restored_by
                    .map(|restored_by| restored_by.pane_id == stand_in)
                    .unwrap_or(false)
            })
            .map(|(pane_id, _)| *pane_id)
    }
    /// The pane whose scrollback `stand_in` is editing.
    fn scrollback_editor_subject(&self, stand_in: PaneId) -> Option<&SuppressedPane> {
        self.suppressed_panes.values().find(|suppressed| {
            suppressed
                .restored_by
                .map(|restored_by| {
                    restored_by.is_scrollback_editor && restored_by.pane_id == stand_in
                })
                .unwrap_or(false)
        })
    }
    fn refresh_or_dissolve_stack_list(&mut self, stack_list_id: StackListId) {
        let (member_count, visible) = match self.stack_lists.get(&stack_list_id) {
            Some(list) => (list.members.len(), list.visible),
            None => return,
        };
        if member_count <= 1 {
            self.dissolve_stack_list(stack_list_id);
        } else {
            self.reserved_top_rows
                .borrow_mut()
                .insert(visible, member_count + 1);
            self.tiled_panes.reapply_pane_frames();
            self.resize_stack_list_hidden_members(stack_list_id);
        }
    }
    fn select_stack_list_member(
        &mut self,
        stack_list_id: StackListId,
        target_member: PaneId,
        client_id: Option<ClientId>,
    ) {
        let current_visible = match self.stack_lists.get(&stack_list_id) {
            Some(list) => list.visible,
            None => return,
        };
        if current_visible == target_member {
            return;
        }
        let target_box = match self.suppressed_panes.remove(&target_member) {
            Some(suppressed) => suppressed.pane,
            None => {
                log::error!(
                    "stack-list member {:?} not found in suppressed panes",
                    target_member
                );
                return;
            },
        };
        let removed_visible = if self.floating_panes.panes_contain(&current_visible) {
            match self
                .floating_panes
                .replace_pane(current_visible, target_box)
            {
                Ok(pane) => Some(pane),
                Err(error) => {
                    Err::<(), _>(error).non_fatal();
                    None
                },
            }
        } else {
            self.tiled_panes.replace_pane(current_visible, target_box)
        };
        if let Some(removed_visible) = removed_visible {
            // Whatever either of these two was standing in for stays suppressed
            // behind it, wherever it goes: an entry names its stand-in, not the
            // other way around, so a member changing places changes nothing.
            self.park_pane(removed_visible);
        }
        if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
            list.visible = target_member;
        }
        self.reserved_top_rows.borrow_mut().remove(&current_visible);
        if let Some(client_id) = client_id {
            if self.floating_panes.panes_contain(&target_member) {
                self.floating_panes.focus_pane(target_member, client_id);
            } else {
                self.tiled_panes.focus_pane(target_member, client_id);
            }
        }
        self.refresh_or_dissolve_stack_list(stack_list_id);
        self.set_force_render();
        self.set_should_clear_display_before_rendering();
    }
    fn move_focus_within_stack_list(&mut self, client_id: ClientId, down: bool) -> bool {
        let active_pane_id = match if self.floating_panes.panes_are_visible() {
            self.floating_panes.active_pane_id(client_id)
        } else {
            self.tiled_panes.get_active_pane_id(client_id)
        } {
            Some(id) => id,
            None => return false,
        };
        let stack_list_id = match self.stack_list_id_of_member(&active_pane_id) {
            Some(id) => id,
            None => return false,
        };
        let target_member = {
            let list = match self.stack_lists.get(&stack_list_id) {
                Some(l) => l,
                None => return false,
            };
            if list.visible != active_pane_id {
                return false;
            }
            let rank = match list.rank_of(&active_pane_id) {
                Some(r) => r,
                None => return false,
            };
            let target_rank = if down {
                rank + 1
            } else if rank == 0 {
                return false;
            } else {
                rank - 1
            };
            match list.members.get(target_rank) {
                Some(member) => *member,
                None => return false,
            }
        };
        self.select_stack_list_member(stack_list_id, target_member, Some(client_id));
        true
    }
    fn close_stack_list_member(&mut self, id: PaneId, exit_status: Option<i32>) -> bool {
        match self.remove_stack_list_member(id) {
            StackListRemoval::Removed(removed) => {
                if let (Some(exit_status), Some(mut removed)) = (exit_status, removed) {
                    removed.update_exit_status(exit_status);
                }
                true
            },
            StackListRemoval::DissolvedInPlace | StackListRemoval::NotRemoved => false,
        }
    }
    fn remove_stack_list_member(&mut self, id: PaneId) -> StackListRemoval {
        let stack_list_id = match self.stack_list_id_of_member(&id) {
            Some(sid) => sid,
            None => return StackListRemoval::NotRemoved,
        };
        let is_visible = self
            .stack_lists
            .get(&stack_list_id)
            .map(|l| l.visible == id)
            .unwrap_or(false);
        let removed = if is_visible {
            let (members, rank) = {
                let list = match self.stack_lists.get(&stack_list_id) {
                    Some(l) => l,
                    None => return StackListRemoval::NotRemoved,
                };
                (list.members.clone(), list.rank_of(&id).unwrap_or(0))
            };
            let promote_target = members
                .get(rank + 1)
                .copied()
                .or_else(|| members[..rank].iter().last().copied());
            let promote_target = match promote_target {
                Some(target) => target,
                None => {
                    self.dissolve_stack_list(stack_list_id);
                    self.reserved_top_rows.borrow_mut().remove(&id);
                    return StackListRemoval::DissolvedInPlace;
                },
            };
            let target_box = match self.suppressed_panes.remove(&promote_target) {
                Some(suppressed) => suppressed.pane,
                None => {
                    log::error!("stack-list promote target not found in suppressed panes");
                    return StackListRemoval::NotRemoved;
                },
            };
            let removed_visible = if self.floating_panes.panes_contain(&id) {
                match self.floating_panes.replace_pane(id, target_box) {
                    Ok(pane) => Some(pane),
                    Err(error) => {
                        Err::<(), _>(error).non_fatal();
                        None
                    },
                }
            } else {
                self.tiled_panes.replace_pane(id, target_box)
            };
            self.stack_list_of_member.remove(&id);
            self.reserved_top_rows.borrow_mut().remove(&id);
            if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
                list.members.retain(|m| *m != id);
                list.visible = promote_target;
            }
            self.refresh_or_dissolve_stack_list(stack_list_id);
            removed_visible
        } else {
            // A member with something suppressed behind it hands its place in the
            // list to that pane rather than taking it down with it - the member was
            // only ever standing in for it.
            let substitute_member = self.hidden_behind(id);
            let removed = self.suppressed_panes.remove(&id).map(|s| s.pane);
            if let Some(substitute_member) = substitute_member {
                if let Some(suppressed) = self.suppressed_panes.get_mut(&substitute_member) {
                    suppressed.restored_by = None;
                }
            }
            self.stack_list_of_member.remove(&id);
            self.reserved_top_rows.borrow_mut().remove(&id);
            if let Some(substitute_member) = substitute_member {
                self.stack_list_of_member
                    .insert(substitute_member, stack_list_id);
                if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
                    for member in list.members.iter_mut() {
                        if *member == id {
                            *member = substitute_member;
                        }
                    }
                }
            } else if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
                list.members.retain(|m| *m != id);
            }
            self.refresh_or_dissolve_stack_list(stack_list_id);
            removed
        };
        self.set_force_render();
        self.set_should_clear_display_before_rendering();
        StackListRemoval::Removed(removed)
    }
    fn track_stack_list_member_id_swap(&mut self, old_id: PaneId, new_id: PaneId) {
        let stack_list_id = match self.stack_list_id_of_member(&old_id) {
            Some(sid) => sid,
            None => return,
        };
        self.stack_list_of_member.remove(&old_id);
        self.stack_list_of_member.insert(new_id, stack_list_id);
        let mut swapped_visible_member = false;
        if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
            for member in list.members.iter_mut() {
                if *member == old_id {
                    *member = new_id;
                }
            }
            if list.visible == old_id {
                list.visible = new_id;
                swapped_visible_member = true;
            }
        }
        {
            let mut reserved = self.reserved_top_rows.borrow_mut();
            if let Some(reserved_rows) = reserved.remove(&old_id) {
                reserved.insert(new_id, reserved_rows);
            }
        }
        if swapped_visible_member {
            self.refresh_or_dissolve_stack_list(stack_list_id);
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
    }
    fn dissolve_stack_list(&mut self, stack_list_id: StackListId) {
        if let Some(list) = self.stack_lists.get(&stack_list_id) {
            let members = list.members.clone();
            let floating = self.floating_panes.panes_contain(&list.visible);
            self.drop_stack_list_ledger(stack_list_id, &members);
            self.tiled_panes.reapply_pane_frames();
            if floating {
                self.floating_panes.set_pane_frames().non_fatal();
            }
        }
    }
    fn drop_stack_list_ledger(&mut self, stack_list_id: StackListId, members: &[PaneId]) {
        self.stack_lists.remove(&stack_list_id);
        let mut reserved = self.reserved_top_rows.borrow_mut();
        for member in members {
            self.stack_list_of_member.remove(member);
            reserved.remove(member);
        }
    }
    fn degroupify(&mut self, stack_list_id: StackListId) {
        let (members, visible) = match self.stack_lists.get(&stack_list_id) {
            Some(list) => (list.members.clone(), list.visible),
            None => return,
        };
        if self.floating_panes.panes_contain(&visible) {
            return;
        }
        let n = members.len();
        if n == 0 || !self.tiled_panes.panes_contain(&visible) {
            self.drop_stack_list_ledger(stack_list_id, &members);
            return;
        }
        let geoms = self.tiled_panes.stack_panes(visible, n);
        if geoms.len() != n {
            log::error!("degroupify: could not compute classic stack geoms");
            return;
        }
        for (i, member) in members.iter().enumerate() {
            let mut geom = geoms[i];
            if *member == visible {
                geom.logical_position = self
                    .tiled_panes
                    .get_pane(*member)
                    .and_then(|p| p.position_and_size().logical_position);
                self.tiled_panes.set_geom_for_pane_with_id(member, geom);
            } else if let Some(mut suppressed) = self.suppressed_panes.remove(member) {
                geom.logical_position = suppressed.pane.position_and_size().logical_position;
                suppressed.pane.set_geom(geom);
                self.tiled_panes
                    .add_pane_with_existing_geom(*member, suppressed.pane);
            }
        }
        self.drop_stack_list_ledger(stack_list_id, &members);
        self.tiled_panes.expand_pane_in_stack(visible);
        self.tiled_panes.reapply_pane_frames();
        self.set_force_render();
        self.set_should_clear_display_before_rendering();
    }
    // the classic layout solver cannot see suppressed stack-list members; dissolve
    // lists into in-grid stacks first, groupify_all re-forms them on mode entry
    fn degroupify_all(&mut self) {
        let ids: Vec<StackListId> = self.stack_lists.keys().copied().collect();
        for id in ids {
            self.degroupify(id);
        }
    }
    fn dissolve_stack_lists_for_classic_mutation(&mut self) {
        if self.stacked_pane_list_is_active() {
            self.degroupify_all();
        }
    }
    pub fn sync_stacked_pane_list_mode(&mut self) {
        if self.stacked_pane_list_is_active() {
            self.groupify_all();
        } else if self.has_stack_lists() {
            self.degroupify_all();
        }
    }
    // the inverse of degroupify_all: adopts classic in-grid stacks into stack
    // lists, suppressing all but the visible member of each
    fn groupify_all(&mut self) {
        if self.tiled_panes.fullscreen_is_active() {
            return;
        }
        let mut stacks: HashMap<usize, Vec<(usize, PaneId, bool)>> = HashMap::new();
        for (id, pane) in self.tiled_panes.get_panes() {
            let geom = pane.position_and_size();
            if let Some(stack_id) = geom.stacked {
                let is_expanded = !geom.rows.is_fixed();
                stacks
                    .entry(stack_id)
                    .or_default()
                    .push((geom.y, *id, is_expanded));
            }
        }
        let mut converted_any = false;
        for (_stack_id, mut members) in stacks {
            if members.len() < 2 {
                continue;
            }
            members.sort_by_key(|(y, _, _)| *y);
            let representative = members[0].1;
            let rect = match self.tiled_panes.position_and_size_of_stack(&representative) {
                Some(r) => r,
                None => continue,
            };
            let visible = members
                .iter()
                .find(|(_, _, expanded)| *expanded)
                .map(|(_, id, _)| *id)
                .unwrap_or_else(|| members.last().map(|(_, id, _)| *id).unwrap());
            let ordered_ids: Vec<PaneId> = members.iter().map(|(_, id, _)| *id).collect();
            let logical_position = self
                .tiled_panes
                .get_pane(visible)
                .and_then(|p| p.position_and_size().logical_position);
            for (_, id, _) in &members {
                if *id == visible {
                    continue;
                }
                if let Some(pane) = self.tiled_panes.extract_pane(*id) {
                    self.park_pane(pane);
                }
            }
            if let Some(vpane) = self.tiled_panes.get_pane_mut(visible) {
                let mut g = rect;
                g.stacked = None;
                g.logical_position = logical_position;
                vpane.set_geom(g);
            }
            let stack_list_id = self.next_stack_list_id;
            self.next_stack_list_id += 1;
            for id in &ordered_ids {
                self.stack_list_of_member.insert(*id, stack_list_id);
            }
            self.stack_lists.insert(
                stack_list_id,
                StackList {
                    members: ordered_ids,
                    visible,
                },
            );
            self.refresh_or_dissolve_stack_list(stack_list_id);
            converted_any = true;
        }
        if converted_any {
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
    }
    fn resize_all_stack_list_hidden_members(&mut self) {
        let ids: Vec<StackListId> = self.stack_lists.keys().copied().collect();
        for id in ids {
            self.resize_stack_list_hidden_members(id);
        }
    }
    fn resize_stack_list_hidden_members(&mut self, stack_list_id: StackListId) {
        if let Some(list) = self.stack_lists.get(&stack_list_id) {
            let headers_visible = self.floating_stack_headers_visible(list);
            if self.floating_panes.fullscreen_pane_id() != Some(list.visible) {
                if let Some(pane) = self.floating_panes.get_pane_mut(list.visible) {
                    let mut offset = if pane.borderless() {
                        Offset::default()
                    } else {
                        Offset::frame(1)
                    };
                    if headers_visible {
                        offset.top = list.members.len() + 1;
                    }
                    if pane.get_content_offset() != offset {
                        pane.set_content_offset(offset);
                        resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                            .non_fatal();
                    }
                }
            }
        }

        let (visible_geom, visible_offset, hidden_members) = {
            let list = match self.stack_lists.get(&stack_list_id) {
                Some(l) => l,
                None => return,
            };
            let (geom, offset) = match self
                .tiled_panes
                .get_pane(list.visible)
                .or_else(|| self.floating_panes.get_pane(list.visible))
            {
                Some(p) => (p.position_and_size(), p.get_content_offset()),
                None => return,
            };
            let hidden: Vec<PaneId> = list
                .members
                .iter()
                .copied()
                .filter(|m| *m != list.visible)
                .collect();
            (geom, offset, hidden)
        };
        for member in hidden_members {
            if let Some(suppressed) = self.suppressed_panes.get_mut(&member) {
                let pane = &mut suppressed.pane;
                if pane.position_and_size() != visible_geom
                    || pane.get_content_offset() != visible_offset
                {
                    pane.set_geom(visible_geom);
                    pane.set_content_offset(visible_offset);
                    resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                        .non_fatal();
                }
            }
        }
    }
    fn render_stack_list_headers(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
        floating: bool,
    ) -> Result<()> {
        if self.stack_lists.is_empty() || (!floating && self.tiled_panes.fullscreen_is_active()) {
            return Ok(());
        }
        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }
        if connected_clients.is_empty() {
            return Ok(());
        }
        let multiple_users_exist_in_session = { self.connected_clients_in_app.borrow().len() > 1 };
        let should_draw_pane_frames = self.pane_frame_style.draws_full_frames();
        let session_is_mirrored = self.stack_list_session_is_mirrored;
        let current_pane_group: HashMap<ClientId, Vec<PaneId>> =
            { self.current_pane_group.borrow().clone_inner() };
        let active_panes: HashMap<ClientId, PaneId> = connected_clients
            .iter()
            .filter_map(|c| {
                let pane = if floating {
                    self.floating_panes.active_pane_id(*c)
                } else {
                    self.tiled_panes.get_active_pane_id(*c)
                };
                pane.map(|p| (*c, p))
            })
            .collect();
        let client_modes: Vec<(ClientId, InputMode)> = {
            let mode_info = self.mode_info.borrow();
            connected_clients
                .iter()
                .map(|c| {
                    let mode = mode_info.get(c).unwrap_or(&self.default_mode_info).mode;
                    (*c, mode)
                })
                .collect()
        };
        let lists: Vec<StackList> = self.stack_lists.values().cloned().collect();
        for list in lists {
            if self.floating_panes.panes_contain(&list.visible) != floating
                || (floating && !self.floating_stack_headers_visible(&list))
            {
                continue;
            }
            let z_index = if floating {
                self.floating_panes
                    .get_pane_z_index(list.visible)
                    .map(|index| index + 1)
            } else {
                None
            };

            let (rect, visible_content_offset) = match self
                .tiled_panes
                .get_pane(list.visible)
                .or_else(|| self.floating_panes.get_pane(list.visible))
            {
                Some(p) => (p.position_and_size(), p.get_content_offset()),
                None => continue,
            };
            let entry_x = rect.x + visible_content_offset.left;
            let entry_cols = rect
                .cols
                .as_usize()
                .saturating_sub(visible_content_offset.left + visible_content_offset.right);
            let widest_member_title = list
                .members
                .iter()
                .filter_map(|member| {
                    if *member == list.visible {
                        self.get_pane_with_id(*member)
                    } else {
                        self.suppressed_panes.get(member).map(|s| &*s.pane)
                    }
                })
                .map(|pane| pane.stack_list_entry_label().width())
                .max()
                .unwrap_or(0);
            let stack_is_focused = active_panes.values().any(|p_id| *p_id == list.visible);
            for (rank, member) in list.members.iter().enumerate() {
                let mut header_geom = rect;
                header_geom.x = entry_x;
                header_geom.cols.set_inner(entry_cols);
                header_geom.y = rect.y + rank;
                header_geom.rows = Dimension::fixed(1);
                header_geom.stacked = None;
                let pane_box = if *member == list.visible {
                    if floating {
                        self.floating_panes.get_pane_mut(*member)
                    } else {
                        self.tiled_panes.get_pane_mut(*member)
                    }
                } else {
                    self.suppressed_panes.get_mut(member).map(|s| &mut s.pane)
                };
                let pane_box = match pane_box {
                    Some(p) => p,
                    None => continue,
                };
                let mut pane_contents_and_ui = PaneContentsAndUi::new(
                    pane_box,
                    output,
                    self.style,
                    &active_panes,
                    multiple_users_exist_in_session,
                    z_index,
                    false,
                    false,
                    should_draw_pane_frames,
                    &self.mouse_hover_pane_id,
                    current_pane_group.clone(),
                    false,
                    false,
                    self.mouse_scroll_resize,
                    self.dimmed_clients.clone(),
                );
                pane_contents_and_ui.set_frame_geom_override(Some(header_geom));
                pane_contents_and_ui.set_stack_list_entry(
                    Some(widest_member_title),
                    *member == list.visible,
                    stack_is_focused,
                );
                for (client_id, client_mode) in &client_modes {
                    pane_contents_and_ui.render_pane_frame(
                        *client_id,
                        *client_mode,
                        session_is_mirrored,
                        false,
                        true,
                        true,
                        true,
                    )?;
                }
            }
            let padding_row = vec![EMPTY_TERMINAL_CHARACTER; entry_cols];
            let padding_chunk =
                CharacterChunk::new(padding_row, entry_x, rect.y + list.members.len());
            output.add_character_chunks_to_multiple_clients(
                vec![padding_chunk],
                connected_clients.iter().copied(),
                z_index,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        client_id: ClientId,
        blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> Result<()> {
        self.swap_layouts
            .set_base_layout((layout.clone(), floating_panes_layout.clone()));
        match LayoutApplier::new(
            &self.viewport,
            &self.senders,
            &self.sixel_image_store,
            &self.kitty_image_store,
            &self.link_handler,
            &self.terminal_emulator_colors,
            &self.terminal_emulator_color_codes,
            &self.character_cell_size,
            &self.connected_clients_in_app,
            &self.style,
            &self.display_area,
            &mut self.tiled_panes,
            &mut self.floating_panes,
            self.pane_frame_style,
            &mut self.focus_pane_id,
            self.os_api.as_ref(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            blocking_terminal,
        )
        .apply_layout(
            layout,
            floating_panes_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
            client_id,
        ) {
            Ok(should_show_floating_panes) => {
                // the layout wants floating panes shown while they are hidden, or hidden while
                // they are shown: toggle either way
                if should_show_floating_panes != self.floating_panes.panes_are_visible() {
                    self.toggle_floating_panes(Some(client_id), None, None)
                        .non_fatal();
                }
                self.tiled_panes.reapply_pane_frames();
                self.is_pending = false;
                self.apply_buffered_instructions().non_fatal();
            },
            Err(e) => {
                // TODO: this should only happen due to an erroneous layout created by user
                // configuration that was somehow not caught in our KDL layout parser
                // we should still be able to properly recover from this with a useful error
                // message though
                log::error!("Failed to apply layout: {}", e);
                self.tiled_panes.reapply_pane_frames();
                self.is_pending = false;
                self.apply_buffered_instructions().non_fatal();
            },
        }
        self.refresh_overlay_expansion();
        if let Some(supported) = self.kitty_host_support {
            self.tiled_panes.update_pane_kitty_host_support(supported);
            self.floating_panes
                .update_pane_kitty_host_support(supported);
            self.overlay_panes.update_pane_kitty_host_support(supported);
        }
        if let Some(supported) = self.sixel_host_support {
            self.tiled_panes.update_pane_sixel_host_support(supported);
            self.floating_panes
                .update_pane_sixel_host_support(supported);
            self.overlay_panes.update_pane_sixel_host_support(supported);
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn override_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        mut new_swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
        mut new_swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
        blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> Result<()> {
        let new_swap_tiled_layouts = new_swap_tiled_layouts.take().unwrap_or_default();
        let new_swap_floating_layouts = new_swap_floating_layouts.take().unwrap_or_default();
        self.swap_layouts
            .set_swap_tiled_layouts(new_swap_tiled_layouts);
        self.swap_layouts
            .set_swap_floating_layouts(new_swap_floating_layouts);
        self.swap_layouts
            .set_base_layout((layout.clone(), floating_panes_layout.clone()));
        match LayoutApplier::new(
            &self.viewport,
            &self.senders,
            &self.sixel_image_store,
            &self.kitty_image_store,
            &self.link_handler,
            &self.terminal_emulator_colors,
            &self.terminal_emulator_color_codes,
            &self.character_cell_size,
            &self.connected_clients_in_app,
            &self.style,
            &self.display_area,
            &mut self.tiled_panes,
            &mut self.floating_panes,
            self.pane_frame_style,
            &mut self.focus_pane_id,
            self.os_api.as_ref(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            blocking_terminal,
        )
        .override_layout(
            layout,
            floating_panes_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            client_id,
        ) {
            Ok(should_show_floating_panes) => {
                // the layout wants floating panes shown while they are hidden, or hidden while
                // they are shown: toggle either way
                if should_show_floating_panes != self.floating_panes.panes_are_visible() {
                    self.toggle_floating_panes(Some(client_id), None, None)
                        .non_fatal();
                }

                // this is essentially another pass of the layout applier
                // we do this because the layout applier does not know about swap layouts, and in
                // this case we might have had to re-add existing panes that were not in the
                // overridden layout (eg. if we had more panes than were in the layout). In such a
                // case, we would like to make sure these extra panes fit the current swap layout
                self.swap_layouts.set_is_tiled_damaged();
                self.swap_layouts.set_is_floating_damaged();
                let _ = self.relayout_tiled_panes(false);
                let _ = self.relayout_floating_panes(false);

                self.tiled_panes.reapply_pane_frames();
                self.is_pending = false;

                LayoutApplier::offset_viewport(
                    self.viewport.clone(),
                    self.display_area.clone(),
                    &mut self.tiled_panes,
                    self.pane_frame_style,
                );
                self.refresh_overlay_expansion();

                self.apply_buffered_instructions().non_fatal();
            },
            Err(e) => {
                // TODO: this should only happen due to an erroneous layout created by user
                // configuration that was somehow not caught in our KDL layout parser
                // we should still be able to properly recover from this with a useful error
                // message though
                log::error!("Failed to apply layout: {}", e);
                self.is_pending = false;
            },
        }
        Ok(())
    }
    pub fn swap_layout_info(&self) -> (Option<String>, bool) {
        if self.floating_panes.panes_are_visible() {
            self.swap_layouts.floating_layout_info()
        } else {
            let selectable_tiled_panes =
                self.tiled_panes.get_panes().filter(|(_, p)| p.selectable());
            if selectable_tiled_panes.count() + self.suppressed_stack_list_members().count() > 1 {
                self.swap_layouts.tiled_layout_info()
            } else {
                // no layout for single pane
                (None, false)
            }
        }
    }
    fn relayout_floating_panes(&mut self, search_backwards: bool) -> Result<()> {
        if let Some(layout_candidate) = self
            .swap_layouts
            .swap_floating_panes(&self.floating_panes, search_backwards)
        {
            LayoutApplier::new(
                &self.viewport,
                &self.senders,
                &self.sixel_image_store,
                &self.kitty_image_store,
                &self.link_handler,
                &self.terminal_emulator_colors,
                &self.terminal_emulator_color_codes,
                &self.character_cell_size,
                &self.connected_clients_in_app,
                &self.style,
                &self.display_area,
                &mut self.tiled_panes,
                &mut self.floating_panes,
                self.pane_frame_style,
                &mut self.focus_pane_id,
                self.os_api.as_ref(),
                self.debug,
                self.arrow_fonts,
                self.styled_underlines,
                self.osc8_hyperlinks,
                self.explicitly_disable_kitty_keyboard_protocol,
                None,
            )
            .apply_floating_panes_layout_to_existing_panes(&layout_candidate)
            .non_fatal();
        }
        self.set_force_render();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .with_context(|| "failed to apply cached resizes".to_string())?;
        Ok(())
    }
    /// Boundary UI panes occupy the area outside the terminal viewport.
    pub fn arrangement_pane_ids(&self) -> Vec<PaneId> {
        let viewport = self.get_viewport();
        self.get_tiled_pane_ids()
            .into_iter()
            .chain(self.suppressed_stack_list_members().map(|(id, _)| *id))
            .filter(|id| {
                self.get_pane_with_id(*id).is_some_and(|pane| {
                    let geom = pane.position_and_size();
                    geom.x < viewport.x + viewport.cols
                        && geom.x + geom.cols.as_usize() > viewport.x
                        && geom.y < viewport.y + viewport.rows
                        && geom.y + geom.rows.as_usize() > viewport.y
                })
            })
            .collect()
    }

    pub fn arrangement_stacks(&self) -> Vec<StackList> {
        self.stack_lists.values().cloned().collect()
    }

    /// Intermediate extraction must not trigger automatic layouts that dissolve lists.
    pub fn prepare_arrangement(&mut self) {
        self.tiled_panes.unset_fullscreen();
        self.swap_layouts.set_is_tiled_damaged();
        self.swap_layouts.set_is_floating_damaged();
    }

    /// Reassemble the preflighted partitions using ordinary member removal and sizing.
    pub fn restore_arrangement_stacks(&mut self, stacks: &[StackList]) {
        for stack in stacks {
            let mut panes = vec![];
            for id in &stack.members {
                if let Some(pane) = self.extract_pane(*id, true) {
                    panes.push(pane);
                }
            }
            for pane in panes {
                if pane.pid() == stack.visible {
                    self.tiled_panes
                        .add_pane_with_existing_geom(pane.pid(), pane);
                } else {
                    self.park_pane(pane);
                }
            }
            if stack.members.len() == 1 {
                continue;
            }
            let id = self.next_stack_list_id;
            self.next_stack_list_id += 1;
            for member in &stack.members {
                self.stack_list_of_member.insert(*member, id);
            }
            self.stack_lists.insert(id, stack.clone());
            self.refresh_or_dissolve_stack_list(id);
        }
    }

    pub fn restore_arrangement_stack_visibility(&mut self, originals: &[StackList]) {
        let remaining: Vec<_> = self
            .stack_lists
            .iter()
            .map(|(id, stack)| (*id, stack.clone()))
            .collect();
        for (id, stack) in remaining {
            if let Some(original) = originals
                .iter()
                .find(|original| original.members.contains(&stack.visible))
            {
                let visible = if stack.members.contains(&original.visible) {
                    Some(original.visible)
                } else {
                    original
                        .members
                        .iter()
                        .find(|member| stack.members.contains(member))
                        .copied()
                };
                if let Some(visible) = visible {
                    self.select_stack_list_member(id, visible, None);
                }
            }
        }
    }

    /// Applies geometry to existing panes only. No terminal or plugin is created.
    pub fn arrange_existing_panes(
        &mut self,
        layout: &TiledPaneLayout,
        order: &[PaneId],
    ) -> Result<()> {
        self.tiled_panes.unset_fullscreen();
        LayoutApplier::new(
            &self.viewport,
            &self.senders,
            &self.sixel_image_store,
            &self.kitty_image_store,
            &self.link_handler,
            &self.terminal_emulator_colors,
            &self.terminal_emulator_color_codes,
            &self.character_cell_size,
            &self.connected_clients_in_app,
            &self.style,
            &self.display_area,
            &mut self.tiled_panes,
            &mut self.floating_panes,
            self.pane_frame_style,
            &mut self.focus_pane_id,
            self.os_api.as_ref(),
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            None,
        )
        .apply_tiled_panes_layout_in_order(layout, Some(order))?;
        self.is_pending = false;
        self.apply_buffered_instructions()?;
        self.tiled_panes.reapply_pane_frames();
        self.resize_all_stack_list_hidden_members();
        self.swap_layouts.set_is_tiled_damaged();
        self.set_should_clear_display_before_rendering();
        self.set_force_render();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)?;
        Ok(())
    }

    fn relayout_tiled_panes(&mut self, search_backwards: bool) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if let Some(layout_candidate) = self
            .swap_layouts
            .swap_tiled_panes(&self.tiled_panes, search_backwards)
        {
            let application_res = LayoutApplier::new(
                &self.viewport,
                &self.senders,
                &self.sixel_image_store,
                &self.kitty_image_store,
                &self.link_handler,
                &self.terminal_emulator_colors,
                &self.terminal_emulator_color_codes,
                &self.character_cell_size,
                &self.connected_clients_in_app,
                &self.style,
                &self.display_area,
                &mut self.tiled_panes,
                &mut self.floating_panes,
                self.pane_frame_style,
                &mut self.focus_pane_id,
                self.os_api.as_ref(),
                self.debug,
                self.arrow_fonts,
                self.styled_underlines,
                self.osc8_hyperlinks,
                self.explicitly_disable_kitty_keyboard_protocol,
                None,
            )
            .apply_tiled_panes_layout_to_existing_panes(&layout_candidate);
            if application_res.is_err() {
                self.swap_layouts.set_is_tiled_damaged();
                application_res.non_fatal();
            }
        } else {
            self.swap_layouts.set_is_tiled_damaged();
        }
        self.tiled_panes.reapply_pane_frames();
        let display_area = *self.display_area.borrow();
        // we do this so that the new swap layout has a chance to pass through the constraint system
        self.tiled_panes.resize(display_area);
        self.refresh_overlay_expansion();
        self.set_should_clear_display_before_rendering();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .with_context(|| "failed to apply cached resizes".to_string())?;
        Ok(())
    }
    pub fn previous_swap_layout(&mut self) -> Result<()> {
        let search_backwards = true;
        if self.floating_panes.panes_are_visible() {
            self.relayout_floating_panes(search_backwards)?;
        } else {
            self.relayout_tiled_panes(search_backwards)?;
        }
        Ok(())
    }
    pub fn next_swap_layout(&mut self) -> Result<()> {
        let search_backwards = false;
        if self.floating_panes.panes_are_visible() {
            self.relayout_floating_panes(search_backwards)?;
        } else {
            self.relayout_tiled_panes(search_backwards)?;
        }
        Ok(())
    }
    pub fn apply_buffered_instructions(&mut self) -> Result<()> {
        let buffered_instructions: Vec<BufferedTabInstruction> =
            std::mem::take(&mut self.pending_instructions);
        for buffered_instruction in buffered_instructions {
            match buffered_instruction {
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
                    // No exit hooks are fired for this replay (this is a tab-internal catch-up,
                    // not the live/buffered-plugin paths in Screen that bracket a hold with
                    // hooks), so a queued respawn can start immediately as before.
                    if let Some((terminal_pane_id, run_command, keep_scrollback)) =
                        self.hold_pane(terminal_id, exit_status, is_first_run, run_command)
                    {
                        self.run_command_in_pane_with_id(
                            terminal_pane_id,
                            run_command,
                            keep_scrollback,
                        );
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
        let tab_plugin_ids = self.get_plugin_ids();
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
        if let Some(hovered_pane_id) = hovered_pane_id {
            if Some(hovered_pane_id) != focused_pane_id {
                return hover_hint_variants();
            }
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
        self.prev_name = self.name.clone();
        self.prev_user_named = self.user_named;
    }
    /// A keystroke in rename mode. The tab is the user's from the first one, so the bar
    /// shows what is being typed rather than the project it would otherwise wear.
    pub fn edit_name_by_user(&mut self, edit: impl FnOnce(&mut String)) {
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
        let changed = self.name != self.prev_name || self.user_named != self.prev_user_named;
        self.name = self.prev_name.clone();
        self.user_named = self.prev_user_named;
        changed
    }
    /// What a lone pane is lending its tab, or `None` when it is lending nothing.
    ///
    /// Nothing is the common case: a pane that has set no title of its own has
    /// only "Pane #1" or its command to offer, and neither says more than the
    /// tab already does. Distinguishing that from a real loan is the whole point
    /// of returning an `Option` — a caller that has a *better* name in hand (a
    /// project, say) needs to know whether this one is actually a name or just
    /// the tab's own read back to it.
    ///
    /// The exit marker is not part of the loan. It describes the pane's fate
    /// rather than its identity, so it belongs to whichever name ends up on the
    /// tab — see [`Tab::exit_marker`].
    pub fn lent_pane_title(&self) -> Option<String> {
        if !self.single_pane_titles() || !self.tab_name_is_automatic() {
            return None;
        }
        let (_, pane) = self.get_selectable_tiled_content_panes().next()?;
        if !pane.has_explicit_title() {
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
        // Client ids are recycled, and a client arriving in a tab that already has clients does
        // not go through `visible(true)`. Whatever the id's previous holder was told about its
        // cursor says nothing about this one's terminal, so start it from scratch.
        self.forget_host_cursor_state(client_id);
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
            if let Some(first_active_floating_pane_id) =
                self.floating_panes.first_active_floating_pane_id()
            {
                self.floating_panes
                    .focus_pane_if_client_not_focused(first_active_floating_pane_id, client_id);
            }
            if let Some(first_active_tiled_pane_id) = self.tiled_panes.first_active_pane_id() {
                self.tiled_panes
                    .focus_pane_if_client_not_focused(first_active_tiled_pane_id, client_id);
            }
        } else {
            let mut pane_ids: Vec<PaneId> = self.tiled_panes.pane_ids().copied().collect();
            let focus_pane_id = self.focus_pane_id.or_else(|| {
                pane_ids.sort(); // TODO: make this predictable
                pane_ids.retain(|p| !self.tiled_panes.panes_to_hide_contains(*p));
                pane_ids.first().copied()
            });
            if let Some(focus_pane_id) = focus_pane_id {
                self.tiled_panes
                    .focus_pane_if_client_not_focused(focus_pane_id, client_id);
            }
            self.floating_panes
                .focus_first_pane_if_client_not_focused(client_id);
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
                });
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
        self.focus_pane_id = None;
        if let Some(c) = self.mode_info.borrow_mut().get_mut(&client_id) {
            c.change_to_default_mode();
        } // TODO: no races?
        self.connected_clients.borrow_mut().remove(&client_id);
        self.mouse_help_text_visible.remove(&client_id);
        self.mouse_last_pane_id.remove(&client_id);
        self.last_mouse_activity_time.remove(&client_id);
        self.set_client_dimmed(client_id, false);
        self.forget_host_cursor_state(client_id);
        self.set_force_render();
    }
    pub fn drain_connected_clients(
        &mut self,
        clients_to_drain: Option<Vec<ClientId>>,
    ) -> Vec<(ClientId, ModeInfo)> {
        // None => all clients
        let mut client_ids_to_mode_infos = vec![];
        let clients_to_drain = clients_to_drain
            .unwrap_or_else(|| self.connected_clients.borrow_mut().drain().collect());
        for client_id in clients_to_drain {
            client_ids_to_mode_infos.push(self.drain_single_client(client_id));
        }
        client_ids_to_mode_infos
    }
    pub fn drain_single_client(&mut self, client_id: ClientId) -> (ClientId, ModeInfo) {
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
        let err_context =
            || format!("failed to toggle embedded/floating pane for client {client_id}");
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        if self.floating_panes.panes_are_visible() {
            if let Some(focused_floating_pane_id) = self.floating_panes.active_pane_id(client_id) {
                if self.tiled_panes.has_room_for_new_pane() {
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
        let err_context = || {
            format!(
                "failed to toggle embedded/floating pane for pane_id {:?}",
                pane_id
            )
        };
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        if self.floating_panes.panes_contain(&pane_id) {
            if self.tiled_panes.has_room_for_new_pane() {
                let floating_pane_to_embed = self
                    .extract_pane(pane_id, true)
                    .with_context(|| {
                        format!("failed to find floating pane (ID: {pane_id:?}) to embed",)
                    })
                    .with_context(err_context)?;
                self.add_tiled_pane(floating_pane_to_embed, pane_id, false, client_id)?;
            }
        } else if self.tiled_panes.panes_contain(&pane_id) {
            if self.get_selectable_tiled_panes().count() <= 1
                && !self.pane_is_stack_list_member(&pane_id)
            {
                log::error!("Cannot float the last tiled pane...");
                // don't close the only pane on screen...
                return Ok(());
            }
            if let Some(embedded_pane_to_float) = self.extract_pane(pane_id, true) {
                self.add_floating_pane(embedded_pane_to_float, pane_id, None, true, client_id)?;
            }
        } else if self.pane_is_hidden_stack_list_member(&pane_id) {
            if let Some(hidden_member_to_float) = self.extract_pane(pane_id, true) {
                self.add_floating_pane(hidden_member_to_float, pane_id, None, true, client_id)?;
            }
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
    fn normalize_invoked_with_for_default_shell(&self, invoked_with: Option<Run>) -> Option<Run> {
        let default_shell_run_command = Run::Command(RunCommand {
            command: self.default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        });
        if invoked_with == Some(default_shell_run_command) {
            None
        } else {
            invoked_with
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        new_pane_placement: NewPanePlacement,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<()> {
        let invoked_with = self.normalize_invoked_with_for_default_shell(invoked_with);
        match new_pane_placement {
            NewPanePlacement::NoPreference { borderless } => self.new_no_preference_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                client_id,
                blocking_notification,
                borderless,
            ),
            NewPanePlacement::Tiled {
                direction: None,
                borderless,
            } => self.new_tiled_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                client_id,
                blocking_notification,
                borderless,
            ),
            NewPanePlacement::Tiled {
                direction: Some(direction),
                borderless,
            } => {
                let is_vertical = direction == Direction::Left || direction == Direction::Right;
                let focused_client_id = client_id.filter(|_| should_focus_pane);
                match focused_client_id {
                    Some(client_id) => {
                        if is_vertical {
                            self.vertical_split(
                                pid,
                                initial_pane_title,
                                client_id,
                                blocking_notification,
                                borderless,
                            )?;
                        } else {
                            self.horizontal_split(
                                pid,
                                initial_pane_title,
                                client_id,
                                blocking_notification,
                                borderless,
                            )?;
                        }
                    },
                    None => {
                        // Nobody to focus for (e.g. `zellij action new-pane -d right` against a
                        // session with no client attached): split the client's active pane if it
                        // has one, else the first selectable pane, else just add a tiled pane.
                        let target_pane_id = client_id
                            .and_then(|client_id| self.tiled_panes.get_active_pane_id(client_id))
                            .or_else(|| self.tiled_panes.first_selectable_pane_id());
                        match target_pane_id {
                            Some(target_pane_id) if is_vertical => {
                                self.vertical_split_of_pane_id(
                                    pid,
                                    initial_pane_title,
                                    invoked_with,
                                    target_pane_id,
                                    blocking_notification,
                                    borderless,
                                )?;
                            },
                            Some(target_pane_id) => {
                                self.horizontal_split_of_pane_id(
                                    pid,
                                    initial_pane_title,
                                    invoked_with,
                                    target_pane_id,
                                    blocking_notification,
                                    borderless,
                                )?;
                            },
                            None => {
                                self.new_tiled_pane(
                                    pid,
                                    initial_pane_title,
                                    invoked_with,
                                    start_suppressed,
                                    should_focus_pane,
                                    client_id,
                                    blocking_notification,
                                    borderless,
                                )?;
                            },
                        }
                    },
                }
                Ok(())
            },
            NewPanePlacement::Floating(floating_pane_coordinates) => self.new_floating_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                floating_pane_coordinates,
                blocking_notification,
            ),
            // The kind travels with the placement all the way into `OverlayPanes`. It used to be
            // flattened to `overlay: true` here and then re-derived from the plugin's own
            // configuration wherever it was needed - which is why a `--modal` *terminal* pane got
            // overlay geometry but no modal focus semantics.
            NewPanePlacement::Overlay(overlay) => self.new_overlay_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                overlay,
                blocking_notification,
            ),
            NewPanePlacement::InPlace {
                pane_id_to_replace,
                close_replaced_pane,
                borderless,
            } => self.new_in_place_pane(
                pid,
                initial_pane_title,
                invoked_with,
                pane_id_to_replace,
                close_replaced_pane,
                client_id,
                blocking_notification,
                borderless,
            ),
            NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                borderless,
            } => self.new_stacked_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                pane_id_to_stack_under,
                client_id,
                blocking_notification,
                borderless,
            ),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new_no_preference_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        let mut new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if let Some(borderless) = borderless {
            new_pane.set_borderless(borderless);
        }

        if start_suppressed {
            self.park_new_pane(new_pane).with_context(err_context)
        } else {
            // no overlay exclusion needed here any more: an overlay is not in `floating_panes`,
            // so "the client's active pane is a floating pane" can no longer accidentally mean
            // "the client is looking at a modal drawn over a hidden layer"
            let active_pane_is_floating = client_id
                .and_then(|client_id| self.get_active_pane_id(client_id))
                .is_some_and(|pane_id| self.floating_panes.panes_contain(&pane_id));
            if active_pane_is_floating && !self.floating_panes.panes_are_visible() {
                self.show_floating_panes();
            }
            let should_add_floating =
                self.floating_panes.panes_are_visible() || active_pane_is_floating;
            if should_focus_pane && should_add_floating {
                self.add_floating_pane(new_pane, pid, None, true, client_id)
            } else if should_focus_pane {
                // an overlay holds this client's focus while the floating layer is down, and
                // `get_active_pane_id` would keep preferring it over the pane we are about to
                // focus. The new pane is what the user just asked for, so a dismissible overlay
                // gives the keyboard up here and a modal keeps it - see
                // `defocus_pane_if_dismissible` for why the two differ. When the modal keeps it,
                // the new pane opens tiled but unfocused behind it.
                let keyboard_is_free = client_id.is_none_or(|client_id| {
                    self.overlay_panes.defocus_pane_if_dismissible(client_id)
                });
                if keyboard_is_free {
                    self.add_tiled_pane(new_pane, pid, false, client_id)
                } else {
                    self.add_tiled_pane(new_pane, pid, false, None)
                }
            } else if should_add_floating {
                self.add_floating_pane(new_pane, pid, None, false, client_id)
            } else {
                self.add_tiled_pane(new_pane, pid, false, None)
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new_tiled_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        if should_focus_pane {
            self.hide_floating_panes();
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        let mut new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if let Some(borderless) = borderless {
            new_pane.set_borderless(borderless);
        }

        if start_suppressed {
            self.park_new_pane(new_pane).with_context(err_context)
        } else {
            let focus_client_id = if should_focus_pane { client_id } else { None };
            self.add_tiled_pane(new_pane, pid, false, focus_client_id)
        }
    }
    #[allow(clippy::too_many_arguments)]
    /// Build the pane a `new_*_pane` path is about to place, without deciding where it goes.
    ///
    /// Shared by no-preference, tiled, stacked, floating, and overlay creation. Each caller
    /// retains its placement, focus, and borderless decisions.
    fn build_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<Box<dyn Pane>> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        Ok(match pid {
            PaneId::Terminal(term_pid) => {
                let next_terminal_position = self.get_next_terminal_position();
                PaneFactory::terminal(
                    term_pid,
                    PaneGeom::default(), // this will be filled out later
                    self.style,
                    next_terminal_position,
                    initial_pane_title.clone().unwrap_or_default(),
                    self.link_handler.clone(),
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
                )
            },
            PaneId::Plugin(plugin_pid) => {
                PaneFactory::plugin(
                    plugin_pid,
                    PaneGeom::default(), // this will be filled out later
                    self.senders
                        .to_plugin
                        .as_ref()
                        .with_context(err_context)?
                        .clone(),
                    initial_pane_title.unwrap_or("".to_owned()),
                    String::new(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.connected_clients_in_app
                        .borrow()
                        .iter()
                        .copied()
                        .collect(),
                    self.style,
                    invoked_with,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                )
            },
        })
    }

    pub fn create_warp_companion(
        &mut self,
        id: PaneId,
        layout: FloatingPaneLayout,
        held_command: HoldForCommand,
    ) -> Result<()> {
        let initial_title = match &layout.run {
            Some(Run::Command(command)) => Some(command.to_string()),
            Some(Run::Plugin(plugin)) => Some(plugin.location_string()),
            _ => None,
        };
        let mut pane = self.build_pane(id, initial_title, layout.run, None)?;
        pane.update_name("\0");
        pane.update_name(&layout.name.unwrap_or_default());
        pane.set_borderless(layout.borderless.unwrap_or(false));
        pane.set_pane_default_colors(layout.default_fg, layout.default_bg);
        if let Some(contents) = layout.pane_initial_contents {
            pane.handle_pty_bytes(contents.into_bytes());
            pane.handle_pty_bytes(b"\n\r".to_vec());
        }
        if let Some(command) = held_command {
            pane.hold(None, true, command);
        }
        self.park_new_pane(pane)
    }

    /// Park a pane in the suppressed panes rather than placing it, giving it a plausible geometry
    /// so that it looks right the moment something takes it back out.
    fn park_new_pane(&mut self, mut new_pane: Box<dyn Pane>) -> Result<()> {
        let err_context = || "failed to park new pane".to_string();
        let viewport = { *self.viewport.borrow() };
        let new_pane_geom = half_size_middle_geom(&viewport, 0);
        new_pane.set_active_at(Instant::now());
        new_pane.set_geom(new_pane_geom);
        new_pane.set_content_offset(Offset::frame(1));
        resize_pty!(
            new_pane,
            self.os_api,
            self.senders,
            self.character_cell_size
        )
        .with_context(err_context)?;
        self.park_pane(new_pane);
        Ok(())
    }

    pub fn new_floating_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<()> {
        // A modal is an overlay because of what it is, not because of the coordinates whichever
        // caller happened to pass: `ScreenInstruction::AddPlugin` only synthesizes overlay
        // coordinates when nobody passed any, and a keybinding written before `overlay`/`modal`
        // existed passes none of it at all. Asking the plugin's own configuration catches both.
        //
        // Both spellings are still *inputs* - what they no longer do is decide where the pane is
        // stored. An overlay goes to `overlay_panes` and never touches the floating layer.
        if let Some(placement) = overlay_placement_for(&floating_pane_coordinates, &invoked_with) {
            return self.new_overlay_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                placement,
                blocking_notification,
            );
        }
        if should_focus_pane {
            self.show_floating_panes();
        }
        self.close_down_to_max_terminals()?;
        let new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if start_suppressed {
            // this pane needs to start in the background (suppressed), only accessible if a plugin
            // takes it out of there in one way or another
            self.park_new_pane(new_pane)
        } else {
            self.add_floating_pane(
                new_pane,
                pid,
                floating_pane_coordinates,
                should_focus_pane,
                None,
            )
        }
    }

    /// Create a pane directly onto the session surface as an overlay.
    ///
    /// Nothing raises the floating layer here, which is the whole point: an overlay that is never
    /// on that layer has no reveal to undo. The bookkeeping this used to be contrasted with -
    /// `layer_visible_when_added`, `hide_floating_panes_under_overlay` - is gone with the
    /// two-stage birth it compensated for.
    #[allow(clippy::too_many_arguments)]
    pub fn new_overlay_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        placement: OverlayPlacement,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<()> {
        self.close_down_to_max_terminals()?;
        let new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;
        let awaiting_placement = run_awaits_placement(new_pane.invoked_with());
        self.add_overlay_pane(
            new_pane,
            pid,
            placement.kind,
            placement.coordinates.unwrap_or_default(),
            // Hidden overlays still accept geometry before reveal.
            OverlayCarry {
                hidden: start_suppressed,
                awaiting_placement,
                ..Default::default()
            },
            should_focus_pane,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_in_place_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        match (pane_id_to_replace, client_id) {
            (Some(pane_id_to_replace), _) => {
                self.suppress_pane_and_replace_with_pid(
                    pane_id_to_replace,
                    pid,
                    close_replaced_pane,
                    invoked_with,
                    blocking_notification,
                    borderless,
                )?;
            },
            (None, Some(client_id)) => match self.get_active_pane_id(client_id) {
                Some(active_pane_id) => {
                    self.suppress_pane_and_replace_with_pid(
                        active_pane_id,
                        pid,
                        close_replaced_pane,
                        invoked_with,
                        blocking_notification,
                        borderless,
                    )?;
                },
                None => {
                    log::error!("Cannot find active pane");
                },
            },
            _ => {
                log::error!("Must have pane id to replace or client id to start pane in place>");
            },
        }
        if let Some(initial_pane_title) = initial_pane_title {
            let _ = self.rename_pane(initial_pane_title.as_bytes().to_vec(), pid);
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new_stacked_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        pane_id_to_stack_under: Option<PaneId>,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        if should_focus_pane {
            self.hide_floating_panes();
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        let mut new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if let Some(borderless) = borderless {
            new_pane.set_borderless(borderless);
        }

        if start_suppressed {
            self.park_new_pane(new_pane).with_context(err_context)
        } else {
            if let Some(pane_id_to_stack_under) = pane_id_to_stack_under {
                self.add_stacked_pane_to_pane_id(
                    new_pane,
                    pid,
                    pane_id_to_stack_under,
                    should_focus_pane,
                )
            } else if let Some(client_id) = client_id {
                self.add_stacked_pane_to_active_pane(new_pane, pid, client_id, should_focus_pane)
            } else {
                log::error!("Must have client id or pane id to stack pane");
                Ok(())
            }
        }
    }
    pub fn replace_active_pane_with_editor_pane(
        &mut self,
        pid: PaneId,
        client_id: ClientId,
    ) -> Result<()> {
        // this method creates a new pane from pid and replaces it with the active pane
        // the active pane is then suppressed (hidden and not rendered) until the current
        // created pane is closed, in which case it will be replaced back by it
        let err_context = || format!("failed to suppress active pane for client {client_id}");

        match pid {
            PaneId::Terminal(pid) => {
                let new_pane = self.new_scrollback_editor_pane(pid);
                let replaced_pane = if self.floating_panes.panes_are_visible() {
                    self.floating_panes
                        .replace_active_pane(Box::new(new_pane), client_id)
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_active_pane(Box::new(new_pane), client_id)
                };
                match replaced_pane {
                    Some(replaced_pane) => {
                        self.track_stack_list_member_id_swap(
                            replaced_pane.pid(),
                            PaneId::Terminal(pid),
                        );
                        self.insert_scrollback_editor_replaced_pane(replaced_pane, pid);
                        self.get_active_pane(client_id)
                            .with_context(|| format!("no active pane found for client {client_id}"))
                            .and_then(|current_active_pane| {
                                resize_pty!(
                                    current_active_pane,
                                    self.os_api,
                                    self.senders,
                                    self.character_cell_size
                                )
                            })
                            .with_context(err_context)?;
                    },
                    None => {
                        Err::<(), _>(anyhow!(
                            "Could not find editor pane to replace - is no pane focused?"
                        ))
                        .with_context(err_context)
                        .non_fatal();
                    },
                }
            },
            PaneId::Plugin(_pid) => {
                // TBD, currently unsupported
            },
        }
        Ok(())
    }
    pub fn replace_pane_with_editor_pane(
        &mut self,
        pid: PaneId,
        pane_id_to_replace: PaneId,
    ) -> Result<()> {
        // this method creates a new pane from pid and replaces it with the pane iwth the given pane_id_to_replace
        // the pane with the given pane_id_to_replace is then suppressed (hidden and not rendered) until the current
        // created pane is closed, in which case it will be replaced back by it
        let err_context = || "failed to suppress pane".to_string();

        match pid {
            PaneId::Terminal(pid) => {
                let new_pane = self.new_scrollback_editor_pane(pid);
                let replaced_pane = if self.floating_panes.panes_contain(&pane_id_to_replace) {
                    self.floating_panes
                        .replace_pane(pane_id_to_replace, Box::new(new_pane))
                        .ok()
                } else if self.overlay_panes.panes_contain(&pane_id_to_replace) {
                    self.overlay_panes
                        .replace_pane(pane_id_to_replace, Box::new(new_pane))
                        .ok()
                } else if self.tiled_panes.panes_contain(&pane_id_to_replace) {
                    self.tiled_panes
                        .replace_pane(pane_id_to_replace, Box::new(new_pane))
                } else if self.suppressed_panes.contains_key(&pane_id_to_replace) {
                    log::error!("Cannot replace suppressed pane");
                    None
                } else {
                    // not a thing
                    None
                };
                match replaced_pane {
                    Some(replaced_pane) => {
                        resize_pty!(
                            replaced_pane,
                            self.os_api,
                            self.senders,
                            self.character_cell_size
                        )
                        .non_fatal();
                        self.track_stack_list_member_id_swap(
                            pane_id_to_replace,
                            PaneId::Terminal(pid),
                        );
                        self.insert_scrollback_editor_replaced_pane(replaced_pane, pid);
                    },
                    None => {
                        Err::<(), _>(anyhow!("Could not find editor pane to replace"))
                            .with_context(err_context)
                            .non_fatal();
                    },
                }
            },
            PaneId::Plugin(_pid) => {
                // TBD, currently unsupported
            },
        }
        Ok(())
    }
    pub fn suppress_pane_and_replace_with_pid(
        &mut self,
        old_pane_id: PaneId,
        new_pane_id: PaneId,
        close_replaced_pane: bool,
        run: Option<Run>,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        // this method creates a new pane from pid and replaces it with the active pane
        // the active pane is then suppressed (hidden and not rendered) until the current
        // created pane is closed, in which case it will be replaced back by it
        let err_context = || "failed to suppress active pane".to_string();

        match new_pane_id {
            PaneId::Terminal(new_pane_id) => {
                let next_terminal_position = self.get_next_terminal_position(); // TODO: this is not accurate in this case
                let mut new_pane = TerminalPane::new(
                    new_pane_id,
                    PaneGeom::default(), // the initial size will be set later
                    self.style,
                    next_terminal_position,
                    String::new(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    None,
                    run,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    completion_tx,
                );
                if let Some(supported) = self.kitty_host_support {
                    new_pane.update_kitty_host_support(supported);
                }
                if let Some(supported) = self.sixel_host_support {
                    new_pane.update_sixel_host_support(supported);
                }
                if let Some(borderless) = borderless {
                    new_pane.set_borderless(borderless);
                }
                let replaced_pane = if self.floating_panes.panes_contain(&old_pane_id) {
                    self.floating_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else if self.overlay_panes.panes_contain(&old_pane_id) {
                    self.overlay_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                };
                if replaced_pane.is_some() {
                    self.track_stack_list_member_id_swap(
                        old_pane_id,
                        PaneId::Terminal(new_pane_id),
                    );
                }
                if close_replaced_pane {
                    if let Some(pid) = replaced_pane.as_ref().map(|p| p.pid()) {
                        self.senders
                            .send_to_pty(PtyInstruction::ClosePane(pid, None))
                            .with_context(err_context)?;
                    }
                    drop(replaced_pane);
                } else {
                    match replaced_pane {
                        Some(replaced_pane) => {
                            let _ = resize_pty!(
                                replaced_pane,
                                self.os_api,
                                self.senders,
                                self.character_cell_size
                            );
                            let is_scrollback_editor = false;
                            self.suppress_pane_behind(
                                replaced_pane,
                                PaneId::Terminal(new_pane_id),
                                is_scrollback_editor,
                            );
                        },
                        None => {
                            Err::<(), _>(anyhow!(
                                "Could not find editor pane to replace - is no pane focused?"
                            ))
                            .with_context(err_context)
                            .non_fatal();
                        },
                    }
                }
            },
            PaneId::Plugin(plugin_pid) => {
                let mut new_pane = PluginPane::new(
                    plugin_pid,
                    PaneGeom::default(), // this will be filled out later
                    self.senders
                        .to_plugin
                        .as_ref()
                        .with_context(err_context)?
                        .clone(),
                    String::new(),
                    String::new(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.connected_clients_in_app
                        .borrow()
                        .iter()
                        .copied()
                        .collect(),
                    self.style,
                    run,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                );
                if let Some(borderless) = borderless {
                    new_pane.set_borderless(borderless);
                }
                let replaced_pane = if self.floating_panes.panes_contain(&old_pane_id) {
                    self.floating_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else if self.overlay_panes.panes_contain(&old_pane_id) {
                    self.overlay_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                };
                if replaced_pane.is_some() {
                    self.track_stack_list_member_id_swap(old_pane_id, PaneId::Plugin(plugin_pid));
                }
                if close_replaced_pane {
                    drop(replaced_pane);
                } else {
                    match replaced_pane {
                        Some(replaced_pane) => {
                            let _ = resize_pty!(
                                replaced_pane,
                                self.os_api,
                                self.senders,
                                self.character_cell_size
                            );
                            let is_scrollback_editor = false;
                            self.suppress_pane_behind(
                                replaced_pane,
                                PaneId::Plugin(plugin_pid),
                                is_scrollback_editor,
                            );
                        },
                        None => {
                            Err::<(), _>(anyhow!(
                                "Could not find editor pane to replace - is no pane focused?"
                            ))
                            .with_context(err_context)
                            .non_fatal();
                        },
                    }
                }
            },
        }
        Ok(())
    }
    pub fn close_pane_and_replace_with_other_pane(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_to_replace_with: Box<dyn Pane>,
        completion_tx: Option<NotificationEnd>,
    ) {
        let replacement_pane_id = pane_to_replace_with.pid();
        let mut replaced_pane = if self.floating_panes.panes_contain(&pane_id_to_replace) {
            self.floating_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else if self.overlay_panes.panes_contain(&pane_id_to_replace) {
            self.overlay_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else {
            self.tiled_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
        };
        if replaced_pane.is_some() {
            self.track_stack_list_member_id_swap(pane_id_to_replace, replacement_pane_id);
        }
        if let Some(replaced_pane) = replaced_pane.take() {
            let pane_id = replaced_pane.pid();
            let _ = self
                .senders
                .send_to_pty(PtyInstruction::ClosePane(pane_id, completion_tx));
            let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::PaneClosed(pane_id),
            )]));
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id });
            drop(replaced_pane);
        }
    }
    pub fn suppress_pane_and_replace_with_other_pane(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_to_replace_with: Box<dyn Pane>,
        _completion_tx: Option<NotificationEnd>,
    ) {
        let mut replaced_pane = if self.floating_panes.panes_contain(&pane_id_to_replace) {
            self.floating_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else if self.overlay_panes.panes_contain(&pane_id_to_replace) {
            self.overlay_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else {
            self.tiled_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
        };

        if let Some(replaced_pane) = replaced_pane.take() {
            // parked rather than suppressed behind its replacement: this one is not
            // restored by the replacement closing, it has to be asked for by name
            self.park_pane(replaced_pane);
        }
    }
    pub fn horizontal_split(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context =
            || format!("failed to split pane {pid:?} horizontally for client {client_id}");
        if self.floating_panes.panes_are_visible() {
            return Ok(());
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        if self.tiled_panes.fullscreen_is_active() {
            self.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if self.tiled_panes.can_split_pane_horizontally(client_id) {
            if let PaneId::Terminal(term_pid) = pid {
                let next_terminal_position = self.get_next_terminal_position();
                let mut new_terminal = TerminalPane::new(
                    term_pid,
                    PaneGeom::default(), // the initial size will be set later
                    self.style,
                    next_terminal_position,
                    String::new(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    initial_pane_title,
                    None,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    completion_tx,
                );
                if let Some(supported) = self.kitty_host_support {
                    new_terminal.update_kitty_host_support(supported);
                }
                if let Some(supported) = self.sixel_host_support {
                    new_terminal.update_sixel_host_support(supported);
                }
                if let Some(borderless) = borderless {
                    new_terminal.set_borderless(borderless);
                }
                self.tiled_panes
                    .split_pane_horizontally(pid, Box::new(new_terminal), client_id);
                self.set_should_clear_display_before_rendering();
                self.tiled_panes.focus_pane(pid, client_id);
                self.swap_layouts.set_is_tiled_damaged();
            }
        } else {
            log::error!("No room to split pane horizontally");
            if let Some(active_pane_id) = self.tiled_panes.get_active_pane_id(client_id) {
                self.senders
                    .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                        vec![active_pane_id],
                        "CAN'T SPLIT!".into(),
                    ))
                    .with_context(err_context)?;
            }
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, completion_tx))
                .with_context(err_context)?;
            return Ok(());
        }
        Ok(())
    }
    pub fn vertical_split(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context =
            || format!("failed to split pane {pid:?} vertically for client {client_id}");
        if self.floating_panes.panes_are_visible() {
            return Ok(());
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        if self.tiled_panes.fullscreen_is_active() {
            self.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if self.tiled_panes.can_split_pane_vertically(client_id) {
            if let PaneId::Terminal(term_pid) = pid {
                let next_terminal_position = self.get_next_terminal_position();
                let mut new_terminal = TerminalPane::new(
                    term_pid,
                    PaneGeom::default(), // the initial size will be set later
                    self.style,
                    next_terminal_position,
                    String::new(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    initial_pane_title,
                    None,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    completion_tx,
                );
                if let Some(supported) = self.kitty_host_support {
                    new_terminal.update_kitty_host_support(supported);
                }
                if let Some(supported) = self.sixel_host_support {
                    new_terminal.update_sixel_host_support(supported);
                }
                if let Some(borderless) = borderless {
                    new_terminal.set_borderless(borderless);
                }
                self.tiled_panes
                    .split_pane_vertically(pid, Box::new(new_terminal), client_id);
                self.set_should_clear_display_before_rendering();
                self.tiled_panes.focus_pane(pid, client_id);
                self.swap_layouts.set_is_tiled_damaged();
            }
        } else {
            log::error!("No room to split pane vertically");
            if let Some(active_pane_id) = self.tiled_panes.get_active_pane_id(client_id) {
                self.senders
                    .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                        vec![active_pane_id],
                        "CAN'T SPLIT!".into(),
                    ))
                    .with_context(err_context)?;
            }
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, completion_tx))
                .with_context(err_context)?;
            return Ok(());
        }
        Ok(())
    }

    pub fn horizontal_split_of_pane_id(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        target_pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        self.split_of_pane_id(
            pid,
            initial_pane_title,
            invoked_with,
            target_pane_id,
            SplitDirection::Horizontal,
            completion_tx,
            borderless,
        )
    }
    pub fn vertical_split_of_pane_id(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        target_pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        self.split_of_pane_id(
            pid,
            initial_pane_title,
            invoked_with,
            target_pane_id,
            SplitDirection::Vertical,
            completion_tx,
            borderless,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn split_of_pane_id(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        target_pane_id: PaneId,
        split_direction: SplitDirection,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context =
            || format!("failed to split pane {target_pane_id:?} without changing focus");
        if self.floating_panes.panes_are_visible() {
            return Ok(());
        }
        if !self.tiled_panes.panes_contain(&target_pane_id) {
            return Ok(());
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        let can_split = match split_direction {
            SplitDirection::Horizontal => self
                .tiled_panes
                .can_split_pane_id_horizontally(target_pane_id),
            SplitDirection::Vertical => self
                .tiled_panes
                .can_split_pane_id_vertically(target_pane_id),
        };
        if !can_split {
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, completion_tx))
                .with_context(err_context)?;
            return Ok(());
        }
        if let PaneId::Terminal(term_pid) = pid {
            let next_terminal_position = self.get_next_terminal_position();
            let mut new_terminal = TerminalPane::new(
                term_pid,
                PaneGeom::default(),
                self.style,
                next_terminal_position,
                String::new(),
                self.link_handler.clone(),
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
                completion_tx,
            );
            if let Some(supported) = self.kitty_host_support {
                new_terminal.update_kitty_host_support(supported);
            }
            if let Some(supported) = self.sixel_host_support {
                new_terminal.update_sixel_host_support(supported);
            }
            if let Some(borderless) = borderless {
                new_terminal.set_borderless(borderless);
            }
            match split_direction {
                SplitDirection::Horizontal => self.tiled_panes.split_pane_id_horizontally(
                    pid,
                    Box::new(new_terminal),
                    target_pane_id,
                ),
                SplitDirection::Vertical => self.tiled_panes.split_pane_id_vertically(
                    pid,
                    Box::new(new_terminal),
                    target_pane_id,
                ),
            }
            self.set_should_clear_display_before_rendering();
            self.swap_layouts.set_is_tiled_damaged();
        }
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
    /// Whether `pane_id`'s agent is still waiting on a person who has not answered yet -
    /// which is exactly the condition the blinking frame indicator draws.
    pub fn pane_wants_attention(&self, pane_id: PaneId) -> bool {
        self.rendered_pane(pane_id)
            .is_some_and(|pane| pane.agent_attention_since().is_some())
    }
    /// When `pane_id`'s unanswered wait started, for a test that needs to tell one blink from
    /// a restarted one.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pane_agent_attention_since(&self, pane_id: PaneId) -> Option<Instant> {
        self.rendered_pane(pane_id)
            .and_then(|pane| pane.agent_attention_since())
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
    pub fn handle_pty_bytes(&mut self, pid: TerminalId, bytes: VteBytes) -> Result<()> {
        if self.is_pending {
            self.pending_instructions
                .push(BufferedTabInstruction::HandlePtyBytes(pid, bytes));
            return Ok(());
        }
        let err_context = || format!("failed to handle pty bytes from fd {pid}");
        let pane_id = PaneId::Terminal(pid);
        // A scrolled pane buffers its vte events, so what the user is reading stays put
        let pane_is_scrolled = self
            .get_pane_with_id(pane_id)
            .is_some_and(|pane| pane.is_scrolled());
        if pane_is_scrolled {
            let pending = self.pending_vte_events.entry(pid).or_default();
            pending.push(bytes);
            // Reset scroll - and process all pending events for this pane
            if pending.len() >= MAX_PENDING_VTE_EVENTS {
                if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                    pane.clear_scroll();
                }
                self.process_pending_vte_events(pid)
                    .with_context(err_context)?;
            }
            return Ok(());
        }
        self.process_pty_bytes(pid, bytes).with_context(err_context)
    }
    pub fn remote_pane_ready(&mut self, pid: TerminalId) -> Result<()> {
        // Anything buffered for this pane is loading-screen progress the clear
        // below erases. A user who scrolled the loading pane left it marked
        // scrolled with those frames queued; replaying them later would paint the
        // loading screen over the live terminal, and leaving the scroll set would
        // keep the command's own output buffering behind it.
        self.pending_vte_events.remove(&pid);
        if let Some(pane) = self.get_pane_with_id_mut(PaneId::Terminal(pid)) {
            pane.clear_scroll();
        }
        self.pids_waiting_resize.insert(pid);
        self.process_pty_bytes(pid, b"\x1b[3J\x1b[2J\x1b[H".to_vec())
    }
    pub fn handle_plugin_bytes(
        &mut self,
        pid: PluginId,
        client_id: ClientId,
        bytes: VteBytes,
    ) -> Result<()> {
        if let Some(plugin_pane) = self.get_pane_with_id_mut(PaneId::Plugin(pid)) {
            plugin_pane.handle_plugin_bytes(client_id, bytes);
        }
        Ok(())
    }
    pub fn process_pending_vte_events(&mut self, pid: TerminalId) -> Result<()> {
        if let Some(pending_vte_events) = self.pending_vte_events.get_mut(&pid) {
            let vte_events: Vec<VteBytes> = std::mem::take(pending_vte_events);
            for vte_event in vte_events {
                self.process_pty_bytes(pid, vte_event)
                    .context("failed to process pending vte events")?;
            }
        }
        Ok(())
    }
    /// Deliver a forwarded host reply (or cache-fallback synthesis,
    /// or a locally-answered query payload) to a pane that is currently
    /// forward-paused. The reply bytes are written to the pane's PTY
    /// first (so the app reads them on its stdin in the same stream
    /// position the original query occupied), then the pause flag is
    /// cleared and any PTY bytes that arrived while the pane was
    /// paused are re-fed through vte. The re-feed itself may produce
    /// another forward and re-arm the pause; the cycle bounds at the
    /// buffered byte count.
    pub fn resume_pane_after_forward(
        &mut self,
        pid: TerminalId,
        reply_bytes: Vec<u8>,
    ) -> Result<()> {
        let err_context = || format!("failed to resume pane {pid} after forward");
        // Write the reply first so the app sees it on stdin before
        // any subsequent (sync or async) reply that follows in the
        // buffered byte stream.
        if !reply_bytes.is_empty() {
            self.write_to_pane_id_without_preprocessing(reply_bytes, PaneId::Terminal(pid))
                .with_context(err_context)?;
        }
        // Clear the pause and pull out any bytes that were buffered
        // while it waited for the host reply.
        let buffered =
            if let Some(terminal_output) = self.get_pane_with_id_mut(PaneId::Terminal(pid)) {
                terminal_output.clear_forward_pause();
                terminal_output.drain_pending_pty_input()
            } else {
                // Pane closed between forward dispatch and reply arrival.
                return Ok(());
            };
        if !buffered.is_empty() {
            self.process_pty_bytes(pid, buffered)
                .with_context(err_context)?;
        }
        Ok(())
    }
    fn process_pty_bytes(&mut self, pid: TerminalId, bytes: VteBytes) -> Result<()> {
        let err_context = || format!("failed to process pty bytes from pid {pid}");

        // Spelled out rather than `get_pane_with_id_mut`: the pane borrow has to live alongside
        // `self.senders` and `self.character_cell_size` for `resize_pty!` below
        if let Some(terminal_output) = self
            .tiled_panes
            .get_pane_mut(PaneId::Terminal(pid))
            .or_else(|| self.floating_panes.get_pane_mut(PaneId::Terminal(pid)))
            .or_else(|| self.overlay_panes.get_pane_mut(PaneId::Terminal(pid)))
            .or_else(|| {
                self.suppressed_panes
                    .get_mut(&PaneId::Terminal(pid))
                    .map(|s_p| &mut s_p.pane)
            })
        {
            if self.pids_waiting_resize.remove(&pid) {
                resize_pty!(
                    terminal_output,
                    self.os_api,
                    self.senders,
                    self.character_cell_size
                )
                .with_context(err_context)?;
            }
            terminal_output.handle_pty_bytes(bytes);
            let messages_to_pty = terminal_output.drain_messages_to_pty();
            let forwarded_queries = terminal_output.drain_forwarded_queries();
            // If at least one forward was produced, the pane stopped
            // processing its `pending_pty_input` queue at the byte
            // that produced the forward. Arm the pause so subsequent
            // PTY bytes accumulate in the same queue rather than
            // being processed; they will be drained in stream order
            // when the host reply (or cache-fallback synthesis, or
            // 500 ms timeout) lands via `resume_pane_after_forward`.
            // Pause arming is independent of which forward bears the
            // reply: resume is keyed by pane_id, not by token, so a
            // single flag suffices for any number of queued forwards.
            if !forwarded_queries.is_empty() {
                terminal_output.arm_forward_pause();
            }
            let nested_session_messages = terminal_output.drain_nested_session_messages();
            let clipboard_update = terminal_output.drain_clipboard_update();
            let desktop_notifications = terminal_output.drain_desktop_notifications();
            let osc7_cwd = terminal_output.drain_osc7_cwd();
            for message in messages_to_pty {
                self.write_to_pane_id_without_preprocessing(message, PaneId::Terminal(pid))
                    .with_context(err_context)?;
            }
            for query in forwarded_queries {
                let _ = self
                    .senders
                    .send_to_screen(ScreenInstruction::ForwardHostQuery {
                        pane_id: PaneId::Terminal(pid),
                        query,
                    });
            }
            for message in nested_session_messages {
                let _ =
                    self.senders
                        .send_to_screen(ScreenInstruction::NestedSessionMessageFromPane {
                            pane_id: PaneId::Terminal(pid),
                            message,
                        });
            }
            if let Some(string) = clipboard_update {
                self.write_selection_to_clipboard(&string)
                    .with_context(err_context)?;
            }
            if !desktop_notifications.is_empty() {
                // The screen knows which protocol each client's host speaks.
                self.senders
                    .send_to_screen(ScreenInstruction::ForwardDesktopNotifications {
                        pane_id: pid,
                        notifications: desktop_notifications,
                    })
                    .with_context(err_context)?;
            }
            if let Some(path) = osc7_cwd {
                let _ = self
                    .senders
                    .send_to_pty(PtyInstruction::NotifyCwdFromOsc7(pid, path));
            }
        }
        Ok(())
    }

    pub fn write_to_terminals_on_current_tab(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        client_id: ClientId,
    ) -> Result<bool> {
        // returns true if a UI update should be triggered (eg. when closing a command pane with
        // ctrl-c)
        let mut should_trigger_ui_change = false;
        let mut pane_ids = self.get_static_and_floating_pane_ids();
        pane_ids.extend(
            self.stack_list_of_member
                .keys()
                .filter(|member| self.pane_is_hidden_stack_list_member(member))
                .copied(),
        );
        for pane_id in pane_ids {
            let ui_change_triggered = self
                .write_to_pane_id(
                    key_with_modifier,
                    raw_input_bytes.clone(),
                    raw_input_bytes_are_kitty,
                    pane_id,
                    Some(client_id),
                    None,
                )
                .context("failed to write to terminals on current tab")?;
            if ui_change_triggered {
                should_trigger_ui_change = true;
            }
        }
        Ok(should_trigger_ui_change)
    }

    pub fn write_to_active_terminal(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        client_id: ClientId,
    ) -> Result<bool> {
        // returns true if a UI update should be triggered (eg. if a command pane
        // was closed with ctrl-c)
        let err_context = || {
            format!(
                "failed to write to active terminal for client {client_id} - msg: {raw_input_bytes:?}"
            )
        };

        self.clear_search(client_id);
        self.mouse_help_text_visible.clear();
        let pane_id = self
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("failed to find active pane id for client {client_id}"))
            .with_context(err_context)?;
        // Can't use 'err_context' here since it borrows 'raw_input_bytes'
        self.write_to_pane_id(
            key_with_modifier,
            raw_input_bytes,
            raw_input_bytes_are_kitty,
            pane_id,
            Some(client_id),
            None,
        )
        .with_context(|| format!("failed to write to active terminal for client {client_id}"))
    }

    pub fn paste_to_pane_id(
        &mut self,
        bytes: Vec<u8>,
        pane_id: PaneId,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let bracketed_paste_begin = vec![27, 91, 50, 48, 48, 126];
        let bracketed_paste_end = vec![27, 91, 50, 48, 49, 126];

        self.write_to_pane_id(&None, bracketed_paste_begin, false, pane_id, None, None)?;
        self.write_to_pane_id(&None, bytes, false, pane_id, None, None)?;
        self.write_to_pane_id(&None, bracketed_paste_end, false, pane_id, None, completion)?;
        Ok(())
    }

    pub fn paste_to_active_terminal(
        &mut self,
        bytes: Vec<u8>,
        client_id: ClientId,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to paste to active terminal for client {client_id}");
        let active_pane_id = self
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("no active pane for client {client_id}"))
            .with_context(err_context)?;
        self.paste_to_pane_id(bytes, active_pane_id, completion)
    }

    pub fn write_to_pane_id(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        pane_id: PaneId,
        client_id: Option<ClientId>,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<bool> {
        // returns true if we need to update the UI (eg. when a command pane is closed with ctrl-c)
        let err_context = || format!("failed to write to pane with id {pane_id:?}");

        let mut should_update_ui = false;
        let is_sync_panes_active = self.is_sync_panes_active();

        // Spelled out rather than `get_pane_with_id_mut`: the pane borrow spans the
        // `pending_agent_escapes` bookkeeping and the sends below
        let active_pane = self
            .floating_panes
            .get_mut(&pane_id)
            .or_else(|| self.overlay_panes.get_mut(&pane_id))
            .or_else(|| self.tiled_panes.get_pane_mut(pane_id))
            .or_else(|| self.suppressed_panes.get_mut(&pane_id).map(|p| &mut p.pane))
            .ok_or_else(|| anyhow!(format!("failed to find pane with id {pane_id:?}")))
            .with_context(err_context)?;

        // We always write for non-synced terminals.
        // However if the terminal is part of a tab-sync, we need to
        // check if the terminal should receive input or not (depending on its
        // 'exclude_from_sync' configuration).
        let should_not_write_to_terminal = is_sync_panes_active && active_pane.exclude_from_sync();

        if should_not_write_to_terminal {
            return Ok(should_update_ui);
        }

        if self.agent_double_escape && matches!(pane_id, PaneId::Terminal(_)) {
            if let (Some(client_id), Some(key)) = (client_id, key_with_modifier) {
                let target = (client_id, pane_id);
                if active_pane.agent_info().is_some()
                    && !active_pane.is_held()
                    && !active_pane.is_suspended()
                    && active_pane.guest_modal_selection(client_id).is_none()
                    && key.bare_key == zellij_types::keys::BareKey::Esc
                    && key.has_no_modifiers()
                {
                    let now = Instant::now();
                    if !self
                        .pending_agent_escapes
                        .remove(&target)
                        .is_some_and(|first| now.duration_since(first) < Duration::from_secs(1))
                    {
                        self.pending_agent_escapes.insert(target, now);
                        return Ok(true);
                    }
                    should_update_ui = true;
                } else {
                    should_update_ui |= self.pending_agent_escapes.remove(&target).is_some();
                }
            }
        }

        // Somebody is answering. Whatever the agent here was waiting for, it is no longer
        // waiting on being *noticed* - so the indicator comes down now rather than when the
        // agent gets round to reporting that it is busy again.
        active_pane.acknowledge_agent_attention();

        match pane_id {
            PaneId::Terminal(active_terminal_id) => {
                match active_pane.adjust_input_to_terminal(
                    key_with_modifier,
                    raw_input_bytes,
                    raw_input_bytes_are_kitty,
                    client_id,
                ) {
                    Some(AdjustedInput::WriteBytesToTerminal(adjusted_input)) => {
                        self.senders
                            .send_to_pty_writer(PtyWriteInstruction::Write(
                                adjusted_input,
                                active_terminal_id,
                                completion_tx,
                            ))
                            .with_context(err_context)?;
                    },
                    Some(AdjustedInput::ReRunCommandInThisPane) => {
                        // Through the screen, which re-runs it with
                        // `rerun_terminal_pane_with_id` and reads the agent record around the
                        // restart. The screen's channel is unbounded, so a tab may send to it
                        // from the screen thread.
                        self.senders
                            .send_to_screen(ScreenInstruction::RerunCommandPane(
                                active_terminal_id,
                                completion_tx,
                            ))
                            .with_context(err_context)?;
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::ResumeSuspendedProcessInThisPane) => {
                        self.senders
                            .send_to_pty(PtyInstruction::ResumeSuspendedPane(PaneId::Terminal(
                                active_terminal_id,
                            )))
                            .with_context(err_context)?;
                    },
                    Some(AdjustedInput::CloseThisPane) => {
                        self.senders
                            .send_to_screen(ScreenInstruction::CloseFocusWithPaneId(
                                PaneId::Terminal(active_terminal_id),
                                completion_tx,
                            ))
                            .with_context(err_context)?;
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::DropToShellInThisPane { working_dir }) => {
                        self.pids_waiting_resize.insert(active_terminal_id);
                        self.senders
                            .send_to_pty(PtyInstruction::DropToShellInPane {
                                pane_id: PaneId::Terminal(active_terminal_id),
                                shell: Some(self.default_shell.clone()),
                                working_dir,
                                completion_tx,
                            })
                            .with_context(err_context)?;
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::GuestModalSelectionChanged) => {
                        should_update_ui = true;
                    },
                    Some(AdjustedInput::GuestModalZoom) => {
                        if let Some(client_id) = client_id {
                            let _ =
                                self.senders
                                    .send_to_screen(ScreenInstruction::GuestModalChoice {
                                        client_id,
                                        pane_id: PaneId::Terminal(active_terminal_id),
                                        outcome: GuestModalOutcome::Zoom,
                                    });
                        }
                    },
                    Some(AdjustedInput::GuestModalDescend) => {
                        if let Some(client_id) = client_id {
                            let _ =
                                self.senders
                                    .send_to_screen(ScreenInstruction::GuestModalChoice {
                                        client_id,
                                        pane_id: PaneId::Terminal(active_terminal_id),
                                        outcome: GuestModalOutcome::Descend,
                                    });
                        }
                    },
                    Some(_) => {},
                    None => {},
                }
            },
            PaneId::Plugin(pid) => match active_pane.adjust_input_to_terminal(
                key_with_modifier,
                raw_input_bytes,
                raw_input_bytes_are_kitty,
                client_id,
            ) {
                Some(AdjustedInput::WriteKeyToPlugin(key_with_modifier)) => {
                    self.senders
                        .send_to_plugin(PluginInstruction::Update(vec![(
                            Some(pid),
                            client_id,
                            Event::Key(key_with_modifier),
                        )]))
                        .with_context(err_context)?;
                },
                Some(AdjustedInput::WriteBytesToTerminal(adjusted_input)) => {
                    let mut plugin_updates = vec![];
                    for key in zellij_runtime::keys::parse_keys(&adjusted_input) {
                        plugin_updates.push((Some(pid), client_id, Event::Key(key)));
                    }
                    self.senders
                        .send_to_plugin(PluginInstruction::Update(plugin_updates))
                        .with_context(err_context)?;
                },
                Some(AdjustedInput::PermissionRequestResult(permissions, status)) => {
                    if active_pane.query_should_be_suppressed() {
                        active_pane.set_should_be_suppressed(false);
                        if self.pane_is_overlay(PaneId::Plugin(pid)) {
                            self.hide_overlay_pane(PaneId::Plugin(pid));
                        } else {
                            self.suppress_pane(PaneId::Plugin(pid), client_id);
                        }
                    }
                    self.request_plugin_permissions(pid, None);
                    self.senders
                        .send_to_plugin(PluginInstruction::PermissionRequestResult(
                            pid,
                            client_id,
                            permissions,
                            status,
                            None,
                        ))
                        .with_context(err_context)?;
                    should_update_ui = true;
                },
                Some(_) => {},
                None => {},
            },
        }
        Ok(should_update_ui)
    }
    pub fn write_to_pane_id_without_preprocessing(
        &mut self,
        raw_input_bytes: Vec<u8>,
        pane_id: PaneId,
    ) -> Result<bool> {
        // returns true if we need to update the UI (eg. when a command pane is closed with ctrl-c)
        let err_context = || format!("failed to write to pane with id {pane_id:?}");

        let mut should_update_ui = false;

        match pane_id {
            PaneId::Terminal(active_terminal_id) => {
                self.senders
                    .send_to_pty_writer(PtyWriteInstruction::Write(
                        raw_input_bytes,
                        active_terminal_id,
                        None,
                    ))
                    .with_context(err_context)?;
                should_update_ui = true;
            },
            PaneId::Plugin(_pid) => {
                log::error!("Unsupported plugin action");
            },
        }
        Ok(should_update_ui)
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
                let x = active_terminal.x() + x_in_terminal;
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
    pub fn set_force_render(&mut self) {
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
        self.tiled_panes.force_render_panes_intersecting(damage);
        self.floating_panes.force_render_panes_intersecting(damage);
        self.overlay_panes.force_render_panes_intersecting(damage);
    }
    pub fn pane_is_overlay(&self, pane_id: PaneId) -> bool {
        self.overlay_panes.panes_contain(&pane_id)
    }
    pub fn linked_overlay_partner(&self, pane_id: PaneId) -> Option<PaneId> {
        self.overlay_panes.linked_partner(&pane_id)
    }
    /// Set an overlay aside without unloading it - the overlay half of `suppress_pane`, which
    /// cannot be used here because it extracts the pane from its collection and an overlay put
    /// back afterwards would have lost its kind and its coordinates.
    pub fn hide_overlay_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.overlay_panes.hide_pane(pane_id) {
            return false;
        }
        // An overlay is drawn after the tab, so the cells it vacates have no other
        // writer: without dirtying everything underneath, the overlay stays on
        // screen as an unrepainted ghost of itself.
        self.set_should_clear_display_before_rendering();
        self.set_force_render();
        true
    }
    /// Draw an overlay again. Does not focus it - see `OverlayPanes::show_pane`.
    pub fn show_overlay_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.overlay_panes.show_pane(pane_id) {
            return false;
        }
        self.set_force_render();
        true
    }
    /// Whether an overlay is being painted at `position`, whatever it is covering.
    ///
    /// The mouse pipeline needs this as a fact about the *position* rather than about a pane,
    /// because the floating collection's own hit tests re-resolve a position from scratch and
    /// have no way to know an overlay was drawn over the answer they find.
    pub fn position_is_on_an_overlay(&self, position: &Position) -> bool {
        self.overlay_panes.visible_pane_id_at(position).is_some()
    }
    /// Whether this tab is painting a floating pane outside its own viewport - onto the tab bar or
    /// the status bar. The one thing a tab draws that the tab cannot clean up after by repainting
    /// itself.
    pub fn has_floating_panes_outside_viewport(&self) -> bool {
        self.floating_panes.has_panes_outside_viewport()
            || (self.overlays_are_rendered() && self.overlay_panes.has_panes_outside_viewport())
    }
    /// Everything this tab draws over its own tiled panes, bottom-most first: the floating layer,
    /// then the overlays on top of it.
    ///
    /// One flat list, because that is what `Output` clips against - a pane's z-index is its
    /// absolute position here and `visible_character_chunks` does `layers.iter().skip(z_index)`.
    /// The concatenation order is what puts an overlay above every floating pane, pinned ones
    /// included; while overlays lived among the floating panes a pinned pane outranked them,
    /// because `make_sure_pinned_panes_are_on_top` had already lifted it.
    ///
    /// `render` must gate on exactly the same conditions this does, or a pane is handed an index
    /// that is not its own, is clipped against its own geometry, and erases itself.
    fn occlusion_stack(&self) -> Option<FloatingPanesStack> {
        let mut layers: Vec<PaneGeom> = self
            .floating_panes
            .stack()
            .map(|stack| stack.layers)
            .unwrap_or_default();
        if self.overlays_are_rendered() {
            layers.extend(self.overlay_panes.layers());
        }
        if layers.is_empty() {
            None
        } else {
            Some(FloatingPanesStack { layers })
        }
    }
    /// How many layers the floating panes occupy in `occlusion_stack`, which is the offset every
    /// overlay's z-index has to be shifted by.
    fn floating_layer_count(&self) -> usize {
        self.floating_panes
            .stack()
            .map(|stack| stack.layers.len())
            .unwrap_or(0)
    }
    /// How this pane is placed on the surface, if it is an overlay. Moving a pane between tabs has
    /// to carry this along: the tab it leaves forgets it, and without it the pane lands in the new
    /// tab sized against that tab's viewport rather than against the session surface.
    ///
    /// The whole placement rather than just the coordinates, so that a modal arrives as a modal -
    /// the kind used to be re-derived from the plugin's configuration on the far side, which is
    /// why a modal *terminal* pane could not survive the trip.
    pub fn overlay_placement_for(&self, pane_id: PaneId) -> Option<OverlayPlacement> {
        self.overlay_panes.placement_of(&pane_id)
    }
    /// The other half of what a mover has to read before `extract_pane` - see [`OverlayCarry`].
    /// Split from `overlay_placement_for` because that one crosses the protobuf boundary and this
    /// one is in-memory state; a caller taking an overlay out of this tab wants both.
    pub fn sample_overlay_ripples(&mut self, now: std::time::Instant, enabled: bool) {
        self.overlay_panes.sample_opening_ripples(now, enabled);
    }

    pub fn settle_overlay_ripples(&mut self) {
        self.overlay_panes.settle_opening_ripples();
    }

    pub fn overlay_ripple_is_live(&self) -> bool {
        self.overlay_panes.opening_ripple_is_live()
    }

    pub fn overlay_carry_for(&self, pane_id: PaneId) -> OverlayCarry {
        self.overlay_panes.carry_of(&pane_id)
    }
    /// Where `pane_id` belongs, for a caller about to take it out of this tab and hand it to
    /// another one. The overlay arm has to come first and has to be asked *here*: `extract_pane`
    /// drops the kind and coordinates on the way out, and the destination has no way to
    /// re-derive them - a modal *terminal* pane has no plugin configuration to read them back
    /// out of.
    pub fn placement_for_extraction(&self, pane_id: PaneId) -> ExtractedPanePlacement {
        if let Some(placement) = self.overlay_placement_for(pane_id) {
            ExtractedPanePlacement::Overlay(placement, self.overlay_carry_for(pane_id))
        } else if self.pane_id_is_floating(&pane_id) {
            ExtractedPanePlacement::Floating
        } else {
            ExtractedPanePlacement::Tiled
        }
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

    /// Draw this tab: its tiled panes and chrome, then the floating layer and the overlays above
    /// it, then the cursor.
    ///
    /// The cursor goes last so that it is placed once everything contributing to the frame is in
    /// the output, rather than wherever the last character chunk happened to land. A caller that
    /// paints *above* the tabs - the session's toasts do - must therefore not use this: it has to
    /// call `render_panes` for every tab, paint its own layer, and only then place the cursor with
    /// `render_cursor_last`. See `Screen::render_to_clients`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn render(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        self.render_panes(output, client_id_override)?;
        self.render_cursor_last(output, client_id_override);
        Ok(())
    }

    pub fn expire_pending_agent_escapes(&mut self, now: Instant) {
        self.pending_agent_escapes
            .retain(|_, first| now.duration_since(*first) < Duration::from_secs(1));
    }

    /// Everything `render` draws except the cursor.
    pub fn render_panes(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        self.expire_pending_agent_escapes(Instant::now());
        for pane_id in self.get_all_pane_ids() {
            let clients = self
                .pending_agent_escapes
                .keys()
                .filter_map(|(client, pane)| (*pane == pane_id).then_some(*client))
                .collect();
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.set_pending_agent_escape_clients(clients);
            }
        }
        self.render_tiled_panes_and_chrome(output, client_id_override)?;
        self.render_floating_panes_and_overlays(output, client_id_override)?;
        Ok(())
    }

    fn render_tiled_panes_and_chrome(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        let err_context = || "failed to render tab".to_string();

        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };

        // If we have a client_id_override (for watcher rendering), add it temporarily
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }

        if connected_clients.is_empty() {
            return Ok(());
        }
        let floating_panes_stack = self.occlusion_stack();
        output.add_clients(
            &connected_clients,
            self.link_handler.clone(),
            floating_panes_stack,
        );
        // Floating-only tabs can still have tiled chrome, even without tiled focus.
        let has_active_tiled_panes = self.tiled_panes.has_active_panes();
        if has_active_tiled_panes && self.stacked_pane_list_is_active() {
            self.groupify_all();
        }
        self.update_active_panes_in_pty_thread()
            .with_context(err_context)?;

        let current_pane_group: HashMap<ClientId, Vec<PaneId>> =
            { self.current_pane_group.borrow().clone_inner() };
        if self.tiled_panes.has_panes() {
            self.tiled_panes
                .render(
                    output,
                    self.floating_panes.panes_are_visible(),
                    &self.mouse_hover_pane_id,
                    current_pane_group,
                    client_id_override,
                    &self.mouse_help_text_visible,
                    self.mouse_scroll_resize,
                )
                .with_context(err_context)?;
            self.render_stack_list_headers(output, client_id_override, false)
                .with_context(err_context)?;
        }
        // the floating panes are *not* rendered here - they draw over the tiled ones, see
        // `render_floating_panes_and_overlays`

        if output.has_rendered_assets() || self.should_clear_display_before_rendering {
            self.hide_cursor_and_clear_display_as_needed(output);
        }

        self.emit_hint_text_if_changed().with_context(err_context)?;
        self.emit_active_pane_scroll_if_changed()
            .with_context(err_context)?;

        Ok(())
    }

    /// Draw this tab's floating panes and then its overlays.
    ///
    /// The overlays go last, and with a z-index offset of the floating layer count, because
    /// `occlusion_stack` concatenates the two in that order and a pane's z-index is its absolute
    /// position in that one flat list.
    fn render_floating_panes_and_overlays(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        if self.has_floating_panes_to_render() {
            let resize: Vec<_> = self
                .stack_lists
                .iter()
                .filter_map(|(id, list)| self.floating_panes.get_pane(list.visible).map(|_| *id))
                .collect();
            for id in resize {
                self.resize_stack_list_hidden_members(id);
            }
            self.floating_panes.render(
                output,
                &self.mouse_hover_pane_id,
                self.current_pane_group.borrow().clone_inner(),
                client_id_override,
                &self.mouse_help_text_visible,
                self.mouse_scroll_resize,
            )?;
        }
        if self.has_floating_panes_to_render() {
            self.render_stack_list_headers(output, client_id_override, true)?;
        }
        if self.overlays_are_rendered() {
            let z_index_offset = self.floating_layer_count();
            self.overlay_panes.render(
                output,
                &self.mouse_hover_pane_id,
                self.current_pane_group.borrow().clone_inner(),
                client_id_override,
                &self.mouse_help_text_visible,
                self.mouse_scroll_resize,
                z_index_offset,
            )?;
        }
        Ok(())
    }

    /// The condition `render` used to apply before drawing the floating panes inline.
    pub fn has_floating_panes_to_render(&self) -> bool {
        !self.tiled_panes.fullscreen_covers_ui()
            && ((self.floating_panes.panes_are_visible() && self.floating_panes.has_active_panes())
                || self.floating_panes.has_pinned_panes())
    }

    /// Whether the overlays contribute to this frame at all.
    ///
    /// An overlay is drawn for as long as it exists and has not been set aside by `hide_pane`.
    /// There is still no *layer* to raise and no layer visibility to check, and - since Phase 4 -
    /// not even a fullscreen tiled pane to yield to: a UI-covering
    /// fullscreen used to hide overlays, which left a modal focused and invisible, a state the
    /// user has no way out of because the modal is the thing that has the keyboard. An overlay
    /// outranks fullscreen for the same reason it outranks a pinned pane.
    ///
    /// `occlusion_stack` and `render_floating_panes_and_overlays` both gate on this, and must keep
    /// doing so together.
    pub fn overlays_are_rendered(&self) -> bool {
        self.overlay_panes.has_panes()
    }

    /// Position this tab's cursor, once everything that contributes to the frame - every pane, and
    /// any layer painted above them - is in the output.
    pub fn render_cursor_last(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) {
        let clients_in_this_frame = self.clients_in_frame(client_id_override);
        if clients_in_this_frame.is_empty() {
            return;
        }
        // each tab's render pointed the output at its own stack, so point it back at ours
        output.set_floating_panes_stack(self.occlusion_stack());
        if output.has_rendered_assets() {
            output.hide_cursor_for_clients(clients_in_this_frame.iter().copied());
        }
        self.render_cursor(output, client_id_override);
    }

    /// Whose cursor this frame is placing.
    ///
    /// An override names one client and means it: the frame is the watchers', built from that
    /// client's panes and going nowhere near anybody else's terminal. Placing every connected
    /// client's cursor into it would write memo entries for bytes those clients never receive -
    /// and the followed client is not necessarily still connected to this tab, since Screen keeps
    /// following a departed client while watchers remain.
    fn clients_in_frame(&self, client_id_override: Option<ClientId>) -> Vec<ClientId> {
        match client_id_override {
            Some(client_id) => vec![client_id],
            None => self.connected_clients.borrow().iter().copied().collect(),
        }
    }
    fn hide_cursor_and_clear_display_as_needed(&mut self, output: &mut Output) {
        let connected_clients: Vec<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        output.hide_cursor_for_clients(connected_clients.iter().copied());
        if self.should_clear_display_before_rendering {
            let clear_display = "\u{1b}[m\u{1b}[2J";
            output.add_pre_vte_instruction_to_multiple_clients(
                connected_clients.iter().copied(),
                clear_display,
            );
            output.mark_host_display_cleared_for_clients(connected_clients.iter().copied());
            self.should_clear_display_before_rendering = false;
        }
    }
    fn render_cursor(&mut self, output: &mut Output, client_id_override: Option<ClientId>) {
        for client_id in self.clients_in_frame(client_id_override) {
            let memo_key = match client_id_override {
                Some(_) => CursorMemoKey::Watchers(client_id),
                None => CursorMemoKey::Client(client_id),
            };
            // Whatever this frame already painted moved the host's cursor away from where we last
            // put it, so it has to be told again - but only about the parts that actually changed.
            // Read before anything is emitted, so this pass does not see its own instructions.
            let frame_moved_cursor = output.frame_moved_cursor_for(client_id);
            let already_hidden_this_frame = output.cursor_was_hidden_for(client_id);
            match self.get_active_terminal_cursor_position(client_id) {
                Some((cursor_position_x, cursor_position_y, is_cursor_visible)) => {
                    // the same flat list `occlusion_stack` builds, so an overlay's index has to
                    // be offset past the floating layers exactly as it is when it renders
                    let active_pane_z_index =
                        self.get_active_pane_id(client_id).and_then(|pane_id| {
                            self.floating_panes.get_pane_z_index(pane_id).or_else(|| {
                                self.overlay_panes
                                    .get_pane_z_index(pane_id)
                                    .filter(|_| self.overlays_are_rendered())
                                    .map(|index| index + self.floating_layer_count())
                            })
                        });
                    output.set_cursor_coordinates(client_id, cursor_position_x, cursor_position_y);
                    let not_occluded = output.cursor_is_visible(
                        cursor_position_x,
                        cursor_position_y,
                        active_pane_z_index,
                        client_id,
                    );
                    let active_terminal_is_mid_frame = self
                        .active_terminal_is_mid_frame(client_id)
                        .unwrap_or(false);

                    if active_terminal_is_mid_frame {
                        // The active terminal is part-way through drawing a frame, so its cursor
                        // is jumping around and must not be shown. On a host that presents frames
                        // atomically nothing hid it earlier, so say so here. The next frame
                        // must restore its position and shape from scratch.
                        self.hide_host_cursor(output, client_id, memo_key);
                    } else if not_occluded && is_cursor_visible {
                        let desired_cursor_shape = self
                            .get_active_pane(client_id)
                            .map(|ap| ap.cursor_shape_csi())
                            .unwrap_or_default();
                        let last = self.host_cursor_state.get(&memo_key);
                        let position_changed = last
                            .map(|last| {
                                last.position != Some((cursor_position_x, cursor_position_y))
                            })
                            .unwrap_or(true);
                        // Re-sending the shape restarts the host's cursor blink phase, so this is
                        // the one piece that must never be emitted "just in case": on an animated
                        // frame cadence it is what the flicker actually is.
                        let shape_changed = last
                            .map(|last| last.shape.as_ref() != Some(&desired_cursor_shape))
                            .unwrap_or(true);
                        let must_show = already_hidden_this_frame
                            || last.map(|last| !last.visible).unwrap_or(true);
                        let must_goto = frame_moved_cursor || position_changed;

                        if must_show {
                            output.add_post_vte_instruction_to_client(client_id, "\u{1b}[?25h");
                        }
                        if must_goto {
                            let goto_cursor_position = &format!(
                                "\u{1b}[{};{}H\u{1b}[m",
                                cursor_position_y + 1,
                                cursor_position_x + 1,
                            ); // goto row/col
                            output.add_post_vte_instruction_to_client(
                                client_id,
                                goto_cursor_position,
                            );
                        }
                        if shape_changed {
                            output.add_post_vte_instruction_to_client(
                                client_id,
                                &desired_cursor_shape,
                            );
                        }
                        self.host_cursor_state.insert(
                            memo_key,
                            HostCursorState {
                                position: Some((cursor_position_x, cursor_position_y)),
                                shape: Some(desired_cursor_shape),
                                visible: true,
                            },
                        );
                    } else if not_occluded {
                        // Cursor is hidden by the app but not occluded by a floating
                        // pane. Position the host terminal cursor at the correct
                        // location (for IME) then hide it. The IME subsystem reads
                        // the cursor position regardless of visibility.
                        let last = self.host_cursor_state.get(&memo_key);
                        let position_changed = last
                            .map(|last| {
                                last.position != Some((cursor_position_x, cursor_position_y))
                            })
                            .unwrap_or(true);
                        let previous_shape = last.and_then(|last| last.shape.clone());
                        self.hide_host_cursor(output, client_id, memo_key);
                        if frame_moved_cursor || position_changed {
                            let goto_cursor_position = &format!(
                                "\u{1b}[{};{}H",
                                cursor_position_y + 1,
                                cursor_position_x + 1,
                            );
                            output.add_post_vte_instruction_to_client(
                                client_id,
                                goto_cursor_position,
                            );
                        }
                        self.host_cursor_state.insert(
                            memo_key,
                            HostCursorState {
                                position: Some((cursor_position_x, cursor_position_y)),
                                // the host keeps its shape across a hide, so remembering it means
                                // the frame that shows the cursor again does not have to re-send it
                                shape: previous_shape,
                                visible: false,
                            },
                        );
                    } else {
                        self.hide_host_cursor(output, client_id, memo_key);
                    }
                },
                None => {
                    self.hide_host_cursor(output, client_id, memo_key);
                },
            }
        }
    }

    /// Keep the cursor hidden and invalidate its position and shape for restoration.
    fn hide_host_cursor(
        &mut self,
        output: &mut Output,
        client_id: ClientId,
        memo_key: CursorMemoKey,
    ) {
        if !output.cursor_was_hidden_for(client_id)
            && !self
                .host_cursor_state
                .get(&memo_key)
                .is_some_and(|last| !last.visible)
        {
            output.add_post_vte_instruction_to_client(client_id, "\u{1b}[?25l");
        }
        self.host_cursor_state.insert(
            memo_key,
            HostCursorState {
                position: None,
                shape: None,
                visible: false,
            },
        );
    }

    /// Forget what the host was last told about this client's cursor, in both streams, so the next
    /// frame states it from scratch. For when the id itself stops meaning what it meant - a client
    /// leaving or arriving, given that ids are recycled.
    pub fn forget_host_cursor_state(&mut self, client_id: ClientId) {
        self.host_cursor_state
            .remove(&CursorMemoKey::Client(client_id));
        self.host_cursor_state
            .remove(&CursorMemoKey::Watchers(client_id));
    }

    /// The same for the watchers' stream alone: a watcher about to be sent a full repaint has not
    /// necessarily seen this screen before, and the regular clients' memo is still good.
    pub fn forget_watcher_host_cursor_state(&mut self, followed_client_id: ClientId) {
        self.host_cursor_state
            .remove(&CursorMemoKey::Watchers(followed_client_id));
    }

    /// Serialization hid the watchers' cursor because it fell outside a watcher's size, which this
    /// memo cannot see - so an entry claiming the cursor is visible at some position is wrong in
    /// both halves. The host's cursor is down, and the goto that "placed" it was clamped to that
    /// watcher's edge.
    ///
    /// Unlike `forget_watcher_host_cursor_state`, the shape is kept: a hide does not lose it, and
    /// re-sending the shape CSI restarts the host's blink phase (see `render_cursor`).
    pub fn mark_watcher_host_cursor_cropped(&mut self, followed_client_id: ClientId) {
        if let Some(host_cursor_state) = self
            .host_cursor_state
            .get_mut(&CursorMemoKey::Watchers(followed_client_id))
        {
            host_cursor_state.visible = false;
            host_cursor_state.position = None;
        }
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

    pub fn warp_relationships(&self, slots: &[PaneId]) -> zellij_types::data::WarpRelationships {
        use zellij_types::data::{WarpHiddenPane, WarpRelationships, WarpStackList};
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
        relationships: &zellij_types::data::WarpRelationships,
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
                    restored_by: hidden.restored_by.map(|cover| StandIn {
                        pane_id: slots[cover as usize],
                        is_scrollback_editor: hidden.is_scrollback_editor,
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
    pub fn has_selectable_panes(&self) -> bool {
        let selectable_tiled_panes = self.tiled_panes.get_panes().filter(|(_, p)| p.selectable());
        let selectable_floating_panes = self
            .floating_panes
            .get_panes()
            .filter(|(_, p)| p.selectable());
        selectable_tiled_panes.count() > 0 || selectable_floating_panes.count() > 0
    }
    /// Retain hidden live work as well as selectable panes. Screen settlement handles the
    /// separate case where no tab provides a usable destination.
    pub fn should_retain_tab(&self) -> bool {
        self.has_selectable_panes() || self.has_live_terminal()
    }
    /// Whether a terminal in this tab - shown or hidden - is still running. A hidden one (parked
    /// by a plugin, say) does not make the tab usable, but it does mean the session has something
    /// in it worth keeping: `Screen::settle_tabs` moves it to a fresh tab rather than ending the
    /// session with it.
    pub fn has_live_terminal(&self) -> bool {
        self.get_all_pane_ids().iter().any(|id| {
            matches!(id, PaneId::Terminal(_))
                && self
                    .get_pane_with_id(*id)
                    .is_some_and(|pane| !pane.exited())
        })
    }

    pub fn has_hidden_live_terminal(&self) -> bool {
        self.get_suppressed_panes()
            .any(|(id, pane)| matches!(id, PaneId::Terminal(_)) && !pane.pane.exited())
            || self.get_all_pane_ids().into_iter().any(|id| {
                matches!(id, PaneId::Terminal(_))
                    && self
                        .get_pane_with_id(id)
                        .is_some_and(|pane| !pane.selectable() && !pane.exited())
            })
    }

    pub fn set_pane_subtitle(&mut self, pane_id: PaneId, subtitle: Option<String>) -> Result<bool> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let subtitle = subtitle
                .map(|text| zellij_types::text::clean_string_from_control_and_linebreak(&text))
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
        self.set_should_clear_display_before_rendering();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .with_context(|| "failed to update plugins with mode info".to_string())?;
        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            &mut self.tiled_panes,
            self.pane_frame_style,
        );
        if let Some(pane_id) = fullscreen_pane_to_restore {
            if self.tiled_panes.panes_contain(&pane_id) {
                if fullscreen_covered_ui {
                    self.tiled_panes.toggle_pane_no_ui_fullscreen(pane_id);
                } else {
                    self.tiled_panes.toggle_pane_fullscreen(pane_id);
                }
            }
        }
        if let Some(pane_id) = floating_fullscreen_pane_to_restore {
            if self.floating_panes.panes_contain(&pane_id) {
                if floating_fullscreen_covered_ui {
                    self.floating_panes.toggle_pane_no_ui_fullscreen(pane_id);
                } else {
                    self.floating_panes.toggle_pane_fullscreen(pane_id);
                }
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
        self.dissolve_stack_lists_for_classic_mutation();
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
                Err(err) => match err.downcast_ref::<ZellijError>() {
                    Some(ZellijError::CantResizeFixedPanes { pane_ids }) => {
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
        } else if self.suppressed_panes.contains_key(&pane_id) {
            log::error!("Cannot resize suppressed panes");
        }
        Ok(())
    }
    pub fn resize(&mut self, client_id: ClientId, strategy: ResizeStrategy) -> Result<()> {
        self.resize_pane_in_layer(PaneSubject::Active(client_id), strategy)
    }
    fn set_pane_active_at(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_active_at(Instant::now());
        }
    }
    /// Whether a lower layer may move this client's focus at all.
    ///
    /// `false` while a modal holds the keyboard: `get_active_pane_id` prefers the overlay over
    /// every other layer, so a move made anyway would be a move nothing could observe - input
    /// would go on reaching the modal, and the focus underneath would have been rearranged where
    /// the user cannot see it.
    ///
    /// This is the *moving* half of the rule. Opening something with focus is the other half, and
    /// answers differently: see `add_floating_pane` and `new_pane`, where a modal keeps the
    /// keyboard and the new pane is still created behind it.
    pub fn lower_layer_may_take_focus(&self, client_id: ClientId) -> bool {
        !self.overlay_panes.a_modal_holds_focus(client_id)
    }
    /// A lower layer has just taken this client's focus, so a dismissible overlay gives the
    /// keyboard up: it learns it is no longer focused by being told, and that is how a
    /// dismiss-on-defocus popup closes itself.
    ///
    /// Called *after* the move rather than before it, and only when the move happened. A
    /// focus-move that found nothing in the direction asked for has changed nothing, and
    /// dismissing a popup on it would make a dead keypress close the thing the user was reading.
    pub fn release_overlay_focus_to_lower_layer(&mut self, client_id: ClientId) {
        self.overlay_panes.defocus_pane_if_dismissible(client_id);
    }
    pub fn focus_next_pane(&mut self, client_id: ClientId) {
        if !self.has_selectable_panes() {
            return;
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        let before = self.tiled_panes.get_active_pane_id(client_id);
        if self.tiled_panes.fullscreen_is_active() {
            self.switch_next_pane_fullscreen(client_id);
        } else {
            self.tiled_panes.focus_next_pane(client_id);
        }
        // release only after a move that happened: a cycle that lands where it started has
        // changed nothing (see release_overlay_focus_to_lower_layer)
        if self.tiled_panes.get_active_pane_id(client_id) != before {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
    }
    pub fn focus_previous_pane(&mut self, client_id: ClientId) {
        if !self.has_selectable_panes() {
            return;
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        let before = self.tiled_panes.get_active_pane_id(client_id);
        if self.tiled_panes.fullscreen_is_active() {
            self.switch_prev_pane_fullscreen(client_id);
        } else {
            self.tiled_panes.focus_previous_pane(client_id);
        }
        // release only after a move that happened: a cycle that lands where it started has
        // changed nothing (see release_overlay_focus_to_lower_layer)
        if self.tiled_panes.get_active_pane_id(client_id) != before {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
    }
    pub fn focus_last_pane(&mut self, client_id: ClientId) {
        if !self.has_selectable_panes() {
            return;
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        // release only after a move that happened: with no last pane recorded nothing moves,
        // and dismissing a popup on it would let a dead keypress close it
        if self.floating_panes.panes_are_visible() {
            let before = self.floating_panes.active_pane_id(client_id);
            self.floating_panes.focus_last_pane(client_id);
            if self.floating_panes.active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        } else {
            let before = self.tiled_panes.get_active_pane_id(client_id);
            if self.tiled_panes.fullscreen_is_active() {
                self.switch_last_pane_fullscreen(client_id);
            } else {
                self.tiled_panes.focus_last_pane(client_id);
            }
            if self.tiled_panes.get_active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        }
    }
    pub fn focus_pane_on_edge(&mut self, direction: Direction, client_id: ClientId) {
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        // release only after a move that happened: an edge with nothing on it moves nothing,
        // and dismissing a popup on it would let a dead keypress close it
        if self.floating_panes.panes_are_visible() {
            let before = self.floating_panes.active_pane_id(client_id);
            self.floating_panes.focus_pane_on_edge(direction, client_id);
            if self.floating_panes.active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        } else if self.has_selectable_panes() && !self.tiled_panes.fullscreen_is_active() {
            let before = self.tiled_panes.get_active_pane_id(client_id);
            self.tiled_panes.focus_pane_on_edge(direction, client_id);
            if self.tiled_panes.get_active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        }
    }
    pub fn focus_pane_adjacent_to(
        &mut self,
        pane_id: PaneId,
        direction: Direction,
        client_id: ClientId,
    ) -> Option<PaneId> {
        if !self.lower_layer_may_take_focus(client_id) {
            return None;
        }
        let focused = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .focus_pane_adjacent_to(pane_id, direction, client_id)
        } else if self.tiled_panes.fullscreen_is_active() {
            None
        } else {
            self.tiled_panes
                .focus_pane_adjacent_to(pane_id, direction, client_id)
        };
        if focused.is_some() {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        focused
    }
    // returns a boolean that indicates whether the focus moved
    pub fn move_focus_left(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus left for client {}", client_id);

        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Left,
                )
                .with_context(err_context)
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_left_fullscreen(client_id))
        } else {
            Ok(self.tiled_panes.move_focus_left(client_id))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    pub fn move_focus_down(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus down for client {}", client_id);

        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        if self.floating_panes.panes_are_visible()
            && self.move_focus_within_stack_list(client_id, true)
        {
            self.release_overlay_focus_to_lower_layer(client_id);
            return Ok(true);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Down,
                )
                .with_context(err_context)
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_down_fullscreen(client_id))
        } else if self.stacked_pane_list_is_active()
            && self.move_focus_within_stack_list(client_id, true)
        {
            Ok(true)
        } else {
            Ok(self.tiled_panes.move_focus_down(client_id))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    pub fn move_focus_up(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus up for client {}", client_id);

        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        if self.floating_panes.panes_are_visible()
            && self.move_focus_within_stack_list(client_id, false)
        {
            self.release_overlay_focus_to_lower_layer(client_id);
            return Ok(true);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Up,
                )
                .with_context(err_context)
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_up_fullscreen(client_id))
        } else if self.stacked_pane_list_is_active()
            && self.move_focus_within_stack_list(client_id, false)
        {
            Ok(true)
        } else {
            Ok(self.tiled_panes.move_focus_up(client_id))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    // returns a boolean that indicates whether the focus moved
    pub fn move_focus_right(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus right for client {}", client_id);

        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Right,
                )
                .with_context(err_context)
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_right_fullscreen(client_id))
        } else {
            Ok(self.tiled_panes.move_focus_right(client_id))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    /// Move a pane within its own layer. One dispatch for the thirteen public entry points
    /// below, so that they cannot disagree about the rules:
    ///
    /// - a floating move always marks the floating layout damaged and forces a render, so the
    ///   panes under the moved one repaint and nothing of it is left behind;
    /// - a tiled move is refused while nothing is selectable or a tiled pane is fullscreen, and
    ///   dissolves the stack lists first, being a classic mutation of the grid.
    ///
    /// A pane named by id moves in the layer it is in; a client's active pane moves in the
    /// floating layer while that layer is shown, and in the tiled one otherwise. A pane in
    /// neither layer - an overlay, or a pane suppressed behind another - is left alone.
    fn move_pane_in_layer(&mut self, subject: PaneSubject, movement: PaneMove) {
        let on_floating_layer = match subject {
            PaneSubject::Active(_) => self.floating_panes.panes_are_visible(),
            PaneSubject::Pane(pane_id) => self.floating_panes.panes_contain(&pane_id),
        };
        if on_floating_layer {
            let pane_id = match subject {
                PaneSubject::Active(client_id) => self.floating_panes.get_active_pane_id(client_id),
                PaneSubject::Pane(pane_id) => Some(pane_id),
            };
            if let Some(pane_id) = pane_id {
                match movement {
                    PaneMove::Next { backwards } => {
                        self.floating_panes.move_pane(backwards, pane_id)
                    },
                    PaneMove::Toward(Direction::Down) => {
                        self.floating_panes.move_pane_down(pane_id)
                    },
                    PaneMove::Toward(Direction::Up) => self.floating_panes.move_pane_up(pane_id),
                    PaneMove::Toward(Direction::Left) => {
                        self.floating_panes.move_pane_left(pane_id)
                    },
                    PaneMove::Toward(Direction::Right) => {
                        self.floating_panes.move_pane_right(pane_id)
                    },
                }
            }
            self.swap_layouts.set_is_floating_damaged();
            self.set_force_render();
        } else {
            if !self.has_selectable_panes() || self.tiled_panes.fullscreen_is_active() {
                return;
            }
            self.dissolve_stack_lists_for_classic_mutation();
            // resolved after the dissolve, which can change which pane stands for a stack list
            let pane_id = match subject {
                PaneSubject::Active(client_id) => self.tiled_panes.get_active_pane_id(client_id),
                PaneSubject::Pane(pane_id) => Some(pane_id),
            };
            let Some(pane_id) = pane_id else {
                return;
            };
            if !self.tiled_panes.panes_contain(&pane_id) {
                // an overlay or a suppressed pane: in neither layer this dispatch moves, so
                // there is nothing to do
                return;
            }
            match movement {
                PaneMove::Next { backwards } => self.tiled_panes.move_pane(backwards, pane_id),
                PaneMove::Toward(Direction::Down) => self.tiled_panes.move_pane_down(pane_id),
                PaneMove::Toward(Direction::Up) => self.tiled_panes.move_pane_up(pane_id),
                PaneMove::Toward(Direction::Left) => self.tiled_panes.move_pane_left(pane_id),
                PaneMove::Toward(Direction::Right) => self.tiled_panes.move_pane_right(pane_id),
            }
        }
    }
    pub fn move_active_pane(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Next { backwards: false },
        );
    }
    pub fn move_pane(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Next { backwards: false },
        );
    }
    pub fn move_active_pane_backwards(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Next { backwards: true },
        );
    }
    pub fn move_active_pane_down(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Down),
        );
    }
    pub fn move_pane_down(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Toward(Direction::Down),
        );
    }
    pub fn move_active_pane_up(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Up),
        );
    }
    pub fn move_pane_up(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(PaneSubject::Pane(pane_id), PaneMove::Toward(Direction::Up));
    }
    pub fn move_active_pane_right(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Right),
        );
    }
    pub fn move_pane_right(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Toward(Direction::Right),
        );
    }
    pub fn move_active_pane_left(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Left),
        );
    }
    pub fn move_pane_left(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Toward(Direction::Left),
        );
    }
    fn close_down_to_max_terminals(&mut self) -> Result<()> {
        if let Some(max_panes) = self.max_panes {
            self.dissolve_stack_lists_for_classic_mutation();
            let terminals = self.get_tiled_pane_ids();
            for &pid in terminals.iter().skip(max_panes - 1) {
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
    /// will act on when it comes back - a hidden hocus learns which tab to reappear on from the
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
    pub fn close_pane(
        &mut self,
        id: PaneId,
        ignore_suppressed_panes: bool,
        exit_status: Option<i32>,
    ) {
        self.pending_command_respawns.remove(&id);
        let id_stands_in_for_a_pane = self.stands_in_for_a_pane(id).is_some();
        if !ignore_suppressed_panes
            && !id_stands_in_for_a_pane
            && self.pane_is_stack_list_member(&id)
            && self.close_stack_list_member(id, exit_status)
        {
            self.announce_pane_closed(id);
            self.repair_focus_after_pane_removal();
            return;
        }
        // we need to ignore suppressed panes when we toggle a pane to be floating/embedded(tiled)
        // this is because in that case, while we do use this logic, we're not actually closing the
        // pane, we're moving it
        if !ignore_suppressed_panes && id_stands_in_for_a_pane {
            match self.replace_pane_with_suppressed_pane(id) {
                // The stand-in has left the layout even though the tab did not shrink, so it is
                // as closed as any other pane and the same things have to hear about it.
                // `close_pane_and_replace_with_other_pane` has always said so for its own swap.
                Ok(Some(_pane)) => {
                    self.announce_pane_closed(id);
                    self.repair_focus_after_pane_removal();
                },
                Ok(None) => {},
                Err(e) => {
                    Err::<(), _>(e)
                        .with_context(|| format!("failed to close pane {:?}", id))
                        .non_fatal();
                },
            }
            return;
        }
        // A suppressed intermediate owns no layout slot. Its predecessors inherit the
        // complete surviving restoration link, or stay parked if it had no successor.
        if !ignore_suppressed_panes {
            if let Some(mut removed) = self.suppressed_panes.remove(&id) {
                let successor = removed
                    .restored_by
                    .filter(|stand_in| self.has_pane_with_pid(&stand_in.pane_id));
                for predecessor in self.suppressed_panes.values_mut() {
                    if predecessor
                        .restored_by
                        .is_some_and(|stand_in| stand_in.pane_id == id)
                    {
                        predecessor.restored_by = successor;
                    }
                }
                if let Some(exit_status) = exit_status {
                    removed.pane.update_exit_status(exit_status);
                }
                self.announce_pane_closed(id);
                return;
            }
        }
        let closed_pane = if self.overlay_panes.panes_contain(&id) {
            // nothing to unset, hide or relayout: an overlay is not on a layer and does not tile
            self.overlay_panes.move_clients_out_of_pane(id);
            let closed_pane = self.overlay_panes.remove_pane(id);
            self.set_force_render();
            self.overlay_panes.set_force_render();
            closed_pane
        } else if self.floating_panes.panes_contain(&id) {
            if self.floating_panes.fullscreen_pane_id() == Some(id) {
                self.floating_panes.unset_fullscreen();
            }
            let closed_pane = self.floating_panes.remove_pane(id);
            self.floating_panes.move_clients_out_of_pane(id);
            if !self.floating_panes.has_selectable_panes() {
                self.swap_layouts.reset_floating_damage();
                self.hide_floating_panes();
            }
            self.set_force_render();
            self.floating_panes.set_force_render();
            if self.auto_layout
                && !self.swap_layouts.is_floating_damaged()
                && self.floating_panes.visible_panes_count() > 0
            {
                self.swap_layouts.set_is_floating_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_floating_panes(false);
            }
            closed_pane
        } else {
            if self.tiled_panes.fullscreen_is_active() {
                self.tiled_panes.unset_fullscreen();
            }
            let closed_pane = self.tiled_panes.remove_pane(id);
            // With only floating panes left, no tiled pane repaints the vacated area.
            if self.tiled_panes.first_selectable_pane_id().is_none() {
                self.set_should_clear_display_before_rendering();
            }
            self.set_force_render();
            self.tiled_panes.set_force_render();
            if self.auto_layout && !self.swap_layouts.is_tiled_damaged() {
                self.swap_layouts.set_is_tiled_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_tiled_panes(false);
            }
            self.resize_all_stack_list_hidden_members();
            closed_pane
        };
        if let Some(mut closed_pane) = closed_pane {
            if let Some(exit_status) = exit_status {
                // in case we need to update on Drop
                closed_pane.update_exit_status(exit_status);
            }
            if !ignore_suppressed_panes {
                self.announce_pane_closed(id);
            }
            self.repair_focus_after_pane_removal();
        }
    }
    pub fn extract_pane(
        &mut self,
        id: PaneId,
        dont_swap_if_suppressed: bool,
    ) -> Option<Box<dyn Pane>> {
        let id_stands_in_for_a_pane = self.stands_in_for_a_pane(id).is_some();
        if self.pane_is_stack_list_member(&id)
            && (dont_swap_if_suppressed || !id_stands_in_for_a_pane)
        {
            if let StackListRemoval::Removed(removed) = self.remove_stack_list_member(id) {
                return removed.map(|mut pane| {
                    pane.reset_logical_position();
                    pane
                });
            }
        }
        if !dont_swap_if_suppressed && id_stands_in_for_a_pane {
            // this is done for the scrollback editor
            return match self.replace_pane_with_suppressed_pane(id) {
                Ok(mut pane) => {
                    // we do this so that the logical index will not affect ordering in the target tab
                    if let Some(pane) = pane.as_mut() {
                        pane.reset_logical_position();
                    }
                    pane
                },
                Err(e) => {
                    Err::<(), _>(e)
                        .with_context(|| format!("failed to close pane {:?}", id))
                        .non_fatal();
                    None
                },
            };
        }
        if self.overlay_panes.panes_contain(&id) {
            // nothing to unset, hide or relayout: an overlay is not on a layer and does not tile
            self.overlay_panes.move_clients_out_of_pane(id);
            let mut closed_pane = self.overlay_panes.remove_pane(id);
            self.set_force_render();
            self.overlay_panes.set_force_render();
            // we do this so that the logical index will not affect ordering in the target tab
            if let Some(closed_pane) = closed_pane.as_mut() {
                closed_pane.reset_logical_position();
            }
            closed_pane
        } else if self.floating_panes.panes_contain(&id) {
            if self.floating_panes.fullscreen_pane_id() == Some(id) {
                self.floating_panes.unset_fullscreen();
            }
            let mut closed_pane = self.floating_panes.remove_pane(id);
            self.floating_panes.move_clients_out_of_pane(id);
            if !self.floating_panes.has_panes() {
                self.swap_layouts.reset_floating_damage();
                self.hide_floating_panes();
            }
            self.set_force_render();
            self.floating_panes.set_force_render();
            if self.auto_layout
                && !self.swap_layouts.is_floating_damaged()
                && self.floating_panes.visible_panes_count() > 0
            {
                self.swap_layouts.set_is_floating_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_floating_panes(false);
            }
            // we do this so that the logical index will not affect ordering in the target tab
            if let Some(closed_pane) = closed_pane.as_mut() {
                closed_pane.reset_logical_position();
            }
            closed_pane
        } else if self.tiled_panes.panes_contain(&id) {
            if self.tiled_panes.fullscreen_is_active() {
                self.tiled_panes.unset_fullscreen();
            }
            let mut closed_pane = self.tiled_panes.remove_pane(id);
            self.set_force_render();
            self.tiled_panes.set_force_render();
            if self.auto_layout && !self.swap_layouts.is_tiled_damaged() {
                self.swap_layouts.set_is_tiled_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_tiled_panes(false);
            }
            self.resize_all_stack_list_hidden_members();
            // we do this so that the logical index will not affect ordering in the target tab
            if let Some(closed_pane) = closed_pane.as_mut() {
                closed_pane.reset_logical_position();
            }
            closed_pane
        } else if let Some(suppressed) = self.suppressed_panes.remove(&id) {
            // TODO: test this (from the path in screen.rs focus_plugin_pane ~line 2519
            Some(suppressed.pane)
        } else {
            None
        }
    }
    /// Returns a replacement command that arrived while this one was still running, so the
    /// caller can start it only after firing this exit's hooks - starting it here, before the
    /// caller's hooks run, would fire them against the replacement instead of the pane that
    /// actually exited, and would skip `agent_start` since it wouldn't go through
    /// `Screen::run_command_in_pane_with_id`.
    pub fn hold_pane(
        &mut self,
        id: PaneId,
        exit_status: Option<i32>,
        is_first_run: bool,
        run_command: RunCommand,
    ) -> Option<(TerminalId, RunCommand, bool)> {
        if self.is_pending {
            self.pending_instructions
                .push(BufferedTabInstruction::HoldPane(
                    id,
                    exit_status,
                    is_first_run,
                    run_command,
                ));
            return None;
        }
        if self.floating_panes.panes_contain(&id) {
            self.floating_panes
                .hold_pane(id, exit_status, is_first_run, run_command);
        } else if let Some(pane) = self.overlay_panes.get_mut(&id) {
            pane.hold(exit_status, is_first_run, run_command);
        } else if self.tiled_panes.panes_contain(&id) {
            self.tiled_panes
                .hold_pane(id, exit_status, is_first_run, run_command);
        } else if let Some(pane) = self.suppressed_panes.get_mut(&id) {
            pane.pane.hold(exit_status, is_first_run, run_command);
        }
        if let PaneId::Terminal(terminal_pane_id) = id {
            self.pending_command_respawns
                .remove(&id)
                .map(|(run_command, keep_scrollback)| {
                    (terminal_pane_id, run_command, keep_scrollback)
                })
        } else {
            None
        }
    }
    /// Detach any respawn queued for `id` from this tab, so it can be carried to another tab
    /// the pane is about to move into. Only needed when a pane crosses tabs - a same-tab move
    /// (eg. a float/embed toggle) never extracts the pane, so the entry stays valid where it
    /// is.
    pub fn take_pending_respawn(&mut self, id: PaneId) -> Option<(RunCommand, bool)> {
        self.pending_command_respawns.remove(&id)
    }
    /// Re-home a respawn entry taken from another tab via `take_pending_respawn`, once the
    /// pane itself has landed here.
    pub fn restore_pending_respawn(&mut self, id: PaneId, entry: (RunCommand, bool)) {
        self.pending_command_respawns.insert(id, entry);
    }
    /// Mark a pane's root process stopped or running.
    ///
    /// Unlike `hold_pane` this is not buffered while the tab is pending: a pane
    /// that does not exist yet cannot have suspended, so there is nothing to
    /// replay.
    pub fn set_pane_suspended(&mut self, id: PaneId, suspended: bool) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(id) {
            pane.set_suspended(suspended);
        }
        self.emit_hint_text_if_changed()?;
        Ok(())
    }
    /// Swap the pane hidden behind `pane_id` back into the place `pane_id` was
    /// holding for it, and hand `pane_id` itself back.
    pub fn replace_pane_with_suppressed_pane(
        &mut self,
        pane_id: PaneId,
    ) -> Result<Option<Box<dyn Pane>>> {
        self.stands_in_for_a_pane(pane_id)
            .and_then(|hidden| self.suppressed_panes.remove(&hidden))
            .with_context(|| format!("no pane is suppressed behind pane {:?}", pane_id))
            .and_then(|suppressed| {
                let suppressed_pane = suppressed.pane;
                let suppressed_pane_id = suppressed_pane.pid();
                // Where the stand-in actually is, rather than whether floating
                // panes happen to be visible: the two differ whenever a tiled
                // stand-in is closed while a floating pane is open, and asking
                // the wrong collection to replace it fails *after* the hidden
                // pane has been taken out of the map - so it is dropped, and its
                // process left with nothing that could ever close it.
                let replaced_pane = if self.floating_panes.panes_contain(&pane_id) {
                    Some(self.floating_panes.replace_pane(pane_id, suppressed_pane)).transpose()?
                } else if self.overlay_panes.panes_contain(&pane_id) {
                    Some(self.overlay_panes.replace_pane(pane_id, suppressed_pane)).transpose()?
                } else {
                    self.tiled_panes.replace_pane(pane_id, suppressed_pane)
                };
                if replaced_pane.is_some() {
                    self.track_stack_list_member_id_swap(pane_id, suppressed_pane_id);
                }
                if let Some(suppressed_pane) = self
                    .floating_panes
                    .get_pane(suppressed_pane_id)
                    .or_else(|| self.overlay_panes.get_pane(suppressed_pane_id))
                    .or_else(|| self.tiled_panes.get_pane(suppressed_pane_id))
                {
                    // You may be thinking: why aren't we using the original "suppressed_pane" here,
                    // isn't it the same one?
                    //
                    // Yes, you are right! However, we moved it into its correct environment above
                    // (either floating_panes or tiled_panes) where it received a new geometry based on
                    // the pane there we replaced. Now, we need to update its pty about its new size.
                    // We couldn't do that before, and we can't use the original moved item now - so we
                    // need to refetch it
                    resize_pty!(
                        suppressed_pane,
                        self.os_api,
                        self.senders,
                        self.character_cell_size
                    )?;
                }
                Ok(replaced_pane)
            })
            .with_context(|| {
                format!(
                    "failed to replace active pane with suppressed pane {:?}",
                    pane_id
                )
            })
    }
    /// Tell the plugins and the render subscribers that `id` is gone.
    ///
    /// Every path that takes a pane out of the tab owes them this; the ones that swap a pane out
    /// rather than shrinking the layout are the ones that used to forget.
    fn announce_pane_closed(&self, id: PaneId) {
        let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
            None,
            None,
            Event::PaneClosed(id),
        )]));
        let _ = self
            .senders
            .send_to_screen(ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id: id });
    }
    /// Explicit close destroys the whole replacement chain. Natural exits and `close_self`
    /// still use `close_pane`, which restores the pane behind an editor or exit view.
    pub fn close_pane_explicitly(
        &mut self,
        id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Vec<PaneId> {
        if !self.has_pane_with_pid(&id) {
            return vec![];
        }
        let mut closed = vec![id];
        let mut index = 0;
        while index < closed.len() {
            let current = closed[index];
            for (subject, suppressed) in &self.suppressed_panes {
                if let Some(stand_in) = suppressed.restored_by {
                    let related = if *subject == current {
                        Some(stand_in.pane_id)
                    } else if stand_in.pane_id == current {
                        Some(*subject)
                    } else {
                        None
                    };
                    if let Some(related) = related {
                        if !closed.contains(&related) && self.has_pane_with_pid(&related) {
                            closed.push(related);
                        }
                    }
                }
            }
            index += 1;
        }
        // Break restoration links before removing anything, including a suppressed stand-in.
        for id in &closed {
            if let Some(pane) = self.suppressed_panes.get_mut(id) {
                pane.restored_by = None;
            }
        }
        for closing in closed.iter().rev() {
            self.close_pane(*closing, false, None);
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(*closing, None))
                .with_context(|| format!("failed to close pane {closing:?}"))
                .non_fatal();
        }
        drop(completion_tx);
        closed
    }
    pub fn close_focused_pane(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<Vec<PaneId>> {
        Ok(match self.get_active_pane_id(client_id) {
            Some(id) => self.close_pane_explicitly(id, completion_tx),
            None => vec![],
        })
    }
    pub fn clear_active_terminal_screen(&mut self, client_id: ClientId) -> Result<()> {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_screen();
        }
        Ok(())
    }
    pub fn clear_screen_for_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_screen();
        }
    }
    pub fn dump_active_terminal_screen(
        &mut self,
        file: Option<String>,
        client_id: ClientId,
        full: bool,
    ) -> Result<()> {
        let err_context =
            || format!("failed to dump active terminal screen for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let dump = active_pane.dump_screen(full, Some(client_id));
            self.os_api
                .write_to_file(dump, file)
                .with_context(err_context)?;
        }
        Ok(())
    }
    pub fn dump_with_ansi_active_terminal_screen(
        &mut self,
        file: Option<String>,
        client_id: ClientId,
        full: bool,
    ) -> Result<()> {
        let err_context =
            || format!("failed to dump active terminal screen for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let dump = active_pane.dump_screen_with_ansi(full, Some(client_id));
            self.os_api
                .write_to_file(dump, file)
                .with_context(err_context)?;
        }
        Ok(())
    }
    pub fn dump_terminal_screen(
        &mut self,
        file: Option<String>,
        pane_id: PaneId,
        full: bool,
    ) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id(pane_id) {
            let dump = pane.dump_screen(full, None);
            self.os_api.write_to_file(dump, file).non_fatal()
        }
        Ok(())
    }
    pub fn dump_with_ansi_terminal_screen(
        &mut self,
        file: Option<String>,
        pane_id: PaneId,
        full: bool,
    ) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id(pane_id) {
            let dump = pane.dump_screen_with_ansi(full, None);
            self.os_api.write_to_file(dump, file).non_fatal()
        }
        Ok(())
    }
    pub fn get_dump_active_terminal_screen(&mut self, client_id: ClientId, full: bool) -> String {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.dump_screen(full, Some(client_id))
        } else {
            String::new()
        }
    }
    pub fn get_dump_with_ansi_active_terminal_screen(
        &mut self,
        client_id: ClientId,
        full: bool,
    ) -> String {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.dump_screen_with_ansi(full, Some(client_id))
        } else {
            String::new()
        }
    }
    pub fn get_dump_terminal_screen(&mut self, pane_id: PaneId, full: bool) -> Option<String> {
        self.get_pane_with_id(pane_id)
            .map(|pane| pane.dump_screen(full, None))
    }
    pub fn get_dump_with_ansi_terminal_screen(
        &mut self,
        pane_id: PaneId,
        full: bool,
    ) -> Option<String> {
        self.get_pane_with_id(pane_id)
            .map(|pane| pane.dump_screen_with_ansi(full, None))
    }
    pub fn edit_scrollback(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to edit scrollback for client {client_id}");

        let mut file = temp_dir();
        file.push(format!("{}.dump", Uuid::new_v4()));
        self.dump_active_terminal_screen(
            Some(String::from(file.to_string_lossy())),
            client_id,
            true,
        )
        .with_context(err_context)?;
        let line_number = self
            .get_active_pane(client_id)
            .and_then(|a_t| a_t.get_line_number());
        self.senders
            .send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::ClientId(client_id),
                completion_tx,
            ))
            .with_context(err_context)
    }
    pub fn edit_scrollback_raw(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to edit scrollback for client {client_id}");

        let mut file = temp_dir();
        file.push(format!("{}.dump", Uuid::new_v4()));
        self.dump_with_ansi_active_terminal_screen(
            Some(String::from(file.to_string_lossy())),
            client_id,
            true,
        )
        .with_context(err_context)?;
        let line_number = self
            .get_active_pane(client_id)
            .and_then(|a_t| a_t.get_line_number());
        self.senders
            .send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::ClientId(client_id),
                completion_tx,
            ))
            .with_context(err_context)
    }
    pub fn edit_scrollback_for_pane_with_id(
        &mut self,
        pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        if let PaneId::Terminal(_terminal_pane_id) = pane_id {
            self.make_hidden_stack_list_member_visible(pane_id);
            let mut file = temp_dir();
            file.push(format!("{}.dump", Uuid::new_v4()));
            self.dump_terminal_screen(Some(String::from(file.to_string_lossy())), pane_id, true)
                .non_fatal();
            let line_number = self
                .get_pane_with_id(pane_id)
                .and_then(|a_t| a_t.get_line_number());
            self.senders.send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::PaneId(pane_id),
                completion_tx,
            ))
        } else {
            log::error!("Editing plugin pane scrollback is currently unsupported.");
            Ok(())
        }
    }
    pub fn edit_scrollback_raw_for_pane_with_id(
        &mut self,
        pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        if let PaneId::Terminal(_terminal_pane_id) = pane_id {
            self.make_hidden_stack_list_member_visible(pane_id);
            let mut file = temp_dir();
            file.push(format!("{}.dump", Uuid::new_v4()));
            self.dump_with_ansi_terminal_screen(
                Some(String::from(file.to_string_lossy())),
                pane_id,
                true,
            )
            .non_fatal();
            let line_number = self
                .get_pane_with_id(pane_id)
                .and_then(|a_t| a_t.get_line_number());
            self.senders.send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::PaneId(pane_id),
                completion_tx,
            ))
        } else {
            log::error!("Editing plugin pane scrollback is currently unsupported.");
            Ok(())
        }
    }
    pub fn scroll_active_terminal_up(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_up(1, client_id);
        }
    }

    pub fn scroll_terminal_up(&mut self, terminal_pane_id: TerminalId) -> Result<()> {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        let err_context = || format!("failed to scroll up pane {pane_id:?}");
        let fictitious_client_id = ClientId::new(1);
        let synthetic_position = Position::new(0, 0);
        let (mouse_sgr, is_alt) = match self.get_pane_with_id(pane_id) {
            Some(pane) => (
                pane.mouse_scroll_up(&synthetic_position),
                pane.is_alternate_mode_active(),
            ),
            None => return Ok(()),
        };
        if let Some(sgr) = mouse_sgr {
            self.write_to_pane_id(&None, sgr.into_bytes(), false, pane_id, None, None)
                .with_context(err_context)?;
        } else if is_alt {
            self.write_to_pane_id(&None, b"\x1b[A".to_vec(), false, pane_id, None, None)
                .with_context(err_context)?;
        } else if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.scroll_up(1, fictitious_client_id);
        }
        Ok(())
    }

    pub fn scroll_active_terminal_down(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to scroll down active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_down(1, client_id);
            if !active_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = active_pane.pid() {
                    self.process_pending_vte_events(raw_fd)
                        .with_context(err_context)?;
                }
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_down(&mut self, terminal_pane_id: TerminalId) -> Result<()> {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        let err_context = || format!("failed to scroll down pane {pane_id:?}");
        let fictitious_client_id = ClientId::new(1);
        let synthetic_position = Position::new(0, 0);
        let (mouse_sgr, is_alt) = match self.get_pane_with_id(pane_id) {
            Some(pane) => (
                pane.mouse_scroll_down(&synthetic_position),
                pane.is_alternate_mode_active(),
            ),
            None => return Ok(()),
        };
        if let Some(sgr) = mouse_sgr {
            self.write_to_pane_id(&None, sgr.into_bytes(), false, pane_id, None, None)
                .with_context(err_context)?;
        } else if is_alt {
            self.write_to_pane_id(&None, b"\x1b[B".to_vec(), false, pane_id, None, None)
                .with_context(err_context)?;
        } else {
            let needs_flush = if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.scroll_down(1, fictitious_client_id);
                !pane.is_scrolled()
            } else {
                false
            };
            if needs_flush {
                self.process_pending_vte_events(terminal_pane_id)
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    pub fn scroll_active_terminal_up_page(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            // prevent overflow when row == 0
            let scroll_rows = active_pane.rows().max(1).saturating_sub(1);
            active_pane.scroll_up(scroll_rows, client_id);
        }
    }

    pub fn scroll_terminal_page_up(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            let fictitious_client_id = ClientId::new(1); // this is not checked for terminal panes and we
                                                         // don't have an actual client id here
                                                         // TODO: traits were a mistake
                                                         // prevent overflow when row == 0
            let scroll_rows = terminal_pane.rows().max(1).saturating_sub(1);
            terminal_pane.scroll_up(scroll_rows, fictitious_client_id);
        }
    }

    pub fn scroll_active_terminal_down_page(&mut self, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to scroll down one page in active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let scroll_rows = active_pane.get_content_rows();
            active_pane.scroll_down(scroll_rows, client_id);
            if !active_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = active_pane.pid() {
                    self.process_pending_vte_events(raw_fd)
                        .with_context(err_context)?;
                }
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_page_down(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            let fictitious_client_id = ClientId::new(1); // this is not checked for terminal panes and we
                                                         // don't have an actual client id here
                                                         // TODO: traits were a mistake
            let scroll_rows = terminal_pane.get_content_rows();
            terminal_pane.scroll_down(scroll_rows, fictitious_client_id);
            if !terminal_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = terminal_pane.pid() {
                    self.process_pending_vte_events(raw_fd).non_fatal()
                }
            }
        }
    }

    pub fn scroll_active_terminal_up_half_page(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            // prevent overflow when row == 0
            let scroll_rows = (active_pane.rows().max(1).saturating_sub(1)) / 2;
            active_pane.scroll_up(scroll_rows, client_id);
        }
    }

    pub fn scroll_active_terminal_down_half_page(&mut self, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to scroll down half a page in active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let scroll_rows = (active_pane.rows().max(1) - 1) / 2;
            active_pane.scroll_down(scroll_rows, client_id);
            if !active_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = active_pane.pid() {
                    self.process_pending_vte_events(raw_fd)
                        .with_context(err_context)?;
                }
            }
        }
        Ok(())
    }

    pub fn scroll_active_terminal_to_bottom(&mut self, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to scroll to bottom in active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_scroll();
            if !active_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = active_pane.pid() {
                    self.process_pending_vte_events(raw_fd)
                        .with_context(err_context)?;
                }
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_to_bottom(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            terminal_pane.clear_scroll();
            if !terminal_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = terminal_pane.pid() {
                    self.process_pending_vte_events(raw_fd).non_fatal();
                }
            }
        }
    }

    pub fn scroll_active_terminal_to_previous_prompt(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_to_previous_prompt(client_id);
        }
    }

    pub fn scroll_active_terminal_to_next_prompt(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_to_next_prompt(client_id);
        }
    }

    pub fn select_command_at_scroll_position(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.select_command_at_scroll_position(client_id);
        }
    }

    pub fn start_text_selection(&mut self, client_id: ClientId) {
        let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) else {
            return;
        };
        let pane_id = active_pane.pid();
        active_pane.start_text_selection(client_id);
        self.keyboard_text_selection.insert(client_id, pane_id);
    }

    /// The pane a keyboard text selection is live on: the one it was started on, which is not
    /// necessarily the focused one any more.
    fn text_selection_pane_mut(&mut self, client_id: ClientId) -> Option<&mut Box<dyn Pane>> {
        match self.keyboard_text_selection.get(&client_id).copied() {
            Some(pane_id) => self.get_pane_with_id_mut(pane_id),
            None => self.get_active_pane_or_floating_pane_mut(client_id),
        }
    }

    pub fn move_text_selection(&mut self, direction: Direction, client_id: ClientId) {
        if let Some(pane) = self.text_selection_pane_mut(client_id) {
            pane.move_text_selection(direction, client_id);
        }
    }

    pub fn swap_text_selection_ends(&mut self, client_id: ClientId) {
        if let Some(pane) = self.text_selection_pane_mut(client_id) {
            pane.swap_text_selection_ends(client_id);
        }
    }

    pub fn remember_copied_text_selection(&mut self, client_id: ClientId) {
        if let Some(pane) = self.text_selection_pane_mut(client_id) {
            pane.remember_copied_text_selection(client_id);
        }
    }

    pub fn finish_text_selection(&mut self, client_id: ClientId) {
        let Some(pane_id) = self.keyboard_text_selection.remove(&client_id) else {
            return;
        };
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.finish_text_selection(client_id);
        }
    }

    /// Copies the last completed command's output and schedules the end of its flash.
    pub fn copy_last_command_output(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to copy last command output for client {client_id}");
        let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) else {
            return Ok(());
        };
        let pane_id = active_pane.pid();
        let Some(output) = active_pane.copy_last_command_output() else {
            return Ok(());
        };
        self.copy_text_to_clipboard(&output)
            .with_context(err_context)?;
        self.senders
            .send_to_background_jobs(BackgroundJob::ClearCommandOutputFlash { pane_id })
            .with_context(err_context)?;
        Ok(())
    }

    pub fn clear_command_output_flash(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_command_output_flash();
        }
    }

    pub fn scroll_active_terminal_to_top(&mut self, client_id: ClientId) -> Result<()> {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_scroll();
            if let Some(size) = active_pane.get_line_number() {
                active_pane.scroll_up(size, client_id);
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_to_top(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            terminal_pane.clear_scroll();
            if let Some(size) = terminal_pane.get_line_number() {
                let fictitious_client_id = ClientId::new(1); // this is not checked for terminal panes and we
                                                             // don't have an actual client id here
                                                             // TODO: traits were a mistake
                terminal_pane.scroll_up(size, fictitious_client_id);
            }
        }
    }

    pub fn clear_active_terminal_scroll(&mut self, client_id: ClientId) -> Result<bool> {
        // TODO: is this a thing?
        let err_context =
            || format!("failed to clear scroll in active pane for client {client_id}");

        let mut was_scrolled = false;
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            was_scrolled = active_pane.is_scrolled();
            active_pane.clear_scroll();
            if !active_pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = active_pane.pid() {
                    self.process_pending_vte_events(raw_fd)
                        .with_context(err_context)?;
                }
            }
        }
        Ok(was_scrolled)
    }

    pub fn handle_scrollwheel_up(
        &mut self,
        point: &Position,
        lines: usize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        MouseHandler::handle_scrollwheel_up(self, point, lines, client_id)
    }

    pub fn handle_scrollwheel_down(
        &mut self,
        point: &Position,
        lines: usize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        MouseHandler::handle_scrollwheel_down(self, point, lines, client_id)
    }

    fn get_pane_id_at(
        &mut self,
        point: &Position,
        search_selectable: bool,
    ) -> Result<Option<PaneId>> {
        let err_context = || format!("failed to get id of pane at position {point:?}");

        if self.tiled_panes.fullscreen_is_active()
            && (self.tiled_panes.fullscreen_covers_ui()
                || self
                    .is_position_inside_viewport(point)
                    .with_context(err_context)?)
        {
            return Ok(self.tiled_panes.fullscreen_pane_id());
        }

        let (stacked_pane_ids_under_flexible_pane, _stacked_pane_ids_over_flexible_pane) = {
            self.tiled_panes
                .stacked_pane_ids_under_and_over_flexible_panes()
                .with_context(err_context)?
        };
        let pane_contains_point = |p: &dyn Pane,
                                   point: &Position,
                                   stacked_pane_ids_under_flexible_pane: &HashSet<PaneId>|
         -> bool {
            let is_flexible_in_stack =
                p.current_geom().is_stacked() && !p.current_geom().rows.is_fixed();
            let is_stacked_under = stacked_pane_ids_under_flexible_pane.contains(&p.pid());
            let geom_to_compare_against =
                if is_stacked_under && !self.pane_frame_style.draws_full_frames() {
                    // these sort of panes are one-liner panes under a flexible pane in a stack when we
                    // don't draw pane frames - because the whole stack's content is offset to allow
                    // room for the boundary between panes, they are actually drawn 1 line above where
                    // they are
                    let mut geom = p.current_geom();
                    geom.y = geom.y.saturating_sub(p.get_content_offset().bottom);
                    geom
                } else if is_flexible_in_stack && !self.pane_frame_style.draws_full_frames() {
                    // these sorts of panes are flexible panes inside a stack when we don't draw pane
                    // frames - because the whole stack's content is offset to give room for the
                    // boundary between panes, we need to take this offset into account when figuring
                    // out whether the position is inside them
                    let mut geom = p.current_geom();
                    geom.rows.decrease_inner(p.get_content_offset().bottom);
                    geom
                } else {
                    p.current_geom()
                };
            geom_to_compare_against.contains(point)
        };

        let found_pane_id = if search_selectable {
            self.get_selectable_tiled_panes()
                .find(|(_, p)| {
                    pane_contains_point(p.as_ref(), point, &stacked_pane_ids_under_flexible_pane)
                })
                .map(|(&id, _)| id)
        } else {
            self.get_tiled_panes()
                .find(|(_, p)| {
                    pane_contains_point(p.as_ref(), point, &stacked_pane_ids_under_flexible_pane)
                })
                .map(|(&id, _)| id)
        };
        let resolved_pane_id = found_pane_id.map(|id| self.stack_list_member_at_point(id, point));
        if search_selectable {
            Ok(resolved_pane_id.filter(|id| {
                self.get_pane_with_id(*id)
                    .map(|p| p.selectable())
                    .unwrap_or(false)
            }))
        } else {
            Ok(resolved_pane_id)
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn handle_mouse_event(
        &mut self,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        MouseHandler::handle_mouse_event(self, event, client_id, None)
    }

    /// `passthrough_pane_id` is the pane the client is descended into, if any: alt+mouse over
    /// it is written to the pane when it tracks the mouse, instead of grouping host panes.
    pub fn handle_mouse_event_with_passthrough(
        &mut self,
        event: &MouseEvent,
        client_id: ClientId,
        passthrough_pane_id: Option<PaneId>,
    ) -> Result<MouseEffect> {
        MouseHandler::handle_mouse_event(self, event, client_id, passthrough_pane_id)
    }

    pub fn copy_selection(&self, client_id: ClientId) -> Result<()> {
        // a keyboard selection is copied from the pane it was started on, which is not
        // necessarily the focused one any more
        let selected_text = match self.keyboard_text_selection.get(&client_id).copied() {
            Some(pane_id) => self.get_pane_with_id(pane_id),
            None => self.get_active_pane(client_id),
        }
        .and_then(|p| p.get_selected_text(client_id));
        if let Some(selected_text) = selected_text {
            // `write_selection_to_clipboard` already tells the plugins how it went - and says
            // `SystemClipboardFailure` when it went badly, which a second unconditional
            // `CopyToClipboard` from here used to paper over.
            self.write_selection_to_clipboard(&selected_text)
                .with_context(|| {
                    format!("failed to write selection to clipboard for client {client_id}")
                })?;
        }
        Ok(())
    }
    pub fn copy_text_to_clipboard(&self, text: &str) -> Result<()> {
        self.write_selection_to_clipboard(text)
            .with_context(|| "failed to write text to clipboard".to_string())
    }

    pub fn write_selection_to_clipboard(&self, selection: &str) -> Result<()> {
        let err_context = || format!("failed to write selection to clipboard: '{}'", selection);

        let mut output = Output::default();
        let connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        output.add_clients(&connected_clients, self.link_handler.clone(), None);
        let client_ids = connected_clients.iter().copied();
        let clipboard_event =
            match self
                .clipboard_provider
                .set_content(selection, &mut output, client_ids)
            {
                Ok(_) => output
                    .serialize()
                    .and_then(|serialized_output| {
                        self.senders
                            .send_to_server(ServerInstruction::Render(Some(serialized_output)))
                    })
                    .map(|_| Event::CopyToClipboard(self.clipboard_provider.as_copy_destination()))
                    .with_context(err_context)?,
                Err(err) => {
                    Err::<(), _>(err).with_context(err_context).non_fatal();
                    Event::SystemClipboardFailure
                },
            };
        self.senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                clipboard_event,
            )]))
            .context("failed to notify plugins about new clipboard event")
            .non_fatal();

        Ok(())
    }
    /// Whether this tab has been told it is on screen. The other half of [`Tab::visible`]: a tab
    /// a client was moved into without this being set renders for nobody. Only the tests ask -
    /// production code sets it and reads it off `Screen::active_tab_ids` instead.
    #[cfg(any(test, feature = "test-support"))]
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
        // the same thing, so it is filtered here rather than extracted: hocus
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
            plugin_updates.push((Some(pid), None, Event::Visible(visible)));
        }
        self.senders
            .send_to_plugin(PluginInstruction::Update(plugin_updates))
            .with_context(|| format!("failed to set visibility of tab to {visible}"))?;
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
        Ok(())
    }

    /// Rename a pane, reporting whether the name actually moved.
    ///
    /// The `bool` is what lets the caller skip the repaint and the session-state
    /// report for a rename that changes nothing. A plugin that reasserts its own
    /// title every render — hocus did, ten times a second while streaming a
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
        Ok(true)
    }

    pub fn rename_active_pane(&mut self, buf: Vec<u8>, client_id: ClientId) -> Result<()> {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.rename(buf);
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

    /// Input that is not text is dropped, the same as a control sequence: a keystroke is never a
    /// reason to fail the screen thread, so this cannot error.
    pub fn update_search_term(&mut self, buf: Vec<u8>, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let Ok(s) = str::from_utf8(&buf) else {
                return;
            };
            // Accept text plus the existing reset, backspace and delete commands.
            let is_updatable = s
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\0' | '\u{8}' | '\u{7f}'));
            if is_updatable {
                active_pane.update_search_term(s);
            }
        }
    }

    pub fn search_down(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.search_down();
        }
    }

    pub fn search_up(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.search_up();
        }
    }

    pub fn toggle_search_case_sensitivity(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.toggle_search_case_sensitivity();
        }
    }

    pub fn toggle_search_wrap(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.toggle_search_wrap();
        }
    }

    pub fn toggle_search_whole_words(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.toggle_search_whole_words();
        }
    }

    pub fn clear_search(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_search();
        }
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
    /// Mark exactly the panes in `related` as kin of the focused one, and unmark every other
    /// pane in this tab. Answers whether anything changed, so the caller can skip a render.
    ///
    /// Assigning to *every* pane rather than only to the members is what clears the previous
    /// set: the sender says what is related now and is not asked to remember what it last
    /// said, so a plugin that reloads re-asserts by simply saying it again.
    pub fn set_related_panes(&mut self, related: &HashSet<PaneId>) -> bool {
        let mut changed = false;
        for pane_id in self.get_all_pane_ids() {
            let is_related = related.contains(&pane_id);
            if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                changed |= pane.set_related_highlight(is_related);
            }
        }
        changed
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
        if loading_indication.is_error() {
            // Failed initialization may never send geometry; show the diagnostic anyway.
            self.overlay_panes.finish_placement(PaneId::Plugin(pid));
        }
        if let Some(plugin_pane) = self.get_pane_with_id_mut(PaneId::Plugin(pid)) {
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
        if let Some(plugin_pane) = self.get_pane_with_id_mut(PaneId::Plugin(pid)) {
            plugin_pane.start_loading_indication(loading_indication);
        }
    }
    pub fn progress_plugin_loading_offset(&mut self, pid: PluginId) {
        if let Some(plugin_pane) = self.get_pane_with_id_mut(PaneId::Plugin(pid)) {
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
            self.notify_floating_plugins_of_visibility(false);
        }
    }

    /// Tell the plugins on the floating surface that the surface was raised or lowered. Only
    /// the panes in `floating_panes` are concerned: overlays are drawn on their own layer and
    /// suppressed panes are not on screen either way. Nothing is sent while the tab itself is
    /// hidden - `visible(true)` reports the state the surface is in when the tab returns.
    fn notify_floating_plugins_of_visibility(&self, visible: bool) {
        if !self.tab_visible {
            return;
        }
        let plugin_updates: Vec<_> = self
            .floating_panes
            .pane_ids()
            .filter_map(|p| match p {
                PaneId::Plugin(pid) => Some((Some(*pid), None, Event::Visible(visible))),
                _ => None,
            })
            .collect();
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
    }

    pub fn focus_pane_with_id(
        &mut self,
        pane_id: PaneId,
        should_float: bool,
        should_be_in_place: bool,
        client_id: ClientId,
    ) -> Result<()> {
        // TODO: should error if pane is not selectable
        //
        // A pane named by id is the pane the user just asked for, so this takes the *opening*
        // shape of the rule rather than the moving one that `lower_layer_may_take_focus` applies
        // to the focus-move family: a dismissible overlay gives the keyboard up so it can dismiss
        // itself, a modal keeps it, and either way the focus underneath still lands where it was
        // asked to - exactly as `add_floating_pane` does.
        //
        // Asked only for a pane that is not itself an overlay - the overlay arm of the chain below
        // is the one place this must not fire, or a request to focus one overlay would defocus
        // another on the way in - and only for a pane this tab actually holds, since a request
        // naming a pane that is not here focuses nothing and should dismiss nothing.
        if !self.overlay_panes.panes_contain(&pane_id) && self.get_pane_with_id(pane_id).is_some() {
            self.overlay_panes.defocus_pane_if_dismissible(client_id);
        }
        if self.focus_hidden_stack_list_member(pane_id, client_id) {
            return Ok(());
        }
        self.tiled_panes
            .focus_pane_if_exists(pane_id, client_id)
            .map(|_| self.hide_floating_panes())
            .or_else(|_| self.overlay_panes.focus_pane_if_exists(pane_id, client_id))
            .or_else(|_| {
                let focused_floating_pane =
                    self.floating_panes.focus_pane_if_exists(pane_id, client_id);
                if focused_floating_pane.is_ok() {
                    self.show_floating_panes();
                };
                focused_floating_pane
            })
            // TODO: change suppressed_panes to be a proper struct with methods that make sense rather
            // than doing this dance every time
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
                        } else if should_be_in_place {
                            let replaced_pane = if self.are_floating_panes_visible() {
                                self.floating_panes
                                    .replace_active_pane(pane, client_id)
                                    .ok()
                            } else {
                                self.tiled_panes.replace_active_pane(pane, client_id)
                            };
                            if let Some(replaced_pane) = replaced_pane {
                                let is_scrollback_editor = false;
                                self.track_stack_list_member_id_swap(replaced_pane.pid(), pane_id);
                                self.suppress_pane_behind(
                                    replaced_pane,
                                    pane_id,
                                    is_scrollback_editor,
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
    pub fn focus_suppressed_pane_for_all_clients(&mut self, pane_id: PaneId) {
        if self.make_hidden_stack_list_member_visible(pane_id) {
            self.tiled_panes.focus_pane_for_all_clients(pane_id);
            return;
        }
        match self.suppressed_panes.remove(&pane_id) {
            Some(suppressed) => {
                // as in `focus_pane_with_id`: a modal coming back out of the suppressed panes is
                // still a modal, and does not bring the floating layer up with it
                if !run_requests_overlay(suppressed.pane.invoked_with()) {
                    self.show_floating_panes();
                }
                // no focus call after this: `add_floating_pane` with no client focuses every
                // client itself, in whichever collection the pane actually landed in. Doing it
                // again here would name `floating_panes` unconditionally, and for an overlay that
                // is a collection the pane is not in - leaving a phantom in its `active_panes` and
                // `z_indices` that outlives the overlay.
                self.add_floating_pane(suppressed.pane, pane_id, None, true, None)
                    .non_fatal();
            },
            None => {
                log::error!("Could not find suppressed pane wiht id: {:?}", pane_id);
            },
        }
    }
    pub fn suppress_pane(&mut self, pane_id: PaneId, _client_id: Option<ClientId>) {
        // this method places a pane in the suppressed pane with its own ID - this means we'll
        // not take it out of there when another pane is closed (eg. like happens with the
        // scrollback editor), but it has to take itself out on its own (eg. a plugin using the
        // show_self() method)
        if let Some(pane) = self.extract_pane(pane_id, true) {
            self.park_pane(pane);
        }
    }
    fn make_hidden_stack_list_member_visible(&mut self, pane_id: PaneId) -> bool {
        self.swap_in_hidden_stack_list_member(pane_id, None)
    }
    /// Put a suppressed pane back in the layout, or leave it suppressed.
    ///
    /// The room check comes *before* the entry is taken out, for the reason
    /// `accept_extracted_pane` spells out: `add_tiled_pane` and
    /// `add_floating_pane` take the box by value and drop it when the tab has no
    /// room, reporting success. For a pane being created that is a failed
    /// insert; for a pane already in this tab it is destruction — the entry is
    /// gone, so nothing can ever close the pane or reap its process, and the
    /// only trace is a log line nobody reads.
    ///
    /// Staying suppressed is a state the pane is already in and can be asked out
    /// of again, so failing that way costs nothing.
    fn restore_suppressed_pane(&mut self, pane_id: PaneId, should_float_if_hidden: bool) -> bool {
        if !self.suppressed_panes.contains_key(&pane_id) {
            return false;
        }
        let has_room = if should_float_if_hidden {
            self.floating_panes.find_room_for_new_pane().is_some()
        } else {
            self.tiled_panes.has_room_for_new_pane()
        };
        if !has_room {
            log::error!(
                "No room to show pane {:?}, leaving it hidden rather than dropping it",
                pane_id
            );
            return true;
        }
        let Some(pane) = self
            .suppressed_panes
            .remove(&pane_id)
            .map(|suppressed| suppressed.pane)
        else {
            return false;
        };
        if should_float_if_hidden {
            self.add_floating_pane(pane, pane_id, None, true, None)
                .non_fatal();
        } else {
            self.add_tiled_pane(pane, pane_id, false, None).non_fatal();
        }
        true
    }
    pub fn unsuppress_pane(&mut self, pane_id: PaneId, should_float_if_hidden: bool) {
        // removes a pane from being suppressed (hidden) but does not focus it
        if self.make_hidden_stack_list_member_visible(pane_id) {
            return;
        }
        if !self.restore_suppressed_pane(pane_id, should_float_if_hidden) {
            log::error!("Could not find suppressed pane with id: {:?}", pane_id);
        }
    }
    pub fn unsuppress_or_expand_pane(&mut self, pane_id: PaneId, should_float_if_hidden: bool) {
        // removes a pane from being suppressed (hidden) but does not focus it
        if self.make_hidden_stack_list_member_visible(pane_id) {
            return;
        }
        if !self.restore_suppressed_pane(pane_id, should_float_if_hidden) {
            let expand_panes_success = !self.tiled_panes.expand_pane_in_stack(pane_id).is_empty();
            if !expand_panes_success {
                log::error!(
                    "Could not find suppressed or stacked pane with id: {:?}",
                    pane_id
                );
            }
        }
    }
    /// Take a pane out of the layout, leaving `stand_in` holding its place.
    ///
    /// Closing the stand-in is what brings it back (`replace_pane_with_suppressed_pane`).
    /// The stand-in may itself be suppressed later without any of this needing to be
    /// revisited: an entry names the pane standing in for it, and that stays true
    /// wherever either of them ends up.
    fn suppress_pane_behind(
        &mut self,
        pane: Box<dyn Pane>,
        stand_in: PaneId,
        is_scrollback_editor: bool,
    ) {
        self.suppressed_panes.insert(
            pane.pid(),
            SuppressedPane::behind(pane, stand_in, is_scrollback_editor),
        );
    }
    /// Take a pane out of the layout with nothing standing in for it.
    ///
    /// Nothing brings it back on its own - it has to be asked for by name (a plugin's
    /// `show_self`, selecting a stack-list member).
    fn park_pane(&mut self, pane: Box<dyn Pane>) {
        self.suppressed_panes
            .insert(pane.pid(), SuppressedPane::parked(pane));
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
    /// Place `pane` on the session surface as an overlay of `kind`.
    ///
    /// Unlike `add_floating_pane` this does no geometry: an overlay's geometry is entirely a
    /// function of its coordinates and the surface, and `OverlayPanes::add_pane` resolves it. It
    /// is also the reason there is no `find_room_for_new_pane` here - overlays do not tile
    /// themselves out of each other's way, they stack.
    ///
    /// `carry` is carried rather than derived: an overlay that follows a client to another tab is
    /// extracted as a bare `Box<dyn Pane>`, so everything the `OverlayPane` wrapper held - its
    /// kind, its coordinates, whether it was set aside and whether the user dragged it - has to be
    /// handed back in here, or the pane arrives on the new tab suddenly visible, or back at the
    /// position its plugin configured rather than the one the user dropped it at.
    /// `Tab::overlay_placement_for` and `Tab::overlay_carry_for` are what a mover reads before
    /// `extract_pane` drops the wrapper.
    pub fn add_overlay_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        pane_id: PaneId,
        kind: OverlayKind,
        coordinates: FloatingPaneCoordinates,
        carry: OverlayCarry,
        should_focus_new_pane: bool,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        let err_context = || "failed to add overlay pane".to_string();
        if let Some(pinned) = coordinates.pinned {
            // an overlay is above every floating pane already, so pinning it means nothing - but
            // honour the flag rather than silently dropping what the caller asked for
            pane.set_pinned(pinned);
        }
        pane.set_active_at(Instant::now());
        self.overlay_panes
            .add_pane(pane_id, pane, kind, coordinates, carry);
        resize_pty!(
            self.overlay_panes
                .get_mut(&pane_id)
                .with_context(err_context)?,
            self.os_api,
            self.senders,
            self.character_cell_size
        )
        .with_context(err_context)?;
        // a pane that arrived hidden is not on screen, so there is nothing to focus and no
        // `may_be_focused` that would allow it anyway
        if should_focus_new_pane && !carry.hidden {
            match client_id {
                Some(client_id) => self.overlay_panes.focus_pane(pane_id, client_id),
                None => self.overlay_panes.focus_pane_for_all_clients(pane_id),
            }
        }
        self.set_force_render();
        Ok(())
    }
    pub fn add_floating_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        pane_id: PaneId,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        should_focus_new_pane: bool,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        let err_context = || "failed to add floating pane".to_string();
        // A pane that is an overlay by configuration is one wherever it came from, including the
        // paths that pass no coordinates at all (`focus_pane_with_id` taking a modal back out of
        // the suppressed panes) or their own (`pane_groups`). Placing it as a floating pane would
        // size it against the tab's viewport and raise the floating layer under it.
        if let Some(placement) =
            overlay_placement_for(&floating_pane_coordinates, pane.invoked_with())
        {
            let awaiting_placement = run_awaits_placement(pane.invoked_with());
            return self.add_overlay_pane(
                pane,
                pane_id,
                placement.kind,
                placement.coordinates.unwrap_or_default(),
                // this arm re-derives overlay-ness for a pane arriving through the floating door;
                // nothing on that path carries a hidden or a dragged overlay, and a pane being
                // born there is waiting to be placed on the same terms as one born the other way
                OverlayCarry {
                    awaiting_placement,
                    ..Default::default()
                },
                should_focus_new_pane,
                client_id,
            );
        }
        if self.floating_panes.fullscreen_is_active() {
            self.floating_panes.unset_fullscreen();
        }
        if let Some(mut new_pane_geom) = self.floating_panes.find_room_for_new_pane() {
            if let Some(floating_pane_coordinates) = &floating_pane_coordinates {
                if let Some(pinned) = floating_pane_coordinates.pinned.as_ref() {
                    pane.set_pinned(*pinned);
                }
                self.floating_panes
                    .adjust_coordinates(&mut new_pane_geom, floating_pane_coordinates.clone());
                self.swap_layouts.set_is_floating_damaged();
            }
            pane.set_active_at(Instant::now());
            pane.set_geom(new_pane_geom);
            let is_borderless = floating_pane_coordinates
                .as_ref()
                .and_then(|c| c.borderless)
                .unwrap_or(false);
            if is_borderless {
                pane.set_content_offset(Offset::frame(0));
                pane.set_borderless(true);
            } else {
                pane.set_content_offset(Offset::frame(1)); // floating panes always have a frame
                pane.render_full_viewport(); // to make sure the frame is re-rendered
            }
            resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                .with_context(err_context)?;
            self.floating_panes.add_pane(pane_id, pane);
            if should_focus_new_pane {
                // an overlay outranks the floating layer in `get_active_pane_id`, so unless a
                // dismissible one gives the keyboard up here the new pane opens *behind* the
                // popup, input keeps going to the popup, and the popup goes on reporting itself
                // focused rather than dismissing. A modal keeps focus - see
                // `defocus_pane_if_dismissible` - and the new pane comes up behind it, as it does
                // on the tiled path in `new_pane`.
                match client_id {
                    Some(client_id) => {
                        self.overlay_panes.defocus_pane_if_dismissible(client_id);
                        self.floating_panes.focus_pane(pane_id, client_id);
                    },
                    None => {
                        self.overlay_panes.defocus_dismissible_panes();
                        self.floating_panes.focus_pane_for_all_clients(pane_id);
                    },
                }
            }
        }
        if self.auto_layout && !self.swap_layouts.is_floating_damaged() {
            // only do this if we're already in this layout, otherwise it might be
            // confusing and not what the user intends
            self.swap_layouts.set_is_floating_damaged(); // we do this so that we won't skip to the
                                                         // next layout
            self.relayout_floating_panes(false)?;
        }
        Ok(())
    }
    pub fn add_tiled_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        pane_id: PaneId,
        without_relayout: bool,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        let should_auto_layout =
            self.auto_layout && !self.swap_layouts.is_tiled_damaged() && !without_relayout;
        if self.tiled_panes.has_room_for_new_pane() {
            pane.set_active_at(Instant::now());
            let not_placed = if should_auto_layout {
                // no need to relayout here, we'll do it when reapplying the swap layout
                // below
                self.tiled_panes
                    .insert_pane_without_relayout(pane_id, pane, client_id)
            } else {
                self.tiled_panes.insert_pane(pane_id, pane, client_id)
            };
            // `has_room_for_new_pane` and the placement itself do not ask quite
            // the same question — the check counts room in a stack that the
            // placement may then fail to make — so the grid can still hand the
            // pane back. Parked rather than dropped: a pane nothing renders is
            // recoverable, a pane nothing *holds* is a process with no owner.
            if let Some(pane) = not_placed {
                log::error!("No room for pane {pane_id:?} in the tiled layout, hiding it instead");
                self.park_pane(pane);
                return Ok(());
            }
            if !self.is_pending {
                // if this tab is pending, the geometry of the panes inside it (including the one we
                // just added) is provisional - the real one will be assigned when the layout is
                // applied to this tab, and so we do not want to reapply the frames here because
                // doing so would send this transient size to the pane's pty, causing it to receive
                // two resizes in quick succession (this one and the real one) - which some
                // terminal applications (eg. vim) coalesce and thus miss the real one
                self.tiled_panes.reapply_pane_frames();
            }
            self.set_should_clear_display_before_rendering();
            if let Some(client_id) = client_id {
                self.tiled_panes.focus_pane(pane_id, client_id);
            }
        }
        if should_auto_layout {
            // only do this if we're already in this layout, otherwise it might be
            // confusing and not what the user intends
            self.swap_layouts.set_is_tiled_damaged(); // we do this so that we won't skip to the
                                                      // next layout
            self.relayout_tiled_panes(false)?;
        }
        Ok(())
    }
    /// The complete destination geometry has already been checked by arrangement.
    /// Intermediate splits must not reject a batch that fits its final layout.
    pub fn accept_arranged_pane(
        &mut self,
        pane: Box<dyn Pane>,
        placement: ExtractedPanePlacement,
    ) -> AcceptedPane {
        let id = pane.pid();
        match placement {
            ExtractedPanePlacement::Tiled => self.tiled_panes.add_pane_with_existing_geom(id, pane),
            ExtractedPanePlacement::Floating => self.floating_panes.add_pane(id, pane),
            placement => return self.accept_extracted_pane(pane, placement),
        }
        AcceptedPane::Accepted
    }

    /// Take a pane extracted from another tab, or hand it back if there is no room.
    ///
    /// `add_tiled_pane`/`add_floating_pane` take the box by value and return `Ok(())`
    /// having *dropped* it when the tab has no room. For a pane being created that is
    /// merely a failed insert; for a pane being **moved** it is destruction - the pane
    /// has already left its old tab, and dropping the box emits no `PaneClosed`, sends
    /// no `PtyInstruction::ClosePane` and registers no orphan. The pane simply ceases
    /// to exist while its child process leaks.
    ///
    /// So check for room *before* giving up ownership, and hand the pane back if there
    /// is none, letting the caller put it where it came from.
    /// The pane is deliberately not focused, in either arm: moving a pane into a tab
    /// should not steal focus from whatever the tab was already showing.
    pub fn accept_extracted_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        placement: ExtractedPanePlacement,
    ) -> AcceptedPane {
        let pane_id = pane.pid();
        match placement {
            ExtractedPanePlacement::Overlay(overlay_placement, carry) => {
                // no room check: an overlay is not on a layer and does not tile, so there is nothing
                // for it to run out of. That is also why this arm cannot return `NoRoom` - only
                // `Accepted` or, if the pty thread is already gone, `LostInAdd`.
                //
                // No geom either: `OverlayPanes::add_pane` resolves the geometry from the
                // coordinates and the destination tab's surface, which is the point of carrying the
                // placement rather than the pane's absolute position across.
                self.add_overlay_pane(
                    pane,
                    pane_id,
                    overlay_placement.kind,
                    overlay_placement.coordinates.unwrap_or_default(),
                    carry,
                    false,
                    None,
                )
                .non_fatal();
            },
            ExtractedPanePlacement::Floating | ExtractedPanePlacement::FloatingDefault { .. } => {
                let Some(geometry) = self.floating_panes.find_room_for_new_pane() else {
                    return AcceptedPane::NoRoom(pane);
                };
                if matches!(placement, ExtractedPanePlacement::FloatingDefault { min_height }
                    if geometry.rows.as_usize() < min_height)
                {
                    return AcceptedPane::NoRoom(pane);
                }
                let keep_geometry = matches!(placement, ExtractedPanePlacement::Floating);
                let floating_pane_coordinates = FloatingPaneCoordinates {
                    x: keep_geometry.then(|| PercentOrFixed::Fixed(pane.x())),
                    y: keep_geometry.then(|| PercentOrFixed::Fixed(pane.y())),
                    width: keep_geometry.then(|| PercentOrFixed::Fixed(pane.cols())),
                    height: keep_geometry.then(|| PercentOrFixed::Fixed(pane.rows())),
                    pinned: Some(pane.current_geom().is_pinned),
                    borderless: Some(pane.borderless()),
                };
                // non_fatal rather than `?`: the only error here comes from `relayout`, which
                // runs after the pane is already in the tab, and the caller must not abort a
                // multi-pane move part-way through
                self.add_floating_pane(pane, pane_id, Some(floating_pane_coordinates), false, None)
                    .non_fatal();
            },
            ExtractedPanePlacement::Tiled => {
                if !self.tiled_panes.has_room_for_new_pane() {
                    return AcceptedPane::NoRoom(pane);
                }
                let new_geom = PaneGeom::from(&self.size);
                pane.set_geom(new_geom);
                self.add_tiled_pane(pane, pane_id, false, None).non_fatal();
            },
        }
        if self.has_pane_with_pid(&pane_id) {
            AcceptedPane::Accepted
        } else {
            // `add_floating_pane` also drops the pane when `resize_pty!` fails, which
            // only happens when the pty thread is already gone. The box is consumed, so
            // it cannot be handed back - say so rather than claim the pane moved.
            AcceptedPane::LostInAdd
        }
    }
    pub fn add_stacked_pane_to_pane_id(
        &mut self,
        pane: Box<dyn Pane>,
        pane_id: PaneId,
        root_pane_id: PaneId,
        should_focus: bool,
    ) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        self.tiled_panes
            .add_pane_to_stack_of_pane_id(pane_id, pane, root_pane_id);
        self.set_should_clear_display_before_rendering();
        // Groupify BEFORE the focus/frame re-application so the just-collapsed
        // in-grid stack member is moved to `suppressed_panes` before
        // `reapply_pane_frames` sends a `resize_pty!` at the 1-row collapsed
        // size. Without this, the shell inside that pane sees two rapid
        // WINCH events (1 row → visible-size rows) and redraws its prompt
        // twice, blanking out the visible rows of previous output.
        self.sync_stacked_pane_list_mode();
        if should_focus {
            self.tiled_panes.expand_pane_in_stack(pane_id);
        }
        self.swap_layouts.set_is_tiled_damaged();
        Ok(())
    }
    pub fn add_stacked_pane_to_active_pane(
        &mut self,
        pane: Box<dyn Pane>,
        pane_id: PaneId,
        client_id: ClientId,
        should_focus: bool,
    ) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        self.tiled_panes
            .add_pane_to_stack_of_active_pane(pane_id, pane, client_id);
        // See comment in `add_stacked_pane_to_pane_id` — groupify before
        // focusing so the collapsed member is suppressed before
        // `reapply_pane_frames` fires its intermediate `resize_pty!`.
        self.sync_stacked_pane_list_mode();
        if should_focus {
            self.tiled_panes.focus_pane(pane_id, client_id);
        }
        self.swap_layouts.set_is_tiled_damaged();
        Ok(())
    }
    pub fn request_plugin_permissions(
        &mut self,
        pid: PluginId,
        permissions: Option<PluginPermission>,
    ) {
        let pane_id = PaneId::Plugin(pid);
        if permissions.is_some() {
            // Permission is needed before a plugin can send its placement.
            self.overlay_panes.finish_placement(pane_id);
            if self.overlay_panes.is_hidden(&pane_id) {
                if let Some(pane) = self.overlay_panes.get_pane_mut(pane_id) {
                    pane.set_should_be_suppressed(true);
                }
                self.show_overlay_pane(pane_id);
                self.overlay_panes.focus_pane_for_all_clients(pane_id);
            }
        }
        // A suppressed pane asked for permissions cannot show the request, so it is focused
        // as a floating pane for the moment, marked so that it is suppressed again once the user
        // has answered
        let should_focus_pane =
            permissions.is_some() && self.suppressed_panes.contains_key(&pane_id);
        if let Some(plugin_pane) = self.get_pane_with_id_mut(pane_id) {
            if should_focus_pane {
                plugin_pane.set_should_be_suppressed(true);
            }
            plugin_pane.request_permissions_from_user(permissions);
        }
        if should_focus_pane {
            self.focus_suppressed_pane_for_all_clients(pane_id);
        }
    }
    pub fn rerun_terminal_pane_with_id(
        &mut self,
        terminal_pane_id: TerminalId,
        completion_tx: Option<NotificationEnd>,
    ) {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        match self.get_pane_with_id_mut(pane_id) {
            Some(pane_to_rerun) => {
                if let Some(command_to_rerun) = pane_to_rerun.rerun() {
                    self.pids_waiting_resize.insert(terminal_pane_id);
                    let _ = self.senders.send_to_pty(PtyInstruction::ReRunCommandInPane(
                        pane_id,
                        command_to_rerun,
                        completion_tx,
                    ));
                } else {
                    log::error!("Pane is still running!")
                }
            },
            None => {
                log::error!(
                    "Failed to find terminal pane with id {} to rerun in tab",
                    terminal_pane_id
                );
            },
        }
    }
    /// Run a *different* command in an existing command pane, reusing its pane id. When the
    /// pane's current command is still running — which includes the window between the
    /// command's exit event reaching its originating plugin and the screen processing the
    /// pane's hold — the replacement is stashed and started by `hold_pane` (last write wins).
    pub fn run_command_in_pane_with_id(
        &mut self,
        terminal_pane_id: TerminalId,
        run_command: RunCommand,
        keep_scrollback: bool,
    ) {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        match self.get_pane_with_id_mut(pane_id) {
            Some(pane) => {
                if let Some(command_to_run) =
                    pane.respawn(Some(run_command.clone()), keep_scrollback)
                {
                    self.pending_command_respawns.remove(&pane_id);
                    self.pids_waiting_resize.insert(terminal_pane_id);
                    let _ = self.senders.send_to_pty(PtyInstruction::ReRunCommandInPane(
                        pane_id,
                        command_to_run,
                        None,
                    ));
                } else {
                    self.pending_command_respawns
                        .insert(pane_id, (run_command, keep_scrollback));
                }
            },
            None => {
                log::error!(
                    "Failed to find terminal pane with id {} to run a command in",
                    terminal_pane_id
                );
            },
        }
    }
    pub fn resize_pane_with_id(&mut self, strategy: ResizeStrategy, pane_id: PaneId) -> Result<()> {
        self.resize_pane_in_layer(PaneSubject::Pane(pane_id), strategy)
    }
    pub fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
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
    pub fn extract_suppressed_panes(&mut self) -> SuppressedPanes {
        let stack_list_members = &self.stack_list_of_member;
        self.suppressed_panes
            .extract_if(|key, _| !stack_list_members.contains_key(key))
            .collect()
    }
    pub fn add_suppressed_panes(&mut self, mut suppressed_panes: SuppressedPanes) {
        for (pane_id, suppressed_pane) in suppressed_panes.drain() {
            self.suppressed_panes.insert(pane_id, suppressed_pane);
        }
    }
    /// Take this tab's dismissible overlays out of it, ready to be given to another tab.
    ///
    /// An overlay is drawn over the whole session surface rather than inside this tab's
    /// viewport, and hocus - the one thing that lives in one for a whole session - holds state no
    /// other pane does. So a tab going away is not a reason for one to be unloaded: `close_tab_by_id`
    /// takes them out here and re-homes them, exactly as it already does for suppressed panes.
    ///
    /// **Modals are deliberately left behind.** A modal is a dialog *about* the tab that is going
    /// away, and one re-homed unfocused is an input deadlock: `OverlayPanes::may_be_focused`
    /// refuses every other overlay the keyboard while a modal is in `stack_layer_ids`, and a modal
    /// has no channel to learn that it lost focus. It closes with the tab.
    ///
    /// The placement is read *before* the extraction, which is what destroys the wrapper holding
    /// an overlay's kind and coordinates - see [`Tab::placement_for_extraction`].
    pub fn extract_overlay_panes(&mut self) -> Vec<(ExtractedPanePlacement, Box<dyn Pane>)> {
        let pane_ids: Vec<PaneId> = self
            .overlay_panes
            .pane_ids()
            .filter(|pane_id| !self.overlay_panes.is_modal(pane_id))
            .copied()
            .collect();
        pane_ids
            .into_iter()
            .filter_map(|pane_id| {
                let placement = self.placement_for_extraction(pane_id);
                self.extract_pane(pane_id, true)
                    .map(|pane| (placement, pane))
            })
            .collect()
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
        self.dissolve_stack_lists_for_classic_mutation();
        if self.floating_panes.panes_contain(&root_pane_id)
            || self.overlay_panes.panes_contain(&root_pane_id)
            || self.suppressed_panes.contains_key(&root_pane_id)
        {
            log::error!("Root pane of stack cannot be floating, an overlay or suppressed");
            return false;
        }
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
        self.swap_layouts.set_is_tiled_damaged(); // TODO: verify we can do all the below first
        if self.pane_is_stacked(root_pane_id) {
            for pane in panes_to_stack.drain(..) {
                if let Some(pane) = self.tiled_panes.add_pane_to_stack(&root_pane_id, pane) {
                    log::error!("No room in the stack for pane {:?}, hiding it", pane.pid());
                    self.park_pane(pane);
                }
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
    /// The batch is the unit: two overlays linked as one panel (`OverlayPanes::link_touching`)
    /// and both named here are placed as that panel rather than one after the other.
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
    /// Link the overlays among `pane_ids` that a single placement batch has just put edge to
    /// edge, so the seam between them is draggable - see `OverlayPanes::link_touching`. Ids that
    /// are not overlays on this tab are ignored.
    pub fn link_touching_overlays(&mut self, pane_ids: &[PaneId]) {
        let here: Vec<PaneId> = pane_ids
            .iter()
            .filter(|pane_id| self.overlay_panes.panes_contain(pane_id))
            .copied()
            .collect();
        self.overlay_panes.link_touching(&here);
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
    #[allow(unused)] // this is used for tests
    pub fn query_mouse_hover_pane_id(&self) -> HashMap<ClientId, PaneId> {
        self.mouse_hover_pane_id.clone()
    }
    fn new_scrollback_editor_pane(&self, pid: TerminalId) -> TerminalPane {
        let next_terminal_position = self.get_next_terminal_position();
        let mut new_pane = TerminalPane::new(
            pid,
            PaneGeom::default(), // the initial size will be set later
            self.style,
            next_terminal_position,
            String::new(),
            self.link_handler.clone(),
            self.character_cell_size.clone(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            None,
            None,
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            None,
        );
        if let Some(supported) = self.kitty_host_support {
            new_pane.update_kitty_host_support(supported);
        }
        if let Some(supported) = self.sixel_host_support {
            new_pane.update_sixel_host_support(supported);
        }
        new_pane.update_name("EDITING SCROLLBACK"); // we do this here and not in the
                                                    // constructor so it won't be overrided
                                                    // by the editor
        new_pane
    }
    fn insert_scrollback_editor_replaced_pane(
        &mut self,
        replaced_pane: Box<dyn Pane>,
        terminal_pane_id: TerminalId,
    ) {
        let is_scrollback_editor = true;
        self.suppress_pane_behind(
            replaced_pane,
            PaneId::Terminal(terminal_pane_id),
            is_scrollback_editor,
        );
    }
    fn pane_is_stacked(&self, pane_id: PaneId) -> bool {
        self.get_pane_with_id(pane_id)
            .map(|p| p.position_and_size().stacked.is_some())
            .unwrap_or(false)
    }
    // --- Pane-targeting CLI methods ---
    pub fn scroll_up_by_pane_id(&mut self, pane_id: PaneId) {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.scroll_up(1, fictitious_client_id);
        }
    }
    pub fn scroll_down_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.scroll_down(1, fictitious_client_id);
            if !pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = pane.pid() {
                    self.process_pending_vte_events(raw_fd)?;
                }
            }
        }
        Ok(())
    }
    pub fn scroll_to_top_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_scroll();
            if let Some(size) = pane.get_line_number() {
                pane.scroll_up(size, fictitious_client_id);
            }
        }
        Ok(())
    }
    pub fn scroll_to_bottom_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_scroll();
            if !pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = pane.pid() {
                    self.process_pending_vte_events(raw_fd)?;
                }
            }
        }
        Ok(())
    }
    pub fn page_scroll_up_by_pane_id(&mut self, pane_id: PaneId) {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = pane.rows().max(1).saturating_sub(1);
            pane.scroll_up(scroll_rows, fictitious_client_id);
        }
    }
    pub fn page_scroll_down_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = pane.get_content_rows();
            pane.scroll_down(scroll_rows, fictitious_client_id);
            if !pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = pane.pid() {
                    self.process_pending_vte_events(raw_fd)?;
                }
            }
        }
        Ok(())
    }
    pub fn half_page_scroll_up_by_pane_id(&mut self, pane_id: PaneId) {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = (pane.rows().max(1).saturating_sub(1)) / 2;
            pane.scroll_up(scroll_rows, fictitious_client_id);
        }
    }
    pub fn half_page_scroll_down_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        let fictitious_client_id = ClientId::new(1);
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = (pane.rows().max(1).saturating_sub(1)) / 2;
            pane.scroll_down(scroll_rows, fictitious_client_id);
            if !pane.is_scrolled() {
                if let PaneId::Terminal(raw_fd) = pane.pid() {
                    self.process_pending_vte_events(raw_fd)?;
                }
            }
        }
        Ok(())
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
        }
        Ok(())
    }
    pub fn undo_rename_pane_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.load_pane_name();
        }
    }
    pub fn toggle_pane_pinned_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.toggle_pinned();
            self.set_force_render();
        }
    }
}

pub fn pane_info_for_pane(
    pane_id: &PaneId,
    pane: &dyn Pane,
    current_pane_group: &HashMap<ClientId, Vec<PaneId>>,
) -> PaneInfo {
    let index_in_pane_group: BTreeMap<ClientId, usize> = current_pane_group
        .iter()
        .filter_map(|(client_id, pane_ids)| {
            pane_ids
                .iter()
                .position(|p_id| p_id == &pane.pid())
                .map(|position| (*client_id, position))
        })
        .collect();
    let (default_fg, default_bg) = pane.get_pane_default_colors();
    let mut pane_info = PaneInfo {
        pane_x: zellij_types::position::Column::new(pane.x()),
        pane_content_x: zellij_types::position::Column::new(pane.get_content_x()),
        pane_y: zellij_types::position::Row::new(pane.y()),
        pane_content_y: zellij_types::position::Row::new(pane.get_content_y()),
        pane_rows: zellij_types::position::RowCount::new(pane.rows()),
        pane_content_rows: zellij_types::position::RowCount::new(pane.get_content_rows()),
        pane_columns: zellij_types::position::ColumnCount::new(pane.cols()),
        pane_content_columns: zellij_types::position::ColumnCount::new(pane.get_content_columns()),
        cursor_coordinates_in_pane: pane
            .cursor_coordinates(None)
            .and_then(|(x, y, is_visible)| if is_visible { Some((x, y)) } else { None })
            .map(Into::into),
        is_selectable: pane.selectable(),
        title: pane.current_title(),
        exited: pane.exited(),
        exit_status: pane.exit_status(),
        is_held: pane.is_held(),
        restart_error: pane.restart_error(),
        index_in_pane_group,
        default_fg,
        default_bg,
        agent: pane.agent_info().cloned(),
        ..Default::default()
    };

    match pane_id {
        PaneId::Terminal(terminal_id) => {
            pane_info.id = PaneId::Terminal(*terminal_id);
            if let Some(Run::Command(command)) = pane.invoked_with() {
                pane_info.target = command.target.clone();
            }
            pane_info.terminal_command = pane.invoked_with().as_ref().and_then(|c| match c {
                Run::Command(run_command) => Some(run_command.to_string()),
                _ => None,
            });
        },
        PaneId::Plugin(plugin_id) => {
            pane_info.id = PaneId::Plugin(*plugin_id);
            pane_info.plugin_url = pane.invoked_with().as_ref().and_then(|c| match c {
                Run::Plugin(run_plugin_or_alias) => Some(run_plugin_or_alias.location_string()),
                _ => None,
            });
        },
    }
    pane_info
}

/// Structural invariants that must hold after every operation on a `Tab`.
///
/// Written for the modals/overlays refactor: the property test in
/// `unit/screen_tests/overlays.rs` drives a random sequence of operations and calls this after
/// each one, and the characterization tests call it at their assertion points. Invariants that
/// only become true once overlays have their own collection are noted and added there.
#[cfg(any(test, feature = "test-support"))]
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
        for (index, pane_id) in overlay_stack.iter().enumerate() {
            assert!(
                self.overlay_panes.panes_contain(pane_id),
                "{pane_id:?} is in the overlay occlusion stack but not in the overlay panes"
            );
            assert_eq!(
                self.overlay_panes.get_pane_z_index(*pane_id),
                Some(index),
                "{pane_id:?} is at position {index} of the overlay occlusion stack but reports a \
                 different z-index"
            );
        }

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

#[cfg(test)]
#[path = "./unit/layout_applier_tests/mod.rs"]
mod layout_applier_tests;

#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub command: Option<String>,
    pub clipboard: zellij_types::input::options::Clipboard,
    pub copy_on_select: bool,
}

impl CopyOptions {
    pub fn new(
        copy_command: Option<String>,
        copy_clipboard: zellij_types::input::options::Clipboard,
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
#[cfg(any(test, feature = "test-support"))]
impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            command: None,
            clipboard: zellij_types::input::options::Clipboard::default(),
            copy_on_select: true,
        }
    }
}
