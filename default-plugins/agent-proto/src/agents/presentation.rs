//! Agent strip, tab labels, mentions and list responses.

use super::{
    activity_hash, checkout_location, classify, status_cell, Activity, AgentPane, AgentRunHeader,
    AgentStatus, HookFeeds, HookPhase, Summaries, POLL_SECONDS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::TabId;
use yinz_types::agent::{AgentState, LiveAgentOrder};

/// How many lines of a pane the model sees for each agent in one `agent_list`.
pub(super) const PREVIEW_LINES: usize = 20;
/// The hard ceiling on one preview, whatever those lines hold.
pub(super) const PREVIEW_CHARS: usize = 500;

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

/// Whether the running agent is waiting for an answer, shared by the strip and model tools.
///
/// Gated on the pane being idle as well as on the answer, because a summary outlives the
/// moment it describes: it is deliberately kept while the agent works again — that is what
/// lets an agent returning to the same screen be free rather than another request — and an
/// agent that is working is not waiting on anybody, whatever it was waiting on before.
pub(super) fn awaiting_confirmation(
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

/// Marks reported state in agent details and notifications.
/// Compact list rows omit this marker.
pub const HOOK_MARKER: &str = "◆";

/// Full provider name, with its reported-state marker and optional remote host.
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

pub(super) fn compact_kind(kind: &str) -> String {
    match kind {
        "codex" => "CDX".into(),
        "claude" => "CLA".into(),
        "opencode" => "OPC".into(),
        "gemini" => "GEM".into(),
        "cursor" => "CUR".into(),
        "qwen" => "QWN".into(),
        "aider" => "AID".into(),
        "cline" => "CLI".into(),
        "pi" => "PIA".into(),
        "muse" => "MUS".into(),
        _ => format!(
            "{:<3}",
            kind.chars().take(3).collect::<String>().to_uppercase()
        ),
    }
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
    /// `text` abbreviates the provider and appends the host. `kind` retains the full
    /// provider name and reported-state marker for the details header.
    Agent {
        pane_id: u32,
        text: String,
        #[serde(default)]
        kind: String,
        /// Current evidence that this agent is waiting for an answer. Never inferred from
        /// its title or from a completed turn; shared with the marker and `agent_list`.
        #[serde(default)]
        waiting: bool,
        /// The remote host the agent runs on, also appended to `text`. A field of its own so
        /// the panel can lay the row out without parsing the compact line.
        #[serde(default)]
        host: Option<String>,
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
/// a line, so they can never disagree. Only identity and state belong in the list.
///
/// `owners` gives the project of a pane and `branches` its checkout's branch.
/// Neither is repeated on the agent's own line: the headers carry them, so an
/// agent row stays short enough to read in a narrow sidebar. Agents of no known
/// project have a header that says so.
/// Needs-input agents precede working agents, then idle agents and other states.
/// Within each activity group, the most recently received hook comes first, with
/// project, branch, and pane id breaking ties. Agents without hooks follow those with hooks.
pub fn strip_rows(
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    activity: &Activity,
    owners: &BTreeMap<u32, String>,
    branches: &BTreeMap<u32, String>,
    summaries: &Summaries,
    hooks: &HookFeeds,
) -> Vec<StripRow> {
    let mut agents: Vec<_> = panes
        .iter()
        .filter_map(|pane| {
            let (kind, _) = classify(pane, manual)?;
            let hooked = hooks.contains_key(&pane.pane_id);
            let cell = kind_cell(&compact_kind(&kind), pane.host.as_deref(), false);
            let waiting =
                awaiting_confirmation(summaries, activity, hooks, pane.status, pane.pane_id);
            let symbol = match pane.status {
                AgentStatus::Held => '■',
                AgentStatus::Exited => '×',
                AgentStatus::Running if waiting => '!',
                AgentStatus::Running => match hooks.get(&pane.pane_id).map(|feed| feed.phase) {
                    Some(HookPhase::Busy) => '●',
                    Some(HookPhase::Idle) => '○',
                    Some(HookPhase::Waiting) => '!',
                    None => match activity.get(&pane.pane_id) {
                        Some((_, 0)) => '●',
                        Some(_) => '○',
                        None => '?',
                    },
                },
            };
            // Rank the same resolved state that supplies the visible marker, including
            // hook precedence and summary-based requests for input.
            let state = match symbol {
                '!' => Some(AgentState::Waiting),
                '●' => Some(AgentState::Busy),
                '○' => Some(AgentState::Idle),
                _ => None,
            };
            let row = format!("{} {cell} {symbol}", pane.pane_id);
            Some((
                LiveAgentOrder::new(
                    state,
                    pane.last_hook_at_ms,
                    owners.get(&pane.pane_id).map(String::as_str),
                    branches.get(&pane.pane_id).map(String::as_str),
                    yinz_types::pane_id::PaneId::from_legacy(pane.pane_id, false),
                    "",
                ),
                owners.get(&pane.pane_id).map(String::as_str),
                branches.get(&pane.pane_id).map(String::as_str),
                StripRow::Agent {
                    pane_id: pane.pane_id,
                    text: row,
                    // The row already names the host; the history header only needs the kind.
                    kind: kind_cell(&kind, None, hooked),
                    waiting,
                    host: pane.host.clone(),
                },
            ))
        })
        .collect();
    agents.sort_by(|left, right| left.0.cmp(&right.0));

    // `None` is "no header written yet", which is not the same as a header for
    // an agent with no project — that one is written, and says so.
    let mut project: Option<Option<&str>> = None;
    let mut branch: Option<Option<&str>> = None;
    let mut rows = Vec::with_capacity(agents.len());
    for (_, owner, checkout, row) in agents {
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
        if let Some((kind, _)) = classify(pane, manual)
            && !candidates.iter().any(|candidate| candidate == &kind)
        {
            candidates.push(kind.to_owned());
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
            if pane.status == AgentStatus::Running
                && let Some((_, polls)) = activity.get(&pane.pane_id)
            {
                item["idle_seconds"] = json!(u64::from(*polls) * POLL_SECONDS);
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
