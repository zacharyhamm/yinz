//! Managed local Swamp execution. The supervisor owns no API connection: the host multiplexes it.
use futures_util::{SinkExt, StreamExt};
use isahc::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use zellij_runtime::errors::prelude::*;

pub const SUPERVISOR_FLAG: &str = "--swamp-server";
const SERVICE: &str = "zellij-service";
const MAX_OUTPUT: usize = 1_048_576;
const STARTUP: Duration = Duration::from_secs(90);

// Deliberately not Debug: credentials must not enter diagnostic output.
#[derive(Clone, Serialize, Deserialize)]
struct Endpoint {
    root: PathBuf,
    pid: u32,
    started: u64,
    child_pid: u32,
    child_started: u64,
    url: String,
    token: String,
}

fn private_dir(path: &Path) -> Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
        Err(error) => return Err(error).context("cannot create Swamp runtime directory"),
    }
    let meta = std::fs::symlink_metadata(path).context("cannot inspect Swamp runtime directory")?;
    if !meta.is_dir() || meta.uid() != nix::unistd::geteuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(anyhow!(
            "Swamp runtime directory must be private and owned by this user"
        ));
    }
    Ok(())
}

fn directory(root: &Path) -> Result<PathBuf> {
    let base = PathBuf::from(format!("/tmp/zellij-swamp-{}", nix::unistd::geteuid()));
    private_dir(&base)?;
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let dir = base.join(digest);
    private_dir(&dir)?;
    Ok(dir)
}

fn lock(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory(root)?.join("lock"))
        .context("cannot open managed Swamp lock")?;
    file.lock().context("cannot lock managed Swamp checkout")?;
    Ok(file)
}

fn endpoint(root: &Path) -> Result<Option<Endpoint>> {
    let path = directory(root)?.join("endpoint.json");
    match std::fs::read(path) {
        Ok(bytes) => {
            let endpoint: Endpoint =
                serde_json::from_slice(&bytes).context("invalid managed Swamp state")?;
            if endpoint.root != root || !valid_url(&endpoint.url) {
                return Err(anyhow!("managed Swamp checkout or endpoint mismatch"));
            }
            Ok(Some(endpoint))
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("cannot read managed Swamp state"),
    }
}

fn valid_url(value: &str) -> bool {
    let Ok(uri) = value.parse::<isahc::http::Uri>() else {
        return false;
    };
    uri.scheme_str() == Some("ws")
        && uri.host() == Some("127.0.0.1")
        && uri.port_u16().is_some_and(|port| port != 0)
        && uri
            .authority()
            .is_some_and(|a| a.as_str() == format!("127.0.0.1:{}", uri.port_u16().unwrap_or(0)))
        && uri.path() == "/"
        && uri.query().is_none()
}

fn process_started(pid: u32) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.start_time())
}

fn alive(endpoint: &Endpoint) -> bool {
    process_started(endpoint.child_pid) == Some(endpoint.child_started)
}

fn publish(endpoint: &Endpoint) -> Result<()> {
    let dir = directory(&endpoint.root)?;
    let mut file =
        tempfile::NamedTempFile::new_in(&dir).context("cannot stage Swamp credentials")?;
    serde_json::to_writer(file.as_file_mut(), endpoint)
        .context("cannot serialize Swamp credentials")?;
    file.as_file_mut()
        .flush()
        .context("cannot flush Swamp credentials")?;
    file.persist(dir.join("endpoint.json"))
        .map_err(|e| e.error)
        .context("cannot publish Swamp credentials")?;
    Ok(())
}

