//! Which panes hold coding agents, and what they are showing.
//!
//! Nothing here calls the Zellij host, so all of it runs under `make test`.
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
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

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

/// What a pane is doing, as far as Zellij will say. `Held` is a command pane that
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
    /// The command with its arguments, as Zellij stringified it. `None` for a shell.
    pub command: Option<String>,
    /// The current folder reported by Zellij.
    pub cwd: Option<String>,
    /// The machine the pane runs on, when it is not this one.
    ///
    /// Part of the pane's identity rather than decoration: `/home/me/work` on
    /// two machines are two different folders, and an agent you are about to
    /// send input to is on one of them specifically.
    #[serde(default)]
    pub host: Option<String>,
    pub status: AgentStatus,
    /// The stable tab id and tab name this pane sits in. `None` for a session-global
    /// pane, which is in no tab.
    pub tab: Option<(usize, String)>,
    /// The kind this pane was last seen *running*, kept while the pane lives.
    ///
    /// `command` is the foreground command, so it stops naming the agent the moment
    /// the agent shells out to `git` or runs a tool — and the pane would drop out of
    /// every list until it came back. `hocus-agentd` is the only writer; it latches
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

/// Every agent we know.
///
/// Canonical names only. The `-cli`, `-code` and `-agent` aliases this used to spell
/// out are what `kind_of` gets from the leading dash-segment, along with the ones no
/// table could have listed: an installer that drops `muse-bin-0.1.0-R708.1` on the
/// path names the same agent as one that drops `muse`.
const KINDS: &[&str] = &[
    "muse", "codex", "claude", "cursor", "qwen", "gemini", "aider", "cline", "opencode", "pi",
];

/// Programs that run another program named in their own arguments. Half the table
/// above ships as a `#!/usr/bin/env node` shim, so the real argv is
/// `["node", "/opt/bin/gemini"]` and the first word names no agent at all.
const INTERPRETERS: &[&str] = &[
    "node", "bun", "deno", "npx", "python", "python3", "ruby", "perl", "uv", "uvx",
];

/// The program a command-line word names: the file name of its path.
fn program(word: &str) -> &str {
    Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(word)
}

/// The kind a program name is: its file name, then that without an extension, then
/// its leading dash-segment.
///
/// Each step strips decoration a packager added and the user never typed — `claude.js`
/// from a shim, `-0.1.0-R708.1` from a versioned binary, `-cli` from a published name.
/// The segment is still a whole one, so `pipenv` is not `pi`.
fn kind_of(word: &str) -> Option<&'static str> {
    let name = program(word).to_lowercase();
    let stem = name.split('.').next().unwrap_or_default();
    [
        name.as_str(),
        stem,
        stem.split('-').next().unwrap_or_default(),
    ]
    .iter()
    .find_map(|candidate| KINDS.iter().find(|kind| *kind == candidate).copied())
}

/// The agent kind a command line names, or `None`.
///
/// Whole tokens, never a substring: `pi` against a substring would make every `pip
/// install` shell an agent, and an agent is something the model may send input to.
///
/// This is the half of detection that can be *trusted over time*: a pane that is
/// running an agent right now is one, whatever it is called. `hocus-agentd` latches
/// on this and nothing else.
pub fn kind_from_command(command: &str) -> Option<&'static str> {
    let mut words = command.split_whitespace();
    let first = words.next().unwrap_or_default();
    if let Some(kind) = kind_of(first) {
        return Some(kind);
    }
    // Later words count only behind an interpreter. Otherwise `git commit -m
    // "ask claude"` would be an agent, which is the same reason detection is
    // token-based rather than substring-based.
    if INTERPRETERS.contains(&program(first).to_lowercase().as_str()) {
        if let Some(kind) = words.find_map(kind_of) {
            return Some(kind);
        }
    }
    None
}

/// The agent kind a pane title names, or `None`.
///
/// A title is free text a program can set to anything, so only the canonical kind
/// counts here, and only as a complete word. Nothing resets a title when the program
/// that set it exits, which is why this is a guess and never a latch.
pub fn kind_from_title(title: &str) -> Option<&'static str> {
    let title = title.to_lowercase();
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|word| KINDS.iter().find(|kind| **kind == word).copied())
}

/// The agent kind a pane runs, from its command first and its title second.
pub fn detect_kind(title: &str, command: Option<&str>) -> Option<&'static str> {
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

