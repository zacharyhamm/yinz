//! Everything that talks to an OpenAI-compatible endpoint: config resolution,
//! request construction, and SSE stream framing.
//!
//! Two transports, chosen by the `api` config key and modelled by [`Api`]:
//! `/v1/chat/completions`, which every OpenAI-compatible provider speaks, and
//! OpenAI's own `/v1/responses`, which is the only endpoint that accepts
//! `reasoning_effort` and `tools` in the same request. They differ in the
//! request shape and in the event stream, and in nothing else: both are parsed
//! down to the same [`SseItem`]s, so no caller knows which one is in use.
//!
//! Nothing in this module calls the Zellij host, so it is all unit-testable on
//! the host triple (see `make test`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::tools::{self, ToolCall};
use crate::{Message, Reasoning, ReasoningItem, ReasoningKey, Role};

// Endpoint shaping lives in hocus-proto so hocus-agentd, which calls the same endpoint to
// summarise an idle agent, cannot disagree with us about where it is. Re-exported rather
// than re-implemented: every call site here already spells them unqualified.
pub use hocus_proto::llm::{
    api_error_message, completions_url, is_openai, is_reasoning_model, models_url, non_empty,
    responses_url, DEFAULT_API_KEY_ENV, DEFAULT_BASE_URL, DEFAULT_MODEL,
};
/// SearxNG's own docker-compose default port.
pub const DEFAULT_SEARCH_URL: &str = "http://localhost:8888";
/// Bare name, so an installed dobbs is found on PATH without configuration.
pub const DEFAULT_DOBBS: &str = "dobbs";
/// The same, for the `zellij` the VM tools shell out to. A bare name is right for an
/// installed Zellij; a development checkout whose `zellij` is not the one on PATH sets
/// `zellij_path`, because a mismatched build can speak a different provider protocol.
pub const DEFAULT_ZELLIJ: &str = "zellij";
/// How many times in a row the model may call tools before the tools are taken
/// away and it has to answer from what it has. Without this a model that keeps
/// re-listing panes bills the user forever.
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 50;
/// How many search results the model gets. A SearxNG page is ~25, and every one
/// of them is paid for on every subsequent turn of the conversation.
pub const SEARCH_RESULTS: usize = 8;
pub const CONTEXT_MAX_BYTES: usize = 262_144;
pub const TOOL_RESULT_BYTES: usize = 8 * 1024;
pub const TOOL_RESULTS_BYTES: usize = 64 * 1024;
const TOOL_FIRST_PROMPT: &str = "You are Hocus. Use native Zellij tools. Before ID actions, call `list_session` unless the prior result gave IDs; use `list_other_sessions` for cross-session IDs. `warp_pane_here`/`warp_tab_here` bring a listed pane or tab from another session into this one; there is no tool for the other direction, so a request to send one out is answered by asking hocus in the destination session. Use only `tab_id`/`pane_id`, never `tab_number`. Do not verify mutations or retry failed pane moves. When executing an approved plan, treat its findings and decisions as settled: do not repeat its research, and call only the tools needed to apply and verify it. `send_input` controls running programs. No arbitrary commands. Swamp workflows: load the `workflow` family. Use workflow_search/get/schema, diagnose with workflow_run_get/logs/reports/data, then workflow_edit, workflow_validate and workflow_resume. Prepare replacement definitions from workflow_get source.content to preserve all fields. Pass the complete prior workflow_get result as expected for edit/delete. After failures inspect reports before changing or retrying. Existing-run controls require exact run UUIDs. Never repeat a mutation after an uncertain response. Active-project code: `find_files`/`search_files`/`read_file`; history: `git_log`/`git_show`; GitHub metadata: GET-only `github_read`. Call `agent_list` before agent actions; `agent_spawn` starts a new one, and `branch` cuts it a git worktree named after the branch; an agent outside a linked worktree needs `allow_outside_worktree`. Past agent work, every session on this machine: `agent_history_search` finds it, `agent_runs` lists runs, `agent_run` reads one - a hit's `run_id` and `revision` read around it. You author skills yourself with `skill_list`/`skill_read`/`skill_write`; `skill_enable`/`skill_disable` change one tier. Author TypeScript plugins yourself with `.ts` filenames; do not spawn an agent. Start with `plugin_docs section=quickstart`; extensionless names select TypeScript. Use `plugin_docs` as authority; `query=` before sections. `plugin_write` creates files and confirms grants. For edits: `plugin_outline`/`plugin_search`, `plugin_read` the range, then `plugin_edit` (`edits` batches). Never re-send a whole file you did not just write. `plugin_check name=…` checks saved files; `source` checks drafts. Writes/edits format and require strict types/lint. Fix diagnostics before reload. Create dependencies first. `plugin_format`: draft `source` or confirmed saved-file formatting with backup. Run `plugin_test` before reload. Recover omitted payloads with `tool_result_read` by call ID. `plugin_reload` for the first render or the load error (`headless` for a plugin with no pane, answered from its log), `plugin_log` for what it logged; a plugin that runs agents uses TypeScript async functions and `zellij.agent` - section `agents`, example `example_agent_harness`; inspect project source only when the docs leave a required detail unanswered or check/reload reports a mismatch; `plugin_list` is the user's plugin folder. Firecracker VMs: `vm_list` for every VM and its worktree and branch, then `vm_changes`/`vm_sync`/`vm_close` by name. Not every tool named here is in \
the tools array: the rest are loaded on demand, and calling one by name loads its family and \
runs it, so call the tool you want rather than asking for it. `tools_load` is there for when \
you do not know the name. Make independent calls together.";
/// Sent in planning mode, where the tool set is narrowed to the ones that only
/// read. Says so explicitly: a model that believes a missing tool is an
/// oversight spends the turn asking for it back.
const PLAN_PROMPT: &str = "Planning mode. You are working out an approach, not carrying it out. \
Only read-only tools are available - every tool that changes anything has been withheld, \
including ones you called earlier in this conversation - so do not plan around calling one, and \
do not report a change as made or as failed. This is not the on-demand loading: a family you \
load here arrives with its writing half still withheld. Investigate with the tools you have, then answer \
with a plan: what you found, what you would do, in order, and what each step touches. Do not \
end by asking permission; the user is asked for you.";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Which of the two wire protocols an endpoint is spoken to with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Api {
    /// `/v1/chat/completions`. The default for every provider except OpenAI's
    /// own endpoint, because it is the only one every OpenAI-compatible
    /// provider implements.
    #[default]
    Chat,
    /// OpenAI's `/v1/responses`. Opt-in: OpenAI's reasoning models refuse
    /// `reasoning_effort` alongside `tools` on chat/completions, and this is the
    /// endpoint that takes both.
    Responses,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Fully resolved endpoint, derived from `base_url` and `api`.
    pub url: String,
    pub api_key: String,
    pub model: String,
    /// Kept verbatim so the "not configured" banner can show what we resolved.
    pub base_url: String,
    pub api_key_env: String,
    /// Which wire protocol `url` speaks. See [`Api`].
    pub api: Api,
    /// Base URL of the SearxNG that backs `web_search`. Any instance on the
    /// network will do; it is a plain HTTP GET against its JSON API.
    pub search_url: String,
    /// A SOCKS5 proxy (`socks5://host:port`) that `web_search` requests are
    /// sent through, e.g. to reach a sandboxed agent's tailscale sidecar.
    /// Applies only to `web_search`; every other hocus request is direct.
    pub search_proxy: Option<String>,
    /// The `dobbs` binary the Slack tools shell out to. dobbs speaks raw TCP to
    /// its daemon, which a wasm plugin cannot, so the CLI is the only way in.
    /// Defaults to `dobbs`, i.e. whatever is on PATH.
    pub dobbs: String,
    /// A remote dobbs daemon to dial instead of the local one, passed to the CLI
    /// as `$DOBBS_ADDR`/`$DOBBS_TOKEN`. Unset leaves dobbs on its own resolution
    /// order: `~/.dobbs/remote.addr`, then the local daemon's addr file.
    pub dobbs_addr: Option<String>,
    pub dobbs_token: Option<String>,
    /// The `zellij` binary the VM tools shell out to. Managing a Firecracker VM means
    /// running the provider, which is a subprocess the CLI owns and a wasm plugin has
    /// no way to start itself. Defaults to `zellij`, i.e. whatever is on PATH.
    pub zellij: String,
    pub system_prompt: Option<String>,
    /// DeepSeek's opt-in thinking control. Omitted for endpoints that do not
    /// understand it.
    pub thinking: Option<String>,
    /// Optional reasoning effort, using the union of the values accepted by
    /// DeepSeek and OpenAI-compatible reasoning endpoints.
    pub reasoning_effort: Option<String>,
    /// Whether to advertise the Zellij tool set. On by default, but hocus points
    /// at arbitrary endpoints and plenty of local models fail outright when sent
    /// a `tools` array, so it needs an off switch.
    pub tools: bool,
    /// Consecutive tool-call rounds allowed in one exchange before the tools are
    /// withheld. Tunable because how far a turn has to reach varies with the
    /// model and the work.
    pub max_tool_rounds: usize,
    /// Whether a thinking model's own `reasoning_content` is handed back to it
    /// on the tool-calling turns of the transcript. On by default, because
    /// DeepSeek's thinking mode refuses the request outright without it and the
    /// tool loop cannot complete. Needs an off switch for the inverse case: an
    /// endpoint that streams reasoning but rejects it on input, as DeepSeek's
    /// own older `deepseek-reasoner` does.
    pub reasoning_echo: bool,
    /// Whether to ask for a final `usage` chunk via `stream_options`. On by
    /// default — it asks a streamed completion to report its usage — but the
    /// same stance as `tools`: `stream_options` is a 2024
    /// addition and an older proxy or a strict endpoint 400s on the unknown
    /// field, so it needs an off switch. Off, hocus shows no totals unless the
    /// provider includes usage without being asked.
    pub stream_usage: bool,
    /// Complete serialized request ceiling, in bytes (not tokens).
    pub context_max_bytes: usize,
}

/// A config problem, carrying enough context to render a useful banner.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError {
    pub message: String,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
}

