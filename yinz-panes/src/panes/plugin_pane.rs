use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Instant;
use yinz_runtime::shared::make_terminal_title;
use yinz_types::internal_ids::PluginId;
use yinz_types::mouse::{MouseEvent, MouseEventType};

use crate::output::{CharacterChunk, KittyImageChunk, SixelImageChunk};
use crate::pane::{AdjustedInput, Pane};
use crate::panes::{
    grid::Grid,
    kitty_graphics::{KittyApcInterceptor, KittyHostSupport, KittyImageStore, Piece},
    sixel::SixelImageStore,
    terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END},
    FrameColorOverride, LinkHandler, PaneId, EMPTY_TERMINAL_CHARACTER,
};
use crate::plugins::PluginInstruction;
use crate::pty::VteBytes;
use crate::ui::{
    loading_indication::LoadingIndication,
    pane_boundaries_frame::{FrameParams, PaneFrame},
};
use crate::ClientId;
use std::cell::RefCell;
use std::rc::Rc;
use vte;
use yinz_runtime::{channels::SenderWithContext, errors::prelude::*};
use yinz_types::data::PaneContents;
use yinz_types::geometry::Direction;
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_size::PaneGeom;
use yinz_types::pane_size::{Offset, SizeInPixels};
use yinz_types::position::Position;
use yinz_types::style::{Palette, Style, Styling};
use {
    yinz_types::data::Event,
    yinz_types::data::InputMode,
    yinz_types::data::Mouse,
    yinz_types::data::PaletteColor,
    yinz_types::input::layout::{Run, RunPluginLocation, RunPluginOrAlias},
};
macro_rules! get_or_create_grid {
    ($self:ident, $client_id:ident) => {{
        let rows = $self.get_content_rows();
        let cols = $self.get_content_columns();
        let osc8_hyperlinks = true; // N/A for plugins, always enabled
        let explicitly_disable_kitty_keyboard_protocol = false; // N/A for plugins

        $self.grids.entry($client_id).or_insert_with(|| {
            let mut grid = Grid::new(
                rows,
                cols,
                $self.terminal_emulator_colors.clone(),
                $self.terminal_emulator_color_codes.clone(),
                $self.link_handler.clone(),
                $self.character_cell_size.clone(),
                $self.sixel_image_store.clone(),
                $self.kitty_image_store.clone(),
                Style {
                    terminal: None,
                    ..$self.style
                },
                $self.debug,
                $self.arrow_fonts,
                $self.styled_underlines,
                osc8_hyperlinks,
                explicitly_disable_kitty_keyboard_protocol,
            );
            grid.hide_cursor();
            grid.update_kitty_host_support($self.kitty_host_support);
            grid
        })
    }};
}

pub struct PluginPane {
    pub pid: PluginId,
    pane_tabs_transfer: Option<crate::pane_tabs::PaneTabsTransfer>,
    pane_tabs_views: HashMap<ClientId, crate::pane_tabs::PaneTabsView>,
    pane_tabs: Option<yinz_types::pane_tabs::PaneTabsInfo>,
    pub pending_pane_tabs: Option<yinz_types::pane_tabs::PaneTabsLayout>,
    pub should_render: HashMap<ClientId, bool>,
    pub selectable: bool,
    pub geom: PaneGeom,
    configured_rows: yinz_types::pane_size::Dimension,
    pub geom_override: Option<PaneGeom>,
    pub content_offset: Offset,
    max_content_width: usize,
    render_width: Option<(usize, usize)>,
    subtitle: Option<String>,
    subtitle_enabled: bool,
    pub send_plugin_instructions: SenderWithContext<PluginInstruction>,
    pub active_at: Instant,
    pub pane_title: String,
    pub pane_name: String,
    pub style: Style,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    terminal_emulator_colors: Rc<RefCell<Palette>>,
    terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
    link_handler: Rc<RefCell<LinkHandler>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    vte_parsers: HashMap<ClientId, vte::Parser>,
    /// One per client, like the vte parser beside it: a plugin frame can in principle end
    /// mid-sequence, and each client's frames are a stream of their own.
    kitty_interceptors: HashMap<ClientId, KittyApcInterceptor>,
    /// What the host terminals can display, as last pushed down from `Screen`. Held on the pane
    /// as well as on each grid so a client that attaches later gets a correctly-configured grid.
    kitty_host_support: KittyHostSupport,
    grids: HashMap<ClientId, Grid>,
    cursor_visibility: HashMap<ClientId, Option<(usize, usize)>>,
    prev_pane_name: String,
    frame: HashMap<ClientId, PaneFrame>,
    borderless: bool,
    exclude_from_sync: bool,
    pane_frame_color_override: Option<FrameColorOverride>,
    /// The loading indicator as last written into the pane, so a clock tick that would draw the
    /// same thing writes nothing. See `progress_animation_offset`.
    last_loading_indication: Option<String>,
    invoked_with: Option<Run>,
    loading_indication: LoadingIndication,
    debug: bool,
    arrow_fonts: bool,
    styled_underlines: bool,
    should_be_suppressed: bool,
    text_being_pasted: Option<Vec<u8>>,
    supports_mouse_selection: bool,
}

