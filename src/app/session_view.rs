//! Session tab view: renders one embedded CLI's PTY surface at full fidelity.
//!
//! This module owns *presentation only*: it asks `crate::pty::PaneHost` for the
//! current screen and forwards keystrokes. It must never parse or reinterpret
//! the tool's output — the native TUI is the UX (ADR-0001, ADR-0003).
//!
//! Failure-mode note (ARCHITECTURE.md): if the underlying native session is
//! already attached elsewhere (e.g. a real terminal outside swarm-tui), do not
//! fight over the PTY — offer a fork instead (Claude `--fork-session`,
//! Codex `fork`, agy `/fork`). Not exercised yet in this milestone.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use crate::app::tabs::SessionId;
use crate::pty::local::LocalPaneHost;
use crate::pty::{PaneHost, PaneId};

pub struct SessionView {
    pub session_id: SessionId,
    pub pane_id: PaneId,
}

impl SessionView {
    pub fn new(session_id: SessionId, pane_id: PaneId) -> Self {
        SessionView {
            session_id,
            pane_id,
        }
    }
}

/// Render one session tab: the live PTY grid (or an "exited" placeholder)
/// plus a one-line status bar. `status_line` is built by the caller (`App`),
/// which is the one place that has both the session record and the current
/// `InputMode` to summarize.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    pane_host: &LocalPaneHost,
    pane_id: PaneId,
    status_line: &str,
) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let (pane_area, status_area) = (chunks[0], chunks[1]);

    if pane_host.is_exited(pane_id) {
        frame.render_widget(
            Paragraph::new("[exited — prefix+x to close]")
                .style(Style::default().fg(Color::DarkGray)),
            pane_area,
        );
    } else {
        let _ = pane_host.with_screen(pane_id, |screen| {
            let widget = PseudoTerminal::new(screen);
            frame.render_widget(widget, pane_area);
        });
    }

    frame.render_widget(
        Paragraph::new(status_line).style(Style::default().fg(Color::Gray)),
        status_area,
    );
}
