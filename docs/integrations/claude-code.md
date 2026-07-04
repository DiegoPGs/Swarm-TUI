# Integration: Claude Code (`claude`)

Verified 2026-07-04 against the official CLI reference and headless docs
(code.claude.com/docs), npm latest **2.1.201**. ✅ = confirmed from official docs;
🔶 = confirmed from reputable secondary sources; ⬜ = **verify locally** with
`scripts/verify-clis.sh` on the target machine.

## Invocation

- ✅ Interactive: `claude`, `claude "initial prompt"`, resume with
  `claude --resume <id|name>` (picker if no arg), continue with `claude -c`.
- ✅ Headless: `claude -p "..."` with `--output-format text|json|stream-json`.
  `json` returns one payload with `result`, `session_id`, `total_cost_usd`, `usage`;
  `stream-json` emits NDJSON events (add `--include-partial-messages` for token-level
  streaming, `--verbose` required by several stream options).
- ✅ Persistent bidirectional headless process: `--input-format stream-json` +
  `--output-format stream-json` (NDJSON in/out on one process). Message format is
  under-documented (tracked upstream, issue #24594) — treat as v2 option.
- ✅ Validated structured final output: `--json-schema '<schema>'` (print mode).
- ✅ `--bare` skips discovery of hooks/skills/MCP/CLAUDE.md **and skips OAuth/keychain
  reads** (API-key only). Therefore **not** overstory's default — we need the existing
  subscription login. Non-bare `-p` shares auth and session state with interactive mode.

## Sessions & resume

- ✅ `-p --resume <session-id> "next"` continues a specific session headlessly;
  **ID lookup is scoped to the current project directory and its git worktrees** —
  dispatch and resume must share a cwd (the task carries its cwd for this reason).
- ✅ `--session-id <uuid>` pre-assigns the ID (overstory generates it → no parsing);
  `--name/-n` names a session, resumable by name; `--fork-session` branches on resume;
  `--no-session-persistence` for throwaway runs.
- ✅ Native background sessions: `claude --bg "task"` (prints session ID, returns),
  `claude agents --json [--all]` lists them as JSON, `claude logs <id>`,
  `claude attach <id>` (adopt into a terminal — maps directly onto tab promotion),
  `claude stop|respawn|rm <id>`, `claude daemon status|stop` for the supervisor.
- ⬜ Transcript store layout on disk (expected `~/.claude/projects/...` JSONL; the
  docs' `claude project purge` confirms transcripts + `~/.claude.json` exist) — record
  exact paths locally. overstory reads none of it in v1; the registry only needs IDs.

## Config & auth (paths only — never contents)

- ✅ User scope `~/.claude/` (+ `~/.claude.json`), project `.claude/`
  (settings, agents, skills), repo `CLAUDE.md` (this repo uses the AGENTS.md shim).
- ✅ `claude auth status` prints auth state as JSON and exits 0/1 — the doctor script's
  safe auth check. `claude setup-token` mints a long-lived token for CI (not needed;
  we reuse the interactive login).

## Guardrails (headless dispatch)

- ✅ `--permission-mode default|acceptEdits|plan|auto|dontAsk|bypassPermissions`,
  `--allowedTools` / `--disallowedTools` / `--tools`, `--max-turns`,
  `--max-budget-usd <amount>` (print mode), `--permission-prompt-tool <mcp-tool>` for
  programmatic approval handling (a v2 option for surfacing approvals in the home view).

## MCP posture

- ✅ MCP client: `claude mcp` subcommands, `--mcp-config`, `--strict-mcp-config`.
- ⬜ MCP **server** mode: not present in the current command reference. Historical
  `claude mcp serve` may or may not still exist — check `claude mcp --help` locally
  before anyone designs against it. Not load-bearing for v1 (ADR-0001).

## Internal subagents (not orchestrated — ADR-0004)

✅ `.claude/agents/*.md`, dynamic `--agents '<json>'`, background agents, agent teams
(`--teammate-mode in-process|auto|tmux|iterm2`), `--worktree/-w` isolation. Noted for
awareness; overstory treats a Claude session as one peer.

## Quirks & risks

- Background Bash tasks started during `-p` runs are killed ~5s after the final result
  (≥2.1.163); background subagent waits are capped at 10 min by default (≥2.1.182,
  `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`).
- `claude --help` deliberately omits flags; absence from `--help` ≠ unavailable — the
  probe greps help but trusts the integration page for documented-but-hidden flags.
- Ships near-daily; re-run the probe and diff against this page when versions jump.
