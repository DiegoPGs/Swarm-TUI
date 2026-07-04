//! Codex CLI adapter. Facts + markers: `docs/integrations/codex.md`.
//!
//! Channels (ADR-0001):
//! - Interactive: `codex` / `codex resume <id>` in a PTY.
//! - Programmatic: `codex exec --json` (JSONL events; `thread.started` carries
//!   the native id) and `codex exec resume <id>` for follow-ups.
//! - v2 seam: `codex app-server` (JSON-RPC; exposes thread/fork that the exec
//!   surface lacks). Deliberately NOT used in v1 — keep one integration style.

use std::path::Path;
use std::process::Command;

use super::{
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, ResumeSupport,
    StructuredOutput,
};
use crate::core::session::SessionRecord;
use crate::core::task::Task;

pub struct Codex;

/// Research-expected caps (npm 0.142.5, 2026-07-04); `probe()` must confirm.
pub const EXPECTED_CAPS: AdapterCaps = AdapterCaps {
    structured_output: StructuredOutput::StreamJson,
    resume: ResumeSupport::ById,
    background_supervisor: false,
};

impl CliAdapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex CLI"
    }

    fn binary(&self) -> &'static str {
        "codex"
    }

    fn probe(&self) -> Result<AdapterCaps, AdapterError> {
        // TODO(next session): `codex --version` (tolerate alpha strings);
        // check `codex exec --help` for --json / -o / --output-schema and
        // `codex exec resume --help` existence. Record ⬜ answers ([agents]
        // block, repo-local .codex/) in the integration page.
        todo!("probe installed codex against EXPECTED_CAPS")
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, cwd: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        match intent {
            LaunchIntent::Fresh => {}
            LaunchIntent::Resume { native_id } => {
                cmd.arg("resume").arg(native_id);
            }
            LaunchIntent::ContinueMostRecent => {
                cmd.args(["resume", "--last"]);
            }
        }
        cmd.current_dir(cwd);
        cmd
    }

    fn dispatch(&self, _task: &Task) -> Result<DispatchHandle, AdapterError> {
        // TODO(next session):
        //   codex exec --json <prompt>          (in task.cwd)
        //   + `--sandbox workspace-write` only when task.budget.allow_writes;
        //   default stays the tool's read-only sandbox.
        // PRECONDITION (router-enforced): task.cwd is inside a git repo —
        // codex exec refuses otherwise; surface that as a friendly dispatch
        // error rather than passing --skip-git-repo-check.
        // Parse JSONL: thread.started → Started{native_id}, item.* → AgentText
        // / ToolActivity, turn.completed → Completed, turn.failed/error →
        // Failed. Capture one real stream as tests/fixtures/ before writing
        // the parser.
        todo!("headless dispatch via codex exec --json (ADR-0001)")
    }

    fn follow_up(
        &self,
        _session: &SessionRecord,
        _task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        // TODO(next session): codex exec resume <native_id> --json <prompt>.
        todo!("headless follow-up via codex exec resume (ADR-0001)")
    }
}
