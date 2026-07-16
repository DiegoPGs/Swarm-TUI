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

## Local verification — 2026-07-05 (first implementation session, target machine)

`scripts/verify-clis.sh` plus follow-up read-only probes on the CachyOS daily driver.
Full detail is folded into `docs/integrations/*.md`; the deltas that matter:

| Check | Result |
| --- | --- |
| Claude Code | **2.1.201** at `~/.local/bin/claude`, logged in; every scripted check green |
| Antigravity CLI | **v1.0.14** at `~/.local/bin/agy` — *older* than the v1.0.16 the remote pass targeted; local flag surface matches the design's assumptions (still no structured output) |
| Codex CLI | **not installed** (no binary on bash/fish PATH, no `~/.codex/`) — owner installs it manually; the probe-downgrade path covers the interim |
| Rust toolchain | was absent → installed rustup **stable 1.96.1** user-locally with `--no-modify-path` (fish config untouched; add `~/.cargo/bin` to PATH by hand) |
| Pinned dep set | resolves and type-checks together (ratatui 0.30.2 / tui-term 0.3.4 / portable-pty 0.9.0 / vt100 0.16.2 / rusqlite 0.40.1) |

**ADR review of the divergences (first-session checklist step 3): none invalidated,
no superseding ADR needed.**

1. `claude mcp serve` **exists** at 2.1.201 — the current online command reference
   omits it and misled the 2026-07-04 pass. This fires ADR-0001's "revisit when
   Claude's command reference (re)documents an MCP server mode" trigger. Assessment:
   `mcp serve` exposes Claude Code's *tools* over MCP, not whole-agent dispatch; agy
   still has no server mode; stream-json remains the richer channel. The dual-channel
   decision stands unchanged.
2. agy's conversation store: `~/.gemini/antigravity-cli/conversations/` exists but is
   empty (machine has never run an agy conversation) and no `*.db` exists under
   `~/.gemini`. ADR-0002 already carries the fallback (serialize agy dispatch, lean on
   `-c`) if ID backfill proves unreliable — nothing to supersede yet; the behavioral
   tests are deferred to a supervised session because they write to the real store.
3. Codex missing entirely is the ARCHITECTURE "CLI missing" failure mode working as
   designed (probe downgrade + roster badge), not a decision change.

Also this session: project renamed **overstory → swarm-tui** (owner's decision — the
README "Naming" question is closed) and the pinned `[dependencies]` were enabled in
`Cargo.toml`.

## Fidelity spike results — 2026-07-05 (ADR-0003 gate: **PASSED**)

`examples/fidelity_spike.rs` (`cargo run --example fidelity_spike`) spawns each
installed CLI's real TUI in a `portable-pty` at 120×40, replays the byte stream
through `vt100` 0.16.2, renders the parsed screen through `tui_term::PseudoTerminal`
0.3.4 into an off-screen ratatui buffer, types characters (never Enter — nothing is
submitted, no model call), resizes to 100×30, and snapshots every stage to
`target/fidelity-spike/*.txt`.

| | claude 2.1.201 (Ink/React) | agy 1.0.14 (Go) | codex |
| --- | --- | --- | --- |
| boot → stable paint | 1.2 s | 2.8 s | not installed — rerun after install |
| render fidelity | block-art logo, box rules, status line, prompt box all exact | trust dialog, menu, model badge all exact | — |
| typed-char echo | ✅ appears in the prompt box | n/a — boot lands on the workspace-trust *menu*, which ignores character keys (correct UX, not a defect) | — |
| resize 120×40 → 100×30 | repaints exactly at the new width (separator rules re-render at 100 cols) | repaints cleanly | — |
| tui-term buffer vs vt100 screen | 1 differing line = tui-term draws the cursor block `█` (a feature, not a defect) | 0 differing lines | — |
| alt-screen / color | alt-screen ✅, 334 colored cells | alt-screen ✅, 122 colored cells | — |

**Verdict: ADR-0003 confirmed. The pane layer stays on `vt100` + `tui-term` — no
`wezterm-term`, no tmux fallback.** Zero rendering defects observed on either
installed CLI.

Carry-forwards for the real `src/pty/` layer:

