//! Custom slash commands: prompt templates from `<config>/commands` (global) and
//! `<workspace>/.watt/commands` (project). Each command is one Markdown file; the file stem is the
//! command name and the body is the template. An optional leading `---` block carries `description:`
//! and `argument-hint:`. Everything is lenient: unreadable, oversize, non-UTF-8 or badly named files
//! are skipped, never an error, and listed in the catalog's `skipped` with the reason.

use std::fs;
use std::path::Path;

use serde::Serialize;

/// A file larger than this is skipped, never read.
const FILE_CAP: u64 = 64 * 1024;
/// A default description is cut to this many characters.
const DESCRIPTION_LIMIT: usize = 100;

/// Where a command came from. Project commands win over global ones of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    Project,
    Global,
}

/// One custom command: its template plus what `/help` shows for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    /// The `argument-hint:` frontmatter value. Null when the file has none.
    pub hint: Option<String>,
    pub source: CommandSource,
    pub template: String,
}

/// The custom commands plus the two directories, so `/help` can show where to create them. The
/// directories are reported even when they do not exist; each is null only when it cannot apply.
/// `skipped` names the `*.md` files that were skipped and why, as `<file>: <reason>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandCatalog {
    pub commands: Vec<CommandInfo>,
    pub project_dir: Option<String>,
    pub global_dir: Option<String>,
    pub skipped: Vec<String>,
}

/// The catalog from two command directories. Pure filesystem, no Tauri: missing directories read as
/// empty, and a bad file never fails the whole read. Direct `*.md` children only, sorted by name.
pub fn catalog(project_dir: Option<&Path>, global_dir: Option<&Path>) -> CommandCatalog {
    let mut commands = Vec::new();
    let mut skipped = Vec::new();
    if let Some(dir) = global_dir {
        let (infos, bad) = read_dir(dir, CommandSource::Global);
        commands.extend(infos);
        skipped.extend(bad);
    }
    if let Some(dir) = project_dir {
        let (infos, bad) = read_dir(dir, CommandSource::Project);
        // Project names replace global ones; the rest are added. Sorted at the end.
        for info in infos {
            match commands.iter_mut().find(|c| c.name == info.name) {
                Some(existing) => *existing = info,
                None => commands.push(info),
            }
        }
        skipped.extend(bad);
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    CommandCatalog {
        commands,
        project_dir: project_dir.map(|p| p.to_string_lossy().into_owned()),
        global_dir: global_dir.map(|p| p.to_string_lossy().into_owned()),
        skipped,
    }
}

/// The valid commands in one directory plus the skipped files, each as `<file>: <reason>`.
/// Direct `*.md` children only, in file-name order.
fn read_dir(dir: &Path, source: CommandSource) -> (Vec<CommandInfo>, Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut names: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    names.sort();
    let mut infos = Vec::new();
    let mut skipped = Vec::new();
    for path in &names {
        match read_file(path, source) {
            Ok(info) => infos.push(info),
            Err(reason) => skipped.push(format!(
                "{}: {reason}",
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
            )),
        }
    }
    (infos, skipped)
}

/// One command file. `Err` is the concise reason it was skipped: invalid name, not a regular
/// file, too large, unreadable, not UTF-8, or an empty template.
fn read_file(path: &Path, source: CommandSource) -> Result<CommandInfo, String> {
    let stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| "invalid name".to_string())?;
    if !valid_name(stem) {
        return Err("invalid name".to_string());
    }
    if fs::metadata(path).is_ok_and(|m| !m.is_file()) {
        return Err("not a regular file".to_string());
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > FILE_CAP) {
        return Err("too large".to_string());
    }
    let bytes = fs::read(path).map_err(|_| "unreadable".to_string())?;
    if bytes.len() as u64 > FILE_CAP {
        return Err("too large".to_string());
    }
    let text = String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    let (frontmatter, body) = split_frontmatter(&text);
    let template = body.trim().to_string();
    if template.is_empty() {
        return Err("empty template".to_string());
    }
    let frontmatter = frontmatter.unwrap_or_default();
    let description =
        front_value(frontmatter, "description").unwrap_or_else(|| default_description(&template, stem));
    let hint = front_value(frontmatter, "argument-hint");
    Ok(CommandInfo { name: stem.to_string(), description, hint, source, template })
}

