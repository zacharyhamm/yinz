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
/// CLI → store: change something.
pub const STORE_MUTATE: &str = "store_mutate";
/// hocus/CLI → store: read something without a subscription.
pub const STORE_QUERY: &str = "store_query";

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
