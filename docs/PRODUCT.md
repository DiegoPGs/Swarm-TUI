# Product brief

## Problem

Three terminal coding agents — Claude Code, Antigravity CLI (`agy`), Codex CLI — are
each installed, logged in, and configured on this machine, and each is used daily.
Working across them means three windows, three session models, and no shared home base.

## Product

One terminal application above all three:

1. **Wraps existing local installs.** Uses the logins and config already on disk.
   Never reimplements an agent, never triggers a new auth flow, never touches
   credential file contents.
2. **Per-service session management.** Start, resume, name, and switch between
   sessions of each CLI from inside the app. A session tab *is* the real tool running
   in a PTY — full native UX, approvals and all.
3. **A home view** for cross-cutting work: a roster of every session across all three
   tools (including Claude's native background sessions), single-target dispatch with
   sane guardrails, and broadcast (same prompt → N agents → compare results side by
   side). Not a passthrough to whichever tab has focus.
4. **Internal tabs**, moved between like panes in a multiplexer: `[Home]` plus one tab
   per live session.

## v1 scope

Roster · dispatch · broadcast · promote-to-tab · resume — on the three adapters, with
the guardrail defaults in `ARCHITECTURE.md`. A minimal named-pipeline feature (Claude
plans → Codex implements, artifacts flowing through the shared working directory) is
v1.5 if broadcast proves the event plumbing.

## Explicit non-goals

Re-authentication or credential handling of any kind; cross-tool context injection;
driving any CLI's internal subagent system (ADR-0004); transcript browsing/replay
(each tool's own resume UX does this better); being a general tmux replacement.

## Open questions for the owner

1. **Name.** ~~Open~~ **Decided 2026-07-05: `swarm-tui`** (owner's pick; the design
   session's working name was *overstory* — see README "Naming"). Crate renamed.
2. **License posture.** MIT scaffolded as the low-friction default; if this is ever
   public and the anti-enclosure instinct matters more than adoption friction, switch
   to GPLv3/AGPL *now*, while there are no outside contributors.
3. **Which cross-agent workflow actually earns its keep first?** Broadcast-and-compare
   (e.g., same review prompt to all three) vs. plan→implement pipelines vs. pure
   roster/dispatch. v1 assumes roster+dispatch+broadcast; reorder if wrong.
   *Roster + dispatch + broadcast-and-compare all shipped 2026-07-17 (milestone 3,
   ADR-0013); which one earns its keep is now a usage question, not a build one.*
4. **Worktree isolation.** Should home-view dispatch default to a fresh git worktree
   per task (Claude has `-w` native; Codex/agy would need swarm-tui to create it), or
   run in-place? In-place assumed for v1; worktrees are the obvious v2 safety upgrade.
   *Partially decided 2026-07-17 (ADR-0012): the plan schema reserves a
   `defaults.worktrees` slot (`in_place` accepted, `per_task` rejected as
   reserved); the behavior question itself stays open until dispatch lands.*
5. **agy under quota.** Given shared-quota burn (see integration page), should
   broadcast exclude agy by default and require opting it in per task?
   *Decided 2026-07-17 (ADR-0012): yes — nothing is broadcast-targeted unless
   named; an agy-backed role joins only when the workspace names it in
   `defaults.broadcast` or the user opts it in per task. Enforced since
   2026-07-17 (milestone 3, ADR-0013): the broadcast form preticks only named
   roles, ticking is the per-task opt-in, and the serialized agy lane refuses
   double-booking at submit.*
6. **Windows.** Design keeps the door open (ConPTY via portable-pty); is it worth any
   testing time at all, or is Linux+macOS the whole world for this tool?
7. **Approval surfacing (v2).** Claude's `--permission-prompt-tool` could route
   headless approval requests into the home view instead of failing/blocking — worth
   the MCP plumbing once dispatch is real?
