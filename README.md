# swarm-tui

One terminal application that sits above the AI coding CLIs already installed and
authenticated on this machine — **Claude Code** (`claude`) and **Antigravity CLI**
(`agy`), with **Codex CLI** (`codex`) suspended for now
([ADR-0008](docs/adr/0008-suspend-codex-integration.md)) — and turns them into one
workspace:

- **Per-service tabs.** Launch, resume, and switch between full interactive sessions of
  each underlying CLI without leaving the app. Each session runs the real tool in a real
  PTY; swarm-tui never reimplements or re-authenticates any of them.
- **A home view.** A cross-cutting surface for work that spans more than one agent:
  dispatch a task to any active tool, broadcast the same prompt to several and compare,
  see every live session (foreground and background) in one roster.
- **A thin session registry.** The CLIs keep owning their own transcripts and
  resume-by-ID mechanics; swarm-tui only maps its tabs onto their native session IDs so
  a task dispatched headlessly from the home view can later be opened as a full
  interactive tab, mid-conversation.

## Status

**Milestone 2d (workspace personalization) is implemented as of 2026-07-17**,
on top of the 2c swarm plan (2026-07-16), the 2b command plane (same day), and
the 2a shell (2026-07-15). `cargo run` boots a real, interactive terminal
shell — tabs, a home roster, and live PTY sessions for Claude Code and
Antigravity CLI (an active tool degrades to a greyed-out badge if its probe
fails, never disappears), plus Claude Code background-agent reconciliation, a
**command palette** (`Ctrl-Space` then `:`), and **launch options**
(model/effort on spawn, stored on the session row). From 2c: a committed
**`.swarm/swarm.json` roles file** — named launch presets the new-session
picker lists above the raw tools, with startup commands injected after first
paint ([ADR-0010](docs/adr/0010-swarm-plan-roles-file.md)) — and a
**Resources view** (`Ctrl-Space` then `u`) showing each vendor's own usage
screen, captured verbatim from a hidden probe pane on manual refresh
([ADR-0011](docs/adr/0011-usage-view-probe-pane.md)). New in 2d: **schema v2
`defaults`** (picker preselect, broadcast set, dispatch guardrail
preferences, worktree policy slot) and a **personal, gitignored
`.swarm/swarm.local.json` overlay** that adds or overrides roles and defaults
per machine
([ADR-0012](docs/adr/0012-workspace-personalization-two-layer-plan.md)). Per-tool command and usage
facts live in
[`docs/integrations/command-surfaces.md`](docs/integrations/command-surfaces.md). **Codex CLI is suspended as of 2026-07-16**
([ADR-0008](docs/adr/0008-suspend-codex-integration.md)): its adapter stays
compiled for easy reversal, but it is never probed, offered, or spawned; historical
codex rows still render read-only in the roster. Headless dispatch
(`dispatch()`/`follow_up()`), broadcast, pipelines, and MCP integration are not
implemented yet — deferred to a later milestone. Read
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the design,
[`docs/adr/`](docs/adr/) for the decisions and their rejected alternatives,
and [`docs/NOTES.md`](docs/NOTES.md) for what's verified against official docs
versus what's inferred (`scripts/verify-clis.sh` does the local verification pass).

## Quickstart

Requires the CLIs you want to use already installed and logged in locally
(`claude` and/or `agy` on `PATH`) — swarm-tui never installs or authenticates
them. (Codex CLI is suspended per ADR-0008.)

```
cargo run
```

Boots straight into the Home tab. Press `Ctrl-Space` then `c` to open a new
session tab — the picker lists your workspace's **roles** first, then the raw
tools (uninstalled tools are greyed out, not hidden). See the keymap below for
everything else.

### Roles & defaults (`.swarm/swarm.json` + `.swarm/swarm.local.json`)

Commit a plan file at the repo root and the picker turns it into one-keystroke
launch presets. Schema v2 adds an optional `defaults` object; this repo
dogfoods its own:

