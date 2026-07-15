//! PTY hosting seam (ADR-0003). This is the ONLY module allowed to depend on
//! `portable-pty` / `tui-term` / `vt100` once those land. Everything above it
//! sees `PaneHost` and never a raw PTY.
//!
//! The seam exists because ADR-0003 carries a fallback ladder — if the
//! fidelity spike finds `vt100` can't faithfully render one of the three
//! native TUIs, the swap (vt100 → wezterm-term → tmux control-mode backend)
//! must happen **behind this trait** without touching `app` or `adapters`.
//!
//! FIRST IMPLEMENTATION MILESTONE (before any Home-view work): the fidelity
//! spike — spawn all three real CLIs through a `PaneHost` impl and interact
//! with each. Record verdicts in docs/NOTES.md.

use std::process::Command;

pub mod local;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

#[derive(Debug, Clone, Copy)]
pub struct PaneSize {
    pub rows: u16,
    pub cols: u16,
}

pub trait PaneHost {
    /// Spawn `cmd` (built by an adapter's `interactive_cmd`) into a new PTY.
    fn spawn(&mut self, cmd: Command, size: PaneSize) -> Result<PaneId, PaneError>;

    /// Forward raw user input (keystrokes, paste) to the child.
    fn write_input(&mut self, pane: PaneId, bytes: &[u8]) -> Result<(), PaneError>;

    /// Propagate a terminal resize (tab layout change, window resize).
    fn resize(&mut self, pane: PaneId, size: PaneSize) -> Result<(), PaneError>;

    /// True once the child has exited (tab shows a "finished" state; the
    /// underlying native session, if any, is untouched — sessions outlive
    /// panes).
    fn is_exited(&self, pane: PaneId) -> bool;

    /// Render surface: locks the pane's parser only for the duration of `f`,
    /// so no lock-guard type leaks across the trait boundary (needed because
    /// the parser is mutated by a background reader thread — see
    /// `LocalPaneHost`).
    fn with_screen<R>(
        &self,
        pane: PaneId,
        f: impl FnOnce(&vt100::Screen) -> R,
    ) -> Result<R, PaneError>;

    /// Kill the PTY child. Must never touch anything the native wrapped tool
    /// persisted itself — this only tears down the local PTY process.
    fn kill(&mut self, pane: PaneId) -> Result<(), PaneError>;

    /// `None` while running. `Some(true)` once the child exited with code 0,
    /// `Some(false)` for any other exit (nonzero code, killed, signaled).
    /// Distinct from `is_exited` so a caller can tell "still running" from
    /// "exited, need the reason".
    fn exit_success(&self, pane: PaneId) -> Option<bool>;
}

#[derive(Debug)]
pub enum PaneError {
    Spawn(std::io::Error),
    UnknownPane(PaneId),
}
