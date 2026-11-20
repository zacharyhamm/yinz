use super::*;

impl super::AgentController {
    pub(in crate::controller) fn deliver_agent_prompt(
        &mut self,
        host: &mut impl AgentHost,
        mut pending: Pending,
        prefix: Vec<u8>,
        paste: Vec<u8>,
    ) {
        let pane = pending.pane;
        // Mark the attempt before any write, including a typed prefix. Even a failed
        // write may have delivered bytes and must never become non-delivery evidence.
        if let Some(submission) = pending
            .submission_id
            .as_deref()
            .and_then(|id| self.submission_mut(pane, id))
        {
            submission.prompt_delivery = Some(PromptDelivery::Started);
        }
        let previous_revision = host.agent_input_revision(pane);
        let kind = pending.kind.clone();
        let command = pending.command;
        let (tx, rx) = tokio::sync::oneshot::channel();
        pending.delivery = Some(rx);
        self.agent_controls.push(pending);
        // Codex reads quickly typed text followed by a paste as one burst, and an empty
        // paste there discards the typed command, so a bare command is only typed.
        let result = if command {
            host.write_command(
                pane,
                prefix,
                paste,
                yinz_bus::route::NotificationEnd::new(tx),
            )
        } else if paste.is_empty() && Provider::for_kind(&kind).type_empty_paste() {
            host.write_bytes(
                pane,
                prefix,
                Some(yinz_bus::route::NotificationEnd::new(tx)),
            )
        } else {
            let typed = (!prefix.is_empty()).then(|| host.write_bytes(pane, prefix, None));
            match typed {
                Some(Err(error)) => Err(error),
                _ => host.paste(pane, paste, Some(yinz_bus::route::NotificationEnd::new(tx))),
            }
        };
        let revision = host.agent_input_revision(pane);
        self.advance_claude_observers(pane, previous_revision, revision);
        if let Some(pending) = self.agent_controls.last_mut() {
            pending.input_revision = revision;
        }
        if let Err(error) = result {
            let Some(mut pending) = self.agent_controls.pop() else {
                return;
            };
            self.finish_agent_control(
                host,
                &pending,
                AgentOutcome::Failed,
                Some(error.to_string()),
                pending.not_ready_reason(),
            );
            pending.abandoned = true;
            pending.blocks_steering = true;
            pending.client = None;
            pending.deadline = None;
            self.agent_controls.push(pending);
        }
    }

