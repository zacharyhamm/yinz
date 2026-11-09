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
#[test]
fn web_agents_configured_steel_native_list_history_guarded_screen_and_detached_focus() {
    use agent_proto::{
        agents::{rows, AgentRun, RunEntry, RunEntryKind, RunOutcome},
        store,
    };
    use serde_json::json;
    use yinz_types::plugin_data::REV_ABSENT;
    yinz_integration_tests::test_env::init();
    let dir = private_data();
    set(dir.path());
    let data = yinz_storage::data_access::DataAccess::sqlite(dir.path());
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
    // Use the real report IPC contract; the in-process CLI action builder reads hook
    // JSON before the fake client's stdin exists.
    {
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
