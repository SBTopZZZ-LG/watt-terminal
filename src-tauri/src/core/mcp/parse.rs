//! Reads a pasted MCP server list into server configs. Accepts the `mcpServers` JSON most MCP servers document, either
//! wrapped (`{"mcpServers": {...}}`) or as a bare map of name to config.
//!
//! Values are read as `serde_json::Value` and checked by hand, so no serde message can echo a value. Env and header
//! values go to `ParsedImport::secrets`; the configs keep only their names. Every error and warning names a server
//! and a field, never a value.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::config::{McpLevel, McpServerConfig, McpSettings, McpTransport, MAX_NAME_CHARS, valid_env_name};
use super::secrets::SecretValues;

/// Largest accepted paste, in bytes.
pub const MAX_PASTE_BYTES: usize = 256 * 1024;

const STDIO_FIELDS: &[&str] = &["type", "disabled", "enabled", "command", "args", "env", "cwd", "working_directory"];
const HTTP_FIELDS: &[&str] = &["type", "disabled", "enabled", "url", "headers"];

/// A checked paste, ready to save: the configs, their secret values per server, and what was ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedImport {
    pub servers: Vec<McpServerConfig>,
    /// Per server name: the env and header values. A server with none has no entry.
    pub secrets: BTreeMap<String, SecretValues>,
    /// Fields that were ignored, one line each. Not errors: the import still goes ahead.
    pub warnings: Vec<String>,
}

/// Parses a paste. `Err` lists every problem; nothing is imported then.
pub fn parse_pasted(text: &str) -> Result<ParsedImport, Vec<String>> {
    if text.len() > MAX_PASTE_BYTES {
        return Err(vec![format!("the paste is larger than {} KB", MAX_PASTE_BYTES / 1024)]);
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(vec!["nothing to import: paste the JSON that lists the servers".into()]);
    }
    // Syntax errors name only the class and the position, never the text around it.
    let root: Value = serde_json::from_str(trimmed).map_err(|e| {
        vec![format!("not valid JSON ({:?} error at line {}, column {})", e.classify(), e.line(), e.column())]
    })?;
    let Value::Object(root) = root else {
        return Err(vec!["expected a JSON object with the servers".into()]);
    };

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let servers_map = match root.get("mcpServers") {
        Some(Value::Object(servers)) => {
            for key in root.keys().filter(|key| key.as_str() != "mcpServers") {
                warnings.push(format!("ignored top-level field {key:?}"));
            }
            servers
        }
        Some(_) => return Err(vec!["mcpServers must be an object of server name to config".into()]),
        None => &root,
    };

    let mut servers = Vec::new();
    let mut secrets = BTreeMap::new();
    let mut origin_of_slug: BTreeMap<String, String> = BTreeMap::new();
    for (raw_name, config) in servers_map {
        let Some(slug) = slugify(raw_name) else {
            errors.push(format!("server {raw_name:?} has no letters or digits to make a name from"));
            continue;
        };
        if let Some(first) = origin_of_slug.get(&slug) {
            errors.push(format!("server names {first:?} and {raw_name:?} both become {slug:?}; rename one"));
            continue;
        }
        origin_of_slug.insert(slug.clone(), raw_name.clone());
        match parse_server(&slug, config, &mut warnings) {
            Ok((server, values)) => {
                if !values.env_values.is_empty() || !values.header_values.is_empty() {
                    secrets.insert(slug, values);
                }
                servers.push(server);
            }
            Err(mut problems) => errors.append(&mut problems),
        }
    }

    if servers.is_empty() && errors.is_empty() {
        errors.push("no servers found in the paste".into());
    }
    if errors.is_empty() {
        Ok(ParsedImport { servers, secrets, warnings })
    } else {
        Err(errors)
    }
}

/// Turns a pasted name into a server slug: trimmed, lowercased, every other character becomes '-', cut to the
/// longest allowed length. `None` when nothing is left.
fn slugify(raw: &str) -> Option<String> {
    let mapped: String = raw
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' }
        })
        .collect();
    let cut: String = mapped.trim_matches('-').chars().take(MAX_NAME_CHARS).collect();
    let cut = cut.trim_end_matches('-');
    (!cut.is_empty()).then(|| cut.to_string())
}

