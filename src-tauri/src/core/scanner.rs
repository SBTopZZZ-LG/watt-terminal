//! Streaming parser for shell integration events (OSC 133/633) and OSC 7 (cwd).
//!
//! Trust model: an OSC 133/633 sequence is an event only if field 2 is `n=<nonce>`, where the nonce
//! is random per session and sent to the shell in an environment variable. Anything else is counted
//! as a forged event and ignored. This stops stray output (for example `cat` of a file containing
//! escape codes) from faking prompts or command boundaries. It does not stop code running in the
//! pane, which can read the nonce from the shell's own events. That is accepted for slice 1.
//!
//! OSC 7 is not nonce-tagged: it can only change the displayed cwd. Titles (OSC 0/1/2) are ignored.
//! The parser keeps its state between `feed` calls, so sequences split across PTY reads still parse.

/// OSC payloads longer than this are discarded. Integration events are much shorter.
pub const OSC_CAP: usize = 4096;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 633;Hello: the shell finished loading the integration.
    Hello { pid: u32 },
    /// 133;A: prompt start.
    PromptStart,
    /// 133;P: continuation prompt (PS2), zsh is waiting for more input.
    Continuation,
    /// 633;E: the command line about to run (escaped by the shell, unescaped here).
    CommandLine(String),
    /// 133;C: command start.
    CommandStart,
    /// 133;D: command finished.
    CommandEnd { exit: i32, pipestatus: Vec<i32> },
    /// OSC 7: working directory.
    Cwd { host: String, path: String },
    /// CSI ... r (DECSTBM). vt100 does not report rows scrolled inside a region, so records are marked approximate.
    ScrollRegion,
}

/// An event and the index just past its terminator, relative to the slice given to `feed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub end: usize,
    pub event: Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Osc,
    OscEsc,
    /// `plain` is true while the CSI parameters are only digits and ';' (no private or intermediate bytes).
    Csi { plain: bool },
}

pub struct Scanner {
    /// The exact field that must appear at index 2 of a tagged event: `n=<nonce>`.
    nonce_field: Vec<u8>,
    state: State,
    buf: Vec<u8>,
    osc_overflow: bool,
    forged: u64,
}

impl Scanner {
    pub fn new(nonce: &str) -> Self {
        Scanner {
            nonce_field: format!("n={nonce}").into_bytes(),
            state: State::Ground,
            buf: Vec::new(),
            osc_overflow: false,
            forged: 0,
        }
    }

    /// Number of 133/633 sequences rejected for a missing or wrong nonce.
    pub fn forged_events(&self) -> u64 {
        self.forged
    }

