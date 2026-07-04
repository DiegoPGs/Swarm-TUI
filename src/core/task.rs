//! Tasks: what the Home view dispatches (ARCHITECTURE task flow, steps 1–2).

use std::path::PathBuf;

/// One unit of headless work aimed at one tool.
/// Broadcast = the same `Task` cloned across N tools (ADR-0004: composition
/// happens at the process level, via tasks and shared cwd — never by reaching
/// into a tool's internal subagent system).
#[derive(Debug, Clone)]
pub struct Task {
    pub prompt: String,
    /// cwd is a property of the TASK, not of overstory — every dispatch names
    /// its working directory explicitly.
    pub cwd: PathBuf,
    pub budget: Budget,
}

/// Guardrails as data. Adapters translate: claude → `--max-turns` /
/// `--max-budget-usd`; codex → sandbox mode stays read-only unless the task
/// asks for `workspace-write`; agy → read-oriented tasks only until its `-p`
/// permission behavior is verified locally (ARCHITECTURE guardrail table).
#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    pub max_turns: Option<u32>,
    pub max_usd: Option<f64>,
    /// Allow file writes in the task cwd (never escalates beyond it by default;
    /// the "never-by-default" flags in ARCHITECTURE.md stay off, period).
    pub allow_writes: bool,
}
