//! One tool of a connected server, as the runtime keeps it, and the schema the model is offered for it. Pure logic.

use rmcp::model::Tool;
use serde_json::{Value, json};

use crate::core::external_tools::ExternalToolSchema;

/// Longest tool description kept, in characters. Longer ones are cut.
pub const DESCRIPTION_LIMIT: usize = 1024;

/// The tools of one server, as the runtime keeps them after `tools/list`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    /// The server's own name for the tool. Calls go out under this name.
    pub original: String,
    /// The description, cut to `DESCRIPTION_LIMIT` characters.
    pub description: String,
    /// The input schema as the server sent it (see `parameters`).
    pub input_schema: Value,
}

impl ToolInfo {
    pub fn from_mcp(tool: &Tool) -> ToolInfo {
        ToolInfo {
            original: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or("").chars().take(DESCRIPTION_LIMIT).collect(),
            input_schema: Value::Object((*tool.input_schema).clone()),
        }
    }
}

/// The schema the model gets for a tool's arguments: the server's schema, with the root made an object schema.
pub fn parameters(input_schema: &Value) -> Value {
    sanitize_schema(root_object(input_schema))
}

/// The root must be a `{"type":"object", ...}` schema. Anything else (no type, another type, a union) becomes an empty
/// object schema, so the provider never sees a non-object root. Everything else passes through unchanged.
fn root_object(schema: &Value) -> Value {
    match schema {
        Value::Object(map) if map.get("type") == Some(&json!("object")) => schema.clone(),
        _ => json!({"type": "object", "properties": {}}),
    }
}

/// Hook for provider-specific rewrites of a schema (for example, a provider that rejects a keyword). Identity for now:
/// the schemas pass through unchanged, which the non-strict OpenAI-compatible request accepts in the POC.
fn sanitize_schema(schema: Value) -> Value {
    schema
}

/// The schema offered for `tool` under the offered `name`, described as coming from `server`.
pub fn offered(server: &str, name: &str, tool: &ToolInfo) -> ExternalToolSchema {
    let description = if tool.description.is_empty() {
        format!("[{server}]")
    } else {
        format!("[{server}] {}", tool.description)
    };
    ExternalToolSchema { name: name.to_string(), description, parameters: parameters(&tool.input_schema) }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use super::*;

    fn tool(name: &str, description: Option<&str>, schema: Value) -> Tool {
        let Value::Object(map) = schema else { panic!("test schema must be an object") };
        let mut tool = Tool::new(Cow::Owned(name.to_string()), Cow::Borrowed(""), Arc::new(map));
        tool.description = description.map(|d| Cow::Owned(d.to_string()));
        tool
    }

    #[test]
    fn an_object_root_passes_through_unchanged() {
        let schema = json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object",
            "properties": {"q": {"$ref": "#/$defs/Q"}}, "$defs": {"Q": {"type": "string"}}});
        assert_eq!(parameters(&schema), schema);
    }

    #[test]
    fn a_missing_or_non_object_root_becomes_an_empty_object_schema() {
        let empty = json!({"type": "object", "properties": {}});
        assert_eq!(parameters(&json!({})), empty);
        assert_eq!(parameters(&json!({"properties": {"a": {}}})), empty);
        assert_eq!(parameters(&json!({"type": "string"})), empty);
        assert_eq!(parameters(&json!({"type": ["object", "null"]})), empty);
        assert_eq!(parameters(&json!(true)), empty);
    }

    #[test]
    fn a_long_description_is_cut_to_the_limit_in_characters() {
        let text = "é".repeat(DESCRIPTION_LIMIT + 10);
        let info = ToolInfo::from_mcp(&tool("t", Some(&text), json!({"type": "object"})));
        assert_eq!(info.description.chars().count(), DESCRIPTION_LIMIT);
        let none = ToolInfo::from_mcp(&tool("t", None, json!({"type": "object"})));
        assert_eq!(none.description, "");
    }

    #[test]
    fn the_offered_schema_names_the_server_and_keeps_the_original_schema() {
        let info = ToolInfo::from_mcp(&tool("get docs", Some("Fetch docs"), json!({"type": "object", "properties": {"id": {"type": "integer", "format": "uint32"}}})));
        let offered = offered("context7", "mcp__context7__get_docs", &info);
        assert_eq!(offered.name, "mcp__context7__get_docs");
        assert_eq!(offered.description, "[context7] Fetch docs");
        assert_eq!(offered.parameters["properties"]["id"]["format"], "uint32");
        assert_eq!(info.original, "get docs");
    }

    #[test]
    fn a_tool_without_description_is_offered_with_the_server_tag_only() {
        let info = ToolInfo::from_mcp(&tool("t", None, json!({"type": "object"})));
        assert_eq!(offered("s", "mcp__s__t", &info).description, "[s]");
    }
}
