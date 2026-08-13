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

/// Serialized as-is into the archive (see `history`). The optional fields
/// carry `serde(default)` so a future field can't make an existing file
/// unreadable, and are skipped when empty so an archived chat stays greppable.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Assistant only: the tool calls this message requested. When non-empty,
    /// `content` is our own human-readable summary for the transcript and is
    /// *not* what gets sent - see `api::request_body`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// `Role::Tool` only: which call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// `Role::Tool` only: render every line rather than eliding to the first
    /// few. Set for a tool the user typed — its output is the thing they asked
    /// for, not context for the model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_output: bool,
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
    pub fn assistant<S: Into<String>>(content: S) -> Self {
        Message::new(Role::Assistant, content)
    }
    pub fn error<S: Into<String>>(content: S) -> Self {
        Message::new(Role::Error, content)
    }
    pub fn notice<S: Into<String>>(content: S) -> Self {
        Message::new(Role::Notice, content)
    }

    /// The assistant turn that asked for tools. Its transcript text is a summary
    /// of the calls; the calls themselves are the payload.
    pub fn tool_calls(calls: Vec<ToolCall>) -> Self {
        let summary = calls
            .iter()
            .map(tools::summarize)
            .collect::<Vec<_>>()
            .join("\n");
        Message {
            role: Role::Assistant,
            content: summary,
            tool_calls: calls,
            tool_call_id: None,
            full_output: false,
        }
    }

    pub fn tool_result<S: Into<String>>(id: &str, content: S) -> Self {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.to_owned()),
            full_output: false,
        }
    }

    /// The result of a tool the user typed: same message, rendered whole.
    pub fn tool_result_full<S: Into<String>>(id: &str, content: S) -> Self {
        Message {
            full_output: true,
            ..Message::tool_result(id, content)
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
    /// The head of `pending` is a todo change waiting on `hocus-store`.
    AwaitingStore,
}

/// A prompt the plugin re-asks itself on an interval. Firing one is exactly
/// what `submit()` does — push a user message and start a request — so a
/// scheduled run streams, calls tools and gets archived like any other turn.
///
/// Not persisted: a reload drops jobs, the same way it drops the in-flight
/// stream.
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

/// A list with a selected row. History and models use one modal instance; the
/// agent strip keeps another visible above the conversation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Picker {
    /// What is shown, one row per entry.
    pub entries: Vec<PickerRow>,
    pub selected: usize,
    pub kind: PickerKind,
}

/// One line of a picker: what to draw, and whether the user can land on it.
///
/// A header is drawn and skipped over. That is what lets the agent list group
/// its rows while keeping its pane ids parallel to its lines — a header simply
/// has no id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickerRow {
    pub text: String,
    pub kind: RowKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RowKind {
    /// A row the user can select.
    Item,
    /// The project the rows below it belong to.
    Project,
    /// The branch the rows below it are checked out on, under a project header.
    Branch,
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
    /// `entries` are agent rows and the group headers over them; the pane ids
    /// are parallel to them, and a header's is `None`. `Enter` focuses the pane.
    Agents(Vec<Option<u32>>),
}
