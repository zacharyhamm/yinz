#![allow(clippy::unnecessary_lazy_evaluations)]
#![allow(clippy::if_same_then_else)]
use crate::output::{CharacterChunk, KittyImageChunk, SixelImageChunk};
use crate::panes::kitty_graphics::{
    InterceptorResult, KittyApcInterceptor, KittyHostSupport, KittyImageStore,
};
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
use crate::tab::{AdjustedInput, GuestChoiceIndicator, Pane, TextAroundPosition};
use crate::ClientId;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::rc::Rc;
use std::time::{self, Instant};
use vte;
use zellij_utils::animation::blink_is_lit;
use zellij_utils::data::PaneContents;
use zellij_utils::data::{AgentInfo, AgentSource, AgentState, HOCUS_AGENT_PANE_CONTEXT};
use zellij_utils::input::command::RunCommand;
use zellij_utils::input::mouse::{MouseEvent, MouseEventType};
use zellij_utils::pane_size::Offset;
use zellij_utils::{
    data::{
        BareKey, Direction, InputMode, KeyWithModifier, Palette, PaletteColor, RegexHighlight,
        Style, Styling,
    },
    errors::prelude::*,
    input::layout::Run,
    nested_session::NestedSessionMessage,
    pane_size::PaneGeom,
    pane_size::SizeInPixels,
    position::Position,
    shared::make_terminal_title,
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
const CTRL_C: &[u8] = &[3]; // TODO: check this to be sure it fits all types of CTRL_C (with mac, etc)
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

pub use zellij_terminal::PaneId;

type IsFirstRun = bool;

// FIXME: This should hold an os_api handle so that terminal panes can set their own size via FD in
// their `reflow_lines()` method. Drop a Box<dyn ServerOsApi> in here somewhere.
#[allow(clippy::too_many_arguments)]
pub struct TerminalPane {
    pub grid: Grid,
    pub pid: u32,
    pub selectable: bool,
    pub geom: PaneGeom,
    pub geom_override: Option<PaneGeom>,
    pub active_at: Instant,
    pub style: Style,
    vte_parser: vte::Parser,
    selection_scrolled_at: time::Instant,
    content_offset: Offset,
    pane_title: String,
    pane_name: String,
    prev_pane_name: String,
    remote_host: Option<String>,
    /// A decoration composed in front of the title, whatever the title is.
    ///
    /// Deliberately not `pane_name`: that slot has three writers already (the
    /// user's rename, the pane's own OSC title, and hocus-agentd's idle
    /// summary), so anything put there is erased by the next one. A pane's
    /// checkout is a fact about the pane rather than a name for it, and the
    /// remote host beside this field is the same kind of fact.
    title_prefix: Option<String>,
    /// The agent running here, if the session knows of one. Starts as what the spawn path
    /// said; replaced whole by every report, and moved to `Exited`/`Busy` by hold and re-run.
    agent: Option<AgentInfo>,
    /// Whether this pane has ever held an agent record, however it got there.
    ///
    /// Sticky, and deliberately not derived from `agent`: a `SessionEnd` hook clears the
    /// record a moment before the agent's process exits, so a pane that gates `agent_exit` on
    /// the live record would disarm itself on every clean exit. The record is what the agent
    /// says now; this is that it was ever an agent at all.
    was_an_agent: bool,
    last_agent_kind: Option<String>,
    last_agent_cwd: Option<String>,
    /// When the agent here entered a wait a person has to answer, while nobody has answered
    /// it yet. Drives the blinking indicator in the frame's top-right corner, and nothing
    /// else.
    ///
    /// Display state rather than part of the record: the record is what the agent said, and
    /// the user answering the prompt is not the agent saying anything. It is armed on the
    /// not-waiting → waiting edge (a repeated `waiting` report must not restart the blink)
    /// and taken down the moment somebody types here or focuses the pane - see
    /// `acknowledge_agent_attention`.
    agent_attention_since: Option<Instant>,
    frame: HashMap<ClientId, PaneFrame>,
    borderless: bool,
    exclude_from_sync: bool,
    fake_cursor_locations: HashSet<(usize, usize)>, // (x, y) - these hold a record of previous fake cursors which we need to clear on render
    search_term: String,
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
    #[allow(dead_code)]
    arrow_fonts: bool,
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
    guest_session_name: Option<String>,
    guest_modal_shortcuts: GuestModalShortcuts,
    /// PTY bytes that have not yet been fed to vte. Single source of
    /// truth: `handle_pty_bytes` always appends here, and processing
    /// pops one byte at a time and advances the vte parser. Processing
    /// stops as soon as Grid produces a forward-bound query, leaving
    /// remaining bytes in the queue to be drained after the host reply
    /// has been written.
    pending_pty_input: VecDeque<u8>,
    kitty_interceptor: KittyApcInterceptor,
}

impl Pane for TerminalPane {
    fn x(&self) -> usize {
        self.get_x()
    }
    fn y(&self) -> usize {
        self.get_y()
    }
    fn rows(&self) -> usize {
        self.get_rows()
    }
    fn cols(&self) -> usize {
        self.get_columns()
    }
    fn get_content_x(&self) -> usize {
        self.get_x() + self.content_offset.left
    }
    fn get_content_y(&self) -> usize {
        self.get_y() + self.content_offset.top
    }
    fn get_content_columns(&self) -> usize {
        // content columns might differ from the pane's columns if the pane has a frame
        // in that case they would be 2 less
        self.get_columns()
            .saturating_sub(self.content_offset.left + self.content_offset.right)
    }
    fn get_content_rows(&self) -> usize {
        // content rows might differ from the pane's rows if the pane has a frame
        // in that case they would be 2 less
        self.get_rows()
            .saturating_sub(self.content_offset.top + self.content_offset.bottom)
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
    }
    fn set_geom_override(&mut self, pane_geom: PaneGeom) {
        self.geom_override = Some(pane_geom);
        self.reflow_lines();
    }
    fn character_cell_size_possibly_changed(&mut self) {
        self.grid.character_cell_size_possibly_changed();
        self.render_full_viewport();
        self.set_should_render(true);
    }
    fn handle_pty_bytes(&mut self, bytes: VteBytes) {
        self.set_should_render(true);
        if self.forward_paused {
            // A host-forward initiated by this pane is outstanding.
            // Buffer the bytes; Tab drains them on resume.
            self.pending_pty_input.extend(bytes);
            return;
        }
        let mut forwarded: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut index = 0;
        let mut capture_started = false;
        while index < bytes.len() {
            // Between sequences the interceptor forwards every non-escape byte unchanged and stays
            // in the same state, so the whole run up to the next escape can be copied at once.
            // Ordinary program output is almost entirely such runs, and driving them through the
            // state machine one byte at a time - each one rebuilding a `ForwardBytes` and calling
            // `extend_from_slice` on a single byte - was the second-largest cost in pty ingest.
            if self.kitty_interceptor.is_in_ground_state() {
                let rest = &bytes[index..];
                let run_length = rest
                    .iter()
                    .position(|byte| *byte == 0x1b)
                    .unwrap_or(rest.len());
                if run_length > 0 {
                    forwarded.extend_from_slice(&rest[..run_length]);
                    index += run_length;
                    capture_started = false;
                    continue;
                }
            }
            let byte = bytes[index];
            index += 1;
            match self.kitty_interceptor.advance(byte) {
                InterceptorResult::Forward(fwd) => {
                    capture_started = false;
                    forwarded.extend_from_slice(fwd.as_slice());
                },
                InterceptorResult::Swallow => {
                    if !capture_started {
                        capture_started = true;
                        // Drain everything before the sequence now, so a pause
                        // in the middle of this flush can never strand a
                        // capture that has already left the input buffer.
                        if !forwarded.is_empty() {
                            let consumed = self
                                .vte_parser
                                .advance_until_terminated(&mut self.grid, &forwarded);
                            if consumed < forwarded.len() {
                                self.pending_pty_input.extend(&forwarded[consumed..]);
                                self.pending_pty_input.extend(&bytes[index - 1..]);
                                self.kitty_interceptor.reset();
                                return;
                            }
                            forwarded.clear();
                        }
                    }
                },
                InterceptorResult::Captured(cmd) => {
                    capture_started = false;
                    if !forwarded.is_empty() {
                        let consumed = self
                            .vte_parser
                            .advance_until_terminated(&mut self.grid, &forwarded);
                        if consumed < forwarded.len() {
                            self.pending_pty_input.extend(&forwarded[consumed..]);
                            self.pending_pty_input.extend(b"\x1b_G");
                            self.pending_pty_input.extend(&cmd);
                            self.pending_pty_input.extend(b"\x1b\\");
                            self.pending_pty_input.extend(&bytes[index..]);
                            return;
                        }
                        forwarded.clear();
                    }
                    self.grid.handle_kitty_apc(&cmd);
                    if !self.grid.pending_forwarded_queries.is_empty() {
                        // Grid produced a forward. Stop feeding; queue the
                        // un-fed remainder so Tab can replay it after the
                        // reply.
                        self.pending_pty_input.extend(&bytes[index..]);
                        return;
                    }
                },
            }
        }
        let consumed = self
            .vte_parser
            .advance_until_terminated(&mut self.grid, &forwarded);
        if consumed < forwarded.len() {
            self.pending_pty_input.extend(&forwarded[consumed..]);
        }
    }
    fn cursor_coordinates(&self, client_id: Option<ClientId>) -> Option<(usize, usize, bool)> {
        // (x, y, is_visible)
        if let Some(client_id) = client_id {
            if self.guest_modal.contains_key(&client_id) {
                return None;
            }
        }
        if self.get_content_rows() < 1 || self.get_content_columns() < 1 {
            // do not render cursor if there's no room for it
            return None;
        }
        let Offset { top, left, .. } = self.content_offset;
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
            // Zellij itself operates in bracketed paste mode, so the terminal sends these
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
                .or_else(|| match raw_input_bytes.as_slice() {
                    b"1" => Some('1'),
                    b"2" => Some('2'),
                    _ => None,
                });
            if is_up {
                self.guest_modal.insert(client_id, (selection + 1) % 2);
                self.set_should_render(true);
                Some(AdjustedInput::GuestModalSelectionChanged)
            } else if is_down {
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
    fn current_geom(&self) -> PaneGeom {
        self.geom_override.unwrap_or(self.geom)
    }
    fn geom_override(&self) -> Option<PaneGeom> {
        self.geom_override
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
                    self.set_should_render(false);
                    Ok(rendered_assets)
                },
                e => e,
            }
        } else {
            Ok(None)
        }
    }
    fn render_frame(
        &mut self,
        client_id: ClientId,
        mut frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>> {
        let err_context = || format!("failed to render frame for client {client_id}");
        frame_params.omit_title = frame_params.omit_title
            && !(input_mode == InputMode::RenamePane && frame_params.is_main_client);
        // TODO: remove the cursor stuff from here
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
        let pane_title = if frame_params.blank_title {
            String::new()
        } else if let Some(text_color_override) =
            frame_color_override.and_then(|override_| override_.text.as_ref())
        {
            text_color_override.into()
        } else {
            self.title_with_bell_indicator(normal_title)
        };

        let frame_geom = frame_params
            .frame_geom_override
            .unwrap_or_else(|| self.current_geom());
        let is_pinned = frame_geom.is_pinned;
        let is_main_client = frame_params.is_main_client;
        let mut frame = PaneFrame::new(
            frame_geom.into(),
            self.grid.scrollback_position_and_length(),
            pane_title,
            frame_params,
        )
        .is_pinned(is_pinned);
        if self.suspended {
            frame.indicate_suspended();
        }
        if let Some(since) = self.agent_attention_since {
            if is_main_client {
                // The client rendering this frame is focused here, so the wait is in front of
                // somebody: it has been noticed, and the corner has nothing to say. Focus
                // reaches a pane by a dozen routes and this is the one place that knows the
                // answer for certain - which also means a wait that arrives while the user is
                // already in the pane never blinks, the same rule the waiting toast keeps.
                self.agent_attention_since = None;
            } else {
                // The clock is read here rather than threaded down, exactly as the bell fade
                // reads it in `frame_color_override_strength`: a display detail, and at most a
                // fraction of a millisecond stale.
                frame.indicate_agent_attention(blink_is_lit(since, Instant::now()));
            }
        }
        if let Some((exit_status, is_first_run, _run_command)) = &self.is_held {
            if *is_first_run {
                frame.indicate_first_run();
            } else {
                frame.add_exit_status(exit_status.as_ref().copied());
            }
        }
        if let Some(override_) = frame_color_override {
            frame.override_color(override_.color);
        }

        let res = match self.frame.get(&client_id) {
            // TODO: use and_then or something?
            Some(last_frame) => {
                if &frame != last_frame {
                    if !self.borderless {
                        let frame_output = frame.render().with_context(err_context)?;
                        self.frame.insert(client_id, frame);
                        Some(frame_output)
                    } else {
                        None
                    }
                } else {
                    None
                }
            },
            None => {
                if !self.borderless {
                    let frame_output = frame.render().with_context(err_context)?;
                    self.frame.insert(client_id, frame);
                    Some(frame_output)
                } else {
                    None
                }
            },
        };
        Ok(res)
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
    fn render_terminal_title(&mut self, input_mode: InputMode) -> String {
        let pane_title = if self.pane_name.is_empty() && input_mode == InputMode::RenamePane {
            "Enter name..."
        } else if self.pane_name.is_empty() {
            self.grid.title.as_deref().unwrap_or("")
        } else {
            &self.pane_name
        };
        make_terminal_title(pane_title)
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
    fn scroll_up(&mut self, count: usize, _client_id: ClientId) {
        self.grid.move_viewport_up(count);
        self.set_should_render(true);
    }
    fn scroll_down(&mut self, count: usize, _client_id: ClientId) {
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
    fn drain_messages_to_pty(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.grid.pending_messages_to_pty)
    }

    fn drain_forwarded_queries(&mut self) -> Vec<crate::host_query::HostQuery> {
        std::mem::take(&mut self.grid.pending_forwarded_queries)
    }

    fn drain_nested_session_messages(&mut self) -> Vec<NestedSessionMessage> {
        std::mem::take(&mut self.grid.pending_nested_session_messages)
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

    fn set_guest_session_name(&mut self, session_name: Option<String>) {
        self.guest_session_name = session_name;
    }

    fn guest_session_name(&self) -> Option<String> {
        self.guest_session_name.clone()
    }

    fn set_guest_modal_shortcuts(&mut self, shortcuts: GuestModalShortcuts) {
        self.guest_modal_shortcuts = shortcuts;
    }

    fn guest_modal_shortcuts(&self) -> GuestModalShortcuts {
        self.guest_modal_shortcuts.clone()
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

    fn push_color_palette_dsr(&mut self, mode: zellij_utils::data::HostTerminalThemeMode) {
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
        let cursor_at_the_bottom = to.line.0 < 0 && should_scroll;
        let cursor_at_the_top = to.line.0 as usize >= self.grid.height && should_scroll;
        let cursor_in_the_middle = to.line.0 >= 0 && (to.line.0 as usize) < self.grid.height;

        // TODO: check how far up/down mouse is relative to pane, to increase scroll lines?
        if cursor_at_the_bottom {
            self.grid.scroll_up_one_line();
            self.selection_scrolled_at = time::Instant::now();
            self.set_should_render(true);
        } else if cursor_at_the_top {
            self.grid.scroll_down_one_line();
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

    fn set_content_offset(&mut self, offset: Offset) {
        self.content_offset = offset;
        self.reflow_lines();
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
            self.pane_name = self.prev_pane_name.clone()
        }
    }

    fn borderless(&self) -> bool {
        self.borderless
    }
    fn set_borderless(&mut self, should_be_borderless: bool) {
        self.borderless = should_be_borderless;
        if should_be_borderless {
            self.set_content_offset(Offset::default());
        } else {
            self.set_content_offset(Offset::frame(1));
        }
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
        self.grid.clear_search();
        if !self.search_term.is_empty() {
            self.grid.set_search_string(&self.search_term);
        }
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
    fn hold(&mut self, exit_status: Option<i32>, is_first_run: bool, run_command: RunCommand) {
        self.invoked_with = Some(Run::Command(run_command.clone()));
        // A held pane has no running process to be stopped, and a stale flag
        // would leave the resume hint up over a dead pane forever.
        self.suspended = false;
        self.is_held = Some((exit_status, is_first_run, run_command));
        if let Some(agent) = self.agent.as_mut() {
            agent.state = AgentState::Exited;
            agent.detail = None;
        }
        if let Some(notification_end) = self.notification_end.as_mut() {
            if let Some(exit_status) = exit_status {
                notification_end.set_exit_status(exit_status);

                // Check if unblock condition is met
                if let Some(condition) = notification_end.unblock_condition() {
                    if condition.is_met(exit_status) {
                        // Condition met - drop the NotificationEnd now to unblock
                        drop(self.notification_end.take());
                    }
                }
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
    fn set_bell_notification(&mut self, val: bool) {
        self.has_bell_notification = val;
    }
    fn get_bell_notification(&self) -> bool {
        self.has_bell_notification
    }
    fn add_red_pane_frame_color_override(&mut self, error_text: Option<String>) {
        // A command that failed is a fact about the pane, so every client sees it.
        self.pane_frame_color_override = Some(FrameColorOverride::new(
            self.style.colors.exit_code_error.base,
            error_text,
            None,
        ));
    }
    fn add_highlight_pane_frame_color_override(
        &mut self,
        text: Option<String>,
        client_id: Option<ClientId>,
    ) {
        self.pane_frame_color_override = Some(FrameColorOverride::new(
            self.style.colors.frame_highlight.emphasis_0,
            text,
            client_id,
        ));
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
            .is_none_or(|override_| override_.is_cleared_by(client_id))
        {
            self.pane_frame_color_override = None;
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
    fn invoked_with(&self) -> &Option<Run> {
        &self.invoked_with
    }
    fn set_notification_end(&mut self, notification_end: Option<NotificationEnd>) {
        self.notification_end = notification_end;
    }
    fn set_title(&mut self, title: String) {
        self.pane_title = title;
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
    fn agent_info(&self) -> Option<&AgentInfo> {
        self.agent.as_ref()
    }
    fn last_agent_cwd(&self) -> Option<&str> {
        self.last_agent_cwd.as_deref()
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
        if let Some(agent) = &agent {
            self.last_agent_kind = Some(agent.kind.clone());
            if agent.cwd.is_some() {
                self.last_agent_cwd = agent.cwd.clone();
            }
        }
        if self.agent == agent {
            return false;
        }
        // On the edge into a wait a person has to answer, and only there: a `waiting` report
        // repeating itself - which is every `Notification` behind a `PermissionRequest` -
        // must not restart the blink. This is the same edge `agent_hook_edges` reads.
        let was_waiting_on_a_person = self
            .agent
            .as_ref()
            .is_some_and(AgentInfo::is_waiting_on_a_person);
        let is_waiting_on_a_person = agent
            .as_ref()
            .is_some_and(AgentInfo::is_waiting_on_a_person);
        if !is_waiting_on_a_person {
            self.agent_attention_since = None;
        } else if !was_waiting_on_a_person {
            self.agent_attention_since = Some(Instant::now());
        }
        self.agent = agent;
        true
    }
    fn agent_attention_since(&self) -> Option<Instant> {
        self.agent_attention_since
    }
    fn acknowledge_agent_attention(&mut self) -> bool {
        self.agent_attention_since.take().is_some()
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
        if lead.is_empty() {
            self.current_title()
        } else if self.has_explicit_title() {
            format!("{}: {}", lead.join(" "), self.current_title())
        } else {
            // No explicit title means the title is "Pane #3" or the command the
            // pane was launched with, which says less than the address does — so
            // the address stands alone rather than introducing a placeholder.
            lead.join(" ")
        }
    }
    fn stack_list_entry_label(&self) -> String {
        self.title_with_bell_indicator(self.framed_title())
    }
    fn custom_title(&self) -> Option<String> {
        if self.pane_name.is_empty() {
            None
        } else {
            Some(self.pane_name.clone())
        }
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
    fn serialize(&self, scrollback_lines_to_serialize: Option<usize>) -> Option<String> {
        self.grid.serialize(scrollback_lines_to_serialize)
    }
    fn respawn(
        &mut self,
        replacement: Option<RunCommand>,
        keep_scrollback: bool,
    ) -> Option<RunCommand> {
        // if this is a command pane that has exited or is waiting to be rerun, will return the
        // command to spawn - its own, or the replacement - otherwise it is safe to assume this
        // is not the right sort of pane or that it is not in the right sort of state
        self.is_held.take().map(|(_, _, run_command)| {
            self.is_held = None;
            self.suspended = false;
            if let Some(agent) = self.agent.as_mut() {
                agent.state = AgentState::Busy;
            }
            if keep_scrollback {
                self.grid.reset_terminal_state_preserving_scrollback();
            } else {
                self.grid.reset_terminal_state();
            }
            self.set_should_render(true);
            self.remove_banner();
            match replacement {
                Some(replacement) => {
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
                        self.agent = None;
                        self.was_an_agent = false;
                        self.last_agent_kind = None;
                        self.last_agent_cwd = None;
                    }
                    self.invoked_with = Some(Run::Command(replacement.clone()));
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
        self.arrow_fonts = should_support_arrow_fonts;
        self.grid.update_arrow_fonts(should_support_arrow_fonts);
    }
    fn update_kitty_host_support(&mut self, supported: KittyHostSupport) {
        self.grid.update_kitty_host_support(supported);
    }
    fn update_sixel_host_support(&mut self, supported: bool) {
        self.grid.update_sixel_host_support(supported);
    }
    fn update_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
        self.frame.clear();
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
        self.geom.is_pinned = !self.geom.is_pinned;
    }
    fn set_pinned(&mut self, should_be_pinned: bool) {
        self.geom.is_pinned = should_be_pinned;
    }
    fn intercept_left_mouse_click(&mut self, position: &Position, client_id: ClientId) -> bool {
        if self.position_is_on_frame(position) {
            let relative_position = self.relative_position(position);
            if let Some(client_frame) = self.frame.get_mut(&client_id) {
                if client_frame.clicked_on_pinned(relative_position) {
                    self.toggle_pinned();
                    return true;
                }
            }
        }
        false
    }
    fn intercept_mouse_event_on_frame(&mut self, event: &MouseEvent, client_id: ClientId) -> bool {
        if self.position_is_on_frame(&event.position) {
            let relative_position = self.relative_position(&event.position);
            if let MouseEventType::Press = event.event_type {
                if let Some(client_frame) = self.frame.get_mut(&client_id) {
                    if client_frame.clicked_on_pinned(relative_position) {
                        self.toggle_pinned();
                        return true;
                    }
                }
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
            if let Some(condition) = notification_end.unblock_condition() {
                if condition.is_met(exit_status) {
                    // Condition met - drop the NotificationEnd now to unblock
                    drop(self.notification_end.take());
                }
            }
        }
    }
    fn set_plugin_regex_highlights(
        &mut self,
        plugin_id: u32,
        highlights: Vec<RegexHighlight>,
        style: &Style,
    ) {
        self.grid
            .set_plugin_regex_highlights(plugin_id, highlights, style);
        self.set_should_render(true);
    }
    fn clear_plugin_highlights(&mut self, plugin_id: u32) {
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
        u32,
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

/// The agent record a pane starts with: one for a pane a plugin opened *as an agent pane* -
/// its `originating_plugin` context carries `HOCUS_AGENT_PANE_CONTEXT`, whose value names the
/// kind, or is the legacy `"true"`, in which case the program's name stands in - and nothing
/// for any other pane. The agent itself has said nothing yet, so the source is `Spawned`.
fn agent_from_run(invoked_with: Option<&Run>) -> Option<AgentInfo> {
    let Some(Run::Command(run_command)) = invoked_with else {
        return None;
    };
    let context = &run_command.originating_plugin.as_ref()?.context;
    let marker = context.get(HOCUS_AGENT_PANE_CONTEXT)?;
    let kind = match marker.trim() {
        "" | "true" => zellij_utils::agent::agent_kind_of(&run_command.command, &run_command.args)?,
        kind => kind.to_owned(),
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
    /// The frame's title: the pane's id, then [`Pane::display_title`].
    ///
    /// The id leads because it is how the pane is addressed from outside — a
    /// `--pane-id` argument, hocus's agent list, a line in the log — and nothing
    /// else on screen says what it is. Bracketed so it reads as a label rather
    /// than as the first word of the title.
    ///
    /// Deliberately not folded into `display_title`: that is also a *tab's* name
    /// when a tab holds one pane, and a pane id is not a name for a tab.
    fn framed_title(&self) -> String {
        format!("[{}] {}", self.pid, self.display_title())
    }
    fn title_with_bell_indicator(&self, title: String) -> String {
        if self.has_bell_notification {
            format!("{} [!]", title)
        } else {
            title
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: u32,
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
        let initial_pane_title =
            initial_pane_title.unwrap_or_else(|| format!("Pane #{}", pane_index));
        let agent = agent_from_run(invoked_with.as_ref());
        let grid = Grid::new(
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
        if let Some(notification_end) = notification_end.as_mut() {
            notification_end.set_affected_pane_id(PaneId::Terminal(pid));
        }
        TerminalPane {
            frame: HashMap::new(),
            content_offset: Offset::default(),
            pid,
            grid,
            selectable: true,
            geom: position_and_size,
            geom_override: None,
            vte_parser: vte::Parser::new(),
            active_at: Instant::now(),
            style,
            selection_scrolled_at: time::Instant::now(),
            pane_title: initial_pane_title,
            pane_name: pane_name.clone(),
            prev_pane_name: pane_name,
            remote_host: None,
            title_prefix: None,
            was_an_agent: agent.is_some(),
            last_agent_kind: agent.as_ref().map(|agent| agent.kind.clone()),
            last_agent_cwd: agent.as_ref().and_then(|agent| agent.cwd.clone()),
            agent,
            // A pane the spawn path opened as an agent pane starts `Busy`, so there is
            // never a wait to draw before the first report arrives.
            agent_attention_since: None,
            borderless: false,
            exclude_from_sync: false,
            fake_cursor_locations: HashSet::new(),
            search_term: String::new(),
            is_held: None,
            suspended: false,
            banner: None,
            pane_frame_color_override: None,
            has_bell_notification: false,
            invoked_with,
            arrow_fonts,
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
    pub fn get_x(&self) -> usize {
        match self.geom_override {
            Some(position_and_size_override) => position_and_size_override.x,
            None => self.geom.x,
        }
    }
    pub fn get_y(&self) -> usize {
        match self.geom_override {
            Some(position_and_size_override) => position_and_size_override.y,
            None => self.geom.y,
        }
    }
    pub fn get_columns(&self) -> usize {
        match self.geom_override {
            Some(position_and_size_override) => position_and_size_override.cols.as_usize(),
            None => self.geom.cols.as_usize(),
        }
    }
    pub fn get_rows(&self) -> usize {
        match self.geom_override {
            Some(position_and_size_override) => position_and_size_override.rows.as_usize(),
            None => self.geom.rows.as_usize(),
        }
    }
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
                .and_then(|k| k.serialize_kitty())
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
                .and_then(|k| k.serialize_non_kitty())
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
        self.is_held
            .as_ref()
            .map(|_| AdjustedInput::ReRunCommandInThisPane)
    }
    fn handle_held_drop_to_shell(&mut self) -> Option<AdjustedInput> {
        self.is_held.take().map(|(_, _, run_command)| {
            // Drop to shell in the same working directory as the command was run
            let working_dir = run_command.cwd.clone();
            self.is_held = None;
            self.grid.reset_terminal_state();
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
