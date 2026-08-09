//! hocus — chat with an OpenAI-compatible endpoint from a Zellij popup.

mod agents;
mod api;
mod history;
mod log;
mod projects;
mod text_input;
mod todos;
mod tools;
mod ui;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use zellij_tile::prelude::*;

use api::{Config, ConfigError, SseItem, SseParser, ToolCallAccumulator};
use text_input::{InputAction, TextInput};
use tools::{Dispatch, ToolCall};

/// How many times in a row the model may call tools before the tools are taken
/// away and it has to answer from what it has. Without this a model that keeps
/// re-listing panes bills the user forever.
const MAX_TOOL_ROUNDS: usize = 20;

/// How much of a command's stdout or stderr reaches the model. Generous enough
/// for a day of a Slack channel or a long build log, small enough that one
/// runaway command cannot dominate the conversation from then on.
const MAX_TOOL_OUTPUT: usize = 40_000;

/// Spinner frame duration. The timer is one-shot, so this is also how often the
/// plugin wakes itself while waiting — and only while waiting.
///
/// ponytail: 10 Hz repaints the whole pane grid, since Zellij gives plugins no
/// partial redraw. Cheap for a popup, and bounded by how long the wait lasts.
/// Slow this down if it ever shows up in a profile.
const FRAME_SECONDS: f64 = 0.1;

/// How often the timer chain wakes when the only thing it is doing is counting
/// scheduled jobs down. A job's interval is measured in tens of seconds at
/// least, so a second of granularity is free and 10 Hz forever would not be.
const JOB_SECONDS: f64 = 1.0;

/// The shortest interval `schedule` will accept. A model that asks for one
/// second bills the user sixty times a minute for a pane that hasn't changed.
const MIN_JOB_SECONDS: f64 = 5.0;

/// How often the agent snapshot is refreshed.
///
/// ponytail: this is a heartbeat the plugin now holds forever — before agent
/// polling it went silent whenever nothing was in flight and no job was scheduled.
/// Each poll is one `get_session_list` plus one `get_pane_running_command` per
/// terminal pane, and it asks for a redraw only when a transition notice landed in
/// the transcript. Tie it to pane focus or drop it back to on-demand if the
/// wake-ups ever cost more than the notices are worth.
const AGENT_SECONDS: f64 = 5.0;

/// Slash commands that drive the plugin itself rather than a tool. Typed and
/// completed like tools, but never offered to the model.
const LOCAL_COMMANDS: [&str; 3] = ["history", "models", "stop_jobs"];

/// Context key carrying a chat request's stream id back on its `WebRequestResult`,
/// so the response can be matched to the chat that asked.
const STREAM_CONTEXT: &str = "stream";

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    /// The result of a tool call, fed back to the model. Rendered dim and
    /// truncated, since a session listing would otherwise fill the pane.
    Tool,
    /// Something went wrong. Rendered in the theme's error colour.
    Error,
    /// Our own aside, e.g. "cancelled". Rendered dim.
    ///
    /// Default only so `Message` can derive it; every constructor sets a role.
    #[default]
    Notice,
}

/// Serialized as-is into the archive (see `history`). The two optional fields
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
        }
    }

    pub fn tool_result<S: Into<String>>(id: &str, content: S) -> Self {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.to_owned()),
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
    /// The head of `pending` is a `run_command` waiting on `RunCommandResult`.
    AwaitingCommand,
    /// The head of `pending` is a `web_search` waiting on `WebRequestResult`.
    AwaitingSearch,
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

/// The chat archive or the model list, open over the transcript. Modal: while it
/// is open it owns every key, because a keystroke leaking into the input box
/// behind it would be invisible.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Picker {
    /// What is shown, one row per entry.
    pub entries: Vec<String>,
    pub selected: usize,
    pub kind: PickerKind,
}

/// Which list the picker is showing. `Tab` cycles between them.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum PickerKind {
    /// `entries` are chat titles; the ids are parallel to them.
    Chats(Vec<u64>),
    /// `entries` are model ids. Empty means the fetch is still in flight — every
    /// response either fills the list or closes the picker.
    Models,
    /// `entries` are agent rows; the pane ids are parallel to them.
    /// `Enter` focuses the pane.
    Agents(Vec<u32>),
}

/// One conversation's worth of `State`, lifted out so a chat can be set aside
/// while another is on screen and still receive its reply.
///
/// The foreground chat's fields stay flat on `State` — moving every
/// `self.messages` behind `self.chat.messages` would churn most of this file
/// and `ui.rs` for nothing. This is only the shape a chat takes while it is
/// parked, and the shape the swap moves.
#[derive(Default)]
struct Chat {
    messages: Vec<Message>,
    status: Status,
    stream: Option<StreamState>,
    reply_at: Option<usize>,
    sse: SseParser,
    tool_calls: ToolCallAccumulator,
    pending: VecDeque<ToolCall>,
    tool_rounds: usize,
    owes_reply: bool,
    chat_id: Option<u64>,
    saved: (usize, usize),
    scroll: BTreeMap<ClientId, usize>,
    job_auto_confirm: bool,
}

impl Chat {
    /// Whether this chat is still owed something — the only reason to hold it
    /// rather than let the archive have it back.
    fn is_live(&self) -> bool {
        self.stream.is_some() || !matches!(self.status, Status::Idle)
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct State {
    /// Whether the permission request has come back. Nothing permission-gated
    /// may run before this is true.
    pub perms: bool,
    pub cfg: Result<Config, ConfigError>,
    /// Plugin configuration from the KDL block, kept for re-resolution.
    kdl: BTreeMap<String, String>,
    /// The session environment, captured once the permission grant makes it
    /// readable. Zellij froze it when the session started and never revises it,
    /// so re-reading it later would cost a host call to learn nothing.
    pub env: BTreeMap<String, String>,
    pub messages: Vec<Message>,
    pub input: TextInput,
    pub status: Status,
    /// The reply on its way in, if there is one.
    pub stream: Option<StreamState>,
    /// Where in `messages` the arriving reply is being appended. An index rather
    /// than "the last message", because a tool the user typed mid-stream pushes
    /// its call and result *after* the placeholder.
    reply_at: Option<usize>,
    sse: SseParser,
    /// Spinner frame counter, advanced only by the timer and so only while
    /// something is actually being waited on.
    tick: u64,
    /// Whether a timer is outstanding. The guard against arming a second chain,
    /// which would double the frame rate and never converge back to one.
    timer_armed: bool,
    tool_calls: ToolCallAccumulator,
    /// Calls the model asked for that have not been answered yet. The head is
    /// the one being confirmed or executed.
    pending: VecDeque<ToolCall>,
    /// Consecutive tool rounds in this exchange, reset by any new user message.
    tool_rounds: usize,
    /// Whether the model is waiting on tool results. A call the user typed is
    /// its own destination — the result is in the transcript for the next
    /// message to build on, but it answers no question — so only the model's
    /// calls set this, and only it makes an emptied queue start a request.
    ///
    /// A flag rather than a per-call mark because the two kinds now mix in one
    /// queue: the model's calls land when its stream ends, which can be while a
    /// typed one is still running. Whichever call happens to empty the queue,
    /// the model is owed its answer if it ever asked.
    owes_reply: bool,
    stream_seq: u64,
    /// Wrapped transcript lines hidden *below* the viewport, per client. Zero —
    /// which is what a missing entry means — is pinned to the bottom, and is
    /// what makes a growing reply follow itself.
    ///
    /// Keyed by client because one shared instance renders once per attached
    /// client (see `client`), and one person reading back should not drag
    /// everyone else with them. `pin_to_bottom` clears the map on any
    /// conversation change, so a departed client leaves nothing behind.
    scroll: BTreeMap<ClientId, usize>,
    /// The client whose event we are handling, or whom we are rendering for.
    ///
    /// Hocus runs as a session-*shared* plugin: one wasm instance serves every
    /// attached client, and the host sets this per event and per render pass
    /// rather than giving each client an instance of its own.
    client: ClientId,
    /// Viewport height from the last render, so paging knows its page size.
    ///
    /// One number for every client: the host renders a shared plugin at a
    /// single size, so each client's render pass gets the same `rows`.
    pub transcript_rows: usize,
    /// Memoised `ui::transcript_lines`, so a keystroke — which changes nothing
    /// in the transcript — re-wraps nothing. See `ui::TranscriptKey` for why
    /// the key is sufficient.
    pub transcript_cache: Option<(ui::TranscriptKey, Vec<ui::VLine>)>,
    /// Which archived chat this conversation is being written back to. `None`
    /// until the first save, which is what makes an untouched pane leave no file.
    chat_id: Option<u64>,
    /// `fingerprint()` as of the last save, so an event that changed nothing
    /// doesn't rewrite the file.
    saved: (usize, usize),
    /// Conversations set aside but still live: a reply arriving, or a tool
    /// still running. A chat that settles is archived and dropped from here —
    /// `/history` loads it back from the file if you want it again.
    parked: Vec<Chat>,
    /// The archive browser, when it is open.
    pub picker: Option<Picker>,
    /// Scheduled prompts, counted down by the timer chain.
    pub jobs: Vec<Job>,
    job_seq: u64,
    /// Authority for the scheduled run that currently owns the conversation.
    job_auto_confirm: bool,
    /// Terminal panes as of the last `refresh_panes`, which every agent tool runs
    /// first. The only pane snapshot the agent tools trust, which is what stops them
    /// acting on a pane that has closed.
    panes: Vec<agents::AgentPane>,
    /// Kinds the user set by hand with `agent_register`, keyed by pane id. These win
    /// over detection, and are dropped when their pane leaves the snapshot.
    manual_kinds: BTreeMap<u32, String>,
    /// What each running agent's pane showed last poll, for telling busy from idle.
    /// Derived from the pane snapshot like `panes`, never archived.
    activity: agents::Activity,
    /// Seconds until the next agent poll, counted down by `on_timer`.
    agent_due_in: f64,
    /// A poke is out to the poll worker and its reply has not come back. A sync
    /// poll clears it, which is also what makes the stale reply drop.
    poll_pending: bool,
    /// Due polls that found the last poke still unanswered, toward the strike-out.
    polls_unanswered: u8,
    /// The worker never answered: unpatched zellij or a wedged instance. Poll
    /// inline for the rest of the session.
    poll_dead: bool,
    /// Milliseconds since plugin start, advanced by `on_timer`.
    mono_ms: u64,
    /// Monotonic log sequence for ordering events.
    log_seq: u64,
    /// Whether the permission request came back *granted*. Distinct from
    /// `perms`, which is also true after a denial: this is what says the gated
    /// host calls will actually be answered.
    granted: bool,
    /// Project names and roots persist across sessions.
    projects: projects::Registry,
    /// Todo lists persist across sessions, keyed on the project root folder
    /// rather than the project name, because a name moves and a folder does not.
    todos: todos::Store,
    /// Stable tab id to project name. Tab ids only live for this session.
    ///
    /// Only what the user assigned. The poll writes `auto_tab_projects`, so an
    /// assignment the user made can never be overwritten.
    tab_projects: BTreeMap<usize, String>,
    /// Tab id to project name, as the poll worked it out from the agents in the
    /// tab. Rebuilt on every poll. `tab_projects` wins over this map.
    auto_tab_projects: BTreeMap<usize, String>,
    /// The tabs as of the last `TabUpdate`.
    ///
    /// The host sends one per client, so with every client served by this one
    /// instance these are whoever switched tabs most recently — and so is
    /// `active_project`. Last writer wins, deliberately: everyone attached
    /// shares the one conversation, so they share its project too.
    tabs: Vec<TabInfo>,
    /// Derived from the active tab in `tabs`.
    active_project: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            perms: false,
            // Never rendered: render() short-circuits on `perms` until the real
            // resolution has run.
            cfg: Err(ConfigError {
                message: "Configuration has not been resolved yet.".to_owned(),
                base_url: api::DEFAULT_BASE_URL.to_owned(),
                model: api::DEFAULT_MODEL.to_owned(),
                api_key_env: api::DEFAULT_API_KEY_ENV.to_owned(),
            }),
            kdl: BTreeMap::new(),
            env: BTreeMap::new(),
            messages: Vec::new(),
            input: TextInput::empty(),
            status: Status::Idle,
            stream: None,
            reply_at: None,
            sse: SseParser::default(),
            tick: 0,
            timer_armed: false,
            tool_calls: ToolCallAccumulator::default(),
            pending: VecDeque::new(),
            tool_rounds: 0,
            owes_reply: false,
            stream_seq: 0,
            scroll: BTreeMap::new(),
            // Overwritten by the host on the first event or render; nothing
            // reads it before then.
            client: 0,
            transcript_rows: 0,
            transcript_cache: None,
            chat_id: None,
            saved: (0, 0),
            parked: Vec::new(),
            picker: None,
            jobs: Vec::new(),
            job_seq: 0,
            job_auto_confirm: false,
            panes: Vec::new(),
            manual_kinds: BTreeMap::new(),
            activity: BTreeMap::new(),
            agent_due_in: 0.0,
            poll_pending: false,
            polls_unanswered: 0,
            poll_dead: false,
            mono_ms: 0,
            log_seq: 0,
            granted: false,
            projects: BTreeMap::new(),
            todos: BTreeMap::new(),
            tab_projects: BTreeMap::new(),
            auto_tab_projects: BTreeMap::new(),
            tabs: Vec::new(),
            active_project: None,
        }
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.kdl = configuration;
        self.input.set_history(history::load_prompt_history());
        self.projects = projects::load();
        self.todos = todos::load();
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            // A bracketed paste never reaches us as keys: the host buffers the
            // bytes between the markers and hands them over whole, and drops
            // them entirely if we are not subscribed here.
            EventType::PastedText,
            EventType::WebRequestResult,
            EventType::WebRequestStreamChunk,
            EventType::RunCommandResult,
            EventType::Timer,
            EventType::PermissionRequestResult,
            EventType::BeforeClose,
            // A running plugin's KDL block is otherwise frozen at server start:
            // even an explicit reload replays the configuration captured then,
            // so without this a config fix needs a whole Zellij restart.
            EventType::PluginConfigurationChanged,
            // Worker replies. Not subscribing means they are silently dropped.
            EventType::CustomMessage,
            EventType::TabUpdate,
        ]);
        set_selectable(true);
        set_self_mouse_selection_support(true);

        // Nothing permission-gated may be called here. The host drops a denied
        // command and writes nothing back, so a value-returning call such as
        // get_session_environment_variables() would leave us blocked on a read
        // that never completes. Config resolution waits for the grant.
        request_permission(&[
            PermissionType::WebAccess,
            PermissionType::ReadSessionEnvironmentVariables,
            // Tools: query the session, mutate it, open panes, run commands,
            // read pane output, and send pane input, in that order.
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::RunCommands,
            PermissionType::ReadPaneContents,
            PermissionType::WriteToStdin,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        // The host directs each event at the client that caused it, so this is
        // who pressed the key, and who a scroll moves.
        self.client = get_plugin_ids().client_id;
        let render = self.handle(event);
        // Archived here rather than from each outcome path, so a finished reply,
        // a cancel, an error and an aborted tool run are all covered without any
        // of them having to remember to.
        self.autosave();
        render
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // Called once per attached client, each pass with a different client
        // set here. Only `scroll` differs between them; everything else on
        // screen is the one conversation everyone shares.
        self.client = get_plugin_ids().client_id;
        ui::render(self, rows, cols);
    }
}

impl State {
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => self.on_permissions(status),
            Event::Key(key) => self.on_key(key),
            Event::PastedText(text) => self.on_paste(text),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            // The three events a *parked* chat can still be owed. Each carries
            // the id of what it answers, which is what says whose it is; the
            // handlers themselves never learn a chat was off-screen.
            Event::WebRequestResult(status, _headers, body, context) => {
                match self.owner_of(&context) {
                    Some(index) => {
                        self.with_parked(index, |state| state.on_response(status, &body, &context))
                    }
                    None => self.on_response(status, &body, &context),
                }
            }
            Event::WebRequestStreamChunk {
                stream_id,
                chunk,
                done,
                error,
            } => match self.parked_with_stream(&stream_id) {
                Some(index) => self.with_parked(index, |state| {
                    state.on_chunk(&stream_id, &chunk, done, error)
                }),
                None => self.on_chunk(&stream_id, &chunk, done, error),
            },
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                match self.owner_of(&context) {
                    Some(index) => self.with_parked(index, |state| {
                        state.on_command_result(exit_code, &stdout, &stderr, &context)
                    }),
                    None => self.on_command_result(exit_code, &stdout, &stderr, &context),
                }
            }
            // Host state, not conversation state: applied to the foreground
            // chat's config only, never routed to a parked chat.
            Event::PluginConfigurationChanged(kdl) => {
                self.kdl = kdl;
                self.reconfigure();
                true
            }
            // The payload is the real time the host slept, which is the only
            // clock a plugin gets — there is no wall clock on offer.
            Event::Timer(elapsed) => self.on_timer(elapsed),
            Event::CustomMessage(message, payload) => self.on_custom_message(&message, &payload),
            Event::TabUpdate(tabs) => self.on_tabs(tabs),
            Event::BeforeClose => {
                self.cancel_in_flight();
                if let Some(json) = history::prompt_history_json(self.input.history()) {
                    history::write_prompt_history(json.as_bytes());
                }
                if let Some(json) = projects::json(&self.projects) {
                    projects::write(json.as_bytes());
                }
                if let Some(json) = todos::json(&self.todos) {
                    todos::write(json.as_bytes());
                }
                // The worker gets no flush on exit; write the transcript here
                // or lose the last exchange. Background chats have the same one
                // chance, and their partial replies are worth no less.
                self.flush_save();
                for chat in std::mem::take(&mut self.parked) {
                    if let Some(stream) = &chat.stream {
                        cancel_web_request(&stream.id);
                    }
                    let mut id = chat.chat_id;
                    if let Some((id, json)) = history::save(&mut id, &chat.messages) {
                        history::write(id, json.as_bytes());
                    }
                }
                false
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Workers — separate wasm instances of this same binary, driven only by
// post_message_to. They share the /cache preopen but no memory with the plugin.
// ---------------------------------------------------------------------------

/// Fire-and-forget file writes, off the plugin's `update()` path. One worker
/// with a FIFO mailbox, so log lines and saves land in post order. No replies.
#[derive(Default, Serialize, Deserialize)]
struct IoWorker;

impl ZellijWorker<'_> for IoWorker {
    fn on_message(&mut self, message: String, payload: String) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) else {
            return;
        };
        match message.as_str() {
            "log" => log::write_line(v["chat_id"].as_u64(), v["line"].as_str().unwrap_or_default()),
            "save" => {
                if let (Some(id), Some(json)) = (v["id"].as_u64(), v["json"].as_str()) {
                    history::write(id, json.as_bytes());
                }
            }
            "prompt_history" => {
                if let Some(json) = v["json"].as_str() {
                    history::write_prompt_history(json.as_bytes());
                }
            }
            "projects" => {
                if let Some(json) = v["json"].as_str() {
                    projects::write(json.as_bytes());
                }
            }
            "todos" => {
                if let Some(json) = v["json"].as_str() {
                    todos::write(json.as_bytes());
                }
            }
            _ => {}
        }
    }
}

register_worker!(IoWorker, io_worker, IO_WORKER);

/// Queue a transcript write on the io worker (addressed as "io" — the host owns
/// the `_worker` suffix).
fn post_save(id: u64, json: String) {
    post_message_to(PluginMessage::new_to_worker(
        "io",
        "save",
        &serde_json::json!({ "id": id, "json": json }).to_string(),
    ));
}

fn post_prompt_history(entries: &[String]) {
    let Some(json) = history::prompt_history_json(entries) else {
        return;
    };
    post_message_to(PluginMessage::new_to_worker(
        "io",
        "prompt_history",
        &serde_json::json!({ "json": json }).to_string(),
    ));
}

fn post_projects(projects: &projects::Registry) {
    let Some(json) = projects::json(projects) else {
        return;
    };
    post_message_to(PluginMessage::new_to_worker(
        "io",
        "projects",
        &serde_json::json!({ "json": json }).to_string(),
    ));
}

fn post_todos(todos: &todos::Store) {
    let Some(json) = todos::json(todos) else {
        return;
    };
    post_message_to(PluginMessage::new_to_worker(
        "io",
        "todos",
        &serde_json::json!({ "json": json }).to_string(),
    ));
}

/// Queue a JSONL append on the io worker.
fn post_log(chat_id: Option<u64>, mono_ms: u64, seq: u64, kind: &str, payload: serde_json::Value) {
    if let Some(line) = log::line(chat_id, mono_ms, seq, kind, payload) {
        post_message_to(PluginMessage::new_to_worker(
            "io",
            "log",
            &serde_json::json!({ "chat_id": chat_id, "line": line }).to_string(),
        ));
    }
}

/// The terminal panes of a manifest, as the agent module wants them.
///
/// Plugin panes are dropped here rather than checked later: an agent is a program at a
/// prompt, and Hocus itself is a plugin pane. Suppressed panes are dropped for the same
/// reason `list_session` drops them — the user cannot see them. Global panes are in no
/// tab, so they are in their own field and would be missed by the tab walk alone.
fn terminal_panes(manifest: &PaneManifest, tabs: &[TabInfo]) -> Vec<agents::AgentPane> {
    // The manifest is keyed by tab *position*, not tab id — same join as
    // tools::list_session.
    let by_position: BTreeMap<usize, (usize, &str)> = tabs
        .iter()
        .map(|tab| (tab.position, (tab.tab_id, tab.name.as_str())))
        .collect();
    let mut panes: Vec<agents::AgentPane> = manifest
        .panes
        .iter()
        .flat_map(|(position, panes)| panes.iter().map(move |pane| (Some(*position), pane)))
        .chain(manifest.global_panes.iter().map(|pane| (None, pane)))
        .filter(|(_, pane)| !pane.is_plugin && !pane.is_suppressed)
        .map(|(position, pane)| agents::AgentPane {
            pane_id: pane.id,
            title: pane.title.clone(),
            command: pane.terminal_command.clone(),
            cwd: None,
            // Order matters: a held pane has also exited, and "waiting to be re-run"
            // is the more useful of the two things to say.
            status: if pane.is_held {
                agents::AgentStatus::Held
            } else if pane.exited {
                agents::AgentStatus::Exited
            } else {
                agents::AgentStatus::Running
            },
            tab: position
                .and_then(|position| by_position.get(&position))
                .map(|(id, name)| (*id, (*name).to_owned())),
        })
        .collect();
    // The manifest is a HashMap keyed by tab position, so its order is not stable.
    // agent_list is read by a model on every turn; give it the same order every time.
    panes.sort_by_key(|pane| pane.pane_id);
    panes
}