- agy's first open in a workspace lands on its trust dialog; a session tab just
  renders it and lets the user answer (the "inherits its full UX including approval
  prompts" property working as intended). The spike deliberately did not answer it.
- Store side-effects of PTY spawn + kill: none. agy created no conversation row and
  claude persisted no session file for a killed pre-prompt TUI (both re-checked by
  filename listing after the run).
- Not yet exercised — fold into the pane-layer build-out or a second spike pass:
  mouse reporting, scrollback capture, resize under *rapid streaming* output (needs a
  live turn, i.e. a supervised session), and codex entirely (blocked on install).

## Milestone 2a complete — 2026-07-15 (Stage E, final stage of "the shell")

Stages A1 through E all landed this milestone: registry + XDG config (A1, `c655e5e`),
the PTY layer / `LocalPaneHost` (A2, `d99bf58`), the `LaunchIntent::Fresh` breaking
change + ADR-0007 (B, `23aa327` + `cdc0ede`), the full app shell (C, `2862b03`), Claude
Code background-agent reconciliation (D, `9cbf488`), and this stage's CI workflow plus
docs pass (E). `cargo run` now boots a real terminal shell instead of a scaffold.

**`LaunchIntent::Fresh` breaking change (Stage B).** `Fresh` went from a unit variant
to `Fresh { session_id_hint: Option<String> }` so a caller can pre-assign a native
session id before a fresh interactive tab spawns, mirroring the existing headless
pre-assigned-ID pattern (ADR-0002). Touched: `src/adapters/mod.rs` (the enum
definition) plus the three adapter files that match on it — `src/adapters/
claude_code.rs` (the only one that acts on the hint, via `--session-id <hint>`),
`src/adapters/antigravity.rs`, and `src/adapters/codex.rs` (both just widen the match
arm and ignore the hint — neither tool has a documented pre-assign flag).

**`PaneHost` trait additions (Stage A2/C).** `with_screen`, `kill`, and
`exit_success` were added to `src/pty/mod.rs`'s `PaneHost` trait: `with_screen`
finalizes the render-surface seam (locks the pane's `vt100::Screen` only for the
duration of a closure, so no lock-guard type leaks across the trait boundary — needed
because a background reader thread mutates the parser); `kill` backs tab-close
(prefix `x`); `exit_success` distinguishes "still running" from "exited, need the
reason" for the `Completed`/`Failed` registry-status decision on close.

**Deviation: no `event-stream` cargo feature.** `src/app/mod.rs`'s event loop needs
async keyboard input alongside PTY-output and render-tick events. Crossterm's async
`EventStream` requires enabling the `event-stream` cargo feature, which pulls in an
unreviewed transitive dependency; instead the event loop spawns a dedicated blocking
thread looping on `crossterm::event::read()` that forwards into a
`tokio::sync::mpsc::unbounded_channel`, merged via `tokio::select!` alongside the
pane-changed channel and a 33ms render tick. Same practical effect, zero `Cargo.toml`
change.

**Reconciliation fixture provenance (Stage D).**
`tests/fixtures/claude_agents_all.synthetic.json` is synthetic, not real transcript
content. The real `claude agents --json --all` *shape* was observed structurally on
this machine (2026-07-15, `claude` 2.1.210): a bare top-level JSON array, entries
keyed `sessionId` (camelCase — not `id`/`session_id`), and no `status` field at all.
That observation is recorded as a dated addendum in `docs/integrations/
claude-code.md` (field names only — no session content, per AGENTS.md). Both the
fixture and `src/app/reconcile.rs`'s parser (which accepts all three id-key
spellings and defaults `status` to `"unknown"`) were committed only after the repo
owner's explicit go-ahead in chat, per AGENTS.md's rule against committing anything
derived from a real session transcript without that sign-off.

**CI.** `.github/workflows/ci.yml` added: checkout, `dtolnay/rust-toolchain@stable`
(with `rustfmt`/`clippy` components), `Swatinem/rust-cache@v2`, then `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings`, `cargo check`, `cargo test` on
`ubuntu-latest`. No extra apt step needed — `rusqlite`'s `bundled` feature compiles
SQLite from C source and `ubuntu-latest` ships `build-essential`; no test shells out
to a wrapped CLI (PTY tests use plain `sh`, reconciliation tests read the fixture
above), so nothing here is CI-environment-fragile.

