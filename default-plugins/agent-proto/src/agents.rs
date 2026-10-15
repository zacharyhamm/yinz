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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::HostId;
use yinz_tile::prelude::TabId;
use yinz_types::internal_ids::SessionName;

pub use yinz_types::agent::control::submitted_prompt;

use crate::location::Location;
use crate::markdown::{MdLine, MdStyle};

#[cfg(test)]
#[path = "unit/agents_attention.rs"]
mod attention_tests;

/// How many lines of a pane the model sees for each agent in one `agent_list`.
const PREVIEW_LINES: usize = 20;
/// The hard ceiling on one preview, whatever those lines hold.
const PREVIEW_CHARS: usize = 500;
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

/// The checkout an archived run worked in, as the archive table draws it: the folder's leaf
/// name, prefixed `host:` where the folder is on another machine.
///
/// A leaf rather than a path because it is a column in a list that is often half a pane wide,
/// and the leaf is the half that distinguishes two worktrees of one project. The whole path is
/// still a keystroke away - the history half's header spells it in full.
///
/// The host is kept because two machines can each have a `agent`, and a row that says which is
/// the difference between "I have seen this before" and "somebody else has".
pub fn worktree_cell(run: &AgentRunHeader) -> String {
    // Empty is absent, the same reading `AgentPane::checkout_location` gives a blank cwd: a
    // hook that reported no folder must not become a cell that names one, and a blank host
    // would prefix the leaf with a bare `:`.
    fn some(value: Option<&str>) -> Option<&str> {
        value.filter(|value| !value.is_empty())
    }
    let Some(location) = checkout_location(some(run.host.as_deref()), some(run.cwd.as_deref()))
    else {
        return String::new();
    };
    // A path with no final component is a root, which is its own best name.
    let leaf = std::path::Path::new(&location.path)
        .file_name()
        .and_then(|leaf| leaf.to_str())
        .unwrap_or(location.path.as_str());
    match &location.host {
        Some(host) => format!("{host}:{leaf}"),
        None => leaf.to_owned(),
    }
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

// ---------------------------------------------------------------------------
// Idle summaries
// ---------------------------------------------------------------------------

/// How many lines of a pane the model is shown when summarising an idle agent.
///
/// Far more than a `preview`: this has to cover the last exchange, not merely say what is
/// on screen, and it is paid for once per busy→idle edge rather than on every `agent_list`.
pub const SUMMARY_LINES: usize = 120;
/// The hard ceiling on one summary request, whatever those lines hold.
pub const SUMMARY_CHARS: usize = 6000;
/// The longest summary worth storing. A model told to be brief occasionally is not, and a
/// row of the agent list has nowhere to put an essay.
const SUMMARY_TEXT_CHARS: usize = 400;
/// The longest title worth keeping. A pane name has a frame to fit in, and a model asked
/// for four words occasionally writes a sentence.
const SUMMARY_TITLE_CHARS: usize = 40;
/// Below this there is nothing to summarise — a pane that has only drawn a prompt.
const SUMMARY_MIN_CHARS: usize = 40;

const SUMMARY_SYSTEM_PROMPT: &str = "You are watching one terminal pane in which a coding \
agent is running. You will be shown the tail of that pane. Reply with a single JSON object \
and nothing else - no prose, no markdown, no code fences. The object has exactly three \
keys: \"summary\", one or two plain sentences describing the current state of the session \
and what was last done; \"awaiting\", true only if the agent has stopped and is waiting for \
the user to answer a question or confirm an action it proposed, false if it merely finished \
its work; and \"title\", at most four words naming the task the agent is working on, with \
no trailing punctuation, to be used as the name of the pane. A completed task is not \
awaiting. A permission prompt, a yes/no question, or a menu of choices is awaiting.";

/// The prompt for a run the agent described itself.
///
/// Two differences from [`SUMMARY_SYSTEM_PROMPT`], and both are the point of the hook
/// feed. The input is the agent's *own account of the session* — the events it reported
/// as it worked — rather than a photograph of a terminal, so the model is reading prose
/// written for a person rather than inferring intent from a redraw. And `awaiting` is not
/// asked for at all: the feed already knows whether the agent is blocked on the user, and
/// a second opinion would only be a disagreement to resolve — see `finish_summary_with`,
/// which overwrites it either way.
///
/// It asks about the session rather than the last turn of one because that is what both
/// remaining callers send: `run_delta` carries the whole run, prompts included, and leaves
/// the run's own prior summary out on purpose.
const RUN_SUMMARY_SYSTEM_PROMPT: &str = "You will be shown a coding agent's session: what \
the user asked for, if anything, followed by the history of what the agent did. \
Reply with a single JSON object and nothing else - no prose, no markdown, no code \
fences. The object has exactly two keys: \"summary\", one or two plain sentences \
describing what the agent did and where the session now stands; and \"title\", at most \
four words naming the task, with no trailing punctuation, to be used as the name of the \
pane. Write about the work, not about the history: do not begin with \"the agent\" or \
quote the events back.";

/// The body of one completion asking what a run has been, or was.
///
/// The counterpart of [`summary_request_body`] for a pane that reports for itself.
/// Carries the same header — kind, folder, branch — so the two paths give the model the
/// same context, and adds the user's request where one was seen: a description of an
/// answer written without sight of the question is half a description.
///
/// `prompt` is carried beside the history rather than left to be found inside it, because
/// `message` is tailed from the newest end: the question a long run opened with is exactly
/// what the tail drops. So the caller must pass the prompt the tail is most likely to lose.
pub fn run_summary_request_body(
    model: &str,
    kind: &str,
    cwd: Option<&str>,
    branch: Option<&str>,
    prompt: Option<&str>,
    message: &str,
) -> Vec<u8> {
    let mut header = format!("Agent: {kind}.");
    if let Some(cwd) = cwd {
        header.push_str(&format!(" Folder: {cwd}."));
    }
    if let Some(branch) = branch {
        header.push_str(&format!(" Branch: {branch}."));
    }
    let mut user = header;
    if let Some(prompt) = prompt.map(str::trim).filter(|p| !p.is_empty()) {
        user.push_str(&format!("\n\n--- the user asked ---\n{prompt}"));
    }
    user.push_str(&format!("\n\n--- run history ---\n{message}"));
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": RUN_SUMMARY_SYSTEM_PROMPT },
            { "role": "user", "content": user },
        ],
    });
    serde_json::to_vec(&body).expect("serializing a summary request cannot fail")
}

/// A summary made from the agent's own words, with no model asked.
///
/// The fallback for a pane whose agent reports for itself but whose session has no
/// endpoint configured — no API key, or `summaries "false"`. Before the hook feed there
/// was nothing to fall back *to*: a screen tail is not a summary of anything, and
/// showing one raw would have been worse than showing nothing. A final message is
/// already prose written for a person, so the honest thing is to show its opening and
/// say no more.
///
/// The title is deliberately empty. Naming a pane in four words is a judgement, and
/// this path has nobody to make it; [`rename_title`] reads an empty title as "leave the
/// pane's name alone", which is the right answer rather than a missing one.
///
/// `None` when what is left is too short to be worth drawing — the same floor
/// [`worth_summarizing`] applies to a screen.
pub fn local_summary(message: &str, hash: u64) -> Option<Summary> {
    let text = plain_prose(message);
    (text.chars().count() >= SUMMARY_MIN_CHARS).then(|| Summary {
        text,
        title: String::new(),
        awaiting: false,
        hash,
        // Stamped by whoever stores it: this has no clock, and a summary nobody kept has no
        // age worth carrying.
        created_at_ms: None,
    })
}

/// An agent's message with its markdown taken off, cut to what a summary holds.
///
/// Not a markdown renderer and not trying to be: what this removes is the decoration
/// that reads as noise in a one-line box — fenced blocks, heading and quote and bullet
/// markers, emphasis runs — and what it keeps is the sentences. A fenced block is
/// dropped whole rather than unwrapped, because a summary that opens with three lines
/// of someone else's Rust has said nothing.
fn plain_prose(message: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in crate::text::sanitize(message).lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Leading structure, in the order it is written: quote markers, then heading
        // hashes, then a list bullet.
        let mut body = trimmed.trim_start_matches(['>', ' ']);
        body = body.trim_start_matches('#').trim_start();
        if let Some(rest) = body
            .strip_prefix("- ")
            .or_else(|| body.strip_prefix("* "))
            .or_else(|| body.strip_prefix("+ "))
        {
            body = rest;
        }
        let body = body.replace("**", "").replace("`", "");
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(body);
        if out.chars().count() >= SUMMARY_TEXT_CHARS {
            break;
        }
    }
    if out.chars().count() > SUMMARY_TEXT_CHARS {
        out = out.chars().take(SUMMARY_TEXT_CHARS - 1).collect();
        out.push('…');
    }
    out
}

/// What the model said about an idle agent's pane.
/// What the model said about an idle agent's pane.
///
/// Held in memory by `agent-service` and shipped in its snapshot. `awaiting` is the strip's
/// `!` marker for as long as `hash` is still the pane's screen, `text` is the inset agent draws
/// over its transcript, and `title` is what the daemon renames the pane to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub text: String,
    /// A few words naming the task, for the pane's name.
    ///
    /// Empty when the model did not answer with one — a summary is worth keeping without a
    /// title, and the rename is simply skipped. See [`rename_title`].
    #[serde(default)]
    pub title: String,
    /// Whether the agent is blocked on the user rather than merely finished.
    #[serde(default)]
    pub awaiting: bool,
    /// The activity hash the pane held when this was made.
    ///
    /// Used for exact freshness checks on replies and waiting markers. Repeat summary
    /// suppression uses a line-level baseline in the daemon instead. Not stable across
    /// runs, and never persisted — the same contract as the hash in [`Activity`], which
    /// is where it comes from.
    #[serde(default)]
    pub hash: u64,
    /// When this summary was made, epoch milliseconds. `None` for a summary made before the
    /// field existed, and for one a parser built before anybody stamped it - the daemon sets
    /// it as it stores the summary, which is the only place with a clock.
    #[serde(default)]
    pub created_at_ms: Option<i64>,
}

/// Whether two summaries say the same thing: the same words, the same title and the same
/// claim about who the turn is waiting on.
///
/// Deliberately not `PartialEq`. The hash is a fact about the pane's screen at the moment the
/// summary was made, and the stamp is a fact about when it was made; neither is part of what
/// the summary *says*, and a re-summary that changed no word is the same answer told twice.
pub fn summary_says_same(a: &Summary, b: &Summary) -> bool {
    a.text == b.text && a.title == b.title && a.awaiting == b.awaiting
}

