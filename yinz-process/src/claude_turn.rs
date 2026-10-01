//! Detached, host-local observation of Claude's transcript turn boundary.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::HookEvent;

pub const OBSERVER_FLAG: &str = "--claude-turn-observer";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Route {
    Local { session: String, pane: String },
    Remote { socket: PathBuf, pane_token: String },
}

#[derive(Serialize, Deserialize)]
struct Observation {
    hook: HookEvent,
    route: Route,
    process: Option<(u32, u64)>,
    #[serde(default)]
    conversation_file: Option<PathBuf>,
}

/// Start only after the prompt report is delivered, so confirmation cannot overtake it.
/// Each installed reporter may start one; the screen deduplicates by prompt identity.
pub fn start(kind: &str, hook: &HookEvent, route: Route) -> Result<()> {
    if kind != "claude"
        || hook.is_worker()
        || !matches!(
            hook.hook_event_name.as_str(),
            "UserPromptSubmit" | "SessionStart" | "SessionEnd"
        )
    {
        return Ok(());
    }
    let result = (|| {
        let process = claude_ancestor();
        // A clear/resume can leave the old transcript intact. Host-side lifecycle hooks
        // replace this process's conversation marker so its detached observers stop too.
        let conversation_file = process.map(|(pid, started)| {
            yinz_runtime::consts::YINZ_TMP_DIR.join(format!("claude-conversation-{pid}-{started}"))
        });
        if let Some(path) = &conversation_file {
            if hook.hook_event_name == "SessionEnd" {
                if std::fs::read_to_string(path).ok().as_deref() == Some(&hook.session_id) {
                    std::fs::remove_file(path).context("end Claude conversation observation")?;
                }
            } else {
                std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_TMP_DIR)
                    .context("create Claude observation directory")?;
                let temporary = path.with_extension(uuid::Uuid::new_v4().to_string());
                let result = (|| {
                    let mut marker = std::fs::File::create_new(&temporary)
                        .context("create Claude conversation marker")?;
                    marker
                        .write_all(hook.session_id.as_bytes())
                        .context("write Claude conversation marker")?;
                    std::fs::rename(&temporary, path).context("replace Claude conversation marker")
                })();
                if result.is_err() {
                    std::fs::remove_file(&temporary)
                        .context("clean up Claude conversation marker")
                        .non_fatal();
                }
                result?;
            }
        }
        if hook.hook_event_name != "UserPromptSubmit"
            || hook.prompt_id.as_deref().is_none_or(str::is_empty)
        {
            return Ok(());
        }
        let observation = Observation {
            hook: hook.clone(),
            route: route.clone(),
            process,
            conversation_file,
        };
        let mut command =
            Command::new(std::env::current_exe().context("resolve turn observer binary")?);
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
        let mut child = command.spawn().context("start Claude turn observer")?;
        serde_json::to_writer(
            child.stdin.take().context("open observer input")?,
            &observation,
        )
        .context("write Claude turn observation")?;
        Ok(())
    })();
    if result.is_err()
        && hook.hook_event_name == "UserPromptSubmit"
        && hook.prompt_id.as_deref().is_some_and(|id| !id.is_empty())
    {
        let mut unavailable = hook.clone();
        unavailable.hook_event_name = "Stop".into();
        unavailable.prompt = None;
        unavailable.claude_turn_complete = Some(false);
        unavailable.last_assistant_message = None;
        return send(&route, &unavailable);
    }
    result
}

fn claude_ancestor() -> Option<(u32, u64)> {
    let mut system = System::new();
    let mut pid = sysinfo::Pid::from_u32(std::process::id());
    for _ in 0..32 {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        if process.name() == "claude"
            || process.cmd().first().is_some_and(|program| {
                std::path::Path::new(program)
                    .file_name()
                    .is_some_and(|name| name == "claude")
            })
        {
            return Some((pid.as_u32(), process.start_time()));
        }
        pid = process.parent()?;
    }
    None
}

pub fn run() -> Result<()> {
    let mut observation: Observation =
        serde_json::from_reader(std::io::stdin()).context("read Claude turn observation")?;
    let completed = follow(&observation);
    observation.hook.hook_event_name = "Stop".into();
    observation.hook.prompt = None;
    observation.hook.prompt_sha256 = None;
    observation.hook.claude_turn_complete = Some(completed.is_ok());
    if let Ok(turn) = completed {
        observation.hook.last_assistant_message = turn.last_text;
        observation.hook.claude_background_tasks = turn.background_tasks;
    } else {
        observation.hook.last_assistant_message = None;
    }
    send(&observation.route, &observation.hook)
}

