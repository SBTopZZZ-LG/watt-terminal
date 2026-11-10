//! The model-facing view of tool results.
//!
//! The UI keeps the full result. This module shapes what goes into the model's history: compact text, long output
//! cut to a head and a tail with a paging pointer, repeated lines collapsed, and everything redacted. Field names
//! follow the serialized forms in `tools.rs` (`RunOutcome`, `ScreenView`, `CommandView`, `RecordSummary`) and
//! `record.rs`. Tools return different shapes and errors arrive as plain text, so every access is a checked lookup:
//! a missing or oddly typed field becomes a placeholder, never a panic.

use serde_json::{Value, json};

use crate::core::chat::redact::Redactor;
use crate::core::chat::todo;

/// `run_command` output longer than this is cut to a head and tail. Output at or under it is sent whole.
const RUN_OUTPUT_LIMIT: usize = 4500;
/// `get_command` output longer than this is cut to a head and tail.
const GET_OUTPUT_LIMIT: usize = 6000;
const HEAD_BYTES: usize = 1500;
const TAIL_BYTES: usize = 3000;
/// Screen and command-list text keep their most recent bytes up to these limits.
const SCREEN_LIMIT: usize = 6000;
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
            todo::TOOL_NAME => todo_write_text(result),
            "list_commands" => list_commands_text(result, redactor),
            "get_cwd" => cwd_text(Some(result)),
            _ => compact_json(result),
        }
    };
    redactor.redact(&text)
}

fn run_command_text(v: &Value, r: &Redactor) -> String {
    match v.get("outcome").and_then(Value::as_str) {
        Some("finished") => finished_text(v.get("record"), r),
        Some("timed_out") => timed_out_text(v),
        Some("not_run") => not_run_text(v),
        _ => compact_json(v),
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

fn timed_out_text(v: &Value) -> String {
    let state = scalar(v.get("state"));
    if state == "running" {
        "timed out after the wait; shell state: running; the command may still be running in the terminal".to_string()
    } else {
        // Only `running` means the command may still be going. Any other state is reported as it is.
        format!("timed out after the wait; shell state: {state}")
    }
}

fn not_run_text(v: &Value) -> String {
    match v.get("reason").and_then(Value::as_str).unwrap_or("") {
        "abandoned" => "not run: the line was abandoned before it ran (cancelled, or finished by the user)".to_string(),
        "waiting_for_input" => {
            "not run: the shell is waiting for the rest of the line, so the command is incomplete. Nothing was sent to cancel it."
                .to_string()
        }
        "cancelled" => "not run: the turn was cancelled before the command ran".to_string(),
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
    format!("{}\n{}", screen_header(v), screen_body(v, r, SCREEN_LIMIT))
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
    lines.push(format!("{}\n{body}", screen_header(screen)));
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

fn screen_header(v: &Value) -> String {
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
    header
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

    #[test]
    fn timed_out_while_running_may_still_be_running() {
        let v = json!({"outcome": "timed_out", "state": "running"});
        let out = project("run_command", true, &v, &none());
        assert_eq!(
            out,
            "timed out after the wait; shell state: running; the command may still be running in the terminal"
        );
    }

    #[test]
    fn timed_out_in_another_state_does_not_claim_running() {
        let v = json!({"outcome": "timed_out", "state": "alt"});
        let out = project("run_command", true, &v, &none());
        assert_eq!(out, "timed out after the wait; shell state: alt");
        assert!(!out.contains("still be running"));
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
        assert!(cancelled.contains("cancelled before the command ran"));
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
            json!({"outcome": "timed_out", "state": {"a": 1}}),
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

    #[test]
    fn error_text_from_a_non_string_is_still_text() {
        assert_eq!(project("run_command", false, &Value::Null, &none()), "error: unknown error");
        assert_eq!(project("run_command", false, &json!(42), &none()), "error: 42");
    }
}
