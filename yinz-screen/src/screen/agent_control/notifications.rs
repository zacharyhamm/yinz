//! Pane-owned positive obligations. Registry inactivity never settles an identified launch.
use super::*;
use yinz_types::agent::claude_notifications::{
    Answer, Evidence, Obligation, Pending, TaskKind, MAX_FACTS,
};

fn ingest(
    pending: &mut Pending,
    evidence: &Evidence,
    hook: &HookEvent,
    answer: Option<Answer>,
) -> Option<Answer> {
    let evidence = evidence.bounded();
    let source = evidence.source.as_ref()?;
    if pending.source.as_ref().is_some_and(|old| old != source) {
        return None; // Ownership is established before report state or evidence is applied.
    }
    pending.source = Some(source.clone());
    if pending.disabled {
        return None;
    }
    for task in &evidence.unsupported_tasks {
        for obligation in &mut pending.launches {
            if &obligation.launch.task == task && obligation.answer_prompt.is_none() {
                obligation.unavailable = true;
            }
        }
    }
    for launch in &evidence.launches {
        if !pending.launches.iter().any(|o| &o.launch == launch) {
            pending.launches.push(Obligation {
                launch: launch.clone(),
                answer_prompt: None,
                remaining_ms: MAX_TIMEOUT_MS,
                settled: false,
                unavailable: false,
            });
        }
    }
    if let Some(notice) = &evidence.notice {
        // A run is immutable, even after settlement and task-ID reuse.
        if !pending.notices.iter().any(|n| n.run == notice.run) {
            pending.notices.push(notice.clone());
        }
    }
    if let Some(cancel) = &evidence.cancellation
        && !pending.cancellations.contains(cancel)
    {
        pending.cancellations.push(cancel.clone());
    }
    if let Some(answer) = answer
        // Without positive work there is no future cancellation to join. A native
        // notification submit can precede its structural acceptance report, though.
        && (pending.active()
            || answer.notification
            || answer_needed(pending, &answer.prompt))
        && !pending.answers.iter().any(|a| a.prompt == answer.prompt)
    {
        pending.answers.push(answer);
    }
    // Foreground answers can precede a reordered cancellation, but are not
    // dependencies until positive attribution arrives. Keep a bounded recent cache;
    // never evict an answer needed by an identified notice/cancellation or an
    // accepted notification whose structural row has not been reported yet.
    while pending.answers.len() > MAX_FACTS {
        let disposable = pending
            .answers
            .iter()
            .enumerate()
            .filter(|(_, a)| !a.notification && !answer_needed(pending, &a.prompt))
            .min_by_key(|(_, a)| a.sequence)
            .map(|(i, _)| i);
        let Some(index) = disposable else { break };
        pending.answers.remove(index);
    }
    if pending.launches.len() > MAX_FACTS
        || pending.notices.len() > MAX_FACTS
        || pending.answers.len() > MAX_FACTS
        || pending.cancellations.len() > MAX_FACTS
    {
        // Capacity is observation loss, never eviction/reuse of a dependent tombstone.
        pending.disabled = true;
        pending.launches.clear();
        pending.notices.clear();
        pending.cancellations.clear();
        pending.answers.clear();
        return None;
    }
    for notice in &pending.notices {
        let eligible: Vec<_> = pending
            .launches
            .iter()
            .enumerate()
            .filter(|(_, o)| o.launch.task == notice.task && o.launch.tool == notice.tool)
            .map(|(i, _)| i)
            .collect();
        if eligible.len() == 1 {
            let obligation = &mut pending.launches[eligible[0]];
            if obligation.answer_prompt.is_none() {
                obligation.answer_prompt = Some(notice.prompt.clone());
            }
        }
    }
    for cancel in &pending.cancellations {
        if pending.launches.iter().any(|o| {
            o.launch.task == cancel.task && o.answer_prompt.as_ref() == Some(&cancel.prompt)
        }) {
            continue;
        }
        let eligible: Vec<_> = pending
            .launches
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.launch.task == cancel.task
                    && o.launch.kind == TaskKind::Bash
                    && !o.settled
                    && !o.unavailable
            })
            .map(|(i, _)| i)
            .collect();
        if eligible.len() == 1 {
            let obligation = &mut pending.launches[eligible[0]];
            if obligation
                .answer_prompt
                .as_ref()
                .is_none_or(|p| p == &cancel.prompt)
            {
                obligation.answer_prompt = Some(cancel.prompt.clone());
            }
        }
    }
    // Binding can arrive in this same unavailable report, or after budget expiry.
    // Apply loss after the positive joins so it reaches the identified answer lifetime.
    if evidence.unavailable {
        for obligation in &mut pending.launches {
            if hook.prompt_id.as_ref().is_none_or(|p| {
                p == &obligation.launch.prompt || obligation.answer_prompt.as_ref() == Some(p)
            }) {
                obligation.unavailable = true;
            }
        }
    }
    let mut final_answer: Option<Answer> = None;
    for obligation in &mut pending.launches {
        if !obligation.settled
            && !obligation.unavailable
            && let Some(answer) = pending
                .answers
                .iter()
                .find(|a| obligation.answer_prompt.as_ref() == Some(&a.prompt))
        {
            obligation.settled = true;
            if final_answer
                .as_ref()
                .is_none_or(|a| a.sequence <= answer.sequence)
            {
                final_answer = Some(answer.clone());
            }
        }
    }
    final_answer
}

