//! Bind a Codex UI's selected conversation to its pane while retaining the shared daemon.
use futures_util::{future::OptionFuture, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use yinz_runtime::agent::codex::bridge_options;
use yinz_runtime::agent::registry;
use yinz_runtime::agent::report::ROUTE_ENV;
use yinz_runtime::errors::prelude::*;

// Both peers are trusted local Codex processes, and Codex's own UI accepts the daemon's
// single-frame `plugin/list` reply, which outgrew tungstenite's 16 MiB frame default.
// Any relay limit would eventually reject traffic that works without the relay.
fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None)
}

fn directory() -> PathBuf {
    // The shared daemon can inherit different TMPDIR and XDG directories from its UIs.
    Path::new("/tmp")
        .join(format!("yinz-{}", nix::unistd::Uid::current()))
        .join("codex")
}

#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    thread_id: String,
    env: HashMap<String, String>,
}

struct Registration {
    registry: registry::Registration,
    /// The yinz binary that announces selected conversations to the pane, if any.
    reporter: Option<PathBuf>,
    backend_socket: Option<PathBuf>,
}

impl Registration {
    fn new(root: &Path, env: HashMap<String, String>) -> Result<Self> {
        Ok(Self {
            registry: registry::Registration::new(root, "ui-", env).context("register Codex UI")?,
            reporter: None,
            backend_socket: sandboxed().then(|| PathBuf::from(PRIVATE_BACKEND)),
        })
    }

    /// Codex runs `SessionStart` hooks only when a conversation's first turn begins, so the
    /// pane would otherwise stay unverified until someone types into it.
    async fn announce(&self, mut hook: Value, program: &Path) {
        if self.reporter.is_none() {
            return;
        }
        let ready = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            verify_capability(program, &hook, self.backend_socket.as_deref()),
        )
        .await
        .context("timed out checking Codex reporting hooks")
        .and_then(|result| result)
        {
            Ok(()) => true,
            Err(error) => {
                Err::<(), _>(error)
                    .context("Codex conversation remains unverified")
                    .non_fatal();
                false
            },
        };
        hook["_yinz_codex_hooks_ready"] = ready.into();
        self.report(hook).await;
    }

    async fn report(&self, hook: Value) {
        let Some(yinz) = &self.reporter else {
            return;
        };
        // Finish reporting before the UI sees the selection or any later request is forwarded.
        forward(yinz, &self.registry.env, hook.to_string().as_bytes())
            .await
            .context("report Codex lifecycle to its pane")
            .non_fatal();
    }

    fn select(&self, thread_id: &str) -> Result<()> {
        if thread_id.is_empty() || thread_id.len() > 256 {
            return Err(anyhow!("invalid Codex conversation id"));
        }
        let binding = Binding {
            thread_id: thread_id.into(),
            env: self.registry.env.clone(),
        };
        registry::write_json(&self.registry.dir.path().join("binding.json"), &binding)
            .context("publish Codex pane binding")?;
        Ok(())
    }
}

fn binding_for(root: &Path, thread_id: &str) -> Result<Option<Binding>> {
    registry::latest_binding(root, |binding: &Binding| binding.thread_id == thread_id)
        .context("read Codex pane registry")
}

/// Give a selected conversation's tool commands the pane's routing environment.
///
/// Codex runs tools in its shared daemon, whose environment belongs to whichever pane started
/// it, or to none. Per-thread `shell_environment_policy.set` overrides reach those commands, so
/// `yinz` run by the agent knows its session and pane. Hidden helper threads never run tools.
/// Returns whether the request changed.
fn inject_pane_env(value: &mut Value, env: &HashMap<String, String>) -> bool {
    if env.is_empty()
        || !matches!(
            value["method"].as_str(),
            Some("thread/start" | "thread/resume" | "thread/fork")
        )
    {
        return false;
    }
    let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) else {
        return false;
    };
    if params.get("ephemeral").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let config = params
        .entry("config")
        .and_modify(|config| {
            if config.is_null() {
                *config = Value::Object(Default::default());
            }
        })
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(config) = config.as_object_mut() else {
        return false;
    };
    // A nested table and dotted keys for the same table would compete, so join whichever
    // spelling the UI already used.
    if let Some(policy) = config
        .get_mut("shell_environment_policy")
        .and_then(Value::as_object_mut)
    {
        let set = policy
            .entry("set")
            .or_insert_with(|| Value::Object(Default::default()));
        let Some(set) = set.as_object_mut() else {
            return false;
        };
        for (key, value) in env {
            set.insert(key.clone(), value.clone().into());
        }
    } else {
        for (key, value) in env {
            config.insert(
                format!("shell_environment_policy.set.{key}"),
                value.clone().into(),
            );
        }
    }
    true
}

