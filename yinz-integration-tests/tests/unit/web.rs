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
    assert!(
        reply.contains("Yinz web is starting") || reply.contains("Yinz web is unavailable"),
        "{reply}"
    );
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
    wait_http("Steel public publication", || {
        get(addr, path).contains(text)
    });
}

fn wait_http(what: &str, ready: impl Fn() -> bool + Send) {
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
                        if ready() {
                            *observed.lock() = true;
                            break;
                        }
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                    }
                });
        });
        published.wait_until(what, default_timeout(), |ready| ready.then_some(()));
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
fn seed_retained_run(
    dir: &Path,
    session_name: &str,
    pane_id: u32,
) -> agent_proto::agents::AgentRun {
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
        pane_id,
        yinz_session_name: session_name.into(),
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
    report_session_start(session, terminal, "codex", "web-native");
}

fn report_session_start(
    session: &yinz_integration_tests::TestSession,
    terminal: &yinz_integration_tests::FakePtyHandle,
    kind: &str,
    conversation: &str,
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
        session_id: conversation.into(),
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
            state: match hook_outcome(kind, &hook) {
                HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                _ => panic!("session start"),
            },
            event: Some(AgentEvent::from_hook(
                kind,
                terminal.terminal_id(),
                &hook,
                serde_json::to_string(&hook).unwrap(),
            )),
            trace: false,
            process_id: trace.process_id,
            kind: kind.into(),
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
    let run = seed_retained_run(dir.path(), "retained-session", 1);
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

#[path = "web_http.rs"]
mod http;
use http::*;
impl Reply {
    #[track_caller]
    fn error_code(&self, status: u16) -> String {
        assert_eq!(self.status, status, "{self}");
        self.json()["error"]["code"].as_str().unwrap().into()
    }
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
    let run = seed_retained_run(dir.path(), "retained-session", 1);
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
    for (path, html) in [("/", page), ("/login", login_html)] {
        assert!(
            !html.contains("apple-mobile-web-app-status-bar-style"),
            "{path} should let iOS choose the default status bar style"
        );
    }
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
    assert_eq!(agent["tab_group_session"], session.session_name());
    assert_eq!(
        agent["tab_group"],
        json!({"id":0,"name":"Default"}),
        "{agent}"
    );

    let (exit, output) = session.run_actions_reporting(vec![Action::RenameTabGroup {
        id: 0,
        name: "Release Review".into(),
    }]);
    assert_eq!(exit, 0, "{output}");
    let renamed = call_ok(addr, "GET", "/api/agents", &token, "");
    let agent = renamed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == agent_id)
        .unwrap_or_else(|| panic!("{renamed}"));
    assert_eq!(agent["tab_group_session"], session.session_name());
    assert_eq!(
        agent["tab_group"],
        json!({"id":0,"name":"Release Review"}),
        "{agent}"
    );
    assert_eq!(agent["ref"], reference);
    assert!(agent.get("detail_id").is_none(), "{agent}");

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
    assert!(history["live"].get("tab_group").is_none(), "{history}");
    assert!(
        history["live"].get("tab_group_session").is_none(),
        "{history}"
    );
    assert!(history["live"].get("detail_id").is_none(), "{history}");
    let unknown_peer = format!("peer-detail:{}", "0".repeat(64));
    let unbound = call_ok(
        addr,
        "GET",
        &query(
            "/api/history",
            &[("run", &run.id), ("agent", &unknown_peer)],
        ),
        &token,
        "",
    );
    assert_eq!(unbound["run"], run.id, "{unbound}");
    assert_eq!(unbound["agent_gone"], true, "{unbound}");
    assert!(unbound["live"].is_null(), "{unbound}");
    assert_eq!(
        unbound["events"][0]["markdown"],
        "Native web retained conversation"
    );
    let unavailable = call(
        addr,
        "GET",
        &query("/api/usage", &[("agent", &unknown_peer)]),
        Some(&token),
        "",
    );
    assert_eq!(unavailable.error_code(404), "not_found");

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

    let frame = call_ok(
        addr,
        "POST",
        "/api/agent/term",
        &token,
        &json!({"agent": reference}).to_string(),
    );
    let frame = &frame["response"]["terminal"];
    assert_eq!(frame["snapshot"], true, "{frame}");
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        frame["data"].as_str().unwrap(),
    )
    .unwrap();
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains("WEB BUILTIN SCREEN CONTENT"));
    let update = b"\x1b[32mLIVE TERM UPDATE\x1b[0m\r\n";
    terminal.output(update);
    session.wait_until("live terminal update", |s| s.contains("LIVE TERM UPDATE"));
    let next = call_ok(
        addr,
        "POST",
        "/api/agent/term",
        &token,
        &json!({"agent": reference, "after": frame["cursor"]}).to_string(),
    );
    assert_eq!(next["response"]["terminal"]["snapshot"], false, "{next}");

    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        next["response"]["terminal"]["data"].as_str().unwrap(),
    )
    .unwrap();
    assert!(bytes.windows(update.len()).any(|window| window == update));

    // Exercise the full native → Steel → HTTP response path at the maximum part size.
    let pair = "\x1b[38;2;123;145;167mA\x1b[38;2;201;211;221mB";
    let row = format!("{}\r\n", pair.repeat(40));
    let marker = "WEB MULTIPART COMPLETE";
    terminal.output(format!("{}{marker}", row.repeat(200)).as_bytes());
    session.wait_until("styled terminal output consumed", |s| s.contains(marker));
    let mut body = json!({"agent": reference});
    let mut snapshot = Vec::new();
    loop {
        let reply = call(
            addr,
            "POST",
            "/api/agent/term",
            Some(&token),
            &body.to_string(),
        );
        assert_eq!(reply.status, 200, "{reply}");
        assert!(reply.body.len() <= yinz_types::web::MAX_RESPONSE_BODY);
        let frame = &reply.json()["response"]["terminal"];
        assert_eq!(frame["snapshot"], snapshot.is_empty());
        let part = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            frame["data"].as_str().unwrap(),
        )
        .unwrap();
        if snapshot.is_empty() {
            assert_eq!(frame["snapshot_continues"], true);
        }
        snapshot.extend(part);
        if frame["snapshot_continues"] == false {
            break;
        }
        assert_ne!(body["after"], frame["cursor"]);
        body["after"] = frame["cursor"].clone();
    }
    assert!(String::from_utf8(snapshot).unwrap().contains(marker));

    let input = call_ok(
        addr,
        "POST",
        "/api/agent/term/input",
        &token,
        &json!({"agent": reference, "data": "eA=="}).to_string(),
    );
    assert_eq!(
        input["response"]["outcome"], "status_unavailable",
        "{input}"
    );

    for data in [
        json!({"bare_key": "Enter", "key_modifiers": ["Shift"]}),
        json!({"mouse": "G1tNICEh"}),
    ] {
        let input = call_ok(
            addr,
            "POST",
            "/api/agent/term/input",
            &token,
            &json!({"agent": reference, "data": data}).to_string(),
        );
        assert_eq!(
            input["response"]["outcome"], "status_unavailable",
            "{input}"
        );
    }
    let invalid_key = json!({"bare_key": {"F": 0}, "key_modifiers": []});
    let invalid = call(
        addr,
        "POST",
        "/api/agent/term/input",
        Some(&token),
        &json!({"agent": reference, "data": invalid_key}).to_string(),
    );
    assert_eq!(invalid.error_code(400), "invalid");

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

    let receipt = call_ok(
        addr,
        "POST",
        "/api/agent/submission-status",
        &token,
        &json!({"agent": reference, "submission_id": "unknown"}).to_string(),
    );
    assert_eq!(receipt["response"]["outcome"], "status_unavailable");
    assert_eq!(receipt["response"]["submission_id"], "unknown");
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
fn web_builtin_siri_pair_search_and_launch_once() {
    use agent_proto::{projects, store};
    use serde_json::json;

    let root = yinz_integration_tests::test_env::write_git_repository("siri-swamp", "trunk")
        .canonicalize()
        .unwrap();
    let dir = private_data();
    set(dir.path());
    let mut project = projects::validate("swamp", root.to_str().unwrap()).unwrap();
    project.worktree_dir = Some("branches".into());
    let data = yinz_storage::data_access::DataAccess::sqlite(dir.path());
    data.plugin_data()
        .define_entity(store::SHARED, "test", &store::project_def(), false)
        .unwrap();
    data.plugin_data()
        .apply(store::SHARED, &[projects::to_mutation(&project)])
        .unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let first = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    let asset = fetch(addr, "GET", "/static/talk-to-yinz.shortcut", "");
    assert_eq!(asset.status, 200);
    assert_eq!(
        asset.header("content-type"),
        Some("application/x-apple-shortcut")
    );
    assert!(asset.body.starts_with(b"AEA1"));
    let browser = login(addr);
    let code = call_ok(addr, "POST", "/api/siri/code", &browser, "{}");
    let paired = call(addr, "POST", "/api/siri/pair", None, &code.to_string());
    assert_eq!(paired.status, 200);
    let paired = paired.json();
    let token = paired["token"].as_str().unwrap();
    assert_eq!(
        call(addr, "POST", "/api/siri/pair", None, &code.to_string()).status,
        401
    );
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(token), "").status,
        401
    );
    assert_eq!(
        call(addr, "POST", "/api/siri/code", Some(token), "{}").status,
        401
    );
    let command = |id: &str, text: &str| {
        call_ok(
            addr,
            "POST",
            "/api/siri/command",
            token,
            &json!({"request_id":id,"command":text}).to_string(),
        )
    };
    let run = seed_retained_run(
        dir.path(),
        session.session_name(),
        first.terminal_id().get(),
    );
    report_codex_session_start(&session, &first);
    let mut entry = run.entries[0].clone();
    entry.revision += 1;
    entry.text = format!("{} session renewal", "word ".repeat(130));
    let mut newest = entry.clone();
    newest.revision += 1;
    newest.text = "The latest reply is different".into();
    data.plugin_data()
        .apply(
            store::SHARED,
            &[entry, newest]
                .map(|e| agent_proto::agents::rows::entry_mutation(&run.id, &run.host_id, &e)),
        )
        .unwrap();
    let mut id = paired["request_id"].as_str().unwrap().to_owned();
    let mut say = |text: &str| {
        let reply = command(&id, text);
        id = reply["request_id"].as_str().unwrap().into();
        reply
    };
    let hit = say("Find the agent that mentioned session renewal");
    assert!(
        hit["speech"].as_str().unwrap().contains("session renewal"),
        "{hit}"
    );
    let read = say("Read that match");
    assert!(
        read["speech"]
            .as_str()
            .unwrap()
            .ends_with("session renewal"),
        "{read}"
    );
    assert_eq!(
        say("Read its latest reply")["speech"],
        "The latest reply is different"
    );
    let miss = say("Find the unicorn agent in swamp");
    assert!(miss["speech"]
        .as_str()
        .unwrap()
        .contains("No live agents matched"));
    let launch_id = miss["request_id"].as_str().unwrap();
    let launch = "Open a new Claude agent in a swamp worktree";
    let receipt = say(launch);
    assert_eq!(command(launch_id, launch), receipt);
    let claude = session.expect_pty_spawn();
    let cwd = claude.run_command().cwd.unwrap();
    assert_eq!(cwd.parent().unwrap(), root.join("branches"));
    assert!(cwd
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("siri-"));
    assert_eq!(std::fs::read_dir(root.join("branches")).unwrap().count(), 1);
    report_session_start(&session, &claude, "claude", "siri-claude");
    let open = say("Show me that agent");
    assert!(
        open["path"]
            .as_str()
            .is_some_and(|s| s.contains("tab=timeline")),
        "{open}"
    );
    let list = call_ok(addr, "GET", "/api/agents", &browser, "");
    let claudes: Vec<_> = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["ref"]["kind"] == "claude")
        .collect();
    assert_eq!(claudes.len(), 1);
    assert_eq!(claudes[0]["ref"]["session_id"], "siri-claude");
    assert_eq!(
        say("What did it say?")["speech"],
        "There is no recorded reply for this request."
    );
    call_ok(addr, "POST", "/api/siri/revoke", &browser, "{}");
    assert_eq!(
        call(addr, "POST", "/api/siri/command", Some(token), "{}").status,
        401
    );
    session.quit();
    records.empty();
}

