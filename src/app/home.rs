//! Home view: the cross-agent surface (docs/PRODUCT.md, ARCHITECTURE task flow).
//!
//! Three panels, in priority order:
//! 1. **Roster** — every known session across all three tools, live + orphaned,
//!    including Claude Code's native background sessions discovered via
//!    reconciliation (`claude agents --json`, ADR-0002). Actions: promote to
//!    tab, follow-up dispatch, stop, forget.
//! 2. **Dispatch** — prompt + target tool + cwd (cwd is a property of the task,
//!    not of the app) + guardrail preset (ARCHITECTURE guardrail table).
//! 3. **Broadcast compare** — same prompt to N tools, normalized `AgentEvent`
//!    streams rendered side by side; agy joins only when opted in (quota).
//!
//! This milestone (Stage C) only builds panel 1 (roster) plus the row
//! navigation/re-attach action; dispatch and broadcast compare land in a
//! later milestone.

use std::collections::HashMap;
use std::collections::HashSet;
use std::time::SystemTime;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::app::tabs::{SessionId, Tab, Tabs};
use crate::core::session::SessionRecord;
use crate::pty::PaneId;

/// One row in the Home roster.
pub enum RosterEntry {
    /// A row swarm-tui's own registry knows about.
    Registered(SessionRecord),
    /// Stage D output: discovered via `claude agents --json --all`, not yet a
    /// registry row (no invented id). Nothing in this milestone produces
    /// this variant yet — it exists so Stage D doesn't need a `RosterEntry`
    /// schema change.
    #[allow(dead_code)]
    ReconciledOnly {
        tool: &'static str,
        native_id: String,
        name: Option<String>,
        status_hint: String,
    },
}

// ---------------------------------------------------------------------------
// State model — keep this section ratatui-free so it stays unit-testable
// headlessly, per this module's original design note.
// ---------------------------------------------------------------------------

pub struct HomeView {
    pub roster: Vec<RosterEntry>,
    /// Index of the selected roster row (Home-tab-local navigation, a
    /// separate input scope from the global prefix-key table — ADR-0007).
    pub selected: usize,
}

impl HomeView {
    pub fn new(roster: Vec<RosterEntry>) -> Self {
        HomeView {
            roster,
            selected: 0,
        }
    }

    pub fn select_next(&mut self) {
        if self.roster.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.roster.len();
    }

    pub fn select_prev(&mut self) {
        if self.roster.is_empty() {
            return;
        }
        self.selected = (self.selected + self.roster.len() - 1) % self.roster.len();
    }

    /// The registry session id under the cursor, if the selected row is a
    /// `Registered` entry (a `ReconciledOnly` row has no swarm-tui id to
    /// re-attach to — Stage D's problem to solve).
    pub fn selected_session_id(&self) -> Option<SessionId> {
        match self.roster.get(self.selected)? {
            RosterEntry::Registered(record) => Some(record.id),
            RosterEntry::ReconciledOnly { .. } => None,
        }
    }
}

/// Whether a registered session is running with a live pane but no tab
/// currently pointing at it. Derived, never stored: comparing the live pane
/// map against the open tab list at call time is the source of truth.
pub fn is_detached(
    session_id: SessionId,
    pane_of_session: &HashMap<SessionId, PaneId>,
    tabs: &Tabs,
) -> bool {
    if !pane_of_session.contains_key(&session_id) {
        return false;
    }
    !tabs
        .items
        .iter()
        .any(|t| matches!(t, Tab::Session { session_id: sid } if *sid == session_id))
}

fn format_age(updated_at: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(updated_at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d", secs / 86400)
    }
}

// ---------------------------------------------------------------------------
// Rendering — ratatui-only. Kept below the state model on purpose so the
// state/render split stays obvious at a glance.
// ---------------------------------------------------------------------------

/// `detached` is precomputed by the caller (`is_detached` against the live
/// `pane_of_session`/`Tabs` state) so this function never needs those types.
pub fn render_home(frame: &mut Frame, area: Rect, home: &HomeView, detached: &HashSet<SessionId>) {
    let header = Row::new(vec![
        "Tool", "Name", "Role", "Status", "Model", "Effort", "Cwd", "Age",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = home
        .roster
        .iter()
        .map(|entry| match entry {
            RosterEntry::Registered(record) => {
                let name = record
                    .name
                    .clone()
                    .or_else(|| record.native_id.clone())
                    .unwrap_or_else(|| format!("#{}", record.id));
                let mut status = format!("{:?}", record.status);
                if detached.contains(&record.id) {
                    status.push_str(" [detached]");
                }
                Row::new(vec![
                    Cell::from(record.tool.clone()),
                    Cell::from(name),
                    Cell::from(record.role.clone().unwrap_or_else(|| "-".to_string())),
                    Cell::from(status),
                    Cell::from(record.model.clone().unwrap_or_else(|| "-".to_string())),
                    Cell::from(record.effort.clone().unwrap_or_else(|| "-".to_string())),
                    Cell::from(record.cwd.display().to_string()),
                    Cell::from(format_age(record.updated_at)),
                ])
            }
            RosterEntry::ReconciledOnly {
                tool,
                native_id,
                name,
                status_hint,
            } => Row::new(vec![
                Cell::from(*tool),
                Cell::from(name.clone().unwrap_or_else(|| native_id.clone())),
                Cell::from("-"),
                Cell::from(status_hint.clone()),
                Cell::from("-"),
                Cell::from("-"),
                Cell::from("-"),
                Cell::from("-"),
            ]),
        })
        .collect();

    let empty = rows.is_empty();
    // Status stays 18 wide: "Running [detached]" is exactly 18 chars and the
    // badge must never truncate (the smoke harness keys on it too). Tool/Name
    // gave up a few columns to the Role column (both tool slugs are 11 chars;
    // the dogfood role names top out at 10).
    let widths = [
        ratatui::layout::Constraint::Length(11),
        ratatui::layout::Constraint::Length(14),
        ratatui::layout::Constraint::Length(10),
        ratatui::layout::Constraint::Length(18),
        ratatui::layout::Constraint::Length(10),
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Min(10),
        ratatui::layout::Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Home — roster"),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    if empty {
        frame.render_widget(table, area);
        let hint_area = Rect {
            x: area.x + 2,
            y: area.y + 2,
            width: area.width.saturating_sub(4),
            height: 1,
        };
        if area.height > 3 {
            frame.render_widget(
                Text::raw("No sessions yet — prefix+c to start one."),
                hint_area,
            );
        }
        return;
    }

    let mut state = TableState::default().with_selected(Some(home.selected));
    frame.render_stateful_widget(table, area, &mut state);
}
