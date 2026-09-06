//! What the hocus plugins say to each other.
//!
//! Message names are constants rather than string literals at each call site
//! because a typo in a pipe name is silent — the host delivers it, the target
//! matches nothing, and the sender waits forever.
//!
//! What is left here is the agent daemon's protocol and the shell-facing store
//! pipes. The store's own snapshot push is gone: projects and todos live in a
//! shared data-store namespace that every hocus plugin reads directly, so there
//! is nothing to replicate and nothing to keep in step.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zellij_tile::prelude::TabId;
use zellij_types::internal_ids::SessionName;

use crate::agents;
use crate::location::Location;
use crate::projects;

/// hocus → agentd: "keep me posted, and here are the manual overrides".
pub const AGENT_SUBSCRIBE: &str = "agent_subscribe";
/// agentd → everyone: "I just started; subscribe again if you were watching".
pub const AGENT_HELLO: &str = "agent_hello";
/// agentd → hocus and agentd → store: a fresh pane snapshot.
pub const AGENT_SNAPSHOT: &str = "agent_snapshot";
/// hocus → agentd: "read the Git checkouts again".
///
/// The one thing no host event reports: a `git checkout` in a folder the daemon
/// already probed changes the branch and nothing says so. Whoever shows a branch
/// asks for this; the answer arrives as an ordinary snapshot.
pub const AGENT_REFRESH: &str = "agent_refresh";
/// hocus → hocus-agents: the agent strip as drawn rows, with the focused pane and the
/// daemon's idle summaries (`agents::Strip` as JSON).
///
/// hocus stays the one place that knows which panes are agents, which project
/// owns them and what branch they are on; the hocus-agents pane is a view over
/// the finished rows, so the two can never disagree about a line.
///
/// The summaries ride along whole rather than only the selected one, because the selection
/// lives in the agents pane and moves under `j`/`k` there: asking hocus for the box on every
/// keystroke would put a round trip between the cursor and what it is pointing at.
pub const AGENT_STRIP: &str = "agent_strip";
/// hocus-agents → hocus: "I just loaded; send me the strip".
///
/// Carries a `plugin_url`, so it opens a hocus when there is none to ask - the agent list is a
/// view over rows only hocus can compute, and a list with nothing feeding it is not worth
/// having. The hocus it opens that way is **visible but not focused**: the panel is one thing
/// with one visibility, so a list on screen means a conversation on screen, but the user asked
/// for the list and the keyboard stays where they put it. [`OPENED_BY`] is what says so.
pub const AGENT_STRIP_HELLO: &str = "agent_strip_hello";
/// The argument on [`AGENT_STRIP_HELLO`] naming who sent it.
///
/// A hocus that receives the greeting *before its own first `PaneUpdate`* is the instance the
/// greeting just launched - nothing else can be that early - and knows not to take the focus
/// back off the list. One that has already heard from the server was open before the message
/// and stays as it is.
pub const OPENED_BY: &str = "opened_by";
/// The value of [`OPENED_BY`] on a greeting from the agent list.
pub const OPENED_BY_AGENTS: &str = "agents";
/// hocus → hocus-agents: open the agent list, or reveal the one already open.
///
/// Deliberately a name of its own rather than a `plugin_url` hung on `AGENT_STRIP`: that message
/// is sent on every snapshot, and a url attached to it would re-open the agent list every second
/// after the user closed it. Nothing reads the payload - the message exists for the launch-or-reuse
/// its url performs, and the pane answers with `AGENT_SELECTION` once it is up.
///
/// Sent **without** a url by `State::greet_agents_pane` on hocus's load, which asks an agent
/// list that already exists to announce itself without opening one that does not. On load there
/// is nothing yet to say whether the panel is up, and a list appearing beside a hocus the user
/// has not revealed would be the panel opening itself.
///
/// Sent **with** the url by `State::open_agents_pane`, which is how a revealed panel grows its
/// missing half: the panel is both halves or it is nothing, so a conversation on screen with no
/// list beside it is a state to repair rather than one to keep. The `a` keybinding carries the
/// url too, and is the other thing that opens a list.
pub const AGENT_STRIP_OPEN: &str = "agent_strip_open";
/// hocus-agents → hocus: which row the cursor is on (`Selection` as JSON).
///
/// Nothing is drawn from it any more — the summary box moved into the agents pane, which is
/// where the cursor already lives. What is left is the one thing only this message carries:
/// the agent list's *plugin id*, which is how hocus finds the pane it has to place beside
/// itself. A pane url cannot tell two instances apart, so there is no other way to learn it.
pub const AGENT_SELECTION: &str = "agent_selection";
/// hocus-agents → hocus: "move me out of the way" - Ctrl-o pressed in the list.
///
/// The list has no geometry of its own: hocus places both overlay panes, because the
/// arithmetic for one of them depends on the other. So the key is a request rather than a move -
/// the pane says it was pressed and hocus answers by walking the list clear of a tiled pane,
/// exactly as the same key does for the conversation.
pub const AGENT_STRIP_REPOSITION: &str = "agent_strip_reposition";
/// hocus-agents → hocus: "put the panel away" - Esc pressed in the list.
///
/// The panel is one thing with one visibility, and hocus is the half that can act on that: it
/// knows both plugin ids, and the list knows only its own. So the key is a request rather than a
/// hide of both, exactly as [`AGENT_STRIP_REPOSITION`] is for Ctrl-o. The list hides *itself*
/// on the same press rather than waiting for the round trip, so the message is only ever about
/// the other half.
///
/// Sent without a `plugin_url`: an Esc must never launch a hocus in order to hide one.
pub const AGENT_STRIP_HIDE: &str = "agent_strip_hide";
/// hocus-agents → hocus: "the ring stepped out of the list; take the keyboard".
///
/// The panel is one focus ring - the list half, the history half, the conversation's tab bar
/// and its input box, walked end to end by Tab and Shift+Tab. Two of those stops are in this
/// pane and two are in the other, so a step that crosses the seam has to be asked for: the
/// list knows only its own plugin id, exactly as it does for [`AGENT_STRIP_HIDE`] and
/// [`AGENT_STRIP_REPOSITION`], and hocus is the half that can focus either one.
///
/// [`PANEL_STOP`] names which of the conversation's two stops to land on - [`STOP_TABS`]
/// stepping forward off the history half, [`STOP_INPUT`] stepping backward off the list half.
///
/// Sent without a `plugin_url`: a Tab must never launch a conversation in order to focus one.
pub const AGENT_STRIP_YIELD: &str = "agent_strip_yield";
/// hocus → hocus-agents: "land on this stop" - the mirror of [`AGENT_STRIP_YIELD`].
///
/// Only the internal stop, not the focus: hocus holds the list's plugin id and moves the
/// keyboard itself with `focus_pane_with_id`, so all this message carries is which of the
/// list's two halves the arriving keys belong to. [`PANEL_STOP`] names it - [`STOP_LIST`]
/// wrapping forward off the input box, [`STOP_HISTORY`] stepping backward off the tab bar.
///
/// Sent without a `plugin_url`, for the reason [`AGENT_STRIP_YIELD`] is: a focus step never
/// grows the panel a half it is missing. Revealing does that; walking the ring does not.
pub const AGENT_STRIP_TAKE: &str = "agent_strip_take";
/// The argument on [`AGENT_STRIP_YIELD`] and [`AGENT_STRIP_TAKE`] naming the stop to land on.
pub const PANEL_STOP: &str = "stop";
/// [`PANEL_STOP`]: the agent list's list half.
pub const STOP_LIST: &str = "list";
/// [`PANEL_STOP`]: the agent list's history half.
pub const STOP_HISTORY: &str = "history";
/// [`PANEL_STOP`]: the conversation's tab bar.
pub const STOP_TABS: &str = "tabs";
/// [`PANEL_STOP`]: the conversation's input box.
pub const STOP_INPUT: &str = "input";
/// hocus → hocus-agents: the full history of the selected run.
pub const AGENT_RUN_DETAIL: &str = "agent_run_detail";
/// hocus-agents → hocus: fetch the next page of completed run headers.
pub const AGENT_ARCHIVE_MORE: &str = "agent_archive_more";
/// hocus-agents → agentd: "describe this agent now".
///
/// The one summary somebody asks for. The daemon buys exactly one of its own accord, when a
/// run closes, and reads no screens at all, so this is what describes an agent that is still
/// working. The guards left on the path are the ones that protect correctness rather than the
/// budget: `summaries "false"`, one request per target at a time, a pane that still looks like
/// an agent and has not dropped back to its shell prompt, and something to describe.
///
/// Broadcast rather than url-addressed, for the same reason [`AGENT_REFRESH`] is: a bare
/// `zellij:hocus-agentd` url carries none of the alias' `shared true`, so it would miss the
/// running daemon and launch a second one. A broadcast only ever reaches plugins that are
/// already running, and this name is handled in the daemon and nowhere else.
pub const AGENT_SUMMARIZE: &str = "agent_summarize";
/// hocus-agents → hocus: start the archived run under the cursor again, resuming its
/// session.
///
/// The panel names the row and nothing else, the same way [`AGENT_SUMMARIZE`] does: which
/// agent it was, which session, and which folder it ran in are all facts hocus holds about
/// the run, and deriving them a second time in the pane that draws the row is exactly the
/// duplication that split hocus and hocus-agents in the first place.
pub const AGENT_RESUME: &str = "agent_resume";
/// hocus → hocus-agents: what became of an [`AGENT_RESUME`].
///
/// Sent either way, because this is a request that can be refused for reasons the panel
/// cannot see — a run with no session id of its own, an agent kind with no resume spelling,
/// a VM that no longer exists — and a key press that silently does nothing is the one
/// outcome a confirmed action must not have.
pub const AGENT_RESUME_RESULT: &str = "agent_resume_result";
/// CLI → store: change something.
pub const STORE_MUTATE: &str = "store_mutate";
/// hocus/CLI → store: read something without a subscription.
pub const STORE_QUERY: &str = "store_query";