/// A command name: lowercase ASCII, digits, `_` and `-`, starting with a letter or digit.
fn valid_name(stem: &str) -> bool {
    let mut chars = stem.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Splits an optional leading `---` frontmatter block from the template body. Without a closing
/// `---` the whole file is the body.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(first_end) = text.find('\n') else {
        // A single line: a lone `---` is a body of its own, not an unclosed block.
        return (None, text);
    };
    let (first, _) = text.split_at(first_end);
    if first.trim() != "---" {
        return (None, text);
    }
    let after = text[first_end + 1..].strip_prefix('\r').unwrap_or(&text[first_end + 1..]);
    let mut offset = 0;
    for line in after.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        if content.trim() == "---" {
            return (Some(&after[..offset]), &after[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, text)
}

/// The last value of a frontmatter `key:` line, unquoted and without surrounding whitespace. Lines
/// without a colon and unknown keys are ignored here by the caller asking only for known keys.
/// Empty values read as absent.
fn front_value(frontmatter: &str, key: &str) -> Option<String> {
    let mut found = None;
    for line in frontmatter.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() == key {
            found = Some(unquote(v.trim()));
        }
    }
    found.filter(|v| !v.is_empty())
}

/// Strips one layer of matching single or double quotes.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// The description when frontmatter has none: the first non-empty body line with leading `#` and
/// `-` stripped, cut to `DESCRIPTION_LIMIT` characters. The file name when nothing readable remains.
fn default_description(template: &str, name: &str) -> String {
    for line in template.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let stripped = trimmed.trim_start_matches(['#', '-']).trim();
        if stripped.is_empty() {
            break;
        }
        return cap_chars(stripped, DESCRIPTION_LIMIT);
    }
    name.to_string()
}

fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max).collect() }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn dir(path: &Path) -> PathBuf {
        fs::create_dir_all(path).unwrap();
        path.to_path_buf()
    }

    fn write(dir: &Path, name: &str, content: &[u8]) {
        fs::write(dir.join(name), content).unwrap();
    }

    fn names(found: &CommandCatalog) -> Vec<&str> {
        found.commands.iter().map(|c| c.name.as_str()).collect()
    }

    fn skipped(found: &CommandCatalog) -> Vec<&str> {
        found.skipped.iter().map(|s| s.as_str()).collect()
    }

    #[test]
    fn missing_dirs_read_as_empty_but_paths_are_reported() {
        let missing = Path::new("/no/such/dir");
        let found = catalog(None, Some(missing));
        assert!(found.commands.is_empty());
        assert_eq!(found.project_dir, None);
        assert_eq!(found.global_dir, Some("/no/such/dir".to_string()));
        let found = catalog(None, None);
        assert!(found.commands.is_empty());
        assert!(found.skipped.is_empty());
        assert_eq!(found.project_dir, None);
        assert_eq!(found.global_dir, None);
    }

    #[test]
    fn plain_body_uses_first_line_as_description() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "plan.md", b"# Ship it\n\nInvestigate and write a plan.\n");
        let found = catalog(None, Some(&global));
        assert_eq!(names(&found), ["plan"]);
        let cmd = &found.commands[0];
        assert_eq!(cmd.description, "Ship it");
        assert_eq!(cmd.hint, None);
        assert_eq!(cmd.source, CommandSource::Global);
        assert_eq!(cmd.template, "# Ship it\n\nInvestigate and write a plan.");
        assert!(found.skipped.is_empty());
        assert_eq!(found.global_dir, Some(global.to_string_lossy().into_owned()));
        assert_eq!(found.project_dir, None);
    }

    #[test]
    fn frontmatter_description_and_hint_win_with_quotes_unquoted() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(
            &global,
            "review.md",
            b"---\ndescription: \"Review the diff\"\nargument-hint: '[focus]'\nunknown: ignored\n---\nLook at $ARGUMENTS.\n",
        );
        let found = catalog(None, Some(&global));
        let cmd = &found.commands[0];
        assert_eq!(cmd.description, "Review the diff");
        assert_eq!(cmd.hint, Some("[focus]".to_string()));
        assert_eq!(cmd.template, "Look at $ARGUMENTS.");
    }

    #[test]
    fn unclosed_frontmatter_is_the_whole_template() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "oops.md", b"---\ndescription: lost\nA template without a fence.\n");
        let found = catalog(None, Some(&global));
        let cmd = &found.commands[0];
        // The first non-empty line is `---`, which strips to nothing, so the name is the description.
        assert_eq!(cmd.description, "oops");
        assert_eq!(cmd.template, "---\ndescription: lost\nA template without a fence.");
    }

    #[test]
    fn invalid_names_are_skipped() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        for bad in ["Bad.md", "_x.md", "-x.md", "has space.md", "a.b.md", ".md", "UPPER.md"] {
            write(&global, bad, b"body\n");
        }
        write(&global, "ok-name_1.md", b"body\n");
        write(&global, "9lives.md", b"body\n");
        let found = catalog(None, Some(&global));
        assert_eq!(names(&found), ["9lives", "ok-name_1"]);
        let bad = skipped(&found);
        assert!(bad.iter().any(|s| s.starts_with("Bad.md:")), "Bad.md is reported, got {bad:?}");
        assert!(bad.iter().any(|s| s.starts_with("UPPER.md:")), "UPPER.md is reported, got {bad:?}");
        assert!(bad.iter().all(|s| s.contains(": ")), "every entry names the file and reason, got {bad:?}");
    }

    #[test]
    fn only_direct_md_files_count() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "top.md", b"top\n");
        write(&global, "note.txt", b"not a command\n");
        write(&global, "noext", b"not a command\n");
        let sub = dir(&global.join("sub"));
        write(&sub, "nested.md", b"nested\n");
        fs::create_dir_all(global.join("weird.md")).unwrap();
        let found = catalog(None, Some(&global));
        assert_eq!(names(&found), ["top"]);
        assert_eq!(skipped(&found), ["weird.md: not a regular file"]);
    }

    #[test]
    fn oversize_and_non_utf8_files_are_skipped() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "big.md", &vec![b'x'; 65 * 1024]);
        write(&global, "ok.md", b"fine\n");
        write(&global, "binary.md", &[0xff, 0xfe, b'x', b'\n']);
        let found = catalog(None, Some(&global));
        assert_eq!(names(&found), ["ok"]);
        assert_eq!(skipped(&found), ["big.md: too large", "binary.md: not UTF-8"]);
    }

    #[test]
    fn empty_body_is_skipped() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "empty.md", b"---\ndescription: nothing\n---\n   \n");
        write(&global, "blank.md", b"\n\n");
        write(&global, "kept.md", b"kept\n");
        let found = catalog(None, Some(&global));
        assert_eq!(names(&found), ["kept"]);
        assert_eq!(skipped(&found), ["blank.md: empty template", "empty.md: empty template"]);
    }

    #[test]
    fn project_beats_global_and_commands_are_sorted() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        let project = dir(&tmp.path().join("project"));
        write(&global, "shared.md", b"global version\n");
        write(&global, "zebra.md", b"---\ndescription: global zebra\n---\nzebra\n");
        write(&global, "apple.md", b"apple\n");
        write(&project, "shared.md", b"project version\n");
        write(&project, "mango.md", b"mango\n");
        let found = catalog(Some(&project), Some(&global));
        assert_eq!(names(&found), ["apple", "mango", "shared", "zebra"]);
        let shared = found.commands.iter().find(|c| c.name == "shared").unwrap();
        assert_eq!(shared.template, "project version");
        assert_eq!(shared.source, CommandSource::Project);
        let zebra = found.commands.iter().find(|c| c.name == "zebra").unwrap();
        assert_eq!(zebra.source, CommandSource::Global);
    }

    #[test]
    fn skipped_merges_global_and_project() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        let project = dir(&tmp.path().join("project"));
        write(&global, "Bad.md", b"body\n");
        write(&global, "ok.md", b"fine\n");
        write(&project, "big.md", &vec![b'x'; 65 * 1024]);
        let found = catalog(Some(&project), Some(&global));
        assert_eq!(names(&found), ["ok"]);
        assert_eq!(skipped(&found), ["Bad.md: invalid name", "big.md: too large"]);
    }

    #[test]
    fn default_description_strips_markers_and_caps_at_100_chars() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "dash.md", b"- do the thing\nrest\n");
        write(&global, "hashes.md", b"### Big title\nrest\n");
        let long = "x".repeat(150);
        write(&global, "long.md", format!("{long}\n").as_bytes());
        let found = catalog(None, Some(&global));
        let by_name = |n: &str| found.commands.iter().find(|c| c.name == n).unwrap().description.clone();
        assert_eq!(by_name("dash"), "do the thing");
        assert_eq!(by_name("hashes"), "Big title");
        assert_eq!(by_name("long").chars().count(), 100);
    }

    #[test]
    fn empty_hint_and_empty_description_fall_back() {
        let tmp = TempDir::new();
        let global = dir(&tmp.path().join("global"));
        write(&global, "bare.md", b"---\ndescription:\nargument-hint: \"\"\n---\nBody line.\n");
        let found = catalog(None, Some(&global));
        let cmd = &found.commands[0];
        assert_eq!(cmd.description, "Body line.");
        assert_eq!(cmd.hint, None);
    }
}
