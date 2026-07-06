//! ADR-0003 fidelity spike — run each wrapped CLI's real TUI inside
//! `portable-pty`, parse the byte stream with `vt100`, and render the parsed
//! screen through `tui-term` into an off-screen ratatui buffer.
//!
//! This is an *experiment*, not a test: it drives the real, locally installed
//! binaries (which is why it lives in `examples/`, outside `cargo test` — the
//! repo rule is fixtures, not live CLIs, in tests) and a human reads the
//! report plus the snapshots it drops in `target/fidelity-spike/`.
//!
//! ```sh
//! cargo run --example fidelity_spike
//! ```
//!
//! Per CLI (claude, agy, codex — skipping any that is not installed):
//!   1. boot — the TUI paints and holds still within the boot window;
//!   2. echo — typed keystrokes appear on the parsed screen (characters only,
//!      never Enter: no prompt is submitted, no model call);
//!   3. resize — after a PTY resize the program repaints inside the new grid;
//!   4. widget — the vt100 screen renders through `tui_term::PseudoTerminal`
//!      into a ratatui `Buffer` without panicking, and the buffer text agrees
//!      with the vt100 view of the same grid;
//!   5. stats — cells filled, colored cells, alternate-screen usage.
//!
//! The verdict this produces is the gate on ADR-0003 (vt100 widget vs. the
//! fallback ladder); findings are recorded in docs/NOTES.md.

use std::error::Error;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use tui_term::widget::PseudoTerminal;

const BOOT_ROWS: u16 = 40;
const BOOT_COLS: u16 = 120;
const RESIZED_ROWS: u16 = 30;
const RESIZED_COLS: u16 = 100;
const TYPED: &str = "hello swarm";

fn main() {
    let out_dir = PathBuf::from("target/fidelity-spike");
    std::fs::create_dir_all(&out_dir).expect("create snapshot dir");

    for bin in ["claude", "agy", "codex"] {
        println!("──── {bin} ────");
        if !installed(bin) {
            println!("  SKIP: not installed on this machine\n");
            continue;
        }
        match spike(bin, &out_dir) {
            Ok(report) => println!("{report}"),
            Err(e) => println!("  ERROR: {e}\n"),
        }
    }
    println!("snapshots: {}", out_dir.display());
}

/// `<bin> --version` exits 0 — presence probe, same read-only shape the
/// adapters will use.
fn installed(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct Report {
    bin: String,
    boot_ms: u128,
    boot_ok: bool,
    echo_ok: bool,
    resize_ok: bool,
    widget_diff_lines: usize,
    filled_cells: usize,
    colored_cells: usize,
    alternate_screen: bool,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "  boot: {} ({} ms to stable paint)",
            tick(self.boot_ok),
            self.boot_ms
        )?;
        writeln!(f, "  echo of typed chars: {}", tick(self.echo_ok))?;
        writeln!(
            f,
            "  repaint after resize {BOOT_COLS}x{BOOT_ROWS} → {RESIZED_COLS}x{RESIZED_ROWS}: {}",
            tick(self.resize_ok)
        )?;
        writeln!(
            f,
            "  tui-term buffer vs vt100 screen: {} differing lines",
            self.widget_diff_lines
        )?;
        writeln!(
            f,
            "  final grid: {} filled cells, {} colored cells, alt-screen={}",
            self.filled_cells, self.colored_cells, self.alternate_screen
        )?;
        writeln!(
            f,
            "  snapshots: {}-{{1-boot,2-typed,3-resized,4-widget}}.txt",
            self.bin
        )
    }
}

fn tick(ok: bool) -> &'static str {
    if ok {
        "OK"
    } else {
        "DEFECT"
    }
}