```json
{
  "version": 2,
  "roles": {
    "researcher": { "tool": "antigravity", "model": "gemini-3.1-pro",
                    "purpose": "web search & docs" },
    "coder":      { "tool": "claude-code", "model": "opus-4.8", "effort": "high",
                    "purpose": "implementation" },
    "advisor":    { "tool": "claude-code", "model": "sonnet-5",
                    "purpose": "general advisor",
                    "startup_commands": ["/advisor fable"] }
  },
  "defaults": {
    "default_role": "coder",
    "broadcast": ["coder", "advisor"],
    "dispatch": { "posture": "plan" },
    "worktrees": "in_place"
  }
}
```

Model strings pass **verbatim** to the tool's `--model` (an invalid id is the
tool's own in-pane error); `startup_commands` inject in order once the pane
paints, and any command whose effect persists beyond the session asks for a y/n
first. `defaults` are preferences, not policy: `default_role` preselects a
picker row; `broadcast` names the roles a broadcast targets by default (naming
a role is the explicit opt-in for its tool — the quota-shared agy is out unless
named); `dispatch` tightens the headless-dispatch guardrails in neutral terms
(`posture`, `max_turns`, `max_budget_usd`, `timeout_secs` — broadcast and
dispatch behavior land in milestone 3); `worktrees` accepts `in_place` for now
(`per_task` is reserved).

A personal, gitignored **`.swarm/swarm.local.json`** overlays the committed
file: same schema, merged per named entry (role names, defaults fields) with
local winning wholesale per entry — add your own roles, pick your own
`default_role`, keep the team's guardrails. Missing file = that layer simply
absent; a malformed layer = a one-line picker error naming the offending file.
Both reload with `Ctrl-Space` `r`. Never put secrets in these files — no field
accepts them. Details in
[ADR-0010](docs/adr/0010-swarm-plan-roles-file.md) and
[ADR-0012](docs/adr/0012-workspace-personalization-two-layer-plan.md).

## Keymap

Press **Ctrl-Space** to enter the one-shot command mode, then press one of:

| Key | Action |
| --- | --- |
| `h` / `0` | Home |
| `1`-`9` | Jump to tab N |
| `n` / `p` | Cycle to next / previous tab |
| `c` | New session (roles, then tools; raw tools open a model/effort form) |
| `:` | Command palette — inject a native slash command into the active session tab |
| `d` | Detach |
| `x` | Close tab (confirm) |
| `r` | Refresh roster + reload `.swarm/swarm.json` and its `.local` overlay |
| `u` | Resources view — per-vendor usage (digit refreshes a vendor; Esc/`u` back) |
| `?` | Keymap overlay |
| `q` | Quit (confirm if any pane is alive; quitting kills remaining panes after confirmation) |

Pressing Ctrl-Space twice sends one literal Ctrl-Space byte through to the pane
instead of dispatching a command — the escape hatch for a wrapped tool that itself
binds Ctrl-Space. Full rationale in
[`docs/adr/0007-input-routing-and-prefix-key.md`](docs/adr/0007-input-routing-and-prefix-key.md).

## The shape of it

```
┌─ swarm-tui (Rust · ratatui) ────────────────────────────────────────┐
│  [Home]  [claude · auth-refactor]  [agy · #1]                 tabs  │
│                                                                     │
│  Home view ── task router ── thin session registry (SQLite)         │
│                    │                                                │
│           CliAdapter trait  (one impl per CLI, capability-gated)    │
│           ├─ interactive channel: portable-pty → vt100 grid → tab   │
│           └─ programmatic channel: headless subprocess → events     │
└──────────────┬─────────────────────────┬────────────────────────────┘
          claude -p /--bg            agy -p
          claude / attach            agy / --conversation
```

(The codex lane — `codex exec --json` / `codex resume` — stays designed in
ADR-0001 but suspended per ADR-0008.)

## Non-goals

Reimplementing any agent, adding new auth flows, reaching inside each CLI's internal
subagent system (see ADR-0004), or being a general terminal multiplexer.

## Naming

**swarm-tui** — chosen by the owner 2026-07-05 (a swarm of coding agents under one
terminal UI). The design-session working name was *overstory*; older commits and the
git history use it. The rejected candidates were *switchboard* and *sindicato*.

## License

MIT (see [`LICENSE`](LICENSE)). Deliberately the low-friction default; swapping to a
copyleft license is a one-commit change while the repo is pre-1.0 — decide before
anything is published.