/// Stamp `summary` with the time it was made, keeping `previous`'s stamp where the two say
/// the same thing.
///
/// The daemon re-summarises a settled pane on its own schedule and often gets the same answer
/// back. Stamping that unconditionally would make the panel show an age counting from the last
/// poll while the stored row - which [`rows::summary_diff_mutations`] leaves alone, because
/// nothing it holds changed - keeps counting from the first, so the same summary would read as
/// minutes old until a restart and hours old after one. The age belongs to what the summary
/// says, not to the poll that repeated it.
///
/// Call it after any fixup to `awaiting`: it is part of what a summary says, so a stamp taken
/// before the fixup compares the wrong summary.
pub fn stamp_summary(summary: &mut Summary, previous: Option<&Summary>, now_ms: i64) {
    summary.created_at_ms = previous
        .filter(|previous| summary_says_same(previous, summary))
        .and_then(|previous| previous.created_at_ms)
        .or(Some(now_ms));
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

/// Pane id → what the model last said about it.
pub type Summaries = BTreeMap<u32, Summary>;

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

/// The `agent_latch` and `agent_summary` rows in `shared:yinz`: what `agent_register` set by
/// hand, and what the daemon last said about each idle pane, so a agent reload keeps both.
/// agent is the only writer; see `store::agent_latch_def` and `store::agent_summary_def`.
pub mod rows {
    use super::{
        summary_says_same, AgentRun, AgentRunHeader, HostId, RunEntry, RunEntryKind, Summaries,
        Summary,
    };
    use crate::store::Backend;
    use std::collections::{BTreeMap, BTreeSet};
    use yinz_tile::prelude::{
        DataRows, DataValue, FetchRequest, Mutation, Pred, PutBuilder, SortDir, MAX_FETCH_ROWS,
    };

    use crate::store::owned_by;
    pub use crate::store::{owned_rows_request, Owner};

    pub const LATCH_ENTITY: &str = "agent_latch";
    pub const SUMMARY_ENTITY: &str = "agent_summary";
    pub const RUN_ENTITY: &str = "agent_run";
    pub const ENTRY_ENTITY: &str = "agent_run_entry";

    /// How much of a run's timeline is handed to the panel at once.
    ///
    /// The one bound left on a run's size, and it is at the delivery seam rather than at the
    /// record: an entry costs a row, but the panel is sent its run as a single JSON string
    /// copied into the interpreter, and that is the only place the length of a long-lived run is
    /// paid for. The newest end is the one that is kept, because that is the end the history
    /// half is anchored to and scrolls back from.
    ///
    /// `ponytail:` page older entries into the panel when it scrolls past the oldest of these.
    /// Nothing does that yet, and a run of five hundred prompts, plans and questions is a very
    /// long day's work.
    pub const RUN_DETAIL_ENTRIES: u32 = 500;

    /// The other half of that bound, and the half that actually holds.
    ///
    /// A row count bounds nothing on its own now that an entry keeps the whole of what the
    /// agent said: five hundred plans is megabytes, and the only ceiling left under it is the
    /// store's own `MAX_FETCH_BYTES`. What the panel is sent has to fit in its memory as one
    /// JSON string, so the entries are taken from the newest end until this much text has been
    /// gathered. Generous next to any real run — a day of prompts and plans is tens of
    /// kilobytes — and small next to the two megabytes a fetch may hand back.
    pub const RUN_DETAIL_BYTES: usize = 256 * 1024;

    /// This server's open runs, for the restore at load.
    ///
    /// Scoped by [`owned_or_unclaimed`] rather than [`owned_by`]: a run recorded before the
    /// store could be shared between machines names no machine, and it is this one's. Read
    /// strictly, the first launch after the upgrade would restore none of its own open runs
    /// and could not close them either — see [`owned_or_unclaimed`].
    pub fn active_runs_request(owner: &Owner) -> FetchRequest {
        FetchRequest::entity(RUN_ENTITY)
            .filter(Pred::eq("active", true))
            .filter(owned_or_unclaimed(owner))
            .select(["payload"])
    }

    /// One run by its id, whatever session it belongs to and whether or not it is still open.
    ///
    /// The lookup a session performs for a pane that arrived from somewhere else: the row is
    /// under another session's name and has already been closed by the session that lost the
    /// pane, so neither of [`active_runs_request`]'s filters would find it.
    pub fn run_by_id_request(run_id: &str) -> FetchRequest {
        FetchRequest::entity(RUN_ENTITY)
            .filter(Pred::eq("uid", run_id))
            .select(["payload"])
            .limit(1)
    }

    /// One entry's row key. Run-scoped and revision-keyed, so re-writing an entry the store
    /// already holds upserts it rather than making a second copy of it — the same shape, and
    /// for the same reason, as agent's `message_uid`.
    fn entry_uid(run_id: &str, revision: u64) -> String {
        format!("{run_id}#{revision}")
    }

    /// One entry on a run's timeline.
    ///
    /// `host_id` is attribution rather than key — the uid is `{run_id}#{revision}`, and a run
    /// id does not collide between machines: the ordinary shape carries the agent's own session
    /// uuid, and the fallback for a hook that supplies none carries the machine. It is here
    /// so that a search across a shared history can say where a hit happened.
    pub fn entry_mutation(run_id: &str, host_id: &HostId, entry: &RunEntry) -> Mutation {
        let mut put = Mutation::put(ENTRY_ENTITY)
            .uid(entry_uid(run_id, entry.revision))
            .set("run_id", run_id)
            .set("host_id", host_id)
            .set("revision", entry.revision as i64)
            .set("kind", entry.kind.label())
            .set("text", entry.text.clone());
        if let Some(tool) = &entry.tool {
            if let Ok(value) = serde_json::to_string(tool) {
                put = put.set("tool", value);
            }
        }
        if let Some(event_id) = entry.event_id.as_ref() {
            put = put.set("event_id", event_id.clone());
        }
        if let Some(created_at_ms) = entry.created_at_ms {
            put = put.set("created_at_ms", created_at_ms);
        }
        put.build()
    }

    /// Every entry of these runs, a page at a time.
    ///
    /// Deliberately unsorted. `after_id` is the only paging that does not have to re-scan what
    /// it has already read, and the store refuses it beside a sort — so the fetch takes the
    /// default `id` ordering and [`entries_from_rows`] sorts by revision in the guest, exactly
    /// as agent's transcript loader does with `seq`. Row order agrees with revision order
    /// anyway: a row is written when its entry is made.
    pub fn entries_request(run_ids: impl IntoIterator<Item = String>) -> FetchRequest {
        FetchRequest::entity(ENTRY_ENTITY)
            .filter(Pred::is_in("run_id", run_ids))
            .limit(MAX_FETCH_ROWS)
    }

    /// The newest [`RUN_DETAIL_ENTRIES`] entries of one run, for the panel.
    ///
    /// Sorted, which rules out a cursor — but one bounded fetch has nothing to page.
    pub fn newest_entries_request(run_id: &str) -> FetchRequest {
        FetchRequest::entity(ENTRY_ENTITY)
            .filter(Pred::eq("run_id", run_id))
            .sort_by("revision", SortDir::Desc)
            .limit(RUN_DETAIL_ENTRIES)
    }

    fn int_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<i64> {
        match rows.get(row, column) {
            Some(DataValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    /// Every entry in these rows, grouped by run and ordered oldest first.
    ///
    /// A row whose `kind` this version does not know is dropped rather than guessed at: a
    /// newer agent writing a kind this one cannot draw is a thing to ignore, not to crash on.
    pub fn entries_from_rows(rows: &DataRows) -> BTreeMap<String, Vec<RunEntry>> {
        let mut grouped: BTreeMap<String, Vec<RunEntry>> = BTreeMap::new();
        for row in &rows.rows {
            let run_id = text_at(rows, row, "run_id");
            let (Some(revision), Some(kind)) = (
                int_at(rows, row, "revision").and_then(|value| u64::try_from(value).ok()),
                RunEntryKind::from_label(&text_at(rows, row, "kind")),
            ) else {
                continue;
            };
            let event_id = match rows.get(row, "event_id") {
                Some(DataValue::Text(id)) if !id.is_empty() => Some(id.clone()),
                _ => None,
            };
            grouped.entry(run_id).or_default().push(RunEntry {
                tool: match rows.get(row, "tool") {
                    Some(DataValue::Text(value)) => serde_json::from_str(value).ok(),
                    _ => None,
                },
                revision,
                event_id,
                kind,
                text: text_at(rows, row, "text"),
                created_at_ms: int_at(rows, row, "created_at_ms"),
            });
        }
        for entries in grouped.values_mut() {
            entries.sort_by_key(|entry| entry.revision);
        }
        grouped
    }

    /// Fill in these runs' timelines from their entry rows.
    ///
    /// A run's `entries` are only replaced when the store actually holds rows for it. That is
    /// what makes the split backward compatible without a migration: a run recorded before
    /// entries had rows of their own still carries them inside its `payload`, deserializes
    /// with them, and is left alone here.
    ///
    /// Returns the ids of the runs that *were* hydrated, which is exactly the question a
    /// caller has to answer before deciding what still needs writing: a run left holding its
    /// payload's entries has none of them stored as rows yet, and its next persist has to
    /// write all of them — the payload it is written with no longer carries a timeline.
    pub fn hydrate_entries(backend: &impl Backend, runs: &mut [AgentRun]) -> BTreeSet<String> {
        if runs.is_empty() {
            return BTreeSet::new();
        }
        let mut request = entries_request(runs.iter().map(|run| run.id.clone()));
        let mut grouped: BTreeMap<String, Vec<RunEntry>> = BTreeMap::new();
        loop {
            let Ok(rows) = backend.fetch(&request) else {
                return BTreeSet::new();
            };
            for (run_id, entries) in entries_from_rows(&rows) {
                grouped.entry(run_id).or_default().extend(entries);
            }
            // A run's timeline can outgrow one page, so keep going while the store says there
            // is more. `next_cursor` is only handed back under the default ordering, which is
            // why `entries_request` does not sort.
            match rows.truncated.then_some(rows.next_cursor).flatten() {
                Some(cursor) => request = request.after_id(cursor),
                None => break,
            }
        }
        let mut hydrated = BTreeSet::new();
        for run in runs {
            if let Some(mut entries) = grouped.remove(&run.id) {
                entries.sort_by_key(|entry| entry.revision);
                run.entries = entries;
                hydrated.insert(run.id.clone());
            }
        }
        hydrated
    }

    /// Fill in one run's timeline with its newest entries, for the panel. See
    /// [`RUN_DETAIL_ENTRIES`] and [`RUN_DETAIL_BYTES`].
    ///
    /// The budget is spent from the newest end, which is the end the history half is anchored
    /// to, and the newest entry is kept whatever it costs: a run whose last plan is on its own
    /// over the budget should show that plan, not an empty history.
    ///
    /// What is left out is counted in [`AgentRun::omitted_entries`], so the half can say its
    /// oldest row is not where the run began.
    pub fn hydrate_newest_entries(backend: &impl Backend, run: &mut AgentRun) {
        run.omitted_entries = 0;
        let Ok(rows) = backend.fetch(&newest_entries_request(&run.id)) else {
            return;
        };
        let Some(mut entries) = entries_from_rows(&rows).remove(&run.id) else {
            return;
        };
        let mut spent = 0usize;
        let mut kept = 0;
        for entry in entries.iter().rev() {
            if kept > 0 && spent.saturating_add(entry.text.len()) > RUN_DETAIL_BYTES {
                break;
            }
            spent = spent.saturating_add(entry.text.len());
            kept += 1;
        }
        let dropped = entries.len() - kept;
        entries.drain(..dropped);
        // A truncated fetch stopped at the row limit with older rows still behind it. It does
        // not say how many, so they count as one: enough for "there is more".
        run.omitted_entries = dropped + usize::from(rows.truncated);
        run.entries = entries;
    }

    fn pane_id_at(rows: &DataRows, row: &[DataValue]) -> Option<u32> {
        match rows.get(row, "pane_id") {
            Some(DataValue::Int(id)) => u32::try_from(*id).ok(),
            _ => None,
        }
    }

    fn text_at(rows: &DataRows, row: &[DataValue], column: &str) -> String {
        match rows.get(row, column) {
            Some(DataValue::Text(text)) => text.clone(),
            _ => String::new(),
        }
    }

    /// A pane's row key: the machine, the session on it, and the pane.
    ///
    /// Session and host identity keep local pane ids distinct across servers and installations.
    fn pane_uid(owner: &Owner, pane_id: u32) -> String {
        format!("{}:{}:{}", owner.host_id, owner.yinz_session_name, pane_id)
    }

    /// This server's rows, and the rows that name no server at all — which are also this
    /// server's, since there was only one machine when they were written.
    ///
    /// The store migrates an entity unasked, and adding `host_id` to `agent_run` was an
    /// `ALTER TABLE ADD COLUMN ... NOT NULL DEFAULT ''`: every run already recorded came out of
    /// the migration with an empty host. Demanding this machine's id would therefore match none
    /// of a daemon's own runs on the first launch after the upgrade. They would never be
    /// restored, never adopted, and — since the guarded close in [`run_owned_mutation`] names
    /// the host too — never closed, sitting `active` forever while a fresh run started beside
    /// each of them for the same agent.
    ///
    /// The session half stays strict, so this is no weaker than the guard was before the host
    /// existed. It is weaker than the current guard in exactly one case: two machines sharing a
    /// store, both running a session of the same name, over a run neither has claimed yet. That
    /// needs a database written before the host column existed, and the first ordinary write of
    /// such a run claims it — see [`claim_unowned_runs`].
    fn owned_or_unclaimed(owner: &Owner) -> Pred {
        crate::store::host_or_legacy(&owner.host_id).and(Pred::eq(
            "yinz_session_name",
            owner.yinz_session_name.clone(),
        ))
    }

    /// Stamp this machine onto restored runs that name none.
    ///
    /// Not a write of its own: [`run_values`] takes `host_id` straight off the run, so the claim
    /// lands with the run's next ordinary persist rather than costing a write per run at load.
    /// A run that is never written again keeps its empty host, which [`owned_or_unclaimed`] goes
    /// on reading — the accommodation is permanent on the read side and one-shot on the write
    /// side, which is the right way round.
    ///
    /// It also puts the machine back on the run's *entries*: `entry_mutation` is given
    /// `run.host_id`, so an unclaimed run would go on recording entries that say where they
    /// happened only by omission.
    pub fn claim_unowned_runs(runs: &mut [AgentRun], owner: &Owner) {
        for run in runs.iter_mut().filter(|run| run.host_id.is_empty()) {
            run.host_id = owner.host_id.clone();
        }
    }

    /// The two columns that say who a row belongs to, for a `Put` that writes one.
    ///
    /// Always both, and in one place, so that a row cannot be written with a uid naming a host
    /// and a column that disagrees.
    fn owner_values(put: PutBuilder, owner: &Owner) -> PutBuilder {
        put.set("host_id", owner.host_id.clone())
            .set("yinz_session_name", owner.yinz_session_name.clone())
    }

    /// Rows left behind by earlier versions of these entities: the one that keyed on the pane id
    /// alone, and the one that keyed on the session without the machine.
    ///
    /// Neither can be reached by a predicate that a current row matches, and their uids collide
    /// with nothing, so they would simply accumulate. Dropped once, on the first write after the
    /// migration.
    ///
    /// An empty `host_id` is the marker for the second kind, since that is what the migration
    /// fills the new column with. It cannot be told apart from a row this machine wrote before
    /// upgrading, which is the point: the pane ids in it belong to a server that is gone either
    /// way, and a stale latch is worth less than a wrong one.
    pub fn stale_unowned_delete_mutations() -> Vec<Mutation> {
        [LATCH_ENTITY, SUMMARY_ENTITY]
            .into_iter()
            .map(|entity| Mutation::delete(entity, Pred::eq("host_id", "")))
            .collect()
    }

    pub fn latch_mutation(owner: &Owner, pane_id: u32, kind: &str) -> Mutation {
        owner_values(
            Mutation::put(LATCH_ENTITY)
                .uid(pane_uid(owner, pane_id))
                .set("pane_id", pane_id),
            owner,
        )
        .set("kind", kind)
        .build()
    }

    pub fn latch_delete_mutation(
        owner: &Owner,
        pane_ids: impl IntoIterator<Item = u32>,
    ) -> Mutation {
        Mutation::delete(
            LATCH_ENTITY,
            Pred::is_in("pane_id", pane_ids).and(owned_by(owner)),
        )
    }

    pub fn latches_from_rows(rows: &DataRows) -> BTreeMap<u32, String> {
        rows.rows
            .iter()
            .filter_map(|row| {
                let kind = text_at(rows, row, "kind");
                if kind.is_empty() {
                    return None;
                }
                Some((pane_id_at(rows, row)?, kind))
            })
            .collect()
    }

    pub fn summary_mutation(owner: &Owner, pane_id: u32, summary: &Summary) -> Mutation {
        let mut put = owner_values(
            Mutation::put(SUMMARY_ENTITY)
                .uid(pane_uid(owner, pane_id))
                .set("pane_id", pane_id),
            owner,
        )
        .set("text", summary.text.clone())
        .set("title", summary.title.clone())
        .set("awaiting", summary.awaiting);
        if let Some(created_at_ms) = summary.created_at_ms {
            put = put.set("created_at_ms", created_at_ms);
        }
        put.build()
    }

    pub fn summary_delete_mutation(
        owner: &Owner,
        pane_ids: impl IntoIterator<Item = u32>,
    ) -> Mutation {
        Mutation::delete(
            SUMMARY_ENTITY,
            Pred::is_in("pane_id", pane_ids).and(owned_by(owner)),
        )
    }

    /// The summaries these rows describe, with no activity hash: the hash is a fact about a
    /// run, and a restored summary is refreshed by the daemon on its own schedule.
    pub fn summaries_from_rows(rows: &DataRows) -> Summaries {
        rows.rows
            .iter()
            .filter_map(|row| {
                Some((
                    pane_id_at(rows, row)?,
                    Summary {
                        text: text_at(rows, row, "text"),
                        title: text_at(rows, row, "title"),
                        awaiting: matches!(rows.get(row, "awaiting"), Some(DataValue::Bool(true))),
                        hash: 0,
                        created_at_ms: int_at(rows, row, "created_at_ms"),
                    },
                ))
            })
            .collect()
    }

    /// The writes that turn `before` into `after`, ignoring the hash and the time it was
    /// made: an unchanged summary produces no write, so a poll that changes nothing is
    /// silent - and a summary the daemon re-stamped without changing a word of is unchanged.
    pub fn summary_diff_mutations(
        owner: &Owner,
        before: &Summaries,
        after: &Summaries,
    ) -> Vec<Mutation> {
        let mut mutations: Vec<Mutation> = after
            .iter()
            .filter(|(pane_id, summary)| {
                !before
                    .get(pane_id)
                    .is_some_and(|b| summary_says_same(b, summary))
            })
            .map(|(pane_id, summary)| summary_mutation(owner, *pane_id, summary))
            .collect();
        let gone: Vec<u32> = before
            .keys()
            .filter(|pane_id| !after.contains_key(pane_id))
            .copied()
            .collect();
        if !gone.is_empty() {
            mutations.push(summary_delete_mutation(owner, gone));
        }
        mutations
    }

    /// The columns a run is stored as. One list, so a `Put` and a guarded `Update` cannot
    /// disagree about what a row holds.
    fn run_values(run: &AgentRun) -> Vec<(&'static str, DataValue)> {
        vec![
            ("pane_id", DataValue::from(run.pane_id)),
            ("session_id", DataValue::from(run.session_id.clone())),
            (
                "yinz_session_name",
                DataValue::from(run.yinz_session_name.clone()),
            ),
            ("host_id", DataValue::from(run.host_id.clone())),
            ("kind", DataValue::from(run.kind.clone())),
            ("active", DataValue::from(run.ended_at_ms.is_none())),
            ("started_at_ms", DataValue::from(run.started_at_ms)),
            (
                "header",
                DataValue::from(
                    serde_json::to_string(&run.header()).expect("AgentRunHeader serializes"),
                ),
            ),
            ("payload", DataValue::from(stored_payload(run))),
        ]
    }

    /// The run as its `payload` column holds it: everything except the timeline, which lives
    /// in [`ENTRY_ENTITY`] rows.
    ///
    /// Written from a cleared copy rather than by making `entries` a skipped serde field,
    /// because the same struct is serialized again to reach the panel — where the timeline is
    /// the whole point of sending it.
    ///
    /// Field by field, and deliberately not `..run.clone()`: functional-update syntax builds
    /// the whole value before overriding, so that spelling deep-clones every entry only to
    /// drop it — on every hook, for a run that has been going all day. A literal without `..`
    /// also cannot silently omit a field, since the compiler refuses one that is not listed.
    fn stored_payload(run: &AgentRun) -> String {
        let stored = AgentRun {
            id: run.id.clone(),
            pane_id: run.pane_id,
            yinz_session_name: run.yinz_session_name.clone(),
            host_id: run.host_id.clone(),
            origin_run_id: run.origin_run_id.clone(),
            previous_run_id: run.previous_run_id.clone(),
            session_id: run.session_id.clone(),
            kind: run.kind.clone(),
            cwd: run.cwd.clone(),
            host: run.host.clone(),
            bubblewrap: run.bubblewrap.clone(),
            started_at_ms: run.started_at_ms,
            ended_at_ms: run.ended_at_ms,
            outcome: run.outcome,
            revision: run.revision,
            summarized_revision: run.summarized_revision,
            summary: run.summary.clone(),
            entries: Vec::new(),
            // A record is every entry the run has; only a copy sent to the panel leaves any out.
            omitted_entries: 0,
        };
        serde_json::to_string(&stored).expect("AgentRun serializes")
    }

    pub fn run_mutation(run: &AgentRun) -> Mutation {
        let mut put = Mutation::put(RUN_ENTITY).uid(run.id.clone());
        for (attr, value) in run_values(run) {
            put = put.set(attr, value);
        }
        put.build()
    }

    /// A run and every entry it holds, as one batch.
    ///
    /// The whole-run write: [`run_mutation`] alone stores only the metadata, since the
    /// timeline has rows of its own. agent-service does not use this — it knows which entries
    /// the store already has and writes only the new ones — but anything holding a complete
    /// run and no such bookkeeping wants all of it written.
    pub fn run_mutations(run: &AgentRun) -> Vec<Mutation> {
        let mut mutations = Vec::with_capacity(run.entries.len() + 1);
        mutations.push(run_mutation(run));
        mutations.extend(
            run.entries
                .iter()
                .map(|entry| entry_mutation(&run.id, &run.host_id, entry)),
        );
        mutations
    }

    /// Write a run *only while it is still ours*.
    ///
    /// Every session sharing a store can hold opinions about one run id — which is exactly what
    /// a warp arranges. The session that lost the pane closes the run when it notices the pane
    /// gone; the session that gained it has by then reopened the same row under its own name.
    /// An unconditional `Put` from the loser would overwrite the winner's copy with a stale,
    /// closed one, and the entries the agent has produced since would be gone.
    ///
    /// The predicate settles that inside the transaction rather than by hoping about order: a
    /// row already claimed by another session matches nothing, and the write is simply dropped.
    /// An adoption is deliberately *not* guarded — claiming the row is precisely the write that
    /// changes the owner, so it must be a `Put`.
    ///
    /// The owner is the machine and the session, not the session alone. On a store shared
    /// between machines a session called `main` on one of them would otherwise pass the guard
    /// held by `main` on another and close a run it has nothing to do with — worse than no
    /// guard, because it looks like one.
    ///
    /// A run that names no machine passes it too ([`owned_or_unclaimed`]): it was recorded
    /// before the column existed, and a guard this session can never satisfy would leave it
    /// open for good. The write itself carries the run's host, so the row is claimed by the
    /// same act that closes it.
    pub fn run_owned_mutation(run: &AgentRun, owner: &Owner) -> Mutation {
        let mut update = Mutation::update(
            RUN_ENTITY,
            Pred::eq("uid", run.id.clone()).and(owned_or_unclaimed(owner)),
        );
        for (attr, value) in run_values(run) {
            update = update.set(attr, value);
        }
        update.build()
    }

    pub fn runs_from_rows(rows: &DataRows) -> Vec<AgentRun> {
        rows.rows
            .iter()
            .filter_map(|row| match rows.get(row, "payload") {
                Some(DataValue::Text(value)) => serde_json::from_str(value).ok(),
                _ => None,
            })
            .collect()
    }

    pub fn run_headers_from_rows(rows: &DataRows) -> Vec<AgentRunHeader> {
        rows.rows
            .iter()
            .filter_map(|row| header_at(rows, row))
            .collect()
    }

    fn header_at(rows: &DataRows, row: &[DataValue]) -> Option<AgentRunHeader> {
        let Some(DataValue::Text(value)) = rows.get(row, "header") else {
            return None;
        };
        let mut header: AgentRunHeader = serde_json::from_str(value).ok()?;
        // Older stored headers omit ownership; the row columns are authoritative when fetched.
        if let Some(DataValue::Text(session)) = rows.get(row, "yinz_session_name") {
            header.yinz_session_name = session.clone().into();
        }
        if let Some(DataValue::Text(host_id)) = rows.get(row, "host_id") {
            header.host_id = host_id.clone().into();
        }
        Some(header)
    }
}

/// The name to give the pane this summary describes, or `None` when there is none worth
/// giving. One rule, so the daemon and its tests cannot disagree about what an empty title
/// means.
pub fn rename_title(summary: &Summary) -> Option<&str> {
    let title = summary.title.trim();
    (!title.is_empty()).then_some(title)
}

/// The name to give the pane of an agent that has just been asked for something.
///
/// The other half of [`rename_title`], and the earlier one: a summary title arrives when the
/// turn ends, and this is what the pane carries while it runs. `yinz_tile::agent::clean_line`
/// does the whole job — a prompt is several lines of a person's typing, and it collapses the
/// whitespace runs, drops the control characters a paste carries, and cuts to
/// [`WAITING_DETAIL_CHARS`] with a `…`. Cutting there rather than to the model title's tighter
/// budget: nothing else has read this prompt yet, so the pane's name is all the user gets of
/// what they asked for, and the tab bar shortens what will not fit.
///
/// `None` for a prompt with nothing in it, which is the same answer [`rename_title`] gives an
/// empty title: there is no name in it to take.
pub fn prompt_title(prompt: &str) -> Option<String> {
    // A prompt a harness wrote is named by what it says rather than by its markup: without
    // this a pane woken by a background task spends the whole turn titled
    // `<task-notification> <task-id>a10fe836…`. `clean_line` still does the collapsing and the
    // cut either way, so a title shortens from a sentence rather than from a temp path.
    crate::envelope::title(prompt)
        .as_deref()
        .and_then(yinz_tile::agent::clean_line)
        .or_else(|| yinz_tile::agent::clean_line(prompt))
}

/// One summary as the toast the daemon shows the moment it is made.
///
/// The inset over agent's transcript only draws while the user is standing in the agents
/// pane with that row selected, which is precisely when they already know. This is the other
/// channel: a box in the corner, over whatever pane they are actually in, saying which pane
/// went idle and what it said. Same reasoning as agent's confirmation toast — a thing worth
/// reading must not wait in a pane nobody is looking at.
///
/// Headline then text, on two lines: the toast renderer takes `\n` as a hard row break, so
/// what is written here is what appears. The text is passed through whole and the renderer
/// wraps and ellipsizes what will not fit — cutting it here would only mean two places
/// disagreeing about how much room a toast has.
///
/// `kind` is optional because a pane's classification can fail; the pane id alone still says
/// which pane this is, so the segment is dropped rather than the whole toast.
pub fn summary_toast(pane_id: u32, kind: Option<&str>, summary: &Summary) -> String {
    // "waiting" is the same claim the strip's `!` marker makes, spelled out: a toast has the
    // room for a word and no legend beside it to explain a punctuation mark.
    let state = match summary.awaiting {
        true => "waiting",
        false => "idle",
    };
    // `{pane_id} · {kind}`, the spelling the inset's own header uses, so the two namings of
    // one pane read alike.
    let mut headline = format!("agent {state} · {pane_id}");
    if let Some(kind) = kind {
        headline.push_str(&format!(" · {kind}"));
    }
    format!("{headline}\n{}", summary.text)
}

/// The moment an agent starts waiting, as the toast that says so.
///
/// The other half of the reason [`summary_toast`] exists, and the more urgent one: a
/// summary describes work that is over, while this describes work that has *stopped* and
/// will not resume until somebody answers. Before the hook feed there was no way to draw
/// this in time — the screen heuristic needed fifteen seconds of stillness and a round
/// trip to a model to reach the same conclusion, by which point the interruption was
/// stale. Direct questions arrive immediately; Claude permission notifications confirm waits.
///
/// Shaped exactly like [`summary_toast`], down to the `·` separators, so a user who has
/// seen one recognises the other: headline, then one line saying what is being asked.
pub fn waiting_toast(pane_id: u32, kind: Option<&str>, feed: &HookFeed) -> String {
    let mut headline = format!("agent waiting · {pane_id}");
    if let Some(kind) = kind {
        headline.push_str(&format!(" · {kind}"));
    }
    // A wait we could not name is still worth interrupting for: the pane id in the
    // headline is what the user acts on, and the toast focuses that pane when clicked.
    let detail = feed
        .waiting_on
        .as_deref()
        .unwrap_or("waiting for you to answer");
    format!("{headline}\n{detail}")
}

/// Below this the text in the history half wraps into a column of single words, and the
/// half draws a placeholder instead.
pub const HISTORY_MIN_COLS: usize = 20;

/// What the history half of the agents pane draws, wrapped to `width` and unframed.
///
/// This used to be a framed box pinned to the foot of the pane, drawn only when there was
/// something to put in it. The half is permanent now and has a divider of its own, so what
/// the caller wants back is content plus enough to label and window it - the frame, the
/// width cap and the row ceiling all belonged to the box rather than to the history.
///
/// The rows are [`MdLine`]s rather than strings: everything in here is markdown by the time
/// it reaches a pane - a prompt is what the user typed, a result is a model's
/// `last_assistant_message` - and drawing the asterisks was never what any of it meant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryLines {
    /// What the divider says - `"claude:9f3c1a7e"`, `"3 · claude !"`. Empty when there is
    /// nothing to name. Uncut: the row it goes on carries the scroll readout too, so only
    /// whoever draws the divider knows the room a label has. See `fit_label`.
    pub label: String,
    /// The timeline, oldest first - summaries among the rest of the events rather than above
    /// them. The caller windows this against the room it has.
    pub entries: Vec<MdLine>,
    /// Which rows of `entries` came from which event, oldest first.
    ///
    /// `entries` alone is a flat run of lines, and a capped event is a lie without the whole
    /// of what it said kept somewhere: this is what lets a caller put a cursor on an event
    /// rather than on a line, and open the uncapped text when the user asks for it. The spans
    /// tile `entries` exactly - every row belongs to one event and no row to two - with one
    /// exception: a run that arrived without its oldest entries opens with a row saying so
    /// (see [`AgentRun::omitted_entries`]), which belongs to no event and which the first
    /// span starts after.
    pub events: Vec<HistoryEvent>,
    /// The rows a reader pins under its divider, above the scrolling timeline: what stays true
    /// of the run however far back it is wound - when it began, and which session it is.
    ///
    /// Held apart from `entries` rather than pushed onto the front of it so that the promise
    /// `events` makes above survives: these rows belong to no event, and a cursor can no more
    /// land on one than it can on the divider.
    ///
    /// Empty for everything but a loaded run: a summary drawn for an agent with no run has
    /// neither of the two facts to state.
    pub header: Vec<MdLine>,
}

/// One event of the timeline: which rows of [`HistoryLines::entries`] it took after the cap,
/// and the whole of what it said, for the reader that shows it uncapped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryEvent {
    /// The recorded entry, or None for the appended rolling summary.
    pub revision: Option<u64>,
    /// First row of this event in `entries`.
    pub start: usize,
    /// How many rows it took - never more than [`MAX_ENTRY_ROWS`].
    pub len: usize,
    /// What a reader's divider calls it: [`RunEntryKind::label`], or `"summary"`.
    pub kind: &'static str,
    /// When it was recorded, so a reader's age stays live rather than freezing at open.
    pub created_at_ms: Option<i64>,
    /// The event's text, uncapped and unwrapped. Shared rather than owned: every history
    /// render hands out every event's text, and a copy of each would cost a pass over the whole
    /// run on the path [`ParsedRun`] exists to make cheap.
    pub text: Arc<str>,
}

impl HistoryEvent {
    /// One past the last row of `entries` this event owns.
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Nothing selected, or nothing to say about what is - what a caller borrows when its cache is
/// empty.
pub static NO_HISTORY: HistoryLines = HistoryLines {
    label: String::new(),
    entries: Vec::new(),
    events: Vec::new(),
    header: Vec::new(),
};

impl HistoryLines {
    /// Nothing to draw at all - no label, no timeline.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The room a half `width` columns wide has for text, or `None` when there is not enough of it
/// to wrap into anything but a column of single words.
///
/// A margin column at each edge, the same one the list's marker column gives it. The divider is
/// not held to it - a rule wants both ends of the row - so this is the body's width alone.
fn wrap_width(width: usize) -> Option<usize> {
    let inner = width.saturating_sub(2);
    (inner >= HISTORY_MIN_COLS).then_some(inner)
}

/// A label cut to the room the divider has, so a long kind cannot push the rule off the row.
///
/// `width` is what is left of the row for the label and its rule: the caller has already
/// taken off anything else sharing it, which for the agents pane is the scroll readout at the
/// right-hand end.
pub fn fit_label(label: String, width: usize) -> String {
    // Two dashes and a space either side of the label is the least a divider is worth
    // drawing; below that there is no label at all.
    cut_to_width(label, width.saturating_sub(4))
}

/// `text` cut from the tail until it fits `width` terminal columns.
///
/// By column rather than by byte or by `char`: a title can hold anything the agent was asked,
/// and both a multi-byte character and a double-width one are drawn wrong by a count of the
/// other. Nothing is appended in place of what was dropped - the columns a table row divides up
/// are all it has, and an ellipsis would cost one of them on every cut cell.
pub fn cut_to_width(mut text: String, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    while text.width() > width {
        text.pop();
    }
    text
}

/// What joins two segments of a hint line.
const HINT_SEPARATOR: &str = " · ";

/// What a hint line ends with when [`fit_hints`] had to leave keys out of it.
const MORE_KEYS: &str = "? keys";

/// A key-hint line: `segments` joined with ` · ` for as long as they fit `width` columns.
///
/// Whole segments are dropped from the end, never cut: half of `Enter focus` names no key, and
/// a hint that reads `Enter fo` is worse than one that stops a key sooner. So callers put the
/// segments most important first - the selection's own verb, then the rest of the keys, then
/// anything that is not a key at all. Where something was dropped and there is still room,
/// the line ends in `? keys` to say there is more - room that is left over, never made:
/// [`fit_hints_marked`] is for a line that would rather give up a key or two for the marker,
/// and [`fit_keys`] for one that must not have it. Only a first segment too wide for the row
/// on its own is cut, by [`cut_to_width`], since an empty hint line helps nobody.
///
/// Not for table cells: those keep [`cut_to_width`], which has no segments to respect.
pub fn fit_hints(segments: &[&str], width: usize) -> String {
    let (mut line, dropped) = fit_segments(segments, width);
    if dropped && !line.is_empty() {
        let marked = line.width() + HINT_SEPARATOR.width() + MORE_KEYS.width();
        if marked <= width {
            line.push_str(HINT_SEPARATOR);
            line.push_str(MORE_KEYS);
        }
    }
    line
}

/// [`fit_hints`] for a line that would rather say `? keys` than show its last key or two.
///
/// [`fit_hints`] adds the marker only where it fits after everything else that did, and on a
/// line of keys alone that is almost never: at the widths a panel is usually drawn, the keys
/// fill the row to within a few columns and `? keys` needs nine. That is the right trade where
/// the line leads with something that is not a key - a status or a note, where the one action
/// after it is the only useful thing left to press - so [`fit_hints`] keeps it. A footer of
/// nothing but keys is the other case: its trailing keys are the least important ones by
/// construction, and the sheet `?` opens lists all of them, so giving up one or two to say so
/// is the better use of the row.
///
/// So this drops whole segments from the end, one at a time, until the marker fits after what
/// is left - but never below the first `keep`, which a caller sets to however many segments
/// name the keys a user cannot do without here. Where no count down to `keep` makes room, or
/// fewer than `keep` fit at all, the line is exactly what [`fit_hints`] would draw.
///
/// Segments are counted rather than found by searching for the separator: a status segment can
/// hold ` · ` of its own, and cutting at the last one would split it. Nothing is ever cut to
/// make room.
pub fn fit_hints_marked(segments: &[&str], width: usize, keep: usize) -> String {
    let unmarked = fit_hints(segments, width);
    if !fit_segments(segments, width).1 || unmarked.ends_with(MORE_KEYS) {
        return unmarked;
    }
    let segments: Vec<&str> = segments
        .iter()
        .copied()
        .filter(|segment| !segment.is_empty())
        .collect();
    // Longest first, and never all of them: something was dropped, so the whole line does not
    // fit even without the marker. Nor does any prefix longer than what `fit_segments` kept, so
    // counting down from the top finds the same line as counting down from what it kept - and a
    // first segment it had to cut fits no count at all, which leaves that cut line standing.
    let marker = HINT_SEPARATOR.width() + MORE_KEYS.width();
    (keep.max(1)..segments.len())
        .rev()
        .map(|n| segments[..n].join(HINT_SEPARATOR))
        .find(|line| line.width() + marker <= width)
        .map(|line| format!("{line}{HINT_SEPARATOR}{MORE_KEYS}"))
        .unwrap_or(unmarked)
}

/// A key-hint line with no `? keys` marker: the whole segments that fit, as [`fit_hints`]
/// drops them.
///
/// For the rows where the marker would be wrong rather than merely short of room: a row drawn
/// while `?` is being typed into a box, where the key types a question mark instead of opening
/// anything, and the rows of the sheet itself, where `?` is what shuts it.
pub fn fit_keys(segments: &[&str], width: usize) -> String {
    fit_segments(segments, width).0
}

/// [`fit_hints`] without the marker: the segments that fit, and whether any were left out.
/// Empty segments are skipped rather than drawn as a doubled separator.
fn fit_segments(segments: &[&str], width: usize) -> (String, bool) {
    let mut segments = segments.iter().filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return (String::new(), false);
    };
    if first.width() > width {
        return (cut_to_width((*first).to_owned(), width), true);
    }
    let mut line = (*first).to_owned();
    for segment in segments {
        if line.width() + HINT_SEPARATOR.width() + segment.width() > width {
            return (line, true);
        }
        line.push_str(HINT_SEPARATOR);
        line.push_str(segment);
    }
    (line, false)
}

/// The selected run as the history half draws it: one timeline, summaries among the events
/// rather than above them.
///
/// The filter is the one the box drew: prompts, plans and questions always, and a result
/// only on a run that has stopped and only while the summary has not yet absorbed it. A
/// finished run whose summary is up to date says what it did once, in the summary row,
/// rather than twice.
///
/// The rolling [`AgentRun::summary`] is appended as the newest row, which is where in time it
/// was made. It is dropped when the newest [`RunEntryKind::Summary`] already on the timeline
/// says the same thing - a summary somebody pressed a key for both stays where it was asked
/// for and is the current one, and two identical paragraphs one under the other read as a bug
/// rather than as a history.
pub fn run_history(
    run: &AgentRun,
    width: usize,
    now_ms: i64,
    utc_offset_secs: i32,
) -> HistoryLines {
    ParsedRun::new(run).lines(width, now_ms, utc_offset_secs)
}

/// [`run_history`] with the expensive half kept: every event's markdown parsed once, and its
/// rows wrapped once per width.
///
/// A caller that draws the same run again and again - the agents pane redraws it on every
/// resize step, every minute, and every time agent sends the run with one more entry on it -
/// holds one of these instead of calling `run_history`. The parse depends on nothing but the
/// text, so a resize only re-wraps; the wrap depends on nothing but the text and the width, so
/// a new entry or a new minute wraps nothing but what is new. A run near
/// `rows::RUN_DETAIL_BYTES` is hundreds of kilobytes of markdown, and re-parsing all of it per
/// resize step is what made dragging the pane's edge stall for seconds.
///
/// What an [`ParsedRun::update`] costs is one lookup per entry and the parse of whatever is
/// new: a recorded entry it already holds is recognised without reading its text (see
/// [`ParsedEvent::is`]). The caller has paid for decoding the run by then, which is the one
/// pass over all of it that is left - and [`run_payload_fingerprint`] is how the agents pane
/// avoids even that for a run sent again unchanged.
#[derive(Default)]
pub struct ParsedRun {
    /// The run's own facts, for the label and the pinned header. Cheap, so refreshed whole on
    /// every [`ParsedRun::update`].
    run: RunFacts,
    /// The timeline's events in order, the rolling summary last when it is drawn at all.
    events: Vec<ParsedEvent>,
    /// How many events have been parsed over this one's life.
    ///
    /// Test-only: a kept parse and a fresh one draw the same rows, so nothing else can tell a
    /// cache that works from one that quietly re-parses everything.
    #[cfg(test)]
    parses: usize,
}

/// One event of a [`ParsedRun`]: what it said, parsed, and its rows at the last width asked for.
struct ParsedEvent {
    /// The recorded entry's revision, or `None` for the rolling summary.
    revision: Option<u64>,
    kind: &'static str,
    style: MdStyle,
    created_at_ms: Option<i64>,
    /// The whole of what the event said, for [`HistoryEvent::text`].
    raw: Arc<str>,
    /// The head of what it says, parsed: only as much as the timeline can draw of it. See
    /// [`ENTRY_HEAD_BLOCKS`] and [`ENTRY_HEAD_BYTES`]. Shared, so a reader opened on a short
    /// event can draw this parse rather than make its own: see [`ParsedRun::whole_blocks`].
    blocks: Arc<[crate::markdown::MdBlock]>,
    /// Whether `blocks` is the whole of what was parsed. An event that was not parsed whole is
    /// capped however few rows its head came to, so the marker still says there is more.
    complete: bool,
    /// Whether `blocks` is the whole of `raw`: complete, and parsed from the text itself rather
    /// than from a digest of its envelope.
    whole: bool,
    /// The capped rows, without the gutter's text on the first of them, and the `(width,
    /// indent)` they were wrapped for.
    rows: Option<((usize, usize), Vec<MdLine>)>,
    /// The gutter the last [`ParsedRun::lines`] put on the first row, which is all
    /// [`ParsedRun::restamp`] has to change when only the clock moved.
    gutter: String,
}

/// Tool payloads are literal terminal-safe text, never Markdown instructions.
pub fn entry_blocks(kind: &str, text: &str) -> Vec<crate::markdown::MdBlock> {
    if !matches!(kind, "tool_call" | "tool_result") {
        return crate::markdown::parse(text);
    }
    crate::text::sanitize(text)
        .lines()
        .map(|line| crate::markdown::MdBlock {
            chars: line
                .chars()
                .map(|ch| crate::markdown::StyledChar {
                    ch,
                    style: Default::default(),
                })
                .collect(),
            compact: true,
            ..Default::default()
        })
        .collect()
}

impl ParsedEvent {
    fn new(
        revision: Option<u64>,
        kind: &'static str,
        style: MdStyle,
        created_at_ms: Option<i64>,
        digest: Option<&str>,
        raw: &str,
    ) -> Self {
        let (blocks, complete) = if matches!(kind, "tool_call" | "tool_result") {
            let mut end = raw.len().min(ENTRY_HEAD_BYTES);
            while !raw.is_char_boundary(end) {
                end -= 1;
            }
            (entry_blocks(kind, &raw[..end]), end == raw.len())
        } else {
            crate::markdown::parse_head(digest.unwrap_or(raw), ENTRY_HEAD_BLOCKS, ENTRY_HEAD_BYTES)
        };
        Self {
            revision,
            kind,
            style,
            created_at_ms,
            raw: raw.into(),
            blocks: blocks.into(),
            complete,
            whole: complete && digest.is_none(),
            rows: None,
            gutter: String::new(),
        }
    }

    /// Whether the entry held under this one's revision would parse to what this holds, so
    /// the parse can be kept.
    ///
    /// A recorded entry is recognised by its kind and its length, not by reading its text. A
    /// revision names one entry of one run, and an entry is never rewritten under it: the
    /// daemon only ever pushes, and `rows::hydrate_newest_entries` drops whole entries to fit
    /// its budget rather than cutting one. Comparing the text as well would walk every byte
    /// of a long run on each resend to confirm what the revision already says. The length is
    /// kept as a cheap tripwire all the same, and the kind because a prompt is parsed from its
    /// envelope's digest rather than from its text. [`ParsedRun::update`] drops everything
    /// held when the run itself changes, since revisions are only unique within one.
    ///
    /// The rolling summary (`revision` `None`) is compared whole: it is rewritten in place
    /// every time the run is summarised again, often to a paragraph much like the last, and it
    /// is one short paragraph. Not the stamp, which only the gutter reads - an entry whose
    /// stamp was corrected takes the new one and keeps its parse.
    fn is(&self, kind: &str, raw: &str) -> bool {
        self.kind == kind
            && match self.revision {
                Some(_) => self.raw.len() == raw.len(),
                None => *self.raw == *raw,
            }
    }
}

impl ParsedRun {
    pub fn new(run: &AgentRun) -> Self {
        let mut parsed = Self::default();
        parsed.update(run);
        parsed
    }

    /// How many styled characters the held parse keeps, across every event and table cell.
    ///
    /// Test-only, like `parses`: a head that quietly grew back to the whole event draws the
    /// same rows, so only this can tell.
    #[cfg(test)]
    fn parsed_chars(&self) -> usize {
        self.events
            .iter()
            .flat_map(|event| event.blocks.iter())
            .map(|block| {
                let cells = block.table.as_ref().map_or(0, |table| {
                    table
                        .head
                        .iter()
                        .chain(table.rows.iter().flatten())
                        .map(Vec::len)
                        .sum()
                });
                block.chars.len() + cells
            })
            .sum()
    }

    /// Take `run` as the one this draws, parsing only the events it does not already hold.
    /// Which events those are, and where the rolling summary goes, is [`run_history`]'s.
    pub fn update(&mut self, run: &AgentRun) {
        // Revisions are counted per run, so another run's revision 3 is not this one's.
        if self.run.id != run.id {
            self.events.clear();
        }
        let mut held: BTreeMap<Option<u64>, ParsedEvent> = std::mem::take(&mut self.events)
            .into_iter()
            .map(|event| (event.revision, event))
            .collect();
        let mut take = |revision: Option<u64>,
                        kind: &'static str,
                        style: MdStyle,
                        created_at_ms: Option<i64>,
                        raw: &str,
                        drawn: &dyn Fn() -> Option<String>| {
            match held.remove(&revision) {
                Some(mut event) if event.is(kind, raw) => {
                    event.style = style;
                    event.created_at_ms = created_at_ms;
                    event
                },
                _ => {
                    #[cfg(test)]
                    {
                        self.parses += 1;
                    }
                    ParsedEvent::new(
                        revision,
                        kind,
                        style,
                        created_at_ms,
                        drawn().as_deref(),
                        raw,
                    )
                },
            }
        };
        for entry in run.entries.iter().filter(|entry| {
            matches!(
                entry.kind,
                RunEntryKind::Prompt
                    | RunEntryKind::Plan
                    | RunEntryKind::Question
                    | RunEntryKind::Summary
                    | RunEntryKind::Message
                    | RunEntryKind::ToolCall
                    | RunEntryKind::ToolResult
                    | RunEntryKind::Result
            )
        }) {
            let style = match entry.kind {
                RunEntryKind::Summary => SUMMARY_LABEL_STYLE,
                _ => LABEL_STYLE,
            };
            // `drawn` and `raw` diverge for exactly one kind: a prompt carrying a hook envelope
            // is *drawn* as `crate::envelope`'s reading of it and *recorded* as what arrived.
            // Everything else passes the same text twice. Keeping the two apart here is what
            // lets the timeline read as prose while `HistoryEvent::text` keeps its contract -
            // the whole of what it said, which is what the reader shows and what `r` shows raw.
            //
            // `digest` is `None` for everything a person typed, which is the common case and
            // the one that must cost nothing; the label stays `prompt:` either way, because a
            // prompt is what it is however it was written. Only asked when the entry is new.
            let prompt = matches!(entry.kind, RunEntryKind::Prompt);
            let event = take(
                Some(entry.revision),
                entry.kind.label(),
                style,
                entry.created_at_ms,
                &entry.text,
                &|| {
                    prompt
                        .then(|| crate::envelope::digest(&entry.text))
                        .flatten()
                },
            );
            self.events.push(event);
        }
        if let Some(summary) = run
            .summary
            .as_ref()
            .filter(|summary| !repeats_newest(run, summary))
        {
            let event = take(
                None,
                "summary",
                SUMMARY_LABEL_STYLE,
                summary.created_at_ms,
                &summary.text,
                &|| None,
            );
            self.events.push(event);
        }
        self.run = RunFacts::of(run);
    }

    /// The run as the history half draws it at `width`, its ages read against `now_ms`.
    pub fn lines(&mut self, width: usize, now_ms: i64, utc_offset_secs: i32) -> HistoryLines {
        let Some(inner) = wrap_width(width) else {
            return HistoryLines::default();
        };
        let mut entries: Vec<MdLine> = Vec::new();
        let mut events: Vec<HistoryEvent> = Vec::with_capacity(self.events.len());
        // First, so it is what the top of the scroll reaches and every span below still starts
        // where its rows do. It belongs to no event, so no cursor can land on it.
        if self.run.omitted_entries > 0 {
            entries.push(omitted_notice(inner));
        }
        // Walked rather than `flat_map`ped: each event's rows have to be counted before the next
        // one's begin, which is the whole of what a span is.
        for event in &mut self.events {
            let gutter = age_gutter(now_ms, event.created_at_ms);
            let key = (inner, gutter.width());
            let rows = match &event.rows {
                Some((held, rows)) if *held == key => rows,
                _ => {
                    let rows = cap_rows(
                        labelled_blocks(&event.blocks, event.kind, key.1, inner, event.style).0,
                        key.1,
                        event.complete,
                    );
                    &event.rows.insert((key, rows)).1
                },
            };
            events.push(HistoryEvent {
                revision: event.revision,
                start: entries.len(),
                len: rows.len(),
                kind: event.kind,
                created_at_ms: event.created_at_ms,
                text: Arc::clone(&event.raw),
            });
            let mut rows = rows.iter().cloned();
            if let Some(first) = rows.next() {
                entries.push(first.prepend(&gutter, AGE_STYLE));
            }
            entries.extend(rows);
            event.gutter = gutter;
        }
        HistoryLines {
            header: run_header(&self.run, inner, utc_offset_secs),
            // The run's own id rather than its kind: the id opens with the kind anyway
            // (`run_id`), so this says everything the kind said and names *which* run of that
            // agent the half is drawing - the one thing the pane could not tell you before.
            // Shortened, because the whole id does not fit a divider (`short_run_id`), and
            // without the run's state, which the selector row above already carries.
            label: short_run_id(&self.run.id),
            entries,
            events,
        }
    }

    /// Bring `lines` - what the last [`Self::lines`] returned, with nothing updated since - up
    /// to `now_ms`, and say whether it could.
    ///
    /// The clock moves nothing but the gutters, so this rewrites the first row of each event
    /// whose age now reads differently and leaves every other row where it is, rather than
    /// cloning the whole timeline out of the kept rows again as `lines` does. Refuses, touching
    /// nothing, when `lines` does not have the shape the kept rows would give it, or a gutter
    /// changed width - rows are wrapped beside the gutter, so that one needs `lines`.
    pub fn restamp(&mut self, lines: &mut HistoryLines, now_ms: i64) -> bool {
        if lines.events.len() != self.events.len() {
            return false;
        }
        let mut changed = Vec::new();
        for (index, (event, drawn)) in self.events.iter().zip(&lines.events).enumerate() {
            let Some(((_, indent), rows)) = &event.rows else {
                return false;
            };
            if drawn.revision != event.revision
                || drawn.len != rows.len()
                || drawn.end() > lines.entries.len()
            {
                return false;
            }
            let gutter = age_gutter(now_ms, event.created_at_ms);
            if gutter.width() != *indent {
                return false;
            }
            if gutter != event.gutter {
                changed.push((index, gutter));
            }
        }
        for (index, gutter) in changed {
            let event = &mut self.events[index];
            let first = event
                .rows
                .as_ref()
                .and_then(|(_, rows)| rows.first())
                .cloned();
            if let Some(first) = first {
                lines.entries[lines.events[index].start] = first.prepend(&gutter, AGE_STYLE);
            }
            event.gutter = gutter;
        }
        true
    }

    /// The kept parse of the event whose [`HistoryEvent::text`] is `text`, where it is the
    /// whole of that text - what a reader of the event would otherwise parse again. `None` for
    /// an event parsed only in part, or drawn from its envelope's digest rather than its text.
    ///
    /// Found by the text's allocation rather than by revision: `lines` hands out the one it
    /// holds, so a match is this very event and not an older one filed under the same revision.
    pub fn whole_blocks(&self, text: &Arc<str>) -> Option<Arc<[crate::markdown::MdBlock]>> {
        self.events
            .iter()
            .find(|event| event.whole && Arc::ptr_eq(&event.raw, text))
            .map(|event| Arc::clone(&event.blocks))
    }
}

/// What [`omitted_notice`] says happened.
const OMITTED_NOTICE: &str = "older events not loaded";

/// [`OMITTED_NOTICE`] in fewer words, for a half too narrow for the whole phrase.
const OMITTED_NOTICE_SHORT: &str = "older not loaded";

// The narrowest half that draws a timeline at all has room for the short phrase after the age
// gutter, so it is never cut. By `len`, since `width` is not const: the phrase is ASCII.
const _: () = assert!(OMITTED_NOTICE_SHORT.len() <= HISTORY_MIN_COLS - AGE_WIDTH);

/// What a timeline opens with when its run arrived without its oldest entries: see
/// [`AgentRun::omitted_entries`].
///
/// Without it the oldest row loaded reads as the start of the run. The second half says where
/// the rest can still be found, and goes first when the half is too narrow for both. Narrower
/// still, the notice says it in fewer words rather than being cut: `older events not` is a
/// sentence that stopped, where [`OMITTED_NOTICE_SHORT`] is a whole one. Always the one row, so
/// the event spans after it start where they did. Dim and under the labels, like the capped
/// marker: it is pane furniture, not something the run said.
fn omitted_notice(width: usize) -> MdLine {
    let room = width.saturating_sub(AGE_WIDTH);
    let notice = if OMITTED_NOTICE.width() <= room {
        fit_segments(&[OMITTED_NOTICE, "/ searches them"], room).0
    } else {
        OMITTED_NOTICE_SHORT.to_owned()
    };
    MdLine::plain("")
        .prepend(&notice, LABEL_STYLE)
        .indent(AGE_WIDTH)
}

/// A run's JSON reduced to its length and a hash, for telling a run sent again unchanged from
/// one that moved without holding on to the bytes it came as.
///
/// A long run is a quarter of a megabyte of JSON, and keeping it only to compare the next one
/// against it was a second whole copy of the run beside the parse. `DefaultHasher` rather than
/// a stable hash: the value is compared within one process and never written anywhere, which
/// is the opposite of what `envelope`'s FNV key is for.
pub fn run_payload_fingerprint(payload: &str) -> (usize, u64) {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(payload.as_bytes());
    (payload.len(), hasher.finish())
}

/// What [`run_header`] and the divider's label read of a run: its identity, without the
/// entries a [`ParsedRun`] holds parsed instead.
#[derive(Default)]
struct RunFacts {
    id: String,
    session_id: String,
    kind: String,
    started_at_ms: i64,
    origin_run_id: Option<String>,
    previous_run_id: Option<String>,
    /// [`AgentRun::omitted_entries`], for the notice the timeline opens with.
    omitted_entries: usize,
}

impl RunFacts {
    fn of(run: &AgentRun) -> Self {
        Self {
            id: run.id.clone(),
            session_id: run.session_id.clone(),
            kind: run.kind.clone(),
            started_at_ms: run.started_at_ms,
            origin_run_id: run.origin_run_id.clone(),
            previous_run_id: run.previous_run_id.clone(),
            omitted_entries: run.omitted_entries,
        }
    }
}

/// The run identity fields [`run_history`] pins above its timeline.
///
/// Both answer a question the timeline cannot. `short_run_id` on the divider says *which of
/// this agent's runs* is being drawn, in the eight characters a rule can spare; this says the
/// whole id, which is the one you paste into a `--resume`, a log grep or an `agent_run` call.
/// And every row below is stamped with a relative age, which stops being an answer the moment
/// the question is "which afternoon was that" - so the run's own start is drawn absolutely.
///
/// Wrapped rather than cut. A tail-cut uuid is a uuid nobody can use, and unlike the divider
/// (`fit_label`) this has as many rows as it needs.
fn run_header(run: &RunFacts, width: usize, utc_offset_secs: i32) -> Vec<MdLine> {
    // `run.id` when the agent supplied no session id of its own: `run_id` keys such a run by
    // pane and start time, and naming that "session" would be a claim the run cannot make.
    let (label, value) = match run.session_id.trim() {
        "" => ("run", run.id.as_str()),
        session_id => ("session", session_id),
    };
    let mut rows = wrapped_field(
        "started",
        &wall_clock(
            yinz_tile::prelude::SignedUnixMillis::new(run.started_at_ms),
            utc_offset_secs,
        ),
        width,
    );
    rows.extend(wrapped_field(label, value, width));
    for (label, id) in [
        ("origin", run.origin_run_id.as_deref()),
        ("previous", run.previous_run_id.as_deref()),
    ] {
        if let Some(id) = id {
            let prefix = format!("{}:", run.kind);
            rows.extend(wrapped_field(
                label,
                id.strip_prefix(&prefix).unwrap_or(id),
                width,
            ));
        }
    }
    rows
}

/// One `label  value` row, plus a continuation row per further slice the value needs, each
/// indented under the value column so the field reads as one block.
///
/// The label column is at least [`HEADER_LABEL_WIDTH`] wide, with a separating space
/// even for the longer `previous` label.
fn wrapped_field(label: &str, value: &str, width: usize) -> Vec<MdLine> {
    let label_width = HEADER_LABEL_WIDTH.max(label.len() + 1);
    let label = format!("{label:<label_width$}");
    // At least one column, or a half narrow enough to leave none would loop forever below.
    let room = width.saturating_sub(label_width).max(1);
    // By character rather than by byte: a session id is a uuid today, but nothing stops a
    // hand-rolled `agent-report` from naming a run in any script at all, and a slice through
    // the middle of a character panics.
    let mut chars = value.chars().peekable();
    let mut rows = Vec::new();
    while chars.peek().is_some() {
        let slice: String = chars.by_ref().take(room).collect();
        let gutter = match rows.is_empty() {
            true => label.clone(),
            false => " ".repeat(label_width),
        };
        rows.push(MdLine::plain(slice).prepend(&gutter, HEADER_LABEL_STYLE));
    }
    rows
}

/// The width of the header's label column - `"session"` and a space, which is the longest of
/// them. Wider than the timeline's own [`AGE_WIDTH`] gutter on purpose: these rows are not part
/// of the timeline and lining them up with it would say they were.
const HEADER_LABEL_WIDTH: usize = 8;

/// How a header label is painted: dim, like every other piece of pane furniture. The values
/// beside them are not - a timestamp and a session id are what the rows are for.
const HEADER_LABEL_STYLE: MdStyle = LABEL_STYLE;

/// Whether the newest summary already on the run's timeline says what `summary` says - the
/// same comparison [`push_summary_entry`] makes, asked of the rolling summary before it is
/// drawn under the entry it duplicates.
fn repeats_newest(run: &AgentRun, summary: &Summary) -> bool {
    run.entries
        .iter()
        .rev()
        .find(|entry| entry.kind == RunEntryKind::Summary)
        .is_some_and(|entry| entry.text == summary.text.trim())
}

/// The idle summary as the history half draws it, for a selected agent with no run loaded.
///
/// `waiting` is the flag on the row's own [`StripRow::Agent`], not `summary.awaiting`: the stored
/// flag outlives the screen it was read from, and a divider that went on saying `!` after the
/// user answered would contradict the row above it, which checks that the summary is still
/// about the current screen.
///
/// A talkative model is windowed by the caller rather than cut here, which is what the box
/// had to do with six rows to give.
pub fn summary_history(
    pane_id: u32,
    kind: &str,
    summary: &Summary,
    waiting: bool,
    width: usize,
    now_ms: i64,
) -> HistoryLines {
    let Some(inner) = wrap_width(width) else {
        return HistoryLines::default();
    };
    let mut label = format!("{pane_id} · {kind}");
    if waiting {
        // The same claim the strip's `!` marker makes, from the same flag, in the one place
        // the user is already looking. Ahead of the age because `fit_label` cuts from the
        // tail: a divider too narrow for both should lose how old the answer is rather than
        // that the agent is waiting on you.
        label.push_str(" !");
    }
    // The age goes on the divider rather than in a gutter: there is one thing below it and
    // no column of rows for the times to line up in.
    if let Some(then_ms) = summary.created_at_ms {
        label.push_str(&format!(" · {}", short_age(now_ms, then_ms)));
    }
    // No label: the divider above it already names the agent this is about, and there is
    // no second kind of row here for one to tell it apart from.
    let entries =
        crate::markdown::render(&summary.text, inner, crate::markdown::Options::compact());
    // Uncapped, unlike a run's events: there is only one of these and nothing beside it
    // competing for the room, so a talkative model costs nothing here. It is still one event,
    // so a cursor can land on it and open it in full the way it can any other.
    let events = match entries.is_empty() {
        true => Vec::new(),
        false => vec![HistoryEvent {
            revision: None,
            start: 0,
            len: entries.len(),
            kind: "summary",
            created_at_ms: summary.created_at_ms,
            text: summary.text.as_str().into(),
        }],
    };
    HistoryLines {
        label,
        entries,
        events,
        // No run, so neither of the two facts a header states is in hand: an idle agent's
        // summary is about the pane, not about a session with a start and an id.
        header: Vec::new(),
    }
}

/// The most rows one event of a run's timeline gets, the overflow marker included.
///
/// Four, because that is enough for a prompt or a question whole and enough of a result to know
/// whether it is the one you were looking for. Before this a single talkative `result:` could
/// fill the half and push everything either side of it out of view, which is the opposite of
/// what a timeline is for. Everything past the cap is still reachable - a cursor on the event
/// and `Enter` opens the whole of it - which is what makes this a cap rather than a truncation.
pub const MAX_ENTRY_ROWS: usize = 4;

/// One event's rows, cut to [`MAX_ENTRY_ROWS`] with the last of them saying there is more.
///
/// The marker takes one of the four rather than being squeezed in beside them: a row that says
/// "there is more here" is worth as much as the row of prose it replaces, and a cap that
/// silently dropped the tail would read as an agent that stopped mid-sentence.
///
/// It does not say how much more. It used to count the rows dropped, which meant wrapping the
/// whole of every event at every width only to throw all but four rows away - most of what a
/// resize of the agents pane cost, on a run of long plans and messages. [`labelled_blocks`]
/// now stops wrapping at the first row past the cap, and `Enter` on the event is how much.
///
/// `indent` is the entry's gutter width, so the marker sits in the column the entry's
/// continuation rows are indented to rather than out at the margin.
///
/// `complete` is whether `rows` were wrapped from the whole of the event. One that was parsed
/// only in part is capped whatever its head came to - see [`ENTRY_HEAD_BYTES`].
fn cap_rows(mut rows: Vec<MdLine>, indent: usize, complete: bool) -> Vec<MdLine> {
    if complete && rows.len() <= MAX_ENTRY_ROWS {
        return rows;
    }
    rows.truncate(MAX_ENTRY_ROWS - 1);
    rows.push(
        MdLine::plain("")
            .prepend(CAPPED_MARKER, LABEL_STYLE)
            .indent(indent),
    );
    rows
}

/// What the last row of a capped event says. See [`cap_rows`].
const CAPPED_MARKER: &str = "… more";

/// How many of an event's blocks the timeline parses.
///
/// Enough to tell whether it overflows at any width. Every block draws at least one row, and
/// [`wrap_head`] never wants more than one row past [`MAX_ENTRY_ROWS`], so the block after
/// these could only ever be cut off.
const ENTRY_HEAD_BLOCKS: usize = MAX_ENTRY_ROWS + 1;

/// How many bytes of an event's text the timeline parses.
///
/// A block count alone does not bound the parse: a 100KB code fence or table is one block, and
/// pulldown-cmark reads the whole of its input before handing over the first. Four rows at even
/// five hundred columns is two thousand characters, so this leaves eight times that for markup
/// and multi-byte characters. An event longer than this is capped whatever its head comes to,
/// and `Enter` opens the whole of it from [`HistoryEvent::text`].
const ENTRY_HEAD_BYTES: usize = 16 * 1024;

/// [`crate::markdown::wrap`] with compact blocks, stopped once it has more than `limit` rows.
///
/// All [`cap_rows`] needs to know is whether an event overflows, and the first row past the
/// cap answers that; the rows after it would be wrapped only to be dropped. That holds inside a
/// block as well as between them, since one block can be the whole of a long event.
fn wrap_head(blocks: &[crate::markdown::MdBlock], cols: usize, limit: usize) -> Vec<MdLine> {
    if blocks.is_empty() {
        return vec![MdLine::blank()];
    }
    let mut lines = Vec::new();
    for block in blocks {
        let Some(wanted) = (limit + 1)
            .checked_sub(lines.len())
            .filter(|rows| *rows > 0)
        else {
            break;
        };
        lines.extend(crate::markdown::render_block_head(block, cols, wanted));
    }
    lines
}

/// One history row's text as markdown, with `label` in front of it, and whether the label took
/// a row to itself.
///
/// The label rides on the first rendered line rather than being spliced into the source: a
/// result that opens with `# Heading` or `- one` must still be a heading or a bullet, and
/// `result: # Heading` on one line is neither. When the first block carries a prefix of its own,
/// a list marker or a quote bar, the label takes a line to itself instead, so it cannot be glued
/// onto a marker it does not belong to, and so the marker keeps the column its continuation rows
/// are indented to.
///
/// Which of the two it is has to be known before a width can be picked, which is why this takes
/// the text already parsed into blocks rather than rendering it twice. Parsed by the caller
/// rather than here so [`ParsedRun`] can keep the parse across widths.
///
/// The gutter's own text is not on the first row - only its width, `indent`, which the rows
/// below are indented by. The gutter is the one part of an entry that changes with the clock,
/// so leaving it off is what lets the wrapped rows be kept across minutes. Never empty, so a
/// caller can always put the gutter there.
///
/// Wrapped only as far as [`cap_rows`] needs: the first row past [`MAX_ENTRY_ROWS`] says an
/// event is capped, and nothing after it is ever drawn.
///
/// `style` is how the label itself is painted: dim for most kinds, because it is the pane
/// talking rather than the agent, and bold for a summary, which is the one kind worth picking
/// out of the timeline now that nothing stands above it.
fn labelled_blocks(
    blocks: &[crate::markdown::MdBlock],
    label: &str,
    indent: usize,
    width: usize,
    style: MdStyle,
) -> (Vec<MdLine>, bool) {
    let label = format!("{label}: ");
    let inline = blocks
        .first()
        .is_some_and(|block| block.prefix.is_empty() && !block.chars.is_empty());
    if inline {
        // Wrapped to what is left after the gutter and the label rather than to the whole
        // half. Only the first row carries the label, so the rows below come out a little
        // short of the edge - but `print_text_with_coordinates` truncates rather than wraps,
        // so wrapping to the full width and *then* prepending would cut the tail off the
        // first row of every entry. The rows below are indented past the gutter instead of
        // starting at the margin, so the ages have a column of their own to be read down.
        let body = width
            .saturating_sub(indent)
            .saturating_sub(label.width())
            .max(1);
        let mut lines = wrap_head(blocks, body, MAX_ENTRY_ROWS);
        let first = lines.remove(0);
        let mut rows = vec![first.prepend(&label, style)];
        rows.extend(lines.into_iter().map(|line| line.indent(indent)));
        return (rows, false);
    }
    // Nothing shares the label's row in this branch, so the body gets the width back - all
    // but the gutter, which every row of an entry leaves clear.
    let mut lines = wrap_head(
        blocks,
        width.saturating_sub(indent).max(1),
        MAX_ENTRY_ROWS - 1,
    );
    let mut labelled = vec![MdLine::plain("").prepend(label.trim_end(), style)];
    labelled.extend(lines.drain(..).map(|line| line.indent(indent)));
    (labelled, true)
}

/// How a history row's `prompt: ` label is painted: dim, the way every other piece of pane
/// furniture is.
const LABEL_STYLE: MdStyle = MdStyle {
    bold: false,
    dim: true,
    color: None,
    error: false,
    strike: false,
};

/// How wide the age column is: three columns for the age itself and a space after it, so
/// `now`, `5m`, `3h` and `2d` all line up and every label in the half starts at the same
/// place whether its entry knows when it was made or not.
pub const AGE_WIDTH: usize = 4;

/// How an age is painted: undimmed and in the accent colour, because it is the one piece of
/// furniture on the row that is meant to be read down the column rather than through the
/// sentence.
const AGE_STYLE: MdStyle = MdStyle {
    bold: false,
    dim: false,
    color: Some(2),
    error: false,
    strike: false,
};

/// The left-hand column of a history row: how long ago the entry was made, right-aligned in
/// [`AGE_WIDTH`], or that much blank for an entry recorded before there was a time to record
/// - the space is kept either way so the labels stay in one column.
pub fn age_gutter(now_ms: i64, then_ms: Option<i64>) -> String {
    match then_ms {
        Some(then_ms) => format!(
            "{:>width$} ",
            short_age(now_ms, then_ms),
            width = AGE_WIDTH - 1
        ),
        None => " ".repeat(AGE_WIDTH),
    }
}

/// How long ago `then_ms` was, in the width a column can afford: `now` under a minute, then
/// whole minutes, hours and days - truncated, not rounded, since a history row cares that an
/// entry is roughly this old, not exactly.
///
/// Never wider than `AGE_WIDTH - 1`, which is what lets [`age_gutter`] right-align it and know
/// every label starts in the same column. That is what caps the day count: past `99d` the
/// answer is "older than this column can say", and a run three months gone is history, where
/// the exact day is not what the column is being read for.
/// Now, as an epoch millisecond — the unit every timestamp in this module is in: a run's
/// `started_at_ms`, an entry's `created_at_ms`, [`short_age`]'s two arguments.
///
/// Re-exported rather than written here: it lives in `yinz-tile` next to
/// `get_local_utc_offset`, which is the host command that makes any of this drawable at all.
pub fn now_ms() -> i64 {
    yinz_tile::prelude::now_ms().get()
}

pub fn short_age(now_ms: i64, then_ms: i64) -> String {
    let seconds = now_ms.saturating_sub(then_ms).max(0) / 1000;
    match seconds {
        s if s < 60 => "now".to_owned(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", (s / 86400).min(MAX_AGE_DAYS)),
    }
}

/// The oldest age [`short_age`] will name, so its answer fits the gutter. See there.
const MAX_AGE_DAYS: i64 = 99;

/// An epoch millisecond as the wall clock read where the session runs:
/// `"2026-09-04 14:32:07 -04:00"`.
///
/// Everything a relative [`short_age`] cannot answer: which day, and which minute of it. The
/// two are not alternatives - a run's start is drawn absolutely because it is asked about long
/// after the fact, and every entry under it is drawn as an age because what is being read there
/// is the spacing.
///
/// Re-exported from `yinz-tile`, which is where the civil-calendar arithmetic and the
/// `get_local_utc_offset` host command that feeds it now live, so every plugin that draws a
/// timestamp shares one copy. See `yinz_tile::clock`.
pub use yinz_tile::prelude::wall_clock;

/// How a `summary: ` label is painted instead: bold and undimmed. The summary used to be told
/// apart by standing above the timeline; it is a row of the timeline now, so its label is what
/// marks it.
const SUMMARY_LABEL_STYLE: MdStyle = MdStyle {
    bold: true,
    dim: false,
    color: None,
    error: false,
    strike: false,
};

/// Whether a pane holds enough to be worth asking about.
pub fn worth_summarizing(text: &str) -> bool {
    text.trim().chars().count() >= SUMMARY_MIN_CHARS
}

/// The body of one non-streaming completion asking what an idle agent is doing.
///
/// Deliberately carries no `temperature` and no `max_tokens`: providers disagree about
/// both — newer OpenAI models reject a `temperature` that is not 1 and want
/// `max_completion_tokens` instead of `max_tokens` — and a rejected request buys nothing
/// over a slightly long answer that [`parse_summary`] caps anyway.
pub fn summary_request_body(
    model: &str,
    kind: &str,
    cwd: Option<&str>,
    branch: Option<&str>,
    tail: &str,
) -> Vec<u8> {
    let mut header = format!("Agent: {kind}.");
    if let Some(cwd) = cwd {
        header.push_str(&format!(" Folder: {cwd}."));
    }
    if let Some(branch) = branch {
        header.push_str(&format!(" Branch: {branch}."));
    }
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": SUMMARY_SYSTEM_PROMPT },
            { "role": "user", "content": format!("{header}\n\n--- pane ---\n{tail}") },
        ],
    });
    serde_json::to_vec(&body).expect("serializing a summary request cannot fail")
}

/// The `{...}` a model wrapped in fences, prose, or both.
///
/// Scanned rather than trimmed because every provider decorates differently, and a model
/// told "no markdown" still sometimes writes ```json. Brace-counting, skipping over string
/// literals so a `}` inside the summary text cannot end the object early.
fn first_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in text[start..].char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {},
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + c.len_utf8()]);
                }
            },
            _ => {},
        }
    }
    None
}