impl PluginPane {
    fn tab_bar_rows(&self) -> usize {
        usize::from(
            self.pane_tabs.is_some()
                && self.rows()
                    > self.content_offset.top + self.content_offset.bottom + self.subtitle_rows(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: PluginId,
        position_and_size: PaneGeom,
        send_plugin_instructions: SenderWithContext<PluginInstruction>,
        title: String,
        pane_name: String,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        terminal_emulator_colors: Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
        link_handler: Rc<RefCell<LinkHandler>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        currently_connected_clients: Vec<ClientId>,
        style: Style,
        invoked_with: Option<Run>,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
    ) -> Self {
        let loading_indication = LoadingIndication::new(title.clone()).with_colors(style.colors);
        let mut plugin = PluginPane {
            pid,
            pane_tabs: None,
            pane_tabs_views: HashMap::new(),
            pane_tabs_transfer: None,
            pending_pane_tabs: None,
            should_render: HashMap::new(),
            selectable: true,
            geom: position_and_size,
            configured_rows: position_and_size.rows,
            geom_override: None,
            send_plugin_instructions,
            active_at: Instant::now(),
            frame: HashMap::new(),
            content_offset: Offset::default(),
            max_content_width: 0,
            render_width: None,
            subtitle: None,
            subtitle_enabled: true,
            pane_title: title,
            borderless: false,
            pane_name: pane_name.clone(),
            prev_pane_name: pane_name,
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            exclude_from_sync: false,
            link_handler,
            character_cell_size,
            sixel_image_store,
            kitty_image_store,
            vte_parsers: HashMap::new(),
            kitty_interceptors: HashMap::new(),
            kitty_host_support: KittyHostSupport::Supported,
            grids: HashMap::new(),
            cursor_visibility: HashMap::new(),
            style,
            pane_frame_color_override: None,
            last_loading_indication: None,
            invoked_with,
            loading_indication,
            debug,
            arrow_fonts,
            styled_underlines,
            should_be_suppressed: false,
            text_being_pasted: None,
            supports_mouse_selection: false,
        };
        for client_id in currently_connected_clients {
            plugin.add_client(client_id);
        }
        plugin
    }
}

impl Pane for PluginPane {
    fn host_colors(&self) -> (PaletteColor, PaletteColor) {
        let palette = self.terminal_emulator_colors.borrow();
        (palette.fg, palette.bg)
    }
    fn content_header_rows(&self) -> usize {
        self.subtitle_rows() + self.tab_bar_rows()
    }
    fn reset_size_and_position_override(&mut self) {
        self.geom_override = None;
        self.resize_grids();
        self.set_should_render(true);
    }
    fn set_geom(&mut self, position_and_size: PaneGeom) {
        let is_pinned = self.geom.is_pinned;
        self.geom = position_and_size;
        self.geom.is_pinned = is_pinned;
        self.resize_grids();
        self.set_should_render(true);
    }
    fn set_geom_override(&mut self, pane_geom: PaneGeom) {
        self.geom_override = Some(pane_geom);
        self.resize_grids();
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
        self.resize_grids();
        self.set_should_render(true);
    }
    fn character_cell_size_possibly_changed(&mut self) {
        for grid in self.grids.values_mut() {
            grid.character_cell_size_possibly_changed();
        }
        self.render_full_viewport();
        self.set_should_render(true);
    }
    fn handle_plugin_bytes(&mut self, client_id: ClientId, bytes: VteBytes) {
        self.set_client_should_render(client_id, true);

        let vte_bytes = bytes;

        let grid = get_or_create_grid!(self, client_id);

        // this is part of the plugin contract, whenever we update the plugin and call its render function, we delete the existing viewport
        // and scroll, reset the cursor position and make sure all the viewport is rendered
        grid.delete_viewport_and_scroll();
        grid.reset_cursor_position();
        grid.render_full_viewport();

        let vte_parser = self.vte_parsers.entry(client_id).or_default();
        let interceptor = self
            .kitty_interceptors
            .entry(client_id)
            .or_insert_with(KittyApcInterceptor::new);

        // Kitty graphics arrive as APC, which vte parses into its ignore-string state and drops,
        // so - exactly as `TerminalPane::handle_pty_bytes` does - the bytes are run past an
        // interceptor first and the captured commands handed to the grid directly. None of the
        // pause/forward machinery from that method applies here: a plugin pane has no pty, so
        // there is nothing to forward a query to and nothing to replay afterwards.
        let mut forwarded: Cow<'_, [u8]> = Cow::Borrowed(&[]);
        for piece in interceptor.split(&vte_bytes) {
            match piece {
                Piece::Run(run) if forwarded.is_empty() => forwarded = Cow::Borrowed(run),
                Piece::Run(run) => forwarded.to_mut().extend_from_slice(run),
                Piece::Forward(fwd) => forwarded.to_mut().extend_from_slice(fwd.as_slice()),
                Piece::CaptureStarted => {
                    // Everything before the command has to reach the grid first: a placement is
                    // positioned by where the cursor is when it arrives.
                    if !forwarded.is_empty() {
                        vte_parser.advance(grid, &forwarded);
                        forwarded = Cow::Borrowed(&[]);
                    }
                },
                Piece::Captured(command) => {
                    if let Some(Err(error)) = grid.handle_kitty_apc(&command) {
                        log::warn!(
                            "plugin kitty command failed: plugin={} client={} image={:?} placement={:?} code={:?} message={} cell={:?} host={:?} images={} bytes={}",
                            self.pid, client_id, error.image_id, error.placement_id,
                            error.code, error.message, *self.character_cell_size.borrow(),
                            self.kitty_host_support, self.kitty_image_store.borrow().image_count(),
                            self.kitty_image_store.borrow().total_bytes(),
                        );
                    }
                    // A plugin has no pty to answer on, so a reply or a forwarded query has
                    // nowhere to go and would otherwise accumulate for the life of the pane.
                    // Plugins should pass `q=2`; this is the belt for the ones that do not.
                    grid.pending_messages_to_pty.clear();
                    grid.pending_forwarded_queries.clear();
                },
            }
        }
        if !forwarded.is_empty() {
            vte_parser.advance(grid, &forwarded);
        }

        self.should_render.insert(client_id, true);
    }
    fn cursor_coordinates(&self, client_id: Option<ClientId>) -> Option<(usize, usize, bool)> {
        let own_content_columns = self.get_content_columns();
        let own_content_rows = self.get_content_rows();
        let Offset { top, left, .. } = self.content_offset;
        let top = top + self.content_header_rows();
        if client_id.is_some_and(|client| {
            self.pane_tabs.as_ref().is_some_and(|tabs| {
                tabs.bar_focused.contains(&client)
                    || tabs.tabs[tabs.active].plugin_id != Some(self.pid)
            })
        }) {
            return None;
        }
        if let Some((x, y)) = client_id
            .and_then(|client_id| self.grids.get(&client_id))
            .and_then(|grid| grid.text_selection_cursor())
        {
            // a keyboard selection puts the cursor on the end being moved, so that swapping the
            // ends is visible here too
            if x >= own_content_columns || y >= own_content_rows {
                return None;
            }
            return Some((x + left, y + top, true));
        }
        if let Some(coordinates) =
            client_id.and_then(|client_id| self.cursor_visibility.get(&client_id))
        {
            coordinates.and_then(|(x, y)| {
                if x >= own_content_columns || y >= own_content_rows {
                    None
                } else {
                    Some((x + left, y + top, true)) // plugins always show cursor when position is set
                }
            })
        } else {
            None
        }
    }
    fn adjust_input_to_terminal(
        &mut self,
        key_with_modifier: &Option<KeyWithModifier>,
        mut raw_input_bytes: Vec<u8>,
        _raw_input_bytes_are_kitty: bool,
        client_id: Option<ClientId>,
    ) -> Option<AdjustedInput> {
        if client_id
            .and_then(|c| self.grids.get(&c))
            .is_some_and(|g| g.has_selection())
        {
            // The next input clears a mouse selection's highlight, and only a bare Esc stops
            // there: it is how you let go of a selection, and passing it on as well would close
            // whatever the plugin has open. Anything else still reaches the plugin - swallowing
            // it lost the key after every copy, and for a paste it lost the bracketed-paste
            // opener, so the text arrived as keystrokes the plugin acted on one by one.
            self.reset_selection(client_id);
            let bare_esc = match key_with_modifier {
                Some(key) => key.bare_key == BareKey::Esc && key.has_no_modifiers(),
                None => raw_input_bytes == [27],
            };
            if bare_esc {
                return None;
            }
        }
        if let Some(key_with_modifier) = key_with_modifier {
            Some(AdjustedInput::WriteKeyToPlugin(key_with_modifier.clone()))
        } else if raw_input_bytes.as_slice() == BRACKETED_PASTE_BEGIN {
            self.text_being_pasted = Some(vec![]);
            None
        } else if raw_input_bytes.as_slice() == BRACKETED_PASTE_END {
            if let Some(text_being_pasted) = self.text_being_pasted.take() {
                match String::from_utf8(text_being_pasted) {
                    Ok(pasted_text) => {
                        let _ = self
                            .send_plugin_instructions
                            .send(PluginInstruction::Update(vec![(
                                Some(self.pid),
                                client_id,
                                Event::PastedText(pasted_text),
                            )]));
                    },
                    Err(e) => {
                        log::error!("Failed to convert pasted bytes as utf8 {:?}", e);
                    },
                }
            }
            None
        } else if let Some(pasted_text) = self.text_being_pasted.as_mut() {
            pasted_text.append(&mut raw_input_bytes);
            None
        } else {
            Some(AdjustedInput::WriteBytesToTerminal(raw_input_bytes))
        }
    }
    fn position_and_size(&self) -> PaneGeom {
        self.geom
    }
    fn geom_override(&self) -> Option<PaneGeom> {
        self.geom_override
    }
    fn add_client(&mut self, client_id: ClientId) {
        if !self.grids.contains_key(&client_id) {
            let bytes = if !self.loading_indication.ended || self.loading_indication.is_error() {
                self.loading_indication.to_string().into_bytes()
            } else {
                vec![]
            };
            // The normal byte handler also draws any pending permission prompt.
            self.handle_plugin_bytes(client_id, bytes);
        }
    }
    fn remove_client(&mut self, client_id: ClientId) {
        self.grids.remove(&client_id);
        self.vte_parsers.remove(&client_id);
        self.kitty_interceptors.remove(&client_id);
        self.should_render.remove(&client_id);
        self.cursor_visibility.remove(&client_id);
        self.frame.remove(&client_id);
        self.pane_tabs_views.remove(&client_id);
        self.clear_pane_frame_color_override(Some(client_id));
    }
    fn prepare_content_replay(&mut self, client_id: ClientId) {
        if let Some(grid) = self.grids.get_mut(&client_id) {
            grid.replay_last_render();
            self.should_render.insert(client_id, true);
        }
    }
    fn invalidate_frame(&mut self, client_id: ClientId) {
        self.frame.remove(&client_id);
    }
    fn should_render(&self) -> bool {
        // set should_render for all clients
        self.should_render.values().any(|v| *v)
    }
    fn set_should_render(&mut self, should_render: bool) {
        self.should_render
            .values_mut()
            .for_each(|v| *v = should_render);
    }
    fn render_full_viewport(&mut self) {
        // this marks the pane for a full re-render, rather than just rendering the
        // diff as it usually does with the OutputBuffer
        self.frame.clear();
        for grid in self.grids.values_mut() {
            grid.render_full_viewport();
        }
    }
    fn selectable(&self) -> bool {
        self.selectable
    }
    fn set_selectable(&mut self, selectable: bool) {
        self.selectable = selectable;
    }
    fn show_cursor(&mut self, client_id: ClientId, cursor_position: Option<(usize, usize)>) {
        self.cursor_visibility.insert(client_id, cursor_position);
        self.should_render.insert(client_id, true);
    }
    fn render(
        &mut self,
        client_id: Option<ClientId>,
    ) -> Result<
        Option<(
            Vec<CharacterChunk>,
            Option<String>,
            Vec<SixelImageChunk>,
            Vec<KittyImageChunk>,
        )>,
    > {
        let Some(client_id) = client_id else {
            return Ok(None);
        };
        if !self.should_render.get(&client_id).copied().unwrap_or(false) {
            return Ok(None);
        }
        let content_x = self.get_content_x();
        let content_y = self.get_content_y();
        let rows = self.get_content_rows();
        let columns = self.get_content_columns();
        let mut rendered = (vec![], None, vec![], vec![]);
        let selected_here = self
            .pane_tabs
            .as_ref()
            .is_none_or(|tabs| tabs.tabs[tabs.active].plugin_id == Some(self.pid));
        if selected_here && rows > 0 && columns > 0 {
            if let Some(grid) = self.grids.get_mut(&client_id) {
                rendered = grid
                    .render(content_x, content_y, &self.style)?
                    .unwrap_or(rendered);
            }
        } else {
            for row in 0..rows {
                rendered.0.push(CharacterChunk::new(
                    vec![EMPTY_TERMINAL_CHARACTER; columns],
                    content_x,
                    content_y + row,
                ));
            }
        }
        if self.tab_bar_rows() > 0
            && let Some(tabs) = &self.pane_tabs
        {
            let view = self
                .pane_tabs_views
                .get(&client_id)
                .copied()
                .unwrap_or_default();
            let characters = crate::pane_tabs::render_bar(
                tabs,
                view,
                tabs.bar_focused.contains(&client_id),
                columns,
                &self.style,
            );
            rendered
                .0
                .push(CharacterChunk::new(characters, content_x, content_y - 1));
            if selected_here
                && rows > 0
                && matches!(
                    tabs.tabs[tabs.active].state,
                    yinz_types::pane_tabs::PaneTabLoadState::Failed(_)
                )
            {
                let mut hint: Vec<_> = "r: retry"
                    .chars()
                    .take(columns)
                    .map(crate::panes::TerminalCharacter::new)
                    .collect();
                hint.resize(columns, EMPTY_TERMINAL_CHARACTER);
                rendered
                    .0
                    .push(CharacterChunk::new(hint, content_x, content_y + rows - 1));
            }
            if !selected_here && rows > 0 {
                let text = match &tabs.tabs[tabs.active].state {
                    yinz_types::pane_tabs::PaneTabLoadState::Loading => "Loading…".to_owned(),
                    yinz_types::pane_tabs::PaneTabLoadState::Failed(error) => {
                        format!("{error} (r: retry)")
                    },
                    _ => "Closed (r: reopen)".to_owned(),
                };
                rendered.0.push(CharacterChunk::new(
                    yinz_types::text::clean_string_from_control_and_linebreak(&text)
                        .chars()
                        .take(columns)
                        .map(crate::panes::TerminalCharacter::new)
                        .collect(),
                    content_x,
                    content_y,
                ));
            }
        }
        self.should_render.insert(client_id, false);
        Ok(Some(rendered))
    }

    fn render_frame(
        &mut self,
        client_id: ClientId,
        mut frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>> {
        if self.borderless {
            return Ok(None);
        }
        // Plugin output replaces its viewport; its grid's discarded rows are not scrollback.
        frame_params.show_scroll_indication = false;
        let subtitle = (self.subtitle_rows() > 0)
            .then(|| self.subtitle.clone())
            .flatten()
            .map(|text| (text, self.get_content_y() - self.tab_bar_rows() - 1));
        let frame_geom = frame_params
            .frame_geom_override
            .unwrap_or_else(|| self.render_geom());
        let err_context = || format!("failed to render frame for client {client_id}");
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
        } else if self.pane_name.is_empty()
            && input_mode == InputMode::RenamePane
            && frame_params.is_main_client
        {
            String::from("Enter name...")
        } else if self.pane_name.is_empty() {
            self.grids
                .get(&client_id)
                .and_then(|grid| grid.title.clone())
                .unwrap_or_else(|| self.pane_title.clone())
        } else {
            self.pane_name.clone()
        };

        let is_pinned = frame_geom.is_pinned;
        // A plugin scrolls its own content: the host grid's scrollback says nothing the
        // user can act on, and reporting it froze a `SCROLL: 0/N` into the title.
        let no_scroll_indication = (0, 0);
        let mut frame = PaneFrame::new(
            frame_geom.into(),
            no_scroll_indication,
            pane_title,
            frame_params,
        )
        .is_pinned(is_pinned);
        frame.subtitle = subtitle;

        if !is_pinned && self.frame.get(&client_id) == Some(&frame) {
            return Ok(None);
        }
        let frame_output = frame.render().with_context(err_context)?;
        self.frame.insert(client_id, frame);
        Ok(Some(frame_output))
    }
    fn render_fake_cursor(
        &mut self,
        _cursor_color: PaletteColor,
        _text_color: PaletteColor,
    ) -> Option<String> {
        None
    }
    fn render_terminal_title(&mut self, input_mode: InputMode) -> String {
        let pane_title = if self.pane_name.is_empty() && input_mode == InputMode::RenamePane {
            "Enter name..."
        } else if self.pane_name.is_empty() {
            &self.pane_title
        } else {
            &self.pane_name
        };
        make_terminal_title(pane_title)
    }
    fn update_name(&mut self, name: &str) {
        match name {
            "\0" => {
                self.pane_name = String::new();
            },
            "\u{007F}" | "\u{0008}" => {
                //delete and backspace keys
                self.pane_name.pop();
            },
            c => {
                self.pane_name.push_str(c);
            },
        }
        self.set_should_render(true);
    }
    fn pid(&self) -> PaneId {
        PaneId::Plugin(self.pid)
    }
    fn reduce_height(&mut self, percent: f64) {
        if let Some(p) = self.geom.rows.as_percent() {
            self.geom.rows.set_percent(p - percent);
            self.resize_grids();
            self.set_should_render(true);
        }
    }
    fn increase_height(&mut self, percent: f64) {
        if let Some(p) = self.geom.rows.as_percent() {
            self.geom.rows.set_percent(p + percent);
            self.resize_grids();
            self.set_should_render(true);
        }
    }
    fn reduce_width(&mut self, percent: f64) {
        if let Some(p) = self.geom.cols.as_percent() {
            self.geom.cols.set_percent(p - percent);
            self.resize_grids();
            self.set_should_render(true);
        }
    }
    fn increase_width(&mut self, percent: f64) {
        if let Some(p) = self.geom.cols.as_percent() {
            self.geom.cols.set_percent(p + percent);
            self.resize_grids();
            self.set_should_render(true);
        }
    }
    fn push_down(&mut self, count: usize) {
        self.geom.y += count;
        self.resize_grids();
        self.set_should_render(true);
    }
    fn push_right(&mut self, count: usize) {
        self.geom.x += count;
        self.resize_grids();
        self.set_should_render(true);
    }
    fn pull_left(&mut self, count: usize) {
        self.geom.x -= count;
        self.resize_grids();
        self.set_should_render(true);
    }
    fn pull_up(&mut self, count: usize) {
        self.geom.y -= count;
        self.resize_grids();
        self.set_should_render(true);
    }
    fn dump_screen(&self, full: bool, client_id: Option<ClientId>) -> String {
        client_id
            .and_then(|c| self.grids.get(&c))
            .map(|g| g.dump_screen(full))
            .unwrap_or_else(|| "".to_owned())
    }
    fn scroll_up(&mut self, count: usize, client_id: Option<ClientId>) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                client_id,
                Event::Mouse(Mouse::ScrollUp(count)),
            )]))
            .unwrap();
    }
    fn scroll_down(&mut self, count: usize, client_id: Option<ClientId>) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                client_id,
                Event::Mouse(Mouse::ScrollDown(count)),
            )]))
            .unwrap();
    }
    fn scroll_left(&mut self, count: usize, client_id: ClientId) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::ScrollLeft(count)),
            )]))
            .unwrap();
    }
    fn scroll_right(&mut self, count: usize, client_id: ClientId) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::ScrollRight(count)),
            )]))
            .unwrap();
    }
    fn clear_screen(&mut self) {
        // do nothing
    }
    fn clear_scroll(&mut self) {
        // noop
    }
    fn set_selection_options(&mut self, osc133_command_selection: bool, word_separators: &str) {
        for grid in self.grids.values_mut() {
            grid.set_selection_options(osc133_command_selection, word_separators);
        }
    }
    fn start_selection(&mut self, start: &Position, client_id: ClientId) {
        if self.supports_mouse_selection
            && let Some(grid) = self.grids.get_mut(&client_id)
        {
            grid.start_selection(start);
            self.set_should_render(true);
        }
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::LeftClick(start.line.get(), start.column.get())),
            )]))
            .unwrap();
    }
    fn update_selection(&mut self, position: &Position, client_id: ClientId) {
        if self.supports_mouse_selection
            && let Some(grid) = self.grids.get_mut(&client_id)
        {
            grid.update_selection(position);
            self.set_should_render(true);
        }
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::Hold(position.line.get(), position.column.get())),
            )]))
            .unwrap();
    }
    fn end_selection(&mut self, end: &Position, client_id: ClientId) {
        if self.supports_mouse_selection
            && let Some(grid) = self.grids.get_mut(&client_id)
        {
            grid.end_selection(end);
            // The grid only marks lines dirty; nothing else flips the pane's own
            // flag, and a selection finished by a double-click has no later
            // `update_selection` to do it.
            self.set_should_render(true);
        }
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::Release(end.line(), end.column())),
            )]))
            .unwrap();
    }
    fn reset_selection(&mut self, client_id: Option<ClientId>) {
        if let Some(grid) = client_id.and_then(|c| self.grids.get_mut(&c)) {
            grid.reset_selection();
            self.set_should_render(true);
        }
    }
    fn start_text_selection(&mut self, client_id: ClientId) {
        let position = self
            .cursor_visibility
            .get(&client_id)
            .copied()
            .flatten()
            .map(|(column, line)| Position::new(line as i32, column as u16))
            .unwrap_or_else(|| Position::new(self.get_content_rows().saturating_sub(1) as i32, 0));
        if self.supports_mouse_selection
            && let Some(grid) = self.grids.get_mut(&client_id)
        {
            grid.start_text_selection(position);
            self.set_should_render(true);
        }
    }
    fn move_text_selection(&mut self, direction: Direction, client_id: ClientId) {
        if self.supports_mouse_selection
            && let Some(grid) = self.grids.get_mut(&client_id)
        {
            grid.move_text_selection(direction);
            self.set_should_render(true);
        }
    }
    fn swap_text_selection_ends(&mut self, client_id: ClientId) {
        if self.supports_mouse_selection
            && let Some(grid) = self.grids.get_mut(&client_id)
        {
            grid.swap_text_selection_ends();
            self.set_should_render(true);
        }
    }
    fn remember_copied_text_selection(&mut self, client_id: ClientId) {
        if let Some(grid) = self.grids.get_mut(&client_id) {
            grid.remember_copied_text_selection();
        }
    }
    fn finish_text_selection(&mut self, client_id: ClientId) {
        if let Some(grid) = self.grids.get_mut(&client_id) {
            grid.finish_text_selection();
            self.set_should_render(true);
        }
    }
    fn supports_mouse_selection(&self) -> bool {
        self.supports_mouse_selection
    }

    fn get_selected_text(&self, client_id: ClientId) -> Option<String> {
        if let Some(grid) = self.grids.get(&client_id) {
            grid.get_selected_text()
        } else {
            None
        }
    }
    fn is_scrolled(&self) -> bool {
        false
    }

    fn active_at(&self) -> Instant {
        self.active_at
    }

    fn set_active_at(&mut self, time: Instant) {
        self.active_at = time;
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
        self.resize_grids();
        self.render_full_viewport();
        true
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
        self.resize_grids();
        self.render_full_viewport();
        true
    }
    fn set_content_offset(&mut self, offset: Offset) {
        self.content_offset = offset;
        self.resize_grids();
    }

    fn set_subtitle(&mut self, subtitle: Option<String>) {
        let subtitle = subtitle
            .map(|text| yinz_types::text::clean_string_from_control_and_linebreak(&text))
            .filter(|text| !text.is_empty());
        if self.subtitle != subtitle {
            let rows = self.subtitle_rows();
            self.subtitle = subtitle;
            if rows != self.subtitle_rows() {
                self.resize_grids();
            }
            self.set_should_render(true);
        }
    }
    fn take_pane_tabs_transfer(&mut self) -> Option<crate::pane_tabs::PaneTabsTransfer> {
        self.pane_tabs_transfer.take()
    }
    fn set_pane_tabs_transfer(&mut self, transfer: crate::pane_tabs::PaneTabsTransfer) {
        self.pane_tabs_transfer = Some(transfer);
    }
    fn take_pane_tabs_layout(&mut self) -> Option<yinz_types::pane_tabs::PaneTabsLayout> {
        self.pending_pane_tabs.take()
    }
    fn set_pane_tabs_views(&mut self, views: HashMap<ClientId, crate::pane_tabs::PaneTabsView>) {
        if self.pane_tabs_views != views {
            self.pane_tabs_views = views;
            self.set_should_render(true);
        }
    }
    fn set_pane_tabs(&mut self, tabs: Option<yinz_types::pane_tabs::PaneTabsInfo>) {
        if self.pane_tabs != tabs {
            let had_bar = self.pane_tabs.is_some();
            self.pane_tabs = tabs;
            if had_bar != self.pane_tabs.is_some() {
                self.resize_grids();
            }
            self.set_should_render(true);
        }
    }
    fn pane_tabs(&self) -> Option<&yinz_types::pane_tabs::PaneTabsInfo> {
        self.pane_tabs.as_ref()
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
    fn borderless(&self) -> bool {
        self.borderless
    }
    fn set_exclude_from_sync(&mut self, exclude_from_sync: bool) {
        self.exclude_from_sync = exclude_from_sync;
    }
    fn exclude_from_sync(&self) -> bool {
        self.exclude_from_sync
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
        let override_ =
            FrameColorOverride::new(self.style.colors.frame_highlight.base, text, client_id);
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
    fn update_plugin_location(&mut self, location: RunPluginLocation) {
        let plugin = match &mut self.invoked_with {
            Some(Run::Plugin(RunPluginOrAlias::RunPlugin(plugin))) => Some(plugin),
            Some(Run::Plugin(RunPluginOrAlias::Alias(alias))) => alias.run_plugin.as_mut(),
            _ => None,
        };
        if let Some(plugin) = plugin {
            plugin.location = location;
        }
    }
    fn invoked_with(&self) -> &Option<Run> {
        &self.invoked_with
    }
    fn configured_rows(&self) -> yinz_types::pane_size::Dimension {
        self.configured_rows
    }
    fn set_configured_rows(&mut self, rows: yinz_types::pane_size::Dimension) {
        self.configured_rows = rows;
    }
    fn set_title(&mut self, title: String) {
        if self.pane_title != title {
            self.pane_title = title;
            self.set_should_render(true);
        }
    }
    fn update_loading_indication(&mut self, loading_indication: LoadingIndication) {
        if self.loading_indication.ended && !loading_indication.is_error() {
            return;
        }
        self.loading_indication.merge(loading_indication);
        self.handle_plugin_bytes_for_all_clients(
            self.loading_indication.to_string().as_bytes().to_vec(),
        );
    }
    fn start_loading_indication(&mut self, loading_indication: LoadingIndication) {
        for grid in self.grids.values_mut() {
            grid.reset_terminal_state();
        }
        self.vte_parsers.clear();
        self.kitty_interceptors.clear();
        self.loading_indication.merge(loading_indication);
        self.handle_plugin_bytes_for_all_clients(
            self.loading_indication.to_string().as_bytes().to_vec(),
        );
    }
    fn progress_animation_offset(&mut self) {
        if self.loading_indication.ended {
            return;
        }
        // The shared clock ticks far faster than the indicator changes, so this is a no-op on most
        // frames - without the comparison a 16ms clock would repaint the whole plugin grid sixty
        // times a second to draw the same four characters.
        //
        // Compared on the rendered string rather than on the dot count, because the dot count is
        // not the only thing that moves: the indicator draws nothing at all for its first 400ms
        // (see `LoadingIndication::fmt`), and gating on the count alone would skip the frame where
        // it first becomes visible and then sit on that decision until the next dot.
        let rendered = self.loading_indication.to_string();
        if self.last_loading_indication.as_deref() == Some(rendered.as_str()) {
            return;
        }
        let bytes = rendered.as_bytes().to_vec();
        self.last_loading_indication = Some(rendered);
        self.handle_plugin_bytes_for_all_clients(bytes);
    }
    fn current_title(&self) -> String {
        if self.pane_name.is_empty() {
            self.pane_title.to_owned()
        } else {
            self.pane_name.to_owned()
        }
    }
    fn custom_title(&self) -> Option<String> {
        if self.pane_name.is_empty() {
            None
        } else {
            Some(self.pane_name.clone())
        }
    }
    fn rename(&mut self, buf: Vec<u8>) {
        self.pane_name = String::from_utf8_lossy(&buf).to_string();
        self.set_should_render(true);
    }
    fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
        for grid in self.grids.values_mut() {
            grid.update_theme(theme);
        }
    }
    fn update_kitty_host_support(&mut self, supported: KittyHostSupport) {
        self.kitty_host_support = supported;
        for grid in self.grids.values_mut() {
            grid.update_kitty_host_support(supported);
        }
    }
    fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        self.arrow_fonts = should_support_arrow_fonts;
        for grid in self.grids.values_mut() {
            grid.update_arrow_fonts(should_support_arrow_fonts);
        }
        self.set_should_render(true);
    }
    fn update_render_options(&mut self, styled_underlines: bool, _osc8_hyperlinks: bool) {
        // links are always on for a plugin, see `get_or_create_grid`
        self.styled_underlines = styled_underlines;
        for grid in self.grids.values_mut() {
            grid.update_render_options(styled_underlines, true);
        }
        self.set_should_render(true);
    }
    fn update_rounded_corners(&mut self, rounded_corners: bool) {
        if self.style.rounded_corners != rounded_corners {
            self.style.rounded_corners = rounded_corners;
            self.frame.clear();
            self.set_should_render(true);
        }
    }
    fn set_should_be_suppressed(&mut self, should_be_suppressed: bool) {
        self.should_be_suppressed = should_be_suppressed;
    }
    fn query_should_be_suppressed(&self) -> bool {
        self.should_be_suppressed
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
    fn mouse_event(&self, event: &MouseEvent, client_id: ClientId) -> Option<String> {
        let (line, column) = (event.position.line(), event.position.column());
        let mouse = match event.event_type {
            MouseEventType::Motion
                if !event.left
                    && !event.right
                    && !event.middle
                    && !event.wheel_up
                    && !event.wheel_down =>
            {
                Mouse::Hover(line, column)
            },
            // The tab sends a right press here only on the focused pane's content, and only when
            // no context menu opened for it. The release and any drag that follow are not clicks.
            MouseEventType::Press if event.right => Mouse::RightClick(line, column),
            _ => return None,
        };
        let _ = self
            .send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(mouse),
            )]));
        None
    }
    fn set_mouse_selection_support(&mut self, selection_support: bool) {
        self.supports_mouse_selection = selection_support;
        if !selection_support {
            let client_ids_with_grids: Vec<ClientId> = self.grids.keys().copied().collect();
            for client_id in client_ids_with_grids {
                self.reset_selection(Some(client_id));
            }
        }
    }
    fn pane_contents(
        &self,
        client_id: Option<ClientId>,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        // The caller chooses a client: cached grids can belong to watchers or departed clients.
        client_id
            .and_then(|c| self.grids.get(&c))
            .map(|g| g.pane_contents(get_full_scrollback, max_scrollback_lines))
            .unwrap_or_default()
    }
    fn pane_contents_with_ansi(
        &self,
        client_id: Option<ClientId>,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        client_id
            .and_then(|c| self.grids.get(&c))
            .map(|g| g.pane_contents_with_ansi(get_full_scrollback, max_scrollback_lines))
            .unwrap_or_default()
    }
}

