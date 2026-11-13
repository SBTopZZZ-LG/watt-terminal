//! The model-facing view of tool results.
//!
//! The UI keeps the full result. This module shapes what goes into the model's history: compact text, long output
//! cut to a head and a tail with a paging pointer, repeated lines collapsed, and everything redacted. Field names
//! follow the serialized forms in `tools.rs` (`RunOutcome`, `ScreenView`, `CommandView`, `RecordSummary`) and
//! `record.rs`. Tools return different shapes and errors arrive as plain text, so every access is a checked lookup:
//! a missing or oddly typed field becomes a placeholder, never a panic.

use serde_json::{Value, json};

use crate::core::chat::redact::Redactor;
use crate::core::chat::{questions, todo};
use crate::core::foreground::{Foreground, ForegroundKind};
use crate::core::observer::SCREEN_HISTORY_CAP;
use crate::core::tools::foreground_next_step;

/// `run_command` output longer than this is cut to a head and tail. Output at or under it is sent whole.
const RUN_OUTPUT_LIMIT: usize = 4500;
/// `get_command` output longer than this is cut to a head and tail.
const GET_OUTPUT_LIMIT: usize = 6000;
const HEAD_BYTES: usize = 1500;
const TAIL_BYTES: usize = 3000;
/// Screen and command-list text keep their most recent bytes up to these limits. `get_screen` with `lines` keeps up to
/// HISTORY_LIMIT: it is the answer the agent asked for, and the observer caps it at the same size.
const SCREEN_LIMIT: usize = 6000;
const HISTORY_LIMIT: usize = SCREEN_HISTORY_CAP;
const LIST_LIMIT: usize = 6000;
/// The output tail of a command that a `wait` finished. The screen gets the rest of `SCREEN_LIMIT`.
const WAIT_OUTPUT_LIMIT: usize = SCREEN_LIMIT / 2;
/// Unknown tools' JSON, error messages and command lines in the list are capped at these many bytes.
const SMALL_LIMIT: usize = 2000;
const COMMAND_TEXT_LIMIT: usize = 200;
/// A run of this many identical lines or more is collapsed.
const REPEAT_MIN: usize = 3;
const TRUNCATED_MARK: &str = "...[truncated]";

/// Shapes one tool result for the model. `ok` is whether the tool call succeeded; on failure `result` carries the
/// error text (a string, or an object with `message` or `error`). Everything passes through `redactor`.
pub fn project(name: &str, ok: bool, result: &Value, redactor: &Redactor) -> String {
    let text = if !ok {
        format!("error: {}", error_message(result, redactor))
    } else {
        match name {
            "run_command" => run_command_text(result, redactor),
            "get_command" => get_command_text(result, redactor),
            "get_screen" => get_screen_text(result, redactor),
            "wait" => wait_text(result, redactor),
            "send_keys" => send_keys_text(result, redactor),
            todo::TOOL_NAME => todo_write_text(result),
            // The question result is compact JSON the history keeps whole: its own cap, not the small one, so even the
            // largest valid answers survive. The validation bounds keep it under the cap, so it is never cut mid-JSON.
            questions::TOOL_NAME => cap(&result.to_string(), questions::ANSWER_LIMIT),
            "request_integration" => result.as_str().unwrap_or_default().to_string(),
            "list_commands" => list_commands_text(result, redactor),
            "get_cwd" => cwd_text(Some(result)),
            _ => compact_json(result),
        }
    };
    redactor.redact(&text)
}

fn run_command_text(v: &Value, r: &Redactor) -> String {
    let text = match v.get("outcome").and_then(Value::as_str) {
        Some("finished") => finished_text(v.get("record"), r),
        Some("running") => running_text(v),
        Some("not_run") => not_run_text(v),
        _ => compact_json(v),
    };
    // The note (see core/tools `INPUT_WAIT_NOTE`) follows the outcome, on its own line.
    match v.get("note").and_then(Value::as_str) {
        Some(note) => format!("{text}\n{note}"),
        None => text,
    }
}

fn finished_text(record: Option<&Value>, r: &Redactor) -> String {
    let Some(record) = record else {
        return "error: the command finished, but its record is missing".to_string();
    };
    let id = scalar(record.get("id"));
    let output = record.get("output").and_then(Value::as_str).unwrap_or("");
    let mut fields = serde_json::Map::new();
    fields.insert("id".into(), record.get("id").cloned().unwrap_or(Value::Null));
    fields.insert("exit_code".into(), record.get("exit_code").cloned().unwrap_or(Value::Null));
    fields.insert("status".into(), record.get("status").cloned().unwrap_or(Value::Null));
    fields.insert("cwd".into(), json!(cwd_text(record.get("cwd"))));
    if let Some(codes) = record.get("pipestatus").and_then(Value::as_array) {
        // Shown only when a stage failed, so a plain command stays short.
        if codes.iter().any(|c| c.as_i64() != Some(0)) {
            fields.insert("pipestatus".into(), Value::Array(codes.clone()));
        }
    }
    if record.get("output_truncated").and_then(Value::as_bool) == Some(true) {
        fields.insert("output_truncated".into(), json!(true));
    }
    fields.insert("output".into(), json!(window_output(output, RUN_OUTPUT_LIMIT, &id, r)));
    Value::Object(fields).to_string()
}

/// A run whose wait ended with the command still going. It is JSON, like a finished command, so the record id stays
/// readable in the history (the agent loop reads it for the finished note). The screen tail is redacted with the rest.
fn running_text(v: &Value) -> String {
    const FIELDS: [&str; 9] =
        ["outcome", "reason", "record_id", "call_id", "state", "foreground", "waited_ms", "screen_tail", "hint"];
    let mut fields = serde_json::Map::new();
    for key in FIELDS {
        if let Some(value) = v.get(key) {
            fields.insert(key.into(), value.clone());
        }
    }
    Value::Object(fields).to_string()
}

fn not_run_text(v: &Value) -> String {
    match v.get("reason").and_then(Value::as_str).unwrap_or("") {
        "abandoned" => "not run: the line was abandoned before it ran (cancelled, or finished by the user)".to_string(),
        "waiting_for_input" => {
            "not run: the shell is waiting for the rest of the line, so the command is incomplete. Nothing was sent to cancel it."
                .to_string()
        }
        "cancelled" => "cancelled while waiting; the command may still be running".to_string(),
        "cancelled_before_send" => "not run: the turn was cancelled before the command was sent. Nothing ran.".to_string(),
        _ => compact_json(v),
    }
}

