use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::thread_main::{apply_pane_agent_report, replay_pending_pane_event, PendingEvents};
use super::{replay_pending_plugin_events, Screen};
use crate::panes::PaneId;
use crate::plugins::PluginId;
use crate::route::NotificationEnd;
use crate::tab::Tab;
use crate::ClientId;
use std::collections::HashMap;
use yinz_bus::screen::{HoldForCommand, TabOverrideResult};
use yinz_types::agent::AgentInfo;
use yinz_types::input::layout::{FloatingPaneLayout, RunPluginOrAlias, TiledPaneLayout};
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_size::{Size, Viewport};

impl Screen {
    /// The pty thread has spawned every tab of an override layout: apply each tab's result
    /// (creating tabs that do not exist yet), close the tabs whose terminals could not be
    /// spawned, and replay what was waiting for a client or a tab.
    pub(super) fn complete_override_layout(
        &mut self,
        tab_results: Vec<TabOverrideResult>,
        failed_tabs: Vec<TabId>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        // Process each tab result
        for tab_result in tab_results {
            if let Some(tab) = self.tabs.get_mut(&tab_result.tab_index) {
                if let Err(e) = tab.override_layout(
                    tab_result.tiled_layout,
                    tab_result.floating_layouts,
                    tab_result.swap_tiled_layouts.clone(),
                    tab_result.swap_floating_layouts.clone(),
                    tab_result.new_terminal_pids,
                    tab_result.new_floating_pane_pids,
                    tab_result.plugin_ids,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    client_id,
                    None,
                ) {
                    log::error!(
                        "Failed to override layout for tab {}: {:?}",
                        tab_result.tab_index,
                        e
                    );
                }
            } else {
                // Tab doesn't exist - create it
                let swap_layouts = (
                    tab_result.swap_tiled_layouts.clone().unwrap_or_default(),
                    tab_result.swap_floating_layouts.clone().unwrap_or_default(),
                );
                if let Err(e) = self.new_tab(
                    tab_result.tab_index,
                    swap_layouts,
                    tab_result.tab_name.clone(),
                    None,
                ) {
                    log::error!(
                        "Failed to create new tab {} during override completion: {:?}",
                        tab_result.tab_index,
                        e
                    );
                    continue;
                }

                // Now override the newly created tab's layout
                if let Some(tab) = self.tabs.get_mut(&tab_result.tab_index) {
                    if let Err(e) = tab.override_layout(
                        tab_result.tiled_layout,
                        tab_result.floating_layouts,
                        tab_result.swap_tiled_layouts,
                        tab_result.swap_floating_layouts,
                        tab_result.new_terminal_pids,
                        tab_result.new_floating_pane_pids,
                        tab_result.plugin_ids,
                        retain_existing_terminal_panes,
                        retain_existing_plugin_panes,
                        client_id,
                        None,
                    ) {
                        log::error!(
                            "Failed to override layout for new tab {}: {:?}",
                            tab_result.tab_index,
                            e
                        );
                    }
                }
            }
        }

        for tab_id in failed_tabs {
            if self.tabs.get(&tab_id).is_some_and(Tab::is_pending) {
                self.close_tab_by_id(tab_id)?;
            }
        }

        for event in std::mem::take(&mut pending.waiting_for_client) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }

