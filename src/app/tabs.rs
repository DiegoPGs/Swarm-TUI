//! Tab model: `[Home]` plus one tab per live session (ADR-0003).
//!
//! A session tab is a *view onto* a session record + a PTY pane — closing the
//! tab must NOT kill the underlying CLI session unless the user says so
//! (sessions outlive tabs; that is the whole point of the registry, ADR-0002).

/// Identifier of an swarm-tui-local session (registry primary key).
pub type SessionId = u64;

pub enum Tab {
    /// Roster + dispatch + broadcast compare (see `home.rs`).
    Home,
    /// An embedded native CLI running in a PTY pane.
    Session { session_id: SessionId },
}

pub struct Tabs {
    pub items: Vec<Tab>,
    pub active: usize,
}

impl Tabs {
    pub fn new() -> Self {
        Tabs {
            items: vec![Tab::Home],
            active: 0,
        }
    }

    // TODO(next session): next/prev/close/promote(session_id) — "promote" is
    // the registry→tab bridge from ADR-0001 (open an interactive resume of a
    // headless session).
}
