# Integration: Codex CLI (`codex`)

> **⏸ SUSPENDED (ADR-0008, 2026-07-16).** Codex is out of swarm-tui's active scope:
> `registry()` no longer lists it, so it is never probed, offered in the new-session
> picker, or spawned. The adapter module, `AdapterKind::Codex`, and this page stay —
> historical `tool = "codex"` registry rows still render read-only in the Home
> roster. Reversal = restore the registry entry and run the recipe in
> [`docs/adr/0008-suspend-codex-integration.md`](../adr/0008-suspend-codex-integration.md).
> The facts below are frozen at the dates recorded.

Verified 2026-07-04 against developers.openai.com/codex (official reference, features,
non-interactive pages), npm latest **0.142.5**. ✅ = official docs; 🔶 = reputable
secondary / upstream issue tracker; ⬜ = **verify locally**.

**Local verification 2026-07-05: NOT INSTALLED.** `codex` is on neither bash nor fish
PATH, `~/.codex/` does not exist, and no npm/pnpm/bun global or pacman package
provides it (node/npm themselves are present). Every ✅ below remains remote-verified
at 0.142.5; every ⬜ is **blocked until the owner installs Codex CLI** — swarm-tui's
boundary forbids installing or updating the wrapped CLIs itself. After installing
(e.g. `npm install -g @openai/codex`), re-run `scripts/verify-clis.sh` and settle this
page's ⬜ items. Until then the adapter probe reports codex unavailable and the roster
badges it (the ARCHITECTURE "CLI missing" failure mode — no design impact).

## Invocation

- ✅ Interactive: `codex` (Rust/ratatui TUI), `codex resume [--last|--all|<id>]`
  (picker by default), `codex fork [--last|<id>]` to branch a session.
- ✅ Headless: `codex exec "..."` — final result to stdout; `codex exec -` reads the
  whole prompt from stdin; piped stdin + a prompt argument = instruction + context.
- ✅ Structured output: `codex exec --json` turns stdout into a JSONL event stream —
  `thread.started` (carries the session/thread ID), `turn.started|completed|failed`,
  `item.*` (agent messages, reasoning, commands, file changes, MCP calls, web
  searches, plan updates), `error`. Also `--output-last-message <file>` (`-o`) and
  `--output-schema <file>` for a schema-validated final response.
- ✅ `--ephemeral` skips writing a rollout (for fire-and-forget probes).

## Sessions & resume

- ✅ Headless resume: `codex exec resume --last "..."` or
  `codex exec resume <SESSION_ID> "..."` — resumed runs keep transcript, plan history,
  and approvals. `--cd` / `--add-dir` steer the environment when resuming.
- ✅ Rollouts stored as JSONL under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`;
  IDs visible in the picker, `/status`, and filenames.
- 🔶 **No headless fork**: `codex fork` is TUI-only (upstream issue #11750, open as of
  Feb 2026). Fine for swarm-tui — promotion to a tab is interactive by definition —
  but rules out headless branch-and-compare on Codex for now. The `app-server`
  JSON-RPC exposes `thread/fork` if that ever matters.
- ⬜ Exact `thread.started` payload field names at the installed version (capture one
  real event stream as a fixture for the adapter tests).

## Config & auth (paths only — never contents)

- ✅ `~/.codex/config.toml`, per-profile `$CODEX_HOME/<profile>.config.toml`, inline
  overrides `-c key=value`, managed policy via `requirements.toml`. Project-level
  config: ⬜ confirm whether the installed version reads a repo-local `.codex/`
  (the starting table assumed it; current official pages emphasize `~/.codex` +
  `AGENTS.md` for project context).
- ✅ Auth: `codex login`, stored in `~/.codex/auth.json` (ChatGPT account or API key);
  `codex exec` **reuses saved CLI authentication by default** — exactly the reuse
  swarm-tui needs. `CODEX_API_KEY` env override exists for exec only (not used).

## Guardrails

- ✅ `codex exec` defaults to a **read-only sandbox**; escalate deliberately with
  `--sandbox workspace-write` or (never by default) `danger-full-access`.
- ✅ Requires a git repository unless `--skip-git-repo-check` — the router treats
  "cwd is a git repo" as a Codex dispatch precondition.
- ✅ Approval policy: `--ask-for-approval` / config `approval_policy`; `on-failure`
  deprecated in favor of `on-request` (interactive) and `never` (non-interactive).
- ✅ MCP servers configured with `required = true` fail the exec run if they don't
  initialize — good failure semantics for dispatch.

## MCP posture & subagents

- ✅ MCP client (config-managed). **Native server modes exist**: `codex mcp-server`
  (Codex as an MCP server) and `codex app-server` (JSON-RPC) are documented runtime
  commands — the richest programmatic seam of the three tools, reserved as Codex's v2
  integration path (ADR-0001).
- ✅ Subagents documented as a feature ("parallelize complex tasks"); ⬜ the starting
  table's `[agents]` role block in `config.toml` remains unverified — check
  `codex --help` / config reference locally. Irrelevant to orchestration (ADR-0004).

## Quirks & risks

- npm distributes per-platform binaries (`0.142.5-linux-x64` etc.) plus an alpha
  channel; the probe should tolerate alpha version strings.
- Third-party guides describe extra resume spellings (`codex continue`,
  `--resume-session-id`) that the official reference does not — the adapter uses only
  the officially documented forms above.
