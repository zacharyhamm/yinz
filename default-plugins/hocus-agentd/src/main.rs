//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! The daemon takes one startup snapshot, follows Zellij's pane, command, and CWD
//! events, and samples agent output on a small heartbeat for activity and Git changes.

mod poll;

use hocus_proto::llm::Endpoint;
use hocus_proto::panes::{focused_pane, focused_plugin_pane, manifest_pane_ids, terminal_panes};
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
/// finished turn does, and each of those settles on a *different* screen, so the hash guard
/// does not catch them either. This is the ceiling that does not depend on telling those
/// apart: one request per pane per minute, however the screen churns.
///
/// Equal to `SUMMARY_TICKS` on purpose: the two windows expire within a tick of each
/// other, so a dropped reply costs the pane about a minute and then lets it ask again,
/// rather than either guard outliving the other and holding the pane silent for longer.
const SUMMARY_COOLDOWN_TICKS: u32 = 12;

/// Context keys correlating a summary's `WebRequestResult` back to the pane that asked.
/// The same scheme the git probes use, for the same reason: results arrive on one event
/// for the whole plugin.
const SUMMARY_PANE: &str = "summary_pane";
const SUMMARY_ID: &str = "summary_id";

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
    /// hocus is a global pane and hears no `PaneUpdate` for itself.
    focused: Option<u32>,
    /// The plugin pane the user is looking at, passed on for the same reason and to
    /// the same reader — it is how hocus tells the agents pane above it from itself.
    focused_plugin: Option<u32>,
    commands: BTreeMap<u32, String>,
    cwds: BTreeMap<u32, String>,
    activity: agents::Activity,
    repositories: BTreeMap<String, projects::Repository>,
    git_probes: BTreeMap<String, GitProbe>,
    /// Seconds left before a folder whose probe answered nothing is tried again.
    probe_backoff: BTreeMap<String, f64>,
    git_probe_seq: u64,
    git_refresh_elapsed: f64,
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
    /// Panes with a summary request in flight. One at a time per pane, so a flurry of
    /// idle edges cannot fan out into a flurry of requests.
    summary_requests: BTreeMap<u32, InFlight>,
    /// Heartbeats left before each pane may ask again, counted down by every poll.
    /// An absent pane is free to ask — see `SUMMARY_COOLDOWN_TICKS`.
    summary_cooldown: BTreeMap<u32, u32>,
    summary_seq: u64,
}

