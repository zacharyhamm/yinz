use super::*;

pub struct AgentClose {
    pub pane: PaneId,
    client: Reply,
    worker: Option<crate::close_completed::ApprovedClose>,
}

impl AgentClose {
    pub(in crate::controller) fn controller(pane: PaneId, client: Reply) -> Self {
        Self {
            pane,
            client,
            worker: None,
        }
    }

    pub fn finish(self, agents: &super::AgentController, host: &impl AgentHost, survived: bool) {
        if let Some(worker) = self.worker {
            match worker.finish(survived) {
                Ok(response) => agents.agent_reply(host, &self.client, response),
                Err(reason) => agents.reject_control(
                    host,
                    &self.client,
                    self.pane,
                    None,
                    AgentOutcome::Rejected,
                    reason,
                ),
            }
            return;
        }
        let mut response = AgentResponse::new(if survived {
            AgentOutcome::Rejected
        } else {
            AgentOutcome::Ok
        });
        if survived {
            response.message = Some("Controller pane could not be closed".into());
        }
        agents.agent_reply(host, &self.client, response);
    }
}

impl super::AgentController {
    pub(in crate::controller) fn close_completed(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
        expected_session_id: Option<String>,
        submission_id: String,
    ) -> Option<AgentClose> {
        use crate::close_completed::{authorize, OutstandingWork};
        let mut pending = self
            .agent_controls
            .iter()
            .filter(|request| request.pane == pane)
            .peekable();
        let work = OutstandingWork {
            background: self.claude_background_pending(pane),
            control_pending: self.agent_draft_clears.contains_key(&pane)
                || pending.peek().is_some(),
            input_claim: pending.find_map(Pending::not_ready_reason),
        };
        let latest = self
            .agent_submissions
            .get(&pane)
            .and_then(|submissions| submissions.back());
        let approved = match authorize(
            host,
            &status,
            expected_session_id.as_deref(),
            &submission_id,
            latest,
            work,
        ) {
            Ok(close) => close,
            Err((outcome, message)) => {
                self.reject_control(host, &client, pane, None, outcome, &message);
                return None;
            },
        };
        Some(AgentClose {
            pane,
            client,
            worker: Some(approved),
        })
    }
}
