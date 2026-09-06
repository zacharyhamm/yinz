use zellij_runtime::errors::prelude::*;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;
use zellij_types::internal_ids::TabPosition;

use super::Screen;
use crate::background_jobs::BackgroundJob;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::tab::{AcceptedPane, ExtractedPanePlacement, Pane, SuppressedPanes, Tab};
use crate::{ClientId, ServerInstruction};
use std::collections::{HashMap, HashSet};
use zellij_types::geometry::Direction;
use zellij_types::input::command::RunCommand;
use {zellij_types::data::Event, zellij_types::data::ModeInfo};

impl Screen {
    pub(super) fn get_new_tab_id(&self) -> TabId {
        if let Some(id) = self.tabs.keys().last() {
            TabId::new(id.get() + 1)
        } else {
            TabId::new(0)
        }
    }

    /// Gets a tab by its stable ID (BTreeMap key).
    ///
    /// Use this when you have a tab ID from active_tab_ids, tab_history, or tab.id.
    pub(super) fn get_tab_by_id(&self, id: TabId) -> Option<&Tab> {
        self.tabs.get(&id)
    }

    /// Gets a mutable tab by its stable ID (BTreeMap key).
    pub(super) fn get_tab_by_id_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.get_mut(&id)
    }

    /// Whether any tab is still waiting for its layout to land. A tab is pending from its
    /// creation until `apply_layout` runs for it; until then its pane count and geometry are
    /// provisional, and instructions that move clients between tabs wait for it. Asked of the
    /// tabs themselves, so a tab made off the `NewTab` instruction - for a warped or an agent
    /// pane - counts too, and a pending tab closed before its layout lands stops counting.
    pub(super) fn has_pending_tabs(&self) -> bool {
        self.tabs.values().any(Tab::is_pending)
    }

    /// Gets the tab holding `pane_id`, if any.
    ///
    /// "Find the tab containing this pane, then act on it" is the single most repeated shape in
    /// the screen thread; before this existed it was spelled out as a `for` loop over
    /// `has_pane_with_pid` at eighty-odd sites.
    pub(super) fn tab_containing_pane(&self, pane_id: PaneId) -> Option<&Tab> {
        self.tabs
            .values()
            .find(|tab| tab.has_pane_with_pid(&pane_id))
    }

    /// The stable id of the tab holding `pane_id`.
    pub(super) fn tab_id_containing_pane(&self, pane_id: PaneId) -> Option<TabId> {
        self.tab_containing_pane(pane_id).map(|tab| tab.id)
    }

    /// The display position of the tab holding `pane_id`, for the callers that go on to look the
    /// tab up by position rather than by id.
    pub(super) fn tab_position_containing_pane(&self, pane_id: PaneId) -> Option<TabPosition> {
        self.tab_containing_pane(pane_id).map(|tab| tab.position)
    }

    /// The mutable half of [`Screen::tab_containing_pane`].
    pub(super) fn tab_containing_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Tab> {
        self.tabs
            .values_mut()
            .find(|tab| tab.has_pane_with_pid(&pane_id))
    }

    /// The tab holding `pane_id` as an overlay, if any.
    ///
    /// Separate from `tab_containing_pane_mut` because the suppress/unsuppress instructions have
    /// to tell the two apart: an overlay is hidden in place, whereas a tab pane is extracted into
    /// `suppressed_panes`, and running an overlay through the second would drop the kind and
    /// coordinates that make it one.
    pub(super) fn tab_containing_overlay(&self, pane_id: PaneId) -> Option<&Tab> {
        self.tabs.values().find(|tab| tab.pane_is_overlay(pane_id))
    }

    pub(super) fn tab_containing_overlay_mut(&mut self, pane_id: PaneId) -> Option<&mut Tab> {
        self.tabs
            .values_mut()
            .find(|tab| tab.pane_is_overlay(pane_id))
    }

    /// Reports a pane id that matched no tab, both to the log and to a CLI client waiting on the
    /// instruction. The `None` arm of nearly every `tab_containing_pane_mut` lookup.
    pub(super) fn report_pane_not_found(
        pane_id: PaneId,
        completion_tx: &mut Option<NotificationEnd>,
    ) {
        log::error!("Pane with id {:?} not found", pane_id);
        if let Some(completion_tx) = completion_tx.as_mut() {
            completion_tx.set_exit_status(1);
            completion_tx.set_error_message(format!("Pane with id {:?} not found", pane_id));
        }
    }

    /// Gets a mutable tab by its display position (0-based).
    pub(super) fn get_tab_by_position_mut(&mut self, position: TabPosition) -> Option<&mut Tab> {
        self.tabs.values_mut().find(|t| t.position == position)
    }

    /// Gets any one tab, for operations that need *a* tab rather than a specific one.
    ///
    /// This is the lowest-id tab, which is not necessarily the leftmost one. Callers
    /// that want the leftmost tab want `get_tab_by_position_mut(0)`.
    pub fn first_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.values_mut().next()
    }

    /// Gets the stable ID of the tab at the given display position.
    ///
    /// Use this to convert position → ID for BTreeMap lookups.
    pub(super) fn get_tab_id_at_position(&self, position: TabPosition) -> Option<TabId> {
        self.tabs
            .values()
            .find(|t| t.position == position)
            .map(|t| t.id)
    }

    /// Gets the display position of the tab with the given ID.
    ///
    /// Use this to convert ID → position for user-facing operations.
    pub(super) fn get_tab_position_by_id(&self, id: TabId) -> Option<TabPosition> {
        self.tabs.get(&id).map(|t| t.position)
    }

    fn move_clients_from_closed_tab(
        &mut self,
        client_ids_and_mode_infos: Vec<(ClientId, ModeInfo)>,
    ) -> Result<()> {
        let err_context = || "failed to move clients from closed tab".to_string();

        if self.tabs.is_empty() {
            Err::<(), _>(anyhow!(
                "No tabs left, cannot move clients: {:?} from closed tab",
                client_ids_and_mode_infos
            ))
            .with_context(err_context)
            .non_fatal();

            return Ok(());
        }
        let first_tab_index = *self
            .tabs
            .keys()
            .next()
            .context("screen contained no tabs")
            .with_context(err_context)?;
        let mut destination_tab_ids = HashSet::new();
        for (client_id, client_mode_info) in client_ids_and_mode_infos {
            let client_tab_history = self.tab_history.entry(client_id).or_default();
            if let Some(client_previous_tab) = client_tab_history.pop() {
                if let Some(client_active_tab) = self.tabs.get_mut(&client_previous_tab) {
                    self.active_tab_ids.insert(client_id, client_previous_tab);
                    client_active_tab
                        .add_client(client_id, Some(client_mode_info))
                        .with_context(err_context)?;
                    destination_tab_ids.insert(client_previous_tab);
                    continue;
                }
            }
            self.active_tab_ids.insert(client_id, first_tab_index);
            self.tabs
                .get_mut(&first_tab_index)
                .with_context(err_context)?
                .add_client(client_id, Some(client_mode_info))
                .with_context(err_context)?;
            destination_tab_ids.insert(first_tab_index);
        }
        // The plugins in a tab a client just landed in last heard about that client's
        // mode when it left, if ever - `update_input_modes` only covers the connected
        // clients, and the ones that just arrived were not among them until now.
        for destination_tab_id in destination_tab_ids {
            if let Some(destination_tab) = self.tabs.get_mut(&destination_tab_id) {
                destination_tab
                    .update_input_modes()
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    fn move_suppressed_panes_from_closed_tab(
        &mut self,
        suppressed_panes: SuppressedPanes,
        pending_respawns: HashMap<PaneId, (RunCommand, bool)>,
    ) -> Result<()> {
        // TODO: this is not entirely accurate, these also sometimes contain a pane who's
        // scrollback is being edited - in this case we need to close it or to move it to the
        // appropriate tab
        let err_context = || "Failed to move suppressed panes from closed tab";
        let first_tab_index = *self
            .tabs
            .keys()
            .next()
            .context("screen contains no tabs")
            .with_context(err_context)?;
        let destination = self
            .tabs
            .get_mut(&first_tab_index)
            .with_context(err_context)?;
        destination.add_suppressed_panes(suppressed_panes);
        for (pane_id, entry) in pending_respawns {
            destination.restore_pending_respawn(pane_id, entry);
        }
        Ok(())
    }

    /// Give a closed tab's overlays to a tab that is still here.
    ///
    /// An overlay belongs to a `Tab` but is drawn over the whole session surface, and hocus
    /// keeps a session's worth of conversation in one. Closing the tab it happens to be sitting
    /// on is not a request to throw that away - so `close_tab_by_id` extracts them
    /// ([`Tab::extract_overlay_panes`]) and they land here.
    ///
    /// **Where they land is where the user lands.** `client` is the lowest-numbered client of
    /// the tab that closed, and `move_clients_from_closed_tab` has already written its
    /// destination into `active_tab_ids` - so reading it back is how the overlays follow the
    /// user with no visible second hop.
    ///
    /// Following the *client* rather than a neighbouring tab position is also the only answer
    /// hocus cannot recover from getting wrong. `log_and_report_session_state` reports the panes
    /// before the tabs, so hocus's `on_panes` has already set `own_tab_index` to wherever we put
    /// it by the time `on_tabs` asks `overlay::tab_to_follow` - which then sees a pane that is
    /// where it belongs and asks for nothing. A pane left on a tab nobody is on therefore stays
    /// there, unseen, until somebody wanders onto that tab.
    ///
    /// With no client to follow - a background tab closed by `CloseTabWithIndex`, or a cli
    /// action - the lowest surviving tab id, which is the rule
    /// [`Screen::move_suppressed_panes_from_closed_tab`] uses for the same situation.
    ///
    /// They arrive **unfocused**, exactly as they do when hocus carries them across a tab switch
    /// itself: a tab closing is not a request to type into hocus. Modals never reach here - see
    /// [`Tab::extract_overlay_panes`] for why they close with their tab instead.
    fn move_overlay_panes_from_closed_tab(
        &mut self,
        overlay_panes: Vec<(ExtractedPanePlacement, Box<dyn Pane>)>,
        mut pending_respawns: HashMap<PaneId, (RunCommand, bool)>,
        client: Option<ClientId>,
    ) {
        if overlay_panes.is_empty() {
            return;
        }
        let destination_tab_id = client
            .and_then(|client_id| self.active_tab_ids.get(&client_id).copied())
            .filter(|tab_id| self.tabs.contains_key(tab_id))
            .or_else(|| self.tabs.keys().next().copied());

        // Collected rather than closed in place: `close_homeless_pane` needs `&mut self` and the
        // destination tab is borrowed from `self` for the length of the loop. The same ordering
        // `break_multiple_panes_to_tab_with_id` keeps, and for the same reason - dropping a
        // `Box<dyn Pane>` without tearing it down first destroys the pane silently.
        let mut homeless = vec![];
        match destination_tab_id.and_then(|tab_id| self.get_tab_by_id_mut(tab_id)) {
            Some(destination) => {
                for (placement, pane) in overlay_panes {
                    let pane_id = pane.pid();
                    match destination.accept_extracted_pane(pane, placement) {
                        // an overlay is not on a layer and does not tile, so `accept_extracted_pane`
                        // has nothing to run out of and this arm is the only one it can reach for
                        // one - `NoRoom` and `LostInAdd` are here for the contract, not because
                        // they are expected
                        AcceptedPane::Accepted => {
                            if let Some(entry) = pending_respawns.remove(&pane_id) {
                                destination.restore_pending_respawn(pane_id, entry);
                            }
                        },
                        AcceptedPane::NoRoom(..) | AcceptedPane::LostInAdd => {
                            log::error!("Tab {destination_tab_id:?} would not take overlay {pane_id:?}; closing it");
                            homeless.push(pane_id);
                        },
                    }
                }
            },
            // Unreachable: the caller only extracts overlays when a tab survives, and nothing
            // between there and here removes one. Handled anyway rather than dropped, because a
            // dropped overlay is a plugin that stops existing with nothing told about it.
            None => {
                log::error!("No tab left to re-home a closed tab's overlays into; closing them");
                homeless.extend(overlay_panes.into_iter().map(|(_, pane)| pane.pid()));
            },
        }
        for pane_id in homeless {
            self.close_homeless_pane(pane_id);
        }
    }

    fn move_clients_between_tabs(
        &mut self,
        source_tab_index: TabId,
        destination_tab_index: TabId,
        update_mode_infos: bool,
        clients_to_move: Option<Vec<ClientId>>,
    ) -> Result<()> {
        let err_context = || {
            format!(
                "failed to move clients from tab {source_tab_index} to tab {destination_tab_index}"
            )
        };

        // None ==> move all clients
        let drained_clients = self
            .get_tab_by_id_mut(source_tab_index)
            .map(|t| t.drain_connected_clients(clients_to_move));

        if let Some(client_mode_info_in_source_tab) = drained_clients {
            let destination_tab = self
                .get_tab_by_id_mut(destination_tab_index)
                .context("failed to get destination tab by index")
                .with_context(err_context)?;
            destination_tab
                .add_multiple_clients(client_mode_info_in_source_tab)
                .with_context(err_context)?;
            if update_mode_infos {
                destination_tab
                    .update_input_modes()
                    .with_context(err_context)?;
            }
            destination_tab.set_force_render();
            if destination_tab.has_stack_lists() {
                destination_tab.set_should_clear_display_before_rendering();
            }
            destination_tab.visible(true).with_context(err_context)?;
        }
        Ok(())
    }

    pub(super) fn update_client_tab_focus(&mut self, client_id: ClientId, new_tab_index: TabId) {
        match self.active_tab_ids.remove(&client_id) {
            Some(old_active_index) => {
                self.active_tab_ids.insert(client_id, new_tab_index);
                if old_active_index != new_tab_index {
                    self.repaint_after_leaving_tab(old_active_index, new_tab_index);
                }
                let client_tab_history = self.tab_history.entry(client_id).or_default();
                client_tab_history.retain(|&e| e != new_tab_index);
                client_tab_history.push(old_active_index);
            },
            None => {
                self.active_tab_ids.insert(client_id, new_tab_index);
            },
        }
        self.clear_bell_for_focused_pane(client_id);
    }

    /// Repaint the tab a client is switching *to*, if the tab it is leaving was painting a
    /// floating pane outside its own viewport.
    ///
    /// A floating pane can be placed - or dragged - onto the tab bar or the status bar, which no
    /// pane's grid owns. The tab being left stops rendering the moment its last client is gone,
    /// which is right: the pane belongs to that tab. But the tab being entered renders diff-based,
    /// so if nothing in it changed it emits nothing and the pane stays on the host terminal over a
    /// tab it is not in.
    fn repaint_after_leaving_tab(&mut self, left_tab_id: TabId, entered_tab_id: TabId) {
        if !self
            .tabs
            .get(&left_tab_id)
            .map(|tab| tab.has_floating_panes_outside_viewport())
            .unwrap_or(false)
        {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(&entered_tab_id) {
            tab.set_should_clear_display_before_rendering();
            tab.set_force_render();
        }
    }

    /// A helper function to switch to a new tab at specified position.
    pub(super) fn switch_active_tab(
        &mut self,
        new_tab_pos: TabPosition,
        should_change_pane_focus: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || {
            format!(
            "Failed to switch to active tab at position {new_tab_pos} for client id: {client_id:?}"
        )
        };

        if let Some(new_tab) = self.tabs.values().find(|t| t.position == new_tab_pos) {
            match self.get_active_tab(client_id) {
                Ok(current_tab) => {
                    // If new active tab is same as the current one, do nothing.
                    if current_tab.position == new_tab_pos {
                        return Ok(());
                    }

                    let current_tab_index = current_tab.id;
                    let new_tab_index = new_tab.id;
                    let affected_client_ids: Vec<ClientId> = if self.session_is_mirrored {
                        self.connected_clients.borrow().iter().copied().collect()
                    } else {
                        vec![client_id]
                    };
                    let old_focused_panes: Vec<(ClientId, Option<PaneId>)> = affected_client_ids
                        .iter()
                        .map(|c| (*c, self.get_active_pane_id(c)))
                        .collect();
                    if self.session_is_mirrored {
                        self.move_clients_between_tabs(
                            current_tab_index,
                            new_tab_index,
                            update_mode_infos,
                            None,
                        )
                        .with_context(err_context)?;
                        let all_connected_clients: Vec<ClientId> =
                            self.connected_clients.borrow().iter().copied().collect();
                        for client_id in all_connected_clients {
                            self.update_client_tab_focus(client_id, new_tab_index);
                            match (
                                should_change_pane_focus,
                                self.get_tab_by_id_mut(new_tab_index),
                            ) {
                                (Some(direction), Some(new_tab)) => {
                                    new_tab.focus_pane_on_edge(direction, client_id);
                                },
                                _ => {},
                            }
                        }
                    } else {
                        self.move_clients_between_tabs(
                            current_tab_index,
                            new_tab_index,
                            update_mode_infos,
                            Some(vec![client_id]),
                        )
                        .with_context(err_context)?;
                        match (
                            should_change_pane_focus,
                            self.get_tab_by_id_mut(new_tab_index),
                        ) {
                            (Some(direction), Some(new_tab)) => {
                                new_tab.focus_pane_on_edge(direction, client_id);
                            },
                            _ => {},
                        }
                        self.update_client_tab_focus(client_id, new_tab_index);
                    }

                    if let Some(current_tab) = self.get_tab_by_id_mut(current_tab_index) {
                        if current_tab.has_no_connected_clients() {
                            current_tab.visible(false).with_context(err_context)?;
                        }
                    } else {
                        Err::<(), _>(anyhow!("Tab index {:?} not found", current_tab_index))
                            .with_context(err_context)
                            .non_fatal();
                    }

                    // Clear tab bell notification for the newly active tab
                    if let Some(new_tab) = self.tabs.get_mut(&new_tab_index) {
                        let tab_id = new_tab.id;
                        new_tab.clear_tab_bell_notification();
                        let _ = self
                            .bus
                            .senders
                            .send_to_background_jobs(BackgroundJob::StopFlashTabBell(tab_id));
                    }

                    self.recompute_tab_size(current_tab_index)
                        .with_context(err_context)?;
                    self.recompute_tab_size(new_tab_index)
                        .with_context(err_context)?;

                    self.log_and_report_session_state()
                        .with_context(err_context)?;
                    for (affected_client_id, old_pane_id) in old_focused_panes {
                        let new_pane_id = self.get_active_pane_id(&affected_client_id);
                        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                            self.report_key_passthrough_state(affected_client_id, old, new);
                        }
                        self.sync_scroll_mode_on_focus(affected_client_id)
                            .non_fatal();
                    }
                    return self.render(None).with_context(err_context);
                },
                Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
            }
        }
        Ok(())
    }

    /// A helper function to switch to a new tab with specified name. Return true if tab [name] has
    /// been created, else false.
    pub(super) fn switch_active_tab_name(
        &mut self,
        name: String,
        client_id: ClientId,
    ) -> Result<bool> {
        match self.tabs.values().find(|t| t.name == name) {
            Some(new_tab) => {
                self.switch_active_tab(new_tab.position, None, true, client_id)?;
                Ok(true)
            },
            None => Ok(false),
        }
    }

    /// Sets this [`Screen`]'s active [`Tab`] to the next tab.
    pub fn switch_tab_next(
        &mut self,
        should_change_pane_focus: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || format!("failed to switch to next tab for client {client_id}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        if let Some(client_id) = client_id {
            match self.get_active_tab(client_id) {
                Ok(active_tab) => {
                    let active_tab_pos = active_tab.position;
                    let new_tab_pos =
                        TabPosition::new((active_tab_pos.get() + 1) % self.tabs.len());
                    return self.switch_active_tab(
                        new_tab_pos,
                        should_change_pane_focus,
                        update_mode_infos,
                        client_id,
                    );
                },
                Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
            }
        }
        Ok(())
    }

    /// Sets this [`Screen`]'s active [`Tab`] to the previous tab.
    pub fn switch_tab_prev(
        &mut self,
        should_change_pane_focus: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || format!("failed to switch to previous tab for client {client_id}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        if let Some(client_id) = client_id {
            match self.get_active_tab(client_id) {
                Ok(active_tab) => {
                    let active_tab_pos = active_tab.position;
                    let new_tab_pos = TabPosition::new(if active_tab_pos.get() == 0 {
                        self.tabs.len() - 1
                    } else {
                        active_tab_pos.get() - 1
                    });

                    return self.switch_active_tab(
                        new_tab_pos,
                        should_change_pane_focus,
                        update_mode_infos,
                        client_id,
                    );
                },
                Err(err) => Err::<(), _>(err).with_context(err_context).non_fatal(),
            }
        }
        Ok(())
    }

    pub fn go_to_tab(&mut self, tab_index: TabNumber, client_id: ClientId) -> Result<()> {
        self.switch_active_tab(tab_index.position(), None, true, client_id)
    }

    pub fn go_to_tab_name(&mut self, name: String, client_id: ClientId) -> Result<bool> {
        self.switch_active_tab_name(name, client_id)
    }

    pub(super) fn close_tab_by_id(&mut self, tab_id: TabId) -> Result<()> {
        let err_context = || format!("failed to close tab at index {tab_id:?}");

        let mut tab_to_close = self.tabs.remove(&tab_id).with_context(err_context)?;
        let mut pane_ids = tab_to_close.get_all_pane_ids();

        // here we extract the suppressed panes (these are background panes that don't care which
        // tab they are in, and in the future we should probably make them global to screen rather
        // than to each tab) and move them to another tab if there is one
        let suppressed_panes = tab_to_close.extract_suppressed_panes();
        let suppressed_respawns = suppressed_panes
            .keys()
            .filter_map(|pane_id| {
                tab_to_close
                    .take_pending_respawn(*pane_id)
                    .map(|entry| (*pane_id, entry))
            })
            .collect();
        for suppressed_pane_id in suppressed_panes.keys() {
            pane_ids.retain(|p| p != suppressed_pane_id);
        }

        // The same treatment, and for a stronger reason: an overlay is drawn over the whole
        // session surface rather than inside this tab, and hocus keeps a session's worth of
        // conversation in one. Taken out here so that the teardown below - `PaneClosed`, the
        // subscriber notification, and the `CloseTab` that ends in `PluginInstruction::Unload` -
        // never names them, and re-homed further down once it is known where the clients went.
        //
        // Only when a tab survives: with none left the session is ending anyway (see the
        // `Render(None)` below), and an overlay extracted with nowhere to go would be a plugin
        // left running with no pane and nothing told about it.
        let overlay_panes = match self.tabs.is_empty() {
            true => vec![],
            false => tab_to_close.extract_overlay_panes(),
        };
        let overlay_respawns = overlay_panes
            .iter()
            .filter_map(|(_, pane)| {
                let pane_id = pane.pid();
                tab_to_close
                    .take_pending_respawn(pane_id)
                    .map(|entry| (pane_id, entry))
            })
            .collect();
        for (_, overlay_pane) in &overlay_panes {
            let overlay_pane_id = overlay_pane.pid();
            pane_ids.retain(|p| p != &overlay_pane_id);
        }

        let _ = self.bus.senders.send_to_plugin(PluginInstruction::Update(
            pane_ids
                .iter()
                .copied()
                .map(|p_id| (None, None, Event::PaneClosed(p_id.into())))
                .collect(),
        ));

        // Notify pane render subscribers of each closed pane
        for p_id in &pane_ids {
            self.notify_pane_closed_to_subscribers((*p_id).into());
        }

        // A closing tab takes its panes with it without ever emitting a per-pane `ClosePane`, so
        // the menus opened over them have to be closed here too - `close_context_menus_for_pane`
        // is otherwise only reached from that instruction. The suppressed panes have already been
        // filtered out above: they move to another tab and keep theirs.
        for p_id in &pane_ids {
            self.close_context_menus_for_pane(*p_id);
        }

        // below we don't check the result of sending the CloseTab instruction to the pty thread
        // because this might be happening when the app is closing, at which point the pty thread
        // has already closed and this would result in an error
        self.bus
            .senders
            .send_to_pty(PtyInstruction::CloseTab(pane_ids))
            .with_context(err_context)?;
        if self.tabs.is_empty() {
            self.active_tab_ids.clear();
            self.bus
                .senders
                .send_to_server(ServerInstruction::Render(None))
                .with_context(err_context)
        } else {
            let client_mode_infos_in_closed_tab = tab_to_close.drain_connected_clients(None);
            // The client the overlays follow. Lowest id rather than first: `connected_clients` is
            // a `HashSet`, so "first" is not a stable answer and the panes would land somewhere
            // else from one run to the next.
            let overlay_client = client_mode_infos_in_closed_tab
                .iter()
                .map(|(client_id, _)| *client_id)
                .min();
            self.move_clients_from_closed_tab(client_mode_infos_in_closed_tab)
                .with_context(err_context)?;
            self.move_suppressed_panes_from_closed_tab(suppressed_panes, suppressed_respawns)
                .with_context(err_context)?;
            // After the clients, which is what wrote `overlay_client`'s destination into
            // `active_tab_ids`, and before the resize and the state report below, so the tab they
            // land in is measured and announced with them already in it.
            self.move_overlay_panes_from_closed_tab(
                overlay_panes,
                overlay_respawns,
                overlay_client,
            );
            let visible_tab_indices: HashSet<TabId> =
                self.active_tab_ids.values().copied().collect();
            for t in self.tabs.values_mut() {
                if visible_tab_indices.contains(&t.id) {
                    t.set_force_render();
                    t.visible(true).with_context(err_context)?;
                }
                if t.position > tab_to_close.position {
                    t.position = TabPosition::new(t.position.get() - 1);
                }
            }
            for tab_id in visible_tab_indices {
                self.recompute_tab_size(tab_id).with_context(err_context)?;
            }
            self.log_and_report_session_state()
                .with_context(err_context)?;
            self.render(None).with_context(err_context)
        }
    }

    // Closes the client_id's focused tab
    pub fn close_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to close tab for client {client_id:?}");

        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };

        match client_id {
            Some(client_id) => {
                let active_tab_index = *self
                    .active_tab_ids
                    .get(&client_id)
                    .with_context(err_context)?;
                self.close_tab_by_id(active_tab_index)
                    .with_context(err_context)
            },
            None => Ok(()),
        }
    }
}