/// Parses one server's config. Returns the config (approval Ask, no overrides) and its secret values.
fn parse_server(
    slug: &str,
    config: &Value,
    warnings: &mut Vec<String>,
) -> Result<(McpServerConfig, SecretValues), Vec<String>> {
    let label = format!("server {slug:?}");
    let Some(fields) = config.as_object() else {
        return Err(vec![format!("{label}: config must be an object")]);
    };
    let mut errors = Vec::new();

    let mut enabled = true;
    for key in ["disabled", "enabled"] {
        match fields.get(key) {
            None => {}
            Some(Value::Bool(value)) => {
                // `disabled: true` and `enabled: false` both switch the server off.
                if (key == "disabled") == *value {
                    enabled = false;
                }
            }
            Some(_) => errors.push(format!("{label}: {key} must be true or false")),
        }
    }

    let kind = match fields.get("type") {
        None => None,
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(_) => {
            errors.push(format!("{label}: type must be a string"));
            None
        }
    };
    let transport_kind = match kind {
        None => match (fields.contains_key("command"), fields.contains_key("url")) {
            (true, false) => Some(Kind::Stdio),
            (false, true) => Some(Kind::Http),
            (true, true) => {
                errors.push(format!("{label}: has both command and url; keep one, or set type"));
                None
            }
            (false, false) => {
                errors.push(format!("{label}: needs a command (stdio) or a url (Streamable HTTP)"));
                None
            }
        },
        Some("stdio") => Some(Kind::Stdio),
        Some("http" | "streamable-http" | "streamableHttp") => Some(Kind::Http),
        Some("sse") => {
            errors.push(format!("{label}: legacy SSE is not supported, use the Streamable HTTP URL"));
            None
        }
        Some("websocket") => {
            errors.push(format!("{label}: websocket is not supported, use the Streamable HTTP URL"));
            None
        }
        Some(other) => {
            errors.push(format!("{label}: unknown type {other:?}"));
            None
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }

    let (transport, values) = match transport_kind {
        Some(Kind::Stdio) => parse_stdio(&label, fields, warnings, &mut errors),
        Some(Kind::Http) => parse_http(&label, fields, warnings, &mut errors),
        None => unreachable!("every error path returned above"),
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    let server = McpServerConfig {
        name: slug.to_string(),
        enabled,
        approval: McpLevel::Ask,
        tools: BTreeMap::new(),
        timeout_secs: None,
        transport: transport.expect("no error means a transport"),
    };
    // The same checks as a saved config (names, URL scheme, env and header names).
    let problems = McpSettings { servers: vec![server.clone()] }.validate();
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok((server, values))
}

#[derive(Clone, Copy)]
enum Kind {
    Stdio,
    Http,
}

fn parse_stdio(
    label: &str,
    fields: &Map<String, Value>,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> (Option<McpTransport>, SecretValues) {
    let mut values = SecretValues::default();
    let command = string_field(fields, "command", label, errors);
    let args = match fields.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item.as_str() {
                Some(arg) => Some(arg.to_string()),
                None => {
                    errors.push(format!("{label}: args must be an array of strings"));
                    None
                }
            })
            .collect(),
        Some(_) => {
            errors.push(format!("{label}: args must be an array of strings"));
            Vec::new()
        }
    };

    let mut env_names = Vec::new();
    match fields.get("env") {
        None => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                if !valid_env_name(name) {
                    errors.push(format!("{label}: env variable name {name:?} is not valid"));
                    continue;
                }
                match value.as_str() {
                    Some(text) => {
                        env_names.push(name.clone());
                        values.env_values.insert(name.clone(), text.to_string());
                    }
                    None => errors.push(format!("{label}: env variable {name:?} must have a string value")),
                }
            }
        }
        Some(_) => errors.push(format!("{label}: env must be an object")),
    }

    let cwd = match (fields.get("cwd"), fields.get("working_directory")) {
        (Some(_), Some(_)) => {
            errors.push(format!("{label}: has both cwd and working_directory; keep one"));
            None
        }
        (Some(_), None) => string_field(fields, "cwd", label, errors),
        (None, Some(_)) => string_field(fields, "working_directory", label, errors),
        (None, None) => None,
    };

    note_unknown_fields(label, fields, STDIO_FIELDS, warnings);
    let transport = command.map(|command| McpTransport::Stdio { command, args, cwd, env: env_names });
    (transport, values)
}

