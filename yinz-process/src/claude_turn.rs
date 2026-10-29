//! Detached, host-local observation of Claude and AGY turn boundaries.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::HookEvent;

pub const OBSERVER_FLAG: &str = "--claude-turn-observer";

fn observer_executable() -> Result<PathBuf> {
    // An observer can outlive an install. On Linux current_exe() then ends in
    // " (deleted)", which cannot be spawned. Re-exec the running image instead.
    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from("/proc/self/exe"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().context("resolve turn observer binary")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Route {
    Local {
        session: String,
        pane: String,
    },
    Pane {
        reference: String,
        pane: Option<String>,
    },
    Remote {
        socket: PathBuf,
        pane_token: String,
    },
}

fn default_claude_kind() -> String {
    "claude".into()
}

#[derive(Serialize, Deserialize)]
struct Observation {
    #[serde(default = "default_claude_kind")]
    kind: String,
    hook: HookEvent,
    route: Route,
    process: Option<(u32, u64)>,
    #[serde(default)]
    conversation_file: Option<PathBuf>,
    #[serde(default)]
    claude_status: Option<ClaudeStatusObservation>,
}

#[derive(Serialize, Deserialize)]
struct ClaudeStatusObservation {
    path: PathBuf,
    after_ms: u64,
}

impl ClaudeStatusObservation {
    fn new(pid: u32) -> Option<Self> {
        let root = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))?;
        Some(Self {
            path: root.join("sessions").join(format!("{pid}.json")),
            after_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_millis()
                .try_into()
                .ok()?,
        })
    }

    fn idle(&self, pid: u32, session: &str) -> bool {
        // Claude 2.1.312 publishes status even when an early Esc writes no transcript
        // boundary. Missing/older registries simply leave transcript observation in charge.
        // Bound reads and reject special files; this optional signal must never block hooks.
        let read = || -> Option<Value> {
            use std::os::unix::fs::OpenOptionsExt;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.path)
                .ok()?;
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            let mut bytes = Vec::new();
            file.take(64 * 1024 + 1).read_to_end(&mut bytes).ok()?;
            if bytes.len() > 64 * 1024 {
                return None;
            }
            serde_json::from_slice(&bytes).ok()
        };
        read().is_some_and(|status| {
            status["pid"].as_u64() == Some(u64::from(pid))
                && status["sessionId"].as_str() == Some(session)
                && status["kind"] == "interactive"
                && status["status"] == "idle"
                && status["statusUpdatedAt"]
                    .as_u64()
                    .is_some_and(|updated| updated > self.after_ms)
        })
    }
}

/// Start only after the prompt report is delivered, so confirmation cannot overtake it.
/// Each installed reporter may start one; the screen deduplicates by prompt identity.
pub fn start(kind: &str, hook: &HookEvent, route: Route) -> Result<()> {
    if !matches!(kind, "claude" | "agy")
        || hook.is_worker()
        || !matches!(
            hook.hook_event_name.as_str(),
            "UserPromptSubmit" | "SessionStart" | "SessionEnd"
        )
    {
        return Ok(());
    }
    let result = (|| {
        let process = agent_ancestor(kind);
        // A clear/resume can leave the old transcript intact. Host-side lifecycle hooks
        // replace this process's conversation marker so its detached observers stop too.
        let conversation_file = process.map(|(pid, started)| {
            yinz_runtime::consts::YINZ_TMP_DIR.join(format!("{kind}-conversation-{pid}-{started}"))
        });
        if let Some(path) = &conversation_file {
            update_conversation_marker(path, kind, hook)?;
        }
        if hook.hook_event_name != "UserPromptSubmit"
            || hook.prompt_id.as_deref().is_none_or(str::is_empty)
        {
            return Ok(());
        }
        let observation = Observation {
            kind: kind.to_owned(),
            hook: hook.clone(),
            route: route.clone(),
            process,
            conversation_file,
            claude_status: process
                .filter(|_| kind == "claude")
                .and_then(|(pid, _)| ClaudeStatusObservation::new(pid)),
        };
        let mut command = Command::new(observer_executable()?);
        command
            .arg(OBSERVER_FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid and descriptor closure use only async-signal-safe operations.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                close_fds::close_open_fds(3, &[]);
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("start {kind} turn observer"))?;
        serde_json::to_writer(
            child.stdin.take().context("open observer input")?,
            &observation,
        )
        .with_context(|| format!("write {kind} turn observation"))?;
        Ok(())
    })();
    if result.is_err()
        && hook.hook_event_name == "UserPromptSubmit"
        && hook.prompt_id.as_deref().is_some_and(|id| !id.is_empty())
    {
        let mut unavailable = hook.clone();
        unavailable.hook_event_name = "Stop".into();
        unavailable.prompt = None;
        if kind == "claude" {
            unavailable.claude_turn_complete = Some(false);
        } else if kind == "agy" {
            unavailable.agy_turn_complete = Some(false);
        }
        unavailable.last_assistant_message = None;
        return send(&route, kind, &unavailable);
    }
    result
}

