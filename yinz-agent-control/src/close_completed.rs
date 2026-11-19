//! Admission of explicit worker cleanup after durable acceptance of a result.
use crate::{host::AgentHost, providers};
use yinz_types::agent::{
    control::{AgentOutcome, AgentResponse, AgentStatus, NotReadyReason},
    tracking::Submission,
    AgentState,
};

/// Outstanding controller work at the instant the close was requested.
pub struct OutstandingWork {
    pub background: bool,
    pub input_claim: Option<NotReadyReason>,
    pub control_pending: bool,
}

pub struct ApprovedClose {
    discarded_input: bool,
}

pub fn authorize(
    host: &impl AgentHost,
    status: &AgentStatus,
    expected_session_id: Option<&str>,
    submission_id: &str,
    latest: Option<&Submission>,
    work: OutstandingWork,
) -> Result<ApprovedClose, (AgentOutcome, String)> {
    // The caller has durably accepted this submission's result. Composer
    // drafts and unconsumed terminal input are discarded with the pane;
    // only identity, correlation and real outstanding work block closing.
    let refusal = if expected_session_id.is_none_or(str::is_empty) {
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
            && s.conversation.as_deref() == expected_session_id
            && s.acknowledged
            && s.result
                .as_ref()
                .is_some_and(|r| r.outcome == AgentOutcome::Completed)
    }) {
        let message = match latest {
            None => format!("submission {submission_id} is not retained for this worker"),
            Some(s) if s.id != submission_id => format!(
                "submission {submission_id} is not the worker's latest submission ({})",
                s.id
            ),
            Some(s) => {
                let outcome = s
                    .result
                    .as_ref()
                    .and_then(|r| serde_json::to_value(r.outcome).ok())
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unresolved".to_owned());
                format!("submission {submission_id} has no acknowledged completed result in this conversation (outcome {outcome})")
            },
        };
        Some((AgentOutcome::Rejected, message))
    } else if status.background || work.background {
        Some((
            AgentOutcome::Rejected,
            "worker background work is still running".to_owned(),
        ))
    } else if let Some(reason) = work.input_claim {
        Some((
            AgentOutcome::Rejected,
            if reason == NotReadyReason::UnresolvedTurn {
                "worker has an unresolved accepted turn".to_owned()
            } else {
                "a Yinz prompt write to the worker is still in flight".to_owned()
            },
        ))
    } else if work.control_pending {
        Some((
            AgentOutcome::Rejected,
            "a Yinz control operation on the worker is still in flight".to_owned(),
        ))
    } else {
        None
    };
    if let Some(reason) = refusal {
        return Err(reason);
    }
    let pane = status.pane_id;
    let discarded_input = status.not_ready_reason == Some(NotReadyReason::UnconsumedInput)
        || providers::draft_composer(host, pane, &status.agent.kind)
            .is_some_and(|composer| !composer.empty);
    if !host.explicit_close_is_single_pane(pane) {
        return Err((
            AgentOutcome::Rejected,
            "worker cleanup would affect other panes".into(),
        ));
    }
    Ok(ApprovedClose { discarded_input })
}

impl ApprovedClose {
    pub fn finish(self, survived: bool) -> Result<AgentResponse, &'static str> {
        if survived {
            return Err("worker pane could not be closed");
        }
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        if self.discarded_input {
            response.message = Some("closed; discarded unconsumed terminal input".into());
        }
        Ok(response)
    }
}
