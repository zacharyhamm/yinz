//! hocus — chat with an OpenAI-compatible endpoint from a Zellij popup.

mod api;
mod archive;
mod events;
mod history;
mod linear;
mod log;
mod message;
mod present;
mod tool_loop;
mod tools;
mod transitions;
mod ui;
mod worker;

pub use message::{
    Job, Message, Picker, PickerKind, PickerRow, Reasoning, ReasoningKey, Role, RowKind, Status,
    StreamState,
};
use worker::post_log;

use hocus_proto::location::Location;
use hocus_proto::store::{self, Backend};
use hocus_proto::wire::{self, PollReply, PollRequest};
use hocus_proto::{agents, launch, projects, todos};
/// The pure half of hocus, shared with `hocus-agentd` and `hocus-store`.
/// Re-exported under the names the code already used when these were modules.
use zellij_tile::ui_components::forms::{InputAction, TextInput};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use zellij_tile::prelude::*;

use api::{Config, ConfigError, SseItem, SseParser, ToolCallAccumulator, DEFAULT_MAX_TOOL_ROUNDS};
use tools::ToolCall;

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

/// How many questions from the ask popup may wait for a busy hocus at once.
/// Deep enough that nobody hits it typing at human speed, shallow enough that
/// a stream which never settles cannot bury the conversation.
const MAX_QUEUED_ASKS: usize = 8;

/// Slash commands that drive the plugin itself rather than a tool. Typed and
/// completed like tools, but never offered to the model.
const LOCAL_COMMANDS: [&str; 4] = ["history", "models", "stop_jobs", "keys"];

/// Full keybinding reference for `/keys`. The chat view has no persistent
/// help line, so this is the one place all of these are written down.
const KEYS_HELP: &str = "\
Ctrl+`        toggle hocus
Ctrl+l        new chat
Ctrl+r        focus agents
Up / Down     previous / next message in history
Tab           complete a /command or @agent
Alt+Enter, Shift+Enter   insert a newline
PageUp / PageDown        scroll the transcript
Esc           cancel a stream or tool, or hide hocus
y / n         confirm or decline a pending tool call

In history or models:
Up / Down     choose
Enter         select
Esc           back

With agents focused:
Up / Down     choose
Enter         focus pane
Esc           return to chat";

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
    /// Travels with the chat: a chat parked mid-confirmation is still waiting on
    /// that pane, and it is the one that will have to clear the mark when it
    /// comes back and is finally answered.
    pub(crate) spotlight: Option<PaneId>,
    pub(crate) tool_rounds: usize,
    pub(crate) owes_reply: bool,
    pub(crate) manual_head: bool,
    pub(crate) chat_id: Option<u64>,
    pub(crate) saved: (usize, usize),
    pub(crate) view: View,
    pub(crate) job_auto_confirm: bool,
}

impl Chat {
    /// Whether this chat is still owed something — the only reason to hold it
    /// rather than let the archive have it back.
    pub(crate) fn is_live(&self) -> bool {
        self.stream.is_some() || !matches!(self.status, Status::Idle)
    }
}

/// Where each client is looking at one conversation, and what holds them there.
///
/// Part of `Chat` rather than of `State` alone: a parked chat receiving a reply
/// pins and anchors its own viewport, and doing that to whoever is reading the
/// foreground would be someone else's chat moving their screen.
#[derive(Default)]
pub(crate) struct View {
    /// Wrapped transcript lines hidden *below* the viewport, per client. Zero —
    /// which is what a missing entry means — is pinned to the bottom, and is
    /// what makes a growing reply follow itself.
    ///
    /// Keyed by client because one shared instance renders once per attached
    /// client (see `State::client`), and one person reading back should not drag
    /// everyone else with them. `pin` clears the map on any conversation change,
    /// so a departed client leaves nothing behind.
    scroll: BTreeMap<ClientId, usize>,
    /// Clients that scrolled for themselves and so are no longer held by
    /// `anchor`. Sticky until the next `pin`: re-arming when someone scrolls
    /// back to the bottom would yank them up to the anchor again the instant
    /// they read to the end of an arriving reply.
    released: BTreeSet<ClientId>,
    /// The message whose first line must stay on the top row — the reply being
    /// streamed — so a long answer cannot scroll its own opening off the pane.
    ///
    /// A message index rather than a line count, because it has to survive the
    /// transcript being re-wrapped at a new width and the viewport changing
    /// height; both move line numbers, neither moves messages.
    anchor: Option<usize>,
}