fn cli(root: &Path, args: &[&str]) -> Result<Value> {
    let mut command = Command::new("swamp");
    for (key, _) in std::env::vars_os() {
        if matches!(
            key.to_str(),
            Some(
                "SWAMP_REPO_DIR"
                    | "SWAMP_SERVER_URL"
                    | "SWAMP_SERVE_URL"
                    | "SWAMP_SERVER_TOKEN"
                    | "SWAMP_SERVE_EXTRA_HEADERS"
            )
        ) {
            command.env_remove(key);
        }
    }
    let output = command
        .current_dir(root)
        .args(args)
        .args(["--json", "--no-telemetry"])
        .stdin(Stdio::null())
        .output()
        .context("cannot execute Swamp bootstrap command")?;
    if !output.status.success() {
        // Never echo token-command stdout/stderr or arguments.
        return Err(anyhow!(
            "Swamp credential bootstrap failed; inspect local vault configuration"
        ));
    }
    serde_json::from_slice(&output.stdout).context("invalid Swamp bootstrap response")
}

// The service vault is machine-local. Its generated definition must not make the
// control checkout dirty and prevent the workflow's guarded Git operations.
fn exclude_service_vault(root: &Path) -> Result<()> {
    let vault = cli(root, &["vault", "get", SERVICE])?;
    let storage = Path::new(
        vault["storagePath"]
            .as_str()
            .context("missing service vault path")?,
    );
    if storage.is_absolute()
        || storage
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(anyhow!("invalid service vault path"));
    }
    let git_path = |args: &[&str]| -> Result<Option<PathBuf>> {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .context("cannot locate Git exclusion file")?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(PathBuf::from(
            String::from_utf8(output.stdout)
                .context("invalid Git path")?
                .trim_end_matches('\n'),
        )))
    };
    let Some(top) = git_path(&["rev-parse", "--show-toplevel"])? else {
        return Ok(());
    };
    let exclude = git_path(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "info/exclude",
    ])?
    .context("missing Git exclusion file")?;
    let relative = root
        .join(storage)
        .strip_prefix(top)
        .context("service vault is outside Git checkout")?
        .to_owned();
    let pattern = format!(
        "/{}",
        relative
            .to_str()
            .context("invalid service vault filename")?
    );
    if pattern.contains(['\n', '\r', '*', '?', '[', '\\']) {
        return Err(anyhow!("unsupported service vault filename"));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&exclude)
        .context("cannot exclude service vault")?;
    file.lock().context("cannot lock Git exclusions")?;
    let existing = std::fs::read_to_string(&exclude).context("cannot read Git exclusions")?;
    if !existing.lines().any(|line| line == pattern) {
        writeln!(file, "\n# Zellij managed Swamp service vault\n{pattern}")
            .context("cannot exclude service vault")?;
    }
    Ok(())
}

fn credential(root: &Path) -> Result<String> {
    let vaults = cli(root, &["vault", "search", SERVICE])?;
    if !vaults["results"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["name"] == SERVICE))
    {
        cli(root, &["vault", "create", "local_encryption", SERVICE])?;
    }
    exclude_service_vault(root)?;
    let existing = cli(root, &["access", "token", "reveal", SERVICE, "--yes"]);
    match existing {
        Ok(_) => {
            cli(
                root,
                &[
                    "access",
                    "token",
                    "rotate",
                    SERVICE,
                    "--vault",
                    SERVICE,
                    "--duration",
                    "7d",
                ],
            )?;
        },
        Err(_) => {
            cli(
                root,
                &[
                    "access",
                    "token",
                    "mint",
                    SERVICE,
                    "--principal",
                    "user:zellij-service",
                    "--duration",
                    "7d",
                    "--vault",
                    SERVICE,
                ],
            )?;
        },
    }
    let value = cli(root, &["access", "token", "reveal", SERVICE, "--yes"])?;
    value["token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .context("Swamp returned no service credential")
}

