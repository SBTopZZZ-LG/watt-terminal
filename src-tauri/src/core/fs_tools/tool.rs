//! The file tools as the model sees them: the JSON schemas, the argument checks, and the dispatch to a backend. The agent loop
//! decides the place (this machine, a nested shell, or unknown) and passes it in. This module only refuses or runs. The
//! write tools (`write_file`, `edit_file`) are not run here: the gate asks first (core/agent), and only `preview_write` and
//! `run_write` touch them.

use std::ops::RangeInclusive;

use serde_json::{Value, json};

use super::backend::{FsBackend, GlobRequest, GrepRequest, ListRequest, ReadRequest};
use super::local::LocalBackend;
use super::scope::{Scope, echo};
use super::write::{self, EditFileArgs, EditSpec, WriteFileArgs, WritePlan};
use crate::core::tools::ToolSchema;

pub const READ_FILE: &str = "read_file";
pub const LIST_DIR: &str = "list_dir";
pub const GLOB: &str = "glob";
pub const GREP: &str = "grep";
/// The read-only file tools' names, in the order they are offered.
pub const TOOL_NAMES: [&str; 4] = [READ_FILE, LIST_DIR, GLOB, GREP];
pub const WRITE_FILE: &str = "write_file";
pub const EDIT_FILE: &str = "edit_file";
/// The write tools' names. Each call needs approval (core/agent).
pub const WRITE_TOOL_NAMES: [&str; 2] = [WRITE_FILE, EDIT_FILE];
/// The most write calls (`write_file` and `edit_file` together) one turn may make.
pub const FS_WRITES_PER_TURN: u32 = 60;

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

/// Whether `name` is one of the file tools, read or write.
pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name) || is_write(name)
}

/// Whether `name` is a write tool: it needs approval, and goes through `preview_write` and `run_write`.
pub fn is_write(name: &str) -> bool {
    WRITE_TOOL_NAMES.contains(&name)
}

/// The schemas of the file tools.
pub fn schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: WRITE_FILE,
            description: "Create or overwrite a whole text file on this machine (UTF-8). Use this instead of heredocs, printf or \
                tee through run_command. Each call needs the user's approval; the card shows what will change. It works only \
                inside the project folder and never writes .git, .env, key or secret files, or Watt's own settings. The parent \
                folder must exist unless create_dirs is true. Up to 2 MiB per call. For a small change to an existing file, \
                use edit_file instead.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The file: relative to the project folder, or absolute inside it."},
                    "content": {"type": "string", "description": "The whole new content of the file, exactly as it should be saved."},
                    "create_dirs": {"type": "boolean", "description": "Create the missing folders of the path. Default false."}
                },
                "required": ["path", "content"]
            }),
        },
        ToolSchema {
            name: EDIT_FILE,
            description: "Change an existing text file on this machine by exact string replacement. Use this instead of sed -i or \
                other edits through run_command. Each call needs the user's approval; the card shows the change. `edits` run in \
                order and all succeed or none does: old_string must match the file exactly (including indentation), once, unless \
                replace_all is true. Put several edits in one call. Line endings and a BOM are kept. It works only inside the \
                project folder and never on .git, .env, key or secret files, or Watt's own settings.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The file: relative to the project folder, or absolute inside it."},
                    "edits": {
                        "type": "array",
                        "description": "The replacements, applied in order. Each edit sees the result of the ones before it.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": {"type": "string", "description": "The exact text to replace. Not empty."},
                                "new_string": {"type": "string", "description": "The text to put in its place. May be empty."},
                                "replace_all": {"type": "boolean", "description": "Replace every match, not only a single one. Default false."}
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        },
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
    let scope = local_scope(place)?;
    let backend = LocalBackend::new(scope);
    match name {
        READ_FILE => backend.read_file(&read_request(args)?),
        LIST_DIR => backend.list_dir(&list_request(args)?),
        GLOB => backend.glob(&glob_request(args)?),
        GREP => backend.grep(&grep_request(args)?),
        _ => Err(format!("unknown file tool `{}`", echo(name))),
    }
}

