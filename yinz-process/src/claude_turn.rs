//! Detached, host-local observation of Claude and AGY turn boundaries.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::HookEvent;

mod notifications;
mod permissions;
mod prompts;
mod recovery;

pub const OBSERVER_FLAG: &str = "--claude-turn-observer";

/// Resume presentation observation for a local pane whose provider has gone quiet.
/// The foreground group anchors discovery; registry files alone never establish ownership.
pub fn observe_restored(
    root: u32,
    conversation: &str,
    publish: impl Fn(Option<yinz_types::agent::AgentState>) -> bool,
) {
    let Some(identity) = restored_claude_process(root) else {
        return;
    };
    // sysinfo keeps `/proc/<pid>/stat` open for every process it has loaded, so
    // this day-long poll must never reuse the host-wide discovery scan.
    let mut system = System::new();
    let pid = sysinfo::Pid::from_u32(identity.0);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_environ(UpdateKind::Always),
    );
    let Some(process) = system.process(pid).filter(|p| p.start_time() == identity.1) else {
        return;
    };
    let Some(reader) = ClaudeStatusObservation::for_process(process) else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(24 * 60 * 60);
    while Instant::now() < deadline {
        let pid = sysinfo::Pid::from_u32(identity.0);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        if !system.process(pid).is_some_and(|p| {
            p.start_time() == identity.1 && p.status() != sysinfo::ProcessStatus::Zombie
        }) {
            return;
        }
        if reader
            .process_record(
                identity.0,
                identity.1.saturating_sub(1).saturating_mul(1000),
            )
            .is_some_and(|record| record["sessionId"].as_str() != Some(conversation))
        {
            return;
        }
        let permission = reader.permission(identity, conversation);
        let state = if permission.is_some_and(|sample| {
            sample.status == yinz_types::agent::claude_permissions::Status::Active
        }) {
            Some(yinz_types::agent::AgentState::Waiting)
        } else {
            restored_state(reader.current(identity, conversation).as_deref())
        };
        if !publish(state) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Find the single Claude process under `root` as `(pid, start time)`.
///
/// The host-wide scan holds an open file per process until it is dropped, so it
/// stays local to discovery.
fn restored_claude_process(root: u32) -> Option<(u32, u64)> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cmd(UpdateKind::Always),
    );
    let mut candidates = Vec::new();
    for (pid, process) in system.processes() {
        if !is_claude_process(process) {
            continue;
        }
        let mut ancestor = Some(*pid);
        for _ in 0..32 {
            let Some(parent) = ancestor else { break };
            if parent.as_u32() == root {
                candidates.push((pid.as_u32(), process.start_time()));
                break;
            }
            ancestor = system.process(parent).and_then(|p| p.parent());
        }
    }
    match candidates.as_slice() {
        [identity] => Some(*identity),
        _ => None,
    }
}

fn restored_state(status: Option<&str>) -> Option<yinz_types::agent::AgentState> {
    use yinz_types::agent::AgentState;
    match status {
        Some("idle") => Some(AgentState::Idle),
        Some("busy" | "shell") => Some(AgentState::Busy),
        // Waiting needs the permission observer's positive human-input evidence.
        _ => None,
    }
}

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

#[derive(Clone, Serialize, Deserialize)]
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
    /// The process-scoped prompt recovery observer, rather than one turn's observer.
    #[serde(default)]
    recovery: bool,
    #[serde(default)]
    permissions: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct ClaudeStatusObservation {
    path: PathBuf,
    after_ms: u64,
}

impl ClaudeStatusObservation {
    fn new(pid: u32) -> Option<Self> {
        Self::from_environment(pid, |key| std::env::var_os(key))
    }

    fn for_process(process: &sysinfo::Process) -> Option<Self> {
        use std::os::unix::ffi::OsStrExt;
        Self::from_environment(process.pid().as_u32(), |key| {
            let prefix = format!("{key}=");
            process.environ().iter().find_map(|entry| {
                entry
                    .as_bytes()
                    .strip_prefix(prefix.as_bytes())
                    .map(|value| std::ffi::OsStr::from_bytes(value).to_owned())
            })
        })
    }

