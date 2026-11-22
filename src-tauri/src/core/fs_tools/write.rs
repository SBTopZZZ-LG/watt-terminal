//! The write tools' work: `write_file` (the whole file) and `edit_file` (exact replacements). Each call first becomes a
//! `WritePlan`: the checks against the file as it is now, the bytes to write, the card preview, and the result text. The
//! gate shows the preview; on approval the plan is made again and committed, so a file that changed while the card was
//! open is checked again. Nothing is written by planning. A write is atomic: a temp file in the same folder, then a rename.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::scope::{Scope, WriteTarget, echo};

/// The most content one `write_file` call may carry.
pub const WRITE_LIMIT: usize = 2 * 1024 * 1024;
/// The largest file `edit_file` changes.
pub const EDIT_LIMIT: usize = 10 * 1024 * 1024;
/// Content lines the `write_file` card shows.
const WRITE_PREVIEW_LINES: usize = 30;
/// Lines per side of one edit the `edit_file` card shows.
const EDIT_SIDE_LINES: usize = 12;
/// Lines the `edit_file` card shows in all.
const EDIT_PREVIEW_LINES: usize = 60;
const BOM: char = '\u{feff}';

/// One exact replacement of `edit_file`. Newlines are normalised to LF before matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSpec {
    pub old: String,
    pub new: String,
    pub replace_all: bool,
}

/// The arguments of `write_file`, already checked for type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
    pub create_dirs: bool,
}

/// The arguments of `edit_file`, already checked for type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFileArgs {
    pub path: String,
    pub edits: Vec<EditSpec>,
}

/// A write checked against the file as it is now. `display` is the card body; `done` is the result text the model reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritePlan {
    target: PathBuf,
    make_dirs: bool,
    bytes: Vec<u8>,
    display: String,
    done: String,
}

impl WritePlan {
    pub fn display(&self) -> &str {
        &self.display
    }

    /// Writes the planned bytes atomically and returns the result text.
    pub fn commit(self) -> Result<String, String> {
        write_atomic(&self.target, &self.bytes, self.make_dirs)?;
        Ok(self.done)
    }
}

/// `write_file`: creates or overwrites the file. An existing UTF-8 BOM is kept.
pub fn plan_write_file(scope: &Scope, args: &WriteFileArgs) -> Result<WritePlan, String> {
    if args.content.len() > WRITE_LIMIT {
        return Err(format!(
            "the content is {} bytes; write_file takes at most 2 MiB per call. Write a smaller file, or make a large one in parts: \
             write_file for the first part, then edit_file to change it.",
            args.content.len()
        ));
    }
    let target = scope.resolve_write(&args.path, args.create_dirs)?;
    let rel = scope.relative(&target.path);
    let (old_lines, bom) = existing_text(&target)?.map_or((None, false), |old| {
        (Some(old.lines().count()), old.starts_with(BOM))
    });
    let text = if bom { format!("{BOM}{}", args.content) } else { args.content.clone() };
    let lines = args.content.lines().count();
    let done = format!(
        "wrote {rel}: {lines} lines, {} bytes ({})",
        text.len(),
        if target.exists { "overwrote" } else { "created" }
    );
    Ok(WritePlan {
        make_dirs: target.new_dirs,
        bytes: text.into_bytes(),
        display: write_preview(&rel, old_lines, &args.content),
        done,
        target: target.path,
    })
}

