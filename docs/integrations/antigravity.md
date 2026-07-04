# Integration: Antigravity CLI (`agy`)

Verified 2026-07-04 from Google's codelab/README plus a flag-level cheat sheet
cross-checked against official docs and GitHub releases at **v1.0.16** (2026-07-02).
This tool is 7 weeks old and post-dates the design model's training data entirely —
treat every ⬜ as a real question, not a formality. ✅ = official source;
🔶 = reputable secondary; ⬜ = **verify locally**.

## Identity & lineage

- ✅ Announced 2026-05-19 as the successor to Gemini CLI; legacy Gemini CLI shut down
  for consumer tiers 2026-06-18. Go single binary named `agy`, installed to
  `~/.local/bin/agy` (Unix) via Google's install script; shares the agent harness and
  settings with the Antigravity 2.0 desktop app, including session export to the GUI.
- 🔶 npm `@google/gemini-cli` still publishes (0.49.0 + nightlies as of 2026-07-04) —
  apparently the enterprise/Code Assist continuation. `agy` is *not* on npm; version
  checks go through the binary itself (`agy changelog` shows release notes).

## Invocation

- ✅ Interactive: `agy`, `agy "initial prompt"`, `-i/--prompt-interactive` to seed a
  prompt then stay interactive.
- ✅ Headless: `agy -p/--print "..."` (alias `--prompt`), `--print-timeout` (default
  5m) as the hard stop.
- 🔶→⬜ **No structured-output flag confirmed at v1.0.16.** One May-2026 article demoed
  `--output-format json` and, in the same article, showed the flag being rejected;
  it does not appear in the v1.0.16-verified flag list. **Assume plain text** until
  `agy --help` on the target machine says otherwise. This is the single biggest
  divergence from the design's starting hypothesis table.

## Sessions & resume

- ✅ `-c/--continue` resumes the most recent conversation; `--conversation <ID>`
  resumes by ID from the command line (upgrading the starting table's "not confirmed").
  In-TUI: `/resume` (aliases `/switch`, `/conversation`), `/fork`, `/rewind`, `/rename`.
- ✅ Conversations stored as **SQLite** (format since v1.0.4); conversation UUID is
  shown in the TUI status area.
- ⬜ Exact `.db` path on disk (expected under `~/.gemini/antigravity-cli/`;
  `~/.gemini/antigravity-cli/cache/projects.json` is confirmed for the project map).
- ⬜ Whether a headless `-p` run creates a resumable conversation and how to learn its
  ID afterward (options to test: newest row in the SQLite store; `-c` immediately
  after). If no reliable ID backfill exists, the adapter serializes agy programmatic
  dispatch and leans on `-c` — see ADR-0002 consequences.
- ⬜ Whether `--conversation <ID>` combines with `-p` for a headless follow-up.

## Config & auth (paths only — never contents)

- ✅ Settings `~/.gemini/antigravity-cli/settings.json` (edit via `/config`),
  keybindings alongside; **shared with the desktop app**: hooks
  `~/.gemini/config/hooks.json`, global MCP `~/.gemini/config/mcp_config.json`;
  per-workspace MCP `.agents/mcp_config.json`. Context files: `GEMINI.md` and
  **`AGENTS.md`** (native support — this repo's convention works unmodified).
- ✅ Auth: OS keyring, falling back to Google Sign-In; SSH-aware (prints an auth URL).
  `ANTIGRAVITY_TOKEN` for CI; `GEMINI_API_KEY` is **ignored** (classic migration trap).
  `/logout` clears credentials.

## Guardrails

- ✅ Tool Permission presets: `request-review` (default), `proceed-in-sandbox`,
  `always-proceed`, `strict`; switch via `/permissions` or settings `toolPermission`.
  Flags: `--sandbox`, `--dangerously-skip-permissions`. Settings:
  `enableTerminalSandbox`, `allowNonWorkspaceAccess`.
- ⬜ How `request-review` behaves under `-p` with no TTY (block? auto-deny? proceed?)
  — determines which task classes are safe to route headlessly. Until tested, the
  router sends agy read/analysis-shaped tasks only.

## MCP posture & subagents

- ✅ MCP client (config files above, `/mcp` manager, configurable server-launch
  timeout). No native MCP-server mode; an experimental community wrapper
  (`ask-antigravity-mcp`) exists — not a dependency.
- ✅ Async subagents via the `/agents` Agent Manager (a headline feature); 🔶 `/goal`
  and `/schedule` reported. Not orchestrated per ADR-0004.

## Quirks & risks

- 🔶 Quota is shared across CLI, desktop app, and SDK, and **parallel subagents burn it
  fast** — early users report Pro-tier lockouts after minimal use. Broadcast tasks that
  include agy should default the roster to showing quota-risk, and `/credits` exists
  in-TUI.
- Model defaults to Gemini 3.5 Flash; `--model` (≥1.0.5) and `agy models` to change.
- Youngest tool of the three; expect the fastest drift. The probe + `agy changelog`
  are the early-warning system.
