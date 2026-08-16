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

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn test_config() -> Config {
        Config {
            url: "https://api.openai.com/v1/chat/completions".into(),
            api_key: "sk-test".into(),
            model: "gpt-4o-mini".into(),
            base_url: DEFAULT_BASE_URL.into(),
            api_key_env: DEFAULT_API_KEY_ENV.into(),
            search_url: DEFAULT_SEARCH_URL.into(),
            dobbs: DEFAULT_DOBBS.into(),
            dobbs_addr: None,
            dobbs_token: None,
            system_prompt: None,
            // Off in most fixtures so the assertions can compare whole bodies.
            tools: false,
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
        }
    }

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    // -- completions_url ----------------------------------------------------

    #[test]
    fn completions_url_appends_to_a_v1_base() {
        assert_eq!(
            completions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn completions_url_ignores_a_trailing_slash() {
        assert_eq!(
            completions_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn completions_url_leaves_a_full_endpoint_alone() {
        assert_eq!(
            completions_url("https://host/v1/chat/completions"),
            "https://host/v1/chat/completions"
        );
    }

    #[test]
    fn completions_url_adds_v1_to_a_bare_host() {
        assert_eq!(
            completions_url("http://localhost:11434"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    // -- models -------------------------------------------------------------

    /// Every shape `completions_url` accepts has to resolve here too — it is the
    /// same `base_url` setting, so a base that chats must also list.
    #[test]
    fn models_url_normalises_every_shape_of_base() {
        for base in [
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/",
            "https://api.openai.com/v1/chat/completions",
            "  https://api.openai.com/v1/chat/completions/  ",
        ] {
            assert_eq!(
                models_url(base),
                "https://api.openai.com/v1/models",
                "{base}"
            );
        }
    }

    #[test]
    fn models_url_adds_v1_to_a_bare_host() {
        assert_eq!(
            models_url("http://localhost:11434"),
            "http://localhost:11434/v1/models"
        );
    }

    #[test]
    fn model_ids_are_sorted_and_deduplicated() {
        let body = br#"{"object":"list","data":[
            {"id":"gpt-4o","object":"model","created":1},
            {"id":"o3-mini","owned_by":"openai"},
            {"id":"gpt-4o"}
        ]}"#;
        assert_eq!(model_ids(body), ["gpt-4o", "o3-mini"]);
    }

    /// A nameless entry is not something the picker can switch to, and an id is
    /// the one field every provider agrees on.
    #[test]
    fn model_ids_drops_entries_with_no_usable_id() {
        let body = br#"{"data":[{"id":"  "},{"object":"model"},{"id":" gpt-4o "}]}"#;
        assert_eq!(model_ids(body), ["gpt-4o"]);
    }

    /// An endpoint that answers /v1/models with HTML, or with a shape nobody
    /// predicted, must read as "nothing to show" rather than panic.
    #[test]
    fn model_ids_of_an_unparseable_body_is_empty() {
        assert!(model_ids(b"<html>404</html>").is_empty());
        assert!(model_ids(b"{}").is_empty());
    }

    // -- Config::resolve ----------------------------------------------------

    #[test]
    fn resolve_falls_back_to_defaults() {
        let cfg = Config::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "sk-1")])).unwrap();
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.api_key, "sk-1");
        assert_eq!(cfg.system_prompt, None);
    }

    #[test]
    fn resolve_prefers_env_over_defaults() {
        let cfg = Config::resolve(
            &map(&[]),
            &map(&[
                ("OPENAI_API_KEY", "sk-1"),
                ("OPENAI_BASE_URL", "http://localhost:11434"),
                ("OPENAI_MODEL", "llama3"),
            ]),
        )
        .unwrap();
        assert_eq!(cfg.base_url, "http://localhost:11434");
        assert_eq!(cfg.model, "llama3");
        assert_eq!(cfg.url, "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn resolve_prefers_kdl_over_env() {
        let cfg = Config::resolve(
            &map(&[("base_url", "https://kdl/v1"), ("model", "from-kdl")]),
            &map(&[
                ("OPENAI_API_KEY", "sk-1"),
                ("OPENAI_BASE_URL", "https://env/v1"),
                ("OPENAI_MODEL", "from-env"),
            ]),
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://kdl/v1");
        assert_eq!(cfg.model, "from-kdl");
    }

    #[test]
    fn resolve_redirects_the_key_lookup_via_api_key_env() {
        let cfg = Config::resolve(
            &map(&[("api_key_env", "MY_OTHER_KEY")]),
            &map(&[("OPENAI_API_KEY", "wrong"), ("MY_OTHER_KEY", "right")]),
        )
        .unwrap();
        assert_eq!(cfg.api_key, "right");
        assert_eq!(cfg.api_key_env, "MY_OTHER_KEY");
    }

    #[test]
    fn resolve_errors_when_the_key_is_missing_and_names_the_variable() {
        let err = Config::resolve(&map(&[]), &map(&[])).unwrap_err();
        assert!(
            err.message.contains("OPENAI_API_KEY"),
            "message should name the variable: {}",
            err.message
        );
        // The banner still needs these.
        assert_eq!(err.base_url, DEFAULT_BASE_URL);
        assert_eq!(err.model, DEFAULT_MODEL);
    }

    #[test]
    fn resolve_treats_an_empty_key_as_missing() {
        assert!(Config::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "   ")])).is_err());
    }

    #[test]
    fn resolve_trims_the_key() {
        let cfg = Config::resolve(&map(&[]), &map(&[("OPENAI_API_KEY", "  sk-1\n")])).unwrap();
        assert_eq!(cfg.api_key, "sk-1");
    }

    #[test]
    fn resolve_reads_the_system_prompt_from_kdl_only() {
        let cfg = Config::resolve(
            &map(&[("system_prompt", "be terse")]),
            &map(&[("OPENAI_API_KEY", "sk-1")]),
        )
        .unwrap();
        assert_eq!(cfg.system_prompt.as_deref(), Some("be terse"));
    }

    #[test]
    fn resolve_leaves_tools_on_unless_kdl_explicitly_says_false() {
        let env = map(&[("OPENAI_API_KEY", "sk-1")]);
        assert!(Config::resolve(&map(&[]), &env).unwrap().tools);
        assert!(
            Config::resolve(&map(&[("tools", "true")]), &env)
                .unwrap()
                .tools
        );
        assert!(
            !Config::resolve(&map(&[("tools", "false")]), &env)
                .unwrap()
                .tools
        );
        assert!(
            !Config::resolve(&map(&[("tools", "FALSE")]), &env)
                .unwrap()
                .tools
        );
        // A typo must not silently disable them.
        assert!(
            Config::resolve(&map(&[("tools", "flase")]), &env)
                .unwrap()
                .tools
        );
    }

    #[test]
    fn resolve_reads_the_tool_budget_from_kdl_only() {
        let env = map(&[("OPENAI_API_KEY", "sk-1")]);
        assert_eq!(
            Config::resolve(&map(&[("max_tool_rounds", "8")]), &env)
                .unwrap()
                .max_tool_rounds,
            8
        );
        // Absent, blank, unparseable, or zero: all fall back rather than leave
        // the tool loop uncapped or capped at nothing.
        for kdl in [
            map(&[]),
            map(&[("max_tool_rounds", "  ")]),
            map(&[("max_tool_rounds", "lots")]),
            map(&[("max_tool_rounds", "-1")]),
            map(&[("max_tool_rounds", "0")]),
        ] {
            assert_eq!(
                Config::resolve(&kdl, &env).unwrap().max_tool_rounds,
                DEFAULT_MAX_TOOL_ROUNDS,
                "{kdl:?}"
            );
        }
    }

    #[test]
    fn resolve_prefers_kdl_search_url_over_env_over_the_default() {
        let key = ("OPENAI_API_KEY", "sk-1");
        assert_eq!(
            Config::resolve(&map(&[]), &map(&[key])).unwrap().search_url,
            DEFAULT_SEARCH_URL
        );
        assert_eq!(
            Config::resolve(&map(&[]), &map(&[key, ("SEARXNG_URL", "http://env:8888")]))
                .unwrap()
                .search_url,
            "http://env:8888"
        );
        assert_eq!(
            Config::resolve(
                &map(&[("search_url", "http://kdl:8888")]),
                &map(&[key, ("SEARXNG_URL", "http://env:8888")]),
            )
            .unwrap()
            .search_url,
            "http://kdl:8888"
        );
    }

    #[test]
    fn resolve_prefers_kdl_dobbs_path_over_env_over_the_default() {
        let key = ("OPENAI_API_KEY", "sk-1");
        assert_eq!(
            Config::resolve(&map(&[]), &map(&[key])).unwrap().dobbs,
            DEFAULT_DOBBS
        );
        assert_eq!(
            Config::resolve(&map(&[]), &map(&[key, ("DOBBS_BIN", "/env/dobbs")]))
                .unwrap()
                .dobbs,
            "/env/dobbs"
        );
        assert_eq!(
            Config::resolve(
                &map(&[("dobbs_path", "/kdl/dobbs")]),
                &map(&[key, ("DOBBS_BIN", "/env/dobbs")]),
            )
            .unwrap()
            .dobbs,
            "/kdl/dobbs"
        );
    }

    /// A local daemon needs no environment at all; a remote one is only reachable
    /// with both halves, so neither may be dropped on the way to the CLI.
    /// Nothing downstream expands a tilde — Zellij passes argv straight to exec
    /// — so a `~/…` binary that got through would fail naming nothing.
    #[test]
    fn resolve_expands_a_tilde_in_the_dobbs_path() {
        let env = map(&[("OPENAI_API_KEY", "sk-1"), ("HOME", "/Users/zach")]);
        assert_eq!(
            Config::resolve(&map(&[("dobbs_path", "~/dobbs/dobbs")]), &env)
                .unwrap()
                .dobbs,
            "/Users/zach/dobbs/dobbs"
        );
        // Only a leading "~/", and only with a HOME to expand against.
        assert_eq!(
            Config::resolve(&map(&[("dobbs_path", "/opt/~/dobbs")]), &env)
                .unwrap()
                .dobbs,
            "/opt/~/dobbs"
        );
        assert_eq!(
            Config::resolve(
                &map(&[("dobbs_path", "~/dobbs/dobbs")]),
                &map(&[("OPENAI_API_KEY", "sk-1")]),
            )
            .unwrap()
            .dobbs,
            "~/dobbs/dobbs"
        );
    }

    #[test]
    fn dobbs_env_carries_a_remote_daemon_and_nothing_otherwise() {
        let key = ("OPENAI_API_KEY", "sk-1");
        let cfg = Config::resolve(&map(&[]), &map(&[key])).unwrap();
        assert!(cfg.dobbs_env().is_empty());

        let cfg = Config::resolve(
            &map(&[("dobbs_addr", "10.0.0.5:7355"), ("dobbs_token", "foobar")]),
            &map(&[key]),
        )
        .unwrap();
        assert_eq!(
            cfg.dobbs_env(),
            BTreeMap::from([
                ("DOBBS_ADDR".to_owned(), "10.0.0.5:7355".to_owned()),
                ("DOBBS_TOKEN".to_owned(), "foobar".to_owned()),
            ])
        );

        // The session environment is the fallback, as for every other setting.
        let cfg = Config::resolve(
            &map(&[("dobbs_addr", "kdl:7355")]),
            &map(&[key, ("DOBBS_ADDR", "env:7355"), ("DOBBS_TOKEN", "t")]),
        )
        .unwrap();
        assert_eq!(cfg.dobbs_addr.as_deref(), Some("kdl:7355"));
        assert_eq!(cfg.dobbs_token.as_deref(), Some("t"));
    }

    // -- search -------------------------------------------------------------

    #[test]
    fn search_query_url_normalises_every_shape_of_base() {
        for base in [
            "http://localhost:8888",
            "http://localhost:8888/",
            "http://localhost:8888/search",
            "  http://localhost:8888/search/  ",
        ] {
            assert_eq!(
                search_query_url(base, "zellij"),
                "http://localhost:8888/search?q=zellij&format=json",
                "{base}"
            );
        }
    }

    #[test]
    fn search_query_url_escapes_everything_outside_the_unreserved_set() {
        assert_eq!(
            search_query_url("http://h:8888", "rust & wasm?"),
            "http://h:8888/search?q=rust%20%26%20wasm%3F&format=json"
        );
    }

    #[test]
    fn search_query_url_escapes_multibyte_characters_per_utf8_byte() {
        assert_eq!(
            search_query_url("http://h:8888", "café"),
            "http://h:8888/search?q=caf%C3%A9&format=json"
        );
    }

    #[test]
    fn search_results_renders_title_url_and_snippet() {
        let body = br#"{"results":[
            {"title":"Zellij","url":"https://zellij.dev","content":"A terminal workspace.","score":9},
            {"title":"Docs","url":"https://zellij.dev/docs","content":"","engine":"google"}
        ]}"#;
        assert_eq!(
            search_results(body, 8),
            "Zellij\nhttps://zellij.dev\nA terminal workspace.\n\nDocs\nhttps://zellij.dev/docs"
        );
    }

    #[test]
    fn search_results_stops_at_the_limit() {
        let body = br#"{"results":[
            {"title":"a","url":"http://a","content":""},
            {"title":"b","url":"http://b","content":""},
            {"title":"c","url":"http://c","content":""}
        ]}"#;
        assert_eq!(search_results(body, 2), "a\nhttp://a\n\nb\nhttp://b");
    }

    /// An instant answer is the whole point of asking a search engine a
    /// question, so it goes above the links rather than after them.
    #[test]
    fn search_results_put_instant_answers_first() {
        let body = br#"{"answers":[{"answer":"42","url":"http://x"}],
                        "results":[{"title":"a","url":"http://a","content":""}]}"#;
        assert_eq!(search_results(body, 8), "42\n\na\nhttp://a");
    }

    #[test]
    fn search_results_accept_a_bare_string_answer() {
        let body = br#"{"answers":["42"],"results":[]}"#;
        assert_eq!(search_results(body, 8), "42");
    }

    #[test]
    fn search_results_report_an_empty_page_rather_than_nothing() {
        assert_eq!(search_results(br#"{"results":[]}"#, 8), "no results");
    }

    /// The 403-serving-HTML case: `format=json` is off in the instance's config.
    #[test]
    fn search_results_name_the_likely_misconfiguration_on_a_non_json_body() {
        let message = search_results(b"<html>Forbidden</html>", 8);
        assert!(message.contains("search.formats"), "{message}");
    }

    // -- request_body -------------------------------------------------------

    fn body_value(cfg: &Config, transcript: &[Message]) -> serde_json::Value {
        serde_json::from_slice(&request_body(cfg, transcript, true)).unwrap()
    }

    #[test]
    fn request_body_always_streams() {
        let body = body_value(&test_config(), &[]);
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["model"], json!("gpt-4o-mini"));
    }

    #[test]
    fn request_body_prepends_the_system_prompt_when_set() {
        let mut cfg = test_config();
        cfg.system_prompt = Some("be terse".into());
        let body = body_value(&cfg, &[Message::user("hi")]);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "be terse"},
                {"role": "user", "content": "hi"},
            ])
        );
    }

    #[test]
    fn request_body_adds_the_tool_first_policy_when_tools_are_enabled() {
        let mut cfg = test_config();
        cfg.tools = true;
        let body = body_value(&cfg, &[Message::user("move pane 3 left")]);
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": TOOL_FIRST_PROMPT})
        );
    }

    #[test]
    fn hocus_prompt_contract_keeps_global_routing_rules() {
        assert_eq!(
            TOOL_FIRST_PROMPT,
            "You are Hocus in Zellij. Use native tools for Zellij operations. Before an ID-based action, call `list_session` unless the immediately previous result supplied the IDs; use `list_other_sessions` for cross-session IDs. Address tabs only by `tab_id` and panes by `pane_id`; `tab_number` is the tab bar's label, not an ID, so never pass it as one. Do not verify successful mutations or retry failed pane moves with variants. Use `send_input` for an existing program. You cannot run arbitrary commands. Inspect code only with `find_files`, `search_files`, and `read_file` inside the active project. Call `agent_list` before acting on agents. Make independent calls together."
        );
    }

    #[test]
    fn request_body_appends_the_configured_prompt_to_the_tool_policy() {
        let mut cfg = test_config();
        cfg.tools = true;
        cfg.system_prompt = Some("be terse".into());
        let body = body_value(&cfg, &[Message::user("hi")]);
        assert_eq!(
            body["messages"][0]["content"],
            json!(format!("{TOOL_FIRST_PROMPT}\n\nbe terse"))
        );
        assert_eq!(
            body["messages"].as_array().unwrap().len(),
            2,
            "policy and custom instructions share one system message"
        );
    }

    #[test]
    fn request_body_omits_the_system_prompt_when_unset() {
        let body = body_value(&test_config(), &[Message::user("hi")]);
        assert_eq!(body["messages"], json!([{"role": "user", "content": "hi"}]));
    }

    #[test]
    fn request_body_excludes_our_own_error_and_notice_lines() {
        let transcript = [
            Message::user("hi"),
            Message::error("HTTP 401: nope"),
            Message::notice("cancelled"),
        ];
        let body = body_value(&test_config(), &transcript);
        assert_eq!(body["messages"], json!([{"role": "user", "content": "hi"}]));
    }

    #[test]
    fn request_body_excludes_the_empty_streaming_placeholder() {
        let transcript = [
            Message::user("hi"),
            Message::assistant("hello"),
            Message::user("again"),
            Message::assistant(""), // the placeholder for the reply we are asking for
        ];
        let body = body_value(&test_config(), &transcript);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
                {"role": "user", "content": "again"},
            ])
        );
    }

    #[test]
    fn request_body_advertises_the_tools_when_they_are_enabled() {
        let mut cfg = test_config();
        cfg.tools = true;
        let body = body_value(&cfg, &[Message::user("hi")]);
        assert_eq!(body["tools"].as_array().unwrap().len(), 52);
    }

    #[test]
    fn hocus_prompt_contract_keeps_the_tool_payload_under_20_kb() {
        let mut cfg = test_config();
        cfg.tools = true;
        let body = request_body(&cfg, &[Message::user("hi")], true);
        assert!(
            body.len() < 20_000,
            "tool payload grew to {} bytes",
            body.len()
        );
    }

    #[test]
    fn request_body_adds_the_active_project_to_the_system_message() {
        let body: serde_json::Value = serde_json::from_slice(&request_body_with_project(
            &test_config(),
            &[Message::user("hi")],
            true,
            Some(("hocus", "/work/hocus")),
        ))
        .unwrap();
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "Active project: hocus. Project root: /work/hocus."})
        );
    }

    #[test]
    fn request_body_omits_the_tools_when_they_are_disabled() {
        let body = body_value(&test_config(), &[Message::user("hi")]);
        assert!(body.get("tools").is_none());
    }

    /// The last round of the tool budget: the tools are withheld even though the
    /// config enables them, and the prompt says why so the model answers rather
    /// than claiming it cannot proceed.
    #[test]
    fn request_body_omits_the_tools_on_the_final_round() {
        let mut cfg = test_config();
        cfg.tools = true;
        let body: serde_json::Value =
            serde_json::from_slice(&request_body(&cfg, &[Message::user("hi")], false)).unwrap();
        assert!(body.get("tools").is_none());
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("budget for this turn is spent"), "{system}");
    }

    #[test]
    fn request_body_sends_tool_calls_rather_than_their_transcript_summary() {
        let transcript = [
            Message::user("open htop"),
            Message::tool_calls(vec![call("call_1", "new_pane", r#"{"command":"htop"}"#)]),
            Message::tool_result("call_1", r#"{"opened":true,"pane_id":7}"#),
        ];
        let body = body_value(&test_config(), &transcript);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "open htop"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "new_pane", "arguments": r#"{"command":"htop"}"#},
                    }],
                },
                {"role": "tool", "tool_call_id": "call_1", "content": r#"{"opened":true,"pane_id":7}"#},
            ])
        );
    }

    /// The whole point of `Message::display` being display-only: a tool the
    /// user typed is *shown* as prose and still *sent* as the raw result the
    /// model's ids and follow-up calls depend on.
    #[test]
    fn request_body_sends_the_raw_result_not_its_presentation() {
        let transcript = [
            Message::tool_calls(vec![call("call_1", "new_pane", "{}")]),
            Message::tool_result_shown(
                "call_1",
                r#"{"opened":true,"pane_id":7}"#,
                Some("Opened pane 7.".to_owned()),
            ),
        ];
        let body = body_value(&test_config(), &transcript);
        assert_eq!(
            body["messages"][1],
            json!({
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"opened":true,"pane_id":7}"#,
            })
        );
        assert!(
            !String::from_utf8_lossy(&request_body(&test_config(), &transcript, true))
                .contains("Opened pane 7"),
            "the presentation must not reach the model"
        );
    }

    /// A no-argument call streams as `""` from some providers, and echoing that
    /// back is a 400: "arguments must be valid json".
    #[test]
    fn unusable_tool_call_arguments_are_sent_as_an_empty_object() {
        for arguments in ["", "   ", "{not json", "[1,2]"] {
            let transcript = [
                Message::tool_calls(vec![call("call_1", "list_session", arguments)]),
                Message::tool_result("call_1", "{}"),
            ];
            let body = body_value(&test_config(), &transcript);
            assert_eq!(
                body["messages"][0]["tool_calls"][0]["function"]["arguments"],
                json!("{}"),
                "arguments {arguments:?}"
            );
        }
    }

    /// The API rejects a transcript where a requested call has no reply, so a
    /// declined tool must still produce one.
    #[test]
    fn every_requested_call_is_answered_even_when_declined() {
        let transcript = [
            Message::user("close everything"),
            Message::tool_calls(vec![
                call("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#),
                call("call_2", "close_pane", r#"{"pane_id":2,"is_plugin":false}"#),
            ]),
            Message::tool_result("call_1", "declined by the user"),
            Message::tool_result("call_2", "declined by the user"),
        ];
        let body = body_value(&test_config(), &transcript);
        let messages = body["messages"].as_array().unwrap();

        let requested = messages
            .iter()
            .filter_map(|m| m["tool_calls"].as_array())
            .flatten()
            .count();
        let answered = messages.iter().filter(|m| m["role"] == "tool").count();
        assert_eq!(requested, 2);
        assert_eq!(answered, requested);
    }

    /// An empty result is still an answer; skipping it would desynchronise the
    /// call/result pairing that the previous test guards.
    #[test]
    fn an_empty_tool_result_is_still_sent() {
        let transcript = [Message::tool_result("call_1", "")];
        let body = body_value(&test_config(), &transcript);
        assert_eq!(
            body["messages"],
            json!([{"role": "tool", "tool_call_id": "call_1", "content": ""}])
        );
    }

    // -- api_error_message --------------------------------------------------

    #[test]
    fn api_error_message_extracts_the_openai_error_field() {
        let body =
            br#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#;
        assert_eq!(
            api_error_message(401, body),
            "HTTP 401: Incorrect API key provided"
        );
    }

    #[test]
    fn api_error_message_falls_back_to_the_raw_body() {
        assert_eq!(
            api_error_message(502, b"<html>bad gateway</html>"),
            "HTTP 502: <html>bad gateway</html>"
        );
    }

    #[test]
    fn api_error_message_handles_an_empty_body() {
        assert_eq!(api_error_message(500, b""), "HTTP 500");
    }

    #[test]
    fn api_error_message_truncates_a_huge_body() {
        let body = "x".repeat(5000);
        let message = api_error_message(500, body.as_bytes());
        assert_eq!(message.chars().count(), "HTTP 500: ".len() + 500);
    }

    // -- SseParser ----------------------------------------------------------

    fn deltas(items: Vec<SseItem>) -> Vec<String> {
        items
            .into_iter()
            .filter_map(|item| match item {
                SseItem::Delta(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    const FRAME: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";

    #[test]
    fn parses_a_single_complete_frame() {
        let mut parser = SseParser::default();
        assert_eq!(deltas(parser.feed(FRAME)), vec!["Hello"]);
    }

    #[test]
    fn reassembles_a_frame_split_across_many_chunks() {
        let mut parser = SseParser::default();
        let mut collected = Vec::new();
        // One byte at a time - the worst case the host can hand us.
        for byte in FRAME {
            collected.extend(parser.feed(&[*byte]));
        }
        assert_eq!(deltas(collected), vec!["Hello"]);
    }

    #[test]
    fn splits_a_multibyte_character_across_chunks_without_corruption() {
        let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"héllo→\"}}]}\n".as_bytes();
        let mut parser = SseParser::default();
        let mut collected = Vec::new();
        for byte in frame {
            collected.extend(parser.feed(&[*byte]));
        }
        assert_eq!(deltas(collected), vec!["héllo→"]);
    }

    #[test]
    fn handles_crlf_line_endings() {
        let mut parser = SseParser::default();
        let frame = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\r\n\r\n";
        assert_eq!(deltas(parser.feed(frame)), vec!["Hi"]);
    }

    #[test]
    fn ignores_keep_alive_comments_and_blank_lines() {
        let mut parser = SseParser::default();
        assert!(parser.feed(b": ping\n\n\n").is_empty());
    }

    #[test]
    fn ignores_non_data_sse_fields() {
        let mut parser = SseParser::default();
        assert!(parser
            .feed(b"event: message\nid: 42\nretry: 1000\n")
            .is_empty());
    }

    #[test]
    fn recognises_the_done_sentinel() {
        let mut parser = SseParser::default();
        assert_eq!(parser.feed(b"data: [DONE]\n"), vec![SseItem::Done]);
    }

    #[test]
    fn ignores_a_usage_only_chunk_with_no_choices() {
        let mut parser = SseParser::default();
        let frame = b"data: {\"choices\":[],\"usage\":{\"total_tokens\":7}}\n";
        assert!(parser.feed(frame).is_empty());
    }

    #[test]
    fn ignores_the_finish_chunk_with_an_empty_delta() {
        let mut parser = SseParser::default();
        let frame = b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n";
        assert!(parser.feed(frame).is_empty());
    }

    #[test]
    fn surfaces_an_error_reported_inside_the_stream() {
        let mut parser = SseParser::default();
        let frame = b"data: {\"error\":{\"message\":\"rate limited\"}}\n";
        assert_eq!(
            parser.feed(frame),
            vec![SseItem::ApiError("rate limited".into())]
        );
    }

    #[test]
    fn ignores_a_data_line_that_is_not_json() {
        let mut parser = SseParser::default();
        assert!(parser.feed(b"data: not json at all\n").is_empty());
    }

    #[test]
    fn preserves_order_across_two_frames_in_one_chunk() {
        let mut parser = SseParser::default();
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n";
        assert_eq!(deltas(parser.feed(chunk)), vec!["a", "b"]);
    }

    #[test]
    fn retains_an_incomplete_trailing_line_for_the_next_feed() {
        let mut parser = SseParser::default();
        assert!(parser
            .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"par")
            .is_empty());
        assert_eq!(deltas(parser.feed(b"tial\"}}]}\n")), vec!["partial"]);
    }

    // -- ToolCallAccumulator ------------------------------------------------

    /// Drive a whole stream through both stages, the way `on_chunk` does.
    fn accumulate(chunks: &[&[u8]]) -> Vec<ToolCall> {
        let mut parser = SseParser::default();
        let mut accumulator = ToolCallAccumulator::default();
        for chunk in chunks {
            for item in parser.feed(chunk) {
                accumulator.feed(&item);
            }
        }
        accumulator.take()
    }

    /// The realistic shape: `id` and `name` arrive once, then `arguments` comes
    /// in as many fragments as the provider feels like.
    #[test]
    fn reassembles_arguments_split_across_many_fragments() {
        let calls = accumulate(&[
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"new_pane","arguments":""}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"com"}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mand\":\"h"}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"top\"}"}}]}}]}
"#,
        ]);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc");
        assert_eq!(calls[0].name, "new_pane");
        assert_eq!(calls[0].arguments, r#"{"command":"htop"}"#);
    }

    /// Two calls in one reply are interleaved by index, not sent in sequence.
    #[test]
    fn keeps_interleaved_parallel_calls_separate() {
        let calls = accumulate(&[
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"close_pane","arguments":"{\"pane_id\":"}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"close_tab","arguments":"{\"tab_id\":"}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"2}"}}]}}]}
"#,
        ]);

        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].id.as_str(), calls[0].name.as_str()),
            ("a", "close_pane")
        );
        assert_eq!(calls[0].arguments, r#"{"pane_id":1}"#);
        assert_eq!(
            (calls[1].id.as_str(), calls[1].name.as_str()),
            ("b", "close_tab")
        );
        assert_eq!(calls[1].arguments, r#"{"tab_id":2}"#);
    }

    /// The SSE framing already survives arbitrary chunk boundaries; this checks
    /// that accumulation on top of it does too.
    #[test]
    fn reassembles_a_call_delivered_one_byte_at_a_time() {
        let frame = br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"list_session","arguments":"{}"}}]}}]}
