//! The `skill` tool: the model loads one skill's instructions, or one file from the skill's folder. It only reads. It runs
//! nothing and grants nothing (there is no allowed-tools), so a skill cannot approve a command. A file path must stay
//! inside the skill folder, and the result is plain text.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::frontmatter;
use super::scan::{SKILL_FILE, MAX_SKILL_FILE, read_skill_md};
use super::{Skill, SkillIndex};
use crate::core::tools::ToolSchema;

pub const TOOL_NAME: &str = "skill";
/// The most text a skill body or a file is shown with. Longer text is cut at a character boundary, with a notice.
pub const TEXT_LIMIT: usize = 32 * 1024;
/// Bytes kept free for the truncation notice, so a cut text stays within its limit.
const NOTICE_RESERVE: usize = 64;
/// The most relative paths listed after a body.
const MAX_LISTED: usize = 50;
/// Folder levels walked when listing files (the top folder is level 1).
const MAX_LIST_DEPTH: usize = 3;
/// Entries looked at while listing; a huge folder stops here.
const MAX_WALK: usize = 1000;
/// The longest requested name or file path echoed in an error.
const ECHO_LIMIT: usize = 200;
/// The longest list of available names in the unknown-skill error.
const AVAILABLE_LIMIT: usize = 1000;

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: TOOL_NAME,
        description: "Load a skill: a reusable instruction pack the user keeps in a skills folder. Call it with the skill's name, \
            from the list in the system prompt, when a task matches the skill's description. It returns the skill's instructions \
            and the files in its folder. Pass `file` (a path relative to the skill folder) to read one of those files. It runs \
            nothing: to run a script, use run_command, which asks the user as usual.",
        parameters: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill's name, as listed."},
                "file": {"type": "string", "description": "Optional. A file inside the skill folder, relative to it, such as scripts/check.sh. Not absolute, no `..`."}
            },
            "required": ["name"]
        }),
    }
}

/// Runs one `skill` call against the turn's index. The text is what the model reads back. An unknown name, or a skill the
/// model may not load, is an error that names the skills it may load.
pub fn run(index: &SkillIndex, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "the skill tool needs a `name`: the skill's name, as listed".to_string())?;
    let skill = index
        .skills
        .iter()
        .find(|s| s.model_invocable && s.name == name)
        .ok_or_else(|| unknown(index, name))?;
    match args.get("file") {
        None | Some(Value::Null) => body(skill),
        Some(Value::String(file)) if file.trim().is_empty() => body(skill),
        Some(Value::String(file)) => file_text(skill, file.trim()),
        Some(_) => Err("`file` must be a string: a path relative to the skill folder".to_string()),
    }
}

/// Cuts `text` to at most `limit` bytes, at a character boundary, and says how many bytes were left out.
pub fn cap_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut keep = limit.saturating_sub(NOTICE_RESERVE);
    while !text.is_char_boundary(keep) {
        keep -= 1;
    }
    format!("{}\n\n[truncated {} bytes]", &text[..keep], text.len() - keep)
}

fn unknown(index: &SkillIndex, name: &str) -> String {
    let shown = echo(name);
    let names: Vec<&str> = index.skills.iter().filter(|s| s.model_invocable).map(|s| s.name.as_str()).collect();
    if names.is_empty() {
        return format!("unknown skill '{shown}'. No skills are available.");
    }
    format!("unknown skill '{shown}'. Available: {}", cap_text(&names.join(", "), AVAILABLE_LIMIT))
}

/// The instructions of a skill (frontmatter stripped, read fresh), then its folder and the files in it.
fn body(skill: &Skill) -> Result<String, String> {
    let text = read_skill_md(&skill.dir)?;
    let parsed = frontmatter::parse(&text)?;
    let files = list_files(&skill.dir);
    let listing = if files.is_empty() { "(none)".to_string() } else { files.join(", ") };
    Ok(format!(
        "{}\n\nSkill folder: {}\nFiles: {listing}\nRelative paths in these instructions are relative to the skill folder. \
         To run a script use run_command (it goes through normal approval).",
        cap_text(parsed.body.trim(), TEXT_LIMIT),
        skill.dir.display(),
    ))
}

/// The text of one file in the skill folder. Only text is shown: a file that is not UTF-8 gives its size instead.
fn file_text(skill: &Skill, file: &str) -> Result<String, String> {
    let shown = echo(file);
    let rel = relative_path(file)?;
    let base = fs::canonicalize(&skill.dir).map_err(|e| format!("cannot open the skill folder ({e})"))?;
    let target = fs::canonicalize(skill.dir.join(&rel)).map_err(|_| format!("no file `{shown}` in the skill folder"))?;
    // A link inside the folder that points outside it resolves outside, and is refused here.
    if !target.starts_with(&base) {
        return Err(format!("`{shown}` leaves the skill folder"));
    }
    if !target.is_file() {
        return Err(format!("`{shown}` is not a file"));
    }
    let total = fs::metadata(&target).map(|m| m.len()).map_err(|e| format!("cannot read `{shown}` ({e})"))?;
    let mut bytes = Vec::new();
    File::open(&target)
        .and_then(|f| f.take(MAX_SKILL_FILE).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read `{shown}` ({e})"))?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        // The read stopped in the middle of a character: keep the whole characters before it.
        Err(e) if e.error_len().is_none() && total > bytes.len() as u64 => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return Ok(format!("binary file, not shown, {total} bytes")),
    };
    Ok(cap_text(text, TEXT_LIMIT))
}