impl Config {
    /// Resolve config from the plugin's KDL configuration and the session
    /// environment. KDL wins over env, env wins over the built-in default.
    ///
    /// There is deliberately no `api_key` KDL key: that would put a secret in a
    /// config file for no gain over pointing `api_key_env` at a different var.
    pub fn resolve(
        kdl: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let base_url = non_empty(kdl, "base_url")
            .or_else(|| non_empty(env, "OPENAI_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = non_empty(kdl, "model")
            .or_else(|| non_empty(env, "OPENAI_MODEL"))
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let api_key_env =
            non_empty(kdl, "api_key_env").unwrap_or_else(|| DEFAULT_API_KEY_ENV.to_owned());
        // Unset defaults to `responses` against OpenAI's own endpoint, since that is
        // the only one of the two it can send `reasoning_effort` and `tools` to
        // together, and to `chat` everywhere else, since it's the only one every
        // OpenAI-compatible provider implements. An explicit value always wins.
        let openai = is_openai(&base_url);
        let api = match enum_setting(kdl, "api", &["chat", "responses"]) {
            Ok(Some(value)) if value == "responses" => Api::Responses,
            Ok(Some(_)) => Api::Chat,
            Ok(None) if openai => Api::Responses,
            Ok(None) => Api::Chat,
            Err(message) => {
                return Err(ConfigError {
                    message,
                    base_url,
                    model,
                    api_key_env,
                })
            },
        };
        let search_url = non_empty(kdl, "search_url")
            .or_else(|| non_empty(env, "SEARXNG_URL"))
            .unwrap_or_else(|| DEFAULT_SEARCH_URL.to_owned());
        let search_proxy =
            non_empty(kdl, "search_proxy").or_else(|| non_empty(env, "SEARXNG_PROXY"));
        let dobbs = expand_home(
            &non_empty(kdl, "dobbs_path")
                .or_else(|| non_empty(env, "DOBBS_BIN"))
                .unwrap_or_else(|| DEFAULT_DOBBS.to_owned()),
            env,
        );
        let dobbs_addr = non_empty(kdl, "dobbs_addr").or_else(|| non_empty(env, "DOBBS_ADDR"));
        let dobbs_token = non_empty(kdl, "dobbs_token").or_else(|| non_empty(env, "DOBBS_TOKEN"));
        let zellij = expand_home(
            &non_empty(kdl, "zellij_path").unwrap_or_else(|| DEFAULT_ZELLIJ.to_owned()),
            env,
        );
        let system_prompt = non_empty(kdl, "system_prompt");
        let thinking =
            enum_setting(kdl, "thinking", &["enabled", "disabled"]).map_err(|message| {
                ConfigError {
                    message,
                    base_url: base_url.clone(),
                    model: model.clone(),
                    api_key_env: api_key_env.clone(),
                }
            })?;
        // The union of what the endpoints accept: `none` and `minimal` are
        // OpenAI's, the rest DeepSeek's and OpenRouter's. `none` is also the
        // documented way to keep tools working on chat/completions against a
        // reasoning model, which refuses any other effort beside a `tools` array.
        //
        // Unset defaults to `medium` only where all three of the conditions that
        // make the field safe hold: OpenAI's own endpoint, the Responses
        // transport, and a model that reasons. Any one of them missing and the
        // field is omitted, because sending it where it is not expected is the
        // same class of problem this key exists to avoid - `gpt-4o-mini`, the
        // built-in default model, rejects the `reasoning` object outright, and
        // chat/completions rejects an effort sent alongside `tools`.
        let reasoning_effort = enum_setting(
            kdl,
            "reasoning_effort",
            &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
        )
        .map_err(|message| ConfigError {
            message,
            base_url: base_url.clone(),
            model: model.clone(),
            api_key_env: api_key_env.clone(),
        })?
        .or_else(|| {
            (openai && api == Api::Responses && is_reasoning_model(&model))
                .then(|| "medium".to_owned())
        });
        // Anything but an explicit "false" leaves tools on: a typo should not
        // silently disable them.
        let tools =
            !non_empty(kdl, "tools").is_some_and(|value| value.eq_ignore_ascii_case("false"));
        // Same stance as `tools`: a typo or a nonsense value falls back to the
        // default rather than crippling the tool loop. Zero is nonsense too —
        // it would take the tools away before the first round.
        let max_tool_rounds = non_empty(kdl, "max_tool_rounds")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|rounds| *rounds > 0)
            .unwrap_or(DEFAULT_MAX_TOOL_ROUNDS);
        // Same stance again: only a literal "false" turns this off. A typo here
        // would break the tool loop against a thinking model rather than
        // merely degrade it, which is the worst thing a typo could do.
        let reasoning_echo = !non_empty(kdl, "reasoning_echo")
            .is_some_and(|value| value.eq_ignore_ascii_case("false"));
        // And again: a typo leaves the title's count exact rather than losing
        // every request to a 400.
        let stream_usage = !non_empty(kdl, "stream_usage")
            .is_some_and(|value| value.eq_ignore_ascii_case("false"));

        match non_empty(env, &api_key_env) {
            Some(api_key) => Ok(Config {
                url: match api {
                    Api::Chat => completions_url(&base_url),
                    Api::Responses => responses_url(&base_url),
                },
                api_key,
                model,
                base_url,
                api_key_env,
                api,
                search_url,
                search_proxy,
                dobbs,
                dobbs_addr,
                dobbs_token,
                zellij,
                system_prompt,
                thinking,
                reasoning_effort,
                tools,
                max_tool_rounds,
                reasoning_echo,
                stream_usage,
                context_max_bytes: non_empty(kdl, "context_max_bytes")
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v > 0)
                    .unwrap_or(CONTEXT_MAX_BYTES),
            }),
            // The environment we see is the one captured when the *session*
            // started, so "I exported it after launching zellij" is by far the
            // most common cause. Say so rather than just reporting it missing.
            None => Err(ConfigError {
                message: format!(
                    "No API key found: ${api_key_env} is not set.\n\nSet it in the environment \
                     the Zellij session was started from — exporting it after launching zellij \
                     will not be picked up. Or set api_key_env in the plugin configuration to \
                     name a different variable."
                ),
                base_url,
                model,
                api_key_env,
            }),
        }
    }

    /// The environment the `dobbs` CLI is run with. Empty unless a remote daemon
    /// is configured — dobbs already finds a local one by itself, and an addr
    /// with no token is refused by any daemon that requires one.
    pub fn dobbs_env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        if let Some(addr) = &self.dobbs_addr {
            env.insert("DOBBS_ADDR".to_owned(), addr.clone());
        }
        if let Some(token) = &self.dobbs_token {
            env.insert("DOBBS_TOKEN".to_owned(), token.clone());
        }
        env
    }
}

/// Expand a leading `~/` against `$HOME`. Nothing downstream does it: Zellij
/// hands a command's argv straight to exec, so a configured `~/dobbs/dobbs`
/// would fail with a bare "No such file or directory" naming nothing at all.
/// Left alone when `HOME` is absent, which is more honest than guessing.
pub(crate) fn expand_home(path: &str, env: &BTreeMap<String, String>) -> String {
    match path.strip_prefix("~/").zip(non_empty(env, "HOME")) {
        Some((rest, home)) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => path.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Request construction
// ---------------------------------------------------------------------------

pub fn request_headers(cfg: &Config) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Accept".to_owned(), "text/event-stream".to_owned()),
        (
            "Authorization".to_owned(),
            format!("Bearer {}", cfg.api_key),
        ),
    ])
}

/// Headers for the model list: plain JSON rather than a stream, and the same
/// credentials. Here rather than at the call site so nothing outside this module
/// has to know how the key is presented.
pub fn models_headers(cfg: &Config) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Accept".to_owned(), "application/json".to_owned()),
        (
            "Authorization".to_owned(),
            format!("Bearer {}", cfg.api_key),
        ),
    ])
}

#[cfg(test)]
pub fn request_body(cfg: &Config, transcript: &[Message]) -> Vec<u8> {
    request_body_with_project(cfg, transcript, None, false)
}

/// The same, in planning mode. A second wrapper rather than a fifth argument on
/// the one above, so the dozen tests that predate planning mode go on asserting
/// what they always did against an untouched call.
#[cfg(test)]
pub fn request_body_planning(cfg: &Config, transcript: &[Message]) -> Vec<u8> {
    request_body_with_project(cfg, transcript, None, true)
}

/// The request as it goes on the wire.
///
/// Borrowed rather than built with `json!`, because the alternative costs the
/// transcript twice over at once. A `serde_json::Value` tree clones every
/// message's content and boxes every node, and it has to be alive at the same
/// moment as both the transcript it was built from and the bytes serialized out
/// of it. Wasm linear memory never shrinks — there is no `memory.shrink` — so
/// the momentary peak, not the resting size, is what the host's memory limiter
/// sees and traps on. Serializing borrowed data straight to bytes skips the
/// middle copy entirely.
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Thinking<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    stream: bool,
    /// Asks for a final `usage` chunk, which is the only way a streamed
    /// completion reports separate input/output usage. OpenAI, OpenRouter,
    /// DeepSeek, vLLM and Ollama all honour it; anything that ignores it leaves
    /// the title without totals. `None` when `Config::stream_usage` is off,
    /// for the endpoints that reject the field outright.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct Thinking<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
}

/// One message, in whichever of the three shapes the endpoint expects. An enum
/// rather than one struct of optional fields because the shapes disagree about
/// what "absent" means: a tool result omitting `tool_call_id` is malformed and
/// must send `null`, while a plain message must not carry the key at all.
#[derive(Serialize)]
#[serde(untagged)]
enum WireMessage<'a> {
    /// `Cow` because the system message is built here, while every transcript
    /// content is borrowed from the transcript.
    Text {
        role: &'static str,
        content: Cow<'a, str>,
    },
    /// Providers require `content` beside tool calls even when it is empty.
    ///
    /// The two reasoning fields are the only place thinking is ever echoed
    /// back, and at most one of them is ever set — see [`wire_reasoning`].
    Calls {
        role: &'static str,
        content: &'a str,
        tool_calls: Vec<WireToolCall<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<&'a str>,
    },
    ToolResult {
        role: &'static str,
        tool_call_id: Option<&'a str>,
        content: &'a str,
    },
}

