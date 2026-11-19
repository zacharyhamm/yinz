use super::*;

impl super::AgentController {
    pub(in crate::screen) fn event_submission(
        &self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> Option<String> {
        self.agent_controls
            .iter()
            .find(|pending| {
                pending.pane == pane
                    && pending
                        .prompt
                        .as_ref()
                        .is_some_and(|p| p.matches_submission(hook) && p.acknowledges(hook))
            })
            .and_then(|pending| pending.submission_id.clone())
    }
}

impl super::AgentController {
    pub(in crate::screen) fn submission(&self, pane: PaneId, id: &str) -> Option<&Submission> {
        self.agent_submissions
            .get(&pane)?
            .iter()
            .find(|submission| submission.id == id)
    }
}

impl super::AgentController {
    pub(in crate::screen) fn submission_mut(
        &mut self,
        pane: PaneId,
        id: &str,
    ) -> Option<&mut Submission> {
        self.agent_submissions
            .get_mut(&pane)?
            .iter_mut()
            .find(|submission| submission.id == id)
    }
}

impl super::AgentController {
    pub(in crate::screen) fn record_submission(
        &mut self,
        pending: &Pending,
        result: Option<SubmissionResult>,
    ) {
        if pending.prompt.is_none() && !pending.command {
            return;
        }
        let Some(id) = &pending.submission_id else {
            return;
        };
        let Some(submission) =
            self.agent_submissions
                .get_mut(&pending.pane)
                .and_then(|submissions| {
                    submissions
                        .iter_mut()
                        .find(|submission| &submission.id == id)
                })
        else {
            return;
        };
        if let Some(prompt) = &pending.prompt {
            submission.acknowledged = prompt.acknowledged();
            submission.turn_id = prompt.turn_id().map(str::to_owned);
        }
        if pending.conversation.is_some() {
            submission.conversation = pending.conversation.clone();
        }
        if let Some(result) = result.filter(|_| submission.result.is_none()) {
            submission.result = Some(result);
        }
    }
}

impl super::AgentController {
    pub(in crate::screen) fn protected_submissions(&self) -> BTreeMap<PaneId, BTreeSet<String>> {
        let mut protected: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for pending in &self.agent_controls {
            if pending.prompt.is_none()
                && pending.client.is_some()
                && let Some(id) = &pending.submission_id
            {
                protected
                    .entry(pending.pane)
                    .or_default()
                    .insert(id.clone());
            }
        }
        protected
    }
}

impl super::AgentController {
    pub(in crate::screen) fn prune_agent_submissions(
        &mut self,
        protected: &BTreeMap<PaneId, BTreeSet<String>>,
    ) {
        for (pane, submissions) in &mut self.agent_submissions {
            prune_submissions(submissions, protected.get(pane));
        }
    }
}

impl super::AgentController {
    pub(in crate::screen) fn retain_closed_agent(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        now: Instant,
    ) {
        if host.pane_info(pane).is_some() {
            let revision = host.agent_input_revision(pane);
            self.closed_agent_submissions
                .retain(|(_, proof, saved_revision, _)| {
                    proof.pane_id != pane || *saved_revision == revision
                });
        }
        // A pre-removal sample may already be frozen by the closing operation.
        if host.pane_info(pane).is_none_or(|info| {
            info.agent
                .is_none_or(|agent| agent.state == AgentState::Exited)
        }) {
            // SessionEnd or HoldPane may already have frozen the original identity.
            // An exited record cannot resample completion; preserve the original TTL.
            return;
        }
        let Some(proof) = self.closed_agent_proof(host, pane) else {
            self.closed_agent_submissions
                .retain(|(_, old, _, _)| old.pane_id != pane);
            return;
        };
        if self
            .closed_agent_submissions
            .iter()
            .any(|(_, old, _, _)| old.submission_id == proof.submission_id && old.pane_id == pane)
        {
            return;
        }
        let source = self
            .agent_lifecycle
            .get(&pane)
            .and_then(|l| l.claude.activity.obligations.source.clone());
        self.closed_agent_submissions.push_back((
            now,
            proof,
            host.agent_input_revision(pane),
            source,
        ));
        self.prune_closed_agents(now);
    }
}

impl super::AgentController {
    pub(in crate::screen) fn closed_agent_proof(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
    ) -> Option<yinz_types::agent::control::RetainedSubmission> {
        let (revision, id, prompt_id) = self.agent_terminal_claims.get(&pane)?;
        let status = self.agent_status(host, pane, None)?;
        if *revision != host.agent_input_revision(pane)
            || !status.ready
            || !status.status_available
            || status.background
            || status.agent.state != AgentState::Idle
            || self
                .agent_controls
                .iter()
                .any(|pending| pending.pane == pane)
            || self.agent_dialog_inputs.contains_key(&pane)
            || self.agent_clear_inputs.contains_key(&pane)
            || self.agent_draft_clears.contains_key(&pane)
            || self.agent_worker_waits.contains_key(&pane)
            || self.claude_permission_waits.contains_key(&pane)
        {
            return None;
        }
        let submission = self.submission(pane, id)?;
        let result = submission.result.as_ref()?;
        if !submission.acknowledged
            || result.outcome != AgentOutcome::Completed
            || result.background
            || submission.conversation != status.agent.session_id
            || submission.prompt_delivery != Some(PromptDelivery::Started)
        {
            return None;
        }
        let cwd = status.cwd?;
        let conversation_id = submission.conversation.clone()?;
        Some(yinz_types::agent::control::RetainedSubmission {
            server_instance: yinz_bus::agent_launch::server_instance().to_owned(),
            session: host.session_name().to_string(),
            pane_id: pane,
            kind: submission.kind.clone(),
            cwd,
            conversation_id,
            submission_id: id.clone(),
            turn_id: submission.turn_id.clone(),
            prompt_id: prompt_id.clone(),
            acknowledged: true,
            prompt_delivery: PromptDelivery::Started,
            outcome: result.outcome,
            message: result.message.clone(),
            answered: true,
            quiescent: true,
        })
    }
}

impl super::AgentController {
    pub(in crate::screen) fn prune_closed_agents(&mut self, now: Instant) {
        const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
        const CAPACITY: usize = 1024;
        while self
            .closed_agent_submissions
            .front()
            .is_some_and(|(at, _, _, _)| now.saturating_duration_since(*at) >= RETENTION)
            || self.closed_agent_submissions.len() > CAPACITY
        {
            self.closed_agent_submissions.pop_front();
        }
    }
}
