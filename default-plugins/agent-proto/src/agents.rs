//! Which panes hold coding agents, and what they are showing.
//!
//! Nothing here calls the Yinz host, so all of it runs under `make test`.
//!
//! There is no registry to keep in step. An agent is a terminal pane from the last
//! session snapshot whose command or title names a known agent, plus any pane id the
//! user registered by hand. A pane that closes stops being an agent because it stops
//! being in the snapshot, which is one less thing that can go stale.
//!
//! `AgentPane::command` is what the pane is running *now*, not what it was launched
//! with — see `tools::pane_running_command`. Detection would find almost nothing
//! otherwise: a pane you opened as a shell and then typed `claude` into was launched
//! with no command at all.

mod runs;
pub use runs::{
    entry_event_id, push_summary_entry, run_entry, run_event_id, run_id, run_id_kind, run_title,
    session_run_id, short_run_id, stored_run_id, AgentRun, AgentRunHeader, RunEntry, RunEntryKind,
    RunOutcome,
};
pub mod rows;
mod summaries;
pub use summaries::{
    local_summary, parse_summary, prompt_title, rename_title, run_summary_request_body,
    stamp_summary, summary_request_body, summary_says_same, summary_toast, waiting_toast,
    worth_summarizing, Summaries, Summary, SUMMARY_CHARS, SUMMARY_LINES,
};
mod history;
pub use history::{
    age_gutter, cut_to_width, entry_blocks, fit_hints, fit_hints_marked, fit_keys, fit_label,
    now_ms, run_history, run_payload_fingerprint, short_age, summary_history, wall_clock,
    HistoryEvent, HistoryLines, ParsedRun, AGE_WIDTH, HISTORY_MIN_COLS, MAX_ENTRY_ROWS, NO_HISTORY,
};
mod presentation;
pub use presentation::{
    kind_cell, list_json, mention_candidates, mention_context, preview, strip_rows,
    tab_project_label, tab_project_labels, tab_projects, tail, worktree_cell, Strip, StripRow,
    HOOK_MARKER,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yinz_tile::prelude::TabId;

pub use yinz_types::agent::control::submitted_prompt;

use crate::location::Location;

#[cfg(test)]
#[path = "unit/agents_attention.rs"]
mod attention_tests;

/// Seconds between activity observations — the same beat as `AGENT_SECONDS` in
/// `main.rs`, restated here so idle arithmetic stays testable off the host.
const POLL_SECONDS: u64 = 5;

/// What each agent's pane showed last poll and for how many polls it has shown it:
/// pane id → (content hash, consecutive unchanged polls).
pub type Activity = BTreeMap<u32, (u64, u32)>;

/// What a pane is doing, as far as Yinz will say. `Held` is a command pane that
/// exited and is waiting to be re-run or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Running,
    Held,
    Exited,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Held => "held",
            AgentStatus::Exited => "exited",
        }
    }
}

/// One terminal pane, cut down to what detection and listing need.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPane {
    pub pane_id: u32,
    pub title: String,
    /// The command with its arguments, as Yinz stringified it. `None` for a shell.
    pub command: Option<String>,
    /// The current folder reported by Yinz.
    pub cwd: Option<String>,
    /// The machine the pane runs on, when it is not this one.
    ///
    /// Part of the pane's identity rather than decoration: `/home/me/work` on
    /// two machines are two different folders, and an agent you are about to
    /// send input to is on one of them specifically.
    #[serde(default)]
    pub host: Option<String>,
    /// The primary checkout whose Bubblewrap environment the pane runs in, when it is in
    /// one. A duplicate or fork of the pane starts in the same environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bubblewrap: Option<String>,
    pub status: AgentStatus,
    /// The stable tab id and tab name this pane sits in. `None` for a session-global
    /// pane, which is in no tab.
    pub tab: Option<(TabId, String)>,
    /// The kind this pane was last seen *running*, kept while the pane lives.
    ///
    /// `command` is the foreground command, so it stops naming the agent the moment
    /// the agent shells out to `git` or runs a tool — and the pane would drop out of
    /// every list until it came back. `agent-service` is the only writer; it latches
    /// this from the command alone and lets it go when the pane closes, is re-run, or
    /// is confirmed back at its prompt. Everyone else reads it off the snapshot, so
    /// no two readers can disagree about what is an agent.
    #[serde(default)]
    pub detected: Option<String>,
    /// The session's own record of the agent here, off the pane manifest. Written by the
    /// spawn path, a plugin, or the agent's hooks; the one answer every reader shares.
    #[serde(default)]
    pub agent: Option<AgentInfo>,
    /// When agent-service last received a live hook for this pane, in Unix milliseconds.
    /// `None` until a hook arrives; pane updates and screen activity do not advance it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hook_at_ms: Option<i64>,
}

