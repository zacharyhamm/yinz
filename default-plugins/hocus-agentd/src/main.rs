//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! The daemon takes one startup snapshot, follows Zellij's pane, command, and CWD
//! events, and samples agent output on a small heartbeat for activity and Git changes.

mod poll;

use hocus_proto::panes::{focused_pane, manifest_pane_ids, terminal_panes};
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
        let Some((panes, focused)) = poll::gather() else {
            return;
        };
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
        self.observe_activity_with(poll::read_pane_text);
    }

    fn observe_activity_with(&mut self, mut read: impl FnMut(u32) -> String) {
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
        self.activity.retain(|id, _| running.contains(id));
        for pane_id in running {
            agents::observe(&mut self.activity, pane_id, &read(pane_id));
        }
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

        state.observe_activity_with(|_| "working".to_owned());
        state.observe_activity_with(|_| "working".to_owned());
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));
        assert_eq!(state.activity.len(), 1);

        state.panes.as_mut().unwrap()[0].status = agents::AgentStatus::Held;
        state.observe_activity_with(|_| "ignored".to_owned());
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
        state.observe_activity_with(|_| "same".to_owned());
        state.observe_activity_with(|_| "same".to_owned());
        assert_eq!(state.activity.get(&7).map(|(_, polls)| *polls), Some(1));

        state.on_command(7, vec!["git".to_owned(), "status".to_owned()], true);
        state.observe_activity_with(|_| "same".to_owned());

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
