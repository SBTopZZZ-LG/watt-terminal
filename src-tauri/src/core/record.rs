//! Command records: what ran, where, how it ended, and what it printed.

use std::collections::VecDeque;

use serde::Serialize;

/// Rendered output is capped. Longer output is cut and flagged with `output_truncated`.
pub const OUTPUT_CAP: usize = 1 << 20;
/// Raw bytes kept per record: the first RAW_HEAD bytes, then the last RAW_TAIL bytes.
pub const RAW_HEAD: usize = 64 << 10;
pub const RAW_TAIL: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    User,
    Agent { call_id: String },
    TermReply,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cwd {
    pub host: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    /// Saw C and D, and no screen-region scrolling or resize during the command.
    Complete,
    /// Ended without D (a new command or prompt arrived first, the observer lost data, or the shell exited).
    Incomplete,
    /// Finished, but the rendered output may be missing rows (scroll region used, or the terminal was resized).
    Approximate,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: u64,
    pub command: String,
    pub source: Source,
    pub shell_pid: Option<u32>,
    pub cwd: Option<Cwd>,
    /// The key of the shell level the command ran in (0 is the local shell).
    pub level: u64,
    /// The label of the nested level the command ran in (for example `ssh ubuntu@lidia`). None on the local shell.
    pub location: Option<String>,
    pub exit_code: Option<i32>,
    pub pipestatus: Vec<i32>,
    pub status: RecordStatus,
    pub used_alt_screen: bool,
    /// Rendered output: rows joined at soft wraps, trailing blank rows removed.
    pub output: String,
    pub output_truncated: bool,
    /// Raw PTY bytes between C and D (capped). Not serialized to the frontend in slice 1.
    #[serde(skip)]
    pub raw_head: Vec<u8>,
    #[serde(skip)]
    pub raw_tail: Vec<u8>,
    pub raw_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordSummary {
    pub id: u64,
    pub command: String,
    pub source: Source,
    pub level: u64,
    pub location: Option<String>,
    pub exit_code: Option<i32>,
    pub status: RecordStatus,
    pub used_alt_screen: bool,
    pub output_bytes: usize,
}

impl From<&Record> for RecordSummary {
    fn from(r: &Record) -> Self {
        RecordSummary {
            id: r.id,
            command: r.command.clone(),
            source: r.source.clone(),
            level: r.level,
            location: r.location.clone(),
            exit_code: r.exit_code,
            status: r.status,
            used_alt_screen: r.used_alt_screen,
            output_bytes: r.output.len(),
        }
    }
}

/// Joins rendered rows into text. A row that soft-wrapped continues the previous line.
#[derive(Debug, Default)]
pub struct OutputBuilder {
    text: String,
    prev_wrapped: bool,
    has_row: bool,
    truncated: bool,
}

impl OutputBuilder {
    /// `row` is the row's plain text; `wrapped` is true when the row continues on the next row.
    pub fn push_row(&mut self, row: &str, wrapped: bool) {
        if self.truncated {
            return;
        }
        let sep = usize::from(self.has_row && !self.prev_wrapped);
        // Trailing spaces only matter inside a wrapped line; a line that ends here is trimmed.
        let piece = if wrapped { row } else { row.trim_end() };
        if self.text.len() + sep + piece.len() > OUTPUT_CAP {
            self.truncated = true;
            return;
        }
        if sep == 1 {
            self.text.push('\n');
        }
        self.text.push_str(piece);
        self.has_row = true;
        self.prev_wrapped = wrapped;
    }

    /// True once the cap was hit. Callers can skip rendering further rows.
    pub fn is_full(&self) -> bool {
        self.truncated
    }

    /// Returns the text (without trailing newlines) and whether it was cut at the cap.
    pub fn finish(mut self) -> (String, bool) {
        while self.text.ends_with('\n') {
            self.text.pop();
        }
        (self.text, self.truncated)
    }
}

/// Keeps the first RAW_HEAD bytes and the most recent RAW_TAIL bytes of a record's raw stream.
#[derive(Debug, Default)]
pub struct RawCapture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: u64,
}

impl RawCapture {
    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let room = RAW_HEAD - self.head.len();
        let (into_head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(into_head);
        self.tail.extend(rest);
        if self.tail.len() > RAW_TAIL {
            let excess = self.tail.len() - RAW_TAIL;
            self.tail.drain(..excess);
        }
    }

    /// Returns (head, tail, truncated).
    pub fn into_parts(self) -> (Vec<u8>, Vec<u8>, bool) {
        let kept = self.head.len() as u64 + self.tail.len() as u64;
        let truncated = self.total > kept;
        (self.head, Vec::from(self.tail), truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_wrapped_rows_are_joined_and_hard_rows_are_not() {
        let mut b = OutputBuilder::default();
        b.push_row("aaaa", true);
        b.push_row("bb   ", false);
        b.push_row("c", false);
        b.push_row("", false);
        let (text, truncated) = b.finish();
        assert_eq!(text, "aaaabb\nc");
        assert!(!truncated);
    }

    #[test]
    fn output_cap_truncates() {
        let mut b = OutputBuilder::default();
        let row = "x".repeat(OUTPUT_CAP / 2);
        b.push_row(&row, false);
        b.push_row(&row, false);
        b.push_row("late", false);
        let (text, truncated) = b.finish();
        assert!(truncated);
        assert!(text.len() <= OUTPUT_CAP);
        assert!(!text.contains("late"));
    }

    #[test]
    fn raw_capture_keeps_head_and_tail() {
        let mut r = RawCapture::default();
        let data: Vec<u8> = (0..(RAW_HEAD + RAW_TAIL + 500)).map(|i| (i % 251) as u8).collect();
        for chunk in data.chunks(7000) {
            r.push(chunk);
        }
        let (head, tail, truncated) = r.into_parts();
        assert!(truncated);
        assert_eq!(head, &data[..RAW_HEAD]);
        assert_eq!(tail, &data[data.len() - RAW_TAIL..]);
    }
}
