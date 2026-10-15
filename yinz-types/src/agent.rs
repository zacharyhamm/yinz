//! Agent records and launch inputs shared by host and plugin callers.

use crate::pane_id::PaneId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use strum_macros::{Display, EnumString};

pub mod theme;

/// Launch marker for direct agent panes, whose command has no originating plugin.
pub const AGENT_PANE_ENV: &str = "YINZ_AGENT_PANE";

/// Internal argument resolved by the host before launching Pi.
pub const PI_EXTENSION_ARGUMENT: &str = "yinz-internal:pi-agent-report";
pub const PI_EXTENSION_FILE: &str = "yinz-agent-report.ts";

/// Recognize the entire reserved generated argument without the host environment.
pub fn is_pi_extension_argument(arg: &str) -> bool {
    arg == PI_EXTENSION_ARGUMENT
        || (std::path::Path::new(arg).is_absolute()
            && std::path::Path::new(arg).ends_with(
                std::path::Path::new("pi")
                    .join(PI_EXTENSION_ARGUMENT)
                    .join(PI_EXTENSION_FILE),
            ))
}

/// What the agent in a pane is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum AgentState {
    /// Working.
    Busy,
    /// Stopped and blocked on the user: a permission prompt, a question, a form.
    Waiting,
    /// Finished its turn and at its prompt.
    Idle,
    /// The agent's process exited; the pane holds what it left.
    Exited,
}

impl AgentState {
    /// `busy` as well as `Busy`.
    pub fn parse_lenient(name: &str) -> Option<AgentState> {
        let mut chars = name.trim().chars();
        let first = chars.next()?.to_uppercase().collect::<String>();
        let rest = chars.as_str().to_lowercase();
        format!("{first}{rest}").parse().ok()
    }
}

/// Who wrote a pane's agent record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum AgentSource {
    /// The pane was opened as an agent pane, and nothing has been heard from the agent since:
    /// the state is what the session can see (running, exited), not what the agent says.
    Spawned,
    /// A person named the pane's agent by hand (agent's `agent_register`), and nothing has been
    /// heard from the agent itself: the kind is theirs, the state is a placeholder.
    Registered,
    /// The agent itself said so — its hooks through `yinz action agent-report`, or a plugin
    /// speaking for it.
    Reported,
    /// The pane's foreground program is an agent nobody launched or registered through Yinz,
    /// such as one started by hand in a shell. Only `agent list` reports it; it is never a
    /// pane's record, and the state is a placeholder.
    Detected,
}

/// The agent running in a pane: what it is, what it is doing, and who said so.
///
/// Carried on `PaneInfo::agent`, so every reader of the pane manifest — plugins, `yinz
/// events`, `list-panes --json` — sees the same answer to "is this an agent, and is it waiting
/// on me". Written by the spawn path, by `PluginCommand::SetPaneAgent`, and by `yinz action
/// agent-report`, which is what coding-agent hooks call. A record is complete every time it is
/// set; nothing merges — with one stated exception, [`AgentInfo::run_id`], which the agent
/// never reports and the session therefore carries forward for it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentInfo {
    /// `claude`, `codex`, … — the program, not a version of it.
    pub kind: String,
    pub state: AgentState,
    #[serde(default = "AgentSource::spawned")]
    pub source: AgentSource,
    /// A few words on what it is doing: the tool it is running, mostly.
    #[serde(default)]
    pub detail: Option<String>,
    /// What it is blocked on, in one line, when the state is `Waiting`: the tool a permission
    /// prompt is about, the question it asked, the sentence an MCP server wrote.
    ///
    /// A field of its own rather than a reuse of `detail`, because the two are cleared by
    /// different events and read in different places: `detail` names a tool in a status row,
    /// this names a question to somebody who has to answer it.
    #[serde(default)]
    pub waiting_on: Option<String>,
    /// Whether this wait is one a person has to answer, when the state is `Waiting`.
    ///
    /// `false` where the permission mode is about to answer the prompt itself — see
    /// `agent::waiting_needs_a_person`, which is where this is decided, and which is the
    /// only place it is worked out. Read by whatever would interrupt somebody over the
    /// wait; the state, the record and the agent strip all take the agent at its word
    /// regardless, so a suppressed wait is still a wait everywhere it is drawn.
    #[serde(default)]
    pub wait_needs_a_person: bool,
    /// The folder the agent is working in, which is not always the pane's: an agent told to
    /// work in a worktree moved without the shell under it hearing.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The agent's own session id, where it has one, so a restart in the same pane can be
    /// told from a pane that merely went quiet.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The id of the run record a plugin is keeping for this pane's agent, where one is
    /// keeping one.
    ///
    /// Opaque to the session: written by whoever holds the agent's history, read back by
    /// whoever picks the pane up. That is what lets a warped pane's run continue in the
    /// session that adopted it — an agent that supplies no session id has nothing else
    /// stable to be found by.
    ///
    /// The one field of a record the agent itself never says, and so the one field that is
    /// carried forward rather than re-supplied: a report that leaves it `None` inherits the
    /// pane's, and only clearing the whole record clears it. See `apply_pane_agent_report`.
    #[serde(default)]
    pub run_id: Option<String>,
}

