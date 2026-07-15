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
        let version = super::command_output(self.binary(), &["--version"])
            .map_err(|e| AdapterError::Probe(format!("claude --version failed to run: {e}")))?;
        if !version.status.success() {
            return Err(AdapterError::Probe(format!(
                "claude --version exited with {:?}",
                version.status.code()
            )));
        }

        let help = super::help_text(self.binary(), &["--help"]);
        let has = |flag: &str| help.contains(flag);

        // docs/integrations/claude-code.md's own warning: `--help` at 2.1.201
        // deliberately hides some real flags (observed: --max-turns), so
        // "absent from --help" is not automatically "unavailable" in general.
        // But none of the six flags this probe checks are on that
        // known-hidden list — the doc's 2026-07-05 local pass positively
        // confirms -p, --output-format, --resume, --session-id, --bg, and
        // --max-budget-usd all appear in top-level `--help` at the verified
        // version. So for *these* flags specifically, absence is a genuine
        // signal worth downgrading on, exactly like agy's --output-format
        // case — only truly-hidden-but-documented flags (not checked here)
        // get the "unknown, trust the doc" treatment.
        let has_print = has("-p") || has("--print");
        let has_output_format = has("--output-format");
        let has_resume = has("--resume");
        let has_session_id = has("--session-id");
        let has_bg = has("--bg") || has("--background");
        let has_max_budget = has("--max-budget-usd");

        // has_print/has_max_budget don't map to an `AdapterCaps` field (the
        // struct only tracks structured_output/resume/background_supervisor)
        // but are still worth confirming — a probe that can't even find `-p`
        // would mean the whole headless channel assumption is broken, which
        // should show up as a probe failure rather than a silent downgrade.
        if !has_print {
            return Err(AdapterError::Probe(
                "claude --help no longer lists -p/--print — headless channel assumption is broken"
                    .to_string(),
            ));
        }

        let structured_output = if has_output_format {
            EXPECTED_CAPS.structured_output
        } else {
            StructuredOutput::None
        };
        let resume = if has_resume || has_session_id {
            EXPECTED_CAPS.resume
        } else {
            ResumeSupport::None
        };
        let background_supervisor = has_bg;
        let _ = has_max_budget; // confirmed present when checked; no cap field to downgrade

        Ok(AdapterCaps {
            structured_output,
            resume,
            background_supervisor,
        })
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, cwd: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        match intent {
            LaunchIntent::Fresh { session_id_hint } => {
                if let Some(hint) = session_id_hint {
                    cmd.arg("--session-id").arg(hint);
                }
            }
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
