//! Application layer: terminal setup, event loop, layout, tab switching.
//!
//! Knows CLIs? **Almost never.** This layer speaks `crate::core` vocabulary
//! (sessions, tasks, `AgentEvent`), `crate::pty::PaneHost` surfaces, and the
//! data-only launch/command vocabulary from `crate::adapters` (`AdapterCaps`,
//! `LaunchIntent`, `LaunchOptions`, `NativeCommand` — ADR-0009). The one
//! pre-authorized exception (this stage) is matching on `adapters::AdapterKind`
//! to decide the claude native_id-hint prepopulation in `open_new_session` —
//! `AdapterKind` is already a shared, CLI-agnostic-safe identifier `app` uses
//! elsewhere (roster "tool" column, the new-session picker). If an
//! `if tool == "codex"` string match, or any *flag/behavior* knowledge, ever
//! appears here, the adapter boundary (ADR-0006) has leaked — fix the
//! boundary, not this module.

pub mod home;
pub mod keys;
pub mod reconcile;
pub mod session_view;
pub mod tabs;

use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::adapters::claude_code::ClaudeCode;
use crate::adapters::{self, AdapterCaps, AdapterError, AdapterKind, CliAdapter};
use crate::core::config::SwarmTuiConfig;
use crate::core::session::{SessionMode, SessionRecord, SessionStatus};
use crate::pty::local::LocalPaneHost;
use crate::pty::{PaneHost, PaneId, PaneSize};
use crate::store::Registry;

use home::{HomeView, RosterEntry};
use tabs::{SessionId, Tab, Tabs};

// ---------------------------------------------------------------------------
// Terminal safety
// ---------------------------------------------------------------------------

