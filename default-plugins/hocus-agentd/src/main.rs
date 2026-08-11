//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! The daemon takes one startup snapshot, then follows Zellij's pane, command,
//! and CWD events. It remains alive after hocus closes, so project discovery and
//! agent detection keep their session-wide state warm without polling.

mod poll;

use hocus_proto::panes::terminal_panes;
use hocus_proto::wire::{self, PollReply, PollRequest};
use hocus_proto::{agents, projects};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    /// Overrides from hocus's `agent_register`, refreshed by every subscribe.
    /// hocus stays authoritative; this is only an input to `classify`.
    manual_kinds: BTreeMap<u32, String>,
    /// The event-fed terminal-pane snapshot. `None` means the startup scan could
    /// not find the current session yet.
    panes: Option<Vec<agents::AgentPane>>,
    manifest: PaneManifest,
    tabs: Vec<TabInfo>,
    commands: BTreeMap<u32, String>,
    cwds: BTreeMap<u32, String>,
    repositories: BTreeMap<String, projects::Repository>,
    git_probes: BTreeMap<String, GitProbe>,
    /// Who to push snapshots to — hocus, and hocus-store for its project
    /// discovery. A list rather than a single slot because both want the same
    /// snapshot, and broadcasting it instead would hand every plugin in the
    /// session the title and working folder of every pane.
    subscribers: Vec<u32>,
    seq: u64,
}

#[derive(Default)]
struct GitProbe {
    metadata: Option<Vec<u8>>,
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
            Event::CommandChanged(PaneId::Terminal(id), command, _, _) => {
                let command = (!command.is_empty()).then(|| command.join(" "));
                if let Some(command) = &command {
                    self.commands.insert(id, command.clone());
                } else {
                    self.commands.remove(&id);
                }
                self.update_pane(id, |pane| pane.command = command);
                self.resolve_repositories();
                self.publish();
            },
            Event::CwdChanged(PaneId::Terminal(id), cwd, _) => {
                let cwd = cwd.to_string_lossy().into_owned();
                self.cwds.insert(id, cwd.clone());
                self.update_pane(id, |pane| pane.cwd = Some(cwd));
                self.resolve_repositories();
                self.publish();
            },
            Event::RunCommandResult(exit_code, stdout, _, context) => {
                self.finish_git_probe(exit_code, stdout, context);
            },
            _ => {},
        }
        // Never rendered, so never asks to be.
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
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
    fn bootstrap(&mut self) {
        let Some(panes) = poll::gather() else {
            return;
        };
        for pane in &panes {
            if let Some(command) = &pane.command {
                self.commands.insert(pane.pane_id, command.clone());
            }
            if let Some(cwd) = &pane.cwd {
                self.cwds.insert(pane.pane_id, cwd.clone());
            }
        }
        self.panes = Some(panes);
        self.resolve_repositories();
    }

    fn rebuild_panes(&mut self) {
        let mut panes = terminal_panes(&self.manifest, &self.tabs);
        for pane in &mut panes {
            if let Some(command) = self.commands.get(&pane.pane_id) {
                pane.command = Some(command.clone());
            }
            pane.cwd = self.cwds.get(&pane.pane_id).cloned();
        }
        self.commands
            .retain(|id, _| panes.iter().any(|pane| pane.pane_id == *id));
        self.cwds
            .retain(|id, _| panes.iter().any(|pane| pane.pane_id == *id));
        self.panes = Some(panes);
        self.resolve_repositories();
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

    fn resolve_repositories(&mut self) {
        let Some(panes) = &self.panes else {
            return;
        };
        let unresolved: Vec<String> = panes
            .iter()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| pane.cwd.clone())
            .filter(|cwd| {
                !self.repositories.contains_key(cwd) && !self.git_probes.contains_key(cwd)
            })
            .collect();
        for cwd in unresolved {
            self.git_probes.insert(cwd.clone(), GitProbe::default());
            let context = |kind: &str| {
                BTreeMap::from([
                    ("git_probe".to_owned(), kind.to_owned()),
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
                    "^(remote\\..*\\.url|remote\\.pushDefault|branch\\..*\\.remote)$",
                ],
                BTreeMap::new(),
                cwd.clone().into(),
                context("config"),
            );
        }
    }

    fn finish_git_probe(
        &mut self,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        context: BTreeMap<String, String>,
    ) {
        let (Some(cwd), Some(kind)) = (context.get("cwd"), context.get("git_probe")) else {
            return;
        };
        let Some(probe) = self.git_probes.get_mut(cwd) else {
            return;
        };
        let output = if exit_code == Some(0) {
            stdout
        } else {
            Vec::new()
        };
        match kind.as_str() {
            "metadata" => probe.metadata = Some(output),
            "config" => probe.config = Some(output),
            _ => return,
        }
        if probe.metadata.is_none() || probe.config.is_none() {
            return;
        }
        let Some(probe) = self.git_probes.remove(cwd) else {
            return;
        };
        self.repositories.insert(
            cwd.clone(),
            poll::repository(
                cwd,
                probe.metadata.as_deref().unwrap_or_default(),
                probe.config.as_deref().unwrap_or_default(),
            ),
        );
        self.publish();
    }

    fn publish(&mut self) {
        self.seq += 1;
        let reply = PollReply {
            panes: self.panes.clone(),
            activity: agents::Activity::new(),
            manual_kinds: self.manual_kinds.clone(),
            repositories: self.repositories.clone(),
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
}
