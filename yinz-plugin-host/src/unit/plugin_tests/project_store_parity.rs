use super::{
    data_parity::DataHost,
    runtime_parity::{default_reply, Responder, SteelReference, WasmReference},
};
use agent_proto::{agents, projects, store};
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{atomic::AtomicU64, Arc},
};
use yinz_plugin_api::plugin_command::*;
use yinz_types::data::*;
use yinz_types::internal_ids::{HostId, PluginId};

const AGENT_SERVICE: u32 = 9;

fn session_list() -> Vec<u8> {
    let mut session = SessionInfo::new("main".into());
    session.is_current_session = true;
    for (id, location) in [(7, "yinz:agents"), (8, "file:/untrusted.scm")] {
        session.plugins.insert(
            PluginId::new(id),
            PluginInfo {
                location: location.into(),
                configuration: BTreeMap::new(),
            },
        );
    }
    ProtobufGetSessionListResponse::from(GetSessionListResponse::Ok(SessionListSnapshot {
        live_sessions: vec![session],
        resurrectable_sessions: vec![],
    }))
    .encode_to_vec()
}
fn host(data: &Arc<DataHost>, root: &Path) -> Responder {
    let root = root.to_owned();
    data.responder(Arc::new(move |command| match command {
        PluginCommand::GetAgentProviders => Some(
            ProtobufAgentProvidersResponse {
                json: "{\"Ok\":[]}".into(),
            }
            .encode_to_vec(),
        ),
        PluginCommand::GetSessionList => Some(session_list()),
        PluginCommand::FsRead {
            path,
            offset,
            max_bytes,
        } => Some(
            ProtobufFsReadResponse {
                result: Some(
                    match std::fs::read(root.join(path.strip_prefix("/cache").unwrap())) {
                        Ok(bytes) => fs_read_response::Result::Contents(
                            bytes
                                .into_iter()
                                .skip(offset.unwrap_or(0) as usize)
                                .take(max_bytes.unwrap_or(4194304))
                                .collect(),
                        ),
                        Err(error) => fs_read_response::Result::Error(error.to_string()),
                    },
                ),
            }
            .encode_to_vec(),
        ),
        _ => default_reply(command),
    }))
}
/// Scheme accepts snapshots only from the session's agent service, so its host runs one.
fn with_agent_service(responder: Responder) -> Responder {
    Arc::new(move |command| match command {
        PluginCommand::GetPluginIdsForUrl(url) if url == "yinz:agent-service" => Some(
            ProtobufGetPluginIdsForUrlResponse {
                plugin_ids: vec![AGENT_SERVICE],
                error: None,
            }
            .encode_to_vec(),
        ),
        _ => responder(command),
    })
}
fn message(
    source: PipeSource,
    name: &str,
    payload: Option<String>,
    seq: Option<u64>,
) -> PipeMessage {
    PipeMessage {
        source,
        name: name.into(),
        payload,
        args: seq
            .map(|seq| BTreeMap::from([("seq".into(), seq.to_string())]))
            .unwrap_or_default(),
        is_private: false,
        pane_id: None,
    }
}
struct Pair {
    wasm: WasmReference,
    steel: SteelReference,
    databases: [Arc<DataHost>; 2],
    seq: u64,
    _root: tempfile::TempDir,
}
impl Pair {
    fn new(legacy: Option<(&str, &str)>) -> Self {
        Self::with_host(legacy, host)
    }
    fn with_host(
        legacy: Option<(&str, &str)>,
        make_host: impl Fn(&Arc<DataHost>, &Path) -> Responder,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        if let Some((projects, todos)) = legacy {
            std::fs::write(root.path().join("projects.json"), projects).unwrap();
            std::fs::write(root.path().join("todos.json"), todos).unwrap();
        }
        let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
        let databases = [
            DataHost::with_clock("project-store", clock.clone()),
            DataHost::with_clock("project-store", clock.clone()),
        ];
        let mut pair = Self {
            wasm: WasmReference::with_clock(
                "project-store",
                BTreeMap::new(),
                root.path(),
                make_host(&databases[0], root.path()),
                Some(clock.clone()),
            ),
            steel: SteelReference::with_clock(
                "project-store",
                BTreeMap::new(),
                root.path(),
                with_agent_service(make_host(&databases[1], root.path())),
                Some(clock),
            ),
            databases,
            seq: 0,
            _root: root,
        };
        pair.commands();
        pair
    }
    fn commands(&mut self) -> Vec<Value> {
        self.commands_with(|_| false)
    }
    /// Compares commands after dropping those only Scheme sends.
    fn commands_with(&mut self, steel_only: fn(&Value) -> bool) -> Vec<Value> {
        // The original reads through WASI; Scheme uses the permission-checked FS command.
        let app = |commands: Vec<Value>| {
            commands
                .into_iter()
                .filter(|command| command.get("FsRead").is_none())
                .collect::<Vec<_>>()
        };
        let expected = app(self.wasm.take_commands());
        let normalize = |mut commands: Vec<Value>| {
            for command in &mut commands {
                // Original traces predate source descriptions; the publisher test checks those.
                if let Some(context) = command
                    .get_mut("SetProjectContext")
                    .and_then(Value::as_array_mut)
                {
                    context.truncate(2);
                }
                if let Some(output) = command.get_mut("CliPipeOutput") {
                    let text = output[1].as_str().unwrap();
                    assert!(text.ends_with('\n'), "CLI replies terminate with a newline");
                    // JSON object order is unspecified; raw text and array order remain exact.
                    if let Ok(value) = serde_json::from_str::<Value>(text) {
                        output[1] = value;
                    }
                }
                if let Some(message) = command.get_mut("MessageToPlugin")
                    && message["message_name"] == "yinz_rpc_reply"
                {
                    message["message_payload"] =
                        serde_json::from_str(message["message_payload"].as_str().unwrap()).unwrap();
                }
            }
            commands
        };
        let mut actual = app(self.steel.take_commands());
        actual.retain(|command| !steel_only(command));
        // Adapted traces include revision updates for tags; link-only assertions remain new.
        // Exclude the metadata lookup and link-only assertion, which attachment tests cover.
        actual.retain(|command| {
            let request = &command["DataFetch"]["request"];
            !(request["entity"] == "note"
                && request["select"] == json!(agent_proto::notes::LIST_COLUMNS)
                && request["limit"] == 1
                && request["filter"]["Eq"][0] == "id")
        });
        for command in &mut actual {
            if let Some(mutations) = command
                .pointer_mut("/DataApply/mutations")
                .and_then(Value::as_array_mut)
                && mutations.iter().any(|mutation| {
                    matches!(
                        mutation["Put"]["entity"].as_str(),
                        Some("note_tag" | "note_link")
                    )
                })
            {
                mutations.retain(|mutation| mutation["AssertRows"]["entity"] != "note");
            }
        }
        assert_eq!(
            normalize(actual),
            normalize(expected.clone()),
            "project-store runtime command mismatch"
        );
        expected
    }
    fn request(&mut self, name: &str, payload: Option<String>) -> Vec<Value> {
        self.send(PipeSource::Cli("request".into()), name, payload)
    }
    fn send(&mut self, source: PipeSource, name: &str, payload: Option<String>) -> Vec<Value> {
        let message = message(source, name, payload, None);
        assert_eq!(self.steel.pipe(&message), self.wasm.pipe(&message));
        self.commands()
    }
    /// Intended divergence: the recorded original trusted snapshots from any sender.
    /// Scheme receives them from the agent service with an increasing sequence number.
    fn snapshot(&mut self, name: &str, payload: Option<String>) -> Vec<Value> {
        self.seq += 1;
        let original = message(
            PipeSource::Cli("request".into()),
            name,
            payload.clone(),
            None,
        );
        let trusted = message(
            PipeSource::Plugin(PluginId::new(AGENT_SERVICE)),
            name,
            payload,
            Some(self.seq),
        );
        assert_eq!(self.steel.pipe(&trusted), self.wasm.pipe(&original));
        // Scheme resolves the service id before trusting it.
        self.commands_with(|command| command.get("GetPluginIdsForUrl").is_some())
    }
    fn op(&mut self, name: &str, op: &str, args: Value) -> Value {
        let commands = self.request(
            name,
            Some(json!({"op":op,"args":args,"structured_reply":true}).to_string()),
        );
        let output = commands
            .iter()
            .find_map(|command| command.get("CliPipeOutput"))
            .unwrap()[1]
            .as_str()
            .unwrap();
        serde_json::from_str(output).unwrap()
    }
    fn mutate(&mut self, op: &str, args: Value) -> Value {
        self.op("store_mutate", op, args)
    }
    fn query(&mut self, op: &str, args: Value) -> Value {
        self.op("store_query", op, args)
    }
    fn watches(&mut self) {
        let left = self.databases[0].changed();
        assert_eq!(self.databases[1].changed(), left);
        for event in left {
            assert_eq!(self.steel.update(&event), self.wasm.update(&event));
            self.commands();
        }
    }
    fn event(&mut self, event: Event) -> Vec<Value> {
        assert_eq!(self.steel.update(&event), self.wasm.update(&event));
        self.commands()
    }
    fn rows(&self) {
        for def in store::shared_defs() {
            let request = FetchRequest::entity(def.name)
                .in_namespace(store::SHARED)
                .limit(1000);
            assert_eq!(
                self.databases[1].fetch(&request),
                self.databases[0].fetch(&request)
            );
        }
    }
    fn reload(&mut self) {
        let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
        self.wasm = WasmReference::with_clock(
            "project-store",
            BTreeMap::new(),
            self._root.path(),
            host(&self.databases[0], self._root.path()),
            Some(clock.clone()),
        );
        self.steel = SteelReference::with_clock(
            "project-store",
            BTreeMap::new(),
            self._root.path(),
            with_agent_service(host(&self.databases[1], self._root.path())),
            Some(clock),
        );
        self.commands();
    }
}

