use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use yinz_types::internal_ids::{RecordingId, TerminalId};
use yinz_types::units::{ElapsedMicros, UnixMicros};

use anyhow::{bail, ensure, Context, Result};
use rusqlite::TransactionBehavior;

use crate::data_access::{RecordingBackend, RecordingCompactor, RecordingSession};
use crate::recording_segment::{self, StoredEvent};

pub use yinz_types::recording::{EventKind, PaneMetadata, RecordedEvent};

struct SqliteRecordingSession {
    connection: rusqlite::Connection,
    db_path: PathBuf,
    session_id: i64,
}

/// Compacts on a connection of its own, so a pass never holds the recorder's.
struct SqliteRecordingCompactor {
    connection: rusqlite::Connection,
    session_id: i64,
    /// Carried from one pass to the next; see [`compact_store`].
    short_blocks: HashMap<i64, i64>,
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
        ensure_segment_schema(&connection)?;
        connection.execute(
            "INSERT INTO recording_sessions (name, started_at_us, capture_input) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_name, started_at_us.get(), if capture_input { 1 } else { 0 }],
        )?;
        let session_id = connection.last_insert_rowid();
        Ok(Self {
            connection,
            db_path,
            session_id,
        })
    }

    pub fn record_events(&mut self, events: &[RecordedEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        // IMMEDIATE for the same reason as `index_pending`: this reads before it writes, and the
        // compactor's connection, or another server's, may hold the write lock meanwhile. A
        // deferred upgrade would fail at once and lose the batch rather than wait its turn.
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        // IMMEDIATE for the same reason as `record_events`.
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
                    let has_resize: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM recording_events WHERE pane_id = ?1 AND code = 'r')
                             OR EXISTS(SELECT 1 FROM recording_segments WHERE pane_id = ?1 AND has_resize)",
                        rusqlite::params![pane_id],
                        |row| row.get(0),
                    )?;
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

    fn compactor(&self) -> Result<Option<Box<dyn RecordingCompactor>>> {
        let connection = connect(&self.db_path)?;
        connection
            .busy_timeout(COMPACTOR_BUSY_TIMEOUT)
            .context("failed to set the recording compactor's busy timeout")?;
        Ok(Some(Box::new(SqliteRecordingCompactor {
            connection,
            session_id: self.session_id,
            short_blocks: HashMap::new(),
        })))
    }
}