#[test]
fn web_builtin_manage_agent_routes() {
    use agent_proto::{projects, store};
    use serde_json::json;
    use yinz_integration_tests::test_env::write_git_repository;

    let root = write_git_repository("web-launch", "trunk")
        .canonicalize()
        .unwrap();
    let detached = root.join("side");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["worktree", "add", "--detach", detached.to_str().unwrap()]);
    let dir = private_data();
    set(dir.path());
    let mut project = projects::validate("web-project", root.to_str().unwrap()).unwrap();
    project.worktree_dir = Some("branches".into());
    let data = yinz_storage::data_access::DataAccess::sqlite(dir.path());
    data.plugin_data()
        .define_entity(store::SHARED, "test", &store::project_def(), false)
        .unwrap();
    data.plugin_data()
        .apply(store::SHARED, &[projects::to_mutation(&project)])
        .unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let _shell = claim_first_terminal_and_wait_for_prompt(&session);
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    let token = login(addr);
    let choices = call_ok(
        addr,
        "GET",
        "/api/agent/launch-choices?project=web-project",
        &token,
        "",
    );
    assert!(choices["providers"]
        .as_array()
        .unwrap()
        .contains(&json!("codex")));
    assert!(choices["projects"]
        .as_array()
        .unwrap()
        .contains(&json!("web-project")));
    assert!(choices["worktrees"]
        .as_array()
        .unwrap()
        .contains(&json!({"path": detached, "branch": null})));
    let instance = choices["server_instance"].as_str().unwrap();
    let launch = |id: &str, checkout| {
        json!({"submission_id":format!("{instance}.{id}"),"kind":"codex","project":"web-project","checkout":checkout}).to_string()
    };
    let body = launch("first", json!({"type":"existing","path":detached}));
    assert_eq!(
        call(addr, "POST", "/api/agent/launch", None, &body).error_code(401),
        "unauthenticated"
    );
    let launched = call_ok(addr, "POST", "/api/agent/launch", &token, &body);
    let agent = session.expect_pty_spawn();
    assert_eq!(
        launched["pane_id"],
        yinz_types::pane_id::PaneId::Terminal(agent.terminal_id()).to_string()
    );
    let run = seed_retained_run(
        dir.path(),
        session.session_name(),
        agent.terminal_id().get(),
    );
    assert_eq!(agent.run_command().cwd, Some(detached.clone()));
    assert_eq!(
        git(&[
            "-C",
            detached.to_str().unwrap(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD"
        ])
        .trim(),
        "HEAD"
    );

    git(&["worktree", "remove", detached.to_str().unwrap()]);
    // Recovery reads the original receipt even though the worktree is gone.
    assert_eq!(
        call_ok(addr, "POST", "/api/agent/launch", &token, &body),
        launched
    );
    let missing = launch("missing", json!({"type":"existing","path":detached}));
    assert_eq!(
        call(addr, "POST", "/api/agent/launch", Some(&token), &missing).error_code(400),
        "invalid"
    );
    let new_checkout = json!({"type":"new","branch":"topic"});
    let conflict = launch("first", new_checkout.clone());
    assert_eq!(
        call(addr, "POST", "/api/agent/launch", Some(&token), &conflict).error_code(409),
        "launch_conflict"
    );
    let new_body = launch("second", new_checkout);
    let created = call_ok(addr, "POST", "/api/agent/launch", &token, &new_body);
    assert_eq!(
        call_ok(addr, "POST", "/api/agent/launch", &token, &new_body),
        created
    );
    assert!(!root.join("branches/topic-2").exists());
    let second = session.expect_pty_spawn();
    assert_eq!(second.run_command().cwd, Some(root.join("branches/topic")));
    assert_eq!(git(&["rev-parse", "topic"]), git(&["rev-parse", "trunk"]));

    report_codex_session_start(&session, &agent);
    let list = || call_ok(addr, "GET", "/api/agents", &token, "");
    let rows = list();
    let row = rows["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["pane_id"] == launched["pane_id"])
        .unwrap();
    let reference = &row["ref"];
    assert_eq!(row["run"], run.id, "{row}");
    let history = |agent: Option<&str>| {
        let mut params = vec![("run", run.id.as_str())];
        params.extend(agent.map(|id| ("agent", id)));
        call_ok(addr, "GET", &query("/api/history", &params), &token, "")
    };
    let live_history = history(Some(row["id"].as_str().unwrap()));
    assert_eq!(live_history["run"], run.id, "{live_history}");
    assert_eq!(&live_history["live"]["ref"], reference, "{live_history}");
    assert_eq!(live_history["events"][0]["markdown"], run.entries[0].text);
    let target = json!({"agent":reference}).to_string();
    let status = |submission_id: &str, agent: &serde_json::Value| {
        call(
            addr,
            "POST",
            "/api/agent/launch-status",
            Some(&token),
            &json!({"submission_id":submission_id, "agent":agent}).to_string(),
        )
    };
    let launch_id = format!("{instance}.first");
    let verified = status(&launch_id, reference);
    assert_eq!(verified.status, 200);
    assert_eq!(
        verified.json(),
        json!({"session":launched["session"],"pane_id":launched["pane_id"],"agent":reference})
    );
    assert_eq!(
        status("retired.first", reference).error_code(409),
        "target_changed"
    );
    assert_eq!(
        status(&format!("{instance}.second"), reference).error_code(409),
        "target_changed"
    );
    let mut replacement = reference.clone();
    replacement["session_id"] = json!("replacement");
    assert_eq!(
        status(&launch_id, &replacement).error_code(409),
        "target_changed"
    );
    let inventory = call_ok(addr, "POST", "/api/agent/destinations", &token, &target);
    assert_eq!(inventory["current_tab"], row["location"]["tab"]["id"]);
    let moved = call_ok(addr, "POST", "/api/agent/move", &token,
        &json!({"agent":reference,"destination":{"type":"new_group","name":"Review","tab_name":"First"}}).to_string());
    assert_eq!(moved["ok"], true);
    let rows = list();
    let moved = rows["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| &a["ref"] == reference)
        .unwrap();
    assert_eq!(moved["pane_id"], launched["pane_id"]);
    assert_ne!(moved["location"]["group"]["id"], inventory["current_group"]);
    let moved_tab = &moved["location"]["tab"];
    assert_eq!(
        moved_tab["name"],
        format!("[{}] First", agent.terminal_id())
    );
    // Close requires a settled source. The destination inventory lists the tab once
    // its background layout lands, with the same displayed name as the live location.
    wait_http("moved agent tab available for management", || {
        let inventory = call_ok(addr, "POST", "/api/agent/destinations", &token, &target);
        inventory["groups"].as_array().unwrap().iter().any(|group| {
            group["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tab| tab["id"] == moved_tab["id"] && tab["name"] == moved_tab["name"])
        })
    });

    call_ok(addr, "POST", "/api/agent/close", &token, &target);
    assert!(list()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["current_session"] != true || a["pane_id"] != launched["pane_id"]));
    assert_eq!(
        call(addr, "POST", "/api/agent/close", Some(&token), &target).error_code(409),
        "target_changed"
    );
    let archived = history(None);
    assert_eq!(archived["run"], run.id, "{archived}");
    assert_eq!(archived["events"][0]["markdown"], run.entries[0].text);
    session.quit();
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
        .recv_timeout(Duration::from_secs(600))
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

fn startup_recovery(method: &str, path: &str, unpublished: bool) {
    use yinz_types::web::ApplicationPhase as Phase;
    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let entry = dir.path().join("plugins/builtins/web.scm");
    let healthy = std::fs::read(&entry).unwrap();
    std::fs::write(&entry, if unpublished { "(define (load config) #f)" } else { r#"(define (load config) (yinz-command (hash "HttpPublish" (hash "path" "/api/private" "content_type" "text/plain; charset=utf-8" "body" "private startup fixture /secret/path token"))))"# }).unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let addr = records.serving(session.session_name());
    records.values.wait_until(
        "first configured application failure",
        Duration::from_secs(90),
        |rows| {
            rows.iter().find_map(|r| {
                r.application
                    .as_ref()
                    .filter(|s| s.phase == Phase::Retrying)
                    .cloned()
            })
        },
    );
    let first = fetch(addr, method, path, "");
    assert_eq!(first.status, 503, "{first}");
    assert!(first.header("retry-after").is_some());
    if method == "HEAD" {
        assert!(first.body.is_empty());
    } else {
        assert!(first.text().contains("http-equiv=\"refresh\""));
        assert!(!first.text().contains("secret"));
    }
    std::fs::write(&entry, healthy).unwrap();
    // Recovery traffic is deliberately only this public navigation method/path. No login,
    // authenticated API, plugin reload, or direct activation supplies the retry signal.
    let done = Watched::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = std::time::Instant::now() + default_timeout();
            while std::time::Instant::now() < deadline {
                let reply = fetch(addr, method, path, "");
                if reply.status == 200 {
                    *done.lock() = true;
                    return;
                }
                assert_eq!(reply.status, 503, "{reply}");
                std::thread::park_timeout(Duration::from_millis(20));
            }
        });
        done.wait_until(
            "public-only configured app recovery",
            default_timeout(),
            |done| done.then_some(()),
        );
    });
    assert_builtin_site(addr);
    let status = records.values.wait_until(
        "recovered application readiness",
        default_timeout(),
        |rows| {
            rows.iter()
                .find_map(|r| r.application.as_ref().filter(|s| s.ready()).cloned())
        },
    );
    assert!((2..=4).contains(&status.attempts), "{status:?}");
    assert!(status.failure.is_none());
    assert_eq!(call(addr, "GET", "/api/agents", None, "").status, 401);
    let token = login(addr);
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(&token), "").status,
        200
    );
    session.quit();
    records.empty();
}
#[test]
fn web_startup_get_root_recovers() {
    startup_recovery("GET", "/", false);
}
#[test]
fn web_startup_get_login_recovers() {
    startup_recovery("GET", "/login", false);
}
#[test]
fn web_startup_head_only_recovers() {
    startup_recovery("HEAD", "/", false);
}
#[test]
fn web_startup_unpublished_deadline_recovers() {
    startup_recovery("GET", "/login", true);
}