/// What a terminal pane is showing, routed through the existing `read_pane` rather
/// than a second host call, so there is only one implementation of "read a pane" to
/// get wrong. Used by `agent_list` previews and the busy/idle poll.
fn read_pane_text(pane_id: u32) -> String {
    let call = ToolCall {
        id: String::new(),
        name: "read_pane".to_owned(),
        arguments: serde_json::json!({
            "pane_id": pane_id,
            "is_plugin": false,
            "full_scrollback": false,
        })
        .to_string(),
    };
    match tools::dispatch(&call) {
        Dispatch::Done(text) => text,
        Dispatch::Deferred => "pane read cannot be deferred".to_owned(),
    }
}

/// One agent poll, host side: what goes to the worker and what comes back. The
/// activity map rides along both ways so the plugin's copy stays the only truth —
/// the worker keeps no state of its own.
#[derive(Serialize, Deserialize)]
struct PollRequest {
    manual_kinds: BTreeMap<u32, String>,
    activity: agents::Activity,
}

#[derive(Serialize, Deserialize)]
struct PollReply {
    /// `None` means no current session — a quiet reply, not a dead worker.
    panes: Option<Vec<agents::AgentPane>>,
    activity: agents::Activity,
}

/// The host work of one agent poll: fresh pane snapshot, the running command of
/// each pane, and a scrollback hash per running agent for busy/idle. A free
/// function so the poll worker and the synchronous in-plugin poll are the same
/// code. O(panes) blocking host calls — the reason the worker exists.
///
/// Pulled, not pushed. `Event::PaneUpdate` goes only to plugins of the active tab
/// and to background plugins, so a Hocus launched into a global pane — which is in
/// no tab — receives it never, and every agent tool saw an empty session.
///
/// The manifest carries `terminal_command`, which is what a pane was *launched*
/// with and so is `None` for every pane the user opened as a shell. Detection
/// needs what is running now, which is a second call per pane.
fn gather_poll(
    manual_kinds: &BTreeMap<u32, String>,
    activity: &mut agents::Activity,
) -> Option<Vec<agents::AgentPane>> {
    let session = tools::current_session()?;
    let mut panes = terminal_panes(&session.panes, &session.tabs);
    // ponytail: one blocking host round-trip per terminal pane, every refresh.
    // Fine at a dozen panes; batch it if a session ever gets big enough to feel.
    for pane in &mut panes {
        if let Some(command) = tools::pane_running_command(pane.pane_id) {
            pane.command = Some(command);
        }
        pane.cwd = tools::pane_cwd(pane.pane_id);
    }
    // A pane that closed took its observations with it.
    activity.retain(|id, _| panes.iter().any(|pane| pane.pane_id == *id));
    // ponytail: one read_pane per running agent per poll, to tell busy from
    // idle. Scales with agents, not panes; batch with the refresh if it feels.
    // An agent whose TUI repaints a spinner every frame never reads idle —
    // false "busy" is the safe direction.
    let running: Vec<u32> = panes
        .iter()
        .filter(|pane| {
            pane.status == agents::AgentStatus::Running
                && agents::classify(pane, manual_kinds).is_some()
        })
        .map(|pane| pane.pane_id)
        .collect();
    for pane_id in running {
        agents::observe(activity, pane_id, &read_pane_text(pane_id));
    }
    Some(panes)
}

/// The agent poll, off the plugin's `update()` path. Stateless: everything it
/// needs arrives in the request and everything it learned goes back in the reply.
#[derive(Default, Serialize, Deserialize)]
struct PollWorker;

impl ZellijWorker<'_> for PollWorker {
    fn on_message(&mut self, _message: String, payload: String) {
        let Ok(mut request) = serde_json::from_str::<PollRequest>(&payload) else {
            return;
        };
        let panes = gather_poll(&request.manual_kinds, &mut request.activity);
        // Always reply — a poke with no answer is how the plugin decides the
        // worker is dead.
        let reply = PollReply {
            panes,
            activity: request.activity,
        };
        if let Ok(json) = serde_json::to_string(&reply) {
            post_message_to_plugin(PluginMessage::new_to_plugin("poll_result", &json));
        }
    }
}

register_worker!(PollWorker, poll_worker, POLL_WORKER);

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

impl State {
    fn is_toggle_key(key: &KeyWithModifier) -> bool {
        key.bare_key == BareKey::Char('`') && key.has_modifiers(&[KeyModifier::Ctrl])
    }

    /// Re-resolve the config after Zellij reports the KDL block changed.
    ///
    /// Gated on `self.granted` — before the grant `self.env` is empty, and
    /// resolving against it would replace a "waiting for permission" state with
    /// a spurious "no API key" banner.
    fn reconfigure(&mut self) {
        if !self.granted {
            return;
        }
        self.cfg = Config::resolve(&self.kdl, &self.env);
    }

    /// Hocus is written for a *shared* plugin: one instance serving every
    /// attached client. Zellij only does that when the KDL block asks for it,
    /// and silently gives each client its own instance otherwise — which looks
    /// fine alone and splits into unrelated conversations the moment someone
    /// else attaches. Say so rather than let that be discovered.
    ///
    /// `Config::resolve` ignores the key, so this is the only thing reading it.
    fn warn_if_not_shared(&mut self) {
        if self.kdl.get("shared").map(|v| v.trim()) == Some("true") {
            return;
        }
        self.messages.push(Message::notice(
            "hocus is missing `shared true` in its plugin configuration. Without it Zellij \
             gives every attached client its own copy of the plugin, and they will not share \
             a conversation."
                .to_owned(),
        ));
    }

    /// This client's scroll position: wrapped transcript lines hidden below the
    /// viewport, zero being pinned to the bottom.
    pub fn scroll_offset(&self) -> usize {
        self.scroll.get(&self.client).copied().unwrap_or(0)
    }

    /// Pin *every* client back to the tail. The conversation changing is not
    /// one client's business, so nobody is left reading a stale middle of it.
    fn pin_to_bottom(&mut self) {
        self.scroll.clear();
    }

    fn on_permissions(&mut self, status: PermissionStatus) -> bool {
        // Either way we stop saying "waiting": this is the only answer we get.
        self.perms = true;
        self.cfg = match status {
            // Safe now — gated calls will actually be answered.
            PermissionStatus::Granted => {
                self.granted = true;
                self.warn_if_not_shared();
                // The agent poll needs `ReadApplicationState`, so this grant is the
                // earliest it can start — and it has to be started from somewhere,
                // or no notice lands until the first keystroke.
                self.arm_timer(AGENT_SECONDS);
                self.env = get_session_environment_variables();
                Config::resolve(&self.kdl, &self.env)
            }
            PermissionStatus::Denied => Err(ConfigError {
                message: "Permission denied.\n\nhocus needs web access (to reach the API) and \
                          read access to session environment variables (to find your API key). \
                          Grants are cached by the plugin's absolute .wasm path — delete this \
                          plugin's entry from permissions.kdl in Zellij's cache directory and \
                          reload to be asked again."
                    .to_owned(),
                base_url: api::DEFAULT_BASE_URL.to_owned(),
                model: api::DEFAULT_MODEL.to_owned(),
                api_key_env: api::DEFAULT_API_KEY_ENV.to_owned(),
            }),
        };
        true
    }

    fn on_tabs(&mut self, tabs: Vec<TabInfo>) -> bool {
        self.tab_projects
            .retain(|id, _| tabs.iter().any(|tab| tab.tab_id == *id));
        self.auto_tab_projects
            .retain(|id, _| tabs.iter().any(|tab| tab.tab_id == *id));
        self.active_project = tabs
            .iter()
            .find(|tab| tab.active)
            .and_then(|tab| self.tab_project(tab.tab_id))
            .map(str::to_owned);
        self.tabs = tabs;
        false
    }

    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if Self::is_toggle_key(&key) {
            hide_self();
            return false;
        }

        // The archive is modal, so it takes every key before anything else can.
        if self.picker.is_some() {
            return self.picker_key(key);
        }

        // These must be caught before TextInput sees them. Its bare-key arm
        // doesn't check for modifiers, so it would take Ctrl-l as a literal 'l'
        // and Alt-Enter as a submit.
        if key.bare_key == BareKey::Char('l') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            return self.new_chat();
        }
        if key.bare_key == BareKey::Char('r') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            return self.open_agent_picker();
        }
        // The confirmation prompt owns y/n outright, for the same reason: they
        // would otherwise be typed into the input box.
        if matches!(self.status, Status::ConfirmingTool) && key.has_no_modifiers() {
            match key.bare_key {
                BareKey::Char('y') | BareKey::Char('Y') => return self.confirm_tool(),
                BareKey::Char('n') | BareKey::Char('N') => return self.decline_tool(),
                _ => {}
            }
        }
        if key.bare_key == BareKey::Enter
            && (key.has_modifiers(&[KeyModifier::Alt]) || key.has_modifiers(&[KeyModifier::Shift]))
        {
            self.input.insert_char('\n');
            return true;
        }

        // History owns bare Up/Down. Page keys and the mouse still scroll.
        if key.has_no_modifiers() {
            let page = self.transcript_rows.saturating_sub(1).max(1);
            match key.bare_key {
                BareKey::PageUp => return self.scroll_up(page),
                BareKey::PageDown => return self.scroll_down(page),
                _ => {}
            }
        }

        match self.input.handle_key(key) {
            InputAction::Submit => self.submit(),
            // Tools first, because they are the nearer thing: a stream and a
            // tool run can now both be outstanding, and the tool is the one the
            // prompt is asking about.
            InputAction::Cancel if self.tools_busy() => self.abort_tools(),
            // One instance holds every stream, so whoever presses Esc can
            // cancel whatever is arriving, no matter who started it.
            InputAction::Cancel if self.is_streaming() => self.cancel_stream(),
            InputAction::Cancel => {
                // Hide the session-global sidebar without unloading the plugin.
                // The launch binding restores the same plugin from any tab.
                // Ctrl-l starts a new archived conversation.
                hide_self();
                false
            }
            InputAction::Continue => true,
            InputAction::Complete => self.complete_tool_name(),
            InputAction::NoAction => false,
        }
    }

    /// A bracketed paste, delivered whole rather than key by key. Newlines in
    /// it stay newlines, exactly as Alt-Enter produces them; a paste never
    /// submits, however it ends.
    fn on_paste(&mut self, text: String) -> bool {
        // The states that already take every key before the input box sees it
        // refuse a paste for the same reason.
        if self.picker.is_some() || matches!(self.status, Status::ConfirmingTool) {
            return false;
        }
        self.input.insert_text(&text);
        true
    }

    /// Tab on a half-typed `/name` or `@agent`. One match completes it, several
    /// are listed, and anything else is left alone — Tab is not a key the input
    /// box has any other use for.
    fn complete_tool_name(&mut self) -> bool {
        let text = self.input.get_text().trim().to_owned();
        let Some(prefix) = text.strip_prefix('/').filter(|rest| !rest.contains(' ')) else {
            return self.complete_mention();
        };
        let matches: Vec<String> = LOCAL_COMMANDS
            .iter()
            .map(|name| (*name).to_owned())
            .chain(tools::names())
            .filter(|name| name.starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [name] => self.input.set_text(format!("/{name} ")),
            [] => return false,
            _ => {
                let notice = format!("/{}", matches.join("  /"));
                self.list_completions(notice);
            }
        }
        true
    }

    /// Tab with the cursor at the end of an `@token`, anywhere in the message.
    /// Candidates are the agents on screen right now; a bare `@` lists them all.
    fn complete_mention(&mut self) -> bool {
        let text = self.input.get_text().to_owned();
        let cursor = self.input.get_cursor_position();
        let head: String = text.chars().take(cursor).collect();
        let token = head.rsplit(char::is_whitespace).next().unwrap_or("");
        let Some(prefix) = token.strip_prefix('@') else {
            return false;
        };
        let matches: Vec<String> =
            agents::mention_candidates(&self.panes, &self.manual_kinds)
                .into_iter()
                .filter(|candidate| candidate.starts_with(prefix))
                .collect();
        match matches.as_slice() {
            [] => false,
            [only] => {
                // `token` is a suffix slice of `head`, so byte arithmetic is safe;
                // the cursor wants chars.
                let stem = &head[..head.len() - token.len()];
                let spliced = format!("{stem}@{only} ");
                let cursor_chars = spliced.chars().count();
                let tail: String = text.chars().skip(cursor).collect();
                self.input.set_text(format!("{spliced}{tail}"));
                self.input.set_cursor_position(cursor_chars);
                true
            }
            _ => {
                self.list_completions(format!("@{}", matches.join("  @")));
                true
            }
        }
    }

    /// The several-matches answer to Tab: the candidates as a notice line.
    fn list_completions(&mut self, notice: String) {
        self.messages.push(Message::notice(notice.clone()));
        self.log(
            "notice",
            serde_json::json!({"content": log::truncate(&notice)}),
        );
        self.pin_to_bottom();
    }

    fn on_mouse(&mut self, mouse: Mouse) -> bool {
        match mouse {
            Mouse::ScrollUp(lines) => self.scroll_up(lines.max(1)),
            Mouse::ScrollDown(lines) => self.scroll_down(lines.max(1)),
            _ => false,
        }
    }

    fn submit(&mut self) -> bool {
        // Ahead of every guard, including the config: a slash command drives the
        // plugin rather than the conversation, so neither an API key nor a
        // settled reply is any of its business. `run_manual` turns away the one
        // kind that does conflict — another tool already running.
        if self.input.get_text().trim_start().starts_with('/') {
            let line = self.drain_input();
            return self.run_manual(&line);
        }
        // One reply per chat: a second question would have nowhere to stream.
        // /history opens another chat to ask it in.
        if self.is_in_flight() {
            return false;
        }
        // Cloned rather than borrowed so the transcript can be mutated below.
        let Ok(cfg) = self.cfg.clone() else {
            return false;
        };

        let mut text = self.drain_input();
        if text.trim().is_empty() {
            // Whitespace-only: the input was still drained, so redraw.
            return true;
        }
        // An @-mention earns the model a context line naming the agent pane, so
        // "ask @codex to run the tests" needs no agent_list round first.
        if let Some(context) =
            agents::mention_context(&text, &self.panes, &self.manual_kinds, &self.activity)
        {
            text.push_str("\n\n");
            text.push_str(&context);
        }

        let content = text.clone();
        self.job_auto_confirm = false;
        self.messages.push(Message::user(text));
        self.log(
            "user_input",
            serde_json::json!({"content": log::truncate(&content)}),
        );
        // A new question starts a fresh tool budget.
        self.tool_rounds = 0;
        self.start_request(&cfg);
        true
    }

    fn drain_input(&mut self) -> String {
        let old_len = self.input.history().len();
        let text = self.input.drain_text();
        if self.input.history().len() != old_len {
            post_prompt_history(self.input.history());
        }
        text
    }

    /// Send the transcript as it currently stands. Used both for a new user
    /// message and for the follow-up after tool results, which differ only in
    /// what was appended before the call.
    fn start_request(&mut self, cfg: &Config) {
        // The last round of the budget goes out without tools, so the model has
        // to answer from what it already collected instead of dead-ending on the
        // cap with nothing to show the user.
        let allow_tools = self.tool_rounds < MAX_TOOL_ROUNDS;
        if !allow_tools {
            self.messages
                .push(Message::notice("tool budget spent — answering without tools"));
        }
        // The placeholder streamed text appends into. request_body skips empty
        // assistant messages, so it never reaches the wire.
        self.reply_at = Some(self.messages.len());
        self.messages.push(Message::assistant(""));

        self.stream_seq += 1;
        let stream_id = format!("hocus-{}", self.stream_seq);
        self.sse = SseParser::default();
        self.tool_calls = ToolCallAccumulator::default();

        let project = self
            .active_project
            .as_deref()
            .and_then(|name| self.projects.get(name))
            .map(|project| (project.name.as_str(), project.root.as_str()));
        let body = api::request_body_with_project(cfg, &self.messages, allow_tools, project);
        let body_len = body.len();
        self.log(
            "api_request",
            serde_json::json!({
                "model": cfg.model,
                "base_url": cfg.base_url,
                "stream_id": stream_id,
                "message_count": self.messages.len(),
                "body_bytes": body_len,
                "allow_tools": allow_tools,
            }),
        );

        web_request_stream(
            &stream_id,
            &cfg.url,
            HttpVerb::Post,
            api::request_headers(cfg),
            body,
            // Round-trips through the host, like the `models` tag. The head of a
            // stream is a `WebRequestResult` carrying no stream id of its own,
            // so without this there is no telling which chat a non-2xx belongs
            // to once more than one is in flight.
            BTreeMap::from([(STREAM_CONTEXT.to_owned(), stream_id.clone())]),
        );

        self.stream = Some(StreamState {
            id: stream_id,
            finishing: false,
        });
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
    }

    /// The single `WebRequestResult` that leads every stream — and, when the
    /// context says so, the whole answer to a `web_search` or a model list.
    fn on_response(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if self.finish_search(status, body, context) {
            return true;
        }
        if self.finish_models(status, body, context) {
            return true;
        }
        // Someone else's stream, or one we already cancelled and forgot.
        let Some(stream_id) = context.get(STREAM_CONTEXT).cloned() else {
            return false;
        };
        if self.active_stream_id() != Some(stream_id.clone()) {
            return false;
        }
        if (200..300).contains(&status) {
            // A 2xx carries only status and headers; the body arrives as chunks.
            return false;
        }

        // Non-2xx: the entire error body is here and no chunks will follow.
        self.pop_empty_placeholder();
        let err = api::api_error_message(status, body);
        self.messages.push(Message::error(err.clone()));
        self.log(
            "api_error",
            serde_json::json!({
                "stream_id": stream_id.clone(),
                "status": status,
                "error": log::truncate(&err),
            }),
        );
        self.job_auto_confirm = false;
        self.stream = Some(StreamState {
            id: stream_id,
            finishing: true,
        });
        self.pin_to_bottom();
        true
    }

    fn on_chunk(
        &mut self,
        stream_id: &str,
        chunk: &[u8],
        done: bool,
        error: Option<String>,
    ) -> bool {
        // A stream we superseded, cancelled and forgot, or never started.
        if self.active_stream_id().as_deref() != Some(stream_id) {
            return false;
        }
        if !self.is_receiving() {
            if done {
                self.stream = None;
                self.reply_at = None;
                self.tool_calls.take();
                self.job_auto_confirm = false;
            }
            return done;
        }

        let items = self.sse.feed(chunk);
        for item in items {
            match item {
                SseItem::Delta(text) => {
                    self.append_to_reply(&text);
                }
                SseItem::ApiError(message) => {
                    let m = message.clone();
                    self.pop_empty_placeholder();
                    self.messages.push(Message::error(message));
                    self.log(
                        "api_error",
                        serde_json::json!({
                            "stream_id": stream_id,
                            "error": log::truncate(&m),
                        }),
                    );
                    self.tool_calls.take();
                    self.job_auto_confirm = false;
                    self.stream = Some(StreamState {
                        id: stream_id.to_owned(),
                        finishing: true,
                    });
                    break;
                }
                SseItem::ToolCallFragment { .. } => self.tool_calls.feed(&item),
                // Informational: `done: true` below is the authoritative end.
                SseItem::Done => {}
            }
        }

        if done {
            let settled = self.stream.take();
            match settled {
                // Outcome already recorded; this is the terminator we were
                // waiting to swallow. Any half-accumulated calls go with it.
                Some(stream) if stream.finishing => {
                    self.tool_calls.take();
                }
                _ => {
                    let calls = self.tool_calls.take();
                    if let Some(message) = error {
                        let m = message.clone();
                        self.pop_empty_placeholder();
                        self.messages.push(Message::error(message));
                        self.log(
                            "api_error",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "error": log::truncate(&m),
                            }),
                        );
                    } else if !calls.is_empty() {
                        // The model asked for tools rather than answering; the
                        // reply placeholder is not the turn's outcome.
                        // Capture any streamed text before we replace the
                        // placeholder with the tool_calls summary.
                        let prior_text = self
                            .reply()
                            .map(|message| message.content.clone())
                            .unwrap_or_default();
                        self.pop_empty_placeholder();
                        self.messages.push(Message::tool_calls(calls.clone()));
                        let tool_payload: Vec<serde_json::Value> = calls
                            .iter()
                            .map(|c| {
                                serde_json::json!({
                                    "id": c.id,
                                    "name": c.name,
                                    "arguments": log::truncate(&c.arguments),
                                })
                            })
                            .collect();
                        self.log(
                            "tool_call",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "tool_calls": tool_payload,
                            }),
                        );
                        if !prior_text.is_empty() {
                            self.log(
                                "assistant_message",
                                serde_json::json!({
                                    "stream_id": stream_id,
                                    "content": log::truncate(&prior_text),
                                }),
                            );
                        }
                        // Extended, not replaced: a tool the user typed
                        // mid-stream may still be running, and its result is
                        // owed to the transcript before these go out. Pumping is
                        // then that call's job — starting a second one here
                        // would run two tools at once.
                        //
                        // ponytail: messages land in arrival order, so a typed
                        // call still running when these arrive has its result
                        // recorded *after* this summary — each result still
                        // follows the call that asked for it, but the two pairs
                        // interleave. Endpoints that want a result immediately
                        // after its call would need the transcript reordered on
                        // the way out in `api::request_body`.
                        self.pending.extend(calls);
                        self.owes_reply = true;
                        if !self.tools_busy() {
                            self.pump_tools();
                        }
                    } else if self.reply_is_empty() {
                        self.pop_empty_placeholder();
                        self.messages
                            .push(Message::error("empty response from endpoint"));
                        self.log(
                            "api_error",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "error": "empty response from endpoint",
                            }),
                        );
                    } else {
                        // Successful assistant reply.
                        let reply = self
                            .reply()
                            .map(|message| message.content.clone())
                            .unwrap_or_default();
                        self.log(
                            "assistant_message",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "content": log::truncate(&reply),
                            }),
                        );
                        self.log(
                            "api_response",
                            serde_json::json!({
                                "stream_id": stream_id,
                                "status": "done",
                            }),
                        );
                    }
                }
            }
            // The reply is settled, so nothing is appending any more — whatever
            // a typed tool pushes from here on is its own message.
            self.reply_at = None;
            if !self.is_in_flight() {
                self.job_auto_confirm = false;
            }
        }
        // Only the terminal chunk renders. Mid-stream chunks arrive once per
        // socket read (~50/s), and every plugin render costs the host a full
        // screen composite on the same thread that services every other Zellij
        // action — rendering per chunk freezes the whole session for the length
        // of the reply. The spinner timer repaints the arriving text instead.
        done
    }

    /// The message for a Slack tool whose `dobbs` could not be executed, or
    /// `None` when this is any other kind of result.
    ///
    /// Matched on the stderr string rather than the code alone: Zellij uses
    /// exit code 2 both for a failure to spawn and, unavoidably, for a command
    /// that ran and exited 2 by itself.
    fn slack_exec_hint(&self, exit_code: Option<i32>, stderr: &[u8]) -> Option<String> {
        let head = self.pending.front()?;
        if !tools::SLACK_TOOLS.contains(&head.name.as_str()) || exit_code != Some(2) {
            return None;
        }
        let stderr = String::from_utf8_lossy(stderr);
        if !stderr.contains("os error 2") {
            return None;
        }
        let dobbs = self.cfg.as_ref().ok().map(|cfg| cfg.dobbs.as_str())?;
        Some(format!(
            "dobbs could not be run: \"{dobbs}\" does not exist or is not executable.\n\n\
             Set dobbs_path in the hocus plugin block of ~/.config/zellij/config.kdl to the \
             binary's absolute path — a leading ~/ is expanded, but nothing else is. Tell the \
             user this; do not try other commands to find dobbs yourself."
        ))
    }

    /// The deferred half of `run_command`. The context map round-tripped through
    /// the host, so it names the call this answers.
    ///
    /// Each stream is capped at `MAX_TOOL_OUTPUT`: a tool result stays in the
    /// transcript, so it is re-sent and re-billed on every later turn of the
    /// conversation. One `slack_read` of a busy channel would otherwise make
    /// every subsequent message cost a full transcript.
    fn on_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !matches!(self.status, Status::AwaitingCommand) {
            return false;
        }
        // A result from a run we already aborted, or from someone else's call.
        let Some(head) = self.pending.front() else {
            return false;
        };
        if context.get("tool_call_id") != Some(&head.id) {
            return false;
        }

        // Zellij reports a command it could not even start as exit code 2 with
        // the raw io::Error as the whole of stderr — "No such file or directory
        // (os error 2)", naming nothing. For a Slack tool the model never sees
        // the argv, so that message is undiagnosable from inside the
        // conversation: say which binary, and which setting names it.
        let result = match self.slack_exec_hint(exit_code, stderr) {
            Some(hint) => hint,
            None => {
                let mut result = format!("exit code: {}", exit_code.unwrap_or(-1));
                for (label, bytes) in [("stdout", stdout), ("stderr", stderr)] {
                    let text = String::from_utf8_lossy(bytes);
                    if !text.trim().is_empty() {
                        let text = log::truncate_str(text.trim_end(), MAX_TOOL_OUTPUT);
                        result.push_str(&format!("\n\n{label}:\n{text}"));
                    }
                }
                result
            }
        };

        self.finish_head(result);
        self.pump_tools();
        true
    }
}

