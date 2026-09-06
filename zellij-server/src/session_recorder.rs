use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use zellij_types::internal_ids::TerminalId;
use zellij_types::units::{ElapsedMicros, UnixMicros};

use zellij_runtime::errors::prelude::*;
use zellij_storage::data_access::{DataAccess, RecordingSession};
use zellij_storage::session_recording::{PaneMetadata, RecordedEvent};

#[derive(Debug)]
pub enum RecorderInstruction {
    Events(Vec<RecordedEvent>),
    PaneMetadata {
        elapsed_us: ElapsedMicros,
        panes: Vec<PaneMetadata>,
    },
    PaneClosed {
        terminal_id: TerminalId,
        elapsed_us: ElapsedMicros,
    },
    Stop,
}

#[derive(Clone)]
pub struct SessionRecorderHandle {
    pub(crate) sender: crossbeam::channel::Sender<RecorderInstruction>,
    pub(crate) started_at: Instant,
    pub(crate) capture_input: bool,
    pub(crate) enabled: Arc<AtomicBool>,
    pub(crate) dropped: Arc<AtomicUsize>,
}

impl SessionRecorderHandle {
    pub fn output(&self, terminal_id: TerminalId, data: Vec<u8>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = ElapsedMicros::new(self.started_at.elapsed().as_micros() as i64);
        let event = RecordedEvent::output(terminal_id, elapsed_us, data);
        if let Err(e) = self
            .sender
            .try_send(RecorderInstruction::Events(vec![event]))
        {
            match e {
                crossbeam::channel::TrySendError::Full(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    log::warn!("session recorder channel full, dropping output event");
                },
                crossbeam::channel::TrySendError::Disconnected(_) => {
                    self.enabled.store(false, Ordering::Relaxed);
                    log::error!("session recorder channel disconnected, disabling");
                },
            }
        }
    }

    pub fn input(&self, terminal_id: TerminalId, data: Vec<u8>) {
        if !self.capture_input {
            return;
        }
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = ElapsedMicros::new(self.started_at.elapsed().as_micros() as i64);
        let event = RecordedEvent::input(terminal_id, elapsed_us, data);
        if let Err(e) = self
            .sender
            .try_send(RecorderInstruction::Events(vec![event]))
        {
            match e {
                crossbeam::channel::TrySendError::Full(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    log::warn!("session recorder channel full, dropping input event");
                },
                crossbeam::channel::TrySendError::Disconnected(_) => {
                    self.enabled.store(false, Ordering::Relaxed);
                    log::error!("session recorder channel disconnected, disabling");
                },
            }
        }
    }

    pub fn resize(&self, terminal_id: TerminalId, cols: u16, rows: u16) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = ElapsedMicros::new(self.started_at.elapsed().as_micros() as i64);
        let event = RecordedEvent::resize(terminal_id, elapsed_us, cols, rows);
        if let Err(e) = self
            .sender
            .try_send(RecorderInstruction::Events(vec![event]))
        {
            match e {
                crossbeam::channel::TrySendError::Full(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    log::warn!("session recorder channel full, dropping resize event");
                },
                crossbeam::channel::TrySendError::Disconnected(_) => {
                    self.enabled.store(false, Ordering::Relaxed);
                    log::error!("session recorder channel disconnected, disabling");
                },
            }
        }
    }

    pub fn pane_metadata(&self, panes: Vec<PaneMetadata>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = ElapsedMicros::new(self.started_at.elapsed().as_micros() as i64);
        if let Err(e) = self
            .sender
            .try_send(RecorderInstruction::PaneMetadata { elapsed_us, panes })
        {
            match e {
                crossbeam::channel::TrySendError::Full(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    log::warn!("session recorder channel full, dropping pane_metadata");
                },
                crossbeam::channel::TrySendError::Disconnected(_) => {
                    self.enabled.store(false, Ordering::Relaxed);
                    log::error!("session recorder channel disconnected, disabling");
                },
            }
        }
    }

    pub fn pane_closed(&self, terminal_id: TerminalId) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = ElapsedMicros::new(self.started_at.elapsed().as_micros() as i64);
        if let Err(e) = self.sender.try_send(RecorderInstruction::PaneClosed {
            terminal_id,
            elapsed_us,
        }) {
            match e {
                crossbeam::channel::TrySendError::Full(_) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    log::warn!("session recorder channel full, dropping pane_closed");
                },
                crossbeam::channel::TrySendError::Disconnected(_) => {
                    self.enabled.store(false, Ordering::Relaxed);
                    log::error!("session recorder channel disconnected, disabling");
                },
            }
        }
    }

    pub fn stop(&self) {
        // Stop must not be lost when the channel is full. Normal events use
        // try_send to avoid blocking the caller, but Stop is a control signal
        // and must be delivered to avoid a false `incomplete` state on shutdown.
        const RETRIES: usize = 5;
        for _ in 0..RETRIES {
            match self
                .sender
                .send_timeout(RecorderInstruction::Stop, Duration::from_millis(500))
            {
                Ok(_) => return,
                Err(crossbeam::channel::SendTimeoutError::Timeout(_)) => continue,
                Err(crossbeam::channel::SendTimeoutError::Disconnected(_)) => {
                    log::error!("session recorder channel disconnected on stop");
                    return;
                },
            }
        }
        log::error!("session recorder channel full, failed to send Stop after retries");
        // Fallback to ensure shutdown does not hang if the recorder thread is
        // deadlocked. This will cause `incomplete` but avoids an indefinite join.
        self.enabled.store(false, Ordering::Relaxed);
    }
}

