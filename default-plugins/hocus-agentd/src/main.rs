//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! The daemon takes one startup snapshot, follows Zellij's pane, command, and CWD
//! events, and samples agent output on a small heartbeat for activity and Git changes.

mod poll;

use hocus_proto::agents::now_ms;
use hocus_proto::llm::Endpoint;
use hocus_proto::location::Location;
use hocus_proto::panes::{focused_pane, focused_plugin_pane, manifest_pane_ids, terminal_panes};
use hocus_proto::store::{self, Backend};
use hocus_proto::wire::{self, PollReply, PollRequest};
use hocus_proto::{agents, projects};
use std::collections::{BTreeMap, BTreeSet};
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

/// How much of the user's request is kept to give the summary its question.
///
/// Generous next to the row-level caps around it, because this never reaches a row: it
/// goes to the model beside the agent's answer, where a truncated question is worse than
/// a long one. Capped at all only because a prompt can carry a pasted file.
const PROMPT_CHARS: usize = 600;

/// The kind a hook event proves its pane is running.
///
/// Claude Code is the only agent that reports for itself, so this is a constant
/// rather than something read off the event: nothing else sends `agent_hook`.
const CLAUDE: &str = "claude";

#[derive(Default)]
struct State {
    /// Overrides from hocus's `agent_register`, refreshed by every subscribe.
    /// hocus stays authoritative; this is only an input to `classify`.
    manual_kinds: BTreeMap<u32, String>,
    /// The tab-to-project assignments the user made in hocus, held only to pass
    /// on. Nothing here reads them: the store names the tabs, and this is the
    /// only route from the pane that makes them to the plugin that uses them.
    tab_projects: BTreeMap<usize, String>,
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
    /// The plugin pane the user is looking at, passed on for the same reason and to
    /// the same reader — it is how hocus tells the agents pane above it from itself.
    focused_plugin: Option<u32>,
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
    hosts: BTreeMap<u32, String>,
    activity: agents::Activity,
    /// The checkout each agent pane's frame is currently decorated with.
    ///
    /// A record of what the server has been *told*, not of what is true, so a
    /// steady session makes no host calls at all: the wanted set is recomputed
    /// on every publish and only a difference is sent. See
    /// `refresh_title_prefixes_with`.
    title_prefixes: BTreeMap<u32, String>,
    /// What Git said about each folder, keyed by folder *and machine*.
    repositories: BTreeMap<Location, projects::Repository>,
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
    subscribers: Vec<u32>,
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
    zellij_session_name: Option<String>,
    runs: BTreeMap<u32, agents::AgentRun>,
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
    /// after the `PermissionRequest` for the same prompt must not overwrite the more precise
    /// account of it - and a complete record has no way to express "keep what you had". The
    /// manifest road is for a pane that reports some other way: one hocus spawned, an
    /// `agent-report --state` script, a plugin speaking for an agent.
    event_fed: BTreeSet<u32>,
    /// The panes now waiting on something nobody was told about.
    ///
    /// A `PermissionRequest` under an unattended mode is drawn but not announced, on the
    /// grounds that the mode is about to answer it. `bypassPermissions` and `dontAsk` really
    /// do; `auto` runs a classifier and puts whatever it will not clear in front of the user
    /// exactly as `default` would - see `agent::UNATTENDED_MODES`. That prompt's only other
    /// signal is the `Notification` six seconds later, and by then the feed is already
    /// `Waiting`, so there is no busy→waiting edge left for it to cross.
    ///
    /// This is that edge, held open. A wait suppressed on the way in leaves its pane here; the
    /// backstop takes it out again and toasts, which is the right reading of a notification
    /// that fired at all - a prompt the mode had answered would never have gone six seconds
    /// unanswered.
    suppressed_waits: BTreeSet<u32>,
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
                    | agents::RunEntryKind::Prompt
            )
        })
        .map(|entry| format!("{}: {}", entry.kind.label(), entry.text))
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

        let _ = store::Host::shared().define_all(&store::shared_defs());

        self.bootstrap();
        set_timeout(ACTIVITY_SECONDS);
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
                self.on_command(id, command, is_foreground);
                self.publish();
            },
            Event::CwdChanged(PaneId::Terminal(id), cwd, _) => {
                self.record_cwd(id, &cwd.to_string_lossy());
                self.rejoin_cwds();
                self.resolve_repositories();
                // A remote pane's folder is probed on the far machine, so a `cd`
                // is attributed on the next ask rather than by the pass above.
                // Asked here as well as on the heartbeat so it lands now rather
                // than in five seconds.
                self.refresh_remote_repositories();
                self.publish();
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
                self.finish_summary(status, &body, &context);
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
                self.poll_window_ms = (elapsed.max(ACTIVITY_SECONDS) * 1500.0) as u64;
                self.observe_activity();
                self.repair_commands();
                self.repair_cwds();
                self.expire_probe_backoff(elapsed);
                // Every tick, not on the Git refresh interval: this reads a
                // cache the far machine keeps fresh on its own clock, so waiting
                // a minute would only add latency to an answer already sitting
                // there. It publishes nothing unless something changed.
                self.refresh_remote_repositories();
                self.git_refresh_elapsed += elapsed;
                if self.git_refresh_elapsed >= GIT_REFRESH_SECONDS {
                    self.git_refresh_elapsed %= GIT_REFRESH_SECONDS;
                    self.refresh_repositories();
                }
                self.publish();
                set_timeout(ACTIVITY_SECONDS);
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
        if pipe_message.name == wire::AGENT_HOOK {
            // The one message here a shell sends. Both halves are dropped in
            // silence when they do not parse: this arrives from outside the
            // session, on a `PipeSource::Cli` there is nothing to answer, and a
            // Claude Code that grows a field we have never seen must cost us
            // nothing.
            // The session says which pane the pipe came from; the `pane_id=` argument is
            // what a hook script from before it could say instead.
            let pane_id = match pipe_message.pane_id {
                Some(PaneId::Terminal(id)) => Some(id),
                _ => pipe_message
                    .args
                    .get(wire::HOOK_PANE_ID)
                    .and_then(|id| id.parse::<u32>().ok()),
            };
            // Which agent it came from. The payload is the agent's own JSON and never says,
            // and this daemon writes a record of its own from it - so without this every
            // pane that reports for itself would be a `claude`. Normalised the way
            // `agent-report` normalises its `--kind`; absent, it is what every script old
            // enough to omit it was installed for.
            let kind = pipe_message
                .args
                .get(wire::HOOK_KIND)
                .map(|kind| kind.trim().to_lowercase())
                .filter(|kind| !kind.is_empty())
                .unwrap_or_else(|| CLAUDE.to_owned());
            let event = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::HookEvent>(json).ok());
            if let (Some(pane_id), Some(event)) = (pane_id, event) {
                self.on_hook_of(pane_id, &kind, &event);
            }
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
            focused_plugin,
        }) = poll::gather()
        else {
            return;
        };
        self.set_zellij_session_name(session_name.clone());
        if let Ok(rows) = store::Host::shared()
            .fetch(&agents::rows::active_runs_request(&session_name).limit(MAX_FETCH_ROWS))
        {
            let mut runs = agents::rows::runs_from_rows(&rows);
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
        self.focused_plugin = focused_plugin;
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

    fn set_zellij_session_name(&mut self, name: String) {
        if self.zellij_session_name.as_ref() == Some(&name) {
            return;
        }
        self.zellij_session_name = Some(name.clone());
        for run in self.runs.values_mut() {
            run.zellij_session_name = name.clone();
            self.dirty_runs.insert(run.id.clone(), run.clone());
        }
        for run in self.dirty_runs.values_mut() {
            run.zellij_session_name = name.clone();
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
            |pane_id, text| notify_with_pane_id(text, PaneId::Terminal(pane_id)),
            |pane_id, title| rename_pane_with_id(PaneId::Terminal(pane_id), title),
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
            self.suppressed_waits.remove(&pane_id);
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
                    self.prompts
                        .insert(pane_id, prompt.chars().take(PROMPT_CHARS).collect());
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
                    // The `Notification` that follows a `PermissionRequest` describes the
                    // same prompt in worse words — "Claude needs your permission to use
                    // Bash" where the request itself said `Bash cargo`. It is the backstop
                    // for a prompt nothing else reported, so it fills a gap and never
                    // overwrites: one wait keeps the first and most precise account of
                    // itself. Everything else here is a first-class statement of what is
                    // being asked, and does replace what came before it.
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
        // The edge into a wait, not the state, and the edge on the *phase* alone.
        //
        // `PermissionRequest` puts the prompt up and the `Notification` for that same
        // prompt follows about six seconds later — and the two word it differently, one
        // naming the tool and the other writing the sentence Claude Code would have shown.
        // So an agent blocked once must not interrupt twice, and comparing the two
        // descriptions is exactly the comparison that would let it: they are two accounts
        // of one prompt, not two prompts.
        //
        // Nothing is lost by that. Between two genuine questions the agent works — it acts
        // on the first answer, and the `PostToolUse` for that is a `busy` — so a real
        // second question crosses this edge on its own.
        let entered_wait = phase == agents::HookPhase::Waiting
            && previous
                .as_ref()
                .is_none_or(|was| was.phase != agents::HookPhase::Waiting);
        // The one thing that reopens a spent edge: a wait entered under a mode that was
        // expected to answer it, and did not. See `suppressed_waits`. Taken out of the set
        // here whether or not this event announces it, because either way the edge it was
        // holding has now been used.
        let held_edge = self.suppressed_waits.remove(&pane_id);
        let announce = phase == agents::HookPhase::Waiting
            && agents::waiting_needs_a_person(event)
            && (entered_wait || held_edge);
        // Not over the very pane it describes: the user is looking at the question. The same
        // rule the summary toast keeps — and the reason this is not part of `announce`, which
        // is about the wait having been spoken for rather than about it being seen.
        if announce && self.focused != Some(pane_id) {
            let kind = self.toast_kind(pane_id);
            toast(
                pane_id,
                &agents::waiting_toast(pane_id, kind.as_deref(), &feed),
            );
        }
        // A wait still standing that nobody has been told about, whether it started that way
        // or a second silent event replaced the first. The edge stays open for its backstop;
        // anything that ends the wait takes it out above and leaves it out.
        if phase == agents::HookPhase::Waiting && !announce && (entered_wait || held_edge) {
            self.suppressed_waits.insert(pane_id);
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
        if event.hook_event_name == agents::HOOK_SESSION_END {
            self.finish_run(pane_id, agents::RunOutcome::Completed, now_ms);
            return;
        }
        let zellij_session_name = self.zellij_session_name.clone().unwrap_or_default();
        if !self.runs.contains_key(&pane_id) {
            // The manifest usually gets here first, but a hook can beat it: the pane arrives
            // and the agent speaks before the next `PaneUpdate`. Looking again costs one fetch
            // on the first event of a run and nothing after.
            self.adopt_stored_run(pane_id, kind, &event.session_id, &zellij_session_name);
        }
        // Read after the adoption above: a run it picked up is not a new one.
        let new_run = !self.runs.contains_key(&pane_id);
        // The machine the pane's process is on, read out here because the entry below borrows
        // `self.runs`. A hook reports the agent's folder and nothing about where that folder
        // is; the pane is the only thing that knows, and without it a run recorded on a build
        // box is indistinguishable from one recorded here.
        let host = self.hosts.get(&pane_id).cloned();
        let run = self
            .runs
            .entry(pane_id)
            .or_insert_with(|| agents::AgentRun {
                id: agents::run_id(kind, pane_id, &event.session_id, now_ms),
                pane_id,
                zellij_session_name,
                session_id: event.session_id.clone(),
                kind: kind.to_owned(),
                cwd: event.cwd.clone(),
                host: host.clone(),
                started_at_ms: now_ms,
                ended_at_ms: None,
                outcome: agents::RunOutcome::Active,
                revision: 0,
                summarized_revision: 0,
                summary: None,
                entries: Vec::new(),
            });
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
        // Kept level with the pane on every hook and not only at the start, so a run whose
        // pane was warped to another machine stops claiming the folder it had here.
        if run.host != host {
            run.host = host;
            changed = true;
        }
        if let Some((kind, text)) = agents::run_entry(event) {
            let event_id = agents::run_event_id(event);
            let duplicate = match event_id.as_ref() {
                Some(event_id) => run
                    .entries
                    .iter()
                    .any(|entry| entry.event_id.as_ref() == Some(event_id)),
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
    }

    /// Look for a stored run this pane's agent already has, from what the event itself says.
    ///
    /// The hook-side twin of `adopt_arrived_runs`. It has no record to read, so it builds the
    /// candidate id from the event: the same derivation, from the same session id the record
    /// would have carried.
    fn adopt_stored_run(&mut self, pane_id: u32, kind: &str, session_id: &str, owner: &str) {
        if session_id.trim().is_empty() || owner.is_empty() {
            return;
        }
        if !self.run_lookups.insert(pane_id) {
            return;
        }
        let run_id = agents::run_id(kind, pane_id, session_id, 0);
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
        set_pane_agent_run(PaneId::Terminal(pane_id), run_id);
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
    fn dirty_run_mutations(&self, owner: &str) -> (Vec<Mutation>, Vec<(String, u64)>) {
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
                mutations.push(agents::rows::entry_mutation(&run.id, entry));
            }
            if let Some(newest) = run.entries.last().map(|entry| entry.revision) {
                written.push((run.id.clone(), newest.max(from)));
            }
        }
        (mutations, written)
    }

    fn persist_dirty_runs(&mut self) {
        let Some(owner) = self.zellij_session_name.clone() else {
            return;
        };
        if self.dirty_runs.is_empty() {
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
                    run.entries
                        .iter()
                        .rev()
                        .find(|entry| entry.kind == agents::RunEntryKind::Result)
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
        self.focused_plugin = focused_plugin_pane(&self.manifest, &self.tabs);
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
        for pane in self.manifest.panes.values().flatten() {
            if let Some(host) = pane.host.clone() {
                self.hosts.insert(pane.id, host);
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
        self.suppressed_waits.retain(|id| live.contains(id));
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
                self.suppressed_waits.remove(&pane.pane_id);
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
    /// moment it lands — but the run behind it is a row in a store shared by every session on
    /// this machine, under an id that means nothing to a pane id or a session name. This is
    /// where that id is looked up.
    ///
    /// Driven off the manifest rather than off the next hook, because an agent that arrives
    /// mid-think may say nothing for minutes, and until it does the run would sit in the
    /// archive marked closed while the pane it belongs to is plainly working.
    fn adopt_arrived_runs(&mut self) {
        let Some(owner) = self.zellij_session_name.clone() else {
            return;
        };
        let candidates: Vec<(u32, String)> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| !self.runs.contains_key(&pane.pane_id))
            .filter(|pane| !self.run_lookups.contains(&pane.pane_id))
            .filter_map(|pane| {
                Some((
                    pane.pane_id,
                    agents::stored_run_id(pane.agent.as_ref()?, pane.pane_id)?,
                ))
            })
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

    /// The folder a pane is in, with the machine it is on.
    ///
    /// The pane is the only thing that knows both: `cwds` holds the path, and
    /// the manifest holds the host. A pane nothing has reported a folder for has
    /// no location at all, which is what keeps it from being probed under a
    /// half-known one.
    fn pane_location(&self, pane: &agents::AgentPane) -> Option<Location> {
        self.location_of(pane.pane_id)
            .or_else(|| Some(Location::new(pane.host.clone(), pane.cwd.as_deref()?)))
    }

    /// Where a pane is, by id alone.
    ///
    /// Both halves come out of the maps rather than off the pane list, so a pane
    /// the list has dropped — suppressed behind the scrollback editor — still
    /// answers, and the folder probed for it is not thrown away while it is
    /// hidden.
    fn location_of(&self, pane_id: u32) -> Option<Location> {
        let cwd = self.cwds.get(&pane_id)?;
        Some(Location::new(self.hosts.get(&pane_id).cloned(), cwd))
    }

    /// Every folder a pane we still know of is in.
    ///
    /// Keyed off `cwds` rather than the visible pane list, because that is what
    /// decides how long a folder is remembered — see `hosts`.
    fn pane_locations(&self) -> Vec<Location> {
        self.cwds
            .keys()
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

    /// Ask the host what Git said about each remote pane's folder.
    ///
    /// A separate path from `start_git_probes`, and deliberately not routed
    /// through `git_probes`: that machinery exists to join two asynchronous
    /// `RunCommandResult` events back together, and this is one synchronous call
    /// that returns an answer the far machine has already computed. Reusing it
    /// would mean inventing a probe id and waiting for an event that never
    /// comes.
    ///
    /// The answer is filed under the folder the probe was *run in*, not under
    /// wherever the pane is now. The far machine probes on its own clock, so a
    /// pane that has just moved reports the folder it came from; filing that
    /// under the new folder would claim a checkout the pane has left. Filed
    /// under its own folder instead, the entry is simply a fact about a folder,
    /// and the ancestor walk in hocus still offers it as a project while the
    /// exact answer is on its way — which is precisely what a local pane does
    /// between a `cd` and its next probe.
    ///
    /// `probe_of` is injected for the reason `repair_cwds_with` and
    /// `repair_commands_with` are: it reaches the host, and this has to stay
    /// testable on the host triple.
    fn refresh_remote_repositories_with(
        &mut self,
        mut probe_of: impl FnMut(u32) -> Option<PaneGitProbe>,
    ) {
        let remote: Vec<u32> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| pane.host.is_some())
            .filter(|pane| pane.status == agents::AgentStatus::Running)
            .map(|pane| pane.pane_id)
            .collect();
        if remote.is_empty() {
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
            .filter(unknown)
            .chain(rotation.iter().copied().filter(|id| !unknown(id)))
            .take(REMOTE_PROBES_PER_TICK)
            .collect();
        self.remote_probe_cursor = (self.remote_probe_cursor + due.len()) % remote.len();

        let mut changed = false;
        for pane_id in due {
            let Some(probe) = probe_of(pane_id) else {
                continue;
            };
            let location = Location::new(Some(probe.host), probe.cwd);
            let outcome = poll::repository(probe.exit_code, &probe.metadata, &probe.config);
            changed |= self.record_probe_outcome(location, outcome);
        }
        // Only on change. This runs on every heartbeat, and a snapshot is the
        // whole session's panes and folders pushed to every subscriber.
        if changed {
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
            .is_some_and(|ms| ms <= self.poll_window());
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
            |pane_id, text| notify_with_pane_id(text, PaneId::Terminal(pane_id)),
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
            run.entries
                .iter()
                .rev()
                .find(|entry| entry.kind == agents::RunEntryKind::Prompt)
                .map(|entry| entry.text.as_str()),
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
            .find(|entry| entry.kind == agents::RunEntryKind::Result)
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
            |pane_id, title| rename_pane_with_id(PaneId::Terminal(pane_id), title),
            |pane_id, text| notify_with_pane_id(text, PaneId::Terminal(pane_id)),
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
        self.suppressed_waits.remove(&pane_id);
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
            set_pane_title_prefix(PaneId::Terminal(pane_id), prefix)
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

    fn publish(&mut self) {
        self.publish_with_force(false);
    }

    /// `publish` even when nothing changed: for a subscriber that just arrived
    /// and holds no snapshot at all, to whom "unchanged" means nothing.
    fn publish_forced(&mut self) {
        self.publish_with_force(true);
    }

    fn publish_with_force(&mut self, force: bool) {
        self.refresh_title_prefixes();
        let reply = PollReply {
            panes: self.panes.clone(),
            activity: self.activity.clone(),
            manual_kinds: self.manual_kinds.clone(),
            zellij_session_name: self.zellij_session_name.clone().unwrap_or_default(),
            repositories: self.repositories.clone(),
            tab_projects: self.tab_projects.clone(),
            focused: self.focused,
            focused_plugin: self.focused_plugin,
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
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::AGENT_SNAPSHOT)
                    .with_destination_plugin_id(*id)
                    .with_payload(payload.clone())
                    .with_args(BTreeMap::from([("seq".to_owned(), self.seq.to_string())])),
            );
        }
        self.last_published = Some(payload);
    }
}

/// Write a pane's agent record to the session. Off wasm there is no session, and the tests
/// read the daemon's own feed instead.
#[cfg(target_arch = "wasm32")]
fn report_agent(pane_id: u32, agent: Option<AgentInfo>) {
    set_pane_agent(PaneId::Terminal(pane_id), agent);
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
    /// `PermissionRequest` for the same prompt needs it to say.
    #[test]
    fn a_pane_update_does_not_overwrite_what_a_pane_told_us_itself() {
        let request = wire::HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
            ..hook("PermissionRequest")
        };
        let notification = wire::HookEvent {
            notification_type: Some("permission_prompt".to_owned()),
            message: Some("Claude needs your permission to use Bash".to_owned()),
            ..hook("Notification")
        };
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            latched: BTreeMap::from([(7, CLAUDE.to_owned())]),
            ..State::default()
        };
        state.on_hook(7, &request);
        assert_eq!(state.hooks[&7].waiting_on.as_deref(), Some("Bash rm"));
        state.on_hook(7, &notification);
        assert_eq!(
            state.hooks[&7].waiting_on.as_deref(),
            Some("Bash rm"),
            "the backstop fills a gap, it does not replace"
        );

        // The manifest now carries the record `agent-report` wrote for that same
        // notification, which names the prompt the other way round.
        let record = match hocus_proto::agent::hook_outcome(CLAUDE, &notification) {
            hocus_proto::agent::HookOutcome::Reported(agent) => agent,
            other => panic!("{other:?}"),
        };
        state.manifest.panes.insert(
            0,
            vec![PaneInfo {
                id: 7,
                title: "claude".to_owned(),
                agent: Some(record),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        assert_eq!(
            state.hooks[&7].waiting_on.as_deref(),
            Some("Bash rm"),
            "a pane we hear from directly is read from its events"
        );
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
            0,
            vec![PaneInfo {
                id: 7,
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
            zellij_session_name: Some("there".to_owned()),
            panes: Some(vec![pane(2, "claude", "/work")]),
            ..State::default()
        };
        let mut run = agents::AgentRun {
            id: "claude:cc-session-1".to_owned(),
            pane_id: 7,
            zellij_session_name: "here".to_owned(),
            session_id: "cc-session-1".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
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
        run = run.adopted_by(2, "there");
        state.adopt_run(2, run.clone());

        let live = state.runs.get(&2).expect("the run is this pane's now");
        assert_eq!(live.entries.len(), 1, "with the history it arrived with");
        assert_eq!(live.started_at_ms, 10);
        assert_eq!(live.outcome, agents::RunOutcome::Active);
        assert_eq!(live.zellij_session_name, "there");
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
            zellij_session_name: Some("here".to_owned()),
            panes: Some(vec![pane(7, "claude", "/work")]),
            latched: BTreeMap::from([(7, CLAUDE.to_owned())]),
            ..State::default()
        };
        state.manifest.panes.insert(
            0,
            vec![PaneInfo {
                id: 7,
                title: "claude".to_owned(),
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();
        state.on_hook(7, &hook("UserPromptSubmit"));
        let run_id = state.runs[&7].id.clone();
        assert_eq!(run_id, "claude:cc-session-1");

        state.manifest.panes.insert(0, vec![]);
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
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

        state.refresh_remote_repositories_with(|pane_id| {
            assert_eq!(pane_id, 7);
            Some(far_probe("build-box", "/work/hocus", "topic"))
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

        state.refresh_remote_repositories_with(|_| {
            Some(far_probe("build-box", "/work/hocus", "topic"))
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

        state.refresh_remote_repositories_with(|_| {
            Some(far_probe("build-box", "/work/hocus", "topic"))
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
        state.refresh_remote_repositories_with(|pane_id| {
            asked.push(pane_id);
            None
        });
        assert_eq!(asked.len(), REMOTE_PROBES_PER_TICK);

        // The next tick resumes where this one stopped, so every pane is
        // reached rather than the first few being asked forever.
        let mut next: Vec<u32> = Vec::new();
        state.refresh_remote_repositories_with(|pane_id| {
            next.push(pane_id);
            None
        });
        assert!(
            next.iter().any(|pane_id| !asked.contains(pane_id)),
            "the rotation did not move: {asked:?} then {next:?}",
        );
    }

    /// A local pane has no cached probe and the host says so; asking would spend
    /// a round trip to be told what the pane already says.
    #[test]
    fn a_local_pane_is_never_asked_for_a_remote_probe() {
        let mut state = agent_in(7, "/work");

        state.refresh_remote_repositories_with(|pane_id| {
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
        let state = State {
            panes: Some(
                (0..count)
                    .map(|id| pane(id, "zsh", &format!("/work/{id}")))
                    .collect(),
            ),
            cwds: (0..count).map(|id| (id, format!("/work/{id}"))).collect(),
            ..State::default()
        };

        assert_eq!(state.repository_cwds(false).len(), GIT_PROBES_PER_PASS);
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
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
            0,
            vec![PaneInfo {
                id: 7,
                title: "shell".to_owned(),
                exited: true,
                ..PaneInfo::default()
            }],
        );
        state.rebuild_panes();

        // Re-run: the same pane id, running again.
        state.commands.insert(7, "zsh".to_owned());
        state.manifest.panes.insert(
            0,
            vec![PaneInfo {
                id: 7,
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
                0,
                vec![PaneInfo {
                    id,
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
            0,
            vec![PaneInfo {
                id: 7,
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

        state.expire_probe_backoff(GIT_RETRY_SECONDS);
        assert!(state.probe_backoff.is_empty());
        assert_eq!(state.repository_cwds(false), vec![local("/work")]);
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
            last_output_ms_ago: Some(0),
        }
    }

    /// A reading of a pane nothing has written to for a long while — what a resize's reflow
    /// or a viewport that moved on its own looks like.
    fn quiet(text: &str) -> poll::PaneSample {
        poll::PaneSample {
            last_output_ms_ago: Some(600_000),
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
            0,
            vec![PaneInfo {
                id: pane_id,
                title: "claude".to_owned(),
                pane_content_rows: rows,
                pane_content_columns: columns,
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
            tab_projects: BTreeMap::from([(4, "hocus".to_owned())]),
            ..State::default()
        };

        state.publish_forced();

        let published: PollReply =
            serde_json::from_str(state.last_published.as_deref().expect("published"))
                .expect("parses");
        assert_eq!(
            published.tab_projects,
            BTreeMap::from([(4, "hocus".to_owned())])
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

    /// A mode that answers permission prompts by itself has nobody to interrupt. The row
    /// still draws what the agent said — only the toast is gated.
    #[test]
    fn a_wait_under_an_unattended_mode_is_drawn_but_not_announced() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let event = wire::HookEvent {
            permission_mode: Some("bypassPermissions".to_owned()),
            ..tool_hook("PermissionRequest", "Bash", serde_json::json!({}))
        };
        let (toasts, _) = feed(&mut state, 7, &event, None);
        assert!(toasts.is_empty());
        assert_eq!(cell(&state, 7), "waiting · Bash");
    }

    /// …and a mode that turns out not to have answered it says so six seconds later.
    ///
    /// `auto` is a classifier, not a blanket allow: it clears what it judges safe and puts
    /// the rest in front of the user exactly as `default` would. Silencing the request is
    /// still right — most of them really are answered — but the request has by then moved the
    /// feed to `waiting`, so the `Notification` behind it has no busy→waiting edge left to
    /// cross, and a genuinely blocked agent goes unannounced. A notification that fired *is*
    /// the evidence: a prompt the mode had answered would never have gone unanswered long
    /// enough for one.
    #[test]
    fn a_wait_the_mode_did_not_answer_is_announced_by_its_backstop() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let auto = |event: wire::HookEvent| wire::HookEvent {
            permission_mode: Some("auto".to_owned()),
            ..event
        };
        let (silent, _) = feed(
            &mut state,
            7,
            &auto(tool_hook(
                "PermissionRequest",
                "Bash",
                serde_json::json!({ "command": "rm -rf target" }),
            )),
            None,
        );
        assert!(silent.is_empty(), "the mode is expected to answer it");
        assert_eq!(cell(&state, 7), "waiting · Bash rm");

        let backstop = auto(wire::HookEvent {
            notification_type: Some("permission_prompt".to_owned()),
            message: Some("Claude needs your permission to use Bash".to_owned()),
            ..hook("Notification")
        });
        let (toasts, _) = feed(&mut state, 7, &backstop, None);
        assert_eq!(
            toasts,
            vec![(7, "agent waiting · 7 · claude◆\nBash rm".to_owned())],
            "the classifier refused it after all, and the row's own words are the better ones"
        );

        // Answered, and then a second prompt the mode does answer: the edge the first one
        // held is spent, and nothing is owed for the second until its own backstop fires.
        feed(&mut state, 7, &auto(hook("PostToolUse")), None);
        let (again, _) = feed(
            &mut state,
            7,
            &auto(tool_hook(
                "PermissionRequest",
                "Read",
                serde_json::json!({ "file_path": "/work/src/main.rs" }),
            )),
            None,
        );
        assert!(again.is_empty(), "back to the odds on the next prompt");
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
            zellij_session_name: Some("zellij".to_owned()),
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
        let (first, written) = state.dirty_run_mutations("zellij");
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
        let (second, written) = state.dirty_run_mutations("zellij");
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
        let (released, written) = state.dirty_run_mutations("zellij");
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
            zellij_session_name: Some("zellij".to_owned()),
            ..State::default()
        };
        let mut run = agents::AgentRun {
            id: "claude:session-1".to_owned(),
            pane_id: 7,
            zellij_session_name: "zellij".to_owned(),
            session_id: "session-1".to_owned(),
            kind: "claude".to_owned(),
            cwd: None,
            host: None,
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

        let (mutations, written) = state.dirty_run_mutations("zellij");
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

    #[test]
    fn active_runs_follow_a_zellij_session_rename() {
        let mut state = State::default();
        feed(&mut state, 7, &hook("UserPromptSubmit"), None);

        state.set_zellij_session_name("first".to_owned());
        assert_eq!(state.runs[&7].zellij_session_name, "first");
        state.set_zellij_session_name("renamed".to_owned());
        assert_eq!(state.runs[&7].zellij_session_name, "renamed");
        assert_eq!(
            state.dirty_runs[&state.runs[&7].id].zellij_session_name,
            "renamed"
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

    /// Neither half of a malformed message may reach the state. This arrives from
    /// outside the session, on a pipe with nobody to answer.
    #[test]
    fn a_hook_message_that_does_not_parse_changes_nothing() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let message = |args: BTreeMap<String, String>, payload: Option<&str>| PipeMessage {
            source: PipeSource::Cli("pipe-1".to_owned()),
            name: wire::AGENT_HOOK.to_owned(),
            payload: payload.map(str::to_owned),
            args,
            is_private: false,
            pane_id: None,
        };
        let good_args = BTreeMap::from([(wire::HOOK_PANE_ID.to_owned(), "7".to_owned())]);

        // No pane id, a pane id that is not a number, no payload, and a payload that is
        // not one of ours.
        state.pipe(message(
            BTreeMap::new(),
            Some(r#"{"hook_event_name":"Stop"}"#),
        ));
        state.pipe(message(
            BTreeMap::from([(wire::HOOK_PANE_ID.to_owned(), "terminal_7".to_owned())]),
            Some(r#"{"hook_event_name":"Stop"}"#),
        ));
        state.pipe(message(good_args.clone(), None));
        state.pipe(message(good_args.clone(), Some("not json at all")));
        assert!(state.hooks.is_empty());

        // A field we have never heard of is not a reason to drop the event: Claude Code
        // grows them between versions.
        state.pipe(message(
            good_args,
            Some(r#"{"hook_event_name":"Stop","some_future_field":{"a":1}}"#),
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
            zellij_session_name: "here".to_owned(),
            session_id: "cc-session-1".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
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
            zellij_session_name: "here".to_owned(),
            session_id: "cc-session-1".to_owned(),
            kind: CLAUDE.to_owned(),
            cwd: Some("/work".to_owned()),
            host: None,
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
}