fn agent_ancestor(kind: &str) -> Option<(u32, u64)> {
    let mut system = System::new();
    let mut pid = sysinfo::Pid::from_u32(std::process::id());
    for _ in 0..32 {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        let matches = match kind {
            "claude" => {
                process.name() == "claude"
                    || process.cmd().first().is_some_and(|program| {
                        std::path::Path::new(program)
                            .file_name()
                            .is_some_and(|name| name == "claude")
                    })
            },
            "agy" => {
                let name = process.name().to_string_lossy();
                name == "agy"
                    || name == "antigravity"
                    || process.cmd().first().is_some_and(|program| {
                        std::path::Path::new(program)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name == "agy"
                                    || name == "antigravity"
                                    || yinz_types::agent::kind_of(name).as_deref() == Some("agy")
                            })
                    })
            },
            _ => false,
        };
        if matches {
            return Some((pid.as_u32(), process.start_time()));
        }
        pid = process.parent()?;
    }
    None
}
pub fn run() -> Result<()> {
    let mut observation: Observation =
        serde_json::from_reader(std::io::stdin()).context("read turn observation")?;
    let completed = follow(&observation);
    observation.hook.hook_event_name = "Stop".into();
    observation.hook.prompt = None;
    observation.hook.prompt_sha256 = None;
    if observation.kind == "claude" {
        observation.hook.claude_turn_complete = Some(completed.is_ok());
    } else if observation.kind == "agy" {
        observation.hook.agy_turn_complete = Some(completed.is_ok());
    }
    if let Ok(turn) = completed {
        observation.hook.last_assistant_message = turn.last_text;
        observation.hook.claude_background_tasks = turn.background_tasks;
    } else {
        observation.hook.last_assistant_message = None;
    }
    send(&observation.route, &observation.kind, &observation.hook)
}

fn conversation_marker(kind: &str, hook: &HookEvent) -> String {
    if kind == "agy" {
        serde_json::json!([hook.session_id, hook.prompt_id]).to_string()
    } else {
        hook.session_id.clone()
    }
}

