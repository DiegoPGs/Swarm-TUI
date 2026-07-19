# ADR-0014: Where the run manifest lives — SQLite registry vs plain-text log

- Status: **Proposed** (2026-07-18) — needs an owner decision. Finding F-011's
  sibling F-010 is **BLOCKED-ON-DECISION** until this is accepted or rejected.

## Context

Finding F-010 (`findings-ledger.md`, tranche 2) asks for a **run manifest**:
per-dispatch provenance sufficient to re-execute and to run a postmortem —
`model_used`, backend CLI version, role-config hash, and the budget/posture the
run actually used. Today none of it is captured: `model_used` arrives in claude's
stream-json `init` event and is discarded (`src/adapters/claude_code.rs:460-464`),
`--version` is run at probe time and only its exit status is read, no hashing
exists anywhere in the crate, and the `dispatches` row carries neither posture nor
budget.

Building it requires deciding **where the manifest is durable**, and there the
requirements document and an accepted ADR disagree:

- **Spec W-NFR-3:** "Logs are plain-text and greppable; **the vault, not a
  database, is the durable store**."
- **ADR-0002 (Accepted):** the registry is SQLite — a thin mapping of
  swarm-tui session id ↔ native session id plus dispatch history.

The work order that opened this finding pre-supposed SQLite ("persist … per
dispatch row, schema v4"), which would settle the conflict silently in favor of
the ADR without anyone deciding. That is the thing this ADR exists to prevent.

Two facts about provenance shape the recommendation:

1. `docs/requirements/20-worker-tui-requirements.md` declares its own status as
   `assumption`, `source: agent`, `confidence: medium`. It is an AI-drafted
   proposal, not a ratified spec. **A human-accepted ADR outranks it.**
2. The same document's **NG-4** says the worker "is not a second vault: all
   durable knowledge lives in the vault; the worker holds only **run state**."

## Decision (proposed)

The conflict is narrower than the two sentences suggest, and NG-4 is the key that
opens it. W-NFR-3's "durable store" is about **knowledge** — the thing a vault
holds. ADR-0002's registry holds **run state** — an id mapping that explicitly
does not own transcripts. NG-4 says run state is exactly what the worker is
*supposed* to hold locally. Read together, the two requirements are compatible;
only the word "database" collides, and it collides with a claim the spec itself
does not otherwise make.

Proposed three-part resolution:

1. **SQLite stays the store for run state. ADR-0002 is unchanged, not
   superseded.** The `dispatches` row remains the operational record, and F-010's
   structured fields land there via the established additive migration chain
   (v3 → v4).
2. **W-NFR-3's plain-text/greppable requirement is met by an additional
   append-only JSONL run log** — one object per dispatch, written next to
   `swarm-tui.log` in the XDG data dir, carrying the same manifest fields. JSONL
   is greppable line-by-line with `grep`/`jq`, satisfies "plain-text," and is
   trivially tailable. It is a **projection of the registry, never a second
   source of truth** (ARCHITECTURE's "the TUI is a view over the run logs, never
   a second source of truth" — W-15 — points the same way).
3. **"The vault" is out of scope for this repository.** swarm-tui has no vault
   and no vault client; the spec's vault is the parent system's concern. If a
   vault integration is ever wanted, it consumes the JSONL log — it does not
   replace the registry.

Everything F-011 already redacts stays redacted on the way into the JSONL log:
the run log is a persistence boundary like any other, so it goes through
`core::redact` (spec W-29). **F-011 must merge before this is implemented** —
which it has.

## Alternatives rejected

- **Do what the work order said and just add schema v4.** Settles a spec-vs-ADR
  conflict by not noticing it. Even though the outcome for part 1 is the same,
  arriving there without a recorded decision means the next reader cannot tell
  whether W-NFR-3 was considered and set aside or simply missed.
- **Follow W-NFR-3 literally: move durable run state out of SQLite into flat
  files.** This supersedes ADR-0002 to satisfy a document that labels itself an
  assumption, and it discards concurrency behavior the registry already relies on
  — `Registry::create` depends on SQLite's write lock to stop two instances
  clobbering session ids (that is finding F-001, closed). Flat files would
  reintroduce the class of bug F-001 fixed.
- **JSONL only, with the registry as a cache.** Same regression as above with
  extra machinery, and it makes crash recovery worse: the registry is what
  reconciliation reads at startup.
- **Postpone the manifest until a vault exists.** Leaves W-25/W-31 unaddressed
  indefinitely for a dependency this repo does not own and may never acquire.

## Consequences

- F-010 stays BLOCKED-ON-DECISION until an owner accepts or rejects this. On
  acceptance it also needs three AGENTS.md "ask before" sign-offs that this ADR
  does **not** grant: adding a `model_used` carrier changes the `AgentEvent`
  schema; capturing a version string changes `AdapterCaps` / the probe contract
  (`AdapterCaps` is `Copy` over `&'static` fields, so a `String` does not fit as
  it stands); and a role-config hash needs either a new dependency or a
  documented non-cryptographic hand-rolled hash.
- W-26 (git-trailer provenance) is **not** resolved here and is not
  implementable as written: swarm-tui never makes commits — it spawns `claude -p`
  / `agy -p` and reads their output. Attaching a trailer would mean injecting
  instructions into the user's prompt or rewriting commits the wrapped CLI made.
  The boundary-respecting option is to record provenance swarm-tui-side and
  render a ready-to-paste trailer block. That deserves its own decision.
- If accepted, `docs/ARCHITECTURE.md` gains the JSONL run log beside the registry
  in the component table, and ADR-0002 gains a pointer here (no supersede).
- Revisit when: a vault client actually exists in this repo, or the run log grows
  past what an unrotated append-only file should hold (note `swarm-tui.log`
  already has no rotation and no size cap — worth fixing at the same time).
