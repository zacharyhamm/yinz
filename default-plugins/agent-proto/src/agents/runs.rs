//! Run records, event identity and timeline updates.

use super::summaries::SUMMARY_TITLE_CHARS;
use super::{question_text, rename_title, rows, submitted_prompt, Summary};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use yinz_tile::prelude::{AgentInfo, HostId};
use yinz_types::internal_ids::SessionName;

// ---------------------------------------------------------------------------
// Hook-backed run history
// ---------------------------------------------------------------------------

/// One retained item in an agent run. Hooks are deliberately reduced to conversation history:
/// the raw event stream is noisy, large, and often contains tool output that has no business
/// being persisted or drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEntryKind {
    ToolCall,
    ToolResult,
    Prompt,
    Plan,
    Question,
    /// Kept only so histories written before questions had their own kind still deserialize.
    Step,
    /// A final response retained from Stop when message capture did not supply it.
    Result,
    /// Assistant prose, including commentary and attributed subagent output.
    Message,
    /// A summary a person asked for, kept on the timeline at the point they asked.
    ///
    /// The automatic summaries do not land here: they replace [`AgentRun::summary`], which
    /// the history half draws as the newest row of the timeline, because a run that rolls one
    /// forward at every turn end would otherwise grow a line per turn and the newest of them
    /// is the only one anybody wanted. A summary somebody pressed a key for is the opposite -
    /// it describes the run *at that moment*, and the moment is half of what it says - so it
    /// stays where it was made.
    ///
    /// Deliberately absent from the delta `run_summary_text` builds: a summary is the
    /// model's words about the agent, and feeding it back as though the agent had said it
    /// would let one paraphrase compound into the next.
    Summary,
}