/// The write as the gate shows it: the card text, after the checks against the file as it is now. Nothing is written. A
/// refusal here is the tool error, and the user is not asked.
pub fn preview_write(place: Place<'_>, name: &str, args: &Value) -> Result<String, String> {
    Ok(write_plan(place, name, args)?.display().to_string())
}

/// Runs an approved write: the checks again against the file as it is now, then the atomic write. Returns the result text.
pub fn run_write(place: Place<'_>, name: &str, args: &Value) -> Result<String, String> {
    write_plan(place, name, args)?.commit()
}

fn write_plan(place: Place<'_>, name: &str, args: &Value) -> Result<WritePlan, String> {
    let scope = local_scope(place)?;
    match name {
        WRITE_FILE => write::plan_write_file(scope, &write_file_args(args)?),
        EDIT_FILE => write::plan_edit_file(scope, &edit_file_args(args)?),
        _ => Err(format!("unknown file tool `{}`", echo(name))),
    }
}

/// The scope of a local place. A remote or unknown place is refused here, before any argument is read.
fn local_scope(place: Place<'_>) -> Result<&Scope, String> {
    match place {
        Place::Local(scope) => Ok(scope),
        Place::Remote { host } => Err(format!(
            "File tools work on the local machine only for now; this session is on {host}. Use run_command."
        )),
        Place::Unknown => Err("File tools need this session's local folder, and none is known right now. Use run_command.".to_string()),
    }
}

fn write_file_args(args: &Value) -> Result<WriteFileArgs, String> {
    let content = match args.get("content") {
        Some(Value::String(content)) => content.clone(),
        None | Some(Value::Null) => return Err(format!("the {WRITE_FILE} tool needs `content`")),
        Some(_) => return Err("`content` must be a string".to_string()),
    };
    Ok(WriteFileArgs { path: required_text(args, WRITE_FILE, "path")?, content, create_dirs: flag_arg(args, "create_dirs")? })
}

