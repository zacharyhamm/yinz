use zellij_runtime::errors::prelude::*;
use zellij_types::internal_ids::TabId;

use super::{BreakPanesToTabOutcome, Screen};
use crate::background_jobs::BackgroundJob;
use crate::panes::{OverlayCarry, PaneId};
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::tab::{AcceptedPane, ExtractedPanePlacement, Pane};
use crate::ClientId;
use zellij_types::data::Event;
use zellij_types::geometry::Direction;
use zellij_types::input::layout::Layout;
use zellij_types::pane_size::PaneGeom;
use {zellij_types::input::command::RunCommand, zellij_types::input::command::TerminalAction};

impl Screen {
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
                        overlay_placement.kind,
                        overlay_placement.coordinates.unwrap_or_default(),
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
                floating_panes_layout,
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

        let all_tabs = self.get_tabs_mut();
        let mut extracted_panes: Vec<(
            ExtractedPanePlacement,
            Box<dyn Pane>,
            Option<(RunCommand, bool)>,
        )> = vec![];
        for pane_id in pane_ids {
            for tab in all_tabs.values_mut() {
                if !tab.has_pane_with_pid(&pane_id) {
                    continue;
                }
                // read before the extraction, which forgets that this was an overlay
                let placement = tab.placement_for_extraction(pane_id);
                let carried_respawn = tab.take_pending_respawn(pane_id);
                // here we pass None instead of the client_id we have because we do not need to
                // necessarily trigger a relayout for this tab
                if let Some(pane) = tab.extract_pane(pane_id, true).take() {
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
        }
        let tab = self.tabs.get_mut(&tab_index).with_context(err_context)?;
        if let Some(new_tab_name) = new_tab_name {
            tab.name = new_tab_name.clone();
        }
        let tab_size = tab.size;
        for (placement, mut pane, carried_respawn) in extracted_panes {
            let run_instruction = pane.invoked_with().clone();
            let pane_id = pane.pid();
            let without_relayout = true;

            if let ExtractedPanePlacement::Overlay(overlay_placement, carry) = placement {
                // no geom: an overlay's geometry is resolved from its coordinates against the
                // new tab's surface, which is exactly what carrying the placement is for. Not
                // focused, for the same reason the tiled arm below is not.
                tab.add_overlay_pane(
                    pane,
                    pane_id,
                    overlay_placement.kind,
                    overlay_placement.coordinates.unwrap_or_default(),
                    carry,
                    false,
                    None,
                )?;
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
            floating_panes_layout,
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
        if self.tabs.len() > 1 {
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
            // the plugins in the tab the pane lands in need the client's mode, not just
            // the pane
            let update_mode_infos = true;
            match direction {
                Direction::Right | Direction::Down => {
                    self.switch_tab_next(None, update_mode_infos, client_id)?;
                },
                Direction::Left | Direction::Up => {
                    self.switch_tab_prev(None, update_mode_infos, client_id)?;
                },
            };
            let new_active_tab = self.get_active_tab_mut(client_id)?;

            if let Some(placement) = overlay_placement {
                // an overlay is not on the floating layer, so arriving here must not raise it
                new_active_tab.add_overlay_pane(
                    active_pane,
                    active_pane_id,
                    placement.kind,
                    placement.coordinates.unwrap_or_default(),
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
        self.render(None)?;
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
        let _ = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::PaneClosed(pane_id.into()),
            )]));
        self.notify_pane_closed_to_subscribers(pane_id.into());
        // not checking the result: this may be happening while the app is closing, at
        // which point the pty thread is already gone
        let _ = self
            .bus
            .senders
            .send_to_pty(PtyInstruction::ClosePane(pane_id, None));
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

        // remember where each pane came from, so that a destination that refuses it can
        // be recovered from rather than losing the pane
        let mut extracted: Vec<(
            TabId,
            ExtractedPanePlacement,
            Box<dyn Pane>,
            Option<(RunCommand, bool)>,
        )> = Vec::with_capacity(to_extract.len());
        {
            let all_tabs = self.get_tabs_mut();
            'panes: for pane_id in to_extract {
                for tab in all_tabs.values_mut() {
                    if tab.id == tab_id || !tab.has_pane_with_pid(&pane_id) {
                        continue;
                    }
                    let source_tab_id = tab.id;
                    // *before* the extraction: it is what drops an overlay's kind and
                    // coordinates, and "not floating" would land a modal here as a tiled pane
                    let placement = if arrangement && tab.pane_is_stack_list_member(&pane_id) {
                        ExtractedPanePlacement::Tiled
                    } else {
                        tab.placement_for_extraction(pane_id)
                    };
                    let carried_respawn = tab.take_pending_respawn(pane_id);
                    // dont_swap_if_suppressed, and no client_id: we do not need to
                    // necessarily trigger a relayout for the source tab
                    if let Some(pane) = tab.extract_pane(pane_id, true) {
                        extracted.push((source_tab_id, placement, pane, carried_respawn));
                        continue 'panes;
                    } else if let Some(entry) = carried_respawn {
                        tab.restore_pending_respawn(pane_id, entry);
                    }
                }
                outcome.not_found.push(pane_id);
            }
        }

        let mut refused: Vec<(
            TabId,
            ExtractedPanePlacement,
            Box<dyn Pane>,
            Option<(RunCommand, bool)>,
        )> = vec![];
        match self.get_tab_by_id_mut(tab_id) {
            Some(destination) => {
                for (source_tab_id, placement, pane, carried_respawn) in extracted {
                    let pane_id = pane.pid();
                    let accepted = if arrangement {
                        destination.accept_arranged_pane(pane, placement.clone())
                    } else {
                        destination.accept_extracted_pane(pane, placement.clone())
                    };
                    match accepted {
                        AcceptedPane::Accepted => {
                            outcome.moved.push(pane_id);
                            if let Some(entry) = carried_respawn {
                                destination.restore_pending_respawn(pane_id, entry);
                            }
                        },
                        AcceptedPane::NoRoom(pane) => {
                            refused.push((source_tab_id, placement, pane, carried_respawn))
                        },
                        AcceptedPane::LostInAdd => outcome.closed.push(pane_id),
                    }
                }
                // A batch carried over as one arrives as one: two overlays moved together and
                // landing edge to edge are the panel they were on the tab they left, seam
                // included. A move of a single pane links nothing.
                let together: Vec<PaneId> = outcome
                    .moved
                    .iter()
                    .chain(outcome.already_in_destination.iter())
                    .copied()
                    .collect();
                destination.link_touching_overlays(&together);
            },
            // Unreachable: the destination was found by this same key above, and
            // nothing since has removed a tab. Handled as a refusal anyway rather than
            // with `?`, because an early return here would drop every pane already
            // extracted - which is precisely how this function used to lose them.
            None => {
                log::error!("Tab {tab_id} vanished mid-move");
                refused = extracted;
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
