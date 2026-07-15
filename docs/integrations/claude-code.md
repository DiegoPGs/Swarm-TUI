# Integration: Claude Code (`claude`)

Verified 2026-07-04 against the official CLI reference and headless docs
(code.claude.com/docs), npm latest **2.1.201**. ✅ = confirmed from official docs;
🔶 = confirmed from reputable secondary sources; ⬜ = **verify locally** with
`scripts/verify-clis.sh` on the target machine.

**Local verification 2026-07-05 (CachyOS, target machine):** installed version
**2.1.201** at `~/.local/bin/claude`, logged in (`claude auth status` → exit 0);
every check in `scripts/verify-clis.sh` passed. Items marked ✅ *(local 2026-07-05)*
were flipped or re-confirmed against the live binary that day.

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
  reads** (API-key only). Therefore **not** swarm-tui's default — we need the existing
  subscription login. Non-bare `-p` shares auth and session state with interactive mode.

## Sessions & resume

- ✅ `-p --resume <session-id> "next"` continues a specific session headlessly;
  **ID lookup is scoped to the current project directory and its git worktrees** —
  dispatch and resume must share a cwd (the task carries its cwd for this reason).
- ✅ `--session-id <uuid>` pre-assigns the ID (swarm-tui generates it → no parsing);
  `--name/-n` names a session, resumable by name; `--fork-session` branches on resume;
  `--no-session-persistence` for throwaway runs.
- ✅ Native background sessions: `claude --bg "task"` (prints session ID, returns),
  `claude agents --json [--all]` lists them as JSON, `claude logs <id>`,
  `claude attach <id>` (adopt into a terminal — maps directly onto tab promotion),
  `claude stop|respawn|rm <id>`, `claude daemon status|stop` for the supervisor.
  ✅ *(local 2026-07-05)*: `--bg/--background` present in help; `agents`, `attach`,
  `logs`, `stop`, `respawn`, `rm`, and `daemon` all answer `--help` at 2.1.201.
  The last six are hidden from top-level `--help` but real; `claude agents --json`
  is documented in its own help as not requiring a TTY (with `--all` for completed).
  ✅ *(local 2026-07-15)* `claude agents --json --all` output shape, field names only
  (no session content recorded, per AGENTS.md): top level is a bare JSON array (not
  wrapped in an object). Entry fields observed: `sessionId` (camelCase — **not**
  `id` or `session_id`), `pid`, `cwd`, `kind`, `startedAt`, `name`. There is **no
  `status` field** in the entries observed. swarm-tui's reconciliation parser
  (`src/app/reconcile.rs`) accepts `id`/`session_id`/`sessionId` defensively and
  defaults `status` to `"unknown"` when absent, since absence is the common case.
- ✅ *(local 2026-07-05)* Transcript store layout confirmed on disk:
  `~/.claude/projects/<cwd-slug>/<session-uuid>.jsonl`, where `<cwd-slug>` is the
  absolute working directory with `/` → `-` (this repo →
  `~/.claude/projects/-home-nacho-Documents-Repositories-Swarm-TUI/`). `~/.claude.json`
  exists; `~/.claude/` also holds `sessions/`, `tasks/`, `history.jsonl` (checked by
  name only — contents never read). swarm-tui reads none of it in v1; the registry
  only needs IDs.

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
- ✅ *(local 2026-07-05)* MCP **server** mode exists after all: `claude mcp serve`
  ("Start the Claude Code MCP server") is a live subcommand at 2.1.201, despite its
  absence from the current online command reference (which misled the 2026-07-04
  remote pass). It exposes Claude Code's *tools* to an MCP client — it is not an
  agent-dispatch seam — so ADR-0001's "revisit when Claude documents an MCP server
  mode" trigger was assessed 2026-07-05 with **no decision change** (see
  `docs/NOTES.md`). Still not load-bearing for v1.

## Internal subagents (not orchestrated — ADR-0004)

✅ `.claude/agents/*.md`, dynamic `--agents '<json>'`, background agents, agent teams
(`--teammate-mode in-process|auto|tmux|iterm2`), `--worktree/-w` isolation. Noted for
awareness; swarm-tui treats a Claude session as one peer.

## Quirks & risks

- Background Bash tasks started during `-p` runs are killed ~5s after the final result
  (≥2.1.163); background subagent waits are capped at 10 min by default (≥2.1.182,
  `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`).
- `claude --help` deliberately omits flags; absence from `--help` ≠ unavailable — the
  probe greps help but trusts the integration page for documented-but-hidden flags.
  Observed instance at 2.1.201: `--max-turns` is missing from `--help` while
  `--max-budget-usd`, `--session-id`, and `--bg` are present. The probe must treat
  help-absence of a documented flag as "unknown", never "unavailable".
- Dual flag spellings are accepted and listed as pairs in help:
  `--allowedTools`/`--allowed-tools`, `--disallowedTools`/`--disallowed-tools`
  (local 2026-07-05).
- Ships near-daily; re-run the probe and diff against this page when versions jump.
