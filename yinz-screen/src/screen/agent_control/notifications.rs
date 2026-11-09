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
        // Old detached reports cannot replace a newer incarnation. Replacement requires
        // a newly observed launch; acceptance/completion never changes process ownership.
        if evidence.launches.is_empty() || source.started <= pending.source.as_ref()?.started {
            return None;
        }
        *pending = Pending::default();
    }
    pending.source = Some(source.clone());
    if pending.disabled {
        return None;
    }
    if evidence.unavailable {
        for obligation in &mut pending.launches {
            if hook.prompt_id.as_ref().is_none_or(|p| {
                p == &obligation.launch.prompt || obligation.answer_prompt.as_ref() == Some(p)
            }) {
                obligation.unavailable = true;
            }
        }
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
        && !pending.answers.iter().any(|a| a.prompt == answer.prompt)
    {
        pending.answers.push(answer);
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

impl Screen {
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
        if let Some(answer) = ingest(&mut lifecycle.claude.obligations, evidence, hook, answer)
            && answer.sequence >= lifecycle.claude.message_sequence
        {
            lifecycle.claude.message_sequence = answer.sequence;
            lifecycle.last_message = answer.text;
        }
    }

    pub(super) fn expire_claude_observations(&mut self, now: Instant) {
        let mut changed = Vec::new();
        for (pane, lifecycle) in &mut self.agent_lifecycle {
            let active = lifecycle.claude.obligations.active();
            let elapsed = now
                .saturating_duration_since(lifecycle.notification_tick)
                .as_millis()
                .min(u64::MAX as u128) as u64;
            lifecycle.notification_tick = now;
            lifecycle.claude.obligations.elapse(elapsed);
            if active && !lifecycle.claude.obligations.active() {
                changed.push(*pane);
            }
        }
        for pane in changed {
            let previous = self.get_pane_info(pane).and_then(|p| p.agent);
            self.reconcile_claude_parent(pane);
            let current = self.get_pane_info(pane).and_then(|p| p.agent);
            if let Some(previous) = previous
                && let Some(current) = current
            {
                if let Some(tab) = self.tab_containing_pane_mut(pane) {
                    tab.set_pane_agent(pane, Some(previous));
                }
                super::super::agent_reports::publish_pane_agent_activity(self, pane, current)
                    .context("publish unavailable Claude notification observation")
                    .non_fatal();
            }
        }
    }
}
