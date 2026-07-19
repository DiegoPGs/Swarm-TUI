//! Session registry (ADR-0002): a thin SQLite mapping, not a session store.
//!
//! Owns: swarm-tui-id ↔ (tool slug, native id, cwd, name, mode, status,
//! timestamps, cost) + dispatch history. Does NOT own transcripts, agent
//! state, or anything the native tools already persist.
//!
//! Reconciliation contract (runs at startup, read-only against the tools):
//! - Claude Code: `claude agents --json --all` → merge native background
//!   sessions into the roster, even ones swarm-tui never started.
//! - All tools: rows whose native session can't be found are marked
//!   `SessionStatus::Orphaned` — never auto-deleted, never cascade anything.
//! - agy backfill lane: headless runs that produced no native id get one
//!   attached later (see the antigravity adapter notes).

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::core::session::{SessionMode, SessionRecord, SessionStatus};

/// Schema v3 (v2 + the `role` launch-preset provenance column, ADR-0010;
/// v2 = v1 + `model`/`effort` launch options, ADR-0009). Newer columns come
/// **last** in the CREATE so fresh and migrated databases have identical
/// column order.
///
/// Note: docs/adr/0002-session-model-thin-registry.md's prose calls the
/// timestamp/cost columns `last_activity`/`last_cost_usd`, but
/// `SessionRecord` (the Rust struct, source of truth) uses
/// `updated_at`/`cost_usd`. This schema intentionally follows the struct,
/// not the ADR prose.
const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY, tool TEXT NOT NULL, native_id TEXT, name TEXT,
    cwd TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('interactive','headless')),
    status TEXT NOT NULL CHECK (status IN ('running','completed','failed','orphaned')),
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, cost_usd REAL,
    model TEXT, effort TEXT, role TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_tool_native ON sessions(tool, native_id);
-- Dispatch provenance/history (ADR-0013): written by record_dispatch /
-- finalize_dispatch since milestone 3; pre-created in 2a so no migration was needed.
CREATE TABLE IF NOT EXISTS dispatches (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER REFERENCES sessions(id),
    tool TEXT NOT NULL, prompt TEXT NOT NULL, cwd TEXT NOT NULL,
    started_at INTEGER NOT NULL, finished_at INTEGER, outcome TEXT, cost_usd REAL
);
";

const SCHEMA_VERSION: i64 = 3;

/// v1 → v2: additive columns only, one transaction. A v1 database opened by
/// this build hits the `CREATE TABLE IF NOT EXISTS` no-op above (its
/// `sessions` lacks the new columns), so the ALTERs below supply them.
const MIGRATE_V1_TO_V2: &str = "
BEGIN;
ALTER TABLE sessions ADD COLUMN model TEXT;
ALTER TABLE sessions ADD COLUMN effort TEXT;
UPDATE schema_version SET version = 2;
COMMIT;
";

/// v2 → v3: same additive shape. Each migration step keeps its own
/// BEGIN/COMMIT so a v1 open interrupted between steps leaves a valid v2
/// database that the next open resumes from.
const MIGRATE_V2_TO_V3: &str = "
BEGIN;
ALTER TABLE sessions ADD COLUMN role TEXT;
UPDATE schema_version SET version = 3;
COMMIT;
";

pub struct Registry {
    conn: Connection,
}