fn update_conversation_marker(path: &Path, kind: &str, hook: &HookEvent) -> Result<()> {
    if hook.hook_event_name == "SessionEnd" {
        let marker = match std::fs::read_to_string(path) {
            Ok(marker) => marker,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| format!("read {kind} conversation marker"));
            },
        };
        let matches_session = if kind == "agy" {
            serde_json::from_str::<(String, Option<String>)>(&marker)
                .map(|(session, _)| session == hook.session_id)
                .unwrap_or_else(|_| {
                    // Older AGY markers contain session: or session:session:<step>.
                    marker == format!("{}:", hook.session_id)
                        || marker
                            .strip_prefix(&format!("{0}:{0}:", hook.session_id))
                            .is_some_and(|step| {
                                !step.is_empty() && step.bytes().all(|b| b.is_ascii_digit())
                            })
                })
        } else {
            marker == hook.session_id
        };
        if matches_session {
            std::fs::remove_file(path)
                .with_context(|| format!("end {kind} conversation observation"))?;
        }
    } else {
        std::fs::create_dir_all(path.parent().context("conversation marker has no parent")?)
            .with_context(|| format!("create {kind} observation directory"))?;
        let temporary = path.with_extension(uuid::Uuid::new_v4().to_string());
        let result = (|| {
            let mut marker = std::fs::File::create_new(&temporary)
                .with_context(|| format!("create {kind} conversation marker"))?;
            marker
                .write_all(conversation_marker(kind, hook).as_bytes())
                .with_context(|| format!("write {kind} conversation marker"))?;
            std::fs::rename(&temporary, path)
                .with_context(|| format!("replace {kind} conversation marker"))
        })();
        if result.is_err() {
            std::fs::remove_file(&temporary)
                .with_context(|| format!("clean up {kind} conversation marker"))
                .non_fatal();
        }
        result?;
    }
    Ok(())
}

fn follow(observation: &Observation) -> Result<Turn> {
    let api = if observation.kind == "agy" {
        Some(AgyApi::new(
            &std::env::var("ANTIGRAVITY_LS_ADDRESS").context("AGY API address missing")?,
            std::env::var("ANTIGRAVITY_CSRF_TOKEN").context("AGY API authentication missing")?,
        )?)
    } else {
        None
    };
    follow_with_api(observation, api)
}