pub fn spawn_session_recorder(
    data_access: Arc<DataAccess>,
    data_dir: PathBuf,
    session_name: String,
    capture_input: bool,
) -> Option<(SessionRecorderHandle, std::thread::JoinHandle<()>)> {
    // validate data_dir can be created and is a directory
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        log::error!(
            "failed to create session recording data_dir {:?}: {}",
            data_dir,
            e
        );
        return None;
    }
    if let Ok(md) = std::fs::metadata(&data_dir) {
        if !md.is_dir() {
            log::error!(
                "session recording data_dir is not a directory: {:?}",
                data_dir
            );
            return None;
        }
    }
    let (sender, receiver) = crossbeam::channel::bounded(4096);
    let enabled = Arc::new(AtomicBool::new(true));
    let dropped = Arc::new(AtomicUsize::new(0));
    let started_at = Instant::now();
    let started_at_us = UnixMicros::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or_else(|e| {
                log::warn!(
                    "system clock is before UNIX_EPOCH ({}), using 0 for session start",
                    e
                );
                0
            }),
    );
    let handle = SessionRecorderHandle {
        sender: sender.clone(),
        started_at,
        capture_input,
        enabled: enabled.clone(),
        dropped: dropped.clone(),
    };
    let enabled_clone = enabled.clone();
    let thread = std::thread::spawn(move || {
        recorder_thread(
            data_access,
            session_name,
            capture_input,
            started_at,
            started_at_us,
            receiver,
            enabled_clone,
            dropped,
        );
    });
    Some((handle, thread))
}

/// The recorder thread's view of one recording: the store, the pending batch, and what the
/// store has been told about loss so far.
struct Recording {
    store: Box<dyn RecordingSession>,
    batch: Vec<RecordedEvent>,
    batch_start: Option<Instant>,
    /// The first storage failure, kept so a later healthy write cannot hide it.
    first_error: Option<String>,
    /// The loss reason last written to the store. The marker is rewritten only when the reason
    /// changes, and a write that failed is retried at the next flush because this stays stale.
    persisted_loss: Option<String>,
    dropped: Arc<AtomicUsize>,
    enabled: Arc<AtomicBool>,
}

impl Recording {
    fn remember(&mut self, result: anyhow::Result<()>) {
        if let Err(error) = result {
            self.first_error.get_or_insert_with(|| format!("{error:#}"));
            Err::<(), _>(error).non_fatal();
        }
    }

