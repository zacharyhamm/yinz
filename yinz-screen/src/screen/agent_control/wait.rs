use super::*;

impl super::AgentController {
    pub(in crate::screen) fn advance_pending_controls(
        &mut self,
        host: &mut impl AgentHost,
        now: Instant,
    ) {
        self.agent_clear_inputs
            .retain(|_, input| now < input.deadline);
        if !self
            .agent_controls
            .iter()
            .any(|pending| !pending.abandoned || pending.delivery.is_some())
        {
            return;
        }
        let mut keep = Vec::new();
        let claimed: std::collections::HashMap<_, _> = self
            .agent_controls
            .iter()
            .filter_map(|pending| {
                pending
                    .not_ready_reason()
                    .map(|reason| (pending.pane, reason))
            })
            .collect();
        let mut advances = Vec::new();
        for mut pending in std::mem::take(&mut self.agent_controls) {
            for &(pane, from, to) in &advances {
                if pending.pane == pane
                    && pending.kind == "claude"
                    && pending.input_revision == from
                    && pending
                        .prompt
                        .as_ref()
                        .is_some_and(PromptObservation::acknowledged)
                {
                    pending.input_revision = to;
                }
            }
            if pending.abandoned && pending.delivery.is_none() {
                keep.push(pending);
                continue;
            }
            let mut delivery_error = None;
            let mut command_delivered = false;
            if let Some(delivery) = &mut pending.delivery {
                match delivery.try_recv() {
                    Ok(result) => {
                        pending.delivery = None;
                        delivery_error = result.error_message;
                        command_delivered = pending.command_enter_sent && delivery_error.is_none();
                        if !pending.command_enter_sent
                            && delivery_error.is_none()
                            && !pending.abandoned
                            && (pending.command
                                || pending
                                    .prompt
                                    .as_ref()
                                    .is_some_and(|prompt| !prompt.acknowledged()))
                        {
                            pending.enter_at = Some(now + pending.submit_delay);
                        }
                    },
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {},
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        pending.delivery = None;
                        delivery_error = Some("paste writer disconnected".into());
                    },
                }
            }
            if pending.resolved && pending.delivery.is_none() {
                continue;
            }
            if pending.abandoned {
                keep.push(pending);
                continue;
            }
            let status = self.agent_status(host, pending.pane, claimed.get(&pending.pane).copied());
            let submission_result = pending
                .submission_id
                .as_deref()
                .filter(|_| pending.prompt.is_none())
                .and_then(|id| self.submission(pending.pane, id))
                .and_then(|submission| submission.result.as_ref());
            let mut outcome = match &status {
                _ if command_delivered => {
                    delivery_error = Some("Command sent to the agent. Acceptance and completion are not verified; check pane output for errors or dialogs. Available commands depend on the provider.".into());
                    Some(AgentOutcome::CommandSent)
                },
                _ if submission_result.is_some() => submission_result.map(|result| result.outcome),
                _ if delivery_error.is_some() => Some(AgentOutcome::Failed),
                _ if pending.command_enter_sent && pending.deadline.is_some_and(|d| now < d) => {
                    None
                },
                None if host.tab_for_pane(pending.pane).is_none() => {
                    Some(AgentOutcome::TargetClosed)
                },
                None if !pending.session_ended => Some(AgentOutcome::TargetChanged),
                None if host.pane_info(pending.pane).is_some_and(|info| info.exited) => {
                    Some(AgentOutcome::Ok)
                },
                Some(status)
                    if status.agent.kind != pending.kind
                        || (pending.conversation.is_some()
                            && status.agent.session_id != pending.conversation) =>
                {
                    Some(AgentOutcome::TargetChanged)
                },
                Some(_)
                    if (pending.command || pending.prompt.is_some())
                        && host.agent_input_revision(pending.pane) != pending.input_revision =>
                {
                    Some(AgentOutcome::Interrupted)
                },
                Some(status)
                    if (pending.command || pending.prompt.is_some())
                        && (!status.prompt_supported
                            || !host.agent_process_matches(pending.pane, &pending.kind)) =>
                {
                    Some(AgentOutcome::StatusUnavailable)
                },
                Some(status) if status.agent.state == AgentState::Exited => Some(
                    if pending.prompt.is_none()
                        && pending.submission_id.is_none()
                        && pending.until == AgentState::Exited
                    {
                        AgentOutcome::Ok
                    } else {
                        AgentOutcome::TargetChanged
                    },
                ),
                Some(status)
                    if pending.prompt.is_none()
                        && pending.submission_id.is_none()
                        && status.status_available
                        && status.agent.state == pending.until
                        && (pending.until != AgentState::Idle
                            || !supports_prompt(&status.agent.kind)
                            // A prompt replaces unconsumed input, so it must not hold
                            // an idle wait that precedes automatic submission.
                            || ((status.ready
                                || (status.not_ready_reason
                                    == Some(NotReadyReason::UnconsumedInput)
                                    && self.prompt_replaces_input(host, pending.pane, status)))
                                && !self.claude_background_pending(pending.pane))) =>
                {
                    // The reply's message is the final text of the turn that made it idle.
                    if pending.until == AgentState::Idle {
                        delivery_error = self
                            .agent_lifecycle
                            .get(&pending.pane)
                            .and_then(|lifecycle| lifecycle.last_message.clone());
                    }
                    Some(AgentOutcome::Ok)
                },
                Some(status)
                    if pending.prompt.is_none()
                        && !pending.command
                        && status.status_available
                        && status.agent.is_waiting_on_a_person()
                        && !self
                            .agent_dialog_inputs
                            .get(&pending.pane)
                            .is_some_and(|input| {
                                !input.fresh_waiting
                                    && input.kind == status.agent.kind
                                    && input.conversation == status.agent.session_id
                            }) =>
                {
                    Some(AgentOutcome::NeedsInput)
                },
                _ if pending.deadline.is_some_and(|deadline| now >= deadline) => {
                    Some(if status.as_ref().is_some_and(|s| !s.status_available) {
                        AgentOutcome::StatusUnavailable
                    } else {
                        AgentOutcome::TimedOut
                    })
                },
                _ => None,
            };
            // Steering shares the PTY with running work. Its submit delay starts at
            // paste delivery and must not be extended by unrelated background output.
            if pending.kind == "claude"
                && !pending.steering
                && let (Some(enter), Some(output)) =
                    (pending.enter_at, host.last_output(pending.pane))
            {
                pending.enter_at = Some(enter.max(output + Duration::from_millis(100)));
            }
            if outcome.is_none() && pending.enter_at.is_some_and(|enter| now >= enter) {
                pending.enter_at = None;
                if pending.command
                    || pending
                        .prompt
                        .as_ref()
                        .is_some_and(|prompt| !prompt.acknowledged())
                {
                    if status.as_ref().is_none_or(|status| {
                        status.agent.state != AgentState::Idle
                            && !(pending.kind == "claude"
                                && !pending.command
                                && !pending.clear
                                && status.agent.state == AgentState::Busy)
                    }) || (pending.kind == "muse"
                        && pending.conversation.is_none()
                        && !providers::muse_composer(host, pending.pane, false))
                    {
                        outcome = Some(AgentOutcome::Interrupted);
                    } else {
                        let submit = if pending.command {
                            let (tx, rx) = tokio::sync::oneshot::channel();
                            pending.delivery = Some(rx);
                            pending.command_enter_sent = true;
                            host.write_key(
                                pending.pane,
                                pending.submit_key.clone(),
                                Some(yinz_bus::route::NotificationEnd::new(tx)),
                            )
                        } else {
                            host.write_key(pending.pane, pending.submit_key.clone(), None)
                        };
                        match submit {
                            Err(error) => {
                                delivery_error = Some(error.to_string());
                                outcome = Some(AgentOutcome::Failed);
                            },
                            _ => {
                                let revision = host.agent_input_revision(pending.pane);
                                advances.push((pending.pane, pending.input_revision, revision));
                                pending.input_revision = revision;
                                if pending.clear
                                    && let (Some(conversation), Some(deadline)) =
                                        (&pending.conversation, pending.deadline)
                                {
                                    self.agent_clear_inputs.insert(
                                        pending.pane,
                                        ClearInput {
                                            kind: pending.kind.clone(),
                                            conversation: conversation.clone(),
                                            input_revision: pending.input_revision,
                                            deadline,
                                            session_ended: false,
                                        },
                                    );
                                }
                            },
                        }
                    }
                }
            }
            if let Some(outcome) = outcome {
                if pending.clear
                    && matches!(
                        outcome,
                        AgentOutcome::Failed
                            | AgentOutcome::TimedOut
                            | AgentOutcome::StatusUnavailable
                    )
                {
                    self.agent_clear_inputs.remove(&pending.pane);
                }
                let retain_claim = pending.prompt.is_some()
                    && matches!(
                        outcome,
                        AgentOutcome::TimedOut
                            | AgentOutcome::StatusUnavailable
                            | AgentOutcome::Interrupted
                            | AgentOutcome::Failed
                    );
                if retain_claim {
                    pending.blocks_steering = outcome != AgentOutcome::TimedOut;
                }
                self.finish_agent_control(
                    host,
                    &pending,
                    outcome,
                    delivery_error,
                    if retain_claim {
                        pending.not_ready_reason()
                    } else if pending.prompt.is_none() {
                        claimed.get(&pending.pane).copied()
                    } else {
                        None
                    },
                );
                if retain_claim {
                    // Ending the wait cannot recall input already sent to the agent, including
                    // input followed by a manual key. Keep the claim until its lifecycle ends:
                    // otherwise an identical retry could accept the first prompt's late hook.
                    pending.client = None;
                    pending.deadline = None;
                    pending.abandoned = true;
                    pending.blocks_steering = outcome != AgentOutcome::TimedOut;
                    pending.enter_at = None;
                    keep.push(pending);
                }
            } else {
                if pending.conversation.is_none() {
                    pending.conversation = self
                        .agent_status(host, pending.pane, None)
                        .and_then(|s| s.agent.session_id);
                    // Muse can announce its first conversation before acknowledging the
                    // prompt. The receipt must accept that pin as soon as the observer does.
                    self.record_submission(&pending, None);
                }
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        for (pane, from, to) in advances {
            self.advance_claude_observers(pane, from, to);
        }
        self.prune_agent_submissions(&self.protected_submissions());
    }

    pub(in crate::screen) fn correlate_agent_submissions(
        &mut self,
        host: &impl AgentHost,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> bool {
        let pane = PaneId::Terminal(event.pane_id);
        let parent = hook.is_some_and(|hook| !hook.is_worker());
        // Passive observations and matching shutdown preserve facts; actual activity invalidates them.
        let frozen_shutdown = event.kind == "claude"
            && event.hook == "SessionEnd"
            && parent
            && self
                .closed_agent_submissions
                .iter()
                .any(|(_, proof, revision, source)| {
                    // Match the live source-admission contract for older reporters.
                    // An explicitly different process cannot reuse the snapshot.
                    let same_source = hook
                        .and_then(|h| h.claude_notifications.as_ref())
                        .and_then(|e| e.source.as_ref())
                        .is_none_or(|incoming| source.as_ref().is_none_or(|old| old == incoming));
                    proof.pane_id == pane
                        && *revision == host.agent_input_revision(pane)
                        && proof.kind == event.kind
                        && event.session_id.as_deref() == Some(proof.conversation_id.as_str())
                        && same_source
                });
        if !passive_claude_permission(event, hook)
            && !frozen_shutdown
            && !hook.is_some_and(|hook| {
                hook.hook_event_name == "Stop"
                    && hook.claude_answered == Some(true)
                    && hook.claude_background == Some(false)
                    && self
                        .agent_terminal_claims
                        .get(&pane)
                        .is_none_or(|(_, _, prompt)| hook.prompt_id.as_ref() == Some(prompt))
            })
        {
            self.agent_terminal_claims.remove(&pane);
            self.closed_agent_submissions
                .retain(|(_, proof, _, _)| proof.pane_id != pane);
        }
        let mut terminal_claim = None;
        let mut completed = false;
        let claude_claims: Vec<_> = self
            .agent_controls
            .iter()
            .filter(|pending| pending.pane == pane && pending.kind == "claude")
            .filter_map(|pending| {
                pending
                    .not_ready_reason()
                    .map(|reason| (pending.submission_id.clone(), reason))
            })
            .collect();
        let mut keep = Vec::new();
        // Capture waiters before taking the controls; result recording must not evict
        // their submissions before refresh_agent_controls delivers the results.
        let protected = self.protected_submissions();
        for mut pending in std::mem::take(&mut self.agent_controls) {
            if pending.pane != pane || pending.command {
                keep.push(pending);
                continue;
            }
            if parent
                && event.hook == "SessionEnd"
                && event.kind == pending.kind
                && pending.conversation.is_some()
                && pending.conversation == event.session_id
                && hook.as_ref().is_some_and(|hook| {
                    Some(hook.session_id.as_str()) == pending.conversation.as_deref()
                })
                && pending.prompt.is_none()
                && pending.until == AgentState::Exited
            {
                // SessionEnd clears the record before the process exits. Only this waiter
                // may survive that gap; the pane's exit flag still confirms termination.
                pending.session_ended = true;
            }
            let other_claim = claude_claims
                .iter()
                .find(|(id, _)| *id != pending.submission_id)
                .map(|(_, reason)| *reason);
            let outcome = if parent
                && event.hook == "SessionStart"
                && hook
                    .as_ref()
                    .is_none_or(|hook| hook.source.as_deref() != Some("compact"))
                && pending.conversation.is_some()
                // The same conversation can start more than once: Codex announces it when the UI
                // selects it and again when its first turn starts, and Claude repeats every hook
                // once per hook registration. Before a turn is accepted, only another
                // conversation retargets.
                && (pending.conversation != event.session_id
                    || pending
                        .prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.acknowledged()))
            {
                Some(AgentOutcome::TargetChanged)
            } else {
                pending
                    .prompt
                    .as_mut()
                    .and_then(|prompt| prompt.observe(event, hook))
            };
            completed |= outcome == Some(AgentOutcome::Completed);
            if outcome == Some(AgentOutcome::Completed)
                && parent
                && hook.is_some_and(|hook| {
                    hook.claude_answered == Some(true) && hook.claude_background == Some(false)
                })
                && let (Some(id), Some(prompt_id)) = (
                    pending.submission_id.clone(),
                    hook.and_then(|hook| hook.prompt_id.clone()),
                )
            {
                terminal_claim = Some((pending.input_revision, id, prompt_id));
            }
            if outcome == Some(AgentOutcome::Submitted) && pending.conversation.is_none() {
                pending.conversation = event.session_id.clone();
            }
            let positive_interrupt = outcome == Some(AgentOutcome::Interrupted)
                && (event.hook == "Interrupt"
                    || (event.kind == "claude"
                        && hook.is_some_and(HookEvent::is_claude_interruption)));
            let interrupted = positive_interrupt.then(|| SubmissionResult {
                outcome: AgentOutcome::Interrupted,
                message: Some(
                    format!(
                        "{} The turn was interrupted.",
                        event.last_message.as_deref().unwrap_or_default()
                    )
                    .trim()
                    .to_owned(),
                ),
                background: false,
            });
            self.record_submission(&pending, interrupted);
            match outcome {
                Some(AgentOutcome::Submitted) => {
                    pending.enter_at = None;
                    if !pending.wait {
                        self.finish_agent_control(
                            host,
                            &pending,
                            AgentOutcome::Submitted,
                            None,
                            pending.not_ready_reason().or(other_claim),
                        );
                        pending.client = None;
                    }
                    keep.push(pending);
                },
                Some(outcome) => {
                    pending.resolved =
                        matches!(outcome, AgentOutcome::Completed | AgentOutcome::Failed)
                            || (positive_interrupt && event.hook == "Stop");
                    let retain_claim = pending.delivery.is_some()
                        || (!pending.resolved
                            && matches!(
                                outcome,
                                AgentOutcome::Interrupted
                                    | AgentOutcome::StatusUnavailable
                                    | AgentOutcome::NeedsInput
                            ));
                    let message = if positive_interrupt {
                        Some(
                            format!(
                                "{} The turn was interrupted.",
                                event.last_message.as_deref().unwrap_or_default()
                            )
                            .trim()
                            .to_owned(),
                        )
                    } else {
                        event.last_message.clone()
                    };
                    if retain_claim {
                        pending.blocks_steering = outcome != AgentOutcome::NeedsInput;
                    }
                    self.finish_agent_control(
                        host,
                        &pending,
                        outcome,
                        message,
                        retain_claim
                            .then(|| pending.not_ready_reason())
                            .flatten()
                            .or(other_claim),
                    );
                    if !pending.resolved
                        && matches!(
                            outcome,
                            AgentOutcome::Interrupted
                                | AgentOutcome::StatusUnavailable
                                | AgentOutcome::NeedsInput
                        )
                    {
                        pending.client = None;
                        pending.deadline = None;
                        pending.abandoned = true;
                        pending.blocks_steering = outcome != AgentOutcome::NeedsInput;
                        pending.enter_at = None;
                        keep.push(pending);
                    } else if pending.delivery.is_some() && pending.resolved {
                        pending.resolved = true;
                        pending.abandoned = true;
                        pending.blocks_steering = true;
                        pending.client = None;
                        pending.deadline = None;
                        pending.enter_at = None;
                        keep.push(pending);
                    }
                },
                None => keep.push(pending),
            }
        }
        self.agent_controls = keep;
        if let Some(claim) = terminal_claim {
            self.agent_terminal_claims.insert(pane, claim);
        }
        self.prune_agent_submissions(&protected);
        completed
    }

    pub(in crate::screen) fn correlate_recovered_claude_submit(
        &mut self,
        host: &impl AgentHost,
        event: &AgentEvent,
        hook: &HookEvent,
    ) {
        let pane = PaneId::Terminal(event.pane_id);
        let Some(turn) = self
            .agent_lifecycle
            .get_mut(&pane)
            .filter(|l| l.kind == "claude" && l.conversation == hook.session_id)
            .and_then(|l| {
                hook.prompt_id
                    .as_ref()
                    .and_then(|id| l.claude.activity.turns.get_mut(id))
            })
            .filter(|turn| turn.awaiting_native_submit)
        else {
            return;
        };
        turn.awaiting_native_submit = false;
        if let Some(revision) = self
            .agent_controls
            .iter()
            .find(|pending| {
                pending.pane == pane
                    && pending.kind == event.kind
                    && pending
                        .prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.matches_submission(hook))
            })
            .map(|pending| pending.input_revision)
            && let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane)
        {
            // Never roll consumption back over a newer accepted input.
            lifecycle.input_revision = Some(
                lifecycle
                    .input_revision
                    .map_or(revision, |old| old.max(revision)),
            );
        }
        self.correlate_agent_submissions(host, event, Some(hook));
    }

    pub(in crate::screen) fn detach_agent_controls(
        &mut self,
        host: &impl AgentHost,
        client: ClientId,
    ) {
        for operation in self.agent_draft_clears.values_mut() {
            if matches!(operation.client, Some(Reply::Client(id)) if id == client) {
                operation.client = None;
                operation.failure = Some((AgentOutcome::Interrupted, "caller disconnected".into()));
            }
        }
        for mut pending in std::mem::take(&mut self.agent_controls) {
            if matches!(pending.client, Some(Reply::Client(id)) if id == client) {
                pending.client = None;
                if pending.prompt.is_none() {
                    continue;
                }
                if pending
                    .prompt
                    .as_ref()
                    .is_some_and(|prompt| !prompt.acknowledged())
                    && (pending.delivery.is_some() || pending.enter_at.is_some())
                {
                    self.finish_agent_control(
                        host,
                        &pending,
                        AgentOutcome::Interrupted,
                        Some("caller disconnected before submission".into()),
                        pending.not_ready_reason(),
                    );
                    pending.abandoned = true;
                    pending.blocks_steering = true;
                    pending.deadline = None;
                    pending.enter_at = None;
                }
            }
            self.agent_controls.push(pending);
        }
        self.prune_agent_submissions(&self.protected_submissions());
    }
}

