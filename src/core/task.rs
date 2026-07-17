//! Tasks: what the Home view dispatches (ARCHITECTURE task flow, steps 1–2;
//! ADR-0013).

use std::path::PathBuf;

use serde::Deserialize;

/// One unit of headless work aimed at one tool.
/// Broadcast = the same `Task` cloned across N tools (ADR-0004: composition
/// happens at the process level, via tasks and shared cwd — never by reaching
/// into a tool's internal subagent system).
#[derive(Debug, Clone)]
pub struct Task {
    pub prompt: String,
    /// cwd is a property of the TASK, not of swarm-tui — every dispatch names
    /// its working directory explicitly.
    pub cwd: PathBuf,
    pub budget: Budget,
    /// Passed verbatim to the tool's model/effort launch flags, exactly like
    /// the picker's `LaunchOptions` — mirrored here as plain strings because
    /// `core` cannot import `adapters` (ADR-0006 direction).
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Guardrails as data (ADR-0013), in ADR-0012's neutral vocabulary. Adapters
/// translate: claude maps posture to `--permission-mode` (+ a conservative
/// `--allowedTools` file set for `Edits`) and the caps to `--max-turns` /
/// `--max-budget-usd`; agy maps `timeout_secs` to `--print-timeout` and
/// refuses `Edits` until its no-TTY permission behavior is verified
/// (ARCHITECTURE guardrail table stays normative). Fields a tool has no
/// mechanism for are ignored, documented per adapter. The "never by default"
/// escape hatches are unrepresentable here.
#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    pub posture: DispatchPosture,
    pub max_turns: Option<u32>,
    pub max_usd: Option<f64>,
    pub timeout_secs: Option<u64>,
}

/// Neutral posture axis (declared in `.swarm/swarm.json` `defaults.dispatch`,
/// ADR-0012; consumed by dispatch, ADR-0013); each adapter maps it to its own
/// mechanism. Deliberately has no "bypass"/"skip permissions" value — the
/// ARCHITECTURE table's "never by default" row is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchPosture {
    /// Read/analyze only.
    ReadOnly,
    /// Plan first; no edits without escalation. The conservative default.
    #[default]
    Plan,
    /// Edits allowed within the adapter's conservative build-task envelope.
    Edits,
}

/// Router precedence (ADR-0013): the normative conservative defaults
/// (`Budget::default()`, i.e. the ARCHITECTURE table's posture) ← the
/// workspace's `defaults.dispatch` (ADR-0012) ← per-task edits, which the
/// dispatch form applies on the value this returns.
pub fn budget_from_workspace(prefs: Option<&crate::core::plan::DispatchPrefs>) -> Budget {
    let mut budget = Budget::default();
    let Some(prefs) = prefs else {
        return budget;
    };
    if let Some(posture) = prefs.posture {
        budget.posture = posture;
    }
    if let Some(turns) = prefs.max_turns {
        budget.max_turns = Some(u32::try_from(turns).unwrap_or(u32::MAX));
    }
    if let Some(usd) = prefs.max_budget_usd {
        budget.max_usd = Some(usd);
    }
    if let Some(secs) = prefs.timeout_secs {
        budget.timeout_secs = Some(secs);
    }
    budget
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_defaults_to_the_plan_posture_and_no_caps() {
        let budget = Budget::default();
        assert_eq!(budget.posture, DispatchPosture::Plan);
        assert_eq!(budget.max_turns, None);
        assert_eq!(budget.max_usd, None);
        assert_eq!(budget.timeout_secs, None);
    }

    #[test]
    fn workspace_dispatch_prefs_override_the_normative_defaults() {
        use crate::core::plan::DispatchPrefs;

        // No workspace prefs → the conservative defaults.
        assert_eq!(budget_from_workspace(None).posture, DispatchPosture::Plan);

        // Set fields override; unset fields keep the default.
        let prefs = DispatchPrefs {
            posture: Some(DispatchPosture::ReadOnly),
            max_turns: Some(12),
            max_budget_usd: None,
            timeout_secs: Some(120),
        };
        let budget = budget_from_workspace(Some(&prefs));
        assert_eq!(budget.posture, DispatchPosture::ReadOnly);
        assert_eq!(budget.max_turns, Some(12));
        assert_eq!(budget.max_usd, None);
        assert_eq!(budget.timeout_secs, Some(120));
    }
}
