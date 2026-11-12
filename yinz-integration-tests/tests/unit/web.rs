use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};
use yinz_integration_tests::{claim_first_terminal_and_wait_for_prompt, TestRunner, TERMINAL_SIZE};
use yinz_test_support::{default_timeout, Watched};
use yinz_types::input::actions::Action;
use yinz_web::{auth::password, listener_file::ListenerRecord};
use zeroize::Zeroizing;

// Polling durable records is intentional: they are written by the server on its own runtime.
// Feed observations into Watched; every test waits on a predicate and joins its observer.
struct Records {
    values: Watched<Vec<ListenerRecord>>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    observer: Option<std::thread::JoinHandle<()>>,
}
impl Records {
    fn new(dir: PathBuf) -> Self {
        let values = Watched::new(Vec::new());
        let output = values.clone();
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let observer = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
                let mut tick = tokio::time::interval(Duration::from_millis(20));
                loop {
                    tokio::select! {
                        _ = &mut cancelled => break,
                        _ = tick.tick() => {
                            let records = std::fs::read_dir(dir.join("web/listeners")).into_iter().flatten()
                                .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
                                .filter_map(|b| serde_json::from_slice(&b).ok()).collect();
                            *output.lock() = records;
                        }
                    }
                }
            });
        });
        Self {
            values,
            cancel: Some(cancel),
            observer: Some(observer),
        }
    }
    fn serving(&self, session: &str) -> SocketAddr {
        self.values.wait_until(
            "session's web listener record",
            default_timeout(),
            |records| {
                records
                    .iter()
                    .find(|r| r.session == session)
                    .map(|r| r.address)
            },
        )
    }
    fn empty(&self) {
        self.values
            .wait_until("owned listener records removed", default_timeout(), |r| {
                r.is_empty().then_some(())
            });
    }
}
impl Drop for Records {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.observer.take().unwrap().join().unwrap();
    }
}
fn private_data() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), temp.path()).unwrap();
    temp
}
fn set(dir: &Path) {
    password::write(dir, &Zeroizing::new("integration password".into())).unwrap();
}
fn get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

fn api(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: &str) -> String {
    api_headers(addr, method, path, token, body, "")
}
fn api_headers(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
    extra: &str,
) -> String {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let authorization = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\n{authorization}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}
fn login(addr: SocketAddr) -> String {
    let reply = api(
        addr,
        "POST",
        "/api/login",
        None,
        r#"{"password":"integration password","label":"Phone"}"#,
    );
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    serde_json::from_str::<serde_json::Value>(reply.split_once("\r\n\r\n").unwrap().1).unwrap()
        ["token"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn enabled_health_starting_rename_clear_and_terminal_usable() {
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; plugin \"file:/missing-web-app.scm\"; }")
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    let reply = get(addr, "/healthz");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains("\"ok\":true"));
    assert!(reply.contains("x-content-type-options: nosniff"));
    let reply = get(addr, "/");
    assert!(reply.starts_with("HTTP/1.1 503"));
    assert!(reply.contains("Yinz web is starting"));
    terminal.output(b"WEB TERMINAL STILL USABLE\r\n");
    session.wait_until("terminal usable without web app", |g| {
        g.contains("WEB TERMINAL STILL USABLE")
    });
    let renamed = format!("{}-renamed", session.session_name());
    assert_eq!(
        session
            .run_actions_reporting(vec![Action::RenameSession {
                name: renamed.clone().into()
            }])
            .0,
        0
    );
    assert_eq!(records.serving(&renamed), addr);
    assert!(get(addr, "/healthz").contains(&renamed));
    password::clear(dir.path()).unwrap();
    records.empty();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err());
    session.quit();
}

#[test]
fn missing_invalid_password_then_write_binds_and_config_waits_for_next_start() {
    let dir = private_data();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    claim_first_terminal_and_wait_for_prompt(&session);
    assert!(records.values.get().is_empty());
    assert_eq!(
        password::read_state(dir.path()),
        password::PasswordState::Missing
    );
    yinz_runtime::shared::ensure_secure_dir(&dir.path().join("web")).unwrap();
    std::fs::write(dir.path().join("web/password"), "invalid").unwrap();
    assert!(matches!(
        password::read_state(dir.path()),
        password::PasswordState::Invalid(_)
    ));
    assert!(records.values.get().is_empty());
    set(dir.path());
    let addr = records.serving(session.session_name());
    assert_eq!(
        session
            .run_actions_reporting(vec![Action::Reconfigure {
                config: "web { enabled false; port 1; }".into(),
                write_to_disk: false
            }])
            .0,
        0
    );
    assert!(get(addr, "/healthz").starts_with("HTTP/1.1 200"));
    session.quit();
    records.empty();
    let mut disabled = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .start();
    claim_first_terminal_and_wait_for_prompt(&disabled);
    assert!(records.values.get().is_empty());
    disabled.quit();
}

#[test]
fn two_sessions_share_password_and_take_over_port_after_shutdown() {
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut first = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    claim_first_terminal_and_wait_for_prompt(&first);
    let addr = records.serving(first.session_name());
    let mut second = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config(&format!("web {{ enabled true; port {}; }}", addr.port()))
        .start();
    claim_first_terminal_and_wait_for_prompt(&second);
    assert!(get(addr, "/healthz").contains(first.session_name()));
    let survivor = login(addr);
    let revoked = login(addr);
    assert!(api(addr, "POST", "/api/logout", Some(&revoked), "{}").starts_with("HTTP/1.1 204"));
    first.quit();
    assert_eq!(records.serving(second.session_name()), addr);
    assert!(api(addr, "GET", "/api/session", Some(&survivor), "").starts_with("HTTP/1.1 200"));
    assert!(api(addr, "GET", "/api/session", Some(&revoked), "").starts_with("HTTP/1.1 401"));
    assert!(get(addr, "/healthz").contains(second.session_name()));
    second.quit();
    records.empty();
}

