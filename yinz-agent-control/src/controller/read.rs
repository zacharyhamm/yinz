use super::*;

impl super::AgentController {
    pub(super) fn reply_status(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
    ) {
        let outcome = if status.agent.state == AgentState::Exited
            && host.pane_info(pane).is_some_and(|info| info.exited)
        {
            AgentOutcome::TargetExited
        } else if status.status_available {
            AgentOutcome::Ok
        } else {
            AgentOutcome::StatusUnavailable
        };
        let message =
            (outcome == AgentOutcome::TargetExited).then(|| "agent process has exited".into());
        self.agent_reply(
            host,
            &client,
            self.agent_result(host, pane, outcome, message),
        );
    }
    pub(super) fn read_agent(
        &self,
        host: &mut impl AgentHost,
        pane: PaneId,
        client: Reply,
        lines: usize,
    ) {
        if lines == 0 || lines > 100_000 {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "lines must be between 1 and 100000",
            );
            return;
        }
        let output = host.dump_screen(pane).unwrap_or_default();
        let all_lines: Vec<_> = output.lines().collect();
        let end = all_lines
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(0, |index| index + 1);
        let output = all_lines[end.saturating_sub(lines)..end]
            .iter()
            .skip_while(|line| line.trim().is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        let mut response = self.agent_result(host, pane, AgentOutcome::Ok, None);
        response.output = Some(output);
        self.agent_reply(host, &client, response);
    }
}