// ---------------------------------------------------------------------------
// The tool loop
// ---------------------------------------------------------------------------

impl State {
    /// Work through `pending` until something has to wait — a confirmation, a
    /// command result — or the queue empties and the results go back to the
    /// model.
    fn pump_tools(&mut self) {
        while let Some(call) = self.pending.front().cloned() {
            let grants_job_authority = call.name == "schedule"
                && tools::Args::parse(&call.arguments).is_ok_and(|args| args.auto_confirm);
            if (grants_job_authority || tools::needs_confirmation(&call.name))
                && !self.job_auto_confirm
            {
                self.status = Status::ConfirmingTool;
                self.pin_to_bottom();
                return;
            }
            if tools::needs_confirmation(&call.name) {
                self.log(
                    "tool_confirm",
                    serde_json::json!({
                        "tool_call_id": call.id,
                        "tool_name": call.name,
                        "decision": "automatic",
                    }),
                );
            }
            if !self.run_head(&call) {
                return;
            }
        }

        // Only the user's own calls were in the queue: nothing to send.
        self.status = Status::Idle;
        if !std::mem::take(&mut self.owes_reply) {
            return;
        }

        // Every call answered: hand the results back so the model can use them.
        // The request at exactly MAX_TOOL_ROUNDS goes out without tools (see
        // `start_request`), so getting past it means the model called a tool it
        // was never offered. Nothing left to try.
        self.tool_rounds += 1;
        if self.tool_rounds > MAX_TOOL_ROUNDS {
            self.job_auto_confirm = false;
            let err =
                format!("stopped after {MAX_TOOL_ROUNDS} rounds of tool calls without a reply");
            self.messages.push(Message::error(err.clone()));
            self.log("error", serde_json::json!({"error": log::truncate(&err)}));
            return;
        }
        let Ok(cfg) = self.cfg.clone() else {
            self.job_auto_confirm = false;
            return;
        };
        self.start_request(&cfg);
    }

    /// Dispatch the head. Returns whether the queue advanced — `false` means the
    /// result is deferred and `pump_tools` must stop and wait.
    fn run_head(&mut self, call: &ToolCall) -> bool {
        // Needs the resolved config and defers on a web request, neither of
        // which tools::dispatch can reach.
        if call.name == "web_search" {
            return self.start_search(call);
        }
        // Same reason: the dobbs binary comes from the config.
        if tools::SLACK_TOOLS.contains(&call.name.as_str()) {
            return self.start_slack(call);
        }
        // The scheduling tools live here rather than in tools::dispatch, which
        // is a free function with no access to the state they mutate.
        if let Some(result) = self.run_local(call) {
            self.finish_head(result);
            return true;
        }
        match tools::dispatch(call) {
            Dispatch::Done(result) => {
                self.finish_head(result);
                true
            }
            // ponytail: no timeout — a command that never exits waits forever
            // and only Esc recovers. Add set_timeout + Event::Timer here if
            // hanging commands turn out to be common in practice.
            Dispatch::Deferred => {
                self.status = Status::AwaitingCommand;
                self.pin_to_bottom();
                self.arm_timer(FRAME_SECONDS);
                false
            }
        }
    }

    /// Run a tool the user typed as `/name key=value …`.
    ///
    /// Recorded as a real call-and-result pair rather than a notice, so the model
    /// sees what was done on the next message — and so the transcript stays valid
    /// for `api::request_body`, which sends every tool result and would otherwise
    /// be left with an orphan.
    ///
    /// No confirmation prompt: typing the command *is* the confirmation, which is
    /// why this calls `run_head` rather than going through `pump_tools`.
    fn run_manual(&mut self, line: &str) -> bool {
        self.pin_to_bottom();
        let line = line.trim();
        if line == "/" || line == "/help" {
            let help = format!(
                "/{}  /{}",
                LOCAL_COMMANDS.join("  /"),
                tools::names().join("  /")
            );
            self.messages.push(Message::notice(help.clone()));
            self.log(
                "notice",
                serde_json::json!({"content": log::truncate(&help)}),
            );
            return true;
        }
        // The plugin's own lists, not tools: the model is never offered these.
        if line == "/history" {
            self.show_chats();
            return true;
        }
        if line == "/models" {
            self.show_models();
            return true;
        }
        if line == "/stop_jobs" {
            return self.stop_jobs();
        }
        // A reply arriving is no reason to wait, but another *tool* is: the
        // queue runs one call at a time, and its head owns the deferred result
        // the host is about to deliver.
        if self.tools_busy() {
            let notice = "a tool is already running — <Esc> aborts it";
            self.messages.push(Message::notice(notice));
            self.log("notice", serde_json::json!({"content": notice}));
            return true;
        }
        let (name, arguments) = match tools::parse_command(line) {
            Ok(parsed) => parsed,
            Err(error) => {
                let e = error.clone();
                self.messages.push(Message::error(error));
                self.log("error", serde_json::json!({"error": log::truncate(&e)}));
                return true;
            }
        };

        let call = ToolCall {
            // Unique within the transcript: every manual call appends at least
            // the summary message below before the next one can be typed.
            id: format!("manual-{}", self.messages.len()),
            name: name.clone(),
            arguments: arguments.clone(),
        };
        self.messages.push(Message::tool_calls(vec![call.clone()]));
        self.log(
            "tool_call",
            serde_json::json!({
                "tool_call_id": call.id,
                "tool_name": call.name,
                "arguments": log::truncate(&call.arguments),
                "manual": true,
            }),
        );
        self.pending.push_back(call.clone());
        // Straight to `run_head` rather than through `pump_tools`: typing the
        // command is itself the confirmation, so a destructive one runs without
        // the y/n prompt the model's calls get.
        self.run_head(&call);
        true
    }

    /// Record a result for the head call and drop it from the queue.
    fn finish_head<S: Into<String>>(&mut self, result: S) {
        if let Some(call) = self.pending.pop_front() {
            let result_str = result.into();
            self.messages
                .push(Message::tool_result(&call.id, result_str.clone()));
            self.log(
                "tool_result",
                serde_json::json!({
                    "tool_call_id": call.id,
                    "tool_name": call.name,
                    "result": log::truncate(&result_str),
                }),
            );
            self.pin_to_bottom();
        }
    }

