//! The swarm plan: workspace launch presets and defaults, loaded from two
//! layers under the launch cwd (ADR-0010, ADR-0012):
//!
//! - `.swarm/swarm.json` — the committed, shared layer;
//! - `.swarm/swarm.local.json` — the personal, gitignored overlay.
//!
//! Schema v1 (roles only) and v2 (roles + `defaults`) both load. The overlay
//! shallow-merges over the shared file per named entry — role names under
//! `roles`, field names under `defaults` — local wins per entry, and an entry
//! replaces wholesale (no deeper recursion). Roles are launch presets, not
//! enforcement; `defaults` are preferences, not policy.
//!
//! Dependency direction (ADR-0006): `core` must not import `adapters`, so the
//! caller passes the active/known adapter slug lists in. "Known but not
//! active" produces the distinct suspended-tool error (ADR-0008's codex).
//!
//! Nothing in this schema accepts credentials, tokens, or paths to them
//! (ADR-0010/0012 guarantee); `deny_unknown_fields` on every struct keeps a
//! field like that from ever sneaking in.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

/// Relative path of the committed, shared layer under the launch cwd.
pub const PLAN_RELATIVE_PATH: &str = ".swarm/swarm.json";
/// Relative path of the personal, gitignored overlay (ADR-0012).
pub const LOCAL_PLAN_RELATIVE_PATH: &str = ".swarm/swarm.local.json";

/// v1 = roles only; v2 adds `defaults`. Each layer declares its own version.
const MAX_SUPPORTED_VERSION: u64 = 2;
const SUPPORTED_VERSIONS_TEXT: &str = "versions 1 and 2";

/// A loaded, validated, merged swarm plan.
#[derive(Debug, Clone, PartialEq)]
pub struct SwarmPlan {
    /// Role name → role. `BTreeMap` so the picker order is deterministic
    /// (alphabetical) without an order-preserving dependency.
    pub roles: BTreeMap<String, Role>,
    /// Workspace defaults (schema v2, ADR-0012). All-`None` when both layers
    /// are v1 or declare no `defaults`.
    pub defaults: Defaults,
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

/// Schema-v2 `defaults` (ADR-0012). Every field optional; an absent field
/// means "no workspace preference" and the built-in behavior applies.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    /// Role preselected (cursor position only) in the new-session picker.
    #[serde(default)]
    pub default_role: Option<String>,
    /// Role names a broadcast targets by default (milestone 3 consumes this).
    /// Listing a role here is the explicit opt-in for its tool — including
    /// the quota-shared one PRODUCT.md question 5 worries about; nothing is
    /// broadcast-targeted by default without being named.
    #[serde(default)]
    pub broadcast: Option<Vec<String>>,
    /// Headless-dispatch guardrail preferences (milestone 3 consumes this).
    #[serde(default)]
    pub dispatch: Option<DispatchPrefs>,
    /// Worktree policy slot; only `in_place` is accepted by this build.
    #[serde(default)]
    pub worktrees: Option<WorktreePolicy>,
}

/// Neutral guardrail preferences for headless dispatch. These only tighten
/// or select among the normative per-adapter defaults in ARCHITECTURE.md
/// ("Guardrail defaults for headless dispatch") — the escape hatches those
/// defaults forbid have no representation here, and per-tool flag names stay
/// inside `src/adapters/` (ADR-0009 vocabulary rule).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPrefs {
    #[serde(default)]
    pub posture: Option<DispatchPosture>,
    /// Cap on agent turns per dispatched task (tools without a turn cap
    /// ignore it).
    #[serde(default)]
    pub max_turns: Option<u64>,
    /// Spend ceiling per dispatched task, in USD (tools without a budget
    /// flag ignore it).
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
    /// Wall-clock ceiling per dispatched task (tools without a timeout
    /// ignore it).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Neutral posture axis; each adapter maps it to its own mechanism at