impl super::AgentController {
    pub(super) fn wait_for_turn(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        client: Reply,
        now: Instant,
        submission_id: String,
        timeout_ms: u64,
        expected_session_id: Option<String>,
    ) -> Option<Completion> {
        if let Err(message) = validate_submission_id(&submission_id) {
            self.reject_control(
                host,
                &client,
                pane,
                Some(submission_id.as_str()),
                AgentOutcome::Rejected,
                message,
            );
            return None;
        }
        let Some(submission) = self.submission(pane, &submission_id) else {
            let mut result = self.agent_result(host, pane, AgentOutcome::StatusUnavailable,
        Some("submission is unknown or no longer retained; inspect the pane before retrying any prompt".into()));
            result.submission_id = Some(submission_id);
            self.agent_reply(host, &client, result);
            return None;
        };
        if expected_session_id.is_some() && expected_session_id != submission.conversation {
            self.reject_control(
                host,
                &client,
                pane,
                Some(submission_id.as_str()),
                AgentOutcome::TargetChanged,
                "submission belongs to a different conversation",
            );
            return None;
        }
        let Some(deadline) = deadline(now, timeout_ms) else {
            self.reject_control(
                host,
                &client,
                pane,
                Some(submission_id.as_str()),
                AgentOutcome::Rejected,
                "timeout must be between 1ms and 24h",
            );
            return None;
        };
        self.agent_controls.push(Pending {
            client: Some(client.clone()),
            pane,
            kind: submission.kind.clone(),
            conversation: submission.conversation.clone(),
            deadline: Some(deadline),
            input_revision: host.agent_input_revision(pane),
            prompt: None,
            command: false,
            command_enter_sent: false,
            submission_id: Some(submission_id),
            clear: false,
            steering: false,
            wait: true,
            until: AgentState::Idle,
            session_ended: false,
            abandoned: false,
            blocks_steering: false,
            resolved: false,
            delivery: None,
            enter_at: None,
            submit_key: KeyWithModifier::new(BareKey::Enter),
            submit_delay: Duration::ZERO,
        });
        Some(Completion::Refresh)
    }

    pub(super) fn wait_for_state(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
        now: Instant,
        until: AgentState,
        timeout_ms: u64,
    ) -> Option<Completion> {
        let Some(deadline) = deadline(now, timeout_ms) else {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "timeout must be between 1ms and 24h",
            );
            return None;
        };
        self.agent_controls.push(Pending {
            client: Some(client.clone()),
            pane,
            kind: status.agent.kind,
            conversation: status.agent.session_id,
            deadline: Some(deadline),
            input_revision: host.agent_input_revision(pane),
            prompt: None,
            command: false,
            command_enter_sent: false,
            submission_id: None,
            clear: false,
            steering: false,
            wait: true,
            until,
            session_ended: false,
            abandoned: false,
            blocks_steering: false,
            resolved: false,
            delivery: None,
            enter_at: None,
            submit_key: KeyWithModifier::new(BareKey::Enter),
            submit_delay: Duration::ZERO,
        });
        Some(Completion::Refresh)
    }
}