/// Programs that are a prompt rather than a program at one.
///
/// Only used to decide that a title has been outlived — see `detect_kind`. Kept
/// deliberately short: a name that is not here merely falls back to today's
/// behaviour, while a wrong name here would hide a real agent.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "nu", "xonsh", "elvish", "pwsh",
    "ash", "busybox",
];

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
    SHELLS.contains(&stem)
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
pub fn classify<'a>(
    pane: &'a AgentPane,
    manual: &'a BTreeMap<u32, String>,
) -> Option<(&'a str, &'static str)> {
    if let Some(kind) = manual.get(&pane.pane_id) {
        return Some((kind.as_str(), "manual"));
    }
    // The session's record first: a pane spawned as an agent, or one whose agent has reported,
    // is an agent whatever its foreground command says right now.
    if let Some(agent) = pane.agent.as_ref() {
        return Some((agent.kind.as_str(), "auto"));
    }
    detect_kind(&pane.title, pane.command.as_deref())
        .map(|kind| (kind, "auto"))
        .or_else(|| pane.detected.as_deref().map(|kind| (kind, "auto")))
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
/// cannot tell "working" from "sitting still", and claiming an edge here would summarise
/// every agent the instant the daemon noticed it.
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

/// Whether a pane has turned over enough of its words since `previous` to be worth asking
/// about again — at least one word in `factor`.
///
/// Words rather than lines: the same output rewrapped at a new width is not new work, and a
/// line-wise comparison would call every resize a fresh screen.
pub fn changed_materially(previous: &str, current: &str, factor: usize) -> bool {
    let mut before: BTreeMap<&str, usize> = BTreeMap::new();
    for token in tokens(previous) {
        *before.entry(token).or_default() += 1;
    }
    let mut total = 0usize;
    let mut shared = 0usize;
    for token in tokens(current) {
        total += 1;
        if let Some(count) = before.get_mut(token).filter(|count| **count > 0) {
            *count -= 1;
            shared += 1;
        }
    }
    if total == 0 {
        return false;
    }
    (total - shared) * factor >= total
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

pub use zellij_tile::agent::{
    clean_hint, hook_detail, waiting_detail, waiting_needs_a_person, HOOK_DETAIL_CHARS,
    HOOK_SESSION_END, WAITING_DETAIL_CHARS,
};
use zellij_tile::prelude::{AgentInfo, AgentSource, AgentState};

/// What a Claude Code pane says it is doing, in the three states a row can draw.
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
/// — [`observe`], [`Activity`] and the idle summary the daemon buys on the busy→idle
/// edge — is inference from a picture; this is the agent saying so. Where the two
/// disagree, this wins.
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
    /// Claude Code's own session id, so a session restarted in the same pane can be
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
pub fn hook_phase(event: &crate::wire::HookEvent) -> Option<HookPhase> {
    zellij_tile::agent::hook_state(event).and_then(HookPhase::from_state)
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

/// The prompt for a turn the agent described itself.
///
/// Two differences from [`SUMMARY_SYSTEM_PROMPT`], and both are the point of the hook
/// feed. The input is the agent's *own last message* rather than a photograph of a
/// terminal, so the model is reading prose written for a person rather than inferring
/// intent from a redraw. And `awaiting` is not asked for at all: the feed already knows
/// whether the agent is blocked on the user, and a second opinion would only be a
/// disagreement to resolve — see `finish_summary_with`, which overwrites it either way.
const TURN_SUMMARY_SYSTEM_PROMPT: &str = "A coding agent has just finished a turn. You \
will be shown what the user asked for, if anything, and the agent's own final message. \
Reply with a single JSON object and nothing else - no prose, no markdown, no code \
fences. The object has exactly two keys: \"summary\", one or two plain sentences \
describing what the agent did and where the session now stands; and \"title\", at most \
four words naming the task, with no trailing punctuation, to be used as the name of the \
pane. Write about the work, not about the message: do not begin with \"the agent\" or \
quote the message back.";

/// The body of one completion asking what a turn that just ended was.
///
/// The counterpart of [`summary_request_body`] for a pane that reports for itself.
/// Carries the same header — kind, folder, branch — so the two paths give the model the
/// same context, and adds the user's request where one was seen: a description of an
/// answer written without sight of the question is half a description.
pub fn turn_summary_request_body(
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
    user.push_str(&format!("\n\n--- the agent replied ---\n{message}"));
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            { "role": "system", "content": TURN_SUMMARY_SYSTEM_PROMPT },
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
/// Held in memory by `hocus-agentd` and shipped in its snapshot. `awaiting` is the strip's
/// `!` marker, `text` is the inset hocus draws over its transcript, and `title` is what the
/// daemon renames the pane to.
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
        cwd,
        session_id: None,
    }
}

/// Pane id → what the model last said about it.
pub type Summaries = BTreeMap<u32, Summary>;

/// The `agent_latch` and `agent_summary` rows in `shared:hocus`: what `agent_register` set by
/// hand, and what the daemon last said about each idle pane, so a hocus reload keeps both.
/// hocus is the only writer; see `store::agent_latch_def` and `store::agent_summary_def`.
pub mod rows {
    use super::{Summaries, Summary};
    use std::collections::BTreeMap;
    use zellij_tile::prelude::{DataRows, DataValue, Mutation, Pred};

    pub const LATCH_ENTITY: &str = "agent_latch";
    pub const SUMMARY_ENTITY: &str = "agent_summary";

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

    pub fn latch_mutation(pane_id: u32, kind: &str) -> Mutation {
        Mutation::put(LATCH_ENTITY)
            .uid(pane_id.to_string())
            .set("pane_id", pane_id)
            .set("kind", kind)
            .build()
    }

    pub fn latch_delete_mutation(pane_ids: impl IntoIterator<Item = u32>) -> Mutation {
        Mutation::delete(LATCH_ENTITY, Pred::is_in("pane_id", pane_ids))
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

    pub fn summary_mutation(pane_id: u32, summary: &Summary) -> Mutation {
        Mutation::put(SUMMARY_ENTITY)
            .uid(pane_id.to_string())
            .set("pane_id", pane_id)
            .set("text", summary.text.clone())
            .set("title", summary.title.clone())
            .set("awaiting", summary.awaiting)
            .build()
    }

    pub fn summary_delete_mutation(pane_ids: impl IntoIterator<Item = u32>) -> Mutation {
        Mutation::delete(SUMMARY_ENTITY, Pred::is_in("pane_id", pane_ids))
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
                    },
                ))
            })
            .collect()
    }

    /// The writes that turn `before` into `after`, ignoring the hash: an unchanged summary
    /// produces no write, so a poll that changes nothing is silent.
    pub fn summary_diff_mutations(before: &Summaries, after: &Summaries) -> Vec<Mutation> {
        let same = |a: &Summary, b: &Summary| {
            a.text == b.text && a.title == b.title && a.awaiting == b.awaiting
        };
        let mut mutations: Vec<Mutation> = after
            .iter()
            .filter(|(pane_id, summary)| !before.get(pane_id).is_some_and(|b| same(b, summary)))
            .map(|(pane_id, summary)| summary_mutation(*pane_id, summary))
            .collect();
        let gone: Vec<u32> = before
            .keys()
            .filter(|pane_id| !after.contains_key(pane_id))
            .copied()
            .collect();
        if !gone.is_empty() {
            mutations.push(summary_delete_mutation(gone));
        }
        mutations
    }
}

