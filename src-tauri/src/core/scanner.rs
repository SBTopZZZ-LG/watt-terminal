//! Streaming parser for shell integration events (OSC 133/633) and OSC 7 (cwd).
//!
//! Trust model: an OSC 133/633 sequence is an event only if field 2 is `n=<nonce>`, where the nonce
//! is random per session and sent to the shell in an environment variable. Anything else is counted
//! as a forged event and ignored. This stops stray output (for example `cat` of a file containing
//! escape codes) from faking prompts or command boundaries. It does not stop code running in the
//! pane, which can read the nonce from the shell's own events. That is accepted for slice 1.
//!
//! The scanner accepts a set of nonces, one per shell level (level 0 is the local shell's session
//! nonce). Each accepted nonce carries its level key, and every Found says which level tagged it. A
//! nonce that was accepted and later retired (its level was popped) counts as stale, not forged: a
//! late event from a shell that has gone is expected. A nonce never seen is forged.
//!
//! OSC 7 is not nonce-tagged: it can only change the displayed cwd. Titles (OSC 0/1/2) are ignored.
//! The parser keeps its state between `feed` calls, so sequences split across PTY reads still parse.

use std::collections::VecDeque;

/// OSC payloads longer than this are discarded. Integration events are much shorter.
pub const OSC_CAP: usize = 4096;
/// Retired nonces remembered for the stale count. The oldest are forgotten first.
pub const RETIRED_CAP: usize = 16;
/// Longest value kept from a 633;Hello text field (sh, ver, user, host). Longer values are cut.
pub const HELLO_FIELD_CAP: usize = 255;
/// Longest token or shell name taken from a 633;Ready. A longer value, or any byte outside [A-Za-z0-9], drops the Ready.
pub const READY_FIELD_CAP: usize = 64;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 633;Hello: the shell finished loading the integration. The text fields are optional and unknown ones are ignored.
    Hello {
        pid: u32,
        sh: Option<String>,
        ver: Option<String>,
        user: Option<String>,
        host: Option<String>,
    },
    /// 633;Ready (untagged): the loader has checked the shell and is waiting for its blob. `sh` is `zsh` or `bash`.
    Ready { token: String, sh: String },
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
    /// 633;Cwd (tagged): the working directory of a nested level, escaped like E.
    LevelCwd { path: String },
    /// OSC 7: working directory.
    Cwd { host: String, path: String },
    /// CSI ... r (DECSTBM). vt100 does not report rows scrolled inside a region, so records are marked approximate.
    ScrollRegion,
}

/// An event and the index just past its terminator, relative to the slice given to `feed`. `level` is the key of the
/// level whose nonce tagged the event. None for untagged events (OSC 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub end: usize,
    pub event: Event,
    pub level: Option<u64>,
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
    /// Accepted nonces, each with its level key. Key 0 is the local shell's session nonce.
    accepted: Vec<(u64, String)>,
    /// Nonces of popped levels, most recent last. Bounded by `RETIRED_CAP`.
    retired: VecDeque<String>,
    state: State,
    buf: Vec<u8>,
    osc_overflow: bool,
    forged: u64,
    stale: u64,
}

impl Scanner {
    pub fn new(nonce: &str) -> Self {
        Scanner {
            accepted: vec![(0, nonce.to_string())],
            retired: VecDeque::new(),
            state: State::Ground,
            buf: Vec::new(),
            osc_overflow: false,
            forged: 0,
            stale: 0,
        }
    }

    /// Accepts `nonce` as the tag of level `key`. Keys are never reused.
    pub fn add_nonce(&mut self, key: u64, nonce: &str) {
        self.accepted.push((key, nonce.to_string()));
    }

    /// Stops accepting the nonce of level `key`. Its late events are then counted as stale.
    pub fn retire_nonce(&mut self, key: u64) {
        if let Some(pos) = self.accepted.iter().position(|(k, _)| *k == key) {
            let (_, nonce) = self.accepted.remove(pos);
            self.retired.push_back(nonce);
            while self.retired.len() > RETIRED_CAP {
                self.retired.pop_front();
            }
        }
    }

    /// The nonces currently accepted (the local one, the live levels and any pending one). Retired ones are left out.
    pub fn accepted_nonces(&self) -> Vec<String> {
        self.accepted.iter().map(|(_, n)| n.clone()).collect()
    }

    /// Number of 133/633 sequences rejected for a missing or wrong nonce.
    pub fn forged_events(&self) -> u64 {
        self.forged
    }