#[test]
fn bearer_sessions_cross_live_servers_logout_all_password_rotation_and_clear() {
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut first = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    claim_first_terminal_and_wait_for_prompt(&first);
    let a = records.serving(first.session_name());
    let mut second = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    claim_first_terminal_and_wait_for_prompt(&second);
    let b = records.serving(second.session_name());
    let token = login(a);
    assert!(api(b, "GET", "/api/session", Some(&token), "").starts_with("HTTP/1.1 200"));
    assert!(api(b, "POST", "/api/logout", Some(&token), "{}").starts_with("HTTP/1.1 204"));
    assert!(api(a, "GET", "/api/session", Some(&token), "").starts_with("HTTP/1.1 401"));
    let token = login(a);
    let other = login(b);
    assert!(api(a, "POST", "/api/logout-all", Some(&token), "{}").starts_with("HTTP/1.1 204"));
    assert!(api(b, "GET", "/api/session", Some(&other), "").starts_with("HTTP/1.1 401"));
    let token = login(a);
    set(dir.path());
    assert!(api(b, "GET", "/api/session", Some(&token), "").starts_with("HTTP/1.1 401"));
    password::clear(dir.path()).unwrap();
    records.empty();
    assert!(TcpStream::connect(a).is_err());
    assert!(TcpStream::connect(b).is_err());
    first.quit();
    second.quit();
}

