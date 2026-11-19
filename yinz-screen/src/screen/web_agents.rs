//! Live snapshots and focus share the palette's population, presentation and action rules.
use super::command_palette::history::LiveAgentTab;
use super::Screen;
use std::collections::HashMap;
use std::time::Instant;
use yinz_bus::command_palette::HistoryHeader;
use yinz_bus::web::{WebAgentRef, WebFocusError, WebLiveAgent, WebLiveHeader};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{peer_web_detail_id, valid_web_peer_instance};
use yinz_types::internal_ids::SessionName;

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
