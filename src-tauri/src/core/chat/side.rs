//! One model call outside a turn, for the side commands (`/btw`, `/compact`): a question or a summary that never
//! touches the stored history or the transcript.
//!
//! A `SideCall` carries its own provider config, model, token cap, redactor and cancel token. `ask` prepends a short
//! dedicated system prompt (never the full tool/approval prompt), sends one non-streamed request with no tools, and
//! returns the trimmed redacted answer. No calibration, no context usage events, no history writes. A missing model
//! configuration is the caller's to refuse; nothing here emulates a model.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::core::chat::agent_loop::LlmSettings;
use crate::core::chat::redact::Redactor;
use crate::core::llm::{self, ChatRequest, LlmError, ProviderConfig, TurnCancel};

/// The system prompt for a side question: brief, no tools, nothing stored.
pub const BTW_SYSTEM: &str = "This is a side question about the conversation below. Answer briefly and directly. Do not \
    use tools, do not change anything, and do not treat this as a new turn. The exchange is not stored.";
/// The system prompt for a compaction summary: a summary only, nothing else.
pub const SUMMARY_SYSTEM: &str =
    "You summarize conversations for context compaction. Reply with a concise summary only.";
/// The system prompt for a title suggestion: a title only, nothing else.
pub const TITLE_SYSTEM: &str =
    "You name conversations. Reply with a short, descriptive title of at most six words, and nothing else.";
/// The error text when the provider's reply is empty after trimming and redaction. Kept as a constant so
/// callers can treat an empty reply as a fallback case rather than a failure.
pub const EMPTY_REPLY: &str = "the provider returned an empty reply";

/// The context budget when the model's limit is not known, in characters.
const DEFAULT_BUDGET: usize = 60_000;
/// The smallest context budget, in characters: below this the selection is too thin to be useful.
const BUDGET_FLOOR: usize = 8_000;
/// The largest context budget, in characters: above this the side request costs more than it is worth.
const BUDGET_CAP: usize = 200_000;
/// A tool result in the summarizer input is cut to this many characters, ellipsis included.
const TOOL_RESULT_CHARS: usize = 2_000;
/// Tool call arguments in the summarizer input are cut to this many characters, ellipsis included.
const TOOL_ARGS_CHARS: usize = 300;
/// The marker between the kept head and tail of an over-budget summarizer input.
const OMITTED_MARKER: &str = "[... older content omitted ...]";
/// The longest `suffixed_title` result, in characters, suffix included.
const MAX_TITLE_CHARS: usize = 120;
/// The title stem when the old title is empty.
const UNTITLED: &str = "Conversation";

/// One model call outside a turn. Built per side command from the conversation's settings; the caller reserves the
/// turn slot and passes that turn's cancel token, so Ctrl-C aborts the side call.
pub struct SideCall {
    client: reqwest::Client,
    cfg: ProviderConfig,
    model: String,
    max_tokens: u32,
    redactor: Redactor,
    cancel: Arc<TurnCancel>,
}

impl SideCall {
    pub fn new(
        client: &reqwest::Client,
        settings: &LlmSettings,
        conversation_id: &str,
        cap: u32,
        cancel: Arc<TurnCancel>,
    ) -> Self {
        SideCall {
            client: client.clone(),
            cfg: settings.provider_config(conversation_id),
            model: settings.model.clone(),
            max_tokens: settings.max_tokens.map_or(cap, |m| m.min(cap)),
            redactor: settings.redactor(conversation_id),
            cancel,
        }
    }

