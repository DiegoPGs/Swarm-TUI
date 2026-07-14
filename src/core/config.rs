//! swarm-tui's own configuration & state locations.
//!
//! Scope guard: this covers swarm-tui's files ONLY (registry db, logs, its own
//! settings). The wrapped tools' config/auth files are **read-never,
//! write-never** territory for the whole codebase (AGENTS.md boundary); not
//! even path constants for them belong here — path knowledge lives in the
//! integration docs and, where needed at runtime, inside each adapter.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SwarmTuiConfig {
    /// Session registry database (ADR-0002), e.g.
    /// `$XDG_DATA_HOME/swarm-tui/registry.db`.
    pub registry_db: PathBuf,
}

impl SwarmTuiConfig {
    /// TODO(next session): resolve via the `directories` crate + optional
    /// `SWARM_TUI_DATA_DIR` override for tests.
    pub fn resolve() -> Result<Self, String> {
        todo!("resolve XDG paths once the `directories` dep is enabled")
    }
}