impl RunEntryKind {
    pub fn label(self) -> &'static str {
        match self {
            RunEntryKind::ToolCall => "tool_call",
            RunEntryKind::ToolResult => "tool_result",
            RunEntryKind::Prompt => "prompt",
            RunEntryKind::Plan => "plan",
            RunEntryKind::Question => "question",
            RunEntryKind::Step => "step",
            RunEntryKind::Result => "result",
            RunEntryKind::Message => "message",
            RunEntryKind::Summary => "summary",
        }
    }

    /// The inverse of [`label`](Self::label), for reading an entry back out of its row.
    ///
    /// A `kind` column rather than a serialized enum so the rows stay legible and filterable
    /// from outside this crate; the two functions are each other's only definition of the
    /// spelling, and `every_kind_survives_its_label` holds them together.
    pub fn from_label(label: &str) -> Option<RunEntryKind> {
        match label {
            "tool_call" => Some(RunEntryKind::ToolCall),
            "tool_result" => Some(RunEntryKind::ToolResult),
            "prompt" => Some(RunEntryKind::Prompt),
            "plan" => Some(RunEntryKind::Plan),
            "question" => Some(RunEntryKind::Question),
            "step" => Some(RunEntryKind::Step),
            "result" => Some(RunEntryKind::Result),
            "message" => Some(RunEntryKind::Message),
            "summary" => Some(RunEntryKind::Summary),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<yinz_tile::agent::tool_capture::ToolCapture>,
    pub revision: u64,
    #[serde(default)]
    pub event_id: Option<String>,
    pub kind: RunEntryKind,
    pub text: String,
    /// When this entry was recorded, epoch milliseconds. `None` for an entry recorded before
    /// this field existed - a run resurrected from an old JSON `payload` blob, or a row from
    /// before the store column was added.
    #[serde(default)]
    pub created_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    #[default]
    Active,
    Completed,
    Closed,
    Replaced,
}

/// The metadata and selected history for one hook session.
///
/// The metadata is stored as one JSON `payload` value; the timeline is not, and has a row per
/// entry in `agent_run_entry` (`store::agent_run_entry_def`). Splitting them is what lets an
/// entry keep the whole of what the agent said: a blob has to be rewritten in full every time
/// the timeline grows, and the cost of that was paid for by cutting each entry's text down.
/// `rows::hydrate_entries` puts the two halves back together, and a run recorded before the
/// split still carries its entries inside its payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: String,
    pub pane_id: u32,
    #[serde(default)]
    pub yinz_session_name: SessionName,
    /// The machine whose server owns this run, with `yinz_session_name` — see
    /// [`rows::Owner`]. Not to be confused with [`AgentRun::host`], which is where the agent's
    /// *folder* is: an agent working over ssh on a build box is owned by the server that opened
    /// the pane, not by the box.
    ///
    /// Empty on runs recorded before the store could be shared between machines, which reads as
    /// "this one" and was true when there was only one. `rows::owned_or_unclaimed` is where
    /// that reading is implemented, and `rows::claim_unowned_runs` is what stops a restored run
    /// staying that way.
    #[serde(default)]
    pub host_id: HostId,
    #[serde(default)]
    pub session_id: String,
    /// Original and immediately preceding runs of an observed context clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_run_id: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub cwd: Option<String>,
    /// The machine `cwd` is on, absent for this one.
    ///
    /// Beside the folder rather than folded into it, exactly as `AgentPane` keeps it: a run
    /// recorded on a build box is work in a folder that is not one of this machine's, and a
    /// project filter that could not tell the two apart would claim it. Absent on every run
    /// written before this field existed, which reads as local and is what those runs were.
    #[serde(default)]
    pub host: Option<String>,
    /// The primary checkout whose Bubblewrap environment the run's pane was in, when it
    /// was in one. The conversation lives in that environment's private home, so a resume
    /// has to go back into it to find it - and must not quietly run the agent on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bubblewrap: Option<String>,
    pub started_at_ms: i64,
    #[serde(default)]
    pub ended_at_ms: Option<i64>,
    #[serde(default)]
    pub outcome: RunOutcome,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub summarized_revision: u64,
    #[serde(default)]
    pub summary: Option<Summary>,
    #[serde(default)]
    pub entries: Vec<RunEntry>,
    /// How many of the run's oldest entries `entries` leaves out, at least.
    ///
    /// Set by [`rows::hydrate_newest_entries`], which hands the panel only the newest end of a
    /// long run: without it the oldest event the history half holds reads as the start of the
    /// run. A lower bound, because the fetch stops at [`rows::RUN_DETAIL_ENTRIES`] and only
    /// says there was more, which counts as one. Zero on every stored run - a record is every
    /// entry it has - so it is only written when it says something.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_entries: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl AgentRun {
    /// Re-point this run at the pane and session that hold it now, keeping everything the run
    /// is made of.
    ///
    /// What a warp moves is the process, not the run: the same agent goes on doing the same
    /// work, and its prompts, plans, questions, results and summary are as much a part of it
    /// after the move as before. Only the two facts that were never the run's own — which pane
    /// it sits in and which yinz session owns that pane — are allowed to change.
    ///
    /// `ended_at_ms` and `outcome` are cleared because the session that lost the pane closed
    /// the run when it saw the pane leave. It could not tell a warp from a `kill`; this is the
    /// answer arriving afterwards.
    ///
    /// The new owner is both halves at once. A warp between machines moves the host as well as
    /// the session, and taking one without the other would leave the row claiming an owner that
    /// is half of each — which no guard would then match.
    pub fn adopted_by(mut self, pane_id: u32, owner: &rows::Owner) -> AgentRun {
        self.pane_id = pane_id;
        self.yinz_session_name = owner.yinz_session_name.clone();
        self.host_id = owner.host_id.clone();
        self.ended_at_ms = None;
        self.outcome = RunOutcome::Active;
        self
    }

    /// Who owns this run now.
    pub fn owner(&self) -> rows::Owner {
        rows::Owner::new(self.host_id.clone(), self.yinz_session_name.clone())
    }

    pub fn header(&self) -> AgentRunHeader {
        AgentRunHeader {
            id: self.id.clone(),
            pane_id: self.pane_id,
            yinz_session_name: self.yinz_session_name.clone(),
            host_id: self.host_id.clone(),
            origin_run_id: self.origin_run_id.clone(),
            previous_run_id: self.previous_run_id.clone(),
            session_id: self.session_id.clone(),
            kind: self.kind.clone(),
            cwd: self.cwd.clone(),
            host: self.host.clone(),
            bubblewrap: self.bubblewrap.clone(),
            started_at_ms: self.started_at_ms,
            ended_at_ms: self.ended_at_ms,
            outcome: self.outcome,
            revision: self.revision,
            summary: self.summary.clone(),
            title: run_title(self),
        }
    }
}

