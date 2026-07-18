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
use std::sync::mpsc::Sender;

use super::{
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, LaunchOptions,
    LaunchOptionsDecl, NativeCommand, ResumeSupport, StreamParser, StructuredOutput,
};
use crate::core::events::AgentEvent;
use crate::core::session::SessionRecord;
use crate::core::task::{DispatchPosture, Task};

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
    serial_dispatch: false,
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
            serial_dispatch: EXPECTED_CAPS.serial_dispatch,
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

    /// Headless dispatch (ADR-0013): `-p … stream-json` with a pre-assigned
    /// `--session-id` (ADR-0002 — the registry learns the id from `Started`,
    /// no output scraping). Gotcha: since 2.1.163 claude kills background
    /// Bash tools ~5s after the final result — dispatched tasks should not
    /// rely on them outliving the run.
    fn dispatch(&self, task: &Task) -> Result<DispatchHandle, AdapterError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let cmd = self.headless_cmd(
            task,
            HeadlessTarget::Fresh {
                session_id: &session_id,
            },
        );
        super::spawn_streaming(cmd, StreamJsonParser::new(session_id))
    }

    fn follow_up(
        &self,
        session: &SessionRecord,
        task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        let Some(native_id) = session.native_id.as_deref() else {
            return Err(AdapterError::Unsupported(
                "headless follow-up needs a native session id",
            ));
        };
        let cmd = self.headless_cmd(
            task,
            HeadlessTarget::Resume {
                native_id,
                cwd: &session.cwd,
            },
        );
        super::spawn_streaming(cmd, StreamJsonParser::new(native_id.to_string()))
    }
}

/// Which headless lane a command targets (ADR-0013).
enum HeadlessTarget<'a> {
    /// Fresh dispatch: pre-assign the native id, run in the task's cwd.
    Fresh { session_id: &'a str },
    /// Follow-up into an existing session: MUST run in the session's
    /// recorded cwd — `--resume` id lookup is scoped to it (AGENTS gotcha).
    Resume { native_id: &'a str, cwd: &'a Path },
}

impl ClaudeCode {
    /// Build the headless argv (ADR-0013 guardrail mapping) — split from
    /// `dispatch`/`follow_up` so tests assert flags without spawning.
    fn headless_cmd(&self, task: &Task, target: HeadlessTarget<'_>) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("-p").arg(&task.prompt);
        cmd.arg("--output-format").arg("stream-json");
        // Several stream options require --verbose (integration page).
        cmd.arg("--verbose");
        match target {
            HeadlessTarget::Fresh { session_id } => {
                cmd.arg("--session-id").arg(session_id);
                cmd.current_dir(&task.cwd);
            }
            HeadlessTarget::Resume { native_id, cwd } => {
                cmd.arg("--resume").arg(native_id);
                cmd.current_dir(cwd);
            }
        }
        match task.budget.posture {
            DispatchPosture::ReadOnly | DispatchPosture::Plan => {
                cmd.arg("--permission-mode").arg("plan");
            }
            DispatchPosture::Edits => {
                cmd.arg("--permission-mode").arg("acceptEdits");
                // The ARCHITECTURE table's "allowlist for build tasks",
                // made concrete (ADR-0013): file tools only — no Bash.
                cmd.arg("--allowedTools").arg("Read,Glob,Grep,Edit,Write");
            }
        }
        if let Some(turns) = task.budget.max_turns {
            cmd.arg("--max-turns").arg(turns.to_string());
        }
        if let Some(usd) = task.budget.max_usd {
            cmd.arg("--max-budget-usd").arg(usd.to_string());
        }
        // task.budget.timeout_secs: no claude mechanism — ignored (ADR-0013).
        if let Some(model) = &task.model {
            cmd.arg("--model").arg(model);
        }
        if let Some(effort) = &task.effort {
            cmd.arg("--effort").arg(effort);
        }
        cmd
    }
}

