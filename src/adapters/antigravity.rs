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
use std::sync::mpsc::Sender;

use super::{
    AdapterCaps, AdapterError, CliAdapter, DispatchHandle, LaunchIntent, LaunchOptions,
    LaunchOptionsDecl, NativeCommand, ResumeSupport, StreamParser, StructuredOutput,
};
use crate::core::events::AgentEvent;
use crate::core::session::SessionRecord;
use crate::core::task::{DispatchPosture, Task};

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
    // ADR-0013: one agy headless run at a time — native-id backfill is ⬜,
    // so "most recent conversation" must stay unambiguous (ADR-0002 lane).
    serial_dispatch: true,
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
            serial_dispatch: EXPECTED_CAPS.serial_dispatch,
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

    /// Headless dispatch (ADR-0013): `agy -p` plain text with synthesized
    /// events. `Edits` posture is refused here — how `request-review`
    /// behaves under `-p` with no TTY is ⬜ (integration page; settling it
    /// is a supervised live run) — so only read/plan-shaped tasks flow.
    /// Whether `-p` creates a resumable conversation is equally ⬜: rows
    /// keep `native_id: None` until the owner's supervised backfill
    /// verification (ADR-0002 lane).
    fn dispatch(&self, task: &Task) -> Result<DispatchHandle, AdapterError> {
        if task.budget.posture == DispatchPosture::Edits {
            return Err(AdapterError::Unsupported(
                "agy headless edits: -p permission behavior is unverified — read/plan tasks only",
            ));
        }
        let cmd = self.headless_cmd(task);
        super::spawn_streaming(cmd, AgyTextParser::default())
    }

    /// ⬜ whether `--conversation <ID>` combines with `-p` at all — a
    /// supervised live verification (integration page). Refused until then.
    fn follow_up(
        &self,
        _session: &SessionRecord,
        _task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        Err(AdapterError::Unsupported(
            "agy headless follow-up: --conversation + -p is unverified (run supervised first)",
        ))
    }
}

impl Antigravity {
    /// Build the headless argv (ADR-0013) — split from `dispatch` so tests
    /// assert flags without spawning. `max_turns`/`max_usd` have no agy
    /// mechanism and are ignored; `--print-timeout` (Go duration syntax) is
    /// the one hard stop, left at the tool's 5m default unless the task
    /// tightens it.
    fn headless_cmd(&self, task: &Task) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("-p").arg(&task.prompt);
        if let Some(secs) = task.budget.timeout_secs {
            cmd.arg("--print-timeout").arg(format!("{secs}s"));
        }
        if let Some(model) = &task.model {
            cmd.arg("--model").arg(model);
        }
        // task.effort: no agy flag (ADR-0009) — ignored.
        cmd.current_dir(&task.cwd);
        cmd
    }
}

/// Plain-text → `AgentEvent` synthesis (ADR-0001/0013): `Started{None}` on
/// the first output line (agy reports no id up front — ADR-0002 backfill
/// lane), every line as `AgentText`, and the terminal event from the exit
/// status with a bounded tail of the output as the result.
#[derive(Default)]
struct AgyTextParser {
    started: bool,
    tail: Vec<String>,
}

impl AgyTextParser {
    const TAIL_LINES: usize = 100;
}

impl StreamParser for AgyTextParser {
    fn on_line(&mut self, line: &str, tx: &Sender<AgentEvent>) {
        if !self.started {
            self.started = true;
            let _ = tx.send(AgentEvent::Started { native_id: None });
        }
        if self.tail.len() == Self::TAIL_LINES {
            self.tail.remove(0);
        }
        self.tail.push(line.to_string());
        if !line.trim().is_empty() {
            let _ = tx.send(AgentEvent::AgentText(line.to_string()));
        }
    }

