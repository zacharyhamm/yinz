use super::*;
use crate::plugins::HookFiring;
use yinz_agent_control::host::{AgentHost, PaneInput, PreparedRestart};
use yinz_types::data::HookPayload;
use yinz_types::input::hooks::HookPoint;

impl ScreenCore {
    pub(super) fn tab_containing_pane(&self, pane_id: PaneId) -> Option<&Tab> {
        self.tabs
            .values()
            .find(|tab| tab.has_pane_with_pid(&pane_id))
    }

    pub(super) fn tab_containing_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Tab> {
        self.tabs
            .values_mut()
            .find(|tab| tab.has_pane_with_pid(&pane_id))
    }

    pub(super) fn get_pane_info(&self, pane_id: PaneId) -> Option<PaneInfo> {
        // Search through all tabs to find the pane
        for tab in self.tabs.values() {
            if let Some(pane_info) = tab.get_pane_info(pane_id) {
                return Some(pane_info);
            }
        }
        None
    }
}

impl AgentHost for ScreenCore {
    fn restore_terminal_state(&mut self, pane: PaneId, state: &str) {
        for tab in self.tabs.values_mut() {
            if let Some(terminal) = tab.get_pane_with_id_mut(pane) {
                terminal
                    .restore_terminal_state(state)
                    .map_err(|e| anyhow!("{e}"))
                    .context("restore adopted terminal state")
                    .non_fatal();
                break;
            }
        }
    }
    fn restored_observer(&self) -> Option<yinz_agent_control::host::RestoredObserverRegistration> {
        Some((
            self.restored_observer.clone()?,
            self.bus.senders.to_screen.clone()?,
        ))
    }

    fn agent_enter_revision(&self, pane: PaneId) -> u64 {
        self.tab_containing_pane(pane)
            .and_then(|tab| tab.get_pane_with_id(pane))
            .map_or(0, |p| p.agent_enter_revision())
    }