impl RecordingCompactor for SqliteRecordingCompactor {
    fn compact(
        &mut self,
        elapsed: ElapsedMicros,
        budget_bytes: u64,
        stop: &AtomicBool,
    ) -> Result<CompactionReport> {
        let live = LiveSession {
            id: self.session_id,
            elapsed_us: elapsed.get(),
        };
        compact_store(
            &mut self.connection,
            Some(live),
            budget_bytes,
            SEGMENT_BYTES,
            stop,
            &mut self.short_blocks,
        )
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

    pub fn compact(
        &mut self,
        elapsed: ElapsedMicros,
        budget_bytes: u64,
    ) -> Result<CompactionReport> {
        match self.session.compactor()? {
            Some(mut compactor) => {
                compactor.compact(elapsed, budget_bytes, &AtomicBool::new(false))
            },
            None => Ok(CompactionReport::default()),
        }
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

    fn compact_all(&self) -> Result<StoreCompaction> {
        sqlite_compact_all(&self.data_dir)
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

/// How long an event stays an uncompressed row. Events reach the store within a flush (100 ms)
/// of happening, so nothing older than this can still be on its way. That is what lets a
/// pane's segments hold its earliest events and its rows the rest, with no event of one
/// belonging among the other.
///
/// "Older" is measured on the clock the session stamps its events with, a monotonic one that
/// stops while the machine sleeps, so the wall clock never enters into it. The recording a pass
/// runs for knows that clock's reading now. Any other running session is known only by its
/// newest event, which its clock has reached at least, so its last ten minutes before that stay
/// rows; a session whose server died keeps them for good. An ended session gets nothing more.
const COMPACT_AFTER_US: i64 = 10 * 60 * 1_000_000;

/// How long a compactor waits on another connection's write before giving the pass up, which
/// loses nothing: the next pass tries again. Shorter than a recorder's wait, since a recorder
/// that gives up drops events, and short so that a session ending mid-pass waits no longer.
const COMPACTOR_BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// Event bytes per segment. At this size deflate reaches its ratio on terminal output (5.2× on
/// 1 MiB blocks, 3.5× on 4 KiB ones), and a reader inflates no more than this at a time.
const SEGMENT_BYTES: usize = 1024 * 1024;

/// What a compaction pass moved out of `recording_events`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompactionReport {
    pub segments: u64,
    pub events: u64,
    /// The events' data, before compression.
    pub raw_bytes: u64,
    /// The segments written for them.
    pub stored_bytes: u64,
}

impl CompactionReport {
    fn add(&mut self, other: CompactionReport) {
        self.segments += other.segments;
        self.events += other.events;
        self.raw_bytes += other.raw_bytes;
        self.stored_bytes += other.stored_bytes;
    }
}

/// A whole-store compaction followed by `VACUUM`, as `yinz recordings compact` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreCompaction {
    pub compaction: CompactionReport,
    /// The database file and its write-ahead log, before and after.
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// The recording a compaction pass runs for, and where its clock is now.
#[derive(Clone, Copy, Debug)]
struct LiveSession {
    id: i64,
    elapsed_us: i64,
}

/// Opens an existing store the way readers and compactors do, which leaves creating the rest of
/// the schema to the recorder.
fn connect(db_path: &Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(db_path)
        .with_context(|| format!("failed to open recording database {}", db_path.display()))?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .context("failed to set the recording database's busy timeout")?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("failed to enable foreign keys on the recording database")?;
    ensure_segment_schema(&conn)?;
    Ok(conn)
}

/// Created by readers as well as writers, so that a database from before segments existed reads
/// as one with none.
fn ensure_segment_schema(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS recording_segments (
            id INTEGER PRIMARY KEY,
            pane_id INTEGER NOT NULL REFERENCES recording_panes(id),
            first_elapsed_us INTEGER NOT NULL,
            last_elapsed_us INTEGER NOT NULL,
            min_event_id INTEGER NOT NULL,
            max_event_id INTEGER NOT NULL,
            event_count INTEGER NOT NULL,
            has_resize INTEGER NOT NULL,
            codec TEXT NOT NULL,
            raw_len INTEGER NOT NULL,
            data BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS recording_segments_pane
        ON recording_segments(pane_id);
        "#,
    )?;
    Ok(())
}

/// One of a pane's segments, as [`for_each_segment`] reads it.
struct Segment {
    /// The highest id among its events, whatever their kind.
    max_event_id: i64,
    /// Its events in the `(elapsed_us, id)` order they were packed in, or why the row cannot be
    /// read: damage is this segment's alone, and each reader decides what it costs.
    events: Result<Vec<StoredEvent>>,
}

/// Calls `each` with the pane's segments holding an event whose id is above `after_id`, in the
/// order they were written. Their events all come before every event the pane still has in
/// `recording_events`, so a reader visits these first and those rows after.
fn for_each_segment(
    conn: &rusqlite::Connection,
    pane_id: i64,
    after_id: i64,
    mut each: impl FnMut(Segment) -> Result<()>,
) -> Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT id, max_event_id, codec, raw_len, data FROM recording_segments
             WHERE pane_id = ?1 AND max_event_id > ?2 ORDER BY id",
        )
        .context("failed to list recording segments")?;
    let mut rows = stmt
        .query(rusqlite::params![pane_id, after_id])
        .context("failed to list recording segments")?;
    while let Some(row) = rows.next().context("failed to list recording segments")? {
        let id: i64 = row.get(0)?;
        let events = (|| {
            let raw_len = usize::try_from(row.get::<_, i64>(3)?).context("negative length")?;
            recording_segment::decode(
                row.get_ref(2)?.as_str()?,
                row.get_ref(4)?.as_blob()?,
                raw_len,
            )
        })()
        .with_context(|| format!("failed to read recording segment {id}"));
        each(Segment {
            max_event_id: row.get(1)?,
            events,
        })?;
    }
    Ok(())
}

/// Packs events older than [`COMPACT_AFTER_US`] into segments, pane by pane and the panes of the
/// `live` session first, until about `budget` bytes of event data have moved or `stop` is set.
/// Every segment is its own transaction, so a recorder writing to the same file never waits on
/// more than one, and a stop on no more than one.
///
/// `short_blocks` remembers each pane found short of a block while busy, with the cutoff it was
/// found at. Until that cutoff moves the pane stays that way — nothing ten minutes old is still
/// arriving, and nothing at or after the cutoff moves — so it is not read again. That is what
/// spares a session whose server died, whose cutoff never moves, a scan every pass for good.
fn compact_store(
    conn: &mut rusqlite::Connection,
    live: Option<LiveSession>,
    budget: u64,
    segment_bytes: usize,
    stop: &AtomicBool,
    short_blocks: &mut HashMap<i64, i64>,
) -> Result<CompactionReport> {
    // Each pane with its session, whether that has ended, and the newest event the session has
    // as a row. Rows alone are enough for that: segments only ever hold a pane's earliest.
    let panes: Vec<(i64, i64, bool, Option<i64>)> = {
        let mut stmt = conn
            .prepare(
                "WITH latest AS (
                     SELECT session_id,
                            MAX((SELECT MAX(elapsed_us) FROM recording_events
                                 WHERE pane_id = recording_panes.id)) AS elapsed_us
                     FROM recording_panes GROUP BY session_id
                 )
                 SELECT p.id, s.id, s.ended_at_us IS NOT NULL, latest.elapsed_us
                 FROM recording_panes p
                 JOIN recording_sessions s ON s.id = p.session_id
                 JOIN latest ON latest.session_id = s.id
                 ORDER BY s.id IS ?1 DESC, p.id",
            )
            .context("failed to list the panes to compact")?;
        let rows = stmt.query_map(rusqlite::params![live.map(|live| live.id)], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()
            .context("failed to list the panes to compact")?
    };
    let mut report = CompactionReport::default();
    for (pane_id, session_id, ended, latest) in panes {
        let cutoff = match live {
            Some(live) if live.id == session_id => live.elapsed_us.saturating_sub(COMPACT_AFTER_US),
            _ if ended => i64::MAX,
            _ => match latest {
                Some(latest) => latest.saturating_sub(COMPACT_AFTER_US),
                None => continue,
            },
        };
        if short_blocks.get(&pane_id) == Some(&cutoff) {
            continue;
        }
        while report.raw_bytes < budget && !stop.load(Ordering::Relaxed) {
            match compact_segment(conn, pane_id, cutoff, segment_bytes)? {
                Block::Packed(segment) => report.add(segment),
                Block::Short => {
                    short_blocks.insert(pane_id, cutoff);
                    break;
                },
                Block::Nothing => break,
            }
        }
        if report.raw_bytes >= budget || stop.load(Ordering::Relaxed) {
            break;
        }
    }
    Ok(report)
}

/// What [`compact_segment`] did with a pane.
enum Block {
    Packed(CompactionReport),
    /// Its cold events come short of a block while the pane is still busy, which at this
    /// cutoff they go on doing.
    Short,
    /// Nothing to move, for any other reason.
    Nothing,
}

/// Moves the pane's next block of events older than `cutoff` into a segment, or says why there
/// is none to move yet.
///
/// A block is always the pane's earliest remaining events in `(elapsed_us, id)` order, which is
/// what keeps segments ahead of rows. It is cut at `segment_bytes`. A shorter one — everything
/// cold the pane has, up to the table's newest row — is packed only once the pane has had nothing
/// newer than `cutoff`, so a slow pane is not chopped into small blocks that compress badly every
/// time compaction runs.
fn compact_segment(
    conn: &mut rusqlite::Connection,
    pane_id: i64,
    cutoff: i64,
    segment_bytes: usize,
) -> Result<Block> {
    const QUIET: &str =
        "SELECT NOT EXISTS(SELECT 1 FROM recording_events WHERE pane_id = ?1 AND elapsed_us >= ?2)";
    // Most panes have nothing cold, and every pass visits every pane: find that out from the
    // index without taking the write lock.
    let oldest: Option<i64> = conn
        .query_row(
            "SELECT MIN(elapsed_us) FROM recording_events WHERE pane_id = ?1",
            rusqlite::params![pane_id],
            |row| row.get(0),
        )
        .with_context(|| format!("failed to find pane {pane_id}'s oldest recorded event"))?;
    if !oldest.is_some_and(|oldest| oldest < cutoff) {
        return Ok(Block::Nothing);
    }
    // A busy pane short of a block is the usual case for a live one, and for good in a session
    // whose server died: settle that without the write lock too, and without reading any data.
    let quiet: bool = conn
        .query_row(QUIET, rusqlite::params![pane_id, cutoff], |row| row.get(0))
        .with_context(|| format!("failed to find whether pane {pane_id} is quiet"))?;
    if !quiet {
        let cold_bytes: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(length(data) + 8), 0) FROM recording_events
                 WHERE pane_id = ?1 AND elapsed_us < ?2",
                rusqlite::params![pane_id, cutoff],
                |row| row.get(0),
            )
            .with_context(|| format!("failed to measure pane {pane_id}'s cold events"))?;
        if usize::try_from(cold_bytes).is_ok_and(|cold_bytes| cold_bytes < segment_bytes) {
            return Ok(Block::Short);
        }
    }
    // IMMEDIATE for the same reason as `index_pending`: this reads and then writes.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .with_context(|| format!("failed to begin compacting pane {pane_id}"))?;
    // A new row's id is one past the largest in the table. Moving that row out would let the
    // next insert reuse an id a segment already holds, and the search indexer, which resumes
    // after the last id it saw, would skip the new row. So the newest row always stays.
    let newest: Option<i64> = tx
        .query_row("SELECT MAX(id) FROM recording_events", [], |row| row.get(0))
        .context("failed to find the newest recorded event")?;
    // Again, now that nothing can change it.
    let quiet: bool = tx
        .query_row(QUIET, rusqlite::params![pane_id, cutoff], |row| row.get(0))
        .with_context(|| format!("failed to find whether pane {pane_id} is quiet"))?;
    let mut events = Vec::new();
    let mut block_bytes = 0;
    {
        let mut stmt = tx.prepare(
            "SELECT id, elapsed_us, code, data FROM recording_events
             WHERE pane_id = ?1 AND elapsed_us < ?2 ORDER BY elapsed_us, id",
        )?;
        let mut rows = stmt.query(rusqlite::params![pane_id, cutoff])?;
        while block_bytes < segment_bytes {
            let Some(row) = rows.next()? else {
                break;
            };
            let id: i64 = row.get(0)?;
            if Some(id) == newest {
                break;
            }
            let code = match row.get_ref(2)?.as_str()? {
                "o" => b'o',
                "i" => b'i',
                "r" => b'r',
                other => bail!("recording event {id} has unknown code {other:?}"),
            };
            let data: Vec<u8> = row.get(3)?;
            // Counting a few bytes of framing keeps a pane of empty events from filling one
            // block without end.
            block_bytes += data.len() + 8;
            events.push(StoredEvent {
                id,
                elapsed_us: row.get(1)?,
                code,
                data,
            });
        }
    }
    let (Some(first), Some(last)) = (events.first(), events.last()) else {
        return Ok(Block::Nothing);
    };
    // Short of a full block, the loop ran out of events it may take. Not `Short`: this can be
    // the newest row holding it back, which stops once any pane records something.
    if block_bytes < segment_bytes && !quiet {
        return Ok(Block::Nothing);
    }
    let (compressed, raw_len) = recording_segment::encode(&events)?;
    let min_id = events
        .iter()
        .map(|event| event.id)
        .min()
        .unwrap_or(first.id);
    let max_id = events.iter().map(|event| event.id).max().unwrap_or(last.id);
    tx.execute(
        "INSERT INTO recording_segments (pane_id, first_elapsed_us, last_elapsed_us, min_event_id,
             max_event_id, event_count, has_resize, codec, raw_len, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            pane_id,
            first.elapsed_us,
            last.elapsed_us,
            min_id,
            max_id,
            events.len() as i64,
            events.iter().any(|event| event.code == b'r'),
            recording_segment::DEFLATE,
            raw_len as i64,
            compressed,
        ],
    )?;
    // Exactly the rows just read: they are the pane's prefix up to `last`.
    let moved = tx.execute(
        "DELETE FROM recording_events WHERE pane_id = ?1 AND (elapsed_us, id) <= (?2, ?3)",
        rusqlite::params![pane_id, last.elapsed_us, last.id],
    )?;
    ensure!(
        moved == events.len(),
        "compacting pane {pane_id} would delete {moved} events but packed {}",
        events.len()
    );
    let report = CompactionReport {
        segments: 1,
        events: events.len() as u64,
        raw_bytes: events.iter().map(|event| event.data.len() as u64).sum(),
        stored_bytes: compressed.len() as u64,
    };
    tx.commit()
        .with_context(|| format!("failed to commit pane {pane_id}'s new segment"))?;
    Ok(Block::Packed(report))
}

