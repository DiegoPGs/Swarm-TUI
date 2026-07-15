# Architecture

*Design date 2026-07-04. Decisions referenced here are argued in `docs/adr/`; per-CLI
facts are sourced in `docs/integrations/`. This page describes how the pieces fit.*

## What swarm-tui is, in one paragraph

A single Rust binary presenting a tabbed terminal UI. One tab is the **home view**
(cross-agent roster, dispatch, broadcast). Every other tab is a **live interactive
session** of one underlying CLI — the real `claude`, `agy`, or `codex` process running
in a PTY that swarm-tui owns and renders. Between the UI and the tools sits one trait,
`CliAdapter`, with three implementations. Underneath everything is a thin SQLite
registry that remembers which native session ID lives behind which tab, so work can move
between headless dispatch and interactive attention without losing the thread.

## Components

| Component | Module | Responsibility | Knows about CLIs? |
| --- | --- | --- | --- |
| TUI shell | `src/app/` | tab bar, keymap, layout, rendering loop | no |
| Home view | `src/app/home.rs` | roster, task input, dispatch/broadcast UI, event timeline | no |
| Session view | `src/app/session_view.rs` | renders one PTY grid, forwards keystrokes | no |
| Task router | `src/core/task.rs` | maps a home-view task to target adapter(s) + guardrails | capability level only |
| Event bus | `src/core/events.rs` | normalized `AgentEvent` stream fanned into UI + registry | no |
| Adapters | `src/adapters/` | everything tool-specific: flags, spawning, parsing, probing | **yes — only here** |
| PTY host | `src/pty/` | spawn/resize/kill PTY children, vt100 grid state (`tui-term`) | no |
| Registry | `src/store/` | SQLite: swarm-tui session ↔ native session ID + metadata | schema only |
| Capability probe | `src/adapters/mod.rs` | startup `--version`/`--help` checks → `AdapterCaps` | yes |

## Implementation status

As of this milestone (Stages A1–E, 2026-07-15): the registry (`src/store/`), the
PTY layer (`src/pty/`), the app shell (`src/app/`), and Claude Code reconciliation
(`src/app/reconcile.rs`) are real and implemented — `cargo run` boots a working
terminal shell with tabs, a home roster, and live PTY sessions. `CliAdapter::
dispatch()`/`follow_up()` (headless dispatch), broadcast, pipelines, and MCP
integration are still not implemented — deferred to a later milestone.

## The two channels (ADR-0001)

Every adapter exposes up to two channels; the interactive one is mandatory, the
programmatic one is capability-gated:

- **Interactive channel** — spawn the CLI's own TUI (`claude`, `agy`, `codex`, or their
  resume forms) inside a PTY. Universal: works for any terminal program, requires zero
  cooperation from the tool, and inherits its full UX including approval prompts. This
  is what a session tab is.
- **Programmatic channel** — a headless invocation per task, streaming events back:
  `claude -p --output-format stream-json`, `codex exec --json`, `agy -p` (plain text —
  see the Antigravity integration page). This is what the home view dispatches through.

The registry is the bridge: a task dispatched programmatically records its native
session ID; **promoting** it to a tab means the adapter spawns the interactive resume
form (`claude --resume <id>` / `codex resume <id>` / `agy --conversation <id>`) in a
fresh PTY. Demoting works the same way in reverse (send a follow-up headlessly into a
session that started interactively), with one caveat per tool documented in its
integration page.

## Task flow, end to end

1. **Input.** The user types a task in the home view and picks a target: one adapter,
   several (broadcast), or a saved routing rule. The router attaches guardrail defaults
   (below) and a working directory — cwd is part of the task, not ambient state, because
   Claude Code scopes session-ID resolution to the project directory.
2. **Dispatch.** `adapter.dispatch(task) -> EventStream`. The adapter builds the exact
   command line, spawns the child through tokio, and begins translating output.
   For Claude Code the adapter may instead hand long tasks to the native background
   supervisor (`claude --bg`) and poll `claude agents --json` — same events either way.
