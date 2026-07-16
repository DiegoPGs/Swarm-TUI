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

/// Schema v2 (v1 + `model`/`effort` launch options, ADR-0009). The new
/// columns come **last** in the CREATE so fresh-v2 and migrated-v1 databases
/// have identical column order.
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
    model TEXT, effort TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_tool_native ON sessions(tool, native_id);
-- Unused this milestone (dispatch()/follow_up() adapter methods stay todo!() elsewhere
-- in the codebase); exists now so a later milestone doesn't need a schema migration for it.
CREATE TABLE IF NOT EXISTS dispatches (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id INTEGER REFERENCES sessions(id),
    tool TEXT NOT NULL, prompt TEXT NOT NULL, cwd TEXT NOT NULL,
    started_at INTEGER NOT NULL, finished_at INTEGER, outcome TEXT, cost_usd REAL
);
";

const SCHEMA_VERSION: i64 = 2;

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

    /// Hand back a fresh, unused session id.
    ///
    /// `upsert` can't allocate one for us because `SessionRecord.id: u64` is
    /// not `Option` — callers need an id *before* they can construct the
    /// record they're about to persist. `MAX(id) + 1` is fine for a local,
    /// single-writer app; there's no concurrent-writer race worth guarding
    /// against here.
    pub fn allocate_id(&self) -> Result<u64, StoreError> {
        let next: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM sessions", [], |row| {
                row.get(0)
            })
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(next as u64)
    }

    pub fn upsert(&mut self, record: &SessionRecord) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO sessions
                    (id, tool, native_id, name, cwd, mode, status, created_at, updated_at,
                     cost_usd, model, effort)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
                    effort=excluded.effort",
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
                        cost_usd, model, effort
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
                Ok((
                    id, tool, native_id, name, cwd, mode, status, created_at, updated_at, cost_usd,
                    model, effort,
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
            });
        }
        Ok(records)
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

fn system_time_to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn unix_to_system_time(v: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(v as u64)
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

    #[test]
    fn v1_db_migrates_to_v2_preserving_rows() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        make_v1_db(&db_path);

        let registry = Registry::open(&db_path).expect("open should migrate v1 → v2");

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
        // Pre-v2 rows have no launch options.
        assert_eq!(got.model, None);
        assert_eq!(got.effort, None);
    }

    #[test]
    fn migrated_db_round_trips_launch_options_and_is_idempotent_on_reopen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        make_v1_db(&db_path);

        {
            let mut registry = Registry::open(&db_path).expect("open should migrate");
            let id = registry.allocate_id().expect("allocate_id");
            registry
                .upsert(&sample_record(id))
                .expect("upsert with model/effort should succeed post-migration");
        }

        // Second open: version is already 2 — must be a no-op, not a re-run.
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
    fn fresh_db_is_created_at_v2_directly() {
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

        let id = registry.allocate_id().expect("allocate_id should succeed");
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
    }

    #[test]
    fn upsert_updates_in_place_and_keeps_created_at() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("registry.db");
        let mut registry = Registry::open(&db_path).expect("open should succeed");

        let id = registry.allocate_id().expect("allocate_id should succeed");
        let mut record = sample_record(id);
        registry
            .upsert(&record)
            .expect("first upsert should succeed");

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
            let id = registry.allocate_id().expect("allocate_id should succeed");
            let record = sample_record(id);
            registry.upsert(&record).expect("upsert should succeed");
        }

        let registry_2 = Registry::open(&db_path).expect("reopen should succeed");
        let all = registry_2.all().expect("all() should succeed");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tool, "claude-code");
    }
}
