use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

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
    pub sender: crossbeam::channel::Sender<RecorderInstruction>,
    pub started_at: Instant,
    pub capture_input: bool,
    pub enabled: Arc<AtomicBool>,
}

impl SessionRecorderHandle {
    pub fn output(&self, terminal_id: u32, data: Vec<u8>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
        let event = RecordedEvent::output(terminal_id, elapsed_us, data);
        if let Err(_) = self
            .sender
            .try_send(RecorderInstruction::Events(vec![event]))
        {
            self.enabled.store(false, Ordering::Relaxed);
            log::error!("session recorder channel full or disconnected, disabling");
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
        if let Err(_) = self
            .sender
            .try_send(RecorderInstruction::Events(vec![event]))
        {
            self.enabled.store(false, Ordering::Relaxed);
            log::error!("session recorder channel full or disconnected, disabling");
        }
    }

    pub fn resize(&self, terminal_id: u32, cols: u16, rows: u16) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
        let event = RecordedEvent::resize(terminal_id, elapsed_us, cols, rows);
        if let Err(_) = self
            .sender
            .try_send(RecorderInstruction::Events(vec![event]))
        {
            self.enabled.store(false, Ordering::Relaxed);
            log::error!("session recorder channel full or disconnected, disabling");
        }
    }

    pub fn pane_metadata(&self, panes: Vec<PaneMetadata>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
        let _ = self
            .sender
            .send(RecorderInstruction::PaneMetadata { elapsed_us, panes });
    }

    pub fn pane_closed(&self, terminal_id: u32) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_us = self.started_at.elapsed().as_micros() as i64;
        let _ = self.sender.send(RecorderInstruction::PaneClosed {
            terminal_id,
            elapsed_us,
        });
    }

    pub fn stop(&self) {
        let _ = self.sender.send(RecorderInstruction::Stop);
    }
}

pub fn spawn_session_recorder(
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
    let (sender, receiver) = crossbeam::channel::bounded(1024);
    let enabled = Arc::new(AtomicBool::new(true));
    let handle = SessionRecorderHandle {
        sender: sender.clone(),
        started_at: Instant::now(),
        capture_input,
        enabled: enabled.clone(),
    };
    let enabled_clone = enabled.clone();
    let started_at = handle.started_at;
    let thread = std::thread::spawn(move || {
        recorder_thread(
            data_dir,
            session_name,
            capture_input,
            started_at,
            receiver,
            enabled_clone,
        );
    });
    Some((handle, thread))
}

