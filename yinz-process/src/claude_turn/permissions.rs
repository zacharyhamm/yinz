//! A single local permission observer, independent of transcripts and turn completion.
use super::*;
use yinz_types::agent::{
    claude_notifications::Evidence,
    claude_permissions::{Sample, Status, EVENT},
};

pub(super) fn start(hook: &HookEvent, route: &Route) -> Result<()> {
    if matches!(route, Route::Remote { .. })
        || hook.archive_only
        || hook.claude_recovered_after.is_some()
        || !matches!(
            hook.hook_event_name.as_str(),
            "SessionStart" | "SessionEnd" | "UserPromptSubmit" | "PermissionRequest"
        )
        || hook.session_id.is_empty()
    {
        return Ok(());
    }
    let Some(source) = hook
        .claude_notifications
        .as_ref()
        .and_then(|e| e.source.clone())
    else {
        return Ok(());
    };
    let marker = yinz_runtime::consts::YINZ_TMP_DIR.join(format!(
        "claude-conversation-{}-{}",
        source.pid, source.started
    ));
    if hook.is_worker() || hook.hook_event_name == "PermissionRequest" {
        match std::fs::read_to_string(&marker) {
            Ok(conversation) if conversation != hook.session_id => return Ok(()),
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                update_conversation_marker(&marker, "claude", hook)?
            },
            Err(error) => return Err(error).context("read permission conversation"),
        }
    } else {
        update_conversation_marker(&marker, "claude", hook)?;
    }
    if hook.hook_event_name == "SessionEnd" {
        return Ok(());
    }
    // Never carry a worker ID, parent prompt or native tool fields into session-wide evidence.
    let observation = Observation {
        kind: "claude".into(),
        hook: HookEvent {
            hook_event_name: EVENT.into(),
            session_id: hook.session_id.clone(),
            claude_notifications: Some(Evidence {
                source: Some(source.clone()),
                ..Default::default()
            }),
            ..Default::default()
        },
        route: route.clone(),
        process: Some((source.pid, source.started)),
        conversation_file: Some(marker),
        claude_status: ClaudeStatusObservation::new(source.pid),
        recovery: false,
        permissions: true,
    };
    spawn_observer(&observation)?;
    Ok(())
}

pub(super) fn lock(observation: &Observation, directory: &Path) -> Result<Option<std::fs::File>> {
    use sha2::{Digest, Sha256};
    let identity = serde_json::to_vec(&(
        observation.process,
        &observation.hook.session_id,
        &observation.route,
    ))
    .context("encode permission observer identity")?;
    let key: String = Sha256::digest(identity)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("claude-permissions-{key}.lock")))
        .context("open permission observer lock")?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error).context("lock permission observer"),
    }
}

/// Tick state is separate from I/O so loss, retries and automatic evaluation are testable.
#[derive(Default)]
pub(super) struct Watch {
    last: Option<Sample>,
    unknown_since: Option<Duration>,
    sent_at: Option<Duration>,
}

impl Watch {
    pub(super) fn next(&mut self, sample: Option<Sample>, now: Duration) -> Option<Sample> {
        let sample = match sample {
            Some(sample) => {
                self.unknown_since = None;
                sample
            },
            None => {
                let since = *self.unknown_since.get_or_insert(now);
                if now.saturating_sub(since) < Duration::from_secs(2) {
                    return None;
                }
                let mut lost = self.last.clone()?;
                lost.status = Status::Unavailable;
                lost.observed_ms = lost.observed_ms.saturating_add(2_000);
                lost
            },
        };
        (self.last.as_ref().is_none_or(|last| {
            last.source != sample.source
                || last.updated_ms != sample.updated_ms
                || last.status != sample.status
        }) || (sample.status == Status::Active
            && self
                .sent_at
                .is_none_or(|sent| now.saturating_sub(sent) >= Duration::from_millis(500))))
        .then_some(sample)
    }

    pub(super) fn accepted(&mut self, sample: Sample, now: Duration) {
        self.last = Some(sample);
        self.sent_at = Some(now);
    }
}

pub(super) fn run(observation: &Observation) -> Result<()> {
    let directory = &*yinz_runtime::consts::YINZ_TMP_DIR;
    yinz_runtime::shared::ensure_secure_dir(directory)
        .context("secure permission observer directory")?;
    let Some(_lock) = lock(observation, directory)? else {
        return Ok(());
    };
    let process = observation.process.context("permission provider missing")?;
    let mut system = System::new();
    let start = Instant::now();
    let mut watch = Watch::default();
    while alive(&mut system, observation).is_ok() {
        let sample = observation
            .claude_status
            .as_ref()
            .and_then(|reader| reader.permission(process, &observation.hook.session_id));
        if let Some(sample) = watch.next(sample, start.elapsed()) {
            let mut hook = observation.hook.clone();
            hook.claude_permission = Some(sample.clone());
            // A bounded transport failure does not abandon a published wait. Retry while
            // the provider lives; the screen lease also covers observer death or a dead route.
            if send(&observation.route, "claude", &hook).is_ok() {
                watch.accepted(sample, start.elapsed());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Some(mut sample) = watch.last {
        sample.status = Status::Unavailable;
        let mut hook = observation.hook.clone();
        hook.claude_permission = Some(sample);
        send(&observation.route, "claude", &hook)
            .context("invalidate stopped permission observer")
            .non_fatal();
    }
    Ok(())
}
