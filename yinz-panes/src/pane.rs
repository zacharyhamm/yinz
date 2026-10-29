//! The `Pane` trait every pane implements, and what the pane containers share with it: the
//! rendered-frame and input types its methods speak, the minimum pane size, the pty resize
//! macro, and the `PaneInfo` projection plugins see.

use crate::output::{CharacterChunk, KittyImageChunk, SixelImageChunk};
use crate::pane_tabs::PaneTabsTransfer;
use crate::panes::grid::PendingNotification;
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::panes::nested_session_modal::GuestModalShortcuts;
use crate::panes::{FloatingPanes, PaneId, TiledPanes};
use crate::pty::VteBytes;
use crate::route::NotificationEnd;
use crate::ui::{loading_indication::LoadingIndication, pane_boundaries_frame::FrameParams};
use crate::{ClientId, PluginId};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;
use yinz_ipc::nested_session::NestedSessionMessage;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::AgentInfo;
use yinz_types::data::{InputMode, PaletteColor, PaneContents, PaneInfo, RegexHighlight};
use yinz_types::geometry::Direction;
use yinz_types::input::command::RunCommand;
use yinz_types::input::layout::{Run, RunPluginLocation};
use yinz_types::internal_ids::SessionName;
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::MouseEvent;
use yinz_types::pane_size::{Offset, PaneGeom};
use yinz_types::position::Position;
use yinz_types::style::{Style, Styling};

