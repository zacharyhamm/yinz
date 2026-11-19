//! Pane-owned positive obligations. Registry inactivity alone never settles an identified
//! launch: an idle registry cannot say whether Claude still owes that task's notification.
//! A launch settles through its correlated answer, or through per-task positive evidence
//! from Claude paired with idleness. When the newest turn's native root `Stop` snapshot no
//! longer lists the task as in flight, the obligation is drained. It settles once the
//! registry has read idle after that same turn and that turn has stayed the newest for
//! [`DRAIN_GRACE_MS`]. Claude computes registry presence independently of its notification
//! queue, so one idle reading can precede the turn of a notification it still owes; that
//! turn's submit within the grace supersedes the drain. A snapshot that lists the task
//! again also supersedes it. Settling this way records no answer.
use yinz_types::agent::claude_notifications::{
    Answer, Evidence, Obligation, Pending, TaskKind, MAX_FACTS,
};
use yinz_types::agent::{control::MAX_TIMEOUT_MS, HookEvent};

/// How long a drained obligation's turn must stay the newest after an idle registry
/// reading. Claude writes the registry from an asynchronous effect and may defer admitting
/// a queued notification, whose submit hook then arrives through a separate reporter.
const DRAIN_GRACE_MS: u64 = 3_000;

/// Facts about the hook's turn that only the lifecycle knows.
pub struct TurnFacts<'a> {
    /// The newest turn the screen has accepted for this lifecycle.
    pub newest: Option<&'a str>,
    /// The registry read idle after the hook's turn ended.
    pub idle: bool,
}

pub fn ingest(
    pending: &mut Pending,
    evidence: &Evidence,
    hook: &HookEvent,
    answer: Option<Answer>,
    turn: TurnFacts,
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
                drained: None,
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
    drain(pending, &evidence, hook, turn);
    final_answer
}

/// Apply a newest-turn `Stop` snapshot and registry idleness after answers, so an
/// answer that is available still settles its obligation and is published.
fn drain(pending: &mut Pending, evidence: &Evidence, hook: &HookEvent, turn: TurnFacts) {
    if let Some(prompt) = turn
        .newest
        .filter(|newest| hook.prompt_id.as_deref() == Some(*newest))
    {
        if hook.hook_event_name == "Stop"
            && hook.claude_turn_complete.is_none()
            && evidence.snapshot
            && !evidence.unavailable
        {
            for obligation in &mut pending.launches {
                if !obligation.settled && !obligation.unavailable {
                    obligation.drained = (!evidence
                        .tasks
                        .iter()
                        .any(|task| task.id == obligation.launch.task))
                    .then(|| prompt.to_owned());
                }
            }
        }
        if turn.idle && pending.idle_prompt.as_deref() != Some(prompt) {
            pending.idle_prompt = Some(prompt.to_owned());
            pending.idle_grace_ms = DRAIN_GRACE_MS;
        }
    }
    settle_drained(pending, turn.newest);
}

/// Settle obligations drained by the turn whose idle reading has outlasted the grace.
/// A newer turn makes that reading stale before anything settles.
pub fn settle_drained(pending: &mut Pending, newest: Option<&str>) {
    let Some(idle) = pending.idle_prompt.as_ref() else {
        return;
    };
    if newest != Some(idle.as_str()) {
        pending.idle_prompt = None;
        pending.idle_grace_ms = 0;
        return;
    }
    if pending.idle_grace_ms > 0 {
        return;
    }
    for obligation in &mut pending.launches {
        if !obligation.settled
            && !obligation.unavailable
            && obligation.drained.as_ref() == Some(idle)
        {
            log::debug!(
                "Claude background task drained task={:?} prompt={idle:?}",
                obligation.launch.task
            );
            obligation.settled = true;
        }
    }
}

/// Time left before a drained obligation's grace may settle it, while one is waiting.
pub fn drain_wait(pending: &Pending) -> Option<u64> {
    let idle = pending.idle_prompt.as_ref()?;
    pending
        .launches
        .iter()
        .any(|o| !o.settled && !o.unavailable && o.drained.as_ref() == Some(idle))
        .then_some(pending.idle_grace_ms)
}

fn answer_needed(pending: &Pending, prompt: &str) -> bool {
    pending.notices.iter().any(|n| n.prompt == prompt)
        || pending.cancellations.iter().any(|c| c.prompt == prompt)
        || pending
            .launches
            .iter()
            .any(|o| o.answer_prompt.as_deref() == Some(prompt))
}

#[cfg(test)]
#[path = "../unit/claude_drain.rs"]
mod tests;
