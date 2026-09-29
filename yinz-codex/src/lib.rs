//! Bind a Codex UI's selected conversation to its pane while retaining the shared daemon.
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::Message;
use yinz_runtime::errors::prelude::*;

const ROUTE_ENV: &[&str] = &[
    "YINZ_PANE_REF",
    "YINZ_PANE_ID",
    "YINZ_SESSION_NAME",
    "YINZ_REMOTE_PANE_TOKEN",
    "YINZ_SWAMP_OWNER",
    "YINZ_SOCKET_DIR",
];

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
    dir: tempfile::TempDir,
    _lease: File,
    env: HashMap<String, String>,
    /// The yinz binary that announces selected conversations to the pane, if any.
    reporter: Option<PathBuf>,
}

impl Registration {
    fn new(root: &Path, mut env: HashMap<String, String>) -> Result<Self> {
        let sockets = std::path::absolute(&*yinz_runtime::consts::YINZ_SOCK_DIR)
            .context("resolve Codex pane socket directory")?;
        // YINZ_SOCKET_DIR takes the base; YINZ_SOCK_DIR appends the contract version.
        env.insert(
            "YINZ_SOCKET_DIR".into(),
            sockets
                .parent()
                .and_then(Path::to_str)
                .context("encode Codex pane socket directory")?
                .into(),
        );
        std::fs::create_dir_all(root).context("create Codex pane registry")?;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .context("protect Codex pane registry")?;
        let dir = tempfile::Builder::new()
            .prefix("ui-")
            .tempdir_in(root)
            .context("create Codex UI registration")?;
        let lease = File::create(dir.path().join("lease")).context("create Codex UI lease")?;
        lease.lock().context("hold Codex UI lease")?;
        Ok(Self {
            dir,
            _lease: lease,
            env,
            reporter: None,
        })
    }

    /// Codex runs `SessionStart` hooks only when a conversation's first turn begins, so the
    /// pane would otherwise stay unverified until someone types into it.
    fn announce(&self, hook: Value) {
        let Some(yinz) = self.reporter.clone() else {
            return;
        };
        let env = self.env.clone();
        // The UI's reply must not wait for the pane to accept the report.
        tokio::task::spawn_blocking(move || {
            forward(&yinz, env, hook.to_string().as_bytes())
                .context("announce Codex conversation to its pane")
                .non_fatal();
        });
    }

    fn select(&self, thread_id: &str) -> Result<()> {
        if thread_id.is_empty() || thread_id.len() > 256 {
            return Err(anyhow!("invalid Codex conversation id"));
        }
        let binding = Binding {
            thread_id: thread_id.into(),
            env: self.env.clone(),
        };
        let mut file = tempfile::NamedTempFile::new_in(self.dir.path())
            .context("prepare Codex pane binding")?;
        serde_json::to_writer(&mut file, &binding).context("write Codex pane binding")?;
        file.persist(self.dir.path().join("binding.json"))
            .context("publish Codex pane binding")?;
        Ok(())
    }
}

fn binding_for(root: &Path, thread_id: &str) -> Result<Option<Binding>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read Codex pane registry"),
    };
    // ponytail: scan the open UIs; index by conversation if thousands of panes make this hot.
    let mut latest = None;
    for entry in entries {
        let entry = entry.context("read Codex UI registration")?;
        let read = || -> Result<Option<_>> {
            let lease = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(entry.path().join("lease"))
                .context("open Codex UI lease")?;
            match lease.try_lock() {
                // A dead launcher cannot own a conversation, even after an unclean exit.
                Ok(()) => return Ok(None),
                Err(TryLockError::WouldBlock) => {},
                Err(TryLockError::Error(e)) => return Err(e).context("check Codex UI lease"),
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(entry.path().join("binding.json"))
                .context("open Codex pane binding")?;
            let metadata = file.metadata().context("inspect Codex pane binding")?;
            if !metadata.is_file() || metadata.len() > 16 * 1024 {
                return Ok(None);
            }
            let binding: Binding =
                serde_json::from_reader(file.take(16 * 1024)).context("read Codex pane binding")?;
            (binding.thread_id == thread_id)
                .then(|| metadata.modified().map(|modified| (modified, binding)))
                .transpose()
                .context("read Codex pane binding time")
        };
        // Registrations are created and removed concurrently with hooks.
        if let Ok(Some(candidate)) = read() {
            if latest.as_ref().is_none_or(|(time, _)| candidate.0 > *time) {
                latest = Some(candidate);
            }
        }
    }
    Ok(latest.map(|(_, binding)| binding))
}

