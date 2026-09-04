use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use zellij_utils::data_access::{DataAccess, RecordingSession};
use zellij_utils::errors::prelude::*;
use zellij_utils::session_recording::{PaneMetadata, RecordedEvent};

#[derive(Debug)]
pub enum RecorderInstruction {
    Events(Vec<RecordedEvent>),
    PaneMetadata {
        elapsed_us: i64,
        panes: Vec<PaneMetadata>,
    },
    PaneClosed {
        terminal_id: u32,
        elapsed_us: i64,
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
    pub fn output(&self, terminal_id: u32, data: Vec<u8>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
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

    pub fn input(&self, terminal_id: u32, data: Vec<u8>) {
        if !self.capture_input {
            return;
        }
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
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

    pub fn resize(&self, terminal_id: u32, cols: u16, rows: u16) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
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
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
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

    pub fn pane_closed(&self, terminal_id: u32) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
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
    let started_at_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or_else(|e| {
            log::warn!(
                "system clock is before UNIX_EPOCH ({}), using 0 for session start",
                e
            );
            0
        });
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

fn remember_recording_error(result: anyhow::Result<()>, first_error: &mut Option<String>) {
    use zellij_utils::errors::prelude::*;

    if let Err(error) = result {
        first_error.get_or_insert_with(|| format!("{error:#}"));
        Err::<(), _>(error).non_fatal();
    }
}

fn mark_recording_loss(
    store: &mut dyn RecordingSession,
    first_error: &Option<String>,
    dropped: &AtomicUsize,
    enabled: bool,
) -> anyhow::Result<()> {
    let mut reasons: Vec<String> = first_error.iter().cloned().collect();
    let dropped = dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        reasons.push(format!("dropped {dropped} recording events"));
    } else if !enabled {
        reasons.push("recorder channel full or disconnected".to_owned());
    }
    if let Some(reason) = (!reasons.is_empty()).then(|| reasons.join("; ")) {
        store
            .mark_incomplete(&reason)
            .context("failed to mark recording incomplete")?;
    }
    Ok(())
}

fn flush_recording_batch(
    store: &mut dyn RecordingSession,
    batch: &mut Vec<RecordedEvent>,
    batch_start: &mut Option<Instant>,
    first_error: &mut Option<String>,
    dropped: &AtomicUsize,
) {
    use zellij_utils::errors::prelude::*;

    if !batch.is_empty() {
        let result = store
            .record_events(batch)
            .context("failed to record events");
        if result.is_err() {
            dropped.fetch_add(batch.len(), Ordering::Relaxed);
        }
        remember_recording_error(result, first_error);
        // The backend does not promise retry safety. Discard failed batches, but
        // remember the failure so finalization cannot report a complete recording.
        batch.clear();
    }
    *batch_start = None;
}

fn recorder_thread(
    data_access: Arc<DataAccess>,
    session_name: String,
    capture_input: bool,
    started_at: Instant,
    started_at_us: i64,
    receiver: crossbeam::channel::Receiver<RecorderInstruction>,
    enabled: Arc<AtomicBool>,
    dropped: Arc<AtomicUsize>,
) {
    use zellij_utils::errors::prelude::*;

    let mut store =
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
    let mut batch: Vec<RecordedEvent> = Vec::new();
    let mut batch_start: Option<Instant> = None;
    let mut first_error = None;
    let flush_interval = Duration::from_millis(100);
    loop {
        if batch.len() >= 256 || batch_start.is_some_and(|start| start.elapsed() >= flush_interval)
        {
            flush_recording_batch(
                store.as_mut(),
                &mut batch,
                &mut batch_start,
                &mut first_error,
                &dropped,
            );
        }
        let timeout = batch_start
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
                if batch.is_empty() {
                    batch_start = Some(Instant::now());
                }
                batch.append(&mut events);
            },
            Ok(RecorderInstruction::PaneMetadata { elapsed_us, panes }) => {
                flush_recording_batch(
                    store.as_mut(),
                    &mut batch,
                    &mut batch_start,
                    &mut first_error,
                    &dropped,
                );
                remember_recording_error(
                    store
                        .update_panes(&panes, elapsed_us)
                        .context("failed to update panes"),
                    &mut first_error,
                );
            },
            Ok(RecorderInstruction::PaneClosed {
                terminal_id,
                elapsed_us,
            }) => {
                flush_recording_batch(
                    store.as_mut(),
                    &mut batch,
                    &mut batch_start,
                    &mut first_error,
                    &dropped,
                );
                remember_recording_error(
                    store
                        .finish_pane(terminal_id, elapsed_us)
                        .with_context(|| format!("failed to finish pane {terminal_id}")),
                    &mut first_error,
                );
            },
            Ok(RecorderInstruction::Stop)
            | Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                flush_recording_batch(
                    store.as_mut(),
                    &mut batch,
                    &mut batch_start,
                    &mut first_error,
                    &dropped,
                );
            },
        }
    }
    flush_recording_batch(
        store.as_mut(),
        &mut batch,
        &mut batch_start,
        &mut first_error,
        &dropped,
    );
    let ended_at_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or_else(|e| {
            log::warn!(
                "system clock is before UNIX_EPOCH on session end ({}), using 0",
                e
            );
            0
        });
    let ended_elapsed_us = started_at.elapsed().as_micros() as i64;
    if let Err(error) = mark_recording_loss(
        store.as_mut(),
        &first_error,
        &dropped,
        enabled.load(Ordering::Relaxed),
    ) {
        Err::<(), _>(error)
            .context("leaving recording unfinalized because its loss marker could not be saved")
            .non_fatal();
        return;
    }
    let finish_session = store
        .finish_session(ended_at_us, ended_elapsed_us)
        .context("failed to finish session");
    if finish_session.is_err() {
        remember_recording_error(finish_session, &mut first_error);
        mark_recording_loss(
            store.as_mut(),
            &first_error,
            &dropped,
            enabled.load(Ordering::Relaxed),
        )
        .non_fatal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zellij_test_support::{default_timeout, Watched};
    use zellij_utils::data_access::{PluginDataBackend, RecordingBackend, RecordingSession};
    use zellij_utils::plugin_data::{
        ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
        Mutation,
    };

    #[derive(Default)]
    struct RecordingLog {
        calls: Vec<&'static str>,
        events: Vec<RecordedEvent>,
        errors: Vec<String>,
        fail_once: Option<&'static str>,
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
            _: i64,
        ) -> anyhow::Result<Box<dyn RecordingSession>> {
            self.0.lock().calls.push("start");
            Ok(Box::new(MemoryRecordingSession(self.0.clone())))
        }

        fn list(&self) -> anyhow::Result<Vec<zellij_utils::data::RecordingSummary>> {
            Ok(Vec::new())
        }

        fn search(&self, _: &str, _: usize) -> anyhow::Result<Vec<zellij_utils::data::SearchHit>> {
            Ok(Vec::new())
        }

        fn export(&self, _: i64, _: &mut dyn std::io::Write) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl RecordingSession for MemoryRecordingSession {
        fn record_events(&mut self, events: &[RecordedEvent]) -> anyhow::Result<()> {
            self.call("events")?;
            self.0.lock().events.extend_from_slice(events);
            Ok(())
        }

        fn update_panes(&mut self, _: &[PaneMetadata], _: i64) -> anyhow::Result<()> {
            self.call("panes")
        }

        fn finish_pane(&mut self, _: u32, _: i64) -> anyhow::Result<()> {
            self.call("pane")
        }

        fn finish_session(&mut self, _: i64, _: i64) -> anyhow::Result<()> {
            self.call("session")
        }

        fn mark_incomplete(&mut self, message: &str) -> anyhow::Result<()> {
            self.call("incomplete")?;
            self.0.lock().errors.push(message.to_owned());
            Ok(())
        }
    }

    #[test]
    fn recorder_thread_uses_an_injected_backend() -> anyhow::Result<()> {
        let calls = Watched::new(RecordingLog::default());
        let backend = Arc::new(MemoryRecordingBackend(calls.clone()));
        let data_access = Arc::new(DataAccess::from_backends(backend.clone(), backend));
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender.send(RecorderInstruction::Events(vec![RecordedEvent::output(
            1,
            1,
            b"x".to_vec(),
        )]))?;
        sender.send(RecorderInstruction::PaneMetadata {
            elapsed_us: 2,
            panes: Vec::new(),
        })?;
        sender.send(RecorderInstruction::PaneClosed {
            terminal_id: 1,
            elapsed_us: 3,
        })?;
        sender.send(RecorderInstruction::Stop)?;

        recorder_thread(
            data_access,
            "test".to_owned(),
            false,
            Instant::now(),
            0,
            receiver,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(0)),
        );

        assert_eq!(
            calls.lock().calls,
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
                let backend = Arc::new(MemoryRecordingBackend(log.clone()));
                let (sender, receiver) = crossbeam::channel::unbounded();
                let first = RecordedEvent::output(1, 1, b"first".to_vec());
                let last = RecordedEvent::output(1, 4, b"last".to_vec());
                sender
                    .send(RecorderInstruction::Events(vec![first.clone()]))
                    .unwrap();
                sender
                    .send(RecorderInstruction::PaneMetadata {
                        elapsed_us: 2,
                        panes: vec![],
                    })
                    .unwrap();
                sender
                    .send(RecorderInstruction::PaneClosed {
                        elapsed_us: 3,
                        terminal_id: 1,
                    })
                    .unwrap();
                sender
                    .send(RecorderInstruction::Events(vec![last.clone()]))
                    .unwrap();
                // Exercise disconnect/drain finalization, with no Stop instruction.
                drop(sender);
                recorder_thread(
                    Arc::new(DataAccess::from_backends(backend.clone(), backend)),
                    "test".to_owned(),
                    false,
                    Instant::now(),
                    0,
                    receiver,
                    Arc::new(AtomicBool::new(enabled)),
                    Arc::new(AtomicUsize::new(dropped)),
                );
                let log = log.lock();
                let expected_events = if failure == Some("events") {
                    vec![last]
                } else {
                    vec![first, last]
                };
                assert_eq!(
                    log.events, expected_events,
                    "events must be attempted once, including when draining"
                );
                let mut expected_calls =
                    vec!["start", "events", "panes", "pane", "events", "session"];
                if failure.is_some() || dropped > 0 || !enabled {
                    if failure == Some("session") && (dropped > 0 || !enabled) {
                        expected_calls.insert(expected_calls.len() - 1, "incomplete");
                    }
                    if failure == Some("session") {
                        expected_calls.push("incomplete");
                    } else {
                        expected_calls.insert(expected_calls.len() - 1, "incomplete");
                    }
                    assert_eq!(
                        log.errors.len(),
                        1 + usize::from(failure == Some("session") && (dropped > 0 || !enabled))
                    );
                    if let Some(operation) = failure {
                        assert!(log
                            .errors
                            .iter()
                            .any(|error| error.contains(&format!("injected {operation} failure"))));
                    }
                    let expected_dropped = dropped + usize::from(failure == Some("events"));
                    if expected_dropped > 0 {
                        assert!(log.errors.iter().any(|error| error
                            .contains(&format!("dropped {expected_dropped} recording events"))));
                    } else if !enabled {
                        assert!(log
                            .errors
                            .iter()
                            .any(|error| error.contains("recorder channel full or disconnected")));
                    }
                } else {
                    assert!(log.errors.is_empty());
                }
                assert_eq!(log.calls, expected_calls);
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
            let backend = Arc::new(MemoryRecordingBackend(log.clone()));
            let (sender, receiver) = crossbeam::channel::unbounded();
            let count = if trigger == "size" { 256 } else { 1 };
            sender
                .send(RecorderInstruction::Events(vec![
                    RecordedEvent::output(
                        1,
                        1,
                        vec![1]
                    );
                    count
                ]))
                .unwrap();
            let thread = std::thread::spawn(move || {
                recorder_thread(
                    Arc::new(DataAccess::from_backends(backend.clone(), backend)),
                    "test".to_owned(),
                    false,
                    Instant::now(),
                    0,
                    receiver,
                    Arc::new(AtomicBool::new(true)),
                    Arc::new(AtomicUsize::new(0)),
                )
            });
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
    fn failed_loss_marker_leaves_session_unfinalized() {
        let log = Watched::new(RecordingLog {
            fail_once: Some("incomplete"),
            ..Default::default()
        });
        let backend = Arc::new(MemoryRecordingBackend(log.clone()));
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender.send(RecorderInstruction::Stop).unwrap();
        recorder_thread(
            Arc::new(DataAccess::from_backends(backend.clone(), backend)),
            "test".to_owned(),
            false,
            Instant::now(),
            0,
            receiver,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicUsize::new(1)),
        );
        assert_eq!(log.lock().calls, vec!["start", "incomplete"]);
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
            terminal_id: 1,
            tab_id: 0,
            tab_position: 0,
            tab_name: "tab".to_string(),
            title: "pane".to_string(),
            cols: 80,
            rows: 24,
        };
        handle.pane_metadata(vec![metadata]);
        handle.output(1, b"hello".to_vec());
        handle.input(1, b"input".to_vec());
        handle.resize(1, 100, 30);
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
                1,
                0,
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
        handle.output(1, b"y".to_vec());
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(100), "should not block");
        assert!(
            enabled.load(Ordering::Relaxed),
            "should remain enabled on Full"
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        // second call should also try and drop, not be no-op due to disabled
        handle.output(1, b"z".to_vec());
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
        handle2.output(1, b"a".to_vec());
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
            1_000,
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
        handle.input(1, b"secret".to_vec());
        assert!(
            receiver.is_empty(),
            "input should be ignored when capture_input false"
        );
        // output should still work
        handle.output(1, b"hello".to_vec());
        assert_eq!(receiver.len(), 1);
    }
}
