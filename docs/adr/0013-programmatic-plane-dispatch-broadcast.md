# ADR-0013: The programmatic plane — headless dispatch, broadcast, one live handle per native session

- Status: Accepted (2026-07-17 — approved by the owner via the milestone-3
  plan sign-off, satisfying AGENTS.md's ask-before for adapter-boundary
  changes)

## Context

Milestone 3 lands the deferred core of PRODUCT.md's v1 scope: dispatch from
the Home view, broadcast-and-compare, and promotion hardening. The seams
already exist and were deliberately pinned earlier: `CliAdapter::dispatch()/
follow_up()` (default `Unsupported`, `todo!()` bodies in the two active
adapters), `Task`/`Budget` in `core/task.rs`, the `AgentEvent` vocabulary,
`SessionMode::Headless`, the unused `dispatches` registry table, and — since
milestone 2d — workspace preferences in `defaults.dispatch`/
`defaults.broadcast` (ADR-0012). Codex stays suspended (ADR-0008); the seams
keep its reversal recipe intact (trait defaults compile it) but only claude +
agy get implementations.

Hard facts that shape the design (integration pages):

- claude: `-p --output-format stream-json --verbose` NDJSON; `--session-id`
  pre-assignment (no output parsing to learn the id); `-p --resume <id>` is
  **cwd-scoped**; `--max-turns`/`--max-budget-usd`/`--permission-mode` exist;
  since 2.1.163 background Bash tools die ~5s after the final result.
- agy: plain text only, no structured output; `--print-timeout` (default 5m)
  is the only hard stop; whether `-p` creates a resumable conversation, how
  to learn its id, whether `--conversation` combines with `-p`, and how
  `request-review` behaves without a TTY are all ⬜ **and settling them
  requires live dispatches — supervised only** (integration page,
  2026-07-05).
- Any spawned wrapped CLI must be env-scrubbed (`CLAUDECODE`/
  `CLAUDE_CODE_*`, plain `TERM`) — AGENTS.md gotcha; the PTY layer already
  does this for tabs; the subprocess path needs its own scrub.

## Decision