fn http(endpoint: &Endpoint, path: &str, post: bool) -> Result<Value> {
    let url = format!(
        "http://{}{}",
        endpoint.url.trim_start_matches("ws://"),
        path
    );
    let request = isahc::Request::builder()
        .method(if post { "POST" } else { "GET" })
        .uri(url)
        .header("Authorization", format!("Bearer {}", endpoint.token))
        .timeout(Duration::from_secs(10))
        .body(())
        .context("cannot build managed Swamp request")?;
    let mut response =
        isahc::send(request).map_err(|_| anyhow!("managed Swamp HTTP connection failed"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "managed Swamp request failed with HTTP {}",
            response.status()
        ));
    }
    let body = response
        .text()
        .context("cannot read managed Swamp response")?;
    serde_json::from_str(&body).context("invalid managed Swamp response")
}

/// Spawned before the application initializes. This process only supervises Swamp and renews its token.
pub fn supervise(root: PathBuf) -> Result<()> {
    unsafe {
        close_fds::close_open_fds(3, &[]);
    }
    let root = root
        .canonicalize()
        .context("cannot resolve Swamp control checkout")?;
    let startup_lock = lock(&root)?;
    if let Some(state) = endpoint(&root)? {
        if alive(&state) {
            return Ok(());
        }
        return Err(anyhow!(
            "managed Swamp server exited; inspect runs, then use zellij workflow server restart"
        ));
    }
    let help = cli(&root, &["help", "serve"])?;
    if help["version"]
        .as_str()
        .is_some_and(|version| version.ends_with("375d7eca"))
    {
        return Err(anyhow!("installed Swamp cannot resume inline model types; use a compatible build and run extensions/tests/serve_compatibility.ts"));
    }
    let token = credential(&root)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start Swamp supervisor runtime")?;
    runtime.block_on(async move {
        let mut command = tokio::process::Command::new("swamp");
        for (key, _) in std::env::vars_os() {
            if matches!(key.to_str(), Some("SWAMP_REPO_DIR" | "SWAMP_SERVER_URL" | "SWAMP_SERVE_URL" | "SWAMP_SERVER_TOKEN" | "SWAMP_SERVE_EXTRA_HEADERS")) { command.env_remove(key); }
        }
        let mut child = command.current_dir(&root).args(["serve", "--host", "127.0.0.1", "--port", "0",
            "--auth-mode", "token", "--admins", "user:zellij-service", "--no-schedule", "--json", "--no-telemetry"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true)
            .spawn().context("cannot start Swamp serve")?;
        let mut lines = BufReader::new(child.stdout.take().context("missing Swamp startup pipe")?).lines();
        let announcement = tokio::time::timeout(STARTUP, async {
            while let Some(line) = lines.next_line().await.context("cannot read Swamp startup")? {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if value["status"] == "listening" { return Ok(value); }
                }
            }
            Err(anyhow!("Swamp exited before announcing an endpoint"))
        }).await.context("Swamp startup timed out")??;
        let url = announcement["url"].as_str().context("Swamp announced no endpoint")?.to_owned();
        if !valid_url(&url) || announcement["detachRuns"] != true || announcement["schedulingEnabled"] != false {
            return Err(anyhow!("Swamp is incompatible: authenticated detached local execution is required"));
        }
        let child_pid = child.id().context("Swamp exited during startup")?;
        let mut state = Endpoint { root, pid: std::process::id(), started: process_started(std::process::id()).context("cannot identify Swamp supervisor")?, child_pid,
            child_started: process_started(child_pid).context("cannot identify Swamp process")?, url, token };
        let check = state.clone();
        tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + STARTUP;
            while Instant::now() < deadline {
                if http(&check, "/ready", false).is_ok() { return Ok(()); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(anyhow!("Swamp did not become ready"))
        }).await.context("Swamp readiness task failed")??;
        publish(&state)?;
        drop(startup_lock);
        let mut renewal = tokio::time::interval(Duration::from_secs(24 * 3600));
        renewal.tick().await;
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).context("cannot watch supervisor termination")?;
        loop {
            tokio::select! {
                status = child.wait() => { status.context("cannot reap Swamp")?; return Ok(()); },
                _ = term.recv() => {
                    if let Some(pid) = child.id() {
                        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), nix::sys::signal::Signal::SIGTERM).context("cannot stop Swamp")?;
                    }
                    if tokio::time::timeout(Duration::from_secs(10), child.wait()).await.is_err() { child.kill().await.context("cannot reap stopped Swamp")?; }
                    return Ok(());
                },
                line = lines.next_line() => { line.context("cannot drain Swamp output")?; },
                _ = renewal.tick() => {
                    let root = state.root.clone();
                    match tokio::task::spawn_blocking(move || credential(&root)).await.context("credential renewal task failed").and_then(|result| result) {
                        Ok(token) => { state.token = token; publish(&state).non_fatal(); },
                        Err(error) => { Err::<(), _>(error).context("Swamp credential renewal failed; existing executions remain active").non_fatal(); },
                    }
                }
            }
        }
    })
}