    /// Ask the configured SearxNG. Deferred like `run_command`: the answer
    /// arrives as the `WebRequestResult` carrying this call's id.
    ///
    /// ponytail: a plain `web_request` has no stream id, so there is nothing to
    /// cancel — `Esc` stops waiting and the late reply is dropped because the
    /// head no longer matches, exactly as for an aborted `run_command`. Move to
    /// `web_request_stream` + `cancel_web_request` if hung searches turn out to
    /// matter.
    fn start_search(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            }
        };
        let Some(query) = args.query.filter(|query| !query.trim().is_empty()) else {
            self.finish_head("query is required and must not be empty");
            return true;
        };
        // Cloned so the request can be issued without holding a borrow of self.
        let Ok(search_url) = self.cfg.as_ref().map(|cfg| cfg.search_url.clone()) else {
            self.finish_head("hocus is not configured");
            return true;
        };

        web_request(
            api::search_query_url(&search_url, &query),
            HttpVerb::Get,
            BTreeMap::from([("Accept".to_owned(), "application/json".to_owned())]),
            Vec::new(),
            // Round-trips through the host, which is how the result is matched
            // back to the call that made it.
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
        );
        self.status = Status::AwaitingSearch;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// Run one of the Slack tools by shelling out to dobbs. Deferred exactly
    /// like `run_command` — the result comes back as `RunCommandResult` carrying
    /// this call's id, so `on_command_result` needs no Slack-specific path.
    ///
    /// dobbs owns the Slack credentials and reaches its daemon over TCP, which a
    /// wasm plugin cannot do; the CLI is the whole integration.
    fn start_slack(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            }
        };
        let argv = match tools::slack_argv(&call.name, &args) {
            Ok(argv) => argv,
            Err(error) => {
                self.finish_head(error);
                return true;
            }
        };
        // Cloned so the command can be run without holding a borrow of self.
        let Ok((dobbs, env)) = self
            .cfg
            .as_ref()
            .map(|cfg| (cfg.dobbs.clone(), cfg.dobbs_env()))
        else {
            self.finish_head("hocus is not configured");
            return true;
        };

        let mut command = vec![dobbs];
        command.extend(argv);
        let borrowed: Vec<&str> = command.iter().map(String::as_str).collect();
        run_command_with_env_variables_and_cwd(
            &borrowed,
            env,
            // dobbs is cwd-independent: it finds its archive and daemon under
            // ~/.dobbs, so there is nothing to inherit from the focused pane.
            std::path::PathBuf::from("."),
            // Round-trips through the host, which is how the result is matched
            // back to the call that made it.
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
        );
        self.status = Status::AwaitingCommand;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// The deferred half of `web_search`. Returns whether this response was one:
    /// a `WebRequestResult` with no `tool_call_id` is the head of an API stream.
    fn finish_search(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !matches!(self.status, Status::AwaitingSearch) {
            return false;
        }
        // A search we already aborted, or someone else's request.
        let Some(head) = self.pending.front() else {
            return false;
        };
        if context.get("tool_call_id") != Some(&head.id) {
            return false;
        }

        let result = if (200..300).contains(&status) {
            api::search_results(body, api::SEARCH_RESULTS)
        } else {
            api::api_error_message(status, body)
        };

        self.status = Status::Idle;
        self.finish_head(result);
        self.pump_tools();
        true
    }

    /// The tools that mutate our own state rather than the session. `None`
    /// means the call belongs to `tools::dispatch`.
    fn run_local(&mut self, call: &ToolCall) -> Option<String> {
        if !matches!(
            call.name.as_str(),
            "schedule"
                | "unschedule"
                | "project_list"
                | "project_assign"
                | "project_open"
                | "todo_add"
                | "todo_list"
                | "todo_search"
                | "todo_done"
                | "agent_register"
                | "agent_list"
                | "agent_send"
                | "agent_control"
                | "agent_spawn"
        ) {
            return None;
        }
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => return Some(error),
        };
        // One refresh for all four agent tools, so no path can act on a snapshot the
        // session has moved past — and none can act on no snapshot at all.
        if call.name.starts_with("agent_") {
            // Through `poll_agents` rather than `refresh_panes` so a transition
            // is noticed in the transcript, not silently absorbed by the tool.
            // The redraw it reports is redundant here — the tool result forces
            // one anyway.
            self.poll_agents();
            self.agent_due_in = AGENT_SECONDS;
        }
        Some(match call.name.as_str() {
            "schedule" => self.schedule(&args),
            "unschedule" => self.unschedule(args.job_id),
            "project_list" => self.project_list(),
            "project_assign" => self.project_assign(&args),
            "project_open" => self.project_open(&args),
            "todo_add" => self.todo_add(&args),
            "todo_list" => self.todo_list(&args),
            "todo_search" => self.todo_search(&args),
            "todo_done" => self.todo_done(&args),
            "agent_register" => self.agent_register(&args),
            "agent_list" => self.agent_list(&args),
            "agent_spawn" => self.agent_spawn(&args),
            _ => self.agent_act(call, &args),
        })
    }

    fn remember_project(&mut self, args: &tools::Args) -> Result<projects::Project, String> {
        let name = args
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "name is required".to_owned())?;
        if let Some(root) = args.root.as_deref() {
            let project = projects::validate(name, root)?;
            // The user just gave this folder a better name than a poll could.
            if let Some(gone) = projects::adopt(&mut self.projects, project.clone()) {
                self.rename_project(&gone, &project.name);
            }
            post_projects(&self.projects);
            Ok(project)
        } else {
            self.projects
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown project {name}; root is required for a new project"))
        }
    }

    fn project_list(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .projects
            .values()
            .map(|project| {
                let ids = |assigned: &BTreeMap<usize, String>| {
                    let mut ids: Vec<usize> = assigned
                        .iter()
                        .filter_map(|(id, name)| (name == &project.name).then_some(*id))
                        .collect();
                    ids.sort_unstable();
                    ids
                };
                let auto_tab_ids = ids(&self.auto_tab_projects);
                let mut tab_ids = ids(&self.tab_projects);
                tab_ids.extend(auto_tab_ids.iter().copied());
                tab_ids.sort_unstable();
                tab_ids.dedup();
                serde_json::json!({
                    "name": project.name,
                    "root": project.root,
                    "discovered": project.discovered,
                    "tab_ids": tab_ids,
                    "auto_tab_ids": auto_tab_ids,
                })
            })
            .collect();
        serde_json::json!({ "active": self.active_project, "projects": items }).to_string()
    }

    fn project_assign(&mut self, args: &tools::Args) -> String {
        let tab_id = match args.tab_id.or_else(|| {
            self.tabs.iter().find(|tab| tab.active).map(|tab| tab.tab_id)
        }) {
            Some(id) if self.tabs.iter().any(|tab| tab.tab_id == id) => id,
            Some(id) => return format!("tab {id} is not available"),
            None => return "no active tab is available".to_owned(),
        };
        let project = match self.remember_project(args) {
            Ok(project) => project,
            Err(error) => return error,
        };
        self.tab_projects.insert(tab_id, project.name.clone());
        // The two maps stay disjoint, so `project_list` cannot report a tab as
        // both assigned and found.
        self.auto_tab_projects.remove(&tab_id);
        if self.tabs.iter().any(|tab| tab.tab_id == tab_id && tab.active) {
            self.active_project = Some(project.name.clone());
        }
        format!("assigned tab {tab_id} to project {}", project.name)
    }

    fn project_open(&mut self, args: &tools::Args) -> String {
        let project = match self.remember_project(args) {
            Ok(project) => project,
            Err(error) => return error,
        };
        let call = ToolCall {
            id: String::new(),
            name: "new_tab".to_owned(),
            arguments: serde_json::json!({
                "name": args.tab_name.as_deref().unwrap_or(&project.name),
                "cwd": project.root,
            })
            .to_string(),
        };
        let result = match tools::dispatch(&call) {
            Dispatch::Done(result) => result,
            Dispatch::Deferred => return "project opening cannot be deferred".to_owned(),
        };
        self.register_opened_project(&project.name, &result)
    }

    fn register_opened_project(&mut self, name: &str, result: &str) -> String {
        let tab_id = serde_json::from_str::<serde_json::Value>(&result)
            .ok()
            .and_then(|value| value["tab_id"].as_u64());
        let Some(tab_id) = tab_id else {
            return result.to_owned();
        };
        self.tab_projects.insert(tab_id as usize, name.to_owned());
        self.auto_tab_projects.remove(&(tab_id as usize));
        self.active_project = Some(name.to_owned());
        result.to_owned()
    }

    /// The project a todo tool acts on, as `(root, name)`.
    ///
    /// A list is keyed on the root folder, so a rename of the project keeps the
    /// list. `project` takes an exact name, or an absolute root for a list whose
    /// project is gone. The name comes back only to put in the answer.
    fn todo_target(&self, args: &tools::Args) -> Result<(String, Option<String>), String> {
        let selected = args.project.as_deref().or(self.active_project.as_deref());
        let Some(selected) = selected else {
            return Err("no active project; give project, or call project_list".to_owned());
        };
        if selected.starts_with('/') {
            let root = todos::normalize_root(selected)?;
            let name = self
                .projects
                .values()
                .find(|project| todos::normalize_root(&project.root) == Ok(root.clone()))
                .map(|project| project.name.clone());
            return Ok((root, name));
        }
        match self.projects.get(selected) {
            Some(project) => Ok((
                todos::normalize_root(&project.root)?,
                Some(project.name.clone()),
            )),
            None => Err(format!("unknown project {selected}; call project_list")),
        }
    }

    fn todo_add(&mut self, args: &tools::Args) -> String {
        let Some(text) = args.text.as_deref() else {
            return "text is required".to_owned();
        };
        let (root, name) = match self.todo_target(args) {
            Ok(target) => target,
            Err(error) => return error,
        };
        let id = match todos::add(&mut self.todos, &root, text, args.parent_id) {
            Ok(id) => id,
            Err(error) => return error,
        };
        post_todos(&self.todos);
        let open = self.todos.get(&root).map_or(0, |list| todos::counts(list).0);
        serde_json::json!({
            "added": { "id": id, "text": text.trim(), "parent_id": args.parent_id },
            "project": name,
            "root": root,
            "open": open,
        })
        .to_string()
    }

    fn todo_list(&self, args: &tools::Args) -> String {
        let (root, name) = match self.todo_target(args) {
            Ok(target) => target,
            Err(error) => return error,
        };
        let mut value = todos::list_json(&self.todos, &root, args.include_done);
        value["project"] = serde_json::json!(name);
        value.to_string()
    }

    /// The name of the project that owns a folder, for a search result.
    fn project_named(&self, root: &str) -> Option<String> {
        self.projects
            .values()
            .find(|project| project.root == root)
            .map(|project| project.name.clone())
    }

    fn todo_search(&self, args: &tools::Args) -> String {
        let Some(query) = args.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) else {
            return "query is required".to_owned();
        };
        // Every list is searched by folder, so a list whose project went away is
        // still found. The registry is worked out again on every poll; the file
        // is the lasting record.
        let (roots, scope) = if args.all_projects {
            (self.todos.keys().cloned().collect(), "all".to_owned())
        } else {
            match self.todo_target(args) {
                Ok((root, name)) => (vec![root.clone()], name.unwrap_or(root)),
                Err(error) => return error,
            }
        };
        let found = todos::search(&self.todos, &roots, query);
        let items: Vec<serde_json::Value> = found
            .iter()
            .map(|found| {
                serde_json::json!({
                    "id": found.id,
                    "text": found.text,
                    "done": found.done,
                    "project": self.project_named(&found.root),
                    "root": found.root,
                    "trail": found.trail,
                })
            })
            .collect();
        serde_json::json!({
            "query": query,
            "scope": scope,
            "matches": items,
            "truncated": found.len() >= todos::MAX_MATCHES,
        })
        .to_string()
    }

    fn todo_done(&mut self, args: &tools::Args) -> String {
        let Some(id) = args.id else {
            return "id is required".to_owned();
        };
        let (root, name) = match self.todo_target(args) {
            Ok(target) => target,
            Err(error) => return error,
        };
        let change = match todos::set_done(&mut self.todos, &root, id, !args.undo) {
            Ok(change) => change,
            Err(error) => return error,
        };
        post_todos(&self.todos);
        serde_json::json!({
            "id": change.id,
            "text": change.text,
            "done": change.done,
            "also_done": change.cascaded,
            "reopened": change.reopened,
            "project": name,
        })
        .to_string()
    }

    /// The project of a tab: what the user assigned, then what the poll found.
    fn tab_project(&self, tab_id: usize) -> Option<&str> {
        self.tab_projects
            .get(&tab_id)
            .or_else(|| self.auto_tab_projects.get(&tab_id))
            .map(String::as_str)
    }

    /// Pane id to project name, for each agent whose folder a project owns.
    fn pane_projects(&self) -> BTreeMap<u32, String> {
        self.panes
            .iter()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| {
                let owner = projects::for_cwd(&self.projects, pane.cwd.as_deref()?)?;
                Some((pane.pane_id, owner.name.clone()))
            })
            .collect()
    }

    /// Make a project for the folder of each agent that no project owns yet.
    ///
    /// Only an agent counts. A shell at a prompt is not a reason to remember a
    /// folder for good.
    fn discover_projects(&mut self) {
        let cwds: Vec<String> = self
            .panes
            .iter()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| pane.cwd.clone())
            .collect();
        let discovery = projects::discover(&mut self.projects, &cwds);
        if !discovery.changed() {
            return;
        }
        // A shallower folder can take the folders of a project a poll found
        // before it. Anything that holds that name must move with it.
        for (gone, kept) in &discovery.replaced {
            self.rename_project(gone, kept);
        }
        post_projects(&self.projects);
    }

    /// Point everything that names a project that went away at the project that
    /// took its folders. Without this, the active project can name nothing and
    /// `agent_list` answers "unknown project".
    fn rename_project(&mut self, gone: &str, kept: &str) {
        for assigned in self
            .tab_projects
            .values_mut()
            .chain(self.auto_tab_projects.values_mut())
            .chain(self.active_project.iter_mut())
        {
            if assigned == gone {
                kept.clone_into(assigned);
            }
        }
    }

    /// Work out the project of each tab again, and of the active tab with it.
    fn refresh_tab_projects(&mut self) {
        let owners = self.pane_projects();
        self.auto_tab_projects = agents::tab_projects(&self.panes, &self.manual_kinds, &owners);
        // A tab the user assigned is left alone, so the two maps stay disjoint.
        self.auto_tab_projects
            .retain(|tab_id, _| !self.tab_projects.contains_key(tab_id));
        self.active_project = self
            .tabs
            .iter()
            .find(|tab| tab.active)
            .and_then(|tab| self.tab_project(tab.tab_id))
            .map(str::to_owned);
    }

    /// The snapshot every agent tool works from. Split out so the host calls in
    /// `gather_poll` are the only part that cannot run under `make test`.
    fn set_panes(&mut self, panes: Vec<agents::AgentPane>) {
        self.panes = panes;
        // A pane that closed is a registration that no longer names anything.
        self.manual_kinds
            .retain(|id, _| self.panes.iter().any(|pane| pane.pane_id == *id));
        self.activity
            .retain(|id, _| self.panes.iter().any(|pane| pane.pane_id == *id));
    }

    /// Name a pane as an agent by hand, for what detection cannot know.
    fn agent_register(&mut self, args: &tools::Args) -> String {
        let Some(pane_id) = args.pane_id else {
            return "pane_id is required".to_owned();
        };
        let Some(kind) = args.kind.as_deref() else {
            return "kind is required".to_owned();
        };
        if !agents::valid_kind(kind) {
            return "kind must be 1 to 32 characters of a-z, 0-9, _ or -".to_owned();
        }
        // `panes` holds terminal panes only, so this one check also answers a plugin
        // pane and a pane id the model invented.
        if !self.panes.iter().any(|pane| pane.pane_id == pane_id) {
            return format!("pane {pane_id} is not available");
        }
        self.manual_kinds.insert(pane_id, kind.to_owned());
        format!("registered pane {pane_id} as {kind}")
    }

    /// Every agent, with the tail of what each one is showing.
    fn agent_list(&self, args: &tools::Args) -> String {
        let selected = args.project.as_deref().or(self.active_project.as_deref());
        let project = match selected {
            Some(name) => match self.projects.get(name) {
                Some(project) => Some(project),
                None => return format!("unknown project {name}; call project_list"),
            },
            None => None,
        };
        let panes: Vec<agents::AgentPane> = self
            .panes
            .iter()
            .filter(|pane| {
                project.is_none_or(|project| {
                    pane.cwd
                        .as_deref()
                        .and_then(|cwd| projects::for_cwd(&self.projects, cwd))
                        .is_some_and(|owner| owner.name == project.name)
                })
            })
            .cloned()
            .collect();
        agents::list_json(
            &panes,
            &self.manual_kinds,
            &self.activity,
            &self.pane_projects(),
            |pane_id| read_pane_text(pane_id),
        )
    }

    /// `agent_send` and `agent_control`, which do their own work through the tool
    /// that already does it. Nothing here talks to the host directly.
    fn agent_act(&mut self, call: &ToolCall, args: &tools::Args) -> String {
        match self.agent_host_call(call, args) {
            Ok(host) => match tools::dispatch(&host) {
                Dispatch::Done(result) => result,
                Dispatch::Deferred => "agent action cannot be deferred".to_owned(),
            },
            Err(error) => error,
        }
    }

    /// Open an agent pane through the tool that already opens panes, then register
    /// the kind the caller gave against the pane id `new_pane` reports.
    fn agent_spawn(&mut self, args: &tools::Args) -> String {
        if args.command_words().is_empty() {
            return "command is required".to_owned();
        }
        if let Some(kind) = args.kind.as_deref() {
            if !agents::valid_kind(kind) {
                return "kind must be 1 to 32 characters of a-z, 0-9, _ or -".to_owned();
            }
        }
        let cwd = match self.agent_cwd(args) {
            Ok(cwd) => cwd,
            Err(error) => return error,
        };
        let host = ToolCall {
            id: String::new(),
            name: "new_pane".to_owned(),
            arguments: serde_json::json!({
                "command": args.command,
                "args": args.args,
                "cwd": cwd,
            })
            .to_string(),
        };
        match tools::dispatch(&host) {
            Dispatch::Done(result) => self.register_spawned(args.kind.as_deref(), &result),
            Dispatch::Deferred => "agent action cannot be deferred".to_owned(),
        }
    }

    fn agent_cwd(&self, args: &tools::Args) -> Result<Option<String>, String> {
        let selected = args.project.as_deref().or(self.active_project.as_deref());
        let project = match selected {
            Some(name) => match self.projects.get(name) {
                Some(project) => Some(project),
                None => return Err(format!("unknown project {name}; call project_list")),
            },
            None => None,
        };
        if let (Some(project), Some(cwd)) = (project, args.cwd.as_deref()) {
            if !projects::contains(project, cwd) {
                return Err(format!("cwd must be inside project {}", project.name));
            }
        }
        Ok(args
            .cwd
            .clone()
            .or_else(|| project.map(|project| project.root.clone())))
    }

    /// Fold a spawn's kind into `manual_kinds`, keyed by the pane id `new_pane`
    /// reported. The pane is not in the snapshot yet — the next poll, at most one
    /// heartbeat away, brings it in; the id is already allocated, so the kind
    /// survives that poll's prune. No kind means detection covers it.
    fn register_spawned(&mut self, kind: Option<&str>, result: &str) -> String {
        let pane_id = serde_json::from_str::<serde_json::Value>(result)
            .ok()
            .and_then(|value| value["pane_id"].as_u64());
        match (kind, pane_id) {
            (Some(kind), Some(pane_id)) => {
                self.manual_kinds.insert(pane_id as u32, kind.to_owned());
                format!("{result} (registered as {kind})")
            }
            _ => result.to_owned(),
        }
    }

    /// The existing tool call an agent call means, or why it means none.
    ///
    /// Pure, so every mapping is testable on the host triple — which matters, because
    /// this is what keeps `agent_control close` from reaching a pane the user never
    /// called an agent.
    fn agent_host_call(&self, call: &ToolCall, args: &tools::Args) -> Result<ToolCall, String> {
        let Some(pane_id) = args.pane_id else {
            return Err("pane_id is required".to_owned());
        };
        if !self.is_agent(pane_id) {
            return Err(format!(
                "pane {pane_id} is not an agent; call agent_list, or agent_register it first"
            ));
        }
        // An agent is always a terminal pane, so is_plugin is settled here rather
        // than asked of the model.
        let (name, arguments) = match call.name.as_str() {
            // Empty input is left to send_input, which already refuses it.
            "agent_send" => (
                "send_input",
                serde_json::json!({
                    "pane_id": pane_id,
                    "is_plugin": false,
                    "text": args.text,
                    "keys": args.keys,
                }),
            ),
            _ => match args.action.as_deref() {
                Some("interrupt") => (
                    "send_input",
                    serde_json::json!({
                        "pane_id": pane_id,
                        "is_plugin": false,
                        "keys": ["ctrl_c"],
                    }),
                ),
                Some("restart") => (
                    "rerun_pane",
                    serde_json::json!({ "pane_id": pane_id, "is_plugin": false }),
                ),
                Some("close") => (
                    "close_pane",
                    serde_json::json!({ "pane_id": pane_id, "is_plugin": false }),
                ),
                Some(other) => return Err(format!("unknown agent action: {other}")),
                None => return Err("action is required".to_owned()),
            },
        };
        Ok(ToolCall {
            id: call.id.clone(),
            name: name.to_owned(),
            arguments: arguments.to_string(),
        })
    }

    /// Whether this pane is an agent right now: in the last snapshot, and either
    /// registered by hand or detected.
    fn is_agent(&self, pane_id: u32) -> bool {
        self.panes.iter().any(|pane| {
            pane.pane_id == pane_id && agents::classify(pane, &self.manual_kinds).is_some()
        })
    }

    /// Refresh the snapshot here and now and note transitions in the transcript.
    /// Returns whether the screen moved: a poll that found nothing new must not
    /// recomposite the session.
    ///
    /// This is the synchronous path — `show_agents` and the `agent_*` tools need
    /// a fresh snapshot before they can answer. The timer path goes through the
    /// poll worker instead; running here supersedes any poke in flight, and the
    /// cleared `poll_pending` makes its late reply drop.
    fn poll_agents(&mut self) -> bool {
        self.poll_pending = false;
        self.polls_unanswered = 0;
        let mut activity = std::mem::take(&mut self.activity);
        match gather_poll(&self.manual_kinds, &mut activity) {
            Some(panes) => self.apply_poll(panes, activity),
            // No session to look at; keep what we knew.
            None => {
                self.activity = activity;
                false
            }
        }
    }

    /// Merge one poll's outcome — sync or worker, same code — and note
    /// transitions in the transcript.
    fn apply_poll(&mut self, panes: Vec<agents::AgentPane>, activity: agents::Activity) -> bool {
        // Taken before `set_panes` prunes `manual_kinds`, so a manual agent that
        // closed still has its kind in the notice.
        let before = agents::agent_map(&self.panes, &self.manual_kinds);
        self.activity = activity;
        self.set_panes(panes);
        // Project detection runs with agent detection, off the same snapshot.
        self.discover_projects();
        self.refresh_tab_projects();
        let notices = agents::transition_notices(
            &before,
            &agents::agent_map(&self.panes, &self.manual_kinds),
        );
        for notice in &notices {
            self.messages.push(Message::notice(notice.clone()));
            self.log(
                "notice",
                serde_json::json!({"content": log::truncate(notice)}),
            );
        }
        !notices.is_empty()
    }

    /// A worker reply. Only the poll worker sends any.
    fn on_custom_message(&mut self, message: &str, payload: &str) -> bool {
        if message != "poll_result" || !self.poll_pending {
            // Unknown, or superseded: a sync poll ran while this one was in
            // flight and what it would merge is older than what we have.
            return false;
        }
        self.poll_pending = false;
        self.polls_unanswered = 0;
        let Ok(reply) = serde_json::from_str::<PollReply>(payload) else {
            return false;
        };
        match reply.panes {
            Some(panes) => self.apply_poll(panes, reply.activity),
            None => false,
        }
    }

    fn schedule(&mut self, args: &tools::Args) -> String {
        let Some(prompt) = args
            .prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty())
        else {
            return "schedule needs a prompt".to_owned();
        };
        // NaN and anything under the floor both land on the floor: f64::max
        // returns the other operand for NaN.
        let every = args.seconds.unwrap_or(0.0).max(MIN_JOB_SECONDS);

        self.job_seq += 1;
        let id = self.job_seq;
        let prompt_clone = prompt.clone();
        let show = args.show;
        self.jobs.push(Job {
            id,
            every,
            due_in: every,
            prompt,
            show,
            auto_confirm: args.auto_confirm,
        });
        self.arm_timer(JOB_SECONDS);
        self.log(
            "job_schedule",
            serde_json::json!({
                "job_id": id,
                "every": every,
                "prompt": log::truncate(&prompt_clone),
                "show": show,
                "auto_confirm": args.auto_confirm,
            }),
        );
        format!("scheduled job {id} — runs every {every:.0}s until unscheduled")
    }

    fn unschedule(&mut self, id: Option<u64>) -> String {
        let result = match id {
            Some(id) => {
                let before = self.jobs.len();
                self.jobs.retain(|job| job.id != id);
                if self.jobs.len() == before {
                    format!("no job {id}")
                } else {
                    format!("cancelled job {id}")
                }
            }
            None => {
                let count = self.jobs.len();
                self.jobs.clear();
                format!("cancelled {count} scheduled job(s)")
            }
        };
        self.log(
            "job_cancel",
            serde_json::json!({
                "job_id": id,
                "result": log::truncate(&result),
            }),
        );
        result
    }

    /// The kill switch that doesn't need the model to cooperate.
    fn stop_jobs(&mut self) -> bool {
        if self.jobs.is_empty() {
            return false;
        }
        let notice = self.unschedule(None);
        self.messages.push(Message::notice(notice.clone()));
        self.log(
            "notice",
            serde_json::json!({"content": log::truncate(&notice)}),
        );
        self.pin_to_bottom();
        true
    }

    fn confirm_tool(&mut self) -> bool {
        let Some(call) = self.pending.front().cloned() else {
            return false;
        };
        self.log(
            "tool_confirm",
            serde_json::json!({
                "tool_call_id": call.id,
                "tool_name": call.name,
                "decision": "confirm",
            }),
        );
        self.status = Status::Idle;
        if self.run_head(&call) {
            self.pump_tools();
        }
        true
    }

    fn decline_tool(&mut self) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        if let Some(head) = self.pending.front() {
            self.log(
                "tool_confirm",
                serde_json::json!({
                    "tool_call_id": head.id,
                    "tool_name": head.name,
                    "decision": "decline",
                }),
            );
        }
        self.status = Status::Idle;
        // Recorded as the tool's result, not as a notice: the model must see a
        // reply for every call it made or the next request is malformed.
        self.finish_head("declined by the user");
        self.pump_tools();
        true
    }

    /// Give up on the whole run. Every outstanding call still gets an answer, so
    /// the transcript stays valid if the conversation continues.
    ///
    /// This is also the escape hatch for a `run_command` that never returns,
    /// which is why there is no timer.
    fn abort_tools(&mut self) -> bool {
        self.status = Status::Idle;
        // The model asked, but it is not getting an answer — the results are
        // recorded for the transcript's sake, not to be sent back.
        self.owes_reply = false;
        self.job_auto_confirm = false;
        while !self.pending.is_empty() {
            self.finish_head("aborted by the user");
        }
        self.messages.push(Message::notice("tools aborted"));
        self.log("notice", serde_json::json!({"content": "tools aborted"}));
        self.log("abort", serde_json::json!({"reason": "user"}));
        self.pin_to_bottom();
        true
    }
}

// ---------------------------------------------------------------------------
// The archive
// ---------------------------------------------------------------------------

impl State {
    /// What autosave watches. The same trick `ui::TranscriptKey` relies on: only
    /// the last message is ever mutated in place, so a count and that message's
    /// length between them catch every change to the transcript.
    fn fingerprint(&self) -> (usize, usize) {
        (self.messages.len(), self.mutable_len())
    }

    /// The length of the one message that can change without the count changing:
    /// the streaming placeholder, which `append_to_reply` grows in place. The
    /// last message is the fallback — it is the placeholder whenever there is
    /// one and nothing has been pushed behind it, and when there is no stream
    /// nothing is mutated in place at all.
    ///
    /// A typed tool mid-reply is what makes the two differ: its call and result
    /// sit *after* the placeholder, so watching the last message would leave the
    /// arriving reply invisible to the cache, the archive and every other client.
    pub fn mutable_len(&self) -> usize {
        let at = self.reply_at.unwrap_or(self.messages.len().saturating_sub(1));
        self.messages.get(at).map_or(0, |m| m.content.len())
    }

    /// Write the transcript out once it has settled.
    ///
    /// In-flight is skipped so a stream saves once at the end rather than on
    /// every socket read, and an empty transcript is skipped so a pane you only
    /// opened leaves nothing behind.
    fn autosave(&mut self) {
        if self.is_in_flight() || self.messages.is_empty() {
            return;
        }
        let key = self.fingerprint();
        if self.saved == key {
            return;
        }
        self.saved = key;
        if let Some((id, json)) = history::save(&mut self.chat_id, &self.messages) {
            post_save(id, json);
        }
    }

    /// Autosave, but write here and now instead of on the worker, and without
    /// waiting for the reply to settle. For the two places a queued write is not
    /// good enough: `show_chats` reads the archive right back, and `BeforeClose`
    /// has no later.
    ///
    /// Unlike `autosave` this writes mid-stream. It is called once per switch or
    /// once per close, not once per socket read, so the reason for that guard
    /// doesn't apply — and a partial reply on disk beats losing it.
    fn flush_save(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        self.saved = self.fingerprint();
        if let Some((id, json)) = history::save(&mut self.chat_id, &self.messages) {
            history::write(id, json.as_bytes());
        }
    }

    /// The archive list, newest first, behind `/history`. Opens mid-reply: the
    /// chat you leave keeps streaming in the background, so switching costs it
    /// nothing.
    fn show_chats(&mut self) {
        // So the chat you are in is in the list, and current. Synchronous —
        // list() below would race a write queued on the worker. Every parked
        // chat is written too, or one that has never been saved would have no
        // id and no row to switch back to.
        self.flush_save();
        for chat in &mut self.parked {
            if let Some((id, json)) = history::save(&mut chat.chat_id, &chat.messages) {
                history::write(id, json.as_bytes());
            }
        }
        let live: Vec<u64> = self.live_chat_ids();
        let (ids, titles) = history::list()
            .into_iter()
            .map(|entry| {
                // A dot marks the ones still working, so a background reply is
                // visible from the list rather than only on arrival.
                let title = match live.contains(&entry.id) {
                    true => format!("● {}", entry.title),
                    false => entry.title,
                };
                (entry.id, title)
            })
            .unzip();
        self.picker = Some(Picker {
            entries: titles,
            selected: 0,
            kind: PickerKind::Chats(ids),
        });
    }

    /// How many other conversations are working off-screen.
    pub fn parked_count(&self) -> usize {
        self.parked.len()
    }

    /// Chats with something still outstanding, foreground and parked alike.
    fn live_chat_ids(&self) -> Vec<u64> {
        self.parked
            .iter()
            .filter(|chat| chat.is_live())
            .filter_map(|chat| chat.chat_id)
            .chain(self.chat_id.filter(|_| self.is_in_flight()))
            .collect()
    }

    /// Ask the endpoint what it serves, and show the list as it arrives. Behind
    /// `/models`.
    ///
    /// No `Status` variant and no spinner: the picker is modal, so this is the
    /// only thing *it* is waiting on and the empty list is itself the "fetching"
    /// state. A reply may well be arriving behind it; the `models` context on the
    /// request is what keeps the two responses apart.
    ///
    /// ponytail: like `web_search`, a plain `web_request` has no stream id and so
    /// nothing to cancel — `Esc` stops waiting and the late reply is dropped
    /// because the picker it was for is gone.
    fn show_models(&mut self) {
        // Nothing to ask, and the banner behind the picker already says why.
        let Ok(cfg) = &self.cfg else {
            return;
        };
        web_request(
            api::models_url(&cfg.base_url),
            HttpVerb::Get,
            api::models_headers(cfg),
            Vec::new(),
            // Round-trips through the host: this is what tells the response
            // apart from the one that leads a chat stream.
            BTreeMap::from([("models".to_owned(), "1".to_owned())]),
        );
        self.picker = Some(Picker {
            entries: Vec::new(),
            selected: 0,
            kind: PickerKind::Models,
        });
    }

    /// Ctrl-r's answer: the agent picker. Opens over a streaming reply for the
    /// same reason `/history` does — it drives the session, not the conversation.
    fn open_agent_picker(&mut self) -> bool {
        self.autosave();
        self.show_agents();
        true
    }

    /// The agent list, fresh: entering the picker re-polls, so the rows are the
    /// session as it is now, not as of the last heartbeat.
    fn show_agents(&mut self) {
        self.poll_agents();
        self.agent_due_in = AGENT_SECONDS;
        let (ids, entries) = agents::strip_rows(
            &self.panes,
            &self.manual_kinds,
            &self.activity,
            &self.pane_projects(),
        )
        .into_iter()
        .unzip();
        self.picker = Some(Picker {
            entries,
            selected: 0,
            kind: PickerKind::Agents(ids),
        });
    }

    /// Bring an agent's pane into view, through the tool that already does it —
    /// `focus` finds the pane's tab itself, so this works across tabs.
    fn focus_agent(&self, pane_id: u32) {
        tools::dispatch(&ToolCall {
            id: String::new(),
            name: "focus".to_owned(),
            arguments: serde_json::json!({"pane_id": pane_id, "is_plugin": false}).to_string(),
        });
    }

    /// The deferred half of `show_models`. Returns whether this response was one:
    /// a `WebRequestResult` with no `models` context is the head of a stream.
    ///
    /// Anything other than a list to show closes the picker and says why in the
    /// transcript, rather than leaving "fetching models…" on screen forever.
    fn finish_models(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !context.contains_key("models") {
            return false;
        }
        // A fetch we already abandoned with Esc, or one already answered: an
        // empty model list is exactly the still-waiting state.
        match self.picker.as_ref() {
            Some(picker)
                if matches!(picker.kind, PickerKind::Models) && picker.entries.is_empty() => {}
            _ => return false,
        }

        if !(200..300).contains(&status) {
            self.picker = None;
            let err = api::api_error_message(status, body);
            self.messages.push(Message::error(err.clone()));
            self.log(
                "error",
                serde_json::json!({"error": log::truncate(&err), "status": status}),
            );
            self.pin_to_bottom();
            return true;
        }

        let models = api::model_ids(body);
        if models.is_empty() {
            self.picker = None;
            self.messages
                .push(Message::notice("the endpoint listed no models"));
            self.log(
                "notice",
                serde_json::json!({"content": "the endpoint listed no models"}),
            );
            self.pin_to_bottom();
            return true;
        }

        // Open on the model in use, so the list says where you are as well as
        // where you can go.
        let current = self
            .cfg
            .as_ref()
            .map(|cfg| cfg.model.clone())
            .unwrap_or_default();
        self.picker = Some(Picker {
            selected: models.iter().position(|id| *id == current).unwrap_or(0),
            entries: models,
            kind: PickerKind::Models,
        });
        true
    }