/// dispatch time. Deliberately has no "bypass"/"skip permissions" value —
/// the ARCHITECTURE table's "never by default" row is unrepresentable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchPosture {
    /// Read/analyze only.
    ReadOnly,
    /// Plan first; no edits without escalation.
    Plan,
    /// Edits allowed within the adapter's conservative build-task envelope.
    Edits,
}

/// Where dispatched work runs. `per_task` is a reserved schema slot
/// (PRODUCT.md question 4) — this build rejects it rather than silently
/// running in place under a label that promises isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreePolicy {
    InPlace,
    PerTask,
}

/// One-line error surfaces (rendered verbatim in the picker — keep short).
/// `file` is the layer that caused the error: role- and defaults-level
/// messages from the shared file keep their pre-overlay wording (no prefix);
/// every other case names the offending file.
#[derive(Debug)]
pub enum PlanError {
    /// The file exists but could not be read (missing file is "layer absent").
    Read {
        file: &'static str,
        msg: String,
    },
    /// JSON/shape error, with serde's position when it has one.
    Parse {
        file: &'static str,
        line: usize,
        column: usize,
        msg: String,
    },
    Version {
        file: &'static str,
        found: u64,
    },
    /// `defaults` present in a file that declares version 1.
    DefaultsNeedV2 {
        file: &'static str,
    },
    UnknownTool {
        file: &'static str,
        role: String,
        tool: String,
        valid: Vec<String>,
    },
    SuspendedTool {
        file: &'static str,
        role: String,
        tool: String,
        valid: Vec<String>,
    },
    BadStartupCommand {
        file: &'static str,
        role: String,
        command: String,
    },
    /// A `defaults` value failed validation (bad reference, reserved policy,
    /// out-of-domain limit).
    Defaults {
        file: &'static str,
        msg: String,
    },
}

/// Role-level messages predate the overlay and are pinned (tests + picker
/// UX): the shared file renders them bare, the overlay names itself.
fn role_error_prefix(file: &str) -> &str {
    if file == PLAN_RELATIVE_PATH {
        ""
    } else {
        LOCAL_ROLE_PREFIX
    }
}
const LOCAL_ROLE_PREFIX: &str = ".swarm/swarm.local.json: ";

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Read { file, msg } => write!(f, "{file}: {msg}"),
            PlanError::Parse {
                file,
                line,
                column,
                msg,
            } => {
                // serde_json reports 0:0 for io-category errors — a position
                // that would only mislead.
                if *line == 0 {
                    write!(f, "{file} — {msg}")
                } else {
                    write!(f, "{file}:{line}:{column} — {msg}")
                }
            }
            PlanError::Version { file, found } => write!(
                f,
                "{file}: version {found} is not supported \
                 (this build reads {SUPPORTED_VERSIONS_TEXT})"
            ),
            PlanError::DefaultsNeedV2 { file } => write!(
                f,
                "{file}: \"defaults\" requires version 2 (this file declares version 1)"
            ),
            PlanError::UnknownTool {
                file,
                role,
                tool,
                valid,
            } => write!(
                f,
                "{}role \"{role}\": unknown tool \"{tool}\" — valid tools: {}",
                role_error_prefix(file),
                valid.join(", ")
            ),
            PlanError::SuspendedTool {
                file,
                role,
                tool,
                valid,
            } => write!(
                f,
                "{}role \"{role}\": tool \"{tool}\" is suspended (ADR-0008) — \
                 valid tools: {}",
                role_error_prefix(file),
                valid.join(", ")
            ),
            PlanError::BadStartupCommand {
                file,
                role,
                command,
            } => write!(
                f,
                "{}role \"{role}\": startup command \"{command}\" must start with '/'",
                role_error_prefix(file)
            ),
            PlanError::Defaults { file, msg } => write!(f, "{file}: defaults: {msg}"),
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
    #[serde(default)]
    defaults: Option<Defaults>,
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