/// What the model answered, or `None` when it answered nothing usable.
///
/// `None` is an ordinary outcome for a background caller: no summary is stored, the flag
/// stays as it was, and the next idle edge asks again.
pub fn parse_summary(body: &[u8], hash: u64) -> Option<Summary> {
    /// One string key, trimmed, stripped of control characters the way `tail` strips
    /// them, and capped. Empty for a key the model left out.
    fn field(object: &Value, key: &str, cap: usize) -> String {
        object
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .chars()
            // Same convention as `tail`: a control character is not content.
            .filter(|c| !c.is_control())
            .take(cap)
            .collect()
    }

    let response: Value = serde_json::from_slice(body).ok()?;
    let content = response.pointer("/choices/0/message/content")?.as_str()?;
    let object: Value = serde_json::from_str(first_json_object(content)?).ok()?;
    let text = field(&object, "summary", SUMMARY_TEXT_CHARS);
    if text.is_empty() {
        return None;
    }
    Some(Summary {
        text,
        // Not required: a summary without a title is still worth drawing, and only the
        // rename goes without.
        title: field(&object, "title", SUMMARY_TITLE_CHARS),
        // Absent means false: a model that forgot the key has not claimed the agent is
        // blocked, and a spurious flag is worse than a missing one.
        awaiting: object
            .get("awaiting")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        hash,
        // As `local_summary`: the daemon stamps it as it stores it.
        created_at_ms: None,
    })
}

/// Whether the running agent is waiting for an answer, shared by the strip and model tools.
///
/// Gated on the pane being idle as well as on the answer, because a summary outlives the
/// moment it describes: it is deliberately kept while the agent works again — that is what
/// lets an agent returning to the same screen be free rather than another request — and an
/// agent that is working is not waiting on anybody, whatever it was waiting on before.
fn awaiting_confirmation(
    summaries: &Summaries,
    activity: &Activity,
    hooks: &HookFeeds,
    status: AgentStatus,
    pane_id: u32,
) -> bool {
    if status != AgentStatus::Running {
        return false;
    }
    // An agent that says it is blocked on the user is blocked on the user. No
    // freshness check against `activity` here, unlike the summary path below: that
    // check exists because a summary describes one screen and outlives it, while a
    // feed has no screen behind it at all - it is the current state or it is nothing.
    if let Some(feed) = hooks.get(&pane_id) {
        return feed.phase == HookPhase::Waiting;
    }
    let idle = activity.get(&pane_id).is_some_and(|(_, polls)| *polls > 0);
    let current = activity_hash(activity, pane_id);
    let fresh = summaries
        .get(&pane_id)
        .is_some_and(|s| s.awaiting && Some(s.hash) == current);
    idle && fresh
}

/// What marks an agent whose state is *reported* rather than inferred.
///
/// Every other row on the strip is a guess made by reading a screen. A row carrying
/// this one is being told, by the agent itself, over its own hooks — so its `busy`,
/// its `waiting` and its summary mean something the others do not, and the difference
/// is worth one column. It rides on the kind, where `@host` already rides, because
/// what it qualifies is *which agent this is*: a claude with a feed, not a claude.
///
/// Deliberately not a character the `!` search in the agents pane could confuse for a
/// marker of its own — see `render_summary` there.
pub const HOOK_MARKER: &str = "◆";

/// The kind column of one agent row: what the agent is, marked with what is known
/// about it and where it runs.
///
/// One formatter, because the strip's rows and the summary box's header both name the
/// same agent and a user comparing the two must not find them spelled differently.
/// Order is `claude◆@remote`: the marker qualifies the kind, and the host stays the
/// outermost suffix so it reads the way a host is written everywhere else.
pub fn kind_cell(kind: &str, host: Option<&str>, hooked: bool) -> String {
    let mut cell = kind.to_owned();
    if hooked {
        cell.push_str(HOOK_MARKER);
    }
    if let Some(host) = host {
        cell.push('@');
        cell.push_str(host);
    }
    cell
}

/// What a header stands in for when there is nothing to name.
const NO_PROJECT: &str = "no project";
const NO_BRANCH: &str = "no branch";
/// How far a branch header sits in under the project header above it.
const BRANCH_INDENT: &str = "  ";

/// One line of the agent strip: an agent, or a header grouping the agents below.
///
/// A header is drawn and never selected, which is what lets the picker keep its
/// pane ids parallel to its lines while still showing structure.
///
/// Serialized: agent computes these rows (it holds the snapshot, the project
/// registry and the branch probes) and pipes them to the agents pane,
/// which only draws them - see `wire::AGENT_STRIP`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StripRow {
    /// The project the agents below belong to.
    Project(String),
    /// The branch they are checked out on, under the project header above it.
    Branch(String),
    /// One agent: the pane it names, the line as drawn, and what kind of agent it is.
    ///
    /// `kind` is not in `text` - the line already spells it, with the host appended - but the
    /// summary box's header wants it on its own, and classification is agent's job. Carrying it
    /// is cheaper than a second classifier in the pane that draws the box.
    Agent {
        pane_id: u32,
        text: String,
        #[serde(default)]
        kind: String,
        /// Current evidence that this agent is waiting for an answer. Never inferred from
        /// its title or from a completed turn; shared with the marker and `agent_list`.
        #[serde(default)]
        waiting: bool,
    },
    /// A completed hook session with no live pane to focus.
    ///
    /// Unlike an `Agent`, whose line is composed here, an archived run arrives as *cells*: the
    /// archive is drawn as a table, and only the pane knows how many columns it has to divide
    /// between them. Agent still decides what each cell says - it holds the run headers, the
    /// checkout of each and the paging - and the pane decides what fits.
    Run {
        run_id: String,
        #[serde(default)]
        kind: String,
        /// The prompt cell: the run's title, or `"{snippet} - {title}"` in search mode - the
        /// matching text first, so a cell too narrow for both still says why the row matched.
        #[serde(default)]
        title: String,
        /// The checkout the run worked in, as a leaf name, empty where there is none to name.
        #[serde(default)]
        worktree: String,
        /// When the run started, for the age column. Absent for a row that cannot say.
        #[serde(default)]
        started_at_ms: Option<i64>,
        /// This run continues an earlier one across a context clear, which the prompt cell
        /// marks with an arrow.
        ///
        /// A flag rather than a marker baked into the title: the kind has a column of its own
        /// now, so the old rule that elided a same-kind continuation's kind - the arrow already
        /// said it descended from the chain, and the daemon only links runs of a matching kind -
        /// has nothing left to elide.
        #[serde(default)]
        continuation: bool,
    },
    /// The archive is unlimited, so only an explicit sentinel asks agent for another page.
    MoreRuns,
}

/// The agent strip as the agents pane should draw it: the rows, and the
/// terminal pane the user is looking at.
///
/// The focus rides along with the rows rather than on its own message because
/// the panel needs both to answer either question: which row the cursor belongs
/// on takes the rows to say which pane ids are agents at all. It is the *raw*
/// focused pane — a shell or an editor, not only an agent — so the panel can
/// tell "you are on this agent" from "you were on this agent"; see
/// `wire::AGENT_STRIP`.
///
/// The summaries ride along too, for the box the agents pane draws under its list. Only the
/// ones naming a pane in `rows` are sent: a summary outlives the pane it is about, and the
/// list has no row to put an orphan under.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strip {
    pub rows: Vec<StripRow>,
    pub focused: Option<u32>,
    #[serde(default)]
    pub summaries: Summaries,
}

/// The agent strip: every agent as a line, under a project header and a branch
/// header.
///
/// One formatter for the agent picker and anything else that shows an agent as
/// a line, so they can never disagree. The tab name goes last and is clipped by
/// the renderer, not here.
///
/// `owners` gives the project of a pane and `branches` its checkout's branch.
/// Neither is repeated on the agent's own line: the headers carry them, so an
/// agent row stays short enough to read in a narrow sidebar. Agents of no known
/// project come last, under a header that says so.
/// One agent row with its project, branch, and pane id as sorting keys.
type SortableAgent<'a> = (Option<&'a str>, Option<&'a str>, u32, StripRow);

pub fn strip_rows(
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    activity: &Activity,
    owners: &BTreeMap<u32, String>,
    branches: &BTreeMap<u32, String>,
    summaries: &Summaries,
    hooks: &HookFeeds,
) -> Vec<StripRow> {
    let mut agents: Vec<SortableAgent<'_>> = panes
        .iter()
        .filter_map(|pane| {
            let (kind, _) = classify(pane, manual)?;
            // The host rides on the kind rather than in a column of its own:
            // it is absent for nearly every agent, and a column that is usually
            // blank costs width in a sidebar that has none to spare. `@host`
            // also reads the way a host is written everywhere else. The hook
            // marker rides there too — see `kind_cell`.
            let hooked = hooks.contains_key(&pane.pane_id);
            let cell = kind_cell(&kind, pane.host.as_deref(), hooked);
            let waiting =
                awaiting_confirmation(summaries, activity, hooks, pane.status, pane.pane_id);
            // A hooked pane that is waiting already says so through its feed. One without hooks
            // is only known to be waiting from its summary, and its status cell would read
            // `idle` beside the `!` - true of the screen, but not what the row is claiming.
            // Only here: `status_cell` is also the model's `state`, which stays what it was.
            let status = match waiting && !hooked {
                true => "waiting".to_owned(),
                false => status_cell(
                    pane.status,
                    activity.get(&pane.pane_id),
                    hooks.get(&pane.pane_id),
                ),
            };
            let mut row = format!(
                "{}{:>3}  {cell}  {status}",
                if waiting { "! " } else { "  " },
                pane.pane_id,
            );
            if let Some((_, name)) = &pane.tab {
                row.push_str("  ");
                row.push_str(name);
            }
            Some((
                owners.get(&pane.pane_id).map(String::as_str),
                branches.get(&pane.pane_id).map(String::as_str),
                pane.pane_id,
                StripRow::Agent {
                    pane_id: pane.pane_id,
                    text: row,
                    // The row already names the host; the history header only needs the kind.
                    kind: kind_cell(&kind, None, hooked),
                    waiting,
                },
            ))
        })
        .collect();
    agents.sort_by(|left, right| {
        // `None` sorts before `Some`, and the nameless go last on both keys, so
        // each is preceded by "has nothing to name".
        (left.0.is_none(), left.0, left.1.is_none(), left.1, left.2).cmp(&(
            right.0.is_none(),
            right.0,
            right.1.is_none(),
            right.1,
            right.2,
        ))
    });

    // `None` is "no header written yet", which is not the same as a header for
    // an agent with no project — that one is written, and says so.
    let mut project: Option<Option<&str>> = None;
    let mut branch: Option<Option<&str>> = None;
    let mut rows = Vec::with_capacity(agents.len());
    for (owner, checkout, _, row) in agents {
        if project != Some(owner) {
            rows.push(StripRow::Project(owner.unwrap_or(NO_PROJECT).to_owned()));
            project = Some(owner);
            // A branch header belongs to the project above it, so the next
            // agent needs one even when it is on the branch just written.
            branch = None;
        }
        if branch != Some(checkout) {
            rows.push(StripRow::Branch(format!(
                "{BRANCH_INDENT}{}",
                checkout.unwrap_or(NO_BRANCH)
            )));
            branch = Some(checkout);
        }
        rows.push(row);
    }
    rows
}

/// The project each tab agrees on.
///
/// One entry for each tab whose agents all name the same project. An agent with
/// no known project is ignored, not a veto. A tab with no agent, and a global
/// pane, which is in no tab, get no entry.
pub fn tab_projects(
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    owners: &BTreeMap<u32, String>,
) -> BTreeMap<TabId, String> {
    // `None` is a tab that two projects disagree over. It stays in the map, so a
    // later agent of one project cannot make the tab agree again.
    let mut found: BTreeMap<TabId, Option<String>> = BTreeMap::new();
    for pane in panes {
        if classify(pane, manual).is_none() {
            continue;
        }
        let (Some((tab_id, _)), Some(owner)) = (&pane.tab, owners.get(&pane.pane_id)) else {
            continue;
        };
        found
            .entry(*tab_id)
            .and_modify(|agreed| {
                if agreed.as_deref() != Some(owner.as_str()) {
                    *agreed = None;
                }
            })
            .or_insert_with(|| Some(owner.clone()));
    }
    found
        .into_iter()
        .filter_map(|(tab_id, agreed)| Some((tab_id, agreed?)))
        .collect()
}

/// Every project/branch pair a tab's panes are working on, sorted without repeats.
///
/// The other half of [`tab_projects`], and it differs from that one twice over.
/// That one asks which project *owns* a tab and refuses to answer for a tab two
/// projects share, because an assignment has to name one project. Naming the tab
/// does not — a tab with a pane in each of two checkouts is honestly described
/// by both, and saying so is more use than falling back to "Tab #4".
///
/// And it counts *every* pane, where `tab_projects` counts agents. Ownership is
/// about which project's work a tab is doing, and only an agent does work. A
/// name is about where the tab is, and a shell sitting in a checkout answers
/// that as well as an agent does — better, in the ordinary session that has no
/// agents in it at all. That is why no `manual` overrides are needed here: they
/// exist to say which panes are agents, and this no longer asks.
///
/// Sorted so the label does not shuffle when the panes come back in another
/// order. A tab whose panes have no known project gets no entry, as before.
pub fn tab_project_labels<'a>(
    panes: impl IntoIterator<Item = &'a AgentPane>,
    owners: &BTreeMap<u32, (String, Option<String>)>,
) -> BTreeMap<TabId, Vec<(String, Option<String>)>> {
    let mut found: BTreeMap<TabId, BTreeSet<(String, Option<String>)>> = BTreeMap::new();
    for pane in panes {
        let (Some((tab_id, _)), Some(owner)) = (&pane.tab, owners.get(&pane.pane_id)) else {
            continue;
        };
        found.entry(*tab_id).or_default().insert(owner.clone());
    }
    found
        .into_iter()
        .map(|(tab_id, projects)| (tab_id, projects.into_iter().collect()))
        .collect()
}

/// Format sorted, unique project/branch pairs within the tab bar's 40 columns.
/// Repeated project prefixes are omitted; entries that do not fit are counted.
pub fn tab_project_label(projects: &[(String, Option<String>)]) -> Option<String> {
    let mut named = String::new();
    let mut best = None;
    for (index, (project, branch)) in projects.iter().enumerate() {
        if index > 0 {
            named.push_str(" + ");
        }
        match branch {
            Some(branch) if index > 0 && projects[index - 1].0 == *project => {
                named.push_str(branch);
            },
            Some(branch) => named.push_str(&format!("{project}:{branch}")),
            None => named.push_str(project),
        }
        let remaining = projects.len() - index - 1;
        let suffix = if remaining == 0 {
            String::new()
        } else {
            format!(" +{remaining}")
        };
        if named.width() + suffix.width() <= 40 {
            best = Some(format!("{named}{suffix}"));
        } else if index == 0 {
            best = Some(format!(
                "{}{suffix}",
                yinz_tile_utils::ellipsize(&named, 40 - suffix.width())
            ));
        }
        if named.width() > 40 {
            break;
        }
    }
    best
}

/// The `@`-mention bodies in a message: the maximal leading run of kind characters
/// after each `@`, lowercased. `@codex,` mentions codex — punctuation ends a
/// mention rather than voiding it. `a@b` and a bare `@` mention nothing.
fn mentions(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|token| token.strip_prefix('@'))
        .map(|body| {
            body.chars()
                .map(|c| c.to_ascii_lowercase())
                .take_while(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-'
                })
                .collect::<String>()
        })
        .filter(|body| !body.is_empty())
        .collect()
}