3. **Normalization.** Tool output becomes `AgentEvent`s on the bus:

   | `AgentEvent` | claude (stream-json) | codex (`--json` JSONL) | agy (plain text) |
   | --- | --- | --- | --- |
   | `Started { native_id }` | init message `session_id` (or pre-assigned via `--session-id`) | `thread.started` | synthesized; native id resolved post-hoc via conversation store — see integration page |
   | `AgentText` | assistant message chunks | `item.*` agent messages | stdout chunks |
   | `ToolActivity` | tool_use blocks | command/file/MCP items | not available |
   | `Completed { result, cost }` | `result` payload incl. `total_cost_usd` | `turn.completed` | process exit + captured stdout |
   | `Failed { reason }` | error subtype / nonzero exit | `turn.failed`, `error` | nonzero exit / `--print-timeout` |

4. **Fan-out.** The home timeline renders events live; the registry upserts the session
   record (`native_id`, status, last activity) on `Started`/`Completed`/`Failed`.
5. **Promotion (optional).** From the roster the user opens the session as a tab; the
   adapter's `attach(record)` spawns the interactive resume command in a PTY and the
   session view takes over rendering. From here the underlying CLI's own approval and
   subagent UX applies untouched.
6. **Exit.** Tab close kills only the PTY child (the native session transcript persists
   in the tool's own store); registry marks the record idle, resumable later.

## Guardrail defaults for headless dispatch

Interactive tabs need no orchestrator guardrails — the CLI's own prompts run. Headless
dispatch is where an unattended agent can act, so the router applies conservative
defaults per adapter, overridable per task:

| | claude | codex | agy |
| --- | --- | --- | --- |
| default posture | `--permission-mode plan` for exploratory; `acceptEdits` + `--allowedTools` allowlist for build tasks | inherit `codex exec` read-only sandbox; `--sandbox workspace-write` only on request | default `request-review` is interactive-shaped; pair `-p` with read-oriented tasks until permission behavior under `-p` is verified locally |
| hard stops | `--max-turns`, `--max-budget-usd` | sandbox policy; `approval_policy=never` implies sandbox trust | `--print-timeout` (default 5m) |
| never by default | `bypassPermissions` | `danger-full-access` | `--dangerously-skip-permissions` / `always-proceed` |

## Capability probe

At startup each adapter runs `<tool> --version` and greps `<tool> --help` for the flags
it depends on, producing an `AdapterCaps` (headless? structured output? resume-by-id?
background supervisor?). A failed probe never disables an adapter — it degrades it to
interactive-only with a visible badge in the roster. This is the mechanism that keeps
swarm-tui honest as all three tools ship weekly, and it's what makes a minimum viable
fourth adapter "a spawn command plus a probe" (ADR-0006).

## Failure modes considered

- **CLI missing / renamed / breaking flag change** → probe downgrade + badge; the
  integration doc records the last version tested.
- **vt100 rendering defects for a fancy TUI** → ADR-0003 fallback ladder
  (`vt100` widget → `wezterm-term` → tmux-backend behind the same `PaneHost` seam).
- **Orchestrator crash** → registry is durable; native transcripts are the source of
  truth; on restart, reconciliation re-reads `claude agents --json` and the native
  session stores (read-only) to re-adopt live/background sessions.
- **Two tabs on one native session** → registry enforces one live handle per native ID;
  a second open offers fork instead (`--fork-session` / `codex fork` — the latter is
  TUI-only, which is fine, since promotion is interactive by definition; agy `/fork`).
  **Not implemented yet in this milestone** — the fork-offer described here doesn't
  exist; nothing currently prevents two tabs from pointing at the same `native_id`;
  deferred.
- **Credential exposure** → adapters are forbidden (AGENTS.md boundary + code review
  rule) from opening credential files; the probe checks path existence only.

## What v1 explicitly does not do

Parse or replay native transcript files for display (tabs show live PTYs; history lives
in each tool), cross-agent shared context injection, automatic task decomposition, or
reaching into any CLI's internal subagent system (ADR-0004).