fn recorder_thread(
    data_dir: PathBuf,
    session_name: String,
    capture_input: bool,
    started_at: Instant,
    receiver: crossbeam::channel::Receiver<RecorderInstruction>,
    enabled: Arc<AtomicBool>,
) {
    let started_at_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    let mut store = match zellij_utils::session_recording::RecordingStore::open(
        &data_dir,
        &session_name,
        capture_input,
        started_at_us,
    ) {
        Ok(s) => s,
        Err(e) => {
            log::error!("failed to start session recorder: {}", e);
            return;
        },
    };
    let mut batch: Vec<RecordedEvent> = Vec::new();
    let mut batch_start: Option<Instant> = None;
    let mut stopped = false;
    loop {
        // check batch timeout
        let timeout = if let Some(start) = batch_start {
            let elapsed = start.elapsed();
            if elapsed >= Duration::from_millis(100) {
                // flush batch
                if !batch.is_empty() {
                    let _ = store.record_events(&batch);
                    batch.clear();
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
            let _ = store.record_events(&batch);
            batch.clear();
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
                    let _ = store.record_events(&batch);
                    batch.clear();
                    batch_start = None;
                }
            },
            Ok(RecorderInstruction::PaneMetadata { elapsed_us, panes }) => {
                // flush batch before metadata
                if !batch.is_empty() {
                    let _ = store.record_events(&batch);
                    batch.clear();
                    batch_start = None;
                }
                let _ = store.update_panes(&panes, elapsed_us);
            },
            Ok(RecorderInstruction::PaneClosed {
                terminal_id,
                elapsed_us,
            }) => {
                if !batch.is_empty() {
                    let _ = store.record_events(&batch);
                    batch.clear();
                    batch_start = None;
                }
                let _ = store.finish_pane(terminal_id, elapsed_us);
            },
            Ok(RecorderInstruction::Stop) => {
                stopped = true;
                break;
            },
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                if !batch.is_empty() {
                    let _ = store.record_events(&batch);
                    batch.clear();
                    batch_start = None;
                }
                // check enabled flag
                if !enabled.load(Ordering::Relaxed) {
                    // mark incomplete but flush already done
                    let _ = store.mark_incomplete("recorder channel full or disconnected");
                    // drain remaining instructions without blocking?
                    while let Ok(instr) = receiver.try_recv() {
                        match instr {
                            RecorderInstruction::Events(mut ev) => {
                                batch.append(&mut ev);
                                if batch.len() >= 256 {
                                    let _ = store.record_events(&batch);
                                    batch.clear();
                                }
                            },
                            RecorderInstruction::PaneMetadata { elapsed_us, panes } => {
                                if !batch.is_empty() {
                                    let _ = store.record_events(&batch);
                                    batch.clear();
                                    batch_start = None;
                                }
                                let _ = store.update_panes(&panes, elapsed_us);
                            },
                            RecorderInstruction::PaneClosed {
                                terminal_id,
                                elapsed_us,
                            } => {
                                if !batch.is_empty() {
                                    let _ = store.record_events(&batch);
                                    batch.clear();
                                    batch_start = None;
                                }
                                let _ = store.finish_pane(terminal_id, elapsed_us);
                            },
                            RecorderInstruction::Stop => {
                                stopped = true;
                                break;
                            },
                        }
                    }
                    if !batch.is_empty() {
                        let _ = store.record_events(&batch);
                    }
                    break;
                }
            },
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                break;
            },
        }
        if stopped {
            break;
        }
        if !enabled.load(Ordering::Relaxed) {
            // flush and mark incomplete
            if !batch.is_empty() {
                let _ = store.record_events(&batch);
                batch.clear();
            }
            let _ = store.mark_incomplete("recorder channel full or disconnected");
            // drain
            while let Ok(instr) = receiver.try_recv() {
                match instr {
                    RecorderInstruction::Events(mut ev) => batch.append(&mut ev),
                    RecorderInstruction::PaneMetadata { elapsed_us, panes } => {
                        if !batch.is_empty() {
                            let _ = store.record_events(&batch);
                            batch.clear();
                        }
                        let _ = store.update_panes(&panes, elapsed_us);
                    },
                    RecorderInstruction::PaneClosed {
                        terminal_id,
                        elapsed_us,
                    } => {
                        if !batch.is_empty() {
                            let _ = store.record_events(&batch);
                            batch.clear();
                        }
                        let _ = store.finish_pane(terminal_id, elapsed_us);
                    },
                    RecorderInstruction::Stop => break,
                }
            }
            if !batch.is_empty() {
                let _ = store.record_events(&batch);
            }
            break;
        }
    }
    // flush remaining
    if !batch.is_empty() {
        let _ = store.record_events(&batch);
    }
    let ended_at_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    let _ = store.finish_session(ended_at_us);
    // if disabled, mark incomplete
    if !enabled.load(Ordering::Relaxed) {
        let _ = store.mark_incomplete("recorder channel full or disconnected");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zellij_utils::session_recording::RecordingStore;

    #[test]
    fn records_through_thread() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (handle, thread) =
            spawn_session_recorder(dir.path().to_path_buf(), "test".to_string(), true).unwrap();
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
        handle.pane_closed(1);
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
    fn overflow_disables_and_does_not_block() {
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
        let handle = SessionRecorderHandle {
            sender: sender.clone(),
            started_at: Instant::now(),
            capture_input: true,
            enabled: enabled.clone(),
        };
        // this should not block, should set enabled to false
        let start = Instant::now();
        handle.output(1, b"y".to_vec());
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(100), "should not block");
        assert!(!enabled.load(Ordering::Relaxed));
        // second call should be no-op due to disabled
        handle.output(1, b"z".to_vec());
        assert_eq!(sender.len(), 1); // still full, not increased
    }

    #[test]
    fn invalid_data_dir_returns_none() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("file");
        std::fs::write(&file_path, b"hello").unwrap();
        let result = spawn_session_recorder(file_path, "test".to_string(), true);
        assert!(result.is_none(), "should return None for invalid data_dir");
    }

    #[test]
    fn input_respected_capture_flag() {
        let (sender, receiver) = crossbeam::channel::bounded(10);
        let enabled = Arc::new(AtomicBool::new(true));
        let handle = SessionRecorderHandle {
            sender: sender.clone(),
            started_at: Instant::now(),
            capture_input: false,
            enabled: enabled.clone(),
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
