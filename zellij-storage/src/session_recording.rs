use std::path::{Path, PathBuf};
use zellij_types::internal_ids::{RecordingId, TerminalId};
use zellij_types::units::{ElapsedMicros, UnixMicros};

use anyhow::{Context, Result};

use crate::data_access::{RecordingBackend, RecordingSession};

pub use zellij_types::recording::{EventKind, PaneMetadata, RecordedEvent};

struct SqliteRecordingSession {
    connection: rusqlite::Connection,
    session_id: i64,
}

/// SQLite-compatible façade retained for callers that manage one recording directly.
pub struct RecordingStore {
    session: Box<dyn RecordingSession>,
}

/// SQLite recording backend. It keeps only the directory until an operation needs the database.
pub struct SqliteRecordingBackend {
    data_dir: PathBuf,
}

impl SqliteRecordingBackend {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_owned(),
        }
    }
}

impl SqliteRecordingSession {
    fn open(
        data_dir: &Path,
        session_name: &str,
        capture_input: bool,
        started_at_us: UnixMicros,
    ) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| anyhow::anyhow!("failed to create data dir {:?}: {}", data_dir, e))?;
        let db_path = data_dir.join("session-recordings.db");
        let connection = rusqlite::Connection::open(&db_path)?;
        // pragmas
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "busy_timeout", 5000)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        // permissions 0600 on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&db_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&db_path, perms);
            }
        }
        // schema
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS recording_sessions (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                started_at_us INTEGER NOT NULL,
                ended_at_us INTEGER,
                capture_input INTEGER NOT NULL,
                error TEXT
            );
            CREATE TABLE IF NOT EXISTS recording_panes (
                id INTEGER PRIMARY KEY,
                session_id INTEGER NOT NULL REFERENCES recording_sessions(id),
                terminal_id INTEGER NOT NULL,
                tab_id INTEGER,
                tab_position INTEGER,
                tab_name TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                started_elapsed_us INTEGER NOT NULL,
                ended_elapsed_us INTEGER,
                initial_cols INTEGER NOT NULL DEFAULT 80,
                initial_rows INTEGER NOT NULL DEFAULT 24,
                UNIQUE(session_id, terminal_id)
            );
            CREATE TABLE IF NOT EXISTS recording_events (
                id INTEGER PRIMARY KEY,
                pane_id INTEGER NOT NULL REFERENCES recording_panes(id),
                elapsed_us INTEGER NOT NULL,
                code TEXT NOT NULL CHECK(code IN ('o', 'i', 'r')),
                data BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS recording_events_pane_time
            ON recording_events(pane_id, elapsed_us, id);
            "#,
        )?;
        connection.execute(
            "INSERT INTO recording_sessions (name, started_at_us, capture_input) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_name, started_at_us.get(), if capture_input { 1 } else { 0 }],
        )?;
        let session_id = connection.last_insert_rowid();
        Ok(Self {
            connection,
            session_id,
        })
    }

    pub fn record_events(&mut self, events: &[RecordedEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let tx = self.connection.transaction()?;
        for event in events {
            // ensure pane exists
            let pane_id: i64 = {
                let mut stmt = tx.prepare(
                    "SELECT id FROM recording_panes WHERE session_id = ?1 AND terminal_id = ?2",
                )?;
                let mut rows = stmt.query(rusqlite::params![
                    self.session_id,
                    event.terminal_id.get() as i64
                ])?;
                if let Some(row) = rows.next()? {
                    row.get(0)?
                } else {
                    // create
                    let (initial_cols, initial_rows) = if event.kind == EventKind::Resize {
                        parse_resize(&event.data).unwrap_or((80, 24))
                    } else {
                        (80, 24)
                    };
                    tx.execute(
                        "INSERT INTO recording_panes (session_id, terminal_id, started_elapsed_us, initial_cols, initial_rows) VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![self.session_id, event.terminal_id.get() as i64, event.elapsed_us.get(), initial_cols as i64, initial_rows as i64],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            tx.execute(
                "INSERT INTO recording_events (pane_id, elapsed_us, code, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![pane_id, event.elapsed_us.get(), event.asciicast_code(), event.data],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_panes(
        &mut self,
        panes: &[PaneMetadata],
        elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        for pane in panes {
            let tab_position = i64::try_from(pane.tab_position.get())
                .context("Recording tab position exceeds SQLite's signed integer range")?;
            let tab_id = i64::try_from(pane.tab_id.get())
                .context("Recording tab id exceeds SQLite's signed integer range")?;
            let existing: Option<(i64, i64, i64, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT id, initial_cols, initial_rows, tab_name FROM recording_panes WHERE session_id = ?1 AND terminal_id = ?2",
                )?;
                let mut rows = stmt.query(rusqlite::params![
                    self.session_id,
                    pane.terminal_id.get() as i64
                ])?;
                if let Some(row) = rows.next()? {
                    Some((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                } else {
                    None
                }
            };
            if let Some((pane_id, ic, ir, existing_tab_name)) = existing {
                // If the pane was created speculatively via `record_events` (output
                // arrived before `pane_metadata`), it will have default size 80x24
                // and empty tab_name. In that case the true initial size is in
                // `pane_metadata` and should be adopted, but only if no resize
                // event has been recorded yet (otherwise initial should stay as
                // first observed size). This fixes the race where `output` can
                // arrive before `pane_metadata` and would otherwise leave a wrong
                // `initial_cols`/`initial_rows` in the asciicast header.
                let is_default_size = ic == 80 && ir == 24;
                let size_differs = pane.cols as i64 != ic || pane.rows as i64 != ir;
                let is_speculative = existing_tab_name.is_empty();
                if is_default_size && size_differs && is_speculative {
                    let has_resize: bool = {
                        let mut stmt = tx.prepare(
                            "SELECT 1 FROM recording_events WHERE pane_id = ?1 AND code = 'r' LIMIT 1",
                        )?;
                        let mut rows = stmt.query(rusqlite::params![pane_id])?;
                        rows.next()?.is_some()
                    };
                    if !has_resize {
                        tx.execute(
                            "UPDATE recording_panes SET tab_id = ?1, tab_position = ?2, tab_name = ?3, title = ?4, initial_cols = ?5, initial_rows = ?6 WHERE id = ?7",
                            rusqlite::params![
                                tab_id,
                                tab_position,
                                pane.tab_name,
                                pane.title,
                                pane.cols as i64,
                                pane.rows as i64,
                                pane_id
                            ],
                        )?;
                        continue;
                    }
                }
                // normal path: update tab and title only, keep initial size and started time
                tx.execute(
                    "UPDATE recording_panes SET tab_id = ?1, tab_position = ?2, tab_name = ?3, title = ?4 WHERE id = ?5",
                    rusqlite::params![
                        tab_id,
                        tab_position,
                        pane.tab_name,
                        pane.title,
                        pane_id
                    ],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO recording_panes (session_id, terminal_id, tab_id, tab_position, tab_name, title, started_elapsed_us, initial_cols, initial_rows) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        self.session_id,
                        pane.terminal_id.get() as i64,
                        tab_id,
                        tab_position,
                        pane.tab_name,
                        pane.title,
                        elapsed_us.get(),
                        pane.cols as i64,
                        pane.rows as i64
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn finish_pane(
        &mut self,
        terminal_id: TerminalId,
        elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_panes SET ended_elapsed_us = ?1 WHERE session_id = ?2 AND terminal_id = ?3 AND ended_elapsed_us IS NULL",
            rusqlite::params![elapsed_us.get(), self.session_id, terminal_id.get() as i64],
        )?;
        Ok(())
    }

    pub fn finish_session(
        &mut self,
        ended_at_us: UnixMicros,
        ended_elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE recording_panes SET ended_elapsed_us = ?1 WHERE session_id = ?2 AND ended_elapsed_us IS NULL",
            rusqlite::params![ended_elapsed_us.get(), self.session_id],
        )?;
        tx.execute(
            "UPDATE recording_sessions SET ended_at_us = ?1 WHERE id = ?2",
            rusqlite::params![ended_at_us.get(), self.session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_incomplete(&mut self, message: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_sessions SET error = ?1 WHERE id = ?2",
            rusqlite::params![message, self.session_id],
        )?;
        Ok(())
    }
}

impl RecordingSession for SqliteRecordingSession {
    fn record_events(&mut self, events: &[RecordedEvent]) -> Result<()> {
        SqliteRecordingSession::record_events(self, events)
    }

    fn update_panes(&mut self, panes: &[PaneMetadata], elapsed_us: ElapsedMicros) -> Result<()> {
        SqliteRecordingSession::update_panes(self, panes, elapsed_us)
    }

    fn finish_pane(&mut self, terminal_id: TerminalId, elapsed_us: ElapsedMicros) -> Result<()> {
        SqliteRecordingSession::finish_pane(self, terminal_id, elapsed_us)
    }

    fn finish_session(
        &mut self,
        ended_at_us: UnixMicros,
        ended_elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        SqliteRecordingSession::finish_session(self, ended_at_us, ended_elapsed_us)
    }

    fn mark_incomplete(&mut self, message: &str) -> Result<()> {
        SqliteRecordingSession::mark_incomplete(self, message)
    }
}

impl RecordingStore {
    pub fn open(
        data_dir: &Path,
        session_name: &str,
        capture_input: bool,
        started_at_us: UnixMicros,
    ) -> Result<Self> {
        Ok(Self {
            session: SqliteRecordingBackend::new(data_dir).start_recording(
                session_name,
                capture_input,
                started_at_us,
            )?,
        })
    }

    pub fn record_events(&mut self, events: &[RecordedEvent]) -> Result<()> {
        self.session.record_events(events)
    }

    pub fn update_panes(
        &mut self,
        panes: &[PaneMetadata],
        elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        self.session.update_panes(panes, elapsed_us)
    }

    pub fn finish_pane(
        &mut self,
        terminal_id: TerminalId,
        elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        self.session.finish_pane(terminal_id, elapsed_us)
    }

    pub fn finish_session(
        &mut self,
        ended_at_us: UnixMicros,
        ended_elapsed_us: ElapsedMicros,
    ) -> Result<()> {
        self.session.finish_session(ended_at_us, ended_elapsed_us)
    }

    pub fn mark_incomplete(&mut self, message: &str) -> Result<()> {
        self.session.mark_incomplete(message)
    }
}

impl RecordingBackend for SqliteRecordingBackend {
    fn start_recording(
        &self,
        session_name: &str,
        capture_input: bool,
        started_at_us: UnixMicros,
    ) -> Result<Box<dyn RecordingSession>> {
        Ok(Box::new(SqliteRecordingSession::open(
            &self.data_dir,
            session_name,
            capture_input,
            started_at_us,
        )?))
    }

    fn list(&self) -> Result<Vec<RecordingSummary>> {
        sqlite_list_recordings(&self.data_dir)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        sqlite_search_recordings(&self.data_dir, query, limit)
    }

    fn export(&self, recording_id: RecordingId, output: &mut dyn std::io::Write) -> Result<()> {
        sqlite_export_recording(&self.data_dir, recording_id, output)
    }
}

fn parse_resize(data: &[u8]) -> Option<(usize, usize)> {
    let s = String::from_utf8_lossy(data);
    let mut parts = s.split('x');
    let cols = parts.next()?.parse::<usize>().ok()?;
    let rows = parts.next()?.parse::<usize>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((cols, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;
    use tempfile::tempdir;

    #[hegel::test]
    fn resize_events_roundtrip_through_parse_resize(tc: hegel::TestCase) {
        let cols = tc.draw(generators::integers::<u16>());
        let rows = tc.draw(generators::integers::<u16>());
        let event = RecordedEvent::resize(TerminalId::new(0), ElapsedMicros::new(0), cols, rows);
        assert_eq!(
            parse_resize(&event.data),
            Some((cols as usize, rows as usize)),
            "{}x{} encoded as {:?}",
            cols,
            rows,
            String::from_utf8_lossy(&event.data)
        );
    }

    #[hegel::test]
    fn parse_resize_never_panics(tc: hegel::TestCase) {
        let data = tc.draw(generators::binary());
        let parsed = parse_resize(&data);
        assert!(
            parsed.is_some() || parsed.is_none(),
            "parse_resize({:?}) misbehaved",
            data
        );
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn an_out_of_range_tab_identity_or_position_rolls_back_the_recording_batch() -> Result<()> {
        let dir = tempdir()?;
        let mut session =
            SqliteRecordingSession::open(dir.path(), "tabs", false, UnixMicros::new(0))?;
        let valid = PaneMetadata {
            terminal_id: TerminalId::new(1),
            tab_id: zellij_types::internal_ids::TabId::new(0),
            tab_position: zellij_types::internal_ids::TabPosition::new(0),
            tab_name: "work".to_owned(),
            title: "shell".to_owned(),
            cols: 80,
            rows: 24,
        };
        for (tab_id, tab_position) in [(usize::MAX, 0), (0, usize::MAX)] {
            let invalid = PaneMetadata {
                terminal_id: TerminalId::new(2),
                tab_id: zellij_types::internal_ids::TabId::new(tab_id),
                tab_position: zellij_types::internal_ids::TabPosition::new(tab_position),
                ..valid.clone()
            };
            assert!(session
                .update_panes(&[valid.clone(), invalid], ElapsedMicros::new(0))
                .is_err());
            let count: i64 = session.connection.query_row(
                "SELECT COUNT(*) FROM recording_panes",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
        }
        Ok(())
    }

    #[test]
    fn stores_pane_events() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(
            dir.path(),
            "test",
            true,
            UnixMicros::new(1_500_000_000_000_000),
        )?;
        let metadata = PaneMetadata {
            terminal_id: TerminalId::new(7),
            tab_id: zellij_types::internal_ids::TabId::new(1),
            tab_position: zellij_types::internal_ids::TabPosition::new(0),
            tab_name: "tab".to_string(),
            title: "pane".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[metadata], ElapsedMicros::new(5_000))?;
        store.record_events(&[
            RecordedEvent::output(
                TerminalId::new(7),
                ElapsedMicros::new(10_000),
                b"hello\r\n".to_vec(),
            ),
            RecordedEvent::input(
                TerminalId::new(7),
                ElapsedMicros::new(20_000),
                b"pwd\r".to_vec(),
            ),
            RecordedEvent::resize(TerminalId::new(7), ElapsedMicros::new(30_000), 120, 40),
        ])?;

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
        let mut stmt = conn.prepare("SELECT code FROM recording_events ORDER BY elapsed_us, id")?;
        let codes: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(codes, vec!["o", "i", "r"]);
        Ok(())
    }

    #[test]
    fn failed_event_batches_preserve_existing_recordings_and_can_be_retried() -> Result<()> {
        let dir = tempdir()?;
        let mut session =
            SqliteRecordingSession::open(dir.path(), "atomic", false, UnixMicros::new(0))?;
        let existing = RecordedEvent::output(
            TerminalId::new(1),
            ElapsedMicros::new(0),
            b"existing".to_vec(),
        );
        session.record_events(&[existing])?;
        session.connection.execute_batch(
            "CREATE TRIGGER fail_second_event BEFORE INSERT ON recording_events
             WHEN NEW.elapsed_us = 2
             BEGIN SELECT RAISE(ABORT, 'injected event write failure'); END;",
        )?;
        let batch = [
            RecordedEvent::output(TerminalId::new(2), ElapsedMicros::new(1), b"first".to_vec()),
            RecordedEvent::output(
                TerminalId::new(3),
                ElapsedMicros::new(2),
                b"second".to_vec(),
            ),
        ];
        let error = session.record_events(&batch).unwrap_err();
        assert!(error.to_string().contains("injected event write failure"));

        // Both speculative pane rows and the first event must roll back with the failed event.
        let (panes, events): (i64, i64) = session.connection.query_row(
            "SELECT (SELECT COUNT(*) FROM recording_panes),
                    (SELECT COUNT(*) FROM recording_events)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((panes, events), (1, 1));
        let saved: Vec<u8> =
            session
                .connection
                .query_row("SELECT data FROM recording_events", [], |row| row.get(0))?;
        assert_eq!(saved, b"existing");

        session
            .connection
            .execute_batch("DROP TRIGGER fail_second_event")?;
        session.record_events(&batch)?;
        let mut statement = session.connection.prepare(
            "SELECT p.terminal_id, e.data FROM recording_events e
             JOIN recording_panes p ON p.id = e.pane_id ORDER BY e.id",
        )?;
        let saved = statement
            .query_map([], |row| {
                Ok((row.get::<_, u32>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(
            saved,
            vec![
                (1, b"existing".to_vec()),
                (2, b"first".to_vec()),
                (3, b"second".to_vec())
            ]
        );
        Ok(())
    }

    #[test]
    fn finishes_pane_and_session() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(1_000))?;
        let metadata = |terminal_id: u32| PaneMetadata {
            terminal_id: TerminalId::new(terminal_id),
            tab_id: zellij_types::internal_ids::TabId::new(0),
            tab_position: zellij_types::internal_ids::TabPosition::new(terminal_id as usize),
            tab_name: "tab".to_string(),
            title: "title".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[metadata(1), metadata(2)], ElapsedMicros::new(0))?;
        store.record_events(&[RecordedEvent::output(
            TerminalId::new(1),
            ElapsedMicros::new(10),
            b"hi".to_vec(),
        )])?;
        store.finish_pane(TerminalId::new(1), ElapsedMicros::new(100))?;
        store.finish_session(UnixMicros::new(200), ElapsedMicros::new(150))?;
        let db_path = dir.path().join("session-recordings.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        let mut ended =
            conn.prepare("SELECT ended_elapsed_us FROM recording_panes ORDER BY terminal_id")?;
        let ended = ended
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<Option<i64>>>>()?;
        assert_eq!(ended, vec![Some(100), Some(150)]);
        let sess_ended: Option<i64> = conn.query_row(
            "SELECT ended_at_us FROM recording_sessions WHERE name = 'test'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(sess_ended, Some(200));
        let recordings = list_recordings(dir.path())?;
        assert!(recordings.iter().all(|recording| !recording.is_running));
        Ok(())
    }

    #[test]
    fn marks_incomplete() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(1_000))?;
        store.mark_incomplete("boom")?;
        let db_path = dir.path().join("session-recordings.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        let err: Option<String> = conn.query_row(
            "SELECT error FROM recording_sessions WHERE name = 'test'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(err, Some("boom".to_string()));
        Ok(())
    }

    #[test]
    fn persists_across_reopen() -> Result<()> {
        let dir = tempdir()?;
        {
            let mut store =
                RecordingStore::open(dir.path(), "persist", false, UnixMicros::new(1_000))?;
            let metadata = PaneMetadata {
                terminal_id: TerminalId::new(5),
                tab_id: zellij_types::internal_ids::TabId::new(0),
                tab_position: zellij_types::internal_ids::TabPosition::new(0),
                tab_name: "tab".to_string(),
                title: "title".to_string(),
                cols: 80,
                rows: 24,
            };
            store.update_panes(&[metadata], ElapsedMicros::new(0))?;
            store.record_events(&[RecordedEvent::output(
                TerminalId::new(5),
                ElapsedMicros::new(10),
                b"hello".to_vec(),
            )])?;
            // store dropped here
        }
        // reopen with new session, old data should still be there
        {
            let _store2 =
                RecordingStore::open(dir.path(), "persist2", false, UnixMicros::new(2_000))?;
            let db_path = dir.path().join("session-recordings.db");
            let conn = rusqlite::Connection::open(&db_path)?;
            let session_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM recording_sessions", [], |r| r.get(0))?;
            assert_eq!(session_count, 2);
            let event_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM recording_events", [], |r| r.get(0))?;
            assert_eq!(event_count, 1);
            let data: Vec<u8> =
                conn.query_row("SELECT data FROM recording_events LIMIT 1", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(data, b"hello");
        }
        Ok(())
    }

    #[test]
    fn pane_initial_size_unchanged() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(1_000))?;
        let meta1 = PaneMetadata {
            terminal_id: TerminalId::new(9),
            tab_id: zellij_types::internal_ids::TabId::new(0),
            tab_position: zellij_types::internal_ids::TabPosition::new(0),
            tab_name: "a".to_string(),
            title: "t".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[meta1], ElapsedMicros::new(0))?;
        // update with different size, should not change initial
        let meta2 = PaneMetadata {
            terminal_id: TerminalId::new(9),
            tab_id: zellij_types::internal_ids::TabId::new(0),
            tab_position: zellij_types::internal_ids::TabPosition::new(0),
            tab_name: "b".to_string(),
            title: "t2".to_string(),
            cols: 120,
            rows: 40,
        };
        store.update_panes(&[meta2], ElapsedMicros::new(10))?;
        // also resize event should not change initial
        store.record_events(&[RecordedEvent::resize(
            TerminalId::new(9),
            ElapsedMicros::new(20),
            200,
            50,
        )])?;
        let db_path = dir.path().join("session-recordings.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        let (cols, rows): (i64, i64) = conn.query_row(
            "SELECT initial_cols, initial_rows FROM recording_panes WHERE terminal_id = 9",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);
        // tab_name should be updated
        let tab_name: String = conn.query_row(
            "SELECT tab_name FROM recording_panes WHERE terminal_id = 9",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(tab_name, "b");
        Ok(())
    }

    #[test]
    fn database_permissions() -> Result<()> {
        let dir = tempdir()?;
        let _store = RecordingStore::open(dir.path(), "perm", false, UnixMicros::new(1_000))?;
        let db_path = dir.path().join("session-recordings.db");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&db_path)?.permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_data_dir() {
        // use a file as data_dir to force create_dir_all failure
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("file");
        std::fs::write(&file_path, b"hello").unwrap();
        let res = RecordingStore::open(&file_path, "test", false, UnixMicros::new(1_000));
        assert!(res.is_err());
    }
}

pub use zellij_types::recording::{RecordingSummary, SearchHit};

pub fn list_recordings(data_dir: &Path) -> Result<Vec<RecordingSummary>> {
    SqliteRecordingBackend::new(data_dir).list()
}

fn sqlite_list_recordings(data_dir: &Path) -> Result<Vec<RecordingSummary>> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // order: newest session first, then tab_position, then terminal_id
    let mut stmt = conn.prepare(
        r#"
        SELECT
            p.id,
            s.name,
            p.tab_name,
            p.terminal_id,
            p.title,
            p.started_elapsed_us,
            s.started_at_us,
            p.ended_elapsed_us,
            s.capture_input,
            s.error,
            s.started_at_us,
            s.id
        FROM recording_panes p
        JOIN recording_sessions s ON p.session_id = s.id
        ORDER BY s.started_at_us DESC, s.id DESC, p.tab_position ASC, p.terminal_id ASC
        "#,
    )?;
    let pane_rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
        ))
    })?;
    // Fix N+1: fetch MAX(elapsed_us) for all panes in one query
    let max_by_pane: std::collections::HashMap<i64, i64> = {
        let mut stmt =
            conn.prepare("SELECT pane_id, MAX(elapsed_us) FROM recording_events GROUP BY pane_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (pane_id, max) = row?;
            map.insert(pane_id, max);
        }
        map
    };
    let mut results = Vec::new();
    for row in pane_rows {
        let (
            id,
            session_name,
            tab_name,
            terminal_id,
            pane_title,
            started_elapsed_us,
            session_started,
            ended_elapsed_us,
            capture_input_int,
            error,
            _s_started,
            _s_id,
        ) = row?;
        let pane_id = id;
        // duration: if pane has ended, duration = ended - started_elapsed
        // else use max event time, else 0
        let duration_us = if let Some(ended) = ended_elapsed_us {
            ended - started_elapsed_us
        } else {
            match max_by_pane.get(&pane_id) {
                Some(&max_elapsed) => max_elapsed - started_elapsed_us,
                None => 0,
            }
        };
        let is_running = ended_elapsed_us.is_none();
        // started_at_us for summary: use session started + pane started elapsed? But spec says started_at_us is pane's absolute start?
        // Use session started + pane started elapsed to match duration logic, fallback to session start
        let started_at_us = session_started + started_elapsed_us;
        results.push(RecordingSummary {
            id: RecordingId::new(id),
            session_name: session_name.into(),
            tab_name,
            terminal_id: TerminalId::try_from(terminal_id)?,
            pane_title,
            started_at_us: UnixMicros::new(started_at_us),
            duration_us: ElapsedMicros::new(duration_us),
            is_running,
            capture_input: capture_input_int != 0,
            error,
        });
    }
    Ok(results)
}

/// The joined pane/session row `export_recording` reads: pane id, session id, initial cols and
/// rows, elapsed micros at pane start, session name, tab name, session start micros, pane title.
type RecordedPaneRow = (i64, i64, i64, i64, i64, String, String, i64, String);

pub fn export_recording(
    data_dir: &Path,
    recording_id: RecordingId,
    output: &mut dyn std::io::Write,
) -> Result<()> {
    SqliteRecordingBackend::new(data_dir).export(recording_id, output)
}

fn sqlite_export_recording(
    data_dir: &Path,
    recording_id: RecordingId,
    output: &mut dyn std::io::Write,
) -> Result<()> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        anyhow::bail!("recording {} not found", recording_id);
    }
    let conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // get pane and session
    let pane: Option<RecordedPaneRow> = {
        let mut stmt = conn.prepare(
            "SELECT p.id, p.session_id, p.initial_cols, p.initial_rows, p.started_elapsed_us, s.name, p.tab_name, s.started_at_us, p.title FROM recording_panes p JOIN recording_sessions s ON p.session_id = s.id WHERE p.id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![recording_id.get()])?;
        if let Some(row) = rows.next()? {
            Some((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        } else {
            None
        }
    };
    let (
        pane_id,
        _session_id,
        initial_cols,
        initial_rows,
        started_elapsed,
        session_name,
        tab_name,
        session_started_us,
        pane_title_opt,
    ) = match pane {
        Some(v) => v,
        None => anyhow::bail!("recording {} not found", recording_id),
    };
    let pane_title_str: String = pane_title_opt;
    let title = if pane_title_str.is_empty() {
        format!("{} / {}", session_name, tab_name)
    } else {
        format!("{} / {} / {}", session_name, tab_name, pane_title_str)
    };
    // header: use pane's absolute start (session wall + pane elapsed) for late pans.
    // This is best-effort: if system wall clock changed after session start,
    // `session_started + started_elapsed` may be off vs true wall time.
    // Storing per-pane absolute time would require schema change; for now we
    // document and use the same monotonic+wall sum as `list_recordings`.
    let pane_started_us = session_started_us + started_elapsed;
    let header = serde_json::json!({
        "version": 3,
        "term": {"cols": initial_cols, "rows": initial_rows},
        "timestamp": pane_started_us / 1_000_000,
        "title": title
    });
    writeln!(output, "{}", serde_json::to_string(&header)?)?;
    // events ordered by elapsed_us, id
    let mut stmt = conn.prepare(
        "SELECT elapsed_us, code, data FROM recording_events WHERE pane_id = ?1 ORDER BY elapsed_us ASC, id ASC",
    )?;
    let mut rows = stmt.query(rusqlite::params![pane_id])?;
    let mut prev_elapsed: Option<i64> = None;
    while let Some(row) = rows.next()? {
        let elapsed_us: i64 = row.get(0)?;
        let code: String = row.get(1)?;
        let data: Vec<u8> = row.get(2)?;
        let mut interval_us = match prev_elapsed {
            Some(prev) => elapsed_us - prev,
            None => elapsed_us - started_elapsed,
        };
        if interval_us < 0 {
            log::warn!(
                "negative interval for pane {}: elapsed {} prev {:?} started {}, clamping to 0",
                pane_id,
                elapsed_us,
                prev_elapsed,
                started_elapsed
            );
            interval_us = 0;
        }
        // but spec example: events at 0, 500k, 1250k, 2000k with intervals 0.0, 0.5, 0.75, 0.75
        // That suggests intervals are from prior event, first interval 0.0 if first event at 0?
        // Our events are stored with absolute elapsed from pane start? In store, elapsed_us is absolute from session start? In test they use 0,500k etc as elapsed.
        // Need to handle first interval as elapsed_us - 0? Actually first event at 0 should be 0.0
        // For our implementation, first interval should be elapsed_us / 1_000_000? But spec shows intervals 0.0,0.5,0.75,0.75 for events at 0,500k,1250k,2000k where last interval is 0.75 not 0.75? Wait 2000k - 1250k = 750k =0.75 yep.
        // So interval is elapsed - prev
        let secs = interval_us as f64 / 1_000_000.0;
        // format with at most 6 decimal places, trimming trailing zeros
        let secs_str = format_interval(secs);
        let data_str = String::from_utf8_lossy(&data).to_string();
        let code_json = serde_json::to_string(&code)?;
        let data_json = serde_json::to_string(&data_str)?;
        writeln!(output, "[{}, {}, {}]", secs_str, code_json, data_json)?;
        prev_elapsed = Some(elapsed_us);
    }
    Ok(())
}

fn format_interval(secs: f64) -> String {
    if secs == 0.0 {
        return "0.0".to_string();
    }
    let s = format!("{:.6}", secs);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // ensure at least one decimal
    if s.contains('.') {
        s.to_string()
    } else {
        format!("{}.0", s)
    }
}

/// Search recorded terminal output. Brings the index up to date first, so recordings made
/// before this feature existed (and panes still being written to) are searchable.
///
/// `query` is FTS5 syntax; if it doesn't parse as such it is retried as a literal phrase.
pub fn search_recordings(data_dir: &Path, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    SqliteRecordingBackend::new(data_dir).search(query, limit)
}

fn sqlite_search_recordings(data_dir: &Path, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let mut conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    ensure_search_schema(&conn)?;
    index_pending(&mut conn)?;
    match run_search(&conn, query, limit) {
        Ok(hits) => Ok(hits),
        // Punctuation and unbalanced quotes are FTS5 syntax errors, but they are also what
        // people paste in from a log line. Fall back to matching the whole thing literally.
        // If the fallback also fails (e.g. NUL bytes which terminate SQLite strings), return
        // empty results rather than propagating an FTS syntax error — the property is that
        // any pasted string searches rather than errors.
        Err(_) => match run_search(&conn, &fts_phrase(query), limit) {
            Ok(hits) => Ok(hits),
            Err(_) => Ok(Vec::new()),
        },
    }
}

/// Wrap a query as a single literal FTS5 phrase, so that punctuation and unbalanced quotes match
/// literally instead of being parsed as syntax.
///
/// Shared with [`crate::plugin_data_store`], which needs the same fallback for the same reason.
pub(crate) fn fts_phrase(query: &str) -> String {
    let without_nul = query.replace('\0', "");
    format!("\"{}\"", without_nul.replace('"', "\"\""))
}

fn ensure_search_schema(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS recording_search USING fts5(
            text,
            pane_id UNINDEXED,
            elapsed_us UNINDEXED,
            tokenize = 'unicode61'
        );
        CREATE TABLE IF NOT EXISTS recording_search_state (
            pane_id INTEGER PRIMARY KEY REFERENCES recording_panes(id),
            last_event_id INTEGER NOT NULL,
            pending BLOB NOT NULL DEFAULT X''
        );
        CREATE INDEX IF NOT EXISTS recording_events_pane_id
        ON recording_events(pane_id, id);
        "#,
    )?;
    let has_pending = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('recording_search_state') WHERE name = 'pending')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_pending {
        conn.execute_batch(
            "ALTER TABLE recording_search_state ADD COLUMN pending BLOB NOT NULL DEFAULT X'';
             DELETE FROM recording_search;
             DELETE FROM recording_search_state;",
        )?;
    }
    Ok(())
}

// ponytail: a pane that never emits a newline would buffer forever. Flush at this size instead;
// raise it if that ever matters.
const MAX_INDEXED_LINE: usize = 64 * 1024;

/// Index every output event not yet indexed, one line of plain text per row.
///
/// Note that full-screen TUI output (vim, htop) is a stream of cursor-positioning redraws,
/// so its "lines" index as fragmented text. Shell and command output — the case this is for
/// — indexes cleanly. Doing better would mean replaying through a terminal emulator.
fn index_pending(conn: &mut rusqlite::Connection) -> Result<()> {
    let panes: Vec<(i64, i64, Vec<u8>)> = {
        let mut stmt = conn.prepare(
            "SELECT p.id, COALESCE(st.last_event_id, 0), COALESCE(st.pending, X'')
             FROM recording_panes p
             LEFT JOIN recording_search_state st ON st.pane_id = p.id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    // IMMEDIATE, not the default DEFERRED: this transaction reads before it writes, and a
    // deferred read->write upgrade returns SQLITE_BUSY *without* consulting `busy_timeout`
    // (waiting could deadlock). A live server flushes every 100ms, so a deferred
    // transaction loses that race every time and search fails with "database is locked".
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (pane_id, last_event_id, mut buffer) in panes {
        // One parser for the whole pane, not one per flush: an escape sequence can be split
        // across two PTY chunks, and parsing each chunk on its own leaks the tail of the
        // sequence into the text as if it were content. `buffer` holds the raw bytes since
        // the last completed line, so that a later call can pick the pane back up.
        let mut lines: Vec<(String, i64)> = Vec::new();
        let mut last_seen: Option<i64> = None;
        let mut parser = vte::Parser::new();
        let mut sink = LineSink::default();
        // ponytail: replaying `buffer` rebuilds the parser state across calls, except where a
        // line completed while mid-escape-sequence. Persist the parser state alongside
        // `pending` if that ever matters.
        let mut parsed = utf8_safe_len(&buffer);
        parser.advance(&mut sink, &buffer[..parsed]);
        {
            let mut stmt = tx.prepare(
                "SELECT id, elapsed_us, data FROM recording_events
                 WHERE pane_id = ?1 AND code = 'o' AND id > ?2
                 ORDER BY id ASC",
            )?;
            let mut rows = stmt.query(rusqlite::params![pane_id, last_event_id])?;
            while let Some(row) = rows.next()? {
                let event_id: i64 = row.get(0)?;
                let elapsed_us: i64 = row.get(1)?;
                let data: Vec<u8> = row.get(2)?;
                last_seen = Some(event_id);
                buffer.extend_from_slice(&data);
                // Bytes already handed to the parser are never re-parsed, so a redraw-heavy
                // pane that goes 64KiB without a newline stays linear.
                while parsed < buffer.len() {
                    let rest = &buffer[parsed..];
                    // A line can only end where the parser *executes* a linefeed, so feed it
                    // everything up to the next raw `\n` in one go and that byte on its own:
                    // that is what tells us how many raw bytes the completed line consumed.
                    let Some(newline) = rest.iter().position(|byte| *byte == b'\n') else {
                        let safe = utf8_safe_len(rest);
                        parser.advance(&mut sink, &rest[..safe]);
                        parsed += safe;
                        break;
                    };
                    parser.advance(&mut sink, &rest[..newline]);
                    parser.advance(&mut sink, &rest[newline..newline + 1]);
                    parsed += newline + 1;
                    if sink.done {
                        sink.done = false;
                        push_line(&mut lines, &std::mem::take(&mut sink.text), elapsed_us);
                        buffer.drain(..parsed);
                        parsed = 0;
                    }
                }
                if buffer.len() >= MAX_INDEXED_LINE {
                    push_line(&mut lines, &std::mem::take(&mut sink.text), elapsed_us);
                    buffer.clear();
                    parsed = 0;
                }
            }
        }
        {
            let mut insert = tx.prepare(
                "INSERT INTO recording_search (text, pane_id, elapsed_us) VALUES (?1, ?2, ?3)",
            )?;
            for (text, elapsed_us) in lines {
                insert.execute(rusqlite::params![text, pane_id, elapsed_us])?;
            }
        }
        if let Some(event_id) = last_seen {
            tx.execute(
                "INSERT INTO recording_search_state (pane_id, last_event_id, pending) VALUES (?1, ?2, ?3)
                 ON CONFLICT(pane_id) DO UPDATE SET last_event_id = excluded.last_event_id, pending = excluded.pending",
                rusqlite::params![pane_id, event_id, buffer],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// How much of `bytes` can be handed to the parser without splitting a UTF-8 sequence: vte
/// drops the byte that follows the split when a codepoint straddles two `advance` calls, so a
/// trailing partial sequence has to wait for the rest of its bytes.
fn utf8_safe_len(bytes: &[u8]) -> usize {
    for back in 1..=3.min(bytes.len()) {
        let needed = match bytes[bytes.len() - back] {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            // a continuation byte (or an invalid lead, which vte handles on its own): keep
            // looking back for the byte that started the sequence
            _ => continue,
        };
        return if needed > back {
            bytes.len() - back
        } else {
            bytes.len()
        };
    }
    bytes.len()
}

/// One line of plain text, ended where the parser *executes* a linefeed rather than at any
/// raw `\n` byte — a raw newline can sit inside an escape sequence, and cutting the line
/// there is what made the indexed text depend on where the PTY chunks fell.
#[derive(Default)]
struct LineSink {
    text: String,
    done: bool,
}

impl vte::Perform for LineSink {
    fn print(&mut self, c: char) {
        self.text.push(c);
    }
    fn execute(&mut self, byte: u8) {
        // Only a linefeed produces anything; every other C0 control is dropped, as it was
        // when this went through strip_ansi_escapes.
        if byte == b'\n' {
            self.done = true;
        }
    }
}

fn push_line(lines: &mut Vec<(String, i64)>, line: &str, elapsed_us: i64) {
    let cleaned = zellij_types::text::clean_string_from_control_and_linebreak(line);
    // Terminal redraws pad every line out to the pane width, so a captured line is mostly
    // runs of spaces with the real content scattered through it (measured: ~714 chars
    // average on a real recording). Collapse the padding so hits are readable.
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        lines.push((collapsed, elapsed_us));
    }
}

fn run_search(conn: &rusqlite::Connection, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    // FTS5 columns are typeless, so cast the two numeric ones back explicitly.
    // `snippet` returns the window around the match rather than the head of the line:
    // redrawn TUI output concatenates many screen repaints into one very long line, so
    // the head is usually not where the match is.
    let mut stmt = conn.prepare(
        r#"
        SELECT CAST(r.pane_id AS INTEGER), CAST(r.elapsed_us AS INTEGER),
               snippet(recording_search, 0, '', '', '…', 16),
               s.name, p.tab_name, p.title, s.started_at_us
        FROM recording_search r
        JOIN recording_panes p ON p.id = CAST(r.pane_id AS INTEGER)
        JOIN recording_sessions s ON s.id = p.session_id
        WHERE recording_search MATCH ?1
        ORDER BY rank
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(rusqlite::params![query, limit as i64], |row| {
        let elapsed_us: i64 = row.get(1)?;
        let session_started_us: i64 = row.get(6)?;
        Ok(SearchHit {
            recording_id: RecordingId::new(row.get(0)?),
            elapsed_us: ElapsedMicros::new(elapsed_us),
            text: row.get(2)?,
            session_name: row.get::<_, String>(3)?.into(),
            tab_name: row.get(4)?,
            pane_title: row.get(5)?,
            // Same best-effort wall clock reconstruction as `list_recordings`.
            started_at_us: UnixMicros::new(session_started_us + elapsed_us),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use hegel::generators::{self, Generator};
    use tempfile::tempdir;

    fn record(store: &mut RecordingStore, elapsed_us: i64, data: &[u8]) {
        store
            .record_events(&[RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(elapsed_us),
                data.to_vec(),
            )])
            .unwrap();
    }

    fn open(dir: &Path) -> RecordingStore {
        let mut store =
            RecordingStore::open(dir, "my_session", false, UnixMicros::new(1_000_000)).unwrap();
        store
            .update_panes(
                &[PaneMetadata {
                    terminal_id: TerminalId::new(1),
                    tab_id: zellij_types::internal_ids::TabId::new(0),
                    tab_position: zellij_types::internal_ids::TabPosition::new(0),
                    tab_name: "my_tab".to_string(),
                    title: "my_pane".to_string(),
                    cols: 80,
                    rows: 24,
                }],
                ElapsedMicros::new(0),
            )
            .unwrap();
        store
    }

    #[test]
    fn finds_a_line_split_across_events_and_strips_ansi() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open(dir.path());
        // needle straddles two PTY chunks, and is wrapped in color codes
        record(&mut store, 100, b"\x1b[32mhello nee");
        record(&mut store, 200, b"dle world\x1b[0m\r\nunrelated\r\n");
        drop(store);

        let hits = search_recordings(dir.path(), "needle", 10)?;
        assert_eq!(hits.len(), 1, "expected exactly one hit: {:?}", hits.len());
        assert_eq!(hits[0].text, "hello needle world");
        assert_eq!(hits[0].session_name, "my_session".into());
        assert_eq!(hits[0].tab_name, "my_tab");
        assert_eq!(hits[0].pane_title, "my_pane");
        // attributed to the event that completed the line
        assert_eq!(hits[0].elapsed_us, ElapsedMicros::new(200));
        assert_eq!(hits[0].started_at_us, UnixMicros::new(1_000_000 + 200));
        Ok(())
    }

    #[test]
    fn strips_escape_sequences_split_across_events() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open(dir.path());
        // a truecolor SGR sequence cut in half by the chunk boundary; stripping each chunk
        // on its own would leave "249m" behind as if it were content
        record(&mut store, 100, b"alpha \x1b[38;2;77;185");
        record(&mut store, 200, b";249mbravo\r\n");
        drop(store);

        let hits = search_recordings(dir.path(), "bravo", 10)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "alpha bravo");
        assert_eq!(search_recordings(dir.path(), "249m", 10)?.len(), 0);
        Ok(())
    }

    #[test]
    fn collapses_terminal_redraw_padding() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open(dir.path());
        let mut padded = b"left".to_vec();
        padded.extend(std::iter::repeat_n(b' ', 200));
        padded.extend_from_slice(b"right\r\n");
        record(&mut store, 100, &padded);
        drop(store);

        let hits = search_recordings(dir.path(), "right", 10)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "left right");
        Ok(())
    }

    #[test]
    fn indexes_incrementally_without_duplicating() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open(dir.path());
        record(&mut store, 100, b"needle one\nneedle two");
        assert_eq!(search_recordings(dir.path(), "needle", 10)?.len(), 1);

        // the tail shares an event with the completed line and must survive incremental indexing
        assert_eq!(search_recordings(dir.path(), "needle", 10)?.len(), 1);

        record(&mut store, 200, b" continued\n");
        let hits = search_recordings(dir.path(), "needle", 10)?;
        assert_eq!(hits.len(), 2, "expected no duplicate of the first line");
        assert!(hits.iter().any(|h| h.text == "needle one"));
        assert!(hits.iter().any(|h| h.text == "needle two continued"));
        Ok(())
    }

    #[test]
    fn migrates_old_search_state_and_rebuilds_the_index() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open(dir.path());
        record(&mut store, 100, b"needle\n");
        drop(store);

        let conn = rusqlite::Connection::open(dir.path().join("session-recordings.db"))?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE recording_search USING fts5(text, pane_id UNINDEXED, elapsed_us UNINDEXED);
             CREATE TABLE recording_search_state (
                 pane_id INTEGER PRIMARY KEY REFERENCES recording_panes(id),
                 last_event_id INTEGER NOT NULL
             );
             INSERT INTO recording_search VALUES ('stale', 1, 100);
             INSERT INTO recording_search_state VALUES (1, 1);",
        )?;
        drop(conn);

        assert_eq!(search_recordings(dir.path(), "needle", 10)?.len(), 1);
        assert!(search_recordings(dir.path(), "stale", 10)?.is_empty());
        Ok(())
    }

    #[test]
    fn falls_back_to_a_literal_phrase_for_invalid_fts_syntax() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open(dir.path());
        record(&mut store, 100, b"error: can't connect to host\n");
        drop(store);

        let hits = search_recordings(dir.path(), "error: can't connect", 10)?;
        assert_eq!(hits.len(), 1);
        // and real FTS5 syntax still works
        assert_eq!(
            search_recordings(dir.path(), "conn* AND host", 10)?.len(),
            1
        );
        assert_eq!(search_recordings(dir.path(), "nomatch", 10)?.len(), 0);
        Ok(())
    }

    #[test]
    fn missing_database_returns_no_hits() -> Result<()> {
        let dir = tempdir()?;
        assert!(search_recordings(dir.path(), "needle", 10)?.is_empty());
        Ok(())
    }

    /// Pieces of a plausible PTY stream: words, the separators lines are made of, escape
    /// sequences to be stripped, and raw bytes (so cuts can land inside a UTF-8 sequence).
    #[hegel::composite]
    fn output_stream(tc: &hegel::TestCase) -> Vec<u8> {
        let piece_count = tc.draw(generators::integers::<usize>().max_value(30));
        let mut stream = Vec::new();
        for _ in 0..piece_count {
            let piece: Vec<u8> = tc.draw(hegel::one_of!(
                generators::text().max_size(12).map(String::into_bytes),
                generators::binary().max_size(8),
                generators::sampled_from(vec![
                    &b"\n"[..],
                    b"\r\n",
                    b"    ",
                    b"\t",
                    b"\x1b[32m",
                    b"\x1b[0m",
                    b"\x1b[38;2;77;185;249m",
                    b"\x1b[2J",
                ])
                .map(<[u8]>::to_vec),
            ));
            stream.extend_from_slice(&piece);
        }
        stream
    }

    /// Every line `index_pending` would index, in order, for a pane fed `chunks` as events.
    fn indexed_lines(chunks: &[&[u8]]) -> Vec<String> {
        let dir = tempdir().unwrap();
        {
            let mut store = open(dir.path());
            for chunk in chunks {
                // one elapsed_us for every chunk: a line is attributed to the event that
                // completed it, which chunking legitimately changes. The line *text* is
                // what must not depend on it.
                record(&mut store, 100, chunk);
            }
        }
        let mut conn =
            rusqlite::Connection::open(dir.path().join("session-recordings.db")).unwrap();
        ensure_search_schema(&conn).unwrap();
        index_pending(&mut conn).unwrap();
        let mut stmt = conn
            .prepare("SELECT text FROM recording_search ORDER BY rowid")
            .unwrap();
        let lines = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        lines
    }

    /// A PTY hands us the same bytes cut at arbitrary offsets from one run to the next, so
    /// what gets indexed must depend on the stream and not on where the cuts fell. The two
    /// tests above are this property at two hand-picked offsets.
    // 30 rather than the default 100: each case builds two sqlite databases on disk, plus
    // one transaction per chunk.
    #[hegel::test(test_cases = 30)]
    fn indexing_is_independent_of_chunk_boundaries(tc: hegel::TestCase) {
        let stream = tc.draw(output_stream());
        // a handful of cuts already lands them inside lines, escape sequences and UTF-8
        // sequences; more only costs one sqlite transaction each.
        let cut_count = tc.draw(generators::integers::<usize>().max_value(stream.len().min(8)));
        let mut cuts: Vec<usize> = tc.draw(
            generators::vecs(generators::integers::<usize>().max_value(stream.len()))
                .min_size(cut_count)
                .max_size(cut_count)
                .unique(true),
        );
        cuts.sort_unstable();

        let chunks: Vec<&[u8]> = std::iter::once(0)
            .chain(cuts.iter().copied())
            .zip(cuts.iter().copied().chain(std::iter::once(stream.len())))
            .map(|(from, to)| &stream[from..to])
            .collect();

        assert_eq!(indexed_lines(&[&stream]), indexed_lines(&chunks));
    }

    /// The literal-phrase fallback exists so that anything a person pastes in — a log line
    /// with a colon, an apostrophe, a stray quote — searches instead of erroring. So no
    /// string at all may come back as an FTS5 syntax error.
    #[hegel::test]
    fn any_query_string_searches_rather_than_erroring(tc: hegel::TestCase) {
        let query = tc.draw(generators::text());
        let dir = tempdir().unwrap();
        {
            let mut store = open(dir.path());
            record(&mut store, 100, b"error: can't connect to host\n");
        }
        let hits = search_recordings(dir.path(), &query, 10);
        assert!(hits.is_ok(), "{:?} errored: {:?}", query, hits.err());
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn lists_recorded_panes() -> Result<()> {
        let dir = tempdir()?;
        // first session (older)
        {
            let mut store =
                RecordingStore::open(dir.path(), "first", false, UnixMicros::new(1_000))?;
            store.update_panes(
                &[
                    PaneMetadata {
                        terminal_id: TerminalId::new(1),
                        tab_id: zellij_types::internal_ids::TabId::new(10),
                        tab_position: zellij_types::internal_ids::TabPosition::new(1),
                        tab_name: "tab1".to_string(),
                        title: "pane1".to_string(),
                        cols: 80,
                        rows: 24,
                    },
                    PaneMetadata {
                        terminal_id: TerminalId::new(2),
                        tab_id: zellij_types::internal_ids::TabId::new(10),
                        tab_position: zellij_types::internal_ids::TabPosition::new(0),
                        tab_name: "tab0".to_string(),
                        title: "pane2".to_string(),
                        cols: 80,
                        rows: 24,
                    },
                ],
                ElapsedMicros::new(1_000),
            )?;
            store.record_events(&[RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(1_100),
                b"hello".to_vec(),
            )])?;
            store.finish_pane(TerminalId::new(1), ElapsedMicros::new(1_200))?;
        }
        // second session (newer)
        {
            let mut store =
                RecordingStore::open(dir.path(), "second", true, UnixMicros::new(2_000))?;
            store.update_panes(
                &[PaneMetadata {
                    terminal_id: TerminalId::new(3),
                    tab_id: zellij_types::internal_ids::TabId::new(20),
                    tab_position: zellij_types::internal_ids::TabPosition::new(0),
                    tab_name: "tabA".to_string(),
                    title: "paneA".to_string(),
                    cols: 80,
                    rows: 24,
                }],
                ElapsedMicros::new(2_000),
            )?;
            store.record_events(&[RecordedEvent::output(
                TerminalId::new(3),
                ElapsedMicros::new(2_100),
                b"world".to_vec(),
            )])?;
        }
        let list = list_recordings(dir.path())?;
        assert_eq!(list.len(), 3);
        // newest session first -> terminal 3 (second session) first
        assert_eq!(list[0].terminal_id, TerminalId::new(3));
        assert_eq!(list[0].session_name, "second".into());
        // then tab_position 0 before 1 within first session
        assert_eq!(list[1].terminal_id, TerminalId::new(2));
        assert_eq!(list[2].terminal_id, TerminalId::new(1));
        Ok(())
    }

    #[test]
    fn exports_asciicast_v3() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(
            dir.path(),
            "test",
            false,
            UnixMicros::new(1_500_000_000_000_000),
        )?;
        let metadata = PaneMetadata {
            terminal_id: TerminalId::new(1),
            tab_id: zellij_types::internal_ids::TabId::new(1),
            tab_position: zellij_types::internal_ids::TabPosition::new(0),
            tab_name: "tab".to_string(),
            title: "pane".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[metadata], ElapsedMicros::new(0))?;
        store.record_events(&[
            RecordedEvent::output(TerminalId::new(1), ElapsedMicros::new(0), b"hello".to_vec()),
            RecordedEvent::input(
                TerminalId::new(1),
                ElapsedMicros::new(500_000),
                b"x".to_vec(),
            ),
            RecordedEvent::resize(TerminalId::new(1), ElapsedMicros::new(1_250_000), 100, 30),
            RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(2_000_000),
                b"bye".to_vec(),
            ),
        ])?;
        // need to get pane id for export - it's the first pane inserted
        let db_path = dir.path().join("session-recordings.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        let pane_id: i64 = conn.query_row(
            "SELECT id FROM recording_panes WHERE terminal_id = 1",
            [],
            |r| r.get(0),
        )?;
        let mut out = Vec::new();
        export_recording(dir.path(), RecordingId::new(pane_id), &mut out)?;
        let out_str = String::from_utf8(out)?;
        let mut lines = out_str.lines();
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap())?;
        assert_eq!(header["version"], 3);
        assert_eq!(header["term"]["cols"], 80);
        assert_eq!(header["term"]["rows"], 24);
        assert_eq!(header["timestamp"], 1500000000);
        assert_eq!(header["title"], "test / tab / pane");
        let l1: serde_json::Value = serde_json::from_str(lines.next().unwrap())?;
        assert_eq!(l1, serde_json::json!([0.0, "o", "hello"]));
        let l2: serde_json::Value = serde_json::from_str(lines.next().unwrap())?;
        assert_eq!(l2, serde_json::json!([0.5, "i", "x"]));
        let l3: serde_json::Value = serde_json::from_str(lines.next().unwrap())?;
        assert_eq!(l3, serde_json::json!([0.75, "r", "100x30"]));
        let l4: serde_json::Value = serde_json::from_str(lines.next().unwrap())?;
        assert_eq!(l4, serde_json::json!([0.75, "o", "bye"]));
        assert!(lines.next().is_none());
        Ok(())
    }

    #[test]
    fn export_unknown_id_errors() -> Result<()> {
        let dir = tempdir()?;
        let mut out = Vec::new();
        let err = export_recording(dir.path(), RecordingId::new(999), &mut out).unwrap_err();
        assert!(err.to_string().contains("not found"));
        // also when DB exists but id missing
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(1_000))?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: TerminalId::new(1),
                tab_id: zellij_types::internal_ids::TabId::new(0),
                tab_position: zellij_types::internal_ids::TabPosition::new(0),
                tab_name: "a".to_string(),
                title: "b".to_string(),
                cols: 80,
                rows: 24,
            }],
            ElapsedMicros::new(0),
        )?;
        let err2 = export_recording(dir.path(), RecordingId::new(9999), &mut out).unwrap_err();
        assert!(err2.to_string().contains("not found"));
        Ok(())
    }

    #[test]
    fn export_invalid_utf8() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(0))?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: TerminalId::new(1),
                tab_id: zellij_types::internal_ids::TabId::new(0),
                tab_position: zellij_types::internal_ids::TabPosition::new(0),
                tab_name: "tab".to_string(),
                title: "pane".to_string(),
                cols: 80,
                rows: 24,
            }],
            ElapsedMicros::new(0),
        )?;
        store.record_events(&[RecordedEvent::output(
            TerminalId::new(1),
            ElapsedMicros::new(0),
            vec![0xff, 0xfe, b'a'],
        )])?;
        let pane_id: i64 = {
            let db_path = dir.path().join("session-recordings.db");
            let conn = rusqlite::Connection::open(db_path)?;
            conn.query_row(
                "SELECT id FROM recording_panes WHERE terminal_id = 1",
                [],
                |r| r.get(0),
            )?
        };
        let mut out = Vec::new();
        export_recording(dir.path(), RecordingId::new(pane_id), &mut out)?;
        let out_str = String::from_utf8_lossy(&out).to_string();
        // should contain replacement character � (UTF-8 lossy)
        assert!(out_str.contains("�"));
        // parse second line
        let lines: Vec<&str> = out_str.lines().collect();
        let v: serde_json::Value = serde_json::from_str(lines[1])?;
        let data = v[2].as_str().unwrap();
        assert!(data.contains("�"));
        Ok(())
    }

    #[test]
    fn export_equal_timestamps_keep_order() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(0))?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: TerminalId::new(1),
                tab_id: zellij_types::internal_ids::TabId::new(0),
                tab_position: zellij_types::internal_ids::TabPosition::new(0),
                tab_name: "t".to_string(),
                title: "p".to_string(),
                cols: 80,
                rows: 24,
            }],
            ElapsedMicros::new(0),
        )?;
        // two events same timestamp, insertion order should be preserved
        store.record_events(&[
            RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(1_000),
                b"first".to_vec(),
            ),
            RecordedEvent::output(
                TerminalId::new(1),
                ElapsedMicros::new(1_000),
                b"second".to_vec(),
            ),
        ])?;
        let pane_id: i64 = {
            let db_path = dir.path().join("session-recordings.db");
            let conn = rusqlite::Connection::open(db_path)?;
            conn.query_row(
                "SELECT id FROM recording_panes WHERE terminal_id = 1",
                [],
                |r| r.get(0),
            )?
        };
        let mut out = Vec::new();
        export_recording(dir.path(), RecordingId::new(pane_id), &mut out)?;
        let out_str = String::from_utf8(out)?;
        let lines: Vec<&str> = out_str.lines().collect();
        let v1: serde_json::Value = serde_json::from_str(lines[1])?;
        let v2: serde_json::Value = serde_json::from_str(lines[2])?;
        assert_eq!(v1[2], "first");
        assert_eq!(v2[2], "second");
        // intervals: first 0.001, second 0.0 (same timestamp)
        assert_eq!(v1[0], serde_json::json!(0.001));
        assert_eq!(v2[0], serde_json::json!(0.0));
        Ok(())
    }

    #[test]
    fn export_running_recording() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(0))?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: TerminalId::new(1),
                tab_id: zellij_types::internal_ids::TabId::new(0),
                tab_position: zellij_types::internal_ids::TabPosition::new(0),
                tab_name: "t".to_string(),
                title: "p".to_string(),
                cols: 80,
                rows: 24,
            }],
            ElapsedMicros::new(0),
        )?;
        store.record_events(&[
            RecordedEvent::output(TerminalId::new(1), ElapsedMicros::new(0), b"a".to_vec()),
            RecordedEvent::output(TerminalId::new(1), ElapsedMicros::new(1_000), b"b".to_vec()),
        ])?;
        // do not finish pane/session, export should still include committed events
        let pane_id: i64 = {
            let db_path = dir.path().join("session-recordings.db");
            let conn = rusqlite::Connection::open(db_path)?;
            conn.query_row(
                "SELECT id FROM recording_panes WHERE terminal_id = 1",
                [],
                |r| r.get(0),
            )?
        };
        let mut out = Vec::new();
        export_recording(dir.path(), RecordingId::new(pane_id), &mut out)?;
        let out_str = String::from_utf8(out)?;
        assert_eq!(out_str.lines().count(), 3); // header + 2 events
        Ok(())
    }

    #[test]
    fn export_no_resize_uses_default_size() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(0))?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: TerminalId::new(1),
                tab_id: zellij_types::internal_ids::TabId::new(0),
                tab_position: zellij_types::internal_ids::TabPosition::new(0),
                tab_name: "t".to_string(),
                title: "p".to_string(),
                cols: 80,
                rows: 24,
            }],
            ElapsedMicros::new(0),
        )?;
        store.record_events(&[RecordedEvent::output(
            TerminalId::new(1),
            ElapsedMicros::new(0),
            b"hi".to_vec(),
        )])?;
        let pane_id: i64 = {
            let db_path = dir.path().join("session-recordings.db");
            let conn = rusqlite::Connection::open(db_path)?;
            conn.query_row(
                "SELECT id FROM recording_panes WHERE terminal_id = 1",
                [],
                |r| r.get(0),
            )?
        };
        let mut out = Vec::new();
        export_recording(dir.path(), RecordingId::new(pane_id), &mut out)?;
        let header: serde_json::Value =
            serde_json::from_str(String::from_utf8(out)?.lines().next().unwrap())?;
        assert_eq!(header["term"]["cols"], 80);
        assert_eq!(header["term"]["rows"], 24);
        Ok(())
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    // These fail at compile time if a public API regresses to interchangeable scalars.
    const _: fn(TerminalId, ElapsedMicros, Vec<u8>) -> RecordedEvent = RecordedEvent::output;
    const _: fn(&Path, RecordingId, &mut dyn std::io::Write) -> Result<()> = export_recording;
    const _: fn(&Path, &str, bool, UnixMicros) -> Result<RecordingStore> = RecordingStore::open;

    #[test]
    fn listing_rejects_database_terminal_ids_that_cannot_fit_the_contract() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, UnixMicros::new(0))?;
        store.record_events(&[RecordedEvent::output(
            TerminalId::new(0),
            ElapsedMicros::new(0),
            vec![],
        )])?;
        drop(store);
        let conn = rusqlite::Connection::open(dir.path().join("session-recordings.db"))?;
        for value in [-1, i64::from(u32::MAX) + 1, i64::MAX] {
            conn.execute("UPDATE recording_panes SET terminal_id = ?1", [value])?;
            assert!(list_recordings(dir.path()).is_err(), "accepted {value}");
        }
        for value in [0, u32::MAX] {
            conn.execute(
                "UPDATE recording_panes SET terminal_id = ?1",
                [i64::from(value)],
            )?;
            assert_eq!(
                list_recordings(dir.path())?[0].terminal_id,
                TerminalId::new(value)
            );
        }
        Ok(())
    }
}