#[test]
fn project_store_actual_runtimes_match_projects_and_guarded_todo_tree() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_projects_and_guarded_todo_tree",
    );
    let mut pair = Pair::new(None);
    assert!(pair.request("store_query", None).is_empty());
    pair.request("store_query", Some("".into()));
    pair.query("project_list", json!({}));
    for args in [
        json!({}),
        json!({"name":"work","root":"relative"}),
        json!({"name":"work","root":"/work"}),
    ] {
        pair.mutate("project_remember", args);
    }
    pair.watches();
    pair.mutate("todo_add", json!({"project":"work","text":"Parent 雪"}));
    pair.mutate(
        "todo_add",
        json!({"project":"work","text":"Child","parent_id":1}),
    );
    pair.query("todo_list", json!({"project":"work"}));
    pair.query("todo_search", json!({"query":"Child"}));
    pair.mutate(
        "todo_edit",
        json!({"project":"work","id":2,"text":"Changed child","expect_rev":999}),
    );
    pair.mutate(
        "todo_edit",
        json!({"project":"work","id":2,"text":"Changed child","expect_rev":1}),
    );
    pair.mutate("todo_move", json!({"project":"work","id":1,"parent_id":2}));
    pair.mutate("todo_done", json!({"project":"work","id":1}));
    pair.query("todo_list", json!({"project":"work","include_done":true}));
    pair.mutate("todo_done", json!({"project":"work","id":2,"undo":true}));
    pair.mutate(
        "todo_move",
        json!({"project":"work","id":2,"parent_id":null}),
    );
    pair.mutate("todo_delete", json!({"project":"work","id":1}));
    pair.mutate(
        "project_rename",
        json!({"name":"work","new_name":"renamed"}),
    );
    pair.mutate(
        "project_remember",
        json!({"name":"second","root":"/second"}),
    );
    pair.mutate(
        "project_merge",
        json!({"source":"second","target":"renamed"}),
    );
    pair.query("project_list", json!({}));
    pair.watches();
    pair.rows();
}

