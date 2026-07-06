# Integration: Antigravity CLI (`agy`)

Verified 2026-07-04 from Google's codelab/README plus a flag-level cheat sheet
cross-checked against official docs and GitHub releases at **v1.0.16** (2026-07-02).
This tool is 7 weeks old and post-dates the design model's training data entirely —
treat every ⬜ as a real question, not a formality. ✅ = official source;
🔶 = reputable secondary; ⬜ = **verify locally**.

**Local verification 2026-07-05:** installed version is **v1.0.14** at
`~/.local/bin/agy` — *older* than the v1.0.16 the remote pass targeted (`agy update`
exists; swarm-tui never runs it — the owner updates by hand). Flag-level facts were
re-checked against the local binary and are marked ✅ *(local 2026-07-05)*.
Behavioral ⬜ items stay open deliberately: settling them means live dispatches that
write to the user's real conversation store — run those supervised, not from an
unattended session.

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
- ✅ *(local 2026-07-05)* **No structured-output flag at v1.0.14: confirmed.**
  `agy --help` lists no `--output-format` (nor any JSON/stream flag) — the
  programmatic channel is plain text, exactly as ADR-0001 designed. (History: one
  May-2026 article demoed `--output-format json` and showed it rejected in the same
  article; it was never in the v1.0.16-verified flag list either.)
- ✅ *(local 2026-07-05)* Full v1.0.14 flag surface, verbatim from `agy --help`:
  `--add-dir` (repeatable), `-c/--continue`, `--conversation <id>`,
  `--dangerously-skip-permissions`, `--log-file`, `--model`, `--new-project`,
  `-p/--print` (alias `--prompt`), `--print-timeout` (default `5m0s`),
  `--project <id>`, `-i/--prompt-interactive`, `--sandbox`. Subcommands: `changelog`,
  `help`, `install`, `models`, `plugin|plugins`, `update`. Notable: projects are a
  CLI-level concept (`--project`/`--new-project`), and there is no `--resume` —
  `--conversation` is the resume path.

## Sessions & resume

- ✅ `-c/--continue` resumes the most recent conversation; `--conversation <ID>`
  resumes by ID from the command line (upgrading the starting table's "not confirmed").
  In-TUI: `/resume` (aliases `/switch`, `/conversation`), `/fork`, `/rewind`, `/rename`.
- ✅ Conversations stored as **SQLite** (format since v1.0.4); conversation UUID is
  shown in the TUI status area.
- 🔶 *(local 2026-07-05)* Store location: `~/.gemini/antigravity-cli/conversations/`
  exists but is **empty** — this machine has never held an agy conversation — and no
  `*.db`/`*.sqlite*` exists anywhere under `~/.gemini` (filename scan only). The
  SQLite-since-1.0.4 claim therefore stays unconfirmed on disk; re-inspect (filenames
  only) right after the first real conversation. Sibling entries observed:
  `brain/`, `knowledge/`, `implicit/`, `cache/`, `history.jsonl`, `cli.log`.
- ⬜ Whether a headless `-p` run creates a resumable conversation and how to learn its
  ID afterward (options to test: newest row in the SQLite store; `-c` immediately
  after). If no reliable ID backfill exists, the adapter serializes agy programmatic
  dispatch and leans on `-c` — see ADR-0002 consequences. *Deferred 2026-07-05:
  requires a live dispatch against the real store; run supervised.*
- ⬜ Whether `--conversation <ID>` combines with `-p` for a headless follow-up.
  *Deferred 2026-07-05: same reason — live dispatch, run supervised.*

## Config & auth (paths only — never contents)

- ✅ Settings `~/.gemini/antigravity-cli/settings.json` (edit via `/config`),
  keybindings alongside; **shared with the desktop app**: hooks
  `~/.gemini/config/hooks.json`, global MCP `~/.gemini/config/mcp_config.json`;
  per-workspace MCP `.agents/mcp_config.json`. Context files: `GEMINI.md` and
  **`AGENTS.md`** (native support — this repo's convention works unmodified).
  *(Local 2026-07-05: `settings.json` ✅ and `config/mcp_config.json` ✅ exist;
  `config/hooks.json` does **not** exist yet — presumably created on first hook use.)*
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
  router sends agy read/analysis-shaped tasks only. *Deferred 2026-07-05: live
  dispatch, run supervised.*

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
- Installs trail releases (this machine ran 1.0.14 three days after 1.0.16 shipped).
  Treat version skew as normal: probe per machine, key facts to the *observed*
  version, and never auto-update — `agy update` is user territory.
- Youngest tool of the three; expect the fastest drift. The probe + `agy changelog`
  are the early-warning system.
