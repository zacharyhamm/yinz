use super::*;
use yinz_agent_control::providers::claude_notifications::{
    drain_wait, ingest, settle_drained, TurnFacts,
};
use yinz_types::agent::claude_notifications::{Answer, Evidence};

/// The newest turn the screen has accepted for this lifecycle.
fn newest_claude_prompt(lifecycle: &Lifecycle) -> Option<String> {
    lifecycle
        .claude
        .activity
        .turns
        .iter()
        .find(|(_, turn)| turn.sequence == lifecycle.claude.activity.sequence)
        .map(|(prompt, _)| prompt.clone())
}

/// Unavailable observation releases only its accepted processing lifetime. Native
/// input waits retain their own ownership until normal release. Keep turn/run
/// tombstones so duplicate accepts cannot reopen processing and real answers still apply.
fn release_unavailable_notifications(lifecycle: &mut Lifecycle) {
    lifecycle
        .claude
        .notifications
        .retain(|prompt, notification| {
            let mut bound = lifecycle
                .claude
                .activity
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
        .claude
        .notifications
        .retain(|id, notification| id != prompt || retain_notification_wait(notification));
}

/// Drop a failed processing entry once its independently known native wait releases.
pub(in crate::screen) fn prune_released_notification_waits(lifecycle: &mut Lifecycle) {
    lifecycle.claude.notifications.retain(|_, notification| {
        !notification.observation_unavailable
            || notification.waiting.is_some()
            || notification.permission_revision.is_some()
    });
}

impl super::AgentController {
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
                    && l.claude.activity.parent.is_some()
                    && l.claude.activity.obligations.source.is_some()
                    && hook
                        .claude_notifications
                        .as_ref()
                        .is_some_and(|e| e.source == l.claude.activity.obligations.source)
            })
    }
    /// When the screen must wake to settle a drained obligation whose grace is running.
    pub(in crate::screen) fn claude_drain_at(&self) -> Option<Instant> {
        self.agent_lifecycle
            .values()
            .filter_map(|lifecycle| {
                let remaining = drain_wait(&lifecycle.claude.activity.obligations)?;
                Some(lifecycle.claude.observed_at + Duration::from_millis(remaining))
            })
            .min()
    }
}

