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
        $crate::resize_pty!($pane, $os_input, $senders, $character_cell_size, ResizePty)
    }};
    ($pane:expr_2021, $os_input:expr_2021, $senders:expr_2021, $character_cell_size:expr_2021, $instruction:ident) => {{
        let (width_in_pixels, height_in_pixels) = {
            let character_cell_size = $character_cell_size.borrow();
            match *character_cell_size {
                Some(size_in_pixels) => {
                    // PTY winsize fields are u16; large dimensions must not wrap.
                    let width_in_pixels = size_in_pixels
                        .width
                        .saturating_mul($pane.get_content_columns())
                        .min(u16::MAX as usize) as u16;
                    let height_in_pixels = size_in_pixels
                        .height
                        .saturating_mul($pane.get_content_rows())
                        .min(u16::MAX as usize) as u16;
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
                    .send_to_pty_writer(PtyWriteInstruction::$instruction(
                        *pid,
                        $pane.get_content_columns().min(u16::MAX as usize) as u16,
                        $pane.get_content_rows().min(u16::MAX as usize) as u16,
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
    fn update_detected_agent_theme(
        &mut self,
        state: Option<yinz_types::agent::theme::DetectedAgentTheme>,
    );
    fn drain_agent_theme_updates(&mut self) -> Vec<yinz_types::agent::theme::AgentThemeEvent>;
    /// Whitelisted terminal queries to forward to the host terminal.
    fn drain_forwarded_queries(&mut self) -> Vec<crate::host_query::HostQuery>;
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
    fn drain_clipboard_updates(&mut self) -> Vec<String>;
    fn drain_desktop_notifications(&mut self) -> Vec<PendingNotification>;
    fn drain_osc7_cwd(&mut self) -> Option<std::path::PathBuf>;
}

/// Plugin input delivery, per-client output, and loading state.
pub trait PluginPaneControl {
    fn update_plugin_location(&mut self, location: RunPluginLocation);
    fn update_loading_indication(&mut self, loading_indication: LoadingIndication);
    fn start_loading_indication(&mut self, loading_indication: LoadingIndication);
    /// A load of the plugin's instance for `client_id` alone, while the plugin keeps running for
    /// the other clients: only that client's grid shows it.
    fn start_client_loading_indication(
        &mut self,
        client_id: ClientId,
        loading_indication: LoadingIndication,
    );
    /// `client_id`'s instance rendered: its own loading indication, if any, is over.
    fn end_client_loading_indication(&mut self, client_id: ClientId);
    fn progress_animation_offset(&mut self);
    fn handle_plugin_bytes(&mut self, client_id: ClientId, bytes: VteBytes);
    fn show_cursor(&mut self, client_id: ClientId, cursor_position: Option<(usize, usize)>);
    fn set_should_be_suppressed(&mut self, should_be_suppressed: bool);
    fn query_should_be_suppressed(&self) -> bool;
    fn set_mouse_selection_support(&mut self, selection_support: bool);
    fn adjust_input_to_plugin(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        client_id: Option<ClientId>,
    ) -> Option<AdjustedInput>;
    /// Deliver one pointer event to this plugin, optionally targeting a single client.
    fn send_mouse(&self, mouse: yinz_types::data::Mouse, client_id: Option<ClientId>)
        -> Result<()>;
}

/// Layout geometry and mutations applied by the solver.
pub trait PaneGeometry {
    fn pid(&self) -> PaneId;
    /// Layout coordinates retain the requested rectangle; use `render_geom` for presentation.
    fn x(&self) -> usize {
        self.current_geom().x
    }
    fn y(&self) -> usize {
        self.current_geom().y
    }
    fn cols(&self) -> usize {
        self.current_geom().cols.as_usize()
    }
    fn rows(&self) -> usize {
        self.current_geom().rows.as_usize()
    }
    fn position_and_size(&self) -> PaneGeom;
    fn geom_override(&self) -> Option<PaneGeom>;
    /// Layout geometry: its override if it has one, else its own, before presentation caps.
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
    fn get_content_offset(&self) -> Offset {
        Offset::default()
    }
    fn min_width(&self) -> usize {
        MIN_TERMINAL_WIDTH
    }
    fn min_height(&self) -> usize {
        MIN_TERMINAL_HEIGHT
    }
    fn set_geom(&mut self, position_and_size: PaneGeom);
    fn set_geom_override(&mut self, pane_geom: PaneGeom);
    fn reset_size_and_position_override(&mut self);
    fn right_boundary_x_coords(&self) -> usize {
        self.x() + self.cols()
    }
    fn bottom_boundary_y_coords(&self) -> usize {
        self.y() + self.rows()
    }
    fn reduce_height(&mut self, percent: f64);
    fn increase_height(&mut self, percent: f64);
    fn reduce_width(&mut self, percent: f64);
    fn increase_width(&mut self, percent: f64);
    fn push_down(&mut self, count: usize);
    fn push_right(&mut self, count: usize);
    fn pull_left(&mut self, count: usize);
    fn pull_up(&mut self, count: usize);
    fn is_right_of(&self, other: PaneGeom) -> bool {
        self.x() > other.x
    }
    fn is_directly_right_of(&self, other: PaneGeom) -> bool {
        self.x() == other.x + other.cols.as_usize()
    }
    fn is_left_of(&self, other: PaneGeom) -> bool {
        self.x() < other.x
    }
    fn is_directly_left_of(&self, other: PaneGeom) -> bool {
        self.x() + self.cols() == other.x
    }
    fn is_below(&self, other: PaneGeom) -> bool {
        self.y() > other.y
    }
    fn is_directly_below(&self, other: PaneGeom) -> bool {
        self.y() == other.y + other.rows.as_usize()
    }
    fn is_above(&self, other: PaneGeom) -> bool {
        self.y() < other.y
    }
    fn is_directly_above(&self, other: PaneGeom) -> bool {
        self.y() + self.rows() == other.y
    }
    /// Shared layout rows allow horizontal navigation between adjacent panes.
    fn horizontally_overlaps_with(&self, other: PaneGeom) -> bool {
        self.get_horizontal_overlap_with(other) > 0
    }
    /// Number of shared layout rows, or zero for empty, touching or disjoint spans.
    fn get_horizontal_overlap_with(&self, other: PaneGeom) -> usize {
        std::cmp::min(self.y() + self.rows(), other.y + other.rows.as_usize())
            .saturating_sub(std::cmp::max(self.y(), other.y))
    }
    /// Shared layout columns allow vertical navigation between adjacent panes.
    fn vertically_overlaps_with(&self, other: PaneGeom) -> bool {
        self.get_vertical_overlap_with(other) > 0
    }
    /// Number of shared layout columns, or zero for empty, touching or disjoint spans.
    fn get_vertical_overlap_with(&self, other: PaneGeom) -> usize {
        std::cmp::min(self.x() + self.cols(), other.x + other.cols.as_usize())
            .saturating_sub(std::cmp::max(self.x(), other.x))
    }
}

/// Presentation state used to navigate a tiled layout.
pub trait TiledGridPane: PaneGeometry {
    fn selectable(&self) -> bool;
    fn active_at(&self) -> Instant;
    fn borderless(&self) -> bool;
}

/// Operations backed by a terminal emulator or its process.
pub trait TerminalPaneOps: TerminalPaneIo {
    // Semantic state and migration
    /// Emulator state only: destination identity and geometry remain local.
    fn terminal_state(&self) -> Result<String, String>;
    fn restore_terminal_state(&mut self, state: &str) -> Result<(), String>;
    fn validate_migration(&self) -> Result<(), String>;
    fn migration_state(&self) -> Result<serde_json::Value, String>;
    fn restore_migration_state(&mut self, state: serde_json::Value) -> Result<(), String>;
    fn serialize(&self, scrollback_lines_to_serialize: Option<usize>) -> Option<String>;
    fn serialize_handoff(
        &self,
        scrollback: Option<usize>,
        budget: usize,
    ) -> Option<(String, Option<yinz_types::agent::theme::AgentThemeHandoff>)>;
    fn set_warp_pending(&mut self, pending: bool);
    fn held_for_warp(&self) -> Option<yinz_types::data::HeldTerminal>;

    // Process lifecycle
    fn set_launch_command(&mut self, command: RunCommand);
    fn set_running_program(&mut self, program: Option<String>);
    fn set_remote_host(&mut self, host: Option<String>);
    fn hold(&mut self, exit_status: Option<i32>, is_first_run: bool, run_command: RunCommand);
    fn is_held(&self) -> bool;
    /// The pane's root process was stopped, or started running again.
    fn set_suspended(&mut self, suspended: bool);
    /// The pane's root process is stopped and can only be woken from out here.
    fn is_suspended(&self) -> bool;
    fn exited(&self) -> bool;
    fn exit_status(&self) -> Option<i32>;
    fn update_exit_status(&mut self, exit_status: i32);
    fn rerun(&mut self) -> Option<RunCommand> {
        self.respawn(None, false)
    }
    /// Restart a held command pane, either with its own command (`None`) or with a
    /// `replacement`. Returns the command to spawn, or `None` when the pane is not a held
    /// command pane.
    fn respawn(
        &mut self,
        replacement: Option<RunCommand>,
        keep_scrollback: bool,
    ) -> Option<RunCommand>;
    fn restart_error(&self) -> Option<String>;
    fn agent_restart_command(
        &self,
        session: yinz_types::agent::control::AgentRestartSession,
        prompt: Option<&str>,
    ) -> Result<RunCommand, String>;
    fn clear_agent_session_id(&mut self);

    // Agent and program status
    /// The agent running in this pane, as the session knows it.
    fn agent_info(&self) -> Option<&AgentInfo>;
    /// Replace the pane's agent record; answers whether it changed.
    fn set_agent_info(&mut self, agent: Option<AgentInfo>) -> bool;
    /// Remains true after the active agent record is cleared on SessionEnd.
    fn was_an_agent(&self) -> bool;
    fn is_detected_agent(&self) -> bool;
    fn set_detected_agent(&mut self, detected: bool);
    fn last_hook_at_ms(&self) -> Option<i64>;
    fn set_last_hook_at_ms(&mut self, at: i64);
    fn last_agent_cwd(&self) -> Option<&str>;
    fn last_agent_session_id(&self) -> Option<&str>;
    fn last_agent_kind(&self) -> Option<&str>;
    /// Counts user input delivered to this pane, independent of its current tab.
    fn agent_input_revision(&self) -> u64;
    /// Input revision of the latest unmodified Enter delivered to this pane.
    fn agent_enter_revision(&self) -> u64;
    /// When input last reached this pane, as counted by [`TerminalPaneOps::agent_input_revision`].
    fn agent_input_at(&self) -> Option<Instant>;
    /// Record submitted input and return whether program-status applicability changed.
    fn record_agent_input(&mut self, is_enter: bool) -> bool;
    fn program_status(&self) -> yinz_types::program_status::ProgramStatusView;
    /// Runtime change counter for status ingestion; reading it never clones history.
    fn program_status_revision(&self) -> u64;
    fn retire_program_status(&mut self);
    /// A decoration to compose in front of the title, whatever the title is.
    ///
    /// Set by whoever knows something durable about the pane that its own title
    /// cannot be trusted to keep — agent-service puts an agent's checkout here.
    /// Only relevant for terminal panes.
    fn set_title_prefix(&mut self, prefix: Option<String>);
    fn set_agent_hook_flash(&mut self, flash: bool);
    /// Clients whose next Esc confirms input to this pane; only their frame shows the hint.
    fn set_pending_agent_escape_clients(&mut self, clients: HashSet<ClientId>);

    // Nested sessions
    fn is_nested_guest(&self) -> bool;
    fn set_is_nested_guest(&mut self, is_nested_guest: bool);
    fn set_guest_modal(&mut self, client_ids: &[ClientId]);
    fn clear_guest_modal(&mut self, client_id: ClientId);
    fn clear_all_guest_modals(&mut self);
    fn guest_modal_selection(&self, client_id: ClientId) -> Option<usize>;
    fn has_guest_modal_for_any_client(&self) -> bool;
    fn set_guest_choice_indicator(
        &mut self,
        client_id: ClientId,
        indicator: Option<GuestChoiceIndicator>,
    );
    fn guest_choice_indicator(&self, client_id: ClientId) -> Option<GuestChoiceIndicator>;
    fn clear_all_guest_choice_indicators(&mut self);
    fn set_guest_session_name(&mut self, session_name: Option<SessionName>);
    fn guest_session_name(&self) -> Option<SessionName>;
    fn set_guest_modal_shortcuts(&mut self, shortcuts: GuestModalShortcuts);
    fn guest_modal_shortcuts(&self) -> GuestModalShortcuts;

    // Terminal input and emulator reports
    fn handle_pty_bytes(&mut self, bytes: VteBytes);
    fn adjust_input_to_terminal(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        client_id: Option<ClientId>,
    ) -> Option<AdjustedInput>;
    fn supports_bracketed_paste(&self) -> bool;
    fn terminal_emulator_wants_mouse(&self) -> bool;
    fn is_alternate_mode_active(&self) -> bool;
    fn mouse_event(&self, event: &MouseEvent, client_id: ClientId) -> Option<String>;
    fn mouse_left_click(&self, position: &Position, is_held: bool) -> Option<String>;
    fn mouse_left_click_release(&self, position: &Position) -> Option<String>;
    fn mouse_right_click(&self, position: &Position, is_held: bool) -> Option<String>;
    fn mouse_right_click_release(&self, position: &Position) -> Option<String>;
    fn mouse_middle_click(&self, position: &Position, is_held: bool) -> Option<String>;
    fn mouse_middle_click_release(&self, position: &Position) -> Option<String>;
    fn mouse_scroll_up(&self, position: &Position) -> Option<String>;
    fn mouse_scroll_down(&self, position: &Position) -> Option<String>;
    fn focus_event(&self) -> Option<String>;
    fn unfocus_event(&self) -> Option<String>;
    fn update_kitty_keyboard_protocol_disabled(&mut self, disabled: bool);
    fn update_sixel_host_support(&mut self, supported: bool);
    fn has_bell(&self) -> bool;
    fn consume_bell(&mut self);
    fn osc7_payload(&self) -> Option<&str>;
    fn set_bell_notification(&mut self, val: bool);
    fn get_bell_notification(&self) -> bool;
    fn set_notification_end(&mut self, notification_end: Option<NotificationEnd>);

    // Scrollback, search, and shell prompts
    fn get_line_number(&self) -> Option<usize>;
    fn scroll_position(&self) -> (usize, usize);
    fn clear_screen(&mut self);
    fn clear_scroll(&mut self);
    fn is_scrolled(&self) -> bool;
    fn scroll_up(&mut self, count: usize);
    fn scroll_down(&mut self, count: usize);
    fn update_search_term(&mut self, needle: &str);
    fn search_down(&mut self);
    fn search_up(&mut self);
    fn toggle_search_case_sensitivity(&mut self);
    fn toggle_search_whole_words(&mut self);
    fn toggle_search_wrap(&mut self);
    fn clear_search(&mut self);
    /// The word and the logical line the cursor is over, as text.
    ///
    /// Both queries walk the same grid rows for the context menu.
    fn text_around_position(&self, position: &Position) -> Option<TextAroundPosition>;
    fn scroll_to_previous_prompt(&mut self, client_id: ClientId);
    fn scroll_to_next_prompt(&mut self, client_id: ClientId);
    fn select_command_at_scroll_position(&mut self, client_id: ClientId);
    /// The output of the last completed OSC 133 command, also starting its feedback flash.
    fn copy_last_command_output(&mut self) -> Option<String>;
    fn clear_command_output_flash(&mut self);

    // Presentation and capture
    fn set_plugin_regex_highlights(
        &mut self,
        plugin_id: PluginId,
        highlights: Vec<RegexHighlight>,
        style: &Style,
    );
    fn clear_plugin_highlights(&mut self, plugin_id: PluginId);
    fn set_hover_position(&mut self, position: Option<Position>) -> bool;
    fn cached_hover_tooltip(&self) -> Option<String>;
    fn plugin_highlight_at(
        &self,
        position: &Position,
    ) -> Option<(PluginId, String, String, BTreeMap<String, String>)>;
    fn render_fake_cursor(
        &mut self,
        cursor_color: PaletteColor,
        text_color: PaletteColor,
    ) -> Option<CharacterChunk>;
    fn drain_fake_cursors(&mut self) -> Option<HashSet<(usize, usize)>>;
    fn web_terminal_frame(
        &mut self,
        after: Option<&str>,
    ) -> Option<yinz_types::agent::control::TerminalFrame>;
    fn clear_web_terminal_snapshot(&mut self);
    fn dump_screen_with_ansi(&self, full: bool, client_id: Option<ClientId>) -> String;
    fn update_agent_theme_mapping(&mut self, mapping: yinz_types::agent::theme::AgentThemeMapping);
    fn update_terminal_theme(&mut self, theme: Option<yinz_types::style::TerminalTheme>);
    /// The server-owned colors shared applications are told about (OSC 10/11/4).
    fn update_application_baseline(&mut self, baseline: yinz_types::style::ApplicationBaseline);
    fn set_pane_default_colors(&mut self, fg: Option<String>, bg: Option<String>);
    fn get_pane_default_colors(&self) -> (Option<String>, Option<String>);
}

pub trait Pane: TiledGridPane {
    fn terminal(&self) -> Option<&dyn TerminalPaneOps> {
        None
    }
    fn terminal_mut(&mut self) -> Option<&mut dyn TerminalPaneOps> {
        None
    }
    fn plugin_control(&self) -> Option<&dyn PluginPaneControl> {
        None
    }
    fn plugin_control_mut(&mut self) -> Option<&mut dyn PluginPaneControl> {
        None
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
    fn remove_client(&mut self, client_id: ClientId);
    /// Replay consumed content for a watcher without invalidating frame caches.
    fn prepare_content_replay(&mut self, client_id: ClientId);
    /// Redraw one client's contents and frame after its host display was lost.
    /// Shared terminal dirty state must be consumed before rendering other clients again.
    fn prepare_full_render_for_client(&mut self, client_id: ClientId);
    /// Preserve shared terminal replay metadata while another client's targeted pass runs.
    fn take_rendered_lines(&mut self) -> Option<yinz_terminal::output::RenderedLines> {
        None
    }
    fn restore_rendered_lines(&mut self, _rendered: yinz_terminal::output::RenderedLines) {}
    /// Replay one client's frame without dirtying its contents.
    fn invalidate_frame(&mut self, client_id: ClientId);
    /// Layout intent survives presentation changes such as a collapsed tab bar.
    fn configured_rows(&self) -> yinz_types::pane_size::Dimension {
        self.position_and_size().rows
    }
    fn set_configured_rows(&mut self, _rows: yinz_types::pane_size::Dimension) {}
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
    fn character_cell_size_possibly_changed(&mut self);
    /// Seed layout contents before the pane's process or plugin produces output.
    fn set_initial_contents(&mut self, contents: &str);
    /// Returns the cursor position relative to `render_geom` and whether it is visible.
    /// The position is returned unconditionally (as long as the cursor is within
    /// bounds) so that the host terminal can position the cursor for IME even
    /// when the app has hidden it. The bool is true when the cursor is visible.
    fn cursor_coordinates(&self, client_id: Option<ClientId>) -> Option<(usize, usize, bool)>;
    fn is_mid_frame(&self) -> bool {
        false
    }
    /// When this pane's synchronized-output lock lapses, if it holds one.
    fn sync_output_deadline(&self) -> Option<Instant> {
        None
    }
    /// Releases a synchronized-output lock that lapsed by `now`. Returns true if the pane now
    /// needs a render.
    fn expire_sync_output(&mut self, _now: Instant) -> bool {
        false
    }
    fn set_render_width(&mut self, width: Option<(usize, usize)>) -> bool;
    /// Returns whether the displayed rectangle changed.
    fn set_max_content_width(&mut self, width: usize) -> bool;
    fn should_render(&self) -> bool;
    fn set_should_render(&mut self, should_render: bool);
    fn set_selectable(&mut self, selectable: bool);
    fn render(&mut self, client_id: Option<ClientId>) -> Result<Option<RenderedPane>>;
    /// Return frame character chunks and optional raw terminal output, or `None` when unchanged.
    fn render_frame(
        &mut self,
        client_id: ClientId,
        frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>>;
    fn render_terminal_title(
        &mut self,
        input_mode: InputMode,
        session_name: Option<&SessionName>,
    ) -> String;
    fn update_name(&mut self, name: &str);
    fn dump_screen(&self, _full: bool, _client_id: Option<ClientId>) -> String {
        "".to_owned()
    }
    fn set_active_at(&mut self, instant: Instant);
    fn invalidate_all_frames(&mut self);
    fn set_content_offset(&mut self, offset: Offset);
    fn cursor_shape_csi(&self) -> String {
        "\u{1b}[0 q".to_string() // default to non blinking block
    }
    fn contains(&self, position: &Position) -> bool {
        self.render_geom().contains(position)
    }
    fn start_selection(&mut self, start: &Position, client_id: ClientId);
    fn update_selection(&mut self, position: &Position, client_id: ClientId);
    fn end_selection(&mut self, end: &Position, client_id: ClientId);
    fn reset_selection(&mut self, client_id: Option<ClientId>);
    fn start_text_selection(&mut self, client_id: ClientId);
    fn move_text_selection(&mut self, direction: Direction, client_id: ClientId);
    fn swap_text_selection_ends(&mut self, client_id: ClientId);
    fn remember_copied_text_selection(&mut self, client_id: ClientId);
    fn finish_text_selection(&mut self, client_id: ClientId);
    fn supports_mouse_selection(&self) -> bool {
        true
    }
    fn get_selected_text(&self, client_id: ClientId) -> Option<String>;
    /// Host defaults for chrome animation, excluding application and pane overrides.
    fn host_colors(&self) -> (PaletteColor, PaletteColor) {
        let palette = yinz_types::style::Palette::default();
        (palette.fg, palette.bg)
    }
    fn right_boundary_x_content_coords(&self) -> usize {
        self.get_content_x() + self.get_content_columns()
    }
    fn bottom_boundary_y_content_coords(&self) -> usize {
        self.get_content_y() + self.get_content_rows()
    }
    fn render_full_viewport(&mut self);
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
        let pos_line = usize::try_from(position.line()).ok()?;
        if !self.contains(position) {
            return None;
        }

        let pos_col = position.column();
        let geom = self.render_geom();
        let mid_col = geom.x + geom.cols.as_usize() / 2;
        let mid_line = geom.y + geom.rows.as_usize() / 2;

        let is_left = pos_col < mid_col;
        let is_top = pos_line < mid_line;

        Some(match (is_top, is_left) {
            (true, true) => PaneEdge::TopLeft,
            (true, false) => PaneEdge::TopRight,
            (false, true) => PaneEdge::BottomLeft,
            (false, false) => PaneEdge::BottomRight,
        })
    }
    fn intercept_mouse_event_on_frame(&mut self, event: &MouseEvent, client_id: ClientId) -> bool;
    fn store_pane_name(&mut self);
    fn load_pane_name(&mut self);
    fn set_borderless(&mut self, borderless: bool);
    /// Set border metadata and the content inset together, without a temporary frame resize.
    fn set_borderless_with_content_offset(&mut self, borderless: bool, offset: Offset) {
        self.set_borderless(borderless);
        self.set_content_offset(offset);
    }
    fn set_exclude_from_sync(&mut self, exclude_from_sync: bool);
    fn exclude_from_sync(&self) -> bool;
    fn add_red_pane_frame_color_override(
        &mut self,
        error_text: Option<String>,
        owner: Option<ClientId>,
    );
    fn add_highlight_pane_frame_color_override(
        &mut self,
        text: Option<String>,
        client_id: Option<ClientId>,
    );
    /// Put up a bell flash: the same highlight colour, but easing back out on its own rather than
    /// waiting to be cleared. Every client sees it, because a bell is a fact about the pane.
    fn flash_pane_frame_color_override(&mut self, now: Instant);
    /// Whether a fading frame override has reached the end of its fade. `true` when there is
    /// nothing fading, so a caller can stop asking for frames.
    fn frame_color_override_strength_is_settled(&self, now: Instant) -> bool;
    /// Take the override down. `None` clears whatever is there; a client only
    /// clears an override that was put up for that same client.
    fn clear_pane_frame_color_override(&mut self, client_id: Option<ClientId>);
    /// The override colour as `client_id` sees it - `None` when there is no
    /// override, or when the one there is drawn for somebody else.
    fn frame_color_override(&self, client_id: ClientId) -> Option<PaletteColor>;
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
    fn is_setup_wizard(&self) -> bool {
        matches!(self.invoked_with(), Some(Run::Plugin(plugin)) if plugin.is_setup_wizard())
    }
    fn is_builtin_tab_bar(&self) -> bool {
        matches!(self.invoked_with(), Some(Run::Plugin(plugin))
            if plugin.presentation().tab_bar)
    }
    fn set_title(&mut self, title: String);
    fn current_title(&self) -> String;
    fn set_subtitle(&mut self, subtitle: Option<String>);
    fn take_pane_tabs_transfer(&mut self) -> Option<PaneTabsTransfer>;
    fn set_pane_tabs_transfer(&mut self, transfer: PaneTabsTransfer);
    fn take_pane_tabs_layout(&mut self) -> Option<yinz_types::pane_tabs::PaneTabsLayout> {
        None
    }
    fn agent_title(&self) -> crate::ui::agent_title::AgentTitle {
        crate::ui::agent_title::AgentTitle {
            state: None,
            hook_flash: false,
        }
    }
    fn set_pane_tab_titles(&mut self, titles: Vec<crate::ui::agent_title::AgentTitle>);
    fn pane_tab_titles(&self) -> &[crate::ui::agent_title::AgentTitle];
    fn set_pane_tabs_views(
        &mut self,
        views: std::collections::HashMap<ClientId, crate::pane_tabs::PaneTabsView>,
    );
    fn pane_tabs_view(&self, client: ClientId) -> crate::pane_tabs::PaneTabsView;
    fn set_pane_tabs(&mut self, tabs: Option<yinz_types::pane_tabs::PaneTabsInfo>);
    fn pane_tabs(&self) -> Option<&yinz_types::pane_tabs::PaneTabsInfo>;
    fn subtitle(&self) -> Option<&str>;
    fn set_subtitle_enabled(&mut self, enabled: bool);
    fn subtitle_enabled(&self) -> bool;
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
                && !self.current_geom().is_collapsed_in_stack(),
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
    fn can_lend_title(&self) -> bool {
        self.has_explicit_title()
    }
    fn has_explicit_title(&self) -> bool {
        false
    }
    fn rename(&mut self, buf: Vec<u8>);
    fn update_theme(&mut self, theme: Styling);
    fn set_selection_options(&mut self, osc133_command_selection: bool, word_separators: &str);
    fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool);
    fn update_render_options(&mut self, styled_underlines: bool, osc8_hyperlinks: bool);
    fn update_kitty_host_support(&mut self, supported: KittyHostSupport);
    fn update_rounded_corners(&mut self, rounded_corners: bool);
    fn toggle_pinned(&mut self);
    fn set_pinned(&mut self, should_be_pinned: bool);
    fn reset_logical_position(&mut self);
    fn pane_contents(
        &self,
        client_id: Option<ClientId>,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents;
    fn pane_contents_with_ansi(
        &self,
        client_id: Option<ClientId>,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents;
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
        index_in_pane_group,
        ..Default::default()
    };

    if let Some(terminal) = pane.terminal() {
        (pane_info.default_fg, pane_info.default_bg) = terminal.get_pane_default_colors();
        pane_info.exited = terminal.exited();
        pane_info.exit_status = terminal.exit_status();
        pane_info.is_held = terminal.is_held();
        pane_info.restart_error = terminal.restart_error();
        pane_info.last_agent_session_id = terminal.last_agent_session_id().map(str::to_owned);
        pane_info.last_hook_at_ms = terminal.last_hook_at_ms();
        pane_info.agent = terminal.agent_info().cloned();
        pane_info.program_status = terminal.program_status();
    }
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
