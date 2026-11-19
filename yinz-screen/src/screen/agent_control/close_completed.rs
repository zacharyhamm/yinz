use super::*;

pub(in crate::screen) struct AgentClose {
    pub(in crate::screen) pane: PaneId,
    client: Reply,
    worker: bool,
    discarded_input: bool,
}

impl AgentClose {
    pub(in crate::screen) fn controller(pane: PaneId, client: Reply) -> Self {
        Self {
            pane,
            client,
            worker: false,
            discarded_input: false,
        }
    }

    pub(in crate::screen) fn finish(
        self,
        agents: &super::AgentController,
        host: &impl AgentHost,
        survived: bool,
    ) {
        if survived && self.worker {
            agents.reject_control(
                host,
                &self.client,
                self.pane,
                None,
                AgentOutcome::Rejected,
                "worker pane could not be closed",
            );
            return;
        }
        let mut response = AgentResponse::new(if survived {
            AgentOutcome::Rejected
        } else {
            AgentOutcome::Ok
        });
        if survived {
            response.message = Some("Controller pane could not be closed".into());
        } else if self.discarded_input {
            response.message = Some("closed; discarded unconsumed terminal input".into());
        }
        agents.agent_reply(host, &self.client, response);
    }
}

impl super::AgentController {
    pub(in crate::screen) fn close_completed(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
        expected_session_id: Option<String>,
        submission_id: String,
    ) -> Option<AgentClose> {
        // The caller has durably accepted this submission's result. Composer
        // drafts and unconsumed terminal input are discarded with the pane;
        // only identity, correlation and real outstanding work block closing.
        let latest = self.agent_submissions.get(&pane).and_then(|s| s.back());
        let refusal = if expected_session_id.as_ref().is_none_or(|id| id.is_empty()) {
            Some((
                AgentOutcome::Rejected,
                "close-completed requires the worker's conversation".to_owned(),
            ))
        } else if !status.status_available {
            Some((
                AgentOutcome::StatusUnavailable,
                "worker status is unavailable".to_owned(),
            ))
        } else if status.agent.state != AgentState::Idle {
            Some((
                AgentOutcome::Rejected,
                format!("worker is {}, not idle", status.agent.state),
            ))
        } else if !latest.is_some_and(|s| {
            s.id == submission_id
                && s.conversation == expected_session_id
                && s.acknowledged
                && s.result
                    .as_ref()
                    .is_some_and(|r| r.outcome == AgentOutcome::Completed)
        }) {
            Some((
        AgentOutcome::Rejected,
        match latest {
            None => format!(
                "submission {submission_id} is not retained for this worker"
            ),
            Some(s) if s.id != submission_id => format!(
                "submission {submission_id} is not the worker's latest submission ({})",
                s.id
            ),
            Some(s) => format!(
                "submission {submission_id} has no acknowledged completed result in this conversation (outcome {})",
                s.result
                    .as_ref()
                    .and_then(|r| serde_json::to_value(r.outcome).ok())
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unresolved".to_owned())
            ),
        },
    ))
        } else if status.background || self.claude_background_pending(pane) {
            Some((
                AgentOutcome::Rejected,
                "worker background work is still running".to_owned(),
            ))
        } else if let Some(reason) = self
            .agent_controls
            .iter()
            .filter(|pending| pending.pane == pane)
            .find_map(Pending::not_ready_reason)
        {
            Some((
                AgentOutcome::Rejected,
                if reason == NotReadyReason::UnresolvedTurn {
                    "worker has an unresolved accepted turn".to_owned()
                } else {
                    "a Yinz prompt write to the worker is still in flight".to_owned()
                },
            ))
        } else if self.agent_draft_clears.contains_key(&pane)
            || self
                .agent_controls
                .iter()
                .any(|pending| pending.pane == pane)
        {
            Some((
                AgentOutcome::Rejected,
                "a Yinz control operation on the worker is still in flight".to_owned(),
            ))
        } else {
            None
        };
        if let Some((outcome, message)) = refusal {
            self.reject_control(host, &client, pane, None, outcome, &message);
            return None;
        }
        let discarded_input = status.not_ready_reason == Some(NotReadyReason::UnconsumedInput)
            || providers::draft_composer(host, pane, &status.agent.kind).is_some_and(|c| !c.empty);
        if !host.explicit_close_is_single_pane(pane) {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "worker cleanup would affect other panes",
            );
            return None;
        }
        Some(AgentClose {
            pane,
            client,
            worker: true,
            discarded_input,
        })
    }
}