#[derive(Default)]
struct Conversations {
    /// Request ids of conversation selections awaiting their reply, and whether each asked
    /// for an ephemeral thread.
    pending: HashMap<String, (String, bool)>,
    /// Hidden helper threads, such as the TUI's title generator, which never own the pane.
    ephemeral: HashSet<String>,
    selected: Option<String>,
    current_turn: Option<String>,
}

impl Conversations {
    fn select(&mut self, thread: &str, registration: &Registration) -> Result<()> {
        registration.select(thread)?;
        if self.selected.as_deref() != Some(thread) {
            self.selected = Some(thread.to_owned());
            self.current_turn = None;
        }
        Ok(())
    }

    fn request(&mut self, value: &Value, registration: &Registration) -> Result<()> {
        let Some(method) = value["method"].as_str() else {
            return Ok(());
        };
        if matches!(method, "thread/start" | "thread/resume" | "thread/fork")
            && let Some(id) = value.get("id")
        {
            let ephemeral = value["params"]["ephemeral"].as_bool() == Some(true);
            self.pending
                .insert(id.to_string(), (method.to_owned(), ephemeral));
        }
        // Also handles a UI returning to an already loaded conversation without resuming it.
        if matches!(method, "turn/start" | "turn/steer")
            && let Some(thread) = value["params"]["threadId"].as_str()
            && !self.ephemeral.contains(thread)
        {
            self.select(thread, registration)?;
        }
        Ok(())
    }

    /// Observe selections and final failures without changing the backend's messages.
    ///
    /// Returns the lifecycle hooks to report, in order.
    fn response(&mut self, value: &Value, registration: &Registration) -> Result<Vec<Value>> {
        if value.get("id").is_none() {
            let params = &value["params"];
            if params["threadId"].as_str().is_some_and(|thread| {
                self.selected.as_deref() == Some(thread) && !self.ephemeral.contains(thread)
            }) {
                let turn = &params["turn"];
                if let Some(id) = turn["id"].as_str().filter(|id| !id.is_empty()) {
                    match value["method"].as_str() {
                        Some("turn/started") => self.current_turn = Some(id.to_owned()),
                        Some("turn/completed")
                            if self.current_turn.as_deref() == Some(id)
                                && matches!(
                                    turn["status"].as_str(),
                                    Some("completed" | "interrupted" | "failed")
                                ) =>
                        {
                            // Consume every terminal event so a duplicate or delayed failure
                            // cannot end this turn twice or affect a newer turn.
                            self.current_turn = None;
                            if turn["status"] == "failed" {
                                return Ok(vec![serde_json::json!({
                                    "hook_event_name": "StopFailure",
                                    "session_id": params["threadId"],
                                    "turn_id": id,
                                    "last_assistant_message": turn["error"]["message"].as_str().unwrap_or("Codex turn failed."),
                                })]);
                            }
                        },
                        _ => {},
                    }
                }
            }
        }
        // Server requests have their own id namespace (eg approval requests).
        if value.get("method").is_some() {
            return Ok(Vec::new());
        }
        let Some((method, ephemeral)) = value
            .get("id")
            .and_then(|id| self.pending.remove(&id.to_string()))
        else {
            return Ok(Vec::new());
        };
        let thread = &value["result"]["thread"];
        let Some(id) = thread["id"].as_str() else {
            return Ok(Vec::new());
        };
        if ephemeral || thread["ephemeral"].as_bool() == Some(true) {
            self.ephemeral.insert(id.to_owned());
            return Ok(Vec::new());
        }
        // A new thread replacing the selected one is the UI's `/new`: the same shape as
        // Claude's `/clear`, so it is reported as one, an end for the old conversation and a
        // `clear` start for the new one, and a submitted `/new` carries its input across.
        let replaced = self
            .selected
            .clone()
            .filter(|previous| method == "thread/start" && previous != id);
        // Publish before the UI sees the reply and can submit its first turn.
        self.select(id, registration)?;
        // Resuming an active thread may not replay its turn/started notification.
        self.current_turn = thread["turns"].as_array().and_then(|turns| {
            turns
                .iter()
                .rev()
                .find(|turn| turn["status"] == "inProgress")
                .and_then(|turn| turn["id"].as_str())
                .map(str::to_owned)
        });
        let source = match method.as_str() {
            "thread/resume" => "resume",
            "thread/fork" => "fork",
            _ if replaced.is_some() => "clear",
            _ => "startup",
        };
        let mut hooks = Vec::new();
        if let Some(previous) = replaced {
            hooks.push(serde_json::json!({
                "hook_event_name": "SessionEnd",
                "session_id": previous,
                "reason": "clear",
            }));
        }
        let mut hook = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": id,
            "source": source,
        });
        // Missing or active status must never grant bootstrap readiness.
        hook["_yinz_codex_hooks_ready"] = (thread["status"]["type"] == "idle").into();
        if let Some(cwd) = thread["cwd"].as_str() {
            hook["cwd"] = cwd.into();
        }
        hooks.push(hook);
        Ok(hooks)
    }
}

