//! Composing pane layers, frames, and host cursor state.
use crate::plugins::PluginInstruction;

use super::{pane_tabs, CursorMemoKey, HostCursorState, Tab};
use crate::output::CharacterChunk;
use crate::output::Output;
use crate::panes::PaneId;
use crate::panes::EMPTY_TERMINAL_CHARACTER;
use crate::ui::damage::overlay_damage;
use crate::ui::damage::CellRect;
use crate::ClientId;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;
use yinz_panes::resize_pty;
use yinz_runtime::errors::prelude::*;
use yinz_types::pane_size::PaneGeom;

impl Tab {
    /// Draw this tab: its tiled panes and chrome, then the floating layer and the overlays above
    /// it, then the cursor.
    ///
    /// The cursor goes last so that it is placed once everything contributing to the frame is in
    /// the output, rather than wherever the last character chunk happened to land. A caller that
    /// paints *above* the tabs - the session's toasts do - must therefore not use this: it has to
    /// call `render_panes` for every tab, paint its own layer, and only then place the cursor with
    /// `render_cursor_last`. See `Screen::render_to_clients`. Production always does, so this is
    /// only for tests that draw one tab on its own.
    #[cfg(test)]
    pub fn render(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        self.render_panes(output, client_id_override)?;
        self.render_cursor_last(output, client_id_override);
        Ok(())
    }

