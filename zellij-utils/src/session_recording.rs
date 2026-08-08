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
            rusqlite::params![session_name, started_at_us, if capture_input { 1 } else { 0 }],
        )?;
        let session_id = connection.last_insert_rowid();
        Ok(Self {
            connection,
            session_id,
        })
    }

    fn get_pane_id(&self, terminal_id: u32) -> Result<Option<i64>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id FROM recording_panes WHERE session_id = ?1 AND terminal_id = ?2")?;
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
                let mut rows =
                    stmt.query(rusqlite::params![self.session_id, event.terminal_id as i64])?;
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
                let mut rows =
                    stmt.query(rusqlite::params![self.session_id, pane.terminal_id as i64])?;
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
        let mut stmt = conn.prepare("SELECT code FROM recording_events ORDER BY elapsed_us, id")?;
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

pub struct RecordingSummary {
    pub id: i64,
    pub session_name: String,
    pub tab_name: String,
    pub terminal_id: u32,
    pub pane_title: String,
    pub started_at_us: i64,
    pub duration_us: i64,
    pub is_running: bool,
    pub capture_input: bool,
    pub error: Option<String>,
}

pub fn list_recordings(data_dir: &Path) -> Result<Vec<RecordingSummary>> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open(&db_path)?;
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
            let mut stmt2 =
                conn.prepare("SELECT MAX(elapsed_us) FROM recording_events WHERE pane_id = ?1")?;
            let max: Option<i64> = stmt2
                .query_row(rusqlite::params![pane_id], |r| r.get(0))
                .unwrap_or(None);
            match max {
                Some(max_elapsed) => max_elapsed - started_elapsed_us,
                None => 0,
            }
        };
        let is_running = ended_elapsed_us.is_none();
        // started_at_us for summary: use session started + pane started elapsed? But spec says started_at_us is pane's absolute start?
        // Use session started + pane started elapsed to match duration logic, fallback to session start
        let started_at_us = session_started + started_elapsed_us;
        results.push(RecordingSummary {
            id,
            session_name,
            tab_name,
            terminal_id: terminal_id as u32,
            pane_title,
            started_at_us,
            duration_us,
            is_running,
            capture_input: capture_input_int != 0,
            error,
        });
    }
    Ok(results)
}