fn spike(bin: &str, out_dir: &Path) -> Result<Report, Box<dyn Error>> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize {
        rows: BOOT_ROWS,
        cols: BOOT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(bin);
    cmd.cwd(std::env::current_dir()?);
    // A tab must look like a plain user terminal: fixed TERM, and none of the
    // markers leaked by the Claude Code session hosting this spike (a nested
    // `claude` changes behavior when CLAUDECODE is set).
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    // Pump raw PTY bytes into a shared buffer; the parser replays increments.
    let bytes: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    let mut reader = pair.master.try_clone_reader()?;
    let pump = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let mut writer = pair.master.take_writer()?;

    let mut parser = vt100::Parser::new(BOOT_ROWS, BOOT_COLS, 0);
    let mut seen = 0usize;

    // 1. boot: poll until the screen is non-blank and identical across two
    // consecutive polls (Ink-based TUIs animate while loading), 12 s cap.
    let start = Instant::now();
    let mut previous = String::new();
    let mut boot_ok = false;
    while start.elapsed() < Duration::from_secs(12) {
        thread::sleep(Duration::from_millis(400));
        feed(&mut parser, &bytes, &mut seen);
        let now = parser.screen().contents();
        if !now.trim().is_empty() && now == previous {
            boot_ok = true;
            break;
        }
        previous = now;
    }
    let boot_ms = start.elapsed().as_millis();
    snapshot(out_dir, bin, "1-boot", &parser.screen().contents())?;

    // 2. echo: characters only — never Enter, so nothing is submitted.
    writer.write_all(TYPED.as_bytes())?;
    writer.flush()?;
    thread::sleep(Duration::from_millis(1500));
    feed(&mut parser, &bytes, &mut seen);
    let typed_view = parser.screen().contents();
    let echo_ok = typed_view.contains(TYPED);
    snapshot(out_dir, bin, "2-typed", &typed_view)?;

    // 3. resize: shrink the PTY (drives SIGWINCH) and mirror it in the parser,
    // exactly what the session view will do on pane-layout changes.
    pair.master.resize(PtySize {
        rows: RESIZED_ROWS,
        cols: RESIZED_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    parser.screen_mut().set_size(RESIZED_ROWS, RESIZED_COLS);
    thread::sleep(Duration::from_secs(2));
    feed(&mut parser, &bytes, &mut seen);
    let resized_view = parser.screen().contents();
    let resize_ok = !resized_view.trim().is_empty();
    snapshot(out_dir, bin, "3-resized", &resized_view)?;

    // 4. widget: the exact production path — vt100 screen through
    // tui_term::PseudoTerminal into a ratatui Buffer.
    let widget_view = render_via_tui_term(parser.screen(), RESIZED_ROWS, RESIZED_COLS);
    let widget_diff_lines = diff_lines(&resized_view, &widget_view);
    snapshot(out_dir, bin, "4-widget", &widget_view)?;

    // 5. stats on the final grid.
    let screen = parser.screen();
    let (filled_cells, colored_cells) = cell_stats(screen, RESIZED_ROWS, RESIZED_COLS);
    let report = Report {
        bin: bin.to_string(),
        boot_ms,
        boot_ok,
        echo_ok,
        resize_ok,
        widget_diff_lines,
        filled_cells,
        colored_cells,
        alternate_screen: screen.alternate_screen(),
    };

    // Tab-close semantics (ARCHITECTURE): kill only the PTY child.
    child.kill()?;
    let _ = child.wait();
    drop(writer);
    drop(pair.master);
    let _ = pump.join();

    Ok(report)
}

fn feed(parser: &mut vt100::Parser, bytes: &Arc<Mutex<Vec<u8>>>, seen: &mut usize) {
    let buf = bytes.lock().unwrap();
    if buf.len() > *seen {
        parser.process(&buf[*seen..]);
        *seen = buf.len();
    }
}

fn render_via_tui_term(screen: &vt100::Screen, rows: u16, cols: u16) -> String {
    let area = Rect::new(0, 0, cols, rows);
    let mut buf = Buffer::empty(area);
    PseudoTerminal::new(screen).render(area, &mut buf);
    let mut out = String::new();
    for y in 0..rows {
        let mut line = String::new();
        for x in 0..cols {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Lines differing between two screen dumps, ignoring trailing blanks —
/// the spike's proxy for "the widget shows what the parser holds".
fn diff_lines(a: &str, b: &str) -> usize {
    let norm = |s: &str| -> Vec<String> {
        let mut v: Vec<String> = s.lines().map(|l| l.trim_end().to_string()).collect();
        while v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v
    };
    let (a, b) = (norm(a), norm(b));
    let common = a.len().min(b.len());
    let mut diff = a.len().max(b.len()) - common;
    for i in 0..common {
        if a[i] != b[i] {
            diff += 1;
        }
    }
    diff
}

fn cell_stats(screen: &vt100::Screen, rows: u16, cols: u16) -> (usize, usize) {
    let mut filled = 0;
    let mut colored = 0;
    for r in 0..rows {
        for c in 0..cols {
            if let Some(cell) = screen.cell(r, c) {
                if cell.has_contents() {
                    filled += 1;
                }
                if cell.fgcolor() != vt100::Color::Default
                    || cell.bgcolor() != vt100::Color::Default
                {
                    colored += 1;
                }
            }
        }
    }
    (filled, colored)
}

fn snapshot(dir: &Path, bin: &str, stage: &str, contents: &str) -> std::io::Result<()> {
    std::fs::write(dir.join(format!("{bin}-{stage}.txt")), contents)
}