impl PluginPane {
    fn resize_grids(&mut self) {
        let content_rows = self.get_content_rows();
        let content_columns = self.get_content_columns();
        for grid in self.grids.values_mut() {
            grid.change_size(content_rows, content_columns);
        }
        self.set_should_render(true);
    }
    fn set_client_should_render(&mut self, client_id: ClientId, should_render: bool) {
        self.should_render.insert(client_id, should_render);
    }
    fn handle_plugin_bytes_for_all_clients(&mut self, bytes: VteBytes) {
        let client_ids: Vec<ClientId> = self.grids.keys().copied().collect();
        for client_id in client_ids {
            self.handle_plugin_bytes(client_id, bytes.clone());
        }
    }
}

#[cfg(test)]
#[path = "./unit/plugin_pane_frame_tests.rs"]
mod frame_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use yinz_runtime::channels;
    use yinz_tile::ui_components::{image_row, place_image_sequence, png_transmit_sequence};
    use yinz_types::pane_size::Dimension;
    use yinz_types::ui_components::UI_COMPONENT_DCS_PREFIX;

    pub(super) fn test_plugin_pane(rows: usize, cols: usize, content_offset: Offset) -> PluginPane {
        test_plugin_pane_with_cell_size(rows, cols, content_offset, None)
    }

    pub(super) fn frame_params() -> FrameParams {
        FrameParams {
            focused_client: Some(ClientId::new(1)),
            is_main_client: true,
            other_focused_clients: vec![],
            style: Style::default(),
            color: None,
            other_cursors_exist_in_session: false,
            pane_is_stacked_over: false,
            pane_is_stacked_under: false,
            pane_is_stacked: false,
            should_draw_pane_frames: true,
            pane_is_floating: false,
            show_pinned_indication: false,
            show_scroll_indication: true,
            content_offset: Offset::frame(1),
            mouse_is_hovering_over_pane: false,
            pane_is_selectable: true,
            show_help_text: false,
            highlight_tooltip: None,
            omit_title: false,
            frame_geom_override: None,
            stack_list_entry: None,
            blank_title: false,
            same_worktree: false,
            mouse_scroll_resize: false,
            dimmed: false,
            guest_choice_indicator: None,
            opening_ripple: None,
        }
    }

    fn rendered_text(chunks: &[CharacterChunk]) -> String {
        chunks
            .iter()
            .flat_map(|chunk| chunk.terminal_characters.iter().map(|c| c.character))
            .collect()
    }

    #[test]
    fn ending_a_selection_marks_the_pane_for_rendering() {
        // `start_selection` and `update_selection` set the flag; `end_selection` did
        // not, so a double-click word selection stayed invisible until an unrelated
        // event repainted the pane.
        let (mut pane, _plugin_rx) =
            test_plugin_pane_and_plugin_receiver(5, 40, Offset::frame(1), None);
        let client_id = ClientId::new(1);
        pane.handle_plugin_bytes(client_id, b"some text to select".to_vec());
        pane.set_mouse_selection_support(true);
        pane.start_selection(&Position::new(0, 0), client_id);
        pane.set_should_render(false);

        pane.end_selection(&Position::new(0, 4), client_id);

        assert!(pane.should_render());
    }

    /// A pane with a finished mouse selection over its first word, as a drag-to-copy leaves it.
    fn pane_with_a_selection() -> (
        PluginPane,
        channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
    ) {
        let (mut pane, plugin_rx) =
            test_plugin_pane_and_plugin_receiver(5, 40, Offset::frame(1), None);
        let client_id = ClientId::new(1);
        pane.handle_plugin_bytes(client_id, b"some text to select".to_vec());
        pane.set_mouse_selection_support(true);
        pane.start_selection(&Position::new(0, 0), client_id);
        pane.update_selection(&Position::new(0, 4), client_id);
        pane.end_selection(&Position::new(0, 4), client_id);
        assert!(pane.grids[&client_id].has_selection());
        (pane, plugin_rx)
    }

    /// The first key after a drag-to-copy used to be spent clearing the highlight, so the `j`,
    /// arrow or Tab that followed every copy did nothing.
    #[test]
    fn a_key_after_a_selection_clears_it_and_still_reaches_the_plugin() {
        let (mut pane, _plugin_rx) = pane_with_a_selection();
        let client_id = ClientId::new(1);
        let j = KeyWithModifier::new(BareKey::Char('j'));

        let adjusted =
            pane.adjust_input_to_terminal(&Some(j.clone()), b"j".to_vec(), false, Some(client_id));

        assert!(
            matches!(&adjusted, Some(AdjustedInput::WriteKeyToPlugin(key)) if *key == j),
            "{adjusted:?}"
        );
        assert!(!pane.grids[&client_id].has_selection());
    }

    /// Esc is how you let go of a selection, so it stops at clearing it: passed on as well, it
    /// would close whatever the plugin has open.
    #[test]
    fn esc_after_a_selection_only_clears_it() {
        for key in [Some(KeyWithModifier::new(BareKey::Esc)), None] {
            let (mut pane, _plugin_rx) = pane_with_a_selection();
            let client_id = ClientId::new(1);

            let adjusted = pane.adjust_input_to_terminal(&key, vec![27], false, Some(client_id));

            assert!(adjusted.is_none(), "{key:?}: {adjusted:?}");
            assert!(!pane.grids[&client_id].has_selection());
        }
    }

    /// A paste after a selection used to lose its bracketed-paste opener to the clearing, so the
    /// text arrived as keystrokes - a newline as Enter, letters as commands.
    #[test]
    fn a_paste_after_a_selection_still_arrives_as_a_paste() {
        let (mut pane, _plugin_rx) = pane_with_a_selection();
        let client_id = ClientId::new(1);

        let adjusted = pane.adjust_input_to_terminal(
            &None,
            BRACKETED_PASTE_BEGIN.to_vec(),
            false,
            Some(client_id),
        );

        assert!(adjusted.is_none(), "{adjusted:?}");
        assert_eq!(pane.text_being_pasted, Some(vec![]));
        assert!(!pane.grids[&client_id].has_selection());
    }

    #[test]
    fn a_plugin_pane_frame_never_shows_a_scroll_indicator() {
        // The host grid keeps a scrollback for a plugin that renders more rows than fit,
        // but the host cannot scroll a plugin pane - `scroll_up` only forwards a mouse
        // event to the plugin - so the indicator sat at `SCROLL: 0/N` for good.
        let mut pane = test_plugin_pane(5, 60, Offset::frame(1));
        let client_id = ClientId::new(1);
        let many_rows: String = (0..30).map(|i| format!("row {i}\r\n")).collect();
        pane.handle_plugin_bytes(client_id, many_rows.into_bytes());
        let scrollback = pane.grids[&client_id].scrollback_position_and_length().1;
        assert!(
            scrollback > 0,
            "precondition: the host grid holds scrollback"
        );

        let (frame, _) = pane
            .render_frame(client_id, frame_params(), InputMode::Normal)
            .unwrap()
            .expect("a framed pane renders a frame");

        assert!(
            !rendered_text(&frame).contains("SCROLL"),
            "{}",
            rendered_text(&frame)
        );
    }

    #[test]
    fn subtitle_resizes_plugin_content_and_omits_small_or_borderless_panes() {
        for offset in [
            Offset::frame(1),
            Offset::shift_right_top_and_bottom(0, 1, 0),
        ] {
            let mut pane = test_plugin_pane(6, 20, offset);
            let before = pane.get_content_rows();
            pane.set_subtitle(Some("review\n · tests\t · running".into()));
            assert_eq!(pane.subtitle(), Some("review · tests · running"));
            assert_eq!(pane.get_content_rows(), before - 1);
            assert_eq!(pane.get_content_y(), offset.top + 1);
            let mut geom = pane.current_geom();
            geom.rows = Dimension::fixed(offset.top + offset.bottom + 1);
            pane.set_geom(geom);
            assert_eq!(pane.get_content_rows(), 1);
            assert_eq!(pane.subtitle_rows(), 0);
            geom.rows = Dimension::fixed(6);
            geom.cols = Dimension::fixed(offset.left + offset.right);
            pane.set_geom(geom);
            assert_eq!(pane.subtitle_rows(), 0);
            geom.cols = Dimension::fixed(20);
            pane.set_geom(geom);
            assert_eq!(pane.subtitle_rows(), 1);
            pane.set_subtitle(None);
            assert_eq!(pane.get_content_rows(), before);
            pane.set_subtitle(Some("again".into()));
            pane.set_borderless(true);
            assert_eq!(pane.subtitle_rows(), 0);
            assert_eq!(pane.get_content_rows(), 6);
        }
    }

    /// Placeholder placements are only resolved once the character cell size is known
    /// (`Grid::kitty_refresh_placeholder_placements`), so an image test has to supply one.
    fn test_plugin_pane_with_cell_size(
        rows: usize,
        cols: usize,
        content_offset: Offset,
        cell_size: Option<SizeInPixels>,
    ) -> PluginPane {
        test_plugin_pane_and_plugin_receiver(rows, cols, content_offset, cell_size).0
    }

    /// Also hands back the plugin thread's receiver, for a test that drives a path which
    /// sends to it (the mouse handlers `unwrap` that send).
    pub(super) fn test_plugin_pane_and_plugin_receiver(
        rows: usize,
        cols: usize,
        content_offset: Offset,
        cell_size: Option<SizeInPixels>,
    ) -> (
        PluginPane,
        channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
    ) {
        let (plugin_tx, plugin_rx) = channels::unbounded();
        let geom = PaneGeom {
            rows: Dimension::fixed(rows),
            cols: Dimension::fixed(cols),
            ..Default::default()
        };
        let mut pane = PluginPane::new(
            PluginId::new(1),
            geom,
            SenderWithContext::new(plugin_tx),
            "test".to_owned(),
            "test".to_owned(),
            Rc::new(RefCell::new(SixelImageStore::default())),
            Rc::new(RefCell::new(KittyImageStore::default())),
            Rc::new(RefCell::new(Palette::default())),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(LinkHandler::new())),
            Rc::new(RefCell::new(cell_size)),
            vec![],
            Style::default(),
            None,
            false,
            false,
            false,
        );
        pane.set_content_offset(content_offset);
        (pane, plugin_rx)
    }

    // regression test: a content offset (the border/title reservation a framed pane applies)
    // used to make `cursor_coordinates` reject any cursor on the
    // last content row/column, because it bounds-checked the offset-shifted coordinates
    // against the un-shifted content extents.
    #[test]
    fn cursor_coordinates_are_shown_on_the_last_content_row_with_a_top_offset() {
        let mut pane = test_plugin_pane(
            10,
            10,
            Offset {
                top: 1,
                bottom: 0,
                left: 0,
                right: 0,
            },
        );
        let last_content_row = pane.get_content_rows() - 1;
        pane.show_cursor(ClientId::new(1), Some((0, last_content_row)));
        assert_eq!(
            pane.cursor_coordinates(Some(ClientId::new(1))),
            Some((0, last_content_row + 1, true))
        );
    }

    #[test]
    fn cursor_coordinates_are_shown_on_the_last_content_column_with_a_left_offset() {
        let mut pane = test_plugin_pane(
            10,
            10,
            Offset {
                top: 0,
                bottom: 0,
                left: 1,
                right: 0,
            },
        );
        let last_content_column = pane.get_content_columns() - 1;
        pane.show_cursor(ClientId::new(1), Some((last_content_column, 0)));
        assert_eq!(
            pane.cursor_coordinates(Some(ClientId::new(1))),
            Some((last_content_column + 1, 0, true))
        );
    }

    #[test]
    fn a_keyboard_selection_moves_the_cursor_to_the_end_being_moved() {
        let mut pane = test_plugin_pane(
            10,
            10,
            Offset {
                top: 1,
                bottom: 0,
                left: 1,
                right: 0,
            },
        );
        pane.set_mouse_selection_support(true);
        pane.handle_plugin_bytes(ClientId::new(1), b"abcdef".to_vec());
        pane.show_cursor(ClientId::new(1), Some((0, 0)));

        pane.start_text_selection(ClientId::new(1));
        pane.move_text_selection(Direction::Right, ClientId::new(1));
        pane.move_text_selection(Direction::Right, ClientId::new(1));
        assert_eq!(
            pane.cursor_coordinates(Some(ClientId::new(1))),
            Some((3, 1, true)),
            "the cursor follows the selection's live end, offset by the frame"
        );

        pane.swap_text_selection_ends(ClientId::new(1));
        assert_eq!(
            pane.cursor_coordinates(Some(ClientId::new(1))),
            Some((1, 1, true)),
            "swapping the ends is visible on a plugin pane too"
        );

        pane.finish_text_selection(ClientId::new(1));
        assert_eq!(
            pane.cursor_coordinates(Some(ClientId::new(1))),
            Some((1, 1, true)),
            "the plugin's own cursor is back once the selection is finished"
        );
    }

    fn ytext(x: usize, y: usize, width: usize, text: &str) -> String {
        let bytes = text
            .as_bytes()
            .iter()
            .map(|byte| byte.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!("{UI_COMPONENT_DCS_PREFIX}text;{x}/{y}/{width}/;{bytes}\u{1b}\\")
    }

    // regression test: the host pads a component out to the width it is given, so a
    // component asked for at an indented `x` with the pane's full width used to run past
    // the right edge and wrap. On the bottom row - where a dialog sized to exactly the
    // rows it asked for draws its help line - the wrap scrolled the whole frame up by
    // one line, and every widget then sat one row above the rectangle the plugin
    // hit-tests clicks against.
    #[test]
    fn a_component_wider_than_the_room_to_its_right_does_not_scroll_the_pane() {
        let mut pane = test_plugin_pane(5, 20, Offset::default());
        let rows = pane.get_content_rows();
        let columns = pane.get_content_columns();
        let frame = format!(
            "{}{}",
            ytext(0, 0, columns, "top"),
            ytext(5, rows - 1, columns, "bottom"),
        );
        pane.handle_plugin_bytes(ClientId::new(1), frame.into_bytes());

        // `Grid`'s `Debug` prints one viewport row per line, each behind a `00 (C): `
        // index prefix.
        let rendered = format!(
            "{:?}",
            pane.grids
                .get(&ClientId::new(1))
                .expect("the grid was created")
        );
        let row_content = |line: &str| {
            line.split_once("): ")
                .map(|(_, content)| content.trim_end().to_owned())
                .unwrap_or_default()
        };
        let lines: Vec<String> = rendered.lines().map(row_content).collect();
        assert_eq!(lines.first().map(String::as_str), Some("top"), "{rendered}");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("     bottom"),
            "{rendered}"
        );
    }

    #[test]
    fn cursor_coordinates_are_none_past_the_content_area() {
        let mut pane = test_plugin_pane(
            10,
            10,
            Offset {
                top: 1,
                bottom: 0,
                left: 0,
                right: 0,
            },
        );
        let content_rows = pane.get_content_rows();
        pane.show_cursor(ClientId::new(1), Some((0, content_rows)));
        assert_eq!(pane.cursor_coordinates(Some(ClientId::new(1))), None);
    }

    /// The frame a plugin drawing an image emits, built with the very helpers a plugin calls -
    /// A second, hand-written copy of the
    /// escape sequences here would pass while the two halves disagreed.
    pub(super) fn image_frame(id: u32, cols: usize, png: &[u8]) -> Vec<u8> {
        let mut frame = png_transmit_sequence(id, png);
        frame.push_str(&place_image_sequence(id, cols, 1));
        frame.push_str(&image_row(id, 0, cols));
        frame.into_bytes()
    }

    /// A PNG wide enough that its base64 does not fit in one command, so `transmit_png` has to
    /// chunk it - which is the shape agent's wand frames arrive in.
    fn chunky_png() -> Vec<u8> {
        let (width, height) = (64u32, 64u32);
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            // Noise rather than a flat colour: a flat image compresses to a single chunk and the
            // chunking test would silently stop testing chunking.
            let pixels: Vec<u8> = (0..(width * height * 4))
                .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
                .collect();
            writer.write_image_data(&pixels).expect("png data");
        }
        png
    }

    /// A 2x2 PNG, small enough to go over in one command.
    pub(super) fn tiny_png() -> Vec<u8> {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&[0; 16]).expect("png data");
        }
        png
    }

    pub(super) fn kitty_chunks(pane: &mut PluginPane, client_id: ClientId) -> Vec<KittyImageChunk> {
        pane.render(Some(client_id))
            .expect("render failed")
            .map(|(_, _, _, kitty)| kitty)
            .unwrap_or_default()
    }

    #[test]
    fn a_plugin_can_place_an_image_with_unicode_placeholders() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 8,
                height: 16,
            }),
        );
        pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
        let chunks = kitty_chunks(&mut pane, ClientId::new(1));
        // A contiguous placeholder run resolves to one chunk covering the cells it spans, not one
        // chunk per cell.
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert_eq!(chunks[0].dest_cells, (2, 1));
        assert_eq!((chunks[0].cell_x, chunks[0].cell_y), (0, 0));
    }

    #[test]
    fn plugin_cleanup_releases_only_its_own_images_and_all_caches() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 10,
                height: 20,
            }),
        );
        let store = pane.kitty_image_store.clone();
        let mut surviving = Vec::new();
        for client in [ClientId::new(1), ClientId::new(2)] {
            pane.handle_plugin_bytes(client, image_frame(42, 2, &tiny_png()));
            let chunks = kitty_chunks(&mut pane, client);
            assert_eq!(chunks.len(), 1);
            let chunk = &chunks[0];
            store
                .borrow_mut()
                .zlib_base64_for(chunk.internal_image_id, Some(chunk.variant_key))
                .unwrap()
                .unwrap();
            store
                .borrow_mut()
                .write_frame(chunk.internal_image_id, None, vec![7; 16], 40)
                .unwrap();
            if client == ClientId::new(2) {
                surviving.push(chunk.internal_image_id);
            }
            // A virtual-only image and an image transmitted without any placement.
            let mut frame = png_transmit_sequence(43, &tiny_png());
            frame.push_str(&place_image_sequence(43, 2, 1));
            frame.push_str(&png_transmit_sequence(44, &tiny_png()));
            frame.push_str(&image_row(42, 0, 2));
            pane.handle_plugin_bytes(client, frame.into_bytes());
            assert_eq!(kitty_chunks(&mut pane, client).len(), 1);
        }
        for id in &surviving {
            store
                .borrow_mut()
                .zlib_base64_for(*id, None)
                .unwrap()
                .unwrap();
        }
        assert_eq!(store.borrow().image_count(), 6);
        pane.remove_client(ClientId::new(1));
        assert_eq!(store.borrow().image_count(), 3);
        for id in surviving {
            assert!(store.borrow().get(id).is_some());
        }
        drop(pane);
        assert_eq!(store.borrow().image_count(), 0);
        assert_eq!(store.borrow().total_bytes(), 0);
    }

    #[test]
    fn closing_a_plugin_reclaims_quota_for_the_next_plugin() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 10,
                height: 20,
            }),
        );
        let store = Rc::new(RefCell::new(KittyImageStore::with_quota(1616)));
        pane.kitty_image_store = store.clone();
        pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
        assert_eq!(kitty_chunks(&mut pane, ClientId::new(1)).len(), 1);
        assert_eq!(store.borrow().total_bytes(), 1616);
        pane.handle_plugin_bytes(
            ClientId::new(1),
            png_transmit_sequence(43, &tiny_png()).into_bytes(),
        );
        assert_eq!(
            store.borrow().image_count(),
            1,
            "placed pixels prevent eviction"
        );
        drop(pane);
        assert_eq!(store.borrow().total_bytes(), 0);
        let mut next = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 10,
                height: 20,
            }),
        );
        next.kitty_image_store = store;
        next.handle_plugin_bytes(ClientId::new(1), image_frame(44, 2, &tiny_png()));
        assert_eq!(kitty_chunks(&mut next, ClientId::new(1)).len(), 1);
    }

    #[test]
    fn plugin_reload_releases_saved_alternate_screen_and_resets_parsers() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 10,
                height: 20,
            }),
        );
        let client = ClientId::new(1);
        for id in 1..4 {
            pane.handle_plugin_bytes(client, image_frame(id, 2, &tiny_png()));
            pane.handle_plugin_bytes(client, b"\x1b[?1049h".to_vec());
            pane.handle_plugin_bytes(client, image_frame(id + 10, 2, &tiny_png()));
            pane.handle_plugin_bytes(client, b"\x1b_Ga=t,m=1;AAAA".to_vec());
            pane.start_loading_indication(LoadingIndication::new("reload".into()));
            assert_eq!(pane.kitty_image_store.borrow().image_count(), 0);
            assert_eq!(pane.kitty_image_store.borrow().total_bytes(), 0);
        }
        pane.start_loading_indication(LoadingIndication::new("reload".into()));
        pane.handle_plugin_bytes(client, image_frame(42, 2, &tiny_png()));
        assert_eq!(kitty_chunks(&mut pane, client).len(), 1);
    }

    #[test]
    fn one_shot_image_survives_delayed_geometry() {
        for (width, height) in [(10, 20), (23, 43)] {
            for delayed in [false, true] {
                let cell = SizeInPixels { width, height };
                let mut pane = test_plugin_pane_with_cell_size(
                    10,
                    10,
                    Offset::default(),
                    if delayed { None } else { Some(cell) },
                );
                pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &chunky_png()));
                if delayed {
                    assert!(kitty_chunks(&mut pane, ClientId::new(1)).is_empty());
                    *pane.character_cell_size.borrow_mut() = Some(cell);
                    pane.character_cell_size_possibly_changed();
                }
                assert_eq!(
                    kitty_chunks(&mut pane, ClientId::new(1)).len(),
                    1,
                    "cell={cell:?} delayed={delayed}"
                );
            }
        }
    }

    #[test]
    fn a_plugins_image_survives_the_viewport_reset_between_frames() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 8,
                height: 16,
            }),
        );
        // The pixels are transmitted once; every later frame is only the placeholder cells, which
        // is the whole reason a plugin can afford to redraw an image at ten frames a second.
        pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
        let first = kitty_chunks(&mut pane, ClientId::new(1));
        pane.handle_plugin_bytes(ClientId::new(1), image_row(42, 0, 2).into_bytes());
        let second = kitty_chunks(&mut pane, ClientId::new(1));
        assert_eq!(second.len(), first.len());
        // The placement uid is stable across the rebuild, so the host diff does not retransmit.
        assert_eq!(
            second.first().map(|chunk| chunk.placement_uid),
            first.first().map(|chunk| chunk.placement_uid)
        );
    }

    #[test]
    fn a_plugins_image_is_dropped_when_the_host_terminal_cannot_show_one() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 8,
                height: 16,
            }),
        );
        pane.update_kitty_host_support(KittyHostSupport::ProtocolDisabled);
        pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
        assert!(kitty_chunks(&mut pane, ClientId::new(1)).is_empty());
    }

    /// A PNG wide enough that its base64 does not fit in one command, so the transmission has to
    /// be chunked - which is the shape `yinz-tile`'s `transmit_png` actually emits and the shape
    /// agent's wand frames arrive in.
    #[test]
    fn a_plugin_can_transmit_a_png_across_several_chunks() {
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 8,
                height: 16,
            }),
        );
        pane.handle_plugin_bytes(ClientId::new(1), image_frame(0xb000, 2, &chunky_png()));
        assert_eq!(kitty_chunks(&mut pane, ClientId::new(1)).len(), 1);
    }

    #[test]
    fn a_png_whose_later_chunks_never_arrived_is_not_placed() {
        // The complement of the test above, and what makes it mean something: a PNG cut off after
        // its first chunk does not decode, so a placement appearing at all is proof that the
        // chunks were reassembled rather than that the first one happened to be enough.
        let mut pane = test_plugin_pane_with_cell_size(
            10,
            10,
            Offset::default(),
            Some(SizeInPixels {
                width: 8,
                height: 16,
            }),
        );
        let png = chunky_png();
        let whole = png_transmit_sequence(0xb000, &png);
        // Everything up to the end of the first command, which by itself is not a PNG.
        let first_chunk_end = whole.find("\u{1b}\\").expect("a terminated command") + 2;
        assert!(first_chunk_end < whole.len(), "the fixture did not chunk");
        let mut frame = whole[..first_chunk_end].to_owned();
        frame.push_str(&place_image_sequence(0xb000, 2, 1));
        frame.push_str(&image_row(0xb000, 0, 2));
        pane.handle_plugin_bytes(ClientId::new(1), frame.into_bytes());
        assert!(kitty_chunks(&mut pane, ClientId::new(1)).is_empty());
    }

    #[test]
    fn a_plugins_kitty_replies_are_dropped_rather_than_queued_for_a_pty_it_does_not_have() {
        let mut pane = test_plugin_pane(10, 10, Offset::default());
        // `q` omitted, so the grid would ordinarily answer this - onto a queue nobody drains.
        pane.handle_plugin_bytes(
            ClientId::new(1),
            b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec(),
        );
        let grid = pane.grids.get(&ClientId::new(1)).expect("grid");
        assert!(grid.pending_messages_to_pty.is_empty());
        assert!(grid.pending_forwarded_queries.is_empty());
    }
}
