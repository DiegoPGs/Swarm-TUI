//! Claude Code adapter. Facts + markers: `docs/integrations/claude-code.md`.
//!
//! Channels (ADR-0001):
//! - Interactive: plain `claude` in a PTY (full TUI, native approvals).
//! - Programmatic: `claude -p --output-format stream-json --verbose` with a
//!   **pre-assigned** `--session-id <uuid>` so the registry knows the native id
//!   before the process even starts (ADR-0002). NEVER pass `--bare` — it skips
//!   OAuth/keychain and would break the reuse-existing-login guarantee.
//! - Long tasks may use the native background supervisor (`--bg`,
//!   `claude agents --json`) instead of holding a pipe open; reconciliation
//!   reads it at startup.

use std::path::Path;
use std::process::Command;

use super::{
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, ResumeSupport,
    StructuredOutput,
};
use crate::core::session::SessionRecord;
use crate::core::task::Task;

pub struct ClaudeCode;

/// What research says the caps SHOULD be (npm 2.1.201, 2026-07-04). `probe()`
/// must confirm against the installed binary, not assume.
pub const EXPECTED_CAPS: AdapterCaps = AdapterCaps {
    structured_output: StructuredOutput::StreamJson,
    resume: ResumeSupport::ById,
    background_supervisor: true,
};

impl CliAdapter for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn binary(&self) -> &'static str {
        "claude"
    }

    fn probe(&self) -> Result<AdapterCaps, AdapterError> {
        // TODO(next session): `claude --version`; grep `--help` for
        // -p/--output-format/--resume/--session-id/--bg/--max-budget-usd
        // (NB: claude's --help deliberately omits some flags — treat absence
        // as "unknown", fall back to EXPECTED_CAPS fields individually, and
        // record findings in the integration page). Also `claude auth status`
        // exit code as the logged-in check for the doctor view.
        todo!("probe installed claude against EXPECTED_CAPS")
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, cwd: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        match intent {
            LaunchIntent::Fresh => {}
            // Gotcha: `--resume <id>` lookup is scoped to cwd (+ worktrees) —
            // the registry's stored cwd is what makes this reliable.
            LaunchIntent::Resume { native_id } => {
                cmd.arg("--resume").arg(native_id);
            }
            LaunchIntent::ContinueMostRecent => {
                cmd.arg("--continue");
            }
        }
        cmd.current_dir(cwd);
        cmd
    }

    fn dispatch(&self, _task: &Task) -> Result<DispatchHandle, AdapterError> {
        // TODO(next session):
        //   claude -p <prompt> --output-format stream-json --verbose \
        //     --session-id <fresh uuid> [--max-turns N] [--max-budget-usd X] \
        //     [--permission-mode plan | acceptEdits per task.budget.allow_writes]
        // in task.cwd; parse NDJSON lines into AgentEvent (init → Started,
        // assistant text → AgentText, tool_use → ToolActivity, result →
        // Completed{cost_usd: total_cost_usd}). Gotcha: since 2.1.163 its
        // background Bash tools are killed ~5s after the final result.
        todo!("headless dispatch via -p stream-json (ADR-0001)")
    }

    fn follow_up(
        &self,
        _session: &SessionRecord,
        _task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        // TODO(next session): same as dispatch but `--resume <native_id>`;
        // MUST run in the session's recorded cwd or the id won't be found.
        todo!("headless follow-up via -p --resume (ADR-0001/0002)")
    }
}
