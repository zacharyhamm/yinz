use crate::output::{CharacterChunk, Output};
use crate::pane::{Pane, RenderedPane};
use crate::panes::terminal_character::AnsiCode;
use crate::panes::PaneId;
use crate::ui::boundaries::Boundaries;
use crate::ui::pane_boundaries_frame::{FrameParams, OpeningRipple, StackListEntry};
use crate::ClientId;
use std::collections::{HashMap, HashSet};
use yinz_runtime::errors::prelude::*;
use yinz_types::animation::blend;
use yinz_types::pane_size::PaneGeom;
use yinz_types::style::Style;
use {
    yinz_types::data::client_id_to_colors, yinz_types::data::InputMode,
    yinz_types::data::PaletteColor,
};

pub fn dim_character_chunks(character_chunks: &mut [CharacterChunk]) {
    for chunk in character_chunks.iter_mut() {
        for terminal_character in chunk.terminal_characters.iter_mut() {
            terminal_character.styles.update(|styles| {
                styles.dim = Some(AnsiCode::On);
            });
        }
    }
}

/// Whether a pane render emitted anything, so a watcher replay has to repeat it.
fn rendered_anything((character_chunks, raw_vte_output, sixel, kitty): &RenderedPane) -> bool {
    !character_chunks.is_empty()
        || !sixel.is_empty()
        || !kitty.is_empty()
        || raw_vte_output.as_ref().is_some_and(|s| !s.is_empty())
}

pub struct PaneContentsAndUi<'a> {
    pane: &'a mut Box<dyn Pane>,
    output: &'a mut Output,
    style: Style,
    focused_clients: Vec<ClientId>,
    multiple_users_exist_in_session: bool,
    z_index: Option<usize>,
    pane_is_stacked_under: bool,
    pane_is_stacked_over: bool,
    should_draw_pane_frames: bool,
    mouse_is_hovering_over_pane_for_clients: HashSet<ClientId>,
    current_pane_group: HashMap<ClientId, Vec<PaneId>>,
    show_help_text: bool,
    omit_title: bool,
    frame_geom_override: Option<PaneGeom>,
    stack_list_entry_width: Option<usize>,
    stack_list_entry_is_selected: bool,
    stack_list_entry_stack_is_focused: bool,
    blank_title: bool,
    same_worktree: bool,
    mouse_scroll_resize: bool,
    dimmed_for_clients: HashSet<ClientId>,
    opening_ripple: Option<OpeningRipple>,
}

