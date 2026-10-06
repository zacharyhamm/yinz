//! project-store — the project registry, todo lists and notes, reachable from a
//! shell.
//!
//! Background-only plugin. Never rendered.
//!
//! These were agent's own files. They live in the shared data store now, so
//! anything can reach them — another plugin, or `yinz pipe` from a script:
//!
//!     yinz pipe --name store_query \
//!         -- '{"op":"todo_list","args":{"project":"work"}}'
//!     yinz pipe --name store_mutate \
//!         -- '{"op":"todo_add","args":{"project":"work","text":"ship it"}}'
//!     yinz pipe --name store_mutate \
//!         -- '{"op":"note_add","args":{"project":"work","title":"why","body":"because"}}'
//!
//! Deliberately no `--plugin`: plugin identity is (location, configuration),
//! and a bare `yinz:project-store` url carries none of the alias' `shared true`
//! — so it would miss the running instance and launch a second store. A
//! host routes these two untargeted CLI message names to one existing store
//! instance, regardless of its alias configuration or the number of clients.
//!
//! What this plugin no longer is, is the *holder* of that data. It keeps no
//! registry and no lists of its own: with the rows in a shared namespace and
//! more than one writer, a copy here would be a cache that goes stale. Every op
//! reads what it needs and writes what it planned.
//!
//! Two jobs are left. Answering the pipes above, and running project discovery
//! off the pane snapshots agent-service sends — which stays here because it has
//! to happen whether or not a agent pane is open.

mod archive;
mod arrangement;
mod notes_ops;
mod ops;

#[cfg(test)]
#[path = "unit/legacy_import.rs"]
mod legacy_import_tests;

use agent_proto::store::{Backend, Host};
use agent_proto::wire::{self, Mutation, PollReply, Query};
use agent_proto::{agents, notes, projects, store, todos};
use std::collections::BTreeMap;
use yinz_tile::prelude::DataRowId;
use yinz_tile::prelude::DataWatchId;
use yinz_tile::prelude::HostId;
use yinz_tile::prelude::TabId;
use yinz_tile::prelude::*;

const RETRY_SECONDS: f64 = 5.0;

type ProjectContext = (Vec<String>, Vec<(PaneId, String)>);

#[derive(Default)]
struct State {
    host_id: HostId,
    project_watch_id: Option<DataWatchId>,
    latest_snapshot: Option<PollReply>,
    retry_snapshot: bool,
    arrangement_pending: bool,
    /// The tab names last handed to the host, so an unchanged map costs no host
    /// call. The daemon already drops a snapshot identical to the last one, but
    /// two different snapshots routinely name the tabs the same way — a pane
    /// going busy changes the snapshot and nothing about where the panes are.
    /// `None` forces the first publication, including an empty map after a reload.
    published_tab_names: Option<BTreeMap<TabId, String>>,
    published_project_context: Option<ProjectContext>,
}

register_plugin!(State);

