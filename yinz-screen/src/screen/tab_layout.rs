use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::thread_main::{apply_pane_agent_report, replay_pending_pane_event, PendingEvents};
use super::{replay_pending_plugin_events, Screen};
use crate::background_jobs::BackgroundJob;
use crate::panes::PaneId;
use crate::plugins::PluginId;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::{ClientId, ServerInstruction};
use std::collections::HashMap;
use yinz_bus::screen::{HoldForCommand, TabOverrideResult};
use yinz_types::agent::AgentInfo;
use yinz_types::input::command::RunCommand;
use yinz_types::input::layout::{FloatingPaneLayout, RunPluginOrAlias, TiledPaneLayout};
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_size::{Size, Viewport};

impl Screen {
    /// Validate the whole override before replacing any panes or closing surplus tabs.
    pub(super) fn complete_override_layout(
        &mut self,
        tab_results: Vec<TabOverrideResult>,
        failed_tabs: Vec<TabId>,
        tabs_to_close: Vec<TabId>,
        retain_existing_terminal_panes: bool,
        retain_existing_plugin_panes: bool,
        client_id: ClientId,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        let mut result = if failed_tabs.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("Failed to spawn layout for tabs {failed_tabs:?}"))
        };
        for tab_result in &tab_results {
            if result.is_err() {
                break;
            }
            if !self.tabs.contains_key(&tab_result.tab_index) {
                result = self.new_tab(
                    tab_result.tab_index,
                    (
                        tab_result.swap_tiled_layouts.clone().unwrap_or_default(),
                        tab_result.swap_floating_layouts.clone().unwrap_or_default(),
                    ),
                    tab_result.tab_name.clone(),
                    None,
                );
            }
            if result.is_ok() {
                result = self
                    .tabs
                    .get_mut(&tab_result.tab_index)
                    .context("override tab is missing")
                    .and_then(|tab| {
                        tab.validate_override_layout(
                            &tab_result.tiled_layout,
                            &tab_result.floating_layouts,
                            &tab_result.new_terminal_pids,
                            &tab_result.new_floating_pane_pids,
                            &tab_result.plugin_ids,
                            retain_existing_terminal_panes,
                            retain_existing_plugin_panes,
                            client_id,
                        )
                    })
                    .with_context(|| {
                        format!("Failed to override layout for tab {}", tab_result.tab_index)
                    });
            }
        }
        let reserved_tabs: Vec<_> = tab_results
            .iter()
            .map(|tab| tab.tab_index)
            .chain(failed_tabs)
            .collect();
        let group_layouts: Vec<_> = tab_results
            .iter()
            .filter(|tab| tab.tiled_layout.tab_group.is_some())
            .map(|tab| {
                (
                    tab.tab_index,
                    TiledPaneLayout {
                        tab_group: tab.tiled_layout.tab_group.clone(),
                        group_focused: tab.tiled_layout.group_focused,
                        ..Default::default()
                    },
                )
            })
            .collect();
        for tab_result in tab_results {
            let spawned_panes: Vec<_> = tab_result
                .new_terminal_pids
                .iter()
                .chain(&tab_result.new_floating_pane_pids)
                .map(|(id, _)| PaneId::Terminal(*id))
                .chain(
                    tab_result
                        .plugin_ids
                        .values()
                        .flatten()
                        .copied()
                        .map(PaneId::Plugin),
                )
                .collect();
            if result.is_ok() {
                result = self
                    .tabs
                    .get_mut(&tab_result.tab_index)
                    .context("override tab is missing")
                    .and_then(|tab| {
                        tab.override_layout(
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
                        )?;
                        if let Some(name) = tab_result.tab_name {
                            tab.name = name;
                        }
                        Ok(())
                    });
            }
            if result.is_err() {
                self.discard_spawned_layout_panes(spawned_panes, pending);
            }
        }
        if result.is_ok() {
            for tab_id in tabs_to_close {
                if self.tabs.contains_key(&tab_id) {
                    self.close_tab_by_id(tab_id)?;
                }
            }
        } else {
            for tab_id in reserved_tabs {
                self.recover_pending_tab(tab_id)?;
            }
        }

        self.replay_events_waiting_for_layout(pending);

        if !self.has_pending_tabs() {
            self.apply_deferred_tab_switches(&mut pending.tab_switches)?;
        }

        if result.is_ok() {
            for (tab, layout) in group_layouts {
                self.restore_tab_group(tab, &layout);
            }
            self.finish_group_restore();
        }

        result
    }

    /// Pending tabs can already hold panes moved or adopted before their layout arrives.
    pub(super) fn recover_pending_tab(&mut self, tab_id: TabId) -> Result<()> {
        if let Some(tab) = self.tabs.get_mut(&tab_id).filter(|tab| tab.is_pending()) {
            tab.fail_pending_agent_placements(
                "destination layout failed; inspect the retained pane and tab before retrying",
            );
            if tab.get_all_pane_ids().is_empty() {
                self.close_tab_by_id(tab_id)?;
            } else {
                tab.finish_pending_layout();
            }
        }
        Ok(())
    }

    pub(super) fn replay_events_waiting_for_layout(&mut self, pending: &mut PendingEvents) {
        for event in std::mem::take(&mut pending.waiting_for_client) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }

        for event in std::mem::take(&mut pending.waiting_for_tab) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }
    }

    pub(super) fn report_layout_error(
        &self,
        message: String,
        client_id: ClientId,
        completion_tx: &mut Option<NotificationEnd>,
    ) {
        log::error!("{message}");
        if let Some(completion) = completion_tx.as_mut() {
            completion.set_exit_status(1);
            completion.set_error_message(message.clone());
        }
        if let Some(pane_id) = self.get_active_pane_id(&client_id) {
            self.bus
                .senders
                .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                    vec![pane_id],
                    message.clone(),
                ))
                .context("failed to display rejected layout error")
                .non_fatal();
        }
        self.bus
            .senders
            .send_to_server(ServerInstruction::LogError(vec![message], client_id, None))
            .context("failed to report rejected layout")
            .non_fatal();
    }

    fn discard_spawned_layout_panes(&mut self, pane_ids: Vec<PaneId>, pending: &mut PendingEvents) {
        for pane_id in pane_ids {
            if let Some(tab) = self
                .tabs
                .values_mut()
                .find(|tab| tab.has_pane_with_pid(&pane_id))
            {
                tab.extract_pane(pane_id, true);
            }
            pending.waiting_for_pane.remove(&pane_id);
            self.bus
                .senders
                .send_to_pty(PtyInstruction::ClosePane(pane_id, None))
                .context("failed to close pane from rejected layout")
                .non_fatal();
        }
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
        adopted_terminals: Vec<(TerminalId, RunCommand, Option<AgentInfo>)>,
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
        if completion_tx
            .as_ref()
            .and_then(|c| c.agent_tab.as_ref())
            .is_some_and(|target| {
                !self.tabs.get(&tab_id).is_some_and(|tab| {
                    target
                        .lifetime
                        .ptr_eq(&std::sync::Arc::downgrade(&tab.agent_tab_lifetime))
                })
            })
        {
            let panes = new_pane_pids
                .iter()
                .chain(&new_floating_pane_pids)
                .map(|(id, _)| PaneId::Terminal(*id))
                .chain(
                    new_plugin_ids
                        .values()
                        .flatten()
                        .copied()
                        .map(PaneId::Plugin),
                )
                .collect();
            self.discard_spawned_layout_panes(panes, pending);
            return Ok(());
        }
        // tab_id is a stable identifier from NewTab instruction
        if let Some(first_terminal_pane) = new_pane_pids.first() {
            if let Some(c) = completion_tx.as_mut() {
                c.set_affected_pane_id(PaneId::Terminal(first_terminal_pane.0))
            }
        } else if let Some(plugin_id) = new_plugin_ids.values().next().and_then(|v| v.first())
            && let Some(c) = completion_tx.as_mut()
        {
            c.set_affected_pane_id(PaneId::Plugin(*plugin_id))
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
        let adopted_commands: HashMap<_, _> = adopted_terminals
            .iter()
            .map(|(id, command, _)| (*id, command))
            .collect();
        let retained_terminals: Vec<_> = new_pane_pids
            .iter()
            .chain(&new_floating_pane_pids)
            .filter_map(|(id, held_command)| {
                adopted_commands
                    .get(id)
                    .map(|&command| (*id, command.clone(), held_command.clone()))
            })
            .collect();
        let spawned_panes: Vec<_> = new_terminal_pane_ids
            .iter()
            .copied()
            .filter(|id| !matches!(id, PaneId::Terminal(id) if adopted_commands.contains_key(id)))
            .chain(
                new_plugin_ids
                    .values()
                    .flatten()
                    .copied()
                    .map(PaneId::Plugin),
            )
            .collect();
        let applied = self.apply_layout(
            layout,
            floating_panes_layout,
            new_pane_pids,
            new_floating_pane_pids,
            new_plugin_ids.clone(),
            tab_id,
            should_change_focus_to_new_tab,
            client_id,
            blocking_terminal,
        );
        if let Ok(Some(source)) = &applied {
            pending.remove_client_plugin_output(*source);
        }
        let layout_failed = applied.is_err();
        if let Err(error) = applied {
            let mut message = format!("Failed to apply layout for tab {tab_id}: {error:#}");
            if let Some(completion) = completion_tx.as_mut() {
                completion.set_exit_status(1);
                completion.set_error_message(message.clone());
            }
            self.discard_spawned_layout_panes(spawned_panes, pending);
            let retained = if !retained_terminals.is_empty() {
                let retention = (|| {
                    if !self.tabs.contains_key(&tab_id) {
                        self.new_tab(tab_id, (vec![], vec![]), None, None)?;
                    }
                    self.tabs
                        .get_mut(&tab_id)
                        .context("missing adopted-pane destination")?
                        .retain_adopted_layout_terminals(retained_terminals, client_id)
                })();
                match retention {
                    Ok(()) => {
                        message.push_str(
                            "; transferred terminals were retained in their destination tab",
                        );
                        true
                    },
                    Err(error) => {
                        message.push_str(&format!(
                            "; could not finish retaining transferred terminals: {error:#}"
                        ));
                        adopted_commands.keys().all(|id| {
                            self.tabs
                                .values()
                                .any(|tab| tab.has_pane_with_pid(&PaneId::Terminal(*id)))
                        })
                    },
                }
            } else {
                false
            };
            self.recover_pending_tab(tab_id).non_fatal();
            if let Some((_, _, _, Some(response), _)) =
                self.pending_group_placements.remove(&tab_id)
            {
                // Receipt is successful when the recovery tab owns the transferred supervisors;
                // the source must not roll those terminals back after a rejected layout.
                let _ = response.send(if retained {
                    Ok(())
                } else {
                    Err(anyhow!(message.clone()))
                });
            }
            self.pending_warp_relationships.remove(&tab_id);
            self.report_layout_error(message, client_id, &mut completion_tx);
        }
        for (terminal_id, _, agent) in adopted_terminals {
            let Some(agent) = agent else {
                continue;
            };
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
            if should_change_focus_to_new_tab
                && !layout_failed
                && let Some(tab_position) = self.get_tab_position_by_id(tab_id)
            {
                self.switch_active_tab(tab_position, None, true, client_id)?;
            }
        } else if should_change_focus_to_new_tab && !layout_failed {
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
                let plugin_exists = self.tabs.values().any(|tab| tab.has_plugin(*plugin_id));
                if plugin_exists {
                    self.render_blocker.register_blocking_plugin(*plugin_id);
                    if let Some(pending_events) = pending.waiting_for_pane.remove(&pane_id) {
                        replay_pending_plugin_events(self, pending_events)?;
                    }
                }
            }
        }

        self.replay_events_waiting_for_layout(pending);

        Ok(())
    }

    /// Every tab shares the smallest interactive client bounds, regardless of which tab each
    /// client is viewing. Watchers and fit-disabled mobile clients are already handled by
    /// `session_size`.
    pub(super) fn tab_surface_size(&self, _tab_id: TabId) -> Option<Size> {
        self.session_size()
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
        if let Some(tab) = self.tabs.get_mut(&tab_id)
            && tab.get_display_area() != tab_size
        {
            tab.resize_whole_tab_to(tab_size)?;
            tab.set_force_render();
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