        for event in std::mem::take(&mut pending.waiting_for_tab) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }

        if !self.has_pending_tabs() {
            self.apply_deferred_tab_switches(&mut pending.tab_switches)?;
        }

        // Single render and log after all tabs are updated (performance optimization)
        Ok(())
    }

    /// The pty thread has spawned a `NewTab`'s panes: apply the layout to the tab, restore any
    /// adopted agents' records, switch to the tab now or once every pending tab has landed, and
    /// replay what was waiting for the new panes, the client and the tab.
    pub(super) fn land_layout(
        &mut self,
        layout: TiledPaneLayout,
        floating_panes_layout: Vec<FloatingPaneLayout>,
        new_pane_pids: Vec<(TerminalId, HoldForCommand)>,
        new_floating_pane_pids: Vec<(TerminalId, HoldForCommand)>,
        adopted_agents: Vec<(TerminalId, AgentInfo)>,
        new_plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>>,
        tab_id: TabId,
        should_change_focus_to_new_tab: bool,
        client_id: ClientId,
        mut completion_tx: Option<NotificationEnd>,
        blocking_terminal: Option<(TerminalId, NotificationEnd)>,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        log::info!(
            "ScreenInstruction::ApplyLayout: applying layout for tab {}",
            tab_id
        );
        // tab_id is a stable identifier from NewTab instruction
        if let Some(first_terminal_pane) = new_pane_pids.first() {
            if let Some(c) = completion_tx.as_mut() {
                c.set_affected_pane_id(PaneId::Terminal(first_terminal_pane.0))
            }
        } else if let Some(plugin_id) = new_plugin_ids.values().next().and_then(|v| v.first()) {
            if let Some(c) = completion_tx.as_mut() {
                c.set_affected_pane_id(PaneId::Plugin(*plugin_id))
            }
        }
        // Set the affected tab ID for plugin API return value
        if let Some(c) = completion_tx.as_mut() {
            c.set_affected_tab_id(tab_id)
        }
        let new_terminal_pane_ids: Vec<_> = new_pane_pids
            .iter()
            .chain(&new_floating_pane_pids)
            .map(|(terminal_id, _)| PaneId::Terminal(*terminal_id))
            .collect();
        self.apply_layout(
            layout,
            floating_panes_layout,
            new_pane_pids,
            new_floating_pane_pids,
            new_plugin_ids.clone(),
            tab_id,
            should_change_focus_to_new_tab,
            client_id,
            blocking_terminal,
        )?;
        for (terminal_id, agent) in adopted_agents {
            let pane_id = PaneId::Terminal(terminal_id);
            if !apply_pane_agent_report(self, pane_id, Some(agent))? {
                Err::<(), _>(anyhow!(
                    "adopted agent record refers to missing terminal pane {terminal_id}"
                ))
                .non_fatal();
            }
        }
        if !self.has_pending_tabs() {
            self.apply_deferred_tab_switches(&mut pending.tab_switches)?;
            if should_change_focus_to_new_tab {
                if let Some(tab_position) = self.get_tab_position_by_id(tab_id) {
                    self.switch_active_tab(tab_position, None, true, client_id)?;
                }
            }
        } else if should_change_focus_to_new_tab {
            let client_id_to_switch = if self.active_tab_ids.contains_key(&client_id) {
                Some(client_id)
            } else {
                self.active_tab_ids.keys().next().copied()
            };
            if let Some(client_id_to_switch) = client_id_to_switch {
                pending.tab_switches.insert((tab_id, client_id_to_switch));
            }
        }

        for pane_id in new_terminal_pane_ids {
            if let Some(pending_events) = pending.waiting_for_pane.remove(&pane_id) {
                for event in pending_events {
                    replay_pending_pane_event(self, event, pending).non_fatal();
                }
            }
        }

        for plugin_ids in new_plugin_ids.values() {
            for plugin_id in plugin_ids {
                let pane_id = PaneId::Plugin(*plugin_id);
                self.render_blocker.register_blocking_plugin(*plugin_id);
                let plugin_exists = self.tabs.values().any(|tab| tab.has_plugin(*plugin_id));
                if plugin_exists {
                    if let Some(pending_events) = pending.waiting_for_pane.remove(&pane_id) {
                        replay_pending_plugin_events(self, pending_events)?;
                    }
                }
            }
        }

        for event in std::mem::take(&mut pending.waiting_for_client) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }

        for event in std::mem::take(&mut pending.waiting_for_tab) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }

        Ok(())
    }

    /// Every tab shares the smallest interactive client bounds, regardless of which tab each
    /// client is viewing. Watchers and fit-disabled mobile clients are already handled by
    /// `session_size`.
    pub(super) fn tab_surface_size(&self, _tab_id: TabId) -> Option<Size> {
        self.session_size()
    }

    /// How much of a watcher's viewport the followed client's frame covers, so
    /// `serialize_with_size` clears only what is beyond it.
    pub(super) fn followed_frame_size(&self, tab_id: TabId) -> Option<Size> {
        if !self.tabs.contains_key(&tab_id) {
            return None;
        }
        Some(
            self.tab_surface_size(tab_id)
                .unwrap_or_else(|| self.surface_size()),
        )
    }

    /// Lay every tab out over the shared session surface, and record that surface as the one the
    /// tabs now sit on.
    ///
    /// The recording belongs here rather than at the debounced render that follows: a client that
    /// attaches and detaches inside that window would otherwise leave `last_surface_size` on its
    /// pre-attach value, and `surface_size` / `size_for_client` stand in with it once every client
    /// has detached - so the next render tick would snap the tabs back, and a `yinz action
    /// new-tab` would build a tab the reattaching client cannot match.
    pub(super) fn reflow_all_tabs(&mut self) -> Result<()> {
        for tab_id in self.tabs.keys().copied().collect::<Vec<_>>() {
            self.reflow_tab(tab_id)?;
        }
        self.remember_surface_size();
        Ok(())
    }

    /// Lay tab `tab_id` out over the shared session surface.
    ///
    /// With no interactive client there is no size to derive, so the tab keeps its last layout.
    pub(super) fn reflow_tab(&mut self, tab_id: TabId) -> Result<()> {
        let Some(tab_size) = self.tab_surface_size(tab_id) else {
            return Ok(());
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.get_display_area() != tab_size {
                tab.resize_whole_tab_to(tab_size)?;
                tab.set_force_render();
            }
        }
        Ok(())
    }

    /// The whole session surface tab `tab_id` sits on. A tab is laid out over the entire surface,
    /// so this is its display area at the screen origin.
    pub(super) fn session_surface_for_tab(&self, tab_id: TabId) -> Option<Viewport> {
        let tab = self.tabs.get(&tab_id)?;
        let size = self
            .tab_surface_size(tab_id)
            .unwrap_or_else(|| tab.get_display_area());
        Some(Viewport {
            x: 0,
            y: 0,
            cols: size.cols,
            rows: size.rows,
        })
    }
}
