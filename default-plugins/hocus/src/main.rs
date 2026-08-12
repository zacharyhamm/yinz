//! hocus — chat with an OpenAI-compatible endpoint from a Zellij popup.

mod api;
mod archive;
mod events;
mod history;
mod log;
mod message;
mod text_input;
mod tool_loop;
mod tools;
mod transitions;
mod ui;
mod worker;

pub use message::{
    Job, Message, Picker, PickerKind, PickerRow, Role, RowKind, Status, StreamState,
};
use worker::{post_log, post_prompt_history, post_save};

/// The pure half of hocus, shared with `hocus-agentd` and `hocus-store`.
/// Re-exported under the names the code already used when these were modules.
use hocus_proto::wire::{self, PollReply, PollRequest};
use hocus_proto::{agents, projects, todos};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use zellij_tile::prelude::*;

use api::{Config, ConfigError, SseItem, SseParser, ToolCallAccumulator};
use text_input::{InputAction, TextInput};
use tools::ToolCall;

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

/// How long a tool call waits for hocus-store before giving up. Generous for a
/// pipe round trip on the same machine, short enough that a missing store is a
/// visible error rather than an apparently hung model.
const STORE_TIMEOUT_SECONDS: f64 = 5.0;

/// Slash commands that drive the plugin itself rather than a tool. Typed and
/// completed like tools, but never offered to the model.
const LOCAL_COMMANDS: [&str; 4] = ["history", "models", "stop_jobs", "keys"];

/// Full keybinding reference for `/keys`. The chat view has no persistent
/// help line, so this is the one place all of these are written down.
const KEYS_HELP: &str = "\
Ctrl+`        toggle hocus
Ctrl+l        new chat
Ctrl+r        agent picker
Up / Down     previous / next message in history
Tab           complete a /command or @agent
Alt+Enter, Shift+Enter   insert a newline
PageUp / PageDown        scroll the transcript
Esc           cancel a stream or tool, or hide hocus
y / n         confirm or decline a pending tool call

In a picker (history, models, agents):
Up / Down     choose
Enter         select
Esc           back";

/// Context key carrying a chat request's stream id back on its `WebRequestResult`,
/// so the response can be matched to the chat that asked.
const STREAM_CONTEXT: &str = "stream";

/// One conversation's worth of `State`, lifted out so a chat can be set aside
/// while another is on screen and still receive its reply.
///
/// The foreground chat's fields stay flat on `State` — moving every
/// `self.messages` behind `self.chat.messages` would churn most of this file
/// and `ui.rs` for nothing. This is only the shape a chat takes while it is
/// parked, and the shape the swap moves.
#[derive(Default)]
struct Chat {
    pub(crate) messages: Vec<Message>,
    pub(crate) status: Status,
    pub(crate) stream: Option<StreamState>,
    pub(crate) reply_at: Option<usize>,
    pub(crate) sse: SseParser,
    pub(crate) tool_calls: ToolCallAccumulator,
    pub(crate) pending: VecDeque<ToolCall>,
    pub(crate) tool_rounds: usize,
    pub(crate) owes_reply: bool,
    pub(crate) manual_head: bool,
    pub(crate) chat_id: Option<u64>,
    pub(crate) saved: (usize, usize),
    pub(crate) scroll: BTreeMap<ClientId, usize>,
    pub(crate) job_auto_confirm: bool,
}

