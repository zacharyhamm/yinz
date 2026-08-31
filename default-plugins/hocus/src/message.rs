use crate::tools::{self, ToolCall};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    /// The result of a tool call, fed back to the model. Rendered dim, and
    /// truncated unless `Message::full_output` says otherwise, since a session
    /// listing would otherwise fill the pane.
    Tool,
    /// Something went wrong. Rendered in the theme's error colour.
    Error,
    /// Our own aside, e.g. "cancelled". Rendered dim.
    ///
    /// Default only so `Message` can derive it; every constructor sets a role.
    #[default]
    Notice,
}

/// A model's chain-of-thought, kept only so it can be handed straight back.
///
/// Endpoints disagree about both whether they produce one and what to call it,
/// so this records the spelling alongside the text: what came back under
/// `reasoning_content` is returned under `reasoning_content`, and likewise for
/// `reasoning`. Nothing normalises between them, because an endpoint that has
/// never heard of the other family's key has no reason to accept it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Reasoning {
    pub text: String,
    #[serde(default)]
    pub key: ReasoningKey,
}

/// Which field a provider streams its thinking under. A closed set rather than
/// the string as it arrived, so an endpoint can only ever be sent a field name
/// it used itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReasoningKey {
    /// `reasoning_content`: DeepSeek, vLLM, SGLang, Qwen.
    #[default]
    ReasoningContent,
    /// `reasoning`: OpenRouter, Ollama.
    Reasoning,
}

impl Reasoning {
    pub fn new(text: impl Into<String>, key: ReasoningKey) -> Self {
        Reasoning {
            text: text.into(),
            key,
        }
    }
}

/// One opaque reasoning item from OpenAI's Responses API, kept so it can be handed
/// straight back on the next turn of a tool loop.
///
/// Nothing here is readable: `encrypted_content` is the model's chain-of-thought sealed
/// for the endpoint that produced it, asked for by `include:
/// ["reasoning.encrypted_content"]` because `store: false` leaves nothing server-side to
/// refer to by id. The visible summary is the separate [`Reasoning`] field.
///
/// `before` is how many of the turn's replayable output items (assistant prose or
/// function calls) had already been announced when this item arrived, which is what
/// puts it back in the position it came in. The API rejects the request if a reasoning
/// item is not followed by the item it produced, so the interleaving is a correctness
/// requirement rather than tidiness.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ReasoningItem {
    pub id: String,
    pub encrypted_content: String,
    #[serde(default)]
    pub before: usize,
}

/// Serialized as-is into the archive (see `history`). The optional fields
/// carry `serde(default)` so a future field can't make an existing file
/// unreadable, and are skipped when empty so an archived chat stays greppable.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Assistant only: the tool calls this message requested. `content` is the
    /// prose the model emitted alongside them and `display` includes the
    /// human-readable call summary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Assistant only: the model's chain-of-thought, as the endpoint sent it,
    /// together with the key it sent it under. Echoed back verbatim on a
    /// tool-calling turn - DeepSeek's thinking mode rejects a request whose
    /// tool-calling assistant turns have lost it, and rejects a summarised one
    /// too. `None` for every endpoint that does not produce one, which is what
    /// keeps the field off the wire for OpenAI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    /// Assistant only, and only on the Responses transport: the opaque reasoning
    /// items this turn produced, echoed back verbatim before the calls they led
    /// to so a reasoning model keeps its thinking across a tool loop. Empty for
    /// every endpoint that does not produce them, which keeps the request to
    /// everyone else unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_items: Vec<ReasoningItem>,
    /// Assistant tool-call turns only, on the Responses transport: how many calls
    /// preceded the assistant prose item. `None` preserves the historical shape,
    /// where prose was replayed first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prose_before: Option<usize>,
    /// `Role::Tool` only: which call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// `Role::Tool` only: render every line rather than eliding to the first
    /// few. Set for a tool the user typed — its output is the thing they asked
    /// for, not context for the model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_output: bool,
    /// Markdown to draw *instead of* `content`. For a tool result this presents
    /// raw output; for an assistant tool-call turn it includes the call summary.
    /// Display-only — `content` is still exactly what `api::request_body` sends.
    ///
    /// `None` means draw `content` as it arrived, which is what every result
    /// the model asked for gets, and what a result we have nothing better to
    /// say about gets — an error sentence, a bare `cleared`. Falling back
    /// rather than presenting badly is why this is an `Option`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

