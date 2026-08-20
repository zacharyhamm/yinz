//! Everything that talks to an OpenAI-compatible `/v1/chat/completions`
//! endpoint: config resolution, request construction, and SSE stream framing.
//!
//! Nothing in this module calls the Zellij host, so it is all unit-testable on
//! the host triple (see `make test`).

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::tools::{self, ToolCall};
use crate::{Message, Reasoning, ReasoningKey, Role};

// Endpoint shaping lives in hocus-proto so hocus-agentd, which calls the same endpoint to
// summarise an idle agent, cannot disagree with us about where it is. Re-exported rather
// than re-implemented: every call site here already spells them unqualified.
pub use hocus_proto::llm::{
    api_error_message, completions_url, models_url, non_empty, DEFAULT_API_KEY_ENV,
    DEFAULT_BASE_URL, DEFAULT_MODEL,
};
/// SearxNG's own docker-compose default port.
pub const DEFAULT_SEARCH_URL: &str = "http://localhost:8888";
/// Bare name, so an installed dobbs is found on PATH without configuration.
pub const DEFAULT_DOBBS: &str = "dobbs";
/// How many times in a row the model may call tools before the tools are taken
/// away and it has to answer from what it has. Without this a model that keeps
/// re-listing panes bills the user forever.
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 50;
/// How many search results the model gets. A SearxNG page is ~25, and every one
/// of them is paid for on every subsequent turn of the conversation.
pub const SEARCH_RESULTS: usize = 8;
const TOOL_FIRST_PROMPT: &str = "You are Hocus. Use native Zellij tools. Before ID actions, call `list_session` unless the prior result gave IDs; use `list_other_sessions` for cross-session IDs. `warp_tab`/`warp_pane` move current items out; `warp_tab_here`/`warp_pane_here` move listed items in. Use only `tab_id`/`pane_id`, never `tab_number`. Do not verify mutations or retry failed pane moves. `send_input` controls running programs. No arbitrary commands. Active-project code: `find_files`/`search_files`/`read_file`; history: `git_log`/`git_show`; GitHub metadata: GET-only `github_read`. Call `agent_list` before agent actions. Make independent calls together.";
/// Sent in place of the tool set on the last round of the tool budget, so the
/// model answers from what it has instead of insisting it needs another call.
const BUDGET_SPENT_PROMPT: &str =
    "The tool budget for this turn is spent. Answer now from the tool results you already have.";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Fully resolved completions endpoint, derived from `base_url`.
    pub url: String,
    pub api_key: String,
    pub model: String,
    /// Kept verbatim so the "not configured" banner can show what we resolved.
    pub base_url: String,
    pub api_key_env: String,
    /// Base URL of the SearxNG that backs `web_search`. Any instance on the
    /// network will do; it is a plain HTTP GET against its JSON API.
    pub search_url: String,
    /// The `dobbs` binary the Slack tools shell out to. dobbs speaks raw TCP to
    /// its daemon, which a wasm plugin cannot, so the CLI is the only way in.
    /// Defaults to `dobbs`, i.e. whatever is on PATH.
    pub dobbs: String,
    /// A remote dobbs daemon to dial instead of the local one, passed to the CLI
    /// as `$DOBBS_ADDR`/`$DOBBS_TOKEN`. Unset leaves dobbs on its own resolution
    /// order: `~/.dobbs/remote.addr`, then the local daemon's addr file.
    pub dobbs_addr: Option<String>,
    pub dobbs_token: Option<String>,
    pub system_prompt: Option<String>,
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
        let search_url = non_empty(kdl, "search_url")
            .or_else(|| non_empty(env, "SEARXNG_URL"))
            .unwrap_or_else(|| DEFAULT_SEARCH_URL.to_owned());
        let dobbs = expand_home(
            &non_empty(kdl, "dobbs_path")
                .or_else(|| non_empty(env, "DOBBS_BIN"))
                .unwrap_or_else(|| DEFAULT_DOBBS.to_owned()),
            env,
        );
        let dobbs_addr = non_empty(kdl, "dobbs_addr").or_else(|| non_empty(env, "DOBBS_ADDR"));
        let dobbs_token = non_empty(kdl, "dobbs_token").or_else(|| non_empty(env, "DOBBS_TOKEN"));
        let system_prompt = non_empty(kdl, "system_prompt");
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

        match non_empty(env, &api_key_env) {
            Some(api_key) => Ok(Config {
                url: completions_url(&base_url),
                api_key,
                model,
                base_url,
                api_key_env,
                search_url,
                dobbs,
                dobbs_addr,
                dobbs_token,
                system_prompt,
                tools,
                max_tool_rounds,
                reasoning_echo,
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
fn expand_home(path: &str, env: &BTreeMap<String, String>) -> String {
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

/// `allow_tools` is the caller's tool budget, not its configuration: with it
/// false the tool set is withheld even though `cfg.tools` is on, which leaves
/// the model no move but to reply in text.
#[cfg(test)]
pub fn request_body(cfg: &Config, transcript: &[Message], allow_tools: bool) -> Vec<u8> {
    request_body_with_project(cfg, transcript, allow_tools, None)
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
    stream: bool,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<serde_json::Value>,
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

pub fn request_body_with_project(
    cfg: &Config,
    transcript: &[Message],
    allow_tools: bool,
    project: Option<(&str, &str)>,
) -> Vec<u8> {
    let send_tools = cfg.tools && allow_tools;
    let mut messages: Vec<WireMessage> = Vec::with_capacity(transcript.len() + 1);
    let mut system = if cfg.tools {
        Some(match &cfg.system_prompt {
            Some(custom) => format!("{TOOL_FIRST_PROMPT}\n\n{custom}"),
            None => TOOL_FIRST_PROMPT.to_owned(),
        })
    } else {
        cfg.system_prompt.clone()
    };
    if cfg.tools && !allow_tools {
        // Unwrap-free: the branch above always produces a prompt when tools are on.
        if let Some(system) = system.as_mut() {
            system.push_str("\n\n");
            system.push_str(BUDGET_SPENT_PROMPT);
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
    if let Some(system) = system {
        messages.push(WireMessage::text("system", system));
    }
    for message in transcript {
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
                    tool_calls: message.tool_calls.iter().map(wire_tool_call).collect(),
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
                content: &message.content,
            }),
            // Error and Notice lines are our own UI, not conversation - sending
            // them back would have the model apologise for our HTTP failures.
            _ => continue,
        }
    }

    let body = ChatRequest {
        model: &cfg.model,
        stream: true,
        messages,
        tools: send_tools.then(tools::specs),
    };

    serde_json::to_vec(&body).expect("serializing a chat request cannot fail")
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

fn wire_tool_call(call: &ToolCall) -> WireToolCall<'_> {
    // `arguments` is echoed back verbatim, and providers 400 with "arguments
    // must be valid json" on anything that is not a JSON object. A no-argument
    // call commonly streams as `""`, which dispatch accepts (see
    // `tools::Args::parse`) but the next request would be rejected for.
    let valid =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&call.arguments).is_ok();
    WireToolCall {
        id: &call.id,
        kind: "function",
        function: WireFunction {
            name: &call.name,
            arguments: if valid { call.arguments.as_str() } else { "{}" },
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
    ApiError(String),
    /// A piece of a tool call. Never complete on its own - see
    /// [`ToolCallAccumulator`].
    ToolCallFragment {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
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
}

impl ToolCallAccumulator {
    pub fn feed(&mut self, item: &SseItem) {
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

    /// Take the completed calls, leaving the accumulator empty.
    ///
    /// A call with no name never had a usable fragment and is dropped: it would
    /// only reach dispatch to be rejected as an unknown tool.
    pub fn take(&mut self) -> Vec<ToolCall> {
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
/// `event:` and `retry:` fields, which no chat/completions implementation uses.
///
/// ponytail: this is line-based rather than blank-line-frame-based. Multi-line
/// `data:` fields are legal SSE but nothing in this ecosystem emits them. If one
/// ever turns up, accumulate `data:` lines until a blank line instead.
fn parse_line(line: &str) -> Vec<SseItem> {
    let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
        return Vec::new();
    };
    if payload == "[DONE]" {
        return vec![SseItem::Done];
    }

    // Anything that isn't JSON is proxy noise; drop it rather than surfacing a
    // parse error to the user mid-conversation.
    let Ok(chunk) = serde_json::from_str::<StreamChunk>(payload) else {
        return Vec::new();
    };
    if let Some(error) = chunk.error {
        return vec![SseItem::ApiError(error.message)];
    }
    let Some(delta) = chunk.choices.into_iter().next().map(|choice| choice.delta) else {
        return Vec::new();
    };

    // A provider may put the final reasoning fragment, prose and several
    // parallel calls in one delta. Keep all of them; dropping any reasoning is
    // enough for DeepSeek to reject the next request.
    let mut items = Vec::new();
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
    /// Empty on usage-only chunks, hence the default.
    #[serde(default)]
    choices: Vec<Choice>,
    /// Some proxies report errors inside an otherwise-200 stream.
    #[serde(default)]
    error: Option<ApiError>,
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

// ---------------------------------------------------------------------------

#[path = "./unit/api_tests/mod.rs"]
#[cfg(test)]
mod api_tests;