/// The name to give the pane this summary describes, or `None` when there is none worth
/// giving. One rule, so the daemon and its tests cannot disagree about what an empty title
/// means.
pub fn rename_title(summary: &Summary) -> Option<&str> {
    let title = summary.title.trim();
    (!title.is_empty()).then_some(title)
}

/// One summary as the toast the daemon shows the moment it is made.
///
/// The inset over hocus's transcript only draws while the user is standing in the agents
/// pane with that row selected, which is precisely when they already know. This is the other
/// channel: a box in the corner, over whatever pane they are actually in, saying which pane
/// went idle and what it said. Same reasoning as hocus's confirmation toast — a thing worth
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
/// stale. A `PermissionRequest` arrives in the milliseconds the pipe took.
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

/// How wide the summary box is allowed to get. A box the full width of a wide pane reads as a
/// second list rather than a note about one row of the one above it.
pub const SUMMARY_BOX_MAX_COLS: usize = 64;
/// Below this the text inside the box wraps into a column of single words.
pub const SUMMARY_BOX_MIN_COLS: usize = 20;
/// Rows of summary the box draws before it cuts the rest off. Two sentences is what the model
/// is asked for; this is the ceiling for the ones that ignore it.
pub const SUMMARY_BOX_MAX_ROWS: usize = 6;

/// The idle summary as a framed box: a header naming the pane, the model's words wrapped
/// inside it, and a footer. Every row is exactly `width` columns.
///
/// Drawn at the foot of the agents pane, under the list, for the agent the cursor is on. It
/// used to be drawn over the head of hocus's transcript, in the pane *beside* the list; it is
/// here now because the two panes are separate surfaces and a note about the row under your
/// cursor belongs in the pane that cursor is in.
///
/// `None` when there is nowhere to put it - a box narrower than [`SUMMARY_BOX_MIN_COLS`] wraps
/// into a column of single words, and one shorter than three rows has no room for a line of
/// text at all. Better nothing than either.
pub fn summary_box(
    pane_id: u32,
    kind: &str,
    summary: &Summary,
    width: usize,
    height: usize,
) -> Option<Vec<String>> {
    let inner = width.saturating_sub(4);
    if inner < SUMMARY_BOX_MIN_COLS || height < 3 {
        return None;
    }

    let room = (height - 2).min(SUMMARY_BOX_MAX_ROWS);
    let mut lines = crate::text::wrap(&summary.text, inner);
    if lines.len() > room {
        lines.truncate(room);
        // The cut is marked rather than silent: a summary that stops mid-sentence
        // reads as a summary that had nothing more to say.
        let last = lines.last_mut().expect("room is at least one");
        while last.width() + 1 > inner {
            last.pop();
        }
        last.push('…');
    }

    let mut label = format!(" {pane_id} · {kind} ");
    if summary.awaiting {
        // The same claim the strip's `!` marker makes, in the one place the user
        // is already looking.
        label.push_str("! ");
    }
    // A label wider than the box it heads is cut rather than allowed to push the
    // frame's corner off the row.
    while label.width() > width - 3 {
        label.pop();
    }

    Some(
        [format!(
            "┌─{label}{}┐",
            "─".repeat(width - 3 - label.width())
        )]
        .into_iter()
        .chain(
            lines
                .iter()
                .map(|line| format!("│ {line}{} │", " ".repeat(inner - line.width()))),
        )
        .chain([format!("└{}┘", "─".repeat(width - 2))])
        .collect(),
    )
}

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
    })
}