impl Message {
    fn new<S: Into<String>>(role: Role, content: S) -> Self {
        Message {
            role,
            content: content.into(),
            ..Message::default()
        }
    }
    pub fn user<S: Into<String>>(content: S) -> Self {
        Message::new(Role::User, content)
    }
    /// A user turn shown as something other than what is sent: a skill, whose
    /// `content` is the expanded prompt and whose `display` is the line that
    /// was typed. `None` is a plain user turn.
    pub fn user_shown<S: Into<String>>(content: S, display: Option<String>) -> Self {
        Message {
            display,
            ..Message::user(content)
        }
    }
    pub fn assistant<S: Into<String>>(content: S) -> Self {
        Message::new(Role::Assistant, content)
    }
    pub fn error<S: Into<String>>(content: S) -> Self {
        Message::new(Role::Error, content)
    }
    pub fn notice<S: Into<String>>(content: S) -> Self {
        Message::new(Role::Notice, content)
    }

    /// An assistant tool-call turn with no model prose. The summary is only for
    /// display; providers such as DeepSeek require non-null wire `content`.
    pub fn tool_calls(calls: Vec<ToolCall>) -> Self {
        let summary = calls
            .iter()
            .map(tools::summarize)
            .collect::<Vec<_>>()
            .join("\n");
        Message {
            role: Role::Assistant,
            display: Some(summary),
            tool_calls: calls,
            ..Message::default()
        }
    }

    pub fn tool_result<S: Into<String>>(id: &str, content: S) -> Self {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(id.to_owned()),
            ..Message::default()
        }
    }

    /// The result of a tool the user typed: same message, rendered whole.
    pub fn tool_result_full<S: Into<String>>(id: &str, content: S) -> Self {
        Message {
            full_output: true,
            ..Message::tool_result(id, content)
        }
    }

    /// The result of a tool the user typed, together with the rendering it
    /// should be shown as. `content` is untouched, so the model is unaffected.
    pub fn tool_result_shown<S: Into<String>>(
        id: &str,
        content: S,
        display: Option<String>,
    ) -> Self {
        Message {
            display,
            ..Message::tool_result_full(id, content)
        }
    }
}

/// The reply arriving from the endpoint. Independent of `Status`: a tool the
/// user typed runs alongside the reply rather than instead of it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamState {
    /// The host's handle for this request, and what `on_chunk` routes on.
    pub id: String,
    /// The outcome is already recorded — the user cancelled, or a non-2xx body
    /// was already shown — but the host still owes us exactly one `done: true`
    /// chunk. This flag exists to swallow it without recording a second
    /// outcome.
    pub finishing: bool,
    /// What the provider said this exchange cost, prompt and completion
    /// together, held until `done` so the `wire_chars` baseline is taken once
    /// the reply it covers — text, tool calls and all — has settled.
    #[serde(default)]
    pub usage: Option<u64>,
}

