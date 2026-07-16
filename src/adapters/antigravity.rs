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
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, LaunchOptions,
    LaunchOptionsDecl, NativeCommand, ResumeSupport, StructuredOutput,
};
use crate::core::session::SessionRecord;
use crate::core::task::Task;

pub struct Antigravity;

/// No picker suggestions yet: the exact string format `--model` accepts is
/// still ⬜ (`agy models` lists display names like "Gemini 3.1 Pro (High)";
/// whether those exact strings are the accepted arguments is unverified —
/// see command-surfaces.md). Free text stays available in the picker.
pub const MODEL_SUGGESTIONS: &[&str] = &[];

/// Research-expected caps (v1.0.16, 2026-07-04; launch decl re-verified
/// locally 2026-07-16 at 1.1.3); `probe()` must confirm — especially
/// `--output-format`: if it EXISTS on the installed build, that invalidates
/// part of ADR-0001 and deserves a superseding note.
pub const EXPECTED_CAPS: AdapterCaps = AdapterCaps {
    structured_output: StructuredOutput::None,
    resume: ResumeSupport::ById,
    background_supervisor: false,
    launch: LaunchOptionsDecl {
        model: Some(MODEL_SUGGESTIONS),
        effort: None, // agy has no effort flag; depth rides on the model variant
    },
};

/// Palette table (ADR-0009): every entry is ✅ *(local 2026-07-16)* in
/// `docs/integrations/command-surfaces.md` — present in the installed 1.1.3
/// "/" menu. Alias rows (`/switch`, `/conversation`, `/branch`, `/undo`,
/// `/cs`, `/quota`, `/settings`) are folded into their primary entries'
/// descriptions. `persists` flags mirror the doc's column; the adapter test
/// pins that correspondence.
const COMMANDS: &[NativeCommand] = &[
    NativeCommand {
        name: "/model",
        inject: "/model",
        description: "Set a model (observed to stick as the default)",
        args_hint: Some("model name — `agy models` lists them; empty opens the picker"),
        persists: true,
    },
    NativeCommand {
        name: "/resume",
        inject: "/resume",
        description: "Browse and resume past conversations (aliases /switch, /conversation)",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/fork",
        inject: "/fork",
        description: "Branch the conversation at this point (alias /branch)",
        args_hint: Some("optional project id to fork into"),
        persists: false,
    },
    NativeCommand {
        name: "/rewind",
        inject: "/rewind",
        description: "Rewind conversation to a previous message (alias /undo)",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/rename",
        inject: "/rename",
        description: "Rename the current conversation",
        args_hint: Some("new name"),
        persists: false,
    },
    NativeCommand {
        name: "/agents",
        inject: "/agents",
        description: "List available custom agents (Agent Manager panel)",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/goal",
        inject: "/goal",
        description: "Run until the specified goal is completely finished",
        args_hint: Some("goal"),
        persists: false,
    },
    NativeCommand {
        name: "/schedule",
        inject: "/schedule",
        description: "Run an instruction on a recurring schedule or one-time timer",
        args_hint: Some("instruction"),
        persists: false,
    },
    NativeCommand {
        name: "/codesearch",
        inject: "/codesearch",
        description: "Search code in the workspace (alias /cs)",
        args_hint: Some("query (regex by default; -F/--literal for exact)"),
        persists: false,
    },
    NativeCommand {
        name: "/btw",
        inject: "/btw",
        description: "Ask a side question without interrupting the current task",
        args_hint: Some("question"),
        persists: false,
    },
    NativeCommand {
        name: "/plan",
        inject: "/plan",
        description: "Plan carefully before executing a task",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/tasks",
        inject: "/tasks",
        description: "View background tasks",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/context",
        inject: "/context",
        description: "Visualize current context usage",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/usage",
        inject: "/usage",
        description: "View model quota usage (alias /quota)",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/credits",
        inject: "/credits",
        description: "Show remaining G1 credits and purchase link",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/permissions",
        inject: "/permissions",
        description: "Manage tool permissions",
        args_hint: None,
        persists: true,
    },
    NativeCommand {
        name: "/config",
        inject: "/config",
        description: "Open settings panel (alias /settings)",
        args_hint: None,
        persists: true,
    },
    NativeCommand {
        name: "/keybindings",
        inject: "/keybindings",
        description: "Set custom keybindings",
        args_hint: None,
        persists: true,
    },
    NativeCommand {
        name: "/diff",
        inject: "/diff",
        description: "View uncommitted changes and per-turn diffs",
        args_hint: None,
        persists: false,
    },
];

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
        let version = super::command_output(self.binary(), &["--version"])
            .map_err(|e| AdapterError::Probe(format!("agy --version failed to run: {e}")))?;
        if !version.status.success() {
            return Err(AdapterError::Probe(format!(
                "agy --version exited with {:?}",
                version.status.code()
            )));
        }

        let help = super::help_text(self.binary(), &["--help"]);
        let has = |flag: &str| help.contains(flag);

        let has_print = has("-p") || has("--print") || has("--prompt");
        let has_print_timeout = has("--print-timeout");
        let has_conversation = has("--conversation");
        let has_continue = has("-c") || has("--continue");
        // Launch-option decl (ADR-0009): model only — agy has no effort flag.
        let launch = LaunchOptionsDecl {
            model: has("--model").then_some(MODEL_SUGGESTIONS),
            effort: None,
        };
        // docs/integrations/antigravity.md: agy genuinely has no
        // --output-format at the locally verified version — unlike claude's
        // hidden-flag situation, this absence IS meaningful, and its
        // presence would be a real capability upgrade worth flagging in
        // ADR-0001, not silently trusted away.
        let has_output_format = has("--output-format");

        if !has_print {
            return Err(AdapterError::Probe(
                "agy --help no longer lists -p/--print — headless channel assumption is broken"
                    .to_string(),
            ));
        }
        let _ = has_print_timeout; // confirmed present when checked; no cap field to downgrade

        let structured_output = if has_output_format {
            // Contradicts ADR-0001's plain-text assumption — surface it as a
            // real (if surprising) capability rather than silently ignoring
            // it; a superseding ADR note is owed once this is observed live.
            StructuredOutput::Json
        } else {
            StructuredOutput::None
        };
        let resume = if has_conversation || has_continue {
            EXPECTED_CAPS.resume
        } else {
            ResumeSupport::None
        };

        Ok(AdapterCaps {
            structured_output,
            resume,
            background_supervisor: EXPECTED_CAPS.background_supervisor,
            launch,
        })
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, opts: &LaunchOptions, cwd: &Path) -> Command {
        let mut cmd = Command::new(self.binary());
        match intent {
            LaunchIntent::Fresh { .. } => {}
            LaunchIntent::Resume { native_id } => {
                cmd.arg("--conversation").arg(native_id);
            }
            LaunchIntent::ContinueMostRecent => {
                cmd.arg("--continue");
            }
        }
        // Model is agy's only launch option; `opts.effort` is silently
        // ignored — agy has no effort flag (ADR-0009).
        if let Some(model) = &opts.model {
            cmd.arg("--model").arg(model);
        }
        cmd.current_dir(cwd);
        cmd
    }

    fn command_table(&self) -> &'static [NativeCommand] {
        COMMANDS
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