    pub(super) fn update_agent_pane_widths(&mut self) -> Result<()> {
        let candidates: HashSet<PaneId> = self.get_all_pane_ids().into_iter().collect();
        let mut eligible: HashSet<PaneId> = candidates
            .iter()
            .copied()
            .filter(|id| {
                self.pane_is_agent(*id)
                    || self
                        .get_pane_with_id(*id)
                        .is_some_and(|pane| pane.is_detected_agent())
            })
            .collect();
        // A pane tab's plugin view shares the agent terminal's presentation.
        for tabs in self.pane_tabs.values() {
            if candidates.contains(&tabs.info.pane_id)
                && tabs
                    .info
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.pane_id())
                    .any(|id| eligible.contains(&id))
            {
                eligible.insert(tabs.info.pane_id);
                eligible.extend(tabs.info.tabs.iter().filter_map(|tab| tab.pane_id()));
            }
        }
        for list in self.stack_lists.values() {
            if list.members.iter().any(|id| eligible.contains(id)) {
                eligible.extend(list.members.iter().copied());
            }
        }
        let ids = self.get_all_pane_ids();
        let before: HashMap<_, _> = ids
            .iter()
            .filter_map(|id| {
                self.get_pane_with_id(*id).map(|pane| {
                    (
                        *id,
                        (
                            pane.render_geom(),
                            pane.get_content_columns(),
                            pane.get_content_rows(),
                        ),
                    )
                })
            })
            .collect();
        for id in &ids {
            let width = if eligible.contains(id) && self.max_agent_width >= 80 {
                self.max_agent_width
            } else {
                0
            };
            if let Some(pane) = self.get_pane_with_id_mut(*id) {
                pane.set_max_content_width(width);
            }
        }
        let mut widths = self.tiled_panes.agent_widths()?;
        widths.extend(self.floating_panes.agent_widths());
        widths.extend(self.overlay_panes.agent_widths());
        for tabs in self.pane_tabs.values() {
            if let Some(width) = widths.get(&tabs.info.pane_id).copied() {
                widths.extend(
                    tabs.info
                        .tabs
                        .iter()
                        .filter_map(|tab| tab.pane_id())
                        .map(|id| (id, width)),
                );
            }
        }
        for list in self.stack_lists.values() {
            if let Some(width) = widths.get(&list.visible).copied() {
                widths.extend(list.members.iter().map(|id| (*id, width)));
            }
        }
        let mut changed = false;
        for id in ids {
            if let Some(pane) = self.get_pane_with_id_mut(id) {
                changed |= pane.render_width() != widths.get(&id).copied();
                pane.set_render_width(widths.get(&id).copied());
            }
            if let Some(pane) = self.get_pane_with_id(id)
                && let Some((geom, cols, rows)) = before.get(&id)
            {
                changed |= *geom != pane.render_geom();
                if (*cols, *rows) != (pane.get_content_columns(), pane.get_content_rows()) {
                    resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                        .context("failed to resize agent pane")?;
                }
            }
        }
        if changed {
            self.set_should_clear_display_before_rendering();
            self.set_force_render();
        }
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
        self.sync_tab_bar_height()?;
        let transfers: Vec<_> = self
            .get_all_pane_ids()
            .into_iter()
            .filter_map(|id| {
                self.get_pane_with_id_mut(id)
                    .and_then(pane_tabs::take_carry)
            })
            .collect();
        for transfer in transfers {
            self.pane_tabs.insert(transfer.tabs.info.id, transfer.tabs);
            self.suppressed_panes.extend(transfer.children);
        }
        let client = self.connected_clients.borrow().iter().next().copied();
        if let Some(client) = client {
            let pending: Vec<_> = self
                .get_all_pane_ids()
                .into_iter()
                .filter_map(|id| {
                    self.get_pane_with_id_mut(id)
                        .and_then(|pane| pane.take_pane_tabs_layout())
                        .map(|layout| (id, layout))
                })
                .collect();
            for (id, layout) in pending {
                let existing = layout.active;
                self.mount_pane_tabs(id, layout, existing, client)?;
            }
        }
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
        self.update_agent_pane_widths()?;
        self.sync_pane_tabs()?;
        self.sync_pane_tab_titles();
        self.render_tiled_panes_and_chrome(output, client_id_override)?;
        self.render_floating_panes_and_overlays(output, client_id_override)?;
        for client in self.clients_in_frame(client_id_override) {
            let Some((x, y, true)) = self.get_active_terminal_cursor_position(client) else {
                continue;
            };
            let z_index = self.active_pane_z_index(client);
            if !self.active_terminal_is_mid_frame(client).unwrap_or(false)
                && output.cursor_is_visible(x, y, z_index, client)
            {
                output.protect_translucent_cursor_cell(client, x, y, z_index);
            }
        }
        output.finish_translucent_composition();
        Ok(())
    }

    pub(super) fn render_tiled_panes_and_chrome(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        let same_worktree_panes = self.same_worktree_panes(client_id_override);
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
        let translucent = self
            .overlay_panes
            .translucent_layers(self.floating_layer_count(), self.background_opacity);
        if !translucent.is_empty() {
            // ponytail: repaint intersecting panes on each requested frame; add persistent cell
            // caches only if measured panel repaint cost warrants their invalidation machinery.
            self.force_render_panes_intersecting(
                &translucent
                    .iter()
                    .map(|(_, rect, _)| *rect)
                    .collect::<Vec<_>>(),
            );
            let palette = *self.terminal_emulator_colors.borrow();
            output.begin_translucent_composition(
                &connected_clients,
                translucent,
                palette.fg,
                palette.bg,
                self.style.colors.text_unselected.background,
                &self.terminal_emulator_color_codes.borrow(),
            );
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
        if !self.pending_render_damage.is_empty() {
            // Margin clearing also covers gutter borders outside the pane geometry.
            self.tiled_panes.reset_boundaries();
            let rect = |geom: PaneGeom| CellRect {
                x: geom.x,
                y: geom.y,
                cols: geom.cols.as_usize(),
                rows: geom.rows.as_usize(),
            };
            let mut margin = EMPTY_TERMINAL_CHARACTER;
            margin.styles.update(|styles| {
                styles.background = Some(self.style.pane_surround().into());
            });
            let margin_chunks = |damage: &[CellRect], covered: &[CellRect]| {
                overlay_damage(damage, covered)
                    .into_iter()
                    .flat_map(|rect| {
                        let margin = &margin;
                        (rect.y..rect.y + rect.rows).map(move |y| {
                            CharacterChunk::new(vec![margin.clone(); rect.cols], rect.x, y)
                        })
                    })
                    .collect()
            };
            let covered: Vec<_> = self
                .tiled_panes
                .get_panes()
                .filter(|(id, _)| !self.tiled_panes.panes_to_hide_contains(**id))
                .map(|(_, pane)| rect(pane.render_geom()))
                .collect();
            output
                .add_character_chunks_to_multiple_clients(
                    margin_chunks(&self.pending_render_damage, &covered),
                    self.clients_in_frame(client_id_override).into_iter(),
                    None,
                )
                .context("failed to repaint tab margins")?;

            // Fullscreen margins occlude tiled output, so clear them at their own layer.
            let mut fullscreen_layers = Vec::new();
            if let Some(id) = self.floating_panes.fullscreen_pane_id()
                && let Some(index) = self.floating_panes.get_pane_z_index(id)
                && let Some(pane) = self.floating_panes.get_pane(id)
            {
                fullscreen_layers.push((index, pane.render_geom()));
            }
            if self.overlays_are_rendered() {
                for id in self.overlay_panes.fullscreen_pane_ids() {
                    if let Some(index) = self.overlay_panes.get_pane_z_index(id)
                        && let Some(geom) = self.overlay_panes.outer_geom(id)
                    {
                        fullscreen_layers.push((index + self.floating_layer_count(), geom));
                    }
                }
            }
            fullscreen_layers.sort_by_key(|(index, _)| *index);
            fullscreen_layers.dedup_by_key(|(index, _)| *index);
            if let Some(stack) = self.occlusion_stack() {
                for (index, rendered) in fullscreen_layers {
                    let bounds = rect(stack.layers[index]);
                    let damage: Vec<_> = self
                        .pending_render_damage
                        .iter()
                        .filter_map(|damage| {
                            let x = damage.x.max(bounds.x);
                            let y = damage.y.max(bounds.y);
                            let right = (damage.x + damage.cols).min(bounds.x + bounds.cols);
                            let bottom = (damage.y + damage.rows).min(bounds.y + bounds.rows);
                            (x < right && y < bottom).then(|| CellRect {
                                x,
                                y,
                                cols: right - x,
                                rows: bottom - y,
                            })
                        })
                        .collect();
                    output
                        .add_character_chunks_to_multiple_clients(
                            margin_chunks(&damage, &[rect(rendered)]),
                            self.clients_in_frame(client_id_override).into_iter(),
                            Some(index + 1),
                        )
                        .context("failed to repaint fullscreen margins")?;
                }
            }
            if output
                .pane_change_client
                .is_some_and(|client| connected_clients.contains(&client))
            {
                output
                    .rendered_margin_damage
                    .extend_from_slice(&self.pending_render_damage);
            }
            self.pending_render_damage.clear();
        }
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
                    &same_worktree_panes,
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
    pub(super) fn render_floating_panes_and_overlays(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) -> Result<()> {
        let same_worktree_panes = self.same_worktree_panes(client_id_override);
        if self.has_floating_panes_to_render() {
            let resize: Vec<_> = self
                .stack_lists
                .iter()
                .filter_map(|(id, list)| self.floating_panes.get_pane(list.visible).map(|_| *id))
                .collect();
            for id in resize {
                self.resize_stack_list_hidden_members(id);
            }
            let stack_list_header_rows = self
                .stack_lists
                .values()
                .filter(|list| self.floating_stack_headers_visible(list))
                .map(|list| (list.visible, list.members.len()))
                .collect();
            self.floating_panes.render(
                output,
                &self.mouse_hover_pane_id,
                self.current_pane_group.borrow().clone_inner(),
                client_id_override,
                &self.mouse_help_text_visible,
                self.mouse_scroll_resize,
                &same_worktree_panes,
                &stack_list_header_rows,
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
                &same_worktree_panes,
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

    pub(super) fn same_worktree_panes(
        &self,
        client_id_override: Option<ClientId>,
    ) -> HashMap<ClientId, HashSet<PaneId>> {
        self.clients_in_frame(client_id_override)
            .into_iter()
            .filter_map(|client| {
                let focused = self.get_active_pane_id(client)?;
                let root = self.agent_worktrees.get(&focused)?;
                let matching = self
                    .agent_worktrees
                    .iter()
                    .filter_map(|(id, candidate)| {
                        (*id != focused && candidate == root).then_some(*id)
                    })
                    .collect();
                Some((client, matching))
            })
            .collect()
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
    fn active_pane_z_index(&self, client_id: ClientId) -> Option<usize> {
        // Match the flat floating-then-overlay order used by occlusion_stack and rendering.
        self.get_active_pane_id(client_id).and_then(|pane_id| {
            self.floating_panes.get_pane_z_index(pane_id).or_else(|| {
                self.overlay_panes
                    .get_pane_z_index(pane_id)
                    .filter(|_| self.overlays_are_rendered())
                    .map(|index| index + self.floating_layer_count())
            })
        })
    }

    pub(super) fn render_cursor(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
    ) {
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
                    let active_pane_z_index = self.active_pane_z_index(client_id);
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
}
