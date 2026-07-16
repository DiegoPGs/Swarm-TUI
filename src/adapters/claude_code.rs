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
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, LaunchOptions,
    LaunchOptionsDecl, NativeCommand, ResumeSupport, StructuredOutput,
};
use crate::core::session::SessionRecord;
use crate::core::task::Task;

pub struct ClaudeCode;

/// Picker suggestions for the free-text model field — the alias forms
/// `claude --help` documents (✅ local 2026-07-16 at 2.1.211); any full model
/// name is also accepted.
pub const MODEL_SUGGESTIONS: &[&str] = &["fable", "opus", "sonnet"];

/// `--effort` accepted levels, verbatim from `claude --help`
/// (✅ local 2026-07-16 at 2.1.211).
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// What research says the caps SHOULD be (npm 2.1.201, 2026-07-04; launch
/// decl re-verified locally 2026-07-16 at 2.1.211). `probe()` must confirm
/// against the installed binary, not assume.
pub const EXPECTED_CAPS: AdapterCaps = AdapterCaps {
    structured_output: StructuredOutput::StreamJson,
    resume: ResumeSupport::ById,
    background_supervisor: true,
    launch: LaunchOptionsDecl {
        model: Some(MODEL_SUGGESTIONS),
        effort: Some(EFFORT_LEVELS),
    },
};

/// Palette table (ADR-0009): every entry is ✅ *(local 2026-07-16)* in
/// `docs/integrations/command-surfaces.md` — present in the installed 2.1.211
/// "/" menu. Deliberately excluded from the ✅ set: `/agents` (a "(removed)"
/// stub) and the `/cost` alias row (folded into `/usage`). `persists` flags
/// mirror the doc's column; the adapter test pins that correspondence.
const COMMANDS: &[NativeCommand] = &[
    NativeCommand {
        name: "/model",
        inject: "/model",
        description: "Set the AI model (choice sticks as your default)",
        args_hint: Some("model or alias: fable, opus, sonnet — empty opens the picker"),
        persists: true,
    },
    NativeCommand {
        name: "/effort",
        inject: "/effort",
        description: "Set effort level for model usage",
        args_hint: Some("low | medium | high | xhigh | max | ultracode | auto"),
        persists: true,
    },
    NativeCommand {
        name: "/advisor",
        inject: "/advisor",
        description: "Let Claude consult a stronger model at key moments",
        args_hint: Some("opus | sonnet | fable | off"),
        persists: false,
    },
    NativeCommand {
        name: "/resume",
        inject: "/resume",
        description: "Resume a previous conversation",
        args_hint: Some("session id or name — empty opens the picker"),
        persists: false,
    },
    NativeCommand {
        name: "/branch",
        inject: "/branch",
        description: "Create a branch of the conversation at this point",
        args_hint: Some("branch name"),
        persists: false,
    },
    NativeCommand {
        name: "/fork",
        inject: "/fork",
        description: "Spawn a background agent that inherits the full conversation",
        args_hint: Some("directive for the forked agent"),
        persists: false,
    },
    NativeCommand {
        name: "/rewind",
        inject: "/rewind",
        description: "Restore the code and/or conversation to a previous point",
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
        name: "/compact",
        inject: "/compact",
        description: "Free up context by summarizing the conversation",
        args_hint: Some("optional focus instructions"),
        persists: false,
    },
    NativeCommand {
        name: "/context",
        inject: "/context",
        description: "Visualize current context usage as a colored grid",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/usage",
        inject: "/usage",
        description: "Show session cost, plan usage, and activity stats (alias /cost)",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/status",
        inject: "/status",
        description: "Show version, model, account, and connectivity status",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/tasks",
        inject: "/tasks",
        description: "View and manage everything running in the background",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/background",
        inject: "/background",
        description: "Send this session to the background and free the terminal",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/goal",
        inject: "/goal",
        description: "Set a goal Claude checks before stopping",
        args_hint: Some("goal condition, or: clear"),
        persists: false,
    },
    NativeCommand {
        name: "/btw",
        inject: "/btw",
        description: "Ask a side question without interrupting the conversation",
        args_hint: Some("question"),
        persists: false,
    },
    NativeCommand {
        name: "/plan",
        inject: "/plan",
        description: "Enable plan mode or view the current session plan",
        args_hint: None,
        persists: false,
    },
    NativeCommand {
        name: "/permissions",
        inject: "/permissions",
        description: "Manage allow/deny tool permission rules",
        args_hint: None,
        persists: true,
    },
    NativeCommand {
        name: "/memory",
        inject: "/memory",
        description: "Open a memory file in your editor",
        args_hint: None,
        persists: true,
    },
    NativeCommand {
        name: "/keybindings",
        inject: "/keybindings",
        description: "Open your keyboard shortcuts file",
        args_hint: None,
        persists: true,
    },
    NativeCommand {
        name: "/config",
        inject: "/config",
        description: "Open settings (alias /settings)",
        args_hint: None,
        persists: true,
    },
];

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
        // Launch-option decl (ADR-0009): offer a picker field only when the
        // installed binary's --help lists the flag (both present at 2.1.211).
        let launch = LaunchOptionsDecl {
            model: has("--model").then_some(MODEL_SUGGESTIONS),
            effort: has("--effort").then_some(EFFORT_LEVELS),
        };

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
            launch,
        })
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, opts: &LaunchOptions, cwd: &Path) -> Command {
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
        // Session-scoped flags, applied for every intent (ADR-0009).
        if let Some(model) = &opts.model {
            cmd.arg("--model").arg(model);
        }
        if let Some(effort) = &opts.effort {
            cmd.arg("--effort").arg(effort);
        }
        cmd.current_dir(cwd);
        cmd
    }

    fn command_table(&self) -> &'static [NativeCommand] {
        COMMANDS
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

