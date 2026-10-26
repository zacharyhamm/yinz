//! Agent control is serialized with state reports and terminal input on the screen thread.
use super::Screen;
use crate::ClientId;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use yinz_ipc::ipc::ServerToClientMsg;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{
    claude_prompt_text, claude_task_notification_ids, is_claude_task_notification,
    is_command_prompt, needs_paste_lead_in, pi_prompt_text, supports_prompt,
    validate_submission_id, AgentOutcome, AgentReadiness, AgentRequest, AgentResponse, AgentStatus,
    NotReadyReason, PromptObservation, MAX_PROMPT_BYTES, MAX_TIMEOUT_MS, PASTE_LEAD_IN,
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

pub(super) struct PermissionWait {
    conversation: String,
    input_revision: u64,
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
    claude_prompt_id: Option<String>,
    claude_confirmation: Option<bool>,
    claude_previous_prompt_id: Option<String>,
    agy_confirmation: Option<bool>,
    /// Claude background tasks launched by confirmed turns whose `<task-notification>` turn
    /// has not been confirmed yet.
    background_tasks: BTreeSet<String>,
    /// Claude `<task-notification>` turns in flight, by prompt ID. They run beside the
    /// caller's prompt tracking, which they never replace.
    claude_notifications: BTreeMap<String, ClaudeNotification>,
    /// The last confirmed notification turn, so a duplicate submission hook is not a new one.
    claude_previous_notification: Option<String>,
    /// Tasks finished by notification turns confirmed before the caller turn they followed.
    /// Observers confirm turns independently, so the caller's confirmation can arrive last;
    /// it must not restart these tasks, and while this is set a later turn already supplied
    /// the final answer.
    claude_overtaken: Option<BTreeSet<String>>,
    /// The final assistant text of the last completed turn, until the next caller prompt.
    last_message: Option<String>,
}

impl Lifecycle {
    pub(super) fn event_background_tasks(&self) -> Vec<String> {
        self.background_tasks.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn background_tasks_for_test(&self) -> &BTreeSet<String> {
        &self.background_tasks
    }

    #[cfg(test)]
    pub(crate) fn last_message_for_test(&self) -> Option<&str> {
        self.last_message.as_deref()
    }

    /// The notification turn a Claude hook belongs to, by its prompt ID.
    fn claude_notification(&self, hook: &HookEvent) -> Option<&ClaudeNotification> {
        hook.prompt_id
            .as_ref()
            .and_then(|prompt| self.claude_notifications.get(prompt))
    }
}

#[derive(Clone)]
struct ClaudeNotification {
    /// The tasks the notification reports finished.
    tasks: Vec<String>,
    /// The caller prompt that was submitted but unconfirmed when this turn began, so this
    /// turn ran after it.
    after: Option<String>,
}

pub(super) struct DialogInput {
    kind: String,
    conversation: Option<String>,
    input_revision: u64,
    startup: bool,
    fresh_waiting: bool,
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

pub(super) struct Submission {
    id: String,
    kind: String,
    conversation: Option<String>,
    acknowledged: bool,
    turn_id: Option<String>,
    result: Option<SubmissionResult>,
}

#[derive(Clone)]
struct SubmissionResult {
    outcome: AgentOutcome,
    message: Option<String>,
    background_tasks: Vec<String>,
}

pub(super) struct Pending {
    client: Option<Reply>,
    pane: PaneId,
    kind: String,
    conversation: Option<String>,
    pub(super) deadline: Option<Instant>,
    input_revision: u64,
    prompt: Option<PromptObservation>,
    submission_id: Option<String>,
    clear: bool,
    wait: bool,
    until: AgentState,
    session_ended: bool,
    abandoned: bool,
    resolved: bool,
    delivery: Option<tokio::sync::oneshot::Receiver<yinz_bus::route::ActionCompletionResult>>,
    enter_at: Option<Instant>,
    submit_key: KeyWithModifier,
    submit_delay: Duration,
}

impl Pending {
    fn not_ready_reason(&self) -> Option<NotReadyReason> {
        self.prompt.as_ref().map(|prompt| {
            if prompt.acknowledged() {
                NotReadyReason::UnresolvedTurn
            } else {
                NotReadyReason::PendingSubmission
            }
        })
    }
}

impl Screen {
    pub(super) fn event_submission(&self, pane: PaneId, hook: &HookEvent) -> Option<String> {
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

    fn submission(&self, pane: PaneId, id: &str) -> Option<&Submission> {
        self.agent_submissions
            .get(&pane)?
            .iter()
            .find(|submission| submission.id == id)
    }

    fn record_submission(&mut self, pending: &Pending, result: Option<SubmissionResult>) {
        let Some(prompt) = &pending.prompt else {
            return;
        };
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
        submission.acknowledged = prompt.acknowledged();
        submission.turn_id = prompt.turn_id().map(str::to_owned);
        if pending.conversation.is_some() {
            submission.conversation = pending.conversation.clone();
        }
        if let Some(result) = result.filter(|_| submission.result.is_none()) {
            submission.result = Some(result);
        }
    }

    fn agent_task_notification(&self, pane: PaneId, kind: &str, hook: &HookEvent) -> bool {
        is_claude_task_notification(kind, hook)
            && !self.agent_controls.iter().any(|pending| {
                pending.pane == pane
                    && pending.kind == kind
                    && pending
                        .prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.matches_submission(hook))
            })
            && !self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == kind
                    && lifecycle.conversation == hook.session_id
                    && hook.prompt_id.is_some()
                    && lifecycle.claude_prompt_id == hook.prompt_id
            })
    }

    pub(super) fn has_agent_lifecycle(&self, pane: PaneId, kind: &str, conversation: &str) -> bool {
        self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == kind && lifecycle.conversation == conversation
        })
    }

    /// Whether the pane holds the launcher's verified Muse hook capability, still waiting on
    /// the provider's first turn to bind a conversation. Claimed by the marker alone.
    pub(super) fn has_provisional_muse_lifecycle(&self, pane: PaneId) -> bool {
        self.agent_lifecycle
            .get(&pane)
            .is_some_and(|lifecycle| lifecycle.kind == "muse" && lifecycle.conversation.is_empty())
    }

    /// Whether this hook is work from a turn that already ended by interruption or failure,
    /// such as the tool call Codex finishes after an Esc. Hooks without a turn id never are.
    pub(super) fn is_after_ended_turn(&self, pane: PaneId, kind: &str, hook: &HookEvent) -> bool {
        !matches!(
            hook.hook_event_name.as_str(),
            "UserPromptSubmit" | "PreInvocation" | "SessionStart" | "SessionEnd"
        ) && matches!(
            yinz_types::agent::hook_state_for(kind, hook),
            Some(AgentState::Busy | AgentState::Waiting)
        ) && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == kind
                && lifecycle.conversation == hook.session_id
                && lifecycle.ended_turn.is_some()
                && lifecycle.ended_turn == hook.turn_id
        })
    }

    pub(super) fn ignore_claude_confirmation(&self, pane: PaneId, hook: &HookEvent) -> bool {
        if hook.is_worker() {
            return false;
        }
        if hook.claude_turn_complete.is_some() {
            return !self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && (lifecycle.claude_notification(hook).is_some()
                        || (lifecycle.claude_prompt_id.is_some()
                            && lifecycle.claude_prompt_id == hook.prompt_id
                            && (lifecycle.claude_confirmation.is_none()
                                || (lifecycle.claude_confirmation == Some(false)
                                    && hook.claude_turn_complete == Some(true)))))
            });
        }
        if self.agent_task_notification(pane, "claude", hook) {
            // A repeated hook for a notification turn already in flight or confirmed.
            return self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && hook.prompt_id.as_ref().is_some_and(|prompt| {
                        lifecycle.claude_notifications.contains_key(prompt)
                            || lifecycle.claude_previous_notification.as_ref() == Some(prompt)
                    })
            });
        }
        if hook.hook_event_name == "UserPromptSubmit" && hook.prompt_id.is_some() {
            return self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && (lifecycle.claude_previous_prompt_id == hook.prompt_id
                        || (lifecycle.claude_prompt_id == hook.prompt_id
                            && lifecycle.claude_confirmation.is_some()))
            });
        }
        hook.hook_event_name == "Stop"
            && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                lifecycle.kind == "claude"
                    && lifecycle.conversation == hook.session_id
                    && lifecycle.claude_confirmation == Some(true)
            })
    }

    /// Whether a Claude confirmation must leave the agent busy: another notification turn
    /// started after the confirmed one and is still running.
    pub(super) fn claude_turn_continues(&self, pane: PaneId, hook: &HookEvent) -> bool {
        self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == "claude"
                && lifecycle.conversation == hook.session_id
                && lifecycle
                    .claude_notifications
                    .keys()
                    .any(|prompt| Some(prompt) != hook.prompt_id.as_ref())
        })
    }

    /// Claude background work an idle wait must still see finish.
    fn claude_background_pending(&self, pane: PaneId) -> bool {
        self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
            lifecycle.kind == "claude"
                && (!lifecycle.background_tasks.is_empty()
                    || !lifecycle.claude_notifications.is_empty())
        })
    }

    /// Track Claude background tasks and the final text of completed turns before pending
    /// operations are answered, so their replies carry both.
    fn track_agent_turn(&mut self, pane: PaneId, event: &AgentEvent, hook: &HookEvent) {
        let notification = self.agent_task_notification(pane, &event.kind, hook);
        let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|lifecycle| {
            lifecycle.kind == event.kind
                && lifecycle.conversation == hook.session_id
                && event.session_id.as_deref() == Some(hook.session_id.as_str())
        }) else {
            return;
        };
        if notification {
            if let Some(prompt) = hook.prompt_id.clone().filter(|id| !id.is_empty()) {
                let tasks = hook
                    .prompt
                    .as_deref()
                    .and_then(claude_task_notification_ids)
                    .unwrap_or_default();
                let after = lifecycle
                    .claude_prompt_id
                    .clone()
                    .filter(|_| lifecycle.claude_confirmation != Some(true));
                lifecycle
                    .claude_notifications
                    .insert(prompt, ClaudeNotification { tasks, after });
            }
            return;
        }
        match event.hook.as_str() {
            "UserPromptSubmit" => {
                lifecycle.last_message = None;
                lifecycle.claude_overtaken = None;
            },
            "Stop" if event.kind == "claude" => {
                let Some(complete) = hook.claude_turn_complete else {
                    return;
                };
                let overtaken = if let Some(notification) = hook
                    .prompt_id
                    .as_ref()
                    .and_then(|prompt| lifecycle.claude_notifications.remove(prompt))
                {
                    for task in &notification.tasks {
                        lifecycle.background_tasks.remove(task);
                    }
                    lifecycle.claude_previous_notification = hook.prompt_id.clone();
                    if notification.after.is_some()
                        && notification.after == lifecycle.claude_prompt_id
                        && lifecycle.claude_confirmation != Some(true)
                    {
                        lifecycle
                            .claude_overtaken
                            .get_or_insert_default()
                            .extend(notification.tasks);
                    }
                    None
                } else if complete {
                    lifecycle.claude_overtaken.take()
                } else {
                    None
                };
                if complete {
                    lifecycle.background_tasks.extend(
                        hook.claude_background_tasks
                            .iter()
                            .filter(|task| {
                                overtaken.as_ref().is_none_or(|done| !done.contains(*task))
                            })
                            .cloned(),
                    );
                    if overtaken.is_none() {
                        lifecycle.last_message = event.last_message.clone();
                    }
                }
            },
            "Stop" | "StopFailure" => lifecycle.last_message = event.last_message.clone(),
            _ => {},
        }
    }

    #[cfg(test)]
    pub(crate) fn claude_background_pending_for_test(&self) -> bool {
        self.claude_background_pending(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
            7,
        )))
    }

    #[cfg(test)]
    pub(crate) fn agent_status_for_test(&self) -> AgentStatus {
        self.agent_status(
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            None,
        )
        .expect("the test pane has an agent")
    }

    pub(super) fn ignore_agy_confirmation(&self, pane: PaneId, hook: &HookEvent) -> bool {
        if hook.is_worker() {
            return false;
        }
        let Some(lifecycle) = self.agent_lifecycle.get(&pane) else {
            return hook.agy_turn_complete.is_some();
        };
        if hook.agy_turn_complete.is_some() {
            if lifecycle.kind != "agy"
                || hook.session_id.is_empty()
                || lifecycle.conversation != hook.session_id
            {
                return true;
            }
            if hook.prompt_id.as_deref().is_none_or(str::is_empty)
                || lifecycle.claude_prompt_id != hook.prompt_id
            {
                return true;
            }
            return !(lifecycle.agy_confirmation.is_none()
                || (lifecycle.agy_confirmation == Some(false)
                    && hook.agy_turn_complete == Some(true)));
        }
        if hook.hook_event_name == "UserPromptSubmit" && hook.prompt_id.is_some() {
            return lifecycle.kind == "agy"
                && lifecycle.conversation == hook.session_id
                && (lifecycle.claude_previous_prompt_id == hook.prompt_id
                    || (lifecycle.claude_prompt_id == hook.prompt_id
                        && lifecycle.agy_confirmation.is_some()));
        }
        hook.hook_event_name == "Stop"
            && lifecycle.kind == "agy"
            && lifecycle.conversation == hook.session_id
            && lifecycle.agy_confirmation == Some(true)
    }

    fn remove_agent_lifecycle(&mut self, pane: PaneId, reason: &str) {
        self.warped_agent_inputs.remove(&pane);
        let previous = self.agent_lifecycle.remove(&pane);
        log::debug!(
            "agent lifecycle removed session={:?} pane={pane:?} reason={reason} previous={:?}",
            self.session_name,
            previous
                .as_ref()
                .map(|record| (&record.kind, &record.conversation)),
        );
    }

    pub(super) fn invalidate_agent_lifecycle(&mut self, pane: PaneId, reason: &str) {
        if let Some(operation) = self.agent_draft_clears.get_mut(&pane) {
            operation.failure = Some((
                AgentOutcome::TargetChanged,
                "agent identity invalidated".into(),
            ));
        }
        self.remove_agent_lifecycle(pane, reason);
        self.agent_dialog_inputs.remove(&pane);
        self.agent_clear_inputs.remove(&pane);
        self.agent_worker_waits.remove(&pane);
        self.claude_permission_waits.remove(&pane);
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.agent_controls) {
            if pending.pane == pane && pending.submission_id.is_some() {
                self.finish_agent_control(
                    &pending,
                    if self.destroyed_panes.contains(&pane) {
                        AgentOutcome::TargetClosed
                    } else {
                        AgentOutcome::TargetChanged
                    },
                    None,
                    None,
                );
            } else {
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        self.agent_submissions.remove(&pane);
    }

    pub(super) fn apply_agent_report_state(
        &mut self,
        report: &yinz_types::agent::report::AgentReport,
        hook: Option<&HookEvent>,
    ) -> Result<bool> {
        use yinz_types::agent::report::AgentReportState;
        use yinz_types::agent::{hook_state_for, waiting_detail, waiting_needs_a_person};
        let pane = report.pane_id;
        // Only the matching clear lifecycle may carry its submitted input across the
        // record removal and conversation replacement. All other invalidations discard it.
        let clear_transition = hook.is_some_and(|hook| {
            !hook.is_worker()
                && report.event.as_ref().is_some_and(|event| {
                    event.kind == report.kind
                        && PaneId::Terminal(event.pane_id) == pane
                        && event.hook == hook.hook_event_name
                        && event.session_id.as_deref() == Some(hook.session_id.as_str())
                })
                && self.agent_process_matches(pane, &report.kind)
                && self
                    .get_pane_info(pane)
                    .is_some_and(|info| !info.is_held && !info.exited)
                && self.agent_clear_inputs.get(&pane).is_some_and(|input| {
                    input.kind == report.kind
                        && Instant::now() < input.deadline
                        && ((hook.hook_event_name == "SessionEnd"
                            && hook.session_id == input.conversation)
                            || (input.session_ended
                                && hook.hook_event_name == "SessionStart"
                                && hook.source.as_deref() == Some("clear")
                                && !hook.session_id.is_empty()
                                && hook.session_id != input.conversation))
                })
        });
        if !clear_transition
            && hook.is_some_and(|hook| {
                !hook.is_worker()
                    && matches!(hook.hook_event_name.as_str(), "SessionStart" | "SessionEnd")
            })
        {
            self.agent_clear_inputs.remove(&pane);
        }
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
                if hook_state_for(&report.kind, hook) == Some(AgentState::Waiting)
                    && waiting_needs_a_person(hook)
                {
                    workers
                        .get_or_insert_with(|| WorkerWaits {
                            parent: parent.clone(),
                            waits: Default::default(),
                        })
                        .waits
                        .insert(id.clone(), waiting_detail(hook));
                } else if (hook_state_for(&report.kind, hook).is_some()
                    || matches!(hook.hook_event_name.as_str(), "SubagentStop" | "SessionEnd"))
                    && let Some(workers) = &mut workers
                {
                    workers.waits.remove(id);
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
        let permission_wait = if hook.is_some_and(HookEvent::is_worker) {
            self.claude_permission_waits.remove(&pane)
        } else {
            hook.filter(|hook| {
                report.kind == "claude"
                    && hook.hook_event_name == "Notification"
                    && hook.notification_type.as_deref() == Some("permission_prompt")
                    && !hook.archive_only
                    && agent.as_ref().is_some_and(|agent| {
                        agent.kind == "claude"
                            && agent.state == AgentState::Waiting
                            && agent.session_id.as_deref() == Some(hook.session_id.as_str())
                    })
            })
            .map(|hook| PermissionWait {
                conversation: hook.session_id.clone(),
                input_revision: self.agent_input_revision(pane),
            })
        };
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
        // The first verified startup changes the record's conversation and invalidates its
        // lifecycle. Carry only its CLI dialog input through that initial transition.
        let startup_input = hook
            .filter(|hook| {
                !hook.is_worker()
                    && hook.codex_hooks_ready != Some(false)
                    && self.agent_dialog_inputs.get(&pane).is_some_and(|input| {
                        input.startup
                            && input.kind == report.kind
                            && input
                                .conversation
                                .as_deref()
                                .is_none_or(|conversation| conversation == hook.session_id)
                    })
                    && ((hook_state_for(&report.kind, hook) == Some(AgentState::Idle)
                        && hook.hook_event_name == "SessionStart")
                        || (report.kind == "agy"
                            && matches!(
                                hook.hook_event_name.as_str(),
                                "UserPromptSubmit" | "PreInvocation"
                            )))
            })
            .and_then(|_| self.agent_dialog_inputs.remove(&pane));
        let clear_input = clear_transition
            .then(|| self.agent_clear_inputs.remove(&pane))
            .flatten();
        let result = super::thread_main::apply_pane_agent_report(self, pane, visible)
            .context("apply parent state and worker waits")?;
        if result && let Some(wait) = permission_wait {
            self.claude_permission_waits.insert(pane, wait);
        }
        if let Some(input) = startup_input {
            self.agent_dialog_inputs.insert(pane, input);
        }
        if let Some(input) = clear_input {
            self.agent_clear_inputs.insert(pane, input);
        }
        if let Some(workers) = workers.filter(|workers| !workers.waits.is_empty()) {
            self.agent_worker_waits.insert(pane, workers);
        }
        Ok(result)
    }

    pub(super) fn warped_agent_input(
        &self,
        pane: PaneId,
    ) -> Option<yinz_types::data::WarpedAgentInput> {
        let info = self.get_pane_info(pane)?;
        let agent = info.agent?;
        let conversation = agent.session_id?;
        if info.is_held
            || info.exited
            || agent.state == AgentState::Exited
            || agent.kind != "codex"
            || agent.source != AgentSource::Reported
            || !self.agent_process_matches(pane, &agent.kind)
        {
            return None;
        }
        let revision = self.agent_input_revision(pane);
        let consumed = if let Some(lifecycle) = self.agent_lifecycle.get(&pane) {
            if lifecycle.kind != agent.kind || lifecycle.conversation != conversation {
                return None;
            }
            lifecycle.input_revision == Some(revision)
        } else {
            let (input, input_revision) = self.warped_agent_inputs.get(&pane)?;
            if input.kind != agent.kind || input.conversation != conversation {
                return None;
            }
            input.consumed && *input_revision == revision
        };
        Some(yinz_types::data::WarpedAgentInput {
            kind: agent.kind,
            conversation,
            consumed: consumed
                && !self.agent_draft_clears.contains_key(&pane)
                && !self
                    .agent_controls
                    .iter()
                    .any(|pending| pending.pane == pane && pending.delivery.is_some()),
        })
    }

    pub(super) fn restore_warped_agent_input(
        &mut self,
        pane: PaneId,
        input: yinz_types::data::WarpedAgentInput,
    ) {
        if input.kind == "codex"
            && self.get_pane_info(pane).is_some_and(|info| {
                !info.is_held
                    && !info.exited
                    && info.agent.is_some_and(|agent| {
                        agent.source == AgentSource::Reported
                            && agent.kind == input.kind
                            && agent.session_id.as_ref() == Some(&input.conversation)
                    })
            })
            && self.agent_process_matches(pane, &input.kind)
        {
            let revision = self.agent_input_revision(pane);
            // Adopted panes start at revision zero; a parent hook can beat this restoration.
            if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|lifecycle| {
                lifecycle.kind == input.kind && lifecycle.conversation == input.conversation
            }) {
                if revision == 0 {
                    lifecycle.input_revision = input.consumed.then_some(0);
                }
            } else {
                self.warped_agent_inputs.insert(pane, (input, 0));
            }
        }
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

    pub(super) fn agent_status(
        &self,
        pane_id: PaneId,
        pending_claim: Option<NotReadyReason>,
    ) -> Option<AgentStatus> {
        let info = self.get_pane_info(pane_id)?;
        let agent = info.agent?;
        // The Muse bootstrap proves hook capability before the provider speaks.
        // Only its visible composer can establish readiness before the first turn.
        let provisional_muse = agent.kind == "muse"
            && agent.session_id.is_none()
            && self.has_provisional_muse_lifecycle(pane_id);
        let readiness = self.agent_readiness(pane_id, &agent, info.is_held, info.exited)?;
        let status_available = readiness.status_available();
        let prompt_supported = readiness.prompt_supported();
        let cwd = agent.cwd.clone().or_else(|| match pane_id {
            PaneId::Terminal(id) => self
                .terminal_cwds
                .get(&id)
                .map(|p| p.to_string_lossy().into_owned()),
            _ => None,
        });
        let not_ready_reason = if self.agent_draft_clears.contains_key(&pane_id) {
            Some(NotReadyReason::ClearingDraft)
        } else if !prompt_supported {
            Some(NotReadyReason::PromptUnsupported)
        } else if agent.state != AgentState::Idle {
            Some(NotReadyReason::NotIdle)
        } else if let Some(reason) = pending_claim.or_else(|| {
            self.agent_controls
                .iter()
                .filter(|pending| pending.pane == pane_id)
                .find_map(Pending::not_ready_reason)
        }) {
            Some(reason)
        } else if (provisional_muse && !self.muse_composer(pane_id, true))
            || !(self.agent_lifecycle.get(&pane_id).is_some_and(|lifecycle| {
                lifecycle.input_revision == Some(self.agent_input_revision(pane_id))
            }) || (provisional_muse
                && self.agent_dialog_inputs.get(&pane_id).is_some_and(|input| {
                    input.startup
                        && input.kind == "muse"
                        && input.conversation.is_none()
                        && input.input_revision == self.agent_input_revision(pane_id)
                        && self.agent_input_at(pane_id).is_some_and(|at| {
                            self.pane_output_activity
                                .get(&pane_id)
                                .is_some_and(|output| *output > at)
                        })
                })))
        {
            Some(NotReadyReason::UnconsumedInput)
        } else {
            None
        };
        let not_ready_reason = not_ready_reason.filter(|reason| {
            *reason != NotReadyReason::UnconsumedInput
                || self.draft_composer(pane_id, &agent.kind).is_none()
                || self.claude_background_pending(pane_id)
                || self
                    .agent_lifecycle
                    .get(&pane_id)
                    .is_some_and(|lifecycle| !lifecycle.background_tasks.is_empty())
        });
        Some(AgentStatus {
            pane_id,
            session: self.session_name.to_string(),
            host_id: yinz_runtime::host_identity::host_id(),
            tab_id: self.tab_containing_pane(pane_id).map(|tab| tab.id),
            is_suppressed: info.is_suppressed,
            target: info.target,
            cwd,
            branch: self
                .agent_launches
                .get(&pane_id)
                .and_then(|launch| launch.branch.clone()),
            ready: not_ready_reason.is_none(),
            not_ready_reason,
            status_available,
            prompt_supported,
            readiness: Some(readiness),
            input_revision: Some(self.agent_input_revision(pane_id)),
            clear_draft_supported: prompt_supported
                && self.draft_composer(pane_id, &agent.kind).is_some(),
            background_tasks: self
                .agent_lifecycle
                .get(&pane_id)
                .filter(|lifecycle| {
                    lifecycle.kind == agent.kind
                        && agent.session_id.as_ref() == Some(&lifecycle.conversation)
                })
                .map(|lifecycle| lifecycle.background_tasks.iter().cloned().collect())
                .unwrap_or_default(),
            agent,
        })
    }

    fn agent_readiness(
        &self,
        pane_id: PaneId,
        agent: &AgentInfo,
        held: bool,
        exited: bool,
    ) -> Option<AgentReadiness> {
        let tab = self.tab_containing_pane(pane_id)?;
        let pane = tab.get_pane_with_id(pane_id)?;
        let lifecycle = self.agent_lifecycle.get(&pane_id);
        let command = tab.pane_run_command(pane_id);
        Some(AgentReadiness {
            kind_supported: supports_prompt(&agent.kind),
            reported: agent.source == AgentSource::Reported,
            lifecycle_kind: lifecycle.map(|lifecycle| lifecycle.kind.clone()),
            lifecycle_session_id: lifecycle.map(|lifecycle| lifecycle.conversation.clone()),
            lifecycle_matches: agent.session_id.as_ref().is_some_and(|conversation| {
                self.has_agent_lifecycle(pane_id, &agent.kind, conversation)
            }),
            provisional_lifecycle: agent.kind == "muse"
                && agent.session_id.is_none()
                && self.has_provisional_muse_lifecycle(pane_id),
            last_report_age_ms: lifecycle.map(|lifecycle| {
                u64::try_from(lifecycle.last_report.elapsed().as_millis()).unwrap_or(u64::MAX)
            }),
            held,
            exited,
            agent_exited: agent.state == AgentState::Exited,
            suspended: pane.is_suspended(),
            sync_active: tab.is_sync_panes_active(),
            excluded_from_sync: pane.exclude_from_sync(),
            command_present: command.is_some(),
            command_kind: command.and_then(|run| {
                yinz_types::agent::kind_from_program_and_args(&run.command, &run.args)
            }),
            process_matches: self.agent_process_matches(pane_id, &agent.kind),
            bracketed_paste: pane.supports_bracketed_paste(),
        })
    }

    /// A pane with no agent record whose foreground program is a recognized agent, such as
    /// a `codex` someone started by hand in a shell. Listed so `agent list` agrees with
    /// `action context`, but never promptable: nothing ties its input to its conversation.
    pub(super) fn detected_agent_status(&self, pane_id: PaneId) -> Option<AgentStatus> {
        let PaneId::Terminal(id) = pane_id else {
            return None;
        };
        let info = self.get_pane_info(pane_id)?;
        if info.agent.is_some() || info.is_held || info.exited {
            return None;
        }
        let (program, args) = self.terminal_foreground_cmds.get(&id)?.split_first()?;
        let kind =
            yinz_types::agent::kind_from_program_and_args(std::path::Path::new(program), args)?;
        let cwd = self
            .terminal_cwds
            .get(&id)
            .map(|path| path.to_string_lossy().into_owned());
        let agent = AgentInfo {
            kind: kind.to_owned(),
            state: AgentState::Busy,
            source: AgentSource::Detected,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: cwd.clone(),
            session_id: None,
            run_id: None,
        };
        Some(AgentStatus {
            pane_id,
            session: self.session_name.to_string(),
            host_id: yinz_runtime::host_identity::host_id(),
            tab_id: self.tab_containing_pane(pane_id).map(|tab| tab.id),
            is_suppressed: info.is_suppressed,
            target: info.target,
            cwd: cwd.clone(),
            branch: None,
            ready: false,
            not_ready_reason: Some(NotReadyReason::Unmanaged),
            status_available: false,
            prompt_supported: false,
            input_revision: Some(self.agent_input_revision(pane_id)),
            clear_draft_supported: false,
            background_tasks: Vec::new(),
            readiness: self.agent_readiness(pane_id, &agent, info.is_held, info.exited),
            agent,
        })
    }

    /// Muse 1.4.2's composer has full-width rules and a visible cursor in its
    /// `❯ ` input block. Unknown layouts fail closed; hooks alone cannot identify it.
    fn muse_composer(&self, pane_id: PaneId, require_empty: bool) -> bool {
        let Some(pane) = self
            .tab_containing_pane(pane_id)
            .and_then(|tab| tab.get_pane_with_id(pane_id))
        else {
            return false;
        };
        let contents = pane.pane_contents(None, false, None);
        let Some((x, y)) = contents.cursor.map(|cursor| cursor.into_tuple()) else {
            return false;
        };
        if contents.is_scrolled || y >= contents.viewport.len() {
            return false;
        }
        let separator = |line: &String| {
            line.chars().count() == pane.get_content_columns() && line.chars().all(|c| c == '─')
        };
        if separator(&contents.viewport[y]) {
            return false;
        }
        let Some(top) = contents.viewport[..y].iter().rposition(separator) else {
            return false;
        };
        let Some(bottom) = contents.viewport[y + 1..]
            .iter()
            .position(separator)
            .map(|offset| y + 1 + offset)
        else {
            return false;
        };
        let input = &contents.viewport[top + 1..bottom];
        let Some(text) = input[0].strip_prefix('❯') else {
            return false;
        };
        if (!text.is_empty() && !text.starts_with(' ')) || (y == top + 1 && x < 2) {
            return false;
        }
        !require_empty
            || (y == top + 1
                && x == 2
                && text.trim().is_empty()
                && input[1..].iter().all(|line| line.trim().is_empty()))
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

    pub(super) fn agent_reply(&self, reply: &Reply, response: AgentResponse) {
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
        response.agents.extend(self.agent_status(pane, None));
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
            Restart {
                pane_id,
                prompt,
                session,
            } => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                match self.restart_agent_pane(*pane_id, prompt.as_deref(), *session, None) {
                    Ok(restarted) => {
                        response.restarted = Some(restarted);
                        self.log_and_report_session_state().non_fatal();
                        self.render().non_fatal();
                    },
                    Err(message) => {
                        response.outcome = AgentOutcome::Rejected;
                        response.message = Some(message);
                    },
                }
                self.agent_reply(&client, response);
                return;
            },
            SpawnPlacementSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-tab-placement-v1".into());
                self.agent_reply(&client, response);
                return;
            },
            SpawnSuppressedSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-suppressed-v1".into());
                self.agent_reply(&client, response);
                return;
            },
            SpawnBubblewrapSupport => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.output = Some("agent-bubblewrap-v1".into());
                self.agent_reply(&client, response);
                return;
            },
            List { .. } => {
                let mut response = AgentResponse::new(AgentOutcome::Ok);
                response.agents = self
                    .tabs
                    .values()
                    .flat_map(|tab| tab.get_all_pane_ids())
                    .filter_map(|id| {
                        self.agent_status(id, None)
                            .or_else(|| self.detected_agent_status(id))
                    })
                    .filter(|status| status.agent.state != AgentState::Exited)
                    .collect();
                self.agent_reply(&client, response);
                return;
            },
            Spawn { .. } | SpawnNewProject { .. } => {
                self.agent_reply(&client, AgentResponse::new(AgentOutcome::Rejected));
                return;
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
            Status {
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
        let reject = |screen: &Self, outcome, message: &str| {
            let mut response = screen.agent_result(pane, outcome, Some(message.into()));
            if outcome == AgentOutcome::StatusUnavailable
                && let Some(readiness) = response.agents.first().and_then(|s| s.readiness.as_ref())
            {
                let failures = readiness.failures();
                if !failures.is_empty() {
                    response.message = Some(format!("{message}: {}", failures.join(", ")));
                }
            }
            screen.agent_reply(&client, response);
        };
        if !matches!(pane, PaneId::Terminal(_)) {
            reject(
                self,
                AgentOutcome::Rejected,
                "target must be a terminal pane",
            );
            return;
        }
        let status = self.agent_status(pane, None);
        // SessionEnd clears the live record before a managed command enters its held state.
        let held_conversation = matches!(request, Show { .. } | Hide { .. })
            .then(|| self.tab_containing_pane(pane)?.get_pane_with_id(pane))
            .flatten()
            .filter(|pane| pane.is_held() && pane.exited() && pane.was_an_agent())
            .map(|pane| pane.last_agent_session_id());
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
                        input_revision: self.agent_input_revision(pane),
                        prompt: None,
                        submission_id: None,
                        clear: false,
                        wait: true,
                        until: *until,
                        session_ended: true,
                        abandoned: false,
                        resolved: false,
                        delivery: None,
                        enter_at: None,
                        submit_key: KeyWithModifier::new(BareKey::Enter),
                        submit_delay: Duration::ZERO,
                    });
                    return;
                }
            }
            let info = self.get_pane_info(pane);
            let exited = info.as_ref().is_some_and(|info| info.exited)
                && self
                    .tab_containing_pane(pane)
                    .is_some_and(|tab| tab.pane_is_agent(pane));
            let (outcome, message) = if info.is_none() {
                (AgentOutcome::TargetNotFound, "target pane does not exist")
            } else if exited {
                (AgentOutcome::TargetExited, "agent process has exited")
            } else if self.detected_agent_status(pane).is_some() {
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
            return;
        };
        if expected.is_some() && expected != conversation {
            reject(
                self,
                AgentOutcome::TargetChanged,
                "agent conversation changed",
            );
            return;
        }
        if matches!(request, Show { .. } | Hide { .. }) {
            let suppress = matches!(request, Hide { .. });
            if suppress
                && self
                    .get_pane_info(pane)
                    .is_some_and(|info| !info.is_suppressed)
                && !self.tabs.values().any(|tab| {
                    !tab.is_pending()
                        && !self.deferred_tab_closes.contains(&tab.id)
                        && tab.has_selectable_panes_except(pane)
                })
            {
                reject(
                    self,
                    AgentOutcome::Rejected,
                    "cannot hide the session's last selectable pane",
                );
                return;
            }
            if let Some(tab) = self.tab_containing_pane_mut(pane)
                && tab
                    .get_pane_info(pane)
                    .is_some_and(|info| info.is_suppressed != suppress)
            {
                if !suppress
                    && (tab.pane_tabs_of(pane).is_some()
                        || tab.pane_is_hidden_stack_list_member(&pane))
                {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "agent pane is hidden in a pane-tab group or stack; use pane-tab or stack navigation to show it",
                    );
                    return;
                }
                if !suppress
                    && tab
                        .overlay_placement_for(pane)
                        .is_some_and(|placement| placement.kind.is_modal())
                {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "cannot show a hidden modal overlay without changing focus; use overlay navigation to show it",
                    );
                    return;
                }
                if tab.pane_is_overlay(pane) {
                    if suppress {
                        tab.hide_overlay_pane(pane);
                    } else {
                        tab.show_overlay_pane(pane);
                    }
                } else if suppress {
                    tab.suppress_pane(pane, None);
                } else {
                    tab.unsuppress_pane(pane, false);
                }
            }
            if !self
                .get_pane_info(pane)
                .is_some_and(|info| info.is_suppressed == suppress)
            {
                reject(
                    self,
                    AgentOutcome::Rejected,
                    if suppress {
                        "agent pane could not be hidden"
                    } else {
                        "no room to show agent pane; it remains suppressed"
                    },
                );
                return;
            }
            self.log_and_report_session_state().non_fatal();
            self.render().non_fatal();
            self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None));
            return;
        }
        let Some(status) = status else {
            return;
        };
        if self.agent_draft_clears.contains_key(&pane)
            && matches!(request, Prompt { .. } | Recover { .. } | ClearDraft { .. })
        {
            reject(
                self,
                AgentOutcome::Rejected,
                "the composer is being cleared",
            );
            return;
        }
        match request {
            request @ ClearDraft { .. } => {
                self.start_draft_clear(pane, status, client, request, now)
            },
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
                self.finalize_agent_recovery(pane, &status.agent, self.agent_input_revision(pane));
                self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None));
            },
            Status { .. } => {
                let outcome = if status.agent.state == AgentState::Exited
                    && self.get_pane_info(pane).is_some_and(|info| info.exited)
                {
                    AgentOutcome::TargetExited
                } else if status.status_available {
                    AgentOutcome::Ok
                } else {
                    AgentOutcome::StatusUnavailable
                };
                let message = (outcome == AgentOutcome::TargetExited)
                    .then(|| "agent process has exited".into());
                self.agent_reply(&client, self.agent_result(pane, outcome, message));
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
                let all_lines: Vec<_> = output.lines().collect();
                let end = all_lines
                    .iter()
                    .rposition(|line| !line.trim().is_empty())
                    .map_or(0, |index| index + 1);
                let output = all_lines[end.saturating_sub(lines)..end]
                    .iter()
                    .skip_while(|line| line.trim().is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut response = self.agent_result(pane, AgentOutcome::Ok, None);
                response.output = Some(output);
                self.agent_reply(&client, response);
            },
            SendKeys { keys, .. } => {
                // An unpinned Muse pane may still show a startup dialog, but its
                // bootstrap cannot prove that subsequent keys were consumed.
                let provisional = status.agent.kind == "muse"
                    && status.agent.session_id.is_none()
                    && self.has_provisional_muse_lifecycle(pane);
                let startup = supports_prompt(&status.agent.kind)
                    && (status.agent.source == AgentSource::Spawned || provisional);
                if !(status.status_available || startup)
                    || !self.agent_process_matches(pane, &status.agent.kind)
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
                let revision = self.agent_input_revision(pane);
                let pre_submit_dialog = status.agent.kind == "pi"
                    && status.agent.is_waiting_on_a_person()
                    && self.agent_controls.iter().any(|pending| {
                        pending.pane == pane
                            && pending
                                .prompt
                                .as_ref()
                                .is_some_and(PromptObservation::waiting_before_submit)
                    });
                let dialog = ((startup && !(provisional && self.muse_composer(pane, false)))
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
                    .try_for_each(|key| self.agent_write_key(pane, key));
                match result {
                    Ok(()) => {
                        let input_revision = self.agent_input_revision(pane);
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
                        self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None))
                    },
                    Err(error) => {
                        self.agent_dialog_inputs.remove(&pane);
                        reject(self, AgentOutcome::Failed, &error.to_string());
                    },
                }
            },
            Prompt {
                preserve_draft: _,
                prompt,
                wait,
                timeout_ms,
                submission_id,
                ..
            } => {
                let submission_id =
                    submission_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                if let Err(message) = validate_submission_id(&submission_id) {
                    reject(self, AgentOutcome::Rejected, message);
                    return;
                }
                if self.submission(pane, &submission_id).is_some() {
                    let mut response = self.agent_result(pane, AgentOutcome::Rejected,
                        Some(format!("submission ID is already retained; inspect its outcome with agent wait {pane} --submission {submission_id}; no input was sent")));
                    response.submission_id = Some(submission_id);
                    self.agent_reply(&client, response);
                    return;
                }
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
                    reject(self, outcome, message);
                    return;
                }
                // Muse's bootstrap precedes workspace trust; only its composer proves startup finished.
                if status.agent.kind == "muse"
                    && status.agent.session_id.is_none()
                    && !self.muse_composer(pane, false)
                {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "Muse startup is incomplete; finish its setup dialogs before submitting",
                    );
                    return;
                }
                let trimmed = if status.agent.kind == "pi" {
                    pi_prompt_text(&prompt)
                } else {
                    prompt.trim()
                };
                if trimmed.is_empty()
                    || prompt.len() > MAX_PROMPT_BYTES
                    || prompt
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    reject(self, AgentOutcome::Rejected, "prompt must contain text, be at most 1 MiB, and contain no terminal controls");
                    return;
                }
                let Some(deadline) = deadline(now, timeout_ms) else {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "timeout must be between 1ms and 24h",
                    );
                    return;
                };
                let provider = yinz_types::agent::provider::provider(&status.agent.kind);
                let input = provider
                    .as_ref()
                    .and_then(|provider| provider.input.as_ref());
                if input.map_or_else(
                    || is_command_prompt(&status.agent.kind, &prompt),
                    |input| input.is_command_prompt(&prompt),
                ) {
                    reject(self, AgentOutcome::Rejected, "command input requires agent send-keys; prompt accepts text, exact Claude /clear, and exact Codex/Pi /new");
                    return;
                }
                let submit_key = match input
                    .map_or("Enter", |input| input.submit_key.as_str())
                    .parse()
                {
                    Ok(key) => key,
                    Err(_) => {
                        reject(
                            self,
                            AgentOutcome::StatusUnavailable,
                            "agent provider has an invalid submit key",
                        );
                        return;
                    },
                };
                let submit_delay = Duration::from_millis(input.map_or_else(
                    || {
                        if status.agent.kind == "claude" {
                            500
                        } else {
                            100
                        }
                    },
                    |input| input.submit_delay_ms,
                ));
                let observation = PromptObservation::with_input(
                    &status.agent.kind,
                    status.agent.session_id.as_deref().unwrap_or_default(),
                    &prompt,
                    input,
                );
                if !observation.supported() {
                    reject(
                        self,
                        AgentOutcome::StatusUnavailable,
                        "agent provider no longer supports prompt input",
                    );
                    return;
                }
                let prompt = if status.agent.kind == "claude" {
                    claude_prompt_text(&prompt).into_owned()
                } else {
                    prompt
                };
                if prompt.trim().is_empty() {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "prompt must contain text after Claude format-character normalization",
                    );
                    return;
                }
                if !status.background_tasks.is_empty() || self.claude_background_pending(pane) {
                    reject(
                        self,
                        AgentOutcome::Rejected,
                        "background work is still running",
                    );
                    return;
                }
                let clear_key = match input
                    .map_or("Ctrl c", |input| input.clear_key.as_str())
                    .parse::<KeyWithModifier>()
                {
                    Ok(key) => key,
                    _ => {
                        reject(
                            self,
                            AgentOutcome::StatusUnavailable,
                            "agent provider has an invalid clear key",
                        );
                        return;
                    },
                };
                let submissions = self.agent_submissions.entry(pane).or_default();
                while submissions.len() >= SUBMISSION_HISTORY_LIMIT {
                    submissions.pop_front();
                }
                submissions.push_back(Submission {
                    id: submission_id.clone(),
                    kind: status.agent.kind.clone(),
                    conversation: status.agent.session_id.clone(),
                    acknowledged: false,
                    turn_id: None,
                    result: None,
                });
                // Claim and observe before input; hooks cannot overtake this screen instruction.
                let pending = Pending {
                    client: Some(client.clone()),
                    pane,
                    kind: status.agent.kind.clone(),
                    conversation: status.agent.session_id.clone(),
                    deadline: Some(deadline),
                    input_revision: self.agent_input_revision(pane),
                    prompt: Some(observation),
                    submission_id: Some(submission_id),
                    clear: yinz_types::agent::control::is_conversation_reset(
                        &status.agent.kind,
                        &prompt,
                    ),
                    wait,
                    until: AgentState::Idle,
                    session_ended: false,
                    abandoned: false,
                    resolved: false,
                    delivery: None,
                    enter_at: None,
                    submit_key,
                    submit_delay,
                };
                // Expose Claude's slash command before its arguments can collapse into a chip,
                // and type Codex's `/new`: a pasted command is draft text, not a command.
                // Keep whitespace in the paste: typing a newline or tab would send a key.
                let (prefix, paste) = if let Some(input) = input {
                    (
                        input.paste_prefix.as_bytes().to_vec(),
                        prompt.replace('\n', &input.newline).into_bytes(),
                    )
                } else if needs_paste_lead_in(&status.agent.kind, &prompt) {
                    (
                        format!("{PASTE_LEAD_IN} ").into_bytes(),
                        prompt.into_bytes(),
                    )
                } else if (status.agent.kind == "claude" && prompt.starts_with('/'))
                    || yinz_types::agent::control::is_conversation_reset(
                        &status.agent.kind,
                        &prompt,
                    )
                {
                    let end = prompt.find(char::is_whitespace).unwrap_or(prompt.len());
                    (
                        prompt.as_bytes()[..end].to_vec(),
                        prompt.as_bytes()[end..].to_vec(),
                    )
                } else {
                    (Vec::new(), prompt.into_bytes())
                };
                self.begin_draft_clear(
                    pane,
                    status.agent,
                    client,
                    now,
                    deadline,
                    Some((pending, prefix, paste, clear_key)),
                );
            },
            WaitTurn {
                submission_id,
                timeout_ms,
                expected_session_id,
                ..
            } => {
                if let Err(message) = validate_submission_id(&submission_id) {
                    reject(self, AgentOutcome::Rejected, message);
                    return;
                }
                let Some(submission) = self.submission(pane, &submission_id) else {
                    let mut result = self.agent_result(pane, AgentOutcome::StatusUnavailable,
                        Some("submission is unknown or no longer retained; inspect the pane before retrying any prompt".into()));
                    result.submission_id = Some(submission_id);
                    self.agent_reply(&client, result);
                    return;
                };
                if expected_session_id.is_some() && expected_session_id != submission.conversation {
                    reject(
                        self,
                        AgentOutcome::TargetChanged,
                        "submission belongs to a different conversation",
                    );
                    return;
                }
                let Some(deadline) = deadline(now, timeout_ms) else {
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
                    kind: submission.kind.clone(),
                    conversation: submission.conversation.clone(),
                    deadline: Some(deadline),
                    input_revision: self.agent_input_revision(pane),
                    prompt: None,
                    submission_id: Some(submission_id),
                    clear: false,
                    wait: true,
                    until: AgentState::Idle,
                    session_ended: false,
                    abandoned: false,
                    resolved: false,
                    delivery: None,
                    enter_at: None,
                    submit_key: KeyWithModifier::new(BareKey::Enter),
                    submit_delay: Duration::ZERO,
                });
                self.refresh_agent_controls();
            },
            Wait {
                until, timeout_ms, ..
            } => {
                let Some(deadline) = deadline(now, timeout_ms) else {
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
                    submission_id: None,
                    clear: false,
                    wait: true,
                    until,
                    session_ended: false,
                    abandoned: false,
                    resolved: false,
                    delivery: None,
                    enter_at: None,
                    submit_key: KeyWithModifier::new(BareKey::Enter),
                    submit_delay: Duration::ZERO,
                });
                self.refresh_agent_controls();
            },
            List { .. }
            | Show { .. }
            | Hide { .. }
            | Restart { .. }
            | Spawn { .. }
            | SpawnNewProject { .. }
            | SpawnPlacementSupport
            | SpawnSuppressedSupport
            | SpawnBubblewrapSupport => {},
        }
    }

    fn deliver_agent_prompt(&mut self, mut pending: Pending, prefix: Vec<u8>, paste: Vec<u8>) {
        let pane = pending.pane;
        let kind = pending.kind.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        pending.delivery = Some(rx);
        self.agent_controls.push(pending);
        // Codex reads quickly typed text followed by a paste as one burst, and an empty
        // paste there discards the typed command, so a bare command is only typed.
        let result = if paste.is_empty() && matches!(kind.as_str(), "codex" | "pi") {
            self.write_to_pane_id(
                &None,
                prefix,
                false,
                pane,
                None,
                Some(yinz_bus::route::NotificationEnd::new(tx)),
            )
            .map(|_| ())
        } else {
            let typed = (!prefix.is_empty())
                .then(|| self.write_to_pane_id(&None, prefix, false, pane, None, None));
            match typed {
                Some(Err(error)) => Err(error),
                _ => self.paste_to_pane_id(
                    paste,
                    pane,
                    None,
                    Some(yinz_bus::route::NotificationEnd::new(tx)),
                ),
            }
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
                pending.not_ready_reason(),
            );
            pending.abandoned = true;
            pending.client = None;
            pending.deadline = None;
            self.agent_controls.push(pending);
        }
    }

    fn finalize_agent_recovery(&mut self, pane: PaneId, agent: &AgentInfo, revision: u64) {
        let state = agent.state;
        if state != AgentState::Idle {
            let mut idle = agent.clone();
            idle.state = AgentState::Idle;
            idle.detail = None;
            idle.waiting_on = None;
            idle.wait_needs_a_person = false;
            self.agent_worker_waits.remove(&pane);
            super::thread_main::apply_pane_agent_report(self, pane, Some(idle))
                .context("reset an interrupted agent to idle")
                .non_fatal();
            self.broadcast_interrupt(pane, agent)
                .context("announce an interrupted turn to plugins")
                .non_fatal();
        }
        if let Some(submissions) = self.agent_submissions.get_mut(&pane) {
            for submission in submissions
                .iter_mut()
                .filter(|submission| submission.result.is_none())
            {
                submission.result = Some(SubmissionResult {
                            outcome: AgentOutcome::Interrupted,
                            message: Some("Observation ended by explicit recovery; the task's completion was not verified.".into()),
                            background_tasks: Vec::new(),
                        });
            }
        }
        self.agent_controls
            .retain(|pending| pending.pane != pane || pending.prompt.is_none());
        self.agent_dialog_inputs.remove(&pane);
        self.agent_clear_inputs.remove(&pane);

        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.input_revision = Some(revision);
            lifecycle.background_tasks.clear();
            lifecycle.claude_notifications.clear();
            lifecycle.claude_overtaken = None;
            if lifecycle.kind == "claude" {
                lifecycle.claude_confirmation = Some(true);
            }
            if lifecycle.kind == "agy" {
                lifecycle.agy_confirmation = Some(true);
            }
        }
    }

    fn agent_write_key(&mut self, pane: PaneId, key: KeyWithModifier) -> Result<()> {
        self.tab_containing_pane_mut(pane)
            .context("agent pane no longer exists")?
            .write_key_to_pane_id(pane, key, None)
    }

    fn finish_agent_control(
        &mut self,
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
            self.agent_observation_uncertain(pending.pane, pending.submission_id.clone(), outcome);
        }
        let completed = pending.resolved
            || matches!(
                outcome,
                AgentOutcome::TargetChanged | AgentOutcome::TargetClosed
            );
        let background_tasks = self
            .agent_status(pending.pane, pending_claim)
            .map(|status| status.background_tasks)
            .unwrap_or_default();
        self.record_submission(
            pending,
            completed.then(|| SubmissionResult {
                outcome,
                message: message.clone(),
                background_tasks: background_tasks.clone(),
            }),
        );
        if let Some(client) = &pending.client {
            let mut response = AgentResponse::new(outcome);
            response.submission_id = pending.submission_id.clone();
            response
                .agents
                .extend(self.agent_status(pending.pane, pending_claim));
            response.message = message;
            if matches!(outcome, AgentOutcome::Completed | AgentOutcome::Ok) {
                response.background_tasks = background_tasks;
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
                    response.background_tasks = result.background_tasks.clone();
                }
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
                    let unresolved = response.agents.first().is_some_and(|status| {
                        status.prompt_supported
                            && status.agent.kind == pending.kind
                            && status.agent.session_id == pending.conversation
                            && (pending_claim.is_some()
                                || status.not_ready_reason == Some(NotReadyReason::UnconsumedInput))
                    });
                    if unresolved {
                        let conversation = pending.conversation.as_deref().unwrap_or_default();
                        let conversation = if !conversation.is_empty()
                            && conversation.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                            }) {
                            conversation.to_owned()
                        } else {
                            format!("'{}'", conversation.replace('\'', "'\\''"))
                        };
                        message.push_str(&format!(" Inspect with agent read {}. Only if the same live conversation has stopped and unresolved input remains, clear its draft and use agent recover {} --conversation {} --confirm-empty. Add --interrupted if it stopped without reporting idle.",
                            pending.pane, pending.pane, conversation));
                    }
                    response.message = Some(message);
                }
            }
            self.agent_reply(client, response);
        }
    }

    pub(super) fn refresh_agent_controls(&mut self) {
        self.refresh_agent_controls_at(Instant::now());
    }

    pub(crate) fn refresh_agent_controls_at(&mut self, now: Instant) {
        self.refresh_claude_permission_waits();
        self.refresh_draft_clears(now);
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
            let status = self.agent_status(pending.pane, claimed.get(&pending.pane).copied());
            let submission_result = pending
                .submission_id
                .as_deref()
                .filter(|_| pending.prompt.is_none())
                .and_then(|id| self.submission(pending.pane, id))
                .and_then(|submission| submission.result.as_ref());
            let mut outcome = match &status {
                _ if submission_result.is_some() => submission_result.map(|result| result.outcome),
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
                            || (status.ready && !self.claude_background_pending(pending.pane))) =>
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
            if pending.kind == "claude"
                && let (Some(enter), Some(output)) = (
                    pending.enter_at,
                    self.pane_output_activity.get(&pending.pane),
                )
            {
                pending.enter_at = Some(enter.max(*output + Duration::from_millis(100)));
            }
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
                        || (pending.kind == "muse"
                            && pending.conversation.is_none()
                            && !self.muse_composer(pending.pane, false))
                    {
                        outcome = Some(AgentOutcome::Interrupted);
                    } else {
                        match self.agent_write_key(pending.pane, pending.submit_key.clone()) {
                            Err(error) => {
                                delivery_error = Some(error.to_string());
                                outcome = Some(AgentOutcome::Failed);
                            },
                            _ => {
                                pending.input_revision = self.agent_input_revision(pending.pane);
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
                self.finish_agent_control(
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
                    pending.enter_at = None;
                    keep.push(pending);
                }
            } else {
                if pending.conversation.is_none() {
                    pending.conversation = self
                        .agent_status(pending.pane, None)
                        .and_then(|s| s.agent.session_id);
                    // Muse can announce its first conversation before acknowledging the
                    // prompt. The receipt must accept that pin as soon as the observer does.
                    self.record_submission(&pending, None);
                }
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
    }

    fn refresh_claude_permission_waits(&mut self) {
        for (pane, wait) in std::mem::take(&mut self.claude_permission_waits) {
            let Some(info) = self.get_pane_info(pane) else {
                continue;
            };
            let Some(mut agent) = info.agent else {
                continue;
            };
            if let Some(workers) = self.agent_worker_waits.get(&pane) {
                agent = workers.parent.clone();
            }
            if info.is_held
                || info.exited
                || agent.kind != "claude"
                || agent.session_id.as_deref() != Some(wait.conversation.as_str())
                || agent.state != AgentState::Waiting
            {
                continue;
            }
            let enter_revision = self
                .tab_containing_pane(pane)
                .and_then(|tab| tab.get_pane_with_id(pane))
                .map_or(0, |pane| pane.agent_enter_revision());
            if enter_revision <= wait.input_revision {
                self.claude_permission_waits.insert(pane, wait);
                continue;
            }
            // shortcut: Enter infers an answer and may open another dialog; use dialog detection if needed.
            agent.state = AgentState::Busy;
            agent.detail = None;
            agent.waiting_on = None;
            agent.wait_needs_a_person = false;
            if let Some(workers) = self.agent_worker_waits.get_mut(&pane) {
                workers.parent = agent;
            } else {
                super::thread_main::apply_pane_agent_report(self, pane, Some(agent))
                    .context("clear Claude permission wait after Enter")
                    .non_fatal();
            }
        }
    }

    pub(super) fn agent_control_event(&mut self, event: &AgentEvent, hook: Option<&HookEvent>) {
        let pane = PaneId::Terminal(event.pane_id);
        let parent = hook.is_some_and(|hook| !hook.is_worker());
        // A notification turn's confirmation never speaks for the caller's prompt.
        let claude_notification_stop = parent
            && hook.is_some_and(|hook| {
                hook.claude_turn_complete.is_some()
                    && self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                        lifecycle.kind == "claude"
                            && lifecycle.conversation == hook.session_id
                            && lifecycle.claude_notification(hook).is_some()
                    })
            });
        if parent
            && event.kind == "claude"
            && !claude_notification_stop
            && hook.is_some_and(|hook| hook.claude_turn_complete == Some(false))
            && let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane)
        {
            lifecycle.claude_confirmation = Some(false);
        }
        if parent
            && event.kind == "agy"
            && hook.is_some_and(|hook| hook.agy_turn_complete == Some(false))
            && let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane)
        {
            lifecycle.agy_confirmation = Some(false);
        }
        let mut clear_revision = None;
        let clear_started = self.get_pane_info(pane).is_some_and(|info| {
            info.agent.is_some_and(|agent| {
                agent.source == AgentSource::Reported
                    && agent.kind == event.kind
                    && agent.session_id == event.session_id
            })
        });
        if parent
            && let Some(input) = self.agent_clear_inputs.get_mut(&pane)
            && event.kind == input.kind
            && hook
                .is_some_and(|hook| event.session_id.as_deref() == Some(hook.session_id.as_str()))
        {
            if event.hook == "SessionEnd" {
                input.session_ended = true;
            } else if event.hook == "SessionStart" && clear_started {
                clear_revision = Some(input.input_revision);
            }
        }
        if clear_revision.is_some() {
            self.agent_clear_inputs.remove(&pane);
        }
        if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
            lifecycle.last_report = Instant::now();
        }
        if let Some(hook) = hook.filter(|_| parent) {
            self.track_agent_turn(pane, event, hook);
        }
        let notification = parent
            && hook.is_some_and(|hook| self.agent_task_notification(pane, &event.kind, hook));
        if parent
            && event.hook == "SessionStart"
            && hook.is_some_and(|hook| hook.source.as_deref() != Some("compact"))
            && self
                .agent_dialog_inputs
                .get(&pane)
                .is_some_and(|input| !input.startup)
        {
            self.agent_dialog_inputs.remove(&pane);
        }
        // Reconcile before replying to completion, so both prompt and wait replies see the
        // consumed dialog revision. A newer manual draft is never consumed with it.
        let mut dialog_revision = None;
        if let Some(input) = self.agent_dialog_inputs.get_mut(&pane)
            && event.kind == input.kind
            && hook.is_some_and(|hook| {
                event.session_id.as_deref() == Some(hook.session_id.as_str())
                    && !hook.session_id.is_empty()
                    && hook.codex_hooks_ready != Some(false)
                    && input
                        .conversation
                        .as_deref()
                        .is_none_or(|conversation| conversation == hook.session_id)
            })
        {
            let state = hook.and_then(|hook| yinz_types::agent::hook_state_for(&event.kind, hook));
            if state == Some(AgentState::Waiting) {
                input.fresh_waiting = true;
            } else if (input.startup
                && parent
                && ((event.hook == "SessionStart" && state == Some(AgentState::Idle))
                    || (input.kind == "agy"
                        && matches!(event.hook.as_str(), "UserPromptSubmit" | "PreInvocation")
                        && matches!(state, Some(AgentState::Busy | AgentState::Idle)))))
                || (!input.startup
                    && parent
                    && event.hook != "SessionStart"
                    && matches!(state, Some(AgentState::Busy | AgentState::Idle)))
            {
                dialog_revision = Some(input.input_revision);
            }
        }
        if let Some(revision) = dialog_revision {
            self.agent_dialog_inputs.remove(&pane);
            if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) {
                lifecycle.input_revision = Some(revision);
            }
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
            if outcome == Some(AgentOutcome::Submitted) && pending.conversation.is_none() {
                pending.conversation = event.session_id.clone();
            }
            let interrupted = (outcome == Some(AgentOutcome::Interrupted)
                && event.hook == "Interrupt")
                .then(|| SubmissionResult {
                    outcome: AgentOutcome::Interrupted,
                    message: Some(
                        format!(
                            "{} The turn was interrupted.",
                            event.last_message.as_deref().unwrap_or_default()
                        )
                        .trim()
                        .to_owned(),
                    ),
                    background_tasks: Vec::new(),
                });
            self.record_submission(&pending, interrupted);
            match outcome {
                Some(AgentOutcome::Submitted) => {
                    pending.enter_at = None;
                    if !pending.wait {
                        self.finish_agent_control(
                            &pending,
                            AgentOutcome::Submitted,
                            None,
                            pending.not_ready_reason(),
                        );
                        pending.client = None;
                    }
                    keep.push(pending);
                },
                Some(outcome) => {
                    pending.resolved =
                        matches!(outcome, AgentOutcome::Completed | AgentOutcome::Failed);
                    let retain_claim = (pending.resolved && pending.delivery.is_some())
                        || matches!(
                            outcome,
                            AgentOutcome::Interrupted
                                | AgentOutcome::StatusUnavailable
                                | AgentOutcome::NeedsInput
                        );
                    let message =
                        if event.hook == "Interrupt" && outcome == AgentOutcome::Interrupted {
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
                    self.finish_agent_control(
                        &pending,
                        outcome,
                        message,
                        retain_claim.then(|| pending.not_ready_reason()).flatten(),
                    );
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
        log::debug!(
            "agent lifecycle hook session={:?} pane={pane:?} kind={:?} conversation={:?} hook={:?} state={:?}",
            self.session_name, event.kind, event.session_id, event.hook,
            hook.and_then(|hook| yinz_types::agent::hook_state_for(&event.kind, hook)),
        );
        if hook.is_some_and(|hook| {
            hook.codex_hooks_ready == Some(false) || hook.muse_hooks_ready == Some(false)
        }) {
            self.remove_agent_lifecycle(pane, "hooks_unverified");
            self.agent_dialog_inputs.remove(&pane);
        } else if event.hook == "SessionEnd" {
            self.remove_agent_lifecycle(pane, "session_end");
            self.agent_dialog_inputs.remove(&pane);
            self.agent_worker_waits.remove(&pane);
        } else if event.kind == "muse"
            && event.session_id.is_none()
            && hook.is_some_and(|hook| {
                hook.hook_event_name == "SessionStart"
                    && hook.source.as_deref() != Some("compact")
                    && hook.session_id.is_empty()
                    && hook.muse_hooks_ready == Some(true)
                    && yinz_types::agent::hook_state_for(&event.kind, hook).is_some()
            })
        {
            // The launcher's startup announcement: verified hook wiring, but
            // unpinned, because Muse speaks no session id until its first turn.
            // Claimed by the marker alone - a provider hook never carries one.
            let previous = self
                .agent_lifecycle
                .get(&pane)
                .filter(|previous| previous.kind == "muse" && previous.conversation.is_empty());
            if previous.is_none() {
                log::debug!("agent lifecycle created pane={pane:?} kind=muse provisional=true");
            }
            let input_revision = clear_revision.or(dialog_revision).or(previous
                .map(|previous| previous.input_revision)
                .unwrap_or_else(|| (self.agent_input_revision(pane) == 0).then_some(0)));
            self.agent_lifecycle.insert(
                pane,
                Lifecycle {
                    kind: "muse".to_owned(),
                    conversation: String::new(),
                    input_revision,
                    last_report: Instant::now(),
                    ended_turn: previous.and_then(|previous| previous.ended_turn.clone()),
                    claude_prompt_id: None,
                    claude_confirmation: None,
                    claude_previous_prompt_id: None,
                    agy_confirmation: None,
                    background_tasks: BTreeSet::new(),
                    claude_notifications: BTreeMap::new(),
                    claude_previous_notification: None,
                    claude_overtaken: None,
                    last_message: None,
                },
            );
        } else if let Some(conversation) = event.session_id.as_ref().filter(|id| {
            !id.is_empty()
                && hook
                    .as_ref()
                    .and_then(|hook| yinz_types::agent::hook_state_for(&event.kind, hook))
                    .is_some()
        }) {
            // Some(None) means a transferred draft: even a startup report cannot
            // treat the destination's zero input revision as an empty source composer.
            let transferred = self
                .warped_agent_inputs
                .remove(&pane)
                .map(|(input, revision)| {
                    (input.kind == event.kind
                        && input.conversation == *conversation
                        && input.consumed
                        && revision == self.agent_input_revision(pane))
                    .then_some(revision)
                });
            let previous = self.agent_lifecycle.get(&pane).filter(|previous| {
                previous.kind == event.kind && previous.conversation == *conversation
            });
            if previous.is_none() {
                log::debug!(
                    "agent lifecycle created session={:?} pane={pane:?} kind={:?} conversation={conversation:?}",
                    self.session_name, event.kind,
                );
            }
            // Without tracked CLI dialog input, startup cannot consume a draft. Only a pane
            // with no input can establish consumption; matching evidence is preserved as-is.
            let input_revision = clear_revision
                .or(dialog_revision)
                .or(if event.hook == "UserPromptSubmit" && !notification {
                    accepted_input_revision
                } else {
                    None
                })
                .or(if previous.is_none() {
                    transferred.flatten()
                } else {
                    None
                })
                .or(match event.hook.as_str() {
                    "SessionStart" if transferred.is_some() && previous.is_none() => {
                        transferred.flatten()
                    },
                    "SessionStart" => previous
                        .map(|previous| previous.input_revision)
                        .unwrap_or_else(|| (self.agent_input_revision(pane) == 0).then_some(0)),
                    // Claude starts a notification turn by itself; it consumes no input.
                    "UserPromptSubmit" if !notification => accepted_input_revision.or_else(|| {
                        (event.kind == "agy"
                            && previous.is_none()
                            && self.agent_input_revision(pane) == 0)
                            .then_some(0)
                    }),
                    "PreInvocation" if event.kind == "agy" => {
                        accepted_input_revision.or_else(|| {
                            previous
                                .map(|previous| previous.input_revision)
                                .unwrap_or_else(|| {
                                    (self.agent_input_revision(pane) == 0).then_some(0)
                                })
                        })
                    },
                    // An uncorrelated acceptance may arrive after a newer draft. Tool progress
                    // and completion preserve consumption, including when it is unknown.
                    _ => previous.and_then(|previous| previous.input_revision),
                });
            let ended_turn = match event.hook.as_str() {
                "Interrupt" | "StopFailure" => hook
                    .and_then(|hook| hook.turn_id.clone())
                    .filter(|turn| !turn.is_empty()),
                "UserPromptSubmit" if !notification => None,
                "PreInvocation" => None,
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
                    claude_prompt_id: if event.hook == "UserPromptSubmit" && !notification {
                        hook.and_then(|hook| hook.prompt_id.clone())
                    } else {
                        previous.and_then(|previous| previous.claude_prompt_id.clone())
                    },
                    claude_previous_prompt_id: previous.and_then(|previous| {
                        if event.hook == "UserPromptSubmit"
                            && !notification
                            && (previous.claude_confirmation.is_some()
                                || previous.agy_confirmation.is_some())
                        {
                            previous.claude_prompt_id.clone()
                        } else {
                            previous.claude_previous_prompt_id.clone()
                        }
                    }),
                    claude_confirmation: if notification || claude_notification_stop {
                        previous.and_then(|previous| previous.claude_confirmation)
                    } else if event.hook == "UserPromptSubmit" {
                        None
                    } else if event.kind == "claude"
                        && matches!(event.hook.as_str(), "StopFailure" | "Interrupt")
                    {
                        Some(true)
                    } else {
                        hook.and_then(|hook| hook.claude_turn_complete)
                            .or_else(|| previous.and_then(|previous| previous.claude_confirmation))
                    },
                    background_tasks: previous
                        .map(|previous| previous.background_tasks.clone())
                        .unwrap_or_default(),
                    claude_notifications: previous
                        .map(|previous| previous.claude_notifications.clone())
                        .unwrap_or_default(),
                    claude_previous_notification: previous
                        .and_then(|previous| previous.claude_previous_notification.clone()),
                    claude_overtaken: previous
                        .and_then(|previous| previous.claude_overtaken.clone()),
                    last_message: previous.and_then(|previous| previous.last_message.clone()),
                    agy_confirmation: if matches!(
                        event.hook.as_str(),
                        "UserPromptSubmit" | "PreInvocation"
                    ) {
                        None
                    } else if event.kind == "agy"
                        && matches!(event.hook.as_str(), "StopFailure" | "Interrupt")
                    {
                        Some(true)
                    } else {
                        hook.and_then(|hook| hook.agy_turn_complete)
                            .or_else(|| previous.and_then(|previous| previous.agy_confirmation))
                    },
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
            .min()
    }

    pub(super) fn detach_agent_controls(&mut self, client: ClientId) {
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
                        &pending,
                        AgentOutcome::Interrupted,
                        Some("caller disconnected before submission".into()),
                        pending.not_ready_reason(),
                    );
                    pending.abandoned = true;
                    pending.deadline = None;
                    pending.enter_at = None;
                }
            }
            self.agent_controls.push(pending);
        }
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

impl Screen {
    /// Recognize only a fully bounded editor at the live cursor. Suggestions and
    /// collapsed paste/attachment labels are input, never evidence of emptiness.
    fn draft_composer(&self, pane: PaneId, kind: &str) -> Option<Composer> {
        let pane = self.tab_containing_pane(pane)?.get_pane_with_id(pane)?;
        let contents = pane.pane_contents(None, false, None);
        let (x, y) = contents.cursor?.into_tuple();
        if contents.is_scrolled || y >= contents.viewport.len() {
            return None;
        }
        let rows = &contents.viewport;
        let separator = |row: &String| {
            row.chars().count() == pane.get_content_columns() && row.chars().all(|c| c == '─')
        };
        let (top, bottom, prefix) = match kind {
            "claude" | "pi" => {
                let top = rows[..y].iter().rposition(separator)? + 1;
                let bottom = y + 1 + rows[y + 1..].iter().position(separator)?;
                let prefix = if kind == "claude" { 2 } else { 0 };
                if kind == "pi" {
                    // Pi has no prompt prefix. Its cyan borders distinguish the
                    // editor from full-width rules typed into a multiline draft.
                    let ansi = pane.pane_contents_with_ansi(None, false, None);
                    let top_rule = ansi.viewport.get(top - 1)?;
                    if !top_rule.starts_with("\x1b[38;5;6m")
                        || ansi.viewport.get(bottom) != Some(top_rule)
                    {
                        return None;
                    }
                }
                if kind == "claude"
                    && !rows[top].starts_with("❯\u{a0}")
                    && !rows[top].starts_with("❯ ")
                {
                    return None;
                }
                // Extra panels below the composer are not an editor footer.
                if rows[bottom + 1..]
                    .iter()
                    .filter(|row| !row.trim().is_empty())
                    .count()
                    > 2
                {
                    return None;
                }
                (top, bottom, prefix)
            },
            "codex" => {
                let top = rows[..=y].iter().rposition(|row| row.starts_with("› "))?;
                let is_footer = |row: &String| row.starts_with("  ") && row.contains(" · ");
                let bottom = y + 1 + rows[y + 1..].iter().position(is_footer)?;
                // Shift+Tab can leave the collaboration-mode footer above the
                // normal status row, occupying the editor's trailing blank row.
                let mode_footer = rows[bottom].trim_end().ends_with("Plan mode")
                    || rows[bottom].trim_end().ends_with("Default mode");
                let mut footer = rows[bottom + 1..]
                    .iter()
                    .filter(|row| !row.trim().is_empty());
                let extra_footer = footer.next();
                let two_row_mode = mode_footer && extra_footer.is_some_and(is_footer);
                if top == 0
                    || !rows[top - 1].trim().is_empty()
                    || (!two_row_mode && (bottom <= y + 1 || !rows[bottom - 1].trim().is_empty()))
                    || (extra_footer.is_some() && !two_row_mode)
                    || footer.next().is_some()
                    || rows[top + 1..bottom]
                        .iter()
                        .any(|row| !row.is_empty() && !row.starts_with("  "))
                {
                    return None;
                }
                (top, bottom, 2)
            },
            _ => return None,
        };
        if (y == top && x < prefix) || rows[top..bottom].iter().any(separator) {
            return None;
        }
        // Only the editor and its validated footer belong to this composer.
        // Codex also renders remote image rows immediately above the blank row
        // preceding its prompt. Local images and pastes are inline editor text.
        let remote_image = kind == "codex"
            && top.checked_sub(2).is_some_and(|row| {
                rows[row]
                    .trim_end()
                    .strip_prefix("  [Image #")
                    .and_then(|label| label.strip_suffix(']'))
                    .is_some_and(|number| {
                        !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                    })
            });
        let indicators = remote_image
            || rows[top..].iter().any(|row| {
                row.contains("[Image") || row.contains("[Pasted") || row.contains("[Attached")
            });
        // Captured Codex 0.160.x and Claude 2.1.289 placeholders are dim;
        // identically spelled typed drafts are not. Codex can style padding
        // after the hint, so a reset immediately after it is not required.
        let hint: String = rows[top].chars().skip(prefix).collect();
        let hint = hint.trim_end();
        let known_hint = match kind {
            "codex" => hint == "Ask Codex to do anything",
            "claude" => hint.starts_with("Try \"") && hint.ends_with('"'),
            _ => false,
        };
        let placeholder = known_hint
            && pane
                .pane_contents_with_ansi(None, false, None)
                .viewport
                .get(top)
                .is_some_and(|row| row.contains(&format!("\x1b[2m\x1b[23m{hint}")));
        let empty = !indicators
            && y == top
            && x == prefix
            && (rows[top].chars().skip(prefix).all(char::is_whitespace) || placeholder)
            && rows[top + 1..bottom]
                .iter()
                .all(|row| row.trim().is_empty());
        let mut before_cursor = String::new();
        for (index, row) in rows.iter().enumerate().take(y + 1).skip(top) {
            let row = if index < y {
                row.trim_end()
            } else {
                row.as_str()
            };
            let mut column = 0;
            for character in row.chars() {
                let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
                if index == y && column + width > x {
                    break;
                }
                if column >= prefix {
                    before_cursor.push(character);
                }
                column += width;
            }
        }
        Some(Composer {
            empty,
            before_cursor,
        })
    }

    fn start_draft_clear(
        &mut self,
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
        } else if input_revision != self.agent_input_revision(pane) {
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
        } else if !status.background_tasks.is_empty() || self.claude_background_pending(pane) {
            Some("background work is still running")
        } else if self
            .agent_input_at(pane)
            .is_some_and(|at| now.saturating_duration_since(at) < RECOVER_QUIET)
        {
            Some("clearing requires three seconds without terminal input; inspect again before retrying")
        } else if self.agent_input_at(pane).is_some_and(|at| {
            self.pane_output_activity
                .get(&pane)
                .is_none_or(|output| *output <= at)
        }) {
            Some("no fresh composer output after the latest input; inspect again before retrying")
        } else if status.agent.state != AgentState::Idle
            && (!interrupted
                || !matches!(status.agent.state, AgentState::Busy | AgentState::Waiting))
        {
            Some("clearing requires idle state, or --interrupted after verifying that work stopped")
        } else if status.agent.state != AgentState::Idle
            && !self.agent_lifecycle.get(&pane).is_some_and(|lifecycle| {
                self.agent_input_at(pane)
                    .is_some_and(|at| at > lifecycle.last_report)
            })
        {
            Some("interrupted recovery requires input after the last lifecycle report and three seconds of quiet")
        } else {
            None
        };
        if let Some(message) = refusal {
            self.agent_reply(
                &client,
                self.agent_result(pane, AgentOutcome::Rejected, Some(message.into())),
            );
            return;
        }
        let Some(_) = self.draft_composer(pane, &status.agent.kind) else {
            self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Rejected, Some("unsupported or hidden composer layout; inspect and clear manually, then use recover --confirm-empty".into())));
            return;
        };
        self.begin_draft_clear(
            pane,
            status.agent,
            client,
            now,
            now + Duration::from_millis(timeout_ms),
            None,
        );
    }

    fn begin_draft_clear(
        &mut self,
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
                let Some(composer) = self.draft_composer(pane, &agent.kind) else {
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
            revision: self.agent_input_revision(pane),
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
            self.write_draft_stage(pane, &mut operation, None, bytes, now);
        }
        self.agent_draft_clears.insert(pane, operation);
    }

    fn write_draft_stage(
        &mut self,
        pane: PaneId,
        operation: &mut DraftClear,
        key: Option<KeyWithModifier>,
        bytes: Vec<u8>,
        now: Instant,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        operation.delivery = Some(rx);
        operation.wrote_at = Some(now);
        let completion = Some(yinz_bus::route::NotificationEnd::new(tx));
        let result = if let Some(key) = key {
            self.tab_containing_pane_mut(pane)
                .context("agent pane no longer exists")
                .and_then(|tab| tab.write_key_to_pane_id(pane, key, completion))
        } else {
            // One queued write owns the entire bracketed paste, including its closing delimiter.
            self.write_to_pane_id(&None, bytes, false, pane, None, completion)
                .map(|_| ())
        };
        if let Err(error) = result {
            operation.failure = Some((AgentOutcome::Failed, error.to_string()));
        }
        operation.revision = self.agent_input_revision(pane);
    }

    fn refresh_draft_clears(&mut self, now: Instant) {
        for pane in self.agent_draft_clears.keys().copied().collect::<Vec<_>>() {
            let Some(mut operation) = self.agent_draft_clears.remove(&pane) else {
                continue;
            };
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
                        operation.failure =
                            Some((AgentOutcome::Failed, "writer disconnected".into()));
                    },
                }
            }
            let status = self.agent_status(pane, None);
            let failure = if self.get_pane_info(pane).is_none() {
                Some((AgentOutcome::TargetClosed, "pane closed"))
            } else if status.as_ref().is_none_or(|status| {
                status.agent.kind != operation.agent.kind
                    || status.agent.session_id != operation.agent.session_id
                    || status.agent.run_id != operation.agent.run_id
            }) {
                Some((AgentOutcome::TargetChanged, "agent identity changed"))
            } else if self.agent_input_revision(pane) != operation.revision {
                Some((
                    AgentOutcome::Interrupted,
                    "new terminal input cancelled clearing",
                ))
            } else if status.as_ref().is_none_or(|status| {
                !status.prompt_supported
                    || self.claude_background_pending(pane)
                    || status.agent.state != operation.agent.state
                    || !status.background_tasks.is_empty()
            }) || self
                .agent_lifecycle
                .get(&pane)
                .is_none_or(|lifecycle| lifecycle.last_report != operation.report)
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
                let composer = self.draft_composer(pane, &operation.agent.kind);
                let fresh = operation.wrote_at.is_none_or(|at| {
                    self.pane_output_activity
                        .get(&pane)
                        .is_some_and(|output| *output > at)
                });
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
                            self.deliver_agent_prompt(pending, prefix, paste);
                            continue;
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
                        self.write_draft_stage(pane, &mut operation, None, vec![3], now);
                    },
                    (DraftStage::Empty { since }, Some(composer)) if fresh && composer.empty => {
                        let since = since.get_or_insert(now);
                        // Any new output restarts stability, even if this snapshot is still empty.
                        let stable = self
                            .pane_output_activity
                            .get(&pane)
                            .copied()
                            .unwrap_or(*since)
                            .max(*since);
                        if now.saturating_duration_since(stable) >= Duration::from_millis(250) {
                            self.finalize_agent_recovery(
                                pane,
                                &operation.agent,
                                operation.revision,
                            );
                            if let Some(client) = &operation.client {
                                self.agent_reply(client, self.agent_result(pane, AgentOutcome::Ok, Some("Composer verified empty; task completion remains unverified.".into())));
                            }
                            continue;
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
                    pending.client = operation.client.take();
                    pending.resolved = true;
                    self.finish_agent_control(
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
                    let mut response = self.agent_result(pane, *outcome, Some(format!("{message}; partial clearing may have occurred. Inspect the composer before manual recovery.")));
                    if operation.delivery.is_some() {
                        for status in &mut response.agents {
                            status.ready = false;
                            status.not_ready_reason = Some(NotReadyReason::ClearingDraft);
                        }
                    }
                    self.agent_reply(&client, response);
                }
                // Keep ownership until every queued byte has been acknowledged, even on timeout.
                if operation.delivery.is_none() {
                    continue;
                }
            }
            self.agent_draft_clears.insert(pane, operation);
        }
    }
}
