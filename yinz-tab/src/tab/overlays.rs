//! Overlay containers, placement, visibility, and occlusion.

use super::ExtractedPanePlacement;
use super::OverlayTransfer;
use super::Tab;
use crate::output::FloatingPanesStack;
use crate::panes::OverlayCarry;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::ui::boundaries::Boundaries;
use crate::ui::damage::overlay_damage;
use crate::ui::damage::CellRect;
use crate::ui::pane_contents_and_ui::frame_color;
use crate::ui::pane_contents_and_ui::FrameColorState;
use crate::ClientId;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::time::Instant;
use yinz_panes::pane::Pane;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;
use yinz_types::data::OverlayPlacement;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::pane_size::Dimension;
use yinz_types::pane_size::Offset;
use yinz_types::pane_size::PaneGeom;
use yinz_types::position::Position;

impl Tab {
    pub fn pane_is_overlay(&self, pane_id: PaneId) -> bool {
        self.overlay_panes.panes_contain(&pane_id)
    }
    pub fn multipane_info(&self, pane_id: PaneId) -> Option<yinz_types::multipane::MultipaneInfo> {
        self.overlay_panes.multipane_info(&pane_id)
    }
    /// Remove one multipane child, leaving its sibling as the container's only pane. This is how a
    /// slot held for a child that will not come is given up: `close_pane` keeps a member in
    /// place (a command that exits there stays held), and closing the container would take the
    /// sibling too. Nothing is sent to the pty: the caller decides what the removed pane's
    /// process or plugin is owed. Returns whether `id` was removed; `false`, with nothing
    /// changed, when it is no multipane member, holds the place of a suppressed pane, which
    /// `close_pane` restores instead, or belongs to pane tabs, whose child `close_pane` closes
    /// to a blank slot that keeps the container's place.
    pub fn remove_multipane_member(&mut self, id: PaneId) -> bool {
        if self.multipane_info(id).is_none()
            || self.stands_in_for_a_pane(id).is_some()
            || self.pane_tabs_of(id).is_some()
        {
            return false;
        }
        if self.overlay_panes.remove_multipane_member(id).is_none() {
            return false;
        }
        self.pending_command_respawns.remove(&id);
        self.set_force_render();
        self.overlay_panes.set_force_render();
        self.announce_pane_closed(id);
        self.repair_focus_after_pane_removal();
        true
    }
    pub fn retain_tab_for_multipane(&mut self, child: PaneId) {
        if self.multipane_info(child).is_some() {
            self.overlay_panes.retain_tab_for(child);
        }
    }
    pub fn take_overlay_container(&mut self, pane_id: PaneId) -> Option<OverlayTransfer> {
        let container = self.overlay_panes.take_container(pane_id)?;
        let mut members: HashSet<_> = container.pane_ids().collect();
        let mut pane_tabs = BTreeMap::new();
        loop {
            let previous = members.len();
            let tab_ids: Vec<_> = members
                .iter()
                .filter_map(|pane| self.pane_tabs_of(*pane))
                .collect();
            for id in tab_ids {
                if let Some(tabs) = self.pane_tabs.remove(&id) {
                    members.extend(tabs.info.tabs.iter().filter_map(|slot| slot.pane_id()));
                    pane_tabs.insert(id, tabs);
                }
            }
            for (id, pane) in &self.suppressed_panes {
                if pane
                    .restored_by
                    .is_some_and(|stand_in| members.contains(&stand_in.pane_id))
                {
                    members.insert(*id);
                }
            }
            if previous == members.len() {
                break;
            }
        }
        let suppressed = self
            .suppressed_panes
            .extract_if(|id, _| members.contains(id))
            .collect();
        let respawns = self
            .pending_command_respawns
            .extract_if(|id, _| members.contains(id))
            .collect();
        let pending_output = self
            .pending_vte_events
            .extract_if(|id, _| members.contains(&PaneId::Terminal(*id)))
            .collect();
        let waiting_resize = self
            .pids_waiting_resize
            .extract_if(|id| members.contains(&PaneId::Terminal(*id)))
            .collect();
        self.set_should_clear_display_before_rendering();
        self.set_force_render();
        Some(OverlayTransfer {
            pane_tabs,
            container,
            suppressed,
            respawns,
            pending_output,
            waiting_resize,
        })
    }
    pub fn accept_overlay_container(&mut self, transfer: OverlayTransfer) {
        let member = transfer.container.pane_ids().next();
        self.pane_tabs.extend(transfer.pane_tabs);
        self.suppressed_panes.extend(transfer.suppressed);
        self.pending_command_respawns.extend(transfer.respawns);
        self.pending_vte_events.extend(transfer.pending_output);
        self.pids_waiting_resize.extend(transfer.waiting_resize);
        self.overlay_panes.insert_container(transfer.container);
        if let Some(member) = member {
            self.notify_overlay_plugins_of_visibility(member);
        }
        self.set_force_render();
    }
    pub fn validate_multipane_child(&self, pane_id: PaneId) -> Result<()> {
        if !self.has_pane_with_pid(&pane_id) {
            return Err(anyhow!("multipane child not found"));
        }
        if self.multipane_info(pane_id).is_some() {
            return Err(anyhow!("nested multipanes are not supported"));
        }
        if self.pane_is_stack_list_member(&pane_id)
            || self
                .suppressed_panes
                .get(&pane_id)
                .is_some_and(|pane| pane.restored_by.is_some())
            || self.stands_in_for_a_pane(pane_id).is_some()
        {
            return Err(anyhow!("cannot adopt a stacked or replaced pane"));
        }
        Ok(())
    }
    pub fn create_multipane(
        &mut self,
        children: [PaneId; 2],
        split: yinz_types::multipane::MultipaneSplit,
        min_cells: u16,
        placement: OverlayPlacement,
    ) -> Result<yinz_types::multipane::MultipaneId> {
        placement.validate().map_err(|error| anyhow!(error))?;
        split.validate().map_err(|error| anyhow!(error))?;
        if children[0] == children[1] {
            return Err(anyhow!("multipane children must be distinct"));
        }
        for child in children {
            self.validate_multipane_child(child)?;
        }
        // Validation precedes all extraction, including validation of existing overlay kinds.
        for child in children {
            if let Some(kind) = self.overlay_panes.kind_of(&child)
                && kind != placement.kind
            {
                return Err(anyhow!(
                    "multipane children must have the requested overlay kind"
                ));
            }
        }
        // Adopting the tab's working panes must not make settlement destroy them. A panel
        // made only from existing overlays (agent) keeps its previous session-lifetime policy.
        let retains_tab = children
            .iter()
            .any(|id| !self.pane_is_overlay(*id) && !self.suppressed_panes.contains_key(id));
        let focused: Vec<_> = self
            .connected_clients
            .borrow()
            .iter()
            .filter_map(|client| {
                self.get_active_pane_id(*client)
                    .filter(|pane| children.contains(pane))
                    .map(|pane| (*client, pane))
            })
            .collect();
        for child in children {
            if !self.pane_is_overlay(child) {
                let mut pane = self
                    .extract_pane(child, true)
                    .context("validated multipane child disappeared")?;
                self.retain_pane_tabs_transfer(&mut pane);
                self.overlay_panes.add_pane(
                    child,
                    pane,
                    placement.kind,
                    FloatingPaneCoordinates::default(),
                    OverlayCarry::default(),
                );
            }
        }
        let id = self
            .overlay_panes
            .create_multipane(children, split, min_cells, placement)?;
        if retains_tab {
            self.overlay_panes.retain_tab_for(children[0]);
        }
        for (client, pane) in focused {
            self.overlay_panes.focus_pane(pane, client);
        }
        self.notify_overlay_plugins_of_visibility(children[0]);
        self.set_force_render();
        Ok(id)
    }
    pub fn update_multipane_split(
        &mut self,
        id: yinz_types::multipane::MultipaneId,
        split: yinz_types::multipane::MultipaneSplit,
    ) -> Result<()> {
        self.overlay_panes.update_split(id, split)
    }
    /// Set an overlay aside without unloading it - the overlay half of `suppress_pane`, which
    /// cannot be used here because it extracts the pane from its collection and an overlay put
    /// back afterwards would have lost its kind and its coordinates.
    pub fn hide_overlay_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.overlay_panes.hide_pane(pane_id) {
            return false;
        }
        self.notify_overlay_plugins_of_visibility(pane_id);
        // An overlay is drawn after the tab, so the cells it vacates have no other
        // writer: without dirtying everything underneath, the overlay stays on
        // screen as an unrepainted ghost of itself.
        self.set_should_clear_display_before_rendering();
        self.set_force_render();
        true
    }
    /// Draw an overlay again. Restoring a modal can take focus; see `OverlayPanes::show_pane`.
    pub fn show_overlay_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.overlay_panes.show_pane(pane_id) {
            return false;
        }
        self.notify_overlay_plugins_of_visibility(pane_id);
        self.set_force_render();
        true
    }
    fn notify_overlay_plugins_of_visibility(&mut self, pane_id: PaneId) {
        let carry = self.overlay_panes.carry_of(&pane_id);
        let visible = self.tab_visible && !carry.hidden && !carry.awaiting_placement;
        let updates: Vec<_> = self
            .overlay_panes
            .members(&pane_id)
            .into_iter()
            .filter(|id| self.pane_tabs_of(*id).is_none())
            .filter_map(|id| match id {
                PaneId::Plugin(id) => Some((Some(id), None, Event::Visible(visible))),
                _ => None,
            })
            .collect();
        self.sync_pane_tabs().non_fatal();
        if !updates.is_empty() {
            self.senders
                .send_to_plugin(PluginInstruction::Update(updates))
                .context("failed to report overlay container visibility")
                .non_fatal();
        }
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
    pub(super) fn occlusion_stack(&self) -> Option<FloatingPanesStack> {
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
    pub(super) fn floating_layer_count(&self) -> usize {
        self.floating_panes
            .stack()
            .map(|stack| stack.layers.len())
            .unwrap_or(0)
    }
    pub fn update_background_opacity(&mut self, opacity: u8) {
        if self.background_opacity != opacity {
            self.background_opacity = opacity;
            self.set_force_render();
        }
    }

    pub fn set_overlay_background_opacity(&mut self, pane: PaneId, opacity: u8) -> Result<()> {
        self.overlay_panes.set_background_opacity(pane, opacity)?;
        self.set_force_render();
        Ok(())
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

    fn waiting_border_is_visible(
        &self,
        pane_id: PaneId,
        client_id_override: Option<ClientId>,
    ) -> bool {
        let Some(pane) = self.get_pane_with_id(pane_id) else {
            return false;
        };
        if pane.borderless() || pane.rows() == 0 || pane.cols() == 0 {
            return false;
        }
        let groups = self.current_pane_group.borrow().clone_inner();
        let clients = self.connected_clients.borrow();
        // Group colors take precedence over overrides in both frames and shared boundaries.
        let frame_can_pulse = clients
            .iter()
            .copied()
            .chain(client_id_override)
            .any(|client_id| {
                pane.frame_color_override(client_id).is_none()
                    || groups
                        .get(&client_id)
                        .is_some_and(|group| group.contains(&pane_id))
            });
        if !frame_can_pulse {
            return false;
        }
        if let Some(list) = self
            .stack_list_id_of_member(&pane_id)
            .and_then(|id| self.stack_lists.get(&id))
        {
            let visible = if self.floating_panes.panes_contain(&list.visible) {
                self.has_floating_panes_to_render()
                    && self
                        .floating_panes
                        .rendered_pane_ids()
                        .contains(&list.visible)
                    && self.floating_stack_headers_visible(list)
            } else {
                !self.tiled_panes.fullscreen_is_active()
                    && !self.tiled_panes.panes_to_hide_contains(list.visible)
            };
            if visible
                && self
                    .get_pane_with_id(list.visible)
                    .zip(list.rank_of(&pane_id))
                    .is_some_and(|(visible_pane, rank)| {
                        let header = Self::stack_list_header_geom(
                            visible_pane.render_geom(),
                            visible_pane.get_content_offset(),
                            rank,
                        );
                        self.waiting_border_is_uncovered(list.visible, &[header])
                    })
            {
                return true;
            }
            if list.visible != pane_id {
                return false;
            }
        }
        let geom = pane.render_geom();
        let is_tiled = self.tiled_panes.panes_contain(&pane_id);
        let floating_gutters = self.floating_panes.fullscreen_pane_id() == Some(pane_id)
            && !self.pane_frame_style.draws_full_frames();
        let viewport = if floating_gutters {
            yinz_types::pane_size::Viewport {
                x: geom.x,
                y: geom.y,
                cols: geom.cols.as_usize(),
                rows: geom.rows.as_usize(),
            }
        } else {
            self.tiled_panes.boundary_viewport()
        };
        let gutter_outer = if (is_tiled && self.tiled_panes.fullscreen_covers_ui())
            || (floating_gutters && self.floating_panes.fullscreen_covers_ui())
        {
            (*self.display_area.borrow()).into()
        } else {
            *self.viewport.borrow()
        };
        let has_gutters = !self.pane_frame_style.draws_full_frames()
            && pane.max_content_width() > 0
            && !pane.borderless()
            && (geom.x > pane.current_geom().x
                || geom.cols.as_usize() < pane.current_geom().cols.as_usize());
        let visible = if self.tiled_panes.panes_contain(&pane_id) {
            !self.tiled_panes.panes_to_hide_contains(pane_id)
                && (!self.tiled_panes.fullscreen_covers_ui() || has_gutters)
                && (has_gutters
                    || pane.get_content_offset() != Offset::default()
                    || geom.is_stacked()
                    || geom.x > viewport.x
                    || geom.y > viewport.y)
        } else {
            (pane.get_content_offset() != Offset::default() || floating_gutters)
                && ((self.has_floating_panes_to_render()
                    && self.floating_panes.rendered_pane_ids().contains(&pane_id))
                    || self.overlay_panes.rendered_pane_ids().contains(&pane_id))
        };
        if !visible {
            return false;
        }
        if !is_tiled {
            if pane.get_content_offset() != Offset::default()
                && self.waiting_border_is_uncovered(pane_id, &[geom])
            {
                return true;
            }
            if !floating_gutters {
                return false;
            }
        } else if self.pane_frame_style.draws_full_frames() {
            return self.waiting_border_is_uncovered(pane_id, &[geom]);
        }

        // ponytail: scan stack members per pane; precompute flags if large stacks make this costly.
        let stack_flags = |pane: &dyn Pane| {
            let geom = pane.position_and_size();
            let mut top = true;
            let mut bottom = true;
            let mut under = false;
            if let Some(stack_id) = geom.stacked {
                for (_, other) in self.tiled_panes.get_panes() {
                    let other = other.position_and_size();
                    if other.stacked == Some(stack_id)
                        && other.x == geom.x
                        && other.cols == geom.cols
                    {
                        top &= other.y >= geom.y;
                        bottom &= other.y <= geom.y;
                        under |= other.y < geom.y && other.rows.is_percent();
                    }
                }
            }
            (top, bottom, under)
        };
        // Titles and stack headers are rendered separately from shared boundaries.
        if is_tiled
            && (geom.is_stacked()
                || (self.pane_frame_style.draws_titles() && pane.get_content_offset().top > 0))
        {
            let (_, _, under) = stack_flags(pane);
            let header = PaneGeom {
                y: geom.y.saturating_sub(if under {
                    pane.get_content_offset().bottom
                } else {
                    0
                }),
                rows: Dimension::fixed(1),
                ..geom
            };
            if self.waiting_border_is_uncovered(pane_id, &[header]) {
                return true;
            }
        }
        self.connected_clients
            .borrow()
            .iter()
            .copied()
            .chain(client_id_override)
            .any(|client_id| {
                let mode = self
                    .mode_info
                    .borrow()
                    .get(&client_id)
                    .unwrap_or(&self.default_mode_info)
                    .mode;
                let mut unpulsed = Boundaries::new(viewport);
                let mut pulsed = Boundaries::new(viewport);
                let panes = self
                    .tiled_panes
                    .get_panes()
                    .filter(|(id, _)| is_tiled && !self.tiled_panes.panes_to_hide_contains(**id));
                let floating = self
                    .floating_panes
                    .get_panes()
                    .filter(|(id, _)| floating_gutters && **id == pane_id);
                for (id, other) in panes.chain(floating) {
                    let (top, bottom, under) = stack_flags(other.as_ref());
                    for (boundaries, pulse) in [(&mut unpulsed, false), (&mut pulsed, true)] {
                        let color = frame_color(
                            &self.style,
                            FrameColorState {
                                host_colors: other.host_colors(),
                                frame_color_override: other.frame_color_override(client_id).map(
                                    |color| (color, other.frame_color_override_strength(client_id)),
                                ),
                                // Test a positive pulse, even at the cycle's zero point or before
                                // its first sample. Otherwise visibility could stop its own clock.
                                waiting_pulse: (pulse && *id == pane_id).then_some(0.4),
                                pane_is_focused: if is_tiled {
                                    !self.floating_panes.panes_are_visible()
                                        && self.tiled_panes.get_active_pane_id(client_id)
                                            == Some(*id)
                                } else {
                                    self.floating_panes.get_active_pane_id(client_id) == Some(*id)
                                },
                                pane_is_in_group: groups
                                    .get(&client_id)
                                    .is_some_and(|group| group.contains(id)),
                                mouse_is_hovering_over_pane: self
                                    .mouse_hover_pane_id
                                    .get(&client_id)
                                    == Some(id),
                                multiple_users_exist_in_session: self
                                    .connected_clients_in_app
                                    .borrow()
                                    .len()
                                    > 1,
                                color_slot: self
                                    .connected_clients_in_app
                                    .borrow()
                                    .color_slot(&client_id),
                            },
                            mode,
                            self.stack_list_session_is_mirrored,
                        );
                        if is_tiled && !self.tiled_panes.fullscreen_covers_ui() {
                            boundaries.add_rect(
                                other.as_ref(),
                                color,
                                self.style.chrome_colors(),
                                top,
                                bottom,
                                under,
                            );
                        }
                        boundaries.add_gutter_edges(
                            other.as_ref(),
                            gutter_outer,
                            color,
                            self.style.chrome_colors(),
                        );
                    }
                }
                let cells: Vec<_> = pulsed
                    .changed_color_cells(&unpulsed)
                    .map(|(x, y)| PaneGeom {
                        x,
                        y,
                        cols: Dimension::fixed(1),
                        rows: Dimension::fixed(1),
                        ..geom
                    })
                    .collect();
                !cells.is_empty() && self.waiting_border_is_uncovered(pane_id, &cells)
            })
    }

    fn waiting_border_is_uncovered(&self, pane_id: PaneId, geoms: &[PaneGeom]) -> bool {
        let Some(stack) = self.occlusion_stack() else {
            return true;
        };
        let floating_layers = self.floating_layer_count();
        let first_above = self
            .floating_panes
            .get_pane_z_index(pane_id)
            .or_else(|| {
                self.overlay_panes
                    .get_pane_z_index(pane_id)
                    .map(|index| index + floating_layers)
            })
            .map_or(0, |index| index + 1);
        let translucent = self
            .overlay_panes
            .translucent_layers(floating_layers, self.background_opacity);
        let covered: Vec<_> = stack
            .layers
            .iter()
            .enumerate()
            .skip(first_above)
            .filter(|(index, _)| !translucent.iter().any(|(layer, _, _)| layer == index))
            .map(|(_, geom)| CellRect {
                x: geom.x,
                y: geom.y,
                cols: geom.cols.as_usize(),
                rows: geom.rows.as_usize(),
            })
            .collect();
        let rects: Vec<_> = geoms
            .iter()
            .map(|geom| CellRect {
                x: geom.x,
                y: geom.y,
                cols: geom.cols.as_usize(),
                rows: geom.rows.as_usize(),
            })
            .collect();
        !overlay_damage(&rects, &covered).is_empty()
    }

    pub fn agent_title_is_visible(
        &self,
        pane_id: PaneId,
        client_id_override: Option<ClientId>,
    ) -> bool {
        if let Some(list) = self
            .stack_list_id_of_member(&pane_id)
            .and_then(|id| self.stack_lists.get(&id))
            && self.title_surface_is_visible(list.visible)
            && let Some((visible, rank)) = self
                .get_pane_with_id(list.visible)
                .zip(list.rank_of(&pane_id))
        {
            let headers_visible = if self.floating_panes.panes_contain(&list.visible) {
                self.floating_stack_headers_visible(list)
            } else {
                !self.tiled_panes.fullscreen_is_active()
            };
            let header = Self::stack_list_header_geom(
                visible.render_geom(),
                visible.get_content_offset(),
                rank,
            );
            if headers_visible && self.waiting_border_is_uncovered(list.visible, &[header]) {
                return true;
            }
        }
        if let Some(info) = self
            .pane_tabs_of(pane_id)
            .and_then(|id| self.pane_tabs_info(id))
            && let Some(pane) = self.get_pane_with_id(info.pane_id)
            && self.title_surface_is_visible(info.pane_id)
            && pane.get_content_y() > pane.y()
            && pane.get_content_rows() > 0
        {
            let titles = self.pane_tab_title_presentations(&info);
            let visible = self
                .connected_clients
                .borrow()
                .iter()
                .copied()
                .chain(client_id_override)
                .any(|client| {
                    yinz_panes::pane_tabs::bar_cells_with_titles(
                        &info,
                        pane.pane_tabs_view(client),
                        info.bar_focused.contains(&client),
                        pane.get_content_columns(),
                        &titles,
                    )
                    .iter()
                    .any(|cell| {
                        matches!(cell.target, yinz_types::tab_bar::TabTarget::Tab(index)
                        if info.tabs[index].pane_id() == Some(pane_id))
                    })
                });
            let mut bar = pane.render_geom();
            bar.y = pane.get_content_y() - 1;
            bar.rows = Dimension::fixed(1);
            if visible && self.waiting_border_is_uncovered(info.pane_id, &[bar]) {
                return true;
            }
        }
        let Some(pane) = self.get_pane_with_id(pane_id) else {
            return false;
        };
        let stacked = pane.current_geom().is_stacked();
        if self.tiled_panes.panes_contain(&pane_id)
            && !self.pane_frame_style.draws_full_frames()
            && !self.pane_frame_style.draws_titles()
            && !stacked
        {
            return false;
        }
        if !self.title_surface_is_visible(pane_id)
            || pane.borderless()
            || pane.render_geom().cols.as_usize() <= 2
            || pane.rows() == 0
            || (pane.get_content_y() == pane.y() && !stacked)
        {
            return false;
        }
        let mut title = pane.render_geom();
        if !self.pane_frame_style.draws_full_frames() && self.tiled_panes.panes_contain(&pane_id) {
            let geom = pane.position_and_size();
            let stacked_under = geom.stacked.is_some()
                && self.tiled_panes.get_panes().any(|(_, other)| {
                    let other = other.position_and_size();
                    other.stacked == geom.stacked
                        && other.x == geom.x
                        && other.cols == geom.cols
                        && other.y < geom.y
                        && other.rows.is_percent()
                });
            if stacked_under {
                title.y = title.y.saturating_sub(pane.get_content_offset().bottom);
            }
        }
        title.rows = Dimension::fixed(1);
        self.waiting_border_is_uncovered(pane_id, &[title])
    }

    fn title_surface_is_visible(&self, pane_id: PaneId) -> bool {
        if self.tiled_panes.panes_contain(&pane_id) {
            !self.tiled_panes.panes_to_hide_contains(pane_id)
                && !self.tiled_panes.fullscreen_covers_ui()
        } else {
            (self.has_floating_panes_to_render()
                && self.floating_panes.rendered_pane_ids().contains(&pane_id))
                || self.overlay_panes.rendered_pane_ids().contains(&pane_id)
        }
    }

    pub fn has_visible_waiting_pulse(&self, client_id_override: Option<ClientId>) -> bool {
        self.get_all_pane_ids().into_iter().any(|id| {
            self.get_pane_with_id(id)
                .is_some_and(|pane| pane.waiting_pulse_started_at().is_some())
                && self.waiting_border_is_visible(id, client_id_override)
        })
    }

    pub fn sample_waiting_pulses(
        &mut self,
        now: Instant,
        enabled: bool,
        client_id_override: Option<ClientId>,
    ) {
        for id in self.get_all_pane_ids() {
            if !self.get_pane_with_id(id).is_some_and(|pane| {
                pane.waiting_pulse_started_at().is_some() || pane.waiting_pulse().is_some()
            }) {
                continue;
            }
            let visible = enabled && self.waiting_border_is_visible(id, client_id_override);
            if let Some(pane) = self.get_pane_with_id_mut(id) {
                pane.sample_waiting_pulse(now, visible);
            }
        }
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
}
