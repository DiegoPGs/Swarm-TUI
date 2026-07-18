//! Headless dispatch UI state (ADR-0013): the Home-local dispatch and
//! broadcast forms, the running-dispatch bookkeeping the app folds events
//! into, the timeline panel, and the broadcast compare surface. Knows tools
//! only as `AdapterKind` + data (ADR-0006): every flag decision lives behind
//! `CliAdapter::dispatch`.

use std::collections::VecDeque;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::adapters::{self, AdapterKind, CliAdapter, DispatchHandle};
use crate::app::tabs::SessionId;
use crate::core::events::AgentEvent;
use crate::core::plan::SwarmPlan;
use crate::core::session::SessionStatus;
use crate::core::task::{Budget, DispatchPosture, Task};

/// One live (or just-finished) headless dispatch the app is polling.
/// Deliberately lean — it grows per milestone-3 stage as surfaces need more.
pub struct RunningDispatch {
    pub handle: DispatchHandle,
    pub kind: AdapterKind,
    /// Terminal event seen (the entry stays for history; the channel is inert).
    pub done: bool,
    /// `dispatches` table row to finalize; `None` if the insert failed.
    pub dispatch_row: Option<u64>,
}

/// One selectable dispatch target: a swarm-plan role (carrying its presets)
/// or a raw active tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: AdapterKind,
    /// Set when the target is a role — recorded on the session row.
    pub role: Option<String>,
    pub label: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Probe succeeded; disabled targets render greyed and refuse submit.
    pub enabled: bool,
}

/// Roles (alphabetical) above raw tools (registry order) — the same shape as
/// the new-session picker's list, in dispatch vocabulary.
pub fn targets(plan: Option<&SwarmPlan>, probe_ok: impl Fn(AdapterKind) -> bool) -> Vec<Target> {
    let mut out = Vec::new();
    if let Some(plan) = plan {
        for (name, role) in &plan.roles {
            let Some(kind) = AdapterKind::from_slug(&role.tool) else {
                continue;
            };
            out.push(Target {
                kind,
                role: Some(name.clone()),
                label: format!("{name} — {}", role.tool),
                model: role.model.clone(),
                effort: role.effort.clone(),
                enabled: probe_ok(kind),
            });
        }
    }
    for &kind in adapters::registry() {
        out.push(Target {
            kind,
            role: None,
            label: kind.display_name().to_string(),
            model: None,
            effort: None,
            enabled: probe_ok(kind),
        });
    }
    out
}

const POSTURES: &[(&str, DispatchPosture)] = &[
    ("read_only", DispatchPosture::ReadOnly),
    ("plan", DispatchPosture::Plan),
    ("edits", DispatchPosture::Edits),
];

/// Focusable form fields, in navigation order.
const FIELD_PROMPT: usize = 0;
const FIELD_TARGET: usize = 1;
const FIELD_CWD: usize = 2;
const FIELD_POSTURE: usize = 3;
const FIELD_TURNS: usize = 4;
const FIELD_USD: usize = 5;
const FIELD_TIMEOUT: usize = 6;
const FIELD_COUNT: usize = 7;

/// The Home-local dispatch form (`i`). Pre-filled from the workspace's
/// `defaults.dispatch` (ADR-0012) via the router's resolved `Budget`; every
/// edit here is the per-task override layer (ADR-0013 precedence).
pub struct DispatchForm {
    pub targets: Vec<Target>,
    pub target_idx: usize,
    pub prompt: String,
    pub cwd: String,
    pub posture_idx: usize,
    pub max_turns: String,
    pub max_usd: String,
    pub timeout_secs: String,
    pub focus: usize,
    pub error: Option<String>,
}

