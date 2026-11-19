mod close_completed;
mod events;
mod handoff;
pub(super) mod host;
mod input;
mod lifecycle;
pub(super) mod reports;
mod worktrees;
use host::AgentHost;
pub(super) mod providers;
mod status;
mod submissions;
use providers::Provider;
mod notifications;
mod read;
mod recover;
mod request;
mod restart;
pub(super) use close_completed::AgentClose;
pub(super) use request::Completion;
mod permissions;
mod wait;
// Agent control is serialized with state reports and terminal input on the screen thread.
use crate::ClientId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{
    is_claude_task_notification, supports_prompt, validate_submission_id, AgentOutcome,
    AgentReadiness, AgentRequest, AgentResponse, AgentStatus, NotReadyReason, PromptDelivery,
    PromptObservation, MAX_PROMPT_BYTES, MAX_TIMEOUT_MS,
};
pub(super) use yinz_types::agent::tracking::{
    ClaudeActivity, ClaudeNotification, ClaudeTurn, DialogInput, LifecycleState, PermissionWait,
    Submission, SubmissionResult, WorkerWaits,
};
use yinz_types::agent::{AgentEvent, AgentInfo, AgentSource, AgentState, HookEvent};
use yinz_types::keys::{BareKey, KeyWithModifier};
use yinz_types::pane_id::PaneId;

