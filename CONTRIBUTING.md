# Contributing

This repo is documentation-first: the docs are the product until the code catches up.
If a change makes the code and the docs disagree, the change isn't done.

## Ground rules

- Read `AGENTS.md` first — it is the canonical brief. `CLAUDE.md` is a shim over it.
- The one thing that must never break: **no new auth flows, and never read, print, or
  copy the contents of credential files** (`~/.codex/auth.json`, anything under
  `~/.claude/` or `~/.gemini/`, OS keyrings). Path-existence checks only.
- Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`).
- `cargo fmt` and `cargo clippy -- -D warnings` must pass before a commit.

## Architecture decisions

Decisions live in `docs/adr/`. The rule is **supersede, never edit**: if reality
contradicts ADR-000N, write ADR-000M ("Supersedes ADR-000N") explaining what was
learned. Small factual corrections (a flag renamed upstream) go in
`docs/integrations/*.md` with a date, not in ADRs.

## Facts about the wrapped CLIs

Any claim about `claude`, `agy`, or `codex` behavior belongs in
`docs/integrations/<tool>.md`, marked ✅ (verified locally or in official docs),
🔶 (reputable secondary source), or ⬜ (unverified). Include the tool version and the
date. Code comments may point at these pages but must not restate flag tables —
they drift.

## Adding an adapter

1. Read ADR-0006 (the boundary) and ADR-0001 (the strategy it implements).
2. Create `docs/integrations/<tool>.md` with verified capabilities before writing code.
3. Add a variant to `AdapterKind` and a module under `src/adapters/`.
4. Implement the minimum viable adapter first: `binary()`, `probe()`,
   `interactive_cmd()` — that alone earns a tab. `dispatch()`/`follow_up()` only if
   the tool has a real headless mode.
5. Never let tool-specific flags, paths, or JSON shapes leak outside your adapter
   module. If `src/core/` or `src/app/` needs an `if tool == X`, the boundary is
   wrong — fix the boundary.

## Testing

- Adapter parsing tests run against **recorded fixtures** (captured JSONL/text in
  `tests/fixtures/`), never against live CLIs.
- Anything that must run against a real binary goes behind `#[ignore]` with a comment
  naming the required tool + version.
- `scripts/verify-clis.sh` is the only sanctioned way to poke the real environment,
  and it stays read-only.