/// The path a `file` argument names, relative to the skill folder. Both separators are accepted, so a Windows path
/// works. Absolute paths, drive prefixes (`C:`), UNC paths and `..` are refused. `.` and empty parts are dropped.
fn relative_path(file: &str) -> Result<PathBuf, String> {
    let shown = echo(file);
    let normal = file.replace('\\', "/");
    if normal.starts_with('/') || normal.contains(':') {
        return Err(format!("`{shown}` must be a path relative to the skill folder"));
    }
    let mut rel = PathBuf::new();
    for part in normal.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(format!("`{shown}` leaves the skill folder: `..` is not allowed")),
            part => rel.push(part),
        }
    }
    if rel.as_os_str().is_empty() {
        return Err("`file` names no file".to_string());
    }
    Ok(rel)
}

/// The files under a skill folder as `/`-separated paths relative to it, sorted. Up to `MAX_LIST_DEPTH` levels; dot-folders
/// and `node_modules` are not entered; `SKILL.md` at the top is left out. Only the first `MAX_WALK` entries are seen, so
/// a huge folder lists a sample of its files. Read errors list nothing.
fn list_files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    walk(dir, "", 1, &mut found);
    found.sort();
    found.truncate(MAX_LISTED);
    found
}

fn walk(dir: &Path, prefix: &str, depth: usize, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<fs::DirEntry> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if found.len() >= MAX_WALK {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_LIST_DEPTH && !name.starts_with('.') && name != "node_modules" {
                walk(&path, &rel, depth + 1, found);
            }
        } else if path.is_file() && !(depth == 1 && name == SKILL_FILE) {
            found.push(rel);
        }
    }
}