#[derive(Clone, Debug)]
pub(super) enum Reply {
    Client(ClientId),
    Host(std::sync::mpsc::Sender<AgentResponse>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lifecycle {
    kind: String,
    conversation: String,
    claude: ClaudeActivity,
    input_revision: Option<u64>,
    /// When the last hook from this pane arrived, worker and stateless hooks included.
    pub(super) last_report: Instant,
    notification_tick: Instant,
    /// The provider turn last reported as interrupted or failed, until the next prompt. Codex
    /// finishes a cancelled turn's tool call after its `Interrupt`, and that hook must not make
    /// the idle agent busy again. `Stop` is not an end here: a blocking Stop hook continues
    /// the same turn.
    ended_turn: Option<String>,
    closeout_turn: Option<String>,
    claude_prompt_id: Option<String>,
    claude_confirmation: Option<bool>,
    claude_previous_prompt_id: Option<String>,
    agy_confirmation: Option<bool>,
    /// The prompt whose confirmed Claude turn left background work running, until Claude's
    /// session registry reports idle or a later turn's confirmation replaces it.
    claude_background: Option<String>,
    /// Claude `<task-notification>` turns in flight, by prompt ID. They run beside the
    /// caller's prompt tracking, which they never replace.
    claude_notifications: BTreeMap<String, ClaudeNotification>,
    /// The final assistant text of the last completed turn, until the next caller prompt.
    last_message: Option<String>,
}

impl Lifecycle {
    pub(super) fn event_background(&self) -> bool {
        self.claude_background.is_some() || self.claude.obligations.active()
    }

    #[cfg(test)]
    pub(crate) fn claude_background_for_test(&self) -> Option<&str> {
        self.claude_background.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn last_message_for_test(&self) -> Option<&str> {
        self.last_message.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn handoff_for_test(&self) -> LifecycleState {
        self.handoff()
    }

    /// The notification turn a Claude hook belongs to, by its prompt ID.
    fn claude_notification(&self, hook: &HookEvent) -> Option<&ClaudeNotification> {
        hook.prompt_id
            .as_ref()
            .and_then(|prompt| self.claude_notifications.get(prompt))
    }
}

impl Lifecycle {
    fn handoff(&self) -> LifecycleState {
        let mut claude = self.claude.clone();
        claude.obligations.elapse(
            self.notification_tick
                .elapsed()
                .as_millis()
                .min(u64::MAX as u128) as u64,
        );
        if let Some(permission) = &mut claude.permission {
            permission.elapse(
                self.notification_tick
                    .elapsed()
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            );
        }
        LifecycleState {
            kind: self.kind.clone(),
            conversation: self.conversation.clone(),
            claude,
            input_revision: self.input_revision,
            ended_turn: self.ended_turn.clone(),
            closeout_turn: self.closeout_turn.clone(),
            claude_prompt_id: self.claude_prompt_id.clone(),
            claude_confirmation: self.claude_confirmation,
            claude_previous_prompt_id: self.claude_previous_prompt_id.clone(),
            agy_confirmation: self.agy_confirmation,
            claude_background: self.claude_background.clone(),
            claude_notifications: self.claude_notifications.clone(),
            last_message: self.last_message.clone(),
        }
    }
}
impl From<LifecycleState> for Lifecycle {
    fn from(state: LifecycleState) -> Self {
        Self {
            notification_tick: Instant::now(),
            last_report: Instant::now(),
            kind: state.kind,
            conversation: state.conversation,
            claude: state.claude,
            input_revision: state.input_revision,
            ended_turn: state.ended_turn,
            closeout_turn: state.closeout_turn,
            claude_prompt_id: state.claude_prompt_id,
            claude_confirmation: state.claude_confirmation,
            claude_previous_prompt_id: state.claude_previous_prompt_id,
            agy_confirmation: state.agy_confirmation,
            claude_background: state.claude_background,
            claude_notifications: state.claude_notifications,
            last_message: state.last_message,
        }
    }
}

/// Input evidence carried without a lifecycle (including provisional startup).
/// None records unknown consumption and remains unknown across repeated warps.
pub(super) struct TransferredInput {
    kind: String,
    conversation: String,
    input_revision: u64,
    consumed: Option<bool>,
}

pub(super) struct ClearInput {
    kind: String,
    conversation: String,
    input_revision: u64,
    deadline: Instant,
    session_ended: bool,
}

/// How long an agent must go without reporting after input before an interrupted recovery may
/// reset it to idle, so recovery cannot overtake a hook that is still on its way.
pub(super) const RECOVER_QUIET: Duration = Duration::from_secs(3);

/// One conversation retains a small recent result window; replacing it releases its history.
pub(super) const SUBMISSION_HISTORY_LIMIT: usize = 32;

/// Keep the recent admission window independently of older unresolved claims.
fn prune_submissions(
    submissions: &mut std::collections::VecDeque<Submission>,
    protected: Option<&BTreeSet<String>>,
) {
    let old = submissions.len().saturating_sub(SUBMISSION_HISTORY_LIMIT);
    let mut index = 0;
    submissions.retain(|submission| {
        let keep = index >= old
            || submission.result.is_none()
            || protected.is_some_and(|ids| ids.contains(&submission.id));
        index += 1;
        keep
    });
}

pub(super) struct Pending {
    client: Option<Reply>,
    pane: PaneId,
    kind: String,
    conversation: Option<String>,
    pub(super) deadline: Option<Instant>,
    input_revision: u64,
    prompt: Option<PromptObservation>,
    command: bool,
    command_enter_sent: bool,
    submission_id: Option<String>,
    clear: bool,
    steering: bool,
    wait: bool,
    until: AgentState,
    session_ended: bool,
    abandoned: bool,
    blocks_steering: bool,
    resolved: bool,
    delivery: Option<tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult>>,
    enter_at: Option<Instant>,
    submit_key: KeyWithModifier,
    submit_delay: Duration,
}

impl Pending {
    fn not_ready_reason(&self) -> Option<NotReadyReason> {
        if self.command {
            return Some(NotReadyReason::PendingSubmission);
        }
        self.prompt.as_ref().and_then(|prompt| {
            if !prompt.acknowledged() || self.delivery.is_some() || self.enter_at.is_some() {
                Some(NotReadyReason::PendingSubmission)
            } else if self.kind == "claude" && !self.blocks_steering {
                None
            } else {
                Some(NotReadyReason::UnresolvedTurn)
            }
        })
    }
}

fn deadline(now: Instant, timeout_ms: u64) -> Option<Instant> {
    (1..=MAX_TIMEOUT_MS)
        .contains(&timeout_ms)
        .then(|| now + Duration::from_millis(timeout_ms))
}

pub(super) struct DraftClear {
    client: Option<Reply>,
    agent: AgentInfo,
    revision: u64,
    report: Instant,
    deadline: Instant,
    marker: String,
    stage: DraftStage,
    wrote_at: Option<Instant>,
    delivery: Option<tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult>>,
    failure: Option<(AgentOutcome, String)>,
    prompt: Option<(Pending, Vec<u8>, Vec<u8>)>,
}

#[derive(Debug)]
enum DraftStage {
    Marker,
    Empty {
        since: Option<Instant>,
    },
    PromptMarker {
        clear_key: KeyWithModifier,
        since: Option<Instant>,
    },
    PromptCleared {
        since: Option<Instant>,
    },
}

struct Composer {
    empty: bool,
    before_cursor: String,
}

/// Called only after source/conversation and permission-sample admission.
fn passive_claude_permission(event: &AgentEvent, hook: Option<&HookEvent>) -> bool {
    event.kind == "claude"
        && event.hook == yinz_types::agent::claude_permissions::EVENT
        && hook.is_some_and(|hook| {
            hook.hook_event_name == event.hook
                && event.session_id.as_deref() == Some(hook.session_id.as_str())
                && hook.claude_permission.as_ref().is_some_and(|sample| {
                    matches!(
                        sample.status,
                        yinz_types::agent::claude_permissions::Status::Inactive
                            | yinz_types::agent::claude_permissions::Status::Unavailable
                    )
                })
        })
}

impl AgentController {
    #[cfg(test)]
    pub(crate) fn claude_background_pending_for_test(&self) -> bool {
        self.claude_background_pending(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
            7,
        )))
    }
    pub(in crate::screen) fn agent_control_deadline(&self) -> Option<Duration> {
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
            .chain(
                self.agent_clear_inputs
                    .values()
                    .map(|input| input.deadline.saturating_duration_since(now)),
            )
            .chain(
                self.agent_draft_clears
                    .values()
                    .map(|_| Duration::from_millis(10)),
            )
            .chain(self.agent_events.deadline())
            .chain(
                self.claude_drain_at()
                    .map(|at| at.saturating_duration_since(now)),
            )
            .min()
    }
}

impl AgentController {
    pub(in crate::screen) fn agent_reply(
        &self,
        host: &impl AgentHost,
        reply: &Reply,
        mut response: AgentResponse,
    ) {
        response.server_instance = Some(yinz_bus::agent_launch::server_instance().to_owned());
        let client = match reply {
            Reply::Client(client) => *client,
            Reply::Host(sender) => {
                let _ = sender.send(response);
                return;
            },
        };
        host.send_control_reply(client, response);
    }

    pub(in crate::screen) fn finish_agent_control(
        &mut self,
        host: &impl AgentHost,
        pending: &Pending,
        outcome: AgentOutcome,
        message: Option<String>,
        pending_claim: Option<NotReadyReason>,
    ) {
        let reported_interrupt = pending
            .submission_id
            .as_deref()
            .and_then(|id| self.submission(pending.pane, id))
            .and_then(|submission| submission.result.as_ref())
            .is_some_and(|result| result.outcome == AgentOutcome::Interrupted);
        if pending.prompt.is_some()
            && !pending.resolved
            && (matches!(
                outcome,
                AgentOutcome::TimedOut | AgentOutcome::StatusUnavailable | AgentOutcome::Failed
            ) || (outcome == AgentOutcome::Interrupted && !reported_interrupt))
        {
            self.agent_observation_uncertain(
                host,
                pending.pane,
                pending.submission_id.clone(),
                outcome,
            );
        }
        let completed = pending.command
            || pending.resolved
            || matches!(
                outcome,
                AgentOutcome::TargetChanged | AgentOutcome::TargetClosed
            );
        let background = pending.prompt.as_ref().map_or_else(
            || {
                self.agent_status(host, pending.pane, pending_claim)
                    .is_some_and(|status| status.background)
            },
            PromptObservation::background,
        );
        self.record_submission(
            pending,
            completed.then(|| SubmissionResult {
                outcome,
                message: message.clone(),
                background,
            }),
        );
        if let Some(client) = &pending.client {
            let mut response = AgentResponse::new(outcome);
            response.submission_id = pending.submission_id.clone();
            response.prompt_delivery = pending
                .submission_id
                .as_deref()
                .and_then(|id| self.submission(pending.pane, id))
                .and_then(|submission| submission.prompt_delivery);
            response
                .agents
                .extend(self.agent_status(host, pending.pane, pending_claim));
            response.message = message;
            if matches!(outcome, AgentOutcome::Completed | AgentOutcome::Ok) {
                response.background = background;
            }
            if pending.prompt.is_none()
                && let Some(submission) = pending
                    .submission_id
                    .as_deref()
                    .and_then(|id| self.submission(pending.pane, id))
            {
                response.acknowledged = submission.acknowledged;
                response.turn_id = submission.turn_id.clone();
                if let Some(result) = &submission.result {
                    response.outcome = result.outcome;
                    response.message = result.message.clone();
                    response.background = result.background;
                }
            }
            if response.background {
                // Current identities, even for a retained result: they say what is still
                // pending now, not what was pending when that turn ended.
                response.background_tasks = response
                    .agents
                    .first()
                    .map(|status| status.background_tasks.clone())
                    .unwrap_or_default();
            }
            if let Some(prompt) = &pending.prompt {
                response.acknowledged = prompt.acknowledged();
                response.turn_id = prompt.turn_id().map(str::to_owned);
                let explanation = match outcome {
                    _ if pending.resolved && !prompt.acknowledged() => "",
                    AgentOutcome::TimedOut if prompt.acknowledged() =>
                        "The agent accepted this turn, but completion was not observed before the timeout. The turn may still be running.",
                    AgentOutcome::TimedOut =>
                        "Prompt acceptance was not acknowledged; acceptance is uncertain.",
                    AgentOutcome::Interrupted if prompt.acknowledged() =>
                        "Observation of the accepted turn was interrupted.",
                    AgentOutcome::Interrupted =>
                        "Submission was interrupted before acknowledgement; acceptance is uncertain.",
                    AgentOutcome::Failed if pending.resolved => "The accepted turn failed.",
                    AgentOutcome::Failed => "Prompt delivery failed; inspect the pane to check acceptance.",
                    AgentOutcome::StatusUnavailable if prompt.acknowledged() =>
                        "The turn was accepted, but its outcome could not be verified.",
                    AgentOutcome::StatusUnavailable =>
                        "Prompt acceptance could not be verified.",
                    _ => "",
                };
                if !explanation.is_empty() {
                    let mut message = response.message.take().unwrap_or_default();
                    if !message.is_empty() {
                        message.push(' ');
                    }
                    message.push_str(explanation);
                    let conversation = pending.conversation.as_deref().unwrap_or_default();
                    let conversation = if !conversation.is_empty()
                        && conversation.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                        }) {
                        conversation.to_owned()
                    } else {
                        format!("'{}'", conversation.replace('\'', "'\\''"))
                    };
                    // Pane input ends a prompt's own observation, but the accepted turn
                    // can keep running. A submission wait ignores input and reports the
                    // provider's actual outcome, so it precedes any recovery.
                    let retrievable = prompt.acknowledged()
                        && !pending.resolved
                        && matches!(
                            outcome,
                            AgentOutcome::Interrupted
                                | AgentOutcome::TimedOut
                                | AgentOutcome::StatusUnavailable
                        );
                    if let Some(submission) =
                        pending.submission_id.as_deref().filter(|_| retrievable)
                    {
                        let lead = if explanation.contains("may still be running") {
                            " Retrieve"
                        } else {
                            " The turn may still be running; retrieve"
                        };
                        // An empty wait pin never matches; an unpinned turn wait omits it.
                        let pin = if pending
                            .conversation
                            .as_deref()
                            .is_some_and(|c| !c.is_empty())
                        {
                            format!(" --conversation {conversation}")
                        } else {
                            String::new()
                        };
                        message.push_str(&format!("{lead} its outcome with agent wait {} --submission {submission}{pin}. Pane input does not end that wait.",
                            pending.pane));
                    }
                    let unresolved = response.agents.first().is_some_and(|status| {
                        status.prompt_supported
                            && status.agent.kind == pending.kind
                            && status.agent.session_id == pending.conversation
                            && (pending_claim.is_some()
                                || status.not_ready_reason == Some(NotReadyReason::UnconsumedInput))
                    });
                    if unresolved {
                        let condition = if retrievable {
                            "Only if that wait cannot resolve the turn and the same live conversation has stopped"
                        } else {
                            "Only if the same live conversation has stopped and unresolved input remains"
                        };
                        message.push_str(&format!(" Inspect with agent read {}. {condition}, clear its draft and use agent recover {} --conversation {conversation} --confirm-empty. Add --interrupted if it stopped without reporting idle.",
                            pending.pane, pending.pane));
                    }
                    response.message = Some(message);
                }
            }
            self.agent_reply(host, client, response);
        }
    }
}

