//! Command palette (ADR-0009, layer 1): prefix + `:` on a session tab lists
//! that tool's ✅-verified native slash commands (`CliAdapter::command_table`)
//! and injects the selected one — command text plus carriage return — into
//! the pane through the same `write_input` path as ordinary keystrokes. The
//! tool's own UI takes over from there; swarm-tui never parses the response.
//!
//! State + pure helpers live here so filtering and byte-building stay
//! unit-testable without a terminal; drawing and key routing stay in
//! `app::mod` next to the other overlays.

use crate::adapters::NativeCommand;
use crate::pty::PaneId;

/// Open palette state. `selected` indexes the *filtered* view; it resets on
/// every filter edit so the cursor never points at a hidden entry.
pub struct PaletteState {
    pub pane_id: PaneId,
    pub tool_display: &'static str,
    pub entries: &'static [NativeCommand],
    pub filter: String,
    pub selected: usize,
    /// `Some` while the free-text argument line is open for an entry that
    /// declared an `args_hint`.
    pub args: Option<ArgsState>,
}

pub struct ArgsState {
    /// Index into `entries` (NOT the filtered view — the filter can change
    /// underneath while the args line is open).
    pub command_index: usize,
    pub text: String,
}

impl PaletteState {
    pub fn new(
        pane_id: PaneId,
        tool_display: &'static str,
        entries: &'static [NativeCommand],
    ) -> Self {
        PaletteState {
            pane_id,
            tool_display,
            entries,
            filter: String::new(),
            selected: 0,
            args: None,
        }
    }
}

/// Case-insensitive contains-filter over name + description; an empty filter
/// lists everything in table order.
pub fn filtered_indices(entries: &[NativeCommand], filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            needle.is_empty()
                || e.name.to_lowercase().contains(&needle)
                || e.description.to_lowercase().contains(&needle)
        })
        .map(|(i, _)| i)
        .collect()
}

/// The exact bytes the palette writes into the pane: the entry's `inject`
/// text, a single space plus args when given, then a carriage return — the
/// same byte `keys::encode_key_event` sends for Enter.
pub fn injection_bytes(cmd: &NativeCommand, args: Option<&str>) -> Vec<u8> {
    let mut out = Vec::from(cmd.inject.as_bytes());
    if let Some(args) = args {
        out.push(b' ');
        out.extend_from_slice(args.as_bytes());
    }
    out.push(b'\r');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &[NativeCommand] = &[
        NativeCommand {
            name: "/model",
            inject: "/model",
            description: "Set the AI model",
            args_hint: Some("model"),
            persists: true,
        },
        NativeCommand {
            name: "/status",
            inject: "/status",
            description: "Show status including MODEL info",
            args_hint: None,
            persists: false,
        },
        NativeCommand {
            name: "/rewind",
            inject: "/rewind",
            description: "Restore an earlier point",
            args_hint: None,
            persists: false,
        },
    ];

    #[test]
    fn empty_filter_lists_everything_in_table_order() {
        assert_eq!(filtered_indices(TABLE, ""), vec![0, 1, 2]);
    }

    #[test]
    fn filter_is_case_insensitive_over_name_and_description() {
        // "model" hits /model by name and /status by description.
        assert_eq!(filtered_indices(TABLE, "MoDeL"), vec![0, 1]);
        // name-only hit
        assert_eq!(filtered_indices(TABLE, "rew"), vec![2]);
        // no hits
        assert!(filtered_indices(TABLE, "zzz").is_empty());
    }

    #[test]
    fn injection_bytes_end_with_a_carriage_return() {
        assert_eq!(injection_bytes(&TABLE[1], None), b"/status\r".to_vec());
    }

    #[test]
    fn injection_bytes_join_args_with_a_single_space() {
        assert_eq!(
            injection_bytes(&TABLE[0], Some("opus")),
            b"/model opus\r".to_vec()
        );
    }

    /// End-to-end injection against a fake pane (`sh -c cat` — no wrapped CLI
    /// anywhere near this test): the exact bytes must arrive and echo.
    #[test]
    fn injected_bytes_arrive_in_a_fake_cat_pane() {
        use crate::pty::local::LocalPaneHost;
        use crate::pty::{PaneHost, PaneSize};
        use std::time::{Duration, Instant};

        let (mut host, _rx) = LocalPaneHost::new();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("cat");
        let pane = host.spawn(cmd, PaneSize { rows: 24, cols: 80 }).unwrap();

        host.write_input(pane, &injection_bytes(&TABLE[0], Some("opus")))
            .unwrap();

        let start = Instant::now();
        let mut seen = String::new();
        while start.elapsed() < Duration::from_secs(2) {
            seen = host.with_screen(pane, |s| s.contents()).unwrap();
            if seen.contains("/model opus") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            seen.contains("/model opus"),
            "pane never echoed the injected command; screen:\n{seen}"
        );
        host.kill(pane).unwrap();
    }
}
