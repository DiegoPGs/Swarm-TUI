# Architecture

*Design date 2026-07-04. Decisions referenced here are argued in `docs/adr/`; per-CLI
facts are sourced in `docs/integrations/`. This page describes how the pieces fit.*

## What swarm-tui is, in one paragraph

A single Rust binary presenting a tabbed terminal UI. One tab is the **home view**
(cross-agent roster, dispatch, broadcast). Every other tab is a **live interactive
session** of one underlying CLI — the real `claude` or `agy` process running in a PTY
that swarm-tui owns and renders (the `codex` integration is suspended — ADR-0008).
Between the UI and the tools sits one trait, `CliAdapter`, with one implementation
per tool (two active, one suspended-but-compiled). Underneath everything is a thin
SQLite registry that remembers which native session ID lives behind which tab, so
work can move between headless dispatch and interactive attention without losing the
thread.

## Components

| Component | Module | Responsibility | Knows about CLIs? |
| --- | --- | --- | --- |
| TUI shell | `src/app/` | tab bar, keymap, command palette, layout, rendering loop | no |
| Home view | `src/app/home.rs` | roster, task input, dispatch/broadcast UI, event timeline | no |
| Resources view | `src/app/usage.rs` | per-vendor usage captures + hidden probe panes (ADR-0011) | no (concept key `/usage` only) |
| Swarm plan | `src/core/plan.rs` | load/validate `.swarm/swarm.json` roles (ADR-0010) | no (slug lists passed in) |
| Startup queue | `src/app/startup.rs` | role startup-command injection: paint wait, echo guard, confirm | no |
| Session view | `src/app/session_view.rs` | renders one PTY grid, forwards keystrokes | no |
| Task router | `src/core/task.rs` | maps a home-view task to target adapter(s) + guardrails | capability level only |
| Event bus | `src/core/events.rs` | normalized `AgentEvent` stream fanned into UI + registry | no |
| Adapters | `src/adapters/` | everything tool-specific: flags, spawning, parsing, probing | **yes — only here** |
| PTY host | `src/pty/` | spawn/resize/kill PTY children, vt100 grid state (`tui-term`) | no |
| Registry | `src/store/` | SQLite: swarm-tui session ↔ native session ID + metadata | schema only |
| Capability probe | `src/adapters/mod.rs` | startup `--version`/`--help` checks → `AdapterCaps` | yes |

## The command plane (ADR-0007, ADR-0009)

Input routing reserves exactly one key: **Ctrl-Space**, the one-shot prefix
(ADR-0007). Everything else flows to the focused surface — the wrapped TUI on a
session tab, row navigation on Home. On top of that sit three layers (ADR-0009):

- **Layer 0 — native passthrough.** Slash commands typed inside a pane go straight
  to the tool; swarm-tui never intercepts or parses them.
- **Layer 1 — command palette.** Prefix + `:` on a session tab lists that tool's
  locally-verified native commands (`CliAdapter::command_table()`, populated from
  `docs/integrations/command-surfaces.md`) and injects the selection — command text
  plus carriage return — through the same write path as ordinary keystrokes; the
  tool's own UI takes over. Entries with cross-session effects carry a `[persists]`
  badge; entries declaring an args hint offer a free-text argument line first.
- **Layer 2 — launch options.** The new-session picker renders a per-tool options
  form (model / effort) declared by `AdapterCaps.launch` — probe-gated on the
  installed binary's `--help`, so upstream flag drift hides a field instead of
  breaking a spawn. Choices persist on the session row (registry schema v2) and
  show in the roster and the session status line.

## The swarm plan and the Resources view (ADR-0010, ADR-0011)

Milestone 2c adds workspace resource planning on top of the command plane:

- **Roles** (`.swarm/swarm.json`, committed and shareable): named launch presets —
  tool, model/effort (verbatim, no translation), purpose, startup commands. The
  picker lists roles above the raw tools; selecting one spawns through the layer-2
  launch path and then injects the declared startup commands, each gated on a
  stable first paint and a type→verify-echo→Enter sequence so a character-swallowing
  modal can never receive a blind Enter. Commands matching a `persists: true`
  command-table entry pause for a y/n confirm (new in 2c — the attended palette
  stays badge-only per ADR-0009). Roles are presets, not enforcement; the registry's
  `role` column (schema v3) records what was requested. `core/plan.rs` validates
  strictly (unknown/suspended tools, non-slash commands, versions) and a broken file
  degrades to a one-line picker error, never a crash.
- **Resources view** (prefix+`u`, Home tab body): one block per active vendor —
  the plan's role assignments, the vendor's own usage screen captured VERBATIM, a
  best-effort `%` headline, and a relative "as of Xm ago" timestamp. Refresh is
  manual only (digit per vendor): it spawns a hidden, unregistered, short-lived
  probe pane, injects the vendor's own `/usage` (same echo guard), snapshots the
  vt100 grid, and kills the pane. No machine-readable usage surface exists on
  either vendor (integration pages, 2026-07-16), so the tools' own words are the
  interface — swarm-tui never parses or normalizes them (claude reports % used,
  agy % available; normalizing would drift).

## Implementation status

As of this milestone (Stages A1–E, 2026-07-15): the registry (`src/store/`), the
PTY layer (`src/pty/`), the app shell (`src/app/`), and Claude Code reconciliation
(`src/app/reconcile.rs`) are real and implemented — `cargo run` boots a working
terminal shell with tabs, a home roster, and live PTY sessions. `CliAdapter::
dispatch()`/`follow_up()` (headless dispatch), broadcast, pipelines, and MCP
integration are still not implemented — deferred to a later milestone.

**Codex CLI is suspended as of 2026-07-16 (ADR-0008):** its adapter stays compiled
(enum variant, module, dispatch arms all intact) but `registry()` no longer lists
it, so it is never probed, offered in the new-session picker, or spawned. Historical
`tool = "codex"` registry rows render read-only in the roster. Codex mentions
elsewhere on this page (channel tables, guardrail defaults, failure modes) remain as
recorded design for the reversal path.

**Milestone 2b (the command plane) is implemented as of 2026-07-16:** the command
palette, per-tool launch options, and registry schema v2 (`model`/`effort` columns
with a real v1→v2 migration) are live — see "The command plane" above and
ADR-0009.

**Milestone 2c (the swarm plan) is implemented as of 2026-07-16:** the
`.swarm/swarm.json` roles file with picker integration and startup-command
injection, the prefix+`u` Resources view with hidden usage probes, and registry
schema v3 (`role` column; v1 databases migrate through a v1→v2→v3 chain) — see
"The swarm plan and the Resources view" above, ADR-0010, and ADR-0011.

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