#[test]
fn web_startup_exhaustion_stays_terminal_under_public_polling() {
    use yinz_types::web::ApplicationPhase as Phase;
    let dir = private_data();
    set(dir.path());
    std::fs::write(
        dir.path().join("plugins/builtins/web.scm"),
        "(define (load config) (error \"permanent private fixture\"))",
    )
    .unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let addr = records.serving(session.session_name());
    let failed = std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = std::time::Instant::now() + Duration::from_secs(50);
            while std::time::Instant::now() < deadline {
                assert_eq!(fetch(addr, "HEAD", "/login", "").status, 503);
                if records.values.lock().iter().any(|r| {
                    r.application
                        .as_ref()
                        .is_some_and(|s| s.phase == Phase::Failed)
                }) {
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(20));
            }
        });
        records.values.wait_until(
            "finite automatic retry budget exhausted",
            Duration::from_secs(50),
            |rows| {
                rows.iter().find_map(|r| {
                    r.application
                        .as_ref()
                        .filter(|s| s.phase == Phase::Failed)
                        .cloned()
                })
            },
        )
    });
    assert_eq!(failed.attempts, 4);
    assert!(failed.retry_after_s.is_none());
    for _ in 0..20 {
        let reply = fetch(addr, "GET", "/", "");
        assert_eq!(reply.status, 503);
        assert!(reply.text().contains("Yinz web is unavailable"));
        assert!(!reply.text().contains("http-equiv=\"refresh\""));
        assert!(!reply.text().contains("private"));
    }
    assert_eq!(fetch(addr, "GET", "/healthz", "").status, 200);
    session.quit();
    records.empty();
}