    fn from_environment(
        pid: u32,
        get: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Option<Self> {
        let root = get("CLAUDE_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                get("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".claude"))
            })?;
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

    /// The status of Claude's session registry record (`busy`, `shell`, `idle` or `waiting`),
    /// when it names this process and conversation and changed after `since_ms`.
    fn record(&self, pid: u32, session: &str, since_ms: u64) -> Option<Value> {
        self.process_record(pid, since_ms)
            .filter(|record| record["sessionId"].as_str() == Some(session))
    }

    fn process_record(&self, pid: u32, since_ms: u64) -> Option<Value> {
        // Claude 2.1.312+ rewrites the record whenever its status changes, including after an
        // early Esc that writes no transcript boundary. Missing/older registries leave hooks and
        // transcript observation in charge. Bound reads and reject special files; this optional
        // signal must never block hooks.
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
        let record: Value = serde_json::from_slice(&bytes).ok()?;
        (record["pid"].as_u64() == Some(u64::from(pid))
            && record["kind"] == "interactive"
            && record["statusUpdatedAt"]
                .as_u64()
                .is_some_and(|updated| updated > since_ms))
        .then_some(record)
    }

    fn status(&self, pid: u32, session: &str, since_ms: u64) -> Option<String> {
        self.record(pid, session, since_ms)?["status"]
            .as_str()
            .map(str::to_owned)
    }

    fn permission(
        &self,
        (pid, started): (u32, u64),
        session: &str,
    ) -> Option<yinz_types::agent::claude_permissions::Sample> {
        use yinz_types::agent::{
            claude_notifications::Source,
            claude_permissions::{Sample, Status},
        };
        let record = self.record(pid, session, started.saturating_sub(1).saturating_mul(1000))?;
        let status = match record["status"].as_str()? {
            "busy" | "shell" | "idle" => Status::Inactive,
            "waiting" => match record["waitingFor"].as_str()? {
                "permission prompt" => Status::Active,
                "input needed" | "dialog open" | "worker request" | "sandbox request" => {
                    Status::Inactive
                },
                _ => return None,
            },
            _ => return None,
        };
        Some(Sample {
            source: Source { pid, started },
            updated_ms: record["statusUpdatedAt"].as_u64()?,
            observed_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_millis()
                .try_into()
                .ok()?,
            status,
        })
    }

    /// Whether the registry shows this prompt's turn over: `idle`, or `shell` when only
    /// background Bash remains. `busy` can be the foreground turn itself.
    fn turn_over(&self, pid: u32, session: &str) -> bool {
        self.status(pid, session, self.after_ms)
            .is_some_and(|status| matches!(status.as_str(), "idle" | "shell"))
    }

    /// The current status, from any record this process instance wrote. The record is
    /// rewritten only when its status changes, so background agents that outlive a turn keep
    /// the `busy` written when it started, before that turn's prompt.
    fn current(&self, (pid, started): (u32, u64), session: &str) -> Option<String> {
        self.status(pid, session, started.saturating_sub(1).saturating_mul(1000))
    }
}

/// Whether a registry status shows Claude working: a turn, background agents, workflows or
/// forked skills (`busy`), only background Bash (`shell`), or blocked on a person (`waiting`).
fn working(status: &str) -> Option<bool> {
    match status {
        "idle" => Some(false),
        "busy" | "shell" | "waiting" => Some(true),
        _ => None,
    }
}

