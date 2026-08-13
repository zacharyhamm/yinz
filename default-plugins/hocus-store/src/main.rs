//! hocus-store — the project registry and todo lists, reachable from a shell.
//!
//! Background-only plugin. Never rendered.
//!
//! These were hocus's own files. They live in the shared data store now, so
//! anything can reach them — another plugin, or `zellij pipe` from a script:
//!
//!     zellij pipe --name store_query \
//!         -- '{"op":"todo_list","args":{"project":"work"}}'
//!     zellij pipe --name store_mutate \
//!         -- '{"op":"todo_add","args":{"project":"work","text":"ship it"}}'
//!
//! Deliberately no `--plugin`: plugin identity is (location, configuration),
//! and a bare `zellij:hocus-store` url carries none of the alias' `shared true`
//! — so it would miss the running instance and launch a second store. A
//! broadcast only ever reaches plugins that are already running, and these two
//! message names are handled here and nowhere else, so it has one responder.
//!
//! What this plugin no longer is, is the *holder* of that data. It keeps no
//! registry and no lists of its own: with the rows in a shared namespace and
//! more than one writer, a copy here would be a cache that goes stale. Every op
//! reads what it needs and writes what it planned.
//!
//! Two jobs are left. Answering the pipes above, and running project discovery
//! off the pane snapshots hocus-agentd sends — which stays here because it has
//! to happen whether or not a hocus pane is open.
//!
//! Everything marked TEMPORARY below is the snapshot push hocus still depends
//! on, and goes in the commit that moves hocus onto the store.

mod ops;

use hocus_proto::store::{Backend, Host};
use hocus_proto::wire::{self, Mutation, PollReply, Query};
use hocus_proto::{agents, projects, store, todos};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// TEMPORARY, and deleted in the commit that moves hocus onto the store.
///
/// hocus still learns about projects and todos from a pushed snapshot, so until
/// it reads the shared namespace itself the snapshot has to keep arriving. It is
/// now assembled by *reading back* what was just written rather than from a copy
/// held here, which is slower and pointless and exists only so that this commit
/// leaves a working tree.
#[derive(Default)]
struct State {
    subscribers: Vec<u32>,
    seq: u64,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // No subscriptions and no permissions. Everything this plugin does is
        // driven by a pipe, and a builtin is granted the data store without
        // asking — asking would only strand it, see hocus-agentd.
        let backend = Host::shared();
        // Declared here as well as in hocus, because either may start first and
        // a fetch against an entity nobody has declared is an error. The
        // definitions come from one place so the two cannot disagree.
        if let Err(error) = backend.define_all(&store::shared_defs()) {
            // Nothing else will work, and there is no UI to say so in.
            eprintln!("hocus-store: {error}");
            return;
        }
        import_legacy(&backend);
        // Ask hocus-agentd for pane snapshots, which is where discovered
        // projects come from. Broadcast, because addressing it by url would
        // launch a second daemon rather than find the running one — the host
        // injects `caller_cwd` into piped configuration and plugin identity is
        // (location, configuration). Deliberately no payload: the daemon reads
        // manual kind overrides out of one, and this plugin has none to give.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
        // TEMPORARY, with `publish`: a reloaded store has no subscribers, and
        // hocus cannot tell a quiet store from one with nothing to say.
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
            // TEMPORARY, with `publish`.
            wire::STORE_SUBSCRIBE => {
                if let PipeSource::Plugin(id) = pipe_message.source {
                    if !self.subscribers.contains(&id) {
                        self.subscribers.push(id);
                    }
                }
                self.publish();
            },
            wire::AGENT_SNAPSHOT => {
                discover_from(&Host::shared(), &payload);
                self.publish();
            },
            wire::STORE_MUTATE => self.handle_mutate(pipe_message.source, &payload),
            wire::STORE_QUERY => self.handle_query(pipe_message.source, &payload),
            _ => {},
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn handle_mutate(&mut self, source: PipeSource, payload: &str) {
        let outcome = match serde_json::from_str::<Mutation>(payload) {
            Ok(mutation) => (
                mutation.request_id,
                ops::run_mutate(&Host::shared(), &mutation.op, &mutation.args),
            ),
            Err(_) => (String::new(), Err("could not parse mutation".to_owned())),
        };
        answer(source, outcome.0, outcome.1);
        self.publish();
    }

    /// TEMPORARY. Push the whole store to everyone watching, read back out of
    /// the store rather than held here. Deleted with the snapshot protocol.
    fn publish(&mut self) {
        if self.subscribers.is_empty() {
            return;
        }
        let backend = Host::shared();
        let (Ok(project_rows), Ok(todo_rows)) = (
            backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT)),
            backend.fetch(
                &FetchRequest::entity("todo")
                    .sort_by("id", SortDir::Asc)
                    .limit(store::LIST_FETCH_LIMIT),
            ),
        ) else {
            return;
        };
        self.seq += 1;
        let snapshot = wire::StoreSnapshot {
            projects: projects::from_rows(&project_rows),
            todos: todos::from_rows(&todo_rows),
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

    fn handle_query(&mut self, source: PipeSource, payload: &str) {
        // A payload we could not parse has no id to echo, so an unparseable
        // query from a plugin gets an answer it cannot correlate. Nothing
        // better is available, and silence would be worse.
        let (request_id, outcome) = match serde_json::from_str::<Query>(payload) {
            Ok(query) => (
                query.request_id,
                ops::run_query(&Host::shared(), &query.op, &query.args),
            ),
            Err(_) => (String::new(), Err("could not parse query".to_owned())),
        };
        answer(source, request_id, outcome);
    }
}