    /// Number of 133/633 sequences tagged with a retired nonce: late events from a popped level.
    pub fn stale_events(&self) -> u64 {
        self.stale
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
            if let Some((event, level)) = self.step(b) {
                out.push(Found { end: i + 1, event, level });
            }
        }
        out
    }

    fn step(&mut self, b: u8) -> Option<(Event, Option<u64>)> {
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
                    (plain && b == b'r').then_some((Event::ScrollRegion, None))
                }
                _ => None,
            },
        }
    }

    fn finish_osc(&mut self) -> Option<(Event, Option<u64>)> {
        let payload = std::mem::take(&mut self.buf);
        if self.osc_overflow {
            self.osc_overflow = false;
            return None;
        }
        let (code, rest) = split_first(&payload, b';');
        match code {
            b"7" => parse_cwd(rest).map(|e| (e, None)),
            b"133" | b"633" => self.parse_integration(&payload),
            _ => None,
        }
    }

    /// `payload` is the whole OSC body, e.g. `133;D;n=<nonce>;exit=0;ps=0`. The nonce at field 2 picks the level:
    /// accepted gives its key, retired counts as stale, anything else is forged.
    fn parse_integration(&mut self, payload: &[u8]) -> Option<(Event, Option<u64>)> {
        let fields: Vec<&[u8]> = payload.split(|&c| c == b';').collect();
        // Ready is untagged: the loader speaks before the shell has any nonce. It is checked by charset, never counted forged.
        if fields.len() >= 2 && fields[0] == b"633" && fields[1] == b"Ready" {
            return parse_ready(&fields[2..]).map(|e| (e, None));
        }
        let Some(nonce) = fields.get(2).and_then(|f| f.strip_prefix(b"n=")) else {
            self.forged += 1;
            return None;
        };
        let level = match self.accepted.iter().find(|(_, n)| n.as_bytes() == nonce) {
            Some((key, _)) => *key,
            None if self.retired.iter().any(|n| n.as_bytes() == nonce) => {
                self.stale += 1;
                return None;
            }
            None => {
                self.forged += 1;
                return None;
            }
        };
        let event = match (fields[0], fields[1]) {
            (b"133", b"A") => Event::PromptStart,
            (b"133", b"P") => Event::Continuation,
            (b"133", b"C") => Event::CommandStart,
            (b"133", b"D") => parse_command_end(&fields[3..])?,
            (b"633", b"E") => Event::CommandLine(unescape_e(&join_fields(&fields[3..]))),
            (b"633", b"Hello") => parse_hello(&fields[3..])?,
            (b"633", b"Cwd") => Event::LevelCwd { path: unescape_e(&join_fields(&fields[3..])) },
            // B (prompt end) and Degraded carry no state change for the observer.
            _ => return None,
        };
        Some((event, Some(level)))
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
    Some(Event::Hello {
        pid,
        sh: text_field(fields, b"sh="),
        ver: text_field(fields, b"ver="),
        user: text_field(fields, b"user="),
        host: text_field(fields, b"host="),
    })
}

/// `633;Ready;t=<token>;sh=<name>`. Both fields are required and must pass `ready_word`.
fn parse_ready(fields: &[&[u8]]) -> Option<Event> {
    let token = ready_word(field_value(fields, b"t=")?)?;
    let sh = ready_word(field_value(fields, b"sh=")?)?;
    Some(Event::Ready { token, sh })
}

/// A Ready field: 1 to `READY_FIELD_CAP` bytes, each [A-Za-z0-9]. Anything else is not a Ready.
fn ready_word(v: &[u8]) -> Option<String> {
    let ok = !v.is_empty() && v.len() <= READY_FIELD_CAP && v.iter().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| String::from_utf8_lossy(v).into_owned())
}