/// Start only after the prompt report is delivered, so confirmation cannot overtake it.
/// Each installed reporter may start one; the screen deduplicates by prompt identity.
pub fn start(kind: &str, hook: &HookEvent, route: Route) -> Result<()> {
    if kind == "claude" {
        permissions::start(hook, &route)
            .context("start Claude permission observation")
            .non_fatal();
    }
    if hook.claude_recovered_after.is_some()
        || !matches!(kind, "claude" | "agy")
        || hook.is_worker()
        || !matches!(
            hook.hook_event_name.as_str(),
            "UserPromptSubmit" | "SessionStart" | "SessionEnd"
        )
    {
        return Ok(());
    }
    let result = (|| {
        let process = hook
            .claude_notifications
            .as_ref()
            .and_then(|e| e.source.as_ref())
            .map(|s| (s.pid, s.started))
            .or_else(|| agent_ancestor(kind));
        // A clear/resume can leave the old transcript intact. Host-side lifecycle hooks
        // replace this process's conversation marker so its detached observers stop too.
        let conversation_file = process.map(|(pid, started)| {
            yinz_runtime::consts::YINZ_TMP_DIR.join(format!("{kind}-conversation-{pid}-{started}"))
        });
        if let Some(path) = &conversation_file {
            update_conversation_marker(path, kind, hook)?;
        }
        let observation = Observation {
            recovery: false,
            permissions: false,
            kind: kind.to_owned(),
            hook: hook.clone(),
            route: route.clone(),
            process,
            conversation_file,
            claude_status: process
                .filter(|_| kind == "claude")
                .and_then(|(pid, _)| ClaudeStatusObservation::new(pid)),
        };
        if hook.hook_event_name != "UserPromptSubmit" {
            return Ok(());
        }
        if kind == "claude" {
            // Prompt recovery is independent of prompt identity and parent completion.
            spawn_observer(&Observation {
                recovery: true,
                ..observation.clone()
            })
            .context("start Claude prompt recovery")
            .non_fatal();
        }
        if hook.prompt_id.as_deref().is_some_and(|id| !id.is_empty()) {
            spawn_observer(&observation)?;
        }
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
            unavailable
                .claude_notifications
                .get_or_insert_with(Default::default)
                .unavailable = true;
        } else if kind == "agy" {
            unavailable.agy_turn_complete = Some(false);
        }
        unavailable.last_assistant_message = None;
        return send(&route, kind, &unavailable);
    }
    result
}

fn spawn_observer(observation: &Observation) -> Result<Child> {
    let kind = &observation.kind;
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
    let child = command
        .spawn()
        .with_context(|| format!("start {kind} turn observer"))?;
    initialize_observer(child, observation)
}

fn initialize_observer(mut child: Child, observation: &Observation) -> Result<Child> {
    let result = (|| {
        serde_json::to_writer(
            child.stdin.take().context("open observer input")?,
            observation,
        )
        .with_context(|| format!("write {} turn observation", observation.kind))
    })();
    if let Err(error) = result {
        stop_observer(&mut child);
        return Err(error);
    }
    Ok(child)
}

fn stop_observer(child: &mut Child) {
    child
        .kill()
        .context("stop Claude completion observer")
        .non_fatal();
    child
        .wait()
        .context("reap Claude completion observer")
        .non_fatal();
}

/// Capture the provider incarnation while the synchronous reporter is still its descendant.
pub fn prepare_hook(hook: &mut HookEvent, local: bool) {
    use yinz_types::agent::claude_notifications::{Evidence, Source};
    let evidence = hook
        .claude_notifications
        .get_or_insert_with(Evidence::default);
    if !local {
        evidence.source = None;
    } else if evidence.source.is_none() {
        evidence.source = agent_ancestor("claude").map(|(pid, started)| Source { pid, started });
    }
    if evidence.source.is_none() {
        evidence.unavailable = true;
    }
    if local
        && hook.claude_permission.is_none()
        && matches!(
            hook.hook_event_name.as_str(),
            "PermissionRequest"
                | "PostToolUse"
                | "PostToolUseFailure"
                | "PermissionDenied"
                | "Notification"
        )
        && let Some(source) = evidence.source.as_ref()
    {
        hook.claude_permission = ClaudeStatusObservation::new(source.pid)
            .and_then(|reader| reader.permission((source.pid, source.started), &hook.session_id));
    }
    if !local {
        hook.claude_permission = None;
    }
}