/// The cheap part of a run, carried in snapshots and archive pages. Full entries are fetched
/// only for the selected row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunHeader {
    pub id: String,
    pub pane_id: u32,
    #[serde(default)]
    pub yinz_session_name: SessionName,
    #[serde(default)]
    pub host_id: HostId,
    #[serde(default)]
    pub session_id: String,
    /// Original and immediately preceding runs of an observed context clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_run_id: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub cwd: Option<String>,
    /// The machine `cwd` is on, absent for this one. See [`AgentRun::host`].
    #[serde(default)]
    pub host: Option<String>,
    /// The checkout whose Bubblewrap environment the run was in. See [`AgentRun::bubblewrap`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bubblewrap: Option<String>,
    pub started_at_ms: i64,
    #[serde(default)]
    pub ended_at_ms: Option<i64>,
    #[serde(default)]
    pub outcome: RunOutcome,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub summary: Option<Summary>,
    #[serde(default)]
    pub title: String,
}

/// A run id derived from the agent's own session id. The kind is part of it so two providers
/// cannot collide, and nothing else is: the derivation has to reach the same answer on the far
/// side of a warp, where the pane and the session are both different. See [`stored_run_id`].
pub fn session_run_id(kind: &str, session_id: &str) -> String {
    format!("{kind}:{}", session_id.trim())
}

/// A stable run id where the agent supplies a session id, and a timestamped fallback for
/// legacy hook scripts that do not.
///
/// The fallback names the machine too, and has to. A pane id means nothing outside the server
/// that handed it out, so `kind:pane:5:<epoch>` is a name two servers sharing a data dir
/// can mint at once. The row
/// is written with an unguarded `Put` keyed on this id, so a collision does not fail: it
/// interleaves two agents' timelines under one uid and says nothing.
///
/// Appended rather than woven in, so that [`short_run_id`] reads old and new ids alike without
/// being taught the difference, and so a run recorded before this keeps the id it has. An
/// empty `host_id` — a caller that does not know its machine yet — leaves the old shape, which
/// is no worse than what it replaced.
pub fn run_id(
    kind: &str,
    pane_id: u32,
    session_id: &str,
    started_at_ms: i64,
    host_id: &HostId,
) -> String {
    match (session_id.trim(), host_id.as_str().trim()) {
        ("", "") => format!("{kind}:pane:{pane_id}:{started_at_ms}"),
        ("", host_id) => format!("{kind}:pane:{pane_id}:{started_at_ms}:{host_id}"),
        (session_id, _) => session_run_id(kind, session_id),
    }
}

/// A run id in the width a divider can afford: [`run_id`] read backwards, with the half no
/// person reads dropped.
///
/// Both shapes `run_id` builds are longer than an agents pane is wide - a session id is a
/// uuid, and the fallback ends in a raw epoch and the machine's own uuid - and `fit_label` cuts
/// from the tail, so the whole id is no id at all on a narrow half. A uuid is recognised by its
/// first few characters and nothing else, and the timestamp and the machine in the fallback say
/// only that the run began here, which the row it labels already says.
///
/// The `splitn(4, ':')` reaches both fallback shapes without knowing there are two: whatever
/// follows the epoch lands in the fourth part along with it, and the fourth part is discarded.
pub fn short_run_id(id: &str) -> String {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        // `kind:pane:<n>:<started at>` - the pane is the name, the epoch is bookkeeping.
        (Some(kind), Some("pane"), Some(pane_id), Some(_)) => format!("{kind}:pane:{pane_id}"),
        // `kind:<session id>` - as much of the session id as tells one run from another.
        (Some(kind), Some(session_id), None, None) => {
            let short: String = match session_id.chars().count() > KEEP_SESSION_ID_UPTO {
                true => session_id.chars().take(SHORT_SESSION_ID).collect(),
                false => session_id.to_owned(),
            };
            format!("{kind}:{short}")
        },
        // Neither shape: whatever it is, it is not ours to shorten.
        _ => id.to_owned(),
    }
}

