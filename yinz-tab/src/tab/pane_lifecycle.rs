//! Closing, extracting, suppressing, and restoring owned panes.

use super::overlay_placement_for;
use super::run_awaits_placement;
use super::run_requests_overlay;
use super::AcceptedPane;
use super::BufferedTabInstruction;
use super::ExtractedPanePlacement;
use super::OverlayTransfer;
use super::StackListRemoval;
use super::SuppressedPane;
use super::SuppressedPanes;
use super::Tab;
use crate::panes::OverlayCarry;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::screen::ScreenInstruction;
use crate::ClientId;
use std::time::Instant;
use yinz_panes::pane::Pane;
use yinz_panes::resize_pty;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;
use yinz_types::data::OverlayPlacement;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::geometry::PercentOrFixed;
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_size::Offset;
use yinz_types::pane_size::PaneGeom;

impl Tab {
    pub fn close_pane(
        &mut self,
        id: PaneId,
        ignore_suppressed_panes: bool,
        exit_status: Option<i32>,
    ) {
        if !ignore_suppressed_panes && let Some(tabs_id) = self.pane_tabs_of(id) {
            if matches!(id, PaneId::Terminal(_)) {
                if let Some(command) = self.pane_run_command(id) {
                    self.hold_pane(id, exit_status, false, command);
                }
                return;
            }
            if let Some(index) = self.pane_tabs[&tabs_id]
                .info
                .tabs
                .iter()
                .position(|slot| slot.pane_id() == Some(id))
            {
                self.close_pane_tab_child(tabs_id, index).non_fatal();
                return;
            }
        }
        let id_stands_in_for_a_pane = self.stands_in_for_a_pane(id).is_some();
        if self.multipane_info(id).is_some() && !id_stands_in_for_a_pane {
            if let Some(command) = self.pane_run_command(id) {
                self.hold_pane(id, exit_status, false, command);
            }
            return;
        }
        self.pending_command_respawns.remove(&id);
        if !ignore_suppressed_panes
            && !id_stands_in_for_a_pane
            && self.pane_is_stack_list_member(&id)
            && self.close_stack_list_member(id, exit_status)
        {
            self.announce_pane_closed(id);
            self.repair_focus_after_pane_removal();
            return;
        }
        // we need to ignore suppressed panes when we toggle a pane to be floating/embedded(tiled)
        // this is because in that case, while we do use this logic, we're not actually closing the
        // pane, we're moving it
        if !ignore_suppressed_panes && id_stands_in_for_a_pane {
            match self.replace_pane_with_suppressed_pane(id) {
                // The stand-in has left the layout even though the tab did not shrink, so it is
                // as closed as any other pane and the same things have to hear about it.
                // `close_pane_and_replace_with_other_pane` has always said so for its own swap.
                Ok(Some(_pane)) => {
                    self.announce_pane_closed(id);
                    self.repair_focus_after_pane_removal();
                },
                Ok(None) => {},
                Err(e) => {
                    Err::<(), _>(e)
                        .with_context(|| format!("failed to close pane {:?}", id))
                        .non_fatal();
                },
            }
            return;
        }
        // A suppressed intermediate owns no layout slot. Its predecessors inherit the
        // complete surviving restoration link, or stay parked if it had no successor.
        if !ignore_suppressed_panes && let Some(mut removed) = self.suppressed_panes.remove(&id) {
            let successor = removed
                .restored_by
                .filter(|stand_in| self.has_pane_with_pid(&stand_in.pane_id));
            for predecessor in self.suppressed_panes.values_mut() {
                if predecessor
                    .restored_by
                    .is_some_and(|stand_in| stand_in.pane_id == id)
                {
                    predecessor.restored_by = successor;
                }
            }
            if let Some(exit_status) = exit_status {
                removed.pane.update_exit_status(exit_status);
            }
            self.announce_pane_closed(id);
            return;
        }
        let closed_pane = if self.overlay_panes.panes_contain(&id) {
            // nothing to unset, hide or relayout: an overlay is not on a layer and does not tile
            self.overlay_panes.move_clients_out_of_pane(id);
            let closed_pane = self.overlay_panes.remove_pane(id);
            self.set_force_render();
            self.overlay_panes.set_force_render();
            closed_pane
        } else if self.floating_panes.panes_contain(&id) {
            if self.floating_panes.fullscreen_pane_id() == Some(id) {
                self.floating_panes.unset_fullscreen();
            }
            let closed_pane = self.floating_panes.remove_pane(id);
            if !self.floating_panes.has_selectable_panes() {
                self.swap_layouts.reset_floating_damage();
                self.hide_floating_panes();
            }
            self.set_force_render();
            self.floating_panes.set_force_render();
            if self.auto_layout
                && !self.swap_layouts.is_floating_damaged()
                && self.floating_panes.visible_panes_count() > 0
            {
                self.swap_layouts.set_is_floating_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_floating_panes(false);
            }
            closed_pane
        } else {
            if self.tiled_panes.fullscreen_is_active() {
                self.tiled_panes.unset_fullscreen();
            }
            let closed_pane = self.tiled_panes.remove_pane(id);
            // With only floating panes left, no tiled pane repaints the vacated area.
            if self.tiled_panes.first_selectable_pane_id().is_none() {
                self.set_should_clear_display_before_rendering();
            }
            self.set_force_render();
            self.tiled_panes.set_force_render();
            if self.auto_layout && !self.swap_layouts.is_tiled_damaged() {
                self.swap_layouts.set_is_tiled_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_tiled_panes(false);
            }
            self.resize_all_stack_list_hidden_members();
            closed_pane
        };
        if let Some(mut closed_pane) = closed_pane {
            if let Some(exit_status) = exit_status {
                // in case we need to update on Drop
                closed_pane.update_exit_status(exit_status);
            }
            if !ignore_suppressed_panes {
                self.announce_pane_closed(id);
            }
            self.repair_focus_after_pane_removal();
        }
    }
    pub fn extract_pane(
        &mut self,
        id: PaneId,
        dont_swap_if_suppressed: bool,
    ) -> Option<Box<dyn Pane>> {
        let id = self
            .pane_tabs_of(id)
            .map(|tabs| self.pane_tabs[&tabs].info.pane_id)
            .unwrap_or(id);
        let mut pane = self.extract_pane_inner(id, dont_swap_if_suppressed)?;
        self.attach_pane_tabs_transfer(&mut pane);
        Some(pane)
    }