fn ensure_locked(root: &Path) -> Result<Endpoint> {
    if let Some(state) = endpoint(root)? {
        if alive(&state) {
            return Ok(state);
        }
        return Err(anyhow!(
            "managed Swamp server exited; inspect runs, then use zellij workflow server restart"
        ));
    }
    let binary = std::env::current_exe().context("cannot locate Zellij supervisor binary")?;
    let mut command = Command::new(binary);
    command
        .args([std::ffi::OsStr::new(SUPERVISOR_FLAG), root.as_os_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("cannot start managed Swamp supervisor")?;
    let deadline = Instant::now() + STARTUP;
    loop {
        if let Some(state) = endpoint(root)? {
            // Reap without binding supervisor lifetime to this application.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(state);
        }
        if child
            .try_wait()
            .context("cannot inspect Swamp startup")?
            .is_some()
        {
            let mut diagnostic = String::new();
            if let Some(stderr) = child.stderr.take() {
                stderr
                    .take(16_384)
                    .read_to_string(&mut diagnostic)
                    .context("cannot read Swamp startup diagnostic")?;
            }
            return Err(anyhow!(
                "managed Swamp startup failed: {}",
                diagnostic.trim()
            ));
        }
        if Instant::now() >= deadline {
            // This is our newly created process group, never an ID from persisted state.
            nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            )
            .ok();
            child
                .wait()
                .context("cannot reap timed-out Swamp startup")?;
            return Err(anyhow!("managed Swamp startup timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn prewarm(root: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .context("cannot resolve Swamp checkout")?;
    ensure_locked(&root)?;
    Ok(())
}

fn allowed(operation: &str) -> bool {
    matches!(
        operation,
        "workflow.validate"
            | "workflow.run"
            | "workflow.history.get"
            | "workflow.history.search"
            | "workflow.history.logs"
            | "workflow.approve"
            | "workflow.resume"
            | "model.method.run"
            | "data.query"
            | "run.attach"
            | "run.history"
    )
}
fn read_request(operation: &str) -> bool {
    matches!(
        operation,
        "workflow.validate"
            | "workflow.history.get"
            | "workflow.history.search"
            | "workflow.history.logs"
            | "data.query"
            | "run.history"
    )
}

struct Request {
    id: String,
    operation: String,
    payload: Value,
    events: mpsc::UnboundedSender<Value>,
    run: Option<String>,
    cursor: u64,
    guard: Option<File>,
    sent_at: Instant,
}
type Registry = Mutex<HashMap<PathBuf, mpsc::Sender<Request>>>;
static CONNECTIONS: OnceLock<Registry> = OnceLock::new();

/// Each caller has a correlated stream; exactly one socket reader/writer runs per checkout.
pub async fn request(
    root: PathBuf,
    id: String,
    operation: String,
    payload: Value,
) -> Result<mpsc::UnboundedReceiver<Value>> {
    if !allowed(&operation)
        || id.is_empty()
        || id.len() > 4096
        || !payload.is_object()
        || payload.to_string().len() > 2 * MAX_OUTPUT
    {
        return Err(anyhow!("invalid or unsupported managed Swamp request"));
    }
    let root = root
        .canonicalize()
        .context("cannot resolve Swamp checkout")?;
    let (events, receiver) = mpsc::unbounded_channel();
    let sender = {
        let mut registry = CONNECTIONS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow!("Swamp connection registry poisoned"))?;
        registry
            .entry(root.clone())
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(128);
                tokio::spawn(connection(root.clone(), receiver));
                sender
            })
            .clone()
    };
    let guard = if read_request(&operation) || operation == "run.attach" {
        None
    } else {
        let root = root.clone();
        Some(
            tokio::task::spawn_blocking(move || lock(&root))
                .await
                .context("Swamp dispatch lock task failed")??,
        )
    };
    let run = (operation == "run.attach")
        .then(|| payload["runId"].as_str().map(str::to_owned))
        .flatten();
    let cursor = payload["afterSeq"].as_u64().unwrap_or(0);
    sender
        .send(Request {
            id,
            operation,
            payload,
            events,
            run,
            cursor,
            guard,
            sent_at: Instant::now(),
        })
        .await
        .context("managed Swamp connection is unavailable")?;
    Ok(receiver)
}

fn failure(id: &str, message: &str) -> Value {
    json!({"type":"error", "id":id, "error":{"code":"transport_unknown", "message":message}})
}

fn bound_frame(mut frame: Value) -> Value {
    // History inputs duplicate the full saved test log. Its exact version is queried separately.
    if frame["type"] == "workflow.history.get" || frame["type"] == "workflow.history.search" {
        let data = &mut frame["payload"]["data"];
        fn strip(row: &mut Value) {
            if let Some(test) = row
                .pointer_mut("/inputs/state/test")
                .and_then(Value::as_object_mut)
            {
                test.remove("output");
            }
        }
        strip(data);
        if let Some(rows) = data["results"].as_array_mut() {
            for row in rows {
                strip(row);
            }
        }
    }
    if frame.to_string().len() > MAX_OUTPUT {
        failure(
            frame["id"].as_str().unwrap_or(""),
            "Swamp response exceeds the bounded preview; retrieve saved data by exact version",
        )
    } else {
        frame
    }
}

async fn socket(
    root: &Path,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let root = root.to_owned();
    // Startup has its own lock. Dispatchers acquire their mutation lock only after prewarming.
    let state = tokio::task::spawn_blocking(move || {
        endpoint(&root)?.context("managed Swamp has no endpoint")
    })
    .await
    .context("Swamp endpoint task failed")??;
    let mut request = state
        .url
        .into_client_request()
        .map_err(|_| anyhow!("invalid managed Swamp endpoint"))?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("bearer.{}", state.token)
            .parse()
            .map_err(|_| anyhow!("invalid Swamp credential"))?,
    );
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(16 * MAX_OUTPUT));
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_with_config(request, Some(config), true),
    )
    .await
    .context("Swamp connection timed out")?
    .map_err(|_| anyhow!("managed Swamp WebSocket connection failed"))?;
    Ok(socket)
}