#[derive(Serialize)]
struct WireToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunction<'a>,
}

#[derive(Serialize)]
struct WireFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

impl<'a> WireMessage<'a> {
    fn text(role: &'static str, content: impl Into<Cow<'a, str>>) -> Self {
        WireMessage::Text {
            role,
            content: content.into(),
        }
    }
}

#[cfg(test)]
pub fn request_body_with_project(
    cfg: &Config,
    transcript: &[Message],
    project: Option<(&str, &str)>,
    plan: bool,
) -> Vec<u8> {
    request_body_with_project_and_tools(cfg, transcript, project, plan, &[])
}

/// The body alone, for the tests that do not care what the tools cost.
#[cfg(test)]
pub fn request_body_with_project_and_tools(
    cfg: &Config,
    transcript: &[Message],
    project: Option<(&str, &str)>,
    plan: bool,
    extension_tools: &[Value],
) -> Vec<u8> {
    request_body_with_stats(cfg, transcript, project, plan, extension_tools).0
}

/// The outgoing body, and what the tool array in it cost.
///
/// The stats come back from here rather than from a second pass, because working
/// them out means building and serializing the whole array — the one thing this
/// function has already done. `specs()` re-materializes 82 JSON trees per call,
/// so a log line is not worth a second one.
pub fn request_body_with_stats(
    cfg: &Config,
    transcript: &[Message],
    project: Option<(&str, &str)>,
    plan: bool,
    extension_tools: &[Value],
) -> (Vec<u8>, ToolStats) {
    let system = system_text(cfg, project, plan);
    // Which families this conversation has reached for. Derived from the transcript
    // rather than tracked, so it costs no state and survives a reopened chat; see
    // `tools::loaded_families`.
    let loaded = tools::loaded_families(transcript.iter().flat_map(|m| m.tool_calls.iter()));
    let tools = tool_specs(cfg, plan, extension_tools, &loaded).map(|tools| match cfg.api {
        Api::Chat => tools,
        Api::Responses => flatten_tools(tools),
    });
    let mut stats = ToolStats {
        count: tools.as_ref().and_then(Value::as_array).map_or(0, Vec::len),
        bytes: tools
            .as_ref()
            .and_then(|tools| serde_json::to_vec(tools).ok())
            .map_or(0, |bytes| bytes.len()),
        ..ToolStats::default()
    };
    // What the transcript no longer needs to send. Built once and handed to whichever transport
    // is speaking; see `Elision`.
    let elision = Elision::plan(transcript);
    let body = match cfg.api {
        Api::Chat => chat_request_body(cfg, transcript, system, tools, &elision),
        Api::Responses => responses_request_body(cfg, transcript, system, tools, &elision),
    };
    for (i, message) in transcript.iter().enumerate() {
        match message.role {
            Role::Tool => {
                let sent = elision.content(i).unwrap_or(&message.content).len();
                stats.original_payload_bytes += message.content.len();
                stats.transmitted_payload_bytes += sent;
                stats.result_bytes += sent;
                stats.max_result_bytes = stats.max_result_bytes.max(sent);
            },
            Role::Assistant => {
                for (at, call) in message.tool_calls.iter().enumerate() {
                    stats.original_payload_bytes += call.arguments.len();
                    stats.transmitted_payload_bytes +=
                        wire_arguments(call, elision.arguments(i, at)).len();
                }
            },
            _ => {},
        }
    }
    stats.omitted_results = elision.results.len();
    (body, stats)
}

/// Tool-array and selected payload sizes for request logging. Payload bytes exclude their
/// enclosing wire JSON; the complete serialized request is measured separately.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ToolStats {
    pub count: usize,
    pub bytes: usize,
    pub original_payload_bytes: usize,
    pub transmitted_payload_bytes: usize,
    pub omitted_results: usize,
    pub result_bytes: usize,
    pub max_result_bytes: usize,
}

/// The system prompt as it goes out, whichever transport carries it: the
/// tool-first preamble, the user's own prompt, the planning-mode sentence and the
/// active project, in that order.
///
/// The order matters. A custom `system_prompt` sits after the tool preamble but
/// before the planning sentence, so it can never displace the explanation of why
/// half the tools are missing.
fn system_text(cfg: &Config, project: Option<(&str, &str)>, plan: bool) -> Option<String> {
    let mut system = if cfg.tools {
        Some(match &cfg.system_prompt {
            Some(custom) => format!("{TOOL_FIRST_PROMPT}\n\n{custom}"),
            None => TOOL_FIRST_PROMPT.to_owned(),
        })
    } else {
        cfg.system_prompt.clone()
    };
    if plan {
        match system.as_mut() {
            Some(system) => {
                system.push_str("\n\n");
                system.push_str(PLAN_PROMPT);
            },
            None => system = Some(PLAN_PROMPT.to_owned()),
        }
    }
    if let Some((name, root)) = project {
        let context = format!("Active project: {name}. Project root: {root}.");
        match system.as_mut() {
            Some(system) => {
                system.push_str("\n\n");
                system.push_str(&context);
            },
            None => system = Some(context),
        }
    }
    system
}

/// The advertised tool set: the builtins this mode and this conversation have
/// earned, plus whatever extensions contribute. `None` when `tools` is off, which
/// keeps the key off the wire for the local models that reject the array outright.
///
/// Two filters, and they compose as an intersection: planning mode withholds
/// everything that acts, and family disclosure withholds everything this
/// conversation has not reached for. A tool has to survive both.
fn tool_specs(
    cfg: &Config,
    plan: bool,
    extension_tools: &[Value],
    loaded: &BTreeSet<&str>,
) -> Option<Value> {
    cfg.tools.then(|| {
        let mut specs = if plan {
            tools::read_only_specs_for(loaded)
        } else {
            tools::specs_for(loaded)
        };
        if let Value::Array(specs) = &mut specs {
            specs.extend_from_slice(extension_tools);
        }
        specs
    })
}

fn chat_request_body(
    cfg: &Config,
    transcript: &[Message],
    system: Option<String>,
    tools: Option<Value>,
    elision: &Elision,
) -> Vec<u8> {
    let mut messages: Vec<WireMessage> = Vec::with_capacity(transcript.len() + 1);
    if let Some(system) = system {
        messages.push(WireMessage::text("system", system));
    }
    for (index, message) in transcript.iter().enumerate() {
        match message.role {
            Role::User => messages.push(WireMessage::text("user", &message.content)),
            // Preserve the assistant response as one message: DeepSeek requires
            // its content, reasoning and tool calls to be handed back together.
            Role::Assistant if !message.tool_calls.is_empty() => {
                let (reasoning_content, reasoning) =
                    wire_reasoning(message.reasoning.as_ref(), cfg.reasoning_echo);
                messages.push(WireMessage::Calls {
                    role: "assistant",
                    // Old archived calls stored their display summary in
                    // `content`; new calls always carry a display override.
                    content: message
                        .display
                        .as_ref()
                        .map_or("", |_| message.content.as_str()),
                    tool_calls: message
                        .tool_calls
                        .iter()
                        .enumerate()
                        .map(|(at, call)| wire_tool_call(call, elision.arguments(index, at)))
                        .collect(),
                    reasoning_content,
                    reasoning,
                })
            },
            // An empty assistant message is the streaming placeholder, which has
            // nothing to say yet.
            Role::Assistant if !message.content.is_empty() => {
                messages.push(WireMessage::text("assistant", &message.content))
            },
            // Every tool call must be answered or the next request is malformed,
            // so this arm has no content-based skip.
            Role::Tool => messages.push(WireMessage::ToolResult {
                role: "tool",
                tool_call_id: message.tool_call_id.as_deref(),
                content: elision.content(index).unwrap_or(&message.content),
            }),
            // Error and Notice lines are our own UI, not conversation - sending
            // them back would have the model apologise for our HTTP failures.
            _ => continue,
        }
    }

    let body = ChatRequest {
        model: &cfg.model,
        thinking: cfg.thinking.as_deref().map(|kind| Thinking { kind }),
        reasoning_effort: cfg.reasoning_effort.as_deref(),
        stream: true,
        stream_options: cfg.stream_usage.then_some(StreamOptions {
            include_usage: true,
        }),
        messages,
        tools,
    };

    serde_json::to_vec(&body).expect("serializing a chat request cannot fail")
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// The Responses request, borrowed for the same reason [`ChatRequest`] is.
///
/// The differences from chat/completions are all here: the system prompt is
/// `instructions` rather than a message, the transcript is `input` rather than
/// `messages`, the effort is an object rather than a bare string, and there is no
/// `stream_options` — usage arrives on the terminal `response.completed` event
/// whether or not it was asked for.
#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    stream: bool,
    /// Nothing is kept server-side. hocus sends the whole transcript every turn
    /// and never uses `previous_response_id`, so a stored copy would be a
    /// 30-day retention of the user's conversation bought for nothing.
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig<'a>>,
    /// Asks for each reasoning item's chain-of-thought back sealed, so it can be
    /// echoed on the next round of a tool loop. Only meaningful with `store: false`,
    /// which is unconditional here, and only sent when a `reasoning` config is - a
    /// model that produces no reasoning items has nothing to include.
    #[serde(skip_serializing_if = "Option::is_none")]
    include: Option<[&'static str; 1]>,
    input: Vec<InputItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Value>,
}

#[derive(Serialize)]
struct ReasoningConfig<'a> {
    effort: &'a str,
    /// Asked for unconditionally: without it a reasoning model streams no
    /// thinking at all and hocus has nothing to show for the seconds it spends.
    /// The raw chain-of-thought is never on offer here — only this summary.
    summary: &'static str,
}

/// One `input` item. The three shapes [`WireMessage`] has, spelled the way this
/// endpoint spells them — and, unlike there, a tool call is its own item rather
/// than a field on the assistant message it came with.
#[derive(Serialize)]
#[serde(untagged)]
enum InputItem<'a> {
    Message {
        #[serde(rename = "type")]
        kind: &'static str,
        role: &'static str,
        content: Cow<'a, str>,
    },
    FunctionCall {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: &'a str,
        name: &'a str,
        arguments: &'a str,
    },
    FunctionCallOutput {
        #[serde(rename = "type")]
        kind: &'static str,
        call_id: &'a str,
        output: &'a str,
    },
    /// A reasoning item handed back exactly as it arrived. `summary` is sent empty:
    /// the visible summary is ours to display, and the endpoint reads only the
    /// sealed content.
    Reasoning {
        #[serde(rename = "type")]
        kind: &'static str,
        id: &'a str,
        encrypted_content: &'a str,
        summary: [(); 0],
    },
}