#[test]
fn web_startup_recovery_preserves_existing_bearer_and_push_subscription() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use yinz_types::{
        cli::{PluginOperation, PluginRequest},
        web::ApplicationPhase as Phase,
    };
    let dir = private_data();
    set(dir.path());
    let entry = dir.path().join("plugins/builtins/web.scm");
    let healthy = std::fs::read(&entry).unwrap();
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; }")
        .start();
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    let ready = records
        .values
        .wait_until("initial ready application", default_timeout(), |rows| {
            rows.iter()
                .find_map(|r| r.application.as_ref().filter(|s| s.ready()).cloned())
        });
    let token = login(addr);
    let mut key = [0; 65];
    key[0] = 4;
    let push = serde_json::json!({"endpoint":"https://fcm.googleapis.com/yinz-141", "keys":{"p256dh":URL_SAFE_NO_PAD.encode(key), "auth":URL_SAFE_NO_PAD.encode([0;16])}}).to_string();
    assert_eq!(
        call(addr, "POST", "/api/push", Some(&token), &push).status,
        200
    );
    let before = std::fs::read(dir.path().join("web/push.json")).unwrap();
    std::fs::write(&entry, r#"(define (load config) (yinz-command (hash "HttpPublish" (hash "path" "/api/private" "content_type" "text/plain; charset=utf-8" "body" "private fixture"))))"#).unwrap();
    // Explicit reload induces the failure, after creating the sign-in. It does not repair it.
    yinz_client::store_client::plugin_request(
        &session.session_name().into(),
        PluginRequest {
            operation: PluginOperation::Reload,
            target: Some(format!("plugin_{}", ready.target.unwrap())),
            configuration: None,
            fallback: None,
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap();
    records.values.wait_until(
        "publication failure after reload",
        default_timeout(),
        |rows| {
            rows.iter()
                .any(|r| {
                    r.application
                        .as_ref()
                        .is_some_and(|s| s.phase == Phase::Retrying)
                })
                .then_some(())
        },
    );
    std::fs::write(&entry, healthy).unwrap();
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    assert_builtin_site(addr);
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(&token), "").status,
        200
    );
    assert_eq!(
        std::fs::read(dir.path().join("web/push.json")).unwrap(),
        before
    );
    session.quit();
    records.empty();
}

