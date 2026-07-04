# ADR-0005: Language and TUI framework — Rust + ratatui + tokio

- Status: Accepted (2026-07-04)

## Constraints that actually matter here

1. **Long-lived PTYs, many at once** — spawn, pump, resize, and render N interactive
   children plus M headless event streams without jank.
2. **Terminal-emulation widget** — a maintained way to render a child TUI inside our
   own UI (the make-or-break piece; see ADR-0003).
3. **Cross-platform** — daily driver is Arch Linux (CachyOS), second machine is macOS;
   Windows via ConPTY is nice-to-have, not a goal.
4. **Feels instant** — startup and keystroke latency must be indistinguishable from a
   bare terminal; this is a tool that sits in front of other tools all day.
5. **Distribution** — one static binary; no runtime to install on a fresh machine.

## Decision

**Rust**, with `ratatui` (0.30) for the UI, `tokio` for the async runtime,
`portable-pty` (0.9) for PTY children, `tui-term` (0.3.4) for the embedded terminal
widget, `rusqlite` (bundled) for the registry, and `clap` for the CLI surface. Versions
pinned as verified live on crates.io 2026-07-04.

The clinching pair: constraint 2 has a first-class, maintained answer only in this
ecosystem (`tui-term` exists precisely to put a pseudoterminal inside ratatui, on the
same PTY layer WezTerm ships), and constraint 4 has an existence proof — Codex CLI is
itself Rust + ratatui and is the snappiest of the three tools being wrapped.

## Alternatives rejected

- **Go + Bubble Tea + creack/pty.** Strong runtime and lovely framework (claude-squad
  is Go), single binary too. Falls short on constraint 2: the vt10x emulation crates
  in that ecosystem are semi-maintained, and Bubble Tea has no equivalent of a
  supported embedded-terminal widget — you end up writing the emulator or shelling out
  to tmux, which is ADR-0003's fallback, not its foundation.
- **Python + Textual.** Fastest to build and the owner's strongest language, but:
  embedding a child TUI is the weak spot (community `textual-terminal` is experimental),
  N PTYs + rendering in asyncio is workable but nearest to constraint 1's edge, and
  distribution to a fresh box means a Python environment (constraint 5). Wrong tool for
  a latency-critical emulator host, even if right for most of the owner's other work.
- **TypeScript + Ink + node-pty.** node-pty is battle-tested (VS Code), but Ink is
  built for line-oriented CLI output, not full-screen multiplexed panes; rendering a
  vt100 grid at speed through React reconciliation fights the framework, and Node
  startup works against constraint 4.

## Consequences

- Implementation sessions are in Rust; the builder (Claude Code) is strong at it, the
  owner is competent-but-not-daily — conventions in AGENTS.md keep the codebase boring
  (no async-trait gymnastics; enum dispatch per ADR-0006).
- `crossterm` backend gives Linux/macOS/Windows terminals; Windows support rides on
  `portable-pty`'s ConPTY and is untested until someone cares.
- Revisit when: the ADR-0003 spike fails on `vt100` *and* `wezterm-term`, at which
  point the language question reopens only if the tmux fallback is also rejected.
