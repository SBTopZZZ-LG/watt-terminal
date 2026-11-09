//! Server-sent events framing for chat-completion streams.
//!
//! Bytes arrive in network chunks that can split a line anywhere, so the parser buffers until it sees a
//! newline. It skips comment lines (leading ':', e.g. OpenRouter's ": OPENROUTER PROCESSING"), blank
//! keepalives and non-data fields (`event:`, `id:`, `retry:`). Lines may end in LF or CRLF. Consecutive
//! `data:` lines form one event, joined with '\n' as the SSE spec says. Each event is returned as its
//! payload string; the caller decides what `[DONE]` means.

#[derive(Default)]
pub struct SseParser {
    pending: Vec<u8>,
    data: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one network chunk and returns the data payloads of every event it completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw[..raw.len() - 1]).into_owned();
            self.line(line.strip_suffix('\r').unwrap_or(&line), &mut out);
        }
        out
    }

    /// Called at end of stream. Processes a last line without a newline and dispatches a pending event.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let raw = std::mem::take(&mut self.pending);
            let line = String::from_utf8_lossy(&raw).into_owned();
            self.line(line.strip_suffix('\r').unwrap_or(&line), &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<String>) {
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        if line.starts_with(':') {
            return;
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
        }
    }

    fn dispatch(&mut self, out: &mut Vec<String>) {
        if let Some(data) = self.data.take().filter(|d| !d.is_empty()) {
            out.push(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SseParser;

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
        let mut p = SseParser::new();
        let mut out = p.push(input);
        out.extend(p.finish());
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
            let mut p = SseParser::new();
            let mut got = p.push(&bytes[..cut]);
            got.extend(p.push(&bytes[cut..]));
            got.extend(p.finish());
            assert_eq!(got, expected, "split at byte {cut}");
        }
    }

    #[test]
    fn crlf_split_between_cr_and_lf_is_handled() {
        let mut p = SseParser::new();
        let mut got = p.push(b"data: {\"x\":1}\r");
        assert!(got.is_empty());
        got.extend(p.push(b"\n\r\n"));
        assert_eq!(got, vec!["{\"x\":1}"]);
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let mut p = SseParser::new();
        let got = p.push(b"data: one\ndata: two\n\n");
        assert_eq!(got, vec!["one\ntwo"]);
    }

    #[test]
    fn pending_event_is_flushed_at_end_of_stream() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: {\"last\":true}\n").is_empty());
        assert_eq!(p.finish(), vec!["{\"last\":true}"]);
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