#[test]
fn project_store_actual_runtimes_match_note_tags_links_search_and_revisions() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_note_tags_links_search_and_revisions",
    );
    let mut pair = Pair::new(None);
    pair.mutate("project_remember", json!({"name":"work","root":"/work"}));
    pair.mutate("todo_add", json!({"project":"work","text":"First"}));
    pair.mutate("todo_add", json!({"project":"work","text":"Second"}));
    pair.mutate("note_add", json!({}));
    pair.mutate(
        "note_add",
        json!({"global":true,"title":"global","body":"Unicode 雪"}),
    );
    let created = pair.mutate(
        "note_add",
        json!({"project":"work","title":"Decision","body":"We chose Steel.","tags":["ENGINE"," design "],"todo_ids":[1,2]}),
    );
    pair.query("note_list", json!({}));
    pair.query("note_list", json!({"project":"work","tag":"engine"}));
    pair.query("note_get", json!({"id":2}));
    pair.query("note_search", json!({"query":"Steel"}));
    pair.query("todo_list", json!({"project":"work"}));
    pair.mutate(
        "note_edit",
        json!({"id":2,"body":"Updated body","expect_rev":999}),
    );
    pair.mutate(
        "note_edit",
        json!({"id":2,"body":"Updated body","tags":["new"],"expect_rev":created["data"]["rev"]}),
    );
    pair.mutate("note_link", json!({"note_id":1,"todo_id":1}));
    pair.mutate("note_link", json!({"note_id":2,"todo_id":2,"unlink":true}));
    pair.mutate("note_archive", json!({"id":2}));
    pair.query("note_list", json!({"include_archived":true}));
    pair.mutate("note_delete", json!({"id":1}));
    pair.rows();
}

