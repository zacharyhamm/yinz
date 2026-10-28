//! Applying, validating, and cycling pane layouts.

use super::{StackList, Tab};
use crate::panes::PaneId;
use crate::plugins::PluginId;
use crate::pty_writer::PtyWriteInstruction;
use crate::route::NotificationEnd;
use crate::ClientId;
use std::collections::HashMap;
use yinz_panes::layout_applier::LayoutApplier;
use yinz_panes::pane::HoldForCommand;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::layout::FloatingPaneLayout;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::input::layout::SwapFloatingLayout;
use yinz_types::input::layout::SwapTiledLayout;
use yinz_types::input::layout::TiledPaneLayout;
use yinz_types::internal_ids::TerminalId;

impl Tab {
    pub(super) fn layout_applier(
        &mut self,
        blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> LayoutApplier<'_> {
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
            blocking_terminal,
        )
    }

    pub fn validate_layout(
        &mut self,
        layout: &TiledPaneLayout,
        floating_panes_layout: &[FloatingPaneLayout],
        new_terminal_ids: &[(TerminalId, HoldForCommand)],
        new_floating_terminal_ids: &[(TerminalId, HoldForCommand)],
        new_plugin_ids: &HashMap<RunPluginOrAlias, Vec<PluginId>>,
    ) -> Result<()> {
        self.layout_applier(None)
            .validate_layout(
                layout,
                floating_panes_layout,
                new_terminal_ids,
                new_floating_terminal_ids,
                new_plugin_ids,
            )
            .context("invalid tab layout")
    }

    pub fn validate_override_layout(
        &mut self,
        layout: &TiledPaneLayout,
        floating_panes_layout: &[FloatingPaneLayout],
        new_terminal_ids: &[(TerminalId, HoldForCommand)],
        new_floating_terminal_ids: &[(TerminalId, HoldForCommand)],
        new_plugin_ids: &HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
    ) -> Result<()> {
        self.layout_applier(None)
            .validate_override_layout(
                layout,
                floating_panes_layout,
                new_terminal_ids,
                new_floating_terminal_ids,
                new_plugin_ids,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
            )
            .context("invalid replacement tab layout")
    }

    pub fn apply_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        client_id: ClientId,
        mut blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> Result<()> {
        if !new_terminal_ids.is_empty()
            || !new_floating_terminal_ids.is_empty()
            || new_plugin_ids.values().any(|ids| !ids.is_empty())
        {
            self.agent_tab_cleanup_allowed = false;
        }
        if let Err(error) = self.validate_layout(
            &layout,
            &floating_panes_layout,
            &new_terminal_ids,
            &new_floating_terminal_ids,
            &new_plugin_ids,
        ) {
            if let Some((_, completion)) = blocking_terminal.as_mut() {
                completion.set_error_message(format!("{error:#}"));
                completion.set_exit_status(1);
            }
            return Err(error);
        }
        let should_show_floating_panes = self
            .layout_applier(blocking_terminal)
            .apply_layout(
                layout.clone(),
                floating_panes_layout.clone(),
                new_terminal_ids,
                new_floating_terminal_ids,
                new_plugin_ids,
                client_id,
            )
            .context("failed to apply tab layout")?;
        self.swap_layouts
            .set_base_layout((layout, floating_panes_layout));
        if should_show_floating_panes != self.floating_panes.panes_are_visible() {
            self.toggle_floating_panes(Some(client_id), None, None)
                .non_fatal();
        }
        self.tiled_panes.reapply_pane_frames();
        self.is_pending = false;
        self.sync_tab_bar_height()?;
        self.apply_buffered_instructions().non_fatal();
        self.complete_pending_agent_placements();
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
    pub fn override_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
        new_swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
        new_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_terminal_ids: Vec<(TerminalId, HoldForCommand)>,
        new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
        mut blocking_terminal: Option<(TerminalId, NotificationEnd)>,
    ) -> Result<()> {
        self.agent_tab_cleanup_allowed = false;
        if let Err(error) = self.validate_override_layout(
            &layout,
            &floating_panes_layout,
            &new_terminal_ids,
            &new_floating_terminal_ids,
            &new_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            client_id,
        ) {
            if let Some((_, completion)) = blocking_terminal.as_mut() {
                completion.set_error_message(format!("{error:#}"));
                completion.set_exit_status(1);
            }
            return Err(error);
        }
        let should_show_floating_panes = self
            .layout_applier(blocking_terminal)
            .override_layout(
                layout.clone(),
                floating_panes_layout.clone(),
                new_terminal_ids,
                new_floating_terminal_ids,
                new_plugin_ids,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
            )
            .context("failed to override tab layout")?;
        self.swap_layouts
            .set_swap_tiled_layouts(new_swap_tiled_layouts.unwrap_or_default());
        self.swap_layouts
            .set_swap_floating_layouts(new_swap_floating_layouts.unwrap_or_default());
        self.swap_layouts
            .set_base_layout((layout, floating_panes_layout));
        if should_show_floating_panes != self.floating_panes.panes_are_visible() {
            self.toggle_floating_panes(Some(client_id), None, None)
                .non_fatal();
        }

        // Retained panes absent from the replacement layout still need to fit its swap layout.
        self.swap_layouts.set_is_tiled_damaged();
        self.swap_layouts.set_is_floating_damaged();
        self.relayout_tiled_panes(false).non_fatal();
        self.relayout_floating_panes(false).non_fatal();
        self.tiled_panes.reapply_pane_frames();
        self.is_pending = false;
        LayoutApplier::offset_viewport(
            self.viewport.clone(),
            self.display_area.clone(),
            &mut self.tiled_panes,
            self.pane_frame_style,
        );
        self.sync_tab_bar_height()?;
        self.refresh_overlay_expansion();
        self.apply_buffered_instructions().non_fatal();
        self.complete_pending_agent_placements();
        Ok(())
    }
    pub fn swap_layout_info(&self) -> (Option<String>, bool) {
        if self.floating_panes.panes_are_visible() {
            self.swap_layouts.floating_layout_info()
        } else {
            self.swap_layouts.tiled_layout_info()
        }
    }
    pub(super) fn relayout_floating_panes(&mut self, search_backwards: bool) -> Result<()> {
        if let Some(layout_candidate) = self
            .swap_layouts
            .swap_floating_panes(&self.floating_panes, search_backwards)
        {
            self.layout_applier(None)
                .apply_floating_panes_layout_to_existing_panes(&layout_candidate)
                .non_fatal();
        }
        self.set_force_render();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::StartCachingResizes)
            .context("keep pane resizes batched until the screen event ends")?;
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
        self.agent_tab_cleanup_allowed = false;
        self.tiled_panes.unset_fullscreen();
        self.layout_applier(None)
            .apply_tiled_panes_layout_in_order(layout, Some(order))?;
        self.is_pending = false;
        self.apply_buffered_instructions()?;
        self.complete_pending_agent_placements();
        self.tiled_panes.reapply_pane_frames();
        self.resize_all_stack_list_hidden_members();
        self.swap_layouts.set_is_tiled_damaged();
        self.set_should_clear_display_before_rendering();
        self.set_force_render();
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::StartCachingResizes)
            .context("keep pane resizes batched until the screen event ends")?;
        Ok(())
    }

    pub(super) fn relayout_tiled_panes(&mut self, search_backwards: bool) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if let Some(layout_candidate) = self
            .swap_layouts
            .swap_tiled_panes(&self.tiled_panes, search_backwards)
        {
            let application_res = self
                .layout_applier(None)
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
            .send_to_pty_writer(PtyWriteInstruction::StartCachingResizes)
            .context("keep pane resizes batched until the screen event ends")?;
        Ok(())
    }
    pub fn previous_swap_layout(&mut self) -> Result<()> {
        let search_backwards = true;
        if self.floating_panes.panes_are_visible() {
            self.relayout_floating_panes(search_backwards)?;
        } else {
            self.relayout_tiled_panes(search_backwards)?;
        }
        self.sync_tab_bar_height()
    }
    pub fn next_swap_layout(&mut self) -> Result<()> {
        let search_backwards = false;
        if self.floating_panes.panes_are_visible() {
            self.relayout_floating_panes(search_backwards)?;
        } else {
            self.relayout_tiled_panes(search_backwards)?;
        }
        self.sync_tab_bar_height()
    }
}