/// One parsed, self-valid layer. Cross-layer references (`default_role`,
/// `broadcast`) are checked after the merge — they may legally point at a
/// role the *other* layer defines.
struct Layer {
    file: &'static str,
    roles: BTreeMap<String, Role>,
    defaults: Defaults,
}

impl SwarmPlan {
    /// Load and merge `dir/.swarm/swarm.json` (shared) and
    /// `dir/.swarm/swarm.local.json` (personal overlay). Both files missing ⇒
    /// `Ok(None)`; a missing layer is simply absent; anything else wrong in
    /// either layer ⇒ one `PlanError` for the whole load (never partial,
    /// never a panic).
    ///
    /// `active_slugs`: adapters offered in the picker (`adapters::registry()`).
    /// `known_slugs`: every compiled adapter (`adapters::all_kinds()`), so a
    /// suspended tool errors as "suspended", not "unknown".
    pub fn load(
        dir: &Path,
        active_slugs: &[&str],
        known_slugs: &[&str],
    ) -> Result<Option<SwarmPlan>, PlanError> {
        let shared = load_layer(dir, PLAN_RELATIVE_PATH, active_slugs, known_slugs)?;
        let local = load_layer(dir, LOCAL_PLAN_RELATIVE_PATH, active_slugs, known_slugs)?;
        match (shared, local) {
            (None, None) => Ok(None),
            (shared, local) => merge_layers(shared, local).map(Some),
        }
    }

    /// Parse + validate a lone shared-layer text (kept for tests: the v1
    /// surface loaded exactly one file and its error strings are pinned).
    #[cfg(test)]
    fn parse(
        text: &str,
        active_slugs: &[&str],
        known_slugs: &[&str],
    ) -> Result<SwarmPlan, PlanError> {
        let layer = parse_layer(text, PLAN_RELATIVE_PATH, active_slugs, known_slugs)?;
        merge_layers(Some(layer), None)
    }
}

fn load_layer(
    dir: &Path,
    file: &'static str,
    active_slugs: &[&str],
    known_slugs: &[&str],
) -> Result<Option<Layer>, PlanError> {
    let path = dir.join(file);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(PlanError::Read {
                file,
                msg: e.to_string(),
            })
        }
    };
    parse_layer(&text, file, active_slugs, known_slugs).map(Some)
}