/// → hocus: a question put to the running hocus, from a plugin or from
/// `zellij pipe`.
pub const ASK: &str = "hocus_ask";
/// hocus → the asker: what became of it.
///
/// Addressed by `destination_plugin_id`, taken from the `PipeSource::Plugin` of
/// the ask, rather than broadcast like everything else here. It is also the only
/// thing that tells a plugin caller "hocus took it" apart from "there is no
/// hocus" — a broadcast to nobody is silent.
pub const ASK_ACK: &str = "hocus_ask_ack";

/// One agent poll: what goes out and what comes back. The activity map rides
/// along both ways so the caller's copy stays the only truth — the poller keeps
/// no state of its own.
#[derive(Serialize, Deserialize)]
pub struct PollRequest {
    pub manual_kinds: BTreeMap<u32, String>,
    pub activity: agents::Activity,
    /// The tab-to-project assignments the user made by hand, for the daemon to
    /// pass on to whoever names the tabs.
    ///
    /// hocus makes these and hocus-store publishes them, and the two never meet:
    /// hocus is a pane the user opens, the store is a background plugin, and a
    /// plugin cannot address another by name without launching one. The daemon
    /// is already between them, already holds `manual_kinds` for exactly this
    /// reason, and already echoes what it holds to every subscriber.
    #[serde(default)]
    pub tab_projects: BTreeMap<TabId, String>,
}

