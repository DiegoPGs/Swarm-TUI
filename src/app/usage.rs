//! Plan-usage visibility (ADR-0011): on user request only, spawn a hidden
//! short-lived pane for a vendor, inject its own `/usage` command (two-step
//! echo guard, like startup injection), wait for a stable paint, snapshot the
//! vt100 grid, kill the pane, and render the captured lines VERBATIM — the
//! tool's own words, zero parsing drift — plus a best-effort one-line
//! headline that degrades to nothing silently.
//!
//! The probe pane is instrumentation, not a session: no tab, no
//! `pane_of_session` entry, no registry row. Headline extraction is tested
//! against the committed synthetic fixtures only.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::adapters::{AdapterKind, CliAdapter};
use crate::core::plan::SwarmPlan;
use crate::pty::{PaneError, PaneHost, PaneId, PaneSize};

use super::home::format_age;

/// Research capture geometry (`slash_probe`) — roomy enough for both tools'
/// usage pages, and never resized (the pane is invisible).
const PROBE_SIZE: PaneSize = PaneSize {
    rows: 44,
    cols: 140,
};
/// Stability cadence, as everywhere: identical non-blank paints 400ms apart.
const PAINT_POLL: Duration = Duration::from_millis(400);
const ECHO_POLL: Duration = Duration::from_millis(100);
const BOOT_CAP: Duration = Duration::from_secs(15);
const ECHO_CAP: Duration = Duration::from_secs(2);
/// After Enter: if the output never stabilizes, capture the last screen
/// anyway — a spinner beats nothing, and the view is verbatim by design.
const OUTPUT_CAP: Duration = Duration::from_secs(10);

/// One vendor's last captured usage screen.
#[derive(Debug, Clone)]
pub struct UsageCapture {
    /// The tool's own lines, verbatim (trailing/leading blank lines trimmed).
    pub lines: Vec<String>,
    /// Best-effort: the first line containing a digit-run immediately
    /// followed by `%`. `None` means the heuristic found nothing — silently.
    pub headline: Option<String>,
    pub at: SystemTime,
}

impl UsageCapture {
    fn from_screen(contents: &str) -> UsageCapture {
        let mut lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        while lines.first().is_some_and(|l| l.trim().is_empty()) {
            lines.remove(0);
        }
        UsageCapture {
            headline: extract_headline(contents),
            lines,
            at: SystemTime::now(),
        }
    }

    /// A capture that reports a probe failure in its own verbatim slot.
    pub fn failure(msg: String) -> UsageCapture {
        UsageCapture {
            lines: vec![msg],
            headline: None,
            at: SystemTime::now(),
        }
    }
}

/// First line containing an ASCII-digit run immediately followed by `%`,
/// trimmed. Hand-rolled scan — no regex dependency. Garbage ⇒ `None`.
pub fn extract_headline(text: &str) -> Option<String> {
    for line in text.lines() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'%' {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    None
}

/// The bytes to inject for a vendor's usage screen: its own `command_table()`
/// entry for the app-level concept key `/usage` (the cross-tool concept map
/// row). `None` ⇒ the vendor gets no refresh key and a "no usage command"
/// block — never a guessed command.
pub fn probe_command_for(kind: AdapterKind) -> Option<&'static str> {
    kind.command_table()
        .iter()
        .find(|e| e.name == "/usage")
        .map(|e| e.inject)
}

#[derive(Debug)]
enum ProbePhase {
    /// Waiting for the tool's first stable paint.
    Boot { previous: Option<String> },
    /// Usage command text written (no `\r` yet); waiting for its echo.
    Typed { since: Instant },
    /// Enter sent; waiting for the usage screen to stabilize.
    AwaitOutput { previous: Option<String> },
}

/// What one `drive()` call did — the app applies `Finished`/`Aborted` by
/// killing the pane and dropping the probe.
pub enum ProbeStep {
    Idle,
    Changed,
    Finished(UsageCapture),
    Aborted(String),
}

/// One in-flight hidden usage probe.
pub struct ProbePane {
    pane_id: PaneId,
    inject: &'static str,
    phase: ProbePhase,
    started: Instant,
    last_poll: Instant,
}

impl ProbePane {
    /// Spawn the vendor CLI into a hidden pane. The command comes from the
    /// adapter (`interactive_cmd`), the injection text from
    /// `probe_command_for` — no CLI knowledge lives here.
    pub fn spawn<H: PaneHost>(
        host: &mut H,
        cmd: std::process::Command,
        inject: &'static str,
    ) -> Result<ProbePane, PaneError> {
        let pane_id = host.spawn(cmd, PROBE_SIZE)?;
        let now = Instant::now();
        Ok(ProbePane {
            pane_id,
            inject,
            phase: ProbePhase::Boot { previous: None },
            started: now,
            last_poll: now,
        })
    }