impl Chat {
    /// Whether this chat is still owed something — the only reason to hold it
    /// rather than let the archive have it back.
    pub(crate) fn is_live(&self) -> bool {
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
    /// Whether the head of `pending` is a call the user typed. Its result is
    /// rendered whole rather than elided, and the head is where that has to be
    /// remembered: a deferred tool answers through `on_command_result` long
    /// after `run_manual` returned.
    ///
    /// A flag rather than a mark on the call itself, for the same reason as
    /// `owes_reply`: `ToolCall` is archived and reserialized, and this is a
    /// property of how we ran it, not of what was asked for.
    manual_head: bool,
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
    /// Terminal panes from hocus-agentd's latest event-fed snapshot. This is the
    /// only snapshot agent tools trust, which stops them acting on a closed pane.
    panes: Vec<agents::AgentPane>,
    /// Agent CWDs resolved by hocus-agentd to Git checkout identities.
    repositories: BTreeMap<String, projects::Repository>,
    /// Kinds the user set by hand with `agent_register`, keyed by pane id. These win
    /// over detection, and are dropped when their pane leaves the snapshot.
    manual_kinds: BTreeMap<u32, String>,
    /// Spawned panes whose ids are allocated but have not reached a pane snapshot yet.
    /// Their manual kinds survive early snapshots and are pruned normally once seen.
    pending_agent_panes: BTreeSet<u32>,
    /// Consecutive unchanged scrollback observations from hocus-agentd.
    activity: agents::Activity,
    /// The tool call waiting on hocus-store, and how long it has left. At most
    /// one: the tool queue runs a single call at a time.
    pending_write: Option<(String, f64)>,
    /// Highest `seq` merged from hocus-store, same watermark rule as `agent_seq`.
    store_seq: u64,
    /// Highest `seq` merged from hocus-agentd. A daemon reload restarts its
    /// counter, so re-subscribing resets this watermark.
    agent_seq: u64,
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
            manual_head: false,
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
            repositories: BTreeMap::new(),
            manual_kinds: BTreeMap::new(),
            pending_agent_panes: BTreeSet::new(),
            activity: BTreeMap::new(),
            pending_write: None,
            store_seq: 0,
            agent_seq: 0,
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
        // projects and todos belong to hocus-store now; ours is a read-only
        // copy, filled by the first snapshot it pushes after we subscribe.
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
            // Tools: query the session, mutate it, open panes, shell out to
            // dobbs for the Slack tools, read pane output, and send pane input,
            // in that order.
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::RunCommands,
            PermissionType::ReadPaneContents,
            PermissionType::WriteToStdin,
            // Deliberately *not* MessageAndLaunchOtherPlugins, though talking to
            // hocus-agentd would nominally need it. A builtin is granted every
            // gated call regardless (see `check_command_permission`), so adding
            // it buys nothing — while `check_permissions` demands an exact match
            // against the cached grant, so a longer list re-prompts every
            // existing user for a permission that was never enforced. A hocus
            // loaded from a `file:` path is not a builtin and so loses every
            // pipe: agents fall back to polling inline, but the store has no
            // fallback, so projects and todos never arrive and writes time out.
            // Not worth re-prompting everyone over — hot-reloading a builtin
            // keeps the `zellij:` location and resolves to the freshly built
            // wasm anyway (see `BUILTIN_PLUGIN_DEV_DIRS`).
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

    /// Snapshots pushed by `hocus-agentd`.
    ///
    /// The daemon and the poll worker both end up in `apply_poll`, which is
    /// idempotent — applying the same snapshot twice changes nothing, and a
    /// transition is only reported against the map we last held. That is what
    /// lets both paths run at once while the daemon proves itself.
    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            wire::STORE_SNAPSHOT => return self.on_store_snapshot(&pipe_message),
            wire::STORE_RESULT => return self.on_store_result(&pipe_message),
            // A store that just (re)started has forgotten us. Nothing to
            // render — the snapshot it pushes back is what does that.
            wire::STORE_HELLO => {
                self.subscribe_to_store();
                return false;
            },
            wire::AGENT_HELLO => {
                self.subscribe_to_agentd();
                return false;
            },
            wire::AGENT_SNAPSHOT => {},
            _ => return false,
        }
        // The daemon pushes snapshots after relevant host events, so this cannot
        // gate on having asked. What it can do is
        // refuse to go backwards: `seq` counts up within one daemon, so a
        // snapshot no newer than the last one applied is a reordered delivery
        // and merging it would resurrect panes we already saw close.
        let seq = pipe_message
            .args
            .get("seq")
            .and_then(|seq| seq.parse::<u64>().ok())
            .unwrap_or(0);
        if seq <= self.agent_seq {
            return false;
        }
        let Some(reply) = pipe_message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<PollReply>(json).ok())
        else {
            return false;
        };
        self.agent_seq = seq;
        self.repositories = reply.repositories;
        let applied = match reply.panes {
            Some(panes) => self.apply_poll(panes, reply.activity),
            // No current session. A quiet reply, not a dead daemon.
            None => false,
        };
        // This is also the answer to the branch refresh an open agent picker
        // asked for, so the list has to take it rather than go on showing the
        // rows it was built from.
        let refreshed = self.refresh_agent_picker();
        applied || refreshed
    }
}