impl Registry {
    pub fn open(db_path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Open(e.to_string()))?;
        }
        let conn = Connection::open(db_path).map_err(|e| StoreError::Open(e.to_string()))?;
        // Best-effort: don't fail open() over WAL mode not being available
        // (e.g. some restricted/networked filesystems).
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| StoreError::Open(e.to_string()))?;

        // MAX over an empty table is NULL → None = a database this build just
        // created (the batch above made v2 tables directly).
        let version: Option<i64> = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .map_err(|e| StoreError::Open(e.to_string()))?;
        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )
                .map_err(|e| StoreError::Open(e.to_string()))?;
            }
            Some(1) => {
                conn.execute_batch(MIGRATE_V1_TO_V2)
                    .map_err(|e| StoreError::Open(format!("v1→v2 migration failed: {e}")))?;
                conn.execute_batch(MIGRATE_V2_TO_V3)
                    .map_err(|e| StoreError::Open(format!("v2→v3 migration failed: {e}")))?;
            }
            Some(2) => {
                conn.execute_batch(MIGRATE_V2_TO_V3)
                    .map_err(|e| StoreError::Open(format!("v2→v3 migration failed: {e}")))?;
            }
            Some(SCHEMA_VERSION) => {}
            Some(newer) => {
                return Err(StoreError::Open(format!(
                    "registry schema v{newer} is newer than this build supports \
                     (max v{SCHEMA_VERSION}) — refusing to open"
                )));
            }
        }

        Ok(Registry { conn })
    }

    /// Insert a brand-new session row and return its id. `record.id` is
    /// ignored: SQLite assigns the id inside the INSERT, under its own write
    /// lock, so two processes sharing one registry file can never allocate
    /// the same id and clobber each other's rows (the previous
    /// `SELECT MAX(id)+1` → `INSERT ON CONFLICT DO UPDATE` pair could).
    /// Status updates to an existing row still go through `upsert`.
    pub fn create(&mut self, record: &SessionRecord) -> Result<u64, StoreError> {
        self.conn
            .execute(
                "INSERT INTO sessions
                    (tool, native_id, name, cwd, mode, status, created_at, updated_at,
                     cost_usd, model, effort, role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    record.tool,
                    record.native_id,
                    record.name,
                    record.cwd.to_string_lossy(),
                    mode_to_str(record.mode),
                    status_to_str(record.status),
                    system_time_to_unix(record.created_at),
                    system_time_to_unix(record.updated_at),
                    record.cost_usd,
                    record.model,
                    record.effort,
                    record.role,
                ],
            )
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn upsert(&mut self, record: &SessionRecord) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO sessions
                    (id, tool, native_id, name, cwd, mode, status, created_at, updated_at,
                     cost_usd, model, effort, role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(id) DO UPDATE SET
                    tool=excluded.tool,
                    native_id=excluded.native_id,
                    name=excluded.name,
                    cwd=excluded.cwd,
                    mode=excluded.mode,
                    status=excluded.status,
                    updated_at=excluded.updated_at,
                    cost_usd=excluded.cost_usd,
                    model=excluded.model,
                    effort=excluded.effort,
                    role=excluded.role",
                params![
                    record.id as i64,
                    record.tool,
                    record.native_id,
                    record.name,
                    record.cwd.to_string_lossy(),
                    mode_to_str(record.mode),
                    status_to_str(record.status),
                    system_time_to_unix(record.created_at),
                    system_time_to_unix(record.updated_at),
                    record.cost_usd,
                    record.model,
                    record.effort,
                    record.role,
                ],
            )
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, tool, native_id, name, cwd, mode, status, created_at, updated_at,
                        cost_usd, model, effort, role
                 FROM sessions ORDER BY updated_at DESC",
            )
            .map_err(|e| StoreError::Query(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let tool: String = row.get(1)?;
                let native_id: Option<String> = row.get(2)?;
                let name: Option<String> = row.get(3)?;
                let cwd: String = row.get(4)?;
                let mode: String = row.get(5)?;
                let status: String = row.get(6)?;
                let created_at: i64 = row.get(7)?;
                let updated_at: i64 = row.get(8)?;
                let cost_usd: Option<f64> = row.get(9)?;
                let model: Option<String> = row.get(10)?;
                let effort: Option<String> = row.get(11)?;
                let role: Option<String> = row.get(12)?;
                Ok((
                    id, tool, native_id, name, cwd, mode, status, created_at, updated_at, cost_usd,
                    model, effort, role,
                ))
            })
            .map_err(|e| StoreError::Query(e.to_string()))?;

        let mut records = Vec::new();
        for row in rows {
            let (
                id,
                tool,
                native_id,
                name,
                cwd,
                mode,
                status,
                created_at,
                updated_at,
                cost_usd,
                model,
                effort,
                role,
            ) = row.map_err(|e| StoreError::Query(e.to_string()))?;
            records.push(SessionRecord {
                id: id as u64,
                tool,
                native_id,
                name,
                cwd: std::path::PathBuf::from(cwd),
                mode: mode_from_str(&mode)?,
                status: status_from_str(&status)?,
                created_at: unix_to_system_time(created_at),
                updated_at: unix_to_system_time(updated_at),
                cost_usd,
                model,
                effort,
                role,
            });
        }
        Ok(records)
    }

    /// First writer of the pre-created `dispatches` table (ADR-0013): one
    /// row per headless dispatch, inserted at spawn; `finalize_dispatch`
    /// completes it on the terminal event. Returns the dispatch row id.
    ///
    /// **The prompt is redacted here, unconditionally** (spec W-29, F-011).
    /// This is the persistence chokepoint by design: redacting at the call
    /// site would let a future caller forget. The *raw* prompt still reaches
    /// the CLI — adapters build argv from `Task.prompt`, never from this row.
    pub fn record_dispatch(
        &mut self,
        session_id: u64,
        tool: &str,
        prompt: &str,
        cwd: &Path,
    ) -> Result<u64, StoreError> {
        self.conn
            .execute(
                "INSERT INTO dispatches (session_id, tool, prompt, cwd, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    session_id as i64,
                    tool,
                    crate::core::redact::redact(prompt),
                    cwd.to_string_lossy(),
                    system_time_to_unix(SystemTime::now()),
                ],
            )
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn finalize_dispatch(
        &mut self,
        dispatch_id: u64,
        outcome: &str,
        cost_usd: Option<f64>,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "UPDATE dispatches SET finished_at = ?1, outcome = ?2, cost_usd = ?3
                 WHERE id = ?4",
                params![
                    system_time_to_unix(SystemTime::now()),
                    outcome,
                    cost_usd,
                    dispatch_id as i64,
                ],
            )
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    /// Recent dispatch history, newest first (test + timeline surface).
    pub fn dispatch_history(&self, limit: usize) -> Result<Vec<DispatchRow>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, session_id, tool, prompt, outcome, cost_usd, finished_at
                 FROM dispatches ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| StoreError::Query(e.to_string()))?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(DispatchRow {
                    id: row.get::<_, i64>(0)? as u64,
                    session_id: row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
                    tool: row.get(2)?,
                    prompt: row.get(3)?,
                    outcome: row.get(4)?,
                    cost_usd: row.get(5)?,
                    finished: row.get::<_, Option<i64>>(6)?.is_some(),
                })
            })
            .map_err(|e| StoreError::Query(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| StoreError::Query(e.to_string()))?);
        }
        Ok(out)
    }

    /// Mark rows whose native sessions vanished. Takes the *found* native ids
    /// per tool so the store stays ignorant of how discovery works.
    ///
    /// Deliberately unimplemented in milestone 2a: reconciliation is
    /// read-only this milestone and never marks orphans; wired in a later
    /// milestone.
    pub fn mark_orphans(
        &mut self,
        _tool_slug: &str,
        _live_native_ids: &[String],
    ) -> Result<usize, StoreError> {
        todo!("wire reconciliation in a later milestone (2a is read-only)")
    }
}