    pub fn pane_id(&self) -> PaneId {
        self.pane_id
    }

    pub fn drive<H: PaneHost>(&mut self, host: &mut H) -> ProbeStep {
        if host.is_exited(self.pane_id) {
            return ProbeStep::Aborted("probe pane exited before capture".to_string());
        }
        match &mut self.phase {
            ProbePhase::Boot { previous } => {
                if self.last_poll.elapsed() < PAINT_POLL {
                    return ProbeStep::Idle;
                }
                self.last_poll = Instant::now();
                let now = host
                    .with_screen(self.pane_id, |s| s.contents())
                    .unwrap_or_default();
                let stable = !now.trim().is_empty() && previous.as_deref() == Some(now.as_str());
                if stable {
                    // Two-step injection, step one: the text only.
                    if host
                        .write_input(self.pane_id, self.inject.as_bytes())
                        .is_err()
                    {
                        return ProbeStep::Aborted("probe pane rejected input".to_string());
                    }
                    self.phase = ProbePhase::Typed {
                        since: Instant::now(),
                    };
                    return ProbeStep::Changed;
                }
                *previous = Some(now);
                if self.started.elapsed() > BOOT_CAP {
                    return ProbeStep::Aborted("probe: tool never painted a stable screen".into());
                }
                ProbeStep::Idle
            }
            ProbePhase::Typed { since } => {
                if self.last_poll.elapsed() < ECHO_POLL {
                    return ProbeStep::Idle;
                }
                self.last_poll = Instant::now();
                let screen = host
                    .with_screen(self.pane_id, |s| s.contents())
                    .unwrap_or_default();
                if screen.contains(self.inject) {
                    // Echo verified — safe to press Enter.
                    if host.write_input(self.pane_id, b"\r").is_err() {
                        return ProbeStep::Aborted("probe pane rejected input".to_string());
                    }
                    self.phase = ProbePhase::AwaitOutput { previous: None };
                    return ProbeStep::Changed;
                }
                if since.elapsed() > ECHO_CAP {
                    // Characters swallowed (untrusted workspace dialog?) —
                    // abort, never blind-Enter (ADR-0011).
                    return ProbeStep::Aborted(
                        "probe failed — is the workspace trusted?".to_string(),
                    );
                }
                ProbeStep::Idle
            }
            ProbePhase::AwaitOutput { previous } => {
                if self.last_poll.elapsed() < PAINT_POLL {
                    return ProbeStep::Idle;
                }
                self.last_poll = Instant::now();
                let now = host
                    .with_screen(self.pane_id, |s| s.contents())
                    .unwrap_or_default();
                let stable = !now.trim().is_empty() && previous.as_deref() == Some(now.as_str());
                if stable {
                    return ProbeStep::Finished(UsageCapture::from_screen(&now));
                }
                let timed_out = self.started.elapsed() > BOOT_CAP + OUTPUT_CAP;
                if timed_out {
                    tracing::warn!("usage probe: output never stabilized — capturing last screen");
                    return ProbeStep::Finished(UsageCapture::from_screen(&now));
                }
                *previous = Some(now);
                ProbeStep::Idle
            }
        }
    }
}