/// The marker column at the head of an agent row.
///
/// Two characters whether or not anything is waiting, so the list does not shift sideways
/// as agents start and stop asking. `!` is the only thing drawn today; the summary text
/// behind it is carried but not yet shown.
///
/// Gated on the pane being idle as well as on the answer, because a summary outlives the
/// moment it describes: it is deliberately kept while the agent works again — that is what
/// lets an agent returning to the same screen be free rather than another request — and an
/// agent that is working is not waiting on anybody, whatever it was waiting on before.
fn waiting_marker(
    summaries: &Summaries,
    activity: &Activity,
    hooks: &HookFeeds,
    status: AgentStatus,
    pane_id: u32,
) -> &'static str {
    // An agent that says it is blocked on the user is blocked on the user. No
    // freshness check against `activity` here, unlike the summary path below: that
    // check exists because a summary describes one screen and outlives it, while a
    // feed has no screen behind it at all - it is the current state or it is nothing.
    if let Some(feed) = hooks.get(&pane_id) {
        return match (status, feed.phase) {
            (AgentStatus::Running, HookPhase::Waiting) => "! ",
            _ => "  ",
        };
    }
    let idle = activity.get(&pane_id).is_some_and(|(_, polls)| *polls > 0);
    let current = activity_hash(activity, pane_id);
    let fresh = summaries
        .get(&pane_id)
        .is_some_and(|s| s.awaiting && Some(s.hash) == current);
    match idle && fresh {
        true => "! ",
        false => "  ",
    }
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
/// Serialized: hocus computes these rows (it holds the snapshot, the project
/// registry and the branch probes) and pipes them to the hocus-agents pane,
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
    /// summary box's header wants it on its own, and classification is hocus's job. Carrying it
    /// is cheaper than a second classifier in the pane that draws the box.
    Agent {
        pane_id: u32,
        text: String,
        #[serde(default)]
        kind: String,
    },
}