/// Parse one layer and validate everything that is knowable from that file
/// alone: shape, version, role tools and startup commands, `defaults` value
/// domains. A present layer must be entirely valid on its own terms — a
/// broken value is an error even if the other layer would override it (the
/// file is shared/committed; someone without the overlay would hit it).
fn parse_layer(
    text: &str,
    file: &'static str,
    active_slugs: &[&str],
    known_slugs: &[&str],
) -> Result<Layer, PlanError> {
    let parsed: PlanFile = serde_json::from_str(text).map_err(|e| PlanError::Parse {
        file,
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

    if parsed.version == 0 || parsed.version > MAX_SUPPORTED_VERSION {
        return Err(PlanError::Version {
            file,
            found: parsed.version,
        });
    }
    if parsed.version == 1 && parsed.defaults.is_some() {
        return Err(PlanError::DefaultsNeedV2 { file });
    }

    let valid = || active_slugs.iter().map(|s| s.to_string()).collect();
    let mut roles = BTreeMap::new();
    for (name, spec) in parsed.roles {
        if !active_slugs.contains(&spec.tool.as_str()) {
            return Err(if known_slugs.contains(&spec.tool.as_str()) {
                PlanError::SuspendedTool {
                    file,
                    role: name,
                    tool: spec.tool,
                    valid: valid(),
                }
            } else {
                PlanError::UnknownTool {
                    file,
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
                file,
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

    let defaults = parsed.defaults.unwrap_or_default();
    validate_defaults_domains(file, &defaults)?;

    Ok(Layer {
        file,
        roles,
        defaults,
    })
}

/// Value-domain checks on one file's `defaults` (references to roles are
/// checked post-merge instead — see `merge_layers`).
fn validate_defaults_domains(file: &'static str, defaults: &Defaults) -> Result<(), PlanError> {
    let err = |msg: String| Err(PlanError::Defaults { file, msg });

    if let Some(names) = &defaults.broadcast {
        if names.is_empty() {
            return err("broadcast must name at least one role".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(dup) = names.iter().find(|n| !seen.insert(n.as_str())) {
            return err(format!("broadcast role \"{dup}\" is listed twice"));
        }
    }
    if let Some(WorktreePolicy::PerTask) = defaults.worktrees {
        return err("worktrees \"per_task\" is reserved (this build runs in_place)".to_string());
    }
    if let Some(dispatch) = &defaults.dispatch {
        if dispatch.max_turns == Some(0) {
            return err("dispatch.max_turns must be at least 1".to_string());
        }
        if let Some(budget) = dispatch.max_budget_usd {
            // `<= 0.0` alone would let NaN through (all comparisons on NaN
            // are false); `!is_finite()` catches it.
            if budget <= 0.0 || !budget.is_finite() {
                return err("dispatch.max_budget_usd must be a positive number".to_string());
            }
        }
        if dispatch.timeout_secs == Some(0) {
            return err("dispatch.timeout_secs must be at least 1".to_string());
        }
    }
    Ok(())
}

/// Shallow merge (ADR-0012): union per named entry — role names under
/// `roles`, field names under `defaults` — local wins per entry, and a
/// winning entry replaces the shared one wholesale (a local role does not
/// inherit fields from the shared role of the same name; a local `dispatch`
/// object replaces the shared one as a unit). Then validate the references
/// that only the merged whole can answer, blaming the layer whose entry won.
fn merge_layers(shared: Option<Layer>, local: Option<Layer>) -> Result<SwarmPlan, PlanError> {
    let mut roles = BTreeMap::new();
    let mut default_role: Option<(String, &'static str)> = None;
    let mut broadcast: Option<(Vec<String>, &'static str)> = None;
    let mut dispatch: Option<DispatchPrefs> = None;
    let mut worktrees: Option<WorktreePolicy> = None;

    for layer in [shared, local].into_iter().flatten() {
        roles.extend(layer.roles);
        if let Some(v) = layer.defaults.default_role {
            default_role = Some((v, layer.file));
        }
        if let Some(v) = layer.defaults.broadcast {
            broadcast = Some((v, layer.file));
        }
        if let Some(v) = layer.defaults.dispatch {
            dispatch = Some(v);
        }
        if let Some(v) = layer.defaults.worktrees {
            worktrees = Some(v);
        }
    }

    if let Some((name, file)) = &default_role {
        if !roles.contains_key(name) {
            return Err(PlanError::Defaults {
                file,
                msg: format!("default_role \"{name}\" is not a defined role"),
            });
        }
    }
    if let Some((names, file)) = &broadcast {
        if let Some(bad) = names.iter().find(|n| !roles.contains_key(*n)) {
            return Err(PlanError::Defaults {
                file,
                msg: format!("broadcast role \"{bad}\" is not a defined role"),
            });
        }
    }

    Ok(SwarmPlan {
        roles,
        defaults: Defaults {
            default_role: default_role.map(|(v, _)| v),
            broadcast: broadcast.map(|(v, _)| v),
            dispatch,
            worktrees,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE: &[&str] = &["claude-code", "antigravity"];
    const KNOWN: &[&str] = &["claude-code", "antigravity", "codex"];

    fn parse(text: &str) -> Result<SwarmPlan, PlanError> {
        SwarmPlan::parse(text, ACTIVE, KNOWN)
    }

    /// Write the given layer texts into a tempdir's `.swarm/` and run the
    /// real two-layer `load` — the same code path the app uses.
    fn load_pair(
        shared: Option<&str>,
        local: Option<&str>,
    ) -> Result<Option<SwarmPlan>, PlanError> {
        let dir = tempfile::tempdir().expect("tempdir");
        let swarm = dir.path().join(".swarm");
        std::fs::create_dir_all(&swarm).expect("mkdir .swarm");
        if let Some(text) = shared {
            std::fs::write(swarm.join("swarm.json"), text).expect("write shared layer");
        }
        if let Some(text) = local {
            std::fs::write(swarm.join("swarm.local.json"), text).expect("write local layer");
        }
        SwarmPlan::load(dir.path(), ACTIVE, KNOWN)
    }

    // -- schema v1 (must keep loading identically) --------------------------

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
        // v1 carries no defaults.
        assert_eq!(plan.defaults, Defaults::default());
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
        let err = parse(r#"{ "version": 3, "roles": {} }"#).expect_err("wrong version");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.json: version 3 is not supported (this build reads versions 1 and 2)"
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

    // -- schema v2 ----------------------------------------------------------

    #[test]
    fn v2_plan_round_trips_defaults() {
        let plan = parse(
            r#"{
                "version": 2,
                "roles": {
                    "coder": { "tool": "claude-code", "model": "opus-4.8" },
                    "researcher": { "tool": "antigravity" }
                },
                "defaults": {
                    "default_role": "coder",
                    "broadcast": ["coder", "researcher"],
                    "dispatch": {
                        "posture": "plan",
                        "max_turns": 30,
                        "max_budget_usd": 2.5,
                        "timeout_secs": 300
                    },
                    "worktrees": "in_place"
                }
            }"#,
        )
        .expect("valid v2 plan");
        assert_eq!(plan.defaults.default_role.as_deref(), Some("coder"));
        assert_eq!(
            plan.defaults.broadcast,
            Some(vec!["coder".to_string(), "researcher".to_string()])
        );
        let dispatch = plan.defaults.dispatch.expect("dispatch prefs");
        assert_eq!(dispatch.posture, Some(DispatchPosture::Plan));
        assert_eq!(dispatch.max_turns, Some(30));
        assert_eq!(dispatch.max_budget_usd, Some(2.5));
        assert_eq!(dispatch.timeout_secs, Some(300));
        assert_eq!(plan.defaults.worktrees, Some(WorktreePolicy::InPlace));
    }

    #[test]
    fn v2_defaults_are_entirely_optional() {
        let plan = parse(r#"{ "version": 2, "roles": { "r": { "tool": "claude-code" } } }"#)
            .expect("v2 without defaults");
        assert_eq!(plan.defaults, Defaults::default());
    }

    #[test]
    fn v1_file_with_defaults_is_rejected() {
        let err =
            parse(r#"{ "version": 1, "roles": {}, "defaults": { "worktrees": "in_place" } }"#)
                .expect_err("defaults need v2");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.json: \"defaults\" requires version 2 (this file declares version 1)"
        );
    }

    #[test]
    fn unknown_defaults_field_is_rejected() {
        let err = parse(r#"{ "version": 2, "roles": {}, "defaults": { "default_rol": "x" } }"#)
            .expect_err("deny_unknown_fields in defaults");
        assert!(
            err.to_string().contains("unknown field `default_rol`"),
            "got: {err}"
        );
    }

    #[test]
    fn unknown_posture_is_rejected() {
        let err = parse(
            r#"{ "version": 2, "roles": {},
                "defaults": { "dispatch": { "posture": "yolo" } } }"#,
        )
        .expect_err("unknown posture variant");
        assert!(
            err.to_string().contains("unknown variant `yolo`"),
            "got: {err}"
        );
    }

    #[test]
    fn default_role_must_name_a_defined_role() {
        let err = parse(
            r#"{ "version": 2, "roles": { "coder": { "tool": "claude-code" } },
                "defaults": { "default_role": "ghost" } }"#,
        )
        .expect_err("unknown default_role");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.json: defaults: default_role \"ghost\" is not a defined role"
        );
    }

    #[test]
    fn broadcast_roles_must_be_defined_nonempty_unique() {
        let unknown = parse(
            r#"{ "version": 2, "roles": { "coder": { "tool": "claude-code" } },
                "defaults": { "broadcast": ["coder", "ghost"] } }"#,
        )
        .expect_err("unknown broadcast role");
        assert_eq!(
            unknown.to_string(),
            ".swarm/swarm.json: defaults: broadcast role \"ghost\" is not a defined role"
        );

        let empty = parse(r#"{ "version": 2, "roles": {}, "defaults": { "broadcast": [] } }"#)
            .expect_err("empty broadcast");
        assert_eq!(
            empty.to_string(),
            ".swarm/swarm.json: defaults: broadcast must name at least one role"
        );

        let dup = parse(
            r#"{ "version": 2, "roles": { "coder": { "tool": "claude-code" } },
                "defaults": { "broadcast": ["coder", "coder"] } }"#,
        )
        .expect_err("duplicate broadcast role");
        assert_eq!(
            dup.to_string(),
            ".swarm/swarm.json: defaults: broadcast role \"coder\" is listed twice"
        );
    }

    #[test]
    fn worktrees_per_task_is_reserved() {
        let err =
            parse(r#"{ "version": 2, "roles": {}, "defaults": { "worktrees": "per_task" } }"#)
                .expect_err("reserved policy");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.json: defaults: worktrees \"per_task\" is reserved \
             (this build runs in_place)"
        );
    }

    #[test]
    fn dispatch_limits_must_be_positive() {
        let turns = parse(
            r#"{ "version": 2, "roles": {},
                "defaults": { "dispatch": { "max_turns": 0 } } }"#,
        )
        .expect_err("zero turns");
        assert_eq!(
            turns.to_string(),
            ".swarm/swarm.json: defaults: dispatch.max_turns must be at least 1"
        );

        let budget = parse(
            r#"{ "version": 2, "roles": {},
                "defaults": { "dispatch": { "max_budget_usd": -1.0 } } }"#,
        )
        .expect_err("negative budget");
        assert_eq!(
            budget.to_string(),
            ".swarm/swarm.json: defaults: dispatch.max_budget_usd must be a positive number"
        );

        let timeout = parse(
            r#"{ "version": 2, "roles": {},
                "defaults": { "dispatch": { "timeout_secs": 0 } } }"#,
        )
        .expect_err("zero timeout");
        assert_eq!(
            timeout.to_string(),
            ".swarm/swarm.json: defaults: dispatch.timeout_secs must be at least 1"
        );
    }

    // -- the two-layer merge (ADR-0012) -------------------------------------

    #[test]
    fn local_only_layer_loads() {
        let plan = load_pair(
            None,
            Some(r#"{ "version": 2, "roles": { "mine": { "tool": "claude-code" } } }"#),
        )
        .expect("no error")
        .expect("plan present");
        assert_eq!(plan.roles.len(), 1);
        assert!(plan.roles.contains_key("mine"));
    }

    #[test]
    fn overlay_adds_roles_and_wins_wholesale_per_role() {
        // The local layer may be v1 (roles only) — same schema family.
        let plan = load_pair(
            Some(
                r#"{ "version": 1, "roles": {
                    "coder": { "tool": "claude-code", "model": "opus-4.8", "effort": "high" },
                    "researcher": { "tool": "antigravity" } } }"#,
            ),
            Some(
                r#"{ "version": 1, "roles": {
                    "coder": { "tool": "claude-code", "model": "sonnet-5" },
                    "reviewer": { "tool": "claude-code" } } }"#,
            ),
        )
        .expect("no error")
        .expect("plan present");
        // Union: shared-only + local-only roles both present.
        assert_eq!(plan.roles.len(), 3);
        assert!(plan.roles.contains_key("researcher"));
        assert!(plan.roles.contains_key("reviewer"));
        // Conflict: the local role wins wholesale — it does NOT inherit the
        // shared role's effort (shallow merge, no per-field recursion).
        let coder = &plan.roles["coder"];
        assert_eq!(coder.model.as_deref(), Some("sonnet-5"));
        assert_eq!(coder.effort, None);
    }

    #[test]
    fn overlay_defaults_merge_per_field() {
        let plan = load_pair(
            Some(
                r#"{ "version": 2,
                    "roles": { "coder": { "tool": "claude-code" } },
                    "defaults": {
                        "default_role": "coder",
                        "dispatch": { "posture": "plan" } } }"#,
            ),
            Some(
                r#"{ "version": 2,
                    "roles": { "reviewer": { "tool": "claude-code" } },
                    "defaults": { "default_role": "reviewer" } }"#,
            ),
        )
        .expect("no error")
        .expect("plan present");
        // Conflicting field: local wins.
        assert_eq!(plan.defaults.default_role.as_deref(), Some("reviewer"));
        // Field only the shared layer set: survives the overlay.
        let dispatch = plan.defaults.dispatch.expect("shared dispatch survives");
        assert_eq!(dispatch.posture, Some(DispatchPosture::Plan));
    }

    #[test]
    fn default_role_may_reference_a_role_from_the_other_layer() {
        // The shared file preselects a role only the overlay defines.
        let plan = load_pair(
            Some(
                r#"{ "version": 2, "roles": {},
                    "defaults": { "default_role": "mine" } }"#,
            ),
            Some(r#"{ "version": 2, "roles": { "mine": { "tool": "claude-code" } } }"#),
        )
        .expect("no error")
        .expect("plan present");
        assert_eq!(plan.defaults.default_role.as_deref(), Some("mine"));
    }

    #[test]
    fn merged_reference_errors_blame_the_layer_that_won() {
        // Shared broadcast is valid; the local override references a ghost —
        // the error must name the local file.
        let err = load_pair(
            Some(
                r#"{ "version": 2,
                    "roles": { "coder": { "tool": "claude-code" } },
                    "defaults": { "broadcast": ["coder"] } }"#,
            ),
            Some(
                r#"{ "version": 2, "roles": {},
                    "defaults": { "broadcast": ["ghost"] } }"#,
            ),
        )
        .expect_err("unknown broadcast role in overlay");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.local.json: defaults: broadcast role \"ghost\" is not a defined role"
        );
    }

    #[test]
    fn malformed_overlay_names_the_local_file_and_fails_the_whole_load() {
        let err = load_pair(
            Some(r#"{ "version": 1, "roles": { "coder": { "tool": "claude-code" } } }"#),
            Some("{ not json"),
        )
        .expect_err("malformed overlay must fail the load, not load partially");
        let msg = err.to_string();
        assert!(
            msg.starts_with(".swarm/swarm.local.json"),
            "error must name the offending file, got: {msg}"
        );
    }

    #[test]
    fn overlay_role_errors_name_the_local_file() {
        let err = load_pair(
            None,
            Some(r#"{ "version": 1, "roles": { "r": { "tool": "gemini" } } }"#),
        )
        .expect_err("unknown tool in overlay");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.local.json: role \"r\": unknown tool \"gemini\" — \
             valid tools: claude-code, antigravity"
        );
    }

    #[test]
    fn layer_versions_gate_independently() {
        let err = load_pair(
            Some(r#"{ "version": 1, "roles": {} }"#),
            Some(r#"{ "version": 3, "roles": {} }"#),
        )
        .expect_err("unsupported overlay version");
        assert_eq!(
            err.to_string(),
            ".swarm/swarm.local.json: version 3 is not supported \
             (this build reads versions 1 and 2)"
        );
    }
}