fn edit_file_args(args: &Value) -> Result<EditFileArgs, String> {
    let path = required_text(args, EDIT_FILE, "path")?;
    let list = match args.get("edits") {
        Some(Value::Array(list)) if !list.is_empty() => list,
        _ => return Err("`edits` must be a non-empty list of objects with old_string and new_string".to_string()),
    };
    let edits = list
        .iter()
        .enumerate()
        .map(|(i, edit)| -> Result<EditSpec, String> {
            let n = i + 1;
            let text = |key: &str| {
                edit.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("edit #{n} needs `{key}` (text)"))
            };
            Ok(EditSpec {
                old: text("old_string")?,
                new: text("new_string")?,
                replace_all: flag_arg(edit, "replace_all").map_err(|e| format!("edit #{n}: {e}"))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(EditFileArgs { path, edits })
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
        Scope::new(root.clone(), root, Vec::new())
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
        let mut expected: Vec<&str> = TOOL_NAMES.to_vec();
        expected.extend(WRITE_TOOL_NAMES);
        let mut names: Vec<&str> = schemas().iter().map(|s| s.name).collect();
        names.sort_unstable();
        expected.sort_unstable();
        assert_eq!(names, expected);
        for schema in schemas() {
            assert!(is_tool(schema.name));
            assert_eq!(schema.parameters["type"], "object", "{}", schema.name);
            assert!(!schema.description.is_empty(), "{}", schema.name);
        }
        assert!(is_write("write_file") && is_write("edit_file") && !is_write("read_file"));
        assert!(!is_tool("skill") && !is_tool("run_command") && !is_tool("send_keys"));
    }

    #[test]
    fn a_write_call_with_bad_arguments_is_refused_before_any_file_is_touched() {
        let tmp = TempDir::new();
        let scope = local_scope(&tmp);
        let local = || Place::Local(&scope);
        assert_eq!(preview_write(local(), WRITE_FILE, &json!({"path": "n.txt"})).unwrap_err(), "the write_file tool needs `content`");
        assert_eq!(preview_write(local(), WRITE_FILE, &json!({"content": "x"})).unwrap_err(), "the write_file tool needs `path`");
        assert!(preview_write(local(), WRITE_FILE, &json!({"path": "n.txt", "content": 3})).unwrap_err().contains("must be a string"));
        assert!(preview_write(local(), EDIT_FILE, &json!({"path": "a.txt", "edits": []})).unwrap_err().contains("non-empty list"));
        let missing_new = preview_write(local(), EDIT_FILE, &json!({"path": "a.txt", "edits": [{"old_string": "a"}]})).unwrap_err();
        assert_eq!(missing_new, "edit #1 needs `new_string` (text)");
        let bad_flag = preview_write(local(), EDIT_FILE, &json!({"path": "a.txt", "edits": [{"old_string": "a", "new_string": "b", "replace_all": "yes"}]}));
        assert!(bad_flag.unwrap_err().starts_with("edit #1: `replace_all` must be true or false"));
        assert!(!tmp.path().join("proj").join("n.txt").exists());
    }

    #[test]
    fn a_write_in_a_remote_or_unknown_place_is_refused_without_a_card() {
        let args = json!({"path": "n.txt", "content": "x"});
        let remote = Place::Remote { host: "pi@box".to_string() };
        assert!(preview_write(remote, WRITE_FILE, &args).unwrap_err().starts_with("File tools work on the local machine only"));
        assert!(preview_write(Place::Unknown, EDIT_FILE, &json!({"path": "a", "edits": []})).unwrap_err().starts_with("File tools need"));
    }

    /// The acceptance case: a long script full of the characters that make run_command refuse or ask. It is written as an
    /// argument, read back byte for byte, and two places are edited in one call. A variant without a final newline too.
    #[test]
    fn an_acceptance_script_round_trips_byte_for_byte_and_takes_two_edits() {
        let tmp = TempDir::new();
        let scope = local_scope(&tmp);
        let place = || Place::Local(&scope);
        let mut script = String::from("#!/bin/bash\n");
        for i in 0..96 {
            script.push_str(&format!(
                "printf '%s\\n' \"item {i}: \\$HOME \\`whoami\\` \\\\ tab\there 'single' caf\u{e9} \u{2014} \u{2713}\"\n"
            ));
        }
        script.push_str("cat <<'EOF'\nhello 'world' $NOT_EXPANDED \\n\nEOF\n");
        let root = tmp.path().join("proj");
        let write_new = |name: &str, content: &str| {
            let args = json!({"path": name, "content": content});
            let plan = write_plan(place(), WRITE_FILE, &args).unwrap();
            assert!(plan.display().starts_with(&format!("{name} (new file,")), "{}", plan.display());
            assert!(plan.commit().unwrap().starts_with(&format!("wrote {name}:")));
        };
        write_new("run.sh", &script);
        write_new("bare.sh", script.trim_end_matches('\n'));
        assert_eq!(fs::read(root.join("run.sh")).unwrap(), script.as_bytes());
        assert_eq!(fs::read(root.join("bare.sh")).unwrap(), script.trim_end_matches('\n').as_bytes());
        assert!(run_write(place(), READ_FILE, &json!({"path": "run.sh"})).is_err(), "a read tool is not a write");

        let args = json!({"path": "run.sh", "edits": [
            {"old_string": "item 7:", "new_string": "item seven:"},
            {"old_string": "item 80:", "new_string": "item eighty:"}
        ]});
        assert_eq!(run_write(place(), EDIT_FILE, &args).unwrap(), "edited run.sh: 2 replacements in 2 edits (100 lines)");
        let expected = script.replacen("item 7:", "item seven:", 1).replacen("item 80:", "item eighty:", 1);
        assert_eq!(fs::read(root.join("run.sh")).unwrap(), expected.as_bytes());
        let shown = run(place(), READ_FILE, &json!({"path": "run.sh", "offset": 9, "limit": 1})).unwrap();
        assert!(shown.contains("item seven:"), "{shown}");
    }
}
