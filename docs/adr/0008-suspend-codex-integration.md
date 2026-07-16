# ADR-0008: Suspend the Codex integration (reversible)

- Status: Accepted (2026-07-16)

## Context

Codex CLI has never been installed on the target machine (verified 2026-07-05,
2026-07-15, and 2026-07-16 — no binary, no `~/.codex/`). Every codex fact in
`docs/integrations/codex.md` is remote-verified at 0.142.5 with its ⬜ items blocked
on a local install, which swarm-tui's own boundary forbids performing (AGENTS.md:
never run installers). Meanwhile the tool ships fast enough that the recorded facts
decay while unverifiable. Milestone 2b builds the command plane (palette + launch
options) on locally-verified behavior only — a standard codex cannot meet.

ADR-0001 decides each tool's integration channels *independently per tool*, and
ADR-0006 makes tool membership a compile-time registry: one enum variant + one module
+ one registry entry. That makes "suspend one tool" a naturally small, reversible
operation rather than a redesign.

## Decision

Codex is **out of active scope** until the owner installs it and re-verifies. Mechanically:

- `registry()` (src/adapters/mod.rs) returns only `[ClaudeCode, Antigravity]` — its
  type becomes `&'static [AdapterKind]` so suspending/reinstating a tool is editing
  one const, never a signature change. Everything that iterates tools flows from
  `registry()`: the startup capability probe, the new-session picker, and therefore
  every spawn path. None of them can produce a codex session anymore.
- `AdapterKind::Codex`, `src/adapters/codex.rs`, every enum-dispatch arm, and
  `from_slug("codex")` **stay compiled** — compile-time exhaustiveness stays intact,
  the recorded adapter logic keeps type-checking against the pinned deps, and
  historical registry rows keep resolving their slug.
- The Home roster still renders historical `tool = "codex"` rows **read-only**:
  roster rendering is pure string display of `SessionRecord`s, and the only roster
  action (Enter → re-attach) applies exclusively to sessions with a *live detached
  pane*, which a suspended tool cannot have.
- `docs/integrations/codex.md` gets a "⏸ SUSPENDED" banner; its facts freeze at their
  recorded dates. `scripts/verify-clis.sh` keeps its codex section — it is the
  reversal tool.

**Reversal recipe:** restore the entry in `registry()`; after the owner installs
codex, run `./scripts/verify-clis.sh` and `cargo run --example fidelity_spike`,
settle the ⬜ items in `docs/integrations/codex.md`, and populate the adapter's
command table / launch declaration (ADR-0009) from local observation. No schema,
trait, or app change is needed.

This ADR **amends the scope** of ADR-0001 (three wrapped CLIs → two active, one
suspended) without superseding its per-tool integration strategy: the codex decision
block in ADR-0001 remains the recorded plan for when codex returns.

## Alternatives rejected

- **Delete the adapter, variant, and page.** Loses compile-time exhaustiveness over
  the adapter surface, deletes working, reviewed code, and turns reversal into
  re-implementation. The suspension is expected to be temporary.
- **Keep codex in the registry and let the failed probe grey it out.** That is the
  right mechanism for a *transiently* missing tool, but as a permanent state it
  renders a dead picker row that implies "install me" — while the boundary forbids
  swarm-tui from doing anything about it — and forces every future feature (command
  tables, launch options) to carry an unverifiable codex column.

## Consequences

- `probe_cache` never contains a `Codex` entry; code must not assume all
  `AdapterKind` variants are probed (the picker/attach gates already treat a missing
  probe as "not offered").
- ADR-0009's per-adapter tables are populated for active adapters only; the codex
  adapter compiles with an empty command table and no launch declaration.
- README, ARCHITECTURE, and AGENTS.md describe two active tools plus a suspended one;
  the "shape of it" diagram drops the codex lane.
- Revisit when: the owner installs codex (run the reversal recipe), or an ADR-0001
  "revisit when" trigger fires for codex (e.g. headless fork ships).
