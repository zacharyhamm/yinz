use super::*;

impl super::AgentController {
    pub(in crate::controller) fn recover_agent(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
        now: Instant,
        expected_session_id: String,
        confirm_empty: bool,
        interrupted: bool,
    ) -> Option<Completion> {
        let state = status.agent.state;
        let in_flight = self.agent_controls.iter().any(|pending| {
            pending.pane == pane
                && (pending.prompt.is_some() || pending.command)
                && (!pending.abandoned || pending.delivery.is_some())
        });
        // An empty pin recovers only a pane that reports no session.
        // Codex binds at startup, before its first turn; Muse binds later.
        let refusal = if !confirm_empty {
            Some("recovery requires --conversation and --confirm-empty".to_owned())
        } else if expected_session_id.is_empty() && status.agent.session_id.is_some() {
            Some(
                "agent already has a conversation; pass its session_id as --conversation"
                    .to_owned(),
            )
        } else if !status.prompt_supported {
            Some("recovery requires an agent that accepts prompts".to_owned())
        } else if in_flight {
            Some("a submission or paste write is still in flight".to_owned())
        } else if state == AgentState::Idle {
            None
        } else if !matches!(state, AgentState::Busy | AgentState::Waiting) {
            Some("recovery requires a live agent".to_owned())
        } else if !interrupted {
            Some("recovery requires an idle agent. If agent read shows the agent stopped without reporting it, such as after an Esc interrupt, add --interrupted".to_owned())
        } else {
            // Claude reports nothing when a turn is interrupted, so the record would
            // stay busy forever. Quiet alone proves nothing: an agent waiting on a
            // person is always quiet. Someone must have acted on the pane after its
            // last report, and no hook, worker hooks included, may have followed.
            let input_at = host.agent_input_at(pane);
            let settled = self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                input_at.is_some_and(|input_at| {
                    input_at > lifecycle.last_report
                        && now.saturating_duration_since(input_at) >= RECOVER_QUIET
                })
            });
            (!settled).then(|| {
        format!(
            "the agent needs input, such as Esc, after its last report and {} seconds without a report since; inspect it again and retry",
            RECOVER_QUIET.as_secs()
        )
    })
        };
        if let Some(message) = refusal {
            self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, &message);
            return None;
        }
        Some(Completion::Recover {
            pane,
            agent: status.agent,
            revision: host.agent_input_revision(pane),
            client,
        })
    }
}

impl super::AgentController {
    pub(in crate::controller) fn begin_recovery(
        &mut self,
        pane: PaneId,
        revision: u64,
        interrupted: bool,
    ) {
        self.release_claude_permission(pane);
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.input_revision = Some(revision);
            lifecycle.claude.background = None;
            for obligation in &mut lifecycle.claude.activity.obligations.launches {
                if !obligation.settled {
                    obligation.unavailable = true;
                }
            }
            lifecycle.claude.notifications.clear();
            lifecycle.claude.activity.worker_wait_tools.clear();
            lifecycle.claude.activity.uncorrelated_worker_waits.clear();
            for turn in lifecycle.claude.activity.turns.values_mut() {
                turn.confirmation = Some(true);
            }
            if let Some(parent) = &mut lifecycle.claude.activity.parent {
                parent.state = AgentState::Idle;
                parent.detail = None;
                parent.waiting_on = None;
                parent.wait_needs_a_person = false;
            }
            if lifecycle.kind == "claude" {
                lifecycle.claude.confirmation = Some(true);
            }
            if lifecycle.kind == "agy" {
                lifecycle.agy = providers::AgyLifecycle::from_confirmation(Some(true));
            }
        }
        if interrupted {
            self.agent_worker_waits.remove(&pane);
        }
    }
    pub(in crate::controller) fn finish_recovery(&mut self, pane: PaneId) {
        if let Some(submissions) = self.agent_submissions.get_mut(&pane) {
            for submission in submissions
                .iter_mut()
                .filter(|submission| submission.result.is_none())
            {
                submission.result = Some(SubmissionResult {
                            outcome: AgentOutcome::Interrupted,
                            message: Some("Observation ended by explicit recovery; the task's completion was not verified.".into()),
                            background: false,
                        });
            }
        }
        self.agent_controls
            .retain(|pending| pending.pane != pane || pending.prompt.is_none());
        self.prune_agent_submissions(&self.protected_submissions());
        self.agent_dialog_inputs.remove(&pane);
        self.agent_clear_inputs.remove(&pane);
    }
}

