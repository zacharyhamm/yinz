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
/// having. The hocus it opens that way starts **hidden**: the user asked for a list, not for a
/// conversation. [`OPENED_BY`] is what says so.
pub const AGENT_STRIP_HELLO: &str = "agent_strip_hello";
/// The argument on [`AGENT_STRIP_HELLO`] naming who sent it.
///
/// A hocus that receives the greeting *before its own first `PaneUpdate`* is the instance the
/// greeting just launched - nothing else can be that early - and hides itself. One that has
/// already heard from the server was open before the message and stays as it is.
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
/// Sent **without** a url by `State::greet_agents_pane` on hocus's load, which is the same
/// question asked of an agent list that already exists without opening one that does not: the
/// two panes are separate surfaces now, and hocus opening the list unbidden is exactly what
/// that separation forbids. The `a` keybinding's own path is the one that carries the url, and
/// the only thing that opens a list.
pub const AGENT_STRIP_OPEN: &str = "agent_strip_open";
/// hocus-agents → hocus: which row the cursor is on (`Selection` as JSON).
///
/// Nothing is drawn from it any more — the summary box moved into the agents pane, which is
/// where the cursor already lives. What is left is the one thing only this message carries:
/// the agent list's *plugin id*, which is how hocus finds the pane it has to place beside
/// itself. A pane url cannot tell two instances apart, so there is no other way to learn it.
pub const AGENT_SELECTION: &str = "agent_selection";
/// CLI → store: change something.
pub const STORE_MUTATE: &str = "store_mutate";
/// hocus/CLI → store: read something without a subscription.
pub const STORE_QUERY: &str = "store_query";
/// A Claude Code hook → agentd: one lifecycle event from a pane running claude.
///
/// The one message here a *shell* sends rather than a plugin. It is broadcast —
/// no `--plugin` — for the same reason the store's two pipes are: a bare
/// `zellij:hocus-agentd` url carries none of the alias' `shared true`, so it
/// would miss the running daemon and launch a second one. A broadcast only ever
/// reaches plugins that are already running, and this name is handled here and
/// nowhere else.
///
/// ```sh
/// zellij pipe --name agent_hook --args "pane_id=$ZELLIJ_PANE_ID" -- '{...}'
/// ```
pub const AGENT_HOOK: &str = "agent_hook";
/// The `--args` key naming the pane a hook fired in.
///
/// The script has to say: `PipeSource::Cli` carries a pipe id and nothing else,
/// and the server drops the calling pane for everything but an in-place plugin.
/// What the script has instead is `ZELLIJ_PANE_ID`, which the server exports
/// into every pane and which a hook process inherits from the agent it watches.
pub const HOOK_PANE_ID: &str = "pane_id";

/// hocus-ask → hocus: a question typed in the popup.
pub const ASK: &str = "hocus_ask";
/// hocus → hocus-ask: what became of it.
///
/// Addressed by `destination_plugin_id`, taken from the `PipeSource::Plugin` of
/// the ask, rather than broadcast like everything else here. It is also the only
/// thing that tells the popup "hocus took it" apart from "there is no hocus" — a
/// broadcast to nobody is silent, and the popup closes on this reply rather than
/// on the keypress so that silence is visible.
pub const ASK_ACK: &str = "hocus_ask_ack";

