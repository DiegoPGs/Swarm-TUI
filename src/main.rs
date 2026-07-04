//! overstory — one terminal above Claude Code, Antigravity CLI, and Codex CLI.
//!
//! This is a documentation-first scaffold: every module below exists to pin a
//! boundary decided in `docs/adr/`, not to do work yet. Start with `CLAUDE.md`
//! (first-session checklist), then `docs/ARCHITECTURE.md`.
#![allow(dead_code)] // scaffold: types land ahead of their call sites

mod adapters;
mod app;
mod core;
mod pty;
mod store;

fn main() {
    // TODO(next session): clap arg parsing, tracing init, tokio runtime, then
    // hand off to `app::run()` (ADR-0005). Before ANY of that: run
    // ./scripts/verify-clis.sh on the real machine and fold results into
    // docs/integrations/*.md — the adapters encode unverified (⬜) facts.
    println!("overstory scaffold — read CLAUDE.md to begin.");
}
