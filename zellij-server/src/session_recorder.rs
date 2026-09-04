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

fn mark_recording_loss(
    store: &mut dyn RecordingSession,
    dropped: &AtomicUsize,
    enabled: bool,
) -> Result<()> {
    let dropped = dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        store
            .mark_incomplete(&format!("dropped {} recording events", dropped))
            .context("failed to mark recording incomplete")?;
    } else if !enabled {
        store
            .mark_incomplete("recorder channel full or disconnected")
            .context("failed to mark recording incomplete")?;
    }
    Ok(())
}

fn flush_recording_events(
    store: &mut dyn RecordingSession,
    batch: &mut Vec<RecordedEvent>,
    dropped: &AtomicUsize,
) {
    if !batch.is_empty() {
        if let Err(error) = store.record_events(batch) {
            dropped.fetch_add(batch.len(), Ordering::Relaxed);
            Err::<(), _>(error)
                .context("failed to record events; counting discarded batch")
                .non_fatal();
        }
        batch.clear();
    }
    // Retry only the loss marker, never the discarded events. A later healthy
    // write must not make a recording with missing output look complete.
    mark_recording_loss(store, dropped, true).non_fatal();
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
    let mut store =
        match data_access
            .recordings()
            .start_recording(&session_name, capture_input, started_at_us)
        {
            Ok(s) => s,
            Err(e) => {
                log::error!("failed to start session recorder: {}", e);
                return;
            },
        };
    let mut batch: Vec<RecordedEvent> = Vec::new();
    let mut batch_start: Option<Instant> = None;
    loop {
        // check batch timeout
        let timeout = if let Some(start) = batch_start {
            let elapsed = start.elapsed();
            if elapsed >= Duration::from_millis(100) {
                // flush batch
                if !batch.is_empty() {
                    flush_recording_events(store.as_mut(), &mut batch, &dropped);
                    batch_start = None;
                }
                Duration::from_millis(100)
            } else {
                Duration::from_millis(100) - elapsed
            }
        } else {
            Duration::from_millis(100)
        };
        // also flush on 256
        if batch.len() >= 256 {
            flush_recording_events(store.as_mut(), &mut batch, &dropped);
            batch_start = None;
            continue;
        }
        match receiver.recv_timeout(timeout) {
            Ok(RecorderInstruction::Events(mut events)) => {
                if batch.is_empty() {
                    batch_start = Some(Instant::now());
                }
                batch.append(&mut events);
                if batch.len() >= 256 {
                    flush_recording_events(store.as_mut(), &mut batch, &dropped);
                    batch_start = None;
                }
            },
            Ok(RecorderInstruction::PaneMetadata { elapsed_us, panes }) => {
                // flush batch before metadata
                if !batch.is_empty() {
                    flush_recording_events(store.as_mut(), &mut batch, &dropped);
                    batch_start = None;
                }
                if let Err(e) = store.update_panes(&panes, elapsed_us) {
                    log::error!("failed to update panes: {}", e);
                }
            },
            Ok(RecorderInstruction::PaneClosed {
                terminal_id,
                elapsed_us,
            }) => {
                if !batch.is_empty() {
                    flush_recording_events(store.as_mut(), &mut batch, &dropped);
                    batch_start = None;
                }
                if let Err(e) = store.finish_pane(terminal_id, elapsed_us) {
                    log::error!("failed to finish pane {}: {}", terminal_id, e);
                }
            },
            Ok(RecorderInstruction::Stop) => {
                break;
            },
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                if !batch.is_empty() {
                    flush_recording_events(store.as_mut(), &mut batch, &dropped);
                    batch_start = None;
                }
            },
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                break;
            },
        }
        if !enabled.load(Ordering::Relaxed) {
            // flush and mark incomplete
            flush_recording_events(store.as_mut(), &mut batch, &dropped);
            mark_recording_loss(store.as_mut(), &dropped, false).non_fatal();
            // drain
            while let Ok(instr) = receiver.try_recv() {
                match instr {
                    RecorderInstruction::Events(mut ev) => {
                        batch.append(&mut ev);
                        if batch.len() >= 256 {
                            flush_recording_events(store.as_mut(), &mut batch, &dropped);
                        }
                    },
                    RecorderInstruction::PaneMetadata { elapsed_us, panes } => {
                        flush_recording_events(store.as_mut(), &mut batch, &dropped);
                        if let Err(e) = store.update_panes(&panes, elapsed_us) {
                            log::error!("failed to update panes: {}", e);
                        }
                    },
                    RecorderInstruction::PaneClosed {
                        terminal_id,
                        elapsed_us,
                    } => {
                        flush_recording_events(store.as_mut(), &mut batch, &dropped);
                        if let Err(e) = store.finish_pane(terminal_id, elapsed_us) {
                            log::error!("failed to finish pane {}: {}", terminal_id, e);
                        }
                    },
                    RecorderInstruction::Stop => break,
                }
            }
            break;
        }
    }
    // flush remaining
    flush_recording_events(store.as_mut(), &mut batch, &dropped);
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
    if let Err(error) =
        mark_recording_loss(store.as_mut(), &dropped, enabled.load(Ordering::Relaxed))
    {
        Err::<(), _>(error)
            .context("leaving recording unfinalized because its loss marker could not be saved")
            .non_fatal();
        return;
    }
    store
        .finish_session(ended_at_us, ended_elapsed_us)
        .context("failed to finish session recording")
        .non_fatal();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zellij_utils::data_access::{PluginDataBackend, RecordingBackend, RecordingSession};
    use zellij_utils::plugin_data::{
        ApplyReport, DataEntityInfo, DataError, DataRows, DataSchemaInfo, EntityDef, FetchRequest,
        Mutation,
    };

    #[derive(Default)]
    struct RecordingState {
        calls: Vec<&'static str>,
        fail_events: std::collections::VecDeque<bool>,
        fail_markers: usize,
        saved: Vec<Vec<u8>>,
        error: Option<String>,
        event_attempt: Option<std::sync::mpsc::Sender<()>>,
    }
    struct MemoryRecordingBackend(Arc<std::sync::Mutex<RecordingState>>);
    struct MemoryRecordingSession(Arc<std::sync::Mutex<RecordingState>>);

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
            self.0.lock().unwrap().calls.push("start");
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
            let mut state = self.0.lock().unwrap();
            state.calls.push("events");
            if let Some(sender) = &state.event_attempt {
                sender.send(()).unwrap();
            }
            if state.fail_events.pop_front().unwrap_or(false) {
                anyhow::bail!("injected event write failure");
            }
            state
                .saved
                .extend(events.iter().map(|event| event.data.clone()));
            Ok(())
        }

        fn update_panes(&mut self, _: &[PaneMetadata], _: i64) -> anyhow::Result<()> {
            self.0.lock().unwrap().calls.push("panes");
            Ok(())
        }

        fn finish_pane(&mut self, _: u32, _: i64) -> anyhow::Result<()> {
            self.0.lock().unwrap().calls.push("pane");
            Ok(())
        }

        fn finish_session(&mut self, _: i64, _: i64) -> anyhow::Result<()> {
            self.0.lock().unwrap().calls.push("session");
            Ok(())
        }

        fn mark_incomplete(&mut self, message: &str) -> anyhow::Result<()> {
            let mut state = self.0.lock().unwrap();
            state.calls.push("incomplete");
            if state.fail_markers > 0 {
                state.fail_markers -= 1;
                anyhow::bail!("injected marker write failure");
            }
            state.error = Some(message.to_owned());
            Ok(())
        }
    }

    #[test]
    fn recorder_thread_uses_an_injected_backend() -> anyhow::Result<()> {
        let calls = Arc::new(std::sync::Mutex::new(RecordingState::default()));
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
            calls.lock().unwrap().calls,
            vec!["start", "events", "panes", "pane", "session"]
        );
        Ok(())
    }

    fn run_with_failures(
        instructions: Vec<RecorderInstruction>,
        fail_events: &[bool],
        fail_markers: usize,
        enabled: bool,
    ) -> (RecordingState, usize) {
        let state = Arc::new(std::sync::Mutex::new(RecordingState {
            fail_events: fail_events.iter().copied().collect(),
            fail_markers,
            ..Default::default()
        }));
        let backend = Arc::new(MemoryRecordingBackend(state.clone()));
        let (sender, receiver) = crossbeam::channel::unbounded();
        for instruction in instructions {
            sender.send(instruction).unwrap();
        }
        drop(sender);
        let dropped = Arc::new(AtomicUsize::new(0));
        recorder_thread(
            Arc::new(DataAccess::from_backends(backend.clone(), backend)),
            "test".to_owned(),
            false,
            Instant::now(),
            0,
            receiver,
            Arc::new(AtomicBool::new(enabled)),
            dropped.clone(),
        );
        let state = Arc::try_unwrap(state).ok().unwrap().into_inner().unwrap();
        (state, dropped.load(Ordering::Relaxed))
    }

    fn events(bytes: &[u8], count: usize) -> RecorderInstruction {
        RecorderInstruction::Events(
            (0..count)
                .map(|i| RecordedEvent::output(1, i as i64, bytes.to_vec()))
                .collect(),
        )
    }

    #[test]
    fn failed_batches_are_counted_and_later_writes_keep_the_loss_marker() {
        let (state, dropped) = run_with_failures(
            vec![
                events(b"lost", 256),
                events(b"saved", 1),
                RecorderInstruction::Stop,
            ],
            &[true, false],
            1,
            true,
        );
        assert_eq!(dropped, 256);
        assert_eq!(state.saved, vec![b"saved".to_vec()]);
        assert_eq!(state.error.as_deref(), Some("dropped 256 recording events"));
        assert_eq!(
            state.calls.iter().filter(|call| **call == "events").count(),
            2
        );
        assert_eq!(state.calls.last(), Some(&"session"));
    }

    #[test]
    fn metadata_close_stop_and_disconnection_flushes_count_failed_events() {
        for ending in [
            vec![
                RecorderInstruction::PaneMetadata {
                    elapsed_us: 2,
                    panes: vec![],
                },
                RecorderInstruction::Stop,
            ],
            vec![
                RecorderInstruction::PaneClosed {
                    terminal_id: 1,
                    elapsed_us: 2,
                },
                RecorderInstruction::Stop,
            ],
            vec![RecorderInstruction::Stop],
            vec![],
        ] {
            let mut instructions = vec![events(b"lost", 3)];
            instructions.extend(ending);
            let (state, dropped) = run_with_failures(instructions, &[true], 0, true);
            assert_eq!(dropped, 3);
            assert!(state.saved.is_empty());
            assert_eq!(state.error.as_deref(), Some("dropped 3 recording events"));
            assert_eq!(
                state.calls.iter().filter(|call| **call == "events").count(),
                1
            );
            assert_eq!(state.calls.last(), Some(&"session"));
        }
    }

    #[test]
    fn failed_final_loss_marker_leaves_the_session_unfinalized() {
        let (state, dropped) = run_with_failures(
            vec![events(b"lost", 1), RecorderInstruction::Stop],
            &[true],
            usize::MAX,
            true,
        );
        assert_eq!(dropped, 1);
        assert!(!state.calls.contains(&"session"));
        assert!(state.error.is_none());
    }

    #[test]
    fn fallback_shutdown_flushes_its_tail_exactly_once() {
        for fail_tail in [false, true] {
            let (state, dropped) = run_with_failures(
                vec![events(b"first", 1), events(b"tail", 1)],
                &[false, fail_tail],
                0,
                false,
            );
            assert_eq!(
                state.calls.iter().filter(|call| **call == "events").count(),
                2
            );
            assert_eq!(dropped, usize::from(fail_tail));
            if fail_tail {
                assert_eq!(state.saved, vec![b"first".to_vec()]);
                assert_eq!(state.error.as_deref(), Some("dropped 1 recording events"));
            } else {
                assert_eq!(state.saved, vec![b"first".to_vec(), b"tail".to_vec()]);
            }
        }
    }

    #[test]
    fn timeout_flush_counts_failure_without_waiting_for_another_instruction() {
        let (attempted, attempt) = std::sync::mpsc::channel();
        let state = Arc::new(std::sync::Mutex::new(RecordingState {
            fail_events: [true].into_iter().collect(),
            event_attempt: Some(attempted),
            ..Default::default()
        }));
        let backend = Arc::new(MemoryRecordingBackend(state.clone()));
        let (sender, receiver) = crossbeam::channel::unbounded();
        sender.send(events(b"lost", 1)).unwrap();
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
        attempt.recv_timeout(Duration::from_secs(10)).unwrap();
        sender.send(RecorderInstruction::Stop).unwrap();
        thread.join().unwrap();
        let state = state.lock().unwrap();
        assert_eq!(state.error.as_deref(), Some("dropped 1 recording events"));
        assert!(state.saved.is_empty());
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