    fn on_exit(&mut self, success: Option<bool>, stderr_tail: &str, tx: &Sender<AgentEvent>) {
        let _ = tx.send(match success {
            Some(true) => AgentEvent::Completed {
                result: self.tail.join("\n"),
                cost_usd: None, // agy reports no cost on the -p channel
            },
            _ => AgentEvent::Failed {
                reason: if stderr_tail.is_empty() {
                    "agy -p exited with a failure".to_string()
                } else {
                    stderr_tail.to_string()
                },
            },
        });
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
    fn headless_argv_is_print_plus_optional_timeout_and_model() {
        // Defaults: bare -p, tool's own 5m timeout, no model.
        let cmd = Antigravity.headless_cmd(&task("summarize the readme"));
        assert_eq!(cmd.get_program(), "agy");
        assert_eq!(argv(&cmd), ["-p", "summarize the readme"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/tmp/repo")));

        // Tightened: timeout in Go-duration seconds + model verbatim;
        // turns/usd have no agy mechanism and must not appear.
        let mut t = task("scan the docs");
        t.budget.timeout_secs = Some(300);
        t.budget.max_turns = Some(9);
        t.budget.max_usd = Some(1.0);
        t.model = Some("gemini-3.1-pro".to_string());
        let cmd = Antigravity.headless_cmd(&t);
        assert_eq!(
            argv(&cmd),
            [
                "-p",
                "scan the docs",
                "--print-timeout",
                "300s",
                "--model",
                "gemini-3.1-pro",
            ]
        );
    }

    #[test]
    fn edits_posture_is_refused_and_follow_up_stays_unsupported() {
        let mut t = task("apply the fix");
        t.budget.posture = crate::core::task::DispatchPosture::Edits;
        match Antigravity.dispatch(&t) {
            Err(AdapterError::Unsupported(reason)) => {
                assert!(reason.contains("unverified"), "got: {reason}")
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }

        let record = SessionRecord {
            id: 1,
            tool: "antigravity".to_string(),
            native_id: Some("conv-9".to_string()),
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
        assert!(matches!(
            Antigravity.follow_up(&record, &task("more")),
            Err(AdapterError::Unsupported(_))
        ));
    }

    #[test]
    fn text_parser_synthesizes_started_text_and_terminal_events() {
        let run = |lines: &[&str], exit: Option<bool>, stderr: &str| -> Vec<AgentEvent> {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut parser = AgyTextParser::default();
            for line in lines {
                parser.on_line(line, &tx);
            }
            parser.on_exit(exit, stderr, &tx);
            drop(tx);
            rx.iter().collect()
        };

        let events = run(&["hello", "world"], Some(true), "");
        assert_eq!(events.len(), 4, "got {events:?}");
        assert!(matches!(
            &events[0],
            AgentEvent::Started { native_id: None }
        ));
        assert!(matches!(&events[1], AgentEvent::AgentText(t) if t == "hello"));
        assert!(matches!(&events[2], AgentEvent::AgentText(t) if t == "world"));
        match &events[3] {
            AgentEvent::Completed { result, cost_usd } => {
                assert_eq!(result, "hello\nworld");
                assert_eq!(*cost_usd, None);
            }
            other => panic!("expected Completed, got {other:?}"),
        }

        let events = run(&[], Some(false), "quota exceeded");
        assert_eq!(events.len(), 1, "no output ⇒ terminal only, got {events:?}");
        assert!(matches!(&events[0], AgentEvent::Failed { reason } if reason.contains("quota")));
    }

    #[test]
    fn model_maps_to_model_flag() {
        let intent = LaunchIntent::Fresh {
            session_id_hint: Some("ignored-by-agy".to_string()),
        };
        let opts = LaunchOptions {
            model: Some("Gemini 3.1 Pro (High)".to_string()),
            effort: None,
        };
        let cmd = Antigravity.interactive_cmd(&intent, &opts, Path::new("/tmp"));
        assert_eq!(cmd.get_program(), "agy");
        assert_eq!(argv(&cmd), ["--model", "Gemini 3.1 Pro (High)"]);
    }

    #[test]
    fn effort_is_silently_ignored() {
        let intent = LaunchIntent::Fresh {
            session_id_hint: None,
        };
        let opts = LaunchOptions {
            model: None,
            effort: Some("high".to_string()),
        };
        let cmd = Antigravity.interactive_cmd(&intent, &opts, Path::new("/tmp"));
        assert!(argv(&cmd).is_empty(), "agy has no effort flag to map");
    }

    #[test]
    fn resume_still_uses_conversation_flag_with_options_appended() {
        let intent = LaunchIntent::Resume {
            native_id: "conv-1".to_string(),
        };
        let opts = LaunchOptions {
            model: Some("Claude Opus 4.6 (Thinking)".to_string()),
            effort: None,
        };
        let cmd = Antigravity.interactive_cmd(&intent, &opts, Path::new("/tmp"));
        assert_eq!(
            argv(&cmd),
            [
                "--conversation",
                "conv-1",
                "--model",
                "Claude Opus 4.6 (Thinking)"
            ]
        );
    }
}
