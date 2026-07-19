# Findings ledger

Two tranches. **Tranche 1 (F-001..F-005)** — 2026-07-17 static audit fixes, all
VERIFIED-CLOSED. **Tranche 2 (F-006..F-013)** — spec-compliance findings opened
2026-07-18; read the provenance note heading that section before citing any of
them. Each entry tracks status with the command output that closed it.

## Tranche 1 — 2026-07-17 audit fixes

Five findings from the 2026-07 static audit (which could not compile the tree).
Toolchain used for verification: rustc 1.97.1 stable (plus 1.94.1 / 1.95.0 /
1.96.0 for the F-005 MSRV bisect).

Full-suite verification (run after all fixes, rustc 1.97.1):

```
$ cargo fmt --check          # exit 0, no output
$ cargo clippy --all-targets -- -D warnings
    Checking swarm-tui v0.0.1 (/home/user/Swarm-TUI)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.83s
$ cargo test
test result: ok. 88 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.96s
```

---

## F-001 (Medium) — concurrent-instance session clobbering

**Status: VERIFIED-CLOSED**

- Where: `src/store/mod.rs` — `allocate_id()` (`SELECT MAX(id)+1`) followed by
  `upsert()`'s `INSERT … ON CONFLICT(id) DO UPDATE`.
- Fix: replaced the racy allocate-then-upsert pair with `Registry::create()`,
  which INSERTs without an id and lets SQLite assign it atomically under its
  write lock (`last_insert_rowid()` is returned). Two writers can no longer
  land on the same id; the second insert gets the next id instead of
  overwriting the first row. `upsert()` is unchanged and still updates
  existing rows in place (status updates on close, `upsert_updates_in_place_and_keeps_created_at`
  still passes). `allocate_id()` removed; its one production caller
  (`App::open_new_session`) now uses `create()`.
- Closing evidence — new test opening two `Registry` handles on one temp DB
  file, each creating a fresh session, asserting two distinct surviving rows:

```
$ cargo test two_handles_creating_fresh_sessions_never_clobber
test store::tests::two_handles_creating_fresh_sessions_never_clobber ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 87 filtered out; finished in 0.04s
```

## F-002 (Low-Medium) — session-close status write silently dropped

**Status: VERIFIED-CLOSED**

- Where: `src/app/mod.rs`, `perform_close_active_tab()` (`let _ = self.registry.upsert(record)`
  and the `if let Ok(…) = self.registry.all()` that skipped read errors).
- Fix: both failure branches now emit `tracing::warn!` with the session id and
  the error; the close path stays non-fatal (tab still closes).
- Closing evidence — two new tests drive the real close path against a
  registry broken underneath the app (an UPDATE-blocking trigger simulating a
  read-only file, and a dropped table for the read branch) with a capturing
  tracing subscriber. Observed warning from the failing run during
  development (proving the emission, before the assertion was adjusted for
  ANSI codes):

```
WARN swarm_tui::app: failed to persist close status; registry row left as-is session_id=1 error=Query("registry is read-only")
```

```
$ cargo test close_warns
test app::tests::close_warns_when_the_status_write_fails ... ok
test app::tests::close_warns_when_the_registry_read_fails ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 86 filtered out; finished in 0.16s
```

## F-003 (Low) — mutex poison cascade in the PTY host

**Status: VERIFIED-CLOSED**

- Where: `src/pty/local.rs` — every `lock().unwrap()` on `parser`, `child`,
  and `exit_success` (reader thread, `with_screen`, `resize`, `kill`,
  `exit_success`).
- Fix: added `lock_ignore_poison()` (`lock().unwrap_or_else(PoisonError::into_inner)`)
  and routed all seven lock sites through it. Locking structure otherwise
  unchanged; a poisoned pane now degrades that pane instead of panicking
  every later lock site.
- Closing evidence — new test poisons one pane's parser mutex via a panicking
  thread, then exercises `with_screen`/`resize` on a second pane (and
  `with_screen` on the poisoned pane) without panics:

```
$ cargo test poisoned_pane_lock_degrades_that_pane_only
test pty::local::tests::poisoned_pane_lock_degrades_that_pane_only ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 87 filtered out; finished in 0.24s
```

## F-004 (Low) — timestamp helpers panic on boundary values

