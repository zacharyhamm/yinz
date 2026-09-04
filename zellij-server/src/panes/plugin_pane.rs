use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crate::output::{CharacterChunk, KittyImageChunk, SixelImageChunk};
use crate::panes::{
    grid::Grid,
    kitty_graphics::{InterceptorResult, KittyApcInterceptor, KittyHostSupport, KittyImageStore},
    sixel::SixelImageStore,
    terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END},
    FrameColorOverride, LinkHandler, PaneId,
};
use crate::plugins::PluginInstruction;
use crate::pty::VteBytes;
use crate::tab::{AdjustedInput, Pane};
use crate::ui::{
    loading_indication::LoadingIndication,
    pane_boundaries_frame::{FrameParams, PaneFrame},
};
use crate::ClientId;
use std::cell::RefCell;
use std::rc::Rc;
use vte;
use zellij_utils::data::PaneContents;
use zellij_utils::data::{
    BareKey, Direction, KeyWithModifier, PermissionStatus, PermissionType, PluginPermission,
};
use zellij_utils::pane_size::{Offset, SizeInPixels};
use zellij_utils::position::Position;
use zellij_utils::{
    channels::SenderWithContext,
    data::{Event, InputMode, Mouse, Palette, PaletteColor, Style, Styling},
    errors::prelude::*,
    input::layout::Run,
    input::mouse::{MouseEvent, MouseEventType},
    pane_size::PaneGeom,
    shared::make_terminal_title,
};

macro_rules! style {
    ($fg:expr) => {
        ansi_term::Style::new().fg(match $fg {
            PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
            PaletteColor::EightBit(color) => ansi_term::Color::Fixed(color),
        })
    };
}

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
                $self.style.clone(),
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

pub(crate) struct PluginPane {
    pub pid: u32,
    pub should_render: HashMap<ClientId, bool>,
    pub selectable: bool,
    pub geom: PaneGeom,
    pub geom_override: Option<PaneGeom>,
    pub content_offset: Offset,
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
    requesting_permissions: Option<PluginPermission>,
    debug: bool,
    arrow_fonts: bool,
    styled_underlines: bool,
    should_be_suppressed: bool,
    text_being_pasted: Option<Vec<u8>>,
    supports_mouse_selection: bool,
}

impl PluginPane {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: u32,
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
        let initial_loading_message = loading_indication.to_string();
        let mut plugin = PluginPane {
            pid,
            should_render: HashMap::new(),
            selectable: true,
            geom: position_and_size,
            geom_override: None,
            send_plugin_instructions,
            active_at: Instant::now(),
            frame: HashMap::new(),
            content_offset: Offset::default(),
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
            requesting_permissions: None,
            debug,
            arrow_fonts,
            styled_underlines,
            should_be_suppressed: false,
            text_being_pasted: None,
            supports_mouse_selection: false,
        };
        for client_id in currently_connected_clients {
            plugin.handle_plugin_bytes(client_id, initial_loading_message.as_bytes().to_vec());
        }
        plugin
    }
}