impl View {
    /// Put every client back on the tail, and stop holding anyone to a reply.
    /// The conversation changing is not one client's business.
    fn pin(&mut self) {
        self.scroll.clear();
        self.released.clear();
        self.anchor = None;
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
    /// The pane currently wearing the confirmation mark, if any.
    ///
    /// Recorded rather than re-derived from `pending` on the way out, because the
    /// head has usually already moved on by the time the mark comes down: a `y`
    /// runs the call and pumps the queue, and the pane that has to be cleared is
    /// the one that was marked, not whatever is being asked about now.
    spotlight: Option<PaneId>,
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
    /// Where each client is looking at the foreground conversation. See `View`.
    view: View,
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
    /// Memoised `ui::transcript`, so a keystroke — which changes nothing in the
    /// transcript — re-wraps nothing. See `ui::TranscriptKey` for why the key is
    /// sufficient.
    pub transcript_cache: Option<(ui::TranscriptKey, ui::Transcript)>,
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
    /// The archive or model browser, when it is open.
    pub picker: Option<Picker>,
    /// The agent strip as last pushed to the hocus-agents pane, so a snapshot
    /// that changes no row sends nothing. hocus owns the data (the agentd
    /// snapshot, the project registry, the branch probes); the strip itself is
    /// a separate global pane split off above this one, and it only draws.
    last_strip: agents::Strip,
    /// Scheduled prompts, counted down by the timer chain.
    pub jobs: Vec<Job>,
    /// Questions from the `hocus-ask` popup that arrived while hocus was busy,
    /// oldest first. Drained one per tick by `drain_asks`.
    ///
    /// A queue rather than a single slot because the popup *tells the user*
    /// their question was taken: last-wins would silently drop something
    /// already acknowledged as queued, and first-wins would drop the newer
    /// question with no acknowledgement to match. Answering both in the order
    /// they were asked is the only behaviour that matches what was promised.
    ///
    /// Capped by `MAX_QUEUED_ASKS`. Past the cap the popup is refused, which is
    /// an answer the user can see, rather than an unbounded pile of questions
    /// stacking up behind one wedged stream.
    pub asks: VecDeque<String>,
    job_seq: u64,
    /// Authority for the scheduled run that currently owns the conversation.
    job_auto_confirm: bool,
    /// Terminal panes from hocus-agentd's latest event-fed snapshot. This is the
    /// only snapshot agent tools trust, which stops them acting on a closed pane.
    panes: Vec<agents::AgentPane>,
    /// Agent CWDs resolved by hocus-agentd to Git checkout identities.
    /// What Git said about each folder an agent is in, keyed by folder *and*
    /// machine. Filled from the daemon's snapshot; see `wire::PollReply`.
    repositories: BTreeMap<Location, projects::Repository>,
    /// Kinds the user set by hand with `agent_register`, keyed by pane id. These win
    /// over detection, and are dropped when their pane leaves the snapshot.
    manual_kinds: BTreeMap<u32, String>,
    /// Spawned panes whose ids are allocated but have not reached a pane snapshot yet.
    /// Their manual kinds survive early snapshots and are pruned normally once seen.
    pending_agent_panes: BTreeSet<u32>,
    /// Consecutive unchanged scrollback observations from hocus-agentd.
    activity: agents::Activity,
    /// What the daemon's model last made of each idle agent's pane. Read-only here: the
    /// daemon is the only writer, the same way it is for `activity`.
    summaries: agents::Summaries,
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
    ///
    /// A copy, kept current by a watch on the `project` rows rather than by a
    /// pushed snapshot. It stays a copy because half the plugin reads it from
    /// synchronous `&self` methods on hot paths — the tab map, the per-request
    /// system message, every agent tool — and a host call in each of those would
    /// buy nothing.
    ///
    /// The todo lists are deliberately *not* here. They are read only by two
    /// tools, both already behind a model round trip, so they are fetched when
    /// asked for. That drops the largest thing hocus used to hold a copy of.
    projects: projects::Registry,
    /// The watch feeding `projects`, so a change event can be told from another
    /// plugin's.
    project_watch: u64,
    /// The data store, on builds that have no host to ask for one.
    ///
    /// A real engine over a temporary directory, so the tests exercise the same
    /// predicates and upserts a session does. In wasm this field does not exist
    /// and [`State::store`] returns the host shims instead.
    #[cfg(not(target_family = "wasm"))]
    store: hocus_proto::store::Local,
    /// Stable tab id to project name. Tab ids only live for this session.
    ///
    /// Only what the user assigned. The poll writes `auto_tab_projects`, so an
    /// assignment the user made can never be overwritten.
    tab_projects: BTreeMap<usize, String>,
    /// Tab id to project name, as the poll worked it out from the agents in the
    /// tab. Rebuilt on every poll. `tab_projects` wins over this map.
    auto_tab_projects: BTreeMap<usize, String>,
    /// What each tab is called after the project in it, as last told to the host.
    ///
    /// Not the same map as the two above: a tab whose agents are in two projects
    /// is named after both, where an *assignment* has to pick one and so picks
    /// neither. Kept so the host is only told when the answer changes — this is
    /// recomputed on every poll and the answer is usually the same one.
    tab_project_labels: BTreeMap<usize, String>,
    /// The tabs as of the last `TabUpdate`.
    ///
    /// The host sends one per client, so with every client served by this one
    /// instance these are whoever switched tabs most recently — and so is
    /// `active_project`. Last writer wins, deliberately: everyone attached
    /// shares the one conversation, so they share its project too.
    tabs: Vec<TabInfo>,
    /// The agent pane the user last had focused, as the daemon reports focus.
    ///
    /// Sticky on purpose: focus lands on the sidebar the moment anyone types into
    /// it, and on a shell or an editor for half the day. The question a project
    /// answers is "what are you working on", and that is the last agent you looked
    /// at, not the pane your cursor happens to be in. Cleared only when the pane
    /// itself goes away.
    last_focused_agent: Option<u32>,
    /// The pane the daemon last reported focused, agent or not.
    ///
    /// The unsticky twin of `last_focused_agent`, kept for the agents pane: that
    /// one can say which agent to put the cursor on, but never that the cursor is
    /// somewhere the user no longer is, which is the difference the pane draws.
    focused_pane: Option<u32>,
    /// The plugin pane the daemon last reported focused.
    ///
    /// Only ever compared against `agents_plugin`: the one thing hocus wants from it
    /// is whether the user is standing in the agents pane, which is what the summary
    /// inset is drawn for. `None` whenever a terminal has the focus.
    focused_plugin: Option<u32>,
    /// The agents pane, as it announced itself.
    ///
    /// Learned from the first `AGENT_SELECTION` rather than assumed, because a plugin
    /// pane is only known by its id — a title is a name the user can change, and a url
    /// cannot tell two instances apart.
    agents_plugin: Option<u32>,
    /// The agent the cursor is on in the agents pane.
    ///
    /// The question the inset answers is "what was *this* one doing", and the agents
    /// pane is where that is asked: the selection moves there under `j`/`k` with no
    /// focus change to report, so nothing else on this side can know it.
    agents_selected: Option<u32>,
    /// The project of `last_focused_agent`, falling back to the active tab in
    /// `tabs`.
    active_project: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        #[allow(unused_mut)]
        let mut state = State {
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
            spotlight: None,
            tool_rounds: 0,
            owes_reply: false,
            manual_head: false,
            stream_seq: 0,
            view: View::default(),
            // Overwritten by the host on the first event or render; nothing
            // reads it before then.
            client: 0,
            transcript_rows: 0,
            transcript_cache: None,
            chat_id: None,
            saved: (0, 0),
            parked: Vec::new(),
            picker: None,
            last_strip: agents::Strip::default(),
            jobs: Vec::new(),
            asks: VecDeque::new(),
            job_seq: 0,
            job_auto_confirm: false,
            panes: Vec::new(),
            repositories: BTreeMap::new(),
            manual_kinds: BTreeMap::new(),
            pending_agent_panes: BTreeSet::new(),
            activity: BTreeMap::new(),
            summaries: agents::Summaries::new(),
            agent_seq: 0,
            mono_ms: 0,
            log_seq: 0,
            granted: false,
            projects: BTreeMap::new(),
            project_watch: 0,
            #[cfg(not(target_family = "wasm"))]
            store: {
                // Declared here rather than in `open_store`, which the tests do
                // not go through: a fresh `State` is expected to have a usable
                // store, and every one of them gets its own temporary file.
                let local = hocus_proto::store::Local::shared("hocus");
                let _ = local.define_all(&store::shared_defs());
                local
            },
            tab_projects: BTreeMap::new(),
            auto_tab_projects: BTreeMap::new(),
            tab_project_labels: BTreeMap::new(),
            tabs: Vec::new(),
            last_focused_agent: None,
            focused_pane: None,
            focused_plugin: None,
            agents_plugin: None,
            agents_selected: None,
            active_project: None,
        };
        // In a plugin this waits for `load`, which is where the host is ready to
        // be asked. On the host triple there is no host and no `load`, so the
        // temporary store is opened here — otherwise every test would have to
        // remember to, and the one that forgot would fail against an entity
        // nobody had declared.
        #[cfg(not(target_family = "wasm"))]
        state.open_store();
        state
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.kdl = configuration;
        self.open_store();
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
            // Deliberately *not* MessageAndLaunchOtherPlugins, and deliberately
            // not PluginData or SharedPluginData either, though talking to
            // hocus-agentd and reading the store would nominally need them. A
            // builtin is granted every gated call regardless (see
            // `check_command_permission`, and `resolve_data_namespace` for the
            // shared namespace), so adding them buys nothing — while
            // `check_permissions` demands an exact match against the cached
            // grant, so a longer list re-prompts every existing user for
            // permissions that were never enforced.
            //
            // A hocus loaded from a `file:` path is not a builtin and so loses
            // all of it, with no inline fallback: the daemon's snapshot is the
            // only source of panes and folders, and the store is the only source
            // of projects, todos, the archive and the up-arrow history. Agents
            // and projects go missing, writes fail, and the archive is empty —
            // that last one is new, and is the reason to mention it here.
            //
            // Not worth re-prompting everyone over: hot-reloading a builtin
            // keeps the `zellij:` location and resolves to the freshly built
            // wasm anyway (see `BUILTIN_PLUGIN_DEV_DIRS`), so a `file:` load is
            // a thing you have to go out of your way to do.
        ]);

        // The agent list is its own pane, split off above this one in the same
        // global strip. Idempotent server-side: an existing hocus-agents global
        // pane (a reload, a session restore) is adopted rather than split
        // again, and a hocus that is not a global pane spawns nothing. Fire-
        // and-forget, so calling it before the permission grant resolves is
        // fine - and a builtin is exempt from the gate anyway.
        Self::launch_agents_pane(false);
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
        // set here. Only `view` differs between them; everything else on screen
        // is the one conversation everyone shares.
        self.client = get_plugin_ids().client_id;
        ui::render(self, rows, cols);
    }

    /// Snapshots pushed by `hocus-agentd`.
    ///
    /// Every snapshot ends up in `apply_poll`. There is no second path into it
    /// any more; the daemon is the only source of panes and folders.
    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            wire::AGENT_HELLO => {
                self.subscribe_to_agentd();
                return false;
            },
            wire::AGENT_STRIP_HELLO => {
                // a freshly loaded (or restored) hocus-agents pane asking for
                // the current rows
                self.push_strip(true);
                return false;
            },
            wire::AGENT_SELECTION => return self.on_selection(&pipe_message),
            wire::ASK => return self.on_ask(&pipe_message),
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
        let inset = self.inset_key();
        // Outside the panes below: a quiet reply — one from a daemon with no session
        // to report — still says where the focus went, and the inset is gated on
        // that alone.
        self.focused_plugin = reply.focused_plugin;
        if let Some(panes) = reply.panes {
            self.apply_poll(panes, reply.activity, reply.summaries, reply.focused);
        }
        // This also answers the branch refresh the agents pane asks for.
        self.push_strip(false);
        // A snapshot is otherwise not worth a repaint - nothing else on screen
        // comes from one. The summary inset does, and it appears and goes with
        // the user's focus rather than with anything they typed here, so without
        // this it would wait for the next keystroke to be drawn.
        self.inset_key() != inset
    }
}

