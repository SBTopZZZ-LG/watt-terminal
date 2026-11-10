//! The todo list the model keeps for a conversation: the `todo_write` tool, its validation, the repair of a loaded
//! list, and the compact block the agent adds to the system message while the list is not empty.
//!
//! The list belongs to the conversation: it survives across turns, is saved with it and cleared by a new conversation.
//! It is not part of the History. The block is rebuilt from the list on every model call, so no stored message carries
//! it, and a changed list changes only the next request's system message.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::tools::ToolSchema;

pub const TOOL_NAME: &str = "todo_write";
pub const MAX_ITEMS: usize = 30;
pub const MAX_CONTENT_CHARS: usize = 200;

/// The conversation's list, shared by the conversation and the agent turns that run on it.
pub type TodoList = Arc<Mutex<Vec<Todo>>>;

const BLOCK_HEADER: &str = "Your todo list (update it with todo_write):";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// The `todo_write` tool as the model sees it. It is not one of the backend tools in `tools::schemas()`: the agent loop
/// handles it itself.
pub fn schema() -> ToolSchema {
    ToolSchema {
        name: TOOL_NAME,
        description: "Replace the whole todo list of this conversation. Use it for work with three or more steps: list \
            the steps, keep exactly one item in_progress, and mark items completed as you finish them. Send an empty \
            list to clear it. At most 30 items; each content is one line of at most 200 characters. The current list \
            is shown to you with every request.",
        parameters: json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "The complete list, in order. It replaces the previous list.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "What the step is, in a few words."},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                        },
                        "required": ["content", "status"],
                    },
                },
            },
            "required": ["items"],
        }),
    }
}

/// Validates the arguments of a `todo_write` call. The error text is for the model: it names the item and the rule.
pub fn parse(args: &Value) -> Result<Vec<Todo>, String> {
    let items = args
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "items must be an array of {content, status} objects".to_string())?;
    if items.len() > MAX_ITEMS {
        return Err(format!(
            "the list has {} items; the limit is {MAX_ITEMS}. Send only the items still needed",
            items.len()
        ));
    }
    let mut todos = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let number = index + 1;
        let raw = item.get("content").and_then(Value::as_str).unwrap_or_default();
        let content = clean_content(raw).map_err(|why| format!("item {number}: {why}"))?;
        let status = match item.get("status").and_then(Value::as_str) {
            Some("pending") => TodoStatus::Pending,
            Some("in_progress") => TodoStatus::InProgress,
            Some("completed") => TodoStatus::Completed,
            _ => return Err(format!("item {number}: status must be pending, in_progress or completed")),
        };
        todos.push(Todo { content, status });
    }
    let active = todos.iter().filter(|t| t.status == TodoStatus::InProgress).count();
    if active > 1 {
        return Err(format!(
            "{active} items are in_progress; keep exactly one in_progress and set the others to pending or completed"
        ));
    }
    Ok(todos)
}

/// Makes a loaded list valid. An entry that fails validation is dropped. Of several in_progress entries the first stays
/// and the rest become pending. At most `MAX_ITEMS` entries are kept.
pub fn repair(items: Vec<Todo>) -> Vec<Todo> {
    let mut out: Vec<Todo> = Vec::new();
    let mut active_seen = false;
    for item in items {
        if out.len() == MAX_ITEMS {
            break;
        }
        let Ok(content) = clean_content(&item.content) else { continue };
        let status = match item.status {
            TodoStatus::InProgress if active_seen => TodoStatus::Pending,
            TodoStatus::InProgress => {
                active_seen = true;
                TodoStatus::InProgress
            }
            other => other,
        };
        out.push(Todo { content, status });
    }
    out
}

/// The content trimmed, or the reason it is refused: empty, containing a control character (a line break or a tab), or
/// longer than `MAX_CONTENT_CHARS` characters.
fn clean_content(raw: &str) -> Result<String, String> {
    let content = raw.trim();
    if content.is_empty() {
        return Err("content is empty".to_string());
    }
    if content.chars().any(char::is_control) {
        return Err("content has a control character; write each item on one line without tabs".to_string());
    }
    let length = content.chars().count();
    if length > MAX_CONTENT_CHARS {
        return Err(format!("content is {length} characters; the limit is {MAX_CONTENT_CHARS}"));
    }
    Ok(content.to_string())
}

/// The block added to the system message, or None when the list is empty. Lines: the current task (or "none"), the
/// completed items, the pending items. Empty Done and Pending lines are left out.
pub fn prompt_block(items: &[Todo]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let current = items.iter().find(|t| t.status == TodoStatus::InProgress).map_or("none", |t| t.content.as_str());
    let mut lines = vec![BLOCK_HEADER.to_string(), format!("Current: {current}")];
    let done = joined(items, TodoStatus::Completed);
    if !done.is_empty() {
        lines.push(format!("Done: {done}"));
    }
    let pending = joined(items, TodoStatus::Pending);
    if !pending.is_empty() {
        lines.push(format!("Pending: {pending}"));
    }
    Some(lines.join("\n"))
}