pub fn export_recording(
    data_dir: &Path,
    recording_id: i64,
    output: &mut dyn std::io::Write,
) -> Result<()> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        anyhow::bail!("recording {} not found", recording_id);
    }
    let conn = rusqlite::Connection::open(&db_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // get pane and session
    let pane: Option<(i64, i64, i64, i64, i64, String, String, i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT p.id, p.session_id, p.initial_cols, p.initial_rows, p.started_elapsed_us, s.name, p.tab_name, s.started_at_us, p.title FROM recording_panes p JOIN recording_sessions s ON p.session_id = s.id WHERE p.id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![recording_id])?;
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
        _started_elapsed,
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
    // header
    let header = serde_json::json!({
        "version": 3,
        "term": {"cols": initial_cols, "rows": initial_rows},
        "timestamp": session_started_us / 1_000_000,
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
        let interval_us = match prev_elapsed {
            Some(prev) => elapsed_us - prev,
            None => elapsed_us,
        };
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

#[cfg(test)]
mod list_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn lists_recorded_panes() -> Result<()> {
        let dir = tempdir()?;
        // first session (older)
        {
            let mut store = RecordingStore::open(dir.path(), "first", false, 1_000)?;
            store.update_panes(
                &[
                    PaneMetadata {
                        terminal_id: 1,
                        tab_id: 10,
                        tab_position: 1,
                        tab_name: "tab1".to_string(),
                        title: "pane1".to_string(),
                        cols: 80,
                        rows: 24,
                    },
                    PaneMetadata {
                        terminal_id: 2,
                        tab_id: 10,
                        tab_position: 0,
                        tab_name: "tab0".to_string(),
                        title: "pane2".to_string(),
                        cols: 80,
                        rows: 24,
                    },
                ],
                1_000,
            )?;
            store.record_events(&[RecordedEvent::output(1, 1_100, b"hello".to_vec())])?;
            store.finish_pane(1, 1_200)?;
        }
        // second session (newer)
        {
            let mut store = RecordingStore::open(dir.path(), "second", true, 2_000)?;
            store.update_panes(
                &[PaneMetadata {
                    terminal_id: 3,
                    tab_id: 20,
                    tab_position: 0,
                    tab_name: "tabA".to_string(),
                    title: "paneA".to_string(),
                    cols: 80,
                    rows: 24,
                }],
                2_000,
            )?;
            store.record_events(&[RecordedEvent::output(3, 2_100, b"world".to_vec())])?;
        }
        let list = list_recordings(dir.path())?;
        assert_eq!(list.len(), 3);
        // newest session first -> terminal 3 (second session) first
        assert_eq!(list[0].terminal_id, 3);
        assert_eq!(list[0].session_name, "second");
        // then tab_position 0 before 1 within first session
        assert_eq!(list[1].terminal_id, 2);
        assert_eq!(list[2].terminal_id, 1);
        Ok(())
    }

    #[test]
    fn exports_asciicast_v3() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, 1_500_000_000_000_000)?;
        let metadata = PaneMetadata {
            terminal_id: 1,
            tab_id: 1,
            tab_position: 0,
            tab_name: "tab".to_string(),
            title: "pane".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[metadata], 0)?;
        store.record_events(&[
            RecordedEvent::output(1, 0, b"hello".to_vec()),
            RecordedEvent::input(1, 500_000, b"x".to_vec()),
            RecordedEvent::resize(1, 1_250_000, 100, 30),
            RecordedEvent::output(1, 2_000_000, b"bye".to_vec()),
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
        export_recording(dir.path(), pane_id, &mut out)?;
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
        let err = export_recording(dir.path(), 999, &mut out).unwrap_err();
        assert!(err.to_string().contains("not found"));
        // also when DB exists but id missing
        let mut store = RecordingStore::open(dir.path(), "test", false, 1_000)?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: 1,
                tab_id: 0,
                tab_position: 0,
                tab_name: "a".to_string(),
                title: "b".to_string(),
                cols: 80,
                rows: 24,
            }],
            0,
        )?;
        let err2 = export_recording(dir.path(), 9999, &mut out).unwrap_err();
        assert!(err2.to_string().contains("not found"));
        Ok(())
    }

    #[test]
    fn export_invalid_utf8() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, 0)?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: 1,
                tab_id: 0,
                tab_position: 0,
                tab_name: "tab".to_string(),
                title: "pane".to_string(),
                cols: 80,
                rows: 24,
            }],
            0,
        )?;
        store.record_events(&[RecordedEvent::output(1, 0, vec![0xff, 0xfe, b'a'])])?;
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
        export_recording(dir.path(), pane_id, &mut out)?;
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
        let mut store = RecordingStore::open(dir.path(), "test", false, 0)?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: 1,
                tab_id: 0,
                tab_position: 0,
                tab_name: "t".to_string(),
                title: "p".to_string(),
                cols: 80,
                rows: 24,
            }],
            0,
        )?;
        // two events same timestamp, insertion order should be preserved
        store.record_events(&[
            RecordedEvent::output(1, 1_000, b"first".to_vec()),
            RecordedEvent::output(1, 1_000, b"second".to_vec()),
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
        export_recording(dir.path(), pane_id, &mut out)?;
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
        let mut store = RecordingStore::open(dir.path(), "test", false, 0)?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: 1,
                tab_id: 0,
                tab_position: 0,
                tab_name: "t".to_string(),
                title: "p".to_string(),
                cols: 80,
                rows: 24,
            }],
            0,
        )?;
        store.record_events(&[
            RecordedEvent::output(1, 0, b"a".to_vec()),
            RecordedEvent::output(1, 1_000, b"b".to_vec()),
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
        export_recording(dir.path(), pane_id, &mut out)?;
        let out_str = String::from_utf8(out)?;
        assert_eq!(out_str.lines().count(), 3); // header + 2 events
        Ok(())
    }

    #[test]
    fn export_no_resize_uses_default_size() -> Result<()> {
        let dir = tempdir()?;
        let mut store = RecordingStore::open(dir.path(), "test", false, 0)?;
        store.update_panes(
            &[PaneMetadata {
                terminal_id: 1,
                tab_id: 0,
                tab_position: 0,
                tab_name: "t".to_string(),
                title: "p".to_string(),
                cols: 80,
                rows: 24,
            }],
            0,
        )?;
        store.record_events(&[RecordedEvent::output(1, 0, b"hi".to_vec())])?;
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
        export_recording(dir.path(), pane_id, &mut out)?;
        let header: serde_json::Value =
            serde_json::from_str(String::from_utf8(out)?.lines().next().unwrap())?;
        assert_eq!(header["term"]["cols"], 80);
        assert_eq!(header["term"]["rows"], 24);
        Ok(())
    }
}