impl<'a> InputItem<'a> {
    fn message(role: &'static str, content: impl Into<Cow<'a, str>>) -> Self {
        InputItem::Message {
            kind: "message",
            role,
            content: content.into(),
        }
    }

    fn reasoning(item: &'a ReasoningItem) -> Self {
        InputItem::Reasoning {
            kind: "reasoning",
            id: &item.id,
            encrypted_content: &item.encrypted_content,
            summary: [],
        }
    }
}

/// Build the Responses request body.
///
/// The one rule that is not obvious: a reasoning item must be followed by the item it
/// produced, or the API rejects the whole request. `ReasoningItem::before` records how
/// many prose/call output items preceded each one in the stream, and
/// `Message::prose_before` puts the prose back among the calls. Replaying both positions
/// reproduces the order the model emitted. `reasoning_echo` gates the sealed items, the
/// same switch the other transport uses.
fn responses_request_body(
    cfg: &Config,
    transcript: &[Message],
    system: Option<String>,
    tools: Option<Value>,
    elision: &Elision,
) -> Vec<u8> {
    let mut input: Vec<InputItem> = Vec::with_capacity(transcript.len());
    for (index, message) in transcript.iter().enumerate() {
        match message.role {
            Role::User => input.push(InputItem::message("user", &message.content)),
            // Unlike chat/completions, the prose and the calls are separate
            // items. Same rule for the prose either way: old archived calls kept
            // their display summary in `content`, so it goes out only when the
            // call carries a `display` override.
            Role::Assistant if !message.tool_calls.is_empty() => {
                let prose = message
                    .display
                    .as_ref()
                    .map_or("", |_| message.content.as_str());
                let reasoning_items: &[ReasoningItem] = if cfg.reasoning_echo {
                    &message.reasoning_items
                } else {
                    &[]
                };
                let at = |position: usize| {
                    reasoning_items
                        .iter()
                        .filter(move |item| item.before == position)
                        .map(InputItem::reasoning)
                };
                let prose_before = (!prose.is_empty()).then(|| {
                    message
                        .prose_before
                        .unwrap_or(0)
                        .min(message.tool_calls.len())
                });
                let mut calls = message.tool_calls.iter().enumerate();
                let output_items = message.tool_calls.len() + usize::from(prose_before.is_some());
                for position in 0..output_items {
                    input.extend(at(position));
                    if prose_before == Some(position) {
                        input.push(InputItem::message("assistant", prose));
                        continue;
                    }
                    // `position` counts output items, which includes the prose slot, so the
                    // call's own index has to come from the iterator rather than from it.
                    let (at, call) = calls.next().expect("one slot per call");
                    input.push(InputItem::FunctionCall {
                        kind: "function_call",
                        call_id: &call.id,
                        name: &call.name,
                        arguments: wire_arguments(call, elision.arguments(index, at)),
                    });
                }
                // Anything recorded after the last output item. Should not happen - a
                // reasoning item is always followed by the item it produced - but
                // dropping one silently would break the pairing rather than
                // preserve it.
                input.extend(
                    reasoning_items
                        .iter()
                        .filter(|item| item.before >= output_items)
                        .map(InputItem::reasoning),
                );
            },
            Role::Assistant if !message.content.is_empty() => {
                input.push(InputItem::message("assistant", &message.content))
            },
            Role::Tool => input.push(InputItem::FunctionCallOutput {
                kind: "function_call_output",
                // The call this answers, by the `call_id` the stream reported.
                // Empty is malformed, but so is dropping the item: leaving a
                // call unanswered fails the next request either way, and this
                // one at least says which tool it was.
                call_id: message.tool_call_id.as_deref().unwrap_or_default(),
                output: elision.content(index).unwrap_or(&message.content),
            }),
            // Error and Notice lines are our own UI, as on the other transport.
            _ => continue,
        }
    }

    let body = ResponsesRequest {
        model: &cfg.model,
        stream: true,
        store: false,
        instructions: system,
        reasoning: cfg
            .reasoning_effort
            .as_deref()
            .map(|effort| ReasoningConfig {
                effort,
                summary: "auto",
            }),
        include: (cfg.reasoning_effort.is_some() && cfg.reasoning_echo)
            .then_some(["reasoning.encrypted_content"]),
        input,
        tools,
    };

    serde_json::to_vec(&body).expect("serializing a responses request cannot fail")
}

/// Rewrite chat/completions tool specs into the flat shape this endpoint wants.
///
/// `tools::specs()` emits `{type, function: {name, description, parameters}}`;
/// here those three fields sit at the top level. Rewritten rather than generated
/// twice, so the tool set has one definition and extension tools
/// (`Registry::spec`, which copies the same shape) come along for free.
///
/// `strict` is off deliberately: our schemas have optional parameters and no
/// `additionalProperties: false`, both of which structured mode rejects.
fn flatten_tools(specs: Value) -> Value {
    let Value::Array(specs) = specs else {
        return specs;
    };
    Value::Array(
        specs
            .into_iter()
            .map(|mut spec| match spec.get_mut("function").map(Value::take) {
                Some(Value::Object(mut function)) => json!({
                    "type": "function",
                    "name": function.remove("name").unwrap_or(Value::Null),
                    "description": function.remove("description").unwrap_or(Value::Null),
                    "parameters": function.remove("parameters").unwrap_or(Value::Null),
                    "strict": false,
                }),
                // Not a shape we produce. Pass it through rather than mangle it:
                // an endpoint's own error names the offending tool, we would not.
                _ => spec,
            })
            .collect(),
    )
}

fn enum_setting(
    kdl: &BTreeMap<String, String>,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, String> {
    let Some(value) = non_empty(kdl, key) else {
        return Ok(None);
    };
    let value = value.to_ascii_lowercase();
    if allowed.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        Err(format!("{key} must be one of: {}", allowed.join(", ")))
    }
}

// ---------------------------------------------------------------------------
// Elision
// ---------------------------------------------------------------------------

/// Outbound-only selection: deduplicate, age completed source arguments, then retain at most
/// 64 KiB of result bodies newest first (8 KiB each, notices included). Original messages and
/// archives remain intact; references recover their exact historical payloads. Call IDs,
/// result pairing, instructions, prose and provider reasoning never change.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Elision {
    /// `(message index, index within that message's `tool_calls`)` -> the arguments to send.
    calls: BTreeMap<(usize, usize), String>,
    /// Index of a `Role::Tool` message -> the content to send in place of its result.
    results: BTreeMap<usize, String>,
}

/// What one plugin call was about, as far as elision cares.
///
/// `plugin_docs` is keyed by section and everything else by plugin name, because those are the
/// things a later call can supersede. A call whose arguments will not parse, or which names no
/// target, gets no `Subject` and is left alone.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Subject {
    /// A plugin file, by its canonical filename, so `keys` and `keys.ts` share a subject.
    File(String),
    /// One page of a plugin file: the name, and the 1-based inclusive line range the read
    /// actually answered with.
    ///
    /// `plugin_read` pages, so the file alone is not what a read is *about*. Keyed by name
    /// only, walking a 900-line plugin in three reads elided the first two as "out of date"
    /// though nothing had changed, and sent the model back to page 1 to recover.
    Page(String, usize, usize),
    /// One `plugin_docs` section, by the name the model asked for.
    Section(String),
    /// The plugin folder as a whole: what `plugin_list` and a `plugin_search` with no name are
    /// about, and what every write or edit is a later statement about.
    Folder,
    /// A `plugin_search`: the plugin it was confined to (`None` for the folder), the pattern,
    /// and whether case was ignored. `context` and `max_results` are left out on purpose - the
    /// same words looked for again are the same search, and the later answer is the one to keep.
    Search(Option<String>, String, bool),
    /// A `plugin_outline` of one plugin.
    Outline(String),
    /// A `plugin_docs query=`: the section it was confined to, and the pattern.
    DocsSearch(Option<String>, String, bool),
    /// One page of one docs section. Its own variant rather than `Page`: a plugin can be called
    /// `data.ts` and a section `data`.
    SectionPage(String, usize, usize),
}

