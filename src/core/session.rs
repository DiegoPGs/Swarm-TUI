//! Session records: what the thin registry stores (ADR-0002).
//!
//! One row per session swarm-tui knows about. The natives own their transcripts
//! and state; we store only the *mapping* needed to find, resume, and display
//! them. Deleting a row never deletes native state (orphan-mark, don't destroy).

use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// swarm-tui-local id (registry primary key).
    pub id: u64,
    /// Stable adapter slug: "claude-code" | "antigravity" | "codex"
    /// (`adapters::AdapterKind::id()`). Kept as a string because that is what
    /// the registry persists; promote to a shared enum if string-typing chafes.
    pub tool: String,
    /// The tool's own session/conversation/thread id, once known. `None` for a
    /// just-spawned agy headless run until backfill (ADR-0002).
    pub native_id: Option<String>,
    /// Human label (claude `--name`, agy `/rename`, or swarm-tui-only).
    pub name: Option<String>,
    /// Working directory the session was started in. Load-bearing: claude's
    /// `--resume <id>` lookup is scoped to this cwd (+ its worktrees).
    pub cwd: PathBuf,
    pub mode: SessionMode,
    pub status: SessionStatus,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Cumulative reported cost, where the tool reports one (claude).
    pub cost_usd: Option<f64>,
    /// Launch options the session was spawned with (ADR-0009, registry schema
    /// v2). `None` for sessions predating v2 or launched with tool defaults.
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// Lives in a tab; the native TUI is the interface.
    Interactive,
    /// Dispatched from Home; interface is the normalized event stream.
    Headless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    /// Registry row whose native session could not be found at reconciliation
    /// (deleted upstream, cwd moved, tool updated). Kept visible, marked stale.
    Orphaned,
}
