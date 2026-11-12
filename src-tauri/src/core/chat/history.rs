//! The conversation as the model sees it: raw chat-completions messages, with tool-call pairing held.
//!
//! Every tool call an assistant message makes gets exactly one `tool` message before the next user or assistant
//! message. A call left without a result is closed with a short reason, never dropped, so a provider never rejects
//! a request because of an orphan call. The system message is not stored; `messages` puts it in front.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::core::chat::attachments::{self, Resolver};
use crate::core::chat::trim::{OLD_OUTPUT_NOTE, OMITTED_TURNS_NOTE, TrimPlan};

/// What replaces a tool result when `elide_tool_results` drops it from the request.
pub const ELIDED_RESULT: &str = "[output elided; call get_command to read it again]";

/// Reasoning fields that are removed from stored assistant messages unless `echo_reasoning` is set.
const REASONING_KEYS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_details"];

#[derive(Debug, Default)]
pub struct History {
    messages: Vec<Value>,
    /// Keep reasoning fields on stored assistant messages so they can be sent back on the next request.
    pub echo_reasoning: bool,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a user message. Calls still pending from the last assistant message are closed first.
    pub fn push_user(&mut self, text: &str) {
        self.push_user_content(Value::String(text.to_string()));
    }

    /// Appends a user message with the given content: a string, or the parts array of a message with attachments (see
    /// core/chat/attachments). Calls still pending from the last assistant message are closed first.
    pub fn push_user_content(&mut self, content: Value) {
        self.close_pending("skipped: turn ended");
        self.messages.push(json!({"role": "user", "content": content}));
    }

    /// Appends an assistant message as given (reasoning fields removed unless `echo_reasoning` is set). Calls still
    /// pending from the previous assistant message are closed first.
    pub fn push_assistant(&mut self, mut raw: Value) {
        self.close_pending("skipped: no result was recorded");
        if !self.echo_reasoning {
            strip_reasoning(&mut raw);
        }
        self.messages.push(raw);
    }

    /// Records the result of a pending call. Returns false, and stores nothing, when `call_id` is not pending
    /// (unknown, or already answered).
    pub fn push_tool_result(&mut self, call_id: &str, content: &str) -> bool {
        if !self.pending_call_ids().iter().any(|id| id == call_id) {
            return false;
        }
        self.messages.push(json!({"role": "tool", "tool_call_id": call_id, "content": content}));
        true
    }

