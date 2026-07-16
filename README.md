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

**Milestone 2c (the swarm plan) is implemented as of 2026-07-16**, on top of the
2b command plane (same day) and the 2a shell (2026-07-15). `cargo run` boots a
real, interactive terminal shell — tabs, a home roster, and live PTY sessions for
Claude Code and Antigravity CLI (an active tool degrades to a greyed-out badge if
its probe fails, never disappears), plus Claude Code background-agent
reconciliation, a **command palette** (`Ctrl-Space` then `:`), and **launch
options** (model/effort on spawn, stored on the session row). New in 2c: a
committed **`.swarm/swarm.json` roles file** — named launch presets the
new-session picker lists above the raw tools, with startup commands injected
after first paint ([ADR-0010](docs/adr/0010-swarm-plan-roles-file.md)) — and a
**Resources view** (`Ctrl-Space` then `u`) showing each vendor's own usage screen,
captured verbatim from a hidden probe pane on manual refresh
([ADR-0011](docs/adr/0011-usage-view-probe-pane.md)). Per-tool command and usage
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

### Roles (`.swarm/swarm.json`)

Commit a roles file at the repo root and the picker turns it into one-keystroke
launch presets. This repo dogfoods its own:

```json
{
  "version": 1,
  "roles": {
    "researcher": { "tool": "antigravity", "model": "gemini-3.1-pro",
                    "purpose": "web search & docs" },
    "coder":      { "tool": "claude-code", "model": "opus-4.8", "effort": "high",
                    "purpose": "implementation" },
    "advisor":    { "tool": "claude-code", "model": "sonnet-5",
                    "purpose": "general advisor",
                    "startup_commands": ["/advisor fable"] }
  }
}
```

Model strings pass **verbatim** to the tool's `--model` (an invalid id is the
tool's own in-pane error); `startup_commands` inject in order once the pane
paints, and any command whose effect persists beyond the session asks for a y/n
first. Missing file = no roles section; malformed file = a one-line error in the
picker. Never put secrets in this file — no field accepts them. Details in
[ADR-0010](docs/adr/0010-swarm-plan-roles-file.md).

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
| `r` | Refresh roster + reload `.swarm/swarm.json` |
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
