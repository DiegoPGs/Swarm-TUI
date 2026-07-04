<!--
SHIM — this file follows the repo-standard two-file pattern: AGENTS.md is the
canonical, tool-agnostic brief (Codex and agy read it natively); CLAUDE.md imports it
and adds only genuinely Claude-specific instructions. Claude strips HTML comments
before loading, so this block costs zero context tokens.

WHERE INSTRUCTIONS BELONG AS THE REPO GROWS
- AGENTS.md            Always-loaded facts: commands, architecture, conventions,
                       definition of done, boundaries. Deltas only, <100 lines.
- .claude/rules/*.md   Modular topic rules with `paths:` frontmatter (add when needed).
- .claude/skills/      Multi-step procedures loaded on demand, not every session.
- Hooks (settings)     Things that MUST happen. CLAUDE.md is guidance; hooks enforce.
- CLAUDE.local.md      Personal, per-repo, gitignored.
- ~/.claude/CLAUDE.md  Global personal preferences.

MAINTENANCE
- Two-mistakes rule: a correction typed twice in chat becomes a line in AGENTS.md
  (or a rule/skill/hook per the map above).
- Run /memory to audit what's loaded; prune anything not earning its context cost.
-->

@AGENTS.md

## Claude-specific

**Read order for a cold start:** `docs/ARCHITECTURE.md` → the ADR relevant to your task
(`docs/adr/`) → the integration page for any CLI you'll touch
(`docs/integrations/`) → `docs/NOTES.md` for what's verified vs. inferred.

**First implementation session — do this before writing any code:**

1. Run `./scripts/verify-clis.sh` on this machine (the design session ran in a sandbox
   without the three CLIs installed; every fact marked *verify locally* in
   `docs/integrations/` is waiting on this).
2. Update the three integration pages: flip confirmed items to ✅ with the observed
   version, correct anything that diverged, and record exact session-store paths.
3. If a divergence invalidates part of an ADR, write a superseding ADR — don't silently
   design around it.
4. Only then: `cargo init` housekeeping (`cargo check` against the pinned deps) and the
   ADR-0003 fidelity spike (render each CLI's TUI through `portable-pty` + `tui-term`
   and note rendering defects). The spike result decides whether the pane layer stays
   on `vt100` or moves to the documented fallback.

**Working style in this repo:** use plan mode for any change under `src/adapters/` or
to the `CliAdapter` trait; those are the contract surfaces. Background/long tasks are
fine, but never point them at the user's real `~/.claude`, `~/.codex`, or `~/.gemini`
state beyond the read-only checks the boundaries allow.