/// `edit_file`: the edits are applied in order, all or nothing. The file's line endings (CRLF or LF, by majority) and its
/// BOM are kept; the edits' newlines are read as LF.
pub fn plan_edit_file(scope: &Scope, args: &EditFileArgs) -> Result<WritePlan, String> {
    let target = scope.resolve_write(&args.path, false)?;
    let shown = echo(args.path.trim());
    if !target.exists {
        return Err(format!("`{shown}` does not exist: edit_file changes existing files; use write_file to create one"));
    }
    let len = fs::metadata(&target.path).map_err(|e| format!("cannot read `{shown}` ({e})"))?.len();
    if len > EDIT_LIMIT as u64 {
        return Err(format!("`{shown}` is {len} bytes; edit_file works on files up to 10 MiB"));
    }
    let bytes = fs::read(&target.path).map_err(|e| format!("cannot read `{shown}` ({e})"))?;
    if bytes.contains(&0) {
        return Err(format!("`{shown}` is binary (it has NUL bytes); edit_file changes text files only"));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("`{shown}` is not UTF-8 text; edit_file changes text files only"))?;
    let (bom, body) = match text.strip_prefix(BOM) {
        Some(rest) => (BOM.to_string(), rest),
        None => (String::new(), text.as_str()),
    };
    let crlf = body.matches("\r\n").count();
    let lf = body.matches('\n').count() - crlf;
    let use_crlf = crlf > lf;

    let edits: Vec<EditSpec> = args
        .edits
        .iter()
        .map(|e| EditSpec { old: e.old.replace("\r\n", "\n"), new: e.new.replace("\r\n", "\n"), replace_all: e.replace_all })
        .collect();
    let applied = apply_edits(&body.replace("\r\n", "\n"), &edits)?;
    let eol_text = if use_crlf { applied.text.replace('\n', "\r\n") } else { applied.text.clone() };
    let rel = scope.relative(&target.path);
    let replacements: usize = applied.counts.iter().sum();
    let done = format!(
        "edited {rel}: {replacements} replacements in {} edits ({} lines)",
        edits.len(),
        applied.text.lines().count()
    );
    Ok(WritePlan {
        make_dirs: false,
        bytes: format!("{bom}{eol_text}").into_bytes(),
        display: edit_preview(&rel, &edits, &applied.counts),
        done,
        target: target.path,
    })
}

/// The result of applying edits to a text: the new text, and per edit how many places it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    pub counts: Vec<usize>,
}

/// Applies `edits` to `text` in order. Each edit sees the result of the ones before it. The first failure is the error,
/// and nothing is returned, so the caller writes nothing.
pub fn apply_edits(text: &str, edits: &[EditSpec]) -> Result<Applied, String> {
    let mut current = text.to_string();
    let mut counts = Vec::with_capacity(edits.len());
    for (i, edit) in edits.iter().enumerate() {
        let n = i + 1;
        if edit.old.is_empty() {
            return Err(format!("edit #{n}: old_string is empty"));
        }
        if edit.old == edit.new {
            return Err(format!("edit #{n}: old_string and new_string are the same"));
        }
        let found = current.matches(edit.old.as_str()).count();
        if found == 0 {
            return Err(format!("old_string not found (edit #{n}). Nothing was written."));
        }
        if found > 1 && !edit.replace_all {
            return Err(format!(
                "matches {found} places (edit #{n}): add surrounding context or set replace_all. Nothing was written."
            ));
        }
        current = if edit.replace_all {
            current.replace(&edit.old, &edit.new)
        } else {
            current.replacen(&edit.old, &edit.new, 1)
        };
        counts.push(if edit.replace_all { found } else { 1 });
    }
    Ok(Applied { text: current, counts })
}

/// The card body for `write_file`: the path and what changes, then the first lines of the content.
pub fn write_preview(rel: &str, old_lines: Option<usize>, content: &str) -> String {
    let new_lines = content.lines().count();
    let head = match old_lines {
        None => format!("{rel} (new file, {new_lines} lines)"),
        Some(old) => format!("{rel} (overwrite, {old} -> {new_lines} lines)"),
    };
    let mut out = vec![head];
    out.extend(content.lines().take(WRITE_PREVIEW_LINES).map(str::to_string));
    if new_lines > WRITE_PREVIEW_LINES {
        out.push(format!("... {} more lines", new_lines - WRITE_PREVIEW_LINES));
    }
    out.join("\n")
}