impl super::AgentController {
    pub(in crate::screen) fn retain_claude_evidence(
        &mut self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        if hook.is_worker() || hook.archive_only {
            return false;
        }
        let confirmed_answer = hook.hook_event_name == "Stop"
            && hook.claude_turn_complete == Some(true)
            && !self.ignore_claude_confirmation(pane, hook)
            && !self.is_after_ended_turn(pane, "claude", hook);
        let Some(evidence) = &hook.claude_notifications else {
            return false;
        };
        let Some(lifecycle) = self
            .agent_lifecycle
            .get_mut(&pane)
            .filter(|l| l.kind == "claude" && l.conversation == hook.session_id)
        else {
            return false;
        };
        let permission_active = lifecycle
            .claude
            .activity
            .permission
            .as_ref()
            .is_some_and(|p| p.active());
        let was_active = lifecycle.claude.activity.obligations.active();
        let notification_count = lifecycle.claude.notifications.len();
        let now = Instant::now();
        if let Some(permission) = &mut lifecycle.claude.activity.permission {
            permission.elapse(
                now.saturating_duration_since(lifecycle.claude.observed_at)
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            );
        }
        lifecycle.claude.activity.obligations.elapse(
            now.saturating_duration_since(lifecycle.claude.observed_at)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        );
        lifecycle.claude.observed_at = now;
        let answer = confirmed_answer
            .then_some(hook.prompt_id.as_ref())
            .flatten()
            .and_then(|prompt| {
                lifecycle
                    .claude
                    .activity
                    .turns
                    .get(prompt)
                    .map(|turn| Answer {
                        prompt: prompt.clone(),
                        sequence: turn.sequence,
                        text: hook.last_assistant_message.clone(),
                        notification: lifecycle.claude.notifications.contains_key(prompt),
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
            return false;
        }
        // Capacity loss clears the reducer's facts, so retain the source-attributed
        // processing prompts long enough to release their activity lifetimes too.
        let lost_prompts: Vec<_> = lifecycle
            .claude
            .activity
            .obligations
            .notices
            .iter()
            .chain(evidence.notice.iter())
            .map(|n| n.prompt.clone())
            .chain(
                lifecycle
                    .claude
                    .activity
                    .obligations
                    .answers
                    .iter()
                    .filter(|a| a.notification)
                    .map(|a| a.prompt.clone()),
            )
            .collect();
        let was_disabled = lifecycle.claude.activity.obligations.disabled;
        let newest = newest_claude_prompt(lifecycle);
        let turn = TurnFacts {
            newest: newest.as_deref(),
            idle: (confirmed_answer && hook.claude_background == Some(false))
                || hook.hook_event_name == yinz_types::agent::CLAUDE_BACKGROUND_IDLE,
        };
        if let Some(answer) = ingest(
            &mut lifecycle.claude.activity.obligations,
            evidence,
            hook,
            answer,
            turn,
        ) && answer.sequence >= lifecycle.claude.activity.message_sequence
        {
            lifecycle.claude.activity.message_sequence = answer.sequence;
            lifecycle.last_message = answer.text;
        }
        let evidence = evidence.bounded();
        if evidence.source.is_some()
            && evidence.source == lifecycle.claude.activity.obligations.source
        {
            if !was_disabled && lifecycle.claude.activity.obligations.disabled {
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
                        .activity
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
        if (permission_active
            && !lifecycle
                .claude
                .activity
                .permission
                .as_ref()
                .is_some_and(|p| p.active()))
            || (was_active && !lifecycle.claude.activity.obligations.active())
            || notification_count != lifecycle.claude.notifications.len()
        {
            // Identity ingestion precedes duplicate confirmation filters. A repeated
            // Stop can add the first scoped loss evidence even if its state is ignored.
            return true;
        }
        false
    }
    pub(in crate::screen) fn expire_claude_observations(&mut self, now: Instant) -> Vec<PaneId> {
        let mut changed = Vec::new();
        for (pane, lifecycle) in &mut self.agent_lifecycle {
            let permission_active = lifecycle
                .claude
                .activity
                .permission
                .as_ref()
                .is_some_and(|p| p.active());
            let active = lifecycle.claude.activity.obligations.active();
            let notifications = lifecycle.claude.notifications.len();
            let elapsed = now
                .saturating_duration_since(lifecycle.claude.observed_at)
                .as_millis()
                .min(u64::MAX as u128) as u64;
            lifecycle.claude.observed_at = now;
            if let Some(permission) = &mut lifecycle.claude.activity.permission {
                permission.elapse(elapsed);
            }
            lifecycle.claude.activity.obligations.elapse(elapsed);
            // This runs before every screen instruction; scan turns only while a drain waits.
            if drain_wait(&lifecycle.claude.activity.obligations).is_some() {
                let newest = newest_claude_prompt(lifecycle);
                settle_drained(
                    &mut lifecycle.claude.activity.obligations,
                    newest.as_deref(),
                );
            }
            release_unavailable_notifications(lifecycle);
            if (permission_active
                && !lifecycle
                    .claude
                    .activity
                    .permission
                    .as_ref()
                    .is_some_and(|p| p.active()))
                || (active && !lifecycle.claude.activity.obligations.active())
                || notifications != lifecycle.claude.notifications.len()
            {
                changed.push(*pane);
            }
        }
        changed
    }
}

impl super::AgentController {
    pub(in crate::screen) fn accept_claude_source(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        let Some(old) = self
            .agent_lifecycle
            .get(&pane)
            .filter(|l| l.kind == "claude" && l.conversation == hook.session_id)
            .and_then(|l| l.claude.activity.obligations.source.as_ref())
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
            || !host.agent_process_matches(pane, "claude")
            || !host.pane_info(pane).is_some_and(|info| {
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
                self.finish_agent_control(host, &pending, AgentOutcome::TargetChanged, None, None);
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
        self.claude.permission_waits.remove(&pane);
        let input_revision = (host.agent_input_revision(pane) == 0).then_some(0);
        let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane) else {
            return false;
        };
        // Replacing a process degrades its observation; it proves no task or answer
        // completed. No old notification, permission or worker lifetime can travel
        // into the replacement, even when the conversation ID is unchanged.
        lifecycle.claude.activity = ClaudeActivity::default();
        lifecycle.claude.activity.obligations.source = Some(source.clone());
        lifecycle.claude.background = None;
        lifecycle.claude.notifications.clear();
        lifecycle.prompt_id = None;
        lifecycle.previous_prompt_id = None;
        lifecycle.claude.confirmation = None;
        lifecycle.ended_turn = None;
        lifecycle.input_revision = input_revision;
        lifecycle.last_message = None;
        lifecycle.claude.observed_at = Instant::now();
        if launch {
            // A native successful launch positively identifies its root prompt even
            // when its earlier submit could not yet establish replacement ownership.
            let prompt = evidence.launches[0].prompt.clone();
            lifecycle.claude.activity.sequence = 1;
            lifecycle.claude.activity.turns.insert(
                prompt.clone(),
                ClaudeTurn {
                    awaiting_native_submit: false,
                    confirmation: None,
                    sequence: 1,
                },
            );
            lifecycle.prompt_id = Some(prompt);
        }
        true
    }
}