#[macro_export]
macro_rules! resize_pty {
    ($pane:expr_2021, $os_input:expr_2021, $senders:expr_2021, $character_cell_size:expr_2021) => {{
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

// Defined in yinz-types so layouts can check against them too, and re-exported under the path
// the pane layer has always named them by.
pub use yinz_types::pane_size::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};

pub type HoldForCommand = Option<RunCommand>;

/// One rendered frame of a pane: its character chunks, the raw vte output to forward to the
/// client (if any), and its sixel and kitty image chunks.
pub type RenderedPane = (
    Vec<CharacterChunk>,
    Option<String>,
    Vec<SixelImageChunk>,
    Vec<KittyImageChunk>,
);

/// Terminal replies, host queries, and output side effects.
pub trait TerminalPaneIo {
    fn drain_messages_to_pty(&mut self) -> Vec<Vec<u8>>;
    /// Learned source palettes to retain in the pane's supervisor.
    fn drain_agent_theme_updates(&mut self) -> Vec<yinz_types::agent::theme::AgentThemeEvent>;
    /// Whitelisted terminal queries to forward to the host terminal.
    fn drain_forwarded_queries(&mut self) -> Vec<crate::host_query::HostQuery>;
    fn record_terminal_color_reply(&mut self, query: &crate::host_query::HostQuery, reply: &[u8]);
    fn drain_nested_session_messages(&mut self) -> Vec<NestedSessionMessage>;
    /// Buffer PTY input until the host reply reaches the pane's stdin, preserving stream order.
    fn arm_forward_pause(&mut self);
    /// Resume parsing, returning whether the pane was previously paused.
    fn clear_forward_pause(&mut self) -> bool;
    /// Drain buffered input for Tab to re-feed after delivering the host reply.
    fn drain_pending_pty_input(&mut self) -> Vec<u8>;
    fn is_forward_paused(&self) -> bool;
    /// Queue a CSI ?997 theme notification only when the application opted in with CSI ?2031 h.
    fn push_color_palette_dsr(&mut self, mode: yinz_types::style::HostTerminalThemeMode);
    fn drain_clipboard_update(&mut self) -> Option<String>;
    fn drain_desktop_notifications(&mut self) -> Vec<PendingNotification>;
    fn drain_osc7_cwd(&mut self) -> Option<std::path::PathBuf>;
}

/// Plugin loading and location updates.
pub trait PluginPaneControl {
    fn update_plugin_location(&mut self, location: RunPluginLocation);
    fn update_loading_indication(&mut self, loading_indication: LoadingIndication);
    fn start_loading_indication(&mut self, loading_indication: LoadingIndication);
    fn progress_animation_offset(&mut self);
}

pub trait Pane {
    fn terminal_io_mut(&mut self) -> Option<&mut dyn TerminalPaneIo> {
        None
    }
    fn plugin_control_mut(&mut self) -> Option<&mut dyn PluginPaneControl> {
        None
    }
    fn x(&self) -> usize {
        self.current_geom().x
    }
    fn y(&self) -> usize {
        self.current_geom().y
    }
    fn rows(&self) -> usize {
        self.current_geom().rows.as_usize()
    }
    fn cols(&self) -> usize {
        self.current_geom().cols.as_usize()
    }
    fn get_content_x(&self) -> usize {
        self.render_geom().x + self.get_content_offset().left
    }
    fn get_content_y(&self) -> usize {
        self.y() + self.get_content_offset().top + self.content_header_rows()
    }
    /// Content columns can differ from the pane's columns: with a frame they are 2 less.
    fn get_content_columns(&self) -> usize {
        let offset = self.get_content_offset();
        self.render_geom()
            .cols
            .as_usize()
            .saturating_sub(offset.left + offset.right)
    }
    /// Content rows can differ from the pane's rows: with a frame they are 2 less, and the
    /// header rows come off too.
    fn get_content_rows(&self) -> usize {
        let offset = self.get_content_offset();
        self.rows()
            .saturating_sub(offset.top + offset.bottom + self.content_header_rows())
    }
    /// Rows drawn below the content offset's top and above the content: the subtitle, plus
    /// anything a pane type adds there, such as a plugin pane's tab bar.
    fn content_header_rows(&self) -> usize {
        self.subtitle_rows()
    }
    /// Initialize per-client state when a client joins the tab, preserving existing output.
    fn add_client(&mut self, _client_id: ClientId) {}
    /// Forget state only when a client leaves the session, not when it changes tabs.
    fn remove_client(&mut self, _client_id: ClientId) {}
    /// Replay consumed content for a watcher without invalidating frame caches.
    fn prepare_content_replay(&mut self, client_id: ClientId);
    /// Replay a frame without dirtying the pane's contents.
    fn invalidate_frame(&mut self, _client_id: ClientId) {}
    fn reset_size_and_position_override(&mut self);
    fn set_geom(&mut self, position_and_size: PaneGeom);
    /// Layout intent survives presentation changes such as a collapsed tab bar.
    fn configured_rows(&self) -> yinz_types::pane_size::Dimension {
        self.position_and_size().rows
    }
    fn set_configured_rows(&mut self, _rows: yinz_types::pane_size::Dimension) {}
    fn set_geom_override(&mut self, pane_geom: PaneGeom);
    /// Apply a complete layout before resizing content. Grid-backed panes override this to avoid
    /// reflowing through intermediate sizes when a frame becomes part of a shared container.
    fn set_layout(
        &mut self,
        geom: PaneGeom,
        geom_override: Option<PaneGeom>,
        borderless: bool,
        content_offset: Offset,
    ) {
        self.set_geom(geom);
        if let Some(geom_override) = geom_override {
            self.set_geom_override(geom_override);
        } else {
            self.reset_size_and_position_override();
        }
        self.set_borderless_with_content_offset(borderless, content_offset);
    }
    fn character_cell_size_possibly_changed(&mut self) {}
    /// Seed layout contents before the pane's process or plugin produces output.
    fn set_initial_contents(&mut self, contents: &str);
    fn handle_pty_bytes(&mut self, _bytes: VteBytes) {}
    fn handle_plugin_bytes(&mut self, _client_id: ClientId, _bytes: VteBytes) {}
    fn show_cursor(&mut self, _client_id: ClientId, _cursor_position: Option<(usize, usize)>) {}
    /// Returns the cursor position relative to `render_geom` and whether it is visible.
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
    /// The geometry the pane occupies now: its override if it has one, else its own.
    fn current_geom(&self) -> PaneGeom {
        self.geom_override()
            .unwrap_or_else(|| self.position_and_size())
    }
    /// Keep requested layout geometry intact when presentation is capped.
    fn render_geom(&self) -> PaneGeom {
        self.constrain_render_geom(self.current_geom())
    }
    fn constrain_render_geom(&self, mut geom: PaneGeom) -> PaneGeom {
        if let Some((x, cols)) = self.render_width() {
            geom.x = x;
            geom.cols.set_inner(cols);
        }
        self.constrain_content_width(geom)
    }
    fn constrain_content_width(&self, mut geom: PaneGeom) -> PaneGeom {
        let limit = self.max_content_width();
        if limit > 0 {
            let offset = self.get_content_offset();
            let width = limit
                .saturating_add(offset.left)
                .saturating_add(offset.right);
            let spare = geom.cols.as_usize().saturating_sub(width);
            geom.x += spare / 2;
            geom.cols.decrease_inner(spare);
        }
        geom
    }
    fn max_content_width(&self) -> usize {
        0
    }
    /// Horizontal presentation geometry, leaving requested layout sizes intact.
    fn render_width(&self) -> Option<(usize, usize)> {
        None
    }
    fn set_render_width(&mut self, _width: Option<(usize, usize)>) -> bool {
        false
    }
    fn is_detected_agent(&self) -> bool {
        false
    }
    fn set_detected_agent(&mut self, _detected: bool) {}
    /// Returns whether the displayed rectangle changed.
    fn set_max_content_width(&mut self, _width: usize) -> bool {
        false
    }
    fn geom_override(&self) -> Option<PaneGeom>;
    fn should_render(&self) -> bool;
    fn set_should_render(&mut self, should_render: bool);
    fn set_should_render_boundaries(&mut self, _should_render: bool) {}
    fn selectable(&self) -> bool;
    fn set_selectable(&mut self, selectable: bool);
    fn render(&mut self, client_id: Option<ClientId>) -> Result<Option<RenderedPane>>;
    /// Clients whose next Esc confirms input to this pane; only their frame shows the hint.
    fn set_pending_agent_escape_clients(&mut self, _clients: HashSet<ClientId>) {}

    /// Return frame character chunks and optional raw terminal output, or `None` when unchanged.
    fn render_frame(
        &mut self,
        client_id: ClientId,
        frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>>;
    fn render_fake_cursor(
        &mut self,
        cursor_color: PaletteColor,
        text_color: PaletteColor,
    ) -> Option<String>;
    fn render_terminal_title(
        &mut self,
        input_mode: InputMode,
        session_name: Option<&SessionName>,
    ) -> String;
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
    /// `client_id` is the client scrolling, or `None` for a request no client made, which a
    /// plugin pane passes to every instance of its plugin.
    fn scroll_up(&mut self, count: usize, client_id: Option<ClientId>);
    fn scroll_down(&mut self, count: usize, client_id: Option<ClientId>);
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
        self.render_geom().contains(position)
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
    fn update_agent_theme_mapping(
        &mut self,
        _mapping: yinz_types::agent::theme::AgentThemeMapping,
    ) {
    }
    fn update_terminal_theme(&mut self, _theme: Option<yinz_types::style::TerminalTheme>) {}
    /// Host defaults for chrome animation, excluding application and pane overrides.
    fn host_colors(&self) -> (PaletteColor, PaletteColor) {
        let palette = yinz_types::style::Palette::default();
        (palette.fg, palette.bg)
    }
    fn set_pane_default_colors(&mut self, _fg: Option<String>, _bg: Option<String>) {}
    fn get_pane_default_colors(&self) -> (Option<String>, Option<String>) {
        (None, None)
    }
    /// The agent running in this pane, as the session knows it. Only terminal panes have one.
    fn agent_info(&self) -> Option<&AgentInfo> {
        None
    }
    fn supports_bracketed_paste(&self) -> bool {
        false
    }
    /// Counts user input delivered to this pane, independent of its current tab.
    fn agent_input_revision(&self) -> u64 {
        0
    }
    /// Input revision of the latest unmodified Enter delivered to this pane.
    fn agent_enter_revision(&self) -> u64 {
        0
    }
    /// When input last reached this pane, as counted by [`Pane::agent_input_revision`].
    fn agent_input_at(&self) -> Option<Instant> {
        None
    }
    fn record_agent_input(&mut self, _is_enter: bool) {}
    /// Replace the pane's agent record; answers whether it changed.
    fn set_agent_info(&mut self, _agent: Option<AgentInfo>) -> bool {
        false
    }
    /// Whether this pane has ever held an agent record. Sticky, unlike the record itself,
    /// which a `SessionEnd` hook clears just before the agent's process exits.
    fn last_hook_at_ms(&self) -> Option<i64> {
        None
    }
    fn set_last_hook_at_ms(&mut self, _at: i64) {}
    fn last_agent_cwd(&self) -> Option<&str> {
        None
    }
    fn last_agent_session_id(&self) -> Option<&str> {
        None
    }
    fn last_agent_kind(&self) -> Option<&str> {
        None
    }
    fn was_an_agent(&self) -> bool {
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
    fn min_width(&self) -> usize {
        MIN_TERMINAL_WIDTH
    }
    fn min_height(&self) -> usize {
        MIN_TERMINAL_HEIGHT
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
    fn render_full_viewport(&mut self) {}
    fn relative_position(&self, position_on_screen: &Position) -> Position {
        position_on_screen.relative_to(self.get_content_y(), self.get_content_x())
    }
    fn position_is_on_frame(&self, position: &Position) -> bool {
        if !self.contains(position) {
            return false;
        }
        let geom = self.render_geom();
        if (geom.x..self.get_content_x()).contains(&position.column()) {
            // position is on left border
            return true;
        }
        if (self.get_content_x() + self.get_content_columns()..(geom.x + geom.cols.as_usize()))
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
        let geom = self.render_geom();
        let mid_col = geom.x + geom.cols.as_usize() / 2;
        let mid_line = geom.y + geom.rows.as_usize() / 2;

        let is_left = pos_col < mid_col;
        let is_top = (pos_line as usize) < mid_line;

        Some(match (is_top, is_left) {
            (true, true) => PaneEdge::TopLeft,
            (true, false) => PaneEdge::TopRight,
            (false, true) => PaneEdge::BottomLeft,
            (false, false) => PaneEdge::BottomRight,
        })
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
    /// Set border metadata and the content inset together, without a temporary frame resize.
    fn set_borderless_with_content_offset(&mut self, borderless: bool, offset: Offset) {
        self.set_borderless(borderless);
        self.set_content_offset(offset);
    }
    fn borderless(&self) -> bool;
    fn set_exclude_from_sync(&mut self, exclude_from_sync: bool);
    fn exclude_from_sync(&self) -> bool;

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
    fn osc7_payload(&self) -> Option<&str> {
        None
    }
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
    /// Start of the current human-input waiting episode, retained until that wait ends.
    fn waiting_pulse_started_at(&self) -> Option<Instant> {
        None
    }
    /// Sample the pulse strength once per frame.
    fn sample_waiting_pulse(&mut self, _now: Instant, _enabled: bool) {}
    fn waiting_pulse(&self) -> Option<f32> {
        None
    }
    fn invoked_with(&self) -> &Option<Run>;
    fn is_builtin_tab_bar(&self) -> bool {
        matches!(self.invoked_with(), Some(Run::Plugin(plugin))
            if plugin.presentation().tab_bar)
    }
    fn set_notification_end(&mut self, _notification_end: Option<NotificationEnd>) {}
    fn set_title(&mut self, title: String);
    fn current_title(&self) -> String;
    fn set_remote_host(&mut self, _host: Option<String>) {} // only relevant for terminal panes
    /// A decoration to compose in front of the title, whatever the title is.
    ///
    /// Set by whoever knows something durable about the pane that its own title
    /// cannot be trusted to keep — agent-service puts an agent's checkout here.
    /// Only relevant for terminal panes.
    fn set_title_prefix(&mut self, _prefix: Option<String>) {}
    fn set_subtitle(&mut self, _subtitle: Option<String>) {}
    fn take_pane_tabs_transfer(&mut self) -> Option<PaneTabsTransfer> {
        None
    }
    fn set_pane_tabs_transfer(&mut self, _transfer: PaneTabsTransfer) {}
    fn take_pane_tabs_layout(&mut self) -> Option<yinz_types::pane_tabs::PaneTabsLayout> {
        None
    }
    fn set_agent_hook_flash(&mut self, _flash: bool) {}
    fn agent_title(&self) -> crate::ui::agent_title::AgentTitle {
        crate::ui::agent_title::AgentTitle {
            state: self.agent_info().map(|agent| agent.state),
            hook_flash: false,
        }
    }
    fn set_pane_tab_titles(&mut self, _titles: Vec<crate::ui::agent_title::AgentTitle>) {}
    fn pane_tab_titles(&self) -> &[crate::ui::agent_title::AgentTitle] {
        &[]
    }
    fn set_pane_tabs_views(
        &mut self,
        _views: std::collections::HashMap<ClientId, crate::pane_tabs::PaneTabsView>,
    ) {
    }
    fn pane_tabs_view(&self, client: ClientId) -> crate::pane_tabs::PaneTabsView;
    fn set_pane_tabs(&mut self, _tabs: Option<yinz_types::pane_tabs::PaneTabsInfo>) {}
    fn pane_tabs(&self) -> Option<&yinz_types::pane_tabs::PaneTabsInfo> {
        None
    }
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
    fn stack_list_entry_label(&self, _same_worktree: bool) -> String {
        self.display_title()
    }
    fn custom_title(&self) -> Option<String>;
    fn set_running_program(&mut self, _program: Option<String>) {}
    fn can_lend_title(&self) -> bool {
        self.has_explicit_title()
    }
    fn has_explicit_title(&self) -> bool {
        false
    }
    fn scroll_position(&self) -> (usize, usize) {
        (0, 0)
    }
    fn set_warp_pending(&mut self, _pending: bool) {}
    fn held_for_warp(&self) -> Option<yinz_types::data::HeldTerminal> {
        if !self.is_held() {
            return None;
        }
        let Some(Run::Command(command)) = self.invoked_with() else {
            return None;
        };
        Some(yinz_types::data::HeldTerminal {
            command: command.clone(),
            first_run: !self.exited(),
            exit_status: self.exit_status(),
        })
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
    fn serialize_handoff(
        &self,
        scrollback: Option<usize>,
        budget: usize,
    ) -> Option<(String, Option<yinz_types::agent::theme::AgentThemeHandoff>)> {
        self.serialize(scrollback).map(|contents| {
            (
                yinz_terminal::agent_theme::fit_legacy_contents(contents, budget),
                None,
            )
        })
    }
    fn serialize(&self, _scrollback_lines_to_serialize: Option<usize>) -> Option<String> {
        None
    }
    fn restart_error(&self) -> Option<String> {
        None
    }
    fn agent_restart_command(
        &self,
        _session: yinz_types::agent::control::AgentRestartSession,
        _prompt: Option<&str>,
    ) -> Result<RunCommand, String> {
        Err("target must be an exited, held agent pane".into())
    }
    fn clear_agent_session_id(&mut self) {}
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
    CloseThisPane,
    DropToShellInThisPane {
        working_dir: Option<PathBuf>,
    },
    WriteKeyToPlugin(KeyWithModifier),
    GuestModalSelectionChanged,
    GuestModalZoom,
    GuestModalDescend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
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
        pane_tabs: pane.pane_tabs().cloned(),
        pane_x: yinz_types::position::Column::new(pane.render_geom().x),
        pane_content_x: yinz_types::position::Column::new(pane.get_content_x()),
        pane_y: yinz_types::position::Row::new(pane.y()),
        pane_content_y: yinz_types::position::Row::new(pane.get_content_y()),
        pane_rows: yinz_types::position::RowCount::new(pane.rows()),
        pane_content_rows: yinz_types::position::RowCount::new(pane.get_content_rows()),
        pane_columns: yinz_types::position::ColumnCount::new(pane.render_geom().cols.as_usize()),
        pane_content_columns: yinz_types::position::ColumnCount::new(pane.get_content_columns()),
        cursor_coordinates_in_pane: pane
            .cursor_coordinates(None)
            .and_then(|(x, y, is_visible)| if is_visible { Some((x, y)) } else { None })
            .map(Into::into),
        is_selectable: pane.selectable(),
        title: pane.current_title(),
        display_title: Some(pane.display_title()),
        exited: pane.exited(),
        exit_status: pane.exit_status(),
        is_held: pane.is_held(),
        restart_error: pane.restart_error(),
        last_agent_session_id: pane.last_agent_session_id().map(str::to_owned),
        last_hook_at_ms: pane.last_hook_at_ms(),
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