fn get_command_text(v: &Value, r: &Redactor) -> String {
    let id = scalar(v.get("id"));
    let header = format!(
        "id={id} exit={} status={} cwd={} offset={} total_bytes={} truncated={}",
        scalar(v.get("exit_code")),
        scalar(v.get("status")),
        cwd_text(v.get("cwd")),
        scalar(v.get("offset")),
        scalar(v.get("total_bytes")),
        scalar(v.get("truncated")),
    );
    let output = v.get("output").and_then(Value::as_str).unwrap_or("");
    format!("{header}\n{}", window_output(output, GET_OUTPUT_LIMIT, &id, r))
}

fn get_screen_text(v: &Value, r: &Redactor) -> String {
    // With `lines` the rows are the history the agent asked for, so they keep the larger limit.
    let limit = if v.get("history").is_some() { HISTORY_LIMIT } else { SCREEN_LIMIT };
    let mut lines = vec![screen_header(v, r)];
    if let Some(note) = v.get("note").and_then(Value::as_str) {
        lines.push(note.to_string());
    }
    lines.push(screen_body(v, r, limit));
    lines.join("\n")
}

/// The rows of a screen, trailing blank rows dropped, redacted, keeping the most recent `limit` bytes.
fn screen_body(v: &Value, r: &Redactor, limit: usize) -> String {
    let rows: Vec<&str> = v
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().map(|row| row.as_str().unwrap_or("")).collect())
        .unwrap_or_default();
    let mut body: Vec<&str> = rows.iter().map(|row| row.trim_end()).collect();
    while body.last().is_some_and(|row| row.is_empty()) {
        body.pop();
    }
    keep_tail(&r.redact(&body.join("\n")), limit)
}

/// A `wait` result: a first line with the reason and the time waited, the note if the seconds were clamped, the screen
/// as get_screen shows it, and for a finished command its record line and output tail. The output is not repeated
/// when every one of its lines is already a row on the screen. Together the screen and the output stay near SCREEN_LIMIT.
fn wait_text(v: &Value, r: &Redactor) -> String {
    let mut lines = vec![format!("waited {}: {}", waited_text(v), wait_reason_text(v))];
    if let Some(note) = v.get("note").and_then(Value::as_str) {
        lines.push(note.to_string());
    }
    let screen = v.get("screen").unwrap_or(&Value::Null);
    let record = v.get("record").filter(|rec| !rec.is_null());
    let output = record.map(|rec| {
        let text = rec.get("output").and_then(Value::as_str).unwrap_or("");
        keep_tail(&r.redact(text.trim_end()), WAIT_OUTPUT_LIMIT)
    });
    let output_len = output.as_ref().map_or(0, String::len);
    let body = screen_body(screen, r, SCREEN_LIMIT.saturating_sub(output_len));
    lines.push(format!("{}\n{body}", screen_header(screen, r)));
    if let (Some(rec), Some(output)) = (record, output) {
        lines.push(command_header(rec, r));
        let screen_rows: Vec<&str> = body.lines().map(str::trim_end).collect();
        let shown = output.lines().all(|line| screen_rows.contains(&line.trim_end()));
        lines.push(if output.trim().is_empty() {
            "(no output)".to_string()
        } else if shown {
            "(the output is on the screen above)".to_string()
        } else {
            output
        });
    }
    lines.join("\n")
}

fn waited_text(v: &Value) -> String {
    v.get("waited_ms").and_then(Value::as_u64).map_or_else(|| "?".to_string(), |ms| format!("{:.1}s", ms as f64 / 1000.0))
}

fn wait_reason_text(v: &Value) -> String {
    match v.get("reason").and_then(Value::as_str).unwrap_or("") {
        "idle" => "the shell is idle at the prompt, so there was nothing to wait for".to_string(),
        "session_exited" => "the shell session has exited".to_string(),
        "command_finished" => match v.get("record").and_then(|rec| rec.get("exit_code")).and_then(Value::as_i64) {
            Some(code) => format!("the command finished with exit status {code}"),
            None => "the command finished, exit status unknown".to_string(),
        },
        "screen_changed" => "the screen changed".to_string(),
        "timeout" => "the wait time ran out and nothing finished".to_string(),
        "cancelled" => "the turn was cancelled".to_string(),
        _ => "the wait ended".to_string(),
    }
}

fn command_header(rec: &Value, r: &Redactor) -> String {
    let command = escape_controls(&r.redact(rec.get("command").and_then(Value::as_str).unwrap_or("")));
    format!(
        "command id={} exit={} status={} cwd={} command={}",
        scalar(rec.get("id")),
        scalar(rec.get("exit_code")),
        scalar(rec.get("status")),
        cwd_text(rec.get("cwd")),
        cap(&command, COMMAND_TEXT_LIMIT),
    )
}

fn screen_header(v: &Value, r: &Redactor) -> String {
    let mut header = format!(
        "screen size={} cursor={} alternate_screen={} state={}",
        pair(v.get("size"), 'x'),
        pair(v.get("cursor"), ','),
        scalar(v.get("alternate_screen")),
        scalar(v.get("state")),
    );
    // The observer and integration are reported only when they are not in their normal state.
    let observer = scalar(v.get("observer_status"));
    if observer != "ok" && observer != "?" {
        header.push_str(&format!(" observer={observer}"));
    }
    let integration = scalar(v.get("integration_status"));
    if integration != "live" && integration != "?" {
        header.push_str(&format!(" integration={integration}"));
    }
    // Set only by `get_screen` with `lines`: how many logical lines the rows are, and what was left out.
    if let Some(history) = v.get("history").filter(|h| h.is_object()) {
        header.push_str(&format!(" lines={}", scalar(history.get("lines"))));
        if history.get("truncated") == Some(&Value::Bool(true)) {
            header.push_str(" truncated=true");
        }
        if history.get("no_history") == Some(&Value::Bool(true)) {
            header.push_str(" no_history=true (the alternate screen has no history)");
        }
    }
    if let Some(location) = v.get("location").and_then(Value::as_str) {
        header.push_str(&format!("\nlocation: {} (a nested shell, not the local machine)", escape_controls(&r.redact(location))));
    }
    if let Some(line) = foreground_line(v, r) {
        header.push('\n');
        header.push_str(&line);
    }
    header
}

/// The one line that names the foreground program, when the screen has a hint. Absent otherwise.
fn foreground_line(v: &Value, r: &Redactor) -> Option<String> {
    let fg: Foreground = serde_json::from_value(v.get("foreground")?.clone()).ok()?;
    let command = escape_controls(&r.redact(&fg.command));
    Some(if fg.kind == ForegroundKind::Other {
        format!("foreground: {command} (a command running in this shell; wait for it to finish)")
    } else {
        format!(
            "foreground: {command} ({}, without shell integration; you can read the screen but cannot run commands here). {}",
            fg.kind.describe(),
            foreground_next_step(&fg)
        )
    })
}

