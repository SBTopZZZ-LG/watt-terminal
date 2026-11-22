//! The file tools as the model sees them: the JSON schemas, the argument checks, and the dispatch to a backend. The agent loop
//! decides the place (this machine, a nested shell, or unknown) and passes it in. This module only refuses or runs.

use std::ops::RangeInclusive;

use serde_json::{Value, json};

use super::backend::{FsBackend, GlobRequest, GrepRequest, ListRequest, ReadRequest};
use super::local::LocalBackend;
use super::scope::{Scope, echo};
use crate::core::tools::ToolSchema;

pub const READ_FILE: &str = "read_file";
pub const LIST_DIR: &str = "list_dir";
pub const GLOB: &str = "glob";
pub const GREP: &str = "grep";
/// The file tools' names, in the order they are offered.
pub const TOOL_NAMES: [&str; 4] = [READ_FILE, LIST_DIR, GLOB, GREP];

/// Lines `read_file` shows when no limit is given.
const DEFAULT_LINES: usize = 2000;
const MAX_LINES: usize = 1_000_000;
const DEFAULT_DEPTH: usize = 1;
const MAX_DEPTH: usize = 3;
const DEFAULT_MATCHES: usize = 200;
const MAX_MATCHES: usize = 500;

/// Where a file tool runs. The agent loop picks it for each call from the session.
pub enum Place<'a> {
    /// This machine, read inside `Scope`.
    Local(&'a Scope),
    /// A shell on another machine (ssh, a container). `host` is named in the refusal.
    Remote { host: String },
    /// No local folder is known for the session.
    Unknown,
}

/// Whether `name` is one of the file tools.
pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// The schemas of the file tools.
pub fn schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: READ_FILE,
            description: "Read a text file on this machine, with line numbers. Use this instead of cat through run_command. \
                It reads only inside the project folder and never reads secrets: private keys, .env files, SSH, GnuPG and AWS \
                folders, or Watt's own settings. It shows up to 2000 lines and 256 KiB per call; when it stops early, the text \
                says which offset to pass next. Binary files and files over 10 MiB are refused.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The file: relative to the project folder, or absolute inside it."},
                    "offset": {"type": "integer", "description": "The first line to show, 1-based. Default 1."},
                    "limit": {"type": "integer", "description": "The most lines to show. Default 2000."}
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: LIST_DIR,
            description: "List a folder on this machine: its entries, sorted, with the size of each file. Use this instead of ls \
                through run_command. Links are listed, not followed. .git and the secret paths are left out. Up to 500 entries.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The folder: relative to the project folder, or absolute inside it. Default the project folder."},
                    "depth": {"type": "integer", "description": "How many levels to list, 1 to 3. Default 1 (the folder's own entries)."}
                }
            }),
        },
        ToolSchema {
            name: GLOB,
            description: "Find files by name on this machine. Works like rg --glob: `*.rs` matches at any depth, `src/*.rs` only \
                directly in src. Respects .gitignore. Returns paths relative to the searched folder, up to 500. Use this instead \
                of find through run_command.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The glob, such as *.md or src/**/*.ts."},
                    "path": {"type": "string", "description": "The folder to search. Default the project folder."}
                },
                "required": ["pattern"]
            }),
        },
        ToolSchema {
            name: GREP,
            description: "Search file contents on this machine with a regular expression (Rust regex syntax). Use this instead of \
                grep or rg through run_command. Respects .gitignore; skips binary files, .git and the secret paths. Each result is \
                file:line:text, with the text cut at 300 characters.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The regular expression to find."},
                    "path": {"type": "string", "description": "A folder or a file to search. Default the project folder."},
                    "glob": {"type": "string", "description": "Only search files whose name matches, such as *.rs."},
                    "case_insensitive": {"type": "boolean", "description": "Match regardless of letter case. Default false."},
                    "max_results": {"type": "integer", "description": "The most matching lines to return, 1 to 500. Default 200."}
                },
                "required": ["pattern"]
            }),
        },
    ]
}

/// Runs one file tool call and returns the text the model reads. A refusal is the error text. The place is checked before
/// the arguments: a remote or unknown place touches no file.
pub fn run(place: Place<'_>, name: &str, args: &Value) -> Result<String, String> {
    let scope = match place {
        Place::Local(scope) => scope,
        Place::Remote { host } => {
            return Err(format!(
                "File tools work on the local machine only for now; this session is on {host}. Use run_command."
            ));
        }
        Place::Unknown => {
            return Err("File tools need this session's local folder, and none is known right now. Use run_command.".to_string());
        }
    };
    let backend = LocalBackend::new(scope);
    match name {
        READ_FILE => backend.read_file(&read_request(args)?),
        LIST_DIR => backend.list_dir(&list_request(args)?),
        GLOB => backend.glob(&glob_request(args)?),
        GREP => backend.grep(&grep_request(args)?),
        _ => Err(format!("unknown file tool `{}`", echo(name))),
    }
}

