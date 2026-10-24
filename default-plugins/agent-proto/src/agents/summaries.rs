//! Summary requests, parsing, and notification text.

use super::HookFeed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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
pub(super) const SUMMARY_TEXT_CHARS: usize = 400;
/// The longest title worth keeping. A pane name has a frame to fit in, and a model asked
/// for four words occasionally writes a sentence.
pub(super) const SUMMARY_TITLE_CHARS: usize = 40;
/// Below this there is nothing to summarise — a pane that has only drawn a prompt.
pub(super) const SUMMARY_MIN_CHARS: usize = 40;

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

/// Pane id → what the model last said about it.
pub type Summaries = BTreeMap<u32, Summary>;
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
/// [`super::WAITING_DETAIL_CHARS`] with a `…`. Cutting there rather than to the model title's tighter
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