    fn loss_reason(&self) -> Option<String> {
        let mut reasons: Vec<String> = self.first_error.iter().cloned().collect();
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            reasons.push(format!("dropped {dropped} recording events"));
        } else if !self.enabled.load(Ordering::Relaxed) {
            reasons.push("recorder channel full or disconnected".to_owned());
        }
        (!reasons.is_empty()).then(|| reasons.join("; "))
    }

    /// Writes the loss marker when the reason has changed since it was last saved. An unchanged
    /// reason costs nothing, and a write that failed is tried again on the next call.
    fn sync_loss_marker(&mut self) -> anyhow::Result<()> {
        let Some(reason) = self.loss_reason() else {
            return Ok(());
        };
        if self.persisted_loss.as_deref() == Some(reason.as_str()) {
            return Ok(());
        }
        self.store
            .mark_incomplete(&reason)
            .context("failed to mark recording incomplete")?;
        self.persisted_loss = Some(reason);
        Ok(())
    }

    /// Saves the marker, logging a failure that the next flush or pane operation will retry.
    fn save_loss_marker(&mut self) {
        self.sync_loss_marker()
            .context("recording loss marker not saved; retrying at the next flush")
            .non_fatal();
    }

    /// Records a pane operation's outcome and saves any loss it introduced right away, so the
    /// marker is on disk before a closed pane can be listed as finished.
    fn remember_and_save(&mut self, result: anyhow::Result<()>) {
        self.remember(result);
        self.save_loss_marker();
    }

    /// Writes the pending batch, then saves any loss it caused or that the channel reported
    /// meanwhile. Empty flushes skip the marker: an idle recorder with broken storage should not
    /// retry every 100ms, and every new loss reason reaches a non-empty flush or a pane operation.
    fn flush(&mut self) {
        self.batch_start = None;
        if self.batch.is_empty() {
            return;
        }
        let result = self
            .store
            .record_events(&self.batch)
            .context("failed to record events");
        if result.is_err() {
            self.dropped.fetch_add(self.batch.len(), Ordering::Relaxed);
        }
        // The backend does not promise retry safety. Discard failed batches, but
        // remember the failure so the recording cannot be reported complete.
        self.batch.clear();
        self.remember(result);
        self.save_loss_marker();
    }

    /// Flushes what is left and finishes the session, unless its loss marker cannot be saved:
    /// a session without an end timestamp lists as running, which is more honest than complete.
    fn finish(&mut self, ended_at_us: UnixMicros, ended_elapsed_us: ElapsedMicros) {
        self.flush();
        if let Err(error) = self.sync_loss_marker() {
            Err::<(), _>(error)
                .context("leaving recording unfinalized because its loss marker could not be saved")
                .non_fatal();
            return;
        }
        let finish_session = self
            .store
            .finish_session(ended_at_us, ended_elapsed_us)
            .context("failed to finish session");
        if finish_session.is_err() {
            self.remember_and_save(finish_session);
        }
    }
}

