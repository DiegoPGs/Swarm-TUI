//! Normalized cross-tool event vocabulary (ARCHITECTURE.md, normalization table).
//!
//! Adapters translate native streams into this enum:
//! - Claude Code: `stream-json` messages (`init` → Started with session_id,
//!   assistant text → AgentText, tool_use → ToolActivity, `result` → Completed
//!   with `total_cost_usd`).
//! - Codex: `--json` JSONL (`thread.started` → Started, `item.*` agent messages
//!   → AgentText, command/file/MCP items → ToolActivity, `turn.completed` →
//!   Completed, `turn.failed`/`error` → Failed).
//! - Antigravity: plain text only (ADR-0001) — adapters synthesize Started on
//!   spawn and Completed on clean exit; **no ToolActivity is possible**, and
//!   the UI must degrade gracefully when it is absent.

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Process is up. `native_id` present when the tool reports one up front
    /// (claude `session_id`, codex thread id); agy backfills later or never
    /// (ADR-0002 backfill lane).
    Started { native_id: Option<String> },
    /// Agent-authored text (assistant messages / final answers).
    AgentText(String),
    /// Tool, command, or file activity — for roster status lines and the
    /// broadcast compare view. Human-readable one-liner, not raw payload.
    ToolActivity(String),
    /// Terminal success. `cost_usd` only where the tool reports it (claude).
    Completed { result: String, cost_usd: Option<f64> },
    /// Terminal failure: non-zero exit, budget stop, `turn.failed`, timeout.
    Failed { reason: String },
}
