// The tool schemas are one `json!` literal each, and the largest is past the default limit.
#![recursion_limit = "256"]

//! hocus — chat with an OpenAI-compatible endpoint from a Zellij popup.
//!
//! A library so the wasm entry point (`src/main.rs`) and the render benchmark
//! (`benches/transcript.rs`) can both see it; a bin crate has nothing to import.

mod api;
mod archive;
mod decisions;
mod events;
mod extensions;
mod help;
mod history;
mod log;
mod message;
mod overlay;
mod plugin_docs;
mod plugin_source;
mod present;
mod skills;
mod tool_loop;
mod tools;
mod transitions;
mod vm_panel;
pub use transitions::{Confirmation, PromptKind};
use zellij_tile::prelude::DataWatchId;
use zellij_tile::prelude::HostId;
use zellij_tile::prelude::TabId;
use zellij_tile::prelude::TabPosition;
use zellij_types::internal_ids::SessionName;
pub mod ui;
mod wand;
mod worker;
mod workflows;

pub use message::{
    Ask, Job, Message, Picker, PickerKind, PickerRow, Plan, Reasoning, ReasoningItem, ReasoningKey,
    Retry, Role, RowKind, ScheduledRun, Status, StreamState, TokenUsage,
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

/// How big a request has to be before its makeup is logged as well as its size.
///
/// The rate limit that started all of this arrived at ~48 000 tokens, so this is well below the
/// point where the question becomes interesting and well above an ordinary turn.
const BREAKDOWN_BYTES: usize = 100_000;

/// How many times a request the endpoint turned away is sent again before the refusal is
/// shown as an error and the turn ends.
///
/// Four, on the doubling backoff `hocus_proto::llm::retry_delay` falls back to, is a little
/// over half a minute of waiting — long enough to ride out a per-minute token budget refilling,
/// short enough that a provider which is actually down says so rather than spinning.
const MAX_RETRIES: u32 = 4;

/// How long a `plugin_reload` waits before reading the plugin's pane. A script plugin loads in
/// well under this, a TypeScript one's module graph compiled and cached included; what the wait
/// covers is the screen thread opening the pane and the first render landing in it, so that
/// what is read back is the plugin and not the loading screen.
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
    /// A plugin loaded headless on request: no pane, no render - its log is the report.
    pub headless: bool,
    /// The name the call gave, which is what `read_plugin_log` takes.
    pub name: String,
}