impl State {
    pub(crate) fn handle(&mut self, event: Event) -> bool {
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
                    },
                    None => self.on_response(status, &body, &context),
                }
            },
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
            },
            // Host state, not conversation state: applied to the foreground
            // chat's config only, never routed to a parked chat.
            Event::PluginConfigurationChanged(kdl) => {
                self.kdl = kdl;
                self.reconfigure();
                true
            },
            // The payload is the real time the host slept, which is the only
            // clock a plugin gets — there is no wall clock on offer.
            Event::Timer(elapsed) => self.on_timer(elapsed),
            Event::TabUpdate(tabs) => self.on_tabs(tabs),
            Event::BeforeClose => {
                self.cancel_in_flight();
                if let Some(json) = history::prompt_history_json(self.input.history()) {
                    history::write_prompt_history(json.as_bytes());
                }
                // No projects or todos here any more: hocus-store owns those
                // files and writes them as each change lands, so there is
                // nothing of ours left unsaved to flush.
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
            },
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hocus_proto::panes::terminal_panes;

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
        state
            .input
            .set_text("ask @codex to run the tests".to_owned());
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
            name: "slack_read".into(),
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
            name: "slack_read".into(),
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

    /// The hint must not swallow a real exit-2 from a dobbs that ran.
    #[test]
    fn the_dobbs_hint_is_limited_to_a_failure_to_spawn_a_slack_tool() {
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
            b"grep: no matches",
            &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
        );

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.starts_with("exit code: 2"),
            "got the hint: {}",
            results[0].content
        );
    }

    /// …nor fire for a head that is not a Slack tool. No non-Slack tool can reach
    /// `AwaitingCommand` any more, so the guard is exercised where it lives.
    #[test]
    fn the_dobbs_hint_does_not_fire_for_a_head_that_is_not_a_slack_tool() {
        let mut state = ready_state();
        state.pending = vec![ToolCall {
            id: "mine".into(),
            name: "list_session".into(),
            arguments: "{}".into(),
        }]
        .into();

        assert!(state
            .slack_exec_hint(Some(2), b"No such file or directory (os error 2)")
            .is_none());
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
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.job_auto_confirm = true;
        state.on_response(
            401,
            br#"{"error":{"message":"bad key"}}"#,
            &stream_context("hocus-1"),
        );

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
            name: "slack_backfill".into(),
            arguments: "{}".into(),
        }]
        .into();

        let loader = state.loader().expect("a loader while awaiting a command");
        assert!(loader.contains("running: slack_backfill"), "{loader}");
    }

    /// Unparseable arguments must not lose the indicator entirely.
    #[test]
    fn a_search_we_cannot_parse_falls_back_to_a_phrase() {
        let mut state = ready_state();
        state.status = Status::AwaitingSearch;
        state.pending = vec![ToolCall {
            id: "a".into(),
            name: "web_search".into(),
            arguments: "{not json".into(),
        }]
        .into();

        let loader = state.loader().expect("still a loader");
        assert!(!loader.contains("searching:"), "{loader}");
    }

    /// With no job, store write, or animation, there is no timer to keep alive.
    #[test]
    fn an_idle_timer_tick_stops_without_repainting() {
        let mut state = ready_state();
        settle(&mut state);
        state.timer_armed = true;

        assert!(!state.on_timer(FRAME_SECONDS), "no redraw");
        assert!(!state.timer_armed, "no agent poll keeps the chain alive");
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
        let mut state = state_streaming(
            vec![Message::user("hi"), Message::assistant("partial")],
            "hocus-1",
        );
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
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.on_chunk("hocus-1", b"", true, Some("connection reset".into()));

        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[1].role, Role::Error);
    }

    #[test]
    fn a_silent_stream_reports_that_nothing_came_back() {
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
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
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-2");
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
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
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
        let mut state = state_streaming(
            vec![Message::user("hi"), Message::assistant("some text")],
            "hocus-1",
        );

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
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
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
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        state.on_response(
            401,
            br#"{"error":{"message":"bad key"}}"#,
            &stream_context("hocus-1"),
        );

        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[1].role, Role::Error);
        assert!(state.messages[1].content.contains("bad key"));
        assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
    }

    #[test]
    fn a_2xx_response_changes_nothing_because_the_body_arrives_as_chunks() {
        let mut state =
            state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
        let rendered = state.on_response(200, b"", &BTreeMap::new());

        assert!(!rendered);
        assert_eq!(state.messages.len(), 2);
        assert!(state.is_receiving());
    }

    // -- the archive --------------------------------------------------------

    fn picker_state(titles: &[&str]) -> State {
        State {
            picker: Some(Picker {
                entries: titles.iter().map(|title| PickerRow::item(*title)).collect(),
                selected: 0,
                kind: PickerKind::Chats((1..=titles.len() as u64).collect()),
            }),
            ..ready_state()
        }
    }

    fn picker(state: &State) -> &Picker {
        state.picker.as_ref().expect("the picker is open")
    }

    /// The open picker's rows as plain lines, which is all most of these tests
    /// care about.
    fn entry_texts(state: &State) -> Vec<&str> {
        picker(state)
            .entries
            .iter()
            .map(|row| row.text.as_str())
            .collect()
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
        assert!(
            state.is_receiving(),
            "with the reply still arriving behind it"
        );
    }

    /// The fingerprint is what tells the transcript cache, the archive and every
    /// other client that something changed. It watched the *last* message, which
    /// a tool typed mid-reply is no longer — the reply would grow invisibly.
    #[test]
    fn the_fingerprint_follows_the_reply_and_not_the_last_message() {
        let mut state = ready_state();
        state.input.set_text("/list_session".to_owned());
        state.submit();
        assert!(
            !state.reply_is_last(),
            "the result is behind the placeholder"
        );

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

    /// A typed tool was run for its output; the model's was run for context.
    /// That is the difference the renderer keys on.
    #[test]
    fn a_typed_tools_result_is_marked_for_full_rendering() {
        let mut state = ready_state();
        state.input.set_text("/list_session".to_owned());
        state.submit();

        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].full_output);
        assert!(!state.manual_head, "consumed with the head it belonged to");

        let frame = tool_call_frame("call_1", "list_session", "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        let results = tool_results(&state);
        assert_eq!(results.len(), 2);
        assert!(!results[1].full_output, "the model's is still elided");
    }

    /// The model's calls land when its stream ends, which can be while a typed
    /// one is still running. They queue behind it rather than replacing it, and
    /// the answer still goes back once the queue drains.
    #[test]
    fn model_calls_queue_behind_a_typed_one_that_is_still_running() {
        let mut state = ready_state();
        state.pending.push_back(ToolCall {
            id: "manual-0".into(),
            name: "slack_read".into(),
            arguments: r##"{"channel":"#general"}"##.into(),
        });
        state.status = Status::AwaitingCommand;

        let frame = tool_call_frame("call_1", "list_session", "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert_eq!(state.pending.len(), 2, "queued, not swapped in");
        assert_eq!(
            state.pending[0].id, "manual-0",
            "the running one is still head"
        );
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
        assert_eq!(
            state.messages[1].content, "",
            "the visible chat is untouched"
        );
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
    /// would act on under the headers that group them, stands outside any cycle,
    /// and Enter hands the pane to the focus tool — stubbed off wasm, so what is
    /// checked is that it closes.
    #[test]
    fn ctrl_r_opens_the_agent_picker_and_enter_closes_it() {
        let mut state = state_with_panes();
        settle(&mut state);
        state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier());

        assert_eq!(
            entry_texts(&state),
            ["no project", "  no branch", "  3  codex  running"]
        );
        assert!(
            matches!(picker(&state).kind, PickerKind::Agents(ref ids) if ids == &vec![None, None, Some(3)])
        );
        // Past the two headers, which nothing can land on.
        assert_eq!(picker(&state).selected, 2);

        state.picker_key(KeyWithModifier::new(BareKey::Tab));
        assert!(
            matches!(picker(&state).kind, PickerKind::Agents(_)),
            "no cycle"
        );

        state.picker_key(KeyWithModifier::new(BareKey::Enter));
        assert!(state.picker.is_none());
    }

    /// Two agents in two worktrees of one repository — the case the branch
    /// headers exist for. Nothing names the project, so they group under the
    /// header that says so and are still told apart by their branches.
    fn state_with_worktrees() -> State {
        let mut state = idle_state();
        state.set_panes(vec![
            agent_in(3, "/work/hocus", 1),
            agent_in(5, "/work/trees/topic", 2),
        ]);
        state.repositories = BTreeMap::from([
            ("/work/hocus".to_owned(), on_branch("/work/hocus", "main")),
            (
                "/work/trees/topic".to_owned(),
                on_branch("/work/trees/topic", "topic"),
            ),
        ]);
        state
    }

    /// The pane the open agent picker would focus.
    fn selected_pane(state: &State) -> Option<u32> {
        match &picker(state).kind {
            PickerKind::Agents(ids) => ids.get(picker(state).selected).copied().flatten(),
            _ => None,
        }
    }

    /// Arrow keys step over the headers, so every landing is an agent and there
    /// is never a selection that `Enter` could do nothing with.
    #[test]
    fn the_agent_picker_never_lands_on_a_header() {
        let mut state = state_with_worktrees();
        settle(&mut state);
        state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier());

        assert_eq!(
            entry_texts(&state),
            [
                "no project",
                "  main",
                "  3  codex  running  tab 1",
                "  topic",
                "  5  codex  running  tab 2",
            ]
        );
        assert_eq!(selected_pane(&state), Some(3), "opens on the first agent");

        state.picker_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(selected_pane(&state), Some(5), "over the branch header");
        state.picker_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(selected_pane(&state), Some(5), "and stops at the last");
        state.picker_key(KeyWithModifier::new(BareKey::Up));
        assert_eq!(selected_pane(&state), Some(3));
        state.picker_key(KeyWithModifier::new(BareKey::Up));
        assert_eq!(
            selected_pane(&state),
            Some(3),
            "the headers above are not rows to climb onto"
        );
    }

    /// The daemon's snapshot of the two worktree agents, with pane 3 moved to
    /// `branch`.
    fn worktree_snapshot(seq: u64, branch: &str) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Plugin(1),
            name: wire::AGENT_SNAPSHOT.to_owned(),
            payload: Some(
                serde_json::to_string(&PollReply {
                    panes: Some(vec![
                        agent_in(3, "/work/hocus", 1),
                        agent_in(5, "/work/trees/topic", 2),
                    ]),
                    activity: agents::Activity::new(),
                    manual_kinds: BTreeMap::new(),
                    repositories: BTreeMap::from([
                        ("/work/hocus".to_owned(), on_branch("/work/hocus", branch)),
                        (
                            "/work/trees/topic".to_owned(),
                            on_branch("/work/trees/topic", "topic"),
                        ),
                    ]),
                })
                .expect("serialize"),
            ),
            args: BTreeMap::from([("seq".to_owned(), seq.to_string())]),
            is_private: true,
        }
    }

    /// Opening the picker asks for the branches to be re-read, so the snapshot
    /// that answers has to reach the open list. The selection follows the pane,
    /// not the row number — the re-read moved every row here.
    #[test]
    fn a_snapshot_rebuilds_the_open_agent_picker_and_keeps_the_pane() {
        let mut state = state_with_worktrees();
        settle(&mut state);
        state.on_key(KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier());
        state.picker_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(selected_pane(&state), Some(5));

        // Pane 3 turns out to be on `zzz`, which sorts it below pane 5.
        assert!(state.pipe(worktree_snapshot(7, "zzz")), "asks to redraw");

        assert_eq!(
            entry_texts(&state),
            [
                "no project",
                "  topic",
                "  5  codex  running  tab 2",
                "  zzz",
                "  3  codex  running  tab 1",
            ]
        );
        assert_eq!(selected_pane(&state), Some(5), "still on the same agent");
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
        assert_eq!(
            entry_texts(&state),
            ["gpt-4o", "gpt-4o-mini", "o3-mini"],
            "sorted"
        );
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
    fn slash_keys_prints_the_keybinding_reference() {
        let mut state = idle_state();
        assert!(state.run_manual("/keys"));
        let notice = state.messages.last().unwrap();
        assert_eq!(notice.role, Role::Notice);
        assert!(notice.content.contains("Ctrl+l"));
        assert!(notice.content.contains("new chat"));
    }

    #[test]
    fn jobs_are_cancelled_by_the_tool_by_the_command_and_by_a_new_chat() {
        let mut state = idle_state();
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"a"}"#));
        state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"b"}"#));

        assert_eq!(state.unschedule(Some(1)), "cancelled job 1");
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.unschedule(Some(1)), "no job 1", "and it stays gone");

        assert!(
            state.run_manual("/stop_jobs"),
            "the slash command clears the rest"
        );
        assert!(state.jobs.is_empty());
        assert!(
            !state.run_manual("/stop_jobs"),
            "and does nothing when there are none"
        );

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
        assert!(
            state.tab_projects.is_empty(),
            "closed tab assignment was removed"
        );
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
        state.repositories = ["/work/hocus/src", "/work/other"]
            .into_iter()
            .map(|cwd| (cwd.to_owned(), checkout(cwd)))
            .collect();
        state.manual_kinds.insert(4, "reviewer".to_owned());

        let list = state
            .run_local(&agent_call("agent_list", r#"{"project":"hocus"}"#))
            .unwrap();
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["pane_id"], 3);
    }

    #[test]
    fn project_agent_lists_include_matching_remote_worktrees() {
        let mut state = state_with_panes();
        let mut project = projects::validate("hocus", "/work/hocus").unwrap();
        project.remote = Some("git@example.com:hocus.git".to_owned());
        project.worktree_status = Some(projects::WorktreeStatus::Primary);
        project.common_dir = Some("/work/hocus/.git".to_owned());
        state.projects.insert("hocus".to_owned(), project);
        state.panes[0].cwd = Some("/worktrees/topic/src".to_owned());
        state.repositories.insert(
            "/worktrees/topic/src".to_owned(),
            projects::Repository {
                root: "/worktrees/topic".to_owned(),
                remote: Some("git@example.com:hocus.git".to_owned()),
                worktree_status: Some(projects::WorktreeStatus::Linked),
                common_dir: Some("/work/hocus/.git".to_owned()),
                branch: Some("topic".to_owned()),
            },
        );

        assert_eq!(
            state.pane_projects().get(&3).map(String::as_str),
            Some("hocus")
        );
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

    /// A folder Git does not claim, as the daemon's probe would report it.
    fn folder(root: &str) -> projects::Repository {
        projects::Repository::folder(root)
    }

    /// A folder Git claims, as the daemon's probe would report it.
    fn checkout(root: &str) -> projects::Repository {
        on_branch(root, "main")
    }

    /// The same, with the branch it has out.
    fn on_branch(root: &str, branch: &str) -> projects::Repository {
        projects::Repository {
            root: root.to_owned(),
            remote: None,
            worktree_status: Some(projects::WorktreeStatus::Primary),
            common_dir: Some(format!("{root}/.git")),
            branch: Some(branch.to_owned()),
        }
    }

    /// One turn of the whole loop: the daemon's snapshot reaches hocus *and*
    /// hocus-store, the store discovers projects from it and pushes the registry
    /// back. Discovery lives in the store now, so a test that only called
    /// `apply_poll` would be testing half a circuit.
    ///
    /// Every agent folder is a checkout here, which is what makes a project.
    /// The tests about folders outside a repository build their own.
    fn poll(state: &mut State, panes: Vec<agents::AgentPane>) {
        poll_with(state, panes, checkout)
    }

    /// The same turn, with the daemon's Git verdict for each agent folder
    /// chosen by the caller.
    fn poll_with(
        state: &mut State,
        panes: Vec<agents::AgentPane>,
        probe: fn(&str) -> projects::Repository,
    ) {
        state.apply_poll(panes.clone(), agents::Activity::new());
        let repositories: Vec<projects::Repository> = panes
            .iter()
            .filter(|pane| agents::classify(pane, &state.manual_kinds).is_some())
            .filter_map(|pane| pane.cwd.as_deref().map(probe))
            .collect();
        state.repositories = repositories
            .iter()
            .map(|repository| (repository.root.clone(), repository.clone()))
            .collect();
        let mut registry = state.projects.clone();
        if projects::discover(&mut registry, &repositories).changed() {
            give_store(state, registry, state.todos.clone());
        }
    }

    /// Deliver a store snapshot, as hocus-store would.
    fn give_store(state: &mut State, projects: projects::Registry, todos: todos::Store) {
        let payload = serde_json::to_string(&wire::StoreSnapshot {
            projects,
            todos,
            seq: state.store_seq + 1,
        })
        .expect("serialize");
        state.pipe(PipeMessage {
            source: PipeSource::Plugin(2),
            name: wire::STORE_SNAPSHOT.to_owned(),
            payload: Some(payload),
            args: BTreeMap::new(),
            is_private: true,
        });
    }

    #[test]
    fn an_agent_folder_becomes_a_project_on_the_poll() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

        assert_eq!(state.projects["hocus"].root, "/work/hocus");
        assert!(state.projects["hocus"].discovered);
    }

    /// A folder Git does not claim is not a project, and gets no tab.
    #[test]
    fn an_agent_outside_a_repository_becomes_no_project() {
        let mut state = state_with_tab(9);
        poll_with(&mut state, vec![agent_in(3, "/work/notes", 9)], folder);

        assert!(state.projects.is_empty());
        assert!(state.auto_tab_projects.is_empty());
        assert!(state.active_project.is_none());
    }

    /// Nor does a project root above it lend it one: an agent that is not in a
    /// checkout is not working on a project.
    #[test]
    fn an_agent_outside_a_repository_joins_no_project_it_sits_under() {
        let mut state = state_with_tab(9);
        state.projects.insert(
            "hocus".to_owned(),
            projects::validate("hocus", "/work/hocus").unwrap(),
        );
        poll_with(
            &mut state,
            vec![agent_in(3, "/work/hocus/notes", 9)],
            folder,
        );

        assert!(state.pane_projects().is_empty());
        assert!(state.auto_tab_projects.is_empty());
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
        assert!(
            state.tab_projects.is_empty(),
            "nothing was assigned by hand"
        );
        assert_eq!(state.active_project.as_deref(), Some("hocus"));
    }

    #[test]
    fn a_tab_with_agents_from_two_projects_gets_no_assignment() {
        let mut state = state_with_tab(9);
        poll(
            &mut state,
            vec![
                agent_in(3, "/work/hocus", 9),
                agent_in(4, "/other/pocus", 9),
            ],
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
            vec![
                agent_in(3, "/work/hocus/src", 9),
                agent_in(4, "/work/hocus", 9),
            ],
        );

        assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["hocus"]);
        assert_eq!(state.auto_tab_projects[&9], "hocus");
        assert_eq!(state.active_project.as_deref(), Some("hocus"));
    }

    #[test]
    fn forgetting_a_project_clears_what_named_it() {
        let mut state = state_with_tab(9);
        state.run_local(&agent_call(
            "project_assign",
            r#"{"name":"hocus","root":"/work/hocus"}"#,
        ));
        assert_eq!(state.tab_projects[&9], "hocus");

        let result = state.run_local(&agent_call("project_forget", r#"{"name":"hocus"}"#));

        assert_eq!(result, Some("forgot hocus".to_owned()));
        assert!(state.projects.is_empty());
        assert!(state.tab_projects.is_empty(), "the tab named a project");
        assert!(state.active_project.is_none());
    }

    #[test]
    fn forgetting_an_unknown_project_says_what_to_call() {
        let mut state = idle_state();
        let result = state
            .run_local(&agent_call("project_forget", r#"{"name":"nope"}"#))
            .unwrap();

        assert!(result.contains("project_list"), "got {result}");
    }

    /// Nothing remembers a project that was forgotten, so a poll that still
    /// sees an agent in the checkout makes it again. The answer says so.
    #[test]
    fn forgetting_a_discovered_project_says_a_poll_makes_it_again() {
        let mut state = state_with_tab(9);
        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);

        let result = state
            .run_local(&agent_call("project_forget", r#"{"name":"hocus"}"#))
            .unwrap();
        assert!(result.contains("/work/hocus"), "got {result}");
        assert!(state.projects.is_empty());
        assert!(state.auto_tab_projects.is_empty());

        poll(&mut state, vec![agent_in(3, "/work/hocus", 9)]);
        assert_eq!(state.projects["hocus"].root, "/work/hocus");
    }

    /// The only tool that removes something the user meant to keep, so it stops
    /// for a y/n rather than running on the model's say-so.
    #[test]
    fn forgetting_a_project_waits_for_a_confirmation() {
        let mut state = ready_state();
        state.projects.insert(
            "hocus".to_owned(),
            projects::validate("hocus", "/work/hocus").unwrap(),
        );
        let frame = tool_call_frame("call_1", "project_forget", r#"{"name":"hocus"}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(state.projects.contains_key("hocus"), "nothing yet");
        state.confirm_tool();
        assert!(state.projects.is_empty());
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

    /// Run a todo tool the way the whole system does.
    ///
    /// Reads are answered from hocus's cached copy. Writes go out to the store
    /// and come back, which is modelled here by applying the change to the same
    /// pure functions hocus-store uses and pushing the result back as a
    /// snapshot — so these tests still cover hocus's half of every call.
    fn todo(state: &mut State, name: &str, arguments: &str) -> serde_json::Value {
        let result = todo_result(state, name, arguments)
            .unwrap_or_else(|| panic!("{name} answered nothing"));
        serde_json::from_str(&result)
            .unwrap_or_else(|_| panic!("{name} answered with an error: {result}"))
    }

    /// The raw answer of a todo tool, or `None` if it is not one.
    fn todo_result(state: &mut State, name: &str, arguments: &str) -> Option<String> {
        let call = agent_call(name, arguments);
        if !matches!(name, "todo_add" | "todo_done") {
            return state.run_local(&call);
        }
        let args = tools::Args::parse(arguments).expect("arguments");
        // Whatever hocus rejects on its own it rejects before the store hears
        // about it, and that answer is the tool's answer.
        let (root, project) = match state.todo_target(&args) {
            Ok(target) => target,
            Err(error) => return Some(error),
        };
        let mut todos_store = state.todos.clone();
        let answer = if name == "todo_add" {
            let Some(text) = args.text.as_deref() else {
                return Some("text is required".to_owned());
            };
            todos::add(&mut todos_store, &root, text, args.parent_id).map(|id| {
                serde_json::json!({ "project": project, "id": id, "text": text }).to_string()
            })
        } else {
            let Some(id) = args.id else {
                return Some("id is required".to_owned());
            };
            todos::set_done(&mut todos_store, &root, id, !args.undo).map(|change| {
                serde_json::json!({
                    "project": project,
                    "id": change.id,
                    "text": change.text,
                    "done": change.done,
                    "also_done": change.cascaded,
                    "reopened": change.reopened,
                })
                .to_string()
            })
        };
        match answer {
            Ok(message) => {
                give_store(state, state.projects.clone(), todos_store);
                Some(message)
            },
            Err(error) => Some(error),
        }
    }

    #[test]
    fn a_todo_tool_says_when_there_is_no_active_project() {
        let mut state = idle_state();
        assert_eq!(
            todo_result(&mut state, "todo_add", r#"{"text":"ship it"}"#),
            Some("no active project; give project, or call project_list".to_owned()),
        );
    }

    #[test]
    fn a_todo_item_lands_under_the_active_project_root() {
        let mut state = state_with_project();
        let added = todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);

        assert_eq!(added["id"], 1);
        assert_eq!(added["project"], "hocus");
        assert_eq!(added["text"], "ship the parser");
        // And it came back to us through a store snapshot, not a local write.
        assert_eq!(state.todos["/work/hocus"].items[0].text, "ship the parser");
    }

    #[test]
    fn a_todo_tool_rejects_a_project_name_it_does_not_know() {
        let mut state = state_with_project();
        assert_eq!(
            todo_result(
                &mut state,
                "todo_add",
                r#"{"text":"ship it","project":"nowhere"}"#
            ),
            Some("unknown project nowhere; call project_list".to_owned()),
        );
    }

    #[test]
    fn a_todo_tool_needs_its_key_argument() {
        let mut state = state_with_project();
        assert_eq!(
            todo_result(&mut state, "todo_add", "{}"),
            Some("text is required".to_owned()),
        );
        assert_eq!(
            state.run_local(&agent_call("todo_search", "{}")),
            Some("query is required".to_owned()),
        );
        assert_eq!(
            todo_result(&mut state, "todo_done", "{}"),
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
            vec![
                agent_in(3, "/work/hocus/src", 9),
                agent_in(4, "/work/hocus", 9),
            ],
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

        let found = todo(
            &mut state,
            "todo_search",
            r#"{"query":"parser","all_projects":true}"#,
        );
        assert_eq!(found["scope"], "all");
        assert_eq!(found["matches"][0]["root"], "/work/gone");
        assert_eq!(found["matches"][0]["project"], serde_json::Value::Null);
    }

    /// The store is asked by name where there is one, and by root where the
    /// project is gone — a `null` project there is a change the store rejects,
    /// for a list `todo_list` and `todo_search` still answer for.
    #[test]
    fn a_write_to_a_list_whose_project_is_gone_is_sent_by_root() {
        let mut state = state_with_project();
        let by_name = state
            .todo_write_payload(&agent_call("todo_add", r#"{"text":"ship it"}"#))
            .expect("payload");
        assert_eq!(by_name["project"], "hocus");

        state.projects.clear();
        state.active_project = None;
        let by_root = state
            .todo_write_payload(&agent_call(
                "todo_done",
                r#"{"id":1,"project":"/work/gone"}"#,
            ))
            .expect("payload");
        assert_eq!(by_root["project"], "/work/gone");
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
        assert_eq!(
            state.agent_cwd(&args).unwrap().as_deref(),
            Some("/work/hocus")
        );
    }

    #[test]
    fn a_project_tab_uses_the_stable_id_from_zellij() {
        let mut state = idle_state();
        let result = state.register_opened_project("hocus", r#"{"opened":true,"tab_id":47}"#);
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

    /// The same panes `state_with_panes` starts from, to refill after a
    /// snapshot has emptied them.
    fn panes_with_agent() -> Vec<agents::AgentPane> {
        terminal_panes(
            &manifest(vec![
                pane_info(3, "codex", Some("codex --resume")),
                pane_info(4, "zsh", Some("zsh")),
            ]),
            &[],
        )
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
        assert_eq!(
            panes[0].status,
            agents::AgentStatus::Held,
            "held beats exited"
        );
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
            state.register_spawned(
                Some("reviewer"),
                r#"{"opened":true,"pane_id":7,"is_plugin":false}"#
            ),
            r#"{"opened":true,"pane_id":7,"is_plugin":false} (registered as reviewer)"#,
        );
        assert_eq!(
            state.manual_kinds.get(&7).map(String::as_str),
            Some("reviewer")
        );
        assert!(state.pending_agent_panes.contains(&7));

        // The subscribe reply can beat the pane-created event back to hocus.
        // That early snapshot must not discard the override it just sent.
        assert!(!state.set_panes(Vec::new()));
        assert_eq!(
            state.manual_kinds.get(&7).map(String::as_str),
            Some("reviewer")
        );

        state.set_panes(terminal_panes(
            &manifest(vec![pane_info(7, "zsh", Some("zsh"))]),
            &[],
        ));
        assert!(!state.pending_agent_panes.contains(&7));
        assert_eq!(
            state.manual_kinds.get(&7).map(String::as_str),
            Some("reviewer")
        );

        assert!(state.set_panes(Vec::new()));
        assert!(!state.manual_kinds.contains_key(&7));

        // No kind, or no pane in the answer: the result passes through untouched.
        assert_eq!(
            state.register_spawned(None, r#"{"opened":true,"pane_id":8,"is_plugin":false}"#),
            r#"{"opened":true,"pane_id":8,"is_plugin":false}"#,
        );
        assert_eq!(
            state.register_spawned(Some("scout"), "zellij did not open the pane"),
            "zellij did not open the pane",
        );
        assert!(state.manual_kinds.is_empty());
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
            manual_kinds: BTreeMap::new(),
            repositories: BTreeMap::from([(
                "/tmp/work".to_owned(),
                projects::Repository {
                    root: "/tmp/work".to_owned(),
                    remote: Some("git@example.com:work.git".to_owned()),
                    worktree_status: Some(projects::WorktreeStatus::Linked),
                    common_dir: Some("/tmp/main/.git".to_owned()),
                    branch: Some("topic".to_owned()),
                },
            )]),
        };
        let json = serde_json::to_string(&reply).expect("serialize");
        let back: PollReply = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.panes, reply.panes);
        assert_eq!(back.activity, reply.activity);
        assert_eq!(back.repositories, reply.repositories);
    }

    /// An empty snapshot carrying `seq`, as hocus-agentd sends it.
    fn agent_snapshot(seq: u64) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Plugin(1),
            name: wire::AGENT_SNAPSHOT.to_owned(),
            payload: Some(
                serde_json::to_string(&PollReply {
                    panes: Some(Vec::new()),
                    activity: agents::Activity::new(),
                    manual_kinds: BTreeMap::new(),
                    repositories: BTreeMap::new(),
                })
                .expect("serialize"),
            ),
            args: BTreeMap::from([("seq".to_owned(), seq.to_string())]),
            is_private: true,
        }
    }

    /// Snapshots are unsolicited, so ordering is all hocus has: one that is no
    /// newer than the last merged arrived late and would resurrect panes the
    /// newer one already saw close.
    #[test]
    fn an_out_of_order_agent_snapshot_is_dropped() {
        let mut state = state_with_panes();

        assert!(state.pipe(agent_snapshot(4)), "a newer snapshot merges");
        assert!(state.panes.is_empty());
        assert_eq!(state.agent_seq, 4);

        state.set_panes(panes_with_agent());
        assert!(!state.pipe(agent_snapshot(3)), "an older one is dropped");
        assert!(!state.panes.is_empty(), "stale snapshot must not merge");
        assert!(
            !state.pipe(agent_snapshot(4)),
            "nor is a repeat of the last"
        );
        assert!(!state.panes.is_empty());
        assert_eq!(state.agent_seq, 4);
    }

    /// A reloaded daemon restarts its counter, so the watermark has to be
    /// dropped whenever we re-subscribe or its snapshots would never be taken
    /// again.
    #[test]
    fn re_subscribing_forgets_the_sequence_watermark() {
        let mut state = state_with_panes();
        state.pipe(agent_snapshot(9));
        assert_eq!(state.agent_seq, 9);

        state.subscribe_to_agentd();
        assert_eq!(state.agent_seq, 0, "a restarted daemon starts at one again");
        state.set_panes(panes_with_agent());
        assert!(state.pipe(agent_snapshot(1)), "and is heard from once more");
        assert!(state.panes.is_empty());
    }

    /// The failure the store introduces: a change goes out and nothing comes
    /// back. The tool call must fail rather than park the queue forever.
    #[test]
    fn a_store_write_that_is_never_answered_gives_up() {
        let mut state = state_with_project();
        let call = agent_call("todo_add", r#"{"text":"ship it"}"#);
        state.pending.push_back(call.clone());

        assert!(
            !state.start_todo_write(&call),
            "the call waits on the store"
        );
        assert!(matches!(state.status, Status::AwaitingStore));
        assert!(state.pending_write.is_some());

        // Not yet — a slow answer is still an answer.
        assert!(!state.tick_store(STORE_TIMEOUT_SECONDS / 2.0));
        assert!(state.pending_write.is_some());

        assert!(state.tick_store(STORE_TIMEOUT_SECONDS));
        assert!(state.pending_write.is_none());
        assert!(matches!(state.status, Status::Idle));
        assert!(state.pending.is_empty(), "the queue moved on");
        let answer = tool_results(&state)
            .last()
            .expect("answered")
            .content
            .clone();
        assert!(answer.contains("did not answer"), "got {answer}");
    }

    /// A reply to a write that already gave up must not answer whatever call is
    /// at the head by then.
    #[test]
    fn a_late_store_reply_answers_nothing() {
        let mut state = state_with_project();
        let result = serde_json::to_string(&wire::StoreResult {
            request_id: "stale".to_owned(),
            ok: true,
            message: "{}".to_owned(),
        })
        .expect("serialize");

        assert!(!state.pipe(PipeMessage {
            source: PipeSource::Plugin(2),
            name: wire::STORE_RESULT.to_owned(),
            payload: Some(result),
            args: BTreeMap::new(),
            is_private: true,
        }));
        assert!(state.pending_write.is_none());
    }

    /// The snapshot is the only way projects arrive now, so a project that goes
    /// away in one has to stop being the active project.
    #[test]
    fn a_snapshot_that_drops_a_project_clears_what_named_it() {
        let mut state = state_with_project();
        assert_eq!(state.active_project.as_deref(), Some("hocus"));

        give_store(&mut state, projects::Registry::new(), todos::Store::new());
        assert_eq!(state.active_project, None);
        assert!(state.tab_projects.is_empty());
    }

    /// A reloaded store restarts its counter at one, so re-subscribing has to
    /// drop the watermark or nothing it sends is ever taken again.
    #[test]
    fn re_subscribing_to_the_store_forgets_the_watermark() {
        let mut state = state_with_project();
        let projects = state.projects.clone();
        give_store(&mut state, projects, todos::Store::new());
        assert!(state.store_seq > 0, "a snapshot has been merged");

        state.subscribe_to_store();
        assert_eq!(state.store_seq, 0, "a restarted store starts at one again");

        // `give_store` sends `store_seq + 1`, so this is the seq-1 snapshot a
        // fresh store would push back at us.
        give_store(&mut state, projects::Registry::new(), todos::Store::new());
        assert_eq!(state.store_seq, 1);
        assert_eq!(state.active_project, None, "and is heard from once more");
    }

    /// The store announces itself on load because it cannot rebuild its own
    /// subscriber list. Hearing it is the only thing that keeps the read cache
    /// live across a store reload.
    #[test]
    fn a_store_hello_re_subscribes() {
        let mut state = state_with_project();
        let projects = state.projects.clone();
        give_store(&mut state, projects, todos::Store::new());
        assert!(state.store_seq > 0);

        assert!(
            !state.pipe(PipeMessage {
                source: PipeSource::Plugin(2),
                name: wire::STORE_HELLO.to_owned(),
                payload: None,
                args: BTreeMap::new(),
                is_private: false,
            }),
            "nothing to render until the snapshot lands"
        );
        assert_eq!(state.store_seq, 0);
    }

    /// Anything that is not a snapshot is somebody else's message — and every
    /// plugin sees them, because the subscribe goes out as a broadcast.
    #[test]
    fn an_unknown_pipe_message_is_ignored() {
        let mut state = state_with_panes();
        assert!(!state.pipe(PipeMessage {
            source: PipeSource::Cli("1".to_owned()),
            name: "something_else".to_owned(),
            payload: Some("{}".to_owned()),
            args: BTreeMap::new(),
            is_private: false,
        }));
        assert!(!state.panes.is_empty());
        assert_eq!(state.agent_seq, 0, "an unrelated message merges nothing");
    }
}
