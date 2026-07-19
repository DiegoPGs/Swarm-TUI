# "The Worker" — TUI Orchestrator Requirements

```
schema: requirements@1
status: assumption
source: agent
model_used: claude-fable-5
confidence: medium
volatility: high            # depends on third-party CLI behavior and provider terms
last_verified: 2026-07-18
consumes: [guidelines/anti-sycophancy.md, 00-workflow-requirements.md]
```

## 1. Purpose

A terminal orchestrator that decomposes a G2-gated spec into tasks and delegates them to specialized sub-agents running on the founder's existing coding-agent CLIs (Claude Code, Codex, Antigravity), with cross-model review, human approval points, full provenance/cost logging into the vault, and MCP integration in both directions (consuming Jira/cloud servers; exposing task intake). The worker is the Engineering module's engine and the executor of the Launch & Operate closed loop (OP-01).

## 2. Non-goals

- **NG-1.** Not an IDE and not a model host: it wraps existing CLIs; it never re-implements them.
- **NG-2.** Not autonomous by default: risk-classed actions stop at approval points (W-11); "fully hands-off" is an explicit per-task opt-in, never a global default.
- **NG-3.** Not a credential broker: it never stores or proxies raw provider credentials (W-03).
- **NG-4.** Not a second vault: all durable knowledge lives in the vault; the worker holds only run state.

## 3. Core concepts

- **Backend** — an adapter around one CLI (Claude Code, Codex, Antigravity), exposing a common interface.
- **Role** — a configured sub-agent specialization (implementer, reviewer, test-writer, infra, docs): system prompt + tool policy + preferred backend + risk ceiling. Roles are config, not code.
- **Task** — a typed artifact: goal, `consumes` refs, constraints, acceptance checks, risk class, linked assumption/metric (G3 hook).
- **Session** — one backend process bound to a task, a workspace, and a run log.
- **Run log** — append-only record per session: prompts sent, actions taken, `model_used`, cost/quota units, outcomes.

## 4. Functional requirements

### 4.1 Backend abstraction & auth

- **W-01.** Common backend interface: `spawn(session)`, `send(task_step)`, `stream(output)`, `interrupt()`, `collect(artifacts)`, `capabilities()`. Backends are plugins; adding a CLI touches no core code (→ §1.2 "model-agnostic invariant makes it a config change").
- **W-02.** Adapters are versioned against the CLI version they target, with contract tests per backend; a CLI update that breaks the contract degrades that backend to `unavailable` with a clear message instead of corrupting sessions.
- **W-03.** Auth delegates to each CLI's own login/subscription mechanism. The worker checks auth state and reports it; it never reads, stores, or forwards provider credentials.
- **W-04.** Backend unavailability (logout, rate limit, quota exhaustion) is a first-class state: affected tasks queue or reroute per policy (W-07); nothing silently retries into a wall.

### 4.2 Task & role model

- **W-05.** Every task links its vault refs (`consumes`) and, where applicable, the assumption/metric it serves (PD-03/G3). Context assembly for a session is the allowlist builder from SYS-02 — the worker never hands a backend "the whole vault." *Acceptance test: session context audit log lists only declared refs.*
- **W-06.** Roles are declared in config: prompt template, allowed tools, workspace scope, risk ceiling, preferred backend + fallback order. Shipping defaults: implementer, reviewer, test-writer, infra, docs.
- **W-07.** Routing policy maps task type → role → backend, with per-task override in the TUI. Routing decisions are logged with reasons.
- **W-08.** Acceptance checks are executable where possible (tests, lint, build); a task is `done` only when its checks pass or a human overrides with a logged reason.

### 4.3 Cross-model review (→ §1.2)

- **W-09.** For any task at or above risk class `workspace-write`, the reviewer role MUST run on a **different model family** than the producer. Config validation enforces it; violations raise the L7-style warning in the run log.
- **W-10.** Producer/reviewer disagreement → the change is flagged `confidence: low`, surfaced in the TUI diff pane, and blocked from auto-apply pending human decision. Disagreement is treated as a cheap low-confidence detector, not noise (→ §1.2).

### 4.4 Approval points & risk classes

- **W-11.** Risk classes: `read-only` → `workspace-write` → `repo-push` → `external-write` (tickets, cloud config) → `prod-affecting`. Defaults: everything above `workspace-write` requires explicit TUI approval; `prod-affecting` additionally requires typed confirmation. Defaults are config, loosening one is a logged event.
- **W-12.** Destructive or irreversible operations (force-push, resource deletion, data migration) always confirm, regardless of risk-class config.
- **W-13.** Approvals show: the diff/action, the task's linked assumption, the reviewer verdict (W-10), and cost so far — enough context to decide without leaving the TUI.

### 4.5 TUI

- **W-14.** Dashboard: parallel sessions with status, role, backend, cost/quota meters; task queue; log tail per session; diff review pane; keyboard-driven throughout.
- **W-15.** Every state change is visible and auditable: the TUI is a view over the run logs, never a second source of truth.

### 4.6 MCP client (Jira, cloud providers, other modules)