"#;
        let chunks: Vec<&[u8]> = frame.iter().map(std::slice::from_ref).collect();
        let calls = accumulate(&chunks);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_session");
        assert_eq!(calls[0].arguments, "{}");
    }

    /// Some providers omit `index` when only one call is in flight.
    #[test]
    fn treats_a_missing_index_as_a_single_call() {
        let calls = accumulate(&[
            br#"data: {"choices":[{"delta":{"tool_calls":[{"id":"x","function":{"name":"list_session","arguments":"{"}}]}}]}
"#,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"}"}}]}}]}
"#,
        ]);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn a_fragment_that_never_named_a_tool_is_dropped() {
        let calls = accumulate(&[
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}
"#,
        ]);
        assert!(calls.is_empty());
    }

    #[test]
    fn a_plain_text_reply_accumulates_no_calls() {
        let calls = accumulate(&[FRAME]);
        assert!(calls.is_empty());
    }

    #[test]
    fn taking_the_calls_leaves_the_accumulator_empty() {
        let mut accumulator = ToolCallAccumulator::default();
        accumulator.feed(&SseItem::ToolCallFragment {
            index: 0,
            id: Some("x".into()),
            name: Some("list_session".into()),
            arguments: Some("{}".into()),
        });
        assert_eq!(accumulator.take().len(), 1);
        assert!(
            accumulator.take().is_empty(),
            "a second take yields nothing"
        );
    }

    #[test]
    fn bails_out_and_resets_when_a_line_grows_without_bound() {
        let mut parser = SseParser::default();
        let flood = vec![b'x'; MAX_PENDING + 1];
        assert!(matches!(
            parser.feed(&flood).as_slice(),
            [SseItem::ApiError(_)]
        ));
        // Buffer was reset, so a well-formed frame still parses afterwards.
        assert_eq!(deltas(parser.feed(FRAME)), vec!["Hello"]);
    }
}
