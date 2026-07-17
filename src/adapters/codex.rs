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
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, LaunchOptions,
    LaunchOptionsDecl, ResumeSupport, StructuredOutput,
};
use crate::core::session::SessionRecord;
use crate::core::task::Task;

pub struct Codex;

/// Research-expected caps (npm 0.142.5, 2026-07-04); `probe()` must confirm.
/// Launch decl is NONE and the command table stays the trait default `&[]`:
/// codex is suspended (ADR-0008) and nothing here is locally verifiable —
/// populate both from observation on reversal (ADR-0009's ✅-only rule).
pub const EXPECTED_CAPS: AdapterCaps = AdapterCaps {
    structured_output: StructuredOutput::StreamJson,
    resume: ResumeSupport::ById,
    background_supervisor: false,
    launch: LaunchOptionsDecl::NONE,
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
        // `codex --version` (tolerated: npm ships per-platform + alpha
        // strings, e.g. `0.142.5-linux-x64` — we don't parse the string,
        // just confirm the binary runs and exits 0).
        let version = super::command_output(self.binary(), &["--version"])
            .map_err(|e| AdapterError::Probe(format!("codex --version failed to run: {e}")))?;
        if !version.status.success() {
            return Err(AdapterError::Probe(format!(
                "codex --version exited with {:?}",
                version.status.code()
            )));
        }

        let exec_help = super::help_text(self.binary(), &["exec", "--help"]);
        let structured_output = if exec_help.contains("--json") {
            EXPECTED_CAPS.structured_output
        } else {
            StructuredOutput::None
        };

        // `codex exec resume --help` existing (non-error exit) is the
        // positive-existence check per the task spec, distinct from a text
        // grep — resume is a subcommand, not a flag.
        let resume_help_ok = super::command_output(self.binary(), &["exec", "resume", "--help"])
            .map(|out| out.status.success())
            .unwrap_or(false);
        let resume = if resume_help_ok {
            EXPECTED_CAPS.resume
        } else {
            ResumeSupport::None
        };

        Ok(AdapterCaps {
            structured_output,
            resume,
            background_supervisor: EXPECTED_CAPS.background_supervisor,
            launch: LaunchOptionsDecl::NONE,
        })
    }

    // `_opts` ignored entirely: codex steers model via config profiles, not
    // flags (codex.md), and is suspended anyway (ADR-0008).
    fn interactive_cmd(&self, intent: &LaunchIntent, _opts: &LaunchOptions, cwd: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        match intent {
            LaunchIntent::Fresh { .. } => {}
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
        //   + `--sandbox workspace-write` only when task.budget.posture is Edits;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn launch_options_are_ignored_entirely() {
        let intent = LaunchIntent::Resume {
            native_id: "tid-9".to_string(),
        };
        let opts = LaunchOptions {
            model: Some("o5".to_string()),
            effort: Some("max".to_string()),
        };
        let cmd = Codex.interactive_cmd(&intent, &opts, Path::new("/tmp"));
        let argv: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // Suspended (ADR-0008) and flagless for model/effort (codex.md):
        // options must leave the argv untouched.
        assert_eq!(argv, ["resume", "tid-9"]);
    }
}