/// stream-json → `AgentEvent` (ADR-0013), defensive per the `reconcile.rs`
/// lenience pattern: malformed or unknown lines are skipped, never fatal.
/// Field names per the official headless docs (remote ✅ 2026-07-04):
/// `{"type":"system","subtype":"init","session_id":…}`, assistant messages
/// carrying `message.content[]` blocks (`text` / `tool_use`), and a final
/// `{"type":"result", …, "total_cost_usd":…}`. The committed fixture is
/// synthetic; the live shape re-check sits on the owner smoke checklist.
struct StreamJsonParser {
    /// Pre-assigned (`--session-id`) or resumed native id — the fallback
    /// when the stream never names one.
    native_id: String,
    started: bool,
    terminal: bool,
}

impl StreamJsonParser {
    fn new(native_id: String) -> Self {
        StreamJsonParser {
            native_id,
            started: false,
            terminal: false,
        }
    }

    fn start_once(&mut self, stream_id: Option<&str>, tx: &Sender<AgentEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let id = stream_id.unwrap_or(&self.native_id).to_string();
        let _ = tx.send(AgentEvent::Started {
            native_id: Some(id),
        });
    }
}

impl StreamParser for StreamJsonParser {
    fn on_line(&mut self, line: &str, tx: &Sender<AgentEvent>) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            // --verbose can interleave non-JSON noise; skip it quietly.
            return;
        };
        match value.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "system" => {
                if value.get("subtype").and_then(|v| v.as_str()) == Some("init") {
                    self.start_once(value.get("session_id").and_then(|v| v.as_str()), tx);
                }
            }
            "assistant" => {
                self.start_once(None, tx);
                let Some(blocks) = value.pointer("/message/content").and_then(|v| v.as_array())
                else {
                    return;
                };
                for block in blocks {
                    match block.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    let _ = tx.send(AgentEvent::AgentText(text.to_string()));
                                }
                            }
                        }
                        "tool_use" => {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                            let _ = tx.send(AgentEvent::ToolActivity(name.to_string()));
                        }
                        _ => {}
                    }
                }
            }
            "result" => {
                self.terminal = true;
                let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
                let is_error = value
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    || subtype.starts_with("error");
                let cost_usd = value.get("total_cost_usd").and_then(|v| v.as_f64());
                let result = value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let _ = tx.send(if is_error {
                    AgentEvent::Failed {
                        reason: if result.is_empty() {
                            format!("result: {subtype}")
                        } else {
                            result
                        },
                    }
                } else {
                    AgentEvent::Completed { result, cost_usd }
                });
            }
            // "user" tool-result echoes and future event types — skip.
            _ => {}
        }
    }

    fn on_exit(&mut self, success: Option<bool>, stderr_tail: &str, tx: &Sender<AgentEvent>) {
        if self.terminal {
            return; // the stream's own result line already carried the terminal
        }
        let _ = tx.send(match success {
            Some(true) => AgentEvent::Completed {
                result: String::new(),
                cost_usd: None,
            },
            _ => AgentEvent::Failed {
                reason: if stderr_tail.is_empty() {
                    "exited without a result event".to_string()
                } else {
                    stderr_tail.to_string()
                },
            },
        });
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

    fn task(prompt: &str) -> crate::core::task::Task {
        crate::core::task::Task {
            prompt: prompt.to_string(),
            cwd: std::path::PathBuf::from("/tmp/repo"),
            budget: crate::core::task::Budget::default(),
            model: None,
            effort: None,
        }
    }

    #[test]
    fn headless_dispatch_argv_defaults_to_plan_posture() {
        let cmd = ClaudeCode.headless_cmd(
            &task("review the diff"),
            HeadlessTarget::Fresh { session_id: "abc" },
        );
        assert_eq!(cmd.get_program(), "claude");
        assert_eq!(
            argv(&cmd),
            [
                "-p",
                "review the diff",
                "--output-format",
                "stream-json",
                "--verbose",
                "--session-id",
                "abc",
                "--permission-mode",
                "plan",
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/tmp/repo")));
    }

    #[test]
    fn headless_edits_posture_maps_to_accept_edits_plus_file_allowlist() {
        let mut t = task("fix the bug");
        t.budget.posture = DispatchPosture::Edits;
        t.budget.max_turns = Some(25);
        t.budget.max_usd = Some(2.5);
        t.budget.timeout_secs = Some(300); // no claude mechanism — must not appear
        t.model = Some("opus".to_string());
        t.effort = Some("high".to_string());
        let cmd = ClaudeCode.headless_cmd(&t, HeadlessTarget::Fresh { session_id: "abc" });
        assert_eq!(
            argv(&cmd),
            [
                "-p",
                "fix the bug",
                "--output-format",
                "stream-json",
                "--verbose",
                "--session-id",
                "abc",
                "--permission-mode",
                "acceptEdits",
                "--allowedTools",
                "Read,Glob,Grep,Edit,Write",
                "--max-turns",
                "25",
                "--max-budget-usd",
                "2.5",
                "--model",
                "opus",
                "--effort",
                "high",
            ]
        );
    }

    #[test]
    fn headless_follow_up_resumes_in_the_sessions_recorded_cwd() {
        // The task says /tmp/repo; the session record says /home/user/other —
        // the record must win (cwd-scoped --resume lookup, AGENTS gotcha).
        let cmd = ClaudeCode.headless_cmd(
            &task("continue"),
            HeadlessTarget::Resume {
                native_id: "xyz-1",
                cwd: Path::new("/home/user/other"),
            },
        );
        let args = argv(&cmd);
        assert!(args.windows(2).any(|w| w == ["--resume", "xyz-1"]));
        assert!(!args.iter().any(|a| a == "--session-id"));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/home/user/other")));
    }

    #[test]
    fn follow_up_without_a_native_id_is_refused_before_spawning() {
        let record = SessionRecord {
            id: 1,
            tool: "claude-code".to_string(),
            native_id: None,
            name: None,
            cwd: std::path::PathBuf::from("/tmp"),
            mode: crate::core::session::SessionMode::Headless,
            status: crate::core::session::SessionStatus::Completed,
            created_at: std::time::SystemTime::now(),
            updated_at: std::time::SystemTime::now(),
            cost_usd: None,
            model: None,
            effort: None,
            role: None,
        };
        match ClaudeCode.follow_up(&record, &task("more")) {
            Err(AdapterError::Unsupported(_)) => {}
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    const STREAM_FIXTURE: &str =
        include_str!("../../tests/fixtures/claude_stream.synthetic.ndjson");

    fn run_parser(lines: &str, exit: Option<bool>, stderr_tail: &str) -> Vec<AgentEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut parser = StreamJsonParser::new("pre-assigned".to_string());
        for line in lines.lines() {
            parser.on_line(line, &tx);
        }
        parser.on_exit(exit, stderr_tail, &tx);
        drop(tx);
        rx.iter().collect()
    }

    #[test]
    fn stream_json_parser_translates_the_fixture() {
        let events = run_parser(STREAM_FIXTURE, Some(true), "");
        // Started (id from init), text, tool, text, Completed — the noise
        // and unknown-type lines vanish, and the stream's own result line
        // suppresses the exit-synthesized terminal.
        assert_eq!(events.len(), 5, "got {events:?}");
        assert!(matches!(
            &events[0],
            AgentEvent::Started { native_id: Some(id) }
                if id == "11111111-2222-4333-8444-555555555555"
        ));
        assert!(matches!(&events[1], AgentEvent::AgentText(t) if t.contains("repo layout")));
        assert!(matches!(&events[2], AgentEvent::ToolActivity(t) if t == "Read"));
        assert!(matches!(&events[3], AgentEvent::AgentText(t) if t.contains("two findings")));
        match &events[4] {
            AgentEvent::Completed { result, cost_usd } => {
                assert!(result.contains("Two findings"));
                assert_eq!(*cost_usd, Some(0.0421));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn stream_json_parser_maps_error_results_and_exit_fallbacks() {
        // An error-subtype result → Failed, still suppressing on_exit.
        let events = run_parser(
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}"#,
            Some(true),
            "",
        );
        assert_eq!(events.len(), 1, "got {events:?}");
        assert!(
            matches!(&events[0], AgentEvent::Failed { reason } if reason.contains("error_max_turns"))
        );

        // No result line + nonzero exit → Failed carrying the stderr tail.
        let events = run_parser("", Some(false), "boom: bad flag");
        assert_eq!(events.len(), 1, "got {events:?}");
        assert!(matches!(&events[0], AgentEvent::Failed { reason } if reason.contains("boom")));

        // No result line + clean exit → synthesized Completed.
        let events = run_parser("", Some(true), "");
        assert!(matches!(&events[0], AgentEvent::Completed { .. }));
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