fn follow_with_api(observation: &Observation, api: Option<AgyApi>) -> Result<Turn> {
    let kind = &observation.kind;
    let (process_id, start_time) = observation
        .process
        .with_context(|| format!("cannot identify {kind} process"))?;
    let pid = sysinfo::Pid::from_u32(process_id);
    let path = observation
        .hook
        .transcript_path
        .as_ref()
        .with_context(|| format!("{kind} transcript path missing"))?;
    let mut options = OpenOptions::new();
    options.read(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NONBLOCK);
    let mut system = System::new();
    let alive = |system: &mut System| -> Result<()> {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        if !system.process(pid).is_some_and(|process| {
            process.start_time() == start_time && process.status() != sysinfo::ProcessStatus::Zombie
        }) {
            return Err(anyhow!("{kind} process exited"));
        }
        if let Some(marker) = &observation.conversation_file
            && std::fs::read_to_string(marker)
                .with_context(|| format!("{kind} conversation ended"))?
                != conversation_marker(kind, &observation.hook)
        {
            return Err(anyhow!("{kind} conversation replaced"));
        }
        Ok(())
    };
    // The first prompt's file can be created only after UserPromptSubmit hooks return.
    let timeout_seconds = match kind.as_str() {
        "agy" => yinz_types::agent::AGY_HOOK_TIMEOUT_SECONDS,
        _ => yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
    };
    let creation_deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let file = loop {
        alive(&mut system)?;
        match options.open(path) {
            Ok(file) => break file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && Instant::now() < creation_deadline =>
            {
                std::thread::sleep(Duration::from_millis(100));
            },
            Err(error) => return Err(error).with_context(|| format!("open {kind} transcript")),
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {kind} transcript"))?;
    if !metadata.is_file() {
        return Err(anyhow!("{kind} transcript is not a regular file"));
    }
    let mut reader = BufReader::new(file);
    let mut turn = Turn::new(kind, &observation.hook);
    if kind == "agy"
        && (turn.session.is_empty()
            || turn.step_idx.is_none_or(|step| step == 0)
            || observation.hook.prompt_id.as_deref()
                != turn
                    .step_idx
                    .map(|step| format!("{}:{step}", turn.session))
                    .as_deref())
    {
        return Err(anyhow!("AGY prompt identity missing or inconsistent"));
    }
    let mut line = Vec::new();
    let mut offset = 0;
    let deadline =
        Instant::now() + Duration::from_millis(yinz_types::agent::control::MAX_TIMEOUT_MS);
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow!("{kind} turn observation expired"));
        }
        alive(&mut system)?;
        let current =
            std::fs::metadata(path).with_context(|| format!("{kind} transcript disappeared"))?;
        if current.ino() != metadata.ino()
            || current.dev() != metadata.dev()
            || current.len() < offset
        {
            return Err(anyhow!("{kind} transcript replaced or truncated"));
        }
        loop {
            let bytes = reader
                .by_ref()
                .take((16 * 1024 * 1024 + 1 - line.len()) as u64)
                .read_until(b'\n', &mut line)
                .with_context(|| format!("read {kind} transcript record"))?;
            offset += bytes as u64;
            if line.len() > 16 * 1024 * 1024 {
                return Err(anyhow!("{kind} transcript record exceeds 16 MiB"));
            }
            if line.last() != Some(&b'\n') {
                break;
            }
            let record: Value = serde_json::from_slice(&line)
                .with_context(|| format!("parse {kind} transcript record"))?;
            line.clear();
            if turn.record(&record)? {
                return Ok(turn);
            }
        }
        if let Some(api) = &api
            && turn.found
            && line.is_empty()
            && api.idle(&turn)?
        {
            // A process exit or a newer submission during the request invalidates Idle.
            alive(&mut system)?;
            if std::fs::metadata(path)
                .context("inspect AGY transcript after Idle")?
                .len()
                == offset
            {
                return Ok(turn);
            }
        }
        if kind == "claude"
            && turn.found
            && line.is_empty()
            && observation
                .claude_status
                .as_ref()
                .is_some_and(|status| status.idle(process_id, &turn.session))
        {
            // Read all records first: a newer prompt invalidates this observation. Recheck
            // process/conversation identity and transcript stability after the status read.
            alive(&mut system)?;
            let current =
                std::fs::metadata(path).context("inspect Claude transcript after Idle")?;
            if current.ino() == metadata.ino()
                && current.dev() == metadata.dev()
                && current.len() == offset
            {
                return Ok(turn);
            }
        }
        // Regular files cannot wake a condition variable; poll only while this exact process lives.
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Turn {
    kind: String,
    session: String,
    prompt: String,
    found: bool,
    ancestors: HashSet<String>,
    last_text: Option<String>,
    /// Bash commands and subagents this turn left running in the background, in launch order.
    background_tasks: Vec<String>,
    step_idx: Option<u64>,
}

impl Turn {
    fn new(kind: &str, hook: &HookEvent) -> Self {
        let step_idx = hook
            .turn_id
            .as_deref()
            .and_then(|id| id.parse::<u64>().ok());
        Self {
            kind: kind.to_owned(),
            session: hook.session_id.clone(),
            prompt: hook.prompt_id.clone().unwrap_or_default(),
            found: false,
            ancestors: HashSet::new(),
            last_text: None,
            background_tasks: Vec::new(),
            step_idx,
        }
    }

    fn record(&mut self, record: &Value) -> Result<bool> {
        if self.kind == "claude" {
            if record["isSidechain"] == true {
                return Ok(false);
            }
            if record["sessionId"]
                .as_str()
                .is_some_and(|session| session != self.session)
            {
                return Err(anyhow!("Claude conversation replaced"));
            }
            if record["type"] == "user"
                && let Some(prompt) = record["promptId"].as_str()
            {
                if self.found && prompt != self.prompt {
                    return Err(anyhow!("Claude prompt replaced"));
                }
                if prompt == self.prompt {
                    self.found = true;
                }
            }
            if !self.found {
                return Ok(false);
            }
            let parent_in_turn = record["parentUuid"]
                .as_str()
                .is_some_and(|parent| self.ancestors.contains(parent));
            let linked = record["promptId"].as_str() == Some(&self.prompt) || parent_in_turn;
            if !linked {
                return Ok(false);
            }
            if let Some(id) = record["uuid"].as_str() {
                self.ancestors.insert(id.to_owned());
            }
            if record["type"] == "user"
                && let Some(task) = background_task(&record["toolUseResult"])
                && !self.background_tasks.iter().any(|known| known == task)
                && self.background_tasks.len() < yinz_types::agent::MAX_BACKGROUND_TASKS
            {
                self.background_tasks.push(task.to_owned());
            }
            if record["type"] == "assistant"
                && let Some(content) = record["message"]["content"].as_array()
            {
                let text: String = content
                    .iter()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect();
                if !text.is_empty() {
                    self.last_text = Some(text);
                }
            }
            // Cancellation can end a turn without a turn_duration record. A prompt ID
            // alone is insufficient: the interruption must descend from this turn.
            let interrupted = record["type"] == "user"
                && parent_in_turn
                && record["interruptedMessageId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
                && {
                    let content = &record["message"]["content"];
                    content == "[Request interrupted by user]"
                        || content.as_array().is_some_and(|parts| {
                            parts.iter().any(|part| {
                                part["type"] == "text"
                                    && part["text"] == "[Request interrupted by user]"
                            })
                        })
                };
            return Ok(
                interrupted || (record["type"] == "system" && record["subtype"] == "turn_duration")
            );
        }

        if self.kind == "agy" {
            if record["conversationId"]
                .as_str()
                .is_some_and(|id| id != self.session)
            {
                return Err(anyhow!("AGY conversation replaced"));
            }
            let step = record["step_index"]
                .as_u64()
                .context("AGY transcript step missing")?;
            let expected = self
                .step_idx
                .and_then(|step| step.checked_sub(1))
                .context("AGY prompt step missing")?;
            if record["type"] == "USER_INPUT" {
                if step > expected {
                    return Err(anyhow!("AGY prompt replaced"));
                }
                if step == expected {
                    self.found = true;
                }
                return Ok(false);
            }
            if !self.found || step < expected {
                return Ok(false);
            }
            if record["status"] == "ERROR" || record["type"] == "ERROR" {
                return Err(anyhow!("AGY turn failed"));
            }

            if record.get("type").and_then(Value::as_str) == Some("PLANNER_RESPONSE")
                && let Some(content) = record.get("content").and_then(Value::as_str)
            {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    self.last_text = Some(trimmed.to_owned());
                }
            }
            return Ok(false);
        }

        Ok(false)
    }
}

/// The task a tool result left running: a Bash `run_in_background` command reports
/// `backgroundTaskId`, and an async subagent launch reports `async_launched` with its `agentId`.
/// Claude names the same ID in the `<task-id>` of the notification that ends it.
fn background_task(result: &Value) -> Option<&str> {
    if let Some(task) = result["backgroundTaskId"].as_str() {
        return Some(task).filter(|task| !task.is_empty());
    }
    (result["isAsync"] == true && result["status"] == "async_launched")
        .then(|| result["agentId"].as_str())
        .flatten()
        .filter(|task| !task.is_empty())
}

struct AgyApi {
    client: isahc::HttpClient,
    url: String,
    token: String,
}

impl AgyApi {
    fn new(address: &str, token: String) -> Result<Self> {
        use isahc::prelude::*;
        let address: std::net::SocketAddr = address.parse().context("invalid AGY API address")?;
        if !address.ip().is_loopback() || token.is_empty() {
            return Err(anyhow!(
                "AGY API requires a loopback address and authentication"
            ));
        }
        Ok(Self {
            client: isahc::HttpClient::builder()
                .timeout(Duration::from_secs(2))
                .redirect_policy(isahc::config::RedirectPolicy::None)
                .proxy(None)
                .build()
                .context("create AGY API client")?,
            url: format!("http://{address}/exa.language_server_pb.LanguageServerService/GetCascadeTrajectory"),
            token,
        })
    }

    fn idle(&self, turn: &Turn) -> Result<bool> {
        let request = isahc::Request::post(&self.url)
            .header("Content-Type", "application/json")
            .header("x-codeium-csrf-token", &self.token)
            .body(serde_json::json!({"cascadeId": turn.session}).to_string())
            .map_err(|_| anyhow!("invalid AGY API request"))?;
        let mut response = self
            .client
            .send(request)
            .map_err(|_| anyhow!("AGY API unavailable"))?;
        if !response.status().is_success() {
            return Err(anyhow!("AGY API request rejected"));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow!("read AGY API response"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(anyhow!("AGY API response exceeds 16 MiB"));
        }
        let value: Value = serde_json::from_slice(&bytes).context("invalid AGY API response")?;
        Self::trajectory_idle(turn, &value)
    }

    fn trajectory_idle(turn: &Turn, value: &Value) -> Result<bool> {
        let trajectory = &value["trajectory"];
        if trajectory["cascadeId"].as_str() != Some(&turn.session) {
            return Err(anyhow!("AGY API conversation missing or replaced"));
        }
        let steps = trajectory["steps"]
            .as_array()
            .context("AGY API steps missing")?;
        if value["numTotalSteps"].as_u64() != Some(steps.len() as u64) {
            return Err(anyhow!("AGY API trajectory incomplete"));
        }
        let expected = turn
            .step_idx
            .and_then(|step| step.checked_sub(1))
            .context("AGY prompt step missing")?;
        let latest = steps
            .iter()
            .rposition(|step| step["type"] == "CORTEX_STEP_TYPE_USER_INPUT")
            .context("AGY API prompt missing")?;
        if latest as u64 != expected {
            return Err(anyhow!("AGY API prompt replaced"));
        }
        if steps
            .iter()
            .skip(latest)
            .any(|step| step["status"] == "CORTEX_STEP_STATUS_ERROR")
        {
            return Err(anyhow!("AGY API turn failed"));
        }
        match value["status"].as_str() {
            // A step after the submitted user input proves this execution has started.
            Some("CASCADE_RUN_STATUS_IDLE") => Ok(turn.found && steps.len() > latest + 1),
            Some("CASCADE_RUN_STATUS_RUNNING" | "CASCADE_RUN_STATUS_BUSY") => Ok(false),
            _ => Err(anyhow!("unsupported AGY API status")),
        }
    }
}

pub(crate) fn send(route: &Route, kind: &str, hook: &HookEvent) -> Result<()> {
    let json = hook.broadcast_json(
        serde_json::to_string(hook)
            .with_context(|| format!("serialize {kind} turn confirmation"))?,
    );
    match route {
        Route::Remote { socket, pane_token } => {
            crate::pty_host::remote::broker::report_agent_hook_to(socket, pane_token, kind, &json)
        },
        Route::Local { .. } | Route::Pane { .. } => {
            let mut command = Command::new(observer_executable()?);
            match route {
                Route::Local { session, pane } => {
                    command.args([
                        "--session",
                        session,
                        "action",
                        "agent-report",
                        "--pane-id",
                        pane,
                    ]);
                },
                Route::Pane { reference, pane } => {
                    command
                        .env(crate::pty_host::PANE_REF_ENV_KEY, reference)
                        .args(["action", "agent-report"]);
                    if let Some(pane) = pane {
                        command.args(["--pane-id", pane]);
                    }
                },
                Route::Remote { .. } => unreachable!(),
            }
            let mut child = command
                .args(["--kind", kind, "--hook-json"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("start confirmation reporter")?;
            child
                .stdin
                .take()
                .context("open confirmation input")?
                .write_all(json.as_bytes())
                .context("write confirmation")?;
            if !child
                .wait()
                .context("wait for confirmation reporter")?
                .success()
            {
                return Err(anyhow!("{kind} confirmation was not delivered"));
            }
            Ok(())
        },
    }
}

#[cfg(test)]
#[path = "unit/claude_turn_tests.rs"]
mod tests;
