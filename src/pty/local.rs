//! Concrete `PaneHost`: a real PTY per pane, fed by `portable-pty`, parsed by
//! `vt100`. Generalizes the mechanics proven in `examples/fidelity_spike.rs`
//! (env-scrub on spawn, dual resize of both the PTY master and the vt100
//! grid) but feeds the parser directly from the reader thread instead of
//! polling a byte buffer, and notifies callers via a channel instead of a
//! poll loop.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::{PaneError, PaneHost, PaneId, PaneSize};

/// Lock a pane mutex, recovering from poison. A poisoned mutex means a
/// thread panicked while holding it (e.g. the pane's reader thread inside
/// vt100 `process()`); the guarded data is a terminal grid or child handle
/// that stays safe to use, and propagating the panic would cascade one
/// pane's failure into every later lock site — aborting the whole UI
/// instead of degrading that pane.
fn lock_ignore_poison<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Pane {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    parser: Arc<Mutex<vt100::Parser>>,
    exited: Arc<AtomicBool>,
    exit_success: Arc<Mutex<Option<bool>>>,
}

/// A `PaneHost` backed by real local PTYs (one OS process per pane).
pub struct LocalPaneHost {
    panes: HashMap<PaneId, Pane>,
    next_id: u64,
    changed_tx: UnboundedSender<PaneId>,
}

impl LocalPaneHost {
    /// Returns the host plus the receiver half of one global "something
    /// changed, maybe redraw" channel — a handful of tabs doesn't need
    /// per-pane channels.
    pub fn new() -> (Self, UnboundedReceiver<PaneId>) {
        let (changed_tx, changed_rx) = mpsc::unbounded_channel();
        (
            Self {
                panes: HashMap::new(),
                next_id: 0,
                changed_tx,
            },
            changed_rx,
        )
    }
}

