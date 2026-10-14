use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

use super::{find_already_running_panes, Screen};
use crate::background_jobs::BackgroundJob;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::tab::{SuppressedPanes, Tab};
use crate::{ClientId, ServerInstruction};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use yinz_types::data::CommandOrPlugin;
use yinz_types::geometry::Direction;
use yinz_types::input::command::{RunCommand, TerminalAction};
use yinz_types::input::layout::{
    FloatingPaneLayout, SwapFloatingLayout, SwapTiledLayout, TabLayoutInfo, TiledPaneLayout,
};
use {yinz_types::data::Event, yinz_types::data::ModeInfo};

pub(super) struct TabTransition {
    destination: TabId,
    clients: Option<Vec<(ClientId, ModeInfo)>>,
    pub old_focus: Vec<(ClientId, Option<PaneId>)>,
}

impl Screen {
    /// Switches the client to the tab called `tab_name`, creating it first when `create` is set
    /// and no tab has that name. A client that is not connected falls back to the first one.
    pub(super) fn go_to_or_create_tab_by_name(
        &mut self,
        tab_name: String,
        default_shell: Option<TerminalAction>,
        create: bool,
        client_id: Option<ClientId>,
        mut completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let swap_layouts = (
            self.default_layout.swap_tiled_layouts.clone(),
            self.default_layout.swap_floating_layouts.clone(),
        );
        let client_id = match client_id {
            None => None,
            Some(client_id) if self.active_tab_ids.contains_key(&client_id) => Some(client_id),
            Some(_) => self.active_tab_ids.keys().next().copied(),
        };
        if let Some(client_id) = client_id {
            if let Ok(tab_exists) = self.go_to_tab_name(tab_name.clone(), client_id) {
                self.render()?;
                if tab_exists {
                    // Tab already exists - find its ID and set in completion
                    if let Some(existing_tab) = self.tabs.values().find(|t| t.name == tab_name) {
                        if let Some(c) = completion_tx.as_mut() {
                            c.set_affected_tab_id(existing_tab.id)
                        }
                    }
                }
                if create && !tab_exists {
                    let tab_index = self.get_new_tab_id();
                    let should_change_focus_to_new_tab = true;
                    // The name was typed in the command that asked for the tab.
                    let user_named = !tab_name.is_empty();
                    self.new_tab(tab_index, swap_layouts, Some(tab_name), Some(client_id))?;
                    if let Some(tab) = self.tabs.get_mut(&tab_index) {
                        tab.set_user_named(user_named);
                    }
                    self.bus.senders.send_to_plugin(PluginInstruction::NewTab(
                        None,
                        default_shell,
                        None,
                        None,
                        tab_index,
                        None,  // initial_panes
                        false, // block_on_first_terminal
                        should_change_focus_to_new_tab,
                        client_id,
                        completion_tx,
                    ))?;
                    return Ok(()); // so we don't get to the completion signalling below
                }
            }
        }
        Ok(())
    }

