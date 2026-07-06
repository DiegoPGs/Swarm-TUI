# ADR-0002: Session model — thin registry over native session stores

- Status: Accepted (2026-07-04)

## Context

All three CLIs persist sessions and can resume them: Claude Code by ID or name
(`--resume`, cwd-scoped; JSON output returns `session_id`; `--session-id` lets a caller
pre-assign one), Codex by ID (`codex resume` / `codex exec resume`, rollout JSONL under
`~/.codex/sessions/`), Antigravity by ID (`--conversation`, SQLite store). The
orchestrator must answer questions none of them can individually: *what sessions exist
across all three tools, which tab is which, what was dispatched from the home view and
what did it cost.*

## Decision

swarm-tui keeps a **thin registry** (SQLite via `rusqlite`, one table to start) that
maps an swarm-tui session to `(tool, native_session_id, cwd, name, mode, status,
created_at, last_activity, last_cost_usd)` plus a dispatch-history table for the home
timeline. **Native stores remain the single source of truth for transcripts and
conversation state**; the registry never copies message content. Continuity is always
delegated to native resume-by-ID. On startup, a read-only reconciliation pass re-adopts
sessions the registry knows about and discovers live Claude background sessions via
`claude agents --json`.

Where a tool allows it, the orchestrator makes the mapping trivial by construction:
Claude sessions are dispatched with a pre-generated `--session-id` UUID and a `--name`,
so no output parsing is needed to learn the ID.

## Alternatives rejected

- **Pure delegation, no registry.** Each tool's picker only sees its own sessions, and
  Claude's ID resolution is cwd-scoped — a cross-tool, cross-directory roster is
  impossible without at least a mapping layer. Also loses dispatch provenance and cost
  history.
- **Full mirror (copy transcripts into swarm-tui's DB).** Duplicates state that three
  fast-moving tools already own, in three formats (JSONL ×2 schemas, SQLite), with
  privacy weight (transcripts contain code) and a permanent sync problem. Nothing in
  the v1 UI needs transcript content — tabs render live PTYs.
- **Filesystem-only registry (JSON per session).** Fine at small scale, but the home
  view wants ordered, filtered queries (status, tool, recency) and atomic updates from
  concurrent event streams; SQLite gives that for one dependency.

## Consequences

- Deleting a session inside a native tool leaves a dangling registry row; the
  reconciliation pass marks rows whose native ID no longer resolves as `orphaned`
  rather than deleting them.
- The registry schema is a contract: changes go through a migration file and an ADR
  note.
- agy rows may briefly lack a `native_session_id` (plain-text channel); the adapter
  backfills from the conversation store. If local verification finds no reliable way
  to do that, the fallback is dispatch-via-`-c`-continue semantics with a single
  serialized agy programmatic lane — record that outcome in the integration page.
- Revisit when: any tool ships a first-party "list sessions as JSON" command (Claude
  already has one for background sessions only), which could shrink the registry
  further.