impl super::AgentController {
    pub fn clearing_panes(&self) -> Vec<PaneId> {
        self.agent_draft_clears.keys().copied().collect()
    }
    pub(in crate::controller) fn advance_draft_clear(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
        now: Instant,
    ) -> Option<(AgentInfo, u64, Option<Reply>)> {
        let mut operation = self.agent_draft_clears.remove(&pane)?;
        if let Some(delivery) = &mut operation.delivery {
            match delivery.try_recv() {
                Ok(result) => {
                    operation.delivery = None;
                    if let Some(message) = result.error_message {
                        operation.failure = Some((AgentOutcome::Failed, message));
                    }
                },
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {},
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    operation.delivery = None;
                    operation.failure = Some((AgentOutcome::Failed, "writer disconnected".into()));
                },
            }
        }
        let status = self.agent_status(host, pane, None);
        // Prompt preparation is not recovery. Workers and same-conversation
        // progress may continue while we replace the steering draft.
        let steering = operation
            .prompt
            .as_ref()
            .is_some_and(|(pending, _, _)| pending.steering);
        // A freshly started agent's run record is bound asynchronously: the plugin
        // keeping its history publishes `<kind>:<conversation>` (`session_run_id`)
        // for the already bound conversation after the agent has reported ready, so
        // a prompt can capture the identity before it arrives. Accept that derived
        // ID once, then pin it for the remaining stages. Any other ID, provider or
        // conversation still fails closed below. The derivation trims the conversation,
        // so this comparison does too.
        if operation.prompt.is_some()
            && operation.agent.run_id.is_none()
            && let Some(conversation) = operation
                .agent
                .session_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            && let Some(status) = &status
            && status.agent.kind == operation.agent.kind
            && status.agent.session_id == operation.agent.session_id
            && status
                .agent
                .run_id
                .as_deref()
                .and_then(|id| id.strip_prefix(operation.agent.kind.as_str()))
                .and_then(|id| id.strip_prefix(':'))
                == Some(conversation)
        {
            operation.agent.run_id = status.agent.run_id.clone();
        }
        let failure = if host.pane_info(pane).is_none() {
            Some((AgentOutcome::TargetClosed, "pane closed"))
        } else if status.is_none() {
            Some((AgentOutcome::StatusUnavailable, "agent status unavailable"))
        } else if status.as_ref().is_some_and(|status| {
            status.agent.kind != operation.agent.kind
                || status.agent.session_id != operation.agent.session_id
                || status.agent.run_id != operation.agent.run_id
        }) {
            Some((AgentOutcome::TargetChanged, "agent identity changed"))
        } else if status
            .as_ref()
            .is_some_and(|status| !status.status_available)
        {
            Some((AgentOutcome::StatusUnavailable, "agent status unavailable"))
        } else if host.agent_input_revision(pane) != operation.revision {
            Some((
                AgentOutcome::Interrupted,
                "new terminal input cancelled clearing",
            ))
        } else if status.as_ref().is_none_or(|status| {
            !status.prompt_supported
                || (!steering
                    && (self.claude_background_pending(pane)
                        || status.agent.state != operation.agent.state
                        || status.background))
                || matches!(status.agent.state, AgentState::Waiting | AgentState::Exited)
        }) || (!steering
            && self
                .agent_lifecycle
                .get(&pane)
                .is_none_or(|lifecycle| lifecycle.last_report != operation.report))
        {
            Some((
                AgentOutcome::Interrupted,
                "agent activity changed during clearing",
            ))
        } else if now >= operation.deadline {
            Some((AgentOutcome::TimedOut, "composer clearing timed out"))
        } else {
            None
        };
        if operation.failure.is_none() {
            operation.failure = failure.map(|(outcome, message)| (outcome, message.into()));
        }
        if operation.failure.is_none() && operation.delivery.is_none() {
            let composer = providers::draft_composer(host, pane, &operation.agent.kind);
            let fresh = operation
                .wrote_at
                .is_none_or(|at| host.last_output(pane).is_some_and(|output| output > at));
            match (&mut operation.stage, composer) {
                (DraftStage::PromptMarker { clear_key, since }, _) => {
                    let since = since.get_or_insert(now);
                    // Pi exits on two Ctrl-C keys within 500 ms, even with a marker present.
                    if operation.agent.kind != "pi"
                        || now.saturating_duration_since(*since) >= Duration::from_millis(600)
                    {
                        let key = clear_key.clone();
                        operation.stage = DraftStage::PromptCleared { since: None };
                        self.write_draft_stage(
                            host,
                            pane,
                            &mut operation,
                            Some(key),
                            Vec::new(),
                            now,
                        );
                    }
                },
                (DraftStage::PromptCleared { since }, _) => {
                    let since = since.get_or_insert(now);
                    if now.saturating_duration_since(*since) >= Duration::from_millis(250)
                        && let Some((mut pending, prefix, paste)) = operation.prompt.take()
                    {
                        pending.input_revision = operation.revision;
                        self.deliver_agent_prompt(host, pending, prefix, paste);
                        return None;
                    }
                },
                // A provider can hide its cursor between chunks of a redraw.
                // Do not write or count stability until a complete layout returns.
                (DraftStage::Empty { since }, None) => *since = None,
                (DraftStage::Marker, None) => {},
                (DraftStage::Marker, Some(composer))
                    if fresh && composer.before_cursor.ends_with(&operation.marker) =>
                {
                    operation.stage = DraftStage::Empty { since: None };
                    self.write_draft_stage(host, pane, &mut operation, None, vec![3], now);
                },
                (DraftStage::Empty { since }, Some(composer)) if fresh && composer.empty => {
                    let since = since.get_or_insert(now);
                    // Any new output restarts stability, even if this snapshot is still empty.
                    let stable = host.last_output(pane).unwrap_or(*since).max(*since);
                    if now.saturating_duration_since(stable) >= Duration::from_millis(250) {
                        return Some((operation.agent, operation.revision, operation.client));
                    }
                },
                (DraftStage::Empty { since }, _) => *since = None,
                _ => {},
            }
        }
        if let Some((outcome, message)) = &operation.failure {
            if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane)
                && lifecycle.input_revision == Some(operation.revision)
            {
                lifecycle.input_revision = None;
            }
            if let Some((mut pending, _, _)) = operation.prompt.take() {
                log::warn!(
                        "agent prompt preparation failed: submission={:?} pane={} stage={:?} outcome={:?} expected_identity={:?} observed_identity={:?} expected_input_revision={} observed_input_revision={} pending_writer={}",
                        pending.submission_id, pane, operation.stage, outcome,
                        (&operation.agent.kind, &operation.agent.session_id, &operation.agent.run_id),
                        status.as_ref().map(|s| (&s.agent.kind, &s.agent.session_id, &s.agent.run_id)),
                        operation.revision, host.agent_input_revision(pane), operation.delivery.is_some(),
                    );
                if let Some(submission) = pending
                    .submission_id
                    .as_deref()
                    .and_then(|id| self.submission_mut(pane, id))
                {
                    submission
                        .prompt_delivery
                        .get_or_insert(PromptDelivery::NotSent);
                }
                pending.client = operation.client.take();
                pending.resolved = true;
                self.finish_agent_control(host,
                        &pending,
                        *outcome,
                        Some(format!("{message}; prompt was not sent. Partial clearing may have occurred; inspect the composer before manual recovery.")),
                        operation
                            .delivery
                            .is_some()
                            .then_some(NotReadyReason::ClearingDraft),
                    );
            }
            if let Some(client) = operation.client.take() {
                let mut response = self.agent_result(host, pane, *outcome, Some(format!("{message}; partial clearing may have occurred. Inspect the composer before manual recovery.")));
                if operation.delivery.is_some() {
                    for status in &mut response.agents {
                        status.ready = false;
                        status.not_ready_reason = Some(NotReadyReason::ClearingDraft);
                    }
                }
                self.agent_reply(host, &client, response);
            }
            // Keep ownership until every queued byte has been acknowledged, even on timeout.
            operation.delivery.as_ref()?;
        }
        self.agent_draft_clears.insert(pane, operation);
        None
    }
}
