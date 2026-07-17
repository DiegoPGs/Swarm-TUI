# ADR-0012: Workspace personalization — plan schema v2 + a personal overlay

- Status: Accepted (2026-07-17)

## Context

ADR-0010 gave a workspace one committed declaration: `.swarm/swarm.json`,
schema v1, roles only. Two pressures on it now:

1. **Workspaces have preferences beyond roles.** Which role a new session
   usually is, which roles a broadcast should target, how tight the headless
   dispatch guardrails should sit (milestone 3 lands both consumers), and
   where dispatched work runs (PRODUCT.md question 4). These are per-project
   facts and belong next to the roles that they reference.
2. **Committed is not personal.** Claude Code splits per-repo configuration
   into a shared, committed layer (`CLAUDE.md`, `.claude/settings.json`) and
   a personal, gitignored one (`CLAUDE.local.md`,
   `.claude/settings.local.json`). swarm-tui's owner uses it daily across
   repos and wants the same split: a team declares `coder`/`reviewer`; an
   individual adds a private `scratch` role or prefers a different default
   without committing that.

## Decision

### Schema v2: a `defaults` object joins `roles`

`.swarm/swarm.json` may declare `"version": 2`, which is v1's `roles`
unchanged plus one optional top-level object:

```json
{
  "version": 2,
  "roles": { "coder": { "tool": "claude-code", "model": "opus-4.8" } },
  "defaults": {
    "default_role": "coder",
    "broadcast": ["coder", "advisor"],
    "dispatch": { "posture": "plan", "max_turns": 30,
                  "max_budget_usd": 2.5, "timeout_secs": 300 },
    "worktrees": "in_place"
  }
}
```

Every field is optional; defaults are **preferences, not policy** — the same
stance as ADR-0010's "presets, not enforcement".

- `default_role`: the picker preselects this role's row (cursor position
  only — Enter still asks; a greyed row just leaves Enter a no-op). Must name
  a defined role.
- `broadcast`: the role names a broadcast targets by default (milestone 3
  consumes it). **Naming a role here is the explicit opt-in for its tool** —
  this is how PRODUCT.md question 5 resolves: nothing is broadcast-targeted
  by default without being named, so the quota-shared tool (agy — quota burns
  across CLI, desktop app, and subagents; see the integration page) is out
  unless the workspace or the per-task UI names it. No `include_<tool>`
  flag: the file already names tools only inside roles, and a per-tool
  boolean would hardcode one vendor's quirk into cross-tool schema.
- `dispatch`: neutral guardrail preferences for headless dispatch —
  `posture` (`read_only` | `plan` | `edits`), `max_turns`, `max_budget_usd`,
  `timeout_secs`, all optional. The **normative** defaults remain the
  ARCHITECTURE.md table ("Guardrail defaults for headless dispatch"); this
  object only tightens or selects among them, and each adapter maps the
  neutral fields to its own flags at dispatch time (milestone 3). The
  postures the table forbids by default (`bypassPermissions`,
  `danger-full-access`, `--dangerously-skip-permissions`) are
  **unrepresentable**: the enum has no such value. Limits must be positive;
  per-tool flag names never appear (ADR-0009 vocabulary rule).
- `worktrees`: policy slot for where dispatched work runs. This build
  accepts `in_place` only; `per_task` is **reserved** (PRODUCT.md question
  4) and rejected with a message saying so — a schema that silently ran
  in-place under a label promising isolation would be worse than an error.
  Behavior lands with dispatch; the slot exists now so committed files have
  a stable place for it.

**The ADR-0010 guarantee extends to v2: no field accepts credentials,
tokens, or paths to them — there is still no place where a secret could
legally live, and `deny_unknown_fields` on every struct (including
`defaults` and `dispatch`) keeps one from being smuggled in.** A v1 file
carrying a `defaults` key is rejected ("requires version 2") rather than
silently ignored.

### The personal overlay: `.swarm/swarm.local.json`

Same schema, same versions (a v1 overlay is legal), gitignored
(`.swarm/*.local.json`). Loaded and merged at the same single load site,
reloaded by the same prefix+`r`.

**Merge rule — shallow, per named entry, local wins.** Both top-level
sections are maps: `roles` keyed by role name, `defaults` keyed by field
name. The overlay merges per key at exactly that one level, and a winning
entry replaces the shared one **wholesale**:

- A local role with a new name is added; a local role with a shared name
  replaces that role entirely (it does not inherit the shared role's model
  or effort — restate what you want).
