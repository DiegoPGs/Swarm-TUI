//! Shared vocabulary: sessions, tasks, normalized events, app config.
//!
//! Dependency direction (ADR-0006): `adapters`, `app`, `pty`, and `store` all
//! depend on `core`; `core` depends on nothing in this crate and contains
//! **zero** CLI-specific knowledge. Note: because this module is named `core`,
//! in-crate paths must be written `crate::core::…` to avoid colliding with the
//! standard `core` crate.

pub mod config;
pub mod events;
pub mod plan;
pub mod redact;
pub mod session;
pub mod task;