impl ClaudeCode {
    /// Native background-agent reconciliation source (ADR-0002): `claude
    /// agents --json --all` lists both running and completed background
    /// sessions as JSON, and — per its own `--help` at 2.1.201 (confirmed
    /// locally 2026-07-05, see `docs/integrations/claude-code.md`) — doesn't
    /// require a TTY. Inherent (not part of `CliAdapter`) because this
    /// capability is Claude-Code-specific; no other adapter has an
    /// equivalent native supervisor.
    ///
    /// Read-only, one-shot, non-interactive: no stdin is written, nothing is
    /// sent to any interactive prompt. There is no documented field-level
    /// schema for the JSON this prints (only the prose above), so this
    /// method only resolves the *top-level* shape — bare array, or an object
    /// wrapping the array under `"agents"`/`"sessions"` — and hands the
    /// per-entry values back unparsed; `crate::app::reconcile::parse_agents_json`
    /// does the lenient per-entry extraction.
    pub fn list_background_agents(&self) -> Result<Vec<serde_json::Value>, AdapterError> {
        let output =
            super::command_output(self.binary(), &["agents", "--json", "--all"]).map_err(|e| {
                AdapterError::Probe(format!("claude agents --json --all failed to run: {e}"))
            })?;
        if !output.status.success() {
            return Err(AdapterError::Probe(format!(
                "claude agents --json --all exited with {:?}",
                output.status.code()
            )));
        }

        let raw = String::from_utf8_lossy(&output.stdout);
        let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            AdapterError::Probe(format!(
                "claude agents --json --all produced invalid JSON: {e}"
            ))
        })?;

        match value {
            serde_json::Value::Array(items) => Ok(items),
            serde_json::Value::Object(mut map) => {
                if let Some(serde_json::Value::Array(items)) = map.remove("agents") {
                    Ok(items)
                } else if let Some(serde_json::Value::Array(items)) = map.remove("sessions") {
                    Ok(items)
                } else {
                    Err(AdapterError::Probe(
                        "claude agents --json --all: object output had neither an \
                         'agents' nor a 'sessions' array"
                            .to_string(),
                    ))
                }
            }
            _ => Err(AdapterError::Probe(
                "claude agents --json --all: unexpected top-level JSON shape \
                 (not an array or object)"
                    .to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn opts(model: Option<&str>, effort: Option<&str>) -> LaunchOptions {
        LaunchOptions {
            model: model.map(String::from),
            effort: effort.map(String::from),
        }
    }

    #[test]
    fn fresh_with_hint_still_maps_to_session_id() {
        let intent = LaunchIntent::Fresh {
            session_id_hint: Some("abc-123".to_string()),
        };
        let cmd = ClaudeCode.interactive_cmd(&intent, &LaunchOptions::default(), Path::new("/tmp"));
        assert_eq!(cmd.get_program(), "claude");
        assert_eq!(argv(&cmd), ["--session-id", "abc-123"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/tmp")));
    }

    #[test]
    fn default_options_add_no_flags() {
        let intent = LaunchIntent::Fresh {
            session_id_hint: None,
        };
        let cmd = ClaudeCode.interactive_cmd(&intent, &LaunchOptions::default(), Path::new("/tmp"));
        assert!(argv(&cmd).is_empty());
    }

    #[test]
    fn model_and_effort_append_for_every_intent() {
        let o = opts(Some("opus"), Some("high"));
        let cases: [(LaunchIntent, &[&str]); 3] = [
            (
                LaunchIntent::Fresh {
                    session_id_hint: Some("abc".to_string()),
                },
                &["--session-id", "abc", "--model", "opus", "--effort", "high"],
            ),
            (
                LaunchIntent::Resume {
                    native_id: "xyz".to_string(),
                },
                &["--resume", "xyz", "--model", "opus", "--effort", "high"],
            ),
            (
                LaunchIntent::ContinueMostRecent,
                &["--continue", "--model", "opus", "--effort", "high"],
            ),
        ];
        for (intent, expected) in cases {
            let cmd = ClaudeCode.interactive_cmd(&intent, &o, Path::new("/tmp"));
            assert_eq!(argv(&cmd), expected);
        }
    }
}
