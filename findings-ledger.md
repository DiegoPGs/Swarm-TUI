# Findings ledger — 2026-07-17 audit fixes

Five findings from the 2026-07 static audit (which could not compile the tree).
Each entry tracks status `OPEN → VERIFIED-CLOSED` with the command output that
closed it. Toolchain used for verification: rustc 1.97.1 stable (plus 1.94.1 /
1.95.0 / 1.96.0 for the F-005 MSRV bisect).

Full-suite verification (run after all fixes, rustc 1.97.1):

```
$ cargo fmt --check          # exit 0, no output
$ cargo clippy --all-targets -- -D warnings
    Checking swarm-tui v0.0.1 (/home/user/Swarm-TUI)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.83s
$ cargo test
test result: ok. 88 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.96s
```

---

## F-001 (Medium) — concurrent-instance session clobbering

**Status: VERIFIED-CLOSED**

- Where: `src/store/mod.rs` — `allocate_id()` (`SELECT MAX(id)+1`) followed by
  `upsert()`'s `INSERT … ON CONFLICT(id) DO UPDATE`.
- Fix: replaced the racy allocate-then-upsert pair with `Registry::create()`,
  which INSERTs without an id and lets SQLite assign it atomically under its
  write lock (`last_insert_rowid()` is returned). Two writers can no longer
  land on the same id; the second insert gets the next id instead of
  overwriting the first row. `upsert()` is unchanged and still updates
  existing rows in place (status updates on close, `upsert_updates_in_place_and_keeps_created_at`
  still passes). `allocate_id()` removed; its one production caller
  (`App::open_new_session`) now uses `create()`.
- Closing evidence — new test opening two `Registry` handles on one temp DB
  file, each creating a fresh session, asserting two distinct surviving rows:

```
$ cargo test two_handles_creating_fresh_sessions_never_clobber
test store::tests::two_handles_creating_fresh_sessions_never_clobber ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 87 filtered out; finished in 0.04s
```

## F-002 (Low-Medium) — session-close status write silently dropped

**Status: VERIFIED-CLOSED**

- Where: `src/app/mod.rs`, `perform_close_active_tab()` (`let _ = self.registry.upsert(record)`
  and the `if let Ok(…) = self.registry.all()` that skipped read errors).
- Fix: both failure branches now emit `tracing::warn!` with the session id and
  the error; the close path stays non-fatal (tab still closes).
- Closing evidence — two new tests drive the real close path against a
  registry broken underneath the app (an UPDATE-blocking trigger simulating a
  read-only file, and a dropped table for the read branch) with a capturing
  tracing subscriber. Observed warning from the failing run during
  development (proving the emission, before the assertion was adjusted for
  ANSI codes):

```
WARN swarm_tui::app: failed to persist close status; registry row left as-is session_id=1 error=Query("registry is read-only")
```

```
$ cargo test close_warns
test app::tests::close_warns_when_the_status_write_fails ... ok
test app::tests::close_warns_when_the_registry_read_fails ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 86 filtered out; finished in 0.16s
```

## F-003 (Low) — mutex poison cascade in the PTY host

**Status: VERIFIED-CLOSED**

- Where: `src/pty/local.rs` — every `lock().unwrap()` on `parser`, `child`,
  and `exit_success` (reader thread, `with_screen`, `resize`, `kill`,
  `exit_success`).
- Fix: added `lock_ignore_poison()` (`lock().unwrap_or_else(PoisonError::into_inner)`)
  and routed all seven lock sites through it. Locking structure otherwise
  unchanged; a poisoned pane now degrades that pane instead of panicking
  every later lock site.
- Closing evidence — new test poisons one pane's parser mutex via a panicking
  thread, then exercises `with_screen`/`resize` on a second pane (and
  `with_screen` on the poisoned pane) without panics:

```
$ cargo test poisoned_pane_lock_degrades_that_pane_only
test pty::local::tests::poisoned_pane_lock_degrades_that_pane_only ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 87 filtered out; finished in 0.24s
```

## F-004 (Low) — timestamp helpers panic on boundary values

**Status: VERIFIED-CLOSED**

- Where: `src/store/mod.rs` — `system_time_to_unix()` (`duration_since(…).unwrap()`)
  and `unix_to_system_time()` (`v as u64` then unchecked `SystemTime` addition).
- Fix: write side clamps a pre-epoch `SystemTime` to `0`; read side (the real
  boundary — the DB file) clamps negative values via `u64::try_from` and
  overflow via `UNIX_EPOCH.checked_add`, both landing on `UNIX_EPOCH` instead
  of panicking.
- Closing evidence — unit tests for both helpers plus an end-to-end read of a
  raw row persisted with negative timestamps:

```
$ cargo test pre_epoch
test store::tests::pre_epoch_system_time_clamps_to_epoch_on_write ... ok
test result: ok. 1 passed; 0 failed; …
$ cargo test timestamp
test store::tests::out_of_range_stored_timestamp_clamps_instead_of_panicking ... ok
test store::tests::negative_stored_timestamps_read_back_clamped ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 86 filtered out; finished in 0.03s
```

## F-005 (Low) — declared MSRV below what dependencies require

**Status: VERIFIED-CLOSED**

- Where: `Cargo.toml` `rust-version = "1.75"`.
- Fix: set `rust-version = "1.95"`, the lowest rustc on which `cargo check`
  actually succeeds for this tree. The binding constraint is not ratatui's
  declared 1.88 but `libsqlite3-sys 0.38.1`, whose build script uses
  `cfg_select!` (stable from 1.95).
- Closing evidence — bisect over installed toolchains:

```
$ cargo +1.94.1 check        # (session default before update)
error[E0658]: use of unstable library feature `cfg_select`
   --> …/libsqlite3-sys-0.38.1/build.rs:110:9
error: could not compile `libsqlite3-sys` (build script) due to 1 previous error

$ cargo +1.95.0 check
    Checking swarm-tui v0.0.1 (/home/user/Swarm-TUI)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.30s

$ cargo +1.96.0 check
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.58s
```
