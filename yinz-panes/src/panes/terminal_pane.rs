use crate::output::{CharacterChunk, KittyImageChunk, SixelImageChunk};
use crate::pane::{AdjustedInput, GuestChoiceIndicator, Pane, TerminalPaneIo, TextAroundPosition};
use crate::panes::kitty_graphics::{KittyApcInterceptor, KittyHostSupport, KittyImageStore, Piece};
use crate::panes::selection::Selection;
use crate::panes::sixel::SixelImageStore;
use crate::panes::FrameColorOverride;
use crate::panes::LinkHandler;
use crate::panes::{
    grid::{Grid, PendingNotification},
    nested_session_modal::GuestModalShortcuts,
    terminal_character::{render_first_run_banner, TerminalCharacter, EMPTY_TERMINAL_CHARACTER},
};
use crate::pty::VteBytes;
use crate::route::NotificationEnd;
use crate::ClientId;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::rc::Rc;
use std::time::{self, Instant};
use vte;
use yinz_runtime::errors::prelude::*;
use yinz_runtime::shared::make_terminal_title;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::animation::Easing;
use yinz_types::data::PaneContents;
use yinz_types::data::AGENT_PANE_CONTEXT;
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TerminalId;
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Offset;
use yinz_types::pane_size::PaneGeom;
use yinz_types::position::Position;
use yinz_types::style::{Palette, Style, Styling};
use yinz_types::{geometry::Direction, pane_size::SizeInPixels};
use {
    yinz_ipc::nested_session::NestedSessionMessage, yinz_types::data::InputMode,
    yinz_types::data::PaletteColor, yinz_types::data::RegexHighlight,
    yinz_types::input::layout::Run,
};

use crate::ui::pane_boundaries_frame::{FrameParams, PaneFrame};

pub const SELECTION_SCROLL_INTERVAL_MS: u64 = 10;

// Some keys in different formats but are used in the code
const LEFT_ARROW: &[u8] = &[27, 91, 68];
const RIGHT_ARROW: &[u8] = &[27, 91, 67];
const UP_ARROW: &[u8] = &[27, 91, 65];
const DOWN_ARROW: &[u8] = &[27, 91, 66];
const HOME_KEY: &[u8] = &[27, 91, 72];
const END_KEY: &[u8] = &[27, 91, 70];
pub const BRACKETED_PASTE_BEGIN: &[u8] = &[27, 91, 50, 48, 48, 126];
pub const BRACKETED_PASTE_END: &[u8] = &[27, 91, 50, 48, 49, 126];
const ENTER_NEWLINE: &[u8] = &[10];
const ESC: &[u8] = &[27];
const ENTER_CARRIAGE_RETURN: &[u8] = &[13];
const SPACE: &[u8] = &[32];
const CTRL_C: &[u8] = &[3]; // raw-byte fallback; the `KeyWithModifier` check comes first
const TERMINATING_STRING: &str = "\0";
const DELETE_KEY: &str = "\u{007F}";
const BACKSPACE_KEY: &str = "\u{0008}";

/// The ansi encoding of some keys
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AnsiEncoding {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

impl AnsiEncoding {
    /// Returns the ANSI representation of the entries.
    /// NOTE: There is an ANSI escape code (27) at the beginning of the string,
    ///       some editors will not show this
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Left => "OD".as_bytes(),
            Self::Right => "OC".as_bytes(),
            Self::Up => "OA".as_bytes(),
            Self::Down => "OB".as_bytes(),
            Self::Home => &[27, 79, 72], // ESC O H
            Self::End => &[27, 79, 70],  // ESC O F
        }
    }

    pub fn as_vec_bytes(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

pub use yinz_terminal::PaneId;

type IsFirstRun = bool;

pub struct TerminalPane {
    pub grid: Grid,
    pub pid: TerminalId,
    pub selectable: bool,
    pub geom: PaneGeom,
    pub geom_override: Option<PaneGeom>,
    pub active_at: Instant,
    pub style: Style,
    vte_parser: vte::Parser,
    #[cfg(test)]
    vte_batches: usize,
    selection_scrolled_at: time::Instant,
    content_offset: Offset,
    detected_agent: bool,
    agent_hook_flash: bool,
    max_content_width: usize,
    render_width: Option<(usize, usize)>,
    pane_tabs: Option<yinz_types::pane_tabs::PaneTabsInfo>,
    pane_tabs_views: HashMap<ClientId, crate::pane_tabs::PaneTabsView>,
    pane_tab_titles: Vec<crate::ui::agent_title::AgentTitle>,
    pane_tabs_transfer: Option<crate::pane_tabs::PaneTabsTransfer>,
    subtitle: Option<String>,
    subtitle_enabled: bool,
    pane_title: String,
    pane_name: String,
    prev_pane_name: String,
    remote_host: Option<String>,
    /// A decoration composed in front of the title, whatever the title is.
    ///
    /// Deliberately not `pane_name`: that slot has three writers already (the
    /// user's rename, the pane's own OSC title, and agent-service's idle
    /// summary), so anything put there is erased by the next one. A pane's
    /// checkout is a fact about the pane rather than a name for it, and the
    /// remote host beside this field is the same kind of fact.
    title_prefix: Option<String>,
    running_program: Option<String>,
    /// The agent running here, if the session knows of one. Starts as what the spawn path
    /// said; replaced whole by every report, and moved to `Exited`/`Busy` by hold and re-run.
    agent: Option<AgentInfo>,
    // None after acknowledgment; the agent record still prevents re-arming until waiting ends.
    waiting_pulse_started_at: Option<Instant>,
    waiting_pulse: Option<f32>,
    agent_input_revision: u64,
    agent_enter_revision: u64,
    agent_input_at: Option<Instant>,
    /// Whether this pane has ever held an agent record, however it got there.
    ///
    /// Sticky, and deliberately not derived from `agent`: a `SessionEnd` hook clears the
    /// record a moment before the agent's process exits, so a pane that gates `agent_exit` on
    /// the live record would disarm itself on every clean exit. The record is what the agent
    /// says now; this is that it was ever an agent at all.
    was_an_agent: bool,
    last_agent_kind: Option<String>,
    last_agent_cwd: Option<String>,
    last_agent_session_id: Option<String>,
    last_hook_at_ms: Option<i64>,
    pending_agent_escape_clients: HashSet<ClientId>,
    frame: HashMap<ClientId, PaneFrame>,
    borderless: bool,
    exclude_from_sync: bool,
    fake_cursor_locations: HashSet<(usize, usize)>, // (x, y) - these hold a record of previous fake cursors which we need to clear on render
    search_term: String,
    warp_pending: bool,
    is_held: Option<(Option<i32>, IsFirstRun, RunCommand)>, // a "held" pane means that its command has either exited and the pane is waiting for a
    // possible user instruction to be re-run, or that the command has not yet been run
    /// The pane's root process is stopped — `^Z` sent it `SIGTSTP`.
    ///
    /// Deliberately not a flavour of `is_held`: holding resets the grid and
    /// injects a banner, which would throw away exactly the output the user
    /// suspended in order to keep looking at. A suspended pane keeps everything;
    /// only the frame and the input handling change.
    suspended: bool,
    banner: Option<String>, // a banner to be rendered inside this TerminalPane, used for panes
    // held on startup and can possibly be used to display some errors
    pane_frame_color_override: Option<FrameColorOverride>,
    has_bell_notification: bool,
    invoked_with: Option<Run>,
    notification_end: Option<NotificationEnd>,
    /// `true` while a host-terminal forward initiated by this pane is
    /// outstanding. While set, processing of `pending_pty_input` is
    /// suspended so that the async host reply lands on the pane's
    /// stdin in the same stream position the original query occupied.
    /// Cleared by Tab when the reply (or 500 ms cache-fallback) lands.
    forward_paused: bool,
    nested_guest: bool,
    guest_modal: HashMap<ClientId, usize>,
    guest_choice_indicators: HashMap<ClientId, GuestChoiceIndicator>,
    guest_session_name: Option<SessionName>,
    guest_modal_shortcuts: GuestModalShortcuts,
    /// Unconsumed PTY bytes retained when a host query pauses parsing, replayed after its reply.
    pending_pty_input: VecDeque<u8>,
    kitty_interceptor: KittyApcInterceptor,
}

impl TerminalPaneIo for TerminalPane {
    fn drain_messages_to_pty(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.grid.pending_messages_to_pty)
    }
    fn drain_agent_theme_updates(&mut self) -> Vec<yinz_types::agent::theme::AgentThemeEvent> {
        std::mem::take(&mut self.grid.pending_agent_theme_updates)
    }
    fn drain_forwarded_queries(&mut self) -> Vec<crate::host_query::HostQuery> {
        std::mem::take(&mut self.grid.pending_forwarded_queries)
    }
    fn record_terminal_color_reply(&mut self, query: &crate::host_query::HostQuery, reply: &[u8]) {
        self.grid.record_terminal_color_reply(query, reply);
    }
    fn drain_nested_session_messages(&mut self) -> Vec<NestedSessionMessage> {
        std::mem::take(&mut self.grid.pending_nested_session_messages)
    }
    fn arm_forward_pause(&mut self) {
        self.forward_paused = true;
    }
    fn clear_forward_pause(&mut self) -> bool {
        let was_paused = self.forward_paused;
        self.forward_paused = false;
        was_paused
    }
    fn drain_pending_pty_input(&mut self) -> Vec<u8> {
        self.pending_pty_input.drain(..).collect()
    }
    fn is_forward_paused(&self) -> bool {
        self.forward_paused
    }
    fn push_color_palette_dsr(&mut self, mode: yinz_types::style::HostTerminalThemeMode) {
        self.grid.push_color_palette_dsr(mode);
    }
    fn drain_clipboard_update(&mut self) -> Option<String> {
        self.grid.pending_clipboard_update.take()
    }
    fn drain_desktop_notifications(&mut self) -> Vec<PendingNotification> {
        std::mem::take(&mut self.grid.pending_desktop_notifications)
    }
    fn drain_osc7_cwd(&mut self) -> Option<std::path::PathBuf> {
        self.grid.pending_osc7_cwd.take()
    }
}

