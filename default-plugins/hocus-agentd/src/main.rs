//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! The daemon takes one startup snapshot, follows Zellij's pane, command, and CWD
//! events, and samples agent output on a small heartbeat for activity and Git changes.

mod messages;
mod poll;

use hocus_proto::agents::now_ms;
use hocus_proto::completion;
use hocus_proto::envelope;
use hocus_proto::llm::Endpoint;
use hocus_proto::location::Location;
use hocus_proto::panes::{focused_pane, manifest_pane_ids, terminal_panes};
use hocus_proto::store::{self, Backend};
use hocus_proto::wire::{self, PollReply, PollRequest};
use hocus_proto::{agents, projects};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use zellij_tile::prelude::HostId;
use zellij_tile::prelude::SessionName;
use zellij_tile::prelude::TabId;
use zellij_tile::prelude::*;

const ACTIVITY_SECONDS: f64 = 5.0;
const GIT_REFRESH_SECONDS: f64 = 60.0;
/// How long to leave a folder alone after a probe that answered nothing.
///
/// `resolve_repositories` runs on nearly every host event and nothing is cached
/// for a failure, so without a wait a missing `git` would be re-run forever.
const GIT_RETRY_SECONDS: f64 = 30.0;
/// How many folders one heartbeat will ask the host about. Each ask is a
/// blocking round-trip, and a pane that has none now will still have none next
/// tick, so there is no reason to pay for them all at once.
const CWD_REPAIRS_PER_TICK: usize = 4;
/// How many panes one heartbeat will ask the host the running command of.
///
/// The same round-trip cost as `CWD_REPAIRS_PER_TICK`, and the same reason for a
/// cap. Unlike a folder, though, a command that is right now can be wrong later,
/// so this budget is spent on a rotation rather than only on the panes we know
/// nothing about — see `repair_commands_with`.
const COMMAND_REPAIRS_PER_TICK: usize = 4;
/// How many folders one pass will ask Git about.
///
/// Two processes per folder, and `resolve_repositories` runs on nearly every
/// host event, so a session that opens twenty panes at once must not spawn forty
/// gits in one breath. The leftovers are taken by the next event or the
/// heartbeat; every filter in `repository_cwds` makes a repeated pass idempotent.
const GIT_PROBES_PER_PASS: usize = 8;
/// How many remote panes one heartbeat will ask the host the Git probe of.
///
/// Cheaper than a local probe — one channel round trip against a cached answer,
/// with no process anywhere — but still a blocking round trip, and the same
/// reason for a cap as `COMMAND_REPAIRS_PER_TICK`. What is left over is taken by
/// the next tick, which resumes the rotation where this one stopped.
const REMOTE_PROBES_PER_TICK: usize = 4;
/// The alias whose KDL block configures the endpoint summaries are asked of.
///
/// hocus's block, not this daemon's: the endpoint, the model and the key are one setting
/// for the whole of hocus, and making the user write them twice to get summaries would be
/// a configuration trap. Read through `get_plugin_alias_configuration`, which answers for
/// an alias whether or not its plugin has ever been launched — hocus usually has not, at
/// the moment the daemon starts.
const HOCUS_ALIAS: &str = "hocus";

/// How many heartbeats a summary request may be outstanding before the pane is allowed to
/// ask again. Twelve ticks is a minute — far longer than any answer worth waiting for, and
/// short enough that a dropped result does not cost the pane its summaries for the session.
const SUMMARY_TICKS: u32 = 12;

/// Context keys correlating a summary's `WebRequestResult` back to the pane that asked.
/// The same scheme the git probes use, for the same reason: results arrive on one event
/// for the whole plugin.
const SUMMARY_PANE: &str = "summary_pane";
const SUMMARY_ID: &str = "summary_id";
/// The same, for a summary of an archived run, which has no pane to name.
///
/// A key of its own rather than a nullable `SUMMARY_PANE`, because the two lanes are
/// correlated in different maps and answer to different rules — a run has no screen to go
/// stale against, no pane to rename and nothing to toast over. Which key is present is what
/// `finish_summary` reads to tell them apart, so a reply can never be taken up by the wrong
/// half.
const SUMMARY_RUN: &str = "summary_run";

/// Context key correlating a completion's `WebRequestResult` back to the caller waiting on
/// it.
///
/// A key of its own rather than a reused [`SUMMARY_ID`], for the reason [`SUMMARY_RUN`] is
/// one: which key is present is what routes the reply, and an answer taken up by the wrong
/// lane would rename somebody's pane after another plugin's question.
const COMPLETE_ID: &str = "complete_id";
/// How many heartbeats a completion may be outstanding before its caller is told nothing
/// came and the slot is reclaimed.
///
/// Twenty-four ticks is two minutes — longer than [`SUMMARY_TICKS`] because a caller
/// driving its own tool loop may be asking a reasoning model something slow, and shorter
/// than any caller's patience. Anyone who wants less has their own timeout: the Lua helper
/// arms one, and a `zellij pipe` can be interrupted.
const COMPLETE_TICKS: u32 = 24;
/// How many completions one caller may have in flight.
///
/// Not a budget so much as a brake: a plugin in a loop should stall on its own answers
/// rather than fan out across the user's key.
const COMPLETIONS_PER_CALLER: usize = 2;
/// And how many across every caller at once.
const COMPLETIONS_TOTAL: usize = 8;

/// How much of the user's request is kept to give the summary its question.
///
/// Generous next to the row-level caps around it, because this never reaches a row: it
/// goes to the model beside the agent's answer, where a truncated question is worse than
/// a long one. Capped at all only because a prompt can carry a pasted file.
const PROMPT_CHARS: usize = 600;

/// The kind the tests report as; the event itself names its kind now.
#[cfg(test)]
const CLAUDE: &str = "claude";

/// The VM a Firecracker pane is in, and the host worktree that VM mirrors.
///
/// The two are kept together because neither answers on its own. The worktree is what
/// identifies the pane's work — its own folder is a guest path in a copy of that one — but a
/// pane that joined a VM by name carries no worktree of its own until the provider answers,
/// so an absent worktree cannot be read as "no longer in a VM". The VM is what tells those
/// two apart: the answer belongs to the VM it was given for, and a pane in another one has
/// left it behind. See `rebuild_panes`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct VmCheckout {
    vm: String,
    worktree: String,
}

#[derive(Default)]
struct State {
    /// Overrides from hocus's `agent_register`, refreshed by every subscribe.
    /// hocus stays authoritative; this is only an input to `classify`.
    manual_kinds: BTreeMap<u32, String>,
    /// The tab-to-project assignments the user made in hocus, held only to pass
    /// on. Nothing here reads them: the store names the tabs, and this is the
    /// only route from the pane that makes them to the plugin that uses them.
    tab_projects: BTreeMap<TabId, String>,
    /// The event-fed terminal-pane snapshot. `None` means nothing has told us
    /// about a pane yet.
    panes: Option<Vec<agents::AgentPane>>,
    /// Whether the startup scan has found the current session and run. Kept
    /// apart from `panes` because a manifest fills those without answering the
    /// one question the scan exists for — see `needs_bootstrap`.
    bootstrapped: bool,
    /// Where the rotating command re-ask left off. See `repair_commands_with`.
    command_cursor: usize,
    /// Panes seen running an agent, and which one. See `AgentPane::detected`.
    latched: BTreeMap<u32, String>,
    /// Latches a non-foreground report has proposed dropping, waiting on a
    /// direct ask to confirm. See `on_command`.
    pending_release: BTreeSet<u32>,
    manifest: PaneManifest,
    tabs: Vec<TabInfo>,
    /// The terminal pane the user is looking at. Passed on rather than acted on:
    /// hocus cannot read its own focus off a `PaneUpdate`.
    focused: Option<u32>,
    commands: BTreeMap<u32, String>,
    /// Where each pane is, as a plain path.
    ///
    /// Deliberately *not* keyed by machine: `CwdChanged` carries a pane id and a
    /// path and nothing else, and a pane can be heard about before anything has
    /// said which machine it is on. Baking a host in here would record a remote
    /// pane as local and then have to repair it. The machine comes from the pane
    /// itself — see `pane_location`.
    cwds: BTreeMap<u32, String>,
    /// Which machine each pane's process runs on, absent for this one.
    ///
    /// Kept beside `cwds` and pruned with it, rather than read off the pane
    /// list, because the pane list is the *visible* panes: a pane hidden behind
    /// the scrollback editor is suppressed, not closed, and it keeps its folder
    /// across that. A host that went away with it would key the folder as local
    /// for as long as the pane stayed hidden.
    ///
    /// The *raw* manifest is authoritative both ways, though: a pane that leaves its
    /// machine keeps its id, so this follows the manifest's answer down as well as up.
    /// See `rebuild_panes`.
    hosts: BTreeMap<u32, String>,
    /// The host worktree each Firecracker pane's VM mirrors, for the panes in one.
    ///
    /// A VM pane's folder is a guest path in a copy of this one, and the copy is a
    /// repository of its own that matches no project here — see
    /// `agents::checkout_location`, which is where the rule lives. Kept beside `hosts`
    /// and pruned with it for the reason `hosts` is: the pane list is the *visible*
    /// panes, and a suppressed pane keeps its folder. Retracted with it too, and for the
    /// same reason - a pane re-run on this machine is not in a VM any more, and this
    /// outranks its folder rather than losing to it.
    ///
    /// The VM is kept beside the worktree because it is what the retraction is keyed on;
    /// see `VmCheckout` and `rebuild_panes`.
    firecracker_worktrees: BTreeMap<u32, VmCheckout>,
    activity: agents::Activity,
    /// The checkout each agent pane's frame is currently decorated with.
    ///
    /// A record of what the server has been *told*, not of what is true, so a
    /// steady session makes no host calls at all: the wanted set is recomputed
    /// on every publish and only a difference is sent. See
    /// `refresh_title_prefixes_with`.
    title_prefixes: BTreeMap<u32, String>,
    /// The agent panes the session has been told are kin of the focused one.
    ///
    /// `None` until the first send, and that is the whole point of the `Option`: a
    /// freshly loaded daemon has no idea what the session still believes, and an empty
    /// `BTreeSet` would compare equal to the empty set it wants to assert and so never
    /// send it. `None` differs from everything, so a load - or a reload after a crash -
    /// always says something, and saying the empty set is exactly what clears a stale
    /// highlight.
    related_panes: Option<BTreeSet<u32>>,
    /// What Git said about each folder, keyed by folder *and machine*.
    repositories: BTreeMap<Location, projects::Repository>,
    /// Store instances awaiting a refreshed arrangement snapshot.
    arrangement_subscribers: Vec<PluginId>,
    arrangement_remote_pending: BTreeSet<u32>,
    arrangement_remote_failed: BTreeSet<u32>,
    arrangement_refresh_token: Option<String>,
    arrangement_started_at: Option<i64>,
    /// Local probes in flight. Only ever local: a remote folder is answered by
    /// the host in one call, with nothing to correlate — see
    /// `refresh_remote_repositories_with`.
    git_probes: BTreeMap<Location, GitProbe>,
    /// Seconds left before a folder whose probe answered nothing is tried again.
    probe_backoff: BTreeMap<Location, f64>,
    git_probe_seq: u64,
    git_refresh_elapsed: f64,
    /// Where the rotating remote re-ask left off. See
    /// `refresh_remote_repositories_with`.
    remote_probe_cursor: usize,
    /// Who to push snapshots to — hocus, and hocus-store for its project
    /// discovery. A list rather than a single slot because both want the same
    /// snapshot, and broadcasting it instead would hand every plugin in the
    /// session the title and working folder of every pane.
    subscribers: Vec<PluginId>,
    seq: u64,
    /// The last snapshot that actually went out, verbatim. Most host events end
    /// in `publish()` whether or not they moved anything a subscriber can see —
    /// a `PaneUpdate` and the `TabUpdate` behind it, the 5s heartbeat, every
    /// hook — and each snapshot costs every subscriber a parse and a merge. An
    /// identical snapshot is withheld instead; see `publish`.
    last_published: Option<String>,
    /// The session environment, for resolving the summary endpoint's API key.
    ///
    /// Read once and kept: the environment a plugin sees is the one captured when the
    /// *session* started, so it cannot change under us. The KDL half is deliberately *not*
    /// cached — see `summary_endpoint`.
    env: BTreeMap<String, String>,
    /// What the model last said about each agent's pane.
    summaries: agents::Summaries,
    /// Hook-backed runs currently attached to a pane. Completed runs live in the shared store;
    /// dirty completed rows ride the snapshot that announces their end.
    zellij_session_name: Option<SessionName>,
    /// The machine this daemon runs on, read once at load. The other half of a row's owner —
    /// see `agents::rows::Owner`.
    host_id: HostId,
    runs: BTreeMap<u32, agents::AgentRun>,
    pending_messages: BTreeMap<u32, messages::Pending>,
    dirty_runs: BTreeMap<String, agents::AgentRun>,
    /// Runs whose pane is gone from this session, so their write has to be guarded: a warped
    /// pane's run has been claimed by the session that took it, and our last word about the
    /// pane we lost must not overwrite theirs. See `agents::rows::run_owned_mutation`.
    ///
    /// Keyed like `dirty_runs` and drained with it. A run that is in both is guarded, because
    /// once its pane is gone nothing here can add to it.
    released_runs: BTreeSet<String>,
    /// Runs that have ended since the last drain and are owed the one summary the daemon buys
    /// of its own accord. Filled by `finish_run_as`, emptied by `drain_finished_runs`.
    ///
    /// A queue rather than a call at the close, because a run closes deep inside passes that
    /// hold `self` — `record_run_event`, `rebuild_panes`, `repair_commands_with` — and the ask
    /// reaches the host. Queuing puts every close on the same footing and leaves one place
    /// that talks to the network.
    finished_runs: Vec<String>,
    /// The newest revision of each run whose entry rows the store already holds.
    ///
    /// A run's timeline is stored one row per entry, and only the entries above this mark are
    /// written on the next persist — which is the point of the split: a run that has been going
    /// all day costs one small insert per hook rather than a rewrite of everything it has said.
    ///
    /// Derived from the run rather than recorded at each `entries.push`, so no future caller
    /// can add an entry and forget to write it.
    persisted_revision: BTreeMap<String, u64>,
    /// Panes we have already looked for a stored run for, so a pane that genuinely has none
    /// costs one fetch rather than one per manifest. Pruned with the panes.
    run_lookups: BTreeSet<u32>,
    /// What each Claude Code pane has said about *itself*, over its own hooks.
    ///
    /// The one thing in this daemon that is reported rather than inferred. Where a
    /// pane has an entry here it is the truth about that pane's state, and the
    /// screen-diff heuristic beside it is only still running because the summaries
    /// hang off it — see `agents::HookFeed`.
    hooks: agents::HookFeeds,
    /// The panes whose events this daemon hears directly, over the `agent_hook` broadcast.
    ///
    /// Their feeds are read from those events and never rebuilt from the pane manifest: the
    /// event stream carries what a record cannot - a `Notification` arriving six seconds
    /// after an `Elicitation` for the same prompt must not overwrite the more precise
    /// account of it - and a complete record has no way to express "keep what you had". The
    /// manifest road is for a pane that reports some other way: one hocus spawned, an
    /// `agent-report --state` script, a plugin speaking for an agent.
    event_fed: BTreeSet<u32>,
    /// What the user last asked each hook-fed pane for, from its `UserPromptSubmit`.
    ///
    /// Kept so a summary can say what the run was *for*.
    /// Only a pane that reports for itself has one: nothing else ever sees the prompt.
    prompts: BTreeMap<u32, String>,
    /// Panes with a summary request in flight. One at a time per pane, so a key held down
    /// cannot fan out into a flurry of requests.
    summary_requests: BTreeMap<u32, InFlight>,
    /// The same for archived runs, keyed by run id — the lane a `SessionEnd` and a manual
    /// ask about an archived row both take.
    ///
    /// Separate from `summary_requests` because the key is the whole difference: a run that
    /// has ended has no pane, and the pane it *had* may since have been given to another
    /// agent. Keying both by pane id would let a summary of last week's run land on whoever
    /// holds that number now.
    run_summary_requests: BTreeMap<String, InFlight>,
    summary_seq: u64,
    /// Completions in flight on another plugin's behalf, keyed by the id in their
    /// `WebRequestResult` context.
    ///
    /// Keyed by the daemon's own sequence rather than by the caller's `request_id`: two
    /// plugins picking the same id must not collide, and a caller's id is echoed rather
    /// than trusted.
    completions: BTreeMap<u64, Completion>,
    completion_seq: u64,
    /// How long one poll interval is, in milliseconds, with slack: how recently a pane must
    /// have written to its pty for this poll's picture to be explained by output. Taken from
    /// the timer's own reported elapsed time because plugin timers drift.
    poll_window_ms: u64,
}

/// A summary request that has gone out and not yet come back.
///
/// Every one of these is a request somebody pressed a key for — the daemon's own summary is
/// bought on `SessionEnd` and goes down the run lane. That is why there is no staleness rule
/// here: the two that used to exist, a screen hash and a session id, were both about
/// discarding an answer nobody had asked for, and discarding one somebody is waiting for
/// because the pane scrolled in the meantime would be the bug rather than the safeguard.
struct InFlight {
    id: u64,
    /// The pane hash the request was made from, so the answer is stamped with the screen it
    /// describes. Zero for a request made from a hook feed or a run's history, neither of
    /// which describes a screen.
    hash: u64,
    /// Heartbeats this has been outstanding. A `WebRequestResult` that never arrives -
    /// a dropped event, a host that gave up - would otherwise leave the pane looking
    /// permanently mid-request and never summarised again for the life of the session.
    ticks: u32,
}

/// A completion that has gone out on somebody else's behalf and not yet come back.
///
/// Where [`InFlight`] can simply be forgotten — the pane will go idle again and ask again —
/// this one is somebody's blocked call, and a `zellij pipe` caller is a blocked process. So
/// everything needed to *answer* it is held here, because the reply arrives on a
/// `WebRequestResult` that knows nothing about the pipe that started it.
struct Completion {
    /// Where the answer goes.
    reply_to: PipeSource,
    /// The caller's own id, echoed on the reply untouched.
    request_id: String,
    /// Heartbeats outstanding, for the reason [`InFlight::ticks`] exists — except that
    /// ageing out here *sends* a reply rather than merely forgetting.
    ticks: u32,
}

/// What a summariser should be told the user asked, from a prompt as it arrived.
///
/// [`envelope::question`] is a no-op for anything a person typed and reduces a harness's
/// envelope to the sentence in it; the cut is the same [`PROMPT_CHARS`] the pane lane keeps.
/// Both roads to a summary go through here, so the header and the body cannot disagree about
/// what the turn was for.
///
/// `None` where nothing is left — a prompt that was entirely machinery, which a
/// `<system-reminder>` on its own is: its question is `None` by design, and a turn with no
/// question behind it is summarised without one rather than against a stale one.
/// Put the machine and the worktree the pane is on *now* onto its run, answering whether
/// either moved.
///
/// A run carries its own copy of both because it outlives its pane: an archived run has
/// nothing left to ask which project it was work on, and `run_history::matches_root` reads
/// what is stored rather than what a pane could say. So the copy has to be kept level, and
/// by whichever of the two roads sees the move first — the hook that arrives after a warp,
/// and the manifest that names a worktree the run's first hook beat. Left to the hook alone,
/// a run whose agent never spoke again would be archived with the identity it was born with.
fn relocate_run(
    run: &mut agents::AgentRun,
    host: Option<String>,
    worktree: Option<String>,
) -> bool {
    let mut moved = false;
    if run.host != host {
        run.host = host;
        moved = true;
    }
    if run.worktree != worktree {
        run.worktree = worktree;
        moved = true;
    }
    moved
}

fn kept_prompt(text: &str) -> Option<String> {
    let question = envelope::question(text);
    let question = question.trim();
    (!question.is_empty()).then(|| question.chars().take(PROMPT_CHARS).collect())
}

/// What to ask a model about a run: the whole of it, prompts included.
///
/// The whole run rather than a delta since the last summary, because both remaining askers
/// want the same thing. A `SessionEnd` is describing a session that is over, and a person
/// pressing the key is looking at a row and asking what it has been doing — a question the
/// record answers better than an accumulated paraphrase does. `None` for a run with no
/// retained entry at all: every hook it saw was a tool call or a lifecycle event, and there
/// is nothing to describe.
fn run_delta(run: &agents::AgentRun) -> Option<String> {
    let delta = run
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                agents::RunEntryKind::Plan
                    | agents::RunEntryKind::Question
                    | agents::RunEntryKind::Result
                    | agents::RunEntryKind::Message
                    | agents::RunEntryKind::Prompt
            )
        })
        // A prompt goes through `envelope::question`, which is a no-op for anything a person
        // typed and reduces a harness's envelope to the sentence in it. The same function the
        // `prompts` map uses, so the summariser's two inputs cannot disagree about what the
        // turn was for.
        .map(|entry| {
            let text = match entry.kind {
                agents::RunEntryKind::Prompt => Cow::Owned(envelope::question(&entry.text)),
                _ => Cow::Borrowed(entry.text.as_str()),
            };
            format!("{}: {text}", entry.kind.label())
        })
        .collect::<Vec<_>>()
        .join("\n");
    if delta.is_empty() {
        return None;
    }
    // The run's own prior summary is left out on purpose: the entries carried here are what
    // it was made from, so including it would ask the model to weigh its own paraphrase
    // against the source it came from.
    Some(agents::tail(
        &delta,
        agents::SUMMARY_LINES,
        agents::SUMMARY_CHARS,
    ))
}

#[derive(Default)]
struct GitProbe {
    id: u64,
    /// Stdout and exit code of the `rev-parse`. Both are kept: a checkout with
    /// no commits writes its identity and then exits non-zero, so throwing the
    /// output away on failure would lose a real repository.
    metadata: Option<(Option<i32>, Vec<u8>)>,
    config: Option<Vec<u8>>,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[
            EventType::AgentEvent,
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::CommandChanged,
            EventType::CwdChanged,
            EventType::SessionUpdate,
            EventType::RunCommandResult,
            EventType::WebRequestResult,
            EventType::Timer,
        ]);
        // No `request_permission` here, deliberately. A builtin plugin — one
        // loaded from a `zellij:` location — is granted every gated call
        // unconditionally by `check_command_permission`, which is why almost no
        // other default plugin asks either.
        //
        // Asking anyway would not merely be redundant, it would strand this
        // plugin: the permission dialog is drawn *in the requesting plugin's
        // pane*, and a background plugin has none, so the request is cached
        // against a pane that never arrives and the answer never comes.

        // Safe to call here, unlike in hocus: a builtin is granted every gated call
        // unconditionally, so this value-returning call is actually answered rather than
        // dropped — and a dropped one would block on a read that never completes.
        self.env = get_session_environment_variables();
        // Which machine's rows these are. Constant for the life of the process, and needed
        // before any per-pane row is read or written.
        self.host_id = get_plugin_ids().host_id;

        let _ = store::Host::shared().define_all(&store::shared_defs());

        self.bootstrap();
        set_timeout(zellij_tile::prelude::FractionalSeconds::new(
            ACTIVITY_SECONDS,
        ));
        // A reload drops the in-memory subscriber list. Ask existing watchers
        // to subscribe again instead of silently stopping their snapshots.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_HELLO));
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PaneUpdate(manifest) => {
                self.manifest = manifest;
                self.rebuild_panes();
                self.publish();
            },
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                self.rebuild_panes();
                self.publish();
            },
            Event::CommandChanged(PaneId::Terminal(id), command, is_foreground, _) => {
                self.on_command(id.get(), command, is_foreground);
                self.publish();
            },
            Event::CwdChanged(PaneId::Terminal(id), cwd, _) => {
                self.record_cwd(id.get(), &cwd.to_string_lossy());
                self.rejoin_cwds();
                self.resolve_repositories();
                // A remote pane's folder is probed on the far machine, so a `cd`
                // is attributed on the next ask rather than by the pass above.
                // Asked here as well as on the heartbeat so it lands now rather
                // than in five seconds.
                self.refresh_remote_repositories();
                self.publish();
            },
            Event::AgentEvent(agent_event) => {
                // The session read the event once for the record and the typed fields; the
                // run's history wants the rest - `tool_input`, the permission mode, the
                // subagent - which `raw` still carries. A Claude Code that grows a field we
                // have never seen must cost us nothing, so a `raw` that does not parse is
                // dropped in silence.
                if let Ok(event) = serde_json::from_str::<wire::HookEvent>(&agent_event.raw) {
                    self.on_hook_of(agent_event.pane_id.get(), &agent_event.kind, &event);
                }
            },
            Event::SessionUpdate(sessions, _) => {
                if let Some(name) = sessions
                    .into_iter()
                    .find(|session| session.is_current_session)
                    .map(|session| session.name)
                {
                    self.set_zellij_session_name(name);
                }
            },
            Event::RunCommandResult(exit_code, stdout, _, context) => {
                self.finish_git_probe(exit_code, stdout, context);
            },
            Event::WebRequestResult(status, _, body, context) => {
                // Which key is present is what says which lane asked, exactly as
                // `finish_summary` does for its own two halves. Split here rather than in
                // there because a completion has a caller blocked on it and a summary has
                // nobody, so the two disagree about what a reply that fits neither costs.
                if context.contains_key(COMPLETE_ID) {
                    self.finish_completion(status, &body, &context);
                } else {
                    self.finish_summary(status, &body, &context);
                }
            },
            Event::Timer(elapsed) => {
                // A load that ran before the session was there left us with no
                // panes at all and nothing to make us look again.
                if self.needs_bootstrap() {
                    self.bootstrap();
                }
                // What "wrote to its pty since the last poll" means for this poll,
                // measured rather than assumed: a timer that fired late would otherwise
                // read a pane's real output as too old to explain what it shows.
                self.poll_window_ms = (elapsed.get().max(ACTIVITY_SECONDS) * 1500.0) as u64;
                self.observe_activity();
                // Not inside `observe_activity_with`, which prunes by pane and has nobody
                // to answer: every one of these is somebody's blocked call.
                self.age_completions_with(answer_completion);
                self.repair_commands();
                self.repair_cwds();
                self.retry_repositories((elapsed).get());
                // Every tick, not on the Git refresh interval: this reads a
                // cache the far machine keeps fresh on its own clock, so waiting
                // a minute would only add latency to an answer already sitting
                // there. It publishes nothing unless something changed.
                self.refresh_remote_repositories();
                self.git_refresh_elapsed += elapsed.get();
                if self.git_refresh_elapsed >= GIT_REFRESH_SECONDS {
                    self.git_refresh_elapsed %= GIT_REFRESH_SECONDS;
                    self.refresh_repositories();
                }
                self.publish();
                set_timeout(zellij_tile::prelude::FractionalSeconds::new(
                    ACTIVITY_SECONDS,
                ));
            },
            _ => {},
        }
        // The closes this pass found, if it found any: an agent the manifest says has exited, a
        // re-run in the same pane, a pane back at its shell prompt. Every one of them ends a run
        // that will never see a `SessionEnd`, and a run archived unsummarised is never asked
        // about again.
        self.drain_finished_runs(|state, run_id| {
            state.summarize_run_now(run_id);
        });
        self.persist_dirty_runs();
        // Never rendered, so never asks to be.
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::COMPLETE {
            self.on_complete(&pipe_message);
            return false;
        }
        if pipe_message.name == wire::AGENT_SUMMARIZE {
            // A person asked for this, so it skips the timing guards the automatic paths
            // keep. Nothing is answered back: the summary arrives the way every other change
            // to a run does, in the next snapshot, and a refusal is silent — see
            // `summarize_pane_now`, which has half a dozen reasons to make none.
            if let Some(request) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::SummarizeRequest>(json).ok())
            {
                match (request.pane_id, request.run_id.as_deref()) {
                    (Some(pane_id), _) => self.summarize_pane_now(pane_id),
                    (None, Some(run_id)) => self.summarize_run_now(run_id),
                    (None, None) => false,
                };
            }
            // `pipe` is not `update`, so the write at the end of the heartbeat does not run
            // for us. The local-fallback path can finish a summary synchronously and dirty a
            // run, and nothing else here would ever write it. Same reason `on_hook_of` calls
            // this.
            self.persist_dirty_runs();
            return false;
        }
        if pipe_message.name == "arrange_projects_refresh" {
            if let PipeSource::Plugin(id) = pipe_message.source {
                if !self.arrangement_subscribers.contains(&id) {
                    self.arrangement_subscribers.push(id);
                    self.arrangement_started_at.get_or_insert_with(now_ms);
                }
                if self.arrangement_refresh_token.is_none() {
                    self.arrangement_refresh_token =
                        Some(format!("{:?}", std::time::SystemTime::now()));
                    self.arrangement_remote_failed.clear();
                    self.arrangement_remote_pending
                        .extend(self.remote_pane_ids());
                }
                let locations = self
                    .pane_locations()
                    .into_iter()
                    .filter(|l| l.host.is_none() && !self.git_probes.contains_key(l))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                for location in &locations {
                    self.repositories.remove(location);
                }
                self.start_git_probes(locations);
                self.refresh_remote_repositories();
                self.publish_forced();
            }
            return false;
        }
        if pipe_message.name == wire::AGENT_REFRESH {
            // No reply of its own: each probe that finishes publishes, so the
            // asker gets the answer the same way it gets everything else.
            self.refresh_repositories();
            return false;
        }
        if pipe_message.name == wire::AGENT_SUBSCRIBE {
            // A CLI pipe has no plugin id to reply to; it can still refresh the
            // overrides and publish the cached event-fed snapshot.
            if let PipeSource::Plugin(id) = pipe_message.source {
                if !self.subscribers.contains(&id) {
                    self.subscribers.push(id);
                }
            }
            // Only when the subscriber actually sent overrides. hocus-store
            // subscribes with no payload, and treating that as "no manual
            // kinds" would throw away hocus's registrations.
            if let Some(request) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<PollRequest>(json).ok())
            {
                self.manual_kinds = request.manual_kinds;
                self.tab_projects = request.tab_projects;
                self.resolve_repositories();
            }
            // Answer immediately: a hocus that just opened wants the snapshot now,
            // and it being the one we already sent everyone else is no reason to
            // withhold it from someone who never got it.
            self.publish_forced();
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    /// True while the startup scan has still not found the current session.
    ///
    /// `gather` answers nothing when the session list does not name a current
    /// session, which a load early in session startup can see. That used to be
    /// the end of it — the one scan never ran again, and every pane that
    /// predated the daemon stayed without a folder, so without a project, for
    /// the life of the session.
    ///
    /// This asks whether the scan *succeeded*, not whether we hold any panes.
    /// Asking `self.panes.is_none()` meant a `PaneUpdate` — which arrives long
    /// before the first heartbeat — filled `panes` and cancelled the retry, and
    /// the manifest alone says only what a pane was *launched* with. So the
    /// panes that most need the scan, the ones that predate the daemon, were
    /// exactly the ones that never got it.
    fn needs_bootstrap(&self) -> bool {
        !self.bootstrapped
    }

    fn bootstrap(&mut self) {
        let Some(poll::StartupSnapshot {
            session_name,
            panes,
            focused,
        }) = poll::gather()
        else {
            return;
        };
        self.set_zellij_session_name(session_name.clone());
        if let Ok(rows) = store::Host::shared()
            .fetch(&agents::rows::active_runs_request(&self.owner()).limit(MAX_FETCH_ROWS))
        {
            let mut runs = agents::rows::runs_from_rows(&rows);
            // A run recorded before the store could be shared between machines names none, and
            // it is this one's. Stamped here rather than written: the claim lands with the run's
            // next ordinary persist.
            agents::rows::claim_unowned_runs(&mut runs, &self.owner());
            self.hydrate_runs(&mut runs);
            for run in runs {
                let (pane_id, run_id) = (run.pane_id, run.id.clone());
                // Kept, not replaced: a run this daemon is already holding is the live one,
                // and the stored copy is behind it.
                if self.runs.contains_key(&pane_id) {
                    continue;
                }
                self.runs.insert(pane_id, run);
                // Ordinarily a no-op: the record belongs to the server and outlived this
                // plugin, so it still names the run. It is not a no-op for a run that predates
                // the field, and a pane that cannot say which run it is in cannot be warped
                // without losing it.
                self.publish_run_id(pane_id, Some(run_id));
                self.run_lookups.insert(pane_id);
            }
        }
        self.focused = focused;
        self.bootstrapped = true;
        for pane in &panes {
            if let Some(command) = &pane.command {
                self.commands.insert(pane.pane_id, command.clone());
            }
            if let Some(cwd) = pane.cwd.clone() {
                self.record_cwd(pane.pane_id, &cwd);
            }
        }
        // Now that the retry really happens, this can run after a manifest has
        // already arrived — and the manifest, not this scan, is the live answer
        // to which panes exist. What the scan alone can see is the running
        // command and the folder, and those are already folded in above.
        if self.panes.is_some() {
            self.rebuild_panes();
            return;
        }
        self.panes = Some(panes);
        self.rejoin_cwds();
        self.settle_latches();
        self.resolve_repositories();
    }

    /// Who this daemon writes rows as: its machine, and the session it is in.
    ///
    /// The session name is empty until the first snapshot names it, which is why the callers
    /// that write per-pane rows check `is_known` first.
    fn owner(&self) -> agents::rows::Owner {
        agents::rows::Owner::new(
            self.host_id.clone(),
            self.zellij_session_name.clone().unwrap_or_default(),
        )
    }

    fn set_zellij_session_name(&mut self, name: SessionName) {
        if self.zellij_session_name.as_ref() == Some(&name) {
            return;
        }
        self.zellij_session_name = Some(name.clone());
        let host_id = self.host_id.clone();
        for run in self.runs.values_mut() {
            run.zellij_session_name = name.clone();
            run.host_id = host_id.clone();
            self.dirty_runs.insert(run.id.clone(), run.clone());
        }
        for run in self.dirty_runs.values_mut() {
            run.zellij_session_name = name.clone();
            run.host_id = host_id.clone();
        }
    }

    /// Record where a pane is, whatever named it — the manifest, a `CwdChanged`,
    /// the startup scan, or a direct ask.
    ///
    /// The one place a folder enters this plugin, and so the one place it is
    /// normalized. Every key downstream — the probed repositories, the project
    /// roots discovery makes out of them — is compared as a string, so one
    /// folder has to give one spelling here or it gives two everywhere.
    fn record_cwd(&mut self, pane_id: u32, cwd: &str) {
        let cwd = hocus_proto::paths::normalize(cwd).unwrap_or_else(|_| cwd.to_owned());
        self.cwds.insert(pane_id, cwd);
    }

    /// Put the folders we know onto the panes we hold.
    fn rejoin_cwds(&mut self) {
        let cwds = self.cwds.clone();
        for pane in self.panes.iter_mut().flatten() {
            // Only ever set, never cleared: a snapshot that does not mention a
            // folder has not said the pane left it.
            if let Some(cwd) = cwds.get(&pane.pane_id) {
                pane.cwd = Some(cwd.clone());
            }
        }
    }

    /// What the host says a pane is running now.
    ///
    /// The command is recorded whatever `is_foreground` says, and that is
    /// deliberate. A pane launched *as* a command — a layout `command="claude"` —
    /// is its own foreground process, so `get_foreground_cmds` skips it and the
    /// host reports its argv through the sysinfo fallback with the flag false.
    /// Gating on the flag would unclassify every agent started that way.
    ///
    /// What the flag decides is only this: with nothing holding the tty
    /// foreground, a command that names no agent means the agent this pane was
    /// running has finished. Even that is a proposal rather than a verdict — a
    /// `tcgetpgrp` that momentarily answers nothing looks exactly the same — so
    /// the heartbeat confirms it with a direct ask before the latch goes.
    fn on_command(&mut self, id: u32, command: Vec<String>, is_foreground: bool) {
        let command = (!command.is_empty()).then(|| command.join(" "));
        match &command {
            Some(command) => {
                self.commands.insert(id, command.clone());
            },
            None => {
                self.commands.remove(&id);
            },
        }
        let still_an_agent = command
            .as_deref()
            .and_then(agents::kind_from_command)
            .is_some();
        if !is_foreground && !still_an_agent && self.latched.contains_key(&id) {
            self.pending_release.insert(id);
        }
        self.update_pane(id, |pane| pane.command = command);
        self.settle_latches();
        self.resolve_repositories();
    }

    /// One agent hook, from the pane it fired in and the agent that sent it.
    ///
    /// Publishes whenever an event changes the feed, and that is the whole point of
    /// the feed: a state change reaches the strip in the milliseconds it took the pipe
    /// to arrive, rather than on a heartbeat up to five seconds away and then only
    /// after three more polls of stillness have agreed with it. Repeated events that
    /// say exactly the same thing cost no snapshot broadcast.
    ///
    /// Two things hang off this beyond the feed itself, and both used to be the screen
    /// heuristic's:
    ///
    /// - **The summary.** A pane whose agent reports for itself is the only pane the daemon
    ///   ever buys a summary for on its own, and it buys one per run: when the run closes, made
    ///   from the whole of its timeline rather than from a photograph of a terminal. A
    ///   `SessionEnd` is the ordinary way that happens, but not the only one — see
    ///   `finish_run_as`, which queues every close that knows the run is over, and
    ///   `drain_finished_runs`, which is where the asking happens.
    /// - **The waiting toast.** A `PermissionRequest` is the user being asked a question,
    ///   said as the question goes up. Drawing it in a sidebar nobody is looking at was
    ///   only ever half the answer.
    ///
    /// The screen heuristic keeps running, but only for the idle *clock*: `activity` is what
    /// `status_cell` counts minutes with, on both paths, so a hook-fed pane's `idle 3m` reads
    /// the same as everyone else's. What it no longer does for any pane is buy summaries —
    /// see `observe_activity_with`.
    fn on_hook_of(&mut self, pane_id: u32, kind: &str, event: &wire::HookEvent) {
        self.on_hook_with(
            pane_id,
            kind,
            event,
            |pane_id, text| {
                notify_with_pane_id(
                    text,
                    PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
                )
            },
            |pane_id, title| {
                rename_pane_with_id(
                    PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
                    title,
                )
            },
        );
        self.drain_finished_runs(|state, run_id| {
            state.summarize_run_now(run_id);
        });
        self.persist_dirty_runs();
    }

    /// The same for a `claude`, which is what all but one of the tests below are about.
    #[cfg(test)]
    fn on_hook(&mut self, pane_id: u32, event: &wire::HookEvent) {
        self.on_hook_of(pane_id, CLAUDE, event);
    }

    /// The body of [`on_hook_of`], with the two host-facing halves injected so the tests can
    /// watch them — the same shape `observe_activity_with` and `finish_summary_with`
    /// already take.
    ///
    /// The summary a `SessionEnd` is owed is not one of them: `record_run_event` closes the
    /// run, and closing it queues the ask. See `drain_finished_runs`, which every caller of
    /// this runs afterwards.
    fn on_hook_with(
        &mut self,
        pane_id: u32,
        kind: &str,
        event: &wire::HookEvent,
        toast: impl FnOnce(u32, &str),
        rename: impl FnOnce(u32, &str),
    ) {
        // The session carries the record, and `zellij action agent-report --hook-json` sets it
        // in the same breath as it broadcasts this event - so this write is a no-op beside it,
        // and the road for a hook script from before that, which only ever piped.
        match hocus_proto::agent::hook_outcome(kind, event) {
            hocus_proto::agent::HookOutcome::Ended => report_agent(pane_id, None),
            hocus_proto::agent::HookOutcome::Reported(agent) => report_agent(pane_id, Some(agent)),
            hocus_proto::agent::HookOutcome::Nothing => {},
        }
        self.record_run_event(pane_id, kind, event, now_ms());
        if event.hook_event_name == agents::HOOK_SESSION_END {
            // The session is over, so there is nothing left reporting for this pane
            // and the heuristic takes it back. The pane itself may well live on - a
            // shell prompt where an agent was - and that is the latch's business,
            // not ours. The question the last turn answered goes with it either way.
            self.prompts.remove(&pane_id);
            self.event_fed.remove(&pane_id);
            if self.hooks.remove(&pane_id).is_some() {
                self.publish();
            }
            // The summary the run is owed was queued by `record_run_event` above, which
            // closed it — this is the ordinary way a run ends, and the best moment there is
            // to describe one: it is closed and complete, so there is a whole session to
            // describe rather than a fragment of one. The close also put it in `dirty_runs`,
            // which is where `load_run` looks first, so the run lane can read it back without
            // a store round trip - and that lane wants no pane, which is right, because by
            // now this one is a shell prompt where an agent was.
            return;
        }
        let Some(phase) = agents::hook_phase(event) else {
            return;
        };
        let pane_is_agent = self
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
            .is_some_and(|pane| agents::classify(pane, &self.manual_kinds).is_some());
        if !pane_is_agent {
            return;
        }
        // A hook fired in this pane is that pane demonstrably running the agent that sent
        // it: better evidence than the foreground command, which stops naming the agent
        // the moment it shells out. Latched through the same map the command sniff
        // uses, so it is released by the same confirmed verdict and cannot become a
        // second thing to keep in step. The kind is the broadcast's own, not one read back
        // out of `classify` - which consults this very map.
        self.latched.insert(pane_id, kind.to_owned());
        self.pending_release.remove(&pane_id);
        let cwd_moved = match &event.cwd {
            Some(cwd) => {
                let known = self.cwds.get(&pane_id).cloned();
                self.record_cwd(pane_id, cwd);
                self.cwds.get(&pane_id) != known.as_ref()
            },
            None => false,
        };
        let previous = self.hooks.get(&pane_id).cloned();
        // A session restarted in the same pane is a different session: its predecessor's
        // summary describes work the user cannot get back to, and its baseline describes
        // a screen that is gone. Checked before the feed is replaced, because the feed is
        // the only thing that carries the old id — and swept before anything from the
        // current event is recorded, because the first event of the new session is
        // usually its `UserPromptSubmit`, and cleanup meant for the session that ended
        // must not carry off the question the new one just asked.
        let restarted = previous
            .as_ref()
            .is_some_and(|feed| !feed.session_id.is_empty() && feed.session_id != event.session_id);
        if restarted {
            self.summaries.remove(&pane_id);
            self.summary_requests.remove(&pane_id);
            self.prompts.remove(&pane_id);
        }
        // What the user asked for, kept for the summary made at the end of the turn.
        // Set on a `UserPromptSubmit` and cleared by nothing: the next prompt replaces
        // it, and a turn with no prompt behind it — a resumed session, a hook-driven
        // continuation — is summarised without one rather than against a stale question.
        if event.hook_event_name == "UserPromptSubmit" {
            match event
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                Some(prompt) => {
                    // What is kept is the *question*, not the prompt: an envelope's first six
                    // hundred characters are opening tags and a temp path, and a model asked
                    // what an agent is doing cannot answer from those. A prompt somebody typed
                    // passes through untouched, which is every prompt before this existed.
                    //
                    // A prompt that reduces to nothing — machinery addressed to the agent and
                    // no question at all — is the `None` arm's case rather than an empty
                    // string kept under this pane's id.
                    match kept_prompt(prompt) {
                        Some(kept) => {
                            self.prompts.insert(pane_id, kept);
                        },
                        None => {
                            self.prompts.remove(&pane_id);
                        },
                    }
                    // And the pane is named after it, now rather than at the end of the
                    // turn. Until this, a pane got a name only when `finish_summary_with`
                    // renamed it to the model's title — so the whole time an agent was
                    // working, the tab bar said `claude`. The question the turn is about is
                    // in hand the moment it is asked, and the summary title replaces it when
                    // there is something better to say. Same guard as that rename, for the
                    // same reason.
                    if let Some(title) = agents::prompt_title(prompt) {
                        if self.title_is_ours_to_take(pane_id) {
                            rename(pane_id, &title);
                        }
                    }
                },
                None => {
                    self.prompts.remove(&pane_id);
                },
            }
        }
        let previous_wait = previous
            .as_ref()
            .filter(|was| was.phase == agents::HookPhase::Waiting)
            .and_then(|was| was.waiting_on.clone());
        let feed = agents::HookFeed {
            phase,
            detail: agents::hook_detail(event),
            waiting_on: match phase {
                agents::HookPhase::Waiting => {
                    // An elicitation reminder keeps the original question's wording.
                    // Direct questions replace it.
                    let held = previous_wait.filter(|_| event.hook_event_name == "Notification");
                    held.or_else(|| agents::waiting_detail(event))
                },
                // Cleared on every phase that is not a wait, so a question the user has
                // since answered cannot go on being drawn beside a working agent.
                _ => None,
            },
            session_id: event.session_id.clone(),
            cwd: event.cwd.clone(),
        };
        // What this pane says for itself is read from its events, not from the record they
        // also write: `waiting_on` survives a `Notification` here and a complete record has no
        // way to express that. See the `event_fed` skip in `on_pane_update`.
        //
        // Set here rather than on the way in, because an event that built no feed - one whose
        // phase says nothing, or one for a pane the snapshot does not call an agent, or one
        // that arrived before the first snapshot did - has nothing to read *instead of* the
        // record. Claiming the pane for the pipe road on such an event closes the manifest
        // road behind a feed that was never built, and the record the same command wrote goes
        // unread until the next hook fires - which for a permission prompt is when the user
        // answers it.
        self.event_fed.insert(pane_id);
        let changed = previous.as_ref() != Some(&feed);
        if changed {
            self.hooks.insert(pane_id, feed.clone());
        }
        self.settle_latches();
        if cwd_moved {
            // The agent's folder, which is not always the pane's: an agent told to
            // work in a worktree moved without the shell under it hearing. This is
            // the only thing that ever says so.
            self.rejoin_cwds();
            self.resolve_repositories();
        }
        // Announce only actual transitions into waiting for user input.
        let entered_wait = phase == agents::HookPhase::Waiting
            && previous
                .as_ref()
                .is_none_or(|was| was.phase != agents::HookPhase::Waiting);
        // Not over the very pane it describes: the user is looking at the question. The same
        // rule the summary toast keeps — and the reason this is not part of `entered_wait`, which
        // is about the wait having been spoken for rather than about it being seen.
        if entered_wait && self.focused != Some(pane_id) {
            let kind = self.toast_kind(pane_id);
            toast(
                pane_id,
                &agents::waiting_toast(pane_id, kind.as_deref(), &feed),
            );
        }
        if changed {
            self.publish();
        }
    }

    fn record_run_event(&mut self, pane_id: u32, kind: &str, event: &wire::HookEvent, now_ms: i64) {
        let changed_session = self.runs.get(&pane_id).is_some_and(|run| {
            !run.session_id.is_empty()
                && !event.session_id.is_empty()
                && run.session_id != event.session_id
        });
        if changed_session {
            self.finish_run(pane_id, agents::RunOutcome::Replaced, now_ms);
        }
        let owner = self.owner();
        // A duplicate installed hook can replay SessionEnd after the first close was
        // persisted. Recover its entries before deduplicating the replay.
        if event.hook_event_name == agents::HOOK_SESSION_END
            && !self.runs.contains_key(&pane_id)
            && !event.session_id.is_empty()
        {
            if let Some(run) = self.load_run(&agents::session_run_id(kind, &event.session_id)) {
                if run.host_id != owner.host_id
                    || run.zellij_session_name != owner.zellij_session_name
                {
                    return;
                }
                self.runs.insert(pane_id, run);
            }
        }
        if event.hook_event_name != agents::HOOK_SESSION_END && !self.runs.contains_key(&pane_id) {
            // The manifest usually gets here first, but a hook can beat it: the pane arrives
            // and the agent speaks before the next `PaneUpdate`. Looking again costs one fetch
            // on the first event of a run and nothing after.
            self.adopt_stored_run(pane_id, kind, &event.session_id, &owner);
        }
        if event.hook_event_name == agents::HOOK_SESSION_END
            && !self.runs.contains_key(&pane_id)
            && event.assistant_messages.is_empty()
        {
            return;
        }
        // Read after the adoption above: a run it picked up is not a new one.
        let new_run = !self.runs.contains_key(&pane_id);
        // The machine the pane's process is on, read out here because the entry below borrows
        // `self.runs`. A hook reports the agent's folder and nothing about where that folder
        // is; the pane is the only thing that knows, and without it a run recorded on a build
        // box is indistinguishable from one recorded here.
        let host = self.hosts.get(&pane_id).cloned();
        // And, for a pane in a Firecracker VM, the host worktree that folder is a copy of.
        // The run outlives the pane, so it has to carry its own answer: an archived VM run
        // has nothing left to ask which project it was work on.
        let worktree = self
            .firecracker_worktrees
            .get(&pane_id)
            .map(|checkout| checkout.worktree.clone());
        let run = self
            .runs
            .entry(pane_id)
            .or_insert_with(|| agents::AgentRun {
                id: agents::run_id(kind, pane_id, &event.session_id, now_ms, &owner.host_id),
                pane_id,
                zellij_session_name: owner.zellij_session_name.clone(),
                host_id: owner.host_id.clone(),
                session_id: event.session_id.clone(),
                kind: kind.to_owned(),
                cwd: event.cwd.clone(),
                host: host.clone(),
                worktree: worktree.clone(),
                started_at_ms: now_ms,
                ended_at_ms: None,
                outcome: agents::RunOutcome::Active,
                revision: 0,
                summarized_revision: 0,
                summary: None,
                entries: Vec::new(),
            });
        let already_ended = run.outcome != agents::RunOutcome::Active;
        let mut changed = new_run;
        if !event.session_id.is_empty() && run.session_id.is_empty() {
            run.session_id = event.session_id.clone();
            changed = true;
        }
        if let Some(cwd) = event.cwd.as_ref().filter(|cwd| !cwd.is_empty()) {
            if run.cwd.as_ref() != Some(cwd) {
                run.cwd = Some(cwd.clone());
                changed = true;
            }
        }
        changed |= relocate_run(run, host, worktree);
        changed |= self
            .pending_messages
            .entry(pane_id)
            .or_default()
            .capture(run, event, now_ms);
        if let Some((kind, text)) = agents::run_entry(event) {
            // The text the entry will keep, not the event's own prompt: the id is stored beside
            // that text and compared against ids taken from other entries' text, and
            // `entry_event_id` is what takes them - including from an entry recorded before the
            // field existed, which has none of its own.
            let event_id = messages::stop_event_id(event, &text)
                .or_else(|| agents::run_event_id(event, &text));
            let duplicate = messages::stop_duplicates(run, event, &text)
                || match event_id.as_deref() {
                    Some(event_id) => run
                        .entries
                        .iter()
                        .any(|entry| agents::entry_event_id(entry).as_deref() == Some(event_id)),
                    None => run
                        .entries
                        .last()
                        .is_some_and(|entry| entry.kind == kind && entry.text == text),
                };
            if !duplicate {
                run.revision += 1;
                run.entries.push(agents::RunEntry {
                    revision: run.revision,
                    event_id,
                    kind,
                    text,
                    created_at_ms: Some(now_ms),
                });
                changed = true;
            }
        }
        if changed {
            let run = run.clone();
            self.dirty_runs.insert(run.id.clone(), run);
            if new_run {
                // Only on the first event of a run: the id does not change under it, and the
                // session drops a report that repeats what the pane already says.
                let run_id = self.runs[&pane_id].id.clone();
                self.publish_run_id(pane_id, Some(run_id));
            }
        }
        if event.hook_event_name == agents::HOOK_SESSION_END {
            if already_ended && !changed {
                self.runs.remove(&pane_id);
                self.pending_messages.remove(&pane_id);
            } else {
                self.finish_run(pane_id, agents::RunOutcome::Completed, now_ms);
            }
        }
    }

    /// Look for a stored run this pane's agent already has, from what the event itself says.
    ///
    /// The hook-side twin of `adopt_arrived_runs`. It has no record to read, so it builds the
    /// candidate id from the event: the same derivation, from the same session id the record
    /// would have carried.
    fn adopt_stored_run(
        &mut self,
        pane_id: u32,
        kind: &str,
        session_id: &str,
        owner: &agents::rows::Owner,
    ) {
        if session_id.trim().is_empty() || !owner.is_known() {
            return;
        }
        if !self.run_lookups.insert(pane_id) {
            return;
        }
        // The session-derived shape by construction — an empty session id returned above — so
        // the derivation is `session_run_id`, which is the one a warp can reproduce.
        let run_id = agents::session_run_id(kind, session_id);
        let Ok(rows) = store::Host::shared().fetch(&agents::rows::run_by_id_request(&run_id))
        else {
            return;
        };
        let mut runs = agents::rows::runs_from_rows(&rows);
        self.hydrate_runs(&mut runs);
        let Some(run) = runs.into_iter().next() else {
            return;
        };
        self.adopt_run(pane_id, run.adopted_by(pane_id, owner));
    }

    fn finish_run(&mut self, pane_id: u32, outcome: agents::RunOutcome, now_ms: i64) {
        self.finish_run_as(pane_id, outcome, now_ms, false);
    }

    /// Close a run whose pane has left this session for one we cannot see.
    ///
    /// The same close, written under a guard. A pane vanishing from the manifest is a warp and
    /// a `kill -9` alike from here — there is no signal that separates them — so this is
    /// written as the weaker claim of the two: *if nobody else has taken this run, it is over*.
    /// Where the pane really was warped, the session that adopted it has already reopened the
    /// row under its own name, the guard matches nothing, and the entries the agent has
    /// produced since the move survive.
    fn release_run(&mut self, pane_id: u32, now_ms: i64) {
        self.finish_run_as(pane_id, agents::RunOutcome::Closed, now_ms, true);
    }

    fn finish_run_as(
        &mut self,
        pane_id: u32,
        outcome: agents::RunOutcome,
        now_ms: i64,
        released: bool,
    ) {
        let Some(mut run) = self.runs.remove(&pane_id) else {
            return;
        };
        if let Some(pending) = self.pending_messages.remove(&pane_id) {
            // A pane leaving may already have another writer. Drop unfinished batches
            // rather than minting entry revisions that could race the new owner.
            if !released {
                pending.flush(&mut run, now_ms);
            }
        }
        run.outcome = outcome;
        run.ended_at_ms = Some(now_ms);
        if released {
            self.released_runs.insert(run.id.clone());
        } else {
            // Every close that *knows* the run is over is owed a summary: a `SessionEnd`, a
            // re-run that replaced it, an agent the manifest says has exited, a pane back at
            // its shell prompt. A released run is the one that does not know — a pane leaving
            // the manifest is a warp and a dead pane alike from here, which is why the write
            // above is guarded. Summarising it would write this session's stale copy of a run
            // another session now owns, unguarded, over theirs; and where it really was
            // warped, that session will summarise it at its own `SessionEnd`.
            self.finished_runs.push(run.id.clone());
        }
        self.dirty_runs.insert(run.id.clone(), run.clone());
        // The pane is no longer keeping this run, so it should no longer say it is. Harmless
        // where the pane has closed; load-bearing where it has not - a re-run in the same pane
        // starts a new run, and the old id left on the record would send the next session
        // looking for the wrong one.
        self.publish_run_id(pane_id, None);
    }

    /// Ask about every run that has ended since the last drain.
    ///
    /// `ask` is handed each id rather than called for its own sake so the tests can watch what
    /// would have gone out; in the daemon it is `summarize_run_now`, which reaches the host.
    ///
    /// Nothing here needs a budget. `summarize_run_now_with` refuses a run it already has a
    /// request in flight for, and `run_delta` gives back nothing for a run whose every hook was
    /// a tool call — so a session closing twenty panes at once asks about the twenty that have
    /// something to say and no more.
    fn drain_finished_runs(&mut self, mut ask: impl FnMut(&mut Self, &str)) {
        for run_id in std::mem::take(&mut self.finished_runs) {
            ask(self, &run_id);
        }
    }

    /// Tell the session which run this pane's agent is in, so a session that adopts the pane
    /// can find it. See `AgentInfo::run_id`.
    fn publish_run_id(&self, pane_id: u32, run_id: Option<String>) {
        set_pane_agent_run(
            PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
            run_id,
        );
    }

    /// Fill in the timelines of runs just read out of the store, and remember how much of
    /// each one the store already holds.
    ///
    /// The mark is set only for a run whose entries really came back as rows. A run recorded
    /// before the split still carries its timeline inside its payload, and leaving its mark at
    /// zero is what makes the next persist write those entries out as rows — which it must,
    /// because the payload it will be rewritten with no longer carries them.
    fn hydrate_runs(&mut self, runs: &mut [agents::AgentRun]) {
        let hydrated = agents::rows::hydrate_entries(&store::Host::shared(), runs);
        for run in runs.iter() {
            if !hydrated.contains(&run.id) {
                continue;
            }
            if let Some(newest) = run.entries.last().map(|entry| entry.revision) {
                self.persisted_revision.insert(run.id.clone(), newest);
            }
        }
    }

    /// One run read back out of the store, timeline and all.
    fn stored_run(&mut self, request: &FetchRequest) -> Option<agents::AgentRun> {
        let rows = store::Host::shared().fetch(request).ok()?;
        let mut runs = agents::rows::runs_from_rows(&rows);
        self.hydrate_runs(&mut runs);
        runs.into_iter().next()
    }

    /// The batch a persist writes, and what each run's mark becomes if it lands.
    ///
    /// The marks are handed back rather than moved here, because a refused batch writes
    /// nothing, and a mark advanced anyway would lose every entry it had claimed to store.
    fn dirty_run_mutations(
        &self,
        owner: &agents::rows::Owner,
    ) -> (Vec<Mutation>, Vec<(String, u64)>) {
        let mut mutations = Vec::with_capacity(self.dirty_runs.len());
        let mut written: Vec<(String, u64)> = Vec::new();
        for run in self.dirty_runs.values() {
            let released = self.released_runs.contains(&run.id);
            mutations.push(match released {
                true => agents::rows::run_owned_mutation(run, owner),
                false => agents::rows::run_mutation(run),
            });
            // The guard is on the run's *row*, and only there. Its payload is the thing two
            // sessions hold conflicting opinions about — a warp means the session that lost
            // the pane and the one that gained it each have a whole run to write, and the
            // loser's is stale. An entry is not that: it is addressed by `run_id#revision`,
            // it is only ever added, and a revision this session holds is a revision no other
            // session minted. So its entries go out unguarded, released or not.
            //
            // They must, too. A released run is exactly the case where entries are most
            // likely to be behind their mark — a run restored from a payload written before
            // the split has no rows at all and a mark of zero — and the guarded write
            // rewrites the payload without a timeline. Skipping the entries here left the run
            // with neither copy.
            let from = self.persisted_revision.get(&run.id).copied().unwrap_or(0);
            for entry in run.entries.iter().filter(|entry| entry.revision > from) {
                mutations.push(agents::rows::entry_mutation(&run.id, &run.host_id, entry));
            }
            if let Some(newest) = run.entries.last().map(|entry| entry.revision) {
                written.push((run.id.clone(), newest.max(from)));
            }
        }
        (mutations, written)
    }

    fn persist_dirty_runs(&mut self) {
        let owner = self.owner();
        if !owner.is_known() || self.dirty_runs.is_empty() {
            return;
        }
        let (mutations, written) = self.dirty_run_mutations(&owner);
        // One batch, so a run's row and the entries it names cannot land apart.
        if store::Host::shared().apply(mutations).is_ok() {
            self.persisted_revision.extend(written);
            // Only live runs can gain an entry, so only their marks are worth carrying. A
            // finished run's would sit here for the rest of the session saying something
            // nothing will ask.
            let live: BTreeSet<&String> = self.runs.values().map(|run| &run.id).collect();
            self.persisted_revision.retain(|id, _| live.contains(id));
            self.dirty_runs.clear();
            self.released_runs.clear();
        }
    }

    /// The meaningful hook events a live pane's run holds, as one request body.
    /// The prompt remains a separate request field, so trimming a long run keeps the newest
    /// steps and final result without losing what the user asked.
    ///
    /// `None` where the pane holds no run, or where the run holds no retained entry: asking
    /// a model to describe nothing would cost a round trip to be told so.
    ///
    /// [`agents::RunEntryKind::Summary`] is deliberately not among the kinds gathered here.
    /// A summary is the model's words about the agent; feeding one back as though the agent
    /// had said it would let a paraphrase compound into the next paraphrase.
    fn run_summary_text(&self, pane_id: u32) -> Option<String> {
        run_delta(self.runs.get(&pane_id)?)
    }

    /// Hang a finished summary on the pane's run, and on its timeline.
    ///
    /// Every summary that reaches a live pane is one somebody pressed a key for, and *when*
    /// they asked is half of what it says, so it stays where it was made. Nothing rolls
    /// forward here any more: the daemon's own summary is bought on `SessionEnd`, against a
    /// run this pane no longer holds.
    fn attach_run_summary(&mut self, pane_id: u32, summary: &agents::Summary) {
        let Some(run) = self.runs.get_mut(&pane_id) else {
            return;
        };
        agents::push_summary_entry(run, &summary.text, now_ms());
        run.summary = Some(summary.clone());
        // After the push, so the entry it just made is inside the accepted revision rather
        // than being read back as unsummarised work by the next request.
        run.summarized_revision = run.revision;
        self.dirty_runs.insert(run.id.clone(), run.clone());
    }

    /// Describe a hook-fed pane from what its run holds, and say whether a request went out.
    ///
    /// The whole of what pressing the summarise key against such a pane does. There is no
    /// new final message — nobody just stopped — so the delta is built from the run's own
    /// entries, and nothing makes the answer stale, because somebody is waiting for it.
    fn send_run_summary(
        &mut self,
        pane_id: u32,
        endpoint: Option<&Endpoint>,
        send: impl FnOnce(&str, HttpVerb, BTreeMap<String, String>, Vec<u8>, BTreeMap<String, String>),
        toast: impl FnOnce(u32, &str),
    ) -> bool {
        let Some(pane) = self
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
        else {
            return false;
        };
        let Some((kind, _)) = agents::classify(pane, &self.manual_kinds) else {
            return false;
        };
        let branch = self
            .pane_location(pane)
            .and_then(|location| self.repositories.get(&location))
            .and_then(|repository| repository.branch.clone());
        let cwd = pane.cwd.clone();
        let prompt = self.prompts.get(&pane_id).cloned();

        let Some(endpoint) = endpoint else {
            // No endpoint: no API key, or summaries switched off. Before the feed there
            // was nothing to say here — a screen tail is not a summary of anything. A
            // final message is prose already, so the honest fallback is its opening.
            //
            // Only prose the agent wrote can be fed to it: `local_summary` shortens a
            // message, and a list of retained events is not one. So it reaches back for the
            // newest message the run kept — the agent's own last words, read out of the
            // record. Nothing else here would be a summary rather than an invention.
            let latest = self
                .runs
                .get(&pane_id)
                .and_then(|run| {
                    run.entries.iter().rev().find(|entry| {
                        matches!(
                            entry.kind,
                            agents::RunEntryKind::Result | agents::RunEntryKind::Message
                        )
                    })
                })
                .map(|entry| entry.text.clone());
            let Some(message) = latest else {
                return false;
            };
            self.finish_local_summary_with(pane_id, &message, toast);
            return true;
        };
        // Somebody asked, and what they asked is "what is this agent doing" — a question
        // about the run, not about its last few minutes, which is why `run_delta` reads
        // every entry the run retained rather than only what is new since its last summary.
        let Some(message) = self.run_summary_text(pane_id) else {
            // No retained entry at all: every hook this run saw was a tool call or a
            // lifecycle event, and there is nothing to describe.
            return false;
        };
        let body = agents::run_summary_request_body(
            &endpoint.model,
            kind,
            cwd.as_deref(),
            branch.as_deref(),
            prompt.as_deref(),
            &message,
        );
        self.summary_seq += 1;
        let id = self.summary_seq;
        self.summary_requests.insert(
            pane_id,
            InFlight {
                id,
                hash: 0,
                ticks: 0,
            },
        );
        send(
            &endpoint.url,
            HttpVerb::Post,
            endpoint.headers(),
            body,
            BTreeMap::from([
                (SUMMARY_PANE.to_owned(), pane_id.to_string()),
                (SUMMARY_ID.to_owned(), id.to_string()),
            ]),
        );
        true
    }

    /// Store and announce a summary made from the agent's own words, with no model asked.
    ///
    /// The same outputs `finish_summary_with` produces, minus the two it cannot: there is
    /// no title, so the pane keeps its name, and there is no reply to have been made stale
    /// in flight because nothing was in flight. Reached by pressing the key against a
    /// hook-fed pane in a session with no API key; the answer is no less theirs for having
    /// been made locally.
    fn finish_local_summary_with(
        &mut self,
        pane_id: u32,
        message: &str,
        toast: impl FnOnce(u32, &str),
    ) {
        let hash = agents::activity_hash(&self.activity, pane_id).unwrap_or_default();
        let Some(mut summary) = agents::local_summary(message, hash) else {
            return;
        };
        // Nothing should be in flight - `summarize_pane_now_with` refuses while one is - but
        // clearing it costs nothing and keeps a stray reply from overwriting the agent's own
        // words with a guess and toasting the same thing twice.
        self.summary_requests.remove(&pane_id);
        // The one thing a summary must never disagree with the row about, on either
        // path. See `finish_summary_with`, which does the same to the model's answer.
        if let Some(feed) = self.hooks.get(&pane_id) {
            summary.awaiting = feed.phase == agents::HookPhase::Waiting;
        }
        // Stamped here rather than in the parser: this is where a summary becomes one the
        // daemon keeps, and it is the side with a clock. The panel draws the age from it.
        // Below the `awaiting` fixup, and against what this pane already holds, so a
        // re-summary that changed no word keeps the age of the answer it repeats.
        agents::stamp_summary(&mut summary, self.summaries.get(&pane_id), now_ms());
        if self.focused != Some(pane_id) {
            let kind = self.toast_kind(pane_id);
            toast(
                pane_id,
                &agents::summary_toast(pane_id, kind.as_deref(), &summary),
            );
        }
        self.attach_run_summary(pane_id, &summary);
        self.summaries.insert(pane_id, summary);
        self.publish();
    }

    /// Note every pane that is running an agent right now, and write what we
    /// hold onto the panes.
    ///
    /// Latching on the *command* alone is what keeps the no-false-positive
    /// guarantee: a pane can only be remembered as a kind it demonstrably ran. A
    /// title that names an agent stays exactly as transient as it is today.
    fn settle_latches(&mut self) {
        let seen: Vec<(u32, String)> = self
            .panes
            .iter()
            .flatten()
            .filter_map(|pane| {
                let kind = agents::kind_from_command(pane.command.as_deref()?)?;
                Some((pane.pane_id, kind.to_owned()))
            })
            .collect();
        for (pane_id, kind) in seen {
            self.latched.insert(pane_id, kind);
            self.pending_release.remove(&pane_id);
        }
        let latched = self.latched.clone();
        for pane in self.panes.iter_mut().flatten() {
            pane.detected = latched.get(&pane.pane_id).cloned();
        }
    }

    /// Put the commands we know onto the panes we hold, the way `rejoin_cwds`
    /// does for folders.
    fn rejoin_commands(&mut self) {
        let commands = self.commands.clone();
        for pane in self.panes.iter_mut().flatten() {
            // Only ever set, never cleared — a pane we were told nothing about
            // keeps whatever the manifest said it was launched with.
            if let Some(command) = commands.get(&pane.pane_id) {
                pane.command = Some(command.clone());
            }
        }
    }

    fn rebuild_panes(&mut self) {
        self.focused = focused_pane(&self.manifest, &self.tabs);
        // Read before `self.panes` is replaced below — it is what tells a re-run
        // pane from one that was running all along.
        let was_running: BTreeSet<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| pane.status == agents::AgentStatus::Running)
            .map(|pane| pane.pane_id)
            .collect();
        // The panes we were showing a moment ago, whatever their status. A pane
        // missing from this was not one whose process we were watching, so
        // nothing about it can have been re-run — see the drop below.
        let was_shown: BTreeSet<u32> = self
            .panes
            .iter()
            .flatten()
            .map(|pane| pane.pane_id)
            .collect();
        // From the raw manifest for the same reason the maps above are pruned
        // against it: a suppressed pane is still a pane, still on its machine.
        //
        // Both directions, because a pane can leave its machine without leaving the
        // manifest: `RunCommandInPane` re-runs a pane in place, keeping its id, and a VM
        // pane re-run here comes back `Local` — or in another VM, which is a different
        // machine and just as much a departure. Insert-only, the pane would keep answering
        // with the checkout it left - `checkout_location` reads the worktree *before* the
        // folder, so the stale half is the one that wins.
        for pane in self.manifest.panes.values().flatten() {
            // Plugin ids and terminal ids are separate number spaces, so a plugin pane
            // clearing by `pane.id` would retract a terminal pane's answer. The same
            // guard `manifest_pane_ids` applies, and a no-op for the inserts either way.
            let PaneId::Terminal(terminal_id) = pane.id else {
                continue;
            };
            let pane_id = terminal_id.get();
            match pane.host.clone() {
                Some(host) => {
                    self.hosts.insert(pane_id, host);
                },
                None => {
                    self.hosts.remove(&pane_id);
                },
            }
            // The host path a Firecracker pane's VM mirrors, which is what identifies
            // its work. Read from the target rather than from the pane's folder: the
            // folder is a guest path.
            match (
                pane.target.firecracker_vm(),
                pane.target.firecracker_worktree(),
            ) {
                (Some(vm), Some(worktree)) => {
                    self.firecracker_worktrees.insert(
                        pane_id,
                        VmCheckout {
                            vm: vm.to_owned(),
                            worktree: worktree.display().to_string(),
                        },
                    );
                },
                // Not retracted on the worktree being absent: a pane that joined a VM by
                // name is `Firecracker` with no worktree of its own until the provider
                // answers, and clearing in that window would drop the answer the server is
                // about to send. It is the *VM* the answer was given for, so a pane in a
                // different one has left it behind as surely as one re-run on this machine.
                (Some(vm), None)
                    if self
                        .firecracker_worktrees
                        .get(&pane_id)
                        .is_some_and(|checkout| checkout.vm == vm) => {},
                _ => {
                    self.firecracker_worktrees.remove(&pane_id);
                },
            }
        }
        let mut panes = terminal_panes(&self.manifest, &self.tabs);
        for pane in &mut panes {
            if let Some(command) = self.commands.get(&pane.pane_id) {
                pane.command = Some(command.clone());
            }
        }
        // Pruned against every pane the manifest carries, not against the ones
        // worth showing. A suppressed pane is hidden behind something — the
        // scrollback editor, most often — and comes back with the same id still
        // running what it was running, so reading its absence from the visible
        // list as "closed" cost it its folder, the branch probed from that
        // folder, and the latch that made it an agent at all.
        let live = manifest_pane_ids(&self.manifest);
        let closed_runs: Vec<u32> = self
            .runs
            .keys()
            .filter(|id| !live.contains(id))
            .copied()
            .collect();
        for pane_id in closed_runs {
            // Released rather than closed outright: a pane leaving the manifest is a warp and a
            // dead process alike from here, and only one of those ends the run. See
            // `release_run`.
            self.release_run(pane_id, now_ms());
        }
        self.commands.retain(|id, _| live.contains(id));
        self.cwds.retain(|id, _| live.contains(id));
        self.hosts.retain(|id, _| live.contains(id));
        self.firecracker_worktrees.retain(|id, _| live.contains(id));
        // The runs' own copies of the two maps above, kept level here as well as on the next
        // hook. Every run left is one whose pane is live — the closed ones were released just
        // above — so the maps are the answer for all of them. Doing it only on a hook leaves a
        // run whose agent has fallen silent archived with the identity it was born with, and
        // an archived run has no pane left for `matches_root` to fall back on. See
        // `relocate_run`.
        for (pane_id, run) in self.runs.iter_mut() {
            let moved = relocate_run(
                run,
                self.hosts.get(pane_id).cloned(),
                self.firecracker_worktrees
                    .get(pane_id)
                    .map(|checkout| checkout.worktree.clone()),
            );
            if moved {
                self.dirty_runs.insert(run.id.clone(), run.clone());
            }
        }
        // A pane that closes stops being an agent because it stops being in the
        // manifest — the same rule as the two maps above, which is what keeps
        // the latch from being one more thing that can go stale.
        self.latched.retain(|id, _| live.contains(id));
        self.pending_release.retain(|id| live.contains(id));
        // Pruned with the latch rather than on a clock of its own. Nothing about a
        // feed expires with time - a permission prompt can sit unanswered all day,
        // and a ten-minute build is one `PreToolUse` and a long silence - so the only
        // honest ends are the ones that are observed: the session saying so, the pane
        // closing, and the latch being released. This is the second.
        self.hooks.retain(|id, _| live.contains(id));
        // The two maps that only make sense beside a feed, pruned with it rather than each on
        // a rule of its own: a pane that closed while its agent was still running would
        // otherwise leave its id in both for the life of the daemon, and leave `event_fed`
        // disagreeing with `hooks` about which road the pane is read from.
        self.event_fed.retain(|id| live.contains(id));
        self.run_lookups.retain(|id| live.contains(id));
        // A pane that has come back to life is running a new process, and has to
        // earn its latch again rather than inherit the dead one's. `settle_latches`
        // below re-earns it in the same breath if it really is running an agent.
        //
        // Only for a pane we were already showing: a re-run happens in place, so
        // the pane never left the list. One arriving from somewhere else — a pane
        // coming back from being suppressed — has been running all along, and
        // reading it as a re-run would take the latch that survived the
        // suppression in the first place.
        let mut forgotten: Vec<u32> = Vec::new();
        for pane in &panes {
            if pane.status == agents::AgentStatus::Running
                && was_shown.contains(&pane.pane_id)
                && !was_running.contains(&pane.pane_id)
            {
                self.latched.remove(&pane.pane_id);
                self.pending_release.remove(&pane.pane_id);
                forgotten.push(pane.pane_id);
            }
        }
        // A re-run starts a new process, which has nothing to do with what the old one was
        // summarised as doing — keeping any of it would show a stale answer the moment the
        // new run pauses once. See `waiting_marker`. Out of the loop above only because it
        // borrows `self` whole.
        for pane_id in forgotten {
            self.finish_run(pane_id, agents::RunOutcome::Replaced, now_ms());
            self.forget_agent(pane_id);
        }
        // Anything the manifest itself carries is news, and worth taking before
        // the rejoin below writes what we already knew back over it.
        let reported: Vec<(u32, String)> = panes
            .iter()
            .filter_map(|pane| Some((pane.pane_id, pane.cwd.clone()?)))
            .collect();
        self.panes = Some(panes);
        let exited_runs: Vec<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| {
                pane.agent
                    .as_ref()
                    .is_some_and(|agent| agent.state == AgentState::Exited)
            })
            .map(|pane| pane.pane_id)
            .collect();
        for pane_id in exited_runs {
            self.finish_run(pane_id, agents::RunOutcome::Closed, now_ms());
        }
        // The manifest's agent records are the feeds: what the agent reported, however the
        // report travelled. A record from the spawn path is not a feed - the agent has said
        // nothing, so the heuristic judges it - and an exited agent's feed is over.
        for pane in self.panes.iter().flatten() {
            let Some(agent) = pane.agent.as_ref() else {
                continue;
            };
            // An exited agent's feed is over however its record travelled, so this is read
            // even for a pane whose events we hear. It is the only end a feed has left when
            // the process dies without a `SessionEnd` - a `kill -9`, a machine that went away
            // - and skipping it there leaves the pane on `waiting · …` for good, with the
            // screen heuristic disabled behind the feed that will not go.
            if agent.state == AgentState::Exited {
                self.hooks.remove(&pane.pane_id);
                self.event_fed.remove(&pane.pane_id);
                continue;
            }
            // A pane whose events we hear is read from them; see `event_fed`.
            if self.event_fed.contains(&pane.pane_id) {
                continue;
            }
            if let Some(feed) = agents::HookFeed::from_agent(agent) {
                self.hooks.insert(pane.pane_id, feed);
            }
        }
        self.adopt_arrived_runs();
        for (pane_id, cwd) in reported {
            self.record_cwd(pane_id, &cwd);
        }
        self.rejoin_cwds();
        self.settle_latches();
        self.resolve_repositories();
    }

    /// Pick up the run of any agent pane that arrived carrying one.
    ///
    /// The other half of a warp. `AgentInfo` travels with the pane, so the record is right the
    /// moment it lands — but the run behind it is a row in a store shared by every session that
    /// reaches it, under an id that means nothing to a pane id or a session name. This is where
    /// that id is looked up.
    ///
    /// Driven off the manifest rather than off the next hook, because an agent that arrives
    /// mid-think may say nothing for minutes, and until it does the run would sit in the
    /// archive marked closed while the pane it belongs to is plainly working.
    fn adopt_arrived_runs(&mut self) {
        let owner = self.owner();
        if !owner.is_known() {
            return;
        }
        let candidates: Vec<(u32, String)> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| !self.runs.contains_key(&pane.pane_id))
            .filter(|pane| !self.run_lookups.contains(&pane.pane_id))
            .filter_map(|pane| Some((pane.pane_id, agents::stored_run_id(pane.agent.as_ref()?)?)))
            .collect();
        for (pane_id, run_id) in candidates {
            // Asked once per pane. A pane that has no stored run is the common case by far, and
            // the answer does not change until the pane does.
            self.run_lookups.insert(pane_id);
            let Some(run) = self.stored_run(&agents::rows::run_by_id_request(&run_id)) else {
                continue;
            };
            self.adopt_run(pane_id, run.adopted_by(pane_id, &owner));
        }
    }

    /// Take a stored run as this pane's live one.
    ///
    /// Split from the fetch above so the tests can reach it: this plugin's tests run natively,
    /// where `store::Host` is a stub that errors on every call, so nothing that reads the store
    /// can be exercised there.
    fn adopt_run(&mut self, pane_id: u32, run: agents::AgentRun) {
        // The row is claimed with an unguarded write - see `run_owned_mutation` for why this
        // one may not be guarded.
        self.released_runs.remove(&run.id);
        self.dirty_runs.insert(run.id.clone(), run.clone());
        self.publish_run_id(pane_id, Some(run.id.clone()));
        // The summary is part of the run and travels inside it. The `agent_summary` row does
        // not: it is keyed by the pane, and this pane is a different one.
        if let Some(summary) = run.summary.clone() {
            self.summaries.insert(pane_id, summary);
        }
        self.runs.insert(pane_id, run);
    }

    /// Ask the host directly for the folder of any agent pane nothing has named.
    ///
    /// One blocking round-trip per pane, so this is capped and runs on the
    /// heartbeat rather than on the manifest, which arrives far too often to
    /// pay for. Normally there is nothing to do: this is the path that stops a
    /// pane whose folder never arrived from being without one for good.
    fn repair_cwds(&mut self) {
        self.repair_cwds_with(poll::pane_cwd);
    }

    /// Ask the host directly what some panes are running.
    ///
    /// `CommandChanged` is the only other source, and the ticker behind it looks
    /// only at panes that produced output since its last pass
    /// (`pane_activity_flags` in `Pty::update_and_report_cwds`). An agent sitting
    /// silent at its prompt produces none, so a command that never arrived never
    /// arrives, and one that went stale stays stale — for the life of the pane.
    /// This is the only thing that closes that gap.
    fn repair_commands(&mut self) {
        self.repair_commands_with(poll::pane_running_command);
    }

    fn repair_commands_with(&mut self, mut command_of: impl FnMut(u32) -> Option<String>) {
        let running: Vec<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| pane.status == agents::AgentStatus::Running)
            .map(|pane| pane.pane_id)
            .collect();
        if running.is_empty() {
            return;
        }
        // Panes waiting on a release decision go first — an agent that has really
        // finished should leave the list promptly. Then panes nothing has named a
        // command for, which cannot be classified at all until something does.
        // Whatever budget is left goes on a rotation over the rest, so every pane
        // is revisited eventually without paying for all of them every tick.
        let mut ask: Vec<u32> = Vec::with_capacity(COMMAND_REPAIRS_PER_TICK);
        let claim = |ask: &mut Vec<u32>, pane_id: u32| {
            if ask.len() < COMMAND_REPAIRS_PER_TICK && !ask.contains(&pane_id) {
                ask.push(pane_id);
            }
        };
        for pane_id in running.iter().copied() {
            if self.pending_release.contains(&pane_id) {
                claim(&mut ask, pane_id);
            }
        }
        for pane_id in running.iter().copied() {
            if !self.commands.contains_key(&pane_id) {
                claim(&mut ask, pane_id);
            }
        }
        for offset in 0..running.len() {
            claim(
                &mut ask,
                running[(self.command_cursor + offset) % running.len()],
            );
        }
        self.command_cursor = self.command_cursor.wrapping_add(ask.len()) % running.len();

        let mut found_something_new = false;
        for pane_id in ask {
            // Only ever set, never cleared: a host call that answered nothing has
            // not said the pane stopped running what it was running. Same reason
            // `rejoin_cwds` never clears a folder — and the same reason a silent
            // answer must not settle a pending release either.
            let Some(command) = command_of(pane_id) else {
                continue;
            };
            // The direct ask is the verdict `on_command` deferred to. It reads
            // the foreground process and falls back to the pane's own, so an
            // agent that is merely between tool runs still names itself here.
            if self.pending_release.remove(&pane_id)
                && agents::kind_from_command(&command).is_none()
            {
                self.latched.remove(&pane_id);
                // The verdict that the agent is gone, not merely quiet. What we last knew
                // about it describes a session that has ended, so it goes now rather than on
                // the next heartbeat — and a settling this pane still owed goes with it.
                self.finish_run(pane_id, agents::RunOutcome::Closed, now_ms());
                self.forget_agent(pane_id);
                found_something_new = true;
            }
            if self.commands.get(&pane_id) != Some(&command) {
                self.commands.insert(pane_id, command);
                found_something_new = true;
            }
        }
        if found_something_new {
            self.rejoin_commands();
            self.settle_latches();
            self.resolve_repositories();
        }
    }

    fn repair_cwds_with(&mut self, mut cwd_of: impl FnMut(u32) -> Option<String>) {
        let unnamed: Vec<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| pane.cwd.is_none())
            .filter(|pane| pane.status == agents::AgentStatus::Running)
            .map(|pane| pane.pane_id)
            .take(CWD_REPAIRS_PER_TICK)
            .collect();
        if unnamed.is_empty() {
            return;
        }
        let mut found = false;
        for pane_id in unnamed {
            if let Some(cwd) = cwd_of(pane_id) {
                self.record_cwd(pane_id, &cwd);
                found = true;
            }
        }
        if found {
            self.rejoin_cwds();
            self.resolve_repositories();
        }
    }

    fn retry_repositories(&mut self, elapsed: f64) {
        self.expire_probe_backoff(elapsed);
        self.resolve_repositories();
    }

    /// Count down the folders we are leaving alone, and let the expired ones go.
    fn expire_probe_backoff(&mut self, elapsed: f64) {
        self.probe_backoff.retain(|_, left| {
            *left -= elapsed;
            *left > 0.0
        });
    }

    fn update_pane(&mut self, id: u32, update: impl FnOnce(&mut agents::AgentPane)) {
        if let Some(pane) = self
            .panes
            .as_mut()
            .and_then(|panes| panes.iter_mut().find(|pane| pane.pane_id == id))
        {
            update(pane);
        }
    }

    /// Probe every agent folder we have not probed yet, and forget the ones no
    /// agent is in any more.
    fn resolve_repositories(&mut self) {
        // Retained on the folders live panes are in, not the folders *agents*
        // are in. What a folder is — its root, its remote, its common directory
        // — cannot change underneath us, but `classify` reads the foreground
        // command and genuinely flickers. Keying eviction on it threw away a
        // probed checkout on every flicker and cancelled the probe in flight
        // with it, so the agent lost its project until two more round trips
        // landed. One cwd per live pane still bounds both maps.
        //
        // A folder is only half a key now, so the live set is `cwds` joined to
        // `hosts` rather than `cwds` alone — building local keys for remote
        // panes would leave every remote entry matching nothing and so never
        // evicted. Both maps are pruned against the *manifest*, which is what
        // keeps a suppressed pane's folder alive while it is hidden.
        let live: BTreeSet<Location> = self.pane_locations().into_iter().collect();
        self.repositories
            .retain(|location, _| live.contains(location));
        self.git_probes
            .retain(|location, _| live.contains(location));
        self.probe_backoff
            .retain(|location, _| live.contains(location));
        self.start_git_probes(self.repository_cwds(false));
    }

    /// Probe every agent folder again, whatever we already know about it.
    ///
    /// The identity a probe finds — root, remote, common directory — cannot
    /// change under a folder, but the branch can, and no host event says when.
    /// The cached answer stays in place until the new one lands, so nothing
    /// blinks while this runs.
    fn refresh_repositories(&mut self) {
        self.start_git_probes(self.repository_cwds(true));
        // Both halves of the same job. This is what `wire::AGENT_REFRESH` calls,
        // and a refresh that quietly did nothing for remote panes would be a
        // trap for whoever asked.
        self.refresh_remote_repositories();
    }

    /// The folders worth asking Git about on this pass.
    ///
    /// Every live pane's folder, not only the folders agents are in — the same
    /// set `resolve_repositories` keys eviction on, so the two stop disagreeing
    /// about what is worth keeping. The asymmetry was a real gap: a pane that
    /// has not latched yet is in `cwds` but not in `agent_cwds`, so its folder
    /// went unprobed, and hocus answered for it with the nearest folder *above*
    /// it that had been probed. For a worktree nested inside its own checkout
    /// that neighbour is the primary, and its branch is not this pane's.
    ///
    /// Agents first all the same: theirs are the folders a header is drawn from,
    /// and the cap below is spent on them before anything else.
    ///
    /// Local folders only. A remote pane's folder is on another machine, where
    /// running Git from here would probe an unrelated directory of the same
    /// name — see `refresh_remote_repositories_with` for the other half.
    fn repository_cwds(&self, refresh: bool) -> Vec<Location> {
        let agents = self.agent_locations();
        let mut seen: BTreeSet<Location> = BTreeSet::new();
        agents
            .iter()
            .cloned()
            .chain(self.pane_locations())
            .filter(|location| location.host.is_none())
            // One folder can hold several panes, and an agent folder is in both
            // halves of the chain.
            .filter(|location| seen.insert(location.clone()))
            .filter(|location| refresh || !self.repositories.contains_key(location))
            // A probe already in flight answers for this folder either way, so
            // even a forced refresh must not start a second one.
            .filter(|location| !self.git_probes.contains_key(location))
            // Nothing is cached for a probe that answered nothing, so without
            // this the next event would run it again immediately.
            .filter(|location| !self.probe_backoff.contains_key(location))
            // Two processes per folder, and this runs on nearly every host
            // event. What is left over is picked up by the next event or the
            // heartbeat, and every filter above makes the pass idempotent.
            .take(GIT_PROBES_PER_PASS)
            .collect()
    }

    /// The folder that identifies a pane's work, with the machine it is on.
    ///
    /// The pane is the only thing that knows every half: `cwds` holds the path,
    /// the manifest holds the host, and `firecracker_worktrees` holds the host
    /// worktree a VM pane's guest folder is a copy of. A pane nothing has said
    /// any of that about has no location at all, which is what keeps it from
    /// being probed under a half-known one.
    fn pane_location(&self, pane: &agents::AgentPane) -> Option<Location> {
        self.location_of(pane.pane_id)
            .or_else(|| pane.checkout_location())
    }

    /// Where a pane is, by id alone.
    ///
    /// Every half comes out of the maps rather than off the pane list, so a pane
    /// the list has dropped — suppressed behind the scrollback editor — still
    /// answers, and the folder probed for it is not thrown away while it is
    /// hidden.
    fn location_of(&self, pane_id: u32) -> Option<Location> {
        agents::checkout_location(
            self.firecracker_worktrees
                .get(&pane_id)
                .map(|checkout| checkout.worktree.as_str()),
            self.hosts.get(&pane_id).map(String::as_str),
            self.panes
                .iter()
                .flatten()
                .find(|pane| pane.pane_id == pane_id)
                .and_then(|pane| pane.agent.as_ref())
                .and_then(|agent| agent.cwd.as_deref())
                .filter(|cwd| !cwd.is_empty())
                .or_else(|| self.cwds.get(&pane_id).map(String::as_str)),
        )
    }

    /// Every folder a pane we still know of is in.
    ///
    /// Keyed off the maps rather than the visible pane list, because that is
    /// what decides how long a folder is remembered — see `hosts`.
    ///
    /// `firecracker_worktrees` is in the key set beside `cwds`, not only read
    /// through it: a VM pane whose guest folder has not been reported yet has a
    /// location all the same, and this list is the live set eviction keys on. A
    /// location `repository_cwds` probes and this one omits is evicted and
    /// probed again on every host event, forever.
    fn pane_locations(&self) -> Vec<Location> {
        self.cwds
            .keys()
            .chain(self.firecracker_worktrees.keys())
            .chain(self.panes.iter().flatten().map(|pane| &pane.pane_id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|pane_id| self.location_of(*pane_id))
            .collect()
    }

    fn agent_locations(&self) -> BTreeSet<Location> {
        self.panes
            .iter()
            .flatten()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| self.pane_location(pane))
            .collect()
    }

    /// Run Git against folders on *this* machine.
    ///
    /// Two processes each, answered asynchronously through `RunCommandResult`,
    /// which is why every one of them needs correlating back — see
    /// `finish_git_probe`. The correlation key is the folder's full location, so
    /// it cannot be confused with a folder of the same path elsewhere.
    fn start_git_probes(&mut self, locations: Vec<Location>) {
        for location in locations {
            let cwd = location.path.clone();
            self.git_probe_seq += 1;
            let id = self.git_probe_seq;
            self.git_probes.insert(
                location.clone(),
                GitProbe {
                    id,
                    ..GitProbe::default()
                },
            );
            let context = |kind: &str| {
                BTreeMap::from([
                    ("git_probe".to_owned(), kind.to_owned()),
                    ("git_probe_id".to_owned(), id.to_string()),
                    ("cwd".to_owned(), location.key()),
                ])
            };
            run_command_with_env_variables_and_cwd(
                &[
                    "git",
                    "rev-parse",
                    "--path-format=absolute",
                    "--show-toplevel",
                    "--git-dir",
                    "--git-common-dir",
                    "--abbrev-ref",
                    "HEAD",
                ],
                BTreeMap::new(),
                cwd.clone().into(),
                context("metadata"),
            );
            run_command_with_env_variables_and_cwd(
                &[
                    "git",
                    "config",
                    "--null",
                    "--get-regexp",
                    "^(remote\\..*\\.url|remote\\.pushdefault|branch\\..*\\.remote)$",
                ],
                BTreeMap::new(),
                cwd.clone().into(),
                context("config"),
            );
        }
    }

    fn refresh_remote_repositories(&mut self) {
        self.refresh_remote_repositories_with(poll::pane_git_probe);
    }

    fn remote_pane_ids(&self) -> Vec<u32> {
        self.panes
            .iter()
            .flatten()
            .filter(|pane| pane.host.is_some())
            // A Firecracker pane that mirrors a host worktree is identified by that
            // worktree, which is on *this* machine and probed with the local pass. The
            // guest's own answer is about a copy — a repository of its own, whose remote
            // names the worktree's path — and it would be filed under the guest folder,
            // which nothing asks about any more: evicted on the next event, re-added on
            // the next heartbeat, and a whole-session snapshot published each time round.
            //
            // Filtered here rather than further down so the emptiness check and the
            // cursor arithmetic below both see the real set.
            .filter(|pane| pane.firecracker_worktree.is_none())
            .filter(|pane| {
                pane.status == agents::AgentStatus::Running
                    || !self.arrangement_subscribers.is_empty()
            })
            .map(|pane| pane.pane_id)
            .collect()
    }

    /// Request the checkout location selected for each remote pane. A pending result
    /// keeps arrangement waiting; results are always filed under their actual location.
    /// `probe_of` is injected because host calls are unavailable in native tests.
    fn refresh_remote_repositories_with(
        &mut self,
        mut probe_of: impl FnMut(u32, &str, Option<&str>) -> Result<Option<PaneGitProbe>, String>,
    ) {
        let remote = self.remote_pane_ids();
        self.arrangement_remote_pending
            .retain(|id| remote.contains(id));
        if remote.is_empty() {
            if !self.arrangement_subscribers.is_empty() {
                self.publish();
            }
            return;
        }

        // A rotation, so every remote pane is revisited without paying for all
        // of them on one tick — the same shape as `repair_commands_with`, and
        // for the same reason: each ask is a host round trip.
        //
        // Panes with no answer for the folder they are in go first *within* the
        // rotation rather than ahead of it. Ahead of it was the obvious way and
        // it starves: a pane whose folder never resolves — no Git on the far
        // machine, a folder that is not a checkout — stays unknown forever, so
        // the first few unknowns would take every tick's budget and the panes
        // behind them would never be reached at all.
        if self.remote_probe_cursor >= remote.len() {
            self.remote_probe_cursor = 0;
        }
        let rotation: Vec<u32> = remote
            .iter()
            .copied()
            .cycle()
            .skip(self.remote_probe_cursor)
            .take(remote.len())
            .collect();
        let unknown = |pane_id: &u32| {
            self.location_of(*pane_id)
                .is_none_or(|location| !self.repositories.contains_key(&location))
        };
        let due: Vec<u32> = rotation
            .iter()
            .copied()
            .filter(|id| self.arrangement_remote_pending.contains(id))
            .chain(
                rotation
                    .iter()
                    .copied()
                    .filter(unknown)
                    .chain(rotation.iter().copied().filter(|id| !unknown(id)))
                    .filter(|id| !self.arrangement_remote_pending.contains(id)),
            )
            .take(REMOTE_PROBES_PER_TICK)
            .collect();
        self.remote_probe_cursor = (self.remote_probe_cursor + due.len()) % remote.len();

        let mut changed = false;
        for pane_id in due {
            let Some(requested) = self.location_of(pane_id).filter(|l| !l.path.is_empty()) else {
                self.arrangement_remote_pending.remove(&pane_id);
                continue;
            };
            let refreshing = self.arrangement_remote_pending.contains(&pane_id);
            let token = refreshing
                .then_some(self.arrangement_refresh_token.as_deref())
                .flatten();
            let probe = match probe_of(pane_id, &requested.path, token) {
                Ok(Some(probe)) => probe,
                Ok(None) => continue,
                Err(_) => {
                    if refreshing {
                        self.arrangement_remote_failed.insert(pane_id);
                    }
                    self.arrangement_remote_pending.remove(&pane_id);
                    continue;
                },
            };
            let location = Location::new(Some(probe.host), probe.cwd);
            if location == requested {
                self.arrangement_remote_pending.remove(&pane_id);
            }
            let outcome = poll::repository(probe.exit_code, &probe.metadata, &probe.config);
            if refreshing && outcome == poll::ProbeOutcome::Failed {
                self.arrangement_remote_failed.insert(pane_id);
            }
            changed |= self.record_probe_outcome(location, outcome);
        }
        // Arrangement completion must publish even when metadata is unchanged.
        if changed || !self.arrangement_subscribers.is_empty() {
            self.publish();
        }
    }

    fn observe_activity(&mut self) {
        self.observe_activity_with(poll::read_pane);
    }

    /// Sample every agent pane and keep its activity in step.
    ///
    /// This buys nothing. It once carried the whole summary schedule - fifteen seconds of
    /// stillness bought a hundred and twenty lines of terminal sent to a model - and that
    /// was inference from a photograph, made several times a turn about a pane that had
    /// never said anything about itself. The only summary the daemon buys of its own accord
    /// now is the one an agent's `SessionEnd` asks for; see `on_hook_with`. What survives
    /// here is the idle clock: `activity` is what `status_cell` counts minutes with, and
    /// what `activity_hash` stamps an answer against.
    ///
    /// `read` is injected because it reaches the host, and this has to stay testable on the
    /// host triple.
    fn observe_activity_with(&mut self, mut read: impl FnMut(u32) -> poll::PaneSample) {
        let running: BTreeSet<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| {
                pane.status == agents::AgentStatus::Running
                    && agents::classify(pane, &self.manual_kinds).is_some()
            })
            .map(|pane| pane.pane_id)
            .collect();
        // Every pane still classified as an agent, whatever its status. `AgentStatus` is a
        // lifecycle flag (running/held/exited), not the idle/busy activity heuristic - an
        // agent finishing up and going `Held` is not the pane going away, so its summary
        // should not go away either. It dies only when the pane itself does.
        let known: BTreeSet<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .map(|pane| pane.pane_id)
            .collect();
        self.activity.retain(|id, _| running.contains(id));
        self.summaries.retain(|id, _| known.contains(id));
        self.prompts.retain(|id, _| known.contains(id));
        // A request that ages out never had an answer. Nothing is owed on its behalf: every
        // request left is one somebody pressed a key for, and they can press it again.
        self.summary_requests.retain(|id, request| {
            request.ticks += 1;
            request.ticks <= SUMMARY_TICKS && running.contains(id)
        });
        // The same ageing, on the lane with no pane to prune by. A run that has ended cannot
        // stop being a running agent, so `SUMMARY_TICKS` is the only thing that ever clears
        // one of these — and without it a reply the host dropped would leave that run
        // unaskable for the life of the session, with the key doing nothing every time.
        self.run_summary_requests.retain(|_, request| {
            request.ticks += 1;
            request.ticks <= SUMMARY_TICKS
        });
        for pane_id in running {
            let reading = read(pane_id);
            let sample = self.qualify(pane_id, &reading);
            agents::observe(&mut self.activity, pane_id, sample);
        }
    }

    /// What one poll's reading is worth as evidence about the agent behind the pane.
    ///
    /// Three readings say nothing. One we could not take at all is an error message rather
    /// than a screen — the pane has usually closed under us — and reading anything into it
    /// would put that message in front of the model. A pane the user has scrolled back shows
    /// old output, so its text is not about now at all. And a pane whose picture moved while
    /// its pty stayed silent moved for some reason other than the process — a reflow after a
    /// resize, most often — which is not work either. All three keep the stretch of stillness
    /// going rather than restarting it: see `agents::Sample`.
    fn qualify<'a>(&self, pane_id: u32, reading: &'a poll::PaneSample) -> agents::Sample<'a> {
        if !reading.readable {
            return agents::Sample::Unusable;
        }
        if reading.is_scrolled {
            return agents::Sample::Unusable;
        }
        let wrote_recently = reading
            .last_output_ms_ago
            .is_some_and(|ms| ms.get() <= self.poll_window());
        let moved = agents::activity_hash(&self.activity, pane_id)
            .is_some_and(|hash| hash != agents::fingerprint(&reading.text));
        if moved && !wrote_recently {
            return agents::Sample::Unusable;
        }
        agents::Sample::Text(&reading.text)
    }

    /// How recently a pane must have written to its pty for this poll's picture to be its
    /// doing: one poll interval with half again as much slack, since plugin timers drift.
    fn poll_window(&self) -> u64 {
        if self.poll_window_ms == 0 {
            (ACTIVITY_SECONDS * 1500.0) as u64
        } else {
            self.poll_window_ms
        }
    }

    /// Describe this pane now.
    ///
    /// The only thing that summarises a *live* pane. Nothing is bought on a schedule any
    /// more, so there is no cooldown or settle counter to get past — a person pressing a key
    /// has already answered "is this worth a request".
    ///
    /// The one guard that survives is not about the budget: one request per pane at a
    /// time. `finish_summary_with` correlates strictly on `(pane, id)` and drops everything
    /// that does not match, so a second request would not arrive twice — it would orphan the
    /// first and answer only itself.
    ///
    /// Which of the two paths it takes is the pane's, not the caller's: a pane that reports
    /// for itself is described from what it said, and one that does not from what it shows.
    /// Asking a hook-fed pane's screen here would reintroduce exactly the second, worse
    /// answer that the feed was brought in to retire.
    fn summarize_pane_now(&mut self, pane_id: u32) -> bool {
        // Two different `None`s, and only one of them permits the local fallback — see
        // `llm::summaries_enabled`. Resolved once, because reading it is a host call.
        let configuration = get_plugin_alias_configuration(HOCUS_ALIAS);
        let wanted = hocus_proto::llm::summaries_enabled(&configuration);
        let endpoint = Endpoint::resolve(&configuration, &self.env);
        self.summarize_pane_now_with(
            pane_id,
            endpoint.as_ref(),
            wanted,
            poll::read_pane,
            |url, verb, headers, body, context| {
                web_request(url, verb, headers, body, context, None)
            },
            |pane_id, text| {
                notify_with_pane_id(
                    text,
                    PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
                )
            },
        )
    }

    /// The body of [`Self::summarize_pane_now`], with the host calls injected for the tests —
    /// the same shape `on_hook_with` and `observe_activity_with` already take.
    fn summarize_pane_now_with(
        &mut self,
        pane_id: u32,
        endpoint: Option<&Endpoint>,
        wanted: bool,
        read: impl FnOnce(u32) -> poll::PaneSample,
        send: impl FnOnce(&str, HttpVerb, BTreeMap<String, String>, Vec<u8>, BTreeMap<String, String>),
        toast: impl FnOnce(u32, &str),
    ) -> bool {
        // `summaries "false"` is a user saying "do not do this", and they did not mean
        // "unless I press a key". The same stance the `SessionEnd` lane takes.
        if !wanted {
            return false;
        }
        if self.summary_requests.contains_key(&pane_id) {
            return false;
        }
        // Nothing to describe: the pane has closed, stopped looking like an agent, or is
        // demonstrably back at its shell prompt. The last of those is the one worth naming —
        // `pending_release` means the foreground command has dropped back to a shell, so the
        // screen is a prompt rather than a session, and describing it would name the shell.
        if self.agent_kind(pane_id).is_none() || self.pending_release.contains(&pane_id) {
            return false;
        }
        if self.hooks.contains_key(&pane_id) {
            // A pane that reports for itself. There is no new final message — nobody just
            // stopped — so the run's own entries are the whole of the input.
            return self.send_run_summary(pane_id, endpoint, send, toast);
        }
        // A screen-only pane. There is no honest fallback for one of these without an
        // endpoint: `local_summary` shortens prose the agent wrote, and a screen tail is not
        // that. Before the hook feed there was nothing to fall back to at all, and pressing
        // a key does not create one.
        let Some(endpoint) = endpoint else {
            return false;
        };
        let reading = read(pane_id);
        // A reading that says nothing is no more summarisable for having been asked for: a
        // pane scrolled back off its live tail shows old output, and one we could not read
        // holds an error message rather than anything the agent wrote.
        let agents::Sample::Text(text) = self.qualify(pane_id, &reading) else {
            return false;
        };
        if !agents::worth_summarizing(text) {
            return false;
        }
        // `finish_summary_with` stamps the answer with the hash the request was made from,
        // and a pane the poller has not reached yet has none to stamp. Nothing is lost by
        // refusing: the next heartbeat gives it one.
        if agents::activity_hash(&self.activity, pane_id).is_none() {
            return false;
        }
        let text = text.to_owned();
        self.send_summary(pane_id, &text, endpoint, send);
        true
    }

    /// Describe an archived run now.
    ///
    /// The lane with no pane in it. A run that has ended has no screen to read, no title to
    /// rename and nobody to interrupt with a toast — and the pane id it carries names
    /// whatever holds that number today, which after a session or two is somebody else. So
    /// this is keyed by run id from end to end: the request is recorded under one, the reply
    /// is correlated by one, and the answer is written back to the store under one.
    ///
    /// It is also the one summary path that reads its own input from the store. The daemon
    /// holds `self.runs` for panes it is watching; an archived run is by definition not one
    /// of those, and the same two calls `adopt_stored_run` makes are what fetch it.
    fn summarize_run_now(&mut self, run_id: &str) -> bool {
        let configuration = get_plugin_alias_configuration(HOCUS_ALIAS);
        let wanted = hocus_proto::llm::summaries_enabled(&configuration);
        let endpoint = Endpoint::resolve(&configuration, &self.env);
        self.summarize_run_now_with(
            run_id,
            endpoint.as_ref(),
            wanted,
            |url, verb, headers, body, context| {
                web_request(url, verb, headers, body, context, None)
            },
        )
    }

    /// The body of [`Self::summarize_run_now`], with the host call injected for the tests.
    fn summarize_run_now_with(
        &mut self,
        run_id: &str,
        endpoint: Option<&Endpoint>,
        wanted: bool,
        send: impl FnOnce(&str, HttpVerb, BTreeMap<String, String>, Vec<u8>, BTreeMap<String, String>),
    ) -> bool {
        if !wanted {
            return false;
        }
        if self.run_summary_requests.contains_key(run_id) {
            return false;
        }
        let Some(run) = self.load_run(run_id) else {
            return false;
        };
        // No endpoint: no API key, or summaries switched off for the model but not for the
        // feature. The same fallback the key press has, and for the same reason — a run's
        // `Result` entries are the agent's own prose, written for a person to read, which is
        // the one thing `local_summary` can shorten honestly. It is the record's *events*
        // that cannot be fed to it, and none of those go in here.
        let Some(endpoint) = endpoint else {
            return self.summarize_run_locally(run);
        };
        let asked = run
            .entries
            .iter()
            .rev()
            .find(|entry| entry.kind == agents::RunEntryKind::Prompt)
            .and_then(|entry| kept_prompt(&entry.text));
        let Some(message) = run_delta(&run) else {
            // A run with no retained entries at all — every hook it saw was a tool call or a
            // lifecycle event, none of which is kept. There is nothing to describe.
            return false;
        };
        let body = agents::run_summary_request_body(
            &endpoint.model,
            &run.kind,
            run.cwd.as_deref(),
            // No branch. The daemon probes checkouts for panes it is watching, and an
            // archived run has none; the folder it names may not even be a checkout any
            // more. A guess made from today's repositories would date the sentence wrongly.
            None,
            // The *newest* prompt, which is what the pane lane sends too. This field exists
            // to carry one prompt past the tail `run_delta` takes, and the tail cuts from the
            // old end — so naming the run's first prompt here would assert in the header the
            // one question the body is most likely to have dropped, and describe ten turns of
            // work as an answer to the question that opened the session.
            //
            // Through `kept_prompt`, which is the reduction the body beside it already had:
            // a header that quoted the markup would tell the model the turn was about six
            // hundred characters of opening tags and a temp path, in the same request whose
            // history says it was about a background task.
            asked.as_deref(),
            &message,
        );
        self.summary_seq += 1;
        let id = self.summary_seq;
        self.run_summary_requests.insert(
            run_id.to_owned(),
            InFlight {
                id,
                // Nothing to stamp: a run that has ended has no screen to have moved.
                hash: 0,
                ticks: 0,
            },
        );
        send(
            &endpoint.url,
            HttpVerb::Post,
            endpoint.headers(),
            body,
            BTreeMap::from([
                (SUMMARY_RUN.to_owned(), run_id.to_owned()),
                (SUMMARY_ID.to_owned(), id.to_string()),
            ]),
        );
        true
    }

    /// One run by id, from whichever of the two places is holding it.
    ///
    /// `dirty_runs` first, because a run summarised twice in quick succession has its first
    /// answer sitting there unwritten, and reading past it would build the second request
    /// against a state that is already superseded.
    fn load_run(&mut self, run_id: &str) -> Option<agents::AgentRun> {
        if let Some(run) = self.dirty_runs.get(run_id) {
            return Some(run.clone());
        }
        if let Some(run) = self.runs.values().find(|run| run.id == run_id) {
            return Some(run.clone());
        }
        self.stored_run(&agents::rows::run_by_id_request(run_id))
    }

    /// Take the reply to a summary of an archived run.
    ///
    /// [`Self::finish_summary_with`] with everything a pane afforded taken away: no rename,
    /// because the pane whose name it would take is somebody else's now; no toast, because
    /// nothing happened — the user asked, and is looking at the panel where the answer
    /// lands; and no entry in `summaries`, which is keyed by pane and describes what is
    /// running rather than what ran.
    ///
    /// No staleness check either, and here there is not even a rule to skip: an archived run
    /// is a record, and records do not move.
    fn finish_run_summary(&mut self, status: u16, body: &[u8], context: &BTreeMap<String, String>) {
        let (Some(run_id), Some(id)) = (
            context.get(SUMMARY_RUN),
            context.get(SUMMARY_ID).and_then(|v| v.parse::<u64>().ok()),
        ) else {
            return;
        };
        if self.run_summary_requests.get(run_id).map(|r| r.id) != Some(id) {
            return;
        }
        // Removed whatever happens next: the request is over either way, and leaving it
        // would block this run from ever being asked about again.
        self.run_summary_requests.remove(run_id);
        // A failed summary is simply no summary — the same stance the pane lane takes, and
        // for the same reason. The user can press the key again.
        if !(200..300).contains(&status) {
            return;
        }
        let Some(summary) = agents::parse_summary(body, 0) else {
            return;
        };
        let Some(run) = self.load_run(run_id) else {
            return;
        };
        self.attach_stored_summary(run, summary);
    }

    /// Describe an ended run from its own last message, with no model asked.
    ///
    /// The run lane's half of the fallback [`Self::send_run_summary`] already has: a session
    /// with no endpoint still ends, and before this it ended into a blank row. The newest
    /// `Result` entry is the agent's own last words — the same selection the pane lane makes,
    /// read out of the record rather than off a screen that has gone.
    ///
    /// No toast and no rename, for the reason [`Self::finish_run_summary`] has none: the pane
    /// this run was in is a shell prompt or somebody else's by now.
    fn summarize_run_locally(&mut self, run: agents::AgentRun) -> bool {
        let Some(message) = run
            .entries
            .iter()
            .rev()
            .find(|entry| {
                matches!(
                    entry.kind,
                    agents::RunEntryKind::Result | agents::RunEntryKind::Message
                )
            })
            .map(|entry| entry.text.clone())
        else {
            return false;
        };
        // Hash 0: a run that has ended has no screen to have moved, so there is nothing for a
        // staleness check to compare and nothing that would make this answer old.
        let Some(summary) = agents::local_summary(&message, 0) else {
            return false;
        };
        self.attach_stored_summary(run, summary);
        true
    }

    /// Hang a summary on a run the daemon is not watching, and store it.
    ///
    /// Straight to the store rather than through [`Self::attach_run_summary`], which writes to
    /// `self.runs` — an ended run is not in it, and putting it there would make the daemon
    /// hold a session it is not watching and cannot see end.
    fn attach_stored_summary(&mut self, mut run: agents::AgentRun, mut summary: agents::Summary) {
        // Stamped here rather than in the parser: this is where a summary becomes one the
        // daemon keeps, and it is the side with a clock. The panel draws the age from it.
        // Against the run's own summary, which is the one this replaces: a re-summary that
        // changed no word keeps the age of the answer it repeats.
        agents::stamp_summary(&mut summary, run.summary.as_ref(), now_ms());
        agents::push_summary_entry(&mut run, &summary.text, now_ms());
        run.summary = Some(summary);
        run.summarized_revision = run.revision;
        self.dirty_runs.insert(run.id.clone(), run);
        self.publish();
    }

    /// Build and send one summary request from a pane's screen. `send` is injected because
    /// it reaches the host.
    ///
    /// Only ever reached from the summarise key, against a pane whose agent does not report
    /// for itself: there is no screen to read on a pane that does, and nothing reads a
    /// screen unasked any more.
    fn send_summary(
        &mut self,
        pane_id: u32,
        text: &str,
        endpoint: &Endpoint,
        send: impl FnOnce(&str, HttpVerb, BTreeMap<String, String>, Vec<u8>, BTreeMap<String, String>),
    ) {
        let Some(pane) = self
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
        else {
            return;
        };
        let Some((kind, _)) = agents::classify(pane, &self.manual_kinds) else {
            return;
        };
        // The branch the daemon already probed for this pane's folder. Context the model
        // cannot get from the pane text, and free here.
        let branch = self
            .pane_location(pane)
            .and_then(|location| self.repositories.get(&location))
            .and_then(|repository| repository.branch.clone());
        let tail = agents::tail(text, agents::SUMMARY_LINES, agents::SUMMARY_CHARS);
        let body = agents::summary_request_body(
            &endpoint.model,
            kind,
            pane.cwd.as_deref(),
            branch.as_deref(),
            &tail,
        );

        // Unwrap-free: `summarize_pane_now_with` is what gates every caller, and it has
        // already established that this pane has a hash to stamp an answer with.
        let hash = agents::activity_hash(&self.activity, pane_id).unwrap_or_default();
        self.summary_seq += 1;
        let id = self.summary_seq;
        self.summary_requests
            .insert(pane_id, InFlight { id, hash, ticks: 0 });
        send(
            &endpoint.url,
            HttpVerb::Post,
            endpoint.headers(),
            body,
            BTreeMap::from([
                (SUMMARY_PANE.to_owned(), pane_id.to_string()),
                (SUMMARY_ID.to_owned(), id.to_string()),
            ]),
        );
    }

    /// Take a summary reply, if it is one and still wanted.
    ///
    /// Every `WebRequestResult` in the session reaches this, so the context is what says
    /// the reply is ours at all — the same correlation `finish_git_probe` does.
    fn finish_summary(&mut self, status: u16, body: &[u8], context: &BTreeMap<String, String>) {
        // Which key is present is what says which lane asked. A run's reply has no pane to
        // rename or toast over and is correlated in a map of its own, so it must never be
        // taken up by the half that would do those things.
        if context.contains_key(SUMMARY_RUN) {
            self.finish_run_summary(status, body, context);
            return;
        }
        self.finish_summary_with(
            status,
            body,
            context,
            |pane_id, title| {
                rename_pane_with_id(
                    PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
                    title,
                )
            },
            |pane_id, text| {
                notify_with_pane_id(
                    text,
                    PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
                )
            },
        );
    }

    /// The body of [`finish_summary`], with the host calls injected so the tests can watch
    /// them — the same shape `observe_activity` and `send_summary` already take.
    fn finish_summary_with(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
        rename: impl FnOnce(u32, &str),
        toast: impl FnOnce(u32, &str),
    ) {
        let (Some(pane_id), Some(id)) = (
            context
                .get(SUMMARY_PANE)
                .and_then(|v| v.parse::<u32>().ok()),
            context.get(SUMMARY_ID).and_then(|v| v.parse::<u64>().ok()),
        ) else {
            return;
        };
        // A reply from a request we have since replaced or aged out, or for a pane that
        // has closed, answers a question nobody is asking any more.
        if self.summary_requests.get(&pane_id).map(|r| r.id) != Some(id) {
            return;
        }
        // Removed whatever happens next: the request is over either way, and leaving it
        // here would block this pane from ever asking again.
        let Some(request) = self.summary_requests.remove(&pane_id) else {
            return;
        };
        let hash = request.hash;
        // Nothing here goes stale. Every request that reaches this is one somebody pressed
        // a key for, and the asker is still waiting whatever the pane has done since; the
        // two staleness rules that used to live here were both about throwing away an
        // answer nobody had asked for. For the same reason every summary that lands here
        // stays on the timeline where it was made — see `attach_run_summary`.
        // The agent went away between the request leaving and its answer landing: the pane
        // closed, or it is back at its prompt. The answer describes a session that is over,
        // and storing it would put a summary on a pane that has nothing to summarise —
        // toasting it is worse still.
        if self.agent_kind(pane_id).is_none() || self.pending_release.contains(&pane_id) {
            return;
        }
        // A failed summary is simply no summary — the same stance `ProbeOutcome::Failed`
        // takes, and for the same reason: a guess here would outlive the reason for it.
        // The user can press the key again.
        if !(200..300).contains(&status) {
            return;
        }
        let Some(mut summary) = agents::parse_summary(body, hash) else {
            return;
        };
        // The model was asked whether the agent is blocked on the user, and where the
        // agent says so itself the model's guess is not wanted. It is still a good
        // guess - it is the only answer there is for every other agent - but a pane
        // with a feed has the fact, and two sources for one flag is a disagreement
        // waiting to be drawn. `waiting_marker` prefers the feed for the same reason;
        // overwriting here keeps the stored summary from ever contradicting the row.
        if let Some(feed) = self.hooks.get(&pane_id) {
            summary.awaiting = feed.phase == agents::HookPhase::Waiting;
        }
        // Stamped here rather than in the parser: this is where a summary becomes one the
        // daemon keeps, and it is the side with a clock. The panel draws the age from it.
        // Below the `awaiting` fixup, and against what this pane already holds, so a
        // re-summary that changed no word keeps the age of the answer it repeats.
        agents::stamp_summary(&mut summary, self.summaries.get(&pane_id), now_ms());
        if let Some(title) = agents::rename_title(&summary) {
            if self.title_is_ours_to_take(pane_id) {
                rename(pane_id, title);
            }
        }
        // Below every gate above, so a stale, failed or unreadable answer is silent: a toast
        // is an interruption, and one describing a screen that has moved on is worse than
        // none. The snapshot below carries the same summary to whoever is subscribed, but a
        // snapshot only reaches a plugin that is running and only shows in a pane the user
        // is looking at — this is the half that finds them wherever they are.
        // A screen summary's `awaiting` is still useful in the strip, but it is a guess and
        // does not interrupt. Hook events report real questions immediately above. Finished
        // summaries still toast, unless the user is already looking at the pane.
        if !summary.awaiting && self.focused != Some(pane_id) {
            let kind = self.toast_kind(pane_id);
            toast(
                pane_id,
                &agents::summary_toast(pane_id, kind.as_deref(), &summary),
            );
        }
        self.attach_run_summary(pane_id, &summary);
        self.summaries.insert(pane_id, summary);
        self.publish();
    }

    // -- Completions on another plugin's behalf -----------------------------------------
    //
    // The other thing a plugin might want from a session that has a model: not a summary of
    // an agent, but its own turn. Everything the model sees arrives in the request and
    // nothing is kept between them, so a caller drives its own tool loop by sending the
    // whole thing back each round. The daemon serves it because it is the half of hocus
    // that is always running and has no conversation of its own to contend with.

    /// Take a completion request from a plugin or a `zellij pipe`.
    fn on_complete(&mut self, message: &PipeMessage) {
        // Read once: each of these is a host call, and hocus's block is what configures
        // them, for the reason `HOCUS_ALIAS` gives.
        let configuration = get_plugin_alias_configuration(HOCUS_ALIAS);
        let allowed = hocus_proto::llm::plugin_completions_enabled(&configuration);
        let pinned = hocus_proto::llm::pinned_completion_model(&configuration);
        let endpoint = Endpoint::resolve_for_completion(&configuration, &self.env);
        self.on_complete_with(
            &message.source,
            message.payload.as_deref(),
            endpoint.as_ref(),
            allowed,
            pinned.as_deref(),
            |url, verb, headers, body, context| {
                web_request(url, verb, headers, body, context, None)
            },
            answer_completion,
            block_cli_pipe_input,
        );
    }

    /// The body of [`Self::on_complete`], with the host calls injected for the tests — the
    /// same shape `summarize_pane_now_with` and `finish_summary_with` take.
    ///
    /// Every path answers, refusals included, for the reason hocus's `on_ask` acknowledges
    /// even the questions it will not take: a caller cannot tell a daemon that heard and
    /// said nothing from a session with no daemon in it. Answering is also what releases a
    /// blocked `zellij pipe`, so a silent path would hang a terminal.
    ///
    /// Returns whether the request went out, for the tests.
    fn on_complete_with(
        &mut self,
        source: &PipeSource,
        payload: Option<&str>,
        endpoint: Option<&Endpoint>,
        allowed: bool,
        pinned: Option<&str>,
        send: impl FnOnce(&str, HttpVerb, BTreeMap<String, String>, Vec<u8>, BTreeMap<String, String>),
        answer: impl FnOnce(&PipeSource, &wire::CompletionReply),
        block: impl FnOnce(&str),
    ) -> bool {
        // Before anything that can answer, so the block and its release are always paired.
        if let PipeSource::Cli(pipe_id) = source {
            block(pipe_id);
        }
        let payload = payload.unwrap_or_default();
        // Length first, on the JSON as it arrived: a caller that sends a megabyte should
        // pay for one comparison rather than a megabyte of parsing.
        if let Err(detail) = completion::within_size(payload) {
            answer(source, &completion::error_reply("", &detail));
            return false;
        }
        let request = match serde_json::from_str::<wire::CompletionRequest>(payload) {
            Ok(request) => request,
            Err(error) => {
                let value = serde_json::from_str::<serde_json::Value>(payload).ok();
                let request_id = value
                    .as_ref()
                    .and_then(|value| value.get("request_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                answer(
                    source,
                    &completion::error_reply(
                        request_id,
                        &format!("that is not a completion request: {error}"),
                    ),
                );
                return false;
            },
        };
        if let Err(detail) = completion::validate(&request) {
            answer(
                source,
                &completion::error_reply(&request.request_id, &detail),
            );
            return false;
        }
        let refusal = self.refuse_completion(source, endpoint, allowed);
        if let Some(detail) = refusal {
            answer(
                source,
                &completion::error_reply(&request.request_id, &detail),
            );
            return false;
        }
        let endpoint = endpoint.expect("refuse_completion answers a missing endpoint");
        // A pinned model is the session's answer to "not at a caller's choice of price", so
        // it beats the caller's; with none pinned the caller's beats the configured one,
        // because a plugin that knows it needs a reasoning model should be able to say so.
        let model = pinned
            .or(request.model.as_deref())
            .unwrap_or(&endpoint.model);

        self.completion_seq += 1;
        let id = self.completion_seq;
        self.completions.insert(
            id,
            Completion {
                reply_to: source.clone(),
                request_id: request.request_id.clone(),
                ticks: 0,
            },
        );
        send(
            &endpoint.url,
            HttpVerb::Post,
            endpoint.headers(),
            completion::completion_request_body(&request, model),
            BTreeMap::from([(COMPLETE_ID.to_owned(), id.to_string())]),
        );
        true
    }

    /// Why this request will not go out, or `None`.
    ///
    /// Split out so the order is readable: what the user switched off, then what the caller
    /// asked for, then what the session can afford, then whether there is a model at all.
    fn refuse_completion(
        &self,
        source: &PipeSource,
        endpoint: Option<&Endpoint>,
        allowed: bool,
    ) -> Option<String> {
        if !allowed {
            return Some(
                "hocus is configured with plugin_completions \"false\"; no completion was made"
                    .to_owned(),
            );
        }
        if self.completions.len() >= COMPLETIONS_TOTAL {
            return Some(format!(
                "hocus-agentd already has {COMPLETIONS_TOTAL} completions in flight; try again \
                 when one finishes"
            ));
        }
        // Equality on the source is equality on the caller: a plugin id names one plugin,
        // and two `zellij pipe` invocations carry different pipe ids. Only `Keybind`, which
        // is never answered and so never gets this far twice, is indistinguishable.
        let mine = self
            .completions
            .values()
            .filter(|pending| pending.reply_to == *source)
            .count();
        if mine >= COMPLETIONS_PER_CALLER {
            return Some(format!(
                "one caller may have {COMPLETIONS_PER_CALLER} completions in flight at a time"
            ));
        }
        // Last, because it is the only refusal that is about the session rather than about
        // what was asked, and a caller reading one reason wants the actionable one first.
        if endpoint.is_none() {
            return Some(
                "hocus has no API key configured; set OPENAI_API_KEY or point api_key_env at \
                 the variable that holds one"
                    .to_owned(),
            );
        }
        None
    }

    /// Take a completion reply and hand it to whoever asked.
    fn finish_completion(&mut self, status: u16, body: &[u8], context: &BTreeMap<String, String>) {
        self.finish_completion_with(status, body, context, answer_completion);
    }

    /// The body of [`Self::finish_completion`], with the reply injected for the tests.
    fn finish_completion_with(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
        answer: impl FnOnce(&PipeSource, &wire::CompletionReply),
    ) {
        let Some(id) = context.get(COMPLETE_ID).and_then(|v| v.parse::<u64>().ok()) else {
            return;
        };
        // Removed whatever happens next: the request is over either way, and leaving it
        // would hold a slot against this caller for the life of the session.
        let Some(pending) = self.completions.remove(&id) else {
            return;
        };
        // No staleness rule, for the reason `finish_summary_with` gives for a key press,
        // only more so: somebody is blocked on this answer.
        answer(
            &pending.reply_to,
            &completion::parse_completion(status, body, &pending.request_id),
        );
    }

    /// Answer and clear every completion that has been outstanding too long.
    ///
    /// Where the two summary lanes simply forget, this replies: the caller is blocked — a
    /// `zellij pipe` literally so — and a `WebRequestResult` the host dropped would
    /// otherwise hang a terminal for the life of the session.
    fn age_completions_with(
        &mut self,
        mut answer: impl FnMut(&PipeSource, &wire::CompletionReply),
    ) {
        let mut expired = Vec::new();
        self.completions.retain(|_, pending| {
            pending.ticks += 1;
            if pending.ticks > COMPLETE_TICKS {
                expired.push((pending.reply_to.clone(), pending.request_id.clone()));
                return false;
            }
            true
        });
        for (source, request_id) in expired {
            answer(
                &source,
                &completion::error_reply(&request_id, "no answer from the model in time"),
            );
        }
    }

    /// Forget everything this daemon holds about a pane *as an agent*.
    ///
    /// For the two verdicts that end an agent rather than pause one: a latch released because
    /// the pane is demonstrably back at its prompt, and a re-run putting a new process behind
    /// the same id. The retention rules in `observe_activity_with` reach the same state on the
    /// next heartbeat, but only after up to a poll interval of showing a summary of a session
    /// that is over — and, for the released latch, one more chance to ask about it.
    ///
    /// Not for a pane that has merely gone `Held` or `Exited`: that is an agent that finished
    /// its work, and its last summary is the most useful thing on the row.
    fn forget_agent(&mut self, pane_id: u32) {
        self.hooks.remove(&pane_id);
        self.event_fed.remove(&pane_id);
        self.activity.remove(&pane_id);
        self.summaries.remove(&pane_id);
        self.summary_requests.remove(&pane_id);
        self.prompts.remove(&pane_id);
    }

    /// What kind of agent a pane is running, if it is one we can still classify.
    ///
    /// `None` for a pane that has closed or stopped looking like an agent between the
    /// request going out and its answer landing, which is why `finish_summary_with` asks it
    /// before storing anything: there is no session left for the answer to be about.
    fn agent_kind(&self, pane_id: u32) -> Option<&str> {
        let pane = self
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)?;
        agents::classify(pane, &self.manual_kinds).map(|(kind, _)| kind)
    }

    /// The kind a toast should name this pane by: what it is, marked if it reports for
    /// itself.
    ///
    /// The same spelling the strip's rows use, from the same formatter — a user reading
    /// `claude◆` in a box in the corner and `claude◆` in the sidebar is looking at one
    /// pane named one way. See `agents::kind_cell`.
    fn toast_kind(&self, pane_id: u32) -> Option<String> {
        let kind = self.agent_kind(pane_id)?;
        Some(agents::kind_cell(
            kind,
            None,
            self.hooks.contains_key(&pane_id),
        ))
    }

    /// Whether renaming this pane would cost us the pane.
    ///
    /// `classify` reads the title before anything else, so a pane recognised *only* by
    /// what its title says — a `zsh` running an agent we have never seen the command for —
    /// would stop being an agent the moment we wrote a task name over it, and stop being
    /// polled with it. Rename only where the kind is anchored somewhere the title cannot
    /// reach: the user's own registration, the session's own agent record, or the command
    /// latch (`settle_latches`), which is the memory of a command we demonstrably saw run.
    ///
    /// The record is the anchor that matters for a prompt title: `classify` consults
    /// `pane.agent` *before* it reads the title, so a pane the session holds a record for
    /// cannot be un-detected by a rename — and every hook-connected pane has one, written by
    /// `agent-report` in the same breath as the broadcast this reacts to.
    fn title_is_ours_to_take(&self, pane_id: u32) -> bool {
        if self.manual_kinds.contains_key(&pane_id) {
            return true;
        }
        self.panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
            .is_some_and(|pane| {
                pane.agent.is_some()
                    || pane.detected.is_some()
                    || pane
                        .command
                        .as_deref()
                        .and_then(agents::kind_from_command)
                        .is_some()
            })
    }

    fn finish_git_probe(
        &mut self,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        context: BTreeMap<String, String>,
    ) {
        let (Some(location), Some(kind), Some(id)) = (
            context.get("cwd").map(|key| Location::parse(key)),
            context.get("git_probe"),
            context
                .get("git_probe_id")
                .and_then(|id| id.parse::<u64>().ok()),
        ) else {
            return;
        };
        let Some(probe) = self
            .git_probes
            .get_mut(&location)
            .filter(|probe| probe.id == id)
        else {
            return;
        };
        match kind.as_str() {
            // The exit code rides along: the parse needs it to tell "not a
            // repository" from "the probe answered nothing", and stdout is worth
            // reading either way.
            "metadata" => probe.metadata = Some((exit_code, stdout)),
            // Nothing but remotes comes out of this one, and a repository with
            // none is ordinary, so a failure here is simply no remotes.
            "config" => {
                probe.config = Some(if exit_code == Some(0) {
                    stdout
                } else {
                    Vec::new()
                })
            },
            _ => return,
        }
        if probe.metadata.is_none() || probe.config.is_none() {
            return;
        }
        let Some(probe) = self.git_probes.remove(&location) else {
            return;
        };
        let (exit_code, metadata) = probe.metadata.unwrap_or_default();
        let outcome = poll::repository(
            exit_code,
            &metadata,
            probe.config.as_deref().unwrap_or_default(),
        );
        self.record_probe_outcome(location, outcome);
        self.publish();
    }

    /// File what one folder's probe concluded, local or remote.
    ///
    /// Returns whether anything changed, so a caller that may run on every
    /// heartbeat can stay quiet when it learned nothing.
    fn record_probe_outcome(&mut self, location: Location, outcome: poll::ProbeOutcome) -> bool {
        let host = location.host.clone();
        let recorded = match outcome {
            poll::ProbeOutcome::Checkout(repository) => Some(repository.on_host(host)),
            // Recorded, because "we looked here and Git said no" is what stops
            // the next event probing it again.
            poll::ProbeOutcome::NotARepository => {
                Some(projects::Repository::folder(location.path.clone()).on_host(host))
            },
            // Deliberately not recorded. A guess here would outlive the reason
            // for it and deny the pane a project for as long as the entry lives.
            poll::ProbeOutcome::Failed => {
                // Only a *local* probe waits: it costs two processes, so
                // repeating it on every event would be real work spent on a
                // missing `git`. A remote ask costs one channel round trip and
                // fails only for reasons that pass on their own — the link, the
                // broker not having answered yet — so it simply asks again next
                // tick.
                if location.host.is_none() {
                    self.probe_backoff
                        .insert(location.clone(), GIT_RETRY_SECONDS);
                }
                None
            },
        };
        let Some(repository) = recorded else {
            return false;
        };
        if self.repositories.get(&location) == Some(&repository) {
            return false;
        }
        self.repositories.insert(location, repository);
        true
    }

    /// Put each agent pane's checkout on its frame, and take it off the panes
    /// that are no longer agents.
    ///
    /// Deliberately not a rename. `set_pane_title_prefix` composes in front of
    /// whatever title a pane has, so this never competes with the four writers
    /// of a pane's *name* — the user, the pane's own OSC title, the prompt an
    /// agent reports in `on_hook_with`, and `finish_summary_with` below. That is also why it needs no
    /// `title_is_ours_to_take` guard: the prefix stays out of the title
    /// `classify` reads, so it cannot un-detect the pane it decorates.
    ///
    /// Driven from `publish` rather than the heartbeat, so the frame changes the
    /// moment a pane is detected or a Git probe answers rather than up to a poll
    /// later — both of those publish, and both are what this reads.
    fn refresh_title_prefixes(&mut self) {
        self.refresh_title_prefixes_with(|pane_id, prefix| {
            set_pane_title_prefix(
                PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
                prefix,
            )
        });
    }

    /// The body of [`refresh_title_prefixes`], with the host call injected so the
    /// tests can watch it — the same shape `finish_summary_with` takes.
    fn refresh_title_prefixes_with(&mut self, mut set: impl FnMut(u32, Option<&str>)) {
        let wanted: BTreeMap<u32, String> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| {
                let location = self.pane_location(pane)?;
                Some((
                    pane.pane_id,
                    self.repositories.get(&location)?.checkout_label()?,
                ))
            })
            .collect();
        let live: BTreeSet<u32> = self
            .panes
            .iter()
            .flatten()
            .map(|pane| pane.pane_id)
            .collect();
        for pane_id in self.title_prefixes.keys().copied().collect::<Vec<_>>() {
            // A pane that has left the snapshot is closed or suppressed, and
            // terminal ids are recycled — clearing one would be aimed at
            // whatever holds the id now. Forgetting it is enough: if the pane
            // comes back it is re-decorated from scratch.
            if !wanted.contains_key(&pane_id) && live.contains(&pane_id) {
                set(pane_id, None);
            }
        }
        for (pane_id, label) in &wanted {
            if self.title_prefixes.get(pane_id) != Some(label) {
                set(*pane_id, Some(label));
            }
        }
        self.title_prefixes = wanted;
    }

    /// The other agent panes in the focused agent's tab that are on the same checkout.
    ///
    /// Empty whenever there is nothing to say: no focus, focus on a plugin or a shell, a
    /// folder Git did not claim, or one no probe has answered for yet. Empty is a real
    /// answer here rather than an absence - it is what takes a highlight down.
    ///
    /// Match the repository's host, full common directory and branch. Worktrees of one
    /// repository on the same branch match; unrelated repositories with identical display
    /// labels do not. Two detached HEADs in the same repository also match.
    fn related_agent_panes(&self) -> BTreeSet<u32> {
        let empty = BTreeSet::new();
        // `focused` is the focused *terminal* pane of the active tab, so a plugin pane with
        // the keyboard already answers `None` here - see `panes::focused_pane`.
        let Some(focused_id) = self.focused else {
            return empty;
        };
        let Some(focused) = self
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == focused_id)
        else {
            return empty;
        };
        if agents::classify(focused, &self.manual_kinds).is_none() {
            return empty;
        }
        // A pane whose manifest position is in no tab we know about. "The same tab" has no
        // meaning there, so say nothing rather than guess at one.
        let Some(tab_id) = focused.tab.as_ref().map(|(id, _)| *id) else {
            return empty;
        };
        let Some(key) = self.checkout_key_of(focused) else {
            return empty;
        };
        self.panes
            .iter()
            .flatten()
            .filter(|pane| pane.pane_id != focused_id)
            .filter(|pane| pane.tab.as_ref().map(|(id, _)| *id) == Some(tab_id))
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter(|pane| self.checkout_key_of(pane) == Some(key))
            .map(|pane| pane.pane_id)
            .collect()
    }

    /// Repository identity and branch, or `None` without a probe or common directory.
    fn checkout_key_of(
        &self,
        pane: &agents::AgentPane,
    ) -> Option<(Option<&str>, &str, Option<&str>)> {
        let repository = self.repositories.get(&self.pane_location(pane)?)?;
        Some((
            repository.host.as_deref(),
            repository.common_dir.as_deref()?,
            repository.branch.as_deref(),
        ))
    }

    /// Tell the session which agent panes are kin of the focused one.
    fn refresh_related_agents(&mut self) {
        self.refresh_related_agents_with(|pane_ids| {
            set_related_panes(
                pane_ids
                    .into_iter()
                    .map(|id| PaneId::Terminal(TerminalId::new(id)))
                    .collect(),
            )
        });
    }

    /// The body of [`refresh_related_agents`], with the host call injected so the tests can
    /// watch it - the same shape `refresh_title_prefixes_with` takes.
    fn refresh_related_agents_with(&mut self, send: impl FnOnce(Vec<u32>)) {
        let wanted = self.related_agent_panes();
        // `Some(wanted)` compared against `None` on the first pass, so a fresh or reloaded
        // daemon always asserts - including the empty set, which is what clears whatever a
        // previous instance left on the session.
        if self.related_panes.as_ref() == Some(&wanted) {
            return;
        }
        send(wanted.iter().copied().collect());
        self.related_panes = Some(wanted);
    }

    fn publish(&mut self) {
        self.publish_with_force(false);
    }

    /// `publish` even when nothing changed: for a subscriber that just arrived
    /// and holds no snapshot at all, to whom "unchanged" means nothing.
    fn publish_forced(&mut self) {
        self.publish_with_force(true);
    }

    fn publish_with_force(&mut self, force: bool) {
        self.publish_with(force, pipe_message_to_plugin);
    }

    fn publish_with(&mut self, force: bool, mut send: impl FnMut(MessageToPlugin)) {
        self.refresh_title_prefixes();
        self.refresh_related_agents();
        let reply = PollReply {
            panes: self.panes.clone(),
            activity: self.activity.clone(),
            manual_kinds: self.manual_kinds.clone(),
            zellij_session_name: self.zellij_session_name.clone().unwrap_or_default(),
            repositories: self.repositories.clone(),
            tab_projects: self.tab_projects.clone(),
            focused: self.focused,
            summaries: self.summaries.clone(),
            hooks: self.hooks.clone(),
            runs: self
                .runs
                .values()
                .map(agents::AgentRun::header)
                .chain(
                    self.dirty_runs
                        .values()
                        .filter(|run| run.ended_at_ms.is_some())
                        .map(agents::AgentRun::header),
                )
                .collect(),
        };
        let Ok(payload) = serde_json::to_string(&reply) else {
            return;
        };
        if ((self.git_probes.is_empty() && self.arrangement_remote_pending.is_empty())
            || self
                .arrangement_started_at
                .is_some_and(|start| now_ms() - start >= 15_000))
            && !self.arrangement_subscribers.is_empty()
        {
            let positions: BTreeMap<_, _> = self
                .manifest
                .panes
                .iter()
                .flat_map(|(tab, panes)| {
                    panes
                        .iter()
                        .enumerate()
                        .filter(|(_, pane)| !pane.is_plugin())
                        .map(move |(position, pane)| (pane.id.legacy_parts().0, (*tab, position)))
                })
                .collect();
            let mut ordered_reply = reply;
            let unavailable: BTreeSet<_> = self
                .arrangement_remote_failed
                .iter()
                .chain(&self.arrangement_remote_pending)
                .copied()
                .collect();
            if !unavailable.is_empty() {
                notify(&format!("Arrange panes: {} remote panes left untouched because fresh repository metadata is unavailable", unavailable.len()));
            }
            if let Some(panes) = &mut ordered_reply.panes {
                panes.retain(|pane| !unavailable.contains(&pane.pane_id));
                panes.sort_by_key(|pane| (positions.get(&pane.pane_id).copied(), pane.pane_id));
            }
            if let Ok(payload) = serde_json::to_string(&ordered_reply) {
                self.arrangement_started_at = None;
                self.arrangement_refresh_token = None;
                self.arrangement_remote_failed.clear();
                self.arrangement_remote_pending.clear();
                for id in std::mem::take(&mut self.arrangement_subscribers) {
                    send(
                        MessageToPlugin::new("arrange_projects_snapshot")
                            .with_destination_plugin_id(id)
                            .with_payload(payload.clone()),
                    );
                }
            }
        }
        // Withheld when identical to what already went out. Nearly every host
        // event ends here — PaneUpdate and the TabUpdate right behind it, the
        // heartbeat, every hook — and a snapshot that says nothing new still
        // costs every subscriber a JSON parse and a full merge. Compared as the
        // serialized string so the check can never drift from what is sent.
        if !force && self.last_published.as_deref() == Some(payload.as_str()) {
            return;
        }
        self.seq += 1;
        // Addressed by plugin id, never by url: `with_plugin_url("zellij:hocus")`
        // would *launch* hocus, which is precisely what a background monitor
        // must not do. No subscribers means there is simply nobody to notify;
        // the cached snapshot remains ready for the first subscriber.
        for id in &self.subscribers {
            send(
                MessageToPlugin::new(wire::AGENT_SNAPSHOT)
                    .with_destination_plugin_id(*id)
                    .with_payload(payload.clone())
                    .with_args(BTreeMap::from([("seq".to_owned(), self.seq.to_string())])),
            );
        }
        self.last_published = Some(payload);
    }
}

/// Reply to whoever asked for a completion, in the way that kind of caller can hear.
///
/// A plugin gets a [`wire::COMPLETE_RESULT`] addressed to its id, the way hocus's `ack_ask`
/// answers an ask: a caller is one of possibly several and must not read somebody else's
/// answer. A `zellij pipe` is a blocked process waiting on stdout, and answering it the
/// other way would hang the terminal. A keybind is neither and gets nothing — there is just
/// nobody to tell, the same stance `on_ask` takes.
fn answer_completion(source: &PipeSource, reply: &wire::CompletionReply) {
    answer_completion_with(
        source,
        reply,
        pipe_message_to_plugin,
        cli_pipe_output,
        unblock_cli_pipe_input,
    );
}

fn answer_completion_with(
    source: &PipeSource,
    reply: &wire::CompletionReply,
    plugin: impl FnOnce(MessageToPlugin),
    cli: impl FnOnce(&str, &str),
    unblock: impl FnOnce(&str),
) {
    let output = prepare_completion(source, reply);
    match source {
        PipeSource::Plugin(id) => plugin(
            MessageToPlugin::new(wire::COMPLETE_RESULT)
                .with_destination_plugin_id(*id)
                .with_payload(output),
        ),
        PipeSource::Cli(pipe_id) => {
            // One line, so a shell can read it without counting braces.
            cli(pipe_id, &output);
            unblock(pipe_id);
        },
        PipeSource::Keybind => {},
    }
}

fn prepare_completion(source: &PipeSource, reply: &wire::CompletionReply) -> String {
    use prost::Message;
    use zellij_ipc::client_server_contract::client_server_contract::{
        server_to_client_msg, CliPipeOutputMsg, ServerToClientMsg,
    };
    use zellij_ipc::MAX_IPC_FRAME_LEN;

    let mut output = serde_json::to_string(reply).expect("a completion reply is serializable");
    let size = if let PipeSource::Cli(pipe_id) = source {
        output.push('\n');
        let frame = ServerToClientMsg {
            message: Some(server_to_client_msg::Message::CliPipeOutput(
                CliPipeOutputMsg {
                    pipe_name: pipe_id.clone(),
                    output,
                },
            )),
        };
        let size = frame.encoded_len();
        if let Some(server_to_client_msg::Message::CliPipeOutput(message)) = frame.message {
            output = message.output;
        } else {
            unreachable!("constructed a CliPipeOutput frame");
        }
        size
    } else {
        output.len()
    };
    if size > MAX_IPC_FRAME_LEN {
        let error = completion::error_reply(
            &reply.request_id,
            &format!("completion reply exceeds the {MAX_IPC_FRAME_LEN}-byte transport limit"),
        );
        output = serde_json::to_string(&error).expect("a completion error is serializable");
        if matches!(source, PipeSource::Cli(_)) {
            output.push('\n');
        }
    }
    output
}

/// Write a pane's agent record to the session. Off wasm there is no session, and the tests
/// read the daemon's own feed instead.
#[cfg(target_arch = "wasm32")]
fn report_agent(pane_id: u32, agent: Option<AgentInfo>) {
    set_pane_agent(
        PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane_id)),
        agent,
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn report_agent(_pane_id: u32, _agent: Option<AgentInfo>) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder on this machine, which is what nearly every test is about.
    fn local(path: &str) -> Location {
        Location::local(path)
    }

    /// The same folder, on a machine that is not this one.
    fn remote(host: &str, path: &str) -> Location {
        Location::new(Some(host), path)
    }

    fn pane(id: u32, command: &str, cwd: &str) -> agents::AgentPane {
        agents::AgentPane {
            pane_id: id,
            title: command.to_owned(),
            command: Some(command.to_owned()),
            cwd: Some(cwd.to_owned()),
            host: None,
            firecracker_image: None,
            firecracker_worktree: None,
            status: agents::AgentStatus::Running,
            tab: None,
            detected: None,
            agent: None,
        }
    }

    /// A checkout, as `finish_git_probe` would have recorded it.
    fn checkout(root: &str, common_dir: &str, branch: &str) -> projects::Repository {
        projects::Repository {
            root: root.to_owned(),
            worktree_status: Some(projects::WorktreeStatus::Linked),
            common_dir: Some(common_dir.to_owned()),
            branch: Some(branch.to_owned()),
            ..projects::Repository::default()
        }
    }

    /// What `refresh_title_prefixes` would have asked the host to do.
    fn prefixes(state: &mut State) -> Vec<(u32, Option<String>)> {
        let mut calls = Vec::new();
        state.refresh_title_prefixes_with(|pane_id, prefix| {
            calls.push((pane_id, prefix.map(ToOwned::to_owned)))
        });
        calls
    }

    /// An agent in a worktree, and a shell beside it in the same folder.
    fn worktree_agent() -> State {
        State {
            panes: Some(vec![
                pane(7, "claude", "/work/hocus/.worktrees/topic"),
                pane(8, "zsh", "/work/hocus/.worktrees/topic"),
            ]),
            cwds: BTreeMap::from([
                (7, "/work/hocus/.worktrees/topic".to_owned()),
                (8, "/work/hocus/.worktrees/topic".to_owned()),
            ]),
            repositories: BTreeMap::from([(
                Location::local("/work/hocus/.worktrees/topic"),
                checkout("/work/hocus/.worktrees/topic", "/work/hocus/.git", "topic"),
            )]),
            ..State::default()
        }
    }

    /// What `refresh_related_agents` would have told the session, or `None` when it said
    /// nothing at all. The empty vector and `None` are different answers here: one takes a
    /// highlight down, the other leaves it alone.
    fn related(state: &mut State) -> Option<Vec<u32>> {
        let mut sent = None;
        state.refresh_related_agents_with(|pane_ids| sent = Some(pane_ids));
        sent
    }

    fn in_tab(mut pane: agents::AgentPane, tab_id: TabId) -> agents::AgentPane {
        pane.tab = Some((tab_id, format!("tab {tab_id}")));
        pane
    }

    /// One tab holding four agents across three checkouts, plus a shell, and a fifth agent
    /// in another tab. Every case the highlight has to tell apart is in here:
    ///
    /// - 7 and 8 are two agents in the *same* worktree,
    /// - 9 is a worktree of the same repository with the same branch out, which counts,
    /// - 10 is the repository's primary checkout on a different branch, which does not,
    /// - 11 is a shell, not an agent,
    /// - 12 is an agent in the same worktree as 7, but in another tab.
    fn related_agents() -> State {
        let topic = "/work/hocus/.worktrees/topic";
        let twin = "/work/hocus/.worktrees/topic-twin";
        let primary = "/work/hocus";
        State {
            focused: Some(7),
            panes: Some(vec![
                in_tab(pane(7, "claude", topic), TabId::new(0)),
                in_tab(pane(8, "claude", topic), TabId::new(0)),
                in_tab(pane(9, "claude", twin), TabId::new(0)),
                in_tab(pane(10, "claude", primary), TabId::new(0)),
                in_tab(pane(11, "zsh", topic), TabId::new(0)),
                in_tab(pane(12, "claude", topic), TabId::new(1)),
            ]),
            repositories: BTreeMap::from([
                (
                    Location::local(topic),
                    checkout(topic, "/work/hocus/.git", "topic"),
                ),
                (
                    Location::local(twin),
                    checkout(twin, "/work/hocus/.git", "topic"),
                ),
                (
                    Location::local(primary),
                    checkout(primary, "/work/hocus/.git", "main"),
                ),
            ]),
            ..State::default()
        }
    }

    /// The feature, in one assertion: the sibling in the same worktree and the worktree on
    /// the same branch light up; the other branch, the shell and the other tab do not.
    #[test]
    fn focusing_an_agent_lights_up_its_checkout_siblings() {
        let mut state = related_agents();
        assert_eq!(related(&mut state), Some(vec![8, 9]));
    }

    /// Sharing a repository is not enough when the branches differ.
    #[test]
    fn a_different_branch_of_the_same_repository_is_not_a_sibling() {
        let mut state = related_agents();
        state.focused = Some(10);
        assert_eq!(
            related(&mut state),
            Some(vec![]),
            "the primary checkout is on `main`, and nothing else here is"
        );
    }

    #[test]
    fn identical_checkout_labels_in_different_repositories_are_not_siblings() {
        let mut state = related_agents();
        let twin = local("/work/hocus/.worktrees/topic-twin");
        let repository = state.repositories.get_mut(&twin).unwrap();
        repository.common_dir = Some("/other/hocus/.git".to_owned());
        assert_eq!(repository.checkout_label().as_deref(), Some("hocus:topic"));
        assert_eq!(related(&mut state), Some(vec![8]));
    }

    #[test]
    fn identical_repository_paths_on_different_hosts_are_not_siblings() {
        for host in [None, Some("build-box")] {
            let mut state = related_agents();
            for pane in state.panes.as_mut().unwrap() {
                pane.host = host.map(str::to_owned);
                if pane.pane_id == 9 {
                    pane.host = Some("other-box".to_owned());
                }
            }
            state.repositories = state
                .repositories
                .into_iter()
                .map(|(location, repository)| {
                    let host = if location.path.ends_with("topic-twin") {
                        Some("other-box")
                    } else {
                        host
                    };
                    (
                        Location::new(host, location.path),
                        repository.on_host(host.map(str::to_owned)),
                    )
                })
                .collect();
            assert_eq!(related(&mut state), Some(vec![8]));
        }
    }

    #[test]
    fn detached_heads_in_the_same_repository_are_siblings() {
        let mut state = related_agents();
        for repository in state.repositories.values_mut() {
            if repository.branch.as_deref() == Some("topic") {
                repository.branch = None;
            }
        }
        assert_eq!(related(&mut state), Some(vec![8, 9]));
    }

    #[test]
    fn a_checkout_without_a_common_directory_has_no_siblings() {
        let mut state = related_agents();
        for repository in state.repositories.values_mut() {
            repository.common_dir = None;
        }
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// A fresh daemon has no idea what the session still believes, so it says so even when
    /// it has nothing to highlight. Without this a reload could not clear a stale tint.
    #[test]
    fn a_fresh_daemon_asserts_the_empty_set() {
        let mut state = State::default();
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// This runs on every publish, and a publish happens several times a poll.
    #[test]
    fn an_unchanged_set_is_not_sent_again() {
        let mut state = related_agents();
        related(&mut state);
        assert_eq!(related(&mut state), None);
    }

    /// Focus moving off the agent takes the highlight down rather than leaving it standing
    /// on the last agent that had it.
    #[test]
    fn focusing_a_shell_clears_the_highlight() {
        let mut state = related_agents();
        related(&mut state);
        state.focused = Some(11);
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// `focused` is `None` while a plugin pane holds the keyboard - the hocus sidebar, say.
    /// The highlight is about agents, so it goes away rather than freezing.
    #[test]
    fn a_focused_plugin_clears_the_highlight() {
        let mut state = related_agents();
        related(&mut state);
        state.focused = None;
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// A folder no probe has answered for yet has no checkout to be a sibling of. Saying
    /// nothing beats guessing, and the next probe publishes again.
    #[test]
    fn an_unprobed_folder_highlights_nothing() {
        let mut state = related_agents();
        state.repositories.clear();
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// A folder Git did not claim is not a checkout, and two of them sharing a parent are
    /// not related by anything this feature knows about.
    #[test]
    fn a_folder_outside_a_checkout_highlights_nothing() {
        let mut state = related_agents();
        state.repositories.insert(
            Location::local("/work/hocus/.worktrees/topic"),
            projects::Repository::folder("/work/hocus/.worktrees/topic"),
        );
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// A pane in no tab we know about: "the same tab" means nothing there, so nothing is
    /// highlighted rather than every agent on the same checkout session-wide.
    #[test]
    fn an_agent_in_no_tab_highlights_nothing() {
        let mut state = related_agents();
        if let Some(panes) = state.panes.as_mut() {
            panes[0].tab = None;
        }
        assert_eq!(related(&mut state), Some(vec![]));
    }

    /// The point of the feature: the agent's frame says which checkout it is in,
    /// and the shell in the same folder is left alone.
    #[test]
    fn an_agent_in_a_checkout_is_decorated_with_it() {
        let mut state = worktree_agent();
        assert_eq!(
            prefixes(&mut state),
            vec![(7, Some("hocus:topic".to_owned()))]
        );
    }

    /// The host is not asked twice for an answer it already has. This runs on every
    /// publish, and a publish happens several times a poll.
    #[test]
    fn an_unchanged_checkout_is_not_sent_again() {
        let mut state = worktree_agent();
        prefixes(&mut state);
        assert!(prefixes(&mut state).is_empty());
    }

    /// A folder Git did not claim has nothing to say about it, so the pane keeps
    /// whatever title it has and nothing is sent.
    #[test]
    fn an_agent_outside_a_checkout_is_not_decorated() {
        let mut state = worktree_agent();
        state.repositories.insert(
            Location::local("/work/hocus/.worktrees/topic"),
            projects::Repository::folder("/work/hocus/.worktrees/topic"),
        );
        assert!(prefixes(&mut state).is_empty());
    }

    /// A pane that stops looking like an agent has its decoration taken off: the
    /// checkout was said *about the agent*, and there is no longer one there.
    #[test]
    fn a_pane_that_stops_being_an_agent_is_cleared() {
        let mut state = worktree_agent();
        prefixes(&mut state);
        state.panes = Some(vec![
            pane(7, "zsh", "/work/hocus/.worktrees/topic"),
            pane(8, "zsh", "/work/hocus/.worktrees/topic"),
        ]);
        assert_eq!(prefixes(&mut state), vec![(7, None)]);
        assert!(state.title_prefixes.is_empty());
    }

    /// A pane that has left the snapshot is closed or suppressed. Terminal ids are
    /// recycled, so a clear aimed at one would land on whatever holds the id now —
    /// the entry is simply forgotten instead.
    #[test]
    fn a_closed_pane_is_forgotten_rather_than_cleared() {
        let mut state = worktree_agent();
        prefixes(&mut state);
        state.panes = Some(vec![pane(8, "zsh", "/work/hocus/.worktrees/topic")]);
        assert!(prefixes(&mut state).is_empty());
        assert!(state.title_prefixes.is_empty());
    }

    /// A checkout inside the worktree changes the branch and nothing but a fresh
    /// probe says so. When one lands, the frame follows it.
    #[test]
    fn a_branch_change_is_sent_on() {
        let mut state = worktree_agent();
        prefixes(&mut state);
        state.repositories.insert(
            Location::local("/work/hocus/.worktrees/topic"),
            checkout("/work/hocus/.worktrees/topic", "/work/hocus/.git", "other"),
        );
        assert_eq!(
            prefixes(&mut state),
            vec![(7, Some("hocus:other".to_owned()))]
        );
    }

    /// A pane the user registered by hand is an agent however it looks, so it is
    /// decorated like one.
    #[test]
    fn a_hand_registered_pane_is_decorated() {
        let mut state = worktree_agent();
        state.manual_kinds.insert(8, "claude".to_owned());
        let calls = prefixes(&mut state);
        assert!(
            calls.contains(&(8, Some("hocus:topic".to_owned()))),
            "{calls:?}"
        );
    }

    /// A pane whose events reach the daemon is read from them, not from the record they also
    /// write. The record is complete every time it is set and so has no way to say "keep the
    /// wording you had" — which is exactly what a `Notification` arriving six seconds after the
    /// `Elicitation` for the same prompt needs it to say.
    #[test]
    fn a_pane_update_does_not_overwrite_what_a_pane_told_us_itself() {
        let request = wire::HookEvent {
            message: Some("Which repository?".to_owned()),
            ..hook("Elicitation")
        };
        let notification = wire::HookEvent {
            notification_type: Some("elicitation_dialog".to_owned()),
            message: Some("Select a repository".to_owned()),
            ..hook("Notification")
        };
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            latched: BTreeMap::from([(7, CLAUDE.to_owned())]),
            ..State::default()
        };
        state.on_hook(7, &request);
        assert_eq!(
            state.hooks[&7].waiting_on.as_deref(),
            Some("Which repository?")
        );
        state.on_hook(7, &notification);
        assert_eq!(
            state.hooks[&7].waiting_on.as_deref(),
            Some("Which repository?"),
            "the backstop fills a gap, it does not replace"
        );

        // The manifest now carries the record `agent-report` wrote for that same
        // notification, which names the prompt the other way round.
        let record = match hocus_proto::agent::hook_outcome(CLAUDE, &notification) {
            hocus_proto::agent::HookOutcome::Reported(agent) => agent,
            other => panic!("{other:?}"),
        };
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "claude".to_owned(),
                agent: Some(record),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        assert_eq!(
            state.hooks[&7].waiting_on.as_deref(),
            Some("Which repository?"),
            "a pane we hear from directly is read from its events"
        );
    }

    #[test]
    fn two_silent_adopted_idle_agents_rebuild_their_hook_feeds() {
        let mut state = State::default();
        let panes = ["codex", "claude"]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                let mut record = agents::registered_agent(kind, None);
                record.source = hocus_proto::agent::AgentSource::Reported;
                record.state = hocus_proto::agent::AgentState::Idle;
                record.session_id = Some(format!("{kind}-session"));
                record.run_id = Some(format!("{kind}:transferred-run"));
                PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(i as u32 + 7, false),
                    agent: Some(record),
                    ..PaneInfo::default()
                }
            })
            .collect();
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), panes);
        for _ in 0..3 {
            state.rebuild_panes();
            assert_eq!(state.hooks.len(), 2);
            for id in [7, 8] {
                assert_eq!(state.hooks[&id].phase, agents::HookPhase::Idle);
            }
            assert!(state.event_fed.is_empty(), "no hook event was needed");
        }
        state.manifest.panes.values_mut().next().unwrap()[0]
            .agent
            .as_mut()
            .unwrap()
            .state = hocus_proto::agent::AgentState::Busy;
        state.rebuild_panes();
        assert_eq!(state.hooks[&7].phase, agents::HookPhase::Busy);
        state.manifest.panes.values_mut().next().unwrap()[0]
            .agent
            .as_mut()
            .unwrap()
            .state = hocus_proto::agent::AgentState::Exited;
        state.rebuild_panes();
        assert!(!state.hooks.contains_key(&7));
        assert_eq!(state.hooks[&8].phase, agents::HookPhase::Idle);
    }

    /// An event that builds no feed claims nothing. The daemon can be reloaded mid-session,
    /// and then the first broadcast for a pane arrives before any snapshot has: there is no
    /// pane list to classify against, so no feed is built. Reading that as "this pane is fed
    /// by its events" closes the manifest road behind a feed that does not exist, and the
    /// record the very same command wrote goes unread — for a permission prompt, until the
    /// user answers it, which is the one thing nobody was going to be told about.
    #[test]
    fn an_event_that_builds_no_feed_leaves_the_manifest_road_open() {
        let request = wire::HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
            ..hook("PermissionRequest")
        };
        // No snapshot yet, which is what a reloaded daemon has.
        let mut state = State::default();
        state.on_hook(7, &request);
        assert!(state.hooks.is_empty(), "nothing to build a feed from");
        assert!(
            !state.event_fed.contains(&7),
            "and so nothing claimed for the pipe road"
        );

        // The snapshot arrives carrying the record that same command wrote.
        let record = match hocus_proto::agent::hook_outcome(CLAUDE, &request) {
            hocus_proto::agent::HookOutcome::Reported(agent) => agent,
            other => panic!("{other:?}"),
        };
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "claude".to_owned(),
                agent: Some(record),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        assert_eq!(
            state.hooks[&7].waiting_on.as_deref(),
            Some("Bash rm"),
            "the record is read, so the wait is drawn"
        );
    }

    /// A feed ends where an ending is *observed*, and `SessionEnd` is not the only observer.
    /// An agent killed outright sends nothing at all, and the record moving to `Exited` is
    /// then the one thing that says the session is over. Skipping that for a pane fed by its
    /// events leaves the row on `waiting · …` for the life of the session, with the screen
    /// heuristic disabled behind the feed that will not go.
    /// A run whose pane arrived from another session picks up where it left off.
    ///
    /// The fetch is the daemon's; what is checked here is everything around it, which is where
    /// the work is: the run becomes this pane's live one, it is queued for a write that claims
    /// the row, the pane is told which run it is in so the *next* session can do the same, and
    /// the summary that came inside the run is put back on the pane it now describes.
    #[test]
    fn an_adopted_run_becomes_this_sessions_and_brings_its_summary() {
        let mut state = State {
            zellij_session_name: Some("there".to_owned().into()),
            panes: Some(vec![pane(2, "claude", "/work")]),
            ..State::default()
        };
        let mut run = agents::AgentRun {
            id: "claude:cc-session-1".to_owned(),
            pane_id: 7,
            zellij_session_name: "here".to_owned().into(),
            host_id: HostId::new("host-a"),
            session_id: "cc-session-1".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
            worktree: None,
            started_at_ms: 10,
            ended_at_ms: Some(20),
            outcome: agents::RunOutcome::Closed,
            revision: 1,
            summarized_revision: 1,
            summary: Some(agents::Summary {
                text: "rebasing".to_owned(),
                title: "rebase".to_owned(),
                awaiting: false,
                hash: 0,
                created_at_ms: None,
            }),
            entries: vec![agents::RunEntry {
                revision: 1,
                event_id: None,
                kind: agents::RunEntryKind::Prompt,
                text: "ship it".to_owned(),
                created_at_ms: Some(10),
            }],
        };
        run = run.adopted_by(
            2,
            &agents::rows::Owner::new(HostId::new("host-a"), "there".into()),
        );
        state.adopt_run(2, run.clone());

        let live = state.runs.get(&2).expect("the run is this pane's now");
        assert_eq!(live.entries.len(), 1, "with the history it arrived with");
        assert_eq!(live.started_at_ms, 10);
        assert_eq!(live.outcome, agents::RunOutcome::Active);
        assert_eq!(live.zellij_session_name, "there".into());
        assert!(
            state.dirty_runs.contains_key(&run.id),
            "and queued for the write that claims the row"
        );
        assert!(
            !state.released_runs.contains(&run.id),
            "claimed unguarded: this is the write that changes the owner"
        );
        assert_eq!(
            state.summaries.get(&2).map(|summary| summary.text.as_str()),
            Some("rebasing"),
            "the summary travels inside the run; the row it used to live in was the old pane's"
        );
    }

    /// A pane leaving the manifest ends its run - but as the weaker of the two claims it could
    /// be making. The daemon cannot tell a warp from a `kill -9`, so it writes "if nobody else
    /// has taken this, it is over" and lets the store settle it.
    #[test]
    fn a_pane_that_leaves_releases_its_run_rather_than_closing_it_outright() {
        let mut state = State {
            zellij_session_name: Some("here".to_owned().into()),
            panes: Some(vec![pane(7, "claude", "/work")]),
            latched: BTreeMap::from([(7, CLAUDE.to_owned())]),
            ..State::default()
        };
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "claude".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        state.on_hook(7, &hook("UserPromptSubmit"));
        let run_id = state.runs[&7].id.clone();
        assert_eq!(run_id, "claude:cc-session-1");

        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![]);
        state.rebuild_panes();

        assert!(
            state.runs.is_empty(),
            "the pane is gone, so the run is not live"
        );
        assert!(
            state.released_runs.contains(&run_id),
            "and its write is guarded, in case the pane was warped rather than killed"
        );
        let released = &state.dirty_runs[&run_id];
        assert_eq!(released.outcome, agents::RunOutcome::Closed);
        assert!(released.ended_at_ms.is_some());
    }

    #[test]
    fn a_killed_agent_loses_its_feed_even_though_it_never_said_goodbye() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            latched: BTreeMap::from([(7, CLAUDE.to_owned())]),
            ..State::default()
        };
        let request = wire::HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
            ..hook("PermissionRequest")
        };
        state.on_hook(7, &request);
        assert_eq!(state.hooks[&7].waiting_on.as_deref(), Some("Bash rm"));
        assert!(state.event_fed.contains(&7));

        // `kill -9`: no `SessionEnd`, and the pane's record goes to `Exited` on its own.
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "claude".to_owned(),
                agent: Some(AgentInfo {
                    kind: CLAUDE.to_owned(),
                    state: AgentState::Exited,
                    source: AgentSource::Reported,
                    detail: None,
                    waiting_on: None,
                    wait_needs_a_person: false,
                    cwd: Some("/work".to_owned()),
                    session_id: Some("cc-session-1".to_owned()),
                    run_id: None,
                }),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        assert!(
            state.runs.is_empty(),
            "the exited agent's run is no longer active"
        );
        let archived = state
            .dirty_runs
            .values()
            .find(|run| run.pane_id == 7 && run.ended_at_ms.is_some())
            .expect("the run was archived");
        assert_eq!(archived.outcome, agents::RunOutcome::Closed);
        assert!(
            !state.hooks.contains_key(&7),
            "the feed is over, so the heuristic has the pane back"
        );
        assert!(!state.event_fed.contains(&7));
    }

    /// The other half: a pane that has never piped an event is read off the manifest, which is
    /// what gives a hocus-spawned agent, an `agent-report --state` script and a plugin
    /// speaking for an agent the same row as a pane with the hooks installed.
    #[test]
    fn a_pane_that_reports_some_other_way_is_read_off_the_manifest() {
        let mut state = State::default();
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "claude".to_owned(),
                agent: Some(AgentInfo {
                    kind: CLAUDE.to_owned(),
                    state: AgentState::Waiting,
                    source: AgentSource::Reported,
                    detail: None,
                    waiting_on: Some("approve the plan".to_owned()),
                    wait_needs_a_person: true,
                    cwd: Some("/work".to_owned()),
                    session_id: Some("cc-session-1".to_owned()),
                    run_id: None,
                }),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        let feed = state.hooks.get(&7).expect("a feed off the record");
        assert_eq!(feed.phase, agents::HookPhase::Waiting);
        assert_eq!(feed.waiting_on.as_deref(), Some("approve the plan"));
        assert_eq!(feed.session_id, "cc-session-1");
    }

    #[test]
    fn command_and_cwd_events_survive_the_next_pane_snapshot() {
        let mut state = State::default();
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "shell".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        state.commands.insert(7, "codex --full-auto".to_owned());
        state.cwds.insert(7, "/work/hocus".to_owned());
        state.rebuild_panes();

        let pane = state.panes.as_ref().unwrap().first().unwrap();
        assert_eq!(pane.command.as_deref(), Some("codex --full-auto"));
        assert_eq!(pane.cwd.as_deref(), Some("/work/hocus"));

        state.manifest = PaneManifest::default();
        state.rebuild_panes();
        assert!(state.commands.is_empty());
        assert!(state.cwds.is_empty());
    }

    #[test]
    fn activity_tracks_running_agents_and_prunes_everything_else() {
        let mut state = State {
            panes: Some(vec![pane(7, "codex", "/work"), pane(8, "zsh", "/tmp")]),
            activity: agents::Activity::from([(9, (1, 4))]),
            ..State::default()
        };

        state.observe_activity_with(|_| sample("working"));
        state.observe_activity_with(|_| sample("working"));
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));
        assert_eq!(state.activity.len(), 1);

        state.panes.as_mut().unwrap()[0].status = agents::AgentStatus::Held;
        state.observe_activity_with(|_| sample("ignored"));
        assert!(state.activity.is_empty());
    }

    /// A state holding one agent in one folder, with that folder recorded the
    /// way an event would have recorded it.
    fn agent_in(pane_id: u32, cwd: &str) -> State {
        State {
            panes: Some(vec![pane(pane_id, "codex", cwd)]),
            cwds: BTreeMap::from([(pane_id, cwd.to_owned())]),
            ..State::default()
        }
    }

    /// An agent on another machine, as the manifest and the daemon's maps see
    /// it together: the folder in `cwds`, the machine in `hosts`.
    fn agent_on(pane_id: u32, host: &str, cwd: &str) -> State {
        let mut pane = pane(pane_id, "codex", cwd);
        pane.host = Some(host.to_owned());
        State {
            panes: Some(vec![pane]),
            cwds: BTreeMap::from([(pane_id, cwd.to_owned())]),
            hosts: BTreeMap::from([(pane_id, host.to_owned())]),
            ..State::default()
        }
    }

    /// An agent in a Firecracker VM, as the manifest and the daemon's maps see it
    /// together: the guest folder in `cwds`, the VM in `hosts`, and the host worktree
    /// the VM mirrors in `firecracker_worktrees`.
    fn vm_agent(pane_id: u32, vm: &str, guest_cwd: &str, worktree: Option<&str>) -> State {
        let mut pane = pane(pane_id, "claude", guest_cwd);
        let host = format!("firecracker:{vm}");
        pane.host = Some(host.clone());
        pane.firecracker_worktree = worktree.map(ToOwned::to_owned);
        State {
            panes: Some(vec![pane]),
            cwds: BTreeMap::from([(pane_id, guest_cwd.to_owned())]),
            hosts: BTreeMap::from([(pane_id, host)]),
            firecracker_worktrees: worktree
                .map(|worktree| {
                    BTreeMap::from([(
                        pane_id,
                        VmCheckout {
                            vm: vm.to_owned(),
                            worktree: worktree.to_owned(),
                        },
                    )])
                })
                .unwrap_or_default(),
            ..State::default()
        }
    }

    /// What the far machine's `git rev-parse` writes for a checkout.
    fn probe_bytes(root: &str, branch: &str) -> Vec<u8> {
        format!("{root}\n{root}/.git\n{root}/.git\n{branch}\n").into_bytes()
    }

    fn far_probe(host: &str, cwd: &str, branch: &str) -> PaneGitProbe {
        PaneGitProbe {
            host: host.to_owned(),
            cwd: cwd.to_owned(),
            exit_code: Some(0),
            metadata: probe_bytes(cwd, branch),
            config: Vec::new(),
        }
    }

    // -- remote folders --------------------------------------------------

    /// Running Git here for a pane whose process is elsewhere probes whatever
    /// this machine happens to have at that path — nothing, usually, and
    /// somebody else's checkout at worst. The far machine has already answered;
    /// the daemon only has to ask for it.
    #[test]
    fn a_remote_pane_is_not_probed_with_a_local_git() {
        let state = agent_on(7, "build-box", "/work/hocus");

        assert!(
            state.repository_cwds(false).is_empty(),
            "a folder on another machine is not ours to run git in",
        );
    }

    #[test]
    fn a_remote_folder_is_resolved_from_its_own_machines_probe() {
        let mut state = agent_on(7, "build-box", "/work/hocus");

        state.refresh_remote_repositories_with(|pane_id, _, _| {
            assert_eq!(pane_id, 7);
            Ok(Some(far_probe("build-box", "/work/hocus", "topic")))
        });

        let repository = state
            .repositories
            .get(&remote("build-box", "/work/hocus"))
            .expect("the folder was resolved under its own machine");
        assert_eq!(repository.branch.as_deref(), Some("topic"));
        assert_eq!(
            repository.host.as_deref(),
            Some("build-box"),
            "the checkout carries the machine, so nothing matches it by path",
        );
        assert!(
            !state.repositories.contains_key(&local("/work/hocus")),
            "and nothing was filed as if it were here",
        );
    }

    // -- firecracker folders ---------------------------------------------

    /// The map is filled from the *manifest*, not from the pane list, so a pane hidden
    /// behind the scrollback editor keeps the worktree that identifies its work — and so
    /// keeps the checkout probed for it — for as long as it is hidden. It goes when the
    /// pane leaves the manifest, which is the only thing that means "closed".
    #[test]
    fn a_suppressed_firecracker_pane_keeps_the_worktree_its_vm_mirrors() {
        let mut state = State::default();
        let mut vm = PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(7, false),
            title: "claude".to_owned(),
            host: Some("firecracker:review".to_owned()),
            is_suppressed: true,
            ..PaneInfo::default()
        };
        vm.target =
            PaneTarget::firecracker("review", None, Some(std::path::Path::new("/work/hocus")))
                .expect("a valid VM name and an absolute worktree");
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![vm]);
        state.rebuild_panes();

        assert!(
            state.panes.iter().flatten().next().is_none(),
            "a suppressed pane is not one of the panes on show",
        );
        assert_eq!(state.location_of(7), Some(local("/work/hocus")));

        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![]);
        state.rebuild_panes();
        assert_eq!(state.location_of(7), None);
    }

    /// A pane can leave its VM without leaving the manifest: `RunCommandInPane` re-runs it
    /// in place and it keeps its id. Both halves have to be retracted, and the worktree
    /// especially — `checkout_location` reads it *before* the folder, so a stale one wins
    /// over the folder the pane is really in and pins the frame prefix, the probed
    /// checkout and the run's project to a checkout the pane left.
    #[test]
    fn a_firecracker_pane_re_run_on_this_machine_stops_answering_with_the_vms_worktree() {
        let mut state = State::default();
        let mut vm = PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(7, false),
            title: "claude".to_owned(),
            host: Some("firecracker:review".to_owned()),
            ..PaneInfo::default()
        };
        vm.target =
            PaneTarget::firecracker("review", None, Some(std::path::Path::new("/work/hocus")))
                .expect("a valid VM name and an absolute worktree");
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![vm]);
        state.cwds.insert(7, "/workspace".to_owned());
        state.rebuild_panes();
        assert_eq!(state.location_of(7), Some(local("/work/hocus")));

        // Re-run here: the same pane id, a local target, and no host — which is exactly
        // what the server reports, since it retracts both when the pane is respawned.
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "claude".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.cwds.insert(7, "/other/repo".to_owned());
        state.rebuild_panes();

        assert_eq!(state.location_of(7), Some(local("/other/repo")));
        assert!(state.firecracker_worktrees.is_empty());
        assert!(
            state.hosts.is_empty(),
            "and not left keyed to the machine it is no longer on",
        );
    }

    /// The same departure without leaving Firecracker: a pane re-run into another VM keeps
    /// its id and comes back `Firecracker` with no worktree of its own, which is the shape
    /// of a join in progress. The VM is what tells the two apart — the answer was given for
    /// the VM the pane was in, and this pane is in a different one.
    #[test]
    fn a_firecracker_pane_moved_to_another_vm_stops_answering_with_the_first_vms_worktree() {
        let mut state = State::default();
        let mut vm = PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(7, false),
            title: "claude".to_owned(),
            host: Some("firecracker:review".to_owned()),
            ..PaneInfo::default()
        };
        vm.target =
            PaneTarget::firecracker("review", None, Some(std::path::Path::new("/work/hocus")))
                .expect("a valid VM name and an absolute worktree");
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![vm]);
        state.cwds.insert(7, "/workspace".to_owned());
        state.rebuild_panes();
        assert_eq!(state.location_of(7), Some(local("/work/hocus")));

        let mut moved = PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(7, false),
            title: "claude".to_owned(),
            host: Some("firecracker:dev".to_owned()),
            ..PaneInfo::default()
        };
        moved.target = PaneTarget::firecracker("dev", None, None).expect("a valid VM name");
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![moved]);
        state.rebuild_panes();

        assert!(
            state.firecracker_worktrees.is_empty(),
            "the first VM's worktree is not the second VM's",
        );
        assert_eq!(
            state.location_of(7),
            Some(Location::new(Some("firecracker:dev"), "/workspace")),
            "and the pane falls back to the guest folder it is really in",
        );
    }

    /// What the retraction is keyed on, and why it is not keyed on the worktree being
    /// absent: `new-pane --firecracker review` joins a VM somebody else provisioned and
    /// names no worktree of its own, so such a pane is a Firecracker one whose target says
    /// nothing until the provider answers. A pane still in a VM keeps what it was told,
    /// whatever its own target names — only leaving the VM retracts it.
    #[test]
    fn a_pane_still_in_a_vm_keeps_its_worktree_though_its_target_names_none() {
        let mut state = State::default();
        let mut joining = PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(7, false),
            title: "claude".to_owned(),
            host: Some("firecracker:review".to_owned()),
            ..PaneInfo::default()
        };
        joining.target = PaneTarget::firecracker("review", None, None).expect("a valid VM name");
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![joining]);
        state.firecracker_worktrees.insert(
            7,
            VmCheckout {
                vm: "review".to_owned(),
                worktree: "/work/hocus".to_owned(),
            },
        );
        state.rebuild_panes();

        assert_eq!(state.location_of(7), Some(local("/work/hocus")));
    }

    /// A VM mirrors a folder on this machine, so that folder is what Git is asked
    /// about — the same answer `zellij firecracker list` gives for a VM's branch. The
    /// guest's own repository is a clone whose only remote names the worktree's path,
    /// so nothing here could match it to a project.
    #[test]
    fn a_firecracker_pane_is_probed_at_the_worktree_its_vm_mirrors() {
        let state = vm_agent(7, "review", "/workspace", Some("/work/hocus"));

        assert_eq!(state.repository_cwds(false), vec![local("/work/hocus")]);
    }

    /// And is not asked for the guest's answer as well. Filed under the guest folder it
    /// would be read by nobody, evicted on the next event and re-added on the next
    /// heartbeat — a whole-session snapshot published each time round.
    #[test]
    fn a_firecracker_pane_with_a_worktree_is_never_asked_for_a_remote_probe() {
        let mut state = vm_agent(7, "review", "/workspace", Some("/work/hocus"));

        state.refresh_remote_repositories_with(|pane_id, _, _| {
            panic!("pane {pane_id} was asked for a remote probe");
        });

        assert!(state.repositories.is_empty());
    }

    /// A VM an agent got to itself mirrors nothing here, so it stays on the remote road
    /// — and is answered by the guest, which is the only machine that can answer.
    #[test]
    fn a_firecracker_pane_without_a_worktree_is_answered_by_its_guest() {
        let mut state = vm_agent(7, "review", "/workspace", None);

        assert!(state.repository_cwds(false).is_empty());
        state.refresh_remote_repositories_with(|pane_id, _, _| {
            assert_eq!(pane_id, 7);
            Ok(Some(far_probe("firecracker:review", "/workspace", "topic")))
        });

        assert!(state
            .repositories
            .contains_key(&remote("firecracker:review", "/workspace")));
    }

    /// The worktree is the identity, so it answers before the guest folder has been
    /// reported at all. Without that the folder is probed by `repository_cwds` and
    /// missing from `pane_locations`, which evicts it — twice a `git` process on every
    /// host event, and a frame that never keeps its prefix.
    #[test]
    fn a_firecracker_pane_with_no_guest_folder_yet_is_probed_once() {
        let mut state = vm_agent(7, "review", "/workspace", Some("/work/hocus"));
        state.cwds.clear();

        assert_eq!(state.repository_cwds(false), vec![local("/work/hocus")]);
        state.resolve_repositories();
        assert!(
            state.repository_cwds(false).is_empty(),
            "the probe already in flight answers for the folder",
        );
    }

    /// The point of it all: an agent in a VM wears the checkout its work will be merged
    /// back into, exactly as an agent sitting in that worktree does.
    #[test]
    fn a_firecracker_agent_is_decorated_with_the_checkout_its_vm_mirrors() {
        let mut state = vm_agent(
            7,
            "review",
            "/workspace",
            Some("/work/hocus/.worktrees/topic"),
        );
        state.repositories = BTreeMap::from([(
            Location::local("/work/hocus/.worktrees/topic"),
            checkout("/work/hocus/.worktrees/topic", "/work/hocus/.git", "topic"),
        )]);

        assert_eq!(
            prefixes(&mut state),
            vec![(7, Some("hocus:topic".to_owned()))]
        );
    }

    /// A suppressed pane is hidden, not closed. Its worktree is kept for the reason its
    /// folder is — the probed checkout must survive the scrollback editor being open —
    /// and goes when the pane leaves the manifest.
    #[test]
    fn a_firecracker_panes_worktree_outlives_the_pane_list_and_not_the_manifest() {
        let mut state = vm_agent(7, "review", "/workspace", Some("/work/hocus"));
        state.panes = Some(vec![]);

        assert_eq!(state.location_of(7), Some(local("/work/hocus")));
        assert_eq!(state.pane_locations(), vec![local("/work/hocus")]);

        state.firecracker_worktrees.clear();
        state.cwds.clear();
        assert_eq!(state.location_of(7), None);
    }

    /// The hazard the whole change exists to close: the same path on two
    /// machines is two folders, and one answer must not stand for both.
    #[test]
    fn the_same_path_on_two_machines_is_two_folders() {
        let mut here = pane(7, "codex", "/work/hocus");
        let mut there = pane(8, "codex", "/work/hocus");
        there.host = Some("build-box".to_owned());
        here.host = None;
        let mut state = State {
            panes: Some(vec![here, there]),
            cwds: BTreeMap::from([(7, "/work/hocus".to_owned()), (8, "/work/hocus".to_owned())]),
            hosts: BTreeMap::from([(8, "build-box".to_owned())]),
            ..State::default()
        };

        state.refresh_remote_repositories_with(|_, _, _| {
            Ok(Some(far_probe("build-box", "/work/hocus", "topic")))
        });
        state.record_probe_outcome(
            local("/work/hocus"),
            poll::repository(Some(0), &probe_bytes("/work/hocus", "main"), &[]),
        );

        assert_eq!(
            state.repositories[&local("/work/hocus")].branch.as_deref(),
            Some("main"),
        );
        assert_eq!(
            state.repositories[&remote("build-box", "/work/hocus")]
                .branch
                .as_deref(),
            Some("topic"),
        );
        // The local half is already answered for by now; what matters is that
        // the remote folder was never a candidate for a local `git` at all.
        assert!(
            !state
                .repository_cwds(true)
                .contains(&remote("build-box", "/work/hocus")),
            "a folder on another machine is not ours to run git in",
        );
    }

    /// The far machine probes on its own clock, so a pane that has just moved
    /// reports the folder it came from. Filing that under the new folder would
    /// claim a checkout the pane has left; filed under its own, it is simply a
    /// fact about a folder, and the pane waits one tick for its own answer —
    /// exactly what a local pane does between a `cd` and its next probe.
    #[test]
    fn a_probe_that_describes_the_folder_a_pane_has_left_is_filed_under_that_folder() {
        let mut state = agent_on(7, "build-box", "/work/hocus/deep");

        state.refresh_remote_repositories_with(|_, _, _| {
            Ok(Some(far_probe("build-box", "/work/hocus", "topic")))
        });

        assert!(
            !state
                .repositories
                .contains_key(&remote("build-box", "/work/hocus/deep")),
            "the pane's own folder has not been answered for yet",
        );
        assert!(state
            .repositories
            .contains_key(&remote("build-box", "/work/hocus")));
    }

    /// This runs on every heartbeat, and a snapshot is the whole session pushed
    /// to every subscriber. An unchanged answer is not worth one.
    #[test]
    fn an_unchanged_remote_answer_is_recorded_once() {
        let mut state = agent_on(7, "build-box", "/work/hocus");
        let outcome = || poll::repository(Some(0), &probe_bytes("/work/hocus", "topic"), &[]);

        assert!(
            state.record_probe_outcome(remote("build-box", "/work/hocus"), outcome()),
            "the first answer is news",
        );
        assert!(
            !state.record_probe_outcome(remote("build-box", "/work/hocus"), outcome()),
            "the same answer again is not",
        );
    }

    /// A failed *local* probe waits, because repeating it costs two processes. A
    /// remote ask costs one round trip and fails only for reasons that pass on
    /// their own, so it must not be penalised for half a minute.
    #[test]
    fn a_remote_folder_that_answered_nothing_is_not_put_in_backoff() {
        let mut state = agent_on(7, "build-box", "/work/hocus");

        state.record_probe_outcome(
            remote("build-box", "/work/hocus"),
            poll::ProbeOutcome::Failed,
        );

        assert!(
            state.probe_backoff.is_empty(),
            "the next tick simply asks again",
        );
    }

    /// Each ask is a blocking host round trip, and the rotation is what stops a
    /// session full of remote panes paying for them all at once.
    #[test]
    fn only_so_many_remote_panes_are_asked_on_one_tick() {
        let count = REMOTE_PROBES_PER_TICK as u32 + 3;
        let panes: Vec<agents::AgentPane> = (0..count)
            .map(|id| {
                let mut pane = pane(id, "codex", &format!("/work/{id}"));
                pane.host = Some("build-box".to_owned());
                pane
            })
            .collect();
        let mut state = State {
            panes: Some(panes),
            cwds: (0..count).map(|id| (id, format!("/work/{id}"))).collect(),
            hosts: (0..count).map(|id| (id, "build-box".to_owned())).collect(),
            ..State::default()
        };

        let mut asked: Vec<u32> = Vec::new();
        state.refresh_remote_repositories_with(|pane_id, _, _| {
            asked.push(pane_id);
            Ok(None)
        });
        assert_eq!(asked.len(), REMOTE_PROBES_PER_TICK);

        // The next tick resumes where this one stopped, so every pane is
        // reached rather than the first few being asked forever.
        let mut next: Vec<u32> = Vec::new();
        state.refresh_remote_repositories_with(|pane_id, _, _| {
            next.push(pane_id);
            Ok(None)
        });
        assert!(
            next.iter().any(|pane_id| !asked.contains(pane_id)),
            "the rotation did not move: {asked:?} then {next:?}",
        );
    }

    #[test]
    fn arrangement_refresh_waits_for_all_remote_attempts_and_local_probes() {
        for local_completion in [0, 1, 2] {
            for missing in [false, true] {
                let count = 20;
                let mut state = State {
                    panes: Some(
                        (0..count)
                            .map(|id| {
                                let mut pane = pane(id, "codex", &format!("/work/{id}"));
                                pane.host = Some("build-box".into());
                                pane
                            })
                            .collect(),
                    ),
                    cwds: (0..count).map(|id| (id, format!("/work/{id}"))).collect(),
                    hosts: (0..count).map(|id| (id, "build-box".into())).collect(),
                    ..State::default()
                };
                // Seed unchanged metadata, then prime the normal snapshot deduplication.
                if !missing {
                    for id in 0..count {
                        let probe = far_probe("build-box", &format!("/work/{id}"), "main");
                        state.record_probe_outcome(
                            remote("build-box", &probe.cwd),
                            poll::repository(probe.exit_code, &probe.metadata, &probe.config),
                        );
                    }
                }
                state.publish();
                if local_completion != 0 {
                    state
                        .git_probes
                        .insert(local("/local"), GitProbe::default());
                }
                state.arrangement_subscribers = vec![PluginId::new(10), PluginId::new(11)];
                state
                    .arrangement_remote_pending
                    .extend(state.remote_pane_ids());
                let mut asked = BTreeSet::new();
                let mut probe = |id, _: &str, _: Option<&str>| {
                    asked.insert(id);
                    if missing {
                        Err("unavailable".into())
                    } else {
                        Ok(Some(far_probe("build-box", &format!("/work/{id}"), "main")))
                    }
                };
                state.refresh_remote_repositories_with(&mut probe);
                assert_eq!(
                    state.arrangement_remote_pending.len(),
                    count as usize - REMOTE_PROBES_PER_TICK
                );
                state.publish_forced();
                assert_eq!(state.arrangement_subscribers.len(), 2, "no early delivery");
                if local_completion == 1 {
                    state.git_probes.clear();
                    state.publish();
                    assert_eq!(state.arrangement_subscribers.len(), 2);
                }
                for _ in 1..(count as usize).div_ceil(REMOTE_PROBES_PER_TICK) {
                    assert_eq!(state.arrangement_subscribers.len(), 2, "no early delivery");
                    state.refresh_remote_repositories_with(&mut probe);
                }
                assert!(state.arrangement_remote_pending.is_empty());
                assert_eq!(asked.len(), count as usize);
                if local_completion == 2 {
                    assert_eq!(state.arrangement_subscribers.len(), 2);
                    state.git_probes.clear();
                    let mut deliveries = vec![];
                    state.publish_with(false, |message| {
                        deliveries.push(message.destination_plugin_id.unwrap())
                    });
                    state.publish_with(true, |message| {
                        deliveries.push(message.destination_plugin_id.unwrap())
                    });
                    assert_eq!(deliveries, vec![PluginId::new(10), PluginId::new(11)]);
                }
                assert!(
                    state.arrangement_subscribers.is_empty(),
                    "each subscriber consumed once"
                );
                state.publish_forced();
                assert!(
                    state.arrangement_subscribers.is_empty(),
                    "no repeat delivery"
                );
            }
        }
    }

    #[test]
    fn arrangement_refresh_waits_for_the_reported_directory_and_non_checkout() {
        let mut state = agent_on(7, "build-box", "/process");
        state.panes.as_mut().unwrap()[0].agent = Some(AgentInfo {
            kind: "codex".into(),
            state: AgentState::Busy,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: Some("/agent".into()),
            session_id: None,
            run_id: None,
        });
        state.arrangement_subscribers.push(PluginId::new(10));
        state.arrangement_started_at = Some(now_ms());
        state.arrangement_remote_pending.insert(7);
        state.refresh_remote_repositories_with(|id, cwd, _| {
            assert_eq!((id, cwd), (7, "/agent"));
            Ok(None)
        });
        assert!(!state.arrangement_subscribers.is_empty());
        state.refresh_remote_repositories_with(|_, _, _| {
            Ok(Some(far_probe("build-box", "/process", "main")))
        });
        assert!(state.arrangement_remote_pending.contains(&7));
        assert!(!state
            .repositories
            .contains_key(&remote("build-box", "/agent")));
        state.panes.as_mut().unwrap()[0].agent.as_mut().unwrap().cwd = Some("/changed".into());
        state.refresh_remote_repositories_with(|_, cwd, _| {
            assert_eq!(cwd, "/changed");
            let mut probe = far_probe("build-box", cwd, "main");
            probe.exit_code = Some(128);
            probe.metadata.clear();
            Ok(Some(probe))
        });
        assert!(
            state.arrangement_subscribers.is_empty(),
            "a matching non-checkout completes the wait"
        );
        assert!(!state.repositories[&remote("build-box", "/changed")].is_checkout());
        state.panes.as_mut().unwrap()[0].agent.as_mut().unwrap().cwd = Some(String::new());
        state.refresh_remote_repositories_with(|_, cwd, _| {
            assert_eq!(cwd, "/process", "empty agent cwd falls back to process cwd");
            Ok(None)
        });
    }

    #[test]
    fn arrangement_excludes_remote_errors_and_timeouts_despite_cached_identity() {
        for failure in ["error", "invalid", "timeout"] {
            let mut state = agent_on(7, "build-box", "/work");
            let old = far_probe("build-box", "/work", "old-branch");
            state.record_probe_outcome(
                remote("build-box", "/work"),
                poll::repository(old.exit_code, &old.metadata, &old.config),
            );
            state.arrangement_subscribers.push(PluginId::new(10));
            state.arrangement_started_at = Some(now_ms());
            state.arrangement_refresh_token = Some("new-arrangement".into());
            state.arrangement_remote_pending.insert(7);
            // Hold delivery so this test can inspect the final payload.
            state
                .git_probes
                .insert(local("/local"), GitProbe::default());
            for _ in 0..2 {
                state.refresh_remote_repositories_with(|_, _, token| {
                    assert_eq!(token, Some("new-arrangement"));
                    Ok(None)
                });
                assert!(state.arrangement_remote_pending.contains(&7));
            }
            state.refresh_remote_repositories_with(|_, _, token| {
                assert_eq!(token, Some("new-arrangement"));
                match failure {
                    "error" => Err("disconnected".into()),
                    "invalid" => Ok(Some(PaneGitProbe {
                        host: "build-box".into(),
                        cwd: "/work".into(),
                        exit_code: None,
                        metadata: vec![],
                        config: vec![],
                    })),
                    _ => Ok(None),
                }
            });
            assert!(
                state
                    .repositories
                    .contains_key(&remote("build-box", "/work")),
                "the old cache exists for background readers"
            );
            state.git_probes.clear();
            if failure == "timeout" {
                state.arrangement_started_at = Some(now_ms() - 15_000);
            }
            let mut delivered = vec![];
            state.publish_with(false, |message| {
                if message.destination_plugin_id == Some(PluginId::new(10)) {
                    delivered.push(
                        serde_json::from_str::<PollReply>(
                            message.message_payload.as_deref().unwrap(),
                        )
                        .unwrap(),
                    );
                }
            });
            assert_eq!(delivered.len(), 1);
            assert!(
                delivered[0].panes.as_ref().unwrap().is_empty(),
                "{failure} must not classify a remote pane from the old cache"
            );
        }
    }

    #[test]
    fn arrangement_refresh_wait_is_bounded() {
        let mut state = agent_on(7, "build-box", "/work");
        state.arrangement_subscribers.push(PluginId::new(10));
        state.arrangement_remote_pending.insert(7);
        state.arrangement_started_at = Some(now_ms());
        state.refresh_remote_repositories_with(|_, _, _| Ok(None));
        assert!(!state.arrangement_subscribers.is_empty());
        state.arrangement_started_at = Some(now_ms() - 15_000);
        let mut delivered = vec![];
        state.publish_with(false, |message| {
            delivered.push(message.destination_plugin_id)
        });
        assert_eq!(delivered, vec![Some(PluginId::new(10))]);
        assert!(state.arrangement_remote_pending.is_empty());
    }

    #[test]
    fn arrangement_refresh_forgets_closed_and_ineligible_remote_panes() {
        let mut state = agent_on(7, "build-box", "/work/hocus");
        state.arrangement_subscribers.push(PluginId::new(10));
        state.arrangement_remote_pending.extend([7, 8]);
        state.panes.as_mut().unwrap()[0].host = None;
        state.refresh_remote_repositories_with(|id, _, _| {
            panic!("ineligible pane {id} was queried")
        });
        assert!(state.arrangement_remote_pending.is_empty());
        assert!(state.arrangement_subscribers.is_empty());
    }

    /// A local pane has no cached probe and the host says so; asking would spend
    /// a round trip to be told what the pane already says.
    #[test]
    fn a_local_pane_is_never_asked_for_a_remote_probe() {
        let mut state = agent_in(7, "/work");

        state.refresh_remote_repositories_with(|pane_id, _, _| {
            panic!("asked the host about local pane {pane_id}");
        });
    }

    /// A remote pane's folder is evicted like any other when its pane goes.
    #[test]
    fn a_remote_folder_is_forgotten_when_its_pane_closes() {
        let mut state = agent_on(7, "build-box", "/work/hocus");
        state.repositories.insert(
            remote("build-box", "/work/hocus"),
            projects::Repository::folder("/work/hocus"),
        );

        state.panes = Some(Vec::new());
        state.cwds.clear();
        state.hosts.clear();
        state.resolve_repositories();

        assert!(state.repositories.is_empty(), "{:?}", state.repositories);
    }

    #[test]
    fn repository_refresh_reprobes_cached_cwds_without_overlapping() {
        let mut state = agent_in(7, "/work");
        state
            .repositories
            .insert(local("/work"), projects::Repository::folder("/work"));

        assert!(state.repository_cwds(false).is_empty());
        assert_eq!(state.repository_cwds(true), vec![local("/work")]);
        state.git_probes.insert(local("/work"), GitProbe::default());
        assert!(state.repository_cwds(true).is_empty());
    }

    /// The window that made a nested worktree show the branch above it: a pane
    /// that has not latched yet was in `cwds` but not in `agent_cwds`, so its
    /// folder was never asked about, and hocus answered for it with the nearest
    /// probed folder above — the primary checkout, on the primary's branch.
    #[test]
    fn a_folder_is_probed_before_its_pane_looks_like_an_agent() {
        let mut state = State {
            panes: Some(vec![pane(7, "zsh", "/work")]),
            cwds: BTreeMap::from([(7, "/work".to_owned())]),
            ..State::default()
        };
        assert!(
            agents::classify(&state.panes.as_ref().unwrap()[0], &state.manual_kinds).is_none(),
            "nothing says this pane runs an agent yet",
        );

        assert_eq!(state.repository_cwds(false), vec![local("/work")]);

        // And it is still asked only once.
        state
            .repositories
            .insert(local("/work"), projects::Repository::folder("/work"));
        assert!(state.repository_cwds(false).is_empty());
    }

    /// A worktree nested inside its own checkout is a different folder, and gets
    /// its own question. Answering for it with the primary's entry is the whole
    /// bug.
    #[test]
    fn a_nested_worktree_is_probed_apart_from_the_checkout_it_sits_in() {
        let worktree = "/work/hocus/.claude/worktrees/topic";
        let state = State {
            panes: Some(vec![
                pane(7, "codex", "/work/hocus"),
                pane(8, "codex", worktree),
            ]),
            cwds: BTreeMap::from([(7, "/work/hocus".to_owned()), (8, worktree.to_owned())]),
            ..State::default()
        };

        assert_eq!(
            state.repository_cwds(false),
            vec![local("/work/hocus"), local(worktree)]
        );
    }

    /// Two processes per folder, on a pass that runs on nearly every host event.
    #[test]
    fn only_so_many_folders_are_probed_on_one_pass() {
        let count = GIT_PROBES_PER_PASS as u32 + 3;
        let mut state = State {
            panes: Some(
                (0..count)
                    .map(|id| pane(id, "zsh", &format!("/work/{id}")))
                    .collect(),
            ),
            cwds: (0..count).map(|id| (id, format!("/work/{id}"))).collect(),
            ..State::default()
        };

        state.resolve_repositories();
        assert_eq!(state.git_probes.len(), GIT_PROBES_PER_PASS);
        state.retry_repositories(ACTIVITY_SECONDS);
        assert_eq!(state.git_probes.len(), count as usize);
        state.retry_repositories(ACTIVITY_SECONDS);
        assert_eq!(state.git_probe_seq, count as u64, "no overlapping probes");
    }

    /// The cap is spent on the folders a header is drawn from first.
    #[test]
    fn an_agents_folder_is_asked_about_before_a_shells() {
        let mut panes: Vec<agents::AgentPane> = (0..GIT_PROBES_PER_PASS as u32)
            .map(|id| pane(id, "zsh", &format!("/shell/{id}")))
            .collect();
        panes.push(pane(99, "codex", "/work/hocus"));
        let mut cwds: BTreeMap<u32, String> = (0..GIT_PROBES_PER_PASS as u32)
            .map(|id| (id, format!("/shell/{id}")))
            .collect();
        cwds.insert(99, "/work/hocus".to_owned());
        let state = State {
            panes: Some(panes),
            cwds,
            ..State::default()
        };

        let asked = state.repository_cwds(false);
        assert_eq!(asked.len(), GIT_PROBES_PER_PASS);
        assert_eq!(
            asked.first().cloned(),
            Some(local("/work/hocus")),
            "the agent's folder is not crowded out by the shells: {asked:?}",
        );
    }

    /// `classify` reads the foreground command, which flickers. It used to take
    /// the probed checkout with it and cancel the probe in flight, so the agent
    /// lost its project until two more round trips landed.
    #[test]
    fn a_cached_repository_survives_a_classification_flicker() {
        let mut state = agent_in(7, "/work");
        state
            .repositories
            .insert(local("/work"), projects::Repository::folder("/work"));
        state.git_probes.insert(local("/work"), GitProbe::default());

        let pane = &mut state.panes.as_mut().unwrap()[0];
        pane.command = None;
        pane.title = "shell".to_owned();
        assert!(
            agents::classify(&state.panes.as_ref().unwrap()[0], &state.manual_kinds).is_none(),
            "the pane really has stopped looking like an agent"
        );
        state.resolve_repositories();

        assert!(state.repositories.contains_key(&local("/work")));
        assert!(state.git_probes.contains_key(&local("/work")));
    }

    /// The same guarantee, entered through the event handler rather than
    /// through `resolve_repositories` directly. Focusing a pane makes it draw,
    /// which sets the pty's activity flag, which makes the ticker report what
    /// the pane is running — so a focus change really does run this path, and it
    /// must not cost the pane the branch already probed for its folder.
    #[test]
    fn a_command_change_on_the_focused_pane_keeps_its_repository() {
        let mut state = ran_codex(7);
        state.record_cwd(7, "/work");
        state
            .repositories
            .insert(local("/work"), projects::Repository::folder("/work"));
        state.git_probes.insert(local("/work"), GitProbe::default());

        state.on_command(7, vec!["zsh".to_owned()], false);

        assert!(
            state.pending_release.contains(&7),
            "the release is still proposed",
        );
        assert!(state.repositories.contains_key(&local("/work")));
        assert!(
            state.git_probes.contains_key(&local("/work")),
            "and the probe in flight was not cancelled either",
        );
    }

    /// The eviction that is meant to happen: a pane really did move, so what was
    /// learned about where it used to be is no longer about this pane.
    #[test]
    fn a_pane_that_changes_folder_gives_up_the_old_one_and_probes_the_new() {
        let mut state = agent_in(7, "/work/a");
        state
            .repositories
            .insert(local("/work/a"), projects::Repository::folder("/work/a"));

        state.record_cwd(7, "/work/b");
        state.rejoin_cwds();
        state.resolve_repositories();

        assert!(!state.repositories.contains_key(&local("/work/a")));
        assert!(
            state.git_probes.contains_key(&local("/work/b")),
            "the new folder was asked about in the same breath",
        );
    }

    /// The folder is still forgotten — just on the pane going away rather than
    /// on it momentarily not looking like an agent.
    #[test]
    fn a_repository_is_forgotten_when_its_pane_closes() {
        let mut state = agent_in(7, "/work");
        state
            .repositories
            .insert(local("/work"), projects::Repository::folder("/work"));

        state.panes = Some(vec![]);
        state.cwds.clear();
        state.resolve_repositories();

        assert!(!state.repositories.contains_key(&local("/work")));
    }

    /// The regression: a snapshot that says nothing about a folder has not said
    /// the pane left it. Clearing it here is what left a pane with no folder,
    /// and so with no project, for good.
    #[test]
    fn a_manifest_without_a_cwd_does_not_clobber_a_known_one() {
        let mut state = State::default();
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "codex".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.record_cwd(7, "/work/hocus");
        state.rebuild_panes();

        assert_eq!(
            state.panes.as_ref().unwrap()[0].cwd.as_deref(),
            Some("/work/hocus")
        );
    }

    /// One folder must give one key, whichever way it arrived, or the probed
    /// repositories and the project roots are keyed apart.
    #[test]
    fn a_folder_is_keyed_the_same_however_it_arrives() {
        let mut state = State::default();
        state.record_cwd(7, "/work/hocus/");
        assert_eq!(state.cwds[&7], "/work/hocus");
        state.record_cwd(8, "/work//hocus");
        assert_eq!(state.cwds[&8], "/work/hocus");
    }

    #[test]
    fn a_pane_with_no_folder_is_asked_directly() {
        let mut state = State {
            panes: Some(vec![agents::AgentPane {
                cwd: None,
                ..pane(7, "codex", "/unused")
            }]),
            ..State::default()
        };

        state.repair_cwds_with(|_| Some("/work/hocus/".to_owned()));

        assert_eq!(state.cwds[&7], "/work/hocus");
        assert_eq!(
            state.panes.as_ref().unwrap()[0].cwd.as_deref(),
            Some("/work/hocus")
        );
    }

    /// A state holding one pane the daemon has seen running codex, built the way
    /// the events would have built it.
    fn ran_codex(pane_id: u32) -> State {
        let mut state = State {
            panes: Some(vec![pane(pane_id, "codex", "/work")]),
            ..State::default()
        };
        state.on_command(pane_id, vec!["codex".to_owned()], true);
        assert_eq!(
            state.latched.get(&pane_id).map(String::as_str),
            Some("codex"),
            "the pane really did latch",
        );
        state
    }

    fn kind_of(state: &State, pane_id: u32) -> Option<&str> {
        let pane = state
            .panes
            .as_ref()?
            .iter()
            .find(|pane| pane.pane_id == pane_id)?;
        agents::classify(pane, &state.manual_kinds).map(|(kind, _)| kind)
    }

    /// The flicker: an agent that shells out to a tool is reported as running
    /// that tool, and used to drop out of every list until the tool finished.
    #[test]
    fn an_agent_that_shells_out_stays_an_agent() {
        let mut state = ran_codex(7);

        state.on_command(7, vec!["git".to_owned(), "status".to_owned()], true);

        assert_eq!(kind_of(&state, 7), Some("codex"));
        assert!(
            state.pending_release.is_empty(),
            "something in the foreground is the agent working, not the agent leaving",
        );
    }

    /// The idle counter is the other casualty: pruning it on a momentary
    /// misclassification reset a long-idle agent to `busy` every time it moved.
    #[test]
    fn a_shell_out_does_not_reset_the_idle_count() {
        let mut state = ran_codex(7);
        state.observe_activity_with(|_| sample("same"));
        state.observe_activity_with(|_| sample("same"));
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));

        state.on_command(7, vec!["git".to_owned(), "status".to_owned()], true);
        state.observe_activity_with(|_| sample("same"));

        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(2));
    }

    /// Nothing in the foreground and no agent left in the command is the agent
    /// having finished — but a `tcgetpgrp` that momentarily answers nothing looks
    /// identical, so it is proposed and confirmed rather than acted on.
    #[test]
    fn a_pane_back_at_its_prompt_gives_up_its_latch_once_confirmed() {
        let mut state = ran_codex(7);

        state.on_command(7, vec!["zsh".to_owned()], false);
        assert!(state.pending_release.contains(&7), "proposed");
        assert_eq!(kind_of(&state, 7), Some("codex"), "but not yet acted on");

        state.repair_commands_with(|_| Some("zsh".to_owned()));

        assert!(state.pending_release.is_empty());
        assert_eq!(kind_of(&state, 7), None, "confirmed, so the latch goes");
    }

    /// The reason the confirmation exists: one bad read must not cost an agent
    /// its place in the list.
    #[test]
    fn a_momentary_bad_read_does_not_cost_a_latch() {
        let mut state = ran_codex(7);
        state.on_command(7, vec!["zsh".to_owned()], false);

        state.repair_commands_with(|_| Some("codex".to_owned()));

        assert!(state.pending_release.is_empty());
        assert_eq!(kind_of(&state, 7), Some("codex"));
    }

    /// A host that answered nothing has said nothing, and must not settle the
    /// question either way.
    #[test]
    fn an_unanswered_confirmation_leaves_the_release_pending() {
        let mut state = ran_codex(7);
        state.on_command(7, vec!["zsh".to_owned()], false);

        state.repair_commands_with(|_| None);

        assert!(state.pending_release.contains(&7));
        assert_eq!(kind_of(&state, 7), Some("codex"));
    }

    /// A pane launched *as* a command is its own foreground process, so the host
    /// reports its argv with the flag false. Reading that as "back at a prompt"
    /// would unclassify every agent started from a layout.
    #[test]
    fn a_command_pane_is_not_mistaken_for_a_prompt() {
        let mut state = State {
            panes: Some(vec![pane(7, "codex", "/work")]),
            ..State::default()
        };

        state.on_command(7, vec!["codex".to_owned()], false);

        assert_eq!(kind_of(&state, 7), Some("codex"));
        assert!(state.pending_release.is_empty());
    }

    /// Only a command earns a latch. A title is free text the last program left
    /// behind, and remembering it would make a pane an agent for good.
    #[test]
    fn a_title_alone_never_earns_a_latch() {
        let mut state = State {
            panes: Some(vec![agents::AgentPane {
                title: "claude".to_owned(),
                command: None,
                ..pane(7, "claude", "/work")
            }]),
            ..State::default()
        };

        state.settle_latches();

        assert!(state.latched.is_empty());
    }

    #[test]
    fn a_latch_goes_when_its_pane_closes() {
        let mut state = ran_codex(7);

        state.rebuild_panes();

        assert!(state.latched.is_empty(), "no manifest, so no pane");
    }

    /// A pane standing behind the scrollback editor is hidden, not closed. It
    /// keeps its id and its process, and used to lose its folder, the branch
    /// probed from that folder, and its latch — so an agent vanished from the
    /// list and came back under the wrong branch header when it returned.
    #[test]
    fn a_suppressed_pane_keeps_its_folder_and_its_latch() {
        let mut state = ran_codex(7);
        state.record_cwd(7, "/work");
        state
            .repositories
            .insert(local("/work"), projects::Repository::folder("/work"));
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "codex".to_owned(),
                is_suppressed: true,
                ..PaneInfo::default()
            }],
        );

        state.rebuild_panes();

        assert_eq!(state.cwds.get(&7).map(String::as_str), Some("/work"));
        assert_eq!(state.latched.get(&7).map(String::as_str), Some("codex"));
        assert!(
            state.repositories.contains_key(&local("/work")),
            "and the branch probed from it was not thrown away",
        );

        // Back in front, and still the same process: it has not re-run, so it
        // does not have to earn its latch again.
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "codex".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        assert_eq!(state.latched.get(&7).map(String::as_str), Some("codex"));

        // A pane that really closes still takes all of it with it.
        state.manifest = PaneManifest::default();
        state.rebuild_panes();
        assert!(state.cwds.is_empty());
        assert!(state.latched.is_empty());
        assert!(!state.repositories.contains_key(&local("/work")));
    }

    /// A re-run pane is a new process and has to earn its latch again, or a
    /// restarted shell would inherit the dead agent's name.
    #[test]
    fn a_rerun_pane_earns_its_latch_again() {
        let mut state = ran_codex(7);
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "shell".to_owned(),
                exited: true,
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        assert_eq!(
            state.latched.get(&7).map(String::as_str),
            Some("codex"),
            "a dead agent keeps its row, so `agent_control restart` can reach it",
        );

        // Re-run: the same pane id, running again, and `commands` went with the
        // manifest's last word rather than the agent's.
        state.commands.insert(7, "zsh".to_owned());
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "shell".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();

        assert!(state.latched.is_empty());
    }

    /// A re-run pane starts a fresh process with nothing to do with what the old one was
    /// summarised as doing — keeping the old activity hash or summary around would show a
    /// stale answer the moment the new run pauses once.
    #[test]
    fn a_rerun_pane_drops_its_old_activity_and_summary() {
        let mut state = ran_codex(7);
        state.activity.insert(7, (1, 3));
        state.summaries.insert(
            7,
            agents::Summary {
                text: "did a thing".to_owned(),
                title: "a thing".to_owned(),
                awaiting: true,
                hash: 1,
                created_at_ms: None,
            },
        );
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "shell".to_owned(),
                exited: true,
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();

        // Re-run: the same pane id, running again.
        state.commands.insert(7, "zsh".to_owned());
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "shell".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();

        assert!(!state.activity.contains_key(&7), "stale hash dropped");
        assert!(!state.summaries.contains_key(&7), "stale summary dropped");
    }

    /// The same sequence the integration test drives, with the manifest events
    /// that only show up in a real session: a pane appears, runs an agent, then
    /// shells out, with a `PaneUpdate` landing at every step.
    #[test]
    fn a_latch_survives_the_pane_updates_that_arrive_around_it() {
        let mut state = State::default();
        let manifest = |id: u32| {
            let mut manifest = PaneManifest::default();
            manifest.panes.insert(
                zellij_tile::prelude::TabPosition::new(0),
                vec![PaneInfo {
                    id: zellij_tile::prelude::PaneId::from_legacy(id, false),
                    title: "Pane #1".to_owned(),
                    ..PaneInfo::default()
                }],
            );
            manifest
        };

        state.manifest = manifest(0);
        state.rebuild_panes();
        state.on_command(0, vec!["claude".to_owned()], true);
        assert_eq!(kind_of(&state, 0), Some("claude"));

        state.manifest = manifest(0);
        state.rebuild_panes();
        assert_eq!(kind_of(&state, 0), Some("claude"), "after a pane update");

        state.on_command(0, vec!["git".to_owned(), "status".to_owned()], true);
        assert_eq!(kind_of(&state, 0), Some("claude"), "after shelling out");

        state.manifest = manifest(0);
        state.rebuild_panes();
        assert_eq!(
            kind_of(&state, 0),
            Some("claude"),
            "and after the pane update that follows it",
        );
    }

    /// The gap `CommandChanged` cannot close: its ticker only looks at panes that
    /// produced output, so an agent sitting silent at its prompt is never asked
    /// what it is running, and stays a pane that runs nothing.
    #[test]
    fn a_quiet_pane_is_asked_what_it_is_running() {
        let mut state = State {
            panes: Some(vec![agents::AgentPane {
                command: None,
                title: "shell".to_owned(),
                ..pane(7, "shell", "/work")
            }]),
            ..State::default()
        };
        assert!(
            agents::classify(&state.panes.as_ref().unwrap()[0], &state.manual_kinds).is_none(),
            "nothing has said this pane runs an agent",
        );

        state.repair_commands_with(|_| Some("codex --resume".to_owned()));

        assert_eq!(state.commands[&7], "codex --resume");
        assert_eq!(
            agents::classify(&state.panes.as_ref().unwrap()[0], &state.manual_kinds),
            Some(("codex", "auto")),
        );
    }

    /// A host call that answered nothing has not said the pane stopped running
    /// what it was running.
    #[test]
    fn an_unanswered_command_ask_leaves_the_last_one_alone() {
        let mut state = State {
            panes: Some(vec![pane(7, "codex", "/work")]),
            commands: BTreeMap::from([(7, "codex".to_owned())]),
            ..State::default()
        };

        state.repair_commands_with(|_| None);

        assert_eq!(state.commands[&7], "codex");
    }

    /// Every pane gets revisited, but not all on one tick: a command that is
    /// right now can be wrong later, so the budget goes on a rotation.
    #[test]
    fn command_asks_are_capped_and_rotate_over_the_panes() {
        let panes = (0..COMMAND_REPAIRS_PER_TICK as u32 * 2)
            .map(|id| pane(id, "codex", "/work"))
            .collect();
        let mut state = State {
            panes: Some(panes),
            commands: (0..COMMAND_REPAIRS_PER_TICK as u32 * 2)
                .map(|id| (id, "codex".to_owned()))
                .collect(),
            ..State::default()
        };

        let mut asked = Vec::new();
        state.repair_commands_with(|pane_id| {
            asked.push(pane_id);
            None
        });
        assert_eq!(asked.len(), COMMAND_REPAIRS_PER_TICK, "capped");

        let mut asked_next = Vec::new();
        state.repair_commands_with(|pane_id| {
            asked_next.push(pane_id);
            None
        });
        assert!(
            asked_next.iter().all(|pane_id| !asked.contains(pane_id)),
            "the next tick moves on: {asked:?} then {asked_next:?}",
        );
    }

    /// Each ask is a blocking round-trip, and a pane without a folder now will
    /// still be without one on the next heartbeat.
    #[test]
    fn only_so_many_folders_are_asked_for_on_one_heartbeat() {
        let panes = (0..CWD_REPAIRS_PER_TICK as u32 + 3)
            .map(|id| agents::AgentPane {
                cwd: None,
                ..pane(id, "codex", "/unused")
            })
            .collect();
        let mut state = State {
            panes: Some(panes),
            ..State::default()
        };

        let mut asked = 0;
        state.repair_cwds_with(|_| {
            asked += 1;
            Some("/work".to_owned())
        });

        assert_eq!(asked, CWD_REPAIRS_PER_TICK);
    }

    #[test]
    fn bootstrap_is_retried_until_the_session_answers() {
        let mut state = State::default();
        assert!(state.needs_bootstrap());
        state.bootstrapped = true;
        assert!(!state.needs_bootstrap());
    }

    /// The startup scan is the only thing that reads a *live* running command for
    /// panes that predate the daemon. It used to be cancelled by anything that
    /// filled `panes` — and a `PaneUpdate` arrives long before the first
    /// heartbeat, so a daemon that loaded before the session was ready never
    /// scanned at all, and every pane already running an agent stayed a plain
    /// shell for the life of the session.
    #[test]
    fn a_pane_update_does_not_cancel_the_bootstrap_retry() {
        let mut state = State::default();
        state.manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(7, false),
                title: "shell".to_owned(),
                ..PaneInfo::default()
            }],
        );

        state.rebuild_panes();

        assert!(state.panes.is_some(), "the manifest still lands");
        assert!(state.needs_bootstrap(), "but the scan is still owed");
    }

    /// Nothing is cached for a probe that answered nothing, so only the wait
    /// keeps `resolve_repositories` — which runs on nearly every event — from
    /// re-running a missing `git` forever.
    #[test]
    fn a_probe_that_answered_nothing_is_left_alone_for_a_while() {
        let mut state = agent_in(7, "/work");
        let context = BTreeMap::from([
            ("cwd".to_owned(), "/work".to_owned()),
            ("git_probe_id".to_owned(), "1".to_owned()),
        ]);
        state.git_probes.insert(
            local("/work"),
            GitProbe {
                id: 1,
                ..GitProbe::default()
            },
        );
        let mut metadata = context.clone();
        metadata.insert("git_probe".to_owned(), "metadata".to_owned());
        let mut config = context;
        config.insert("git_probe".to_owned(), "config".to_owned());

        state.finish_git_probe(Some(0), Vec::new(), config);
        state.finish_git_probe(None, Vec::new(), metadata);

        assert!(
            !state.repositories.contains_key(&local("/work")),
            "no guess cached"
        );
        assert!(state.probe_backoff.contains_key(&local("/work")));
        assert!(state.repository_cwds(false).is_empty(), "not asked again");

        state.retry_repositories(GIT_RETRY_SECONDS - ACTIVITY_SECONDS);
        assert!(state.git_probes.is_empty(), "still in backoff");
        state.retry_repositories(ACTIVITY_SECONDS);
        assert!(state.probe_backoff.is_empty());
        assert!(state.git_probes.contains_key(&local("/work")));
    }

    // -- summaries ----------------------------------------------------------

    fn endpoint() -> Endpoint {
        Endpoint {
            url: "http://localhost:11434/v1/chat/completions".to_owned(),
            api_key: "sk-test".to_owned(),
            model: "test-model".to_owned(),
        }
    }

    /// One poll of every pane, showing `text`. Nothing is bought from a poll any more; what
    /// it keeps in step is the activity map, which is what a request is stamped against.
    fn poll_showing(state: &mut State, text: &str) {
        poll_reading(state, sample(text))
    }

    /// One poll of every pane from an arbitrary reading, for the tests about readings that
    /// are worth nothing — a scrolled pane, or a picture that moved with the pty silent.
    fn poll_reading(state: &mut State, reading: poll::PaneSample) {
        let mut reading = Some(reading);
        state.observe_activity_with(|_| reading.take().expect("one pane per poll in these tests"));
    }

    /// One screen-only agent showing `text`, with the summarise key already pressed against
    /// it, and the requests that went out.
    ///
    /// Polled first so the pane has an activity hash: `summarize_pane_now_with` refuses a
    /// pane the poller has not reached, because there would be nothing to stamp the answer
    /// with. The request that goes out is the daemon's first, so its correlation id is 1.
    fn idle_agent(text: &str) -> (State, Vec<(u32, Vec<u8>)>) {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        poll_showing(&mut state, text);
        let (_, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(text));
        (
            state,
            sent.into_iter()
                .map(|asked| (asked.pane_id, asked.body))
                .collect(),
        )
    }

    /// A reading of a pane showing its live tail, with the process having just written to
    /// it: the ordinary case, and what every test means unless it says otherwise.
    fn sample(text: &str) -> poll::PaneSample {
        poll::PaneSample {
            text: text.to_owned(),
            readable: true,
            is_scrolled: false,
            last_output_ms_ago: Some(zellij_tile::prelude::ElapsedMillis::new(0)),
        }
    }

    /// A reading of a pane nothing has written to for a long while — what a resize's reflow
    /// or a viewport that moved on its own looks like.
    fn quiet(text: &str) -> poll::PaneSample {
        poll::PaneSample {
            last_output_ms_ago: Some(zellij_tile::prelude::ElapsedMillis::new(600_000)),
            ..sample(text)
        }
    }

    /// A reading of a pane the user has scrolled back off its tail.
    fn scrolled(text: &str) -> poll::PaneSample {
        poll::PaneSample {
            is_scrolled: true,
            ..sample(text)
        }
    }

    /// A reading the host refused — what a pane that has closed under us answers with, and
    /// what `poll::read_pane` makes of it: an error message where the screen would be.
    fn unreadable() -> poll::PaneSample {
        poll::PaneSample {
            readable: false,
            last_output_ms_ago: None,
            ..sample("could not read pane: Pane Terminal(7) not found")
        }
    }

    /// One manifest for one pane at one size, for the tests about resizing.
    fn sized_manifest(pane_id: u32, rows: usize, columns: usize) -> PaneManifest {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            zellij_tile::prelude::TabPosition::new(0),
            vec![PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(pane_id, false),
                title: "claude".to_owned(),
                pane_content_rows: zellij_tile::prelude::RowCount::new(rows),
                pane_content_columns: zellij_tile::prelude::ColumnCount::new(columns),
                ..PaneInfo::default()
            }],
        );
        manifest
    }

    /// The pane text a test agent shows - long enough to clear `worth_summarizing`.
    const BUSY_PANE: &str = "Running the test suite and waiting for it to finish now...";
    const OTHER_PANE: &str = "Something else entirely happened on this pane instead...";
    /// What an agent was showing before the screen a test settles on, so the pane is seen
    /// working at least once - see `idle_agent`.
    const EARLIER_PANE: &str = "Reading the files it was asked about, one after another...";

    /// A model's answer, carrying the title every real answer is asked for.
    fn reply(summary: &str, awaiting: bool) -> Vec<u8> {
        titled_reply(summary, awaiting, TITLE)
    }

    /// The title in every [`reply`], and what a pane is expected to be renamed to.
    const TITLE: &str = "running the tests";

    /// An answer with `title` in it, for the tests that are about the rename. An empty
    /// `title` stands for the key being useless rather than absent; `parse_summary`
    /// treats the two the same.
    fn titled_reply(summary: &str, awaiting: bool, title: &str) -> Vec<u8> {
        format!(
            r#"{{"choices":[{{"message":{{"content":"{{\"summary\":\"{summary}\",\"awaiting\":{awaiting},\"title\":\"{title}\"}}"}}}}]}}"#
        )
        .into_bytes()
    }

    /// What a [`finish`] would have asked the host to do.
    #[derive(Default)]
    struct Finished {
        renames: Vec<(u32, String)>,
        toasts: Vec<(u32, String)>,
    }

    /// `finish_summary` with the host calls it would have made handed back instead. Every
    /// test goes through this: the host is not there to be called.
    fn finish(
        state: &mut State,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> Finished {
        let mut finished = Finished::default();
        state.finish_summary_with(
            status,
            body,
            context,
            |pane_id, title| finished.renames.push((pane_id, title.to_owned())),
            |pane_id, toast| finished.toasts.push((pane_id, toast.to_owned())),
        );
        finished
    }

    fn context(pane_id: u32, id: u64) -> BTreeMap<String, String> {
        BTreeMap::from([
            (SUMMARY_PANE.to_owned(), pane_id.to_string()),
            (SUMMARY_ID.to_owned(), id.to_string()),
        ])
    }

    // -----------------------------------------------------------------------
    // What an agent says about itself
    // -----------------------------------------------------------------------

    /// One hook payload, as the generated script forwards it.
    fn hook(name: &str) -> wire::HookEvent {
        wire::HookEvent {
            hook_event_name: name.to_owned(),
            session_id: "cc-session-1".to_owned(),
            source: Some("startup".to_owned()),
            ..Default::default()
        }
    }

    /// The label the agent strip would draw for a pane, which is what all of this is
    /// ultimately for.
    fn cell(state: &State, pane_id: u32) -> String {
        let pane = state
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
            .expect("the pane is in the snapshot");
        agents::status_cell(
            pane.status,
            state.activity.get(&pane_id),
            state.hooks.get(&pane_id),
        )
    }

    /// The point of the whole feed: a `Stop` names the pane idle on the message that
    /// carries it. The heuristic beside it has just watched the screen move and would
    /// have called the same moment `busy`, then spent three more polls agreeing.
    #[test]
    fn a_stop_hook_lands_the_idle_label_without_waiting_for_the_screen_to_settle() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        poll_showing(&mut state, EARLIER_PANE);
        poll_showing(&mut state, BUSY_PANE);
        assert_eq!(cell(&state, 7), "busy", "the screen is moving");

        state.on_hook(7, &hook("Stop"));

        assert_eq!(cell(&state, 7), "idle");
    }

    #[test]
    fn compacting_a_session_does_not_mark_a_working_agent_idle() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(7, &hook("UserPromptSubmit"));
        let seq = state.seq;

        state.on_hook(
            7,
            &wire::HookEvent {
                source: Some("compact".to_owned()),
                ..hook("SessionStart")
            },
        );

        assert_eq!(cell(&state, 7), "busy");
        assert_eq!(state.seq, seq, "an ignored event publishes nothing");
    }

    /// The question a turn is about names the pane it was typed in, the moment it is asked.
    ///
    /// Before this a pane got a name only at the end of the turn, from the model's summary
    /// title — so for the whole time an agent was actually working the tab bar said `claude`.
    #[test]
    fn a_prompt_names_the_pane_it_was_typed_in() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };

        let named = feed_naming(&mut state, 7, &prompt("  fix the flaky\n reconnect test  "));

        assert_eq!(named, vec![(7, "fix the flaky reconnect test".to_owned())]);
        // And the prompt is still kept for the summary made at the end of the turn: naming
        // the pane is a second reader of it, not a replacement for the first.
        assert_eq!(
            state.prompts.get(&7).map(String::as_str),
            Some("fix the flaky\n reconnect test")
        );
    }

    /// A notification of the shape Claude Code submits when a background task wakes an agent.
    fn task_notification() -> String {
        [
            "<task-notification>",
            "<task-id>a10fe836c6a9fa4ca</task-id>",
            "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
            "<status>completed</status>",
            "<summary>Agent \"/code-review\" finished</summary>",
            "<note>A task-notification fires each time this agent stops.",
            "The same task-id may notify more than once.</note>",
            "<result>The review found four issues.</result>",
            "</task-notification>",
        ]
        .join("\n")
    }

    /// A pane woken by a background task is named for the task, and the question kept for its
    /// summary is a sentence. Before this the tab bar read `<task-notification> <task-id>a10f…`
    /// for the whole of the turn, and the model was handed six hundred characters of opening
    /// tags and a temp path as the question the turn was about.
    #[test]
    fn a_pane_woken_by_a_task_is_named_and_questioned_in_words() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };

        let named = feed_naming(&mut state, 7, &prompt(&task_notification()));

        let title = &named.first().expect("a rename").1;
        assert!(
            title.starts_with("task · Agent \"/code-review\""),
            "{title}"
        );
        assert!(!title.contains('<'), "{title}");

        let kept = state.prompts.get(&7).expect("a question").as_str();
        assert!(
            kept.starts_with("background task a10fe836 completed:"),
            "{kept}"
        );
        assert!(kept.contains("The review found four issues."), "{kept}");
        assert!(!kept.contains('<'), "{kept}");
        // The note is boilerplate addressed to the agent, and the path is one the model cannot
        // read. Neither is worth any of the six hundred characters.
        assert!(!kept.contains("notify more than once"), "{kept}");
        assert!(!kept.contains("/private/tmp"), "{kept}");
    }

    /// The other reader of the same reduction. Both go through `envelope::question`, so the two
    /// inputs to one summary cannot disagree about what the turn was for.
    #[test]
    fn a_run_delta_carries_a_task_notification_as_a_sentence() {
        let mut run = agents::AgentRun {
            id: "claude:s".to_owned(),
            pane_id: 7,
            zellij_session_name: "here".to_owned().into(),
            host_id: HostId::new("host-a"),
            session_id: "s".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: None,
            host: None,
            worktree: None,
            started_at_ms: 0,
            ended_at_ms: None,
            outcome: agents::RunOutcome::Active,
            revision: 1,
            summarized_revision: 0,
            summary: None,
            entries: Vec::new(),
        };
        run.entries.push(agents::RunEntry {
            revision: 1,
            event_id: None,
            kind: agents::RunEntryKind::Prompt,
            text: task_notification(),
            created_at_ms: Some(10),
        });

        let delta = run_delta(&run).expect("there is a prompt to describe");

        assert!(
            delta.starts_with("prompt: background task a10fe836 completed:"),
            "{delta}"
        );
        assert!(!delta.contains('<'), "{delta}");

        // And a prompt somebody typed is carried exactly as it was, which is every prompt this
        // path saw before the reduction existed.
        run.entries[0].text = "fix the flaky reconnect test".to_owned();
        assert_eq!(
            run_delta(&run).expect("a delta"),
            "prompt: fix the flaky reconnect test"
        );
    }

    /// A `UserPromptSubmit` with nothing in it says nothing to name a pane after, and
    /// clearing a name it did not write is not this path's business.
    #[test]
    fn a_prompt_with_nothing_in_it_leaves_the_panes_name_alone() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };

        assert!(feed_naming(&mut state, 7, &prompt("   \n\t ")).is_empty());
        assert!(feed_naming(&mut state, 7, &hook("UserPromptSubmit")).is_empty());
        // And no other event names a pane either: the turn's end has the summary title,
        // which `finish_summary_with` writes with its own reading of what is worth saying.
        assert!(feed_naming(&mut state, 7, &hook("Stop")).is_empty());
    }

    /// The same gate every other half of `on_hook_with` sits behind: an event from a pane
    /// the snapshot does not call an agent is not acted on at all.
    #[test]
    fn a_prompt_from_a_pane_that_is_not_an_agent_names_nothing() {
        let mut state = State {
            panes: Some(vec![pane(7, "zsh", "/work")]),
            ..State::default()
        };

        assert!(feed_naming(&mut state, 7, &prompt("fix the test")).is_empty());
    }

    /// `title_is_ours_to_take`, on the road that made it worth widening. A pane recognised
    /// only by what its title says would stop being an agent the moment a prompt was written
    /// over it — but a pane the session holds a *record* for is anchored where a rename
    /// cannot reach, and every hook-connected pane has one.
    #[test]
    fn a_prompt_takes_the_name_of_a_pane_whose_record_anchors_it() {
        let unanchored = agents::AgentPane {
            command: Some("ssh box".to_owned()),
            ..pane(7, "claude", "/work")
        };
        let mut state = State {
            panes: Some(vec![unanchored.clone()]),
            ..State::default()
        };
        assert!(
            feed_naming(&mut state, 7, &prompt("fix the test")).is_empty(),
            "a pane known only by its title keeps the title that makes it an agent"
        );

        let mut state = State {
            panes: Some(vec![agents::AgentPane {
                agent: Some(agents::registered_agent(CLAUDE, None)),
                ..unanchored
            }]),
            ..State::default()
        };
        assert_eq!(
            feed_naming(&mut state, 7, &prompt("fix the test")),
            vec![(7, "fix the test".to_owned())]
        );
    }

    /// A pane that is not a `claude`, said by the broadcast rather than guessed at.
    ///
    /// The payload is the agent's own JSON and names no agent, so without the `kind=`
    /// argument this daemon would write `claude` over the record `agent-report` had just
    /// written correctly, and latch the pane as one besides. Codex's question is an ordinary
    /// tool call, which is why the wait below needs no rule of its own here.
    #[test]
    fn a_hook_that_names_its_agent_is_latched_and_recorded_as_that_agent() {
        let mut state = State {
            panes: Some(vec![pane(7, "codex", "/work")]),
            ..State::default()
        };

        state.on_hook_of(
            7,
            "codex",
            &wire::HookEvent {
                tool_name: Some("request_user_input".to_owned()),
                ..hook("PreToolUse")
            },
        );

        assert_eq!(state.latched.get(&7).map(String::as_str), Some("codex"));
        assert_eq!(state.hooks[&7].phase, agents::HookPhase::Waiting);
        assert_eq!(state.hooks[&7].waiting_on.as_deref(), Some("a question"));
        assert_eq!(cell(&state, 7), "waiting · a question");
    }

    #[test]
    fn a_hook_only_latches_a_pane_the_snapshot_already_calls_an_agent() {
        let mut state = State {
            panes: Some(vec![pane(7, "zsh", "/work")]),
            ..State::default()
        };

        state.on_hook(99, &hook("Stop"));
        state.on_hook(7, &hook("Stop"));

        assert!(state.latched.is_empty());
        assert!(state.hooks.is_empty());
        assert_eq!(state.seq, 0);
    }

    #[test]
    fn an_unchanged_world_is_not_republished() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.publish();
        assert_eq!(state.seq, 1, "the first snapshot goes out");

        state.publish();
        assert_eq!(state.seq, 1, "nothing moved, so nothing is sent");

        state.update(Event::PaneUpdate(sized_manifest(7, 24, 80)));
        let seq = state.seq;
        state.update(Event::PaneUpdate(sized_manifest(7, 24, 80)));
        assert_eq!(
            state.seq, seq,
            "a PaneUpdate that changes nothing a subscriber sees publishes nothing"
        );
    }

    #[test]
    fn a_new_subscriber_gets_the_unchanged_snapshot_anyway() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.publish();
        let seq = state.seq;

        // What AGENT_SUBSCRIBE runs: the world is unchanged, but the asker has
        // no snapshot at all, so it goes out (and to everyone, renumbered).
        state.publish_forced();
        assert_eq!(state.seq, seq + 1);
    }

    #[test]
    fn a_real_change_after_a_withheld_publish_still_goes_out() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.publish();
        state.publish();
        let seq = state.seq;

        state.manual_kinds.insert(7, "codex".to_owned());
        state.publish();
        assert_eq!(state.seq, seq + 1);
    }

    /// The daemon has no opinion about tabs; it is only the road between the
    /// plugin that makes these assignments and the one that names the tabs.
    #[test]
    fn a_hand_made_tab_assignment_is_carried_through_to_the_snapshot() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            tab_projects: BTreeMap::from([(TabId::new(4), "hocus".to_owned())]),
            ..State::default()
        };

        state.publish_forced();

        let published: PollReply =
            serde_json::from_str(state.last_published.as_deref().expect("published"))
                .expect("parses");
        assert_eq!(
            published.tab_projects,
            BTreeMap::from([(TabId::new(4), "hocus".to_owned())])
        );
    }

    #[test]
    fn an_identical_hook_does_not_publish_the_same_snapshot_twice() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let event = hook("UserPromptSubmit");
        state.on_hook(7, &event);
        let seq = state.seq;

        state.on_hook(7, &event);
        assert_eq!(state.seq, seq);

        state.on_hook(7, &hook("PermissionRequest"));
        assert_eq!(state.seq, seq + 1, "a changed phase still publishes");
    }

    /// The other half: blocked on the user, said outright rather than guessed at from a
    /// screenful of text. `PermissionRequest` fires as the prompt goes up, where the
    /// notification that used to be the only signal waits six seconds for silence.
    #[test]
    fn a_permission_request_marks_the_pane_waiting_at_once() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(7, &hook("UserPromptSubmit"));
        assert_eq!(cell(&state, 7), "busy");

        state.on_hook(7, &hook("PermissionRequest"));
        assert_eq!(cell(&state, 7), "waiting");

        // Approved, and back to work.
        state.on_hook(7, &hook("PostToolUse"));
        assert_eq!(cell(&state, 7), "busy");
    }

    /// Which tool, in the row itself. A pane that says only `busy` for ten minutes of
    /// `cargo build` is telling the truth and saying nothing.
    #[test]
    fn a_tool_starting_names_itself_in_the_row() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(
            7,
            &wire::HookEvent {
                hook_event_name: "PreToolUse".to_owned(),
                tool_name: Some("Bash".to_owned()),
                tool_input: Some(serde_json::json!({"command": "cargo x test"})),
                ..Default::default()
            },
        );
        assert_eq!(cell(&state, 7), "busy · Bash cargo");

        state.on_hook(7, &hook("PostToolUse"));
        assert_eq!(cell(&state, 7), "busy", "the tool finished, the hint goes");
    }

    /// A hook fired in a pane is that pane demonstrably running Claude Code. That is
    /// better evidence than the foreground command, which names `git` the moment the
    /// agent shells out - and used to drop the pane out of every list while it did.
    #[test]
    fn a_hook_keeps_a_pane_classified_while_its_agent_shells_out() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(7, &hook("UserPromptSubmit"));

        // The agent runs git, so the foreground command stops naming an agent.
        state.on_command(7, vec!["git".to_owned(), "status".to_owned()], true);

        let pane = state.panes.iter().flatten().next().expect("the pane");
        assert_eq!(pane.detected.as_deref(), Some("claude"));
        assert!(
            agents::classify(pane, &BTreeMap::new()).is_some(),
            "still an agent: {pane:?}"
        );
    }

    /// The session ending is the feed ending. The pane may well live on as a shell
    /// prompt, and from there it is the heuristic's again.
    #[test]
    fn a_session_ending_hands_the_pane_back_to_the_heuristic() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(7, &hook("Stop"));
        assert!(state.hooks.contains_key(&7));

        // Through `feed`, because a session ending now buys a summary, and that reaches
        // the host.
        feed(&mut state, 7, &hook(agents::HOOK_SESSION_END), None);
        assert!(state.hooks.is_empty());
        assert_eq!(
            cell(&state, 7),
            "running",
            "no feed and nothing observed yet"
        );
    }

    /// The one summary the daemon buys of its own accord. A `Stop` buys nothing - a turn
    /// ending is a fragment of a session, and a sentence about one is not what anybody
    /// reads - and the session ending buys exactly one, made from the whole run.
    #[test]
    fn a_session_that_ends_is_summarised_once_from_the_whole_run() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let (_, sent) = feed(
            &mut state,
            7,
            &prompt("fix the retry backoff"),
            Some(&endpoint()),
        );
        assert!(sent.is_empty(), "a prompt buys nothing");
        let (_, sent) = feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        assert!(sent.is_empty(), "and neither does a turn ending");
        let (_, sent) = feed(
            &mut state,
            7,
            &stop_saying("And the second turn finished too."),
            Some(&endpoint()),
        );
        assert!(sent.is_empty(), "however many of them there are");

        let run_id = state.runs[&7].id.clone();
        let (_, sent) = feed(
            &mut state,
            7,
            &hook(agents::HOOK_SESSION_END),
            Some(&endpoint()),
        );

        assert_eq!(sent.len(), 1, "one session, one summary");
        assert_eq!(sent[0].0, run_id, "correlated by run, not by pane");
        // Every turn of the session is in it, and so is what the user asked for: the
        // request is built from the run rather than from the turn that happened to end.
        let body = asked_about(&sent[0].1);
        assert!(body.contains("fix the retry backoff"), "{body}");
        assert!(body.contains("I ran the test suite"), "{body}");
        assert!(body.contains("And the second turn finished too."), "{body}");

        state.finish_run_summary(
            200,
            &reply("Fixed the backoff and ran the tests.", false),
            &run_context(&run_id, 1),
        );
        assert_eq!(
            state.dirty_runs[&run_id]
                .summary
                .as_ref()
                .expect("the run kept it")
                .text,
            "Fixed the backoff and ran the tests."
        );
        assert!(
            !state.summaries.contains_key(&7),
            "and nothing lands on the pane, which is a shell prompt by now"
        );
    }

    /// A session whose every hook was a tool call has nothing anybody would want a sentence
    /// about, and asking a model to describe nothing costs a round trip to be told so.
    #[test]
    fn a_session_that_ends_with_nothing_to_say_asks_nobody() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(
            &mut state,
            7,
            &tool_hook("PreToolUse", "Bash", serde_json::json!({"command": "ls"})),
            Some(&endpoint()),
        );

        let (_, sent) = feed(
            &mut state,
            7,
            &hook(agents::HOOK_SESSION_END),
            Some(&endpoint()),
        );
        assert!(sent.is_empty());
    }

    /// A picture that moved while the pty stayed silent moved for some reason other than
    /// the process - a reflow, a repaint - and is not the agent working. The idle clock is
    /// what still reads this, so it must not be reset by one.
    #[test]
    fn a_screen_that_moved_with_the_pty_silent_is_not_work() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        for _ in 0..6 {
            poll_showing(&mut state, BUSY_PANE);
        }
        let settled = cell(&state, 7);

        poll_reading(&mut state, quiet(OTHER_PANE));

        assert_eq!(cell(&state, 7), settled, "a silent repaint stopped nothing");
    }

    /// A pane with no hook feed at all is never summarised on its own. It is still polled -
    /// that is what the idle clock is made of - but nothing about a screen holding still
    /// buys a request any more.
    #[test]
    fn a_screen_only_pane_that_settles_is_never_asked_about() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        poll_showing(&mut state, EARLIER_PANE);
        for _ in 0..20 {
            poll_showing(&mut state, BUSY_PANE);
        }

        assert!(state.summary_requests.is_empty(), "nobody was asked");
        assert!(state.summaries.is_empty());
        assert!(
            cell(&state, 7).starts_with("idle"),
            "but the clock still runs"
        );
    }

    /// An agent told to work somewhere else moved without the shell under it hearing,
    /// so the hook is the only thing that ever says where the agent actually is.
    #[test]
    fn a_hook_reports_the_folder_the_agent_moved_to() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(
            7,
            &wire::HookEvent {
                cwd: Some("/work/.worktrees/topic".to_owned()),
                ..hook("SessionStart")
            },
        );
        assert_eq!(
            state.cwds.get(&7).map(String::as_str),
            Some("/work/.worktrees/topic")
        );
    }

    /// The model is still asked what the screen says - that is what the toast and the
    /// pane's new name are made of - but it is no longer asked whether the user is being
    /// waited on. Two answers to one question is a disagreement waiting to be drawn.
    #[test]
    fn the_feed_overrules_what_the_model_made_of_the_screen() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        state.on_hook(7, &hook("Stop"));

        finish(
            &mut state,
            200,
            &reply("Waiting on you.", true),
            &context(7, 1),
        );

        let summary = state.summaries.get(&7).expect("a summary was stored");
        assert_eq!(summary.text, "Waiting on you.");
        assert!(
            !summary.awaiting,
            "the agent said it finished; the model only thought otherwise"
        );
    }

    /// And the other way round, which is the case that matters: a permission prompt the
    /// model read as a completed task.
    #[test]
    fn the_feed_marks_a_pane_waiting_that_the_model_called_finished() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        state.on_hook(7, &hook("PermissionRequest"));

        finish(&mut state, 200, &reply("All done.", false), &context(7, 1));

        assert!(state.summaries.get(&7).expect("stored").awaiting);
    }

    /// What the host would have been told about: the toasts by pane, and the requests by
    /// the run they describe.
    type Reported = (Vec<(u32, String)>, Vec<(String, Vec<u8>)>);

    /// `on_hook` with both host-facing halves handed back: what it would have toasted,
    /// and what a `SessionEnd` would have sent. Every hook test that cares about either
    /// goes through this — the host is not there to be called.
    fn feed(
        state: &mut State,
        pane_id: u32,
        event: &wire::HookEvent,
        endpoint: Option<&Endpoint>,
    ) -> Reported {
        let mut toasts = Vec::new();
        let mut sent = Vec::new();
        state.on_hook_with(
            pane_id,
            CLAUDE,
            event,
            |pane_id, text| toasts.push((pane_id, text.to_owned())),
            // Dropped: the renames are what `feed_naming` is for, and every caller here is
            // asking about the toasts or the request instead.
            |_, _| {},
        );
        drain_asks(state, endpoint, &mut sent);
        (toasts, sent)
    }

    /// Drain whatever the pass just closed, with the request handed back rather than sent.
    ///
    /// The daemon's `drain_finished_runs` reaches the network; this is the same drain with the
    /// host taken out, so a test can see which runs were asked about and with what body.
    fn drain_asks(
        state: &mut State,
        endpoint: Option<&Endpoint>,
        sent: &mut Vec<(String, Vec<u8>)>,
    ) {
        state.drain_finished_runs(|state, run_id| {
            state.summarize_run_now_with(run_id, endpoint, true, |_, _, _, body, context| {
                sent.push((context[SUMMARY_RUN].clone(), body));
            });
        });
    }

    /// `on_hook` with the rename handed back instead: what the pane would have been named,
    /// which for a `UserPromptSubmit` is the question the turn is about.
    fn feed_naming(state: &mut State, pane_id: u32, event: &wire::HookEvent) -> Vec<(u32, String)> {
        let mut named = Vec::new();
        state.on_hook_with(
            pane_id,
            CLAUDE,
            event,
            |_, _| {},
            |pane_id, title| named.push((pane_id, title.to_owned())),
        );
        // Nothing drained: these tests are about the name the prompt writes on the way in, and
        // a run left queued costs them nothing.
        named
    }

    /// A hook event with a tool named, and its arguments.
    fn tool_hook(name: &str, tool: &str, input: serde_json::Value) -> wire::HookEvent {
        wire::HookEvent {
            tool_name: Some(tool.to_owned()),
            tool_input: Some(input),
            ..hook(name)
        }
    }

    /// A `Stop` carrying the agent's own last words, which is what a summary is made of.
    fn stop_saying(message: &str) -> wire::HookEvent {
        wire::HookEvent {
            last_assistant_message: Some(message.to_owned()),
            ..hook("Stop")
        }
    }

    /// The other half of a turn: what the user asked for, which is kept both as the
    /// question a summary is written against and as an entry on the run.
    fn prompt(text: &str) -> wire::HookEvent {
        wire::HookEvent {
            prompt: Some(text.to_owned()),
            ..hook("UserPromptSubmit")
        }
    }

    /// Long enough to be worth drawing, and recognisable in an assertion.
    const REPLY: &str = "I ran the test suite and every test passes. Two snapshots were \
                         regenerated on the way; both diffs are whitespace only.";

    /// The interruption the feed exists to make in time. The `!` in a sidebar was only
    /// ever half the answer: a permission prompt lands in the milliseconds the pipe took,
    /// and the sidebar is very often not what is being looked at.
    #[test]
    fn an_agent_that_starts_waiting_says_so_over_whatever_pane_the_user_is_in() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let (toasts, _) = feed(&mut state, 7, &hook("UserPromptSubmit"), None);
        assert!(toasts.is_empty(), "work is not an interruption");

        let (toasts, _) = feed(
            &mut state,
            7,
            &tool_hook(
                "PermissionRequest",
                "Bash",
                serde_json::json!({"command": "rm -rf target"}),
            ),
            None,
        );
        assert_eq!(
            toasts,
            vec![(7, "agent waiting · 7 · claude◆\nBash rm".to_owned())],
            "one toast, naming the pane and what it is asking for"
        );
        assert_eq!(cell(&state, 7), "waiting · Bash rm");
    }

    /// `PermissionRequest` puts the prompt up and the `Notification` for that same prompt
    /// follows about six seconds later. An agent blocked once must interrupt once.
    #[test]
    fn one_prompt_reported_twice_interrupts_only_once() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let (first, _) = feed(
            &mut state,
            7,
            &tool_hook("PermissionRequest", "Bash", serde_json::json!({})),
            None,
        );
        assert_eq!(first.len(), 1);

        let backstop = wire::HookEvent {
            notification_type: Some("permission_prompt".to_owned()),
            message: Some("Claude needs your permission to use Bash".to_owned()),
            ..hook("Notification")
        };
        let (second, _) = feed(&mut state, 7, &backstop, None);
        assert!(
            second.is_empty(),
            "the backstop for a prompt already announced says nothing"
        );
        assert_eq!(
            cell(&state, 7),
            "waiting · Bash",
            "the row keeps the request's own account of the prompt, not the backstop's"
        );
    }

    /// The backstop is a backstop: for a prompt nothing else reported it is the only
    /// thing that says so, and it must still fill that gap.
    #[test]
    fn a_notification_still_reports_a_wait_nothing_else_did() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let only = wire::HookEvent {
            notification_type: Some("elicitation_dialog".to_owned()),
            message: Some("Pick a repository".to_owned()),
            ..hook("Notification")
        };
        let (toasts, _) = feed(&mut state, 7, &only, None);
        assert_eq!(
            toasts,
            vec![(
                7,
                "agent waiting · 7 · claude◆\nPick a repository".to_owned()
            )]
        );
        assert_eq!(cell(&state, 7), "waiting · Pick a repository");
    }

    /// A second question does interrupt again. Nothing is lost by keying the edge on the
    /// phase alone: between two genuine questions the agent *works* — it acts on the first
    /// answer, and the `PostToolUse` for that is a `busy` — so a real second question
    /// crosses the edge on its own.
    #[test]
    fn a_second_question_interrupts_again() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(
            &mut state,
            7,
            &tool_hook("PermissionRequest", "Bash", serde_json::json!({})),
            None,
        );
        // Allowed, and the tool it was asking about runs.
        feed(&mut state, 7, &hook("PostToolUse"), None);
        let (toasts, _) = feed(
            &mut state,
            7,
            &tool_hook(
                "PreToolUse",
                "AskUserQuestion",
                serde_json::json!({"questions": [{"question": "Which one?"}]}),
            ),
            None,
        );
        assert_eq!(
            toasts,
            vec![(7, "agent waiting · 7 · claude◆\nWhich one?".to_owned())]
        );
    }

    /// Not over the very pane it describes: the user is looking at the question. The same
    /// rule the summary toast keeps.
    #[test]
    fn a_question_in_the_pane_the_user_is_watching_is_not_toasted() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            focused: Some(7),
            ..State::default()
        };
        let (toasts, _) = feed(
            &mut state,
            7,
            &tool_hook("PermissionRequest", "Bash", serde_json::json!({})),
            None,
        );
        assert!(toasts.is_empty(), "no toast over the pane itself");
        // The row still says it, which is the half that is not an interruption.
        assert_eq!(cell(&state, 7), "waiting · Bash");
    }

    /// Automatic approval stays busy through reminders, and a direct question still waits.
    #[test]
    fn automatic_permissions_stay_busy_until_a_person_is_asked() {
        for (kind, question) in [
            ("claude", "AskUserQuestion"),
            ("codex", "request_user_input"),
        ] {
            let feed =
                |state: &mut State, pane_id, event: &wire::HookEvent, _: Option<&Endpoint>| {
                    let mut toasts = Vec::new();
                    state.on_hook_with(
                        pane_id,
                        kind,
                        event,
                        |pane_id, text| toasts.push((pane_id, text.to_owned())),
                        |_, _| {},
                    );
                    (toasts, ())
                };
            for (mode, tool) in [
                ("auto", "Bash"),
                ("bypassPermissions", "Bash"),
                ("dontAsk", "Bash"),
                ("acceptEdits", "Edit"),
            ] {
                let mut state = State {
                    panes: Some(vec![pane(7, kind, "/work")]),
                    ..State::default()
                };
                let request = wire::HookEvent {
                    permission_mode: Some(mode.to_owned()),
                    ..tool_hook(
                        "PermissionRequest",
                        tool,
                        serde_json::json!({"command": "git status", "file_path": "/work/main.rs"}),
                    )
                };
                let (toasts, _) = feed(&mut state, 7, &request, None);
                assert!(toasts.is_empty(), "{kind} {mode}");
                let expected = if tool == "Bash" {
                    "busy · Bash git"
                } else {
                    "busy · Edit main.rs"
                };
                assert_eq!(cell(&state, 7), expected);
                assert_eq!(state.hooks[&7].waiting_on, None);
                for notification in ["permission_prompt", "worker_permission_prompt"] {
                    let reminder = wire::HookEvent {
                        notification_type: Some(notification.to_owned()),
                        ..hook("Notification")
                    };
                    let (toasts, _) = feed(&mut state, 7, &reminder, None);
                    assert!(toasts.is_empty());
                    assert_eq!(cell(&state, 7), expected);
                }
                feed(&mut state, 7, &hook("PostToolUse"), None);
                assert_eq!(cell(&state, 7), "busy");
                feed(&mut state, 7, &request, None);
                let asking = wire::HookEvent {
                    permission_mode: Some(mode.to_owned()),
                    ..tool_hook(
                        "PreToolUse",
                        question,
                        serde_json::json!({"questions": [{"question": "Which crate?"}]}),
                    )
                };
                let (toasts, _) = feed(&mut state, 7, &asking, None);
                assert_eq!(toasts.len(), 1, "{kind} {mode}");
                assert_eq!(cell(&state, 7), "waiting · Which crate?");
                feed(&mut state, 7, &hook("PostToolUse"), None);
                assert_eq!(cell(&state, 7), "busy");
                assert_eq!(state.hooks[&7].waiting_on, None);
                feed(&mut state, 7, &hook("Stop"), None);
                assert_eq!(cell(&state, 7), "idle");
            }
        }
    }

    /// But only the prompts that mode actually answers. `acceptEdits` accepts edits;
    /// a `Bash` prompt under it blocks until somebody says otherwise, and a question is
    /// not a permission prompt at all — so both still interrupt. The toast is the only
    /// thing that ever says so, because it is gated on the edge into the wait and the
    /// `Notification` six seconds later cannot cross that edge a second time.
    #[test]
    fn an_unattended_mode_only_silences_the_prompts_it_answers() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let accept_edits = |event: wire::HookEvent| wire::HookEvent {
            permission_mode: Some("acceptEdits".to_owned()),
            ..event
        };

        // An edit is what the mode is for: accepted without anybody.
        let (toasts, _) = feed(
            &mut state,
            7,
            &accept_edits(tool_hook(
                "PermissionRequest",
                "Edit",
                serde_json::json!({ "file_path": "/work/src/main.rs" }),
            )),
            None,
        );
        assert!(
            toasts.is_empty(),
            "an edit under acceptEdits answers itself"
        );

        // Back to work, so the next wait crosses the edge on its own.
        feed(&mut state, 7, &hook("PostToolUse"), None);
        let (toasts, _) = feed(
            &mut state,
            7,
            &accept_edits(tool_hook(
                "PermissionRequest",
                "Bash",
                serde_json::json!({ "command": "cargo test" }),
            )),
            None,
        );
        assert_eq!(toasts.len(), 1, "a Bash prompt under acceptEdits blocks");
        assert!(toasts[0].1.contains("Bash cargo"));

        feed(&mut state, 7, &hook("PostToolUse"), None);
        let (toasts, _) = feed(
            &mut state,
            7,
            &accept_edits(tool_hook(
                "PreToolUse",
                "AskUserQuestion",
                serde_json::json!({ "questions": [{ "question": "Which library?" }] }),
            )),
            None,
        );
        assert_eq!(toasts.len(), 1, "no mode answers a question");
        assert!(toasts[0].1.contains("Which library?"));
    }

    /// The run is the record a summary is made of, so what it keeps is the point: the
    /// prompts, the plans, the questions and the results of every turn, deduplicated, and
    /// nothing else. Nothing is summarised along the way — the session ending is what asks.
    #[test]
    fn a_run_keeps_the_key_events_of_every_turn() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("implement history"), None);
        // A spawned Claude with the same user-level hook installed reports each event twice.
        feed(&mut state, 7, &prompt("implement history"), None);
        feed(
            &mut state,
            7,
            &tool_hook(
                "PreToolUse",
                "ExitPlanMode",
                serde_json::json!({"plan": "persist the important events"}),
            ),
            None,
        );
        feed(
            &mut state,
            7,
            &tool_hook(
                "PreToolUse",
                "AskUserQuestion",
                serde_json::json!({"questions": [{"question": "Keep old runs?"}]}),
            ),
            None,
        );
        let (_, sent) = feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        assert!(sent.is_empty(), "a turn ending buys nothing");
        feed(&mut state, 7, &prompt("add the archive"), None);
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));

        let run = &state.runs[&7];
        assert_eq!(
            run.entries
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [
                agents::RunEntryKind::Prompt,
                agents::RunEntryKind::Plan,
                agents::RunEntryKind::Question,
                agents::RunEntryKind::Result,
                agents::RunEntryKind::Prompt,
                agents::RunEntryKind::Result,
            ]
        );
        assert!(run.summary.is_none(), "and nothing has described it yet");

        let run_id = state.runs[&7].id.clone();
        let (_, sent) = feed(
            &mut state,
            7,
            &hook(agents::HOOK_SESSION_END),
            Some(&endpoint()),
        );

        // One request, carrying every turn: both prompts and both results.
        assert_eq!(sent.len(), 1);
        let user = asked_about(&sent[0].1);
        assert!(user.contains("implement history"), "{user}");
        assert!(user.contains("add the archive"), "{user}");
        assert_eq!(
            user.matches("I ran the test suite").count(),
            2,
            "both turns' results: {user}"
        );

        let completed = &state.dirty_runs[&run_id];
        assert_eq!(completed.outcome, agents::RunOutcome::Completed);
        assert!(completed.ended_at_ms.is_some());
        assert!(state.runs.is_empty());
    }

    /// A persist writes the run's row and only the entries the store does not have yet.
    ///
    /// The point of storing the timeline as rows: a run that has been going all day costs one
    /// small insert per hook, not a rewrite of everything the agent has ever said.
    #[test]
    fn a_persist_writes_only_the_entries_the_store_does_not_have() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            zellij_session_name: Some("zellij".to_owned().into()),
            ..State::default()
        };
        feed(
            &mut state,
            7,
            &wire::HookEvent {
                prompt: Some("implement history".to_owned()),
                ..hook("UserPromptSubmit")
            },
            None,
        );
        let run_id = state.runs[&7].id.clone();
        // One run row, one entry row.
        let (first, written) = state.dirty_run_mutations(&agents::rows::Owner::new(
            HostId::new("host-a"),
            "zellij".into(),
        ));
        assert_eq!(first.len(), 2);
        assert_eq!(written, vec![(run_id.clone(), 1)]);

        // The mark the persist would have set, and a second prompt on top of it.
        state.persisted_revision.extend(written);
        feed(
            &mut state,
            7,
            &wire::HookEvent {
                prompt: Some("and the archive".to_owned()),
                ..hook("UserPromptSubmit")
            },
            None,
        );
        let (second, written) = state.dirty_run_mutations(&agents::rows::Owner::new(
            HostId::new("host-a"),
            "zellij".into(),
        ));
        assert_eq!(second.len(), 2, "the run row and the one new entry");
        assert_eq!(written, vec![(run_id.clone(), 2)]);

        // A run whose pane has left this session still writes its entries. Only the run row
        // is guarded: the payload is what two sessions disagree about, and it goes out
        // without a timeline, so an entry skipped here would have no copy left anywhere.
        state.persisted_revision.extend(written);
        state.released_runs.insert(run_id.clone());
        feed(
            &mut state,
            7,
            &wire::HookEvent {
                prompt: Some("and the release".to_owned()),
                ..hook("UserPromptSubmit")
            },
            None,
        );
        let (released, written) = state.dirty_run_mutations(&agents::rows::Owner::new(
            HostId::new("host-a"),
            "zellij".into(),
        ));
        assert_eq!(
            released.len(),
            2,
            "the guarded run row and the one new entry"
        );
        assert_eq!(written, vec![(run_id, 3)]);
    }

    /// A run restored from a payload written before the split has no entry rows and no mark,
    /// so its whole timeline is unwritten. Releasing it must still store it: the guarded row
    /// it goes out as carries no entries, and there would be nothing left to read it from.
    #[test]
    fn a_released_run_whose_entries_were_never_rows_still_writes_them() {
        let mut state = State {
            zellij_session_name: Some("zellij".to_owned().into()),
            ..State::default()
        };
        let mut run = agents::AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            zellij_session_name: "zellij".to_owned().into(),
            host_id: HostId::new("host-a"),
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
            worktree: None,
            started_at_ms: 1,
            ended_at_ms: None,
            outcome: agents::RunOutcome::Active,
            revision: 0,
            summarized_revision: 0,
            summary: None,
            entries: Vec::new(),
        };
        for text in ["ship it", "and the archive"] {
            agents::push_summary_entry(&mut run, text, 100);
        }
        // Released with no mark at all, exactly as a legacy run arrives from `hydrate_runs`.
        state.released_runs.insert(run.id.clone());
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (mutations, written) = state.dirty_run_mutations(&agents::rows::Owner::new(
            HostId::new("host-a"),
            "zellij".into(),
        ));
        assert_eq!(
            mutations.len(),
            3,
            "the guarded run row and both of the entries it no longer carries"
        );
        assert_eq!(written, vec![(run.id, 2)]);
    }

    /// A session resumed with `--continue` has no `UserPromptSubmit` of its own: its whole
    /// record is the results of the turns it ran. That is still something to describe.
    #[test]
    fn a_result_only_run_is_summarised_when_the_session_ends() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));

        let run = &state.runs[&7];
        assert_eq!(run.entries.len(), 1);
        assert_eq!(run.entries[0].kind, agents::RunEntryKind::Result);

        let (_, sent) = feed(
            &mut state,
            7,
            &hook(agents::HOOK_SESSION_END),
            Some(&endpoint()),
        );
        let user = asked_about(&sent[0].1);
        assert!(user.contains("result: I ran the test suite"), "{user}");
    }

    /// Every turn's result is kept whole on the run, so the summary made when the session
    /// ends sees all of them, once each — including turns whose own summary request failed
    /// or was superseded while the session was still running.
    #[test]
    fn every_turns_result_reaches_the_summary_made_at_the_end() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let first = "The first turn completed and its durable result must survive.";
        let second = "The second turn completed before the first summary was accepted.";
        let third = "The third turn completed after the second request was superseded.";

        feed(&mut state, 7, &stop_saying(first), Some(&endpoint()));
        // A summary somebody asked for, and the reply that never usefully came back.
        ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        finish(&mut state, 500, b"failed", &context(7, 1));

        feed(&mut state, 7, &hook("UserPromptSubmit"), None);
        feed(&mut state, 7, &stop_saying(second), Some(&endpoint()));
        feed(&mut state, 7, &hook("UserPromptSubmit"), None);
        feed(&mut state, 7, &stop_saying(third), Some(&endpoint()));

        let (_, sent) = feed(
            &mut state,
            7,
            &hook(agents::HOOK_SESSION_END),
            Some(&endpoint()),
        );
        let user = asked_about(&sent[0].1);
        for result in [first, second, third] {
            assert_eq!(user.matches(result).count(), 1, "{user}");
        }
    }

    /// The header carries the *newest* prompt, not the session's first.
    ///
    /// It is there to carry one prompt past the tail `run_delta` takes, and that tail cuts
    /// from the old end - so naming the first prompt would assert in the header the one
    /// question the body is most likely to have dropped, and describe a session's last hour
    /// as an answer to what opened it.
    #[test]
    fn the_summary_of_a_session_is_headed_by_its_newest_question() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("fix the flaky test"), None);
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));

        let (_, sent) = feed(
            &mut state,
            7,
            &hook(agents::HOOK_SESSION_END),
            Some(&endpoint()),
        );

        let user = asked_about(&sent[0].1);
        assert!(
            user.contains("--- the user asked ---\nrewrite the retry backoff"),
            "{user}"
        );
        // Both are still in the history, which is where the first one belongs.
        assert!(user.contains("fix the flaky test"), "{user}");
    }

    /// A `SessionEnd` is the ordinary way a run ends, not the only one. A re-run in the same
    /// pane replaces the run that was there, and that run is as over as any - if nothing asks
    /// about it now, nothing ever will: it is archived, and only a key press reaches it again.
    #[test]
    fn a_run_replaced_by_a_new_session_is_summarised_as_it_closes() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        let replaced = state.runs[&7].id.clone();

        let (_, sent) = feed(
            &mut state,
            7,
            &wire::HookEvent {
                session_id: "cc-session-2".to_owned(),
                ..prompt("and again, from the top")
            },
            Some(&endpoint()),
        );

        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, replaced);
        assert_eq!(
            state.dirty_runs[&replaced].outcome,
            agents::RunOutcome::Replaced
        );
        assert_ne!(state.runs[&7].id, replaced, "and the new run has begun");
    }

    /// The other close that knows: the agent is gone but its pane is not - it exited, or the
    /// foreground command dropped back to a shell. No `SessionEnd` will ever arrive for it.
    #[test]
    fn a_run_closed_without_a_session_end_is_summarised_as_it_closes() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        let closed = state.runs[&7].id.clone();

        state.finish_run(7, agents::RunOutcome::Closed, now_ms());
        let mut sent = Vec::new();
        drain_asks(&mut state, Some(&endpoint()), &mut sent);

        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, closed);
    }

    /// The one close that does not know. A pane leaving the manifest is a warp and a dead
    /// pane alike from here, which is why its write is guarded - and a summary written from
    /// this session's stale copy would land unguarded over the row the session that took the
    /// pane now owns. Where it really was warped, that session ends it and describes it.
    #[test]
    fn a_released_run_is_left_to_whoever_took_its_pane() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        let released = state.runs[&7].id.clone();

        state.release_run(7, now_ms());
        let mut sent = Vec::new();
        drain_asks(&mut state, Some(&endpoint()), &mut sent);

        assert!(sent.is_empty());
        assert!(state.released_runs.contains(&released));
        assert!(state.dirty_runs[&released].summary.is_none());
    }

    #[test]
    fn duplicate_delivery_is_dropped_but_repeated_questions_are_kept() {
        let mut state = State::default();
        let call = |id: &str, event: &str| wire::HookEvent {
            tool_use_id: Some(id.to_owned()),
            ..tool_hook(
                event,
                "AskUserQuestion",
                serde_json::json!({"questions": [{"question": "Which library?"}]}),
            )
        };

        feed(&mut state, 7, &call("tool-1", "PreToolUse"), None);
        feed(&mut state, 7, &call("tool-1", "PreToolUse"), None);
        feed(&mut state, 7, &call("tool-1", "PostToolUse"), None);
        feed(&mut state, 7, &call("tool-2", "PreToolUse"), None);

        let run = &state.runs[&7];
        assert_eq!(run.entries.len(), 2);
        assert_eq!(run.entries[0].text, "Which library?");
        assert_eq!(run.entries[1].text, "Which library?");
        assert_ne!(run.entries[0].event_id, run.entries[1].event_id);
    }

    /// A notification of the shape Claude Code submits when a background task wakes an agent,
    /// with a result as long as the caller asks for.
    fn notification(result: &str) -> String {
        format!(
            "<task-notification>\n\
             <task-id>a10fe836c6a9fa4ca</task-id>\n\
             <status>completed</status>\n\
             <summary>Agent \"/code-review\" finished</summary>\n\
             <result>{result}</result>\n\
             </task-notification>"
        )
    }

    fn prompts_of(state: &State, pane_id: u32) -> Vec<&str> {
        state.runs[&pane_id]
            .entries
            .iter()
            .filter(|entry| entry.kind == agents::RunEntryKind::Prompt)
            .map(|entry| entry.text.as_str())
            .collect()
    }

    /// The whole reason a prompt was given an identity. A person's prompt is compared against
    /// the entry before it, which is right for something typed twice — but a harness
    /// redelivering the envelope that woke an agent can put a whole turn in between, and the
    /// adjacent rule never saw it.
    #[test]
    fn a_redelivered_notification_is_dropped_however_far_apart_it_arrives() {
        let mut state = State::default();
        let woken = notification("The review found four issues.");

        feed(&mut state, 7, &prompt(&woken), None);
        feed(&mut state, 7, &stop_saying(REPLY), None);
        feed(&mut state, 7, &prompt(&woken), None);
        assert_eq!(
            prompts_of(&state, 7).len(),
            1,
            "the redelivery landed across an intervening turn"
        );

        // A second report for the same task is a second thing happening, not a second
        // delivery of the first - the envelope's own note says so.
        feed(
            &mut state,
            7,
            &prompt(&notification("The review found one issue.")),
            None,
        );
        assert_eq!(prompts_of(&state, 7).len(), 2);
    }

    /// The cost of refusing to guess, stated rather than hidden. A cut notification has no
    /// identity, so it keeps the adjacent-only rule: two deliveries back to back still collapse,
    /// and two separated by a turn land twice. A visible duplicate is the right way round -
    /// the alternative was dropping a real report because the bytes telling it apart from
    /// another one were the bytes the broadcast cap took.
    #[test]
    fn a_cut_notification_is_kept_rather_than_dropped() {
        let mut state = State::default();
        let shared = "a line of the review\n".repeat(20);
        let first = notification(&format!("{shared}four issues"));
        let second = notification(&format!("{shared}one issue"));
        let (first, second) = (first[..300].to_owned(), second[..300].to_owned());
        assert_eq!(first, second, "the cut took the difference between them");

        feed(&mut state, 7, &prompt(&first), None);
        feed(&mut state, 7, &prompt(&first), None);
        assert_eq!(
            prompts_of(&state, 7).len(),
            1,
            "back to back, the adjacent rule still catches it"
        );

        feed(&mut state, 7, &stop_saying(REPLY), None);
        feed(&mut state, 7, &prompt(&second), None);
        assert_eq!(prompts_of(&state, 7).len(), 2);
    }

    /// A run recorded before `event_id` existed comes back out of its JSON `payload` blob with
    /// no ids on its entries, so the whole-run comparison against it used to match nothing at
    /// all, and the oldest runs — the ones with the most history to be confused by a repeat —
    /// were exactly the ones a redelivery landed twice against. The id is a function of the
    /// text now, so it can be worked out again.
    #[test]
    fn a_legacy_entry_with_no_id_still_absorbs_a_redelivery() {
        let mut state = State::default();
        let woken = notification("The review found four issues.");

        feed(&mut state, 7, &prompt(&woken), None);
        let entry = &mut state.runs.get_mut(&7).expect("a run").entries[0];
        assert!(entry.event_id.is_some(), "it was given one on the way in");
        // What a timeline hydrated from the old blob looks like.
        entry.event_id = None;

        feed(&mut state, 7, &stop_saying(REPLY), None);
        feed(&mut state, 7, &prompt(&woken), None);
        assert_eq!(prompts_of(&state, 7).len(), 1);
    }

    #[test]
    fn active_runs_follow_a_zellij_session_rename() {
        let mut state = State::default();
        feed(&mut state, 7, &hook("UserPromptSubmit"), None);

        state.set_zellij_session_name("first".to_owned().into());
        assert_eq!(state.runs[&7].zellij_session_name, "first".into());
        state.set_zellij_session_name("renamed".to_owned().into());
        assert_eq!(state.runs[&7].zellij_session_name, "renamed".into());
        assert_eq!(
            state.dirty_runs[&state.runs[&7].id].zellij_session_name,
            "renamed".into()
        );
    }

    /// `summaries "false"` means do not do this, and that has to switch off the summary a
    /// session's end would otherwise buy as well as the one somebody presses a key for.
    #[test]
    fn summaries_switched_off_are_switched_off_on_both_paths() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(7, &stop_saying(REPLY));
        let run_id = state.runs[&7].id.clone();
        state.on_hook_with(
            7,
            CLAUDE,
            &hook(agents::HOOK_SESSION_END),
            |_, _| panic!("nothing to say"),
            |_, _| {},
        );
        state.drain_finished_runs(|state, run_id| {
            state.summarize_run_now_with(run_id, Some(&endpoint()), false, |_, _, _, _, _| {
                panic!("nobody should be asked")
            });
        });
        assert!(state.summaries.is_empty());
        assert!(
            state.dirty_runs[&run_id].summary.is_none(),
            "and the run that ended kept none either"
        );

        // And the key, which is a user asking for the same thing by hand. "Do not do this"
        // did not mean "unless I press a key", and it switches off the local fallback too —
        // which is the whole difference between the setting and simply having no API key.
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_hook(7, &stop_saying(REPLY));
        let (asked, sent, toasts) =
            ask_wanting(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE), false);
        assert!(!asked);
        assert!(sent.is_empty());
        assert!(toasts.is_empty());
        assert!(state.summaries.is_empty());
    }

    /// A summary that says what the one already held says is the same summary told twice, so
    /// it keeps the age of the answer rather than the age of the poll that repeated it. The
    /// stored `agent_summary` row is left alone by an unchanged summary, so a stamp that moved
    /// here would disagree with the row the moment anything restarted.
    #[test]
    fn a_re_summary_that_changed_no_word_keeps_its_first_stamp() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        // What this path is about to produce, stamped long ago and already held.
        let mut held = agents::local_summary(REPLY, 0).expect("worth drawing");
        held.created_at_ms = Some(1);
        // Not a hash this path can produce, so the stored summary demonstrably *is* the new
        // one rather than the seeded one left standing.
        held.hash = u64::MAX;
        state.summaries.insert(7, held);

        feed(&mut state, 7, &stop_saying(REPLY), None);
        ask(&mut state, 7, None, sample(BUSY_PANE));
        assert_ne!(state.summaries[&7].hash, u64::MAX, "a summary was stored");
        assert_eq!(
            state.summaries[&7].created_at_ms,
            Some(1),
            "the same answer, so the same age"
        );

        // A different answer is a new one, and is stamped now.
        feed(
            &mut state,
            7,
            &stop_saying("I rebased onto main and pushed the branch. Nothing else changed."),
            None,
        );
        ask(&mut state, 7, None, sample(BUSY_PANE));
        let stamped = state.summaries[&7]
            .created_at_ms
            .expect("a new summary is stamped");
        assert!(stamped > 1, "{stamped}");
    }

    /// A hook-fed pane works with no API key at all: what the agent last said is prose
    /// already, so it is its own summary. There is no title in one, so the pane keeps the
    /// name it has.
    #[test]
    fn a_session_with_no_endpoint_still_gets_a_summary_and_keeps_its_pane_name() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &stop_saying(REPLY), None);
        let (asked, sent, toasts) = ask(&mut state, 7, None, sample(BUSY_PANE));
        assert!(asked);
        assert!(sent.is_empty(), "nobody was asked");

        let summary = state.summaries.get(&7).expect("a summary was stored");
        assert!(summary.text.starts_with("I ran the test suite"));
        assert_eq!(
            agents::rename_title(summary),
            None,
            "no title, so the pane keeps its name"
        );
        assert_eq!(
            toasts,
            vec![(7, format!("agent idle · 7 · claude◆\n{}", summary.text))],
        );
    }

    /// And a session replaced in the same pane: `/clear` starts a new one, and the old
    /// session's summary describes work the user cannot get back to.
    #[test]
    fn a_restarted_session_drops_what_was_said_about_the_one_before_it() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &stop_saying(REPLY), Some(&endpoint()));
        ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        let id = state.summary_requests.get(&7).expect("in flight").id;

        let restarted = wire::HookEvent {
            session_id: "cc-session-2".to_owned(),
            ..hook("UserPromptSubmit")
        };
        feed(&mut state, 7, &restarted, None);
        assert!(
            state.summary_requests.is_empty(),
            "the request belonged to the session that ended"
        );
        finish(
            &mut state,
            200,
            &reply("Tests pass.", false),
            &context(7, id),
        );
        assert!(state.summaries.is_empty());
    }

    /// But the restart takes only the old session with it. `/clear` mints a new id and
    /// the first thing the new session says is its `UserPromptSubmit`, so the cleanup
    /// and the question arrive in the same event — and the question is the new
    /// session's, not the dead one's.
    #[test]
    fn a_restarted_sessions_own_prompt_survives_the_cleanup() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let asked = |session: &str, prompt: &str| wire::HookEvent {
            session_id: session.to_owned(),
            prompt: Some(prompt.to_owned()),
            ..hook("UserPromptSubmit")
        };

        feed(
            &mut state,
            7,
            &asked("cc-session-1", "port the parser"),
            None,
        );
        assert_eq!(
            state.prompts.get(&7).map(String::as_str),
            Some("port the parser")
        );

        feed(
            &mut state,
            7,
            &asked("cc-session-2", "now write the docs"),
            None,
        );
        assert_eq!(
            state.prompts.get(&7).map(String::as_str),
            Some("now write the docs"),
            "the new session asked it, so the summary of its first turn must have it"
        );
    }

    /// An event whose `raw` does not parse changes nothing: the session already read it for
    /// the record, and this daemon keeps the history from the agent's own JSON.
    #[test]
    fn an_agent_event_whose_raw_does_not_parse_changes_nothing() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let event = |raw: &str| {
            Event::AgentEvent(wire::AgentEvent {
                pane_id: zellij_tile::prelude::TerminalId::new(7),
                kind: CLAUDE.to_owned(),
                hook: "Stop".to_owned(),
                raw: raw.to_owned(),
                ..Default::default()
            })
        };
        state.update(event("not json at all"));
        assert!(state.hooks.is_empty());

        // A field we have never heard of is not a reason to drop the event: Claude Code
        // grows them between versions.
        state.update(event(
            r#"{"hook_event_name":"Stop","some_future_field":{"a":1}}"#,
        ));
        assert!(state.hooks.contains_key(&7));
    }

    #[test]
    fn an_agent_going_idle_is_asked_about_once() {
        let (mut state, sent) = idle_agent(BUSY_PANE);
        assert_eq!(sent.len(), 1, "one edge, one request");
        assert_eq!(sent[0].0, 7);
        let body: serde_json::Value = serde_json::from_slice(&sent[0].1).unwrap();
        assert_eq!(body["model"], "test-model");
        assert!(body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains(BUSY_PANE));

        let finished = finish(
            &mut state,
            200,
            &reply("Ran the tests.", true),
            &context(7, 1),
        );
        let summary = state.summaries.get(&7).expect("stored");
        assert_eq!(summary.text, "Ran the tests.");
        assert!(summary.awaiting);
        assert_eq!(
            finished.renames,
            vec![(7, TITLE.to_owned())],
            "and the pane is named after what the model said it was doing",
        );
        assert!(
            state.summary_requests.is_empty(),
            "and the request is no longer in flight",
        );
    }

    /// A title is the only part of an answer that is optional. A model that gave none,
    /// or gave one made of nothing, leaves the pane called whatever it was called.
    #[test]
    fn a_summary_without_a_title_renames_nothing() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        let finished = finish(
            &mut state,
            200,
            &titled_reply("Done.", false, "   "),
            &context(7, 1),
        );
        assert!(finished.renames.is_empty());
        assert!(state.summaries.contains_key(&7), "but the summary is kept");
    }

    /// `classify` reads the title first, so renaming a pane we only recognise *by* its
    /// title would take the pane out of the list and stop it being polled.
    #[test]
    fn a_pane_known_only_by_its_title_is_left_named_as_it_is() {
        let mut state = State {
            // An agent running at the far end of an ssh session: the command names
            // nothing we know, so the title is all that makes this pane an agent.
            panes: Some(vec![agents::AgentPane {
                command: Some("ssh box".to_owned()),
                ..pane(7, "claude", "/work")
            }]),
            ..State::default()
        };
        poll_showing(&mut state, BUSY_PANE);
        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        assert!(asked, "still summarised");
        assert_eq!(sent.len(), 1);

        let finished = finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        assert!(finished.renames.is_empty());
    }

    /// The user's own registration anchors the kind the way the command latch does, so a
    /// pane registered by hand can be renamed however it was detected.
    #[test]
    fn a_hand_registered_pane_can_be_renamed() {
        let mut state = State {
            panes: Some(vec![agents::AgentPane {
                command: Some("ssh box".to_owned()),
                ..pane(7, "shell", "/work")
            }]),
            manual_kinds: BTreeMap::from([(7, "reviewer".to_owned())]),
            ..State::default()
        };
        poll_showing(&mut state, BUSY_PANE);
        ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));

        let finished = finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        assert_eq!(finished.renames, vec![(7, TITLE.to_owned())]);
    }

    #[test]
    fn a_pane_with_nothing_on_it_is_not_worth_asking_about() {
        let (_, sent) = idle_agent("$");
        assert!(sent.is_empty());
    }

    /// The summary reaches the user the moment it is made, over whatever pane they are in,
    /// naming the one it is about — the snapshot alone only reaches a hocus that is running
    /// and only shows in a pane they are looking at.
    #[test]
    fn a_new_summary_is_toasted_over_whatever_pane_the_user_is_in() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(
            &mut state,
            200,
            &reply("Ran the tests.", false),
            &context(7, 1),
        );

        assert_eq!(
            finished.toasts,
            vec![(7, "agent idle · 7 · claude\nRan the tests.".to_owned())],
            "one summary, one toast, naming the pane and its kind",
        );
    }

    /// The user is looking at the very pane the summary describes: they can see what it
    /// says, and a toast over it is an interruption that tells them nothing.
    #[test]
    fn a_summary_for_the_pane_the_user_is_watching_renames_but_does_not_toast() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        state.focused = Some(7);

        let finished = finish(
            &mut state,
            200,
            &reply("Ran the tests.", false),
            &context(7, 1),
        );

        assert!(finished.toasts.is_empty(), "no toast over the pane itself");
        assert_eq!(
            finished.renames,
            vec![(7, TITLE.to_owned())],
            "but the pane is still named after what it did"
        );
        assert!(state.summaries.contains_key(&7), "and the answer is kept");
    }

    // -----------------------------------------------------------------------
    // Readings that are not work: scrolling, resizing, attaching
    // -----------------------------------------------------------------------

    /// And when the proposal is confirmed, what we knew about the agent goes with it: a
    /// summary of a session that has ended is worse than no summary at all.
    #[test]
    fn a_confirmed_release_takes_the_summary_with_it() {
        let (mut state, sent) = idle_agent(BUSY_PANE);
        assert_eq!(sent.len(), 1);
        state.on_command(7, vec!["claude".to_owned()], true);
        finish(
            &mut state,
            200,
            &reply("Running the tests", false),
            &context(7, 1),
        );
        assert!(state.summaries.contains_key(&7), "there was one to lose");

        state.on_command(7, vec!["zsh".to_owned()], false);
        state.repair_commands_with(|_| Some("zsh".to_owned()));

        assert!(state.summaries.is_empty(), "the session is over");
        assert!(state.activity.is_empty());
    }

    /// The agent went away while we were asking about it. The answer describes a session
    /// that has ended, and a toast about one is an interruption for nothing.
    #[test]
    fn a_reply_that_outlived_its_agent_is_dropped() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        state.on_command(7, vec!["claude".to_owned()], true);
        state.on_command(7, vec!["zsh".to_owned()], false);

        let finished = finish(
            &mut state,
            200,
            &reply("Running the tests", false),
            &context(7, 1),
        );

        assert!(finished.toasts.is_empty(), "nothing to interrupt for");
        assert!(finished.renames.is_empty());
        assert!(!state.summaries.contains_key(&7));

        // The same for a pane that has gone altogether rather than merely stopped being an
        // agent — there is nothing left to name, toast at, or describe.
        let (mut state, _) = idle_agent(BUSY_PANE);
        state.panes = Some(Vec::new());

        let finished = finish(
            &mut state,
            200,
            &reply("Running the tests", false),
            &context(7, 1),
        );

        assert!(finished.toasts.is_empty());
        assert!(state.summaries.is_empty());
    }

    /// A screen summary may still mark an agent as awaiting input, but only an explicit hook
    /// wait is reliable enough to interrupt the user.
    #[test]
    fn a_screen_summary_waiting_on_the_user_is_stored_but_not_toasted() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(
            &mut state,
            200,
            &reply("Shall I push?", true),
            &context(7, 1),
        );

        assert!(finished.toasts.is_empty());
        assert!(state.summaries.get(&7).expect("stored").awaiting);
    }

    /// A reply whose request has been replaced, or whose pane has closed, answers a
    /// question nobody is asking any more.
    #[test]
    fn a_reply_from_a_superseded_request_is_ignored() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(&mut state, 200, &reply("Stale.", true), &context(7, 99));
        assert!(state.summaries.is_empty());
        assert!(
            finished.toasts.is_empty(),
            "and nothing interrupts the user over it",
        );
        assert_eq!(
            state.summary_requests.get(&7).map(|r| r.id),
            Some(1),
            "and the real request is still expected",
        );

        // Nor does a reply for a pane nothing asked about land.
        finish(
            &mut state,
            200,
            &reply("Nobody asked.", true),
            &context(8, 1),
        );
        assert!(state.summaries.is_empty());
    }

    /// A failed summary is simply no summary - the same stance a failed Git probe takes.
    /// The pane is free to be asked about again rather than being written off.
    #[test]
    fn a_failed_request_stores_nothing_and_frees_the_pane_to_ask_again() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(&mut state, 500, b"upstream is down", &context(7, 1));

        assert!(state.summaries.is_empty());
        assert!(state.summary_requests.is_empty());
        assert!(finished.toasts.is_empty(), "and says nothing to the user");
        let (asked, _, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        assert!(asked, "so pressing the key again tries again");
    }

    /// An answer the parser cannot use is the same as no answer, and must not leave the
    /// pane looking like it still has a request out.
    #[test]
    fn an_unusable_answer_is_no_answer() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(&mut state, 200, b"{\"choices\":[]}", &context(7, 1));

        assert!(state.summaries.is_empty());
        assert!(state.summary_requests.is_empty());
        assert!(finished.toasts.is_empty());
    }

    /// A `WebRequestResult` that never arrives must not leave the pane looking permanently
    /// mid-request, or it would never be summarised again for the life of the session.
    #[test]
    fn a_request_that_never_comes_back_stops_blocking_the_pane() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        let (asked, _, _) = ask(&mut state, 7, Some(&endpoint()), sample(OTHER_PANE));
        assert!(!asked, "blocked while in flight");

        // It survives exactly `SUMMARY_TICKS` heartbeats and goes on the next.
        for _ in 0..SUMMARY_TICKS {
            state.observe_activity_with(|_| sample(BUSY_PANE));
        }
        assert!(!state.summary_requests.is_empty(), "still within its grace");
        state.observe_activity_with(|_| sample(BUSY_PANE));

        assert!(state.summary_requests.is_empty(), "aged out");
        let (asked, _, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        assert!(asked, "and the pane can be asked about again");
    }

    /// A summary belongs to a pane, and dies with it - the same rule `activity` follows,
    /// so a new agent in a recycled pane id cannot inherit the old one's state.
    #[test]
    fn a_closed_pane_takes_its_summary_and_its_request_with_it() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("Done.", true), &context(7, 1));
        assert!(!state.summaries.is_empty());

        state.panes = Some(vec![]);
        state.observe_activity_with(|_| sample(BUSY_PANE));

        assert!(state.summaries.is_empty());
        assert!(state.summary_requests.is_empty());
        assert!(
            state.activity.is_empty(),
            "the same rule that prunes activity"
        );
    }

    /// `AgentStatus` is a lifecycle flag, not the pane leaving. A summary dies with the
    /// pane, not with a status change - an agent going `Held` (finished, or `Exited`) is
    /// exactly the moment its last summary is worth keeping on screen.
    #[test]
    fn an_agent_gone_held_or_exited_still_shows_its_last_summary() {
        for status in [agents::AgentStatus::Held, agents::AgentStatus::Exited] {
            let (mut state, _) = idle_agent(BUSY_PANE);
            finish(&mut state, 200, &reply("Done.", true), &context(7, 1));
            assert!(!state.summaries.is_empty());

            state.panes.as_mut().unwrap()[0].status = status;
            state.observe_activity_with(|_| sample(BUSY_PANE));

            assert!(
                state.summaries.contains_key(&7),
                "{status:?} is a status change, not the pane closing"
            );
            assert!(
                state.activity.is_empty(),
                "activity still resets - only the summary is kept"
            );
        }
    }

    /// The context is what makes a reply ours. Every `WebRequestResult` in the session
    /// reaches this handler, including hocus's own streaming chat.
    #[test]
    fn a_web_response_that_is_not_a_summary_is_left_alone() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        finish(&mut state, 200, &reply("Not mine.", true), &BTreeMap::new());
        finish(
            &mut state,
            200,
            &reply("Not mine.", true),
            &BTreeMap::from([("stream_id".to_owned(), "hocus-1".to_owned())]),
        );

        assert!(state.summaries.is_empty());
        assert_eq!(
            state.summary_requests.get(&7).map(|r| r.id),
            Some(1),
            "still in flight",
        );
    }

    /// The branch the daemon already probed rides along: context the model cannot get
    /// from the pane text.
    #[test]
    fn a_request_carries_the_folder_and_the_branch_the_daemon_knows() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            repositories: BTreeMap::from([(
                local("/work"),
                projects::Repository {
                    branch: Some("topic".to_owned()),
                    ..projects::Repository::folder("/work")
                },
            )]),
            activity: agents::Activity::from([(7, (1, 1))]),
            ..State::default()
        };

        let mut body = Vec::new();
        state.send_summary(7, BUSY_PANE, &endpoint(), |_, _, _, sent, _| body = sent);

        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let user = value["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("Agent: claude."), "{user}");
        assert!(user.contains("Folder: /work."), "{user}");
        assert!(user.contains("Branch: topic."), "{user}");
    }

    #[test]
    fn a_result_from_a_replaced_git_probe_is_ignored() {
        let mut state = State {
            git_probes: BTreeMap::from([(
                local("/work"),
                GitProbe {
                    id: 2,
                    ..GitProbe::default()
                },
            )]),
            ..State::default()
        };
        let context = BTreeMap::from([
            ("cwd".to_owned(), "/work".to_owned()),
            ("git_probe".to_owned(), "metadata".to_owned()),
            ("git_probe_id".to_owned(), "1".to_owned()),
        ]);

        state.finish_git_probe(Some(0), b"stale".to_vec(), context);
        assert!(state.git_probes[&local("/work")].metadata.is_none());
    }

    // -----------------------------------------------------------------------
    // Summaries somebody asked for
    // -----------------------------------------------------------------------

    /// `summarize_pane_now` with the host calls handed back: whether a request went out,
    /// what was sent, and what was toasted.
    fn ask(
        state: &mut State,
        pane_id: u32,
        endpoint: Option<&Endpoint>,
        reading: poll::PaneSample,
    ) -> (bool, Vec<Asked>, Vec<(u32, String)>) {
        ask_wanting(state, pane_id, endpoint, reading, true)
    }

    /// One request an ask sent: the pane it named, the correlation id it will be answered
    /// on, and the body. The id is here because a manual ask shares `summary_seq` with every
    /// other request the daemon makes, so a test that guessed at it would be asserting on
    /// how many summaries its setup happened to provoke.
    struct Asked {
        pane_id: u32,
        id: u64,
        body: Vec<u8>,
    }

    /// The same, for the one test that is about `summaries "false"`.
    fn ask_wanting(
        state: &mut State,
        pane_id: u32,
        endpoint: Option<&Endpoint>,
        reading: poll::PaneSample,
        wanted: bool,
    ) -> (bool, Vec<Asked>, Vec<(u32, String)>) {
        let mut sent = Vec::new();
        let mut toasts = Vec::new();
        let asked = state.summarize_pane_now_with(
            pane_id,
            endpoint,
            wanted,
            |_| reading,
            |_, _, _, body, context| {
                sent.push(Asked {
                    pane_id: context[SUMMARY_PANE].parse().unwrap(),
                    id: context[SUMMARY_ID].parse().unwrap(),
                    body,
                });
            },
            |pane_id, text| toasts.push((pane_id, text.to_owned())),
        );
        (asked, sent, toasts)
    }

    /// The user content of a request, for the tests that are about what a model was shown.
    fn asked_about(body: &[u8]) -> String {
        let json: serde_json::Value = serde_json::from_slice(body).expect("a request body");
        json["messages"][1]["content"]
            .as_str()
            .expect("a user message")
            .to_owned()
    }

    /// The guard that is not about the budget. `finish_summary_with` correlates strictly on
    /// `(pane, id)`, so a second request would not arrive twice - it would orphan the first
    /// and answer only itself, and the answer already on its way would be thrown away.
    #[test]
    fn only_one_asked_for_summary_is_in_flight_at_a_time() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("done", false), &context(7, 1));
        let (asked, _, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        assert!(asked);

        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));

        assert!(!asked, "the first is still out");
        assert!(sent.is_empty());
    }

    /// The answer to a request somebody is waiting for survives the pane having moved, which
    /// is the exact opposite of the rule an unasked-for screen summary keeps. A person who
    /// pressed the key while an agent was mid-build would otherwise get silence, press again,
    /// and get silence again.
    #[test]
    fn an_asked_for_summary_survives_a_screen_that_moved_while_it_was_in_flight() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("first", false), &context(7, 1));
        let (asked, _, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        assert!(asked);
        // The agent got on with something else while the request was out.
        poll_showing(&mut state, "a completely different screen entirely");

        finish(
            &mut state,
            200,
            &reply("what I was asked", false),
            &context(7, 2),
        );

        assert_eq!(
            state.summaries[&7].text, "what I was asked",
            "somebody was waiting for this"
        );
    }

    /// A pane that reports for itself is described from what it said, not from what it
    /// shows. Asking its screen here would reintroduce the second, worse answer that the
    /// hook feed was brought in to retire.
    #[test]
    fn asking_about_a_hook_fed_pane_reads_its_run_and_not_its_screen() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        feed(&mut state, 7, &stop_saying(REPLY), None);
        // The turn's own summary is what would otherwise still be in flight.
        state.summary_requests.clear();

        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));

        assert!(asked);
        let body = asked_about(&sent[0].body);
        assert!(
            body.contains("rewrite the retry backoff"),
            "the request carries what the user asked: {body}"
        );
        assert!(
            !body.contains(BUSY_PANE),
            "and not a photograph of the pane: {body}"
        );
    }

    /// A run whose every entry an earlier summary already absorbed is still worth asking
    /// about. Somebody pressing a key wants to know what the agent has been doing;
    /// answering "nothing has changed since I last said" is indistinguishable from the key
    /// not working.
    #[test]
    fn asking_about_a_hook_fed_pane_reads_the_whole_run_not_only_what_is_new() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        feed(&mut state, 7, &stop_saying(REPLY), None);
        ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        finish(
            &mut state,
            200,
            &reply("rewrote the backoff", false),
            &context(7, 1),
        );
        assert_eq!(
            state.runs[&7].summarized_revision, state.runs[&7].revision,
            "every entry is inside the accepted revision"
        );

        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));

        assert!(asked);
        let body = asked_about(&sent[0].body);
        assert!(
            body.contains("rewrite the retry backoff"),
            "the run is re-read from its record: {body}"
        );
        assert!(
            !body.contains("previous summary"),
            "and not weighed against the paraphrase made from it: {body}"
        );
    }

    /// A run with nothing retained at all — every hook it saw was a tool call or a lifecycle
    /// event, none of which is kept. There is genuinely nothing to describe.
    #[test]
    fn asking_about_a_pane_whose_run_holds_nothing_asks_nobody() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &hook("SessionStart"), None);
        assert!(state.runs[&7].entries.is_empty());

        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));

        assert!(!asked);
        assert!(sent.is_empty());
    }

    /// A reading that says nothing is no more summarisable for having been asked for: a pane
    /// scrolled off its live tail shows old output, and one that could not be read holds an
    /// error message rather than anything the agent wrote.
    #[test]
    fn asking_about_a_pane_that_cannot_be_read_asks_nobody() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("done", false), &context(7, 1));

        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), scrolled(BUSY_PANE));
        assert!(!asked, "a scrolled pane shows old output");
        assert!(sent.is_empty());

        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), unreadable());
        assert!(!asked, "and an unreadable one shows an error message");
        assert!(sent.is_empty());
    }

    /// A screen-only pane has no honest fallback without an endpoint. `local_summary`
    /// shortens prose the agent wrote, and a screen tail is not that - which is why there was
    /// nothing to fall back to at all before the hook feed. Pressing a key does not make one.
    #[test]
    fn asking_about_a_screen_only_pane_with_no_endpoint_makes_nothing_up() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("done", false), &context(7, 1));

        let (asked, sent, _) = ask(&mut state, 7, None, sample(BUSY_PANE));

        assert!(!asked);
        assert!(sent.is_empty());
        assert_eq!(
            state.summaries[&7].text, "done",
            "and the summary it already had is left alone"
        );
    }

    /// A hook-fed pane with no endpoint still has an honest answer: the agent's own last
    /// message, shortened. A screen tail is not one, which is why the pane beside it gets
    /// nothing.
    #[test]
    fn asking_about_a_hook_fed_pane_with_no_endpoint_falls_back_to_what_it_said() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("run the tests"), None);
        feed(&mut state, 7, &stop_saying(REPLY), None);
        assert!(
            state.summaries.is_empty(),
            "the turn ending bought nothing by itself"
        );

        let (asked, sent, _) = ask(&mut state, 7, None, sample(BUSY_PANE));

        assert!(asked);
        assert!(sent.is_empty(), "nobody was asked");
        assert!(
            state.summaries[&7].text.starts_with("I ran the test suite"),
            "{}",
            state.summaries[&7].text
        );
    }

    /// `summaries "false"` is a user saying "do not do this", and they did not mean "unless
    /// I press a key".
    #[test]
    fn asking_for_a_summary_that_is_switched_off_asks_nobody() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("done", false), &context(7, 1));

        let (asked, sent, _) =
            ask_wanting(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE), false);

        assert!(!asked);
        assert!(sent.is_empty());
    }

    /// The point of the whole feature: the answer stays on the timeline at the place it was
    /// asked for, as well as replacing the rolling sentence.
    #[test]
    fn an_asked_for_summary_stays_on_the_timeline() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        feed(&mut state, 7, &stop_saying(REPLY), None);
        state.summary_requests.clear();
        let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
        assert!(asked);

        finish(
            &mut state,
            200,
            &reply("rewrote the backoff", false),
            &context(sent[0].pane_id, sent[0].id),
        );

        let run = state.runs.get(&7).expect("the pane's run");
        let summaries: Vec<&str> = run
            .entries
            .iter()
            .filter(|entry| entry.kind == agents::RunEntryKind::Summary)
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(summaries, vec!["rewrote the backoff"]);
        assert_eq!(
            run.summary.as_ref().expect("the rolling one too").text,
            "rewrote the backoff"
        );
        assert!(
            run.summarized_revision >= run.revision,
            "and the entry it just made is inside the accepted revision, so the next request \
             does not read it back as work the agent did"
        );
    }

    /// A run records the machine its pane is on, not only the folder the agent reported.
    /// Without it, a run on a build box in `/work/hocus` is indistinguishable from one here,
    /// and a project filter reading the history claims work nobody did on this machine.
    #[test]
    fn a_run_records_the_machine_its_pane_is_on() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.hosts.insert(7, "build-box".to_owned());
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        let run = state.runs.get(&7).expect("the pane's run");
        assert_eq!(run.host.as_deref(), Some("build-box"));

        // And it follows the pane: a warp back to this machine repairs it.
        state.hosts.remove(&7);
        feed(&mut state, 7, &prompt("and now the timeout"), None);
        let run = state.runs.get(&7).expect("the pane's run");
        assert_eq!(run.host, None);
    }

    /// And it is repaired by the manifest as well as by the next hook, because there may not
    /// be a next hook. A run whose first event beat the manifest that named its VM's worktree
    /// would otherwise be archived with none, and `matches_root` files a run with a host and
    /// no worktree under no project at all — the run vanishes from `agent_runs --project`.
    #[test]
    fn a_manifest_repairs_a_run_whose_first_hook_beat_it() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/workspace")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        let run = state.runs.get(&7).expect("the pane's run");
        assert_eq!(run.worktree, None, "the hook got here first");
        let run_id = run.id.clone();
        state.dirty_runs.clear();

        let mut vm = PaneInfo {
            id: zellij_tile::prelude::PaneId::from_legacy(7, false),
            title: "claude".to_owned(),
            host: Some("firecracker:review".to_owned()),
            ..PaneInfo::default()
        };
        vm.target =
            PaneTarget::firecracker("review", None, Some(std::path::Path::new("/work/hocus")))
                .expect("a valid VM name and an absolute worktree");
        state
            .manifest
            .panes
            .insert(zellij_tile::prelude::TabPosition::new(0), vec![vm]);
        state.rebuild_panes();

        let run = state.runs.get(&7).expect("the pane's run");
        assert_eq!(run.worktree.as_deref(), Some("/work/hocus"));
        assert_eq!(run.host.as_deref(), Some("firecracker:review"));
        assert!(
            state.dirty_runs.contains_key(&run_id),
            "and the repair is written, not only held",
        );
    }

    /// Two presses against an agent that has not moved produce the same sentence twice, and
    /// two identical paragraphs one under the other read as a bug rather than as a history.
    #[test]
    fn asking_twice_for_the_same_sentence_files_it_once() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("rewrite the retry backoff"), None);
        feed(&mut state, 7, &stop_saying(REPLY), None);
        state.summary_requests.clear();

        for _ in 0..2 {
            state.summary_requests.clear();
            let (asked, sent, _) = ask(&mut state, 7, Some(&endpoint()), sample(BUSY_PANE));
            assert!(
                asked,
                "the run is re-read whole, so there is always something to ask"
            );
            finish(
                &mut state,
                200,
                &reply("rewrote the backoff", false),
                &context(sent[0].pane_id, sent[0].id),
            );
        }

        let run = state.runs.get(&7).expect("the pane's run");
        assert_eq!(
            run.entries
                .iter()
                .filter(|entry| entry.kind == agents::RunEntryKind::Summary)
                .count(),
            1,
            "the second press bought nothing, and says so by adding nothing"
        );
    }

    /// A summary is the model's words about the agent. Feeding one back as though the agent
    /// had said it would let a paraphrase compound into the next paraphrase, and a long run
    /// would end up describing its own descriptions.
    #[test]
    fn a_filed_summary_is_not_shown_back_to_the_model() {
        let mut run = agents::AgentRun {
            id: "claude:cc-session-1".to_owned(),
            pane_id: 7,
            zellij_session_name: "here".to_owned().into(),
            host_id: HostId::new("host-a"),
            session_id: "cc-session-1".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
            worktree: None,
            started_at_ms: 10,
            ended_at_ms: None,
            outcome: agents::RunOutcome::Active,
            revision: 0,
            summarized_revision: 0,
            summary: None,
            entries: Vec::new(),
        };
        agents::push_summary_entry(&mut run, "a paraphrase of a paraphrase", 100);
        run.entries.push(agents::RunEntry {
            revision: 2,
            event_id: None,
            kind: agents::RunEntryKind::Question,
            text: "Which library?".to_owned(),
            created_at_ms: Some(100),
        });
        run.revision = 2;

        let delta = run_delta(&run).expect("there is a question to describe");

        assert!(delta.contains("Which library?"));
        assert!(!delta.contains("a paraphrase of a paraphrase"));
    }

    // -----------------------------------------------------------------------
    // Summaries of runs that have ended
    // -----------------------------------------------------------------------

    /// One archived run, as the store would hand it back.
    fn archived_run(id: &str) -> agents::AgentRun {
        agents::AgentRun {
            id: id.to_owned(),
            pane_id: 7,
            zellij_session_name: "here".to_owned().into(),
            host_id: HostId::new("host-a"),
            session_id: "cc-session-1".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
            worktree: None,
            started_at_ms: 10,
            ended_at_ms: Some(20),
            outcome: agents::RunOutcome::Completed,
            revision: 2,
            summarized_revision: 0,
            summary: None,
            entries: vec![
                agents::RunEntry {
                    revision: 1,
                    event_id: None,
                    kind: agents::RunEntryKind::Prompt,
                    text: "rewrite the retry backoff".to_owned(),
                    created_at_ms: Some(10),
                },
                agents::RunEntry {
                    revision: 2,
                    event_id: None,
                    kind: agents::RunEntryKind::Result,
                    text: REPLY.to_owned(),
                    created_at_ms: Some(15),
                },
            ],
        }
    }

    fn ask_run(
        state: &mut State,
        run_id: &str,
        endpoint: Option<&Endpoint>,
    ) -> (bool, Vec<(String, Vec<u8>)>) {
        let mut sent = Vec::new();
        let asked =
            state.summarize_run_now_with(run_id, endpoint, true, |_, _, _, body, context| {
                sent.push((context[SUMMARY_RUN].clone(), body));
            });
        (asked, sent)
    }

    fn run_context(run_id: &str, id: u64) -> BTreeMap<String, String> {
        BTreeMap::from([
            (SUMMARY_RUN.to_owned(), run_id.to_owned()),
            (SUMMARY_ID.to_owned(), id.to_string()),
        ])
    }

    /// A run that has ended has no pane to read and no session to watch, and is summarised
    /// from what it recorded. The answer goes back to the store, which is the only place an
    /// archived run lives.
    #[test]
    fn an_archived_run_is_summarised_from_what_it_recorded() {
        let mut state = State::default();
        let run = archived_run("claude:cc-session-1");
        // Standing in for the store fetch, which `load_run` tries second.
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (asked, sent) = ask_run(&mut state, &run.id, Some(&endpoint()));

        assert!(asked);
        assert_eq!(sent[0].0, run.id, "correlated by run, not by pane");
        let body = asked_about(&sent[0].1);
        assert!(body.contains("rewrite the retry backoff"), "{body}");

        state.finish_run_summary(
            200,
            &reply("rewrote the backoff", false),
            &run_context(&run.id, 1),
        );

        let stored = state.dirty_runs.get(&run.id).expect("queued for the store");
        assert_eq!(
            stored.summary.as_ref().expect("the rolling one").text,
            "rewrote the backoff"
        );
        assert_eq!(
            stored
                .entries
                .iter()
                .filter(|entry| entry.kind == agents::RunEntryKind::Summary)
                .count(),
            1,
            "and filed on the timeline, which is the whole reason to have asked"
        );
        assert!(
            !state.runs.contains_key(&7),
            "the daemon does not start watching a session that is over"
        );
        assert!(
            !state.summaries.contains_key(&7),
            "nor put it on whoever holds pane 7 now"
        );
    }

    /// The header and the body of one request are two readings of the same run, and the header
    /// used to be the only one still written in markup: the model was told the user asked
    /// `<task-notification> <task-id>a10fe836…` while the history beside it said, in words,
    /// that a background task had reported.
    #[test]
    fn an_archived_runs_header_is_a_question_rather_than_its_markup() {
        let mut state = State::default();
        let mut run = archived_run("claude:cc-session-1");
        run.entries[0].text = task_notification();
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (asked, sent) = ask_run(&mut state, &run.id, Some(&endpoint()));

        assert!(asked);
        let body = asked_about(&sent[0].1);
        let (_, question) = body
            .split_once("--- the user asked ---")
            .expect("a question in the header");
        let question = question
            .split_once("--- run history ---")
            .expect("and a history under it")
            .0;
        assert!(
            question.contains("background task a10fe836 completed:"),
            "{question}"
        );
        assert!(
            question.contains("The review found four issues."),
            "{question}"
        );
        assert!(!question.contains('<'), "{question}");
        // The same two things the pane lane drops, for the same reasons.
        assert!(!question.contains("notify more than once"), "{question}");
        assert!(!question.contains("/private/tmp"), "{question}");
    }

    /// A prompt with a question in it and then one with none. The second is a new turn rather
    /// than a continuation of the first, so what it leaves behind is nothing at all - an empty
    /// string kept under the pane's id would be a question the summariser has to filter out
    /// again downstream, and one nothing here would ever clear.
    #[test]
    fn a_prompt_that_is_all_machinery_keeps_no_question() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };

        feed_naming(&mut state, 7, &prompt("fix the flaky reconnect test"));
        assert_eq!(
            state.prompts.get(&7).map(String::as_str),
            Some("fix the flaky reconnect test")
        );

        // A reminder is the harness talking to the agent about how to behave. It says nothing
        // about what the turn is for, which is why its `question` is `None`.
        feed_naming(
            &mut state,
            7,
            &prompt("<system-reminder>\nBe concise.\n</system-reminder>"),
        );
        assert_eq!(state.prompts.get(&7), None);
    }

    /// A reply carrying a run id must never be taken up by the half that renames panes and
    /// toasts: the pane the run names is somebody else's by now.
    #[test]
    fn a_run_reply_is_not_taken_for_a_pane_reply() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let run = archived_run("claude:cc-session-1");
        state.dirty_runs.insert(run.id.clone(), run.clone());
        assert!(ask_run(&mut state, &run.id, Some(&endpoint())).0);

        // The dispatcher rather than the pane-lane body: which of the two a reply reaches is
        // exactly what is under test. The run branch makes no host call, so this is safe to
        // drive with no host behind it.
        state.finish_summary(200, &reply("rewrote it", false), &run_context(&run.id, 1));

        let finished = finish(
            &mut state,
            200,
            &reply("hijacked", false),
            &run_context(&run.id, 1),
        );
        assert!(finished.renames.is_empty(), "no pane of its own to rename");
        assert!(finished.toasts.is_empty(), "and nobody to interrupt");
        assert_eq!(
            state.dirty_runs[&run.id]
                .summary
                .as_ref()
                .expect("it still landed")
                .text,
            "rewrote it"
        );
    }

    /// An archived run that already has a summary is still worth asking about — the same
    /// span rule the live lane keeps, and the case this feature most exists for: a run whose
    /// one sentence was written three prompts before it ended.
    #[test]
    fn an_archived_run_that_already_has_a_summary_is_still_worth_asking_about() {
        let mut state = State::default();
        let mut run = archived_run("claude:cc-session-1");
        run.summarized_revision = run.revision;
        run.summary = Some(agents::Summary {
            text: "an old sentence".to_owned(),
            title: String::new(),
            awaiting: false,
            hash: 0,
            created_at_ms: None,
        });
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (asked, sent) = ask_run(&mut state, &run.id, Some(&endpoint()));

        assert!(asked);
        let body = asked_about(&sent[0].1);
        assert!(body.contains("rewrite the retry backoff"), "{body}");
        assert!(!body.contains("an old sentence"), "{body}");
    }

    /// A run with nothing retained at all has nothing to describe.
    #[test]
    fn an_archived_run_that_holds_nothing_asks_nobody() {
        let mut state = State::default();
        let mut run = archived_run("claude:cc-session-1");
        run.entries.clear();
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (asked, sent) = ask_run(&mut state, &run.id, Some(&endpoint()));

        assert!(!asked);
        assert!(sent.is_empty());
    }

    /// The in-flight guard, on the lane that has no pane to key it by.
    #[test]
    fn only_one_summary_of_a_run_is_in_flight_at_a_time() {
        let mut state = State::default();
        let run = archived_run("claude:cc-session-1");
        state.dirty_runs.insert(run.id.clone(), run.clone());
        assert!(ask_run(&mut state, &run.id, Some(&endpoint())).0);

        let (asked, sent) = ask_run(&mut state, &run.id, Some(&endpoint()));

        assert!(!asked);
        assert!(sent.is_empty());
    }

    /// A reply the host dropped must not cost a run its summaries for the rest of the
    /// session. Nothing else ever clears this lane — a run that has ended cannot stop being
    /// a running agent, which is what prunes the pane lane — so the tick count is the whole
    /// of its recovery.
    #[test]
    fn a_run_whose_reply_never_came_can_be_asked_about_again() {
        let mut state = State::default();
        let run = archived_run("claude:cc-session-1");
        state.dirty_runs.insert(run.id.clone(), run.clone());
        assert!(ask_run(&mut state, &run.id, Some(&endpoint())).0);
        assert!(!ask_run(&mut state, &run.id, Some(&endpoint())).0);

        for _ in 0..=SUMMARY_TICKS {
            poll_showing(&mut state, BUSY_PANE);
        }

        assert!(
            ask_run(&mut state, &run.id, Some(&endpoint())).0,
            "the answer is not coming, so the question may be asked again"
        );
    }

    /// A session with no API key still ends, and before this it ended into a blank row. The
    /// run's own last message is prose the agent wrote for a person to read, which is the one
    /// thing there is to fall back to — the same fallback the key press has had all along.
    #[test]
    fn an_archived_run_with_no_endpoint_falls_back_to_the_agents_own_words() {
        let mut state = State::default();
        let run = archived_run("claude:cc-session-1");
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (asked, sent) = ask_run(&mut state, &run.id, None);

        assert!(asked);
        assert!(sent.is_empty(), "nobody was asked, so nothing went out");
        let stored = state.dirty_runs[&run.id]
            .summary
            .as_ref()
            .expect("the run kept it");
        assert!(
            stored.text.starts_with("I ran the test suite"),
            "{}",
            stored.text
        );
        // No title: naming a pane in four words is a judgement, and this path has nobody to
        // make it. See `agents::local_summary`.
        assert!(stored.title.is_empty());
        assert_eq!(
            state.dirty_runs[&run.id].summarized_revision, state.dirty_runs[&run.id].revision,
            "and the run is not owed another"
        );
    }

    /// The record's *events* are what cannot be fed to `local_summary` - a list of hook names
    /// is not prose anybody wrote. A run that never said anything gets nothing rather than a
    /// paraphrase of its own bookkeeping.
    #[test]
    fn an_archived_run_that_never_spoke_has_no_local_fallback() {
        let mut state = State::default();
        let mut run = archived_run("claude:cc-session-1");
        run.entries
            .retain(|entry| entry.kind != agents::RunEntryKind::Result);
        state.dirty_runs.insert(run.id.clone(), run.clone());

        let (asked, sent) = ask_run(&mut state, &run.id, None);

        assert!(!asked);
        assert!(sent.is_empty());
        assert!(state.dirty_runs[&run.id].summary.is_none());
    }
    fn display(
        turn: &str,
        message: &str,
        index: u64,
        final_batch: bool,
        delta: &str,
    ) -> wire::HookEvent {
        wire::HookEvent {
            turn_id: Some(turn.into()),
            message_id: Some(message.into()),
            index: Some(index),
            final_batch: Some(final_batch),
            delta: Some(delta.into()),
            ..hook("MessageDisplay")
        }
    }

    #[test]
    fn assistant_message_batches_preserve_waits_and_deduplicate_after_reload_and_adoption() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        feed(&mut state, 7, &prompt("implement capture"), None);
        feed(&mut state, 7, &hook("PermissionRequest"), None);
        let waiting = cell(&state, 7);
        for event in [
            display("t1", "m1", 2, true, ""),
            display("t1", "m1", 1, false, "雪\n"),
            display("t1", "m1", 1, false, "雪\n"),
            display("t1", "m1", 0, false, "hello "),
        ] {
            feed(&mut state, 7, &event, None);
            assert_eq!(cell(&state, 7), waiting);
        }
        let run = state.runs[&7].clone();
        assert_eq!(run.entries.len(), 2);
        assert_eq!(run.entries[1].text, "hello 雪\n");
        let mut reloaded = State::default();
        let run = serde_json::from_str::<agents::AgentRun>(&serde_json::to_string(&run).unwrap())
            .unwrap();
        reloaded.adopt_run(9, run.adopted_by(9, &reloaded.owner()));
        reloaded.record_run_event(9, "claude", &display("t1", "m1", 0, true, "hello 雪\n"), 10);
        assert_eq!(reloaded.runs[&9].entries.len(), 2);
        reloaded.record_run_event(9, "claude", &display("t2", "m1", 0, true, "hello 雪\n"), 11);
        assert_eq!(
            reloaded.runs[&9].entries.len(),
            3,
            "same text in another turn is distinct"
        );
        assert!(run_delta(&reloaded.runs[&9]).unwrap().contains("hello"));
    }

    #[test]
    fn assistant_messages_deduplicate_stop_only_in_the_same_turn_and_capture_session_end() {
        let mut state = State::default();
        state.record_run_event(7, "claude", &prompt("work"), 1);
        state.record_run_event(7, "claude", &display("t1", "m1", 0, true, "done"), 2);
        let stop = wire::HookEvent {
            turn_id: Some("t1".into()),
            ..stop_saying("done")
        };
        state.record_run_event(7, "claude", &stop, 3);
        assert_eq!(state.runs[&7].entries.len(), 2);
        state.record_run_event(
            7,
            "claude",
            &wire::HookEvent {
                turn_id: Some("t2".into()),
                ..stop
            },
            4,
        );
        assert_eq!(
            state.runs[&7].entries.last().unwrap().kind,
            agents::RunEntryKind::Result
        );
        let message = hocus_proto::agent::CapturedMessage {
            source_id: "session:m2".into(),
            turn_id: "t2".into(),
            text: "closing words".into(),
            incomplete: false,
        };
        let end = wire::HookEvent {
            assistant_messages: vec![message],
            ..hook("SessionEnd")
        };
        let id = state.runs[&7].id.clone();
        state.record_run_event(7, "claude", &end, 5);
        assert!(!state.runs.contains_key(&7));
        assert_eq!(
            state.dirty_runs[&id].entries.last().unwrap().text,
            "closing words"
        );
        assert!(!state.pending_messages.contains_key(&7));
        let closed = state.dirty_runs[&id].clone();
        let summaries = state.finished_runs.len();
        state.record_run_event(7, "claude", &end, 6);
        assert_eq!(
            state.dirty_runs[&id], closed,
            "duplicate SessionEnd preserves the whole archive"
        );
        assert_eq!(state.finished_runs.len(), summaries);
        assert!(!state.runs.contains_key(&7));
    }

    #[test]
    fn capture_before_stop_requires_a_nonempty_matching_turn() {
        for turn in [None, Some(""), Some("other"), Some("t")] {
            let mut state = State::default();
            state.record_run_event(7, "claude", &display("t", "m", 0, true, "done"), 1);
            state.record_run_event(
                7,
                "claude",
                &wire::HookEvent {
                    turn_id: turn.map(str::to_owned),
                    ..stop_saying("done")
                },
                2,
            );
            assert_eq!(
                state.runs[&7].entries.len(),
                if turn == Some("t") { 1 } else { 2 }
            );
        }
    }

    #[test]
    fn session_end_recovery_respects_the_stored_owner() {
        for foreign in ["host", "session", "neither"] {
            for captured in [false, true] {
                let mut state = State {
                    host_id: HostId::new("host-a"),
                    zellij_session_name: Some("here".into()),
                    ..State::default()
                };
                let id = agents::session_run_id("claude", "cc-session-1");
                let mut run = archived_run(&id);
                run.outcome = agents::RunOutcome::Active;
                run.ended_at_ms = None;
                if foreign == "host" {
                    run.host_id = HostId::new("host-b");
                } else if foreign == "session" {
                    run.zellij_session_name = "elsewhere".into();
                }
                state.dirty_runs.insert(id.clone(), run.clone());
                let mut end = hook("SessionEnd");
                if captured {
                    end.assistant_messages
                        .push(hocus_proto::agent::CapturedMessage {
                            source_id: "s:m".into(),
                            turn_id: "t".into(),
                            text: "closing words".into(),
                            incomplete: false,
                        });
                }
                state.record_run_event(7, "claude", &end, 30);
                assert!(state.runs.is_empty());
                assert!(
                    state.run_lookups.is_empty(),
                    "closing never adopts a live pane"
                );
                if foreign != "neither" {
                    assert_eq!(state.dirty_runs[&id], run);
                    assert!(state.pending_messages.is_empty());
                    assert!(state.finished_runs.is_empty());
                } else {
                    assert_eq!(state.dirty_runs[&id].outcome, agents::RunOutcome::Completed);
                    assert_eq!(state.finished_runs, vec![id.clone()]);
                    assert_eq!(
                        state.dirty_runs[&id].entries.len(),
                        2 + usize::from(captured)
                    );
                    let closed = state.dirty_runs[&id].clone();
                    state.record_run_event(7, "claude", &end, 31);
                    assert_eq!(state.dirty_runs[&id], closed);
                    assert_eq!(state.finished_runs, vec![id]);
                }
            }
        }
    }

    #[test]
    fn late_capture_reconciles_persisted_stop_results_only_with_a_proven_match() {
        for kind in ["codex", "claude"] {
            for case in [
                "match",
                "other turn",
                "other text",
                "incomplete",
                "subagent",
                "legacy",
                "empty turn",
            ] {
                let mut state = State::default();
                let stop = wire::HookEvent {
                    turn_id: match case {
                        "legacy" => None,
                        "empty turn" => Some(String::new()),
                        _ => Some("t".into()),
                    },
                    ..stop_saying("done\r\nnow")
                };
                state.record_run_event(7, kind, &stop, 1);
                let run = &state.runs[&7];
                let saved = serde_json::to_string(run).unwrap();
                let original = run.entries[0].clone();
                let mut reloaded = State::default();
                reloaded
                    .runs
                    .insert(7, serde_json::from_str(&saved).unwrap());
                let turn = if case == "other turn" { "next" } else { "t" };
                let text = if case == "other text" {
                    "different"
                } else {
                    "done\nnow"
                };
                let mut capture = if kind == "codex" {
                    wire::HookEvent {
                        assistant_messages: vec![hocus_proto::agent::CapturedMessage {
                            source_id: "s:m".into(),
                            turn_id: turn.into(),
                            text: text.into(),
                            incomplete: case == "incomplete",
                        }],
                        ..hook("PostToolUse")
                    }
                } else {
                    reloaded.record_run_event(7, kind, &display(turn, "m", 0, false, text), 2);
                    wire::HookEvent {
                        message_incomplete: case == "incomplete",
                        ..display(turn, "m", 1, true, "")
                    }
                };
                if case == "subagent" {
                    capture.agent_id = Some("worker".into());
                }
                reloaded.record_run_event(7, kind, &capture, 3);
                let run = &reloaded.runs[&7];
                assert_eq!(run.entries[0], original);
                assert_eq!(
                    run.entries.len(),
                    if case == "match" { 1 } else { 2 },
                    "{kind}: {case}"
                );
                if case == "match" {
                    assert_eq!(run.revision, 1);
                    assert_eq!(run_delta(run).unwrap().matches(text).count(), 1);
                    assert!(reloaded.dirty_runs.is_empty());
                }
            }
        }
    }

    #[test]
    fn assistant_message_missing_batches_and_overflow_are_incomplete_on_run_close() {
        for event in [
            display("t", "missing", 1, true, "tail"),
            display("t", "interrupted", 0, false, "start"),
            display("t", "huge", 0, true, &"雪".repeat(400000)),
        ] {
            let mut state = State::default();
            state.record_run_event(7, "claude", &event, 1);
            let id = state.runs[&7].id.clone();
            state.record_run_event(7, "claude", &hook("SessionEnd"), 2);
            let entries = &state.dirty_runs[&id].entries;
            assert_eq!(entries.len(), 1);
            assert!(entries[0].text.contains("[incomplete message"));
            assert!(entries[0].text.len() <= 1024 * 1024 + 100);
            assert!(!state.pending_messages.contains_key(&7));
        }
    }

    #[test]
    fn assistant_message_buffers_leave_with_the_pane_without_writing_over_its_new_owner() {
        let mut state = State::default();
        state.record_run_event(7, "claude", &display("t", "m", 0, false, "pending"), 1);
        let id = state.runs[&7].id.clone();
        state.release_run(7, 2);
        assert!(!state.pending_messages.contains_key(&7));
        assert!(state.dirty_runs[&id].entries.is_empty());
    }

    #[test]
    fn assistant_subagent_messages_are_attributed_without_ending_the_parent_turn() {
        for kind in ["claude", "codex"] {
            let mut state = State::default();
            let event = wire::HookEvent {
                agent_id: Some("worker-7".into()),
                last_assistant_message: Some("worker finished".into()),
                ..hook("SubagentStop")
            };
            state.record_run_event(7, kind, &event, 1);
            state.record_run_event(7, kind, &event, 2);
            let run = &state.runs[&7];
            assert_eq!(run.outcome, agents::RunOutcome::Active);
            assert_eq!(run.entries.len(), 1);
            assert_eq!(run.entries[0].text, "Subagent worker-7:\nworker finished");
            assert_eq!(hocus_proto::agent::hook_state(&event), None);
        }
    }
    #[test]
    fn codex_assistant_message_windows_replay_once_and_keep_final_responses() {
        let mut state = State::default();
        state.record_run_event(7, "codex", &prompt("inspect the lexer"), 1);
        let event = wire::HookEvent {
            assistant_messages: vec![
                hocus_proto::agent::CapturedMessage {
                    source_id: "s:m1".into(),
                    turn_id: "t".into(),
                    text: "checking between tools".into(),
                    incomplete: false,
                },
                hocus_proto::agent::CapturedMessage {
                    source_id: "s:m2".into(),
                    turn_id: "t".into(),
                    text: "done".into(),
                    incomplete: false,
                },
            ],
            turn_id: Some("t".into()),
            ..hook("PostToolUse")
        };
        state.record_run_event(7, "codex", &event, 2);
        state.record_run_event(7, "codex", &event, 3);
        let stop = wire::HookEvent {
            hook_event_name: "Stop".into(),
            last_assistant_message: Some("done".into()),
            ..event
        };
        state.record_run_event(7, "codex", &stop, 4);
        assert_eq!(state.runs[&7].entries.len(), 3);
        assert_eq!(state.runs[&7].entries[1].text, "checking between tools");
        assert_eq!(state.runs[&7].entries[2].text, "done");
        state.record_run_event(
            7,
            "codex",
            &wire::HookEvent {
                turn_id: Some("next".into()),
                assistant_messages: Vec::new(),
                ..stop
            },
            5,
        );
        assert_eq!(
            state.runs[&7].entries.len(),
            4,
            "another turn keeps its Stop fallback"
        );
    }

    // -- Completions on another plugin's behalf ------------------------------------------

    /// One completion request as it left the daemon: the correlation id it will be answered
    /// on, and the body. Same shape as [`Asked`], and for the same reason — `completion_seq`
    /// is the daemon's, so a test that guessed at it would be asserting on its own setup.
    struct Requested {
        id: u64,
        body: serde_json::Value,
    }

    /// Everything one `on_complete_with` did: whether it went out, what went out, what was
    /// answered, and which pipes it blocked.
    struct Completed {
        sent: bool,
        requests: Vec<Requested>,
        replies: Vec<wire::CompletionReply>,
        blocked: Vec<String>,
    }

    fn complete(
        state: &mut State,
        source: &PipeSource,
        request: &wire::CompletionRequest,
    ) -> Completed {
        complete_configured(state, source, request, Some(&endpoint()), true, None)
    }

    /// The same, for the tests that are about the session's configuration rather than the
    /// caller's request.
    fn complete_configured(
        state: &mut State,
        source: &PipeSource,
        request: &wire::CompletionRequest,
        endpoint: Option<&Endpoint>,
        allowed: bool,
        pinned: Option<&str>,
    ) -> Completed {
        complete_raw(
            state,
            source,
            &serde_json::to_string(request).expect("encoded"),
            endpoint,
            allowed,
            pinned,
        )
    }

    /// The same again, from a payload that need not be a valid request.
    fn complete_raw(
        state: &mut State,
        source: &PipeSource,
        payload: &str,
        endpoint: Option<&Endpoint>,
        allowed: bool,
        pinned: Option<&str>,
    ) -> Completed {
        let mut requests = Vec::new();
        let mut replies = Vec::new();
        let mut blocked = Vec::new();
        let sent = state.on_complete_with(
            source,
            Some(payload),
            endpoint,
            allowed,
            pinned,
            |_, _, _, body, context| {
                requests.push(Requested {
                    id: context[COMPLETE_ID].parse().expect("a correlation id"),
                    body: serde_json::from_slice(&body).expect("a request body"),
                });
            },
            |_, reply| replies.push(reply.clone()),
            |pipe_id| blocked.push(pipe_id.to_owned()),
        );
        Completed {
            sent,
            requests,
            replies,
            blocked,
        }
    }

    /// The reply `finish_completion_with` handed back, and who it was addressed to.
    fn finish_completion(
        state: &mut State,
        status: u16,
        body: &[u8],
        id: u64,
    ) -> Option<(PipeSource, wire::CompletionReply)> {
        let mut answered = None;
        state.finish_completion_with(
            status,
            body,
            &BTreeMap::from([(COMPLETE_ID.to_owned(), id.to_string())]),
            |source, reply| answered = Some((source.clone(), reply.clone())),
        );
        answered
    }

    fn asking(text: &str) -> wire::CompletionRequest {
        wire::CompletionRequest {
            request_id: "hc1".to_owned(),
            messages: vec![wire::CompletionMessage {
                role: "user".to_owned(),
                content: text.to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// One ordinary completion body, as a provider would answer it.
    fn completion_body(content: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "model": "test-model",
            "choices": [{ "finish_reason": "stop", "message": { "content": content } }],
        }))
        .expect("encoded")
    }

    #[test]
    fn completion_transport_accepts_exactly_fitting_replies() {
        use prost::Message;
        use zellij_ipc::client_server_contract::client_server_contract::ServerToClientMsg;
        use zellij_ipc::MAX_IPC_FRAME_LEN;

        for source in [
            PipeSource::Plugin(PluginId::new(9)),
            PipeSource::Cli("pipe-name".repeat(20)),
        ] {
            let size = |reply: &wire::CompletionReply| {
                let json = serde_json::to_string(reply).unwrap();
                match &source {
                    PipeSource::Cli(pipe_name) => {
                        let frame: ServerToClientMsg =
                            zellij_ipc::ipc::ServerToClientMsg::CliPipeOutput {
                                pipe_name: pipe_name.clone(),
                                output: format!("{json}\n"),
                            }
                            .into();
                        frame.encoded_len()
                    },
                    _ => json.len(),
                }
            };
            let mut reply = wire::CompletionReply {
                request_id: "boundary".to_owned(),
                content: "x".repeat(MAX_IPC_FRAME_LEN - 1024),
                ..Default::default()
            };
            reply
                .content
                .push_str(&"x".repeat(MAX_IPC_FRAME_LEN - size(&reply)));
            assert_eq!(size(&reply), MAX_IPC_FRAME_LEN);
            let output = prepare_completion(&source, &reply);
            let back: wire::CompletionReply = serde_json::from_str(&output).unwrap();
            assert!(back.error.is_empty());
            assert_eq!(back.content, reply.content);
            if matches!(source, PipeSource::Cli(_)) {
                assert!(output.ends_with('\n'));
                assert!(
                    output.len() < MAX_IPC_FRAME_LEN,
                    "the envelope consumes budget"
                );
            }
            drop((output, back));
            reply.content.push('x');
            assert_eq!(size(&reply), MAX_IPC_FRAME_LEN + 1);
            let back: wire::CompletionReply =
                serde_json::from_str(&prepare_completion(&source, &reply)).unwrap();
            assert_eq!(back.request_id, reply.request_id);
            assert!(back.error.contains(&MAX_IPC_FRAME_LEN.to_string()));
            assert!(back.content.is_empty());
            assert!(back.tool_calls.is_empty());
        }
    }

    #[test]
    fn oversized_completion_tool_arguments_and_http_errors_reach_both_callers() {
        use zellij_ipc::MAX_IPC_FRAME_LEN;

        // Escaping makes the reply exceed the budget even though the raw arguments fit.
        let body = serde_json::to_vec(&serde_json::json!({
            "choices": [{"message": {"tool_calls": [{
                "id": "call-1", "function": {"name": "save", "arguments": "\"".repeat(MAX_IPC_FRAME_LEN / 2)}
            }]}}]
        })).unwrap();
        let http_error =
            format!("web response exceeded the {MAX_IPC_FRAME_LEN}-byte buffered-body limit");
        for source in [
            PipeSource::Plugin(PluginId::new(9)),
            PipeSource::Cli("cli-pipe".to_owned()),
        ] {
            for (status, body, expected) in [
                (200, body.as_slice(), "transport limit"),
                // The host maps a bounded-reader error to WebRequestResult(400, ..., error, context).
                (400, http_error.as_bytes(), "buffered-body limit"),
            ] {
                let mut state = State::default();
                let request = complete(&mut state, &source, &asking("why?"));
                assert!(request.sent);
                let output = std::cell::RefCell::new(None);
                let unblocked = std::cell::Cell::new(false);
                state.finish_completion_with(
                    status,
                    body,
                    &BTreeMap::from([(COMPLETE_ID.to_owned(), request.requests[0].id.to_string())]),
                    |caller, reply| {
                        answer_completion_with(
                            caller,
                            reply,
                            |message| {
                                assert_eq!(
                                    source,
                                    PipeSource::Plugin(message.destination_plugin_id.unwrap())
                                );
                                assert_eq!(message.message_name, wire::COMPLETE_RESULT);
                                *output.borrow_mut() = message.message_payload;
                            },
                            |pipe, json| {
                                assert_eq!(source, PipeSource::Cli(pipe.to_owned()));
                                assert!(json.ends_with('\n'));
                                assert!(!unblocked.get(), "write before releasing the CLI");
                                *output.borrow_mut() = Some(json.to_owned());
                            },
                            |_| unblocked.set(true),
                        )
                    },
                );
                assert!(state.completions.is_empty());
                assert_eq!(unblocked.get(), matches!(source, PipeSource::Cli(_)));
                let reply: wire::CompletionReply =
                    serde_json::from_str(&output.into_inner().unwrap()).unwrap();
                assert_eq!(reply.request_id, "hc1");
                assert!(reply.error.contains(expected), "{}", reply.error);
                assert!(reply.content.is_empty());
                assert!(reply.tool_calls.is_empty());
            }
        }
    }

    #[test]
    fn a_completion_goes_out_correlated_and_comes_back_to_the_caller_that_asked() {
        let mut state = State::default();
        let caller = PipeSource::Plugin(PluginId::new(9));

        let out = complete(&mut state, &caller, &asking("why is the build red?"));

        assert!(out.sent);
        assert!(
            out.replies.is_empty(),
            "nothing to say until the model does"
        );
        assert_eq!(out.requests.len(), 1);
        assert_eq!(out.requests[0].body["model"], "test-model");
        assert_eq!(out.requests[0].body["stream"], false);
        assert_eq!(
            out.requests[0].body["messages"][0]["content"],
            "why is the build red?"
        );

        let (source, reply) = finish_completion(
            &mut state,
            200,
            &completion_body("a stale lockfile"),
            out.requests[0].id,
        )
        .expect("answered");

        assert_eq!(source, caller);
        assert_eq!(reply.request_id, "hc1", "the caller's own id, echoed");
        assert_eq!(reply.content, "a stale lockfile");
        assert!(reply.error.is_empty());
        assert!(
            state.completions.is_empty(),
            "the slot is given back once the answer is in"
        );
    }

    /// The context key is the whole of what keeps the two lanes apart, so the request must
    /// carry this one and none of the summary lane's.
    #[test]
    fn a_completion_carries_its_own_correlation_key_and_not_a_summarys() {
        let mut state = State::default();
        let mut context = None;
        state.on_complete_with(
            &PipeSource::Plugin(PluginId::new(9)),
            Some(&serde_json::to_string(&asking("why?")).unwrap()),
            Some(&endpoint()),
            true,
            None,
            |_, _, _, _, sent| context = Some(sent),
            |_, _| {},
            |_| {},
        );
        let context = context.expect("sent");
        assert!(context.contains_key(COMPLETE_ID));
        assert!(!context.contains_key(SUMMARY_PANE));
        assert!(!context.contains_key(SUMMARY_ID));
        assert!(!context.contains_key(SUMMARY_RUN));
    }

    /// The split in the `WebRequestResult` arm, from the other side: a summary's context
    /// names no completion, so the completion lane must leave it entirely alone.
    #[test]
    fn a_summarys_answer_is_not_taken_up_by_the_completion_lane() {
        let mut state = State::default();
        let out = complete(
            &mut state,
            &PipeSource::Plugin(PluginId::new(9)),
            &asking("why?"),
        );
        assert!(out.sent);

        let mut answered = false;
        state.finish_completion_with(
            200,
            &completion_body("not yours"),
            &BTreeMap::from([
                (SUMMARY_PANE.to_owned(), "7".to_owned()),
                (SUMMARY_ID.to_owned(), "1".to_owned()),
            ]),
            |_, _| answered = true,
        );

        assert!(!answered);
        assert_eq!(state.completions.len(), 1, "still waiting for its own");
    }

    /// A reply for a request that has already been answered or aged out is nobody's, and
    /// must not be handed to whoever holds that slot next.
    #[test]
    fn a_reply_for_a_completion_nobody_is_waiting_for_is_dropped() {
        let mut state = State::default();
        assert!(finish_completion(&mut state, 200, &completion_body("late"), 99).is_none());
    }

    #[test]
    fn a_completion_is_refused_when_the_session_switched_them_off() {
        let mut state = State::default();
        let out = complete_configured(
            &mut state,
            &PipeSource::Plugin(PluginId::new(9)),
            &asking("why?"),
            Some(&endpoint()),
            false,
            None,
        );

        assert!(!out.sent);
        assert!(out.requests.is_empty());
        assert_eq!(out.replies.len(), 1);
        assert_eq!(out.replies[0].request_id, "hc1");
        assert!(out.replies[0].error.contains("plugin_completions"));
        assert!(state.completions.is_empty(), "nothing is holding a slot");
    }

    /// A refusal, not silence: a caller cannot tell an unconfigured session from one with no
    /// daemon in it, and the reply is the only thing that says which.
    #[test]
    fn a_completion_is_refused_with_no_api_key_and_says_what_to_set() {
        let mut state = State::default();
        let out = complete_configured(
            &mut state,
            &PipeSource::Plugin(PluginId::new(9)),
            &asking("why?"),
            None,
            true,
            None,
        );

        assert!(!out.sent);
        assert_eq!(out.replies.len(), 1);
        assert!(out.replies[0].error.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn invalid_completion_payloads_are_answered_with_any_recoverable_request_id() {
        for (payload, request_id) in [
            ("{not json", ""),
            (r#"{"request_id":"caller-9","messages":{}}"#, "caller-9"),
            (r#"{"request_id":"caller-9","messages":[]"#, ""),
            (r#"{"messages":{}}"#, ""),
            (r#"{"request_id":9,"messages":{}}"#, ""),
            (r#"{"request_id":null,"messages":{}}"#, ""),
        ] {
            let mut state = State::default();
            let out = complete_raw(
                &mut state,
                &PipeSource::Plugin(PluginId::new(9)),
                payload,
                Some(&endpoint()),
                true,
                None,
            );

            assert!(!out.sent);
            assert!(out.requests.is_empty());
            assert_eq!(out.replies.len(), 1);
            assert!(out.replies[0].error.contains("not a completion request"));
            assert_eq!(out.replies[0].request_id, request_id);
            assert!(state.completions.is_empty());
            assert_eq!(state.completion_seq, 0);
        }
    }

    #[test]
    fn oversized_json_is_rejected_before_parsing_or_claiming_a_completion_slot() {
        let mut large_array = asking("why?");
        large_array.messages = vec![large_array.messages[0].clone(); completion::MAX_REQUEST_BYTES];
        large_array.tools = vec![serde_json::json!({}); completion::MAX_REQUEST_BYTES];
        for request in [
            asking(&"x".repeat(completion::MAX_REQUEST_BYTES)),
            large_array,
        ] {
            let payload = serde_json::to_string(&request).expect("valid JSON with a request ID");
            for (allowed, full) in [(true, false), (false, false), (true, true)] {
                let mut state = State::default();
                if full {
                    for id in 0..COMPLETIONS_TOTAL {
                        assert!(
                            complete(
                                &mut state,
                                &PipeSource::Cli(format!("busy-{id}")),
                                &asking("why?"),
                            )
                            .sent
                        );
                    }
                }
                let slots = state.completions.len();
                let sequence = state.completion_seq;
                let out = complete_raw(
                    &mut state,
                    &PipeSource::Cli("oversized".to_owned()),
                    &payload,
                    Some(&endpoint()),
                    allowed,
                    None,
                );
                assert!(!out.sent);
                assert!(out.requests.is_empty());
                assert_eq!(out.blocked, ["oversized"]);
                assert_eq!(out.replies.len(), 1);
                assert_eq!(out.replies[0].request_id, "");
                assert!(out.replies[0].error.contains("at most 64000 bytes"));
                assert_eq!(state.completions.len(), slots);
                assert_eq!(state.completion_seq, sequence);
            }
        }
    }

    /// `validate`'s refusals reach the caller in the daemon's reply rather than as silence.
    #[test]
    fn a_request_with_no_messages_is_refused_in_words() {
        let mut state = State::default();
        let out = complete(
            &mut state,
            &PipeSource::Plugin(PluginId::new(9)),
            &wire::CompletionRequest {
                request_id: "hc1".to_owned(),
                ..Default::default()
            },
        );

        assert!(!out.sent);
        assert!(out.replies[0].error.contains("at least one message"));
        assert_eq!(out.replies[0].request_id, "hc1", "echoed even in a refusal");
    }

    /// The brake, not a budget: a plugin in a loop stalls on its own answers.
    #[test]
    fn one_caller_may_only_have_so_many_completions_in_flight() {
        let mut state = State::default();
        let caller = PipeSource::Plugin(PluginId::new(9));
        for _ in 0..COMPLETIONS_PER_CALLER {
            assert!(complete(&mut state, &caller, &asking("why?")).sent);
        }

        let out = complete(&mut state, &caller, &asking("why?"));
        assert!(!out.sent);
        assert!(out.replies[0].error.contains("at a time"));

        // Another plugin is another caller, and is not held up by this one.
        assert!(
            complete(
                &mut state,
                &PipeSource::Plugin(PluginId::new(10)),
                &asking("why?")
            )
            .sent
        );
        // Nor is somebody at a shell.
        assert!(
            complete(
                &mut state,
                &PipeSource::Cli("pipe-1".to_owned()),
                &asking("why?")
            )
            .sent
        );
    }

    #[test]
    fn the_session_as_a_whole_has_a_ceiling_too() {
        let mut state = State::default();
        for caller in 0..(COMPLETIONS_TOTAL / COMPLETIONS_PER_CALLER) {
            for _ in 0..COMPLETIONS_PER_CALLER {
                assert!(
                    complete(
                        &mut state,
                        &PipeSource::Plugin(PluginId::new(caller as u32)),
                        &asking("why?")
                    )
                    .sent
                );
            }
        }

        let out = complete(
            &mut state,
            &PipeSource::Plugin(PluginId::new(99)),
            &asking("why?"),
        );

        assert!(!out.sent);
        assert!(out.replies[0].error.contains("in flight"));
    }

    /// With nothing pinned the caller's model wins, because a plugin that knows it needs a
    /// reasoning model should be able to say so.
    #[test]
    fn a_callers_model_is_used_unless_the_session_pinned_one() {
        let mut state = State::default();
        let mut request = asking("why?");
        request.model = Some("gpt-5".to_owned());

        let out = complete(&mut state, &PipeSource::Plugin(PluginId::new(9)), &request);
        assert_eq!(out.requests[0].body["model"], "gpt-5");

        let mut state = State::default();
        let out = complete_configured(
            &mut state,
            &PipeSource::Plugin(PluginId::new(9)),
            &request,
            Some(&endpoint()),
            true,
            Some("cheap-model"),
        );
        assert_eq!(
            out.requests[0].body["model"], "cheap-model",
            "the session's answer to a caller's choice of price"
        );
    }

    /// The caller's tools travel verbatim, which is what makes a plugin-driven tool loop
    /// possible at all.
    #[test]
    fn a_callers_tools_and_their_results_make_the_round_trip() {
        let mut state = State::default();
        let mut request = asking("where is Oslo?");
        request.tools = vec![serde_json::json!({
            "type": "function",
            "function": { "name": "lookup", "parameters": { "type": "object" } },
        })];

        let out = complete(&mut state, &PipeSource::Plugin(PluginId::new(9)), &request);
        assert_eq!(
            out.requests[0].body["tools"][0]["function"]["name"],
            "lookup"
        );

        let body = serde_json::to_vec(&serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": { "content": null, "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "lookup", "arguments": "{\"city\":\"Oslo\"}" },
                }] },
            }],
        }))
        .expect("encoded");
        let (_, reply) =
            finish_completion(&mut state, 200, &body, out.requests[0].id).expect("answered");

        assert_eq!(reply.finish_reason, "tool_calls");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "lookup");
        assert_eq!(reply.tool_calls[0].arguments, r#"{"city":"Oslo"}"#);
    }

    /// A provider's refusal is the caller's answer, not the daemon's silence.
    #[test]
    fn a_rate_limit_reaches_the_caller_in_the_providers_words() {
        let mut state = State::default();
        let out = complete(
            &mut state,
            &PipeSource::Plugin(PluginId::new(9)),
            &asking("why?"),
        );

        let (_, reply) = finish_completion(
            &mut state,
            429,
            br#"{"error":{"message":"Rate limit reached"}}"#,
            out.requests[0].id,
        )
        .expect("answered");

        assert!(reply.error.contains("Rate limit reached"));
        assert_eq!(reply.request_id, "hc1");
        assert!(state.completions.is_empty(), "a failure frees the slot too");
    }

    /// The whole reason this lane ages out at all: a `zellij pipe` is a blocked process, and
    /// a `WebRequestResult` the host dropped would hang it for the life of the session.
    #[test]
    fn a_completion_that_never_answers_is_timed_out_and_its_slot_freed() {
        let mut state = State::default();
        let caller = PipeSource::Cli("pipe-1".to_owned());
        assert!(complete(&mut state, &caller, &asking("why?")).sent);

        let mut answered = Vec::new();
        for _ in 0..COMPLETE_TICKS {
            state.age_completions_with(|source, reply| {
                answered.push((source.clone(), reply.clone()))
            });
        }
        assert!(answered.is_empty(), "still within its patience");

        state.age_completions_with(|source, reply| answered.push((source.clone(), reply.clone())));

        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0].0, caller);
        assert_eq!(answered[0].1.request_id, "hc1");
        assert!(answered[0].1.error.contains("no answer"));
        assert!(state.completions.is_empty());
        // And the caller may ask again, which is what freeing the slot was for.
        assert!(complete(&mut state, &caller, &asking("why?")).sent);
    }

    /// Blocked before anything that can answer, so the block and its release are paired on
    /// every path — including the refusals, which answer immediately.
    #[test]
    fn a_cli_caller_is_blocked_on_every_path_including_a_refusal() {
        let mut state = State::default();
        let caller = PipeSource::Cli("pipe-1".to_owned());

        let out = complete(&mut state, &caller, &asking("why?"));
        assert_eq!(out.blocked, vec!["pipe-1".to_owned()]);

        let refused = complete_configured(
            &mut state,
            &PipeSource::Cli("pipe-2".to_owned()),
            &asking("why?"),
            None,
            true,
            None,
        );
        assert_eq!(refused.blocked, vec!["pipe-2".to_owned()]);
        assert_eq!(refused.replies.len(), 1, "answered, so released");
    }

    /// Somebody at a shell has nothing to correlate — the answer comes back down their own
    /// pipe — so an empty id must be an ordinary request rather than a refusal.
    #[test]
    fn a_request_with_no_id_is_ordinary() {
        let mut state = State::default();
        let out = complete_raw(
            &mut state,
            &PipeSource::Cli("pipe-1".to_owned()),
            r#"{"messages":[{"role":"user","content":"why?"}]}"#,
            Some(&endpoint()),
            true,
            None,
        );

        assert!(out.sent);
        let (_, reply) = finish_completion(
            &mut state,
            200,
            &completion_body("because"),
            out.requests[0].id,
        )
        .expect("answered");
        assert!(reply.request_id.is_empty());
        assert_eq!(reply.content, "because");
    }
}