impl AgentPane {
    /// The folder this pane's work is *identified* by. See [`checkout_location`].
    pub fn checkout_location(&self) -> Option<Location> {
        checkout_location(
            self.host.as_deref(),
            self.agent
                .as_ref()
                .and_then(|agent| agent.cwd.as_deref())
                .filter(|cwd| !cwd.is_empty())
                .or(self.cwd.as_deref()),
        )
    }
}

pub fn checkout_location(host: Option<&str>, cwd: Option<&str>) -> Option<Location> {
    Some(Location::new(host, cwd?))
}

// The command-line classifier - `KINDS`, `kind_of`, `kind_from_command` - lives in
// `yinz_types::agent` now, because the server reads it too: where a spawned agent pane
// goes is decided before the process has said anything, from the same list `agent-service`
// latches "detected" from. Re-exported so every caller in the plugins keeps one name for it.
pub use yinz_tile::agent::{kind_from_command, kind_of, program, KINDS};

/// The agent kind a pane title names, or `None`.
///
/// A title is free text a program can set to anything, so only the canonical kind
/// counts here, and only as a complete word. Nothing resets a title when the program
/// that set it exits, which is why this is a guess and never a latch.
pub fn kind_from_title(title: &str) -> Option<String> {
    let title = title.to_lowercase();
    title
        .split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_'))
        .find_map(|word| {
            yinz_tile::agent::provider::provider(word)
                .filter(|provider| provider.kind == word)
                .map(|provider| provider.kind.clone())
        })
        .or_else(|| {
            title
                .split(|c: char| !c.is_ascii_alphanumeric())
                .find_map(|word| {
                    KINDS
                        .iter()
                        .find(|kind| **kind == word)
                        .map(|kind| (*kind).to_owned())
                })
        })
}

/// The agent kind a pane runs, from its command first and its title second.
pub fn detect_kind(title: &str, command: Option<&str>) -> Option<String> {
    if let Some(kind) = command.and_then(kind_from_command) {
        return Some(kind);
    }
    // A command that names a shell is the pane telling us it is at a prompt, and
    // that outranks a title the last program left behind. Without this a pane
    // that ran `claude` and returned to its shell stayed an agent for as long as
    // it stayed open, because nothing ever rewrites a pane title.
    if command.is_some_and(is_shell) {
        return None;
    }
    kind_from_title(title)
}

/// Whether a command line is a shell sitting at its prompt.
///
/// The first word only, and the same filename-then-stem treatment `kind_of` gives —
/// `/usr/bin/zsh` and `zsh -l` are both a shell, `zsh script.sh` is too, and nothing
/// here is ever matched as a substring.
pub fn is_shell(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or_default();
    let name = program(first).to_lowercase();
    let stem = name.split('.').next().unwrap_or_default();
    // A login shell is spelled `-zsh` in argv, which is not a dash-segment to
    // strip but a marker to ignore.
    let stem = stem.strip_prefix('-').unwrap_or(stem);
    yinz_tile::agent::SHELLS.contains(&stem)
}