impl Pane for TerminalPane {
    fn terminal_io_mut(&mut self) -> Option<&mut dyn TerminalPaneIo> {
        Some(self)
    }
    fn reset_size_and_position_override(&mut self) {
        self.geom_override = None;
        self.reflow_lines();
    }
    fn set_geom(&mut self, position_and_size: PaneGeom) {
        let is_pinned = self.geom.is_pinned;
        self.geom = position_and_size;
        self.geom.is_pinned = is_pinned;
        self.reflow_lines();
        self.render_full_viewport();
        // A pane whose geometry changed is repainted whether or not the caller goes on to
        // relayout the tab, as `PluginPane::set_geom` already did. The stack paths used
        // to force-render every pane in the tab to make up for this.
        self.set_should_render(true);
    }
    fn set_geom_override(&mut self, pane_geom: PaneGeom) {
        self.geom_override = Some(pane_geom);
        self.reflow_lines();
        self.set_should_render(true);
    }
    fn set_layout(
        &mut self,
        mut geom: PaneGeom,
        geom_override: Option<PaneGeom>,
        borderless: bool,
        content_offset: Offset,
    ) {
        geom.is_pinned = self.geom.is_pinned;
        self.geom = geom;
        self.geom_override = geom_override;
        self.borderless = borderless;
        self.content_offset = content_offset;
        self.reflow_lines();
        self.render_full_viewport();
    }
    fn character_cell_size_possibly_changed(&mut self) {
        self.grid.character_cell_size_possibly_changed();
        self.render_full_viewport();
        self.set_should_render(true);
    }
    fn set_initial_contents(&mut self, contents: &str) {
        self.handle_pty_bytes(contents.as_bytes().to_vec());
        self.handle_pty_bytes(b"\n\r".to_vec());
    }
    fn handle_pty_bytes(&mut self, bytes: VteBytes) {
        self.grid.should_render = true;
        if self.forward_paused {
            // A host-forward initiated by this pane is outstanding.
            // Buffer the bytes; Tab drains them on resume.
            self.pending_pty_input.extend(bytes);
            return;
        }
        let mut forwarded: Cow<'_, [u8]> = Cow::Borrowed(&[]);
        let mut pieces = self.kitty_interceptor.split(&bytes);
        while let Some(piece) = pieces.next() {
            match piece {
                Piece::Run(run) if forwarded.is_empty() => forwarded = Cow::Borrowed(run),
                Piece::Run(run) => forwarded.to_mut().extend_from_slice(run),
                Piece::Forward(fwd) => forwarded.to_mut().extend_from_slice(fwd.as_slice()),
                Piece::CaptureStarted => {
                    // Only an actual Kitty APC needs a flush. Ordinary escape sequences stay
                    // batched together, even when the same read also contains image commands.
                    if !forwarded.is_empty() {
                        #[cfg(test)]
                        {
                            self.vte_batches += 1;
                        }
                        let consumed = self
                            .vte_parser
                            .advance_until_terminated(&mut self.grid, &forwarded);
                        if !self.grid.pending_forwarded_queries.is_empty() {
                            self.pending_pty_input.extend(&forwarded[consumed..]);
                            // The prefix may have spanned reads, so reconstruct it explicitly.
                            self.pending_pty_input.extend(b"\x1b_G");
                            self.pending_pty_input.extend(&bytes[pieces.position()..]);
                            pieces.reset_interceptor();
                            return;
                        }
                        forwarded = Cow::Borrowed(&[]);
                    }
                },
                Piece::Captured(cmd) => {
                    self.grid.handle_kitty_apc(&cmd);
                    if !self.grid.pending_forwarded_queries.is_empty() {
                        // Grid produced a forward. Stop feeding; queue the
                        // un-fed remainder so Tab can replay it after the
                        // reply.
                        self.pending_pty_input.extend(&bytes[pieces.position()..]);
                        return;
                    }
                },
            }
        }
        if !forwarded.is_empty() {
            #[cfg(test)]
            {
                self.vte_batches += 1;
            }
            let consumed = self
                .vte_parser
                .advance_until_terminated(&mut self.grid, &forwarded);
            if !self.grid.pending_forwarded_queries.is_empty() {
                self.pending_pty_input.extend(&forwarded[consumed..]);
                self.pending_pty_input
                    .extend(self.kitty_interceptor.take_pending_prefix());
            }
        }
    }
    fn cursor_coordinates(&self, client_id: Option<ClientId>) -> Option<(usize, usize, bool)> {
        // (x, y, is_visible)
        if let Some(client_id) = client_id
            && self.guest_modal.contains_key(&client_id)
        {
            return None;
        }
        if self.pane_tabs.as_ref().is_some_and(|tabs| {
            tabs.tabs[tabs.active].pane_id() != Some(PaneId::Terminal(self.pid))
        }) {
            return None;
        }
        if self.get_content_rows() < 1 || self.get_content_columns() < 1 {
            // do not render cursor if there's no room for it
            return None;
        }
        let Offset { top, left, .. } = self.content_offset;
        let top = top + self.content_header_rows();
        self.grid
            .cursor_coordinates()
            .map(|(x, y, is_visible)| (x + left, y + top, is_visible))
    }
    fn is_mid_frame(&self) -> bool {
        self.grid.is_mid_frame()
    }
    fn adjust_input_to_terminal(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
        client_id: Option<ClientId>,
    ) -> Option<AdjustedInput> {
        // there are some cases in which the terminal state means that input sent to it
        // needs to be adjusted.
        // here we match against those cases - if need be, we adjust the input and if not
        // we send back the original input

        self.reset_selection(client_id);
        if !self.grid.bracketed_paste_mode {
            // Yinz itself operates in bracketed paste mode, so the terminal sends these
            // instructions (bracketed paste start and bracketed paste end respectively)
            // when pasting input. We only need to make sure not to send them to terminal
            // panes who do not work in this mode
            match raw_input_bytes.as_slice() {
                BRACKETED_PASTE_BEGIN | BRACKETED_PASTE_END => {
                    return Some(AdjustedInput::WriteBytesToTerminal(vec![]))
                },
                _ => {},
            }
        }

        if let Some(selection) = client_id.and_then(|c| self.guest_modal.get(&c).copied()) {
            let client_id = client_id.expect("guest modal selection requires a client id");
            let is_up = key_with_modifier
                .as_ref()
                .map(|k| {
                    k.is_key_without_modifier(BareKey::Up)
                        || k.is_key_without_modifier(BareKey::Char('k'))
                })
                .unwrap_or(false)
                || raw_input_bytes.as_slice() == UP_ARROW;
            let is_down = key_with_modifier
                .as_ref()
                .map(|k| {
                    k.is_key_without_modifier(BareKey::Down)
                        || k.is_key_without_modifier(BareKey::Char('j'))
                })
                .unwrap_or(false)
                || raw_input_bytes.as_slice() == DOWN_ARROW;
            let is_enter = key_with_modifier
                .as_ref()
                .map(|k| k.is_key_without_modifier(BareKey::Enter))
                .unwrap_or(false)
                || matches!(
                    raw_input_bytes.as_slice(),
                    ENTER_CARRIAGE_RETURN | ENTER_NEWLINE
                );
            let is_esc = key_with_modifier
                .as_ref()
                .map(|k| k.is_key_without_modifier(BareKey::Esc))
                .unwrap_or(false)
                || raw_input_bytes.as_slice() == ESC;
            let digit = key_with_modifier
                .as_ref()
                .and_then(|k| match k.bare_key {
                    BareKey::Char(c @ '1'..='2') if k.key_modifiers.is_empty() => Some(c),
                    _ => None,
                })
                .or(match raw_input_bytes.as_slice() {
                    b"1" => Some('1'),
                    b"2" => Some('2'),
                    _ => None,
                });
            // up or down: the modal has two entries, so either key flips the selection
            if is_up || is_down {
                self.guest_modal.insert(client_id, (selection + 1) % 2);
                self.set_should_render(true);
                Some(AdjustedInput::GuestModalSelectionChanged)
            } else if let Some(digit) = digit {
                match digit {
                    '1' => Some(AdjustedInput::GuestModalZoom),
                    _ => Some(AdjustedInput::GuestModalDescend),
                }
            } else if is_enter {
                match selection {
                    0 => Some(AdjustedInput::GuestModalZoom),
                    _ => Some(AdjustedInput::GuestModalDescend),
                }
            } else if is_esc {
                Some(AdjustedInput::GuestModalDescend)
            } else {
                None
            }
        } else if self.suspended {
            // Nothing typed here can reach the child: it is stopped, so the pty
            // would only buffer it. Swallow everything except the two keys that
            // mean something out here.
            let is_enter = key_with_modifier
                .as_ref()
                .map(|k| k.is_key_without_modifier(BareKey::Enter))
                .unwrap_or(false)
                || matches!(
                    raw_input_bytes.as_slice(),
                    ENTER_CARRIAGE_RETURN | ENTER_NEWLINE | SPACE
                );
            let is_ctrl_c = key_with_modifier
                .as_ref()
                .map(|k| k.is_key_with_ctrl_modifier(BareKey::Char('c')))
                .unwrap_or(false)
                || raw_input_bytes.as_slice() == CTRL_C;
            if is_enter {
                Some(AdjustedInput::ResumeSuspendedProcessInThisPane)
            } else if is_ctrl_c {
                Some(AdjustedInput::CloseThisPane)
            } else {
                None
            }
        } else if self.is_held.is_some() {
            if key_with_modifier
                .as_ref()
                .map(|k| k.is_key_without_modifier(BareKey::Enter))
                .unwrap_or(false)
            {
                self.handle_held_run()
            } else if key_with_modifier
                .as_ref()
                .map(|k| k.is_key_without_modifier(BareKey::Esc))
                .unwrap_or(false)
            {
                self.handle_held_drop_to_shell()
            } else if key_with_modifier
                .as_ref()
                .map(|k| k.is_key_with_ctrl_modifier(BareKey::Char('c')))
                .unwrap_or(false)
            {
                Some(AdjustedInput::CloseThisPane)
            } else {
                match raw_input_bytes.as_slice() {
                    ENTER_CARRIAGE_RETURN | ENTER_NEWLINE | SPACE => self.handle_held_run(),
                    ESC => self.handle_held_drop_to_shell(),
                    CTRL_C => Some(AdjustedInput::CloseThisPane),
                    _ => None,
                }
            }
        } else {
            if self.grid.supports_kitty_keyboard_protocol {
                self.adjust_input_to_terminal_with_kitty_keyboard_protocol(
                    key_with_modifier,
                    raw_input_bytes,
                    raw_input_bytes_are_kitty,
                )
            } else {
                self.adjust_input_to_terminal_without_kitty_keyboard_protocol(
                    key_with_modifier,
                    raw_input_bytes,
                    raw_input_bytes_are_kitty,
                )
            }
        }
    }
    fn position_and_size(&self) -> PaneGeom {
        self.geom
    }
    fn geom_override(&self) -> Option<PaneGeom> {
        self.geom_override
    }
    fn remove_client(&mut self, client_id: ClientId) {
        self.frame.remove(&client_id);
        self.guest_modal.remove(&client_id);
        self.guest_choice_indicators.remove(&client_id);
        self.pending_agent_escape_clients.remove(&client_id);
        self.clear_pane_frame_color_override(Some(client_id));
    }
    fn prepare_content_replay(&mut self, _client_id: ClientId) {
        self.grid.should_render = true;
        self.grid.replay_last_render();
    }
    fn invalidate_frame(&mut self, client_id: ClientId) {
        self.frame.remove(&client_id);
    }
    fn should_render(&self) -> bool {
        self.grid.should_render
    }
    fn set_should_render(&mut self, should_render: bool) {
        self.grid.should_render = should_render;
    }
    fn render_full_viewport(&mut self) {
        // this marks the pane for a full re-render, rather than just rendering the
        // diff as it usually does with the OutputBuffer
        self.frame.clear();
        self.grid.render_full_viewport();
    }
    fn selectable(&self) -> bool {
        self.selectable
    }
    fn set_selectable(&mut self, selectable: bool) {
        self.selectable = selectable;
    }
    fn update_agent_theme_mapping(&mut self, mapping: yinz_types::agent::theme::AgentThemeMapping) {
        self.style.agent_theme_mapping = mapping;
        self.grid.set_agent_theme_mapping(mapping);
    }
    fn update_terminal_theme(&mut self, theme: Option<yinz_types::style::TerminalTheme>) {
        self.style.terminal = theme;
        self.grid.update_terminal_theme(theme);
    }
    fn host_colors(&self) -> (PaletteColor, PaletteColor) {
        self.grid.host_colors()
    }
    fn set_pane_default_colors(&mut self, fg: Option<String>, bg: Option<String>) {
        self.grid.set_pane_default_colors(fg, bg);
        self.set_should_render(true);
    }
    fn get_pane_default_colors(&self) -> (Option<String>, Option<String>) {
        self.grid.get_pane_default_color_strings()
    }
    fn render(
        &mut self,
        _client_id: Option<ClientId>,
    ) -> Result<
        Option<(
            Vec<CharacterChunk>,
            Option<String>,
            Vec<SixelImageChunk>,
            Vec<KittyImageChunk>,
        )>,
    > {
        if self.should_render() {
            let content_x = self.get_content_x();
            let content_y = self.get_content_y();
            let rows = self.get_content_rows();
            let columns = self.get_content_columns();
            if rows < 1 || columns < 1 {
                return Ok(None);
            }
            match self.grid.render(content_x, content_y, &self.style) {
                Ok(rendered_assets) => {
                    let mut rendered_assets = rendered_assets;
                    if let Some(tabs) = &self.pane_tabs
                        && tabs.tabs[tabs.active].pane_id() != Some(PaneId::Terminal(self.pid))
                    {
                        let message = match &tabs.tabs[tabs.active].state {
                            yinz_types::pane_tabs::PaneTabLoadState::Failed(error) => {
                                format!("{error} (r: retry; select Conversation to return)")
                            },
                            _ => "Loading Overview… Select Conversation to return.".into(),
                        };
                        let chunks = &mut rendered_assets
                            .get_or_insert_with(|| (vec![], None, vec![], vec![]))
                            .0;
                        chunks.clear();
                        for row in 0..rows {
                            let mut characters: Vec<_> = if row == 0 {
                                yinz_types::text::clean_string_from_control_and_linebreak(&message)
                                    .chars()
                                    .take(columns)
                                    .map(TerminalCharacter::new)
                                    .collect()
                            } else {
                                vec![]
                            };
                            characters.resize(columns, EMPTY_TERMINAL_CHARACTER);
                            chunks.push(CharacterChunk::new(
                                characters,
                                content_x,
                                content_y + row,
                            ));
                        }
                    }
                    self.set_should_render(false);
                    Ok(rendered_assets)
                },
                e => e,
            }
        } else {
            Ok(None)
        }
    }
    fn set_pending_agent_escape_clients(&mut self, clients: HashSet<ClientId>) {
        self.pending_agent_escape_clients = clients;
    }