#[test]
fn project_store_actual_runtimes_match_archive_empty_and_invalid_requests() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_archive_empty_and_invalid_requests",
    );
    let mut pair = Pair::new(None);
    for op in [
        "agent_runs",
        "agent_run",
        "agent_predecessors",
        "agent_history_search",
    ] {
        for args in [
            json!({}),
            json!({"run_id":"absent","query":"needle"}),
            json!({"query":"[","regex":true}),
            json!({"query":"x","regex":"bad"}),
        ] {
            pair.query(op, args);
        }
    }
    for payload in [
        "{}",
        "{\"op\":1}",
        "{\"op\":\"project_list\",\"args\":[]}",
        "{\"op\":\"project_list\",\"structured_reply\":null}",
    ] {
        pair.request("store_query", Some(payload.into()));
    }
    pair.rows();
}

#[test]
fn project_store_actual_runtimes_match_archive_pages_scopes_and_search() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_archive_pages_scopes_and_search",
    );
    let mut pair = Pair::new(None);
    let project = projects::validate("work", "/work").unwrap();
    for data in &pair.databases {
        data.apply(store::SHARED, &[projects::to_mutation(&project)]);
        for (id, host, session, cwd, previous) in [
            (
                "claude:first",
                "parity-host-id",
                "main",
                "/work/sub",
                Some("claude:second"),
            ),
            (
                "claude:second",
                "parity-host-id",
                "other",
                "/work",
                Some("claude:first"),
            ),
            (
                "claude:third",
                "remote-host",
                "main",
                "/work",
                Some("missing"),
            ),
            ("claude:legacy", "", "main", "/outside", None),
        ] {
            let mut run: agents::AgentRun = serde_json::from_value(json!({"id":id,"pane_id":7,"host_id":host,"yinz_session_name":session,
                "kind":"claude","title":id,"started_at_ms":1_699_999_800_000i64,"cwd":cwd,"previous_run_id":previous})).unwrap();
            run.entries = (1..=18)
                .map(|revision| agents::RunEntry {
                    tool: None,
                    revision,
                    event_id: Some(format!("event-{revision}")),
                    kind: agents::RunEntryKind::Result,
                    text: if revision == 4 {
                        "雪 long needle text ".repeat(250)
                    } else {
                        format!("result needle {revision}")
                    },
                    created_at_ms: Some(1_699_999_800_000 + revision as i64 * 1000),
                })
                .collect();
            run.revision = 18;
            data.apply(store::SHARED, &agents::rows::run_mutations(&run));
        }
    }
    for args in [
        json!({"limit":2}),
        json!({"limit":2,"offset":2}),
        json!({"project":"work"}),
        json!({"run_session":"main","host_id":"parity-host-id"}),
        json!({"host_id":"parity-host-id"}),
        json!({"kind":"claude","active":true}),
        json!({"since_ms":1_700_000_000_001i64}),
    ] {
        pair.query("agent_runs", args);
    }
    for args in [
        json!({"limit":3}),
        json!({"from_revision":1,"limit":3}),
        json!({"from_revision":4,"limit":1}),
        json!({"revision":5}),
        json!({"from_revision":100}),
    ] {
        let mut args = args;
        args["run_id"] = json!("claude:first");
        pair.query("agent_run", args);
    }
    for id in ["claude:first", "claude:third", "claude:legacy"] {
        pair.query("agent_predecessors", json!({"run_id":id}));
    }
    for args in [
        json!({"query":"needle","limit":3}),
        json!({"query":"needle","project":"work"}),
        json!({"query":"needle","run_id":"claude:first"}),
        json!({"query":"needle [123]$","regex":true,"limit":2}),
        json!({"query":"absent"}),
        json!({"query":"needle","run_session":"other"}),
    ] {
        pair.query("agent_history_search", args);
    }
    pair.rows();
}