/// An optional text field of a Hello, cut to `HELLO_FIELD_CAP` bytes.
fn text_field(fields: &[&[u8]], key: &[u8]) -> Option<String> {
    let value = field_value(fields, key)?;
    let kept = &value[..value.len().min(HELLO_FIELD_CAP)];
    Some(String::from_utf8_lossy(kept).into_owned())
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
        if s[i] == b'%'
            && i + 2 < s.len()
            && let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
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
            if s.get(i + 1) == Some(&b'x')
                && let (Some(h), Some(l)) = (s.get(i + 2).and_then(|&c| hex(c)), s.get(i + 3).and_then(|&c| hex(c)))
            {
                out.push(h << 4 | l);
                i += 4;
                continue;
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
                Event::Hello { pid: 4242, sh: None, ver: None, user: None, host: None },
            ]
        );
    }

    /// The zsh shim's Hello (no sh/ver/user/host) and the new optional fields, with unknown fields ignored.
    #[test]
    fn hello_fields_are_optional_and_capped() {
        let long = "h".repeat(HELLO_FIELD_CAP + 40);
        let body = format!("633;Hello;n={NONCE};v=1;sh=bash;ver=5.2;user=ubuntu;host={long};pid=77;extra=zzz");
        let got = events(&osc(&body));
        assert_eq!(
            got,
            vec![Event::Hello {
                pid: 77,
                sh: Some("bash".into()),
                ver: Some("5.2".into()),
                user: Some("ubuntu".into()),
                host: Some("h".repeat(HELLO_FIELD_CAP)),
            }]
        );
        assert!(events(&osc(&format!("633;Hello;n={NONCE};v=2;pid=1"))).is_empty(), "other protocol versions are ignored");
    }

    #[test]
    fn level_cwd_is_tagged_and_escaped_like_e() {
        let mut sc = Scanner::new(NONCE);
        let found = sc.feed(&osc(&format!("633;Cwd;n={NONCE};/srv/a\\x3bb")));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].event, Event::LevelCwd { path: "/srv/a;b".into() });
        assert_eq!(found[0].level, Some(0));
        assert_eq!(sc.forged_events(), 0);
    }

    /// Ready carries no nonce: it is found with no level, and it is never counted as forged or stale.
    #[test]
    fn ready_is_untagged_and_not_counted() {
        let mut sc = Scanner::new(NONCE);
        let found = sc.feed(&osc("633;Ready;t=Ab12;sh=bash"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].event, Event::Ready { token: "Ab12".into(), sh: "bash".into() });
        assert_eq!(found[0].level, None);
        assert_eq!((sc.forged_events(), sc.stale_events()), (0, 0));
    }

    /// A Ready whose token or shell name is not [A-Za-z0-9], empty, missing, or over the cap is ignored. Not counted either.
    #[test]
    fn bad_ready_fields_are_ignored() {
        let long = "a".repeat(READY_FIELD_CAP + 1);
        let bodies = [
            "633;Ready;t=a%20b;sh=zsh".to_string(),
            "633;Ready;t=abc;sh=zsh-5".to_string(),
            "633;Ready;t=;sh=zsh".to_string(),
            "633;Ready;sh=zsh".to_string(),
            "633;Ready;t=abc".to_string(),
            format!("633;Ready;t={long};sh=zsh"),
            format!("633;Ready;t=abc;sh={long}"),
        ];
        for body in bodies {
            let mut sc = Scanner::new(NONCE);
            assert!(sc.feed(&osc(&body)).is_empty(), "{body}");
            assert_eq!((sc.forged_events(), sc.stale_events()), (0, 0), "{body}");
        }
        let at_cap = "b".repeat(READY_FIELD_CAP);
        assert_eq!(
            events(&osc(&format!("633;Ready;t={at_cap};sh=zsh"))),
            vec![Event::Ready { token: at_cap, sh: "zsh".into() }],
            "the cap itself is allowed"
        );
    }

    #[test]
    fn each_found_names_the_level_of_its_nonce() {
        let second = "11111111111111111111111111111111";
        let mut sc = Scanner::new(NONCE);
        sc.add_nonce(1, second);
        let mut s = Vec::new();
        s.extend(osc(&format!("133;A;n={NONCE}")));
        s.extend(osc(&format!("133;A;n={second}")));
        s.extend(osc("7;file://mac.local/tmp"));
        let found = sc.feed(&s);
        let levels: Vec<Option<u64>> = found.iter().map(|f| f.level).collect();
        assert_eq!(levels, vec![Some(0), Some(1), None], "the untagged OSC 7 has no level");
    }

    /// A retired nonce is stale, not forged. A nonce that was never accepted is forged. Both are ignored.
    #[test]
    fn retired_nonces_are_stale_and_unknown_ones_are_forged() {
        let child = "22222222222222222222222222222222";
        let mut sc = Scanner::new(NONCE);
        sc.add_nonce(1, child);
        assert_eq!(sc.feed(&osc(&format!("133;A;n={child}"))).len(), 1);
        sc.retire_nonce(1);
        assert!(sc.feed(&osc(&format!("133;A;n={child}"))).is_empty());
        assert_eq!((sc.stale_events(), sc.forged_events()), (1, 0));
        assert!(sc.feed(&osc("133;A;n=ffffffffffffffffffffffffffffffff")).is_empty());
        assert_eq!((sc.stale_events(), sc.forged_events()), (1, 1));
        assert_eq!(sc.accepted_nonces(), vec![NONCE.to_string()], "the retired nonce is no longer accepted");
    }

    #[test]
    fn retired_nonces_are_remembered_up_to_the_cap() {
        let mut sc = Scanner::new(NONCE);
        let nonces: Vec<String> = (1..=(RETIRED_CAP as u64 + 2)).map(|k| format!("{k:032x}")).collect();
        for (i, n) in nonces.iter().enumerate() {
            sc.add_nonce(i as u64 + 1, n);
            sc.retire_nonce(i as u64 + 1);
        }
        // The two oldest were forgotten, so they now count as forged. The newest is still stale.
        sc.feed(&osc(&format!("133;A;n={}", nonces[0])));
        sc.feed(&osc(&format!("133;A;n={}", nonces[RETIRED_CAP + 1])));
        assert_eq!(sc.forged_events(), 1);
        assert_eq!(sc.stale_events(), 1);
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
