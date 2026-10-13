//! What the agent plugins say to each other.
//!
//! Message names are constants rather than string literals at each call site
//! because a typo in a pipe name is silent — the host delivers it, the target
//! matches nothing, and the sender waits forever.
//!
//! What is left here is the agent daemon's protocol and the shell-facing store
//! pipes. The store's own snapshot push is gone: projects and todos live in a
//! shared data-store namespace that every agent plugin reads directly, so there
//! is nothing to replicate and nothing to keep in step.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yinz_tile::prelude::TabId;
use yinz_types::internal_ids::SessionName;

use crate::agents;
use crate::location::Location;
use crate::projects;

/// agent → agentd: "keep me posted, and here are the manual overrides".
pub const AGENT_SUBSCRIBE: &str = "agent_subscribe";
/// agentd → everyone: "I just started; subscribe again if you were watching".
pub const AGENT_HELLO: &str = "agent_hello";
/// agentd → agent and agentd → store: a fresh pane snapshot.
pub const AGENT_SNAPSHOT: &str = "agent_snapshot";
/// agent → agentd: "read the Git checkouts again".
///
/// The one thing no host event reports: a `git checkout` in a folder the daemon
/// already probed changes the branch and nothing says so. Whoever shows a branch
/// asks for this; the answer arrives as an ordinary snapshot.
pub const AGENT_REFRESH: &str = "agent_refresh";
/// Move the project panel clockwise, from any of its views.
pub const AGENT_STRIP_REPOSITION: &str = "agent_strip_reposition";
/// Focus the project panel's tab bar.
pub const AGENT_STRIP_YIELD: &str = "agent_strip_yield";
/// Select the receiving view's focus stop.
pub const AGENT_STRIP_TAKE: &str = "agent_strip_take";
pub const PANEL_STOP: &str = "stop";
pub const STOP_LIST: &str = "list";
pub const STOP_HISTORY: &str = "history";
pub const STOP_TABS: &str = "tabs";
/// agents → agentd: "describe this agent now".
///
/// The one summary somebody asks for. The daemon buys exactly one of its own accord, when a
/// run closes, and reads no screens at all, so this is what describes an agent that is still
/// working. The guards left on the path are the ones that protect correctness rather than the
/// budget: `summaries "false"`, one request per target at a time, a pane that still looks like
/// an agent and has not dropped back to its shell prompt, and something to describe.
///
/// Broadcast rather than url-addressed, for the same reason [`AGENT_REFRESH`] is: a bare
/// `yinz:agent-service` url carries none of the alias' `shared true`, so it would miss the
/// running daemon and launch a second one. A broadcast only ever reaches plugins that are
/// already running, and this name is handled in the daemon and nowhere else.
pub const AGENT_SUMMARIZE: &str = "agent_summarize";
/// CLI → store: change something.
pub const STORE_MUTATE: &str = "store_mutate";
/// agent/CLI → store: read something without a subscription.
pub const STORE_QUERY: &str = "store_query";

