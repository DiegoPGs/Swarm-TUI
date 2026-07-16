//! Milestone 2b stage 2 — observe a wrapped CLI's native "/" command menu,
//! read-only, for `docs/integrations/command-surfaces.md`.
//!
//! Boots the real `claude` or `agy` TUI inside `portable-pty`, types `/` plus
//! probe strings (filtering the tool's own autocomplete), walks the list with
//! arrow keys, and snapshots every state to `target/slash-probe/`. Then Esc and
//! kill — **nothing is ever submitted**.
//!
//! Ground rules (AGENTS.md + the milestone briefs):
//! - characters, arrows, backspace, and Esc only — never Enter at a prompt;
//! - exception 1 (2b, owner-authorized 2026-07-16, recorded in
//!   `docs/NOTES.md`): answering agy's workspace-trust dialog once for this
//!   repo dir (the dialog ignores character keys; Enter accepts the default
//!   "Yes, I trust this folder", which persists one trusted-workspace entry);
//! - exception 2 (2c, owner-authorized 2026-07-16, recorded in
//!   `docs/NOTES.md`): the `usage` mode below MAY submit exactly these
//!   read-only display commands, nothing else, ever: `/usage` and `/status`
//!   (claude), `/usage` and `/credits` (agy).
//!
//! ```sh
//! cargo run --example slash_probe -- claude          # menu observation (2b)
//! cargo run --example slash_probe -- agy
//! cargo run --example slash_probe -- claude usage    # usage screens (2c)
//! cargo run --example slash_probe -- agy usage
//! ```

use std::error::Error;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

const ROWS: u16 = 44;
const COLS: u16 = 140;
const DOWN: &[u8] = b"\x1b[B";
const BACKSPACE: &[u8] = b"\x7f";
const ESC: &[u8] = b"\x1b";

/// Filter strings typed after "/" — each names a command whose local presence
/// the research doc needs settled.
const CLAUDE_PROBES: &[&str] = &[
    "advisor",
    "agents",
    "background",
    "branch",
    "btw",
    "compact",
    "context",
    "cost",
    "effort",
    "fork",
    "goal",
    "keybindings",
    "memory",
    "model",
    "permissions",
    "plan",
    "rename",
    "resume",
    "rewind",
    "status",
    "tasks",
    "usage",
];
const AGY_PROBES: &[&str] = &[
    "agents",
    "btw",
    "codesearch",
    "conversation",
    "credits",
    "fork",
    "goal",
    "keybindings",
    "mcp",
    "model",
    "permissions",
    "plan",
    "rename",
    "resume",
    "rewind",
    "schedule",
    "settings",
    "switch",
    "tasks",
];

/// The `usage` mode's submit whitelists — exactly the four commands the owner
/// authorized for submission (exception 2 in the header). Do not extend.
const CLAUDE_USAGE_CMDS: &[&str] = &["/usage", "/status"];
const AGY_USAGE_CMDS: &[&str] = &["/usage", "/credits"];

fn main() {
    let target = std::env::args().nth(1).unwrap_or_default();
    let mode = std::env::args().nth(2).unwrap_or_default();
    let (bin, probes, usage_cmds): (&str, &[&str], &[&str]) = match target.as_str() {
        "claude" => ("claude", CLAUDE_PROBES, CLAUDE_USAGE_CMDS),
        "agy" => ("agy", AGY_PROBES, AGY_USAGE_CMDS),
        _ => {
            eprintln!("usage: cargo run --example slash_probe -- <claude|agy> [usage]");
            std::process::exit(2);
        }
    };
    // Everything is anchored to the crate root: a stray shell cwd must never
    // relocate the snapshots or — worse — change the workspace the wrapped
    // CLI opens in (agy trust is per-workspace-directory).
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/slash-probe");
    std::fs::create_dir_all(&out_dir).expect("create snapshot dir");

    let result = match mode.as_str() {
        "" => probe(bin, probes, &out_dir),
        "usage" => usage(bin, usage_cmds, &out_dir),
        _ => {
            eprintln!("usage: cargo run --example slash_probe -- <claude|agy> [usage]");
            std::process::exit(2);
        }
    };
    match result {
        Ok(()) => println!(
            "\nslash_probe {bin}: done — snapshots: {}",
            out_dir.display()
        ),
        Err(e) => {
            eprintln!("\nslash_probe {bin}: ERROR — {e}");
            std::process::exit(1);
        }
    }
}

struct Probe {
    bytes: Arc<Mutex<Vec<u8>>>,
    seen: usize,
    parser: vt100::Parser,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    out_dir: PathBuf,
    bin: String,
}

