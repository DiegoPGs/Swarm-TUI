//! The adapter boundary (ADR-0006). **Everything CLI-specific lives at or below
//! this module.** `app`, `core`, `pty`, `store` know tools only as opaque slugs.
//!
//! Two channels per tool (ADR-0001):
//! - *Interactive*: `interactive_cmd()` → a `Command` the PTY layer spawns into
//!   a tab. Mandatory for every adapter.
//! - *Programmatic*: `dispatch()` / `follow_up()` → normalized `AgentEvent`
//!   streams. Capability-gated; the default impls say "unsupported".
//!
//! Minimum viable adapter = `id`/`display_name`/`binary` + `probe` +
//! `interactive_cmd`. That alone earns a tab.

pub mod antigravity;
pub mod claude_code;
pub mod codex;

use std::collections::VecDeque;
use std::io::BufRead;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::core::events::AgentEvent;
use crate::core::session::SessionRecord;
use crate::core::task::Task;

/// What a tool can do, expressed as **data** — probed at startup, cached, and
/// used by the Home view to enable/disable actions. A failed probe downgrades
/// a tool to interactive-only; it never removes the tool (ARCHITECTURE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCaps {
    pub structured_output: StructuredOutput,
    pub resume: ResumeSupport,
    /// Tool ships its own background-session supervisor
    /// (Claude Code `--bg` / `claude agents`). ADR-0002 reconciles with it.
    pub background_supervisor: bool,
    /// Which launch options the new-session picker may offer for this tool
    /// (ADR-0009). Set by `probe()` gated on the flag actually appearing in
    /// the installed binary's `--help`, so upstream drift degrades to "field
    /// hidden in the picker", never a broken spawn.
    pub launch: LaunchOptionsDecl,
    /// At most one headless dispatch of this tool at a time (ADR-0013;
    /// ADR-0002's serialized agy lane — its `-c`-based follow-up story only
    /// works if "most recent conversation" is unambiguous). The app enforces
    /// it; caps-driven so the app never branches on adapter identity.
    pub serial_dispatch: bool,
}

/// Per-tool declaration of the launch options `interactive_cmd` can map
/// (ADR-0009). `Some` means the flag exists on the installed binary; the slice
/// carries UI suggestions — alias suggestions for the free-text model field,
/// the fixed level list for effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchOptionsDecl {
    pub model: Option<&'static [&'static str]>,
    pub effort: Option<&'static [&'static str]>,
}

impl LaunchOptionsDecl {
    pub const NONE: LaunchOptionsDecl = LaunchOptionsDecl {
        model: None,
        effort: None,
    };