fn parse_http(
    label: &str,
    fields: &Map<String, Value>,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> (Option<McpTransport>, SecretValues) {
    let mut values = SecretValues::default();
    let url = string_field(fields, "url", label, errors);

    let mut header_names = Vec::new();
    match fields.get("headers") {
        None => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                match value.as_str() {
                    Some(text) => {
                        header_names.push(name.clone());
                        values.header_values.insert(name.clone(), text.to_string());
                    }
                    None => errors.push(format!("{label}: header {name:?} must have a string value")),
                }
            }
        }
        Some(_) => errors.push(format!("{label}: headers must be an object")),
    }

    note_unknown_fields(label, fields, HTTP_FIELDS, warnings);
    let transport = url.map(|url| McpTransport::Http { url, headers: header_names });
    (transport, values)
}

/// A required string field. Pushes an error when it is missing or not a string.
fn string_field(fields: &Map<String, Value>, key: &str, label: &str, errors: &mut Vec<String>) -> Option<String> {
    match fields.get(key).and_then(Value::as_str) {
        Some(text) => Some(text.to_string()),
        None => {
            errors.push(format!("{label}: {key} must be a string"));
            None
        }
    }
}

fn note_unknown_fields(label: &str, fields: &Map<String, Value>, known: &[&str], warnings: &mut Vec<String>) {
    for key in fields.keys().filter(|key| !known.contains(&key.as_str())) {
        warnings.push(format!("{label}: ignored unknown field {key:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> ParsedImport {
        match parse_pasted(text) {
            Ok(parsed) => parsed,
            Err(errors) => panic!("expected a clean import, got {errors:?}"),
        }
    }

    fn errors(text: &str) -> Vec<String> {
        parse_pasted(text).expect_err("expected errors")
    }

    fn server<'a>(parsed: &'a ParsedImport, name: &str) -> &'a McpServerConfig {
        parsed.servers.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no server {name}"))
    }

    // Real-world snippets, as the servers document them.

    #[test]
    fn fetch_server_from_uvx() {
        let parsed = ok(r#"{"mcpServers":{"fetch":{"command":"uvx","args":["mcp-server-fetch"]}}}"#);
        assert_eq!(parsed.servers.len(), 1);
        let fetch = server(&parsed, "fetch");
        assert!(fetch.enabled);
        assert_eq!(fetch.approval, McpLevel::Ask);
        assert_eq!(
            fetch.transport,
            McpTransport::Stdio { command: "uvx".into(), args: vec!["mcp-server-fetch".into()], cwd: None, env: vec![] }
        );
        assert!(parsed.secrets.is_empty());
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn you_com_free_is_a_keyless_http_url() {
        let parsed = ok(r#"{"mcpServers":{"you.com":{"url":"https://api.you.com/mcp/free"}}}"#);
        let you = server(&parsed, "you-com");
        assert_eq!(
            you.transport,
            McpTransport::Http { url: "https://api.you.com/mcp/free".into(), headers: vec![] }
        );
        assert!(parsed.secrets.is_empty());
    }

    #[test]
    fn you_com_keyed_moves_the_bearer_value_to_secrets() {
        let parsed = ok(r#"{"mcpServers":{"you-com":{"url":"https://api.you.com/mcp","headers":{"Authorization":"Bearer ydc-test-key-000111"}}}}"#);
        let you = server(&parsed, "you-com");
        assert_eq!(
            you.transport,
            McpTransport::Http { url: "https://api.you.com/mcp".into(), headers: vec!["Authorization".into()] }
        );
        let values = &parsed.secrets["you-com"];
        assert_eq!(values.header_values.get("Authorization").map(String::as_str), Some("Bearer ydc-test-key-000111"));
        assert!(values.env_values.is_empty());
    }

    #[test]
    fn context7_remote_with_a_header_key() {
        let parsed = ok(r#"{"mcpServers":{"context7":{"type":"http","url":"https://mcp.context7.com/mcp","headers":{"CONTEXT7_API_KEY":"ctx7-remote-secret-42"}}}}"#);
        let ctx = server(&parsed, "context7");
        assert!(matches!(&ctx.transport, McpTransport::Http { url, .. } if url == "https://mcp.context7.com/mcp"));
        assert_eq!(parsed.secrets["context7"].header_values["CONTEXT7_API_KEY"], "ctx7-remote-secret-42");
    }

    #[test]
    fn context7_local_form_keeps_its_arguments_as_pasted() {
        // The key is an argument here, so it stays in the args. See the report: this needs a decision.
        let parsed = ok(r#"{"mcpServers":{"context7":{"command":"npx","args":["-y","@upstash/context7-mcp","--api-key","ctx7-local-arg-7"]}}}"#);
        assert_eq!(
            server(&parsed, "context7").transport,
            McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@upstash/context7-mcp".into(), "--api-key".into(), "ctx7-local-arg-7".into()],
                cwd: None,
                env: vec![],
            }
        );
    }

    #[test]
    fn filesystem_through_cmd_c_npx_on_windows() {
        let parsed = ok(r#"{"mcpServers":{"filesystem":{"command":"cmd","args":["/c","npx","-y","@modelcontextprotocol/server-filesystem","C:\\Users\\me\\Projects"]}}}"#);
        match &server(&parsed, "filesystem").transport {
            McpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "cmd");
                assert_eq!(args.last().map(String::as_str), Some(r"C:\Users\me\Projects"));
            }
            other => panic!("expected stdio, got {other:?}"),
        }
    }

    // Shapes and options.

    #[test]
    fn a_bare_map_without_the_wrapper_is_accepted() {
        let parsed = ok(r#"{"fetch":{"command":"uvx","args":["mcp-server-fetch"]},"ydc":{"url":"https://api.you.com/mcp/free"}}"#);
        assert_eq!(parsed.servers.len(), 2);
        assert!(matches!(server(&parsed, "ydc").transport, McpTransport::Http { .. }));
    }

    #[test]
    fn explicit_http_type_spellings_are_all_http() {
        for kind in ["http", "streamable-http", "streamableHttp"] {
            let text = format!(r#"{{"mcpServers":{{"s":{{"type":"{kind}","url":"https://x.example/mcp"}}}}}}"#);
            assert!(matches!(server(&ok(&text), "s").transport, McpTransport::Http { .. }), "{kind}");
        }
    }

    #[test]
    fn explicit_stdio_type_with_cwd_and_env_names() {
        let parsed = ok(r#"{"mcpServers":{"local":{"type":"stdio","command":"node","args":["s.js"],"working_directory":"C:\\srv","env":{"API_BASE":"https://a.example","TOKEN":"tok-value-9"}}}}"#);
        assert_eq!(
            server(&parsed, "local").transport,
            McpTransport::Stdio {
                command: "node".into(),
                args: vec!["s.js".into()],
                cwd: Some(r"C:\srv".into()),
                env: vec!["API_BASE".into(), "TOKEN".into()],
            }
        );
        assert_eq!(parsed.secrets["local"].env_values["TOKEN"], "tok-value-9");
        assert_eq!(parsed.secrets["local"].env_values["API_BASE"], "https://a.example");
    }

    #[test]
    fn disabled_true_and_enabled_false_both_switch_a_server_off() {
        let parsed = ok(r#"{"mcpServers":{"a":{"command":"x","disabled":true},"b":{"command":"x","enabled":false},"c":{"command":"x","disabled":false}}}"#);
        assert!(!server(&parsed, "a").enabled);
        assert!(!server(&parsed, "b").enabled);
        assert!(server(&parsed, "c").enabled);
    }

    #[test]
    fn legacy_sse_and_websocket_are_refused() {
        let sse = errors(r#"{"mcpServers":{"old":{"type":"sse","url":"https://x.example/sse"}}}"#);
        assert_eq!(sse, vec![r#"server "old": legacy SSE is not supported, use the Streamable HTTP URL"#.to_string()]);
        let ws = errors(r#"{"mcpServers":{"old":{"type":"websocket","url":"wss://x.example/ws"}}}"#);
        assert!(ws[0].contains("websocket is not supported"), "{ws:?}");
    }

    #[test]
    fn names_are_slugged_and_collisions_are_errors() {
        let parsed = ok(r#"{"mcpServers":{"  My Server!  ":{"command":"x"},"_under-score":{"command":"y"}}}"#);
        assert!(parsed.servers.iter().any(|s| s.name == "my-server"), "{:?}", parsed.servers);
        assert!(parsed.servers.iter().any(|s| s.name == "_under-score"));

        let clash = errors(r#"{"mcpServers":{"You.com":{"command":"x"},"you-com":{"command":"y"}}}"#);
        assert_eq!(clash, vec![r#"server names "You.com" and "you-com" both become "you-com"; rename one"#.to_string()]);
    }

    #[test]
    fn a_long_name_is_cut_to_the_maximum() {
        let long = "a".repeat(50);
        let parsed = ok(&format!(r#"{{"mcpServers":{{"{long}":{{"command":"x"}}}}}}"#));
        assert_eq!(parsed.servers[0].name.len(), MAX_NAME_CHARS);
    }

    #[test]
    fn unknown_fields_are_ignored_with_a_warning() {
        let parsed = ok(r#"{"mcpServers":{"s":{"command":"x","timeout":5,"description":"d"}},"meta":1}"#);
        assert_eq!(parsed.servers.len(), 1);
        assert!(parsed.warnings.iter().any(|w| w.contains("\"timeout\"")), "{:?}", parsed.warnings);
        assert!(parsed.warnings.iter().any(|w| w.contains("\"description\"")), "{:?}", parsed.warnings);
        assert!(parsed.warnings.iter().any(|w| w.contains("\"meta\"")), "{:?}", parsed.warnings);
    }

    // Errors.

    #[test]
    fn malformed_json_reports_the_position() {
        let problems = errors("{\"mcpServers\": {\"a\": }}");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("not valid JSON") && problems[0].contains("line 1, column"), "{problems:?}");
    }

    #[test]
    fn empty_and_non_object_input_are_refused() {
        assert!(errors("   \n ")[0].contains("nothing to import"));
        assert!(errors("[1,2]")[0].contains("expected a JSON object"));
        assert!(errors(r#"{"mcpServers": []}"#)[0].contains("mcpServers must be an object"));
        assert!(errors(r#"{"mcpServers": {}}"#)[0].contains("no servers found"));
    }

    #[test]
    fn huge_input_is_refused_before_parsing() {
        let padding = " ".repeat(MAX_PASTE_BYTES);
        let problems = errors(&format!("{{\"mcpServers\":{{}}}}{padding}"));
        assert!(problems[0].contains("larger than 256 KB"), "{problems:?}");
    }

    #[test]
    fn a_server_without_command_or_url_is_an_error() {
        let problems = errors(r#"{"mcpServers":{"s":{"args":["x"]}}}"#);
        assert_eq!(problems, vec![r#"server "s": needs a command (stdio) or a url (Streamable HTTP)"#.to_string()]);
    }

    #[test]
    fn command_and_url_together_without_a_type_is_an_error() {
        let problems = errors(r#"{"mcpServers":{"s":{"command":"x","url":"https://x.example/mcp"}}}"#);
        assert!(problems[0].contains("has both command and url"), "{problems:?}");
    }

    #[test]
    fn wrong_field_types_are_reported_by_field_name() {
        let problems = errors(r#"{"mcpServers":{"s":{"command":"x","args":"not-a-list","env":[],"cwd":5}}}"#);
        assert!(problems.iter().any(|p| p.contains("args must be an array of strings")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("env must be an object")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("cwd must be a string")), "{problems:?}");
    }

    #[test]
    fn invalid_env_names_and_unknown_type_are_errors() {
        let problems = errors(r#"{"mcpServers":{"s":{"command":"x","env":{"1BAD":"v"}},"t":{"type":"grpc","url":"https://x.example"}}}"#);
        assert!(problems.iter().any(|p| p.contains("env variable name \"1BAD\" is not valid")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("unknown type \"grpc\"")), "{problems:?}");
    }

    #[test]
    fn a_non_http_url_is_an_error_without_echoing_it() {
        let problems = errors(r#"{"mcpServers":{"s":{"url":"ftp://user:pw-value@x.example/mcp?key=url-secret-5"}}}"#);
        assert!(problems.iter().any(|p| p.contains("http or https")), "{problems:?}");
        assert!(problems.iter().all(|p| !p.contains("url-secret-5") && !p.contains("pw-value")), "{problems:?}");
    }

    #[test]
    fn secret_values_never_appear_in_errors_warnings_or_debug_output() {
        let secret = "zz-very-secret-value-991";
        let bad = format!(
            r#"{{"mcpServers":{{"s":{{"command":"x","env":{{"TOKEN":"{secret}","1BAD":"{secret}"}},"headers":{{"X":"{secret}"}},"timeout":3}},"t":{{"command":"y","env":{{"OK":"{secret}"}},"args":5}}}}}}"#
        );
        let problems = errors(&bad);
        assert!(problems.iter().all(|p| !p.contains(secret)), "{problems:?}");

        let good = format!(
            r#"{{"mcpServers":{{"s":{{"command":"x","env":{{"TOKEN":"{secret}"}},"timeout":3}},"h":{{"url":"https://x.example/mcp","headers":{{"Authorization":"Bearer {secret}"}}}}}}}}"#
        );
        let parsed = ok(&good);
        assert!(parsed.warnings.iter().all(|w| !w.contains(secret)), "{:?}", parsed.warnings);
        let debug = format!("{parsed:?}");
        assert!(!debug.contains(secret), "{debug}");
        assert!(debug.contains("TOKEN") && debug.contains("Authorization"), "{debug}");
    }
}
