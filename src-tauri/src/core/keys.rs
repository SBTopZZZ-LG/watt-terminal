//! The keys `send_keys` types into a command the agent started: the model's arguments, their bytes for the PTY, the text the
//! approval card shows, and a best-effort tracker of the line typed so far (the approval floor reads it at each Enter).
//!
//! Pure: no session, no shell, no I/O. The bytes are encoded under the observer's lock (see `Core::keys_target`), because
//! the arrow and Home/End codes depend on the application cursor mode of the screen at that moment.

use serde_json::Value;

/// The most text one `send_keys` call types, in bytes.
pub const MAX_TEXT_BYTES: usize = 4096;
/// The most keys one `send_keys` call sends.
pub const MAX_KEYS: usize = 64;

/// The named keys, as the model spells them (matched case-insensitively).
const NAMED: [(&str, Key); 14] = [
    ("Enter", Key::Enter),
    ("Tab", Key::Tab),
    ("Esc", Key::Esc),
    ("Backspace", Key::Backspace),
    ("Delete", Key::Delete),
    ("Space", Key::Space),
    ("Up", Key::Up),
    ("Down", Key::Down),
    ("Left", Key::Left),
    ("Right", Key::Right),
    ("Home", Key::Home),
    ("End", Key::End),
    ("PageUp", Key::PageUp),
    ("PageDown", Key::PageDown),
];

/// One key the model may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Tab,
    Esc,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Ctrl with an ASCII letter, as the uppercase letter (`Ctrl(b'C')` is Ctrl-C).
    Ctrl(u8),
}

impl Key {
    /// The key a name stands for, or None. Names are case-insensitive. `Ctrl-` takes one ASCII letter.
    pub fn parse(name: &str) -> Option<Key> {
        let lower = name.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("ctrl-") {
            let mut chars = rest.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Some(Key::Ctrl(c.to_ascii_uppercase() as u8)),
                _ => None,
            };
        }
        NAMED.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)).map(|(_, key)| *key)
    }

    /// The name the model and the card use: `Enter`, `Ctrl-C`.
    pub fn name(self) -> String {
        match self {
            Key::Ctrl(letter) => format!("Ctrl-{}", letter as char),
            other => NAMED.iter().find(|(_, key)| *key == other).map_or_else(String::new, |(name, _)| (*name).to_string()),
        }
    }

    /// The short form the approval card shows: `⏎` for Enter, `^C` for Ctrl-C, a space for Space, `<Name>` for the rest.
    fn display(self) -> String {
        match self {
            Key::Enter => "⏎".to_string(),
            Key::Ctrl(letter) => format!("^{}", letter as char),
            Key::Space => " ".to_string(),
            other => format!("<{}>", other.name()),
        }
    }

    /// The bytes a terminal sends for this key. Arrows, Home and End use the application form (`ESC O A`) while the
    /// application cursor mode is on, and the normal form (`ESC [ A`) otherwise.
    pub fn encode(self, app_cursor: bool) -> Vec<u8> {
        let cursor = |letter: char| if app_cursor { format!("\x1bO{letter}") } else { format!("\x1b[{letter}") };
        let text = match self {
            Key::Enter => "\r".to_string(),
            Key::Tab => "\t".to_string(),
            Key::Esc => "\x1b".to_string(),
            Key::Backspace => "\x7f".to_string(),
            Key::Delete => "\x1b[3~".to_string(),
            Key::Space => " ".to_string(),
            Key::Up => cursor('A'),
            Key::Down => cursor('B'),
            Key::Right => cursor('C'),
            Key::Left => cursor('D'),
            Key::Home => cursor('H'),
            Key::End => cursor('F'),
            Key::PageUp => "\x1b[5~".to_string(),
            Key::PageDown => "\x1b[6~".to_string(),
            Key::Ctrl(letter) => char::from(letter - b'A' + 1).to_string(),
        };
        text.into_bytes()
    }
}

/// What one `send_keys` call sends, in order: the literal text, then the keys. `record` is the command id the model named,
/// when it named one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPlan {
    pub text: String,
    pub keys: Vec<Key>,
    pub record: Option<u64>,
}