/// A requested name or path, shortened for an error message.
fn echo(text: &str) -> String {
    text.chars().take(ECHO_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;
    use crate::core::skills::{Source, scan::scan, Root};

    /// A skill folder `name` under a fresh root, with the given SKILL.md text. Returns the root (kept alive by the caller).
    fn skill_folder(tmp: &TempDir, name: &str, skill_md: &str) -> PathBuf {
        let dir = tmp.path().join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKILL_FILE), skill_md).unwrap();
        dir
    }

    const DEMO: &str = "---\nname: demo\ndescription: Demo skill\n---\nStep one.\nStep two.\n";

    fn index_of(root: &Path) -> SkillIndex {
        scan(&[Root { dir: root.to_path_buf(), source: Source::User }])
    }

    #[test]
    fn no_file_returns_the_body_with_the_folder_and_its_files() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("scripts").join("check.sh"), "echo hi").unwrap();
        fs::write(dir.join("notes.md"), "notes").unwrap();
        let text = run(&index_of(tmp.path()), &json!({"name": "demo"})).expect("loads");
        assert!(text.starts_with("Step one.\nStep two."), "{text}");
        assert!(!text.contains("name: demo"), "frontmatter is stripped: {text}");
        assert!(text.contains(&format!("Skill folder: {}", dir.display())), "{text}");
        assert!(text.contains("Files: notes.md, scripts/check.sh"), "SKILL.md is not listed: {text}");
        assert!(text.contains("To run a script use run_command"), "{text}");
    }

    #[test]
    fn the_listing_skips_dot_folders_and_node_modules_and_stops_at_depth_three() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git").join("config"), "x").unwrap();
        fs::create_dir_all(dir.join("node_modules")).unwrap();
        fs::write(dir.join("node_modules").join("dep.js"), "x").unwrap();
        fs::create_dir_all(dir.join("a").join("b").join("c")).unwrap();
        fs::write(dir.join("a").join("b").join("deep.txt"), "x").unwrap();
        fs::write(dir.join("a").join("b").join("c").join("deeper.txt"), "x").unwrap();
        let text = run(&index_of(tmp.path()), &json!({"name": "demo"})).expect("loads");
        assert!(text.contains("Files: a/b/deep.txt"), "{text}");
        assert!(!text.contains("deeper.txt") && !text.contains(".git") && !text.contains("node_modules"), "{text}");
    }

    #[test]
    fn a_file_argument_returns_its_text() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("scripts").join("check.sh"), "echo ok\n").unwrap();
        let index = index_of(tmp.path());
        assert_eq!(run(&index, &json!({"name": "demo", "file": "scripts/check.sh"})).unwrap(), "echo ok\n");
        // Windows separators and `.` parts work too.
        assert_eq!(run(&index, &json!({"name": "demo", "file": "scripts\\check.sh"})).unwrap(), "echo ok\n");
        assert_eq!(run(&index, &json!({"name": "demo", "file": "./scripts//check.sh"})).unwrap(), "echo ok\n");
    }

    #[test]
    fn paths_that_leave_the_folder_are_refused() {
        let tmp = TempDir::new();
        skill_folder(&tmp, "demo", DEMO);
        fs::write(tmp.path().join("secret.txt"), "outside").unwrap();
        let index = index_of(tmp.path());
        for file in ["../secret.txt", "..\\secret.txt", "/etc/passwd", "\\\\server\\share\\x", "C:\\Windows\\win.ini", "C:win.ini", "a/../../secret.txt"] {
            let err = run(&index, &json!({"name": "demo", "file": file})).expect_err(file);
            assert!(err.contains("relative") || err.contains("leaves") || err.contains("`..`"), "{file}: {err}");
        }
    }

    #[test]
    fn a_missing_file_and_a_folder_are_errors_not_panics() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        fs::create_dir_all(dir.join("sub")).unwrap();
        let index = index_of(tmp.path());
        assert!(run(&index, &json!({"name": "demo", "file": "nope.txt"})).unwrap_err().contains("no file"));
        assert!(run(&index, &json!({"name": "demo", "file": "sub"})).unwrap_err().contains("not a file"));
    }

    #[test]
    fn a_binary_file_is_not_shown() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        fs::write(dir.join("image.bin"), [0xff, 0xfe, 0x00, 0x81]).unwrap();
        let text = run(&index_of(tmp.path()), &json!({"name": "demo", "file": "image.bin"})).unwrap();
        assert_eq!(text, "binary file, not shown, 4 bytes");
    }

    #[test]
    fn long_text_is_cut_at_a_character_boundary_with_a_notice() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        // Multi-byte characters so the cut has to find a boundary.
        let long = "é".repeat(TEXT_LIMIT);
        fs::write(dir.join("long.txt"), &long).unwrap();
        let text = run(&index_of(tmp.path()), &json!({"name": "demo", "file": "long.txt"})).unwrap();
        assert!(text.len() <= TEXT_LIMIT, "{} bytes", text.len());
        assert!(text.contains("[truncated "), "{}", &text[text.len() - 40..]);
        assert!(long.starts_with(text.split("\n\n[truncated").next().unwrap()));
    }

    #[test]
    fn an_unknown_or_manual_skill_is_refused_with_the_available_names() {
        let tmp = TempDir::new();
        skill_folder(&tmp, "demo", DEMO);
        skill_folder(&tmp, "manual", "---\nname: manual\ndescription: Only when asked\ndisable-model-invocation: true\n---\nBody\n");
        let index = index_of(tmp.path());
        let err = run(&index, &json!({"name": "nope"})).unwrap_err();
        assert_eq!(err, "unknown skill 'nope'. Available: demo");
        assert_eq!(run(&index, &json!({"name": "manual"})).unwrap_err(), "unknown skill 'manual'. Available: demo");
        let long = "x".repeat(500);
        let err = run(&index, &json!({"name": long})).unwrap_err();
        assert!(err.starts_with("unknown skill 'xxx") && err.len() < 300, "{err}");
    }

    #[test]
    fn a_missing_name_is_an_error() {
        let index = SkillIndex::default();
        assert!(run(&index, &json!({})).unwrap_err().contains("needs a `name`"));
        assert_eq!(run(&index, &json!({"name": "x"})).unwrap_err(), "unknown skill 'x'. No skills are available.");
        let tmp = TempDir::new();
        skill_folder(&tmp, "demo", DEMO);
        let index = index_of(tmp.path());
        assert!(run(&index, &json!({"name": "demo", "file": 3})).unwrap_err().contains("must be a string"));
    }

    #[test]
    fn a_file_link_that_points_outside_the_folder_is_refused() {
        let tmp = TempDir::new();
        let dir = skill_folder(&tmp, "demo", DEMO);
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        if !make_file_link(&outside, &dir.join("link.txt")) {
            return;
        }
        let err = run(&index_of(tmp.path()), &json!({"name": "demo", "file": "link.txt"})).unwrap_err();
        assert!(err.contains("leaves the skill folder"), "{err}");
    }

    #[cfg(unix)]
    fn make_file_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn make_file_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[test]
    fn cap_text_keeps_the_limit_and_says_how_much_went() {
        assert_eq!(cap_text("short", 100), "short");
        let capped = cap_text(&"ab".repeat(100), 100);
        assert!(capped.len() <= 100, "{}", capped.len());
        assert!(capped.ends_with(" bytes]"), "{capped}");
    }

    #[test]
    fn the_schema_names_the_tool_and_its_required_name() {
        let schema = schema();
        assert_eq!(schema.name, TOOL_NAME);
        assert_eq!(schema.parameters["required"], json!(["name"]));
        assert!(schema.parameters["properties"].get("file").is_some());
    }
}