/// The card body for `edit_file`: the path, then per edit a header and its removed (`-`) and added (`+`) lines, capped.
pub fn edit_preview(rel: &str, edits: &[EditSpec], counts: &[usize]) -> String {
    let mut body = Vec::new();
    for (i, (edit, count)) in edits.iter().zip(counts).enumerate() {
        let places = if *count == 1 { "1 place".to_string() } else { format!("{count} places") };
        body.push(format!("edit #{} ({places})", i + 1));
        push_side(&mut body, "- ", &edit.old);
        push_side(&mut body, "+ ", &edit.new);
    }
    if body.len() > EDIT_PREVIEW_LINES {
        let more = body.len() - EDIT_PREVIEW_LINES;
        body.truncate(EDIT_PREVIEW_LINES);
        body.push(format!("... {more} more lines"));
    }
    let mut out = vec![rel.to_string()];
    out.extend(body);
    out.join("\n")
}

fn push_side(out: &mut Vec<String>, prefix: &str, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    out.extend(lines.iter().take(EDIT_SIDE_LINES).map(|line| format!("{prefix}{line}")));
    if lines.len() > EDIT_SIDE_LINES {
        out.push(format!("... {} more lines", lines.len() - EDIT_SIDE_LINES));
    }
}

/// The current text of an existing target, or None when it does not exist. Only the line count and the BOM are used, so a
/// file that is not UTF-8 is read lossily.
fn existing_text(target: &WriteTarget) -> Result<Option<String>, String> {
    if !target.exists {
        return Ok(None);
    }
    let bytes = fs::read(&target.path).map_err(|e| format!("cannot read the existing file ({e})"))?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Writes `bytes` to `path` through a temp file in the same folder, then renames it over `path`. Creates the folder first
/// when `make_dirs`. On failure the temp file is removed and `path` is left as it was.
pub fn write_atomic(path: &Path, bytes: &[u8], make_dirs: bool) -> Result<(), String> {
    let shown = path.display();
    let dir = path.parent().ok_or_else(|| format!("cannot write `{shown}`: it has no folder"))?;
    let name = path.file_name().ok_or_else(|| format!("cannot write `{shown}`: it does not name a file"))?;
    if make_dirs {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create the folder of `{shown}` ({e})"))?;
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!(".{}.watt-{}-{nanos}.tmp", name.to_string_lossy(), std::process::id()));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cannot write `{shown}` ({e}). The file was not changed."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn edit(old: &str, new: &str) -> EditSpec {
        EditSpec { old: old.into(), new: new.into(), replace_all: false }
    }

    fn scope(tmp: &TempDir) -> (Scope, PathBuf) {
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        (Scope::new(root.clone(), root.clone(), Vec::new()), root)
    }

    fn write_args(path: &str, content: &str) -> WriteFileArgs {
        WriteFileArgs { path: path.into(), content: content.into(), create_dirs: false }
    }

    fn edit_args(path: &str, edits: Vec<EditSpec>) -> EditFileArgs {
        EditFileArgs { path: path.into(), edits }
    }

    #[test]
    fn write_preview_shows_the_change_and_the_first_lines() {
        assert_eq!(write_preview("a.txt", None, "x\ny\n"), "a.txt (new file, 2 lines)\nx\ny");
        assert_eq!(write_preview("a.txt", Some(5), "x"), "a.txt (overwrite, 5 -> 1 lines)\nx");
        let long: String = (1..=35).map(|i| format!("l{i}\n")).collect();
        let shown = write_preview("b.txt", None, &long);
        assert!(shown.contains("\nl30\n... 5 more lines"), "{shown}");
        assert!(!shown.contains("l31"), "{shown}");
    }

    #[test]
    fn edit_preview_lists_each_edit_and_caps_the_lines() {
        let edits = vec![edit("old one", "new one"), EditSpec { old: "a".into(), new: "b".into(), replace_all: true }];
        let shown = edit_preview("src/a.rs", &edits, &[1, 3]);
        assert_eq!(
            shown,
            "src/a.rs\nedit #1 (1 place)\n- old one\n+ new one\nedit #2 (3 places)\n- a\n+ b"
        );
        let big = EditSpec { old: (0..20).map(|i| format!("o{i}\n")).collect(), new: "n".into(), replace_all: false };
        let shown = edit_preview("f", &[big], &[1]);
        assert!(shown.contains("- o11\n... 8 more lines\n+ n"), "{shown}");
        let many: Vec<EditSpec> = (0..40).map(|i| edit(&format!("a{i}"), &format!("b{i}"))).collect();
        let shown = edit_preview("f", &many, &vec![1; 40]);
        assert!(shown.ends_with(&format!("... {} more lines", 40 * 3 - EDIT_PREVIEW_LINES)), "{shown}");
        assert_eq!(shown.lines().count(), 1 + EDIT_PREVIEW_LINES + 1);
    }

    #[test]
    fn apply_edits_applies_in_order_and_counts_each_edit() {
        let applied = apply_edits("a b a\nc", &[edit("b", "B"), EditSpec { old: "a".into(), new: "A".into(), replace_all: true }]).unwrap();
        assert_eq!(applied.text, "A B A\nc");
        assert_eq!(applied.counts, vec![1, 2]);
        let second_sees_first = apply_edits("x", &[edit("x", "yy"), edit("yy", "z")]).unwrap();
        assert_eq!(second_sees_first.text, "z");
    }

    #[test]
    fn apply_edits_names_the_failing_edit() {
        let not_found = apply_edits("abc", &[edit("a", "b"), edit("zz", "y")]).unwrap_err();
        assert_eq!(not_found, "old_string not found (edit #2). Nothing was written.");
        let ambiguous = apply_edits("a a", &[edit("a", "b")]).unwrap_err();
        assert!(ambiguous.starts_with("matches 2 places (edit #1): add surrounding context or set replace_all"), "{ambiguous}");
        assert!(apply_edits("a", &[edit("", "b")]).unwrap_err().contains("old_string is empty"));
        assert!(apply_edits("a", &[edit("a", "a")]).unwrap_err().contains("are the same"));
    }

    #[test]
    fn edit_file_is_all_or_nothing() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let result = plan_edit_file(&scope, &edit_args("a.txt", vec![edit("one", "1"), edit("missing", "x")]));
        assert_eq!(result.unwrap_err(), "old_string not found (edit #2). Nothing was written.");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn edit_file_two_spots_in_one_call_write_the_exact_result() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::write(root.join("a.sh"), "echo 'hi'\necho \"$HOME\"\n").unwrap();
        let plan = plan_edit_file(&scope, &edit_args("a.sh", vec![edit("'hi'", "'bye'"), edit("$HOME", "${HOME}")])).unwrap();
        assert_eq!(
            plan.display(),
            "a.sh\nedit #1 (1 place)\n- 'hi'\n+ 'bye'\nedit #2 (1 place)\n- $HOME\n+ ${HOME}"
        );
        let done = plan.commit().unwrap();
        assert_eq!(done, "edited a.sh: 2 replacements in 2 edits (2 lines)");
        assert_eq!(fs::read_to_string(root.join("a.sh")).unwrap(), "echo 'bye'\necho \"${HOME}\"\n");
    }

    #[test]
    fn edit_file_keeps_crlf_bom_and_a_missing_final_newline() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::write(root.join("win.txt"), "\u{feff}one\r\ntwo\r\nthree").unwrap();
        let plan = plan_edit_file(&scope, &edit_args("win.txt", vec![edit("one\ntwo", "one\nTWO\nextra")])).unwrap();
        plan.commit().unwrap();
        assert_eq!(fs::read(root.join("win.txt")).unwrap(), "\u{feff}one\r\nTWO\r\nextra\r\nthree".as_bytes());
    }

    #[test]
    fn edit_file_refuses_binary_invalid_utf8_and_missing_files() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::write(root.join("bin.dat"), [b'a', 0, b'b']).unwrap();
        fs::write(root.join("latin.txt"), [0xffu8, 0xfe]).unwrap();
        assert!(plan_edit_file(&scope, &edit_args("bin.dat", vec![edit("a", "b")])).unwrap_err().contains("binary"));
        assert!(plan_edit_file(&scope, &edit_args("latin.txt", vec![edit("a", "b")])).unwrap_err().contains("not UTF-8"));
        let missing = plan_edit_file(&scope, &edit_args("nope.txt", vec![edit("a", "b")])).unwrap_err();
        assert!(missing.contains("does not exist") && missing.contains("write_file"), "{missing}");
    }

    #[test]
    fn edit_file_refuses_a_file_over_its_limit() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::write(root.join("big.txt"), vec![b'a'; EDIT_LIMIT + 1]).unwrap();
        let refused = plan_edit_file(&scope, &edit_args("big.txt", vec![edit("a", "b")])).unwrap_err();
        assert!(refused.contains("up to 10 MiB"), "{refused}");
    }

    #[test]
    fn write_file_creates_overwrites_and_keeps_the_bom() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        let created = plan_write_file(&scope, &write_args("n.txt", "a\nb\n")).unwrap();
        assert_eq!(created.display(), "n.txt (new file, 2 lines)\na\nb");
        assert_eq!(created.commit().unwrap(), "wrote n.txt: 2 lines, 4 bytes (created)");
        assert_eq!(fs::read_to_string(root.join("n.txt")).unwrap(), "a\nb\n");

        fs::write(root.join("bom.txt"), "\u{feff}old\n").unwrap();
        let over = plan_write_file(&scope, &write_args("bom.txt", "new\n")).unwrap();
        assert!(over.display().starts_with("bom.txt (overwrite, 1 -> 1 lines)"));
        assert_eq!(over.commit().unwrap(), "wrote bom.txt: 1 lines, 7 bytes (overwrote)");
        assert_eq!(fs::read(root.join("bom.txt")).unwrap(), "\u{feff}new\n".as_bytes());
    }

    #[test]
    fn write_file_checks_the_folder_and_the_size() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        let missing = plan_write_file(&scope, &write_args("sub/x.txt", "x")).unwrap_err();
        assert!(missing.contains("create_dirs"), "{missing}");
        let made = plan_write_file(
            &scope,
            &WriteFileArgs { path: "sub/x.txt".into(), content: "x".into(), create_dirs: true },
        )
        .unwrap();
        made.commit().unwrap();
        assert_eq!(fs::read_to_string(root.join("sub").join("x.txt")).unwrap(), "x");
        let big = "x".repeat(WRITE_LIMIT + 1);
        assert!(plan_write_file(&scope, &write_args("big.txt", &big)).unwrap_err().contains("at most 2 MiB"));
        assert!(!root.join("big.txt").exists());
    }

    #[test]
    fn write_file_refuses_git_blocked_and_outside_paths_before_writing() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::create_dir_all(root.join(".git")).unwrap();
        assert!(plan_write_file(&scope, &write_args(".git/config", "x")).unwrap_err().contains(".git"));
        assert!(plan_write_file(&scope, &write_args(".env", "x")).unwrap_err().contains(".env"));
        assert!(plan_write_file(&scope, &write_args("../outside.txt", "x")).unwrap_err().contains("outside the project folder"));
        assert!(!root.join(".env").exists());
    }

    #[test]
    fn a_plan_is_checked_again_when_committed() {
        let tmp = TempDir::new();
        let (scope, root) = scope(&tmp);
        fs::write(root.join("a.txt"), "one\n").unwrap();
        let shown = plan_edit_file(&scope, &edit_args("a.txt", vec![edit("one", "1")])).unwrap();
        // The file changes while the card is open: the edit no longer applies, so the commit is planned again and refused.
        fs::write(root.join("a.txt"), "uno\n").unwrap();
        let again = plan_edit_file(&scope, &edit_args("a.txt", vec![edit("one", "1")])).unwrap_err();
        assert!(again.starts_with("old_string not found"), "{again}");
        assert!(shown.display().contains("one"));
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "uno\n");
    }

    #[test]
    fn a_failed_atomic_write_leaves_no_temp_file_and_the_target_alone() {
        let tmp = TempDir::new();
        let (_, root) = scope(&tmp);
        // A folder where the file should be: the rename fails, so the temp file must be removed.
        let target = root.join("is-a-folder");
        fs::create_dir_all(&target).unwrap();
        assert!(write_atomic(&target, b"data", false).is_err());
        let leftovers: Vec<_> = fs::read_dir(&root).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert!(leftovers.iter().all(|n| !n.to_string_lossy().contains(".watt-")), "{leftovers:?}");
        assert!(target.is_dir());
    }
}
