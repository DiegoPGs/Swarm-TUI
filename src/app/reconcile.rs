//! Reconciliation of Claude Code's native background-agent supervisor
//! (ADR-0002) into the Home roster. Source: `ClaudeCode::list_background_agents`
//! (`claude agents --json --all`).
//!
//! There is no documented field-level schema for that command's output —
//! only prose in `docs/integrations/claude-code.md` ("lists them as JSON",
//! "doesn't require a TTY (--all for completed)"). Parsing here is therefore
//! deliberately lenient: an unexpected top-level shape or a malformed
//! individual entry is logged via `tracing::warn!` and skipped, never a hard
//! failure — one bad entry (or a whole malformed payload) must never take
//! down the roster.
//!
//! Structurally observed locally (2026-07-15, `claude` 2.1.210): a bare
//! top-level array, entries keyed `sessionId` (camelCase, not `id`/
//! `session_id`) with no `status` field. `parse_agents_json` accepts all
//! three id-key spellings and defaults `status` to `"unknown"` accordingly —
//! see `docs/integrations/claude-code.md` for the field-name note.

use crate::app::home::RosterEntry;

/// One native background agent/session as reconciled from `claude agents
/// --json --all`, before it's folded into a `RosterEntry::ReconciledOnly`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledAgent {
    pub native_id: String,
    pub name: Option<String>,
    pub status: String,
}

/// Parse `claude agents --json --all`'s raw stdout leniently:
///
/// 1. Invalid JSON → `tracing::warn!`, return `vec![]`.
/// 2. Top level must be a bare array, or an object wrapping the array under
///    `"agents"` or `"sessions"` — any other shape → `tracing::warn!`,
///    return `vec![]`.
/// 3. Per entry: `native_id` from `"id"`, `"session_id"`, or `"sessionId"`
///    (`as_str`); if none is present, `tracing::warn!` and skip just that
///    entry. `name` from `"name"` (optional). `status` from `"status"`,
///    defaulting to `"unknown"` (the live `claude agents --json --all`
///    payload observed locally on 2026-07-15 has no `status` field at all —
///    see `docs/integrations/claude-code.md` — so this default is the
///    common case in practice, not just a defensive fallback).
pub fn parse_agents_json(raw: &str) -> Vec<ReconciledAgent> {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("claude agents --json --all: invalid JSON ({e}); treating as empty");
            return Vec::new();
        }
    };

    let items: &Vec<serde_json::Value> = match &value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(items)) = map.get("agents") {
                items
            } else if let Some(serde_json::Value::Array(items)) = map.get("sessions") {
                items
            } else {
                tracing::warn!(
                    "claude agents --json --all: object output had neither an 'agents' \
                     nor a 'sessions' array; treating as empty"
                );
                return Vec::new();
            }
        }
        _ => {
            tracing::warn!(
                "claude agents --json --all: unexpected top-level JSON shape \
                 (not an array or object); treating as empty"
            );
            return Vec::new();
        }
    };

    items
        .iter()
        .filter_map(|entry| {
            let native_id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| entry.get("session_id").and_then(|v| v.as_str()))
                .or_else(|| entry.get("sessionId").and_then(|v| v.as_str()));
            let Some(native_id) = native_id else {
                tracing::warn!(
                    "claude agents --json --all: entry missing 'id'/'session_id'/'sessionId'; skipping"
                );
                return None;
            };
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let status = entry
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some(ReconciledAgent {
                native_id: native_id.to_string(),
                name,
                status,
            })
        })
        .collect()
}

/// Fold one reconciled agent into a roster row. Never invents a registry
/// row — callers merge the result alongside `RosterEntry::Registered` rows
/// from `registry.all()`, additive only (Stage D spec).
pub fn to_roster_entry(agent: ReconciledAgent) -> RosterEntry {
    RosterEntry::ReconciledOnly {
        tool: "claude-code",
        native_id: agent.native_id,
        name: agent.name,
        status_hint: agent.status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/claude_agents_all.synthetic.json");

    #[test]
    fn parses_well_formed_entries_and_skips_malformed_one() {
        let agents = parse_agents_json(FIXTURE);

        // Fixture has 5 entries: 4 well-formed (one keyed by "id", one by
        // "session_id", one with no "name", one keyed by "sessionId" with no
        // "status" — matching the real shape observed locally) and 1
        // deliberately malformed (missing "id"/"session_id"/"sessionId")
        // that must be skipped, not panic the parse.
        assert_eq!(agents.len(), 4);

        assert_eq!(
            agents[0],
            ReconciledAgent {
                native_id: "fce17a07-840b-4f83-8aef-da36f14bedf7".to_string(),
                name: Some("test-session-alpha".to_string()),
                status: "running".to_string(),
            }
        );
        assert_eq!(
            agents[1],
            ReconciledAgent {
                native_id: "661c6bf1-b7e0-48c0-a653-eebcd0699a84".to_string(),
                name: Some("test-session-beta".to_string()),
                status: "completed".to_string(),
            }
        );
        assert_eq!(
            agents[2],
            ReconciledAgent {
                native_id: "d5de3421-1c07-4224-9847-dbbd95bc6979".to_string(),
                name: None,
                status: "running".to_string(),
            }
        );
        // The "sessionId"-keyed entry (real-shape case): other fields like
        // "pid"/"cwd"/"kind"/"startedAt" are present but ignored — only the
        // three recognized keys are extracted — and "status" is absent, so
        // it defaults to "unknown".
        assert_eq!(
            agents[3],
            ReconciledAgent {
                native_id: "0b6f0a5e-2c9c-4c1e-9a2f-5b7d0f6a9c3e".to_string(),
                name: Some("test-session-gamma".to_string()),
                status: "unknown".to_string(),
            }
        );

        // None of the surviving entries should be the malformed one.
        assert!(agents
            .iter()
            .all(|a| a.native_id != "test-session-missing-id"));
    }

    #[test]
    fn invalid_json_returns_empty_without_panicking() {
        assert!(parse_agents_json("not json").is_empty());
    }

    #[test]
    fn unwraps_object_wrapped_under_agents_key() {
        let raw = r#"{"agents": [{"id": "abc", "status": "running"}]}"#;
        let agents = parse_agents_json(raw);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].native_id, "abc");
    }

    #[test]
    fn unwraps_object_wrapped_under_sessions_key() {
        let raw = r#"{"sessions": [{"session_id": "xyz"}]}"#;
        let agents = parse_agents_json(raw);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].native_id, "xyz");
        assert_eq!(agents[0].status, "unknown");
    }

    #[test]
    fn unrecognized_top_level_shape_returns_empty() {
        assert!(parse_agents_json(r#"{"unexpected": "shape"}"#).is_empty());
        assert!(parse_agents_json("42").is_empty());
    }

    #[test]
    fn to_roster_entry_maps_fields() {
        let agent = ReconciledAgent {
            native_id: "id-1".to_string(),
            name: Some("name-1".to_string()),
            status: "running".to_string(),
        };
        match to_roster_entry(agent) {
            RosterEntry::ReconciledOnly {
                tool,
                native_id,
                name,
                status_hint,
            } => {
                assert_eq!(tool, "claude-code");
                assert_eq!(native_id, "id-1");
                assert_eq!(name, Some("name-1".to_string()));
                assert_eq!(status_hint, "running");
            }
            RosterEntry::Registered(_) => panic!("expected ReconciledOnly"),
        }
    }
}