#[test]
fn project_store_actual_runtimes_match_real_legacy_import_files() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_real_legacy_import_files",
    );
    let registry = BTreeMap::from([("work", projects::validate("work", "/work").unwrap())]);
    let projects = serde_json::to_string(&registry).unwrap();
    let todos = json!({"/work":{"items":[{"id":1,"text":"parent","children":[{"id":2,"text":"child","children":[{"id":3,"text":"雪 leaf"}]}]}]},
        "/other":{"items":[{"id":1,"text":"other root"}]}}).to_string();
    let mut pair = Pair::new(Some((&projects, &todos)));
    pair.query("project_list", json!({}));
    pair.query("todo_list", json!({"project":"work"}));
    pair.rows();
    let before = pair.databases[0].fetch(&FetchRequest::entity("todo").in_namespace(store::SHARED));
    std::fs::write(
        pair._root.path().join("todos.json"),
        json!({"/work":{"items":[{"id":90,"text":"must not be imported twice"}]}}).to_string(),
    )
    .unwrap();
    pair.reload();
    assert_eq!(
        pair.databases[0].fetch(&FetchRequest::entity("todo").in_namespace(store::SHARED)),
        before
    );
    for data in &pair.databases {
        data.apply(
            store::SHARED,
            &[
                Mutation::delete("import_marker", Pred::eq("host_id", "parity-host-id")),
                store::import_marker_mutation(&HostId::new(""), "projects"),
                store::import_marker_mutation(&HostId::new(""), "todos"),
            ],
        );
    }
    pair.reload();
    assert_eq!(
        pair.databases[0].fetch(&FetchRequest::entity("todo").in_namespace(store::SHARED)),
        before
    );
    assert_eq!(
        pair.databases[0]
            .fetch(
                &FetchRequest::entity("import_marker")
                    .in_namespace(store::SHARED)
                    .filter(Pred::eq("host_id", ""))
            )
            .len(),
        2,
        "claiming this host's import must preserve legacy markers for other hosts"
    );
    pair.rows();
}

#[test]
fn project_store_actual_runtimes_match_discovery_context_arrangement_and_rpc_authority() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_discovery_context_arrangement_and_rpc_authority",
    );
    let mut pair = Pair::new(None);
    let mut snapshot = json!({"panes":[
        {"pane_id":2,"title":"agent","command":"claude","cwd":"/work","status":"Running","tab":[1,"old"]},
        {"pane_id":1,"title":"shell","cwd":"/work/tree","status":"Running","tab":[1,"old"]},
        {"pane_id":3,"title":"unknown","cwd":"/unprobed","status":"Running","tab":[2,"other"]}],
        "activity":{},"manual_kinds":{"2":"claude"},"yinz_session_name":"main","repositories":{
            "/work":{"root":"/work","common_dir":"/work/.git","remote":"https://example.com/repo.git","branch":"main","worktree_status":"primary"},
            "/work/tree":{"root":"/work/tree","common_dir":"/work/.git","remote":"git@example.com:repo","branch":"feature","worktree_status":"linked"}}});
    pair.request("agent_hello", None);
    pair.snapshot("agent_snapshot", Some("bad".into()));
    pair.snapshot("agent_snapshot", Some(snapshot.to_string()));
    pair.watches();
    pair.query("project_list", json!({}));
    pair.snapshot("agent_snapshot", Some(snapshot.to_string()));
    pair.snapshot("arrange_projects_snapshot", Some(snapshot.to_string()));
    pair.request("arrange_panes_by_project", None);
    assert!(pair.request("arrange_panes_by_project", None).is_empty());
    pair.snapshot("arrange_projects_snapshot", Some(snapshot.to_string()));
    pair.request("arrange_panes_by_project", None);
    pair.snapshot("arrange_projects_snapshot", Some("bad".into()));
    snapshot["panes"] = json!([]);
    pair.snapshot("agent_snapshot", Some(snapshot.to_string()));
    let payload = json!({"op":"project_list","structured_reply":true,"request_id":"correlated-1"})
        .to_string();
    for id in [8, 7] {
        pair.send(
            PipeSource::Plugin(PluginId::new(id)),
            "store_query",
            Some(payload.clone()),
        );
    }
    pair.rows();
}

