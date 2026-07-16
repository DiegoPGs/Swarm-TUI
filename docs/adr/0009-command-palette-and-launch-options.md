# ADR-0009: Command palette and launch options

- Status: Accepted (2026-07-16)

## Context

Milestone 2b gives swarm-tui a command plane over the two active tools (ADR-0008).
The research base is `docs/integrations/command-surfaces.md` (claude 2.1.211,
agy 1.1.3, observed locally 2026-07-16): both tools expose rich in-TUI slash
commands, and both accept model selection at launch (`--model`), with claude adding
`--effort`. swarm-tui needs (a) a discoverable way to fire a session's native
commands without memorizing each tool's menu, and (b) a way to choose model/effort
when spawning a tab — without ever parsing or reinterpreting tool output
(ADR-0003/session-view rule) and without `app` learning flag names (ADR-0006).

The owner pre-authorized the two adapter-trait changes below in the milestone plan
(satisfying AGENTS.md's "ask before changing the `CliAdapter` trait").

## Decision

Three layers:

**Layer 0 — native passthrough (already true; documented here).** Slash commands
typed inside a pane go straight to the tool; swarm-tui never intercepts, parses, or
rewrites them. The palette below is additive discoverability, not interception.

**Layer 1 — command palette.** Prefix (`Ctrl-Space`) + `:` on a session tab opens a
swarm-tui palette listing that tool's commands from a declarative per-adapter table.
Selecting an entry **injects the command text plus a carriage return** into the pane
— raw bytes through the exact same `PaneHost::write_input` path as keystrokes; the
tool's own UI takes over from there (e.g. `/model` opens claude's native picker).
This is user-initiated at runtime and therefore allowed; the development-session
"never press Enter in a real pane" rule is about unattended agents, not the user.

- v1 injects the **bare command** by default. When the entry declares an
  `args_hint`, the palette offers a free-text argument line first; empty args fall
  back to the bare command.
- Entries whose effect outlives the session (per command-surfaces.md's "persists"
  column) render with a **`[persists]` badge**.