fn recorder_thread(
    data_access: Arc<DataAccess>,
    session_name: String,
    capture_input: bool,
    started_at: Instant,
    started_at_us: UnixMicros,
    receiver: crossbeam::channel::Receiver<RecorderInstruction>,
    enabled: Arc<AtomicBool>,
    dropped: Arc<AtomicUsize>,
) {
    let store =
        match data_access
            .recordings()
            .start_recording(&session_name, capture_input, started_at_us)
        {
            Ok(s) => s,
            Err(e) => {
                Err::<(), _>(e)
                    .context("failed to start session recorder")
                    .non_fatal();
                return;
            },
        };
    let mut recording = Recording {
        store,
        batch: Vec::new(),
        batch_start: None,
        first_error: None,
        persisted_loss: None,
        dropped,
        enabled: enabled.clone(),
    };
    let flush_interval = Duration::from_millis(100);
    loop {
        if recording.batch.len() >= 256
            || recording
                .batch_start
                .is_some_and(|start| start.elapsed() >= flush_interval)
        {
            recording.flush();
        }
        let timeout = recording
            .batch_start
            .map(|start| flush_interval.saturating_sub(start.elapsed()))
            .unwrap_or(flush_interval);
        // Once disabled, drain queued instructions without waiting for more.
        // Using the same dispatch keeps flush ordering identical during shutdown.
        let instruction = if enabled.load(Ordering::Relaxed) {
            receiver.recv_timeout(timeout)
        } else {
            receiver
                .try_recv()
                .map_err(|_| crossbeam::channel::RecvTimeoutError::Disconnected)
        };
        match instruction {
            Ok(RecorderInstruction::Events(mut events)) => {
                if recording.batch.is_empty() {
                    recording.batch_start = Some(Instant::now());
                }
                recording.batch.append(&mut events);
            },
            Ok(RecorderInstruction::PaneMetadata { elapsed_us, panes }) => {
                recording.flush();
                let result = recording
                    .store
                    .update_panes(&panes, elapsed_us)
                    .context("failed to update panes");
                recording.remember_and_save(result);
            },
            Ok(RecorderInstruction::PaneClosed {
                terminal_id,
                elapsed_us,
            }) => {
                recording.flush();
                let result = recording
                    .store
                    .finish_pane(terminal_id, elapsed_us)
                    .with_context(|| format!("failed to finish pane {terminal_id}"));
                recording.remember_and_save(result);
            },
            Ok(RecorderInstruction::Stop)
            | Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => recording.flush(),
        }
    }
    let ended_at_us = UnixMicros::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or_else(|e| {
                log::warn!(
                    "system clock is before UNIX_EPOCH on session end ({}), using 0",
                    e
                );
                0
            }),
    );
    let ended_elapsed_us = ElapsedMicros::new(started_at.elapsed().as_micros() as i64);
    recording.finish(ended_at_us, ended_elapsed_us);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zellij_storage::data_access::{PluginDataBackend, RecordingBackend, RecordingSession};
    use zellij_test_support::{default_timeout, Watched};
    use zellij_types::internal_ids::RecordingId;
    use zellij_types::plugin_data::{
        ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
        Mutation,
    };

    #[derive(Default)]
    struct RecordingLog {
        calls: Vec<&'static str>,
        events: Vec<RecordedEvent>,
        errors: Vec<String>,
        fail_once: Option<&'static str>,
        fail_always: Option<&'static str>,
    }

    impl RecordingLog {
        fn marker_writes(&self) -> usize {
            self.calls
                .iter()
                .filter(|call| **call == "incomplete")
                .count()
        }
    }

    struct MemoryRecordingBackend(Watched<RecordingLog>);
    struct MemoryRecordingSession(Watched<RecordingLog>);

    impl MemoryRecordingSession {
        fn call(&self, operation: &'static str) -> anyhow::Result<()> {
            let mut log = self.0.lock();
            log.calls.push(operation);
            if log.fail_once == Some(operation) {
                log.fail_once = None;
                anyhow::bail!("injected {operation} failure");
            }
            if log.fail_always == Some(operation) {
                anyhow::bail!("injected {operation} failure");
            }
            Ok(())
        }
    }

    impl PluginDataBackend for MemoryRecordingBackend {
        fn define_entity(
            &self,
            _: &str,
            _: &str,
            _: &EntityDef,
            _: bool,
        ) -> Result<DataSchemaInfo, DataError> {
            Ok(DataSchemaInfo::default())
        }

        fn catalog(&self, _: Option<&str>) -> Result<Vec<DataEntityInfo>, DataError> {
            Ok(Vec::new())
        }

        fn fetch(&self, _: &str, _: &FetchRequest) -> Result<DataRows, DataError> {
            Ok(DataRows::default())
        }

        fn count(&self, _: &str, _: &FetchRequest) -> Result<u64, DataError> {
            Ok(0)
        }

        fn apply(&self, _: &str, _: &[Mutation]) -> Result<ApplyReport, DataError> {
            Ok(ApplyReport::default())
        }
    }

    impl RecordingBackend for MemoryRecordingBackend {
        fn start_recording(
            &self,
            _: &str,
            _: bool,
            _: UnixMicros,
        ) -> anyhow::Result<Box<dyn RecordingSession>> {
            self.0.lock().calls.push("start");
            Ok(Box::new(MemoryRecordingSession(self.0.clone())))
        }

        fn list(&self) -> anyhow::Result<Vec<zellij_types::data::RecordingSummary>> {
            Ok(Vec::new())
        }

        fn search(&self, _: &str, _: usize) -> anyhow::Result<Vec<zellij_types::data::SearchHit>> {
            Ok(Vec::new())
        }

        fn export(&self, _: RecordingId, _: &mut dyn std::io::Write) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl RecordingSession for MemoryRecordingSession {
        fn record_events(&mut self, events: &[RecordedEvent]) -> anyhow::Result<()> {
            self.call("events")?;
            self.0.lock().events.extend_from_slice(events);
            Ok(())
        }

        fn update_panes(&mut self, _: &[PaneMetadata], _: ElapsedMicros) -> anyhow::Result<()> {
            self.call("panes")
        }

        fn finish_pane(&mut self, _: TerminalId, _: ElapsedMicros) -> anyhow::Result<()> {
            self.call("pane")
        }

        fn finish_session(&mut self, _: UnixMicros, _: ElapsedMicros) -> anyhow::Result<()> {
            self.call("session")
        }

        fn mark_incomplete(&mut self, message: &str) -> anyhow::Result<()> {
            self.call("incomplete")?;
            self.0.lock().errors.push(message.to_owned());
            Ok(())
        }
    }

    /// Runs the recorder thread body against the in-memory backend until `receiver` yields
    /// `Stop` or disconnects.
    fn run_recorder(
        log: &Watched<RecordingLog>,
        receiver: crossbeam::channel::Receiver<RecorderInstruction>,
        enabled: bool,
        dropped: usize,
    ) {
        let backend = Arc::new(MemoryRecordingBackend(log.clone()));
        recorder_thread(
            Arc::new(DataAccess::from_backends(backend.clone(), backend)),
            "test".to_owned(),
            false,
            Instant::now(),
            UnixMicros::new(0),
            receiver,
            Arc::new(AtomicBool::new(enabled)),
            Arc::new(AtomicUsize::new(dropped)),
        );
    }

    #[test]
    fn recorder_thread_uses_an_injected_backend() -> anyhow::Result<()> {
        let log = Watched::new(RecordingLog::default());
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender.send(RecorderInstruction::Events(vec![RecordedEvent::output(
            TerminalId::new(1),
            ElapsedMicros::new(1),
            b"x".to_vec(),
        )]))?;
        sender.send(RecorderInstruction::PaneMetadata {
            elapsed_us: ElapsedMicros::new(2),
            panes: Vec::new(),
        })?;
        sender.send(RecorderInstruction::PaneClosed {
            terminal_id: crate::TerminalId::new(1),
            elapsed_us: ElapsedMicros::new(3),
        })?;
        sender.send(RecorderInstruction::Stop)?;

        run_recorder(&log, receiver, true, 0);

        assert_eq!(
            log.lock().calls,
            vec!["start", "events", "panes", "pane", "session"]
        );
        Ok(())
    }

    #[test]
    fn storage_failures_survive_later_success_and_channel_loss() {
        for failure in [
            Some("events"),
            Some("panes"),
            Some("pane"),
            Some("session"),
            None,
        ] {
            for (enabled, dropped) in [(true, 0), (true, 2), (false, 0), (false, 2)] {
                let log = Watched::new(RecordingLog {
                    fail_once: failure,
                    ..Default::default()
                });
                let (sender, receiver) = crossbeam::channel::unbounded();
                let first = RecordedEvent::output(
                    TerminalId::new(1),
                    ElapsedMicros::new(1),
                    b"first".to_vec(),
                );
                let last = RecordedEvent::output(
                    TerminalId::new(1),
                    ElapsedMicros::new(4),
                    b"last".to_vec(),
                );
                sender
                    .send(RecorderInstruction::Events(vec![first.clone()]))
                    .unwrap();
                sender
                    .send(RecorderInstruction::PaneMetadata {
                        elapsed_us: ElapsedMicros::new(2),
                        panes: vec![],
                    })
                    .unwrap();
                sender
                    .send(RecorderInstruction::PaneClosed {
                        elapsed_us: ElapsedMicros::new(3),
                        terminal_id: crate::TerminalId::new(1),
                    })
                    .unwrap();
                sender
                    .send(RecorderInstruction::Events(vec![last.clone()]))
                    .unwrap();
                // Exercise disconnect/drain finalization, with no Stop instruction.
                drop(sender);
                run_recorder(&log, receiver, enabled, dropped);
                let log = log.lock();
                let case = format!("failure {failure:?}, enabled {enabled}, dropped {dropped}");
                let expected_events = if failure == Some("events") {
                    vec![last]
                } else {
                    vec![first, last]
                };
                assert_eq!(
                    log.events, expected_events,
                    "events must be attempted once, including when draining ({case})"
                );
                // The marker is written when a loss reason first appears and whenever it
                // changes: channel loss at the first flush, a storage failure right after the
                // operation that failed. An unchanged reason is never rewritten.
                let channel_loss = dropped > 0 || !enabled;
                let mut expected_calls = vec!["start"];
                for (index, operation) in ["events", "panes", "pane", "events", "session"]
                    .into_iter()
                    .enumerate()
                {
                    let first_of_its_kind = !expected_calls.contains(&operation);
                    expected_calls.push(operation);
                    let failed_here = first_of_its_kind && failure == Some(operation);
                    if failed_here || (index == 0 && channel_loss) {
                        expected_calls.push("incomplete");
                    }
                }
                assert_eq!(log.calls, expected_calls, "{case}");
                assert_eq!(log.errors.len(), log.marker_writes(), "{case}");
                let Some(last_error) = log.errors.last() else {
                    assert!(failure.is_none() && !channel_loss, "{case}");
                    continue;
                };
                if let Some(operation) = failure {
                    assert!(
                        last_error.contains(&format!("injected {operation} failure")),
                        "{case}: {last_error}"
                    );
                }
                let expected_dropped = dropped + usize::from(failure == Some("events"));
                if expected_dropped > 0 {
                    assert!(
                        last_error
                            .contains(&format!("dropped {expected_dropped} recording events")),
                        "{case}: {last_error}"
                    );
                } else if !enabled {
                    assert!(
                        last_error.contains("recorder channel full or disconnected"),
                        "{case}: {last_error}"
                    );
                }
            }
        }
    }

    #[test]
    fn size_timeout_and_stop_flush_failures_mark_incomplete() {
        for trigger in ["size", "timeout", "stop"] {
            let log = Watched::new(RecordingLog {
                fail_once: Some("events"),
                ..Default::default()
            });
            let (sender, receiver) = crossbeam::channel::unbounded();
            let count = if trigger == "size" { 256 } else { 1 };
            sender
                .send(RecorderInstruction::Events(vec![
                    RecordedEvent::output(
                        TerminalId::new(1),
                        ElapsedMicros::new(1),
                        vec![1]
                    );
                    count
                ]))
                .unwrap();
            let thread = {
                let log = log.clone();
                std::thread::spawn(move || run_recorder(&log, receiver, true, 0))
            };
            if trigger != "stop" {
                log.wait_until("event flush before stopping", default_timeout(), |log| {
                    log.calls.contains(&"events").then_some(())
                });
            }
            sender.send(RecorderInstruction::Stop).unwrap();
            thread.join().unwrap();
            let log = log.lock();
            assert_eq!(
                log.calls,
                vec!["start", "events", "incomplete", "session"],
                "{trigger}"
            );
            assert!(log.events.is_empty());
            assert_eq!(log.errors.len(), 1);
            assert!(log.errors[0].contains("failed to record events: injected events failure"));
        }
    }

    #[test]
    fn loss_is_persisted_before_the_pane_closes() {
        let log = Watched::new(RecordingLog {
            fail_once: Some("events"),
            ..Default::default()
        });
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender
            .send(RecorderInstruction::Events(vec![RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(1),
                b"lost".to_vec(),
            )]))
            .unwrap();
        sender
            .send(RecorderInstruction::PaneClosed {
                terminal_id: crate::TerminalId::new(1),
                elapsed_us: ElapsedMicros::new(2),
            })
            .unwrap();
        sender.send(RecorderInstruction::Stop).unwrap();
        run_recorder(&log, receiver, true, 0);
        let log = log.lock();
        // A pane's end timestamp is what makes a listing call it finished, so the loss must be
        // saved before `finish_pane`, not when the session ends.
        assert_eq!(
            log.calls,
            vec!["start", "events", "incomplete", "pane", "session"]
        );
        assert_eq!(log.errors.len(), 1);
        assert!(log.errors[0].contains("injected events failure"));
        assert!(log.errors[0].contains("dropped 1 recording events"));
    }

    #[test]
    fn a_failed_loss_marker_is_retried_at_the_next_flush() {
        let log = Watched::new(RecordingLog {
            fail_once: Some("incomplete"),
            ..Default::default()
        });
        let (sender, receiver) = crossbeam::channel::unbounded();
        let thread = {
            let log = log.clone();
            std::thread::spawn(move || run_recorder(&log, receiver, true, 1))
        };
        let first =
            RecordedEvent::output(TerminalId::new(1), ElapsedMicros::new(1), b"first".to_vec());
        let second = RecordedEvent::output(
            TerminalId::new(1),
            ElapsedMicros::new(2),
            b"second".to_vec(),
        );
        sender
            .send(RecorderInstruction::Events(vec![first.clone()]))
            .unwrap();
        log.wait_until("the first marker write to fail", default_timeout(), |log| {
            (log.marker_writes() == 1).then_some(())
        });
        sender
            .send(RecorderInstruction::Events(vec![second.clone()]))
            .unwrap();
        log.wait_until("the marker to be retried", default_timeout(), |log| {
            (log.marker_writes() == 2).then_some(())
        });
        sender.send(RecorderInstruction::Stop).unwrap();
        thread.join().unwrap();
        let log = log.lock();
        // Idle flushes between the two batches do not retry, and finalization does not rewrite
        // a marker that is already saved.
        assert_eq!(
            log.calls,
            vec![
                "start",
                "events",
                "incomplete",
                "events",
                "incomplete",
                "session"
            ]
        );
        assert_eq!(log.events, vec![first, second]);
        assert_eq!(log.errors, vec!["dropped 1 recording events".to_owned()]);
    }

    #[test]
    fn failed_loss_marker_leaves_session_unfinalized() {
        let log = Watched::new(RecordingLog {
            fail_once: Some("incomplete"),
            ..Default::default()
        });
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender.send(RecorderInstruction::Stop).unwrap();
        run_recorder(&log, receiver, true, 1);
        assert_eq!(log.lock().calls, vec!["start", "incomplete"]);
    }

    #[test]
    fn a_marker_that_never_saves_leaves_the_session_unfinalized() {
        let log = Watched::new(RecordingLog {
            fail_always: Some("incomplete"),
            ..Default::default()
        });
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender
            .send(RecorderInstruction::Events(vec![RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(1),
                b"x".to_vec(),
            )]))
            .unwrap();
        sender.send(RecorderInstruction::Stop).unwrap();
        run_recorder(&log, receiver, true, 1);
        let log = log.lock();
        // One attempt at the flush, one at finalization, and no end timestamp either way.
        assert_eq!(
            log.calls,
            vec!["start", "events", "incomplete", "incomplete"]
        );
        assert!(log.errors.is_empty());
    }

    #[test]
    fn records_through_thread() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (handle, thread) = spawn_session_recorder(
            Arc::new(DataAccess::sqlite(dir.path())),
            dir.path().to_path_buf(),
            "test".to_string(),
            true,
        )
        .unwrap();
        let metadata = PaneMetadata {
            terminal_id: TerminalId::new(1),
            tab_id: 0,
            tab_position: 0,
            tab_name: "tab".to_string(),
            title: "pane".to_string(),
            cols: 80,
            rows: 24,
        };
        handle.pane_metadata(vec![metadata]);
        handle.output(crate::TerminalId::new(1), b"hello".to_vec());
        handle.input(crate::TerminalId::new(1), b"input".to_vec());
        handle.resize(crate::TerminalId::new(1), 100, 30);
        handle.stop();
        thread.join().unwrap();
        // open DB and assert
        let db_path = dir.path().join("session-recordings.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        let session_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM recording_sessions", [], |r| r.get(0))?;
        assert_eq!(session_count, 1);
        let pane_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM recording_panes", [], |r| r.get(0))?;
        assert_eq!(pane_count, 1);
        let event_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM recording_events", [], |r| r.get(0))?;
        assert_eq!(event_count, 3);
        let ended: Option<i64> = conn.query_row(
            "SELECT ended_elapsed_us FROM recording_panes WHERE terminal_id = 1",
            [],
            |r| r.get(0),
        )?;
        assert!(ended.is_some());
        let sess_ended: Option<i64> =
            conn.query_row("SELECT ended_at_us FROM recording_sessions", [], |r| {
                r.get(0)
            })?;
        assert!(sess_ended.is_some());
        Ok(())
    }

    #[test]
    fn overflow_drops_and_does_not_block_or_disable() {
        // Create a bounded channel with capacity 1 and a receiver that never reads
        let (sender, _receiver) = crossbeam::channel::bounded::<RecorderInstruction>(1);
        // fill it
        sender
            .try_send(RecorderInstruction::Events(vec![RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(0),
                b"x".to_vec(),
            )]))
            .unwrap();
        assert!(sender.is_full());
        let enabled = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicUsize::new(0));
        let handle = SessionRecorderHandle {
            sender: sender.clone(),
            started_at: Instant::now(),
            capture_input: true,
            enabled: enabled.clone(),
            dropped: dropped.clone(),
        };
        // this should not block, should drop and metric increment, but not disable
        let start = Instant::now();
        handle.output(crate::TerminalId::new(1), b"y".to_vec());
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(100), "should not block");
        assert!(
            enabled.load(Ordering::Relaxed),
            "should remain enabled on Full"
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        // second call should also try and drop, not be no-op due to disabled
        handle.output(crate::TerminalId::new(1), b"z".to_vec());
        assert_eq!(sender.len(), 1); // still full, not increased
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert!(enabled.load(Ordering::Relaxed));
        // disconnected case should still disable
        let (sender2, receiver2) = crossbeam::channel::bounded::<RecorderInstruction>(1);
        drop(receiver2);
        let enabled2 = Arc::new(AtomicBool::new(true));
        let dropped2 = Arc::new(AtomicUsize::new(0));
        let handle2 = SessionRecorderHandle {
            sender: sender2,
            started_at: Instant::now(),
            capture_input: true,
            enabled: enabled2.clone(),
            dropped: dropped2.clone(),
        };
        handle2.output(crate::TerminalId::new(1), b"a".to_vec());
        assert!(
            !enabled2.load(Ordering::Relaxed),
            "disconnected should disable"
        );
    }

    #[test]
    fn dropped_events_mark_session_incomplete() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender.send(RecorderInstruction::Stop)?;
        recorder_thread(
            Arc::new(DataAccess::sqlite(dir.path())),
            "test".to_string(),
            false,
            Instant::now(),
            UnixMicros::new(1_000),
            receiver,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(1)),
        );
        let conn = rusqlite::Connection::open(dir.path().join("session-recordings.db"))?;
        let error: String =
            conn.query_row("SELECT error FROM recording_sessions", [], |row| row.get(0))?;
        assert_eq!(error, "dropped 1 recording events");
        Ok(())
    }

    #[test]
    fn invalid_data_dir_returns_none() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("file");
        std::fs::write(&file_path, b"hello").unwrap();
        let result = spawn_session_recorder(
            Arc::new(DataAccess::sqlite(dir.path())),
            file_path,
            "test".to_string(),
            true,
        );
        assert!(result.is_none(), "should return None for invalid data_dir");
    }

    #[test]
    fn input_respected_capture_flag() {
        let (sender, receiver) = crossbeam::channel::bounded(10);
        let enabled = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicUsize::new(0));
        let handle = SessionRecorderHandle {
            sender: sender.clone(),
            started_at: Instant::now(),
            capture_input: false,
            enabled: enabled.clone(),
            dropped: dropped.clone(),
        };
        handle.input(crate::TerminalId::new(1), b"secret".to_vec());
        assert!(
            receiver.is_empty(),
            "input should be ignored when capture_input false"
        );
        // output should still work
        handle.output(crate::TerminalId::new(1), b"hello".to_vec());
        assert_eq!(receiver.len(), 1);
    }
}