/// Full-body Resources view (prefix+`u`, ADR-0011): one block per active
/// vendor — plan role assignments, headline, timestamp, capture verbatim.
pub fn render_resources(
    frame: &mut Frame,
    area: Rect,
    plan: Option<&SwarmPlan>,
    usage: &HashMap<AdapterKind, UsageCapture>,
    in_flight: &HashMap<AdapterKind, ProbePane>,
    scroll: usize,
) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();
    for (i, kind) in crate::adapters::registry().iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        let n = i + 1;
        let status = if in_flight.contains_key(kind) {
            "refreshing…".to_string()
        } else if probe_command_for(*kind).is_none() {
            "no usage command".to_string()
        } else {
            match usage.get(kind) {
                Some(capture) => format!("as of {} ago", format_age(capture.at)),
                None => format!("no capture yet — press {n}"),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(format!("[{n}] {}", kind.display_name()), bold),
            Span::styled(format!(" — {status}"), dim),
        ]));

        // Role assignments from the swarm plan (name → tool · model/effort).
        if let Some(plan) = plan {
            for (name, role) in plan.roles.iter().filter(|(_, r)| r.tool == kind.id()) {
                let launch = match (role.model.as_deref(), role.effort.as_deref()) {
                    (Some(m), Some(e)) => format!(" · {m}/{e}"),
                    (Some(m), None) => format!(" · {m}"),
                    (None, Some(e)) => format!(" · effort {e}"),
                    (None, None) => String::new(),
                };
                lines.push(Line::from(format!("  {name} → {}{launch}", role.tool)));
            }
        }

        if let Some(capture) = usage.get(kind) {
            if let Some(headline) = &capture.headline {
                lines.push(Line::from(Span::styled(
                    format!("  {headline}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.push(Line::from(Span::styled(
                "  ┄┄┄ capture (verbatim) ┄┄┄",
                dim,
            )));
            for line in &capture.lines {
                lines.push(Line::from(format!("  {line}")));
            }
        }
    }

    let block = Block::default().borders(Borders::ALL).title(
        "Resources — 1..N refresh vendor · j/k scroll · Esc/u back (refresh injects /usage into a real, hidden, unregistered pane)",
    );
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0))
            .block(block),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_FIXTURE: &str = include_str!("../../tests/fixtures/claude_usage.synthetic.txt");
    const AGY_FIXTURE: &str = include_str!("../../tests/fixtures/agy_usage.synthetic.txt");

    #[test]
    fn headline_extracts_first_percent_line_from_claude_fixture() {
        let headline = extract_headline(CLAUDE_FIXTURE).expect("claude fixture has a % line");
        // The first plan-window bar ("13% used"), not the later ones.
        assert!(headline.contains("13% used"), "got: {headline}");
    }

    #[test]
    fn headline_extracts_first_percent_line_from_agy_fixture() {
        let headline = extract_headline(AGY_FIXTURE).expect("agy fixture has a % line");
        assert!(headline.contains("71.00%"), "got: {headline}");
    }

    #[test]
    fn garbage_capture_yields_no_headline() {
        assert_eq!(extract_headline(""), None);
        assert_eq!(
            extract_headline("no percentages here, 42 and 7 alone"),
            None
        );
        assert_eq!(extract_headline("stray % with no digits"), None);
        // A digit-run must IMMEDIATELY precede the '%'.
        assert_eq!(extract_headline("50 %"), None);
    }

    #[test]
    fn capture_trims_blank_edges_and_keeps_inner_lines_verbatim() {
        let capture = UsageCapture::from_screen("\n\n  Usage\n\n   13% used\n\n\n");
        assert_eq!(capture.lines, vec!["  Usage", "", "   13% used"]);
        assert_eq!(capture.headline.as_deref(), Some("13% used"));
    }

    #[test]
    fn probe_command_comes_from_the_adapter_tables() {
        // Both active vendors declare /usage (pinned against the doc by the
        // adapters tests); suspended codex has an empty table.
        assert_eq!(probe_command_for(AdapterKind::ClaudeCode), Some("/usage"));
        assert_eq!(probe_command_for(AdapterKind::Antigravity), Some("/usage"));
        assert_eq!(probe_command_for(AdapterKind::Codex), None);
    }

    // -- probe machine vs fake panes (no wrapped CLI anywhere near) ----------

    use crate::pty::local::LocalPaneHost;

    fn drive_to_end(
        probe: &mut ProbePane,
        host: &mut LocalPaneHost,
    ) -> Result<UsageCapture, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match probe.drive(host) {
                ProbeStep::Finished(capture) => return Ok(capture),
                ProbeStep::Aborted(msg) => return Err(msg),
                _ => {}
            }
            assert!(Instant::now() < deadline, "probe never resolved");
            std::thread::sleep(Duration::from_millis(33));
        }
    }

    #[test]
    fn probe_pane_captures_a_fake_usage_screen() {
        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("echo 'plan: 42% used'; cat");
        let mut probe = ProbePane::spawn(&mut host, cmd, "/usage").unwrap();

        let capture = drive_to_end(&mut probe, &mut host).expect("capture");
        let _ = host.kill(probe.pane_id());

        // Boot line survived verbatim; the injected command echoed; the
        // headline is the first %-line.
        assert!(capture.lines.iter().any(|l| l.contains("42% used")));
        assert!(capture.lines.iter().any(|l| l.contains("/usage")));
        assert_eq!(capture.headline.as_deref(), Some("plan: 42% used"));
    }

    #[test]
    fn probe_aborts_when_the_pane_swallows_characters() {
        let (mut host, _rx) = LocalPaneHost::new();
        // The trust-dialog shape: paints, then eats input without echo.
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("stty -echo; echo ready; cat");
        let mut probe = ProbePane::spawn(&mut host, cmd, "/usage").unwrap();

        let err = drive_to_end(&mut probe, &mut host).expect_err("must abort");
        let _ = host.kill(probe.pane_id());
        assert!(err.contains("trusted"), "unexpected abort reason: {err}");
    }
}