impl KeyPlan {
    /// Reads the arguments `text` (optional string), `keys` (optional array of names) and `id` (optional positive integer),
    /// and checks the caps. The error is the text the model reads.
    pub fn from_args(args: &Value) -> Result<KeyPlan, String> {
        let text = match args.get("text") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(_) => return Err("argument `text` must be a string".into()),
        };
        if text.len() > MAX_TEXT_BYTES {
            return Err(format!("text is {} bytes; the limit is {MAX_TEXT_BYTES} bytes per call", text.len()));
        }
        if text.chars().any(char::is_control) {
            return Err(
                "text contains a control character (a newline, tab or escape), which cannot be typed as text: use keys for \
                 Enter, Tab or Esc"
                    .into(),
            );
        }
        let names: Vec<String> = match args.get("keys") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_string).ok_or_else(|| "argument `keys` must be an array of key names".to_string()))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("argument `keys` must be an array of key names".into()),
        };
        if names.len() > MAX_KEYS {
            return Err(format!("keys has {} entries; the limit is {MAX_KEYS} per call", names.len()));
        }
        let keys = names
            .iter()
            .map(|name| {
                Key::parse(name).ok_or_else(|| {
                    format!(
                        "unknown key `{name}`: use Enter, Tab, Esc, Backspace, Delete, Space, Up, Down, Left, Right, Home, End, \
                         PageUp, PageDown, or Ctrl-A to Ctrl-Z"
                    )
                })
            })
            .collect::<Result<Vec<Key>, String>>()?;
        if text.is_empty() && keys.is_empty() {
            return Err("send_keys needs text, keys, or both".into());
        }
        let record = match args.get("id") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_u64() {
                Some(id) if id >= 1 => Some(id),
                _ => return Err("argument `id` must be a positive integer (the command id)".into()),
            },
        };
        Ok(KeyPlan { text, keys, record })
    }

    /// The bytes to send: the text, then each key, encoded for the application cursor mode.
    pub fn encode(&self, app_cursor: bool) -> Vec<u8> {
        let mut bytes = self.text.as_bytes().to_vec();
        for key in &self.keys {
            bytes.extend(key.encode(app_cursor));
        }
        bytes
    }

    /// The key names in send order (the text is not a name, so it is not listed).
    pub fn key_names(&self) -> Vec<String> {
        self.keys.iter().map(|key| key.name()).collect()
    }

    /// The decoded input for the approval card: the text, then the keys in short form (`echo hi⏎`, `:wq⏎`, `^C`).
    pub fn display(&self) -> String {
        let mut shown = self.text.clone();
        for key in &self.keys {
            shown.push_str(&key.display());
        }
        shown
    }

    /// True when the call sends only Ctrl-C, Ctrl-D or Esc, and no text. Such a call can only interrupt or leave the command
    /// the agent started, so the policy lets it through.
    pub fn is_interrupt_only(&self) -> bool {
        self.text.is_empty()
            && !self.keys.is_empty()
            && self.keys.iter().all(|key| matches!(key, Key::Ctrl(b'C') | Key::Ctrl(b'D') | Key::Esc))
    }

    /// True when the call presses Enter, which submits the line in a shell or a REPL.
    pub fn has_enter(&self) -> bool {
        self.keys.contains(&Key::Enter)
    }
}

/// The line typed at a command so far, as far as the app can follow it. Text adds to it, Backspace takes a character off,
/// and Ctrl-C and Ctrl-U clear it. Any key that moves the cursor, completes, or changes the line in a way this does not
/// follow (arrows, Tab, Home, Ctrl-A, history) makes the line unknown until the next Enter or Ctrl-C. The check is best
/// effort: it is not a boundary, because the program may edit the line in ways not seen here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypedLine {
    text: String,
    unknown: bool,
}

impl TypedLine {
    /// Marks the line unknown, for example after the user typed into it (their keys are not seen here).
    pub fn mark_unknown(&mut self) {
        self.unknown = true;
    }

