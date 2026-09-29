//! Agent control is serialized with state reports and terminal input on the screen thread.
use super::Screen;
use crate::ClientId;
use std::time::{Duration, Instant};
use yinz_ipc::ipc::ServerToClientMsg;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{
    AgentOutcome, AgentRequest, AgentResponse, AgentStatus, PromptObservation,
};
use yinz_types::agent::{AgentEvent, AgentSource, AgentState, HookEvent};
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_id::PaneId;

pub(super) struct Lifecycle {
    kind: String,
    conversation: String,
    input_revision: Option<u64>,
}

pub(super) struct Pending {
    client: Option<ClientId>,
    pane: PaneId,
    kind: String,
    conversation: Option<String>,
    pub(super) deadline: Option<Instant>,
    input_revision: u64,
    prompt: Option<PromptObservation>,
    wait: bool,
    until: AgentState,
    session_ended: bool,
}

impl Screen {
    fn agent_input_revision(&self, pane: PaneId) -> u64 {
        self.tab_containing_pane(pane)
            .map(|tab| tab.agent_input_revision(pane))
            .unwrap_or(0)
    }

    fn agent_status(&self, pane_id: PaneId) -> Option<AgentStatus> {
        let info = self.get_pane_info(pane_id)?;
        let agent = info.agent?;
        let status_available = agent.source == AgentSource::Reported
            && self.agent_lifecycle.get(&pane_id).is_some_and(|lifecycle| {
                lifecycle.kind == agent.kind
                    && Some(&lifecycle.conversation) == agent.session_id.as_ref()
            });
        let live = !info.is_held
            && !info.exited
            && agent.state != AgentState::Exited
            && self.agent_input_enabled(pane_id);
        let supported = matches!(agent.kind.as_str(), "claude" | "codex");
        let prompt_supported = supported
            && status_available
            && live
            && self.agent_process_matches(pane_id, &agent.kind)
            && self
                .tab_containing_pane(pane_id)
                .and_then(|tab| tab.get_pane_with_id(pane_id))
                .is_some_and(|pane| pane.supports_bracketed_paste());
        let cwd = agent.cwd.clone().or_else(|| match pane_id {
            PaneId::Terminal(id) => self
                .terminal_cwds
                .get(&id)
                .map(|p| p.to_string_lossy().into_owned()),
            _ => None,
        });
        Some(AgentStatus {
            pane_id,
            session: self.session_name.to_string(),
            cwd,
            branch: self
                .agent_launches
                .get(&pane_id)
                .and_then(|launch| launch.branch.clone()),
            ready: prompt_supported
                && agent.state == AgentState::Idle
                && self.agent_lifecycle.get(&pane_id).is_some_and(|lifecycle| {
                    lifecycle.input_revision == Some(self.agent_input_revision(pane_id))
                })
                && !self
                    .agent_controls
                    .iter()
                    .any(|pending| pending.pane == pane_id && pending.prompt.is_some()),
            status_available,
            prompt_supported,
            agent,
        })
    }

    // A stale hook record in a shell is insufficient authority to type into that shell.
    fn agent_process_matches(&self, pane: PaneId, kind: &str) -> bool {
        self.tab_containing_pane(pane)
            .and_then(|tab| tab.pane_run_command(pane))
            .and_then(|run| yinz_types::agent::kind_from_program_and_args(&run.command, &run.args))
            == yinz_types::agent::kind_of(kind)
            && yinz_types::agent::kind_of(kind).is_some()
    }

    fn agent_input_enabled(&self, pane: PaneId) -> bool {
        self.tab_containing_pane(pane).is_some_and(|tab| {
            tab.get_pane_with_id(pane).is_some_and(|pane| {
                !pane.is_suspended() && !(tab.is_sync_panes_active() && pane.exclude_from_sync())
            })
        })
    }

    fn agent_reply(&self, client: ClientId, response: AgentResponse) {
        if let Some(os) = &self.bus.os_input {
            os.send_to_client(client, ServerToClientMsg::AgentControl(response))
                .context("reply to agent command")
                .non_fatal();
        }
    }

    fn agent_result(
        &self,
        pane: PaneId,
        outcome: AgentOutcome,
        message: Option<String>,
    ) -> AgentResponse {
        let mut response = AgentResponse::new(outcome);
        response.agents.extend(self.agent_status(pane));
        response.message = message;
        response
    }

