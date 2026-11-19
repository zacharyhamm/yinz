use super::*;

impl super::AgentController {
    pub fn restart_agent_pane(
        &mut self,
        host: &mut impl AgentHost,
        pane_id: PaneId,
        prompt: Option<&str>,
        session: yinz_types::agent::control::AgentRestartSession,
        originating_plugin: Option<yinz_types::data::OriginatingPlugin>,
    ) -> Result<yinz_types::agent::control::RestartedAgent, String> {
        let prepared = host.prepare_restart(pane_id, prompt, session, originating_plugin)?;
        let PaneId::Terminal(terminal) = pane_id else {
            return Err("target must be a terminal pane".into());
        };
        self.refresh_agent_events(host);
        host.apply_restart(
            terminal,
            prepared.command,
            prepared.fresh,
            &prepared.current,
        )?;
        self.retire_agent_event_pane(host, pane_id, "pane_replaced");
        self.invalidate_agent_lifecycle(host, pane_id, "agent_restarted");
        host.state_hooks(pane_id, prepared.previous.as_ref(), Some(&prepared.current));
        Ok(prepared.result)
    }
}