impl DispatchForm {
    pub fn new(
        targets: Vec<Target>,
        preselect_role: Option<&str>,
        budget: Budget,
        cwd: String,
    ) -> Self {
        let target_idx = preselect_role
            .and_then(|want| {
                targets
                    .iter()
                    .position(|t| t.enabled && t.role.as_deref() == Some(want))
            })
            .or_else(|| targets.iter().position(|t| t.enabled))
            .unwrap_or(0);
        let posture_idx = POSTURES
            .iter()
            .position(|(_, p)| *p == budget.posture)
            .unwrap_or(1);
        DispatchForm {
            targets,
            target_idx,
            prompt: String::new(),
            cwd,
            posture_idx,
            max_turns: budget.max_turns.map(|v| v.to_string()).unwrap_or_default(),
            max_usd: budget.max_usd.map(|v| v.to_string()).unwrap_or_default(),
            timeout_secs: budget
                .timeout_secs
                .map(|v| v.to_string())
                .unwrap_or_default(),
            focus: FIELD_PROMPT,
            error: None,
        }
    }

    /// Field navigation and edits (Enter/Esc are the caller's — they close
    /// the form). Unknown keys are ignored.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Tab => self.focus = (self.focus + 1) % FIELD_COUNT,
            KeyCode::Up | KeyCode::BackTab => {
                self.focus = (self.focus + FIELD_COUNT - 1) % FIELD_COUNT
            }
            KeyCode::Left | KeyCode::Right => {
                let forward = key.code == KeyCode::Right;
                match self.focus {
                    FIELD_TARGET if !self.targets.is_empty() => {
                        let len = self.targets.len();
                        self.target_idx = if forward {
                            (self.target_idx + 1) % len
                        } else {
                            (self.target_idx + len - 1) % len
                        };
                    }
                    FIELD_POSTURE => {
                        let len = POSTURES.len();
                        self.posture_idx = if forward {
                            (self.posture_idx + 1) % len
                        } else {
                            (self.posture_idx + len - 1) % len
                        };
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) => {
                if let Some(field) = self.text_field_mut() {
                    field.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(field) = self.text_field_mut() {
                    field.pop();
                }
            }
            _ => {}
        }
    }

    fn text_field_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            FIELD_PROMPT => Some(&mut self.prompt),
            FIELD_CWD => Some(&mut self.cwd),
            FIELD_TURNS => Some(&mut self.max_turns),
            FIELD_USD => Some(&mut self.max_usd),
            FIELD_TIMEOUT => Some(&mut self.timeout_secs),
            _ => None,
        }
    }

    /// Validate and build the dispatch request. `Err` is the one-line form
    /// error; the form stays open.
    pub fn submit(&self) -> Result<(Target, Task), String> {
        let target = self
            .targets
            .get(self.target_idx)
            .ok_or_else(|| "no dispatch target".to_string())?;
        if !target.enabled {
            return Err(format!("{} is not installed", target.label));
        }
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err("prompt is empty".to_string());
        }
        let cwd = PathBuf::from(self.cwd.trim());
        if !cwd.is_dir() {
            return Err(format!("cwd is not a directory: {}", cwd.display()));
        }
        let budget = parse_budget(
            self.posture_idx,
            &self.max_turns,
            &self.max_usd,
            &self.timeout_secs,
        )?;
        let task = Task {
            prompt: prompt.to_string(),
            cwd,
            budget,
            model: target.model.clone(),
            effort: target.effort.clone(),
        };
        Ok((target.clone(), task))
    }
}

/// Validate the shared budget fields of either form into a `Budget`.
fn parse_budget(
    posture_idx: usize,
    max_turns: &str,
    max_usd: &str,
    timeout_secs: &str,
) -> Result<Budget, String> {
    let max_turns = parse_optional(max_turns, "turns must be a whole number", |s| {
        s.parse::<u32>().ok().filter(|v| *v > 0)
    })?;
    let max_usd = parse_optional(max_usd, "budget must be a positive number", |s| {
        s.parse::<f64>().ok().filter(|v| *v > 0.0 && v.is_finite())
    })?;
    let timeout_secs = parse_optional(timeout_secs, "timeout must be whole seconds", |s| {
        s.parse::<u64>().ok().filter(|v| *v > 0)
    })?;
    Ok(Budget {
        posture: POSTURES[posture_idx].1,
        max_turns,
        max_usd,
        timeout_secs,
    })
}

fn parse_optional<T>(
    raw: &str,
    err: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    parse(raw).map(Some).ok_or_else(|| err.to_string())
}

// ---------------------------------------------------------------------------
// Broadcast (ADR-0013 decision 5, second half)
// ---------------------------------------------------------------------------

