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

    // TODO(next session): the render surface. Deliberately unspecified until
    // the spike — likely `fn screen(&self, pane) -> &vt100::Screen` consumed
    // by a tui-term widget, but that signature is exactly what the spike must
    // validate.
}

#[derive(Debug)]
pub enum PaneError {
    Spawn(std::io::Error),
    UnknownPane(PaneId),
}