fn is_claude_process(process: &sysinfo::Process) -> bool {
    process.name() == "claude"
        || process.cmd().first().is_some_and(|program| {
            Path::new(program)
                .file_name()
                .is_some_and(|name| name == "claude")
        })
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
            "claude" => is_claude_process(process),
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
    if observation.permissions {
        return permissions::run(&observation);
    }
    if observation.recovery {
        return recovery::run(&observation);
    }
    let completed = follow(&observation);
    if let Ok(turn) = &completed
        && turn.reported
    {
        if turn.background {
            return watch_background(&observation, |hook| {
                send(&observation.route, &observation.kind, hook)
            });
        }
        return Ok(());
    }
    observation.hook.hook_event_name = "Stop".into();
    observation.hook.prompt = None;
    observation.hook.prompt_sha256 = None;
    if observation.kind == "claude" {
        observation.hook.claude_turn_complete = Some(completed.is_ok());
        if completed.is_err() {
            observation
                .hook
                .claude_notifications
                .get_or_insert_with(Default::default)
                .unavailable = true;
        }
    } else if observation.kind == "agy" {
        observation.hook.agy_turn_complete = Some(completed.is_ok());
    }
    if let Ok(turn) = completed {
        observation.hook.last_assistant_message = turn.last_text;
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
    yinz_runtime::shared::ensure_secure_dir(
        path.parent().context("conversation marker has no parent")?,
    )
    .with_context(|| format!("secure {kind} observation directory"))?;
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
    follow_with_api_reporting(observation, api, |hook| {
        send(&observation.route, &observation.kind, hook)
    })
}

fn follow_with_api_reporting(
    observation: &Observation,
    api: Option<AgyApi>,
    mut report: impl FnMut(&HookEvent) -> Result<()>,
) -> Result<Turn> {
    follow_with_limits(
        observation,
        api,
        &mut report,
        Duration::from_secs(yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS),
        Duration::from_millis(yinz_types::agent::control::MAX_TIMEOUT_MS),
    )
}

fn follow_with_limits(
    observation: &Observation,
    api: Option<AgyApi>,
    mut report: impl FnMut(&HookEvent) -> Result<()>,
    creation_window: Duration,
    observation_window: Duration,
) -> Result<Turn> {
    let kind = &observation.kind;
    let (process_id, _) = observation
        .process
        .with_context(|| format!("cannot identify {kind} process"))?;
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
    let alive = |system: &mut System| alive(system, observation);
    // The first prompt's file can be created only after UserPromptSubmit hooks return.
    let timeout_seconds = match kind.as_str() {
        "agy" => yinz_types::agent::AGY_HOOK_TIMEOUT_SECONDS,
        _ => yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS,
    };
    let deadline = Instant::now() + observation_window;
    let creation_deadline = Instant::now()
        + if kind == "claude" {
            creation_window
        } else {
            Duration::from_secs(timeout_seconds)
        };
    let mut unavailable_reported = false;
    let file = loop {
        alive(&mut system)?;
        match options.open(path) {
            Ok(file) => break file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && Instant::now()
                        < if kind == "claude" {
                            deadline
                        } else {
                            creation_deadline
                        } =>
            {
                if kind == "claude" && !unavailable_reported && Instant::now() >= creation_deadline
                {
                    let mut hook = observation.hook.clone();
                    hook.hook_event_name = "Stop".into();
                    hook.prompt = None;
                    hook.prompt_sha256 = None;
                    hook.claude_turn_complete = Some(false);
                    log::debug!(
                        "Claude transcript acquisition retry process={:?} prompt={:?}",
                        observation.process,
                        hook.prompt_id
                    );
                    // This report is advisory; keep ownership of the original observation.
                    if report(&hook).is_ok() {
                        unavailable_reported = true;
                    }
                }
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
    let mut completed = false;
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
            let read = reader
                .by_ref()
                .take((16 * 1024 * 1024 + 1 - line.len()) as u64)
                .read_until(b'\n', &mut line);
            let bytes = match read {
                Ok(bytes) => bytes,
                Err(error)
                    if kind == "claude"
                        && matches!(
                            error.kind(),
                            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                        ) =>
                {
                    break
                },
                Err(error) => {
                    return Err(error).with_context(|| format!("read {kind} transcript record"))
                },
            };
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
            if kind == "claude"
                && let Some(notice) = notifications::accepted(&record, &turn.session, &turn.prompt)
            {
                let mut hook = observation.hook.clone();
                hook.hook_event_name = "NotificationAccepted".into();
                hook.prompt = None;
                hook.claude_notifications
                    .get_or_insert_with(Default::default)
                    .notice = Some(notice.clone());
                report_with_retry(observation, &hook, deadline, &mut report)?;
                turn.notice = Some(notice);
            }
            if !completed && turn.record(&record)? {
                completed = true;
                if kind != "claude" {
                    return Ok(turn);
                }
                confirm(observation, &mut turn, deadline, &mut report)?;
            }
        }
        if completed {
            return Ok(turn);
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
            && !completed
            && turn.found
            && line.is_empty()
            && observation
                .claude_status
                .as_ref()
                .is_some_and(|status| status.turn_over(process_id, &turn.session))
        {
            // Read all records first, including overlapping prompts. Recheck
            // process/conversation identity and transcript stability after the status read.
            alive(&mut system)?;
            let current =
                std::fs::metadata(path).context("inspect Claude transcript after turn end")?;
            if current.ino() == metadata.ino()
                && current.dev() == metadata.dev()
                && current.len() == offset
            {
                confirm(observation, &mut turn, deadline, &mut report)?;
                return Ok(turn);
            }
        }
        // Regular files cannot wake a condition variable; poll only while this exact process lives.
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn confirm(
    observation: &Observation,
    turn: &mut Turn,
    deadline: Instant,
    report: &mut impl FnMut(&HookEvent) -> Result<()>,
) -> Result<()> {
    let mut hook = observation.hook.clone();
    hook.hook_event_name = "Stop".into();
    hook.prompt = None;
    hook.prompt_sha256 = None;
    hook.claude_turn_complete = Some(!turn.interrupted);
    hook.last_assistant_message = turn.last_text.clone();
    if let Some(notice) = &turn.notice {
        if !turn.answer_boundary {
            hook.claude_turn_complete = Some(false);
            hook.claude_notifications
                .get_or_insert_with(Default::default)
                .unavailable = true;
        }
        hook.claude_notifications
            .get_or_insert_with(Default::default)
            .notice = Some(notice.clone());
    }
    hook.claude_background = settle_background(observation);
    hook.claude_answered = Some(turn.answer_boundary);
    hook.claude_interrupted = Some(turn.interrupted);
    report_with_retry(observation, &hook, deadline, report)?;
    turn.reported = true;
    turn.background = hook.claude_background == Some(true);
    Ok(())
}

fn report_with_retry(
    observation: &Observation,
    hook: &HookEvent,
    deadline: Instant,
    report: &mut impl FnMut(&HookEvent) -> Result<()>,
) -> Result<()> {
    let mut system = System::new();
    loop {
        alive(&mut system, observation)?;
        match report(hook) {
            Ok(()) => break,
            Err(error) if Instant::now() >= deadline => {
                return Err(error).context("Claude completion report deadline expired");
            },
            Err(error) => {
                log::debug!(
                    "Claude completion report retry prompt={:?}: {error:#}",
                    hook.prompt_id
                );
                std::thread::sleep(Duration::from_millis(100));
            },
        }
    }
    Ok(())
}

/// How long a turn that ended in the transcript may still read `busy` in the registry before
/// that counts as background work. Claude clears its foreground flag only after Stop hooks.
const BACKGROUND_SETTLE: Duration = Duration::from_secs(1);

/// Whether work outlives a turn the transcript or registry has already ended.
fn settle_background(observation: &Observation) -> Option<bool> {
    let process = observation.process?;
    let status = observation.claude_status.as_ref()?;
    let deadline = Instant::now() + BACKGROUND_SETTLE;
    loop {
        let settled = Instant::now() >= deadline;
        match status.current(process, &observation.hook.session_id) {
            Some(current) if current != "busy" || settled => return working(&current),
            Some(_) => {},
            // A missing registry is no signal, but an existing record can be read mid-rewrite;
            // a confirmation without a signal would replace an older turn's background status.
            None if settled || !status.path.exists() => return None,
            None => {},
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// How long a background watch tolerates an unreadable or unrecognized registry record
/// before treating Claude's status signal as lost.
const BACKGROUND_SIGNAL_GRACE: Duration = Duration::from_secs(2);

/// After a turn confirmed with background work, report once the registry shows Claude idle.
/// Lost identity or signal leaves background work unresolved.
fn watch_background(
    observation: &Observation,
    mut report: impl FnMut(&HookEvent) -> Result<()>,
) -> Result<()> {
    let mut idle = false;
    if let (Some(process), Some(status)) = (observation.process, &observation.claude_status) {
        let mut system = System::new();
        let deadline =
            Instant::now() + Duration::from_millis(yinz_types::agent::control::MAX_TIMEOUT_MS);
        let mut unknown_since = None;
        while Instant::now() < deadline && alive(&mut system, observation).is_ok() {
            match status
                .current(process, &observation.hook.session_id)
                .as_deref()
                .and_then(working)
            {
                Some(true) => unknown_since = None,
                Some(false) => {
                    idle = true;
                    break;
                },
                // A read can land between a rewrite's truncation and its contents. Only a
                // record that stays unreadable or unrecognized counts as a lost signal.
                None => {
                    if unknown_since.get_or_insert_with(Instant::now).elapsed()
                        >= BACKGROUND_SIGNAL_GRACE
                    {
                        break;
                    }
                },
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if !idle {
        return Err(anyhow!("Claude background observation lost its identity or idle signal; completion remains unverified"));
    }
    let mut hook = observation.hook.clone();
    hook.hook_event_name = yinz_types::agent::CLAUDE_BACKGROUND_IDLE.into();
    hook.prompt = None;
    hook.prompt_sha256 = None;
    hook.claude_background = Some(false);
    report(&hook)
}

/// Fails once the observed process exits or its conversation marker changes.
fn alive(system: &mut System, observation: &Observation) -> Result<()> {
    let kind = &observation.kind;
    let (process_id, start_time) = observation
        .process
        .with_context(|| format!("cannot identify {kind} process"))?;
    let pid = sysinfo::Pid::from_u32(process_id);
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
        && std::fs::read_to_string(marker).with_context(|| format!("{kind} conversation ended"))?
            != conversation_marker(kind, &observation.hook)
    {
        return Err(anyhow!("{kind} conversation replaced"));
    }
    Ok(())
}

struct Turn {
    notice: Option<yinz_types::agent::claude_notifications::Notice>,
    answer_boundary: bool,
    interrupted: bool,
    reported: bool,
    kind: String,
    session: String,
    prompt: String,
    found: bool,
    ancestors: HashSet<String>,
    last_text: Option<String>,
    /// Claude's registry still showed work running when this turn was confirmed.
    background: bool,
    step_idx: Option<u64>,
}

impl Turn {
    fn new(kind: &str, hook: &HookEvent) -> Self {
        let step_idx = hook
            .turn_id
            .as_deref()
            .and_then(|id| id.parse::<u64>().ok());
        Self {
            notice: None,
            answer_boundary: false,
            interrupted: false,
            reported: false,
            kind: kind.to_owned(),
            session: hook.session_id.clone(),
            prompt: hook.prompt_id.clone().unwrap_or_default(),
            found: false,
            ancestors: HashSet::new(),
            last_text: None,
            background: false,
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
                && prompt == self.prompt
            {
                self.found = true;
            }
            if !self.found {
                return Ok(false);
            }
            let parent_in_turn = record["parentUuid"]
                .as_str()
                .is_some_and(|parent| self.ancestors.contains(parent));
            // Explicit prompt identity takes precedence over transcript ancestry: steering
            // prompts can descend from this turn without belonging to it. Their descendants
            // must not contribute text or completion boundaries to this observation.
            let linked = record["promptId"]
                .as_str()
                .map_or(parent_in_turn, |prompt| prompt == self.prompt);
            if !linked {
                return Ok(false);
            }
            if let Some(id) = record["uuid"].as_str() {
                self.ancestors.insert(id.to_owned());
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
            self.interrupted = interrupted;
            self.answer_boundary =
                record["type"] == "system" && record["subtype"] == "turn_duration";
            return Ok(interrupted || self.answer_boundary);
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
