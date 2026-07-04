# overstory

*Working name — see "Naming" below.*

One terminal application that sits above three AI coding CLIs already installed and
authenticated on this machine — **Claude Code** (`claude`), **Antigravity CLI** (`agy`),
and **Codex CLI** (`codex`) — and turns them into one workspace:

- **Per-service tabs.** Launch, resume, and switch between full interactive sessions of
  each underlying CLI without leaving the app. Each session runs the real tool in a real
  PTY; overstory never reimplements or re-authenticates any of them.
- **A home view.** A cross-cutting surface for work that spans more than one agent:
  dispatch a task to any of the three, broadcast the same prompt to several and compare,
  see every live session (foreground and background) in one roster.
- **A thin session registry.** The CLIs keep owning their own transcripts and
  resume-by-ID mechanics; overstory only maps its tabs onto their native session IDs so
  a task dispatched headlessly from the home view can later be opened as a full
  interactive tab, mid-conversation.

## Status

**Architecture and scaffold only — no runnable orchestration logic yet.** This repo is
the output of a design-and-research session (2026-07-04). Read
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the design,
[`docs/adr/`](docs/adr/) for the six decisions and their rejected alternatives, and
[`docs/NOTES.md`](docs/NOTES.md) for what was verified against official docs versus what
still needs confirmation on the target machine (`scripts/verify-clis.sh` does that pass).

## The shape of it

```
┌─ overstory (Rust · ratatui) ────────────────────────────────────────┐
│  [Home]  [claude · auth-refactor]  [codex · #1]  [agy · #1]   tabs  │
│                                                                     │
│  Home view ── task router ── thin session registry (SQLite)         │
│                    │                                                │
│           CliAdapter trait  (one impl per CLI, capability-gated)    │
│           ├─ interactive channel: portable-pty → vt100 grid → tab   │
│           └─ programmatic channel: headless subprocess → events     │
└──────────────┬───────────────────┬───────────────────┬──────────────┘
          claude -p /--bg      codex exec --json      agy -p
          claude / attach      codex / resume         agy / --conversation
```

## Non-goals

Reimplementing any agent, adding new auth flows, reaching inside each CLI's internal
subagent system (see ADR-0004), or being a general terminal multiplexer.

## Naming

Three options, pick one and rename the crate:

- **overstory** — the forest canopy layer that sits above independent trees; the tool is
  a layer above three agents that keep living their own lives. *(Current working name.)*
- **switchboard** — the operator that patches calls between independent parties; honest
  about the core job (routing work), extensible past three CLIs.
- **sindicato** — a union of autonomous peers acting together without a boss; wears the
  peer-agent architecture (ADR-0004) on its sleeve, reads well in Spanish and English.

## License

MIT (see [`LICENSE`](LICENSE)). Deliberately the low-friction default; swapping to a
copyleft license is a one-commit change while the repo is pre-1.0 — decide before
anything is published.
