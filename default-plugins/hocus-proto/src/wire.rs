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
/// hocus → hocus-agents: the agent strip as drawn rows, with the focused pane
/// (`agents::Strip` as JSON).
///
/// hocus stays the one place that knows which panes are agents, which project
/// owns them and what branch they are on; the hocus-agents pane is a view over
/// the finished rows, so the two can never disagree about a line.
pub const AGENT_STRIP: &str = "agent_strip";
/// hocus-agents → hocus: "I just loaded; send me the strip".
pub const AGENT_STRIP_HELLO: &str = "agent_strip_hello";
/// hocus-agents → hocus: which row the cursor is on (`Selection` as JSON).
///
/// The strip goes one way and this comes back: hocus draws the selected agent's
/// summary, and the selection lives here, where the keys that move it are.
pub const AGENT_SELECTION: &str = "agent_selection";
/// CLI → store: change something.
pub const STORE_MUTATE: &str = "store_mutate";
/// hocus/CLI → store: read something without a subscription.
pub const STORE_QUERY: &str = "store_query";
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
    /// answer it: a global pane hears no `PaneUpdate` of its own.
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