#[derive(Default)]
struct Conversations {
    /// Request ids of conversation selections awaiting their reply, and whether each asked
    /// for an ephemeral thread.
    pending: HashMap<String, (String, bool)>,
    /// Hidden helper threads, such as the TUI's title generator, which never own the pane.
    ephemeral: HashSet<String>,
}

impl Conversations {
    fn request(&mut self, value: &Value, registration: &Registration) -> Result<()> {
        let Some(method) = value["method"].as_str() else {
            return Ok(());
        };
        if matches!(method, "thread/start" | "thread/resume" | "thread/fork") {
            if let Some(id) = value.get("id") {
                let ephemeral = value["params"]["ephemeral"].as_bool() == Some(true);
                self.pending
                    .insert(id.to_string(), (method.to_owned(), ephemeral));
            }
        }
        // Also handles a UI returning to an already loaded conversation without resuming it.
        if matches!(method, "turn/start" | "turn/steer") {
            if let Some(thread) = value["params"]["threadId"].as_str() {
                if !self.ephemeral.contains(thread) {
                    registration.select(thread)?;
                }
            }
        }
        Ok(())
    }

    /// Returns a `SessionStart` hook announcing a newly selected conversation.
    fn response(&mut self, value: &Value, registration: &Registration) -> Result<Option<Value>> {
        // Server requests have their own id namespace (eg approval requests).
        if value.get("method").is_some() {
            return Ok(None);
        }
        let Some((method, ephemeral)) = value
            .get("id")
            .and_then(|id| self.pending.remove(&id.to_string()))
        else {
            return Ok(None);
        };
        let thread = &value["result"]["thread"];
        let Some(id) = thread["id"].as_str() else {
            return Ok(None);
        };
        if ephemeral || thread["ephemeral"].as_bool() == Some(true) {
            self.ephemeral.insert(id.to_owned());
            return Ok(None);
        }
        // Publish before the UI sees the reply and can submit its first turn.
        registration.select(id)?;
        let source = match method.as_str() {
            "thread/resume" => "resume",
            "thread/fork" => "fork",
            _ => "startup",
        };
        let mut hook = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": id,
            "source": source,
        });
        if let Some(cwd) = thread["cwd"].as_str() {
            hook["cwd"] = cwd.into();
        }
        Ok(Some(hook))
    }
}

