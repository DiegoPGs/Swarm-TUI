//! Role startup-command injection (ADR-0010): after a role spawn, wait for
//! the pane's first stable paint, then inject the declared commands in order
//! through the palette's byte path — with a two-step echo guard so a modal
//! that swallows characters (agy's trust dialog) can never receive a blind
//! Enter, and a confirm pause before any command that matches a
//! `persists: true` command-table entry.
//!
//! Pure state machine over the `PaneHost` trait: driven from the app's tick
//! (`drive`), rate-limited internally; unit-tested against fake `sh -c` panes
//! — no wrapped CLI anywhere near this module.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::app::palette;
use crate::app::tabs::SessionId;
use crate::pty::{PaneHost, PaneId};

/// Stability check cadence (the `slash_probe`/`shell_smoke` pattern: two
/// consecutive identical non-blank paints ⇒ stable).
const PAINT_POLL: Duration = Duration::from_millis(400);
/// Give up waiting for the first stable paint after this long.
const FIRST_PAINT_CAP: Duration = Duration::from_secs(15);
/// Echo-poll cadence while a typed command awaits its on-screen echo.
const ECHO_POLL: Duration = Duration::from_millis(100);
/// A typed command that hasn't echoed within this window means the pane is
/// swallowing characters (modal/dialog) — skip everything, never blind-Enter.
const ECHO_CAP: Duration = Duration::from_secs(2);

/// One startup command, with its confirm requirement precomputed by the app
/// (first token matches a `command_table()` entry with `persists: true`) so
/// this module never needs the adapter tables.
#[derive(Debug, Clone)]
pub struct StartupCommand {
    pub text: String,
    pub needs_confirm: bool,
}

#[derive(Debug)]
enum Phase {
    /// Before the first command: waiting for a stable, non-blank paint.
    AwaitFirstPaint { previous: Option<String> },
    /// Ready to start (or been approved to start) command `next`.
    Ready { approved: bool },
    /// Command text written (no `\r` yet); polling for its echo.
    Typed { since: Instant },
    /// A persists command is waiting for the user's y/n modal.
    AwaitingConfirm,
}

/// Outcome of one `step()` — applied to the map after the borrow ends.
enum Step {
    Idle,
    Changed,
    Done,
    Failed,
}

#[derive(Debug)]
struct SessionStartup {
    pane_id: PaneId,
    role: String,
    commands: Vec<StartupCommand>,
    next: usize,
    phase: Phase,
    seeded: Instant,
    last_poll: Instant,
}

/// All in-flight startup injections, keyed by session. Sessions leave the map
/// on completion or failure; failures stay visible via `failed_sessions`.
#[derive(Default)]
pub struct StartupQueue {
    entries: HashMap<SessionId, SessionStartup>,
    failed: HashSet<SessionId>,
}

impl StartupQueue {
    pub fn seed(
        &mut self,
        session_id: SessionId,
        pane_id: PaneId,
        role: String,
        commands: Vec<StartupCommand>,
    ) {
        if commands.is_empty() {
            return;
        }
        let now = Instant::now();
        self.entries.insert(
            session_id,
            SessionStartup {
                pane_id,
                role,
                commands,
                next: 0,
                phase: Phase::AwaitFirstPaint { previous: None },
                seeded: now,
                last_poll: now,
            },
        );
    }

    /// Sessions whose startup commands were (partly) skipped — the status
    /// line appends a note for these.
    pub fn failed_sessions(&self) -> &HashSet<SessionId> {
        &self.failed
    }

    /// The next session whose persists-command confirm still needs raising.
    /// Callers only invoke this while no modal is up, so every session still
    /// in `AwaitingConfirm` at that point is unraised by construction.
    pub fn next_confirm_request(&self) -> Option<SessionId> {
        self.entries
            .iter()
            .filter(|(_, s)| matches!(s.phase, Phase::AwaitingConfirm))
            .map(|(id, _)| *id)
            .min() // deterministic order when several sessions want one
    }

    /// One-line y/n prompt for the session's pending command.
    pub fn confirm_prompt(&self, session_id: SessionId) -> Option<String> {
        let s = self.entries.get(&session_id)?;
        let cmd = s.commands.get(s.next)?;
        Some(format!(
            "Role \"{}\": inject {}? Its effect persists beyond this session.",
            s.role, cmd.text
        ))
    }

    /// The user confirmed the pending command: type it on the next drive.
    pub fn approve(&mut self, session_id: SessionId) {
        if let Some(s) = self.entries.get_mut(&session_id) {
            if matches!(s.phase, Phase::AwaitingConfirm) {
                s.phase = Phase::Ready { approved: true };
            }
        }
    }

    /// The user declined: skip this entry, continue with the rest (ADR-0010).
    pub fn skip_current(&mut self, session_id: SessionId) {
        let done = match self.entries.get_mut(&session_id) {
            Some(s) => {
                if matches!(s.phase, Phase::AwaitingConfirm) {
                    s.next += 1;
                    s.phase = Phase::Ready { approved: false };
                }
                s.next >= s.commands.len()
            }
            None => false,
        };
        if done {
            self.entries.remove(&session_id);
        }
    }

