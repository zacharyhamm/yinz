use super::*;
use crate::listener_file::ListenerRecord;
use crate::{Backend, DispatchFuture, WebServer, WebSettings};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        RwLock,
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yinz_test_support::Watched;
use yinz_types::web::{Site, StaticFile, WebRequest};

#[derive(Default)]
struct Fake {
    quiesced: AtomicBool,
    activations: AtomicUsize,
    dispatches: AtomicUsize,
    site: RwLock<Option<Arc<Site>>>,
    name: RwLock<String>,
    rename_on_activate: std::sync::Mutex<Option<String>>,
    requests: std::sync::Mutex<Vec<WebRequest>>,
}
impl Backend for Fake {
    fn dispatch(&self, request: WebRequest) -> DispatchFuture {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        Box::pin(async { None })
    }
    fn site(&self) -> Option<Arc<Site>> {
        self.site.read().unwrap().clone()
    }
    fn quiesced(&self) -> bool {
        self.quiesced.load(Ordering::Acquire)
    }
    fn activate(&self) {
        self.activations.fetch_add(1, Ordering::SeqCst);
        if let Some(name) = self.rename_on_activate.lock().unwrap().take() {
            *self.name.write().unwrap() = name;
        }
    }
    fn session_name(&self) -> String {
        self.name.read().unwrap().clone()
    }
}
fn settings(dir: &std::path::Path) -> WebSettings {
    let config = yinz_types::input::web::WebConfig {
        port: Some(0),
        ..Default::default()
    };
    let mut settings = WebSettings::from_config(&config, dir).unwrap();
    settings.retry_interval = Duration::from_millis(50);
    settings.header_timeout = Duration::from_secs(3);
    settings
}
async fn state(server: &WebServer, wanted: impl Fn(&WebStatus) -> bool) -> WebStatus {
    let mut receiver = server.status_receiver();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = receiver.borrow_and_update().clone();
            if wanted(&status) {
                return status;
            }
            receiver.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("web state: {:?}", server.status()))
}
async fn serving(server: &WebServer) -> SocketAddr {
    match state(server, |s| matches!(s, WebStatus::Serving { .. })).await {
        WebStatus::Serving { addr } => addr,
        _ => unreachable!(),
    }
}
async fn stop(server: &WebServer) {
    server.stop();
    state(server, |s| *s == WebStatus::Stopped).await;
}
async fn request(addr: SocketAddr, text: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(text.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        String::from_utf8(bytes).unwrap()
    })
    .await
    .expect("HTTP response")
}
async fn get(addr: SocketAddr, path: &str) -> String {
    request(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    )
    .await
}
// Enumeration is not a directory-wide snapshot: publication and cleanup may run between
// read_dir and read. Only completed final files are records; empty reservations still own paths.
#[derive(Debug, Default)]
struct RecordObservation {
    records: Vec<ListenerRecord>,
    pending: Vec<PathBuf>,
}
impl RecordObservation {
    fn is_empty(&self) -> bool {
        self.records.is_empty() && self.pending.is_empty()
    }
}
fn records(dir: &Path) -> Result<RecordObservation> {
    observe_records(dir, |path| std::fs::read(path))
}
fn observe_records(
    dir: &Path,
    mut read: impl FnMut(&Path) -> std::io::Result<Vec<u8>>,
) -> Result<RecordObservation> {
    let directory = dir.join("web/listeners");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecordObservation::default());
        },
        Err(error) => {
            return Err(error).with_context(|| format!("enumerate {}", directory.display()))
        },
    };
    let mut observation = RecordObservation::default();
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", directory.display()))?;
        let name = entry.file_name();
        let Some((pid, suffix)) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|name| name.split_once('-'))
        else {
            continue;
        };
        if ![pid, suffix]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let path = entry.path();
        let bytes = match read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        if bytes.is_empty() {
            observation.pending.push(path);
        } else {
            observation.records.push(
                serde_json::from_slice(&bytes).with_context(|| {
                    format!("parse published listener record {}", path.display())
                })?,
            );
        }
    }
    Ok(observation)
}
async fn wait_for_record(
    dir: &Path,
    address: SocketAddr,
    session: String,
    instance: Option<String>,
) -> ListenerRecord {
    let what = format!("published listener {address}, session {session:?}, instance {instance:?}");
    let observed = Watched::new(None);
    let updates = observed.clone();
    let dir = dir.to_owned();
    let cancel = CancellationToken::new();
    let cancelled = cancel.clone();
    let observer = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                biased;
                _ = cancelled.cancelled() => break,
                _ = tick.tick() => {},
            }
            let dir = dir.clone();
            let result = tokio::task::spawn_blocking(move || records(&dir))
                .await
                .context("join listener record observation")
                .and_then(|result| result)
                .map_err(|error| format!("{error:#}"));
            let failed = result.is_err();
            *updates.lock() = Some(result);
            if failed {
                break;
            }
        }
    });
    // Watched waits block a thread; keep this test's current-thread runtime free to serve.
    let waiter = tokio::task::spawn_blocking(move || {
        observed.wait_until(&what, Duration::from_secs(5), |snapshot| {
            match snapshot.as_ref()? {
                Err(error) => Some(Err(error.clone())),
                Ok(snapshot) => snapshot
                    .records
                    .iter()
                    .find(|record| {
                        record.address == address
                            && record.session == session
                            && instance
                                .as_ref()
                                .is_none_or(|instance| record.instance == *instance)
                    })
                    .cloned()
                    .map(Ok),
            }
        })
    })
    .await;
    // Even a timeout panic must stop and join the observer before it reaches fixture cleanup.
    cancel.cancel();
    observer.await.expect("join listener record observer");
    waiter
        .expect("listener record wait")
        .expect("observe listener records")
}
fn assert_no_records(dir: &Path) {
    assert!(records(dir).expect("observe listener cleanup").is_empty());
    // Stopped means publication and cleanup workers have joined, including staging files.
    let directory = dir.join("web/listeners");
    assert_eq!(
        std::fs::read_dir(directory).unwrap().count(),
        0,
        "listener staging files remain"
    );
}