impl<'a> PaneContentsAndUi<'a> {
    pub fn new(
        pane: &'a mut Box<dyn Pane>,
        output: &'a mut Output,
        style: Style,
        active_panes: &HashMap<ClientId, PaneId>,
        multiple_users_exist_in_session: bool,
        z_index: Option<usize>,
        pane_is_stacked_under: bool,
        pane_is_stacked_over: bool,
        should_draw_pane_frames: bool,
        mouse_hover_pane_id: &HashMap<ClientId, PaneId>,
        current_pane_group: HashMap<ClientId, Vec<PaneId>>,
        show_help_text: bool,
        omit_title: bool,
        mouse_scroll_resize: bool,
        dimmed_for_clients: HashSet<ClientId>,
    ) -> Self {
        let mut focused_clients: Vec<ClientId> = active_panes
            .iter()
            .filter(|(_c_id, p_id)| **p_id == pane.pid())
            .map(|(c_id, _p_id)| *c_id)
            .collect();
        focused_clients.sort_unstable();
        let mouse_is_hovering_over_pane_for_clients = mouse_hover_pane_id
            .iter()
            .filter_map(|(client_id, pane_id)| {
                if pane_id == &pane.pid() {
                    Some(*client_id)
                } else {
                    None
                }
            })
            .collect();
        PaneContentsAndUi {
            pane,
            output,
            style,
            focused_clients,
            multiple_users_exist_in_session,
            z_index,
            pane_is_stacked_under,
            pane_is_stacked_over,
            should_draw_pane_frames,
            mouse_is_hovering_over_pane_for_clients,
            current_pane_group,
            show_help_text,
            omit_title,
            frame_geom_override: None,
            stack_list_entry_width: None,
            stack_list_entry_is_selected: false,
            stack_list_entry_stack_is_focused: false,
            blank_title: false,
            same_worktree: false,
            mouse_scroll_resize,
            dimmed_for_clients,
            opening_ripple: None,
        }
    }
    fn frame_is_dimmed_for_client(&self, client_id: ClientId) -> bool {
        self.dimmed_for_clients.contains(&client_id) && !self.focused_clients.contains(&client_id)
    }
    pub fn set_same_worktree(&mut self, same_worktree: bool) {
        self.same_worktree = same_worktree;
    }
    pub fn set_opening_ripple(&mut self, ripple: Option<OpeningRipple>) {
        self.opening_ripple = ripple;
    }
    pub fn set_frame_geom_override(&mut self, frame_geom_override: Option<PaneGeom>) {
        self.frame_geom_override = frame_geom_override;
    }
    pub fn set_blank_title(&mut self, blank_title: bool) {
        self.blank_title = blank_title;
    }
    pub fn set_stack_list_entry(
        &mut self,
        stack_list_entry_width: Option<usize>,
        stack_list_entry_is_selected: bool,
        stack_list_entry_stack_is_focused: bool,
    ) {
        self.stack_list_entry_width = stack_list_entry_width;
        self.stack_list_entry_is_selected = stack_list_entry_is_selected;
        self.stack_list_entry_stack_is_focused = stack_list_entry_stack_is_focused;
    }
    pub fn render_pane_contents_to_multiple_clients(
        &mut self,
        clients: impl Iterator<Item = ClientId>,
    ) -> Result<()> {
        let err_context = "failed to render pane contents to multiple clients";

        // here we drop the fake cursors so that their lines will be updated
        // and we can clear them from the UI below
        drop(self.pane.drain_fake_cursors());

        let clients: Vec<ClientId> = clients.collect();
        if let Some(rendered) = self.pane.render(None).context(err_context)? {
            if self
                .output
                .pane_change_client
                .is_some_and(|client| clients.contains(&client))
                && rendered_anything(&rendered)
            {
                self.output.rendered_pane_contents.insert(self.pane.pid());
            }
            let (character_chunks, raw_vte_output, sixel_image_chunks, kitty_image_chunks) =
                rendered;
            self.output
                .add_character_chunks_to_multiple_clients(
                    character_chunks,
                    clients.iter().copied(),
                    self.z_index,
                )
                .context(err_context)?;
            self.output.add_sixel_image_chunks_to_multiple_clients(
                sixel_image_chunks,
                clients.iter().copied(),
                self.z_index,
            );
            self.output.add_kitty_image_chunks_to_multiple_clients(
                self.pane.pid(),
                kitty_image_chunks,
                clients.iter().copied(),
                self.z_index,
            );
            if let Some(raw_vte_output) = raw_vte_output {
                if !raw_vte_output.is_empty() {
                    self.output.add_post_vte_instruction_to_multiple_clients(
                        clients.iter().copied(),
                        &format!(
                            "\u{1b}[{};{}H\u{1b}[m{}",
                            self.pane.y() + 1,
                            self.pane.render_geom().x + 1,
                            raw_vte_output
                        ),
                    );
                }
            }
        }
        if let Some(tabs) = self
            .pane
            .pane_tabs()
            .filter(|_| matches!(self.pane.pid(), PaneId::Terminal(_)))
        {
            if self.pane.get_content_y() > self.pane.y() && self.pane.get_content_rows() > 0 {
                for client in clients {
                    let characters = crate::pane_tabs::render_bar(
                        tabs,
                        self.pane.pane_tabs_view(client),
                        tabs.bar_focused.contains(&client),
                        self.pane.get_content_columns(),
                        &self.style,
                    );
                    self.output
                        .add_character_chunks_to_client(
                            client,
                            vec![CharacterChunk::new(
                                characters,
                                self.pane.get_content_x(),
                                self.pane.get_content_y() - 1,
                            )],
                            self.z_index,
                        )
                        .context("render terminal pane tabs")?;
                }
            }
        }
        Ok(())
    }
    pub fn render_pane_contents_for_client(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to render pane contents for client {client_id}");

        if let Some(rendered) = self
            .pane
            .render(Some(client_id))
            .with_context(err_context)?
        {
            if self.output.pane_change_client == Some(client_id) && rendered_anything(&rendered) {
                self.output.rendered_pane_contents.insert(self.pane.pid());
            }
            let (character_chunks, raw_vte_output, sixel_image_chunks, kitty_image_chunks) =
                rendered;
            self.output
                .add_character_chunks_to_client(client_id, character_chunks, self.z_index)
                .with_context(err_context)?;
            self.output.add_sixel_image_chunks_to_client(
                client_id,
                sixel_image_chunks,
                self.z_index,
            );
            self.output.add_kitty_image_chunks_to_client(
                client_id,
                self.pane.pid(),
                kitty_image_chunks,
                self.z_index,
            );
            if let Some(raw_vte_output) = raw_vte_output {
                if !raw_vte_output.is_empty() {
                    self.output.add_post_vte_instruction_to_client(
                        client_id,
                        &format!(
                            "\u{1b}[{};{}H\u{1b}[m{}",
                            self.pane.y() + 1,
                            self.pane.render_geom().x + 1,
                            raw_vte_output
                        ),
                    );
                }
            }
        }
        Ok(())
    }
    pub fn client_has_guest_modal(&self, client_id: ClientId) -> bool {
        self.pane.guest_modal_selection(client_id).is_some()
    }
    pub fn drain_pane_render_state(&mut self) {
        drop(self.pane.drain_fake_cursors());
        let _ = self.pane.render(None);
    }
    pub fn render_guest_modal_for_client(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to render guest modal for client {client_id}");
        let selection = self.pane.guest_modal_selection(client_id).unwrap_or(0);
        let session_name = self
            .pane
            .guest_session_name()
            .unwrap_or_else(|| String::from("unknown").into());
        let columns = self.pane.get_content_columns();
        let rows = self.pane.get_content_rows();
        let content_x = self.pane.get_content_x();
        let content_y = self.pane.get_content_y();
        let shortcuts = self.pane.guest_modal_shortcuts();
        let chunks = crate::panes::nested_session_modal::guest_modal_chunks(
            columns,
            rows,
            content_x,
            content_y,
            &self.style,
            session_name.as_str(),
            selection,
            &shortcuts,
        );
        self.output
            .add_character_chunks_to_client(client_id, chunks, self.z_index)
            .with_context(err_context)?;
        Ok(())
    }
    pub fn render_fake_cursor_if_needed(&mut self, client_id: ClientId) -> Result<()> {
        let pane_focused_for_client_id = self.focused_clients.contains(&client_id);
        let pane_focused_for_different_client = self
            .focused_clients
            .iter()
            .filter(|&&c_id| c_id != client_id)
            .count()
            > 0;
        if pane_focused_for_different_client && !pane_focused_for_client_id {
            let fake_cursor_client_id = self
                .focused_clients
                .iter()
                .find(|&&c_id| c_id != client_id)
                .with_context(|| {
                    format!("failed to render fake cursor if needed for client {client_id}")
                })?;
            if let Some(colors) = client_id_to_colors(
                *fake_cursor_client_id,
                self.style.colors.multiplayer_user_colors,
            ) {
                let cursor_is_visible = self
                    .pane
                    .cursor_coordinates(Some(*fake_cursor_client_id))
                    .and_then(|(x, y, is_visible)| {
                        if is_visible {
                            Some(self.output.cursor_is_visible(
                                self.pane.render_geom().x + x,
                                self.pane.y() + y,
                                self.z_index,
                                // the client being drawn for, not the one whose cursor this is:
                                // the menu that could hide it is the viewer's own
                                client_id,
                            ))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false);
                if cursor_is_visible {
                    if let Some(vte_output) = self.pane.render_fake_cursor(colors.0, colors.1) {
                        self.output.add_post_vte_instruction_to_client(
                            client_id,
                            &format!(
                                "\u{1b}[{};{}H\u{1b}[m{}",
                                self.pane.y() + 1,
                                self.pane.render_geom().x + 1,
                                vte_output
                            ),
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub fn render_terminal_title_if_needed(
        &mut self,
        client_id: ClientId,
        client_mode: InputMode,
        previous_title: &mut Option<String>,
    ) {
        if !self.focused_clients.contains(&client_id) {
            return;
        }
        let vte_output = self.pane.render_terminal_title(client_mode);
        if let Some(previous_title) = previous_title {
            if *previous_title == vte_output {
                return;
            }
        }
        *previous_title = Some(vte_output.clone());
        self.output
            .add_post_vte_instruction_to_client(client_id, &vte_output);
    }
    pub fn render_pane_frame(
        &mut self,
        client_id: ClientId,
        client_mode: InputMode,
        session_is_mirrored: bool,
        pane_is_floating: bool,
        pane_is_selectable: bool,
        show_pinned_indication: bool,
        show_scroll_indication: bool,
    ) -> Result<()> {
        let err_context = || format!("failed to render pane frame for client {client_id}");

        let pane_focused_for_client_id = self.focused_clients.contains(&client_id);
        let other_focused_clients: Vec<ClientId> = self
            .focused_clients
            .iter()
            .filter(|&&c_id| c_id != client_id)
            .copied()
            .collect();
        let pane_focused_for_differet_client = !other_focused_clients.is_empty();

        let frame_color = self.frame_color(client_id, client_mode, session_is_mirrored);
        let highlight_tooltip = self.pane.cached_hover_tooltip();
        let focused_client = if pane_focused_for_client_id {
            Some(client_id)
        } else if pane_focused_for_differet_client {
            Some(*other_focused_clients.first().with_context(err_context)?)
        } else {
            None
        };
        let pane_is_stacked = self.pane.current_geom().is_stacked();
        let pane_is_in_group = self
            .current_pane_group
            .get(&client_id)
            .map(|p| p.contains(&self.pane.pid()))
            .unwrap_or(false);
        let stack_list_entry = self.stack_list_entry_width.map(|width| StackListEntry {
            width,
            label: self.pane.stack_list_entry_label(self.same_worktree),
            is_selected: self.stack_list_entry_is_selected,
            stack_is_focused: self.stack_list_entry_stack_is_focused,
            border_color: self
                .pane
                .waiting_pulse()
                .and_then(|_| frame_color.map(|(color, _)| color)),
            is_emphasized: pane_is_in_group
                || (self
                    .mouse_is_hovering_over_pane_for_clients
                    .contains(&client_id)
                    && !pane_focused_for_client_id),
        });
        let frame_is_dimmed = self.frame_is_dimmed_for_client(client_id);
        let guest_choice_indicator = self.pane.guest_choice_indicator(client_id);
        let frame_params = if session_is_mirrored {
            FrameParams {
                focused_client,
                is_main_client: pane_focused_for_client_id,
                other_focused_clients: vec![],
                style: self.style,
                color: frame_color.map(|c| c.0),
                other_cursors_exist_in_session: false,
                pane_is_stacked_over: self.pane_is_stacked_over,
                pane_is_stacked_under: self.pane_is_stacked_under,
                pane_is_stacked,
                should_draw_pane_frames: self.should_draw_pane_frames,
                pane_is_floating,
                show_pinned_indication,
                show_scroll_indication,
                content_offset: self.pane.get_content_offset(),
                mouse_is_hovering_over_pane: self
                    .mouse_is_hovering_over_pane_for_clients
                    .contains(&client_id),
                pane_is_selectable,
                show_help_text: self.show_help_text,
                highlight_tooltip: highlight_tooltip.clone(),
                omit_title: self.omit_title,
                frame_geom_override: self.frame_geom_override,
                stack_list_entry: stack_list_entry.clone(),
                blank_title: self.blank_title,
                same_worktree: self.same_worktree,
                mouse_scroll_resize: self.mouse_scroll_resize,
                dimmed: frame_is_dimmed,
                guest_choice_indicator,
                opening_ripple: self.opening_ripple,
            }
        } else {
            FrameParams {
                focused_client,
                is_main_client: pane_focused_for_client_id,
                other_focused_clients,
                style: self.style,
                color: frame_color.map(|c| c.0),
                other_cursors_exist_in_session: self.multiple_users_exist_in_session,
                pane_is_stacked_over: self.pane_is_stacked_over,
                pane_is_stacked_under: self.pane_is_stacked_under,
                pane_is_stacked,
                should_draw_pane_frames: self.should_draw_pane_frames,
                pane_is_floating,
                show_pinned_indication,
                show_scroll_indication,
                content_offset: self.pane.get_content_offset(),
                mouse_is_hovering_over_pane: self
                    .mouse_is_hovering_over_pane_for_clients
                    .contains(&client_id),
                pane_is_selectable,
                show_help_text: self.show_help_text,
                highlight_tooltip,
                omit_title: self.omit_title,
                frame_geom_override: self.frame_geom_override,
                stack_list_entry,
                blank_title: self.blank_title,
                same_worktree: self.same_worktree,
                mouse_scroll_resize: self.mouse_scroll_resize,
                dimmed: frame_is_dimmed,
                guest_choice_indicator,
                opening_ripple: self.opening_ripple,
            }
        };

        if let Some((mut frame_terminal_characters, vte_output)) = self
            .pane
            .render_frame(client_id, frame_params, client_mode)
            .with_context(err_context)?
        {
            if let Some(outer) = self.frame_geom_override {
                frame_terminal_characters.retain_mut(|chunk| {
                    if chunk.y < outer.y
                        || chunk.y >= outer.y + outer.rows.as_usize()
                        || chunk.x < outer.x
                        || chunk.x >= outer.x + outer.cols.as_usize()
                    {
                        return false;
                    }
                    chunk.retain_by_width(outer.x + outer.cols.as_usize() - chunk.x);
                    true
                });
            }
            if self.output.pane_change_client == Some(client_id)
                && (!frame_terminal_characters.is_empty()
                    || vte_output.as_ref().is_some_and(|s| !s.is_empty()))
            {
                self.output.rendered_pane_frames.insert(self.pane.pid());
            }
            if self.frame_is_dimmed_for_client(client_id) {
                dim_character_chunks(&mut frame_terminal_characters);
            }
            self.output
                .add_character_chunks_to_client(client_id, frame_terminal_characters, self.z_index)
                .with_context(err_context)?;
            if let Some(vte_output) = vte_output {
                self.output
                    .add_post_vte_instruction_to_client(client_id, &vte_output);
            }
        }

        Ok(())
    }
    pub fn render_pane_boundaries(
        &self,
        client_id: ClientId,
        client_mode: InputMode,
        boundaries: &mut Boundaries,
        session_is_mirrored: bool,
        pane_is_on_top_of_stack: bool,
        pane_is_on_bottom_of_stack: bool,
    ) {
        let color = self.frame_color(client_id, client_mode, session_is_mirrored);
        boundaries.add_rect(
            self.pane.as_ref(),
            color,
            self.style.chrome_colors(),
            pane_is_on_top_of_stack,
            pane_is_on_bottom_of_stack,
            self.pane_is_stacked_under,
        );
    }
    fn frame_color(
        &self,
        client_id: ClientId,
        mode: InputMode,
        session_is_mirrored: bool,
    ) -> Option<(PaletteColor, usize)> {
        frame_color(
            &self.style,
            FrameColorState {
                host_colors: self.pane.host_colors(),
                frame_color_override: self
                    .pane
                    .frame_color_override(client_id)
                    .map(|color| (color, self.pane.frame_color_override_strength(client_id))),
                waiting_pulse: self.pane.waiting_pulse(),
                pane_is_focused: self.focused_clients.contains(&client_id),
                pane_is_in_group: self
                    .current_pane_group
                    .get(&client_id)
                    .map(|p| p.contains(&self.pane.pid()))
                    .unwrap_or(false),
                pane_is_related: self.pane.has_related_highlight(),
                mouse_is_hovering_over_pane: self
                    .mouse_is_hovering_over_pane_for_clients
                    .contains(&client_id),
                multiple_users_exist_in_session: self.multiple_users_exist_in_session,
            },
            client_id,
            mode,
            session_is_mirrored,
        )
    }
}

/// Everything about one pane, for one client, that decides the colour of that pane's frame.
pub struct FrameColorState {
    /// Host foreground/background, before any pane-local overrides.
    pub host_colors: (PaletteColor, PaletteColor),
    /// The override colour and how strongly it applies right now - `1.0` fully, less while a bell
    /// flash is fading back out. A strength below `1.0` blends the override into the colour the
    /// pane would otherwise have, which is why the blend happens *here*: this function is the only
    /// place that knows what that colour is, and it differs per client with focus, group
    /// membership and hover.
    pub frame_color_override: Option<(PaletteColor, f32)>,
    /// The sampled pulse strength; colors are resolved from the current render context.
    pub waiting_pulse: Option<f32>,
    pub pane_is_focused: bool,
    pub pane_is_in_group: bool,
    /// This pane is kin of the focused one - see `Pane::has_related_highlight`. The gentlest
    /// state there is: it loses to focus, to a group, to a hover and to an override, and only
    /// ever replaces the colour a pane would have had for being nothing in particular.
    pub pane_is_related: bool,
    pub mouse_is_hovering_over_pane: bool,
    pub multiple_users_exist_in_session: bool,
}

/// Fade the focused frame's hue toward the chrome background to mark related panes gently.
const RELATED_FADE: f32 = 0.55;

/// The colour a pane's frame and boundary cells are drawn in, and the precedence that colour wins
/// with where two panes share one boundary cell (see `Boundaries::add_rect` and `combine_symbols`).
///
/// A free function so a caller outside `PaneContentsAndUi` can arrive at the same answer for a
/// pane it is drawing a boundary for.
///
/// The base precedences run `0` (nothing in particular) → `2` related → `4` hovered → `6` grouped →
/// `8` focused → `10` overridden. A positive waiting pulse adds `1`, beating an unpulsed neighbour
/// of the same base state but losing to every higher base state. Overrides suppress the pulse.
/// Only their order matters, never the values: `combine_symbols`
/// settles a shared cell with `>=`, so the pane whose `add_rect` ran first wins a tie - which is
/// stable but keyed on pane id, and so means nothing to whoever is looking at it. Two states that
/// can apply to two neighbouring panes at once must therefore not share a number.
pub fn frame_color(
    style: &Style,
    state: FrameColorState,
    client_id: ClientId,
    mode: InputMode,
    session_is_mirrored: bool,
) -> Option<(PaletteColor, usize)> {
    let FrameColorState {
        host_colors,
        frame_color_override,
        waiting_pulse,
        pane_is_focused,
        pane_is_in_group,
        pane_is_related,
        mouse_is_hovering_over_pane,
        multiple_users_exist_in_session,
    } = state;
    let (foreground, background) = style.chrome_colors().unwrap_or(host_colors);
    if let Some((override_color, strength)) = frame_color_override.filter(|_| !pane_is_in_group) {
        // a fading override blends towards whatever this pane's frame would be without it, so a
        // bell settles back into the normal colour instead of snapping off it
        let settled = frame_color(
            style,
            FrameColorState {
                host_colors,
                frame_color_override: None,
                waiting_pulse: None,
                pane_is_focused,
                pane_is_in_group,
                pane_is_related,
                mouse_is_hovering_over_pane,
                multiple_users_exist_in_session,
            },
            client_id,
            mode,
            session_is_mirrored,
        );
        if strength <= 0.0 || strength.is_nan() {
            return settled;
        }
        let base = settled.map(|(color, _)| color).unwrap_or(foreground);
        return Some((blend(base, override_color, strength), 10));
    }
    let base = if pane_is_in_group && !pane_is_focused {
        Some((style.colors.frame_highlight.emphasis_0, 6))
    } else if pane_is_in_group && pane_is_focused {
        Some((style.colors.frame_highlight.emphasis_1, 8))
    } else if pane_is_focused {
        match mode {
            InputMode::Normal | InputMode::Locked => {
                if session_is_mirrored || !multiple_users_exist_in_session {
                    Some((style.colors.frame_selected.base, 8))
                } else {
                    let colors =
                        client_id_to_colors(client_id, style.colors.multiplayer_user_colors);
                    colors.map(|colors| (colors.0, 8))
                }
            },
            _ => Some((style.colors.frame_highlight.base, 8)),
        }
    } else if mouse_is_hovering_over_pane {
        Some((style.colors.ribbon_unselected.emphasis_1, 4))
    } else if pane_is_related {
        // Kin of the focused pane, and drawn as such: the focused frame's own colour, faded
        // most of the way back to the background, so it reads as "these belong with that one"
        // rather than as a second focus competing with the first.
        Some((
            blend(style.colors.frame_selected.base, background, RELATED_FADE),
            2,
        ))
    } else {
        style.colors.frame_unselected.map(|frame| (frame.base, 0))
    };
    if let Some(strength) = waiting_pulse.filter(|strength| *strength > 0.0) {
        let (color, precedence) = base.unwrap_or((foreground, 0));
        let rgb = |color| match color {
            PaletteColor::Rgb(rgb) => rgb,
            PaletteColor::EightBit(index) => yinz_types::colors::eightbit_to_rgb(index),
        };
        let start = rgb(color);
        let mut target = style.colors.text_unselected.base;
        if rgb(target) == start {
            target = style.colors.frame_selected.base;
        }
        if rgb(target) == start {
            let (r, g, b) = start;
            let channel = if u16::from(r) + u16::from(g) + u16::from(b) <= 3 * 255 / 2 {
                255
            } else {
                0
            };
            target = PaletteColor::Rgb((channel, channel, channel));
        }
        Some((blend(color, target, strength), precedence + 1))
    } else {
        base
    }
}

#[cfg(test)]
#[path = "./unit/frame_color_tests.rs"]
mod frame_color_tests;
