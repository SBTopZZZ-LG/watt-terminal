//! Builds an `AssistantReply` from chat-completion chunks (stream) or from one completion object (non-stream).
//!
//! Tool calls are merged by `index`: the id and name come from the first delta that has them, arguments are
//! concatenated, and empty final arguments become "{}". Tool calls exist iff the accumulated set is
//! non-empty; `finish_reason` is recorded but not trusted for that decision.
//!
//! Reasoning text is read from `reasoning_content` or `reasoning` (plain strings, whichever is non-empty).
//! `reasoning_details` is NOT reconstructed: providers send it as per-delta fragments, and v1 does not echo
//! reasoning back to the model (see the POC results). The raw message for a stream is rebuilt here, not
//! copied from the provider.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{provider_error, AssistantReply, Delta, LlmError, ToolCall, Usage};

#[derive(Default)]
pub struct Accumulator {
    text: String,
    reasoning: String,
    calls: BTreeMap<u64, PartialCall>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges one parsed stream chunk. Text and reasoning deltas are appended to `out` as they arrive.
    pub fn push_chunk(&mut self, chunk: &Value, out: &mut Vec<Delta>) -> Result<(), LlmError> {
        if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
            return Err(provider_error(err));
        }
        // Guarded: the usage-only final chunk may have an empty `choices` array.
        if let Some(choice) = first_choice(chunk) {
            if let Some(delta) = choice.get("delta") {
                if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    self.text.push_str(text);
                    out.push(Delta::Text(text.to_string()));
                }
                if let Some(reasoning) = reasoning_piece(delta) {
                    self.reasoning.push_str(reasoning);
                    out.push(Delta::Reasoning(reasoning.to_string()));
                }
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for (pos, tc) in calls.iter().enumerate() {
                        let index = tc.get("index").and_then(Value::as_u64).unwrap_or(pos as u64);
                        let call = self.calls.entry(index).or_default();
                        if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|_| call.id.is_empty()) {
                            call.id = id.to_string();
                        }
                        let function = tc.get("function");
                        if let Some(name) = function.and_then(|f| f.get("name")).and_then(Value::as_str)
                            && call.name.is_empty()
                        {
                            call.name = name.to_string();
                        }
                        if let Some(args) = function.and_then(|f| f.get("arguments")).and_then(Value::as_str) {
                            call.arguments.push_str(args);
                        }
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_string());
            }
        }
        if let Some(usage) = usage_of(chunk) {
            self.usage = Some(usage);
        }
        Ok(())
    }

    /// True once a chunk has carried a `finish_reason`. The stream client uses it to tell a stream that
    /// ended cleanly without `[DONE]` from one that was cut off.
    pub fn has_finish_reason(&self) -> bool {
        self.finish_reason.is_some()
    }

    /// Ends the stream and rebuilds the assistant message as it should be echoed back in history.
    pub fn finish(self) -> Result<AssistantReply, LlmError> {
        let mut tool_calls = Vec::with_capacity(self.calls.len());
        for (index, call) in self.calls {
            tool_calls.push(checked_call(index, call.id, call.name, call.arguments)?);
        }
        let mut raw = json!({
            "role": "assistant",
            "content": if self.text.is_empty() { Value::Null } else { Value::String(self.text.clone()) },
        });
        if !tool_calls.is_empty() {
            raw["tool_calls"] = Value::Array(tool_calls.iter().map(call_json).collect());
        }
        if !self.reasoning.is_empty() {
            raw["reasoning_content"] = Value::String(self.reasoning.clone());
        }
        Ok(AssistantReply {
            raw,
            text: self.text,
            reasoning: self.reasoning,
            tool_calls,
            finish_reason: self.finish_reason,
            usage: self.usage,
        })
    }
}

/// Reads a complete (non-stream) chat-completion object. The caller has already handled HTTP and error objects.
pub fn from_completion(body: &Value) -> Result<AssistantReply, LlmError> {
    let choice = first_choice(body).ok_or_else(|| LlmError::Parse("response has no choices".to_string()))?;
    let message = choice
        .get("message")
        .filter(|m| m.is_object())
        .ok_or_else(|| LlmError::Parse("choice has no message object".to_string()))?;
    let text = message.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let reasoning = reasoning_piece(message).unwrap_or("").to_string();
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (pos, tc) in calls.iter().enumerate() {
            let function = tc.get("function");
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let name = function.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("").to_string();
            let args = arguments_text(function.and_then(|f| f.get("arguments")));
            tool_calls.push(checked_call(pos as u64, id, name, args)?);
        }
    }
    Ok(AssistantReply {
        raw: message.clone(),
        text,
        reasoning,
        tool_calls,
        finish_reason: choice.get("finish_reason").and_then(Value::as_str).map(str::to_string),
        usage: usage_of(body),
    })
}

