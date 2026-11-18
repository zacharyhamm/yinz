//! Live snapshots and focus share the palette's population, presentation and action rules.
use super::command_palette::history::LiveAgentTab;
use super::Screen;
use std::collections::HashMap;
use yinz_bus::command_palette::HistoryHeader;
use yinz_bus::web::{WebAgentRef, WebFocusError, WebLiveAgent, WebLiveHeader};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;

impl Screen {
    pub(super) fn web_live_agents(&mut self, reply: crossbeam::channel::Sender<Vec<WebLiveAgent>>) {
        let mut local_groups = HashMap::new();
        let agents = self
            .live_agents_with_tabs()
            .into_iter()
            .filter_map(|(live, tab)| {
                let presentation = live.presentation()?;
                let (tab_group_session, tab_group) = match tab {
                    LiveAgentTab::Local(tab) => (
                        &self.session_name,
                        self.tab_group_ids.get(&tab).and_then(|&id| {
                            local_groups
                                .entry(id)
                                .or_insert_with(|| self.tab_group_info(tab))
                                .clone()
                        }),
                    ),
                    // A renamed socket can be discovered before the checkpoint name changes.
                    LiveAgentTab::Peer {
                        socket_name,
                        session,
                        position,
                    } => (
                        socket_name,
                        session
                            .tabs
                            .iter()
                            .find(|tab| tab.position == position)
                            .and_then(|tab| tab.group.clone()),
                    ),
                };
                let (glyph, status, _) = yinz_panes::ui::agent_title::status(presentation.state);
                let mut header = HistoryHeader::default();
                live.header_overlay(&mut header, None);
                Some(WebLiveAgent {
                    id: live.id(),
                    session: live.session.clone(),
                    tab_group_session: tab_group_session.clone(),
                    tab_group,
                    current_session: live.session == self.session_name,
                    run: live.run(),
                    label_source: live.label_source(),
                    provider: presentation.provider,
                    activity: presentation.activity,
                    host: presentation.host,
                    pane_label: presentation.pane_id,
                    glyph,
                    status,
                    focused: live.session == self.session_name
                        && self
                            .active_tab_ids
                            .keys()
                            .any(|client| self.get_active_pane_id(client) == Some(live.pane.id)),
                    selectable: live.pane.is_selectable,
                    suppressed: live.pane.is_suppressed,
                    usage_unavailable: live.usage_unavailable(&self.session_name),
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
        reply: crossbeam::channel::Sender<Result<(), WebFocusError>>,
    ) {
        let result = (|| {
            use yinz_types::agent::control::valid_web_identity;
            if ![&agent.id, &agent.kind, &agent.session_id]
                .into_iter()
                .all(|value| valid_web_identity(value))
            {
                return Err(WebFocusError::TargetChanged);
            }
            let mut matches = self
                .live_agents_with_tabs()
                .into_iter()
                .filter(|(live, _)| {
                    live.presentation().is_some()
                        && live.id() == agent.id
                        && live.pane.agent.as_ref().is_some_and(|a| {
                            a.kind == agent.kind
                                && a.session_id.as_deref() == Some(agent.session_id.as_str())
                        })
                });
            let (live, tab) = matches.next().ok_or(WebFocusError::TargetChanged)?;
            if matches.next().is_some() {
                return Err(WebFocusError::TargetChanged);
            }
            drop(matches);
            let socket_name = match tab {
                LiveAgentTab::Local(_) => &self.session_name,
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
                self.bus
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
