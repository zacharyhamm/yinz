use std::path::Path;

use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Output,
    Input,
    Resize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedEvent {
    pub terminal_id: u32,
    pub elapsed_us: i64,
    pub kind: EventKind,
    pub data: Vec<u8>,
}

impl RecordedEvent {
    pub fn output(terminal_id: u32, elapsed_us: i64, data: Vec<u8>) -> Self {
        Self {
            terminal_id,
            elapsed_us,
            kind: EventKind::Output,
            data,
        }
    }

    pub fn input(terminal_id: u32, elapsed_us: i64, data: Vec<u8>) -> Self {
        Self {
            terminal_id,
            elapsed_us,
            kind: EventKind::Input,
            data,
        }
    }

    pub fn resize(terminal_id: u32, elapsed_us: i64, cols: u16, rows: u16) -> Self {
        Self {
            terminal_id,
            elapsed_us,
            kind: EventKind::Resize,
            data: format!("{}x{}", cols, rows).into_bytes(),
        }
    }

    fn code(&self) -> &'static str {
        match self.kind {
            EventKind::Output => "o",
            EventKind::Input => "i",
            EventKind::Resize => "r",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneMetadata {
    pub terminal_id: u32,
    pub tab_id: usize,
    pub tab_position: usize,
    pub tab_name: String,
    pub title: String,
    pub cols: usize,
    pub rows: usize,
}

pub struct RecordingStore {
    connection: rusqlite::Connection,
    session_id: i64,
}

impl RecordingStore {
    pub fn open(
        data_dir: &Path,
        session_name: &str,
        capture_input: bool,
        started_at_us: i64,
    ) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| {
            anyhow::anyhow!("failed to create data dir {:?}: {}", data_dir, e)
        })?;
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
            rusqlite::params![session_name, started_at_us, if capture_input { 1 } else { 0 }],
        )?;
        let session_id = connection.last_insert_rowid();
        Ok(Self {
            connection,
            session_id,
        })
    }

    fn get_pane_id(&self, terminal_id: u32) -> Result<Option<i64>> {
        let mut stmt = self.connection.prepare(
            "SELECT id FROM recording_panes WHERE session_id = ?1 AND terminal_id = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![self.session_id, terminal_id as i64])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    fn ensure_pane_for_event(&mut self, event: &RecordedEvent) -> Result<i64> {
        if let Some(id) = self.get_pane_id(event.terminal_id)? {
            return Ok(id);
        }
        // pane does not exist, create with defaults or resize dimensions
        let (initial_cols, initial_rows) = if event.kind == EventKind::Resize {
            parse_resize(&event.data).unwrap_or((80, 24))
        } else {
            (80, 24)
        };
        self.connection.execute(
            "INSERT INTO recording_panes (session_id, terminal_id, started_elapsed_us, initial_cols, initial_rows) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![self.session_id, event.terminal_id as i64, event.elapsed_us, initial_cols as i64, initial_rows as i64],
        )?;
        Ok(self.connection.last_insert_rowid())
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
                let mut rows = stmt.query(rusqlite::params![self.session_id, event.terminal_id as i64])?;
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
                        rusqlite::params![self.session_id, event.terminal_id as i64, event.elapsed_us, initial_cols as i64, initial_rows as i64],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            tx.execute(
                "INSERT INTO recording_events (pane_id, elapsed_us, code, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![pane_id, event.elapsed_us, event.code(), event.data],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_panes(&mut self, panes: &[PaneMetadata], elapsed_us: i64) -> Result<()> {
        for pane in panes {
            let existing: Option<(i64, i64, i64)> = {
                let mut stmt = self.connection.prepare(
                    "SELECT id, initial_cols, initial_rows FROM recording_panes WHERE session_id = ?1 AND terminal_id = ?2",
                )?;
                let mut rows = stmt.query(rusqlite::params![self.session_id, pane.terminal_id as i64])?;
                if let Some(row) = rows.next()? {
                    Some((row.get(0)?, row.get(1)?, row.get(2)?))
                } else {
                    None
                }
            };
            if let Some((pane_id, _ic, _ir)) = existing {
                // update tab and title, but not initial size nor started time
                self.connection.execute(
                    "UPDATE recording_panes SET tab_id = ?1, tab_position = ?2, tab_name = ?3, title = ?4 WHERE id = ?5",
                    rusqlite::params![
                        pane.tab_id as i64,
                        pane.tab_position as i64,
                        pane.tab_name,
                        pane.title,
                        pane_id
                    ],
                )?;
            } else {
                self.connection.execute(
                    "INSERT INTO recording_panes (session_id, terminal_id, tab_id, tab_position, tab_name, title, started_elapsed_us, initial_cols, initial_rows) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        self.session_id,
                        pane.terminal_id as i64,
                        pane.tab_id as i64,
                        pane.tab_position as i64,
                        pane.tab_name,
                        pane.title,
                        elapsed_us,
                        pane.cols as i64,
                        pane.rows as i64
                    ],
                )?;
            }
        }
        Ok(())
    }

    pub fn finish_pane(&mut self, terminal_id: u32, elapsed_us: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_panes SET ended_elapsed_us = ?1 WHERE session_id = ?2 AND terminal_id = ?3 AND ended_elapsed_us IS NULL",
            rusqlite::params![elapsed_us, self.session_id, terminal_id as i64],
        )?;
        Ok(())
    }

    pub fn finish_session(&mut self, ended_at_us: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE recording_sessions SET ended_at_us = ?1 WHERE id = ?2",
            rusqlite::params![ended_at_us, self.session_id],
        )?;
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
    use tempfile::tempdir;

    #[test]
    fn stores_pane_events() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", true, 1_500_000_000_000_000)?;
        let metadata = PaneMetadata {
            terminal_id: 7,
            tab_id: 1,
            tab_position: 0,
            tab_name: "tab".to_string(),
            title: "pane".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[metadata], 5_000)?;
        store.record_events(&[
            RecordedEvent::output(7, 10_000, b"hello\r\n".to_vec()),
            RecordedEvent::input(7, 20_000, b"pwd\r".to_vec()),
            RecordedEvent::resize(7, 30_000, 120, 40),
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
        let mut stmt = conn.prepare(
            "SELECT code FROM recording_events ORDER BY elapsed_us, id",
        )?;
        let codes: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(codes, vec!["o", "i", "r"]);
        Ok(())
    }

    #[test]
    fn finishes_pane_and_session() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, 1_000)?;
        let metadata = PaneMetadata {
            terminal_id: 1,
            tab_id: 0,
            tab_position: 0,
            tab_name: "tab".to_string(),
            title: "title".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[metadata], 0)?;
        store.record_events(&[RecordedEvent::output(1, 10, b"hi".to_vec())])?;
        store.finish_pane(1, 100)?;
        store.finish_session(200)?;
        let db_path = dir.path().join("session-recordings.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        let ended: Option<i64> = conn.query_row(
            "SELECT ended_elapsed_us FROM recording_panes WHERE terminal_id = 1",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(ended, Some(100));
        let sess_ended: Option<i64> = conn.query_row(
            "SELECT ended_at_us FROM recording_sessions WHERE name = 'test'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(sess_ended, Some(200));
        Ok(())
    }

    #[test]
    fn marks_incomplete() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, 1_000)?;
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
            let mut store = RecordingStore::open(dir.path(), "persist", false, 1_000)?;
            let metadata = PaneMetadata {
                terminal_id: 5,
                tab_id: 0,
                tab_position: 0,
                tab_name: "tab".to_string(),
                title: "title".to_string(),
                cols: 80,
                rows: 24,
            };
            store.update_panes(&[metadata], 0)?;
            store.record_events(&[RecordedEvent::output(5, 10, b"hello".to_vec())])?;
            // store dropped here
        }
        // reopen with new session, old data should still be there
        {
            let _store2 = RecordingStore::open(dir.path(), "persist2", false, 2_000)?;
            let db_path = dir.path().join("session-recordings.db");
            let conn = rusqlite::Connection::open(&db_path)?;
            let session_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM recording_sessions", [], |r| r.get(0))?;
            assert_eq!(session_count, 2);
            let event_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM recording_events", [], |r| r.get(0))?;
            assert_eq!(event_count, 1);
            let data: Vec<u8> = conn.query_row(
                "SELECT data FROM recording_events LIMIT 1",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(data, b"hello");
        }
        Ok(())
    }

    #[test]
    fn pane_initial_size_unchanged() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, 1_000)?;
        let meta1 = PaneMetadata {
            terminal_id: 9,
            tab_id: 0,
            tab_position: 0,
            tab_name: "a".to_string(),
            title: "t".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[meta1], 0)?;
        // update with different size, should not change initial
        let meta2 = PaneMetadata {
            terminal_id: 9,
            tab_id: 0,
            tab_position: 0,
            tab_name: "b".to_string(),
            title: "t2".to_string(),
            cols: 120,
            rows: 40,
        };
        store.update_panes(&[meta2], 10)?;
        // also resize event should not change initial
        store.record_events(&[RecordedEvent::resize(9, 20, 200, 50)])?;
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
        let _store = RecordingStore::open(dir.path(), "perm", false, 1_000)?;
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
        let res = RecordingStore::open(&file_path, "test", false, 1_000);
        assert!(res.is_err());
    }
}
