# ADR-0004: "Combining subagents" means peer-agent composition

- Status: Accepted (2026-07-04)

## Context

Each CLI ships its own internal subagent machinery: Claude Code has `.claude/agents/*.md`
definitions, dynamic `--agents` JSON, background agents, and agent teams; Antigravity has
async subagents managed through the `/agents` Agent Manager; Codex documents subagents
for parallelizing tasks (config surface unverified — see integration page). The brief
asks what "combining subagents" means operationally: reach inside each system, or treat
each CLI's top-level session as one agent and route between the three.

## Decision

**Peer-agent composition.** The unit swarm-tui orchestrates is a *top-level session* of
one CLI. The router dispatches tasks to peers, broadcasts to several, and (later)
chains them — it never enumerates, spawns, schedules, or messages another tool's
internal subagents. Whatever subagent fan-out a tool performs inside a session is that
tool's private business; swarm-tui sees it only as that session's activity.

One deliberate exception at the **provisioning** level, not runtime: swarm-tui may drop
static context files into a working directory before dispatch — notably `AGENTS.md`,
which Claude Code (via the CLAUDE.md shim convention), Codex, and Antigravity all read
natively. Writing files a tool reads on startup is configuration, not orchestration.

## Alternatives rejected

- **Cross-tool subagent fusion** (e.g., a Claude session's subagent calls into a Codex
  session's subagent). None of the three exposes a stable external contract for its
  internal agents: Claude's teams and background agents are driven through its own
  UI/CLI, agy's Agent Manager is a TUI panel, Codex's subagent config is undocumented.
  Building on private, weekly-changing internals maximizes breakage for a capability —
  finer-grained parallelism — that peer-level broadcast plus each tool's *own* internal
  fan-out already approximates.
- **Adopting one tool's subagent system as the meta-orchestrator** (e.g., Claude Code
  agent teams driving codex/agy as tools). Genuinely attractive — Claude's supervisor
  and teams are strong — but it makes swarm-tui a Claude plugin rather than a neutral
  layer, couples the home view to one vendor's roadmap, and inverts the product: the
  brief wants the orchestrator above all three, with each replaceable.

## Consequences

- The router's vocabulary stays small and stable: *dispatch, broadcast, promote,
  resume* — implementable with only the seams verified in ADR-0001.
- Fine-grained cross-tool pipelines (Claude plans → Codex implements → agy reviews)
  compose from peer dispatches with the artifact (files in the shared cwd) as the
  interchange — no new protocol.
- Revisit when: any vendor publishes a stable external API for driving its internal
  agents (Codex's `app-server` JSON-RPC is the closest existing thing and is already
  flagged in ADR-0001 as Codex's v2 seam).