/// Rolling text tail kept per compare column — smaller than the 200-line
/// global timeline; the panel shows only a handful of rows anyway.
const COLUMN_TAIL_CAP: usize = 100;

/// The Home-local broadcast form (`b`): one prompt, many ticked targets.
/// Preselection is the workspace's `defaults.broadcast` role names — nothing
/// is broadcast-targeted unless named there or ticked per task (ADR-0012's
/// PRODUCT Q5 answer; the quota-shared tools start unticked like the rest).
#[derive(Debug)]
pub struct BroadcastForm {
    pub targets: Vec<Target>,
    /// Parallel to `targets`; Space toggles the cursor row (enabled only).
    pub ticked: Vec<bool>,
    pub target_cursor: usize,
    pub prompt: String,
    pub cwd: String,
    pub posture_idx: usize,
    pub max_turns: String,
    pub max_usd: String,
    pub timeout_secs: String,
    pub focus: usize,
    pub error: Option<String>,
}

impl BroadcastForm {
    pub fn new(
        targets: Vec<Target>,
        preticked_roles: &[String],
        budget: Budget,
        cwd: String,
    ) -> Self {
        let ticked = targets
            .iter()
            .map(|t| {
                t.enabled
                    && t.role
                        .as_deref()
                        .is_some_and(|name| preticked_roles.iter().any(|want| want == name))
            })
            .collect();
        let posture_idx = POSTURES
            .iter()
            .position(|(_, p)| *p == budget.posture)
            .unwrap_or(1);
        BroadcastForm {
            targets,
            ticked,
            target_cursor: 0,
            prompt: String::new(),
            cwd,
            posture_idx,
            max_turns: budget.max_turns.map(|v| v.to_string()).unwrap_or_default(),
            max_usd: budget.max_usd.map(|v| v.to_string()).unwrap_or_default(),
            timeout_secs: budget
                .timeout_secs
                .map(|v| v.to_string())
                .unwrap_or_default(),
            focus: FIELD_PROMPT,
            error: None,
        }
    }

    /// Field navigation, edits, and target ticking (Enter/Esc are the
    /// caller's). Space toggles the cursor row while the target list has
    /// focus; everywhere else it types a space.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Tab => self.focus = (self.focus + 1) % FIELD_COUNT,
            KeyCode::Up | KeyCode::BackTab => {
                self.focus = (self.focus + FIELD_COUNT - 1) % FIELD_COUNT
            }
            KeyCode::Left | KeyCode::Right => {
                let forward = key.code == KeyCode::Right;
                match self.focus {
                    FIELD_TARGET if !self.targets.is_empty() => {
                        let len = self.targets.len();
                        self.target_cursor = if forward {
                            (self.target_cursor + 1) % len
                        } else {
                            (self.target_cursor + len - 1) % len
                        };
                    }
                    FIELD_POSTURE => {
                        let len = POSTURES.len();
                        self.posture_idx = if forward {
                            (self.posture_idx + 1) % len
                        } else {
                            (self.posture_idx + len - 1) % len
                        };
                    }
                    _ => {}
                }
            }
            KeyCode::Char(' ') if self.focus == FIELD_TARGET => {
                if let (Some(target), Some(tick)) = (
                    self.targets.get(self.target_cursor),
                    self.ticked.get_mut(self.target_cursor),
                ) {
                    if target.enabled {
                        *tick = !*tick;
                    }
                }
            }
            KeyCode::Char(c) => {
                if let Some(field) = self.text_field_mut() {
                    field.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(field) = self.text_field_mut() {
                    field.pop();
                }
            }
            _ => {}
        }
    }

    fn text_field_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            FIELD_PROMPT => Some(&mut self.prompt),
            FIELD_CWD => Some(&mut self.cwd),
            FIELD_TURNS => Some(&mut self.max_turns),
            FIELD_USD => Some(&mut self.max_usd),
            FIELD_TIMEOUT => Some(&mut self.timeout_secs),
            _ => None,
        }
    }

    /// Validate and build one dispatch request per ticked target: shared
    /// prompt/cwd/budget, per-target model/effort presets. `Err` is the
    /// one-line form error; the form stays open.
    pub fn submit(&self) -> Result<Vec<(Target, Task)>, String> {
        let picked: Vec<&Target> = self
            .targets
            .iter()
            .zip(&self.ticked)
            .filter(|(t, ticked)| **ticked && t.enabled)
            .map(|(t, _)| t)
            .collect();
        if picked.is_empty() {
            return Err("tick at least one target".to_string());
        }
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err("prompt is empty".to_string());
        }
        let cwd = PathBuf::from(self.cwd.trim());
        if !cwd.is_dir() {
            return Err(format!("cwd is not a directory: {}", cwd.display()));
        }
        let budget = parse_budget(
            self.posture_idx,
            &self.max_turns,
            &self.max_usd,
            &self.timeout_secs,
        )?;
        Ok(picked
            .into_iter()
            .map(|target| {
                let task = Task {
                    prompt: prompt.to_string(),
                    cwd: cwd.clone(),
                    budget,
                    model: target.model.clone(),
                    effort: target.effort.clone(),
                };
                (target.clone(), task)
            })
            .collect())
    }
}

