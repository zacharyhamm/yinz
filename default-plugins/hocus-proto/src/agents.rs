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
use std::collections::BTreeMap;
use std::path::Path;

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
    detect_kind(&pane.title, pane.command.as_deref())
        .map(|kind| (kind, "auto"))
        .or_else(|| pane.detected.as_deref().map(|kind| (kind, "auto")))
}

/// Fold one poll's pane content into the activity map.
///
/// The hash is `DefaultHasher` over the raw text: not stable across runs, never
/// persisted, only ever compared with the previous poll's.
pub fn observe(activity: &mut Activity, pane_id: u32, text: &str) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let hash = hasher.finish();
    match activity.get_mut(&pane_id) {
        Some((last, polls)) if *last == hash => *polls += 1,
        _ => {
            activity.insert(pane_id, (hash, 0));
        },
    }
}

/// The status column of one agent row: the plain status until activity says more,
/// then `busy`, `idle`, and `idle Nm`.
///
/// Minutes are the finest label on purpose: a per-second label would make every
/// poll a new row text for nothing.
pub fn status_cell(status: AgentStatus, activity: Option<&(u64, u32)>) -> String {
    let Some((_, polls)) = activity.filter(|_| status == AgentStatus::Running) else {
        return status.as_str().to_owned();
    };
    match u64::from(*polls) * POLL_SECONDS {
        0 => "busy".to_owned(),
        seconds if seconds < 60 => "idle".to_owned(),
        seconds => format!("idle {}m", seconds / 60),
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
/// Serialized: hocus computes these rows (it holds the snapshot, the project
/// registry and the branch probes) and pipes them to the hocus-agents pane,
/// which only draws them - see `wire::AGENT_STRIP`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StripRow {
    /// The project the agents below belong to.
    Project(String),
    /// The branch they are checked out on, under the project header above it.
    Branch(String),
    /// One agent: the pane it names, and the line as drawn.
    Agent { pane_id: u32, text: String },
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strip {
    pub rows: Vec<StripRow>,
    pub focused: Option<u32>,
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
pub fn strip_rows(
    panes: &[AgentPane],
    manual: &BTreeMap<u32, String>,
    activity: &Activity,
    owners: &BTreeMap<u32, String>,
    branches: &BTreeMap<u32, String>,
) -> Vec<StripRow> {
    let mut agents: Vec<(Option<&str>, Option<&str>, u32, String)> = panes
        .iter()
        .filter_map(|pane| {
            let (kind, _) = classify(pane, manual)?;
            let mut row = format!(
                "{:>3}  {kind}  {}",
                pane.pane_id,
                status_cell(pane.status, activity.get(&pane.pane_id))
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
    for (owner, checkout, pane_id, text) in agents {
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
        rows.push(StripRow::Agent { pane_id, text });
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
                status_cell(pane.status, activity.get(id))
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
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines[lines.len().saturating_sub(PREVIEW_LINES)..]
        .join("\n")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(PREVIEW_CHARS)
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
            Some(item)
        })
        .collect();
    Value::Array(items).to_string()
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(pane_id: u32, title: &str, command: Option<&str>) -> AgentPane {
        AgentPane {
            pane_id,
            title: title.to_owned(),
            command: command.map(str::to_owned),
            cwd: None,
            status: AgentStatus::Running,
            tab: None,
            detected: None,
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

    fn agent(pane_id: u32, text: &str) -> StripRow {
        StripRow::Agent {
            pane_id,
            text: text.to_owned(),
        }
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
        observe(&mut activity, 3, "working");
        assert_eq!(activity[&3].1, 0);
        observe(&mut activity, 3, "working");
        observe(&mut activity, 3, "working");
        assert_eq!(activity[&3].1, 2);
        observe(&mut activity, 3, "done");
        assert_eq!(activity[&3].1, 0);
    }

    #[test]
    fn status_cell_grades_a_running_agent_by_how_long_its_pane_sat_still() {
        let cell = |polls| status_cell(AgentStatus::Running, Some(&(1, polls)));
        assert_eq!(status_cell(AgentStatus::Running, None), "running");
        assert_eq!(cell(0), "busy");
        assert_eq!(cell(11), "idle");
        assert_eq!(cell(12), "idle 1m");
        assert_eq!(cell(36), "idle 3m");
        // A pane that is not running has a better word than any guess.
        assert_eq!(status_cell(AgentStatus::Held, Some(&(1, 40))), "held");
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
            mention_context("ask @codex and @5, please", &panes, &kinds, &activity),
            Some(
                "[@codex is agent pane 3 (codex, idle 2m) on tab \"work\"]\n\
                 [@5 is agent pane 5 (reviewer, busy)]"
                    .to_owned()
            ),
        );
        // An id mention, a duplicate, and punctuation after the body.
        assert_eq!(
            mention_context("@3 @codex again", &panes, &kinds, &activity),
            Some("[@3 is agent pane 3 (codex, idle 2m) on tab \"work\"]".to_owned()),
        );
        // Unresolved kinds, non-agent panes, embedded @ and bare @ say nothing.
        assert_eq!(
            mention_context("@aider @7 a@b @", &panes, &kinds, &activity),
            None
        );
        assert_eq!(
            mention_context("no mentions here", &panes, &kinds, &activity),
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
        let global = AgentPane {
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
            // Tab 3 has a shell, not an agent, and the global pane is in no tab.
            in_folder(7, "zsh", "/work/hocus", 3),
            global,
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
}
