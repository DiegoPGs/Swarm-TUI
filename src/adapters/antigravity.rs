//! Antigravity CLI (`agy`) adapter. Facts: `docs/integrations/antigravity.md`.
//!
//! The constrained one (ADR-0001): no structured output confirmed at v1.0.16,
//! so the programmatic channel is `-p` plain text with **synthesized** events
//! (Started on spawn, Completed on clean exit, Failed otherwise — never a
//! ToolActivity). Resume-by-id IS real (`--conversation <ID>`), which is more
//! than the original brief hoped for.
//!
//! Quota note: agy shares quota with the desktop app; the Home view keeps agy
//! **opt-in** for broadcast (docs/PRODUCT.md, open question 5).

use std::path::Path;
use std::process::Command;

use super::{
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, ResumeSupport,
    StructuredOutput,
};
use crate::core::session::SessionRecord;
use crate::core::task::Task;

pub struct Antigravity;

/// Research-expected caps (v1.0.16, 2026-07-04); `probe()` must confirm —
/// especially `--output-format`: if it EXISTS on the installed build, that
/// invalidates part of ADR-0001 and deserves a superseding note.
pub const EXPECTED_CAPS: AdapterCaps = AdapterCaps {
    structured_output: StructuredOutput::None,
    resume: ResumeSupport::ById,
    background_supervisor: false,
};

impl CliAdapter for Antigravity {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn display_name(&self) -> &'static str {
        "Antigravity CLI"
    }

    fn binary(&self) -> &'static str {
        "agy"
    }

    fn probe(&self) -> Result<AdapterCaps, AdapterError> {
        // TODO(next session): `agy --version`; grep --help for
        // -p/--print, --print-timeout, --conversation, -c/--continue, and the
        // (expected-absent) --output-format. This tool moves fastest — probe
        // results should be trusted over EXPECTED_CAPS wherever they differ.
        todo!("probe installed agy against EXPECTED_CAPS")
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, cwd: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        match intent {
            LaunchIntent::Fresh => {}
            LaunchIntent::Resume { native_id } => {
                cmd.arg("--conversation").arg(native_id);
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
        //   agy -p <prompt> --print-timeout <task budget-ish>   (in task.cwd)
        // Synthesize events from process lifecycle + stdout as AgentText.
        // ⬜ unverified (verify-clis.sh + one live run):
        //   * does -p create a resumable conversation? if yes, backfill
        //     native_id via the ADR-0002 lane (serialized `-c` follow-up, or
        //     conversation-store lookup by timestamp);
        //   * -p permission behavior with no TTY — until known, router policy
        //     is read-oriented tasks only (ARCHITECTURE guardrail table).
        todo!("headless dispatch via agy -p, synthesized events (ADR-0001)")
    }

    fn follow_up(
        &self,
        _session: &SessionRecord,
        _task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        // TODO(next session): agy -p --conversation <native_id> <prompt> —
        // the flag combo itself is ⬜; if unsupported, fall back to the
        // serialized `-c` lane (one agy follow-up in flight at a time).
        todo!("headless follow-up via agy --conversation (ADR-0001/0002)")
    }
}
