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
    /// Resolve swarm-tui's own data directory via XDG (through the
    /// `directories` crate), honoring a `SWARM_TUI_DATA_DIR` override.
    pub fn resolve() -> Result<Self, String> {
        let override_dir = std::env::var("SWARM_TUI_DATA_DIR").ok();
        Self::resolve_from(override_dir.as_deref())
    }

    /// Same as [`Self::resolve`] but takes the override directly instead of
    /// reading the environment, so tests never need to mutate real process
    /// env (which is flaky under parallel `cargo test`).
    fn resolve_from(override_dir: Option<&str>) -> Result<Self, String> {
        let data_dir: PathBuf = match override_dir {
            Some(dir) => PathBuf::from(dir),
            None => directories::ProjectDirs::from("", "", "swarm-tui")
                .ok_or_else(|| "could not resolve a home directory for XDG paths".to_string())?
                .data_dir()
                .to_path_buf(),
        };
        Ok(SwarmTuiConfig {
            registry_db: data_dir.join("registry.db"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_from_override_joins_registry_db() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = SwarmTuiConfig::resolve_from(Some(tmp.path().to_str().expect("utf8 path")))
            .expect("resolve_from should succeed with an explicit override");
        assert_eq!(cfg.registry_db, tmp.path().join("registry.db"));
    }
}