/// Whether a kind the user typed is one we will show to the model.
pub fn valid_kind(kind: &str) -> bool {
    (1..=32).contains(&kind.len())
        && kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The kind and source of one pane, or `None` when the pane is not an agent.
///
/// Precedence: the manual kind, then what the pane is running now, then what it was
/// last seen running. A user who registered a `codex` pane as `reviewer` said
/// something the command name cannot, so that wins; and what a pane is doing right
/// now beats the memory of what it was doing, so a pane that has become a different
/// agent is reported as the one it is.
///
/// The latch comes last and is the only part that survives a pane shelling out. It
/// is `"auto"` like any other detection — `source` is a contract with the model in
/// `list_json`, and a third value there would earn nothing.
pub fn classify(
    pane: &AgentPane,
    manual: &BTreeMap<u32, String>,
) -> Option<(String, &'static str)> {
    if let Some(kind) = manual.get(&pane.pane_id) {
        return Some((kind.clone(), "manual"));
    }
    // The session's record first: a pane spawned as an agent, or one whose agent has reported,
    // is an agent whatever its foreground command says right now.
    if let Some(agent) = pane.agent.as_ref() {
        return Some((agent.kind.clone(), "auto"));
    }
    detect_kind(&pane.title, pane.command.as_deref())
        .map(|kind| (kind, "auto"))
        .or_else(|| pane.detected.clone().map(|kind| (kind, "auto")))
}

/// What one poll did to a pane's activity, for a caller that acts on how long a pane has
/// been still rather than on the label that stillness draws.
///
/// The busy/idle *label* is derived on every render from the poll counter, so nothing
/// needed this until the daemon grew a reason to act when an agent stops working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    /// The pane is showing something new — the agent is working.
    Busy,
    /// The pane has held still for `polls` consecutive polls.
    ///
    /// `1` is the busy→idle edge. It is a weak signal on its own: any pause inside a turn
    /// crosses it, so a caller that wants an agent to have *stopped* rather than merely
    /// paused waits for a higher count.
    Idle { polls: u32 },
}

/// One poll's reading of a pane, qualified by whether it says anything about the agent.
///
/// A pane's picture moves for reasons that have nothing to do with the process behind it:
/// scrolling back shows old output, and a resize rewraps every line. Sampling the text
/// alone cannot tell those apart from work, so the caller — which can see the scroll flag
/// and the pty's own output clock — says which kind of reading this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample<'a> {
    /// The pane is showing its live tail, and this text is worth comparing.
    Text(&'a str),
    /// The reading says nothing: the pane is scrolled back, or the picture moved with
    /// nobody writing to the pty. The stretch of stillness continues either way.
    Unusable,
}

/// The comparison key for a pane's content: its words, in order, with everything else
/// dropped.
///
/// Line breaks are not content. A pane rewrapped to a new width holds exactly the words it
/// held before, so hashing the raw text would call a resize a whole new screen; hashing the
/// word sequence calls it what it is. Tokens with no alphanumeric character go too — box
/// drawing, rules and spinner frames redraw constantly without the session changing.
pub fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for token in tokens(text) {
        token.hash(&mut hasher);
    }
    hasher.finish()
}

/// The words of a pane, in order: whitespace-separated runs that hold at least one
/// alphanumeric character.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace()
        .filter(|token| token.chars().any(char::is_alphanumeric))
}

/// Fold one poll's reading into the activity map, and say what that did.
///
/// The hash is `DefaultHasher` over the pane's word sequence: not stable across runs, never
/// persisted, only ever compared with the previous poll's.
///
/// An [`Sample::Unusable`] reading leaves the stored hash alone and counts the poll as more
/// stillness. That is deliberate: a pane the user scrolled back through must neither look
/// like it started working nor lose the hash an in-flight summary is stamped against.
///
/// The first poll of a pane is [`Observed::Busy`] whatever it is showing — one reading
/// cannot tell "working" from "sitting still", and claiming an edge here would call every
/// agent busy the instant the daemon noticed it.
pub fn observe(activity: &mut Activity, pane_id: u32, sample: Sample) -> Observed {
    let text = match sample {
        Sample::Text(text) => text,
        Sample::Unusable => {
            return match activity.get_mut(&pane_id) {
                Some((_, polls)) => {
                    *polls += 1;
                    Observed::Idle { polls: *polls }
                },
                // Nothing to compare against yet, and this reading cannot start the
                // record - the next usable one will.
                None => Observed::Busy,
            };
        },
    };
    let hash = fingerprint(text);
    match activity.get_mut(&pane_id) {
        Some((last, polls)) if *last == hash => {
            *polls += 1;
            Observed::Idle { polls: *polls }
        },
        _ => {
            activity.insert(pane_id, (hash, 0));
            Observed::Busy
        },
    }
}