/// The agent kind a run id names, read off the front. Every shape [`run_id`] builds leads with
/// it, which is what lets a row say something about the run an id points at without having that
/// run in hand — the archive is paged, so an ancestor is often not loaded.
///
/// `None` for an id of neither shape, and for one that leads with an empty kind: better to say
/// nothing than to name an agent called "".
pub fn run_id_kind(id: &str) -> Option<&str> {
    id.split_once(':')
        .map(|(kind, _)| kind)
        .filter(|kind| !kind.is_empty())
}

/// How much of a long session id [`short_run_id`] keeps. Eight characters of a uuid is the
/// length git settled on for the same job, and the same reasoning applies: the ids being told
/// apart are the handful a pane has run, not every id there could be.
const SHORT_SESSION_ID: usize = 8;

/// A session id at most this long is left whole: it already fits a divider, and cutting an id
/// that was going to be read in full is a loss for no room gained. Only the uuids are long
/// enough to be worth [`SHORT_SESSION_ID`].
const KEEP_SESSION_ID_UPTO: usize = 16;

/// The stored run a pane's record points at, where it points at one.
///
/// Two roads to the same answer, in order of authority. The record may *name* the run
/// outright — a plugin keeping the history says so with `set_pane_agent_run`, and that is the
/// only road open to an agent that supplies no session id of its own. Failing that, an agent
/// that does supply one has a run id derived from it, and the derivation is the same wherever
/// it happens ([`run_id`]), so it can be recomputed on the far side of a warp.
///
/// `None` where neither holds: a run keyed by a pane, a timestamp and a machine is a name only
/// the server that made it can pronounce, and there is nothing honest to do but start a new one.
///
/// Takes no pane id, and that absence is the point: the derived shape must not mention the
/// pane, or the far side of a warp — a different pane in a different session — could not reach
/// the same answer.
pub fn stored_run_id(agent: &AgentInfo) -> Option<String> {
    if let Some(run_id) = agent
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return Some(run_id.to_owned());
    }
    let session_id = agent.session_id.as_deref().unwrap_or_default().trim();
    // [`session_run_id`] rather than [`run_id`]: this branch is the session-derived shape by
    // construction, and calling the dispatcher would mean handing it a machine and a timestamp
    // it must not use.
    (!session_id.is_empty()).then(|| session_run_id(&agent.kind, session_id))
}