fn configured_operation(
    session: &yinz_integration_tests::TestSession,
    operation: yinz_types::cli::PluginOperation,
    target: &str,
) {
    yinz_client::store_client::plugin_request(
        &session.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation,
            target: Some(target.into()),
            configuration: None,
            fallback: yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                target, &None, None, None,
            )
            .unwrap()
            .get_run_plugin(),
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap();
}

#[test]
fn web_startup_peer_two_clients_reload_keeps_site_and_bearer() {
    use yinz_types::cli::PluginOperation;
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; plugin \"yinz:web\"; }")
        .start();
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    assert_builtin_site(addr);
    let token = login(addr);
    let peer = session.attach_client(TERMINAL_SIZE);
    peer.wait_until("second terminal client attached", |g| g.tab_bar_appears());
    let initial = records.values.wait_until(
        "ready before two-client reload",
        default_timeout(),
        |rows| {
            rows.iter()
                .find_map(|r| r.application.as_ref().filter(|s| s.ready()).cloned())
        },
    );
    configured_operation(&session, PluginOperation::Reload, "yinz:web");
    let ready = records.values.wait_until(
        "two-client reload and rejected peer complete",
        default_timeout(),
        |rows| {
            rows.iter().find_map(|r| {
                r.application
                    .as_ref()
                    .filter(|s| {
                        s.ready()
                            && s.generation != initial.generation
                            && s.publication_rejection.is_some()
                    })
                    .cloned()
            })
        },
    );
    assert_eq!(ready.target, initial.target);
    assert_eq!(ready.attempts, 1);
    assert_builtin_site(addr);
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(&token), "").status,
        200
    );
    peer.quit();
    session.quit();
    records.empty();
}