/// → agent-service: one stateless completion, from a plugin or from `yinz pipe`.
///
/// A plugin owns its messages and
/// its own tools, answered to itself. The daemon keeps nothing between requests — everything
/// the model is to see travels in the [`CompletionRequest`] — which is what lets a caller
/// drive its own tool loop by sending the whole transcript back each round.
///
/// Discover exactly one `yinz:agent-service` in the current session through
/// `get_session_list`, then send with `destination_plugin_id`. This needs
/// `read_application_state` and `message_and_launch_other_plugins`. Discovery failure sends
/// nothing. Accept replies only from the discovered plugin ID, with the matching request ID.
/// A bare daemon URL can launch another instance, so callers must not use it as the address.
pub const COMPLETE: &str = "llm_complete";
/// agent-service → the caller: what the model said.
///
/// Addressed by `destination_plugin_id`, taken from the `PipeSource::Plugin` of the request,
/// so that a caller does not read
/// somebody else's answer. A CLI caller is answered on its own pipe instead.
pub const COMPLETE_RESULT: &str = "llm_complete_result";
/// One agent poll: what goes out and what comes back. The activity map rides
/// along both ways so the caller's copy stays the only truth — the poller keeps
/// no state of its own.
#[derive(Serialize, Deserialize)]
pub struct PollReply {
    /// `None` means no current session — a quiet reply, not a dead poller.
    pub panes: Option<Vec<agents::AgentPane>>,
    /// Hidden terminals omitted from `panes`, excluding stack members already listed there.
    /// They still contribute to tab names until they close or move.
    #[serde(default)]
    pub suppressed_panes: Vec<agents::AgentPane>,
    pub activity: agents::Activity,
    /// The overrides the poller classified with, so a subscriber deciding which
    /// panes are agents reaches the same answer it did. Without this a manually
    /// registered agent looks like an ordinary shell to everyone downstream.
    #[serde(default)]
    pub manual_kinds: BTreeMap<u32, String>,
    /// The yinz session these panes belong to.
    ///
    /// Here because the store is shared by every session on the machine, so anything agent
    /// writes about a *pane* has to say which session's pane it means — and agent has no other
    /// route to the name. The daemon holds it already.
    #[serde(default)]
    pub yinz_session_name: SessionName,
    /// Resolved agent folder to its repository or plain-folder definition.
    ///
    /// Keyed by folder *and machine*: a remote pane's path names a place on
    /// another host, and two panes sitting in the same path on different
    /// machines are in different checkouts. A local folder's key is its plain
    /// path, exactly as it was before hosts existed, so this is unchanged on the
    /// wire for a session with no remote panes.
    #[serde(default)]
    pub repositories: BTreeMap<Location, projects::Repository>,
    /// Hand-made tab assignments reported by the service.
    ///
    /// These remain available to project features without overriding automatic names.
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
pub use yinz_tile::agent::AgentEvent;
/// A Claude Code hook event, as its JSON arrives - the session's own reading of one, so the
/// daemon and `yinz action agent-report` cannot disagree about what an event means.
pub use yinz_tile::agent::HookEvent;

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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PredecessorsRequest {
    pub request_id: String,
    pub pane_id: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PredecessorsResult {
    pub request_id: String,
    pub pane_id: u32,
    pub anchor: Option<crate::agents::AgentRunHeader>,
    pub runs: Vec<crate::agents::AgentRunHeader>,
    pub diagnostic: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchRequest {
    pub request_id: String,
    #[serde(default)]
    pub query: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchRun {
    pub run_id: String,
    pub kind: String,
    pub title: String,
    pub revision: u64,
    pub snippet: String,
    pub started_at_ms: i64,
    /// The checkout the run worked in, as the archive table's column draws it. See
    /// [`crate::agents::StripRow::Run`]: a search hit is drawn in the same columns as the
    /// archive rows it replaces, so it has to carry the same cells.
    #[serde(default)]
    pub worktree: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchResult {
    pub request_id: String,
    #[serde(default)]
    pub runs: Vec<SearchRun>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub error: String,
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
    /// Opt in to a JSON success/error envelope. Raw pipe replies remain the default.
    #[serde(default)]
    pub structured_reply: bool,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// A read from a shell. agent reads the rows itself and never sends one.
#[derive(Serialize, Deserialize)]
pub struct Query {
    /// Unused, for the reason given on `Mutation`.
    #[serde(default)]
    pub request_id: String,
    /// Opt in to a JSON success/error envelope. Raw pipe replies remain the default.
    #[serde(default)]
    pub structured_reply: bool,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// One turn of a conversation the *caller* owns.
///
/// Every field the model is to see is here, because the daemon holds nothing between
/// requests. That is the whole design: a caller that wants a tool loop runs one by sending
/// this again with the assistant's calls and its own results appended.
///
/// The endpoint, the key and the provider are deliberately absent. A caller names messages,
/// tools and at most a model — never where to send them or with what credentials — so the
/// worst a hostile plugin can do is spend tokens at the user's own endpoint.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The caller's own correlation id, echoed on the reply untouched.
    ///
    /// A plugin needs one: it may have several in flight and is answered on a message name
    /// it shares with every other caller. Someone at a shell does not, and may leave it
    /// empty — the answer comes back down their own pipe — which is the stance
    /// [`Mutation::request_id`] already takes.
    #[serde(default)]
    pub request_id: String,
    pub messages: Vec<CompletionMessage>,
    /// Prepended as a `system` message.
    ///
    /// A convenience over writing one into `messages`, and the shape a caller wants that is
    /// asking one question rather than keeping a transcript.
    #[serde(default)]
    pub system: Option<String>,
    /// Override the configured model, unless the session pinned one — see
    /// `llm::pinned_completion_model`.
    #[serde(default)]
    pub model: Option<String>,
    /// OpenAI-shaped tool specs, passed to the endpoint verbatim.
    ///
    /// Untyped because they are the caller's, not ours: a schema here would be one more
    /// thing to keep in step with whatever the provider accepts this month, and every
    /// mistake in one is already reported by the endpoint in words.
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    /// `"auto"`, `"none"`, `"required"`, or a `{"type":"function",…}` object. Passed through.
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    /// Omitted from the body entirely when unset, for the reason
    /// `agents::summary_request_body` omits both: providers disagree about them, and a
    /// rejected request buys nothing over a slightly long answer.
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// One message, in the one shape all four roles fit through.
///
/// Flat where agent's own `api::WireMessage` is an untagged enum, because this crosses a
/// JSON boundary a TypeScript object has to be able to write by hand.
/// `completion::completion_request_body` is what re-nests it into the shapes the endpoint
/// expects.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompletionMessage {
    /// `system`, `user`, `assistant` or `tool`.
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// On an `assistant` message handed back: what the model asked for last round.
    #[serde(default)]
    pub tool_calls: Vec<CompletionToolCall>,
    /// On a `tool` message: which call this answers.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

/// One tool call, flattened out of the endpoint's `{id, type, function:{name, arguments}}`.
///
/// `arguments` stays the raw JSON *string* the model produced, undecoded. A model that emits
/// malformed arguments is a thing the caller has to be told about rather than have swallowed
/// by a parse failure here, and it is also the only form that survives a round trip back
/// into the next request byte for byte.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// What the turn cost, as the endpoint reported it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// What became of a [`CompletionRequest`].
///
/// One shape for every outcome with `error` empty on success, the stance [`ResumeResult::detail`] takes. An enum would read better in Rust and worse in
/// the TypeScript object this becomes, and a caller has to branch on failure either way.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompletionReply {
    pub request_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<CompletionToolCall>,
    /// `stop`, `tool_calls`, `length`, … as the endpoint named it. Empty when the request
    /// never reached one.
    #[serde(default)]
    pub finish_reason: String,
    #[serde(default)]
    pub usage: Option<CompletionUsage>,
    /// The model that actually answered, as the endpoint named it. Empty on a failure and
    /// on a provider that does not echo it.
    #[serde(default)]
    pub model: String,
    /// Empty on success. Otherwise the whole of what went wrong: a refusal from the daemon,
    /// `llm::api_error_message` for a non-2xx, or an answer that never arrived.
    #[serde(default)]
    pub error: String,
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
    fn a_completion_request_parses_with_nothing_but_its_messages() {
        let request: CompletionRequest =
            serde_json::from_str(r#"{"messages":[{"role":"user","content":"why?"}]}"#)
                .expect("parsed");
        assert!(request.request_id.is_empty());
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        assert!(request.messages[0].tool_calls.is_empty());
        assert_eq!(request.messages[0].tool_call_id, None);
        assert_eq!(request.system, None);
        assert_eq!(request.model, None);
        assert!(request.tools.is_empty());
        assert_eq!(request.tool_choice, None);
        assert_eq!(request.temperature, None);
        assert_eq!(request.max_tokens, None);
    }

    /// The round a caller's own tool loop sends back: the assistant's calls, then a result
    /// naming the call it answers.
    #[test]
    fn a_completion_request_round_trips_a_tool_round() {
        let request = CompletionRequest {
            request_id: "ts:hc1".to_owned(),
            messages: vec![
                CompletionMessage {
                    role: "assistant".to_owned(),
                    tool_calls: vec![CompletionToolCall {
                        id: "call_1".to_owned(),
                        name: "lookup".to_owned(),
                        arguments: r#"{"city":"Oslo"}"#.to_owned(),
                    }],
                    ..Default::default()
                },
                CompletionMessage {
                    role: "tool".to_owned(),
                    content: "clear".to_owned(),
                    tool_call_id: Some("call_1".to_owned()),
                    ..Default::default()
                },
            ],
            tools: vec![serde_json::json!({ "type": "function" })],
            tool_choice: Some(serde_json::json!("auto")),
            temperature: Some(0.5),
            max_tokens: Some(256),
            ..Default::default()
        };
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("completion-fixtures.json")).unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), fixtures["request"]);
        let json = serde_json::to_string(&request).expect("encoded");
        let back: CompletionRequest = serde_json::from_str(&json).expect("parsed");
        assert_eq!(back.request_id, "ts:hc1");
        assert_eq!(
            back.messages[0].tool_calls[0].arguments,
            r#"{"city":"Oslo"}"#
        );
        assert_eq!(back.messages[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(back.tool_choice, Some(serde_json::json!("auto")));
        assert_eq!(back.temperature, Some(0.5));
        assert_eq!(back.max_tokens, Some(256));
    }

    #[test]
    fn a_completion_reply_round_trips_with_tool_calls_and_usage() {
        let reply = CompletionReply {
            request_id: "ts:hc1".to_owned(),
            content: "here".to_owned(),
            tool_calls: vec![CompletionToolCall {
                id: "call_1".to_owned(),
                name: "lookup".to_owned(),
                arguments: "{}".to_owned(),
            }],
            finish_reason: "tool_calls".to_owned(),
            usage: Some(CompletionUsage {
                prompt_tokens: 38,
                completion_tokens: 24,
                total_tokens: 62,
            }),
            model: "gpt-5".to_owned(),
            error: String::new(),
        };
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("completion-fixtures.json")).unwrap();
        assert_eq!(serde_json::to_value(&reply).unwrap(), fixtures["reply"]);
        let json = serde_json::to_string(&reply).expect("encoded");
        let back: CompletionReply = serde_json::from_str(&json).expect("parsed");
        assert_eq!(back.request_id, "ts:hc1");
        assert_eq!(back.tool_calls, reply.tool_calls);
        assert_eq!(back.finish_reason, "tool_calls");
        assert_eq!(back.usage.unwrap().total_tokens, 62);
        assert_eq!(back.model, "gpt-5");
        // Success is the empty error, which is what a caller branches on.
        assert!(back.error.is_empty());
    }

    /// A provider that echoes no usage, and a caller that must not need one on the wire to
    /// read the answer.
    #[test]
    fn a_completion_reply_parses_without_its_optional_halves() {
        let reply: CompletionReply =
            serde_json::from_str(r#"{"request_id":"x","content":"hi"}"#).expect("parsed");
        assert_eq!(reply.content, "hi");
        assert!(reply.tool_calls.is_empty());
        assert!(reply.finish_reason.is_empty());
        assert_eq!(reply.usage, None);
        assert!(reply.error.is_empty());
    }
}
