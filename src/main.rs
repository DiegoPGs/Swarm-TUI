//! swarm-tui — one terminal above Claude Code, Antigravity CLI, and Codex CLI.
//!
//! Boot sequence: resolve config (so we know where the log file goes) → init
//! logging (a file under the data dir — NEVER stdout, the alternate screen
//! owns that for the app's whole lifetime) → hand off to `app::run` (ADR-0005).
//! Terminal setup/teardown itself lives in `app::TerminalGuard`, not here.
#![allow(dead_code)] // scaffold: some types still land ahead of their call sites

mod adapters;
mod app;
mod core;
mod pty;
mod store;

use std::fs::OpenOptions;
use std::sync::Mutex;

use clap::Parser;

use core::config::SwarmTuiConfig;

/// swarm-tui: a terminal orchestrator for Claude Code, Antigravity, and Codex.
///
/// No subcommands yet (milestone 2a) — this exists so a later real
/// subcommand (e.g. `swarm-tui doctor`) isn't a breaking CLI change.
#[derive(Parser, Debug)]
#[command(name = "swarm-tui", version, about, long_about = None)]
struct Cli {}

#[tokio::main]
async fn main() {
    let _cli = Cli::parse();

    let config = match SwarmTuiConfig::resolve() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("swarm-tui: failed to resolve config: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = init_logging(&config) {
        eprintln!("swarm-tui: failed to open log file: {e}");
        std::process::exit(1);
    }

    if let Err(e) = app::run(config).await {
        eprintln!("swarm-tui: {e}");
        std::process::exit(1);
    }
}

/// Log destination: `<data_dir>/swarm-tui.log`. Never stdout/stderr once the
/// TUI is up — those belong to the alternate screen.
fn init_logging(config: &SwarmTuiConfig) -> std::io::Result<()> {
    let log_path = config
        .registry_db
        .parent()
        .map(|dir| dir.join("swarm-tui.log"))
        .unwrap_or_else(|| std::path::PathBuf::from("swarm-tui.log"));

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .init();

    Ok(())
}