impl AgentInfo {
    /// Whether this record is a wait that a person has to answer.
    ///
    /// The two halves have to be read together: `wait_needs_a_person` is only meaningful while
    /// the state is `Waiting`, and a record that has moved on carries whatever the last report
    /// happened to set.
    pub fn is_waiting_on_a_person(&self) -> bool {
        self.state == AgentState::Waiting && self.wait_needs_a_person
    }
}

impl AgentSource {
    fn spawned() -> AgentSource {
        AgentSource::Spawned
    }
}

/// Everything it takes to start a coding agent, in terms every caller has: agent's
/// `agent_spawn` tool, the `new-agent` modal, and a Scheme plugin's `yinz-agent-spawn`.
///
/// Every field is optional because every caller has partial inputs - a form with blank
/// fields, a tool call with omitted keys, a Scheme hash naming two of them - and the defaults
/// belong in `agent::launch::plan`, where all of them get the same ones.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSpawnSpec {
    /// Run in the explicitly prepared project Bubblewrap environment.
    pub bubblewrap: bool,
    /// The agent to run, by kind: `claude`, `codex`, `pi`, `cursor`, `muse`, `opencode`,
    /// or any other name the agent detection would recognise. Resolved to a program by
    /// `agent::launch::binary_for`.
    pub kind: Option<String>,
    /// A program to run instead of the kind's default executable. A recognized program
    /// supplies the kind when it is omitted; an explicit conflicting kind is rejected.
    /// One of the two is required.
    pub command: Option<String>,
    /// Arguments before the ones the planner adds.
    pub args: Vec<String>,
    /// Passed as `--model <model>`, the one flag every agent spells the same.
    pub model: Option<String>,
    /// Optional reasoning effort, translated by the shared launch planner.
    pub effort: Option<String>,
    /// An opening prompt: the last positional argument for `claude`, `codex` and
    /// `cursor-agent`, and `--prompt <prompt>` for `opencode`.
    pub prompt: Option<String>,
    /// The branch to create. Absent means no Git work at all: run where we are.
    pub branch: Option<String>,
    /// What `branch` is cut from. Defaults to `agent::launch::DEFAULT_BASE`.
    pub base: Option<String>,
    /// Whether `branch` lives in a new worktree rather than in this checkout. Defaults to
    /// *true* once a branch is named, which is the safer of the two: a worktree leaves the
    /// checkout the caller is looking at alone.
    pub worktree: Option<bool>,
    /// The *directory the worktree is placed in*, relative to the repository's primary
    /// checkout (an absolute path is taken as it stands). Defaults to
    /// `agent::launch::WORKTREE_PREFIX`. Never the worktree itself: a worktree is always
    /// named after the branch it holds, so `<worktree_dir>/<branch>` is the whole of it and
    /// a caller has no way to ask for a worktree whose name and branch disagree.
    pub worktree_dir: Option<String>,
    /// Environment the agent starts with, on top of the pane's own. Carried by an `env(1)`
    /// wrapper in the command - `env K=V claude ...` - because a command pane has no
    /// environment of its own to set; `env` execs the agent, so the pane still runs it.
    pub env: BTreeMap<String, String>,
    /// How much the agent may do without asking, in the agent's own terms - see
    /// `agent::launch::shape_args` for what each kind accepts. `skip` is understood by every
    /// kind that has a way to turn permissions off. `opencode` has no full bypass, so its
    /// `skip` and `auto` both become `--auto`, which approves only what is not explicitly
    /// denied.
    pub permission_mode: Option<String>,
    /// A session to resume: an id, or `last` for the most recent.
    pub resume: Option<String>,
    /// Whether a `claude` pane reports its state through its hooks. Defaults to true; the
    /// hooks are handed to it inline, so nothing needs installing.
    pub hooks: Option<bool>,
    /// Whether an existing branch, or an existing worktree at the name this spec asks for, is
    /// the work the caller means. Defaults to *false*: a taken name is a collision, and the
    /// host cuts the next free one (`<branch>-2`, `<branch>-3`, …) for the branch and its
    /// worktree together, so an agent never silently joins somebody else's work.
    pub reuse: Option<bool>,
    /// Opening prompt used only when checkout preparation reuses an existing worktree.
    pub reuse_prompt: Option<String>,
    /// Whether this agent may start somewhere that is not a linked worktree - the repository's
    /// primary checkout, or a folder that is no repository at all. Defaults to *false*: an
    /// agent left loose in the checkout the user is working in is a thing to ask for
    /// deliberately, not the shape a spec falls into by leaving `branch` out.
    pub allow_outside_worktree: Option<bool>,
}

