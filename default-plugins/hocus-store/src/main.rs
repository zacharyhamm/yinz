//! hocus-store — the project registry, todo lists and notes, reachable from a
//! shell.
//!
//! Background-only plugin. Never rendered.
//!
//! These were hocus's own files. They live in the shared data store now, so
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
//! and a bare `yinz:hocus-store` url carries none of the alias' `shared true`
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
//! off the pane snapshots hocus-agentd sends — which stays here because it has
//! to happen whether or not a hocus pane is open.

mod arrangement;
mod notes_ops;
mod ops;

#[cfg(test)]
#[path = "unit/legacy_import.rs"]
mod legacy_import_tests;

use hocus_proto::store::{Backend, Host};
use hocus_proto::wire::{self, Mutation, PollReply, Query};
use hocus_proto::{agents, notes, projects, store, todos};
use std::collections::BTreeMap;
use yinz_tile::prelude::DataRowId;
use yinz_tile::prelude::DataWatchId;
use yinz_tile::prelude::HostId;
use yinz_tile::prelude::TabId;
use yinz_tile::prelude::*;

const RETRY_SECONDS: f64 = 5.0;

#[derive(Default)]
struct State {
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
}

register_plugin!(State);

impl YinzPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // Builtins already have data-store access; see hocus-agentd.
        let backend = Host::shared();
        // Declared here as well as in hocus, because either may start first and
        // a fetch against an entity nobody has declared is an error. The
        // definitions come from one place so the two cannot disagree.
        if let Err(error) = backend.define_all(&store::shared_defs()) {
            // Nothing else will work, and there is no UI to say so in.
            eprintln!("hocus-store: {error}");
            return;
        }
        subscribe(&[EventType::Timer]);
        set_timeout(yinz_tile::prelude::FractionalSeconds::new(RETRY_SECONDS));
        import_legacy(&backend, &get_plugin_ids().host_id);
        match backend.watch(
            &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
            WatchMode::Rows,
        ) {
            Ok(handle) => self.project_watch_id = Some(handle.watch_id),
            Err(error) => eprintln!("hocus-store: could not watch projects: {error}"),
        }
        // Ask hocus-agentd for pane snapshots, which is where discovered
        // projects come from. Broadcast, because addressing it by url would
        // launch a second daemon rather than find the running one — the host
        // injects `caller_cwd` into piped configuration and plugin identity is
        // (location, configuration). Deliberately no payload: the daemon reads
        // manual kind overrides out of one, and this plugin has none to give.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
    }

    fn update(&mut self, event: Event) -> bool {
        if let Event::Timer(_) = event {
            self.retry_snapshot(&Host::shared());
            set_timeout(yinz_tile::prelude::FractionalSeconds::new(RETRY_SECONDS));
        }
        if let Event::PluginDataChanged { watch_id, rows, .. } = event {
            if self.project_watch_id == Some(watch_id) {
                if let Some(reply) = self.latest_snapshot.take() {
                    if let Some(panes) = reply.panes.as_deref() {
                        self.publish_tab_names(&projects::from_rows(&rows), &reply, panes);
                    }
                    self.latest_snapshot = Some(reply);
                }
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
    /// in hocus where the rest of the project machinery is, because a tab has to
    /// be named whether or not a hocus pane is open — and in most sessions one
    /// never is. hocus keeps its own copy of this map to show the model, derived
    /// by the same two functions so the two cannot disagree.
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
        let owners: BTreeMap<u32, String> = panes
            .iter()
            .chain(&reply.suppressed_panes)
            .filter_map(|pane| {
                let location = pane.checkout_location()?;
                let label = projects::label_for_cwd(&reply.repositories, registry, &location)?;
                Some((pane.pane_id, label))
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
        answer(source, outcome.0, outcome.1);
    }

    fn handle_query(&mut self, source: PipeSource, payload: &str) {
        // Malformed requests retain the raw error reply used by existing pipe clients.
        let (structured_reply, outcome) = match serde_json::from_str::<Query>(payload) {
            Ok(query) => (
                query.structured_reply,
                ops::run_query(&Host::shared(), &query.op, &query.args),
            ),
            Err(_) => (false, Err("could not parse query".to_owned())),
        };
        answer(source, structured_reply, outcome);
    }
}

/// Add any project the agent snapshot implies, from the folders its panes are
/// sitting in, and hand back the registry as it stands afterwards.
///
/// Driven by the daemon's snapshot rather than by a poll of our own: this plugin
/// has no reason to look at panes otherwise, and running discovery off the same
/// snapshot hocus sees keeps the two from disagreeing about which projects
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
        eprintln!("hocus-store: legacy todo import incomplete: {error}");
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

/// Shared with hocus rather than written twice: the two have to agree about the marker's uid,
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
/// hocus writes the shared namespace itself, and a reply nobody waits for is a
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

fn answer(source: PipeSource, structured: bool, outcome: ops::OpResult) {
    let message = reply_message(structured, outcome);
    match source {
        PipeSource::Cli(pipe_id) => {
            cli_pipe_output(&pipe_id, &format!("{message}\n"));
            unblock_cli_pipe_input(&pipe_id);
        },
        PipeSource::Plugin(_) | PipeSource::Keybind => {},
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hocus_proto::store::Local;

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

    /// A store with the shared entities declared, as a session has them.
    ///
    /// The real engine over a temporary file, so discovery below is a genuine
    /// round trip rather than a check against a stub.
    fn backend() -> Local {
        let backend = Local::shared("hocus_store_discovery");
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
                vec![pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1))],
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "hocus"
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
                    pane_on_tab(4, "zsh", "/work/hocus", TabId::new(1)),
                ],
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "hocus + pocus",
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
                &projects::validate("the-workspace", "/work/hocus").expect("valid"),
            )])
            .expect("registered");
        let snapshot = snapshot_of_panes(
            vec![pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1))],
            BTreeMap::new(),
        );
        let mut state = State::default();
        state.on_snapshot(&backend, &snapshot);
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "the-workspace"
        );

        state.on_snapshot(&Unreachable, &snapshot);

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "the-workspace",
            "a failed fetch is not an empty registry"
        );
    }

    #[test]
    fn a_failed_initial_read_recovers_without_another_snapshot() {
        let backend = backend();
        let mut state = State::default();
        state.on_snapshot(
            &Unreachable,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1))],
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
            "hocus"
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
            "current"
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
                &projects::validate("the-workspace", "/work/hocus").expect("valid"),
            )])
            .expect("registered");

        let state = apply(
            &backend,
            &snapshot_of_panes(
                vec![pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1))],
                BTreeMap::new(),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "the-workspace"
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

        backend
            .apply(vec![projects::to_mutation(
                &projects::validate("hocus", "/work/hocus").expect("valid"),
            )])
            .expect("initial project");
        state.on_snapshot(
            &backend,
            &snapshot_of_panes(
                vec![
                    pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1)),
                    pane_on_tab(4, "zsh", "/work/hocus", TabId::new(2)),
                ],
                BTreeMap::from([(TabId::new(2), "manual".to_owned())]),
            ),
        );
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "hocus"
        );
        backend
            .apply(vec![
                projects::delete_mutation("hocus"),
                projects::to_mutation(
                    &projects::validate("renamed", "/work/hocus").expect("valid"),
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
            "hocus"
        );
        state.update(changed(rows.clone(), watch.watch_id));
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "renamed"
        );
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(2)],
            "renamed"
        );
        state.update(changed(rows.clone(), watch.watch_id));
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "renamed"
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
                vec![pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1))],
                BTreeMap::from([(TabId::new(1), "notes".to_owned())]),
            ),
        );

        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "hocus"
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
                vec![pane_on_tab(3, "zsh", "/work/hocus", TabId::new(1))],
                BTreeMap::new(),
            ),
        );
        assert_eq!(
            state.published_tab_names.as_ref().unwrap()[&TabId::new(1)],
            "hocus"
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
            // Only hocus makes these, and only when the user assigns a tab by
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
    /// Attributing a remote agent to a project that already exists is hocus's
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
    /// `reply.repositories.get(pane.cwd)` — while hocus falls back to the
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
        let remote = Some("git@example.com:hocus.git".to_owned());
        let worktree = "/tmp/worktrees/feature";
        let primary = "/tmp/work/hocus";

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
