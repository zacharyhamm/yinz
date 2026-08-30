// The tool schemas are one `json!` literal each, and the largest is past the default limit.
#![recursion_limit = "256"]

//! hocus — chat with an OpenAI-compatible endpoint from a Zellij popup.
//!
//! A library so the wasm entry point (`src/main.rs`) and the render benchmark
//! (`benches/transcript.rs`) can both see it; a bin crate has nothing to import.

mod api;
mod archive;
mod events;
mod extensions;
mod history;
mod linear;
mod log;
mod lua_docs;
mod message;
mod overlay;
mod present;
mod skills;
mod tool_loop;
mod tools;
mod transitions;
pub use transitions::{Confirmation, PromptKind};
pub mod ui;
mod wand;
mod worker;

pub use message::{
    Ask, Job, Message, Picker, PickerKind, PickerRow, Plan, Reasoning, ReasoningKey, Role, RowKind,
    ScheduledRun, Status, StreamState,
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

/// How long a `plugin_reload` waits before reading the plugin's pane. A Lua plugin loads in
/// well under this; what the wait covers is the screen thread opening the pane and the first
/// render landing in it, so that what is read back is the plugin and not the loading screen.
const PLUGIN_LOAD_MS: u64 = 750;

/// A `plugin_reload` between asking the host to load the plugin and looking at the result.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginLoadWatch {
    /// The absolute path that was reloaded, which is what the pane's `plugin_url` ends in.
    pub target: String,
    /// `mono_ms` when the reload was requested.
    pub started_ms: u64,
    /// Hocus extensions load headlessly and prove success by registering tools; ordinary
    /// plugins open a pane and prove it by rendering there.
    pub extension: bool,
}

/// The shortest interval `schedule` will accept. A model that asks for one
/// second bills the user sixty times a minute for a pane that hasn't changed.
const MIN_JOB_SECONDS: f64 = 5.0;

/// How many `hocus_ask` questions may wait for a busy hocus at once.
/// Deep enough that nobody hits it typing at human speed, shallow enough that
/// a stream which never settles cannot bury the conversation.
const MAX_QUEUED_ASKS: usize = 8;

/// Slash commands that drive the plugin itself rather than a tool. Typed and
/// completed like tools, but never offered to the model.
const LOCAL_COMMANDS: [&str; 8] = [
    "history",
    "models",
    "stop_jobs",
    "keys",
    "plan",
    "execute",
    "clear",
    "skills",
];

/// Full keybinding reference for `/keys`. The chat view has no persistent
/// help line, so this is the one place all of these are written down.
const KEYS_HELP: &str = "\
Ctrl+`        toggle hocus
Ctrl+l        new chat, in a new tab (also /clear)
Ctrl+o        move hocus clear of a pane - the one it is covering, or the
              largest one you are not in (top right if it is the only pane)
Shift+Tab     open chats: focus the tab bar
Up / Down     previous / next message in history
Tab           complete a /command or @agent
Alt+Enter, Shift+Enter   insert a newline
PageUp / PageDown        scroll the transcript
Esc           hide hocus
Ctrl+c        cancel a stream or tool, keep refining a
              plan, or clear the prompt
y / n         run the pending tool call, or decline it and stop
n             keep refining a plan hocus has written
/plan <text>  plan before acting: read-only tools until
              you /execute the plan hocus writes
/execute      carry out that plan; trailing text amends it
/<skill> <text>  send a skill's prompt with <text> as its
              arguments; /skills lists them, /skills reload
              rescans, /skills enable|disable <scope> <name>
              changes one

In history or models:
Up / Down     choose
Enter         select
Esc           back

With the tab bar focused:
Left / Right  choose (Tab / Shift+Tab too)
Enter         switch to that chat
x             close it
Ctrl+l        new chat
Esc           back to the prompt

With agents focused:
Up / Down     choose
Enter         focus pane
Esc           return to chat";

/// Context key carrying a chat request's stream id back on its `WebRequestResult`,
/// so the response can be matched to the chat that asked.
const STREAM_CONTEXT: &str = "stream";

