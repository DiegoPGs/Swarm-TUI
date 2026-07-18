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

pub mod dispatch;
pub mod home;
pub mod keys;
pub mod palette;
pub mod reconcile;
pub mod session_view;
pub mod startup;
pub mod tabs;
pub mod usage;

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
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::adapters::claude_code::ClaudeCode;
use crate::adapters::{self, AdapterCaps, AdapterError, AdapterKind, CliAdapter};
use crate::core::config::SwarmTuiConfig;
use crate::core::events::AgentEvent;
use crate::core::plan::{Role, SwarmPlan};
use crate::core::session::{SessionMode, SessionRecord, SessionStatus};
use crate::pty::local::LocalPaneHost;
use crate::pty::{PaneHost, PaneId, PaneSize};
use crate::store::Registry;

use home::{HomeView, RosterEntry};
use startup::{StartupCommand, StartupQueue};
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
    /// A role startup command matching a `persists: true` command-table entry
    /// wants injecting (ADR-0010) — `y` injects, `n`/Esc skips that entry and
    /// the queue continues. NEW in 2c: ADR-0009 shipped badge-only, the
    /// attended palette still injects without this confirm.
    StartupInjection {
        session_id: SessionId,
    },
}

/// The new-session picker, now two-stage (ADR-0009): choose a tool, then —
/// when its adapter declares launch options — fill a small options form.
/// Tools declaring nothing spawn straight from stage one.
#[derive(Debug, Clone)]
pub enum PickerState {
    ChooseTool {
        selected: usize,
    },
    Options {
        kind: AdapterKind,
        /// Copied out of `probe_cache` when the tool was chosen, so drawing
        /// and key handling never re-consult the cache.
        decl: adapters::LaunchOptionsDecl,
        model: String,
        /// 0 = tool default (send nothing); 1.. indexes `decl.effort`.
        effort_idx: usize,
    },
}

/// Truncate for one-line surfaces (timeline rows), char-boundary safe.
fn truncate_line(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn is_ctrl_space(key: &KeyEvent) -> bool {
    (key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL))
        || key.code == KeyCode::Null
}

/// Map the options-form state to `LaunchOptions`: a field only materializes
/// when the adapter declared it AND the user chose something (empty model
/// text / effort index 0 mean "tool default" → send nothing).
fn build_launch_options(
    model_text: &str,
    effort_idx: usize,
    decl: &adapters::LaunchOptionsDecl,
) -> adapters::LaunchOptions {
    let model = match decl.model {
        Some(_) if !model_text.trim().is_empty() => Some(model_text.trim().to_string()),
        _ => None,
    };
    let effort = decl
        .effort
        .filter(|_| effort_idx > 0)
        .and_then(|levels| levels.get(effort_idx - 1))
        .map(|level| level.to_string());
    adapters::LaunchOptions { model, effort }
}

/// One selectable row of the new-session picker's first stage (ADR-0010):
/// swarm-plan roles list above the raw tools; section headers are rendering
/// artifacts, not items — `PickerState::ChooseTool.selected` indexes this.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickerItem {
    Role {
        name: String,
        kind: AdapterKind,
        /// Prebuilt display line: `name — tool · model/effort — purpose`.
        label: String,
    },
    Tool(AdapterKind),
}

/// Roles (alphabetical — `SwarmPlan.roles` is a `BTreeMap`) above tools
/// (`registry()` order). Pure so the key handler and the renderer can never
/// disagree about indexing.
fn picker_items(plan: Option<&SwarmPlan>) -> Vec<PickerItem> {
    let mut items = Vec::new();
    if let Some(plan) = plan {
        for (name, role) in &plan.roles {
            // Validated as an active slug at load time; a miss here would be
            // a bug, and skipping beats panicking in a UI path.
            let Some(kind) = AdapterKind::from_slug(&role.tool) else {
                continue;
            };
            let launch = match (role.model.as_deref(), role.effort.as_deref()) {
                (Some(m), Some(e)) => format!(" · {m}/{e}"),
                (Some(m), None) => format!(" · {m}"),
                (None, Some(e)) => format!(" · effort {e}"),
                (None, None) => String::new(),
            };
            let purpose = role
                .purpose
                .as_deref()
                .map(|p| format!(" — {p}"))
                .unwrap_or_default();
            let label = format!("{name} — {}{launch}{purpose}", role.tool);
            items.push(PickerItem::Role {
                name: name.clone(),
                kind,
                label,
            });
        }
    }
    items.extend(adapters::registry().iter().copied().map(PickerItem::Tool));
    items
}

/// What a role launch adds on top of a raw-tool launch (bundled so
/// `open_new_session` keeps a small signature).
struct RoleSpawn {
    name: String,
    commands: Vec<StartupCommand>,
}

/// Precompute each startup command's confirm requirement: its first token
/// matches a `command_table()` entry with `persists: true` (ADR-0010). Done
/// here — not in `startup` — so the queue never needs the adapter tables.
fn role_startup_commands(kind: AdapterKind, role: &Role) -> Vec<StartupCommand> {
    role.startup_commands
        .iter()
        .map(|text| {
            let first = text.split_whitespace().next().unwrap_or("");
            let needs_confirm = kind
                .command_table()
                .iter()
                .any(|e| e.name == first && e.persists);
            StartupCommand {
                text: text.clone(),
                needs_confirm,
            }
        })
        .collect()
}