/// A started agent, as the host answers for it.
///
/// The pane is what the caller asked for; `branch` and `cwd` are what it *got*, which is not
/// always what it asked for - a name already taken is cut afresh with a suffix, and a branch
/// held by another worktree sends the agent there. Both travel back rather than being
/// re-derived from the request, because the request no longer knows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnedAgent {
    /// Observed destination on success; a known/requested destination on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<crate::internal_ids::TabId>,
    /// A launch-created tab, retained or uncertain on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_tab_id: Option<crate::internal_ids::TabId>,
    pub pane_id: PaneId,
    /// The branch the agent is on, when git work was done. `None` for a spawn that made no
    /// checkout.
    pub branch: Option<String>,
    /// The folder the pane runs in, when git work was done.
    pub cwd: Option<PathBuf>,
    /// The branch the caller asked for, present only when the agent ended up on another one:
    /// a taken name cut afresh with a suffix, or a reused worktree holding a different branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_branch: Option<String>,
    /// The agent joined a worktree that already existed (`--reuse`) instead of a new one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reused: bool,
    /// Other live agents already working in `cwd`. Reported, not refused: `--reuse` asked
    /// for this folder, but two agents editing one checkout can overwrite each other.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occupied_by: Vec<PaneId>,
}

/// Prepared or attempted checkout resources retained when launching an agent fails before
/// its pane is known. Creation may be incomplete; inspect before retrying a failed spawn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunchResources {
    /// Observed destination on success; a known/requested destination on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<crate::internal_ids::TabId>,
    /// A launch-created tab, retained or uncertain on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_tab_id: Option<crate::internal_ids::TabId>,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_branch: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reused: bool,
}

mod hooks;
pub use hooks::*;
pub mod agy;
pub use agy::{AGY_HOOK_EVENTS, AGY_HOOK_TIMEOUT_SECONDS};
pub mod control;
pub mod opencode;
pub mod provider;
pub mod tool_capture;
pub use opencode::*;
pub mod report;
