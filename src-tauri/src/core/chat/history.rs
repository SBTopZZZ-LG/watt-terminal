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
}