/// A summary request that has gone out and not yet come back.
struct InFlight {
    id: u64,
    /// The pane hash the request was made from, so the answer is stamped with the screen
    /// it actually describes rather than with whatever the pane shows when it lands.
    hash: u64,
    /// Heartbeats this has been outstanding. A `WebRequestResult` that never arrives -
    /// a dropped event, a host that gave up - would otherwise leave the pane looking
    /// permanently mid-request and never summarised again for the life of the session.
    ticks: u32,
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
                self.observe_activity();
                self.repair_commands();
                self.repair_cwds();
                self.expire_probe_backoff(elapsed);
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
        // A pane that closes stops being an agent because it stops being in the
        // manifest — the same rule as the two maps above, which is what keeps
        // the latch from being one more thing that can go stale.
        self.latched.retain(|id, _| live.contains(id));
        self.pending_release.retain(|id| live.contains(id));
        // A pane that has come back to life is running a new process, and has to
        // earn its latch again rather than inherit the dead one's. `settle_latches`
        // below re-earns it in the same breath if it really is running an agent.
        //
        // Only for a pane we were already showing: a re-run happens in place, so
        // the pane never left the list. One arriving from somewhere else — a pane
        // coming back from being suppressed — has been running all along, and
        // reading it as a re-run would take the latch that survived the
        // suppression in the first place.
        for pane in &panes {
            if pane.status == agents::AgentStatus::Running
                && was_shown.contains(&pane.pane_id)
                && !was_running.contains(&pane.pane_id)
            {
                self.latched.remove(&pane.pane_id);
                self.pending_release.remove(&pane.pane_id);
            }
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
        let live: BTreeSet<&String> = self.cwds.values().collect();
        self.repositories.retain(|cwd, _| live.contains(cwd));
        self.git_probes.retain(|cwd, _| live.contains(cwd));
        self.probe_backoff.retain(|cwd, _| live.contains(cwd));
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
    fn repository_cwds(&self, refresh: bool) -> Vec<String> {
        let agents = self.agent_cwds();
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        agents
            .iter()
            .chain(self.cwds.values())
            // One folder can hold several panes, and an agent folder is in both
            // halves of the chain.
            .filter(|cwd| seen.insert(cwd))
            .filter(|cwd| refresh || !self.repositories.contains_key(*cwd))
            // A probe already in flight answers for this folder either way, so
            // even a forced refresh must not start a second one.
            .filter(|cwd| !self.git_probes.contains_key(*cwd))
            // Nothing is cached for a probe that answered nothing, so without
            // this the next event would run it again immediately.
            .filter(|cwd| !self.probe_backoff.contains_key(*cwd))
            // Two processes per folder, and this runs on nearly every host
            // event. What is left over is picked up by the next event or the
            // heartbeat, and every filter above makes the pass idempotent.
            .take(GIT_PROBES_PER_PASS)
            .cloned()
            .collect()
    }

    fn agent_cwds(&self) -> BTreeSet<String> {
        self.panes
            .iter()
            .flatten()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| pane.cwd.clone())
            .collect()
    }