/// The content hash the activity map holds for a pane, if it has one.
///
/// What a summary is stamped with, so a stale answer or waiting marker cannot be applied
/// to a screen the pane has since left.
pub fn activity_hash(activity: &Activity, pane_id: u32) -> Option<u64> {
    activity.get(&pane_id).map(|(hash, _)| *hash)
}

// ---------------------------------------------------------------------------
// What an agent says about itself
// ---------------------------------------------------------------------------

pub use yinz_tile::agent::{
    clean_hint, hook_detail, hook_detail_for, question_text, waiting_detail,
    waiting_needs_a_person, HOOK_DETAIL_CHARS, HOOK_SESSION_END, WAITING_DETAIL_CHARS,
};
use yinz_tile::prelude::{AgentInfo, AgentSource, AgentState};

/// What an agent pane says it is doing, in the three states a row can draw.
///
/// The session's `AgentState` minus `Exited`, which a row draws from the pane's own status
/// rather than from a feed: the feed is what the agent *says*, and an exited agent says nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookPhase {
    /// The agent is working.
    Busy,
    /// The agent has stopped and is blocked on the user — a permission prompt, a
    /// question, a form. This is the claim the strip's `!` marker makes.
    Waiting,
    /// The agent finished its turn and is at its prompt.
    Idle,
}

impl HookPhase {
    /// The phase a session-side state draws as, or `None` for an exited agent.
    pub fn from_state(state: AgentState) -> Option<HookPhase> {
        match state {
            AgentState::Busy => Some(HookPhase::Busy),
            AgentState::Waiting => Some(HookPhase::Waiting),
            AgentState::Idle => Some(HookPhase::Idle),
            AgentState::Exited => None,
        }
    }
}

/// What one pane's own hooks have last reported.
///
/// A pane with one of these is not guessed at. The screen-diff heuristic beside it
/// — [`observe`] and [`Activity`] — is inference from a picture, and all it is still
/// trusted for is the idle clock; this is the agent saying so. Where the two disagree,
/// this wins.
///
/// The session carries the record itself (`PaneInfo::agent`), whichever road a report took;
/// a feed is that record read back off the manifest, so nothing here expires on a clock and
/// nothing here can disagree with what every other reader of the manifest sees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookFeed {
    pub phase: HookPhase,
    /// The tool the agent is running, as a few words. `None` outside a tool call.
    #[serde(default)]
    pub detail: Option<String>,
    /// What the agent is blocked on, in one line. `None` outside a wait.
    ///
    /// A separate field from `detail` rather than a reuse of it, because the two are
    /// cleared by different events and read in different places: `detail` names a tool
    /// in a sidebar cell, this names a question in a toast. Sharing one slot would mean
    /// a `PostToolUse` clearing a question it knows nothing about.
    #[serde(default)]
    pub waiting_on: Option<String>,
    /// The agent's own session id, so a session restarted in the same pane can be
    /// told from one that merely went quiet.
    #[serde(default)]
    pub session_id: String,
    /// The folder the *agent* is working in, which is not always the pane's shell
    /// folder — an agent that moved is what this reports, and the shell never heard.
    #[serde(default)]
    pub cwd: Option<String>,
}

impl HookFeed {
    /// The feed a pane's record reads as: one for a record the agent reported itself and is
    /// not exited, none for a spawned-but-silent agent, whose state is the heuristic's to
    /// judge, and none for an exited one.
    pub fn from_agent(agent: &AgentInfo) -> Option<HookFeed> {
        // `Registered` is a person's word for the kind and nothing about the state, so it is
        // not a feed either.
        if agent.source != AgentSource::Reported {
            return None;
        }
        let phase = HookPhase::from_state(agent.state)?;
        Some(HookFeed {
            phase,
            detail: agent.detail.clone(),
            // Only on a wait, which is what the pipe road does for itself: a feed built from
            // an event clears this on every phase that is not one. Without the same rule here
            // a pane read off the manifest could show a question beside a working agent, and
            // `list_json` would hand a model `"state":"idle"` and a `waiting_on` in one row.
            waiting_on: (phase == HookPhase::Waiting)
                .then(|| agent.waiting_on.clone())
                .flatten(),
            session_id: agent.session_id.clone().unwrap_or_default(),
            cwd: agent.cwd.clone(),
        })
    }
}