**Status: VERIFIED-CLOSED**

- Where: `src/store/mod.rs` — `system_time_to_unix()` (`duration_since(…).unwrap()`)
  and `unix_to_system_time()` (`v as u64` then unchecked `SystemTime` addition).
- Fix: write side clamps a pre-epoch `SystemTime` to `0`; read side (the real
  boundary — the DB file) clamps negative values via `u64::try_from` and
  overflow via `UNIX_EPOCH.checked_add`, both landing on `UNIX_EPOCH` instead
  of panicking.
- Closing evidence — unit tests for both helpers plus an end-to-end read of a
  raw row persisted with negative timestamps:

```
$ cargo test pre_epoch
test store::tests::pre_epoch_system_time_clamps_to_epoch_on_write ... ok
test result: ok. 1 passed; 0 failed; …
$ cargo test timestamp
test store::tests::out_of_range_stored_timestamp_clamps_instead_of_panicking ... ok
test store::tests::negative_stored_timestamps_read_back_clamped ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 86 filtered out; finished in 0.03s
```

## F-005 (Low) — declared MSRV below what dependencies require

**Status: VERIFIED-CLOSED**

- Where: `Cargo.toml` `rust-version = "1.75"`.
- Fix: set `rust-version = "1.95"`, the lowest rustc on which `cargo check`
  actually succeeds for this tree. The binding constraint is not ratatui's
  declared 1.88 but `libsqlite3-sys 0.38.1`, whose build script uses
  `cfg_select!` (stable from 1.95).
- Closing evidence — bisect over installed toolchains:

```
$ cargo +1.94.1 check        # (session default before update)
error[E0658]: use of unstable library feature `cfg_select`
   --> …/libsqlite3-sys-0.38.1/build.rs:110:9
error: could not compile `libsqlite3-sys` (build script) due to 1 previous error

$ cargo +1.95.0 check
    Checking swarm-tui v0.0.1 (/home/user/Swarm-TUI)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.30s

$ cargo +1.96.0 check
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.58s
```

---

# Tranche 2 — spec-compliance findings (F-006..F-013)

## Provenance — read before citing any finding below

**These are not an external auditor's findings.** The instruction that opened
this work said to seed F-006..F-013 "verbatim from the 2026-07-18 external audit
report." No such report exists. It is not in this repo, on any local or remote
branch, anywhere in `git log --all --diff-filter=A`, or in the owner's
`~/Downloads`, `~/Documents`, or `~/.claude`. The only occurrences of its
distinctive vocabulary (`W-NFR-3`, `worker_submit_task`) are the instruction text
itself and the session transcript that received it.

The source of record for F-006..F-011 is therefore **the work order in that
instruction** — one clause per finding — plus a repo-vs-spec reading performed on
2026-07-18 and recorded below. This was confirmed with the owner before seeding.
Nothing here should be attributed to an audit.

**F-012 and F-013 have no definition in any available input.** They appear only
as an undescribed mention ("fold into the phases they touch"). They are seeded
BLOCKED-ON-INPUT rather than silently dropped.

**Spec under test:** `docs/requirements/20-worker-tui-requirements.md`, vendored
into the repo on 2026-07-18 from the owner's `~/Downloads` — it had never been
in-tree despite being cited by path. Its own frontmatter reads `status:
assumption`, `source: agent`, `model_used: claude-fable-5`, `confidence: medium`,
`volatility: high`. It is an AI-drafted proposal, not a ratified spec: **where it
collides with a human-accepted ADR, the ADR wins and the spec item becomes a
decision to be made, not a defect to be fixed.**

Toolchain for this tranche: rustc 1.96.1 stable. Baseline before any change:
`cargo test` → 157 passed, 0 failed.

Phase map: P0 = F-011 + F-010 · P1 = F-007 · P2 = F-008 · P3 = F-009 · P4 = F-006.

---

## F-006 — no MCP server surface (W-19..W-24)

**Status: OPEN** — scheduled P4.

- Spec: W-19 (`worker_submit_task` / `worker_get_task_status` /
  `worker_list_tasks` / `worker_cancel_task`), W-20 (schemas), W-21 (accurate
  `readOnlyHint`/`destructiveHint`; submissions enter the queue **subject to
  approvals — the MCP boundary never bypasses them**), W-23 (stdio only),
  W-24 (~10-case read-only eval set in CI).
