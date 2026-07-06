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

use crate::core::session::SessionRecord;

pub struct Registry {
    // TODO(next session): rusqlite::Connection (bundled feature; version
    // pinned-but-commented in Cargo.toml). Schema v1 = sessions table
    // mirroring SessionRecord + dispatches table (task prompt, target,
    // outcome, cost) + schema_version pragma.
}

impl Registry {
    pub fn open(_db_path: &Path) -> Result<Self, StoreError> {
        todo!("open/create sqlite db + run migrations (ADR-0002)")
    }

    pub fn upsert(&mut self, _record: &SessionRecord) -> Result<(), StoreError> {
        todo!()
    }

    pub fn all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        todo!()
    }

    /// Mark rows whose native sessions vanished. Takes the *found* native ids
    /// per tool so the store stays ignorant of how discovery works.
    pub fn mark_orphans(
        &mut self,
        _tool_slug: &str,
        _live_native_ids: &[String],
    ) -> Result<usize, StoreError> {
        todo!()
    }
}

#[derive(Debug)]
pub enum StoreError {
    Open(String),
    Query(String),
}