#[test]
fn web_startup_stopped_app_requires_explicit_start_and_preserves_bearer() {
    use yinz_types::{cli::PluginOperation, web::ApplicationPhase as Phase};
    let dir = private_data();
    set(dir.path());
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config("web { enabled true; port 0; plugin \"yinz:web\"; }")
        .start();
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    let token = login(addr);
    configured_operation(&session, PluginOperation::Stop, "yinz:web");
    let stopped = records
        .values
        .wait_until("configured app stopped", default_timeout(), |rows| {
            rows.iter().find_map(|r| {
                r.application
                    .as_ref()
                    .filter(|s| s.phase == Phase::Stopped)
                    .cloned()
            })
        });
    for path in ["/", "/login"] {
        let reply = fetch(addr, "GET", path, "");
        assert_eq!(reply.status, 503);
        assert!(!reply.text().contains("http-equiv=\"refresh\""));
        assert!(reply.text().contains("Yinz web is unavailable"));
    }
    // Request completion is a plugin-thread round trip after stop and its cleanup.
    configured_operation(&session, PluginOperation::List, "yinz:web");
    assert!(stopped.retry_after_s.is_none());
    configured_operation(&session, PluginOperation::Start, "yinz:web");
    wait_public(addr, "/", r#"<div id="app"></div>"#);
    assert_builtin_site(addr);
    assert_eq!(
        call(addr, "GET", "/api/agents", Some(&token), "").status,
        200
    );
    session.quit();
    records.empty();
}

/// A disposable nonshared app. The second client identifies itself by its own timezone,
/// avoiding assumptions about numeric client IDs allocated to native CLI connections.
fn permission_app(dir: &Path, selected_denial: bool) -> String {
    yinz_integration_tests::test_env::init();
    let root = dir.join("permission-app");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("static")).unwrap();
    std::fs::write(
        root.join("static/app.js"),
        "export const selected = 'permission-app';",
    )
    .unwrap();
    let source = root.join("app.scm");
    let body = r#"
(define (publish)
  (yinz-command (hash "HttpAssets" (hash "prefix" "/static/" "directory" "static")))
  (yinz-command (hash "HttpPublish" (hash "path" "/" "content_type" "text/html; charset=utf-8" "body" "<main>selected permission app</main><script type=\"module\" src=\"/static/app.js\"></script>")))
  (yinz-command (hash "HttpPublish" (hash "path" "/login" "content_type" "text/html; charset=utf-8" "body" "<main>selected permission login</main>"))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ChangeApplicationState")) (lambda (_)
    (if PEER
      (yinz-catch (yinz-then (yinz-wait-for-permissions '("ReadFiles"))
        (lambda (_) (yinz-command (hash "RenameTab" '(0 "permission-granted")))))
        (lambda (_) (yinz-command (hash "RenameTab" '(0 "permission-denied")))))
      (publish)))))
(define (yinz-web-dispatch request)
  (yinz-command (hash "HttpRespond" (hash "request_id" (hash-ref request "id")
    "response" (yinz-json-string (hash "status" 200 "headers" (hash "content-type" "text/plain") "body" "selected authenticated app"))))))
"#;
    std::fs::write(
        &source,
        body.replace(
            "PEER",
            if selected_denial {
                "#t"
            } else {
                "(equal? (yinz-client-timezone) \"Asia/Kolkata\")"
            },
        ),
    )
    .unwrap();
    let url = format!("file:{}", source.display());
    // This process's xtask test environment owns its cache. Only prerequisites are granted;
    // ReadFiles is deliberately absent for this unique source identity.
    let cache_path = yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        yinz_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        yinz_runtime::plugin_location::parse_plugin_location(&url, None)
            .unwrap()
            .to_string(),
        vec![
            yinz_types::data::PermissionType::RunCommands,
            yinz_types::data::PermissionType::ChangeApplicationState,
        ],
    );
    cache.write_to_file().unwrap();
    url
}

fn assert_permission_site(addr: SocketAddr) {
    for (path, marker) in [
        ("/", "selected permission app"),
        ("/login", "selected permission login"),
    ] {
        let reply = fetch(addr, "GET", path, "");
        assert_page(&reply);
        assert!(reply.text().contains(marker), "{reply}");
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        let head = fetch(addr, "HEAD", path, "");
        assert_page(&head);
        assert!(head.body.is_empty());
    }
    let asset = fetch(addr, "GET", "/static/app.js", "");
    assert_eq!(asset.status, 200);
    assert_eq!(asset.text(), "export const selected = 'permission-app';");
}

#[test]
fn web_startup_permission_peer_modal_denial_keeps_public_site_bearer_and_push() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let dir = private_data();
    set(dir.path());
    let url = permission_app(dir.path(), false);
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_timezone(Some("America/Chicago"))
        .with_config(&format!(
            "mirror_session false\nweb {{ enabled true; port 0; plugin {url:?}; }}"
        ))
        .start();
    let addr = records.serving(session.session_name());
    wait_public(addr, "/", "selected permission app");
    assert_permission_site(addr);
    let initial = records
        .values
        .wait_until("selected app ready", default_timeout(), |rows| {
            rows.iter()
                .find_map(|r| r.application.as_ref().filter(|s| s.ready()).cloned())
        });
    let token = login(addr);
    let mut key = [0; 65];
    key[0] = 4;
    let push = serde_json::json!({"endpoint":"https://fcm.googleapis.com/yinz-141-permission", "keys":{"p256dh":URL_SAFE_NO_PAD.encode(key), "auth":URL_SAFE_NO_PAD.encode([0;16])}}).to_string();
    assert_eq!(
        call(addr, "POST", "/api/push", Some(&token), &push).status,
        200
    );
    let subscription = std::fs::read(dir.path().join("web/push.json")).unwrap();
    let peer = session.attach_client_with_timezone(TERMINAL_SIZE, Some("Asia/Kolkata"));
    peer.wait_until("peer-only extra permission modal", |g| {
        g.contains("Plugin permissions") && g.contains("ReadFiles")
    });
    // Public-only observations while the genuine peer prompt remains unanswered.
    assert_permission_site(addr);
    peer.send_stdin(b"\x1b");
    peer.wait_until("peer deferred handled denial", |g| {
        g.contains("permission-denied") && !g.contains("Plugin permissions")
    });
    assert_permission_site(addr);
    let after = records.values.wait_until(
        "same selected publication after peer denial",
        default_timeout(),
        |rows| {
            rows.iter().find_map(|r| {
                r.application
                    .as_ref()
                    .filter(|s| s.ready() && s.generation == initial.generation)
                    .cloned()
            })
        },
    );
    assert_eq!(
        (after.target, after.load, after.attempts, after.failure),
        (initial.target, initial.load, initial.attempts, None)
    );
    assert_eq!(
        call(addr, "GET", "/api/session", Some(&token), "").status,
        200
    );
    let authenticated = call(addr, "GET", "/api/probe", Some(&token), "");
    assert_eq!(authenticated.status, 200);
    assert_eq!(authenticated.text(), "selected authenticated app");
    assert_eq!(
        std::fs::read(dir.path().join("web/push.json")).unwrap(),
        subscription
    );
    peer.quit();
    session.quit();
    records.empty();
}

#[test]
fn web_startup_permission_selected_modal_denial_remains_terminal_and_private() {
    use yinz_types::web::{ApplicationFailure, ApplicationPhase};
    let dir = private_data();
    set(dir.path());
    let url = permission_app(dir.path(), true);
    let records = Records::new(dir.path().to_owned());
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_data_dir(dir.path().to_owned())
        .with_config(&format!("web {{ enabled true; port 0; plugin {url:?}; }}"))
        .start();
    let addr = records.serving(session.session_name());
    session.wait_until("selected owner permission modal", |g| {
        g.contains("Plugin permissions") && g.contains("ReadFiles")
    });
    session.send_stdin(b"\x1b");
    session.wait_until("selected initializer caught denial", |g| {
        g.contains("permission-denied")
    });
    let failed =
        records
            .values
            .wait_until("selected permission failure", default_timeout(), |rows| {
                rows.iter().find_map(|r| {
                    r.application
                        .as_ref()
                        .filter(|s| s.phase == ApplicationPhase::Failed)
                        .cloned()
                })
            });
    assert_eq!(failed.failure, Some(ApplicationFailure::PermissionDenied));
    assert_eq!(failed.retry_after_s, None);
    assert_eq!(failed.attempts, 1);
    for method in ["GET", "HEAD"] {
        for path in ["/", "/login"] {
            let reply = fetch(addr, method, path, "");
            assert_eq!(reply.status, 503);
            assert!(!reply.text().contains("http-equiv=\"refresh\""));
            assert!(!reply.text().contains("ReadFiles"));
            assert!(!reply.text().contains(&dir.path().display().to_string()));
            if method == "HEAD" {
                assert!(reply.body.is_empty());
            }
        }
    }
    session.quit();
    records.empty();
}
