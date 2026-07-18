# swarm-tui

Terminal orchestrator (Rust, ratatui) that wraps the locally installed Claude Code
and Antigravity CLI (`agy`): per-service interactive session tabs plus a home view
that routes cross-agent work. The Codex CLI integration is **suspended**
(ADR-0008) — its adapter stays compiled for reversal but is never probed, offered,
or spawned. Maturity: **mid implementation** — the shell (milestones 2a–2d) and the
programmatic plane (milestone 3: headless dispatch, broadcast-and-compare, the
one-live-handle promote rule; ADR-0013) are real; pipelines and MCP integration are
not. The single thing an agent must never break: **swarm-tui reuses the
wrapped CLIs' existing local logins and config; it must never trigger a new auth flow or
read, print, or copy the contents of any credential file** (`~/.codex/auth.json`,
anything under `~/.claude/`, `~/.gemini/`, or the OS keyring). Checking that these paths
*exist* is fine; their contents are off-limits.

## Commands

| Task | Command |
| --- | --- |
| Verify the three CLIs on this machine | `./scripts/verify-clis.sh` |
| Type/borrow check | `cargo check` |
| Test (all) | `cargo test` |
| Test (single) | `cargo test <name>` |
| Lint + format | `cargo clippy -- -D warnings && cargo fmt` |
| Run (dev) | `cargo run` |

## Architecture

- Entry point: `src/main.rs` — boots config, capability probe, registry, then the TUI shell.
- TUI shell: `src/app/` — tab bar, home view, per-session pane views. No CLI knowledge.
- Core: `src/core/` — normalized `AgentEvent`s, task routing, session records, config.
- Adapters: `src/adapters/` — one module per CLI behind the `CliAdapter` trait; the
  **only** place that knows flag names, paths, or output formats of an underlying tool.
- PTY plumbing: `src/pty/` — spawn/attach/resize via `portable-pty`, grids via `tui-term`.
- Registry: `src/store/` — SQLite mapping swarm-tui sessions → native CLI session IDs.
- Deeper docs: `docs/ARCHITECTURE.md` (data flow), `docs/adr/` (why), and
  `docs/integrations/<cli>.md` (per-CLI ground truth). Read the relevant integration doc
  before touching its adapter; read the ADR before changing anything it decided.

## Conventions

- Rust 2021, `rustfmt` defaults, clippy clean at `-D warnings`.
- Commits: Conventional Commits (`feat(adapters): …`, `docs(adr): …`).
- Architecture changes require a new ADR (`docs/adr/000N-title.md`); supersede, never
  edit, an accepted ADR.
- Everything CLI-specific stays inside its adapter module. `core` and `app` may only
  speak `AgentEvent`, `SessionRecord`, `AdapterCaps`, and the data-only
  launch/command vocabulary `LaunchIntent`, `LaunchOptions`, `NativeCommand`
  (ADR-0009). Flag names and per-tool semantics never leave `src/adapters/`.
- Facts about the three CLIs go in `docs/integrations/` with a verified/unverified
  marker and a date — never inline in code comments alone.

## Definition of done

- `cargo check`, `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` pass.
- New behavior is covered by a test; adapter behavior is covered against recorded
  fixtures, not live CLI calls.
- Affected docs updated: integration page if a CLI fact changed, ADR if a decision did.

## Boundaries

- Never read, log, or copy credential file contents (see the never-break rule above).
- Never modify the user's CLI configs (`~/.claude/`, `~/.codex/`, `~/.gemini/`) —
  swarm-tui reads paths and public state only.
- Never run installers or `claude/agy/codex` auth subcommands from code or scripts.
- Ask before: adding dependencies, changing the `CliAdapter` trait or `AgentEvent`
  schema, or committing anything derived from a real session transcript.

## Gotchas

- `agy -p` has **no structured-output flag** (confirmed locally at v1.0.14
  2026-07-05 and v1.1.3 2026-07-16) — the Antigravity programmatic channel is plain
  text; don't assume JSON parity with Claude Code.
- `claude -p --resume <id>` resolves session IDs **scoped to the current project
  directory and its worktrees** — dispatch and resume must run from the same cwd.
- `claude --bare` skips OAuth/keychain reads and needs an API key — it breaks the
  "reuse existing login" requirement; do not use it as the default dispatch mode.
- `codex exec` defaults to a **read-only sandbox** and refuses to run outside a git
  repository (`--skip-git-repo-check` to override); plan guardrail defaults accordingly.
- A `claude` spawned from inside another Claude Code session inherits `CLAUDECODE`/
  `CLAUDE_CODE_*` env vars and changes behavior; any PTY spawn of a wrapped CLI must
  scrub those vars and set a plain `TERM` (pattern: `examples/fidelity_spike.rs`).