- The table is populated **only from commands verified ✅ locally** in
  command-surfaces.md. 🔶/⬜ rows stay out: an injected command executes the moment
  it lands in the pane, so a stale guess isn't a cosmetic bug — it types a wrong
  command into a live agent session. Deliberate exclusions from the ✅ set:
  claude `/agents` (a "(removed)" stub at 2.1.211) and pure alias rows
  (`/cost` is folded into `/usage`'s description).
- Keymap: `:` joins the ADR-0007 one-shot table (that table is amended by this ADR
  — see below). The palette is session-tab-only; on Home, prefix+`:` is a no-op.

**Layer 2 — launch options.** The new-session picker gains a per-tool options form
(model free text with suggested aliases; effort as a fixed list) driven by a
declaration the adapter publishes — never by `app`-side flag knowledge. Chosen
model/effort are persisted on the session row (registry schema v2) and shown in the
roster. claude maps model/effort to `--model`/`--effort`; agy maps model to
`--model` and has no effort flag (reasoning depth is encoded in its model-variant
names).

**Deferred, verified-but-out:** permission mode has a launch equivalent in *both*
worlds — claude `--permission-mode` (`acceptEdits, auto, bypassPermissions, manual,
dontAsk, plan` at 2.1.211) and agy `--mode` (`accept-edits`, `plan`, new in 1.1.x).
The owner decided (2026-07-16) v1 `LaunchOptions` stays `{ model, effort }`;
permission mode is recorded here as the first candidate for a v2 field.

### The two adapter-boundary changes

```rust
// src/adapters/mod.rs — alongside LaunchIntent
pub struct NativeCommand {
    pub name: &'static str,          // as the tool's menu shows it, e.g. "/model"
    pub inject: &'static str,        // exact text typed into the pane (usually == name)
    pub description: &'static str,
    pub args_hint: Option<&'static str>,
    pub persists: bool,              // effect outlives the session (writes tool state)
}

pub struct LaunchOptions { pub model: Option<String>, pub effort: Option<String> } // + Default

pub trait CliAdapter {
    // changed: options threaded through; adapters map what they support and
    // silently ignore the rest.
    fn interactive_cmd(&self, intent: &LaunchIntent, opts: &LaunchOptions, cwd: &Path) -> Command;
    // new: declarative command table; default empty so a suspended or
    // minimum-viable adapter needs no override.
    fn command_table(&self) -> &'static [NativeCommand] { &[] }
}
```

`name`/`inject` are separate on purpose: if stage-2's autocomplete-interference
risk materializes (a trailing `\r` selecting a highlighted menu entry instead of
submitting typed text), `inject` can carry a disambiguating suffix (e.g. a trailing
space) without renaming the entry. Not needed at 2.1.211/1.1.3 as observed.

`impl CliAdapter for AdapterKind` adds an explicit three-arm `command_table`
dispatch. This is load-bearing: without it, enum-dispatched calls would silently
hit the default `&[]` — pinned by the `claude_and_agy_command_tables_populated`
test.

Options are appended **uniformly for every `LaunchIntent`** (`--model`/`--effort`
are session-scoped flags, meaningful on resume too). Only `Fresh` is reachable from
the app today; resume-with-options is syntactically valid but not live-verified.

### Launch-option declaration rides on `AdapterCaps`

```rust
pub struct LaunchOptionsDecl {
    pub model: Option<&'static [&'static str]>,   // Some = flag exists; slice = alias suggestions (free text allowed)
    pub effort: Option<&'static [&'static str]>,  // Some = flag exists; slice = the fixed level list
}
pub struct AdapterCaps { /* existing fields */ pub launch: LaunchOptionsDecl }
```

`AdapterCaps` is already sanctioned `app` vocabulary (AGENTS.md), and every adapter
already builds it in `probe()` — so the picker learns which fields to render from
`probe_cache`, with **no third trait method**. Each adapter sets a field only when
the flag actually appears in the installed binary's `--help` (`model:
has("--model").then_some(…)`), so upstream flag drift degrades to "field hidden in
the picker", never a broken spawn. "Model is free text with suggestions, effort is
a fixed list" is a UI rule keyed by field, not a type distinction.

### Vocabulary rule amendment

`core`/`app` may speak, beyond `AgentEvent`/`SessionRecord`/`AdapterCaps`: the
data-only launch/command vocabulary `LaunchIntent` (already in practice since
milestone 2a), `LaunchOptions`, and `NativeCommand`. AGENTS.md's conventions line
is updated accordingly. Flag names, injection quirks, and per-tool semantics stay
inside `src/adapters/`.

### ADR-0007 amendment

The one-shot keymap gains one row — `:` → command palette (session tab only) — and
`Ctrl-Space` remains the only reserved key. ADR-0007's table is annotated in place
with a pointer here rather than superseded: the *decision* of ADR-0007 (single
prefix, one-shot mode, double-press passthrough) is unchanged; this ADR only
extends its command table, and duplicating the whole keymap into a new ADR would
fork the source of truth.

## Alternatives rejected

- **Intercepting/parsing native command output** (e.g. running `/model` and
  scraping the picker): violates the session-view fidelity rule and couples
  swarm-tui to Ink/agy render details. Injection + native UI is strictly simpler.
- **A third trait method for launch declarations** (`launch_options(&self)`):
  works, but duplicates a channel `AdapterCaps` already provides and grows the
  trait for pure data. Rejected in favor of the caps field.
- **Free-form command entry in the palette** (type anything, inject it): already
  covered by Layer 0 — the user can just type in the pane. The palette's value is
  the *curated, verified* table with persistence badges; arbitrary entry would
  reintroduce the stale-guess problem the ✅-only rule exists to prevent.
- **Persisting effort/model as swarm-tui-side defaults** (auto-applying to every
  new session): deferred — the tools have their own sticky defaults (`/model`,
  `/effort`), and double-defaulting invites surprising precedence bugs.

## Consequences

- `interactive_cmd`'s signature change is breaking for all three adapters and the
  one app call site; codex's adapter compiles with `launch: NONE` and the default
  empty table but is unreachable (ADR-0008).
- The palette needs the session's tool slug → `AdapterKind` mapping;
  `AdapterKind::from_slug` gains its first caller.
- Registry schema v2 (model/effort columns) is required so the roster can show
  what a session was launched with; see the milestone's store change.
- The command tables are snapshots of verified versions (2.1.211 / 1.1.3) and will
  drift; `persists_flags_match_command_surfaces_doc` keeps the code table honest
  against the research doc, and the doc records observed versions for re-checks.
- Revisit when: injection interference is observed live (switch `inject` to the
  trailing-space form), a tool ships a programmatic command-listing surface (table
  could become probe-derived), or permission mode is promoted into `LaunchOptions`.