impl AgentController {
    pub(in crate::screen) fn agent_result(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
        outcome: AgentOutcome,
        message: Option<String>,
    ) -> AgentResponse {
        let mut response = AgentResponse::new(outcome);
        response.agents.extend(self.agent_status(host, pane, None));
        response.message = message;
        response
    }

    pub(in crate::screen) fn reject_control(
        &self,
        host: &impl AgentHost,
        client: &Reply,
        pane: PaneId,
        requested_submission: Option<&str>,
        outcome: AgentOutcome,
        message: &str,
    ) {
        let mut response = self.agent_result(host, pane, outcome, Some(message.into()));
        response.submission_id = requested_submission.map(str::to_owned);
        if outcome == AgentOutcome::TargetClosed && requested_submission.is_some() {
            response.message = Some(format!("{message}; no qualifying retained completion (missing proof, expired retention, or server restart)"));
        }
        if outcome == AgentOutcome::StatusUnavailable
            && let Some(readiness) = response.agents.first().and_then(|s| s.readiness.as_ref())
        {
            let failures = readiness.failures();
            if !failures.is_empty() {
                response.message = Some(format!("{message}: {}", failures.join(", ")));
            }
        }
        self.agent_reply(host, client, response);
    }

    pub(in crate::screen) fn closeout_fence(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
    ) -> std::result::Result<yinz_types::agent::control::CloseoutFence, String> {
        let status = self
            .agent_status(host, pane, None)
            .ok_or("Controller identity unavailable; inspect the original pane")?;
        let lifecycle = self
            .agent_lifecycle
            .get(&pane)
            .ok_or("Controller hooks unavailable")?;
        let revision = host.agent_input_revision(pane);
        let turn = lifecycle
            .closeout_turn
            .as_ref()
            .filter(|turn| !turn.is_empty())
            .ok_or("Provider has no current turn identity; retain the pane")?;
        if !matches!(pane, PaneId::Terminal(_))
            || !status.status_available
            || status.agent.session_id.as_ref() != Some(&lifecycle.conversation)
            || lifecycle.conversation.is_empty()
            || status.agent.kind != lifecycle.kind
            || status.background
            || lifecycle.event_background()
            || !lifecycle.claude_notifications.is_empty()
            || self
                .agent_worker_waits
                .get(&pane)
                .is_some_and(|waits| !waits.waits.is_empty())
            || self.agent_draft_clears.contains_key(&pane)
            || self.agent_dialog_inputs.contains_key(&pane)
            || self.agent_clear_inputs.contains_key(&pane)
            || providers::draft_composer(host, pane, &status.agent.kind).is_some_and(|c| !c.empty)
            || (lifecycle.input_revision != Some(revision)
                && providers::draft_composer(host, pane, &status.agent.kind)
                    .is_none_or(|c| !c.empty))
        {
            return Err("Controller has background work, a draft or unverified input; inspect before closeout".into());
        }
        let pending: Vec<_> = self
            .agent_controls
            .iter()
            .filter(|p| p.pane == pane)
            .collect();
        if pending.len() > 1
            || pending.iter().any(|p| {
                p.delivery.is_some()
                    || p.enter_at.is_some()
                    || p.clear
                    || p.steering
                    || p.input_revision != revision
                    || p.conversation.as_ref() != Some(&lifecycle.conversation)
                    || p.prompt.as_ref().is_none_or(|prompt| {
                        !prompt.acknowledged()
                            || (if status.agent.kind == "claude" {
                                prompt.prompt_id()
                            } else {
                                prompt.turn_id()
                            }) != Some(turn.as_str())
                    })
            })
        {
            return Err(
                "Controller has additional in-flight control work; resolve it before closeout"
                    .into(),
            );
        }
        if !host.explicit_close_is_single_pane(pane) {
            return Err("Controller closeout would affect other panes".into());
        }
        Ok(yinz_types::agent::control::CloseoutFence {
            server_instance: yinz_bus::agent_launch::server_instance().to_owned(),
            pane_id: pane,
            provider: status.agent.kind,
            conversation: lifecycle.conversation.clone(),
            input_revision: revision,
            turn: turn.clone(),
        })
    }
}