async fn connections(
    listener: tokio::net::UnixListener,
    program: &Path,
    registration: &Registration,
) -> Result<()> {
    let mut pending = OptionFuture::from(None);
    let mut active = OptionFuture::from(None);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accept Codex UI")?;
                pending = Some(Box::pin(async move {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        tokio_tungstenite::accept_async_with_config(stream, Some(websocket_config())),
                    )
                    .await
                    .context("timed out accepting Codex UI connection")?
                    .context("accept Codex UI connection")
                })).into();
            },
            Some(result) = &mut pending => {
                pending = None.into();
                match result {
                    Ok(ui) => {
                        // Drop the old relay and its children before polling the replacement.
                        active = Some(Box::pin(proxy(ui, program, registration))).into();
                    },
                    Err(error) => Err::<(), _>(error).non_fatal(),
                }
            },
            Some(result) = &mut active => {
                active = None.into();
                // Connection failures must not end the UI's lifetime; Codex reconnects.
                result.non_fatal();
            },
        }
    }
}

async fn proxy(
    ui: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    program: &Path,
    registration: &Registration,
) -> Result<()> {
    let (backend, stream) = daemon_connection(program, registration.backend_socket.as_deref())?;
    let result = relay(ui, stream, registration, program).await;
    drop(backend);
    result
}

// Keep the proxy child alive for the lifetime of its WebSocket; cancellation kills it.
fn daemon_connection(
    program: &Path,
    socket: Option<&Path>,
) -> Result<(
    tokio::process::Child,
    impl AsyncRead + AsyncWrite + Unpin + use<>,
)> {
    let mut command = tokio::process::Command::new(program);
    command.args(["app-server", "proxy"]);
    if let Some(socket) = socket {
        command.arg("--sock").arg(socket);
    }
    let mut backend = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("connect to the Codex daemon")?;
    let input = backend.stdin.take().context("open Codex daemon input")?;
    let output = backend.stdout.take().context("open Codex daemon output")?;
    Ok((backend, tokio::io::join(output, input)))
}

async fn capability_request<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    ws.send(Message::text(
        serde_json::json!({"id": id, "method": method, "params": params}).to_string(),
    ))
    .await
    .context("send Codex capability request")?;
    while let Some(message) = ws.next().await {
        match message.context("read Codex capability response")? {
            Message::Text(text) => {
                let response: Value =
                    serde_json::from_str(&text).context("parse Codex capability response")?;
                if response.get("method").is_none() && response["id"] == id {
                    if let Some(error) = response.get("error") {
                        if error["data"]["config_write_error_code"] == "configVersionConflict" {
                            return Err(anyhow!(ConfigVersionConflict));
                        }
                        return Err(anyhow!("Codex {method}: {error}"));
                    }
                    return response
                        .get("result")
                        .cloned()
                        .context("Codex capability response has no result");
                }
            },
            Message::Ping(_) | Message::Pong(_) => ws
                .flush()
                .await
                .context("flush Codex capability control frame")?,
            _ => return Err(anyhow!("Codex capability connection closed")),
        }
    }
    Err(anyhow!("Codex capability connection ended"))
}