    /// The tool-call ids of the last assistant message that have no result yet, in order, without duplicates.
    pub fn pending_call_ids(&self) -> Vec<String> {
        let Some((last, assistant)) =
            self.messages.iter().enumerate().rev().find(|(_, m)| role(m) == Some("assistant"))
        else {
            return Vec::new();
        };
        let calls: Vec<String> = assistant
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls.iter().filter_map(|c| c.get("id").and_then(Value::as_str)).map(str::to_string).collect()
            })
            .unwrap_or_default();
        let answered: HashSet<&str> = self
            .messages
            .iter()
            .skip(last + 1)
            .filter(|m| role(m) == Some("tool"))
            .filter_map(|m| m.get("tool_call_id").and_then(Value::as_str))
            .collect();
        let mut pending: Vec<String> = Vec::new();
        for id in calls {
            if !answered.contains(id.as_str()) && !pending.contains(&id) {
                pending.push(id);
            }
        }
        pending
    }

    /// Gives every pending call a `tool` message with `reason` as its content, so the pairing holds.
    pub fn close_pending(&mut self, reason: &str) {
        for id in self.pending_call_ids() {
            self.messages.push(json!({"role": "tool", "tool_call_id": id, "content": reason}));
        }
    }

    /// The request's message list: the system message first, then the stored messages. This is the one place where
    /// stored messages become wire messages. A user message with attachments is translated (see
    /// `attachments::wire_content`): its images are read through `resolver`, and only the recent ones are sent. Every
    /// other message is sent as stored.
    pub fn messages(&self, system: &str, resolver: &mut Resolver<'_>) -> Vec<Value> {
        self.messages_with(system, resolver, &TrimPlan::none())
    }

    /// The request's message list under `plan` (see core/chat/trim): `messages` with the older content left out. The
    /// stored messages are not changed. Dropped turns are cut at a user message, so no tool result is separated from its
    /// call, and the first message after the system message is the first kept user message.
    pub fn messages_with(&self, system: &str, resolver: &mut Resolver<'_>, plan: &TrimPlan) -> Vec<Value> {
        let turns: Vec<usize> =
            self.messages.iter().enumerate().filter(|(_, m)| role(m) == Some("user")).map(|(i, _)| i).collect();
        // The last turn is never dropped.
        let drop = plan.drop_turns.min(turns.len().saturating_sub(1));
        let start = if drop == 0 { 0 } else { turns[drop] };
        let last_user = turns.last().copied();
        // The newest `keep` tool results of the kept messages stay; the older ones are replaced.
        let tools: Vec<usize> = (start..self.messages.len()).filter(|&i| role(&self.messages[i]) == Some("tool")).collect();
        let replaced = plan.keep_tool_results.map_or(0, |keep| tools.len().saturating_sub(keep));
        let old_outputs: HashSet<usize> = tools.into_iter().take(replaced).collect();
        let sent = attachments::images_to_send(&self.messages);
        let mut out = Vec::with_capacity(self.messages.len() - start + 1);
        out.push(json!({"role": "system", "content": system}));
        for index in start..self.messages.len() {
            let m = &self.messages[index];
            let mut wire = m.clone();
            if role(m) == Some("user") {
                if let Some(parts) = m.get("content").and_then(Value::as_array) {
                    let parts = if plan.elide_old_docs && Some(index) != last_user {
                        elide_documents(parts)
                    } else {
                        parts.clone()
                    };
                    wire["content"] = attachments::wire_content(&parts, |part| sent.contains(&(index, part)), resolver);
                }
                if drop > 0 && index == start {
                    wire["content"] = with_note(wire["content"].take(), OMITTED_TURNS_NOTE);
                }
            } else if old_outputs.contains(&index) {
                wire["content"] = json!(OLD_OUTPUT_NOTE);
            }
            out.push(wire);
        }
        out
    }

    /// The number of stored user messages, which is the number of turns.
    pub fn turn_count(&self) -> usize {
        self.messages.iter().filter(|m| role(m) == Some("user")).count()
    }

    /// Replaces the content of every tool message except the most recent `keep_recent` with `ELIDED_RESULT`.
    pub fn elide_tool_results(&mut self, keep_recent: usize) {
        let tool_positions: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| role(m) == Some("tool"))
            .map(|(i, _)| i)
            .collect();
        let elide = tool_positions.len().saturating_sub(keep_recent);
        for i in tool_positions.into_iter().take(elide) {
            if let Some(obj) = self.messages.get_mut(i).and_then(Value::as_object_mut) {
                obj.insert("content".to_string(), json!(ELIDED_RESULT));
            }
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Number of stored messages, not counting the system message.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// A copy of the stored messages, without the system message. What the persistence layer writes to disk.
    pub fn snapshot(&self) -> Vec<Value> {
        self.messages.clone()
    }

    /// Rebuilds a history from stored messages (for example a saved conversation), repairing what is broken.
    ///
    /// Kept: user, assistant and tool messages with sane content (a user message needs string content or an array of
    /// known parts, see `repair_user`; a tool message needs a string `tool_call_id` and string content; an assistant
    /// message needs string or null content, and a non-string content on an assistant with tool calls becomes null).
    /// Dropped: non-object entries, other roles, and tool messages that answer no pending call or answer a call already
    /// answered. Calls with no usable id, or repeated, are removed. Every call still unanswered before the next user or
    /// assistant message, or at the end, gets a `tool` message with the content "skipped: not recorded".
    pub fn from_messages(messages: Vec<Value>) -> History {
        let mut out: Vec<Value> = Vec::with_capacity(messages.len());
        let mut pending: Vec<String> = Vec::new();
        let mut answered: HashSet<String> = HashSet::new();
        for m in messages {
            let Some(obj) = m.as_object() else { continue };
            match role(&m) {
                Some("user") => {
                    let Some(repaired) = repair_user(m) else { continue };
                    close_unanswered(&mut out, &mut pending, &mut answered);
                    out.push(repaired);
                }
                Some("assistant") => {
                    let Some(repaired) = repair_assistant(m) else { continue };
                    close_unanswered(&mut out, &mut pending, &mut answered);
                    for id in calls_of(&repaired) {
                        if !pending.contains(&id) {
                            pending.push(id);
                        }
                    }
                    out.push(repaired);
                }
                Some("tool") => {
                    let call_id = obj.get("tool_call_id").and_then(Value::as_str);
                    let content_ok = obj.get("content").is_some_and(Value::is_string);
                    let Some(call_id) = call_id.filter(|_| content_ok) else { continue };
                    if pending.iter().any(|p| p == call_id) && answered.insert(call_id.to_string()) {
                        out.push(m);
                    }
                }
                _ => {}
            }
        }
        close_unanswered(&mut out, &mut pending, &mut answered);
        History { messages: out, echo_reasoning: false }
    }
}

fn role(m: &Value) -> Option<&str> {
    m.get("role").and_then(Value::as_str)
}

/// The parts of a user message with each document replaced by a note that it was left out. The parts keep their order,
/// so the image indexes that `images_to_send` gave still hold.
fn elide_documents(parts: &[Value]) -> Vec<Value> {
    parts
        .iter()
        .map(|part| {
            if part["type"] == "watt_file" {
                let name = attachments::sanitize_name(part["name"].as_str().unwrap_or_default());
                json!({"type": "text", "text": format!("[file {name} omitted from context]")})
            } else {
                part.clone()
            }
        })
        .collect()
}

/// The wire content with `note` before it: a line of its own, as the first part of a parts array.
fn with_note(content: Value, note: &str) -> Value {
    match content {
        Value::String(text) if text.is_empty() => json!(note),
        Value::String(text) => json!(format!("{note}\n\n{text}")),
        Value::Array(mut parts) => {
            parts.insert(0, json!({"type": "text", "text": note}));
            Value::Array(parts)
        }
        other => other,
    }
}

fn strip_reasoning(raw: &mut Value) {
    if let Some(obj) = raw.as_object_mut() {
        for key in REASONING_KEYS {
            obj.remove(key);
        }
    }
}

/// Repairs a user message for `from_messages`. String content is kept as it is. An array keeps its known parts (see
/// `attachments::repair_part`), and the message is dropped only when no part remains. Any other content is dropped.
fn repair_user(mut m: Value) -> Option<Value> {
    let content = m.get("content")?.clone();
    match content {
        Value::String(_) => Some(m),
        Value::Array(parts) => {
            let kept: Vec<Value> = parts.iter().filter_map(attachments::repair_part).collect();
            if kept.is_empty() {
                return None;
            }
            m["content"] = Value::Array(kept);
            Some(m)
        }
        _ => None,
    }
}

