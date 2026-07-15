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

    /// Cycle to the next tab, wrapping around.
    pub fn next(&mut self) {
        self.active = (self.active + 1) % self.items.len();
    }

    /// Cycle to the previous tab, wrapping around.
    pub fn prev(&mut self) {
        self.active = (self.active + self.items.len() - 1) % self.items.len();
    }

    /// Remove the tab at `index`. Home (index 0) is never removable through
    /// this method — callers (the `x`/detach handlers) should not pass 0,
    /// but this guards it anyway so a stray call can never empty the tab bar
    /// down to nothing. `active` is clamped so it never points past the end.
    pub fn close(&mut self, index: usize) {
        if index == 0 || index >= self.items.len() {
            return;
        }
        self.items.remove(index);
        if self.active >= self.items.len() {
            self.active = self.items.len() - 1;
        }
    }

    /// Open (or refocus) a session tab and switch to it — the registry→tab
    /// bridge (ADR-0001): promoting a headless/detached session back into
    /// view. Reuses an already-open tab for the same session instead of
    /// opening a duplicate.
    pub fn promote(&mut self, session_id: SessionId) {
        if let Some(pos) = self
            .items
            .iter()
            .position(|t| matches!(t, Tab::Session { session_id: sid } if *sid == session_id))
        {
            self.active = pos;
            return;
        }
        self.items.push(Tab::Session { session_id });
        self.active = self.items.len() - 1;
    }
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_and_prev_wrap_around() {
        let mut tabs = Tabs::new();
        tabs.promote(1);
        tabs.promote(2);
        assert_eq!(tabs.active, 2);
        tabs.next();
        assert_eq!(tabs.active, 0);
        tabs.prev();
        assert_eq!(tabs.active, 2);
    }

    #[test]
    fn close_never_removes_home_and_clamps_active() {
        let mut tabs = Tabs::new();
        tabs.promote(1);
        tabs.promote(2);
        assert_eq!(tabs.items.len(), 3);
        tabs.close(0); // no-op: Home is never removable this way
        assert_eq!(tabs.items.len(), 3);
        tabs.close(2);
        assert_eq!(tabs.items.len(), 2);
        assert_eq!(tabs.active, 1);
    }

    #[test]
    fn promote_reuses_an_already_open_tab() {
        let mut tabs = Tabs::new();
        tabs.promote(1);
        tabs.promote(2);
        tabs.active = 0;
        tabs.promote(1);
        assert_eq!(tabs.active, 1);
        assert_eq!(tabs.items.len(), 3);
    }
}