/// RAII terminal setup/teardown: raw mode + alternate screen. `Drop` reverses
/// both, best-effort — errors in `Drop` can't be propagated, so they're
/// swallowed there (and only there).
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Install a panic hook that restores the terminal before chaining to
/// whatever hook was previously installed, so a panic backtrace prints
/// somewhere sane instead of inside a raw/alt-screen terminal. Must run
/// BEFORE `TerminalGuard::new()`.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        prev(info);
    }));
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum AppError {
    Io(io::Error),
    Store(crate::store::StoreError),
    Pane(crate::pty::PaneError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Io(e) => write!(f, "io error: {e}"),
            AppError::Store(e) => write!(f, "registry error: {e:?}"),
            AppError::Pane(e) => write!(f, "pane error: {e:?}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<io::Error> for AppError {
    fn from(e: io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<crate::store::StoreError> for AppError {
    fn from(e: crate::store::StoreError) -> Self {
        AppError::Store(e)
    }
}

impl From<crate::pty::PaneError> for AppError {
    fn from(e: crate::pty::PaneError) -> Self {
        AppError::Pane(e)
    }
}

// ---------------------------------------------------------------------------
// Input routing state (ADR-0007)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Every key forwards to the active pane (Session tab) or drives
    /// Home-local navigation (Home tab), except Ctrl-Space.
    Normal,
    /// One-shot: the next key dispatches per the ADR-0007 keymap, then
    /// control returns to `Normal`.
    AwaitingCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    CloseActiveTab,
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub struct NewSessionPicker {
    pub selected: usize,
}

fn is_ctrl_space(key: &KeyEvent) -> bool {
    (key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL))
        || key.code == KeyCode::Null
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// Top-level application state.
pub struct App {
    pub tabs: Tabs,
    pub registry: Registry,
    pub pane_host: LocalPaneHost,
    pub pane_of_session: HashMap<SessionId, PaneId>,
    pub home: HomeView,
    pub input_mode: InputMode,
    pub probe_cache: HashMap<AdapterKind, Result<AdapterCaps, AdapterError>>,
    pub pending_confirm: Option<ConfirmAction>,
    pub new_session_picker: Option<NewSessionPicker>,
    pub show_keymap_overlay: bool,
    /// Size newly spawned/live panes are kept at: terminal area minus the
    /// tab bar row. Recomputed every `draw()` call so it tracks real resizes
    /// without needing a dedicated `Event::Resize` handler.
    pane_area_size: PaneSize,
}

/// Query Claude Code's native background-agent supervisor (ADR-0002
/// reconciliation, Stage D) if the claude probe succeeded — no point
/// shelling out to a binary this machine doesn't have. The blocking `claude
/// agents --json --all` subprocess itself runs via `tokio::task::
/// spawn_blocking` so a slow or hung `claude` never stalls the
/// input-handling side of the event loop.
///
/// Free function (not `App::`) so `bootstrap` can call it before an `App`
/// exists yet. Naming `ClaudeCode` here is a narrow, pre-authorized
/// exception to the module doc's "`app` almost never knows CLIs" rule —
/// exactly like the existing `AdapterKind` match for the claude
/// native_id-hint: `list_background_agents` is Claude-Code-specific by
/// design (it's an inherent method, not on `CliAdapter`), and reconciliation
/// has to name it somewhere.
async fn reconciled_claude_agents(
    probe_cache: &HashMap<AdapterKind, Result<AdapterCaps, AdapterError>>,
) -> Vec<RosterEntry> {
    let claude_probe_ok = matches!(probe_cache.get(&AdapterKind::ClaudeCode), Some(Ok(_)));
    if !claude_probe_ok {
        return Vec::new();
    }

    let values = match tokio::task::spawn_blocking(|| ClaudeCode.list_background_agents()).await {
        Ok(Ok(values)) => values,
        Ok(Err(e)) => {
            tracing::warn!("claude agents --json --all failed: {e:?}");
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!("claude agents --json --all: background task join error: {e}");
            return Vec::new();
        }
    };

    // Re-serialize the already shape-normalized `Vec<Value>` so the single
    // per-entry defensive parser (`reconcile::parse_agents_json`, unit
    // tested against a fixture) stays the one place that knows the
    // id/session_id/name/status field-mapping rules.
    let raw = match serde_json::to_string(&values) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!("failed to re-serialize claude agents --json --all output: {e}");
            return Vec::new();
        }
    };

    reconcile::parse_agents_json(&raw)
        .into_iter()
        .map(reconcile::to_roster_entry)
        .collect()
}

impl App {
    /// Probe every adapter (cached for the app's lifetime — probes are
    /// `--version`/`--help` child processes, not free), open the registry,
    /// and load the initial roster — plus, if the claude probe succeeded,
    /// Claude Code's native background agents (Stage D, ADR-0002
    /// reconciliation). Returns the pane-change notification receiver
    /// alongside the app since `LocalPaneHost::new()` hands that back
    /// separately (a handful of tabs doesn't need per-pane channels).
    pub async fn bootstrap(
        config: &SwarmTuiConfig,
    ) -> Result<(App, UnboundedReceiver<PaneId>), AppError> {
        let mut probe_cache = HashMap::new();
        for &kind in adapters::registry() {
            probe_cache.insert(kind, kind.probe());
        }

        let registry = Registry::open(&config.registry_db)?;
        let roster = registry.all()?;
        let mut roster: Vec<RosterEntry> =
            roster.into_iter().map(RosterEntry::Registered).collect();
        roster.extend(reconciled_claude_agents(&probe_cache).await);
        let home = HomeView::new(roster);

        let (pane_host, pane_changed_rx) = LocalPaneHost::new();

        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pane_area_size = PaneSize {
            rows: rows.saturating_sub(1).max(1),
            cols: cols.max(1),
        };

        Ok((
            App {
                tabs: Tabs::new(),
                registry,
                pane_host,
                pane_of_session: HashMap::new(),
                home,
                input_mode: InputMode::Normal,
                probe_cache,
                pending_confirm: None,
                new_session_picker: None,
                show_keymap_overlay: false,
                pane_area_size,
            },
            pane_changed_rx,
        ))
    }

    // -- roster / registry helpers -----------------------------------------

    fn refresh_roster(&mut self) -> Result<(), AppError> {
        let records = self.registry.all()?;
        self.home.roster = records.into_iter().map(RosterEntry::Registered).collect();
        if self.home.selected >= self.home.roster.len() {
            self.home.selected = self.home.roster.len().saturating_sub(1);
        }
        Ok(())
    }

    fn detached_set(&self) -> HashSet<SessionId> {
        self.home
            .roster
            .iter()
            .filter_map(|e| match e {
                RosterEntry::Registered(r) => Some(r.id),
                RosterEntry::ReconciledOnly { .. } => None,
            })
            .filter(|id| home::is_detached(*id, &self.pane_of_session, &self.tabs))
            .collect()
    }

    fn record_for(&self, session_id: SessionId) -> Option<&SessionRecord> {
        self.home.roster.iter().find_map(|e| match e {
            RosterEntry::Registered(r) if r.id == session_id => Some(r),
            _ => None,
        })
    }

    fn tab_titles(&self) -> Vec<String> {
        self.tabs
            .items
            .iter()
            .map(|t| match t {
                Tab::Home => "Home".to_string(),
                Tab::Session { session_id } => match self.record_for(*session_id) {
                    Some(r) => {
                        let label = r
                            .name
                            .clone()
                            .or_else(|| r.native_id.clone())
                            .unwrap_or_else(|| format!("#{}", r.id));
                        format!("{} {label}", r.tool)
                    }
                    None => format!("session #{session_id}"),
                },
            })
            .collect()
    }

    fn status_line_for(&self, session_id: SessionId) -> String {
        const HINT: &str = "Ctrl-Space for commands";
        match self.record_for(session_id) {
            Some(r) => {
                let native = r
                    .native_id
                    .as_deref()
                    .map(|n| format!(" [{n}]"))
                    .unwrap_or_default();
                format!("{}{native}  —  {HINT}", r.tool)
            }
            None => format!("session #{session_id}  —  {HINT}"),
        }
    }

    // -- session lifecycle ---------------------------------------------------

    /// New session (prefix `c`): the app generates the claude session-id
    /// hint UNCONDITIONALLY for every tool — only `claude_code.rs`'s own
    /// `interactive_cmd` match arm decides whether to consume it. This keeps
    /// the ADR-0006 boundary intact: `app` never asks "is this claude?" to
    /// decide whether to generate a hint, only to decide whether the
    /// registry should prepopulate `native_id` with it (see the module doc).
    fn open_new_session(&mut self, kind: AdapterKind) -> Result<(), AppError> {
        let hint = uuid::Uuid::new_v4().to_string();
        let intent = adapters::LaunchIntent::Fresh {
            session_id_hint: Some(hint.clone()),
        };
        // TODO(2b): prompt for cwd instead of always using the launch cwd.
        let cwd = std::env::current_dir()?;
        let cmd = kind.interactive_cmd(&intent, &adapters::LaunchOptions::default(), &cwd);
        let pane_id = self.pane_host.spawn(cmd, self.pane_area_size)?;

        let id = self.registry.allocate_id()?;
        let now = SystemTime::now();
        let record = SessionRecord {
            id,
            tool: kind.id().to_string(),
            native_id: if kind == AdapterKind::ClaudeCode {
                Some(hint.clone())
            } else {
                None
            },
            name: None,
            cwd,
            mode: SessionMode::Interactive,
            status: SessionStatus::Running,
            created_at: now,
            updated_at: now,
            cost_usd: None,
        };
        self.registry.upsert(&record)?;
        self.pane_of_session.insert(record.id, pane_id);
        self.tabs.promote(record.id);
        self.refresh_roster()?;
        Ok(())
    }

    /// Detach (prefix `d`): remove the tab only. The pane, the
    /// `LocalPaneHost` entry, and the registry row are untouched — the
    /// session keeps running invisibly and the roster picks up the
    /// "detached" badge (derived, not stored).
    fn detach_active_tab(&mut self) {
        let active = self.tabs.active;
        if active == 0 {
            return; // Home isn't detachable
        }
        if matches!(self.tabs.items.get(active), Some(Tab::Session { .. })) {
            self.tabs.close(active);
        }
    }

    /// Close (prefix `x`, confirmed): kill the pane, poll briefly for the
    /// captured exit status (it may still be `None` immediately after
    /// `kill()` — capture happens on the reader thread's EOF), then mark the
    /// registry row `Completed`/`Failed` and drop the tab + pane mapping.
    async fn perform_close_active_tab(&mut self) {
        let active = self.tabs.active;
        let session_id = match self.tabs.items.get(active) {
            Some(Tab::Session { session_id }) => *session_id,
            _ => return,
        };

        if let Some(pane_id) = self.pane_of_session.get(&session_id).copied() {
            // A no-op if the pane already exited spontaneously.
            let _ = self.pane_host.kill(pane_id);

            let mut success = self.pane_host.exit_success(pane_id);
            let start = Instant::now();
            while success.is_none() && start.elapsed() < Duration::from_millis(500) {
                tokio::time::sleep(Duration::from_millis(50)).await;
                success = self.pane_host.exit_success(pane_id);
            }
            // Some(false) or still-unknown after the poll window both land
            // on Failed — a killed process rarely exits 0, and "unknown" is
            // never left as Running.
            let status = match success {
                Some(true) => SessionStatus::Completed,
                _ => SessionStatus::Failed,
            };

            if let Ok(mut records) = self.registry.all() {
                if let Some(record) = records.iter_mut().find(|r| r.id == session_id) {
                    record.status = status;
                    record.updated_at = SystemTime::now();
                    let _ = self.registry.upsert(record);
                }
            }
            self.pane_of_session.remove(&session_id);
        }

        self.tabs.close(active);
        let _ = self.refresh_roster();
    }

    // -- input handling -------------------------------------------------------

    /// Returns `true` when the app should quit.
    pub async fn handle_key(&mut self, key: KeyEvent) -> bool {
        if let Some(action) = self.pending_confirm {
            return self.handle_confirm_key(action, key).await;
        }
        if self.new_session_picker.is_some() {
            self.handle_picker_key(key);
            return false;
        }
        if self.show_keymap_overlay {
            // Any key dismisses the overlay; ADR-0007 leaves the exact
            // indicator/dismiss UX to the implementation. Swallow the
            // keypress that closes it so it doesn't also act underneath.
            self.show_keymap_overlay = false;
            return false;
        }

        // Global prefix key, regardless of which tab has focus — Home-local
        // navigation is a separate input scope for *bare* keys (ADR-0007),
        // but Ctrl-Space itself is the one thing reserved everywhere.
        if self.input_mode == InputMode::Normal && is_ctrl_space(&key) {
            self.input_mode = InputMode::AwaitingCommand;
            return false;
        }

        match self.input_mode {
            InputMode::AwaitingCommand => self.handle_command_key(key).await,
            InputMode::Normal => {
                self.handle_normal_key(key);
                false
            }
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match self.tabs.items.get(self.tabs.active) {
            Some(Tab::Home) => self.handle_home_key(key),
            Some(Tab::Session { session_id }) => {
                let session_id = *session_id;
                if let Some(&pane_id) = self.pane_of_session.get(&session_id) {
                    let bytes = keys::encode_key_event(&key);
                    if !bytes.is_empty() {
                        let _ = self.pane_host.write_input(pane_id, &bytes);
                    }
                }
            }
            None => {}
        }
    }

    fn handle_home_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.home.select_prev(),
            KeyCode::Down | KeyCode::Char('j') => self.home.select_next(),
            KeyCode::Enter => {
                if let Some(session_id) = self.home.selected_session_id() {
                    if home::is_detached(session_id, &self.pane_of_session, &self.tabs) {
                        self.tabs.promote(session_id);
                    }
                }
            }
            _ => {}
        }
    }

    /// Dispatch the ADR-0007 keymap. Always returns to `Normal` mode; the
    /// bool return means "quit now" (only true for `q` with no panes alive —
    /// the confirmed path goes through `pending_confirm` instead).
    ///
    /// `async` (Stage D) only because the `r` arm now also reconciles
    /// Claude Code's native background agents, which shells out via
    /// `tokio::task::spawn_blocking`; every other arm is still synchronous.
    async fn handle_command_key(&mut self, key: KeyEvent) -> bool {
        self.input_mode = InputMode::Normal;
        let mut quit_now = false;
        match key.code {
            KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl-Space twice: literal-forward escape hatch for a
                // wrapped tool that itself binds Ctrl-Space (ADR-0007).
                if let Some(Tab::Session { session_id }) = self.tabs.items.get(self.tabs.active) {
                    if let Some(&pane_id) = self.pane_of_session.get(session_id) {
                        let _ = self.pane_host.write_input(pane_id, &[0x00]);
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Char('0') => self.tabs.active = 0,
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let n = c as usize - '0' as usize;
                if n < self.tabs.items.len() {
                    self.tabs.active = n;
                }
            }
            KeyCode::Char('n') => self.tabs.next(),
            KeyCode::Char('p') => self.tabs.prev(),
            KeyCode::Char('c') => {
                self.new_session_picker = Some(NewSessionPicker { selected: 0 });
            }
            KeyCode::Char('d') => self.detach_active_tab(),
            KeyCode::Char('x') => {
                if matches!(
                    self.tabs.items.get(self.tabs.active),
                    Some(Tab::Session { .. })
                ) {
                    self.pending_confirm = Some(ConfirmAction::CloseActiveTab);
                }
            }
            KeyCode::Char('r') => {
                // Registry-backed rows first, then re-run reconciliation
                // (Stage D, ADR-0002) and fold the `ReconciledOnly` rows in
                // additively — never dropping the `Registered` rows
                // `refresh_roster` just rebuilt, never inventing a registry
                // row for a reconciled-only entry.
                let _ = self.refresh_roster();
                let reconciled = reconciled_claude_agents(&self.probe_cache).await;
                self.home.roster.extend(reconciled);
            }
            KeyCode::Char('?') => {
                self.show_keymap_overlay = !self.show_keymap_overlay;
            }
            KeyCode::Char('q') => {
                if self.pane_of_session.is_empty() {
                    quit_now = true;
                } else {
                    self.pending_confirm = Some(ConfirmAction::Quit);
                }
            }
            _ => {}
        }
        quit_now
    }

    async fn handle_confirm_key(&mut self, action: ConfirmAction, key: KeyEvent) -> bool {
        let confirmed = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'));
        let cancelled = matches!(
            key.code,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc
        );
        if !confirmed && !cancelled {
            return false; // keep prompting until the user answers
        }
        self.pending_confirm = None;
        if !confirmed {
            return false;
        }
        match action {
            ConfirmAction::CloseActiveTab => {
                self.perform_close_active_tab().await;
                false
            }
            ConfirmAction::Quit => {
                // Fire-and-forget: a bulk quit-kill doesn't update registry
                // status (out of scope for a clean-shutdown path) — just
                // make sure nothing is left running.
                for &pane_id in self.pane_of_session.values() {
                    let _ = self.pane_host.kill(pane_id);
                }
                true
            }
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let kinds = adapters::registry();
        let len = kinds.len();
        let Some(picker) = self.new_session_picker.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                picker.selected = (picker.selected + len - 1) % len;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.selected = (picker.selected + 1) % len;
            }
            KeyCode::Esc => {
                self.new_session_picker = None;
            }
            KeyCode::Enter => {
                let kind = kinds[picker.selected];
                let installed = matches!(self.probe_cache.get(&kind), Some(Ok(_)));
                if installed {
                    self.new_session_picker = None;
                    let _ = self.open_new_session(kind);
                }
                // Else: a failed-probe ("not installed") entry is greyed and
                // disabled per ARCHITECTURE.md — Enter is a no-op and the
                // picker stays open so the user can pick a different tool.
            }
            _ => {}
        }
    }

    // -- rendering ------------------------------------------------------------

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();

        let new_pane_size = PaneSize {
            rows: area.height.saturating_sub(1).max(1),
            cols: area.width.max(1),
        };
        if new_pane_size.rows != self.pane_area_size.rows
            || new_pane_size.cols != self.pane_area_size.cols
        {
            self.pane_area_size = new_pane_size;
            let pane_ids: Vec<PaneId> = self.pane_of_session.values().copied().collect();
            for pane_id in pane_ids {
                let _ = self.pane_host.resize(pane_id, new_pane_size);
            }
        }

        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
        let (tab_bar_area, body_area) = (chunks[0], chunks[1]);

        self.draw_tab_bar(frame, tab_bar_area);

        match self.tabs.items.get(self.tabs.active) {
            Some(Tab::Session { session_id }) => {
                let session_id = *session_id;
                if let Some(&pane_id) = self.pane_of_session.get(&session_id) {
                    let view = session_view::SessionView::new(session_id, pane_id);
                    let status_line = self.status_line_for(view.session_id);
                    session_view::render(
                        frame,
                        body_area,
                        &self.pane_host,
                        view.pane_id,
                        &status_line,
                    );
                } else {
                    frame.render_widget(
                        Paragraph::new(
                            "session pane not available in this run (closed or not yet attached)",
                        ),
                        body_area,
                    );
                }
            }
            _ => {
                let detached = self.detached_set();
                home::render_home(frame, body_area, &self.home, &detached);
            }
        }

        if self.show_keymap_overlay {
            self.draw_keymap_overlay(frame, area);
        }
        if let Some(picker) = self.new_session_picker {
            self.draw_new_session_picker(frame, area, picker);
        }
        if let Some(action) = self.pending_confirm {
            self.draw_confirm(frame, area, action);
        }

        // ADR-0007 consequence: the one-shot "awaiting command" state needs
        // its own visible indicator so a user mid-keystroke isn't guessing
        // whether the next key goes to the shell or the pane.
        if self.input_mode == InputMode::AwaitingCommand && area.height > 0 {
            let banner_area = Rect {
                x: area.x,
                y: area.y + area.height - 1,
                width: area.width,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(
                    "AWAITING COMMAND — h/0 Home  1-9 jump  n/p cycle  c new  d detach  x close  r refresh  ? help  q quit",
                )
                .style(Style::default().add_modifier(Modifier::REVERSED)),
                banner_area,
            );
        }
    }

    fn draw_tab_bar(&self, frame: &mut Frame, area: Rect) {
        let titles = self.tab_titles();
        let widget = ratatui::widgets::Tabs::new(titles)
            .select(self.tabs.active)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .divider(" | ");
        frame.render_widget(widget, area);
    }

    fn draw_keymap_overlay(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(60, 70, area);
        frame.render_widget(Clear, popup);
        let text = vec![
            Line::from("Ctrl-Space, then:"),
            Line::from("  h / 0   Home"),
            Line::from("  1-9     Jump to tab N"),
            Line::from("  n / p   Next / previous tab"),
            Line::from("  c       New session"),
            Line::from("  d       Detach"),
            Line::from("  x       Close tab (confirm)"),
            Line::from("  r       Refresh roster"),
            Line::from("  ?       Toggle this overlay"),
            Line::from("  q       Quit (confirm if any pane alive)"),
            Line::from(""),
            Line::from("Ctrl-Space twice sends one literal Ctrl-Space to the pane."),
            Line::from(""),
            Line::from("(press any key to close)"),
        ];
        let block = Block::default().borders(Borders::ALL).title("Keymap");
        frame.render_widget(Paragraph::new(text).block(block), popup);
    }

    fn draw_new_session_picker(&self, frame: &mut Frame, area: Rect, picker: NewSessionPicker) {
        let popup = centered_rect(50, 40, area);
        frame.render_widget(Clear, popup);

        let kinds = adapters::registry();
        let items: Vec<ListItem> = kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| {
                let installed = matches!(self.probe_cache.get(kind), Some(Ok(_)));
                let label = if installed {
                    kind.display_name().to_string()
                } else {
                    format!("{} (not installed)", kind.display_name())
                };
                let mut style = if installed {
                    Style::default()
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                if i == picker.selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                ListItem::new(label).style(style)
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .title("New session — Enter to launch, Esc to cancel");
        frame.render_widget(List::new(items).block(block), popup);
    }

    fn draw_confirm(&self, frame: &mut Frame, area: Rect, action: ConfirmAction) {
        let popup = centered_rect(46, 20, area);
        frame.render_widget(Clear, popup);
        let message = match action {
            ConfirmAction::CloseActiveTab => {
                "Close this session? This kills the underlying process. [y/n]"
            }
            ConfirmAction::Quit => "Quit swarm-tui? This kills all running panes. [y/n]",
        };
        let block = Block::default().borders(Borders::ALL).title("Confirm");
        frame.render_widget(Paragraph::new(message).block(block), popup);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Boots the terminal, runs the event loop until quit, and always restores
/// the terminal on the way out (`TerminalGuard`'s `Drop` covers both a
/// normal `break` and a propagated error).
pub async fn run(config: SwarmTuiConfig) -> Result<(), AppError> {
    let (mut app, mut pane_changed_rx) = App::bootstrap(&config).await?;

    install_panic_hook();
    let _guard = TerminalGuard::new()?;

    // Deliberately no explicit `terminal.clear()` here: it round-trips a
    // cursor-position query (DSR `ESC[6n`) through the backend, which some
    // terminals/multiplexers don't answer promptly. It isn't needed either
    // way — `Terminal::new`'s initial "previous" buffer starts blank, so the
    // first `draw()` below already repaints the whole screen from scratch.
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Crossterm's async EventStream needs the `event-stream` cargo feature
    // (not enabled — see AGENTS.md pre-authorized-additions list). A
    // dedicated blocking thread looping on the blocking `crossterm::event::
    // read()` and forwarding into an unbounded channel gets the same
    // practical effect with zero Cargo.toml changes.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if input_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let mut dirty = true;
    let mut tick = tokio::time::interval(Duration::from_millis(33));

    loop {
        tokio::select! {
            Some(ev) = input_rx.recv() => {
                if let Event::Key(key) = ev {
                    if key.kind == KeyEventKind::Press && app.handle_key(key).await {
                        break;
                    }
                }
                dirty = true;
            }
            Some(_pane_id) = pane_changed_rx.recv() => {
                // Coalesce a burst of pane-change notifications (streaming
                // PTY output) into one redraw instead of one per byte-chunk.
                while pane_changed_rx.try_recv().is_ok() {}
                dirty = true;
            }
            _ = tick.tick() => {
                if dirty {
                    terminal.draw(|frame| app.draw(frame))?;
                    dirty = false;
                }
            }
        }
    }

    Ok(())
}
