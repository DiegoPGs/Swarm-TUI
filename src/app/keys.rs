//! Pure keystroke → byte-sequence encoding for forwarding into a wrapped
//! CLI's PTY (ADR-0007). `KeyEvent` in, raw bytes out — no PTY, no ratatui,
//! so this is unit-testable without spawning anything. `app/mod.rs`'s
//! prefix-key state machine is the only caller.
//!
//! xterm CSI sequences below match `TERM=xterm-256color`, which
//! `LocalPaneHost::spawn` always sets, cross-checked against the XTerm
//! Control Sequences reference (invisible-island.net/xterm/ctlseqs) and the
//! `xterm` terminfo entry. F5+ uses the VT220/xterm numeric CSI family
//! (F1-F4 stay on the classic `ESC O` SS3 forms; there is no F13/F14 gap to
//! worry about since F5 starts at `~15` and F11/F12 skip 16/22, matching
//! historical VT220 codes xterm still emits).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate one key event into the byte sequence a terminal program expects
/// on stdin. Best-effort: keys with no terminal encoding (media keys, lone
/// modifier presses, etc.) encode to an empty `Vec`.
pub fn encode_key_event(ev: &KeyEvent) -> Vec<u8> {
    if ev.modifiers.contains(KeyModifiers::ALT) {
        // Alt+key is ESC-prefixed on the key's own encoding (classic
        // "meta" xterm convention) — recurse with ALT stripped so the base
        // encoding below doesn't need to know about it.
        let mut without_alt = *ev;
        without_alt.modifiers.remove(KeyModifiers::ALT);
        let mut bytes = vec![0x1b];
        bytes.extend(encode_key_event(&without_alt));
        return bytes;
    }

    match ev.code {
        KeyCode::Char(c) => encode_char(c, ev.modifiers),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => encode_f_key(n),
        // crossterm represents the Ctrl-Space byte (0x00) as `Char(' ')` +
        // CONTROL on this platform (verified against crossterm 0.29's unix
        // parser), but keep `Null` mapped too for robustness across
        // backends/platforms that report it directly.
        KeyCode::Null => vec![0x00],
        _ => Vec::new(),
    }
}

fn encode_char(c: char, modifiers: KeyModifiers) -> Vec<u8> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match c {
            // Ctrl-Space: NUL. `app/mod.rs` intercepts this before it ever
            // reaches `encode_key_event` in the normal forwarding path (it's
            // the prefix key), but the byte is still correct here for the
            // ADR-0007 "Ctrl-Space twice" literal-forward escape hatch.
            ' ' => return vec![0x00],
            c if c.is_ascii_alphabetic() => {
                return vec![(c.to_ascii_lowercase() as u8) - b'a' + 1];
            }
            _ => {}
        }
    }
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

fn encode_f_key(n: u8) -> Vec<u8> {
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_letter_encodes_to_its_utf8_bytes() {
        let ev = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(encode_key_event(&ev), b"a".to_vec());
    }

    #[test]
    fn enter_encodes_to_carriage_return() {
        let ev = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(encode_key_event(&ev), vec![b'\r']);
    }

    #[test]
    fn backspace_encodes_to_del() {
        let ev = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(encode_key_event(&ev), vec![0x7f]);
    }

    #[test]
    fn esc_encodes_to_escape_byte() {
        let ev = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(encode_key_event(&ev), vec![0x1b]);
    }

    #[test]
    fn up_arrow_encodes_to_xterm_csi_a() {
        let ev = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(encode_key_event(&ev), b"\x1b[A".to_vec());
    }

    #[test]
    fn ctrl_c_encodes_to_0x03() {
        let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(encode_key_event(&ev), vec![0x03]);
    }

    #[test]
    fn ctrl_space_encodes_to_nul() {
        let ev = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
        assert_eq!(encode_key_event(&ev), vec![0x00]);
    }

    #[test]
    fn null_keycode_also_encodes_to_nul() {
        let ev = KeyEvent::new(KeyCode::Null, KeyModifiers::NONE);
        assert_eq!(encode_key_event(&ev), vec![0x00]);
    }

    #[test]
    fn alt_key_is_esc_prefixed() {
        let ev = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(encode_key_event(&ev), vec![0x1b, b'x']);
    }

    #[test]
    fn f_keys_use_the_documented_xterm_split() {
        assert_eq!(
            encode_key_event(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            b"\x1bOP".to_vec()
        );
        assert_eq!(
            encode_key_event(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            b"\x1b[15~".to_vec()
        );
        assert_eq!(
            encode_key_event(&KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE)),
            b"\x1b[24~".to_vec()
        );
    }
}