/// A pre-epoch clock clamps to the epoch instead of panicking the write.
fn system_time_to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The stored value crosses a real boundary (any process/tool can have
/// written the database file): a negative or overflowing value clamps to
/// the epoch instead of panicking the roster read.
fn unix_to_system_time(v: i64) -> SystemTime {
    u64::try_from(v)
        .ok()
        .and_then(|secs| UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
        .unwrap_or(UNIX_EPOCH)
}

fn mode_to_str(mode: SessionMode) -> &'static str {
    match mode {
        SessionMode::Interactive => "interactive",
        SessionMode::Headless => "headless",
    }
}

fn mode_from_str(s: &str) -> Result<SessionMode, StoreError> {
    match s {
        "interactive" => Ok(SessionMode::Interactive),
        "headless" => Ok(SessionMode::Headless),
        other => Err(StoreError::Query(format!(
            "unrecognized session mode in registry: {other:?}"
        ))),
    }
}

fn status_to_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "running",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Orphaned => "orphaned",
    }
}

fn status_from_str(s: &str) -> Result<SessionStatus, StoreError> {
    match s {
        "running" => Ok(SessionStatus::Running),
        "completed" => Ok(SessionStatus::Completed),
        "failed" => Ok(SessionStatus::Failed),
        "orphaned" => Ok(SessionStatus::Orphaned),
        other => Err(StoreError::Query(format!(
            "unrecognized session status in registry: {other:?}"
        ))),
    }
}