- **W-16 — Untrusted content.** Content fetched from external systems (ticket bodies, alert payloads, cloud metadata) is **data, never instructions**. Any action derived from external content passes the W-11 approval policy regardless of what the content requests; instruction-like content in a ticket is flagged in the run log. This is the prompt-injection posture for the OP-01 loop.
- **W-17.** Per-server tool allowlists: each connected MCP server gets an explicit allowed-tool set and a risk-class mapping (e.g., `jira.create_comment` = external-write). Unknown tools are unusable until classified.
- **W-18.** Cloud provider connections (AWS / Azure / GCP MCP servers) default to read-only credentials (CM-04); write operations exist as separately-credentialed, approval-gated actions.

### 4.7 MCP server (exposing the worker)

- **W-19.** The worker exposes a small MCP server so other modules can submit work: `worker_submit_task`, `worker_get_task_status`, `worker_list_tasks`, `worker_cancel_task`. Consistent `worker_` prefix, action-oriented names.
- **W-20.** Tools declare input/output schemas (Zod/Pydantic-style validation) with constraints and examples in field descriptions; responses include structured content, and list operations paginate.
- **W-21.** Tool annotations are accurate: `readOnlyHint` on status/list tools, `destructiveHint` on cancel; submissions are not auto-executed — they enter the queue subject to W-11 approvals (the MCP boundary never bypasses the approval policy).
- **W-22.** Errors are actionable: what failed, why, and the next step (e.g., "backend claude-code unavailable: not logged in — run `claude login`").
- **W-23.** Transport: stdio for local module-to-module use; if a remote transport is ever added, it is a separate, explicitly secured deployment decision — not a default.
- **W-24.** An evaluation set (≈10 realistic read-only tasks against the MCP surface) ships with the server and runs in CI, per MCP best-practice guidance.

### 4.8 Vault integration, telemetry, provenance

- **W-25.** Every session writes a run log with `model_used`, backend, duration, cost/quota units, artifacts touched (SYS-08). Where a provider exposes no quota API, usage is estimated and **labeled as an estimate** — no fabricated precision (CM-02).
- **W-26.** Generated code and infra changes carry provenance: which task, which refs, which model, which reviewer. `git` trailers or commit metadata link back to the task artifact.
- **W-27.** G3 hook: a "deploy prototype" task template includes the metric-instrumentation checklist from the G2 spec; G3 evaluation reads the run log to confirm instrumentation shipped (EN gate interface).

### 4.9 Sandboxing, security, reliability

- **W-28.** Sessions run in project-scoped working directories; file and network access per role policy; secrets injected via environment/secret manager, never embedded in prompts or logs.
- **W-29.** Run logs redact detected secrets before persistence.
- **W-30.** Crash/rate-limit recovery: sessions are resumable from the run log; partial work is never silently lost or silently retried into external-write actions.
- **W-31.** Reproducibility: a run manifest (task, refs, role config hash, backend versions) allows re-execution and postmortems.

## 5. Non-functional requirements

- **W-NFR-1.** TUI interaction latency stays responsive while sessions stream (streaming is async; the UI never blocks on a backend).
- **W-NFR-2.** Platforms: macOS and Linux at minimum (`TBD` whether Windows is in scope — depends on backend CLI support, verify per CLI).
- **W-NFR-3.** Logs are plain-text and greppable; the vault, not a database, is the durable store.
- **W-NFR-4.** Offline tolerance: queueing and log review work without network; only backend execution requires it.

## 6. Risks & open questions

1. **Provider terms (blocking legal task, OD-02a):** whether programmatic orchestration of subscription-authenticated CLIs is permitted differs per provider and changes over time. **This document does not assert it is or isn't permitted** — it is unresolved, `volatility: high`, and must be verified per provider before orchestration-via-subscription becomes a distributed product promise (vs. a personal tool). Fallback design: W-01's abstraction must also accommodate API-key backends so the answer changes config, not architecture.
2. **CLI interface churn:** all three backends evolve quickly; W-02 contract tests are the mitigation, but adapter maintenance is a permanent cost — feed it into CM roll-ups so the true cost of multi-backend support is visible.
3. **Quota opacity:** subscription quotas are often not queryable; estimates (W-25) may drift badly. Measure drift where ground truth appears.
4. **Correlated failure:** if routing policy sends producer and reviewer to the same family for some task types, §1.2's detector silently disappears; config validation (W-09) covers review, but routing defaults should also prefer family diversity.
5. **Single-tool counterfactual:** the orchestrator must beat "just use one good CLI directly" — see disconfirming case.

## Disconfirming case

- **Strongest reason this conclusion is wrong:** multi-backend orchestration may add fragility (adapters, quota juggling, approval friction) without measurably better output than a single well-configured CLI, in which case the worker's real value is only the vault/provenance/approval layer — which could wrap one backend far more simply.
- **Evidence that would change it:** run-log comparisons on real tasks — cross-model review catch rate vs. same-model review, task completion time and cost per backend mix, adapter-maintenance hours from CM roll-ups.
- **Where that evidence would be found:** the worker's own run logs and MV-02 meta-metrics after dogfooding on the venture's first prototypes.
