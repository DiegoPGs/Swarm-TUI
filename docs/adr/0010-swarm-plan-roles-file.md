# ADR-0010: The swarm plan — a workspace roles file

- Status: Accepted (2026-07-16)

## Context

Milestone 2b gave every new session launch options (model/effort) chosen by
hand in the picker, each time. Teams working in a repo tend to re-create the
same few session shapes — "the researcher on agy", "the implementer on opus,
high effort", "the advisor tab" — and re-typing them is both friction and
drift. swarm-tui needs a way for a workspace to *declare* those shapes once,
shareably, without adding orchestration (headless dispatch, broadcast,
pipelines, and auto-scheduling stay out of scope; codex stays suspended per
ADR-0008).

## Decision

A workspace MAY commit a roles file at **`<launch cwd>/.swarm/swarm.json`**.
It is plain data consumed by the new-session picker; roles are **launch
presets, not enforcement** — nothing stops the user changing model or effort
mid-session; swarm-tui records what was *requested* (the registry `role`
column is provenance, not a contract).

### Schema v1

```json
{
  "version": 1,
  "roles": {
    "coder": {
      "tool": "claude-code",
      "model": "opus-4.8",
      "effort": "high",
      "purpose": "implementation",
      "startup_commands": ["/advisor fable"]
    }
  }
}
```

- `tool` (required): must be an **active** adapter slug. A suspended slug
  (`"codex"`, ADR-0008) or an unknown one is a load-time error that names the
  valid slugs — the two cases produce distinct messages so a suspended tool
  reads as "suspended", not "typo".
- `model`, `effort` (optional): passed **verbatim** to the adapter's launch
  flags. **Non-goal: no translation table in swarm-tui.** An invalid model id
  surfaces as the tool's own error inside the pane — the file author's
  feedback loop, not swarm-tui's. Adapters that don't declare an option ignore
  it (agy ignores `effort`, as with the 2b picker).
- `purpose` (optional): shown in the picker role line (and nowhere else — the
  spec's "roster tooltip" has no terminal equivalent; the roster shows the
  role *name* column, the session status line appends `· role <name>`).
- `startup_commands` (optional): each MUST start with `/`; injected in
  declared order after the pane's first stable paint (below).
- Parsing is strict serde with `deny_unknown_fields`; unknown versions are
  rejected. **Nothing in the schema accepts credentials, tokens, or paths to
  them — the file must never contain secrets, and there is no field where one
  could legally live.**

### Loading

Loaded at startup from the launch cwd and reloaded by the existing refresh key
(prefix+`r`), which already re-reads the roster. Missing file ⇒ the picker's
roles section is simply absent. Malformed file ⇒ a one-line error in the
picker (`.swarm/swarm.json:LINE:COL — message` when serde provides a
position), and the raw-tool list keeps working — a broken plan never crashes
or blocks the app. Validation errors (unknown/suspended tool, bad startup
command, wrong version) surface the same way.

`core/plan.rs` owns types and loading but receives the active/known slug lists
as parameters — `core` must not import `adapters` (ADR-0006 dependency
direction). `adapters` gains `all_kinds()` beside `registry()` so the app can
distinguish "suspended" from "unknown" without the string `"codex"` ever
appearing outside `src/adapters/`.

### Startup-command injection

Selecting a role spawns through the existing `LaunchOptions` path (the role
*is* the preset — the options form is skipped), then a `StartupQueue` drives
the declared commands:

1. **Wait for first paint**: screen contents non-blank and identical across
   two consecutive ~400 ms polls (the `slash_probe`/`shell_smoke` stability
   pattern), capped at 15 s.
2. **Two-step injection** per command: write the command *text* only; poll up
   to ~2 s for it to echo in the pane's screen; only then write the carriage
   return. If the text never echoes, skip **all** remaining commands and note
   it. Rationale: a first paint can be a modal that swallows characters —
   agy's workspace-trust dialog demonstrably does (NOTES.md, 2b Stage 2) — and
   a blind `text+\r` write would press Enter *on the modal*. The guard's
   failure mode is "skip", never "blind Enter".
3. Commands go in declared order, one per poll tick. A command whose first
   token matches a `command_table()` entry with `persists: true` pauses the
   queue for a **confirmation modal** (role name + command + a "persists
   beyond this session" warning); `y` injects, `n`/Esc skips that entry and
   the queue continues. The modal reuses the existing `pending_confirm`
   mechanism and can therefore pop while the user is typing into the pane —
   accepted: it is exactly as prompt-stealing as the close/quit confirms.
4. Pane exit or the 15 s cap drops the remaining commands, logs a warning, and
   appends `· startup commands skipped` to that session's status line.

**Correction recorded**: the milestone brief referred to "the ADR-0009
confirm" for persists-flagged entries. ADR-0009 in fact shipped a *badge only*
— the palette injects `[persists]` entries immediately, with no confirmation.
The confirm introduced here is **new** and applies to **startup injection
only** (the unattended, from-a-file path). The palette's attended behavior is
unchanged; retrofitting it for parity is a deliberate non-goal of this ADR and
a one-line follow-up if the owner wants it.

### The committed example

This repo dogfoods the feature with a checked-in `.swarm/swarm.json`:
`researcher` (antigravity, `gemini-3.1-pro`, "web search & docs"), `coder`
(claude-code, `opus-4.8`, effort `high`, "implementation"), `advisor`
(claude-code, `sonnet-5`, "general advisor", startup `/advisor fable`).
Advisor semantics: the *main* model is sonnet-5, consulting Fable 5 through
claude's own `/advisor` command; if the owner ever wants Fable as the main
model instead, that is a one-line JSON edit. Note the verbatim-model non-goal
above applies to these strings too (agy's accepted `--model` format is still
⬜).

## Alternatives rejected

- **TOML/YAML** — a new dependency for format taste; serde_json is already in
  the tree and JSON is fine for a ~20-line file.
- **Roles stored in the registry DB** — the whole point is a *shareable,
  committed* declaration; a per-machine SQLite row is the opposite. The
  registry only records which role a session was launched from.
- **Per-tool role files or adapter-owned schemas** — roles are cross-tool
  vocabulary (ADR-0009's data-only launch language); one file, tool named by
  slug.
- **Validating slugs inside `core` by importing `adapters`** — breaks the
  ADR-0006 dependency direction for one convenience; slug lists passed as
  parameters cost three lines.
- **Editable role form on selection** (picker opens the options form
  pre-filled) — contradicts "launch presets", doubles the picker state
  machine; editing is what the raw-tool path is for.

## Consequences

- Registry schema **v3**: `sessions` gains `role TEXT` (nullable, last
  column); `SessionRecord.role: Option<String>`; roster gains a Role column
  (reconciled-only rows show `-`). v1 databases migrate through a v1→v2→v3
  chain; each step keeps its own transaction so an interruption leaves a
  valid, re-openable intermediate.
- `adapters::all_kinds()` joins `registry()` as the second place a reinstated
  codex must touch — the adapters test pinning its contents makes reversal
  compiler-guided (ADR-0008's one-const promise becomes one-const-plus-one-
  slice, guarded).
- The picker grows a roles section above the tools; `examples/shell_smoke.rs`
  selects picker items by position and must navigate past the committed roles.
- Every dev run of swarm-tui *in this repo* now shows the dogfood roles, and
  selecting one injects into a real CLI — user-initiated product behavior, but
  worth knowing before pressing Enter in the picker.