fn joined(items: &[Todo], status: TodoStatus) -> String {
    items.iter().filter(|t| t.status == status).map(|t| t.content.as_str()).collect::<Vec<_>>().join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: &str) -> Value {
        json!({"content": content, "status": status})
    }

    fn todo(content: &str, status: TodoStatus) -> Todo {
        Todo { content: content.to_string(), status }
    }

    fn err(args: Value) -> String {
        parse(&args).expect_err("refused")
    }

    #[test]
    fn valid_items_keep_their_order_and_trim_their_content() {
        let args = json!({"items": [item("  read the log ", "completed"), item("fix it", "in_progress"), item("test", "pending")]});
        assert_eq!(
            parse(&args).unwrap(),
            vec![
                todo("read the log", TodoStatus::Completed),
                todo("fix it", TodoStatus::InProgress),
                todo("test", TodoStatus::Pending),
            ]
        );
    }

    #[test]
    fn an_empty_list_is_valid_and_clears() {
        assert_eq!(parse(&json!({"items": []})).unwrap(), Vec::new());
    }

    #[test]
    fn the_list_is_limited_to_thirty_items() {
        let thirty: Vec<Value> = (0..MAX_ITEMS).map(|i| item(&format!("step {i}"), "pending")).collect();
        assert_eq!(parse(&json!({"items": thirty})).unwrap().len(), MAX_ITEMS);
        let thirty_one: Vec<Value> = (0..=MAX_ITEMS).map(|i| item(&format!("step {i}"), "pending")).collect();
        let why = err(json!({"items": thirty_one}));
        assert!(why.contains("31 items") && why.contains("limit is 30"), "{why}");
    }

    #[test]
    fn content_must_be_non_empty_after_trimming_and_at_most_200_characters() {
        assert!(err(json!({"items": [item("   ", "pending")]})).contains("item 1: content is empty"));
        assert!(err(json!({"items": [json!({"status": "pending"})]})).contains("item 1: content is empty"));
        let limit = "é".repeat(MAX_CONTENT_CHARS);
        assert_eq!(parse(&json!({"items": [item(&limit, "pending")]})).unwrap()[0].content, limit);
        let too_long = "é".repeat(MAX_CONTENT_CHARS + 1);
        let why = err(json!({"items": [item("ok", "pending"), item(&too_long, "pending")]}));
        assert!(why.contains("item 2: content is 201 characters; the limit is 200"), "{why}");
    }

    #[test]
    fn control_characters_inside_the_content_are_refused() {
        let why = err(json!({"items": [item("first\nsecond", "pending")]}));
        assert!(why.contains("item 1: content has a control character"), "{why}");
        assert!(err(json!({"items": [item("tab\there", "pending")]})).contains("control character"));
        // Only the outer whitespace is trimmed away; a line break at the end is trimmed, not refused.
        assert_eq!(parse(&json!({"items": [item("done\n", "completed")]})).unwrap()[0].content, "done");
    }

    #[test]
    fn a_bad_status_is_refused_with_the_allowed_values() {
        let why = err(json!({"items": [item("x", "doing")]}));
        assert_eq!(why, "item 1: status must be pending, in_progress or completed");
        assert!(err(json!({"items": [json!({"content": "x"})]})).contains("status must be"));
    }

    #[test]
    fn at_most_one_item_may_be_in_progress() {
        let why = err(json!({"items": [item("a", "in_progress"), item("b", "in_progress")]}));
        assert!(why.contains("2 items are in_progress") && why.contains("exactly one"), "{why}");
    }

    #[test]
    fn a_missing_or_non_array_items_field_is_refused() {
        assert!(err(json!({})).contains("items must be an array"));
        assert!(err(json!({"items": "one"})).contains("items must be an array"));
    }

    #[test]
    fn repair_drops_invalid_entries_and_demotes_extra_in_progress_items() {
        let loaded = vec![
            todo("  kept  ", TodoStatus::Completed),
            todo("", TodoStatus::Pending),
            todo("two\nlines", TodoStatus::Pending),
            todo(&"x".repeat(MAX_CONTENT_CHARS + 1), TodoStatus::Pending),
            todo("first active", TodoStatus::InProgress),
            todo("second active", TodoStatus::InProgress),
        ];
        assert_eq!(
            repair(loaded),
            vec![
                todo("kept", TodoStatus::Completed),
                todo("first active", TodoStatus::InProgress),
                todo("second active", TodoStatus::Pending),
            ]
        );
    }

    #[test]
    fn repair_keeps_at_most_thirty_items() {
        let loaded: Vec<Todo> = (0..40).map(|i| todo(&format!("s{i}"), TodoStatus::Pending)).collect();
        let kept = repair(loaded);
        assert_eq!(kept.len(), MAX_ITEMS);
        assert_eq!(kept[0].content, "s0");
    }

    #[test]
    fn the_block_is_absent_for_an_empty_list() {
        assert_eq!(prompt_block(&[]), None);
    }

    #[test]
    fn the_block_names_the_current_task_then_done_then_pending() {
        let items = vec![
            todo("write tests", TodoStatus::Pending),
            todo("read code", TodoStatus::Completed),
            todo("fix bug", TodoStatus::InProgress),
            todo("ship", TodoStatus::Pending),
            todo("plan", TodoStatus::Completed),
        ];
        assert_eq!(
            prompt_block(&items).unwrap(),
            "Your todo list (update it with todo_write):\nCurrent: fix bug\nDone: read code; plan\nPending: write tests; ship"
        );
    }

    #[test]
    fn the_block_says_none_and_leaves_out_empty_lines() {
        let only_pending = vec![todo("a", TodoStatus::Pending)];
        assert_eq!(
            prompt_block(&only_pending).unwrap(),
            "Your todo list (update it with todo_write):\nCurrent: none\nPending: a"
        );
        let all_done = vec![todo("a", TodoStatus::Completed), todo("b", TodoStatus::Completed)];
        assert_eq!(
            prompt_block(&all_done).unwrap(),
            "Your todo list (update it with todo_write):\nCurrent: none\nDone: a; b"
        );
    }

    #[test]
    fn the_schema_takes_items_with_content_and_status() {
        let s = schema();
        assert_eq!(s.name, "todo_write");
        assert_eq!(s.parameters["required"], json!(["items"]));
        assert_eq!(s.parameters["properties"]["items"]["items"]["properties"]["status"]["enum"],
            json!(["pending", "in_progress", "completed"]));
    }
}