fn read_request(args: &Value) -> Result<ReadRequest, String> {
    Ok(ReadRequest {
        path: required_text(args, READ_FILE, "path")?,
        offset: count_arg(args, "offset", 1, 1..=MAX_LINES)?,
        limit: count_arg(args, "limit", DEFAULT_LINES, 1..=MAX_LINES)?,
    })
}

fn list_request(args: &Value) -> Result<ListRequest, String> {
    Ok(ListRequest { path: text_arg(args, "path")?, depth: count_arg(args, "depth", DEFAULT_DEPTH, 1..=MAX_DEPTH)? })
}

fn glob_request(args: &Value) -> Result<GlobRequest, String> {
    Ok(GlobRequest { pattern: required_text(args, GLOB, "pattern")?, path: text_arg(args, "path")? })
}

fn grep_request(args: &Value) -> Result<GrepRequest, String> {
    Ok(GrepRequest {
        pattern: required_text(args, GREP, "pattern")?,
        path: text_arg(args, "path")?,
        glob: text_arg(args, "glob")?,
        case_insensitive: flag_arg(args, "case_insensitive")?,
        max_results: count_arg(args, "max_results", DEFAULT_MATCHES, 1..=MAX_MATCHES)?,
    })
}

/// A string argument: None when absent, null or empty. Not trimmed, since a pattern may need its spaces.
fn text_arg(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn required_text(args: &Value, tool: &str, key: &str) -> Result<String, String> {
    text_arg(args, key)?.ok_or_else(|| format!("the {tool} tool needs `{key}`"))
}

/// A whole-number argument within `range`, or `default` when absent or null.
fn count_arg(args: &Value, key: &str, default: usize, range: RangeInclusive<usize>) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| range.contains(n))
            .ok_or_else(|| format!("`{key}` must be a whole number from {} to {}", range.start(), range.end())),
    }
}

fn flag_arg(args: &Value, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn local_scope(tmp: &TempDir) -> Scope {
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "hello\n").unwrap();
        Scope::new(root.clone(), root, None)
    }

    #[test]
    fn a_remote_place_refuses_every_tool_without_touching_files() {
        let place = || Place::Remote { host: "pi@box".to_string() };
        for name in TOOL_NAMES {
            let err = run(place(), name, &json!({"path": "a.txt", "pattern": "x"})).unwrap_err();
            assert_eq!(
                err,
                "File tools work on the local machine only for now; this session is on pi@box. Use run_command.",
                "{name}"
            );
        }
    }

    #[test]
    fn an_unknown_place_refuses_every_tool() {
        for name in TOOL_NAMES {
            let err = run(Place::Unknown, name, &json!({"path": "a.txt", "pattern": "x"})).unwrap_err();
            assert!(err.starts_with("File tools need this session's local folder"), "{name}: {err}");
        }
    }

    #[test]
    fn a_local_place_runs_the_tool() {
        let tmp = TempDir::new();
        let scope = local_scope(&tmp);
        assert_eq!(run(Place::Local(&scope), READ_FILE, &json!({"path": "a.txt"})).unwrap(), "     1\thello\n");
    }

    #[test]
    fn the_arguments_are_checked() {
        let tmp = TempDir::new();
        let scope = local_scope(&tmp);
        let local = || Place::Local(&scope);
        assert_eq!(run(local(), READ_FILE, &json!({})).unwrap_err(), "the read_file tool needs `path`");
        assert_eq!(run(local(), GLOB, &json!({})).unwrap_err(), "the glob tool needs `pattern`");
        assert!(run(local(), READ_FILE, &json!({"path": "a.txt", "offset": 0})).unwrap_err().contains("`offset` must be a whole number from 1"));
        assert!(run(local(), READ_FILE, &json!({"path": "a.txt", "limit": "5"})).unwrap_err().contains("`limit` must be a whole number"));
        assert!(run(local(), LIST_DIR, &json!({"depth": 4})).unwrap_err().contains("from 1 to 3"));
        assert!(run(local(), GREP, &json!({"pattern": "a", "max_results": 501})).unwrap_err().contains("from 1 to 500"));
        assert!(run(local(), GREP, &json!({"pattern": "a", "case_insensitive": "yes"})).unwrap_err().contains("true or false"));
        assert!(run(local(), GREP, &json!({"pattern": "a", "path": 3})).unwrap_err().contains("`path` must be a string"));
        assert!(run(local(), "nope", &json!({})).unwrap_err().contains("unknown file tool"));
    }

    #[test]
    fn the_schemas_name_exactly_the_file_tools() {
        let names: Vec<&str> = schemas().iter().map(|s| s.name).collect();
        assert_eq!(names, TOOL_NAMES);
        for schema in schemas() {
            assert!(is_tool(schema.name));
            assert_eq!(schema.parameters["type"], "object", "{}", schema.name);
            assert!(!schema.description.is_empty(), "{}", schema.name);
        }
        assert!(!is_tool("skill") && !is_tool("run_command"));
    }
}