impl PaneHost for LocalPaneHost {
    fn spawn(&mut self, cmd: Command, size: PaneSize) -> Result<PaneId, PaneError> {
        let mut builder = CommandBuilder::new(cmd.get_program());
        for arg in cmd.get_args() {
            builder.arg(arg);
        }
        if let Some(cwd) = cmd.get_current_dir() {
            builder.cwd(cwd);
        }
        for (k, v) in cmd.get_envs() {
            match v {
                Some(v) => builder.env(k, v),
                None => builder.env_remove(k),
            }
        }

        // AGENTS.md gotcha, generalized: a wrapped CLI spawned from inside
        // another Claude Code session inherits CLAUDECODE/CLAUDE_CODE_*
        // env vars and changes behavior. Every local PTY spawn must look
        // like a plain user terminal.
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env_remove("CLAUDECODE");
        builder.env_remove("CLAUDE_CODE_ENTRYPOINT");
        builder.env_remove("CLAUDE_CODE_SSE_PORT");

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PaneError::Spawn(std::io::Error::other(e)))?;

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| PaneError::Spawn(std::io::Error::other(e)))?;
        drop(pair.slave);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 5000)));
        let exited = Arc::new(AtomicBool::new(false));
        let exit_success = Arc::new(Mutex::new(None));
        let child = Arc::new(Mutex::new(child));

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PaneError::Spawn(std::io::Error::other(e)))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PaneError::Spawn(std::io::Error::other(e)))?;

        let pane_id = PaneId(self.next_id);
        self.next_id += 1;

        let reader_parser = Arc::clone(&parser);
        let reader_exited = Arc::clone(&exited);
        let reader_exit_success = Arc::clone(&exit_success);
        let reader_child = Arc::clone(&child);
        let reader_changed_tx = self.changed_tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        lock_ignore_poison(&reader_parser).process(&buf[..n]);
                        let _ = reader_changed_tx.send(pane_id);
                    }
                    _ => {
                        reader_exited.store(true, Ordering::SeqCst);
                        // PTY EOF can land before the kernel has the child's
                        // exit status ready — a single try_wait() here raced
                        // and misreported clean exits as failures. Poll
                        // briefly (lock released between polls so kill() can
                        // interleave); a child still alive after the window
                        // stays conservatively "failed", same as before.
                        let mut success = None;
                        for _ in 0..100 {
                            match lock_ignore_poison(&reader_child).try_wait() {
                                Ok(Some(status)) => {
                                    success = Some(status.success());
                                    break;
                                }
                                Ok(None) => {}
                                Err(_) => break,
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        *lock_ignore_poison(&reader_exit_success) = Some(success.unwrap_or(false));
                        let _ = reader_changed_tx.send(pane_id);
                        break;
                    }
                }
            }
        });

        self.panes.insert(
            pane_id,
            Pane {
                master: pair.master,
                writer,
                child,
                parser,
                exited,
                exit_success,
            },
        );

        Ok(pane_id)
    }

    fn write_input(&mut self, pane: PaneId, bytes: &[u8]) -> Result<(), PaneError> {
        let pane = self
            .panes
            .get_mut(&pane)
            .ok_or(PaneError::UnknownPane(pane))?;
        pane.writer.write_all(bytes).map_err(PaneError::Spawn)?;
        pane.writer.flush().map_err(PaneError::Spawn)?;
        Ok(())
    }

    fn resize(&mut self, pane: PaneId, size: PaneSize) -> Result<(), PaneError> {
        let pane = self
            .panes
            .get_mut(&pane)
            .ok_or(PaneError::UnknownPane(pane))?;
        pane.master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PaneError::Spawn(std::io::Error::other(e)))?;
        lock_ignore_poison(&pane.parser)
            .screen_mut()
            .set_size(size.rows, size.cols);
        Ok(())
    }

    fn is_exited(&self, pane: PaneId) -> bool {
        self.panes
            .get(&pane)
            .map(|p| p.exited.load(Ordering::SeqCst))
            .unwrap_or(true)
    }

    fn with_screen<R>(
        &self,
        pane: PaneId,
        f: impl FnOnce(&vt100::Screen) -> R,
    ) -> Result<R, PaneError> {
        self.panes
            .get(&pane)
            .ok_or(PaneError::UnknownPane(pane))
            .map(|p| {
                let guard = lock_ignore_poison(&p.parser);
                f(guard.screen())
            })
    }

    fn kill(&mut self, pane: PaneId) -> Result<(), PaneError> {
        let pane = self
            .panes
            .get_mut(&pane)
            .ok_or(PaneError::UnknownPane(pane))?;
        lock_ignore_poison(&pane.child)
            .kill()
            .map_err(PaneError::Spawn)?;
        Ok(())
    }

    fn exit_success(&self, pane: PaneId) -> Option<bool> {
        self.panes
            .get(&pane)
            .and_then(|p| *lock_ignore_poison(&p.exit_success))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Poll `with_screen(...).contents()` until `pred` is satisfied or the
    /// bounded timeout elapses; returns the last-seen contents either way so
    /// assertions get a useful failure message instead of a hang.
    fn poll_screen(
        host: &LocalPaneHost,
        pane: PaneId,
        timeout: Duration,
        mut pred: impl FnMut(&str) -> bool,
    ) -> String {
        let start = Instant::now();
        let mut last = String::new();
        while start.elapsed() < timeout {
            last = host.with_screen(pane, |s| s.contents()).unwrap();
            if pred(&last) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        last
    }

    fn poll_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        cond()
    }

    #[test]
    fn env_scrub_applies_to_every_spawn() {
        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd = Command::new("sh");
        // The ambient environment in CI/sandboxes can carry far more entries
        // than fit in the pane's visible rows, and `vt100::Screen::contents`
        // only reflects the viewport (not scrollback) — so grep down to just
        // the variables under test rather than dumping the full `env`.
        cmd.arg("-c")
            .arg("env | grep -E '^(TERM|COLORTERM|CLAUDECODE)='");
        cmd.env("CLAUDECODE", "1");
        let pane = host.spawn(cmd, PaneSize { rows: 24, cols: 80 }).unwrap();

        let contents = poll_screen(&host, pane, Duration::from_secs(2), |s| {
            s.contains("TERM=") && s.contains("COLORTERM=")
        });

        assert!(
            contents.contains("TERM=xterm-256color"),
            "expected scrubbed TERM, got:\n{contents}"
        );
        assert!(
            contents.contains("COLORTERM=truecolor"),
            "expected scrubbed COLORTERM, got:\n{contents}"
        );
        assert!(
            !contents.lines().any(|l| l.starts_with("CLAUDECODE=")),
            "CLAUDECODE leaked into child env:\n{contents}"
        );
    }

    #[test]
    fn write_input_roundtrips_through_the_child() {
        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("cat");
        let pane = host.spawn(cmd, PaneSize { rows: 24, cols: 80 }).unwrap();

        host.write_input(pane, b"hello\n").unwrap();

        let contents = poll_screen(&host, pane, Duration::from_secs(2), |s| s.contains("hello"));
        assert!(contents.contains("hello"), "got:\n{contents}");
    }

    #[test]
    fn resize_updates_master_and_parser_grid() {
        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 5");
        let pane = host.spawn(cmd, PaneSize { rows: 24, cols: 80 }).unwrap();

        host.resize(
            pane,
            PaneSize {
                rows: 30,
                cols: 100,
            },
        )
        .unwrap();

        let size = host.with_screen(pane, |s| s.size()).unwrap();
        assert_eq!(size, (30, 100));

        host.kill(pane).unwrap();
    }

    #[test]
    fn exit_detection_reports_success_and_failure() {
        let (mut host, _rx) = LocalPaneHost::new();

        let mut ok_cmd = Command::new("sh");
        ok_cmd.arg("-c").arg("exit 0");
        let ok_pane = host.spawn(ok_cmd, PaneSize { rows: 24, cols: 80 }).unwrap();
        assert!(poll_until(
            || host.is_exited(ok_pane),
            Duration::from_secs(2)
        ));
        assert_eq!(host.exit_success(ok_pane), Some(true));

        let mut fail_cmd = Command::new("sh");
        fail_cmd.arg("-c").arg("exit 7");
        let fail_pane = host
            .spawn(fail_cmd, PaneSize { rows: 24, cols: 80 })
            .unwrap();
        assert!(poll_until(
            || host.is_exited(fail_pane),
            Duration::from_secs(2)
        ));
        assert_eq!(host.exit_success(fail_pane), Some(false));
    }

    /// F-003: a panic while holding one pane's parser lock (the way a
    /// reader-thread panic inside vt100 `process()` would poison it) must
    /// degrade that pane only — never cascade into panics on other panes'
    /// lock sites.
    #[test]
    fn poisoned_pane_lock_degrades_that_pane_only() {
        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd_a = Command::new("sh");
        cmd_a.arg("-c").arg("sleep 5");
        let poisoned = host.spawn(cmd_a, PaneSize { rows: 24, cols: 80 }).unwrap();
        let mut cmd_b = Command::new("sh");
        cmd_b.arg("-c").arg("sleep 5");
        let healthy = host.spawn(cmd_b, PaneSize { rows: 24, cols: 80 }).unwrap();

        let parser = Arc::clone(&host.panes.get(&poisoned).unwrap().parser);
        let panicked = std::thread::spawn(move || {
            let _guard = parser.lock().unwrap();
            panic!("simulated reader-thread panic under the parser lock");
        })
        .join();
        assert!(panicked.is_err(), "helper thread must have panicked");
        assert!(host.panes.get(&poisoned).unwrap().parser.is_poisoned());

        // The other pane keeps working…
        host.with_screen(healthy, |s| s.contents())
            .expect("with_screen on the healthy pane");
        host.resize(
            healthy,
            PaneSize {
                rows: 30,
                cols: 100,
            },
        )
        .expect("resize on the healthy pane");

        // …and even the poisoned pane recovers instead of panicking.
        host.with_screen(poisoned, |s| s.contents())
            .expect("with_screen on the poisoned pane");

        host.kill(poisoned).unwrap();
        host.kill(healthy).unwrap();
    }

    #[test]
    fn kill_terminates_a_running_child() {
        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30");
        let pane = host.spawn(cmd, PaneSize { rows: 24, cols: 80 }).unwrap();

        host.kill(pane).unwrap();

        assert!(poll_until(|| host.is_exited(pane), Duration::from_secs(2)));
    }
}
