//! The adapter boundary (ADR-0006). **Everything CLI-specific lives at or below
//! this module.** `app`, `core`, `pty`, `store` know tools only as opaque slugs.
//!
//! Two channels per tool (ADR-0001):
//! - *Interactive*: `interactive_cmd()` → a `Command` the PTY layer spawns into
//!   a tab. Mandatory for every adapter.
//! - *Programmatic*: `dispatch()` / `follow_up()` → normalized `AgentEvent`
//!   streams. Capability-gated; the default impls say "unsupported".
//!
//! Minimum viable adapter = `id`/`display_name`/`binary` + `probe` +
//! `interactive_cmd`. That alone earns a tab.

pub mod antigravity;
pub mod claude_code;
pub mod codex;

use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc::Receiver;

use crate::core::events::AgentEvent;
use crate::core::session::SessionRecord;
use crate::core::task::Task;

/// What a tool can do, expressed as **data** — probed at startup, cached, and
/// used by the Home view to enable/disable actions. A failed probe downgrades
/// a tool to interactive-only; it never removes the tool (ARCHITECTURE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCaps {
    pub structured_output: StructuredOutput,
    pub resume: ResumeSupport,
    /// Tool ships its own background-session supervisor
    /// (Claude Code `--bg` / `claude agents`). ADR-0002 reconciles with it.
    pub background_supervisor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredOutput {
    /// Plain text only (agy as verified at v1.0.16) — events are synthesized.
    None,
    /// Single JSON document at end of run.
    Json,
    /// Incremental structured events (claude `stream-json`, codex `--json`).
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeSupport {
    /// Resume any session by its native id (all three, per current research).
    ById,
    /// Only "continue most recent" is available.
    ContinueOnly,
    None,
}

/// Why a tab/PTY is being opened. Produced by the Home view & tab manager.
pub enum LaunchIntent {
    /// `session_id_hint` lets the registry pre-assign a native id before the
    /// process starts (ADR-0002); only Claude Code's `--session-id` acts on
    /// it today, the other adapters ignore it.
    Fresh {
        session_id_hint: Option<String>,
    },
    Resume {
        native_id: String,
    },
    ContinueMostRecent,
}

/// A running headless dispatch: normalized events plus the child to reap.
/// TODO(next session): becomes an async stream once tokio lands (ADR-0005);
/// the std `mpsc` shape here exists only to pin the boundary dependency-free.
pub struct DispatchHandle {
    pub events: Receiver<AgentEvent>,
    pub child: Child,
}

#[derive(Debug)]
pub enum AdapterError {
    /// The tool has no programmatic channel for this operation.
    Unsupported(&'static str),
    Spawn(std::io::Error),
    Probe(String),
}

pub trait CliAdapter {
    /// Stable slug used in the registry (`SessionRecord::tool`).
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// Binary name resolved via PATH. Never a hardcoded absolute path: we run
    /// exactly what the user's shell would run (reuse-existing-install rule).
    fn binary(&self) -> &'static str;

    /// Read-only capability probe: `--version` + `--help` greps, nothing else.
    /// Must never touch config/auth files (AGENTS.md boundary).
    fn probe(&self) -> Result<AdapterCaps, AdapterError>;

    /// Command that opens the tool's own TUI for a tab.
    fn interactive_cmd(&self, intent: &LaunchIntent, cwd: &Path) -> Command;

    /// Headless one-shot in `task.cwd`, translating `task.budget` into the
    /// tool's native guardrails (ARCHITECTURE guardrail table).
    fn dispatch(&self, _task: &Task) -> Result<DispatchHandle, AdapterError> {
        Err(AdapterError::Unsupported("headless dispatch"))
    }

    /// Headless follow-up into an existing session (`session.native_id`).
    fn follow_up(
        &self,
        _session: &SessionRecord,
        _task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        Err(AdapterError::Unsupported("headless follow-up"))
    }
}

/// Compile-time dispatch over the built-in adapters — no `dyn`, no
/// `async-trait` (ADR-0006). Adding a tool = add a variant + module; the
/// exhaustive matches below make every missing integration a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    ClaudeCode,
    Antigravity,
    Codex,
}

/// The adapter registry: iterate this to probe/list every known tool.
pub fn registry() -> [AdapterKind; 3] {
    [
        AdapterKind::ClaudeCode,
        AdapterKind::Antigravity,
        AdapterKind::Codex,
    ]
}

impl AdapterKind {
    pub fn from_slug(slug: &str) -> Option<AdapterKind> {
        match slug {
            "claude-code" => Some(AdapterKind::ClaudeCode),
            "antigravity" => Some(AdapterKind::Antigravity),
            "codex" => Some(AdapterKind::Codex),
            _ => None,
        }
    }
}

impl CliAdapter for AdapterKind {
    fn id(&self) -> &'static str {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.id(),
            AdapterKind::Antigravity => antigravity::Antigravity.id(),
            AdapterKind::Codex => codex::Codex.id(),
        }
    }

    fn display_name(&self) -> &'static str {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.display_name(),
            AdapterKind::Antigravity => antigravity::Antigravity.display_name(),
            AdapterKind::Codex => codex::Codex.display_name(),
        }
    }

    fn binary(&self) -> &'static str {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.binary(),
            AdapterKind::Antigravity => antigravity::Antigravity.binary(),
            AdapterKind::Codex => codex::Codex.binary(),
        }
    }

    fn probe(&self) -> Result<AdapterCaps, AdapterError> {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.probe(),
            AdapterKind::Antigravity => antigravity::Antigravity.probe(),
            AdapterKind::Codex => codex::Codex.probe(),
        }
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, cwd: &Path) -> Command {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.interactive_cmd(intent, cwd),
            AdapterKind::Antigravity => antigravity::Antigravity.interactive_cmd(intent, cwd),
            AdapterKind::Codex => codex::Codex.interactive_cmd(intent, cwd),
        }
    }

    fn dispatch(&self, task: &Task) -> Result<DispatchHandle, AdapterError> {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.dispatch(task),
            AdapterKind::Antigravity => antigravity::Antigravity.dispatch(task),
            AdapterKind::Codex => codex::Codex.dispatch(task),
        }
    }

    fn follow_up(
        &self,
        session: &SessionRecord,
        task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.follow_up(session, task),
            AdapterKind::Antigravity => antigravity::Antigravity.follow_up(session, task),
            AdapterKind::Codex => codex::Codex.follow_up(session, task),
        }
    }
}