/// One `dispatches` row (ADR-0013 provenance/history; not a session record).
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRow {
    pub id: u64,
    pub session_id: Option<u64>,
    pub tool: String,
    pub prompt: String,
    pub outcome: Option<String>,
    pub cost_usd: Option<f64>,
    pub finished: bool,
}

#[derive(Debug)]
pub enum StoreError {
    Open(String),
    Query(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_record(id: u64) -> SessionRecord {
        SessionRecord {
            id,
            tool: "claude-code".to_string(),
            native_id: None,
            name: None,
            cwd: PathBuf::from("/home/user/project"),
            mode: SessionMode::Interactive,
            status: SessionStatus::Running,
            created_at: UNIX_EPOCH + Duration::from_secs(1_000),
            updated_at: UNIX_EPOCH + Duration::from_secs(2_000),
            cost_usd: None,
            model: Some("opus".to_string()),
            effort: Some("high".to_string()),
            role: Some("coder".to_string()),
        }
    }

    /// The v1 schema + version stamp, verbatim as shipped in milestone 2a —
    /// used to build a genuine v1 database on disk for migration tests. Keep
    /// frozen; it intentionally does NOT track `SCHEMA_SQL`.
    const V1_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY, tool TEXT NOT NULL, native_id TEXT, name TEXT,
    cwd TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('interactive','headless')),
    status TEXT NOT NULL CHECK (status IN ('running','completed','failed','orphaned')),
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, cost_usd REAL
);
CREATE INDEX IF NOT EXISTS idx_sessions_tool_native ON sessions(tool, native_id);
CREATE TABLE IF NOT EXISTS dispatches (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER REFERENCES sessions(id),
    tool TEXT NOT NULL, prompt TEXT NOT NULL, cwd TEXT NOT NULL,
    started_at INTEGER NOT NULL, finished_at INTEGER, outcome TEXT, cost_usd REAL
);
INSERT INTO schema_version (version) VALUES (1);
";

    /// Build a real v1 database with one session row, exactly as a 2a-era
    /// build would have left it.
    fn make_v1_db(db_path: &std::path::Path) {
        let conn = Connection::open(db_path).expect("open raw v1 db");
        conn.execute_batch(V1_SCHEMA_SQL).expect("apply v1 schema");
        conn.execute(
            "INSERT INTO sessions
                (id, tool, native_id, name, cwd, mode, status, created_at, updated_at, cost_usd)
             VALUES (1, 'claude-code', 'abc-123', 'old session', '/home/user/project',
                     'interactive', 'completed', 1000, 2000, NULL)",
            [],
        )
        .expect("insert v1 row");
    }

    /// The v2 schema + version stamp, verbatim as shipped in milestone 2b —
    /// frozen like `V1_SCHEMA_SQL`; it intentionally does NOT track
    /// `SCHEMA_SQL`.
    const V2_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY, tool TEXT NOT NULL, native_id TEXT, name TEXT,
    cwd TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('interactive','headless')),
    status TEXT NOT NULL CHECK (status IN ('running','completed','failed','orphaned')),
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, cost_usd REAL,
    model TEXT, effort TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_tool_native ON sessions(tool, native_id);