/// Repairs an assistant message for `from_messages`: the content shape, and the tool calls (usable, unique ids
/// only; no `tool_calls` key when none are left). `None` when the message cannot be kept.
fn repair_assistant(mut m: Value) -> Option<Value> {
    let obj = m.as_object_mut()?;
    let content_ok = match obj.get("content") {
        None | Some(Value::Null) | Some(Value::String(_)) => true,
        Some(_) => false,
    };
    let has_calls_key = obj.contains_key("tool_calls");
    let mut calls: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(Value::Array(items)) = obj.get("tool_calls") {
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else { continue };
            if seen.insert(id.to_string()) {
                calls.push(item.clone());
            }
        }
    }
    if !content_ok {
        if calls.is_empty() {
            return None;
        }
        obj.insert("content".to_string(), Value::Null);
    }
    if has_calls_key {
        if calls.is_empty() {
            obj.remove("tool_calls");
        } else {
            obj.insert("tool_calls".to_string(), Value::Array(calls));
        }
    }
    Some(m)
}

/// The tool-call ids of a repaired assistant message, in order.
fn calls_of(m: &Value) -> Vec<String> {
    m.get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| calls.iter().filter_map(|c| c.get("id").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Closes every pending call that has no answer yet, then forgets the pending set.
fn close_unanswered(out: &mut Vec<Value>, pending: &mut Vec<String>, answered: &mut HashSet<String>) {
    for id in pending.drain(..) {
        if !answered.contains(&id) {
            out.push(json!({"role": "tool", "tool_call_id": id, "content": "skipped: not recorded"}));
        }
    }
    answered.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> Value {
        json!({"id": id, "type": "function", "function": {"name": "get_cwd", "arguments": "{}"}})
    }

    fn assistant_calls(ids: &[&str]) -> Value {
        json!({"role": "assistant", "content": null, "tool_calls": ids.iter().map(|id| call(id)).collect::<Vec<_>>()})
    }

    /// Checks the pairing invariant over the stored messages: every call has exactly one tool message after it,
    /// before the next user or assistant message, and every tool message answers a call.
    fn assert_paired(h: &History) {
        let msgs = h.messages("", &mut Resolver::none());
        let msgs = &msgs[1..];
        let mut all_calls: Vec<String> = Vec::new();
        for (i, m) in msgs.iter().enumerate() {
            if role(m) != Some("assistant") {
                continue;
            }
            let ids: Vec<String> = m
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|cs| cs.iter().map(|c| c["id"].as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            for id in &ids {
                let answers = msgs[i + 1..]
                    .iter()
                    .take_while(|n| !matches!(role(n), Some("user") | Some("assistant")))
                    .filter(|n| n["tool_call_id"].as_str() == Some(id.as_str()))
                    .count();
                assert_eq!(answers, 1, "call {id} must have exactly one result");
            }
            all_calls.extend(ids);
        }
        for m in msgs.iter().filter(|m| role(m) == Some("tool")) {
            let id = m["tool_call_id"].as_str().unwrap_or("");
            assert!(all_calls.iter().any(|c| c == id), "tool message {id} answers no call");
        }
    }

    #[test]
    fn a_full_turn_with_two_calls_is_paired() {
        let mut h = History::new();
        h.push_user("what dir?");
        h.push_assistant(assistant_calls(&["c1", "c2"]));
        assert!(h.push_tool_result("c1", "a"));
        assert!(h.push_tool_result("c2", "b"));
        h.push_assistant(json!({"role": "assistant", "content": "done"}));
        assert_paired(&h);
        assert!(h.pending_call_ids().is_empty());
        assert_eq!(h.len(), 5);
    }

    #[test]
    fn push_user_closes_pending_calls_with_turn_ended() {
        let mut h = History::new();
        h.push_user("first");
        h.push_assistant(assistant_calls(&["c1"]));
        h.push_user("second");
        assert_paired(&h);
        let closed = &h.messages("", &mut Resolver::none())[3];
        assert_eq!(closed["tool_call_id"], "c1");
        assert_eq!(closed["content"], "skipped: turn ended");
    }

    #[test]
    fn push_assistant_closes_pending_calls_before_the_new_message() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(assistant_calls(&["c1"]));
        h.push_assistant(json!({"role": "assistant", "content": "again"}));
        assert_paired(&h);
        assert_eq!(h.len(), 4);
    }

    #[test]
    fn close_pending_answers_every_pending_call_with_the_reason() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(assistant_calls(&["c1", "c2"]));
        h.push_tool_result("c1", "ok");
        h.close_pending("cancelled");
        assert_paired(&h);
        let last = h.messages("", &mut Resolver::none()).pop().unwrap();
        assert_eq!(last["tool_call_id"], "c2");
        assert_eq!(last["content"], "cancelled");
    }

    #[test]
    fn push_tool_result_rejects_unknown_and_already_answered_ids() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(assistant_calls(&["c1"]));
        assert!(!h.push_tool_result("nope", "x"));
        assert!(h.push_tool_result("c1", "x"));
        assert!(!h.push_tool_result("c1", "again"));
        assert_paired(&h);
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn pending_ids_skip_missing_ids_and_duplicates() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{"type": "function"}, call("c1"), call("c1"), call("c2")],
        }));
        assert_eq!(h.pending_call_ids(), vec!["c1".to_string(), "c2".to_string()]);
        assert!(h.push_tool_result("c1", "x"));
        assert_eq!(h.pending_call_ids(), vec!["c2".to_string()]);
    }

    #[test]
    fn assistant_without_calls_has_nothing_pending() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(json!({"role": "assistant", "content": "plain"}));
        assert!(h.pending_call_ids().is_empty());
        assert!(!h.push_tool_result("c1", "x"));
    }

    #[test]
    fn messages_put_the_system_message_first_and_only_once() {
        let mut h = History::new();
        h.push_user("hi");
        let msgs = h.messages("be brief", &mut Resolver::none());
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], json!({"role": "system", "content": "be brief"}));
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn reasoning_fields_are_stripped_by_default() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(json!({
            "role": "assistant",
            "content": "x",
            "reasoning_content": "thinking",
            "reasoning": "thinking",
            "reasoning_details": [{"type": "text"}],
        }));
        let stored = &h.messages("", &mut Resolver::none())[2];
        assert_eq!(stored, &json!({"role": "assistant", "content": "x"}));
    }

    #[test]
    fn reasoning_fields_are_kept_when_echo_is_set() {
        let mut h = History::new();
        h.echo_reasoning = true;
        let raw = json!({"role": "assistant", "content": "x", "reasoning_content": "thinking"});
        h.push_assistant(raw.clone());
        assert_eq!(h.messages("", &mut Resolver::none())[1], raw);
    }

    #[test]
    fn elide_keeps_the_most_recent_tool_results() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(assistant_calls(&["a", "b", "c"]));
        h.push_tool_result("a", "first");
        h.push_tool_result("b", "second");
        h.push_tool_result("c", "third");
        h.elide_tool_results(1);
        let msgs = h.messages("", &mut Resolver::none());
        assert_eq!(msgs[3]["content"], ELIDED_RESULT);
        assert_eq!(msgs[4]["content"], ELIDED_RESULT);
        assert_eq!(msgs[5]["content"], "third");
        assert_paired(&h);
    }

    #[test]
    fn elide_with_fewer_results_than_kept_changes_nothing() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(assistant_calls(&["a"]));
        h.push_tool_result("a", "only");
        h.elide_tool_results(5);
        assert_eq!(h.messages("", &mut Resolver::none())[3]["content"], "only");
    }

    #[test]
    fn clear_and_len_and_is_empty() {
        let mut h = History::new();
        assert!(h.is_empty());
        h.push_user("hi");
        assert_eq!(h.len(), 1);
        assert!(!h.is_empty());
        h.clear();
        assert!(h.is_empty());
        assert_eq!(h.messages("s", &mut Resolver::none()).len(), 1);
    }

    #[test]
    fn snapshot_has_the_stored_messages_without_the_system_message() {
        let mut h = History::new();
        h.push_user("hi");
        h.push_assistant(assistant_calls(&["c1"]));
        h.push_tool_result("c1", "ok");
        let snap = h.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0]["role"], "user");
        assert_eq!(snap, h.messages("", &mut Resolver::none())[1..].to_vec());
    }

    #[test]
    fn from_messages_round_trips_a_valid_history() {
        let mut h = History::new();
        h.push_user("what dir?");
        h.push_assistant(assistant_calls(&["c1", "c2"]));
        h.push_tool_result("c1", "a");
        h.push_tool_result("c2", "b");
        h.push_assistant(json!({"role": "assistant", "content": "done"}));
        let restored = History::from_messages(h.snapshot());
        assert_eq!(restored.snapshot(), h.snapshot());
        assert_paired(&restored);
        assert!(!restored.echo_reasoning);
    }

    #[test]
    fn from_messages_drops_entries_that_are_not_messages() {
        let restored = History::from_messages(vec![
            json!("just a string"),
            json!(42),
            Value::Null,
            json!({"role": "system", "content": "injected"}),
            json!({"role": "developer", "content": "nope"}),
            json!({"content": "no role"}),
            json!({"role": "user", "content": "kept"}),
        ]);
        assert_eq!(restored.snapshot(), vec![json!({"role": "user", "content": "kept"})]);
    }

    #[test]
    fn from_messages_drops_user_messages_without_string_content() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": ["parts"]}),
            json!({"role": "user"}),
            json!({"role": "user", "content": 7}),
            json!({"role": "user", "content": "kept"}),
        ]);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.snapshot()[0]["content"], "kept");
    }

    #[test]
    fn from_messages_drops_assistant_messages_with_bad_content_and_no_calls() {
        let restored = History::from_messages(vec![
            json!({"role": "assistant", "content": 3}),
            json!({"role": "assistant", "content": {"text": "x"}}),
            json!({"role": "assistant", "content": "kept"}),
        ]);
        assert_eq!(restored.snapshot(), vec![json!({"role": "assistant", "content": "kept"})]);
    }

    #[test]
    fn from_messages_nulls_bad_content_on_an_assistant_with_calls() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": ["odd"], "tool_calls": [call("c1")]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
        ]);
        let snap = restored.snapshot();
        assert_eq!(snap[1]["content"], Value::Null);
        assert_eq!(snap[1]["tool_calls"][0]["id"], "c1");
        assert_paired(&restored);
    }

    #[test]
    fn from_messages_closes_unanswered_calls_before_the_next_user_message() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": "first"}),
            assistant_calls(&["a1", "a2"]),
            json!({"role": "tool", "tool_call_id": "a2", "content": "two"}),
            json!({"role": "user", "content": "second"}),
        ]);
        assert_paired(&restored);
        let snap = restored.snapshot();
        assert_eq!(snap.len(), 5);
        assert_eq!(snap[2], json!({"role": "tool", "tool_call_id": "a2", "content": "two"}));
        assert_eq!(snap[3], json!({"role": "tool", "tool_call_id": "a1", "content": "skipped: not recorded"}));
        assert_eq!(snap[4]["content"], "second");
    }

    #[test]
    fn from_messages_closes_calls_left_open_at_the_end() {
        let restored = History::from_messages(vec![json!({"role": "user", "content": "hi"}), assistant_calls(&["c1"])]);
        assert_paired(&restored);
        let last = restored.snapshot().pop().unwrap();
        assert_eq!(last, json!({"role": "tool", "tool_call_id": "c1", "content": "skipped: not recorded"}));
        assert!(restored.pending_call_ids().is_empty());
    }

    #[test]
    fn from_messages_drops_orphan_duplicate_and_unknown_tool_messages() {
        let restored = History::from_messages(vec![
            json!({"role": "tool", "tool_call_id": "early", "content": "before any call"}),
            json!({"role": "user", "content": "hi"}),
            assistant_calls(&["c1"]),
            json!({"role": "tool", "tool_call_id": "c1", "content": "first"}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "second"}),
            json!({"role": "tool", "tool_call_id": "nope", "content": "unknown"}),
            json!({"role": "tool", "tool_call_id": "c1"}),
            json!({"role": "tool", "content": "no id"}),
            json!({"role": "tool", "tool_call_id": 5, "content": "bad id"}),
        ]);
        assert_paired(&restored);
        let snap = restored.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[2]["content"], "first");
    }

    #[test]
    fn from_messages_drops_a_tool_message_that_came_after_a_user_message() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": "hi"}),
            assistant_calls(&["c1"]),
            json!({"role": "user", "content": "again"}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "too late"}),
        ]);
        assert_paired(&restored);
        let snap = restored.snapshot();
        assert_eq!(snap.len(), 4);
        assert_eq!(snap[2]["content"], "skipped: not recorded");
        assert_eq!(snap[3]["content"], "again");
    }

    #[test]
    fn from_messages_repairs_the_calls_of_an_assistant_message() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": "hi"}),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{"type": "function"}, call("c1"), call("c1"), call(""), "text", call("c2")],
            }),
            json!({"role": "tool", "tool_call_id": "c1", "content": "x"}),
            json!({"role": "tool", "tool_call_id": "c2", "content": "y"}),
        ]);
        assert_paired(&restored);
        let snap = restored.snapshot();
        assert_eq!(snap.len(), 4);
        let calls = snap[1]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["id"], "c1");
        assert_eq!(calls[1]["id"], "c2");
    }

    #[test]
    fn from_messages_removes_an_empty_or_non_array_tool_calls_key() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": "a", "tool_calls": []}),
            json!({"role": "assistant", "content": "b", "tool_calls": "oops"}),
            json!({"role": "assistant", "content": "c", "tool_calls": [{"type": "function"}]}),
        ]);
        assert_paired(&restored);
        for m in restored.snapshot().iter().skip(1) {
            assert!(m.get("tool_calls").is_none(), "{m}");
        }
        assert_eq!(restored.len(), 4);
    }

    #[test]
    fn from_messages_checks_pairing_for_every_assistant_message() {
        // A mix of broken pieces, several turns long. Whatever survives must satisfy the invariant everywhere.
        let restored = History::from_messages(vec![
            json!({"role": "tool", "tool_call_id": "x", "content": "orphan"}),
            json!({"role": "user", "content": "one"}),
            assistant_calls(&["a", "b", "b"]),
            json!({"role": "tool", "tool_call_id": "b", "content": "b1"}),
            json!({"role": "tool", "tool_call_id": "b", "content": "b2"}),
            json!({"role": "assistant", "content": 9}),
            assistant_calls(&["c"]),
            json!(null),
            json!({"role": "assistant", "content": null, "tool_calls": [call("d")]}),
            json!({"role": "user", "content": "two"}),
            assistant_calls(&["e", "f"]),
            json!({"role": "tool", "tool_call_id": "f", "content": "f1"}),
        ]);
        assert_paired(&restored);
        let snap = restored.snapshot();
        assert!(snap.iter().all(|m| m["role"] != "system"));
        assert!(snap.iter().filter(|m| m["role"] == "assistant").count() >= 4);
    }

    // Attachments: stored parts, and what is sent for them

    use crate::core::persist::attachments::{AttachmentStore, sha256_hex};
    use crate::core::settings::test_support::TempDir;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    /// A valid conversation id, for the attachment files the tests write.
    const CONV: &str = "ses_00000000000000000000000000000001";

    /// Bytes that start like a PNG. Nothing here decodes them, so only the content address matters.
    fn png_bytes(seed: u8) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[seed; 24]);
        bytes
    }

    /// Writes an image file for conversation `id` and returns its content address.
    fn put_image(store: &AttachmentStore, id: &str, bytes: &[u8]) -> String {
        let sha = sha256_hex(bytes);
        store.write(id, &sha, "png", bytes).unwrap();
        sha
    }

    fn image_part(name: &str, sha: &str, bytes: u64) -> Value {
        json!({"type": "watt_image", "sha256": sha, "mime": "image/png", "name": name, "bytes": bytes, "w": 1, "h": 1})
    }

    fn file_part(name: &str, text: &str) -> Value {
        json!({"type": "watt_file", "name": name, "bytes": text.len(), "text": text})
    }

    fn text_part(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn data_url(bytes: &[u8]) -> String {
        format!("data:image/png;base64,{}", STANDARD.encode(bytes))
    }

    #[test]
    fn a_plain_user_message_is_sent_exactly_as_stored() {
        let mut h = History::new();
        h.push_user("just text");
        let sent = h.messages("sys", &mut Resolver::none());
        assert_eq!(sent[1], json!({"role": "user", "content": "just text"}));
    }

    #[test]
    fn text_and_a_document_are_sent_as_one_string() {
        let mut h = History::new();
        h.push_user_content(json!([text_part("summarise"), file_part("notes.txt", "hello")]));
        let sent = h.messages("", &mut Resolver::none());
        assert_eq!(
            sent[1]["content"],
            "summarise\n\n<attached_file name=\"notes.txt\">\nhello\n</attached_file>"
        );
    }

    #[test]
    fn a_sent_image_makes_the_content_an_array_with_an_image_url_part() {
        let tmp = TempDir::new();
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        let bytes = png_bytes(1);
        let sha = put_image(&store, CONV, &bytes);
        let mut h = History::new();
        h.push_user_content(json!([text_part("look"), image_part("shot.png", &sha, bytes.len() as u64)]));

        let sent = h.messages("", &mut Resolver::new(Some(&store), CONV));
        assert_eq!(
            sent[1]["content"],
            json!([
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": data_url(&bytes)}},
            ])
        );
        assert_eq!(h.snapshot()[0]["content"][1]["type"], "watt_image", "the history keeps the stored part");
    }

    #[test]
    fn parts_keep_their_order_when_an_image_and_a_document_are_both_sent() {
        let tmp = TempDir::new();
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        let bytes = png_bytes(2);
        let sha = put_image(&store, CONV, &bytes);
        let mut h = History::new();
        h.push_user_content(json!([
            text_part("compare"),
            file_part("a.txt", "first"),
            image_part("b.png", &sha, bytes.len() as u64),
        ]));
        let sent = h.messages("", &mut Resolver::new(Some(&store), CONV));
        assert_eq!(
            sent[1]["content"],
            json!([
                {"type": "text", "text": "compare"},
                {"type": "text", "text": "<attached_file name=\"a.txt\">\nfirst\n</attached_file>"},
                {"type": "image_url", "image_url": {"url": data_url(&bytes)}},
            ])
        );
    }

    #[test]
    fn an_image_whose_file_is_gone_is_a_note_and_the_content_stays_a_string() {
        let tmp = TempDir::new();
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        let sha = sha256_hex(b"never written");
        let mut h = History::new();
        h.push_user_content(json!([text_part("look"), image_part("shot.png", &sha, 9)]));
        let sent = h.messages("", &mut Resolver::new(Some(&store), CONV));
        assert_eq!(sent[1]["content"], "look\n\n[image shot.png is no longer available]");
        let none = h.messages("", &mut Resolver::none());
        assert_eq!(none[1]["content"], "look\n\n[image shot.png is no longer available]");
    }

    #[test]
    fn a_file_that_does_not_match_its_content_address_is_treated_as_missing() {
        let tmp = TempDir::new();
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        let bytes = png_bytes(3);
        let sha = sha256_hex(&bytes);
        store.write(CONV, &sha, "png", b"tampered").unwrap();
        let mut h = History::new();
        h.push_user_content(json!([text_part("x"), image_part("t.png", &sha, 9)]));
        let sent = h.messages("", &mut Resolver::new(Some(&store), CONV));
        assert_eq!(sent[1]["content"], "x\n\n[image t.png is no longer available]");
    }

    #[test]
    fn only_the_last_three_messages_with_images_send_their_images() {
        let tmp = TempDir::new();
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        let mut h = History::new();
        let mut shas = Vec::new();
        for (n, name) in ["a.png", "b.png", "c.png", "d.png"].iter().enumerate() {
            let bytes = png_bytes(10 + n as u8);
            let sha = put_image(&store, CONV, &bytes);
            h.push_user_content(json!([text_part(&format!("message {n}")), image_part(name, &sha, bytes.len() as u64)]));
            h.push_assistant(json!({"role": "assistant", "content": format!("reply {n}")}));
            shas.push(sha);
        }
        h.push_user("and now a plain question");

        let sent = h.messages("", &mut Resolver::new(Some(&store), CONV));
        // The first image message is older than the last three, so its image is a note in the request.
        assert_eq!(sent[1]["content"], "message 0\n\n[image a.png omitted from context]");
        // The other three send their images.
        for (wire_index, name) in [(3usize, "b.png"), (5, "c.png"), (7, "d.png")] {
            let content = sent[wire_index]["content"].as_array().unwrap_or_else(|| panic!("{name} is an array"));
            assert_eq!(content[1]["type"], "image_url", "{name}");
        }
        assert_eq!(sent[9], json!({"role": "user", "content": "and now a plain question"}));
        // The history itself is unchanged: the first image is still stored.
        assert_eq!(h.snapshot()[0]["content"][1]["sha256"], shas[0].as_str());
    }

    #[test]
    fn at_most_eight_images_are_sent_newest_first_within_the_window() {
        let mut h = History::new();
        let parts: Vec<Value> = (0u8..9).map(|n| image_part(&format!("{n}.png"), &sha256_hex(&[n]), 1)).collect();
        h.push_user_content(Value::Array(parts));
        let sent = attachments::images_to_send(&h.snapshot());
        assert_eq!(sent.len(), 8);
        assert!(!sent.contains(&(0, 8)), "the ninth image is not sent");
        assert!(sent.contains(&(0, 0)));

        let notes = h.messages("", &mut Resolver::none());
        let text = notes[1]["content"].as_str().unwrap();
        assert_eq!(text.matches("[image 8.png omitted from context]").count(), 1, "{text}");
        assert_eq!(text.matches("is no longer available").count(), 8, "{text}");
    }

    #[test]
    fn the_byte_budget_stops_sending_before_the_count_cap() {
        let mb4 = 4 * 1024 * 1024;
        let mut h = History::new();
        let parts: Vec<Value> = (0u8..3).map(|n| image_part(&format!("{n}.png"), &sha256_hex(&[n]), mb4)).collect();
        h.push_user_content(Value::Array(parts));
        let sent = attachments::images_to_send(&h.snapshot());
        let expected: HashSet<(usize, usize)> = [(0, 0), (0, 1)].into_iter().collect();
        assert_eq!(sent, expected, "8 MB fit in 10 MB, the third image would make 12 MB");
    }

    #[test]
    fn from_messages_keeps_the_known_parts_and_drops_the_rest() {
        let sha = sha256_hex(b"kept");
        let restored = History::from_messages(vec![json!({
            "role": "user",
            "content": [
                {"type": "video", "src": "x"},
                text_part("hi"),
                image_part("a.png", &sha, 4),
                file_part("n.txt", "doc"),
                ["nested"],
            ],
        })]);
        let snap = restored.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0]["content"], json!([text_part("hi"), image_part("a.png", &sha, 4), file_part("n.txt", "doc")]));
    }

    #[test]
    fn from_messages_drops_a_user_message_only_when_no_part_remains() {
        let restored = History::from_messages(vec![
            json!({"role": "user", "content": [{"type": "video"}]}),
            json!({"role": "user", "content": []}),
            json!({"role": "user", "content": [{"type": "watt_file", "name": "no text"}]}),
            json!({"role": "user", "content": [text_part("")]}),
        ]);
        assert_eq!(restored.snapshot(), vec![json!({"role": "user", "content": [text_part("")]})]);
    }

    #[test]
    fn from_messages_removes_a_thumbnail_that_is_not_a_png_or_jpeg_data_url() {
        let sha = sha256_hex(b"thumb");
        let mut bad = image_part("a.png", &sha, 5);
        bad["thumb"] = json!("https://example.test/tracker.png");
        let mut good = image_part("b.png", &sha, 5);
        good["thumb"] = json!("data:image/jpeg;base64,/9j/");
        let restored = History::from_messages(vec![json!({"role": "user", "content": [bad, good]})]);
        let parts = restored.snapshot()[0]["content"].clone();
        assert!(parts[0].get("thumb").is_none(), "{parts}");
        assert_eq!(parts[1]["thumb"], "data:image/jpeg;base64,/9j/");
    }

    #[test]
    fn a_message_with_attachments_survives_a_save_and_a_restore() {
        let sha = sha256_hex(b"saved");
        let mut h = History::new();
        h.push_user_content(json!([text_part("look"), image_part("a.png", &sha, 5), file_part("n.txt", "doc")]));
        let restored = History::from_messages(h.snapshot());
        assert_eq!(restored.snapshot(), h.snapshot());
    }

    // Trimming the wire copy (core/chat/trim)

    /// One turn: the user's text, one assistant message with a call per id, a result for each, then a plain answer.
    fn tool_turn(h: &mut History, text: &str, ids: &[&str]) {
        h.push_user(text);
        h.push_assistant(assistant_calls(ids));
        for id in ids {
            h.push_tool_result(id, &format!("result of {id}"));
        }
        h.push_assistant(json!({"role": "assistant", "content": format!("answer to {text}")}));
    }

    fn wire(h: &History, plan: &TrimPlan) -> Vec<Value> {
        h.messages_with("sys", &mut Resolver::none(), plan)
    }

    /// The content of each tool message in wire messages, in order.
    fn tool_contents(messages: &[Value]) -> Vec<String> {
        messages.iter().filter(|m| role(m) == Some("tool")).map(|m| m["content"].as_str().unwrap_or_default().to_string()).collect()
    }

    /// The pairing invariant on wire messages whose first message is the system one: the first message after it is a
    /// user message, every call has exactly one result before the next user or assistant message, and no result answers
    /// nothing.
    fn assert_wire_paired(messages: &[Value]) {
        assert_eq!(messages[0]["role"], "system");
        if messages.len() > 1 {
            assert_eq!(messages[1]["role"], "user", "{messages:?}");
        }
        let msgs = &messages[1..];
        let mut all_calls: Vec<String> = Vec::new();
        for (i, m) in msgs.iter().enumerate() {
            if role(m) != Some("assistant") {
                continue;
            }
            for call in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let id = call["id"].as_str().unwrap_or_default().to_string();
                let answers = msgs[i + 1..]
                    .iter()
                    .take_while(|n| !matches!(role(n), Some("user") | Some("assistant")))
                    .filter(|n| n["tool_call_id"].as_str() == Some(id.as_str()))
                    .count();
                assert_eq!(answers, 1, "call {id} must have exactly one result: {messages:?}");
                all_calls.push(id);
            }
        }
        for m in msgs.iter().filter(|m| role(m) == Some("tool")) {
            let id = m["tool_call_id"].as_str().unwrap_or_default();
            assert!(all_calls.iter().any(|c| c == id), "tool message {id} answers no call: {messages:?}");
        }
    }

    /// Histories with the shapes that trimming must respect: parallel calls, a turn with no answer, calls closed by the
    /// turn ending or by a reason, and documents in several turns.
    fn shaped_histories() -> Vec<History> {
        let mut out = Vec::new();
        let mut a = History::new();
        tool_turn(&mut a, "first", &["c1"]);
        tool_turn(&mut a, "second", &["c2", "c3"]);
        tool_turn(&mut a, "third", &["c4"]);
        out.push(a);

        let mut b = History::new();
        b.push_user("alone");
        b.push_user("then this");
        b.push_assistant(assistant_calls(&["d1", "d2"]));
        b.push_tool_result("d1", "one");
        b.push_user("next");
        b.push_assistant(assistant_calls(&["d3"]));
        b.close_pending("cancelled");
        out.push(b);

        let mut c = History::new();
        c.push_user_content(json!([text_part("read this"), file_part("a.txt", "AAA")]));
        c.push_assistant(json!({"role": "assistant", "content": "ok"}));
        c.push_user_content(json!([text_part("and this"), file_part("b.txt", "BBB")]));
        tool_turn(&mut c, "third", &["e1"]);
        out.push(c);
        out
    }

    #[test]
    fn the_turn_count_is_the_number_of_user_messages() {
        let mut h = History::new();
        assert_eq!(h.turn_count(), 0);
        tool_turn(&mut h, "a", &["c1"]);
        tool_turn(&mut h, "b", &["c2"]);
        assert_eq!(h.turn_count(), 2);
    }

    #[test]
    fn the_untrimmed_plan_is_the_request_that_messages_builds() {
        for h in shaped_histories() {
            assert_eq!(wire(&h, &TrimPlan::none()), h.messages("sys", &mut Resolver::none()));
        }
    }

    #[test]
    fn keeping_the_newest_tool_results_replaces_the_older_ones_across_turns() {
        let mut h = History::new();
        tool_turn(&mut h, "first", &["c1", "c2"]);
        tool_turn(&mut h, "second", &["c3"]);
        tool_turn(&mut h, "third", &["c4"]);
        let keep = |n| TrimPlan { keep_tool_results: Some(n), ..TrimPlan::none() };
        assert_eq!(
            tool_contents(&wire(&h, &keep(2))),
            [OLD_OUTPUT_NOTE, OLD_OUTPUT_NOTE, "result of c3", "result of c4"]
        );
        assert_eq!(tool_contents(&wire(&h, &keep(8))), ["result of c1", "result of c2", "result of c3", "result of c4"]);
        assert_eq!(tool_contents(&wire(&h, &keep(0))), [OLD_OUTPUT_NOTE; 4]);
        assert_wire_paired(&wire(&h, &keep(2)));
        // The stored results are not changed.
        assert_eq!(tool_contents(&h.messages("sys", &mut Resolver::none())), ["result of c1", "result of c2", "result of c3", "result of c4"]);
    }

    #[test]
    fn eliding_documents_keeps_the_document_of_the_last_user_message() {
        let mut h = History::new();
        h.push_user_content(json!([text_part("one"), file_part("a.txt", "AAA")]));
        h.push_assistant(json!({"role": "assistant", "content": "ok"}));
        h.push_user_content(json!([text_part("two"), file_part("b.txt", "BBB")]));
        let plan = TrimPlan { elide_old_docs: true, ..TrimPlan::none() };
        let sent = wire(&h, &plan);
        assert_eq!(sent[1]["content"], "one\n\n[file a.txt omitted from context]");
        assert_eq!(sent[3]["content"], "two\n\n<attached_file name=\"b.txt\">\nBBB\n</attached_file>");
        assert_wire_paired(&sent);
        assert_eq!(h.snapshot()[0]["content"][1]["type"], "watt_file", "the stored document is unchanged");
    }

    #[test]
    fn dropping_turns_keeps_whole_turns_and_marks_the_first_kept_one() {
        let mut h = History::new();
        tool_turn(&mut h, "first", &["c1"]);
        tool_turn(&mut h, "second", &["c2"]);
        tool_turn(&mut h, "third", &["c3"]);
        let drop = |n| TrimPlan { drop_turns: n, ..TrimPlan::none() };

        let one = wire(&h, &drop(1));
        assert_eq!(one.len(), 1 + 8, "the system message and the last two turns, four messages each");
        assert_eq!(one[1]["content"], format!("{OMITTED_TURNS_NOTE}\n\nsecond"));
        assert!(!tool_contents(&one).iter().any(|c| c == "result of c1"), "the first turn is gone");
        assert_wire_paired(&one);

        // Asking for more than the turns that can go keeps the last turn only.
        let last = wire(&h, &drop(99));
        assert_eq!(last.len(), 1 + 4);
        assert_eq!(last[1]["content"], format!("{OMITTED_TURNS_NOTE}\n\nthird"));
        assert_wire_paired(&last);

        assert_eq!(wire(&h, &drop(0)), h.messages("sys", &mut Resolver::none()));
    }

    #[test]
    fn every_rung_keeps_the_pairing_and_starts_with_a_user_message() {
        for h in shaped_histories() {
            for keep in [None, Some(0), Some(1), Some(2), Some(8)] {
                for elide_old_docs in [false, true] {
                    for drop_turns in 0..=h.turn_count() {
                        let plan = TrimPlan { keep_tool_results: keep, elide_old_docs, drop_turns };
                        assert_wire_paired(&wire(&h, &plan));
                    }
                }
            }
        }
    }

    #[test]
    fn a_plan_never_changes_the_stored_history() {
        for h in shaped_histories() {
            let before = h.snapshot();
            for drop_turns in 0..=h.turn_count() {
                let _ = wire(&h, &TrimPlan { keep_tool_results: Some(1), elide_old_docs: true, drop_turns });
            }
            assert_eq!(h.snapshot(), before);
        }
    }
}