/// A plugin running in the session, as the plugin list reports it, plus the one thing the list
/// does not say: whether a pane shows it. A headless `plugin_reload` reloads the instance
/// without a pane and leaves the pane copies alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInstance {
    /// The `file:` (or other) location the plugin was loaded from.
    pub location: String,
    /// Whether some pane in the session is this plugin's.
    pub has_pane: bool,
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
const LOCAL_COMMANDS: [&str; 9] = [
    "help",
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
Ctrl+l        new chat, in a new tab (also /clear)
Ctrl+o        move the panel one corner clockwise - bottom left, top left,
              top right, bottom right, and round again
Tab, Shift+Tab   walk the panel's ring of stops: the agent list, its history
              half, the chat tab bar, the box you type in - and round again
Up / Down     previous / next message in history
Tab           complete a /command or @agent, when there is something half
              typed to complete. A box with a draft in it keeps Tab for that;
              Esc lets go of the box so Tab walks the ring again, and
              Shift+Tab walks it either way
Alt+Enter, Shift+Enter   insert a newline
PageUp / PageDown        scroll the transcript
Esc           let go of a box you have typed in, keeping the draft; from an
              empty box, or one already let go of, hide the panel - the agent
              list goes with it, and Ctrl+e c or Ctrl+e a brings it all back
Ctrl+c        cancel a stream or tool, keep refining a
              plan, or clear the prompt
y / n         run the pending tool call, or decline it and stop
n             keep refining a plan hocus has written
/plan <text>  plan before acting: read-only tools until
              you /execute the plan hocus writes
/plan off     leave planning mode without carrying the plan
              out; the plan stays in the transcript
/execute      carry out that plan; trailing text amends it
/<skill> <text>  send a skill's prompt with <text> as its
              arguments; /skills lists them, /skills reload
              rescans, /skills enable|disable <scope> <name>
              changes one

In history or models:
type          narrow the list to the rows containing what you type
Tab           complete what you have typed as far as the matches agree
Backspace, Ctrl+u  erase a character, or the whole filter
Up / Down     choose
Enter         select
Esc           clear the filter, or close the list once it is empty

With a box you have let go of:
Tab, Shift+Tab   walk the ring
Enter         send what is in the box
type anything    take the box back and carry on writing
Esc           hide the panel

With the tab bar focused:
Left / Right  choose
Tab, Shift+Tab   leave the bar - the box below, or the agent list
Enter         switch to that chat
x             close it
Ctrl+l        new chat
Esc           back to the prompt

In the agent list (the panel's other half):
Tab, Shift+Tab   walk the ring on into the conversation
Up / Down     choose an agent, from either of the list's halves
j / k         step by whatever the half you are in is made of
Enter         focus that agent's pane, or open the event under the cursor
s             ask for a summary of the agent under the cursor
Ctrl+o        move the panel     Ctrl+r  re-read the branch
Esc           hide the panel, the conversation with it";

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
    pub(crate) saved: (usize, usize, Option<TokenUsage>, Plan),
    pub(crate) view: View,
    pub(crate) scheduled_run: Option<ScheduledRun>,
    /// A rate-limited request waiting to go out again. Travels with the chat for the reason
    /// `plugin_load` does: it is *this* conversation's request that was turned away, and a
    /// parked chat is as able to wait out a rate limit as a foreground one.
    pub(crate) retry: Option<Retry>,
    pub(crate) plan: Plan,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) last_request_input: Option<u64>,
    /// Travels with the chat for the same reason `spotlight` does: the reload was asked for by
    /// *this* conversation, and it is the one owed the report. A watch left on `State` alone
    /// would be overwritten by another chat's reload and stranded when this one is parked.
    pub(crate) plugin_load: Option<PluginLoadWatch>,
    /// `mono_ms` when this chat's awaited extension call was sent. With the chat for the
    /// reason `plugin_load` is: two chats mid-call would otherwise share one clock, and a
    /// parked chat's call would never be given up on.
    pub(crate) extension_call_started_ms: Option<u64>,
    pub(crate) workflow_call: Option<workflows::Pending>,
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
    /// Live extension-provided tools, shared by every conversation in this Hocus instance.
    extensions: extensions::Registry,
    /// Valid `hocus/*.{ts,js,mjs,mts}` files from the latest scan, keyed by their namespace.
    extension_files: BTreeMap<String, String>,
    /// Running plugin ids whose resolved file location agrees with their Hocus launch config.
    extension_instances: BTreeMap<zellij_tile::prelude::PluginId, (String, String)>,
    /// Every running plugin by id, from the session's plugin list - what a headless
    /// `plugin_reload` looks the running instance up in.
    plugin_instances: BTreeMap<zellij_tile::prelude::PluginId, PluginInstance>,
    /// `mono_ms` when the foreground chat's awaited extension call was sent;
    /// `poll_extension_call` gives up on it after `extensions::CALL_TIMEOUT_MS`. Parked chats
    /// carry their own in `Chat::extension_call_started_ms`.
    extension_call_started_ms: Option<u64>,
    workflow_call: Option<workflows::Pending>,
    workflow_seq: u64,
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
    /// Provider-reported input/output totals for this chat.
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) last_request_input: Option<u64>,
    /// Which archived chat this conversation is being written back to. `None`
    /// until the first save, which is what makes an untouched pane leave no file.
    chat_id: Option<u64>,
    /// `fingerprint()` as of the last save, so an event that changed nothing
    /// doesn't rewrite the file.
    saved: (usize, usize, Option<TokenUsage>, Plan),
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
    /// The input box has let go of the keyboard: Esc pressed with a draft in it.
    ///
    /// One of the panel's four ring stops is the input box, and the box owns bare Tab for
    /// `/command` and `@agent` completion - so with text typed there is nothing left for Tab
    /// to mean "step forward" with. Esc is what releases it: the draft stays exactly where it
    /// is, the cursor goes away (`ui::render` draws `show_cursor(None)`), and Tab walks the
    /// ring again. Typing anything or pressing Enter takes the box straight back, which is
    /// why this is cleared on `InputAction::Continue` and `InputAction::Submit` and nowhere
    /// else - Up/Down walk the message history and are not typing.
    ///
    /// Not modal, unlike `picker` and `chat_tab_focus`: every key but Tab and Esc means what
    /// it always meant, and lands in the box as it always did.
    chat_input_released: bool,
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
    /// The foreground chat's rate-limited request, waiting on `poll_retry` to resend it. A
    /// parked chat's is in `Chat::retry` and is polled through `with_parked`.
    pub(crate) retry: Option<Retry>,
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
    zellij_session_name: SessionName,
    /// The machine this hocus runs on, read once at load. With `zellij_session_name` it is what
    /// owns this session's rows in the shared store — see `agents::rows::Owner`.
    host_id: HostId,
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
    selected_run_revision: Option<u64>,
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
    vm_panel_owner: Option<PluginId>,
    vm_panel_pending: Option<(PluginId, hocus_proto::vms::Request)>,
    vm_panel_snapshot: hocus_proto::vms::Snapshot,
    vm_panel_current_session: Option<String>,
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
    project_ids: BTreeMap<String, zellij_tile::prelude::DataRowId>,
    /// The watch feeding `projects`, so a change event can be told from another
    /// plugin's.
    project_watch: DataWatchId,
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
    tab_projects: BTreeMap<TabId, String>,
    /// Tab id to project name, as the poll worked it out from the agents in the
    /// tab. Rebuilt on every poll. `tab_projects` wins over this map.
    auto_tab_projects: BTreeMap<TabId, String>,
    /// `tab_projects` as the daemon last heard it, so an unchanged map costs no
    /// message. See `send_tab_assignments`.
    sent_tab_projects: BTreeMap<TabId, String>,
    /// What each tab is called after the project in it, as last told to the host.
    ///
    /// Not the same map as the two above: a tab whose agents are in two projects
    /// is named after both, where an *assignment* has to pick one and so picks
    /// neither. Kept so the host is only told when the answer changes — this is
    /// recomputed on every poll and the answer is usually the same one.
    tab_project_labels: BTreeMap<TabId, String>,
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
    /// The session surface the overlays are placed against, from the active tab's `TabInfo`.
    ///
    /// An overlay's coordinates are resolved against the whole session surface
    /// (`OverlayPanes::surface`), which a tab occupies in full. `overlay::surface_of` reads the
    /// pair off a `TabInfo`; `overlay::panel_size` does the arithmetic.
    pub(crate) surface_rows: usize,
    pub(crate) surface_cols: usize,
    /// This plugin's own pane id, read once at load.
    ///
    /// Cached rather than asked for each time: `get_plugin_ids` is a host round-trip, and the
    /// overlay paths want it on every `TabUpdate`. It never changes for the life of the plugin.
    pub(crate) own_plugin_id: zellij_tile::prelude::PluginId,
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
    /// The tab that owns the container, as reported by the host.
    pub(crate) own_tab_index: Option<TabPosition>,
    /// Whether our own pane is an overlay, as the manifest reports it - and so whether this
    /// instance is a *panel* at all. `None` until a `PaneUpdate` has named our pane.
    ///
    /// hocus is normally drawn over the session as an overlay, and everything in `overlay` is
    /// about that: a second half, a corner, a draggable divider, one visibility for the pair. But
    /// nothing stops a user putting `plugin location="hocus"` in their layout, and that instance
    /// is a plain tiled pane which owns none of it. Told apart by `is_floating`, which an overlay
    /// reports unconditionally (`OverlayPanes::pane_info`) and a layout pane never does. There is
    /// no configuration to read it from instead: `launch::overlay_configuration` is part of plugin
    /// identity and has to match the keybindings character for character.
    ///
    /// Read through [`State::is_panel`], which answers `false` while this is `None` - a placement
    /// before the first manifest could not land anyway, and `on_panes` retries every one that
    /// could not.
    pub(crate) own_is_floating: Option<bool>,
    /// Whether the server has the keyboard on this pane, off the last manifest.
    ///
    /// The ring's stop state lives in two fields (`chat_tab_focus` and `chat_input_released`)
    /// that only mean anything while this half is the focused one. Focus can leave by routes
    /// the ring knows nothing about - a click on the other half, `Ctrl e a` - and a stop left
    /// standing through one of those would greet the user with the tab bar highlighted, or a
    /// released box, the next time they came back. So `on_panes` resets both when this goes
    /// false. `None` until the first manifest, which is not "unfocused": nothing to reset yet.
    pub(crate) own_is_focused: Option<bool>,
    /// `PaneUpdate`s a focus this half asked for itself may discount before the reset above
    /// applies again - see [`overlay::RING_FOCUS_PENDING_FRAMES`], which is what sets it.
    ///
    /// A ring step across the seam sets the stop and *then* asks for the focus, and the
    /// manifests already in flight still report this half unfocused. Without the budget one of
    /// those lands in the reset and takes the new stop with it.
    pub(crate) ring_focus_pending: u8,
    /// The agent list's tab, used to validate adoption before the container exists.
    pub(crate) agents_tab_index: Option<TabPosition>,
    /// Last visibility sent to the VM panel, to avoid duplicate notifications.
    pub(crate) agents_visible: bool,
    /// Where each attached client was looking, as of their last `TabUpdate`.
    ///
    /// hocus is one shared instance and the host reports tab state *per client*, so without this
    /// every client's update is a move order: two clients on different tabs would drag the panes
    /// back and forth between them for as long as they stayed attached, each move provoking the
    /// report that causes the next one. See `overlay::tab_to_follow` for the rule this feeds.
    pub(crate) client_tabs: BTreeMap<ClientId, TabPosition>,
    /// Container visibility from the host, also used to decide whether to follow tab switches.
    pub(crate) hidden: bool,
    /// Whether creation succeeded or the manifest identified the existing container.
    pub(crate) placed: bool,
    /// The surface used for the last placement request, captured by tests.
    #[cfg(test)]
    pub(crate) placed_against: (usize, usize),
    /// Where the panel sits: the corner it is in and the rectangle it asked for.
    ///
    /// See [`overlay::Panel`]: the agent list and the conversation are the two halves of one
    /// panel, so there is one corner between them - Ctrl-o in either half advances it one corner
    /// clockwise.
    pub(crate) panel: overlay::Panel,
    pub(crate) multipane: Option<MultipaneInfo>,
    /// The last placement sent, and whether it reclaimed a user's drag.
    ///
    /// Test-only capture of the placement request.
    #[cfg(test)]
    pub(crate) last_placement: Option<(Vec<(PaneId, FloatingPaneCoordinates)>, bool)>,
    /// The agents pane, as it announced itself.
    ///
    /// Learned from the first `AGENT_SELECTION` rather than assumed, because a plugin
    /// pane is only known by its id — a title is a name the user can change, and a url
    /// cannot tell two instances apart.
    agents_plugin: Option<zellij_tile::prelude::PluginId>,
    /// Every `show`/`hide` this plugin asked the host for, as `(pane, shown)`.
    ///
    /// Test-only, for the same reason `last_placement` is: the host call has no reply off wasm,
    /// so nothing else can tell a hide that was sent from one that was skipped.
    #[cfg(test)]
    pub(crate) visibility_calls: Vec<(PaneId, bool)>,
    /// How many agent lists have been asked for. Test-only, same reason.
    #[cfg(test)]
    pub(crate) agents_launches: usize,
    /// Every ring step that crossed the seam, as the stop the agent list was sent to.
    ///
    /// Test-only, for the same reason `visibility_calls` is: `focus_pane_with_id` and the
    /// `AGENT_STRIP_TAKE` beside it have no reply off wasm, so nothing else can tell a step
    /// that was sent from one the panel declined to make.
    #[cfg(test)]
    pub(crate) ring_focus_calls: Vec<(PaneId, &'static str)>,
    /// The project of `last_focused_agent`, falling back to the active tab in
    /// `tabs`.
    active_project: Option<String>,
    /// The quit notice last published, so a pass that would restate the same one does not.
    ///
    /// Memoised for the reason `last_pane_title` is: the call becomes a host instruction, and
    /// this runs on every update - including every keystroke and every streamed chunk. What it
    /// says only actually changes when a conversation starts or finishes working.
    last_quit_notice: Option<(Option<String>, bool)>,
    /// How many quit notices have actually gone out to the host.
    ///
    /// Test-only, for the same reason `last_placement` is: the host call has no reply off wasm,
    /// so nothing else can tell a notice that was sent from one the memo skipped.
    #[cfg(test)]
    pub(crate) quit_notice_publishes: usize,
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
            plugin_instances: BTreeMap::new(),
            extension_call_started_ms: None,
            workflow_call: None,
            workflow_seq: 0,
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
            client: ClientId::new(0),
            transcript_rows: 0,
            transcript_cache: None,
            plan_cache: None,
            usage: None,
            last_request_input: None,
            chat_id: None,
            saved: (0, 0, None, Plan::Off),
            parked: Vec::new(),
            chat_tab: 0,
            chat_tab_focus: None,
            chat_input_released: false,
            chat_tab_view: BTreeMap::new(),
            chat_tab_width: 0,
            picker: None,
            last_strip: agents::Strip::default(),
            jobs: Vec::new(),
            held_jobs: BTreeMap::new(),
            asks: VecDeque::new(),
            job_seq: 0,
            scheduled_run: None,
            retry: None,
            panes: Vec::new(),
            repositories: BTreeMap::new(),
            manual_kinds: BTreeMap::new(),
            zellij_session_name: SessionName::default(),
            host_id: HostId::default(),
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
            selected_run_revision: None,
            agent_seq: 0,
            mono_ms: 0,
            log_seq: 0,
            granted: false,
            vm_panel_owner: None,
            vm_panel_pending: None,
            vm_panel_snapshot: Default::default(),
            vm_panel_current_session: None,
            projects: BTreeMap::new(),
            project_ids: BTreeMap::new(),
            project_watch: DataWatchId::new(0),
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
            sent_tab_projects: BTreeMap::new(),
            tab_project_labels: BTreeMap::new(),
            tabs: Vec::new(),
            last_focused_agent: None,
            focused_pane: None,
            own_plugin_id: zellij_tile::prelude::PluginId::new(0),
            last_pane_title: None,
            surface_rows: 0,
            surface_cols: 0,
            own_tab_index: None,
            own_is_floating: None,
            own_is_focused: None,
            agents_tab_index: None,
            agents_visible: false,
            client_tabs: BTreeMap::new(),
            hidden: false,
            ring_focus_pending: 0,
            placed: false,
            #[cfg(test)]
            placed_against: (0, 0),
            panel: overlay::Panel::default(),
            multipane: None,
            #[cfg(test)]
            last_placement: None,
            agents_plugin: None,
            #[cfg(test)]
            visibility_calls: Vec::new(),
            #[cfg(test)]
            agents_launches: 0,
            #[cfg(test)]
            ring_focus_calls: Vec::new(),
            active_project: None,
            last_quit_notice: None,
            #[cfg(test)]
            quit_notice_publishes: 0,
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
        let ids = get_plugin_ids();
        self.own_plugin_id = ids.plugin_id;
        // Which machine's rows this hocus writes. Constant for the process, and read before the
        // store is opened so that nothing can write a row without it.
        self.host_id = ids.host_id;
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
            // How a click on one of hocus's own toasts comes back - see `wire::TOAST_CLICK`. The
            // host has already focused our pane by then; this is what says which conversation the
            // toast was about, which the focus alone cannot.
            EventType::CustomMessage,
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

        // Discover an existing agents child before composing the panel.
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
        // And for the same reason: whether a quit should be confirmed changes on exactly the
        // paths a reply, a tool run or a cancel finishes on, and none of them should have to
        // remember to say so.
        self.publish_quit_notice();
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
        if matches!(
            pipe_message.name.as_str(),
            "zellij_rpc_reply" | "zellij_rpc_progress"
        ) {
            if let (PipeSource::Plugin(owner), Some(payload)) =
                (&pipe_message.source, &pipe_message.payload)
            {
                if self.on_workflow_reply(
                    *owner,
                    payload,
                    pipe_message.name == "zellij_rpc_progress",
                ) {
                    return true;
                }
            }
        }
        if pipe_message.name == extensions::RESULT {
            let PipeSource::Plugin(owner) = pipe_message.source else {
                return false;
            };
            return self.on_extension_result(owner, pipe_message.payload.as_deref());
        }
        if pipe_message.name == hocus_proto::vms::REQUEST {
            return self.on_vm_panel_request(pipe_message);
        }
        match pipe_message.name.as_str() {
            wire::AGENT_HELLO => {
                self.subscribe_to_agentd();
                return false;
            },
            wire::AGENT_STRIP_HELLO => {
                // A freshly loaded (or restored) hocus-agents pane asking for the current rows.
                //
                // It carries a `plugin_url`, so it may also be what launched *this* instance.
                // The panel is both halves, so that conversation belongs on screen - what it
                // must not take is the focus, which the greeting itself settles. What is noted
                // here is only that a list already exists, so the first manifest does not go
                // asking for a second one. Nothing but that launch can reach us before our own
                // first `PaneUpdate`, so `own_tab_index` being unset is the test.

                self.push_strip(true);
                return false;
            },
            wire::AGENT_SELECTION => return self.on_selection(&pipe_message),
            wire::AGENT_STRIP_REPOSITION => {
                // Ctrl-o in the agent list. hocus owns the panel's geometry, so the list asks
                // rather than moves; nothing to redraw here, the move is the server's to paint.
                self.reposition_from_list();
                return false;
            },
            wire::AGENT_STRIP_HIDE => {
                // Esc or agent selection hides the host-owned container.
                self.hide_panel();
                return false;
            },
            wire::AGENT_STRIP_YIELD => {
                // Tab or Shift+Tab walked off the end of the agent list's two stops. The list
                // knows only its own plugin id, so crossing the seam is a request - the same
                // shape as the Esc and Ctrl-o arms above.
                return self
                    .take_ring_focus(pipe_message.args.get(wire::PANEL_STOP).map(String::as_str));
            },
            wire::AGENT_ARCHIVE_MORE => {
                self.load_more_agent_runs();
                self.push_strip(false);
                return false;
            },
            wire::AGENT_PREDECESSORS => return self.on_agent_predecessors(&pipe_message),
            wire::AGENT_SEARCH => return self.on_agent_search(&pipe_message),
            wire::AGENT_RESUME => {
                // `r` then `y` in the agent list, on a row in the archive. Nothing to redraw
                // here - what this opens is a terminal pane, and the strip that follows is
                // what puts the run back among the live agents.
                let request = pipe_message
                    .payload
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<wire::ResumeRequest>(json).ok());
                // The sender rather than `agents_plugin`, which is only set once the panel
                // has announced itself: the answer has to go back to whoever asked, or a
                // confirmed `y` is a key that does nothing at all. `agents_plugin` is the
                // fallback for a request that came from somewhere else entirely.
                let plugin_id = match pipe_message.source {
                    PipeSource::Plugin(id) => Some(id),
                    _ => self.agents_plugin,
                };
                if let (Some(request), Some(plugin_id)) = (request, plugin_id) {
                    self.resume_run(plugin_id, &request.run_id);
                }
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
                if context.contains_key(vm_panel::CONTEXT) {
                    return self.on_vm_panel_result(exit_code, &stdout, &stderr, &context);
                }
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
            Event::Timer(elapsed) => self.on_timer((elapsed).get()),
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
                self.vm_panel_current_session = mode_info.session_name.map(|name| name.to_string());
                if mode_info.capabilities.kitty_graphics {
                    self.kitty_graphics.insert(self.client)
                } else {
                    self.kitty_graphics.remove(&self.client)
                }
            },
            Event::PaneUpdate(manifest) => self.on_panes(manifest),
            // Somebody clicked a toast hocus raised. The host has already focused our pane; this
            // arm is what makes the click land on the conversation the toast was actually about.
            Event::CustomMessage(name, payload) if name.starts_with("__zellij_plugin_test:") => {
                self.on_plugin_test_result(&name["__zellij_plugin_test:".len()..], &payload)
            },
            Event::CustomMessage(name, payload) if name == wire::TOAST_CLICK => {
                self.on_toast_click(payload.parse::<u64>().ok())
            },
            Event::SessionUpdate(sessions, _) => self.on_sessions(sessions),
            Event::BeforeClose => {
                // Before anything else: whatever was outstanding is about to be cancelled, and
                // a guard outliving the plugin that set it is a session that will not close its
                // last pane and cannot say why.
                self.withdraw_quit_notice();
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