/// One agent poll: what goes out and what comes back. The activity map rides
/// along both ways so the caller's copy stays the only truth — the poller keeps
/// no state of its own.
#[derive(Serialize, Deserialize)]
pub struct PollRequest {
    pub manual_kinds: BTreeMap<u32, String>,
    pub activity: agents::Activity,
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
    /// Resolved agent folder to its repository or plain-folder definition.
    ///
    /// Keyed by folder *and machine*: a remote pane's path names a place on
    /// another host, and two panes sitting in the same path on different
    /// machines are in different checkouts. A local folder's key is its plain
    /// path, exactly as it was before hosts existed, so this is unchanged on the
    /// wire for a session with no remote panes.
    #[serde(default)]
    pub repositories: BTreeMap<Location, projects::Repository>,
    /// The terminal pane the user is looking at, if it is one.
    ///
    /// Here rather than on `AgentPane` because it is a fact about the session, not
    /// about a pane, and one field costs one place to set it. Only the daemon can
    /// answer it: a plugin cannot read its own focus off a `PaneUpdate`.
    #[serde(default)]
    pub focused: Option<u32>,
    /// The plugin pane the user is looking at, if it is one.
    ///
    /// The counterpart of `focused`, and never set at the same time. Only the daemon
    /// can answer this either, and for the same reason: the panes that would most like
    /// to know — hocus and the agents pane, both global — hear no `PaneUpdate` about
    /// themselves. It is what tells the two halves of the sidebar apart, which is what
    /// the summary inset is gated on.
    #[serde(default)]
    pub focused_plugin: Option<u32>,
    /// What the model last said about each idle agent's pane.
    ///
    /// Only the daemon can answer this: it is the one thing watching for the busy→idle
    /// edge, and it holds the endpoint. Rides with the snapshot rather than on a message
    /// of its own because a summary is only ever read beside the pane it is about.
    #[serde(default)]
    pub summaries: agents::Summaries,
    /// What each Claude Code pane has said about itself, over its own hooks.
    ///
    /// Only the daemon can answer this either: it is what receives [`AGENT_HOOK`].
    /// Where a pane has one of these, it is the truth about that pane's state and
    /// the screen-diff heuristic beside it is not — see [`agents::HookFeed`].
    #[serde(default)]
    pub hooks: agents::HookFeeds,
}

/// One Claude Code hook payload, forwarded whole by the generated hook script.
///
/// The field names are Claude Code's own, so the script has nothing to reshape and
/// needs no `jq`: it reads its stdin and hands it over. Every field but the event
/// name is optional and unknown ones are ignored, because Claude Code adds fields
/// between versions and an unrecognised one must not cost us the event.
///
/// Deliberately *not* named for `input::hooks::HookPoint`, which is a different
/// mechanism entirely — that one is zellij handing a moment of its own to a plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// `SessionStart`, `PreToolUse`, `Stop`, … — see [`agents::hook_phase`].
    pub hook_event_name: String,
    /// Claude Code's own session id. Kept so a pane that is restarted in place can
    /// be told from one that merely went quiet.
    #[serde(default)]
    pub session_id: String,
    /// The folder Claude Code is working in, which is not always the pane's shell
    /// folder — the agent is what moved, and this is the one that says where to.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Which notification this is, on a `Notification`. Most of them say nothing
    /// about whether the user is being waited on; [`agents::hook_phase`] is where
    /// the ones that do are named.
    #[serde(default)]
    pub notification_type: Option<String>,
    /// The tool about to run, or just finished, on a `PreToolUse`/`PostToolUse`.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// That tool's arguments. Free-form, attacker-influenced text of arbitrary
    /// size — whatever is taken from it is truncated hard and drawn as plain text.
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
}

/// Where the cursor is in the agents pane.
///
/// `plugin_id` is the pane's own id, sent so hocus can compare it against the
/// `focused_plugin` the daemon reports and know that *this* pane — and not the chat
/// beneath it — has the focus. A name or a title would be a guess; an id is the pane.
///
/// `pane_id` is `None` when nothing selectable is on the list at all, which is a strip
/// with no agents in it.
#[derive(Serialize, Deserialize)]
pub struct Selection {
    pub plugin_id: u32,
    #[serde(default)]
    pub pane_id: Option<u32>,
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

/// One question from the ask popup.
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
    /// Not taken at all. `detail` says why, and is what the popup shows.
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

    /// The accepting outcomes have nothing to explain, so the popup must not
    /// need a `detail` on the wire to read them.
    #[test]
    fn an_acknowledgement_parses_without_a_detail() {
        let ack: AskAck = serde_json::from_str(r#"{"outcome":"Queued"}"#).expect("parsed");
        assert_eq!(ack.outcome, AskOutcome::Queued);
        assert!(ack.detail.is_empty());
    }
}
