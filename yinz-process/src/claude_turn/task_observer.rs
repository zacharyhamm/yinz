//! One replayable task observer per provider process, conversation and reporting route.
use super::*;
use std::collections::BTreeMap;

pub(super) fn run(observation: &Observation) -> Result<()> {
    let directory = &*yinz_runtime::consts::YINZ_TMP_DIR;
    yinz_runtime::shared::ensure_secure_dir(directory).context("secure task observer directory")?;
    let Some(_lock) = lock(observation, directory)? else {
        return Ok(());
    };
    follow(observation, |hook| send(&observation.route, "claude", hook))
}

pub(super) fn lock(observation: &Observation, directory: &Path) -> Result<Option<std::fs::File>> {
    use sha2::{Digest, Sha256};
    let identity = serde_json::to_vec(&(
        observation
            .process
            .context("Claude task observer process missing")?,
        &observation.hook.session_id,
        &observation.route,
    ))
    .context("encode task observer identity")?;
    let key: String = Sha256::digest(identity)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("claude-tasks-{key}.lock")))
        .context("open task observer lock")?;
    // Keep the inode: unlinking an unlocked file races a competing observer.
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error).context("lock task observer"),
    }
}

pub(super) fn follow(
    observation: &Observation,
    report: impl FnMut(&HookEvent) -> Result<()>,
) -> Result<()> {
    follow_reporting(observation, report, spawn_observer)
}

pub(super) fn follow_reporting(
    observation: &Observation,
    mut report: impl FnMut(&HookEvent) -> Result<()>,
    mut observe: impl FnMut(&Observation) -> Result<Child>,
) -> Result<()> {
    let (process_id, started) = observation.process.context("Claude process missing")?;
    let pid = sysinfo::Pid::from_u32(process_id);
    let root = Path::new(
        observation
            .hook
            .transcript_path
            .as_deref()
            .context("Claude transcript missing")?,
    );
    let sidechains = root.with_extension("").join("subagents");
    let mut system = System::new();
    let mut tasks = tasks::Tasks::new(&observation.hook.session_id);
    let mut prompts = prompts::Prompts::new(&observation.hook);
    let mut readers = BTreeMap::<PathBuf, Transcript>::new();
    let mut children = CompletionChildren::default();
    loop {
        children.reap()?;
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        if !system.process(pid).is_some_and(|process| {
            process.start_time() == started && process.status() != sysinfo::ProcessStatus::Zombie
        }) {
            return Ok(());
        }
        if let Some(marker) = &observation.conversation_file
            && std::fs::read_to_string(marker).context("read Claude conversation")?
                != observation.hook.session_id
        {
            return Ok(());
        }
        let mut paths = vec![root.to_path_buf()];
        match std::fs::read_dir(&sidechains) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.context("read Claude sidechain entry")?.path();
                    if path.extension().is_some_and(|ext| ext == "jsonl") {
                        paths.push(path);
                        if paths.len() > 4096 {
                            return Err(anyhow!("too many Claude sidechains"));
                        }
                    }
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error).context("list Claude sidechains"),
        }
        paths.sort();
        for path in paths {
            if !readers.contains_key(&path) {
                if readers.len() >= 4096 {
                    return Err(anyhow!("too many observed Claude transcripts"));
                }
                match Transcript::open(&path) {
                    Ok(reader) => {
                        readers.insert(path.clone(), reader);
                    },
                    Err(error)
                        if error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        continue
                    },
                    Err(error) => return Err(error),
                }
            }
            let reader = readers
                .get_mut(&path)
                .context("Claude transcript reader missing")?;
            reader.poll(&path, |record| {
                if path == root
                    && let Some(hook) = prompts.record(record)?
                {
                    // Deliver acceptance before completion can race it. Keep the original
                    // process, marker and route; a reporter child has no Claude ancestor.
                    report(&hook)?;
                    let recovered = recovered_observation(observation, hook, record);
                    match observe(&recovered) {
                        Ok(child) => children.0.push(child),
                        Err(error) => {
                            let mut unavailable = recovered.hook;
                            unavailable.hook_event_name = "Stop".into();
                            unavailable.claude_turn_complete = Some(false);
                            report(&unavailable)?;
                            return Err(error).context("observe recovered Claude prompt");
                        },
                    }
                }
                for hook in tasks.record(record)? {
                    report(&hook)?;
                }
                Ok(())
            })?;
        }
        // EOF and parent Idle are not completion signals. A later trigger replays
        // all files if this observer exits on an I/O or bounded reporting failure.
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn recovered_observation(
    observation: &Observation,
    hook: HookEvent,
    record: &Value,
) -> Observation {
    // Idle may have been published before replay catches up. Only the prompt's own
    // timestamp is a freshness boundary; malformed/missing timestamps leave transcript
    // completion in charge. Keep the original process-specific registry path.
    let after_ms = record["timestamp"]
        .as_str()
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .and_then(|timestamp| u64::try_from(timestamp.timestamp_millis()).ok());
    Observation {
        hook,
        task_observer: false,
        claude_status: after_ms.and_then(|after_ms| {
            observation
                .claude_status
                .as_ref()
                .map(|status| ClaudeStatusObservation {
                    path: status.path.clone(),
                    after_ms,
                })
        }),
        ..observation.clone()
    }
}

#[derive(Default)]
struct CompletionChildren(Vec<Child>);

impl CompletionChildren {
    fn reap(&mut self) -> Result<()> {
        let mut index = 0;
        while index < self.0.len() {
            if self.0[index]
                .try_wait()
                .context("poll Claude completion observer")?
                .is_some()
            {
                self.0.swap_remove(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}

impl Drop for CompletionChildren {
    fn drop(&mut self) {
        for child in &mut self.0 {
            stop_observer(child);
        }
    }
}

struct Transcript {
    reader: BufReader<std::fs::File>,
    metadata: std::fs::Metadata,
    offset: u64,
    line: Vec<u8>,
}

impl Transcript {
    fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .context("open Claude task transcript")?;
        let metadata = file.metadata().context("inspect Claude task transcript")?;
        if !metadata.is_file() {
            return Err(anyhow!("Claude task transcript is not a regular file"));
        }
        Ok(Self {
            reader: BufReader::new(file),
            metadata,
            offset: 0,
            line: vec![],
        })
    }

    fn poll(&mut self, path: &Path, mut record: impl FnMut(&Value) -> Result<()>) -> Result<()> {
        let current = std::fs::metadata(path).context("inspect Claude task transcript")?;
        if current.ino() != self.metadata.ino()
            || current.dev() != self.metadata.dev()
            || current.len() < self.offset
        {
            return Err(anyhow!("Claude task transcript replaced or truncated"));
        }
        // Bound each sweep so a continuously growing file cannot starve sidechains
        // or process/conversation checks. Partial lines survive polling boundaries.
        for _ in 0..1024 {
            let bytes = self
                .reader
                .by_ref()
                .take((16 * 1024 * 1024 + 1 - self.line.len()) as u64)
                .read_until(b'\n', &mut self.line)
                .context("read Claude task record")?;
            self.offset += bytes as u64;
            if self.line.len() > 16 * 1024 * 1024 {
                return Err(anyhow!("Claude task record exceeds 16 MiB"));
            }
            if self.line.last() != Some(&b'\n') {
                break;
            }
            let value = serde_json::from_slice(&self.line).context("parse Claude task record")?;
            self.line.clear();
            record(&value)?;
        }
        Ok(())
    }
}
