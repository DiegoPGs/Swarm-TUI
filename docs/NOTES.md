# Investigation notes — 2026-07-04

Written so a later session does not redo this research. Read alongside
`docs/integrations/*.md`, which carry the per-fact ✅/🔶/⬜ markers.

## Method and its one big limitation

**The design session ran in a sandboxed container, not on the target machine.** None of
`claude`, `agy`, `codex` (nor `tmux`, nor a Rust toolchain) were installed there —
confirmed by direct check. Everything below was therefore verified *remotely* against
primary sources on 2026-07-04: official docs (code.claude.com, developers.openai.com,
Google codelab/GitHub for agy), package registries (npm, crates.io), and upstream issue
trackers — with secondary sources only where no primary existed (marked 🔶). The brief's
"dispatch parallel subagents to confirm on this machine" step is **deferred to the
first implementation session** and scripted as `scripts/verify-clis.sh`; every fact it
must confirm is pre-marked ⬜ in the integration pages.

## Versions current on 2026-07-04

| Tool | Version | Source |
| --- | --- | --- |
| Claude Code | 2.1.201 (npm latest; 2.1.193 stable tag) | registry.npmjs.org |
| Codex CLI | 0.142.5 (+0.143 alpha channel) | registry.npmjs.org |
| Antigravity CLI | v1.0.16, released 2026-07-02 | GitHub releases via cheat-sheet cross-check |
| `@google/gemini-cli` | 0.49.0, nightlies still publishing 2026-07-04 | registry.npmjs.org |
| ratatui / portable-pty / tui-term / vt100 | 0.30.2 / 0.9.0 / 0.3.4 / 0.16.2 | crates.io API |

## Material divergences from the brief's starting table

1. **agy structured output: worse than hoped.** No `--output-format` in the
   v1.0.16-verified flag set; one early article showed the flag *and* showed it being
   rejected. Designed for plain-text programmatic channel (ADR-0001), pending local
   `agy --help`.
2. **agy resume: better than hoped.** CLI resume-by-ID is confirmed —
   `--conversation <ID>` and `-c`, plus `/fork`, `/rewind`. Conversation store is
   SQLite (since 1.0.4). Config lives under `~/.gemini/antigravity-cli/` (shared
   `~/.gemini` tree with the desktop app), not a standalone antigravity dir.
3. **Claude Code is bigger than the table implies.** `--max-budget-usd` is real
   (confirmed in the official flag table — the table row was right). Beyond it: a
   native background-session supervisor (`--bg`, `claude agents --json`,
   `attach/logs/stop/respawn/rm`, `daemon`), `--session-id` pre-assignment, named
   sessions, `--json-schema`, `--bare` (which skips OAuth — a trap for this product),
   agent teams with a tmux/iterm2 display mode, native worktrees. This reshaped the
   Claude adapter (ADR-0001) and simplified the registry (ADR-0002).
4. **Claude Code MCP server mode is *not* in the current command reference** — the
   table's flat "MCP: yes" is true for client use; server mode needs local
   confirmation before anyone leans on it.
5. **Codex as tabled, plus**: `codex mcp-server` *and* an `app-server` JSON-RPC surface
   (exposes `thread/fork`, which the exec surface lacks — headless fork is an open
   upstream gap, issue #11750). `[agents]` config block: not found in official pages;
   left ⬜. Gemini CLI's consumer shutdown (2026-06-18) vs. npm still publishing is
   reconciled as enterprise/Code Assist continuation.
6. **`aiskillstore` / `claude-code-headless`: exact artifact not found.** The
   aiskillstore marketplace exists (LobeHub); the closest match is its **`spawn-agent`**
   skill family — per-CLI "cookbooks" for spawning Claude/Codex/Gemini/Cursor/OpenCode
   in new terminal sessions, with notes that align with this repo's findings (Claude:
   headless `--print` friendly; Codex and others: PTY required for some paths). It is
   a prompt playbook that opens external terminals, not a reusable dispatch library —
   prior art for invocation patterns, not a component. Also surfaced: `acpx`
   (headless Agent Client Protocol client) — ACP is a fourth possible seam to watch,
   not used in v1.

## Prior-art shortlist (feeds ADR-0003/0004)

claude-squad (tmux + worktrees, Go — the tmux-backend proof), Claude Code's own agent
view + `--teammate-mode tmux` (first-party validation of the in-process/tmux ladder),
cc-switch, Crystal, Vibe Kanban (different product shapes), tui-term + portable-pty
(the in-process building blocks; tui-term actively maintained as of 2026-04).

## Confirmed-by-checking-the-sandbox

`node` present; no CLIs, no tmux, no cargo — reinforcing ADR-0005's single-binary,
no-runtime-dependency stance for the eventual distribution.

## For the fidelity spike (first implementation milestone)

Record here: per-CLI rendering defects under `vt100`, whether `wezterm-term` was
needed, resize behavior under rapid streaming, and the verdict that either confirms
ADR-0003 or triggers its fallback ladder.
