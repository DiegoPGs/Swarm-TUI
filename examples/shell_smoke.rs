//! Milestone 2b shell smoke — drive the real `swarm-tui` binary inside a
//! `portable-pty` through the milestone-2a smoke checklist:
//!
//! claude tab: paint → type characters only → resize → detach → re-attach →
//! close; agy tab: the pane paints (workspace-trust dialog expected while the
//! repo dir is untrusted). Then quit.
//!
//! Like `fidelity_spike`, this is an *experiment*, not a test — it drives the
//! real wrapped CLIs and a human reads the report plus the snapshots in
//! `target/shell-smoke/`. Ground rules it encodes:
//!
//! - **Enter is only ever pressed on swarm-tui's own surfaces** (new-session
//!   picker, Home roster, y/n confirms). Into a wrapped pane we send printable
//!   characters only — nothing is ever submitted to a model.
//! - The child runs with `SWARM_TUI_DATA_DIR` pointed at a throwaway tempdir,
//!   so the user's real registry is never touched.
//!
//! ```sh
//! cargo build && cargo run --example shell_smoke
//! ```

use std::error::Error;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

const BOOT_ROWS: u16 = 40;
const BOOT_COLS: u16 = 120;
const RESIZED_ROWS: u16 = 30;
const RESIZED_COLS: u16 = 100;
const PREFIX: &[u8] = &[0x00]; // Ctrl-Space over a PTY is a NUL byte
const TYPED: &str = "hello swarm";

fn main() {
    let bin = PathBuf::from("target/debug/swarm-tui");
    if !bin.exists() {
        eprintln!("missing {}: run `cargo build` first", bin.display());
        std::process::exit(2);
    }
    let out_dir = PathBuf::from("target/shell-smoke");
    std::fs::create_dir_all(&out_dir).expect("create snapshot dir");

    match smoke(&bin, &out_dir) {
        Ok(()) => println!("\nsmoke: all steps OK — snapshots: {}", out_dir.display()),
        Err(e) => {
            eprintln!(
                "\nsmoke: DEFECT — {e}\nsnapshots so far: {}",
                out_dir.display()
            );
            std::process::exit(1);
        }
    }
}

/// One PTY session driving the shell. Reader thread + vt100 parser mirror the
/// fidelity spike; every step waits for an on-screen marker before moving on,
/// so the harness never types blind.
struct Driver {
    bytes: Arc<Mutex<Vec<u8>>>,
    seen: usize,
    parser: vt100::Parser,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    out_dir: PathBuf,
}

impl Driver {
    fn feed(&mut self) {
        let buf = self.bytes.lock().unwrap();
        if buf.len() > self.seen {
            self.parser.process(&buf[self.seen..]);
            self.seen = buf.len();
        }
    }