    /// Drops any partial sequence. Used after the observer lost bytes, so a half-read sequence is not completed by later data.
    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.buf.clear();
        self.osc_overflow = false;
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Found> {
        let mut out = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            if let Some(event) = self.step(b) {
                out.push(Found { end: i + 1, event });
            }
        }
        out
    }

    fn step(&mut self, b: u8) -> Option<Event> {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
                None
            }
            State::Esc => {
                self.state = match b {
                    b']' => {
                        self.buf.clear();
                        self.osc_overflow = false;
                        State::Osc
                    }
                    b'[' => State::Csi { plain: true },
                    ESC => State::Esc,
                    _ => State::Ground,
                };
                None
            }
            State::Osc => match b {
                BEL => {
                    self.state = State::Ground;
                    self.finish_osc()
                }
                ESC => {
                    self.state = State::OscEsc;
                    None
                }
                _ => {
                    if self.buf.len() < OSC_CAP {
                        self.buf.push(b);
                    } else {
                        self.osc_overflow = true;
                    }
                    None
                }
            },
            State::OscEsc => match b {
                b'\\' => {
                    self.state = State::Ground;
                    self.finish_osc()
                }
                ESC => {
                    self.state = State::Esc;
                    None
                }
                _ => {
                    self.state = State::Ground;
                    None
                }
            },
            State::Csi { plain } => match b {
                ESC => {
                    self.state = State::Esc;
                    None
                }
                0x30..=0x3f => {
                    let param = b.is_ascii_digit() || b == b';';
                    self.state = State::Csi { plain: plain && param };
                    None
                }
                0x20..=0x2f => {
                    self.state = State::Csi { plain: false };
                    None
                }
                0x40..=0x7e => {
                    self.state = State::Ground;
                    (plain && b == b'r').then_some(Event::ScrollRegion)
                }
                _ => None,
            },
        }
    }

    fn finish_osc(&mut self) -> Option<Event> {
        let payload = std::mem::take(&mut self.buf);
        if self.osc_overflow {
            self.osc_overflow = false;
            return None;
        }
        let (code, rest) = split_first(&payload, b';');
        match code {
            b"7" => parse_cwd(rest),
            b"133" | b"633" => self.parse_integration(&payload),
            _ => None,
        }
    }

    /// `payload` is the whole OSC body, e.g. `133;D;n=<nonce>;exit=0;ps=0`.
    fn parse_integration(&mut self, payload: &[u8]) -> Option<Event> {
        let fields: Vec<&[u8]> = payload.split(|&c| c == b';').collect();
        if fields.len() < 3 || fields[2] != self.nonce_field.as_slice() {
            self.forged += 1;
            return None;
        }
        match (fields[0], fields[1]) {
            (b"133", b"A") => Some(Event::PromptStart),
            (b"133", b"P") => Some(Event::Continuation),
            (b"133", b"C") => Some(Event::CommandStart),
            (b"133", b"D") => parse_command_end(&fields[3..]),
            (b"633", b"E") => Some(Event::CommandLine(unescape_e(&join_fields(&fields[3..])))),
            (b"633", b"Hello") => parse_hello(&fields[3..]),
            // B (prompt end) and Degraded carry no state change for the observer.
            _ => None,
        }
    }
}

fn split_first(s: &[u8], sep: u8) -> (&[u8], &[u8]) {
    match s.iter().position(|&c| c == sep) {
        Some(p) => (&s[..p], &s[p + 1..]),
        None => (s, &[]),
    }
}

fn join_fields(fields: &[&[u8]]) -> Vec<u8> {
    fields.join(&b';')
}

fn field_value<'a>(fields: &[&'a [u8]], key: &[u8]) -> Option<&'a [u8]> {
    fields.iter().find_map(|f| f.strip_prefix(key))
}

fn parse_command_end(fields: &[&[u8]]) -> Option<Event> {
    let exit: i32 = std::str::from_utf8(field_value(fields, b"exit=")?).ok()?.parse().ok()?;
    let pipestatus = field_value(fields, b"ps=")
        .and_then(|v| std::str::from_utf8(v).ok())
        .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default();
    Some(Event::CommandEnd { exit, pipestatus })
}

fn parse_hello(fields: &[&[u8]]) -> Option<Event> {
    if field_value(fields, b"v=")? != b"1" {
        return None;
    }
    let pid = std::str::from_utf8(field_value(fields, b"pid=")?).ok()?.parse().ok()?;
    Some(Event::Hello { pid })
}