    fn start_git_probes(&mut self, cwds: Vec<String>) {
        for cwd in cwds {
            self.git_probe_seq += 1;
            let id = self.git_probe_seq;
            self.git_probes.insert(
                cwd.clone(),
                GitProbe {
                    id,
                    ..GitProbe::default()
                },
            );
            let context = |kind: &str| {
                BTreeMap::from([
                    ("git_probe".to_owned(), kind.to_owned()),
                    ("git_probe_id".to_owned(), id.to_string()),
                    ("cwd".to_owned(), cwd.clone()),
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

    fn observe_activity(&mut self) {
        self.observe_activity_with(poll::read_pane_text, Self::start_summary);
    }

    /// Sample every agent pane, and act on the ones that just stopped working.
    ///
    /// `read` and `ask` are injected for the same reason: both reach the host, and this
    /// has to stay testable on the host triple.
    fn observe_activity_with(
        &mut self,
        mut read: impl FnMut(u32) -> String,
        mut ask: impl FnMut(&mut Self, u32, &str),
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
        self.summaries.retain(|id, _| known.contains(id));
        self.summary_requests.retain(|id, request| {
            request.ticks += 1;
            running.contains(id) && request.ticks <= SUMMARY_TICKS
        });
        // The same shape one line up, and pruned by the same rule: a pane that stops being
        // a running agent starts clean if it comes back.
        self.summary_cooldown.retain(|id, ticks| {
            *ticks = ticks.saturating_sub(1);
            running.contains(id) && *ticks > 0
        });
        for pane_id in running {
            let text = read(pane_id);
            match agents::observe(&mut self.activity, pane_id, &text) {
                // `>=`, not equality: what stops a settled pane asking every five seconds is
                // the cooldown and the hash guard in `summary_due`, not this arm. Equality
                // would drop a settling the cooldown happened to swallow and never come back
                // to it, losing that pane its summary for the whole stretch.
                agents::Observed::Idle { polls } if polls >= SUMMARY_SETTLE_POLLS => {
                    ask(self, pane_id, &text)
                },
                // Busy, or still settling. Note that a pane going busy keeps the summary it
                // already has rather than dropping it: the marker is gated on the pane
                // being idle (`agents::waiting_marker`), so a working agent already shows
                // nothing - and keeping the answer, stamped with the screen it was made
                // from, is what lets an agent that comes back to that same screen be free
                // rather than another request.
                _ => {},
            }
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
    fn start_summary(&mut self, pane_id: u32, text: &str) {
        if !self.summary_due(pane_id, text) {
            return;
        }
        let Some(endpoint) = self.summary_endpoint() else {
            return;
        };
        // Wrapped rather than passed by name: `web_request` is generic over its url type,
        // and inference pins that to one lifetime, which will not satisfy the for-any-
        // lifetime bound `send_summary` needs.
        self.send_summary(
            pane_id,
            text,
            &endpoint,
            |url, verb, headers, body, context| web_request(url, verb, headers, body, context),
        );
    }

    /// Whether this settling is worth an HTTP request.
    ///
    /// No when the pane asked less than a minute ago, when one is already in flight for it,
    /// when it holds too little to say anything about, or when it is showing exactly what
    /// it was showing the last time it was summarised. That last one is what makes an agent
    /// that goes idle, is nudged, and settles back on the same screen free rather than
    /// another request.
    fn summary_due(&self, pane_id: u32, text: &str) -> bool {
        // First, because it is the cheapest and the one that fires most: a busy agent
        // settles for fifteen seconds several times a turn, and only the first is paid for.
        if self.summary_cooldown.contains_key(&pane_id) {
            return false;
        }
        if self.summary_requests.contains_key(&pane_id) || !agents::worth_summarizing(text) {
            return false;
        }
        match agents::activity_hash(&self.activity, pane_id) {
            Some(hash) => !self.summaries.get(&pane_id).is_some_and(|s| s.hash == hash),
            // No hash means the poll that would have recorded one has not run, so there is
            // nothing to stamp an answer with.
            None => false,
        }
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
        let branch = pane
            .cwd
            .as_ref()
            .and_then(|cwd| self.repositories.get(cwd))
            .and_then(|repository| repository.branch.clone());
        let body = agents::summary_request_body(
            &endpoint.model,
            kind,
            pane.cwd.as_deref(),
            branch.as_deref(),
            &agents::tail(text, agents::SUMMARY_LINES, agents::SUMMARY_CHARS),
        );

        // Unwrap-free: `summary_due` is what gates every caller, and it has already
        // established that this pane has a hash to stamp an answer with.
        let hash = agents::activity_hash(&self.activity, pane_id).unwrap_or_default();
        self.summary_seq += 1;
        let id = self.summary_seq;
        self.summary_requests
            .insert(pane_id, InFlight { id, hash, ticks: 0 });
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
            notify,
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
        toast: impl FnOnce(&str),
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
        let hash = self
            .summary_requests
            .remove(&pane_id)
            .map(|request| request.hash)
            .unwrap_or_default();
        // The pane has moved on since we asked, so this answer describes a screen that is
        // gone. Compared against the hash the *request* was made from rather than
        // whatever the pane shows now: stamping a stale answer with the current screen
        // would make `summary_due` skip the request that screen is actually owed.
        if agents::activity_hash(&self.activity, pane_id) != Some(hash) {
            return;
        }
        // A failed summary is simply no summary — the same stance `ProbeOutcome::Failed`
        // takes, and for the same reason: a guess here would outlive the reason for it.
        // The next idle edge asks again.
        if !(200..300).contains(&status) {
            return;
        }
        let Some(summary) = agents::parse_summary(body, hash) else {
            return;
        };
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
        toast(&agents::summary_toast(
            pane_id,
            self.agent_kind(pane_id),
            &summary,
        ));
        self.summaries.insert(pane_id, summary);
        self.publish();
    }

    /// What kind of agent a pane is running, if it is one we can still classify.
    ///
    /// `None` for a pane that has closed or stopped looking like an agent between the
    /// request going out and its answer landing — the toast simply goes without the label.
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
        let (Some(cwd), Some(kind), Some(id)) = (
            context.get("cwd"),
            context.get("git_probe"),
            context
                .get("git_probe_id")
                .and_then(|id| id.parse::<u64>().ok()),
        ) else {
            return;
        };
        let Some(probe) = self.git_probes.get_mut(cwd).filter(|probe| probe.id == id) else {
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
        let Some(probe) = self.git_probes.remove(cwd) else {
            return;
        };
        let (exit_code, metadata) = probe.metadata.unwrap_or_default();
        let cwd = cwd.clone();
        match poll::repository(
            exit_code,
            &metadata,
            probe.config.as_deref().unwrap_or_default(),
        ) {
            poll::ProbeOutcome::Checkout(repository) => {
                self.repositories.insert(cwd, repository);
            },
            // Recorded, because "we looked here and Git said no" is what stops
            // the next event probing it again.
            poll::ProbeOutcome::NotARepository => {
                self.repositories
                    .insert(cwd.clone(), projects::Repository::folder(cwd));
            },
            // Deliberately not recorded. A guess here would outlive the reason
            // for it and deny the pane a project for as long as the entry lives.
            poll::ProbeOutcome::Failed => {
                self.probe_backoff.insert(cwd, GIT_RETRY_SECONDS);
            },
        }
        self.publish();
    }

    fn publish(&mut self) {
        self.seq += 1;
        let reply = PollReply {
            panes: self.panes.clone(),
            activity: self.activity.clone(),
            manual_kinds: self.manual_kinds.clone(),
            repositories: self.repositories.clone(),
            focused: self.focused,
            focused_plugin: self.focused_plugin,
            summaries: self.summaries.clone(),
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

    /// The summariser, for tests about everything else. `start_summary` reaches the host
    /// twice - the alias configuration and the request itself - so no test that is not
    /// about summaries may run it.
    fn no_ask(_: &mut State, _: u32, _: &str) {}

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

        state.observe_activity_with(|_| "working".to_owned(), no_ask);
        state.observe_activity_with(|_| "working".to_owned(), no_ask);
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));
        assert_eq!(state.activity.len(), 1);

        state.panes.as_mut().unwrap()[0].status = agents::AgentStatus::Held;
        state.observe_activity_with(|_| "ignored".to_owned(), no_ask);
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

    #[test]
    fn repository_refresh_reprobes_cached_cwds_without_overlapping() {
        let mut state = agent_in(7, "/work");
        state
            .repositories
            .insert("/work".to_owned(), projects::Repository::folder("/work"));

        assert!(state.repository_cwds(false).is_empty());
        assert_eq!(state.repository_cwds(true), vec!["/work"]);
        state
            .git_probes
            .insert("/work".to_owned(), GitProbe::default());
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

        assert_eq!(state.repository_cwds(false), vec!["/work"]);

        // And it is still asked only once.
        state
            .repositories
            .insert("/work".to_owned(), projects::Repository::folder("/work"));
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

        assert_eq!(state.repository_cwds(false), vec!["/work/hocus", worktree]);
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
            asked.first().map(String::as_str),
            Some("/work/hocus"),
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
            .insert("/work".to_owned(), projects::Repository::folder("/work"));
        state
            .git_probes
            .insert("/work".to_owned(), GitProbe::default());

        let pane = &mut state.panes.as_mut().unwrap()[0];
        pane.command = None;
        pane.title = "shell".to_owned();
        assert!(
            agents::classify(&state.panes.as_ref().unwrap()[0], &state.manual_kinds).is_none(),
            "the pane really has stopped looking like an agent"
        );
        state.resolve_repositories();

        assert!(state.repositories.contains_key("/work"));
        assert!(state.git_probes.contains_key("/work"));
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
            .insert("/work".to_owned(), projects::Repository::folder("/work"));
        state
            .git_probes
            .insert("/work".to_owned(), GitProbe::default());

        state.on_command(7, vec!["zsh".to_owned()], false);

        assert!(
            state.pending_release.contains(&7),
            "the release is still proposed",
        );
        assert!(state.repositories.contains_key("/work"));
        assert!(
            state.git_probes.contains_key("/work"),
            "and the probe in flight was not cancelled either",
        );
    }

    /// The eviction that is meant to happen: a pane really did move, so what was
    /// learned about where it used to be is no longer about this pane.
    #[test]
    fn a_pane_that_changes_folder_gives_up_the_old_one_and_probes_the_new() {
        let mut state = agent_in(7, "/work/a");
        state.repositories.insert(
            "/work/a".to_owned(),
            projects::Repository::folder("/work/a"),
        );

        state.record_cwd(7, "/work/b");
        state.rejoin_cwds();
        state.resolve_repositories();

        assert!(!state.repositories.contains_key("/work/a"));
        assert!(
            state.git_probes.contains_key("/work/b"),
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
            .insert("/work".to_owned(), projects::Repository::folder("/work"));

        state.panes = Some(vec![]);
        state.cwds.clear();
        state.resolve_repositories();

        assert!(!state.repositories.contains_key("/work"));
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
        state.observe_activity_with(|_| "same".to_owned(), no_ask);
        state.observe_activity_with(|_| "same".to_owned(), no_ask);
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));

        state.on_command(7, vec!["git".to_owned(), "status".to_owned()], true);
        state.observe_activity_with(|_| "same".to_owned(), no_ask);

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
            .insert("/work".to_owned(), projects::Repository::folder("/work"));
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
            state.repositories.contains_key("/work"),
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
        assert!(!state.repositories.contains_key("/work"));
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
            "/work".to_owned(),
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

        assert!(!state.repositories.contains_key("/work"), "no guess cached");
        assert!(state.probe_backoff.contains_key("/work"));
        assert!(state.repository_cwds(false).is_empty(), "not asked again");

        state.expire_probe_backoff(GIT_RETRY_SECONDS);
        assert!(state.probe_backoff.is_empty());
        assert_eq!(state.repository_cwds(false), vec!["/work"]);
    }

    // -- idle summaries -----------------------------------------------------

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
        state.observe_activity_with(
            |_| text.to_owned(),
            |state, pane_id, text| {
                if state.summary_due(pane_id, text) {
                    state.send_summary(pane_id, text, &endpoint(), |_, _, _, body, context| {
                        sent.push((context[SUMMARY_PANE].parse().unwrap(), body));
                    });
                }
            },
        );
    }

    /// Poll `text` for a whole cooldown, so a pane that is holding still goes on holding
    /// still while its window runs out. Enough polls to clear the window whichever tick of
    /// it the caller is on, and showing one screen throughout, so the hash guard sees no
    /// change either — what expires is the cooldown and nothing else.
    fn poll_out_the_cooldown(state: &mut State, text: &str, sent: &mut Vec<(u32, Vec<u8>)>) {
        for _ in 0..=SUMMARY_COOLDOWN_TICKS {
            poll_showing(state, text, sent);
        }
    }

    /// One agent, polled until it has settled, and the requests that went out.
    fn idle_agent(text: &'static str) -> (State, Vec<(u32, Vec<u8>)>) {
        let mut state = State {
            panes: Some(vec![pane(7, "claude", "/work")]),
            ..State::default()
        };
        let mut sent: Vec<(u32, Vec<u8>)> = Vec::new();
        // The first poll is `Busy` - a single reading cannot tell working from finished -
        // and the settle window is counted from the one after it.
        for _ in 0..=SUMMARY_SETTLE_POLLS {
            poll_showing(&mut state, text, &mut sent);
        }
        (state, sent)
    }

    /// The pane text a test agent shows - long enough to clear `worth_summarizing`.
    const BUSY_PANE: &str = "Running the test suite and waiting for it to finish now...";
    const OTHER_PANE: &str = "Something else entirely happened on this pane instead...";

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
        toasts: Vec<String>,
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
            |toast| finished.toasts.push(toast.to_owned()),
        );
        finished
    }

    fn context(pane_id: u32, id: u64) -> BTreeMap<String, String> {
        BTreeMap::from([
            (SUMMARY_PANE.to_owned(), pane_id.to_string()),
            (SUMMARY_ID.to_owned(), id.to_string()),
        ])
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
    /// minute, and after it lapses the hash guard is what has to carry this, because the
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
    /// and each such pause leaves the pane on a *different* screen, so the hash guard does
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
            )
            .iter()
            .any(|row| matches!(row, agents::StripRow::Agent { text, .. } if text.starts_with('!')))
        };
        assert!(waiting(&state), "idle and blocked: the marker is up");

        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);

        assert!(
            !waiting(&state),
            "working again, so nothing is waiting on the user"
        );
        assert!(
            state.summaries.contains_key(&7),
            "but the answer is still held"
        );
    }