fn first_choice(body: &Value) -> Option<&Value> {
    body.get("choices").and_then(Value::as_array).and_then(|choices| choices.first())
}

fn usage_of(body: &Value) -> Option<Usage> {
    let usage = body.get("usage").filter(|u| u.is_object())?;
    Some(Usage {
        prompt_tokens: usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// First non-empty of `reasoning_content` and `reasoning`. Only plain strings count.
fn reasoning_piece(message: &Value) -> Option<&str> {
    ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|key| message.get(*key).and_then(Value::as_str).filter(|s| !s.is_empty()))
}

fn arguments_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Empty arguments mean a call with no parameters, which must still be valid JSON for history.
fn checked_call(index: u64, id: String, name: String, arguments: String) -> Result<ToolCall, LlmError> {
    if id.is_empty() {
        return Err(LlmError::Parse(format!("tool call at index {index} has no id")));
    }
    if name.is_empty() {
        return Err(LlmError::Parse(format!("tool call {id} has no name")));
    }
    let arguments = if arguments.trim().is_empty() { "{}".to_string() } else { arguments };
    Ok(ToolCall { id, name, arguments })
}

fn call_json(call: &ToolCall) -> Value {
    json!({
        "id": call.id,
        "type": "function",
        "function": { "name": call.name, "arguments": call.arguments },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm::MAX_SSE_LINE_BYTES;
    use crate::core::llm::sse::SseParser;

    /// Runs a recorded SSE body through the framer and the accumulator, the same way the client does.
    fn accumulate_sse(raw: &str) -> (AssistantReply, Vec<Delta>) {
        let mut parser = SseParser::new(MAX_SSE_LINE_BYTES);
        let mut payloads = parser.push(raw.as_bytes()).expect("fixture is within the cap");
        payloads.extend(parser.finish().expect("fixture is within the cap"));
        let mut acc = Accumulator::new();
        let mut deltas = Vec::new();
        for payload in payloads {
            if payload == "[DONE]" {
                break;
            }
            let chunk: Value = serde_json::from_str(&payload).expect("fixture chunk is JSON");
            acc.push_chunk(&chunk, &mut deltas).expect("fixture chunk has no error");
        }
        (acc.finish().expect("fixture accumulates"), deltas)
    }

    fn chunk(json_text: &str) -> Value {
        serde_json::from_str(json_text).expect("test chunk is JSON")
    }

    #[test]
    fn opencode_go_stream_fixture_gives_one_tool_call_and_usage() {
        let raw = include_str!("../../../tests/fixtures/llm/opencodego_stream.sse");
        let (reply, deltas) = accumulate_sse(raw);
        assert!(deltas.is_empty(), "this fixture has only empty content and reasoning deltas");
        assert_eq!(reply.text, "");
        assert_eq!(reply.reasoning, "");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "call_00_16a4w1l8hhiztd5311fg325f");
        assert_eq!(reply.tool_calls[0].name, "get_cwd");
        assert_eq!(reply.tool_calls[0].arguments, "{}");
        assert_eq!(reply.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(reply.usage, Some(Usage { prompt_tokens: 334, completion_tokens: 23 }));
        assert_eq!(reply.raw["role"], "assistant");
        assert_eq!(reply.raw["content"], Value::Null);
        assert_eq!(reply.raw["tool_calls"][0]["function"]["arguments"], "{}");
        assert!(reply.raw.get("reasoning_content").is_none(), "no reasoning text appeared");
    }

    #[test]
    fn openrouter_stream_fixture_gives_reasoning_text_and_usage() {
        let raw = include_str!("../../../tests/fixtures/llm/openrouter_stream.sse");
        let (reply, deltas) = accumulate_sse(raw);
        assert!(deltas.iter().any(|d| matches!(d, Delta::Reasoning(_))));
        assert!(!reply.reasoning.is_empty());
        assert_eq!(reply.raw["reasoning_content"], Value::String(reply.reasoning.clone()));
        assert!(reply.usage.is_some(), "usage arrives in the final chunk");
        assert_eq!(reply.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "get_cwd");
        assert_eq!(reply.tool_calls[0].arguments, "{}");
    }

    #[test]
    fn arguments_split_across_fragments_are_concatenated_by_index() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"run_command","arguments":""}}]}}]}"#),
            &mut out,
        ).unwrap();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}}]}"#),
            &mut out,
        ).unwrap();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" \"ls -la\"}"}}]}}]}"#),
            &mut out,
        ).unwrap();
        let reply = acc.finish().unwrap();
        assert_eq!(reply.tool_calls[0].arguments, r#"{"command": "ls -la"}"#);
        assert!(serde_json::from_str::<Value>(&reply.tool_calls[0].arguments).is_ok());
    }

    #[test]
    fn two_calls_in_one_stream_stay_separate_and_ordered_by_index() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"get_cwd","arguments":""}}]}}]}"#),
            &mut out,
        ).unwrap();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"get_screen","arguments":"{}"}}]}}]}"#),
            &mut out,
        ).unwrap();
        let reply = acc.finish().unwrap();
        let ids: Vec<&str> = reply.tool_calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(reply.tool_calls[1].arguments, "{}", "empty arguments become {{}}");
    }

    #[test]
    fn id_comes_from_the_first_delta_that_has_one() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_cwd","arguments":""}}]}}]}"#),
            &mut out,
        ).unwrap();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"late_id","function":{"arguments":"{}"}}]}}]}"#),
            &mut out,
        ).unwrap();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"ignored_second_id","function":{"arguments":""}}]}}]}"#),
            &mut out,
        ).unwrap();
        let reply = acc.finish().unwrap();
        assert_eq!(reply.tool_calls[0].id, "late_id");
        assert_eq!(reply.tool_calls[0].name, "get_cwd");
    }

    #[test]
    fn tool_calls_count_even_when_finish_reason_is_stop() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"get_cwd","arguments":"{}"}}]}}]}"#),
            &mut out,
        ).unwrap();
        acc.push_chunk(&chunk(r#"{"choices":[{"finish_reason":"stop","delta":{}}]}"#), &mut out).unwrap();
        let reply = acc.finish().unwrap();
        assert_eq!(reply.finish_reason.as_deref(), Some("stop"));
        assert_eq!(reply.tool_calls.len(), 1);
    }

    #[test]
    fn has_finish_reason_is_false_until_a_chunk_carries_one() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(&chunk(r#"{"choices":[{"delta":{"content":"hi"}}]}"#), &mut out).unwrap();
        assert!(!acc.has_finish_reason());
        acc.push_chunk(&chunk(r#"{"choices":[{"finish_reason":"stop","delta":{}}]}"#), &mut out).unwrap();
        assert!(acc.has_finish_reason());
    }

    #[test]
    fn usage_chunk_with_empty_choices_is_accepted() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(&chunk(r#"{"choices":[{"delta":{"content":"hi"}}],"usage":null}"#), &mut out).unwrap();
        acc.push_chunk(
            &chunk(r#"{"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#),
            &mut out,
        ).unwrap();
        let reply = acc.finish().unwrap();
        assert_eq!(reply.text, "hi");
        assert_eq!(reply.usage, Some(Usage { prompt_tokens: 5, completion_tokens: 2 }));
        assert_eq!(reply.raw["content"], "hi");
        assert!(reply.raw.get("tool_calls").is_none());
    }

    #[test]
    fn error_object_in_a_stream_chunk_is_a_provider_error() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        let err = acc
            .push_chunk(
                &chunk(r#"{"error":{"message":"upstream overloaded","code":503},"choices":[{"delta":{"content":""},"finish_reason":"error"}]}"#),
                &mut out,
            )
            .unwrap_err();
        assert_eq!(
            err,
            LlmError::Provider { code: Some(503), kind: None, message: "upstream overloaded".to_string() }
        );
    }

    #[test]
    fn tool_call_without_id_is_a_parse_error() {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        acc.push_chunk(
            &chunk(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_cwd","arguments":"{}"}}]}}]}"#),
            &mut out,
        ).unwrap();
        assert!(matches!(acc.finish(), Err(LlmError::Parse(_))));
    }

    #[test]
    fn non_stream_openrouter_fixture_keeps_the_provider_message_as_raw() {
        let text = include_str!("../../../tests/fixtures/llm/openrouter_tools_nonstream.json");
        let body: Value = serde_json::from_str(text.trim_start()).unwrap();
        let reply = from_completion(&body).unwrap();
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "get_cwd");
        assert_eq!(reply.tool_calls[0].arguments, "{}");
        assert_eq!(reply.reasoning, "The user wants me to use the get_cwd tool.");
        assert_eq!(reply.raw, body["choices"][0]["message"], "raw message is the provider object as returned");
        assert_eq!(reply.usage, Some(Usage { prompt_tokens: 349, completion_tokens: 36 }));
    }

    #[test]
    fn non_stream_opencode_go_fixture_reads_reasoning_content_field() {
        let text = include_str!("../../../tests/fixtures/llm/opencodego_tools_nonstream.json");
        let body: Value = serde_json::from_str(text).unwrap();
        let reply = from_completion(&body).unwrap();
        assert_eq!(reply.text, "I'll check that for you.");
        assert_eq!(reply.reasoning, "", "reasoning_content is present but empty");
        assert_eq!(reply.tool_calls[0].id, "call_00_kmz9kf3kzjshdkcvbdxqkrnx");
        assert_eq!(reply.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(reply.usage, Some(Usage { prompt_tokens: 334, completion_tokens: 29 }));
    }
}
