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

use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc::Receiver;

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

/// A running headless dispatch: normalized events plus the child to reap.
/// TODO(next session): becomes an async stream once tokio lands (ADR-0005);
/// the std `mpsc` shape here exists only to pin the boundary dependency-free.
pub struct DispatchHandle {
    pub events: Receiver<AgentEvent>,
    pub child: Child,
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
    Command::new(binary).args(args).output()
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
