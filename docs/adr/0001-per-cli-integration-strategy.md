# ADR-0001: Per-CLI integration strategy — dual-channel, decided per tool

- Status: Accepted (2026-07-04)
- Deciders: design session; verify-locally items flagged in `docs/integrations/`

## Context

Three candidate seams exist for each CLI: (a) PTY-wrap the interactive TUI, (b) headless
invocation per task with parseable output, (c) MCP or another server protocol where the
tool offers one. The product needs both live interactive sessions (tabs) *and*
programmatic dispatch (home view), which no single seam serves well: PTY output is for
humans, headless runs have no UI, and MCP server mode exists natively only for Codex.

## Decision

Every adapter is **dual-channel**: a mandatory interactive channel (PTY-wrapped native
TUI) plus a capability-gated programmatic channel. The programmatic channel is decided
per tool:

- **Claude Code** — `claude -p --output-format stream-json` for routed foreground
  tasks, with the orchestrator pre-assigning IDs via `--session-id` and applying
  `--max-turns` / `--max-budget-usd`. Long or parallel tasks may instead be delegated
  to Claude's **native background supervisor** (`claude --bg`, monitored with
  `claude agents --json`, logs via `claude logs <id>`, tab promotion via
  `claude attach <id>`): it is crash-resilient, already supervised, and free code.
  Not `--bare` by default: bare mode skips OAuth/keychain and would violate the
  reuse-existing-login requirement.
- **Codex CLI** — `codex exec --json` (JSONL events; `thread.started` carries the
  session ID), resume via `codex exec resume <id>`, guardrails via `--sandbox`.
- **Antigravity CLI** — `agy -p` returning **plain text** (no structured-output flag
  confirmed at v1.0.16), `--print-timeout` as the hard stop, resume via
  `--conversation <id>` / `-c`. The adapter synthesizes `Started`/`Completed` events
  and resolves the native conversation ID from the tool's store after the fact.

## Alternatives rejected

- **MCP as the primary dispatch seam.** Only Codex ships a native server mode
  (`codex mcp-server`); Claude Code's current command reference lists no equivalent and
  agy has none (a third-party wrapper exists). Building the primary path on a seam one
  of three tools natively supports creates the asymmetry the adapter layer exists to
  hide. Kept as a per-adapter upgrade: Codex's `app-server` (JSON-RPC; exposes
  `thread/fork`, which the exec surface lacks) is the designated v2 seam for Codex.
- **Agent SDK for Claude Code.** Sanctioned and richer, but TypeScript/Python — it
  would put a second runtime inside a Rust binary for capabilities the CLI's
  stream-json + background supervisor already expose.
- **PTY-only everywhere, scrape the screen.** Uniform but brittle (parsing human UI),
  and discards the structured output two of three tools ship precisely for this.
- **Headless-only, no PTY.** Fails the product: tabs must be the real interactive tools.

## Consequences

- The home view gets rich streaming telemetry from claude/codex and coarse
  text-plus-exit-status from agy; UI must degrade gracefully (no `ToolActivity` lane
  for agy).
- cwd becomes part of every task (Claude scopes `-p --resume` ID lookup to the project
  directory).
- Revisit when: agy ships a structured output flag; Claude's command reference
  (re)documents an MCP server mode; or Codex's exec surface gains fork.
