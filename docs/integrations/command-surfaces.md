# Command surfaces: Claude Code & Antigravity CLI

*Milestone 2b, stage 2 — the research base for ADR-0009's per-adapter command
tables and launch options. Codex is suspended (ADR-0008) and has no columns here.*

Observed versions: **claude 2.1.211**, **agy 1.1.3**, both on 2026-07-16.
Markers: ✅ *(local 2026-07-16)* = observed on the installed binary (its "/"
autocomplete menu via `examples/slash_probe.rs`, or `--help`/subcommand output);
✅ *(docs)* = the official page (claude: code.claude.com/docs/en/commands, fetched
2026-07-16; agy: `agy changelog` release notes, which are shipped by the vendor and
read locally); 🔶 = reputable secondary; ⬜ = still unverified.

**Method.** Each TUI was booted inside `portable-pty` (env-scrubbed), `/` was typed
to open the native autocomplete, the list was walked with arrow keys and filtered by
typing command names, and every state was snapshotted from the vt100 grid
(`target/slash-probe/`). Characters, arrows, backspace, Esc only — **nothing was
ever submitted**. One owner-authorized exception (2026-07-16, recorded in NOTES.md):
agy's workspace-trust dialog for this repo dir was answered once to reach the
prompt. "Persists" below means the command's effect outlives the session (writes
tool state/config); session-scoped actions are marked "no".

## Claude Code (2.1.211)

All rows ✅ *(local 2026-07-16)* — present in the installed binary's "/" menu —
unless marked otherwise. Since-versions and persistence come from the official
commands page ✅ *(docs)* where noted. The menu mixes built-ins with user-installed
skills (e.g. `/postular`, `/claude-api`); this table covers built-ins only.

| Command | Since | Behavior (menu text) | Persists |
| --- | --- | --- | --- |
| `/model` | — | Set the AI model for Claude Code | **yes** — saved as default ✅ *(docs)*; picker offers `s` for session-only; `-p` mode session-only ≥2.1.205 |
| `/effort` | — | Set effort level for model usage | **yes** — persists as default ✅ *(docs)*; levels `low`–`ultracode` (see note) |
| `/advisor` | 2.1.170 ✅ *(docs)* | Let Claude consult a stronger model at key moments | no (session-only unless set as default ✅ *(docs)*) |
| `/resume` | — | Resume a previous conversation | no |
| `/branch` | — | Create a branch of the current conversation at this point | no |
| `/fork` | repurposed 2.1.161 ✅ *(docs)* | Spawn a background agent that inherits the full conversation | no |
| `/rewind` | — | Restore the code and/or conversation to a previous point | no |
| `/rename` | — | Rename the current conversation | no |
| `/compact` | — | Free up context by summarizing the conversation so far | no |
| `/context` | — | Visualize current context usage as a colored grid | no |
| `/usage` | — | Show session cost, plan usage, and activity stats | no |
| `/cost` | — | Alias of `/usage` — the menu renders it as `/usage (cost)` | no |
| `/status` | — | Show Claude Code status: version, model, account, API connectivity, tool statuses | no |
| `/tasks` | — | View and manage everything running in the background | no |
| `/background` | — | Send this session to the background and free the terminal (alias `/bg` ✅ *(docs)*) | no |
| `/goal` | — | Set a goal Claude checks before stopping | no |
| `/btw` | — | Ask a quick side question without interrupting the main conversation | no |
| `/plan` | — | Enable plan mode or view the current session plan | no |
| `/permissions` | — | Manage allow and deny tool permission rules | **yes** — rules land in settings |
| `/memory` | — | Open a memory file in your editor | **yes** — edits CLAUDE.md/auto-memory |
| `/keybindings` | — | Open your keyboard shortcuts file | **yes** — edits keybindings file |
| `/config` | — | Open settings (`/settings` alias ✅ *(docs)*) | **yes** ✅ *(docs)* |
| `/agents` | stubbed ≥2.1.198 ✅ *(docs)* | "(removed)" — prints a reminder to ask Claude or edit `.claude/agents/` | no |