    fn instance_id(&self) -> uuid::Uuid {
        self.instance_id
    }
    fn held_agent_conversation(&self, pane: PaneId) -> Option<Option<&str>> {
        self.tab_containing_pane(pane)?
            .get_pane_with_id(pane)
            .filter(|p| p.is_held() && p.exited() && p.was_an_agent())
            .map(|p| p.last_agent_session_id())
    }
    fn pane_is_agent(&self, pane: PaneId) -> bool {
        self.tab_containing_pane(pane)
            .is_some_and(|tab| tab.pane_is_agent(pane))
    }
    fn set_suppressed(&mut self, pane: PaneId, suppress: bool) -> Result<(), &'static str> {
        if suppress
            && self
                .get_pane_info(pane)
                .is_some_and(|info| !info.is_suppressed)
            && !self.tabs.values().any(|tab| {
                !tab.is_pending()
                    && !self.deferred_tab_closes.contains(&tab.id)
                    && tab.has_selectable_panes_except(pane)
            })
        {
            return Err("cannot hide the session's last selectable pane");
        }
        if let Some(tab) = self.tab_containing_pane_mut(pane)
            && tab
                .get_pane_info(pane)
                .is_some_and(|info| info.is_suppressed != suppress)
        {
            if !suppress
                && (tab.pane_tabs_of(pane).is_some() || tab.pane_is_hidden_stack_list_member(&pane))
            {
                return Err("agent pane is hidden in a pane-tab group or stack; use pane-tab or stack navigation to show it");
            }
            if !suppress
                && tab
                    .overlay_placement_for(pane)
                    .is_some_and(|placement| placement.kind.is_modal())
            {
                return Err("cannot show a hidden modal overlay without changing focus; use overlay navigation to show it");
            }
            if tab.pane_is_overlay(pane) {
                if suppress {
                    tab.hide_overlay_pane(pane);
                } else {
                    tab.show_overlay_pane(pane);
                }
            } else if suppress {
                tab.suppress_pane(pane, None);
            } else {
                tab.unsuppress_pane(pane, false);
            }
        }
        if !self
            .get_pane_info(pane)
            .is_some_and(|info| info.is_suppressed == suppress)
        {
            return Err(if suppress {
                "agent pane could not be hidden"
            } else {
                "no room to show agent pane; it remains suppressed"
            });
        }
        Ok(())
    }

    fn write_key(
        &mut self,
        pane: PaneId,
        key: KeyWithModifier,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        self.tab_containing_pane_mut(pane)
            .context("agent pane no longer exists")?
            .write_key_to_pane_id(pane, key, completion)
    }
    fn write_command(
        &mut self,
        pane: PaneId,
        prefix: Vec<u8>,
        paste: Vec<u8>,
        completion: NotificationEnd,
    ) -> Result<()> {
        self.tab_containing_pane_mut(pane)
            .context("agent pane no longer exists")?
            .write_agent_command_to_pane_id(pane, prefix, paste, Some(completion))
            .map(|_| ())
    }
    fn write_bytes(
        &mut self,
        pane: PaneId,
        bytes: Vec<u8>,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        self.tab_containing_pane_mut(pane)
            .ok_or_else(|| anyhow!("failed to find pane with id {pane:?}"))?
            .write_to_pane_id(&None, bytes, false, pane, None, completion)
            .map(|_| ())
    }
    fn paste(
        &mut self,
        pane: PaneId,
        bytes: Vec<u8>,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        self.tab_containing_pane_mut(pane)
            .ok_or_else(|| anyhow!("failed to find pane with id {pane:?}"))?
            .paste_to_pane_id(bytes, pane, None, completion)
            .map(|_| ())
    }

    fn local_agent_directories(&self) -> HashMap<PaneId, std::path::PathBuf> {
        #[cfg(test)]
        self.worktree_scans.set(self.worktree_scans.get() + 1);
        self.tabs.values().flat_map(|tab| {
            tab.get_all_pane_ids().into_iter().filter_map(|id| {
                let pane = tab.get_pane_with_id(id)?;
                let agent = pane.agent_info()?;
                if agent.state == AgentState::Exited || pane.exited()
                    || matches!(id, PaneId::Terminal(id) if self.terminal_hosts.contains_key(&id))
                    || matches!(pane.invoked_with(), Some(Run::Command(command)) if command.target.is_remote())
                {
                    return None;
                }
                let cwd = PathBuf::from(agent.cwd.as_ref()?);
                cwd.is_absolute().then_some((id, cwd))
            })
        }).collect()
    }
    fn set_worktree_roots(&mut self, roots: HashMap<PaneId, std::path::PathBuf>) {
        for tab in self.tabs.values_mut() {
            tab.agent_worktrees = tab
                .get_all_pane_ids()
                .into_iter()
                .filter_map(|id| roots.get(&id).cloned().map(|root| (id, root)))
                .collect();
        }
    }
    fn worktree_sender(
        &self,
    ) -> Option<yinz_runtime::channels::SenderWithContext<ScreenInstruction>> {
        self.bus.senders.to_screen.clone()
    }
    fn set_agent(&mut self, pane: PaneId, agent: Option<AgentInfo>) -> bool {
        self.tab_containing_pane_mut(pane)
            .is_some_and(|tab| tab.set_pane_agent(pane, agent))
    }

    fn event_transport_available(&self) -> bool {
        self.bus.os_input.is_some()
    }
    fn send_event(&self, client: ClientId, event_json: String) -> bool {
        self.bus.os_input.as_ref().is_some_and(|os| {
            os.send_to_client(
                client,
                yinz_ipc::ipc::ServerToClientMsg::EventUpdate {
                    event_type: "agent_lifecycle".into(),
                    event_json,
                },
            )
            .is_ok()
        })
    }
    fn subscription_error(&self, client: ClientId, message: String) {
        self.bus
            .senders
            .send_to_server(ServerInstruction::LogError(vec![message], client, None))
            .non_fatal();
    }

    fn pane_info(&self, pane: PaneId) -> Option<PaneInfo> {
        self.get_pane_info(pane)
    }
    fn session_name(&self) -> &str {
        self.session_name.as_str()
    }
    fn tab_for_pane(&self, pane: PaneId) -> Option<TabId> {
        self.tab_containing_pane(pane).map(|tab| tab.id)
    }
    fn terminal_cwd(&self, pane: PaneId) -> Option<String> {
        let PaneId::Terminal(id) = pane else {
            return None;
        };
        self.terminal_cwds
            .get(&id)
            .map(|path| path.to_string_lossy().into_owned())
    }
    fn foreground_command(&self, id: TerminalId) -> Option<&[String]> {
        self.terminal_foreground_cmds.get(&id).map(Vec::as_slice)
    }
    fn last_output(&self, pane: PaneId) -> Option<Instant> {
        self.pane_output_activity.get(&pane).copied()
    }
    fn pane_input(&self, pane_id: PaneId) -> Option<PaneInput> {
        let tab = self.tab_containing_pane(pane_id)?;
        let pane = tab.get_pane_with_id(pane_id)?;
        let command = tab.pane_run_command(pane_id);
        Some(PaneInput {
            suspended: pane.is_suspended(),
            sync_active: tab.is_sync_panes_active(),
            excluded_from_sync: pane.exclude_from_sync(),
            command_present: command.is_some(),
            command_kind: command.and_then(|run| {
                yinz_types::agent::kind_from_program_and_args(&run.command, &run.args)
            }),
            bracketed_paste: pane.supports_bracketed_paste(),
        })
    }
    fn viewport(
        &self,
        pane: PaneId,
        ansi: bool,
    ) -> Option<(usize, yinz_types::data::PaneContents)> {
        let pane = self.tab_containing_pane(pane)?.get_pane_with_id(pane)?;
        let contents = if ansi {
            pane.pane_contents_with_ansi(None, false, None)
        } else {
            pane.pane_contents(None, false, None)
        };
        Some((pane.get_content_columns(), contents))
    }
    fn agent_input_revision(&self, pane: PaneId) -> u64 {
        self.tab_containing_pane(pane)
            .map(|tab| tab.agent_input_revision(pane))
            .unwrap_or(0)
    }
    fn agent_input_at(&self, pane: PaneId) -> Option<Instant> {
        self.tab_containing_pane(pane)
            .and_then(|tab| tab.agent_input_at(pane))
    }
    fn prepare_restart(
        &self,
        pane_id: PaneId,
        prompt: Option<&str>,
        session: yinz_types::agent::control::AgentRestartSession,
        originating_plugin: Option<yinz_types::data::OriginatingPlugin>,
    ) -> Result<PreparedRestart, String> {
        use yinz_types::agent::{
            control::AgentRestartSession, control::RestartedAgent, AgentSource,
        };
        let PaneId::Terminal(_) = pane_id else {
            return Err("target must be a terminal pane".into());
        };
        let pane = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.get_pane_with_id(pane_id))
            .ok_or("target pane does not exist")?;
        let mut command = pane.agent_restart_command(session, prompt)?;
        let kind = pane
            .last_agent_kind()
            .ok_or("target has no remembered agent kind")?
            .to_owned();
        let cwd = command
            .cwd
            .as_ref()
            .map(|cwd| cwd.display().to_string())
            .or_else(|| pane.last_agent_cwd().map(str::to_owned));
        let session_id = (session == AgentRestartSession::Resume)
            .then(|| pane.last_agent_session_id().map(str::to_owned))
            .flatten();
        let previous = pane.agent_info().cloned();
        if let Some(origin) = originating_plugin {
            command.originating_plugin = Some(origin);
        }
        command.hold_on_close = true;
        let result = RestartedAgent {
            pane_id,
            session_id: session_id.clone(),
            command: std::iter::once(command.command.display().to_string())
                .chain(command.args.iter().cloned())
                .collect(),
        };
        let current = AgentInfo {
            kind,
            cwd,
            session_id,
            state: AgentState::Busy,
            source: AgentSource::Spawned,
            detail: None,
            waiting_on: None,
            run_id: None,
            wait_needs_a_person: false,
        };
        Ok(PreparedRestart {
            command,
            result,
            previous,
            current,
            fresh: session == AgentRestartSession::Fresh,
        })
    }
    fn apply_restart(
        &mut self,
        terminal_id: TerminalId,
        command: RunCommand,
        fresh: bool,
        current: &AgentInfo,
    ) -> Result<(), String> {
        let pane_id = PaneId::Terminal(terminal_id);
        let tab = self
            .tab_containing_pane_mut(pane_id)
            .ok_or("target pane does not exist")?;
        if !tab.run_command_in_pane_with_id(terminal_id, command, false) {
            return Err("agent pane could not accept the restart".into());
        }
        if fresh && let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
            pane.clear_agent_session_id();
        }
        tab.set_pane_agent(pane_id, Some(current.clone()));
        Ok(())
    }
    fn explicit_close_is_single_pane(&self, pane: PaneId) -> bool {
        self.tab_containing_pane(pane)
            .is_some_and(|tab| tab.explicit_close_is_single_pane(pane))
    }
    fn dump_screen(&mut self, pane: PaneId) -> Option<String> {
        self.tab_containing_pane_mut(pane)?
            .get_dump_terminal_screen(pane, true)
    }
    fn pane_ids(&self) -> Vec<PaneId> {
        self.tabs
            .values()
            .flat_map(|tab| tab.get_all_pane_ids())
            .collect()
    }
    fn pane_agent(&self, pane: PaneId) -> Option<AgentInfo> {
        self.tab_containing_pane(pane)?.pane_agent_info(pane)
    }
    fn pane_was_destroyed(&self, pane: PaneId) -> bool {
        self.destroyed_panes.contains(&pane)
    }
    fn send_control_reply(
        &self,
        client: ClientId,
        response: yinz_types::agent::control::AgentResponse,
    ) {
        if let Some(os) = &self.bus.os_input {
            os.send_to_client(
                client,
                yinz_ipc::ipc::ServerToClientMsg::AgentControl(response),
            )
            .context("reply to agent command")
            .non_fatal();
        }
    }
    fn remove_event_subscriber(&self, client: ClientId) {
        self.bus
            .senders
            .send_to_server(ServerInstruction::RemoveClient(client))
            .non_fatal();
    }
    fn resolve_event_projects(
        &self,
        version: u64,
        agents: Vec<yinz_types::agent::control::AgentStatus>,
        projects: Vec<String>,
    ) {
        self.bus
            .senders
            .send_to_plugin(
                crate::plugins::PluginInstruction::ResolveAgentEventProjects {
                    version,
                    agents,
                    projects,
                },
            )
            .non_fatal();
    }
    fn state_hooks(&self, pane: PaneId, previous: Option<&AgentInfo>, current: Option<&AgentInfo>) {
        self.fire_agent_state_hooks(pane, previous, current);
    }
}