    /// Every key while the picker is open. Unhandled keys are swallowed rather
    /// than falling through to the input box hidden behind it.
    fn picker_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Esc => self.picker = None,
            BareKey::Enter => {
                let Some(picker) = self.picker.take() else {
                    return false;
                };
                match &picker.kind {
                    PickerKind::Chats(ids) => {
                        if let Some(id) = ids.get(picker.selected).copied() {
                            self.switch_to_chat(id);
                        }
                    }
                    // Held only for this session: config is re-resolved from the
                    // KDL block and the environment on every load.
                    PickerKind::Models => {
                        if let (Some(model), Ok(cfg)) =
                            (picker.entries.get(picker.selected), &mut self.cfg)
                        {
                            let m = model.clone();
                            cfg.model = m.clone();
                            self.log_global("model_switch", serde_json::json!({"model": m}));
                        }
                    }
                    PickerKind::Agents(ids) => {
                        if let Some(id) = ids.get(picker.selected).copied() {
                            self.focus_agent(id);
                        }
                    }
                }
            }
            BareKey::Up | BareKey::Down => {
                let Some(picker) = self.picker.as_mut() else {
                    return false;
                };
                let last = picker.entries.len().saturating_sub(1);
                picker.selected = match key.bare_key {
                    BareKey::Up => picker.selected.saturating_sub(1),
                    _ => (picker.selected + 1).min(last),
                };
            }
            _ => return false,
        }
        true
    }

    /// Put another chat on screen, and set this one aside rather than throwing
    /// it away. Three cases, in order of how little there is to do:
    ///
    /// 1. it is already the one you are looking at — nothing;
    /// 2. it is parked and still live — swap the two, and the reply that has
    ///    been arriving off-screen is there waiting;
    /// 3. otherwise load it from the archive.
    ///
    /// The chat being left is parked only if something is still owed to it. A
    /// settled one is already in the archive and is dropped, which is what keeps
    /// `parked` to the handful of conversations actually working.
    fn switch_to_chat(&mut self, id: u64) {
        if self.chat_id == Some(id) {
            return;
        }
        let parked = self.parked.iter().position(|chat| chat.chat_id == Some(id));
        let incoming = match parked {
            Some(index) => self.parked.remove(index),
            None => {
                let Some(messages) = history::load(id) else {
                    return;
                };
                Chat {
                    messages,
                    chat_id: Some(id),
                    ..Chat::default()
                }
            }
        };

        let outgoing = self.take_chat();
        self.put_chat(incoming);
        if parked.is_none() {
            // Loaded, not resumed: nothing has changed yet, so this must not
            // immediately rewrite the file it came from. A parked chat brings
            // its own `saved`, which may well be behind its transcript.
            self.saved = self.fingerprint();
        }
        self.pin_to_bottom();
        self.log_global("open_chat", serde_json::json!({"chat_id": id}));
        if outgoing.is_live() {
            self.parked.push(outgoing);
        }
    }

}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

impl State {
    fn active_stream_id(&self) -> Option<String> {
        self.stream.as_ref().map(|stream| stream.id.clone())
    }

    /// Lift the foreground conversation out, leaving an empty one behind.
    fn take_chat(&mut self) -> Chat {
        self.transcript_cache = None;
        Chat {
            messages: std::mem::take(&mut self.messages),
            status: std::mem::take(&mut self.status),
            stream: self.stream.take(),
            reply_at: self.reply_at.take(),
            sse: std::mem::take(&mut self.sse),
            tool_calls: std::mem::take(&mut self.tool_calls),
            pending: std::mem::take(&mut self.pending),
            tool_rounds: std::mem::take(&mut self.tool_rounds),
            owes_reply: std::mem::take(&mut self.owes_reply),
            chat_id: self.chat_id.take(),
            saved: std::mem::take(&mut self.saved),
            scroll: std::mem::take(&mut self.scroll),
            job_auto_confirm: std::mem::take(&mut self.job_auto_confirm),
        }
    }

    /// Put one in the foreground. Whatever was there is overwritten, so this is
    /// always paired with a `take_chat`.
    fn put_chat(&mut self, chat: Chat) {
        self.transcript_cache = None;
        self.messages = chat.messages;
        self.status = chat.status;
        self.stream = chat.stream;
        self.reply_at = chat.reply_at;
        self.sse = chat.sse;
        self.tool_calls = chat.tool_calls;
        self.pending = chat.pending;
        self.tool_rounds = chat.tool_rounds;
        self.owes_reply = chat.owes_reply;
        self.chat_id = chat.chat_id;
        self.saved = chat.saved;
        self.scroll = chat.scroll;
        self.job_auto_confirm = chat.job_auto_confirm;
    }

    /// Run `f` against a parked chat by swapping it into the foreground for the
    /// duration and swapping the real one back afterwards.
    ///
    /// Doing it this way is what keeps `on_chunk`, `pump_tools` and the rest
    /// single-conversation code: they only ever see the chat they are for, in
    /// the one place they know to look for it. A chat that settles while it is
    /// in here is archived and dropped rather than re-parked — `/history` reads
    /// it back from the file.
    fn with_parked<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let foreground = self.take_chat();
        let parked = self.parked.remove(index);
        self.put_chat(parked);

        let out = f(self);
        self.autosave();

        let background = self.take_chat();
        self.put_chat(foreground);
        if background.is_live() {
            self.parked.insert(index, background);
        }
        out
    }

    /// Which parked chat a response belongs to, by whichever id its context
    /// carries. `None` means the foreground's — including when it is nobody's,
    /// which the handlers already turn away on their own.
    fn owner_of(&self, context: &BTreeMap<String, String>) -> Option<usize> {
        if let Some(stream_id) = context.get(STREAM_CONTEXT) {
            return self.parked_with_stream(stream_id);
        }
        self.parked_with_call(context.get("tool_call_id")?)
    }

    /// Which parked chat started this stream, if any.
    fn parked_with_stream(&self, stream_id: &str) -> Option<usize> {
        self.parked
            .iter()
            .position(|chat| chat.stream.as_ref().is_some_and(|s| s.id == stream_id))
    }

    /// Which parked chat is waiting on this tool call. Only the head can be:
    /// the queue runs one at a time.
    fn parked_with_call(&self, call_id: &str) -> Option<usize> {
        self.parked
            .iter()
            .position(|chat| chat.pending.front().is_some_and(|call| call.id == call_id))
    }

    /// A reply is arriving, or its trailing chunk is still owed.
    fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    /// A reply is arriving *and* is still ours to record — `finishing` means the
    /// outcome is already in the transcript and the rest is being swallowed.
    fn is_receiving(&self) -> bool {
        matches!(&self.stream, Some(stream) if !stream.finishing)
    }

    /// The tool queue is doing something. Independent of the stream: both can be
    /// true at once, which is the point of the split.
    fn tools_busy(&self) -> bool {
        !matches!(self.status, Status::Idle)
    }

    /// Anything at all outstanding. The archive and the `Ctrl-l` teardown care
    /// about this rather than about which half it is.
    fn is_in_flight(&self) -> bool {
        self.is_streaming() || self.tools_busy()
    }

    /// The call awaiting `y`/`n`, if any. Rendered so the prompt names the one
    /// call it is about rather than leaving the user to infer it.
    pub fn confirmation_prompt(&self) -> Option<String> {
        match self.status {
            Status::ConfirmingTool => self.pending.front().map(tools::summarize),
            _ => None,
        }
    }

    /// Whether we are waiting on something *and* nothing is arriving — the one
    /// predicate the whole waiting indicator keys off.
    fn is_loading(&self) -> bool {
        match self.status {
            Status::AwaitingCommand | Status::AwaitingSearch => true,
            // A confirmation prompt is waiting on the user rather than on us,
            // so the only thing left to be waiting on is the reply. Once its
            // first token lands the arriving text is its own proof of life; a
            // spinner beside streaming words is just noise. `finishing` has
            // already recorded its outcome and is waiting on nothing.
            Status::Idle => self.is_receiving() && self.reply_is_empty(),
            Status::ConfirmingTool => false,
        }
    }

    /// Whether the timer chain should keep running. Wider than `is_loading`:
    /// a stream that is already producing text still needs frames, because
    /// `on_chunk` no longer asks for a render of its own and this is what
    /// repaints the arriving reply.
    fn wants_frames(&self) -> bool {
        self.is_loading() || self.is_receiving()
    }

    /// The spinner and its phrase, or `None` when nothing is being waited on.
    pub fn loader(&self) -> Option<String> {
        if !self.is_loading() {
            return None;
        }
        let spinner = ui::spinner(self.tick);

        // A running command names itself: "running: cargo build" beats a joke
        // when what you actually want to know is what you are waiting on.
        let head = self
            .pending
            .front()
            .and_then(|call| tools::Args::parse(&call.arguments).ok());
        if matches!(self.status, Status::AwaitingCommand) {
            let words = head.map(|args| args.command_words()).unwrap_or_default();
            if !words.is_empty() {
                return Some(format!("{spinner} running: {}", words.join(" ")));
            }
        } else if matches!(self.status, Status::AwaitingSearch) {
            if let Some(query) = head.and_then(|args| args.query) {
                return Some(format!("{spinner} searching: {query}"));
            }
        }

        Some(format!(
            "{spinner} {}",
            ui::phrase(self.tick, self.stream_seq)
        ))
    }

    /// Schedule the next tick. `set_timeout` is one-shot, so the spinner
    /// animation, the streaming repaint and the job countdown are all this
    /// being re-armed from `on_timer` for as long as there is something to do.
    ///
    /// ponytail: one chain, so a request that starts while a slow job tick is
    /// outstanding can show its spinner up to `JOB_SECONDS` late — Zellij has
    /// no cancel_timeout, and a second chain would defeat `timer_armed`.
    fn arm_timer(&mut self, interval: f64) {
        if !self.timer_armed {
            set_timeout(interval);
            self.timer_armed = true;
        }
    }

    fn on_timer(&mut self, elapsed: f64) -> bool {
        self.timer_armed = false;
        self.mono_ms = self.mono_ms.saturating_add((elapsed * 1000.0) as u64);
        let fired = self.tick_jobs(elapsed);
        let moved = self.tick_agents(elapsed);
        let frames = self.wants_frames();
        if frames {
            self.tick += 1;
        }
        // The chain no longer dies: the agent poll has to keep looking. It still
        // slows right down to `AGENT_SECONDS` once nothing else wants the clock.
        self.arm_timer(if frames {
            FRAME_SECONDS
        } else if self.jobs.is_empty() {
            AGENT_SECONDS
        } else {
            JOB_SECONDS
        });
        // A quiet tick changed nothing on screen; asking for a render would
        // recomposite the whole session for nothing.
        frames || fired || moved
    }

    /// Count the agent poll down and poke the poll worker when it comes due.
    /// Returns whether a notice landed in the transcript — from the worker path
    /// never, its reply lands later as a `CustomMessage`.
    fn tick_agents(&mut self, elapsed: f64) -> bool {
        self.agent_due_in -= elapsed;
        if self.agent_due_in > 0.0 {
            return false;
        }
        self.agent_due_in = AGENT_SECONDS;
        if self.poll_dead {
            return self.poll_agents();
        }
        if self.poll_pending {
            // ponytail: three silent pokes (~15s) writes the worker off for the
            // session and the poll runs inline as before it existed. Covers a
            // wedged worker and a zellij without the worker-permissions patch
            // alike; three strikes absorbs one legitimately slow poll.
            self.polls_unanswered += 1;
            if self.polls_unanswered >= 3 {
                self.poll_dead = true;
                self.log_global(
                    "notice",
                    serde_json::json!({"content": "poll worker unresponsive; polling inline"}),
                );
                return self.poll_agents();
            }
            return false;
        }
        if let Ok(request) = serde_json::to_string(&PollRequest {
            manual_kinds: self.manual_kinds.clone(),
            activity: self.activity.clone(),
        }) {
            self.poll_pending = true;
            post_message_to(PluginMessage::new_to_worker("poll", "poll", &request));
        }
        false
    }

    /// Count the jobs down and start at most one due run. Returns whether the
    /// transcript changed.
    ///
    /// Only fires when `Idle`: there is exactly one stream slot, and a job that
    /// comes due mid-reply waits for a later tick rather than stacking runs.
    ///
    /// ponytail: a scheduled run that calls a destructive tool parks on its y/n
    /// prompt and holds every other job with it. Auto-declining confirmations
    /// on scheduled runs is the fix if that ever bites.
    fn tick_jobs(&mut self, elapsed: f64) -> bool {
        for job in &mut self.jobs {
            job.due_in -= elapsed;
        }
        if self.is_in_flight() || self.picker.is_some() {
            return false;
        }
        let Ok(cfg) = self.cfg.clone() else {
            return false;
        };
        let Some(job) = self.jobs.iter_mut().find(|job| job.due_in <= 0.0) else {
            return false;
        };

        job.due_in = job.every;
        let prompt = job.prompt.clone();
        let job_id = job.id;
        self.job_auto_confirm = job.auto_confirm;
        // Popping the pane open is also what makes a confirmation prompt
        // reachable, so the show flag covers both meanings of "tell me".
        if job.show {
            show_self(true);
        }
        // Mentions resolve at fire time, so "check @codex every minute" reads the
        // agent as it is now, not as it was when the job was armed.
        let mut fired = format!("[scheduled] {prompt}");
        if let Some(context) =
            agents::mention_context(&fired, &self.panes, &self.manual_kinds, &self.activity)
        {
            fired.push_str("\n\n");
            fired.push_str(&context);
        }
        self.messages.push(Message::user(fired));
        self.log(
            "job_fire",
            serde_json::json!({
                "job_id": job_id,
                "prompt": log::truncate(&prompt),
            }),
        );
        self.log(
            "user_input",
            serde_json::json!({
                "content": log::truncate(&format!("[scheduled] {prompt}")),
                "scheduled": true,
                "job_id": job_id,
            }),
        );
        // A fresh tool budget, same as a typed question.
        self.tool_rounds = 0;
        self.start_request(&cfg);
        true
    }

    /// The message the arriving reply is being appended to, by index. `None`
    /// once the stream has settled, or if the placeholder was dropped.
    fn reply(&mut self) -> Option<&mut Message> {
        let at = self.reply_at?;
        self.messages
            .get_mut(at)
            .filter(|message| message.role == Role::Assistant)
    }

    fn append_to_reply(&mut self, text: &str) {
        match self.reply() {
            Some(message) => message.content.push_str(text),
            // An error line replaced the placeholder; start a fresh reply rather
            // than appending model output onto the error text.
            None => {
                self.reply_at = Some(self.messages.len());
                self.messages.push(Message::assistant(text));
            }
        }
    }

    /// Whether the arriving reply is the last thing in the transcript. False
    /// once a tool typed mid-reply has pushed anything behind the placeholder.
    pub fn reply_is_last(&self) -> bool {
        self.reply_at == Some(self.messages.len().saturating_sub(1))
    }

    fn reply_is_empty(&self) -> bool {
        matches!(
            self.reply_at.and_then(|at| self.messages.get(at)),
            Some(message) if message.role == Role::Assistant && message.content.is_empty()
        )
    }

    /// Drop the streaming placeholder if nothing ever arrived in it. Partial
    /// text is deliberately kept — a reply cut short is still worth reading.
    ///
    /// Removing it shifts everything a typed tool appended after it, so this is
    /// the one place indices move; `reply_at` is cleared either way, since after
    /// this there is no placeholder left to append to.
    fn pop_empty_placeholder(&mut self) {
        if let (true, Some(at)) = (self.reply_is_empty(), self.reply_at) {
            self.messages.remove(at);
        }
        self.reply_at = None;
    }

    fn cancel_stream(&mut self) -> bool {
        let Some(stream_id) = self.active_stream_id() else {
            return false;
        };
        cancel_web_request(&stream_id);
        self.pop_empty_placeholder();
        self.messages.push(Message::notice("cancelled"));
        self.log("notice", serde_json::json!({"content": "cancelled"}));
        self.log(
            "cancel",
            serde_json::json!({"stream_id": stream_id.clone()}),
        );
        self.job_auto_confirm = false;
        // Not dropped: the host still owes exactly one `done: true` chunk, and
        // `finishing` is what keeps it from being recorded as a second outcome.
        self.stream = Some(StreamState {
            id: stream_id,
            finishing: true,
        });
        true
    }

    /// Unloading a plugin does not cancel its in-flight streams host-side, so
    /// the shutdown path has to.
    fn cancel_in_flight(&mut self) {
        if let Some(stream_id) = self.active_stream_id() {
            cancel_web_request(stream_id);
        }
    }

    /// Start over. The chat being left is *kept* in the archive — detaching from
    /// its file is all this does, so the next save opens a new one.
    fn new_chat(&mut self) -> bool {
        let old_id = self.chat_id;
        self.log_global("new_chat", serde_json::json!({"old_chat_id": old_id}));
        self.cancel_in_flight();
        self.status = Status::Idle;
        self.stream = None;
        self.reply_at = None;
        self.messages.clear();
        self.chat_id = None;
        self.saved = self.fingerprint();
        self.picker = None;
        self.input.clear();
        self.sse = SseParser::default();
        self.tool_calls = ToolCallAccumulator::default();
        self.pending.clear();
        self.tool_rounds = 0;
        self.owes_reply = false;
        self.job_auto_confirm = false;
        self.pin_to_bottom();
        // A watcher armed for the conversation being thrown away has nothing
        // left to compare against, and would otherwise keep billing.
        self.jobs.clear();
        true
    }

    /// Scroll toward older messages, for the client that asked. Clamped at
    /// render against the real line count, which the key handler doesn't know.
    fn scroll_up(&mut self, lines: usize) -> bool {
        *self.scroll.entry(self.client).or_default() += lines;
        true
    }

    fn scroll_down(&mut self, lines: usize) -> bool {
        let offset = self.scroll.entry(self.client).or_default();
        *offset = offset.saturating_sub(lines);
        true
    }

    fn log(&mut self, kind: &str, payload: serde_json::Value) {
        if self.chat_id.is_none() && !self.messages.is_empty() {
            if let Some((id, json)) = history::save(&mut self.chat_id, &self.messages) {
                self.saved = self.fingerprint();
                post_save(id, json);
            }
        }
        self.log_seq = self.log_seq.wrapping_add(1);
        post_log(self.chat_id, self.mono_ms, self.log_seq, kind, payload);
    }

    fn log_global(&mut self, kind: &str, payload: serde_json::Value) {
        self.log_seq = self.log_seq.wrapping_add(1);
        post_log(self.chat_id, self.mono_ms, self.log_seq, kind, payload);
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(messages: Vec<Message>, status: Status) -> State {
        State {
            messages,
            status,
            ..State::default()
        }
    }

    /// Mid-reply, as `start_request` would have left it: a stream owning `id`,
    /// and the trailing assistant message as the placeholder it appends into.
    fn state_streaming(messages: Vec<Message>, id: &str) -> State {
        let reply_at = messages
            .iter()
            .rposition(|message| message.role == Role::Assistant);
        State {
            messages,
            reply_at,
            stream: Some(StreamState {
                id: id.to_owned(),
                finishing: false,
            }),
            ..State::default()
        }
    }

    /// Everything outstanding has landed — the reply and the tool queue both.
    /// The two are independent now, so "idle" takes clearing both.
    fn settle(state: &mut State) {
        state.status = Status::Idle;
        state.stream = None;
        state.reply_at = None;
    }

    /// The stream that has already had its outcome recorded and is only waiting
    /// to swallow the host's last chunk.
    fn finishing(id: &str) -> Option<StreamState> {
        Some(StreamState {
            id: id.to_owned(),
            finishing: true,
        })
    }

    /// A state that can actually send, so the tool loop reaches its follow-up
    /// request. `web_request_stream` is a no-op stub off wasm, so nothing leaves.
    fn ready_state() -> State {
        State {
            perms: true,
            cfg: Config::resolve(
                &BTreeMap::new(),
                &BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-test".to_owned())]),
            ),
            stream: Some(StreamState {
                id: "hocus-1".into(),
                finishing: false,
            }),
            reply_at: Some(1),
            messages: vec![Message::user("hi"), Message::assistant("")],
            ..State::default()
        }
    }

    /// One SSE frame carrying a complete tool call, as `on_chunk` would see it.
    fn tool_call_frame(id: &str, name: &str, arguments: &str) -> Vec<u8> {
        format!(
            r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"{id}","function":{{"name":"{name}","arguments":"{}"}}}}]}}}}]}}
