//! Server-sent events framing for chat-completion streams.
//!
//! Bytes arrive in network chunks that can split a line anywhere, so the parser buffers until it sees a
//! newline. It skips comment lines (leading ':', e.g. OpenRouter's ": OPENROUTER PROCESSING"), blank
//! keepalives and non-data fields (`event:`, `id:`, `retry:`). Lines may end in LF or CRLF. Consecutive
//! `data:` lines form one event, joined with '\n' as the SSE spec says. Each event is returned as its
//! payload string; the caller decides what `[DONE]` means.
//!
//! A line, and the data of one event, may not grow past `max` bytes. A longer one is an error, so a
//! provider that never ends a line cannot make the buffer grow without limit.

use super::LlmError;

pub struct SseParser {
    max: usize,
    pending: Vec<u8>,
    data: Option<String>,
}

impl SseParser {
    /// `max` is the longest line, and the longest event data, in bytes.
    pub fn new(max: usize) -> Self {
        Self { max, pending: Vec::new(), data: None }
    }

    /// Feeds one network chunk and returns the data payloads of every event it completed.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, LlmError> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            if pos > self.max {
                return Err(too_large(self.max));
            }
            let raw: Vec<u8> = self.pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw[..raw.len() - 1]).into_owned();
            self.line(line.strip_suffix('\r').unwrap_or(&line), &mut out)?;
        }
        // A line still without its newline is checked now, so it cannot grow past the limit.
        if self.pending.len() > self.max {
            return Err(too_large(self.max));
        }
        Ok(out)
    }

    /// Called at end of stream. Processes a last line without a newline and dispatches a pending event.
    pub fn finish(&mut self) -> Result<Vec<String>, LlmError> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            if self.pending.len() > self.max {
                return Err(too_large(self.max));
            }
            let raw = std::mem::take(&mut self.pending);
            let line = String::from_utf8_lossy(&raw).into_owned();
            self.line(line.strip_suffix('\r').unwrap_or(&line), &mut out)?;
        }
        self.dispatch(&mut out);
        Ok(out)
    }

    fn line(&mut self, line: &str, out: &mut Vec<String>) -> Result<(), LlmError> {
        if line.is_empty() {
            self.dispatch(out);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            match &mut self.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(rest);
                }
                None => self.data = Some(rest.to_string()),
            }
            if self.data.as_ref().is_some_and(|data| data.len() > self.max) {
                return Err(too_large(self.max));
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, out: &mut Vec<String>) {
        if let Some(data) = self.data.take().filter(|d| !d.is_empty()) {
            out.push(data);
        }
    }
}

fn too_large(max: usize) -> LlmError {
    LlmError::TooLarge(format!("a single line or event is more than {}", super::size_text(max)))
}

#[cfg(test)]
mod tests {
    use super::SseParser;
    use crate::core::llm::MAX_SSE_LINE_BYTES;

    const SAMPLE: &str = concat!(
        ": OPENROUTER PROCESSING\n",
        "\n",
        "event: message\n",
        "data: {\"a\":1}\n",
        "\n",
        ": keepalive comment\n",
        "data: {\"b\":\"two\"}\r\n",
        "\r\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    fn all_at_once(input: &[u8]) -> Vec<String> {
        let mut p = SseParser::new(MAX_SSE_LINE_BYTES);
        let mut out = p.push(input).unwrap();
        out.extend(p.finish().unwrap());
        out
    }

    #[test]
    fn skips_comments_blank_keepalives_and_event_lines() {
        assert_eq!(all_at_once(SAMPLE.as_bytes()), vec!["{\"a\":1}", "{\"b\":\"two\"}", "[DONE]"]);
    }

    #[test]
    fn splitting_at_every_byte_offset_gives_the_same_events() {
        let bytes = SAMPLE.as_bytes();
        let expected = all_at_once(bytes);
        for cut in 0..=bytes.len() {
            let mut p = SseParser::new(MAX_SSE_LINE_BYTES);
            let mut got = p.push(&bytes[..cut]).unwrap();
            got.extend(p.push(&bytes[cut..]).unwrap());
            got.extend(p.finish().unwrap());
            assert_eq!(got, expected, "split at byte {cut}");
        }
    }

    #[test]
    fn crlf_split_between_cr_and_lf_is_handled() {
        let mut p = SseParser::new(MAX_SSE_LINE_BYTES);
        let mut got = p.push(b"data: {\"x\":1}\r").unwrap();
        assert!(got.is_empty());
        got.extend(p.push(b"\n\r\n").unwrap());
        assert_eq!(got, vec!["{\"x\":1}"]);
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let mut p = SseParser::new(MAX_SSE_LINE_BYTES);
        let got = p.push(b"data: one\ndata: two\n\n").unwrap();
        assert_eq!(got, vec!["one\ntwo"]);
    }

    #[test]
    fn pending_event_is_flushed_at_end_of_stream() {
        let mut p = SseParser::new(MAX_SSE_LINE_BYTES);
        assert!(p.push(b"data: {\"last\":true}\n").unwrap().is_empty());
        assert_eq!(p.finish().unwrap(), vec!["{\"last\":true}"]);
    }

    #[test]
    fn a_line_over_the_limit_is_an_error_even_without_a_newline_yet() {
        let mut p = SseParser::new(16);
        let err = p.push(b"data: 0123456789abcdefghij").unwrap_err();
        assert!(matches!(err, crate::core::llm::LlmError::TooLarge(_)), "{err:?}");
    }

    #[test]
    fn a_complete_line_over_the_limit_is_an_error() {
        let mut p = SseParser::new(16);
        assert!(p.push(b"data: 0123456789abcdefghij\n\n").is_err());
    }

    #[test]
    fn a_line_within_the_limit_is_fine() {
        let mut p = SseParser::new(16);
        assert_eq!(p.push(b"data: short\n\n").unwrap(), vec!["short".to_string()]);
    }

    #[test]
    fn event_data_joined_from_lines_over_the_limit_is_an_error() {
        // Each line is 10 bytes, within the limit of 20, but the joined data is 21.
        let mut p = SseParser::new(20);
        assert!(p.push(b"data: 0123456789\ndata: 0123456789\n\n").is_err());
        let mut p = SseParser::new(21);
        assert_eq!(p.push(b"data: 0123456789\ndata: 0123456789\n\n").unwrap(), vec!["0123456789\n0123456789"]);
    }

    #[test]
    fn recorded_openrouter_stream_yields_one_payload_per_chunk_and_done() {
        let raw = include_str!("../../../tests/fixtures/llm/openrouter_stream.sse");
        let payloads = all_at_once(raw.as_bytes());
        let data_lines = raw.lines().filter(|l| l.starts_with("data: ")).count();
        assert_eq!(payloads.len(), data_lines);
        assert_eq!(payloads.last().map(String::as_str), Some("[DONE]"));
    }
}