async fn verify_capability(program: &Path, hook: &Value, socket: Option<&Path>) -> Result<()> {
    if hook["_yinz_codex_hooks_ready"] != true {
        return Err(anyhow!("selected Codex conversation is not idle"));
    }
    let cwd = hook["cwd"]
        .as_str()
        .context("selected Codex conversation has no working directory")?;
    let root = std::env::var_os("CODEX_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".codex"))
        })
        .context("cannot locate Codex reporting script")?;
    let script = root.join("yinz/yinz-hook.sh");
    let (_child, stream) = daemon_connection(program, socket)?;
    let (mut ws, _) = tokio_tungstenite::client_async_with_config(
        "ws://localhost",
        stream,
        Some(websocket_config()),
    )
    .await
    .context("connect Codex capability WebSocket")?;
    check_capability(
        &mut ws,
        hook["session_id"]
            .as_str()
            .context("missing selected Codex thread id")?,
        cwd,
        &script,
    )
    .await
}

async fn check_capability<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    thread: &str,
    cwd: &str,
    script: &Path,
) -> Result<()> {
    capability_request(
        ws,
        0,
        "initialize",
        serde_json::json!({
            "clientInfo": {"name": "yinz-hooks-check", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
        }),
    )
    .await?
    .as_object()
    .context("malformed Codex initialize response")?;
    ws.send(Message::text(r#"{"method":"initialized"}"#))
        .await
        .context("initialize Codex capability connection")?;
    let mut cursor = Value::Null;
    let mut request_id = 1;
    loop {
        let features = capability_request(
            ws,
            request_id,
            "experimentalFeature/list",
            serde_json::json!({
                "threadId": thread, "cursor": cursor
            }),
        )
        .await?;
        let data = features["data"]
            .as_array()
            .context("malformed Codex feature list")?;
        if let Some(hooks) = data.iter().find(|feature| feature["name"] == "hooks") {
            if hooks["enabled"] != true {
                return Err(anyhow!("Codex hooks feature is disabled"));
            }
            break;
        }
        cursor = features["nextCursor"].clone();
        if !cursor.is_string() {
            return Err(anyhow!("Codex hooks feature is unavailable"));
        }
        request_id += 1;
    }
    let hooks = capability_request(
        ws,
        request_id + 1,
        "hooks/list",
        serde_json::json!({"cwds": [cwd]}),
    )
    .await?;
    verify_handlers(&hooks, cwd, script)
}

fn verify_handlers(response: &Value, cwd: &str, script: &Path) -> Result<()> {
    let metadata = std::fs::metadata(script).context("inspect Codex reporting script")?;
    if !metadata.is_file() {
        return Err(anyhow!("Codex reporting script is not a regular file"));
    }
    nix::unistd::access(script, nix::unistd::AccessFlags::X_OK)
        .context("Codex reporting script is not executable")?;
    let command = format!("'{}'", script.display().to_string().replace('\'', r"'\''"));
    let entries = response["data"]
        .as_array()
        .context("malformed Codex hook list")?;
    let entry = entries
        .iter()
        .find(|entry| entry["cwd"] == cwd)
        .context("Codex hook list omits selected directory")?;
    if !entry["errors"]
        .as_array()
        .context("malformed Codex hook errors")?
        .is_empty()
    {
        return Err(anyhow!("Codex hook configuration has errors"));
    }
    let hooks = entry["hooks"]
        .as_array()
        .context("malformed Codex hook handlers")?;
    for event in yinz_types::agent::CODEX_HOOK_EVENTS {
        let event = format!("{}{}", event[..1].to_ascii_lowercase(), &event[1..]);
        if !hooks.iter().any(|hook| {
            hook["eventName"] == event
                && hook["handlerType"] == "command"
                && hook["command"] == command
                && hook["enabled"] == true
                && matches!(hook["trustStatus"].as_str(), Some("trusted" | "managed"))
                && hook.get("async").is_none_or(|value| value == false)
                && hook.get("matcher").is_none_or(Value::is_null)
        }) {
            return Err(anyhow!("Codex {event} has no enabled, trusted, synchronous Yinz reporting handler without a matcher"));
        }
    }
    Ok(())
}

async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    mut ui: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    stream: S,
    registration: &Registration,
    program: &Path,
) -> Result<()> {
    let (mut backend, _) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::client_async_with_config(
            "ws://localhost",
            stream,
            Some(websocket_config()),
        ),
    )
    .await
    .context("timed out establishing Codex daemon WebSocket connection")?
    .context("establish Codex daemon WebSocket connection")?;
    let mut conversations = Conversations::default();
    loop {
        tokio::select! {
            message = ui.next() => {
                let Some(message) = message else {
                    backend.close(None).await.context("close Codex daemon connection")?;
                    break;
                };
                match message.context("read Codex UI message")? {
                    Message::Text(text) => {
                        let mut value: Value = serde_json::from_str(&text).context("parse Codex UI message")?;
                        conversations.request(&value, registration)?;
                        let text = if inject_pane_env(&mut value, &registration.registry.env) {
                            value.to_string().into()
                        } else {
                            text
                        };
                        backend.send(Message::Text(text)).await.context("forward Codex request")?;
                    },
                    Message::Close(frame) => {
                        // Codex can drop the transport immediately after sending Close.
                        ui.flush().await.context("acknowledge Codex UI closure").non_fatal();
                        backend.close(frame).await.context("close Codex daemon connection")?;
                        break;
                    },
                    Message::Ping(_) | Message::Pong(_) => {
                        ui.flush().await.context("flush Codex connection control frame")?;
                    },
                    _ => return Err(anyhow!("Codex UI sent a non-text message")),
                }
            },
            message = backend.next() => {
                let Some(message) = message else {
                    ui.close(None).await.context("close Codex UI connection")?;
                    break;
                };
                match message.context("read Codex daemon reply")? {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text).context("parse Codex daemon reply")?;
                        for hook in conversations.response(&value, registration)? {
                            if hook["hook_event_name"] == "SessionStart" {
                                registration.announce(hook, program).await;
                            } else {
                                registration.report(hook).await;
                            }
                        }
                        ui.send(Message::Text(text)).await.context("forward Codex daemon reply")?;
                    },
                    Message::Close(frame) => {
                        backend.flush().await.context("acknowledge Codex daemon closure").non_fatal();
                        ui.close(frame).await.context("close Codex UI connection")?;
                        break;
                    },
                    Message::Ping(_) | Message::Pong(_) => {
                        backend.flush().await.context("flush Codex daemon control frame")?;
                    },
                    _ => return Err(anyhow!("Codex daemon sent a non-text message")),
                }
            },
        }
    }
    Ok(())
}