impl Pane for PluginPane {
    // FIXME: These position and size things should all be moved to default trait implementations,
    // with something like a get_pos_and_sz() method underpinning all of them. Alternatively and
    // preferably, just use an enum and not a trait object
    fn x(&self) -> usize {
        self.geom_override.unwrap_or(self.geom).x
    }
    fn y(&self) -> usize {
        self.geom_override.unwrap_or(self.geom).y
    }
    fn rows(&self) -> usize {
        self.geom_override.unwrap_or(self.geom).rows.as_usize()
    }
    fn cols(&self) -> usize {
        self.geom_override.unwrap_or(self.geom).cols.as_usize()
    }
    fn get_content_x(&self) -> usize {
        self.x() + self.content_offset.left
    }
    fn get_content_y(&self) -> usize {
        self.y() + self.content_offset.top
    }
    fn get_content_columns(&self) -> usize {
        // content columns might differ from the pane's columns if the pane has a frame
        // in that case they would be 2 less
        self.cols()
            .saturating_sub(self.content_offset.left + self.content_offset.right)
    }
    fn get_content_rows(&self) -> usize {
        // content rows might differ from the pane's rows if the pane has a frame
        // in that case they would be 2 less
        self.rows()
            .saturating_sub(self.content_offset.top + self.content_offset.bottom)
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
    fn character_cell_size_possibly_changed(&mut self) {
        for grid in self.grids.values_mut() {
            grid.character_cell_size_possibly_changed();
        }
        self.render_full_viewport();
        self.set_should_render(true);
    }
    fn handle_plugin_bytes(&mut self, client_id: ClientId, bytes: VteBytes) {
        self.set_client_should_render(client_id, true);

        let mut vte_bytes = bytes;
        if let Some(plugin_permission) = &self.requesting_permissions {
            vte_bytes = self
                .display_request_permission_message(plugin_permission)
                .into();
        }

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
        let mut forwarded: Vec<u8> = Vec::with_capacity(vte_bytes.len());
        let mut index = 0;
        while index < vte_bytes.len() {
            // Between sequences every non-escape byte is forwarded unchanged, so the whole run up
            // to the next escape can be copied at once rather than driven through the state
            // machine a byte at a time. A plugin frame is almost entirely such runs.
            if interceptor.is_in_ground_state() {
                let rest = &vte_bytes[index..];
                let run_length = rest
                    .iter()
                    .position(|byte| *byte == 0x1b)
                    .unwrap_or(rest.len());
                if run_length > 0 {
                    forwarded.extend_from_slice(&rest[..run_length]);
                    index += run_length;
                    continue;
                }
            }
            let byte = vte_bytes[index];
            index += 1;
            match interceptor.advance(byte) {
                InterceptorResult::Forward(fwd) => forwarded.extend_from_slice(fwd.as_slice()),
                InterceptorResult::Swallow => {},
                InterceptorResult::Captured(command) => {
                    // Everything before the command has to reach the grid first: a placement is
                    // positioned by where the cursor is when it arrives.
                    if !forwarded.is_empty() {
                        vte_parser.advance(grid, &forwarded);
                        forwarded.clear();
                    }
                    grid.handle_kitty_apc(&command);
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
            .map(|g| g.has_selection())
            .unwrap_or(false)
        {
            self.reset_selection(client_id);
            None
        } else if let Some(requesting_permissions) = &self.requesting_permissions {
            let permissions = requesting_permissions.permissions.clone();
            if let Some(key_with_modifier) = key_with_modifier {
                match key_with_modifier.bare_key {
                    BareKey::Char('y') if key_with_modifier.has_no_modifiers() => {
                        Some(AdjustedInput::PermissionRequestResult(
                            permissions,
                            PermissionStatus::Granted,
                        ))
                    },
                    BareKey::Char('n') if key_with_modifier.has_no_modifiers() => {
                        Some(AdjustedInput::PermissionRequestResult(
                            permissions,
                            PermissionStatus::Denied,
                        ))
                    },
                    _ => None,
                }
            } else {
                match raw_input_bytes.as_slice() {
                    // Y or y
                    &[89] | &[121] => Some(AdjustedInput::PermissionRequestResult(
                        permissions,
                        PermissionStatus::Granted,
                    )),
                    // N or n
                    &[78] | &[110] => Some(AdjustedInput::PermissionRequestResult(
                        permissions,
                        PermissionStatus::Denied,
                    )),
                    _ => None,
                }
            }
        } else if let Some(key_with_modifier) = key_with_modifier {
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
    fn current_geom(&self) -> PaneGeom {
        self.geom_override.unwrap_or(self.geom)
    }
    fn geom_override(&self) -> Option<PaneGeom> {
        self.geom_override
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
    fn request_permissions_from_user(&mut self, permissions: Option<PluginPermission>) {
        self.requesting_permissions = permissions;
        self.handle_plugin_bytes_for_all_clients(Default::default()); // to trigger the render of
                                                                      // the permission message
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
        if client_id.is_none() {
            return Ok(None);
        }
        if let Some(client_id) = client_id {
            if self.should_render.get(&client_id).copied().unwrap_or(false) {
                let content_x = self.get_content_x();
                let content_y = self.get_content_y();
                let rows = self.get_content_rows();
                let columns = self.get_content_columns();
                if rows < 1 || columns < 1 {
                    return Ok(None);
                }
                if let Some(grid) = self.grids.get_mut(&client_id) {
                    match grid.render(content_x, content_y, &self.style) {
                        Ok(rendered_assets) => {
                            self.should_render.insert(client_id, false);
                            return Ok(rendered_assets);
                        },
                        e => return e,
                    }
                }
            }
        }
        Ok(None)
    }
    fn render_frame(
        &mut self,
        client_id: ClientId,
        frame_params: FrameParams,
        input_mode: InputMode,
    ) -> Result<Option<(Vec<CharacterChunk>, Option<String>)>> {
        if self.borderless {
            return Ok(None);
        }
        let frame_geom = frame_params
            .frame_geom_override
            .unwrap_or_else(|| self.current_geom());
        let grid = get_or_create_grid!(self, client_id);
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
            grid.title
                .clone()
                .unwrap_or_else(|| self.pane_title.clone())
        } else {
            self.pane_name.clone()
        };

        let is_pinned = frame_geom.is_pinned;
        let mut frame = PaneFrame::new(
            frame_geom.into(),
            grid.scrollback_position_and_length(),
            pane_title,
            frame_params,
        )
        .is_pinned(is_pinned);
        if let Some(override_) = frame_color_override {
            frame.override_color(override_.color);
        }

        let res = match self.frame.get(&client_id) {
            // TODO: use and_then or something?
            Some(last_frame) => {
                if &frame != last_frame || is_pinned {
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
    fn scroll_up(&mut self, count: usize, client_id: ClientId) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::ScrollUp(count)),
            )]))
            .unwrap();
    }
    fn scroll_down(&mut self, count: usize, client_id: ClientId) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
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
        if self.supports_mouse_selection {
            if let Some(grid) = self.grids.get_mut(&client_id) {
                grid.start_selection(start);
                self.set_should_render(true);
            }
        }
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::LeftClick(start.line.0, start.column.0)),
            )]))
            .unwrap();
    }
    fn update_selection(&mut self, position: &Position, client_id: ClientId) {
        if self.supports_mouse_selection {
            if let Some(grid) = self.grids.get_mut(&client_id) {
                grid.update_selection(position);
                self.set_should_render(true); // TODO: no??
            }
        }
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::Hold(position.line.0, position.column.0)),
            )]))
            .unwrap();
    }
    fn end_selection(&mut self, end: &Position, client_id: ClientId) {
        if self.supports_mouse_selection {
            if let Some(grid) = self.grids.get_mut(&client_id) {
                grid.end_selection(end);
            }
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
        if self.supports_mouse_selection {
            if let Some(grid) = self.grids.get_mut(&client_id) {
                grid.start_text_selection(position);
                self.set_should_render(true);
            }
        }
    }
    fn move_text_selection(&mut self, direction: Direction, client_id: ClientId) {
        if self.supports_mouse_selection {
            if let Some(grid) = self.grids.get_mut(&client_id) {
                grid.move_text_selection(direction);
                self.set_should_render(true);
            }
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
    fn set_content_offset(&mut self, offset: Offset) {
        self.content_offset = offset;
        self.resize_grids();
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

    fn set_borderless(&mut self, should_be_borderless: bool) {
        self.borderless = should_be_borderless;
        if should_be_borderless {
            self.set_content_offset(Offset::default());
        } else {
            self.set_content_offset(Offset::frame(1));
        }
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
    fn handle_right_click(&mut self, to: &Position, client_id: ClientId) {
        self.send_plugin_instructions
            .send(PluginInstruction::Update(vec![(
                Some(self.pid),
                Some(client_id),
                Event::Mouse(Mouse::RightClick(to.line.0, to.column.0)),
            )]))
            .unwrap();
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
            self.style.colors.frame_highlight.base,
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
    fn set_title(&mut self, title: String) {
        self.pane_title = title;
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
    fn update_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
        self.frame.clear();
    }
    fn set_should_be_suppressed(&mut self, should_be_suppressed: bool) {
        self.should_be_suppressed = should_be_suppressed;
    }
    fn query_should_be_suppressed(&self) -> bool {
        self.should_be_suppressed
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
    fn mouse_event(&self, event: &MouseEvent, client_id: ClientId) -> Option<String> {
        match event.event_type {
            MouseEventType::Motion
                if !event.left
                    && !event.right
                    && !event.middle
                    && !event.wheel_up
                    && !event.wheel_down =>
            {
                let _ = self
                    .send_plugin_instructions
                    .send(PluginInstruction::Update(vec![(
                        Some(self.pid),
                        Some(client_id),
                        Event::Mouse(Mouse::Hover(event.position.line(), event.position.column())),
                    )]));
            },
            _ => {},
        }
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
    fn display_request_permission_message(&self, plugin_permission: &PluginPermission) -> String {
        let bold_white = style!(self.style.colors.text_unselected.base).bold();
        let cyan = style!(self.style.colors.text_unselected.emphasis_1).bold();
        let orange = style!(self.style.colors.text_unselected.emphasis_0).bold();
        let green = style!(self.style.colors.text_unselected.emphasis_2).bold();

        let mut messages = String::new();
        let permissions: BTreeSet<PermissionType> =
            plugin_permission.permissions.clone().into_iter().collect();

        let min_row_count = permissions.len() + 4;

        if self.rows() >= min_row_count {
            messages.push_str(&format!(
                "{} {} {}\n",
                bold_white.paint("Plugin"),
                cyan.paint(&plugin_permission.name),
                bold_white.paint("asks permission to:"),
            ));
            permissions.iter().enumerate().for_each(|(i, p)| {
                messages.push_str(&format!(
                    "\n\r{}. {}",
                    bold_white.paint(format!("{}", i + 1)),
                    orange.paint(p.display_name())
                ));
            });

            messages.push_str(&format!(
                "\n\n\r{} {}",
                bold_white.paint("Allow?"),
                green.paint("(y/n)"),
            ));
        } else {
            messages.push_str(&format!(
                "{} {}. {} {}",
                bold_white.paint("This plugin asks permission to:"),
                orange.paint(
                    permissions
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                bold_white.paint("Allow?"),
                green.paint("(y/n)"),
            ));
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_tile::ui_components::{image_row, place_image_sequence, png_transmit_sequence};
    use zellij_utils::channels;
    use zellij_utils::pane_size::Dimension;

    fn test_plugin_pane(rows: usize, cols: usize, content_offset: Offset) -> PluginPane {
        test_plugin_pane_with_cell_size(rows, cols, content_offset, None)
    }

    /// Placeholder placements are only resolved once the character cell size is known
    /// (`Grid::kitty_refresh_placeholder_placements`), so an image test has to supply one.
    fn test_plugin_pane_with_cell_size(
        rows: usize,
        cols: usize,
        content_offset: Offset,
        cell_size: Option<SizeInPixels>,
    ) -> PluginPane {
        let (plugin_tx, _plugin_rx) = channels::unbounded();
        let geom = PaneGeom {
            rows: Dimension::fixed(rows),
            cols: Dimension::fixed(cols),
            ..Default::default()
        };
        let mut pane = PluginPane::new(
            1,
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
        pane
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
        pane.show_cursor(1, Some((0, last_content_row)));
        assert_eq!(
            pane.cursor_coordinates(Some(1)),
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
        pane.show_cursor(1, Some((last_content_column, 0)));
        assert_eq!(
            pane.cursor_coordinates(Some(1)),
            Some((last_content_column + 1, 0, true))
        );
    }

    fn ztext(x: usize, y: usize, width: usize, text: &str) -> String {
        let bytes = text
            .as_bytes()
            .iter()
            .map(|byte| byte.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!("\u{1b}Pztext;{x}/{y}/{width}/;{bytes}\u{1b}\\")
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
            ztext(0, 0, columns, "top"),
            ztext(5, rows - 1, columns, "bottom"),
        );
        pane.handle_plugin_bytes(1, frame.into_bytes());

        // `Grid`'s `Debug` prints one viewport row per line, each behind a `00 (C): `
        // index prefix.
        let rendered = format!("{:?}", pane.grids.get(&1).expect("the grid was created"));
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
        pane.show_cursor(1, Some((0, content_rows)));
        assert_eq!(pane.cursor_coordinates(Some(1)), None);
    }

    /// The frame a plugin drawing an image emits, built with the very helpers a plugin calls -
    /// `zellij-tile` is a dev-dependency for exactly this. A second, hand-written copy of the
    /// escape sequences here would pass while the two halves disagreed.
    fn image_frame(id: u32, cols: usize, png: &[u8]) -> Vec<u8> {
        let mut frame = png_transmit_sequence(id, png);
        frame.push_str(&place_image_sequence(id, cols, 1));
        frame.push_str(&image_row(id, 0, cols));
        frame.into_bytes()
    }

    /// A PNG wide enough that its base64 does not fit in one command, so `transmit_png` has to
    /// chunk it - which is the shape hocus's wand frames arrive in.
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
    fn tiny_png() -> Vec<u8> {
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

    fn kitty_chunks(pane: &mut PluginPane, client_id: ClientId) -> Vec<KittyImageChunk> {
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
        pane.handle_plugin_bytes(1, image_frame(42, 2, &tiny_png()));
        let chunks = kitty_chunks(&mut pane, 1);
        // A contiguous placeholder run resolves to one chunk covering the cells it spans, not one
        // chunk per cell.
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert_eq!(chunks[0].dest_cells, (2, 1));
        assert_eq!((chunks[0].cell_x, chunks[0].cell_y), (0, 0));
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
        pane.handle_plugin_bytes(1, image_frame(42, 2, &tiny_png()));
        let first = kitty_chunks(&mut pane, 1);
        pane.handle_plugin_bytes(1, image_row(42, 0, 2).into_bytes());
        let second = kitty_chunks(&mut pane, 1);
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
        pane.handle_plugin_bytes(1, image_frame(42, 2, &tiny_png()));
        assert!(kitty_chunks(&mut pane, 1).is_empty());
    }

    /// A PNG wide enough that its base64 does not fit in one command, so the transmission has to
    /// be chunked - which is the shape `zellij-tile`'s `transmit_png` actually emits and the shape
    /// hocus's wand frames arrive in.
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
        pane.handle_plugin_bytes(1, image_frame(0xb000, 2, &chunky_png()));
        assert_eq!(kitty_chunks(&mut pane, 1).len(), 1);
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
        pane.handle_plugin_bytes(1, frame.into_bytes());
        assert!(kitty_chunks(&mut pane, 1).is_empty());
    }

    #[test]
    fn a_plugins_kitty_replies_are_dropped_rather_than_queued_for_a_pty_it_does_not_have() {
        let mut pane = test_plugin_pane(10, 10, Offset::default());
        // `q` omitted, so the grid would ordinarily answer this - onto a queue nobody drains.
        pane.handle_plugin_bytes(1, b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
        let grid = pane.grids.get(&1).expect("grid");
        assert!(grid.pending_messages_to_pty.is_empty());
        assert!(grid.pending_forwarded_queries.is_empty());
    }
}