impl AgentController {}

#[cfg(test)]
#[path = "../unit/agent_controller_tests.rs"]
mod controller_tests;
/// Pane identity and observations staged before asynchronous pane creation.
type PendingAgentTracking = (
    Option<yinz_types::agent::AgentInfo>,
    Option<yinz_types::agent::tracking::AgentTrackingHandoff>,
);

#[derive(Default)]
pub(super) struct AgentController {
    pub(super) agent_events: events::AgentEvents,
    pub(super) agent_worktrees: worktrees::Worktrees,
    pub(super) agent_controls: Vec<Pending>,
    pub(super) agent_draft_clears: HashMap<PaneId, DraftClear>,
    pub(super) agent_terminal_claims: HashMap<PaneId, (u64, String, String)>,
    pub(super) closed_agent_submissions: std::collections::VecDeque<(
        std::time::Instant,
        yinz_types::agent::control::RetainedSubmission,
        u64, // Input revision frozen before normal provider shutdown.
        Option<yinz_types::agent::claude_notifications::Source>,
    )>,
    pub(super) agent_submissions: HashMap<PaneId, std::collections::VecDeque<Submission>>,
    pub(super) agent_launches: HashMap<PaneId, yinz_types::agent::SpawnedAgent>,
    pub(super) pending_agent_tracking: HashMap<PaneId, PendingAgentTracking>,
    pub(super) pending_terminal_states: HashMap<PaneId, String>,
    pub(super) warped_agent_inputs: HashMap<PaneId, TransferredInput>,
    pub(super) agent_lifecycle: HashMap<PaneId, Lifecycle>,
    pub(super) restored_claude_observers: HashMap<PaneId, Instant>,
    pub(super) agent_dialog_inputs: HashMap<PaneId, DialogInput>,
    pub(super) agent_clear_inputs: HashMap<PaneId, ClearInput>,
    pub(super) agent_worker_waits: HashMap<PaneId, WorkerWaits>,
    pub(super) claude_permission_waits: HashMap<PaneId, PermissionWait>,
    /// Local hook activity, guarded by agent identity and sampled with monotonic frame time.
    pub(super) agent_hook_flashes: HashMap<PaneId, reports::HookFlash>,
}
