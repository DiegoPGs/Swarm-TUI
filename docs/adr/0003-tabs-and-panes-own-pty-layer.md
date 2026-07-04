# ADR-0003: Tabs and panes — own PTY layer on `portable-pty` + `tui-term`

- Status: Accepted (2026-07-04) — subject to the fidelity spike below

## Prior art surveyed

- **claude-squad** (smtg-ai): manages many Claude Code/Codex/Gemini/Aider instances by
  driving **tmux** sessions plus git worktrees. Proves the tmux-backend approach works
  for exactly these tools — and shows its ceiling: the UI is tmux's, the dependency is
  hard, and Windows is out.
- **Claude Code itself**: its agent view and `--teammate-mode in-process|tmux|iterm2`
  show a first-party team shipping *both* an in-process pane renderer and a tmux
  escape hatch — validating the same ladder this ADR adopts.
- **cc-switch / Crystal / Vibe Kanban**: desktop/web multi-agent frontends; different
  product shape (not a terminal app), useful mainly as UX references for rosters.
- **Rust building blocks**: `portable-pty` 0.9 (WezTerm's PTY layer; Unix + Windows
  ConPTY), `tui-term` 0.3.4 (actively maintained ratatui pseudoterminal widget over the
  `vt100` parser), `ratatui` 0.30. Codex CLI is itself Rust + ratatui — an existence
  proof that the stack feels instant.

## Decision

Build tabs and panes **in-process**: one `portable-pty` child per interactive session,
parsed into a `vt100` screen via `tui-term`, rendered as a ratatui widget; the tab bar,
home view, and keymap are plain ratatui. The PTY host hides behind a small `PaneHost`
seam so the rendering backend can be swapped without touching adapters or the shell.

**Gate:** the first implementation milestone is a fidelity spike — run each of the
three real TUIs (Claude's Ink-based UI, Codex's ratatui UI, agy's Go TUI) inside the
widget and log defects (alt-screen, truecolor, mouse, cursor shapes, resize storms).
Fallback ladder if `vt100` fidelity is insufficient: swap the parser for
`wezterm-term`; if still insufficient, implement `PaneHost` on **tmux control mode**
(claude-squad's approach) as a Linux/macOS backend and keep the native backend for
simple panes.

## Alternatives rejected

- **Embedded tmux as the pane engine (v1).** Maximum rendering fidelity for free, but:
  hard runtime dependency (not guaranteed present; this sandboxless design must run on
  a fresh CachyOS or macOS box), UI chrome ceded to tmux, awkward Windows story, and
  every UX idea filtered through send-keys/control-mode. Retained as the documented
  fallback backend, not the foundation.
- **Zellij plugin.** Zellij plugins are sandboxed WASM; owning long-lived PTY children
  and a SQLite registry from inside that sandbox fights the platform.
- **No embedding — launch sessions in external terminal windows/tabs.** Simplest
  possible v1 (this is what the spawn-agent skill does), but it abandons the core
  product promise: one application, internal tabs, a home view adjacent to live
  sessions.
- **Web UI (xterm.js).** Mature terminal widget, but the brief says *terminal
  application*; a localhost web app changes the product.

## Consequences

- overstory owns resize propagation, scrollback buffers, and copy-mode — real work,
  scoped in `src/pty/`.
- The fidelity spike is the riskiest unknown in the whole design and is deliberately
  scheduled first; its outcome is recorded in `docs/NOTES.md` and, if it triggers the
  fallback, a superseding ADR.
- Revisit when: `tui-term`/`vt100` gain or lose momentum, or the spike fails twice.