const PRIVATE_BACKEND: &str = "/run/user/codex-backend.sock";

fn sandboxed() -> bool {
    std::env::var("YINZ_BUBBLEWRAP").as_deref() == Ok("1")
}

async fn private_backend(
    program: &Path,
    socket: &Path,
    overrides: &[String],
) -> Result<tokio::process::Child> {
    let mut child = tokio::process::Command::new(program)
        .args([
            "app-server",
            "--listen",
            &format!("unix://{}", socket.display()),
        ])
        .args(overrides.iter().flat_map(|value| ["-c", value.as_str()]))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start private Codex backend")?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(20));
        loop {
            if let Some(status) = child.try_wait().context("inspect private Codex backend")? {
                bail!("private Codex backend exited: {status}");
            }
            if tokio::net::UnixStream::connect(socket).await.is_ok() {
                return Ok(());
            }
            poll.tick().await;
        }
    })
    .await
    .context("private Codex backend did not become ready")??;
    Ok(child)
}

/// Run the ordinary Codex UI through a pane-aware connection to its backend.
pub fn launch(program: &Path, args: &[String]) -> Result<i32> {
    let result = launch_inner(program, args);
    result.map(yinz_process::agent_theme::exit_status)
}

fn launch_inner(program: &Path, args: &[String]) -> Result<std::process::ExitStatus> {
    let env: HashMap<_, _> = ROUTE_ENV
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
        .collect();
    let options = bridge_options(args).filter(|_| {
        ["YINZ_PANE_REF", "YINZ_REMOTE_PANE_TOKEN"]
            .iter()
            .any(|key| env.get(*key).is_some_and(|value| !value.is_empty()))
    });
    let Some(options) = options else {
        let _theme = yinz_process::agent_theme::LaunchGuard::start("codex", program, args);
        let mut ui =
            yinz_process::agent_theme::AgentChild::spawn(Command::new(program).args(args))?;
        return ui.wait();
    };
    let cwd = match &options.cd {
        Some(cd) => std::path::absolute(cd),
        None => std::env::current_dir(),
    }
    .context("resolve Codex launcher working directory")?;
    let worktree = worktree_roots(&cwd);
    let mut registration = Registration::new(&directory(), env)?;
    registration.reporter = Some(std::env::current_exe().context("find this yinz binary")?);
    // Remote UIs read trust from the backend before their thread config overrides take effect.
    if !options.project_overrides.is_empty() && registration.backend_socket.is_none() {
        std::fs::set_permissions(
            registration.registry.dir.path(),
            std::fs::Permissions::from_mode(0o700),
        )
        .context("protect private Codex backend directory")?;
        registration.backend_socket = Some(registration.registry.dir.path().join("backend.sock"));
    }
    if registration.backend_socket.is_none() {
        let status = Command::new(program)
            .args(["app-server", "daemon", "start"])
            .stdout(Stdio::null())
            .status()
            .context("start the Codex daemon")?;
        if !status.success() {
            return Err(anyhow!("Codex could not start its shared daemon"));
        }
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start Codex pane bridge")?
        .block_on(async {
            let _backend = if let Some(socket) = &registration.backend_socket {
                Some(private_backend(program, socket, &options.project_overrides).await?)
            } else {
                None
            };
            if let Some((root, primary)) = &worktree {
                // Establish worktree trust before the UI starts.
                if !register_worktree_trust(
                    program,
                    root,
                    primary,
                    registration.backend_socket.as_deref(),
                )
                .await
                .context(
                    "establish Codex worktree trust; inspect Codex config and retry the launch",
                )? {
                    return Ok(std::process::ExitStatus::from_raw(1 << 8));
                }
            }
            let path = registration.registry.dir.path().join("ui.sock");
            yinz_process::pty_host::protocol::check_socket_path_length(&path, 0)?;
            let listener = tokio::net::UnixListener::bind(&path).context("listen for Codex UI")?;
            let mut command = Command::new(program);
            command
                .arg("--remote")
                .arg(format!("unix://{}", path.display()));
            // A shared daemon may have started in a different pane's working directory.
            if options.cd.is_none() {
                command.arg("--cd").arg(&cwd);
            }
            let _theme = yinz_process::agent_theme::LaunchGuard::start("codex", program, args);
            let mut ui = yinz_process::agent_theme::AgentChild::spawn(command.args(args))?;
            tokio::select! {
                status = async {
                    loop {
                        if let Some(status) = ui.poll()? { break Ok(status); }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                } => status,
                result = connections(listener, program, &registration) => {
                    result?;
                    Err(anyhow!("Codex pane connection stopped"))
                },
            }
        })
}

/// Resolve the selected linked worktree and primary checkout through Git metadata.
fn worktree_roots(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = Path::new(String::from_utf8(output.stdout).ok()?.strip_suffix('\n')?)
        .canonicalize()
        .ok()?;
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain", "-z"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata = String::from_utf8(output.stdout).ok()?;
    let mut worktrees = metadata
        .split('\0')
        .filter_map(|field| field.strip_prefix("worktree "));
    let primary = Path::new(worktrees.next()?).canonicalize().ok()?;
    (root != primary
        && worktrees.any(|path| {
            Path::new(path)
                .canonicalize()
                .is_ok_and(|path| path == root)
        }))
    .then_some((root, primary))
}

#[derive(Debug)]
struct ConfigVersionConflict;

impl std::fmt::Display for ConfigVersionConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Codex configuration changed since it was read")
    }
}

