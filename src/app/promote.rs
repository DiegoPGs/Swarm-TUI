//! Promote hardening (ADR-0013 decision 6): one live handle per native
//! session. Enter on a roster row runs the pure [`decide`] function over the
//! record plus a snapshot of what is live right now; the app executes the
//! verdict. Pure so every arm is unit-testable without an `App`, a pane, or
//! a wrapped CLI.

use crate::app::tabs::SessionId;
use crate::core::session::{SessionMode, SessionRecord, SessionStatus};

/// What Enter on a Registered roster row should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromoteAction {
    /// `tabs.promote(session_id)`: focus the open tab, or open a tab onto
    /// the live pane (the pre-existing re-attach behavior — and the guard's
    /// "focus instead of spawning a second process").
    FocusExisting {
        session_id: SessionId,
    },
    /// Raise the stop-and-resume confirm for a running headless dispatch.
    ConfirmStopDispatch {
        session_id: SessionId,
    },
    /// Run the registry→tab resume bridge on this finished headless row.
    ResumeHeadless {
        session_id: SessionId,
    },
    NotPromotable {
        reason: String,
    },
}

/// Where a native session is currently attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attachment {
    Pane,
    Dispatch,
}

/// Snapshot of the live state [`decide`] needs — assembled by the app,
/// consumed here, so the decision itself stays `App`-free.
#[derive(Debug, Default)]
pub struct AttachmentSnapshot {
    /// The selected row itself has an open pane (tabbed or detached).
    pub selected_has_pane: bool,
    /// The selected row itself has a live (not-done) headless dispatch.
    pub selected_has_running_dispatch: bool,
    /// Another session row holds a live attachment on the same native_id.
    pub native_attached_elsewhere: Option<(SessionId, Attachment)>,
    /// The row's tool probed Ok and declares `ResumeSupport::ById`
    /// (caps-driven — `app` never names a tool, ADR-0006).
    pub resume_by_id: bool,
}