    /// One non-streamed request: the system prompt first, then `messages`. No tools, no history writes. The whole call
    /// (not the idle gap) must finish within `timeout`; a timeout cancels the shared token and fails. The answer and
    /// any error are redacted; an empty answer is an error.
    /// A variant overlay is deliberately not applied here: side calls (`/btw`, `/compact`) run on the base model in v1.
    pub async fn ask(&self, system: &str, messages: Vec<Value>, timeout: Duration) -> Result<String, String> {
        let mut full = Vec::with_capacity(messages.len() + 1);
        full.push(json!({"role": "system", "content": system}));
        full.extend(messages);
        let req =
            ChatRequest { model: self.model.clone(), messages: full, tools: None, max_tokens: Some(self.max_tokens), extra: None };
        let reply = match tokio::time::timeout(timeout, llm::complete(&self.client, &self.cfg, &req, &self.cancel)).await
        {
            Err(_) => {
                self.cancel.cancel();
                return Err(format!("the side request timed out after {}s", timeout.as_secs()));
            }
            Ok(Err(e)) => return Err(self.redactor.redact(&side_error(&e))),
            Ok(Ok(reply)) => reply,
        };
        let text = self.redactor.redact(reply.text.trim());
        if text.is_empty() {
            return Err(EMPTY_REPLY.to_string());
        }
        Ok(text)
    }
}

/// The notice for a failed side call. Provider text is already free of the key; the caller redacts it once more.
fn side_error(e: &LlmError) -> String {
    match e {
        LlmError::Http { status, message, .. } => format!("The provider returned HTTP {status}: {message}"),
        LlmError::Provider { message, .. } => format!("The provider reported an error: {message}"),
        LlmError::Transport(detail) => format!("I could not reach the provider: {detail}"),
        LlmError::Parse(detail) => format!("I could not read the provider's reply: {detail}"),
        LlmError::TooLarge(detail) => format!("The provider's reply is too large: {detail}"),
        LlmError::Cancelled => "The side request was cancelled.".to_string(),
    }
}

/// The context budget for a side request, in characters: about three characters per token of the model's limit minus
/// the `reserve` kept for the reply, within a floor and a cap. `None` (no known limit) gives the default budget.
pub fn budget_chars(limit: Option<u32>, reserve: u32) -> usize {
    let Some(limit) = limit else { return DEFAULT_BUDGET };
    (limit.saturating_sub(reserve) as usize).saturating_mul(3).clamp(BUDGET_FLOOR, BUDGET_CAP)
}

/// The conversation as context for a side question: user and assistant text only, as `role`/`content` messages.
/// Whole turns are taken newest-first (a turn starts at a user message) up to `budget` characters, then reversed back
/// to chronological order. Tool messages are dropped; assistant tool calls become a `[tools: name, ...]` line. The
/// first message is always a user message; empty when there is no user message.
pub fn conversation_context(messages: &[Value], budget: usize) -> Vec<Value> {
    let starts: Vec<usize> =
        messages.iter().enumerate().filter(|(_, m)| role(m) == Some("user")).map(|(i, _)| i).collect();
    if starts.is_empty() {
        return Vec::new();
    }
    let mut picked: Vec<Vec<Value>> = Vec::new();
    let mut used = 0usize;
    for (n, &start) in starts.iter().enumerate().rev() {
        let end = starts.get(n + 1).copied().unwrap_or(messages.len());
        // A turn whose user text is empty (e.g. only unknown attachment parts) would otherwise
        // start with an assistant message: skip it, other turns may still be included.
        if user_text(messages[start].get("content").unwrap_or(&Value::Null)).is_empty() {
            continue;
        }
        let turn = context_turn(&messages[start..end]);
        if turn.is_empty() {
            continue;
        }
        // The first picked turn is not assistant-first: its user text is non-empty, so the turn
        // starts with a user message.
        if turn.first().and_then(|m| m.get("role").and_then(Value::as_str)) != Some("user") {
            continue;
        }
        let cost: usize = turn.iter().map(|m| m["content"].as_str().map_or(0, |c| c.chars().count())).sum();
        // The newest turn is always kept, so a side question never runs with no context at all.
        // When it alone exceeds the budget, cap each message to an even share so the whole
        // context fits.
        if picked.is_empty() && cost > budget {
            if budget == 0 {
                return Vec::new();
            }
            let share = (budget / turn.len()).max(1);
            let truncated: Vec<Value> = turn
                .into_iter()
                .map(|m| {
                    let capped = cap_chars(m["content"].as_str().unwrap_or(""), share);
                    json!({"role": m.get("role").cloned().unwrap_or(Value::Null), "content": capped})
                })
                .collect();
            picked.push(truncated);
            break;
        }
        if !picked.is_empty() && used + cost > budget {
            break;
        }
        used += cost;
        picked.push(turn);
        if used >= budget {
            break;
        }
    }
    picked.into_iter().rev().flatten().collect()
}