fn public_request(addr: SocketAddr, method: &str, path: &str, extra: &str) -> String {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

fn wait_public(addr: SocketAddr, path: &str, text: &str) {
    let published = Watched::new(false);
    let observed = published.clone();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap()
                .block_on(async {
                    let deadline = tokio::time::Instant::now() + default_timeout();
                    let mut tick = tokio::time::interval(Duration::from_millis(20));
                    loop {
                        tick.tick().await;
                        if get(addr, path).contains(text) {
                            *observed.lock() = true;
                            break;
                        }
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                    }
                });
        });
        published.wait_until("Steel public publication", default_timeout(), |ready| {
            ready.then_some(())
        });
    });
}
fn web_plugin_operation(
    session: &yinz_integration_tests::TestSession,
    source: &Path,
    operation: yinz_types::cli::PluginOperation,
) {
    let response = yinz_client::store_client::plugin_request(
        &session.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation,
            target: Some(source.display().to_string()),
            configuration: None,
            fallback: Some(yinz_types::input::layout::RunPlugin {
                location: yinz_types::input::layout::RunPluginLocation::File(source.to_owned()),
                ..Default::default()
            }),
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap();
    assert!(!response.plugins.is_empty(), "{response:?}");
}

#[test]
fn web_transport_authenticated_steel_and_native_frozen_site() {
    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let plugin_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(plugin_dir.path().join("static")).unwrap();
    std::fs::write(plugin_dir.path().join("static/app.js"), "frozen javascript").unwrap();
    let source = plugin_dir.path().join("web-test.scm");
    std::fs::write(&source, r#"
(define count 0)
(define ready #f)
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (yinz-command (hash "HttpAssets" (hash "prefix" "/static/" "directory" "static")))
    (yinz-command (hash "HttpPublish" (hash "path" "/" "content_type" "text/html; charset=utf-8" "body" "<p>frozen page</p>")))
    (set! ready #t) #f)))
(define (yinz-web-dispatch request)
  (set! count (+ count 1))
  (define path (hash-ref request "path"))
  (define result (hash "count" count "request" request "ready" ready))
  (define (send-result)
    (yinz-command (hash "HttpRespond" (hash "request_id" (hash-ref request "id")
      "response" (yinz-json-string (hash "status" 200 "headers" (hash "content-type" "application/json" "x-yinz-test" "steel") "body" (yinz-json-string result)))))) #f)
  (cond
    [(equal? path "/api/deferred") (yinz-then (yinz-sleep 0.01) (lambda (_) (send-result)))]
    [(equal? path "/api/timeout") #f]
    [(equal? path "/api/hold")
      (yinz-command (hash "HttpPublish" (hash "path" "/pending" "content_type" "text/plain; charset=utf-8" "body" "pending request admitted"))) #f]
    [(equal? path "/api/invalid")
      (yinz-command (hash "HttpRespond" (hash "request_id" (hash-ref request "id") "response" "{\"status\":200,\"headers\":{\"set-cookie\":\"forbidden\"},\"body\":\"\"}"))) #f]
    [else (send-result)]))
"#).unwrap();
    let url = format!("file:{}", source.display());
    let cache_path = yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        yinz_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        yinz_runtime::plugin_location::parse_plugin_location(&url, None)
            .unwrap()
            .to_string(),
        vec![yinz_types::data::PermissionType::RunCommands],
    );
    cache.write_to_file().unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config(&format!("web {{ enabled true; port 0; plugin {url:?}; }}"))
        .start();
    claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    let token = login(addr);
    assert!(api(addr, "GET", "/api/echo", None, "").starts_with("HTTP/1.1 401"));
    // Public readiness polling cannot dispatch a guest request or disturb the counter.
    wait_public(addr, "/", "frozen page");
    let reply = api_headers(
        addr,
        "GET",
        "/api/echo?x=a&x=b",
        Some(&token),
        "",
        "Cookie: guest-secret-cookie\r\nTailscale-User-Login: guest-secret-proxy\r\n",
    );
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains("x-yinz-test: steel"));
    assert!(reply.contains("cache-control: no-store"));
    let value: serde_json::Value =
        serde_json::from_str(reply.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(value["count"], 1);
    assert_eq!(value["ready"], true);
    assert_eq!(value["request"]["query"]["x"], "b");
    assert_eq!(value["request"]["query_pairs"].as_array().unwrap().len(), 2);
    assert_eq!(
        value["request"]["server"]["session"],
        session.session_name()
    );
    assert!(value["request"]["headers"].get("authorization").is_none());
    assert!(!value.to_string().contains(&token));
    assert!(!value.to_string().contains("guest-secret"));
    let asset = get(addr, "/static/app.js");
    assert!(asset.ends_with("frozen javascript"), "{asset}");
    let etag = asset
        .lines()
        .find_map(|line| line.strip_prefix("etag: "))
        .unwrap()
        .trim();
    let head = public_request(addr, "HEAD", "/static/app.js", "");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(head.ends_with("\r\n\r\n"));
    let cached = public_request(
        addr,
        "GET",
        "/static/app.js",
        &format!("If-None-Match: {etag}\r\n"),
    );
    assert!(cached.starts_with("HTTP/1.1 304"));
    let next = api(addr, "GET", "/api/echo", Some(&token), "");
    let next: serde_json::Value =
        serde_json::from_str(next.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(next["count"], 2);
    assert!(api(addr, "GET", "/api/deferred", Some(&token), "").starts_with("HTTP/1.1 200"));
    assert!(api(addr, "GET", "/api/invalid", Some(&token), "").starts_with("HTTP/1.1 502"));
    let timed_out = api(addr, "GET", "/api/timeout", Some(&token), "");
    assert!(timed_out.starts_with("HTTP/1.1 504"), "{timed_out}");
    // The fixture publishes only after admitting the pending call. Reload must resolve it
    // promptly, and the next generation starts its own counter and site.
    std::thread::scope(|scope| {
        let token = &token;
        let pending = scope.spawn(move || api(addr, "GET", "/api/hold", Some(token), ""));
        wait_public(addr, "/pending", "pending request admitted");
        let replacement = std::fs::read_to_string(&source)
            .unwrap()
            .replace("frozen page", "replacement page");
        std::fs::write(&source, replacement).unwrap();
        web_plugin_operation(&session, &source, yinz_types::cli::PluginOperation::Reload);
        assert!(pending.join().unwrap().starts_with("HTTP/1.1 503"));
    });
    wait_public(addr, "/", "replacement page");
    let next = api(addr, "GET", "/api/echo", Some(&token), "");
    let next: serde_json::Value =
        serde_json::from_str(next.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(next["count"], 1);
    std::thread::scope(|scope| {
        let token = &token;
        let pending = scope.spawn(move || api(addr, "GET", "/api/hold", Some(token), ""));
        wait_public(addr, "/pending", "pending request admitted");
        web_plugin_operation(&session, &source, yinz_types::cli::PluginOperation::Stop);
        assert!(pending.join().unwrap().starts_with("HTTP/1.1 503"));
    });
    session.quit();
    records.empty();
}

// This handler answers HTTP only after the matching native AgentView response.
const WEB_AGENTS_HANDLER: &str = r#"
(define pending (hash))
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (yinz-command (hash "HttpPublish" (hash "path" "/" "content_type" "text/plain; charset=utf-8" "body" "native agent fixture ready"))) #f)))
(define (yinz-web-dispatch request)
  (define id (hash-ref request "id"))
  (define correlation (string-append "native-" (number->string id)))
  (set! pending (hash-insert pending correlation id))
  (yinz-command (hash "AgentView" (hash "id" correlation "request" (hash-ref request "body")))) #f)
(define (update event)
  (when (and (hash? event) (hash-contains? event "CustomMessage"))
    (define message (hash-ref event "CustomMessage"))
    (when (equal? (list-ref message 0) "agent-view-response")
      (define result (yinz-json-parse (list-ref message 1)))
      (define correlation (hash-ref result "id"))
      (when (hash-contains? pending correlation)
        (yinz-command (hash "HttpRespond" (hash "request_id" (hash-ref pending correlation)
          "response" (yinz-json-string (hash "status" 200 "headers" (hash "content-type" "application/json")
            "body" (yinz-json-string (hash-ref result "reply")))))))))) #f)
"#;
#[track_caller]
fn web_agents_call(
    addr: SocketAddr,
    token: &str,
    operation: serde_json::Value,
) -> serde_json::Value {
    let response = api(
        addr,
        "POST",
        "/api/agents",
        Some(token),
        &operation.to_string(),
    );
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "{operation}: {response}\n{}",
        std::fs::read_to_string(&*yinz_runtime::consts::YINZ_TMP_LOG_FILE).unwrap_or_default()
    );
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}
/// Owner-scoped retained history the native views read back. Its session ID matches the live
/// identity `report_codex_session_start` reports.
fn seed_retained_run(dir: &Path) -> agent_proto::agents::AgentRun {
    use agent_proto::{
        agents::{rows, AgentRun, RunEntry, RunEntryKind, RunOutcome},
        store,
    };
    use yinz_types::plugin_data::REV_ABSENT;
    let data = yinz_storage::data_access::DataAccess::sqlite(dir);
    for definition in store::shared_defs() {
        data.plugin_data()
            .define_entity(store::SHARED, "test", &definition, false)
            .unwrap();
    }
    let run = AgentRun {
        id: "codex:web-native".into(),
        pane_id: 1,
        yinz_session_name: "retained-session".into(),
        host_id: yinz_runtime::host_identity::host_id(),
        origin_run_id: None,
        previous_run_id: Some("previous".into()),
        session_id: "web-native".into(),
        kind: "codex".into(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 100,
        ended_at_ms: Some(200),
        outcome: RunOutcome::Completed,
        revision: 1,
        summarized_revision: 0,
        summary: None,
        omitted_entries: 0,
        entries: vec![RunEntry {
            worker_id: None,
            conversation_turn_id: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Message,
            text: "Native web retained conversation".into(),
            created_at_ms: Some(150),
            tool: None,
        }],
    };
    data.plugin_data()
        .apply(store::SHARED, &rows::run_mutations(&run, REV_ABSENT))
        .unwrap();
    run
}
/// Establish a live codex identity, session ID `web-native`, on a fake terminal pane.
fn report_codex_session_start(
    session: &yinz_integration_tests::TestSession,
    terminal: &yinz_integration_tests::FakePtyHandle,
) {
    // Use the real report IPC contract; the in-process CLI action builder reads hook
    // JSON before the fake client's stdin exists.
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    use yinz_types::agent::{
        hook_outcome,
        report::{AgentReport, AgentReportState},
        AgentEvent, HookEvent, HookOutcome,
    };
    let hook = HookEvent {
        hook_event_name: "SessionStart".into(),
        session_id: "web-native".into(),
        source: Some("startup".into()),
        ..Default::default()
    };
    let trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
    let socket = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(session.session_name()),
    )
    .unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    receiver.set_recv_timeout(Some(default_timeout())).unwrap();
    sender
        .send_client_msg(ClientToServerMsg::AgentReport(AgentReport {
            report_id: trace.report_id,
            pane_id: yinz_types::pane_id::PaneId::Terminal(terminal.terminal_id()),
            state: match hook_outcome("codex", &hook) {
                HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                _ => panic!("session start"),
            },
            event: Some(AgentEvent::from_hook(
                "codex",
                terminal.terminal_id(),
                &hook,
                serde_json::to_string(&hook).unwrap(),
            )),
            trace: false,
            process_id: trace.process_id,
            kind: "codex".into(),
        }))
        .unwrap();
    assert!(matches!(
        receiver.recv_server_msg(),
        IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _)
    ));
}
#[test]
fn web_agents_configured_steel_native_list_history_guarded_screen_and_detached_focus() {
    use serde_json::json;
    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let run = seed_retained_run(dir.path());
    let plugin_dir = tempfile::tempdir().unwrap();
    let source = plugin_dir.path().join("web-agents.scm");
    std::fs::write(&source, WEB_AGENTS_HANDLER).unwrap();
    let url = format!("file:{}", source.display());
    let cache_path = yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        yinz_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        yinz_runtime::plugin_location::parse_plugin_location(&url, None)
            .unwrap()
            .to_string(),
        vec![yinz_types::data::PermissionType::RunCommands],
    );
    cache.write_to_file().unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config(&format!("plugins {{ native-web-fixture location={url:?} {{ shared true; }}; }}\nweb {{ enabled true; port 0; plugin \"native-web-fixture\"; }}"))
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    let token = login(addr);
    wait_public(addr, "/", "native agent fixture ready");
    let empty = web_agents_call(addr, &token, json!({"op":"list"}));
    assert_eq!(empty["ok"]["session"], session.session_name());
    assert_eq!(empty["ok"]["agents"], json!([]));
    let history = web_agents_call(
        addr,
        &token,
        json!({"op":"history","run":run.id,"tz":"UTC"}),
    );
    assert_eq!(
        history["ok"]["events"][0]["markdown"],
        "Native web retained conversation"
    );
    assert_eq!(history["ok"]["previous_run"], "previous");
    report_codex_session_start(&session, &terminal);
    terminal.output(b"WEB GUARDED READ CONTENT\r\n");
    session.wait_until("native agent screen content", |s| {
        s.contains("WEB GUARDED READ CONTENT")
    });
    let list = web_agents_call(addr, &token, json!({"op":"list"}));
    let agent = list["ok"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["ref"]["session_id"] == "web-native")
        .unwrap();
    let reference = agent["ref"].clone();
    let read = web_agents_call(addr, &token, json!({"op":"screen","agent":reference}));
    assert_eq!(read["ok"]["response"]["outcome"], "ok");
    assert!(read["ok"]["response"]["output"]
        .as_str()
        .unwrap()
        .contains("WEB GUARDED READ CONTENT"));
    let focus = web_agents_call(addr, &token, json!({"op":"focus","agent":reference}));
    assert_eq!(focus["ok"], true);
    let mut stale = reference.clone();
    stale["kind"] = json!("claude");
    assert_eq!(
        web_agents_call(
            addr,
            &token,
            json!({"op":"keys","agent":stale,"keys":["enter"]})
        )["error"]["code"],
        "target_changed"
    );
    session.detach_main_client();
    assert_eq!(
        web_agents_call(addr, &token, json!({"op":"list"}))["ok"]["agents"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        web_agents_call(addr, &token, json!({"op":"screen","agent":reference}))["ok"]["response"]
            ["outcome"],
        "ok"
    );
    let focus = web_agents_call(addr, &token, json!({"op":"focus","agent":reference}));
    assert_eq!(focus["error"]["code"], "unavailable");
    assert_eq!(focus["error"]["message"], "No terminal attached");
    session.quit();
    records.empty();
}

#[test]
fn web_sdk_authenticated_routes_pages_assets_and_failures() {
    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let plugin_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(plugin_dir.path().join("static")).unwrap();
    std::fs::write(plugin_dir.path().join("static/app.js"), "sdk asset").unwrap();
    std::fs::write(plugin_dir.path().join("static/login.js"), r#"
document.querySelector('form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const status = document.querySelector('#status');
  try {
    const reply = await fetch('/api/login', {method:'POST', headers:{'Content-Type':'application/json'},
      body:JSON.stringify({password:document.querySelector('input').value,label:'Browser smoke'})});
    status.textContent = reply.ok ? 'Logged in' : 'Login failed';
  } catch (_) { status.textContent = 'Login failed'; }
});
"#).unwrap();
    let source = plugin_dir.path().join("sdk.scm");
    std::fs::write(&source,r#"
      (define (load config)
        (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
          (yinz-web-assets "/static/" "static")
          (yinz-web-page "/login" "text/html; charset=utf-8" (lambda () (yinz-html-document
            (yinz-h "title" (hash) "<SDK login>")
            (list (yinz-html-form (hash "action" "/api/login")
                    (yinz-h "label" (hash "for" "password") "Password")
                    (yinz-h "input" (hash "id" "password" "type" "password" "name" "password" "required" #t))
                    (yinz-h "button" (hash "type" "submit") "Log in"))
                  (yinz-h "p" (hash "id" "status" "role" "status" "aria-live" "polite") "JavaScript is required to log in.")
                  (yinz-h "p" (hash) "JavaScript is required to log in.")
                  (yinz-h "noscript" (hash) "JavaScript is required to log in.")
                  (yinz-html-json-script "bootstrap" (hash "text" "</script>\u{2028}\u{2029}"))
                  (yinz-h "script" (hash "src" "/static/login.js"))))))
          (yinz-web-hook-define "title" (lambda () "<SDK>"))
          (yinz-web-page "/" "text/html; charset=utf-8" (lambda () (yinz-html-document
            (yinz-h "title" (hash) (yinz-web-hook "title"))
            (list (yinz-h "p" (hash) "SDK ready") (yinz-html-json-script "data" (hash "text" "</script>"))))))
          #f)))
      (yinz-web-route "GET" "/api/echo/:id" (lambda (r) (yinz-web-text (string-append (hash-ref r "method") ":" (yinz-web-param r "id")))))
      (yinz-web-route "*" "/api/override" (lambda (_) (yinz-web-text "wild")))
      (yinz-web-route "GET" "/api/override" (lambda (_) (yinz-web-text "get")))
      (yinz-web-route "HEAD" "/api/override" (lambda (_) (yinz-web-text "explicit-head" 202)))
      (yinz-web-route "HEAD" "/api/echo/:x" (lambda (_) (yinz-web-text "param" 203)))
      (yinz-web-route "GET" "/api/echo/literal" (lambda (r) (yinz-web-text (hash-ref r "method"))))
      (yinz-web-route "HEAD" "/api/only-head" (lambda (_) (yinz-web-text "head")))
      (yinz-web-route "GET" "/api/deferred" (lambda (_) (yinz-then (yinz-sleep 1) (lambda (_) (yinz-web-text "deferred")))))
      (yinz-web-route "GET" "/api/raised" (lambda (_) (error "hidden exception")))
      (yinz-web-route "GET" "/api/rejected" (lambda (_) (define d (yinz-deferred)) (yinz-reject! d "hidden rejection") d))
      (yinz-web-route "POST" "/api/json" (lambda (r) (yinz-web-json (yinz-web-body-json r))))
      (yinz-web-route "POST" "/api/republish" (lambda (_) (yinz-web-hook-wrap! "title" (lambda (next) "updated")) (yinz-web-text "ok")))
    "#).unwrap();
    let url = format!("file:{}", source.display());
    let cache_path = yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        yinz_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        yinz_runtime::plugin_location::parse_plugin_location(&url, None)
            .unwrap()
            .to_string(),
        vec![yinz_types::data::PermissionType::RunCommands],
    );
    cache.write_to_file().unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config(&format!("web {{ enabled true; port 0; plugin {url:?}; }}"))
        .start();
    claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    let token = login(addr);
    wait_public(addr, "/", "SDK ready");
    let page = get(addr, "/");
    assert!(page.contains("&lt;SDK&gt;"));
    assert!(page.contains("\\u003c/script\\u003e"));
    assert!(page.contains("form-action 'none'"));
    assert!(get(addr, "/static/app.js").ends_with("sdk asset"));
    assert!(api(addr, "GET", "/api/echo/7", None, "").starts_with("HTTP/1.1 401"));
    let response = api(addr, "GET", "/api/echo/7", Some(&token), "");
    assert!(response.ends_with("GET:7"), "{response}");
    // GET fallback preserves HEAD; explicit method rank is applied after path specificity.
    for (path, status, length) in [
        ("/api/echo/literal", 200, 4),
        ("/api/override", 202, 13),
        ("/api/deferred", 200, 8),
        ("/api/missing", 404, 30),
    ] {
        let response = api(addr, "HEAD", path, Some(&token), "");
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
        assert!(response.ends_with("\r\n\r\n"), "{response}");
        if status != 404 {
            assert!(
                response.contains(&format!("content-length: {length}\r\n")),
                "{response}"
            );
        }
        assert!(response.contains("cache-control: no-store"));
        assert!(response.contains("default-src 'none'"));
    }
    for method in ["GET", "POST"] {
        assert!(api(addr, method, "/api/only-head", Some(&token), "").starts_with("HTTP/1.1 404"));
    }
    for path in ["/api/raised", "/api/rejected"] {
        let response = api(addr, "GET", path, Some(&token), "");
        assert!(response.starts_with("HTTP/1.1 500"));
        assert!(!response.contains("hidden"));
    }
    assert!(api(addr, "POST", "/api/json", Some(&token), "{").starts_with("HTTP/1.1 400"));
    assert!(api(addr, "POST", "/api/json", Some(&token), "null").ends_with("null"));
    assert!(api(addr, "POST", "/api/republish", Some(&token), "{}").starts_with("HTTP/1.1 200"));
    wait_public(addr, "/", "updated");
    if let Some(script) = std::env::var_os("YINZ_WEB_BROWSER_SMOKE") {
        let result = std::process::Command::new("python")
            .arg(script)
            .arg(addr.to_string())
            .status()
            .unwrap();
        assert!(result.success(), "supplementary Firefox smoke failed");
    }
    session.quit();
    records.empty();
}

/// A whole HTTP/1.1 reply read as bytes, because PNG bodies are not UTF-8.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
    #[track_caller]
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {self}"))
    }
    #[track_caller]
    fn error_code(&self, status: u16) -> String {
        assert_eq!(self.status, status, "{self}");
        self.json()["error"]["code"].as_str().unwrap().into()
    }
}
impl std::fmt::Display for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {:?} ", self.status, self.headers)?;
        f.write_str(&String::from_utf8_lossy(&self.body))
    }
}
fn exchange(addr: SocketAddr, head: &str, body: &str) -> Reply {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "{head}Connection: close\r\n\r\n{body}").unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    let split = reply
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("complete reply head");
    let head = std::str::from_utf8(&reply[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap()["HTTP/1.1 ".len()..][..3]
        .parse()
        .unwrap();
    let headers = lines
        .map(|line| {
            let (key, value) = line.split_once(':').unwrap();
            (key.trim().to_owned(), value.trim().to_owned())
        })
        .collect();
    Reply {
        status,
        headers,
        body: reply[split + 4..].to_vec(),
    }
}
/// A browser navigation or subresource load: no bearer, no JSON content type.
fn fetch(addr: SocketAddr, method: &str, path: &str, extra: &str) -> Reply {
    exchange(
        addr,
        &format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{extra}"),
        "",
    )
}
/// A same-origin `fetch` from the frontend's API client.
fn call(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: &str) -> Reply {
    let authorization = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    exchange(
        addr,
        &format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\n\
             Content-Type: application/json\r\n{authorization}Content-Length: {}\r\n",
            body.len()
        ),
        body,
    )
}
#[track_caller]
fn call_ok(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    body: &str,
) -> serde_json::Value {
    let reply = call(addr, method, path, Some(token), body);
    assert_eq!(
        reply.status,
        200,
        "{method} {path}: {reply}\n{}",
        std::fs::read_to_string(&*yinz_runtime::consts::YINZ_TMP_LOG_FILE).unwrap_or_default()
    );
    reply.json()
}
fn query(path: &str, pairs: &[(&str, &str)]) -> String {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!("{path}?{encoded}")
}
#[track_caller]
fn assert_page(reply: &Reply) {
    assert_eq!(reply.status, 200, "{reply}");
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    let policy = reply.header("content-security-policy").expect("page CSP");
    assert!(policy.contains("script-src 'self'"), "{policy}");
    assert!(!policy.contains("unsafe-inline"), "{policy}");
    assert!(!policy.contains("unsafe-eval"), "{policy}");
}
/// The bootstrap JSON carries only the public whitelist.
#[track_caller]
fn assert_bootstrap(page: &str) {
    let start = page
        .find(r#"<script type="application/json" id="yinz-config">"#)
        .expect("bootstrap JSON script")
        + r#"<script type="application/json" id="yinz-config">"#.len();
    let end = start + page[start..].find("</script>").unwrap();
    let config: serde_json::Value = serde_json::from_str(&page[start..end]).unwrap();
    let mut keys: Vec<_> = config.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["poll", "quick_keys", "timeouts", "title"]);
    assert_eq!(config["title"], "yinz");
}
/// Follow the frontend's static ES module graph from its entry points, fetching each module
/// once. Every specifier must be local and relative: no bare names, import maps or CDNs.
fn fetch_module_graph(addr: SocketAddr, entries: &[&str]) -> Vec<String> {
    let specifier = regex::Regex::new(r#"(?:\bfrom|\bimport)\s*\(?\s*["']([^"']+)["']"#).unwrap();
    let origin = url::Url::parse("http://localhost/").unwrap();
    let mut pending: Vec<url::Url> = entries.iter().map(|e| origin.join(e).unwrap()).collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(module) = pending.pop() {
        if !seen.insert(module.path().to_owned()) {
            continue;
        }
        let reply = fetch(addr, "GET", module.path(), "");
        assert_eq!(reply.status, 200, "{module}: {reply}");
        assert_eq!(
            reply.header("content-type"),
            Some("text/javascript; charset=utf-8"),
            "{module}"
        );
        for import in specifier.captures_iter(reply.text()) {
            let import = &import[1];
            assert!(
                import.starts_with("./") || import.starts_with("../"),
                "{module} imports non-relative {import:?}"
            );
            let resolved = module.join(import).unwrap();
            assert_eq!(resolved.origin(), origin.origin(), "{import}");
            assert!(resolved.path().starts_with("/static/"), "{resolved}");
            pending.push(resolved);
        }
    }
    seen.into_iter().collect()
}

#[test]
fn web_builtin_production_http() {
    use serde_json::json;
    // Pane OS and process execution is fake: TestRunner's FakeServerOsApi and fake PTY stand in
    // for real shells. The HTTP listener, native password and bearer authentication, plugin host,
    // supervised compilation worker and the builtin yinz:web plugin, installed from this checkout
    // with its frontend, are the production paths. No custom web plugin is configured.
    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let run = seed_retained_run(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());

    // Frozen pages, waiting on published content rather than publication counts.
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    wait_public(addr, "/login", r#"id="login-form""#);
    let app = fetch(addr, "GET", "/", "");
    assert_page(&app);
    let page = app.text();
    assert!(
        page.contains(r#"<script src="/static/app.js" type="module"></script>"#),
        "{page}"
    );
    assert!(
        page.contains(r#"href="/static/manifest.webmanifest""#),
        "{page}"
    );
    assert!(page.contains(r#"href="/static/app.css""#), "{page}");
    assert_bootstrap(page);
    let login_page = fetch(addr, "GET", "/login", "");
    assert_page(&login_page);
    let login_html = login_page.text();
    for id in [
        "login-form",
        "password",
        "label",
        "remember",
        "login-submit",
        "login-status",
    ] {
        assert!(
            login_html.contains(&format!(r#"id="{id}""#)),
            "{id}: {login_html}"
        );
    }
    assert!(login_html.contains(r#"autocomplete="current-password""#));
    assert!(login_html.contains(r#"<script src="/static/login.js" type="module"></script>"#));
    assert_bootstrap(login_html);
    let head = fetch(addr, "HEAD", "/", "");
    assert_eq!(head.status, 200, "{head}");
    assert!(head.body.is_empty(), "{head}");
    assert_eq!(
        head.header("content-length"),
        Some(app.body.len().to_string().as_str())
    );

    // Assets: the whole local module graph, styles, manifest and icons.
    let modules = fetch_module_graph(addr, &["/static/app.js", "/static/login.js"]);
    for expected in [
        "/static/api.js",
        "/static/views/ui.js",
        "/static/vendor/preact.module.js",
    ] {
        assert!(
            modules.iter().any(|m| m == expected),
            "{expected}: {modules:?}"
        );
    }
    let css = fetch(addr, "GET", "/static/app.css", "");
    assert_eq!(css.status, 200, "{css}");
    assert_eq!(css.header("content-type"), Some("text/css; charset=utf-8"));
    let etag = css.header("etag").expect("asset etag").to_owned();
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "strong {etag}"
    );
    let cached = fetch(
        addr,
        "GET",
        "/static/app.css",
        &format!("If-None-Match: {etag}\r\n"),
    );
    assert_eq!(cached.status, 304, "{cached}");
    assert!(cached.body.is_empty());
    let manifest = fetch(addr, "GET", "/static/manifest.webmanifest", "");
    assert_eq!(manifest.status, 200, "{manifest}");
    assert_eq!(
        manifest.header("content-type"),
        Some("application/manifest+json")
    );
    let icons = manifest.json()["icons"].as_array().unwrap().clone();
    assert!(!icons.is_empty());
    for icon in &icons {
        let src = icon["src"].as_str().unwrap();
        let reply = fetch(addr, "GET", src, "");
        assert_eq!(reply.status, 200, "{src}");
        assert_eq!(reply.header("content-type"), icon["type"].as_str(), "{src}");
    }
    for (path, kind, magic) in [
        ("/static/icon.svg", "image/svg+xml", &b"<svg"[..]),
        ("/static/icon-192.png", "image/png", &b"\x89PNG"[..]),
        ("/static/icon-512.png", "image/png", &b"\x89PNG"[..]),
    ] {
        let reply = fetch(addr, "GET", path, "");
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(reply.header("content-type"), Some(kind), "{path}");
        assert!(
            reply
                .body
                .windows(magic.len())
                .take(256)
                .any(|w| w == magic),
            "{path}"
        );
    }

    // Native authentication, then the plugin routes' failure contract.
    assert_eq!(
        call(addr, "GET", "/api/agents", None, "").error_code(401),
        "unauthenticated"
    );
    let token = login(addr);
    for path in ["/", "/login"] {
        let published = fetch(addr, "GET", path, "");
        assert!(!published.text().contains("integration password"), "{path}");
        assert!(!published.text().contains(&token), "{path}");
        assert!(!published.text().contains("yzw1_"), "{path}");
    }
    assert!(call_ok(addr, "GET", "/api/session", &token, "")["session"]["id"].is_string());
    let empty = call_ok(addr, "GET", "/api/agents", &token, "");
    assert_eq!(empty["session"], session.session_name());
    assert!(empty["agents"].is_array(), "{empty}");
    assert!(call_ok(addr, "GET", "/api/prompts", &token, "")["prompts"].is_array());
    let head = call(addr, "HEAD", "/api/agents", Some(&token), "");
    assert_eq!(head.status, 200, "{head}");
    assert!(head.body.is_empty(), "{head}");
    assert_eq!(
        call(addr, "GET", "/api/nope", Some(&token), "").error_code(404),
        "not_found"
    );
    assert_eq!(
        call(addr, "GET", "/api/agent/prompt", Some(&token), "").error_code(405),
        "method_not_allowed"
    );
    for path in [
        "/api/history",
        "/api/agents?unknown=1",
        "/api/event?run=x&revision=0",
    ] {
        assert_eq!(
            call(addr, "GET", path, Some(&token), "").error_code(400),
            "invalid",
            "{path}"
        );
    }

    // A live agent identity through the real report IPC, on a fake terminal.
    report_codex_session_start(&session, &terminal);
    terminal.output(b"WEB BUILTIN SCREEN CONTENT\r\n");
    session.wait_until("builtin agent screen content", |s| {
        s.contains("WEB BUILTIN SCREEN CONTENT")
    });
    let list = call_ok(addr, "GET", "/api/agents", &token, "");
    let rows = list["agents"].as_array().unwrap();
    assert!(rows.iter().all(|row| row["html"].is_string()), "{list}");
    let agent = rows
        .iter()
        .find(|a| a["ref"]["session_id"] == "web-native")
        .unwrap_or_else(|| panic!("{list}"));
    assert!(
        agent["html"].as_str().unwrap().contains("agent-row"),
        "{agent}"
    );
    let reference = agent["ref"].clone();
    let agent_id = agent["id"].as_str().unwrap().to_owned();

    let history = call_ok(
        addr,
        "GET",
        &query(
            "/api/history",
            &[("run", &run.id), ("agent", &agent_id), ("tz", "UTC")],
        ),
        &token,
        "",
    );
    // The fixture run is the live agent's run, so history stays bound to it.
    assert_eq!(history["retargeted"], false, "{history}");
    assert_eq!(history["live"]["ref"], reference, "{history}");
    let event = &history["events"][0];
    assert_eq!(
        event["markdown"], "Native web retained conversation",
        "{history}"
    );
    assert!(event["html"]
        .as_str()
        .unwrap()
        .contains("Native web retained conversation"));
    assert!(
        history["header_html"]
            .as_str()
            .unwrap()
            .contains("agent-header"),
        "{history}"
    );
    assert!(history["details_html"]
        .as_str()
        .unwrap()
        .contains(r#"class="details""#));
    assert!(history["navigation"].is_object(), "{history}");
    assert_eq!(history["previous_run"], "previous");
    let reader = call_ok(
        addr,
        "GET",
        &query(
            "/api/event",
            &[("run", &run.id), ("revision", "1"), ("tz", "UTC")],
        ),
        &token,
        "",
    );
    assert!(
        reader["html"]
            .as_str()
            .unwrap()
            .contains("Native web retained conversation"),
        "{reader}"
    );

    let screen = call_ok(
        addr,
        "POST",
        "/api/agent/screen",
        &token,
        &json!({"agent": reference}).to_string(),
    );
    assert_eq!(screen["response"]["outcome"], "ok", "{screen}");
    let screen_html = screen["html"].as_str().unwrap();
    assert!(
        screen_html.starts_with(r#"<pre class="screen">"#),
        "{screen_html}"
    );
    assert!(screen_html.contains("WEB BUILTIN SCREEN CONTENT"));

    // Guarded keys need a verifiable live agent process. The fake PTY's process is unmanaged
    // and has no bracketed paste, so native preserves a non-success outcome inside HTTP 200
    // and sends nothing; the client reads response.outcome, never the status alone.
    let keys = call_ok(
        addr,
        "POST",
        "/api/agent/keys",
        &token,
        &json!({"agent": reference, "keys": ["y"]}).to_string(),
    );
    assert_eq!(keys["response"]["outcome"], "status_unavailable", "{keys}");
    assert!(keys["response"]["message"]
        .as_str()
        .unwrap()
        .contains("cannot verify a live agent process"));
    assert!(terminal.stdin_bytes().is_empty());

    // Native rejects a stale identity before any delivery: HTTP 409, not a 200 outcome.
    let mut stale = reference.clone();
    stale["session_id"] = json!("web-stale");
    let prompt = call(
        addr,
        "POST",
        "/api/agent/prompt",
        Some(&token),
        &json!({"agent": stale, "submission_id": "web-stale-1", "text": "STALE PROMPT"})
            .to_string(),
    );
    assert_eq!(prompt.error_code(409), "target_changed");
    assert_eq!(
        prompt.json()["error"]["message"],
        "Agent disappeared or changed identity"
    );
    assert!(!String::from_utf8_lossy(&terminal.stdin_bytes()).contains("STALE PROMPT"));

    // The harness client is attached, so native focus succeeds; once detached, it reports that
    // no terminal is attached.
    let focus = json!({"agent": reference}).to_string();
    assert_eq!(
        call_ok(addr, "POST", "/api/agent/focus", &token, &focus),
        json!({"ok": true})
    );
    session.detach_main_client();
    let detached = call(addr, "POST", "/api/agent/focus", Some(&token), &focus);
    assert_eq!(detached.error_code(409), "unavailable");
    assert_eq!(detached.json()["error"]["message"], "No terminal attached");

    // Logout revokes the bearer; the client recovers by signing in again.
    let logout = call(addr, "POST", "/api/logout", Some(&token), "{}");
    assert_eq!(logout.status, 204, "{logout}");
    assert!(logout.body.is_empty());
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(&token), "").error_code(401),
        "unauthenticated"
    );
    let token = login(addr);
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(&token), "").status,
        200
    );
    session.quit();
    records.empty();
}

#[test]
#[ignore = "needs the test-only Playwright package and Firefox; see docs/PLUGIN_TESTING.md"]
fn web_builtin_browser_phone_flows() {
    use std::{
        process::{Command, Stdio},
        sync::mpsc,
    };
    // A real Playwright Firefox drives the builtin frontend on the same default-builtin HTTP
    // session as web_builtin_production_http: the native listener, password login, bearer
    // sessions, logout, frozen pages and assets stay live. run.mjs intercepts only the agent-data
    // routes with labelled deterministic fixtures, replaying each bearer against the live
    // /api/session. Ignored by default, so the unfiltered integration suite and `cargo x ci check`
    // never need the tooling; when selected with `--run-ignored only`, missing tooling fails with
    // the provisioning command and never skips.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    const RERUN: &str = "env PLAYWRIGHT_BROWSERS_PATH=$PWD/target/web-browser cargo x \
                         integration-test --run-ignored only -E \
                         'test(web_builtin_browser_phone_flows)' -j 16";
    let package = root.join("yinz-steel-builtins/src/unit/web/browser");
    assert!(
        package
            .join("node_modules/playwright/package.json")
            .is_file(),
        "the test-only Playwright package is missing; from {} run:\n  npm ci --prefix \
         yinz-steel-builtins/src/unit/web/browser --ignore-scripts --no-audit --no-fund\nthen \
         rerun with:\n  {RERUN}",
        root.display()
    );
    let browsers = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_else(|| {
        panic!(
            "PLAYWRIGHT_BROWSERS_PATH must name the provisioned browsers; from {} run:\n  env \
             PLAYWRIGHT_BROWSERS_PATH=$PWD/target/web-browser node \
             yinz-steel-builtins/src/unit/web/browser/node_modules/playwright/cli.js install \
             firefox\nthen rerun with the same PLAYWRIGHT_BROWSERS_PATH:\n  {RERUN}",
            root.display()
        )
    });

    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let _terminal = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    wait_public(addr, "/login", r#"id="login-form""#);

    let mut child = Command::new("node")
        .arg(package.join("run.mjs"))
        .arg(format!("http://{addr}"))
        .current_dir(&root)
        .env("PLAYWRIGHT_BROWSERS_PATH", &browsers)
        .env_remove("YINZ_WEB_PASSWORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("could not start `node` (install Node.js 20 or newer): {e}"));
    // The password goes over stdin, never through arguments or the environment.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"integration password\n")
        .unwrap();
    let pid = nix::unistd::Pid::from_raw(child.id().try_into().unwrap());
    let (done, finished) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let _ = done.send(child.wait_with_output());
    });
    // run.mjs bounds every wait itself; this deadline only reaps a wedged browser. SIGTERM lets
    // it close Firefox before exiting.
    let output = finished
        .recv_timeout(Duration::from_secs(300))
        .unwrap_or_else(|_| {
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
            finished
                .recv_timeout(Duration::from_secs(30))
                .expect("browser checks ignored SIGTERM")
        })
        .expect("browser checks output");
    waiter.join().unwrap();
    session.quit();
    records.empty();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("integration password") && !stderr.contains("integration password"));
    assert!(
        output.status.success() && stdout.contains("browser scenarios passed"),
        "browser phone flows failed ({}):\n{stdout}\n{stderr}",
        output.status
    );
}