    fn render_frame(
        &mut self,
        client_id: ClientId,
        mut frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>> {
        let subtitle = (self.subtitle_rows() > 0)
            .then(|| self.subtitle.clone())
            .flatten()
            .map(|text| {
                (
                    text,
                    self.get_content_y() - (self.content_header_rows() - self.subtitle_rows()) - 1,
                )
            });
        let err_context = || format!("failed to render frame for client {client_id}");
        frame_params.omit_title = frame_params.omit_title
            && !(input_mode == InputMode::RenamePane && frame_params.is_main_client);
        let normal_title = if self.pane_name.is_empty()
            && input_mode == InputMode::RenamePane
            && frame_params.is_main_client
        {
            String::from("Enter name...")
        } else if input_mode == InputMode::EnterSearch
            && frame_params.is_main_client
            && self.search_term.is_empty()
        {
            String::from("Enter search...")
        } else if (input_mode == InputMode::EnterSearch || input_mode == InputMode::Search)
            && !self.search_term.is_empty()
        {
            let mut modifier_text = String::new();
            if self.grid.search_results.has_modifiers_set() {
                let mut modifiers = Vec::new();
                modifier_text.push_str(" [");
                if self.grid.search_results.case_insensitive {
                    modifiers.push("c")
                }
                if self.grid.search_results.whole_word_only {
                    modifiers.push("o")
                }
                if self.grid.search_results.wrap_search {
                    modifiers.push("w")
                }
                modifier_text.push_str(&modifiers.join(", "));
                modifier_text.push(']');
            }
            format!("SEARCHING: {}{}", self.search_term, modifier_text)
        } else {
            self.framed_title()
        };
        let frame_color_override = self
            .pane_frame_color_override
            .as_ref()
            .filter(|override_| override_.applies_to(client_id));
        let mut pane_title = if frame_params.blank_title {
            String::new()
        } else if let Some(text_color_override) =
            frame_color_override.and_then(|override_| override_.text.as_ref())
        {
            text_color_override.into()
        } else {
            self.title_with_bell_indicator(normal_title)
        };

        if !frame_params.blank_title && self.pending_agent_escape_clients.contains(&client_id) {
            pane_title = format!("Esc again {}", pane_title);
        }

        if !frame_params.blank_title {
            pane_title = self.title_with_agent_indicator(pane_title, frame_params.same_worktree);
        }

        let agent_title = if frame_params.blank_title {
            Default::default()
        } else {
            self.agent_title()
        };
        let frame_geom = frame_params
            .frame_geom_override
            .unwrap_or_else(|| self.render_geom());
        let is_pinned = frame_geom.is_pinned;
        let mut frame = PaneFrame::new(
            frame_geom.into(),
            self.grid.scrollback_position_and_length(),
            pane_title,
            frame_params,
        )
        .is_pinned(is_pinned);
        frame.subtitle = subtitle;
        frame.agent_title = agent_title;
        if self.suspended {
            frame.indicate_suspended();
        }
        if let Some((exit_status, is_first_run, _run_command)) = &self.is_held {
            if *is_first_run {
                frame.indicate_first_run();
            } else {
                frame.add_exit_status(exit_status.as_ref().copied());
            }
        }

        if self.borderless || self.frame.get(&client_id) == Some(&frame) {
            return Ok(None);
        }
        let frame_output = frame.render().with_context(err_context)?;
        self.frame.insert(client_id, frame);
        Ok(Some(frame_output))
    }
    fn render_fake_cursor(
        &mut self,
        cursor_color: PaletteColor,
        text_color: PaletteColor,
    ) -> Option<String> {
        let mut vte_output = None;
        if let Some((cursor_x, cursor_y, true)) = self.cursor_coordinates() {
            // the character at the cell we are about to draw on, which is not necessarily the
            // one under the pty cursor (a keyboard selection moves the cursor to its live end)
            let mut character_under_cursor = self
                .grid
                .get_character_at(cursor_x, cursor_y)
                .unwrap_or(EMPTY_TERMINAL_CHARACTER);
            character_under_cursor.styles.update(|styles| {
                styles.background = Some(cursor_color.into());
                styles.foreground = Some(text_color.into());
            });
            // we keep track of these so that we can clear them up later (see render function)
            self.fake_cursor_locations.insert((cursor_y, cursor_x));
            let mut fake_cursor = format!(
                "\u{1b}[{};{}H\u{1b}[m{}",           // goto row column and clear styles
                self.get_content_y() + cursor_y + 1, // + 1 because goto is 1 indexed
                self.get_content_x() + cursor_x + 1,
                character_under_cursor.styles,
            );
            fake_cursor.push(character_under_cursor.character);
            vte_output = Some(fake_cursor);
        }
        vte_output
    }
    fn render_terminal_title(
        &mut self,
        input_mode: InputMode,
        session_name: Option<&SessionName>,
    ) -> String {
        let pane_title = if self.pane_name.is_empty() && input_mode == InputMode::RenamePane {
            "Enter name..."
        } else if self.pane_name.is_empty() {
            self.grid.title.as_deref().unwrap_or("")
        } else {
            &self.pane_name
        };
        make_terminal_title(pane_title, session_name)
    }
    fn update_name(&mut self, name: &str) {
        match name {
            TERMINATING_STRING => {
                self.pane_name = String::new();
            },
            DELETE_KEY | BACKSPACE_KEY => {
                self.pane_name.pop();
            },
            c => {
                self.pane_name.push_str(c);
            },
        }
        self.set_should_render(true);
    }
    fn pid(&self) -> PaneId {
        PaneId::Terminal(self.pid)
    }
    fn reduce_height(&mut self, percent: f64) {
        if let Some(p) = self.geom.rows.as_percent() {
            self.geom.rows.set_percent(p - percent);
            self.set_should_render(true);
        }
    }
    fn increase_height(&mut self, percent: f64) {
        if let Some(p) = self.geom.rows.as_percent() {
            self.geom.rows.set_percent(p + percent);
            self.set_should_render(true);
        }
    }
    fn reduce_width(&mut self, percent: f64) {
        if let Some(p) = self.geom.cols.as_percent() {
            self.geom.cols.set_percent(p - percent);
            self.set_should_render(true);
        }
    }
    fn increase_width(&mut self, percent: f64) {
        if let Some(p) = self.geom.cols.as_percent() {
            self.geom.cols.set_percent(p + percent);
            self.set_should_render(true);
        }
    }
    fn push_down(&mut self, count: usize) {
        self.geom.y += count;
        self.reflow_lines();
    }
    fn push_right(&mut self, count: usize) {
        self.geom.x += count;
        self.reflow_lines();
    }
    fn pull_left(&mut self, count: usize) {
        self.geom.x -= count;
        self.reflow_lines();
    }
    fn pull_up(&mut self, count: usize) {
        self.geom.y -= count;
        self.reflow_lines();
    }
    fn dump_screen(&self, full: bool, _client_id: Option<ClientId>) -> String {
        self.grid.dump_screen(full)
    }
    fn dump_screen_with_ansi(&self, full: bool, _client_id: Option<ClientId>) -> String {
        self.grid.dump_screen_with_ansi(full)
    }
    fn clear_screen(&mut self) {
        self.grid.clear_screen()
    }
    fn scroll_up(&mut self, count: usize, _client_id: Option<ClientId>) {
        self.grid.move_viewport_up(count);
        self.set_should_render(true);
    }
    fn scroll_down(&mut self, count: usize, _client_id: Option<ClientId>) {
        self.grid.move_viewport_down(count);
        self.set_should_render(true);
    }
    fn scroll_to_previous_prompt(&mut self, _client_id: ClientId) {
        if self.grid.scroll_to_previous_prompt() {
            self.set_should_render(true);
        }
    }
    fn scroll_to_next_prompt(&mut self, _client_id: ClientId) {
        if self.grid.scroll_to_next_prompt() {
            self.set_should_render(true);
        }
    }
    fn select_command_at_scroll_position(&mut self, _client_id: ClientId) {
        if self.grid.select_command_at_scroll_position() {
            self.set_should_render(true);
        }
    }
    fn copy_last_command_output(&mut self) -> Option<String> {
        let (output, start, end) = self.grid.last_completed_command_output()?;
        self.grid.set_command_output_flash(start, end);
        self.set_should_render(true);
        Some(output)
    }
    fn clear_command_output_flash(&mut self) {
        if self.grid.clear_command_output_flash() {
            self.set_should_render(true);
        }
    }
    fn clear_scroll(&mut self) {
        if self.grid.is_scrolled {
            self.grid.reset_viewport();
            self.set_should_render(true);
        }
    }
    fn is_scrolled(&self) -> bool {
        self.grid.is_scrolled
    }

    fn active_at(&self) -> Instant {
        self.active_at
    }

    fn set_active_at(&mut self, time: Instant) {
        self.active_at = time;
    }
    fn cursor_shape_csi(&self) -> String {
        self.grid.cursor_shape().get_csi_str().to_string()
    }

    fn is_nested_guest(&self) -> bool {
        self.nested_guest
    }

    fn set_is_nested_guest(&mut self, is_nested_guest: bool) {
        self.nested_guest = is_nested_guest;
    }

    fn set_guest_modal(&mut self, client_ids: &[ClientId]) {
        for client_id in client_ids {
            self.guest_modal.insert(*client_id, 0);
        }
        self.set_should_render(true);
    }

    fn clear_guest_modal(&mut self, client_id: ClientId) {
        if self.guest_modal.remove(&client_id).is_some() {
            self.render_full_viewport();
            self.set_should_render(true);
        }
    }

    fn clear_all_guest_modals(&mut self) {
        if !self.guest_modal.is_empty() {
            self.guest_modal.clear();
            self.render_full_viewport();
            self.set_should_render(true);
        }
    }

    fn guest_modal_selection(&self, client_id: ClientId) -> Option<usize> {
        self.guest_modal.get(&client_id).copied()
    }

    fn has_guest_modal_for_any_client(&self) -> bool {
        !self.guest_modal.is_empty()
    }

    fn set_guest_choice_indicator(
        &mut self,
        client_id: ClientId,
        indicator: Option<GuestChoiceIndicator>,
    ) {
        let previous = self.guest_choice_indicators.get(&client_id).copied();
        if previous == indicator {
            return;
        }
        match indicator {
            Some(indicator) => {
                self.guest_choice_indicators.insert(client_id, indicator);
            },
            None => {
                self.guest_choice_indicators.remove(&client_id);
            },
        }
        self.set_should_render(true);
    }

    fn guest_choice_indicator(&self, client_id: ClientId) -> Option<GuestChoiceIndicator> {
        self.guest_choice_indicators.get(&client_id).copied()
    }

    fn clear_all_guest_choice_indicators(&mut self) {
        if !self.guest_choice_indicators.is_empty() {
            self.guest_choice_indicators.clear();
            self.set_should_render(true);
        }
    }

    fn set_guest_session_name(&mut self, session_name: Option<SessionName>) {
        self.guest_session_name = session_name;
    }

    fn guest_session_name(&self) -> Option<SessionName> {
        self.guest_session_name.clone()
    }

    fn set_guest_modal_shortcuts(&mut self, shortcuts: GuestModalShortcuts) {
        self.guest_modal_shortcuts = shortcuts;
    }

    fn guest_modal_shortcuts(&self) -> GuestModalShortcuts {
        self.guest_modal_shortcuts.clone()
    }

    fn set_selection_options(&mut self, osc133_command_selection: bool, word_separators: &str) {
        self.grid
            .set_selection_options(osc133_command_selection, word_separators);
    }

    fn start_selection(&mut self, start: &Position, _client_id: ClientId) {
        self.grid.start_selection(start);
        self.set_should_render(true);
    }

    fn update_selection(&mut self, to: &Position, _client_id: ClientId) {
        let should_scroll = self.selection_scrolled_at.elapsed()
            >= time::Duration::from_millis(SELECTION_SCROLL_INTERVAL_MS);
        let cursor_at_the_bottom = to.line.get() < 0 && should_scroll;
        let cursor_at_the_top = to.line.get() as usize >= self.grid.height && should_scroll;
        let cursor_in_the_middle =
            to.line.get() >= 0 && (to.line.get() as usize) < self.grid.height;

        // The further past the edge the pointer is, the more lines each step scrolls, up
        // to a cap: one line per interval made selecting a long scrollback a crawl.
        const MAX_LINES_PER_STEP: usize = 10;
        if cursor_at_the_bottom {
            let overshoot = to.line.get().unsigned_abs();
            for _ in 0..overshoot.clamp(1, MAX_LINES_PER_STEP) {
                // Past the end of the scrollback this is a no-op; the bool it returns is
                // about search results, not about whether it scrolled.
                self.grid.scroll_up_one_line();
            }
            self.selection_scrolled_at = time::Instant::now();
            self.set_should_render(true);
        } else if cursor_at_the_top {
            let overshoot = (to.line.get() as usize).saturating_sub(self.grid.height) + 1;
            for _ in 0..overshoot.clamp(1, MAX_LINES_PER_STEP) {
                self.grid.scroll_down_one_line();
            }
            self.selection_scrolled_at = time::Instant::now();
            self.set_should_render(true);
        } else if cursor_in_the_middle {
            // here we'll only render if the selection was updated, and that'll be handled by the
            // grid
            self.grid.update_selection(to);
        }
    }

    fn end_selection(&mut self, end: &Position, _client_id: ClientId) {
        self.grid.end_selection(end);
        self.set_should_render(true);
    }

    fn reset_selection(&mut self, _client_id: Option<ClientId>) {
        self.grid.reset_selection();
    }

    fn start_text_selection(&mut self, _client_id: ClientId) {
        let position = self
            .grid
            .selection_cursor_coordinates()
            .map(|(column, line)| Position::new(line as i32, column as u16))
            .unwrap_or_else(|| Position::new(self.grid.height.saturating_sub(1) as i32, 0));
        self.grid.start_text_selection(position);
        self.set_should_render(true);
    }

    fn move_text_selection(&mut self, direction: Direction, _client_id: ClientId) {
        self.grid.move_text_selection(direction);
        self.set_should_render(true);
    }

    fn swap_text_selection_ends(&mut self, _client_id: ClientId) {
        self.grid.swap_text_selection_ends();
        self.set_should_render(true);
    }

    fn remember_copied_text_selection(&mut self, _client_id: ClientId) {
        self.grid.remember_copied_text_selection();
    }

    fn finish_text_selection(&mut self, _client_id: ClientId) {
        self.grid.finish_text_selection();
        self.set_should_render(true);
    }

    fn get_selected_text(&self, _client_id: ClientId) -> Option<String> {
        self.grid.get_selected_text()
    }

    fn set_frame(&mut self, _frame: bool) {
        self.frame.clear();
    }

    fn render_width(&self) -> Option<(usize, usize)> {
        self.render_width
    }
    fn set_render_width(&mut self, width: Option<(usize, usize)>) -> bool {
        let before = self.render_geom();
        self.render_width = width;
        if before == self.render_geom() {
            return false;
        }
        self.reflow_lines();
        self.render_full_viewport();
        true
    }
    fn is_detected_agent(&self) -> bool {
        self.detected_agent
    }
    fn set_detected_agent(&mut self, detected: bool) {
        self.detected_agent = detected;
    }
    fn max_content_width(&self) -> usize {
        self.max_content_width
    }
    fn set_max_content_width(&mut self, width: usize) -> bool {
        let before = self.render_geom();
        self.max_content_width = width;
        if before == self.render_geom() {
            return false;
        }
        self.reflow_lines();
        self.render_full_viewport();
        true
    }
    fn set_content_offset(&mut self, offset: Offset) {
        self.content_offset = offset;
        self.reflow_lines();
    }

    fn set_subtitle(&mut self, subtitle: Option<String>) {
        let subtitle = subtitle
            .map(|text| yinz_types::text::clean_string_from_control_and_linebreak(&text))
            .filter(|text| !text.is_empty());
        if self.subtitle != subtitle {
            let rows = self.subtitle_rows();
            self.subtitle = subtitle;
            if rows != self.subtitle_rows() {
                self.reflow_lines();
            }
            self.set_should_render(true);
        }
    }
    fn subtitle(&self) -> Option<&str> {
        self.subtitle.as_deref()
    }
    fn set_subtitle_enabled(&mut self, enabled: bool) {
        self.subtitle_enabled = enabled;
    }
    fn subtitle_enabled(&self) -> bool {
        self.subtitle_enabled
    }

    fn content_header_rows(&self) -> usize {
        self.subtitle_rows()
            + usize::from(
                self.pane_tabs.is_some()
                    && self.rows()
                        > self.content_offset.top
                            + self.content_offset.bottom
                            + self.subtitle_rows(),
            )
    }
    fn take_pane_tabs_transfer(&mut self) -> Option<crate::pane_tabs::PaneTabsTransfer> {
        self.pane_tabs_transfer.take()
    }
    fn set_pane_tabs_transfer(&mut self, transfer: crate::pane_tabs::PaneTabsTransfer) {
        self.pane_tabs_transfer = Some(transfer);
    }
    fn set_pane_tab_titles(&mut self, titles: Vec<crate::ui::agent_title::AgentTitle>) {
        if self.pane_tab_titles != titles {
            self.pane_tab_titles = titles;
            self.set_should_render(true);
        }
    }
    fn pane_tab_titles(&self) -> &[crate::ui::agent_title::AgentTitle] {
        &self.pane_tab_titles
    }
    fn set_pane_tabs_views(&mut self, views: HashMap<ClientId, crate::pane_tabs::PaneTabsView>) {
        if self.pane_tabs_views != views {
            self.pane_tabs_views = views;
            self.set_should_render(true);
        }
    }
    fn pane_tabs_view(&self, client: ClientId) -> crate::pane_tabs::PaneTabsView {
        self.pane_tabs_views
            .get(&client)
            .copied()
            .unwrap_or_default()
    }
    fn set_pane_tabs(&mut self, tabs: Option<yinz_types::pane_tabs::PaneTabsInfo>) {
        if self.pane_tabs != tabs {
            let had_bar = self.pane_tabs.is_some();
            self.pane_tabs = tabs;
            if had_bar != self.pane_tabs.is_some() {
                self.reflow_lines();
            }
            self.set_should_render(true);
        }
    }
    fn pane_tabs(&self) -> Option<&yinz_types::pane_tabs::PaneTabsInfo> {
        self.pane_tabs.as_ref()
    }
    fn get_content_offset(&self) -> Offset {
        self.content_offset
    }

    fn store_pane_name(&mut self) {
        if self.pane_name != self.prev_pane_name {
            self.prev_pane_name = self.pane_name.clone()
        }
    }
    fn load_pane_name(&mut self) {
        if self.pane_name != self.prev_pane_name {
            self.pane_name = self.prev_pane_name.clone();
            self.set_should_render(true);
        }
    }

    fn borderless(&self) -> bool {
        self.borderless
    }
    fn set_borderless(&mut self, should_be_borderless: bool) {
        self.set_borderless_with_content_offset(
            should_be_borderless,
            if should_be_borderless {
                Offset::default()
            } else {
                Offset::frame(1)
            },
        );
    }
    fn set_borderless_with_content_offset(&mut self, borderless: bool, offset: Offset) {
        self.borderless = borderless;
        self.set_content_offset(offset);
    }

    fn set_exclude_from_sync(&mut self, exclude_from_sync: bool) {
        self.exclude_from_sync = exclude_from_sync;
    }

    fn exclude_from_sync(&self) -> bool {
        self.exclude_from_sync
    }

    fn mouse_event(&self, event: &MouseEvent, _client_id: ClientId) -> Option<String> {
        self.grid.mouse_event_signal(event)
    }

    fn mouse_left_click(&self, position: &Position, is_held: bool) -> Option<String> {
        self.grid.mouse_left_click_signal(position, is_held)
    }
    fn mouse_left_click_release(&self, position: &Position) -> Option<String> {
        self.grid.mouse_left_click_release_signal(position)
    }
    fn mouse_right_click(&self, position: &Position, is_held: bool) -> Option<String> {
        self.grid.mouse_right_click_signal(position, is_held)
    }
    fn mouse_right_click_release(&self, position: &Position) -> Option<String> {
        self.grid.mouse_right_click_release_signal(position)
    }
    fn mouse_middle_click(&self, position: &Position, is_held: bool) -> Option<String> {
        self.grid.mouse_middle_click_signal(position, is_held)
    }
    fn mouse_middle_click_release(&self, position: &Position) -> Option<String> {
        self.grid.mouse_middle_click_release_signal(position)
    }
    fn mouse_scroll_up(&self, position: &Position) -> Option<String> {
        self.grid.mouse_scroll_up_signal(position)
    }
    fn mouse_scroll_down(&self, position: &Position) -> Option<String> {
        self.grid.mouse_scroll_down_signal(position)
    }
    fn focus_event(&self) -> Option<String> {
        self.grid.focus_event()
    }
    fn unfocus_event(&self) -> Option<String> {
        self.grid.unfocus_event()
    }
    fn get_line_number(&self) -> Option<usize> {
        // + 1 because the absolute position in the scrollback is 0 indexed and this should be 1 indexed
        Some(self.grid.absolute_position_in_scrollback() + 1)
    }

    fn update_search_term(&mut self, needle: &str) {
        match needle {
            TERMINATING_STRING => {
                self.search_term = String::new();
            },
            DELETE_KEY | BACKSPACE_KEY => {
                self.search_term.pop();
            },
            c => {
                self.search_term.push_str(c);
            },
        }
        self.grid.set_search_string(&self.search_term);
        self.set_should_render(true);
    }
    fn search_down(&mut self) {
        if self.search_term.is_empty() {
            return; // No-op
        }
        self.grid.search_down();
        self.set_should_render(true);
    }
    fn search_up(&mut self) {
        if self.search_term.is_empty() {
            return; // No-op
        }
        self.grid.search_up();
        self.set_should_render(true);
    }
    fn toggle_search_case_sensitivity(&mut self) {
        self.grid.toggle_search_case_sensitivity();
        self.set_should_render(true);
    }
    fn toggle_search_whole_words(&mut self) {
        self.grid.toggle_search_whole_words();
        self.set_should_render(true);
    }
    fn toggle_search_wrap(&mut self) {
        self.grid.toggle_search_wrap();
    }
    fn clear_search(&mut self) {
        self.grid.clear_search();
        self.search_term.clear();
    }
    fn is_alternate_mode_active(&self) -> bool {
        self.grid.is_alternate_mode_active()
    }
    fn hold(&mut self, exit_status: Option<i32>, is_first_run: bool, mut run_command: RunCommand) {
        // Exit notifications travel separately from PTY bytes and can precede the
        // final provider output. Let the ordered end event reset the grid palette;
        // respawn also resets it before accepting a new process's output.
        run_command.agent_theme = None;
        self.invoked_with = Some(Run::Command(run_command.clone()));
        // A held pane has no running process to be stopped, and a stale flag
        // would leave the resume hint up over a dead pane forever.
        self.suspended = false;
        self.is_held = Some((exit_status, is_first_run, run_command));
        self.clear_waiting_pulse();
        if let Some(agent) = self.agent.as_mut() {
            agent.state = AgentState::Exited;
            agent.detail = None;
        }
        if let Some(notification_end) = self.notification_end.as_mut()
            && let Some(exit_status) = exit_status
        {
            notification_end.set_exit_status(exit_status);

            // Check if unblock condition is met
            if let Some(condition) = notification_end.unblock_condition()
                && condition.is_met(exit_status)
            {
                // Condition met - drop the NotificationEnd now to unblock
                drop(self.notification_end.take());
            }
        }
        if is_first_run {
            self.render_first_run_banner();
        }
        self.set_should_render(true);
    }
    fn has_bell(&self) -> bool {
        self.grid.ring_bell
    }
    fn consume_bell(&mut self) {
        self.grid.ring_bell = false;
    }
    fn osc7_payload(&self) -> Option<&str> {
        self.grid.osc7_payload()
    }
    fn set_bell_notification(&mut self, val: bool) {
        if self.has_bell_notification != val {
            self.has_bell_notification = val;
            self.set_should_render(true);
        }
    }
    fn get_bell_notification(&self) -> bool {
        self.has_bell_notification
    }
    fn add_red_pane_frame_color_override(&mut self, error_text: Option<String>) {
        // A command that failed is a fact about the pane, so every client sees it.
        let override_ =
            FrameColorOverride::new(self.style.colors.exit_code_error.base, error_text, None);
        if FrameColorOverride::set_if_changed(&mut self.pane_frame_color_override, override_) {
            self.set_should_render(true);
        }
    }
    fn add_highlight_pane_frame_color_override(
        &mut self,
        text: Option<String>,
        client_id: Option<ClientId>,
    ) {
        let override_ = FrameColorOverride::new(
            self.style.colors.frame_highlight.emphasis_0,
            text,
            client_id,
        );
        if FrameColorOverride::set_if_changed(&mut self.pane_frame_color_override, override_) {
            self.set_should_render(true);
        }
    }
    fn frame_color_override_strength_is_settled(&self, now: Instant) -> bool {
        self.pane_frame_color_override
            .as_ref()
            .is_none_or(|override_| override_.is_faded_out(now))
    }
    fn flash_pane_frame_color_override(&mut self, now: Instant) {
        self.pane_frame_color_override = Some(FrameColorOverride::flashing(
            self.style.colors.frame_highlight.emphasis_0,
            now,
        ));
    }
    fn clear_pane_frame_color_override(&mut self, client_id: Option<ClientId>) {
        if self
            .pane_frame_color_override
            .as_ref()
            .is_some_and(|override_| override_.is_cleared_by(client_id))
        {
            self.pane_frame_color_override = None;
            self.set_should_render(true);
        }
    }
    fn frame_color_override(&self, client_id: ClientId) -> Option<PaletteColor> {
        self.pane_frame_color_override
            .as_ref()
            .filter(|override_| override_.applies_to(client_id))
            .map(|override_| override_.color)
    }
    fn frame_color_override_strength(&self, client_id: ClientId) -> f32 {
        self.pane_frame_color_override
            .as_ref()
            .filter(|override_| override_.applies_to(client_id))
            .map_or(1.0, |override_| override_.strength(Instant::now()))
    }
    fn waiting_pulse_started_at(&self) -> Option<Instant> {
        self.waiting_pulse_started_at
    }
    fn sample_waiting_pulse(&mut self, now: Instant, enabled: bool) {
        let pulse = self
            .waiting_pulse_started_at
            .filter(|_| enabled)
            .map(|start| {
                // Two eased 900ms halves, sampled from elapsed time rather than tick count.
                let phase = (now.saturating_duration_since(start).as_nanos() % 1_800_000_000)
                    as f32
                    / 900_000_000.0;
                0.7 * Easing::EaseInOut.apply(1.0 - (phase - 1.0).abs())
            });
        if self.waiting_pulse != pulse {
            self.waiting_pulse = pulse;
            self.set_should_render(true);
        }
    }
    fn waiting_pulse(&self) -> Option<f32> {
        self.waiting_pulse
    }
    fn invoked_with(&self) -> &Option<Run> {
        &self.invoked_with
    }
    fn set_notification_end(&mut self, notification_end: Option<NotificationEnd>) {
        self.notification_end = notification_end;
    }
    fn set_title(&mut self, title: String) {
        if self.pane_title != title {
            self.pane_title = title;
            self.set_should_render(true);
        }
    }
    fn current_title(&self) -> String {
        if self.pane_name.is_empty() {
            self.grid
                .title
                .as_deref()
                .unwrap_or(&self.pane_title)
                .into()
        } else {
            self.pane_name.to_owned()
        }
    }
    fn set_remote_host(&mut self, host: Option<String>) {
        if self.remote_host != host {
            self.remote_host = host;
            self.set_should_render(true);
        }
    }
    fn set_agent_hook_flash(&mut self, flash: bool) {
        self.agent_hook_flash = flash;
    }
    fn agent_title(&self) -> crate::ui::agent_title::AgentTitle {
        crate::ui::agent_title::AgentTitle {
            state: self.agent.as_ref().map(|agent| agent.state),
            hook_flash: self.agent_hook_flash,
        }
    }
    fn agent_info(&self) -> Option<&AgentInfo> {
        self.agent.as_ref()
    }
    fn supports_bracketed_paste(&self) -> bool {
        self.grid.bracketed_paste_mode
    }
    fn agent_input_revision(&self) -> u64 {
        self.agent_input_revision
    }
    fn agent_enter_revision(&self) -> u64 {
        self.agent_enter_revision
    }
    fn agent_input_at(&self) -> Option<Instant> {
        self.agent_input_at
    }
    fn record_agent_input(&mut self, is_enter: bool) {
        self.agent_input_revision = self.agent_input_revision.wrapping_add(1);
        if is_enter {
            self.agent_enter_revision = self.agent_input_revision;
        }
        self.agent_input_at = Some(Instant::now());
    }
    fn last_hook_at_ms(&self) -> Option<i64> {
        self.last_hook_at_ms
    }
    fn set_last_hook_at_ms(&mut self, at: i64) {
        self.last_hook_at_ms = Some(at);
    }
    fn last_agent_cwd(&self) -> Option<&str> {
        self.last_agent_cwd.as_deref()
    }
    fn last_agent_session_id(&self) -> Option<&str> {
        self.last_agent_session_id.as_deref()
    }
    fn clear_agent_session_id(&mut self) {
        self.last_hook_at_ms = None;
        self.last_agent_session_id = None;
    }
    fn last_agent_kind(&self) -> Option<&str> {
        self.last_agent_kind.as_deref()
    }
    fn was_an_agent(&self) -> bool {
        self.was_an_agent
    }
    fn set_agent_info(&mut self, agent: Option<AgentInfo>) -> bool {
        // Before the equality check: a record that repeats itself still says this is an agent
        // pane, and the memory is set once and never taken back.
        self.was_an_agent |= agent.is_some();
        if agent.is_none()
            || self
                .agent
                .as_ref()
                .map(|agent| (&agent.kind, &agent.session_id, &agent.run_id))
                != agent
                    .as_ref()
                    .map(|agent| (&agent.kind, &agent.session_id, &agent.run_id))
        {
            self.agent_hook_flash = false;
        }
        if agent.is_none()
            || self.agent.as_ref().map(|a| (&a.kind, &a.session_id))
                != agent.as_ref().map(|a| (&a.kind, &a.session_id))
        {
            self.last_hook_at_ms = None;
        }
        if let Some(agent) = &agent {
            if self.last_agent_kind.as_deref() != Some(agent.kind.as_str()) {
                self.last_agent_session_id = None;
            }
            self.last_agent_kind = Some(agent.kind.clone());
            if let Some(id) = agent.session_id.as_ref().filter(|id| !id.is_empty()) {
                self.last_agent_session_id = Some(id.clone());
            }
            if agent.cwd.is_some() {
                self.last_agent_cwd = agent.cwd.clone();
            }
        }
        if self.agent == agent {
            return false;
        }
        let waiting = agent
            .as_ref()
            .is_some_and(AgentInfo::is_waiting_on_a_person);
        if !waiting {
            self.clear_waiting_pulse();
        } else if !self
            .agent
            .as_ref()
            .is_some_and(AgentInfo::is_waiting_on_a_person)
        {
            self.waiting_pulse_started_at = Some(Instant::now());
        }
        self.agent = agent;
        self.set_should_render(true);
        true
    }
    fn set_title_prefix(&mut self, prefix: Option<String>) {
        if self.title_prefix != prefix {
            self.title_prefix = prefix;
            self.set_should_render(true);
        }
    }
    fn display_title(&self) -> String {
        // Where the pane is, before what it is doing: the host it runs on, then
        // the checkout it sits in. Both are stable facts about the pane, so they
        // read as an address and the title reads as the news.
        let lead: Vec<&str> = self
            .remote_host
            .iter()
            .chain(self.title_prefix.iter())
            .map(String::as_str)
            .collect();
        let title = if self.has_explicit_title() {
            Some(self.current_title())
        } else {
            self.running_program.clone()
        };
        match (lead.is_empty(), title) {
            (true, title) => title.unwrap_or_else(|| self.current_title()),
            (false, Some(title)) => format!("{}: {}", lead.join(" "), title),
            (false, None) => lead.join(" "),
        }
    }
    fn stack_list_entry_label(&self, same_worktree: bool) -> String {
        self.title_with_agent_indicator(
            self.title_with_bell_indicator(self.framed_title()),
            same_worktree,
        )
    }
    fn custom_title(&self) -> Option<String> {
        if self.pane_name.is_empty() {
            None
        } else {
            Some(self.pane_name.clone())
        }
    }
    fn set_running_program(&mut self, program: Option<String>) {
        let program = program
            .as_deref()
            .and_then(|program| std::path::Path::new(program).file_name())
            .and_then(|name| name.to_str())
            .map(|name| name.trim_start_matches('-').to_owned())
            .filter(|name| !name.is_empty());
        if self.running_program != program {
            self.running_program = program;
            self.set_should_render(true);
        }
    }
    fn can_lend_title(&self) -> bool {
        self.has_explicit_title() || self.title_prefix.is_some() || self.running_program.is_some()
    }
    fn has_explicit_title(&self) -> bool {
        !self.pane_name.is_empty() || self.grid.title.is_some()
    }
    fn scroll_position(&self) -> (usize, usize) {
        self.grid.scrollback_position_and_length()
    }
    fn exit_status(&self) -> Option<i32> {
        self.is_held
            .as_ref()
            .and_then(|(exit_status, _, _)| *exit_status)
    }
    fn set_warp_pending(&mut self, pending: bool) {
        self.warp_pending = pending;
    }
    fn is_held(&self) -> bool {
        self.is_held.is_some()
    }
    fn is_suspended(&self) -> bool {
        self.suspended
    }
    fn set_suspended(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        self.set_should_render(true);
    }
    fn exited(&self) -> bool {
        match self.is_held {
            Some((_, is_first_run, _)) => !is_first_run,
            None => false,
        }
    }
    fn rename(&mut self, buf: Vec<u8>) {
        self.pane_name = String::from_utf8_lossy(&buf).to_string();
        self.set_should_render(true);
    }
    fn serialize_handoff(
        &self,
        scrollback: Option<usize>,
        budget: usize,
    ) -> Option<(String, Option<yinz_types::agent::theme::AgentThemeHandoff>)> {
        self.grid.serialize_handoff(scrollback, budget)
    }
    fn serialize(&self, scrollback_lines_to_serialize: Option<usize>) -> Option<String> {
        self.grid.serialize(scrollback_lines_to_serialize)
    }
    fn restart_error(&self) -> Option<String> {
        self.restart_command_args().err()
    }
    fn agent_restart_command(
        &self,
        session: yinz_types::agent::control::AgentRestartSession,
        prompt: Option<&str>,
    ) -> Result<RunCommand, String> {
        use yinz_types::agent::control::{AgentRestartSession, MAX_PROMPT_BYTES};
        if prompt.is_some_and(|prompt| prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0')) {
            return Err("restart prompt exceeds 1 MiB or contains NUL".into());
        }
        let Some((_, false, command)) = self.is_held.as_ref() else {
            return Err("target must be an exited, held agent pane".into());
        };
        if !self.was_an_agent || self.warp_pending {
            return Err("target must be an exited, held agent pane".into());
        }
        self.restart_command_args()?;
        let session_id = match session {
            AgentRestartSession::Resume => Some(
                self.last_agent_session_id
                    .as_deref()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or("agent pane has no remembered session id")?,
            ),
            AgentRestartSession::Fresh => None,
        };
        let args = yinz_runtime::agent::restart_with_prompt(
            &command.command,
            &command.args,
            session_id,
            prompt,
        )?;
        Ok(RunCommand {
            args,
            ..command.clone()
        })
    }
    fn respawn(
        &mut self,
        replacement: Option<RunCommand>,
        keep_scrollback: bool,
    ) -> Option<RunCommand> {
        // if this is a command pane that has exited or is waiting to be rerun, will return the
        // command to spawn - its own, or the replacement - otherwise it is safe to assume this
        // is not the right sort of pane or that it is not in the right sort of state
        if self.warp_pending {
            return None;
        }
        self.is_held.as_ref()?;
        let cleaned = if replacement.is_none() {
            match self.restart_command_args() {
                Ok(args) => args,
                Err(message) => {
                    Err::<(), _>(anyhow!(message.clone())).non_fatal();
                    self.add_red_pane_frame_color_override(Some(message));
                    self.set_should_render(true);
                    return None;
                },
            }
        } else {
            None
        };
        // Explicit resumes restore their selected session in Screen::restart_agent_pane.
        let fresh_agent = cleaned.is_some() || replacement.is_some();
        if fresh_agent {
            self.last_hook_at_ms = None;
            self.last_agent_session_id = None;
        }
        self.is_held.take().map(|(_, _, mut run_command)| {
            run_command.agent_theme = None;
            if let Some(Run::Command(command)) = self.invoked_with.as_mut() {
                command.agent_theme = None;
            }
            if let Some(args) = cleaned {
                run_command.args = args;
                self.invoked_with = Some(Run::Command(run_command.clone()));
            }
            self.is_held = None;
            self.suspended = false;
            self.agent_input_revision = 0;
            self.agent_enter_revision = 0;
            self.agent_input_at = None;
            self.clear_waiting_pulse();
            if let Some(agent) = self.agent.as_mut() {
                agent.state = AgentState::Busy;
                if fresh_agent {
                    agent.session_id = None;
                    agent.run_id = None;
                    agent.detail = None;
                    agent.waiting_on = None;
                    agent.wait_needs_a_person = false;
                    agent.source = AgentSource::Spawned;
                }
            }
            if keep_scrollback {
                self.grid.reset_terminal_state_preserving_scrollback();
            } else {
                self.grid.reset_terminal_state();
            }
            self.grid.begin_agent_run();
            self.set_should_render(true);
            self.remove_banner();
            match replacement {
                Some(mut replacement) => {
                    replacement.agent_theme = None;
                    // A pane re-run with a *different* command is not the same pane's work
                    // any more, and neither the record nor the memory of one says anything
                    // true about it. Left alone, a plugin reusing a held agent pane for `npm
                    // test` would fire `agent_exit` on that command's exit, naming it as the
                    // agent's - and the sticky memory would make it unrecoverable, because
                    // clearing the record no longer disarms the hook.
                    //
                    // `command` and `args` rather than the whole `RunCommand`: a plugin
                    // re-running the same agent builds a fresh one, whose `hold_on_close`,
                    // `originating_plugin` and the rest may differ incidentally, and reading
                    // that as a new command would forget a live agent.
                    if replacement.command != run_command.command
                        || replacement.args != run_command.args
                    {
                        self.clear_agent_identity();
                    }
                    self.invoked_with = Some(Run::Command(replacement.clone()));
                    // A replacement marked as an agent starts a fresh conversation.
                    if let Some(agent) = agent_from_run(self.invoked_with.as_ref()) {
                        self.set_agent_info(Some(agent));
                    }
                    replacement
                },
                None => run_command.clone(),
            }
        })
    }
    fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
        self.grid.update_theme(theme);
        if self.banner.is_some() {
            // we do this so that the banner will be updated with the new theme colors
            self.render_first_run_banner();
        }
    }
    fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        self.grid.update_arrow_fonts(should_support_arrow_fonts);
    }
    fn update_render_options(&mut self, styled_underlines: bool, osc8_hyperlinks: bool) {
        self.grid
            .update_render_options(styled_underlines, osc8_hyperlinks);
    }
    fn update_kitty_keyboard_protocol_disabled(&mut self, disabled: bool) {
        self.grid.set_kitty_keyboard_protocol_disabled(disabled);
    }
    fn update_kitty_host_support(&mut self, supported: KittyHostSupport) {
        self.grid.update_kitty_host_support(supported);
    }
    fn update_sixel_host_support(&mut self, supported: bool) {
        self.grid.update_sixel_host_support(supported);
    }
    fn update_rounded_corners(&mut self, rounded_corners: bool) {
        if self.style.rounded_corners != rounded_corners {
            self.style.rounded_corners = rounded_corners;
            self.frame.clear();
            self.set_should_render(true);
        }
    }
    fn drain_fake_cursors(&mut self) -> Option<HashSet<(usize, usize)>> {
        if !self.fake_cursor_locations.is_empty() {
            for (y, _x) in &self.fake_cursor_locations {
                // we do this because once these fake_cursor_locations
                // have been drained, we have to make sure to render the line
                // they appeared on so that whatever clears their location
                // won't leave a hole
                self.grid.update_line_for_rendering(*y);
            }
            Some(std::mem::take(&mut self.fake_cursor_locations))
        } else {
            None
        }
    }
    fn toggle_pinned(&mut self) {
        self.set_pinned(!self.geom.is_pinned);
    }
    fn set_pinned(&mut self, should_be_pinned: bool) {
        if self.geom.is_pinned != should_be_pinned {
            self.geom.is_pinned = should_be_pinned;
            self.set_should_render(true);
        }
    }
    fn intercept_mouse_event_on_frame(&mut self, event: &MouseEvent, client_id: ClientId) -> bool {
        if self.position_is_on_frame(&event.position) {
            let relative_position = self.relative_position(&event.position);
            if let MouseEventType::Press = event.event_type
                && let Some(client_frame) = self.frame.get_mut(&client_id)
                && client_frame.clicked_on_pinned(relative_position)
            {
                self.toggle_pinned();
                return true;
            }
        }
        false
    }
    fn reset_logical_position(&mut self) {
        self.geom.logical_position = None;
    }
    fn pane_contents(
        &self,
        _client_id: Option<ClientId>,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        self.grid
            .pane_contents(get_full_scrollback, max_scrollback_lines)
    }
    fn pane_contents_with_ansi(
        &self,
        _client_id: Option<ClientId>,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        self.grid
            .pane_contents_with_ansi(get_full_scrollback, max_scrollback_lines)
    }
    fn update_exit_status(&mut self, exit_status: i32) {
        if let Some(notification_end) = self.notification_end.as_mut() {
            notification_end.set_exit_status(exit_status);
            // Check if unblock condition is met
            if let Some(condition) = notification_end.unblock_condition()
                && condition.is_met(exit_status)
            {
                // Condition met - drop the NotificationEnd now to unblock
                drop(self.notification_end.take());
            }
        }
    }
    fn set_plugin_regex_highlights(
        &mut self,
        plugin_id: PluginId,
        highlights: Vec<RegexHighlight>,
        style: &Style,
    ) {
        self.grid
            .set_plugin_regex_highlights(plugin_id, highlights, style);
        self.set_should_render(true);
    }
    fn clear_plugin_highlights(&mut self, plugin_id: PluginId) {
        self.grid.clear_plugin_highlights(plugin_id);
        self.set_should_render(true);
    }
    fn set_hover_position(&mut self, position: Option<Position>) -> bool {
        let changed = self.grid.set_hover_position(position);
        if changed {
            self.set_should_render(true);
        }
        changed
    }
    fn cached_hover_tooltip(&self) -> Option<String> {
        self.grid.cached_hover_tooltip.clone()
    }
    fn plugin_highlight_at(
        &self,
        position: &Position,
    ) -> Option<(
        PluginId,
        String,
        String,
        std::collections::BTreeMap<String, String>,
    )> {
        self.grid.plugin_highlight_at(position)
    }
    fn terminal_emulator_wants_mouse(&self) -> bool {
        self.grid.mouse_tracking != crate::panes::grid::MouseTracking::Off
    }
    fn text_around_position(&self, position: &Position) -> Option<TextAroundPosition> {
        // The same two lookups double-click and triple-click already use, read rather than
        // selected: this must not disturb the selection the caller is about to record.
        let text_for = |bounds: Option<(Position, Position)>| {
            let (start, end) = bounds?;
            let mut selection = Selection::default();
            selection.set_start_and_end_positions(start, end);
            self.grid
                .text_for_selection(&selection)
                .filter(|text| !text.trim().is_empty())
        };
        Some(TextAroundPosition {
            word: text_for(self.grid.word_around_position(position)),
            line: text_for(self.grid.canonical_line_around_position(position)),
        })
    }
}

