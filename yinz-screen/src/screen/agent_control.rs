//! Agent control is serialized with state reports and terminal input on the screen thread.
use super::Screen;
use crate::ClientId;
use std::time::{Duration, Instant};
use yinz_ipc::ipc::ServerToClientMsg;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{
    needs_paste_lead_in, AgentOutcome, AgentRequest, AgentResponse, AgentStatus, PromptObservation,
    MAX_PROMPT_BYTES, MAX_TIMEOUT_MS, PASTE_LEAD_IN,
};
use yinz_types::agent::{AgentEvent, AgentInfo, AgentSource, AgentState, HookEvent};
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_id::PaneId;

#[derive(Clone, Debug)]
pub(super) enum Reply {
    Client(ClientId),
    Host(std::sync::mpsc::Sender<AgentResponse>),
}

pub(super) struct WorkerWaits {
    parent: AgentInfo,
    waits: std::collections::BTreeMap<String, Option<String>>,
}

pub(super) struct Lifecycle {
    kind: String,
    conversation: String,
    input_revision: Option<u64>,
    /// When the last hook from this pane arrived, worker and stateless hooks included.
    pub(super) last_report: Instant,
    /// The provider turn last reported as interrupted or failed, until the next prompt. Codex
    /// finishes a cancelled turn's tool call after its `Interrupt`, and that hook must not make
    /// the idle agent busy again. `Stop` is not an end here: a blocking Stop hook continues
    /// the same turn.
    ended_turn: Option<String>,
}

/// How long an agent must go without reporting after input before an interrupted recovery may
/// reset it to idle, so recovery cannot overtake a hook that is still on its way.
pub(super) const RECOVER_QUIET: Duration = Duration::from_secs(3);

pub(super) struct Pending {
    client: Option<Reply>,
    pane: PaneId,
    kind: String,
    conversation: Option<String>,
    pub(super) deadline: Option<Instant>,
    input_revision: u64,
    prompt: Option<PromptObservation>,
    wait: bool,
    until: AgentState,
    session_ended: bool,
    abandoned: bool,
    resolved: bool,
    delivery: Option<tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult>>,
    enter_at: Option<Instant>,
}