    /// Whether the picker has anything to ask for this tool.
    pub fn any(&self) -> bool {
        self.model.is_some() || self.effort.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredOutput {
    /// Plain text only (agy as verified at v1.0.16) — events are synthesized.
    None,
    /// Single JSON document at end of run.
    Json,
    /// Incremental structured events (claude `stream-json`, codex `--json`).
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeSupport {
    /// Resume any session by its native id (all three, per current research).
    ById,
    /// Only "continue most recent" is available.
    ContinueOnly,
    None,
}

/// Why a tab/PTY is being opened. Produced by the Home view & tab manager.
pub enum LaunchIntent {
    /// `session_id_hint` lets the registry pre-assign a native id before the
    /// process starts (ADR-0002); only Claude Code's `--session-id` acts on
    /// it today, the other adapters ignore it.
    Fresh {
        session_id_hint: Option<String>,
    },
    Resume {
        native_id: String,
    },
    ContinueMostRecent,
}

/// User-chosen launch options for an interactive spawn (ADR-0009). Data only —
/// each adapter maps the options it supports to its own flags and silently
/// ignores the rest. Persisted on the session row (schema v2) so the roster
/// can show what a session was launched with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// One entry in an adapter's declarative command table (ADR-0009): a native
/// slash command the palette may inject into that tool's pane. Populated only
/// from commands verified ✅ locally in
/// `docs/integrations/command-surfaces.md` — an injected command executes on
/// arrival, so a stale guess types a wrong command into a live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeCommand {
    /// As the tool's own menu shows it, e.g. `/model`.
    pub name: &'static str,
    /// Exact text typed into the pane (usually == `name`; kept separate so an
    /// autocomplete-interference workaround can alter bytes without renaming).
    pub inject: &'static str,
    pub description: &'static str,
    /// When `Some`, the palette offers a free-text argument line first; the
    /// hint describes what the tool expects after the command.
    pub args_hint: Option<&'static str>,
    /// The command's effect outlives the session (writes tool state/config) —
    /// rendered as a `[persists]` badge.
    pub persists: bool,
}

/// A running headless dispatch: normalized events plus a stop lever
/// (ADR-0013). The spawning adapter's reader thread owns the child — it
/// parses stdout to EOF, reaps the exit status, and synthesizes the terminal
/// event (per-tool failure semantics stay inside the adapter) — so the app
/// only ever polls `events` on its render tick and, at most, fires `kill`.
/// Decided in ADR-0013: this stays a std `mpsc` receiver polled by the tick;
/// no async trait.
#[derive(Debug)]
pub struct DispatchHandle {
    pub events: Receiver<AgentEvent>,
    pub kill: DispatchKill,
}

/// Best-effort stop for a dispatched child. Cloneable so the app can hold
/// one while the reader thread keeps reaping through the same shared child.
#[derive(Debug, Clone)]
pub struct DispatchKill(Arc<Mutex<Child>>);

impl DispatchKill {
    /// Best-effort kill; the reader thread still observes EOF, reaps, and
    /// emits the terminal event — callers never reap themselves.
    pub fn kill(&self) {
        let _ = lock_ignore_poison(&self.0).kill();
    }
}

/// Same poison stance as the PTY layer (findings-ledger F-003): a panicking
/// holder degrades that dispatch, it doesn't cascade.
fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Per-tool stdout translation for `spawn_streaming` (ADR-0013): one
/// implementation per adapter, holding whatever state it needs (e.g. claude
/// tracks whether the stream's own `result` line already carried the
/// terminal event). Tool-agnostic — tests drive the runner with `sh` fakes,
/// never a wrapped CLI.
pub(crate) trait StreamParser: Send + 'static {
    /// One stdout line, trailing newline removed.
    fn on_line(&mut self, line: &str, tx: &Sender<AgentEvent>);
    /// Process ended. `success` is the reaped exit status; `None` only when
    /// the child ignored stdout-EOF for 60s and had to be force-killed
    /// without yielding a status. `stderr_tail` carries the last captured
    /// stderr lines for failure reasons.
    fn on_exit(&mut self, success: Option<bool>, stderr_tail: &str, tx: &Sender<AgentEvent>);
}

/// Spawn `cmd` (env-scrubbed, stdin closed, stdout/stderr piped) and stream
/// `parser`'s events from a dedicated reader thread (ADR-0013). stderr is
/// drained concurrently — a full pipe would wedge the child — keeping a
/// bounded tail of last lines for `on_exit`'s failure reason.
pub(crate) fn spawn_streaming(
    mut cmd: Command,
    mut parser: impl StreamParser,
) -> Result<DispatchHandle, AdapterError> {
    scrub_env(&mut cmd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(AdapterError::Spawn)?;
    let stdout = child.stdout.take().expect("stdout was piped above");
    let stderr = child.stderr.take().expect("stderr was piped above");
    let child = Arc::new(Mutex::new(child));
    let (tx, rx) = std::sync::mpsc::channel();

    let stderr_tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let tail_writer = Arc::clone(&stderr_tail);
    let drainer = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let mut tail = lock_ignore_poison(&tail_writer);
                    tail.push_back(line.trim_end_matches(['\n', '\r']).to_string());
                    if tail.len() > 20 {
                        tail.pop_front();
                    }
                }
            }
        }
    });

    let reader_child = Arc::clone(&child);
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => parser.on_line(line.trim_end_matches(['\n', '\r']), &tx),
            }
        }
        // EOF: reap WITHOUT holding the lock across a blocking wait, so
        // `DispatchKill` can always cut in (the same EOF-vs-exit race shape
        // the PTY layer polls through — NOTES.md, milestone 2b).
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut success = None;
        loop {
            match lock_ignore_poison(&reader_child).try_wait() {
                Ok(Some(status)) => {
                    success = Some(status.success());
                    break;
                }
                Ok(None) => {}
                Err(_) => break,
            }
            if Instant::now() >= deadline {
                // stdout closed but the process lingers: stop it, then give
                // it one short reap window.
                let _ = lock_ignore_poison(&reader_child).kill();
                for _ in 0..100 {
                    if let Ok(Some(status)) = lock_ignore_poison(&reader_child).try_wait() {
                        success = Some(status.success());
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        // Child is reaped (or force-killed), so stderr has hit EOF and the
        // drainer is finishing — join it for a complete tail.
        let _ = drainer.join();
        let tail = lock_ignore_poison(&stderr_tail)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        parser.on_exit(success, &tail, &tx);
    });

    Ok(DispatchHandle {
        events: rx,
        kill: DispatchKill(child),
    })
}

/// AGENTS.md gotcha, subprocess edition (the PTY layer carries its own copy
/// for portable-pty's `CommandBuilder`): a wrapped CLI spawned from inside
/// another Claude Code session inherits `CLAUDECODE`/`CLAUDE_CODE_*` and
/// changes behavior. Every adapter subprocess must look like a plain user
/// terminal invocation.
pub(crate) fn scrub_env(cmd: &mut Command) {
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    for (key, _) in std::env::vars_os() {
        if let Some(name) = key.to_str() {
            if name.starts_with("CLAUDE_CODE_") {
                cmd.env_remove(name);
            }
        }
    }
}

#[derive(Debug)]
pub enum AdapterError {
    /// The tool has no programmatic channel for this operation.
    Unsupported(&'static str),
    Spawn(std::io::Error),
    Probe(String),
}

pub trait CliAdapter {
    /// Stable slug used in the registry (`SessionRecord::tool`).
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// Binary name resolved via PATH. Never a hardcoded absolute path: we run
    /// exactly what the user's shell would run (reuse-existing-install rule).
    fn binary(&self) -> &'static str;

    /// Read-only capability probe: `--version` + `--help` greps, nothing else.
    /// Must never touch config/auth files (AGENTS.md boundary).
    fn probe(&self) -> Result<AdapterCaps, AdapterError>;

    /// Command that opens the tool's own TUI for a tab. `opts` carries the
    /// user's launch choices (ADR-0009): map the supported ones to flags,
    /// silently ignore the rest. Options apply to every intent — the flags
    /// are session-scoped on resume too.
    fn interactive_cmd(&self, intent: &LaunchIntent, opts: &LaunchOptions, cwd: &Path) -> Command;

    /// This tool's palette-injectable native commands (ADR-0009). Default
    /// empty: a minimum-viable or suspended adapter needs no override.
    fn command_table(&self) -> &'static [NativeCommand] {
        &[]
    }

    /// Headless one-shot in `task.cwd`, translating `task.budget` into the
    /// tool's native guardrails (ARCHITECTURE guardrail table).
    fn dispatch(&self, _task: &Task) -> Result<DispatchHandle, AdapterError> {
        Err(AdapterError::Unsupported("headless dispatch"))
    }

    /// Headless follow-up into an existing session (`session.native_id`).
    fn follow_up(
        &self,
        _session: &SessionRecord,
        _task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        Err(AdapterError::Unsupported("headless follow-up"))
    }
}

/// Compile-time dispatch over the built-in adapters — no `dyn`, no
/// `async-trait` (ADR-0006). Adding a tool = add a variant + module; the
/// exhaustive matches below make every missing integration a compile error.
///
/// `Hash` is derived so `app` can key a probe-result cache by kind
/// (`HashMap<AdapterKind, Result<AdapterCaps, AdapterError>>`) — this is the
/// one narrow, pre-authorized exception to "app never names a specific CLI":
/// `app` matches on `AdapterKind` only to decide the claude native_id-hint
/// prepopulation, never to branch on flags/behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdapterKind {
    ClaudeCode,
    Antigravity,
    Codex,
}