/// The context block a message's `@`-mentions earn, or `None` when none resolve.
///
/// One line per resolved agent, e.g. `[@codex is agent pane 3 (codex, busy) on tab
/// "work"]`. An all-digit mention names a pane id, anything else a kind — which may
/// match several agents, and earns a line for each. A mention that resolves to
/// nothing says nothing: the model has `agent_list` for what the snapshot does not
/// know.
pub fn mention_context(
    text: &str,
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    activity: &Activity,
    hooks: &HookFeeds,
) -> Option<String> {
    // (mention as typed, pane id), deduped by pane, first-appearance order.
    let mut found: Vec<(String, u32)> = Vec::new();
    let mut claim = |body: &str, pane_id: u32| {
        if !found.iter().any(|(_, id)| *id == pane_id) {
            found.push((body.to_owned(), pane_id));
        }
    };
    for body in mentions(text) {
        if let Ok(id) = body.parse::<u32>() {
            if panes
                .iter()
                .any(|pane| pane.pane_id == id && classify(pane, manual).is_some())
            {
                claim(&body, id);
            }
        } else {
            for pane in panes {
                if classify(pane, manual).is_some_and(|(kind, _)| kind == body) {
                    claim(&body, pane.pane_id);
                }
            }
        }
    }

    let lines: Vec<String> = found
        .iter()
        .filter_map(|(body, id)| {
            let pane = panes.iter().find(|pane| pane.pane_id == *id)?;
            let (kind, _) = classify(pane, manual)?;
            let mut line = format!(
                "[@{body} is agent pane {id} ({kind}, {})",
                status_cell(pane.status, activity.get(id), hooks.get(id))
            );
            if let Some((_, name)) = &pane.tab {
                line.push_str(&format!(" on tab \"{name}\""));
            }
            line.push(']');
            Some(line)
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// What Tab may complete after `@`: each agent kind once, then each agent's pane id.
pub fn mention_candidates(panes: &[AgentPane], manual: &BTreeMap<u32, String>) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for pane in panes {
        if let Some((kind, _)) = classify(pane, manual) {
            if !candidates.iter().any(|candidate| candidate == &kind) {
                candidates.push(kind.to_owned());
            }
        }
    }
    for pane in panes {
        if classify(pane, manual).is_some() {
            candidates.push(pane.pane_id.to_string());
        }
    }
    candidates
}

/// The tail of what a pane is showing.
///
/// Trailing blank lines go first: a viewport read is mostly empty below the prompt, and
/// twenty lines of nothing would tell the model nothing. Control characters are dropped
/// rather than escaped — they are cursor moves it cannot act on, and every one of them
/// is paid for on every subsequent turn.
pub fn preview(text: &str) -> String {
    tail(text, PREVIEW_LINES, PREVIEW_CHARS)
}

/// The last `lines` lines of `text`, capped at `chars`.
///
/// The body of [`preview`], taken apart when the idle summary wanted a bigger cut of the
/// same pane under the same rules — one place that knows a viewport read is mostly empty
/// below the prompt, and that a control character is a cursor move nobody downstream can
/// act on.
pub fn tail(text: &str, lines: usize, chars: usize) -> String {
    let mut kept: Vec<&str> = text.lines().collect();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    kept[kept.len().saturating_sub(lines)..]
        .join("\n")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(chars)
        .collect()
}

/// Every agent among `panes`, as the compact JSON array `agent_list` answers with.
///
/// `read` is injected rather than called: reading a pane is a host call, and this
/// module has to stay testable on the host triple. It runs one time for each agent,
/// and not at all for a pane that is not one.
pub fn list_json(
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    activity: &Activity,
    owners: &BTreeMap<u32, String>,
    branches: &BTreeMap<u32, String>,
    summaries: &Summaries,
    hooks: &HookFeeds,
    mut read: impl FnMut(u32) -> String,
) -> String {
    let items: Vec<Value> = panes
        .iter()
        .filter_map(|pane| {
            let (kind, source) = classify(pane, manual)?;
            let mut item = json!({
                "pane_id": pane.pane_id,
                "kind": kind,
                "source": source,
                "status": pane.status.as_str(),
                "title": pane.title,
                "preview": preview(&read(pane.pane_id)),
            });
            // The exact number the picker rounds to minutes. Only for a running
            // agent the poll has observed; absent means "no idea yet".
            if pane.status == AgentStatus::Running {
                if let Some((_, polls)) = activity.get(&pane.pane_id) {
                    item["idle_seconds"] = json!(u64::from(*polls) * POLL_SECONDS);
                }
            }
            if let Some((tab_id, name)) = &pane.tab {
                item["tab_id"] = json!(tab_id);
                item["tab"] = json!(name);
            }
            // The folder and the project the poll found for it. Absent means the
            // host did not report a folder for this pane.
            if let Some(cwd) = &pane.cwd {
                item["cwd"] = json!(cwd);
            }
            if let Some(host) = &pane.host {
                item["host"] = json!(host);
            }
            if let Some(owner) = owners.get(&pane.pane_id) {
                item["project"] = json!(owner);
            }
            // The branch of the checkout the agent sits in, which the picker
            // groups by. Absent on a folder Git does not claim and on a
            // detached HEAD.
            if let Some(branch) = branches.get(&pane.pane_id) {
                item["branch"] = json!(branch);
            }
            // What the daemon last made of this agent's pane, so the model sees the same
            // thing the strip draws. Absent until an agent has gone idle at least once.
            if let Some(summary) = summaries.get(&pane.pane_id) {
                item["summary"] = json!(summary.text);
            }
            // Answered exactly as the strip's marker is, so the model and the list
            // cannot disagree about who is blocked. See `awaiting_confirmation`: an agent
            // reporting for itself is believed outright, and only a pane without a
            // feed is read off a summary — where the answer is gated on that summary
            // still describing the screen the pane is showing.
            //
            // The feed answers whether or not a summary exists, which the summary path
            // cannot: a pane can be blocked on the user before anything has been
            // written about it, and the model asking for the list needs to know that.
            if let Some(feed) = hooks.get(&pane.pane_id) {
                item["awaiting_confirmation"] = json!(awaiting_confirmation(
                    summaries,
                    activity,
                    hooks,
                    pane.status,
                    pane.pane_id
                ));
                // The fact the strip spells `◆`. A reader of this list wants to know
                // that the two fields below are reported rather than inferred; it has
                // no use for the glyph that says so in a column.
                item["hooks"] = json!(true);
                if let Some(waiting_on) = &feed.waiting_on {
                    item["waiting_on"] = json!(waiting_on);
                }
                // The label the strip draws, spelled out for a reader that has one
                // agent's row rather than the whole column: `idle_seconds` above is
                // the screen's clock, and an agent that reports for itself is not
                // described by it. Absent for a pane without a feed, whose readers
                // go on deriving the label from the seconds as they always have.
                item["state"] = json!(status_cell(
                    pane.status,
                    activity.get(&pane.pane_id),
                    Some(feed)
                ));
            } else if summaries.contains_key(&pane.pane_id) {
                item["awaiting_confirmation"] = json!(awaiting_confirmation(
                    summaries,
                    activity,
                    hooks,
                    pane.status,
                    pane.pane_id
                ));
            }
            Some(item)
        })
        .collect();
    Value::Array(items).to_string()
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine most of these tests are on. Named, rather than empty, so that a helper which
    /// forgot to write `host_id` fails a test instead of matching the pre-migration rows.
    const HOST: &str = "host-a";

    /// An owner on [`HOST`]. The cross-machine cases spell `Owner::new` out.
    fn owner(yinz_session_name: &str) -> rows::Owner {
        rows::Owner::new(HostId::new(HOST), yinz_session_name.into())
    }

    #[test]
    fn a_registered_record_names_the_kind_and_nothing_it_does_not_know() {
        let record = registered_agent("reviewer", Some("/work".to_owned()));
        assert_eq!(record.kind, "reviewer");
        assert_eq!(record.source, AgentSource::Registered);
        assert_eq!(record.state, AgentState::Idle);
        assert_eq!(record.cwd.as_deref(), Some("/work"));
        assert!(record.detail.is_none() && record.session_id.is_none());
        assert!(
            HookFeed::from_agent(&record).is_none(),
            "a person's word about the kind is not a hook feed"
        );
    }

    /// A feed read off a record clears `waiting_on` on every phase that is not a wait, the
    /// way a feed built from events does. A row that says `idle` beside a question, or a
    /// `list_json` item carrying `"state":"idle"` and a `waiting_on` together, is a record
    /// disagreeing with itself in front of whoever reads it.
    #[test]
    fn a_feed_off_a_record_only_waits_on_something_while_it_is_waiting() {
        let record = |state, waiting_on: Option<&str>| AgentInfo {
            kind: "claude".to_owned(),
            state,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: waiting_on.map(str::to_owned),
            wait_needs_a_person: state == AgentState::Waiting,
            cwd: None,
            session_id: None,
            run_id: None,
        };

        let waiting = HookFeed::from_agent(&record(AgentState::Waiting, Some("Bash rm")))
            .expect("a wait is a feed");
        assert_eq!(waiting.waiting_on.as_deref(), Some("Bash rm"));

        let idle = HookFeed::from_agent(&record(AgentState::Idle, Some("Bash rm")))
            .expect("an idle agent is a feed too");
        assert_eq!(idle.phase, HookPhase::Idle);
        assert_eq!(idle.waiting_on, None);
    }

    fn pane(pane_id: u32, title: &str, command: Option<&str>) -> AgentPane {
        AgentPane {
            pane_id,
            title: title.to_owned(),
            command: command.map(str::to_owned),
            cwd: None,
            host: None,
            bubblewrap: None,
            status: AgentStatus::Running,
            tab: None,
            detected: None,
            agent: None,
        }
    }

    /// A pane that has been seen running `kind` and is now running something else —
    /// an agent that shelled out.
    fn latched(pane_id: u32, kind: &str, command: &str) -> AgentPane {
        AgentPane {
            detected: Some(kind.to_owned()),
            ..pane(pane_id, "zsh", Some(command))
        }
    }

    fn manual(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
        pairs
            .iter()
            .map(|(id, kind)| (*id, (*kind).to_owned()))
            .collect()
    }

    fn owners(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
        manual(pairs)
    }

    fn branches(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
        manual(pairs)
    }

    fn project(name: &str) -> StripRow {
        StripRow::Project(name.to_owned())
    }

    fn branch(name: &str) -> StripRow {
        StripRow::Branch(format!("  {name}"))
    }

    /// An agent row with nothing waiting on the user - the blank marker column every row
    /// carries, spelled once here rather than as two leading spaces in every expectation.
    fn agent(pane_id: u32, text: &str) -> StripRow {
        StripRow::Agent {
            pane_id,
            kind: kind_in(text),
            text: format!("  {text}"),
            waiting: false,
        }
    }

    /// The kind `strip_rows` would have put on the row, read back off the line it drew.
    /// The line spells it as the second field, with `@host` appended for a remote pane;
    /// the field carries the bare kind.
    fn kind_in(text: &str) -> String {
        let kind = text.split_whitespace().nth(1).unwrap_or_default();
        kind.split('@').next().unwrap_or_default().to_owned()
    }

    /// An agent row the model said is blocked on a confirmation.
    fn waiting(pane_id: u32, text: &str) -> StripRow {
        StripRow::Agent {
            pane_id,
            kind: kind_in(text),
            text: format!("! {text}"),
            waiting: true,
        }
    }

    /// A session where nothing reports for itself.
    fn no_hooks() -> HookFeeds {
        HookFeeds::new()
    }

    /// One pane's feed, in the phase named.
    fn feeds(pairs: &[(u32, HookPhase)]) -> HookFeeds {
        pairs
            .iter()
            .map(|(pane_id, phase)| {
                (
                    *pane_id,
                    HookFeed {
                        phase: *phase,
                        detail: None,
                        waiting_on: None,
                        session_id: "s1".to_owned(),
                        cwd: None,
                    },
                )
            })
            .collect()
    }

    fn summaries(pairs: &[(u32, bool)]) -> Summaries {
        pairs
            .iter()
            .map(|(pane_id, awaiting)| {
                (
                    *pane_id,
                    Summary {
                        text: "did a thing".to_owned(),
                        title: "a thing".to_owned(),
                        awaiting: *awaiting,
                        hash: 1,
                        created_at_ms: None,
                    },
                )
            })
            .collect()
    }

    /// The case that made the dash rule earn its place: an installer that puts a
    /// versioned binary on the path, which no alias table could have anticipated.
    #[test]
    fn detects_a_versioned_binary_name() {
        assert_eq!(
            detect_kind(
                "zsh",
                Some("/Users/x/.local/bin/muse-bin-0.1.0-R708.1 --resume"),
            )
            .as_deref(),
            Some("muse"),
        );
    }

    #[test]
    fn detects_exact_command_aliases() {
        for (command, kind) in [
            ("muse", "muse"),
            ("muse-cli", "muse"),
            ("codex", "codex"),
            ("codex-cli", "codex"),
            ("claude", "claude"),
            ("claude-code", "claude"),
            ("cursor", "cursor"),
            ("cursor-agent", "cursor"),
            ("qwen", "qwen"),
            ("qwen-code", "qwen"),
            ("gemini", "gemini"),
            ("gemini-cli", "gemini"),
            ("aider", "aider"),
            ("cline", "cline"),
            ("pi", "pi"),
            ("pi-agent", "pi"),
        ] {
            assert_eq!(
                detect_kind("shell", Some(command)).as_deref(),
                Some(kind),
                "{command}"
            );
        }
    }

    #[test]
    fn detects_the_program_behind_a_path_and_its_arguments() {
        assert_eq!(
            detect_kind("shell", Some("/usr/local/bin/Codex --resume")).as_deref(),
            Some("codex"),
        );
    }

    #[test]
    fn detects_a_kind_named_as_a_whole_word_in_the_title() {
        assert_eq!(
            detect_kind("aider — src/main.rs", None).as_deref(),
            Some("aider")
        );
        assert_eq!(detect_kind("CLAUDE", None).as_deref(), Some("claude"));
    }

    /// The whole reason detection is token-based rather than substring-based.
    #[test]
    fn does_not_detect_a_kind_inside_a_longer_word() {
        assert_eq!(detect_kind("pip install", Some("zsh")).as_deref(), None);
        assert_eq!(detect_kind("piano", None).as_deref(), None);
        assert_eq!(detect_kind("shell", Some("pipenv run pi")).as_deref(), None);
    }

    #[test]
    fn detects_an_agent_behind_the_interpreter_that_runs_it() {
        assert_eq!(
            detect_kind("shell", Some("node /opt/bin/gemini")).as_deref(),
            Some("gemini"),
        );
        assert_eq!(
            detect_kind("shell", Some("python3 -m aider")).as_deref(),
            Some("aider")
        );
        assert_eq!(
            detect_kind("shell", Some("node /opt/lib/claude.js")).as_deref(),
            Some("claude"),
        );
    }

    /// The guard that keeps the interpreter case from swallowing every command line
    /// that happens to say an agent's name.
    #[test]
    fn does_not_look_past_the_first_word_of_anything_else() {
        assert_eq!(
            detect_kind("shell", Some("git commit -m ask-claude")).as_deref(),
            None
        );
        assert_eq!(detect_kind("shell", Some("zsh")).as_deref(), None);
        assert_eq!(detect_kind("shell", Some("man codex")).as_deref(), None);
    }

    #[test]
    fn rejects_a_kind_that_is_empty_too_long_or_not_a_plain_word() {
        assert!(valid_kind("reviewer"));
        assert!(valid_kind("agent_2-b"));
        assert!(!valid_kind(""));
        assert!(!valid_kind(&"a".repeat(33)));
        assert!(!valid_kind("Reviewer"));
        assert!(!valid_kind("code review"));
    }

    #[test]
    fn a_manual_kind_wins_over_detection_and_survives_a_title_change() {
        let manual = manual(&[(3, "reviewer")]);
        assert_eq!(
            classify(&pane(3, "codex", Some("codex")), &manual),
            Some(("reviewer".to_owned(), "manual")),
        );
        assert_eq!(
            classify(&pane(3, "new title", Some("codex")), &manual),
            Some(("reviewer".to_owned(), "manual")),
        );
        assert_eq!(
            classify(&pane(4, "codex", Some("codex")), &manual),
            Some(("codex".to_owned(), "auto")),
        );
        assert_eq!(classify(&pane(5, "zsh", Some("zsh")), &manual), None);
    }

    /// The flicker the latch exists for: an agent that shells out is still an
    /// agent, and the pane must not leave every list until the tool it ran
    /// finishes.
    #[test]
    fn a_pane_that_shelled_out_keeps_the_kind_it_latched() {
        let manual = manual(&[]);
        assert_eq!(
            classify(&latched(3, "codex", "git status"), &manual),
            Some(("codex".to_owned(), "auto")),
        );
        // Even when what it is running is a shell of its own.
        assert_eq!(
            classify(&latched(3, "codex", "bash -c rg"), &manual),
            Some(("codex".to_owned(), "auto")),
        );
    }

    /// What a pane is doing now outranks what it was doing: a pane that has
    /// become a different agent is reported as the one it is, and a manual kind
    /// still beats both.
    #[test]
    fn the_latch_is_the_last_word_and_not_the_first() {
        let running = AgentPane {
            detected: Some("codex".to_owned()),
            ..pane(3, "zsh", Some("claude"))
        };
        assert_eq!(
            classify(&running, &manual(&[])),
            Some(("claude".to_owned(), "auto"))
        );
        assert_eq!(
            classify(&running, &manual(&[(3, "reviewer")])),
            Some(("reviewer".to_owned(), "manual")),
        );
    }

    /// Nothing rewrites a pane title when the program that set it exits, so a
    /// pane that ran an agent and went back to its prompt used to stay an agent
    /// for as long as it stayed open.
    #[test]
    fn a_shell_prompt_outranks_the_title_the_last_program_left() {
        assert_eq!(detect_kind("claude", Some("zsh")).as_deref(), None);
        assert_eq!(
            detect_kind("claude", Some("/usr/bin/zsh -l")).as_deref(),
            None
        );
        assert_eq!(detect_kind("claude", Some("-zsh")).as_deref(), None);
        // Still an agent while it is actually running one, and still detected by
        // title when nothing has said what it runs.
        assert_eq!(
            detect_kind("claude", Some("claude")).as_deref(),
            Some("claude")
        );
        assert_eq!(detect_kind("claude", None).as_deref(), Some("claude"));
        // A command that is neither a shell nor an agent leaves the title alone:
        // a wrapper we do not recognise must not hide a real agent.
        assert_eq!(
            detect_kind("claude", Some("mise exec")).as_deref(),
            Some("claude")
        );
    }

    /// opencode is detected like every other kind, which is what puts a pane a person
    /// started by hand into the agents strip. From its command first - a path or a
    /// bare binary, flags and all - then from its title when nothing has said what it runs,
    /// and the latch survives a shell-out the way any other agent's does.
    #[test]
    fn an_opencode_pane_is_detected_from_its_command_and_its_title() {
        yinz_types::agent::provider::register(
            0,
            yinz_types::agent::provider::AgentProvider {
                kind: "opencode".into(),
                executable: "opencode".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            detect_kind("shell", Some("/home/u/.opencode/bin/opencode --prompt fix")).as_deref(),
            Some("opencode"),
        );
        assert_eq!(
            classify(
                &pane(3, "shell", Some("opencode --auto --prompt go")),
                &manual(&[]),
            ),
            Some(("opencode".to_owned(), "auto")),
        );
        assert_eq!(
            classify(&pane(4, "opencode — src/main.rs", None), &manual(&[])),
            Some(("opencode".to_owned(), "auto")),
        );
        assert_eq!(
            classify(&latched(5, "opencode", "git status"), &manual(&[])),
            Some(("opencode".to_owned(), "auto")),
        );
        // Back at a prompt, the title it left behind is outlived like any other agent's.
        assert_eq!(detect_kind("opencode", Some("zsh")).as_deref(), None);
    }

    #[test]
    fn is_shell_reads_whole_program_names_only() {
        assert!(is_shell("zsh"));
        assert!(is_shell("/bin/bash --login"));
        assert!(is_shell("-zsh"));
        assert!(!is_shell("shellcheck"));
        assert!(!is_shell("codex"));
        assert!(!is_shell(""));
    }

    #[test]
    fn preview_keeps_the_last_lines_without_the_trailing_blanks() {
        let text = (1..=30).fold(String::new(), |mut all, line| {
            all.push_str(&format!("line {line}\n"));
            all
        }) + "\n\n\n";
        let preview = preview(&text);
        assert!(preview.starts_with("line 11\n"));
        assert!(preview.ends_with("line 30"));
        assert_eq!(preview.lines().count(), PREVIEW_LINES);
    }

    /// Same convention as `ui::wrap`: the control character goes, whatever followed it
    /// stays. A pane read is already rendered text, so this is a belt-and-braces pass.
    #[test]
    fn preview_drops_control_characters_and_stops_at_the_ceiling() {
        assert_eq!(preview("re\u{7}ad\ty"), "ready");
        assert!(!preview("\x1b[0mready").contains('\x1b'));
        assert_eq!(preview(&"x".repeat(900)).len(), PREVIEW_CHARS);
    }

    #[test]
    fn observe_counts_unchanged_polls_and_resets_on_new_content() {
        let mut activity = Activity::new();
        observe(&mut activity, 3, Sample::Text("working"));
        assert_eq!(activity[&3].1, 0);
        observe(&mut activity, 3, Sample::Text("working"));
        observe(&mut activity, 3, Sample::Text("working"));
        assert_eq!(activity[&3].1, 2);
        observe(&mut activity, 3, Sample::Text("done"));
        assert_eq!(activity[&3].1, 0);
    }

    /// What the daemon debounces on. Each poll of a stretch of stillness reports its own
    /// count, so a caller can pick the length of stillness it acts at and hit it exactly
    /// once — and a new screen restarts the count from one.
    #[test]
    fn observe_counts_the_polls_of_each_stretch_of_stillness() {
        let mut activity = Activity::new();
        // A first sighting cannot tell "working" from "sitting still", so it is not idle.
        assert_eq!(
            observe(&mut activity, 3, Sample::Text("working")),
            Observed::Busy
        );
        for polls in 1..=4 {
            assert_eq!(
                observe(&mut activity, 3, Sample::Text("working")),
                Observed::Idle { polls }
            );
        }

        // Back to work, then still again: a second stretch counts from the start.
        assert_eq!(
            observe(&mut activity, 3, Sample::Text("more")),
            Observed::Busy
        );
        assert_eq!(
            observe(&mut activity, 3, Sample::Text("more")),
            Observed::Idle { polls: 1 }
        );
    }

    /// The whole point of hashing words rather than text: a pane resized to a new width
    /// holds the same session, and used to look like a screenful of new work.
    #[test]
    fn rewrapping_the_same_output_is_not_a_change() {
        let narrow = "the agent finished\nthe refactor and is\nwaiting for you";
        let wide = "the agent finished the refactor\nand is waiting for you";
        assert_eq!(fingerprint(narrow), fingerprint(wide));

        let mut activity = Activity::new();
        assert_eq!(
            observe(&mut activity, 3, Sample::Text(narrow)),
            Observed::Busy
        );
        assert_eq!(
            observe(&mut activity, 3, Sample::Text(wide)),
            Observed::Idle { polls: 1 },
            "a rewrap is not the agent doing something"
        );
    }

    /// The other side of it: normalising must not go so far that a working agent looks
    /// still. Anything with a digit or a letter in it counts.
    #[test]
    fn a_ticking_counter_is_still_a_change() {
        let mut activity = Activity::new();
        observe(&mut activity, 3, Sample::Text("thinking (12s)"));
        assert_eq!(
            observe(&mut activity, 3, Sample::Text("thinking (13s)")),
            Observed::Busy
        );
    }

    /// Spinner frames and rules are not the session changing.
    #[test]
    fn punctuation_only_tokens_are_not_content() {
        assert_eq!(
            fingerprint("* working on it\n----------"),
            fingerprint("- working on it\n===")
        );
    }

    /// A reading that says nothing must neither restart the stretch of stillness nor
    /// disturb the hash an in-flight summary is stamped against.
    #[test]
    fn a_sample_we_cannot_use_keeps_the_hash_and_counts_the_pause() {
        let mut activity = Activity::new();
        observe(&mut activity, 3, Sample::Text("waiting for you"));
        let hash = activity_hash(&activity, 3).expect("recorded");
        assert_eq!(
            observe(&mut activity, 3, Sample::Unusable),
            Observed::Idle { polls: 1 }
        );
        assert_eq!(
            observe(&mut activity, 3, Sample::Unusable),
            Observed::Idle { polls: 2 }
        );
        assert_eq!(activity_hash(&activity, 3), Some(hash));
    }

    /// Before any usable reading there is nothing to be still about.
    #[test]
    fn the_first_reading_of_a_pane_cannot_be_an_unusable_one() {
        let mut activity = Activity::new();
        assert_eq!(observe(&mut activity, 3, Sample::Unusable), Observed::Busy);
        assert_eq!(activity_hash(&activity, 3), None);
    }

    #[test]
    fn activity_hash_is_the_hash_the_poll_recorded() {
        let mut activity = Activity::new();
        assert_eq!(activity_hash(&activity, 3), None);
        observe(&mut activity, 3, Sample::Text("working"));
        let first = activity_hash(&activity, 3).expect("recorded");
        observe(&mut activity, 3, Sample::Text("working"));
        assert_eq!(
            activity_hash(&activity, 3),
            Some(first),
            "same text, same hash"
        );
        observe(&mut activity, 3, Sample::Text("done"));
        assert_ne!(activity_hash(&activity, 3), Some(first));
    }

    /// One trimmer for the preview and the summary, so they cannot disagree about what a
    /// pane's tail is.
    #[test]
    fn tail_is_the_body_preview_is_built_from() {
        let text = (1..=200).fold(String::new(), |mut all, line| {
            all.push_str(&format!("line {line}\n"));
            all
        });
        assert_eq!(preview(&text), tail(&text, PREVIEW_LINES, PREVIEW_CHARS));

        let big = tail(&text, SUMMARY_LINES, SUMMARY_CHARS);
        assert_eq!(big.lines().count(), SUMMARY_LINES);
        assert!(big.starts_with("line 81\n"), "{big:.20}");
        assert!(big.ends_with("line 200"));
    }

    #[test]
    fn only_a_pane_with_something_on_it_is_worth_asking_about() {
        assert!(!worth_summarizing(""));
        assert!(!worth_summarizing("   \n \n "));
        assert!(!worth_summarizing("$ "));
        assert!(worth_summarizing(&"x".repeat(SUMMARY_MIN_CHARS)));
    }

    /// The request the daemon sends. `stream` must be false - the daemon reads a buffered
    /// body and has no SSE parser - and neither `temperature` nor `max_tokens` may appear,
    /// since providers reject them inconsistently.
    #[test]
    fn a_summary_request_is_buffered_and_carries_no_contested_knobs() {
        let body = summary_request_body(
            "gpt-4o-mini",
            "claude",
            Some("/work/agent"),
            Some("topic"),
            "waiting for you",
        );
        let value: Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(value["model"], "gpt-4o-mini");
        assert_eq!(value["stream"], false);
        assert!(value.get("temperature").is_none());
        assert!(value.get("max_tokens").is_none());
        assert_eq!(value["messages"][0]["role"], "system");
        let user = value["messages"][1]["content"]
            .as_str()
            .expect("a user turn");
        assert!(user.contains("Agent: claude."), "{user}");
        assert!(user.contains("Folder: /work/agent."), "{user}");
        assert!(user.contains("Branch: topic."), "{user}");
        assert!(user.contains("waiting for you"), "{user}");

        // A pane with no folder simply says less, rather than saying "None".
        let bare = summary_request_body("m", "codex", None, None, "hi");
        let value: Value = serde_json::from_slice(&bare).expect("valid json");
        let user = value["messages"][1]["content"].as_str().unwrap();
        assert!(!user.contains("Folder"), "{user}");
        assert!(!user.contains("Branch"), "{user}");
    }

    /// A completion whose content is `content`, as the provider would frame it.
    fn completion(content: &str) -> Vec<u8> {
        json!({ "choices": [{ "message": { "content": content } }] })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn parse_summary_reads_a_clean_object() {
        let summary = parse_summary(
            &completion(r#"{"summary":"Ran the tests, all green.","awaiting":false}"#),
            7,
        )
        .expect("parsed");
        assert_eq!(summary.text, "Ran the tests, all green.");
        assert!(!summary.awaiting);
        assert_eq!(summary.hash, 7, "stamped with the pane it was made from");
    }

    /// Every provider decorates differently, and a model told "no markdown" still
    /// sometimes writes a fence or a sentence of preamble.
    #[test]
    fn parse_summary_digs_the_object_out_of_whatever_wraps_it() {
        for content in [
            "```json\n{\"summary\":\"Waiting.\",\"awaiting\":true}\n```",
            "Sure! {\"summary\":\"Waiting.\",\"awaiting\":true} Hope that helps.",
            "```\n{\"summary\": \"Waiting.\", \"awaiting\": true}\n```",
        ] {
            let summary = parse_summary(&completion(content), 1).expect(content);
            assert_eq!(summary.text, "Waiting.", "{content}");
            assert!(summary.awaiting, "{content}");
        }
    }

    /// A `}` inside the summary text must not end the object early, or the parse fails on
    /// exactly the answers that talk about code.
    #[test]
    fn parse_summary_survives_braces_inside_the_summary() {
        let summary = parse_summary(
            &completion(r#"{"summary":"Closed the `if {` block \" here","awaiting":false}"#),
            1,
        )
        .expect("parsed");
        assert_eq!(summary.text, r#"Closed the `if {` block " here"#);
    }

    /// Absent means false: a model that forgot the key has not claimed the agent is
    /// blocked, and a spurious `!` is worse than a missing one.
    #[test]
    fn a_missing_awaiting_is_not_a_claim_that_the_agent_is_blocked() {
        let summary = parse_summary(&completion(r#"{"summary":"Done."}"#), 1).expect("parsed");
        assert!(!summary.awaiting);
        // Nor is a non-boolean one.
        let summary = parse_summary(&completion(r#"{"summary":"Done.","awaiting":"yes"}"#), 1)
            .expect("parsed");
        assert!(!summary.awaiting);
    }

    #[test]
    fn parse_summary_refuses_what_it_cannot_use() {
        assert!(parse_summary(b"not json at all", 1).is_none());
        assert!(parse_summary(&completion("no object here"), 1).is_none());
        assert!(parse_summary(&completion(r#"{"awaiting":true}"#), 1).is_none());
        assert!(parse_summary(&completion(r#"{"summary":"   "}"#), 1).is_none());
        assert!(parse_summary(&completion(r#"{"summary":42}"#), 1).is_none());
        // An unterminated object is not an object.
        assert!(parse_summary(&completion(r#"{"summary":"hi""#), 1).is_none());
        // A well-formed error response has no content to read.
        assert!(parse_summary(br#"{"error":{"message":"nope"}}"#, 1).is_none());
    }

    #[test]
    fn parse_summary_reads_the_title_the_pane_is_named_after() {
        let summary = parse_summary(
            &completion(
                r#"{"summary":"Ran the tests.","awaiting":false,"title":"running the tests"}"#,
            ),
            1,
        )
        .expect("parsed");
        assert_eq!(summary.title, "running the tests");
        assert_eq!(rename_title(&summary), Some("running the tests"));
    }

    /// A title is a nicety, a summary is the point. A model that answered only the two keys
    /// it used to be asked for still gets its summary drawn - it simply does not rename the
    /// pane.
    #[test]
    fn a_summary_without_a_title_is_still_a_summary() {
        for content in [
            r#"{"summary":"Done.","awaiting":false}"#,
            r#"{"summary":"Done.","title":"   "}"#,
            r#"{"summary":"Done.","title":42}"#,
        ] {
            let summary = parse_summary(&completion(content), 1).expect(content);
            assert_eq!(summary.text, "Done.", "{content}");
            assert_eq!(rename_title(&summary), None, "{content}");
        }
    }

    /// The title travels the same wrapped-in-whatever path the summary does.
    #[test]
    fn parse_summary_digs_the_title_out_of_a_fence() {
        let summary = parse_summary(
            &completion("```json\n{\"summary\":\"Waiting.\",\"title\":\"fixing resize\"}\n```"),
            1,
        )
        .expect("parsed");
        assert_eq!(summary.title, "fixing resize");
    }

    #[test]
    fn a_long_title_is_cut_to_something_a_pane_frame_could_hold() {
        let long = "x".repeat(SUMMARY_TITLE_CHARS * 2);
        let summary = parse_summary(
            &completion(&json!({"summary": "Done.", "title": long}).to_string()),
            1,
        )
        .expect("parsed");
        assert_eq!(summary.title.chars().count(), SUMMARY_TITLE_CHARS);
    }

    #[test]
    fn a_long_summary_is_cut_to_something_a_row_could_hold() {
        let long = "x".repeat(SUMMARY_TEXT_CHARS * 2);
        let summary = parse_summary(&completion(&json!({"summary": long}).to_string()), 1);
        assert_eq!(
            summary.expect("parsed").text.chars().count(),
            SUMMARY_TEXT_CHARS
        );
    }

    /// One summary, as `summary_toast` is asked to frame it.
    fn toast_summary(text: &str, awaiting: bool) -> Summary {
        Summary {
            text: text.to_owned(),
            title: "running the tests".to_owned(),
            awaiting,
            hash: 1,
            created_at_ms: None,
        }
    }

    /// The headline is the whole point of the toast: the summary text on its own says
    /// nothing about which of several agents produced it.
    #[test]
    fn a_toast_names_the_pane_above_what_it_said() {
        let toast = summary_toast(3, Some("claude"), &toast_summary("Ran the tests.", false));
        assert_eq!(toast, "agent idle · 3 · claude\nRan the tests.");
    }

    /// The same claim the strip's `!` makes, in a word.
    #[test]
    fn a_toast_for_an_agent_blocked_on_the_user_says_so() {
        let toast = summary_toast(7, Some("codex"), &toast_summary("Shall I push?", true));
        assert!(
            toast.starts_with("agent waiting · 7 · codex\n"),
            "got {toast:?}"
        );
    }

    /// A pane whose kind would not classify still gets a toast - the id names it.
    #[test]
    fn a_toast_without_a_kind_still_names_the_pane() {
        let toast = summary_toast(9, None, &toast_summary("Done.", false));
        assert_eq!(toast, "agent idle · 9\nDone.");
    }

    /// The renderer wraps and ellipsizes to the room a toast has; doing it here as well
    /// would mean two places disagreeing about how much that is.
    #[test]
    fn a_toast_carries_the_summary_whole() {
        let long = "x".repeat(SUMMARY_TEXT_CHARS);
        let toast = summary_toast(3, Some("claude"), &toast_summary(&long, false));
        assert!(
            toast.ends_with(&long),
            "the text is passed through untouched"
        );
    }

    /// The fallback that makes summaries work with no API key at all. Before the hook
    /// feed there was nothing to fall back *to* — a screen tail is not a summary of
    /// anything, and showing one raw would be worse than showing nothing. A final message
    /// is prose already, so its opening is an honest answer.
    #[test]
    fn a_summary_can_be_made_from_the_agents_own_words_with_no_model() {
        let summary = local_summary(
            "I ran the test suite and it passes. Two snapshots were regenerated along \
             the way; both diffs are whitespace only.",
            42,
        )
        .expect("long enough to be worth drawing");
        assert!(summary
            .text
            .starts_with("I ran the test suite and it passes."));
        assert_eq!(summary.hash, 42);
        // Naming a pane in four words is a judgement, and this path has nobody to make
        // it. An empty title is `rename_title`'s "leave the pane's name alone".
        assert_eq!(summary.title, "");
        assert_eq!(rename_title(&summary), None);
        // Decided by the feed, never here.
        assert!(!summary.awaiting);
    }

    /// A summary that opens with three lines of somebody else's Rust has said nothing,
    /// so a fenced block is dropped whole rather than unwrapped. The markers around the
    /// prose go too: they are structure for a renderer this box does not have.
    #[test]
    fn a_local_summary_keeps_the_sentences_and_drops_the_markdown() {
        let summary = local_summary(
            "## Done\n\n\
             The **flaky** test is fixed. It was waiting on a `sleep` rather than on the \
             condition it cared about.\n\n\
             ```rust\nfn main() { panic!() }\n```\n\n\
             - regenerated one snapshot\n\
             - left the other alone\n",
            0,
        )
        .expect("long enough");
        assert_eq!(
            summary.text,
            "Done The flaky test is fixed. It was waiting on a sleep rather than on the \
             condition it cared about. regenerated one snapshot left the other alone"
        );
        assert!(!summary.text.contains("panic!"));
        assert!(!summary.text.contains('`') && !summary.text.contains('#'));
    }

    /// The same floor a screen has: below it there is nothing to describe, and a box
    /// holding three words is worse than no box.
    #[test]
    fn a_reply_too_short_to_be_a_summary_is_not_one() {
        assert_eq!(local_summary("Done.", 0), None);
        assert_eq!(local_summary("", 0), None);
        assert_eq!(local_summary("```\nfn main() {}\n```", 0), None);
        assert!(local_summary(&"x".repeat(SUMMARY_MIN_CHARS), 0).is_some());
    }

    /// A model told to be brief occasionally is not, and neither is an agent. The cut is
    /// the same one the model's own answer is held to.
    #[test]
    fn a_long_reply_is_cut_to_what_a_summary_holds() {
        let summary = local_summary(&"word ".repeat(400), 0).expect("long enough");
        assert!(summary.text.chars().count() <= SUMMARY_TEXT_CHARS);
        assert!(summary.text.ends_with('…'));
    }

    /// What the model is shown for a run: the history the agent reported as it worked, and
    /// the question it was answering. A description of an answer written without sight of
    /// the question is half a description.
    #[test]
    fn a_run_summary_asks_about_the_history_and_the_request_behind_it() {
        let body = run_summary_request_body(
            "gpt-4o-mini",
            "claude",
            Some("/work"),
            Some("main"),
            Some("fix the flaky test"),
            "Fixed it; it was waiting on a sleep.",
        );
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["stream"], false);
        let user = body["messages"][1]["content"].as_str().expect("a string");
        assert!(user.starts_with("Agent: claude. Folder: /work. Branch: main."));
        assert!(user.contains("fix the flaky test"));
        assert!(user.contains("Fixed it; it was waiting on a sleep."));

        // The system prompt asks for two keys and does not ask about `awaiting`: the feed
        // already knows, and a second opinion would only be a disagreement to resolve.
        let system = body["messages"][0]["content"].as_str().expect("a string");
        assert!(system.contains("\"summary\"") && system.contains("\"title\""));
        assert!(!system.contains("awaiting"));

        // A run with no prompt behind it - a resumed session, a hook-driven
        // continuation - is summarised without one rather than against a stale question.
        let body = run_summary_request_body("m", "claude", None, None, None, "Done.");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        let user = body["messages"][1]["content"].as_str().expect("a string");
        assert_eq!(user, "Agent: claude.\n\n--- run history ---\nDone.");
    }

    #[test]
    fn run_history_keeps_only_meaningful_hook_events() {
        let event = |name: &str| crate::wire::HookEvent {
            hook_event_name: name.to_owned(),
            ..Default::default()
        };
        assert_eq!(run_entry(&event("PostToolUse")), None);
        assert_eq!(
            run_entry(&event("PreToolUse")),
            None,
            "unnamed tools are noise"
        );

        let prompt = crate::wire::HookEvent {
            prompt: Some("build the history".to_owned()),
            ..event("UserPromptSubmit")
        };
        assert_eq!(
            run_entry(&prompt),
            Some((RunEntryKind::Prompt, "build the history".to_owned()))
        );
        let pasted = crate::wire::HookEvent {
            prompt: Some(
                "Follow the instructions in the pasted text: \n\n<pasted_content id=\"8c9e\">\nbuild\nthe history\n</pasted_content id=\"8c9e\">\n"
                    .to_owned(),
            ),
            ..event("UserPromptSubmit")
        };
        assert_eq!(
            run_entry(&pasted),
            Some((RunEntryKind::Prompt, "build\nthe history".to_owned()))
        );
        let plan = crate::wire::HookEvent {
            tool_name: Some("ExitPlanMode".to_owned()),
            tool_input: Some(json!({"plan": "store prompt, plan, steps, result"})),
            ..event("PreToolUse")
        };
        assert_eq!(
            run_entry(&plan),
            Some((
                RunEntryKind::Plan,
                "store prompt, plan, steps, result".to_owned()
            ))
        );
        let ask = crate::wire::HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(json!({"questions": [{"question": "Which library?"}]})),
            ..event("PreToolUse")
        };
        assert_eq!(
            run_entry(&ask),
            Some((RunEntryKind::Question, "Which library?".to_owned()))
        );
        let codex_ask = crate::wire::HookEvent {
            tool_name: Some("request_user_input".to_owned()),
            tool_input: Some(json!({"questions": [{"question": "Which history?"}]})),
            ..event("PreToolUse")
        };
        assert_eq!(
            run_entry(&codex_ask),
            Some((RunEntryKind::Question, "Which history?".to_owned()))
        );
        let result = crate::wire::HookEvent {
            last_assistant_message: Some("Implemented and tested.".to_owned()),
            ..event("Stop")
        };
        assert_eq!(
            run_entry(&result),
            Some((RunEntryKind::Result, "Implemented and tested.".to_owned()))
        );
        assert_eq!(
            run_entry(&event("StopFailure")),
            Some((RunEntryKind::Result, "Turn failed".to_owned()))
        );
        assert_eq!(
            run_entry(&crate::wire::HookEvent {
                tool_name: Some("Bash".to_owned()),
                ..event("PreToolUse")
            }),
            None
        );
        assert_eq!(run_entry(&event("PermissionRequest")), None);
    }

    /// Nothing a hook event says is cut on its way into the history.
    ///
    /// The regression the split was made for: a prompt used to stop at 600 characters and a
    /// question at 80, both silently, because the whole timeline was one blob that grew with
    /// every event. It is rows now, and the panel wraps and scrolls.
    #[test]
    fn a_retained_entry_keeps_the_whole_of_what_the_event_said() {
        let long = "word ".repeat(4000);
        let prompt = crate::wire::HookEvent {
            hook_event_name: "UserPromptSubmit".to_owned(),
            prompt: Some(long.clone()),
            ..Default::default()
        };
        assert_eq!(
            run_entry(&prompt),
            Some((RunEntryKind::Prompt, long.trim().to_owned()))
        );

        let plan = crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some("ExitPlanMode".to_owned()),
            tool_input: Some(json!({ "plan": long.clone() })),
            ..Default::default()
        };
        assert_eq!(
            run_entry(&plan),
            Some((RunEntryKind::Plan, long.trim().to_owned()))
        );

        let ask = crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(json!({
                "questions": [
                    {
                        "header": "Caps",
                        "question": "Which of these should the history keep whole?",
                        "options": [{ "label": "all of them" }, { "label": "none of them" }],
                    },
                    { "question": "And the second question?" },
                ],
            })),
            ..Default::default()
        };
        let (kind, text) = run_entry(&ask).expect("a question");
        assert_eq!(kind, RunEntryKind::Question);
        assert!(
            text.contains("And the second question?"),
            "every question, not only the first: {text}"
        );
        assert!(text.contains("\n- all of them"), "and its options: {text}");
        assert!(
            text.chars().count() > WAITING_DETAIL_CHARS,
            "which is more than a toast's line: {text}"
        );
    }

    /// The two spellings of a kind are each other's inverse, so a row round trips.
    #[test]
    fn every_kind_survives_its_label() {
        for kind in [
            RunEntryKind::Prompt,
            RunEntryKind::Plan,
            RunEntryKind::Question,
            RunEntryKind::Step,
            RunEntryKind::Result,
            RunEntryKind::Message,
            RunEntryKind::Summary,
        ] {
            assert_eq!(RunEntryKind::from_label(kind.label()), Some(kind));
        }
        assert_eq!(RunEntryKind::from_label("something-newer"), None);
    }

    /// The whole timeline comes back, oldest first, and the caller windows it. The
    /// windowing itself now lives in the agents pane, which is the only thing that knows
    /// how many rows the history half has.
    #[test]
    fn a_run_history_is_the_whole_timeline_oldest_first() {
        let mut run = AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            yinz_session_name: "yinz".to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: 6,
            summarized_revision: 0,
            summary: None,
            entries: ["oldest", "second", "third", "fourth", "fifth", "newest"]
                .into_iter()
                .enumerate()
                .map(|(index, text)| RunEntry {
                    tool: None,
                    revision: index as u64 + 1,
                    event_id: None,
                    kind: RunEntryKind::Question,
                    text: text.to_owned(),
                    created_at_ms: None,
                })
                .collect(),
            omitted_entries: 0,
        };

        let history = run_history(&run, 40, 100, 0);
        assert_eq!(history.label, "claude:session-1");
        assert_eq!(history.entries.len(), 6);
        assert!(history.entries[0].text.contains("oldest"));
        assert!(history.entries[5].text.contains("newest"));

        run.entries.truncate(1);
        let short = run_history(&run, 40, 100, 0);
        assert_eq!(short.entries.len(), 1);
        assert!(short.entries[0].text.contains("oldest"));

        // Too narrow to wrap into anything but a column of single words: better nothing,
        // and the half says so instead.
        assert!(run_history(&run, HISTORY_MIN_COLS + 1, 100, 0).is_empty());
    }

    /// The two rows above the timeline, which are what the divider's eight characters of
    /// session id cannot be: the whole id, and the day and minute the run began.
    #[test]
    fn the_header_states_the_start_and_the_whole_session_id() {
        let mut run = a_pane_run();
        run.session_id = "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned();
        run.id = format!("claude:{}", run.session_id);
        // 2026-09-04 18:32:07 UTC, read an hour east of it.
        run.started_at_ms = 1_788_546_727_000;

        let header = run_history(&run, 60, run.started_at_ms, 3600).header;
        let rows: Vec<&str> = header.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            rows,
            vec![
                "started 2026-09-04 19:32:07 +01:00",
                "session 9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f",
            ]
        );
        // The divider still says the short form: the header is what the whole id is for.
        assert_eq!(run_history(&run, 60, 0, 0).label, "claude:9f3c1a7e");
    }

    /// The fallback id names the machine, and has to.
    ///
    /// A pane id means nothing outside the server that handed it out, so two servers sharing a
    /// store in this machine's data dir
    /// can reach for the same `kind:pane:5:<epoch>`. The row is written with an unguarded `Put`
    /// keyed on it, so a collision would not fail: it would interleave two agents' timelines
    /// under one uid and say nothing.
    #[test]
    fn two_machines_minting_a_fallback_run_at_once_do_not_collide() {
        let here = run_id("claude", 5, "", 1_764_618_000_000, &HostId::new("host-a"));
        let there = run_id("claude", 5, "", 1_764_618_000_000, &HostId::new("host-b"));
        assert_ne!(here, there);
        assert_eq!(here, "claude:pane:5:1764618000000:host-a");

        // An agent with a session id of its own is host-free, and stays that way: the
        // derivation has to reach the same answer on the far side of a warp.
        assert_eq!(
            run_id("claude", 5, "s1", 1_764_618_000_000, &HostId::new("host-a")),
            run_id("claude", 99, "s1", 0, &HostId::new("host-b")),
        );
        assert_eq!(
            run_id("claude", 5, "s1", 0, &HostId::new("host-a")),
            session_run_id("claude", "s1"),
        );

        // A caller that does not know its machine yet keeps the old shape, which is no worse
        // than what it replaced — and it is the shape every run recorded before this has.
        assert_eq!(
            run_id("claude", 5, "", 1_764_618_000_000, &HostId::new("")),
            "claude:pane:5:1764618000000",
        );
    }

    /// A run the agent gave no session id is keyed by pane, start time and machine (`run_id`),
    /// and calling that a session would be a claim it cannot make.
    #[test]
    fn a_run_with_no_session_id_of_its_own_is_headed_by_its_run_id() {
        let mut run = a_pane_run();
        run.session_id = String::new();
        run.id = run_id("claude", 3, "", 10, &HostId::new("host-a"));

        let header = run_history(&run, 60, 100, 0).header;
        assert_eq!(header[1].text, "run     claude:pane:3:10:host-a");
    }

    /// Cut ids are useless ids, so a half too narrow for one wraps it instead - under the
    /// value column, so the field still reads as one thing.
    #[test]
    fn a_session_id_wider_than_the_half_wraps_rather_than_being_cut() {
        let mut run = a_pane_run();
        run.session_id = "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned();

        // Wide enough for the timestamp on one row (26 columns of value), too narrow for a
        // 36-character uuid: the two fields are wrapped independently.
        let header = run_history(&run, 40, 100, 0).header;
        assert_eq!(header.len(), 3, "{header:?}");
        let id: String = header[1..]
            .iter()
            .map(|line| &line.text[HEADER_LABEL_WIDTH..])
            .collect();
        assert_eq!(id, run.session_id);
        for row in &header[2..] {
            assert!(
                row.text.starts_with(&" ".repeat(HEADER_LABEL_WIDTH)),
                "a continuation row sits under the value column: {:?}",
                row.text
            );
        }
    }

    /// Nothing pins a header over an idle agent's summary: there is no run, so neither of the
    /// two facts is in hand.
    #[test]
    fn a_summary_with_no_run_behind_it_has_no_header() {
        let summary = Summary {
            text: "did a thing".to_owned(),
            title: String::new(),
            awaiting: false,
            hash: 0,
            created_at_ms: Some(50),
        };
        assert!(summary_history(3, "claude", &summary, false, 40, 100)
            .header
            .is_empty());
    }

    /// A summary somebody asked for is drawn wherever it was made, on a run that is still
    /// going and on one that has ended. The timeline keeps it beside the original messages.
    #[test]
    fn a_summary_somebody_asked_for_is_drawn_on_the_timeline_whatever_the_run_is_doing() {
        let mut run = AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            yinz_session_name: "yinz".to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: 1,
            summarized_revision: 1,
            summary: Some(Summary {
                text: "Rewrote the retry backoff.".to_owned(),
                title: String::new(),
                awaiting: false,
                hash: 0,
                created_at_ms: None,
            }),
            entries: vec![RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Summary,
                text: "Rewrote the retry backoff.".to_owned(),
                created_at_ms: None,
            }],
            omitted_entries: 0,
        };

        let live = run_history(&run, 60, 100, 0);
        assert!(
            live.entries
                .iter()
                .any(|line| line.text.contains("summary: Rewrote the retry backoff.")),
            "filed on the timeline"
        );
        assert_eq!(
            live.entries
                .iter()
                .filter(|line| line.text.contains("Rewrote the retry backoff."))
                .count(),
            1,
            "once: the rolling summary says the same thing, and is dropped rather than              repeated under the entry"
        );

        run.outcome = RunOutcome::Completed;
        assert!(
            run_history(&run, 60, 100, 0)
                .entries
                .iter()
                .any(|line| line.text.contains("summary:")),
            "and it survives the run ending, unlike a result the summary absorbed"
        );
    }

    /// The kind travels through the shared store as `summary`, which is what an older build
    /// reading the same file would have to understand. Asserted on the wire spelling rather
    /// than on a round trip alone, because the spelling is the compatibility surface.
    #[test]
    fn a_filed_summary_round_trips_as_its_own_kind() {
        let entry = RunEntry {
            tool: None,
            revision: 4,
            event_id: None,
            kind: RunEntryKind::Summary,
            text: "Rewrote the retry backoff.".to_owned(),
            created_at_ms: None,
        };

        let json = serde_json::to_string(&entry).expect("encoded");
        assert!(json.contains(r#""kind":"summary""#), "{json}");
        let back: RunEntry = serde_json::from_str(&json).expect("parsed");
        assert_eq!(back, entry);
        assert_eq!(RunEntryKind::Summary.label(), "summary");
    }

    /// Two presses against an agent that has not moved produce the same sentence twice, and
    /// two identical paragraphs one under the other read as a bug rather than as a history.
    /// Only the newest is compared: a run that comes back round to the same state after
    /// doing something else really was in that state twice.
    #[test]
    fn filing_a_summary_skips_one_that_repeats_the_newest() {
        let mut run = AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            yinz_session_name: "yinz".to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: 0,
            summarized_revision: 0,
            summary: None,
            entries: Vec::new(),
            omitted_entries: 0,
        };

        assert!(push_summary_entry(
            &mut run,
            "Rewrote the retry backoff.",
            100
        ));
        assert_eq!(
            run.revision, 1,
            "a filed summary is a revision like any other"
        );
        assert!(!push_summary_entry(
            &mut run,
            "Rewrote the retry backoff.",
            100
        ));
        assert!(
            !push_summary_entry(&mut run, "  Rewrote the retry backoff.  ", 100),
            "trimmed before comparing, so whitespace is not a new sentence"
        );
        assert_eq!(run.entries.len(), 1);
        assert_eq!(run.revision, 1, "and a skipped one costs no revision");

        assert!(push_summary_entry(&mut run, "Then rewrote the tests.", 100));
        assert!(
            push_summary_entry(&mut run, "Rewrote the retry backoff.", 100),
            "the run came back round to the same state, which really did happen twice"
        );
        assert_eq!(run.entries.len(), 3);

        assert!(
            !push_summary_entry(&mut run, "   ", 100),
            "and nothing at all is never filed"
        );
    }

    #[test]
    fn active_and_archived_runs_keep_results_visible_after_summarization() {
        let mut run = AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            yinz_session_name: "yinz".to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: 1,
            summarized_revision: 0,
            summary: None,
            entries: vec![RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Result,
                text: "Implemented and tested.".to_owned(),
                created_at_ms: None,
            }],
            omitted_entries: 0,
        };

        assert!(
            !run_history(&run, 60, 100, 0).entries.is_empty(),
            "a completed response stays visible during the run"
        );

        run.outcome = RunOutcome::Completed;
        let archived = run_history(&run, 60, 100, 0);
        assert_eq!(archived.label, "claude:session-1");
        assert!(archived
            .entries
            .iter()
            .any(|line| line.text.contains("result: Implemented and tested.")));

        run.summarized_revision = run.revision;
        run.summary = Some(Summary {
            text: "The implementation is complete.".to_owned(),
            title: String::new(),
            awaiting: false,
            hash: 0,
            created_at_ms: None,
        });
        let summarized = run_history(&run, 60, 100, 0);
        assert!(summarized
            .entries
            .iter()
            .any(|line| line.text.contains("result:")));
        assert!(
            summarized
                .entries
                .iter()
                // Past the age gutter, which is blank on an entry with no recorded time.
                .any(|line| line
                    .text
                    .trim_start()
                    .starts_with("summary: The implementation")),
            "{:?}",
            summarized.entries
        );
    }

    #[test]
    fn an_agent_run_round_trips_through_the_shared_store() {
        use crate::store::{self, Backend};
        use yinz_tile::prelude::{FetchRequest, Mutation, Pred};

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let legacy = |uid: &str, active: bool| {
            Mutation::put(rows::RUN_ENTITY)
                .uid(uid)
                .set("pane_id", 7_u32)
                .set("session_id", uid)
                .set("kind", "claude")
                .set("active", active)
                .set("started_at_ms", 1_i64)
                .set("payload", "{}")
                .build()
        };
        local
            .apply(vec![
                legacy("legacy-active", true),
                legacy("legacy-archive", false),
            ])
            .unwrap();
        let mut run = AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            yinz_session_name: "yinz".to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: 1,
            summarized_revision: 0,
            summary: None,
            entries: vec![RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: "implement history".to_owned(),
                created_at_ms: None,
            }],
            omitted_entries: 0,
        };
        local.apply(rows::run_mutations(&run)).unwrap();
        let mut foreign = run.clone();
        foreign.id = "claude:session-2".to_owned();
        foreign.session_id = "session-2".to_owned();
        foreign.yinz_session_name = "another-yinz".to_owned().into();
        local.apply(rows::run_mutations(&foreign)).unwrap();
        let active = local
            .fetch(&rows::active_runs_request(&owner("yinz")))
            .unwrap();
        // The metadata comes back from the run's own row; the timeline is rows of its own, and
        // arrives only once it is asked for.
        let mut restored = rows::runs_from_rows(&active);
        assert_eq!(
            restored.iter().map(|run| &run.id).collect::<Vec<_>>(),
            vec![&run.id]
        );
        assert!(restored[0].entries.is_empty());
        assert_eq!(
            rows::hydrate_entries(&local, &mut restored),
            std::collections::BTreeSet::from([run.id.clone()])
        );
        assert_eq!(restored, vec![run.clone()]);

        run.ended_at_ms = Some(20);
        run.outcome = RunOutcome::Completed;
        local.apply(rows::run_mutations(&run)).unwrap();
        let completed = local
            .fetch(
                &FetchRequest::entity(rows::RUN_ENTITY)
                    .filter(Pred::eq("active", false))
                    .filter(Pred::not_null("header"))
                    .select(["header"]),
            )
            .unwrap();
        assert!(!completed.columns.iter().any(|column| column == "payload"));
        assert_eq!(rows::run_headers_from_rows(&completed), vec![run.header()]);

        let detail = local
            .fetch(
                &FetchRequest::entity(rows::RUN_ENTITY)
                    .filter(Pred::eq("uid", run.id.clone()))
                    .select(["payload"]),
            )
            .unwrap();
        let mut detail = rows::runs_from_rows(&detail);
        rows::hydrate_entries(&local, &mut detail);
        assert_eq!(detail, vec![run]);
    }

    /// A run written before the timeline had rows of its own keeps the entries in its payload.
    ///
    /// The whole of the backward compatibility: there is no migration and no backfill, so a
    /// hydration that finds no rows for a run must leave what the payload carried alone.
    #[test]
    fn a_run_stored_before_the_split_keeps_the_entries_in_its_payload() {
        use crate::store::{self, Backend};
        use yinz_tile::prelude::Mutation;

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let run = a_run_with_entries("claude:session-1", "yinz", 7, &["asked", "answered"]);
        // The old shape: the whole run, timeline included, inside `payload`.
        local
            .apply(vec![Mutation::put(rows::RUN_ENTITY)
                .uid(run.id.clone())
                .set("pane_id", run.pane_id)
                .set("session_id", run.session_id.clone())
                .set("yinz_session_name", run.yinz_session_name.clone())
                .set("host_id", run.host_id.clone())
                .set("kind", run.kind.clone())
                .set("active", true)
                .set("started_at_ms", run.started_at_ms)
                .set("payload", serde_json::to_string(&run).unwrap())
                .build()])
            .unwrap();
        let mut restored = rows::runs_from_rows(
            &local
                .fetch(&rows::active_runs_request(&owner("yinz")))
                .unwrap(),
        );
        assert_eq!(
            restored,
            vec![run.clone()],
            "the payload still carries them"
        );
        assert!(
            rows::hydrate_entries(&local, &mut restored).is_empty(),
            "nothing was hydrated, so the caller knows they are not yet rows"
        );
        assert_eq!(restored, vec![run], "and they were not taken away");
    }

    /// What the panel is sent is bounded by bytes, not only by rows.
    ///
    /// An entry keeps the whole of what the agent said, so five hundred of them is not a size:
    /// only [`rows::RUN_DETAIL_BYTES`] makes the payload the panel is handed a bounded one.
    #[test]
    fn a_run_detail_is_cut_to_its_byte_budget_from_the_newest_end() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        // Ten entries of a tenth of the budget each, so half of them fit and the oldest half
        // is what goes.
        let each = rows::RUN_DETAIL_BYTES / 10;
        let texts: Vec<String> = (0..10).map(|n| format!("{n}").repeat(each)).collect();
        let run = a_run_with_entries(
            "claude:session-1",
            "yinz",
            7,
            &texts.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        local.apply(rows::run_mutations(&run)).unwrap();

        let mut detail = AgentRun {
            entries: Vec::new(),
            ..run.clone()
        };
        rows::hydrate_newest_entries(&local, &mut detail);
        let spent: usize = detail.entries.iter().map(|entry| entry.text.len()).sum();
        assert!(
            spent <= rows::RUN_DETAIL_BYTES,
            "the payload is within the budget, got {spent}"
        );
        assert_eq!(
            detail.entries,
            run.entries[run.entries.len() - detail.entries.len()..],
            "and what it kept is the newest end, in order"
        );
        assert!(!detail.entries.is_empty());
        assert_eq!(
            detail.omitted_entries,
            run.entries.len() - detail.entries.len(),
            "and it counts what it left out"
        );

        // One entry over the budget on its own is still sent: an empty history is worse than
        // a large one.
        let huge = "x".repeat(rows::RUN_DETAIL_BYTES * 2);
        let big = a_run_with_entries("claude:session-2", "yinz", 8, &[huge.as_str()]);
        local.apply(rows::run_mutations(&big)).unwrap();
        let mut detail = AgentRun {
            entries: Vec::new(),
            ..big.clone()
        };
        rows::hydrate_newest_entries(&local, &mut detail);
        assert_eq!(detail.entries, big.entries);
        assert_eq!(detail.omitted_entries, 0);
    }

    /// A run longer than the panel is handed says so at the top of its timeline, rather than
    /// letting the oldest event it was sent pass for the start of the run.
    #[test]
    fn a_run_cut_to_its_newest_entries_opens_with_a_notice() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let texts: Vec<String> = (0..rows::RUN_DETAIL_ENTRIES + 5)
            .map(|n| format!("entry {n}"))
            .collect();
        let run = a_run_with_entries(
            "claude:session-1",
            "yinz",
            7,
            &texts.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        local.apply(rows::run_mutations(&run)).unwrap();
        let mut detail = AgentRun {
            entries: Vec::new(),
            ..run.clone()
        };
        rows::hydrate_newest_entries(&local, &mut detail);
        assert_eq!(detail.entries.len(), rows::RUN_DETAIL_ENTRIES as usize);
        assert!(detail.omitted_entries > 0, "the row limit cut it");
        // The count rides to the panel with the run, and is left off a run that has nothing
        // to say about it.
        let sent: AgentRun =
            serde_json::from_str(&serde_json::to_string(&detail).unwrap()).unwrap();
        assert_eq!(sent.omitted_entries, detail.omitted_entries);
        assert!(!serde_json::to_string(&run)
            .unwrap()
            .contains("omitted_entries"));

        let history = run_history(&detail, 60, 100, 0);
        assert_eq!(
            history.entries[0].text.trim(),
            "older events not loaded · / searches them"
        );
        assert!(history.entries[0]
            .styles
            .iter()
            .all(|style| style.dim || *style == MdStyle::default()));
        // It belongs to no event: the first one starts after it, and the rest still tile.
        let mut at = 1;
        for event in &history.events {
            assert_eq!(event.start, at, "{:?}", history.events);
            at = event.end();
        }
        assert_eq!(at, history.entries.len());

        // A half too narrow for all of it keeps the part that says what happened, and one too
        // narrow for that says it shorter - a whole phrase at every width, never one cut off
        // mid-word, and still the one row.
        for width in HISTORY_MIN_COLS + 2..=60 {
            let history = run_history(&detail, width, 100, 0);
            let notice = &history.entries[0].text;
            assert!(
                [
                    "older not loaded",
                    "older events not loaded",
                    "older events not loaded · / searches them",
                ]
                .contains(&notice.trim()),
                "{width}: {notice:?}"
            );
            assert!(notice.width() <= width - 2, "{width}: {notice:?}");
            assert_eq!(history.events[0].start, 1, "{width}");
        }

        // A run that was sent whole has no notice.
        let whole = run_history(&run, 60, 100, 0);
        assert!(!whole.entries[0].text.contains("not loaded"));
        assert_eq!(whole.events[0].start, 0);
    }

    /// A timeline longer than one fetch page still comes back whole.
    #[test]
    fn a_long_timeline_is_paged_back_in_full() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let texts: Vec<String> = (0..yinz_tile::prelude::MAX_FETCH_ROWS + 20)
            .map(|n| format!("entry {n}"))
            .collect();
        let run = a_run_with_entries(
            "claude:session-1",
            "yinz",
            7,
            &texts.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        local.apply(rows::run_mutations(&run)).unwrap();
        let mut restored = rows::runs_from_rows(
            &local
                .fetch(&rows::active_runs_request(&owner("yinz")))
                .unwrap(),
        );
        rows::hydrate_entries(&local, &mut restored);
        assert_eq!(restored, vec![run]);
    }

    #[test]
    fn lineage_round_trips_in_payload_and_header_and_defaults_to_absent() {
        use crate::store::{self, Backend};
        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let mut run = a_run_with_entries("claude:C", "here", 7, &["continue"]);
        for linked in [false, true] {
            if linked {
                run.origin_run_id = Some("claude:A".into());
                run.previous_run_id = Some("claude:B".into());
            }
            let json = serde_json::to_value(&run).unwrap();
            assert_eq!(json.get("origin_run_id").is_some(), linked);
            assert_eq!(serde_json::from_value::<AgentRun>(json).unwrap(), run);
            let header = serde_json::to_value(run.header()).unwrap();
            assert_eq!(header.get("previous_run_id").is_some(), linked);
            assert_eq!(
                serde_json::from_value::<AgentRunHeader>(header).unwrap(),
                run.header()
            );
            local.apply(rows::run_mutations(&run)).unwrap();
            assert_eq!(stored_run(&local, &run.id), run);
            let fetched = local
                .fetch(&rows::run_by_id_request(&run.id).select(["header"]))
                .unwrap();
            assert_eq!(rows::run_headers_from_rows(&fetched), vec![run.header()]);
            let adopted = stored_run(&local, &run.id).adopted_by(9, &owner("there"));
            assert_eq!(adopted.origin_run_id, run.origin_run_id);
            assert_eq!(adopted.previous_run_id, run.previous_run_id);
        }
    }

    #[test]
    fn lineage_header_wraps_full_agent_session_ids() {
        let mut run = a_pane_run();
        let origin = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let previous = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        run.origin_run_id = Some(format!("claude:{origin}"));
        run.previous_run_id = Some(format!("claude:{previous}"));
        for width in [20, 32, 60] {
            let header = run_header(&RunFacts::of(&run), width, 0);
            assert!(header.iter().all(|row| row.text.chars().count() <= width));
            for (label, expected) in [("origin", origin), ("previous", previous)] {
                let field: String = header
                    .iter()
                    .skip_while(|row| !row.text.starts_with(label))
                    .enumerate()
                    .take_while(|(i, row)| *i == 0 || row.text.starts_with(' '))
                    .map(|(i, row)| {
                        if i == 0 {
                            row.text[label.len()..].trim()
                        } else {
                            row.text.trim()
                        }
                    })
                    .collect();
                assert_eq!(field, expected);
            }
        }
    }

    fn a_run_with_entries(id: &str, session: &str, pane_id: u32, entries: &[&str]) -> AgentRun {
        AgentRun {
            id: id.to_owned(),
            pane_id,
            yinz_session_name: session.to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: entries.len() as u64,
            summarized_revision: 0,
            summary: None,
            entries: entries
                .iter()
                .enumerate()
                .map(|(index, text)| RunEntry {
                    tool: None,
                    revision: index as u64 + 1,
                    event_id: None,
                    kind: RunEntryKind::Prompt,
                    text: (*text).to_owned(),
                    created_at_ms: None,
                })
                .collect(),
            omitted_entries: 0,
        }
    }

    /// One stored run, timeline and all — the two fetches a reader of a whole run makes.
    fn stored_run(local: &impl crate::store::Backend, id: &str) -> AgentRun {
        let rows = local.fetch(&rows::run_by_id_request(id)).unwrap();
        let mut runs = rows::runs_from_rows(&rows);
        rows::hydrate_entries(local, &mut runs);
        runs.into_iter().next().expect("the run is stored")
    }

    /// A warped pane's run continues where it left off.
    ///
    /// The session that lost the pane cannot tell a warp from a `kill -9`, so it closes the
    /// run. The session that gained it looks the same row up by id — which is the agent's own
    /// session id, unchanged by the move — and reopens it. Nothing about the run is rebuilt:
    /// the prompts, plans and results are the same work by the same agent.
    #[test]
    fn a_closed_run_is_reopened_by_the_session_that_adopted_its_pane() {
        use crate::store::{self, Backend};

        let losing = store::Local::shared("agent-service");
        losing.define_all(&store::shared_defs()).unwrap();
        let gaining = losing.joined(store::SHARED, "agent-service");

        let mut run = a_run_with_entries("claude:session-1", "here", 7, &["ship it"]);
        losing.apply(rows::run_mutations(&run)).unwrap();
        // The pane leaves the manifest, so the losing session releases the run.
        run.ended_at_ms = Some(20);
        run.outcome = RunOutcome::Closed;
        losing
            .apply(vec![rows::run_owned_mutation(&run, &owner("here"))])
            .unwrap();
        assert_eq!(
            stored_run(&losing, "claude:session-1").outcome,
            RunOutcome::Closed
        );

        // The gaining session finds it by the id the record carried, and claims it.
        let adopted = stored_run(&gaining, "claude:session-1").adopted_by(2, &owner("there"));
        assert_eq!(adopted.entries.len(), 1, "the history came with it");
        assert_eq!(adopted.started_at_ms, 10, "and so did when it began");
        assert_eq!(adopted.outcome, RunOutcome::Active);
        gaining.apply(rows::run_mutations(&adopted)).unwrap();

        let stored = stored_run(&losing, "claude:session-1");
        assert_eq!(stored.pane_id, 2);
        assert_eq!(stored.yinz_session_name, "there".into());
        assert_eq!(stored.ended_at_ms, None);
    }

    /// The race the guard exists for, in the order that used to lose the history.
    ///
    /// A session notices a pane is gone on its next manifest, which can be well after the
    /// session that took it has adopted the run and added to it. An unconditional write of the
    /// loser's closed copy would put the row back to where it was before the move, throwing
    /// away everything the agent has done since. The predicate settles it inside the
    /// transaction: the row is no longer ours, so the write matches nothing.
    #[test]
    fn a_release_after_the_run_moved_on_does_not_overwrite_it() {
        use crate::store::{self, Backend};

        let losing = store::Local::shared("agent-service");
        losing.define_all(&store::shared_defs()).unwrap();
        let gaining = losing.joined(store::SHARED, "agent-service");

        let run = a_run_with_entries("claude:session-1", "here", 7, &["ship it"]);
        losing.apply(rows::run_mutations(&run)).unwrap();

        // The gaining session gets there first, and the agent keeps working.
        let mut adopted = stored_run(&gaining, "claude:session-1").adopted_by(2, &owner("there"));
        adopted.revision += 1;
        adopted.entries.push(RunEntry {
            tool: None,
            revision: adopted.revision,
            event_id: None,
            kind: RunEntryKind::Result,
            text: "shipped".to_owned(),
            created_at_ms: None,
        });
        gaining.apply(rows::run_mutations(&adopted)).unwrap();

        // Only now does the losing session notice the pane is gone.
        let mut released = run.clone();
        released.ended_at_ms = Some(20);
        released.outcome = RunOutcome::Closed;
        losing
            .apply(vec![rows::run_owned_mutation(&released, &owner("here"))])
            .unwrap();

        let stored = stored_run(&losing, "claude:session-1");
        assert_eq!(stored, adopted, "the loser's last word was dropped whole");
        assert_eq!(
            stored.entries.len(),
            2,
            "including what arrived after the move"
        );
        assert_eq!(stored.ended_at_ms, None, "and the run is still open");
    }

    /// The same guard, doing nothing when it should do nothing: a run whose pane really did
    /// close is still ours, so releasing it closes it.
    #[test]
    fn a_release_of_a_run_nobody_took_closes_it() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let mut run = a_run_with_entries("claude:session-1", "here", 7, &["ship it"]);
        local.apply(rows::run_mutations(&run)).unwrap();

        run.ended_at_ms = Some(20);
        run.outcome = RunOutcome::Closed;
        local
            .apply(vec![rows::run_owned_mutation(&run, &owner("here"))])
            .unwrap();

        let stored = stored_run(&local, "claude:session-1");
        assert_eq!(stored.outcome, RunOutcome::Closed);
        assert_eq!(stored.ended_at_ms, Some(20));
        // The write that closed it rewrote the payload without a timeline, which is what the
        // entry rows are for: closing a run does not cost it its history.
        assert_eq!(
            stored
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["ship it"],
        );
        assert!(
            local
                .fetch(&rows::active_runs_request(&owner("here")))
                .unwrap()
                .rows
                .is_empty(),
            "and it is out of the live set"
        );
    }

    /// Which id a pane's record points at, and when it points at nothing.
    #[test]
    fn a_records_stored_run_is_named_or_derived_or_neither() {
        let record = |session_id: Option<&str>, run_id: Option<&str>| AgentInfo {
            kind: "claude".to_owned(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: session_id.map(str::to_owned),
            run_id: run_id.map(str::to_owned),
        };
        // Named outright: the only road open to an agent with no session id of its own.
        assert_eq!(
            stored_run_id(&record(None, Some("mine:7"))),
            Some("mine:7".to_owned())
        );
        // Derived, and the derivation mentions neither the pane nor the machine - which is
        // what lets the far side of a warp reach the same answer.
        assert_eq!(
            stored_run_id(&record(Some("s1"), None)),
            Some("claude:s1".to_owned())
        );
        // A name wins over a derivation: it is what a plugin actually wrote down.
        assert_eq!(
            stored_run_id(&record(Some("s1"), Some("mine:7"))),
            Some("mine:7".to_owned())
        );
        // Neither. A run keyed by pane id and a timestamp cannot be found from here.
        assert_eq!(stored_run_id(&record(None, None)), None);
        assert_eq!(stored_run_id(&record(Some("  "), Some(" "))), None);
    }

    /// Two sessions, one store file, one pane id. These rows used to be uid'd by the pane id
    /// alone, so each session's write landed on the other's row and each session's prune
    /// deleted the other's rows.
    #[test]
    fn per_pane_rows_belong_to_one_session() {
        use crate::store::{self, Backend};

        let here = store::Local::shared("agent");
        here.define_all(&store::shared_defs()).unwrap();
        let there = here.joined(store::SHARED, "agent");

        here.apply(vec![rows::latch_mutation(&owner("here"), 5, "reviewer")])
            .unwrap();
        there
            .apply(vec![rows::latch_mutation(&owner("there"), 5, "builder")])
            .unwrap();
        let latches_of = |backend: &dyn Backend, session: &str| {
            let rows = backend
                .fetch(&rows::owned_rows_request(
                    rows::LATCH_ENTITY,
                    &owner(session),
                ))
                .unwrap();
            rows::latches_from_rows(&rows)
        };
        assert_eq!(
            latches_of(&here, "here"),
            BTreeMap::from([(5, "reviewer".to_owned())])
        );
        assert_eq!(
            latches_of(&there, "there"),
            BTreeMap::from([(5, "builder".to_owned())])
        );

        // And a prune reaches only its own.
        here.apply(vec![rows::latch_delete_mutation(&owner("here"), [5])])
            .unwrap();
        assert!(latches_of(&here, "here").is_empty());
        assert_eq!(
            latches_of(&there, "there"),
            BTreeMap::from([(5, "builder".to_owned())]),
            "the other session's pane 5 is a different pane"
        );
    }

    /// The same collision one size up: two host identities in one store, one session name, one pane
    /// id. Session names are not unique between machines — `main` is the obvious case — so
    /// scoping by session alone put two machines' pane 5 back into the row the session key had
    /// just separated, and each machine's prune deleted the other's.
    #[test]
    fn per_pane_rows_belong_to_one_machine_as_well_as_one_session() {
        use crate::store::{self, Backend};

        let laptop = store::Local::shared("agent");
        laptop.define_all(&store::shared_defs()).unwrap();
        let build_box = laptop.joined(store::SHARED, "agent");

        // Both sessions are called `main`, which is what makes this a collision rather than a
        // coincidence.
        let on_laptop = rows::Owner::new(HostId::new("host-laptop"), "main".into());
        let on_build_box = rows::Owner::new(HostId::new("host-build-box"), "main".into());

        laptop
            .apply(vec![rows::latch_mutation(&on_laptop, 5, "reviewer")])
            .unwrap();
        build_box
            .apply(vec![rows::latch_mutation(&on_build_box, 5, "builder")])
            .unwrap();

        let latches_of = |backend: &dyn Backend, owner: &rows::Owner| {
            let rows = backend
                .fetch(&rows::owned_rows_request(rows::LATCH_ENTITY, owner))
                .unwrap();
            rows::latches_from_rows(&rows)
        };
        assert_eq!(
            latches_of(&laptop, &on_laptop),
            BTreeMap::from([(5, "reviewer".to_owned())])
        );
        assert_eq!(
            latches_of(&build_box, &on_build_box),
            BTreeMap::from([(5, "builder".to_owned())]),
            "the other machine's pane 5 is a different pane"
        );

        // And a prune reaches only its own machine's.
        laptop
            .apply(vec![rows::latch_delete_mutation(&on_laptop, [5])])
            .unwrap();
        assert!(latches_of(&laptop, &on_laptop).is_empty());
        assert_eq!(
            latches_of(&build_box, &on_build_box),
            BTreeMap::from([(5, "builder".to_owned())]),
            "one machine's prune deleted another machine's row"
        );
    }

    /// The ownership guard, one size up. A run released by one session is written under a
    /// predicate naming its owner, so a session that has since taken the pane keeps its copy.
    /// With the owner as a bare session name, `main` on one machine passed the guard held by
    /// `main` on another — worse than no guard, because it looks like one.
    #[test]
    fn a_guarded_run_write_does_not_match_another_machines_session_of_the_same_name() {
        use crate::store::{self, Backend};

        let laptop = store::Local::shared("agent-service");
        laptop.define_all(&store::shared_defs()).unwrap();
        let build_box = laptop.joined(store::SHARED, "agent-service");

        let on_laptop = rows::Owner::new(HostId::new("host-laptop"), "main".into());
        let on_build_box = rows::Owner::new(HostId::new("host-build-box"), "main".into());

        // The build box owns the run, and has said something recent about it.
        let mut run = a_run_with_entries("claude:session-1", "main", 7, &["answered"]);
        run.host_id = on_build_box.host_id.clone();
        build_box.apply(rows::run_mutations(&run)).unwrap();

        // The laptop's session, also called `main`, closes a run it believes it lost.
        let mut stale = run.clone();
        stale.host_id = on_laptop.host_id.clone();
        stale.ended_at_ms = Some(99);
        stale.outcome = RunOutcome::Completed;
        laptop
            .apply(vec![rows::run_owned_mutation(&stale, &on_laptop)])
            .unwrap();

        let live = rows::runs_from_rows(
            &build_box
                .fetch(&rows::active_runs_request(&on_build_box))
                .unwrap(),
        );
        assert_eq!(
            live.len(),
            1,
            "the other machine's write closed a run it does not own"
        );
        assert_eq!(live[0].outcome, RunOutcome::Active);
    }

    /// The v1 → v2 migration is one the store performs unasked, and it had better be: every
    /// agent declares these entities on load and gives up on the store entirely if a
    /// declaration is refused. Dropping `unique` from `pane_id` and adding a column with a
    /// default are both non-destructive, but that is a property of the engine rather than of
    /// this crate, so it is checked here rather than assumed.
    #[test]
    fn the_pane_id_only_entities_migrate_rather_than_refusing() {
        use crate::store::{self, Backend};
        use yinz_tile::prelude::{AttrDef, EntityDef, FetchRequest, Mutation};

        let local = store::Local::shared("agent");
        // The definitions as they were, and a row under the uid they used.
        local
            .define(
                &EntityDef::new(rows::LATCH_ENTITY, 1)
                    .attr(AttrDef::int("pane_id").unique().required())
                    .attr(AttrDef::text("kind").required()),
            )
            .expect("the old definition still declares");
        local
            .apply(vec![Mutation::put(rows::LATCH_ENTITY)
                .uid("5")
                .set("pane_id", 5_u32)
                .set("kind", "reviewer")
                .build()])
            .expect("an old row");

        local
            .define_all(&store::shared_defs())
            .expect("the new definition migrates rather than being refused");

        // Two sessions can now hold pane 5, which is what the unique index forbade.
        local
            .apply(vec![
                rows::latch_mutation(&owner("here"), 5, "reviewer"),
                rows::latch_mutation(&owner("there"), 5, "builder"),
            ])
            .expect("one pane id, two sessions");

        // And the row from before, which no predicate a current row matches can reach, is
        // dropped by the one-time cleanup.
        local
            .apply(rows::stale_unowned_delete_mutations())
            .expect("cleaned up");
        let all = local
            .fetch(&FetchRequest::entity(rows::LATCH_ENTITY))
            .unwrap();
        assert_eq!(
            all.rows.len(),
            2,
            "only the two session-scoped rows are left"
        );
    }

    /// The `agent_run` half of the same migration, and the one that could not be cleaned up
    /// afterwards: a run is history, so an unreachable row is data lost rather than litter.
    ///
    /// The store fills the new `host_id` in with the declaration's default, which is empty, so
    /// every run recorded before this branch names no machine. A restore that demanded this
    /// machine's id found none of its own open runs — and the guarded close names the host too,
    /// so it could not have closed them either. They would have sat `active` forever while a
    /// fresh run started beside each of them for the same agent.
    #[test]
    fn a_run_written_before_the_host_column_is_restored_and_then_claimed() {
        use crate::store::{self, Backend};
        use yinz_tile::prelude::{AttrDef, EntityDef, Mutation};

        let local = store::Local::shared("agent-service");
        // The definition as it was, without a host, and a run stored under it. Its payload has
        // no `host_id` field at all, which is what a run serialized by that version looks like.
        local
            .define(
                &EntityDef::new(rows::RUN_ENTITY, 2)
                    .attr(AttrDef::int("pane_id").indexed().required())
                    .attr(AttrDef::text("session_id").indexed().required().default(""))
                    .attr(AttrDef::text("yinz_session_name").indexed())
                    .attr(AttrDef::text("kind").indexed().required())
                    .attr(
                        AttrDef::boolean("active")
                            .indexed()
                            .required()
                            .default(true),
                    )
                    .attr(AttrDef::int("started_at_ms").required())
                    .attr(AttrDef::json("header"))
                    .attr(AttrDef::json("payload").required()),
            )
            .expect("the old definition still declares");
        let mut before = a_run_with_entries("claude:session-1", "yinz", 7, &[]);
        before.host_id = HostId::default();
        local
            .apply(vec![Mutation::put(rows::RUN_ENTITY)
                .uid(before.id.clone())
                .set("pane_id", before.pane_id)
                .set("session_id", before.session_id.clone())
                .set("yinz_session_name", before.yinz_session_name.clone())
                .set("kind", before.kind.clone())
                .set("active", true)
                .set("started_at_ms", before.started_at_ms)
                .set("payload", serde_json::to_string(&before).unwrap())
                .build()])
            .expect("a run from before the column");

        local
            .define_all(&store::shared_defs())
            .expect("the new definition migrates rather than being refused");

        // The restore finds it, where a strict host check would have found nothing.
        let mut restored = rows::runs_from_rows(
            &local
                .fetch(&rows::active_runs_request(&owner("yinz")))
                .unwrap(),
        );
        assert_eq!(restored, vec![before.clone()], "the run was stranded");
        assert!(restored[0].host_id.is_empty(), "and it still names no host");

        // Claiming costs no write of its own: the stamp rides out with the next persist.
        rows::claim_unowned_runs(&mut restored, &owner("yinz"));
        assert_eq!(restored[0].host_id, HostId::new(HOST));
        local.apply(rows::run_mutations(&restored[0])).unwrap();

        // Which narrows the row. Another machine running a session of the same name could reach
        // it while it named no host, and cannot once it does.
        let elsewhere = rows::Owner::new(HostId::new("host-b"), "yinz".into());
        assert!(
            rows::runs_from_rows(&local.fetch(&rows::active_runs_request(&elsewhere)).unwrap())
                .is_empty(),
            "the claimed run is not the other machine's"
        );
        assert_eq!(
            rows::runs_from_rows(
                &local
                    .fetch(&rows::active_runs_request(&owner("yinz")))
                    .unwrap()
            )
            .len(),
            1,
            "and is still ours"
        );
    }

    /// The other end of the same run's life. A close is written under a guard, so a guard this
    /// session could never satisfy would leave a pre-migration run open for good.
    #[test]
    fn a_guarded_close_can_close_a_run_that_predates_the_host_column() {
        use crate::store::{self, Backend};

        let local = store::Local::shared("agent-service");
        local.define_all(&store::shared_defs()).unwrap();
        let mut run = a_run_with_entries("claude:session-1", "yinz", 7, &["asked"]);
        run.host_id = HostId::default();
        local.apply(rows::run_mutations(&run)).unwrap();

        // The daemon restores it, claims it in memory, and closes it when the pane goes.
        run.host_id = HostId::new(HOST);
        run.ended_at_ms = Some(99);
        run.outcome = RunOutcome::Closed;
        local
            .apply(vec![rows::run_owned_mutation(&run, &owner("yinz"))])
            .unwrap();

        assert!(
            rows::runs_from_rows(
                &local
                    .fetch(&rows::active_runs_request(&owner("yinz")))
                    .unwrap()
            )
            .is_empty(),
            "the run could not be closed"
        );
        let closed = rows::runs_from_rows(&local.fetch(&rows::run_by_id_request(&run.id)).unwrap());
        assert_eq!(closed[0].outcome, RunOutcome::Closed);
        assert_eq!(
            closed[0].host_id,
            HostId::new(HOST),
            "the close is also what claimed the row"
        );
    }

    /// And the case that must *not* be adopted, so the two predicates stay distinct. A latch or
    /// a summary that names no machine names a pane in a server that is gone, and is dropped
    /// rather than taken over — a stale latch is worth less than a wrong one.
    #[test]
    fn a_latch_that_predates_the_host_column_is_not_adopted() {
        use crate::store::{self, Backend};
        use yinz_tile::prelude::{FetchRequest, Mutation};

        let local = store::Local::shared("agent");
        local.define_all(&store::shared_defs()).unwrap();
        local
            .apply(vec![Mutation::put(rows::LATCH_ENTITY)
                .uid("yinz:5")
                .set("pane_id", 5_u32)
                .set("yinz_session_name", "yinz")
                .set("host_id", "")
                .set("kind", "reviewer")
                .build()])
            .expect("a latch from before the column");

        let mine = local
            .fetch(&rows::owned_rows_request(
                rows::LATCH_ENTITY,
                &owner("yinz"),
            ))
            .unwrap();
        assert!(
            rows::latches_from_rows(&mine).is_empty(),
            "a latch naming no machine is not this machine's to restore"
        );
        local.apply(rows::stale_unowned_delete_mutations()).unwrap();
        assert!(local
            .fetch(&FetchRequest::entity(rows::LATCH_ENTITY))
            .unwrap()
            .is_empty());
    }

    /// The interruption the feed exists to make in time. Shaped like `summary_toast`, so
    /// a user who has seen one recognises the other.
    #[test]
    fn a_waiting_toast_names_the_pane_and_what_it_is_asking() {
        let feed = |waiting_on: Option<&str>| HookFeed {
            phase: HookPhase::Waiting,
            detail: None,
            waiting_on: waiting_on.map(str::to_owned),
            session_id: "s1".to_owned(),
            cwd: None,
        };
        assert_eq!(
            waiting_toast(7, Some("claude◆"), &feed(Some("Bash cargo"))),
            "agent waiting · 7 · claude◆\nBash cargo"
        );
        // A wait we could not name is still worth interrupting for: the pane id is what
        // the user acts on, and the toast focuses that pane when it is clicked.
        assert_eq!(
            waiting_toast(7, Some("claude"), &feed(None)),
            "agent waiting · 7 · claude\nwaiting for you to answer"
        );
        // A pane whose classification failed keeps the id and loses the segment.
        assert_eq!(
            waiting_toast(9, None, &feed(Some("approve the plan"))),
            "agent waiting · 9\napprove the plan"
        );
    }

    /// The marker column is always there, so the list does not shift sideways as agents
    /// start and stop waiting - only what is *in* it changes.
    #[test]
    fn strip_rows_flag_only_the_agents_waiting_on_an_answer() {
        let panes = vec![
            pane(3, "codex", Some("codex")),
            pane(4, "claude", Some("claude")),
            pane(5, "aider", Some("aider")),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            // All three idle, so nothing but the answers decides who is flagged.
            &Activity::from([(3, (1, 2)), (4, (1, 2)), (5, (1, 2))]),
            &owners(&[]),
            &branches(&[]),
            // Pane 4 is blocked; pane 3 has a summary but is merely finished; pane 5 has
            // never gone idle and so has no summary at all.
            &summaries(&[(3, false), (4, true)]),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("no project"),
                branch("no branch"),
                agent(3, "  3  codex  idle"),
                // `waiting` rather than the `idle` its screen is: the row says one thing.
                waiting(4, "  4  claude  waiting"),
                agent(5, "  5  aider  idle"),
            ],
        );
    }

    /// A pane whose state is reported rather than inferred says so, in the one column
    /// that already qualifies the kind. Everything about that pane's row means something
    /// different from the rows around it — its `busy` is a fact and theirs is a guess —
    /// and there is no other way for a user to tell which they are looking at.
    #[test]
    fn a_pane_that_reports_for_itself_is_marked_as_such() {
        let mut remote = pane(5, "claude", Some("claude"));
        remote.host = Some("box".to_owned());
        let panes = vec![
            pane(3, "codex", Some("codex")),
            pane(4, "claude", Some("claude")),
            remote,
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::from([(3, (1, 2)), (4, (1, 2)), (5, (1, 2))]),
            &owners(&[]),
            &branches(&[]),
            &summaries(&[]),
            // The two claudes report; the codex beside them cannot.
            &feeds(&[(4, HookPhase::Idle), (5, HookPhase::Busy)]),
        );

        assert_eq!(
            rows,
            vec![
                project("no project"),
                branch("no branch"),
                agent(3, "  3  codex  idle"),
                agent(4, "  4  claude◆  idle"),
                // The marker qualifies the kind; the host stays the outermost suffix, so
                // it reads the way a host is written everywhere else.
                agent(5, "  5  claude◆@box  busy"),
            ],
        );

        // The summary box heads its pane the same way the row names it, minus the host:
        // the box is drawn beside a row that already says which machine.
        assert_eq!(
            rows.iter()
                .filter_map(|row| match row {
                    StripRow::Agent { pane_id, kind, .. } => Some((*pane_id, kind.as_str())),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![(3, "codex"), (4, "claude◆"), (5, "claude◆")],
        );
    }

    /// The marker cannot be confused for the one the agents pane looks for. That pane
    /// colours the `!` in a box header by *searching* the line for the character, so a
    /// marker sharing it would light up the wrong column — see `render_summary` there.
    #[test]
    fn the_hook_marker_is_not_the_waiting_marker() {
        assert!(!HOOK_MARKER.contains('!'));
        assert_eq!(kind_cell("claude", None, false), "claude");
        assert_eq!(kind_cell("claude", None, true), "claude◆");
        assert_eq!(kind_cell("claude", Some("box"), true), "claude◆@box");
        assert_eq!(kind_cell("claude", Some("box"), false), "claude@box");
    }

    /// A pane that has settled on a screen the stored summary was never made for — the
    /// hash it was made for belongs to whatever the pane showed before. Showing the marker
    /// here would tell the user (and the model) the summary describes the current screen
    /// when it does not.
    #[test]
    fn strip_rows_does_not_flag_a_pane_whose_summary_predates_its_current_screen() {
        let panes = vec![pane(4, "claude", Some("claude"))];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            // Idle on hash 2; the summary below was made for hash 1.
            &Activity::from([(4, (2, 3))]),
            &owners(&[]),
            &branches(&[]),
            &summaries(&[(4, true)]),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("no project"),
                branch("no branch"),
                agent(4, "  4  claude  idle")
            ],
        );
    }

    #[test]
    fn list_json_tells_the_model_what_the_strip_draws() {
        let json = list_json(
            &[
                pane(3, "codex", Some("codex")),
                pane(4, "aider", Some("aider")),
            ],
            &manual(&[]),
            &Activity::from([(3, (1, 2)), (4, (1, 2))]),
            &owners(&[]),
            &branches(&[]),
            &summaries(&[(3, true)]),
            &no_hooks(),
            |_| String::new(),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["summary"], "did a thing");
        assert_eq!(value[0]["awaiting_confirmation"], true);
        // An agent that has never gone idle says nothing rather than saying "false".
        assert!(value[1].get("summary").is_none());
        assert!(value[1].get("awaiting_confirmation").is_none());
    }

    /// `awaiting_confirmation` must not disagree with `waiting_marker` — a summary made for
    /// a screen the pane has since moved past does not describe the pane's current state.
    #[test]
    fn list_json_does_not_claim_a_stale_summary_is_awaiting_confirmation() {
        let json = list_json(
            &[pane(3, "codex", Some("codex"))],
            &manual(&[]),
            // Idle on hash 2; the summary below was made for hash 1.
            &Activity::from([(3, (2, 2))]),
            &owners(&[]),
            &branches(&[]),
            &summaries(&[(3, true)]),
            &no_hooks(),
            |_| String::new(),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        // The stale text is still surfaced (it is the last thing known), but not as an
        // answer to the pane's current screen.
        assert_eq!(value[0]["summary"], "did a thing");
        assert_eq!(value[0]["awaiting_confirmation"], false);
    }

    #[test]
    fn status_cell_grades_a_running_agent_by_how_long_its_pane_sat_still() {
        let cell = |polls| status_cell(AgentStatus::Running, Some(&(1, polls)), None);
        assert_eq!(status_cell(AgentStatus::Running, None, None), "running");
        assert_eq!(cell(0), "busy");
        assert_eq!(cell(11), "idle");
        assert_eq!(cell(12), "idle 1m");
        assert_eq!(cell(36), "idle 3m");
        // A pane that is not running has a better word than any guess.
        assert_eq!(status_cell(AgentStatus::Held, Some(&(1, 40)), None), "held");
    }

    /// The whole point of the feed: a pane whose agent reports for itself is read off
    /// that report, not off how long its picture has held still. A `Stop` names the
    /// pane idle on the poll it arrives, where the heuristic would have called the
    /// same reading `busy` and then spent three more polls settling.
    #[test]
    fn a_reporting_agent_is_read_off_its_own_report_not_off_its_screen() {
        let feed = |phase| HookFeed {
            phase,
            detail: None,
            waiting_on: None,
            session_id: "s1".to_owned(),
            cwd: None,
        };
        let just_moved = Some(&(1u64, 0u32));

        assert_eq!(
            status_cell(AgentStatus::Running, just_moved, None),
            "busy",
            "without a feed, a screen that just moved is a working agent"
        );
        assert_eq!(
            status_cell(
                AgentStatus::Running,
                just_moved,
                Some(&feed(HookPhase::Idle))
            ),
            "idle",
            "with one, the same reading is the start of an idle stretch"
        );
        assert_eq!(
            status_cell(
                AgentStatus::Running,
                just_moved,
                Some(&feed(HookPhase::Waiting))
            ),
            "waiting"
        );
        // The idle clock is still `activity`'s, so one pane's minutes read like
        // every other pane's.
        assert_eq!(
            status_cell(
                AgentStatus::Running,
                Some(&(1, 36)),
                Some(&feed(HookPhase::Idle))
            ),
            "idle 3m"
        );
        // A pane that is not running has a better word than any report about it.
        assert_eq!(
            status_cell(AgentStatus::Held, just_moved, Some(&feed(HookPhase::Busy))),
            "held"
        );
    }

    /// The tool hint rides in the status cell rather than in a variant of its own.
    #[test]
    fn a_running_tool_is_named_in_the_status_cell() {
        let busy = |detail: Option<&str>| {
            status_cell(
                AgentStatus::Running,
                Some(&(1, 0)),
                Some(&HookFeed {
                    phase: HookPhase::Busy,
                    detail: detail.map(str::to_owned),
                    waiting_on: None,
                    session_id: "s1".to_owned(),
                    cwd: None,
                }),
            )
        };
        assert_eq!(busy(None), "busy");
        assert_eq!(busy(Some("Bash cargo")), "busy · Bash cargo");
    }

    /// One hook event carrying nothing but its name.
    fn event(name: &str) -> crate::wire::HookEvent {
        crate::wire::HookEvent {
            hook_event_name: name.to_owned(),
            ..Default::default()
        }
    }

    /// One `Notification` of the kind named.
    fn notification(kind: &str) -> crate::wire::HookEvent {
        crate::wire::HookEvent {
            hook_event_name: "Notification".to_owned(),
            notification_type: Some(kind.to_owned()),
            ..Default::default()
        }
    }

    /// Every event we wire, and the states they name. The three that say nothing are
    /// as important as the ones that do: a notification about a quota reset must not
    /// draw a `!` beside a pane that is working.
    #[test]
    fn hook_events_map_to_the_states_they_name() {
        for source in ["startup", "resume", "fork", "clear"] {
            assert_eq!(
                hook_phase(
                    "claude",
                    &crate::wire::HookEvent {
                        source: Some(source.to_owned()),
                        ..event("SessionStart")
                    }
                ),
                Some(HookPhase::Idle)
            );
        }
        for source in ["compact", "something_new"] {
            assert_eq!(
                hook_phase(
                    "claude",
                    &crate::wire::HookEvent {
                        source: Some(source.to_owned()),
                        ..event("SessionStart")
                    }
                ),
                None
            );
        }
        assert_eq!(hook_phase("claude", &event("SessionStart")), None);
        assert_eq!(hook_phase("claude", &event("Stop")), Some(HookPhase::Busy));
        assert_eq!(
            hook_phase("claude", &event("UserPromptSubmit")),
            Some(HookPhase::Busy)
        );
        assert_eq!(
            hook_phase("claude", &event("PreToolUse")),
            Some(HookPhase::Busy)
        );
        assert_eq!(
            hook_phase("claude", &event("PostToolUse")),
            Some(HookPhase::Busy)
        );
        assert_eq!(
            hook_phase("claude", &event("PermissionRequest")),
            Some(HookPhase::Busy)
        );
        assert_eq!(
            hook_phase("claude", &notification("permission_prompt")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(
            hook_phase("claude", &notification("agent_needs_input")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(
            hook_phase("claude", &notification("worker_permission_prompt")),
            None
        );

        // A finished turn nobody has answered yet is idleness, not a question.
        assert_eq!(hook_phase("claude", &notification("idle_prompt")), None);
        assert_eq!(
            hook_phase("claude", &notification("quota_auto_resume_fired")),
            None
        );
        // Ending a session is not a state to be in - callers check HOOK_SESSION_END.
        assert_eq!(hook_phase("claude", &event(HOOK_SESSION_END)), None);
        // A version that grows an event we have never heard of costs us nothing.
        assert_eq!(
            hook_phase("claude", &event("SomethingNewInAYearsTime")),
            None
        );
    }

    /// A turn that dies on an API error has ended as surely as one that finished, and
    /// `StopFailure` is the only thing that says so. Without it the feed sits on `busy`
    /// until the user types again — and nothing here expires on a clock, so "until the
    /// user types again" means exactly that.
    #[test]
    fn a_turn_that_failed_ends_the_agents_work_like_one_that_finished() {
        let failed = crate::wire::HookEvent {
            hook_event_name: "StopFailure".to_owned(),
            last_assistant_message: Some("I was about to run the tests.".to_owned()),
            ..Default::default()
        };
        assert_eq!(hook_phase("claude", &failed), Some(HookPhase::Idle));
    }

    /// The two tools that are themselves a question. Every other tool starting is work;
    /// these do nothing but put a prompt up and wait, so a `PreToolUse` naming one is a
    /// wait rather than the busy every other `PreToolUse` is.
    #[test]
    fn a_tool_that_asks_the_user_something_is_a_wait_not_work() {
        let pre = |tool: &str| crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some(tool.to_owned()),
            ..Default::default()
        };
        assert_eq!(
            hook_phase("claude", &pre("AskUserQuestion")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(
            hook_phase("claude", &pre("ExitPlanMode")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(hook_phase("claude", &pre("Bash")), Some(HookPhase::Busy));

        // And the answer coming back is ordinary work again, which is what clears the
        // wait without either tool needing a case of its own on the way out.
        let post = crate::wire::HookEvent {
            hook_event_name: "PostToolUse".to_owned(),
            tool_name: Some("AskUserQuestion".to_owned()),
            ..Default::default()
        };
        assert_eq!(hook_phase("claude", &post), Some(HookPhase::Busy));
    }

    /// The events wired since the first eight, each in the state it names.
    #[test]
    fn the_rest_of_the_lifecycle_names_the_state_it_is_in() {
        for name in [
            "PostToolUseFailure",
            "PermissionDenied",
            "ElicitationResult",
            "PreCompact",
            "PostCompact",
        ] {
            assert_eq!(
                hook_phase("claude", &event(name)),
                Some(HookPhase::Busy),
                "{name} is the agent working"
            );
        }
        assert_eq!(
            hook_phase("claude", &event("Elicitation")),
            Some(HookPhase::Waiting)
        );
        for name in ["SubagentStart", "SubagentStop"] {
            assert_eq!(hook_phase("claude", &event(name)), None);
        }
        // A *background* session that finished, which is the pane's agent still working —
        // Claude Code raises this one from its background-job monitor, on the foreground
        // session's payload. The pane's own turn ending is `Stop`, which arrives earlier and
        // carries the words to describe it. See `agent::WAITING_NOTIFICATIONS`.
        assert_eq!(hook_phase("claude", &notification("agent_completed")), None);
    }

    /// One `PermissionRequest` for a tool, under a mode.
    fn request(tool: Option<&str>, mode: Option<&str>) -> crate::wire::HookEvent {
        crate::wire::HookEvent {
            hook_event_name: "PermissionRequest".to_owned(),
            tool_name: tool.map(str::to_owned),
            permission_mode: mode.map(str::to_owned),
            ..Default::default()
        }
    }

    /// Manual approval waits; automatic approval is work on both reporting paths.
    #[test]
    fn permission_phases_distinguish_manual_and_automatic_approval() {
        assert!(waiting_needs_a_person(&request(Some("Bash"), None)));
        assert_eq!(
            hook_phase("codex", &request(Some("Bash"), None)),
            Some(HookPhase::Waiting)
        );
        assert!(!waiting_needs_a_person(&request(
            Some("Bash"),
            Some("bypassPermissions")
        )));
        assert_eq!(
            hook_phase("claude", &request(Some("Bash"), Some("bypassPermissions"))),
            Some(HookPhase::Busy)
        );
    }

    /// What the user is actually being asked, from each of the four events that can
    /// say. This is what the toast carries, so it is worth pinning exactly.
    #[test]
    fn a_wait_says_what_it_is_waiting_for() {
        let ask = crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(json!({
                "questions": [{
                    "question": "Which library should we use for date formatting?",
                    "header": "Library",
                    "options": [{"label": "chrono", "description": "the usual one"}],
                }],
            })),
            ..Default::default()
        };
        assert_eq!(
            waiting_detail(&ask).as_deref(),
            Some("Which library should we use for date formatting?")
        );

        let plan = crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some("ExitPlanMode".to_owned()),
            ..Default::default()
        };
        assert_eq!(waiting_detail(&plan).as_deref(), Some("approve the plan"));

        let permission = crate::wire::HookEvent {
            hook_event_name: "PermissionRequest".to_owned(),
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(json!({"command": "cargo x test -p agent-proto"})),
            ..Default::default()
        };
        assert_eq!(waiting_detail(&permission).as_deref(), Some("Bash cargo"));

        let elicitation = crate::wire::HookEvent {
            hook_event_name: "Elicitation".to_owned(),
            message: Some("Pick a repository".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            waiting_detail(&elicitation).as_deref(),
            Some("Pick a repository")
        );

        // Nothing to say about an event that is not a wait.
        assert_eq!(waiting_detail(&event("Stop")), None);
        assert_eq!(waiting_detail(&event("PostToolUse")), None);
    }

    /// `tool_input` has no schema we control. A Claude Code that reshapes
    /// `AskUserQuestion`'s arguments must cost us the question's *text*, not the fact
    /// that a question was asked — and must never cost us a panic.
    #[test]
    fn a_question_we_cannot_read_still_says_a_question_was_asked() {
        let ask = |input: serde_json::Value| crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(input),
            ..Default::default()
        };
        for input in [
            json!({}),
            json!({"questions": []}),
            json!({"questions": "not an array"}),
            json!({"questions": [{}]}),
            json!({"questions": [{"question": ""}]}),
            json!({"questions": [{"question": 7}]}),
        ] {
            assert_eq!(
                waiting_detail(&ask(input.clone())).as_deref(),
                Some("a question"),
                "{input}"
            );
        }
        // The header is the fallback while there is still one to read.
        assert_eq!(
            waiting_detail(&ask(json!({"questions": [{"header": "Library"}]}))).as_deref(),
            Some("Library")
        );
    }

    /// The other half of the `tool_input` argument: text the agent's own input can
    /// steer, going into a toast. Newlines collapse, control characters go, and a long
    /// question is cut rather than allowed to fill the box.
    #[test]
    fn a_wait_detail_is_one_clean_line_of_bounded_length() {
        let ask = |question: &str| crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(json!({"questions": [{"question": question}]})),
            ..Default::default()
        };

        let detail =
            waiting_detail(&ask("  which\n\n\tone\x1b[2J  do   you\rwant?  ")).expect("a question");
        assert_eq!(detail, "which one[2J do you want?");
        assert!(!detail.contains('\n') && !detail.contains('\t'));

        // The cap counts the ellipsis: what it bounds is the width of the finished line,
        // not the length of what was kept before it was marked as cut.
        let long = waiting_detail(&ask(&"word ".repeat(80))).expect("a question");
        assert!(long.chars().count() <= WAITING_DETAIL_CHARS, "{long}");
        assert!(long.ends_with('…'));
        assert!(long.starts_with("word word"), "{long}");
    }

    /// The prompt names the pane the moment it is submitted, so what a person typed over
    /// several lines has to become one line a frame can hold.
    #[test]
    fn a_prompt_title_is_one_bounded_line_of_what_was_asked_for() {
        assert_eq!(
            prompt_title("  fix the flaky\n\treconnect\x1b[2J test  ").as_deref(),
            Some("fix the flaky reconnect[2J test")
        );

        // Cut to the same budget a waiting detail has, ellipsis included in it.
        let long = prompt_title(&"word ".repeat(80)).expect("a title");
        assert!(long.chars().count() <= WAITING_DETAIL_CHARS, "{long}");
        assert!(long.ends_with('…'), "{long}");

        // Nothing to name the pane after leaves its name alone, the same answer
        // `rename_title` gives an empty title.
        assert_eq!(prompt_title("   \n\t ").as_deref(), None);
        assert_eq!(prompt_title("").as_deref(), None);
    }

    /// Whatever the toast is given, the row gets the short form: a sidebar cell that
    /// carried the whole of a question would push every tab name off the end of the
    /// line beside it.
    #[test]
    fn a_waiting_row_carries_the_question_cut_to_the_sidebars_budget() {
        let waiting = |waiting_on: Option<&str>| {
            status_cell(
                AgentStatus::Running,
                Some(&(1, 0)),
                Some(&HookFeed {
                    phase: HookPhase::Waiting,
                    detail: None,
                    waiting_on: waiting_on.map(str::to_owned),
                    session_id: "s1".to_owned(),
                    cwd: None,
                }),
            )
        };
        assert_eq!(waiting(None), "waiting");
        assert_eq!(waiting(Some("Bash cargo")), "waiting · Bash cargo");

        let long = waiting(Some("Which library should we use for date formatting?"));
        assert!(long.starts_with("waiting · Which library"), "{long}");
        assert!(long.ends_with('…'), "{long}");
        assert_eq!(
            long.chars().count(),
            "waiting · ".chars().count() + HOOK_DETAIL_CHARS
        );
    }

    /// `tool_input` is free-form text an agent's own input can steer, so the little
    /// that is taken from it is cut short and stripped of anything that could move a
    /// cursor or break a row.
    #[test]
    fn a_tool_hint_is_short_and_holds_no_control_characters() {
        let pre = |tool: &str, input: serde_json::Value| crate::wire::HookEvent {
            hook_event_name: "PreToolUse".to_owned(),
            tool_name: Some(tool.to_owned()),
            tool_input: Some(input),
            ..Default::default()
        };

        assert_eq!(
            hook_detail(&pre(
                "Bash",
                json!({"command": "cargo x test -p agent-proto"})
            )),
            Some("Bash cargo".to_owned()),
            "the program, not the command line"
        );
        assert_eq!(
            hook_detail(&pre(
                "Edit",
                json!({"file_path": "/work/agent/src/main.rs"})
            )),
            Some("Edit main.rs".to_owned())
        );
        assert_eq!(
            hook_detail(&pre("WebFetch", json!({"url": "https://example/x"}))),
            Some("WebFetch".to_owned()),
            "a tool with no obvious argument is named on its own"
        );

        let nasty = hook_detail(&pre(
            "Bash",
            json!({"command": "\u{1b}[2Jrm-rf-and-a-very-long-argument-indeed"}),
        ))
        .expect("a hint");
        assert!(
            !nasty.chars().any(char::is_control),
            "no escape sequence survives: {nasty:?}"
        );
        assert!(nasty.chars().count() <= 5 + HOOK_DETAIL_CHARS, "{nasty:?}");

        let exact = "abcdefghijkl mnopqrst uvwx";
        assert_eq!(
            clean_hint(exact),
            Some("abcdefghijklmnopqrstuvwx".to_owned())
        );
        assert_eq!(
            clean_hint(&format!("{exact}0")),
            Some("abcdefghijklmnopqrstuvw…".to_owned())
        );

        // Only a tool *starting* has a hint; anything else clears it.
        assert_eq!(
            hook_detail(&crate::wire::HookEvent {
                hook_event_name: "PostToolUse".to_owned(),
                tool_name: Some("Bash".to_owned()),
                ..Default::default()
            }),
            None
        );
    }

    /// The `!` marker, and the two rules behind it. A summary describes one screen and
    /// outlives it, so it is believed only while the pane still shows that screen. A
    /// feed has no screen behind it - it is the state - so it is believed outright.
    #[test]
    fn the_waiting_marker_believes_a_feed_outright_and_a_summary_only_while_it_is_fresh() {
        let panes = vec![pane(3, "claude", Some("claude"))];
        let owners = owners(&[]);
        let branches = branches(&[]);

        // Waiting, with a screen that is still moving and no summary anywhere.
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::from([(3, (9, 0))]),
            &owners,
            &branches,
            &Summaries::new(),
            &feeds(&[(3, HookPhase::Waiting)]),
        );
        assert!(
            matches!(&rows[2], StripRow::Agent { text, .. } if text.starts_with("! ")),
            "{rows:?}"
        );

        // The same pane working: no marker, whatever a stale summary once said.
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::from([(3, (1, 4))]),
            &owners,
            &branches,
            &summaries(&[(3, true)]),
            &feeds(&[(3, HookPhase::Busy)]),
        );
        assert!(
            matches!(&rows[2], StripRow::Agent { text, .. } if text.starts_with("  ")),
            "a feed outranks the summary beside it: {rows:?}"
        );
    }

    /// A pane can be blocked on the user before anything has been written about it,
    /// which the summary path could never say - it only ever answered where a summary
    /// existed, and gated that on the screen not having moved since.
    #[test]
    fn list_json_reports_a_waiting_agent_that_has_no_summary_yet() {
        let json = list_json(
            &[pane(3, "claude", Some("claude"))],
            &manual(&[]),
            &Activity::from([(3, (9, 0))]),
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &feeds(&[(3, HookPhase::Waiting)]),
            |_| String::new(),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["awaiting_confirmation"], true);
        assert!(value[0].get("summary").is_none());
    }

    #[test]
    fn automatic_permissions_have_no_waiting_marker() {
        for kind in ["claude", "codex"] {
            let event = crate::wire::HookEvent {
                permission_mode: Some("auto".to_owned()),
                tool_name: Some("Bash".to_owned()),
                tool_input: Some(json!({"command": "git status"})),
                ..event("PermissionRequest")
            };
            let crate::agent::HookOutcome::Reported(record) =
                crate::agent::hook_outcome(kind, &event)
            else {
                panic!("permission request must report a record");
            };
            let hooks = HookFeeds::from([(3, HookFeed::from_agent(&record).unwrap())]);
            assert_eq!(hook_phase(kind, &event), Some(HookPhase::Busy));
            assert_eq!(
                status_cell(AgentStatus::Running, None, hooks.get(&3)),
                "busy · Bash git"
            );
            assert!(!awaiting_confirmation(
                &Summaries::new(),
                &Activity::new(),
                &hooks,
                AgentStatus::Running,
                3
            ));
            let json = list_json(
                &[pane(3, kind, Some(kind))],
                &manual(&[]),
                &Activity::new(),
                &owners(&[]),
                &branches(&[]),
                &Summaries::new(),
                &hooks,
                |_| String::new(),
            );
            let value: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value[0]["state"], "busy · Bash git");
            assert_eq!(value[0]["awaiting_confirmation"], false);
        }
    }

    #[test]
    fn a_non_running_pane_does_not_wait_on_a_stale_feed() {
        let mut held = pane(3, "claude", Some("claude"));
        held.status = AgentStatus::Held;
        let panes = [held];
        let hooks = feeds(&[(3, HookPhase::Waiting)]);
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &hooks,
        );
        assert!(matches!(&rows[2], StripRow::Agent { text, .. } if text.starts_with("  ")));

        let json = list_json(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &hooks,
            |_| String::new(),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["state"], "held");
        assert_eq!(value[0]["awaiting_confirmation"], false);
    }

    #[test]
    fn mention_context_resolves_ids_and_kinds_and_skips_what_it_cannot() {
        let tabbed = AgentPane {
            tab: Some((TabId::new(9), "work".to_owned())),
            ..pane(3, "codex", Some("codex"))
        };
        let panes = [tabbed, pane(5, "zsh", Some("zsh"))];
        let kinds = manual(&[(5, "reviewer")]);
        let activity = Activity::from([(3, (1, 24)), (5, (1, 0))]);

        assert_eq!(
            mention_context(
                "ask @codex and @5, please",
                &panes,
                &kinds,
                &activity,
                &no_hooks()
            ),
            Some(
                "[@codex is agent pane 3 (codex, idle 2m) on tab \"work\"]\n\
                 [@5 is agent pane 5 (reviewer, busy)]"
                    .to_owned()
            ),
        );
        // An id mention, a duplicate, and punctuation after the body.
        assert_eq!(
            mention_context("@3 @codex again", &panes, &kinds, &activity, &no_hooks(),),
            Some("[@3 is agent pane 3 (codex, idle 2m) on tab \"work\"]".to_owned()),
        );
        // Unresolved kinds, non-agent panes, embedded @ and bare @ say nothing.
        assert_eq!(
            mention_context("@aider @7 a@b @", &panes, &kinds, &activity, &no_hooks(),),
            None
        );
        assert_eq!(
            mention_context("no mentions here", &panes, &kinds, &activity, &no_hooks(),),
            None
        );
    }

    #[test]
    fn mention_candidates_are_each_kind_once_then_every_pane_id() {
        let panes = [
            pane(3, "codex", Some("codex")),
            pane(4, "codex", Some("codex")),
            pane(5, "zsh", Some("zsh")),
        ];
        assert_eq!(
            mention_candidates(&panes, &manual(&[(5, "reviewer")])),
            vec!["codex", "reviewer", "3", "4", "5"],
        );
    }

    #[test]
    fn strip_rows_name_the_pane_and_end_with_its_tab_when_it_has_one() {
        let tabbed = AgentPane {
            tab: Some((TabId::new(9), "work".to_owned())),
            ..pane(3, "codex", Some("codex"))
        };
        let rows = strip_rows(
            &[tabbed, pane(5, "aider", Some("aider"))],
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &no_hooks(),
        );
        // Nothing to group by, and the headers say so rather than going missing:
        // a row with no header above it would read as belonging to the one above
        // that.
        assert_eq!(
            rows,
            vec![
                project("no project"),
                branch("no branch"),
                agent(3, "  3  codex  running  work"),
                agent(5, "  5  aider  running"),
            ],
        );
    }

    #[test]
    fn list_json_carries_the_tab_when_the_pane_has_one() {
        let tabbed = AgentPane {
            tab: Some((TabId::new(9), "work".to_owned())),
            ..pane(3, "codex", Some("codex"))
        };
        let json = list_json(
            &[tabbed],
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &no_hooks(),
            |_| String::new(),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["tab_id"], 9);
        assert_eq!(value[0]["tab"], "work");
    }

    #[test]
    fn list_json_reads_each_agent_once_and_no_other_pane() {
        let panes = vec![
            pane(3, "codex", Some("codex")),
            pane(4, "zsh", Some("zsh")),
            AgentPane {
                status: AgentStatus::Held,
                ..pane(5, "shell", Some("zsh"))
            },
        ];
        let mut reads = Vec::new();
        // Pane 5 has activity too, but it is held, so no idle_seconds for it.
        let activity = Activity::from([(3, (1, 24)), (5, (1, 2))]);
        let json = list_json(
            &panes,
            &manual(&[(5, "reviewer")]),
            &activity,
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &no_hooks(),
            |pane_id| {
                reads.push(pane_id);
                "ready".to_owned()
            },
        );

        assert_eq!(reads, vec![3, 5]);
        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            serde_json::json!([
                {"idle_seconds":120,"kind":"codex","pane_id":3,"preview":"ready","source":"auto","status":"running","title":"codex"},
                {"kind":"reviewer","pane_id":5,"preview":"ready","source":"manual","status":"held","title":"shell"}
            ]),
        );
    }

    fn in_folder(pane_id: u32, kind: &str, cwd: &str, tab_id: TabId) -> AgentPane {
        AgentPane {
            cwd: Some(cwd.to_owned()),
            tab: Some((tab_id, format!("tab {tab_id}"))),
            ..pane(pane_id, kind, Some(kind))
        }
    }

    #[test]
    fn list_json_carries_the_folder_the_project_and_the_branch() {
        let pane = in_folder(3, "codex", "/work/agent/src", TabId::new(9));
        let json = list_json(
            &[pane],
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "agent")]),
            &branches(&[(3, "topic")]),
            &Summaries::new(),
            &no_hooks(),
            |_| String::new(),
        );

        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            serde_json::json!([{"branch":"topic","cwd":"/work/agent/src","kind":"codex","pane_id":3,"preview":"","project":"agent","source":"auto","status":"running","tab":"tab 9","tab_id":9,"title":"codex"}]),
        );
    }

    #[test]
    fn strip_rows_group_agents_under_their_project() {
        let panes = vec![
            in_folder(3, "codex", "/work/pocus", TabId::new(1)),
            in_folder(4, "aider", "/elsewhere", TabId::new(1)),
            in_folder(5, "codex", "/work/agent", TabId::new(2)),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "pocus"), (5, "agent")]),
            &branches(&[(3, "main"), (5, "main")]),
            &Summaries::new(),
            &no_hooks(),
        );

        // Projects in name order with the unowned agent last, and no row
        // repeating what the headers above it already say.
        assert_eq!(
            rows,
            vec![
                project("agent"),
                branch("main"),
                agent(5, "  5  codex  running  tab 2"),
                project("pocus"),
                branch("main"),
                agent(3, "  3  codex  running  tab 1"),
                project("no project"),
                branch("no branch"),
                agent(4, "  4  aider  running  tab 1"),
            ],
        );
    }

    /// The case the branch header exists for: one project, two worktrees. The
    /// project is named once and the agents of each checkout sit together.
    #[test]
    fn strip_rows_group_the_worktrees_of_one_project_by_branch() {
        let panes = vec![
            in_folder(3, "codex", "/work/agent", TabId::new(1)),
            in_folder(4, "claude", "/work/trees/topic", TabId::new(2)),
            in_folder(5, "aider", "/work/agent", TabId::new(1)),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "agent"), (4, "agent"), (5, "agent")]),
            &branches(&[(3, "main"), (4, "topic"), (5, "main")]),
            &Summaries::new(),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("agent"),
                branch("main"),
                agent(3, "  3  codex  running  tab 1"),
                agent(5, "  5  aider  running  tab 1"),
                branch("topic"),
                agent(4, "  4  claude  running  tab 2"),
            ],
        );
    }

    /// An agent on another machine has to say so on its own line. Which machine
    /// you are about to send input to is not decoration — two agents in the same
    /// folder on different hosts are doing different work.
    #[test]
    fn strip_rows_name_the_host_of_a_remote_agent() {
        let mut remote = pane(3, "", Some("claude"));
        remote.host = Some("build-box".to_owned());
        let local = pane(4, "", Some("claude"));

        let rows = strip_rows(
            &[remote, local],
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[]),
            &summaries(&[]),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("no project"),
                branch("no branch"),
                agent(3, "  3  claude@build-box  running"),
                agent(4, "  4  claude  running"),
            ],
        );
    }

    /// A checkout nobody named still has a branch, and an agent in one is still
    /// worth telling apart from an agent in the next.
    #[test]
    fn strip_rows_group_by_branch_without_a_project() {
        let panes = vec![
            in_folder(3, "codex", "/work/one", TabId::new(1)),
            in_folder(4, "codex", "/work/two", TabId::new(2)),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[(3, "main"), (4, "topic")]),
            &Summaries::new(),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("no project"),
                branch("main"),
                agent(3, "  3  codex  running  tab 1"),
                branch("topic"),
                agent(4, "  4  codex  running  tab 2"),
            ],
        );
    }

    /// An agent whose folder has not been asked about yet has no branch to
    /// group under, and waits under the header that says so rather than joining
    /// whichever branch happens to be nearby. The shape matters: this is what
    /// the picker draws for the round trip between a pane appearing in a
    /// worktree and Git answering about it, and it must not read as a claim.
    #[test]
    fn an_agent_whose_branch_is_not_known_yet_waits_under_its_own_header() {
        let panes = vec![
            in_folder(3, "codex", "/work/agent", TabId::new(1)),
            in_folder(
                4,
                "claude",
                "/work/agent/.claude/worktrees/topic",
                TabId::new(2),
            ),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "agent"), (4, "agent")]),
            // Pane 4's folder is in the project, but no probe has answered for
            // it yet.
            &branches(&[(3, "main")]),
            &Summaries::new(),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("agent"),
                branch("main"),
                agent(3, "  3  codex  running  tab 1"),
                branch("no branch"),
                agent(4, "  4  claude  running  tab 2"),
            ],
        );
    }

    #[test]
    fn tab_projects_agree_only_when_one_project_owns_the_tab() {
        let untabbed = AgentPane {
            cwd: Some("/work/agent".to_owned()),
            ..pane(9, "codex", Some("codex"))
        };
        let panes = vec![
            // Tab 1 agrees, even though pane 4 has no known project.
            in_folder(3, "codex", "/work/agent", TabId::new(1)),
            in_folder(4, "aider", "/elsewhere", TabId::new(1)),
            // Tab 2 has two projects, so it agrees on nothing.
            in_folder(5, "codex", "/work/agent", TabId::new(2)),
            in_folder(6, "aider", "/work/pocus", TabId::new(2)),
            // Tab 3 has a shell, not an agent, and pane 9 is in no tab at all.
            in_folder(7, "zsh", "/work/agent", TabId::new(3)),
            untabbed,
        ];
        let owners = owners(&[
            (3, "agent"),
            (5, "agent"),
            (6, "pocus"),
            (7, "agent"),
            (9, "agent"),
        ]);

        assert_eq!(
            tab_projects(&panes, &manual(&[]), &owners),
            BTreeMap::from([(TabId::new(1), "agent".to_owned())]),
        );
    }

    #[test]
    fn tab_project_labels_name_a_tab_after_every_project_in_it() {
        let untabbed = AgentPane {
            cwd: Some("/work/agent".to_owned()),
            ..pane(9, "codex", Some("codex"))
        };
        let panes = vec![
            // One project, named once however many agents are on it.
            in_folder(3, "codex", "/work/agent", TabId::new(1)),
            in_folder(4, "aider", "/work/agent", TabId::new(1)),
            // Two projects, sorted rather than left in pane order.
            in_folder(5, "codex", "/work/pocus", TabId::new(2)),
            in_folder(6, "aider", "/work/agent", TabId::new(2)),
            // A shell names its tab as readily as an agent does — which is the
            // whole point, since the ordinary session has no agent in it. Pane 9
            // is in no tab at all, and still names nothing.
            in_folder(7, "zsh", "/work/agent", TabId::new(3)),
            untabbed,
        ];
        let owners = owners(&[
            (3, "agent"),
            (4, "agent"),
            (5, "pocus"),
            (6, "agent"),
            (7, "agent"),
            (9, "agent"),
        ]);

        assert_eq!(
            tab_project_labels(
                &panes,
                &owners
                    .iter()
                    .map(|(id, name)| (*id, (name.clone(), None)))
                    .collect()
            ),
            BTreeMap::from([
                (TabId::new(1), vec![("agent".to_owned(), None)]),
                (
                    TabId::new(2),
                    vec![("agent".to_owned(), None), ("pocus".to_owned(), None)]
                ),
                (TabId::new(3), vec![("agent".to_owned(), None)]),
            ]),
        );
    }

    /// The other half of the split from `tab_projects`, which counts only agents
    /// because an assignment says whose *work* a tab is doing. A name says where
    /// the tab is, and a shell answers that.
    #[test]
    fn only_the_label_counts_a_shell_the_assignment_does_not() {
        let panes = vec![in_folder(7, "zsh", "/work/agent", TabId::new(3))];
        let owners = owners(&[(7, "agent")]);

        assert_eq!(
            tab_project_labels(
                &panes,
                &owners
                    .iter()
                    .map(|(id, name)| (*id, (name.clone(), None)))
                    .collect()
            ),
            BTreeMap::from([(TabId::new(3), vec![("agent".to_owned(), None)])]),
        );
        assert!(tab_projects(&panes, &manual(&[]), &owners).is_empty());
    }

    #[test]
    fn all_projects_that_fit_are_named() {
        let named = |names: &[&str]| -> Vec<(String, Option<String>)> {
            names
                .iter()
                .map(|name| ((*name).to_owned(), None))
                .collect()
        };

        assert_eq!(tab_project_label(&named(&[])), None);
        assert_eq!(tab_project_label(&named(&["agent"])).unwrap(), "agent");
        assert_eq!(
            tab_project_label(&named(&["agent", "pocus"])).unwrap(),
            "agent + pocus"
        );
        assert_eq!(
            tab_project_label(&named(&["agent", "pocus", "focus", "locus"])).unwrap(),
            "agent + pocus + focus + locus"
        );
    }

    #[test]
    fn branch_labels_are_sorted_and_deduplicated() {
        let panes = (1..=5)
            .map(|id| in_folder(id, "zsh", "/work", TabId::new(1)))
            .collect::<Vec<_>>();
        let owners = BTreeMap::from([
            (1, ("project".into(), Some("b".into()))),
            (2, ("project".into(), Some("a".into()))),
            (3, ("project".into(), Some("b".into()))),
            (4, ("other".into(), Some("main".into()))),
            (5, ("project".into(), None)),
        ]);
        let labels = tab_project_labels(&panes, &owners);
        assert_eq!(labels[&TabId::new(1)].len(), 4);
        assert_eq!(
            tab_project_label(&labels[&TabId::new(1)]).unwrap(),
            "other:main + project + a + b"
        );
        assert_eq!(tab_project_labels(panes.iter().rev(), &owners), labels);
    }

    #[test]
    fn branch_summary_fits_complete_entries_and_reserves_the_count() {
        let cases = [
            (vec![("project", Some("a"))], "project:a".to_owned()),
            (
                vec![
                    ("project", Some("a")),
                    ("project", Some("b")),
                    ("project", Some("c")),
                ],
                "project:a + b + c".into(),
            ),
            (
                vec![("project", Some("a")), ("other", Some("b"))],
                "project:a + other:b".into(),
            ),
            (
                vec![
                    ("1234567890123456789012345678901234567", None),
                    ("ab", None),
                ],
                "1234567890123456789012345678901234567 +1".into(),
            ),
            (
                vec![("123456789012345678901234567890123456", None), ("a", None)],
                "123456789012345678901234567890123456 + a".into(),
            ),
            (
                vec![("1234567890123456789012345678901234567890", None)],
                "1234567890123456789012345678901234567890".into(),
            ),
            (
                vec![("12345678901234567890123456789012345678901", None)],
                "123456789012345678901234567890123456789…".into(),
            ),
            (
                vec![
                    ("12345678901234567890123456789012345678901", None),
                    ("b", None),
                    ("c", None),
                ],
                "123456789012345678901234567890123456… +2".into(),
            ),
            (
                vec![("日本語", Some("á")), ("日本語", Some("界"))],
                "日本語:á + 界".into(),
            ),
            (
                vec![
                    ("日本語日本語日本語日本語日本語日本語日本語", None),
                    ("z", None),
                ],
                format!("{}… +1", "日本語".repeat(6)),
            ),
        ];
        for (entries, expected) in cases {
            let entries = entries
                .into_iter()
                .map(|(p, b)| (p.to_owned(), b.map(str::to_owned)))
                .collect::<Vec<_>>();
            let label = tab_project_label(&entries).unwrap();
            assert_eq!(label, expected);
            assert!(label.width() <= 40);
        }
        let entries = vec![("x".repeat(50), None); 12];
        assert_eq!(
            tab_project_label(&entries).unwrap(),
            format!("{}… +11", "x".repeat(35))
        );
    }

    #[test]
    fn a_tab_whose_agents_have_no_project_is_left_out() {
        let panes = vec![in_folder(3, "codex", "/elsewhere", TabId::new(1))];

        assert!(tab_project_labels(&panes, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn a_tab_that_two_projects_disagree_over_stays_unassigned() {
        // Pane 7 agrees with pane 5, but pane 6 already broke the tab. Order
        // must not be able to heal it.
        let panes = vec![
            in_folder(5, "codex", "/work/agent", TabId::new(1)),
            in_folder(6, "aider", "/work/pocus", TabId::new(1)),
            in_folder(7, "codex", "/work/agent", TabId::new(1)),
        ];
        let owners = owners(&[(5, "agent"), (6, "pocus"), (7, "agent")]);

        assert!(tab_projects(&panes, &manual(&[]), &owners).is_empty());
    }

    // -- the idle summary in the history half --------------------------------

    /// History rows as they read, for the assertions that are about the words rather than
    /// about how they are painted.
    fn texts(lines: &[MdLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    fn a_summary(text: &str, awaiting: bool) -> Summary {
        Summary {
            text: text.to_owned(),
            title: "running the tests".to_owned(),
            awaiting,
            hash: 1,
            created_at_ms: None,
        }
    }

    /// The model's words, wrapped to the half and nothing else: no frame, no ceiling. The
    /// box this replaced cut at six rows because it had six rows to give; the half scrolls.
    #[test]
    fn an_idle_summary_is_the_model_s_words_wrapped_to_the_half() {
        let history = summary_history(
            7,
            "claude",
            &a_summary("Ran the tests, all green.", false),
            false,
            30,
            0,
        );
        assert_eq!(history.label, "7 · claude");
        assert_eq!(texts(&history.entries), ["Ran the tests, all green."]);

        let long = "word ".repeat(200);
        let long = summary_history(7, "claude", &a_summary(&long, false), false, 30, 0);
        assert!(
            long.entries.len() > 6,
            "every line of it, for the half to window, past the box's old ceiling of {}",
            long.entries.len()
        );
        // Uncapped, unlike a run's events: there is one of these and nothing beside it
        // competing for the room. It is still one event, so a cursor can open it in full.
        assert!(long.entries.len() > MAX_ENTRY_ROWS);
        assert_eq!(long.events.len(), 1);
        assert_eq!(long.events[0].start, 0);
        assert_eq!(long.events[0].len, long.entries.len());
        assert_eq!(long.events[0].kind, "summary");
    }

    /// One event gets four rows of the timeline and no more, the last of them saying how much
    /// was left out. Before the cap a single talkative result filled the half and pushed
    /// everything either side of it out of view, which is the opposite of what a timeline is
    /// for.
    #[test]
    fn a_long_event_is_capped_at_four_rows_that_name_what_is_missing() {
        let mut run = a_pane_run();
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            // Ten lines of it at this width, against the four the cap allows.
            text: "word ".repeat(50),
            created_at_ms: None,
        }];

        let history = run_history(&run, 30, 100, 0);
        assert_eq!(history.entries.len(), MAX_ENTRY_ROWS);
        // Three rows kept, and the marker speaks for everything from the fourth on.
        assert_eq!(
            history.entries.last().expect("four rows").text.trim(),
            CAPPED_MARKER
        );

        // The event keeps the whole of what it said, which is what makes the cap honest: the
        // pane opens this rather than the three rows it drew.
        assert_eq!(history.events.len(), 1);
        assert_eq!(&*history.events[0].text, run.entries[0].text);
        assert_eq!(history.events[0].kind, "prompt");
        assert_eq!(history.events[0].len, MAX_ENTRY_ROWS);
    }

    /// A run shaped like a real one: every kind the timeline draws, a harness prompt that is
    /// drawn as its digest, an entry too long for the cap, entries with and without stamps, and
    /// a rolling summary.
    fn a_mixed_run() -> AgentRun {
        let mut run = a_pane_run();
        let entry = |revision: u64, kind: RunEntryKind, text: String, created_at_ms| RunEntry {
            tool: None,
            revision,
            event_id: None,
            kind,
            text,
            created_at_ms,
        };
        run.entries = vec![
            entry(
                1,
                RunEntryKind::Prompt,
                "fix the **flaky** test".to_owned(),
                Some(0),
            ),
            entry(2, RunEntryKind::Prompt, task_notification(3), Some(60_000)),
            entry(
                3,
                RunEntryKind::Plan,
                "- one\n- two\n\n# Then\n\ndone".to_owned(),
                None,
            ),
            entry(
                4,
                RunEntryKind::Message,
                "word ".repeat(80),
                Some(3_600_000),
            ),
            entry(
                5,
                RunEntryKind::Summary,
                "Fixed it.".to_owned(),
                Some(7_200_000),
            ),
        ];
        run.summary = Some(a_summary("Fixed it, and ran the suite.", false));
        run
    }

    /// Keeping the parse is only allowed to change what the half costs, never what it draws:
    /// one `ParsedRun` walked through several widths draws exactly what a fresh `run_history`
    /// draws at each of them, and a later minute re-reads every age.
    #[test]
    fn a_kept_parse_draws_what_a_fresh_one_draws_at_every_width() {
        let run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        for (width, now) in [(80, 100), (30, 100), (57, 90_000_000), (80, 100)] {
            assert_eq!(
                parsed.lines(width, now, 0),
                run_history(&run, width, now, 0),
                "width {width} at {now}"
            );
        }
        // Resizing and the clock re-wrap and re-stamp; they never parse.
        assert_eq!(parsed.parses, 6);
    }

    /// A run that grew by an entry costs the parse of that entry, and draws what the whole
    /// run parsed afresh draws.
    #[test]
    fn a_grown_run_parses_only_what_is_new() {
        let mut run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let _ = parsed.lines(60, 100, 0);
        let before = parsed.parses;

        run.entries.push(RunEntry {
            tool: None,
            revision: 6,
            event_id: None,
            kind: RunEntryKind::Question,
            text: "Shall I push?".to_owned(),
            created_at_ms: Some(8_000_000),
        });
        parsed.update(&run);
        assert_eq!(parsed.lines(60, 100, 0), run_history(&run, 60, 100, 0));
        assert_eq!(parsed.parses - before, 1, "only the question");
    }

    /// A revision that comes back a different length is parsed again rather than drawn as what
    /// it used to say - and so is a rolling summary that changed. Only the length is read of a
    /// recorded entry (see `ParsedEvent::is`), so the rewrite here is deliberately not the
    /// length it was.
    #[test]
    fn an_entry_that_changed_is_parsed_again() {
        let mut run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let _ = parsed.lines(60, 100, 0);
        let before = parsed.parses;

        run.entries[0].text = "fix the *other* test".to_owned();
        assert_ne!(
            run.entries[0].text.len(),
            a_mixed_run().entries[0].text.len()
        );
        run.summary = Some(a_summary("Something else entirely.", true));
        parsed.update(&run);
        assert_eq!(parsed.lines(60, 100, 0), run_history(&run, 60, 100, 0));
        assert_eq!(parsed.parses - before, 2);

        // And one the run dropped is dropped here too.
        run.entries.remove(1);
        parsed.update(&run);
        assert_eq!(parsed.lines(60, 100, 0), run_history(&run, 60, 100, 0));
    }

    /// The rolling summary is rewritten in place, so it is recognised by what it says and not
    /// by its length: one that came back the same size saying something else is parsed again.
    #[test]
    fn a_summary_rewritten_to_the_same_length_is_parsed_again() {
        let mut run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let _ = parsed.lines(60, 100, 0);
        let before = parsed.parses;

        let old = run.summary.as_ref().expect("a summary").text.clone();
        let new = "Broke it, and ran the suite.";
        assert_eq!(old.len(), new.len());
        run.summary = Some(a_summary(new, false));
        parsed.update(&run);
        assert_eq!(parsed.lines(60, 100, 0), run_history(&run, 60, 100, 0));
        assert_eq!(parsed.parses - before, 1, "only the summary");
    }

    /// Entries are recognised by revision, and revisions are only unique within a run, so a
    /// parse handed another run starts over: its revision 1 is not this one's, however alike
    /// the two look from outside.
    #[test]
    fn another_run_is_parsed_afresh_however_alike_its_revisions_are() {
        let run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let _ = parsed.lines(60, 100, 0);

        let mut other = run.clone();
        other.id = "claude:session-2".to_owned();
        other.entries[0].text = "fix the **stale** test".to_owned();
        assert_eq!(other.entries[0].text.len(), run.entries[0].text.len());
        parsed.update(&other);
        assert_eq!(parsed.lines(60, 100, 0), run_history(&other, 60, 100, 0));
        assert!(parsed
            .lines(60, 100, 0)
            .entries
            .iter()
            .any(|line| line.text.contains("stale")));
    }

    /// A stamp is read by the gutter and nothing else, so an entry whose stamp was corrected
    /// keeps its parse - and is still drawn with the age the new stamp gives it.
    #[test]
    fn a_restamped_entry_is_not_parsed_again() {
        let mut run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let _ = parsed.lines(60, 90_000_000, 0);
        let before = parsed.parses;

        // A new age, a stamp where there was none - which widens the gutter the rows were
        // wrapped beside - and the same for the rolling summary.
        run.entries[0].created_at_ms = Some(89_000_000);
        run.entries[2].created_at_ms = Some(60_000);
        run.summary.as_mut().expect("a summary").created_at_ms = Some(80_000_000);
        parsed.update(&run);
        assert_eq!(
            parsed.lines(60, 90_000_000, 0),
            run_history(&run, 60, 90_000_000, 0)
        );
        assert_eq!(parsed.parses, before);
    }

    /// A minute on, the timeline restamped in place is the one drawn afresh at that minute -
    /// and one of another shape than the parse would draw is refused and left as it was.
    #[test]
    fn a_restamped_timeline_is_the_one_drawn_at_the_new_minute() {
        let run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let mut lines = parsed.lines(60, 100, 0);
        for now in [90_000_000, 90_060_000, 200_000_000, 200_000_000] {
            assert!(parsed.restamp(&mut lines, now), "at {now}");
            assert_eq!(lines, run_history(&run, 60, now, 0), "at {now}");
        }

        let mut other = HistoryLines::default();
        assert!(!parsed.restamp(&mut other, 5));
        assert_eq!(other, HistoryLines::default());
    }

    /// The timeline's parse is handed to a reader only where it is the whole of the event's
    /// text - and only for that very event, not another holding the same words.
    #[test]
    fn only_an_event_parsed_whole_shares_its_parse() {
        let run = a_mixed_run();
        let mut parsed = ParsedRun::new(&run);
        let lines = parsed.lines(60, 100, 0);
        let options = crate::markdown::Options::compact();

        let blocks = parsed
            .whole_blocks(&lines.events[0].text)
            .expect("a short prompt is parsed whole");
        assert_eq!(
            crate::markdown::wrap(&blocks, 40, options),
            crate::markdown::render(&run.entries[0].text, 40, options)
        );
        assert!(
            parsed.whole_blocks(&lines.events[1].text).is_none(),
            "a prompt drawn as its digest"
        );
        assert!(
            parsed
                .whole_blocks(&Arc::from(&*lines.events[0].text))
                .is_none(),
            "the same words, somewhere else"
        );

        let mut long = a_pane_run();
        long.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Message,
            text: "word ".repeat(ENTRY_HEAD_BYTES),
            created_at_ms: None,
        }];
        let mut parsed = ParsedRun::new(&long);
        let lines = parsed.lines(60, 100, 0);
        assert!(
            parsed.whole_blocks(&lines.events[0].text).is_none(),
            "an event parsed only in part"
        );
    }

    /// An entry that is one enormous block - a paragraph, a code fence, a table - is parsed
    /// and wrapped only as far as the timeline draws it, and is still capped and still opens
    /// whole. The cap used to stop only between blocks, so an entry like these was wrapped in
    /// full on every resize step and then cut to four rows.
    #[test]
    fn one_enormous_block_is_parsed_only_as_far_as_its_head() {
        let table = std::iter::once("| id | what |\n|---|---|".to_owned())
            .chain((0..4_000).map(|n| format!("| {n} | {} |", "cell ".repeat(4))))
            .collect::<Vec<_>>()
            .join("\n");
        let fence = format!(
            "```\n{}\n```",
            (0..5_000)
                .map(|n| format!("let line_{n} = {n};"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        for (shape, text) in [
            ("paragraph", "word ".repeat(20_000)),
            ("fence", fence),
            ("table", table),
        ] {
            assert!(text.len() >= 100_000, "{shape} is {} bytes", text.len());
            let mut run = a_pane_run();
            run.entries = vec![RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Message,
                text: text.clone(),
                created_at_ms: None,
            }];

            let parsed = ParsedRun::new(&run);
            assert!(
                parsed.parsed_chars() <= ENTRY_HEAD_BYTES,
                "{shape} kept {} characters",
                parsed.parsed_chars()
            );

            let history = run_history(&run, 60, 100, 0);
            assert_eq!(history.entries.len(), MAX_ENTRY_ROWS, "{shape}");
            assert_eq!(
                history.entries.last().expect("four rows").text.trim(),
                CAPPED_MARKER,
                "{shape}"
            );
            assert_eq!(&*history.events[0].text, text, "{shape} opens whole");
        }
    }

    /// An event parsed only in part says there is more even when its head fits: what was
    /// parsed is not all it said. Whole, the same rows are left alone.
    #[test]
    fn a_partly_parsed_event_is_capped_however_short_its_head() {
        let rows = vec![MdLine::plain("one"), MdLine::plain("two")];
        let whole = cap_rows(rows.clone(), 2, true);
        assert_eq!(whole, rows);

        let part = cap_rows(rows, 2, false);
        assert_eq!(part.len(), 3);
        assert_eq!(part[..2], whole[..]);
        assert_eq!(part[2].text.trim(), CAPPED_MARKER);
    }

    /// A head is only ever a saving: on text short enough to be parsed whole, the timeline is
    /// the one the whole parse draws, at every width.
    #[test]
    fn a_parsed_head_draws_what_the_whole_parse_draws() {
        let run = a_mixed_run();
        for width in [20, 30, 57, 80] {
            let Some(inner) = wrap_width(width) else {
                continue;
            };
            for entry in &run.entries {
                let whole = crate::markdown::parse(&entry.text);
                let (head, complete) =
                    crate::markdown::parse_head(&entry.text, ENTRY_HEAD_BLOCKS, ENTRY_HEAD_BYTES);
                let draw = |blocks: &[crate::markdown::MdBlock], complete| {
                    cap_rows(
                        labelled_blocks(blocks, "message", 3, inner, LABEL_STYLE).0,
                        3,
                        complete,
                    )
                };
                assert_eq!(
                    draw(&head, complete),
                    draw(&whole, true),
                    "{:?} at {width}",
                    entry.text
                );
            }
        }
    }

    /// An event that fits is left alone: the marker costs a row, so spending one to say "one
    /// more line" would show less than saying nothing.
    #[test]
    fn an_event_that_fits_the_cap_grows_no_marker() {
        let mut run = a_pane_run();
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            // A list, so the label takes a row of its own: exactly the four the cap allows.
            text: "- one\n- two\n- three".to_owned(),
            created_at_ms: None,
        }];
        let history = run_history(&run, 40, 100, 0);
        assert_eq!(history.entries.len(), MAX_ENTRY_ROWS);
        assert!(
            !history.entries.last().unwrap().text.contains('…'),
            "{:?}",
            texts(&history.entries)
        );
    }

    /// The spans tile the rows: every row of the half belongs to exactly one event, so a
    /// cursor on an event and a window over the lines can be read against each other.
    #[test]
    fn the_event_spans_tile_the_rows_they_were_drawn_from() {
        let mut run = a_pane_run();
        run.summary = Some(a_summary("Ran the tests, all green.", false));
        run.entries = vec![
            RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: "word ".repeat(50),
                created_at_ms: Some(40),
            },
            RunEntry {
                tool: None,
                revision: 2,
                event_id: None,
                kind: RunEntryKind::Question,
                text: "Ship it?".to_owned(),
                created_at_ms: None,
            },
        ];

        let history = run_history(&run, 40, 100, 0);
        assert_eq!(
            history.events.len(),
            3,
            "two entries and the rolling summary"
        );
        let mut at = 0;
        for event in &history.events {
            assert_eq!(event.start, at, "no gap and no overlap");
            assert!(event.len > 0 && event.len <= MAX_ENTRY_ROWS);
            at = event.end();
        }
        assert_eq!(at, history.entries.len(), "and nothing left over");

        // The stamp travels with the event, so a reader's divider can go on ticking.
        assert_eq!(history.events[0].created_at_ms, Some(40));
        assert_eq!(&*history.events[1].text, "Ship it?");
        assert_eq!(history.events[2].kind, "summary");
        assert_eq!(&*history.events[2].text, "Ran the tests, all green.");
    }

    /// Everything in the half is markdown by the time it gets here - a prompt is what the user
    /// typed, a result is a model's last message - so it is drawn as markdown rather than as
    /// the asterisks and hashes it is spelled with.
    #[test]
    fn a_run_s_history_is_rendered_as_markdown() {
        let mut run = a_pane_run();
        run.summary = Some(a_summary("Ran the **tests**, all green.", false));
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "Do it:\n\n- one\n- two".to_owned(),
            created_at_ms: None,
        }];

        let history = run_history(&run, 40, 100, 0);
        assert_eq!(
            texts(&history.entries),
            [
                "    prompt: Do it:",
                "    • one",
                "    • two",
                "    summary: Ran the tests, all green.",
            ],
            "the list is a list, the label rides on the prose above it, and the summary is the              newest row rather than a block above them - no asterisks in any of it"
        );

        // and the emphasis landed on the word rather than on the whole line
        let summary = history.entries.last().unwrap();
        let emphasised: String = summary
            .text
            .chars()
            .zip(&summary.styles)
            .filter(|(_, style)| style.bold)
            .map(|(ch, _)| ch)
            .collect();
        assert_eq!(
            emphasised, "summary: tests",
            "the model's emphasis, and the label that marks the row as a summary"
        );
        assert!(
            summary.styles[AGE_WIDTH..AGE_WIDTH + 8]
                .iter()
                .all(|style| !style.dim),
            "the summary's label is the one piece of furniture that is not dimmed"
        );
    }

    /// An entry that opens with a marker of its own gives the label a row, whichever kind of
    /// marker it is: `prompt: 1. one` is not a numbered list, and the numbering would take the
    /// continuation rows of the item with it.
    #[test]
    fn a_label_never_lands_on_a_list_marker() {
        let mut run = a_pane_run();
        run.summary = None;
        run.entries = vec![
            RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: "1. one\n2. two".to_owned(),
                created_at_ms: None,
            },
            RunEntry {
                tool: None,
                revision: 2,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: "- one\n- two".to_owned(),
                created_at_ms: None,
            },
        ];

        assert_eq!(
            texts(&run_history(&run, 40, 100, 0).entries),
            [
                "    prompt:",
                "    1. one", // the numbering kept, and its column with it
                "    2. two",
                "    prompt:",
                "    • one",
                "    • two",
            ],
        );
    }

    /// A summary opening with a marker gives the label a row of its own, the same as any other
    /// entry: `summary: - ran the tests` is not a bullet, and gluing the label onto the marker
    /// would take the column its continuation rows are indented to.
    #[test]
    fn a_summary_that_opens_with_a_marker_gives_the_label_a_row() {
        let mut run = a_pane_run();
        run.summary = Some(a_summary("- ran the tests", false));
        let history = run_history(&run, 40, 100, 0);
        assert_eq!(
            texts(&history.entries),
            ["    summary:", "    • ran the tests"]
        );

        run.summary = Some(a_summary("ran the tests", false));
        let history = run_history(&run, 40, 100, 0);
        assert_eq!(texts(&history.entries), ["    summary: ran the tests"]);
    }

    /// The rolling summary is the newest row of the timeline rather than a block above it, and
    /// it scrolls away with everything else - which is the whole point of unpinning it.
    #[test]
    fn the_rolling_summary_is_the_newest_row_of_the_timeline() {
        let mut run = a_pane_run();
        run.summary = Some(a_summary("Ran the tests.", false));
        run.entries = vec![
            RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: "run the tests".to_owned(),
                created_at_ms: None,
            },
            RunEntry {
                tool: None,
                revision: 2,
                event_id: None,
                kind: RunEntryKind::Question,
                text: "which ones?".to_owned(),
                created_at_ms: None,
            },
        ];
        assert_eq!(
            texts(&run_history(&run, 40, 100, 0).entries),
            [
                "    prompt: run the tests",
                "    question: which ones?",
                "    summary: Ran the tests.",
            ],
        );
    }

    /// A summary somebody asked for is both the newest entry and the rolling one, and saying it
    /// twice under itself reads as a bug rather than as a history.
    #[test]
    fn a_rolling_summary_that_repeats_the_newest_filed_one_is_drawn_once() {
        let mut run = a_pane_run();
        run.summary = Some(a_summary("  Ran the tests.  ", false));
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Summary,
            // What `push_summary_entry` files: the trimmed text of the same summary.
            text: "Ran the tests.".to_owned(),
            created_at_ms: None,
        }];
        assert_eq!(
            texts(&run_history(&run, 60, 100, 0).entries),
            ["    summary: Ran the tests."],
        );

        // A summary that has moved on since is a second row, where it was made.
        run.summary = Some(a_summary("Ran the tests and shipped it.", false));
        assert_eq!(
            texts(&run_history(&run, 60, 100, 0).entries),
            [
                "    summary: Ran the tests.",
                "    summary: Ran the tests and shipped it."
            ],
        );
    }

    /// The half has eight rows on a normal pane, so a paragraph break costs an eighth of what
    /// there is to read. The conversation, with the whole pane, keeps its air.
    #[test]
    fn the_half_draws_the_blocks_hard_against_each_other() {
        let mut run = a_pane_run();
        run.summary = None;
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "first\n\nsecond".to_owned(),
            created_at_ms: None,
        }];
        assert_eq!(
            texts(&run_history(&run, 40, 100, 0).entries),
            ["    prompt: first", "    second"]
        );
    }

    /// A result that opens with a bullet or a heading is still one: splicing `result: ` into
    /// the source would have made it neither, so the label takes a line of its own instead.
    #[test]
    fn a_label_never_swallows_the_block_it_names() {
        let mut run = a_pane_run();
        run.summary = None;
        run.outcome = RunOutcome::Completed;
        run.summarized_revision = 0;
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Result,
            text: "- one\n- two".to_owned(),
            created_at_ms: None,
        }];
        assert_eq!(
            texts(&run_history(&run, 40, 100, 0).entries),
            ["    result:", "    • one", "    • two"]
        );
    }

    /// The label shares the first row with the prose, and the host truncates a line rather
    /// than wrapping it - so wrapping to the whole half and *then* putting the label on would
    /// cut the tail off the first row of every entry in the pane.
    #[test]
    fn no_history_row_is_wider_than_the_half() {
        let mut run = a_pane_run();
        run.outcome = RunOutcome::Completed;
        run.summarized_revision = 0;
        run.summary = Some(a_summary(
            "Moved the renderer into agent-proto so both panes draw one dialect.",
            false,
        ));
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Result,
            text: "Every word of this sentence is short enough to fit but the whole of it is not"
                .to_owned(),
            created_at_ms: None,
        }];

        for width in [HISTORY_MIN_COLS + 2, 30, 46, 80] {
            let history = run_history(&run, width, 100, 0);
            let inner = width - 2;
            for line in &history.entries {
                assert!(line.text.width() <= inner, "{width}: {:?}", line.text);
            }
        }
    }

    /// One `AgentRun` to vary, the way the pane receives them.
    /// The age is the first thing on the row, in a column of its own, and an entry with no
    /// recorded time keeps that column clear rather than closing it up - so every label in
    /// the half starts at the same place and the times can be read straight down.
    #[test]
    fn an_entry_wears_its_age_in_the_left_hand_column() {
        let mut run = a_pane_run();
        run.summary = None;
        let minute = 60_000;
        run.entries = vec![
            RunEntry {
                tool: None,
                revision: 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: "run the tests".to_owned(),
                created_at_ms: Some(100 * minute),
            },
            RunEntry {
                tool: None,
                revision: 2,
                event_id: None,
                kind: RunEntryKind::Question,
                text: "which ones?".to_owned(),
                created_at_ms: Some(105 * minute),
            },
            RunEntry {
                tool: None,
                revision: 3,
                event_id: None,
                kind: RunEntryKind::Plan,
                text: "all of them".to_owned(),
                // Recorded before there was a time to record.
                created_at_ms: None,
            },
        ];

        let history = run_history(&run, 60, 106 * minute, 0);
        assert_eq!(
            texts(&history.entries),
            [
                " 6m prompt: run the tests",
                " 1m question: which ones?",
                "    plan: all of them",
            ],
        );
        assert!(
            history.entries.iter().all(|line| line
                .text
                .char_indices()
                .nth(AGE_WIDTH)
                .is_some_and(|(_, ch)| ch.is_alphabetic())),
            "every label starts in the same column, whether its entry has a time or not"
        );
    }

    /// The age is painted to be read as a column rather than as more label: undimmed and in
    /// the accent colour, against a dim `prompt: `.
    #[test]
    fn the_age_is_the_undimmed_part_of_the_furniture() {
        let mut run = a_pane_run();
        run.summary = None;
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "run the tests".to_owned(),
            created_at_ms: Some(0),
        }];

        let history = run_history(&run, 60, 5 * 60_000, 0);
        let row = &history.entries[0];
        assert!(
            row.styles[..AGE_WIDTH]
                .iter()
                .all(|style| !style.dim && style.color == Some(2)),
            "the age"
        );
        assert!(
            row.styles[AGE_WIDTH..AGE_WIDTH + "prompt:".len()]
                .iter()
                .all(|style| style.dim),
            "and the label it sits beside, still dim"
        );
    }

    /// A rolling summary is a row of the timeline like any other, so it says how old it is
    /// too - and says nothing where the daemon that made it predates the stamp.
    #[test]
    fn a_rolling_summary_wears_its_age_as_well() {
        let mut run = a_pane_run();
        run.summary = Some(Summary {
            created_at_ms: Some(0),
            ..a_summary("Ran the tests.", false)
        });
        assert_eq!(
            texts(&run_history(&run, 60, 3 * 3_600_000, 0).entries),
            [" 3h summary: Ran the tests."],
        );

        run.summary = Some(a_summary("Ran the tests.", false));
        assert_eq!(
            texts(&run_history(&run, 60, 3 * 3_600_000, 0).entries),
            ["    summary: Ran the tests."],
        );
    }

    /// The buckets, at the edges. Truncated rather than rounded: a row cares that an entry is
    /// roughly this old, not exactly.
    #[test]
    fn short_age_counts_whole_minutes_hours_and_days() {
        let cases = [
            (0, "now"),
            (59_000, "now"),
            (60_000, "1m"),
            (3_599_000, "59m"),
            (3_600_000, "1h"),
            (86_399_000, "23h"),
            (86_400_000, "1d"),
            (10 * 86_400_000, "10d"),
        ];
        for (elapsed, expected) in cases {
            assert_eq!(short_age(elapsed, 0), expected, "{elapsed}ms ago");
        }
        assert_eq!(
            short_age(0, 5_000),
            "now",
            "a clock that went backwards is not a negative age"
        );
    }

    /// The epoch, the two directions an offset can go, and the year boundaries the era shift
    /// exists to get right.
    #[test]
    fn wall_clock_reads_an_epoch_millisecond_as_a_date_and_a_time() {
        let cases = [
            // The epoch itself, and the offsets either side of it.
            (0_i64, 0_i32, "1970-01-01 00:00:00 +00:00"),
            (0, 3600, "1970-01-01 01:00:00 +01:00"),
            (0, -18_000, "1969-12-31 19:00:00 -05:00"),
            // An offset that carries the local time over a day boundary in each direction.
            (86_399_000, 3600, "1970-01-02 00:59:59 +01:00"),
            (3_600_000, -18_000, "1969-12-31 20:00:00 -05:00"),
            // An offset that is not a whole hour, which is what the minutes field is for.
            (0, 20_700, "1970-01-01 05:45:00 +05:45"),
            // A leap day, and the day after it.
            (1_709_208_000_000, 0, "2024-02-29 12:00:00 +00:00"),
            (1_709_294_400_000, 0, "2024-03-01 12:00:00 +00:00"),
            // A non-leap century, which the /100 and /400 terms disagree about.
            (4_107_542_400_000, 0, "2100-03-01 00:00:00 +00:00"),
            // Well before the epoch, where truncating division would floor the wrong way.
            (-2_208_988_800_000, 0, "1900-01-01 00:00:00 +00:00"),
        ];
        for (epoch_ms, offset, expected) in cases {
            assert_eq!(
                wall_clock(yinz_tile::prelude::SignedUnixMillis::new(epoch_ms), offset),
                expected,
                "{epoch_ms}ms at {offset}s"
            );
        }
    }

    /// The seconds of the day, at both ends of it: a rendering that read the hour off a
    /// truncating division would put midnight and noon in the same place on a pre-epoch date.
    #[test]
    fn wall_clock_keeps_the_time_of_day_inside_the_day() {
        for day in [-800_i64, -1, 0, 1, 20_000] {
            let midnight = day * 86_400_000;
            assert!(
                wall_clock(yinz_tile::prelude::SignedUnixMillis::new(midnight), 0)
                    .ends_with("00:00:00 +00:00"),
                "day {day} did not start at midnight: {}",
                wall_clock(yinz_tile::prelude::SignedUnixMillis::new(midnight), 0)
            );
            assert!(
                wall_clock(
                    yinz_tile::prelude::SignedUnixMillis::new(midnight + 86_399_000),
                    0
                )
                .ends_with("23:59:59 +00:00"),
                "day {day} did not end a second before midnight: {}",
                wall_clock(
                    yinz_tile::prelude::SignedUnixMillis::new(midnight + 86_399_000),
                    0
                )
            );
        }
    }

    fn a_pane_run() -> AgentRun {
        AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 3,
            yinz_session_name: "yinz".to_owned().into(),
            host_id: HostId::new(HOST),
            origin_run_id: None,
            previous_run_id: None,
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: Some(20),
            outcome: RunOutcome::Active,
            revision: 1,
            summarized_revision: 0,
            summary: None,
            entries: Vec::new(),
            omitted_entries: 0,
        }
    }

    /// The claim the strip's `!` makes, on the divider the user is already reading - and made
    /// from the row's flag, so the two cannot disagree.
    #[test]
    fn a_blocked_agent_is_marked_on_the_divider() {
        let waiting = summary_history(7, "claude", &a_summary("Shall I?", true), true, 30, 0);
        assert_eq!(waiting.label, "7 · claude !");
        // An agent that merely finished is not.
        let done = summary_history(7, "claude", &a_summary("Shall I?", false), false, 30, 0);
        assert!(!done.label.contains('!'), "{}", done.label);
        // Nor is one whose stored summary still says it was waiting, once the row stopped
        // saying so: the user answered, and the screen the summary was about is gone.
        let answered = summary_history(7, "claude", &a_summary("Shall I?", true), false, 30, 0);
        assert!(!answered.label.contains('!'), "{}", answered.label);
    }

    /// Better nothing than a column of single words - the half draws a placeholder instead.
    #[test]
    fn there_is_no_history_where_there_is_no_room_for_one() {
        let one = a_summary("Ran the tests.", false);
        assert!(summary_history(7, "claude", &one, false, HISTORY_MIN_COLS + 1, 0).is_empty());
        assert!(
            !summary_history(7, "claude", &one, false, HISTORY_MIN_COLS + 2, 0).is_empty(),
            "just enough"
        );
    }

    /// A row can say something about the run an ancestor id points at without holding that run:
    /// the archive is paged, so an origin is often not loaded, and the kind is the front of every
    /// id `run_id` mints.
    #[test]
    fn a_run_id_names_the_kind_that_minted_it() {
        assert_eq!(
            run_id_kind("claude:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
            Some("claude"),
        );
        // The fallback shape, machine-named or not: the kind is still what leads.
        assert_eq!(run_id_kind("codex:pane:7:1764618000000"), Some("codex"));
        assert_eq!(
            run_id_kind("codex:pane:7:1764618000000:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
            Some("codex"),
        );
        // Neither shape, and a leading empty kind: nothing to name.
        assert_eq!(run_id_kind("whatever"), None);
        assert_eq!(run_id_kind(":session-1"), None);
        assert_eq!(run_id_kind(""), None);
        // Whatever `run_id` builds, it round-trips.
        for (kind, session) in [("claude", "abc"), ("codex", "")] {
            let id = run_id(kind, 7, session, 1764618000000, &HostId::new("host"));
            assert_eq!(run_id_kind(&id), Some(kind), "{id}");
        }
    }

    /// The divider names *which* run it is drawing, and a real id does not fit one: a hook
    /// agent's is a uuid and an agent with no session id of its own is named by a raw epoch.
    /// Both shapes are cut back to the part a person reads, so the divider is not truncated
    /// into a meaningless prefix by `fit_label`.
    #[test]
    fn a_run_divider_names_a_run_a_person_can_read() {
        assert_eq!(
            short_run_id("claude:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
            "claude:9f3c1a7e",
        );
        assert_eq!(short_run_id("claude:pane:7:1764618000000"), "claude:pane:7");
        // The shape that names the machine too, which `splitn(4, ':')` reaches without being
        // taught there are two fallbacks: everything after the epoch lands in the part that is
        // thrown away.
        assert_eq!(
            short_run_id("claude:pane:7:1764618000000:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
            "claude:pane:7",
        );
        // Short enough to be read whole is left whole.
        assert_eq!(short_run_id("claude:session-1"), "claude:session-1");
        // Neither shape: not ours to shorten.
        assert_eq!(short_run_id("whatever"), "whatever");

        let mut run = a_pane_run();
        run.id = "claude:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa".to_owned();
        let label = run_history(&run, 40, 100, 0).label;
        assert_eq!(label, "claude:9f3c1a7e");
        assert_eq!(
            fit_label(label.clone(), 40),
            label,
            "and a divider the width of the pane keeps all of it"
        );
    }

    /// The `!` outranks the age on a divider too narrow for both: how old the answer is can
    /// wait, that the agent is blocked on you cannot.
    #[test]
    fn a_narrow_summary_divider_loses_the_age_before_the_marker() {
        let minute = 60_000;
        let mut summary = a_summary("Shall I?", true);
        summary.created_at_ms = Some(100 * minute);
        let label = summary_history(
            7,
            "claude",
            &summary,
            true,
            HISTORY_MIN_COLS + 2,
            105 * minute,
        )
        .label;
        assert_eq!(label, "7 · claude ! · 5m", "uncut where it is built");
        // A divider with less room than that - the scroll readout shares the row - loses the
        // age and keeps the marker, where before it kept the age and lost the marker.
        let cut = fit_label(label, 19);
        assert!(cut.contains('!'), "{cut}");
        assert!(!cut.contains("5m"), "{cut}");
    }

    /// [`age_gutter`] right-aligns into a fixed column and pads without truncating, so an
    /// age wider than the column would push that row's label one place further in than
    /// every other row's. `short_age` is what guarantees it cannot.
    #[test]
    fn an_age_never_outgrows_the_column_it_is_drawn_in() {
        let day = 86_400_000;
        for seconds in [0, 59, 60, 3_599, 3_600, 86_399, 86_400] {
            let age = short_age(seconds * 1000, 0);
            assert!(age.width() < AGE_WIDTH, "{age}");
        }
        for days in [1, 98, 99, 100, 365, 3_650] {
            let age = short_age(days * day, 0);
            assert!(age.width() < AGE_WIDTH, "{days} days: {age}");
        }
        // Past the ceiling the column stops counting rather than overflowing.
        assert_eq!(short_age(98 * day, 0), "98d");
        assert_eq!(short_age(99 * day, 0), "99d");
        assert_eq!(short_age(3_650 * day, 0), "99d");
    }

    /// A kind is up to 32 characters and this pane is half a panel wide, so the label can
    /// be asked for more room than the divider has. It is cut rather than allowed to push
    /// the rule off the row - but only by whoever draws the divider, which is the one place
    /// that knows what else is on that row.
    #[test]
    fn a_label_too_wide_for_its_divider_is_cut_rather_than_burst() {
        let kind = "a-very-long-agent-kind-indeed-xx";
        let label = summary_history(7, kind, &a_summary("Done.", true), true, 26, 0).label;
        assert_eq!(label, format!("7 · {kind} !"), "uncut where it is built");

        let cut = fit_label(label, 26);
        assert_eq!(cut.width(), 22, "{cut}");
        assert!(kind.starts_with(&cut["7 · ".len()..]));

        // And a divider with no room for a label at all keeps the rule.
        assert!(fit_label("claude · active".to_owned(), 3).is_empty());
    }

    /// A hint line loses whole keys from its end, never half of one, and says when it did.
    #[test]
    fn a_hint_line_drops_whole_segments_from_the_end() {
        let segments = ["Enter focus", "/ search", "w waiting (0)", "s summary"];
        let whole = segments.join(" · ");
        assert_eq!(
            fit_hints(&segments, whole.width()),
            whole,
            "no marker when all fit"
        );
        assert_eq!(fit_hints(&segments, 200), whole);
        // Room for two keys but not for the marker as well: the keys win.
        assert_eq!(fit_hints(&segments, 24), "Enter focus · / search");
        // Room for both: the marker says there is more.
        assert_eq!(fit_hints(&segments, 31), "Enter focus · / search · ? keys");
        // Only a first segment too wide for the row alone is ever cut.
        assert_eq!(fit_hints(&segments, 5), "Enter");
        assert_eq!(fit_hints(&segments, 0), "");
        assert_eq!(fit_hints(&[], 10), "");
        // An empty segment is a key a caller had nothing to say about, not a doubled rule.
        assert_eq!(fit_hints(&["a", "", "b"], 10), "a · b");

        for width in 0..=whole.width() + 1 {
            let hint = fit_hints(&segments, width);
            assert!(hint.width() <= width, "{width}: {hint:?}");
            if width < segments[0].width() {
                assert!(segments[0].starts_with(&hint), "{width}: {hint:?}");
                continue;
            }
            let (shown, marked) = match hint.strip_suffix(" · ? keys") {
                Some(shown) => (shown, true),
                None => (hint.as_str(), false),
            };
            let parts: Vec<&str> = shown.split(" · ").collect();
            assert_eq!(
                parts,
                segments[..parts.len()],
                "{width}: whole, from the front"
            );
            let dropped = parts.len() < segments.len();
            let room = shown.width() + " · ? keys".width() <= width;
            assert_eq!(marked, dropped && room, "{width}: {hint:?}");
        }
    }

    /// A line that asks for it gives up its trailing keys to say `? keys`, down to the ones it
    /// keeps - and a line that must not have the marker never gets it.
    #[test]
    fn a_marked_hint_line_gives_way_to_the_marker() {
        let segments = [
            "Enter focus",
            "s summary",
            "p previous",
            "w waiting (0)",
            "/ search",
            "Tab history",
        ];
        let whole = segments.join(" · ");
        // Room for two keys but not the marker after them: the second gives way to it, where
        // `fit_hints` would keep it.
        assert_eq!(fit_hints_marked(&segments, 24, 1), "Enter focus · ? keys");
        assert_eq!(fit_hints(&segments, 24), "Enter focus · s summary");
        // Room for the marker after both, which `fit_hints` already draws.
        assert_eq!(
            fit_hints_marked(&segments, 34, 1),
            "Enter focus · s summary · ? keys"
        );
        // Two kept: there is no giving way below them, so the line is what `fit_hints` draws.
        assert_eq!(
            fit_hints_marked(&segments, 24, 2),
            "Enter focus · s summary"
        );
        // No room for the marker after even the first key, and a first key cut to fit.
        assert_eq!(fit_hints_marked(&segments, 15, 1), "Enter focus");
        assert_eq!(fit_hints_marked(&segments, 5, 1), "Enter");
        // Everything fits: nothing to point at.
        assert_eq!(fit_hints_marked(&segments, whole.width(), 1), whole);
        assert_eq!(fit_hints_marked(&segments, 200, 3), whole);

        let join = |n: usize| segments[..n].join(" · ");
        for keep in 1..=3 {
            for width in 1..=whole.width() {
                let hint = fit_hints_marked(&segments, width, keep);
                assert!(hint.width() <= width, "{keep} {width}: {hint:?}");
                if width < segments[0].width() {
                    assert_eq!(hint, fit_hints(&segments, width), "{keep} {width}");
                    continue;
                }
                let (shown, marked) = match hint.strip_suffix(" · ? keys") {
                    Some(shown) => (shown, true),
                    None => (hint.as_str(), false),
                };
                let parts: Vec<&str> = shown.split(" · ").collect();
                assert_eq!(
                    parts,
                    segments[..parts.len()],
                    "{keep} {width}: whole, from the front"
                );
                // What `fit_segments` keeps, and so what `fit_hints` would draw.
                let kept = (1..=segments.len())
                    .rev()
                    .find(|&n| join(n).width() <= width)
                    .unwrap();
                let dropped = kept < segments.len();
                // Never below `keep` for the marker's sake - but where fewer than `keep` fit at
                // all, the line is `fit_hints`'s, marker and all.
                let floor = keep.min(kept);
                let room = join(floor).width() + " · ? keys".width() <= width;
                assert_eq!(marked, dropped && room, "{keep} {width}: {hint:?}");
                // And it gives up no more than the marker needs.
                let most = if marked {
                    (floor..=kept)
                        .rev()
                        .find(|&n| join(n).width() + " · ? keys".width() <= width)
                        .unwrap()
                } else {
                    kept
                };
                assert_eq!(parts.len(), most, "{keep} {width}: {hint:?}");
            }
        }

        // A status can hold the separator itself, and is still one segment: never split to
        // make room for the marker.
        let status = ["2 of 5 · waiting", "Enter", "Tab history"];
        assert_eq!(fit_hints_marked(&status, 24, 1), "2 of 5 · waiting · Enter");
        assert_eq!(
            fit_hints_marked(&status, 25, 1),
            "2 of 5 · waiting · ? keys"
        );

        // The unmarked line is the keys that fit and nothing else, however much was dropped.
        assert_eq!(fit_keys(&segments, 34), "Enter focus · s summary");
        assert_eq!(fit_keys(&segments, whole.width()), whole);
        for width in 0..=whole.width() + 1 {
            let keys = fit_keys(&segments, width);
            assert!(keys.width() <= width, "{width}: {keys:?}");
            assert!(!keys.ends_with("? keys"), "{width}: {keys:?}");
            assert!(whole.starts_with(&keys), "{width}: {keys:?}");
        }
    }
    // -- prompts that a harness wrote ----------------------------------------

    /// A notification of the shape Claude Code actually submits, with `result` as long as the
    /// caller asks for.
    fn task_notification(result_lines: usize) -> String {
        format!(
            "<task-notification>\n\
             <task-id>a10fe836c6a9fa4ca</task-id>\n\
             <output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>\n\
             <status>completed</status>\n\
             <summary>Agent \"/code-review high up to 3 subagents unstaged code\" finished</summary>\n\
             <note>A task-notification fires each time this agent stops.\n\
             The same task-id may notify more than once.</note>\n\
             <result>{}</result>\n\
             </task-notification>",
            "a line of the review\n".repeat(result_lines)
        )
    }

    fn run_of_prompts(prompts: &[&str]) -> AgentRun {
        let mut run = a_pane_run();
        run.entries = prompts
            .iter()
            .enumerate()
            .map(|(index, text)| RunEntry {
                tool: None,
                revision: index as u64 + 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: (*text).to_owned(),
                created_at_ms: Some(10),
            })
            .collect();
        run.revision = run.entries.len() as u64;
        run
    }

    /// The whole point, in one assertion: the rows say what happened rather than which tags it
    /// arrived in.
    #[test]
    fn a_task_notification_on_the_timeline_reads_as_what_it_is() {
        let history = run_history(&run_of_prompts(&[&task_notification(3)]), 52, 10, 0);
        let rows = texts(&history.entries).join("\n");

        assert!(rows.contains("prompt:"), "{rows}");
        assert!(rows.contains('✓'), "{rows}");
        assert!(rows.contains("a10fe836"), "{rows}");
        assert!(rows.contains("/code-review"), "{rows}");
        assert!(rows.contains("▸ result 3 lines"), "{rows}");
        // None of what made the row unreadable survives.
        assert!(!rows.contains('<'), "{rows}");
        assert!(!rows.contains("/private/tmp"), "{rows}");
        assert!(!rows.contains("notify more than once"), "{rows}");
    }

    /// The load-bearing one. `MAX_ENTRY_ROWS` is four, and the row that says what `Enter` would
    /// show is the one worth protecting: `DIGEST_SUMMARY_COLS` is sized so `cap_rows` never
    /// eats it at a width a half-pane actually has.
    #[test]
    fn the_digest_fits_the_rows_a_timeline_entry_gets() {
        for width in [HISTORY_MIN_COLS + 2, 30, 46, 52, 80] {
            let history = run_history(&run_of_prompts(&[&task_notification(400)]), width, 10, 0);
            assert_eq!(history.events.len(), 1, "at {width}");
            assert!(
                history.events[0].len <= MAX_ENTRY_ROWS,
                "{} rows at {width}",
                history.events[0].len
            );
            assert_eq!(history.entries.len(), history.events[0].len, "at {width}");
            if width >= 46 {
                let rows = texts(&history.entries).join("\n");
                assert!(
                    rows.contains('▸'),
                    "the affordances row was cut at {width}: {rows}"
                );
            }
        }
    }

    /// The same guarantee in a script that spends two columns to the character. Cut by
    /// character rather than by column, this summary is twice as wide as the budget says: it
    /// takes a third row, and `cap_rows` eats the affordances row to fit.
    #[test]
    fn a_digest_of_wide_characters_fits_the_same_rows() {
        let wide = format!(
            "<task-notification>\n\
             <task-id>a10fe836c6a9fa4ca</task-id>\n\
             <status>completed</status>\n\
             <summary>{}</summary>\n\
             <result>{}</result>\n\
             </task-notification>",
            "代理审查了这个补丁 ".repeat(6),
            "a line of the review\n".repeat(400),
        );
        for width in [HISTORY_MIN_COLS + 2, 30, 46, 52, 80] {
            let history = run_history(&run_of_prompts(&[&wide]), width, 10, 0);
            assert_eq!(history.events.len(), 1, "at {width}");
            assert!(
                history.events[0].len <= MAX_ENTRY_ROWS,
                "{} rows at {width}",
                history.events[0].len
            );
            if width >= 46 {
                let rows = texts(&history.entries).join("\n");
                assert!(
                    rows.contains('▸'),
                    "the affordances row was cut at {width}: {rows}"
                );
            }
        }
    }

    /// The invariant this design is most likely to break by accident, and the one that keeps it
    /// a rendering change rather than a record change: the digest is drawn, the prompt is kept.
    #[test]
    fn the_history_event_behind_a_digest_still_carries_the_raw_prompt() {
        let raw = task_notification(3);
        let history = run_history(&run_of_prompts(&[&raw]), 52, 10, 0);
        assert_eq!(&*history.events[0].text, raw);
        assert_eq!(history.events[0].kind, "prompt");
    }

    /// Every row belongs to one event and no row to two — still true once an event's rows come
    /// from somewhere other than its own text.
    #[test]
    fn an_events_spans_still_tile_the_entries() {
        let run = run_of_prompts(&[&task_notification(3), "plain prose", &task_notification(1)]);
        let history = run_history(&run, 52, 10, 0);
        let mut at = 0;
        for event in &history.events {
            assert_eq!(event.start, at, "{:?}", history.events);
            at = event.end();
        }
        assert_eq!(at, history.entries.len());
    }

    /// The regression guard that matters most: nothing a person typed goes anywhere near the
    /// scanner's output.
    #[test]
    fn a_plain_prompt_is_rendered_exactly_as_before() {
        let prose = "fix the flaky reconnect test\n\n- check the timeout\n- check the retry";
        let run = run_of_prompts(&[prose]);
        let history = run_history(&run, 52, 10, 0);
        assert_eq!(&*history.events[0].text, prose);

        // Byte for byte what an entry of any other kind renders to.
        let mut same = a_pane_run();
        same.entries = vec![RunEntry {
            tool: None,
            kind: RunEntryKind::Question,
            ..run.entries[0].clone()
        }];
        let expected = run_history(&same, 52, 10, 0);
        assert_eq!(
            texts(&history.entries)
                .iter()
                .map(|row| row.replacen("prompt", "question", 1))
                .collect::<Vec<_>>(),
            texts(&expected.entries)
        );
    }

    /// Before this the tab bar spent a whole woken turn reading
    /// `<task-notification> <task-id>a10fe836…`.
    #[test]
    fn a_pane_woken_by_a_task_is_named_for_the_task() {
        let title = prompt_title(&task_notification(3)).expect("a title");
        assert!(title.starts_with("task · Agent \"/code-review"), "{title}");
        assert!(!title.contains('<'), "{title}");

        // And a person's prompt is named exactly as it was.
        assert_eq!(
            prompt_title("fix the flaky test").as_deref(),
            Some("fix the flaky test")
        );
    }

    /// An archive row is a sentence now rather than the opening tag the first line of such a
    /// prompt literally is.
    #[test]
    fn a_run_woken_by_a_task_is_titled_by_the_task_and_not_by_its_tags() {
        let title = run_title(&run_of_prompts(&[&task_notification(3)]));
        assert!(title.starts_with("task · Agent"), "{title}");
        assert!(!title.contains('<'), "{title}");
    }

    /// A run woken by a background task and later typed into by a person is about what the
    /// person typed, wherever in the run they typed it.
    #[test]
    fn a_run_that_a_person_later_typed_into_is_titled_by_what_they_typed() {
        let run = run_of_prompts(&[
            &task_notification(3),
            "rebase onto main and rerun the suite",
        ]);
        assert_eq!(run_title(&run), "rebase onto main and rerun the suite");
    }

    fn prompt_event(prompt: &str) -> yinz_tile::agent::HookEvent {
        yinz_tile::agent::HookEvent {
            hook_event_name: "UserPromptSubmit".to_owned(),
            prompt: Some(prompt.to_owned()),
            ..Default::default()
        }
    }

    /// The id of a prompt, composed exactly as the ingest path composes it: the text the entry
    /// will keep is what the id is taken from.
    fn id_of(prompt: &str) -> Option<String> {
        let event = prompt_event(prompt);
        let (_, text) = run_entry(&event)?;
        run_event_id(&event, &text)
    }

    /// A prompt has no `tool_use_id`, so before this the ingest path could only compare one
    /// against the entry immediately before it and a redelivery with anything in between landed
    /// twice. A second notification for the same task still does, because it is a second thing
    /// happening — see `envelope::task_id`.
    #[test]
    fn a_redelivered_notification_does_not_land_twice() {
        let first = id_of(&task_notification(3)).expect("an id");
        assert_eq!(id_of(&task_notification(3)), Some(first.clone()));
        assert!(
            first.starts_with("UserPromptSubmit:task-notification:"),
            "{first}"
        );
        assert_ne!(id_of(&task_notification(4)), Some(first));

        // A person's prompt keeps today's adjacent-only rule, which is the right one for it.
        assert_eq!(id_of("fix the flaky test"), None);
    }

    /// Reports that differ only beyond the retained prefix must not share a history id.
    #[test]
    fn a_notification_cut_by_the_broadcast_cap_gets_no_id() {
        let lines = yinz_tile::agent::HOOK_BROADCAST_MAX_BYTES / 10;
        let prune = |text: &str| {
            let event = prompt_event(text);
            event
                .broadcast(serde_json::to_string(&event).unwrap())
                .0
                .into_owned()
        };
        let (first, second) = (
            prune(&task_notification(lines)),
            prune(&task_notification(lines + 1)),
        );
        assert_ne!(
            id_of(&task_notification(lines)),
            id_of(&task_notification(lines + 1))
        );
        assert_eq!(
            first.prompt, second.prompt,
            "the cap took the difference between them"
        );
        assert_ne!(first.prompt_sha256, second.prompt_sha256);
        for event in [&first, &second] {
            let (_, text) = run_entry(event).expect("an entry");
            assert_eq!(run_event_id(event, &text), None);
        }
    }

    /// A collapsed paste over the broadcast cap reaches the history as what was pasted, not as
    /// an opener whose closer the cap cut off.
    #[test]
    fn an_oversize_collapsed_paste_is_recorded_without_its_wrapper() {
        let body = "build\n".repeat(yinz_tile::agent::HOOK_BROADCAST_MAX_BYTES / 4);
        let event = prompt_event(&format!(
            "\n\n<pasted_content id=\"8c9e\">\n{body}\n</pasted_content id=\"8c9e\">\n"
        ));
        let (pruned, _) = event.broadcast(serde_json::to_string(&event).unwrap());
        let (kind, text) = run_entry(&pruned).expect("an entry");
        assert_eq!(kind, RunEntryKind::Prompt);
        assert!(text.starts_with("build\nbuild"), "{}", &text[..40]);
        assert!(!text.contains("pasted_content"));
    }

    /// Finding 2, in the form that bites: the id has to be taken from the text the entry keeps,
    /// or it is a hash of a string nothing has.
    #[test]
    fn an_entrys_id_is_a_function_of_the_text_it_stores() {
        let typed = task_notification(3);
        let expected = id_of(&typed).expect("an id");

        // `sanitize` drops a bare control character, so both deliveries record the same text —
        // and an id taken from the raw prompt would have called them two different events.
        let belled = typed.replace("a line", "a\u{7} line");
        assert_ne!(belled, typed);
        assert_eq!(id_of(&belled).as_deref(), Some(expected.as_str()));

        // And the whole point of taking it from there: it can be computed again from the entry.
        let (_, text) = run_entry(&prompt_event(&typed)).expect("an entry");
        let stored = RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text,
            created_at_ms: None,
        };
        assert_eq!(entry_event_id(&stored).as_deref(), Some(expected.as_str()));
    }

    /// The reading the raw prompt gets wrong. `next_closing` wants the closing tag to end a
    /// line, and under CRLF the byte after it is `\r`, so the element reads as cut and loses its
    /// identity — while the text actually recorded, with the carriage returns dropped, is whole.
    #[test]
    fn a_notification_with_windows_line_endings_is_still_one_notification() {
        let crlf = task_notification(3).replace('\n', "\r\n");
        let expected = id_of(&task_notification(3)).expect("an id");
        assert_eq!(
            id_of(&crlf).as_deref(),
            Some(expected.as_str()),
            "the same notification, delivered by a harness that ends its lines differently"
        );
    }

    /// A run older than the entry-row split comes back out of its JSON `payload` blob, where
    /// there were no ids to come back with. Compared as stored it matches nothing ever, so the
    /// oldest runs — the ones with the most history — were the ones a redelivery landed twice
    /// against.
    #[test]
    fn a_run_recorded_before_ids_existed_is_still_compared_by_one() {
        let (_, text) = run_entry(&prompt_event(&task_notification(3))).expect("an entry");
        let legacy = |kind, text: &str| RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind,
            text: text.to_owned(),
            created_at_ms: None,
        };
        assert_eq!(
            entry_event_id(&legacy(RunEntryKind::Prompt, &text)),
            id_of(&task_notification(3)).map(Cow::Owned),
        );

        // Only a prompt. A tool call's identity was never in its text, and a summary's `None`
        // is a statement rather than a gap — see `push_summary_entry`.
        assert_eq!(entry_event_id(&legacy(RunEntryKind::Result, &text)), None);
        assert_eq!(entry_event_id(&legacy(RunEntryKind::Summary, &text)), None);
        assert_eq!(
            entry_event_id(&legacy(RunEntryKind::Prompt, "fix the flaky test")),
            None,
            "and a prompt with no envelope in it still has no identity"
        );

        // An id it was given is the one it keeps, whatever its text now says.
        let stored = RunEntry {
            tool: None,
            event_id: Some("UserPromptSubmit:given".to_owned()),
            ..legacy(RunEntryKind::Prompt, &text)
        };
        assert_eq!(
            entry_event_id(&stored).as_deref(),
            Some("UserPromptSubmit:given")
        );
    }

    #[test]
    fn the_worktree_cell_names_the_checkout_a_run_worked_in() {
        let run = |cwd: Option<&str>, host: Option<&str>| AgentRunHeader {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            yinz_session_name: "main".into(),
            host_id: HostId::new(HOST),
            session_id: "session-1".to_owned(),
            origin_run_id: None,
            previous_run_id: None,
            kind: "claude".to_owned(),
            cwd: cwd.map(str::to_owned),
            host: host.map(str::to_owned),
            bubblewrap: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: 1,
            summary: None,
            title: "hook history".to_owned(),
        };
        assert_eq!(worktree_cell(&run(Some("/work/agent"), None,)), "agent");
        assert_eq!(
            worktree_cell(&run(Some("/work/agent/.claude/worktrees/topic"), None,)),
            "topic",
            "two worktrees of one project are told apart by the leaf, which is why it is the leaf",
        );
        assert_eq!(
            worktree_cell(&run(Some("/work/agent"), Some("build-box"),)),
            "build-box:agent",
            "another machine's agent is not this one",
        );
        assert_eq!(worktree_cell(&run(None, None,)), "");
        assert_eq!(
            worktree_cell(&run(Some(""), Some("build-box"),)),
            "",
            "a run that reported no folder names none, rather than a bare `build-box:`",
        );
    }

    #[test]
    fn panes_keep_their_checkout_location() {
        let ssh = AgentPane {
            host: Some("build-box".to_owned()),
            cwd: Some("/work/agent".to_owned()),
            ..pane(8, "claude", Some("claude"))
        };
        assert_eq!(
            ssh.checkout_location(),
            Some(Location::new(Some("build-box"), "/work/agent"))
        );

        let local = AgentPane {
            cwd: Some("/work/agent".to_owned()),
            ..pane(9, "claude", Some("claude"))
        };
        assert_eq!(
            local.checkout_location(),
            Some(Location::local("/work/agent"))
        );

        // Nothing said about either half is no location at all, which is what keeps a
        // pane from being probed under a half-known one.
        assert_eq!(pane(10, "zsh", None).checkout_location(), None);
    }
}
