use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::{BreakPanesToTabOutcome, Screen};
use crate::background_jobs::BackgroundJob;
use crate::panes::{OverlayCarry, PaneId};
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::tab::{AcceptedPane, ExtractedPanePlacement, Pane};
use crate::ClientId;
use yinz_types::data::Event;
use yinz_types::geometry::Direction;
use yinz_types::input::layout::Layout;
use yinz_types::pane_size::PaneGeom;
use {yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction};

/// A pane taken out of its tab on the way to another: where it sat, the pane itself, and the
/// respawn it had pending, carried along so the move loses nothing.
type ExtractedPane = (
    ExtractedPanePlacement,
    Box<dyn Pane>,
    Option<(RunCommand, bool)>,
);

/// An `ExtractedPane` that remembers the tab it came from, so a destination that refuses it
/// can hand it back.
pub(super) type ExtractedPaneWithSource = (
    TabId,
    ExtractedPanePlacement,
    Box<dyn Pane>,
    Option<(RunCommand, bool)>,
);

impl Screen {
    pub(super) fn extract_pane_for_transfer(
        &mut self,
        source: TabId,
        pane_id: PaneId,
    ) -> Option<ExtractedPaneWithSource> {
        let tab = self.tabs.get_mut(&source)?;
        let original = tab.placement_for_extraction(pane_id);
        let respawn = tab.take_pending_respawn(pane_id);
        if let Some(pane) = tab.extract_pane(pane_id, true) {
            Some((source, original, pane, respawn))
        } else {
            if let Some(entry) = respawn {
                tab.restore_pending_respawn(pane_id, entry);
            }
            None
        }
    }

    pub(super) fn accept_transferred_panes(
        &mut self,
        extracted: Vec<(ExtractedPaneWithSource, ExtractedPanePlacement)>,
        tab_id: TabId,
        arrangement: bool,
    ) -> BreakPanesToTabOutcome {
        let mut outcome = BreakPanesToTabOutcome::default();
        let mut refused: Vec<ExtractedPaneWithSource> = vec![];
        match self.get_tab_by_id_mut(tab_id) {
            Some(destination) => {
                for ((source_tab_id, original, pane, carried_respawn), placement) in extracted {
                    let pane_id = pane.pid();
                    let accepted = if arrangement {
                        destination.accept_arranged_pane(pane, placement)
                    } else {
                        destination.accept_extracted_pane(pane, placement)
                    };
                    match accepted {
                        AcceptedPane::Accepted => {
                            outcome.moved.push(pane_id);
                            if let Some(entry) = carried_respawn {
                                destination.restore_pending_respawn(pane_id, entry);
                            }
                        },
                        AcceptedPane::NoRoom(pane) => {
                            refused.push((source_tab_id, original, pane, carried_respawn))
                        },
                        AcceptedPane::LostInAdd => outcome.closed.push(pane_id),
                    }
                }
            },
            // Every extracted pane needs a home even if its destination disappeared.
            None => {
                log::error!("Tab {tab_id} vanished mid-move");
                refused = extracted.into_iter().map(|(pane, _)| pane).collect();
            },
        }

        // put back anything the destination would not take - as what it was, so a refusal does
        // not quietly demote a modal in the tab it never left
        for (source_tab_id, placement, pane, carried_respawn) in refused {
            let pane_id = pane.pid();
            let put_back = match self.get_tab_by_id_mut(source_tab_id) {
                Some(source) => source.accept_extracted_pane(pane, placement),
                None => AcceptedPane::NoRoom(pane),
            };
            match put_back {
                AcceptedPane::Accepted => {
                    log::error!(
                        "Tab {tab_id} had no room for pane {pane_id:?}; returned it to tab {source_tab_id}"
                    );
                    outcome.returned_to_source.push(pane_id);
                    if let Some(entry) = carried_respawn {
                        if let Some(source) = self.get_tab_by_id_mut(source_tab_id) {
                            source.restore_pending_respawn(pane_id, entry);
                        }
                    }
                },
                AcceptedPane::NoRoom(..) | AcceptedPane::LostInAdd => {
                    // Last resort: the pane fits nowhere. Dropping the box here is only
                    // safe because `close_homeless_pane` below tears the pane down
                    // first - the same order `close_tab_by_id` uses for the panes of a
                    // tab it removes. Dropping it *without* that is the bug this
                    // function exists to fix. A pending respawn is dropped along with it,
                    // matching `Tab::close_pane`'s own cleanup for a pane that closes
                    // outright.
                    log::error!(
                        "Pane {pane_id:?} fits in neither tab {tab_id} nor tab {source_tab_id}; closing it"
                    );
                    outcome.closed.push(pane_id);
                },
            }
        }
        for pane_id in outcome.closed.clone() {
            self.close_homeless_pane(pane_id);
        }

        outcome
    }

