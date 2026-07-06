//! Session tab view: renders one embedded CLI's PTY surface at full fidelity.
//!
//! This module owns *presentation only*: it asks `crate::pty::PaneHost` for the
//! current screen and forwards keystrokes. It must never parse or reinterpret
//! the tool's output — the native TUI is the UX (ADR-0001, ADR-0003).
//!
//! Failure-mode note (ARCHITECTURE.md): if the underlying native session is
//! already attached elsewhere (e.g. a real terminal outside swarm-tui), do not
//! fight over the PTY — offer a fork instead (Claude `--fork-session`,
//! Codex `fork`, agy `/fork`).

use crate::app::tabs::SessionId;

pub struct SessionView {
    pub session_id: SessionId,
    // TODO(next session): pane: crate::pty::PaneId + scrollback controls.
}