    pub(in crate::controller) fn start_draft_clear(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
        status: AgentStatus,
        client: Reply,
        request: AgentRequest,
        now: Instant,
    ) {
        let AgentRequest::ClearDraft {
            expected_session_id,
            input_revision,
            confirm_discard,
            interrupted,
            timeout_ms,
            ..
        } = request
        else {
            return;
        };
        let refusal = if !confirm_discard || expected_session_id.is_empty() {
            Some("clearing requires --conversation and --confirm-discard")
        } else if input_revision != host.agent_input_revision(pane) {
            Some("input revision changed; inspect the draft and obtain fresh discard authorization")
        } else if !(1..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            Some("timeout must be between 1ms and 24h")
        } else if !status.prompt_supported {
            Some("clearing requires a managed live agent with terminal input and bracketed paste enabled")
        } else if !matches!(status.agent.kind.as_str(), "claude" | "codex" | "pi") {
            Some("provider does not support clearing; clear the composer manually, then use recover --confirm-empty")
        } else if self.agent_controls.iter().any(|pending| {
            pending.pane == pane
                && pending.prompt.is_some()
                && (!pending.abandoned || pending.delivery.is_some())
        }) {
            Some("a submission or paste write is still in flight")
        } else if status.background || self.claude_background_pending(pane) {
            Some("background work is still running")
        } else if host
            .agent_input_at(pane)
            .is_some_and(|at| now.saturating_duration_since(at) < RECOVER_QUIET)
        {
            Some("clearing requires three seconds without terminal input; inspect again before retrying")
        } else if host
            .agent_input_at(pane)
            .is_some_and(|at| host.last_output(pane).is_none_or(|output| output <= at))
        {
            Some("no fresh composer output after the latest input; inspect again before retrying")
        } else if status.agent.state != AgentState::Idle
            && (!interrupted
                || !matches!(status.agent.state, AgentState::Busy | AgentState::Waiting))
        {
            Some("clearing requires idle state, or --interrupted after verifying that work stopped")
        } else if status.agent.state != AgentState::Idle
            && !self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                host.agent_input_at(pane)
                    .is_some_and(|at| at > lifecycle.last_report)
            })
        {
            Some("interrupted recovery requires input after the last lifecycle report and three seconds of quiet")
        } else {
            None
        };
        if let Some(message) = refusal {
            self.agent_reply(
                host,
                &client,
                self.agent_result(host, pane, AgentOutcome::Rejected, Some(message.into())),
            );
            return;
        }
        let Some(_) = providers::draft_composer(host, pane, &status.agent.kind) else {
            self.agent_reply(host, &client, self.agent_result(host, pane, AgentOutcome::Rejected, Some("unsupported or hidden composer layout; inspect and clear manually, then use recover --confirm-empty".into())));
            return;
        };
        self.begin_draft_clear(
            host,
            pane,
            status.agent,
            client,
            now,
            now + Duration::from_millis(timeout_ms),
            None,
        );
    }

    pub(in crate::controller) fn begin_draft_clear(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
        agent: AgentInfo,
        client: Reply,
        now: Instant,
        deadline: Instant,
        prompt: Option<(Pending, Vec<u8>, Vec<u8>, KeyWithModifier)>,
    ) {
        let (prompt, stage, marker_required) =
            if let Some((pending, prefix, paste, clear_key)) = prompt {
                (
                    Some((pending, prefix, paste)),
                    DraftStage::PromptMarker {
                        clear_key,
                        since: None,
                    },
                    true,
                )
            } else {
                let Some(composer) = providers::draft_composer(host, pane, &agent.kind) else {
                    return;
                };
                (
                    None,
                    if composer.empty {
                        DraftStage::Empty { since: Some(now) }
                    } else {
                        DraftStage::Marker
                    },
                    !composer.empty,
                )
            };
        let Some(lifecycle) = self.agent_lifecycle.get(&pane) else {
            return;
        };
        let mut operation = DraftClear {
            client: Some(client),
            agent,
            revision: host.agent_input_revision(pane),
            report: lifecycle.last_report,
            deadline,
            marker: format!("yz{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            stage,
            wrote_at: None,
            delivery: None,
            failure: None,
            prompt,
        };
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.input_revision = None;
        }
        if marker_required {
            let bytes = format!("\x1b[200~{}\x1b[201~", operation.marker).into_bytes();
            self.write_draft_stage(host, pane, &mut operation, None, bytes, now);
        }
        self.agent_draft_clears.insert(pane, operation);
    }

    pub(in crate::controller) fn write_draft_stage(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
        operation: &mut DraftClear,
        key: Option<KeyWithModifier>,
        bytes: Vec<u8>,
        now: Instant,
    ) {
        let previous_revision = operation.revision;
        let (tx, rx) = tokio::sync::oneshot::channel();
        operation.delivery = Some(rx);
        operation.wrote_at = Some(now);
        let completion = Some(yinz_bus::route::NotificationEnd::new(tx));
        let result = if let Some(key) = key {
            host.write_key(pane, key, completion)
        } else {
            host.write_bytes(pane, bytes, completion)
        };
        if let Err(error) = result {
            operation.failure = Some((AgentOutcome::Failed, error.to_string()));
        }
        operation.revision = host.agent_input_revision(pane);
        if operation
            .prompt
            .as_ref()
            .is_some_and(|(pending, _, _)| pending.steering)
        {
            self.advance_claude_observers(pane, previous_revision, operation.revision);
        }
    }

    pub(in crate::controller) fn prompt_replaces_input(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
        status: &AgentStatus,
    ) -> bool {
        !(status.agent.kind == "muse"
            && status.agent.session_id.is_none()
            && !providers::muse_composer(host, pane, false))
    }
}