/// What the provider last said the context cost, and how much of the
/// transcript was on the wire when it said so. The difference between that
/// and the transcript now is what the title's `~` estimate is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextUsage {
    /// Prompt and completion together: what the *next* request will carry.
    pub(crate) tokens: u64,
    /// `api::wire_chars` at the moment the count arrived.
    pub(crate) wire_chars: usize,
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
    pub(crate) scheduled_run: Option<ScheduledRun>,
    pub(crate) plan: Plan,
    pub(crate) usage: Option<ContextUsage>,
    pub(crate) context_cache: Option<((usize, usize), usize)>,
    /// Travels with the chat for the same reason `spotlight` does: the reload was asked for by
    /// *this* conversation, and it is the one owed the report. A watch left on `State` alone
    /// would be overwritten by another chat's reload and stranded when this one is parked.
    pub(crate) plugin_load: Option<PluginLoadWatch>,
    /// What was typed but not sent, and where the cursor was. The draft rather
    /// than the whole `TextInput`: the box also owns the prompt history, which
    /// is one list for every chat.
    ///
    /// Only the user-facing switches — `swap_foreground` — move it. The
    /// engine swap that services a parked chat (`with_parked`) leaves the input
    /// box alone: touching it would throw away the undo stack and the Up/Down
    /// draft of whoever is typing, on every tick.
    pub(crate) draft: (String, usize),
    /// The render memos, travelling with the chat they were computed for. Their
    /// keys are derived from the transcript, so a cache that leaves with its
    /// chat comes back valid, and the foreground's is not thrown away every
    /// time a parked chat is serviced.
    pub(crate) transcript_cache: Option<ui::TranscriptCache>,
    pub(crate) plan_cache: Option<(ui::PlanKey, Vec<ui::VLine>)>,
    /// Scheduled prompts. They fire into the chat that armed them, on screen or
    /// not, so they travel with it.
    pub(crate) jobs: Vec<Job>,
    /// Questions waiting behind this chat's reply. See `State::asks`.
    pub(crate) asks: VecDeque<Ask>,
}