/// The adapter registry: iterate this to probe/list every **active** tool.
///
/// Codex is suspended (ADR-0008): its variant, module, and dispatch arms stay
/// compiled — reversal is restoring one entry here — but nothing iterates it,
/// so it is never probed, offered in the picker, or spawned.
pub fn registry() -> &'static [AdapterKind] {
    &[AdapterKind::ClaudeCode, AdapterKind::Antigravity]
}

/// Every **compiled** adapter, suspended ones included. Exists so callers can
/// tell "suspended tool" (in here, not in `registry()`) apart from "unknown
/// tool" without naming any slug themselves — the swarm-plan loader's error
/// wording depends on it (ADR-0010). Reinstating codex (ADR-0008) touches
/// `registry()` only; this list already carries every variant and the
/// `all_kinds_lists_every_compiled_adapter` test keeps it honest.
pub fn all_kinds() -> &'static [AdapterKind] {
    &[
        AdapterKind::ClaudeCode,
        AdapterKind::Antigravity,
        AdapterKind::Codex,
    ]
}

impl AdapterKind {
    pub fn from_slug(slug: &str) -> Option<AdapterKind> {
        match slug {
            "claude-code" => Some(AdapterKind::ClaudeCode),
            "antigravity" => Some(AdapterKind::Antigravity),
            "codex" => Some(AdapterKind::Codex),
            _ => None,
        }
    }
}