#[derive(Serialize, Deserialize)]
pub struct PollReply {
    /// `None` means no current session — a quiet reply, not a dead poller.
    pub panes: Option<Vec<agents::AgentPane>>,
    pub activity: agents::Activity,
    /// The overrides the poller classified with, so a subscriber deciding which
    /// panes are agents reaches the same answer it did. Without this a manually
    /// registered agent looks like an ordinary shell to everyone downstream.
    #[serde(default)]
    pub manual_kinds: BTreeMap<u32, String>,
    /// The zellij session these panes belong to.
    ///
    /// Here because the store is shared by every session on the machine, so anything hocus
    /// writes about a *pane* has to say which session's pane it means — and hocus has no other
    /// route to the name. The daemon holds it already.
    #[serde(default)]
    pub zellij_session_name: SessionName,
    /// Resolved agent folder to its repository or plain-folder definition.
    ///
    /// Keyed by folder *and machine*: a remote pane's path names a place on
    /// another host, and two panes sitting in the same path on different
    /// machines are in different checkouts. A local folder's key is its plain
    /// path, exactly as it was before hosts existed, so this is unchanged on the
    /// wire for a session with no remote panes.
    #[serde(default)]
    pub repositories: BTreeMap<Location, projects::Repository>,
    /// The hand-made tab assignments, as the last [`PollRequest`] carried them.
    ///
    /// Relayed rather than derived: only hocus knows what the user assigned, and
    /// only the store publishes tab names, so this is the leg between them.
    #[serde(default)]
    pub tab_projects: BTreeMap<TabId, String>,
    /// The terminal pane the user is looking at, if it is one.
    ///
    /// Here rather than on `AgentPane` because it is a fact about the session, not
    /// about a pane, and one field costs one place to set it. Only the daemon can
    /// answer it: a plugin cannot read its own focus off a `PaneUpdate`.
    #[serde(default)]
    pub focused: Option<u32>,
    /// What the model last said about each idle agent's pane.
    ///
    /// Only the daemon can answer this: it is the one thing watching for the busy→idle
    /// edge, and it holds the endpoint. Rides with the snapshot rather than on a message
    /// of its own because a summary is only ever read beside the pane it is about.
    #[serde(default)]
    pub summaries: agents::Summaries,
    /// What each Claude Code pane has said about itself, over its own hooks.
    ///
    /// Only the daemon can answer this either: it is what receives every `agent_event`.
    /// Where a pane has one of these, it is the truth about that pane's state and
    /// the screen-diff heuristic beside it is not — see [`agents::HookFeed`].
    #[serde(default)]
    pub hooks: agents::HookFeeds,
    /// Active hook sessions and the last completed one. Full archives live in the shared store.
    #[serde(default)]
    pub runs: Vec<agents::AgentRunHeader>,
}