    fn extract_pane_inner(
        &mut self,
        id: PaneId,
        dont_swap_if_suppressed: bool,
    ) -> Option<Box<dyn Pane>> {
        // Structural moves must take the owning container, never an individual child.
        if self.overlay_panes.multipane_info(&id).is_some() {
            return None;
        }
        let id_stands_in_for_a_pane = self.stands_in_for_a_pane(id).is_some();
        if self.pane_is_stack_list_member(&id)
            && (dont_swap_if_suppressed || !id_stands_in_for_a_pane)
            && let StackListRemoval::Removed(removed) = self.remove_stack_list_member(id)
        {
            return removed.map(|mut pane| {
                pane.reset_logical_position();
                pane
            });
        }
        if !dont_swap_if_suppressed && id_stands_in_for_a_pane {
            // this is done for the scrollback editor
            return match self.replace_pane_with_suppressed_pane(id) {
                Ok(mut pane) => {
                    // we do this so that the logical index will not affect ordering in the target tab
                    if let Some(pane) = pane.as_mut() {
                        pane.reset_logical_position();
                    }
                    pane
                },
                Err(e) => {
                    Err::<(), _>(e)
                        .with_context(|| format!("failed to close pane {:?}", id))
                        .non_fatal();
                    None
                },
            };
        }
        if self.overlay_panes.panes_contain(&id) {
            // nothing to unset, hide or relayout: an overlay is not on a layer and does not tile
            self.overlay_panes.move_clients_out_of_pane(id);
            let mut closed_pane = self.overlay_panes.remove_pane(id);
            self.set_force_render();
            self.overlay_panes.set_force_render();
            // we do this so that the logical index will not affect ordering in the target tab
            if let Some(closed_pane) = closed_pane.as_mut() {
                closed_pane.reset_logical_position();
            }
            closed_pane
        } else if self.floating_panes.panes_contain(&id) {
            if self.floating_panes.fullscreen_pane_id() == Some(id) {
                self.floating_panes.unset_fullscreen();
            }
            let mut closed_pane = self.floating_panes.remove_pane(id);
            if !self.floating_panes.has_panes() {
                self.swap_layouts.reset_floating_damage();
                self.hide_floating_panes();
            }
            self.set_force_render();
            self.floating_panes.set_force_render();
            if self.auto_layout
                && !self.swap_layouts.is_floating_damaged()
                && self.floating_panes.visible_panes_count() > 0
            {
                self.swap_layouts.set_is_floating_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_floating_panes(false);
            }
            // we do this so that the logical index will not affect ordering in the target tab
            if let Some(closed_pane) = closed_pane.as_mut() {
                closed_pane.reset_logical_position();
            }
            closed_pane
        } else if self.tiled_panes.panes_contain(&id) {
            if self.tiled_panes.fullscreen_is_active() {
                self.tiled_panes.unset_fullscreen();
            }
            let mut closed_pane = self.tiled_panes.remove_pane(id);
            self.set_force_render();
            self.tiled_panes.set_force_render();
            if self.auto_layout && !self.swap_layouts.is_tiled_damaged() {
                self.swap_layouts.set_is_tiled_damaged();
                // only relayout if the user is already "in" a layout, otherwise this might be
                // confusing
                let _ = self.relayout_tiled_panes(false);
            }
            self.resize_all_stack_list_hidden_members();
            // we do this so that the logical index will not affect ordering in the target tab
            if let Some(closed_pane) = closed_pane.as_mut() {
                closed_pane.reset_logical_position();
            }
            closed_pane
        } else {
            match self.suppressed_panes.remove(&id) {
                Some(mut suppressed) => {
                    suppressed.pane.reset_logical_position();
                    Some(suppressed.pane)
                },
                _ => None,
            }
        }
    }
    /// Returns a replacement command that arrived while this one was still running, so the
    /// caller can start it only after firing this exit's hooks - starting it here, before the
    /// caller's hooks run, would fire them against the replacement instead of the pane that
    /// actually exited, and would skip `agent_start` since it wouldn't go through
    /// `Screen::run_command_in_pane_with_id`.
    pub fn hold_pane(
        &mut self,
        id: PaneId,
        exit_status: Option<i32>,
        is_first_run: bool,
        run_command: RunCommand,
    ) -> Option<(TerminalId, RunCommand, bool)> {
        if self.is_pending {
            self.pending_instructions
                .push(BufferedTabInstruction::HoldPane(
                    id,
                    exit_status,
                    is_first_run,
                    run_command,
                ));
            return None;
        }
        if self.floating_panes.panes_contain(&id) {
            self.floating_panes
                .hold_pane(id, exit_status, is_first_run, run_command);
        } else if let Some(pane) = self.overlay_panes.get_mut(&id) {
            pane.hold(exit_status, is_first_run, run_command);
        } else if self.tiled_panes.panes_contain(&id) {
            self.tiled_panes
                .hold_pane(id, exit_status, is_first_run, run_command);
        } else if let Some(pane) = self.suppressed_panes.get_mut(&id) {
            pane.pane.hold(exit_status, is_first_run, run_command);
        }
        if let PaneId::Terminal(terminal_pane_id) = id {
            self.pending_command_respawns
                .remove(&id)
                .map(|(run_command, keep_scrollback)| {
                    (terminal_pane_id, run_command, keep_scrollback)
                })
        } else {
            None
        }
    }
    /// Detach any respawn queued for `id` from this tab, so it can be carried to another tab
    /// the pane is about to move into. Only needed when a pane crosses tabs - a same-tab move
    /// (eg. a float/embed toggle) never extracts the pane, so the entry stays valid where it
    /// is.
    pub fn take_pending_respawn(&mut self, id: PaneId) -> Option<(RunCommand, bool)> {
        self.pending_command_respawns.remove(&id)
    }
    /// Re-home a respawn entry taken from another tab via `take_pending_respawn`, once the
    /// pane itself has landed here.
    pub fn restore_pending_respawn(&mut self, id: PaneId, entry: (RunCommand, bool)) {
        self.pending_command_respawns.insert(id, entry);
    }
    /// Mark a pane's root process stopped or running.
    ///
    /// Unlike `hold_pane` this is not buffered while the tab is pending: a pane
    /// that does not exist yet cannot have suspended, so there is nothing to
    /// replay.
    pub fn set_pane_suspended(&mut self, id: PaneId, suspended: bool) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(id) {
            pane.set_suspended(suspended);
        }
        self.emit_hint_text_if_changed()?;
        Ok(())
    }
    /// Swap the pane hidden behind `pane_id` back into the place `pane_id` was
    /// holding for it, and hand `pane_id` itself back.
    pub fn replace_pane_with_suppressed_pane(
        &mut self,
        pane_id: PaneId,
    ) -> Result<Option<Box<dyn Pane>>> {
        self.stands_in_for_a_pane(pane_id)
            .and_then(|hidden| self.suppressed_panes.remove(&hidden))
            .with_context(|| format!("no pane is suppressed behind pane {:?}", pane_id))
            .and_then(|suppressed| {
                let suppressed_pane = suppressed.pane;
                let suppressed_pane_id = suppressed_pane.pid();
                // Where the stand-in actually is, rather than whether floating
                // panes happen to be visible: the two differ whenever a tiled
                // stand-in is closed while a floating pane is open, and asking
                // the wrong collection to replace it fails *after* the hidden
                // pane has been taken out of the map - so it is dropped, and its
                // process left with nothing that could ever close it.
                let replaced_pane = if self.floating_panes.panes_contain(&pane_id) {
                    Some(self.floating_panes.replace_pane(pane_id, suppressed_pane)).transpose()?
                } else if self.overlay_panes.panes_contain(&pane_id) {
                    Some(self.overlay_panes.replace_pane(pane_id, suppressed_pane)).transpose()?
                } else {
                    self.tiled_panes.replace_pane(pane_id, suppressed_pane)
                };
                if replaced_pane.is_some() {
                    self.track_stack_list_member_id_swap(pane_id, suppressed_pane_id);
                }
                if let Some(suppressed_pane) = self
                    .floating_panes
                    .get_pane(suppressed_pane_id)
                    .or_else(|| self.overlay_panes.get_pane(suppressed_pane_id))
                    .or_else(|| self.tiled_panes.get_pane(suppressed_pane_id))
                {
                    // You may be thinking: why aren't we using the original "suppressed_pane" here,
                    // isn't it the same one?
                    //
                    // Yes, you are right! However, we moved it into its correct environment above
                    // (either floating_panes or tiled_panes) where it received a new geometry based on
                    // the pane there we replaced. Now, we need to update its pty about its new size.
                    // We couldn't do that before, and we can't use the original moved item now - so we
                    // need to refetch it
                    resize_pty!(
                        suppressed_pane,
                        self.os_api,
                        self.senders,
                        self.character_cell_size
                    )?;
                }
                Ok(replaced_pane)
            })
            .with_context(|| {
                format!(
                    "failed to replace active pane with suppressed pane {:?}",
                    pane_id
                )
            })
    }
    /// Tell the plugins and the render subscribers that `id` is gone.
    ///
    /// Every path that takes a pane out of the tab owes them this; the ones that swap a pane out
    /// rather than shrinking the layout are the ones that used to forget.
    pub(super) fn announce_pane_closed(&self, id: PaneId) {
        let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
            None,
            None,
            Event::PaneClosed(id),
        )]));
        let _ = self
            .senders
            .send_to_screen(ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id: id });
    }
    /// Explicit close destroys the replacement chain. Natural exits and `close_self`
    /// still use `close_pane`, which restores the pane behind an editor or exit view.
    ///
    /// The chain is followed in both directions, with one exception: a lent slot
    /// (`ReplacedPane::Lend`) is not followed from the stand-in back to the pane it borrowed
    /// the slot from. Closing that stand-in restores the lender, as any other close of it
    /// would, so a plugin's own pane survives the user closing a command it ran in its place.
    /// Closing the lender, or any pane behind it, still closes everything standing in front
    /// of it: closing the bottom of a stack is the way to close the whole stack.
    pub fn close_pane_explicitly(
        &mut self,
        id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Vec<PaneId> {
        if !self.has_pane_with_pid(&id) {
            return vec![];
        }
        let mut container = None;
        let mut closed = vec![id];
        let mut index = 0;
        while index < closed.len() {
            let current = closed[index];
            if let Some(tabs_id) = self.pane_tabs_of(current)
                && let Some(tabs) = self.pane_tabs.remove(&tabs_id)
            {
                for pane in tabs
                    .info
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.pane_id())
                    .chain([tabs.info.pane_id])
                {
                    if !closed.contains(&pane) {
                        closed.push(pane);
                    }
                    if let Some(child) = self.get_pane_with_id_mut(pane) {
                        child.set_pane_tabs(None);
                    }
                }
            }
            if let Some(owning_container) = self
                .multipane_info(current)
                .and_then(|_| self.overlay_panes.take_container(current))
            {
                for sibling in owning_container.pane_ids() {
                    if !closed.contains(&sibling) {
                        closed.push(sibling);
                    }
                }
                container = Some(owning_container);
                self.set_should_clear_display_before_rendering();
                self.set_force_render();
            }
            for (subject, suppressed) in &self.suppressed_panes {
                if let Some(stand_in) = suppressed.restored_by {
                    let related = if *subject == current {
                        Some(stand_in.pane_id)
                    } else if stand_in.pane_id == current && !stand_in.lent {
                        Some(*subject)
                    } else {
                        None
                    };
                    if let Some(related) = related
                        && !closed.contains(&related)
                        && self.has_pane_with_pid(&related)
                    {
                        closed.push(related);
                    }
                }
            }
            index += 1;
        }
        // Break restoration links before removing anything, including a suppressed stand-in.
        for id in &closed {
            if let Some(pane) = self.suppressed_panes.get_mut(id) {
                pane.restored_by = None;
            }
        }
        for closing in closed.iter().rev() {
            if container
                .as_ref()
                .is_some_and(|container| container.pane_ids().any(|pane| pane == *closing))
            {
                self.pending_command_respawns.remove(closing);
                self.announce_pane_closed(*closing);
            } else {
                self.close_pane(*closing, false, None);
            }
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(*closing, None))
                .with_context(|| format!("failed to close pane {closing:?}"))
                .non_fatal();
        }
        self.repair_focus_after_pane_removal();
        drop(completion_tx);
        closed
    }
    pub fn close_focused_pane(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<Vec<PaneId>> {
        Ok(match self.get_active_pane_id(client_id) {
            Some(id) => self.close_pane_explicitly(id, completion_tx),
            None => vec![],
        })
    }

    pub fn focus_suppressed_pane_for_all_clients(&mut self, pane_id: PaneId) {
        if self.make_hidden_stack_list_member_visible(pane_id) {
            self.tiled_panes.focus_pane_for_all_clients(pane_id);
            return;
        }
        match self.suppressed_panes.remove(&pane_id) {
            Some(suppressed) => {
                // as in `focus_pane_with_id`: a modal coming back out of the suppressed panes is
                // still a modal, and does not bring the floating layer up with it
                if !run_requests_overlay(suppressed.pane.invoked_with()) {
                    self.show_floating_panes();
                }
                // no focus call after this: `add_floating_pane` with no client focuses every
                // client itself, in whichever collection the pane actually landed in. Doing it
                // again here would name `floating_panes` unconditionally, and for an overlay that
                // is a collection the pane is not in - leaving a phantom in its `active_panes` and
                // `z_indices` that outlives the overlay.
                self.add_floating_pane(suppressed.pane, pane_id, None, true, None)
                    .non_fatal();
            },
            None => {
                log::error!("Could not find suppressed pane wiht id: {:?}", pane_id);
            },
        }
    }
    pub fn suppress_pane(&mut self, pane_id: PaneId, _client_id: Option<ClientId>) {
        let pane_id = self
            .pane_tabs_of(pane_id)
            .and_then(|id| self.pane_tabs_info(id))
            .map(|tabs| tabs.pane_id)
            .unwrap_or(pane_id);
        // this method places a pane in the suppressed pane with its own ID - this means we'll
        // not take it out of there when another pane is closed (eg. like happens with the
        // scrollback editor), but it has to take itself out on its own (eg. a plugin using the
        // show_self() method)
        if let Some(pane) = self.extract_pane(pane_id, true) {
            self.park_pane(pane);
        }
    }
    pub(super) fn make_hidden_stack_list_member_visible(&mut self, pane_id: PaneId) -> bool {
        self.swap_in_hidden_stack_list_member(pane_id, None)
    }
    /// Put a suppressed pane back in the layout, or leave it suppressed.
    ///
    /// The room check comes *before* the entry is taken out, for the reason
    /// `accept_extracted_pane` spells out: `add_tiled_pane` and
    /// `add_floating_pane` take the box by value and drop it when the tab has no
    /// room, reporting success. For a pane being created that is a failed
    /// insert; for a pane already in this tab it is destruction — the entry is
    /// gone, so nothing can ever close the pane or reap its process, and the
    /// only trace is a log line nobody reads.
    ///
    /// Staying suppressed is a state the pane is already in and can be asked out
    /// of again, so failing that way costs nothing.
    fn restore_suppressed_pane(&mut self, pane_id: PaneId, should_float_if_hidden: bool) -> bool {
        if !self.suppressed_panes.contains_key(&pane_id) {
            return false;
        }
        let has_room = if should_float_if_hidden {
            self.floating_panes.find_room_for_new_pane().is_some()
        } else {
            self.tiled_panes.has_room_for_new_pane()
        };
        if !has_room {
            log::error!(
                "No room to show pane {:?}, leaving it hidden rather than dropping it",
                pane_id
            );
            return true;
        }
        let Some(pane) = self
            .suppressed_panes
            .remove(&pane_id)
            .map(|suppressed| suppressed.pane)
        else {
            return false;
        };
        if should_float_if_hidden {
            self.add_floating_pane(pane, pane_id, None, true, None)
                .non_fatal();
        } else {
            self.add_tiled_pane(pane, pane_id, false, None).non_fatal();
        }
        true
    }
    pub fn unsuppress_pane(&mut self, pane_id: PaneId, should_float_if_hidden: bool) {
        // removes a pane from being suppressed (hidden) but does not focus it
        if self.make_hidden_stack_list_member_visible(pane_id) {
            return;
        }
        if !self.restore_suppressed_pane(pane_id, should_float_if_hidden) {
            log::error!("Could not find suppressed pane with id: {:?}", pane_id);
        }
    }
    pub fn unsuppress_or_expand_pane(&mut self, pane_id: PaneId, should_float_if_hidden: bool) {
        let pane_id = self
            .pane_tabs_of(pane_id)
            .and_then(|id| self.pane_tabs_info(id))
            .map(|tabs| tabs.pane_id)
            .unwrap_or(pane_id);
        // removes a pane from being suppressed (hidden) but does not focus it
        if self.make_hidden_stack_list_member_visible(pane_id) {
            return;
        }
        if !self.restore_suppressed_pane(pane_id, should_float_if_hidden) {
            let expand_panes_success = !self.tiled_panes.expand_pane_in_stack(pane_id).is_empty();
            if !expand_panes_success {
                log::error!(
                    "Could not find suppressed or stacked pane with id: {:?}",
                    pane_id
                );
            }
        }
    }
    /// Take a pane out of the layout, leaving `stand_in` holding its place.
    ///
    /// Closing the stand-in is what brings it back (`replace_pane_with_suppressed_pane`).
    /// The stand-in may itself be suppressed later without any of this needing to be
    /// revisited: an entry names the pane standing in for it, and that stays true
    /// wherever either of them ends up.
    pub(super) fn suppress_pane_behind(
        &mut self,
        pane: Box<dyn Pane>,
        stand_in: PaneId,
        is_scrollback_editor: bool,
        lent: bool,
    ) {
        self.suppressed_panes.insert(
            pane.pid(),
            SuppressedPane::behind(pane, stand_in, is_scrollback_editor, lent),
        );
    }
    /// Why nothing can open in `pane_id`'s place, for a pane that holds no slot: only a pane
    /// in the layout (`has_non_suppressed_pane_with_pid`) has one that a pane opened in its
    /// place can take, and asking a collection to replace any other pane drops the replacement.
    pub fn no_slot_error(&self, pane_id: PaneId) -> anyError {
        if self.suppressed_panes.contains_key(&pane_id) {
            anyhow!("pane {pane_id:?} is suppressed, so it has no slot to open a pane in place of")
        } else {
            anyhow!("pane {pane_id:?} is not in this tab")
        }
    }
    /// Take a pane out of the layout with nothing standing in for it.
    ///
    /// Nothing brings it back on its own - it has to be asked for by name (a plugin's
    /// `show_self`, selecting a stack-list member).
    pub fn park_pane(&mut self, mut pane: Box<dyn Pane>) {
        self.agent_tab_cleanup_allowed = false;
        self.retain_pane_tabs_transfer(&mut pane);
        self.suppressed_panes
            .insert(pane.pid(), SuppressedPane::parked(pane));
    }

    /// Place `pane` on the session surface as an overlay of `kind`.
    ///
    /// Unlike `add_floating_pane` this does no geometry: an overlay's geometry is entirely a
    /// function of its coordinates and the surface, and `OverlayPanes::add_pane` resolves it. It
    /// is also the reason there is no `find_room_for_new_pane` here - overlays do not tile
    /// themselves out of each other's way, they stack.
    ///
    /// `carry` is carried rather than derived: an overlay that follows a client to another tab is
    /// extracted as a bare `Box<dyn Pane>`, so everything the `OverlayPane` wrapper held - its
    /// kind, its coordinates, whether it was set aside and whether the user dragged it - has to be
    /// handed back in here, or the pane arrives on the new tab suddenly visible, or back at the
    /// position its plugin configured rather than the one the user dropped it at.
    /// `Tab::overlay_placement_for` and `Tab::overlay_carry_for` are what a mover reads before
    /// `extract_pane` drops the wrapper.
    pub fn add_overlay_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        pane_id: PaneId,
        placement: OverlayPlacement,
        carry: OverlayCarry,
        should_focus_new_pane: bool,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        self.agent_tab_cleanup_allowed = false;
        self.retain_pane_tabs_transfer(&mut pane);
        let err_context = || "failed to add overlay pane".to_string();
        placement.validate().map_err(|error| anyhow!(error))?;
        let coordinates = placement.coordinates.unwrap_or_default();
        if let Some(pinned) = coordinates.pinned {
            // an overlay is above every floating pane already, so pinning it means nothing - but
            // honour the flag rather than silently dropping what the caller asked for
            pane.set_pinned(pinned);
        }
        pane.set_active_at(Instant::now());
        self.overlay_panes
            .add_pane(pane_id, pane, placement.kind, coordinates, carry);
        if let Some(opacity) = placement.background_opacity {
            self.overlay_panes
                .set_background_opacity(pane_id, opacity)?;
        }
        resize_pty!(
            self.overlay_panes
                .get_mut(&pane_id)
                .with_context(err_context)?,
            self.os_api,
            self.senders,
            self.character_cell_size
        )
        .with_context(err_context)?;
        // a pane that arrived hidden is not on screen, so there is nothing to focus and no
        // `may_be_focused` that would allow it anyway
        if should_focus_new_pane && !carry.hidden {
            match client_id {
                Some(client_id) => self.overlay_panes.focus_pane(pane_id, client_id),
                None => self.overlay_panes.focus_pane_for_all_clients(pane_id),
            }
        }
        self.set_force_render();
        Ok(())
    }
    pub fn add_floating_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        pane_id: PaneId,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        should_focus_new_pane: bool,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        self.agent_tab_cleanup_allowed = false;
        self.retain_pane_tabs_transfer(&mut pane);
        let err_context = || "failed to add floating pane".to_string();
        // A pane that is an overlay by configuration is one wherever it came from, including the
        // paths that pass no coordinates at all (`focus_pane_with_id` taking a modal back out of
        // the suppressed panes) or their own (`pane_groups`). Placing it as a floating pane would
        // size it against the tab's viewport and raise the floating layer under it.
        if let Some(placement) =
            overlay_placement_for(&floating_pane_coordinates, pane.invoked_with())
        {
            let awaiting_placement = run_awaits_placement(pane.invoked_with());
            return self.add_overlay_pane(
                pane,
                pane_id,
                placement,
                // this arm re-derives overlay-ness for a pane arriving through the floating door;
                // nothing on that path carries a hidden or a dragged overlay, and a pane being
                // born there is waiting to be placed on the same terms as one born the other way
                OverlayCarry {
                    awaiting_placement,
                    ..Default::default()
                },
                should_focus_new_pane,
                client_id,
            );
        }
        if self.floating_panes.fullscreen_is_active() {
            self.floating_panes.unset_fullscreen();
        }
        if let Some(mut new_pane_geom) = self.floating_panes.find_room_for_new_pane() {
            if let Some(floating_pane_coordinates) = &floating_pane_coordinates {
                if let Some(pinned) = floating_pane_coordinates.pinned.as_ref() {
                    pane.set_pinned(*pinned);
                }
                self.floating_panes
                    .adjust_coordinates(&mut new_pane_geom, floating_pane_coordinates.clone());
                self.swap_layouts.set_is_floating_damaged();
            }
            pane.set_active_at(Instant::now());
            pane.set_geom(new_pane_geom);
            let is_borderless = floating_pane_coordinates
                .as_ref()
                .and_then(|c| c.borderless)
                .unwrap_or(false);
            if is_borderless {
                pane.set_content_offset(Offset::frame(0));
                pane.set_borderless(true);
            } else {
                pane.set_content_offset(Offset::frame(1)); // floating panes always have a frame
                pane.render_full_viewport(); // to make sure the frame is re-rendered
            }
            let resize_result =
                resize_pty!(pane, self.os_api, self.senders, self.character_cell_size);
            // Retain ownership even if the PTY writer has disconnected.
            self.floating_panes.add_pane(pane_id, pane);
            if should_focus_new_pane {
                // an overlay outranks the floating layer in `get_active_pane_id`, so unless a
                // dismissible one gives the keyboard up here the new pane opens *behind* the
                // popup, input keeps going to the popup, and the popup goes on reporting itself
                // focused rather than dismissing. A modal keeps focus - see
                // `defocus_pane_if_dismissible` - and the new pane comes up behind it, as it does
                // on the tiled path in `new_pane`.
                match client_id {
                    Some(client_id) => {
                        self.overlay_panes.defocus_pane_if_dismissible(client_id);
                        self.floating_panes.focus_pane(pane_id, client_id);
                    },
                    None => {
                        self.overlay_panes.defocus_dismissible_panes();
                        self.floating_panes.focus_pane_for_all_clients(pane_id);
                    },
                }
            }
            resize_result.with_context(err_context)?;
        }
        if self.auto_layout && !self.swap_layouts.is_floating_damaged() {
            // only do this if we're already in this layout, otherwise it might be
            // confusing and not what the user intends
            self.swap_layouts.set_is_floating_damaged(); // we do this so that we won't skip to the
                                                         // next layout
            self.relayout_floating_panes(false)?;
        }
        Ok(())
    }
    pub fn add_tiled_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        pane_id: PaneId,
        without_relayout: bool,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        self.agent_tab_cleanup_allowed = false;
        self.retain_pane_tabs_transfer(&mut pane);
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        let should_auto_layout =
            self.auto_layout && !self.swap_layouts.is_tiled_damaged() && !without_relayout;
        if self.is_pending && self.tiled_panes.get_panes().next().is_none() {
            // Later arrivals need usable geometry to split before the initial layout lands.
            pane.set_geom(PaneGeom::from(&*self.display_area.borrow()));
        }
        if self.tiled_panes.has_room_for_new_pane() {
            pane.set_active_at(Instant::now());
            let not_placed = if should_auto_layout {
                // no need to relayout here, we'll do it when reapplying the swap layout
                // below
                self.tiled_panes
                    .insert_pane_without_relayout(pane_id, pane, client_id)
            } else {
                self.tiled_panes.insert_pane(pane_id, pane, client_id)
            };
            // `has_room_for_new_pane` and the placement itself do not ask quite
            // the same question — the check counts room in a stack that the
            // placement may then fail to make — so the grid can still hand the
            // pane back. Parked rather than dropped: a pane nothing renders is
            // recoverable, a pane nothing *holds* is a process with no owner.
            if let Some(pane) = not_placed {
                log::error!("No room for pane {pane_id:?} in the tiled layout, hiding it instead");
                self.park_pane(pane);
                return Ok(());
            }
            if !self.is_pending {
                // if this tab is pending, the geometry of the panes inside it (including the one we
                // just added) is provisional - the real one will be assigned when the layout is
                // applied to this tab, and so we do not want to reapply the frames here because
                // doing so would send this transient size to the pane's pty, causing it to receive
                // two resizes in quick succession (this one and the real one) - which some
                // terminal applications (eg. vim) coalesce and thus miss the real one
                self.tiled_panes.reapply_pane_frames();
            }
            self.set_should_clear_display_before_rendering();
            if let Some(client_id) = client_id {
                self.tiled_panes.focus_pane(pane_id, client_id);
            }
        } else if self.is_pending {
            self.park_pane(pane);
        }
        // Wait for the initial layout before applying swap layouts.
        if should_auto_layout && !self.is_pending {
            // only do this if we're already in this layout, otherwise it might be
            // confusing and not what the user intends
            self.swap_layouts.set_is_tiled_damaged(); // we do this so that we won't skip to the
                                                      // next layout
            self.relayout_tiled_panes(false)?;
        }
        Ok(())
    }
    /// The complete destination geometry has already been checked by arrangement.
    /// Intermediate splits must not reject a batch that fits its final layout.
    pub fn accept_arranged_pane(
        &mut self,
        pane: Box<dyn Pane>,
        placement: ExtractedPanePlacement,
    ) -> AcceptedPane {
        self.agent_tab_cleanup_allowed = false;
        let id = pane.pid();
        match placement {
            ExtractedPanePlacement::Tiled => self.tiled_panes.add_pane_with_existing_geom(id, pane),
            ExtractedPanePlacement::Floating => self.floating_panes.add_pane(id, pane),
            placement => return self.accept_extracted_pane(pane, placement),
        }
        AcceptedPane::Accepted
    }

    /// Take a pane extracted from another tab, or hand it back if there is no room.
    ///
    /// `add_tiled_pane`/`add_floating_pane` take the box by value and return `Ok(())`
    /// having *dropped* it when the tab has no room. For a pane being created that is
    /// merely a failed insert; for a pane being **moved** it is destruction - the pane
    /// has already left its old tab, and dropping the box emits no `PaneClosed`, sends
    /// no `PtyInstruction::ClosePane` and registers no orphan. The pane simply ceases
    /// to exist while its child process leaks.
    ///
    /// So check for room *before* giving up ownership, and hand the pane back if there
    /// is none, letting the caller put it where it came from.
    /// The pane is deliberately not focused, in either arm: moving a pane into a tab
    /// should not steal focus from whatever the tab was already showing.
    pub fn accept_extracted_pane(
        &mut self,
        mut pane: Box<dyn Pane>,
        placement: ExtractedPanePlacement,
    ) -> AcceptedPane {
        let pane_id = pane.pid();
        match placement {
            ExtractedPanePlacement::Overlay(overlay_placement, carry) => {
                // no room check: an overlay is not on a layer and does not tile, so there is nothing
                // for it to run out of. That is also why this arm cannot return `NoRoom` - only
                // `Accepted` or, if the pty thread is already gone, `LostInAdd`.
                //
                // No geom either: `OverlayPanes::add_pane` resolves the geometry from the
                // coordinates and the destination tab's surface, which is the point of carrying the
                // placement rather than the pane's absolute position across.
                self.add_overlay_pane(pane, pane_id, overlay_placement, carry, false, None)
                    .non_fatal();
            },
            ExtractedPanePlacement::Floating | ExtractedPanePlacement::FloatingDefault { .. } => {
                let Some(geometry) = self.floating_panes.find_room_for_new_pane() else {
                    return AcceptedPane::NoRoom(pane);
                };
                if matches!(placement, ExtractedPanePlacement::FloatingDefault { min_height }
                    if geometry.rows.as_usize() < min_height)
                {
                    return AcceptedPane::NoRoom(pane);
                }
                let keep_geometry = matches!(placement, ExtractedPanePlacement::Floating);
                let floating_pane_coordinates = FloatingPaneCoordinates {
                    x: keep_geometry.then(|| PercentOrFixed::Fixed(pane.x())),
                    y: keep_geometry.then(|| PercentOrFixed::Fixed(pane.y())),
                    width: keep_geometry.then(|| PercentOrFixed::Fixed(pane.cols())),
                    height: keep_geometry.then(|| PercentOrFixed::Fixed(pane.rows())),
                    pinned: Some(pane.current_geom().is_pinned),
                    borderless: Some(pane.borderless()),
                };
                // non_fatal rather than `?`: the only error here comes from `relayout`, which
                // runs after the pane is already in the tab, and the caller must not abort a
                // multi-pane move part-way through
                self.add_floating_pane(pane, pane_id, Some(floating_pane_coordinates), false, None)
                    .non_fatal();
            },
            ExtractedPanePlacement::Tiled => {
                if !self.tiled_panes.has_room_for_new_pane() {
                    return AcceptedPane::NoRoom(pane);
                }
                let new_geom = PaneGeom::from(&self.size);
                pane.set_geom(new_geom);
                self.add_tiled_pane(pane, pane_id, false, None).non_fatal();
            },
        }
        if self.has_pane_with_pid(&pane_id) {
            AcceptedPane::Accepted
        } else {
            // A failed insertion consumed the box, so it cannot be handed back.
            // Say so rather than claim the pane moved.
            AcceptedPane::LostInAdd
        }
    }
    pub fn add_stacked_pane_to_pane_id(
        &mut self,
        pane: Box<dyn Pane>,
        pane_id: PaneId,
        root_pane_id: PaneId,
        should_focus: bool,
    ) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        let not_placed = self
            .tiled_panes
            .add_pane_to_stack_of_pane_id(pane_id, pane, root_pane_id);
        self.set_should_clear_display_before_rendering();
        // Groupify BEFORE the focus/frame re-application so the just-collapsed
        // in-grid stack member is moved to `suppressed_panes` before
        // `reapply_pane_frames` sends a `resize_pty!` at the 1-row collapsed
        // size. Without this, the shell inside that pane sees two rapid
        // WINCH events (1 row → visible-size rows) and redraws its prompt
        // twice, blanking out the visible rows of previous output.
        self.sync_stacked_pane_list_mode();
        let placed = match not_placed {
            Some(pane) => {
                // Parked rather than dropped, as `add_tiled_pane` parks a pane the grid
                // hands back: a pane nothing renders is recoverable, a pane nothing holds
                // is a process with no owner. `park_new_pane`, because this one still has
                // the geometry `build_pane` gave it.
                log::error!(
                    "No room in the stack of {root_pane_id:?} for pane {pane_id:?}, hiding it instead"
                );
                self.park_new_pane(pane)
            },
            None => {
                if should_focus {
                    self.tiled_panes.expand_pane_in_stack(pane_id);
                }
                Ok(())
            },
        };
        self.swap_layouts.set_is_tiled_damaged();
        placed
    }
    pub fn add_stacked_pane_to_active_pane(
        &mut self,
        pane: Box<dyn Pane>,
        pane_id: PaneId,
        client_id: ClientId,
        should_focus: bool,
    ) -> Result<()> {
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        let not_placed = self
            .tiled_panes
            .add_pane_to_stack_of_active_pane(pane_id, pane, client_id);
        // See comment in `add_stacked_pane_to_pane_id` — groupify before
        // focusing so the collapsed member is suppressed before
        // `reapply_pane_frames` fires its intermediate `resize_pty!`.
        self.sync_stacked_pane_list_mode();
        let placed = match not_placed {
            Some(pane) => {
                // Parked rather than dropped; see `add_stacked_pane_to_pane_id`.
                log::error!(
                    "No room in the stack of client {client_id}'s active pane for pane {pane_id:?}, hiding it instead"
                );
                self.park_new_pane(pane)
            },
            None => {
                if should_focus {
                    self.tiled_panes.focus_pane(pane_id, client_id);
                }
                Ok(())
            },
        };
        self.swap_layouts.set_is_tiled_damaged();
        placed
    }
    /// Returns whether the pane accepted a command for PTY respawn.
    pub fn rerun_terminal_pane_with_id(
        &mut self,
        terminal_pane_id: TerminalId,
        completion_tx: Option<NotificationEnd>,
    ) -> bool {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        match self.get_pane_with_id_mut(pane_id) {
            Some(pane_to_rerun) => {
                if let Some(command_to_rerun) = pane_to_rerun.rerun() {
                    self.pids_waiting_resize.insert(terminal_pane_id);
                    let _ = self.senders.send_to_pty(PtyInstruction::ReRunCommandInPane(
                        pane_id,
                        command_to_rerun,
                        completion_tx,
                    ));
                    return true;
                } else {
                    log::error!("Pane is still running!")
                }
            },
            None => {
                log::error!(
                    "Failed to find terminal pane with id {} to rerun in tab",
                    terminal_pane_id
                );
            },
        }
        false
    }
    /// Run a *different* command in an existing command pane, reusing its pane id. When the
    /// pane's current command is still running — which includes the window between the
    /// command's exit event reaching its originating plugin and the screen processing the
    /// pane's hold — the replacement is stashed and started by `hold_pane` (last write wins).
    /// Returns true only when the pane accepted a command for PTY respawn.
    pub fn run_command_in_pane_with_id(
        &mut self,
        terminal_pane_id: TerminalId,
        run_command: RunCommand,
        keep_scrollback: bool,
    ) -> bool {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        match self.get_pane_with_id_mut(pane_id) {
            Some(pane) => {
                if let Some(command_to_run) =
                    pane.respawn(Some(run_command.clone()), keep_scrollback)
                {
                    self.pending_command_respawns.remove(&pane_id);
                    self.pids_waiting_resize.insert(terminal_pane_id);
                    let _ = self.senders.send_to_pty(PtyInstruction::ReRunCommandInPane(
                        pane_id,
                        command_to_run,
                        None,
                    ));
                    return true;
                } else {
                    self.pending_command_respawns
                        .insert(pane_id, (run_command, keep_scrollback));
                }
            },
            None => {
                log::error!(
                    "Failed to find terminal pane with id {} to run a command in",
                    terminal_pane_id
                );
            },
        }
        false
    }

    pub fn extract_suppressed_panes(&mut self) -> SuppressedPanes {
        let stack_list_members = &self.stack_list_of_member;
        self.suppressed_panes
            .extract_if(|key, _| !stack_list_members.contains_key(key))
            .collect()
    }
    pub fn add_suppressed_panes(&mut self, mut suppressed_panes: SuppressedPanes) {
        for (pane_id, suppressed_pane) in suppressed_panes.drain() {
            self.suppressed_panes.insert(pane_id, suppressed_pane);
        }
    }
    /// Take this tab's dismissible overlays out of it, ready to be given to another tab.
    ///
    /// An overlay is drawn over the whole session surface rather than inside this tab's
    /// viewport, and agent - the one thing that lives in one for a whole session - holds state no
    /// other pane does. So a tab going away is not a reason for one to be unloaded: `close_tab_by_id`
    /// takes them out here and re-homes them, exactly as it already does for suppressed panes.
    ///
    /// **Modals are deliberately left behind.** A modal is a dialog *about* the tab that is going
    /// away, and one re-homed unfocused is an input deadlock: `OverlayPanes::may_be_focused`
    /// refuses every other overlay the keyboard while a modal is in `stack_layer_ids`, and a modal
    /// has no channel to learn that it lost focus. It closes with the tab.
    ///
    /// The placement is read *before* the extraction, which is what destroys the wrapper holding
    /// an overlay's kind and coordinates - see [`Tab::placement_for_extraction`].
    pub fn extract_overlay_panes(&mut self) -> Vec<OverlayTransfer> {
        let ids: Vec<_> = self
            .overlay_panes
            .pane_ids()
            .filter(|id| !self.overlay_panes.is_modal(id))
            .copied()
            .collect();
        ids.into_iter()
            .filter_map(|id| self.take_overlay_container(id))
            .collect()
    }
}