fn answer_needed(pending: &Pending, prompt: &str) -> bool {
    pending.notices.iter().any(|n| n.prompt == prompt)
        || pending.cancellations.iter().any(|c| c.prompt == prompt)
        || pending
            .launches
            .iter()
            .any(|o| o.answer_prompt.as_deref() == Some(prompt))
}

/// Unavailable observation releases only its accepted processing lifetime. Native
/// input waits retain their own ownership until normal release. Keep turn/run
/// tombstones so duplicate accepts cannot reopen processing and real answers still apply.
fn release_unavailable_notifications(lifecycle: &mut Lifecycle) {
    lifecycle
        .claude_notifications
        .retain(|prompt, notification| {
            let mut bound = lifecycle
                .claude
                .obligations
                .launches
                .iter()
                .filter(|o| o.answer_prompt.as_ref() == Some(prompt))
                .peekable();
            bound.peek().is_none()
                || bound.any(|o| !o.unavailable)
                || retain_notification_wait(notification)
        });
}

fn retain_notification_wait(notification: &mut ClaudeNotification) -> bool {
    notification.observation_unavailable = true;
    notification.waiting.is_some() || notification.permission_revision.is_some()
}

fn release_notification_observation(lifecycle: &mut Lifecycle, prompt: &str) {
    lifecycle
        .claude_notifications
        .retain(|id, notification| id != prompt || retain_notification_wait(notification));
}

/// Drop a failed processing entry once its independently known native wait releases.
pub(super) fn prune_released_notification_waits(lifecycle: &mut Lifecycle) {
    lifecycle.claude_notifications.retain(|_, notification| {
        !notification.observation_unavailable
            || notification.waiting.is_some()
            || notification.permission_revision.is_some()
    });
}

impl Screen {
    pub(in crate::screen) fn preserve_claude_start_state(
        &self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        hook.hook_event_name == "SessionStart"
            && !hook.is_worker()
            && !hook.archive_only
            && self.agent_lifecycle.get(&pane).is_some_and(|l| {
                l.kind == "claude"
                    && l.conversation == hook.session_id
                    && l.claude.parent.is_some()
                    && l.claude.obligations.source.is_some()
                    && hook
                        .claude_notifications
                        .as_ref()
                        .is_some_and(|e| e.source == l.claude.obligations.source)
            })
    }

