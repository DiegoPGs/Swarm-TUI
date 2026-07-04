//! Home view: the cross-agent surface (docs/PRODUCT.md, ARCHITECTURE task flow).
//!
//! Three panels, in priority order:
//! 1. **Roster** — every known session across all three tools, live + orphaned,
//!    including Claude Code's native background sessions discovered via
//!    reconciliation (`claude agents --json`, ADR-0002). Actions: promote to
//!    tab, follow-up dispatch, stop, forget.
//! 2. **Dispatch** — prompt + target tool + cwd (cwd is a property of the task,
//!    not of the app) + guardrail preset (ARCHITECTURE guardrail table).
//! 3. **Broadcast compare** — same prompt to N tools, normalized `AgentEvent`
//!    streams rendered side by side; agy joins only when opted in (quota).

pub struct HomeView;

impl HomeView {
    // TODO(next session): state = Vec<SessionRecord> + dispatch form model.
    // Rendering waits for ADR-0003 spike; keep this module ratatui-free until
    // then so the data model can be unit-tested headlessly.
}