CREATE TABLE IF NOT EXISTS dispatches (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER REFERENCES sessions(id),
    tool TEXT NOT NULL, prompt TEXT NOT NULL, cwd TEXT NOT NULL,
    started_at INTEGER NOT NULL, finished_at INTEGER, outcome TEXT, cost_usd REAL
);
INSERT INTO schema_version (version) VALUES (2);
";

    /// Build a real v2 database with one session row that *uses* the v2
    /// columns — proving migration preserves populated launch options.
    fn make_v2_db(db_path: &std::path::Path) {
        let conn = Connection::open(db_path).expect("open raw v2 db");
        conn.execute_batch(V2_SCHEMA_SQL).expect("apply v2 schema");
        conn.execute(
            "INSERT INTO sessions
                (id, tool, native_id, name, cwd, mode, status, created_at, updated_at,
                 cost_usd, model, effort)
             VALUES (1, 'claude-code', 'def-456', '2b session', '/home/user/project',
                     'interactive', 'completed', 1000, 2000, NULL, 'sonnet', 'xhigh')",
            [],
        )
        .expect("insert v2 row");
    }

    #[test]
    fn v1_db_migrates_through_chain_to_v3() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        make_v1_db(&db_path);

        let registry = Registry::open(&db_path).expect("open should migrate v1 → v2 → v3");

        let version: i64 = registry
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);

        let all = registry.all().expect("all() should succeed post-migration");
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.id, 1);
        assert_eq!(got.tool, "claude-code");
        assert_eq!(got.native_id.as_deref(), Some("abc-123"));
        assert_eq!(got.name.as_deref(), Some("old session"));
        assert_eq!(got.status, SessionStatus::Completed);
        // Pre-v2 rows have no launch options; pre-v3 rows have no role.
        assert_eq!(got.model, None);
        assert_eq!(got.effort, None);
        assert_eq!(got.role, None);
    }

    #[test]
    fn v2_db_migrates_to_v3_preserving_rows_and_nulls_role() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        make_v2_db(&db_path);

        let registry = Registry::open(&db_path).expect("open should migrate v2 → v3");

        let version: i64 = registry
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);

        let all = registry.all().expect("all() post-migration");
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.native_id.as_deref(), Some("def-456"));
        // v2 data survives intact; the new column defaults to NULL.
        assert_eq!(got.model.as_deref(), Some("sonnet"));
        assert_eq!(got.effort.as_deref(), Some("xhigh"));
        assert_eq!(got.role, None);
    }

    #[test]
    fn migrated_db_round_trips_role() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        make_v2_db(&db_path);

        {
            let mut registry = Registry::open(&db_path).expect("open should migrate");
            registry
                .create(&sample_record(0))
                .expect("create with role should succeed post-migration");
        }

        // Second open: version is already 3 — must be a no-op, not a re-run.
        let registry = Registry::open(&db_path).expect("reopen should be a no-op");
        let all = registry.all().expect("all()");
        assert_eq!(all.len(), 2);
        let migrated = all.iter().find(|r| r.id == 1).expect("v2 row survives");
        assert_eq!(migrated.role, None);
        let new = all.iter().find(|r| r.id == 2).expect("new row present");
        assert_eq!(new.role.as_deref(), Some("coder"));
    }

    #[test]
    fn migrated_db_round_trips_launch_options_and_is_idempotent_on_reopen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        make_v1_db(&db_path);

        {
            let mut registry = Registry::open(&db_path).expect("open should migrate");
            registry
                .create(&sample_record(0))
                .expect("create with model/effort should succeed post-migration");
        }

        // Second open: version is already current — must be a no-op.
        let registry = Registry::open(&db_path).expect("reopen should be a no-op");
        let all = registry.all().expect("all()");
        assert_eq!(all.len(), 2);
        let migrated = all.iter().find(|r| r.id == 1).expect("v1 row survives");
        assert_eq!(migrated.model, None);
        let new = all.iter().find(|r| r.id == 2).expect("new row present");
        assert_eq!(new.model.as_deref(), Some("opus"));
        assert_eq!(new.effort.as_deref(), Some("high"));
    }

    #[test]
    fn fresh_db_is_created_at_v3_directly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        let registry = Registry::open(&db_path).expect("open");
        let version: i64 = registry
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn newer_schema_than_supported_refuses_to_open() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        {
            let conn = Connection::open(&db_path).expect("open raw db");
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version (version) VALUES (99);",
            )
            .expect("stamp future version");
        }
        let err = match Registry::open(&db_path) {
            Err(e) => e,
            Ok(_) => panic!("must refuse a newer schema"),
        };
        assert!(matches!(err, StoreError::Open(msg) if msg.contains("v99")));
    }

    #[test]
    fn dispatch_rows_record_and_finalize() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut registry = Registry::open(&tmp.path().join("registry.db")).expect("open");
        let sid = registry.create(&sample_record(0)).expect("create session");
        let did = registry
            .record_dispatch(
                sid,
                "claude-code",
                "review the diff",
                Path::new("/tmp/repo"),
            )
            .expect("record dispatch");

        let rows = registry.dispatch_history(10).expect("history");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, did);
        assert_eq!(rows[0].session_id, Some(sid));
        assert_eq!(rows[0].tool, "claude-code");
        assert_eq!(rows[0].prompt, "review the diff");
        assert!(!rows[0].finished);
        assert_eq!(rows[0].outcome, None);

        registry
            .finalize_dispatch(did, "completed", Some(0.05))
            .expect("finalize");
        let rows = registry.dispatch_history(10).expect("history");
        assert!(rows[0].finished);
        assert_eq!(rows[0].outcome.as_deref(), Some("completed"));
        assert_eq!(rows[0].cost_usd, Some(0.05));
    }

    /// F-011 / spec W-29: a credential pasted into a dispatch prompt must not
    /// reach the registry verbatim. `record_dispatch` is the chokepoint — the
    /// raw prompt still goes to the CLI (pinned by
    /// `dispatch_argv_still_carries_the_raw_prompt` in the claude adapter);
    /// only the persisted copy is redacted.
    #[test]
    fn dispatch_prompt_with_api_key_is_redacted_before_persistence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut registry = Registry::open(&tmp.path().join("registry.db")).expect("open");
        let sid = registry.create(&sample_record(0)).expect("create session");
        let secret = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJKKKK";
        registry
            .record_dispatch(
                sid,
                "claude-code",
                &format!("deploy using {secret} then report back"),
                Path::new("/tmp/repo"),
            )
            .expect("record dispatch");

        let rows = registry.dispatch_history(10).expect("history");
        let stored = &rows[0].prompt;
        assert!(
            !stored.contains(secret),
            "secret persisted verbatim: {stored}"
        );
        assert!(
            stored.contains("[redacted:"),
            "no redaction marker in stored prompt: {stored}"
        );
        // Redaction is targeted, not a blanket wipe — the surrounding task
        // text is what makes the row useful for a postmortem.
        assert!(
            stored.contains("deploy using"),
            "lost leading text: {stored}"
        );
        assert!(
            stored.contains("then report back"),
            "lost trailing text: {stored}"
        );

        // The property that actually matters is bytes at rest, not what the
        // read path hands back. Scan every file the registry produced — the
        // .db plus its WAL/SHM sidecars, which hold recently-written rows until
        // a checkpoint folds them in.
        drop(registry);
        let mut scanned_bytes = 0usize;
        for entry in std::fs::read_dir(tmp.path()).expect("read tempdir") {
            let path = entry.expect("dir entry").path();
            let bytes = std::fs::read(&path).expect("read registry file");
            scanned_bytes += bytes.len();
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "secret found at rest in {}",
                path.display()
            );
        }
        // Guard against a vacuous pass if the layout ever changes and the scan
        // above finds nothing to read.
        assert!(scanned_bytes > 0, "scanned no registry bytes");
    }

    /// The failure mode on the other side of F-011: over-redaction mangling
    /// ordinary prompts. A prompt with no credential shapes round-trips byte
    /// for byte.
    #[test]
    fn ordinary_dispatch_prompts_are_persisted_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut registry = Registry::open(&tmp.path().join("registry.db")).expect("open");
        let sid = registry.create(&sample_record(0)).expect("create session");
        let prompt = "Refactor src/app/mod.rs: extract the 33ms tick into \
                      drive_background() and add a test at tests/fixtures/x.json";
        registry
            .record_dispatch(sid, "claude-code", prompt, Path::new("/tmp/repo"))
            .expect("record dispatch");

        let rows = registry.dispatch_history(10).expect("history");
        assert_eq!(rows[0].prompt, prompt);
    }

    #[test]
    fn fresh_db_has_no_sessions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        let registry = Registry::open(&db_path).expect("open should succeed");
        let all = registry.all().expect("all() should succeed");
        assert!(all.is_empty());
    }

    #[test]
    fn upsert_round_trips_all_fields() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        let mut registry = Registry::open(&db_path).expect("open should succeed");

        let id = registry
            .create(&sample_record(0))
            .expect("create should succeed");
        assert_eq!(id, 1);
        let record = sample_record(id);
        registry.upsert(&record).expect("upsert should succeed");

        let all = registry.all().expect("all() should succeed");
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.id, record.id);
        assert_eq!(got.tool, record.tool);
        assert_eq!(got.native_id, record.native_id);
        assert_eq!(got.name, record.name);
        assert_eq!(got.cwd, record.cwd);
        assert_eq!(got.mode, record.mode);
        assert_eq!(got.status, record.status);
        assert_eq!(got.created_at, record.created_at);
        assert_eq!(got.updated_at, record.updated_at);
        assert_eq!(got.cost_usd, record.cost_usd);
        assert_eq!(got.model, record.model);
        assert_eq!(got.effort, record.effort);
        assert_eq!(got.role, record.role);
    }

    #[test]
    fn upsert_updates_in_place_and_keeps_created_at() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        let mut registry = Registry::open(&db_path).expect("open should succeed");

        let id = registry
            .create(&sample_record(0))
            .expect("create should succeed");
        let mut record = sample_record(id);

        record.status = SessionStatus::Completed;
        record.updated_at = UNIX_EPOCH + Duration::from_secs(9_999);
        registry
            .upsert(&record)
            .expect("second upsert should succeed");

        let all = registry.all().expect("all() should succeed");
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.status, SessionStatus::Completed);
        assert_eq!(got.created_at, UNIX_EPOCH + Duration::from_secs(1_000));
        assert_eq!(got.updated_at, UNIX_EPOCH + Duration::from_secs(9_999));
    }

    #[test]
    fn reopening_the_same_db_sees_persisted_data() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");

        {
            let mut registry = Registry::open(&db_path).expect("open should succeed");
            registry
                .create(&sample_record(0))
                .expect("create should succeed");
        }

        let registry_2 = Registry::open(&db_path).expect("reopen should succeed");
        let all = registry_2.all().expect("all() should succeed");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tool, "claude-code");
    }

    /// F-001: two Registry handles on the same file (as two concurrent
    /// swarm-tui processes would hold) each create a fresh session — the
    /// result must be two distinct rows, never one overwriting the other.
    #[test]
    fn two_handles_creating_fresh_sessions_never_clobber() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        let mut handle_a = Registry::open(&db_path).expect("open handle a");
        let mut handle_b = Registry::open(&db_path).expect("open handle b");

        let mut record_a = sample_record(0);
        record_a.name = Some("from process a".to_string());
        let mut record_b = sample_record(0);
        record_b.name = Some("from process b".to_string());

        let id_a = handle_a.create(&record_a).expect("create via handle a");
        let id_b = handle_b.create(&record_b).expect("create via handle b");
        assert_ne!(id_a, id_b, "fresh sessions must get distinct ids");

        let all = handle_a.all().expect("all()");
        assert_eq!(all.len(), 2, "both sessions must survive");
        let names: Vec<_> = all.iter().filter_map(|r| r.name.as_deref()).collect();
        assert!(names.contains(&"from process a"));
        assert!(names.contains(&"from process b"));
    }

    /// F-004: a pre-epoch clock on the write side clamps to the epoch
    /// instead of panicking.
    #[test]
    fn pre_epoch_system_time_clamps_to_epoch_on_write() {
        let pre_epoch = UNIX_EPOCH - Duration::from_secs(100);
        assert_eq!(system_time_to_unix(pre_epoch), 0);
    }

    /// F-004: negative/overflowing values stored in the database file (a
    /// real boundary — any process can have written it) clamp instead of
    /// panicking.
    #[test]
    fn out_of_range_stored_timestamp_clamps_instead_of_panicking() {
        assert_eq!(unix_to_system_time(-1), UNIX_EPOCH);
        assert_eq!(unix_to_system_time(i64::MIN), UNIX_EPOCH);
        // i64::MAX seconds overflows SystemTime on common platforms; the
        // exact clamped value doesn't matter, not panicking does.
        let _ = unix_to_system_time(i64::MAX);
    }

    /// F-004 end to end: a row with negative timestamps already persisted
    /// in the file must not panic the roster read.
    #[test]
    fn negative_stored_timestamps_read_back_clamped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        {
            let _ = Registry::open(&db_path).expect("create schema");
        }
        {
            let conn = Connection::open(&db_path).expect("raw open");
            conn.execute(
                "INSERT INTO sessions
                    (id, tool, cwd, mode, status, created_at, updated_at)
                 VALUES (1, 'claude-code', '/x', 'interactive', 'running', -5, -5)",
                [],
            )
            .expect("insert row with negative timestamps");
        }

        let registry = Registry::open(&db_path).expect("reopen");
        let all = registry
            .all()
            .expect("all() must not panic on negative stored timestamps");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].created_at, UNIX_EPOCH);
        assert_eq!(all[0].updated_at, UNIX_EPOCH);
    }
}
