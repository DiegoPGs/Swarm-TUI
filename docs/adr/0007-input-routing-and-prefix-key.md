# ADR-0007: Input routing and prefix key

- Status: Accepted (2026-07-15)

## Context

No event-loop, keymap, or prefix-key design exists anywhere in the repo yet —
`docs/ARCHITECTURE.md` and every ADR through 0006 are silent on how keystrokes get from
the terminal to a tab or to the shell itself. A session tab must forward every keystroke
to the wrapped CLI's own TUI: `docs/ARCHITECTURE.md` and `session_view.rs`'s doc comment
both say the native TUI is the UX, and `session_view.rs` is explicit that it "must never
parse or reinterpret the tool's output." At the same time, the app still needs *some*
global shell-level commands — switch tabs, close a tab, quit, refresh the roster — that
have nowhere else to live. These two needs are in tension for any input scheme that
doesn't reserve something: forward-everything breaks global commands, and
intercept-anything breaks fidelity to the wrapped tool's own keymap.

## Decision

A single prefix key, **Ctrl-Space**, not user-configurable in this milestone. When a
session tab has focus, every key is forwarded verbatim to the PTY except Ctrl-Space.
Pressing Ctrl-Space enters a one-shot "awaiting command" mode where the *next* key is
interpreted per the keymap below, then control returns to forwarding mode. Pressing
Ctrl-Space twice in a row sends one literal Ctrl-Space byte through to the pane instead
of dispatching a command — that's the escape hatch for a wrapped tool that itself binds
Ctrl-Space.

Keymap (active only in the one-shot "awaiting command" state):

| Key | Action |
| --- | --- |
| `h` / `0` | Home |
| `1`-`9` | Jump to tab N |
| `n` / `p` | Cycle to next / previous tab |
| `c` | New session |
| `d` | Detach |
| `x` | Close tab (confirm) |
| `r` | Refresh roster |
| `?` | Keymap overlay |
| `q` | Quit (confirm if any pane is alive; quitting kills remaining panes after confirmation) |
| `:` | Command palette — inject a native slash command into the active session tab (added by ADR-0009, 2026-07-16) |

*Amended by ADR-0009 (2026-07-16): the `:` row above was added when the command
palette landed. The decision of this ADR — single Ctrl-Space prefix, one-shot
command mode, double-press literal passthrough — is unchanged.*

## Alternatives rejected

- **tmux-style Ctrl-b.** Rejected for two independent reasons: it clashes with existing
  tmux muscle memory AND it collides with Claude Code's own Ctrl-b background-shortcut,
  so a Claude Code session running inside a tab would have an ambiguous Ctrl-b.
- **Per-action global hotkeys (e.g. a bare letter key always means "new tab").**
  Rejected because they'd collide with the wrapped TUIs' own keymaps; e.g. any letter is
  meaningful inside Claude's Ink-based UI, so no letter can be safely stolen as an
  unprefixed global shortcut.

## Consequences

- Home-tab-local navigation (e.g. arrow keys / row selection when the Home tab has
  focus) is a *separate* input scope from the global prefix-key command table above —
  this is a likely source of future confusion between "what Ctrl-Space + a key does"
  and "what a bare key does while Home has focus," and is worth stating outright here.
- Every wrapped CLI keeps full use of its own keymap, including any use it makes of
  Ctrl-b, since swarm-tui never reserves anything but Ctrl-Space.
- The one-shot "awaiting command" state needs its own visible indicator (status line or
  similar) so a user mid-keystroke isn't guessing whether the next key goes to the shell
  or the pane; left to the implementation, not fixed by this ADR.
- Revisit when: the prefix key becomes user-configurable, or a future 4th wrapped CLI's
  own keymap turns out to collide with Ctrl-Space itself.