impl Probe {
    fn contents(&mut self) -> String {
        let buf = self.bytes.lock().unwrap();
        if buf.len() > self.seen {
            self.parser.process(&buf[self.seen..]);
            self.seen = buf.len();
        }
        self.parser.screen().contents()
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn settle(&mut self, ms: u64) -> String {
        thread::sleep(Duration::from_millis(ms));
        self.contents()
    }

    fn snapshot(&mut self, name: &str) {
        let contents = self.contents();
        let path = self.out_dir.join(format!("{}-{name}.txt", self.bin));
        let _ = std::fs::write(path, contents);
    }

    /// Wait for a stable, non-blank paint: contents identical across two
    /// consecutive 400ms polls (Ink/agy animate while booting/fetching).
    fn wait_stable(&mut self, cap: Duration) -> Result<(), String> {
        let start = Instant::now();
        let mut previous = String::new();
        while start.elapsed() < cap {
            thread::sleep(Duration::from_millis(400));
            let now = self.contents();
            if !now.trim().is_empty() && now == previous {
                return Ok(());
            }
            previous = now;
        }
        Err(format!("no stable paint within {}s", cap.as_secs()))
    }

    fn wait_boot(&mut self) -> Result<(), String> {
        self.wait_stable(Duration::from_secs(15))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn the wrapped CLI in a PTY with the plain-terminal env, exactly like
/// the fidelity spike / production panes.
fn spawn_probe(bin: &str, out_dir: &std::path::Path) -> Result<Probe, Box<dyn Error>> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(bin);
    cmd.cwd(env!("CARGO_MANIFEST_DIR"));
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SSE_PORT");

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

    Ok(Probe {
        bytes,
        seen: 0,
        parser: vt100::Parser::new(ROWS, COLS, 0),
        writer: pair.master.take_writer()?,
        child,
        out_dir: out_dir.to_path_buf(),
        bin: bin.to_string(),
    })
}

/// Boot to a stable paint and, agy only, answer the workspace-trust dialog
/// (exception 1 — accepting the default "Yes, I trust this folder").
fn boot_and_trust(p: &mut Probe, bin: &str) -> Result<(), Box<dyn Error>> {
    p.wait_boot().map_err(|e| format!("{bin} boot: {e}"))?;
    p.snapshot("00-boot");

    if p.contents().contains("Do you trust") {
        println!("  {bin}: workspace-trust dialog present — accepting (owner-authorized)");
        p.send(b"\r")?;
        p.wait_boot()
            .map_err(|e| format!("{bin} post-trust boot: {e}"))?;
        p.snapshot("01-post-trust");
    }
    Ok(())
}

fn probe(bin: &str, probes: &[&str], out_dir: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let mut p = spawn_probe(bin, out_dir)?;
    boot_and_trust(&mut p, bin)?;

    // Open the native command menu and capture the unfiltered view, then walk
    // downward to expose later pages (arrow keys navigate, never select).
    p.send(b"/")?;
    p.settle(700);
    p.snapshot("02-slash-menu");
    for page in 1..=10 {
        for _ in 0..4 {
            p.send(DOWN)?;
            thread::sleep(Duration::from_millis(60));
        }
        p.settle(250);
        p.snapshot(&format!("03-menu-page{page:02}"));
    }

    // Per-command probes: type the name (filtering the menu), snapshot, then
    // backspace the filter away again.
    for probe in probes {
        p.send(probe.as_bytes())?;
        p.settle(450);
        p.snapshot(&format!("10-{probe}"));
        for _ in 0..probe.len() {
            p.send(BACKSPACE)?;
            thread::sleep(Duration::from_millis(25));
        }
        p.settle(150);
    }

    // Close the menu, clear the "/", leave the input empty, and tear down.
    p.send(ESC)?;
    p.settle(200);
    p.send(BACKSPACE)?;
    p.settle(200);
    p.snapshot("99-final");
    println!("  {bin}: {} probes captured", probes.len());
    Ok(())
}

/// Milestone 2c stage 1 — capture the tools' own usage/quota screens by
/// submitting the whitelisted display commands (exception 2 in the header;
/// every submission is printed so the run log records it). Snapshots are for
/// deriving SYNTHETIC fixtures only — real captures are never committed.
fn usage(bin: &str, commands: &[&str], out_dir: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let mut p = spawn_probe(bin, out_dir)?;
    boot_and_trust(&mut p, bin)?;

    for cmd in commands {
        let name = cmd.trim_start_matches('/');
        p.send(cmd.as_bytes())?;
        p.settle(450);
        p.snapshot(&format!("20-{name}-typed"));

        println!("  {bin}: submitting {cmd} (owner-authorized usage-surface exception)");
        p.send(b"\r")?;
        // Usage pages fetch live quota data; give the redraw a head start,
        // then wait for a stable paint before capturing.
        p.settle(1500);
        p.wait_stable(Duration::from_secs(20))
            .map_err(|e| format!("{bin} {cmd}: {e}"))?;
        p.snapshot(&format!("21-{name}-screen"));

        // Dismiss any full-screen panel, then clear leftover prompt text.
        p.send(ESC)?;
        p.settle(300);
        for _ in 0..(cmd.len() + 2) {
            p.send(BACKSPACE)?;
            thread::sleep(Duration::from_millis(20));
        }
        p.settle(200);
    }

    p.snapshot("99-usage-final");
    println!("  {bin}: {} usage screens captured", commands.len());
    Ok(())
}