    fn contents(&mut self) -> String {
        self.feed();
        self.parser.screen().contents()
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Poll until the screen contains `marker` (step gate). On timeout, dump
    /// the current screen into the error so the failure is diagnosable.
    fn wait_for(&mut self, step: &str, marker: &str, timeout: Duration) -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.contents().contains(marker) {
                println!("  {step}: OK");
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "{step}: marker {marker:?} never appeared within {timeout:?}\n--- screen ---\n{}",
            self.contents()
        ))
    }

    fn snapshot(&mut self, name: &str) {
        let contents = self.contents();
        let _ = std::fs::write(self.out_dir.join(format!("{name}.txt")), contents);
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        // On the happy path the shell already quit; on failure, kill it (its
        // pane children see PTY EOF and exit — same as closing a terminal).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn smoke(bin: &Path, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let data_dir = tempfile::tempdir()?; // throwaway registry, dropped on exit

    let pty = native_pty_system();
    let pair = pty.openpty(PtySize {
        rows: BOOT_ROWS,
        cols: BOOT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(bin.canonicalize()?);
    cmd.cwd(std::env::current_dir()?);
    // Same plain-terminal env as production panes/fidelity spike, plus the
    // data-dir override that keeps this run off the real registry.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");
    cmd.env("SWARM_TUI_DATA_DIR", data_dir.path());

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let bytes: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    let mut reader = pair.master.try_clone_reader()?;
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });

    let mut d = Driver {
        bytes,
        seen: 0,
        parser: vt100::Parser::new(BOOT_ROWS, BOOT_COLS, 0),
        writer: pair.master.take_writer()?,
        child,
        out_dir: out_dir.to_path_buf(),
    };

    // 1. The shell boots to Home with an empty roster (fresh tempdir).
    d.wait_for("home paints", "Home — roster", Duration::from_secs(10))?;
    d.snapshot("1-home");

    // 2. Prefix + c opens the new-session picker; Enter launches the first
    // row (Claude Code). Enter here lands on swarm-tui's picker, not a pane.
    d.send(PREFIX)?;
    d.wait_for("prefix banner", "AWAITING COMMAND", Duration::from_secs(5))?;
    d.send(b"c")?;
    d.wait_for("picker opens", "New session", Duration::from_secs(5))?;
    d.send(b"\r")?;
    d.wait_for("claude tab paints", "\u{276f}", Duration::from_secs(20))?; // ❯ prompt
    thread::sleep(Duration::from_millis(800)); // let the Ink UI settle
    d.snapshot("2-claude-boot");

    // 3. Characters only — never Enter into the pane.
    d.send(TYPED.as_bytes())?;
    d.wait_for("typed chars echo", TYPED, Duration::from_secs(10))?;
    d.snapshot("3-claude-typed");

    // 4. Resize the outer terminal; the shell re-fits live panes on its next
    // draw and the typed text must survive the repaint.
    pair.master.resize(PtySize {
        rows: RESIZED_ROWS,
        cols: RESIZED_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    d.parser.screen_mut().set_size(RESIZED_ROWS, RESIZED_COLS);
    thread::sleep(Duration::from_secs(2));
    d.wait_for("repaint after resize", TYPED, Duration::from_secs(10))?;
    d.snapshot("4-claude-resized");

    // 5. Detach: tab drops, session keeps running, roster shows the badge.
    d.send(PREFIX)?;
    d.send(b"d")?;
    d.wait_for("detach badge on Home", "[detached]", Duration::from_secs(5))?;
    d.snapshot("5-home-detached");

    // 6. Re-attach from the Home roster (Enter on swarm-tui's own surface):
    // the pane still holds the typed text.
    d.send(b"\r")?;
    d.wait_for("re-attach restores pane", TYPED, Duration::from_secs(5))?;
    d.snapshot("6-claude-reattached");

    // 7. Close (confirmed): kills the pane child, records Failed (killed
    // processes rarely exit 0), lands back on Home.
    d.send(PREFIX)?;
    d.send(b"x")?;
    d.wait_for(
        "close confirm",
        "Close this session?",
        Duration::from_secs(5),
    )?;
    d.send(b"y")?;
    d.wait_for("home after close", "Failed", Duration::from_secs(10))?;
    d.snapshot("7-home-after-close");

    // 8. Second tool: picker → down → Enter spawns agy. While the repo dir is
    // untrusted, agy boots to its workspace-trust dialog; after the owner
    // trusts it (milestone 2b stage 2), the main UI boots instead — both
    // paint a Gemini model name, so wait on that and report which state.
    d.send(PREFIX)?;
    d.send(b"c")?;
    d.wait_for("picker opens again", "New session", Duration::from_secs(5))?;
    d.send(b"j")?;
    d.send(b"\r")?;
    d.wait_for("agy pane paints", "Gemini", Duration::from_secs(20))?;
    let trust_dialog = d.contents().contains("Do you trust");
    println!(
        "  agy trust dialog visible: {}",
        if trust_dialog {
            "yes"
        } else {
            "no (workspace already trusted)"
        }
    );
    d.snapshot("8-agy-pane");

    // 9. Close agy (nothing was answered in the pane) and quit the shell —
    // with no panes left alive, q quits without a confirm.
    d.send(PREFIX)?;
    d.send(b"x")?;
    d.wait_for(
        "agy close confirm",
        "Close this session?",
        Duration::from_secs(5),
    )?;
    d.send(b"y")?;
    d.wait_for("home again", "Home — roster", Duration::from_secs(10))?;
    d.snapshot("9-home-final");
    d.send(PREFIX)?;
    d.send(b"q")?;

    let quit_by = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = d.child.try_wait()? {
            println!("  quit: OK (exit {status:?})");
            break;
        }
        if Instant::now() > quit_by {
            return Err("quit: shell did not exit within 5s of prefix+q".into());
        }
        thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}