    /// Creates an empty tab and asks the plugin thread to fill it: the layout lands later as
    /// `ApplyLayout`. `None` swap layouts fall back to the default layout's.
    pub(super) fn open_new_tab(
        &mut self,
        cwd: Option<PathBuf>,
        default_shell: Option<TerminalAction>,
        layout: Option<TiledPaneLayout>,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        tab_name: Option<String>,
        swap_tiled_layouts: Option<Vec<SwapTiledLayout>>,
        swap_floating_layouts: Option<Vec<SwapFloatingLayout>>,
        initial_panes: Option<Vec<CommandOrPlugin>>,
        block_on_first_terminal: bool,
        should_change_focus_to_new_tab: bool,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let tab_index = self.get_new_tab_id();
        let client_id_for_new_tab = if should_change_focus_to_new_tab {
            Some(client_id)
        } else {
            None
        };
        let resolved_swap_layouts = (
            swap_tiled_layouts.unwrap_or_else(|| self.default_layout.swap_tiled_layouts.clone()),
            swap_floating_layouts
                .unwrap_or_else(|| self.default_layout.swap_floating_layouts.clone()),
        );
        self.new_tab(
            tab_index,
            resolved_swap_layouts,
            tab_name.clone(),
            client_id_for_new_tab,
        )?;
        let user_named = Screen::tab_name_is_users(tab_name.as_deref(), layout.as_ref());
        if let Some(tab) = self.tabs.get_mut(&tab_index) {
            tab.set_user_named(user_named);
        }
        self.bus.senders.send_to_plugin(PluginInstruction::NewTab(
            cwd,
            default_shell,
            layout,
            (!floating_panes_layout.is_empty()).then_some(floating_panes_layout),
            tab_index,
            initial_panes,
            block_on_first_terminal,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        ))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn override_layout(
        &mut self,
        cwd: Option<PathBuf>,
        default_shell: Option<TerminalAction>,
        mut tab_layouts: Vec<TabLayoutInfo>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        apply_only_to_focused_tab: bool,
        client_id: ClientId,
        mut completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let focused_tab_id = if apply_only_to_focused_tab {
            if tab_layouts.is_empty() {
                self.report_layout_error(
                    "No tab layouts found, cannot override.".into(),
                    client_id,
                    &mut completion_tx,
                );
                return Ok(());
            }
            tab_layouts.truncate(1);
            match self.get_active_tab(client_id) {
                Ok(tab) => Some(tab.id),
                Err(e) => {
                    self.report_layout_error(
                        format!("Failed to override layout of active tab: {e:#}"),
                        client_id,
                        &mut completion_tx,
                    );
                    return Ok(());
                },
            }
        } else {
            None
        };

        // Resolve all positions before creating or closing tabs changes their positions.
        let tab_layouts: Vec<_> = tab_layouts
            .into_iter()
            .map(|layout| {
                let id = focused_tab_id.or_else(|| self.get_tab_id_at_position(layout.tab_index));
                (id, layout)
            })
            .collect();
        let mut tabs_to_close: HashSet<TabId> = self.tabs.keys().copied().collect();
        let mut processed_tab_layouts = Vec::new();
        for (tab_id, mut layout) in tab_layouts {
            let tab_id = match tab_id {
                Some(id) => id,
                None => {
                    // Reserve the ID before asynchronous plugin and PTY work starts.
                    let id = self.get_new_tab_id();
                    self.new_tab(
                        id,
                        (
                            layout.swap_tiled_layouts.clone().unwrap_or_default(),
                            layout.swap_floating_layouts.clone().unwrap_or_default(),
                        ),
                        layout.tab_name.clone(),
                        None,
                    )?;
                    id
                },
            };
            tabs_to_close.remove(&tab_id);
            let tab = self
                .tabs
                .get_mut(&tab_id)
                .context("override tab is missing")?;
            let (tiled_to_ignore, floating_indices) =
                find_already_running_panes(&layout.tiled_layout, &layout.floating_layouts, tab);
            for run_instruction in tiled_to_ignore {
                layout.tiled_layout.ignore_run_instruction(run_instruction);
            }
            for idx in floating_indices {
                layout.floating_layouts[idx].already_running = true;
            }
            processed_tab_layouts.push((tab_id, layout));
        }

        self.bus
            .senders
            .send_to_plugin(PluginInstruction::OverrideLayout(
                cwd,
                default_shell,
                if apply_only_to_focused_tab {
                    vec![]
                } else {
                    tabs_to_close.into_iter().collect()
                },
                processed_tab_layouts,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
                completion_tx,
            ))?;
        Ok(())
    }

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
        let first_tab_index = self
            .tabs
            .values()
            .find(|tab| !tab.is_pending() && tab.has_selectable_panes())
            .or_else(|| self.tabs.values().find(|tab| !tab.is_pending()))
            .map(|tab| tab.id)
            .context("screen contained no tabs")
            .with_context(err_context)?;
        let mut destination_tab_ids = HashSet::new();
        for (client_id, client_mode_info) in client_ids_and_mode_infos {
            // The tab it was on before, then the first one there is. `active_tab_ids` is written
            // only once a tab has actually taken the client: the two have to agree, and a client
            // pointed at a tab that did not take it receives no frames and reaches nothing.
            let client_tab_history = self.tab_history.entry(client_id).or_default();
            let previous_tab = client_tab_history.pop().filter(|id| {
                self.tabs
                    .get(id)
                    .is_some_and(|tab| !tab.is_pending() && tab.has_selectable_panes())
            });
            let candidates = previous_tab
                .into_iter()
                .chain(std::iter::once(first_tab_index));
            let mode_info = Some(client_mode_info);
            let mut landed = false;
            for candidate in candidates {
                let Some(tab) = self.tabs.get_mut(&candidate) else {
                    continue;
                };
                if tab.is_pending() {
                    continue;
                }
                match tab.add_client(client_id, mode_info.clone()) {
                    Ok(()) => {
                        self.active_tab_ids.insert(client_id, candidate);
                        destination_tab_ids.insert(candidate);
                        landed = true;
                        break;
                    },
                    // Kept, rather than consumed, so the next candidate is offered the mode the
                    // client left with rather than the tab's default.
                    Err(e) => Err::<(), _>(e)
                        .with_context(|| {
                            format!("tab {candidate} would not take client {client_id}")
                        })
                        .non_fatal(),
                }
            }
            if !landed {
                // Nothing took it, so leaving a stale entry behind would strand it. Dropped
                // instead: `Screen::add_client` rebuilds the mapping from scratch on a reattach.
                log::error!("no tab would take client {client_id} from the closed tab");
                self.active_tab_ids.remove(&client_id);
            }
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

    /// Give a closed tab's surviving suppressed chains to a tab that is still here.
    ///
    /// Parked panes and the panes behind them move together, keeping their restoration
    /// links and queued respawns. Chains whose stand-ins close stay in `teardown_tab`.
    fn move_suppressed_panes_from_closed_tab(
        &mut self,
        suppressed_panes: SuppressedPanes,
        pending_respawns: HashMap<PaneId, (RunCommand, bool)>,
    ) -> Result<()> {
        let err_context = || "Failed to move suppressed panes from closed tab";
        let first_tab_index = self
            .tabs
            .values()
            .find(|tab| !tab.is_pending() && tab.has_selectable_panes())
            .or_else(|| self.tabs.values().find(|tab| !tab.is_pending()))
            .map(|tab| tab.id)
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
    /// An overlay belongs to a `Tab` but is drawn over the whole session surface, and agent
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
    /// agent cannot recover from getting wrong. `log_and_report_session_state` reports the panes
    /// before the tabs, so agent's `on_panes` has already set `own_tab_index` to wherever we put
    /// it by the time `on_tabs` asks `overlay::tab_to_follow` - which then sees a pane that is
    /// where it belongs and asks for nothing. A pane left on a tab nobody is on therefore stays
    /// there, unseen, until somebody wanders onto that tab.
    ///
    /// With no client to follow - a background tab closed by `CloseTabWithIndex`, or a cli
    /// action - the lowest surviving tab id, which is the rule
    /// [`Screen::move_suppressed_panes_from_closed_tab`] uses for the same situation.
    ///
    /// A panel that had a client's keys **keeps them**: `close_tab_by_id` extracts it while that
    /// client is still on the closing tab, so the keyboard goes where the user goes. Only a panel
    /// carried after its client switched tabs - agent following a tab switch itself - arrives
    /// unfocused (`OverlayPanes::take_container`). Modals never reach here - see
    /// [`Tab::extract_overlay_panes`] for why they close with their tab instead.
    fn move_overlay_panes_from_closed_tab(
        &mut self,
        overlay_panes: Vec<crate::tab::OverlayTransfer>,
        client: Option<ClientId>,
    ) {
        if overlay_panes.is_empty() {
            return;
        }
        let destination_tab_id = client
            .and_then(|client_id| self.active_tab_ids.get(&client_id).copied())
            .filter(|tab_id| self.tabs.get(tab_id).is_some_and(|tab| !tab.is_pending()))
            .or_else(|| {
                self.tabs
                    .values()
                    .find(|tab| !tab.is_pending() && tab.has_selectable_panes())
                    .map(|tab| tab.id)
            })
            .or_else(|| {
                self.tabs
                    .values()
                    .find(|tab| !tab.is_pending())
                    .map(|tab| tab.id)
            });

        // Collected rather than closed in place: `close_homeless_pane` needs `&mut self` and the
        // destination tab is borrowed from `self` for the length of the loop. The same ordering
        // `break_multiple_panes_to_tab_with_id` keeps, and for the same reason - dropping a
        // `Box<dyn Pane>` without tearing it down first destroys the pane silently.
        let mut homeless = vec![];
        match destination_tab_id.and_then(|tab_id| self.get_tab_by_id_mut(tab_id)) {
            Some(destination) => {
                for container in overlay_panes {
                    destination.accept_overlay_container(container);
                }
            },
            // Unreachable: the caller only extracts overlays when a tab survives, and nothing
            // between there and here removes one. Handled anyway rather than dropped, because a
            // dropped overlay is a plugin that stops existing with nothing told about it.
            None => {
                log::error!("No tab left to re-home a closed tab's overlays into; closing them");
                homeless.extend(
                    overlay_panes
                        .iter()
                        .flat_map(|container| container.pane_ids()),
                );
            },
        }
        for pane_id in homeless {
            self.close_homeless_pane(pane_id);
        }
    }

    pub(super) fn begin_tab_transition(
        &mut self,
        client: ClientId,
        destination: TabId,
    ) -> TabTransition {
        let source = self.active_tab_ids.get(&client).copied();
        let affected: Vec<_> = if self.session_is_mirrored {
            self.connected_clients.borrow().iter().copied().collect()
        } else {
            vec![client]
        };
        let old_focus = affected
            .iter()
            .map(|id| (*id, self.get_active_pane_id(id)))
            .collect();
        let clients = source
            .and_then(|source| self.tabs.get_mut(&source))
            .map(|tab| {
                let clients = tab.drain_connected_clients(if self.session_is_mirrored {
                    None
                } else {
                    Some(vec![client])
                });
                if tab.has_no_connected_clients() {
                    tab.visible(false)
                        .context("failed to hide departed tab")
                        .non_fatal();
                }
                clients
            });
        if clients.is_some() || self.session_is_mirrored {
            for id in affected {
                self.update_client_tab_focus(id, destination);
            }
        }
        TabTransition {
            destination,
            clients,
            old_focus,
        }
    }

    pub(super) fn finish_tab_transition(
        &mut self,
        transition: &mut TabTransition,
        direction: Option<Direction>,
    ) -> Result<()> {
        if let Some(clients) = transition.clients.take() {
            let tab = self
                .tabs
                .get_mut(&transition.destination)
                .context("failed to find destination tab")?;
            tab.add_multiple_clients(clients)
                .context("failed to attach clients")?;
            tab.visible(true)
                .context("failed to show destination tab")?;
            tab.set_force_render();
            if tab.has_stack_lists() {
                tab.set_should_clear_display_before_rendering();
            }
            if let Some(direction) = direction {
                for (client, _) in &transition.old_focus {
                    tab.focus_pane_on_edge(direction, *client);
                }
            }
        }
        Ok(())
    }

    pub(super) fn update_client_tab_focus(&mut self, client_id: ClientId, new_tab_index: TabId) {
        match self.active_tab_ids.remove(&client_id) {
            Some(old_active_index) => {
                self.active_tab_ids.insert(client_id, new_tab_index);
                if old_active_index != new_tab_index {
                    self.close_command_palette(client_id);
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

    /// User tab navigation may reveal parked work; automatic relocation must not.
    pub(super) fn navigate_to_tab(
        &mut self,
        position: TabPosition,
        direction: Option<Direction>,
        update_mode_infos: bool,
        client_id: ClientId,
    ) -> Result<()> {
        let same_tab = self
            .get_active_tab(client_id)
            .is_ok_and(|tab| tab.position == position);
        if let Some(tab) = self.tabs.values_mut().find(|tab| tab.position == position) {
            let recovered = match tab
                .recover_suppressed_terminal_for_navigation(client_id)
                .context("failed to recover hidden terminal on tab navigation")
            {
                Ok(recovered) => recovered,
                Err(error) => {
                    Err::<(), _>(error).non_fatal();
                    return Ok(());
                },
            };
            // Switching to the current tab returns early, but recovery still needs publishing.
            if recovered && same_tab {
                self.log_and_report_session_state()
                    .context("failed to publish recovered tab")?;
                return self.render().context("failed to render recovered tab");
            }
        }
        self.switch_active_tab(position, direction, update_mode_infos, client_id)
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
                    let mut transition = self.begin_tab_transition(client_id, new_tab_index);
                    self.finish_tab_transition(&mut transition, should_change_pane_focus)
                        .with_context(err_context)?;
                    if update_mode_infos {
                        if let Some(tab) = self.tabs.get_mut(&new_tab_index) {
                            tab.update_input_modes().with_context(err_context)?;
                        }
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
                    for (affected_client_id, old_pane_id) in transition.old_focus {
                        let new_pane_id = self.get_active_pane_id(&affected_client_id);
                        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                            self.report_key_passthrough_state(affected_client_id, old, new);
                        }
                        self.sync_scroll_mode_on_focus(affected_client_id)
                            .non_fatal();
                    }
                    return self.render().with_context(err_context);
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
                self.navigate_to_tab(new_tab.position, None, true, client_id)?;
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
                    return self.navigate_to_tab(
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

                    return self.navigate_to_tab(
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
        self.navigate_to_tab(tab_index.position(), None, true, client_id)
    }

    pub fn go_to_tab_name(&mut self, name: String, client_id: ClientId) -> Result<bool> {
        self.switch_active_tab_name(name, client_id)
    }

    /// Settle tab lifetime after each instruction. Pending layouts are never destinations.
    /// Hidden-only tabs survive beside a usable tab; otherwise their work gets a replacement.
    /// Keep replacement identity after loading so natural exits hold instead of respawning.
    pub(crate) fn settle_tabs(&mut self) -> Result<()> {
        let err_context = "failed to settle tabs";
        for (client, tab_id) in &self.active_tab_ids {
            if let Some(tab) = self.tabs.get_mut(tab_id) {
                tab.ensure_selectable_focus(*client).context(err_context)?;
            }
        }
        let usable: Vec<TabId> = self
            .tabs
            .values()
            .filter(|tab| {
                !tab.is_pending()
                    && tab.has_selectable_panes()
                    && !self.deferred_tab_closes.contains(&tab.id)
            })
            .map(|tab| tab.id)
            .collect();
        if usable.is_empty() && self.in_flight_pane_warps > 0 {
            return Ok(());
        }
        let another_destination = usable.iter().any(|id| Some(*id) != self.replacement_tab);
        let doomed: Vec<TabId> = self
            .tabs
            .values()
            .filter(|tab| {
                !tab.is_pending()
                    && !self.pending_arrangement.as_ref().is_some_and(|pending| {
                        pending.destinations.values().any(|id| *id == tab.id)
                    })
                    && (self.deferred_tab_closes.contains(&tab.id)
                        || (!tab.agent_tab_waiting_for_pane
                            && !tab.has_selectable_panes()
                            && (!tab.has_live_terminal()
                                || usable.is_empty()
                                || (self.replacement_pending && !another_destination))))
            })
            .map(|tab| tab.id)
            .collect();
        // Relocate clients from retained hidden-work tabs. Teardown uses tab history for closing tabs.
        if let Some(destination) = usable.first().copied() {
            let stranded: Vec<_> = self
                .active_tab_ids
                .iter()
                .filter_map(|(client, id)| {
                    self.tabs
                        .get(id)
                        .filter(|tab| {
                            !tab.is_pending() && !tab.has_selectable_panes() && !doomed.contains(id)
                        })
                        .map(|_| *client)
                })
                .collect();
            for client in stranded {
                let position = self.tabs[&destination].position;
                self.switch_active_tab(position, None, true, client)
                    .context(err_context)?;
            }
        }
        if doomed.is_empty() {
            self.replacement_pending = false;
            return Ok(());
        }
        if usable.is_empty() {
            // A pending user, plugin, adopted or replacement tab is not a destination yet.
            if self.has_pending_tabs() {
                return Ok(());
            }
            if self.replacement_pending && self.replacement_tab.is_some() {
                // A layout with no usable pane must not cause an endless spawn loop.
                self.replacement_pending = false;
            } else if self.session_must_survive(&doomed) {
                if let Some(client_id) = self.active_tab_ids.keys().copied().min() {
                    self.open_replacement_tab(client_id).context(err_context)?;
                }
                return Ok(());
            }
        }
        let confirm = self.replacement_pending && !another_destination;
        self.replacement_pending = false;
        for tab_id in doomed {
            self.teardown_tab(tab_id, !usable.is_empty())
                .context(err_context)?;
        }
        if confirm && self.quit_is_guarded() {
            if let Some(client_id) = self.active_tab_ids.keys().copied().min() {
                self.confirm_quit(client_id, true, None)
                    .context(err_context)
                    .non_fatal();
            }
        }
        Ok(())
    }

    /// Whether the session has a reason to outlive its last usable pane: a plugin asked for a
    /// quit to be confirmed, or a terminal is still running hidden in one of the tabs about to
    /// close (a pane a plugin parked, whose process would die with the session).
    fn session_must_survive(&self, doomed: &[TabId]) -> bool {
        self.quit_is_guarded()
            || doomed
                .iter()
                .filter_map(|id| self.tabs.get(id))
                .any(Tab::has_live_terminal)
    }

    /// Open the tab that stands in for the session's emptied last tab: the default layout with
    /// the configured shell, built for `client_id` the way a `NewTab` action's would be. The
    /// emptied tabs are closed by `settle_tabs` once this one has applied its layout.
    fn open_replacement_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to open a replacement tab for client {client_id}");
        let tab_id = self.get_new_tab_id();
        let swap_layouts = (
            self.default_layout.swap_tiled_layouts.clone(),
            self.default_layout.swap_floating_layouts.clone(),
        );
        self.new_tab(tab_id, swap_layouts, None, Some(client_id))
            .with_context(err_context)?;
        self.replacement_tab = Some(tab_id);
        self.replacement_pending = true;
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::NewTab(
                None,
                self.default_shell_action.clone(),
                None,
                None,
                tab_id,
                None,
                false,
                false, // settlement moves clients only if this replacement is still needed
                client_id,
                None,
            ))
            .with_context(err_context)
    }

    pub(super) fn apply_deferred_tab_switches(
        &mut self,
        switches: &mut HashSet<(TabId, ClientId)>,
    ) -> Result<()> {
        for (tab_id, client_id) in switches.drain() {
            if let Some(position) = self.get_tab_position_by_id(tab_id) {
                self.navigate_to_tab(position, None, true, client_id)?;
            }
        }
        Ok(())
    }

    pub(super) fn replacement_exit_command(&self, pane_id: PaneId) -> Option<RunCommand> {
        let tab = self.tab_containing_pane(pane_id)?;
        if tab.stands_in_for_a_pane(pane_id).is_some() {
            return None;
        }
        if Some(tab.id) != self.replacement_tab && tab.multipane_info(pane_id).is_none() {
            return None;
        }
        if !matches!(pane_id, PaneId::Terminal(_)) || !tab.has_pane_with_pid(&pane_id) {
            return None;
        }
        tab.pane_run_command(pane_id)
            .or_else(|| match &self.default_shell_action {
                Some(TerminalAction::RunCommand(command)) => Some(command.clone()),
                _ => Some(RunCommand {
                    command: crate::pty::get_default_shell(),
                    ..Default::default()
                }),
            })
    }

    pub(super) fn request_close_tab(
        &mut self,
        tab_id: TabId,
        client_id: Option<ClientId>,
    ) -> Result<()> {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return Ok(());
        };
        let client_id = client_id.or_else(|| self.get_first_client_id());
        let has_destination = self
            .tabs
            .values()
            .any(|other| other.id != tab_id && !other.is_pending() && other.has_selectable_panes());
        if !has_destination {
            if self.quit_is_guarded() {
                if let Some(client_id) = client_id {
                    self.confirm_quit(client_id, true, None)?;
                }
                return Ok(());
            }
            if tab.has_hidden_live_terminal() || self.has_pending_tabs() {
                self.deferred_tab_closes.insert(tab_id);
                if !self.has_pending_tabs() {
                    if let Some(client_id) = client_id {
                        self.open_replacement_tab(client_id)?;
                    }
                }
                return Ok(());
            }
        }
        self.close_tab_by_id(tab_id)
    }

    pub(super) fn close_tab_by_id(&mut self, tab_id: TabId) -> Result<()> {
        // Keep clients and preserved panes attached until relocation has a loaded destination.
        // Pending sources still close immediately so failed layouts cannot block settlement.
        if self.tabs.get(&tab_id).is_some_and(|tab| !tab.is_pending())
            && self.tabs.len() > 1
            && self
                .tabs
                .values()
                .all(|tab| tab.id == tab_id || tab.is_pending())
        {
            self.deferred_tab_closes.insert(tab_id);
            return Ok(());
        }
        self.teardown_tab(tab_id, false)
    }

    fn teardown_tab(&mut self, tab_id: TabId, preserve_guards: bool) -> Result<()> {
        let err_context = || format!("failed to close tab at index {tab_id:?}");

        if self
            .pending_arrangement
            .as_ref()
            .is_some_and(|pending| pending.destinations.values().any(|id| *id == tab_id))
        {
            self.pending_arrangement = None;
            self.arrangement_notice(
                "Arrange panes: destination closed; arrangement cancelled".into(),
            );
        }
        let mut tab_to_close = self.tabs.remove(&tab_id).with_context(err_context)?;
        if self.departed_followed_tab_id == Some(tab_id) {
            self.departed_followed_tab_id = self.tabs.keys().next().copied();
            for watcher in self.watcher_clients.values_mut() {
                watcher.set_force_render();
            }
        }
        self.deferred_tab_closes.remove(&tab_id);
        // An adopted group whose tab closes before its layout applied is never
        // placed. Dropping the entry releases its warp hold.
        if let Some((_, _, _, Some(response), _)) = self.pending_group_placements.remove(&tab_id) {
            let _ = response.send(Err(anyhow!(
                "the tab for the adopted panes closed before they were placed"
            )));
        }
        self.pending_warp_relationships.remove(&tab_id);
        // Overlays keep their placement through the existing overlay relocation path.
        if preserve_guards {
            for (&plugin_id, notice) in &self.quit_notices {
                let id = PaneId::Plugin(plugin_id);
                if notice.guard && tab_to_close.overlay_placement_for(id).is_none() {
                    tab_to_close.suppress_pane(id, None);
                }
            }
        }
        let hidden_live: Vec<_> = tab_to_close
            .get_all_pane_ids()
            .into_iter()
            .filter(|id| {
                matches!(id, PaneId::Terminal(_))
                    && tab_to_close
                        .get_pane_with_id(*id)
                        .is_some_and(|pane| !pane.selectable() && !pane.exited())
            })
            .collect();
        for id in hidden_live {
            tab_to_close.suppress_pane(id, None);
        }
        let mut pane_ids = tab_to_close.get_all_pane_ids();

        // The same treatment, and for a stronger reason: an overlay is drawn over the whole
        // session surface rather than inside this tab, and agent keeps a session's worth of
        // conversation in one. Taken out here so that the teardown below - `PaneClosed`, the
        // subscriber notification, and the `CloseTab` that ends in `PluginInstruction::Unload` -
        // never names them, and re-homed further down once it is known where the clients went.
        //
        // Only when a tab survives: with none left the session is ending anyway (see the
        // `Render(None)` below), and an overlay extracted with nowhere to go would be a plugin
        // left running with no pane and nothing told about it.
        let mut overlay_panes = match self.tabs.is_empty() {
            true => vec![],
            false => tab_to_close.extract_overlay_panes(),
        };
        if preserve_guards {
            for (&plugin_id, notice) in &self.quit_notices {
                let id = PaneId::Plugin(plugin_id);
                if notice.guard && tab_to_close.pane_is_overlay(id) {
                    if let Some(container) = tab_to_close.take_overlay_container(id) {
                        overlay_panes.push(container);
                    }
                }
            }
        }
        // Close each chain whose stand-in stays in this tab. A hidden stand-in moves
        // with every pane behind it, preserving their restoration links. With no
        // destination left, every pane closes, including parked panes and overlays.
        let mut suppressed_panes = tab_to_close.extract_suppressed_panes();
        let mut closing: HashSet<_> = if self.tabs.is_empty() {
            pane_ids.iter().copied().collect()
        } else {
            tab_to_close.get_all_pane_ids().into_iter().collect()
        };
        loop {
            let previous_count = closing.len();
            for (id, suppressed) in &suppressed_panes {
                if suppressed
                    .restored_by
                    .is_some_and(|stand_in| closing.contains(&stand_in.pane_id))
                {
                    closing.insert(*id);
                }
            }
            if closing.len() == previous_count {
                break;
            }
        }
        suppressed_panes.retain(|id, _| !closing.contains(id));
        pane_ids.retain(|id| closing.contains(id));
        let suppressed_respawns = suppressed_panes
            .keys()
            .filter_map(|pane_id| {
                tab_to_close
                    .take_pending_respawn(*pane_id)
                    .map(|entry| (*pane_id, entry))
            })
            .collect();
        // The plugins going down with the tab take what they had to say about a quit with them,
        // as they do when closed one at a time (`ScreenInstruction::ClosePane`). Left behind, a
        // guard nothing could lift would keep the session from ever ending.
        for p_id in &pane_ids {
            self.cleanup_closed_pane(*p_id);
        }
        // Nothing should be able to aim at this tab again: `toggle_tab` reads the history, and
        // an arrangement's memory of the project here would otherwise outlive the tab.
        for history in self.tab_history.values_mut() {
            history.retain(|id| *id != tab_id);
        }
        self.arrangement_projects.remove(&tab_id);
        if self.replacement_tab == Some(tab_id) {
            self.replacement_tab = None;
        }

        let _ = self.bus.senders.send_to_plugin(PluginInstruction::Update(
            pane_ids
                .iter()
                .copied()
                .map(|p_id| (None, None, Event::PaneClosed(p_id)))
                .collect(),
        ));

        // Notify pane render subscribers of each closed pane
        for p_id in &pane_ids {
            self.notify_pane_closed_to_subscribers(*p_id);
        }

        // below we don't check the result of sending the CloseTab instruction to the pty thread
        // because this might be happening when the app is closing, at which point the pty thread
        // has already closed and this would result in an error
        self.bus
            .senders
            .send_to_pty(PtyInstruction::CloseTab(pane_ids))
            .with_context(err_context)
            .non_fatal();
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
            self.move_overlay_panes_from_closed_tab(overlay_panes, overlay_client);
            // Belt and braces, and it has to come before the visibility pass below.
            // `move_clients_from_closed_tab` only ever hears about the clients
            // `drain_connected_clients` handed it, so a client the closing tab did not count as
            // connected would keep an `active_tab_ids` entry naming a tab that no longer exists -
            // and every later instruction for it would be dropped by `active_tab!`, with no frame
            // ever rendered for it either. `Tab::add_client` is total now, so this should find
            // nothing; it says so loudly if it does.
            //
            // Before `visible_tab_indices`, because that set is what tells a tab it is on screen
            // and what sizes it: a client re-homed after it was computed would land in a tab whose
            // plugins are still holding the `visible(false)` `Screen::remove_client` gave them when
            // its last client left, and nothing would correct that until the next tab switch.
            self.reconcile_client_tabs().with_context(err_context)?;
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
            self.render().with_context(err_context)
        }
    }

    /// Put right any client whose tab and whose tab's client list disagree.
    ///
    /// The two halves have to hold together: `Screen::active_tab_ids` says which tab a client
    /// sees and types into, and `Tab::connected_clients` is what that tab renders for and routes
    /// input to. A client that names a tab which has been removed, or one that does not count
    /// it, receives no frames and reaches nothing - and nothing repairs either state short of a
    /// reattach, which is what made it look like the whole session had hung.
    ///
    /// `move_clients_from_closed_tab` only ever hears about the clients
    /// `drain_connected_clients` handed it, so this is the backstop for the ones it cannot see.
    /// With `Tab::add_client` total it should find nothing; it says so loudly if it does.
    fn reconcile_client_tabs(&mut self) -> Result<()> {
        let stranded: Vec<ClientId> = self
            .active_tab_ids
            .iter()
            .filter(|(client_id, tab_id)| {
                self.tabs
                    .get(tab_id)
                    .map(|tab| !tab.has_connected_client(client_id))
                    .unwrap_or(true)
            })
            .map(|(client_id, _)| *client_id)
            .collect();
        if stranded.is_empty() {
            return Ok(());
        }
        log::error!("clients {stranded:?} were left with no tab to render into; re-homing them");
        // The mode the client is actually in, not the default: it was stranded mid-session and
        // may well have been in Scroll or Locked, and `Screen::mode_info` is the map
        // `change_mode` writes on every change. The default is only for a client with no entry.
        let mode_infos: Vec<(ClientId, ModeInfo)> = stranded
            .into_iter()
            .map(|client_id| {
                let mode_info = self
                    .mode_info
                    .get(&client_id)
                    .cloned()
                    .unwrap_or_else(|| self.default_mode_info.clone());
                (client_id, mode_info)
            })
            .collect();
        self.move_clients_from_closed_tab(mode_infos)
    }

    // Closes the client_id's focused tab
    pub fn close_tab(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to close tab for client {client_id:?}");

        let client_id = self.client_to_act_for(client_id);

        match client_id {
            Some(client_id) => {
                let active_tab_index = *self
                    .active_tab_ids
                    .get(&client_id)
                    .with_context(err_context)?;
                self.request_close_tab(active_tab_index, Some(client_id))
                    .with_context(err_context)
            },
            None => Ok(()),
        }
    }
}
