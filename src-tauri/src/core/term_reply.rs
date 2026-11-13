//! Recognises terminal reply sequences: answers the shell (or a full-screen program) sends to the
//! queries the frontend's xterm.js emits (device attributes, cursor position, colour queries, focus reports).
//!
//! The frontend forwards those bytes to the PTY through `write_user`, so they must not count as the user
//! typing: they neither set the typed-text flag nor close the submit gate, and they do not bump `user_writes`
//! (which ends the agent's `send_keys` approval). Pure: no session, no I/O.
//!
//! `is_terminal_reply` accepts one or more concatenated known replies and requires the whole slice to be
//! consumed, so `reply + text` and `text + reply` are not replies. Anything else (typed text, arrows, SGR,
//! mode sets, OSC titles, partial sequences) is not a reply.

const ESC: u8 = 0x1b;

/// True when `bytes` is one or more concatenated terminal reply sequences and nothing else. Empty input is
/// not a reply.
pub fn is_terminal_reply(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        match one_reply(&bytes[i..]) {
            Some(len) => i += len,
            None => return false,
        }
    }
    true
}

/// The length of the single reply sequence at the start of `bytes`, or None when it starts with anything else.
fn one_reply(bytes: &[u8]) -> Option<usize> {
    if bytes.len() >= 3 && bytes[0] == ESC && bytes[1] == b'[' && (bytes[2] == b'I' || bytes[2] == b'O') {
        // Focus in/out: `ESC [ I` / `ESC [ O`. No parameters, so no digits to check.
        return Some(3);
    }
    if bytes.len() >= 2 && bytes[0] == ESC && bytes[1] == b'[' {
        return csi_reply(&bytes[2..]).map(|len| len + 2);
    }
    if bytes.len() >= 2 && bytes[0] == ESC && bytes[1] == b']' {
        return osc_reply(&bytes[2..]).map(|len| len + 2);
    }
    None
}

/// A CSI reply after the `ESC [` prefix. Returns the bytes consumed past the prefix.
fn csi_reply(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    // The optional private/intermediate prefix. Only `?` and `>` open a reply sequence.
    let prefix = match bytes.first() {
        Some(b'?') | Some(b'>') => {
            i += 1;
            Some(bytes[0])
        }
        _ => None,
    };
    // The parameters: digits and `;` only, with at least one digit.
    let mut digits = 0;
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_digit() {
            digits += 1;
            i += 1;
        } else if b == b';' {
            i += 1;
        } else {
            break;
        }
    }
    if digits == 0 {
        return None;
    }
    match (prefix, bytes.get(i)) {
        // Cursor position report: `ESC [ row ; col R`. No prefix.
        (None, Some(b'R')) => Some(i + 1),
        // Device attributes: `ESC [ [?|>] ... c`.
        (None, Some(b'c')) | (Some(b'?'), Some(b'c')) | (Some(b'>'), Some(b'c')) => Some(i + 1),
        // Device status report: `ESC [ [?] ... n`.
        (None, Some(b'n')) | (Some(b'?'), Some(b'n')) => Some(i + 1),
        // Window report: `ESC [ ... t`. No prefix.
        (None, Some(b't')) => Some(i + 1),
        // DECRPM (mode report): `ESC [ ? ... $ y`.
        (Some(b'?'), Some(b'$')) if bytes.get(i + 1) == Some(&b'y') => Some(i + 2),
        _ => None,
    }
}