/// A `send_keys` result. `sent` names the text and keys that went in, the state, and the screen after them. `finished` is the
/// command's record, as run_command gives it. The screen is redacted like the rest.
fn send_keys_text(v: &Value, r: &Redactor) -> String {
    match v.get("outcome").and_then(Value::as_str) {
        Some("finished") => finished_text(v.get("record"), r),
        Some("sent") => {
            let mut head = format!("sent {} bytes", scalar(v.get("bytes")));
            if let Some(text) = v.get("text").and_then(Value::as_str) {
                head.push_str(&format!(" of text `{}`", r.redact(text)));
            }
            let names: Vec<&str> = v.get("keys").and_then(Value::as_array).map(|k| k.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            if !names.is_empty() {
                head.push_str(&format!(" and keys {}", names.join(" ")));
            }
            head.push_str(&format!(". state={}", scalar(v.get("state"))));
            let rows: Vec<&str> =
                v.get("screen_tail").and_then(Value::as_array).map(|rows| rows.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            format!(
                "{head}\nscreen:\n{}\nThe command is still running: wait, get_screen, or send_keys to answer it.",
                keep_tail(&r.redact(&rows.join("\n")), SCREEN_LIMIT)
            )
        }
        _ => compact_json(v),
    }
}

fn list_commands_text(v: &Value, r: &Redactor) -> String {
    let Some(list) = v.as_array() else {
        return compact_json(v);
    };
    if list.is_empty() {
        return "no finished commands".to_string();
    }
    let lines: Vec<String> = list.iter().map(|c| command_line(c, r)).collect();
    keep_tail(&lines.join("\n"), LIST_LIMIT)
}

fn command_line(c: &Value, r: &Redactor) -> String {
    let source = c.get("source").and_then(|s| s.get("kind")).and_then(Value::as_str).unwrap_or("?");
    let command = escape_controls(&r.redact(c.get("command").and_then(Value::as_str).unwrap_or("")));
    format!(
        "{} exit={} bytes={} source={} {}",
        scalar(c.get("id")),
        scalar(c.get("exit_code")),
        scalar(c.get("output_bytes")),
        source,
        cap(&command, COMMAND_TEXT_LIMIT),
    )
}

/// The result of a `todo_write` call: the counts of the list it now holds, or "cleared" for an empty list.
fn todo_write_text(v: &Value) -> String {
    let items = v.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    if items.is_empty() {
        return "todo list cleared".to_string();
    }
    let count = |status: &str| items.iter().filter(|item| item["status"] == status).count();
    format!(
        "todo list updated: {} done, {} in progress, {} pending",
        count("completed"),
        count("in_progress"),
        count("pending")
    )
}

fn cwd_text(v: Option<&Value>) -> String {
    let host = v.and_then(|c| c.get("host")).and_then(Value::as_str);
    let path = v.and_then(|c| c.get("path")).and_then(Value::as_str);
    match (host, path) {
        (Some(host), Some(path)) => format!("{host}:{path}"),
        _ => "unknown".to_string(),
    }
}

fn error_message(v: &Value, r: &Redactor) -> String {
    let message = match v {
        Value::String(s) => s.clone(),
        Value::Null => "unknown error".to_string(),
        Value::Object(_) => v
            .get("message")
            .or_else(|| v.get("error"))
            .and_then(Value::as_str)
            .map_or_else(|| v.to_string(), str::to_string),
        other => other.to_string(),
    };
    cap(&r.redact(&message), SMALL_LIMIT)
}

/// Redacts the output, collapses repeated lines, and keeps it whole when it fits. Otherwise keeps the first
/// `HEAD_BYTES` and the last `TAIL_BYTES` with a pointer to the omitted middle. Redaction comes first, so a cut
/// never splits a secret into pieces that no pattern matches.
fn window_output(raw: &str, limit: usize, id: &str, r: &Redactor) -> String {
    let text = r.redact(&collapse_repeats(raw));
    if text.len() <= limit {
        return text;
    }
    let head_end = floor_boundary(&text, HEAD_BYTES);
    let tail_start = ceil_boundary(&text, text.len().saturating_sub(TAIL_BYTES)).max(head_end);
    let omitted = tail_start - head_end;
    format!(
        "{}\n[... {omitted} bytes omitted; use get_command(id={id}, offset=...) to read more ...]\n{}",
        &text[..head_end],
        &text[tail_start..],
    )
}

/// Keeps the most recent `limit` bytes (starting at a whole line when there is one), with a note on what was left out.
fn keep_tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut start = ceil_boundary(text, text.len() - limit);
    if let Some(newline) = text[start..].find('\n') {
        start += newline + 1;
    }
    format!("[... {start} bytes above omitted ...]\n{}", &text[start..])
}

/// Collapses a run of `REPEAT_MIN` or more identical lines to the line plus a count. Trailing whitespace is trimmed.
fn collapse_repeats(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().map(str::trim_end).collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for run in lines.chunk_by(|a, b| a == b) {
        let Some(line) = run.first() else {
            continue;
        };
        if run.len() >= REPEAT_MIN {
            out.push(line.to_string());
            out.push(format!("[line repeated {} times]", run.len()));
        } else {
            out.extend(run.iter().map(|l| l.to_string()));
        }
    }
    out.join("\n")
}

/// Tool results that are not shaped further: compact JSON, capped.
fn compact_json(v: &Value) -> String {
    cap(&v.to_string(), SMALL_LIMIT)
}

/// Truncates to at most `limit` bytes (ending with a marker when cut), on a character boundary.
fn cap(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let keep = floor_boundary(s, limit.saturating_sub(TRUNCATED_MARK.len()));
    format!("{}{TRUNCATED_MARK}", &s[..keep])
}

/// Text for a scalar field: strings as they are, numbers and booleans as written, anything else as `?`.
fn scalar(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => "?".to_string(),
    }
}

/// A two-element array as `a<sep>b`, anything else as `?`.
fn pair(v: Option<&Value>, sep: char) -> String {
    match v.and_then(Value::as_array).map(Vec::as_slice) {
        Some([a, b]) => format!("{}{sep}{}", scalar(Some(a)), scalar(Some(b))),
        _ => "?".to_string(),
    }
}