impl std::error::Error for ConfigVersionConflict {}

async fn register_worktree_trust(
    program: &Path,
    root: &Path,
    primary: &Path,
    socket: Option<&Path>,
) -> Result<bool> {
    let (_child, stream) = daemon_connection(program, socket)?;
    let (mut ws, _) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::client_async_with_config(
            "ws://localhost",
            stream,
            Some(websocket_config()),
        ),
    )
    .await
    .context("timed out connecting Codex trust WebSocket")?
    .context("connect Codex trust WebSocket")?;
    inherit_worktree_trust(&mut ws, root, primary, || {
        let mut output = std::io::stdout().lock();
        writeln!(output, "Codex worktree: {}", root.display())
            .context("display Codex worktree path")?;
        writeln!(
            output,
            "Approval persists trust for this worktree in your Codex configuration."
        )
        .context("explain Codex worktree trust")?;
        write!(output, "Trust this worktree? [y/N] ").context("display Codex trust prompt")?;
        output.flush().context("flush Codex trust prompt")?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("read Codex trust approval; launch cancelled")?;
        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    })
    .await
}

// Bound network waits separately so time spent answering the prompt does not expire the write.
async fn trust_request<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        capability_request(ws, id, method, params),
    )
    .await
    .with_context(|| format!("timed out during Codex {method}; retry the launch"))?
}