impl CliAdapter for AdapterKind {
    fn id(&self) -> &'static str {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.id(),
            AdapterKind::Antigravity => antigravity::Antigravity.id(),
            AdapterKind::Codex => codex::Codex.id(),
        }
    }

    fn display_name(&self) -> &'static str {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.display_name(),
            AdapterKind::Antigravity => antigravity::Antigravity.display_name(),
            AdapterKind::Codex => codex::Codex.display_name(),
        }
    }

    fn binary(&self) -> &'static str {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.binary(),
            AdapterKind::Antigravity => antigravity::Antigravity.binary(),
            AdapterKind::Codex => codex::Codex.binary(),
        }
    }

    fn probe(&self) -> Result<AdapterCaps, AdapterError> {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.probe(),
            AdapterKind::Antigravity => antigravity::Antigravity.probe(),
            AdapterKind::Codex => codex::Codex.probe(),
        }
    }

    fn interactive_cmd(&self, intent: &LaunchIntent, opts: &LaunchOptions, cwd: &Path) -> Command {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.interactive_cmd(intent, opts, cwd),
            AdapterKind::Antigravity => antigravity::Antigravity.interactive_cmd(intent, opts, cwd),
            AdapterKind::Codex => codex::Codex.interactive_cmd(intent, opts, cwd),
        }
    }

    // Explicit dispatch is load-bearing here: without it, enum-dispatched
    // calls would silently hit the trait's default `&[]` and every palette
    // would be empty (pinned by claude_and_agy_command_tables_populated).
    fn command_table(&self) -> &'static [NativeCommand] {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.command_table(),
            AdapterKind::Antigravity => antigravity::Antigravity.command_table(),
            AdapterKind::Codex => codex::Codex.command_table(),
        }
    }

    fn dispatch(&self, task: &Task) -> Result<DispatchHandle, AdapterError> {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.dispatch(task),
            AdapterKind::Antigravity => antigravity::Antigravity.dispatch(task),
            AdapterKind::Codex => codex::Codex.dispatch(task),
        }
    }

    fn follow_up(
        &self,
        session: &SessionRecord,
        task: &Task,
    ) -> Result<DispatchHandle, AdapterError> {
        match self {
            AdapterKind::ClaudeCode => claude_code::ClaudeCode.follow_up(session, task),
            AdapterKind::Antigravity => antigravity::Antigravity.follow_up(session, task),
            AdapterKind::Codex => codex::Codex.follow_up(session, task),
        }
    }
}

/// Shared probe primitive: run `<binary> <args>` and capture the raw output.
/// Every adapter's `probe()` funnels through this — the only I/O the probe
/// path performs is `--version`/`--help` invocations (AGENTS.md boundary:
/// never touch credential/config file contents).
pub(crate) fn command_output(binary: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    // Probes and `claude agents --json` are subprocess invocations of a
    // wrapped CLI too — scrub them like dispatch children (ADR-0013).
    scrub_env(&mut cmd);
    cmd.output()
}