fn follow(observation: &Observation) -> Result<Turn> {
    let (process_id, start_time) = observation
        .process
        .context("cannot identify Claude process")?;
    let pid = sysinfo::Pid::from_u32(process_id);
    let path = observation
        .hook
        .transcript_path
        .as_ref()
        .context("Claude transcript path missing")?;
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
            return Err(anyhow!("Claude process exited"));
        }
        if let Some(marker) = &observation.conversation_file {
            if std::fs::read_to_string(marker).context("Claude conversation ended")?
                != observation.hook.session_id
            {
                return Err(anyhow!("Claude conversation replaced"));
            }
        }
        Ok(())
    };
    // The first prompt's file can be created only after UserPromptSubmit hooks return.
    let creation_deadline =
        Instant::now() + Duration::from_secs(yinz_types::agent::CLAUDE_HOOK_TIMEOUT_SECONDS);
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
            Err(error) => return Err(error).context("open Claude transcript"),
        }
    };
    let metadata = file.metadata().context("inspect Claude transcript")?;
    if !metadata.is_file() {
        return Err(anyhow!("Claude transcript is not a regular file"));
    }
    let mut reader = BufReader::new(file);
    let mut turn = Turn::new(&observation.hook);
    let mut line = Vec::new();
    let mut offset = 0;
    let deadline =
        Instant::now() + Duration::from_millis(yinz_types::agent::control::MAX_TIMEOUT_MS);
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow!("Claude turn observation expired"));
        }
        alive(&mut system)?;
        let current = std::fs::metadata(path).context("Claude transcript disappeared")?;
        if current.ino() != metadata.ino()
            || current.dev() != metadata.dev()
            || current.len() < offset
        {
            return Err(anyhow!("Claude transcript replaced or truncated"));
        }
        loop {
            let bytes = reader
                .by_ref()
                .take((16 * 1024 * 1024 + 1 - line.len()) as u64)
                .read_until(b'\n', &mut line)
                .context("read Claude transcript record")?;
            offset += bytes as u64;
            if line.len() > 16 * 1024 * 1024 {
                return Err(anyhow!("Claude transcript record exceeds 16 MiB"));
            }
            if line.last() != Some(&b'\n') {
                break;
            }
            let record: Value =
                serde_json::from_slice(&line).context("parse Claude transcript record")?;
            line.clear();
            if turn.record(&record)? {
                return Ok(turn);
            }
        }
        // Regular files cannot wake a condition variable; poll only while this exact process lives.
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Turn {
    session: String,
    prompt: String,
    found: bool,
    ancestors: HashSet<String>,
    last_text: Option<String>,
    /// Bash commands and subagents this turn left running in the background, in launch order.
    background_tasks: Vec<String>,
}

impl Turn {
    fn new(hook: &HookEvent) -> Self {
        Self {
            session: hook.session_id.clone(),
            prompt: hook.prompt_id.clone().unwrap_or_default(),
            found: false,
            ancestors: HashSet::new(),
            last_text: None,
            background_tasks: Vec::new(),
        }
    }

    fn record(&mut self, record: &Value) -> Result<bool> {
        if record["isSidechain"] == true {
            return Ok(false);
        }
        if record["sessionId"]
            .as_str()
            .is_some_and(|session| session != self.session)
        {
            return Err(anyhow!("Claude conversation replaced"));
        }
        if record["type"] == "user" {
            if let Some(prompt) = record["promptId"].as_str() {
                if self.found && prompt != self.prompt {
                    return Err(anyhow!("Claude prompt replaced"));
                }
                if prompt == self.prompt {
                    self.found = true;
                }
            }
        }
        if !self.found {
            return Ok(false);
        }
        let linked = record["promptId"].as_str() == Some(&self.prompt)
            || record["parentUuid"]
                .as_str()
                .is_some_and(|parent| self.ancestors.contains(parent));
        if !linked {
            return Ok(false);
        }
        if let Some(id) = record["uuid"].as_str() {
            self.ancestors.insert(id.to_owned());
        }
        if record["type"] == "user" {
            if let Some(task) = background_task(&record["toolUseResult"]) {
                if !self.background_tasks.iter().any(|known| known == task)
                    && self.background_tasks.len() < yinz_types::agent::MAX_BACKGROUND_TASKS
                {
                    self.background_tasks.push(task.to_owned());
                }
            }
        }
        if record["type"] == "assistant" {
            if let Some(content) = record["message"]["content"].as_array() {
                let text: String = content
                    .iter()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect();
                if !text.is_empty() {
                    self.last_text = Some(text);
                }
            }
        }
        Ok(record["type"] == "system" && record["subtype"] == "turn_duration")
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

pub(crate) fn send(route: &Route, hook: &HookEvent) -> Result<()> {
    let json = hook
        .broadcast_json(serde_json::to_string(hook).context("serialize Claude turn confirmation")?);
    match route {
        Route::Remote { socket, pane_token } => {
            crate::pty_host::remote::broker::report_agent_hook_to(
                socket, pane_token, "claude", &json,
            )
        },
        Route::Local { session, pane } => {
            let mut child =
                Command::new(std::env::current_exe().context("resolve confirmation reporter")?)
                    .args([
                        "--session",
                        session,
                        "action",
                        "agent-report",
                        "--kind",
                        "claude",
                        "--pane-id",
                        pane,
                        "--hook-json",
                    ])
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
                return Err(anyhow!("Claude confirmation was not delivered"));
            }
            Ok(())
        },
    }
}

#[cfg(test)]
#[path = "unit/claude_turn_tests.rs"]
mod tests;