/// One turn of `conversation_context`: the user and assistant messages of `slice` as `role`/`content` pairs.
fn context_turn(slice: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in slice {
        match role(m) {
            Some("user") => {
                let text = user_text(m.get("content").unwrap_or(&Value::Null));
                if !text.is_empty() {
                    out.push(json!({"role": "user", "content": text}));
                }
            }
            Some("assistant") => {
                let mut text = m.get("content").and_then(Value::as_str).unwrap_or("").to_string();
                let tools = tool_names(m);
                if !tools.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!("[tools: {}]", tools.join(", ")));
                }
                if !text.is_empty() {
                    out.push(json!({"role": "assistant", "content": text}));
                }
            }
            _ => {}
        }
    }
    out
}

/// The summarizer input for `messages`: one string with roles, tool-call names and truncated tool results. Whole lines
/// while it fits `budget`; over budget, the head and the tail around the omitted marker. Empty when there is nothing
/// to summarize.
pub fn summarizer_input(messages: &[Value], budget: usize) -> String {
    let full = summary_lines(messages).join("\n");
    if full.chars().count() <= budget {
        return full;
    }
    let head_keep = budget / 2;
    let tail_keep = budget - head_keep;
    let chars: Vec<char> = full.chars().collect();
    let head: String = chars.iter().take(head_keep).collect();
    let tail: String = chars.iter().skip(chars.len().saturating_sub(tail_keep)).collect();
    format!("{head}\n{OMITTED_MARKER}\n{tail}")
}

/// One line per user, assistant and tool message of `messages`.
fn summary_lines(messages: &[Value]) -> Vec<String> {
    let mut names: HashMap<String, String> = HashMap::new();
    for m in messages.iter().filter(|m| role(m) == Some("assistant")) {
        for call in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
            let id = call.get("id").and_then(Value::as_str);
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .or_else(|| call.get("name").and_then(Value::as_str));
            if let (Some(id), Some(name)) = (id, name) {
                names.insert(id.to_string(), name.to_string());
            }
        }
    }
    let mut lines = Vec::new();
    for m in messages {
        match role(m) {
            Some("user") => {
                let text = user_text(m.get("content").unwrap_or(&Value::Null));
                if !text.is_empty() {
                    lines.push(format!("User: {text}"));
                }
            }
            Some("assistant") => {
                let text = m.get("content").and_then(Value::as_str).unwrap_or("").trim();
                if !text.is_empty() {
                    lines.push(format!("Assistant: {text}"));
                }
                for call in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    let name = call
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .or_else(|| call.get("name").and_then(Value::as_str))
                        .unwrap_or("unknown");
                    let args = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                        .or_else(|| call.get("arguments").and_then(Value::as_str))
                        .unwrap_or("");
                    if args.is_empty() || args == "{}" {
                        lines.push(format!("Assistant tool call: {name}"));
                    } else {
                        lines.push(format!("Assistant tool call: {name} {}", cap_chars(args, TOOL_ARGS_CHARS)));
                    }
                }
            }
            Some("tool") => {
                let name = m
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .and_then(|id| names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "result".to_string());
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                lines.push(format!("Tool {name}: {}", cap_chars(content, TOOL_RESULT_CHARS)));
            }
            _ => {}
        }
    }
    lines
}

/// The text of a user message: plain string content as-is; parts keep their text, images become `[image]`, documents
/// become `[file: name]`. Anything else is not context for a side call.
fn user_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => {
            let mut segments = Vec::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = part.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            segments.push(text.to_string());
                        }
                    }
                    Some("watt_image") | Some("image_url") => segments.push("[image]".to_string()),
                    Some("watt_file") => {
                        let name = part.get("name").and_then(Value::as_str).unwrap_or("attachment");
                        segments.push(format!("[file: {name}]"));
                    }
                    _ => {}
                }
            }
            segments.join("\n")
        }
        _ => String::new(),
    }
}

