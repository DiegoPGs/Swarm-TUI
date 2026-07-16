//! The swarm plan: a workspace roles file, `<launch cwd>/.swarm/swarm.json`
//! (ADR-0010). Roles are launch presets — tool, model, effort, purpose,
//! startup commands — consumed by the new-session picker. Presets, not
//! enforcement.
//!
//! Dependency direction (ADR-0006): `core` must not import `adapters`, so the
//! caller passes the active/known adapter slug lists in. "Known but not
//! active" produces the distinct suspended-tool error (ADR-0008's codex).

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

/// Relative path of the roles file under the launch cwd.
pub const PLAN_RELATIVE_PATH: &str = ".swarm/swarm.json";

const SUPPORTED_VERSION: u64 = 1;

/// A loaded, validated swarm plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmPlan {
    /// Role name → role. `BTreeMap` so the picker order is deterministic
    /// (alphabetical) without an order-preserving dependency.
    pub roles: BTreeMap<String, Role>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    /// Active adapter slug (validated against the caller's lists).
    pub tool: String,
    /// Passed verbatim to the tool's model launch flag — no translation table
    /// (ADR-0010 non-goal); invalid ids surface as the tool's own error.
    pub model: Option<String>,
    /// Ignored by adapters that don't declare an effort option.
    pub effort: Option<String>,
    /// Shown in the picker role line.
    pub purpose: Option<String>,
    /// Injected in order after first stable paint; each starts with '/'.
    pub startup_commands: Vec<String>,
}

/// One-line error surfaces (rendered verbatim in the picker — keep short).
#[derive(Debug)]
pub enum PlanError {
    /// The file exists but could not be read (missing file is `Ok(None)`).
    Read(String),
    /// JSON/shape error, with serde's position when it has one.
    Parse {
        line: usize,
        column: usize,
        msg: String,
    },
    Version {
        found: u64,
    },
    UnknownTool {
        role: String,
        tool: String,
        valid: Vec<String>,
    },
    SuspendedTool {
        role: String,
        tool: String,
        valid: Vec<String>,
    },
    BadStartupCommand {
        role: String,
        command: String,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Read(msg) => write!(f, "{PLAN_RELATIVE_PATH}: {msg}"),
            PlanError::Parse { line, column, msg } => {
                // serde_json reports 0:0 for io-category errors — a position
                // that would only mislead.
                if *line == 0 {
                    write!(f, "{PLAN_RELATIVE_PATH} — {msg}")
                } else {
                    write!(f, "{PLAN_RELATIVE_PATH}:{line}:{column} — {msg}")
                }
            }
            PlanError::Version { found } => write!(
                f,
                "{PLAN_RELATIVE_PATH}: version {found} is not supported \
                 (this build reads version {SUPPORTED_VERSION})"
            ),
            PlanError::UnknownTool { role, tool, valid } => write!(
                f,
                "role \"{role}\": unknown tool \"{tool}\" — valid tools: {}",
                valid.join(", ")
            ),
            PlanError::SuspendedTool { role, tool, valid } => write!(
                f,
                "role \"{role}\": tool \"{tool}\" is suspended (ADR-0008) — \
                 valid tools: {}",
                valid.join(", ")
            ),
            PlanError::BadStartupCommand { role, command } => write!(
                f,
                "role \"{role}\": startup command \"{command}\" must start with '/'"
            ),
        }
    }
}

// -- raw file shape (strict; private) ---------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanFile {
    version: u64,
    #[serde(default)]
    roles: BTreeMap<String, RoleSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleSpec {
    tool: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    startup_commands: Vec<String>,
}