#[test]
fn project_store_actual_runtimes_match_partial_note_writes_and_missing_attachment_targets() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_match_partial_note_writes_and_missing_attachment_targets",
    );
    for disappears in [false, true] {
        let mut pair = Pair::with_host(None, |data, root| {
            let responder = host(data, root);
            let data = data.clone();
            Arc::new(move |command| {
                if let PluginCommand::DataApply { mutations, .. } = command {
                    if mutations.iter().any(|mutation| matches!(mutation,Mutation::Put {entity,..} if entity == "note_tag")) {
                        return Some(ProtobufDataApplyResponse::from(DataApplyResponse::Err("fixture attachment failure".into())).encode_to_vec());
                    }
                    if disappears && mutations.iter().any(|mutation| matches!(mutation,Mutation::Put {entity,..} if entity == "note")) {
                        let response=responder(command);
                        data.apply(store::SHARED,&[Mutation::delete("todo",Pred::eq("id",1i64))]);
                        return response;
                    }
                }
                responder(command)
            })
        });
        assert_eq!(
            pair.mutate("todo_add", json!({"project":"/work","text":"attachment"}))["ok"],
            true
        );
        let result=pair.mutate("note_add",json!({"global":true,"title":"Keep this note","body":"written before attachments","tags":["tag"],"todo_ids":[1]}));
        assert_eq!(result["ok"], true);
        assert!(result["data"]["warning"]
            .as_str()
            .unwrap()
            .contains(if disappears {
                "no todo item 1"
            } else {
                "fixture attachment failure"
            }));
        assert_eq!(result["data"]["tags"], json!([]));
        assert_eq!(result["data"]["todo_ids"], json!([]));
        pair.rows();
    }
}

#[test]
fn project_store_actual_runtimes_retry_failed_discovery_without_clearing_context() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "project_store_actual_runtimes_retry_failed_discovery_without_clearing_context",
    );
    use std::sync::atomic::{AtomicBool, Ordering};
    let fail = Arc::new(AtomicBool::new(false));
    let mut pair = Pair::with_host(None, |data, root| {
        let responder = host(data, root);
        let fail = fail.clone();
        Arc::new(move |command| {
            if matches!(command,PluginCommand::DataFetch {request} if request.entity == "project")
                && fail.load(Ordering::SeqCst)
            {
                Some(
                    ProtobufDataFetchResponse::from(DataFetchResponse::Err(
                        "fixture busy database".into(),
                    ))
                    .encode_to_vec(),
                )
            } else {
                responder(command)
            }
        })
    });
    pair.mutate("project_remember", json!({"name":"work","root":"/work"}));
    pair.watches();
    let snapshot = json!({"panes":[{"pane_id":1,"title":"shell","cwd":"/work","status":"Running","tab":[1,"old"]}],"activity":{}});
    fail.store(true, Ordering::SeqCst);
    let commands = pair.snapshot("agent_snapshot", Some(snapshot.to_string()));
    assert_eq!(
        commands.len(),
        1,
        "failed read must not replace published project names"
    );
    fail.store(false, Ordering::SeqCst);
    let commands = pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(5.0)));
    assert!(commands
        .iter()
        .any(|command| command.get("SetTabProjects").is_some()));
    let commands = pair.event(Event::Timer(yinz_types::units::FractionalSeconds::new(5.0)));
    assert_eq!(
        commands,
        vec![json!({"SetTimeout":5.0})],
        "successful retry stops rereading unchanged snapshots"
    );
    pair.rows();
}