/// The tool-call names of an assistant message, in order.
fn tool_names(m: &Value) -> Vec<String> {
    m.get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    call.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .or_else(|| call.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `text` cut to `max` characters, the last one an ellipsis when it was cut.
fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The first message of a compacted conversation: the summary, labelled as context for what follows, not a
/// new request.
pub fn compact_seed(summary: &str) -> String {
    format!("Compacted summary (from the previous conversation; not a new request):\n\n{summary}")
}

/// Cleans a model's title reply: the first non-empty line, without surrounding quotes or backticks, a leading
/// `Title:` label or trailing punctuation, with runs of whitespace collapsed. `None` when nothing usable remains:
/// empty, over 60 characters, or holding control characters.
pub fn clean_title(reply: &str) -> Option<String> {
    for line in reply.lines() {
        let mut title = line.trim();
        if title.is_empty() {
            continue;
        }
        // A leading `Title:` label (any case), e.g. `Title: Fix the build`.
        title = strip_title_label(title);
        // Surrounding quotes or backticks, e.g. `"Fix the build"`. One layer at a time, so nested layers unwrap fully.
        loop {
            let bytes = title.as_bytes();
            if bytes.len() >= 2 {
                let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
                if first == last && (first == b'"' || first == b'\'' || first == b'`') {
                    title = title[1..title.len() - 1].trim();
                    continue;
                }
            }
            break;
        }
        title = strip_title_label(title);
        let title = title.trim_end_matches(['.', '!', '?', ',', ';', ':']).trim();
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() || title.chars().count() > 60 || title.chars().any(|c| c.is_control()) {
            continue;
        }
        return Some(title);
    }
    None
}

/// Strips a leading `Title:` label (any case). `get` keeps this safe for titles starting in multibyte text.
fn strip_title_label(title: &str) -> &str {
    match title.get(..6) {
        Some(prefix) if prefix.eq_ignore_ascii_case("title:") => title[6..].trim(),
        _ => title,
    }
}

/// The title for a `suggest_name` reply: the cleaned model title, or `fallback` (the derived first-message title)
/// when the reply is empty or cleans to nothing. Real errors (HTTP/transport/cancel/timeout) stay errors.
pub fn title_or_fallback(reply: Result<String, String>, fallback: String) -> Result<String, String> {
    match reply {
        Ok(text) => Ok(clean_title(&text).unwrap_or(fallback)),
        Err(e) if e == EMPTY_REPLY => Ok(fallback),
        Err(e) => Err(e),
    }
}

/// The title for a derived conversation: the trimmed old title (or a fallback when it is empty) plus the suffix,
/// capped at 120 characters while keeping the suffix whole.
pub fn suffixed_title(old: &str, suffix: &str) -> String {
    let stem = old.trim();
    let stem = if stem.is_empty() { UNTITLED } else { stem };
    let titled = format!("{stem}{suffix}");
    if titled.chars().count() <= MAX_TITLE_CHARS {
        return titled;
    }
    let keep = MAX_TITLE_CHARS.saturating_sub(suffix.chars().count());
    let cut: String = stem.chars().take(keep).collect();
    format!("{}{suffix}", cut.trim_end())
}

fn role(m: &Value) -> Option<&str> {
    m.get("role").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Value {
        json!({"role": "user", "content": text})
    }

    fn assistant(text: &str) -> Value {
        json!({"role": "assistant", "content": text})
    }

    fn result(call_id: &str, content: &str) -> Value {
        json!({"role": "tool", "tool_call_id": call_id, "content": content})
    }

    #[test]
    fn the_budget_defaults_without_a_limit_and_scales_with_it() {
        assert_eq!(budget_chars(None, 0), 60_000);
        assert_eq!(budget_chars(None, 4000), 60_000, "no known limit means the default, whatever the reserve");
        // About three characters per token after the reserve.
        assert_eq!(budget_chars(Some(32_000), 2_000), 90_000);
        // The reserve never borrows below zero.
        assert_eq!(budget_chars(Some(1_000), 4_000), 8_000, "a reserve past the limit still gives the floor");
        assert_eq!(budget_chars(Some(10_000), 9_000), 8_000, "a thin remainder gives the floor");
        assert_eq!(budget_chars(Some(100_000), 4_000), 200_000, "a huge limit gives the cap");
    }

    #[test]
    fn context_keeps_text_and_marks_attachments_and_tool_calls() {
        let messages = vec![
            user("first question"),
            assistant("first answer"),
            json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "look at this"},
                    {"type": "watt_image", "sha256": "a", "mime": "image/png", "name": "shot.png"},
                    {"type": "watt_file", "name": "notes.txt", "bytes": 3, "text": "abc"},
                ],
            }),
            json!({
                "role": "assistant",
                "content": "seen",
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_cwd", "arguments": "{}"}}],
            }),
            result("c1", "/tmp"),
        ];
        let context = conversation_context(&messages, 60_000);
        assert_eq!(context.len(), 4, "tool messages are dropped: {context:?}");
        assert_eq!(context[0]["role"], "user");
        assert_eq!(context[1]["role"], "assistant");
        assert!(context[2]["content"].as_str().is_some_and(|c| c.contains("look at this")), "{context:?}");
        assert!(context[2]["content"].as_str().is_some_and(|c| c.contains("[image]")), "{context:?}");
        assert!(context[2]["content"].as_str().is_some_and(|c| c.contains("[file: notes.txt]")), "{context:?}");
        assert!(
            context[3]["content"].as_str().is_some_and(|c| c.contains("[tools: get_cwd]")),
            "{context:?}"
        );
    }

    #[test]
    fn context_takes_whole_turns_newest_first_and_never_starts_on_an_assistant() {
        let messages = vec![
            assistant("stray answer with no question"),
            result("c0", "stray result"),
            user("aaa"),
            assistant("bbb"),
            user("ccc"),
            assistant("ddd"),
        ];
        // "aaa\nbbb" is 6 chars and "ccc\nddd" is 6 chars: room for one turn only.
        let context = conversation_context(&messages, 6);
        assert_eq!(context.len(), 2, "one whole turn: {context:?}");
        assert_eq!(context[0]["content"], json!("ccc"), "the newest turn wins: {context:?}");
        assert_eq!(context[1]["content"], json!("ddd"), "{context:?}");

        let context = conversation_context(&messages, 60_000);
        assert_eq!(context.len(), 4, "the stray assistant is never the start: {context:?}");
        assert_eq!(context[0]["content"], json!("aaa"));

        assert!(conversation_context(&[], 60_000).is_empty());
        assert!(conversation_context(&[assistant("only")], 60_000).is_empty(), "no user message means no context");
    }

    #[test]
    fn newest_huge_turn_is_truncated_to_fit_the_budget() {
        let huge = "y".repeat(5_000);
        let messages = vec![user("old"), assistant("old answer"), user(&huge), assistant("late answer")];
        let budget = 100;
        let context = conversation_context(&messages, budget);
        assert!(!context.is_empty(), "the newest turn is always kept");
        assert_eq!(context[0]["role"], json!("user"), "chronological, starting with a user message: {context:?}");
        let total: usize =
            context.iter().map(|m| m["content"].as_str().map_or(0, |c| c.chars().count())).sum();
        assert!(total <= budget, "total {total} exceeds budget {budget}: {context:?}");
    }

    #[test]
    fn attachment_only_turn_is_skipped_not_assistant_first() {
        let unknown_user = json!({"role": "user", "content": [{"type": "mystery", "foo": "bar"}]});
        let lone = vec![unknown_user.clone(), assistant("late answer")];
        let context = conversation_context(&lone, 60_000);
        assert!(context.is_empty(), "no qualifying turn means empty, never assistant-first: {context:?}");

        let messages = vec![user("real"), assistant("reply"), unknown_user, assistant("late answer")];
        let context = conversation_context(&messages, 60_000);
        assert_eq!(context.len(), 2, "the other turn is still included: {context:?}");
        assert_eq!(context[0]["role"], json!("user"), "{context:?}");
        assert_eq!(context[0]["content"], json!("real"), "{context:?}");
        assert_eq!(context[1]["content"], json!("reply"), "{context:?}");
    }

    #[test]
    fn the_summarizer_input_names_calls_and_cuts_results_and_marks_what_was_left_out() {
        let long = "x".repeat(5_000);
        let messages = vec![
            user("do the thing"),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "run_command", "arguments": "{\"command\":\"make\"}"}}],
            }),
            result("c1", &long),
            assistant("done"),
        ];
        let full = summarizer_input(&messages, 100_000);
        assert!(full.contains("User: do the thing"), "{full}");
        assert!(full.contains("Assistant tool call: run_command"), "{full}");
        assert!(full.contains("Tool run_command: "), "{full}");
        assert!(!full.contains(&long), "the tool result is truncated");
        assert!(!full.contains(OMITTED_MARKER), "nothing is left out within budget: {full}");

        let over = summarizer_input(&messages, 100);
        assert!(over.contains(OMITTED_MARKER), "over budget the middle goes: {over}");
        assert!(over.starts_with("User: do the thing"), "the head is kept: {over}");
        assert!(over.ends_with("Assistant: done"), "the tail is kept: {over}");
        assert!(summarizer_input(&[], 100).is_empty());
    }

    #[test]
    fn the_compact_seed_labels_the_summary_as_context_not_a_request() {
        let seed = compact_seed("decided to ship it");
        assert!(seed.contains("not a new request"), "{seed}");
        assert!(seed.ends_with("decided to ship it"), "{seed}");
    }

    #[test]
    fn titles_keep_the_suffix_and_cap_at_120_characters() {
        assert_eq!(suffixed_title("My chat", " (fork)"), "My chat (fork)");
        assert_eq!(suffixed_title("   ", " (compacted)"), "Conversation (compacted)");
        assert_eq!(suffixed_title("  padded  ", " (fork)"), "padded (fork)", "the old title is trimmed");
        let long = "t".repeat(200);
        let titled = suffixed_title(&long, " (compacted)");
        assert_eq!(titled.chars().count(), 120, "{titled}");
        assert!(titled.ends_with(" (compacted)"), "the suffix survives the cap: {titled}");
        // Multibyte stems cut on a character boundary.
        let titled = suffixed_title(&"日".repeat(200), " (fork)");
        assert_eq!(titled.chars().count(), 120);
        assert!(titled.ends_with(" (fork)"));
        assert!(titled.starts_with("日"));
    }

    #[test]
    fn suggested_titles_take_the_first_line_cleaned() {
        assert_eq!(clean_title("Fix the build"), Some("Fix the build".to_string()));
        assert_eq!(clean_title("\n\n  Fix the build  \nsecond line\n"), Some("Fix the build".to_string()));
        assert_eq!(clean_title("\"Fix the build\""), Some("Fix the build".to_string()));
        assert_eq!(clean_title("`Fix the build`"), Some("Fix the build".to_string()));
        assert_eq!(clean_title("'Fix the build'"), Some("Fix the build".to_string()));
        assert_eq!(clean_title("Title: Fix the build"), Some("Fix the build".to_string()));
        assert_eq!(clean_title("\"Title: Fix the build.\""), Some("Fix the build".to_string()));
        assert_eq!(clean_title("Fix the build!"), Some("Fix the build".to_string()));
        assert_eq!(clean_title("Fix   the\nbuild"), Some("Fix the".to_string()), "only the first line is kept");
        assert_eq!(clean_title("  Fix\t the   build  "), Some("Fix the build".to_string()));
    }

    #[test]
    fn suggested_titles_reject_empty_long_and_controlled_replies() {
        assert_eq!(clean_title(""), None);
        assert_eq!(clean_title("   \n  \n"), None);
        assert_eq!(clean_title("\"\""), None, "quotes alone leave nothing");
        assert_eq!(clean_title("Title:"), None, "a label alone leaves nothing");
        assert_eq!(clean_title(&"t".repeat(60)), Some("t".repeat(60)));
        assert_eq!(clean_title(&"t".repeat(61)), None, "over 60 characters is rejected");
        assert_eq!(clean_title("Fix\x07the build"), None, "control characters are rejected");
        assert_eq!(clean_title("ok\x7f"), None, "DEL is a control character");
    }

    #[test]
    fn an_empty_or_unusable_title_falls_back_while_real_errors_stay_errors() {
        let fallback = "Fix the parser".to_string();
        assert_eq!(
            title_or_fallback(Ok("Fix the build".to_string()), fallback.clone()),
            Ok("Fix the build".to_string())
        );
        assert_eq!(
            title_or_fallback(Ok("   \n  ".to_string()), fallback.clone()),
            Ok(fallback.clone()),
            "an unusable title falls back"
        );
        assert_eq!(
            title_or_fallback(Err(EMPTY_REPLY.to_string()), fallback.clone()),
            Ok(fallback.clone()),
            "an empty reply falls back"
        );
        assert_eq!(
            title_or_fallback(Err("the side request timed out after 90s".to_string()), fallback.clone()),
            Err("the side request timed out after 90s".to_string()),
            "a timeout stays an error"
        );
    }

    #[test]
    fn a_side_call_caps_its_tokens_at_the_smaller_of_the_setting_and_the_cap() {
        let client = llm::http_client().expect("a client builds without a network");
        let settings = LlmSettings {
            chat_url: "https://x.test/v1/chat/completions".to_string(),
            api_key: "sk-test-SECRETVALUE".to_string(),
            model: "m".to_string(),
            headers: Vec::new(),
            stream: true,
            max_tokens: Some(500),
            context_limit: None,
            extra: None,
        };
        let cancel = Arc::new(TurnCancel::new());
        let under = SideCall::new(&client, &settings, "ses_abc", 1200, cancel.clone());
        assert_eq!(under.max_tokens, 500, "the setting wins when it is smaller");
        let over = SideCall::new(&client, &settings, "ses_abc", 200, cancel);
        assert_eq!(over.max_tokens, 200, "the cap wins when it is smaller");
        let unset = LlmSettings { max_tokens: None, ..settings.clone() };
        let fallback = SideCall::new(&client, &unset, "ses_abc", 1200, Arc::new(TurnCancel::new()));
        assert_eq!(fallback.max_tokens, 1200, "no setting means the cap");
    }

    /// A side call never carries the variant overlay, even when the settings have one: it runs on the base model.
    #[tokio::test]
    async fn a_side_call_leaves_the_variant_overlay_behind() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = seen.clone();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                data.extend_from_slice(&buf[..n]);
                let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break;
                }
            }
            *kept.lock().unwrap_or_else(|e| e.into_inner()) = data;
            let body = r#"{"choices":[{"message":{"role":"assistant","content":"brief answer"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(response.as_bytes());
        });

        let client = llm::http_client().expect("a client builds without a network");
        let settings = LlmSettings {
            chat_url: format!("http://{addr}/v1/chat/completions"),
            api_key: String::new(),
            model: "m".to_string(),
            headers: Vec::new(),
            stream: true,
            max_tokens: None,
            context_limit: None,
            extra: Some(serde_json::from_str(r#"{"reasoning":{"effort":"high"}}"#).unwrap()),
        };
        let call = SideCall::new(&client, &settings, "ses_abc", 64, Arc::new(TurnCancel::new()));
        let answer = call.ask(BTW_SYSTEM, Vec::new(), Duration::from_secs(10)).await.unwrap();
        assert_eq!(answer, "brief answer");

        let raw = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let text = String::from_utf8_lossy(&raw);
        let body: Value =
            serde_json::from_str(text.split("\r\n\r\n").nth(1).expect("the request has a body"))
                .expect("the side body is JSON");
        assert!(body.get("reasoning").is_none(), "the overlay was sent: {body}");
        assert_eq!(body["model"], "m", "{body}");
    }
}