async fn connection(root: PathBuf, mut incoming: mpsc::Receiver<Request>) {
    let mut pending: HashMap<String, Request> = HashMap::new();
    let mut deadlines = tokio::time::interval(Duration::from_secs(1));
    loop {
        if pending.is_empty() {
            let Some(request) = incoming.recv().await else {
                return;
            };
            pending.insert(request.id.clone(), request);
        }
        let mut ws = match socket(&root).await {
            Ok(socket) => socket,
            Err(error) => {
                for (_, request) in pending.drain() {
                    let _ = request
                        .events
                        .send(failure(&request.id, &error.to_string()));
                }
                continue;
            },
        };
        let mut broken = false;
        for request in pending.values() {
            let frame = if let Some(run) = &request.run {
                json!({"type":"run.attach", "id":request.id, "payload":{"runId":run,"afterSeq":request.cursor}})
            } else {
                json!({"type":request.operation,"id":request.id,"payload":request.payload})
            };
            if ws
                .send(Message::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                broken = true;
                break;
            }
        }
        while !broken {
            tokio::select! {
                _ = deadlines.tick() => {
                    pending.retain(|id, request| {
                        if request.run.is_none() && request.sent_at.elapsed() > STARTUP {
                            let _ = request.events.send(failure(id,"Swamp request timed out; reconcile before dispatching a mutation again"));
                            false
                        } else { true }
                    });
                },
                request = incoming.recv() => {
                    let Some(request) = request else { return; };
                    if pending.contains_key(&request.id) {
                        let _ = request.events.send(failure(&request.id, "request identity already in flight; reconcile the existing operation"));
                        continue;
                    }
                    let frame = json!({"type":request.operation,"id":request.id,"payload":request.payload});
                    pending.insert(request.id.clone(), request);
                    broken = ws.send(Message::Text(frame.to_string().into())).await.is_err();
                },
                message = ws.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            let Ok(frame) = serde_json::from_str::<Value>(&text) else { broken = true; continue; };
                            let Some(id) = frame["id"].as_str().map(str::to_owned) else { continue; };
                            let terminal = frame["type"] != "event" && frame["type"] != "run.attached";
                            if let Some(request) = pending.get_mut(&id) {
                                if let Some(run) = frame["event"]["runId"].as_str() { request.run = Some(run.to_owned()); request.guard.take(); }
                                if let Some(seq) = frame["event"]["seq"].as_u64() { request.cursor = seq; }
                                let _ = request.events.send(bound_frame(frame));
                            }
                            if terminal { pending.remove(&id); }
                        },
                        Some(Ok(Message::Ping(bytes))) => { broken = ws.send(Message::Pong(bytes)).await.is_err(); },
                        Some(Ok(Message::Pong(_))) => {},
                        _ => { broken = true; },
                    }
                }
            }
        }
        pending.retain(|id, request| {
            request.guard.take();
            if request.run.is_some() || read_request(&request.operation) { true } else {
                let _ = request.events.send(failure(id, "delivery is uncertain; reconcile persisted run/submission state before dispatch"));
                false
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn call(root: PathBuf, operation: &str, payload: Value) -> Result<Value> {
    let warm = root.clone();
    tokio::task::spawn_blocking(move || prewarm(&warm))
        .await
        .context("Swamp startup task failed")??;
    let mut events = request(
        root,
        uuid::Uuid::new_v4().to_string(),
        operation.to_owned(),
        payload,
    )
    .await?;
    while let Some(frame) = events.recv().await {
        if frame["type"] == "error" {
            return Err(anyhow!("{}", frame["error"]));
        }
        if frame.get("payload").is_some() {
            return Ok(frame["payload"]
                .get("data")
                .unwrap_or(&frame["payload"])
                .clone());
        }
    }
    Err(anyhow!("Swamp request ended without a result"))
}

pub async fn cancel(root: PathBuf, run: String) -> Result<Value> {
    uuid::Uuid::parse_str(&run).context("cancellation requires a run UUID")?;
    tokio::task::spawn_blocking(move || {
        let state = endpoint(&root)?.context("managed Swamp is not running")?;
        http(&state, &format!("/api/v1/cancel/workflow-run/{run}"), true)
    })
    .await
    .context("Swamp cancellation task failed")?
}

pub async fn lifecycle(root: PathBuf, action: &str) -> Result<Value> {
    let root = root
        .canonicalize()
        .context("cannot resolve control checkout")?;
    if action == "status" {
        return Ok(match endpoint(&root)? {
            Some(state) => {
                json!({"status":if alive(&state) {"running"} else {"exited"},"pid":state.pid,"checkout":root})
            },
            None => json!({"status":"stopped","checkout":root}),
        });
    }
    if !matches!(action, "stop" | "restart") {
        return Err(anyhow!("unknown managed Swamp lifecycle action"));
    }
    let locked_root = root.clone();
    let guard = tokio::task::spawn_blocking(move || lock(&locked_root))
        .await
        .context("Swamp lifecycle lock failed")??;
    if let Some(state) = endpoint(&root)? {
        if alive(&state) {
            // No startup here: the lifecycle lock already excludes new dispatch.
            let mut frames = request(
                root.clone(),
                uuid::Uuid::new_v4().to_string(),
                "run.history".into(),
                json!({"active":true}),
            )
            .await?;
            let frame = tokio::time::timeout(Duration::from_secs(10), frames.recv())
                .await
                .context("cannot verify active Swamp executions")?
                .context("missing Swamp activity response")?;
            let runs = frame["payload"]["runs"]
                .as_array()
                .context("cannot verify active Swamp executions")?;
            if runs.iter().any(|run| run["status"] == "running") {
                return Err(anyhow!("Swamp has active executions; stop/restart refused"));
            }
            let pid = if process_started(state.pid) == Some(state.started) {
                state.pid
            } else {
                state.child_pid
            };
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).context("invalid supervisor PID")?),
                nix::sys::signal::Signal::SIGTERM,
            )
            .context("cannot stop Swamp supervisor")?;
            let deadline = Instant::now() + Duration::from_secs(15);
            while alive(&state) {
                if Instant::now() > deadline {
                    return Err(anyhow!("Swamp supervisor did not stop"));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        std::fs::remove_file(directory(&root)?.join("endpoint.json"))
            .context("cannot remove stopped Swamp endpoint")?;
    }
    if action == "restart" {
        drop(guard);
        tokio::task::spawn_blocking(move || ensure_locked(&root))
            .await
            .context("Swamp restart task failed")??;
    }
    Ok(json!({"status":if action == "restart" {"running"} else {"stopped"}}))
}

#[cfg(test)]
mod tests {
    // tungstenite requires a full HTTP response as the handshake error.
    #![allow(clippy::result_large_err)]
    use super::*;

    async fn fixture() -> (tempfile::TempDir, PathBuf, tokio::net::TcpListener) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pid = std::process::id();
        let started = process_started(pid).unwrap();
        publish(&Endpoint {
            root: root.clone(),
            pid,
            started,
            child_pid: pid,
            child_started: started,
            url: format!("ws://{}", listener.local_addr().unwrap()),
            token: "test.secret".into(),
        })
        .unwrap();
        (dir, root, listener)
    }

    async fn next(events: &mut mpsc::UnboundedReceiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn multiplexes_responses_and_preserves_structured_errors() {
        let (_dir, root, listener) = fixture().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                Ok(response)
            }).await.unwrap();
            let first: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            let second: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            ws.send(Message::Text(json!({"id":second["id"],"type":"error","error":{"code":"denied","message":"no access"}}).to_string().into())).await.unwrap();
            ws.send(Message::Text(
                json!({"id":first["id"],"type":"data.query","payload":{"data":{"answer":42}}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        });
        let mut first = request(
            root.clone(),
            "first".into(),
            "data.query".into(),
            json!({"predicate":"true"}),
        )
        .await
        .unwrap();
        let mut second = request(
            root.clone(),
            "second".into(),
            "data.query".into(),
            json!({"predicate":"false"}),
        )
        .await
        .unwrap();
        assert_eq!(next(&mut first).await["payload"]["data"]["answer"], 42);
        assert_eq!(next(&mut second).await["error"]["code"], "denied");
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn reconnect_attaches_with_cursor_without_replaying_mutation() {
        let (_dir, root, listener) = fixture().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut first = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                Ok(response)
            }).await.unwrap();
            let frame: Value =
                serde_json::from_str(first.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(frame["type"], "workflow.run");
            first.send(Message::Text(json!({"id":"mutation","type":"event","event":{"kind":"started","runId":"run","seq":1}}).to_string().into())).await.unwrap();
            first.close(None).await.unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut second = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                Ok(response)
            }).await.unwrap();
            let frame: Value =
                serde_json::from_str(second.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(frame["type"], "run.attach");
            assert_eq!(frame["payload"], json!({"runId":"run","afterSeq":1}));
            second
                .send(Message::Text(
                    json!({"id":"mutation","type":"event","event":{"kind":"completed","seq":2}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            second
                .send(Message::Text(
                    json!({"id":"mutation","type":"done"}).to_string().into(),
                ))
                .await
                .unwrap();
        });
        let mut events = request(
            root.clone(),
            "mutation".into(),
            "workflow.run".into(),
            json!({"workflowIdOrName":"workflow"}),
        )
        .await
        .unwrap();
        assert_eq!(next(&mut events).await["event"]["seq"], 1);
        assert_eq!(next(&mut events).await["event"]["seq"], 2);
        assert_eq!(next(&mut events).await["type"], "done");
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }
    #[tokio::test]
    async fn lost_acknowledgement_replays_reads_but_never_mutations() {
        let (_dir, root, listener) = fixture().await;
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    response.headers_mut().insert("Sec-WebSocket-Protocol", request.headers()["Sec-WebSocket-Protocol"].clone());
                    Ok(response)
                }).await.unwrap();
                if attempt == 0 {
                    for _ in 0..2 {
                        ws.next().await.unwrap().unwrap();
                    }
                    ws.close(None).await.unwrap();
                } else {
                    let frame: Value =
                        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap())
                            .unwrap();
                    assert_eq!(frame["type"], "data.query");
                    assert_eq!(frame["id"], "read");
                    ws.send(Message::Text(
                        json!({"id":"read","type":"data.query","payload":{"data":{"answer":42}}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                }
            }
        });
        let mut mutation = request(
            root.clone(),
            "unknown".into(),
            "workflow.run".into(),
            json!({"workflowIdOrName":"workflow"}),
        )
        .await
        .unwrap();
        let mut read = request(
            root.clone(),
            "read".into(),
            "data.query".into(),
            json!({"predicate":"true"}),
        )
        .await
        .unwrap();
        assert_eq!(
            next(&mut mutation).await["error"]["code"],
            "transport_unknown"
        );
        assert!(mutation.recv().await.is_none());
        assert_eq!(next(&mut read).await["payload"]["data"]["answer"], 42);
        server.await.unwrap();
        std::fs::remove_dir_all(directory(&root).unwrap()).unwrap();
    }

    #[test]
    fn operations_and_previews_are_bounded() {
        assert!(valid_url("ws://127.0.0.1:12345"));
        for invalid in [
            "ws://127.0.0.1:0",
            "ws://127.0.0.1:123@evil.example",
            "ws://localhost:123",
            "ws://127.0.0.1:123/other",
        ] {
            assert!(!valid_url(invalid));
        }
        assert!(allowed("workflow.run"));
        assert!(!allowed("vault.read"));
        assert!(!read_request("workflow.resume"));
        let frame = bound_frame(
            json!({"id":"history","type":"workflow.history.get","payload":{"data":{"inputs":{"state":{"test":{"output":"large","log":"exact-version"}}}}}}),
        );
        assert!(frame
            .pointer("/payload/data/inputs/state/test/output")
            .is_none());
        assert_eq!(
            frame.pointer("/payload/data/inputs/state/test/log"),
            Some(&json!("exact-version"))
        );
        assert_eq!(
            bound_frame(
                json!({"id":"large","type":"event","event":{"output":"x".repeat(MAX_OUTPUT)}})
            )["type"],
            "error"
        );
    }
}