/// Load the swarm plan from the launch cwd — `.swarm/swarm.json` plus the
/// personal `.swarm/swarm.local.json` overlay (ADR-0010, ADR-0012; the merge
/// lives in `core/plan.rs`). Both slug lists come from `adapters` so no tool
/// name is ever spelled here; errors come back as the one-line string the
/// picker renders.
fn load_swarm_plan() -> (Option<SwarmPlan>, Option<String>) {
    let dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => return (None, Some(format!("launch cwd unavailable: {e}"))),
    };
    let active: Vec<&str> = adapters::registry().iter().map(|k| k.id()).collect();
    let known: Vec<&str> = adapters::all_kinds().iter().map(|k| k.id()).collect();
    match SwarmPlan::load(&dir, &active, &known) {
        Ok(plan) => (plan, None),
        Err(e) => (None, Some(e.to_string())),
    }
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
    pub new_session_picker: Option<PickerState>,
    pub palette: Option<palette::PaletteState>,
    pub show_keymap_overlay: bool,
    /// The workspace roles file, when present and valid (ADR-0010); reloaded
    /// with the roster on prefix+`r`.
    pub plan: Option<SwarmPlan>,
    /// One-line load error rendered in the picker (`plan` is `None` then).
    pub plan_error: Option<String>,
    /// In-flight role startup-command injections (ADR-0010).
    pub startup: StartupQueue,
    /// Resources view toggle (prefix+`u`, ADR-0011): renders instead of the
    /// roster in the Home tab's body.
    pub show_resources: bool,
    pub resources_scroll: usize,
    /// Per-vendor last usage capture (ADR-0011).
    pub usage: HashMap<AdapterKind, usage::UsageCapture>,
    /// In-flight hidden usage probes — at most one per vendor; refresh keys
    /// no-op while one runs.
    pub probes: HashMap<AdapterKind, usage::ProbePane>,
    /// Size newly spawned/live panes are kept at: terminal area minus the
    /// tab bar row. Recomputed every `draw()` call so it tracks real resizes
    /// without needing a dedicated `Event::Resize` handler.
    pane_area_size: PaneSize,
    /// Live + finished headless dispatches, keyed by their registry session
    /// id (ADR-0013); events fold in on the tick via `drive_dispatches`.
    pub dispatches: HashMap<SessionId, dispatch::RunningDispatch>,
    /// The Home-local dispatch form (`i`), when open.
    pub dispatch_form: Option<dispatch::DispatchForm>,
    /// Recent dispatch activity one-liners for the Home timeline panel.
    pub timeline: std::collections::VecDeque<String>,
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

        let (plan, plan_error) = load_swarm_plan();

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
                palette: None,
                show_keymap_overlay: false,
                plan,
                plan_error,
                startup: StartupQueue::default(),
                show_resources: false,
                resources_scroll: 0,
                usage: HashMap::new(),
                probes: HashMap::new(),
                pane_area_size,
                dispatches: HashMap::new(),
                dispatch_form: None,
                timeline: std::collections::VecDeque::new(),
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
                let launch = match (r.model.as_deref(), r.effort.as_deref()) {
                    (Some(m), Some(e)) => format!(" · {m}/{e}"),
                    (Some(m), None) => format!(" · {m}"),
                    (None, Some(e)) => format!(" · effort {e}"),
                    (None, None) => String::new(),
                };
                let role = r
                    .role
                    .as_deref()
                    .map(|n| format!(" · role {n}"))
                    .unwrap_or_default();
                let skipped = if self.startup.failed_sessions().contains(&session_id) {
                    " · startup commands skipped"
                } else {
                    ""
                };
                format!("{}{native}{launch}{role}{skipped}  —  {HINT}", r.tool)
            }
            None => format!("session #{session_id}  —  {HINT}"),
        }
    }

    // -- session lifecycle ---------------------------------------------------

    /// New session (prefix `c`, then the picker): the app generates the
    /// claude session-id hint UNCONDITIONALLY for every tool — only
    /// `claude_code.rs`'s own `interactive_cmd` match arm decides whether to
    /// consume it. This keeps the ADR-0006 boundary intact: `app` never asks
    /// "is this claude?" to decide whether to generate a hint, only to decide
    /// whether the registry should prepopulate `native_id` with it (see the
    /// module doc). `opts` are the user's picker choices (ADR-0009); the
    /// adapter maps what it supports and the registry row remembers them.
    fn open_new_session(
        &mut self,
        kind: AdapterKind,
        opts: adapters::LaunchOptions,
        role: Option<RoleSpawn>,
    ) -> Result<(), AppError> {
        let hint = uuid::Uuid::new_v4().to_string();
        let intent = adapters::LaunchIntent::Fresh {
            session_id_hint: Some(hint.clone()),
        };
        // TODO(2b follow-up): prompt for cwd instead of always using the
        // launch cwd.
        let cwd = std::env::current_dir()?;
        let cmd = kind.interactive_cmd(&intent, &opts, &cwd);
        let pane_id = self.pane_host.spawn(cmd, self.pane_area_size)?;

        let now = SystemTime::now();
        let mut record = SessionRecord {
            // Placeholder — the registry assigns the real id on create().
            id: 0,
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
            model: opts.model,
            effort: opts.effort,
            role: role.as_ref().map(|r| r.name.clone()),
        };
        record.id = self.registry.create(&record)?;
        self.pane_of_session.insert(record.id, pane_id);
        self.tabs.promote(record.id);
        if let Some(role) = role {
            // Startup commands wait for first paint; the tick drives them.
            self.startup
                .seed(record.id, pane_id, role.name, role.commands);
        }
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

            // Persist failures stay non-fatal (the tab still closes), but
            // never silently: a dropped write leaves the row stuck Running.
            match self.registry.all() {
                Ok(mut records) => {
                    if let Some(record) = records.iter_mut().find(|r| r.id == session_id) {
                        record.status = status;
                        record.updated_at = SystemTime::now();
                        if let Err(e) = self.registry.upsert(record) {
                            tracing::warn!(
                                session_id,
                                error = ?e,
                                "failed to persist close status; registry row left as-is"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        session_id,
                        error = ?e,
                        "failed to read registry while closing session; status not persisted"
                    );
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
        if self.palette.is_some() {
            self.handle_palette_key(key);
            return false;
        }
        if self.dispatch_form.is_some() {
            self.handle_dispatch_form_key(key);
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
        // Resources view (ADR-0011) takes over Home-local keys while shown.
        if self.show_resources {
            match key.code {
                KeyCode::Esc | KeyCode::Char('u') => self.show_resources = false,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.resources_scroll = self.resources_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.resources_scroll = self.resources_scroll.saturating_add(1);
                }
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let idx = (c as usize - '0' as usize) - 1;
                    if let Some(&kind) = adapters::registry().get(idx) {
                        self.refresh_usage(kind);
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.home.select_prev(),
            KeyCode::Down | KeyCode::Char('j') => self.home.select_next(),
            KeyCode::Char('i') => self.open_dispatch_form(),
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

    /// Home-local `i` (ADR-0013): open the dispatch form, prefilled from the
    /// workspace's `defaults.dispatch` and preselecting `default_role`.
    fn open_dispatch_form(&mut self) {
        let targets = dispatch::targets(self.plan.as_ref(), |kind| {
            matches!(self.probe_cache.get(&kind), Some(Ok(_)))
        });
        if targets.iter().all(|t| !t.enabled) {
            self.push_timeline("✗ dispatch: no installed tool to target".to_string());
            return;
        }
        let budget = crate::core::task::budget_from_workspace(
            self.plan
                .as_ref()
                .and_then(|plan| plan.defaults.dispatch.as_ref()),
        );
        let preselect = self
            .plan
            .as_ref()
            .and_then(|plan| plan.defaults.default_role.clone());
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.dispatch_form = Some(dispatch::DispatchForm::new(
            targets,
            preselect.as_deref(),
            budget,
            cwd,
        ));
    }

    fn handle_dispatch_form_key(&mut self, key: KeyEvent) {
        let Some(mut form) = self.dispatch_form.take() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {} // drop the form
            KeyCode::Enter => match form.submit() {
                Ok((target, task)) => {
                    if self.serial_lane_busy(target.kind) {
                        // ADR-0013 serialized lane (caps-driven, ADR-0002):
                        // the form stays open with the reason.
                        form.error =
                            Some(format!("{}: one headless run at a time", target.kind.id()));
                        self.dispatch_form = Some(form);
                    } else {
                        self.start_dispatch(target, task);
                    }
                }
                Err(msg) => {
                    form.error = Some(msg);
                    self.dispatch_form = Some(form);
                }
            },
            _ => {
                form.handle_key(key);
                self.dispatch_form = Some(form);
            }
        }
    }

    /// Whether `kind` declares `serial_dispatch` (ADR-0013) and already has
    /// a live headless run — the app enforces the one-lane rule without
    /// ever naming a tool (caps-driven, ADR-0006).
    fn serial_lane_busy(&self, kind: AdapterKind) -> bool {
        let serial = matches!(
            self.probe_cache.get(&kind),
            Some(Ok(caps)) if caps.serial_dispatch
        );
        serial && self.dispatches.values().any(|d| d.kind == kind && !d.done)
    }

    fn push_timeline(&mut self, line: String) {
        self.timeline.push_back(line);
        while self.timeline.len() > 200 {
            self.timeline.pop_front();
        }
    }

    /// Spawn one headless dispatch (ADR-0013): adapter builds and runs the
    /// command; the registry gets a Headless/Running session row plus a
    /// `dispatches` row; events fold in on the tick. Failures surface on the
    /// timeline — never a crash.
    fn start_dispatch(&mut self, target: dispatch::Target, task: crate::core::task::Task) {
        let handle = match target.kind.dispatch(&task) {
            Ok(handle) => handle,
            Err(e) => {
                self.push_timeline(format!("✗ {}: dispatch failed: {e:?}", target.kind.id()));
                return;
            }
        };
        let now = SystemTime::now();
        let record = SessionRecord {
            id: 0, // assigned by create()
            tool: target.kind.id().to_string(),
            native_id: None, // learned from the Started event
            name: None,
            cwd: task.cwd.clone(),
            mode: SessionMode::Headless,
            status: SessionStatus::Running,
            created_at: now,
            updated_at: now,
            cost_usd: None,
            model: task.model.clone(),
            effort: task.effort.clone(),
            role: target.role.clone(),
        };
        let session_id = match self.registry.create(&record) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("dispatch registry create failed: {e:?}");
                self.push_timeline(format!("✗ registry error, dispatch aborted: {e:?}"));
                handle.kill.kill();
                return;
            }
        };
        let dispatch_row = match self.registry.record_dispatch(
            session_id,
            target.kind.id(),
            &task.prompt,
            &task.cwd,
        ) {
            Ok(row) => Some(row),
            Err(e) => {
                tracing::warn!("dispatches insert failed: {e:?}");
                None
            }
        };
        self.dispatches.insert(
            session_id,
            dispatch::RunningDispatch {
                handle,
                kind: target.kind,
                done: false,
                dispatch_row,
            },
        );
        let label = target.role.as_deref().unwrap_or(target.kind.id());
        self.push_timeline(format!(
            "▸ #{session_id} {label}: {}",
            truncate_line(&task.prompt, 60)
        ));
        let _ = self.refresh_roster();
    }

    /// Fold pending dispatch events into the registry, timeline, and roster
    /// (ADR-0013). Called from the tick; returns true when anything changed.
    fn drive_dispatches(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;

        // Pass 1: drain channels (mutably borrows the dispatch map only).
        let mut folded: Vec<(SessionId, AgentEvent)> = Vec::new();
        for (&session_id, disp) in self.dispatches.iter_mut() {
            if disp.done {
                continue;
            }
            loop {
                match disp.handle.events.try_recv() {
                    Ok(event) => {
                        if matches!(
                            event,
                            AgentEvent::Completed { .. } | AgentEvent::Failed { .. }
                        ) {
                            disp.done = true;
                        }
                        folded.push((session_id, event));
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if !disp.done {
                            // The reader thread always sends a terminal
                            // event; a bare disconnect means it died.
                            disp.done = true;
                            folded.push((
                                session_id,
                                AgentEvent::Failed {
                                    reason: "event stream ended unexpectedly".to_string(),
                                },
                            ));
                        }
                        break;
                    }
                }
            }
        }

        // Pass 2: apply (borrows registry/timeline freely).
        let changed = !folded.is_empty();
        let mut roster_dirty = false;
        for (session_id, event) in folded {
            match event {
                AgentEvent::Started { native_id } => {
                    if let Some(native_id) = native_id {
                        self.update_session_row(session_id, |record| {
                            record.native_id = Some(native_id.clone());
                        });
                        roster_dirty = true;
                    }
                    self.push_timeline(format!("· #{session_id} started"));
                }
                AgentEvent::AgentText(text) => {
                    let first = text.lines().next().unwrap_or("");
                    self.push_timeline(format!("· #{session_id} {}", truncate_line(first, 70)));
                }
                AgentEvent::ToolActivity(tool) => {
                    self.push_timeline(format!("· #{session_id} [{tool}]"));
                }
                AgentEvent::Completed { result, cost_usd } => {
                    self.update_session_row(session_id, |record| {
                        record.status = SessionStatus::Completed;
                        record.cost_usd = cost_usd;
                    });
                    self.finalize_dispatch_row(session_id, "completed", cost_usd);
                    let cost = cost_usd.map(|c| format!(" (${c:.2})")).unwrap_or_default();
                    self.push_timeline(format!(
                        "✓ #{session_id} completed{cost}: {}",
                        truncate_line(result.lines().next().unwrap_or(""), 60)
                    ));
                    roster_dirty = true;
                }
                AgentEvent::Failed { reason } => {
                    self.update_session_row(session_id, |record| {
                        record.status = SessionStatus::Failed;
                    });
                    self.finalize_dispatch_row(session_id, "failed", None);
                    self.push_timeline(format!(
                        "✗ #{session_id} failed: {}",
                        truncate_line(reason.lines().next().unwrap_or(""), 60)
                    ));
                    roster_dirty = true;
                }
            }
        }
        if roster_dirty {
            let _ = self.refresh_roster();
        }
        changed
    }

    /// Read-modify-upsert one session row; load failures are warned, never
    /// fatal (findings-ledger F-002 stance).
    fn update_session_row(
        &mut self,
        session_id: SessionId,
        apply: impl FnOnce(&mut SessionRecord),
    ) {
        match self.registry.all() {
            Ok(mut records) => {
                if let Some(record) = records.iter_mut().find(|r| r.id == session_id) {
                    apply(record);
                    record.updated_at = SystemTime::now();
                    if let Err(e) = self.registry.upsert(record) {
                        tracing::warn!(session_id, "dispatch status write failed: {e:?}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(session_id, "dispatch registry read failed: {e:?}");
            }
        }
    }

    fn finalize_dispatch_row(&mut self, session_id: SessionId, outcome: &str, cost: Option<f64>) {
        let Some(row) = self
            .dispatches
            .get(&session_id)
            .and_then(|d| d.dispatch_row)
        else {
            return;
        };
        if let Err(e) = self.registry.finalize_dispatch(row, outcome, cost) {
            tracing::warn!(session_id, "dispatch finalize failed: {e:?}");
        }
    }

    /// User-initiated usage refresh (ADR-0011): spawn the hidden probe pane
    /// for `kind` unless one is already in flight or the vendor declares no
    /// usage command. The pane goes into `probes` only — never into
    /// `pane_of_session`, tabs, or the registry.
    fn refresh_usage(&mut self, kind: AdapterKind) {
        if self.probes.contains_key(&kind) {
            return;
        }
        if !matches!(self.probe_cache.get(&kind), Some(Ok(_))) {
            return; // not installed — no block to refresh
        }
        let Some(inject) = usage::probe_command_for(kind) else {
            return;
        };
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        let cmd = kind.interactive_cmd(
            &adapters::LaunchIntent::Fresh {
                session_id_hint: None,
            },
            &adapters::LaunchOptions::default(),
            &cwd,
        );
        match usage::ProbePane::spawn(&mut self.pane_host, cmd, inject) {
            Ok(probe) => {
                self.probes.insert(kind, probe);
            }
            Err(e) => {
                self.usage.insert(
                    kind,
                    usage::UsageCapture::failure(format!("probe spawn failed: {e:?}")),
                );
            }
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
                self.new_session_picker = Some(PickerState::ChooseTool {
                    selected: self.default_picker_selection(),
                });
            }
            KeyCode::Char(':') => self.open_palette(),
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
                // row for a reconciled-only entry. Also reload the swarm
                // plan (ADR-0010): refresh means "re-read the world".
                let _ = self.refresh_roster();
                let reconciled = reconciled_claude_agents(&self.probe_cache).await;
                self.home.roster.extend(reconciled);
                let (plan, plan_error) = load_swarm_plan();
                self.plan = plan;
                self.plan_error = plan_error;
            }
            KeyCode::Char('u') => {
                // Resources view (ADR-0011) lives in the Home tab's body.
                self.tabs.active = 0;
                self.show_resources = !self.show_resources;
            }
            KeyCode::Char('?') => {
                self.show_keymap_overlay = !self.show_keymap_overlay;
            }
            KeyCode::Char('q') => {
                let dispatches_running = self.dispatches.values().any(|d| !d.done);
                if self.pane_of_session.is_empty() && !dispatches_running {
                    // Session panes are gone; make sure no hidden probe pane
                    // outlives the app either (ADR-0011).
                    self.kill_probe_panes();
                    quit_now = true;
                } else {
                    self.pending_confirm = Some(ConfirmAction::Quit);
                }
            }
            _ => {}
        }
        quit_now
    }

    fn kill_probe_panes(&mut self) {
        for (_, probe) in self.probes.drain() {
            let _ = self.pane_host.kill(probe.pane_id());
        }
    }

    /// Open the command palette (prefix `:`, ADR-0009) — session tabs only,
    /// and only when the pane is alive and the tool has a non-empty command
    /// table. Silently a no-op otherwise (matching the other one-shot keys).
    fn open_palette(&mut self) {
        let Some(Tab::Session { session_id }) = self.tabs.items.get(self.tabs.active) else {
            return;
        };
        let session_id = *session_id;
        let Some(&pane_id) = self.pane_of_session.get(&session_id) else {
            return;
        };
        if self.pane_host.is_exited(pane_id) {
            return;
        }
        let Some(kind) = self
            .record_for(session_id)
            .and_then(|r| AdapterKind::from_slug(&r.tool))
        else {
            return;
        };
        let entries = kind.command_table();
        if entries.is_empty() {
            return;
        }
        self.palette = Some(palette::PaletteState::new(
            pane_id,
            kind.display_name(),
            entries,
        ));
    }

    fn handle_palette_key(&mut self, key: KeyEvent) {
        let Some(pal) = self.palette.as_mut() else {
            return;
        };

        // Argument sub-stage: free text for an entry that declared a hint.
        if let Some(args) = pal.args.as_mut() {
            match key.code {
                KeyCode::Esc => pal.args = None,
                KeyCode::Enter => {
                    let entry = &pal.entries[args.command_index];
                    let text = args.text.trim().to_string();
                    let bytes =
                        palette::injection_bytes(entry, (!text.is_empty()).then_some(&*text));
                    let pane_id = pal.pane_id;
                    self.palette = None;
                    let _ = self.pane_host.write_input(pane_id, &bytes);
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    args.text.push(c);
                }
                KeyCode::Backspace => {
                    args.text.pop();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Up => pal.selected = pal.selected.saturating_sub(1),
            KeyCode::Down => {
                let len = palette::filtered_indices(pal.entries, &pal.filter).len();
                if pal.selected + 1 < len {
                    pal.selected += 1;
                }
            }
            KeyCode::Enter => {
                let filtered = palette::filtered_indices(pal.entries, &pal.filter);
                let Some(&entry_index) = filtered.get(pal.selected) else {
                    return; // filter matches nothing — keep the palette open
                };
                let entry = &pal.entries[entry_index];
                if entry.args_hint.is_some() {
                    pal.args = Some(palette::ArgsState {
                        command_index: entry_index,
                        text: String::new(),
                    });
                } else {
                    let bytes = palette::injection_bytes(entry, None);
                    let pane_id = pal.pane_id;
                    self.palette = None;
                    let _ = self.pane_host.write_input(pane_id, &bytes);
                }
            }
            // Letters feed the filter (↑/↓ own navigation); Ctrl-modified
            // chords — including the global prefix — are ignored here.
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                pal.filter.push(c);
                pal.selected = 0;
            }
            KeyCode::Backspace => {
                pal.filter.pop();
                pal.selected = 0;
            }
            _ => {}
        }
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
        match action {
            // Unlike the destructive confirms, a decline here still acts:
            // it skips the pending entry and the queue continues (ADR-0010).
            ConfirmAction::StartupInjection { session_id } => {
                if confirmed {
                    self.startup.approve(session_id);
                } else {
                    self.startup.skip_current(session_id);
                }
                false
            }
            _ if !confirmed => false,
            ConfirmAction::CloseActiveTab => {
                self.perform_close_active_tab().await;
                false
            }
            ConfirmAction::Quit => {
                // Fire-and-forget: a bulk quit-kill doesn't update registry
                // status for PANES (native transcripts persist and
                // reconciliation re-adopts them) — just make sure nothing is
                // left running, hidden probe panes included (ADR-0011).
                for &pane_id in self.pane_of_session.values() {
                    let _ = self.pane_host.kill(pane_id);
                }
                self.kill_probe_panes();
                // Headless dispatches DO get their rows closed (ADR-0013):
                // nothing reconciles them yet, so a bare kill would leave
                // zombie Running rows in the roster forever.
                let running: Vec<SessionId> = self
                    .dispatches
                    .iter()
                    .filter(|(_, d)| !d.done)
                    .map(|(&id, _)| id)
                    .collect();
                for session_id in running {
                    if let Some(disp) = self.dispatches.get_mut(&session_id) {
                        disp.handle.kill.kill();
                        disp.done = true;
                    }
                    self.update_session_row(session_id, |record| {
                        record.status = SessionStatus::Failed;
                    });
                    self.finalize_dispatch_row(session_id, "stopped at quit", None);
                }
                true
            }
        }
    }

    /// Advance the background state machines (role startup injections,
    /// ADR-0010; hidden usage probes, ADR-0011). Called from the event-loop
    /// tick — the machines rate-limit themselves, so every-33ms is fine.
    /// Returns true when anything changed (the caller marks the frame
    /// dirty). Also raises the persists-confirm modal when a queue wants one
    /// and the confirm slot is free.
    fn drive_background(&mut self) -> bool {
        let mut changed = self.startup.drive(&mut self.pane_host);
        if self.pending_confirm.is_none() {
            if let Some(session_id) = self.startup.next_confirm_request() {
                self.pending_confirm = Some(ConfirmAction::StartupInjection { session_id });
                changed = true;
            }
        }

        // Usage probes: capture-or-abort ends with the pane killed and the
        // probe dropped; the result lands in the per-vendor usage slot.
        let mut done: Vec<(AdapterKind, usage::UsageCapture)> = Vec::new();
        for (kind, probe) in self.probes.iter_mut() {
            match probe.drive(&mut self.pane_host) {
                usage::ProbeStep::Idle => {}
                usage::ProbeStep::Changed => changed = true,
                usage::ProbeStep::Finished(capture) => done.push((*kind, capture)),
                usage::ProbeStep::Aborted(msg) => {
                    done.push((*kind, usage::UsageCapture::failure(msg)));
                }
            }
        }
        for (kind, capture) in done {
            if let Some(probe) = self.probes.remove(&kind) {
                let _ = self.pane_host.kill(probe.pane_id());
            }
            self.usage.insert(kind, capture);
            changed = true;
        }

        // Headless dispatch events (ADR-0013).
        changed |= self.drive_dispatches();
        changed
    }

    /// Initial picker cursor: the plan's `defaults.default_role` row when one
    /// is set (ADR-0012), else the top. The name is validated against the
    /// merged roles at load time, so the position lookup only misses if the
    /// role's tool failed `from_slug` — degrade to the top, never panic.
    fn default_picker_selection(&self) -> usize {
        let Some(plan) = self.plan.as_ref() else {
            return 0;
        };
        let Some(want) = plan.defaults.default_role.as_deref() else {
            return 0;
        };
        picker_items(Some(plan))
            .iter()
            .position(|item| matches!(item, PickerItem::Role { name, .. } if name == want))
            .unwrap_or(0)
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let items = picker_items(self.plan.as_ref());
        let len = items.len();
        // Take the state out so the arms below can call &mut self methods;
        // every path either reinstalls an updated state or ends the picker.
        let Some(picker) = self.new_session_picker.take() else {
            return;
        };
        match picker {
            PickerState::ChooseTool { selected } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.new_session_picker = Some(PickerState::ChooseTool {
                        selected: (selected + len - 1) % len,
                    });
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.new_session_picker = Some(PickerState::ChooseTool {
                        selected: (selected + 1) % len,
                    });
                }
                KeyCode::Esc => {}
                KeyCode::Enter => match items.get(selected) {
                    Some(PickerItem::Role { name, kind, .. }) => {
                        let kind = *kind;
                        // Greyed/disabled unless the tool's probe succeeded,
                        // same rule as raw tools below.
                        if !matches!(self.probe_cache.get(&kind), Some(Ok(_))) {
                            self.new_session_picker = Some(PickerState::ChooseTool { selected });
                        } else if let Some(role) =
                            self.plan.as_ref().and_then(|p| p.roles.get(name)).cloned()
                        {
                            // The role IS the preset — no options form
                            // (ADR-0010); its model/effort pass verbatim.
                            let opts = adapters::LaunchOptions {
                                model: role.model.clone(),
                                effort: role.effort.clone(),
                            };
                            let spawn = RoleSpawn {
                                name: name.clone(),
                                commands: role_startup_commands(kind, &role),
                            };
                            let _ = self.open_new_session(kind, opts, Some(spawn));
                        }
                    }
                    Some(PickerItem::Tool(kind)) => {
                        let kind = *kind;
                        match self.probe_cache.get(&kind) {
                            Some(Ok(caps)) if caps.launch.any() => {
                                // Tool declares launch options → options form.
                                self.new_session_picker = Some(PickerState::Options {
                                    kind,
                                    decl: caps.launch,
                                    model: String::new(),
                                    effort_idx: 0,
                                });
                            }
                            Some(Ok(_)) => {
                                let _ = self.open_new_session(
                                    kind,
                                    adapters::LaunchOptions::default(),
                                    None,
                                );
                            }
                            _ => {
                                // Failed-probe ("not installed") entries are greyed
                                // and disabled per ARCHITECTURE.md — Enter is a
                                // no-op and the picker stays open.
                                self.new_session_picker =
                                    Some(PickerState::ChooseTool { selected });
                            }
                        }
                    }
                    None => {
                        self.new_session_picker = Some(PickerState::ChooseTool { selected });
                    }
                },
                _ => {
                    self.new_session_picker = Some(PickerState::ChooseTool { selected });
                }
            },
            PickerState::Options {
                kind,
                decl,
                mut model,
                mut effort_idx,
            } => match key.code {
                KeyCode::Esc => {
                    // Back to the first stage, cursor restored onto this
                    // tool's row (roles list above the tools).
                    let selected = items
                        .iter()
                        .position(|item| matches!(item, PickerItem::Tool(k) if *k == kind))
                        .unwrap_or(0);
                    self.new_session_picker = Some(PickerState::ChooseTool { selected });
                }
                KeyCode::Enter => {
                    let opts = build_launch_options(&model, effort_idx, &decl);
                    let _ = self.open_new_session(kind, opts, None);
                }
                KeyCode::Char(c) if decl.model.is_some() => {
                    model.push(c);
                    self.new_session_picker = Some(PickerState::Options {
                        kind,
                        decl,
                        model,
                        effort_idx,
                    });
                }
                KeyCode::Backspace if decl.model.is_some() => {
                    model.pop();
                    self.new_session_picker = Some(PickerState::Options {
                        kind,
                        decl,
                        model,
                        effort_idx,
                    });
                }
                KeyCode::Left | KeyCode::Right => {
                    if let Some(levels) = decl.effort {
                        let states = levels.len() + 1; // index 0 = tool default
                        effort_idx = if key.code == KeyCode::Right {
                            (effort_idx + 1) % states
                        } else {
                            (effort_idx + states - 1) % states
                        };
                    }
                    self.new_session_picker = Some(PickerState::Options {
                        kind,
                        decl,
                        model,
                        effort_idx,
                    });
                }
                _ => {
                    self.new_session_picker = Some(PickerState::Options {
                        kind,
                        decl,
                        model,
                        effort_idx,
                    });
                }
            },
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
                if self.show_resources {
                    usage::render_resources(
                        frame,
                        body_area,
                        self.plan.as_ref(),
                        &self.usage,
                        &self.probes,
                        self.resources_scroll,
                    );
                } else {
                    let detached = self.detached_set();
                    if self.timeline.is_empty() {
                        home::render_home(frame, body_area, &self.home, &detached);
                    } else {
                        // Roster on top, recent dispatch activity below
                        // (ADR-0013 timeline panel).
                        let rows = (self.timeline.len() as u16 + 2).min(10);
                        let split =
                            Layout::vertical([Constraint::Min(5), Constraint::Length(rows)])
                                .split(body_area);
                        home::render_home(frame, split[0], &self.home, &detached);
                        dispatch::render_timeline(frame, split[1], &self.timeline);
                    }
                }
            }
        }

        if self.show_keymap_overlay {
            self.draw_keymap_overlay(frame, area);
        }
        if let Some(picker) = &self.new_session_picker {
            self.draw_new_session_picker(frame, area, picker);
        }
        if let Some(pal) = &self.palette {
            self.draw_palette(frame, area, pal);
        }
        if let Some(form) = &self.dispatch_form {
            dispatch::render_form(frame, area, form);
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
                    "AWAITING COMMAND — h/0 Home  1-9 jump  n/p cycle  c new  : palette  d detach  x close  r refresh  u usage  ? help  q quit",
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
            Line::from("  :       Command palette (session tab)"),
            Line::from("  d       Detach"),
            Line::from("  x       Close tab (confirm)"),
            Line::from("  r       Refresh roster + swarm plan"),
            Line::from("  u       Resources view (usage per vendor)"),
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

    fn draw_new_session_picker(&self, frame: &mut Frame, area: Rect, picker: &PickerState) {
        match picker {
            PickerState::ChooseTool { selected } => {
                let popup = centered_rect(56, 50, area);
                frame.render_widget(Clear, popup);

                // Selection indexes `picker_items()`; the headers and the
                // plan-error line below are rendering-only rows.
                let items = picker_items(self.plan.as_ref());
                let has_roles = items
                    .iter()
                    .any(|item| matches!(item, PickerItem::Role { .. }));

                let header_style = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD);
                let mut rows: Vec<ListItem> = Vec::new();
                if let Some(err) = &self.plan_error {
                    rows.push(
                        ListItem::new(Line::from(err.clone()))
                            .style(Style::default().fg(Color::Red)),
                    );
                }
                if has_roles {
                    rows.push(
                        ListItem::new(format!("Roles — {}", crate::core::plan::PLAN_RELATIVE_PATH))
                            .style(header_style),
                    );
                }
                for (i, item) in items.iter().enumerate() {
                    if has_roles && matches!(item, PickerItem::Tool(_)) {
                        let first_tool = items
                            .iter()
                            .position(|it| matches!(it, PickerItem::Tool(_)));
                        if first_tool == Some(i) {
                            rows.push(ListItem::new("Tools").style(header_style));
                        }
                    }
                    let (kind, label) = match item {
                        PickerItem::Role { kind, label, .. } => (kind, format!("  {label}")),
                        PickerItem::Tool(kind) => {
                            let indent = if has_roles { "  " } else { "" };
                            (kind, format!("{indent}{}", kind.display_name()))
                        }
                    };
                    let installed = matches!(self.probe_cache.get(kind), Some(Ok(_)));
                    let label = if installed {
                        label
                    } else {
                        format!("{label} (not installed)")
                    };
                    let mut style = if installed {
                        Style::default()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    if i == *selected {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    rows.push(ListItem::new(label).style(style));
                }

                let block = Block::default()
                    .borders(Borders::ALL)
                    .title("New session — Enter to select, Esc to cancel");
                frame.render_widget(List::new(rows).block(block), popup);
            }
            PickerState::Options {
                kind,
                decl,
                model,
                effort_idx,
            } => {
                let popup = centered_rect(60, 34, area);
                frame.render_widget(Clear, popup);

                let mut lines: Vec<Line> = Vec::new();
                if let Some(suggestions) = decl.model {
                    lines.push(Line::from(format!("Model:  {model}▏")));
                    let hint = if suggestions.is_empty() {
                        "free text — empty keeps the tool's default".to_string()
                    } else {
                        format!(
                            "free text — e.g. {}; empty keeps the default",
                            suggestions.join(", ")
                        )
                    };
                    lines.push(Line::from(format!("        ({hint})")));
                    lines.push(Line::from(""));
                }
                if let Some(levels) = decl.effort {
                    let shown = if *effort_idx == 0 {
                        "(default)"
                    } else {
                        levels[*effort_idx - 1]
                    };
                    lines.push(Line::from(format!("Effort: ◀ {shown} ▶")));
                    lines.push(Line::from(""));
                }
                lines.push(Line::from("Enter launch · ←/→ effort · Esc back"));

                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!("New session — {}", kind.display_name()));
                frame.render_widget(Paragraph::new(lines).block(block), popup);
            }
        }
    }

    fn draw_palette(&self, frame: &mut Frame, area: Rect, pal: &palette::PaletteState) {
        let popup = centered_rect(64, 60, area);
        frame.render_widget(Clear, popup);

        // Argument sub-stage: one entry, one free-text line, the tool's hint.
        if let Some(args) = &pal.args {
            let entry = &pal.entries[args.command_index];
            let lines = vec![
                Line::from(format!("{} {}▏", entry.inject, args.text)),
                Line::from(""),
                Line::from(format!("({})", entry.args_hint.unwrap_or(""))),
                Line::from(""),
                Line::from("Enter inject · empty = bare command · Esc back"),
            ];
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!("{} — arguments", entry.name));
            frame.render_widget(Paragraph::new(lines).block(block), popup);
            return;
        }

        let filtered = palette::filtered_indices(pal.entries, &pal.filter);
        let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(format!(
            "› {}▏   (type to filter · ↑/↓ · Enter · Esc)",
            pal.filter
        )))];
        items.extend(filtered.iter().enumerate().map(|(row, &entry_index)| {
            let entry = &pal.entries[entry_index];
            let mut spans = vec![
                Span::styled(
                    format!("{:<14}", entry.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(entry.description),
            ];
            if entry.persists {
                spans.push(Span::styled(
                    "  [persists]",
                    Style::default().fg(Color::Yellow),
                ));
            }
            let mut style = Style::default();
            if row == pal.selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(Line::from(spans)).style(style)
        }));
        if filtered.is_empty() {
            items.push(ListItem::new("  (no matching command)"));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Commands — {}", pal.tool_display));
        frame.render_widget(List::new(items).block(block), popup);
    }

    fn draw_confirm(&self, frame: &mut Frame, area: Rect, action: ConfirmAction) {
        let popup = centered_rect(46, 20, area);
        frame.render_widget(Clear, popup);
        let message = match action {
            ConfirmAction::CloseActiveTab => {
                "Close this session? This kills the underlying process. [y/n]".to_string()
            }
            ConfirmAction::Quit => {
                "Quit swarm-tui? This kills all running panes. [y/n]".to_string()
            }
            ConfirmAction::StartupInjection { session_id } => self
                .startup
                .confirm_prompt(session_id)
                .map(|p| format!("{p} [y/n = skip]"))
                .unwrap_or_else(|| "Inject startup command? [y/n = skip]".to_string()),
        };
        let block = Block::default().borders(Borders::ALL).title("Confirm");
        frame.render_widget(
            Paragraph::new(message)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(block),
            popup,
        );
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
                if app.drive_background() {
                    dirty = true;
                }
                if dirty {
                    terminal.draw(|frame| app.draw(frame))?;
                    dirty = false;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::LaunchOptionsDecl;
    use crate::core::plan::Defaults;

    #[test]
    fn build_launch_options_maps_empty_fields_to_none() {
        const LEVELS: &[&str] = &["low", "high"];
        let decl = LaunchOptionsDecl {
            model: Some(&[]),
            effort: Some(LEVELS),
        };
        // Nothing chosen → nothing sent.
        let opts = build_launch_options("", 0, &decl);
        assert_eq!(opts, adapters::LaunchOptions::default());
        // Whitespace is not a model.
        assert_eq!(build_launch_options("   ", 0, &decl).model, None);
        // Chosen values map through (effort_idx is 1-based over the decl).
        let opts = build_launch_options("  opus ", 2, &decl);
        assert_eq!(opts.model.as_deref(), Some("opus"));
        assert_eq!(opts.effort.as_deref(), Some("high"));
        // Undeclared fields never materialize, whatever the form held.
        let opts = build_launch_options("opus", 1, &LaunchOptionsDecl::NONE);
        assert_eq!(opts, adapters::LaunchOptions::default());
    }

    #[test]
    fn picker_items_lists_roles_alphabetically_above_tools() {
        use std::collections::BTreeMap;

        let mk = |tool: &str| Role {
            tool: tool.to_string(),
            model: None,
            effort: None,
            purpose: None,
            startup_commands: vec![],
        };
        let mut roles = BTreeMap::new();
        roles.insert("researcher".to_string(), mk("antigravity"));
        roles.insert("advisor".to_string(), mk("claude-code"));
        roles.insert("coder".to_string(), mk("claude-code"));
        let plan = SwarmPlan {
            roles,
            defaults: Defaults::default(),
        };

        let names: Vec<String> = picker_items(Some(&plan))
            .iter()
            .map(|item| match item {
                PickerItem::Role { name, .. } => format!("role:{name}"),
                PickerItem::Tool(kind) => format!("tool:{}", kind.id()),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "role:advisor",
                "role:coder",
                "role:researcher",
                "tool:claude-code",
                "tool:antigravity",
            ]
        );

        // No plan → exactly today's picker: active tools, registry order.
        let items = picker_items(None);
        assert_eq!(items.len(), adapters::registry().len());
        assert!(matches!(
            items[0],
            PickerItem::Tool(AdapterKind::ClaudeCode)
        ));
    }

    #[test]
    fn picker_preselects_the_default_role_row() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");
        let mk = |tool: &str| Role {
            tool: tool.to_string(),
            model: None,
            effort: None,
            purpose: None,
            startup_commands: vec![],
        };
        let mut roles = std::collections::BTreeMap::new();
        roles.insert("advisor".to_string(), mk("claude-code"));
        roles.insert("coder".to_string(), mk("claude-code"));
        roles.insert("researcher".to_string(), mk("antigravity"));

        // No plan → the cursor starts at the top.
        assert_eq!(app.default_picker_selection(), 0);

        // A plan without a default_role → still the top.
        app.plan = Some(SwarmPlan {
            roles: roles.clone(),
            defaults: Defaults::default(),
        });
        assert_eq!(app.default_picker_selection(), 0);

        // default_role → that role's row (alphabetical: advisor, coder,
        // researcher — so "coder" is index 1).
        app.plan = Some(SwarmPlan {
            roles,
            defaults: Defaults {
                default_role: Some("coder".to_string()),
                ..Defaults::default()
            },
        });
        assert_eq!(app.default_picker_selection(), 1);
    }

    #[test]
    fn drive_dispatches_folds_events_into_registry_and_timeline() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");
        // A headless registry row, exactly as start_dispatch would create it.
        let now = SystemTime::now();
        let record = SessionRecord {
            id: 0,
            tool: "claude-code".to_string(),
            native_id: None,
            name: None,
            cwd: std::path::PathBuf::from("/tmp"),
            mode: SessionMode::Headless,
            status: SessionStatus::Running,
            created_at: now,
            updated_at: now,
            cost_usd: None,
            model: None,
            effort: None,
            role: Some("coder".to_string()),
        };
        let sid = app.registry.create(&record).expect("create headless row");
        let row = app
            .registry
            .record_dispatch(sid, "claude-code", "review", std::path::Path::new("/tmp"))
            .expect("dispatch row");
        let (tx, rx) = std::sync::mpsc::channel();
        app.dispatches.insert(
            sid,
            dispatch::RunningDispatch {
                handle: adapters::test_dispatch_handle(rx),
                kind: AdapterKind::ClaudeCode,
                done: false,
                dispatch_row: Some(row),
            },
        );

        tx.send(AgentEvent::Started {
            native_id: Some("native-xyz".to_string()),
        })
        .unwrap();
        tx.send(AgentEvent::AgentText("thinking about it".to_string()))
            .unwrap();
        tx.send(AgentEvent::ToolActivity("Read".to_string()))
            .unwrap();
        assert!(app.drive_dispatches(), "pending events must mark dirty");

        tx.send(AgentEvent::Completed {
            result: "done".to_string(),
            cost_usd: Some(0.02),
        })
        .unwrap();
        drop(tx);
        assert!(app.drive_dispatches());

        let rec = app
            .registry
            .all()
            .expect("read registry")
            .into_iter()
            .find(|r| r.id == sid)
            .expect("row survives");
        assert_eq!(rec.native_id.as_deref(), Some("native-xyz"));
        assert_eq!(rec.status, SessionStatus::Completed);
        assert_eq!(rec.cost_usd, Some(0.02));
        let history = app.registry.dispatch_history(5).expect("history");
        assert!(history[0].finished);
        assert_eq!(history[0].outcome.as_deref(), Some("completed"));
        assert!(app.dispatches[&sid].done);
        assert!(app.timeline.iter().any(|l| l.contains("completed")));
        // A finished dispatch is inert: nothing new to fold.
        assert!(!app.drive_dispatches());
    }

    #[test]
    fn serial_dispatch_lane_blocks_a_second_run_of_that_tool_only() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");
        app.probe_cache.insert(
            AdapterKind::Antigravity,
            Ok(crate::adapters::antigravity::EXPECTED_CAPS),
        );
        app.probe_cache.insert(
            AdapterKind::ClaudeCode,
            Ok(crate::adapters::claude_code::EXPECTED_CAPS),
        );

        // No running dispatches → both lanes free.
        assert!(!app.serial_lane_busy(AdapterKind::Antigravity));
        assert!(!app.serial_lane_busy(AdapterKind::ClaudeCode));

        // One live agy dispatch → the agy lane is busy; claude (which does
        // not declare serial_dispatch) never is.
        let (_tx, rx) = std::sync::mpsc::channel();
        app.dispatches.insert(
            42,
            dispatch::RunningDispatch {
                handle: adapters::test_dispatch_handle(rx),
                kind: AdapterKind::Antigravity,
                done: false,
                dispatch_row: None,
            },
        );
        assert!(app.serial_lane_busy(AdapterKind::Antigravity));
        assert!(!app.serial_lane_busy(AdapterKind::ClaudeCode));

        // A finished run frees the lane.
        app.dispatches.get_mut(&42).unwrap().done = true;
        assert!(!app.serial_lane_busy(AdapterKind::Antigravity));
    }

    #[test]
    fn open_dispatch_form_requires_an_installed_tool() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");
        // Empty probe cache → every target greyed → the form refuses to open.
        app.open_dispatch_form();
        assert!(app.dispatch_form.is_none());
        assert!(app.timeline.iter().any(|l| l.contains("no installed tool")));

        app.probe_cache.insert(
            AdapterKind::ClaudeCode,
            Ok(crate::adapters::claude_code::EXPECTED_CAPS),
        );
        app.open_dispatch_form();
        assert!(app.dispatch_form.is_some());
    }

    // -- palette wiring (fake `sh -c cat` panes only — no wrapped CLI) -------

    fn test_record(id: u64, tool: &str) -> SessionRecord {
        SessionRecord {
            id,
            tool: tool.to_string(),
            native_id: None,
            name: None,
            cwd: std::path::PathBuf::from("/tmp"),
            mode: SessionMode::Interactive,
            status: SessionStatus::Running,
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            cost_usd: None,
            model: None,
            effort: None,
            role: None,
        }
    }

    /// An `App` with one registered session backed by a fake `sh -c cat`
    /// pane. Session id 1; the session tab is NOT promoted (tests do that).
    fn app_with_cat_pane(tool: &str) -> (App, tempfile::TempDir) {
        app_with_pane(tool, "cat")
    }

    /// Like `app_with_cat_pane`, but with a chosen `sh -c` script — startup
    /// tests need a pane that paints something before echoing (`echo ready;
    /// cat`), since the queue waits for a stable non-blank paint.
    fn app_with_pane(tool: &str, script: &str) -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = Registry::open(&tmp.path().join("registry.db")).expect("open registry");
        // Dropping the change-notification receiver is fine: reader threads
        // send best-effort (`let _ =`) and nothing here polls redraws.
        let (mut pane_host, _pane_changed_rx) = LocalPaneHost::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        let pane_id = pane_host
            .spawn(cmd, PaneSize { rows: 24, cols: 80 })
            .expect("spawn fake pane");

        let record = test_record(1, tool);
        let app = App {
            tabs: Tabs::new(),
            registry,
            pane_host,
            pane_of_session: HashMap::from([(1, pane_id)]),
            home: HomeView::new(vec![RosterEntry::Registered(record)]),
            input_mode: InputMode::Normal,
            probe_cache: HashMap::new(),
            pending_confirm: None,
            new_session_picker: None,
            palette: None,
            show_keymap_overlay: false,
            plan: None,
            plan_error: None,
            startup: StartupQueue::default(),
            show_resources: false,
            resources_scroll: 0,
            usage: HashMap::new(),
            probes: HashMap::new(),
            pane_area_size: PaneSize { rows: 24, cols: 80 },
            dispatches: HashMap::new(),
            dispatch_form: None,
            timeline: std::collections::VecDeque::new(),
        };
        (app, tmp)
    }

    #[test]
    fn colon_opens_palette_only_for_live_session_tab() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");

        // Home tab active → no-op.
        app.open_palette();
        assert!(app.palette.is_none());

        // Session tab with a live pane and a known tool → opens.
        app.tabs.promote(1);
        app.open_palette();
        let pal = app.palette.as_ref().expect("palette should open");
        assert_eq!(
            pal.entries.len(),
            AdapterKind::ClaudeCode.command_table().len()
        );
        app.palette = None;

        // Unknown tool slug → no-op (from_slug fails; nothing to list).
        let (mut alien, _tmp2) = app_with_cat_pane("mystery-tool");
        alien.tabs.promote(1);
        alien.open_palette();
        assert!(alien.palette.is_none());

        // Exited pane → no-op.
        let pane_id = app.pane_of_session[&1];
        app.pane_host.kill(pane_id).expect("kill fake pane");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.pane_host.is_exited(pane_id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        app.open_palette();
        assert!(app.palette.is_none());
    }

    #[tokio::test]
    async fn palette_enter_injects_the_selected_command_into_the_pane() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");
        app.tabs.promote(1);

        let press = |code: KeyCode, mods: KeyModifiers| KeyEvent::new(code, mods);

        // Prefix, then ':' opens the palette on the session tab.
        app.handle_key(press(KeyCode::Char(' '), KeyModifiers::CONTROL))
            .await;
        app.handle_key(press(KeyCode::Char(':'), KeyModifiers::NONE))
            .await;
        assert!(app.palette.is_some());

        // Filter down to /status (no args_hint → Enter injects directly).
        for c in "status".chars() {
            app.handle_key(press(KeyCode::Char(c), KeyModifiers::NONE))
                .await;
        }
        app.handle_key(press(KeyCode::Enter, KeyModifiers::NONE))
            .await;
        assert!(app.palette.is_none(), "palette closes after injecting");

        // The fake pane must have received and echoed "/status" + CR.
        let pane_id = app.pane_of_session[&1];
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen = String::new();
        while Instant::now() < deadline {
            seen = app
                .pane_host
                .with_screen(pane_id, |s| s.contents())
                .expect("screen");
            if seen.contains("/status") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            seen.contains("/status"),
            "injected command never reached the pane; screen:\n{seen}"
        );
    }

    // -- close-path persistence failures (F-002) ------------------------------

    /// Collects everything the tracing subscriber writes, so tests can
    /// assert the close path warns instead of failing silently.
    #[derive(Clone, Default)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    fn capture_warnings() -> (CaptureWriter, tracing::subscriber::DefaultGuard) {
        let writer = CaptureWriter::default();
        let sink = writer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (writer, guard)
    }

    #[tokio::test]
    async fn close_warns_when_the_status_write_fails() {
        let (mut app, tmp) = app_with_cat_pane("claude-code");
        app.registry
            .create(&test_record(0, "claude-code"))
            .expect("seed session row");
        // Simulate a registry that can be read but not written (e.g. the
        // file went read-only after open): block UPDATEs at the SQL layer.
        let raw = rusqlite::Connection::open(tmp.path().join("registry.db")).expect("raw open");
        raw.execute_batch(
            "CREATE TRIGGER block_updates BEFORE UPDATE ON sessions
             BEGIN SELECT RAISE(ABORT, 'registry is read-only'); END;",
        )
        .expect("install write-blocking trigger");

        app.tabs.promote(1);
        let (log, _guard) = capture_warnings();
        app.perform_close_active_tab().await;

        let out = log.contents();
        assert!(
            out.contains("failed to persist close status"),
            "expected a warning about the dropped status write, got:\n{out}"
        );
        assert!(
            out.contains("session_id=1"),
            "warning names the session:\n{out}"
        );
        // Non-fatal: the tab is gone even though the write failed.
        assert_eq!(app.tabs.items.len(), 1);
    }

    #[tokio::test]
    async fn close_warns_when_the_registry_read_fails() {
        let (mut app, tmp) = app_with_cat_pane("claude-code");
        let raw = rusqlite::Connection::open(tmp.path().join("registry.db")).expect("raw open");
        raw.execute_batch("DROP TABLE sessions;")
            .expect("break the registry read");

        app.tabs.promote(1);
        let (log, _guard) = capture_warnings();
        app.perform_close_active_tab().await;

        let out = log.contents();
        assert!(
            out.contains("failed to read registry while closing session"),
            "expected a warning about the failed roster read, got:\n{out}"
        );
        assert_eq!(app.tabs.items.len(), 1);
    }

    // -- roles wiring (ADR-0010; fake panes only — no wrapped CLI) -----------

    #[test]
    fn role_startup_commands_flags_persists_entries() {
        let role = Role {
            tool: "claude-code".to_string(),
            model: None,
            effort: None,
            purpose: None,
            startup_commands: vec![
                "/model opus".to_string(),     // in the table, persists → confirm
                "/status".to_string(),         // in the table, transient
                "/advisor fable".to_string(),  // in the table, transient
                "/not-a-real-cmd".to_string(), // absent from the table
            ],
        };
        let flags: Vec<bool> = role_startup_commands(AdapterKind::ClaudeCode, &role)
            .iter()
            .map(|c| c.needs_confirm)
            .collect();
        assert_eq!(flags, vec![true, false, false, false]);
    }

    #[test]
    fn enter_on_role_with_failed_probe_is_a_noop() {
        let (mut app, _tmp) = app_with_cat_pane("claude-code");
        // A plan with one role, but an empty probe cache — the role renders
        // greyed and Enter must neither spawn nor close the picker.
        let mut roles = std::collections::BTreeMap::new();
        roles.insert(
            "advisor".to_string(),
            Role {
                tool: "claude-code".to_string(),
                model: Some("sonnet-5".to_string()),
                effort: None,
                purpose: None,
                startup_commands: vec!["/advisor fable".to_string()],
            },
        );
        app.plan = Some(SwarmPlan {
            roles,
            defaults: Defaults::default(),
        });
        app.new_session_picker = Some(PickerState::ChooseTool { selected: 0 });

        app.handle_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            matches!(
                app.new_session_picker,
                Some(PickerState::ChooseTool { selected: 0 })
            ),
            "picker must stay open on the greyed role"
        );
        assert!(
            app.registry.all().expect("registry").is_empty(),
            "nothing may be registered from a no-op Enter"
        );
    }

    /// The full modal wiring: drive_background raises the persists confirm,
    /// `n` through the real key path skips that entry, and the queue then
    /// continues with the next command into the fake pane.
    #[tokio::test]
    async fn startup_confirm_flows_through_the_app_modal() {
        let (mut app, _tmp) = app_with_pane("claude-code", "echo ready; cat");
        let pane_id = app.pane_of_session[&1];
        app.startup.seed(
            1,
            pane_id,
            "coder".to_string(),
            vec![
                StartupCommand {
                    text: "/model opus".to_string(),
                    needs_confirm: true,
                },
                StartupCommand {
                    text: "/plain after".to_string(),
                    needs_confirm: false,
                },
            ],
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        while app.pending_confirm.is_none() && Instant::now() < deadline {
            app.drive_background();
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
        assert_eq!(
            app.pending_confirm,
            Some(ConfirmAction::StartupInjection { session_id: 1 })
        );

        // Decline via the real key path — skips the entry, queue continues.
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .await;
        assert!(app.pending_confirm.is_none());

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        while Instant::now() < deadline {
            app.drive_background();
            seen = app
                .pane_host
                .with_screen(pane_id, |s| s.contents())
                .expect("screen");
            if seen.contains("/plain after") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
        assert!(
            seen.contains("/plain after"),
            "queue never continued past the declined entry; screen:\n{seen}"
        );
        assert!(
            !seen.contains("/model opus"),
            "declined command was injected anyway; screen:\n{seen}"
        );
        assert!(app.startup.failed_sessions().is_empty());
    }
}