- Repo state: no MCP surface of any kind. `docs/ARCHITECTURE.md:100` records MCP
  as explicitly deferred.
- Blocked by design on F-007: W-21's guarantee is meaningless until the approval
  ladder it defers to exists. The MCP *client* (W-16..18) is out of scope until
  P4 passes human review.

## F-007 — no risk-class ladder or approval gates (W-11/12/13)

**Status: OPEN** — scheduled P1.

- Spec: five classes `read-only → workspace-write → repo-push → external-write →
  prod-affecting`; everything above `workspace-write` needs explicit approval;
  `prod-affecting` needs typed confirmation; loosening a default is a logged
  event; destructive ops always confirm regardless of config (W-12).
- Repo state: `DispatchPosture` (`src/core/task.rs:46-56`) has three variants —
  `ReadOnly`, `Plan`, `Edits` — which is a *posture* concept, not a risk ladder.
  There are no approval gates on dispatch, no typed confirmation anywhere, and no
  persisted record of a loosened default.
- **Surprise worth recording before implementing:** `ReadOnly` and `Plan` emit
  byte-identical claude argv (`--permission-mode plan`,
  `src/adapters/claude_code.rs:386-396`). The work order proposes mapping
  existing postures onto the bottom two rungs; that mapping is not currently
  observable in behavior, so the mapping is a decision, not a description.

## F-008 — no reviewer role, no family-diversity enforcement (W-09/W-10/W-14)

**Status: OPEN** — scheduled P2.

- Spec: at/above `workspace-write` the reviewer MUST run on a different model
  family than the producer, enforced by config validation (W-09); disagreement →
  `confidence: low`, surfaced in a diff pane, blocked from auto-apply (W-10).
- Repo state: "reviewer" is not a concept in code — roles are free-form
  `BTreeMap` keys and `src/core/plan.rs:11` states outright that "roles are
  launch presets, not enforcement." There is no model-family taxonomy anywhere;
  `Role.model` is passed verbatim to the tool with no translation table
  (`src/core/plan.rs:51-53`). There is no diff pane (the W-14 gap).
- Scope rule from the work order: with codex suspended (ADR-0008), family
  diversity means Anthropic vs Google. If only one family is live at runtime the
  task **BLOCKS with a visible reason** — it must never silently degrade to
  same-family review.

## F-009 — tasks are untyped; no acceptance checks, no context allowlist (W-05/W-08)

**Status: OPEN** — scheduled P3.

- Spec: every task links `consumes` refs; acceptance checks are executable where
  possible and a task is `done` only when they pass or a human overrides **with a
  logged reason**; context assembly is an allowlist builder, never "the whole
  vault." W-05 carries an explicit acceptance test: *session context audit log
  lists only declared refs.*
- Repo state: `Task` (`src/core/task.rs`) carries prompt/cwd/model/effort and a
  `Budget` — no `consumes`, no acceptance checks, no notion of `done` beyond a
  process exit code, and no context audit log.

## F-010 — no run manifest; provenance is not persisted (W-25/W-26/W-31)

**Status: BLOCKED-ON-DECISION** — see `docs/adr/0014-run-manifest-durable-store.md`.

- Spec: W-31 (a run manifest of task, refs, role-config hash and backend versions
  enables re-execution and postmortems), W-25 (`model_used`, backend, duration,
  cost), W-26 (generated changes carry provenance via git trailers).
- Repo state — four gaps, all verified:
  - `model_used` is **on the wire and discarded**. The claude adapter parses only
    `session_id` from the stream-json `init` event
    (`src/adapters/claude_code.rs:460-464`); the committed fixture
    `tests/fixtures/claude_stream.synthetic.ndjson` line 1 already carries
    `"model"`. `SessionRecord.model` stores the *requested* model, which is
    `None` whenever no `--model` was passed — requested ≠ used.
  - Backend version is **probed then thrown away** at all three probe sites
    (e.g. `src/adapters/claude_code.rs:219-226`): `--version` runs, only the exit
    status is read.
  - No hashing of any kind exists in the repo, so there is no role-config hash.
  - `dispatches` rows carry no posture, budget, model, version, or config hash.