    /// An agent that goes idle showing exactly what it was showing when it was last
    /// summarised has nothing new to say, and must not be paid for twice.
    #[test]
    fn an_unchanged_pane_is_not_summarized_again() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        finish(&mut state, 200, &reply("Done.", false), &context(7, 1));
        // Dropped rather than polled out, so what the assertions below answer for is the
        // hash guard alone and not the cooldown standing in front of it.
        state.summary_cooldown.clear();

        // Away and back to the same screen: busy, then idle again on identical text.
        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);
        state.observe_activity_with(|_| BUSY_PANE.to_owned(), no_ask);
        assert!(
            !state.summary_due(7, BUSY_PANE),
            "the stored summary was made from exactly this screen",
        );

        // A screen it has not been asked about is due again.
        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);
        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);
        assert!(state.summary_due(7, OTHER_PANE));
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
            vec!["agent idle · 7 · claude\nRan the tests.".to_owned()],
            "one summary, one toast, naming the pane and its kind",
        );
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
            vec!["agent waiting · 7 · claude\nShall I push?".to_owned()],
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
        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);

        let finished = finish(
            &mut state,
            200,
            &reply("Waiting on you.", true),
            &context(7, 1),
        );

        assert!(state.summaries.is_empty());
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
        assert!(state.summary_requests.is_empty());
        assert!(finished.toasts.is_empty());
    }

    /// A reply is stamped with the screen the *request* was made from, not with whatever
    /// the pane happens to show when it lands.
    ///
    /// The bug this guards: a pane that moves on while the request is out is still idle
    /// when the answer arrives, so a naive "is it busy?" check lets the stale answer
    /// through - and stamping it with the *current* hash then makes `summary_due` treat
    /// the new screen as already summarised, so the screen that is actually owed a
    /// request never gets one.
    #[test]
    fn an_answer_about_a_screen_the_pane_has_left_is_dropped() {
        let (mut state, _) = idle_agent(BUSY_PANE);
        // The pane moves on and settles somewhere else, all while the request is out.
        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);
        state.observe_activity_with(|_| OTHER_PANE.to_owned(), no_ask);
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
        assert!(
            finished.renames.is_empty(),
            "and the pane keeps the name it had"
        );
        // Dropped so the hash guard is what answers below - the cooldown is its own test.
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
            state.observe_activity_with(|_| BUSY_PANE.to_owned(), no_ask);
        }
        assert!(!state.summary_requests.is_empty(), "still within its grace");
        state.observe_activity_with(|_| BUSY_PANE.to_owned(), no_ask);

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

        state.panes = Some(vec![]);
        state.observe_activity_with(|_| BUSY_PANE.to_owned(), no_ask);

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
            state.observe_activity_with(|_| BUSY_PANE.to_owned(), no_ask);

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
                "/work".to_owned(),
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
                "/work".to_owned(),
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
        assert!(state.git_probes["/work"].metadata.is_none());
    }
}
