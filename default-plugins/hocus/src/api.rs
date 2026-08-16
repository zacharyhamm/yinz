//! Everything that talks to an OpenAI-compatible `/v1/chat/completions`
//! endpoint: config resolution, request construction, and SSE stream framing.
//!
//! Nothing in this module calls the Zellij host, so it is all unit-testable on
//! the host triple (see `make test`).

use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

use crate::tools::{self, ToolCall};
use crate::{Message, Role};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_API_KEY_ENV: &str = "OPENAI_API_KEY";
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
const TOOL_FIRST_PROMPT: &str = "You are Hocus in Zellij. Use native tools for Zellij operations. Before an ID-based action, call `list_session` unless the immediately previous result supplied the IDs; use `list_other_sessions` for cross-session IDs. Address tabs only by `tab_id` and panes by `pane_id`; `tab_number` is the tab bar's label, not an ID, so never pass it as one. Do not verify successful mutations or retry failed pane moves with variants. Use `send_input` for an existing program. You cannot run arbitrary commands. Inspect code only with `find_files`, `search_files`, and `read_file` inside the active project. Call `agent_list` before acting on agents. Make independent calls together.";
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

/// Look up a key, treating whitespace-only values as absent. An exported-but-
/// empty `OPENAI_API_KEY` should produce the config banner, not an
/// `Authorization: Bearer ` header that fails confusingly at the API.
fn non_empty(map: &BTreeMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Accept whatever shape of base URL the user has lying around.
///
/// ```text
/// https://api.openai.com/v1         -> https://api.openai.com/v1/chat/completions
/// https://api.openai.com/v1/        -> https://api.openai.com/v1/chat/completions
/// https://host/v1/chat/completions  -> unchanged
/// http://localhost:11434            -> http://localhost:11434/v1/chat/completions
/// ```
pub fn completions_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// The model list that sits beside the completions endpoint. Same four shapes of
/// base URL as `completions_url`, since it is the same `base_url` setting.
///
/// ```text
/// https://api.openai.com/v1         -> https://api.openai.com/v1/models
/// https://api.openai.com/v1/        -> https://api.openai.com/v1/models
/// https://host/v1/chat/completions  -> https://host/v1/models
/// http://localhost:11434            -> http://localhost:11434/v1/models
/// ```
pub fn models_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let base = base.strip_suffix("/chat/completions").unwrap_or(base);
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
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

pub fn request_body_with_project(
    cfg: &Config,
    transcript: &[Message],
    allow_tools: bool,
    project: Option<(&str, &str)>,
) -> Vec<u8> {
    let send_tools = cfg.tools && allow_tools;
    let mut messages: Vec<serde_json::Value> = Vec::with_capacity(transcript.len() + 1);
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
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in transcript {
        match message.role {
            Role::User => messages.push(json!({ "role": "user", "content": message.content })),
            // The tool call the model asked for. `content` holds our own
            // human-readable summary for the transcript, so it is deliberately
            // not sent - the tool_calls array is the real payload.
            Role::Assistant if !message.tool_calls.is_empty() => messages.push(json!({
                "role": "assistant",
                "content": serde_json::Value::Null,
                "tool_calls": message.tool_calls.iter().map(tool_call_json).collect::<Vec<_>>(),
            })),
            // An empty assistant message is the streaming placeholder, which has
            // nothing to say yet.
            Role::Assistant if !message.content.is_empty() => {
                messages.push(json!({ "role": "assistant", "content": message.content }))
            },
            // Every tool call must be answered or the next request is malformed,
            // so this arm has no content-based skip.
            Role::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id,
                "content": message.content,
            })),
            // Error and Notice lines are our own UI, not conversation - sending
            // them back would have the model apologise for our HTTP failures.
            _ => continue,
        }
    }

    let mut body = json!({
        "model": cfg.model,
        "stream": true,
        "messages": messages,
    });
    if send_tools {
        body["tools"] = tools::specs();
    }

    serde_json::to_vec(&body).expect("serializing a chat request cannot fail")
}

fn tool_call_json(call: &ToolCall) -> serde_json::Value {
    // `arguments` is echoed back verbatim, and providers 400 with "arguments
    // must be valid json" on anything that is not a JSON object. A no-argument
    // call commonly streams as `""`, which dispatch accepts (see
    // `tools::Args::parse`) but the next request would be rejected for.
    let valid =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&call.arguments).is_ok();
    json!({
        "id": call.id,
        "type": "function",
        "function": {
            "name": call.name,
            "arguments": if valid { call.arguments.as_str() } else { "{}" },
        },
    })
}

/// Best-effort human-readable message from a non-2xx response body.
pub fn api_error_message(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.trim().chars().take(500).collect());

    if detail.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
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
            if let Some(item) = parse_line(line.trim_end_matches('\r')) {
                items.push(item);
            }
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

/// One SSE line yields at most one item.
///
/// Blank lines are frame separators and `:`-prefixed lines are keep-alive
/// comments; both are dropped by the `data:` prefix check, as are the `id:`,
/// `event:` and `retry:` fields, which no chat/completions implementation uses.
///
/// ponytail: this is line-based rather than blank-line-frame-based. Multi-line
/// `data:` fields are legal SSE but nothing in this ecosystem emits them. If one
/// ever turns up, accumulate `data:` lines until a blank line instead.
fn parse_line(line: &str) -> Option<SseItem> {
    let payload = line.strip_prefix("data:")?.trim();
    if payload == "[DONE]" {
        return Some(SseItem::Done);
    }

    // Anything that isn't JSON is proxy noise; drop it rather than surfacing a
    // parse error to the user mid-conversation.
    let chunk: StreamChunk = serde_json::from_str(payload).ok()?;
    if let Some(error) = chunk.error {
        return Some(SseItem::ApiError(error.message));
    }
    let delta = chunk.choices.into_iter().next()?.delta;

    // A delta carries text or tool-call fragments, never both.
    if let Some(fragment) = delta.tool_calls.into_iter().next() {
        let function = fragment.function.unwrap_or_default();
        return Some(SseItem::ToolCallFragment {
            index: fragment.index,
            id: fragment.id,
            name: function.name,
            arguments: function.arguments,
        });
    }

    delta
        .content
        .filter(|content| !content.is_empty())
        .map(SseItem::Delta)
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