impl State {
    /// The configured tool budget, falling back to the default when the config
    /// failed to resolve — an unconfigured hocus still has to stop looping.
    pub(crate) fn max_tool_rounds(&self) -> usize {
        self.cfg
            .as_ref()
            .map(|cfg| cfg.max_tool_rounds)
            .unwrap_or(DEFAULT_MAX_TOOL_ROUNDS)
    }

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
            // The project rows changed — ours or another plugin's write. The
            // store only sends this when the rows actually differ, so there is
            // no watermark to check and nothing to do when nothing moved.
            Event::PluginDataChanged { watch_id, rows, .. } => {
                self.on_projects_changed(watch_id, &rows)
            },
            Event::TabUpdate(tabs) => self.on_tabs(tabs),
            Event::BeforeClose => {
                self.cancel_in_flight();
                // Nobody is left to press `y`, and the mark outlives the plugin:
                // a pane would otherwise sit there titled for a question that
                // can no longer be answered.
                self.drop_spotlight();
                // A question the popup was told had been queued must not simply
                // vanish. As a notice it survives into the archive and shows up
                // in /history, and — being a notice — it never reaches the
                // model, so reopening the chat does not answer it by surprise.
                for text in std::mem::take(&mut self.asks) {
                    self.messages
                        .push(Message::notice(format!("unsent question: {text}")));
                }
                // Nothing left to flush but the transcripts. Projects and todos
                // are written as each change lands, and so is every prompt.
                //
                // This is the last chance: write the transcript here or lose the
                // last exchange. Background chats have the same one chance, and
                // their partial replies are worth no less.
                self.flush_save();
                for chat in &self.parked {
                    if let Some(stream) = &chat.stream {
                        cancel_web_request(&stream.id);
                    }
                }
                self.save_parked();
                false
            },
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------

#[path = "./unit/main_tests/mod.rs"]
#[cfg(test)]
mod main_tests;
