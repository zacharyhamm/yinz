//! One replayable prompt recovery observer per provider process, conversation and route.
use super::*;

pub(super) fn run(observation: &Observation) -> Result<()> {
    let directory = &*yinz_runtime::consts::YINZ_TMP_DIR;
    yinz_runtime::shared::ensure_secure_dir(directory)
        .context("secure prompt recovery directory")?;
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
            .context("Claude recovery observer process missing")?,
        &observation.hook.session_id,
        &observation.route,
    ))
    .context("encode recovery observer identity")?;
    let key: String = Sha256::digest(identity)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("claude-prompts-{key}.lock")))
        .context("open recovery observer lock")?;
    // Keep the inode: unlinking an unlocked file races a competing observer.
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error).context("lock recovery observer"),
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
    let root = Path::new(
        observation
            .hook
            .transcript_path
            .as_deref()
            .context("Claude transcript missing")?,
    );
    let mut system = System::new();
    let mut prompts = prompts::Prompts::new(&observation.hook);
    let mut reader = None;
    let mut children = CompletionChildren::default();
    loop {
        children.reap()?;
        if alive(&mut system, observation).is_err() {
            return Ok(());
        }
        if reader.is_none() {
            match Transcript::open(root) {
                Ok(opened) => reader = Some(opened),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {},
                Err(error) => return Err(error),
            }
        }
        if let Some(reader) = &mut reader {
            reader.poll(root, |record| {
                if let Some(hook) = prompts.record(record)? {
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
                Ok(())
            })?;
        }
        // EOF and parent Idle are not completion signals. A later trigger replays
        // the transcript if this observer exits on an I/O or bounded reporting failure.
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
        recovery: false,
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
                self.0
                    .swap_remove(index)
                    .wait()
                    .context("reap finished Claude completion observer")?;
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
            .context("open Claude transcript")?;
        let metadata = file.metadata().context("inspect Claude transcript")?;
        if !metadata.is_file() {
            return Err(anyhow!("Claude transcript is not a regular file"));
        }
        Ok(Self {
            reader: BufReader::new(file),
            metadata,
            offset: 0,
            line: vec![],
        })
    }

    fn poll(&mut self, path: &Path, mut record: impl FnMut(&Value) -> Result<()>) -> Result<()> {
        let current = std::fs::metadata(path).context("inspect Claude transcript")?;
        if current.ino() != self.metadata.ino()
            || current.dev() != self.metadata.dev()
            || current.len() < self.offset
        {
            return Err(anyhow!("Claude transcript replaced or truncated"));
        }
        // Bound each sweep so a continuously growing file cannot starve process and
        // conversation checks. Partial lines survive polling boundaries.
        for _ in 0..1024 {
            let bytes = self
                .reader
                .by_ref()
                .take((16 * 1024 * 1024 + 1 - self.line.len()) as u64)
                .read_until(b'\n', &mut self.line)
                .context("read Claude transcript record")?;
            self.offset += bytes as u64;
            if self.line.len() > 16 * 1024 * 1024 {
                return Err(anyhow!("Claude transcript record exceeds 16 MiB"));
            }
            if self.line.last() != Some(&b'\n') {
                break;
            }
            let value =
                serde_json::from_slice(&self.line).context("parse Claude transcript record")?;
            self.line.clear();
            record(&value)?;
        }
        Ok(())
    }
}