async fn inherit_worktree_trust<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    root: &Path,
    primary: &Path,
    mut confirm: impl FnMut() -> Result<bool>,
) -> Result<bool> {
    let root = root.to_str().context("encode Codex worktree path")?;
    let primary = primary
        .to_str()
        .context("encode Codex primary checkout path")?;
    trust_request(
        ws,
        0,
        "initialize",
        serde_json::json!({
            "clientInfo": {"name": "yinz-worktree-trust", "version": env!("CARGO_PKG_VERSION")}
        }),
    )
    .await?
    .as_object()
    .context("malformed Codex initialize response")?;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        ws.send(Message::text(r#"{"method":"initialized"}"#)),
    )
    .await
    .context("timed out initializing Codex trust connection")?
    .context("initialize Codex trust connection")?;
    for attempt in 0..2 {
        let response = trust_request(
            ws,
            1 + attempt * 2,
            "config/read",
            serde_json::json!({
                "includeLayers": true
            }),
        )
        .await?;
        let layers = response["layers"]
            .as_array()
            .context("missing Codex config layers")?;
        let mut users = layers
            .iter()
            .filter(|layer| layer["name"]["type"] == "user" && layer["name"]["profile"].is_null());
        let user = users.next().context("missing Codex base user config")?;
        if users.next().is_some() || !user["disabledReason"].is_null() {
            return Err(anyhow!("ambiguous or disabled Codex base user config"));
        }
        let config = user["config"]
            .as_object()
            .context("malformed Codex user config")?;
        let projects = config
            .get("projects")
            .map(|projects| {
                projects
                    .as_object()
                    .context("malformed Codex projects config")
            })
            .transpose()?;
        let trust = |path| -> Result<Option<&str>> {
            let Some(entry) = projects.and_then(|projects| projects.get(path)) else {
                return Ok(None);
            };
            let entry = entry
                .as_object()
                .context("malformed Codex project config")?;
            match entry.get("trust_level") {
                None => Ok(None),
                Some(Value::String(level)) if matches!(level.as_str(), "trusted" | "untrusted") => {
                    Ok(Some(level))
                },
                _ => Err(anyhow!("malformed Codex project trust level")),
            }
        };
        let worktree_trust = trust(root)?;
        if worktree_trust == Some("trusted") {
            return Ok(true);
        }
        let inherited = worktree_trust.is_none() && trust(primary)? == Some("trusted");
        let version = user["version"]
            .as_str()
            .filter(|v| !v.is_empty())
            .context("missing Codex user config version")?;
        let file = user["name"]["file"]
            .as_str()
            .context("missing Codex user config path")?;
        // Capture the author's version before approval, never refresh it before writing.
        if !inherited && !confirm()? {
            return Ok(false);
        }
        let result = trust_request(
            ws,
            2 + attempt * 2,
            "config/value/write",
            serde_json::json!({
                "keyPath": "projects", "value": {root: {"trust_level": "trusted"}},
                "mergeStrategy": "upsert", "filePath": file, "expectedVersion": version
            }),
        )
        .await;
        match result {
            Err(error) if inherited && attempt == 0 && error.is::<ConfigVersionConflict>() => {
                continue
            },
            Err(error) if error.is::<ConfigVersionConflict>() => {
                return Err(error.context("Codex trust changed while awaiting approval; retry the launch to review the current decision"));
            },
            Err(error) => return Err(error),
            Ok(response) => {
                if !matches!(response["status"].as_str(), Some("ok" | "okOverridden")) {
                    return Err(anyhow!("malformed Codex config write response"));
                }
                return Ok(true);
            },
        }
    }
    unreachable!("the second write attempt returns")
}

/// Dispatch a hook by its conversation, never by the shared daemon's inherited pane.
pub fn report(yinz: &Path) -> Result<()> {
    yinz_runtime::agent::report::traced_report("codex", |trace| {
        if let Ok(mut trace) = trace.lock() {
            trace.enter("codex_route");
        }
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .context("read Codex hook")?;
        if let Ok(mut trace) = trace.lock() {
            trace.payload_bytes = raw.len();
        }
        let event: Value = serde_json::from_str(&raw).context("parse Codex hook")?;
        if let Ok(mut trace) = trace.lock() {
            trace.event = event["hook_event_name"].as_str().unwrap_or_default().into();
        }
        let thread = event["session_id"]
            .as_str()
            .context("Codex hook has no conversation id")?;
        let binding = binding_for(&directory(), thread)?;
        let env = match binding {
            Some(binding) => binding.env,
            None => {
                // Standalone Codex still runs underneath its pane's original process. A daemon
                // is detached from that ancestry, so an unregistered daemon hook must be ignored.
                let env: HashMap<_, _> = ROUTE_ENV
                    .iter()
                    .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
                    .collect();
                if !owned_by_ancestor(&env) {
                    if let Ok(trace) = trace.lock() {
                        trace.record("codex_route", trace.started.elapsed(), "unbound");
                    }
                    return Ok(());
                }
                env
            },
        };
        let raw = prune_for_forward(raw)?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("start Codex hook forwarding")?
            .block_on(forward(yinz, &env, raw.as_bytes()))
    })
}

fn prune_for_forward(raw: String) -> Result<String> {
    yinz_runtime::agent::hooks::prune_for_forward("codex", raw).context("invalid Codex hook JSON")
}

/// Deliver one raw Codex hook to the pane that `env` identifies.
async fn forward(yinz: &Path, env: &HashMap<String, String>, raw: &[u8]) -> Result<()> {
    let mut child = tokio::process::Command::from(yinz_runtime::agent::report::forwarding_command(
        yinz, "codex", env,
    ))
    .kill_on_drop(true)
    .spawn()
    .context("forward Codex hook to its pane")?;
    let mut input = child.stdin.take().context("open Codex report input")?;
    input
        .write_all(raw)
        .await
        .context("write Codex hook to its pane")?;
    drop(input);
    if !child
        .wait()
        .await
        .context("wait for Codex hook report")?
        .success()
    {
        return Err(anyhow!("Codex pane did not accept its hook report"));
    }
    Ok(())
}

fn owned_by_ancestor(env: &HashMap<String, String>) -> bool {
    let remote = env
        .get("YINZ_REMOTE_PANE_TOKEN")
        .is_some_and(|token| !token.is_empty());
    let child = if remote {
        None
    } else if let Some(reference) = env.get("YINZ_PANE_REF") {
        if reference.len() != 12 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        let Ok(status) =
            yinz_process::pty_host::client::query(&yinz_process::pty_host::protocol::socket_path(
                &yinz_types::internal_ids::SupervisorId::new(reference.clone()),
            ))
        else {
            return false;
        };
        let Some(child) = status.child_pid else {
            return false;
        };
        if status.host_id.as_str() != reference || !status.attached || status.session_name.is_none()
        {
            return false;
        }
        Some(child.get())
    } else {
        return false;
    };
    let mut system = sysinfo::System::new();
    let executable = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .ok();
    let refresh = sysinfo::ProcessRefreshKind::nothing()
        .with_cmd(sysinfo::UpdateKind::OnlyIfNotSet)
        .with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
    let mut pid = Some(sysinfo::Pid::from_u32(std::process::id()));
    for _ in 0..128 {
        let Some(current) = pid else { break };
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[current]),
            true,
            refresh,
        );
        let Some(process) = system.process(current) else {
            break;
        };
        if Some(current.as_u32()) == child {
            return true;
        }
        // Remote supervisors deliberately omit a local pane reference. Their child still
        // inherits the broker token, but a shared Codex daemon has no supervisor ancestor.
        if remote
            && process
                .cmd()
                .get(1)
                .is_some_and(|arg| arg == yinz_process::pty_host::PTY_HOST_FLAG)
            && process
                .exe()
                .and_then(|exe| exe.canonicalize().ok())
                .is_some_and(|exe| Some(exe) == executable)
        {
            return true;
        }
        pid = process.parent();
    }
    false
}

#[cfg(test)]
#[path = "unit/codex_tests.rs"]
mod tests;