/// Pane id → what that pane's own hooks last said.
pub type HookFeeds = BTreeMap<u32, HookFeed>;

/// The phase one hook event names, or `None` for an event that says nothing about
/// the agent's state — an unrecognised name, or a notification that is not about
/// the user being waited on.
///
/// `SessionEnd` is `None` here too, and deliberately: it does not move a feed to a
/// state, it ends one. Callers check [`HOOK_SESSION_END`] first.
pub fn hook_phase(kind: &str, event: &crate::wire::HookEvent) -> Option<HookPhase> {
    yinz_tile::agent::hook_state_for(kind, event).and_then(HookPhase::from_state)
}

/// A line already cleaned for a toast, cut again to what one cell of a row can hold.
///
/// Cutting rather than re-cleaning: the text has been through `agent::clean_line` already, so
/// it holds no control characters and no whitespace runs, and doing that work twice would only
/// be a second place for the two to disagree.
fn short_hint(line: &str) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let mut chars = line.chars();
    let mut hint: String = chars.by_ref().take(HOOK_DETAIL_CHARS).collect();
    if chars.next().is_some() {
        hint.pop();
        hint.push('…');
    }
    Some(hint)
}

/// The status column of one agent row.
///
/// A pane whose agent reports for itself is rendered from that report and nothing
/// else: `busy`, `busy · Bash`, `waiting`, `idle`, `idle Nm`. A pane without one
/// falls back to the screen-diff heuristic, which is the plain status until activity
/// says more, then `busy`, `idle`, and `idle Nm`.
///
/// Minutes are the finest label on purpose: a per-second label would make every
/// poll a new row text for nothing. They come from `activity` on both paths, so a
/// hook-fed pane's idle clock reads the same as every other pane's.
pub fn status_cell(
    status: AgentStatus,
    activity: Option<&(u64, u32)>,
    hook: Option<&HookFeed>,
) -> String {
    if status != AgentStatus::Running {
        return status.as_str().to_owned();
    }
    if let Some(feed) = hook {
        return match feed.phase {
            // Same shape as `busy · Bash cargo` below, so the two states read alike.
            // Cut to the sidebar's own budget rather than the toast's: `waiting_on` is
            // written for a box fifty columns wide, and a row that carries the whole of
            // it pushes the tab name off the end of every line beside it.
            HookPhase::Waiting => match feed.waiting_on.as_deref().and_then(short_hint) {
                Some(waiting_on) => format!("waiting · {waiting_on}"),
                None => "waiting".to_owned(),
            },
            HookPhase::Busy => match &feed.detail {
                Some(detail) => format!("busy · {detail}"),
                None => "busy".to_owned(),
            },
            // Zero polls is `busy` on the heuristic path below, because a screen that
            // just moved is a screen doing something. Here the agent has said it
            // stopped, so the same reading is the start of an idle stretch.
            HookPhase::Idle => {
                match activity.map_or(0, |(_, polls)| u64::from(*polls)) * POLL_SECONDS {
                    seconds if seconds < 60 => "idle".to_owned(),
                    seconds => format!("idle {}m", seconds / 60),
                }
            },
        };
    }
    let Some((_, polls)) = activity else {
        return status.as_str().to_owned();
    };
    match u64::from(*polls) * POLL_SECONDS {
        0 => "busy".to_owned(),
        seconds if seconds < 60 => "idle".to_owned(),
        seconds => format!("idle {}m", seconds / 60),
    }
}

/// The record `agent_register` writes for a pane: the kind a person gave it, `idle` as a
/// placeholder for a state nobody has reported, and `Registered` so every reader knows which.
/// The agent's own reports replace it whole, as any record replaces any other.
pub fn registered_agent(kind: &str, cwd: Option<String>) -> AgentInfo {
    AgentInfo {
        kind: kind.to_owned(),
        state: AgentState::Idle,
        source: AgentSource::Registered,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd,
        session_id: None,
        run_id: None,
    }
}

#[cfg(test)]
#[path = "unit/agents.rs"]
mod tests;