/// The agent record a pane starts with: an explicitly marked plugin or direct CLI launch.
/// The agent itself has said nothing yet, so the source is `Spawned`.
fn agent_from_run(invoked_with: Option<&Run>) -> Option<AgentInfo> {
    let Some(Run::Command(run_command)) = invoked_with else {
        return None;
    };
    let kind = match &run_command.originating_plugin {
        Some(plugin) => match plugin.context.get(AGENT_PANE_CONTEXT)?.trim() {
            "" | "true" => {
                yinz_types::agent::agent_kind_of(&run_command.command, &run_command.args)?
            },
            kind => kind.to_owned(),
        },
        None if run_command
            .command
            .file_name()
            .is_some_and(|name| name == "env")
            && run_command
                .args
                .iter()
                .take_while(|arg| arg.contains('='))
                .any(|arg| arg.strip_prefix(yinz_types::agent::AGENT_PANE_ENV) == Some("=1")) =>
        {
            yinz_types::agent::kind_from_program_and_args(&run_command.command, &run_command.args)?
                .to_owned()
        },
        None => return None,
    };
    Some(AgentInfo {
        kind,
        state: AgentState::Busy,
        source: AgentSource::Spawned,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: run_command
            .cwd
            .as_ref()
            .map(|cwd| cwd.display().to_string()),
        session_id: None,
        // Nothing is keeping this agent's history yet; a plugin that starts to says so with
        // `SetPaneAgentRun`.
        run_id: None,
    })
}