impl SwarmPlan {
    /// Load `dir/.swarm/swarm.json`. Missing file ⇒ `Ok(None)`; anything else
    /// wrong ⇒ one `PlanError` (never partial, never a panic).
    ///
    /// `active_slugs`: adapters offered in the picker (`adapters::registry()`).
    /// `known_slugs`: every compiled adapter (`adapters::all_kinds()`), so a
    /// suspended tool errors as "suspended", not "unknown".
    pub fn load(
        dir: &Path,
        active_slugs: &[&str],
        known_slugs: &[&str],
    ) -> Result<Option<SwarmPlan>, PlanError> {
        let path = dir.join(PLAN_RELATIVE_PATH);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(PlanError::Read(e.to_string())),
        };
        Self::parse(&text, active_slugs, known_slugs).map(Some)
    }

    /// Parse + validate one file's contents (split from `load` for tests).
    fn parse(
        text: &str,
        active_slugs: &[&str],
        known_slugs: &[&str],
    ) -> Result<SwarmPlan, PlanError> {
        let file: PlanFile = serde_json::from_str(text).map_err(|e| PlanError::Parse {
            line: e.line(),
            column: e.column(),
            // serde_json appends " at line L column C" to `to_string()`;
            // the position already renders in the prefix — trim the echo.
            msg: e
                .to_string()
                .split(" at line ")
                .next()
                .unwrap_or_default()
                .to_string(),
        })?;

        if file.version != SUPPORTED_VERSION {
            return Err(PlanError::Version {
                found: file.version,
            });
        }

        let valid = || active_slugs.iter().map(|s| s.to_string()).collect();
        let mut roles = BTreeMap::new();
        for (name, spec) in file.roles {
            if !active_slugs.contains(&spec.tool.as_str()) {
                return Err(if known_slugs.contains(&spec.tool.as_str()) {
                    PlanError::SuspendedTool {
                        role: name,
                        tool: spec.tool,
                        valid: valid(),
                    }
                } else {
                    PlanError::UnknownTool {
                        role: name,
                        tool: spec.tool,
                        valid: valid(),
                    }
                });
            }
            if let Some(bad) = spec
                .startup_commands
                .iter()
                .find(|cmd| !cmd.starts_with('/'))
            {
                return Err(PlanError::BadStartupCommand {
                    role: name,
                    command: bad.clone(),
                });
            }
            roles.insert(
                name,
                Role {
                    tool: spec.tool,
                    model: spec.model,
                    effort: spec.effort,
                    purpose: spec.purpose,
                    startup_commands: spec.startup_commands,
                },
            );
        }
        Ok(SwarmPlan { roles })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE: &[&str] = &["claude-code", "antigravity"];
    const KNOWN: &[&str] = &["claude-code", "antigravity", "codex"];

    fn parse(text: &str) -> Result<SwarmPlan, PlanError> {
        SwarmPlan::parse(text, ACTIVE, KNOWN)
    }

    #[test]
    fn valid_plan_round_trips_all_fields() {
        let plan = parse(
            r#"{
                "version": 1,
                "roles": {
                    "coder": {
                        "tool": "claude-code",
                        "model": "opus-4.8",
                        "effort": "high",
                        "purpose": "implementation",
                        "startup_commands": ["/advisor fable"]
                    },
                    "researcher": { "tool": "antigravity" }
                }
            }"#,
        )
        .expect("valid plan");
        assert_eq!(plan.roles.len(), 2);
        let coder = &plan.roles["coder"];
        assert_eq!(coder.tool, "claude-code");
        assert_eq!(coder.model.as_deref(), Some("opus-4.8"));
        assert_eq!(coder.effort.as_deref(), Some("high"));
        assert_eq!(coder.purpose.as_deref(), Some("implementation"));
        assert_eq!(coder.startup_commands, vec!["/advisor fable"]);
        let researcher = &plan.roles["researcher"];
        assert_eq!(researcher.tool, "antigravity");
        assert!(researcher.model.is_none());
        assert!(researcher.startup_commands.is_empty());
        // BTreeMap ⇒ alphabetical iteration for the picker.
        let names: Vec<&str> = plan.roles.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["coder", "researcher"]);
    }

    #[test]
    fn missing_file_is_ok_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loaded = SwarmPlan::load(dir.path(), ACTIVE, KNOWN).expect("no error");
        assert!(loaded.is_none());
    }

    #[test]
    fn unknown_tool_errors_naming_valid_slugs() {
        let err = parse(r#"{ "version": 1, "roles": { "r": { "tool": "gemini" } } }"#)
            .expect_err("unknown tool");
        assert_eq!(
            err.to_string(),
            "role \"r\": unknown tool \"gemini\" — valid tools: claude-code, antigravity"
        );
    }

    #[test]
    fn suspended_codex_errors_distinctly_from_unknown() {
        let err = parse(r#"{ "version": 1, "roles": { "legacy": { "tool": "codex" } } }"#)
            .expect_err("suspended tool");
        assert_eq!(
            err.to_string(),
            "role \"legacy\": tool \"codex\" is suspended (ADR-0008) — \
             valid tools: claude-code, antigravity"
        );
    }

    #[test]
    fn startup_command_without_slash_is_rejected() {
        let err = parse(
            r#"{ "version": 1, "roles": { "r": {
                "tool": "claude-code", "startup_commands": ["status"] } } }"#,
        )
        .expect_err("bad startup command");
        assert_eq!(
            err.to_string(),
            "role \"r\": startup command \"status\" must start with '/'"
        );
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let err = parse(r#"{ "version": 2, "roles": {} }"#).expect_err("wrong version");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.json: version 2 is not supported (this build reads version 1)"
        );
    }

    #[test]
    fn malformed_json_error_carries_line_and_column() {
        let err = parse("{\n  \"version\": 1,\n  \"roles\": nope\n}").expect_err("malformed");
        let msg = err.to_string();
        assert!(
            msg.starts_with(".swarm/swarm.json:3:"),
            "expected file:line:col prefix, got: {msg}"
        );
        // serde's " at line L column C" echo is trimmed from the tail.
        assert!(!msg.contains(" at line "), "position echoed twice: {msg}");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let err = parse(
            r#"{ "version": 1, "roles": { "r": {
                "tool": "claude-code", "mdoel": "opus" } } }"#,
        )
        .expect_err("deny_unknown_fields");
        assert!(
            err.to_string().contains("unknown field `mdoel`"),
            "got: {err}"
        );
    }
}
