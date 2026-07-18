//! Headless dispatch UI state (ADR-0013): the Home-local dispatch form, the
//! running-dispatch bookkeeping the app folds events into, and the timeline
//! panel. Knows tools only as `AdapterKind` + data (ADR-0006): every flag
//! decision lives behind `CliAdapter::dispatch`.

use std::collections::VecDeque;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::adapters::{self, AdapterKind, CliAdapter, DispatchHandle};
use crate::core::plan::SwarmPlan;
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
        let max_turns = parse_optional(&self.max_turns, "turns must be a whole number", |s| {
            s.parse::<u32>().ok().filter(|v| *v > 0)
        })?;
        let max_usd = parse_optional(&self.max_usd, "budget must be a positive number", |s| {
            s.parse::<f64>().ok().filter(|v| *v > 0.0 && v.is_finite())
        })?;
        let timeout_secs =
            parse_optional(&self.timeout_secs, "timeout must be whole seconds", |s| {
                s.parse::<u64>().ok().filter(|v| *v > 0)
            })?;
        let task = Task {
            prompt: prompt.to_string(),
            cwd,
            budget: Budget {
                posture: POSTURES[self.posture_idx].1,
                max_turns,
                max_usd,
                timeout_secs,
            },
            model: target.model.clone(),
            effort: target.effort.clone(),
        };
        Ok((target.clone(), task))
    }
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
}