1. **Event delivery stays synchronous-polled; no async trait.**
   `DispatchHandle` keeps its `std::sync::mpsc::Receiver<AgentEvent>` but the
   `child: Child` field is replaced by a `kill: DispatchKill` handle
   (`Arc<Mutex<Child>>`-backed): the adapter's reader thread owns the child,
   reads stdout to EOF, waits the exit status, and synthesizes the terminal
   event — per-tool "what does Failed mean" stays inside the adapter. The
   app polls `try_recv` from the existing 33 ms `drive_background` tick.
   This resolves the pinned TODO ("becomes an async stream once tokio
   lands") as **no**: the app is tick-driven; an async trait would cost
   boxing/`async-trait` for nothing (ADR-0006's original objection).
2. **`Budget` speaks ADR-0012's neutral vocabulary.**
   `Budget { posture: DispatchPosture, max_turns: Option<u32>, max_usd:
   Option<f64>, timeout_secs: Option<u64> }` replaces `allow_writes: bool`;
   `DispatchPosture` moves from `core::plan` to `core::task` (plan
   re-exports it, public API stable). `Task` gains `model`/`effort`
   (verbatim strings — the core-owned mirror of `LaunchOptions`, which core
   cannot import). The router builds Budget as: ARCHITECTURE normative
   defaults ← `defaults.dispatch` ← per-task form edits. The
   never-by-default escape hatches remain unrepresentable in every layer.
3. **Per-adapter mapping (the normative guardrail table, made concrete):**
   - claude dispatch: `claude -p <prompt> --output-format stream-json
     --verbose --session-id <uuid>` in `task.cwd`; posture ReadOnly/Plan →
     `--permission-mode plan`; Edits → `--permission-mode acceptEdits
     --allowedTools Read,Glob,Grep,Edit,Write` (the table's "allowlist for
     build tasks" made concrete: file tools, **no Bash**; per-task widening
     is a later decision); `--max-turns`/`--max-budget-usd` when set;
     `--model`/`--effort` when set; `timeout_secs` has no claude mechanism
     and is ignored (documented — no swarm-side watchdog, see rejected).
     follow_up: same + `--resume <native_id>`, **run in `session.cwd`** (the
     record's cwd — the id lookup is scoped to it).
   - agy dispatch: `agy -p <prompt> --print-timeout <timeout_secs|tool
     default>` (+ `--model`); posture Edits → `AdapterError::Unsupported`
     (read-oriented tasks only until the ⬜ no-TTY permission behavior is
     verified — the *adapter* refuses, so the knowledge stays in
     `src/adapters/`). Events synthesized: `Started{native_id: None}` on
     spawn, stdout → `AgentText`, exit → `Completed`/`Failed`. **One agy
     headless run at a time** (app-serialized lane — ADR-0002's recorded
     fallback) and `follow_up` → Unsupported until the owner supervises the
     `--conversation`+`-p` verification; agy headless rows keep
     `native_id: None` and are not promotable yet.
   - Both spawns are env-scrubbed via a shared `adapters` helper (removes
     `CLAUDECODE` + every `CLAUDE_CODE_*` by prefix scan, sets plain TERM);
     also applied to `list_background_agents`.
4. **NDJSON parsing is defensive and fixture-tested** (the `reconcile.rs`
   pattern): unknown line types warn-and-skip; fields resolve with
   fallbacks; fixture `tests/fixtures/claude_stream.synthetic.ndjson`
   (synthetic, per repo convention). No live CLI in tests; the live path
   goes on the owner smoke checklist (supervised).
5. **Home UI:** Home-local keys (focused-surface scope — ADR-0007's
   reserved-key table is untouched): `i` opens the dispatch form (prompt,
   target tool/role, cwd prefilled with the launch cwd, posture/limits
   prefilled from `defaults.dispatch`; role targets prefill model/effort),
   `b` the broadcast form (multi-select; preselection =
   `defaults.broadcast`; agy-backed targets render unticked unless named
   there — ticking is the per-task opt-in, completing ADR-0012's PRODUCT
   Q5 answer). A dispatch creates the registry row (Headless/Running;
   claude rows carry the pre-assigned uuid) plus a `dispatches` row; events
   fold into a Home timeline panel and finalize both rows. Broadcast
   renders a side-by-side compare surface (status, cost, rolling text tail
   per target).
6. **One live handle per native session (promote hardening).** An app-level
   guard: a `native_id` may have at most one live attachment — an open pane
   or a running dispatch. Enter/promote on a session whose native_id is
   already attached focuses the existing tab instead of spawning a second
   process. Promotion of a **finished** headless claude row implements the
   registry→tab bridge at last: `interactive_cmd(Resume{native_id},
   record's model/effort, record.cwd)` in a fresh pane. Promoting a
   **running** dispatch asks first (the existing `pending_confirm`
   mechanism): stop the headless run, reap it, then resume interactively.
   The fork-offer (`--fork-session`) stays deferred until its semantics are
   live-verified; ADR-0002's session model is unchanged — **no supersede**;
   this implements ARCHITECTURE's recorded "one live handle" line.
7. **No registry schema change.** v3 stays; the pre-created `dispatches`
   table gets its first writers (insert on dispatch, finalize on terminal
   event). `mark_orphans` remains out of scope.

## Alternatives rejected

- **Async `CliAdapter` (tokio streams in the trait):** boxing or
  `async-trait` on every call and the object-safety friction ADR-0006
  explicitly avoided — for an app that repaints on a 33 ms tick anyway.
  Polling a std receiver on the tick is strictly simpler and keeps adapters
  dependency-free.
- **`claude --bg` as the primary dispatch channel:** attractive (the native
  supervisor owns the process), but events then come from polling
  `claude agents --json` — a schema-less surface with no observed `status`
  field — versus stream-json's typed events. Stays recorded as the long-task
  option; v1 dispatches hold the pipe.
- **A swarm-side watchdog emulating `timeout_secs` for claude:** kills a
  claude mid-write with no native cleanup; turns/budget are the honest
  native stops. agy has a real flag; claude doesn't; emulating one badly is
  worse than documenting the asymmetry.
- **Auto-backfilling agy native ids from its conversation store now:** the
  store layout is unverified (⬜, empty on this machine) and probing it for
  real means live dispatches the boundaries reserve for supervised runs.
  Serialized lane + explicit "not promotable yet" is honest; backfill lands
  after the owner's supervised verification.
- **Fork-offer now instead of the guard:** `--fork-session` is
  syntactically known but live-unverified, and the fork UX (naming, registry
  row splitting) deserves its own decision once real. The guard closes the
  correctness hole today; the fork-offer remains the recorded follow-up.
- **A dispatch prefix key (amending ADR-0007):** dispatch/broadcast are
  Home-surface actions like j/k/Enter, not global commands; Home-local keys
  keep the reserved-key budget at exactly one.

## Consequences

- `adapters/mod.rs`: `DispatchHandle` reshape + shared spawn/scrub/stream
  helpers; both active adapters implement dispatch (+ claude follow_up);
  codex keeps compiling on the trait defaults (reversal recipe untouched).
- `core/task.rs`: Budget reshape + Task model/effort; `core/plan.rs`
  re-exports `DispatchPosture` from its new home.
- `app`: dispatch/broadcast forms, timeline panel, compare surface, the
  one-handle guard, running-promote confirm, serialized agy lane state.
- Registry rows for headless runs exist end-to-end for the first time
  (create → events → finalize), exercising `SessionMode::Headless`.
- New tests: adapter argv construction, NDJSON parser fixtures, synthesized
  agy events via `sh -c` fakes through the shared runner, app fold-in with
  hand-built handles, guard/promote/confirm flows. Live dispatch stays on
  the owner smoke checklist (supervised), per the integration-page
  constraint.
- Revisit when: agy's ⬜ dispatch items are settled supervised (backfill +
  follow_up + posture unlock), `--fork-session` is live-verified
  (fork-offer), or a long-task path wants `--bg` + supervisor polling.