impl Elision {
    /// Work out what this transcript no longer needs to send.
    pub(crate) fn plan(transcript: &[Message]) -> Self {
        let mut elision = Self::deduplicate(transcript);
        let completed: BTreeSet<&str> = transcript
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();
        let mut batches = 0;
        for (i, m) in transcript.iter().enumerate().rev() {
            if m.role != Role::Assistant || m.tool_calls.is_empty() {
                continue;
            }
            batches += 1;
            for (at, call) in m.tool_calls.iter().enumerate() {
                if !completed.contains(call.id.as_str()) {
                    elision.calls.remove(&(i, at));
                    continue;
                }
                if matches!(
                    call.name.as_str(),
                    "plugin_write" | "plugin_edit" | "plugin_check"
                ) && (batches > 2 || elision.calls.contains_key(&(i, at)))
                {
                    let note = retrieval_reference(&call.id, "arguments");
                    if let Ok(mut args) = serde_json::from_str::<Value>(&call.arguments) {
                        mask_arguments(&mut args, &note);
                        let replacement = args.to_string();
                        elision.calls.remove(&(i, at));
                        if replacement.len() < call.arguments.len() {
                            elision.calls.insert((i, at), replacement);
                        }
                    }
                }
            }
        }
        // Reserve space for every older reference before spending the rest newest first.
        let results: Vec<_> = transcript
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::Tool)
            .map(|(i, m)| {
                let note = retrieval_reference(m.tool_call_id.as_deref().unwrap_or(""), "result");
                let content = if elision.results.contains_key(&i) && note.len() < m.content.len() {
                    note.clone()
                } else {
                    m.content.clone()
                };
                let minimum = content.len().min(note.len());
                (i, content, note, minimum)
            })
            .collect();
        let mut remaining = TOOL_RESULTS_BYTES.saturating_sub(results.iter().map(|r| r.3).sum());
        for (i, content, note, minimum) in results.into_iter().rev() {
            let limit = TOOL_RESULT_BYTES.min(minimum + remaining);
            let selected = bounded_result(&content, &note, limit);
            remaining = remaining.saturating_sub(selected.len().saturating_sub(minimum));
            elision.results.remove(&i);
            if selected != transcript[i].content {
                elision.results.insert(i, selected);
            }
        }
        elision
    }

    fn deduplicate(transcript: &[Message]) -> Self {
        let mut elision = Elision::default();
        // Backwards, which is the whole trick: the first thing seen for a subject is the newest
        // one, so "keep the most recent" needs no special case - it is simply whatever got
        // there first. `seen` is what has already been kept.
        let mut seen: BTreeSet<(&'static str, Subject)> = BTreeSet::new();
        // Which call each result answers, so a result can be keyed by the call that produced it
        // rather than by guessing from its text.
        let subjects = call_subjects(transcript);

        for (index, message) in transcript.iter().enumerate().rev() {
            match message.role {
                Role::Tool => {
                    let Some(id) = message.tool_call_id.as_deref() else {
                        continue;
                    };
                    let Some((name, subject)) = subjects.get(id) else {
                        continue;
                    };
                    if let Some(replacement) =
                        stale_result(name, subject, &message.content, &mut seen)
                    {
                        elision.results.insert(index, replacement);
                    }
                },
                Role::Assistant => {
                    // Backwards within the turn as well as across turns. A model that edits
                    // the same file twice in one batch sends both calls in one message, and
                    // walking them forwards would call the *first* one the newest - keeping
                    // the version that was superseded and eliding the one that stands.
                    for (at, call) in message.tool_calls.iter().enumerate().rev() {
                        let Some(subject) = subject_of(&call.name, &call.arguments) else {
                            continue;
                        };
                        if let Some(replacement) =
                            stale_arguments(&call.name, &subject, &call.arguments, &mut seen)
                        {
                            elision.calls.insert((index, at), replacement);
                        }
                        note_write(&call.name, &subject, &mut seen);
                    }
                },
                _ => {},
            }
        }
        elision
    }

    fn arguments(&self, message: usize, call: usize) -> Option<&str> {
        self.calls.get(&(message, call)).map(String::as_str)
    }

    fn content(&self, message: usize) -> Option<&str> {
        self.results.get(&message).map(String::as_str)
    }
}

fn retrieval_reference(id: &str, part: &str) -> String {
    format!(
        "[omitted: tool_result_read tool_call_id={} part={part} offset=0]",
        json!(id)
    )
}

fn mask_arguments(args: &mut Value, note: &str) {
    for key in ["source", "old_string", "new_string"] {
        if let Some(Value::String(s)) = args.get_mut(key) {
            if s.len() > note.len() {
                *s = note.to_owned();
            }
        }
    }
    if let Some(edits) = args.get_mut("edits").and_then(Value::as_array_mut) {
        for edit in edits {
            mask_arguments(edit, note);
        }
    }
}

fn bounded_result(content: &str, note: &str, limit: usize) -> String {
    if content.len() <= limit {
        return content.to_owned();
    }
    if note.len() >= content.len() {
        return content.to_owned();
    }
    let mut end = limit.saturating_sub(note.len() + 1).min(content.len());
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        note.to_owned()
    } else {
        format!("{}\n{note}", &content[..end])
    }
}

/// Read the original bytes through character offsets, including JSON escaping in the page cap.
pub(crate) fn tool_result_read(
    transcript: &[Message],
    args: &tools::Args,
) -> Result<String, String> {
    let id = args
        .tool_call_id
        .as_deref()
        .ok_or("tool_call_id is required")?;
    let call = transcript
        .iter()
        .flat_map(|m| &m.tool_calls)
        .find(|c| c.id == id)
        .ok_or("unknown tool_call_id in this chat")?;
    let text = match args.part.as_deref() {
        Some("arguments") => &call.arguments,
        Some("result") => {
            &transcript
                .iter()
                .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(id))
                .ok_or("tool result is not available")?
                .content
        },
        _ => return Err("part must be result or arguments".into()),
    };
    let offset = args.offset.unwrap_or(0);
    if offset > text.chars().count() {
        return Err("offset exceeds character count".into());
    }
    let mut page = String::new();
    // Reserve metadata space. Each character costs its JSON-escaped UTF-8 representation.
    let mut bytes = 128;
    let mut count = 0;
    for c in text.chars().skip(offset).take(4000) {
        let cost = json!(c.to_string()).to_string().len() - 2;
        if bytes + cost > TOOL_RESULT_BYTES {
            break;
        }
        page.push(c);
        bytes += cost;
        count += 1;
    }
    let next = (offset + count < text.chars().count()).then_some(offset + count);
    Ok(json!({"text": page, "next_offset": next}).to_string())
}

/// Every plugin call in the transcript, keyed by its id, as `(tool name, subject)`.
fn call_subjects(transcript: &[Message]) -> BTreeMap<&str, (&str, Subject)> {
    transcript
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .filter_map(|call| {
            let subject = subject_of(&call.name, &call.arguments)?;
            Some((call.id.as_str(), (call.name.as_str(), subject)))
        })
        .collect()
}

/// What a call is about, or `None` for a call elision has no rule for.
fn subject_of(name: &str, arguments: &str) -> Option<Subject> {
    let args = tools::Args::parse(arguments).ok()?;
    match name {
        "plugin_docs" => {
            let section = args
                .section
                .as_deref()
                .map(str::trim)
                .filter(|section| !section.is_empty())
                .map(str::to_owned);
            if let Some(query) = trimmed(args.query.as_deref()) {
                return Some(Subject::DocsSearch(
                    section,
                    query.to_owned(),
                    args.case_insensitive,
                ));
            }
            // A page of a section, as `plugin_docs` itself resolves the range. Without a section
            // there is no page, and the refusal it gets is nothing to elide.
            if args.start_line.is_some() || args.line_count.is_some() {
                let (start, end) = tools::read_range(&args);
                return section.map(|section| Subject::SectionPage(section, start, end));
            }
            Some(Subject::Section(
                section.unwrap_or_else(|| "index".to_owned()),
            ))
        },
        "plugin_search" => {
            let query = trimmed(args.query.as_deref())?;
            Some(Subject::Search(
                plugin_file(args.name.as_deref()),
                query.to_owned(),
                args.case_insensitive,
            ))
        },
        "plugin_outline" => plugin_file(args.name.as_deref()).map(Subject::Outline),
        "plugin_list" => Some(Subject::Folder),
        // The range as `plugin_read` itself resolves it, defaults and all, so two calls are
        // the same page when they answered with the same lines rather than when they were
        // spelled the same way.
        "plugin_read" => {
            let (start, end) = tools::read_range(&args);
            plugin_file(args.name.as_deref()).map(|file| Subject::Page(file, start, end))
        },
        "plugin_write" | "plugin_edit" | "plugin_check" | "plugin_reload" => {
            plugin_file(args.name.as_deref()).map(Subject::File)
        },
        _ => None,
    }
}

/// Canonical authoring filename, so an extensionless name and its `.ts` spelling share a subject.
fn plugin_file(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    (!name.is_empty()).then(|| crate::plugin_source::file_name(name))
}

fn plugin_display_name(file: &str) -> String {
    crate::plugin_source::file_name(file)
}

/// A query as the search tools take it: trimmed, and `None` when there is nothing left.
fn trimmed(query: Option<&str>) -> Option<&str> {
    query.map(str::trim).filter(|query| !query.is_empty())
}

/// The argument keys that carry a file's text. A call with none of them says nothing about
/// what the file contains and so supersedes nothing.
const PAYLOAD_KEYS: [&str; 4] = ["source", "old_string", "new_string", "edits"];