/// Add any project the agent snapshot implies, from the folders its panes are
/// sitting in.
///
/// Driven by the daemon's snapshot rather than by a poll of our own: this plugin
/// has no reason to look at panes otherwise, and running discovery off the same
/// snapshot hocus sees keeps the two from disagreeing about which projects
/// exist.
///
/// One apply for the projects and the moved lists together. That is new: they
/// used to be two independent file writes, so a crash between them left a
/// project pointing at a root whose todos were still under the old one.
fn discover_from(backend: &impl Backend, payload: &str) {
    let Ok(reply) = serde_json::from_str::<PollReply>(payload) else {
        return;
    };
    let Some(panes) = reply.panes else {
        return;
    };
    // Only an agent counts. A shell sitting at a prompt is not a reason to
    // remember a folder for good, and every pane would otherwise become a
    // project the first time someone cd'd somewhere.
    let repositories: Vec<projects::Repository> = panes
        .iter()
        .filter(|pane| agents::classify(pane, &reply.manual_kinds).is_some())
        .filter_map(|pane| reply.repositories.get(pane.cwd.as_deref()?).cloned())
        .collect();

    let Ok(rows) = backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
    else {
        return;
    };
    let before = projects::from_rows(&rows);
    let mut after = before.clone();
    let discovery = projects::discover(&mut after, &repositories);
    if !discovery.changed() {
        return;
    }
    let mut mutations = projects::diff_mutations(&before, &after);
    for (old, new) in discovery.root_moves {
        if let Ok(moved) = todos::plan_move_root(&old, &new) {
            mutations.extend(moved);
        }
    }
    let _ = backend.apply(mutations);
}

/// Read the two JSON files in, once, if this is the first launch that has the
/// store.
///
/// Guarded by a marker row written in the same transaction as the rows it
/// describes, so the import is atomic and cannot half-happen. A marker rather
/// than "is the table empty?" — emptiness is a state a user can reach honestly,
/// by crossing off and clearing the last todo, and that must not resurrect the
/// file on the next launch.
fn import_legacy(backend: &impl Backend) {
    if !imported(backend, "projects") {
        let registry = projects::parse_legacy(&legacy_bytes(projects::FILE));
        let mut mutations: Vec<zellij_tile::prelude::Mutation> =
            registry.values().map(projects::to_mutation).collect();
        mutations.push(marker("projects"));
        let _ = backend.apply(mutations);
    }
    if !imported(backend, "todos") {
        let lists = todos::parse_legacy(&legacy_bytes(todos::FILE));
        // One apply per level per list, because a child's parent_id is the id
        // the store gave its parent, which is only known once that row exists.
        for (root, list) in &lists {
            let mut new_ids: BTreeMap<u64, i64> = BTreeMap::new();
            for level in todos::levels_for_import(list) {
                let Ok(report) = backend.apply(todos::import_mutations(root, &level, &new_ids))
                else {
                    break;
                };
                for (pending, id) in level.iter().zip(report.ids) {
                    new_ids.insert(pending.old_id, id);
                }
            }
        }
        let _ = backend.apply(vec![marker("todos")]);
    }
}

fn marker(what: &str) -> zellij_tile::prelude::Mutation {
    zellij_tile::prelude::Mutation::put("import_marker")
        .uid(what)
        .set("what", what)
        .build()
}

fn imported(backend: &impl Backend, what: &str) -> bool {
    backend
        .count(&FetchRequest::entity("import_marker").filter(Pred::eq("what", what)))
        .map(|count| count > 0)
        // An error here means the store is unreachable, and the import would
        // fail too. Claiming it is done is the safe direction: a retry next
        // launch is fine, a double import is not.
        .unwrap_or(true)
}

#[cfg(target_family = "wasm")]
fn legacy_bytes(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// Native builds run the tests, where there is no `/cache` to read.
#[cfg(not(target_family = "wasm"))]
fn legacy_bytes(_path: &str) -> Vec<u8> {
    Vec::new()
}

/// Reply to whoever asked, in the way that kind of caller can hear.
///
/// A CLI pipe is a blocked process waiting on stdout, not a plugin with an id —
/// answering it the other way would hang the terminal it was typed in.
///
/// An empty `request_id` is not a reason to stay silent: a caller that sent none
/// matches nothing when the reply lands, which is what a fire-and-forget write
/// already relies on.
///
/// The `Plugin` arm is TEMPORARY. hocus still parks a tool call waiting on a
/// `STORE_RESULT`, and goes on to time out after five seconds without one. It
/// goes when hocus writes the store itself and stops asking.
fn answer(source: PipeSource, request_id: String, outcome: ops::OpResult) {
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
            let result = wire::StoreResult {
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
