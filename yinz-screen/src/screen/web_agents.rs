//! Live snapshots and focus share the palette's population, presentation and action rules.
use super::command_palette::history::LiveAgentTab;
use super::Screen;
use std::collections::HashMap;
use std::time::Instant;
use yinz_bus::command_palette::HistoryHeader;
use yinz_bus::web::{
    MoveDestination, MoveDestinations, WebAgentRef, WebFocusError, WebGroup, WebLiveAgent,
    WebLiveHeader, WebManageAction, WebTab,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{peer_web_detail_id, valid_web_peer_instance};
use yinz_types::internal_ids::{ClientId, SessionName, TabId};
use yinz_types::pane_id::PaneId;

impl Screen {
    pub(super) fn web_live_agents(&mut self, reply: crossbeam::channel::Sender<Vec<WebLiveAgent>>) {
        let instance = self.core.instance_id.to_string();
        let mut local_groups = HashMap::new();
        let agents =
            self.live_agents_with_tabs()
                .into_iter()
                .filter_map(|(live, tab)| {
                    let presentation = live.presentation()?;
                    let current_session = matches!(tab, LiveAgentTab::Local(_));
                    let (tab_group_session, tab_group, peer_instance) = match tab {
                        LiveAgentTab::Local(tab) => (
                            &self.core.session_name,
                            self.core.tab_group_ids.get(&tab).and_then(|&id| {
                                local_groups
                                    .entry(id)
                                    .or_insert_with(|| self.tab_group_info(tab))
                                    .clone()
                            }),
                            None,
                        ),
                        // A renamed socket can be discovered before the checkpoint name changes.
                        LiveAgentTab::Peer {
                            socket_name,
                            session,
                            position,
                        } if session.web_peer_instance.as_deref() != Some(instance.as_str()) => (
                            socket_name,
                            session
                                .tabs
                                .iter()
                                .find(|tab| tab.position == position)
                                .and_then(|tab| tab.group.clone()),
                            session
                                .web_peer_instance
                                .as_deref()
                                .filter(|value| valid_web_peer_instance(value))
                                .map(str::to_owned),
                        ),
                        LiveAgentTab::Peer { .. } => return None,
                    };
                    let (glyph, status, _) =
                        yinz_panes::ui::agent_title::status(presentation.state);
                    let mut header = HistoryHeader::default();
                    live.header_overlay(&mut header, None);
                    Some(WebLiveAgent {
                        id: live.id(),
                        session: live.session.clone(),
                        tab_group_session: tab_group_session.clone(),
                        tab_group,
                        local_tab: match tab {
                            LiveAgentTab::Local(id) => self.web_tab(id),
                            _ => None,
                        },
                        current_session,
                        peer_instance,
                        run: live.run(),
                        label_source: live.label_source(),
                        provider: presentation.provider,
                        activity: presentation.activity,
                        host: presentation.host,
                        pane_label: presentation.pane_id,
                        glyph,
                        status,
                        focused: current_session
                            && self.core.active_tab_ids.keys().any(|client| {
                                self.get_active_pane_id(client) == Some(live.pane.id)
                            }),
                        selectable: live.pane.is_selectable,
                        suppressed: live.pane.is_suppressed,
                        usage_unavailable: live.usage_unavailable(&self.core.session_name),
                        details: live.text(),
                        header: WebLiveHeader {
                            name: header.name,
                            status: header.status,
                            location: header.location,
                            task: header.task,
                            project: header.project,
                            session_id: live
                                .metadata
                                .get("agent-session")
                                .map(|id| format!("session {id}")),
                        },
                        pane: live.pane,
                        metadata: live.metadata,
                    })
                })
                .collect();
        reply
            .send(agents)
            .map_err(|_| anyhow!("web reply receiver dropped"))
            .non_fatal();
    }

    fn web_tab(&self, id: TabId) -> Option<WebTab> {
        let tab = self.core.tabs.get(&id)?;
        Some(WebTab {
            id,
            name: tab.name.clone(),
            incarnation: tab.agent_tab_incarnation.clone(),
        })
    }

    fn web_move_destination_tab(&self, id: TabId) -> Option<&crate::tab::Tab> {
        self.core
            .tabs
            .get(&id)
            .filter(|tab| !tab.is_pending() && !self.core.deferred_tab_closes.contains(&id))
    }

    /// Resolve only local panes, and perform the guarded operation in this same screen turn.
    pub(super) fn web_manage_agent(
        &mut self,
        agent: &WebAgentRef,
        action: WebManageAction,
        client: ClientId,
        deadline: Instant,
    ) -> Result<Option<MoveDestinations>, WebFocusError> {
        let mut matches = self
            .live_agents_with_tabs()
            .into_iter()
            .filter_map(|(live, tab)| {
                let LiveAgentTab::Local(source) = tab else {
                    return None;
                };
                agent
                    .matches_agent(&live.id(), live.pane.agent.as_ref()?)
                    .then_some((live.pane.id, source))
            });
        let (pane, source) = matches.next().ok_or(WebFocusError::TargetChanged)?;
        if matches.next().is_some() {
            return Err(WebFocusError::TargetChanged);
        }
        drop(matches);
        if Instant::now() >= deadline {
            return Err(WebFocusError::Unavailable(
                "Agent request expired. Refresh and try again.".into(),
            ));
        }
        let destination = match action {
            WebManageAction::Destinations => {
                let current_group = *self
                    .core
                    .tab_group_ids
                    .get(&source)
                    .ok_or(WebFocusError::TargetChanged)?;
                let groups = self
                    .core
                    .tab_groups
                    .keys()
                    .filter_map(|&id| {
                        Some(WebGroup {
                            id,
                            name: self.tab_group_name(id)?,
                            tabs: self
                                .group_tabs(id)
                                .into_iter()
                                .filter(|id| self.web_move_destination_tab(*id).is_some())
                                .filter_map(|id| self.web_tab(id))
                                .collect(),
                        })
                    })
                    .collect();
                return Ok(Some(MoveDestinations {
                    groups,
                    current_group,
                    current_tab: source,
                    allow_new_group: self.tab_groups_enabled(),
                }));
            },
            WebManageAction::Close => {
                if !self.core.tabs[&source].explicit_close_is_single_pane(pane)
                    || self.settlement_after_close_risks_other_panes(source, pane)
                {
                    return Err(WebFocusError::Unavailable(
                        "Closing this agent would also close other panes.".into(),
                    ));
                }
                None
            },
            WebManageAction::Move(destination) => Some(destination),
        };
        for &client in self.core.connected_clients.borrow().iter() {
            if !self.core.watcher_clients.contains_key(&client) {
                let previous = (
                    self.active_pane_id(client),
                    self.active_pane_is_scrolled(client),
                );
                self.core
                    .focus_before_web_mutation
                    .entry(client)
                    .or_insert(previous);
            }
        }
        if let Some(destination) = destination {
            self.web_move_agent(pane, source, destination, client)?;
        } else {
            self.close_pane_explicitly(pane, None);
            self.log_and_report_session_state().map_err(web_failed)?;
        }
        self.render().map_err(web_failed)?;
        Ok(None)
    }

    pub(super) fn reconcile_web_focus_after_settlement(&mut self) {
        for (client, previous) in std::mem::take(&mut self.core.focus_before_web_mutation) {
            let Ok(tab) = self.get_active_tab(client) else {
                continue;
            };
            if tab.is_pending()
                || self.core.deferred_tab_closes.contains(&tab.id)
                || (!tab.has_selectable_panes() && self.has_pending_tabs())
            {
                self.core.focus_before_web_mutation.insert(client, previous);
                continue;
            }
            let (previous_pane, was_scrolled) = previous;
            let active_pane = tab.get_active_pane_id(client);
            let focus_changed = previous_pane != active_pane;
            let scroll_changed = was_scrolled != tab.active_pane_is_scrolled(client);
            if let Some(current) = active_pane
                && (focus_changed
                    || self.nested.dimmed_clients.contains(&client)
                        != self.should_route_keys_to_pane(client, current))
            {
                self.report_key_passthrough_state(
                    client,
                    previous_pane.unwrap_or(current),
                    current,
                );
            }
            if focus_changed || scroll_changed {
                self.sync_scroll_mode_on_focus(client).non_fatal();
            }
        }
    }

    fn web_move_agent(
        &mut self,
        pane: PaneId,
        source: TabId,
        destination: MoveDestination,
        client: ClientId,
    ) -> Result<(), WebFocusError> {
        let unavailable = || {
            WebFocusError::Unavailable(
                "Destination changed. Refresh destinations and choose again.".into(),
            )
        };
        let source_tab = &self.core.tabs[&source];
        if source_tab.is_pending() {
            return Err(WebFocusError::Unavailable(
                "Source tab is still loading.".into(),
            ));
        }
        let owner = source_tab.pane_owner_for_extraction(pane);
        let placement = source_tab.placement_for_extraction(owner);
        let restore_source = source_tab.stands_in_for_a_pane(owner).is_some();
        if restore_source
            && source_tab.multipane_info(owner).is_some()
            && !matches!(destination, MoveDestination::Tab { tab, .. } if tab == source)
        {
            return Err(WebFocusError::Unavailable(
                "Cannot move an in-place agent out of a multipane overlay.".into(),
            ));
        }
        let created = !matches!(destination, MoveDestination::Tab { .. });
        let target = match destination {
            MoveDestination::Tab {
                group,
                tab,
                incarnation,
            } => {
                if self.core.tab_group_ids.get(&tab) != Some(&group)
                    || !self
                        .web_move_destination_tab(tab)
                        .is_some_and(|dest| dest.agent_tab_incarnation == incarnation)
                {
                    return Err(unavailable());
                }
                if tab == source {
                    return Ok(());
                }
                tab
            },
            new => {
                let (group, group_name, tab_name, new_group) = match new {
                    MoveDestination::NewTab { group, name }
                        if self.core.tab_groups.contains_key(&group) =>
                    {
                        (group, None, name, false)
                    },
                    MoveDestination::NewGroup { name, tab_name } if self.tab_groups_enabled() => (
                        self.next_group_id().map_err(web_failed)?,
                        name,
                        tab_name,
                        true,
                    ),
                    _ => return Err(unavailable()),
                };
                let invoked = source_tab
                    .get_pane_with_id(owner)
                    .ok_or(WebFocusError::TargetChanged)?
                    .invoked_with()
                    .clone();
                let tab = self
                    .new_tab_for_adopted_pane(invoked, client, false)
                    .map_err(web_failed)?;
                self.core
                    .tabs
                    .get_mut(&tab)
                    .context("missing web move destination")
                    .map_err(web_failed)?
                    .background_layout = true;
                if new_group {
                    self.start_tab_group(group, tab, group_name.filter(|s| !s.trim().is_empty()));
                } else {
                    self.core.tab_group_ids.insert(tab, group);
                }
                if let Some(name) = tab_name.filter(|s| !s.trim().is_empty())
                    && let Some(dest) = self.core.tabs.get_mut(&tab)
                {
                    dest.rename_by_user(name);
                }
                tab
            },
        };
        let has_room = self
            .core
            .tabs
            .get_mut(&target)
            .is_some_and(|dest| dest.has_room_for_extracted_pane(&placement));
        let moved = if !has_room {
            Err(WebFocusError::Unavailable(
                "The destination tab has no room for this pane.".into(),
            ))
        } else if restore_source {
            if let Some(extracted) = self.extract_pane_for_transfer(source, owner, false) {
                self.accept_transferred_panes(vec![(extracted, placement)], target, false);
            }
            self.refresh_agent_worktrees();
            self.log_and_report_session_state().map_err(web_failed)
        } else {
            self.break_multiple_panes_to_tab_with_id(vec![owner], target, false, client)
                .map(|_| ())
                .map_err(web_failed)
        };
        if created
            && self
                .core
                .tabs
                .get(&target)
                .is_some_and(|tab| tab.get_all_pane_ids().is_empty())
        {
            self.close_tab_by_id(target).map_err(web_failed)?;
        }
        moved?;
        // A pane in a pane-tab container travels with its owner. The transfer
        // outcome can name the owner, so verify where the requested pane landed.
        if !self
            .core
            .tabs
            .get(&target)
            .is_some_and(|tab| tab.has_pane_with_pid(&pane))
        {
            return Err(WebFocusError::Unavailable(
                "Could not move the agent to that tab.".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn web_focus_agent(
        &mut self,
        agent: WebAgentRef,
        owner: SessionName,
        expected_instance: Option<&str>,
        deadline: Instant,
        reply: crossbeam::channel::Sender<Result<(), WebFocusError>>,
    ) {
        let result = (|| {
            let own_instance = self.core.instance_id.to_string();
            let mut matches = self
                .live_agents_with_tabs()
                .into_iter()
                .filter(|(live, tab)| {
                    let Some(info) = live.pane.agent.as_ref() else {
                        return false;
                    };
                    live.presentation().is_some()
                        && match (tab, expected_instance) {
                            (LiveAgentTab::Local(_), None) => agent.matches_agent(&live.id(), info),
                            (LiveAgentTab::Peer { session, .. }, Some(instance))
                                if instance != own_instance
                                    && session.web_peer_instance.as_deref() == Some(instance) =>
                            {
                                agent.matches_agent(
                                    &peer_web_detail_id(instance, live.pane.id, info),
                                    info,
                                )
                            },
                            _ => false,
                        }
                });
            let (live, tab) = matches.next().ok_or(WebFocusError::TargetChanged)?;
            if matches.next().is_some() {
                return Err(WebFocusError::TargetChanged);
            }
            drop(matches);
            let socket_name = match tab {
                LiveAgentTab::Local(_) => &self.core.session_name,
                LiveAgentTab::Peer { socket_name, .. } => socket_name,
            };
            if *socket_name != owner {
                return Err(WebFocusError::TargetChanged);
            }
            if *socket_name != live.session {
                return Err(WebFocusError::Unavailable(
                    "Focus is temporarily unavailable while the peer publishes its renamed session. Refresh Agents and reopen the card.".into(),
                ));
            }
            if expected_instance.is_some() && (!live.pane.is_selectable || live.pane.is_suppressed)
            {
                return Err(WebFocusError::Unavailable(
                    "The peer pane cannot be focused. Refresh Agents and try again.".into(),
                ));
            }
            let action = self
                .live_agent_focus_action(&live)
                .map_err(|_| WebFocusError::TargetChanged)?;
            let client = self
                .get_first_client_id()
                .ok_or(WebFocusError::NoTerminal)?;
            let pane = self
                .get_active_pane_id(&client)
                .ok_or(WebFocusError::NoTerminal)?;
            let tab = *self
                .core
                .active_tab_ids
                .get(&client)
                .ok_or(WebFocusError::NoTerminal)?;
            if !self.palette_target_exists(pane, tab, &action) {
                return Err(WebFocusError::TargetChanged);
            }
            if let yinz_types::input::actions::Action::FocusPaneByPaneId { pane_id } = action {
                if self.has_pending_tabs() {
                    return Err(WebFocusError::Unavailable(
                        "Terminal layout is not ready; retry".into(),
                    ));
                }
                self.core
                    .bus
                    .senders
                    .send_to_plugin(yinz_bus::plugins::PluginInstruction::UserInput {
                        client_id: client,
                        action: yinz_types::input::actions::Action::FocusPaneByPaneId { pane_id },
                        terminal_id: match pane {
                            yinz_types::pane_id::PaneId::Terminal(id) => Some(id),
                            _ => None,
                        },
                        cli_client_id: None,
                    })
                    .map_err(|e| WebFocusError::Failed(e.to_string()))?;
                // The same focus core as FocusPaneWithId, in this identity-validated turn.
                self.focus_existing_pane_now(pane_id, true, false, client)
                    .map_err(|e| WebFocusError::Failed(e.to_string()))?;
                if self.get_active_pane_id(&client) != Some(pane_id) {
                    return Err(WebFocusError::Failed(
                        "Agent pane could not be focused".into(),
                    ));
                }
            } else {
                // Peer focus retains palette session-switch semantics, based on this snapshot.
                if Instant::now() >= deadline {
                    return Err(WebFocusError::Unavailable(
                        "Focus verification expired. Refresh Agents and try again.".into(),
                    ));
                }
                self.dispatch_command_palette_action(
                    pane,
                    tab,
                    action,
                    client,
                    "Focus agent".into(),
                );
            }
            Ok(())
        })();
        reply
            .send(result)
            .map_err(|_| anyhow!("web reply receiver dropped"))
            .non_fatal();
    }
}

fn web_failed(error: anyhow::Error) -> WebFocusError {
    WebFocusError::Failed(format!("{error:#}"))
}