- **Why blocked.** The work order's deliverable is "persist … per dispatch row
  (schema v4)" — i.e. SQLite. Spec W-NFR-3 says "logs are plain-text and
  greppable; **the vault, not a database, is the durable store**." That collides
  with accepted ADR-0002 (thin SQLite registry). Per the standing scope rule, a
  conflict between the spec and an accepted ADR is not mine to settle silently:
  ADR-0014 proposes a resolution and this finding stops here for human review.
- **Also needs owner sign-off when it resumes** (AGENTS.md "ask before"): adding
  a `model_used` carrier changes the `AgentEvent` schema; capturing a version
  string changes `AdapterCaps`/the probe contract (`AdapterCaps` is `Copy` with
  `&'static` fields, so a `String` version does not fit as-is); and a
  content-hash needs either a new dependency or a documented non-cryptographic
  hand-rolled hash.
- **W-26 is not implementable as written** — recorded here so the next session
  does not rediscover it. swarm-tui never makes commits; it spawns `claude -p` /
  `agy -p` and reads their output. Attaching a git trailer would mean injecting
  instructions into the user's prompt (mutates user input, unverifiable) or
  rewriting commits the CLI made (history rewriting — outside the boundaries).
  The only boundary-respecting option is to record provenance swarm-tui-side and
  render a ready-to-paste trailer. That is a decision, not an implementation
  detail.

## F-011 — user prompts are persisted verbatim, unredacted (W-29/W-28)

**Status: VERIFIED-CLOSED**

- Spec: W-29 "Run logs redact detected secrets before persistence"; supporting
  W-28 "secrets … never embedded in prompts or logs."
- Repo state — the exposure, as verified:
  - `dispatches.prompt TEXT NOT NULL` (`src/store/mod.rs:47`) stores the full raw
    user prompt. No truncation, no expiry, **and no delete path anywhere in the
    repo**. Written by `Registry::record_dispatch` (`src/store/mod.rs:270-291`),
    whose only production caller is `src/app/mod.rs:1170`.
  - WAL is on (`src/store/mod.rs:87`), so prompt text also lands in
    `registry.db-wal`; `.gitignore` covered `*.db` and `*.db-journal` but not
    `*.db-wal` / `*.db-shm`.
  - The tracing sink (`src/main.rs:71-74`) is an append-only file with **no
    rotation and no size cap** — anything logged there is permanent.
  - No prompt reaches the log today, but only *incidentally*:
    `src/app/mod.rs:1178` logs `{e:?}` from the very INSERT that carries the
    prompt, and it is safe only because the prompt is a bound parameter (`?3`,
    `src/store/mod.rs:279-284`). Nothing enforces that invariant.
  - There was no redaction code in the repo. (`scrub_env`,
    `src/adapters/mod.rs:285-300`, is process-env hygiene — not text redaction.)

### Interpretation recorded before implementing (working rule 7)

Two readings the owner should veto now if they are wrong, rather than after
seeing a diff:

1. **W-29 governs persistence only.** The raw prompt is what the user
   deliberately typed and it must reach the CLI unchanged — redacting the argv
   would break dispatch outright. So the split implemented is:
   **raw → adapter argv/stdin; redacted → SQLite and the log.** A test pins the
   argv half so the guarantee is not silently lost later.
2. **Detection is best-effort, not a guarantee.** Pattern matching cannot catch
   an arbitrary high-entropy string a user pastes. W-29's own wording ("detected
   secrets") concedes this, and this entry does not claim completeness. What the
   change buys is that the well-known credential shapes stop landing in a
   permanent store.

Two deliberate non-actions, flagged for sign-off:

- **Existing rows are left alone.** Prompts already in a user's `dispatches`
  table may contain secrets, but rewriting stored user data inside a migration is
  irreversible and is not a call to make unilaterally. Follow-up decision.
- **No schema change.** F-011 changes what text goes into an existing column, so
  the additive-migration rule does not engage and this finding is fully clear of
  the W-NFR-3 conflict blocking F-010 — it adds no durable store, it cleans two
  that already exist.

### Fix