/// One hook event as the session hands it to plugins: `Event::AgentEvent`. The daemon reads
/// `raw` back into a [`HookEvent`] for the fields the typed event does not carry.
pub use zellij_tile::agent::AgentEvent;
/// A Claude Code hook event, as its JSON arrives - the session's own reading of one, so the
/// daemon and `zellij action agent-report` cannot disagree about what an event means.
pub use zellij_tile::agent::HookEvent;

/// Where the cursor is in the agents pane.
///
/// `plugin_id` identifies the agents pane that sent the selection.
///
/// `pane_id` is `None` when nothing selectable is on the list at all, which is a strip
/// with no agents in it.
#[derive(Serialize, Deserialize)]
pub struct Selection {
    pub plugin_id: zellij_tile::prelude::PluginId,
    #[serde(default)]
    pub pane_id: Option<u32>,
    #[serde(default)]
    pub run_id: Option<String>,
}

/// What [`AGENT_SUMMARIZE`] names: one row of the agent list, as the panel has it.
///
/// Exactly one of the two is set, because a row carries one or the other and the panel
/// sends what its row has. Which of them it is decides the whole shape of the work: a live
/// agent has a pane to read and a run the daemon is already holding, an archived one has
/// neither and has to be loaded from the store. The panel knows nothing about that
/// difference and should not — it names the row and lets the daemon choose the path.
#[derive(Serialize, Deserialize)]
pub struct SummarizeRequest {
    #[serde(default)]
    pub pane_id: Option<u32>,
    #[serde(default)]
    pub run_id: Option<String>,
}

/// What [`AGENT_RESUME`] names: one archived run, by the id the panel's row carries.
///
/// One field where [`SummarizeRequest`] has two, because only an archived run can be
/// resumed. A live agent already has its pane, and handing its session id to a second
/// process would put two agents on one conversation.
#[derive(Serialize, Deserialize)]
pub struct ResumeRequest {
    pub run_id: String,
}