- A local `default_role` overrides the shared one while a shared
  `dispatch` the overlay doesn't mention survives; a local `dispatch`
  replaces the shared `dispatch` as one unit (no per-limit merge).

**Failure posture (ADR-0010's, extended per layer):** a missing file means
that layer is simply absent (both absent ⇒ no plan, no error). A broken
layer — unreadable, malformed, wrong version, invalid role, invalid
defaults — fails the **whole** load with the existing one-line picker error
**naming the offending file**; never a crash, never a partial load. Partial
would be worse than none here: loading shared-without-local would silently
drop exactly the personal overrides the user relies on. A present layer
must be entirely valid on its own terms (shape, version, role tools,
startup commands, defaults value domains) — committed files are validated
as their other consumers will see them, so a local override cannot mask a
broken shared file. Only cross-references (`default_role` and `broadcast`
naming roles) resolve against the *merged* role set — they may legally
point at a role the other layer defines — and a bad reference blames the
layer whose entry won. Error wording: the shared file's role-level messages
keep their exact pre-overlay form (pinned by tests and picker UX); overlay
messages prefix `.swarm/swarm.local.json:`.

## Alternatives rejected

- **A separate settings file** (`.swarm/settings.json` beside
  `swarm.json`) — mirrors Claude Code's file layout, but the split earns
  nothing here: `defaults` reference roles by name (`default_role`,
  `broadcast`), so a second file would need cross-file validation, a second
  failure surface in the picker, and a second reload path, to hold ~10
  lines. Claude Code's split separates *prose context* from *machine
  settings*; swarm-tui has no prose file — one schema, one loader, one
  error line. The shared/personal split (the part of Claude Code's model
  that pays) is exactly what the overlay provides.
- **TOML/YAML** — rejected in ADR-0010 (a new dependency for format taste);
  nothing about v2 changes that.
- **Defaults in the registry DB** — rejected in ADR-0010 for roles and
  doubly wrong for defaults: the committed file is the *shareable*
  declaration, and a per-machine SQLite row is the opposite of "my team
  gets the same broadcast set". The registry keeps recording what a session
  *was* launched as; it does not hold preferences.
- **Deep (recursive) merge for the overlay** — merging *inside* a role or
  inside `dispatch` ("local coder overrides only `model`") reads
  attractively but fails in practice: JSON has no "unset" marker, so a
  local layer could never *remove* a shared field (set `effort` back to
  tool-default), and what's in effect becomes the mental diff of two files
  at arbitrary depth. Wholesale-per-entry keeps the answer to "what is
  `coder` right now?" one file lookup; restating a five-line role is
  cheaper than reasoning about a recursive merge.
- **Wholesale-section replace** (the other shallow extreme: a local
  `defaults` object replaces the shared one entirely, roles too) — makes
  the overlay hostile: adding one personal role would delete the team's
  roles from your picker, and overriding `default_role` would silently drop
  the shared dispatch guardrails — plus every later change to the shared
  section stops reaching you. Per-entry is the smallest unit that keeps
  "add mine, keep theirs" true.
- **An editable defaults form in the TUI** — same rejection as ADR-0010's
  editable role form: the files are the source of truth and a text editor
  is their editor; swarm-tui reads, validates, and reports.

## Consequences

- `core/plan.rs` owns the v2 types (`Defaults`, `DispatchPrefs`,
  `DispatchPosture`, `WorktreePolicy`), both layer paths, and the merge;
  slug lists stay caller-passed (ADR-0006 direction unchanged). `app`
  consumes exactly one new thing today: `defaults.default_role` for the
  picker preselect. `broadcast`, `dispatch`, and `worktrees` are loaded,
  validated, carried — and consumed by milestone 3.
- `.gitignore` gains `.swarm/*.local.json`. The repo's own
  `.swarm/swarm.json` moves to v2 and dogfoods a `defaults` block
  (`default_role: "coder"`, broadcast without the agy role, posture
  `plan`, `worktrees: "in_place"`).
- `examples/shell_smoke.rs` navigates the picker by position and now
  starts from the dogfooded preselect ("coder", row 1) — its step counts
  changed and it assumes no local overlay in the repo root (documented in
  the harness).
- v1 files keep loading, byte-for-byte identically, indefinitely; the
  version error now reads "this build reads versions 1 and 2".
- Milestone 3 consumes `defaults.dispatch` and `defaults.broadcast`
  through the router; if their shape needs to grow (e.g. per-tool posture
  overrides), that is a v3 discussion, not a silent field addition —
  `deny_unknown_fields` makes that boundary self-enforcing.