/// The one-live-handle rule, as a total function. Arms in priority order;
/// every terminal state maps to exactly one action.
pub fn decide(record: &SessionRecord, snap: &AttachmentSnapshot) -> PromoteAction {
    // 1. A live pane on the row itself: focus it (subsumes the old
    //    detached re-attach, and upgrades "tab already open" from a silent
    //    no-op to a focus).
    if snap.selected_has_pane {
        return PromoteAction::FocusExisting {
            session_id: record.id,
        };
    }

    // 2. A running headless dispatch on the row itself: offer stop+resume
    //    when a resume is actually possible.
    if snap.selected_has_running_dispatch {
        if record.native_id.is_none() {
            return PromoteAction::NotPromotable {
                reason: "still waiting for a native session id".to_string(),
            };
        }
        if !snap.resume_by_id {
            return PromoteAction::NotPromotable {
                reason: "resume by id is not available for this tool".to_string(),
            };
        }
        return PromoteAction::ConfirmStopDispatch {
            session_id: record.id,
        };
    }

    // 3. The native session is attached through some OTHER row: focus that
    //    tab rather than spawning a second process on one native session.
    match snap.native_attached_elsewhere {
        Some((other, Attachment::Pane)) => {
            return PromoteAction::FocusExisting { session_id: other }
        }
        Some((other, Attachment::Dispatch)) => {
            return PromoteAction::NotPromotable {
                reason: format!("native session is busy in headless run #{other}"),
            }
        }
        None => {}
    }

    // 4. Status says Running but nothing is live — a stale row (e.g. the
    //    app crashed mid-run; `mark_orphans` is still out of scope).
    //    Resuming would double-attach or resurrect under a wrong status.
    if record.status == SessionStatus::Running {
        return PromoteAction::NotPromotable {
            reason: "row says running but nothing is attached (stale)".to_string(),
        };
    }
    if record.status == SessionStatus::Orphaned {
        return PromoteAction::NotPromotable {
            reason: "orphaned — the native session is gone".to_string(),
        };
    }

    // 5. Finished rows: the resume bridge covers headless rows only
    //    (ADR-0013 names them verbatim; finished interactive rows are the
    //    recorded follow-up, not silent any more).
    if record.mode == SessionMode::Interactive {
        return PromoteAction::NotPromotable {
            reason: "finished interactive session — reopen it from its own tool for now"
                .to_string(),
        };
    }
    if record.native_id.is_none() {
        return PromoteAction::NotPromotable {
            reason: "no native session id — not promotable yet".to_string(),
        };
    }
    if !snap.resume_by_id {
        return PromoteAction::NotPromotable {
            reason: "resume by id is not available for this tool".to_string(),
        };
    }
    PromoteAction::ResumeHeadless {
        session_id: record.id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn record(mode: SessionMode, status: SessionStatus, native_id: Option<&str>) -> SessionRecord {
        let now = SystemTime::now();
        SessionRecord {
            id: 7,
            tool: "claude-code".to_string(),
            native_id: native_id.map(String::from),
            name: None,
            cwd: PathBuf::from("/tmp"),
            mode,
            status,
            created_at: now,
            updated_at: now,
            cost_usd: None,
            model: None,
            effort: None,
            role: None,
        }
    }

    fn snap() -> AttachmentSnapshot {
        AttachmentSnapshot {
            resume_by_id: true,
            ..AttachmentSnapshot::default()
        }
    }

    #[test]
    fn promote_focuses_when_the_row_has_a_live_pane() {
        let rec = record(SessionMode::Interactive, SessionStatus::Running, None);
        let snap = AttachmentSnapshot {
            selected_has_pane: true,
            ..snap()
        };
        assert_eq!(
            decide(&rec, &snap),
            PromoteAction::FocusExisting { session_id: 7 }
        );
    }

    #[test]
    fn promote_focuses_the_session_holding_the_native_attachment_elsewhere() {
        let rec = record(
            SessionMode::Headless,
            SessionStatus::Completed,
            Some("native-a"),
        );
        let pane_elsewhere = AttachmentSnapshot {
            native_attached_elsewhere: Some((3, Attachment::Pane)),
            ..snap()
        };
        assert_eq!(
            decide(&rec, &pane_elsewhere),
            PromoteAction::FocusExisting { session_id: 3 }
        );

        // A live dispatch elsewhere can't be focused — it has no tab yet.
        let dispatch_elsewhere = AttachmentSnapshot {
            native_attached_elsewhere: Some((3, Attachment::Dispatch)),
            ..snap()
        };
        match decide(&rec, &dispatch_elsewhere) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("busy"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }
    }

    #[test]
    fn promote_confirms_stop_for_a_running_dispatch_with_a_native_id() {
        let rec = record(
            SessionMode::Headless,
            SessionStatus::Running,
            Some("native-a"),
        );
        let snap = AttachmentSnapshot {
            selected_has_running_dispatch: true,
            ..snap()
        };
        assert_eq!(
            decide(&rec, &snap),
            PromoteAction::ConfirmStopDispatch { session_id: 7 }
        );
    }

    #[test]
    fn promote_refuses_a_running_dispatch_without_a_native_id_yet() {
        let rec = record(SessionMode::Headless, SessionStatus::Running, None);
        let snap = AttachmentSnapshot {
            selected_has_running_dispatch: true,
            ..snap()
        };
        match decide(&rec, &snap) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("waiting"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }
    }

    #[test]
    fn promote_resumes_a_finished_headless_row_with_native_id_and_resume_caps() {
        for status in [SessionStatus::Completed, SessionStatus::Failed] {
            let rec = record(SessionMode::Headless, status, Some("native-a"));
            assert_eq!(
                decide(&rec, &snap()),
                PromoteAction::ResumeHeadless { session_id: 7 }
            );
        }
    }

    #[test]
    fn promote_refuses_headless_rows_without_a_native_id() {
        let rec = record(SessionMode::Headless, SessionStatus::Completed, None);
        match decide(&rec, &snap()) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("no native session id"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }
    }

    #[test]
    fn promote_refuses_stale_running_rows_with_no_live_attachment() {
        let rec = record(
            SessionMode::Headless,
            SessionStatus::Running,
            Some("native-a"),
        );
        match decide(&rec, &snap()) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("stale"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }
    }

    #[test]
    fn promote_refuses_when_resume_by_id_is_not_probed() {
        let rec = record(
            SessionMode::Headless,
            SessionStatus::Completed,
            Some("native-a"),
        );
        let no_resume = AttachmentSnapshot::default();
        match decide(&rec, &no_resume) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("resume by id"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }
    }

    #[test]
    fn promote_explains_finished_interactive_rows_are_not_bridged() {
        let rec = record(
            SessionMode::Interactive,
            SessionStatus::Completed,
            Some("native-a"),
        );
        match decide(&rec, &snap()) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("interactive"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }

        // Orphaned rows explain themselves too.
        let rec = record(
            SessionMode::Headless,
            SessionStatus::Orphaned,
            Some("native-a"),
        );
        match decide(&rec, &snap()) {
            PromoteAction::NotPromotable { reason } => {
                assert!(reason.contains("orphaned"), "got: {reason}")
            }
            other => panic!("expected NotPromotable, got {other:?}"),
        }
    }
}