impl TerminalPane {
    fn clear_agent_identity(&mut self) {
        self.set_agent_info(None);
        self.was_an_agent = false;
        self.last_agent_kind = None;
        self.last_agent_cwd = None;
        self.last_agent_session_id = None;
        self.detected_agent = false;
        self.agent_input_revision = 0;
        self.agent_enter_revision = 0;
        self.agent_input_at = None;
        self.pending_agent_escape_clients.clear();
    }

    fn clear_waiting_pulse(&mut self) {
        self.waiting_pulse_started_at = None;
        if self.waiting_pulse.take().is_some() {
            self.set_should_render(true);
        }
    }

    /// The byte-at-a-time ingest `handle_pty_bytes` has to match: every byte through the
    /// interceptor, and whatever it forwards straight to the parser. Without the host-query
    /// pause, so only for streams that ask the host nothing.
    #[cfg(test)]
    pub(crate) fn ingest_per_byte(&mut self, bytes: &[u8]) {
        use crate::panes::kitty_graphics::InterceptorResult;
        self.grid.should_render = true;
        for &byte in bytes {
            match self.kitty_interceptor.advance(byte) {
                InterceptorResult::Forward(forward) => {
                    self.vte_parser.advance(&mut self.grid, forward.as_slice())
                },
                InterceptorResult::Swallow | InterceptorResult::CaptureStarted => {},
                InterceptorResult::Captured(command) => {
                    self.grid.handle_kitty_apc(&command);
                },
            }
        }
    }

