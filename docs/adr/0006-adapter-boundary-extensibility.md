# ADR-0006: Adapter boundary — trait + capability struct + compile-time registry

- Status: Accepted (2026-07-04)

## Context

A fourth CLI will show up (candidates already visible in the ecosystem: Cursor CLI,
OpenCode, Droid, Aider). Adding it must not touch the existing three adapters, the
shell, or the core. Also, the three current tools are *unequal*: structured output
exists for two, resume-by-ID for all three but with different scoping, a background
supervisor for one. The boundary has to express inequality without leaking tool names
upward.

## Decision

- One trait, **`CliAdapter`**, in `src/adapters/mod.rs`: identity (`id`, `display_name`,
  `binary`), `probe()` → `AdapterCaps`, `interactive_cmd(intent)` → command spec for
  the PTY host, and `dispatch(task)` / `follow_up(record, task)` → normalized event
  streams. Nothing else in the codebase may name a CLI.
- **`AdapterCaps`** is data, not behavior: booleans and small enums
  (`structured_output: None|Json|StreamJson`, `resume: ById|ContinueOnly|None`,
  `background_supervisor: bool`, …). The router and UI branch on caps, never on
  adapter identity.
- **Compile-time registry, enum dispatch.** `AdapterKind` enumerates built-in adapters;
  one `registry()` function returns them. Enum dispatch (rather than
  `Box<dyn CliAdapter>`) sidesteps async-fn-in-trait object-safety friction and keeps
  call sites monomorphized. Adding CLI #4 = one new module + one enum variant + one
  registry line; the compiler enforces exhaustiveness everywhere caps are matched.
- **Minimum viable adapter is interactive-only**: a binary name, an
  `interactive_cmd`, and a probe. Everything else is opt-in via caps. This is the
  property that makes "wrap it now, integrate it properly later" a same-day job.

## Alternatives rejected

- **`dyn` trait objects + runtime plugin registration.** Buys nothing today (adapters
  ship in-tree) and costs either `async_trait` boxing on every call or a hand-rolled
  poll API. Config-driven *enable/disable* of built-ins covers the real runtime need.
- **Dynamic loading (dylib/WASM plugins).** The honest extensibility endgame for
  third-party adapters, and the wrong first move: ABI/versioning burden for a tool
  with one user; the trait isn't stable yet. Revisit if outsiders ever want to add
  adapters without forking.
- **Config-file adapters (declare a CLI in TOML: binary + flags + regexes).** Seductive
  — a fourth CLI without code — but the three existing integrations already show the
  hard parts are *behavioral* (event normalization, ID backfill, cwd scoping), which
  templated config can't express. A TOML layer would become a worse programming
  language.

## Consequences

- `AgentEvent`, `AdapterCaps`, and `CliAdapter` are the repo's contract surfaces —
  changes to them require an ADR note and a pass over every `match` on caps.
- The capability probe (ADR-0001/ARCHITECTURE) doubles as the drift alarm when a tool
  ships a breaking flag change.
- Revisit when: someone outside this repo wants to ship an adapter, or a fifth/sixth
  CLI makes the enum feel like friction.