impl super::AgentController {
    pub(super) fn verify_agent_input(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
        status: &AgentStatus,
    ) -> Result<(bool, bool), (AgentOutcome, &'static str)> {
        // An unpinned Muse pane may still show a startup dialog, but its
        // bootstrap cannot prove that subsequent keys were consumed.
        let provisional = supports_prompt(&status.agent.kind)
            && status.agent.kind == "muse"
            && status.agent.session_id.is_none()
            && self.has_provisional_muse_lifecycle(pane);
        let startup = provisional
            || (supports_prompt(&status.agent.kind) && status.agent.source == AgentSource::Spawned);
        if !(status.status_available || startup)
            || !host.agent_process_matches(pane, &status.agent.kind)
        {
            return Err((
                AgentOutcome::StatusUnavailable,
                "cannot verify a live agent process",
            ));
        }
        if !host.agent_input_enabled(pane) {
            return Err((AgentOutcome::Rejected, "agent terminal input is disabled"));
        }
        if status.agent.state == AgentState::Exited
            || host
                .pane_info(pane)
                .is_none_or(|info| info.is_held || info.exited)
        {
            return Err((AgentOutcome::TargetChanged, "agent process has exited"));
        }
        Ok((startup, provisional))
    }

    pub(in crate::controller) fn send_agent_keys(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
        keys: Vec<KeyWithModifier>,
    ) {
        let (startup, provisional) = match self.verify_agent_input(host, pane, &status) {
            Ok(state) => state,
            Err((outcome, message)) => {
                self.reject_control(host, &client, pane, None, outcome, message);
                return;
            },
        };
        if keys.is_empty() || keys.len() > 1024 {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "send between 1 and 1024 keys",
            );
            return;
        }
        let revision = host.agent_input_revision(pane);
        let pre_submit_dialog = self.pi_pre_submit_dialog(pane, &status.agent);
        let dialog = ((startup && !(provisional && providers::muse_composer(host, pane, false)))
            || status.agent.state == AgentState::Waiting)
            && (self
                .agent_lifecycle
                .get(&pane)
                .is_some_and(|lifecycle| lifecycle.input_revision == Some(revision))
                || (startup && revision == 0)
                || pre_submit_dialog
                || self.agent_dialog_inputs.get(&pane).is_some_and(|input| {
                    input.kind == status.agent.kind
                        && input.conversation == status.agent.session_id
                        && input.input_revision == revision
                }))
            && !self.agent_controls.iter().any(|pending| {
                pending.pane == pane
                    && pending.prompt.as_ref().is_some_and(|prompt| {
                        (!prompt.acknowledged() && !pre_submit_dialog)
                            || pending.delivery.is_some()
                            || pending.enter_at.is_some()
                            || pending.input_revision != revision
                            || pending.kind != status.agent.kind
                            || pending.conversation != status.agent.session_id
                    })
            });
        let result = keys
            .into_iter()
            .try_for_each(|key| host.write_key(pane, key, None));
        match result {
            Ok(()) => {
                let input_revision = host.agent_input_revision(pane);
                if dialog && input_revision > revision {
                    self.agent_dialog_inputs.insert(
                        pane,
                        DialogInput {
                            kind: status.agent.kind.clone(),
                            conversation: status.agent.session_id.clone(),
                            input_revision,
                            startup,
                            fresh_waiting: false,
                        },
                    );
                    // Dialog answers preserve the pending prompt, including Pi dialogs
                    // before submission. The checks above exclude incomplete delivery
                    // and intervening manual input.
                    for pending in &mut self.agent_controls {
                        if pending.pane == pane && pending.prompt.is_some() {
                            pending.input_revision = input_revision;
                        }
                    }
                }
                self.agent_reply(
                    host,
                    &client,
                    self.agent_result(host, pane, AgentOutcome::Ok, None),
                )
            },
            Err(error) => {
                self.agent_dialog_inputs.remove(&pane);
                self.reject_control(
                    host,
                    &client,
                    pane,
                    None,
                    AgentOutcome::Failed,
                    &error.to_string(),
                );
            },
        }
    }

    pub(in crate::controller) fn submit_prompt(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
        client: Reply,
        status: AgentStatus,
        now: Instant,
        web_command: bool,
        preserve_draft: bool,
        prompt: String,
        wait: bool,
        timeout_ms: u64,
        submission_id: Option<String>,
    ) {
        let submission_id = submission_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Err(message) = validate_submission_id(&submission_id) {
            self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, message);
            return;
        }
        if self.submission(pane, &submission_id).is_some() {
            let mut response = self.agent_result(host, pane, AgentOutcome::Rejected,
        Some(format!("submission ID is already retained; inspect its outcome with agent wait {pane} --submission {submission_id}; no input was sent")));
            response.submission_id = Some(submission_id);
            self.agent_reply(host, &client, response);
            return;
        }
        let command_input = if web_command {
            match yinz_types::agent::control::web_slash_input(&status.agent.kind, &prompt) {
                Ok(input) => Some(input),
                Err(message) => {
                    self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, message);
                    return;
                },
            }
        } else {
            None
        };
        if let Some(reason) = status
            .not_ready_reason
            .filter(|reason| *reason != NotReadyReason::UnconsumedInput)
        {
            let (outcome, message) = match reason {
        NotReadyReason::ClearingDraft => (AgentOutcome::Rejected, "the composer is being cleared"),
        NotReadyReason::PromptUnsupported => (AgentOutcome::StatusUnavailable,
            "prompt requires lifecycle hooks and a live supported agent command pane with bracketed paste enabled"),
        NotReadyReason::NotIdle => (AgentOutcome::Rejected, "agent is not idle"),
        NotReadyReason::PendingSubmission => (AgentOutcome::Rejected, "agent has an unresolved pending submission"),
        NotReadyReason::UnresolvedTurn => (AgentOutcome::Rejected, "agent has an unresolved accepted turn; inspect the pane and recover only after it has stopped and its draft is empty"),
        NotReadyReason::UnconsumedInput => (AgentOutcome::Rejected, "agent has unconsumed terminal input; inspect the pane before submitting"),
        // Unreachable: a detected agent has no record, so the lookup above refuses it.
        NotReadyReason::Unmanaged => (AgentOutcome::NotAgent, "target runs an agent that Yinz did not launch; relaunch it with agent spawn to control it"),
    };
            self.reject_control(host, &client, pane, None, outcome, message);
            return;
        }
        // Muse's bootstrap precedes workspace trust; only its composer proves startup finished.
        if !self.prompt_replaces_input(host, pane, &status) {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "Muse startup is incomplete; finish its setup dialogs before submitting",
            );
            return;
        }
        let adapter = Provider::for_kind(&status.agent.kind);
        let trimmed = adapter.trimmed(&prompt);
        if trimmed.is_empty()
            || prompt.len() > MAX_PROMPT_BYTES
            || prompt
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "prompt must contain text, be at most 1 MiB, and contain no terminal controls",
            );
            return;
        }
        let Some(deadline) = deadline(now, timeout_ms) else {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "timeout must be between 1ms and 24h",
            );
            return;
        };
        let provider = yinz_types::agent::provider::provider(&status.agent.kind);
        let input = provider
            .as_ref()
            .and_then(|provider| provider.input.as_ref());
        if !web_command && providers::is_command(&status.agent.kind, input, &prompt) {
            self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, "command input requires agent send-keys; prompt accepts text, exact Claude /clear, and exact Codex/Pi /new");
            return;
        }
        let submit_key = match input
            .map_or("Enter", |input| input.submit_key.as_str())
            .parse()
        {
            Ok(key) => key,
            Err(_) => {
                self.reject_control(
                    host,
                    &client,
                    pane,
                    None,
                    AgentOutcome::StatusUnavailable,
                    "agent provider has an invalid submit key",
                );
                return;
            },
        };
        let submit_delay = Duration::from_millis(input.map_or_else(
            || adapter.default_submit_delay_ms(),
            |input| input.submit_delay_ms,
        ));
        let observation = PromptObservation::with_input(
            &status.agent.kind,
            status.agent.session_id.as_deref().unwrap_or_default(),
            &prompt,
            input,
        );
        if !observation.supported() {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::StatusUnavailable,
                "agent provider no longer supports prompt input",
            );
            return;
        }
        let prompt = adapter.normalize(prompt);
        if prompt.trim().is_empty() {
            self.reject_control(
                host,
                &client,
                pane,
                None,
                AgentOutcome::Rejected,
                "prompt must contain text after Claude format-character normalization",
            );
            return;
        }
        let reset = yinz_types::agent::control::is_conversation_reset(&status.agent.kind, &prompt);
        if (reset || web_command)
            && (status.agent.state != AgentState::Idle
                || self.claude_background_pending(pane)
                || self
                    .agent_controls
                    .iter()
                    .any(|pending| pending.pane == pane && pending.prompt.is_some()))
        {
            self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, "commands and conversation resets require an inactive agent with no outstanding submissions");
            return;
        }
        let steering = self.claude_steering(pane, &status.agent);
        if steering && preserve_draft && !self.claude_steering_composer(host, pane) {
            self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, "preserving the draft requires a verified empty composer; retry without preserve-draft to replace it");
            return;
        }
        let clear_key = match input
            .map_or("Ctrl c", |input| input.clear_key.as_str())
            .parse::<KeyWithModifier>()
        {
            Ok(key) => key,
            _ => {
                self.reject_control(
                    host,
                    &client,
                    pane,
                    None,
                    AgentOutcome::StatusUnavailable,
                    "agent provider has an invalid clear key",
                );
                return;
            },
        };
        let protected = self.protected_submissions();
        let submissions = self.agent_submissions.entry(pane).or_default();
        submissions.push_back(Submission {
            prompt_delivery: None,
            id: submission_id.clone(),
            kind: status.agent.kind.clone(),
            conversation: status.agent.session_id.clone(),
            acknowledged: false,
            turn_id: None,
            result: None,
        });
        prune_submissions(submissions, protected.get(&pane));
        // Claim and observe before input; hooks cannot overtake this screen instruction.
        let pending = Pending {
            client: Some(client.clone()),
            pane,
            kind: status.agent.kind.clone(),
            conversation: status.agent.session_id.clone(),
            deadline: Some(deadline),
            input_revision: host.agent_input_revision(pane),
            prompt: (!web_command).then_some(observation),
            command: web_command,
            command_enter_sent: false,
            submission_id: Some(submission_id),
            clear: !web_command
                && yinz_types::agent::control::is_conversation_reset(&status.agent.kind, &prompt),
            steering,
            wait,
            until: AgentState::Idle,
            session_ended: false,
            abandoned: false,
            blocks_steering: false,
            resolved: false,
            delivery: None,
            enter_at: None,
            submit_key,
            submit_delay,
        };
        // Expose Claude's slash command before its arguments can collapse into a chip,
        // and type Codex's `/new`: a pasted command is draft text, not a command.
        // Keep whitespace in the paste: typing a newline or tab would send a key.
        let (prefix, paste) = command_input
            .unwrap_or_else(|| providers::encode_prompt(&status.agent.kind, input, prompt));
        if steering && preserve_draft {
            self.deliver_agent_prompt(host, pending, prefix, paste);
            return;
        }
        self.begin_draft_clear(
            host,
            pane,
            status.agent,
            client,
            now,
            deadline,
            Some((pending, prefix, paste, clear_key)),
        );
    }
}