/// Reduce one hook event to the history it is worth keeping.
///
/// *Which* events are kept is the whole of the reduction; how much of one is kept is not.
/// Nothing here is cut. The history's home is a half-pane that wraps and scrolls, an entry
/// costs one row of its own, and the event has already been bounded on the way in by
/// `agent::HOOK_BROADCAST_MAX_BYTES` and the per-field caps its prune applies. The lengths
/// this used to cut to — `WAITING_DETAIL_CHARS`, and a bare `600` for a prompt — are a toast's
/// budget and a sidebar cell's, and they were only ever applied here because the timeline was
/// stored as one blob that grew with every event.
pub fn run_entry(event: &yinz_tile::agent::HookEvent) -> Option<(RunEntryKind, String)> {
    let (kind, text) = match event.hook_event_name.as_str() {
        "UserPromptSubmit" => (
            RunEntryKind::Prompt,
            submitted_prompt(event.prompt.as_deref()?).into_owned(),
        ),
        "PreToolUse"
            if matches!(
                event.tool_name.as_deref(),
                Some("ExitPlanMode" | "update_plan")
            ) =>
        {
            let plan = event
                .tool_input
                .as_ref()
                .and_then(|input| input.get("plan"))
                .map(|plan| match plan {
                    Value::String(plan) => plan.clone(),
                    Value::Array(steps) => steps
                        .iter()
                        .filter_map(|step| step.get("step").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    plan => plan.to_string(),
                })
                .filter(|plan| !plan.is_empty())
                .unwrap_or_else(|| "Plan submitted for approval".to_owned());
            (RunEntryKind::Plan, plan)
        },
        "PreToolUse"
            if matches!(
                event.tool_name.as_deref(),
                Some("AskUserQuestion" | "request_user_input")
            ) =>
        {
            (RunEntryKind::Question, question_text(event)?)
        },
        "Stop" => (RunEntryKind::Result, event.last_assistant_message.clone()?),
        "StopFailure" => (
            RunEntryKind::Result,
            event
                .last_assistant_message
                .clone()
                .unwrap_or_else(|| "Turn failed".to_owned()),
        ),
        _ => return None,
    };
    let text = crate::text::sanitize(&text);
    let text = text.trim();
    (!text.is_empty()).then(|| (kind, text.to_owned()))
}

/// The hook event a prompt entry is recorded from, and the name its identity is spelled with.
///
/// A constant because [`run_event_id`] writes that spelling when the event arrives and
/// [`entry_event_id`] reads it back when a stored entry is compared, and the two have to agree.
const PROMPT_EVENT: &str = "UserPromptSubmit";

/// The identity of the entry a retained hook event is recorded as. The event name keeps a
/// failed tool's completion distinct from the `PreToolUse` carrying the same call id.
///
/// Two roads to one thing. A tool call has the provider's own `tool_use_id`. A
/// `UserPromptSubmit` carries none, so a prompt used to have no identity at all and the ingest
/// path could only compare it against the entry immediately before it. That is the right rule
/// for something a person typed — two identical prompts an hour apart are two prompts — but not
/// for a harness redelivering the same envelope, which can arrive with anything in between.
/// Where the prompt *is* an envelope with an identity of its own,
/// [`crate::envelope::dedupe_id`] supplies one and the whole-run comparison applies to it.
///
/// `recorded` is the text [`run_entry`] returned rather than the event's own `prompt`, and the
/// difference is the whole point. The id is stored beside that text and compared against ids
/// computed from other entries' text, so an id derived from anything else describes a string
/// nothing keeps: it can never be recomputed, audited or backfilled. It is a better reading
/// besides — `crate::text::sanitize` drops the `\r` that leaves a CRLF notification looking cut
/// to the scanner, and expands the tabs that would otherwise make one delivery differ from the
/// next. Stable for exactly as long as `sanitize` and [`crate::envelope`] are.
pub fn run_event_id(event: &yinz_tile::agent::HookEvent, recorded: &str) -> Option<String> {
    if let Some(id) = event
        .tool_use_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map(|id| format!("{}:{id}", event.hook_event_name))
    {
        return Some(id);
    }
    // A prompt and nothing else. A `Stop` whose `last_assistant_message` quotes the
    // notification that woke the agent — which is what an agent describing its own wake-up
    // writes — would otherwise take on that notification's identity and suppress the real one
    // when it arrives.
    (event.hook_event_name == PROMPT_EVENT)
        .then(|| prompt_event_id(recorded))
        .flatten()
}

/// The identity of a prompt recorded as `recorded`, in the one spelling both roads to it use.
fn prompt_event_id(recorded: &str) -> Option<String> {
    crate::envelope::dedupe_id(recorded).map(|id| format!("{PROMPT_EVENT}:{id}"))
}

/// What to compare a stored entry by: the id it was given, or — for a prompt recorded before
/// that field existed — the id its text still yields.
///
/// A run older than the entry-row split has no `agent_run_entry` rows to hydrate from and comes
/// back out of the JSON `payload` blob, where `event_id` is absent and `serde(default)` makes it
/// `None`. Compared as stored, such a run matches nothing ever, so a redelivery lands twice
/// against precisely the oldest runs — the ones with the most history to be confused by it.
/// Recomputing is possible at all only because the id is a function of the recorded text; see
/// [`run_event_id`].
///
/// Only a prompt can be recomputed. A tool call's identity is its `tool_use_id`, which the
/// entry's text never held, and a summary's `None` is a statement rather than a gap: there is no
/// provider event behind one to deduplicate deliveries of. Both stay `None` here.
///
/// Costs nothing until an event that has an id arrives, and `envelope::parse` returns on the
/// first byte of a prompt with no `<` in it, which is nearly every prompt.
pub fn entry_event_id(entry: &RunEntry) -> Option<Cow<'_, str>> {
    match (&entry.event_id, entry.kind) {
        (Some(id), _) => Some(Cow::Borrowed(id.as_str())),
        (None, RunEntryKind::Prompt) => prompt_event_id(&entry.text).map(Cow::Owned),
        (None, _) => None,
    }
}