    /// Advance every in-flight session one step. Called from the app tick;
    /// internally rate-limited, so calling it every 33ms is fine. Returns
    /// true when anything changed (the caller marks the frame dirty).
    pub fn drive<H: PaneHost>(&mut self, host: &mut H) -> bool {
        let mut changed = false;
        let ids: Vec<SessionId> = self.entries.keys().copied().collect();
        for id in ids {
            changed |= self.drive_one(id, host);
        }
        changed
    }

    fn drive_one<H: PaneHost>(&mut self, id: SessionId, host: &mut H) -> bool {
        // Decide inside the entry borrow, remove/fail after it — the map
        // can't be mutated while one of its values is.
        let step = {
            let Some(s) = self.entries.get_mut(&id) else {
                return false;
            };
            Self::step(s, host)
        };
        match step {
            Step::Idle => false,
            Step::Changed => true,
            Step::Done => {
                self.entries.remove(&id);
                true
            }
            Step::Failed => {
                self.entries.remove(&id);
                self.failed.insert(id);
                true
            }
        }
    }

    fn step<H: PaneHost>(s: &mut SessionStartup, host: &mut H) -> Step {
        // A dead pane can't take input; drop everything still queued.
        if host.is_exited(s.pane_id) {
            tracing::warn!(
                "startup[{}]: pane exited — skipping remaining commands",
                s.role
            );
            return Step::Failed;
        }

        match &mut s.phase {
            Phase::AwaitFirstPaint { previous } => {
                if s.last_poll.elapsed() < PAINT_POLL {
                    return Step::Idle;
                }
                s.last_poll = Instant::now();
                let now = host
                    .with_screen(s.pane_id, |screen| screen.contents())
                    .unwrap_or_default();
                let stable = !now.trim().is_empty() && previous.as_deref() == Some(now.as_str());
                if stable {
                    s.phase = Phase::Ready { approved: false };
                    return Step::Changed;
                }
                *previous = Some(now);
                if s.seeded.elapsed() > FIRST_PAINT_CAP {
                    tracing::warn!(
                        "startup[{}]: no stable paint within {FIRST_PAINT_CAP:?} — \
                         skipping startup commands",
                        s.role
                    );
                    return Step::Failed;
                }
                Step::Idle
            }
            Phase::Ready { approved } => {
                if s.last_poll.elapsed() < PAINT_POLL {
                    return Step::Idle;
                }
                s.last_poll = Instant::now();
                let Some(cmd) = s.commands.get(s.next) else {
                    return Step::Done;
                };
                if cmd.needs_confirm && !*approved {
                    s.phase = Phase::AwaitingConfirm;
                    return Step::Changed;
                }
                // Step one of two: the text only. Enter follows the echo.
                let bytes = palette::raw_command_bytes(&cmd.text);
                if host.write_input(s.pane_id, &bytes).is_err() {
                    return Step::Failed;
                }
                s.phase = Phase::Typed {
                    since: Instant::now(),
                };
                Step::Changed
            }
            Phase::Typed { since } => {
                if s.last_poll.elapsed() < ECHO_POLL {
                    return Step::Idle;
                }
                s.last_poll = Instant::now();
                let text = &s.commands[s.next].text;
                let screen = host
                    .with_screen(s.pane_id, |screen| screen.contents())
                    .unwrap_or_default();
                if screen.contains(text.as_str()) {
                    // Echo verified — safe to press Enter.
                    if host.write_input(s.pane_id, b"\r").is_err() {
                        return Step::Failed;
                    }
                    s.next += 1;
                    s.phase = Phase::Ready { approved: false };
                    if s.next >= s.commands.len() {
                        return Step::Done;
                    }
                    return Step::Changed;
                }
                if since.elapsed() > ECHO_CAP {
                    // The pane is swallowing characters (modal?). Never send
                    // a blind Enter — skip everything (ADR-0010).
                    //
                    // This is the only site that logs user-authored text, and
                    // the log file is append-only with no rotation — so it goes
                    // through the same redaction as persisted prompts (W-29,
                    // F-011). Defense in depth: a startup command is a slash
                    // command, so a credential here is unlikely, not impossible.
                    let text = crate::core::redact::redact(text);
                    tracing::warn!(
                        "startup[{}]: {text:?} never echoed — pane swallowing input? \
                         Skipping remaining commands",
                        s.role
                    );
                    return Step::Failed;
                }
                Step::Idle
            }
            // Waits for approve()/skip_current() from the confirm modal.
            Phase::AwaitingConfirm => Step::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::local::LocalPaneHost;
    use crate::pty::PaneSize;

    /// Fake TUI: paints a prompt line, then echoes stdin — enough for the
    /// stability wait and the echo guard, no wrapped CLI anywhere near.
    fn cat_pane(host: &mut LocalPaneHost, script: &str) -> PaneId {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script);
        host.spawn(cmd, PaneSize { rows: 24, cols: 80 }).unwrap()
    }

    fn cmd(text: &str, needs_confirm: bool) -> StartupCommand {
        StartupCommand {
            text: text.to_string(),
            needs_confirm,
        }
    }

    /// Drive until the queue drains (or a deadline passes), like the app
    /// tick would.
    fn drive_until_done(queue: &mut StartupQueue, host: &mut LocalPaneHost, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !queue.entries.is_empty() && Instant::now() < deadline {
            // Confirm requests would deadlock a drain — tests that use
            // confirms drive manually instead.
            assert!(
                queue.next_confirm_request().is_none(),
                "unexpected confirm request in a plain drain"
            );
            queue.drive(host);
            std::thread::sleep(Duration::from_millis(33));
        }
        assert!(queue.entries.is_empty(), "queue never drained");
    }

    fn screen_of(host: &LocalPaneHost, pane: PaneId) -> String {
        host.with_screen(pane, |s| s.contents()).unwrap()
    }

    #[test]
    fn startup_commands_arrive_in_order_in_a_fake_cat_pane() {
        let (mut host, _rx) = LocalPaneHost::new();
        let pane = cat_pane(&mut host, "echo ready; cat");
        let mut queue = StartupQueue::default();
        queue.seed(
            7,
            pane,
            "advisor".into(),
            vec![cmd("/first one", false), cmd("/second two", false)],
        );

        drive_until_done(&mut queue, &mut host, 10);

        let screen = screen_of(&host, pane);
        let first = screen.find("/first one").expect("first command echoed");
        let second = screen.find("/second two").expect("second command echoed");
        assert!(
            first < second,
            "commands out of declared order; screen:\n{screen}"
        );
        assert!(queue.failed_sessions().is_empty());
        host.kill(pane).unwrap();
    }

    #[test]
    fn persists_entry_pauses_for_confirm_and_injects_on_yes() {
        let (mut host, _rx) = LocalPaneHost::new();
        let pane = cat_pane(&mut host, "echo ready; cat");
        let mut queue = StartupQueue::default();
        queue.seed(3, pane, "coder".into(), vec![cmd("/model opus", true)]);

        // Drive until the confirm is requested — nothing may be typed yet.
        let deadline = Instant::now() + Duration::from_secs(10);
        while queue.next_confirm_request().is_none() && Instant::now() < deadline {
            queue.drive(&mut host);
            std::thread::sleep(Duration::from_millis(33));
        }
        assert_eq!(queue.next_confirm_request(), Some(3));
        assert!(
            !screen_of(&host, pane).contains("/model"),
            "typed before the confirm was answered"
        );
        let prompt = queue.confirm_prompt(3).expect("prompt");
        assert!(prompt.contains("coder") && prompt.contains("/model opus"));

        queue.approve(3);
        drive_until_done(&mut queue, &mut host, 10);
        assert!(screen_of(&host, pane).contains("/model opus"));
        assert!(queue.failed_sessions().is_empty());
        host.kill(pane).unwrap();
    }

    #[test]
    fn declined_confirm_skips_entry_and_continues_queue() {
        let (mut host, _rx) = LocalPaneHost::new();
        let pane = cat_pane(&mut host, "echo ready; cat");
        let mut queue = StartupQueue::default();
        queue.seed(
            4,
            pane,
            "coder".into(),
            vec![cmd("/model opus", true), cmd("/after decline", false)],
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        while queue.next_confirm_request().is_none() && Instant::now() < deadline {
            queue.drive(&mut host);
            std::thread::sleep(Duration::from_millis(33));
        }
        assert_eq!(queue.next_confirm_request(), Some(4));

        queue.skip_current(4);
        drive_until_done(&mut queue, &mut host, 10);

        let screen = screen_of(&host, pane);
        assert!(
            !screen.contains("/model opus"),
            "declined command was injected anyway:\n{screen}"
        );
        assert!(screen.contains("/after decline"));
        assert!(queue.failed_sessions().is_empty());
        host.kill(pane).unwrap();
    }

    #[test]
    fn startup_gives_up_when_pane_never_echoes() {
        let (mut host, _rx) = LocalPaneHost::new();
        // Paints, then swallows input without echo — the trust-dialog shape.
        let pane = cat_pane(&mut host, "stty -echo; echo ready; cat");
        let mut queue = StartupQueue::default();
        queue.seed(
            9,
            pane,
            "advisor".into(),
            vec![cmd("/never echoed", false), cmd("/also dropped", false)],
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        while !queue.entries.is_empty() && Instant::now() < deadline {
            queue.drive(&mut host);
            std::thread::sleep(Duration::from_millis(33));
        }

        assert!(queue.entries.is_empty(), "queue never resolved");
        assert!(queue.failed_sessions().contains(&9));
        // The guard's failure mode is skip: no Enter byte was ever sent, so
        // `cat` received text but the screen shows nothing and no command ran.
        assert!(!screen_of(&host, pane).contains("/also dropped"));
        host.kill(pane).unwrap();
    }
}
