//! Application layer: terminal setup, event loop, layout, tab switching.
//!
//! Knows CLIs? **No.** This layer speaks only in `crate::core` vocabulary
//! (sessions, tasks, `AgentEvent`) and `crate::pty::PaneHost` surfaces. If an
//! `if tool == "codex"` ever appears here, the adapter boundary (ADR-0006) has
//! leaked — fix the boundary, not this module.

pub mod home;
pub mod session_view;
pub mod tabs;

use tabs::Tabs;

/// Top-level application state.
pub struct App {
    pub tabs: Tabs,
}

impl App {
    /// TODO(next session, after the ADR-0003 fidelity spike): initialize
    /// ratatui + crossterm, spawn the tokio event loop, reconcile the session
    /// registry (ADR-0002), and enter draw/input cycles.
    pub fn run() -> Result<(), String> {
        todo!("ADR-0003/0005: event loop lands after the pane fidelity spike")
    }
}