/// `7;file://HOST/PATH` (payload after the code), path percent-encoded.
fn parse_cwd(rest: &[u8]) -> Option<Event> {
    let url = rest.strip_prefix(b"file://")?;
    let slash = url.iter().position(|&c| c == b'/')?;
    let host = String::from_utf8_lossy(&url[..slash]).into_owned();
    let path = String::from_utf8_lossy(&percent_decode(&url[slash..])).into_owned();
    Some(Event::Cwd { host, path })
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn percent_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

/// Reverses the shell's E-payload escaping: `\\` for backslash and `\xHH` for any byte.
fn unescape_e(s: &[u8]) -> String {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' {
            if s.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            if s.get(i + 1) == Some(&b'x') {
                if let (Some(h), Some(l)) = (s.get(i + 2).and_then(|&c| hex(c)), s.get(i + 3).and_then(|&c| hex(c))) {
                    out.push(h << 4 | l);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(s[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "00112233445566778899aabbccddeeff";

    fn osc(body: &str) -> Vec<u8> {
        format!("\x1b]{body}\x1b\\").into_bytes()
    }

    fn events(bytes: &[u8]) -> Vec<Event> {
        Scanner::new(NONCE).feed(bytes).into_iter().map(|f| f.event).collect()
    }

    #[test]
    fn tagged_events_are_parsed() {
        let mut s = Vec::new();
        s.extend(osc(&format!("133;A;n={NONCE}")));
        s.extend(osc(&format!("633;E;n={NONCE};echo a\\x3bb")));
        s.extend(osc(&format!("133;C;n={NONCE}")));
        s.extend(osc(&format!("133;D;n={NONCE};exit=3;ps=0,3")));
        s.extend(osc(&format!("633;Hello;n={NONCE};v=1;zsh=5.9;pid=4242")));
        assert_eq!(
            events(&s),
            vec![
                Event::PromptStart,
                Event::CommandLine("echo a;b".into()),
                Event::CommandStart,
                Event::CommandEnd { exit: 3, pipestatus: vec![0, 3] },
                Event::Hello { pid: 4242 },
            ]
        );
    }

    #[test]
    fn untagged_and_wrong_nonce_are_forged() {
        let mut sc = Scanner::new(NONCE);
        let found = sc.feed(&osc("133;D;0"));
        assert!(found.is_empty());
        let found = sc.feed(&osc("133;D;n=ffffffffffffffffffffffffffffffff;exit=0"));
        assert!(found.is_empty());
        assert_eq!(sc.forged_events(), 2);
    }

    #[test]
    fn split_at_every_position_gives_the_same_events() {
        let mut s = Vec::new();
        s.extend(b"junk before ");
        s.extend(osc(&format!("633;E;n={NONCE};ls -l")));
        s.extend(b"output\r\n");
        s.extend(osc(&format!("133;D;n={NONCE};exit=1;ps=1")));
        let whole = events(&s);
        assert_eq!(whole.len(), 2);
        for cut in 0..s.len() {
            let mut sc = Scanner::new(NONCE);
            let mut got: Vec<Event> = sc.feed(&s[..cut]).into_iter().map(|f| f.event).collect();
            got.extend(sc.feed(&s[cut..]).into_iter().map(|f| f.event));
            assert_eq!(got, whole, "cut at {cut}");
        }
    }

    #[test]
    fn oversized_osc_is_dropped_and_scanning_recovers() {
        let mut s = format!("\x1b]633;E;n={NONCE};").into_bytes();
        s.extend(std::iter::repeat_n(b'x', OSC_CAP + 10));
        s.push(BEL);
        s.extend(osc(&format!("133;C;n={NONCE}")));
        assert_eq!(events(&s), vec![Event::CommandStart]);
    }

    #[test]
    fn cwd_is_decoded_and_not_nonce_checked() {
        let s = osc("7;file://mac.local/tmp/a%20b");
        assert_eq!(events(&s), vec![Event::Cwd { host: "mac.local".into(), path: "/tmp/a b".into() }]);
    }

    #[test]
    fn scroll_region_is_detected_but_private_modes_are_not() {
        assert_eq!(events(b"\x1b[1;24r"), vec![Event::ScrollRegion]);
        assert_eq!(events(b"\x1b[r"), vec![Event::ScrollRegion]);
        assert!(events(b"\x1b[?1049h").is_empty());
        assert!(events(b"\x1b[2;3H").is_empty());
    }

    #[test]
    fn e_unescape_handles_all_escapes() {
        assert_eq!(unescape_e(br"a\\b\x3bc\x0ad\x07e"), "a\\b;c\nd\x07e");
        assert_eq!(unescape_e(br"\x1b[31m"), "\x1b[31m");
    }
}