/// The agent strip as the hocus-agents pane should draw it: the rows, and the
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
/// One agent, sorted before it becomes a row: its project, its branch, its pane, the line as
/// drawn, and the kind that line names. Named because clippy counts the tuple's arms, and
/// because the sort below reads better against a name than against five anonymous slots.
/// The kind is owned rather than borrowed because it carries the hook marker, which
/// is a fact about the pane rather than about the kind's name.
type SortableAgent<'a> = (Option<&'a str>, Option<&'a str>, u32, String, String);

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
            let cell = kind_cell(kind, pane.host.as_deref(), hooked);
            let mut row = format!(
                "{}{:>3}  {cell}  {}",
                waiting_marker(summaries, activity, hooks, pane.status, pane.pane_id),
                pane.pane_id,
                status_cell(
                    pane.status,
                    activity.get(&pane.pane_id),
                    hooks.get(&pane.pane_id)
                )
            );
            if let Some((_, name)) = &pane.tab {
                row.push_str("  ");
                row.push_str(name);
            }
            Some((
                owners.get(&pane.pane_id).map(String::as_str),
                branches.get(&pane.pane_id).map(String::as_str),
                pane.pane_id,
                row,
                // Marked, but never hosted: the box this heads is drawn beside the row
                // that already names the machine, and a header is one line wide.
                kind_cell(kind, None, hooked),
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
    for (owner, checkout, pane_id, text, kind) in agents {
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
        rows.push(StripRow::Agent {
            pane_id,
            text,
            kind,
        });
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
) -> BTreeMap<usize, String> {
    // `None` is a tab that two projects disagree over. It stays in the map, so a
    // later agent of one project cannot make the tab agree again.
    let mut found: BTreeMap<usize, Option<String>> = BTreeMap::new();
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

/// Every project a tab's agents are working on, sorted and without repeats.
///
/// The other half of [`tab_projects`]: that one asks which project *owns* a tab
/// and refuses to answer for a tab two projects share, because an assignment has
/// to name one project. Naming the tab does not — a tab with an agent in each of
/// two checkouts is honestly described by both, and saying so is more use than
/// falling back to "Tab #4".
///
/// Sorted so the label does not shuffle when the panes come back in another
/// order. Agents only, and a tab whose agents have no known project gets no
/// entry, exactly as in `tab_projects`.
pub fn tab_project_labels(
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    owners: &BTreeMap<u32, String>,
) -> BTreeMap<usize, Vec<String>> {
    let mut found: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for pane in panes {
        if classify(pane, manual).is_none() {
            continue;
        }
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
            if !candidates.iter().any(|candidate| candidate == kind) {
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
            // cannot disagree about who is blocked. See `waiting_marker`: an agent
            // reporting for itself is believed outright, and only a pane without a
            // feed is read off a summary — where the answer is gated on that summary
            // still describing the screen the pane is showing.
            //
            // The feed answers whether or not a summary exists, which the summary path
            // cannot: a pane can be blocked on the user before anything has been
            // written about it, and the model asking for the list needs to know that.
            if let Some(feed) = hooks.get(&pane.pane_id) {
                item["awaiting_confirmation"] =
                    json!(pane.status == AgentStatus::Running && feed.phase == HookPhase::Waiting);
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
            } else if let Some(summary) = summaries.get(&pane.pane_id) {
                item["awaiting_confirmation"] = json!(
                    summary.awaiting
                        && activity
                            .get(&pane.pane_id)
                            .is_some_and(|(_, polls)| *polls > 0)
                        && Some(summary.hash) == activity_hash(activity, pane.pane_id)
                );
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
            cwd: None,
            session_id: None,
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
        }
    }

    /// A session where nothing reports for itself, which is every pane that is not
    /// a Claude Code one with the hooks installed.
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
            ),
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
            ("opencode", "opencode"),
            ("pi", "pi"),
            ("pi-agent", "pi"),
        ] {
            assert_eq!(detect_kind("shell", Some(command)), Some(kind), "{command}");
        }
    }

    #[test]
    fn detects_the_program_behind_a_path_and_its_arguments() {
        assert_eq!(
            detect_kind("shell", Some("/usr/local/bin/Codex --resume")),
            Some("codex"),
        );
    }

    #[test]
    fn detects_a_kind_named_as_a_whole_word_in_the_title() {
        assert_eq!(detect_kind("aider — src/main.rs", None), Some("aider"));
        assert_eq!(detect_kind("CLAUDE", None), Some("claude"));
    }

    /// The whole reason detection is token-based rather than substring-based.
    #[test]
    fn does_not_detect_a_kind_inside_a_longer_word() {
        assert_eq!(detect_kind("pip install", Some("zsh")), None);
        assert_eq!(detect_kind("piano", None), None);
        assert_eq!(detect_kind("shell", Some("pipenv run pi")), None);
    }

    #[test]
    fn detects_an_agent_behind_the_interpreter_that_runs_it() {
        assert_eq!(
            detect_kind("shell", Some("node /opt/bin/gemini")),
            Some("gemini"),
        );
        assert_eq!(
            detect_kind("shell", Some("python3 -m aider")),
            Some("aider")
        );
        assert_eq!(
            detect_kind("shell", Some("node /opt/lib/claude.js")),
            Some("claude"),
        );
    }

    /// The guard that keeps the interpreter case from swallowing every command line
    /// that happens to say an agent's name.
    #[test]
    fn does_not_look_past_the_first_word_of_anything_else() {
        assert_eq!(detect_kind("shell", Some("git commit -m ask-claude")), None);
        assert_eq!(detect_kind("shell", Some("zsh")), None);
        assert_eq!(detect_kind("shell", Some("man codex")), None);
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
            Some(("reviewer", "manual")),
        );
        assert_eq!(
            classify(&pane(3, "new title", Some("codex")), &manual),
            Some(("reviewer", "manual")),
        );
        assert_eq!(
            classify(&pane(4, "codex", Some("codex")), &manual),
            Some(("codex", "auto")),
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
            Some(("codex", "auto")),
        );
        // Even when what it is running is a shell of its own.
        assert_eq!(
            classify(&latched(3, "codex", "bash -c rg"), &manual),
            Some(("codex", "auto")),
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
        assert_eq!(classify(&running, &manual(&[])), Some(("claude", "auto")));
        assert_eq!(
            classify(&running, &manual(&[(3, "reviewer")])),
            Some(("reviewer", "manual")),
        );
    }

    /// Nothing rewrites a pane title when the program that set it exits, so a
    /// pane that ran an agent and went back to its prompt used to stay an agent
    /// for as long as it stayed open.
    #[test]
    fn a_shell_prompt_outranks_the_title_the_last_program_left() {
        assert_eq!(detect_kind("claude", Some("zsh")), None);
        assert_eq!(detect_kind("claude", Some("/usr/bin/zsh -l")), None);
        assert_eq!(detect_kind("claude", Some("-zsh")), None);
        // Still an agent while it is actually running one, and still detected by
        // title when nothing has said what it runs.
        assert_eq!(detect_kind("claude", Some("claude")), Some("claude"));
        assert_eq!(detect_kind("claude", None), Some("claude"));
        // A command that is neither a shell nor an agent leaves the title alone:
        // a wrapper we do not recognise must not hide a real agent.
        assert_eq!(detect_kind("claude", Some("mise exec")), Some("claude"));
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
    fn changed_materially_wants_a_fifth_of_the_words_turned_over() {
        let before = (1..=20).fold(String::new(), |mut all, line| {
            all.push_str(&format!("line {line}\n"));
            all
        });
        let scrolled = format!("{before}line 21\n");
        assert!(
            !changed_materially(&before, &scrolled, 5),
            "one new line in twenty is not a new screen"
        );
        let rewrapped = before.replace('\n', " ");
        assert!(
            !changed_materially(&before, &rewrapped, 5),
            "a rewrap turns over nothing"
        );
        let after = (1..=20).fold(String::new(), |mut all, line| {
            all.push_str(&format!("other {line}\n"));
            all
        });
        assert!(changed_materially(&before, &after, 5));
        assert!(
            !changed_materially(&before, "", 5),
            "an empty screen is not news"
        );
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
            Some("/work/hocus"),
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
        assert!(user.contains("Folder: /work/hocus."), "{user}");
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

    /// What the model is shown for a turn: the agent's own message, and the question it
    /// was answering. A description of an answer written without sight of the question is
    /// half a description.
    #[test]
    fn a_turn_summary_asks_about_the_reply_and_the_request_behind_it() {
        let body = turn_summary_request_body(
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

        // A turn with no prompt behind it - a resumed session, a hook-driven
        // continuation - is summarised without one rather than against a stale question.
        let body = turn_summary_request_body("m", "claude", None, None, None, "Done.");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        let user = body["messages"][1]["content"].as_str().expect("a string");
        assert_eq!(user, "Agent: claude.\n\n--- the agent replied ---\nDone.");
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
                waiting(4, "  4  claude  idle"),
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
        for source in ["startup", "resume", "fork"] {
            assert_eq!(
                hook_phase(&crate::wire::HookEvent {
                    source: Some(source.to_owned()),
                    ..event("SessionStart")
                }),
                Some(HookPhase::Idle)
            );
        }
        for source in ["clear", "compact", "something_new"] {
            assert_eq!(
                hook_phase(&crate::wire::HookEvent {
                    source: Some(source.to_owned()),
                    ..event("SessionStart")
                }),
                None
            );
        }
        assert_eq!(hook_phase(&event("SessionStart")), None);
        assert_eq!(hook_phase(&event("Stop")), Some(HookPhase::Idle));
        assert_eq!(
            hook_phase(&event("UserPromptSubmit")),
            Some(HookPhase::Busy)
        );
        assert_eq!(hook_phase(&event("PreToolUse")), Some(HookPhase::Busy));
        assert_eq!(hook_phase(&event("PostToolUse")), Some(HookPhase::Busy));
        assert_eq!(
            hook_phase(&event("PermissionRequest")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(
            hook_phase(&notification("permission_prompt")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(
            hook_phase(&notification("agent_needs_input")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(
            hook_phase(&notification("worker_permission_prompt")),
            Some(HookPhase::Waiting)
        );

        // A finished turn nobody has answered yet is idleness, not a question.
        assert_eq!(hook_phase(&notification("idle_prompt")), None);
        assert_eq!(hook_phase(&notification("quota_auto_resume_fired")), None);
        // Ending a session is not a state to be in - callers check HOOK_SESSION_END.
        assert_eq!(hook_phase(&event(HOOK_SESSION_END)), None);
        // A version that grows an event we have never heard of costs us nothing.
        assert_eq!(hook_phase(&event("SomethingNewInAYearsTime")), None);
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
        assert_eq!(hook_phase(&failed), Some(HookPhase::Idle));
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
            hook_phase(&pre("AskUserQuestion")),
            Some(HookPhase::Waiting)
        );
        assert_eq!(hook_phase(&pre("ExitPlanMode")), Some(HookPhase::Waiting));
        assert_eq!(hook_phase(&pre("Bash")), Some(HookPhase::Busy));

        // And the answer coming back is ordinary work again, which is what clears the
        // wait without either tool needing a case of its own on the way out.
        let post = crate::wire::HookEvent {
            hook_event_name: "PostToolUse".to_owned(),
            tool_name: Some("AskUserQuestion".to_owned()),
            ..Default::default()
        };
        assert_eq!(hook_phase(&post), Some(HookPhase::Busy));
    }

    /// The events wired since the first eight, each in the state it names.
    #[test]
    fn the_rest_of_the_lifecycle_names_the_state_it_is_in() {
        for name in [
            "PostToolUseFailure",
            "PermissionDenied",
            "ElicitationResult",
            "SubagentStart",
            "SubagentStop",
            "PreCompact",
            "PostCompact",
        ] {
            assert_eq!(
                hook_phase(&event(name)),
                Some(HookPhase::Busy),
                "{name} is the agent working"
            );
        }
        assert_eq!(hook_phase(&event("Elicitation")), Some(HookPhase::Waiting));
        // A *background* session that finished, which is the pane's agent still working —
        // Claude Code raises this one from its background-job monitor, on the foreground
        // session's payload. The pane's own turn ending is `Stop`, which arrives earlier and
        // carries the words to describe it. See `agent::WAITING_NOTIFICATIONS`.
        assert_eq!(hook_phase(&notification("agent_completed")), None);
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

    /// A permission mode that answers by itself has nobody to interrupt. The phase is
    /// still whatever the agent said — only the toast is gated on this.
    ///
    /// Which prompts a mode answers is `zellij_utils::agent`'s to decide and to test; what is
    /// worth pinning here is that the row and the `!` are unaffected by the answer.
    #[test]
    fn a_wait_under_an_unattended_mode_is_drawn_but_not_announced() {
        assert!(waiting_needs_a_person(&request(Some("Bash"), None)));
        assert!(!waiting_needs_a_person(&request(
            Some("Bash"),
            Some("bypassPermissions")
        )));

        // The phase is unaffected either way: the agent said it stopped.
        assert_eq!(
            hook_phase(&request(Some("Bash"), Some("bypassPermissions"))),
            Some(HookPhase::Waiting)
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
            tool_input: Some(json!({"command": "cargo x test -p hocus-proto"})),
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
                json!({"command": "cargo x test -p hocus-proto"})
            )),
            Some("Bash cargo".to_owned()),
            "the program, not the command line"
        );
        assert_eq!(
            hook_detail(&pre(
                "Edit",
                json!({"file_path": "/work/hocus/src/main.rs"})
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
            tab: Some((9, "work".to_owned())),
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
            tab: Some((9, "work".to_owned())),
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
            tab: Some((9, "work".to_owned())),
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
            json,
            r#"[{"idle_seconds":120,"kind":"codex","pane_id":3,"preview":"ready","source":"auto","status":"running","title":"codex"},{"kind":"reviewer","pane_id":5,"preview":"ready","source":"manual","status":"held","title":"shell"}]"#,
        );
    }

    fn in_folder(pane_id: u32, kind: &str, cwd: &str, tab_id: usize) -> AgentPane {
        AgentPane {
            cwd: Some(cwd.to_owned()),
            tab: Some((tab_id, format!("tab {tab_id}"))),
            ..pane(pane_id, kind, Some(kind))
        }
    }

    #[test]
    fn list_json_carries_the_folder_the_project_and_the_branch() {
        let pane = in_folder(3, "codex", "/work/hocus/src", 9);
        let json = list_json(
            &[pane],
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "hocus")]),
            &branches(&[(3, "topic")]),
            &Summaries::new(),
            &no_hooks(),
            |_| String::new(),
        );

        assert_eq!(
            json,
            r#"[{"branch":"topic","cwd":"/work/hocus/src","kind":"codex","pane_id":3,"preview":"","project":"hocus","source":"auto","status":"running","tab":"tab 9","tab_id":9,"title":"codex"}]"#,
        );
    }

    #[test]
    fn strip_rows_group_agents_under_their_project() {
        let panes = vec![
            in_folder(3, "codex", "/work/pocus", 1),
            in_folder(4, "aider", "/elsewhere", 1),
            in_folder(5, "codex", "/work/hocus", 2),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "pocus"), (5, "hocus")]),
            &branches(&[(3, "main"), (5, "main")]),
            &Summaries::new(),
            &no_hooks(),
        );

        // Projects in name order with the unowned agent last, and no row
        // repeating what the headers above it already say.
        assert_eq!(
            rows,
            vec![
                project("hocus"),
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
            in_folder(3, "codex", "/work/hocus", 1),
            in_folder(4, "claude", "/work/trees/topic", 2),
            in_folder(5, "aider", "/work/hocus", 1),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "hocus"), (4, "hocus"), (5, "hocus")]),
            &branches(&[(3, "main"), (4, "topic"), (5, "main")]),
            &Summaries::new(),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("hocus"),
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
            in_folder(3, "codex", "/work/one", 1),
            in_folder(4, "codex", "/work/two", 2),
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
            in_folder(3, "codex", "/work/hocus", 1),
            in_folder(4, "claude", "/work/hocus/.claude/worktrees/topic", 2),
        ];
        let rows = strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(3, "hocus"), (4, "hocus")]),
            // Pane 4's folder is in the project, but no probe has answered for
            // it yet.
            &branches(&[(3, "main")]),
            &Summaries::new(),
            &no_hooks(),
        );

        assert_eq!(
            rows,
            vec![
                project("hocus"),
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
            cwd: Some("/work/hocus".to_owned()),
            ..pane(9, "codex", Some("codex"))
        };
        let panes = vec![
            // Tab 1 agrees, even though pane 4 has no known project.
            in_folder(3, "codex", "/work/hocus", 1),
            in_folder(4, "aider", "/elsewhere", 1),
            // Tab 2 has two projects, so it agrees on nothing.
            in_folder(5, "codex", "/work/hocus", 2),
            in_folder(6, "aider", "/work/pocus", 2),
            // Tab 3 has a shell, not an agent, and pane 9 is in no tab at all.
            in_folder(7, "zsh", "/work/hocus", 3),
            untabbed,
        ];
        let owners = owners(&[
            (3, "hocus"),
            (5, "hocus"),
            (6, "pocus"),
            (7, "hocus"),
            (9, "hocus"),
        ]);

        assert_eq!(
            tab_projects(&panes, &manual(&[]), &owners),
            BTreeMap::from([(1, "hocus".to_owned())]),
        );
    }

    #[test]
    fn tab_project_labels_name_a_tab_after_every_project_in_it() {
        let untabbed = AgentPane {
            cwd: Some("/work/hocus".to_owned()),
            ..pane(9, "codex", Some("codex"))
        };
        let panes = vec![
            // One project, named once however many agents are on it.
            in_folder(3, "codex", "/work/hocus", 1),
            in_folder(4, "aider", "/work/hocus", 1),
            // Two projects, sorted rather than left in pane order.
            in_folder(5, "codex", "/work/pocus", 2),
            in_folder(6, "aider", "/work/hocus", 2),
            // A shell is not an agent, and pane 9 is in no tab at all.
            in_folder(7, "zsh", "/work/hocus", 3),
            untabbed,
        ];
        let owners = owners(&[
            (3, "hocus"),
            (4, "hocus"),
            (5, "pocus"),
            (6, "hocus"),
            (7, "hocus"),
            (9, "hocus"),
        ]);

        assert_eq!(
            tab_project_labels(&panes, &manual(&[]), &owners),
            BTreeMap::from([
                (1, vec!["hocus".to_owned()]),
                (2, vec!["hocus".to_owned(), "pocus".to_owned()]),
            ]),
        );
    }

    #[test]
    fn a_tab_whose_agents_have_no_project_is_left_out() {
        let panes = vec![in_folder(3, "codex", "/elsewhere", 1)];

        assert!(tab_project_labels(&panes, &manual(&[]), &owners(&[])).is_empty());
    }

    #[test]
    fn a_tab_that_two_projects_disagree_over_stays_unassigned() {
        // Pane 7 agrees with pane 5, but pane 6 already broke the tab. Order
        // must not be able to heal it.
        let panes = vec![
            in_folder(5, "codex", "/work/hocus", 1),
            in_folder(6, "aider", "/work/pocus", 1),
            in_folder(7, "codex", "/work/hocus", 1),
        ];
        let owners = owners(&[(5, "hocus"), (6, "pocus"), (7, "hocus")]);

        assert!(tab_projects(&panes, &manual(&[]), &owners).is_empty());
    }

    // -- the idle summary box -------------------------------------------------

    fn a_summary(text: &str, awaiting: bool) -> Summary {
        Summary {
            text: text.to_owned(),
            title: "running the tests".to_owned(),
            awaiting,
            hash: 1,
        }
    }

    /// Every row exactly as wide as every other, or the frame does not close and
    /// the transcript shows through the ragged end of it.
    #[test]
    fn the_inset_is_a_box_of_one_width() {
        let rows = summary_box(
            7,
            "claude",
            &a_summary("Ran the tests, all green.", false),
            30,
            10,
        )
        .expect("room for a box");
        assert_eq!(rows.len(), 3, "a header, one line of summary, a footer");
        for row in &rows {
            assert_eq!(row.width(), 30, "{row}");
        }
        assert!(rows[0].starts_with("┌─ 7 · claude ─"), "{}", rows[0]);
        assert!(rows[0].ends_with('┐'));
        assert_eq!(
            rows[1], "│ Ran the tests, all green.  │",
            "padded to the frame"
        );
        assert!(rows[2].starts_with('└') && rows[2].ends_with('┘'));
    }

    /// The claim the strip's `!` makes, in the box the user is already reading.
    #[test]
    fn a_blocked_agent_is_marked_in_the_header() {
        let rows = summary_box(7, "claude", &a_summary("Shall I?", true), 30, 10).expect("a box");
        assert!(rows[0].contains("! "), "{}", rows[0]);
        assert_eq!(rows[0].width(), 30);
        // And an agent that merely finished is not.
        let rows = summary_box(7, "claude", &a_summary("Shall I?", false), 30, 10).expect("a box");
        assert!(!rows[0].contains('!'), "{}", rows[0]);
    }

    /// A model told to write two sentences occasionally writes ten, and the box
    /// has to end above the input either way.
    #[test]
    fn a_long_summary_is_cut_to_the_room_there_is_and_marked_where_it_was_cut() {
        let long = "word ".repeat(200);
        let rows = summary_box(7, "claude", &a_summary(&long, false), 30, 40).expect("a box");
        assert_eq!(
            rows.len(),
            SUMMARY_BOX_MAX_ROWS + 2,
            "the ceiling, not the text"
        );
        let last = &rows[SUMMARY_BOX_MAX_ROWS];
        assert!(last.contains('…'), "{last}");
        assert_eq!(last.width(), 30);

        // And the transcript is the tighter of the two ceilings.
        let rows = summary_box(7, "claude", &a_summary(&long, false), 30, 5).expect("a box");
        assert_eq!(rows.len(), 5);
    }

    /// Better nothing than a column of single words, or a box drawn over the
    /// input line because there was nowhere else for it to go.
    #[test]
    fn there_is_no_box_where_there_is_no_room_for_one() {
        let one = a_summary("Ran the tests.", false);
        assert!(summary_box(7, "claude", &one, SUMMARY_BOX_MIN_COLS + 3, 10).is_none());
        assert!(summary_box(7, "claude", &one, 30, 2).is_none());
        assert!(
            summary_box(7, "claude", &one, 30, 3).is_some(),
            "just enough"
        );
    }

    /// A kind is up to 32 characters and a pane can be in a narrow split, so the
    /// header can be asked for more room than the box has. The corner survives.
    #[test]
    fn a_header_too_wide_for_its_box_is_cut_rather_than_burst() {
        let kind = "a-very-long-agent-kind-indeed-xx";
        let rows = summary_box(7, kind, &a_summary("Done.", true), 26, 10).expect("a box");
        for row in &rows {
            assert_eq!(row.width(), 26, "{row}");
        }
        assert!(rows[0].ends_with('┐'), "{}", rows[0]);
    }
}
