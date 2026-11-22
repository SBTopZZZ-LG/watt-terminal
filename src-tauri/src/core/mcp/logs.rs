//! The stderr log of one MCP server: the last lines, redacted and capped. Pure logic.

use std::collections::VecDeque;
use std::sync::LazyLock;

use regex::Regex;

use crate::core::chat::redact::Redactor;

/// Lines kept per server. The oldest go first.
pub const LOG_LINES: usize = 200;
/// Longest kept line, in characters.
pub const LOG_LINE_CHARS: usize = 500;

/// An http(s) URL followed by a query string. The query often carries a key, and reqwest puts the URL in its errors.
static URL_QUERY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(https?://[^\s?#'"<>)]*)\?[^\s'"<>)]*"#).expect("constant pattern compiles"));

/// Text that is shown or stored from a server: known secret values and credential-shaped text removed (`Redactor`), and
/// URL query strings replaced by a marker. Applied to server log lines and to every error text that leaves the runtime.
pub fn scrub(text: &str, redactor: &Redactor) -> String {
    let redacted = redactor.redact(text);
    URL_QUERY.replace_all(&redacted, "$1?[query hidden]").into_owned()
}

/// A bounded ring of server log lines.
#[derive(Debug, Default)]
pub struct LogRing {
    lines: VecDeque<String>,
}

impl LogRing {
    /// Keeps `line` after `scrub`, cut to `LOG_LINE_CHARS`. Scrubbing comes first, so a cut cannot leave half a secret.
    pub fn push(&mut self, line: &str, redactor: &Redactor) {
        if self.lines.len() == LOG_LINES {
            self.lines.pop_front();
        }
        let scrubbed = scrub(line, redactor);
        self.lines.push_back(scrubbed.chars().take(LOG_LINE_CHARS).collect());
    }

    /// The kept lines, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_last_lines() {
        let redactor = Redactor::new(Vec::new());
        let mut ring = LogRing::default();
        for n in 0..(LOG_LINES + 5) {
            ring.push(&format!("line {n}"), &redactor);
        }
        let lines = ring.lines();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("line 5"));
        assert_eq!(lines.last().map(String::as_str), Some(format!("line {}", LOG_LINES + 4).as_str()));
    }

    #[test]
    fn a_secret_value_is_redacted_before_it_is_kept() {
        let redactor = Redactor::new(vec!["hunter2-secret-value".to_string()]);
        let mut ring = LogRing::default();
        ring.push("token=hunter2-secret-value now", &redactor);
        let lines = ring.lines();
        assert_eq!(lines, vec!["token=[REDACTED] now".to_string()]);
    }

    #[test]
    fn a_url_query_is_hidden_but_the_url_stays() {
        let redactor = Redactor::new(Vec::new());
        let text = "error sending request for url (https://mcp.example.test/mcp?key=abc123xyz)";
        assert_eq!(scrub(text, &redactor), "error sending request for url (https://mcp.example.test/mcp?[query hidden])");
    }

    #[test]
    fn a_long_line_is_cut_to_the_limit_in_characters() {
        let redactor = Redactor::new(Vec::new());
        let mut ring = LogRing::default();
        ring.push(&"ü".repeat(LOG_LINE_CHARS + 50), &redactor);
        assert_eq!(ring.lines()[0].chars().count(), LOG_LINE_CHARS);
    }
}