/// Lifecycle of the tool queue. What the *model* asked for and what the user
/// typed share one queue, so this is one state for both.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Idle,
    /// The head of `pending` is destructive and is waiting on `y`/`n`.
    ConfirmingTool,
    /// The head of `pending` is a Slack tool waiting on `RunCommandResult`.
    AwaitingCommand,
    /// The head of `pending` is a `web_search` waiting on `WebRequestResult`.
    AwaitingSearch,
    /// The head of `pending` is a Linear tool waiting on `WebRequestResult`.
    ///
    /// Its own state rather than sharing `AwaitingSearch`: both finishers guard
    /// on the status *and* the head's id, so a shared one would leave two
    /// claimants for the same reply and let call order decide — exactly the
    /// coupling `on_response`'s "was this mine?" contract exists to prevent.
    AwaitingLinear,
    /// The head is owned by a registered Lua extension and is waiting on its private pipe reply.
    AwaitingExtension,
    /// The head of `pending` is a `plugin_reload` whose plugin has been asked to load, waiting
    /// on the timer to go and look at the pane it landed in.
    ///
    /// A reload is fire-and-forget on the host, so the only way to say whether it worked is to
    /// read the pane afterwards; this is the state between asking and looking.
    AwaitingPluginLoad,
}

/// Whether this conversation is planning rather than acting, and how far along.
///
/// Its own field rather than two more `Status` variants, because `Status` is the
/// lifecycle of the *tool queue* and `tools_busy()` is `status != Idle`. A
/// planning state spelled there would make `is_in_flight()` true, which queues
/// every typed message and every `hocus_ask` question behind the plan — and
/// typing at a plan is exactly how you correct one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Plan {
    #[default]
    Off,
    /// Only the read-only tools are advertised, and the model is asked for a
    /// plan rather than for the work.
    Planning,
    /// A plan has been given and is waiting on `y`/`n`.
    AwaitingApproval,
}

/// A question waiting behind a busy chat. `content` is what will be sent;
/// `display`, when set, is what the person typed to produce it — a skill's
/// `/name args` — and is what the queue notices and the archive show.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub content: String,
    pub display: Option<String>,
}

impl Ask {
    pub fn text<S: Into<String>>(content: S) -> Self {
        Ask {
            content: content.into(),
            display: None,
        }
    }

    /// What to print for this ask: the typed line if there was one.
    pub fn shown(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.content)
    }
}

/// A prompt the plugin re-asks itself on an interval. Firing one is exactly
/// what `submit()` does — push a user message and start a request — so a
/// scheduled run streams, calls tools and gets archived like any other turn.
///
/// Persisted as a `job` row in `shared:hocus` when armed and when it fires,
/// so a reload restores it with its next run still due when it was; the
/// countdown itself lives only here. See `State::restore_persisted`.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    /// Seconds between runs.
    pub every: f64,
    /// Counts down; reset to `every` when the job fires.
    pub due_in: f64,
    pub prompt: String,
    /// Whether firing should pop the pane open. The model sets this for jobs
    /// the user asked to be told about.
    pub show: bool,
    /// Whether this job can run confirmation-gated tools without asking.
    pub auto_confirm: bool,
}

/// The scheduled run that currently owns the conversation, from the moment a
/// job fires until its turn ends. A manual turn has none.
///
/// What `pump_tools` reads to answer a confirmation-gated tool call: a run
/// with `auto_confirm` runs it, one without declines it rather than parking
/// the conversation — and every other job behind it — on a `y`/`n` nobody
/// may be there to press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduledRun {
    pub job_id: u64,
    pub auto_confirm: bool,
}

/// A list with a selected row: the history and model browser. The agent list
/// that used to share this type lives in the hocus-agents pane now.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Picker {
    /// What is shown, one row per entry.
    pub entries: Vec<PickerRow>,
    pub selected: usize,
    pub kind: PickerKind,
}

/// One line of a picker: what to draw, and whether the user can land on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickerRow {
    pub text: String,
    pub kind: RowKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RowKind {
    /// A row the user can select.
    Item,
}

impl PickerRow {
    /// A row that is nothing but itself — every chat title and model id.
    pub fn item(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: RowKind::Item,
        }
    }

    pub fn selectable(&self) -> bool {
        matches!(self.kind, RowKind::Item)
    }
}

/// Which list the picker is showing.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum PickerKind {
    /// `entries` are chat titles; the ids are parallel to them.
    Chats(Vec<u64>),
    /// `entries` are model ids. Empty means the fetch is still in flight — every
    /// response either fills the list or closes the picker.
    Models,
}