impl Screen {
    pub(super) fn has_agent_lifecycle(&self, pane: PaneId, kind: &str, conversation: &str) -> bool {
        self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == kind && lifecycle.conversation == conversation
        })
    }

    /// Whether this hook is work from a turn that already ended by interruption or failure,
    /// such as the tool call Codex finishes after an Esc. Hooks without a turn id never are.
    pub(super) fn is_after_ended_turn(&self, pane: PaneId, kind: &str, hook: &HookEvent) -> bool {
        !matches!(
            hook.hook_event_name.as_str(),
            "UserPromptSubmit" | "SessionStart" | "SessionEnd"
        ) && matches!(
            yinz_types::agent::hook_state(hook),
            Some(AgentState::Busy | AgentState::Waiting)
        ) && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == kind
                && lifecycle.conversation == hook.session_id
                && lifecycle.ended_turn.is_some()
                && lifecycle.ended_turn == hook.turn_id
        })
    }

    pub(super) fn invalidate_agent_lifecycle(&mut self, pane: PaneId) {
        self.agent_lifecycle.remove(&pane);
        self.agent_worker_waits.remove(&pane);
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.agent_controls) {
            if pending.pane == pane && pending.prompt.is_some() {
                self.finish_agent_control(
                    &pending,
                    if self.destroyed_panes.contains(&pane) {
                        AgentOutcome::TargetClosed
                    } else {
                        AgentOutcome::TargetChanged
                    },
                    None,
                );
            } else {
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
    }

    pub(super) fn apply_agent_report_state(
        &mut self,
        report: &yinz_types::agent::report::AgentReport,
        hook: Option<&HookEvent>,
    ) -> Result<bool> {
        use yinz_types::agent::report::AgentReportState;
        use yinz_types::agent::{hook_state, waiting_detail, waiting_needs_a_person};
        let pane = report.pane_id;
        let mut workers = self.agent_worker_waits.remove(&pane);
        let agent = if let Some(hook) = hook.filter(|hook| hook.is_worker()) {
            let Some(mut parent) = self
                .tab_containing_pane(pane)
                .and_then(|tab| tab.pane_agent_info(pane))
            else {
                return Ok(true);
            };
            if let Some(workers) = &workers {
                // Only the presentation is overlaid. Keep current run and checkout metadata.
                parent.state = workers.parent.state;
                parent.detail = workers.parent.detail.clone();
                parent.waiting_on = workers.parent.waiting_on.clone();
                parent.wait_needs_a_person = workers.parent.wait_needs_a_person;
            }
            if parent.kind != report.kind
                || parent.session_id.as_deref() != Some(hook.session_id.as_str())
            {
                if let Some(workers) = workers {
                    self.agent_worker_waits.insert(pane, workers);
                }
                return Ok(true);
            }
            if let Some(id) = &hook.agent_id {
                if hook_state(hook) == Some(AgentState::Waiting) && waiting_needs_a_person(hook) {
                    workers
                        .get_or_insert_with(|| WorkerWaits {
                            parent: parent.clone(),
                            waits: Default::default(),
                        })
                        .waits
                        .insert(id.clone(), waiting_detail(hook));
                } else if hook_state(hook).is_some()
                    || matches!(hook.hook_event_name.as_str(), "SubagentStop" | "SessionEnd")
                {
                    if let Some(workers) = &mut workers {
                        workers.waits.remove(id);
                    }
                }
            }
            Some(parent)
        } else {
            match &report.state {
                AgentReportState::Unchanged => {
                    if let Some(workers) = workers {
                        self.agent_worker_waits.insert(pane, workers);
                    }
                    return Ok(true);
                },
                AgentReportState::Clear => None,
                AgentReportState::Set(agent) => Some(agent.clone()),
            }
        };
        if agent.is_none() {
            workers = None;
        }
        let mut visible = agent.clone();
        if let (Some(workers), Some(parent)) = (&mut workers, agent) {
            if workers.parent.kind == parent.kind
                && workers.parent.session_id == parent.session_id
                && parent.state != AgentState::Exited
                && !workers.waits.is_empty()
            {
                workers.parent = parent.clone();
                let mut waiting = parent;
                waiting.state = AgentState::Waiting;
                waiting.wait_needs_a_person = true;
                waiting.waiting_on = workers.waits.values().next().cloned().flatten();
                waiting.detail = None;
                visible = Some(waiting);
            } else {
                workers.waits.clear();
            }
        }
        let result = super::thread_main::apply_pane_agent_report(self, pane, visible)
            .context("apply parent state and worker waits")?;
        if let Some(workers) = workers.filter(|workers| !workers.waits.is_empty()) {
            self.agent_worker_waits.insert(pane, workers);
        }
        Ok(result)
    }

    fn agent_input_revision(&self, pane: PaneId) -> u64 {
        self.tab_containing_pane(pane)
            .map(|tab| tab.agent_input_revision(pane))
            .unwrap_or(0)
    }

    fn agent_input_at(&self, pane: PaneId) -> Option<Instant> {
        self.tab_containing_pane(pane)
            .and_then(|tab| tab.agent_input_at(pane))
    }

    /// Tell plugins that an interrupted turn ended. Claude has no hook for it, so this is the
    /// `Interrupt` Codex reports for a cancelled turn.
    fn broadcast_interrupt(&self, pane: PaneId, agent: &AgentInfo) -> Result<()> {
        let PaneId::Terminal(terminal) = pane else {
            return Ok(());
        };
        let hook = HookEvent {
            hook_event_name: "Interrupt".into(),
            session_id: agent.session_id.clone().unwrap_or_default(),
            cwd: agent.cwd.clone(),
            ..Default::default()
        };
        let json = serde_json::to_string(&hook).context("serialize an interrupt event")?;
        let event = AgentEvent::from_hook(&agent.kind, terminal, &hook, json);
        self.bus
            .senders
            .send_to_plugin(crate::plugins::PluginInstruction::Update(vec![(
                None,
                None,
                yinz_types::data::Event::AgentEvent(event),
            )]))
    }

    fn agent_status(&self, pane_id: PaneId) -> Option<AgentStatus> {
        let info = self.get_pane_info(pane_id)?;
        let agent = info.agent?;
        let status_available = agent.source == AgentSource::Reported
            && agent.session_id.as_ref().is_some_and(|conversation| {
                self.has_agent_lifecycle(pane_id, &agent.kind, conversation)
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
            host_id: yinz_runtime::host_identity::host_id(),
            tab_id: self.tab_containing_pane(pane_id).map(|tab| tab.id),
            target: info.target,
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

    fn agent_reply(&self, reply: &Reply, response: AgentResponse) {
        let client = match reply {
            Reply::Client(client) => *client,
            Reply::Host(sender) => {
                let _ = sender.send(response);
                return;
            },
        };
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
        self.agent_control_with_reply(request, Reply::Client(client));
    }

    pub(super) fn agent_control_with_reply(&mut self, request: AgentRequest, client: Reply) {
        self.agent_control_at(request, client, Instant::now());
    }

    pub(super) fn agent_control_at(&mut self, request: AgentRequest, client: Reply, now: Instant) {
        use AgentRequest::*;
        let (pane, expected) = match &request {
            List { .. } => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.agents = self
                    .tabs
                    .values()
                    .flat_map(|tab| tab.get_all_pane_ids())
                    .filter_map(|id| self.agent_status(id))
                    .filter(|status| status.agent.state != AgentState::Exited)
                    .collect();
                self.agent_reply(&client, response);
                return;
            },
            Spawn { .. } => {
                self.agent_reply(&client, AgentResponse::new(AgentOutcome::Rejected));
                return;
            },
            Recover {
                pane_id,
                expected_session_id,
                ..
            } => (*pane_id, Some(expected_session_id.as_str())),
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
                &client,
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
            Recover {
                expected_session_id,
                confirm_empty,
                interrupted,
                ..
            } => {
                let state = status.agent.state;
                let in_flight = self.agent_controls.iter().any(|pending| {
                    pending.pane == pane
                        && pending.prompt.is_some()
                        && (!pending.abandoned || pending.delivery.is_some())
                });
                let refusal = if !confirm_empty || expected_session_id.is_empty() {
                    Some("recovery requires --conversation and --confirm-empty".to_owned())
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
                    let input_at = self.agent_input_at(pane);
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
                    reject(self, AgentOutcome::Rejected, &message);
                    return;
                }
                if state != AgentState::Idle {
                    let mut idle = status.agent.clone();
                    idle.state = AgentState::Idle;
                    idle.detail = None;
                    idle.waiting_on = None;
                    idle.wait_needs_a_person = false;
                    self.agent_worker_waits.remove(&pane);
                    super::thread_main::apply_pane_agent_report(self, pane, Some(idle))
                        .context("reset an interrupted agent to idle")
                        .non_fatal();
                    self.broadcast_interrupt(pane, &status.agent)
                        .context("announce an interrupted turn to plugins")
                        .non_fatal();
                }
                self.agent_controls
                    .retain(|pending| pending.pane != pane || pending.prompt.is_none());
                let revision = self.agent_input_revision(pane);
                if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
                    lifecycle.input_revision = Some(revision);
                }
                self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None));
            },
            Status { .. } => {
                let outcome = if status.status_available {
                    AgentOutcome::Ok
                } else {
                    AgentOutcome::StatusUnavailable
                };
                self.agent_reply(&client, self.agent_result(pane, outcome, None));
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
                self.agent_reply(&client, response);
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
                        self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None))
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
                    || prompt.len() > MAX_PROMPT_BYTES
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
                let (tx, rx) = tokio::sync::oneshot::channel();
                // Claim and observe before input; hooks cannot overtake this screen instruction.
                self.agent_controls.push(Pending {
                    client: Some(client.clone()),
                    pane,
                    kind: status.agent.kind.clone(),
                    conversation: status.agent.session_id,
                    deadline: Some(deadline),
                    input_revision: self.agent_input_revision(pane),
                    prompt: Some(observation),
                    wait,
                    until: AgentState::Idle,
                    session_ended: false,
                    abandoned: false,
                    resolved: false,
                    delivery: Some(rx),
                    enter_at: None,
                });
                // Expose Claude's slash command before its arguments can collapse into a chip.
                // Keep whitespace in the paste: typing a newline or tab would send a key.
                let (prefix, paste) = if needs_paste_lead_in(&status.agent.kind, &prompt) {
                    (
                        format!("{PASTE_LEAD_IN} ").into_bytes(),
                        prompt.into_bytes(),
                    )
                } else if status.agent.kind == "claude" && prompt.starts_with('/') {
                    let end = prompt.find(char::is_whitespace).unwrap_or(prompt.len());
                    (
                        prompt.as_bytes()[..end].to_vec(),
                        prompt.as_bytes()[end..].to_vec(),
                    )
                } else {
                    (Vec::new(), prompt.into_bytes())
                };
                let typed = (!prefix.is_empty())
                    .then(|| self.write_to_pane_id(&None, prefix, false, pane, None, None));
                let result = match typed {
                    Some(Err(error)) => Err(error),
                    _ => self.paste_to_pane_id(
                        paste,
                        pane,
                        None,
                        Some(yinz_bus::route::NotificationEnd::new(tx)),
                    ),
                };
                let revision = self.agent_input_revision(pane);
                if let Some(pending) = self.agent_controls.last_mut() {
                    pending.input_revision = revision;
                }
                if let Err(error) = result {
                    let Some(mut pending) = self.agent_controls.pop() else {
                        return;
                    };
                    self.finish_agent_control(
                        &pending,
                        AgentOutcome::Failed,
                        Some(error.to_string()),
                    );
                    pending.abandoned = true;
                    pending.client = None;
                    pending.deadline = None;
                    self.agent_controls.push(pending);
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
                    client: Some(client.clone()),
                    pane,
                    kind: status.agent.kind,
                    conversation: status.agent.session_id,
                    deadline: Some(deadline),
                    input_revision: self.agent_input_revision(pane),
                    prompt: None,
                    wait: true,
                    until,
                    session_ended: false,
                    abandoned: false,
                    resolved: false,
                    delivery: None,
                    enter_at: None,
                });
                self.refresh_agent_controls();
            },
            List { .. } | Spawn { .. } => {},
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
        if let Some(client) = &pending.client {
            let mut response = self.agent_result(pending.pane, outcome, message);
            if let Some(prompt) = &pending.prompt {
                if matches!(
                    outcome,
                    AgentOutcome::TimedOut
                        | AgentOutcome::Interrupted
                        | AgentOutcome::StatusUnavailable
                        | AgentOutcome::Failed
                ) {
                    response.message = Some(format!("{} Inspect the pane and clear any draft, then use agent recover {} --conversation {} --confirm-empty.",
                        response.message.as_deref().unwrap_or("Submission is uncertain."), pending.pane,
                        pending.conversation.as_deref().unwrap_or("ID")));
                }
                response.acknowledged = prompt.acknowledged();
                response.turn_id = prompt.turn_id().map(str::to_owned);
                if pending.delivery.is_some()
                    || pending.enter_at.is_some()
                    || matches!(
                        outcome,
                        AgentOutcome::TimedOut
                            | AgentOutcome::StatusUnavailable
                            | AgentOutcome::Interrupted
                    )
                {
                    for status in &mut response.agents {
                        status.ready = false;
                    }
                }
            }
            self.agent_reply(client, response);
        }
    }

    pub(super) fn refresh_agent_controls(&mut self) {
        self.refresh_agent_controls_at(Instant::now());
    }

    pub(crate) fn refresh_agent_controls_at(&mut self, now: Instant) {
        if !self
            .agent_controls
            .iter()
            .any(|pending| !pending.abandoned || pending.delivery.is_some())
        {
            return;
        }
        let mut keep = Vec::new();
        let claimed: std::collections::HashSet<_> = self
            .agent_controls
            .iter()
            .filter(|pending| pending.prompt.is_some())
            .map(|pending| pending.pane)
            .collect();
        for mut pending in std::mem::take(&mut self.agent_controls) {
            if pending.abandoned && pending.delivery.is_none() {
                keep.push(pending);
                continue;
            }
            let mut delivery_error = None;
            if let Some(delivery) = &mut pending.delivery {
                match delivery.try_recv() {
                    Ok(result) => {
                        pending.delivery = None;
                        delivery_error = result.error_message;
                        if delivery_error.is_none()
                            && !pending.abandoned
                            && pending
                                .prompt
                                .as_ref()
                                .is_some_and(|prompt| !prompt.acknowledged())
                        {
                            pending.enter_at = Some(now + Duration::from_millis(100));
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
            let mut status = self.agent_status(pending.pane);
            if let Some(status) = &mut status {
                status.ready &= !claimed.contains(&pending.pane);
            }
            let mut outcome = match &status {
                _ if delivery_error.is_some() => Some(AgentOutcome::Failed),
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
                Some(status)
                    if pending.prompt.is_some()
                        && (!status.prompt_supported
                            || !self.agent_process_matches(pending.pane, &pending.kind)) =>
                {
                    Some(AgentOutcome::StatusUnavailable)
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
                _ if pending.deadline.is_some_and(|deadline| now >= deadline) => {
                    Some(if status.as_ref().is_some_and(|s| !s.status_available) {
                        AgentOutcome::StatusUnavailable
                    } else {
                        AgentOutcome::TimedOut
                    })
                },
                _ => None,
            };
            if outcome.is_none() && pending.enter_at.is_some_and(|enter| now >= enter) {
                pending.enter_at = None;
                if pending
                    .prompt
                    .as_ref()
                    .is_some_and(|prompt| !prompt.acknowledged())
                {
                    if status
                        .as_ref()
                        .is_none_or(|status| status.agent.state != AgentState::Idle)
                    {
                        outcome = Some(AgentOutcome::Interrupted);
                    } else if let Err(error) =
                        self.agent_write_key(pending.pane, KeyWithModifier::new(BareKey::Enter))
                    {
                        delivery_error = Some(error.to_string());
                        outcome = Some(AgentOutcome::Failed);
                    } else {
                        pending.input_revision = self.agent_input_revision(pending.pane);
                    }
                }
            }
            if let Some(outcome) = outcome {
                self.finish_agent_control(&pending, outcome, delivery_error);
                if pending.prompt.is_some()
                    && matches!(
                        outcome,
                        AgentOutcome::TimedOut
                            | AgentOutcome::StatusUnavailable
                            | AgentOutcome::Interrupted
                            | AgentOutcome::Failed
                    )
                {
                    // Ending the wait cannot recall input already sent to the agent, including
                    // input followed by a manual key. Keep the claim until its lifecycle ends:
                    // otherwise an identical retry could accept the first prompt's late hook.
                    pending.client = None;
                    pending.deadline = None;
                    pending.abandoned = true;
                    pending.enter_at = None;
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
    }

    pub(super) fn agent_control_event(&mut self, event: &AgentEvent, hook: Option<&HookEvent>) {
        let pane = PaneId::Terminal(event.pane_id);
        let parent = hook.is_some_and(|hook| !hook.is_worker());
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.last_report = Instant::now();
        }
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
                    pending.enter_at = None;
                    if !pending.wait {
                        self.finish_agent_control(&pending, AgentOutcome::Submitted, None);
                        pending.client = None;
                    }
                    keep.push(pending);
                },
                Some(outcome) => {
                    self.finish_agent_control(&pending, outcome, event.last_message.clone());
                    if matches!(
                        outcome,
                        AgentOutcome::Interrupted
                            | AgentOutcome::StatusUnavailable
                            | AgentOutcome::NeedsInput
                    ) {
                        pending.client = None;
                        pending.deadline = None;
                        pending.abandoned = true;
                        pending.enter_at = None;
                        keep.push(pending);
                    } else if pending.delivery.is_some()
                        && matches!(outcome, AgentOutcome::Completed | AgentOutcome::Failed)
                    {
                        pending.resolved = true;
                        pending.abandoned = true;
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
        if !parent {
            return;
        }
        if hook.is_some_and(|hook| hook.codex_hooks_ready == Some(false)) {
            self.agent_lifecycle.remove(&pane);
        } else if event.hook == "SessionEnd" {
            self.agent_lifecycle.remove(&pane);
            self.agent_worker_waits.remove(&pane);
        } else if let Some(conversation) = event.session_id.as_ref().filter(|id| {
            !id.is_empty()
                && hook
                    .as_ref()
                    .and_then(|hook| yinz_types::agent::hook_state(hook))
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
            let ended_turn = match event.hook.as_str() {
                "Interrupt" | "StopFailure" => hook
                    .and_then(|hook| hook.turn_id.clone())
                    .filter(|turn| !turn.is_empty()),
                "UserPromptSubmit" => None,
                _ => previous.and_then(|previous| previous.ended_turn.clone()),
            };
            self.agent_lifecycle.insert(
                pane,
                Lifecycle {
                    kind: event.kind.clone(),
                    conversation: conversation.clone(),
                    input_revision,
                    last_report: Instant::now(),
                    ended_turn,
                },
            );
        }
    }

    pub(super) fn agent_control_deadline(&self) -> Option<Duration> {
        let now = Instant::now();
        self.agent_controls
            .iter()
            .flat_map(|pending| {
                [
                    pending
                        .deadline
                        .map(|deadline| deadline.saturating_duration_since(now)),
                    pending
                        .enter_at
                        .map(|enter| enter.saturating_duration_since(now)),
                    pending.delivery.as_ref().map(|_| Duration::from_millis(10)),
                ]
            })
            .flatten()
            .min()
    }

    pub(super) fn detach_agent_controls(&mut self, client: ClientId) {
        self.agent_controls.retain_mut(|pending| {
            if matches!(pending.client, Some(Reply::Client(id)) if id == client) {
                pending.client = None;
                return pending.prompt.is_some();
            }
            true
        });
    }
}

fn deadline(timeout_ms: u64) -> Option<Instant> {
    (1..=MAX_TIMEOUT_MS)
        .contains(&timeout_ms)
        .then(|| Instant::now() + Duration::from_millis(timeout_ms))
}
