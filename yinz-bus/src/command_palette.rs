//! Private host routing for native and Deno palettes. Guests cannot supply host requests.
use std::sync::{atomic::AtomicBool, Arc};
use yinz_types::command_palette::{Command, Message, Scope};
use yinz_types::internal_ids::{ClientId, PluginId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Owner {
    pub plugin_id: PluginId,
    pub generation: u64,
    /// Shared instances serve every client; ordinary instances serve their owner only.
    pub client_id: Option<ClientId>,
}

#[derive(Clone, Debug)]
pub struct Delivery {
    pub owner: Owner,
    pub client: ClientId,
    pub message: Message,
    /// Invalidated synchronously by screen input, before debounce or executor delivery.
    pub active: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Debug)]
pub enum Instruction {
    UsageReply {
        client: ClientId,
        opening: u64,
        view: u64,
        usage: yinz_types::agent::control::ProviderUsage,
    },
    HistoryTick {
        client: ClientId,
        opening: u64,
        view: u64,
        generation: u64,
    },
    AgentReply {
        client: ClientId,
        opening: u64,
        step: u64,
        reply: Result<AgentReply, String>,
    },
    Command {
        owner: Owner,
        client: ClientId,
        unavailable: Arc<AtomicBool>,
        command: Command,
    },
    Dispose {
        plugin_id: PluginId,
        generation: u64,
    },
    Discover {
        client: ClientId,
        opening: u64,
        revision: u64,
    },
    Refresh {
        client: ClientId,
        scope: Scope,
    },
    Timeout {
        owner: Owner,
        client: ClientId,
        scope: Scope,
    },
}

/// Native palette requests never cross the plugin wire boundary.
#[derive(Clone, Debug)]
pub struct AgentRequest {
    pub client: ClientId,
    pub opening: u64,
    pub step: u64,
    pub source: yinz_types::data::PaneInfo,
    pub action: AgentAction,
    pub values: Vec<String>,
    pub active: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub enum AgentAction {
    Usage {
        provider: String,
    },
    Prompt {
        pane_id: yinz_types::pane_id::PaneId,
        session_id: Option<String>,
        target_session: Option<yinz_types::internal_ids::SessionName>,
        submission_id: String,
    },
    History(HistoryAction),
    NewProject {
        kind: String,
    },
    Start {
        kind: String,
        project: Option<String>,
    },
    Projects {
        kind: String,
    },
    Worktrees {
        kind: String,
        project: AgentProject,
    },
    Checkout {
        kind: String,
        project: AgentProject,
        path: Option<std::path::PathBuf>,
    },
    Launch {
        kind: String,
        project: AgentProject,
        path: Option<std::path::PathBuf>,
        branch: Option<String>,
    },
    Current {
        kind: String,
    },
    Duplicate {
        kind: Option<String>,
    },
    Fork,
}

#[derive(Clone, Debug)]
pub enum AgentProject {
    Registered(String),
    Repository(std::path::PathBuf),
}

#[derive(Clone, Debug)]
pub struct AgentChoice {
    pub label: String,
    pub description: String,
    pub action: AgentAction,
}

#[derive(Clone, Debug)]
pub enum AgentReply {
    Prompted(yinz_types::agent::control::AgentResponse),
    History(HistoryReply),
    Choices {
        title: String,
        choices: Vec<AgentChoice>,
    },
    Form {
        title: String,
        labels: Vec<String>,
        submit: String,
        action: AgentAction,
    },
    Launched(yinz_types::agent::control::AgentResponse),
}

#[path = "palette_query.rs"]
pub mod query;

#[derive(Clone, Debug)]
pub enum HistoryAction {
    Tools {
        id: String,
        revisions: Vec<u64>,
    },
    Hit {
        id: String,
        revision: u64,
    },
    Search(query::AgentQuery),
    Run {
        id: String,
        from: Option<u64>,
        query: String,
    },
    Event {
        id: String,
        revision: u64,
    },
    Resume {
        id: String,
    },
}

#[derive(Clone, Debug)]
pub struct HistoryRow {
    pub id: String,
    pub label: String,
    pub description: String,
    pub action: HistoryAction,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryHeader {
    pub name: String,
    pub status: String,
    pub location: String,
    pub started: String,
    pub details: String,
    pub task: String,
    pub project: String,
    pub session_id: String,
    pub usage: Option<yinz_types::agent::control::ProviderUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEvent {
    pub revisions: Vec<u64>,
    pub tool_calls: usize,
    pub revision: u64,
    pub kind: String,
    pub time: String,
    pub created_at_ms: Option<i64>,
    pub markdown: String,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub enum HistoryReply {
    Run {
        header: HistoryHeader,
        events: Vec<HistoryEvent>,
        rows: Vec<HistoryRow>,
        status: String,
    },
    Reader {
        header: String,
        markdown: String,
        tool: String,
    },
    Rows {
        rows: Vec<HistoryRow>,
        status: String,
    },
}