    pub fn break_pane(
        &mut self,
        default_shell: Option<TerminalAction>,
        default_layout: Box<Layout>,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || "failed break pane out of tab".to_string();
        let active_tab = self.get_active_tab_mut(client_id)?;
        let active_pane_id = active_tab
            .get_active_pane_id(client_id)
            .with_context(err_context)?;
        if active_tab.pane_is_overlay(active_pane_id) {
            let tab_id = self.break_multiple_panes_to_new_tab(
                vec![active_pane_id],
                default_shell,
                true,
                None,
                client_id,
            )?;
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                tab.focus_pane_with_id(active_pane_id, false, false, client_id)?;
            }
            return Ok(());
        }
        // read before the extraction, which forgets that this was an overlay
        let placement = active_tab.placement_for_extraction(active_pane_id);
        let pane_is_overlay = matches!(placement, ExtractedPanePlacement::Overlay(..));
        // an overlay is not on a layer and does not tile, so taking it out cannot leave this tab
        // without a pane - how many panes sit underneath it has no say in whether it may leave
        if pane_is_overlay
            || active_tab.get_selectable_tiled_panes_count() > 1
            || active_tab.get_visible_selectable_floating_panes_count() > 0
            || active_tab.pane_is_stack_list_member(&active_pane_id)
        {
            let carried_respawn = active_tab.take_pending_respawn(active_pane_id);
            let active_pane = active_tab
                .extract_pane(active_pane_id, false)
                .with_context(err_context)?;
            let active_pane_run_instruction = active_pane.invoked_with().clone();
            let tab_index = self.get_new_tab_id();
            let swap_layouts = (
                default_layout.swap_tiled_layouts.clone(),
                default_layout.swap_floating_layouts.clone(),
            );
            self.new_tab(tab_index, swap_layouts, None, Some(client_id))?;
            let tab = self.tabs.get_mut(&tab_index).with_context(err_context)?;
            let (mut tiled_panes_layout, floating_panes_layout) = default_layout.new_tab();
            let without_relayout = true;
            match placement {
                ExtractedPanePlacement::Overlay(overlay_placement, carry) => {
                    // no geom: an overlay's geometry is resolved from its coordinates against the
                    // new tab's surface, which is exactly what carrying the placement is for.
                    //
                    // The run instruction is deliberately *not* ignored here: an overlay does not
                    // tile, so ignoring it would leave the new tab with nothing underneath the
                    // overlay at all.
                    tab.add_overlay_pane(
                        active_pane,
                        active_pane_id,
                        overlay_placement,
                        // this breaks out the *active* pane, and `hide_pane` moves clients off
                        // what it hides - so the pane reaching here cannot be a hidden one
                        OverlayCarry {
                            hidden: false,
                            ..carry
                        },
                        true,
                        Some(client_id),
                    )?;
                },
                _ => {
                    tab.add_tiled_pane(
                        active_pane,
                        active_pane_id,
                        without_relayout,
                        Some(client_id),
                    )?;
                    tiled_panes_layout.ignore_run_instruction(active_pane_run_instruction.clone());
                },
            }
            if let Some(entry) = carried_respawn {
                if let Some(tab) = self.tabs.get_mut(&tab_index) {
                    tab.restore_pending_respawn(active_pane_id, entry);
                }
            }
            let should_change_focus_to_new_tab = true;
            self.bus.senders.send_to_plugin(PluginInstruction::NewTab(
                None,
                default_shell,
                Some(tiled_panes_layout),
                (!floating_panes_layout.is_empty()).then_some(floating_panes_layout),
                tab_index,
                None,  // initial_panes
                false, // block_on_first_terminal
                should_change_focus_to_new_tab,
                client_id,
                None,
            ))?;
        } else {
            self.bus
                .senders
                .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                    vec![active_pane_id],
                    "Cannot break single pane out!".into(),
                ))
                .with_context(err_context)?;
        }
        Ok(())
    }
    pub fn break_multiple_panes_to_new_tab(
        &mut self,
        pane_ids: Vec<PaneId>,
        default_shell: Option<TerminalAction>,
        should_change_focus_to_new_tab: bool,
        new_tab_name: Option<String>,
        client_id: ClientId,
    ) -> Result<TabId> {
        let err_context = || "failed break multiple panes to a new tab".to_string();

        let mut containers = vec![];
        let all_tabs = self.get_tabs_mut();
        let mut extracted_panes: Vec<ExtractedPane> = vec![];
        for pane_id in pane_ids {
            for tab in all_tabs.values_mut() {
                if !tab.has_pane_with_pid(&pane_id) {
                    continue;
                }
                if let Some(container) = tab.take_overlay_container(pane_id) {
                    containers.push(container);
                    break;
                }
                let placement = tab.placement_for_extraction(pane_id);
                let carried_respawn = tab.take_pending_respawn(pane_id);
                // here we pass None instead of the client_id we have because we do not need to
                // necessarily trigger a relayout for this tab
                if let Some(pane) = tab.extract_pane(pane_id, true) {
                    extracted_panes.push((placement, pane, carried_respawn));
                    break;
                } else if let Some(entry) = carried_respawn {
                    tab.restore_pending_respawn(pane_id, entry);
                }
            }
        }

        let (mut tiled_panes_layout, floating_panes_layout) = self.default_layout.new_tab();
        let tab_index = self.get_new_tab_id();
        let swap_layouts = (
            self.default_layout.swap_tiled_layouts.clone(),
            self.default_layout.swap_floating_layouts.clone(),
        );
        if should_change_focus_to_new_tab {
            self.new_tab(tab_index, swap_layouts, None, Some(client_id))?;
        } else {
            self.new_tab(tab_index, swap_layouts, None, None)?;
            self.assign_tab_group(tab_index, Some(client_id));
        }
        let tab = self.tabs.get_mut(&tab_index).with_context(err_context)?;
        if let Some(new_tab_name) = new_tab_name {
            tab.name = new_tab_name.clone();
        }
        let tab_size = tab.size;
        for container in containers {
            tab.accept_overlay_container(container);
        }
        for (placement, mut pane, carried_respawn) in extracted_panes {
            let run_instruction = pane.invoked_with().clone();
            let pane_id = pane.pid();
            let without_relayout = true;

            if let ExtractedPanePlacement::Overlay(overlay_placement, carry) = placement {
                // no geom: an overlay's geometry is resolved from its coordinates against the
                // new tab's surface, which is exactly what carrying the placement is for. Not
                // focused, for the same reason the tiled arm below is not.
                tab.add_overlay_pane(pane, pane_id, overlay_placement, carry, false, None)?;
            } else {
                let new_geom = PaneGeom::from(&tab_size);
                pane.set_geom(new_geom);

                // here we pass None instead of the ClientId, because we do not want this pane to be
                // necessarily focused
                tab.add_tiled_pane(pane, pane_id, without_relayout, None)?;
            }
            if let Some(entry) = carried_respawn {
                tab.restore_pending_respawn(pane_id, entry);
            }
            tiled_panes_layout.ignore_run_instruction(run_instruction.clone());
        }
        self.bus.senders.send_to_plugin(PluginInstruction::NewTab(
            None,
            default_shell,
            Some(tiled_panes_layout),
            (!floating_panes_layout.is_empty()).then_some(floating_panes_layout),
            tab_index,
            None,  // initial_panes
            false, // block_on_first_terminal
            should_change_focus_to_new_tab,
            client_id,
            None,
        ))?;
        Ok(tab_index)
    }
    pub fn break_pane_to_new_tab(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || "failed break pane out of tab".to_string();
        if self
            .group_tabs(self.group_for_client(Some(client_id)))
            .len()
            > 1
        {
            let tab = self.get_active_tab(client_id)?;
            let destination_position = self
                .adjacent_group_tab(
                    tab.id,
                    matches!(direction, Direction::Right | Direction::Down),
                )
                .context("no adjacent tab in group")?;
            let active_tab = self.get_active_tab_mut(client_id)?;
            let active = active_tab
                .get_active_pane_id(client_id)
                .context("no active pane")?;
            if active_tab.pane_is_overlay(active) {
                self.switch_active_tab(destination_position, None, true, client_id)?;
                let destination = self.get_active_tab_mut(client_id)?.id;
                self.break_multiple_panes_to_tab_with_id(
                    vec![active],
                    destination,
                    false,
                    client_id,
                )?;
                self.get_active_tab_mut(client_id)?
                    .focus_pane_with_id(active, false, false, client_id)?;
                return Ok(());
            }
            let (
                active_pane_id,
                active_pane,
                pane_to_break_is_floating,
                overlay_placement,
                overlay_carry,
                carried_respawn,
            ) = {
                let active_tab = self.get_active_tab_mut(client_id)?;
                let active_pane_id = active_tab
                    .get_active_pane_id(client_id)
                    .with_context(err_context)?;
                let pane_to_break_is_floating = active_tab.are_floating_panes_visible();
                // the active pane *is* the overlay whenever one is up, and the extraction below
                // is what forgets that - so ask first, for the placement and for the in-memory
                // half that goes with it
                let overlay_placement = active_tab.overlay_placement_for(active_pane_id);
                let overlay_carry = active_tab.overlay_carry_for(active_pane_id);
                let carried_respawn = active_tab.take_pending_respawn(active_pane_id);
                let active_pane = active_tab
                    .extract_pane(active_pane_id, false)
                    .with_context(err_context)?;
                (
                    active_pane_id,
                    active_pane,
                    pane_to_break_is_floating,
                    overlay_placement,
                    overlay_carry,
                    carried_respawn,
                )
            };
            // Moving an existing pane supplies content itself; do not recover parked work.
            self.switch_active_tab(destination_position, None, true, client_id)?;
            let new_active_tab = self.get_active_tab_mut(client_id)?;

            if let Some(placement) = overlay_placement {
                // an overlay is not on the floating layer, so arriving here must not raise it
                new_active_tab.add_overlay_pane(
                    active_pane,
                    active_pane_id,
                    placement,
                    // this breaks out the *active* pane, and `hide_pane` moves clients off what
                    // it hides - so the pane reaching here cannot be a hidden one. Where the user
                    // dragged it still travels: it is the same pane on the far side.
                    OverlayCarry {
                        hidden: false,
                        ..overlay_carry
                    },
                    true,
                    Some(client_id),
                )?;
            } else if pane_to_break_is_floating {
                new_active_tab.show_floating_panes();
                new_active_tab.add_floating_pane(
                    active_pane,
                    active_pane_id,
                    None,
                    true,
                    Some(client_id),
                )?;
            } else {
                new_active_tab.hide_floating_panes();
                new_active_tab.add_tiled_pane(
                    active_pane,
                    active_pane_id,
                    false,
                    Some(client_id),
                )?;
            }
            if let Some(entry) = carried_respawn {
                new_active_tab.restore_pending_respawn(active_pane_id, entry);
            }

            self.log_and_report_session_state()?;
        } else {
            let active_pane_id = {
                let active_tab = self.get_active_tab_mut(client_id)?;
                active_tab
                    .get_active_pane_id(client_id)
                    .with_context(err_context)?
            };
            self.bus
                .senders
                .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                    vec![active_pane_id],
                    "No other tabs to add pane to!".into(),
                ))
                .with_context(err_context)?;
        }
        self.render()?;
        Ok(())
    }
    /// Tear down a pane that belongs to no tab.
    ///
    /// Reached from [`Screen::break_multiple_panes_to_tab_with_id`], when a pane
    /// extracted for a move fits in neither the destination nor the tab it came from,
    /// and from [`Screen::move_overlay_panes_from_closed_tab`], for an overlay the tab
    /// it was re-homed into would not take. The alternative is dropping the
    /// `Box<dyn Pane>`, which destroys the pane while leaving its child process running
    /// with nothing attached to it - the failure mode this whole path exists to prevent.
    /// Mirrors the teardown `close_tab_by_id` does for the panes of a tab it removes.
    pub(super) fn close_homeless_pane(&mut self, pane_id: PaneId) {
        self.cleanup_closed_pane(pane_id);
        let _ = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::PaneClosed(pane_id),
            )]));
        self.notify_pane_closed_to_subscribers(pane_id);
        // not checking the result: this may be happening while the app is closing, at
        // which point the pty thread is already gone
        let _ = self
            .bus
            .senders
            .send_to_pty(PtyInstruction::ClosePane(pane_id, None));
    }
    /// Move the overlay container holding `pane_id` into `destination`, as the overlay it is,
    /// and return every pane that went with it. `None` when the pane is not an overlay in
    /// another tab. The destination is checked first: a container taken with nowhere to put it
    /// would be dropped, panes and all.
    pub(super) fn move_overlay_container(
        &mut self,
        pane_id: PaneId,
        destination: TabId,
    ) -> Option<Vec<PaneId>> {
        let source = self.tab_containing_overlay(pane_id)?.id;
        if source == destination || !self.tabs.contains_key(&destination) {
            return None;
        }
        let container = self
            .tabs
            .get_mut(&source)?
            .take_overlay_container(pane_id)?;
        let members = container.pane_ids().collect();
        // Present: checked above, and nothing since has removed a tab.
        self.tabs
            .get_mut(&destination)?
            .accept_overlay_container(container);
        Some(members)
    }
    /// Move `pane_ids` into the tab with the given **stable id**.
    ///
    /// `tab_id` is an ID - the `Screen::tabs` BTreeMap key - and never a display
    /// position. Callers holding a position must convert first with
    /// [`Screen::get_tab_id_at_position`]; see the `BreakPanesToTabWithIndex` handler,
    /// and `CloseTabWithIndex` for the same pattern. The module header documents the
    /// id/position/index convention.
    ///
    /// The ordering here is load-bearing. The destination is confirmed to exist
    /// *before* anything is extracted, and any pane the destination refuses is put back
    /// in the tab it came from. Dropping a `Box<dyn Pane>` destroys the pane with no
    /// `PaneClosed` event, no `PtyInstruction::ClosePane` and no orphan registration:
    /// the pane ceases to exist while its child process leaks. So **no `?` and no early
    /// return may appear between the first `extract_pane` and the last re-home**.
    pub fn break_multiple_panes_to_tab_with_id(
        &mut self,
        pane_ids: Vec<PaneId>,
        tab_id: TabId,
        should_change_focus_to_target_tab: bool,
        client_id: ClientId,
    ) -> Result<BreakPanesToTabOutcome> {
        self.move_project_or_existing_panes(
            pane_ids,
            tab_id,
            should_change_focus_to_target_tab,
            client_id,
            false,
        )
    }

    pub(super) fn move_project_or_existing_panes(
        &mut self,
        pane_ids: Vec<PaneId>,
        tab_id: TabId,
        should_change_focus_to_target_tab: bool,
        client_id: ClientId,
        arrangement: bool,
    ) -> Result<BreakPanesToTabOutcome> {
        let err_context = || format!("failed to move panes to tab with id {tab_id}");
        let mut outcome = BreakPanesToTabOutcome::default();

        // the destination first: if it does not exist, nothing is touched
        let Some(destination) = self.get_tab_by_id(tab_id) else {
            log::error!("Cannot find tab with id: {tab_id}");
            return Ok(outcome);
        };
        outcome.destination_tab_id = Some(tab_id);

        // panes already in the destination are left exactly where they are
        let mut to_extract = Vec::with_capacity(pane_ids.len());
        for pane_id in pane_ids {
            if destination.has_pane_with_pid(&pane_id) {
                outcome.already_in_destination.push(pane_id);
            } else {
                to_extract.push(pane_id);
            }
        }

        let mut moved_members = std::collections::HashSet::new();
        for pane_id in &to_extract {
            if moved_members.contains(pane_id) {
                continue;
            }
            if let Some(members) = self.move_overlay_container(*pane_id, tab_id) {
                moved_members.extend(members);
            }
        }
        outcome.moved.extend(moved_members.iter().copied());
        to_extract.retain(|id| !moved_members.contains(id));

        // remember where each pane came from, so that a destination that refuses it can
        // be recovered from rather than losing the pane
        let mut extracted = Vec::with_capacity(to_extract.len());
        for pane_id in to_extract {
            let source = self
                .tabs
                .values()
                .find(|tab| tab.id != tab_id && tab.has_pane_with_pid(&pane_id));
            let Some(source) = source else {
                outcome.not_found.push(pane_id);
                continue;
            };
            let destination_placement = if arrangement && source.pane_is_stack_list_member(&pane_id)
            {
                ExtractedPanePlacement::Tiled
            } else {
                source.placement_for_extraction(pane_id)
            };
            let source = source.id;
            if let Some(pane) = self.extract_pane_for_transfer(source, pane_id) {
                extracted.push((pane, destination_placement));
            } else {
                outcome.not_found.push(pane_id);
            }
        }

        let transferred = self.accept_transferred_panes(extracted, tab_id, arrangement);
        outcome.moved.extend(transferred.moved);
        outcome.returned_to_source = transferred.returned_to_source;
        outcome.closed = transferred.closed;

        // focus last, so the destination renders with the panes already in it
        if should_change_focus_to_target_tab {
            if let Some(position) = self.get_tab_position_by_id(tab_id) {
                self.switch_active_tab(position, None, true, client_id)
                    .with_context(err_context)?;
            }
        }
        self.log_and_report_session_state()?;
        Ok(outcome)
    }
}