/// The arguments to send for a call whose payload a later one has replaced, or `None` to send
/// it as it stands.
///
/// The subject is entered under a *rule* rather than under the tool that made the call, because
/// what supersedes what crosses tools: a `plugin_edit` replaces the source a `plugin_write`
/// sent, and both replace what a `plugin_check` was checking.
fn stale_arguments(
    name: &str,
    subject: &Subject,
    arguments: &str,
    seen: &mut BTreeSet<(&'static str, Subject)>,
) -> Option<String> {
    let rule = match name {
        "plugin_write" | "plugin_edit" | "plugin_check" => "source",
        _ => return None,
    };
    let Subject::File(file) = subject else {
        return None;
    };
    let mut args: serde_json::Map<String, Value> = serde_json::from_str(arguments).ok()?;
    // Whether this call carries a payload at all, settled before the slot is claimed. A
    // `plugin_check` names a file and sends nothing; letting it take the slot would elide the
    // `plugin_write` behind it as "replaced by a later write or edit" that never happened, and
    // cost the model the only copy of the source it is working from.
    if !PAYLOAD_KEYS.iter().any(|key| args.contains_key(*key)) {
        return None;
    }
    if seen.insert((rule, Subject::File(file.clone()))) {
        // The newest change to this file. Whatever the model most recently sent is what it is
        // reasoning from, so it goes out whole.
        return None;
    }
    let display = plugin_display_name(file);
    let note = Value::String(format!(
        "[elided: an earlier version of {display}, replaced by a later write or edit in this \
         conversation. plugin_read name={file} for what is on disk now.]"
    ));
    for key in PAYLOAD_KEYS {
        if let Some(slot) = args.get_mut(key) {
            *slot = note.clone();
        }
    }
    let elided = serde_json::to_string(&args).ok()?;
    // A placeholder is ~150 characters. Replacing a two-line plugin with it would *grow* the
    // request, and cost the model the text as well - so the rewrite only happens when it
    // actually saves something. The bookkeeping above already ran, so the newest version is
    // still the newest either way.
    (elided.len() < arguments.len()).then_some(elided)
}

/// The content to send for a result a later call has made stale, or `None` to send it whole.
fn stale_result(
    name: &str,
    subject: &Subject,
    content: &str,
    seen: &mut BTreeSet<(&'static str, Subject)>,
) -> Option<String> {
    let (stale, note) = match (name, subject) {
        // A read is out of date for either of two reasons, and they are not the same thing to
        // say. Both lookups run: the page has to be recorded as seen whichever answer comes
        // back, or the read before it would be judged against nothing.
        ("plugin_read", Subject::Page(file, start, end)) => {
            // A write or an edit is a later statement about the whole file's contents, so it
            // invalidates every earlier read of it, page or not - which is why `note_write`
            // records it under this same "read" rule.
            let changed = seen.contains(&("read", Subject::File(file.clone())));
            let read_again = !seen.insert(("read", Subject::Page(file.clone(), *start, *end)));
            let count = end.saturating_sub(*start).saturating_add(1);
            let display = plugin_display_name(file);
            let note = if changed {
                format!(
                    "[elided: this read of {display} is out of date - the file was written or \
                     edited later in this conversation. plugin_read name={file} \
                     start_line={start} line_count={count} for what is there now.]"
                )
            } else {
                format!(
                    "[elided: lines {start}-{end} of {display} are read again later in this \
                     conversation.]"
                )
            };
            (changed || read_again, note)
        },
        // A pane screenshot of a plugin that has been reloaded since.
        ("plugin_reload", Subject::File(file)) => (
            !seen.insert(("reload", Subject::File(file.clone()))),
            format!(
                "[elided: superseded by a later plugin_reload of {}.]",
                plugin_display_name(file)
            ),
        ),
        // The same section fetched twice. Rare, but it is 4-13 KB when it happens.
        ("plugin_docs", Subject::Section(section)) => (
            !seen.insert(("docs", Subject::Section(section.clone()))),
            format!("[elided: plugin_docs section={section} appears again below.]"),
        ),
        ("plugin_docs", Subject::DocsSearch(..)) => (
            !seen.insert(("docs", subject.clone())),
            "[elided: the same plugin_docs query appears again below.]".to_owned(),
        ),
        ("plugin_docs", Subject::SectionPage(section, start, end)) => (
            !seen.insert(("docs", subject.clone())),
            format!(
                "[elided: lines {start}-{end} of plugin_docs section={section} appear again \
                 below.]"
            ),
        ),
        // A search is a picture of the plugins it looked through, so a later write or edit of
        // one of them - of any of them, for a search of the folder - makes it a picture of
        // something that is no longer there. The same words looked for again are the same
        // search, and the later answer is the one kept.
        ("plugin_search", Subject::Search(file, ..)) => {
            let changed = match file {
                Some(file) => seen.contains(&("read", Subject::File(file.clone()))),
                None => seen.contains(&("read", Subject::Folder)),
            };
            let repeat = !seen.insert(("search", subject.clone()));
            let note = match changed {
                true => {
                    "[elided: this plugin_search is out of date - a plugin it searched was \
                         written or edited later in this conversation. Call it again for what \
                         is there now.]"
                },
                false => "[elided: the same plugin_search appears again below.]",
            };
            (changed || repeat, note.to_owned())
        },
        ("plugin_outline", Subject::Outline(file)) => {
            let changed = seen.contains(&("read", Subject::File(file.clone())));
            let repeat = !seen.insert(("outline", subject.clone()));
            let display = plugin_display_name(file);
            (
                changed || repeat,
                format!(
                    "[elided: this outline of {display} is out of date or repeated below. \
                     plugin_outline name={file} for the current one.]"
                ),
            )
        },
        ("plugin_list", Subject::Folder) => {
            let changed = seen.contains(&("read", Subject::Folder));
            let repeat = !seen.insert(("list", Subject::Folder));
            (
                changed || repeat,
                "[elided: this plugin_list is out of date or repeated below. plugin_list for \
                 the folder as it is now.]"
                    .to_owned(),
            )
        },
        _ => return None,
    };
    // See `stale_arguments`: a placeholder that is longer than what it replaces is a loss
    // twice over.
    (stale && note.len() < content.len()).then_some(note)
}

/// Mark a write or an edit as the newest statement about a file, so earlier reads of it are
/// stale. Called from the same backwards walk, before any earlier read is looked at.
fn note_write(name: &str, subject: &Subject, seen: &mut BTreeSet<(&'static str, Subject)>) {
    if let (Subject::File(file), "plugin_write" | "plugin_edit") = (subject, name) {
        seen.insert(("read", Subject::File(file.clone())));
        // And about the folder: a listing or a folder-wide search from before this is stale.
        seen.insert(("read", Subject::Folder));
    }
}

/// Largest serialized pieces of the actual outbound body, including reasoning and metadata.
pub fn wire_breakdown(body: &[u8], top: usize) -> Vec<(String, usize)> {
    let Ok(Value::Object(body)) = serde_json::from_slice(body) else {
        return Vec::new();
    };
    let messages = body
        .get("messages")
        .or_else(|| body.get("input"))
        .and_then(Value::as_array);
    let mut names = BTreeMap::new();
    for m in messages.into_iter().flatten() {
        if m["type"] == "function_call" {
            names.insert(
                m["call_id"].as_str().unwrap_or_default(),
                m["name"].as_str().unwrap_or("?"),
            );
        }
        for c in m["tool_calls"].as_array().into_iter().flatten() {
            names.insert(
                c["id"].as_str().unwrap_or_default(),
                c["function"]["name"].as_str().unwrap_or("?"),
            );
        }
    }
    let mut rows = Vec::new();
    for (key, value) in &body {
        if key != "messages" && key != "input" {
            rows.push((key.clone(), value.to_string().len()));
            continue;
        }
        for m in value.as_array().into_iter().flatten() {
            let label = if m["role"] == "tool" || m["type"] == "function_call_output" {
                let id = m["tool_call_id"]
                    .as_str()
                    .or_else(|| m["call_id"].as_str())
                    .unwrap_or_default();
                format!("result:{}", names.get(id).unwrap_or(&"?"))
            } else if let Some(calls) = m["tool_calls"].as_array() {
                format!(
                    "call:{}",
                    calls
                        .iter()
                        .filter_map(|c| c["function"]["name"].as_str())
                        .collect::<Vec<_>>()
                        .join("+")
                )
            } else if m["type"] == "function_call" {
                format!("call:{}", m["name"].as_str().unwrap_or("?"))
            } else {
                m["role"]
                    .as_str()
                    .or_else(|| m["type"].as_str())
                    .unwrap_or("?")
                    .to_owned()
            };
            rows.push((label, m.to_string().len()));
        }
    }
    rows.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
    rows.truncate(top);
    rows
}

/// Which reasoning field, if either, this assistant turn goes out with, as
/// `(reasoning_content, reasoning)`.
///
/// Both `None` is the answer for every endpoint that does not think, because
/// nothing ever put a `Reasoning` on the message — which is what keeps the
/// request to OpenAI byte-identical to the one it got before any of this
/// existed. When there is one, it goes back under the same key it arrived
/// under, never translated into the other family's spelling.
///
/// Only reached for a turn that carries tool calls. DeepSeek's rule is scoped
/// to those: without them "the intermediate assistant's reasoning_content does
/// not need to participate in the context concatenation" and is ignored, so
/// sending it would be paying for tokens to be thrown away — and would put the
/// field in front of endpoints that have no business seeing it.
fn wire_reasoning(carried: Option<&Reasoning>, echo: bool) -> (Option<&str>, Option<&str>) {
    let reasoning = match carried
        .filter(|_| echo)
        .filter(|carried| !carried.text.is_empty())
    {
        Some(reasoning) => reasoning,
        None => return (None, None),
    };
    match reasoning.key {
        ReasoningKey::ReasoningContent => (Some(&reasoning.text), None),
        ReasoningKey::Reasoning => (None, Some(&reasoning.text)),
    }
}

/// The `arguments` a call goes out with.
///
/// They are echoed back verbatim, and providers 400 with "arguments must be
/// valid json" on anything that is not a JSON object. A no-argument call
/// commonly streams as `""`, which dispatch accepts (see `tools::Args::parse`)
/// but the next request would be rejected for.
fn wire_arguments<'a>(call: &'a ToolCall, elided: Option<&'a str>) -> &'a str {
    // The elided form is checked too. It is an object by construction - `stale_arguments` edits
    // a parsed map and re-serializes it - but the check is free, and this is the one invariant
    // both endpoints enforce on us.
    let arguments = elided.unwrap_or(&call.arguments);
    let valid =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(arguments).is_ok();
    if valid {
        arguments
    } else {
        "{}"
    }
}

fn wire_tool_call<'a>(call: &'a ToolCall, elided: Option<&'a str>) -> WireToolCall<'a> {
    WireToolCall {
        id: &call.id,
        kind: "function",
        function: WireFunction {
            name: &call.name,
            arguments: wire_arguments(call, elided),
        },
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// The ids from a `/v1/models` response, sorted and deduplicated.
///
/// Only `id` is read. The rest of an entry is provider-specific metadata the
/// picker has nowhere to put, and every provider disagrees about it.
pub fn model_ids(body: &[u8]) -> Vec<String> {
    let Ok(response) = serde_json::from_slice::<ModelsResponse>(body) else {
        // An empty list is the caller's "the endpoint listed nothing" path, and
        // unparseable is indistinguishable from empty as far as it can act.
        return Vec::new();
    };
    let mut ids: Vec<String> = response
        .data
        .into_iter()
        .map(|model| model.id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ModelEntry {
    id: String,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// A SearxNG JSON query, from whatever shape of base URL the user configured.
///
/// ```text
/// http://localhost:8888          -> http://localhost:8888/search?q=…&format=json
/// http://localhost:8888/         -> http://localhost:8888/search?q=…&format=json
/// http://localhost:8888/search   -> http://localhost:8888/search?q=…&format=json
/// ```
pub fn search_query_url(base: &str, query: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let base = base.strip_suffix("/search").unwrap_or(base);
    format!("{base}/search?q={}&format=json", percent_encode(query))
}

/// Percent-encode a query-string value. Nothing in the dependency set does this
/// and it is eight lines; keep the RFC 3986 unreserved set, escape the rest.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            },
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Render a SearxNG response as the few fields the model can use.
///
/// A results page is ~25 entries of ~20 fields each, and every one of them would
/// be paid for on every subsequent turn — the same reason `list_session` builds
/// its own summary rather than serialising `SessionInfo`.
pub fn search_results(body: &[u8], limit: usize) -> String {
    let Ok(response) = serde_json::from_slice::<SearchResponse>(body) else {
        // The one likely misconfiguration, and invisible from here: SearxNG
        // serves HTML and answers `format=json` with a 403 unless json is in
        // `search.formats`.
        return "could not parse the search response as JSON — check that `json` is in \
                `search.formats` in the SearxNG instance's settings.yml"
            .to_owned();
    };

    let mut blocks: Vec<String> = response
        .answers
        .iter()
        .map(answer_text)
        .filter(|answer| !answer.trim().is_empty())
        .collect();
    blocks.extend(response.results.iter().take(limit).map(|result| {
        let mut block = format!("{}\n{}", result.title.trim(), result.url.trim());
        if !result.content.trim().is_empty() {
            block.push('\n');
            block.push_str(result.content.trim());
        }
        block
    }));

    if blocks.is_empty() {
        return "no results".to_owned();
    }
    blocks.join("\n\n")
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    results: Vec<SearchResult>,
    /// Instant answers. A string in older SearxNG, an object in current ones.
    answers: Vec<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResult {
    title: String,
    url: String,
    content: String,
}

/// An instant answer is a bare string in older SearxNG and an object with an
/// `answer` field in current ones.
fn answer_text(answer: &serde_json::Value) -> String {
    match answer {
        serde_json::Value::String(text) => text.clone(),
        other => other["answer"].as_str().unwrap_or_default().to_owned(),
    }
}

// ---------------------------------------------------------------------------
// SSE framing
// ---------------------------------------------------------------------------

/// A line this long with no break means we are not reading SSE. Bail rather than
/// grow the buffer without bound.
const MAX_PENDING: usize = 1 << 20;

/// Reassembles SSE lines from arbitrary byte chunks.
///
/// The host hands us raw socket reads (8 KiB at a time), *not* SSE frames, so
/// `data: ...\n` boundaries split across chunks at arbitrary offsets. Framing is
/// line-based: a line is complete exactly when its `\n` has arrived, which is
/// the whole of the partial-frame problem.
///
/// This also makes UTF-8 handling free. A multi-byte character can only be split
/// mid-line, and a line is decoded only once complete, so `from_utf8_lossy` is
/// reached only for genuinely invalid bytes.
#[derive(Debug, Default)]
pub struct SseParser {
    pending: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SseItem {
    Delta(String),
    /// A piece of the model's chain-of-thought, with the field it arrived
    /// under. Only endpoints that think emit these at all.
    Reasoning {
        text: String,
        key: ReasoningKey,
    },
    /// A whole reasoning item, sealed. Unlike [`SseItem::Reasoning`], which is the
    /// summary we display a fragment at a time, this arrives complete on one event
    /// and is never shown - it exists only to be handed back. Responses only.
    ReasoningItem {
        id: String,
        encrypted_content: String,
    },
    /// The Responses output item that owns any streamed assistant prose. Its
    /// contents still arrive through [`SseItem::Delta`]; this marker records its
    /// position among calls for stateless replay.
    AssistantMessage,
    ApiError(String),
    /// A piece of a tool call. Never complete on its own - see
    /// [`ToolCallAccumulator`].
    ToolCallFragment {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    /// The provider's count of what the request cost, in tokens, and in
    /// nano-USD when the provider prices it too (OpenRouter's `usage.cost`).
    /// Normally sent once on the final chunk.
    Usage {
        prompt: Option<u64>,
        completion: u64,
        cost_nanos: Option<u64>,
    },
    /// The `[DONE]` sentinel. Informational only: the authoritative end of
    /// stream is the host's `WebRequestStreamChunk { done: true }`.
    Done,
}

impl SseParser {
    /// Feed raw bytes, get back whatever complete lines decoded to. An
    /// incomplete trailing line is retained for the next call.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseItem> {
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > MAX_PENDING {
            self.pending.clear();
            return vec![SseItem::ApiError(format!(
                "malformed stream: no line break in {MAX_PENDING} bytes"
            ))];
        }

        let mut items = Vec::new();
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line[..newline]);
            items.extend(parse_line(line.trim_end_matches('\r')));
        }
        items
    }
}

/// Reassembles streamed tool-call fragments into complete calls.
///
/// Providers split `function.arguments` across arbitrarily many deltas — often
/// a token at a time — and send `id` and `name` only on the first fragment of
/// each `index`. So a fragment is never usable on its own: the index is the
/// identity, and everything else accumulates onto it.
///
/// Order is preserved by first-appearance of each index rather than by index
/// value, which keeps the results in the order the model asked for even if a
/// provider numbers them oddly.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<(usize, ToolCall)>,
    /// The turn's reasoning items, each tagged with how many replayable output
    /// items had been seen when it arrived.
    reasoning_items: Vec<ReasoningItem>,
    /// How many calls preceded the assistant prose output item.
    prose_before: Option<usize>,
}

impl ToolCallAccumulator {
    pub fn feed(&mut self, item: &SseItem) {
        if let SseItem::ReasoningItem {
            id,
            encrypted_content,
        } = item
        {
            // A reasoning item completes before the call it produced is announced,
            // so the number of calls started so far *is* its position.
            self.reasoning_items.push(ReasoningItem {
                id: id.clone(),
                encrypted_content: encrypted_content.clone(),
                before: self.calls.len() + usize::from(self.prose_before.is_some()),
            });
            return;
        }
        if matches!(item, SseItem::AssistantMessage) {
            self.prose_before.get_or_insert(self.calls.len());
            return;
        }
        let SseItem::ToolCallFragment {
            index,
            id,
            name,
            arguments,
        } = item
        else {
            return;
        };

        let call = match self.calls.iter_mut().find(|(seen, _)| seen == index) {
            Some((_, call)) => call,
            None => {
                self.calls.push((*index, ToolCall::default()));
                &mut self.calls.last_mut().expect("just pushed").1
            },
        };

        // Assigned rather than appended: these arrive whole, on one fragment.
        if let Some(id) = id {
            call.id = id.clone();
        }
        if let Some(name) = name {
            call.name = name.clone();
        }
        if let Some(arguments) = arguments {
            call.arguments.push_str(arguments);
        }
    }

    /// Take the turn's stateless replay metadata, leaving that part of the
    /// accumulator empty.
    ///
    /// Separate from [`Self::take`] so the several places that discard a dead
    /// stream's half-accumulated calls go on saying `take()` and mean it - both
    /// halves are cleared there either way.
    pub fn take_replay(&mut self) -> (Vec<ReasoningItem>, Option<usize>) {
        (
            std::mem::take(&mut self.reasoning_items),
            self.prose_before.take(),
        )
    }

    /// Take the completed calls, leaving the accumulator empty.
    ///
    /// A call with no name never had a usable fragment and is dropped: it would
    /// only reach dispatch to be rejected as an unknown tool.
    pub fn take(&mut self) -> Vec<ToolCall> {
        self.reasoning_items.clear();
        self.prose_before = None;
        std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(_, call)| call)
            .filter(|call| !call.name.is_empty())
            .collect()
    }
}

/// One SSE line yields whatever its delta carried, in order.
///
/// Blank lines are frame separators and `:`-prefixed lines are keep-alive
/// comments; both are dropped by the `data:` prefix check, as are the `id:`,
/// `event:` and `retry:` fields. chat/completions uses none of them, and the
/// Responses API's `event:` line only repeats the `type` its payload already
/// carries — which is what lets both streams share this one framing.
///
/// ponytail: this is line-based rather than blank-line-frame-based. Multi-line
/// `data:` fields are legal SSE but nothing in this ecosystem emits them. If one
/// ever turns up, accumulate `data:` lines until a blank line instead.
fn parse_line(line: &str) -> Vec<SseItem> {
    let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
        return Vec::new();
    };
    // Only chat/completions sends this. The Responses stream simply ends after
    // `response.completed`; either way the authoritative end is the host's
    // `done: true`, so nothing downstream depends on the sentinel.
    if payload == "[DONE]" {
        return vec![SseItem::Done];
    }

    // Anything that isn't JSON is proxy noise; drop it rather than surfacing a
    // parse error to the user mid-conversation.
    match serde_json::from_str::<StreamPayload>(payload) {
        Ok(StreamPayload::Response(event)) => parse_response_event(event),
        Ok(StreamPayload::Chat(chunk)) => parse_chat_chunk(chunk),
        Err(_) => Vec::new(),
    }
}

/// Which shape a `data:` payload is in.
///
/// Untagged, tried in order: a Responses event is recognised by its `type`, whose
/// enum refuses anything that is not one, so a chat chunk — which has no `type`
/// at all — falls through to the second arm. The reverse could not work:
/// [`StreamChunk`]'s fields all default, so it matches every JSON object there is.
#[derive(Deserialize)]
#[serde(untagged)]
enum StreamPayload {
    Response(ResponseEvent),
    Chat(StreamChunk),
}

fn parse_chat_chunk(chunk: StreamChunk) -> Vec<SseItem> {
    if let Some(error) = chunk.error {
        return vec![SseItem::ApiError(error.message)];
    }
    let mut items = Vec::new();
    // Ahead of the delta: the usage chunk has no choices at all on OpenAI,
    // while Ollama hangs it on the last content chunk. Either way it is kept.
    // A `usage` with neither count — `{}`, or `total_tokens` alone — says
    // nothing worth recording as an exact number; an empty object is what some
    // proxies send in place of leaving the field out.
    if let Some(Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        cost,
    }) = chunk.usage
    {
        let cost_nanos = cost_nanos(cost);
        if prompt.is_some() || completion.is_some() || cost_nanos.is_some() {
            items.push(SseItem::Usage {
                prompt,
                completion: completion.unwrap_or(0),
                cost_nanos,
            });
        }
    }
    let Some(delta) = chunk.choices.into_iter().next().map(|choice| choice.delta) else {
        return items;
    };

    // A provider may put the final reasoning fragment, prose and several
    // parallel calls in one delta. Keep all of them; dropping any reasoning is
    // enough for DeepSeek to reject the next request.
    if let Some((text, key)) = reasoning_delta(&delta) {
        items.push(SseItem::Reasoning {
            text: text.to_owned(),
            key,
        });
    }
    for fragment in delta.tool_calls {
        let function = fragment.function.unwrap_or_default();
        items.push(SseItem::ToolCallFragment {
            index: fragment.index,
            id: fragment.id,
            name: function.name,
            arguments: function.arguments,
        });
    }
    if let Some(content) = delta.content.filter(|content| !content.is_empty()) {
        items.push(SseItem::Delta(content));
    }
    items
}

/// The thinking a delta carried, under whichever key its endpoint uses.
/// `reasoning_content` wins if a provider somehow sends both, being the
/// spelling of the endpoint this exists for.
fn reasoning_delta(delta: &Delta) -> Option<(&str, ReasoningKey)> {
    delta
        .reasoning_content
        .as_deref()
        .map(|text| (text, ReasoningKey::ReasoningContent))
        .or_else(|| {
            delta
                .reasoning
                .as_deref()
                .map(|text| (text, ReasoningKey::Reasoning))
        })
        .filter(|(text, _)| !text.is_empty())
}

#[derive(Deserialize)]
struct StreamChunk {
    /// Empty on the usage-only chunk `stream_options` asks for, hence the
    /// default.
    #[serde(default)]
    choices: Vec<Choice>,
    /// Some proxies report errors inside an otherwise-200 stream.
    #[serde(default)]
    error: Option<ApiError>,
    /// Only on the final chunk, and only when asked for.
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    /// What the request was charged, in USD. OpenRouter sends it on every
    /// response unasked; the providers that bill by token alone never do.
    #[serde(default, deserialize_with = "lenient_cost")]
    cost: Option<f64>,
}

/// A provider-reported USD cost as the integer nano-USD hocus accumulates, so
/// sums across a long chat stay exact. A negative cost says nothing believable.
fn cost_nanos(cost: Option<f64>) -> Option<u64> {
    cost.filter(|cost| cost.is_finite() && *cost >= 0.0)
        .map(|cost| (cost * 1e9).round() as u64)
}

/// A `cost` the provider spells as anything but a number - an object of
/// per-direction prices, a string - is no price hocus can add up, and must
/// not cost the chunk it rides on.
fn lenient_cost<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(Value::deserialize(d)?.as_f64())
}

#[derive(Deserialize)]
struct Choice {
    /// `{}` on the finish chunk, hence the default.
    #[serde(default)]
    delta: Delta,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek, vLLM, SGLang, Qwen.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// OpenRouter, Ollama. Two fields rather than a serde `alias`, because
    /// which one arrived is what decides the key we send back under.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    /// Which call this fragment belongs to. Absent on some providers when only
    /// one call is in flight, hence the default.
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Deserialize, Default)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

/// One event off the Responses stream, in the shape of whichever of its ~40
/// event types this is.
///
/// Only the handful that carry something [`SseItem`] can express are named; the
/// rest — every `.added`/`.done` bookend, the item-level lifecycle, the
/// `response.created` echo of the request — deserialize to `Other` and produce
/// nothing. Their fields are all optional here for the same reason: which ones
/// are present is a property of the type, and the type is what decides what to
/// read.
#[derive(Deserialize)]
struct ResponseEvent {
    #[serde(rename = "type")]
    kind: ResponseEventKind,
    /// Which output item this belongs to. The Responses API's answer to
    /// chat/completions' `index`, and used as such: it is what tells two
    /// parallel tool calls apart.
    #[serde(default)]
    output_index: usize,
    /// The text of a `*.delta` event, whatever it is a delta of.
    #[serde(default)]
    delta: Option<String>,
    /// The item a `response.output_item.added` announces.
    #[serde(default)]
    item: Option<OutputItem>,
    /// The whole response, on the terminal events.
    #[serde(default)]
    response: Option<ResponseBody>,
    /// A transport-level `error` event, which carries the error at the top level
    /// rather than inside a response.
    #[serde(default)]
    error: Option<ApiError>,
    /// The message of a `type: "error"` frame, which spells it as a bare top-level
    /// field beside `code` and `param` rather than wrapping it in an `error` object
    /// the way a non-2xx body does.
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize, PartialEq)]
enum ResponseEventKind {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta,
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta,
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded,
    /// The completed item. Only reasoning items are read from it - a function call
    /// is already assembled from `added` plus its argument deltas - and only
    /// because `encrypted_content` is not populated until the item is done.
    #[serde(rename = "response.output_item.done")]
    OutputItemDone,
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta,
    #[serde(rename = "response.completed")]
    Completed,
    #[serde(rename = "response.failed")]
    Failed,
    #[serde(rename = "response.incomplete")]
    Incomplete,
    #[serde(rename = "error")]
    Error,
    /// Every other event type. Present so an unrecognised `response.*` still
    /// parses as a Responses event rather than falling through to the
    /// chat/completions arm, which would read it as an empty chunk.
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(rename = "type", default)]
    kind: String,
    /// The id a `function_call_output` must quote to answer this call. Not the
    /// item's own `id`, which is a different identifier and is not accepted
    /// there.
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// A reasoning item's own id, which is what it must be echoed back under.
    #[serde(default)]
    id: Option<String>,
    /// The sealed chain-of-thought, present only because the request asked for it
    /// with `include: ["reasoning.encrypted_content"]`.
    #[serde(default)]
    encrypted_content: Option<String>,
}

#[derive(Deserialize)]
struct ResponseBody {
    #[serde(default)]
    usage: Option<ResponseUsage>,
    #[serde(default)]
    error: Option<ApiError>,
    /// Why a `response.incomplete` stopped early - `max_output_tokens`, a content
    /// filter. That event carries no `error` at all, so this is the only thing it
    /// has to say for itself.
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Deserialize)]
struct IncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