fn observation_fixture() -> (tempfile::TempDir, ListenerRecord, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("web/listeners");
    std::fs::create_dir_all(&directory).unwrap();
    let record = ListenerRecord {
        startup_config: None,
        version: 1,
        pid: 123,
        instance: "instance".into(),
        address: "127.0.0.1:7419".parse().unwrap(),
        session: "observed".into(),
    };
    (temp, record, directory.join("123-456.json"))
}
#[test]
fn listener_record_observation_ignores_unpublished_staging_files() {
    let (temp, record, path) = observation_fixture();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    for (name, bytes) in [
        (".tmp-empty", ""),
        (".tmp-partial", "{\"version\":"),
        ("123-x.json", "invalid"),
        ("-456.json", "invalid"),
        ("123-.json", "invalid"),
    ] {
        std::fs::write(path.parent().unwrap().join(name), bytes).unwrap();
    }
    let observed = records(temp.path()).unwrap();
    assert!(observed.pending.is_empty());
    assert_eq!(observed.records.len(), 1);
    assert_eq!(observed.records[0].instance, record.instance);
    assert_eq!(observed.records[0].session, record.session);
    assert_eq!(observed.records[0].address, record.address);
}
#[tokio::test]
async fn listener_record_observation_tracks_pending_until_atomic_publication() {
    let (temp, record, path) = observation_fixture();
    std::fs::write(&path, []).unwrap();
    let pending = records(temp.path()).unwrap();
    assert!(pending.records.is_empty());
    assert_eq!(pending.pending, std::slice::from_ref(&path));
    assert!(
        !pending.is_empty(),
        "a reservation must prevent cleanup success"
    );
    crate::auth::password::atomic_write(
        path.parent().unwrap(),
        "123-456.json",
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let published = wait_for_record(
        temp.path(),
        record.address,
        record.session.clone(),
        Some(record.instance.clone()),
    )
    .await;
    assert_eq!(published.pid, record.pid);
    assert!(records(temp.path()).unwrap().pending.is_empty());
    std::fs::remove_file(path).unwrap();
    assert_no_records(temp.path());
}
#[test]
fn listener_record_observation_tolerates_removal_between_enumeration_and_read() {
    let (temp, record, path) = observation_fixture();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let mut read_count = 0;
    let observed = observe_records(temp.path(), |entry| {
        assert_eq!(entry, path);
        read_count += 1;
        std::fs::remove_file(entry)?;
        std::fs::read(entry)
    })
    .unwrap();
    assert_eq!(read_count, 1, "removal must occur after enumeration");
    assert!(observed.is_empty());
    std::fs::remove_dir(path.parent().unwrap()).unwrap();
    assert!(records(temp.path()).unwrap().is_empty());
}
#[test]
fn listener_record_observation_reports_malformed_publication_and_io_errors() {
    let (temp, _, path) = observation_fixture();
    std::fs::write(&path, b"{").unwrap();
    let error = records(temp.path()).unwrap_err();
    assert!(error.to_string().contains(&path.display().to_string()));
    assert!(format!("{error:#}").contains("EOF"));
    let error = observe_records(temp.path(), |_| {
        Err(std::io::ErrorKind::PermissionDenied.into())
    })
    .unwrap_err();
    assert!(error.to_string().contains(&path.display().to_string()));
    assert!(error.downcast_ref::<std::io::Error>().is_some());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(path.parent().unwrap()).unwrap();
    std::fs::write(path.parent().unwrap(), b"not a directory").unwrap();
    assert!(records(temp.path()).is_err());
}
fn fixture() -> (tempfile::TempDir, Arc<Fake>) {
    let temp = tempfile::tempdir().unwrap();
    crate::auth::password::tests::ready(temp.path());
    let backend = Arc::new(Fake::default());
    *backend.name.write().unwrap() = "<session & \"name\">".into();
    (temp, backend)
}

#[tokio::test]
async fn native_health_starting_security_and_no_dispatch() {
    let (temp, backend) = fixture();
    let server = WebServer::start(
        settings(temp.path()),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    let health = get(addr, "/healthz").await;
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(health.contains("\"ok\":true"));
    assert!(health.contains("x-frame-options: DENY"));
    assert!(health.contains("cache-control: no-store"));
    let proxied = request(addr, "GET /healthz HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 100.64.0.1\r\nConnection: close\r\n\r\n").await;
    assert!(!proxied.contains("\"session\""));
    let starting = get(addr, "/").await;
    assert!(starting.starts_with("HTTP/1.1 503"));
    assert!(starting.contains("retry-after: 1"));
    assert!(starting.contains("&lt;session &amp; &quot;name&quot;&gt;"));
    for path in ["/api/session", "/api/agents"] {
        let reply = request(addr, &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer forged\r\nAuthorization: Bearer another\r\nConnection: close\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 401"));
        assert!(reply.contains("www-authenticate: Bearer"));
    }
    let reply = request(addr, "POST /api/login HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
    assert!(reply.starts_with("HTTP/1.1 400"));
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    wait_for_record(temp.path(), addr, backend.session_name(), None).await;
    stop(&server).await;
    assert_no_records(temp.path());
}

#[tokio::test]
async fn frozen_site_etags_head_and_parallel_requests() {
    let (temp, backend) = fixture();
    let file = StaticFile {
        bytes: Arc::from(b"hello world".as_slice()),
        content_type: "text/plain",
        etag: "\"test\"".into(),
    };
    let mut site = Site::default();
    site.pages.insert("/".into(), file.clone());
    site.assets
        .insert("/static/".into(), [("test.txt".into(), file)].into());
    *backend.site.write().unwrap() = Some(Arc::new(site));
    let server = WebServer::start(
        settings(temp.path()),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    let mut parallel = JoinSet::new();
    for _ in 0..16 {
        parallel.spawn(async move {
            assert!(get(addr, "/static/test.txt").await.ends_with("hello world"));
        });
    }
    while let Some(result) = parallel.join_next().await {
        result.unwrap();
    }
    for etag in ["*", "W/\"test\"", "\"other\", \"test\""] {
        let reply = request(addr, &format!("GET / HTTP/1.1\r\nHost: localhost\r\nIf-None-Match: {etag}\r\nConnection: close\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 304"));
        assert!(reply.ends_with("\r\n\r\n"));
        assert!(reply.contains("cache-control: no-cache"));
    }
    for (path, status) in [
        ("/", 200),
        ("/healthz", 200),
        ("/missing", 404),
        ("/api/session", 405),
        ("/%2f", 400),
    ] {
        let reply = request(
            addr,
            &format!("HEAD {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
        assert!(reply.ends_with("\r\n\r\n"));
    }
    assert!(get(addr, "/missing").await.starts_with("HTTP/1.1 404"));
    stop(&server).await;
}

#[tokio::test]
async fn policy_and_bodies_apply_to_every_route() {
    let (temp, backend) = fixture();
    let server = WebServer::start(
        settings(temp.path()),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    for path in ["/healthz", "/", "/api/login", "/api/agents"] {
        for (headers, body, status) in [
            ("Content-Length: 2\r\nContent-Type: application/json\r\n", "{}", 403),
            ("Origin: http://localhost\r\nContent-Length: 2\r\n", "{}", 415),
            ("Origin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: 65537\r\n", "", 413),
        ] {
            let reply = request(addr, &format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n{body}")).await;
            assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
            assert!(reply.contains("content-security-policy:"));
        }
        let reply = request(addr, &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{{}}GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 400"));
        assert_eq!(reply.matches("HTTP/1.1").count(), 1);
    }
    for (line, headers, status) in [
        ("OPTIONS /healthz", "Host: localhost\r\n", 405),
        ("GET /healthz", "Host: evil.example\r\n", 421),
        (
            "GET /healthz",
            "Host: localhost\r\nTailscale-Funnel-Request: true\r\n",
            403,
        ),
        ("GET /%2F", "Host: localhost\r\n", 400),
        ("GET /?q=%ff", "Host: localhost\r\n", 400),
    ] {
        let reply = request(
            addr,
            &format!("{line} HTTP/1.1\r\n{headers}Connection: close\r\n\r\n"),
        )
        .await;
        assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
    }
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    stop(&server).await;
}

#[tokio::test]
async fn password_loss_quiesce_rebind_rename_and_stop() {
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(Fake::default());
    let server = WebServer::start(
        settings(temp.path()),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    state(&server, |s| *s == WebStatus::WaitingForPassword).await;
    let dir = temp.path().to_owned();
    tokio::task::spawn_blocking(move || crate::auth::password::tests::ready(&dir))
        .await
        .unwrap();
    let addr = serving(&server).await;
    let mut keepalive = TcpStream::connect(addr).await.unwrap();
    keepalive
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = [0; 4096];
    let len = keepalive.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..len]).contains("200"));
    backend.quiesced.store(true, Ordering::Release);
    state(&server, |s| *s == WebStatus::Quiesced).await;
    assert!(TcpStream::connect(addr).await.is_err());
    tokio::time::timeout(
        Duration::from_secs(5),
        keepalive.read_to_end(&mut Vec::new()),
    )
    .await
    .unwrap()
    .unwrap();
    backend.quiesced.store(false, Ordering::Release);
    let addr = serving(&server).await;
    let original = wait_for_record(temp.path(), addr, backend.session_name(), None).await;
    *backend.name.write().unwrap() = "renamed".into();
    assert!(get(addr, "/healthz").await.contains("renamed"));
    wait_for_record(temp.path(), addr, "renamed".into(), Some(original.instance)).await;
    crate::auth::password::clear(temp.path()).unwrap();
    state(&server, |s| *s == WebStatus::WaitingForPassword).await;
    assert!(TcpStream::connect(addr).await.is_err());
    assert!(backend.activations.load(Ordering::SeqCst) >= 2);
    stop(&server).await;
    assert_no_records(temp.path());
}

#[tokio::test]
async fn port_takeover_and_idle_early_reset() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    config.header_timeout = Duration::from_millis(300);
    let holder = WebServer::start(
        config.clone(),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&holder).await;
    config.bind = addr;
    let waiter = WebServer::start(config, backend, &tokio::runtime::Handle::current());
    state(&waiter, |s| matches!(s, WebStatus::Binding { .. })).await;
    let mut idle = TcpStream::connect(addr).await.unwrap();
    let reset = TcpStream::connect(addr).await.unwrap();
    drop(reset);
    assert!(get(addr, "/healthz").await.starts_with("HTTP/1.1 200"));
    tokio::time::timeout(Duration::from_secs(5), idle.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    let started = std::time::Instant::now();
    holder.stop();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(serving(&waiter).await, addr);
    state(&holder, |s| *s == WebStatus::Stopped).await;
    assert!(get(addr, "/healthz").await.starts_with("HTTP/1.1 200"));
    stop(&waiter).await;
    assert_no_records(temp.path());
}

#[tokio::test]
async fn connection_and_header_limits_and_parser_errors() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    config.max_connections = 2;
    let server = WebServer::start(config, backend.clone(), &tokio::runtime::Handle::current());
    let addr = serving(&server).await;
    let mut first = TcpStream::connect(addr).await.unwrap();
    let mut second = TcpStream::connect(addr).await.unwrap();
    // A completed request on each proves both permits have actually been acquired.
    for stream in [&mut first, &mut second] {
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut buffer = [0; 4096];
        assert!(stream.read(&mut buffer).await.unwrap() > 0);
    }
    let mut third = TcpStream::connect(addr).await.unwrap();
    let mut b = [0; 1];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), third.read(&mut b))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    drop(first);
    drop(second);
    stop(&server).await;
    let server = WebServer::start(
        settings(temp.path()),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    for extra in [
        (0..65).map(|i| format!("x-{i}: a\r\n")).collect::<String>(),
        format!("x-large: {}\r\n", "a".repeat(17000)),
        "Content-Length: 1\r\nContent-Length: 2\r\n".into(),
    ] {
        let reply = request(
            addr,
            &format!(
                "GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}\r\n"
            ),
        )
        .await;
        assert!(
            reply.starts_with("HTTP/1.1 431") || reply.starts_with("HTTP/1.1 400"),
            "{reply}"
        );
    }
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    stop(&server).await;
}

async fn read_reply(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 1);
            bytes.push(byte[0]);
            if bytes.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8(bytes.clone()).unwrap();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        bytes.extend(body);
    })
    .await
    .expect("keep-alive response");
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn keepalive_repeated_requests_idle_expiry_and_drop() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    config.header_timeout = Duration::from_millis(300);
    let server = WebServer::start(config, backend.clone(), &tokio::runtime::Handle::current());
    let addr = serving(&server).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    for (path, status) in [("/healthz", 200), ("/", 503), ("/api/session", 401)] {
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_reply(&mut stream)
            .await
            .starts_with(&format!("HTTP/1.1 {status}")));
    }
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let mut status = server.status_receiver();
    drop(server);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if *status.borrow_and_update() == WebStatus::Stopped {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(TcpStream::connect(addr).await.is_err());
    assert_no_records(temp.path());
}

#[tokio::test]
async fn chunked_overflow_invalid_utf8_and_body_deadlines() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    config.header_timeout = Duration::from_secs(20);
    let server = WebServer::start(
        config.clone(),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    for (body, status) in [
        (b"1\r\n\xff\r\n0\r\n\r\n".to_vec(), 400),
        (
            format!("10001\r\n{}\r\n0\r\n\r\n", "x".repeat(65537)).into_bytes(),
            413,
        ),
    ] {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(b"POST /healthz HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
        stream.write_all(&body).await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
    }
    let mut slow = TcpStream::connect(addr).await.unwrap();
    slow.write_all(b"POST /api/login HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{").await.unwrap();
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(15), slow.read_to_string(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    stop(&server).await;
    config.post_timeout = Duration::from_millis(100);
    let server = WebServer::start(config, backend.clone(), &tokio::runtime::Handle::current());
    let addr = serving(&server).await;
    let reply = request(addr, "POST / HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{").await;
    assert!(reply.starts_with("HTTP/1.1 504"), "{reply}");
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    stop(&server).await;
}

#[tokio::test]
async fn header_deadline_and_listener_publication_failure_are_fail_closed() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    config.header_timeout = Duration::from_millis(100);
    let server = WebServer::start(
        config.clone(),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    let mut slow = TcpStream::connect(addr).await.unwrap();
    slow.write_all(b"GET /healthz HTTP/1.1\r\nHost:")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), slow.read_to_end(&mut Vec::new()))
        .await
        .unwrap()
        .unwrap();
    stop(&server).await;
    std::fs::remove_dir(temp.path().join("web/listeners")).unwrap();
    std::os::unix::fs::symlink("missing", temp.path().join("web/listeners")).unwrap();
    config.bind = addr;
    let server = WebServer::start(config, backend, &tokio::runtime::Handle::current());
    state(&server, |s| matches!(s, WebStatus::Binding { .. })).await;
    assert!(TcpStream::connect(addr).await.is_err());
    stop(&server).await;
}

#[tokio::test]
async fn rename_during_activation_refreshes_initial_record() {
    let (temp, backend) = fixture();
    *backend.rename_on_activate.lock().unwrap() = Some("renamed during bind".into());
    let server = WebServer::start(
        settings(temp.path()),
        backend,
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&server).await;
    wait_for_record(temp.path(), addr, "renamed during bind".into(), None).await;
    stop(&server).await;
    assert_no_records(temp.path());
}

async fn api(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> String {
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    request(addr,&format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())).await
}
async fn login(addr: SocketAddr) -> String {
    let reply = api(
        addr,
        "POST",
        "/api/login",
        None,
        r#"{"password":"twelve words long","label":"Phone"}"#,
    )
    .await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    serde_json::from_str::<serde_json::Value>(reply.split_once("\r\n\r\n").unwrap().1).unwrap()
        ["token"]
        .as_str()
        .unwrap()
        .into()
}
#[tokio::test]
async fn native_bearer_lifecycle_cross_server_revocation_takeover_and_dispatch_secrecy() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    let a = WebServer::start(
        config.clone(),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let addr = serving(&a).await;
    let token = login(addr).await;
    let r = api(addr, "GET", "/api/session", Some(&token), "").await;
    assert!(r.starts_with("HTTP/1.1 200"));
    assert!(r.contains("\"label\":\"Phone\""));
    assert!(r.contains("idle_expires_at_ms"));
    let r=request(addr,&format!("GET /api/agents?filter=running HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nCookie: secret-cookie\r\nTailscale-User-Login: private-login\r\nAccept: application/json\r\nConnection: close\r\n\r\n")).await;
    assert!(r.starts_with("HTTP/1.1 503"));
    let forwarded = backend.requests.lock().unwrap()[0].clone();
    assert_eq!(forwarded.auth.label, "Phone");
    assert_eq!(forwarded.query, vec![("filter".into(), "running".into())]);
    assert_eq!(
        forwarded.headers,
        vec![("accept".into(), "application/json".into())]
    );
    assert!(forwarded.body.is_none());
    assert!(!format!("{forwarded:?}").contains(&token));
    let b = WebServer::start(
        config.clone(),
        backend.clone(),
        &tokio::runtime::Handle::current(),
    );
    let other = serving(&b).await;
    assert!(api(other, "GET", "/api/session", Some(&token), "")
        .await
        .starts_with("HTTP/1.1 200"));
    assert!(api(other, "POST", "/api/logout", Some(&token), "{}")
        .await
        .ends_with("\r\n\r\n"));
    assert!(api(addr, "GET", "/api/session", Some(&token), "")
        .await
        .starts_with("HTTP/1.1 401"));
    let revoked = login(addr).await;
    let other_token = login(other).await;
    let r = api(addr, "POST", "/api/logout-all", Some(&revoked), "{}").await;
    assert!(r.starts_with("HTTP/1.1 204"));
    assert!(r.ends_with("\r\n\r\n"));
    assert!(api(other, "GET", "/api/session", Some(&other_token), "")
        .await
        .starts_with("HTTP/1.1 401"));
    let rotated = login(addr).await;
    crate::auth::password::revoke_sessions(temp.path()).unwrap();
    assert!(api(other, "GET", "/api/session", Some(&rotated), "")
        .await
        .starts_with("HTTP/1.1 401"));
    let survivor = login(addr).await;
    stop(&a).await;
    config.bind = addr;
    let replacement = WebServer::start(config, backend.clone(), &tokio::runtime::Handle::current());
    assert_eq!(serving(&replacement).await, addr);
    assert!(api(addr, "GET", "/api/session", Some(&survivor), "")
        .await
        .starts_with("HTTP/1.1 200"));
    assert!(api(addr, "GET", "/api/session", Some(&revoked), "")
        .await
        .starts_with("HTTP/1.1 401"));
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 1);
    stop(&b).await;
    stop(&replacement).await;
}
#[tokio::test]
async fn actual_status_probe_identifies_listener_with_startup_allowlist_and_rename() {
    let (temp, backend) = fixture();
    let mut config = settings(temp.path());
    config.tailscale_users = vec!["allowed@example.com".into()];
    config.startup_config.as_mut().unwrap().tailscale_users = config.tailscale_users.clone();
    let a = WebServer::start(config, backend.clone(), &tokio::runtime::Handle::current());
    let addr = serving(&a).await;
    assert!(get(addr, "/").await.starts_with("HTTP/1.1 403"));
    assert!(get(addr, "/api/session").await.starts_with("HTTP/1.1 403"));
    let dir = temp.path().to_owned();
    let rows = tokio::task::spawn_blocking(move || crate::listener_file::inspect(&dir).unwrap())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_current());
    let instance = rows[0].responding_instance.clone().unwrap();
    assert_eq!(rows[0].responding_session, Some(backend.session_name()));
    assert_eq!(
        rows[0]
            .record
            .startup_config
            .as_ref()
            .unwrap()
            .tailscale_users,
        vec!["allowed@example.com"]
    );
    assert_eq!(
        rows[0].record.startup_config.as_ref().unwrap().port,
        Some(0)
    );
    assert_eq!(rows[0].record.address, addr);
    *backend.name.write().unwrap() = "renamed".into();
    wait_for_record(temp.path(), addr, "renamed".into(), None).await;
    let dir = temp.path().to_owned();
    let renamed = tokio::task::spawn_blocking(move || crate::listener_file::inspect(&dir).unwrap())
        .await
        .unwrap();
    assert!(renamed[0].is_current());
    assert_eq!(renamed[0].responding_instance.as_ref(), Some(&instance));
    let proxied=request(addr,"GET /healthz HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 100.64.0.1\r\nConnection: close\r\n\r\n").await;
    assert!(proxied.starts_with("HTTP/1.1 403"));
    stop(&a).await;
    let b = WebServer::start(
        settings(temp.path()),
        backend,
        &tokio::runtime::Handle::current(),
    );
    serving(&b).await;
    let dir = temp.path().to_owned();
    let rebound = tokio::task::spawn_blocking(move || crate::listener_file::inspect(&dir).unwrap())
        .await
        .unwrap();
    assert!(rebound[0].is_current());
    assert_ne!(rebound[0].responding_instance.as_ref(), Some(&instance));
    stop(&b).await;
}
#[tokio::test]
async fn rate_limits_native_errors_and_rejected_requests_never_dispatch() {
    struct Frozen;
    impl crate::auth::Clock for Frozen {
        fn now_ms(&self) -> i64 {
            1000000
        }
    }
    let (temp, backend) = fixture();
    let config = settings(temp.path());
    let auth = Arc::new(crate::auth::Auth::new(
        temp.path().into(),
        crate::auth::AuthConfig::default(),
        Arc::new(Frozen),
    ));
    let server = WebServer::start_with_auth(
        config,
        backend.clone(),
        &tokio::runtime::Handle::current(),
        auth,
    );
    let addr = serving(&server).await;
    let token = login(addr).await;
    for _ in 0..5 {
        assert!(
            api(addr, "POST", "/api/login", None, r#"{"password":"wrong"}"#)
                .await
                .starts_with("HTTP/1.1 401")
        );
    }
    let r = api(addr, "POST", "/api/login", None, r#"{"password":"wrong"}"#).await;
    assert!(r.starts_with("HTTP/1.1 429"));
    assert!(r.contains("retry-after: 60"));
    for i in 0..25 {
        let body = r#"{"password":"wrong"}"#;
        let r=request(addr,&format!("POST /api/login HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nTailscale-User-Login: client{i}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    }
    let r=request(addr,"POST /api/login HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nTailscale-User-Login: new-client\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"password\":\"wrong\"}").await;
    assert!(r.starts_with("HTTP/1.1 429"), "{r}");
    assert!(r.contains("retry-after: 900"));
    for _ in 0..40 {
        assert!(api(addr, "GET", "/api/session", Some(&token), "")
            .await
            .starts_with("HTTP/1.1 200"));
    }
    let r = api(addr, "GET", "/api/session", Some(&token), "").await;
    assert!(r.starts_with("HTTP/1.1 429"));
    assert!(r.contains("retry-after: 1"));
    for _ in 0..100 {
        assert!(get(addr, "/").await.starts_with("HTTP/1.1 503"));
    }
    let r = get(addr, "/").await;
    assert!(r.starts_with("HTTP/1.1 429"));
    assert!(r.contains("retry-after: 1"));
    assert!(get(addr, "/healthz").await.starts_with("HTTP/1.1 200"));
    for (method, path, body, status) in [
        ("GET", "/api/login", "", 405),
        ("HEAD", "/api/session", "", 405),
        ("POST", "/api/login", "{}", 400),
        ("GET", "/api/agents", "", 401),
    ] {
        let r = api(addr, method, path, None, body).await;
        assert!(r.starts_with(&format!("HTTP/1.1 {status}")), "{r}");
    }
    for value in [
        format!("Bearer {token}, Bearer {token}"),
        format!("Bearer  {token}"),
        "Basic forged".into(),
        "Bearer forged".into(),
    ] {
        let r=request(addr,&format!("GET /api/agents HTTP/1.1\r\nHost: localhost\r\nAuthorization: {value}\r\nConnection: close\r\n\r\n")).await;
        assert!(r.starts_with("HTTP/1.1 401"));
        assert!(r.contains("www-authenticate: Bearer"));
    }
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    stop(&server).await;
}

#[tokio::test]
async fn status_probes_ipv4_ipv6_wildcard_binds_using_local_family() {
    for address in ["0.0.0.0", "::"] {
        let (temp, backend) = fixture();
        let c = yinz_types::input::web::WebConfig {
            enabled: Some(true),
            address: Some(address.into()),
            allow_non_loopback: Some(true),
            port: Some(0),
            ..Default::default()
        };
        let mut config = WebSettings::from_config(&c, temp.path()).unwrap();
        config.retry_interval = Duration::from_millis(50);
        let server = WebServer::start(config, backend.clone(), &tokio::runtime::Handle::current());
        serving(&server).await;
        let data = temp.path().to_owned();
        let rows =
            tokio::task::spawn_blocking(move || crate::listener_file::inspect(&data).unwrap())
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].responding_session, Some(backend.session_name()));
        stop(&server).await;
    }
}

#[tokio::test]
async fn native_login_queue_overflow_returns_retry_and_does_not_block_health() {
    let (temp, backend) = fixture();
    let auth = Arc::new(crate::auth::Auth::new(
        temp.path().into(),
        crate::auth::AuthConfig::default(),
        Arc::new(crate::auth::SystemClock),
    ));
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let work_gate = gate.clone();
    let (entered, rx) = std::sync::mpsc::channel();
    *auth.verify_hook.lock().unwrap() = Some(Arc::new(move || {
        let _ = entered.send(());
        let mut open = work_gate.0.lock().unwrap();
        while !*open {
            open = work_gate.1.wait(open).unwrap();
        }
    }));
    let server = WebServer::start_with_auth(
        settings(temp.path()),
        backend.clone(),
        &tokio::runtime::Handle::current(),
        auth.clone(),
    );
    let addr = serving(&server).await;
    let mut clients = JoinSet::new();
    for _ in 0..3 {
        clients.spawn(async move {
            api(
                addr,
                "POST",
                "/api/login",
                None,
                r#"{"password":"twelve words long"}"#,
            )
            .await
        });
    }
    tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)).unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while auth.login_slots() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let overflow = api(
        addr,
        "POST",
        "/api/login",
        None,
        r#"{"password":"twelve words long"}"#,
    )
    .await;
    assert!(overflow.starts_with("HTTP/1.1 429"), "{overflow}");
    assert!(overflow.contains("retry-after: 1"));
    assert!(get(addr, "/healthz").await.starts_with("HTTP/1.1 200"));
    assert_eq!(backend.dispatches.load(Ordering::SeqCst), 0);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    while let Some(reply) = clients.join_next().await {
        assert!(reply.unwrap().starts_with("HTTP/1.1 200"));
    }
    stop(&server).await;
}