"#,
            arguments.replace('"', "\\\"")
        )
        .into_bytes()
    }

    fn tool_results(state: &State) -> Vec<&Message> {
        state
            .messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .collect()
    }

    // -- the tool loop ------------------------------------------------------

    #[test]
    fn a_pane_closure_runs_without_confirmation() {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(!matches!(state.status, Status::ConfirmingTool));
        assert_eq!(tool_results(&state).len(), 1);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn a_read_only_call_runs_without_asking() {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "list_session", "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(!matches!(state.status, Status::ConfirmingTool));
        assert_eq!(tool_results(&state).len(), 1);
        assert_eq!(state.tool_rounds, 1);
    }

    /// Declining must still answer the call: the API rejects a transcript where
    /// a requested call has no matching result.
    #[test]
    fn declining_records_a_result_rather_than_dropping_the_call() {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);
        state.decline_tool();

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_call_id.as_deref(), Some("call_1"));
        assert!(results[0].content.contains("declined"));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn confirming_runs_the_call_and_moves_on() {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);
        state.confirm_tool();

        assert_eq!(tool_results(&state).len(), 1);
        assert!(!state.messages[tool_result_index(&state)]
            .content
            .contains("declined"));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn an_unrestricted_job_requires_one_confirmation_when_the_model_creates_it() {
        let mut state = ready_state();
        let frame = tool_call_frame(
            "call_1",
            "schedule",
            r#"{"seconds":60,"prompt":"check","auto_confirm":true}"#,
        );
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(state.jobs.is_empty(), "the job waits for user authority");
        state.confirm_tool();
        assert_eq!(state.jobs.len(), 1);
        assert!(state.jobs[0].auto_confirm);
    }

    fn tool_result_index(state: &State) -> usize {
        state
            .messages
            .iter()
            .position(|message| message.role == Role::Tool)
            .expect("a tool result")
    }

    /// Aborting answers every outstanding call, so the transcript stays valid if
    /// the conversation continues.
    #[test]
    fn aborting_answers_every_outstanding_call() {
        let mut state = ready_state();
        state.pending = vec![
            ToolCall {
                id: "a".into(),
                name: "close_pane".into(),
                arguments: "{}".into(),
            },
            ToolCall {
                id: "b".into(),
                name: "close_tab".into(),
                arguments: "{}".into(),
            },
        ]
        .into();
        state.status = Status::ConfirmingTool;
        state.abort_tools();

        let results = tool_results(&state);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_call_id.as_deref(), Some("a"));
        assert_eq!(results[1].tool_call_id.as_deref(), Some("b"));
        assert!(matches!(state.status, Status::Idle));
    }

    /// One round short of the cap the model still gets a request — but a
    /// tool-free one, so the answer comes from the results already in hand
    /// instead of the turn dead-ending on an error.
    #[test]
    fn the_last_round_asks_without_tools() {
        let mut state = ready_state();
        state.tool_rounds = MAX_TOOL_ROUNDS - 1;

        let frame = tool_call_frame("call_1", "list_session", "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(state.is_receiving(), "asked again");
        assert_eq!(state.tool_rounds, MAX_TOOL_ROUNDS);
        // Last is the streaming placeholder; the notice sits just before it.
        let notice = &state.messages[state.messages.len() - 2];
        assert_eq!(notice.role, Role::Notice);
        assert!(notice.content.contains("tool budget spent"));
    }

    /// Past the cap the model called a tool it was no longer offered. Nothing
    /// left to try, so the turn stops.
    #[test]
    fn a_model_that_calls_tools_it_was_not_offered_is_stopped() {
        let mut state = ready_state();
        state.tool_rounds = MAX_TOOL_ROUNDS;

        let frame = tool_call_frame("call_1", "list_session", "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(matches!(state.status, Status::Idle), "no follow-up request");
        assert_eq!(state.messages.last().unwrap().role, Role::Error);
        assert!(state.messages.last().unwrap().content.contains("rounds"));
    }

    // -- manual `/tool` invocation ------------------------------------------

    /// The call-and-result pair is the whole point: an orphan result makes the
    /// next request malformed, and the model would never see what was run. The
    /// default state is deliberately unconfigured — this must not need an API.
    #[test]
    fn a_slash_command_runs_the_tool_and_sends_nothing() {
        let mut state = State::default();
        state.input.set_text("/list_session".to_owned());
        state.submit();

        assert!(matches!(state.status, Status::Idle), "no request fired");
        assert!(!state.owes_reply && state.pending.is_empty());
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].tool_calls.len(), 1);
        assert_eq!(state.messages[0].tool_calls[0].name, "list_session");
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].tool_call_id,
            Some(state.messages[0].tool_calls[0].id.clone()),
            "the result answers the call"
        );
    }

    /// Typing the command is the confirmation, so the y/n prompt is skipped.
    #[test]
    fn a_destructive_slash_command_does_not_ask() {
        let mut state = State::default();
        state
            .input
            .set_text("/close_pane pane_id=1 is_plugin=false".to_owned());
        state.submit();

        assert!(!matches!(state.status, Status::ConfirmingTool));
        assert_eq!(
            state.messages[0].tool_calls[0].arguments, r#"{"is_plugin":false,"pane_id":1}"#,
            "the pairs came out typed, not as strings"
        );
    }

    #[test]
    fn a_malformed_slash_command_is_an_error_not_a_call() {
        let mut state = State::default();
        state.input.set_text("/read_pane 3".to_owned());
        state.submit();

        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, Role::Error);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn tab_completes_a_unique_tool_name() {
        let mut state = State::default();
        state.input.set_text("/list_ses".to_owned());
        state.complete_tool_name();
        assert_eq!(state.input.get_text(), "/list_session ");

        // Ambiguous, and ordinary text, both leave the buffer alone.
        state.input.set_text("/new_".to_owned());
        state.complete_tool_name();
        assert_eq!(state.input.get_text(), "/new_");
        state.input.set_text("hello".to_owned());
        assert!(!state.complete_tool_name());
    }

    #[test]
    fn tab_completes_a_mention_from_the_agents_on_screen() {
        let mut state = state_with_panes();
        state.manual_kinds.insert(4, "reviewer".to_owned());

        state.input.set_text("ask @cod".to_owned());
        assert!(state.complete_tool_name());
        assert_eq!(state.input.get_text(), "ask @codex ");
        assert_eq!(state.input.get_cursor_position(), 11);

        // Mid-message: only the token at the cursor completes; the tail stays.
        state.input.set_text("ask @rev about pane 3".to_owned());
        state.input.set_cursor_position(8);
        assert!(state.complete_tool_name());
        assert_eq!(state.input.get_text(), "ask @reviewer  about pane 3");
        assert_eq!(state.input.get_cursor_position(), 14);

        // A bare @ lists every candidate; the buffer is left alone.
        state.input.set_text("@".to_owned());
        assert!(state.complete_tool_name());
        assert_eq!(state.input.get_text(), "@");
        assert_eq!(
            state.messages.last().unwrap().content,
            "@codex  @reviewer  @3  @4",
        );
    }

    #[test]
    fn a_submitted_mention_carries_the_agent_context_with_it() {
        let mut state = ready_state();
        settle(&mut state);
        state.set_panes(terminal_panes(
            &manifest(vec![pane_info(3, "codex", Some("codex"))]),
            &[tab_info(0, 9, "work")],
        ));
        state.input.set_text("ask @codex to run the tests".to_owned());
        state.submit();

        let user = state
            .messages
            .iter()
            .rfind(|message| message.role == Role::User)
            .unwrap();
        assert_eq!(
            user.content,
            "ask @codex to run the tests\n\n[@codex is agent pane 3 (codex, running) on tab \"work\"]",
        );
    }

    #[test]
    fn a_new_question_resets_the_tool_budget() {
        let mut state = ready_state();
        settle(&mut state);
        state.tool_rounds = MAX_TOOL_ROUNDS;
        state.input.insert_char('?');
        state.submit();

        assert_eq!(state.tool_rounds, 0);
    }

    #[test]
    fn a_command_result_for_a_call_we_are_not_waiting_on_is_ignored() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.pending = vec![ToolCall {
            id: "mine".into(),
            name: "run_command".into(),
            arguments: "{}".into(),
        }]
        .into();

        let rendered = state.on_command_result(
            Some(0),
            b"out",
            b"",
            &BTreeMap::from([("tool_call_id".to_owned(), "someone_elses".to_owned())]),
        );

        assert!(!rendered);
        assert!(tool_results(&state).is_empty());
        assert_eq!(state.pending.len(), 1, "still waiting on ours");
    }

    #[test]
    fn a_command_result_reports_the_exit_code_and_both_streams() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.pending = vec![ToolCall {
            id: "mine".into(),
            name: "run_command".into(),
            arguments: "{}".into(),
        }]
        .into();

        state.on_command_result(
            Some(2),
            b"stdout text",
            b"stderr text",
            &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
        );

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("exit code: 2"));
        assert!(results[0].content.contains("stdout text"));
        assert!(results[0].content.contains("stderr text"));
    }

    /// A tool result is re-sent on every later turn, so a command that dumps
    /// megabytes must not be paid for forever.
    #[test]
    fn a_command_result_is_capped_before_it_reaches_the_transcript() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.pending = vec![ToolCall {
            id: "mine".into(),
            name: "slack_read".into(),
            arguments: "{}".into(),
        }]
        .into();

        let flood = "x".repeat(MAX_TOOL_OUTPUT * 2);
        state.on_command_result(
            Some(0),
            flood.as_bytes(),
            b"",
            &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
        );

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.len() < flood.len());
        assert!(results[0].content.contains("exit code: 0"));
    }

    /// Zellij reports "could not spawn" as exit code 2 with a bare io::Error,
    /// naming neither the binary nor the setting that chose it.
    #[test]
    fn a_slack_tool_that_cannot_find_dobbs_says_which_binary_and_setting() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.pending = vec![ToolCall {
            id: "mine".into(),
            name: "slack_search".into(),
            arguments: "{}".into(),
        }]
        .into();

        state.on_command_result(
            Some(2),
            b"",
            b"No such file or directory (os error 2)",
            &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
        );

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains(api::DEFAULT_DOBBS));
        assert!(results[0].content.contains("dobbs_path"));
    }

    /// The hint must not swallow a real exit-2 from a command that ran, nor fire
    /// for `run_command`, whose argv the model wrote itself.
    #[test]
    fn the_dobbs_hint_is_limited_to_a_failure_to_spawn_a_slack_tool() {
        for (name, stderr) in [
            ("slack_search", &b"grep: no matches"[..]),
            ("run_command", b"No such file or directory (os error 2)"),
        ] {
            let mut state = ready_state();
            state.status = Status::AwaitingCommand;
            state.pending = vec![ToolCall {
                id: "mine".into(),
                name: name.into(),
                arguments: "{}".into(),
            }]
            .into();

            state.on_command_result(
                Some(2),
                b"",
                stderr,
                &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
            );

            let results = tool_results(&state);
            assert_eq!(results.len(), 1);
            assert!(
                results[0].content.starts_with("exit code: 2"),
                "{name} got the hint: {}",
                results[0].content
            );
        }
    }

    /// A changed KDL block is the only way a running plugin's config can move:
    /// Zellij freezes it at server start and replays it even on a reload.
    #[test]
    fn a_configuration_change_event_re_resolves_the_config() {
        let mut state = ready_state();
        // What the permission grant would have left behind.
        state.granted = true;
        state.env = BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-test".to_owned())]);
        assert_eq!(state.cfg.as_ref().unwrap().dobbs, api::DEFAULT_DOBBS);

        state.handle(Event::PluginConfigurationChanged(BTreeMap::from([(
            "dobbs_path".to_owned(),
            "/opt/dobbs".to_owned(),
        )])));

        assert_eq!(state.cfg.as_ref().unwrap().dobbs, "/opt/dobbs");

        // Before the permission grant there is no environment to resolve
        // against, so the config must be left as it is rather than becoming a
        // spurious "no API key" banner.
        let mut state = State::default();
        state.handle(Event::PluginConfigurationChanged(BTreeMap::from([(
            "model".to_owned(),
            "gpt-5".to_owned(),
        )])));
        assert!(state.cfg.is_err());
    }

    /// The Slack tools are intercepted before `tools::dispatch`, which on the
    /// host triple would answer "tools are unavailable outside zellij".
    #[test]
    fn a_bad_slack_call_is_answered_without_reaching_dispatch() {
        let mut state = ready_state();
        let call = ToolCall {
            id: "mine".into(),
            name: "slack_read".into(),
            arguments: "{}".into(),
        };
        state.pending = vec![call.clone()].into();

        assert!(state.run_head(&call), "a rejected call must not defer");
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.starts_with("channel is required"));
    }

    // -- web_search ---------------------------------------------------------

    fn searching_state(query: &str) -> State {
        let mut state = ready_state();
        state.pending = vec![ToolCall {
            id: "mine".into(),
            name: "web_search".into(),
            arguments: format!(r#"{{"query":"{query}"}}"#),
        }]
        .into();
        state
    }

    /// The whole point of the deferred path: the queue must not advance until
    /// the response lands, or the model is answered before it was asked.
    #[test]
    fn a_search_defers_until_its_response_arrives() {
        let mut state = searching_state("zellij");
        let call = state.pending.front().cloned().unwrap();

        assert!(!state.run_head(&call), "the queue must wait");
        assert!(matches!(state.status, Status::AwaitingSearch));
        assert!(tool_results(&state).is_empty());

        let rendered = state.on_response(
            200,
            br#"{"results":[{"title":"Zellij","url":"https://zellij.dev","content":"a workspace"}]}"#,
            &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
        );

        assert!(rendered);
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("https://zellij.dev"));
    }

    #[test]
    fn a_search_response_for_a_call_we_are_not_waiting_on_is_ignored() {
        let mut state = searching_state("zellij");
        state.status = Status::AwaitingSearch;

        let rendered = state.on_response(
            200,
            br#"{"results":[]}"#,
            &BTreeMap::from([("tool_call_id".to_owned(), "someone_elses".to_owned())]),
        );

        assert!(!rendered);
        assert!(tool_results(&state).is_empty());
        assert_eq!(state.pending.len(), 1, "still waiting on ours");
    }

    /// A dead SearxNG must answer the call rather than hang the tool loop.
    #[test]
    fn a_failed_search_is_reported_as_the_tools_result() {
        let mut state = searching_state("zellij");
        state.status = Status::AwaitingSearch;

        state.on_response(
            502,
            b"bad gateway",
            &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
        );

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("502"), "{}", results[0].content);
    }

    #[test]
    fn a_search_with_no_query_is_answered_rather_than_sent() {
        let mut state = ready_state();
        let call = ToolCall {
            id: "mine".into(),
            name: "web_search".into(),
            arguments: r#"{"query":"  "}"#.into(),
        };
        state.pending = vec![call.clone()].into();

        assert!(state.run_head(&call), "the queue advances");
        assert!(!matches!(state.status, Status::AwaitingSearch));
        assert_eq!(tool_results(&state).len(), 1);
    }

    /// The API stream's own lead response is told apart by its `stream` context,
    /// so it must still reach the streaming path even while a search is somehow
    /// in flight.
    #[test]
    fn a_stream_response_is_not_mistaken_for_a_search_result() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.job_auto_confirm = true;
        state.on_response(401, br#"{"error":{"message":"bad key"}}"#, &stream_context("hocus-1"));

        assert_eq!(state.messages[1].role, Role::Error);
        assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
        assert!(!state.job_auto_confirm);
    }

    #[test]
    fn a_new_chat_clears_pending_tool_calls() {
        let mut state = ready_state();
        state.status = Status::ConfirmingTool;
        state.pending = vec![ToolCall {
            id: "a".into(),
            name: "close_pane".into(),
            arguments: "{}".into(),
        }]
        .into();
        state.tool_rounds = 3;
        state.new_chat();

        assert!(state.pending.is_empty());
        assert_eq!(state.tool_rounds, 0);
        assert!(matches!(state.status, Status::Idle));
    }

    // -- the waiting indicator ----------------------------------------------

    #[test]
    fn waiting_on_a_reply_shows_the_loader() {
        let state = ready_state();
        assert!(state.is_loading());
        assert!(state.loader().is_some());
    }

    #[test]
    fn the_loader_clears_the_moment_a_token_arrives() {
        let mut state = ready_state();
        state.on_chunk(
            "hocus-1",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n",
            false,
            None,
        );

        assert!(!state.is_loading(), "text is its own proof of life");
        assert!(state.loader().is_none());
    }

    /// The prompt is waiting on the user, not on us — a spinner there would be
    /// claiming we're busy while we sit doing nothing.
    #[test]
    fn a_confirmation_prompt_shows_no_loader() {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(!state.is_loading());
    }

    #[test]
    fn an_idle_pane_shows_no_loader() {
        assert!(!State::default().is_loading());
        assert!(State::default().loader().is_none());
    }

    #[test]
    fn a_running_command_is_named_rather_than_given_a_phrase() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.pending = vec![ToolCall {
            id: "a".into(),
            name: "run_command".into(),
            arguments: r#"{"command":["git","status"]}"#.into(),
        }]
        .into();

        let loader = state.loader().expect("a loader while awaiting a command");
        assert!(loader.contains("running: git status"), "{loader}");
    }

    /// Unparseable arguments must not lose the indicator entirely.
    #[test]
    fn a_command_we_cannot_parse_falls_back_to_a_phrase() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.pending = vec![ToolCall {
            id: "a".into(),
            name: "run_command".into(),
            arguments: "{not json".into(),
        }]
        .into();

        let loader = state.loader().expect("still a loader");
        assert!(!loader.contains("running:"), "{loader}");
    }

    /// The runaway guard. The chain no longer stops — the agent poll has to keep
    /// looking — so what it must not do is repaint. A quiet poll that asked for a
    /// render would recomposite the whole session every `AGENT_SECONDS` forever.
    #[test]
    fn an_idle_timer_tick_polls_without_repainting() {
        let mut state = ready_state();
        settle(&mut state);
        state.timer_armed = true;

        assert!(!state.on_timer(FRAME_SECONDS), "no redraw");
        assert!(state.timer_armed, "but the agent poll keeps the chain alive");
        assert_eq!(state.tick, 0, "the spinner does not advance either");
    }

    #[test]
    fn the_timer_chain_continues_while_waiting() {
        let mut state = ready_state();
        state.timer_armed = true;

        assert!(state.on_timer(FRAME_SECONDS));
        assert!(state.timer_armed, "rearmed for the next frame");
        assert_eq!(state.tick, 1);
    }

    /// A cancelled stream must not also execute whatever tools it had half
    /// emitted before the user hit Esc.
    #[test]
    fn tool_calls_from_a_cancelled_stream_are_discarded() {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.cancel_stream();
        state.on_chunk("hocus-1", b"", true, Some("cancelled".into()));

        assert!(state.pending.is_empty());
        assert!(matches!(state.status, Status::Idle));
    }

    #[test]
    fn a_partial_reply_survives_a_mid_stream_error() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("partial")], "hocus-1");
        state.on_chunk("hocus-1", b"", true, Some("connection reset".into()));

        assert_eq!(state.messages[1].content, "partial");
        assert_eq!(state.messages[2].role, Role::Error);
        assert!(matches!(state.status, Status::Idle));
    }

    #[test]
    fn an_api_error_revokes_job_authority_and_discards_tool_calls() {
        let mut state = ready_state();
        state.job_auto_confirm = true;
        let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk(
            "hocus-1",
            b"data: {\"error\":{\"message\":\"bad response\"}}\n",
            false,
            None,
        );

        assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
        assert!(!state.job_auto_confirm);
        state.on_chunk("hocus-1", b"", true, None);
        assert!(tool_results(&state).is_empty());
    }

    #[test]
    fn cancelling_a_stream_revokes_job_authority_immediately() {
        let mut state = ready_state();
        state.job_auto_confirm = true;

        state.cancel_stream();

        assert!(!state.job_auto_confirm);
    }

    #[test]
    fn an_empty_placeholder_is_replaced_by_the_error() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.on_chunk("hocus-1", b"", true, Some("connection reset".into()));

        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[1].role, Role::Error);
    }

    #[test]
    fn a_silent_stream_reports_that_nothing_came_back() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.on_chunk("hocus-1", b"", true, None);

        assert_eq!(state.messages[1].role, Role::Error);
        assert!(state.messages[1].content.contains("empty response"));
    }

    #[test]
    fn finishing_swallows_the_terminator_without_a_second_outcome() {
        // What a cancel leaves behind: notice already recorded, terminator due.
        let mut state = State {
            messages: vec![Message::user("hi"), Message::notice("cancelled")],
            stream: finishing("hocus-1"),
            ..State::default()
        };
        state.on_chunk("hocus-1", b"", true, Some("cancelled".into()));

        assert_eq!(state.messages.len(), 2, "no extra error line");
        assert!(matches!(state.status, Status::Idle));
    }

    #[test]
    fn chunks_from_a_superseded_stream_are_ignored() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-2");
        let rendered = state.on_chunk(
            "hocus-1",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"stale\"}}]}\n",
            false,
            None,
        );

        assert!(!rendered);
        assert_eq!(state.messages[1].content, "");
    }

    /// Every plugin render costs the host a full screen composite, on the same
    /// thread that services every other Zellij action — one per socket read
    /// froze the whole session for the length of a reply. Only the terminator
    /// asks for a render now; the frame timer repaints what arrives in between.
    #[test]
    fn a_mid_stream_chunk_accumulates_text_without_asking_for_a_render() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        let rendered = state.on_chunk(
            "hocus-1",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n",
            false,
            None,
        );

        assert!(!rendered, "no render per chunk");
        assert_eq!(state.messages[1].content, "Hel", "but the text still lands");
        assert!(
            state.on_chunk("hocus-1", b"", true, None),
            "the terminator renders"
        );
    }

    /// The counterpart to the above: with chunks no longer rendering, the timer
    /// is the only thing repainting a reply. It must outlive the spinner, which
    /// stops at the first token.
    #[test]
    fn the_frame_timer_outlives_the_spinner() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("some text")], "hocus-1");

        assert!(!state.is_loading(), "text is arriving, so no spinner");
        assert!(state.on_timer(FRAME_SECONDS), "but frames keep coming");
        assert!(state.timer_armed, "and the chain re-armed itself");
    }

    #[test]
    fn the_frame_timer_stops_once_the_stream_ends() {
        let mut state = state_with(vec![Message::user("hi")], Status::Idle);

        assert!(!state.on_timer(FRAME_SECONDS));
        assert_eq!(state.tick, 0, "no more frames for an idle plugin");
    }

    #[test]
    fn streamed_deltas_accumulate_into_the_placeholder() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.on_chunk(
            "hocus-1",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n",
            false,
            None,
        );
        state.on_chunk(
            "hocus-1",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n",
            false,
            None,
        );

        assert_eq!(state.messages[1].content, "Hello");
    }

    #[test]
    fn a_non_2xx_response_reports_the_api_error_and_expects_a_terminator() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.on_response(401, br#"{"error":{"message":"bad key"}}"#, &stream_context("hocus-1"));

        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[1].role, Role::Error);
        assert!(state.messages[1].content.contains("bad key"));
        assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
    }

    #[test]
    fn a_2xx_response_changes_nothing_because_the_body_arrives_as_chunks() {
        let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        let rendered = state.on_response(200, b"", &BTreeMap::new());

        assert!(!rendered);
        assert_eq!(state.messages.len(), 2);
        assert!(state.is_receiving());
    }

    // -- the archive --------------------------------------------------------

    fn picker_state(titles: &[&str]) -> State {
        State {
            picker: Some(Picker {
                entries: titles.iter().map(|title| (*title).to_owned()).collect(),
                selected: 0,
                kind: PickerKind::Chats((1..=titles.len() as u64).collect()),
            }),
            ..ready_state()
        }
    }

    fn picker(state: &State) -> &Picker {
        state.picker.as_ref().expect("the picker is open")
    }

    /// The point of the archive: starting over must not destroy what came
    /// before. Ctrl-l detaches from the file rather than deleting it.
    #[test]
    fn a_new_chat_detaches_from_the_archived_file_rather_than_deleting_it() {
        let mut state = ready_state();
        settle(&mut state);
        state.chat_id = Some(7);
        state.new_chat();

        assert_eq!(state.chat_id, None, "the next save opens a new file");
        assert_eq!(state.saved, (0, 0), "and is not skipped as unchanged");
    }

    // -- alongside a live reply ---------------------------------------------

    /// The whole point of the split: a tool the user typed runs while the reply
    /// is still arriving, and neither loses anything to the other. The arriving
    /// text must keep landing in its own placeholder rather than in whatever
    /// message happens to be last.
    #[test]
    fn a_typed_tool_runs_beside_an_arriving_reply() {
        let mut state = ready_state();
        state.on_chunk("hocus-1", &delta_frame("half a "), false, None);

        state.input.set_text("/list_session".to_owned());
        assert!(state.submit(), "the command is not refused mid-reply");
        assert_eq!(tool_results(&state).len(), 1, "and it ran");

        state.on_chunk("hocus-1", &delta_frame("sentence"), false, None);
        assert_eq!(
            state.messages[1].content, "half a sentence",
            "the reply is still whole"
        );
        assert!(state.is_receiving(), "and still arriving");
    }

    /// The symptom this all started from: Enter on a slash command mid-reply did
    /// nothing at all, not even redraw.
    #[test]
    fn the_archive_opens_over_an_arriving_reply() {
        let mut state = ready_state();
        state.input.set_text("/history".to_owned());

        assert!(state.submit(), "handled, so the pane repaints");
        assert!(state.input.get_text().is_empty(), "and the line was taken");
        assert!(matches!(
            state.picker.as_ref().map(|picker| &picker.kind),
            Some(PickerKind::Chats(_))
        ));
        assert!(state.is_receiving(), "with the reply still arriving behind it");
    }

    /// The fingerprint is what tells the transcript cache, the archive and every
    /// other client that something changed. It watched the *last* message, which
    /// a tool typed mid-reply is no longer — the reply would grow invisibly.
    #[test]
    fn the_fingerprint_follows_the_reply_and_not_the_last_message() {
        let mut state = ready_state();
        state.input.set_text("/list_session".to_owned());
        state.submit();
        assert!(!state.reply_is_last(), "the result is behind the placeholder");

        let before = state.fingerprint();
        state.on_chunk("hocus-1", &delta_frame("more"), false, None);
        assert_ne!(state.fingerprint(), before, "and the reply still shows up");
    }

    /// One tool at a time is still the rule — the queue's head owns the result
    /// the host is about to deliver. Only the *stream* runs alongside.
    #[test]
    fn a_typed_tool_waits_for_the_one_already_running() {
        let mut state = ready_state();
        state.status = Status::AwaitingCommand;
        state.input.set_text("/list_session".to_owned());
        state.submit();

        assert!(tool_results(&state).is_empty(), "nothing ran");
        assert_eq!(state.messages.last().map(|m| m.role), Some(Role::Notice));
    }

    /// The model's calls land when its stream ends, which can be while a typed
    /// one is still running. They queue behind it rather than replacing it, and
    /// the answer still goes back once the queue drains.
    #[test]
    fn model_calls_queue_behind_a_typed_one_that_is_still_running() {
        let mut state = ready_state();
        state.pending.push_back(ToolCall {
            id: "manual-0".into(),
            name: "run_command".into(),
            arguments: r#"{"command":["sleep","1"]}"#.into(),
        });
        state.status = Status::AwaitingCommand;

        let frame = tool_call_frame("call_1", "list_session", "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert_eq!(state.pending.len(), 2, "queued, not swapped in");
        assert_eq!(state.pending[0].id, "manual-0", "the running one is still head");
        assert!(state.owes_reply, "and the model is owed its answer");
    }

    // -- background chats ---------------------------------------------------

    /// A reply for a chat that is not on screen lands in that chat, and the one
    /// you are looking at is untouched.
    #[test]
    fn a_chunk_for_a_parked_chat_lands_in_it() {
        let mut state = ready_state();
        state.parked.push(Chat {
            messages: vec![Message::user("older"), Message::assistant("")],
            reply_at: Some(1),
            chat_id: Some(4),
            stream: Some(StreamState {
                id: "hocus-7".into(),
                finishing: false,
            }),
            ..Chat::default()
        });

        state.handle(Event::WebRequestStreamChunk {
            stream_id: "hocus-7".into(),
            chunk: delta_frame("background"),
            done: false,
            error: None,
        });

        assert_eq!(state.parked[0].messages[1].content, "background");
        assert_eq!(state.messages[1].content, "", "the visible chat is untouched");
    }

    /// Once a background chat settles it is archived and let go: the file is the
    /// copy of record, and `/history` reads it back.
    #[test]
    fn a_parked_chat_that_settles_is_dropped() {
        let mut state = ready_state();
        state.parked.push(Chat {
            messages: vec![Message::user("older"), Message::assistant("done")],
            reply_at: Some(1),
            chat_id: Some(4),
            stream: Some(StreamState {
                id: "hocus-7".into(),
                finishing: false,
            }),
            ..Chat::default()
        });

        state.handle(Event::WebRequestStreamChunk {
            stream_id: "hocus-7".into(),
            chunk: Vec::new(),
            done: true,
            error: None,
        });

        assert!(state.parked.is_empty(), "settled, so the archive has it");
        assert!(state.is_receiving(), "and ours is still going");
    }

    /// Switching to a chat that is still working picks it back up where it was —
    /// its transcript, its stream and its place in the queue — and the one being
    /// left is parked rather than dropped.
    #[test]
    fn switching_to_a_parked_chat_swaps_the_two() {
        let mut state = ready_state();
        state.chat_id = Some(1);
        state.parked.push(Chat {
            messages: vec![Message::user("older question")],
            chat_id: Some(4),
            stream: Some(StreamState {
                id: "hocus-7".into(),
                finishing: false,
            }),
            ..Chat::default()
        });

        state.switch_to_chat(4);
        assert_eq!(state.chat_id, Some(4));
        assert_eq!(state.messages.len(), 1, "its transcript, not the other's");
        assert_eq!(state.active_stream_id().as_deref(), Some("hocus-7"));
        assert_eq!(state.parked.len(), 1, "and the one we left is held");
        assert_eq!(state.parked[0].chat_id, Some(1));

        state.switch_to_chat(1);
        assert_eq!(state.messages.len(), 2, "back to where we started");
        assert_eq!(state.active_stream_id().as_deref(), Some("hocus-1"));
    }

    /// A settled chat is already in the archive, so parking it would only be a
    /// second copy to keep in step.
    #[test]
    fn a_settled_chat_is_not_parked_when_you_leave_it() {
        let mut state = ready_state();
        settle(&mut state);
        state.chat_id = Some(1);
        state.parked.push(Chat {
            messages: vec![Message::user("older")],
            chat_id: Some(4),
            ..Chat::default()
        });

        state.switch_to_chat(4);
        assert!(state.parked.is_empty(), "nothing outstanding, nothing held");
    }

    /// A picker drives the session, not the conversation, so an arriving reply
    /// is no reason to refuse it — and the reply keeps arriving behind it.
    #[test]
    fn the_picker_opens_over_an_arriving_reply() {
        let mut state = ready_state();
        assert!(state.open_agent_picker());
        assert!(state.picker.is_some());

        state.on_chunk(
            "hocus-1",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"tokens\"}}]}\n",
            false,
            None,
        );
        assert_eq!(state.messages[1].content, "tokens", "still landing");
    }

    #[test]
    fn the_picker_selection_stops_at_both_ends() {
        let mut state = picker_state(&["first", "second"]);
        state.picker_key(KeyWithModifier::new(BareKey::Up));
        assert_eq!(picker(&state).selected, 0, "already at the top");

        state.picker_key(KeyWithModifier::new(BareKey::Down));
        state.picker_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(picker(&state).selected, 1, "and stops at the bottom");
    }

    #[test]
    fn esc_closes_the_picker_and_leaves_the_transcript_alone() {
        let mut state = picker_state(&["first"]);
        let before = state.messages.clone();
        state.picker_key(KeyWithModifier::new(BareKey::Esc));

        assert!(state.picker.is_none());
        assert_eq!(state.messages, before);
        assert_eq!(state.chat_id, None, "nothing was opened");
    }

    /// A keystroke falling through to the input box behind the picker would be
    /// typed invisibly and turn up in the next message.
    #[test]
    fn the_picker_swallows_keys_it_does_not_use() {
        let mut state = picker_state(&["first"]);
        state.on_key(KeyWithModifier::new(BareKey::Char('x')));

        assert!(state.picker.is_some(), "still open");
        assert_eq!(state.input.get_text(), "", "and nothing was typed");
    }

    // -- the model list -----------------------------------------------------

    /// Idle, with the picker open and the fetch in flight — the state `/models`
    /// leaves behind. `web_request` is a no-op stub off wasm.
    fn fetching_models() -> State {
        let mut state = ready_state();
        settle(&mut state);
        state.run_manual("/models");
        state
    }

    /// One SSE frame of reply text, as `on_chunk` would see it.
    fn delta_frame(text: &str) -> Vec<u8> {
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n").into_bytes()
    }

    /// What the host hands back on the response that leads a chat stream.
    fn stream_context(id: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(STREAM_CONTEXT.to_owned(), id.to_owned())])
    }

    fn models_context() -> BTreeMap<String, String> {
        BTreeMap::from([("models".to_owned(), "1".to_owned())])
    }

    /// Each list stands alone behind its own command; Tab no longer cycles them,
    /// so in a picker it is just another swallowed key.
    #[test]
    fn slash_commands_open_the_lists_and_tab_stays_put() {
        let mut state = fetching_models();
        assert!(matches!(picker(&state).kind, PickerKind::Models));
        assert!(picker(&state).entries.is_empty(), "still fetching");

        state.picker_key(KeyWithModifier::new(BareKey::Tab));
        assert!(matches!(picker(&state).kind, PickerKind::Models));

        state.picker = None;
        state.run_manual("/history");
        assert!(matches!(picker(&state).kind, PickerKind::Chats(_)));

        state.picker_key(KeyWithModifier::new(BareKey::Tab));
        assert!(matches!(picker(&state).kind, PickerKind::Chats(_)));
    }

    /// The agent picker opens on its own key, lists the same rows the agent tools
    /// would act on, stands outside any cycle, and Enter hands the pane to the
    /// focus tool — stubbed off wasm, so what is checked is that it closes.
    #[test]
    fn ctrl_r_opens_the_agent_picker_and_enter_closes_it() {
        let mut state = state_with_panes();
        settle(&mut state);
        state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier());

        let rows: Vec<String> = agents::strip_rows(
            &state.panes,
            &state.manual_kinds,
            &state.activity,
            &state.pane_projects(),
        )
        .into_iter()
        .map(|(_, row)| row)
        .collect();
        assert_eq!(picker(&state).entries, rows);
        assert!(matches!(picker(&state).kind, PickerKind::Agents(ref ids) if ids == &vec![3]));

        state.picker_key(KeyWithModifier::new(BareKey::Tab));
        assert!(matches!(picker(&state).kind, PickerKind::Agents(_)), "no cycle");

        state.picker_key(KeyWithModifier::new(BareKey::Enter));
        assert!(state.picker.is_none());
    }

    /// The list is also a display of where you are, so it must open on the model
    /// in use rather than on whatever sorts first.
    #[test]
    fn the_model_list_opens_on_the_model_in_use() {
        let mut state = fetching_models();
        let rendered = state.on_response(
            200,
            br#"{"data":[{"id":"o3-mini"},{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]}"#,
            &models_context(),
        );

        assert!(rendered);
        assert_eq!(picker(&state).entries, ["gpt-4o", "gpt-4o-mini", "o3-mini"]);
        assert_eq!(picker(&state).selected, 1, "DEFAULT_MODEL, sorted second");
    }

    #[test]
    fn choosing_a_model_switches_the_config_and_closes_the_picker() {
        let mut state = fetching_models();
        state.on_response(
            200,
            br#"{"data":[{"id":"gpt-4o"},{"id":"o3"}]}"#,
            &models_context(),
        );
        state.picker_key(KeyWithModifier::new(BareKey::Down));
        state.picker_key(KeyWithModifier::new(BareKey::Enter));

        assert!(state.picker.is_none());
        assert_eq!(state.cfg.as_ref().unwrap().model, "o3");
        assert_eq!(state.messages.len(), 2, "the transcript is untouched");
    }

    /// A dead endpoint must not leave "fetching models…" on screen forever.
    #[test]
    fn a_failed_fetch_closes_the_picker_and_says_why() {
        let mut state = fetching_models();
        let body = br#"{"error":{"message":"bad key"}}"#;
        let rendered = state.on_response(401, body, &models_context());

        assert!(rendered);
        assert!(state.picker.is_none());
        assert_eq!(state.messages.last().unwrap().role, Role::Error);
        assert!(state.messages.last().unwrap().content.contains("bad key"));
    }

    #[test]
    fn an_endpoint_that_lists_nothing_closes_the_picker_too() {
        let mut state = fetching_models();
        state.on_response(200, br#"{"data":[]}"#, &models_context());

        assert!(state.picker.is_none());
        assert_eq!(state.messages.last().unwrap().role, Role::Notice);
    }

    /// Esc while fetching drops the picker; the reply still arrives, and must
    /// not reopen it — the same rule as an aborted `web_search`.
    #[test]
    fn a_models_response_arriving_after_esc_is_dropped() {
        let mut state = fetching_models();
        state.picker_key(KeyWithModifier::new(BareKey::Esc));

        let rendered = state.on_response(200, br#"{"data":[{"id":"gpt-4o"}]}"#, &models_context());

        assert!(!rendered);
        assert!(state.picker.is_none());
        assert_eq!(state.cfg.as_ref().unwrap().model, api::DEFAULT_MODEL);
    }

    /// The gate that keeps a stream from writing the file once per socket read.
    #[test]
    fn autosave_holds_off_until_the_reply_has_landed() {
        let mut state = ready_state();
        state.messages = vec![Message::user("hi"), Message::assistant("partial")];
        state.autosave();
        assert_eq!(state.saved, (0, 0), "still streaming");

        settle(&mut state);
        state.autosave();
        assert_eq!(state.saved, state.fingerprint(), "settled, so recorded");

        let recorded = state.saved;
        state.autosave();
        assert_eq!(
            state.saved, recorded,
            "and an unchanged transcript is a no-op"
        );
    }

    #[test]
    fn an_untouched_pane_leaves_nothing_behind() {
        let mut state = State::default();
        state.autosave();
        assert_eq!(state.chat_id, None, "no file was ever opened");
    }

    #[test]
    fn scrolling_down_stops_at_the_bottom() {
        let mut state = State::default();
        state.scroll_down(5);
        assert_eq!(state.scroll_offset(), 0);
    }

    /// One shared instance serves every client, so scrolling has to be told
    /// apart by client or one person reading back drags everyone with them.
    #[test]
    fn each_client_scrolls_its_own_copy() {
        let mut state = State::default();

        state.client = 1;
        state.scroll_up(4);
        state.client = 2;
        state.scroll_up(9);

        assert_eq!(state.scroll_offset(), 9);
        state.client = 1;
        assert_eq!(state.scroll_offset(), 4);

        // A new message is everyone's, so it puts everyone back on the tail.
        state.pin_to_bottom();
        assert_eq!(state.scroll_offset(), 0);
        state.client = 2;
        assert_eq!(state.scroll_offset(), 0);
    }

    // -----------------------------------------------------------------------
    // Scheduled jobs
    // -----------------------------------------------------------------------

    fn idle_state() -> State {
        State {
            status: Status::Idle,
            stream: None,
            reply_at: None,
            messages: Vec::new(),
            ..ready_state()
        }
    }

    fn schedule_call(arguments: &str) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: "schedule".into(),
            arguments: arguments.to_owned(),
        }
    }

    #[test]
    fn a_job_fires_only_once_its_interval_has_elapsed() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check pane 3"}"#));
        assert_eq!(state.jobs.len(), 1);

        assert!(!state.on_timer(1.0), "a second in, nothing to do");
        assert!(state.messages.is_empty());

        assert!(state.on_timer(60.0), "now it is due");
        assert_eq!(
            state.messages.first().map(|m| (m.role, m.content.as_str())),
            Some((Role::User, "[scheduled] check pane 3"))
        );
        assert!(state.is_receiving(), "and it sent");
        assert_eq!(state.jobs[0].due_in, 60.0, "rearmed for the next run");
    }

    #[test]
    fn an_unrestricted_job_runs_gated_tools_without_confirmation() {
        let mut state = idle_state();
        state.run_manual(r#"/schedule seconds=5 prompt=check auto_confirm=true"#);
        state.messages.clear();
        assert!(state.on_timer(MIN_JOB_SECONDS));

        let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(!matches!(state.status, Status::ConfirmingTool));
        assert_eq!(tool_results(&state).len(), 1);

        state.on_chunk(
            "hocus-2",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n",
            false,
            None,
        );
        state.on_chunk("hocus-2", b"", true, None);
        state.input.set_text("ordinary question".to_owned());
        state.submit();
        let frame = tool_call_frame("call_2", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
        state.on_chunk("hocus-3", &frame, false, None);
        state.on_chunk("hocus-3", b"", true, None);

        assert!(matches!(state.status, Status::ConfirmingTool));
    }

    #[test]
    fn an_unrestricted_job_can_create_another_unrestricted_job() {
        let mut state = idle_state();
        state.run_manual(r#"/schedule seconds=5 prompt=check auto_confirm=true"#);
        state.messages.clear();
        assert!(state.on_timer(MIN_JOB_SECONDS));

        let frame = tool_call_frame(
            "call_1",
            "schedule",
            r#"{"seconds":60,"prompt":"more","auto_confirm":true}"#,
        );
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(!matches!(state.status, Status::ConfirmingTool));
        assert_eq!(state.jobs.len(), 2);
        assert!(state.jobs[1].auto_confirm);
    }

    #[test]
    fn a_due_job_waits_for_the_reply_it_would_interrupt() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));
        state.stream = state_streaming(Vec::new(), "hocus-9").stream;

        state.on_timer(90.0);
        assert!(state.messages.is_empty(), "one stream slot, so it holds");
        assert!(state.jobs[0].due_in <= 0.0, "still due");

        state.stream = None;
        assert!(state.on_timer(0.1), "and fires on the next tick");
        assert_eq!(state.messages.first().map(|m| m.role), Some(Role::User));
    }

    #[test]
    fn a_quiet_job_tick_asks_for_no_render() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":600,"prompt":"check"}"#));
        state.timer_armed = true;

        assert!(!state.on_timer(1.0), "nothing changed on screen");
        assert_eq!(state.tick, 0, "and the spinner did not advance");
        assert!(state.timer_armed, "but the countdown keeps running");
    }

    #[test]
    fn an_absurd_interval_is_clamped_and_a_promptless_job_is_refused() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":0.2,"prompt":"check"}"#));
        assert_eq!(state.jobs[0].every, MIN_JOB_SECONDS);

        state.run_local(&schedule_call(r#"{"seconds":60}"#));
        assert_eq!(state.jobs.len(), 1, "no prompt, no job");
    }

    #[test]
    fn jobs_are_cancelled_by_the_tool_by_the_command_and_by_a_new_chat() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"a"}"#));
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"b"}"#));

        assert_eq!(state.unschedule(Some(1)), "cancelled job 1");
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.unschedule(Some(1)), "no job 1", "and it stays gone");

        assert!(state.run_manual("/stop_jobs"), "the slash command clears the rest");
        assert!(state.jobs.is_empty());
        assert!(!state.run_manual("/stop_jobs"), "and does nothing when there are none");

        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"c"}"#));
        state.new_chat();
        assert!(
            state.jobs.is_empty(),
            "a new chat takes its watchers with it"
        );
    }

    #[test]
    fn ctrl_g_is_not_handled_by_hocus() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));

        assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()));
        assert_eq!(state.jobs.len(), 1);
    }

    #[test]
    fn ctrl_backtick_is_the_hocus_toggle_key() {
        assert!(State::is_toggle_key(
            &KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier()
        ));
        assert!(!State::is_toggle_key(
            &KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()
        ));
    }

    #[test]
    fn only_the_stateful_tools_are_handled_locally() {
        let mut state = idle_state();
        let call = ToolCall {
            id: "1".into(),
            name: "read_pane".into(),
            arguments: "{}".into(),
        };
        assert_eq!(state.run_local(&call), None, "the rest go to the host");
    }

    // -- projects -----------------------------------------------------------

    #[test]
    fn project_assignment_remembers_the_root_and_uses_the_active_tab() {
        let mut state = idle_state();
        state.tabs = vec![TabInfo {
            active: true,
            tab_id: 9,
            ..TabInfo::default()
        }];
        let result = state.run_local(&agent_call(
            "project_assign",
            r#"{"name":"hocus","root":"/work/hocus"}"#,
        ));

        assert_eq!(result, Some("assigned tab 9 to project hocus".to_owned()));
        assert_eq!(state.projects["hocus"].root, "/work/hocus");
        assert_eq!(state.tab_projects[&9], "hocus");
        assert_eq!(state.active_project.as_deref(), Some("hocus"));
    }

    #[test]
    fn an_unassigned_active_tab_clears_the_project_context() {
        let mut state = idle_state();
        state.tab_projects.insert(9, "hocus".to_owned());
        state.on_tabs(vec![TabInfo {
            active: true,
            tab_id: 9,
            ..TabInfo::default()
        }]);
        assert_eq!(state.active_project.as_deref(), Some("hocus"));

        state.on_tabs(vec![TabInfo {
            active: true,
            tab_id: 10,
            ..TabInfo::default()
        }]);
        assert!(state.active_project.is_none());
        assert!(state.tab_projects.is_empty(), "closed tab assignment was removed");
    }

    #[test]
    fn project_agent_lists_use_pane_folders() {
        let mut state = state_with_panes();
        state.projects.insert(
            "hocus".to_owned(),
            projects::validate("hocus", "/work/hocus").unwrap(),
        );
        state.panes[0].cwd = Some("/work/hocus/src".to_owned());
        state.panes[1].cwd = Some("/work/other".to_owned());
        state.manual_kinds.insert(4, "reviewer".to_owned());

        let list = state
            .run_local(&agent_call("agent_list", r#"{"project":"hocus"}"#))
            .unwrap();
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["pane_id"], 3);
    }

    /// An agent pane in a folder, in a tab, as a poll would report it.
    fn agent_in(pane_id: u32, cwd: &str, tab_id: usize) -> agents::AgentPane {
        agents::AgentPane {
            pane_id,
            title: "codex".to_owned(),
            command: Some("codex".to_owned()),
            cwd: Some(cwd.to_owned()),
            status: agents::AgentStatus::Running,
            tab: Some((tab_id, format!("tab {tab_id}"))),
        }
    }

    /// A state with one tab, active, that the poll can assign.
    fn state_with_tab(tab_id: usize) -> State {
        let mut state = idle_state();
        state.tabs = vec![TabInfo {
            active: true,
            tab_id,
            ..TabInfo::default()
        }];
        state
    }

    fn poll(state: &mut State, panes: Vec<agents::AgentPane>) {
        state.apply_poll(panes, agents::Activity::new());
    }

    #[test]
    fn an_agent_folder_becomes_a_project_on_the_poll() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

        assert_eq!(state.projects["hocus"].root, "/work/hocus");
        assert!(state.projects["hocus"].discovered);
    }

    #[test]
    fn a_shell_folder_becomes_no_project() {
        let mut state = state_with_tab(9);
        let shell = agents::AgentPane {
            title: "zsh".to_owned(),
            command: Some("zsh".to_owned()),
            ..agent_in(4, "/work/other", 9)
        };
        poll(&mut state, vec![shell]);

        assert!(state.projects.is_empty());
    }

    #[test]
    fn a_tab_whose_agents_share_a_project_is_assigned_automatically() {
        let mut state = state_with_tab(9);
        poll(
            &mut state,
            vec![agent_in(3, "/work/hocus", 9), agent_in(4, "/work/hocus", 9)],
        );

        assert_eq!(state.auto_tab_projects[&9], "hocus");
        assert!(state.tab_projects.is_empty(), "nothing was assigned by hand");
        assert_eq!(state.active_project.as_deref(), Some("hocus"));
    }

    #[test]
    fn a_tab_with_agents_from_two_projects_gets_no_assignment() {
        let mut state = state_with_tab(9);
        poll(
            &mut state,
            vec![agent_in(3, "/work/hocus", 9), agent_in(4, "/other/pocus", 9)],
        );

        assert_eq!(state.projects.len(), 2);
        assert!(state.auto_tab_projects.is_empty());
        assert!(state.active_project.is_none());
    }

    #[test]
    fn an_explicit_assignment_wins_over_the_automatic_one() {
        let mut state = state_with_tab(9);
        state.run_local(&agent_call(
            "project_assign",
            r#"{"name":"named","root":"/work/named"}"#,
        ));
        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

        assert_eq!(state.tab_project(9), Some("named"));
        assert_eq!(state.active_project.as_deref(), Some("named"));
        assert!(
            state.auto_tab_projects.is_empty(),
            "an assigned tab is left alone",
        );
    }

    #[test]
    fn a_closed_tab_drops_its_automatic_assignment() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
        assert_eq!(state.auto_tab_projects[&9], "hocus");

        state.on_tabs(vec![TabInfo {
            active: true,
            tab_id: 10,
            ..TabInfo::default()
        }]);
        assert!(state.auto_tab_projects.is_empty());
        assert!(state.active_project.is_none());
    }

    #[test]
    fn naming_a_discovered_project_leaves_one_project_on_the_root() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
        assert_eq!(state.active_project.as_deref(), Some("hocus"));

        let result = state.run_local(&agent_call(
            "project_assign",
            r#"{"name":"pocus","root":"/work/hocus"}"#,
        ));

        assert_eq!(result, Some("assigned tab 9 to project pocus".to_owned()));
        assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["pocus"]);
        assert!(!state.projects["pocus"].discovered);
        assert_eq!(state.active_project.as_deref(), Some("pocus"));
    }

    /// The case the rename bookkeeping exists for: a tab points at a project a
    /// later poll absorbs, and must follow it rather than name nothing.
    #[test]
    fn absorbed_project_moves_its_tab() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus/src", 9)]);
        assert_eq!(state.active_project.as_deref(), Some("src"));

        poll(
            &mut state,
            vec![agent_in(3, "/work/hocus/src", 9), agent_in(4, "/work/hocus", 9)],
        );

        assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
        assert_eq!(state.auto_tab_projects[&9], "hocus");
        assert_eq!(state.active_project.as_deref(), Some("hocus"));
    }

    #[test]
    fn an_agent_list_names_the_folder_and_the_project() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus/src", 9)]);

        let list = state.run_local(&agent_call("agent_list", "{}")).unwrap();
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(list[0]["cwd"], "/work/hocus/src");
        assert_eq!(list[0]["project"], "src");
    }

    // -- todo lists ---------------------------------------------------------

    /// A state whose active project is `hocus` at `/work/hocus`, assigned by
    /// hand, so the todo tools have somewhere to write.
    fn state_with_project() -> State {
        let mut state = state_with_tab(9);
        state.run_local(&agent_call(
            "project_assign",
            r#"{"name":"hocus","root":"/work/hocus"}"#,
        ));
        state
    }

    fn todo(state: &mut State, name: &str, arguments: &str) -> serde_json::Value {
        let result = state.run_local(&agent_call(name, arguments)).unwrap();
        serde_json::from_str(&result)
            .unwrap_or_else(|_| panic!("{name} answered with an error: {result}"))
    }

    #[test]
    fn a_todo_tool_says_when_there_is_no_active_project() {
        let mut state = idle_state();
        assert_eq!(
            state.run_local(&agent_call("todo_add", r#"{"text":"ship it"}"#)),
            Some("no active project; give project, or call project_list".to_owned()),
        );
    }

    #[test]
    fn a_todo_item_lands_under_the_active_project_root() {
        let mut state = state_with_project();
        let added = todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);

        assert_eq!(added["added"]["id"], 1);
        assert_eq!(added["project"], "hocus");
        assert_eq!(added["root"], "/work/hocus");
        assert_eq!(state.todos["/work/hocus"].items[0].text, "ship the parser");
    }

    #[test]
    fn a_todo_tool_rejects_a_project_name_it_does_not_know() {
        let mut state = state_with_project();
        assert_eq!(
            state.run_local(&agent_call(
                "todo_add",
                r#"{"text":"ship it","project":"nowhere"}"#,
            )),
            Some("unknown project nowhere; call project_list".to_owned()),
        );
    }

    #[test]
    fn a_todo_tool_needs_its_key_argument() {
        let mut state = state_with_project();
        assert_eq!(
            state.run_local(&agent_call("todo_add", "{}")),
            Some("text is required".to_owned()),
        );
        assert_eq!(
            state.run_local(&agent_call("todo_search", "{}")),
            Some("query is required".to_owned()),
        );
        assert_eq!(
            state.run_local(&agent_call("todo_done", "{}")),
            Some("id is required".to_owned()),
        );
    }

    /// The reason the file is keyed on the folder and not on the name.
    #[test]
    fn a_todo_list_follows_a_project_the_user_renames() {
        let mut state = state_with_project();
        todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
        state.run_local(&agent_call(
            "project_assign",
            r#"{"name":"pocus","root":"/work/hocus"}"#,
        ));

        let list = todo(&mut state, "todo_list", r#"{"project":"pocus"}"#);
        assert_eq!(list["items"][0]["text"], "ship the parser");
    }

    /// The other way a name moves: a poll finds a shallower folder and takes
    /// the deeper project's name away.
    #[test]
    fn a_todo_list_survives_a_root_that_absorbs_its_project() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus/src", 9)]);
        todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);

        poll(
            &mut state,
            vec![agent_in(3, "/work/hocus/src", 9), agent_in(4, "/work/hocus", 9)],
        );
        assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["hocus"]);

        // The list is still there, under the folder it was made in.
        let list = todo(&mut state, "todo_list", r#"{"project":"/work/hocus/src"}"#);
        assert_eq!(list["project"], serde_json::Value::Null);
        assert_eq!(list["items"][0]["text"], "ship the parser");
    }

    #[test]
    fn a_todo_list_comes_back_as_a_tree_with_ids() {
        let mut state = state_with_project();
        todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
        todo(
            &mut state,
            "todo_add",
            r#"{"text":"write the test","parent_id":1}"#,
        );

        let list = todo(&mut state, "todo_list", "{}");
        assert_eq!(list["open"], 2);
        assert_eq!(list["items"][0]["id"], 1);
        assert_eq!(list["items"][0]["children"][0]["id"], 2);
        assert_eq!(list["items"][0]["children"][0]["text"], "write the test");
    }

    #[test]
    fn crossing_an_item_off_crosses_off_the_items_below_it() {
        let mut state = state_with_project();
        todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
        todo(
            &mut state,
            "todo_add",
            r#"{"text":"write the test","parent_id":1}"#,
        );

        let change = todo(&mut state, "todo_done", r#"{"id":1}"#);
        assert_eq!(change["done"], true);
        assert_eq!(change["also_done"], serde_json::json!([2]));

        let open = todo(&mut state, "todo_list", "{}");
        assert!(open["items"].as_array().unwrap().is_empty());
        let all = todo(&mut state, "todo_list", r#"{"include_done":true}"#);
        assert_eq!(all["done"], 2);
    }

    #[test]
    fn an_undo_opens_the_item_and_the_item_above_it() {
        let mut state = state_with_project();
        todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
        todo(
            &mut state,
            "todo_add",
            r#"{"text":"write the test","parent_id":1}"#,
        );
        todo(&mut state, "todo_done", r#"{"id":1}"#);

        let change = todo(&mut state, "todo_done", r#"{"id":2,"undo":true}"#);
        assert_eq!(change["done"], false);
        assert_eq!(change["reopened"], serde_json::json!([1]));
        assert_eq!(todo(&mut state, "todo_list", "{}")["open"], 2);
    }

    #[test]
    fn a_search_names_the_project_and_the_items_above_the_match() {
        let mut state = state_with_project();
        todo(&mut state, "todo_add", r#"{"text":"release 2"}"#);
        todo(
            &mut state,
            "todo_add",
            r#"{"text":"ship the parser","parent_id":1}"#,
        );

        let found = todo(&mut state, "todo_search", r#"{"query":"PARSER"}"#);
        assert_eq!(found["scope"], "hocus");
        assert_eq!(found["matches"][0]["id"], 2);
        assert_eq!(found["matches"][0]["project"], "hocus");
        assert_eq!(
            found["matches"][0]["trail"],
            serde_json::json!(["release 2", "ship the parser"]),
        );
    }

    #[test]
    fn a_search_over_every_project_finds_a_list_whose_project_is_gone() {
        let mut state = state_with_project();
        todo(
            &mut state,
            "todo_add",
            r#"{"text":"ship the parser","project":"/work/gone"}"#,
        );
        state.projects.clear();
        state.active_project = None;

        let found = todo(&mut state, "todo_search", r#"{"query":"parser","all_projects":true}"#);
        assert_eq!(found["scope"], "all");
        assert_eq!(found["matches"][0]["root"], "/work/gone");
        assert_eq!(found["matches"][0]["project"], serde_json::Value::Null);
    }

    #[test]
    fn project_agent_spawn_rejects_a_folder_outside_the_root() {
        let mut state = idle_state();
        state.projects.insert(
            "hocus".to_owned(),
            projects::validate("hocus", "/work/hocus").unwrap(),
        );
        assert_eq!(
            state.run_local(&agent_call(
                "agent_spawn",
                r#"{"command":"codex","project":"hocus","cwd":"/work/other"}"#,
            )),
            Some("cwd must be inside project hocus".to_owned()),
        );
    }

    #[test]
    fn a_project_supplies_the_default_agent_folder() {
        let mut state = idle_state();
        state.projects.insert(
            "hocus".to_owned(),
            projects::validate("hocus", "/work/hocus").unwrap(),
        );
        state.active_project = Some("hocus".to_owned());
        let args = tools::Args::parse(r#"{"command":"codex"}"#).unwrap();
        assert_eq!(state.agent_cwd(&args).unwrap().as_deref(), Some("/work/hocus"));
    }

    #[test]
    fn a_project_tab_uses_the_stable_id_from_zellij() {
        let mut state = idle_state();
        let result = state.register_opened_project(
            "hocus",
            r#"{"opened":true,"tab_id":47}"#,
        );
        assert_eq!(result, r#"{"opened":true,"tab_id":47}"#);
        assert_eq!(state.tab_projects[&47], "hocus");
        assert_eq!(state.active_project.as_deref(), Some("hocus"));
    }

    // -- agents -------------------------------------------------------------

    fn pane_info(id: u32, title: &str, command: Option<&str>) -> PaneInfo {
        PaneInfo {
            id,
            title: title.to_owned(),
            terminal_command: command.map(str::to_owned),
            ..PaneInfo::default()
        }
    }

    fn manifest(panes: Vec<PaneInfo>) -> PaneManifest {
        PaneManifest {
            panes: std::collections::HashMap::from([(0, panes)]),
            ..PaneManifest::default()
        }
    }

    fn tab_info(position: usize, tab_id: usize, name: &str) -> TabInfo {
        TabInfo {
            position,
            tab_id,
            name: name.to_owned(),
            ..TabInfo::default()
        }
    }

    /// A state that has seen one `codex` pane and one plain shell.
    fn state_with_panes() -> State {
        let mut state = idle_state();
        state.set_panes(terminal_panes(
            &manifest(vec![
                pane_info(3, "codex", Some("codex --resume")),
                pane_info(4, "zsh", Some("zsh")),
            ]),
            &[],
        ));
        state
    }

    fn agent_call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        }
    }

    /// A global pane is in no tab, so it is in its own field of the manifest. Missing
    /// it is how an agent beside a global Hocus would stay invisible.
    #[test]
    fn a_snapshot_keeps_terminal_panes_of_every_scope() {
        let plugin = PaneInfo {
            is_plugin: true,
            ..pane_info(1, "hocus", None)
        };
        let suppressed = PaneInfo {
            is_suppressed: true,
            ..pane_info(2, "hidden", Some("codex"))
        };
        let held = PaneInfo {
            is_held: true,
            exited: true,
            ..pane_info(3, "codex", Some("codex"))
        };
        let global = PaneManifest {
            global_panes: vec![pane_info(5, "aider", Some("aider"))],
            ..manifest(vec![plugin, suppressed, held])
        };

        let panes = terminal_panes(&global, &[tab_info(0, 9, "work")]);
        assert_eq!(
            panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
            vec![3, 5],
        );
        assert_eq!(panes[0].status, agents::AgentStatus::Held, "held beats exited");
        assert_eq!(panes[0].tab, Some((9, "work".to_owned())));
        assert_eq!(panes[1].tab, None, "a global pane is in no tab");
    }

    #[test]
    fn agent_register_rejects_a_pane_that_is_not_there_and_a_kind_that_is_not_a_word() {
        let mut state = state_with_panes();
        assert_eq!(
            state.run_local(&agent_call(
                "agent_register",
                r#"{"pane_id":9,"kind":"codex"}"#
            )),
            Some("pane 9 is not available".into()),
        );
        assert_eq!(
            state.run_local(&agent_call(
                "agent_register",
                r#"{"pane_id":4,"kind":"code review"}"#
            )),
            Some("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into()),
        );
        assert!(state.manual_kinds.is_empty());
    }

    #[test]
    fn a_manual_kind_lasts_until_its_pane_closes() {
        let mut state = state_with_panes();
        assert_eq!(
            state.run_local(&agent_call(
                "agent_register",
                r#"{"pane_id":4,"kind":"reviewer"}"#
            )),
            Some("registered pane 4 as reviewer".into()),
        );

        // A retitled pane is still the same pane, so the kind stays.
        state.set_panes(terminal_panes(
            &manifest(vec![pane_info(4, "new title", Some("zsh"))]),
            &[],
        ));
        assert_eq!(
            state.manual_kinds.get(&4).map(String::as_str),
            Some("reviewer")
        );

        state.set_panes(Vec::new());
        assert!(state.manual_kinds.is_empty(), "the pane closed, so it goes");
    }

    /// Off wasm every host call is a stub, so this checks which panes were listed and
    /// what was said about them, not what they were showing.
    #[test]
    fn agent_list_covers_detected_and_registered_panes_only() {
        let mut state = state_with_panes();
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":4,"kind":"reviewer"}"#,
        ));

        let list = state.run_local(&agent_call("agent_list", "{}")).unwrap();
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let list = list.as_array().unwrap();

        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["pane_id"], 3);
        assert_eq!(list[0]["kind"], "codex");
        assert_eq!(list[0]["source"], "auto");
        assert_eq!(list[0]["status"], "running");
        assert_eq!(list[1]["pane_id"], 4);
        assert_eq!(list[1]["kind"], "reviewer");
        assert_eq!(list[1]["source"], "manual");
    }

    #[test]
    fn agent_list_is_empty_before_the_first_pane_update() {
        let mut state = idle_state();
        assert_eq!(
            state.run_local(&agent_call("agent_list", "{}")),
            Some("[]".into()),
        );
    }

    #[test]
    fn agent_spawn_rejects_no_command_and_a_kind_that_is_not_a_word() {
        let mut state = idle_state();
        assert_eq!(
            state.run_local(&agent_call("agent_spawn", "{}")),
            Some("command is required".into()),
        );
        assert_eq!(
            state.run_local(&agent_call(
                "agent_spawn",
                r#"{"command":"zsh","kind":"code review"}"#
            )),
            Some("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into()),
        );
        assert!(state.manual_kinds.is_empty());
    }

    /// The dispatch itself needs the host, so what is tested here is the half that
    /// does not: folding `new_pane`'s answer back into the kinds.
    #[test]
    fn a_spawned_kind_is_registered_against_the_pane_new_pane_reported() {
        let mut state = idle_state();
        assert_eq!(
            state.register_spawned(Some("reviewer"), r#"{"opened":true,"pane_id":7,"is_plugin":false}"#),
            r#"{"opened":true,"pane_id":7,"is_plugin":false} (registered as reviewer)"#,
        );
        assert_eq!(
            state.manual_kinds.get(&7).map(String::as_str),
            Some("reviewer")
        );

        // No kind, or no pane in the answer: the result passes through untouched.
        assert_eq!(
            state.register_spawned(None, r#"{"opened":true,"pane_id":8,"is_plugin":false}"#),
            r#"{"opened":true,"pane_id":8,"is_plugin":false}"#,
        );
        assert_eq!(
            state.register_spawned(Some("scout"), "zellij did not open the pane"),
            "zellij did not open the pane",
        );
        assert_eq!(state.manual_kinds.len(), 1);
    }

    /// The whole of `agent_control`, compared as the complete host call it becomes.
    /// A wrong pane kind or a wrong tool here is a killed pane.
    #[test]
    fn every_agent_action_maps_onto_the_tool_that_already_does_it() {
        let state = state_with_panes();
        for (arguments, name, mapped) in [
            (
                r#"{"pane_id":3,"text":"go","keys":["enter"]}"#,
                "send_input",
                r#"{"is_plugin":false,"keys":["enter"],"pane_id":3,"text":"go"}"#,
            ),
            (
                r#"{"pane_id":3,"action":"interrupt"}"#,
                "send_input",
                r#"{"is_plugin":false,"keys":["ctrl_c"],"pane_id":3}"#,
            ),
            (
                r#"{"pane_id":3,"action":"restart"}"#,
                "rerun_pane",
                r#"{"is_plugin":false,"pane_id":3}"#,
            ),
            (
                r#"{"pane_id":3,"action":"close"}"#,
                "close_pane",
                r#"{"is_plugin":false,"pane_id":3}"#,
            ),
        ] {
            let call = agent_call(
                if arguments.contains("action") {
                    "agent_control"
                } else {
                    "agent_send"
                },
                arguments,
            );
            let args = tools::Args::parse(arguments).unwrap();
            let host = state.agent_host_call(&call, &args).unwrap();
            assert_eq!(host.name, name, "{arguments}");
            assert_eq!(host.arguments, mapped, "{arguments}");
            assert_eq!(host.id, call.id, "the result still answers the call");
        }
    }

    #[test]
    fn agent_tools_refuse_a_pane_that_is_not_an_agent_and_an_action_that_is_not_one() {
        let mut state = state_with_panes();
        assert_eq!(
            state.run_local(&agent_call(
                "agent_control",
                r#"{"pane_id":3,"action":"pause"}"#
            )),
            Some("unknown agent action: pause".into()),
        );
        // Pane 4 is a plain shell that nobody registered.
        assert_eq!(
            state.run_local(&agent_call("agent_send", r#"{"pane_id":4,"text":"go"}"#)),
            Some("pane 4 is not an agent; call agent_list, or agent_register it first".into()),
        );
        assert_eq!(
            state.run_local(&agent_call("agent_send", r#"{"text":"go"}"#)),
            Some("pane_id is required".into()),
        );
    }

    /// A closed pane leaves the snapshot, and with it every way to send to it.
    #[test]
    fn a_closed_agent_can_no_longer_be_sent_to() {
        let mut state = state_with_panes();
        assert!(state.is_agent(3));
        state.set_panes(Vec::new());
        assert!(!state.is_agent(3));
    }

    #[test]
    fn a_model_agent_action_is_confirmed_and_a_typed_one_is_not() {
        for (name, arguments) in [
            ("agent_send", r#"{"pane_id":3,"text":"go"}"#),
            ("agent_control", r#"{"pane_id":3,"action":"close"}"#),
        ] {
            let mut state = State {
                stream: state_streaming(Vec::new(), "hocus-1").stream,
                reply_at: Some(1),
                messages: vec![Message::user("hi"), Message::assistant("")],
                ..state_with_panes()
            };
            let frame = tool_call_frame("call_1", name, arguments);
            state.on_chunk("hocus-1", &frame, false, None);
            state.on_chunk("hocus-1", b"", true, None);

            assert!(matches!(state.status, Status::ConfirmingTool), "{name}");
            assert!(tool_results(&state).is_empty(), "{name} ran unasked");
        }

        // Typing the command is the confirmation, so it runs on the spot.
        let mut state = state_with_panes();
        state.run_manual(r#"/agent_control pane_id=3 action=close"#);
        assert!(!matches!(state.status, Status::ConfirmingTool));
        assert_eq!(tool_results(&state).len(), 1);
    }

    /// The poll request and reply cross the worker boundary as JSON; a field
    /// that stops round-tripping silently kills the whole poll.
    #[test]
    fn poll_reply_round_trips_through_json() {
        let mut activity = agents::Activity::new();
        activity.insert(7, (0xdead_beef, 3));
        let reply = PollReply {
            panes: Some(vec![agents::AgentPane {
                pane_id: 7,
                title: "claude".to_owned(),
                command: Some("claude --resume".to_owned()),
                status: agents::AgentStatus::Running,
                tab: Some((2, "work".to_owned())),
                cwd: Some("/tmp/work".to_owned()),
            }]),
            activity,
        };
        let json = serde_json::to_string(&reply).expect("serialize");
        let back: PollReply = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.panes, reply.panes);
        assert_eq!(back.activity, reply.activity);
    }

    /// A sync poll supersedes a poke in flight: the worker's late reply must not
    /// resurrect panes the sync poll already saw close.
    #[test]
    fn a_superseded_poll_reply_is_dropped() {
        let mut state = state_with_panes();
        let reply = serde_json::to_string(&PollReply {
            panes: Some(Vec::new()),
            activity: agents::Activity::new(),
        })
        .expect("serialize");

        state.poll_pending = false;
        assert!(!state.on_custom_message("poll_result", &reply));
        assert!(!state.panes.is_empty(), "stale reply must not merge");

        state.poll_pending = true;
        state.on_custom_message("poll_result", &reply);
        assert!(state.panes.is_empty(), "live reply must merge");
        assert!(!state.poll_pending);
    }
}