Also present in the local menu (not needed by swarm-tui's v1 table): `/add-dir`,
`/artifacts`, `/autofix-pr`, `/cd`, `/chrome`, `/clear`, `/color`, `/copy`,
`/diff`, `/exit`, `/export`, `/fast` ("Toggle fast mode (Opus 4.8)"), `/feedback`,
`/focus`, `/help`, `/hooks`, `/ide`, `/install-github-app`, `/install-slack-app`,
`/login`, `/logout`, `/mcp`, `/mobile`, `/security-review`, `/statusline`,
`/teleport`, `/usage-credits`, `/doctor`, `/insights`, `/heapdump`, `/passes`,
`/desktop`, `/upgrade` (plan-gated), plus skill entries.

Notes:

- **`/advisor` verdict: real.** Present in the installed menu ✅ *(local
  2026-07-16)* and on the official page ✅ *(docs)* — `/advisor [model|off]`,
  accepts `opus`/`sonnet`/`fable`, since v2.1.170. The secondary-source
  "advisor-model pairing" description matches.
- **`/branch` / `/fork` history.** The official page records `/fork` as an alias of
  `/branch` before v2.1.161, when `/fork` was repurposed into the background
  forked-subagent spawner above. (Owner-recorded history: `/branch` renamed from
  `/fork` in v2.1.77 🔶.) Both exist today with distinct meanings.
- **`/effort` levels & persistence.** `--help` ✅ *(local)* lists launch levels
  `low, medium, high, xhigh, max`; the official page adds `ultracode` (= xhigh
  reasoning + auto workflow) and `auto` (reset) for the slash command, and says the
  interactive choice **persists as a saved default**. Owner-reported nuance 🔶:
  `max` and `ultracode` are session-only (do not stick as the default); the
  official page does not distinguish. ⬜ confirm by observation across two real
  sessions — not verifiable under this repo's never-Enter rule.
- **`/rename` is menu-only.** It exists locally ✅ but does **not** appear on the
  official commands page (fetched 2026-07-16).

### Launch flags relevant to ADR-0009 (all ✅ *(local 2026-07-16)*, `claude --help`)

- `--model <model>` — "Model for the current session"; aliases `fable`, `opus`,
  `sonnet`, or a full model name (e.g. `claude-fable-5`).
- `--effort <level>` — `low, medium, high, xhigh, max`.
- `--permission-mode <mode>` — `acceptEdits, auto, bypassPermissions, manual,
  dontAsk, plan`. **Drift:** the 2.1.201-era docs recorded `default` where 2.1.211
  offers `manual`.
- `--session-id <uuid>` — accepted on interactive fresh spawns (see
  claude-code.md, stamped 2026-07-16).

## Antigravity CLI (1.1.3)

All rows ✅ *(local 2026-07-16)* from the installed "/" menu (39 built-in entries;
the menu also lists installed skills like `/grill-me`, `/teamwork-preview`).
Alias notation is the menu's own — filtering by an alias shows it in parentheses,
e.g. typing `switch` renders `/resume (switch)`.

| Command | Aliases | Behavior (menu text) | Persists |
| --- | --- | --- | --- |
| `/model` | — | Set a model | **yes** 🔶 — observed default is non-factory ("Gemini 3.1 Pro (High)" vs the documented 3.5 Flash factory default), implying the choice sticks; exact mechanism ⬜ |
| `/resume` | `/switch`, `/conversation` (both ✅) | Browse and resume past conversations | no |
| `/fork` | `/branch` | Create a branch of the current conversation at this point, optionally specifying a project ID to fork into | no |
| `/rewind` | `/undo` | Rewind conversation to a previous message | no |
| `/rename` | — | Rename the current conversation | no |
| `/agents` | — | List available custom agents (the Agent Manager panel, per 1.1.0 release notes) | no |
| `/goal` | — | Run until the specified goal is completely finished. | no |
| `/schedule` | — | Run an instruction on a recurring schedule or as a one-time timer. | no (⬜ whether schedules outlive the session) |
| `/codesearch` | `/cs` ✅, `/search` ✅ *(docs — 1.1.3 release notes)* | Search code in the workspace (usage: `/codesearch <query>`) | no |
| `/btw` | — | Ask a side question without interrupting the current task | no |
| `/plan` | — | Plan carefully before executing a task. (replaced `/planning`; `/fast` removed — 1.1.0 ✅ *(docs)*) | no |
| `/tasks` | — | View background tasks | no |
| `/context` | — | Visualize current context usage | no |
| `/usage` | `/quota` | View model quota usage | no |
| `/credits` | — | Show remaining G1 credits and purchase link | no |
| `/permissions` | — | Manage tool permissions | **yes** — allow-rules land in `settings.json` (1.1.1 ✅ *(docs)*) |
| `/config` | `/settings` | Open settings panel | **yes** — `~/.gemini/antigravity-cli/settings.json` |
| `/keybindings` | — | Set custom keybindings | **yes** — `keybindings.json` (1.1.2 ✅ *(docs)*) |
| `/mcp` | — | Manage MCP servers | **yes** |
| `/diff` | — | View uncommitted changes and per-turn diffs | no |
| `/clear` | `/new` | Clear conversation and start a new one | no |

Remaining menu entries: `/add-dir`, `/artifact`, `/changelog`, `/copy`, `/exit
(quit)`, `/feedback`, `/help`, `/hooks`, `/learn`, `/logout`, `/open`, `/skills`,
`/statusline`, `/title`.

### Changes vs. the v1.0.14 facts in `antigravity.md` (all ✅ *(local 2026-07-16)*)

- New flags: `--agent` (1.1.1), `--mode` (`accept-edits`, `plan`; execution-mode
  cycling `default → accept-edits → plan` via shift+tab, and a persistable default
  via `/settings` "Agent Mode" — 1.1.0). Default execution behavior is
  `request-review` (1.1.0).
- New subcommands: `agent`/`agents` (list custom agents, 1.1.1).
- `agy models` ✅ lists exactly: `Gemini 3.5 Flash (Medium)`, `Gemini 3.5 Flash
  (High)`, `Gemini 3.5 Flash (Low)`, `Gemini 3.1 Pro (Low)`, `Gemini 3.1 Pro
  (High)`, `Claude Sonnet 4.6 (Thinking)`, `Claude Opus 4.6 (Thinking)`,
  `GPT-OSS 120B (Medium)`. Note the parenthesized reasoning level — agy has no
  `--effort`; depth is a property of the model variant.
  ⬜ the exact string format `--model` accepts (likely the listed names; a failed
  resolve hard-fails in `-p` listing valid values per 1.1.2 release notes — a
  supervised one-liner for the owner: `agy -p --model definitely-not-a-model "hi"`).
- **Still no structured-output flag** at 1.1.3 (`--help` re-checked) — the
  ADR-0001 "agy ships structured output" revisit trigger has NOT fired.
- Headless `-p` permission behavior changed at 1.1.3: tools needing confirmation
  are now soft-denied with a stderr notice naming the allow-rule (release notes ✅)
  — relevant to the still-open ⬜ dispatch items in antigravity.md.
- Slash menu is fully enumerable (this page); v1.0.14's page recorded only a
  partial list.

## Cross-tool concept map

"—" = no equivalent in that world. Codex: suspended, ADR-0008 — no column.

| Concept | Claude Code (2.1.211) | Antigravity CLI (1.1.3) |
| --- | --- | --- |
| Model selection | `/model` (sticky default) · launch `--model` (aliases `fable`/`opus`/`sonnet`) | `/model` · launch `--model` · `agy models` lists values |
| Reasoning/effort depth | `/effort` (`low`→`ultracode`, sticky) · launch `--effort` | — as a flag; encoded in the model variant name ("… (High)") |
| Resume | `/resume` · launch `--resume <id|name>` / `-c` | `/resume` (`/switch`, `/conversation`) · launch `--conversation <id>` / `-c` |
| Branch (conversation) | `/branch` · resume-time `--fork-session` | `/fork` (`/branch`) — optionally into another project |
| Fork-to-background | `/fork` (≥2.1.161, inherits conversation) | — |
| Rewind | `/rewind` (code and/or conversation) | `/rewind` (`/undo`) (conversation) |
| Rename | `/rename` | `/rename` |
| Background / subagents | `/background` (`/bg`) · `/tasks` · `claude agents` CLI | `/agents` panel · `/tasks` |
| Cost / usage visibility | `/usage` (`/cost`) · `/context` | `/usage` (`/quota`) · `/credits` · `/context` |
| Goal-until-done | `/goal` | `/goal` |
| Scheduled runs | — built-in (loop/schedule exist as user-side skills) | `/schedule` |
| Permission posture | launch `--permission-mode` (6 modes) · `/permissions` | launch `--mode` (`accept-edits`, `plan`) · shift+tab cycling · `/permissions` |
| Workspace context files | `CLAUDE.md` (+ `AGENTS.md` via shim) read natively ✅ *(docs)* | `AGENTS.md` read natively 🔶; workspace `.agents/` dir (hooks — 1.1.0 ✅ *(docs)*) |
| Config | `/config` (`/settings`) | `/config` (`/settings`) |
| Keybindings | `/keybindings` | `/keybindings` |
| Code search command | — (internal tooling) | `/codesearch` (`/cs`, `/search`) |
| Manual compaction | `/compact` | — (auto-compaction with visible boundaries — 1.1.3 ✅ *(docs)*) |

## Open ⬜ after this pass

- agy: exact `--model` argument format (supervised one-liner above).
- agy: `/model` persistence mechanism (sticky-default evidence is 🔶).
- agy: whether `/schedule` timers survive the session.
- claude: whether `/effort max`/`ultracode` stick as defaults (owner says no 🔶;
  official docs don't distinguish).
- The three pre-existing antigravity.md ⬜ items (live headless dispatch) are
  unchanged — out of scope this milestone.