/// One target's column on the compare surface: status, cost, and a rolling
/// text tail — presentation state only, never persisted (the registry and
/// `dispatches` rows carry the durable record).
#[derive(Debug)]
pub struct BroadcastColumn {
    pub session_id: SessionId,
    pub label: String,
    pub status: SessionStatus,
    pub cost_usd: Option<f64>,
    pub tail: VecDeque<String>,
}

impl BroadcastColumn {
    pub fn new(session_id: SessionId, label: String) -> Self {
        BroadcastColumn {
            session_id,
            label,
            status: SessionStatus::Running,
            cost_usd: None,
            tail: VecDeque::new(),
        }
    }

    fn push_tail(&mut self, line: String) {
        self.tail.push_back(line);
        while self.tail.len() > COLUMN_TAIL_CAP {
            self.tail.pop_front();
        }
    }
}

/// The active broadcast's side-by-side compare state. Events fold in from
/// `drive_dispatches` alongside (never instead of) the registry/timeline
/// fold; sessions not in this group are ignored.
#[derive(Debug)]
pub struct BroadcastGroup {
    pub columns: Vec<BroadcastColumn>,
}

impl BroadcastGroup {
    pub fn fold(&mut self, session_id: SessionId, event: &AgentEvent) {
        let Some(col) = self.columns.iter_mut().find(|c| c.session_id == session_id) else {
            return;
        };
        match event {
            AgentEvent::Started { .. } => col.push_tail("· started".to_string()),
            AgentEvent::AgentText(text) => {
                for line in text.lines().filter(|l| !l.trim().is_empty()) {
                    col.push_tail(line.to_string());
                }
            }
            AgentEvent::ToolActivity(tool) => col.push_tail(format!("[{tool}]")),
            // The final text already streamed through AgentText for both
            // active adapters — status + cost is the terminal delta here.
            AgentEvent::Completed { cost_usd, .. } => {
                col.status = SessionStatus::Completed;
                col.cost_usd = *cost_usd;
            }
            AgentEvent::Failed { reason } => {
                col.status = SessionStatus::Failed;
                col.push_tail(format!("✗ {}", reason.lines().next().unwrap_or("")));
            }
        }
    }
}

fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "running",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
        SessionStatus::Orphaned => "orphaned",
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn popup_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area.width * percent_x / 100;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height: height.min(area.height),
    }
}

