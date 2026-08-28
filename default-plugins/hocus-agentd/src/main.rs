//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! The daemon takes one startup snapshot, follows Zellij's pane, command, and CWD
//! events, and samples agent output on a small heartbeat for activity and Git changes.

mod poll;

use hocus_proto::llm::Endpoint;
use hocus_proto::location::Location;
use hocus_proto::panes::{
    focused_pane, focused_plugin_pane, manifest_pane_ids, pane_geometry, terminal_panes,
};
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

/// How many consecutive unchanged polls a pane must hold before it is summarised — three
/// ticks, about fifteen seconds.
///
/// The busy→idle edge is crossed by any pause, including the one-poll gaps inside a single
/// agent turn while a tool runs, and each crossing would buy a request describing a screen
/// the agent is about to leave. Waiting for the count to reach three spends the request on
/// the screen the agent settles on instead: the counter resets to zero on any new output,
/// so a burst of short pauses costs nothing and only the last one is paid for.
const SUMMARY_SETTLE_POLLS: u32 = 3;

/// How many heartbeats a pane waits after a summary request before it may make another —
/// twelve ticks, a minute.
///
/// The settle window says a screen has stopped moving; it cannot say the agent has stopped
/// working, because a compile or a long tool call holds a screen still just as well as a
/// finished turn does, and each of those can materially change the screen, so the summary
/// baseline does not catch them either. This is the ceiling that does not depend on telling
/// those apart: one request per pane per minute, however the screen churns.
///
/// Equal to `SUMMARY_TICKS` on purpose: the two windows expire within a tick of each
/// other, so a dropped reply costs the pane about a minute and then lets it ask again,
/// rather than either guard outliving the other and holding the pane silent for longer.
const SUMMARY_COOLDOWN_TICKS: u32 = 12;

/// How much of the last summarised pane tail must turn over before another summary is
/// useful. Terminal output is line-oriented, and comparing its lines avoids paying the
/// quadratic cost of edit distance for every settled pane.
const SUMMARY_CHANGE_FACTOR: usize = 5;

/// How many heartbeats a pane is left alone after its pty was resized, so a repaint the
/// resize itself provoked cannot be mistaken for a finished turn.
///
/// Resizing a pane, resizing the terminal, and attaching or detaching a client all resize
/// the pty, and the SIGWINCH that follows makes an agent redraw itself: real output, arriving
/// for a reason that has nothing to do with the agent's work. The word-wise fingerprint
/// already hides a plain rewrap, but a TUI that redraws differently at a new width would
/// still cross an edge, so the edge is consumed outright for this window.
///
/// Two ticks longer than the settle window rather than one: the poll that first sees the
/// repaint spends a tick of the window before the stillness it starts is counted, so a
/// shorter window would lapse on the very poll the edge lands on.
const GEOMETRY_GRACE_TICKS: u32 = SUMMARY_SETTLE_POLLS + 2;

/// Context keys correlating a summary's `WebRequestResult` back to the pane that asked.
/// The same scheme the git probes use, for the same reason: results arrive on one event
/// for the whole plugin.
const SUMMARY_PANE: &str = "summary_pane";
const SUMMARY_ID: &str = "summary_id";

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
    /// The session environment, for resolving the summary endpoint's API key.
    ///
    /// Read once and kept: the environment a plugin sees is the one captured when the
    /// *session* started, so it cannot change under us. The KDL half is deliberately *not*
    /// cached — see `summary_endpoint`.
    env: BTreeMap<String, String>,
    /// What the model last said about each agent's pane.
    summaries: agents::Summaries,
    /// What each Claude Code pane has said about *itself*, over its own hooks.
    ///
    /// The one thing in this daemon that is reported rather than inferred. Where a
    /// pane has an entry here it is the truth about that pane's state, and the
    /// screen-diff heuristic beside it is only still running because the summaries
    /// hang off it — see `agents::HookFeed`.
    hooks: agents::HookFeeds,
    /// The pane tail each successful summary described. Small changes accumulate against
    /// this baseline until at least a fifth of its lines have turned over.
    summary_baselines: BTreeMap<u32, String>,
    /// Panes with a summary request in flight. One at a time per pane, so a flurry of
    /// idle edges cannot fan out into a flurry of requests.
    summary_requests: BTreeMap<u32, InFlight>,
    /// Heartbeats left before each pane may ask again, counted down by every poll.
    /// An absent pane is free to ask — see `SUMMARY_COOLDOWN_TICKS`.
    summary_cooldown: BTreeMap<u32, u32>,
    summary_seq: u64,
    /// The pty size each pane was last seen at, so a change can be noticed. See
    /// `GEOMETRY_GRACE_TICKS`.
    geometry: BTreeMap<u32, (usize, usize)>,
    /// Heartbeats left in which a pane's settling is consumed silently, counted down by
    /// every poll. An absent pane is in no grace window.
    geometry_grace: BTreeMap<u32, u32>,
    /// Where each pane is in the work cycle. The ask hangs off the transition into
    /// `Waiting`, not off the pane being settled — see `Phase`.
    phase: BTreeMap<u32, Phase>,
    /// How long one poll interval is, in milliseconds, with slack: how recently a pane must
    /// have written to its pty for this poll's picture to be explained by output. Taken from
    /// the timer's own reported elapsed time because plugin timers drift.
    poll_window_ms: u64,
}

/// Where a pane is in one cycle of work.
///
/// The point of the phase is that a summary hangs off a *transition*, not off a pane
/// being settled: a pane that has been sitting still for an hour is as settled as one that
/// stopped a moment ago, and only the second has anything new to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The pane is showing new words — the agent is doing something.
    Working,
    /// The pane has stopped. `owed` is whether this stop still has a summary coming: true
    /// from the moment it settles until a request goes out, false once one has (or if the
    /// stop landed inside a grace window, where the stillness is not the agent's).
    ///
    /// `owed` is what separates *deferring* an ask from *suppressing* one. The cooldown and
    /// the in-flight guard defer: the pane keeps asking on later polls until one gets
    /// through, so a settling that arrives a few seconds into a cooldown is not lost. A
    /// grace window suppresses: the ask is dropped and never retried, because there was no
    /// work behind it.
    Waiting { owed: bool },
}