/// An OSC colour reply after the `ESC ]` prefix: `4`, `10` or `11`, then `;`, a printable body, and BEL or
/// ST (`ESC \`). Returns the bytes consumed past the prefix. Titles (`ESC ] 0 ; ...`) are not replies.
fn osc_reply(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    if bytes.first() == Some(&b'4') {
        i += 1;
    } else if bytes.first() == Some(&b'1') && (bytes.get(1) == Some(&b'0') || bytes.get(1) == Some(&b'1')) {
        i += 2;
    } else {
        return None;
    }
    if bytes.get(i) != Some(&b';') {
        return None;
    }
    i += 1;
    loop {
        match bytes.get(i) {
            // Printable body (letters, digits, `;`, `:`, `/`, `?`, ...). Control bytes end the match as a failure.
            Some(b) if (0x20..=0x7e).contains(b) => i += 1,
            // BEL terminates the reply.
            Some(0x07) => return Some(i + 1),
            // ST (`ESC \`) terminates the reply.
            Some(0x1b) if bytes.get(i + 1) == Some(&b'\\') => return Some(i + 2),
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(s: &str) -> Vec<u8> {
        s.replace('@', "\x1b").into_bytes()
    }

    #[test]
    fn cursor_position_reports_are_replies() {
        assert!(is_terminal_reply(&esc("@[24;80R")));
        assert!(is_terminal_reply(&esc("@[1;1R")));
        assert!(is_terminal_reply(&esc("@[999R")));
    }

    #[test]
    fn device_attributes_replies_are_replies() {
        assert!(is_terminal_reply(&esc("@[?1;2c")));
        assert!(is_terminal_reply(&esc("@[>0;95;0c")));
        assert!(is_terminal_reply(&esc("@[1;2c")));
    }

    #[test]
    fn device_status_replies_are_replies() {
        assert!(is_terminal_reply(&esc("@[0n")));
        assert!(is_terminal_reply(&esc("@[?15n")));
    }

    #[test]
    fn mode_reports_are_replies() {
        assert!(is_terminal_reply(&esc("@[?25;1$y")));
        assert!(is_terminal_reply(&esc("@[?1;2$y")));
    }

    #[test]
    fn window_reports_are_replies() {
        assert!(is_terminal_reply(&esc("@[8;24;80t")));
        assert!(is_terminal_reply(&esc("@[4;620;600t")));
    }

    #[test]
    fn focus_reports_are_replies() {
        assert!(is_terminal_reply(&esc("@[I")));
        assert!(is_terminal_reply(&esc("@[O")));
    }

    #[test]
    fn colour_replies_are_replies() {
        assert!(is_terminal_reply(&esc("@]11;rgb:0000/0000/0000\x07")));
        assert!(is_terminal_reply(&esc("@]10;?\x07")));
        assert!(is_terminal_reply(&esc("@]4;1;rgb:ffff/0000/0000\x07")));
        assert!(is_terminal_reply(&esc("@]11;rgb:0000/0000/0000@\\")));
    }

    #[test]
    fn concatenated_replies_are_one_reply() {
        assert!(is_terminal_reply(&esc("@[24;80R@[I")));
        assert!(is_terminal_reply(&esc("@[?1;2c@[0n@[?25;1$y")));
    }

    #[test]
    fn text_is_not_a_reply() {
        assert!(!is_terminal_reply(b""));
        assert!(!is_terminal_reply(b"hello"));
        assert!(!is_terminal_reply(b"ls\r"));
        assert!(!is_terminal_reply(&[0x03]));
        assert!(!is_terminal_reply(&esc("@[24;80Rhello")), "a reply plus text is not a reply");
        assert!(!is_terminal_reply(&esc("x@[24;80R")), "text plus a reply is not a reply");
    }

    #[test]
    fn lookalike_sequences_are_not_replies() {
        assert!(!is_terminal_reply(&esc("@[A")), "an arrow is typed text, not a reply");
        assert!(!is_terminal_reply(&esc("@[31m")), "SGR is output styling, not a reply");
        assert!(!is_terminal_reply(&esc("@[?25h")), "a mode set is not a reply");
        assert!(!is_terminal_reply(&esc("@[?25l")), "a mode reset is not a reply");
        assert!(!is_terminal_reply(&esc("@]0;title\x07")), "a title set is not a reply");
        assert!(!is_terminal_reply(&esc("@[25;1$y")), "DECRPM needs the `?` prefix");
        assert!(!is_terminal_reply(&esc("@[>1;2n")), "`>` opens only DA");
        assert!(!is_terminal_reply(&esc("@[24;80t;x")), "junk after a reply is not a reply");
    }

    #[test]
    fn partial_sequences_are_not_replies() {
        assert!(!is_terminal_reply(&esc("@")));
        assert!(!is_terminal_reply(&esc("@[")));
        assert!(!is_terminal_reply(&esc("@[24;8")));
        assert!(!is_terminal_reply(&esc("@[?25;1$")));
        assert!(!is_terminal_reply(&esc("@]11;rgb:0000")));
        assert!(!is_terminal_reply(&esc("@]11;rgb:0000@")));
    }
}