#[test]
fn project_store_accepts_snapshots_only_from_the_agent_service_in_order() {
    let root = tempfile::tempdir().unwrap();
    let data = DataHost::new("project-store");
    let mut steel = SteelReference::new(
        "project-store",
        BTreeMap::new(),
        root.path(),
        with_agent_service(host(&data, root.path())),
    );
    let snapshot = |repository: &str| {
        json!({"panes":[{"pane_id":2,"title":"agent","command":"claude","cwd":repository,"status":"Running","tab":[1,"old"]}],
            "activity":{},"manual_kinds":{"2":"claude"},"yinz_session_name":"main","repositories":{repository:{"root":repository,
                "common_dir":format!("{repository}/.git"),"remote":format!("https://example.com{repository}.git"),
                "branch":"main","worktree_status":"primary"}}})
        .to_string()
    };
    // Effects a forged snapshot could cause: shared data writes, tab labels, context, and pane moves.
    let mut send = |source: PipeSource, name: &str, payload: Option<String>, seq: Option<u64>| {
        steel.pipe(&message(source, name, payload, seq));
        steel
            .take_commands()
            .into_iter()
            .filter(|command| {
                [
                    "DataApply",
                    "SetTabProjects",
                    "SetProjectContext",
                    "ArrangeProjectPanes",
                ]
                .iter()
                .any(|name| command.get(name).is_some())
            })
            .collect::<Vec<_>>()
    };
    let service = || PipeSource::Plugin(PluginId::new(AGENT_SERVICE));
    let foreign = || PipeSource::Plugin(PluginId::new(8));
    let cli = || PipeSource::Cli("forged".into());
    let projects = || {
        data.fetch(&FetchRequest::entity("project").in_namespace(store::SHARED))
            .len()
    };
    send(cli(), "agent_hello", None, None);
    // Plugin 7 is the trusted store client, which is still not the agent service.
    for (source, seq) in [
        (foreign(), Some(1)),
        (PipeSource::Plugin(PluginId::new(7)), Some(1)),
        (cli(), Some(1)),
        (service(), None),
    ] {
        let effects = send(
            source.clone(),
            "agent_snapshot",
            Some(snapshot("/forged")),
            seq,
        );
        assert!(effects.is_empty(), "{source:?}: {effects:?}");
    }
    assert_eq!(projects(), 0, "rejected snapshots must not write projects");
    assert!(!send(
        service(),
        "agent_snapshot",
        Some(snapshot("/work")),
        Some(2)
    )
    .is_empty());
    assert_eq!(projects(), 1);
    for seq in [2, 1] {
        let effects = send(
            service(),
            "agent_snapshot",
            Some(snapshot("/stale")),
            Some(seq),
        );
        assert!(effects.is_empty(), "seq {seq}: {effects:?}");
    }
    send(foreign(), "agent_hello", None, None);
    let effects = send(
        service(),
        "agent_snapshot",
        Some(snapshot("/stale")),
        Some(1),
    );
    assert!(
        effects.is_empty(),
        "a foreign hello must keep the sequence: {effects:?}"
    );
    assert_eq!(projects(), 1);
    send(service(), "agent_hello", None, None);
    let effects = send(
        service(),
        "agent_snapshot",
        Some(snapshot("/restarted")),
        Some(1),
    );
    assert!(
        !effects.is_empty(),
        "the service's hello restarts its sequence"
    );
    assert_eq!(projects(), 2);

    send(cli(), "arrange_panes_by_project", None, None);
    for source in [foreign(), cli()] {
        let effects = send(
            source,
            "arrange_projects_snapshot",
            Some(snapshot("/work")),
            None,
        );
        assert!(effects.is_empty(), "{effects:?}");
    }
    let arranged = send(
        service(),
        "arrange_projects_snapshot",
        Some(snapshot("/work")),
        None,
    );
    assert!(
        arranged
            .iter()
            .any(|command| command.get("ArrangeProjectPanes").is_some()),
        "rejected snapshots must not cancel the pending arrangement: {arranged:?}"
    );
}

#[test]
fn project_store_publishes_saved_sources_and_source_edits() {
    let root = tempfile::tempdir().unwrap();
    let data = DataHost::new("project-store");
    let mut steel = SteelReference::new(
        "project-store",
        BTreeMap::new(),
        root.path(),
        with_agent_service(host(&data, root.path())),
    );
    steel.take_commands();
    for remote in [
        json!(" git@github.com:owner/雪.git "),
        Value::Null,
        json!(""),
        json!("  "),
    ] {
        let registry = json!({"notes": {"root": "/home/[notes]", "remote": remote}});
        steel
            .runtime
            .invoke("publish-context", vec![registry, Value::Null])
            .unwrap();
        let commands = steel.take_commands();
        let expected = if remote.as_str().is_some_and(|r| !r.trim().is_empty()) {
            remote.clone()
        } else {
            json!("/home/[notes]")
        };
        if remote == json!("") || remote == json!("  ") {
            assert!(commands.is_empty(), "unchanged source is not republished");
        } else {
            assert_eq!(
                commands,
                vec![json!({"SetProjectContext": [["notes"], [], {"notes": expected}]})]
            );
        }
    }
    steel
        .runtime
        .invoke(
            "publish-context",
            vec![json!({"notes": {"root": "/new-root"}}), Value::Null],
        )
        .unwrap();
    assert_eq!(
        steel.take_commands(),
        vec![json!({"SetProjectContext": [["notes"], [], {"notes": "/new-root"}]})]
    );
}