impl ScreenCore {
    pub(in crate::screen) fn fire_hook(
        &self,
        point: HookPoint,
        pane_id: Option<PaneId>,
        payload: HookFiring,
        client_id: ClientId,
    ) {
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::RunHook {
                point,
                pane_id,
                payload,
                client_id,
            })
            .non_fatal();
    }
}

impl ScreenCore {
    pub(in crate::screen) fn hook_client_id(
        &self,
        run_command: Option<&RunCommand>,
    ) -> Option<ClientId> {
        run_command
            .and_then(|run_command| run_command.originating_plugin.as_ref())
            .map(|plugin| plugin.client_id)
            .or_else(|| self.active_tab_ids.keys().next().copied())
    }
}

impl ScreenCore {
    pub(in crate::screen) fn fire_agent_state_hooks(
        &self,
        pane_id: PaneId,
        previous: Option<&AgentInfo>,
        current: Option<&AgentInfo>,
    ) {
        let edges = agent_hook_edges(previous, current);
        if edges.is_empty() {
            return;
        }
        let Some(current) = current else {
            return;
        };
        let PaneId::Terminal(terminal_pane_id) = pane_id else {
            return;
        };
        let run_command = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.pane_run_command(pane_id));
        let Some(client_id) = self.hook_client_id(run_command.as_ref()) else {
            return;
        };
        // The record as it stands, with the pane's folder filled in where the agent has not
        // said where it works.
        let mut agent = current.clone();
        if agent.cwd.is_none() {
            agent.cwd = run_command
                .as_ref()
                .and_then(|run_command| run_command.cwd.as_ref())
                .map(|cwd| cwd.display().to_string());
        }
        let payload = HookPayload::Agent {
            pane_id: terminal_pane_id,
            agent,
            previous_state: previous.map(|previous| previous.state),
            command: run_command
                .as_ref()
                .map(|run_command| run_command.to_string()),
        };
        for point in edges {
            self.fire_hook(
                point,
                Some(pane_id),
                HookFiring::Headless(payload.clone()),
                client_id,
            );
        }
    }
}