/// `--help`-shaped output as one string (stdout + stderr concatenated, since
/// some CLIs print help/usage to stderr): empty string if the command
/// couldn't even run, so callers can treat "not found" and "printed nothing"
/// uniformly as "no flags confirmed present".
pub(crate) fn help_text(binary: &str, args: &[&str]) -> String {
    match command_output(binary, args) {
        Ok(out) => format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(_) => String::new(),
    }
}

/// Test-only handle factory: a real receiver whose sender the test holds,
/// and a kill lever backed by a trivial already-exiting `sh` child — app
/// tests hand-feed events without ever spawning a wrapped CLI.
#[cfg(test)]
pub(crate) fn test_dispatch_handle(events: Receiver<AgentEvent>) -> DispatchHandle {
    let child = Command::new("sh")
        .arg("-c")
        .arg(":")
        .spawn()
        .expect("spawn trivial child for test handle");
    DispatchHandle {
        events,
        kill: DispatchKill(Arc::new(Mutex::new(child))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_excludes_codex_while_suspended() {
        assert_eq!(
            registry(),
            &[AdapterKind::ClaudeCode, AdapterKind::Antigravity]
        );
        // Reversal path (ADR-0008): the slug stays resolvable so historical
        // registry rows keep mapping to the compiled-but-suspended adapter.
        assert_eq!(AdapterKind::from_slug("codex"), Some(AdapterKind::Codex));
    }

    /// `all_kinds()` must list every compiled variant — the swarm-plan
    /// loader's suspended-vs-unknown distinction rides on it (ADR-0010), and
    /// a reinstated/added adapter that forgets this list would silently load
    /// as "unknown tool".
    #[test]
    fn all_kinds_lists_every_compiled_adapter() {
        assert_eq!(
            all_kinds(),
            &[
                AdapterKind::ClaudeCode,
                AdapterKind::Antigravity,
                AdapterKind::Codex,
            ]
        );
        for kind in all_kinds() {
            assert_eq!(AdapterKind::from_slug(kind.id()), Some(*kind));
        }
    }

    fn all_tables() -> Vec<(&'static str, &'static [NativeCommand])> {
        vec![
            ("claude-code", AdapterKind::ClaudeCode.command_table()),
            ("antigravity", AdapterKind::Antigravity.command_table()),
            ("codex", AdapterKind::Codex.command_table()),
        ]
    }

    #[test]
    fn command_tables_inject_start_with_slash() {
        for (tool, table) in all_tables() {
            for entry in table {
                assert!(
                    entry.name.starts_with('/') && entry.inject.starts_with('/'),
                    "{tool}: entry {:?} must name/inject a slash command",
                    entry.name
                );
                assert!(
                    !entry.description.is_empty(),
                    "{tool}: {} has an empty description",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn command_table_names_are_unique_per_tool() {
        for (tool, table) in all_tables() {
            let unique: HashSet<&str> = table.iter().map(|e| e.name).collect();
            assert_eq!(unique.len(), table.len(), "{tool}: duplicate entry names");
        }
    }

    /// Pins the load-bearing `AdapterKind::command_table` dispatch override —
    /// if it were dropped, enum-dispatched calls would silently return the
    /// trait default `&[]` and every palette would render empty.
    #[test]
    fn claude_and_agy_command_tables_populated() {
        assert!(!AdapterKind::ClaudeCode.command_table().is_empty());
        assert!(!AdapterKind::Antigravity.command_table().is_empty());
    }

    #[test]
    fn codex_command_table_is_empty_while_suspended() {
        // ADR-0008/0009: nothing codex is locally verifiable, so its table
        // stays the trait default until reversal.
        assert!(AdapterKind::Codex.command_table().is_empty());
    }

    /// A tiny tool-agnostic parser for exercising the streaming runner with
    /// plain `sh` (never a wrapped CLI): every line becomes `AgentText`, the
    /// exit becomes `Completed`/`Failed` with the stderr tail in the reason.
    struct EchoParser;

    impl StreamParser for EchoParser {
        fn on_line(&mut self, line: &str, tx: &Sender<AgentEvent>) {
            let _ = tx.send(AgentEvent::AgentText(line.to_string()));
        }
        fn on_exit(&mut self, success: Option<bool>, stderr_tail: &str, tx: &Sender<AgentEvent>) {
            let _ = tx.send(match success {
                Some(true) => AgentEvent::Completed {
                    result: String::new(),
                    cost_usd: None,
                },
                _ => AgentEvent::Failed {
                    reason: format!("stderr: {stderr_tail}"),
                },
            });
        }
    }

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[test]
    fn streaming_runner_forwards_lines_then_a_terminal_event() {
        let handle = spawn_streaming(sh("printf 'a\\nb\\n'"), EchoParser).expect("spawn");
        // `iter()` ends when the reader thread finishes and drops the sender.
        let events: Vec<AgentEvent> = handle.events.iter().collect();
        assert_eq!(events.len(), 3, "got: {events:?}");
        assert!(matches!(&events[0], AgentEvent::AgentText(t) if t == "a"));
        assert!(matches!(&events[1], AgentEvent::AgentText(t) if t == "b"));
        assert!(matches!(&events[2], AgentEvent::Completed { .. }));
    }

    #[test]
    fn streaming_runner_reports_failure_with_stderr_tail() {
        let handle = spawn_streaming(sh("echo oops >&2; exit 3"), EchoParser).expect("spawn");
        let events: Vec<AgentEvent> = handle.events.iter().collect();
        match events.last() {
            Some(AgentEvent::Failed { reason }) => {
                assert!(reason.contains("oops"), "stderr tail missing: {reason}")
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_kill_stops_a_running_child_promptly() {
        let started = Instant::now();
        let handle = spawn_streaming(sh("sleep 30"), EchoParser).expect("spawn");
        handle.kill.kill();
        let events: Vec<AgentEvent> = handle.events.iter().collect();
        assert!(
            matches!(events.last(), Some(AgentEvent::Failed { .. })),
            "killed child must land Failed, got {events:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "kill took {:?} — the 30s sleep won the race",
            started.elapsed()
        );
    }

    #[test]
    fn spawned_children_get_the_env_scrub() {
        // The caller "inherits" a Claude Code environment; the child must not.
        let mut cmd = sh("env");
        cmd.env("CLAUDECODE", "1");
        cmd.env("CLAUDE_CODE_ENTRYPOINT", "cli");
        let handle = spawn_streaming(cmd, EchoParser).expect("spawn");
        let lines: Vec<String> = handle
            .events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::AgentText(line) => Some(line),
                _ => None,
            })
            .collect();
        assert!(
            lines.iter().any(|l| l == "TERM=xterm-256color"),
            "plain TERM missing:\n{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with("CLAUDECODE=") || l.starts_with("CLAUDE_CODE_")),
            "Claude Code env leaked into a dispatch child:\n{lines:?}"
        );
    }

    /// The `persists` flags are copies of the ✅-verified "Persists" column in
    /// docs/integrations/command-surfaces.md (claude 2.1.211 / agy 1.1.3,
    /// 2026-07-16) — that doc is the single source of truth; update it first,
    /// then these sets, then the tables.
    #[test]
    fn persists_flags_match_command_surfaces_doc() {
        let expected: [(&str, &[&str]); 2] = [
            (
                "claude-code",
                &[
                    "/model",
                    "/effort",
                    "/permissions",
                    "/memory",
                    "/keybindings",
                    "/config",
                ],
            ),
            (
                "antigravity",
                &["/model", "/permissions", "/config", "/keybindings"],
            ),
        ];
        for (tool, expected_persistent) in expected {
            let kind = AdapterKind::from_slug(tool).unwrap();
            let mut actual: Vec<&str> = kind
                .command_table()
                .iter()
                .filter(|e| e.persists)
                .map(|e| e.name)
                .collect();
            actual.sort_unstable();
            let mut expected_sorted = expected_persistent.to_vec();
            expected_sorted.sort_unstable();
            assert_eq!(actual, expected_sorted, "{tool}: persists flags drifted");
        }
    }
}