pub fn render_form(frame: &mut Frame, area: Rect, form: &DispatchForm) {
    let popup = popup_rect(72, 12, area);
    frame.render_widget(Clear, popup);

    let field = |idx: usize, label: &str, value: String| -> Line<'static> {
        let marker = if form.focus == idx { "▸ " } else { "  " };
        let mut line = Line::from(format!("{marker}{label}{value}"));
        if form.focus == idx {
            line = line.style(Style::default().add_modifier(Modifier::BOLD));
        }
        line
    };

    let target_label = form
        .targets
        .get(form.target_idx)
        .map(|t| {
            let grey = if t.enabled { "" } else { " (not installed)" };
            format!("◂ {}{grey} ▸", t.label)
        })
        .unwrap_or_else(|| "—".to_string());

    let mut lines = vec![
        field(FIELD_PROMPT, "Prompt:   ", format!("{}▏", form.prompt)),
        field(FIELD_TARGET, "Target:   ", target_label),
        field(FIELD_CWD, "Cwd:      ", format!("{}▏", form.cwd)),
        field(
            FIELD_POSTURE,
            "Posture:  ",
            format!("◂ {} ▸", POSTURES[form.posture_idx].0),
        ),
        field(FIELD_TURNS, "Turns:    ", format!("{}▏", form.max_turns)),
        field(FIELD_USD, "Budget $: ", format!("{}▏", form.max_usd)),
        field(
            FIELD_TIMEOUT,
            "Timeout s:",
            format!("{}▏", form.timeout_secs),
        ),
    ];
    if let Some(err) = &form.error {
        lines.push(Line::from(err.clone()).style(Style::default().fg(Color::Red)));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Dispatch — Enter run · Esc cancel · Tab/↑↓ field · ◂▸ choose");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

pub fn render_broadcast_form(frame: &mut Frame, area: Rect, form: &BroadcastForm) {
    let height = (13 + form.targets.len() as u16).min(area.height);
    let popup = popup_rect(72, height, area);
    frame.render_widget(Clear, popup);

    let field = |idx: usize, label: &str, value: String| -> Line<'static> {
        let marker = if form.focus == idx { "▸ " } else { "  " };
        let mut line = Line::from(format!("{marker}{label}{value}"));
        if form.focus == idx {
            line = line.style(Style::default().add_modifier(Modifier::BOLD));
        }
        line
    };

    let mut lines = vec![
        field(FIELD_PROMPT, "Prompt:   ", format!("{}▏", form.prompt)),
        field(FIELD_TARGET, "Targets:  ", String::new()),
    ];
    for (i, target) in form.targets.iter().enumerate() {
        let tick = if form.ticked.get(i).copied().unwrap_or(false) {
            "[x]"
        } else {
            "[ ]"
        };
        let cursor = if form.focus == FIELD_TARGET && form.target_cursor == i {
            "▸"
        } else {
            " "
        };
        let grey = if target.enabled {
            ""
        } else {
            " (not installed)"
        };
        let mut line = Line::from(format!("   {cursor} {tick} {}{grey}", target.label));
        if !target.enabled {
            line = line.style(Style::default().fg(Color::DarkGray));
        }
        lines.push(line);
    }
    lines.extend([
        field(FIELD_CWD, "Cwd:      ", format!("{}▏", form.cwd)),
        field(
            FIELD_POSTURE,
            "Posture:  ",
            format!("◂ {} ▸", POSTURES[form.posture_idx].0),
        ),
        field(FIELD_TURNS, "Turns:    ", format!("{}▏", form.max_turns)),
        field(FIELD_USD, "Budget $: ", format!("{}▏", form.max_usd)),
        field(
            FIELD_TIMEOUT,
            "Timeout s:",
            format!("{}▏", form.timeout_secs),
        ),
    ]);
    if let Some(err) = &form.error {
        lines.push(Line::from(err.clone()).style(Style::default().fg(Color::Red)));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Broadcast — Enter run · Esc cancel · Space tick · Tab/↑↓ field · ◂▸ choose");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// The side-by-side compare surface (ADR-0013): one column per broadcast
/// target — status, cost, rolling text tail, newest at the bottom.
pub fn render_compare(frame: &mut Frame, area: Rect, group: &BroadcastGroup) {
    if group.columns.is_empty() {
        return;
    }
    let n = group.columns.len() as u32;
    let slots = Layout::horizontal(vec![Constraint::Ratio(1, n); n as usize]).split(area);
    for (col, slot) in group.columns.iter().zip(slots.iter()) {
        let title = format!(
            "#{} {} — {}",
            col.session_id,
            col.label,
            status_label(col.status)
        );
        let cost = match col.cost_usd {
            Some(c) => format!("cost ${c:.2}"),
            None => "cost —".to_string(),
        };
        let mut lines = vec![Line::from(cost).style(Style::default().fg(Color::DarkGray))];
        let capacity = slot.height.saturating_sub(3) as usize;
        lines.extend(
            col.tail
                .iter()
                .rev()
                .take(capacity)
                .rev()
                .map(|l| Line::from(l.clone())),
        );
        let block = Block::default().borders(Borders::ALL).title(title);
        frame.render_widget(Paragraph::new(lines).block(block), *slot);
    }
}

/// The Home timeline panel: recent dispatch activity, newest at the bottom.
pub fn render_timeline(frame: &mut Frame, area: Rect, timeline: &VecDeque<String>) {
    let capacity = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = timeline
        .iter()
        .rev()
        .take(capacity)
        .rev()
        .map(|line| ListItem::new(line.clone()))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Dispatches — recent activity");
    frame.render_widget(List::new(items).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::core::plan::{Defaults, Role};
    use crossterm::event::KeyModifiers;

    fn plan_with_roles() -> SwarmPlan {
        let mk = |tool: &str, model: Option<&str>| Role {
            tool: tool.to_string(),
            model: model.map(String::from),
            effort: None,
            purpose: None,
            startup_commands: vec![],
        };
        let mut roles = BTreeMap::new();
        roles.insert("coder".to_string(), mk("claude-code", Some("opus-4.8")));
        roles.insert("researcher".to_string(), mk("antigravity", None));
        SwarmPlan {
            roles,
            defaults: Defaults::default(),
        }
    }

    #[test]
    fn targets_list_roles_with_presets_then_tools() {
        let plan = plan_with_roles();
        let all = targets(Some(&plan), |kind| kind == AdapterKind::ClaudeCode);
        let labels: Vec<(&str, bool)> = all.iter().map(|t| (t.label.as_str(), t.enabled)).collect();
        assert_eq!(
            labels,
            vec![
                ("coder — claude-code", true),
                ("researcher — antigravity", false),
                ("Claude Code", true),
                ("Antigravity CLI", false),
            ]
        );
        assert_eq!(all[0].model.as_deref(), Some("opus-4.8"));
        assert_eq!(all[0].role.as_deref(), Some("coder"));
        assert_eq!(all[2].role, None);
    }

    fn press(form: &mut DispatchForm, code: KeyCode) {
        form.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(form: &mut DispatchForm, text: &str) {
        for c in text.chars() {
            press(form, KeyCode::Char(c));
        }
    }

    #[test]
    fn form_prefills_workspace_budget_and_preselects_default_role() {
        let plan = plan_with_roles();
        let all = targets(Some(&plan), |_| true);
        let budget = Budget {
            posture: DispatchPosture::ReadOnly,
            max_turns: Some(12),
            max_usd: None,
            timeout_secs: None,
        };
        let form = DispatchForm::new(all, Some("coder"), budget, "/tmp".to_string());
        assert_eq!(form.targets[form.target_idx].role.as_deref(), Some("coder"));
        assert_eq!(POSTURES[form.posture_idx].1, DispatchPosture::ReadOnly);
        assert_eq!(form.max_turns, "12");
        assert_eq!(form.max_usd, "");
    }

    #[test]
    fn form_submit_validates_and_builds_the_task() {
        let plan = plan_with_roles();
        let all = targets(Some(&plan), |_| true);
        let mut form = DispatchForm::new(all, Some("coder"), Budget::default(), "/tmp".into());

        // Empty prompt refused.
        assert!(form.submit().is_err());

        type_text(&mut form, "review the diff");
        // Focus the turns field and type a cap; then a bad budget value.
        press(&mut form, KeyCode::Tab); // target
        press(&mut form, KeyCode::Tab); // cwd
        press(&mut form, KeyCode::Tab); // posture
        press(&mut form, KeyCode::Right); // plan → edits
        press(&mut form, KeyCode::Tab); // turns
        type_text(&mut form, "25");
        press(&mut form, KeyCode::Tab); // usd
        type_text(&mut form, "abc");
        assert!(form.submit().is_err(), "non-numeric budget must be refused");
        press(&mut form, KeyCode::Backspace);
        press(&mut form, KeyCode::Backspace);
        press(&mut form, KeyCode::Backspace);
        type_text(&mut form, "2.5");

        let (target, task) = form.submit().expect("valid form");
        assert_eq!(target.role.as_deref(), Some("coder"));
        assert_eq!(task.prompt, "review the diff");
        assert_eq!(task.cwd, PathBuf::from("/tmp"));
        assert_eq!(task.budget.posture, DispatchPosture::Edits);
        assert_eq!(task.budget.max_turns, Some(25));
        assert_eq!(task.budget.max_usd, Some(2.5));
        assert_eq!(task.model.as_deref(), Some("opus-4.8"));
    }

    #[test]
    fn form_refuses_disabled_targets_and_bad_cwd() {
        let plan = plan_with_roles();
        // Nothing installed: everything greyed.
        let all = targets(Some(&plan), |_| false);
        let mut form = DispatchForm::new(all, None, Budget::default(), "/tmp".into());
        type_text(&mut form, "hi");
        let err = form.submit().expect_err("disabled target");
        assert!(err.contains("not installed"), "got: {err}");

        let all = targets(Some(&plan), |_| true);
        let mut form =
            DispatchForm::new(all, None, Budget::default(), "/definitely/not/a/dir".into());
        type_text(&mut form, "hi");
        let err = form.submit().expect_err("bad cwd");
        assert!(err.contains("not a directory"), "got: {err}");
    }

    // -- broadcast (ADR-0013 decision 5, second half) ------------------------

    fn bpress(form: &mut BroadcastForm, code: KeyCode) {
        form.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn btype(form: &mut BroadcastForm, text: &str) {
        for c in text.chars() {
            bpress(form, KeyCode::Char(c));
        }
    }

    #[test]
    fn broadcast_form_preselects_named_roles_and_leaves_agy_unticked_unless_named() {
        let plan = plan_with_roles();

        // Targets: coder (claude), researcher (agy), then the two raw tools.
        // Only names in `defaults.broadcast` start ticked — nothing else.
        let all = targets(Some(&plan), |_| true);
        let form = BroadcastForm::new(
            all,
            &["coder".to_string()],
            Budget::default(),
            "/tmp".into(),
        );
        assert_eq!(form.ticked, vec![true, false, false, false]);

        // Naming the agy-backed role is the workspace opt-in (PRODUCT Q5).
        let all = targets(Some(&plan), |_| true);
        let form = BroadcastForm::new(
            all,
            &["coder".to_string(), "researcher".to_string()],
            Budget::default(),
            "/tmp".into(),
        );
        assert_eq!(form.ticked, vec![true, true, false, false]);

        // A named role whose tool failed probe stays unticked.
        let all = targets(Some(&plan), |kind| kind == AdapterKind::ClaudeCode);
        let form = BroadcastForm::new(
            all,
            &["researcher".to_string()],
            Budget::default(),
            "/tmp".into(),
        );
        assert_eq!(form.ticked, vec![false, false, false, false]);
    }

    #[test]
    fn broadcast_form_space_ticks_targets_but_never_disabled_ones() {
        let plan = plan_with_roles();
        // researcher (agy) and the raw agy tool are disabled.
        let all = targets(Some(&plan), |kind| kind == AdapterKind::ClaudeCode);
        let mut form = BroadcastForm::new(all, &[], Budget::default(), "/tmp".into());

        // Space in the prompt field types a space, ticks nothing.
        btype(&mut form, "a b");
        assert_eq!(form.prompt, "a b");
        assert!(form.ticked.iter().all(|t| !t));

        // Tick the enabled coder row.
        bpress(&mut form, KeyCode::Tab); // → targets
        bpress(&mut form, KeyCode::Char(' '));
        assert_eq!(form.ticked, vec![true, false, false, false]);

        // The disabled researcher row refuses to tick.
        bpress(&mut form, KeyCode::Right);
        bpress(&mut form, KeyCode::Char(' '));
        assert_eq!(form.ticked, vec![true, false, false, false]);

        // Toggling off works too.
        bpress(&mut form, KeyCode::Left);
        bpress(&mut form, KeyCode::Char(' '));
        assert!(form.ticked.iter().all(|t| !t));
    }

    #[test]
    fn broadcast_form_submit_requires_prompt_and_a_ticked_target() {
        let plan = plan_with_roles();
        let all = targets(Some(&plan), |_| true);
        let mut form = BroadcastForm::new(all, &[], Budget::default(), "/tmp".into());

        let err = form.submit().expect_err("nothing ticked");
        assert!(err.contains("tick at least one"), "got: {err}");

        bpress(&mut form, KeyCode::Tab); // → targets
        bpress(&mut form, KeyCode::Char(' ')); // tick coder
        let err = form.submit().expect_err("empty prompt");
        assert!(err.contains("prompt is empty"), "got: {err}");

        bpress(&mut form, KeyCode::BackTab); // → prompt
        btype(&mut form, "compare this");
        assert!(form.submit().is_ok());
    }

    #[test]
    fn broadcast_form_submit_builds_one_task_per_ticked_target_sharing_budget_and_cwd() {
        let plan = plan_with_roles();
        let all = targets(Some(&plan), |_| true);
        let budget = Budget {
            posture: DispatchPosture::ReadOnly,
            max_turns: Some(9),
            max_usd: None,
            timeout_secs: Some(60),
        };
        let mut form = BroadcastForm::new(
            all,
            &["coder".to_string(), "researcher".to_string()],
            budget,
            "/tmp".into(),
        );
        btype(&mut form, "same prompt");

        let picked = form.submit().expect("valid form");
        assert_eq!(picked.len(), 2);
        let (coder, coder_task) = &picked[0];
        let (researcher, researcher_task) = &picked[1];
        assert_eq!(coder.role.as_deref(), Some("coder"));
        assert_eq!(researcher.role.as_deref(), Some("researcher"));
        // Per-target presets diverge; everything else is shared.
        assert_eq!(coder_task.model.as_deref(), Some("opus-4.8"));
        assert_eq!(researcher_task.model, None);
        for task in [coder_task, researcher_task] {
            assert_eq!(task.prompt, "same prompt");
            assert_eq!(task.cwd, PathBuf::from("/tmp"));
            assert_eq!(task.budget.posture, DispatchPosture::ReadOnly);
            assert_eq!(task.budget.max_turns, Some(9));
            assert_eq!(task.budget.timeout_secs, Some(60));
        }
    }

    #[test]
    fn compare_group_folds_status_cost_and_tail_per_column() {
        let mut group = BroadcastGroup {
            columns: vec![
                BroadcastColumn::new(1, "coder".to_string()),
                BroadcastColumn::new(2, "researcher".to_string()),
            ],
        };

        group.fold(1, &AgentEvent::Started { native_id: None });
        group.fold(
            1,
            &AgentEvent::AgentText("line one\n\nline two".to_string()),
        );
        group.fold(1, &AgentEvent::ToolActivity("Read".to_string()));
        group.fold(
            1,
            &AgentEvent::Completed {
                result: "done".to_string(),
                cost_usd: Some(0.5),
            },
        );
        group.fold(
            2,
            &AgentEvent::Failed {
                reason: "timeout\ndetail".to_string(),
            },
        );
        // Unknown session → no-op, no panic.
        group.fold(99, &AgentEvent::AgentText("lost".to_string()));

        let coder = &group.columns[0];
        assert_eq!(coder.status, SessionStatus::Completed);
        assert_eq!(coder.cost_usd, Some(0.5));
        assert_eq!(
            coder.tail.iter().cloned().collect::<Vec<_>>(),
            vec!["· started", "line one", "line two", "[Read]"]
        );
        let researcher = &group.columns[1];
        assert_eq!(researcher.status, SessionStatus::Failed);
        assert_eq!(
            researcher.tail.back().map(String::as_str),
            Some("✗ timeout")
        );

        // The rolling tail is capped.
        let mut col = BroadcastColumn::new(3, "x".to_string());
        for i in 0..150 {
            col.push_tail(format!("line {i}"));
        }
        assert_eq!(col.tail.len(), COLUMN_TAIL_CAP);
        assert_eq!(col.tail.front().map(String::as_str), Some("line 50"));
    }
}
