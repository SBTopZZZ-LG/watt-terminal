//! The `/export` Markdown: the stored conversation as `## You` / `## Agent` blocks, in stored order.
//!
//! The live history is the source (never the capped UI transcript), so nothing is lost. User messages keep
//! their string content or their text parts, with attachment names appended; assistant messages keep their
//! non-empty text. Tool messages and empty or tool-call-only assistant messages are skipped.

use serde_json::Value;

/// The stored conversation as Markdown: `## You` blocks for user messages (string content or text parts,
/// with `Attachments: name, ...` appended from `watt_image`/`watt_file` parts), `## Agent` for non-empty
/// assistant text. Tool messages, empty assistant messages and tool-call-only messages are skipped.
pub fn markdown(messages: &[Value]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for message in messages {
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                if let Some(block) = user_block(message.get("content")) {
                    blocks.push(block);
                }
            }
            Some("assistant") => {
                if let Some(text) = message.get("content").and_then(Value::as_str)
                    && !text.trim().is_empty()
                {
                    blocks.push(format!("## Agent\n\n{text}"));
                }
            }
            _ => {}
        }
    }
    blocks.join("\n\n")
}

/// One `## You` block, or `None` when the content holds nothing to show (no text and no named attachments,
/// or content that is neither a string nor a parts array).
fn user_block(content: Option<&Value>) -> Option<String> {
    let (text, names) = match content {
        Some(Value::String(text)) => (text.clone(), Vec::new()),
        Some(Value::Array(parts)) => user_parts(parts),
        _ => return None,
    };
    if text.trim().is_empty() && names.is_empty() {
        return None;
    }
    if names.is_empty() {
        return Some(format!("## You\n\n{text}"));
    }
    let attached = names.join(", ");
    if text.trim().is_empty() {
        return Some(format!("## You\n\nAttachments: {attached}"));
    }
    Some(format!("## You\n\n{text}\n\nAttachments: {attached}"))
}

/// The text and attachment names of a user message's parts array: `text` parts keep their text,
/// `watt_image`/`watt_file` parts lend their names. Anything else is skipped.
fn user_parts(parts: &[Value]) -> (String, Vec<String>) {
    let mut texts: Vec<&str> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    texts.push(text);
                }
            }
            Some("watt_image") | Some("watt_file") => {
                if let Some(name) = part.get("name").and_then(Value::as_str) {
                    names.push(name.to_string());
                }
            }
            _ => {}
        }
    }
    (texts.join("\n"), names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_and_assistant_messages_become_blocks_in_order() {
        let messages = vec![
            json!({"role": "user", "content": "do it"}),
            json!({"role": "assistant", "content": "done"}),
        ];
        assert_eq!(markdown(&messages), "## You\n\ndo it\n\n## Agent\n\ndone");
    }

    #[test]
    fn user_parts_keep_text_and_append_attachment_names() {
        let messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "look"},
                {"type": "watt_image", "name": "shot.png"},
                {"type": "watt_file", "name": "notes.txt"},
            ],
        })];
        assert_eq!(markdown(&messages), "## You\n\nlook\n\nAttachments: shot.png, notes.txt");
    }

    #[test]
    fn tool_messages_and_empty_or_call_only_assistant_messages_are_skipped() {
        let messages = vec![
            json!({"role": "user", "content": "run it"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
            json!({"role": "assistant", "content": ""}),
            json!({"role": "assistant", "content": "   "}),
            json!({"role": "assistant", "content": "finished"}),
        ];
        assert_eq!(markdown(&messages), "## You\n\nrun it\n\n## Agent\n\nfinished");
    }

    #[test]
    fn empty_histories_and_unknown_shapes_export_nothing() {
        assert_eq!(markdown(&[]), "");
        assert_eq!(markdown(&[json!({"role": "user", "content": 7})]), "");
        assert_eq!(markdown(&[json!({"role": "user", "content": [{"type": "video"}]})]), "");
        assert_eq!(markdown(&[json!({"role": "system", "content": "rules"})]), "");
    }

    #[test]
    fn an_attachment_only_message_exports_its_names() {
        let messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "watt_image", "name": "shot.png"},
            ],
        })];
        assert_eq!(markdown(&messages), "## You\n\nAttachments: shot.png");
    }
}
