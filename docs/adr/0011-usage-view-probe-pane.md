# ADR-0011: Plan-usage visibility — the probe pane

- Status: Accepted (2026-07-16)

## Context

Milestone 2c wants a Home-side view answering "how much of each vendor's plan
is left?" without swarm-tui becoming a quota accountant. The Stage 1 research
(`docs/integrations/command-surfaces.md` → "Usage surfaces", 2026-07-16)
settled the mechanism question: **neither active vendor exposes a
machine-readable or CLI-level usage surface** — `claude --help` and
`agy --help` list nothing quota-shaped, `agy models` prints display names only.
Usage exists solely as in-TUI pages: claude's Settings→Usage tab (plan windows
as "NN% used" + reset times) and agy's Models & Quota page (per-group bars as
"NN.NN%" of quota **available** — semantics *inverted* relative to claude).
The wrapped CLIs' own screens are therefore the only truthful source, and any
parsing/normalizing layer would drift the moment a vendor reworded a line.

## Decision

### Mechanism: a short-lived, hidden probe pane (both vendors)

On explicit user request — never automatically — swarm-tui spawns the vendor's
CLI in a hidden PTY pane, injects its usage command, waits for the screen to
stabilize, snapshots the vt100 grid, kills the pane, and renders the captured
lines **verbatim**: the tool's own words, zero parsing drift. A best-effort
one-line **headline** (the first line containing a digit-run immediately
followed by `%`, trimmed) is lifted above the capture and **degrades to
nothing, silently** on garbage — it is a convenience copy, not an
interpretation. Given the used-vs-available inversion between the two vendors,
verbatim-first is a correctness stance, not a shortcut.

Probe details:

- The injected command is the vendor's `command_table()` entry named `/usage`
  (its `inject` bytes) — `/usage` is app-level *concept* vocabulary (the
  cross-tool concept map row "Cost / usage visibility"), the bytes stay
  adapter-declared. A vendor without such an entry gets no refresh key and a
  "no usage command" block. Runtime probes send **`/usage` only**; agy's
  `/credits` stays a research-documented surface a future refresh could add.
- Spawn shape: `interactive_cmd(Fresh { session_id_hint: None }, defaults,
  launch cwd)` at a fixed 44×140 grid (the research capture geometry).
- The pane is **instrumentation, not a session**: it gets no tab, no
  `pane_of_session` entry, and **no registry row**. It is invisible
  everywhere, exempt from the resize loop, killed after capture, and also
  killed by both quit paths.
- Injection uses the same **two-step echo guard** as ADR-0010 startup
  commands (type, verify echo, then Enter); if the text never echoes — e.g. an
  untrusted-workspace dialog is swallowing characters — the probe aborts,
  kills the pane, and the block shows a generic "probe failed — is the
  workspace trusted?". No blind Enter, ever.
- Stability waits: boot capped at 15 s, post-Enter output at 10 s. An output
  timeout still captures the last screen (the verbatim view is the product; a
  spinner beats nothing) with a warning logged.
- One probe per vendor in flight; refresh keys no-op while one runs.

**Accepted costs**: a claude probe creates a real (unregistered) native
session on the vendor side — `claude agents` lists only *background* sessions,
so reconciliation rows are unaffected; an agy probe spends a request against
quota that is shared with the desktop app. Both costs are why refresh is
**manual only**.

### Authorization boundary

Runtime probe injection is **user-initiated product behavior** (the user
pressed refresh; the commands are the vendors' own read-only display
commands). This is distinct from the *development-session* rule, where agents
may never submit into a real CLI beyond an explicitly owner-granted whitelist
— for milestone 2c that whitelist was exactly `/usage`+`/status` (claude) and
`/usage`+`/credits` (agy), used once per tool and recorded in NOTES.md.

### Placement and refresh UX

Prefix+**`u`** toggles a full-body **Resources** view under the Home tab (a
sibling of the roster render, not an overlay — Home's body was the only
uncontended space, and a split panel would starve both halves at typical
terminal sizes). Per active vendor (registry() order), one block:

- role assignments from the swarm plan (`coder → claude-code · opus-4.8/high`),
- the headline (if any) and the last capture verbatim,
- a timestamp, and the refresh key: digit `1..N` refreshes vendor N (digits
  stay vendor-agnostic; vendor-initial letters would leak CLI names into the
  keymap).

Esc or `u` returns to the roster; `j`/`k`/arrows scroll. Refresh is **manual
only** and captures are timestamped **relative** — `as of 3m ago`, the
roster's Age format. (Deviation, owner-approved: the brief's "as of HH:MM"
needs local-timezone access that std Rust lacks; a chrono/time dependency was
declined. Relative age also *is* the honest-staleness signal the timestamp
exists for.) The ADR-0007 keymap table gains the `u` row via the same
in-place-amendment pattern ADR-0009 used for `:`.

## Alternatives rejected

- **Parse captures into structured fields** — guaranteed drift against two
  vendors' unversioned TUI text, with inverted semantics between them; the
  tools' own words cannot lie about themselves.
- **Auto-refresh (interval or on-view)** — spends shared quota without
  consent and injects into real CLIs on a timer; manual refresh keeps every
  injection a deliberate user act.
- **A persistent hidden pane per vendor** (keep the CLI running, re-query) —
  holds a session and its context alive forever to save a 2-second spawn;
  worse quota citizenship, and a crashed hidden pane is invisible state.
- **Rendering usage inside the roster** — the roster is per-session; usage is
  per-vendor; a split panel starves both at 80×24.
- **Registry rows for probe panes** — they are not sessions; recording them
  would pollute the roster and the reconcile logic they exist to avoid.

## Consequences

- New `src/app/usage.rs` (capture state, headline scan, probe state machine,
  Resources rendering); App carries per-vendor capture + in-flight maps.
- Both quit paths must kill in-flight probe panes.
- The keymap gains `u` in five synchronized places (command handler, keymap
  overlay, AWAITING COMMAND banner, README, ADR-0007 amendment row).
- Headline extraction is tested against the committed **synthetic** fixtures
  only; real captures never enter the tree (they carry account email, tier,
  timezone, live percentages).
- If a vendor ever ships a machine-readable usage surface, that vendor's block
  switches to it and this ADR's probe remains for the other — the view is
  per-vendor mechanism-agnostic by construction.