impl Chat {
    /// Whether this chat is still working off-screen: a reply arriving, a tool
    /// waiting on an answer, a plan mid-mode, or a job counting down. This is
    /// what earns a tab its `●`, the `+n` in the title and the dot in
    /// `/history`; a settled tab shows none of them.
    pub(crate) fn is_live(&self) -> bool {
        // A plan waiting on a `y`/`n` is owed an answer as surely as a stream is
        // owed its last chunk, and a chat merely `Planning` (no reply back yet)
        // is still mid-mode. A job is a watcher that will fire — and bill —
        // whether or not anyone is looking, so a chat holding one is working
        // in every sense that matters to the person deciding where to look.
        self.stream.is_some()
            || !matches!(self.status, Status::Idle)
            || !matches!(self.plan, Plan::Off)
            || !self.jobs.is_empty()
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
    /// How far up the last render could have gone: transcript lines beyond the
    /// pane's height. Lets a scroll that cannot move say so instead of asking
    /// for a frame identical to the last one — over-scrolling past the top is
    /// what a flick of the wheel does most of the time. `None` until a render
    /// has measured the transcript, trusted only while the transcript cache
    /// still matches the messages (`State::transcript_measured`), and only ever
    /// advisory: the render still clamps, so a stale value could at worst cost
    /// a missed notch, never a wrong frame.
    max_offset: Option<usize>,
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
    /// Where the user's own Zellij plugins live, captured once the permission grant makes it
    /// readable. Empty means the host could not resolve one, in which case there is nowhere to
    /// write a plugin and the plugin tools say so rather than guessing at a path.
    pub user_plugin_dir: String,
    /// The foreground chat's `plugin_reload`, while its `status` is `AwaitingPluginLoad`: the
    /// absolute path that was asked to load, and the tick it was asked on. Parked chats carry
    /// their own in `Chat::plugin_load`; `poll_plugin_load` polls those through `with_parked`.
    pub plugin_load: Option<PluginLoadWatch>,
    /// Live Lua-provided tools, shared by every conversation in this Hocus instance.
    extensions: extensions::Registry,
    /// Valid `hocus/*.lua` files from the latest scan, keyed by their namespace.
    extension_files: BTreeMap<String, String>,
    /// Running plugin ids whose resolved file location agrees with their Hocus launch config.
    extension_instances: BTreeMap<u32, (String, String)>,
    /// When to load scanned files that did not answer the discovery broadcast. The grace period
    /// lets extensions survive a Hocus reload without being duplicated.
    extension_scan_due_ms: Option<u64>,
    /// Prompts on disk typed as `/name`, shared by every conversation. See `skills`.
    skills: skills::Registry,
    /// The project root the project tier of `skills` was read from, so a tab
    /// switch that lands on the same project does not read the folder again.
    /// The root rather than the name: a rename keeps the folder.
    skills_project_root: Option<String>,
    /// Scans still to be answered, and whether the one who asked for them
    /// (`/skills reload`) is owed the listing when the last one lands.
    skills_scans_pending: usize,
    skills_announce: bool,
    /// The id of the newest scan asked for, per tier, and the counter they are
    /// drawn from. A scan answering with anything else is stale — two scans of
    /// one tier can be out at once and answer in either order — and is dropped.
    skills_scan_latest: BTreeMap<skills::Source, u64>,
    skills_scan_seq: u64,
    /// The argv of the last skill scan asked for. Test-only, for the reason
    /// `last_placement` is: the host call has no reply off wasm.
    #[cfg(test)]
    pub(crate) last_skill_scan: Option<Vec<String>>,
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
    /// Which wand frame the empty-chat splash is on. Advanced by the same timer as `tick`; the
    /// kitty protocol stores animation frames but never advances one on its own.
    pub(crate) wand_frame: usize,
    /// Clients whose pane already holds the wand's pixels. A plugin pane keeps one grid per
    /// attached client, so the frames have to be transmitted into each of them - and cleared here
    /// when the splash stops being drawn, since the images are only kept alive by the placeholder
    /// cells that reference them.
    pub(crate) wand_transmitted: BTreeSet<ClientId>,
    /// Whether the last render actually drew the wand. What keeps the timer chain running while
    /// the splash is up - and stops it the moment the chat has a message in it, the pane is set
    /// aside, or the pane is too small for the picture.
    pub(crate) wand_drawing: bool,
    /// Clients whose host terminal can display kitty graphics, as last reported in their
    /// `ModeUpdate`. An absent client starts false: the answer comes from a probe the client sends
    /// at startup, after hocus has already drawn its first frame.
    pub(crate) kitty_graphics: BTreeSet<ClientId>,
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
    /// Whether this conversation is planning rather than acting. See [`Plan`].
    pub(crate) plan: Plan,
    /// Whether the model is waiting on tool results. A call the user typed is
    /// its own destination — the result is in the transcript for the next
    /// message to build on, but it answers no question — so only the model's
    /// calls set this, and only it makes an emptied queue start a request.
    ///
    /// A decline or an abort clears it: the calls are answered for the
    /// transcript's sake, but the model's question waits on the user's next
    /// message rather than being sent back.
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
    pub transcript_cache: Option<ui::TranscriptCache>,
    /// The awaiting plan is rendered narrower inside its frame, so it has its
    /// own cache rather than making every keystroke parse the markdown again.
    pub plan_cache: Option<(ui::PlanKey, Vec<ui::VLine>)>,
    /// The provider's last token count for this chat, `None` until a reply
    /// has carried one. See `context_tokens`.
    pub(crate) usage: Option<ContextUsage>,
    /// Memoised `api::wire_chars` under the `fingerprint` it was taken at, so
    /// the title's estimate does not re-walk the transcript on every keystroke.
    pub(crate) context_cache: Option<((usize, usize), usize)>,
    /// Which archived chat this conversation is being written back to. `None`
    /// until the first save, which is what makes an untouched pane leave no file.
    chat_id: Option<u64>,
    /// `fingerprint()` as of the last save, so an event that changed nothing
    /// doesn't rewrite the file.
    saved: (usize, usize),
    /// Every open conversation other than the one on screen, in the order the
    /// tab bar shows them, with the foreground one spliced in at `chat_tab`.
    /// A parked chat keeps receiving its reply, running its tools and firing
    /// its jobs, all through `with_parked`; it stays
    /// here, settled or not, until it is closed from the bar.
    parked: Vec<Chat>,
    /// The foreground chat's position in the tab bar: the bar reads
    /// `parked[..chat_tab]`, then the foreground, then `parked[chat_tab..]`.
    chat_tab: usize,
    /// The bar position under the highlight while Shift+Tab has the bar
    /// focused. Modal while `Some`, the way `picker` is.
    chat_tab_focus: Option<usize>,
    /// The tab the overflow controls keep visible without switching chats, per client.
    chat_tab_view: BTreeMap<ClientId, usize>,
    /// Width used by the last rendered tab bar, for exact mouse hit-testing.
    chat_tab_width: usize,
    /// The archive or model browser, when it is open.
    pub picker: Option<Picker>,
    /// The agent strip as last pushed to the hocus-agents pane, so a snapshot
    /// that changes no row sends nothing. hocus owns the data (the agentd
    /// snapshot, the project registry, the branch probes); the strip itself is
    /// a separate pane - the overlay above this one - and it only draws.
    last_strip: agents::Strip,
    /// The foreground chat's scheduled prompts, counted down by the timer
    /// chain. A parked chat's are in `Chat::jobs` and tick through `with_parked`.
    pub jobs: Vec<Job>,
    /// Jobs read back from the store for chats that are not open, keyed by
    /// the chat that armed them. A restart reopens no chat, so the jobs of
    /// every chat that was open at exit land here and wait: they do not tick,
    /// and `/history` opening their chat is what hands them to it. They still
    /// count for `/stop_jobs`, and their chat shows as live in `/history`.
    pub(crate) held_jobs: BTreeMap<u64, Vec<Job>>,
    /// Questions that arrived for the foreground chat while it was busy —
    /// from the `hocus_ask` pipe or typed mid-reply — oldest first. Drained
    /// one per tick by `drain_asks`. Per chat, so a question queued at one
    /// conversation is never sent as part of another's exchange.
    ///
    /// A queue rather than a single slot because a plugin caller is *told*
    /// their question was taken: last-wins would silently drop something
    /// already acknowledged as queued, and first-wins would drop the newer
    /// question with no acknowledgement to match. Answering both in the order
    /// they were asked is the only behaviour that matches what was promised.
    ///
    /// Capped by `MAX_QUEUED_ASKS`. Past the cap the ask is refused, which is
    /// an answer the caller can see, rather than an unbounded pile of questions
    /// stacking up behind one wedged stream.
    pub asks: VecDeque<Ask>,
    job_seq: u64,
    /// The scheduled run that currently owns the conversation, if one does.
    scheduled_run: Option<ScheduledRun>,
    /// Terminal panes from hocus-agentd's latest event-fed snapshot. This is the
    /// only snapshot agent tools trust, which stops them acting on a closed pane.
    panes: Vec<agents::AgentPane>,
    /// Agent CWDs resolved by hocus-agentd to Git checkout identities.
    /// What Git said about each folder an agent is in, keyed by folder *and*
    /// machine. Filled from the daemon's snapshot; see `wire::PollReply`.
    repositories: BTreeMap<Location, projects::Repository>,
    /// Kinds the user set by hand with `agent_register`, keyed by pane id. These win
    /// over detection, and are dropped when their pane leaves the snapshot. Mirrored
    /// as `agent_latch` rows in the store, so a reload keeps them.
    manual_kinds: BTreeMap<u32, String>,
    /// The session these panes are in, from the daemon's snapshot.
    ///
    /// Half the key of every per-pane row this plugin writes. The store is one file per
    /// machine rather than per session, so a row about "pane 5" is meaningless without it —
    /// see `store::agent_latch_def`. Empty until the first snapshot, and nothing per-pane is
    /// written while it is.
    zellij_session_name: String,
    /// Whether `restore_pane_rows` has run. It waits for the session name, so it cannot ride
    /// on `restore_persisted`'s single call at load.
    restored_pane_rows: bool,
    /// Spawned panes whose ids are allocated but have not reached a pane snapshot yet.
    /// Their manual kinds survive early snapshots and are pruned normally once seen.
    pending_agent_panes: BTreeSet<u32>,
    /// Consecutive unchanged scrollback observations from hocus-agentd.
    activity: agents::Activity,
    /// What the daemon's model last made of each idle agent's pane. The daemon is the
    /// only source, the same way it is for `activity`; hocus mirrors them as
    /// `agent_summary` rows so a reload starts with the last ones rather than none.
    summaries: agents::Summaries,
    /// What each Claude Code pane has reported about itself, over its own hooks.
    /// Read-only here for the same reason and from the same writer.
    hooks: agents::HookFeeds,
    /// Cheap live and archived run rows. Full histories are fetched only for the selected row.
    active_runs: BTreeMap<u32, agents::AgentRunHeader>,
    archived_runs: Vec<agents::AgentRunHeader>,
    archive_offset: u32,
    archive_loaded_through: Option<(i64, String)>,
    archive_more: bool,
    selected_agent: Option<u32>,
    selected_run: Option<String>,
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
    /// Reported to the agents pane along with the strip, so it can tell "you are in this
    /// agent" from "you were in it". `None` whenever a terminal has the focus.
    focused_plugin: Option<u32>,
    /// The session surface the overlays are placed against, from the active tab's `TabInfo`.
    ///
    /// An overlay's coordinates are resolved against the whole session surface
    /// (`OverlayPanes::surface`), which a tab occupies in full. `overlay::surface_of` reads the
    /// pair off a `TabInfo`; `overlay::overlay_rects` does the arithmetic.
    pub(crate) surface_rows: usize,
    pub(crate) surface_cols: usize,
    /// This plugin's own pane id, read once at load.
    ///
    /// Cached rather than asked for each time: `get_plugin_ids` is a host round-trip, and the
    /// overlay paths want it on every `TabUpdate`. It never changes for the life of the plugin.
    pub(crate) own_plugin_id: u32,
    /// The pane title last asserted with `rename_plugin_pane`, so a render that
    /// would restate the same one does not.
    ///
    /// The call is not free from the host's side: it becomes a
    /// `ScreenInstruction::RenamePane`, which re-renders the screen and reports
    /// the whole session state — rebuilding every tab's pane manifest and
    /// pushing `PaneUpdate`/`TabUpdate` at every subscribed plugin. Doing that
    /// ten times a second during a stream, and once per keystroke, buried the
    /// user's input behind our own event traffic. The title only actually
    /// changes when the model or plan mode does.
    ///
    /// Cleared by `on_panes` when the manifest shows the pane under some other
    /// title, which is what keeps the reassert this replaced: placement can
    /// rename the pane behind our back.
    pub(crate) last_pane_title: Option<String>,
    /// Which tab this pane is currently on. An overlay lives on a tab, so hocus carries whichever
    /// halves are on screen to the tab that became active - `None` until the first `TabUpdate`
    /// says.
    pub(crate) own_tab_index: Option<usize>,
    /// Which tab the agent list is on, as the last `PaneUpdate` found it.
    ///
    /// Tracked separately from `own_tab_index` because the two halves can be parted: the `a` and
    /// `c` keybindings carry `move_to_focused_tab`, which moves only the pane the binding names,
    /// so a reveal from another tab brings the conversation across and leaves the list behind.
    /// `on_panes` is what notices and drags it after us.
    pub(crate) agents_tab_index: Option<usize>,
    /// Whether the agent list was drawn - not suppressed - in the last `PaneUpdate`.
    ///
    /// Read by `on_tabs`, which has no manifest of its own: a hidden conversation beside a visible
    /// list still has to carry that list to the tab the user switched to, and this is the only
    /// thing that says there is one to carry.
    pub(crate) agents_visible: bool,
    /// Where each attached client was looking, as of their last `TabUpdate`.
    ///
    /// hocus is one shared instance and the host reports tab state *per client*, so without this
    /// every client's update is a move order: two clients on different tabs would drag the panes
    /// back and forth between them for as long as they stayed attached, each move provoking the
    /// report that causes the next one. See `overlay::tab_to_follow` for the rule this feeds.
    pub(crate) client_tabs: BTreeMap<ClientId, usize>,
    /// Whether *this* pane has been set aside with Esc. The agent list hides on its own and is
    /// not covered by this.
    ///
    /// Tracked here as well as server-side because it gates two things hocus decides for itself:
    /// whether to follow tab switches (a panel with nothing on screen does not chase the user
    /// around), and where the agent list goes - the conversation owns the left-hand slot, so the
    /// list sits beside it or in its place depending on this bit.
    pub(crate) hidden: bool,
    /// A hide we have asked for and not yet seen take effect.
    ///
    /// `PaneUpdate` is a snapshot, and one taken before our `hide_self` reached the screen thread
    /// still reports us visible. Without this the transition test in `on_panes` would read that
    /// stale frame as a reveal and put the pane straight back.
    pub(crate) hide_pending: bool,
    /// Whether the overlays' geometry has actually reached the server.
    ///
    /// A pane with no coordinates is born half the surface, *centered* - that is what
    /// `OverlayPanes::base_geom` gives an overlay the keybindings launched without any - and a
    /// pane is only moved out of the middle of the screen by `apply_overlay_geometry`. That call
    /// is fire-and-forget: the screen thread looks our pane up and does nothing if no tab contains
    /// it yet, which is the state a `TabUpdate` replayed from the pre-permission cache arrives in.
    /// The old code placed the panes on the first `TabUpdate` that changed the display area and
    /// no other, so a placement lost that way was never retried and hocus opened centered.
    ///
    /// So the placement is tracked rather than assumed, and `on_tabs` and `on_panes` both keep
    /// trying until it is true. `apply_overlay_geometry` sets it from `own_tab_index`, which the
    /// `PaneManifest` is the only source of.
    pub(crate) placed: bool,
    /// The same question as [`State::placed`], asked about the *other* pane.
    ///
    /// One flag was not enough, and the gap was the whole of the bug it closes: the agent list is
    /// launched by its own keybinding and announces itself from `load`, which runs before the
    /// screen thread has put its pane on a tab. The placement `on_selection` makes on that
    /// announcement is therefore dropped in the same silence `placed` exists for - and `placed`
    /// itself, being about *our* pane, was already true, so neither retry came back to the list.
    /// It sat where `OverlayPanes::base_geom` put it, in the middle of the screen, over the
    /// conversation.
    ///
    /// `agents_tab_index` is the evidence, exactly as `own_tab_index` is for `placed`: the
    /// manifest naming the list's tab is the only word hocus gets that a placement for it could
    /// have landed. **No list counts as placed**, or the retry would spin on every event for as
    /// long as nobody had opened one.
    pub(crate) agents_placed: bool,
    /// The surface the last placement was computed against.
    ///
    /// Test-only, and a test hook rather than state: `change_overlay_coordinates` is a host call
    /// with no reply, so nothing in the plugin can otherwise tell a geometry that was restated
    /// from one that was skipped - `placed` and `agents_placed` answer *where the panes are*, not
    /// *what they were last placed against*.
    #[cfg(test)]
    pub(crate) placed_against: (usize, usize),
    /// Where the conversation sits, and what the next Ctrl-o in it moves it clear of.
    ///
    /// See [`overlay::Panel`]: the anchor, the tour and the corner are all per-panel, because
    /// both panes take the key and each moves on its own.
    pub(crate) chat: overlay::Panel,
    /// The same, for the agent list.
    ///
    /// Its `placed_corner` starts `None` and stays that way until the first press: a list nobody
    /// has moved sits in the slot beside the conversation, exactly where it always did.
    pub(crate) agents: overlay::Panel,
    /// Every pane on our tab from the last `PaneUpdate`, our own entry included.
    ///
    /// What Ctrl-o ranks. `TabUpdate` reports the surface but not the panes on it, so the manifest
    /// is the only word hocus has about where the tiled panes are, and a key can arrive at any
    /// time after it.
    pub(crate) tab_panes: Vec<PaneInfo>,
    /// The last placement sent, and whether it reclaimed a user's drag.
    ///
    /// Test-only, for the same reason `placed_against` is: the host call has no reply.
    #[cfg(test)]
    pub(crate) last_placement: Option<(Vec<(PaneId, FloatingPaneCoordinates)>, bool)>,
    /// The agents pane, as it announced itself.
    ///
    /// Learned from the first `AGENT_SELECTION` rather than assumed, because a plugin
    /// pane is only known by its id — a title is a name the user can change, and a url
    /// cannot tell two instances apart.
    agents_plugin: Option<u32>,
    /// Whether this instance was launched by the agent list's greeting, and so has to put itself
    /// away as soon as it has a pane to put away.
    ///
    /// The list needs a hocus - it is a view over rows only hocus computes - but the user who
    /// opened a list asked for a list. Set from `AGENT_STRIP_HELLO` arriving before our own first
    /// `PaneUpdate`, which nothing but the launch that provoked it can be; acted on in `on_panes`,
    /// because a `hide_self` before the pane is on a tab is dropped without a word.
    pub(crate) open_hidden: bool,
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
            user_plugin_dir: String::new(),
            plugin_load: None,
            extensions: extensions::Registry::default(),
            extension_files: BTreeMap::new(),
            extension_instances: BTreeMap::new(),
            extension_scan_due_ms: None,
            skills: skills::Registry::default(),
            skills_project_root: None,
            skills_scans_pending: 0,
            skills_announce: false,
            skills_scan_latest: BTreeMap::new(),
            skills_scan_seq: 0,
            #[cfg(test)]
            last_skill_scan: None,
            messages: Vec::new(),
            input: TextInput::empty(),
            status: Status::Idle,
            stream: None,
            reply_at: None,
            sse: SseParser::default(),
            tick: 0,
            timer_armed: false,
            wand_frame: 0,
            wand_drawing: false,
            wand_transmitted: BTreeSet::new(),
            kitty_graphics: BTreeSet::new(),
            tool_calls: ToolCallAccumulator::default(),
            pending: VecDeque::new(),
            spotlight: None,
            tool_rounds: 0,
            plan: Plan::Off,
            owes_reply: false,
            manual_head: false,
            stream_seq: 0,
            view: View::default(),
            // Overwritten by the host on the first event or render; nothing
            // reads it before then.
            client: 0,
            transcript_rows: 0,
            transcript_cache: None,
            plan_cache: None,
            usage: None,
            context_cache: None,
            chat_id: None,
            saved: (0, 0),
            parked: Vec::new(),
            chat_tab: 0,
            chat_tab_focus: None,
            chat_tab_view: BTreeMap::new(),
            chat_tab_width: 0,
            picker: None,
            last_strip: agents::Strip::default(),
            jobs: Vec::new(),
            held_jobs: BTreeMap::new(),
            asks: VecDeque::new(),
            job_seq: 0,
            scheduled_run: None,
            panes: Vec::new(),
            repositories: BTreeMap::new(),
            manual_kinds: BTreeMap::new(),
            zellij_session_name: String::new(),
            restored_pane_rows: false,
            pending_agent_panes: BTreeSet::new(),
            activity: BTreeMap::new(),
            summaries: agents::Summaries::new(),
            hooks: agents::HookFeeds::new(),
            active_runs: BTreeMap::new(),
            archived_runs: Vec::new(),
            archive_offset: 0,
            archive_loaded_through: None,
            archive_more: false,
            selected_agent: None,
            selected_run: None,
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
            own_plugin_id: 0,
            last_pane_title: None,
            surface_rows: 0,
            surface_cols: 0,
            own_tab_index: None,
            agents_tab_index: None,
            agents_visible: false,
            client_tabs: BTreeMap::new(),
            hidden: false,
            hide_pending: false,
            placed: false,
            agents_placed: true,
            #[cfg(test)]
            placed_against: (0, 0),
            chat: overlay::Panel::default(),
            agents: overlay::Panel::default(),
            tab_panes: Vec::new(),
            #[cfg(test)]
            last_placement: None,
            agents_plugin: None,
            open_hidden: false,
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

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.kdl = configuration;
        self.own_plugin_id = get_plugin_ids().plugin_id;
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
            EventType::SessionUpdate,
            // Worker replies. Not subscribing means they are silently dropped.
            EventType::TabUpdate,
            // How hocus learns it was revealed from outside - the `c` keybinding, or `show_self`.
            // `is_suppressed` on our own pane is the bit, and a `PaneUpdate` is the only place it
            // is reported. Deliberately not `EventType::Visible`, which is about the *tab* going
            // off screen and fires on every tab switch: reading that as "I was hidden" would fight
            // the reveal below.
            EventType::PaneUpdate,
            // Only for `capabilities.kitty_graphics`, which decides whether the empty-chat splash
            // is an image or its text stand-in.
            EventType::ModeUpdate,
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
            // wasm anyway (see `BUILTIN_PLUGIN_DEBUG_DIR`), so a `file:` load is
            // a thing you have to go out of your way to do.
        ]);

        // The agent list is its own pane, opened by the user and not by us: the two are separate
        // surfaces, and a chat that drags a list up with it is exactly what that separation
        // forbids. What this does is *ask* - an existing hocus-agents instance (a reload, a
        // session restore, the `a` keybinding having got here first) answers with its plugin id,
        // which is what lets us place it beside ourselves. No url, so nothing is launched.
        // Fire-and-forget, so calling it before the permission grant resolves is fine - and a
        // builtin is exempt from the gate anyway.
        self.greet_agents_pane();
        // Places this pane, and the agent list once it announces itself. Does nothing useful
        // until the first `TabUpdate` says how big the screen is, which is why `on_tabs` calls
        // it too.
        self.apply_overlay_geometry();
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
        // The empty-chat wand is the one animation nothing else arms a timer for, and `render`
        // cannot arm one itself - a host call there would be handed the half-printed frame as its
        // argument. So it is done here, where nothing has been printed yet.
        if self.wants_frames() {
            self.arm_timer(FRAME_SECONDS);
        }
        render
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // A render can expose the wand without an update first: on initial draw, after `/clear`,
        // or when a resize makes it fit. Arm before printing, while stdout is still safe for a
        // host call; the tick will stop immediately if this render does not draw the wand.
        self.arm_timer(FRAME_SECONDS);
        // Called once per attached client, each pass with a different client set here. Their view
        // and terminal capabilities differ; the conversation itself is shared.
        self.client = get_plugin_ids().client_id;
        ui::render(self, rows, cols);
    }

    /// Snapshots pushed by `hocus-agentd`.
    ///
    /// Every snapshot ends up in `apply_poll`. There is no second path into it
    /// any more; the daemon is the only source of panes and folders.
    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == extensions::REGISTER {
            let PipeSource::Plugin(owner) = pipe_message.source else {
                return false;
            };
            return self.on_extension_registration(owner, pipe_message.payload.as_deref());
        }
        if pipe_message.name == extensions::RESULT {
            let PipeSource::Plugin(owner) = pipe_message.source else {
                return false;
            };
            return self.on_extension_result(owner, pipe_message.payload.as_deref());
        }
        match pipe_message.name.as_str() {
            wire::AGENT_HELLO => {
                self.subscribe_to_agentd();
                return false;
            },
            wire::AGENT_STRIP_HELLO => {
                // A freshly loaded (or restored) hocus-agents pane asking for the current rows.
                //
                // It carries a `plugin_url`, so it may also be what launched *this* instance -
                // and in that case the user asked for a list, not for a conversation. Nothing but
                // that launch can reach us before our own first `PaneUpdate`, so `own_tab_index`
                // being unset is the test. `on_panes` does the hiding, once there is a pane to
                // hide; see `State::open_hidden`.
                self.note_greeting_from_agents(
                    pipe_message.args.get(wire::OPENED_BY).map(String::as_str)
                        == Some(wire::OPENED_BY_AGENTS),
                );
                self.push_strip(true);
                return false;
            },
            wire::AGENT_SELECTION => return self.on_selection(&pipe_message),
            wire::AGENT_STRIP_REPOSITION => {
                // Ctrl-o in the agent list. hocus owns both panes' geometry, so the list asks
                // rather than moves; nothing to redraw here, the move is the server's to paint.
                self.reposition(overlay::Which::Agents);
                return false;
            },
            wire::AGENT_ARCHIVE_MORE => {
                self.load_more_agent_runs();
                self.push_strip(false);
                return false;
            },
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
        self.zellij_session_name = reply.zellij_session_name;
        // The first snapshot is the first thing that says which session's panes these are, and
        // so the first moment the per-pane rows can be read. See `restore_pane_rows`.
        self.restore_pane_rows();
        self.repositories = reply.repositories;
        self.merge_run_headers(reply.runs);

        // Outside the panes below: a quiet reply — one from a daemon with no session
        // to report — still says where the focus went, and the strip carries it on
        // to the agents pane whether or not there were panes in this reply.
        self.focused_plugin = reply.focused_plugin;
        if let Some(panes) = reply.panes {
            self.apply_poll(
                panes,
                reply.activity,
                reply.summaries,
                reply.hooks,
                reply.focused,
            );
        }
        // This also answers the branch refresh the agents pane asks for, and carries the
        // summaries the box under its list is drawn from.
        self.push_strip(false);
        // A snapshot is not worth a repaint here: nothing on *this* pane's screen comes from
        // one. What it changes is the agents pane, and `push_strip` is what tells it.
        false
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
                if context.contains_key(extensions::SCAN_CONTEXT) {
                    return self.on_extension_scan_result(exit_code, &stdout);
                }
                // Host state, like the extension scan: answered on the
                // foreground whatever chat is up.
                if context.contains_key(skills::SCAN_CONTEXT) {
                    let source = context
                        .get(skills::SOURCE_CONTEXT)
                        .and_then(|label| skills::Source::parse(label));
                    let scan_id = context
                        .get(skills::SCAN_ID_CONTEXT)
                        .and_then(|id| id.parse::<u64>().ok());
                    return match (source, scan_id) {
                        (Some(source), Some(scan_id)) => {
                            self.on_skill_scan_result(source, scan_id, exit_code, &stdout, &stderr)
                        },
                        _ => false,
                    };
                }
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
                let skills_dir_was = self.skills_dir();
                self.kdl = kdl;
                self.reconfigure();
                // `skills_dir` is read straight from the block, so a change to
                // it is only noticed here.
                if self.skills_dir() != skills_dir_was {
                    self.start_user_skill_scan();
                }
                // `width`, `height` and `pad` live in the same block, and nothing else re-reads
                // them: without this the panes keep their old geometry until an unrelated resize
                // or tab move happens to re-place them. Not folded into `reconfigure`, which is
                // gated on the permission grant - placing a pane is not.
                self.apply_overlay_geometry();
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
            // Subscribed to for one bit: whether the host terminal can show an image. It arrives
            // from a probe the client sends at startup, so it lands after the first render and
            // flips the empty-chat splash over from text to the real wand.
            Event::ModeUpdate(mode_info) => {
                if mode_info.capabilities.kitty_graphics {
                    self.kitty_graphics.insert(self.client)
                } else {
                    self.kitty_graphics.remove(&self.client)
                }
            },
            Event::PaneUpdate(manifest) => self.on_panes(manifest),
            Event::SessionUpdate(sessions, _) => self.on_sessions(sessions),
            Event::BeforeClose => {
                self.cancel_in_flight();
                // Nobody is left to press `y`, and the mark outlives the plugin:
                // a pane would otherwise sit there titled for a question that
                // can no longer be answered.
                self.drop_spotlight();
                // A question the asker was told had been queued must not simply
                // vanish. As a notice it survives into the archive and shows up
                // in /history, and — being a notice — it never reaches the
                // model, so reopening the chat does not answer it by surprise.
                archive::unsent_asks_to_notices(&mut self.messages, &mut self.asks);
                for chat in &mut self.parked {
                    archive::unsent_asks_to_notices(&mut chat.messages, &mut chat.asks);
                }
                // Nothing left to flush but the transcripts. Projects, todos,
                // jobs, latches and summaries are written as each change lands,
                // and so is every prompt.
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