/// Escapes control characters (newlines, escape sequences) so a command stays on one line.
fn escape_controls(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { c.escape_default().collect::<String>() } else { c.to_string() })
        .collect()
}

/// Largest index <= `index` that is a character boundary of `s`.
fn floor_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest index >= `index` that is a character boundary of `s` (at most `s.len()`).
fn ceil_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Redactor {
        Redactor::new(Vec::new())
    }

    fn finished(output: &str) -> Value {
        json!({"outcome": "finished", "record": {
            "id": 7,
            "command": "ls",
            "source": {"kind": "agent", "call_id": "run-1"},
            "shell_pid": null,
            "cwd": {"host": "mac", "path": "/tmp"},
            "exit_code": 0,
            "pipestatus": [0],
            "status": "complete",
            "used_alt_screen": false,
            "output": output,
            "output_truncated": false,
            "raw_truncated": false,
        }})
    }

    fn run(v: &Value) -> Value {
        serde_json::from_str(&project("run_command", true, v, &none())).expect("finished output is JSON")
    }

    /// Distinct lines, no trailing newline (the collapse step drops one anyway).
    fn numbered_lines(n: usize) -> String {
        (0..n).map(|i| format!("line {i:05}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn finished_small_output_is_compact_json() {
        let out = run(&finished("hello\nworld"));
        assert_eq!(out["id"], 7);
        assert_eq!(out["exit_code"], 0);
        assert_eq!(out["status"], "complete");
        assert_eq!(out["cwd"], "mac:/tmp");
        assert_eq!(out["output"], "hello\nworld");
        assert!(out.get("pipestatus").is_none(), "all-zero pipestatus is left out");
        assert!(out.get("output_truncated").is_none());
    }

    #[test]
    fn pipestatus_is_shown_when_a_stage_failed() {
        let mut v = finished("x");
        v["record"]["pipestatus"] = json!([0, 2]);
        assert_eq!(run(&v)["pipestatus"], json!([0, 2]));
    }

    #[test]
    fn record_level_output_cap_is_reported() {
        let mut v = finished("x");
        v["record"]["output_truncated"] = json!(true);
        assert_eq!(run(&v)["output_truncated"], true);
    }

    #[test]
    fn output_at_the_limit_is_sent_whole() {
        let text = "a".repeat(RUN_OUTPUT_LIMIT);
        let out = run(&finished(&text));
        assert_eq!(out["output"], text.as_str());
    }

    #[test]
    fn huge_output_keeps_head_and_tail_with_a_paging_pointer() {
        let text = numbered_lines(3000);
        let out = run(&finished(&text));
        let shown = out["output"].as_str().unwrap();
        assert!(shown.len() < text.len());
        assert!(text.starts_with(&shown[..HEAD_BYTES]));
        assert!(shown.contains("use get_command(id=7, offset=...) to read more"));
        assert!(shown.ends_with(&text[text.len() - TAIL_BYTES..]));
        let omitted = text.len() - HEAD_BYTES - TAIL_BYTES;
        assert!(shown.contains(&format!("[... {omitted} bytes omitted;")), "{shown}");
    }

    #[test]
    fn windowing_cuts_on_character_boundaries() {
        // 7-byte units (two 3-byte characters and one ASCII byte): byte 1500 falls inside a character,
        // so the head has to be cut back to the last boundary before it.
        let text = "日日a".repeat(1000);
        let out = run(&finished(&text));
        let shown = out["output"].as_str().unwrap();
        assert!(shown.contains("bytes omitted"));
        let head: &str = shown.split('\n').next().unwrap();
        assert!(head.len() <= HEAD_BYTES && head.len() > HEAD_BYTES - 4, "head is {} bytes", head.len());
        assert!(text.starts_with(head));
    }

    #[test]
    fn runs_of_identical_lines_are_collapsed() {
        let text = "start\na\na\na\na\nb\nc\nc\nend";
        let out = run(&finished(text));
        assert_eq!(out["output"], "start\na\n[line repeated 4 times]\nb\nc\nc\nend");
    }

    #[test]
    fn trailing_whitespace_is_trimmed_per_line() {
        let out = run(&finished("one   \ntwo\t \n"));
        assert_eq!(out["output"], "one\ntwo");
    }

    #[test]
    fn redaction_is_applied_to_the_output() {
        let r = Redactor::new(vec!["hunter2-secret".to_string()]);
        let v = finished("token hunter2-secret and sk-proj-abcdefghijklmnopqrstuvwx");
        let out = serde_json::from_str::<Value>(&project("run_command", true, &v, &r)).unwrap();
        let shown = out["output"].as_str().unwrap();
        assert_eq!(shown, "token [REDACTED] and [REDACTED:api_key]");
    }

    /// A running command is JSON with its record id, state, tail and the next step. The id is what the agent waits on.
    #[test]
    fn a_running_command_gives_its_record_id_tail_and_next_step() {
        let v = json!({
            "outcome": "running", "reason": "timeout", "record_id": 4, "call_id": "run-9", "state": "running",
            "foreground": {"kind": "other", "command": "sleep 3"}, "waited_ms": 1003,
            "screen_tail": ["$ sleep 3"], "hint": "Still running. Call wait with id 4 to wait for it to finish.",
            "private_field": "not shown",
        });
        let out = run_text(&v);
        assert_eq!(out["outcome"], "running");
        assert_eq!(out["record_id"], 4);
        assert_eq!(out["state"], "running");
        assert_eq!(out["screen_tail"], json!(["$ sleep 3"]));
        assert!(out["hint"].as_str().is_some_and(|h| h.contains("wait with id 4")));
        assert!(out.get("private_field").is_none(), "only the model's fields are shown: {out}");
    }

    /// Before the shell has started the command there is no record id: it is null, and the state says so.
    #[test]
    fn a_running_command_without_a_record_id_says_null() {
        let v = json!({"outcome": "running", "reason": "timeout", "record_id": null, "state": "submitted",
                       "call_id": "run-1", "waited_ms": 5000, "screen_tail": [], "hint": "not started"});
        let out = run_text(&v);
        assert!(out["record_id"].is_null(), "{out}");
        assert_eq!(out["state"], "submitted");
    }

    /// An early return names its reason and carries the record id, the prompt row in the tail and the next step, the same
    /// shape as the timeout. Both reasons are shown.
    #[test]
    fn an_early_return_for_a_remote_shell_or_a_prompt_is_shown_with_its_reason_and_id() {
        let remote = json!({
            "outcome": "running", "reason": "remote_shell", "record_id": 9, "call_id": "run-4", "state": "running",
            "foreground": {"kind": "ssh", "command": "ssh ubuntu@lidia"}, "waited_ms": 812,
            "screen_tail": ["ubuntu@lidia:~$"],
            "hint": "`ssh ubuntu@lidia` opened a shell that has no shell integration, so commands cannot run in it yet.",
        });
        let out = run_text(&remote);
        assert_eq!(out["reason"], "remote_shell");
        assert_eq!(out["record_id"], 9);
        assert_eq!(out["screen_tail"], json!(["ubuntu@lidia:~$"]));
        assert!(out["hint"].as_str().is_some_and(|h| h.contains("opened a shell that has no shell integration")), "{out}");

        let prompt = json!({
            "outcome": "running", "reason": "needs_user", "record_id": 10, "call_id": "run-5", "state": "running",
            "waited_ms": 340, "screen_tail": ["Password: "],
            "hint": "The shell is waiting for a password or other input. Do not type it. Ask the user to handle it, then wait with id 10 until it ends.",
            "note": "The shell appears to be waiting for a password or other input. Do not type it. Ask the user to handle it, then use get_screen.",
        });
        let text = project("run_command", true, &prompt, &none());
        let (json_part, note) = text.split_once('\n').expect("the note follows the outcome on its own line");
        let out: Value = serde_json::from_str(json_part).expect("the outcome is JSON");
        assert_eq!(out["reason"], "needs_user");
        assert_eq!(out["record_id"], 10);
        assert!(out["hint"].as_str().is_some_and(|h| h.contains("wait with id 10")), "{out}");
        assert!(note.starts_with("The shell appears to be waiting for a password"), "{note}");
    }

    fn run_text(v: &Value) -> Value {
        serde_json::from_str(&project("run_command", true, v, &none())).expect("a running result is JSON")
    }

    #[test]
    fn not_run_reasons_are_named_in_words() {
        let r = none();
        let abandoned = project("run_command", true, &json!({"outcome": "not_run", "reason": "abandoned"}), &r);
        assert!(abandoned.starts_with("not run: the line was abandoned"));
        let waiting =
            project("run_command", true, &json!({"outcome": "not_run", "reason": "waiting_for_input"}), &r);
        assert!(waiting.contains("waiting for the rest of the line"));
        let cancelled = project("run_command", true, &json!({"outcome": "not_run", "reason": "cancelled"}), &r);
        assert!(cancelled.contains("cancelled while waiting; the command may still be running"), "{cancelled}");
        // Before the line was sent nothing runs, so the text must not say the command may still be running.
        let before = project("run_command", true, &json!({"outcome": "not_run", "reason": "cancelled_before_send"}), &r);
        assert!(before.contains("cancelled before the command was sent") && before.contains("Nothing ran"), "{before}");
        assert!(!before.contains("may still be running"), "{before}");
        let other = project("run_command", true, &json!({"outcome": "not_run", "reason": "later"}), &r);
        assert!(other.starts_with("{"), "unknown reason falls back to JSON: {other}");
    }

    #[test]
    fn tool_error_is_reported_as_error_text() {
        let out = project(
            "run_command",
            false,
            &json!("shell integration is not live, so commands cannot be run"),
            &none(),
        );
        assert_eq!(out, "error: shell integration is not live, so commands cannot be run");
    }

    #[test]
    fn error_object_uses_its_message() {
        let out = project("get_command", false, &json!({"message": "no finished command with id 9"}), &none());
        assert_eq!(out, "error: no finished command with id 9");
    }

    #[test]
    fn get_command_has_a_header_and_a_window() {
        let v = json!({
            "id": 3, "command": "make", "source": {"kind": "user"}, "exit_code": 2,
            "status": "complete", "used_alt_screen": false, "output_bytes": 5,
            "cwd": {"host": "box", "path": "/src"}, "output": "err\nmore",
            "offset": 0, "total_bytes": 8, "truncated": false,
        });
        let out = project("get_command", true, &v, &none());
        assert_eq!(
            out,
            "id=3 exit=2 status=complete cwd=box:/src offset=0 total_bytes=8 truncated=false\nerr\nmore"
        );
    }

    #[test]
    fn get_command_large_window_points_back_at_get_command_with_the_id() {
        let output = numbered_lines(1000);
        let v = json!({
            "id": 12, "exit_code": 0, "status": "complete", "cwd": null, "output": output,
            "offset": 0, "total_bytes": output.len(), "truncated": true,
        });
        let out = project("get_command", true, &v, &none());
        assert!(out.lines().next().unwrap().contains("truncated=true"));
        assert!(out.contains("use get_command(id=12, offset=...) to read more"));
        assert!(out.len() <= GET_OUTPUT_LIMIT + 200, "window too big: {}", out.len());
    }

    #[test]
    fn get_command_output_between_run_and_get_limits_is_sent_whole() {
        let output = "x".repeat(5000);
        let v = json!({"id": 1, "output": output, "offset": 0, "total_bytes": 5000, "truncated": false});
        let out = project("get_command", true, &v, &none());
        assert!(!out.contains("omitted"));
        assert!(out.ends_with(&"x".repeat(5000)));
    }

    #[test]
    fn get_screen_trims_blank_rows_and_shows_a_header() {
        let v = json!({
            "rows": ["$ ls", "a  b", "", "   "],
            "cursor": [1, 0], "alternate_screen": false, "size": [24, 80],
            "state": "prompt", "observer_status": "ok", "integration_status": "live",
        });
        let out = project("get_screen", true, &v, &none());
        assert_eq!(out, "screen size=24x80 cursor=1,0 alternate_screen=false state=prompt\n$ ls\na  b");
    }

    #[test]
    fn get_screen_shows_observer_and_integration_only_when_not_live() {
        let v = json!({
            "rows": ["x"], "cursor": [0, 0], "alternate_screen": true, "size": [2, 2],
            "state": "alt", "observer_status": "desynced", "integration_status": "raw",
        });
        let header = project("get_screen", true, &v, &none()).lines().next().unwrap().to_string();
        assert_eq!(
            header,
            "screen size=2x2 cursor=0,0 alternate_screen=true state=alt observer=desynced integration=raw"
        );
    }

    #[test]
    fn get_screen_keeps_the_bottom_of_a_huge_screen() {
        let rows: Vec<String> = (0..2000).map(|i| format!("row {i:05}")).collect();
        let v = json!({"rows": rows, "cursor": [0, 0], "alternate_screen": false, "size": [2000, 9],
            "state": "running", "observer_status": "ok", "integration_status": "live"});
        let out = project("get_screen", true, &v, &none());
        assert!(out.contains("bytes above omitted"));
        assert!(out.ends_with("row 01999"));
        assert!(out.len() <= SCREEN_LIMIT + 200);
    }

    fn screen(rows: &[&str]) -> Value {
        json!({
            "rows": rows, "cursor": [0, 0], "alternate_screen": false, "size": [24, 80],
            "state": "prompt", "observer_status": "ok", "integration_status": "live",
        })
    }

    #[test]
    fn todo_write_reports_the_counts_of_the_list_it_holds() {
        let v = json!({"items": [
            {"content": "a", "status": "completed"},
            {"content": "b", "status": "in_progress"},
            {"content": "c", "status": "pending"},
            {"content": "d", "status": "pending"},
            {"content": "e", "status": "completed"},
            {"content": "f", "status": "pending"},
        ]});
        assert_eq!(project("todo_write", true, &v, &none()), "todo list updated: 2 done, 1 in progress, 3 pending");
        assert_eq!(project("todo_write", true, &json!({"items": []}), &none()), "todo list cleared");
    }

    #[test]
    fn todo_write_errors_reach_the_model_as_text() {
        let err = Value::String("item 2: content is empty".to_string());
        assert_eq!(project("todo_write", false, &err, &none()), "error: item 2: content is empty");
    }

    /// An `ask_user` result keeps its whole JSON under its own cap, not the small one other tools get, and is redacted
    /// like everything else.
    #[test]
    fn ask_user_results_are_compact_json_and_redacted() {
        let stored = questions::parse(&json!({"questions": [{"question": "Which?", "header": "H",
            "options": [{"label": "a"}, {"label": "b"}]}]}))
        .unwrap();
        let answers = vec![questions::AnswerInput { question_id: "q1".into(), selected: vec![0], other: None, skipped: false }];
        let value = questions::result(&stored, &answers);
        assert_eq!(project(questions::TOOL_NAME, true, &value, &none()), value.to_string());
        let sneaky = vec![questions::AnswerInput {
            question_id: "q1".into(),
            selected: Vec::new(),
            other: Some("token hunter2-secret".into()),
            skipped: false,
        }];
        let r = Redactor::new(vec!["hunter2-secret".to_string()]);
        let shown = project(questions::TOOL_NAME, true, &questions::result(&stored, &sneaky), &r);
        assert!(shown.contains("[REDACTED]") && !shown.contains("hunter2-secret"), "{shown}");
    }

    /// The largest valid answers fit the answer cap, so the projection never cuts them mid-JSON.
    #[test]
    fn the_largest_valid_answers_survive_the_projection_whole() {
        let option = || {
            json!({"label": "l".repeat(questions::MAX_LABEL), "description": "d".repeat(questions::MAX_DESCRIPTION)})
        };
        let items: Vec<Value> = (0..questions::MAX_QUESTIONS)
            .map(|_| {
                json!({"question": "q".repeat(questions::MAX_QUESTION), "header": "h".repeat(questions::MAX_HEADER),
                    "multiple": true, "options": [option(), option(), option(), option()]})
            })
            .collect();
        let stored = questions::parse(&json!({"questions": items})).unwrap();
        let answers: Vec<questions::AnswerInput> = stored
            .iter()
            .map(|q| questions::AnswerInput {
                question_id: q.id.clone(),
                selected: (0..questions::MAX_OPTIONS as u32).collect(),
                other: Some("o".repeat(questions::MAX_OTHER)),
                skipped: false,
            })
            .collect();
        let value = questions::result(&stored, &answers);
        let shown = project(questions::TOOL_NAME, true, &value, &none());
        assert_eq!(shown, value.to_string(), "the maximum valid result is intact, not cut");
        let parsed: Value = serde_json::from_str(&shown).expect("the projected result parses");
        assert_eq!(parsed, value);
    }

    #[test]
    fn wait_first_line_gives_the_reason_and_the_time_waited() {
        let v = json!({"reason": "screen_changed", "waited_ms": 1300, "state": "running", "screen": screen(&["x"])});
        let out = project("wait", true, &v, &none());
        assert!(out.starts_with("waited 1.3s: the screen changed\n"), "{out}");

        let idle = json!({"reason": "idle", "waited_ms": 0, "state": "prompt", "screen": screen(&["$ "])});
        let out = project("wait", true, &idle, &none());
        assert!(out.starts_with("waited 0.0s: the shell is idle at the prompt"), "{out}");
    }

    #[test]
    fn wait_for_a_finished_command_gives_its_exit_status_and_output() {
        let v = json!({
            "reason": "command_finished", "waited_ms": 4200, "state": "prompt",
            "screen": screen(&["$ make"]), "record": finished("hello\nworld")["record"],
        });
        let out = project("wait", true, &v, &none());
        assert!(out.starts_with("waited 4.2s: the command finished with exit status 0\n"), "{out}");
        assert!(out.contains("command id=7 exit=0 status=complete cwd=mac:/tmp command=ls"), "{out}");
        assert!(out.ends_with("hello\nworld"), "the output tail comes last: {out}");
    }

    #[test]
    fn wait_does_not_repeat_output_the_screen_already_shows() {
        let v = json!({
            "reason": "command_finished", "waited_ms": 900, "state": "prompt",
            "screen": screen(&["$ ls", "hello", "world", "$ "]), "record": finished("hello\nworld")["record"],
        });
        let out = project("wait", true, &v, &none());
        assert!(out.contains("(the output is on the screen above)"), "{out}");
        assert_eq!(out.matches("hello").count(), 1, "the output was repeated: {out}");
    }

    #[test]
    fn wait_shows_output_the_screen_does_not_have_and_reports_empty_output() {
        let v = json!({
            "reason": "command_finished", "waited_ms": 900, "state": "prompt",
            "screen": screen(&["$ ls"]), "record": finished("hello\nworld")["record"],
        });
        let out = project("wait", true, &v, &none());
        assert!(out.ends_with("hello\nworld"), "{out}");

        let mut quiet = finished("");
        quiet["record"]["exit_code"] = json!(1);
        let v = json!({"reason": "command_finished", "waited_ms": 5, "screen": screen(&["$ true"]), "record": quiet["record"]});
        let out = project("wait", true, &v, &none());
        assert!(out.starts_with("waited 0.0s: the command finished with exit status 1\n"), "{out}");
        assert!(out.ends_with("(no output)"), "{out}");
    }

    #[test]
    fn wait_says_when_the_requested_seconds_were_clamped() {
        let v = json!({
            "reason": "timeout", "waited_ms": 120000, "state": "running",
            "note": "seconds 500 is outside 1 to 120; waited 120 s instead", "screen": screen(&["x"]),
        });
        let out = project("wait", true, &v, &none());
        assert_eq!(out.lines().nth(1), Some("seconds 500 is outside 1 to 120; waited 120 s instead"), "{out}");
    }

    #[test]
    fn wait_result_stays_near_the_screen_limit_even_for_huge_screen_and_output() {
        let rows: Vec<String> = (0..2000).map(|i| format!("row {i:05}")).collect();
        let v = json!({
            "reason": "command_finished", "waited_ms": 1, "state": "prompt",
            "screen": {"rows": rows, "cursor": [0, 0], "alternate_screen": false, "size": [2000, 9],
                       "state": "prompt", "observer_status": "ok", "integration_status": "live"},
            "record": finished(&numbered_lines(3000))["record"],
        });
        let out = project("wait", true, &v, &none());
        assert!(out.len() <= SCREEN_LIMIT + 600, "{} bytes", out.len());
        assert!(out.contains("bytes above omitted"));
        assert!(out.ends_with("line 02999"), "the output keeps its most recent lines: {out}");
    }

    #[test]
    fn list_commands_gives_one_line_per_command() {
        let v = json!([
            {"id": 4, "command": "git status", "source": {"kind": "user"}, "exit_code": 0,
             "status": "complete", "used_alt_screen": false, "output_bytes": 120},
            {"id": 5, "command": "cat a\nb", "source": {"kind": "agent", "call_id": "run-2"},
             "exit_code": null, "status": "incomplete", "used_alt_screen": false, "output_bytes": 0},
        ]);
        let out = project("list_commands", true, &v, &none());
        assert_eq!(out, "4 exit=0 bytes=120 source=user git status\n5 exit=? bytes=0 source=agent cat a\\nb");
    }

    #[test]
    fn list_commands_empty_and_odd_shapes() {
        let r = none();
        assert_eq!(project("list_commands", true, &json!([]), &r), "no finished commands");
        let odd = project("list_commands", true, &json!({"not": "a list"}), &r);
        assert!(odd.contains("not"));
    }

    #[test]
    fn list_commands_truncates_long_commands_and_redacts_them() {
        let secret = "x".repeat(300);
        let v = json!([{"id": 1, "command": format!("echo {secret}"), "output_bytes": 0}]);
        let out = project("list_commands", true, &v, &none());
        assert!(out.len() < 300, "{out}");
        assert!(out.ends_with(TRUNCATED_MARK));
        let r = Redactor::new(vec!["SECRETVALUE123".to_string()]);
        let v = json!([{"id": 2, "command": "run SECRETVALUE123 now", "output_bytes": 0}]);
        assert_eq!(project("list_commands", true, &v, &r), "2 exit=? bytes=0 source=? run [REDACTED] now");
    }

    #[test]
    fn get_cwd_is_host_and_path_or_unknown() {
        let r = none();
        assert_eq!(project("get_cwd", true, &json!({"host": "mac", "path": "/x"}), &r), "mac:/x");
        assert_eq!(project("get_cwd", true, &Value::Null, &r), "unknown");
    }

    #[test]
    fn unknown_tool_is_compact_json_capped_at_2000_bytes() {
        let big = json!({"blob": "y".repeat(5000)});
        let out = project("some_future_tool", true, &big, &none());
        assert!(out.len() <= SMALL_LIMIT, "{} bytes", out.len());
        assert!(out.ends_with(TRUNCATED_MARK));
        let small = project("some_future_tool", true, &json!({"a": 1}), &none());
        assert_eq!(small, r#"{"a":1}"#);
    }

    #[test]
    fn odd_shapes_never_panic_and_stay_bounded() {
        let values = [
            Value::Null,
            json!(1),
            json!("text"),
            json!([]),
            json!({}),
            json!([null, 1, "x"]),
            json!({"outcome": 5}),
            json!({"outcome": "finished"}),
            json!({"outcome": "finished", "record": "nope"}),
            json!({"outcome": "finished", "record": {"output": 3, "pipestatus": "x", "cwd": 9}}),
            json!({"outcome": "running", "record_id": {"a": 1}, "state": {"a": 1}}),
            json!({"outcome": "not_run"}),
            json!({"rows": "x", "size": [1], "cursor": "y"}),
            json!({"rows": [1, null, {}], "size": [1, 2, 3], "observer_status": 1}),
            json!({"id": "x", "output": null, "offset": [], "truncated": "yes"}),
            json!([{"source": 3, "command": 4}, null]),
        ];
        let names = [
            "run_command", "get_command", "get_screen", "list_commands", "get_cwd", "wait", "send_keys", "unknown",
        ];
        let r = none();
        for name in names {
            for v in &values {
                for ok in [true, false] {
                    let out = project(name, ok, v, &r);
                    assert!(out.len() < 10_000, "{name} {ok} produced {} bytes", out.len());
                }
            }
        }
    }

    fn with_foreground(mut v: Value, kind: &str, command: &str) -> Value {
        v["foreground"] = json!({"kind": kind, "command": command});
        v
    }

    /// The screen names the foreground program in one line, only when there is a hint, with the same words the agent is
    /// told elsewhere.
    #[test]
    fn get_screen_shows_the_foreground_line_only_when_there_is_one() {
        let plain = project("get_screen", true, &screen(&["ubuntu@lidia:~$"]), &none());
        assert!(!plain.contains("foreground:"), "{plain}");

        let v = with_foreground(screen(&["ubuntu@lidia:~$"]), "ssh", "ssh ubuntu@lidia");
        let out = project("get_screen", true, &v, &none());
        assert_eq!(
            out.lines().nth(1),
            Some("foreground: ssh ubuntu@lidia (a shell on another machine, without shell integration; you can read the screen but cannot run commands here). Call request_integration to ask for it."),
            "{out}"
        );
        assert!(out.ends_with("ubuntu@lidia:~$"), "the rows follow the header: {out}");
    }

    #[test]
    fn a_plain_running_command_is_named_as_still_running_not_as_unintegrated() {
        let v = with_foreground(screen(&["x"]), "other", "sleep 30");
        let out = project("get_screen", true, &v, &none());
        assert!(out.contains("foreground: sleep 30 (a command running in this shell; wait for it to finish)"), "{out}");
        assert!(!out.contains("without shell integration"), "{out}");
    }

    /// The command text of the hint is the user's, so the redactor sees it like any other output.
    #[test]
    fn the_foreground_command_is_redacted() {
        let r = Redactor::new(vec!["hunter2-secret".to_string()]);
        let v = with_foreground(screen(&["x"]), "ssh", "sshpass -p hunter2-secret ssh host");
        let out = project("get_screen", true, &v, &r);
        assert!(out.contains("foreground: sshpass -p [REDACTED] ssh host ("), "{out}");
        assert!(!out.contains("hunter2-secret"), "{out}");
    }

    /// `wait` shows the screen as get_screen does, so the hint reaches the model there too.
    #[test]
    fn wait_carries_the_foreground_line_in_its_screen() {
        let v = json!({
            "reason": "timeout", "waited_ms": 10000, "state": "running",
            "screen": with_foreground(screen(&["x"]), "ssh", "ssh h"),
        });
        let out = project("wait", true, &v, &none());
        assert!(out.contains("foreground: ssh h (a shell on another machine, without shell integration"), "{out}");
    }

    /// An odd hint (wrong kind, not an object) is left out rather than shown wrongly.
    #[test]
    fn a_malformed_foreground_is_left_out() {
        let mut v = screen(&["x"]);
        v["foreground"] = json!({"kind": "unknown_kind", "command": "x"});
        assert!(!project("get_screen", true, &v, &none()).contains("foreground:"));
        v["foreground"] = json!("ssh");
        assert!(!project("get_screen", true, &v, &none()).contains("foreground:"));
    }

    /// A remote command is not a login: its screen line says to wait for it, and offers no integration. A login does offer it.
    #[test]
    fn a_remote_command_on_screen_is_waited_for_not_integrated() {
        let make = with_foreground(screen(&["x"]), "ssh", "ssh host make");
        let text = project("get_screen", true, &make, &none());
        assert!(text.contains("foreground: ssh host make"), "{text}");
        assert!(!text.contains("request_integration"), "{text}");
        assert!(text.contains("Wait for it to end (wait)"), "{text}");
        let login = with_foreground(screen(&["x"]), "ssh", "ssh ubuntu@lidia");
        assert!(project("get_screen", true, &login, &none()).contains("Call request_integration to ask for it."));
    }

    /// The send_keys result: `sent` names the text and the keys and shows the screen after them; `finished` is the record, as
    /// run_command gives it.
    #[test]
    fn send_keys_results_say_what_went_in_and_what_is_on_the_screen() {
        let sent = json!({
            "outcome": "sent", "bytes": 6, "keys": ["Enter"], "text": "hello", "state": "running", "screen_tail": ["hello", "hello"],
        });
        let text = project("send_keys", true, &sent, &none());
        assert!(text.starts_with("sent 6 bytes of text `hello` and keys Enter. state=running"), "{text}");
        assert!(text.contains("screen:\nhello\nhello"), "{text}");
        assert!(text.contains("The command is still running"), "{text}");
        let done = json!({"outcome": "finished", "record": {"id": 4, "exit_code": 0, "status": "complete", "output": "hello"}});
        assert!(project("send_keys", true, &done, &none()).contains("\"id\":4"), "a finished result carries the record");
    }

    #[test]
    fn error_text_from_a_non_string_is_still_text() {
        assert_eq!(project("run_command", false, &Value::Null, &none()), "error: unknown error");
        assert_eq!(project("run_command", false, &json!(42), &none()), "error: 42");
    }

    /// A nested shell's screen names its location in the header; the local screen does not.
    #[test]
    fn the_screen_header_names_a_nested_location_only() {
        let mut nested = screen(&["ubuntu@lidia:~$"]);
        nested["location"] = json!("ssh ubuntu@lidia");
        let text = project("get_screen", true, &nested, &none());
        assert!(text.contains("\nlocation: ssh ubuntu@lidia (a nested shell, not the local machine)"), "{text}");
        let local = project("get_screen", true, &screen(&["$ ls"]), &none());
        assert!(!local.contains("location:"), "{local}");
    }

    /// A run result that carries the input-wait note says so after the outcome. Without the note nothing is added.
    #[test]
    fn a_run_note_follows_the_outcome() {
        let timed = json!({"outcome": "running", "record_id": 3, "state": "running", "note": "The shell appears to be waiting for a password or other input. Do not type it. Ask the user to handle it, then use get_screen."});
        let text = project("run_command", true, &timed, &none());
        assert!(text.starts_with("{\"outcome\":\"running\""), "{text}");
        assert!(text.ends_with("\nThe shell appears to be waiting for a password or other input. Do not type it. Ask the user to handle it, then use get_screen."), "{text}");
        let plain = project("run_command", true, &json!({"outcome": "running", "record_id": 3, "state": "running"}), &none());
        assert!(!plain.contains("password"), "{plain}");
    }

    /// `get_screen` with `lines` says how many lines it has, and keeps the larger limit. The plain screen does not change.
    #[test]
    fn get_screen_with_lines_names_the_history_and_keeps_more_of_it() {
        // 200 rows of 42 bytes: about 8 KB, over the screen limit (6000) and under the history limit (12 KB).
        let rows: Vec<String> = (0..200).map(|i| format!("line {i:05} {}", "x".repeat(30))).collect();
        let mut v = json!({
            "rows": rows, "cursor": [0, 0], "alternate_screen": false, "size": [24, 80],
            "state": "prompt", "observer_status": "ok", "integration_status": "live",
            "history": {"lines": 200, "truncated": false, "no_history": false},
        });
        let text = project("get_screen", true, &v, &none());
        assert!(text.lines().next().unwrap().ends_with(" lines=200"), "{text}");
        assert!(text.contains("line 00000 "), "the first line is kept under the history limit");
        assert!(text.len() > SCREEN_LIMIT, "{} bytes", text.len());
        let plain = json!({
            "rows": v["rows"].clone(), "cursor": [0, 0], "alternate_screen": false, "size": [24, 80],
            "state": "prompt", "observer_status": "ok", "integration_status": "live",
        });
        assert!(project("get_screen", true, &plain, &none()).contains("bytes above omitted"), "the screen limit cuts the same rows");

        v["history"] = json!({"lines": 3, "truncated": true, "no_history": true});
        v["note"] = json!("lines 900 is outside 1 to 200; showing 200 lines instead");
        let text = project("get_screen", true, &v, &none());
        let header = text.lines().next().unwrap();
        assert!(header.contains(" lines=3 truncated=true no_history=true"), "{header}");
        assert_eq!(text.lines().nth(1), Some("lines 900 is outside 1 to 200; showing 200 lines instead"), "{text}");

        let plain = project("get_screen", true, &screen(&["$ ls"]), &none());
        assert!(!plain.contains("lines="), "{plain}");
    }
}