    /// The lines `plan` submits, one for each Enter, in order (None where the line is not known), and the line left after
    /// the plan. Pure: `self` is not changed, so a refused call leaves the line as it was.
    pub fn after(&self, plan: &KeyPlan) -> (Vec<Option<String>>, TypedLine) {
        let mut line = self.clone();
        let mut submitted = Vec::new();
        line.text.push_str(&plan.text);
        for key in &plan.keys {
            match key {
                Key::Enter => {
                    submitted.push((!line.unknown).then(|| line.text.clone()));
                    line.text.clear();
                    line.unknown = false;
                }
                Key::Ctrl(b'C') => {
                    line.text.clear();
                    line.unknown = false;
                }
                Key::Ctrl(b'U') => line.text.clear(),
                Key::Ctrl(b'L') => {}
                Key::Backspace => {
                    line.text.pop();
                }
                Key::Space => line.text.push(' '),
                _ => line.unknown = true,
            }
        }
        (submitted, line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(text: &str, keys: &[&str]) -> KeyPlan {
        KeyPlan {
            text: text.to_string(),
            keys: keys.iter().map(|name| Key::parse(name).expect("a known key")).collect(),
            record: None,
        }
    }

    #[test]
    fn named_keys_parse_case_insensitively_and_ctrl_takes_one_letter() {
        assert_eq!(Key::parse("enter"), Some(Key::Enter));
        assert_eq!(Key::parse("PAGEUP"), Some(Key::PageUp));
        assert_eq!(Key::parse("Ctrl-c"), Some(Key::Ctrl(b'C')));
        assert_eq!(Key::parse("ctrl-Z"), Some(Key::Ctrl(b'Z')));
        assert_eq!(Key::parse("Ctrl-["), None, "only letters");
        assert_eq!(Key::parse("Ctrl-"), None);
        assert_eq!(Key::parse("Ctrl-AB"), None);
        assert_eq!(Key::parse("Return"), None, "not a name in the list");
        assert_eq!(Key::parse(" Enter"), None, "no trimming");
        for (name, key) in NAMED {
            assert_eq!(Key::parse(name), Some(key));
            assert_eq!(key.name(), name, "the canonical spelling round-trips");
        }
    }

    #[test]
    fn plain_keys_encode_to_their_terminal_bytes() {
        assert_eq!(Key::Enter.encode(false), b"\r");
        assert_eq!(Key::Tab.encode(false), b"\t");
        assert_eq!(Key::Esc.encode(false), b"\x1b");
        assert_eq!(Key::Backspace.encode(false), b"\x7f");
        assert_eq!(Key::Delete.encode(false), b"\x1b[3~");
        assert_eq!(Key::PageUp.encode(false), b"\x1b[5~");
        assert_eq!(Key::PageDown.encode(false), b"\x1b[6~");
        assert_eq!(Key::Ctrl(b'C').encode(false), b"\x03");
        assert_eq!(Key::Ctrl(b'D').encode(false), b"\x04");
        assert_eq!(Key::Ctrl(b'Z').encode(false), b"\x1a");
        assert_eq!(Key::Ctrl(b'L').encode(false), b"\x0c");
    }

    /// Arrows, Home and End follow the application cursor mode: `ESC [ A` normally, `ESC O A` in application mode.
    #[test]
    fn cursor_keys_follow_the_application_cursor_mode() {
        assert_eq!(Key::Up.encode(false), b"\x1b[A");
        assert_eq!(Key::Up.encode(true), b"\x1bOA");
        assert_eq!(Key::Down.encode(true), b"\x1bOB");
        assert_eq!(Key::Right.encode(true), b"\x1bOC");
        assert_eq!(Key::Left.encode(false), b"\x1b[D");
        assert_eq!(Key::Home.encode(false), b"\x1b[H");
        assert_eq!(Key::Home.encode(true), b"\x1bOH");
        assert_eq!(Key::End.encode(false), b"\x1b[F");
        assert_eq!(Key::End.encode(true), b"\x1bOF");
    }

    #[test]
    fn text_is_sent_before_the_keys_in_order() {
        let p = KeyPlan::from_args(&json!({"text": "echo hi", "keys": ["Enter", "Ctrl-D"]})).unwrap();
        assert_eq!(p.encode(false), b"echo hi\r\x04");
        assert_eq!(p.key_names(), ["Enter", "Ctrl-D"]);
    }

    #[test]
    fn text_must_not_hold_control_characters() {
        for bad in ["a\nb", "a\tb", "esc\x1b[A", "cr\r", "del\x7f"] {
            let err = KeyPlan::from_args(&json!({"text": bad})).expect_err("control character");
            assert!(err.contains("use keys for Enter, Tab or Esc"), "{err}");
        }
        assert!(KeyPlan::from_args(&json!({"text": "héllo wörld ✓"})).is_ok(), "printable non-ASCII is fine");
    }

    #[test]
    fn the_caps_are_checked_per_call() {
        let long = "a".repeat(MAX_TEXT_BYTES);
        assert!(KeyPlan::from_args(&json!({"text": long})).is_ok());
        let too_long = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(KeyPlan::from_args(&json!({"text": too_long})).unwrap_err().contains("4096 bytes"));
        let many: Vec<&str> = vec!["Space"; MAX_KEYS];
        assert!(KeyPlan::from_args(&json!({"keys": many})).is_ok());
        let too_many: Vec<&str> = vec!["Space"; MAX_KEYS + 1];
        assert!(KeyPlan::from_args(&json!({"keys": too_many})).unwrap_err().contains("64"));
    }

    #[test]
    fn a_call_needs_text_or_keys_and_valid_arguments() {
        assert!(KeyPlan::from_args(&json!({})).unwrap_err().contains("needs text, keys, or both"));
        assert!(KeyPlan::from_args(&json!({"text": ""})).unwrap_err().contains("needs text, keys, or both"));
        assert!(KeyPlan::from_args(&json!({"keys": ["Hyper"]})).unwrap_err().contains("unknown key `Hyper`"));
        assert!(KeyPlan::from_args(&json!({"keys": "Enter"})).unwrap_err().contains("array of key names"));
        assert!(KeyPlan::from_args(&json!({"keys": [5]})).unwrap_err().contains("array of key names"));
        assert!(KeyPlan::from_args(&json!({"text": 5, "keys": ["Enter"]})).unwrap_err().contains("must be a string"));
        assert!(KeyPlan::from_args(&json!({"text": "x", "id": 0})).unwrap_err().contains("positive integer"));
        assert!(KeyPlan::from_args(&json!({"text": "x", "id": "7"})).unwrap_err().contains("positive integer"));
        assert_eq!(KeyPlan::from_args(&json!({"text": "x", "id": 7})).unwrap().record, Some(7));
    }

    #[test]
    fn the_card_shows_the_decoded_input() {
        assert_eq!(plan("echo hi", &["Enter"]).display(), "echo hi⏎");
        assert_eq!(plan(":wq", &["Enter"]).display(), ":wq⏎");
        assert_eq!(plan("", &["Ctrl-C"]).display(), "^C");
        assert_eq!(plan("", &["Up", "Esc"]).display(), "<Up><Esc>");
    }

    #[test]
    fn interrupt_only_is_ctrl_c_ctrl_d_or_esc_with_no_text() {
        assert!(plan("", &["Ctrl-C"]).is_interrupt_only());
        assert!(plan("", &["Ctrl-D", "Esc", "Ctrl-C"]).is_interrupt_only());
        assert!(!plan("", &[]).is_interrupt_only(), "nothing to send is not an interrupt");
        assert!(!plan("q", &["Ctrl-C"]).is_interrupt_only(), "text is typed");
        assert!(!plan("", &["Ctrl-C", "Enter"]).is_interrupt_only());
        assert!(!plan("", &["Ctrl-L"]).is_interrupt_only());
    }

    #[test]
    fn the_line_is_followed_through_typing_backspace_and_clears() {
        let start = TypedLine::default();
        let (lines, after) = start.after(&plan("ec", &[]));
        assert!(lines.is_empty());
        // "ec" + "ho" = "echo", two Backspaces make "ec", then Space makes "ec ".
        let (lines, after) = after.after(&plan("ho", &["Backspace", "Backspace", "Space"]));
        assert!(lines.is_empty());
        let (lines, after) = after.after(&plan("", &["Enter"]));
        assert_eq!(lines, [Some("ec ".to_string())]);
        assert_eq!(after, TypedLine::default(), "a line starts again after Enter");
        let (lines, _) = TypedLine::default().after(&plan("rm -rf x", &["Ctrl-U", "Ctrl-C", "Enter"]));
        assert_eq!(lines, [Some(String::new())], "Ctrl-U and Ctrl-C clear the line");
    }

    /// A key the tracker does not follow makes the line unknown, so the Enter after it is checked as unknown.
    #[test]
    fn a_cursor_or_completion_key_makes_the_line_unknown_until_enter() {
        for key in ["Up", "Left", "Tab", "Home", "Delete", "Esc", "Ctrl-A", "Ctrl-R"] {
            let (lines, _) = TypedLine::default().after(&plan("ls", &[key, "Enter"]));
            assert_eq!(lines, [None], "{key} then Enter");
        }
        let (lines, _) = TypedLine::default().after(&plan("ls", &["Ctrl-L", "Enter"]));
        assert_eq!(lines, [Some("ls".to_string())], "Ctrl-L changes nothing on the line");
    }

    #[test]
    fn a_user_typed_line_is_unknown_and_the_tracker_never_changes_in_place() {
        let mut typed = TypedLine::default();
        typed.mark_unknown();
        let before = typed.clone();
        let (lines, _) = typed.after(&plan("echo", &["Enter"]));
        assert_eq!(lines, [None]);
        assert_eq!(typed, before, "after() does not change the line it is asked about");
    }

    #[test]
    fn every_enter_in_a_call_is_a_line() {
        // The text goes in before the keys: "a", Enter, Space, Enter.
        let (lines, _) = TypedLine::default().after(&plan("a", &["Enter", "Space", "Enter"]));
        assert_eq!(lines, [Some("a".to_string()), Some(" ".to_string())]);
        let (lines, _) = TypedLine::default().after(&plan("", &["Enter", "Enter"]));
        assert_eq!(lines, [Some(String::new()), Some(String::new())]);
    }
}
