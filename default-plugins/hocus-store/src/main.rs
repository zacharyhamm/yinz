//! hocus-store — owns the project registry and the todo lists.
//!
//! Background-only plugin. Never rendered.
//!
//! These were hocus's own files. They live out here so that anything else can
//! reach them — another plugin, or `zellij pipe` from a script — rather than
//! only the one plugin that happened to be holding them:
//!
//!     zellij pipe --name store_query \
//!         -- '{"op":"todo_list","args":{"project":"work"}}'
//!     zellij pipe --name store_mutate \
//!         -- '{"op":"todo_add","args":{"project":"work","text":"ship it"}}'
//!
//! Deliberately no `--plugin`: plugin identity is (location, configuration),
//! and a bare `zellij:hocus-store` url carries none of the alias' `shared true`
//! — so it would miss the running instance and launch a second store, which
//! owns the same two files and would write them from under the first. A
//! broadcast only ever reaches plugins that are already running, and these two
//! message names are handled here and nowhere else, so it has one responder.
//!
//! hocus keeps a read-only copy, pushed here on every change, so its own list
//! and search tools answer instantly and only writes cross the pipe.

mod ops;

use hocus_proto::wire::{self, Mutation, PollReply, Query, StoreResult, StoreSnapshot};
use hocus_proto::{agents, projects, todos};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    projects: projects::Registry,
    todos: todos::Store,
    /// Everyone to push snapshots to. A list rather than a single slot because
    /// a store is the kind of thing several plugins might reasonably watch, and
    /// dropping one because another subscribed would be a silent bug in
    /// whichever lost.
    subscribers: Vec<u32>,
    seq: u64,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // No subscriptions and no permissions: everything this plugin does is
        // driven by a pipe, and its files are reached through the WASI preopen
        // rather than through a gated host call. (It is a builtin besides, so
        // asking would only strand it — see hocus-agentd.)
        self.projects = projects::load();
        self.todos = todos::load();
        // Ask hocus-agentd for pane snapshots, which is where discovered
        // projects come from. Broadcast, because addressing it by url would
        // launch a second daemon rather than find the running one — the host
        // injects `caller_cwd` into piped configuration and plugin identity is
        // (location, configuration). Deliberately no payload: the daemon reads
        // manual kind overrides out of one, and this plugin has none to give.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
        // And tell anyone who was already watching to say so again: this is a
        // fresh `State`, so a reload just dropped every subscriber it had.
        // Mutations would keep working and the snapshots would go nowhere,
        // which reads as a store that answers writes and never changes.
        // Broadcast for the same reason as above.
        pipe_message_to_plugin(MessageToPlugin::new(wire::STORE_HELLO));
    }

    fn update(&mut self, _event: Event) -> bool {
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let payload = pipe_message.payload.unwrap_or_default();
        match pipe_message.name.as_str() {
            wire::AGENT_HELLO => {
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
            },
            wire::STORE_SUBSCRIBE => {
                if let PipeSource::Plugin(id) = pipe_message.source {
                    if !self.subscribers.contains(&id) {
                        self.subscribers.push(id);
                    }
                }
                // Answer at once: a subscriber that just started has nothing,
                // and nothing here changes on a timer to give it something.
                self.publish();
            },
            wire::AGENT_SNAPSHOT => self.discover_from(&payload),
            wire::STORE_MUTATE => self.handle_mutate(pipe_message.source, &payload),
            wire::STORE_QUERY => self.handle_query(pipe_message.source, &payload),
            _ => {},
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    /// Add any project the agent snapshot implies, from the folders its panes
    /// are sitting in.
    ///
    /// Driven by the daemon's snapshot rather than by a poll of our own: this
    /// plugin has no reason to look at panes otherwise, and running discovery
    /// off the same snapshot hocus sees keeps the two from disagreeing about
    /// which projects exist.
    fn discover_from(&mut self, payload: &str) {
        let Ok(reply) = serde_json::from_str::<PollReply>(payload) else {
            return;
        };
        let Some(panes) = reply.panes else {
            return;
        };
        // Only an agent counts. A shell sitting at a prompt is not a reason to
        // remember a folder for good, and every pane would otherwise become a
        // project the first time someone cd'd somewhere.
        let cwds: Vec<String> = panes
            .iter()
            .filter(|pane| agents::classify(pane, &reply.manual_kinds).is_some())
            .filter_map(|pane| pane.cwd.clone())
            .collect();
        if !projects::discover(&mut self.projects, &cwds).changed() {
            return;
        }
        self.persist_projects();
        self.publish();
    }

    fn handle_mutate(&mut self, source: PipeSource, payload: &str) {
        let Ok(mutation) = serde_json::from_str::<Mutation>(payload) else {
            self.answer(
                source,
                String::new(),
                Err("could not parse mutation".to_owned()),
            );
            return;
        };
        let outcome = ops::mutate(
            &mut self.projects,
            &mut self.todos,
            &mutation.op,
            &mutation.args,
        );
        if outcome.is_ok() {
            // Only on success: a rejected call left the stores alone, and
            // writing them anyway would rewrite both files for nothing.
            self.persist_projects();
            self.persist_todos();
        }
        self.answer(source, mutation.request_id, outcome);
        self.publish();
    }

    fn handle_query(&mut self, source: PipeSource, payload: &str) {
        // A payload we could not parse has no id to echo, so an unparseable
        // query from a plugin gets an answer it cannot correlate. Nothing
        // better is available, and silence would be worse.
        let (request_id, outcome) = match serde_json::from_str::<Query>(payload) {
            Ok(query) => (
                query.request_id,
                ops::query(&self.projects, &self.todos, &query.op, &query.args),
            ),
            Err(_) => (String::new(), Err("could not parse query".to_owned())),
        };
        self.answer(source, request_id, outcome);
    }

    /// Reply to whoever asked, in the way that kind of caller can hear.
    ///
    /// A CLI pipe is a blocked process waiting on stdout, not a plugin with an
    /// id — answering it the other way would hang the terminal it was typed in.
    /// An empty `request_id` is not a reason to stay silent: a caller that sent
    /// none matches nothing when the reply lands, which is what a
    /// fire-and-forget write already relies on.
    fn answer(&mut self, source: PipeSource, request_id: String, outcome: ops::OpResult) {
        let (ok, message) = match outcome {
            Ok(message) => (true, message),
            Err(message) => (false, message),
        };
        match source {
            PipeSource::Cli(pipe_id) => {
                cli_pipe_output(&pipe_id, &format!("{message}\n"));
                unblock_cli_pipe_input(&pipe_id);
            },
            PipeSource::Plugin(id) => {
                let result = StoreResult {
                    request_id,
                    ok,
                    message,
                };
                if let Ok(payload) = serde_json::to_string(&result) {
                    pipe_message_to_plugin(
                        MessageToPlugin::new(wire::STORE_RESULT)
                            .with_destination_plugin_id(id)
                            .with_payload(payload),
                    );
                }
            },
            PipeSource::Keybind => {},
        }
    }

    /// Push the whole store to everyone watching.
    ///
    /// Whole-state rather than a delta because the store is small and a dropped
    /// delta is an invisible divergence, where a dropped snapshot is corrected
    /// by the next one.
    fn publish(&mut self) {
        if self.subscribers.is_empty() {
            return;
        }
        self.seq += 1;
        let snapshot = StoreSnapshot {
            projects: self.projects.clone(),
            todos: self.todos.clone(),
            seq: self.seq,
        };
        let Ok(payload) = serde_json::to_string(&snapshot) else {
            return;
        };
        // By id, never by url: a url would *launch* a subscriber that had since
        // closed, which is the last thing a background store should do.
        for id in &self.subscribers {
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::STORE_SNAPSHOT)
                    .with_destination_plugin_id(*id)
                    .with_payload(payload.clone()),
            );
        }
    }

    fn persist_projects(&self) {
        if let Some(json) = projects::json(&self.projects) {
            projects::write(json.as_bytes());
        }
    }

    fn persist_todos(&self) {
        if let Some(json) = todos::json(&self.todos) {
            todos::write(json.as_bytes());
        }
    }
}