    pub(super) fn agent_control(&mut self, request: AgentRequest, client: ClientId) {
        use AgentRequest::*;
        let (pane, expected) = match &request {
            List => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.agents = self
                    .tabs
                    .values()
                    .flat_map(|tab| tab.get_all_pane_ids())
                    .filter_map(|id| self.agent_status(id))
                    .filter(|status| status.agent.state != AgentState::Exited)
                    .collect();
                self.agent_reply(client, response);
                return;
            },
            Spawn { .. } => {
                self.agent_reply(client, AgentResponse::new(AgentOutcome::Rejected));
                return;
            },
            Status {
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
            } => (*pane_id, expected_session_id.as_deref()),
        };
        let reject = |screen: &Self, outcome, message: &str| {
            screen.agent_reply(
                client,
                screen.agent_result(pane, outcome, Some(message.into())),
            );
        };
        if !matches!(pane, PaneId::Terminal(_)) {
            reject(
                self,
                AgentOutcome::Rejected,
                "target must be a terminal pane",
            );
            return;
        }
        let Some(status) = self.agent_status(pane) else {
            let outcome = if self.tab_containing_pane(pane).is_some() {
                AgentOutcome::TargetChanged
            } else {
                AgentOutcome::TargetClosed
            };
            reject(self, outcome, "target does not contain an agent");
            return;
        };
        if expected.is_some() && expected != status.agent.session_id.as_deref() {
            reject(
                self,
                AgentOutcome::TargetChanged,
                "agent conversation changed",
            );
            return;
        }
        match request {
            Status { .. } => {
                let outcome = if status.status_available {
                    AgentOutcome::Ok
                } else {
                    AgentOutcome::StatusUnavailable
                };
                self.agent_reply(client, self.agent_result(pane, outcome, None));
            },
            Read { lines, .. } => {
                if lines == 0 || lines > 100_000 {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "lines must be between 1 and 100000",
                    );
                    return;
                }
                let output = self
                    .tab_containing_pane_mut(pane)
                    .and_then(|tab| tab.get_dump_terminal_screen(pane, true))
                    .unwrap_or_default();
                let mut last: Vec<_> = output.lines().rev().take(lines).collect();
                last.reverse();
                let mut response = self.agent_result(pane, AgentOutcome::Ok, None);
                response.output = Some(last.join("\n"));
                self.agent_reply(client, response);
            },
            SendKeys { keys, .. } => {
                if !status.status_available || !self.agent_process_matches(pane, &status.agent.kind)
                {
                    reject(
                        self,
                        AgentOutcome::StatusUnavailable,
                        "cannot verify a live agent process",
                    );
                    return;
                }
                if !self.agent_input_enabled(pane) {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "agent terminal input is disabled",
                    );
                    return;
                }
                if status.agent.state == AgentState::Exited
                    || self
                        .get_pane_info(pane)
                        .is_none_or(|info| info.is_held || info.exited)
                {
                    reject(
                        self,
                        AgentOutcome::TargetChanged,
                        "agent process has exited",
                    );
                    return;
                }
                if keys.is_empty() || keys.len() > 1024 {
                    reject(self, AgentOutcome::Rejected, "send between 1 and 1024 keys");
                    return;
                }
                let result = keys
                    .into_iter()
                    .try_for_each(|key| self.agent_write_key(pane, key));
                match result {
                    Ok(()) => {
                        self.agent_reply(client, self.agent_result(pane, AgentOutcome::Ok, None))
                    },
                    Err(error) => reject(self, AgentOutcome::Failed, &error.to_string()),
                }
            },
            Prompt {
                prompt,
                wait,
                timeout_ms,
                ..
            } => {
                if !status.prompt_supported {
                    reject(
                        self,
                        AgentOutcome::StatusUnavailable,
                        "prompt requires lifecycle hooks and a live supported agent command pane with bracketed paste enabled",
                    );
                    return;
                }
                if !status.ready {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "agent is not idle or already has a pending submission",
                    );
                    return;
                }
                if prompt.trim().is_empty()
                    || prompt.len() > 1024 * 1024
                    || prompt
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    reject(self, AgentOutcome::Rejected, "prompt must contain text, be at most 1 MiB, and contain no terminal controls");
                    return;
                }
                let Some(deadline) = deadline(timeout_ms) else {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "timeout must be between 1ms and 24h",
                    );
                    return;
                };
                let observation = PromptObservation::new(
                    &status.agent.kind,
                    status.agent.session_id.as_deref().unwrap_or_default(),
                    &prompt,
                );
                // Claim and observe before input; hooks cannot overtake this screen instruction.
                self.agent_controls.push(Pending {
                    client: Some(client),
                    pane,
                    kind: status.agent.kind.clone(),
                    conversation: status.agent.session_id,
                    deadline: Some(deadline),
                    input_revision: self.agent_input_revision(pane),
                    prompt: Some(observation),
                    wait,
                    until: AgentState::Idle,
                    session_ended: false,
                });
                let result = self
                    .paste_to_pane_id(prompt.into_bytes(), pane, None, None)
                    .and_then(|()| {
                        self.agent_write_key(pane, KeyWithModifier::new(BareKey::Enter))
                    });
                let revision = self.agent_input_revision(pane);
                if let Some(pending) = self.agent_controls.last_mut() {
                    pending.input_revision = revision;
                }
                if let Err(error) = result {
                    self.agent_controls.pop();
                    reject(self, AgentOutcome::Failed, &error.to_string());
                }
            },
            Wait {
                until, timeout_ms, ..
            } => {
                let Some(deadline) = deadline(timeout_ms) else {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "timeout must be between 1ms and 24h",
                    );
                    return;
                };
                self.agent_controls.push(Pending {
                    client: Some(client),
                    pane,
                    kind: status.agent.kind,
                    conversation: status.agent.session_id,
                    deadline: Some(deadline),
                    input_revision: self.agent_input_revision(pane),
                    prompt: None,
                    wait: true,
                    until,
                    session_ended: false,
                });
                self.refresh_agent_controls();
            },
            List | Spawn { .. } => {},
        }
    }

    fn agent_write_key(&mut self, pane: PaneId, key: KeyWithModifier) -> Result<()> {
        self.tab_containing_pane_mut(pane)
            .context("agent pane no longer exists")?
            .write_key_to_pane_id(pane, key)
    }

    fn finish_agent_control(
        &self,
        pending: &Pending,
        outcome: AgentOutcome,
        message: Option<String>,
    ) {
        if let Some(client) = pending.client {
            let mut response = self.agent_result(pending.pane, outcome, message);
            if let Some(prompt) = &pending.prompt {
                response.acknowledged = prompt.acknowledged();
                response.turn_id = prompt.turn_id().map(str::to_owned);
                if matches!(
                    outcome,
                    AgentOutcome::TimedOut
                        | AgentOutcome::StatusUnavailable
                        | AgentOutcome::Interrupted
                ) {
                    for status in &mut response.agents {
                        status.ready = false;
                    }
                }
            }
            self.agent_reply(client, response);
        }
    }

    pub(super) fn refresh_agent_controls(&mut self) {
        let mut keep = Vec::new();
        let claimed: std::collections::HashSet<_> = self
            .agent_controls
            .iter()
            .filter(|pending| pending.prompt.is_some())
            .map(|pending| pending.pane)
            .collect();
        for mut pending in std::mem::take(&mut self.agent_controls) {
            let mut status = self.agent_status(pending.pane);
            if let Some(status) = &mut status {
                status.ready &= !claimed.contains(&pending.pane);
            }
            let outcome = match &status {
                None if self.tab_containing_pane(pending.pane).is_none() => {
                    Some(AgentOutcome::TargetClosed)
                },
                None if !pending.session_ended => Some(AgentOutcome::TargetChanged),
                None if self
                    .get_pane_info(pending.pane)
                    .is_some_and(|info| info.exited) =>
                {
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
                    if pending.prompt.is_some()
                        && self.agent_input_revision(pending.pane) != pending.input_revision =>
                {
                    Some(AgentOutcome::Interrupted)
                },
                Some(status) if status.agent.state == AgentState::Exited => Some(
                    if pending.prompt.is_none() && pending.until == AgentState::Exited {
                        AgentOutcome::Ok
                    } else {
                        AgentOutcome::TargetChanged
                    },
                ),
                Some(status)
                    if pending.prompt.is_none()
                        && status.status_available
                        && status.agent.state == pending.until
                        && (pending.until != AgentState::Idle
                            || !matches!(status.agent.kind.as_str(), "claude" | "codex")
                            || status.ready) =>
                {
                    Some(AgentOutcome::Ok)
                },
                Some(status)
                    if pending.prompt.is_none()
                        && status.status_available
                        && status.agent.is_waiting_on_a_person() =>
                {
                    Some(AgentOutcome::NeedsInput)
                },
                _ if pending
                    .deadline
                    .is_some_and(|deadline| Instant::now() >= deadline) =>
                {
                    Some(if status.is_some_and(|s| !s.status_available) {
                        AgentOutcome::StatusUnavailable
                    } else {
                        AgentOutcome::TimedOut
                    })
                },
                _ => None,
            };
            if let Some(outcome) = outcome {
                self.finish_agent_control(&pending, outcome, None);
                if pending.prompt.is_some()
                    && matches!(
                        outcome,
                        AgentOutcome::TimedOut
                            | AgentOutcome::StatusUnavailable
                            | AgentOutcome::Interrupted
                    )
                {
                    // Ending the wait cannot recall input already sent to the agent, including
                    // input followed by a manual key. Keep the claim until its lifecycle ends:
                    // otherwise an identical retry could accept the first prompt's late hook.
                    pending.client = None;
                    pending.deadline = None;
                    keep.push(pending);
                }
            } else {
                if pending.conversation.is_none() {
                    pending.conversation = self
                        .agent_status(pending.pane)
                        .and_then(|s| s.agent.session_id);
                }
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        self.agent_launches
            .retain(|pane, _| !self.destroyed_panes.contains(pane));
        let stale: Vec<_> = self
            .agent_lifecycle
            .keys()
            .copied()
            .filter(|pane| {
                self.agent_status(*pane)
                    .is_none_or(|s| !s.status_available || s.agent.state == AgentState::Exited)
            })
            .collect();
        for pane in stale {
            self.agent_lifecycle.remove(&pane);
        }
    }

    pub(super) fn agent_control_event(&mut self, event: &AgentEvent) {
        let pane = PaneId::Terminal(event.pane_id);
        let hook = serde_json::from_str::<HookEvent>(&event.raw).ok();
        let parent = hook.as_ref().is_some_and(|hook| {
            hook.agent_id.is_none()
                && !matches!(
                    hook.hook_event_name.as_str(),
                    "SubagentStart" | "SubagentStop"
                )
        });
        let mut keep = Vec::new();
        let mut accepted_input_revision = None;
        for mut pending in std::mem::take(&mut self.agent_controls) {
            if pending.pane != pane {
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
            let outcome = if parent
                && event.hook == "SessionStart"
                && hook
                    .as_ref()
                    .is_none_or(|hook| hook.source.as_deref() != Some("compact"))
                && pending.conversation.is_some()
            {
                Some(AgentOutcome::TargetChanged)
            } else {
                pending
                    .prompt
                    .as_mut()
                    .and_then(|prompt| prompt.observe(event))
            };
            if parent
                && event.hook == "UserPromptSubmit"
                && matches!(outcome, None | Some(AgentOutcome::Submitted))
                && pending
                    .prompt
                    .as_ref()
                    .is_some_and(|prompt| prompt.acknowledged())
            {
                // A late or duplicate acceptance consumes only the submitted input,
                // not a newer draft that interrupted the caller's wait.
                accepted_input_revision = Some(pending.input_revision);
            }
            match outcome {
                Some(AgentOutcome::Submitted) => {
                    if !pending.wait {
                        self.finish_agent_control(&pending, AgentOutcome::Submitted, None);
                        pending.client = None;
                    }
                    keep.push(pending);
                },
                Some(outcome) => {
                    self.finish_agent_control(&pending, outcome, event.last_message.clone())
                },
                None => keep.push(pending),
            }
        }
        self.agent_controls = keep;
        if !parent {
            return;
        }
        if event.hook == "SessionEnd" {
            self.agent_lifecycle.remove(&pane);
        } else if let Some(conversation) = event.session_id.as_ref().filter(|id| {
            !id.is_empty()
                && hook
                    .as_ref()
                    .and_then(yinz_types::agent::hook_state)
                    .is_some()
        }) {
            let previous = self.agent_lifecycle.get(&pane).filter(|previous| {
                previous.kind == event.kind && previous.conversation == *conversation
            });
            // Startup cannot consume a draft. Only a pane with no input can establish an
            // initial consumed revision; matching lifecycle evidence is preserved as-is.
            let input_revision = match event.hook.as_str() {
                "SessionStart" => previous
                    .map(|previous| previous.input_revision)
                    .unwrap_or_else(|| (self.agent_input_revision(pane) == 0).then_some(0)),
                "UserPromptSubmit" => accepted_input_revision,
                // An uncorrelated acceptance may arrive after a newer draft. Tool progress
                // and completion preserve consumption, including when it is unknown.
                _ => previous.and_then(|previous| previous.input_revision),
            };
            self.agent_lifecycle.insert(
                pane,
                Lifecycle {
                    kind: event.kind.clone(),
                    conversation: conversation.clone(),
                    input_revision,
                },
            );
        }
    }

    pub(super) fn agent_control_deadline(&self) -> Option<Duration> {
        self.agent_controls
            .iter()
            .filter_map(|pending| {
                pending
                    .deadline
                    .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            })
            .min()
    }

    pub(super) fn detach_agent_controls(&mut self, client: ClientId) {
        self.agent_controls.retain_mut(|pending| {
            if pending.client == Some(client) {
                pending.client = None;
                return pending.prompt.is_some();
            }
            true
        });
    }
}

fn deadline(timeout_ms: u64) -> Option<Instant> {
    (1..=86_400_000)
        .contains(&timeout_ms)
        .then(|| Instant::now() + Duration::from_millis(timeout_ms))
}
