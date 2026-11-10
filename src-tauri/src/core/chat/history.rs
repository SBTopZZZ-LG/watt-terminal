//! The conversation as the model sees it: raw chat-completions messages, with tool-call pairing held.
//!
//! Every tool call an assistant message makes gets exactly one `tool` message before the next user or assistant
//! message. A call left without a result is closed with a short reason, never dropped, so a provider never rejects
//! a request because of an orphan call. The system message is not stored; `messages` puts it in front.

use std::collections::HashSet;

use serde_json::{Value, json};

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
        self.close_pending("skipped: turn ended");
        self.messages.push(json!({"role": "user", "content": text}));
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

    /// The request's message list: the system message first, then the stored messages.
    pub fn messages(&self, system: &str) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.messages.len() + 1);
        out.push(json!({"role": "system", "content": system}));
        out.extend(self.messages.iter().cloned());
        out
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
    /// Kept: user, assistant and tool messages with sane content (a user message needs string content; a tool
    /// message needs a string `tool_call_id` and string content; an assistant message needs string or null content,
    /// and a non-string content on an assistant with tool calls becomes null). Dropped: non-object entries, other
    /// roles, and tool messages that answer no pending call or answer a call already answered. Calls with no
    /// usable id, or repeated, are removed. Every call still unanswered before the next user or assistant message,
    /// or at the end, gets a `tool` message with the content "skipped: not recorded".
    pub fn from_messages(messages: Vec<Value>) -> History {
        let mut out: Vec<Value> = Vec::with_capacity(messages.len());
        let mut pending: Vec<String> = Vec::new();
        let mut answered: HashSet<String> = HashSet::new();
        for m in messages {
            let Some(obj) = m.as_object() else { continue };
            match role(&m) {
                Some("user") => {
                    if !obj.get("content").is_some_and(Value::is_string) {
                        continue;
                    }
                    close_unanswered(&mut out, &mut pending, &mut answered);
                    out.push(m);
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

fn strip_reasoning(raw: &mut Value) {
    if let Some(obj) = raw.as_object_mut() {
        for key in REASONING_KEYS {
            obj.remove(key);
        }
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
        let msgs = h.messages("");
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
        let closed = &h.messages("")[3];
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
        let last = h.messages("").pop().unwrap();
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
        let msgs = h.messages("be brief");
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
        let stored = &h.messages("")[2];
        assert_eq!(stored, &json!({"role": "assistant", "content": "x"}));
    }

    #[test]
    fn reasoning_fields_are_kept_when_echo_is_set() {
        let mut h = History::new();
        h.echo_reasoning = true;
        let raw = json!({"role": "assistant", "content": "x", "reasoning_content": "thinking"});
        h.push_assistant(raw.clone());
        assert_eq!(h.messages("")[1], raw);
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
        let msgs = h.messages("");
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
        assert_eq!(h.messages("")[3]["content"], "only");
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
        assert_eq!(h.messages("s").len(), 1);
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
        assert_eq!(snap, h.messages("")[1..].to_vec());
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
}
