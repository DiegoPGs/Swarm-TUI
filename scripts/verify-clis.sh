#!/usr/bin/env bash
# verify-clis.sh — read-only environment check for the three wrapped CLIs.
#
# Confirms, on THIS machine, the facts marked "verify locally" (⬜) in
# docs/integrations/*.md: binary presence, versions, key flags, and config/auth
# PATH EXISTENCE. It never prints, reads, or copies the CONTENTS of any
# credential or settings file — that is a hard project boundary (see AGENTS.md).
#
# Usage: ./scripts/verify-clis.sh   (no arguments, no side effects)

set -u

ok()   { printf '  \033[32m✔\033[0m %s\n' "$1"; }
warn() { printf '  \033[33m⚠\033[0m %s\n' "$1"; }
miss() { printf '  \033[31m✘\033[0m %s\n' "$1"; }
hdr()  { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

# path_check <label> <path>  — existence only, never contents
path_check() {
  if [ -e "$2" ]; then ok "$1 exists: $2"; else warn "$1 not found: $2"; fi
}

# flag_check <binary> <flag-substring> <label>
flag_check() {
  if "$1" --help 2>&1 | grep -q -- "$2"; then
    ok "$3 ($2)"
  else
    warn "$3: '$2' not in --help (Claude Code hides some flags; others may have drifted — check docs/integrations)"
  fi
}

# ---------------------------------------------------------------- Claude Code
hdr "Claude Code (claude)"
if command -v claude >/dev/null 2>&1; then
  ok "binary: $(command -v claude) — version: $(claude --version 2>/dev/null | head -n1)"
  for spec in \
    "--print|headless print mode" \
    "--output-format|structured output" \
    "--resume|resume by id/name" \
    "--session-id|pre-assigned session id" \
    "--max-budget-usd|budget hard stop" \
    "--bg|native background sessions"; do
    flag_check claude "${spec%%|*}" "${spec#*|}"
  done
  # Auth state via the documented JSON status command (safe: no secrets printed).
  if claude auth status >/dev/null 2>&1; then ok "auth: logged in (claude auth status → 0)"; else warn "auth: not logged in (exit 1)"; fi
  # ⬜ MCP server mode — record the answer in docs/integrations/claude-code.md:
  if claude mcp --help 2>&1 | grep -qi "serve"; then ok "MCP server subcommand present"; else warn "no MCP 'serve' in claude mcp --help (as current docs suggest)"; fi
  path_check "user config dir" "$HOME/.claude"
  path_check "state file"      "$HOME/.claude.json"
else
  miss "claude not found on PATH"
fi

# ----------------------------------------------------------- Antigravity CLI
hdr "Antigravity CLI (agy)"
if command -v agy >/dev/null 2>&1; then
  ok "binary: $(command -v agy) — version: $(agy --version 2>/dev/null | head -n1)"
  for spec in \
    "--print|headless print mode" \
    "--conversation|resume by conversation id" \
    "--continue|continue most recent" \
    "--print-timeout|print-mode timeout" \
    "--output-format|structured output (EXPECTED ABSENT at v1.0.16 — if present, update ADR-0001!)"; do
    flag_check agy "${spec%%|*}" "${spec#*|}"
  done
  path_check "CLI settings"        "$HOME/.gemini/antigravity-cli/settings.json"
  path_check "shared hooks config" "$HOME/.gemini/config/hooks.json"
  path_check "global MCP config"   "$HOME/.gemini/config/mcp_config.json"
  # ⬜ Locate the SQLite conversation store (path only). Common candidates:
  found_db=$(find "$HOME/.gemini/antigravity-cli" -maxdepth 3 -name '*.db' 2>/dev/null | head -n3)
  if [ -n "$found_db" ]; then printf '%s\n' "$found_db" | while read -r f; do ok "candidate conversation store: $f"; done
  else warn "no .db found under ~/.gemini/antigravity-cli — locate the conversation store manually"; fi
else
  miss "agy not found on PATH"
fi

# ------------------------------------------------------------------ Codex CLI
hdr "Codex CLI (codex)"
if command -v codex >/dev/null 2>&1; then
  ok "binary: $(command -v codex) — version: $(codex --version 2>/dev/null | head -n1)"
  if codex exec --help >/dev/null 2>&1; then ok "exec subcommand present"; else miss "codex exec missing"; fi
  if codex exec --help 2>&1 | grep -q -- "--json"; then ok "exec --json (JSONL events)"; else warn "exec --json not in help"; fi
  if codex exec resume --help >/dev/null 2>&1; then ok "headless resume (codex exec resume)"; else warn "codex exec resume missing"; fi
  if codex mcp-server --help >/dev/null 2>&1 || codex --help 2>&1 | grep -q "mcp-server"; then ok "mcp-server mode present"; else warn "mcp-server not visible in help"; fi
  # ⬜ [agents] config block — grep help/docs, not the user's config contents:
  if codex --help 2>&1 | grep -qi "agents"; then ok "'agents' mentioned in top-level help"; else warn "'agents' not in help — [agents] config remains unverified"; fi
  path_check "config"        "$HOME/.codex/config.toml"
  path_check "auth file"     "$HOME/.codex/auth.json"
  path_check "sessions dir"  "$HOME/.codex/sessions"
else
  miss "codex not found on PATH"
fi

hdr "Next step"
echo "  Fold results into docs/integrations/*.md (flip ⬜ → ✅/✘ with version + date)."
echo "  Any result contradicting an ADR gets a superseding ADR, not a silent workaround."
