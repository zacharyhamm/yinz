use super::*;
use yinz_types::agent::control::valid_web_identity;
pub(in crate::controller) enum Completion {
    Close(close_completed::AgentClose),
    Refresh,
    Recover {
        pane: PaneId,
        agent: AgentInfo,
        revision: u64,
        client: Reply,
    },
    Visibility {
        pane: PaneId,
        client: Reply,
    },
    Restarted {
        client: Reply,
        response: Box<AgentResponse>,
    },
}
impl super::AgentController {
    pub(in crate::controller) fn handle(
        &mut self,
        host: &mut impl AgentHost,
        request: AgentRequest,
        client: Reply,
        now: Instant,
    ) -> Option<Completion> {
        use AgentRequest::*;
        // Check the complete identity in this instruction turn, before lowering to the
        // existing control state machine (which keeps its asynchronous identity fences).
        let mut web_command = false;
        let (request, instance_matches) = match request {
            WebPeerGuarded {
                expected_instance,
                pane_id,
                expected_kind,
                expected_session_id,
                action: yinz_types::agent::control::WebAgentAction::VerifyFocus,
            } => {
                let outcome = self.verify_web_peer_focus(
                    host,
                    &expected_instance,
                    pane_id,
                    &expected_kind,
                    &expected_session_id,
                );
                self.agent_reply(host, &client, AgentResponse::new(outcome));
                return None;
            },
            WebPeerGuarded {
                expected_instance,
                pane_id,
                expected_kind,
                expected_session_id,
                action,
            } => (
                WebGuarded {
                    pane_id,
                    expected_kind,
                    expected_session_id,
                    action,
                },
                expected_instance == host.instance_id().to_string(),
            ),
            request => (request, true),
        };
        let request = match request {
            WebGuarded {
                pane_id,
                expected_kind,
                expected_session_id,
                action,
            } => {
                web_command = matches!(&action, yinz_types::agent::control::WebAgentAction::Prompt { text, .. } if Provider::for_kind(&expected_kind).web_command(text));
                let submission_id = action.submission_id().map(str::to_owned);
                let valid = yinz_types::agent::control::valid_web_identity(&expected_kind)
                    && yinz_types::agent::control::valid_web_identity(&expected_session_id);
                let matches = instance_matches
                    && valid
                    && self
                        .agent_status(host, pane_id, None)
                        .is_some_and(|status| {
                            status.agent.kind == expected_kind
                                && status.agent.session_id.as_deref()
                                    == Some(expected_session_id.as_str())
                        });
                let lowered = if matches {
                    action.lower(pane_id, expected_session_id)
                } else {
                    let mut response = AgentResponse::new(AgentOutcome::TargetChanged);
                    response.message = Some("Agent disappeared or changed identity".into());
                    response.submission_id = submission_id;
                    self.agent_reply(host, &client, response);
                    return None;
                };
                match lowered {
                    Ok(request) => request,
                    Err(message) => {
                        let mut response = AgentResponse::new(AgentOutcome::Rejected);
                        response.message = Some(message);
                        response.submission_id = submission_id;
                        self.agent_reply(host, &client, response);
                        return None;
                    },
                }
            },
            request => request,
        };
        if matches!(
            &request,
            CloseoutSupport | CloseoutSnapshot { .. } | CloseoutClose { .. }
        ) {
            return self
                .controller_closeout(host, request, &client)
                .map(Completion::Close);
        }
        let (pane, expected) = match &request {
            WebGuarded { .. } | WebPeerGuarded { .. } => unreachable!("lowered above"),
            Restart {
                pane_id,
                prompt,
                session,
            } => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                match self.restart_agent_pane(host, *pane_id, prompt.as_deref(), *session, None) {
                    Ok(restarted) => {
                        response.restarted = Some(restarted);
                        return Some(Completion::Restarted {
                            client,
                            response: Box::new(response),
                        });
                    },
                    Err(message) => {
                        response.outcome = AgentOutcome::Rejected;
                        response.message = Some(message);
                    },
                }
                self.agent_reply(host, &client, response);
                return None;
            },
            LaunchSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some(yinz_types::agent::launch::CAPABILITY.to_owned());
                response.server_instance =
                    Some(yinz_bus::agent_launch::server_instance().to_owned());
                self.agent_reply(host, &client, response);
                return None;
            },
            CloseoutSupport
            | CloseoutSnapshot { .. }
            | CloseoutClose { .. }
            | Launch { .. }
            | LaunchStatus { .. } => {
                let mut response = AgentResponse::new(AgentOutcome::Rejected);
                response.message = Some("launch transactions require the host coordinator".into());
                self.agent_reply(host, &client, response);
                return None;
            },
            SpawnStackSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-controller-stack-v1".into());
                self.agent_reply(host, &client, response);
                return None;
            },
            SpawnPlacementSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-tab-placement-v1".into());
                self.agent_reply(host, &client, response);
                return None;
            },
            SpawnSuppressedSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-suppressed-v1".into());
                self.agent_reply(host, &client, response);
                return None;
            },
            SpawnBubblewrapSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-bubblewrap-v1".into());
                self.agent_reply(host, &client, response);
                return None;
            },
            List { .. } => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.agents = host
                    .pane_ids()
                    .into_iter()
                    .filter_map(|id| {
                        self.agent_status(host, id, None)
                            .or_else(|| self.detected_agent_status(host, id))
                    })
                    .filter(|status| status.agent.state != AgentState::Exited)
                    .collect();
                self.agent_reply(host, &client, response);
                return None;
            },
            Spawn { .. } | SpawnNewProject { .. } | CreateProject { .. } | Usage { .. } => {
                self.agent_reply(host, &client, AgentResponse::new(AgentOutcome::Rejected));
                return None;
            },
            ClearDraft {
                pane_id,
                expected_session_id,
                ..
            }
            | Recover {
                pane_id,
                expected_session_id,
                ..
            } => (
                *pane_id,
                (!expected_session_id.is_empty()).then_some(expected_session_id.as_str()),
            ),
            CloseCompleted {
                pane_id,
                expected_session_id,
                ..
            }
            | Status {
                pane_id,
                expected_session_id,
            }
            | Show {
                pane_id,
                expected_session_id,
            }
            | Hide {
                pane_id,
                expected_session_id,
            }
            | Read {
                pane_id,
                expected_session_id,
                ..
            }
            | SendKeys {
                pane_id,
                expected_session_id,
                ..
            }
            | Prompt {
                pane_id,
                expected_session_id,
                ..
            }
            | Wait {
                pane_id,
                expected_session_id,
                ..
            }
            | WaitTurn {
                pane_id,
                expected_session_id,
                ..
            } => (*pane_id, expected_session_id.as_deref()),
        };
        let requested_submission = match &request {
            WaitTurn { submission_id, .. } => Some(submission_id.clone()),
            _ => None,
        };
        let reject = |screen: &Self, outcome, message: &str| {
            screen.reject_control(
                host,
                &client,
                pane,
                requested_submission.as_deref(),
                outcome,
                message,
            )
        };
        if !matches!(pane, PaneId::Terminal(_)) {
            reject(
                self,
                AgentOutcome::Rejected,
                "target must be a terminal pane",
            );
            return None;
        }
        if let WaitTurn {
            submission_id,
            timeout_ms,
            ..
        } = &request
        {
            if let Err(message) = validate_submission_id(submission_id) {
                reject(self, AgentOutcome::Rejected, message);
                return None;
            }
            if deadline(now, *timeout_ms).is_none() {
                reject(
                    self,
                    AgentOutcome::Rejected,
                    "timeout must be between 1ms and 24h",
                );
                return None;
            }
            self.prune_closed_agents(now);
            if self
                .agent_status(host, pane, None)
                .is_none_or(|status| status.agent.state == AgentState::Exited)
                && let Some((_, proof, _, _)) =
                    self.closed_agent_submissions
                        .iter()
                        .rev()
                        .find(|(_, proof, revision, _)| {
                            (host.pane_info(pane).is_none()
                                || *revision == host.agent_input_revision(pane))
                                && proof.pane_id == pane
                                && proof.submission_id == *submission_id
                                && expected == Some(proof.conversation_id.as_str())
                        })
            {
                let mut response = AgentResponse::new(AgentOutcome::Completed);
                response.submission_id = Some(proof.submission_id.clone());
                response.turn_id = proof.turn_id.clone();
                response.acknowledged = true;
                response.prompt_delivery = Some(proof.prompt_delivery);
                response.message = proof.message.clone();
                response.retained_submission = Some(proof.clone());
                self.agent_reply(host, &client, response);
                return None;
            }
        }
        let status = self.agent_status(host, pane, None);
        // SessionEnd clears the live record before a managed command enters its held state.
        let held_conversation = matches!(request, Show { .. } | Hide { .. })
            .then(|| host.held_agent_conversation(pane))
            .flatten();
        let Some(conversation) = status
            .as_ref()
            .map(|s| s.agent.session_id.as_deref())
            .or(held_conversation)
        else {
            // Between a submitted `/clear` or `/new` ending the old conversation and the new
            // one starting there is no record. An unpinned wait then waits for the new one.
            if let (
                None,
                Wait {
                    until, timeout_ms, ..
                },
            ) = (expected, &request)
            {
                let clearing = self
                    .agent_clear_inputs
                    .get(&pane)
                    .filter(|input| input.session_ended && now < input.deadline)
                    .map(|input| input.kind.clone());
                if let (Some(kind), Some(deadline)) = (clearing, deadline(now, *timeout_ms)) {
                    self.agent_controls.push(Pending {
                        client: Some(client.clone()),
                        pane,
                        kind,
                        conversation: None,
                        deadline: Some(deadline),
                        input_revision: host.agent_input_revision(pane),
                        prompt: None,
                        command: false,
                        command_enter_sent: false,
                        submission_id: None,
                        clear: false,
                        steering: false,
                        wait: true,
                        until: *until,
                        session_ended: true,
                        abandoned: false,
                        blocks_steering: false,
                        resolved: false,
                        delivery: None,
                        enter_at: None,
                        submit_key: KeyWithModifier::new(BareKey::Enter),
                        submit_delay: Duration::ZERO,
                    });
                    return None;
                }
            }
            let info = host.pane_info(pane);
            let exited = info.as_ref().is_some_and(|info| info.exited) && host.pane_is_agent(pane);
            let (outcome, message) = if info.is_none() {
                (AgentOutcome::TargetNotFound, "target pane does not exist")
            } else if exited {
                (AgentOutcome::TargetExited, "agent process has exited")
            } else if self.detected_agent_status(host, pane).is_some() {
                (
                    AgentOutcome::NotAgent,
                    "target runs an agent that Yinz did not launch; relaunch it with agent spawn to control it",
                )
            } else {
                (AgentOutcome::NotAgent, "target does not contain an agent")
            };
            // A conversation pin supplies a prior identity; an unpinned lookup does not.
            let outcome = if expected.is_some() {
                if info.is_none() {
                    AgentOutcome::TargetClosed
                } else {
                    AgentOutcome::TargetChanged
                }
            } else {
                outcome
            };
            reject(self, outcome, message);
            return None;
        };
        if expected.is_some() && expected != conversation {
            reject(
                self,
                AgentOutcome::TargetChanged,
                "agent conversation changed",
            );
            return None;
        }
        if matches!(request, Show { .. } | Hide { .. }) {
            let suppress = matches!(request, Hide { .. });
            if let Err(message) = host.set_suppressed(pane, suppress) {
                self.reject_control(host, &client, pane, None, AgentOutcome::Rejected, message);
                return None;
            }
            return Some(Completion::Visibility { client, pane });
        }
        let status = status?;
        if web_command
            && let Prompt {
                submission_id: Some(submission_id),
                ..
            } = &request
            && let Some(submission) = self.submission(pane, submission_id)
        {
            let mut response = self.agent_result(
                host,
                pane,
                AgentOutcome::TimedOut,
                Some(
                    "Command delivery is still uncertain; inspect the pane. No input was resent."
                        .into(),
                ),
            );
            response.submission_id = Some(submission_id.clone());
            response.prompt_delivery = submission.prompt_delivery;
            if let Some(result) = &submission.result {
                response.outcome = result.outcome;
                response.message = result.message.clone();
            }
            self.agent_reply(host, &client, response);
            return None;
        }
        if self.agent_draft_clears.contains_key(&pane)
            && matches!(request, Prompt { .. } | Recover { .. } | ClearDraft { .. })
        {
            reject(
                self,
                AgentOutcome::Rejected,
                "the composer is being cleared",
            );
            return None;
        }
        match request {
            request @ ClearDraft { .. } => {
                self.start_draft_clear(host, pane, status, client, request, now)
            },
            Recover {
                expected_session_id,
                confirm_empty,
                interrupted,
                ..
            } => {
                return self.recover_agent(
                    host,
                    pane,
                    client,
                    status,
                    now,
                    expected_session_id,
                    confirm_empty,
                    interrupted,
                );
            },
            CloseCompleted {
                expected_session_id,
                submission_id,
                ..
            } => {
                if let Some(close) = self.close_completed(
                    host,
                    pane,
                    client,
                    status,
                    expected_session_id,
                    submission_id,
                ) {
                    return Some(Completion::Close(close));
                }
            },
            Status { .. } => {
                self.reply_status(host, pane, client, status);
            },
            Read { lines, .. } => {
                self.read_agent(host, pane, client, lines);
            },
            SendKeys { keys, .. } => {
                self.send_agent_keys(host, pane, client, status, keys);
            },
            Prompt {
                preserve_draft,
                prompt,
                wait,
                timeout_ms,
                submission_id,
                ..
            } => {
                self.submit_prompt(
                    host,
                    pane,
                    client,
                    status,
                    now,
                    web_command,
                    preserve_draft,
                    prompt,
                    wait,
                    timeout_ms,
                    submission_id,
                );
            },
            WaitTurn {
                submission_id,
                timeout_ms,
                expected_session_id,
                ..
            } => {
                return self.wait_for_turn(
                    host,
                    pane,
                    client,
                    now,
                    submission_id,
                    timeout_ms,
                    expected_session_id,
                );
            },
            Wait {
                until, timeout_ms, ..
            } => {
                return self.wait_for_state(host, pane, client, status, now, until, timeout_ms);
            },
            List { .. }
            | Usage { .. }
            | Show { .. }
            | Hide { .. }
            | Restart { .. }
            | Spawn { .. }
            | SpawnNewProject { .. }
            | CreateProject { .. }
            | Launch { .. }
            | LaunchStatus { .. }
            | LaunchSupport
            | SpawnStackSupport
            | SpawnPlacementSupport
            | SpawnSuppressedSupport
            | CloseoutSupport
            | CloseoutSnapshot { .. }
            | CloseoutClose { .. }
            | SpawnBubblewrapSupport
            | WebGuarded { .. }
            | WebPeerGuarded { .. } => {},
        }
        None
    }
    fn controller_closeout(
        &self,
        host: &impl AgentHost,
        request: AgentRequest,
        client: &Reply,
    ) -> Option<close_completed::AgentClose> {
        use yinz_types::agent::control::CLOSEOUT_CAPABILITY;
        let mut response = AgentResponse::new(AgentOutcome::Ok);
        response.server_instance = Some(yinz_bus::agent_launch::server_instance().to_owned());
        let (pane, expected) = match request {
            AgentRequest::CloseoutSupport => {
                response.output = Some(CLOSEOUT_CAPABILITY.into());
                self.agent_reply(host, client, response);
                return None;
            },
            AgentRequest::CloseoutSnapshot { pane_id } => (pane_id, None),
            AgentRequest::CloseoutClose { fence } => (fence.pane_id, Some(fence)),
            _ => return None,
        };
        if host.tab_for_pane(pane).is_none() {
            response.outcome = AgentOutcome::TargetNotFound;
        } else {
            match self.closeout_fence(host, pane) {
                Err(message) => {
                    response.outcome = AgentOutcome::Rejected;
                    response.message = Some(message);
                },
                Ok(fence) => {
                    if let Some(expected) = expected {
                        if expected != fence {
                            response.outcome = AgentOutcome::TargetChanged;
                            response.message = Some("Controller identity, input or turn changed; obtain a fresh closeout offer and consent".into());
                        } else {
                            return Some(close_completed::AgentClose::controller(
                                pane,
                                client.clone(),
                            ));
                        }
                    } else {
                        response.closeout_target = Some(fence);
                    }
                },
            }
        }
        self.agent_reply(host, client, response);
        None
    }
    pub(in crate::controller) fn verify_web_peer_focus(
        &self,
        host: &impl AgentHost,
        expected_instance: &str,
        pane_id: PaneId,
        kind: &str,
        conversation: &str,
    ) -> AgentOutcome {
        if expected_instance != host.instance_id().to_string()
            || !valid_web_identity(kind)
            || !valid_web_identity(conversation)
        {
            return AgentOutcome::TargetChanged;
        }
        let Some(pane) = host.pane_info(pane_id) else {
            return AgentOutcome::TargetNotFound;
        };
        let Some(agent) = pane.agent else {
            return AgentOutcome::TargetChanged;
        };
        if agent.kind != kind || agent.session_id.as_deref() != Some(conversation) {
            return AgentOutcome::TargetChanged;
        }
        if pane.exited || agent.state == AgentState::Exited {
            return AgentOutcome::TargetExited;
        }
        if !pane.is_selectable || pane.is_suppressed {
            return AgentOutcome::Rejected;
        }
        AgentOutcome::Ok
    }
}