    fn restart_command_args(&self) -> Result<Option<Vec<String>>, String> {
        match self.is_held.as_ref() {
            Some((_, false, command)) if self.was_an_agent => {
                yinz_runtime::agent::restart_args(&command.command, &command.args)
                    .map(Some)
                    .map_err(|error| format!("Cannot restart agent: {error}"))
            },
            _ => Ok(None),
        }
    }

    /// The frame's title: the pane's id, then [`Pane::display_title`].
    ///
    /// The id leads because it is how the pane is addressed from outside — a
    /// `--pane-id` argument, agent's agent list, a line in the log — and nothing
    /// else on screen says what it is. Bracketed so it reads as a label rather
    /// than as the first word of the title.
    ///
    /// Deliberately not folded into `display_title`: that is also a *tab's* name
    /// when a tab holds one pane, and a pane id is not a name for a tab.
    fn framed_title(&self) -> String {
        format!("[{}] {}", self.pid, self.display_title())
    }
    fn title_with_agent_indicator(&self, title: String, same_worktree: bool) -> String {
        let title = if same_worktree {
            format!("👯 {title}")
        } else {
            title
        };
        if self
            .agent
            .as_ref()
            .is_some_and(AgentInfo::is_waiting_on_a_person)
        {
            format!("🔔 {title}")
        } else {
            title
        }
    }
    fn title_with_bell_indicator(&self, title: String) -> String {
        if self.has_bell_notification {
            format!("{} [!]", title)
        } else {
            title
        }
    }
    pub fn new(
        pid: TerminalId,
        position_and_size: PaneGeom,
        style: Style,
        pane_index: usize,
        pane_name: String,
        link_handler: Rc<RefCell<LinkHandler>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        terminal_emulator_colors: Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        explicitly_disable_keyboard_protocol: bool,
        mut notification_end: Option<NotificationEnd>,
    ) -> TerminalPane {
        let initial_pane_title = initial_pane_title.unwrap_or_else(|| match &invoked_with {
            Some(Run::Command(command)) => command.to_string(),
            _ => format!("Pane #{}", pane_index),
        });
        let agent = agent_from_run(invoked_with.as_ref());
        let mut grid = Grid::new(
            position_and_size.rows.as_usize(),
            position_and_size.cols.as_usize(),
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            link_handler,
            character_cell_size,
            sixel_image_store,
            kitty_image_store,
            style,
            debug,
            arrow_fonts,
            styled_underlines,
            osc8_hyperlinks,
            explicitly_disable_keyboard_protocol,
        );
        if let Some(Run::Command(command)) = &invoked_with
            && let Some(snapshot) = &command.agent_theme
        {
            grid.apply_agent_theme_event(yinz_types::agent::theme::AgentThemeEvent {
                version: yinz_types::agent::theme::EVENT_VERSION,
                launch_id: snapshot.launch_id.clone(),
                snapshot: Some(snapshot.snapshot.clone()),
            });
        }
        if let Some(notification_end) = notification_end.as_mut() {
            notification_end.set_affected_pane_id(PaneId::Terminal(pid));
        }
        TerminalPane {
            pending_agent_escape_clients: HashSet::new(),
            frame: HashMap::new(),
            content_offset: Offset::default(),
            detected_agent: false,
            agent_hook_flash: false,
            max_content_width: 0,
            render_width: None,
            pane_tabs: None,
            pane_tabs_views: HashMap::new(),
            pane_tab_titles: Vec::new(),
            pane_tabs_transfer: None,
            subtitle: None,
            subtitle_enabled: true,
            pid,
            grid,
            selectable: true,
            geom: position_and_size,
            geom_override: None,
            vte_parser: vte::Parser::new(),
            #[cfg(test)]
            vte_batches: 0,
            active_at: Instant::now(),
            style,
            selection_scrolled_at: time::Instant::now(),
            pane_title: initial_pane_title,
            pane_name: pane_name.clone(),
            prev_pane_name: pane_name,
            remote_host: None,
            title_prefix: None,
            running_program: None,
            was_an_agent: agent.is_some(),
            last_agent_kind: agent.as_ref().map(|agent| agent.kind.clone()),
            last_agent_cwd: agent.as_ref().and_then(|agent| agent.cwd.clone()),
            last_hook_at_ms: None,
            last_agent_session_id: agent.as_ref().and_then(|agent| agent.session_id.clone()),
            agent,
            waiting_pulse_started_at: None,
            waiting_pulse: None,
            agent_input_revision: 0,
            agent_enter_revision: 0,
            agent_input_at: None,
            borderless: false,
            exclude_from_sync: false,
            fake_cursor_locations: HashSet::new(),
            search_term: String::new(),
            warp_pending: false,
            is_held: None,
            suspended: false,
            banner: None,
            pane_frame_color_override: None,
            has_bell_notification: false,
            invoked_with,
            notification_end,
            forward_paused: false,
            nested_guest: false,
            guest_modal: HashMap::new(),
            guest_choice_indicators: HashMap::new(),
            guest_session_name: None,
            guest_modal_shortcuts: GuestModalShortcuts::default(),
            pending_pty_input: VecDeque::new(),
            kitty_interceptor: KittyApcInterceptor::new(),
        }
    }
    /// Resizes the grid to the pane's content area, and only the grid. The pty gets its size
    /// separately: the pane's container or the tab sends `resize_pty!` through the pty writer,
    /// where the screen thread's `ResizeCache` collapses a burst of resizes into one per pane, and
    /// `ServerOsApi` delivers it over the pane's transport.
    /// Resizing the pty from here would signal the child at every intermediate size.
    fn reflow_lines(&mut self) {
        let rows = self.get_content_rows();
        let cols = self.get_content_columns();
        self.grid.force_change_size(rows, cols);
        if self.banner.is_some() {
            self.grid.reset_terminal_state();
            self.render_first_run_banner();
        }
        self.set_should_render(true);
    }
    pub fn read_buffer_as_lines(&self) -> Vec<Vec<TerminalCharacter>> {
        self.grid.as_character_lines()
    }
    pub fn cursor_coordinates(&self) -> Option<(usize, usize, bool)> {
        // (x, y, is_visible)
        if self.get_content_rows() < 1 || self.get_content_columns() < 1 {
            // do not render cursor if there's no room for it
            return None;
        }
        self.grid.cursor_coordinates()
    }
    fn render_first_run_banner(&mut self) {
        let columns = self.get_content_columns();
        let rows = self.get_content_rows();
        let banner = match &self.is_held {
            Some((_exit_status, _is_first_run, run_command)) => {
                render_first_run_banner(columns, rows, &self.style, Some(run_command))
            },
            None => render_first_run_banner(columns, rows, &self.style, None),
        };
        self.banner = Some(banner.clone());
        self.handle_pty_bytes(banner.as_bytes().to_vec());
    }
    fn remove_banner(&mut self) {
        if self.banner.is_some() {
            self.grid.reset_terminal_state();
            self.set_should_render(true);
            self.banner = None;
        }
    }
    fn adjust_input_to_terminal_with_kitty_keyboard_protocol(
        &self,
        key: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
    ) -> Option<AdjustedInput> {
        if raw_input_bytes_are_kitty || key.is_none() {
            Some(AdjustedInput::WriteBytesToTerminal(raw_input_bytes))
        } else {
            // here what happens is that the host terminal is operating in non "kitty keys" mode, but
            // this terminal pane *is* operating in "kitty keys" mode - so we need to serialize the "non kitty"
            // key to a "kitty key"
            key.as_ref()
                .and_then(yinz_key_codec::keys::serialize_kitty)
                .map(|s| AdjustedInput::WriteBytesToTerminal(s.as_bytes().to_vec()))
        }
    }
    fn adjust_input_to_terminal_without_kitty_keyboard_protocol(
        &self,
        key: &Option<KeyWithModifier>,
        raw_input_bytes: Vec<u8>,
        raw_input_bytes_are_kitty: bool,
    ) -> Option<AdjustedInput> {
        if self.grid.new_line_mode {
            let key_is_enter = raw_input_bytes.as_slice() == [13]
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::Enter))
                    .unwrap_or(false);
            if key_is_enter {
                // LNM - carriage return is followed by linefeed
                return Some(AdjustedInput::WriteBytesToTerminal(
                    "\u{0d}\u{0a}".as_bytes().to_vec(),
                ));
            };
        }
        if self.grid.cursor_key_mode {
            let key_is_left_arrow = raw_input_bytes.as_slice() == LEFT_ARROW
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::Left))
                    .unwrap_or(false);
            let key_is_right_arrow = raw_input_bytes.as_slice() == RIGHT_ARROW
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::Right))
                    .unwrap_or(false);
            let key_is_up_arrow = raw_input_bytes.as_slice() == UP_ARROW
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::Up))
                    .unwrap_or(false);
            let key_is_down_arrow = raw_input_bytes.as_slice() == DOWN_ARROW
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::Down))
                    .unwrap_or(false);
            let key_is_home_key = raw_input_bytes.as_slice() == HOME_KEY
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::Home))
                    .unwrap_or(false);
            let key_is_end_key = raw_input_bytes.as_slice() == END_KEY
                || key
                    .as_ref()
                    .map(|k| k.is_key_without_modifier(BareKey::End))
                    .unwrap_or(false);
            if key_is_left_arrow {
                return Some(AdjustedInput::WriteBytesToTerminal(
                    AnsiEncoding::Left.as_vec_bytes(),
                ));
            } else if key_is_right_arrow {
                return Some(AdjustedInput::WriteBytesToTerminal(
                    AnsiEncoding::Right.as_vec_bytes(),
                ));
            } else if key_is_up_arrow {
                return Some(AdjustedInput::WriteBytesToTerminal(
                    AnsiEncoding::Up.as_vec_bytes(),
                ));
            } else if key_is_down_arrow {
                return Some(AdjustedInput::WriteBytesToTerminal(
                    AnsiEncoding::Down.as_vec_bytes(),
                ));
            } else if key_is_home_key {
                return Some(AdjustedInput::WriteBytesToTerminal(
                    AnsiEncoding::Home.as_vec_bytes(),
                ));
            } else if key_is_end_key {
                return Some(AdjustedInput::WriteBytesToTerminal(
                    AnsiEncoding::End.as_vec_bytes(),
                ));
            }
        }
        if raw_input_bytes_are_kitty {
            // here what happens is that the host terminal is operating in "kitty keys" mode, but
            // this terminal pane is not - so we need to serialize the kitty key to "non kitty" if
            // possible - if not possible (eg. with multiple modifiers), we'll return a None here
            // and write nothing to the terminal pane
            key.as_ref()
                .and_then(yinz_key_codec::keys::serialize_non_kitty)
                .map(|s| AdjustedInput::WriteBytesToTerminal(s.as_bytes().to_vec()))
        } else {
            Some(AdjustedInput::WriteBytesToTerminal(raw_input_bytes))
        }
    }
    /// Enter on a held pane asks for a re-run rather than doing one here. The restart itself
    /// is [`Pane::rerun`], and it runs under the screen's `rerun_command_pane_with_id` -
    /// the one place that reads the agent record around the restart, so an exited agent
    /// coming back is an `agent_start` whichever road asked for it.
    fn handle_held_run(&self) -> Option<AdjustedInput> {
        if self.warp_pending {
            return None;
        }
        self.is_held
            .as_ref()
            .map(|_| AdjustedInput::ReRunCommandInThisPane)
    }
    fn handle_held_drop_to_shell(&mut self) -> Option<AdjustedInput> {
        if self.warp_pending {
            return None;
        }
        self.is_held.take().map(|(_, _, run_command)| {
            // Drop to shell in the same working directory as the command was run
            let working_dir = run_command.cwd.clone();
            self.is_held = None;
            self.clear_agent_identity();
            self.invoked_with = None;
            self.grid.reset_terminal_state();
            self.grid.begin_agent_run();
            self.set_should_render(true);
            self.remove_banner();
            AdjustedInput::DropToShellInThisPane { working_dir }
        })
    }
}

#[cfg(test)]
#[path = "./unit/terminal_pane_tests.rs"]
mod grid_tests;

#[cfg(test)]
#[path = "./unit/search_in_pane_tests.rs"]
mod search_tests;

#[cfg(test)]
#[path = "./unit/render_pipeline_tests.rs"]
mod render_pipeline_tests;