**Codex bonus verification: skipped this session.** Codex CLI is still not installed
on this machine (`command -v codex` fails) — the codex-verification pass from the
Stage E brief (re-run `scripts/verify-clis.sh`, settle the `docs/integrations/
codex.md` ⬜ items, re-run the fidelity spike) stays deferred until the owner
installs it, unchanged from the 2026-07-05 entry above.

## Milestone 2b — Stage 0: local validation (2026-07-16)

**Gates: all green.** `cargo fmt --check`, `cargo clippy --all-targets -- -D
warnings`, `cargo check`, `cargo test` (29/29), and `cargo run --example
fidelity_spike` all pass on the target machine. Installed versions this session:
`claude` **2.1.211**, `agy` **1.1.3**, codex still absent.

**CI: enabled and green.** The `CI` workflow has two successful runs on GitHub —
`29387475686` (push to main, 2026-07-15, the milestone-2a merge) and `29386886662`
(the PR run). Nothing needed enabling.

**Fidelity spike re-run (claude 2.1.211 / agy 1.1.3) — ADR-0003 verdict unchanged.**
Two report lines differ from the 2026-07-05 run, both explained and neither a
rendering defect:

- *claude, "1 differing line" (widget vs vt100)*: the diff is the **cursor cell** —
  `tui_term::PseudoTerminal` paints `█` at the cursor position while
  `vt100::Screen::contents()` renders no cursor glyph. Comparison artifact of the
  spike's proxy check, not a fidelity failure (on 2026-07-05 the cursor happened to
  sit on content the proxy trimmed).
- *agy, "echo of typed chars: DEFECT"*: the repo dir is an untrusted agy workspace,
  so agy boots to its workspace-trust dialog, which by design ignores character keys
  (recorded 2026-07-05). Chars typed at that dialog don't echo. Expected state, not
  a defect. Incidental drift observation from the dialog footer: agy 1.1.3's default
  model indicator reads **"Gemini 3.1 Pro (High)"** (the page recorded a 3.5 Flash
  default at v1.0.14–1.0.16) — refreshed in the Stage 2 pass.

**2a smoke checklist: executed, all steps OK.** Deviation to note: the milestone-2a
entry above recorded no smoke checklist, so the checklist executed here is the one
from the milestone-2b brief. tmux isn't installed, so the checklist is driven by a
new PTY harness, `examples/shell_smoke.rs` (`cargo build && cargo run --example
shell_smoke`), which boots the real `target/debug/swarm-tui` inside `portable-pty`
with `SWARM_TUI_DATA_DIR` pointed at a throwaway tempdir (the real registry is never
touched) and gates every step on an on-screen vt100 marker. Enter is pressed only on
swarm-tui's own surfaces (picker/roster/confirms); wrapped panes receive printable
characters only. Results (2026-07-16):

| Step | Result |
| --- | --- |
| Home paints (empty roster) | OK |
| prefix banner (`Ctrl-Space`) → picker (`c`) | OK |
| claude tab: paint (`--session-id <uuid>` spawn path) | OK |
| claude tab: typed characters echo (no Enter) | OK |
| claude tab: repaint after 120×40 → 100×30 resize | OK |
| detach (`d`) → Home shows `[detached]` badge | OK |
| re-attach (Enter on roster row) restores the pane | OK |
| close (`x`, confirm) → row lands `Failed` (killed) | OK |
| agy tab: pane paints — trust dialog renders | OK (dialog visible) |
| close agy, quit (`q`) — clean exit 0 | OK |

Incidental ✅: the claude spawn path proves `claude --session-id <uuid>` is accepted
on an **interactive fresh spawn** at 2.1.211 — stamped in
`docs/integrations/claude-code.md`.

**Open-⬜ review (read-only pass):** `claude-code.md` had no open ⬜ items.
`antigravity.md`'s three ⬜ items all require a live headless dispatch ("run
supervised") and stay open by design this milestone.