impl YinzPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        yinz_tile::shim::init_agent_providers();
        self.host_id = get_plugin_ids().host_id;
        // Builtins already have data-store access; see agent-service.
        let backend = Host::shared();
        // Declared here as well as in agent, because either may start first and
        // a fetch against an entity nobody has declared is an error. The
        // definitions come from one place so the two cannot disagree.
        if let Err(error) = backend.define_all(&store::shared_defs()) {
            // Nothing else will work, and there is no UI to say so in.
            eprintln!("project-store: {error}");
            return;
        }
        subscribe(&[EventType::Timer]);
        set_timeout(yinz_tile::prelude::FractionalSeconds::new(RETRY_SECONDS));
        import_legacy(&backend, &get_plugin_ids().host_id);
        match backend.watch(
            &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
            WatchMode::Rows,
        ) {
            Ok(handle) => {
                self.project_watch_id = Some(handle.watch_id);
                self.publish_project_context(&projects::from_rows(&handle.rows), None);
            },
            Err(error) => eprintln!("project-store: could not watch projects: {error}"),
        }
        // Ask agent-service for pane snapshots, which is where discovered
        // projects come from. Broadcast, because addressing it by url would
        // launch a second daemon rather than find the running one — the host
        // injects `caller_cwd` into piped configuration and plugin identity is
        // (location, configuration). Deliberately no payload: the daemon reads
        // manual kind overrides out of one, and this plugin has none to give.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
    }

    fn update(&mut self, event: Event) -> bool {
        if let Event::AgentProvidersChanged(_) = event {
            self.providers_changed(&Host::shared());
        }
        if let Event::Timer(_) = event {
            self.retry_snapshot(&Host::shared());
            set_timeout(yinz_tile::prelude::FractionalSeconds::new(RETRY_SECONDS));
        }
        if let Event::PluginDataChanged { watch_id, rows, .. } = event {
            if self.project_watch_id == Some(watch_id) {
                let registry = projects::from_rows(&rows);
                let snapshot = self.latest_snapshot.take();
                if let Some((reply, panes)) = snapshot
                    .as_ref()
                    .and_then(|reply| reply.panes.as_deref().map(|panes| (reply, panes)))
                {
                    self.publish_tab_names(&registry, reply, panes);
                } else {
                    self.publish_project_context(&registry, snapshot.as_ref());
                }
                self.latest_snapshot = snapshot;
            }
        }
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        // CLI EOF has no payload; an explicitly empty request still needs a parse error.
        if matches!(pipe_message.source, PipeSource::Cli(_))
            && matches!(
                pipe_message.name.as_str(),
                wire::STORE_MUTATE | wire::STORE_QUERY
            )
            && pipe_message.payload.is_none()
        {
            return false;
        }
        let payload = pipe_message.payload.unwrap_or_default();
        if matches!(
            pipe_message.name.as_str(),
            wire::STORE_MUTATE | wire::STORE_QUERY
        ) {
            if let PipeSource::Plugin(id) = pipe_message.source {
                let allowed = get_session_list().is_ok_and(|snapshot| {
                    snapshot
                        .live_sessions
                        .iter()
                        .filter(|s| s.is_current_session)
                        .any(|s| {
                            s.plugins
                                .get(&id)
                                .is_some_and(|p| p.location == "yinz:agents")
                        })
                });
                if !allowed {
                    return false;
                }
            }
        }
        match pipe_message.name.as_str() {
            "arrange_panes_by_project" if !self.arrangement_pending => {
                self.arrangement_pending = true;
                pipe_message_to_plugin(MessageToPlugin::new("arrange_projects_refresh"));
            },
            "arrange_projects_snapshot" if std::mem::take(&mut self.arrangement_pending) => {
                match serde_json::from_str::<PollReply>(&payload) {
                    Ok(reply) => match Host::shared()
                        .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
                        .map(|rows| projects::from_rows(&rows))
                    {
                        Ok(registry) => {
                            let (groups, skipped) = arrangement::classify(&registry, &reply);
                            if skipped > 0 {
                                notify(&format!("Arrange panes: {skipped} panes have no available project identity"));
                            }
                            arrange_project_panes(groups);
                        },
                        Err(error) => notify(&format!("Arrange panes: {error}")),
                    },
                    Err(error) => notify(&format!("Arrange panes: {error}")),
                }
            },
            wire::AGENT_HELLO => {
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
            },
            wire::AGENT_SNAPSHOT => self.on_snapshot(&Host::shared(), &payload),
            wire::STORE_MUTATE => self.handle_mutate(pipe_message.source, &payload),
            wire::STORE_QUERY => self.handle_query(pipe_message.source, &payload),
            _ => {},
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    /// Everything one pane snapshot is worth: the projects it implies, and the
    /// names its tabs should wear.
    ///
    /// One parse and one registry read for both, because they ask the same two
    /// questions of the same folders and a second fetch could answer differently
    /// from the first.
    fn on_snapshot(&mut self, backend: &impl Backend, payload: &str) {
        let Ok(reply) = serde_json::from_str::<PollReply>(payload) else {
            return;
        };
        if reply.panes.is_none() {
            return;
        }
        self.latest_snapshot = Some(reply);
        self.retry_snapshot = true;
        self.retry_snapshot(backend);
    }

    fn providers_changed(&mut self, backend: &impl Backend) {
        self.retry_snapshot = true;
        self.retry_snapshot(backend);
    }

    fn retry_snapshot(&mut self, backend: &impl Backend) {
        if !self.retry_snapshot {
            return;
        }
        let Some(reply) = self.latest_snapshot.take() else {
            return;
        };
        if let Some(panes) = reply.panes.as_deref() {
            // A failed read must preserve displayed names and retry the latest snapshot.
            if let Some(registry) = discover_from(backend, &reply, panes) {
                self.publish_tab_names(&registry, &reply, panes);
                self.retry_snapshot = false;
            }
        }
        self.latest_snapshot = Some(reply);
    }

    /// Name each tab after the project its panes are in, and tell the host.
    ///
    /// The one publisher of tab projects in the session. It lives here, and not
    /// in agent where the rest of the project machinery is, because a tab has to
    /// be named whether or not a agent pane is open — and in most sessions one
    /// never is. Project assignments remain separate from these display labels.
    ///
    /// The host does the rest of the naming: it knows the tab order the `#1`/`#2`
    /// of two tabs on one project are read in, it knows which tabs the user has
    /// named and must be left alone, and it knows when a lone pane can supply a
    /// fallback title. What it is
    /// missing is what a project is, which is the one thing this plugin can say.
    fn publish_tab_names(
        &mut self,
        registry: &projects::Registry,
        reply: &PollReply,
        panes: &[agents::AgentPane],
    ) {
        self.publish_project_context(registry, Some(reply));
        let owners: BTreeMap<_, _> = panes
            .iter()
            .chain(&reply.suppressed_panes)
            .filter_map(|pane| {
                let location = pane.checkout_location()?;
                let label = projects::label_for_cwd(&reply.repositories, registry, &location)?;
                // A parent probe can identify the project, but a nested worktree
                // may have a different branch. Only use this location's probe.
                let branch = reply
                    .repositories
                    .get(&location)
                    .and_then(|repository| repository.branch.clone());
                Some((pane.pane_id, (label, branch)))
            })
            .collect();
        // Tab *ids*, never the tab name `AgentPane.tab` also carries — that name
        // is the one we published last time, fed back through the manifest, and
        // reading it here would close the loop. As it is the loop opens and shuts
        // once: naming a tab changes the next snapshot, which derives the same map
        // from the same ids, which publishes nothing.
        let names: BTreeMap<TabId, String> =
            agents::tab_project_labels(panes.iter().chain(&reply.suppressed_panes), &owners)
                .into_iter()
                .filter_map(|(tab_id, found)| Some((tab_id, agents::tab_project_label(&found)?)))
                .collect();
        // No pruning of tabs that closed: the map is rebuilt from the panes that
        // are open and replaces the host's wholesale, so a tab whose panes are
        // gone is simply absent and goes back to its own name.
        if self.published_tab_names.as_ref() == Some(&names) {
            return;
        }
        Self::set_tab_names(&names);
        self.published_tab_names = Some(names);
    }

    fn publish_project_context(
        &mut self,
        registry: &projects::Registry,
        reply: Option<&PollReply>,
    ) {
        let mut assignments = Vec::new();
        if let Some(reply) = reply {
            for pane in reply.panes.iter().flatten().chain(&reply.suppressed_panes) {
                if let Some(project) = pane.checkout_location().and_then(|location| {
                    projects::registered_for_cwd(&reply.repositories, registry, &location)
                }) {
                    assignments.push((
                        PaneId::Terminal(TerminalId::new(pane.pane_id)),
                        project.name.clone(),
                    ));
                }
            }
        }
        assignments.sort();
        let context = (registry.keys().cloned().collect(), assignments);
        if self.published_project_context.as_ref() != Some(&context) {
            #[cfg(target_family = "wasm")]
            set_project_context(context.0.clone(), context.1.clone());
            self.published_project_context = Some(context);
        }
    }

    #[cfg(target_family = "wasm")]
    fn set_tab_names(names: &BTreeMap<TabId, String>) {
        set_tab_projects(
            names
                .iter()
                .map(|(tab_id, project)| (*tab_id, project.clone()))
                .collect(),
        );
    }

    /// Outside a plugin there is no host to tell, and `published_tab_names` is
    /// what the tests read instead.
    #[cfg(not(target_family = "wasm"))]
    fn set_tab_names(_names: &BTreeMap<TabId, String>) {}

    fn handle_mutate(&mut self, source: PipeSource, payload: &str) {
        let outcome = match serde_json::from_str::<Mutation>(payload) {
            Ok(mutation) => (
                mutation.structured_reply,
                ops::run_mutate(&Host::shared(), &mutation.op, &mutation.args),
            ),
            Err(_) => (false, Err("could not parse mutation".to_owned())),
        };
        let request_id = serde_json::from_str::<Mutation>(payload)
            .map(|r| r.request_id)
            .unwrap_or_default();
        answer(source, &request_id, outcome.0, outcome.1);
    }

    fn handle_query(&mut self, source: PipeSource, payload: &str) {
        // Malformed requests retain the raw error reply used by existing pipe clients.
        let (structured_reply, outcome) = match serde_json::from_str::<Query>(payload) {
            Ok(query) if !query.args.is_null() && !query.args.is_object() => (
                query.structured_reply,
                Err("query args must be an object".to_owned()),
            ),
            Ok(mut query) => {
                query.args["local_host_id"] = serde_json::json!(self.host_id);
                (
                    query.structured_reply,
                    ops::run_query(&Host::shared(), &query.op, &query.args).and_then(|reply| {
                        if query.op != "project_list" { return Ok(reply); }
                        let mut value: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
                        value["live"] = self.latest_snapshot.as_ref().map(|snapshot| serde_json::json!({"session":snapshot.yinz_session_name,"tab_projects":snapshot.tab_projects,"panes":snapshot.panes,"suppressed_panes":snapshot.suppressed_panes,"repositories":snapshot.repositories})).unwrap_or(serde_json::Value::Null);
                        Ok(value.to_string())
                    }),
                )
            },
            Err(_) => (false, Err("could not parse query".to_owned())),
        };
        let request_id = serde_json::from_str::<Query>(payload)
            .map(|r| r.request_id)
            .unwrap_or_default();
        answer(source, &request_id, structured_reply, outcome);
    }
}

/// Add any project the agent snapshot implies, from the folders its panes are
/// sitting in, and hand back the registry as it stands afterwards.
///
/// Driven by the daemon's snapshot rather than by a poll of our own: this plugin
/// has no reason to look at panes otherwise, and running discovery off the same
/// snapshot agent sees keeps the two from disagreeing about which projects
/// exist.
///
/// The registry comes back because the caller names the tabs from it, and a
/// second fetch there could answer differently from this one — including
/// missing the project this pass just made.
///
/// One apply for the projects and the moved lists together. That is new: they
/// used to be two independent file writes, so a crash between them left a
/// project pointing at a root whose todos were still under the old one.
fn discover_from(
    backend: &impl Backend,
    reply: &PollReply,
    panes: &[agents::AgentPane],
) -> Option<projects::Registry> {
    // Only an agent counts. A shell sitting at a prompt is not a reason to
    // remember a folder for good, and every pane would otherwise become a
    // project the first time someone cd'd somewhere.
    //
    // Deliberately narrower than the naming above it, which counts every pane.
    // The two widths are the difference between what is written down and what is
    // merely displayed: a name is worked out again from the panes that are open
    // and costs a redraw when it is wrong, where a remembered folder outlives
    // the session and costs the user a row to delete.
    let repositories: Vec<projects::Repository> = panes
        .iter()
        .filter(|pane| agents::classify(pane, &reply.manual_kinds).is_some())
        .filter_map(|pane| {
            let location = pane.checkout_location()?;
            reply.repositories.get(&location).cloned()
        })
        // A checkout on another machine makes no project: every path a project
        // row holds is a path on *this* one. `projects::discover` refuses these
        // too, and is the guarantee; this is the same rule said where the panes
        // are, so a reader here is not left wondering.
        //
        // A Firecracker pane passes it. Its location is the host worktree its VM
        // mirrors — a folder on this machine, probed here — so the checkout it
        // names is one a project row can hold, exactly as it would be for a pane
        // sitting in that worktree without a VM.
        .filter(|repository| repository.host.is_none())
        .collect();

    // `None`, not an empty registry: the projects are still there, they could not be read.
    let rows = backend
        .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
        .ok()?;
    let before = projects::from_rows(&rows);
    let mut after = before.clone();
    let discovery = projects::discover(&mut after, &repositories);
    if !discovery.changed() {
        return Some(after);
    }
    let mut mutations = projects::diff_mutations(&before, &after);
    // A folder's todo list and its notes move with it, in the same batch as
    // the project row, so no reader sees the project at one root and its
    // lists at another.
    for (old, new) in discovery.root_moves {
        if let Ok(moved) = todos::plan_move_root(&old, &new) {
            mutations.extend(moved);
        }
        if let Ok(moved) = notes::plan_move_root(&old, &new) {
            mutations.extend(moved);
        }
    }
    let _ = backend.apply(mutations);
    Some(after)
}

/// Read the two JSON files in, once, if this is the first launch that has the
/// store.
///
/// Guarded by marker rows written in the same transaction as each imported
/// batch, so a failed import resumes at its first uncommitted level. A marker rather
/// than "is the table empty?" — emptiness is a state a user can reach honestly,
/// by crossing off and clearing the last todo, and that must not resurrect the
/// file on the next launch.
fn import_legacy(backend: &impl Backend, host_id: &HostId) {
    if !imported(backend, host_id, "projects") {
        let registry = projects::parse_legacy(&legacy_bytes(projects::FILE));
        let mut mutations: Vec<yinz_tile::prelude::Mutation> =
            registry.values().map(projects::to_mutation).collect();
        mutations.push(marker(host_id, "projects"));
        let _ = backend.apply(mutations);
    }
    if let Err(error) = import_todos(
        backend,
        host_id,
        &todos::parse_legacy(&legacy_bytes(todos::FILE)),
    ) {
        eprintln!("project-store: legacy todo import incomplete: {error}");
    }
}

fn import_todos(
    backend: &impl Backend,
    host_id: &HostId,
    lists: &todos::Store,
) -> Result<(), String> {
    if !imported(backend, host_id, "todos") {
        // One apply per level per list, because a child's parent_id is the id
        // the store gave its parent, which is only known once that row exists.
        for (root, list) in lists {
            for (depth, level) in todos::levels_for_import(list).into_iter().enumerate() {
                let checkpoint = format!("todos:{}", serde_json::json!([root, depth]));
                // Unlike old completion markers, these checkpoints always name a host.
                // A failed read must stop the import, not count as a completed level.
                if !backend
                    .fetch(&FetchRequest::entity("import_marker").filter(
                        Pred::eq("host_id", host_id).and(Pred::eq("what", checkpoint.clone())),
                    ))?
                    .is_empty()
                {
                    continue;
                }

                // Keep UIDs to recover parents after a restart. Use this same snapshot
                // for validation and the transaction guard.
                let rows = store::fetch_all(
                    backend,
                    FetchRequest::entity("todo").filter(Pred::eq("root", root.as_str())),
                )?;
                let ids_by_uid: BTreeMap<_, _> = rows
                    .rows
                    .iter()
                    .filter_map(|row| match (rows.get(row, "uid"), rows.get(row, "id")) {
                        (Some(DataValue::Text(uid)), Some(DataValue::Int(id))) => {
                            Some((uid.as_str(), DataRowId::new(*id)))
                        },
                        _ => None,
                    })
                    .collect();
                let mut new_ids = BTreeMap::new();
                for old_parent in level.iter().filter_map(|pending| pending.old_parent) {
                    let uid = todos::import_uid(host_id, root, old_parent);
                    let id = ids_by_uid.get(uid.as_str()).ok_or(
                        "the imported todo parent is missing or moved; import remains incomplete",
                    )?;
                    new_ids.insert(old_parent, *id);
                }
                let current = todos::from_rows(&rows).remove(root).unwrap_or_default();
                let mut mutations =
                    todos::import_mutations(&current, root, &level, &new_ids, host_id)?;
                let mut checkpoint = marker(host_id, &checkpoint);
                if let yinz_tile::prelude::Mutation::Put { expect_rev, .. } = &mut checkpoint {
                    *expect_rev = Some(REV_ABSENT);
                }
                mutations.push(checkpoint);
                backend.apply(mutations)?;
            }
        }
        backend.apply(vec![marker(host_id, "todos")])?;
    }
    Ok(())
}

/// The host is carried in from `load` rather than asked for here. `get_plugin_ids` is a host
/// shim, and this file's tests run natively, where the shims read real stdin — see `store`'s
/// module doc.
fn marker(host_id: &HostId, what: &str) -> yinz_tile::prelude::Mutation {
    store::import_marker_mutation(host_id, what)
}

/// Shared with agent rather than written twice: the two have to agree about the marker's uid,
/// and about what a marker written before that uid carried a host means — see
/// `store::import_recorded`.
fn imported(backend: &impl Backend, host_id: &HostId, what: &str) -> bool {
    store::import_recorded(backend, host_id, what)
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
/// A plugin gets nothing. Nothing pipes a change here from a plugin now that
/// agent writes the shared namespace itself, and a reply nobody waits for is a
/// message name kept working for no reader.
fn reply_message(structured: bool, outcome: ops::OpResult) -> String {
    if !structured {
        return outcome.unwrap_or_else(|message| message);
    }
    match outcome {
        Ok(message) => serde_json::json!({"ok": true, "data":
            serde_json::from_str::<serde_json::Value>(&message)
                .unwrap_or(serde_json::Value::String(message))})
        .to_string(),
        Err(error) => serde_json::json!({"ok": false, "error": error}).to_string(),
    }
}

fn answer(source: PipeSource, request_id: &str, structured: bool, outcome: ops::OpResult) {
    let message = reply_message(structured, outcome);
    match source {
        PipeSource::Cli(pipe_id) => {
            cli_pipe_output(&pipe_id, &format!("{message}\n"));
            unblock_cli_pipe_input(&pipe_id);
        },
        PipeSource::Plugin(id) if !request_id.is_empty() => {
            let result: serde_json::Value =
                serde_json::from_str(&message).unwrap_or_else(|_| serde_json::json!(message));
            pipe_message_to_plugin(
                MessageToPlugin::new("yinz_rpc_reply")
                    .with_destination_plugin_id(id)
                    .with_payload(
                        serde_json::json!({"version":1,"call_id":request_id,"result":result})
                            .to_string(),
                    ),
            );
        },
        PipeSource::Plugin(_) | PipeSource::Keybind => {},
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::store::Local;

    #[test]
    fn structured_store_replies_are_opt_in_and_distinguish_failure() {
        use serde_json::{json, Value};
        assert_eq!(
            reply_message(false, Ok("remembered p".into())),
            "remembered p"
        );
        assert_eq!(
            reply_message(false, Err("unknown project".into())),
            "unknown project"
        );
        for (outcome, expected) in [
            (
                Ok("remembered p".into()),
                json!({"ok":true,"data":"remembered p"}),
            ),
            (Ok(r#"{"id":7}"#.into()), json!({"ok":true,"data":{"id":7}})),
            (
                Err("unknown project".into()),
                json!({"ok":false,"error":"unknown project"}),
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<Value>(&reply_message(true, outcome)).unwrap(),
                expected
            );
        }
        let raw: Query = serde_json::from_str(r#"{"op":"project_list"}"#).unwrap();
        assert!(!raw.structured_reply);
    }

    #[test]
    fn arrangement_waits_past_three_timers_for_a_snapshot() {
        let mut state = State::default();
        let request = PipeMessage::new(
            PipeSource::Keybind,
            "arrange_panes_by_project",
            &None,
            &None,
            false,
        );
        state.pipe(request.clone());
        assert!(state.arrangement_pending);
        for _ in 0..5 {
            state.update(Event::Timer(FractionalSeconds::new(RETRY_SECONDS)));
            assert!(state.arrangement_pending);
        }
        state.pipe(request.clone());
        assert!(state.arrangement_pending);

        // A malformed snapshot still consumes the request through the existing
        // error path, without a native test reading the host's store from stdin.
        state.pipe(PipeMessage::new(
            PipeSource::Plugin(PluginId::new(10)),
            "arrange_projects_snapshot",
            &Some("invalid snapshot".into()),
            &None,
            false,
        ));
        assert!(!state.arrangement_pending);
        state.pipe(request);
        assert!(state.arrangement_pending);
    }

    #[test]
    fn project_context_publishes_registry_without_panes_and_registered_membership() {
        let project = projects::validate("registered", "/work/root").unwrap();
        let mut registry = projects::Registry::from([(project.name.clone(), project)]);
        let mut state = State::default();
        state.publish_project_context(&registry, None);
        assert_eq!(
            state.published_project_context,
            Some((vec!["registered".into()], vec![]))
        );
        let reply: PollReply = serde_json::from_str(&snapshot_of_panes(
            vec![
                pane_in(1, "shell", None, "/work/root"),
                pane_in(2, "shell", None, "/unregistered"),
                agents::AgentPane {
                    host: Some("remote".into()),
                    ..pane_in(3, "shell", None, "/work/root")
                },
            ],
            BTreeMap::new(),
        ))
        .unwrap();
        state.publish_project_context(&registry, Some(&reply));
        let expected = Some((
            vec!["registered".into()],
            vec![(PaneId::Terminal(TerminalId::new(1)), "registered".into())],
        ));
        assert_eq!(state.published_project_context, expected);
        state.publish_project_context(&registry, Some(&reply));
        assert_eq!(state.published_project_context, expected);
        registry.clear();
        state.publish_project_context(&registry, Some(&reply));
        assert_eq!(state.published_project_context, Some((vec![], vec![])));
    }

    /// A store with the shared entities declared, as a session has them.
    ///
    /// The real engine over a temporary file, so discovery below is a genuine
    /// round trip rather than a check against a stub.
    fn backend() -> Local {
        let backend = Local::shared("project_store_discovery");
        backend
            .define_all(&store::shared_defs())
            .expect("declared the shared entities");
        backend
    }

    fn project_names(backend: &Local) -> Vec<String> {
        let rows = backend
            .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
            .expect("fetched the projects");
        let mut names: Vec<String> = projects::from_rows(&rows).into_keys().collect();
        names.sort();
        names
    }

    /// Drive one snapshot through the real entry point, and hand back what the
    /// plugin made of it — the tab names included, which is the half no store
    /// row records.
    fn apply(backend: &Local, payload: &str) -> State {
        let mut state = State::default();
        state.on_snapshot(backend, payload);
        state
    }

    fn checkout(root: &str) -> projects::Repository {
        projects::Repository {
            root: root.to_owned(),
            remote: None,
            worktree_status: Some(projects::WorktreeStatus::Primary),
            common_dir: Some(format!("{root}/.git")),
            branch: Some("main".to_owned()),
            host: None,
        }
    }

    fn pane_in(pane_id: u32, title: &str, command: Option<&str>, cwd: &str) -> agents::AgentPane {
        agents::AgentPane {
            pane_id,
            title: title.to_owned(),
            command: command.map(str::to_owned),
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

    /// A pane sitting in a tab, which is what it takes to name one.
    fn pane_on_tab(pane_id: u32, command: &str, cwd: &str, tab_id: TabId) -> agents::AgentPane {
        agents::AgentPane {
            tab: Some((tab_id, format!("Tab #{}", tab_id))),
            ..pane_in(pane_id, command, Some(command), cwd)
        }
    }

    /// A snapshot of several panes, each in the checkout its folder names, plus
    /// whatever the user assigned by hand.
    fn snapshot_of_panes(
        panes: Vec<agents::AgentPane>,
        assigned: BTreeMap<TabId, String>,
    ) -> String {
        let repositories = panes
            .iter()
            .filter_map(|pane| {
                // Keyed the way every reader looks a folder up, so a Firecracker
                // pane's checkout is filed under the host worktree it mirrors and
                // not under its guest folder.
                let location = pane.checkout_location()?;
                let repository = checkout(&location.path).on_host(location.host.clone());
                Some((location, repository))
            })
            .collect();
        serde_json::to_string(&PollReply {
            suppressed_panes: Vec::new(),
            panes: Some(panes),
            activity: agents::Activity::new(),
            manual_kinds: BTreeMap::new(),
            yinz_session_name: String::new().into(),
            repositories,
            tab_projects: assigned,
            focused: None,
            hooks: agents::HookFeeds::new(),
            summaries: agents::Summaries::new(),
            runs: Vec::new(),
        })
        .expect("serialize")
    }

    /// The whole point of the change: a session with no agent in it still gets
    /// its tabs named, because a shell says where it is as well as an agent does.
    #[test]
    fn a_shell_in_a_checkout_names_its_tab() {
        let backend = backend();

        let state = apply(
            &backend,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/agent", TabId::new(1))],
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main"
        );
        assert!(
            project_names(&backend).is_empty(),
            "naming a tab is not a reason to remember the folder for good",
        );
    }

    /// A tab is a place to work rather than a thing one project owns, so a tab
    /// holding two checkouts is honestly described by both.
    #[test]
    fn a_tab_in_two_checkouts_is_named_after_both() {
        let backend = backend();

        let state = apply(
            &backend,
            &snapshot_of_panes(
                vec![
                    pane_on_tab(3, "zsh", "/work/pocus", TabId::new(1)),
                    pane_on_tab(4, "zsh", "/work/agent", TabId::new(1)),
                ],
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main + pocus:main",
            "sorted, so the name does not shuffle when the panes come back in another order",
        );
    }

    /// A store that answers nothing: every call fails, as a backend does across
    /// a transient error.
    struct Unreachable;

    impl Backend for Unreachable {
        fn define(&self, _def: &EntityDef) -> Result<DataSchemaInfo, String> {
            Err("unreachable".to_owned())
        }
        fn fetch(&self, _request: &FetchRequest) -> Result<DataRows, String> {
            Err("unreachable".to_owned())
        }
        fn count(&self, _request: &FetchRequest) -> Result<u64, String> {
            Err("unreachable".to_owned())
        }
        fn apply(
            &self,
            _mutations: Vec<yinz_tile::prelude::Mutation>,
        ) -> Result<ApplyReport, yinz_tile::prelude::WriteError> {
            Err(yinz_tile::prelude::WriteError::Failed(
                "unreachable".to_owned(),
            ))
        }
        fn watch(
            &self,
            _request: &FetchRequest,
            _mode: WatchMode,
        ) -> Result<DataWatchHandle, String> {
            Err("unreachable".to_owned())
        }
    }

    /// A snapshot that arrives while the store cannot be read publishes nothing:
    /// the projects are still there, and naming the tabs from an empty registry
    /// would flip every one of them to its folder's name until the next snapshot.
    #[test]
    fn a_store_that_cannot_be_read_renames_no_tab() {
        let backend = backend();
        backend
            .apply(vec![projects::to_mutation(
                &projects::validate("the-workspace", "/work/agent").expect("valid"),
            )])
            .expect("registered");
        let snapshot = snapshot_of_panes(
            vec![pane_on_tab(3, "zsh", "/work/agent", TabId::new(1))],
            BTreeMap::new(),
        );
        let mut state = State::default();
        state.on_snapshot(&backend, &snapshot);
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "the-workspace:main"
        );

        let context = state.published_project_context.clone();
        state.on_snapshot(&Unreachable, &snapshot);
        assert_eq!(state.published_project_context, context);

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "the-workspace:main",
            "a failed fetch is not an empty registry"
        );
    }

    #[test]
    fn unhooked_opencode_is_discovered_even_when_registration_arrives_late() {
        use yinz_tile::agent::provider::{self, AgentProvider};
        let backend = backend();
        let payload = snapshot(
            pane_in(3, "opencode", Some("/opt/bin/opencode"), "/work/agent"),
            BTreeMap::new(),
        );
        let mut state = apply(&backend, &payload);
        assert!(project_names(&backend).is_empty());
        assert!(!state.retry_snapshot);
        provider::register(
            901_614,
            AgentProvider {
                kind: "opencode".into(),
                executable: "opencode".into(),
                ..Default::default()
            },
        )
        .unwrap();
        state.providers_changed(&Unreachable);
        assert!(state.retry_snapshot);
        state.retry_snapshot(&backend);
        assert_eq!(project_names(&backend), vec!["agent"]);
        assert!(!state.retry_snapshot);
        // Registration before the first snapshot works too.
        apply(
            &backend,
            &snapshot(
                pane_in(4, "opencode", Some("opencode"), "/work/second"),
                BTreeMap::new(),
            ),
        );
        assert_eq!(project_names(&backend), vec!["agent", "second"]);
        assert!(provider::unregister_owner(901_614));
    }

    #[test]
    fn a_failed_initial_read_recovers_without_another_snapshot() {
        let backend = backend();
        let mut state = State::default();
        state.on_snapshot(
            &Unreachable,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/agent", TabId::new(1))],
                BTreeMap::new(),
            ),
        );
        assert!(state.latest_snapshot.is_some());
        assert!(state.published_tab_names.is_none());
        state.retry_snapshot(&Unreachable);
        assert!(state.retry_snapshot);
        state.retry_snapshot(&backend);
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main"
        );
        assert!(!state.retry_snapshot);
    }

    #[test]
    fn a_newer_snapshot_supersedes_a_failed_read() {
        let backend = backend();
        let mut state = State::default();
        for folder in ["/work/old", "/work/current"] {
            state.on_snapshot(
                &Unreachable,
                &snapshot_of_panes(
                    vec![pane_on_tab(3, "zsh", folder, TabId::new(1))],
                    BTreeMap::new(),
                ),
            );
        }
        state.on_snapshot(&Unreachable, "invalid");
        state.retry_snapshot(&backend);
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "current:main"
        );
        assert!(!state.retry_snapshot);
    }

    /// The registered name, not the folder's, when the folder is a project. The
    /// user renamed it for a reason.
    #[test]
    fn a_registered_project_names_the_tab_over_its_folder() {
        let backend = backend();
        backend
            .apply(vec![projects::to_mutation(
                &projects::validate("the-workspace", "/work/agent").expect("valid"),
            )])
            .expect("registered");

        let state = apply(
            &backend,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/agent", TabId::new(1))],
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "the-workspace:main"
        );
    }

    #[test]
    fn project_changes_refresh_names_without_another_snapshot() {
        let backend = backend();
        let watch = backend
            .watch(
                &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
                WatchMode::Rows,
            )
            .expect("watch registered");
        let mut state = State {
            project_watch_id: Some(watch.watch_id),
            ..State::default()
        };
        let changed = |rows, watch_id| Event::PluginDataChanged {
            watch_id,
            rows,
            truncated: false,
        };
        state.update(changed(watch.rows, watch.watch_id));
        assert!(state.latest_snapshot.is_none());
        assert!(state.published_tab_names.is_none());
        assert_eq!(state.published_project_context, Some((vec![], vec![])));

        backend
            .apply(vec![projects::to_mutation(
                &projects::validate("agent", "/work/agent").expect("valid"),
            )])
            .expect("initial project");
        state.on_snapshot(
            &backend,
            &snapshot_of_panes(
                vec![
                    pane_on_tab(3, "zsh", "/work/agent", TabId::new(1)),
                    pane_on_tab(4, "zsh", "/work/agent", TabId::new(2)),
                ],
                BTreeMap::from([(TabId::new(2), "manual".to_owned())]),
            ),
        );
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main"
        );
        backend
            .apply(vec![
                projects::delete_mutation("agent"),
                projects::to_mutation(
                    &projects::validate("renamed", "/work/agent").expect("valid"),
                ),
            ])
            .expect("renamed registry record");
        let rows = backend
            .fetch(&FetchRequest::entity("project"))
            .expect("rows");
        state.update(changed(
            rows.clone(),
            DataWatchId::new(watch.watch_id.get() + 1),
        ));
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main"
        );
        state.update(changed(rows.clone(), watch.watch_id));
        assert_eq!(
            state.published_project_context.as_ref().unwrap().0,
            vec!["renamed"]
        );
        assert!(state
            .published_project_context
            .as_ref()
            .unwrap()
            .1
            .iter()
            .all(|(_, name)| name == "renamed"));
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "renamed:main"
        );
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(2)],
            "renamed:main"
        );
        state.update(changed(rows.clone(), watch.watch_id));
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "renamed:main"
        );

        // Invalid payloads and partial replies must not replace the last full pane snapshot.
        state.on_snapshot(&backend, "invalid");
        let mut partial: PollReply = serde_json::from_str(
            &serde_json::to_string(state.latest_snapshot.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        partial.panes = None;
        state.on_snapshot(&backend, &serde_json::to_string(&partial).unwrap());
        assert!(state.latest_snapshot.as_ref().unwrap().panes.is_some());
        state.on_snapshot(&backend, &snapshot_of_panes(Vec::new(), BTreeMap::new()));
        state.update(changed(rows, watch.watch_id));
        assert!(state.published_tab_names.as_ref().unwrap().is_empty());
        assert!(state
            .latest_snapshot
            .as_ref()
            .unwrap()
            .panes
            .as_ref()
            .unwrap()
            .is_empty());
    }

    /// A folder Git does not claim names nothing. Without this line every `cd`
    /// would put a folder name on the tab bar.
    #[test]
    fn a_plain_folder_names_no_tab() {
        let backend = backend();
        let payload = snapshot_with(
            pane_on_tab(3, "zsh", "/home/me/Downloads", TabId::new(1)),
            projects::Repository::folder("/home/me/Downloads"),
            BTreeMap::new(),
        );

        let state = apply(&backend, &payload);

        assert!(state.published_tab_names.as_ref().unwrap().is_empty());
    }

    /// Manual assignments remain project metadata, independent of tab names.
    #[test]
    fn a_hand_made_assignment_does_not_override_the_folder() {
        let backend = backend();

        let state = apply(
            &backend,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/agent", TabId::new(1))],
                BTreeMap::from([(TabId::new(1), "notes".to_owned())]),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main"
        );
    }

    /// A tab whose panes have all gone simply has no entry in the next map, and
    /// the host replaces the whole map, so it goes back to its own name.
    #[test]
    fn a_tab_whose_panes_closed_is_named_no_longer() {
        let backend = backend();
        let mut state = State::default();

        state.on_snapshot(
            &backend,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/agent", TabId::new(1))],
                BTreeMap::new(),
            ),
        );
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "agent:main"
        );

        state.on_snapshot(&backend, &snapshot_of_panes(Vec::new(), BTreeMap::new()));

        assert!(state.published_tab_names.as_ref().unwrap().is_empty());
    }

    #[test]
    fn the_initial_empty_snapshot_clears_names_from_a_previous_instance() {
        let backend = backend();
        let mut state = State::default();
        assert!(state.published_tab_names.is_none());
        let payload = snapshot_of_panes(Vec::new(), BTreeMap::new());
        state.on_snapshot(&backend, &payload);
        assert_eq!(state.published_tab_names, Some(BTreeMap::new()));
        state.on_snapshot(&backend, &payload);
        assert_eq!(state.published_tab_names, Some(BTreeMap::new()));
    }

    #[test]
    fn names_follow_current_panes_including_hidden_and_exited_ones() {
        let backend = backend();
        backend
            .apply(vec![
                projects::to_mutation(&projects::validate("A", "/work/a").unwrap()),
                projects::to_mutation(&projects::validate("B", "/work/b").unwrap()),
            ])
            .unwrap();
        let mut state = State::default();
        let mut reply: PollReply = serde_json::from_str(&snapshot_of_panes(
            vec![
                pane_on_tab(3, "zsh", "/work/a", TabId::new(1)),
                pane_on_tab(4, "zsh", "/work/a", TabId::new(1)),
            ],
            BTreeMap::from([
                (TabId::new(1), "manual".into()),
                (TabId::new(99), "gone".into()),
            ]),
        ))
        .unwrap();
        // Explicit plain folders name the tabs without waiting for any Git result.
        reply.repositories.clear();
        let check = |state: &mut State, reply: &PollReply, expected: &[(usize, &str)]| {
            state.on_snapshot(&backend, &serde_json::to_string(reply).unwrap());
            assert_eq!(
                state.published_tab_names.as_ref().unwrap(),
                &expected
                    .iter()
                    .map(|(id, name)| (TabId::new(*id), (*name).to_owned()))
                    .collect()
            );
        };
        check(&mut state, &reply, &[(1, "A")]);
        let mut b = pane_on_tab(5, "zsh", "/work/b", TabId::new(1));
        b.status = agents::AgentStatus::Held;
        reply.panes.as_mut().unwrap().push(b);
        check(&mut state, &reply, &[(1, "A + B")]);
        reply.panes.as_mut().unwrap().remove(0);
        check(&mut state, &reply, &[(1, "A + B")]);
        reply.panes.as_mut().unwrap()[0].cwd = Some("/unknown".into());
        check(&mut state, &reply, &[(1, "B")]);

        let mut hidden = reply.panes.as_mut().unwrap().pop().unwrap();
        hidden.tab = Some((TabId::new(2), "another tab".into()));
        reply.suppressed_panes.push(hidden);
        check(&mut state, &reply, &[(2, "B")]);
        reply.suppressed_panes[0].agent =
            Some(agents::registered_agent("codex", Some("/work/a".into())));
        check(&mut state, &reply, &[(2, "A")]);
        reply.suppressed_panes[0].host = Some("remote".into());
        check(&mut state, &reply, &[]);
        reply.suppressed_panes[0].host = Some("firecracker:review".into());
        reply.suppressed_panes[0].firecracker_worktree = Some("/work/b".into());
        check(&mut state, &reply, &[(2, "B")]);
        reply
            .panes
            .as_mut()
            .unwrap()
            .push(reply.suppressed_panes.pop().unwrap());
        check(&mut state, &reply, &[(2, "B")]);
        reply.panes.as_mut().unwrap().pop();
        check(&mut state, &reply, &[]);
    }

    #[test]
    fn branches_follow_checkout_probes_without_changing_project_assignments() {
        let backend = backend();
        backend
            .apply(vec![projects::to_mutation(
                &projects::validate("registered", "/work/repo").unwrap(),
            )])
            .unwrap();
        let mut reply: PollReply = serde_json::from_str(&snapshot_of_panes(
            vec![pane_on_tab(3, "zsh", "/work/repo", TabId::new(1))],
            BTreeMap::new(),
        ))
        .unwrap();
        // Hidden panes use the same checkout resolution as visible ones, including VMs.
        let mut hidden = reply.panes.as_mut().unwrap().pop().unwrap();
        hidden.host = Some("firecracker:review".into());
        hidden.firecracker_worktree = Some("/work/repo".into());
        hidden.cwd = Some("/workspace".into());
        reply.suppressed_panes.push(hidden);
        let mut state = State::default();
        for (branch, expected) in [(Some("topic"), "registered:topic"), (None, "registered")] {
            reply.repositories.values_mut().next().unwrap().branch = branch.map(str::to_owned);
            state.on_snapshot(&backend, &serde_json::to_string(&reply).unwrap());
            assert_eq!(
                state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
                expected
            );
            assert_eq!(
                state.published_project_context.as_ref().unwrap().1[0].1,
                "registered"
            );
        }
        reply.repositories.clear();
        state.on_snapshot(&backend, &serde_json::to_string(&reply).unwrap());
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "registered"
        );
    }

    /// A snapshot with one pane in one checkout, as the daemon sends it.
    fn snapshot(pane: agents::AgentPane, manual_kinds: BTreeMap<u32, String>) -> String {
        // The folder the daemon would have probed, which for a Firecracker pane is the
        // host worktree its VM mirrors rather than the guest copy.
        let location = pane.checkout_location().expect("the pane is somewhere");
        let repository = checkout(&location.path);
        snapshot_with(pane, repository, manual_kinds)
    }

    /// A snapshot whose one checkout is described by the caller.
    fn snapshot_of(pane: agents::AgentPane, repository: projects::Repository) -> String {
        snapshot_with(pane, repository, BTreeMap::new())
    }

    fn snapshot_with(
        pane: agents::AgentPane,
        repository: projects::Repository,
        manual_kinds: BTreeMap<u32, String>,
    ) -> String {
        let location = pane.checkout_location().expect("the pane is somewhere");
        let repository = repository.on_host(location.host.clone());
        serde_json::to_string(&PollReply {
            suppressed_panes: Vec::new(),
            panes: Some(vec![pane]),
            activity: agents::Activity::new(),
            manual_kinds,
            // Nor which session the panes are in: a project is a folder on a machine, and
            // discovery is the same answer whoever asks.
            yinz_session_name: String::new().into(),
            repositories: BTreeMap::from([(location, repository)]),
            // Only agent makes these, and only when the user assigns a tab by
            // hand. Discovery does not read them at all.
            tab_projects: BTreeMap::new(),
            // Discovery does not read it; which pane has focus says nothing
            // about which folders are projects.
            focused: None,
            // Nor does it read what an agent says about itself: a project is made
            // of folders, and a folder is a folder whether the agent in it is
            // working or waiting.
            hooks: agents::HookFeeds::new(),
            summaries: agents::Summaries::new(),
            runs: Vec::new(),
        })
        .expect("serialize")
    }

    /// The rule discovery exists to enforce: a folder becomes a project because
    /// an *agent* is working in it, not because a shell happened to be there.
    #[test]
    fn only_an_agents_folder_becomes_a_project() {
        let backend = backend();

        apply(
            &backend,
            &snapshot(
                pane_in(3, "zsh", Some("zsh"), "/tmp/plain"),
                BTreeMap::new(),
            ),
        );
        assert!(project_names(&backend).is_empty());

        apply(
            &backend,
            &snapshot(
                pane_in(4, "codex", Some("codex"), "/tmp/coded"),
                BTreeMap::new(),
            ),
        );
        assert_eq!(project_names(&backend), vec!["coded"]);
    }

    /// A checkout on another machine makes no project. Every path a project row
    /// holds is a path on *this* machine, so a remote one would name a folder
    /// here that is missing — or, on a build box laid out like the laptop,
    /// somebody else's.
    ///
    /// Attributing a remote agent to a project that already exists is agent's
    /// job and works by remote url; *creating* one from a folder over there is
    /// deliberately not something discovery does.
    #[test]
    fn a_checkout_on_another_machine_makes_no_project() {
        let backend = backend();
        let mut pane = pane_in(3, "codex", Some("codex"), "/tmp/work");
        pane.host = Some("build-box".to_owned());

        apply(&backend, &snapshot(pane, BTreeMap::new()));

        assert!(
            project_names(&backend).is_empty(),
            "{:?}",
            project_names(&backend)
        );
    }

    /// The exception, and the pair the rule above reads with. A Firecracker VM mirrors a
    /// folder on *this* machine, and the agent in it is working on that folder — so the
    /// project it makes is the same one a local agent there would make, named after the
    /// host worktree and not after the guest copy.
    #[test]
    fn a_firecracker_agents_worktree_becomes_the_project_its_guest_copy_came_from() {
        let backend = backend();
        let mut pane = pane_in(3, "claude", Some("claude"), "/workspace");
        pane.host = Some("firecracker:review".to_owned());
        pane.firecracker_worktree = Some("/tmp/work".to_owned());

        apply(&backend, &snapshot(pane, BTreeMap::new()));

        assert_eq!(project_names(&backend), vec!["work"]);
    }

    /// And makes no second project when the worktree is already inside one — the
    /// ordinary case, since a VM is usually a copy of a checkout the user is in.
    #[test]
    fn a_firecracker_agent_inside_a_known_project_starts_no_new_one() {
        let backend = backend();
        apply(
            &backend,
            &snapshot(
                pane_in(3, "codex", Some("codex"), "/tmp/work"),
                BTreeMap::new(),
            ),
        );
        assert_eq!(project_names(&backend), vec!["work"]);

        let mut vm = pane_in(4, "claude", Some("claude"), "/workspace");
        vm.host = Some("firecracker:review".to_owned());
        vm.firecracker_worktree = Some("/tmp/work/sub".to_owned());
        apply(&backend, &snapshot(vm, BTreeMap::new()));

        assert_eq!(project_names(&backend), vec!["work"]);
    }

    /// A worktree nested inside the checkout it came from makes no second
    /// project: the containing root already owns the folder. Worth pinning here
    /// as well as in `projects`, because this end looks its folders up exactly —
    /// `reply.repositories.get(pane.cwd)` — while agent falls back to the
    /// nearest folder above. The two must still name the same project for a pane
    /// in a worktree, or the picker's header and the agent's project disagree.
    #[test]
    fn a_nested_worktree_makes_no_project_of_its_own() {
        let backend = backend();
        let worktree = "/tmp/nested/.claude/worktrees/topic";

        apply(
            &backend,
            &snapshot(
                pane_in(3, "codex", Some("codex"), "/tmp/nested"),
                BTreeMap::new(),
            ),
        );
        assert_eq!(project_names(&backend), vec!["nested"]);

        apply(
            &backend,
            &snapshot(
                pane_in(4, "codex", Some("codex"), worktree),
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            project_names(&backend),
            vec!["nested"],
            "the worktree joins the project above it rather than starting one",
        );
    }

    /// When the primary checkout of a project first seen through a linked
    /// worktree shows up, the project's root moves to it — and its todo list
    /// and notes move with it, or they would be stranded at a root no project
    /// names any more.
    #[test]
    fn a_root_move_carries_the_folders_todos_and_notes() {
        let backend = backend();
        let remote = Some("git@example.com:agent.git".to_owned());
        let worktree = "/tmp/worktrees/feature";
        let primary = "/tmp/work/agent";

        let mut linked = checkout(worktree);
        linked.remote.clone_from(&remote);
        linked.worktree_status = Some(projects::WorktreeStatus::Linked);
        apply(
            &backend,
            &snapshot_of(pane_in(3, "codex", Some("codex"), worktree), linked),
        );
        assert_eq!(project_names(&backend), vec!["feature"]);
        backend
            .apply(vec![
                yinz_tile::prelude::Mutation::put("todo")
                    .set("root", worktree)
                    .set("text", "ship it")
                    .set("done", false)
                    .build(),
                yinz_tile::prelude::Mutation::put("note")
                    .set("root", worktree)
                    .set("title", "why")
                    .set("body", "because")
                    .set("author", "user")
                    .set("archived", false)
                    .build(),
            ])
            .expect("seeded");

        let mut main = checkout(primary);
        main.remote.clone_from(&remote);
        apply(
            &backend,
            &snapshot_of(pane_in(4, "codex", Some("codex"), primary), main),
        );

        let todos = backend
            .fetch(&FetchRequest::entity("todo").filter(Pred::eq("root", primary)))
            .expect("fetched");
        assert_eq!(todos.len(), 1, "the todo moved to the primary root");
        let notes = backend
            .fetch(&FetchRequest::entity("note").filter(Pred::eq("root", primary)))
            .expect("fetched");
        assert_eq!(notes.len(), 1, "the note moved with it");
        let left = backend
            .fetch(&FetchRequest::entity("note").filter(Pred::eq("root", worktree)))
            .expect("fetched");
        assert!(left.is_empty());
    }

    /// A pane the user named by hand is an agent for this purpose too — the
    /// snapshot carries the overrides for exactly that reason.
    #[test]
    fn a_manual_kind_from_the_snapshot_counts_as_an_agent() {
        let backend = backend();

        apply(
            &backend,
            &snapshot(
                pane_in(3, "zsh", Some("zsh"), "/tmp/named"),
                BTreeMap::from([(3, "reviewer".to_owned())]),
            ),
        );

        assert_eq!(project_names(&backend), vec!["named"]);
    }

    /// The reason the latch is carried on the snapshot rather than derived at
    /// each end: an agent that shells out must not stop being one *here* either,
    /// or its project would come and go with every tool it ran.
    #[test]
    fn a_pane_that_shelled_out_still_names_its_project() {
        let backend = backend();
        let shelled_out = agents::AgentPane {
            detected: Some("codex".to_owned()),
            ..pane_in(3, "zsh", Some("git status"), "/tmp/latched")
        };

        apply(&backend, &snapshot(shelled_out, BTreeMap::new()));

        assert_eq!(project_names(&backend), vec!["latched"]);
    }
}