    /// Only native startup or a positive launch can establish a replacement provider.
    /// Reject other incarnations before they can affect turns, input, waits or presentation.
    pub(in crate::screen) fn accept_claude_source(
        &mut self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        let Some(old) = self
            .agent_lifecycle
            .get(&pane)
            .filter(|l| l.kind == "claude" && l.conversation == hook.session_id)
            .and_then(|l| l.claude.obligations.source.as_ref())
        else {
            return true;
        };
        let Some(evidence) = hook.claude_notifications.as_ref().map(Evidence::bounded) else {
            return true; // Preserve compatibility with reporters lacking process identity.
        };
        let Some(source) = evidence.source.as_ref().filter(|source| *source != old) else {
            return true;
        };
        let startup = hook.hook_event_name == "SessionStart"
            && hook.claude_turn_complete.is_none()
            && hook.claude_recovered_after.is_none()
            && matches!(
                hook.source.as_deref(),
                Some("startup" | "resume" | "clear" | "fork")
            );
        let launch = hook.hook_event_name == "PostToolUse"
            && hook.is_error != Some(true)
            && hook.error.is_none()
            && !evidence.launches.is_empty()
            && evidence
                .launches
                .iter()
                .all(|l| hook.prompt_id.as_ref() == Some(&l.prompt));
        if source.started <= old.started
            || source.pid == 0
            || evidence.unavailable
            || hook.is_worker()
            || hook.archive_only
            || (!startup && !launch)
            || !self.agent_process_matches(pane, "claude")
            || !self.get_pane_info(pane).is_some_and(|info| {
                !info.is_held
                    && !info.exited
                    && info.target == yinz_types::command::PaneTarget::Local
            })
        {
            return false;
        }
        log::debug!("Claude provider replaced pane={pane:?} old={old:?} new={source:?}; old observation unavailable");
        // Requests belong to the provider they targeted. Retain completed submission
        // results, but never complete an unresolved old request with the new answer.
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.agent_controls) {
            if pending.pane == pane {
                self.finish_agent_control(&pending, AgentOutcome::TargetChanged, None, None);
            } else {
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        if let Some(operation) = self.agent_draft_clears.get_mut(&pane) {
            operation.failure = Some((
                AgentOutcome::TargetChanged,
                "Claude provider replaced".into(),
            ));
        }
        self.agent_dialog_inputs.remove(&pane);
        self.agent_clear_inputs.remove(&pane);
        self.warped_agent_inputs.remove(&pane);
        self.agent_worker_waits.remove(&pane);
        self.claude_permission_waits.remove(&pane);
        let input_revision = (self.agent_input_revision(pane) == 0).then_some(0);
        let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) else {
            return false;
        };
        // Replacing a process degrades its observation; it proves no task or answer
        // completed. No old notification, permission or worker lifetime can travel
        // into the replacement, even when the conversation ID is unchanged.
        lifecycle.claude = ClaudeActivity::default();
        lifecycle.claude.obligations.source = Some(source.clone());
        lifecycle.claude_background = None;
        lifecycle.claude_notifications.clear();
        lifecycle.claude_prompt_id = None;
        lifecycle.claude_previous_prompt_id = None;
        lifecycle.claude_confirmation = None;
        lifecycle.ended_turn = None;
        lifecycle.input_revision = input_revision;
        lifecycle.last_message = None;
        lifecycle.notification_tick = Instant::now();
        if launch {
            // A native successful launch positively identifies its root prompt even
            // when its earlier submit could not yet establish replacement ownership.
            let prompt = evidence.launches[0].prompt.clone();
            lifecycle.claude.sequence = 1;
            lifecycle.claude.turns.insert(
                prompt.clone(),
                ClaudeTurn {
                    awaiting_native_submit: false,
                    confirmation: None,
                    sequence: 1,
                },
            );
            lifecycle.claude_prompt_id = Some(prompt);
        }
        true
    }