- **New `src/core/redact.rs`** — a dependency-free single-pass scanner. Two rule
  families: vendor prefixes with a length floor (`sk-ant-`, `sk-`, `AIza`,
  `ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`/`github_pat_`, `xox[abprs]-`), fixed shapes
  (`AKIA`/`ASIA` + 16, three-segment JWTs, `scheme://user:pass@host`, PEM
  `PRIVATE KEY` blocks), and explicit `key=`/`key:` assignment shapes. Every rule
  requires an unambiguous marker rather than guessing from entropy — over-
  redaction destroys a row's postmortem value, so it is treated as a real failure
  mode with its own test. `CERTIFICATE` blocks are deliberately left alone
  (public material). Hand-rolled rather than adding `regex`, which would have
  needed an AGENTS.md dependency sign-off.
- **Chokepoint: `Registry::record_dispatch`** (`src/store/mod.rs`) redacts
  unconditionally before binding `?3`. Redacting at the store boundary rather
  than at the call site means a future caller cannot forget to.
- **Tracing sink:** `src/app/startup.rs` is the only site that logs
  user-authored text; it now routes through the same redaction. Note this is
  *hardening, not a bug fix* — no prompt reached the log before this change. It
  matters because the log file is append-only with no rotation, so any future
  leak would be permanent.
- **`.gitignore`** gained `*.db-wal` / `*.db-shm` — the WAL sidecar holds
  recently-written rows until a checkpoint folds them in.

### Closing evidence (rustc 1.96.1)

Red first — the failure scenario reproduced before the fix existed:

```
$ cargo test redacted_before_persistence
running 1 test
test store::tests::dispatch_prompt_with_api_key_is_redacted_before_persistence ... FAILED

thread 'store::tests::dispatch_prompt_with_api_key_is_redacted_before_persistence'
panicked at src/store/mod.rs:717:9:
secret persisted verbatim: deploy using sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJKKKK then report back

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 158 filtered out
```

Green after:

```
$ cargo test dispatch_prompt_with_api_key_is_redacted_before_persistence
running 1 test
test store::tests::dispatch_prompt_with_api_key_is_redacted_before_persistence ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 166 filtered out; finished in 0.00s
---exit=0---
```

Ten tests added in total (157 → 167). Beyond the red one above they cover the
rule table case-by-case, assignment shapes, PEM blocks, the certificate
exclusion, punctuation/quote affixes, multiple secrets in one prompt, and two
guards: `ordinary_text_round_trips_byte_for_byte` /
`ordinary_dispatch_prompts_are_persisted_unchanged` (over-redaction) and
`dispatch_argv_still_carries_the_raw_prompt` (the raw half of the split — it
fails if anyone later redacts on the way to the CLI). The last three were green
before the change as well; they are regression guards, not reproductions, and are
listed as such rather than counted as red-first evidence.

Full suite:

```
$ cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
test result: ok. 167 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.50s
---chain_exit=0---
```

### Left open, needs owner sign-off

- **Existing rows are not scrubbed.** A `dispatches` table written before this
  change may still hold verbatim secrets. Rewriting stored user data inside a
  migration is irreversible, so it was not done unilaterally. Options when
  decided: a one-shot scrub on open, a `swarm-tui --scrub-history` subcommand, or
  leaving history as-is and documenting it.
- **Coverage is a fixed table.** New vendor key formats need a new rule; there is
  no entropy heuristic and deliberately so.
- **Known gap, named rather than glossed:** `classify()` matches on token
  *prefixes*, so a key glued inside punctuation with no surrounding whitespace —
  a minified `{"model":"sk-ant-…"}` blob, say — is not detected, because the
  token starts with `{"model":"`. Env-var lines, config lines, bare keys, and
  spaced JSON are all caught. Closing this means matching key shapes anywhere
  inside a token, which trades a measurable rise in false positives against a
  narrow gain; it is a deliberate deferral, not an oversight.

## F-012 — undefined

**Status: BLOCKED-ON-INPUT** — no definition available in any input.

Appears only as an undescribed mention in the work order ("F-012/F-013: fold into
the phases they touch"). No description exists in the repo, in git history, or in
any file on this machine. Seeded rather than dropped, per the work order's own
"no silent drops" requirement. Needs one line from the owner saying what it is.

## F-013 — undefined

**Status: BLOCKED-ON-INPUT** — no definition available in any input.

Same as F-012: mentioned, never defined. Awaiting a definition.