/// Compacts every cold event in the store, then rewrites the file so the space the events held
/// goes back to the file system: deleting rows alone only frees pages for SQLite to reuse.
pub fn compact_recordings(data_dir: &Path) -> Result<StoreCompaction> {
    SqliteRecordingBackend::new(data_dir).compact_all()
}

fn sqlite_compact_all(data_dir: &Path) -> Result<StoreCompaction> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        return Ok(StoreCompaction::default());
    }
    let wal_path = data_dir.join("session-recordings.db-wal");
    let size = || {
        [&db_path, &wal_path]
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum::<u64>()
    };
    let bytes_before = size();
    let mut conn = connect(&db_path)?;
    let compaction = compact_store(
        &mut conn,
        None,
        u64::MAX,
        SEGMENT_BYTES,
        &AtomicBool::new(false),
        &mut HashMap::new(),
    )?;
    conn.execute_batch("VACUUM").context(
        "VACUUM needs the recording database to itself while it runs; \
         retry when no session is recording",
    )?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    drop(conn);
    Ok(StoreCompaction {
        compaction,
        bytes_before,
        bytes_after: size(),
    })
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
            tab_id: yinz_types::internal_ids::TabId::new(0),
            tab_position: yinz_types::internal_ids::TabPosition::new(0),
            tab_name: "work".to_owned(),
            title: "shell".to_owned(),
            cols: 80,
            rows: 24,
        };
        for (tab_id, tab_position) in [(usize::MAX, 0), (0, usize::MAX)] {
            let invalid = PaneMetadata {
                terminal_id: TerminalId::new(2),
                tab_id: yinz_types::internal_ids::TabId::new(tab_id),
                tab_position: yinz_types::internal_ids::TabPosition::new(tab_position),
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
            tab_id: yinz_types::internal_ids::TabId::new(1),
            tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
            tab_id: yinz_types::internal_ids::TabId::new(0),
            tab_position: yinz_types::internal_ids::TabPosition::new(terminal_id as usize),
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
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
            tab_id: yinz_types::internal_ids::TabId::new(0),
            tab_position: yinz_types::internal_ids::TabPosition::new(0),
            tab_name: "a".to_string(),
            title: "t".to_string(),
            cols: 80,
            rows: 24,
        };
        store.update_panes(&[meta1], ElapsedMicros::new(0))?;
        // update with different size, should not change initial
        let meta2 = PaneMetadata {
            terminal_id: TerminalId::new(9),
            tab_id: yinz_types::internal_ids::TabId::new(0),
            tab_position: yinz_types::internal_ids::TabPosition::new(0),
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

pub use yinz_types::recording::{RecordingSummary, SearchHit};

pub fn list_recordings(data_dir: &Path) -> Result<Vec<RecordingSummary>> {
    SqliteRecordingBackend::new(data_dir).list()
}

fn sqlite_list_recordings(data_dir: &Path) -> Result<Vec<RecordingSummary>> {
    let db_path = data_dir.join("session-recordings.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let mut conn = connect(&db_path)?;
    // One snapshot for every query below, so that a compaction committing meanwhile cannot
    // show a pane's events in neither table. Read-only: dropping it ends the snapshot.
    let tx = conn
        .transaction()
        .context("failed to begin reading recordings")?;
    // order: newest session first, then tab_position, then terminal_id
    let mut stmt = tx.prepare(
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
    // Fix N+1: fetch MAX(elapsed_us) for all panes in one query per table
    let mut max_by_pane: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for query in [
        "SELECT pane_id, MAX(elapsed_us) FROM recording_events GROUP BY pane_id",
        "SELECT pane_id, MAX(last_elapsed_us) FROM recording_segments GROUP BY pane_id",
    ] {
        let mut stmt = tx.prepare(query)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (pane_id, max) = row?;
            let latest = max_by_pane.entry(pane_id).or_insert(max);
            *latest = (*latest).max(max);
        }
    }
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
    let mut conn = connect(&db_path)?;
    // One snapshot for the segments and the rows both: a compaction committing between reading
    // one and the other would leave the events it moved in neither. Read-only: dropping it
    // ends the snapshot.
    let tx = conn
        .transaction()
        .context("failed to begin reading the recording")?;
    // get pane and session
    let pane: Option<RecordedPaneRow> = {
        let mut stmt = tx.prepare(
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
    let mut prev_elapsed: Option<i64> = None;
    let mut pending_text = [Vec::new(), Vec::new()];
    let mut write_event = |elapsed_us: i64, code: &str, data: &[u8]| -> Result<()> {
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
        let secs = interval_us as f64 / 1_000_000.0;
        // format with at most 6 decimal places, trimming trailing zeros
        let secs_str = format_interval(secs);
        let data_str = if matches!(code, "o" | "i") {
            // PTY chunks and compressed segments can split a codepoint. Keep the incomplete
            // suffix for each independent byte stream until its next event completes it.
            let pending = &mut pending_text[usize::from(code == "i")];
            pending.extend_from_slice(data);
            let complete = utf8_safe_len(pending);
            let text = String::from_utf8_lossy(&pending[..complete]).into_owned();
            pending.drain(..complete);
            text
        } else {
            String::from_utf8_lossy(data).into_owned()
        };
        let code_json = serde_json::to_string(code)?;
        let data_json = serde_json::to_string(&data_str)?;
        writeln!(output, "[{}, {}, {}]", secs_str, code_json, data_json)?;
        prev_elapsed = Some(elapsed_us);
        Ok(())
    };
    // A damaged segment fails the export: a recording with a hole in it would play back as if
    // nothing were missing.
    for_each_segment(&tx, pane_id, i64::MIN, |segment| {
        for event in segment.events? {
            write_event(
                event.elapsed_us,
                &char::from(event.code).to_string(),
                &event.data,
            )?;
        }
        Ok(())
    })?;
    let mut stmt = tx.prepare(
        "SELECT elapsed_us, code, data FROM recording_events WHERE pane_id = ?1 ORDER BY elapsed_us ASC, id ASC",
    )?;
    let mut rows = stmt.query(rusqlite::params![pane_id])?;
    while let Some(row) = rows.next()? {
        let data: Vec<u8> = row.get(2)?;
        write_event(row.get(0)?, &row.get::<_, String>(1)?, &data)?;
    }
    // A stream ending inside a codepoint is genuinely incomplete. Emit its replacement at
    // the final timestamp; ordinary chunk boundaries must never introduce replacements.
    for (code, pending) in ["o", "i"].into_iter().zip(pending_text) {
        if !pending.is_empty() {
            writeln!(
                output,
                "{}",
                serde_json::json!([0, code, String::from_utf8_lossy(&pending)])
            )?;
        }
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
    let mut conn = connect(&db_path)?;
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
    // Acquire the write lock before reading progress: another search may have indexed the
    // same events while we waited. IMMEDIATE also avoids a deferred read-to-write upgrade,
    // which fails with SQLITE_BUSY without consulting the connection's busy timeout.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let panes: Vec<(i64, i64, Vec<u8>)> = {
        let mut stmt = tx.prepare(
            "SELECT p.id, COALESCE(st.last_event_id, 0), COALESCE(st.pending, X'')
             FROM recording_panes p
             LEFT JOIN recording_search_state st ON st.pane_id = p.id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
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
            let mut index_event = |elapsed_us: i64, data: &[u8]| {
                buffer.extend_from_slice(data);
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
            };
            // Output comes from one reader per pane, so the order segments keep it in is
            // also id order, and resuming after `last_event_id` works across both tables. It
            // resumes after the highest id seen of any kind, not the last output's: a segment
            // ending in input or a resize would otherwise be inflated again by every search.
            for_each_segment(&tx, pane_id, last_event_id, |segment| {
                last_seen = last_seen.max(Some(segment.max_event_id));
                match segment.events {
                    Ok(events) => {
                        for event in events {
                            if event.code == b'o' && event.id > last_event_id {
                                index_event(event.elapsed_us, &event.data);
                            }
                        }
                    },
                    // Moving past it costs search this segment's lines only, and only once.
                    Err(error) => log::warn!(
                        "search skips a damaged segment of recording {pane_id}: {error:#}"
                    ),
                }
                Ok(())
            })?;
            let mut stmt = tx.prepare(
                "SELECT id, elapsed_us, code, data FROM recording_events
                 WHERE pane_id = ?1 AND id > ?2
                 ORDER BY id ASC",
            )?;
            let mut rows = stmt.query(rusqlite::params![pane_id, last_event_id])?;
            while let Some(row) = rows.next()? {
                last_seen = Some(row.get(0)?);
                if row.get_ref(2)?.as_str()? == "o" {
                    let data: Vec<u8> = row.get(3)?;
                    index_event(row.get(1)?, &data);
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
    let cleaned = yinz_types::text::clean_string_from_control_and_linebreak(line);
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
                    tab_id: yinz_types::internal_ids::TabId::new(0),
                    tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
    fn concurrent_indexers_read_progress_after_acquiring_the_write_lock() -> Result<()> {
        use std::cell::RefCell;
        use std::sync::mpsc::{channel, Receiver, Sender};
        use std::time::Duration;

        thread_local! {
            static RETRY: RefCell<Option<(Sender<()>, Receiver<()>)>> = const { RefCell::new(None) };
        }
        let dir = tempdir()?;
        let mut store = open(dir.path());
        record(&mut store, 0, b"exactly once\n");
        let path = dir.path().join("session-recordings.db");
        let mut first = connect(&path)?;
        ensure_search_schema(&first)?;
        let blocker = first.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (waiting_tx, waiting_rx) = channel();
        let (retry_tx, retry_rx) = channel();
        let second = std::thread::spawn(move || -> Result<()> {
            let mut conn = connect(&path)?;
            RETRY.with(|retry| *retry.borrow_mut() = Some((waiting_tx, retry_rx)));
            conn.busy_handler(Some(|_| {
                RETRY.with(|retry| {
                    let retry = retry.borrow();
                    let Some((waiting, proceed)) = retry.as_ref() else {
                        return false;
                    };
                    waiting.send(()).is_ok()
                        && proceed.recv_timeout(Duration::from_secs(10)).is_ok()
                })
            }))?;
            index_pending(&mut conn)
        });
        waiting_rx.recv_timeout(Duration::from_secs(10))?;
        blocker.rollback()?;
        index_pending(&mut first)?;
        retry_tx.send(())?;
        second.join().unwrap()?;
        assert_eq!(run_search(&first, "exactly", 10)?.len(), 1);
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
                        tab_id: yinz_types::internal_ids::TabId::new(10),
                        tab_position: yinz_types::internal_ids::TabPosition::new(1),
                        tab_name: "tab1".to_string(),
                        title: "pane1".to_string(),
                        cols: 80,
                        rows: 24,
                    },
                    PaneMetadata {
                        terminal_id: TerminalId::new(2),
                        tab_id: yinz_types::internal_ids::TabId::new(10),
                        tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
                    tab_id: yinz_types::internal_ids::TabId::new(20),
                    tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
            tab_id: yinz_types::internal_ids::TabId::new(1),
            tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
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
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
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

#[cfg(test)]
mod compaction_tests {
    use super::*;
    use hegel::generators;
    use tempfile::tempdir;

    /// Long after everything recorded here, so all of it is cold.
    const LATER: i64 = i64::MAX / 2;

    fn open_store(dir: &Path) -> RecordingStore {
        let mut store = RecordingStore::open(dir, "session", true, UnixMicros::new(0)).unwrap();
        let panes = (1..=3)
            .map(|terminal| PaneMetadata {
                terminal_id: TerminalId::new(terminal),
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(terminal as usize),
                tab_name: "tab".to_string(),
                title: format!("pane {terminal}"),
                cols: 80,
                rows: 24,
            })
            .collect::<Vec<_>>();
        store.update_panes(&panes, ElapsedMicros::new(0)).unwrap();
        store
    }

    #[test]
    fn export_preserves_utf8_across_events_and_compaction() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        let text = "aé中🦀\u{fffd}";
        // Interleave independent input and output streams with resizes; neither a PTY read
        // nor a segment boundary is a character boundary.
        let mut events = Vec::new();
        for (index, byte) in text.bytes().chain([0xff, 0xe2, 0x82]).enumerate() {
            let elapsed = ElapsedMicros::new(index as i64);
            events.push(RecordedEvent::output(
                TerminalId::new(1),
                elapsed,
                vec![byte],
            ));
            events.push(RecordedEvent::input(
                TerminalId::new(1),
                elapsed,
                vec![byte],
            ));
            events.push(RecordedEvent::resize(TerminalId::new(1), elapsed, 80, 24));
        }
        store.record_events(&events)?;
        let before = export(dir.path(), 1);
        for compacted in [false, true] {
            if compacted {
                let mut conn = connection(dir.path());
                while matches!(compact_segment(&mut conn, 1, LATER, 1)?, Block::Packed(_)) {}
            }
            let exported = export(dir.path(), 1);
            assert_eq!(exported, before);
            let rows = exported
                .lines()
                .skip(1)
                .map(serde_json::from_str::<serde_json::Value>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for code in ["i", "o"] {
                let actual: String = rows
                    .iter()
                    .filter(|row| row[1] == code)
                    .map(|row| row[2].as_str().unwrap())
                    .collect();
                assert_eq!(actual, format!("{text}��"));
            }
            let elapsed: f64 = rows.iter().map(|row| row[0].as_f64().unwrap()).sum();
            assert!((elapsed - (text.len() + 2) as f64 / 1_000_000.0).abs() < 1e-12);
        }
        Ok(())
    }

    fn output(terminal: u32, elapsed_us: i64, data: &[u8]) -> RecordedEvent {
        RecordedEvent::output(
            TerminalId::new(terminal),
            ElapsedMicros::new(elapsed_us),
            data.to_vec(),
        )
    }

    fn connection(dir: &Path) -> rusqlite::Connection {
        rusqlite::Connection::open(dir.join("session-recordings.db")).unwrap()
    }

    /// One pass by a compactor that remembers nothing and is never stopped.
    fn pass(
        conn: &mut rusqlite::Connection,
        live: Option<LiveSession>,
        budget: u64,
        segment_bytes: usize,
    ) -> Result<CompactionReport> {
        compact_store(
            conn,
            live,
            budget,
            segment_bytes,
            &AtomicBool::new(false),
            &mut HashMap::new(),
        )
    }

    /// The store's one recording as its own compactor sees it, its clock at `elapsed_us`.
    fn own_clock(dir: &Path, elapsed_us: i64) -> Option<LiveSession> {
        let id = connection(dir)
            .query_row("SELECT id FROM recording_sessions", [], |row| row.get(0))
            .unwrap();
        Some(LiveSession { id, elapsed_us })
    }

    fn export(dir: &Path, terminal: u32) -> String {
        let summary = list_recordings(dir)
            .unwrap()
            .into_iter()
            .find(|summary| summary.terminal_id == TerminalId::new(terminal))
            .unwrap();
        let mut out = Vec::new();
        export_recording(dir, summary.id, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Every pane's listing and export, which is everything a reader can see of the recording
    /// apart from search.
    fn exports(dir: &Path) -> Vec<(RecordingSummary, String)> {
        list_recordings(dir)
            .unwrap()
            .into_iter()
            .map(|summary| {
                let mut out = Vec::new();
                export_recording(dir, summary.id, &mut out).unwrap();
                (summary, String::from_utf8(out).unwrap())
            })
            .collect()
    }

    /// An indexed line: its text, pane and the elapsed time of the event that completed it.
    type IndexedLine = (String, i64, i64);
    /// Where the indexer left a pane: its id, the last event indexed and the unfinished line.
    type IndexerState = (i64, i64, Vec<u8>);

    /// Indexes whatever is pending, then returns the search index and the indexer's resume
    /// state: the lines, which event each was attributed to, and where each pane left off.
    fn search_index(dir: &Path) -> (Vec<IndexedLine>, Vec<IndexerState>) {
        search_recordings(dir, "anything", 1).unwrap();
        let conn = connection(dir);
        let lines = conn
            .prepare("SELECT text, pane_id, elapsed_us FROM recording_search ORDER BY rowid")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let state = conn
            .prepare("SELECT pane_id, last_event_id, pending FROM recording_search_state ORDER BY pane_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (lines, state)
    }

    fn row_count(dir: &Path, table: &str) -> i64 {
        connection(dir)
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// Two stores fed the same events, searched at the same moments, one of them compacted at
    /// random moments with random block sizes and budgets, by its own recorder or from another
    /// process, each remembering the short blocks it found: nothing a reader sees differs.
    ///
    /// Output is timed on a clock that only moves forward, as a pane's single reader does;
    /// input and resizes run up to 100 ms behind it, as events from other threads can.
    #[hegel::test]
    fn compaction_changes_nothing_a_reader_sees(tc: hegel::TestCase) {
        let (packed, plain) = (tempdir().unwrap(), tempdir().unwrap());
        let mut stores = [open_store(packed.path()), open_store(plain.path())];
        let segment_bytes = tc.draw(generators::integers::<usize>().min_value(1).max_value(300));
        // The recorder's own compactor's memory, and another process's.
        let (mut own_short_blocks, mut other_short_blocks) = (HashMap::new(), HashMap::new());
        let mut clock = 0i64;
        let op_count = tc.draw(generators::integers::<usize>().max_value(40));
        for _ in 0..op_count {
            let terminal = tc.draw(generators::integers::<u32>().min_value(1).max_value(3));
            let event = match tc.draw(generators::integers::<u8>().max_value(6)) {
                0..=2 => {
                    let mut data = tc.draw(generators::binary().max_size(40));
                    if tc.draw(generators::booleans()) {
                        data.extend_from_slice(b"line\r\n");
                    }
                    Some(output(terminal, clock, &data))
                },
                3 => {
                    let jitter = tc.draw(
                        generators::integers::<i64>()
                            .min_value(0)
                            .max_value(100_000),
                    );
                    let elapsed_us = ElapsedMicros::new((clock - jitter).max(0));
                    Some(if tc.draw(generators::booleans()) {
                        RecordedEvent::input(TerminalId::new(terminal), elapsed_us, b"k".to_vec())
                    } else {
                        let cols = tc.draw(generators::integers::<u16>());
                        RecordedEvent::resize(TerminalId::new(terminal), elapsed_us, cols, 30)
                    })
                },
                4 => {
                    // In whole minutes: drawn in microseconds, the clock rarely gets far
                    // enough for anything to go cold.
                    let minutes = tc.draw(generators::integers::<i64>().min_value(0).max_value(20));
                    clock += minutes * 60_000_000 + 1;
                    None
                },
                5 => {
                    assert_eq!(search_index(packed.path()), search_index(plain.path()));
                    None
                },
                _ => {
                    let budget = tc.draw(generators::integers::<u64>().min_value(1).max_value(400));
                    let (live, short_blocks) = if tc.draw(generators::booleans()) {
                        (own_clock(packed.path(), clock), &mut own_short_blocks)
                    } else {
                        (None, &mut other_short_blocks)
                    };
                    compact_store(
                        &mut connection(packed.path()),
                        live,
                        budget,
                        segment_bytes,
                        &AtomicBool::new(false),
                        short_blocks,
                    )
                    .unwrap();
                    None
                },
            };
            if let Some(event) = event {
                for store in &mut stores {
                    store.record_events(std::slice::from_ref(&event)).unwrap();
                }
            }
        }
        assert_eq!(exports(packed.path()), exports(plain.path()));
        assert_eq!(search_index(packed.path()), search_index(plain.path()));
    }

    #[test]
    fn the_newest_row_stays_so_later_events_are_still_indexed() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[output(1, 0, b"first\n"), output(1, 1, b"second\n")])?;
        let live = own_clock(dir.path(), LATER);
        let report = pass(&mut connection(dir.path()), live, u64::MAX, 1)?;
        assert_eq!(report.events, 1, "only the older of the two rows moves");

        store.record_events(&[output(1, 2, b"third\n")])?;
        let conn = connection(dir.path());
        let (new_id, packed_max): (i64, i64) = conn.query_row(
            "SELECT (SELECT MAX(id) FROM recording_events),
                    (SELECT MAX(max_event_id) FROM recording_segments)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert!(
            new_id > packed_max,
            "id {new_id} reuses one a segment holds"
        );
        let texts: Vec<String> = search_recordings(dir.path(), "first OR second OR third", 10)?
            .into_iter()
            .map(|hit| hit.text)
            .collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        Ok(())
    }

    #[test]
    fn a_resize_packed_into_a_segment_still_keeps_the_first_seen_size() -> Result<()> {
        let dir = tempdir()?;
        // A pane whose output arrives before its metadata is created at the default size,
        // which the metadata may correct only if no resize has been recorded since.
        let mut store = RecordingStore::open(dir.path(), "session", false, UnixMicros::new(0))?;
        store.record_events(&[
            output(1, 0, b"early"),
            RecordedEvent::resize(TerminalId::new(1), ElapsedMicros::new(1), 120, 40),
            output(2, 2, b"newest row, left behind"),
        ])?;
        let live = own_clock(dir.path(), LATER);
        pass(&mut connection(dir.path()), live, u64::MAX, 1)?;
        let rows_left: i64 = connection(dir.path()).query_row(
            "SELECT COUNT(*) FROM recording_events e JOIN recording_panes p ON p.id = e.pane_id
             WHERE p.terminal_id = 1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            rows_left, 0,
            "the resize must be in a segment for this test to mean anything"
        );

        store.update_panes(
            &[PaneMetadata {
                terminal_id: TerminalId::new(1),
                tab_id: yinz_types::internal_ids::TabId::new(0),
                tab_position: yinz_types::internal_ids::TabPosition::new(0),
                tab_name: "tab".to_string(),
                title: "late metadata".to_string(),
                cols: 100,
                rows: 30,
            }],
            ElapsedMicros::new(3),
        )?;
        let size: (i64, i64) = connection(dir.path()).query_row(
            "SELECT initial_cols, initial_rows FROM recording_panes WHERE terminal_id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(size, (80, 24));
        Ok(())
    }

    #[test]
    fn a_short_block_waits_until_the_pane_goes_quiet() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        let old = 1_000;
        let recent = old + COMPACT_AFTER_US + 1_000;
        store.record_events(&[
            output(1, old, b"old output"),
            output(1, recent, b"recent output"),
            output(2, recent, b"newest row"),
        ])?;
        // Only the old event is cold, and it is far short of a block while the pane is busy.
        let now = ElapsedMicros::new(recent);
        assert_eq!(store.compact(now, u64::MAX)?, CompactionReport::default());
        // Once the pane has been quiet for as long, the short block is packed after all.
        let later = ElapsedMicros::new(recent + COMPACT_AFTER_US + 1);
        let report = store.compact(later, u64::MAX)?;
        assert_eq!((report.segments, report.events), (1, 2));
        Ok(())
    }

    /// Settling that a busy pane is short of a block reads nothing under the write lock, so the
    /// recorder's flushes never wait on it.
    #[test]
    fn a_short_block_in_a_busy_pane_takes_no_write_lock() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        let old = 1_000;
        let recent = old + COMPACT_AFTER_US + 1_000;
        store.record_events(&[
            output(1, old, b"old output"),
            output(1, recent, b"recent output"),
            output(2, recent, b"newest row"),
        ])?;
        let live = own_clock(dir.path(), recent);
        let writer = connection(dir.path());
        writer.execute_batch("BEGIN IMMEDIATE")?;
        // With no wait for it, taking the write lock would fail at once.
        let mut compactor = connection(dir.path());
        compactor.busy_timeout(Duration::ZERO)?;
        let report = pass(&mut compactor, live, u64::MAX, SEGMENT_BYTES)?;
        assert_eq!(report, CompactionReport::default());
        writer.execute_batch("COMMIT")?;
        Ok(())
    }

    /// A pane remembered as short of a block is passed over only while its cutoff stays put.
    #[test]
    fn a_remembered_short_block_is_revisited_once_its_cutoff_moves() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        let old = 1_000;
        let recent = old + COMPACT_AFTER_US + 1_000;
        store.record_events(&[
            output(1, old, b"old output"),
            output(1, recent, b"recent output"),
        ])?;
        let pane_id: i64 = connection(dir.path()).query_row(
            "SELECT id FROM recording_panes WHERE terminal_id = 1",
            [],
            |row| row.get(0),
        )?;
        // Compacted from another process, the session known only by its newest event.
        let (stop, mut short_blocks) = (AtomicBool::new(false), HashMap::new());
        let report = compact_store(
            &mut connection(dir.path()),
            None,
            u64::MAX,
            SEGMENT_BYTES,
            &stop,
            &mut short_blocks,
        )?;
        assert_eq!(report, CompactionReport::default());
        assert_eq!(
            short_blocks.get(&pane_id),
            Some(&(recent - COMPACT_AFTER_US))
        );

        // The session records again, far enough on that pane 1 has gone quiet.
        store.record_events(&[output(2, recent + COMPACT_AFTER_US + 1, b"much later")])?;
        let report = compact_store(
            &mut connection(dir.path()),
            None,
            u64::MAX,
            SEGMENT_BYTES,
            &stop,
            &mut short_blocks,
        )?;
        assert_eq!((report.segments, report.events), (1, 2));
        Ok(())
    }

    #[test]
    fn a_stopped_pass_moves_nothing() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[
            output(1, 0, b"one"),
            output(1, 1, b"two"),
            output(2, 2, b"newest row"),
        ])?;
        let live = own_clock(dir.path(), LATER);
        let report = compact_store(
            &mut connection(dir.path()),
            live,
            u64::MAX,
            1,
            &AtomicBool::new(true),
            &mut HashMap::new(),
        )?;
        assert_eq!(report, CompactionReport::default());
        assert_eq!(row_count(dir.path(), "recording_events"), 3);
        Ok(())
    }

    /// The newest row in the table stays, but the rest of its pane's short tail does not wait
    /// for some other pane to write something first.
    #[test]
    fn a_quiet_pane_holding_the_newest_row_packs_everything_else() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[
            output(1, 0, b"one"),
            output(1, 1, b"two"),
            output(1, 2, b"three, the newest row"),
        ])?;
        let report = store.compact(ElapsedMicros::new(LATER), u64::MAX)?;
        assert_eq!((report.segments, report.events), (1, 2));
        assert_eq!(row_count(dir.path(), "recording_events"), 1);
        // With only the newest row left, a later pass has nothing to do.
        assert_eq!(
            store.compact(ElapsedMicros::new(LATER), u64::MAX)?,
            CompactionReport::default()
        );
        Ok(())
    }

    #[test]
    fn compact_recordings_shrinks_the_file_and_changes_no_reader() -> Result<()> {
        let dir = tempdir()?;
        {
            let mut store = open_store(dir.path());
            // About 2 MiB of build-log-like output in PTY-sized reads.
            let events: Vec<RecordedEvent> = (0..800)
                .map(|read| {
                    let chunk: String = (0..40)
                        .map(|line| {
                            format!(
                                "\x1b[32m   Compiling\x1b[0m crate-{read}-{line} v0.{line}.0 (/src/crate-{read})\r\n"
                            )
                        })
                        .collect();
                    output(1, read, chunk.as_bytes())
                })
                .chain([output(2, 900, b"newest row")])
                .collect();
            store.record_events(&events)?;
            store.finish_session(UnixMicros::new(1_000), ElapsedMicros::new(1_000))?;
        }
        let before = exports(dir.path());

        // The session has finished, so all of it is cold whatever time it is.
        let result = compact_recordings(dir.path())?;
        assert_eq!(result.compaction.segments, 2, "{result:?}");
        assert!(
            result.bytes_after * 3 < result.bytes_before,
            "expected the file to shrink well past 3x: {result:?}"
        );
        assert_eq!(exports(dir.path()), before);
        let hits = search_recordings(dir.path(), "\"crate-799-39\"", 10)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(row_count(dir.path(), "recording_events"), 1);
        Ok(())
    }

    /// Seen from another process, a running session's clock is known only by its newest event,
    /// so the ten minutes before that stay rows: an event still on its way, stamped a little
    /// behind, must be able to land among them.
    #[test]
    fn a_running_session_compacted_from_elsewhere_keeps_its_last_ten_minutes() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        let newest = COMPACT_AFTER_US + 60_000_000;
        store.record_events(&[
            output(1, 0, b"early"),
            output(1, newest, b"late"),
            output(2, newest, b"newest row"),
        ])?;
        let report = pass(&mut connection(dir.path()), None, u64::MAX, 1)?;
        assert_eq!(
            report.events, 1,
            "only the event ten minutes before the newest moves"
        );

        store.record_events(&[RecordedEvent::input(
            TerminalId::new(1),
            ElapsedMicros::new(newest - 50_000),
            b"typed".to_vec(),
        )])?;
        let events: Vec<String> = export(dir.path(), 1)
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()[2].to_string())
            .collect();
        assert_eq!(events, ["\"early\"", "\"typed\"", "\"late\""]);
        Ok(())
    }

    /// Writes an export into memory, and runs a compaction pass on a connection of its own as
    /// the first event goes out: after the export has begun reading segments, before the rows.
    struct CompactingMidExport<'a> {
        dir: &'a Path,
        out: Vec<u8>,
        compacted: Option<CompactionReport>,
    }

    impl std::io::Write for CompactingMidExport<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.compacted.is_none() && self.out.ends_with(b"\n") {
                let live = own_clock(self.dir, LATER);
                self.compacted =
                    Some(pass(&mut connection(self.dir), live, u64::MAX, SEGMENT_BYTES).unwrap());
            }
            self.out.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_export_sees_every_event_while_compaction_moves_them() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[
            output(1, 0, b"one"),
            output(1, 1, b"two"),
            output(1, 2, b"three"),
            output(1, 3, b"four"),
            output(2, 4, b"newest row"),
        ])?;
        // One event in a segment, so the export has a segment to be reading when the rest move.
        let live = own_clock(dir.path(), LATER);
        assert_eq!(pass(&mut connection(dir.path()), live, 1, 1)?.events, 1);

        let pane = list_recordings(dir.path())?
            .into_iter()
            .find(|summary| summary.terminal_id == TerminalId::new(1))
            .context("pane 1 is listed")?;
        let mut during = CompactingMidExport {
            dir: dir.path(),
            out: Vec::new(),
            compacted: None,
        };
        export_recording(dir.path(), pane.id, &mut during)?;
        assert_eq!(during.compacted.map(|report| report.events), Some(3));
        let after = export(dir.path(), 1);
        assert_eq!(String::from_utf8(during.out)?, after);
        assert_eq!(after.lines().count(), 5, "{after}");
        Ok(())
    }

    /// Search resumes after the highest id it has seen of any kind, so a segment that ends in
    /// input is not inflated again by every later search.
    #[test]
    fn search_resumes_after_a_segment_that_ends_in_input() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[
            output(1, 0, b"shown\n"),
            RecordedEvent::input(TerminalId::new(1), ElapsedMicros::new(1), b"typed".to_vec()),
            output(2, 2, b"newest row"),
        ])?;
        let live = own_clock(dir.path(), LATER);
        pass(&mut connection(dir.path()), live, u64::MAX, SEGMENT_BYTES)?;
        let (pane_id, packed_max): (i64, i64) = connection(dir.path()).query_row(
            "SELECT pane_id, max_event_id FROM recording_segments",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (_, state) = search_index(dir.path());
        let resumed_after = state
            .iter()
            .find(|(pane, ..)| *pane == pane_id)
            .map(|(_, last_event_id, _)| *last_event_id);
        assert_eq!(resumed_after, Some(packed_max));
        Ok(())
    }

    /// A segment that no longer decodes costs search only the events it held, and only once:
    /// the rest of the store, the pane's later rows included, stays searchable. The pane's
    /// export still fails, rather than hand over a recording with a hole in it.
    #[test]
    fn a_damaged_segment_costs_search_only_its_own_events() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[
            output(1, 0, b"damaged\n"),
            output(2, 1, b"unharmed\n"),
            output(3, 2, b"newest row\n"),
        ])?;
        let live = own_clock(dir.path(), LATER);
        let report = pass(&mut connection(dir.path()), live, u64::MAX, SEGMENT_BYTES)?;
        assert_eq!(report.segments, 2);
        connection(dir.path()).execute(
            "UPDATE recording_segments SET data = X'00'
             WHERE pane_id = (SELECT id FROM recording_panes WHERE terminal_id = 1)",
            [],
        )?;
        store.record_events(&[output(1, 3, b"written after\n")])?;
        for _ in 0..2 {
            let mut texts: Vec<String> =
                search_recordings(dir.path(), "damaged OR unharmed OR written", 10)?
                    .into_iter()
                    .map(|hit| hit.text)
                    .collect();
            texts.sort();
            assert_eq!(texts, ["unharmed", "written after"]);
        }

        let panes = list_recordings(dir.path())?;
        let pane = |terminal| {
            panes
                .iter()
                .find(|summary| summary.terminal_id == TerminalId::new(terminal))
                .map(|summary| summary.id)
                .context("the pane is listed")
        };
        assert!(export_recording(dir.path(), pane(1)?, &mut Vec::<u8>::new()).is_err());
        export_recording(dir.path(), pane(2)?, &mut Vec::<u8>::new())?;
        Ok(())
    }

    /// A flush that meets another connection's write, the compactor's or another server's,
    /// waits its turn instead of failing and losing the batch.
    #[test]
    fn recording_waits_for_another_connections_write() -> Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};
        static WAITED: AtomicBool = AtomicBool::new(false);
        fn busy(_attempts: i32) -> bool {
            WAITED.store(true, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(1));
            true
        }

        let dir = tempdir()?;
        let mut session =
            SqliteRecordingSession::open(dir.path(), "busy", false, UnixMicros::new(0))?;
        session.connection.busy_handler(Some(busy))?;
        let writer = connection(dir.path());
        writer.execute_batch("BEGIN IMMEDIATE")?;
        let recording = std::thread::spawn(move || {
            session.record_events(&[output(1, 0, b"flushed while another connection writes")])
        });
        // Until the recorder is waiting, or has given up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !WAITED.load(Ordering::SeqCst) && !recording.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "the flush neither waited nor ended"
            );
            std::thread::yield_now();
        }
        writer.execute_batch("COMMIT")?;
        recording.join().expect("the recording thread panicked")?;
        assert_eq!(row_count(dir.path(), "recording_events"), 1);
        Ok(())
    }

    #[test]
    fn readers_open_a_database_from_before_segments() -> Result<()> {
        let dir = tempdir()?;
        let mut store = open_store(dir.path());
        store.record_events(&[output(1, 0, b"needle\n")])?;
        drop(store);
        connection(dir.path()).execute_batch("DROP TABLE recording_segments")?;
        assert_eq!(list_recordings(dir.path())?.len(), 3);
        assert_eq!(search_recordings(dir.path(), "needle", 10)?.len(), 1);
        assert_eq!(exports(dir.path()).len(), 3);
        Ok(())
    }
}