    /// Called before confirmation filters/state application and again after lifecycle creation.
    pub(in crate::screen) fn retain_claude_evidence(&mut self, pane: PaneId, hook: &HookEvent) {
        if hook.is_worker() || hook.archive_only {
            return;
        }
        let confirmed_answer = hook.hook_event_name == "Stop"
            && hook.claude_turn_complete == Some(true)
            && !self.ignore_claude_confirmation(pane, hook)
            && !self.is_after_ended_turn(pane, "claude", hook);
        let Some(evidence) = &hook.claude_notifications else {
            return;
        };
        let Some(lifecycle) = self
            .agent_lifecycle
            .get_mut(&pane)
            .filter(|l| l.kind == "claude" && l.conversation == hook.session_id)
        else {
            return;
        };
        let was_active = lifecycle.claude.obligations.active();
        let notification_count = lifecycle.claude_notifications.len();
        let now = Instant::now();
        lifecycle.claude.obligations.elapse(
            now.saturating_duration_since(lifecycle.notification_tick)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        );
        lifecycle.notification_tick = now;
        let answer = confirmed_answer
            .then_some(hook.prompt_id.as_ref())
            .flatten()
            .and_then(|prompt| {
                lifecycle.claude.turns.get(prompt).map(|turn| Answer {
                    prompt: prompt.clone(),
                    sequence: turn.sequence,
                    text: hook.last_assistant_message.clone(),
                    notification: lifecycle.claude_notifications.contains_key(prompt),
                })
            });
        if evidence
            .launches
            .iter()
            .any(|l| hook.prompt_id.as_ref() != Some(&l.prompt))
            || evidence
                .notice
                .as_ref()
                .is_some_and(|n| hook.prompt_id.as_ref() != Some(&n.prompt))
            || evidence
                .cancellation
                .as_ref()
                .is_some_and(|c| hook.prompt_id.as_ref() != Some(&c.prompt))
        {
            return;
        }
        // Capacity loss clears the reducer's facts, so retain the source-attributed
        // processing prompts long enough to release their activity lifetimes too.
        let lost_prompts: Vec<_> = lifecycle
            .claude
            .obligations
            .notices
            .iter()
            .chain(evidence.notice.iter())
            .map(|n| n.prompt.clone())
            .chain(
                lifecycle
                    .claude
                    .obligations
                    .answers
                    .iter()
                    .filter(|a| a.notification)
                    .map(|a| a.prompt.clone()),
            )
            .collect();
        let was_disabled = lifecycle.claude.obligations.disabled;
        if let Some(answer) = ingest(&mut lifecycle.claude.obligations, evidence, hook, answer)
            && answer.sequence >= lifecycle.claude.message_sequence
        {
            lifecycle.claude.message_sequence = answer.sequence;
            lifecycle.last_message = answer.text;
        }
        let evidence = evidence.bounded();
        if evidence.source.is_some() && evidence.source == lifecycle.claude.obligations.source {
            if !was_disabled && lifecycle.claude.obligations.disabled {
                for prompt in lost_prompts {
                    release_notification_observation(lifecycle, &prompt);
                }
            }
            if evidence.unavailable {
                if let Some(prompt) = &hook.prompt_id {
                    release_notification_observation(lifecycle, prompt);
                } else {
                    // Source-wide loss may release only structurally attributed notices.
                    let prompts: Vec<_> = lifecycle
                        .claude
                        .obligations
                        .notices
                        .iter()
                        .map(|notice| notice.prompt.clone())
                        .collect();
                    for prompt in prompts {
                        release_notification_observation(lifecycle, &prompt);
                    }
                }
            }
        }
        release_unavailable_notifications(lifecycle);
        if (was_active && !lifecycle.claude.obligations.active())
            || notification_count != lifecycle.claude_notifications.len()
        {
            // Identity ingestion precedes duplicate confirmation filters. A repeated
            // Stop can add the first scoped loss evidence even if its state is ignored.
            self.publish_claude_observation_activity(pane);
        }
    }

    pub(super) fn expire_claude_observations(&mut self, now: Instant) {
        let mut changed = Vec::new();
        for (pane, lifecycle) in &mut self.agent_lifecycle {
            let active = lifecycle.claude.obligations.active();
            let notifications = lifecycle.claude_notifications.len();
            let elapsed = now
                .saturating_duration_since(lifecycle.notification_tick)
                .as_millis()
                .min(u64::MAX as u128) as u64;
            lifecycle.notification_tick = now;
            lifecycle.claude.obligations.elapse(elapsed);
            release_unavailable_notifications(lifecycle);
            if (active && !lifecycle.claude.obligations.active())
                || notifications != lifecycle.claude_notifications.len()
            {
                changed.push(*pane);
            }
        }
        for pane in changed {
            self.publish_claude_observation_activity(pane);
        }
    }

    fn publish_claude_observation_activity(&mut self, pane: PaneId) {
        let previous = self.get_pane_info(pane).and_then(|p| p.agent);
        self.reconcile_claude_parent(pane);
        let current = self.get_pane_info(pane).and_then(|p| p.agent);
        if previous != current
            && let Some(previous) = previous
            && let Some(current) = current
        {
            if let Some(tab) = self.tab_containing_pane_mut(pane) {
                tab.set_pane_agent(pane, Some(previous));
            }
            super::super::agent_reports::publish_pane_agent_activity(self, pane, current)
                .context("publish Claude notification observation activity")
                .non_fatal();
        }
    }
}