async fn proxy(
    stream: tokio::net::UnixStream,
    program: &Path,
    registration: &Registration,
) -> Result<()> {
    let ui = tokio_tungstenite::accept_async(stream)
        .await
        .context("accept Codex UI connection")?;
    // Codex owns daemon discovery and connection setup. The child forwards raw WebSocket bytes;
    // dropping it closes this UI's connection without stopping the shared server.
    let mut backend = tokio::process::Command::new(program)
        .args(["app-server", "proxy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("connect to the Codex daemon")?;
    let input = backend.stdin.take().context("open Codex daemon input")?;
    let output = backend.stdout.take().context("open Codex daemon output")?;
    relay(ui, tokio::io::join(output, input), registration).await
}

async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    mut ui: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    stream: S,
    registration: &Registration,
) -> Result<()> {
    let (mut backend, _) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::client_async("ws://localhost", stream),
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
                        let value: Value = serde_json::from_str(&text).context("parse Codex UI message")?;
                        conversations.request(&value, registration)?;
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
                        if let Some(hook) = conversations.response(&value, registration)? {
                            registration.announce(hook);
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

#[derive(Default)]
struct BridgeOptions {
    cd: Option<PathBuf>,
}

// Only known, unambiguous UI arguments may acquire an implicit --remote endpoint.
fn bridge_options(args: &[String]) -> Option<BridgeOptions> {
    let mut options = BridgeOptions::default();
    let mut args = args.iter().map(String::as_str);
    let mut first_positional = true;
    let mut resume_or_fork = false;
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            let (option, inline_value) = if arg.starts_with("--") {
                arg.split_once('=')
                    .map_or((arg, None), |(option, value)| (option, Some(value)))
            } else if matches!(
                arg.as_bytes().get(1),
                Some(b'c' | b'm' | b'p' | b's' | b'C' | b'a')
            ) {
                (
                    &arg[..2],
                    (arg.len() > 2).then(|| arg[2..].trim_start_matches('=')),
                )
            } else {
                return None;
            };
            match option {
                "--config" | "-c" | "--model" | "-m" | "--profile" | "-p" | "--sandbox" | "-s"
                | "--cd" | "-C" | "--ask-for-approval" | "-a" | "--enable" | "--disable"
                | "--local-provider" => {
                    let value = inline_value.or_else(|| args.next());
                    if value.is_none_or(|value| value.is_empty() || value.starts_with('-')) {
                        return None;
                    }
                    if matches!(option, "--config" | "-c")
                        && value.and_then(|value| value.split_once('=').map(|(key, _)| key.trim()))
                            == Some("sandbox_workspace_write.writable_roots")
                    {
                        return None;
                    }
                    if matches!(option, "--cd" | "-C") {
                        options.cd = value.map(PathBuf::from);
                    }
                },
                "--oss"
                | "--full-auto"
                | "--approve-for-me"
                | "--dangerously-bypass-approvals-and-sandbox"
                | "--dangerously-bypass-hook-trust"
                | "--search"
                | "--no-alt-screen"
                | "--strict-config"
                    if inline_value.is_none() => {},
                "--last" | "--all" | "--include-non-interactive"
                    if resume_or_fork && inline_value.is_none() => {},
                // Includes --add-dir (unsupported with --remote), explicit endpoints,
                // standalone launches, help, and variadic --image arguments.
                _ => return None,
            }
        } else if first_positional {
            first_positional = false;
            match arg {
                "resume" | "fork" => resume_or_fork = true,
                "agents"
                | "exec"
                | "e"
                | "review"
                | "login"
                | "logout"
                | "mcp"
                | "plugin"
                | "app-server"
                | "remote-control"
                | "completion"
                | "update"
                | "doctor"
                | "sandbox"
                | "debug"
                | "apply"
                | "a"
                | "queue"
                | "archive"
                | "delete"
                | "migrate-rollouts"
                | "unarchive"
                | "features"
                | "exec-server"
                | "execpolicy"
                | "tcp-tunnel"
                | "responses-api-proxy"
                | "stdio-to-uds"
                | "cloud"
                | "help" => return None,
                _ => {},
            }
        }
    }
    Some(options)
}

/// Run the ordinary Codex UI through a pane-aware connection to its shared daemon.
pub fn launch(program: &Path, args: &[String]) -> Result<i32> {
    let env: HashMap<_, _> = ROUTE_ENV
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
        .collect();
    let options = bridge_options(args).filter(|_| {
        ["YINZ_PANE_REF", "YINZ_PANE_ID", "YINZ_REMOTE_PANE_TOKEN"]
            .iter()
            .any(|key| env.get(*key).is_some_and(|value| !value.is_empty()))
    });
    let Some(options) = options else {
        return Command::new(program)
            .args(args)
            .status()
            .map(exit_code)
            .context("run Codex");
    };
    let cwd = match &options.cd {
        Some(cd) => std::path::absolute(cd),
        None => std::env::current_dir(),
    }
    .context("resolve Codex launcher working directory")?;
    let trust = worktree_trust(&cwd);
    let mut registration = Registration::new(&directory(), env)?;
    registration.reporter = Some(std::env::current_exe().context("find this yinz binary")?);
    let status = Command::new(program)
        .args(["app-server", "daemon", "start"])
        .stdout(Stdio::null())
        .status()
        .context("start the Codex daemon")?;
    if !status.success() {
        return Err(anyhow!("Codex could not start its shared daemon"));
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start Codex pane bridge")?
        .block_on(async {
            let path = registration.dir.path().join("ui.sock");
            yinz_process::pty_host::protocol::check_socket_path_length(&path, 0)?;
            let listener = tokio::net::UnixListener::bind(&path).context("listen for Codex UI")?;
            let mut command = tokio::process::Command::new(program);
            command
                .arg("--remote")
                .arg(format!("unix://{}", path.display()));
            // A shared daemon may have started in a different pane's working directory.
            if options.cd.is_none() {
                command.arg("--cd").arg(&cwd);
            }
            if let Some(trust) = &trust {
                command.args(["--config", trust]);
            }
            let mut ui = command
                .args(args)
                .kill_on_drop(true)
                .spawn()
                .context("start Codex UI")?;
            let connections = async {
                loop {
                    let (stream, _) = listener.accept().await.context("accept Codex UI")?;
                    proxy(stream, program, &registration).await?;
                }
                #[expect(unreachable_code, reason = "the connection loop exits through errors")]
                Ok::<(), anyhow::Error>(())
            };
            tokio::select! {
                status = ui.wait() => status.map(exit_code).context("wait for Codex UI"),
                result = connections => {
                    result?;
                    Err(anyhow!("Codex pane connection stopped"))
                },
            }
        })
}

/// Trust only the linked worktree selected for this launch, without persisting a setting.
fn worktree_trust(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata = String::from_utf8(output.stdout).ok()?;
    let mut lines = metadata.lines();
    let root = lines.next()?;
    let git_dir = lines.next()?;
    let common_dir = lines.next()?;
    if git_dir == common_dir {
        return None;
    }
    // An inline table keeps dots and quotes in paths out of the CLI's dotted-key parser.
    let root = serde_json::Value::String(root.to_owned());
    Some(format!("projects={{{root}={{trust_level=\"trusted\"}}}}"))
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

/// Dispatch a hook by its conversation, never by the shared daemon's inherited pane.
pub fn report(yinz: &Path) -> Result<()> {
    let trace = std::sync::Arc::new(std::sync::Mutex::new(
        yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now()),
    ));
    yinz_runtime::agent::report::reporting_watchdog(trace.clone())?;
    {
        let mut trace = trace
            .lock()
            .map_err(|_| anyhow!("Codex hook trace poisoned"))?;
        trace.kind = "codex".into();
        trace.enter("codex_route");
    }
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("read Codex hook")?;
    let event: Value = serde_json::from_str(&raw).context("parse Codex hook")?;
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
    forward(yinz, env, raw.as_bytes())
}

/// Deliver one raw Codex hook to the pane that `env` identifies.
fn forward(yinz: &Path, env: HashMap<String, String>, raw: &[u8]) -> Result<()> {
    let mut command = Command::new(yinz);
    for key in ROUTE_ENV {
        command.env_remove(key);
    }
    if env
        .get("YINZ_REMOTE_PANE_TOKEN")
        .is_some_and(|token| !token.is_empty())
    {
        command.args(["--pty-broker", "report", "codex"]);
    } else {
        command.args(["action", "agent-report", "--hook-json", "--kind", "codex"]);
    }
    let mut child = command
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("forward Codex hook to its pane")?;
    child
        .stdin
        .take()
        .context("open Codex report input")?
        .write_all(raw)
        .context("write Codex hook to its pane")?;
    if !child
        .wait()
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
    let mut server_socket = None;
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
        Some(child.get())
    } else {
        if env.contains_key("YINZ_REMOTE_PANE_TOKEN")
            || env
                .get("YINZ_PANE_ID")
                .and_then(|id| id.parse::<u32>().ok())
                .is_none()
        {
            return false;
        }
        let Some(session) = env.get("YINZ_SESSION_NAME") else {
            return false;
        };
        if yinz_session::sessions::validate_session_name(session).is_err() {
            return false;
        }
        let Ok(socket) = yinz_runtime::consts::YINZ_SOCK_DIR
            .join(session)
            .canonicalize()
        else {
            return false;
        };
        let Ok(leader) = nix::unistd::getsid(None) else {
            return false;
        };
        server_socket = Some(socket);
        Some(leader.as_raw() as u32)
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
            let Some(socket) = &server_socket else {
                return true;
            };
            // An in-process PTY's session leader is a direct child of this session's
            // server. A detached daemon has a different leader or has been reparented.
            let Some(parent) = process.parent() else {
                return false;
            };
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[parent]),
                true,
                refresh,
            );
            return system.process(parent).is_some_and(|server| {
                server
                    .exe()
                    .and_then(|exe| exe.canonicalize().ok())
                    .is_some_and(|exe| Some(exe) == executable)
                    && matches!(server.cmd(), [_, flag, path, ..]
                        if flag == "--server"
                            && Path::new(path)
                                .canonicalize()
                                .is_ok_and(|path| path == *socket)
                    )
            });
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