/// The Responses API's spelling of `prompt_tokens`/`completion_tokens`. Reported
/// on the terminal event without being asked for, so there is no `stream_options`
/// equivalent to switch off.
#[derive(Deserialize)]
struct ResponseUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    /// As on chat/completions: only a provider that prices the request sends it.
    #[serde(default, deserialize_with = "lenient_cost")]
    cost: Option<f64>,
}

fn response_usage(response: Option<&ResponseBody>) -> Option<SseItem> {
    let usage = response?.usage.as_ref()?;
    let cost_nanos = cost_nanos(usage.cost);
    (usage.input_tokens.is_some() || usage.output_tokens.is_some() || cost_nanos.is_some())
        .then_some(SseItem::Usage {
            prompt: usage.input_tokens,
            completion: usage.output_tokens.unwrap_or(0),
            cost_nanos,
        })
}

fn parse_response_event(event: ResponseEvent) -> Vec<SseItem> {
    match event.kind {
        ResponseEventKind::OutputTextDelta => event
            .delta
            .filter(|text| !text.is_empty())
            .map(SseItem::Delta)
            .into_iter()
            .collect(),
        // Only the summary is ever on offer here — the raw chain-of-thought is
        // not streamed — so it goes out under the `reasoning` key, which is the
        // one for providers that summarise rather than think out loud.
        ResponseEventKind::ReasoningSummaryTextDelta => event
            .delta
            .filter(|text| !text.is_empty())
            .map(|text| SseItem::Reasoning {
                text,
                key: ReasoningKey::Reasoning,
            })
            .into_iter()
            .collect(),
        // The one event that names a call. Its arguments arrive afterwards, one
        // `.delta` at a time, keyed by the same `output_index`.
        ResponseEventKind::OutputItemAdded => match event.item {
            Some(item) if item.kind == "function_call" => vec![SseItem::ToolCallFragment {
                index: event.output_index,
                id: item.call_id,
                name: item.name,
                arguments: None,
            }],
            Some(item) if item.kind == "message" => vec![SseItem::AssistantMessage],
            _ => Vec::new(),
        },
        // The sealed thinking, which is worth nothing on its own and everything on
        // the next round of a tool loop. Both fields are required: an item missing
        // either cannot be handed back, and a half-formed one would break the
        // pairing rule rather than satisfy it.
        ResponseEventKind::OutputItemDone => match event.item {
            Some(OutputItem {
                kind,
                id: Some(id),
                encrypted_content: Some(encrypted_content),
                ..
            }) if kind == "reasoning" && !encrypted_content.is_empty() => {
                vec![SseItem::ReasoningItem {
                    id,
                    encrypted_content,
                }]
            },
            _ => Vec::new(),
        },
        ResponseEventKind::FunctionCallArgumentsDelta => vec![SseItem::ToolCallFragment {
            index: event.output_index,
            id: None,
            name: None,
            arguments: event.delta,
        }],
        ResponseEventKind::Completed => {
            let mut items: Vec<_> = response_usage(event.response.as_ref())
                .into_iter()
                .collect();
            items.push(SseItem::Done);
            items
        },
        // A failure mid-stream, and the three events spell it three different
        // ways: `response.failed` puts an error object on the response,
        // `response.incomplete` has no error at all and reports through
        // `incomplete_details.reason`, and a transport `error` frame carries a
        // bare `message` at the top level. Tried in that order, and never
        // allowed to come back empty - a terminal failure that produced no
        // `ApiError` would leave an empty or truncated answer looking like a
        // clean stop.
        ResponseEventKind::Failed | ResponseEventKind::Incomplete | ResponseEventKind::Error => {
            let usage = response_usage(event.response.as_ref());
            let detail = event
                .response
                .and_then(|response| {
                    let ResponseBody {
                        error,
                        incomplete_details,
                        ..
                    } = response;
                    error
                        .map(|error| error.message)
                        .or_else(|| incomplete_details.and_then(|details| details.reason))
                })
                .or_else(|| event.error.map(|error| error.message))
                .or(event.message)
                .filter(|detail| !detail.trim().is_empty())
                .unwrap_or_else(|| "the response ended without completing".to_owned());
            usage
                .into_iter()
                .chain(std::iter::once(SseItem::ApiError(detail)))
                .collect()
        },
        ResponseEventKind::Other => Vec::new(),
    }
}

// ---------------------------------------------------------------------------

#[path = "./unit/api_tests/mod.rs"]
#[cfg(test)]
mod api_tests;
