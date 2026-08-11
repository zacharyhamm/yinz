//! What the hocus plugins say to each other.
//!
//! Message names are constants rather than string literals at each call site
//! because a typo in a pipe name is silent — the host delivers it, the target
//! matches nothing, and the sender waits forever.
//!
//! Mutations and queries carry a `request_id`. Snapshots are whole-state
//! pushes, so a stale one is simply superseded by the next and there is nothing
//! to correlate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::agents;
use crate::projects;
use crate::todos;

/// hocus → agentd: "keep me posted, and here are the manual overrides".
pub const AGENT_SUBSCRIBE: &str = "agent_subscribe";
/// agentd → everyone: "I just started; subscribe again if you were watching".
pub const AGENT_HELLO: &str = "agent_hello";
/// agentd → hocus and agentd → store: a fresh pane snapshot.
pub const AGENT_SNAPSHOT: &str = "agent_snapshot";
/// hocus → store: "keep me posted".
pub const STORE_SUBSCRIBE: &str = "store_subscribe";
/// store → everyone: "I just started; subscribe again if you were watching".
/// A reloaded store has an empty subscriber list and no way to rebuild it, so
/// it has to ask — nothing on the other side can tell a quiet store from a
/// store with nothing to say.
pub const STORE_HELLO: &str = "store_hello";
/// store → hocus: the whole registry and every todo list.
pub const STORE_SNAPSHOT: &str = "store_snapshot";
/// hocus/CLI → store: change something.
pub const STORE_MUTATE: &str = "store_mutate";
/// hocus/CLI → store: read something without a subscription.
pub const STORE_QUERY: &str = "store_query";
/// store → hocus: how a mutation or a query turned out.
pub const STORE_RESULT: &str = "store_result";

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
    /// Resolved agent CWD to its repository or plain-folder definition.
    #[serde(default)]
    pub repositories: BTreeMap<String, projects::Repository>,
}

/// Everything the store owns, pushed whole on every change.
///
/// Whole-state rather than deltas because the store is small (a project
/// registry and a few todo lists) and a dropped delta is an invisible
/// divergence, whereas a dropped snapshot is corrected by the next one.
#[derive(Default, Serialize, Deserialize)]
pub struct StoreSnapshot {
    pub projects: projects::Registry,
    pub todos: todos::Store,
    /// Increases on every push. A subscriber that sees a lower value than it
    /// already holds is looking at a reordered delivery and should drop it.
    pub seq: u64,
}

/// A change to the store. `args` stays loosely typed so the CLI can drive the
/// same path the plugin does without a second schema to keep in step.
#[derive(Serialize, Deserialize)]
pub struct Mutation {
    /// Echoed back in the `StoreResult` so the caller can finish the tool
    /// call that asked for it. Absent from a hand-typed CLI mutation, which is
    /// answered on its own pipe and has nothing to correlate.
    #[serde(default)]
    pub request_id: String,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// How a mutation or a query turned out. Both sides of the store run the same
/// `OpResult`, so one reply shape covers reads and writes alike.
#[derive(Serialize, Deserialize)]
pub struct StoreResult {
    pub request_id: String,
    pub ok: bool,
    /// What to show the model: the new state on success, why not on failure.
    pub message: String,
}

/// A read that does not go through a subscription. Only the CLI and other
/// plugins use this — hocus answers its own read tools from its cached
/// snapshot rather than paying a round trip.
#[derive(Serialize, Deserialize)]
pub struct Query {
    /// Echoed back in the `StoreResult` so a plugin caller can match the answer
    /// to the question it asked. Absent from a hand-typed CLI query, which is
    /// answered on its own pipe and has nothing to correlate.
    #[serde(default)]
    pub request_id: String,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
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
}