/// A summary request that has gone out and not yet come back.
struct InFlight {
    id: u64,
    /// The pane hash the request was made from, so the answer is stamped with the screen
    /// it actually describes rather than with whatever the pane shows when it lands.
    hash: u64,
    /// The exact tail sent to the model, promoted to the pane's baseline only if the
    /// response succeeds while this screen is still current.
    tail: String,
    /// Heartbeats this has been outstanding. A `WebRequestResult` that never arrives -
    /// a dropped event, a host that gave up - would otherwise leave the pane looking
    /// permanently mid-request and never summarised again for the life of the session.
    ticks: u32,
}

/// Whether enough of the last successful summary's tail has turned over to make another one
/// useful. Words are a multiset: scrolling one line preserves every word still on screen, and
/// repeated output is counted rather than collapsed.
///
/// Words rather than lines because a pane rewrapped to a new width holds exactly the words it
/// held before: a line-wise comparison read every resize as a screen of new work.
fn summary_materially_changed(previous: &str, current: &str) -> bool {
    agents::changed_materially(previous, current, SUMMARY_CHANGE_FACTOR)
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
            let pane_id = pipe_message
                .args
                .get(wire::HOOK_PANE_ID)
                .and_then(|id| id.parse::<u32>().ok());
            let event = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<wire::HookEvent>(json).ok());
            if let (Some(pane_id), Some(event)) = (pane_id, event) {
                self.on_hook(pane_id, &event);
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
                self.resolve_repositories();
            }
            // Answer immediately: a hocus that just opened wants the snapshot now.
            self.publish();
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
        let Some((panes, focused, focused_plugin)) = poll::gather() else {
            return;
        };
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

    /// One Claude Code hook, from the pane it fired in.
    ///
    /// Publishes on every event it acts on, and that is the whole point of the feed:
    /// a state change reaches the strip in the milliseconds it took the pipe to
    /// arrive, rather than on a heartbeat up to five seconds away and then only
    /// after three more polls of stillness have agreed with it.
    ///
    /// The summary machinery is deliberately left alone. It still hangs off the
    /// screen heuristic, because what it produces is a *description* — the toast, the
    /// box, the pane's new name — and a description of a screen is fairly made by
    /// reading the screen. What it may no longer decide is whether the agent is
    /// blocked on the user; see `finish_summary_with`.
    fn on_hook(&mut self, pane_id: u32, event: &wire::HookEvent) {
        if event.hook_event_name == agents::HOOK_SESSION_END {
            // The session is over, so there is nothing left reporting for this pane
            // and the heuristic takes it back. The pane itself may well live on - a
            // shell prompt where an agent was - and that is the latch's business,
            // not ours.
            if self.hooks.remove(&pane_id).is_some() {
                self.publish();
            }
            return;
        }
        let Some(phase) = agents::hook_phase(event) else {
            return;
        };
        // A hook fired in this pane is that pane demonstrably running Claude Code:
        // better evidence than the foreground command, which stops naming the agent
        // the moment it shells out. Latched through the same map the command sniff
        // uses, so it is released by the same confirmed verdict and cannot become a
        // second thing to keep in step.
        self.latched.insert(pane_id, CLAUDE.to_owned());
        self.pending_release.remove(&pane_id);
        let cwd_moved = match &event.cwd {
            Some(cwd) => {
                let known = self.cwds.get(&pane_id).cloned();
                self.record_cwd(pane_id, cwd);
                self.cwds.get(&pane_id) != known.as_ref()
            },
            None => false,
        };
        self.hooks.insert(
            pane_id,
            agents::HookFeed {
                phase,
                detail: agents::hook_detail(event),
                session_id: event.session_id.clone(),
                cwd: event.cwd.clone(),
            },
        );
        self.settle_latches();
        if cwd_moved {
            // The agent's folder, which is not always the pane's: an agent told to
            // work in a worktree moved without the shell under it hearing. This is
            // the only thing that ever says so.
            self.rejoin_cwds();
            self.resolve_repositories();
        }
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

    /// Notice panes whose pty has been resized, and leave them alone for a while.
    ///
    /// A resize is the one thing that makes an agent produce real output for a reason that
    /// is not work: the pane rewraps, the process is sent SIGWINCH, and a full-screen agent
    /// redraws itself. Attaching or detaching a client does it to every pane at once, which
    /// is why re-attaching to a session used to arrive with a fistful of toasts.
    ///
    /// The pane's summary baseline goes with it: it described the same words at a different
    /// width, and comparing across that would read a rewrap as a screenful of new work.
    fn watch_geometry(&mut self) {
        let geometry = pane_geometry(&self.manifest);
        for (pane_id, size) in &geometry {
            if self
                .geometry
                .get(pane_id)
                .is_some_and(|previous| previous != size)
            {
                self.geometry_grace.insert(*pane_id, GEOMETRY_GRACE_TICKS);
                self.summary_baselines.remove(pane_id);
            }
        }
        self.geometry = geometry;
    }

    fn rebuild_panes(&mut self) {
        self.watch_geometry();
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
            self.forget_agent(pane_id);
        }
        // Anything the manifest itself carries is news, and worth taking before
        // the rejoin below writes what we already knew back over it.
        let reported: Vec<(u32, String)> = panes
            .iter()
            .filter_map(|pane| Some((pane.pane_id, pane.cwd.clone()?)))
            .collect();
        self.panes = Some(panes);
        for (pane_id, cwd) in reported {
            self.record_cwd(pane_id, &cwd);
        }
        self.rejoin_cwds();
        self.settle_latches();
        self.resolve_repositories();
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
        self.observe_activity_with(poll::read_pane, Self::start_summary);
    }

    /// Sample every agent pane, and act on the ones that just stopped working.
    ///
    /// `read` and `ask` are injected for the same reason: both reach the host, and this
    /// has to stay testable on the host triple. `ask` answers whether a request actually
    /// went out, which is what tells a pane its debt is paid.
    fn observe_activity_with(
        &mut self,
        mut read: impl FnMut(u32) -> poll::PaneSample,
        mut ask: impl FnMut(&mut Self, u32, &str) -> bool,
    ) {
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
        self.phase.retain(|id, _| running.contains(id));
        self.summaries.retain(|id, _| known.contains(id));
        self.summary_baselines.retain(|id, _| known.contains(id));
        // A request that ages out never had an answer, so the settling that bought it is
        // owed again: a reply the host dropped must not cost the pane its summary.
        let mut dropped: Vec<u32> = Vec::new();
        self.summary_requests.retain(|id, request| {
            request.ticks += 1;
            let answered_in_time = request.ticks <= SUMMARY_TICKS;
            if !answered_in_time && running.contains(id) {
                dropped.push(*id);
            }
            answered_in_time && running.contains(id)
        });
        for pane_id in dropped {
            if let Some(Phase::Waiting { owed }) = self.phase.get_mut(&pane_id) {
                *owed = true;
            }
        }
        // The same shape one line up, and pruned by the same rule: a pane that stops being
        // a running agent starts clean if it comes back.
        self.summary_cooldown.retain(|id, ticks| {
            *ticks = ticks.saturating_sub(1);
            running.contains(id) && *ticks > 0
        });
        // Aged on the same beat, and kept for panes that are merely held rather than
        // running: a resize does not stop mattering because the agent paused.
        self.geometry_grace.retain(|id, ticks| {
            *ticks = ticks.saturating_sub(1);
            known.contains(id) && *ticks > 0
        });
        for pane_id in running {
            let reading = read(pane_id);
            let sample = self.qualify(pane_id, &reading);
            // Nothing is ever asked about a reading that says nothing: a scrolled pane, or
            // one we could not read at all, has no screen worth describing. The debt stays
            // owed, so the next usable reading pays it.
            let usable = matches!(sample, agents::Sample::Text(_));
            let observed = agents::observe(&mut self.activity, pane_id, sample);
            let phase = self.phase.get(&pane_id).copied();
            match (phase, observed) {
                // The very first reading of a pane is `Busy` whatever it shows, because one
                // reading cannot be compared with anything - so it says nothing about work:
                // a session left alone overnight looks exactly like an agent mid-turn. The
                // pane starts out settled and owing nothing, and earns `Working` on the next
                // poll if it really is doing something.
                (None, agents::Observed::Busy) => {
                    self.phase.insert(pane_id, Phase::Waiting { owed: false });
                },
                // New words, and the reading was worth believing: the agent is working, and
                // whatever it settles on next is worth asking about.
                //
                // Note that a pane going busy keeps the summary it already has rather than
                // dropping it: the marker is gated on the pane being idle
                // (`agents::waiting_marker`), so a working agent already shows nothing - and
                // keeping the answer, stamped with the screen it was made from, is what lets
                // an agent that comes back to that same screen be free rather than another
                // request.
                (_, agents::Observed::Busy) => {
                    self.phase.insert(pane_id, Phase::Working);
                },
                // The edge, and the only place a debt is taken on. Equality, not `>=`: a
                // pane that has been settled for an hour crosses no edge, and the retry
                // below is what carries a settling the cooldown swallowed. Only a pane that
                // was working owes anything, and only outside a grace window - a screen that
                // moved because the pty was resized stopped nothing.
                (phase, agents::Observed::Idle { polls }) if polls == SUMMARY_SETTLE_POLLS => {
                    let owed = phase == Some(Phase::Working)
                        && !self.geometry_grace.contains_key(&pane_id);
                    let paid = owed && usable && ask(self, pane_id, &reading.text);
                    self.phase.insert(
                        pane_id,
                        Phase::Waiting {
                            owed: owed && !paid,
                        },
                    );
                },
                // Still owed: the cooldown, an in-flight request or a baseline that has not
                // moved refused this settling, and none of those mean it was not real. Keep
                // offering it until one of them lets it through.
                (Some(Phase::Waiting { owed: true }), agents::Observed::Idle { .. })
                    if usable && ask(self, pane_id, &reading.text) =>
                {
                    self.phase.insert(pane_id, Phase::Waiting { owed: false });
                },
                // Settling, or settled and paid for.
                _ => {},
            }
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

    /// Where to send a summary request, resolved fresh each time.
    ///
    /// Deliberately not cached. The endpoint is configured in *hocus's* KDL block, and the
    /// host's `PluginConfigurationChanged` only notifies a plugin about its own block — so
    /// a cached copy would go stale the moment the user edited the setting and never
    /// recover. One host read, paid only on the edge that is about to make an HTTP
    /// request, is not worth a staleness bug.
    fn summary_endpoint(&self) -> Option<Endpoint> {
        Endpoint::resolve(&get_plugin_alias_configuration(HOCUS_ALIAS), &self.env)
    }

    /// Ask the model what an agent that just went idle is doing.
    ///
    /// The cheap local guards run *before* the endpoint is resolved, so an edge that was
    /// never going to produce a request does not pay for a host round-trip either.
    ///
    /// Answers whether a request went out, which is what tells the pane whether its
    /// settling has been paid for or is still owed — see `Phase::Waiting`.
    fn start_summary(&mut self, pane_id: u32, text: &str) -> bool {
        if !self.summary_due(pane_id, text) {
            return false;
        }
        let Some(endpoint) = self.summary_endpoint() else {
            return false;
        };
        // Wrapped rather than passed by name: `web_request` is generic over its url type,
        // and inference pins that to one lifetime, which will not satisfy the for-any-
        // lifetime bound `send_summary` needs.
        self.send_summary(
            pane_id,
            text,
            &endpoint,
            |url, verb, headers, body, context| {
                web_request(url, verb, headers, body, context, None)
            },
        );
        true
    }

    /// Whether this settling is worth an HTTP request.
    ///
    /// No when the pane asked less than a minute ago, when one is already in flight for it,
    /// when it holds too little to say anything about, or when less than a fifth of the
    /// last successfully summarised tail has changed.
    fn summary_due(&self, pane_id: u32, text: &str) -> bool {
        // First, because it is the cheapest and the one that fires most: a busy agent
        // settles for fifteen seconds several times a turn, and only the first is paid for.
        if self.summary_cooldown.contains_key(&pane_id) {
            return false;
        }
        if self.summary_requests.contains_key(&pane_id) || !agents::worth_summarizing(text) {
            return false;
        }
        // The pane's foreground command has dropped back to a shell, which proposes that the
        // agent behind it has finished; `repair_commands` confirms or withdraws that within a
        // tick or two. Asking in the meantime describes a shell prompt as an agent session —
        // the latch still calls this pane an agent, but its screen is no longer one. Nothing
        // is lost by waiting: the settling stays owed, and is either paid once the proposal
        // is withdrawn or forgotten with the pane when it is confirmed.
        if self.pending_release.contains(&pane_id) {
            return false;
        }
        if agents::activity_hash(&self.activity, pane_id).is_none() {
            // The poll that would have recorded one has not run, so there is nothing to
            // stamp an answer with.
            return false;
        }
        self.summary_baselines.get(&pane_id).is_none_or(|previous| {
            summary_materially_changed(
                previous,
                &agents::tail(text, agents::SUMMARY_LINES, agents::SUMMARY_CHARS),
            )
        })
    }

    /// Build and send one summary request. `send` is injected because it reaches the host.
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

        // Unwrap-free: `summary_due` is what gates every caller, and it has already
        // established that this pane has a hash to stamp an answer with.
        let hash = agents::activity_hash(&self.activity, pane_id).unwrap_or_default();
        self.summary_seq += 1;
        let id = self.summary_seq;
        self.summary_requests.insert(
            pane_id,
            InFlight {
                id,
                hash,
                tail,
                ticks: 0,
            },
        );
        // Stamped when the request goes out rather than when its answer lands, so a slow or
        // dropped reply cannot buy the pane a longer window than a fast one.
        self.summary_cooldown
            .insert(pane_id, SUMMARY_COOLDOWN_TICKS);
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
        let (hash, tail) = self
            .summary_requests
            .remove(&pane_id)
            .map(|request| (request.hash, request.tail))
            .unwrap_or_default();
        // The pane has moved on since we asked, so this answer describes a screen that is
        // gone. Compared against the hash the *request* was made from rather than
        // whatever the pane shows now: stamping a stale answer with the current screen
        // would make `summary_due` skip the request that screen is actually owed.
        if agents::activity_hash(&self.activity, pane_id) != Some(hash) {
            return;
        }
        // The agent went away between the request leaving and its answer landing: the pane
        // closed, or it is back at its prompt. The answer describes a session that is over,
        // and storing it would put a summary on a pane that has nothing to summarise —
        // toasting it is worse still.
        if self.agent_kind(pane_id).is_none() || self.pending_release.contains(&pane_id) {
            return;
        }
        // A failed summary is simply no summary — the same stance `ProbeOutcome::Failed`
        // takes, and for the same reason: a guess here would outlive the reason for it.
        // The next idle edge asks again.
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
        // Not while the user is looking at the pane: a toast over the very screen it
        // describes tells them what they can already see, and the summary still reaches
        // them through the snapshot below and the pane's new name.
        if self.focused != Some(pane_id) {
            toast(
                pane_id,
                &agents::summary_toast(pane_id, self.agent_kind(pane_id), &summary),
            );
        }
        self.summary_baselines.insert(pane_id, tail);
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
        self.activity.remove(&pane_id);
        self.phase.remove(&pane_id);
        self.summaries.remove(&pane_id);
        self.summary_baselines.remove(&pane_id);
        self.summary_requests.remove(&pane_id);
        self.summary_cooldown.remove(&pane_id);
        self.geometry_grace.remove(&pane_id);
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

    /// Whether renaming this pane would cost us the pane.
    ///
    /// `classify` reads the title before anything else, so a pane recognised *only* by
    /// what its title says — a `zsh` running an agent we have never seen the command for —
    /// would stop being an agent the moment we wrote a task name over it, and stop being
    /// polled with it. Rename only where the kind is anchored somewhere the title cannot
    /// reach: the user's own registration, or the command latch (`settle_latches`), which
    /// is the memory of a command we demonstrably saw run.
    fn title_is_ours_to_take(&self, pane_id: u32) -> bool {
        if self.manual_kinds.contains_key(&pane_id) {
            return true;
        }
        self.panes
            .iter()
            .flatten()
            .find(|pane| pane.pane_id == pane_id)
            .is_some_and(|pane| {
                pane.detected.is_some()
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
    /// whatever title a pane has, so this never competes with the three writers
    /// of a pane's *name* — the user, the pane's own OSC title, and
    /// `finish_summary_with` below. That is also why it needs no
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
        self.refresh_title_prefixes();
        self.seq += 1;
        let reply = PollReply {
            panes: self.panes.clone(),
            activity: self.activity.clone(),
            manual_kinds: self.manual_kinds.clone(),
            repositories: self.repositories.clone(),
            focused: self.focused,
            focused_plugin: self.focused_plugin,
            summaries: self.summaries.clone(),
            hooks: self.hooks.clone(),
        };
        let Ok(payload) = serde_json::to_string(&reply) else {
            return;
        };
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
    }
}

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
            status: agents::AgentStatus::Running,
            tab: None,
            detected: None,
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

    /// The summariser, for tests about everything else. `start_summary` reaches the host
    /// twice - the alias configuration and the request itself - so no test that is not
    /// about summaries may run it.
    fn no_ask(_: &mut State, _: u32, _: &str) -> bool {
        false
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

        state.observe_activity_with(|_| sample("working"), no_ask);
        state.observe_activity_with(|_| sample("working"), no_ask);
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));
        assert_eq!(state.activity.len(), 1);

        state.panes.as_mut().unwrap()[0].status = agents::AgentStatus::Held;
        state.observe_activity_with(|_| sample("ignored"), no_ask);
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
        state.observe_activity_with(|_| sample("same"), no_ask);
        state.observe_activity_with(|_| sample("same"), no_ask);
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));

        state.on_command(7, vec!["git".to_owned(), "status".to_owned()], true);
        state.observe_activity_with(|_| sample("same"), no_ask);

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
        state
            .summary_baselines
            .insert(7, "the old process".to_owned());
        state.summaries.insert(
            7,
            agents::Summary {
                text: "did a thing".to_owned(),
                title: "a thing".to_owned(),
                awaiting: true,
                hash: 1,
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
        assert!(
            !state.summary_baselines.contains_key(&7),
            "stale baseline dropped"
        );
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

    // -- idle summaries -----------------------------------------------------

    #[test]
    fn summary_change_requires_a_fifth_of_the_words_to_turn_over() {
        // Three words a line, none of them shared, so the arithmetic below is the ratio
        // the constant names and nothing else.
        let line = |n: usize| format!("output{n} of{n} ten{n}");
        let baseline = (1..=10).map(line).collect::<Vec<_>>().join("\n");
        let one_changed = baseline.replace(&line(10), "one new line");
        let two_changed = one_changed.replace(&line(9), "another different line");
        let scrolled = (2..=11).map(line).collect::<Vec<_>>().join("\n");

        assert!(!summary_materially_changed(&baseline, &one_changed));
        assert!(summary_materially_changed(&baseline, &two_changed));
        assert!(!summary_materially_changed(&baseline, &scrolled));
        assert!(!summary_materially_changed(
            &baseline,
            &baseline.replace(&line(1), &format!("  {}  ", line(1)))
        ));
        // The whole reason words replaced lines: the same output at a new width.
        assert!(!summary_materially_changed(
            &baseline,
            &baseline.replace('\n', " ")
        ));
    }

    fn endpoint() -> Endpoint {
        Endpoint {
            url: "http://localhost:11434/v1/chat/completions".to_owned(),
            api_key: "sk-test".to_owned(),
            model: "test-model".to_owned(),
        }
    }

    /// One poll of every pane, showing `text`, with the summariser wired to `sent`.
    ///
    /// `observe_activity_with` is driven rather than `start_summary` called directly, so
    /// what is under test is the whole path from "the pane held still" to "a request left".
    fn poll_showing(state: &mut State, text: &str, sent: &mut Vec<(u32, Vec<u8>)>) {
        poll_reading(state, sample(text), sent)
    }

    /// One poll of every pane from an arbitrary reading, for the tests about readings that
    /// are worth nothing — a scrolled pane, or a picture that moved with the pty silent.
    fn poll_reading(state: &mut State, reading: poll::PaneSample, sent: &mut Vec<(u32, Vec<u8>)>) {
        let mut reading = Some(reading);
        state.observe_activity_with(
            |_| reading.take().expect("one pane per poll in these tests"),
            |state, pane_id, text| {
                if state.summary_due(pane_id, text) {
                    state.send_summary(pane_id, text, &endpoint(), |_, _, _, body, context| {
                        sent.push((context[SUMMARY_PANE].parse().unwrap(), body));
                    });
                    return true;
                }
                false
            },
        );
    }

    /// Poll `text` for a whole cooldown, so a pane that is holding still goes on holding
    /// still while its window runs out. Enough polls to clear the window whichever tick of
    /// it the caller is on, and showing one screen throughout, so the baseline sees no
    /// change either — what expires is the cooldown and nothing else.
    fn poll_out_the_cooldown(state: &mut State, text: &str, sent: &mut Vec<(u32, Vec<u8>)>) {
        for _ in 0..=SUMMARY_COOLDOWN_TICKS {
            poll_showing(state, text, sent);
        }
    }

    /// One agent, polled until it has settled, and the requests that went out.
    fn idle_agent(text: &str) -> (State, Vec<(u32, Vec<u8>)>) {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        // A pane the daemon has only ever seen still is not one that just stopped working,
        // so the agent is shown working first: one screen, then the screen it settles on.
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, text, &mut sent);
        }
        (state, sent)
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
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        poll_showing(&mut state, BUSY_PANE, &mut sent);
        assert_eq!(cell(&state, 7), "busy", "the screen is moving");

        state.on_hook(7, &hook("Stop"));

        assert_eq!(cell(&state, 7), "idle");
        assert!(
            sent.is_empty(),
            "and nothing has been asked of a model to get there"
        );
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

        state.on_hook(7, &hook(agents::HOOK_SESSION_END));
        assert!(state.hooks.is_empty());
        assert_eq!(
            cell(&state, 7),
            "running",
            "no feed and nothing observed yet"
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
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1, "still summarised");

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
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }

        let finished = finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        assert_eq!(finished.renames, vec![(7, TITLE.to_owned())]);
    }

    /// Going on holding still is not a new settling. Without this every idle agent would
    /// be re-summarised every five seconds for as long as it sat there.
    ///
    /// Polled well past the cooldown on purpose: the cooldown is what covers the first
    /// minute, and after it lapses the baseline guard is what has to carry this, because the
    /// settle arm goes on matching for as long as the pane sits there.
    #[test]
    fn sitting_idle_does_not_ask_again() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("Done.", false), &context(7, 1));

        for _ in 0..3 {
            poll_out_the_cooldown(&mut state, BUSY_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1);
    }

    /// An agent working through a turn pauses - a tool runs, a file is read - and each
    /// pause crosses the busy→idle edge. Acting on the edge would buy a request per pause,
    /// each describing a screen the agent is about to leave.
    #[test]
    fn a_burst_of_short_pauses_asks_nothing() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        // Well past the polls a settled pane would need, but never two polls of stillness
        // in a row, so the count never reaches the window.
        for round in 0..6 {
            let text = format!("{BUSY_PANE} step {round}");
            poll_showing(&mut state, &text, &mut sent);
            poll_showing(&mut state, &text, &mut sent);
        }
        assert!(
            sent.is_empty(),
            "no pause lasted long enough to mean the agent had stopped"
        );
    }

    /// And when the burst ends, the one request that is paid for describes the screen the
    /// agent settled on - not any of the ones it passed through.
    #[test]
    fn only_the_screen_a_burst_settles_on_is_asked_about() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        for round in 0..3 {
            let text = format!("{BUSY_PANE} step {round}");
            poll_showing(&mut state, &text, &mut sent);
            poll_showing(&mut state, &text, &mut sent);
        }
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, OTHER_PANE, &mut sent);
        }

        assert_eq!(sent.len(), 1, "one settling, one request");
        let body: serde_json::Value = serde_json::from_slice(&sent[0].1).unwrap();
        let prompt = body["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains(OTHER_PANE), "the screen it settled on");
        assert!(!prompt.contains("step 2"), "not the one before it");
    }

    /// A pause long enough to settle is not the same as a turn ending. A compile or a slow
    /// tool holds a screen still for fifteen seconds just as well as a finished agent does,
    /// and each such pause materially changes the screen, so the baseline guard does
    /// not catch it. The cooldown is what does.
    #[test]
    fn a_second_settling_inside_the_cooldown_asks_nothing() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        // Three long pauses, each on a screen of its own, well inside one minute.
        for round in 0..3 {
            let text = format!("{BUSY_PANE} step {round}");
            for _ in 0..=SUMMARY_SETTLE_POLLS {
                poll_showing(&mut state, &text, &mut sent);
            }
        }
        assert_eq!(sent.len(), 1, "one minute, one request");
    }

    /// And the request the cooldown swallowed is not lost. The pane is still sitting on
    /// that screen when the window lapses, so it is asked about then — which is what the
    /// settle arm matching on `>=` rather than `==` buys.
    #[test]
    fn a_pane_still_settled_when_the_cooldown_ends_is_asked_about() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        assert_eq!(sent.len(), 1, "the first screen");

        // A new screen, settled on while the window is still running.
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, OTHER_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1, "swallowed for now");

        poll_out_the_cooldown(&mut state, OTHER_PANE, &mut sent);
        assert_eq!(sent.len(), 2, "asked once the window lapsed");
        let body: serde_json::Value = serde_json::from_slice(&sent[1].1).unwrap();
        let prompt = body["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains(OTHER_PANE), "about the screen it is on");
    }

    /// A reply that never lands must not cost the pane its summaries. Both windows are a
    /// minute, so the in-flight guard and the cooldown lapse together and the pane retries.
    #[test]
    fn a_dropped_reply_is_retried_after_a_minute() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        assert_eq!(sent.len(), 1);

        // No `finish` - the answer is simply never delivered.
        poll_out_the_cooldown(&mut state, BUSY_PANE, &mut sent);

        assert_eq!(sent.len(), 2, "asked again rather than silently giving up");
        assert!(
            state.summaries.is_empty(),
            "and still with nothing to show for the first ask"
        );
    }

    /// The ceiling, stated plainly: whatever the screen does, one pane cannot buy more than
    /// a request a minute.
    #[test]
    fn a_settled_pane_asks_at_most_once_a_minute() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        // Twenty screens, each held long enough to settle: eighty polls, near seven minutes.
        for round in 0..20 {
            let text = format!("{BUSY_PANE} step {round}");
            for _ in 0..=SUMMARY_SETTLE_POLLS {
                poll_showing(&mut state, &text, &mut sent);
            }
        }
        let minutes = 20 * (SUMMARY_SETTLE_POLLS as usize + 1) / SUMMARY_COOLDOWN_TICKS as usize;
        assert!(
            sent.len() <= minutes + 1,
            "{} requests over {minutes} minutes of polls",
            sent.len()
        );
    }

    /// An agent that starts working again is not waiting on anyone, whatever it was
    /// waiting on before — but the answer itself is kept, stamped with the screen it was
    /// made from, so a return to that same screen costs nothing. The marker is what goes,
    /// and it goes because it is drawn only for an idle pane.
    #[test]
    fn going_busy_again_clears_the_flag_without_throwing_the_answer_away() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(
            &mut state,
            200,
            &reply("Waiting on you.", true),
            &context(7, 1),
        );
        let waiting = |state: &State| {
            agents::strip_rows(
                state.panes.as_deref().unwrap_or_default(),
                &state.manual_kinds,
                &state.activity,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &state.summaries,
                &state.hooks,
            )
            .iter()
            .any(|row| matches!(row, agents::StripRow::Agent { text, .. } if text.starts_with('!')))
        };
        assert!(waiting(&state), "idle and blocked: the marker is up");

        state.observe_activity_with(|_| sample(OTHER_PANE), no_ask);

        assert!(
            !waiting(&state),
            "working again, so nothing is waiting on the user"
        );
        assert!(
            state.summaries.contains_key(&7),
            "but the answer is still held"
        );
    }

    /// Small pane changes do not make the last summary obsolete. They accumulate against
    /// the successful summary's tail until a fifth of its lines have turned over.
    #[test]
    fn a_barely_changed_pane_is_not_summarized_again() {
        let line = |n: usize| format!("agent{n} output{n} line{n}");
        let baseline = (1..=10).map(line).collect::<Vec<_>>().join("\n");
        let (mut state, _) = idle_agent(&baseline);
        finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        // Dropped rather than polled out, so what the assertions below answer for is the
        // baseline guard alone and not the cooldown standing in front of it.
        state.summary_cooldown.clear();

        let one_changed = baseline.replace(&line(10), "one new status");
        state.observe_activity_with(|_| sample(&one_changed), no_ask);
        state.observe_activity_with(|_| sample(&one_changed), no_ask);
        assert!(
            !state.summary_due(7, &one_changed),
            "one changed line is still covered by the stored summary",
        );

        let two_changed = one_changed.replace(&line(9), "another different note");
        state.observe_activity_with(|_| sample(&two_changed), no_ask);
        state.observe_activity_with(|_| sample(&two_changed), no_ask);
        assert!(
            state.summary_due(7, &two_changed),
            "changes accumulate against the successful baseline"
        );
    }

    #[test]
    fn a_pane_with_nothing_on_it_is_not_worth_asking_about() {
        let (_, sent) = idle_agent("$");
        assert!(sent.is_empty());
    }

    /// One request per pane at a time: a second edge while one is in flight would double
    /// the bill for one answer.
    #[test]
    fn a_second_edge_does_not_race_the_request_already_in_flight() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        assert_eq!(state.summary_requests.get(&7).map(|r| r.id), Some(1));
        // Dropped so the in-flight guard is the only thing left to refuse.
        state.summary_cooldown.clear();
        assert!(
            !state.summary_due(7, OTHER_PANE),
            "even for a different screen"
        );
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

    /// Scrolling a pane back shows old output. It is not the agent doing anything, and the
    /// pane is not working again because the user looked at what it already wrote.
    #[test]
    fn a_scrolled_pane_is_not_mistaken_for_a_working_one() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        assert_eq!(sent.len(), 1, "the settling itself");
        let hash = agents::activity_hash(&state.activity, 7);
        state.summary_cooldown.clear();

        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_reading(&mut state, scrolled(OTHER_PANE), &mut sent);
        }

        assert_eq!(sent.len(), 1, "scrolling bought no second request");
        assert_eq!(
            agents::activity_hash(&state.activity, 7),
            hash,
            "and the reply already in flight can still land"
        );
    }

    /// Coming back down to the tail is not a fresh settling either — the screen the pane
    /// ends on is the one it was already summarised for.
    #[test]
    fn scrolling_back_down_does_not_buy_a_second_summary() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        state.summary_cooldown.clear();

        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_reading(&mut state, scrolled(OTHER_PANE), &mut sent);
        }
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }

        assert_eq!(sent.len(), 1);
    }

    /// A resize rewraps every line without the agent doing a thing. The words are the same
    /// words, so the pane never left the screen it settled on.
    #[test]
    fn a_repaint_at_a_new_width_is_not_work() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        state.summary_cooldown.clear();
        let rewrapped = BUSY_PANE.replace(' ', "\n");

        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, &rewrapped, &mut sent);
        }

        assert_eq!(sent.len(), 1, "the same words at a new width are not news");
    }

    /// A pane whose picture moved while nothing was written to its pty moved for some
    /// reason other than the agent — a reflow, most often. It is not work, and it does not
    /// restart the count.
    #[test]
    fn a_screen_that_moved_with_the_pty_silent_is_not_work() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        state.summary_cooldown.clear();

        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_reading(&mut state, quiet(OTHER_PANE), &mut sent);
        }

        assert_eq!(sent.len(), 1);
    }

    /// A pane whose pty was resized is left alone for a while: the SIGWINCH that follows
    /// makes a full-screen agent redraw itself, which is real output for a reason that has
    /// nothing to do with the user's work.
    #[test]
    fn a_pane_that_changed_size_is_left_alone_until_it_settles() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            manifest: sized_manifest(7, 40, 80),
            ..State::default()
        };
        state.watch_geometry();
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);

        // The resize, and the redraw it provokes.
        state.manifest = sized_manifest(7, 40, 100);
        state.rebuild_panes();
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }

        assert!(sent.is_empty(), "the redraw was not a finished turn");

        // And the suppression does not outlive the grace window: it is a window, not a
        // ban on ever summarising this pane again.
        for _ in 0..GEOMETRY_GRACE_TICKS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }
        assert!(sent.is_empty(), "still nothing new to say afterwards");
    }

    /// Attaching to a session resizes every pane in it at once. Whatever the agents were
    /// doing, none of them just finished doing it.
    #[test]
    fn attaching_to_a_settled_session_says_nothing() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            manifest: sized_manifest(7, 40, 80),
            ..State::default()
        };
        state.watch_geometry();
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1, "the agent really did finish, once");
        finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        state.summary_cooldown.clear();

        // The attach: a narrower client, so every pane is resized and repaints.
        state.manifest = sized_manifest(7, 30, 60);
        state.rebuild_panes();
        let rewrapped = BUSY_PANE.replace(' ', "\n");
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, &rewrapped, &mut sent);
        }

        assert_eq!(sent.len(), 1, "attaching told the user nothing new");
    }

    /// The other half of the grace window: work that lands after it is still work.
    #[test]
    fn work_that_lands_after_a_resize_is_still_asked_about() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            manifest: sized_manifest(7, 40, 80),
            ..State::default()
        };
        state.watch_geometry();
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);

        state.manifest = sized_manifest(7, 40, 100);
        state.rebuild_panes();
        for _ in 0..GEOMETRY_GRACE_TICKS {
            poll_showing(&mut state, EARLIER_PANE, &mut sent);
        }
        assert!(sent.is_empty());

        // The window has lapsed, and now the agent actually does something and stops.
        poll_showing(&mut state, BUSY_PANE, &mut sent);
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, OTHER_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1);
    }

    /// A session left alone overnight is full of settled agents. Noticing them is not the
    /// same as watching one stop, and starting the daemon must not toast every one of them.
    #[test]
    fn a_pane_that_was_already_still_when_we_first_saw_it_is_not_asked_about() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent = Vec::new();
        for _ in 0..SUMMARY_COOLDOWN_TICKS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }

        assert!(sent.is_empty(), "nothing here ever stopped working");

        // But it is not written off: the next thing it does is asked about.
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, OTHER_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1);
    }

    /// A settling that lands while the pane is unreadable — scrolled back, or a host call
    /// that failed — is not lost: it is asked about as soon as there is a screen worth
    /// describing again.
    #[test]
    fn a_settling_nobody_could_read_is_asked_about_once_it_can_be() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        poll_showing(&mut state, BUSY_PANE, &mut sent);
        // The user scrolls back just as the agent stops.
        for _ in 0..SUMMARY_SETTLE_POLLS {
            poll_reading(&mut state, scrolled(OTHER_PANE), &mut sent);
        }
        assert!(sent.is_empty(), "nothing worth describing was on screen");

        poll_showing(&mut state, BUSY_PANE, &mut sent);
        assert_eq!(sent.len(), 1, "and the settling was not thrown away");
    }

    /// The pane closed under the poll, so the host answered an error instead of a screen.
    /// That error is not what the agent wrote, and asking the model to summarise it would
    /// describe the failure as if it were the session.
    #[test]
    fn a_pane_we_could_not_read_is_never_sent_to_the_model() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        // The flag alone has to be enough. Nothing else about this reading looks wrong —
        // it is not scrolled, and there is no earlier screen for it to have moved away
        // from — so it is the failure itself that makes it worth nothing.
        assert!(matches!(
            state.qualify(7, &unreadable()),
            agents::Sample::Unusable
        ));

        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        poll_showing(&mut state, BUSY_PANE, &mut sent);
        for _ in 0..SUMMARY_SETTLE_POLLS {
            poll_reading(&mut state, unreadable(), &mut sent);
        }

        assert!(sent.is_empty(), "an error message is not a screen");

        // And if the pane comes back — a read that failed for some passing reason — the
        // settling it could not pay for is still owed.
        poll_showing(&mut state, BUSY_PANE, &mut sent);
        assert_eq!(sent.len(), 1);
    }

    /// The agent exits and leaves its pane sitting at a shell prompt. The latch and the
    /// title still call the pane an agent, and the prompt drawing itself looks exactly like
    /// a screenful of work settling — so this used to ask the model to describe a session
    /// that had already ended.
    #[test]
    fn an_agent_that_has_finished_is_not_summarised() {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        state.on_command(7, vec!["claude".to_owned()], true);
        let mut sent = Vec::new();
        poll_showing(&mut state, EARLIER_PANE, &mut sent);
        // The agent exits: nothing in the foreground, and no agent in the command.
        state.on_command(7, vec!["zsh".to_owned()], false);
        assert!(state.pending_release.contains(&7), "proposed, not settled");
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, BUSY_PANE, &mut sent);
        }

        assert!(sent.is_empty(), "there is no agent behind this screen");

        // The proposal was wrong — the agent was only between tool runs — and the settling
        // it swallowed was deferred rather than dropped.
        state.repair_commands_with(|_| Some("claude".to_owned()));
        poll_showing(&mut state, BUSY_PANE, &mut sent);
        assert_eq!(sent.len(), 1);
    }

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
        assert!(state.summary_baselines.is_empty());
        assert!(state.activity.is_empty());
        assert!(state.phase.is_empty());
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

    /// A settling the cooldown swallowed is deferred, not dropped — the opposite of one a
    /// grace window suppresses.
    #[test]
    fn an_edge_the_cooldown_swallowed_is_asked_about_when_it_expires() {
        let (mut state, mut sent) = idle_agent(BUSY_PANE);
        assert_eq!(sent.len(), 1);

        // A second stretch of work, finished well inside the cooldown.
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, OTHER_PANE, &mut sent);
        }
        assert_eq!(sent.len(), 1, "refused for now");

        poll_out_the_cooldown(&mut state, OTHER_PANE, &mut sent);
        assert_eq!(sent.len(), 2, "and asked once it lapsed");
    }

    /// The state the toast is worth interrupting for: the agent is not finished, it is
    /// stuck on you.
    #[test]
    fn a_toast_for_an_agent_waiting_on_the_user_says_so() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(
            &mut state,
            200,
            &reply("Shall I push?", true),
            &context(7, 1),
        );

        assert_eq!(
            finished.toasts,
            vec![(7, "agent waiting · 7 · claude\nShall I push?".to_owned())],
        );
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

    /// The pane went back to work while we were asking. Storing the answer would raise a
    /// marker on an agent that is busy.
    #[test]
    fn a_reply_for_a_pane_that_went_busy_again_is_dropped() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        state.observe_activity_with(|_| sample(OTHER_PANE), no_ask);

        let finished = finish(
            &mut state,
            200,
            &reply("Waiting on you.", true),
            &context(7, 1),
        );

        assert!(state.summaries.is_empty());
        assert!(state.summary_baselines.is_empty());
        assert!(
            finished.toasts.is_empty(),
            "and a toast describing a screen that is gone is worse than none",
        );
    }

    /// A failed summary is simply no summary - the same stance a failed Git probe takes.
    /// The pane asks again once its cooldown is up, rather than being written off.
    #[test]
    fn a_failed_request_stores_nothing_and_frees_the_pane_to_ask_again() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(&mut state, 500, b"upstream is down", &context(7, 1));

        assert!(state.summaries.is_empty());
        assert!(state.summary_baselines.is_empty());
        assert!(state.summary_requests.is_empty());
        assert!(finished.toasts.is_empty(), "and says nothing to the user");
        // Dropped rather than polled out: a failure is not meant to shorten the window, so
        // what is asserted below is that nothing *else* is holding the pane back.
        state.summary_cooldown.clear();
        assert!(
            state.summary_due(7, BUSY_PANE),
            "so the next edge tries again"
        );
    }

    /// An answer the parser cannot use is the same as no answer, and must not leave the
    /// pane looking like it still has a request out.
    #[test]
    fn an_unusable_answer_is_no_answer() {
        let (mut state, _) = idle_agent(BUSY_PANE);

        let finished = finish(&mut state, 200, b"{\"choices\":[]}", &context(7, 1));

        assert!(state.summaries.is_empty());
        assert!(state.summary_baselines.is_empty());
        assert!(state.summary_requests.is_empty());
        assert!(finished.toasts.is_empty());
    }

    /// A reply is stamped with the screen the *request* was made from, not with whatever
    /// the pane happens to show when it lands.
    ///
    /// The bug this guards: a pane that moves on while the request is out is still idle
    /// when the answer arrives, so a naive "is it busy?" check lets the stale answer
    /// through. The request hash is the exact identity check that rejects it; the summary
    /// baseline must remain untouched so the new screen can still earn its own request.
    #[test]
    fn an_answer_about_a_screen_the_pane_has_left_is_dropped() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        // The pane moves on and settles somewhere else, all while the request is out.
        state.observe_activity_with(|_| sample(OTHER_PANE), no_ask);
        state.observe_activity_with(|_| sample(OTHER_PANE), no_ask);
        assert!(
            state.activity[&7].1 > 0,
            "idle again, so idleness alone cannot tell this answer is stale",
        );

        let finished = finish(
            &mut state,
            200,
            &reply("About the old screen.", true),
            &context(7, 1),
        );

        assert!(state.summaries.is_empty());
        assert!(state.summary_baselines.is_empty());
        assert!(
            finished.renames.is_empty(),
            "and the pane keeps the name it had"
        );
        // Dropped so the baseline guard is what answers below - the cooldown is its own test.
        state.summary_cooldown.clear();
        assert!(
            state.summary_due(7, OTHER_PANE),
            "and the screen it really is on is still owed a request",
        );
    }

    /// A `WebRequestResult` that never arrives must not leave the pane looking permanently
    /// mid-request, or it would never be summarised again for the life of the session.
    #[test]
    fn a_request_that_never_comes_back_stops_blocking_the_pane() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        assert!(!state.summary_due(7, OTHER_PANE), "blocked while in flight");

        // It survives exactly `SUMMARY_TICKS` heartbeats and goes on the next.
        for _ in 0..SUMMARY_TICKS {
            state.observe_activity_with(|_| sample(BUSY_PANE), no_ask);
        }
        assert!(!state.summary_requests.is_empty(), "still within its grace");
        state.observe_activity_with(|_| sample(BUSY_PANE), no_ask);

        assert!(state.summary_requests.is_empty(), "aged out");
        assert!(state.summary_due(7, BUSY_PANE));
    }

    /// A summary belongs to a pane, and dies with it - the same rule `activity` follows,
    /// so a new agent in a recycled pane id cannot inherit the old one's state.
    #[test]
    fn a_closed_pane_takes_its_summary_and_its_request_with_it() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("Done.", true), &context(7, 1));
        assert!(!state.summaries.is_empty());
        assert!(!state.summary_baselines.is_empty());

        state.panes = Some(vec![]);
        state.observe_activity_with(|_| sample(BUSY_PANE), no_ask);

        assert!(state.summaries.is_empty());
        assert!(state.summary_baselines.is_empty());
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
            assert!(!state.summary_baselines.is_empty());

            state.panes.as_mut().unwrap()[0].status = status;
            state.observe_activity_with(|_| sample(BUSY_PANE), no_ask);

            assert!(
                state.summaries.contains_key(&7),
                "{status:?} is a status change, not the pane closing"
            );
            assert!(state.summary_baselines.contains_key(&7));
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
}