/// A short archive-row label, preferring the endpoint's title and then the first prompt.
pub fn run_title(run: &AgentRun) -> String {
    let title = run
        .summary
        .as_ref()
        .and_then(|summary| rename_title(summary))
        .map(str::to_owned)
        .or_else(|| first_prompt_title(run))
        .unwrap_or_else(|| "agent run".to_owned());
    let mut title: String = title.chars().take(SUMMARY_TITLE_CHARS).collect();
    if title.is_empty() {
        title = "agent run".to_owned();
    }
    title
}

/// What the run's prompts call it, for an archive row with no summary to use.
///
/// The first prompt that carries any **prose** wins, rather than simply the first prompt: a run
/// woken by a background task and later typed into by a person is about what the person typed,
/// and only a run whose every prompt was machinery is named after the machinery. The fallback
/// is `envelope::title`, which is a sentence rather than the `<task-notification>` the first
/// line of such a prompt literally is.
fn first_prompt_title(run: &AgentRun) -> Option<String> {
    let prompts = || {
        run.entries
            .iter()
            .filter(|entry| entry.kind == RunEntryKind::Prompt)
    };
    prompts()
        .find_map(|entry| {
            let prose = crate::envelope::parse(&entry.text).prose;
            let first = prose.lines().next().unwrap_or_default().trim().to_owned();
            (!first.is_empty()).then_some(first)
        })
        .or_else(|| prompts().find_map(|entry| crate::envelope::title(&entry.text)))
}

/// Put a summary somebody asked for onto the run's timeline, and say whether it went on.
///
/// `false` when the newest summary already on the timeline says the same thing. Two presses
/// of the key against an agent that has not moved produce the same sentence twice, and two
/// identical paragraphs one under the other read as a bug rather than as a history — the
/// second press bought nothing, and the honest way to show that is to leave the timeline
/// alone. It is deliberately only the *newest* that is compared: a run that comes back round
/// to the same state after doing something else really was in that state twice.
///
/// Bumps [`AgentRun::revision`], like every other entry. Callers must set
/// `summarized_revision` *after* this, so the entry it just made is inside the accepted
/// revision and cannot be read back as unsummarised work.
pub fn push_summary_entry(run: &mut AgentRun, text: &str, now_ms: i64) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let newest = run
        .entries
        .iter()
        .rev()
        .find(|entry| entry.kind == RunEntryKind::Summary);
    if newest.is_some_and(|entry| entry.text == text) {
        return false;
    }
    run.revision += 1;
    run.entries.push(RunEntry {
        tool: None,
        revision: run.revision,
        // No provider event behind it, so nothing to deduplicate deliveries of. The
        // comparison above is what stops this kind repeating, and it is a different
        // question: two deliveries of one event against one event happening twice.
        event_id: None,
        kind: RunEntryKind::Summary,
        text: text.to_owned(),
        created_at_ms: Some(now_ms),
    });
    true
}