/// What became of an [`AGENT_RESUME`], as [`AGENT_RESUME_RESULT`] carries it.
///
/// `detail` is empty on success and is the reason otherwise — the strings
/// `launch::resume_command` hands back, which say what is missing rather than that
/// something is. The run id rides along so the panel can tell an answer about the row
/// under the cursor from one about the row it has since walked off.
#[derive(Serialize, Deserialize)]
pub struct ResumeResult {
    pub run_id: String,
    #[serde(default)]
    pub detail: String,
}

/// A change to the store. `args` stays loosely typed so the CLI can drive the
/// same path the plugin does without a second schema to keep in step.
#[derive(Serialize, Deserialize)]
pub struct Mutation {
    /// Unused by the CLI, which is answered on its own pipe and has nothing to
    /// correlate. Kept so that a caller with several calls in flight could tell
    /// the answers apart.
    #[serde(default)]
    pub request_id: String,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// A read from a shell. hocus reads the rows itself and never sends one.
#[derive(Serialize, Deserialize)]
pub struct Query {
    /// Unused, for the reason given on `Mutation`.
    #[serde(default)]
    pub request_id: String,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// One question put over the `hocus_ask` pipe.
///
/// A struct rather than a bare string so a later version can carry context —
/// the asking pane, a project hint — without needing a second message name and
/// a second handler on the hocus side.
#[derive(Serialize, Deserialize)]
pub struct Ask {
    pub text: String,
}

/// What hocus did with a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AskOutcome {
    /// Sent to the model straight away.
    Sent,
    /// hocus was busy; held, and will go out as soon as it is idle.
    Queued,
    /// Not taken at all. `detail` says why.
    Refused,
}

#[derive(Serialize, Deserialize)]
pub struct AskAck {
    pub outcome: AskOutcome,
    /// Empty on the two accepting outcomes, which have nothing to explain.
    #[serde(default)]
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What someone types at a shell: no `request_id`, because there is nothing
    /// to correlate when the answer comes back down the same pipe.
    #[test]
    fn a_cli_mutation_parses_without_a_request_id() {
        let mutation: Mutation =
            serde_json::from_str(r#"{"op":"todo_add","args":{"project":"work","text":"ship it"}}"#)
                .expect("parsed");
        assert_eq!(mutation.op, "todo_add");
        assert!(mutation.request_id.is_empty());
        assert_eq!(mutation.args["text"], "ship it");
    }

    /// Same for a query — and a plugin's query keeps the id it sent, which is
    /// the only thing that lets the reply find its way back to the right call.
    #[test]
    fn a_query_carries_a_request_id_only_when_one_was_sent() {
        let typed: Query = serde_json::from_str(r#"{"op":"todo_list","args":{"project":"work"}}"#)
            .expect("parsed");
        assert!(typed.request_id.is_empty());

        let from_plugin: Query =
            serde_json::from_str(r#"{"request_id":"call_7","op":"todo_list","args":{}}"#)
                .expect("parsed");
        assert_eq!(from_plugin.request_id, "call_7");
    }

    #[test]
    fn an_ask_and_its_acknowledgement_round_trip() {
        let json = serde_json::to_string(&Ask {
            text: "why is the build red?".to_owned(),
        })
        .expect("encoded");
        let ask: Ask = serde_json::from_str(&json).expect("parsed");
        assert_eq!(ask.text, "why is the build red?");

        let json = serde_json::to_string(&AskAck {
            outcome: AskOutcome::Refused,
            detail: "hocus has no working API configuration".to_owned(),
        })
        .expect("encoded");
        let ack: AskAck = serde_json::from_str(&json).expect("parsed");
        assert_eq!(ack.outcome, AskOutcome::Refused);
        assert_eq!(ack.detail, "hocus has no working API configuration");
    }

    /// The accepting outcomes have nothing to explain, so an asker must not
    /// need a `detail` on the wire to read them.
    #[test]
    fn an_acknowledgement_parses_without_a_detail() {
        let ack: AskAck = serde_json::from_str(r#"{"outcome":"Queued"}"#).expect("parsed");
        assert_eq!(ack.outcome, AskOutcome::Queued);
        assert!(ack.detail.is_empty());
    }
}
