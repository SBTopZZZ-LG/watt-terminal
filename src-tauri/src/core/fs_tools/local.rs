//! The local backend: reads this machine's files. Every path a tool touches goes through `Scope::resolve` first, and every walk
//! leaves out what the scope excludes (`Scope::excluded`) and reads only files the scope permits (`Scope::permits`).

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use ignore::{DirEntry, Walk, WalkBuilder};
use regex::RegexBuilder;

use super::backend::{FsBackend, GlobRequest, GrepRequest, ListRequest, ReadRequest};
use super::scope::{Scope, echo};

/// The most bytes one `read_file` call returns.
const READ_BYTE_LIMIT: usize = 256 * 1024;
/// Bytes kept free for a notice, so a cut read stays inside `READ_BYTE_LIMIT` plus the notice.
const NOTICE_RESERVE: usize = 200;
/// Files over this size are refused by `read_file`, and skipped by `grep`.
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// The first bytes looked at for a NUL: a NUL there means a binary file.
const BINARY_PROBE_BYTES: usize = 8 * 1024;
/// The most entries `list_dir` shows.
const LIST_ENTRY_LIMIT: usize = 500;
/// The most paths `glob` shows.
const GLOB_RESULT_LIMIT: usize = 500;
/// Entries a walk looks at before it stops, so a huge tree cannot hang one call.
const WALK_LIMIT: usize = 20_000;
/// A grep match line is cut to this many characters.
const GREP_LINE_CHARS: usize = 300;

/// The local machine's files, confined by `scope`.
pub struct LocalBackend<'a> {
    scope: &'a Scope,
}

impl<'a> LocalBackend<'a> {
    pub fn new(scope: &'a Scope) -> Self {
        LocalBackend { scope }
    }
}

impl FsBackend for LocalBackend<'_> {
    fn read_file(&self, req: &ReadRequest) -> Result<String, String> {
        let shown = echo(&req.path);
        let path = self.scope.resolve(Some(&req.path))?;
        if path.is_dir() {
            return Err(format!("`{shown}` is a folder; use list_dir to see what is in it"));
        }
        if !path.is_file() {
            return Err(format!("`{shown}` is not a regular file"));
        }
        let bytes = read_capped(&path).map_err(|e| format!("cannot read `{shown}` ({e})"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("`{shown}` is larger than 10 MiB; the file tools read files up to 10 MiB"));
        }
        if is_binary(&bytes) {
            return Err(format!("`{shown}` is a binary file, not shown ({} bytes)", bytes.len()));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| format!("`{shown}` is not UTF-8 text, not shown ({} bytes)", bytes.len()))?;
        numbered(text, req, &shown)
    }

    fn list_dir(&self, req: &ListRequest) -> Result<String, String> {
        let shown = req.path.as_deref().map_or_else(|| ".".to_string(), echo);
        let dir = self.scope.resolve(req.path.as_deref())?;
        if !dir.is_dir() {
            return Err(format!("`{shown}` is not a folder"));
        }
        let mut entries = Vec::new();
        list_into(self.scope, &dir, &dir, req.depth, &mut entries);
        if entries.is_empty() {
            return Ok("(empty folder)".to_string());
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let total = entries.len();
        let mut lines: Vec<String> = entries.iter().take(LIST_ENTRY_LIMIT).map(|(rel, kind)| listed_line(rel, kind)).collect();
        if total > LIST_ENTRY_LIMIT {
            lines.push(format!("[showing the first {LIST_ENTRY_LIMIT} of {total} entries; list a subfolder or use a lower depth]"));
        }
        Ok(lines.join("\n"))
    }

    fn glob(&self, req: &GlobRequest) -> Result<String, String> {
        let base = self.scope.resolve(req.path.as_deref())?;
        let filter = NameFilter::new(&req.pattern)?;
        let mut found = Vec::new();
        for entry in walker(&base, self.scope).take(WALK_LIMIT).flatten() {
            if !is_file(&entry) || !filter.accepts(&base, entry.path()) || !self.scope.permits(entry.path()) {
                continue;
            }
            found.push(rel_text(&base, entry.path()));
        }
        if found.is_empty() {
            return Ok(format!("no files match `{}`", echo(&req.pattern)));
        }
        found.sort();
        let total = found.len();
        found.truncate(GLOB_RESULT_LIMIT);
        let mut text = found.join("\n");
        if total > GLOB_RESULT_LIMIT {
            text.push_str(&format!("\n[showing {GLOB_RESULT_LIMIT} of {total} matches; narrow the pattern or the path]"));
        }
        Ok(text)
    }

    fn grep(&self, req: &GrepRequest) -> Result<String, String> {
        let re = RegexBuilder::new(&req.pattern)
            .case_insensitive(req.case_insensitive)
            .build()
            .map_err(|e| format!("invalid regex `{}`: {}", echo(&req.pattern), echo(&e.to_string())))?;
        let base = self.scope.resolve(req.path.as_deref())?;
        let filter = req.glob.as_deref().map(NameFilter::new).transpose()?;
        let mut matches = Vec::new();
        let mut stop = None;
        let mut visited = 0usize;
        'files: for entry in walker(&base, self.scope).flatten() {
            visited += 1;
            if visited > WALK_LIMIT {
                stop = Some(format!("[stopped after {WALK_LIMIT} entries; narrow the path or add a glob]"));
                break;
            }
            let wanted = filter.as_ref().is_none_or(|f| f.accepts(&base, entry.path()));
            if !is_file(&entry) || !wanted || !self.scope.permits(entry.path()) {
                continue;
            }
            let path = entry.path();
            // A file over the size limit, or a binary one, is skipped without a word: it is not text to search.
            let Ok(bytes) = read_capped(path) else { continue };
            if bytes.len() as u64 > MAX_FILE_BYTES || is_binary(&bytes) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let rel = rel_text(&base, path);
            for (i, line) in text.lines().enumerate() {
                if !re.is_match(line) {
                    continue;
                }
                matches.push(format!("{rel}:{}:{}", i + 1, cut_chars(line, GREP_LINE_CHARS)));
                if matches.len() >= req.max_results {
                    stop = Some(format!(
                        "[stopped at {} matches; narrow the pattern or the path, or raise max_results up to 500]",
                        req.max_results
                    ));
                    break 'files;
                }
            }
        }
        let mut text = if matches.is_empty() { format!("no matches for `{}`", echo(&req.pattern)) } else { matches.join("\n") };
        if let Some(note) = stop {
            text.push('\n');
            text.push_str(&note);
        }
        Ok(text)
    }
}

/// The lines of `text` from `req.offset`, numbered like `cat -n`, within the byte limit. A notice says where to continue.
fn numbered(text: &str, req: &ReadRequest, shown: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok("(empty file)".to_string());
    }
    if req.offset > total {
        return Err(format!("offset {} is past the end: `{shown}` has {total} lines", req.offset));
    }
    let first = req.offset - 1;
    let last = total.min(first.saturating_add(req.limit));
    let mut out = String::new();
    // The first line not shown yet (1-based).
    let mut next = req.offset;
    let mut cut = None;
    for (i, line) in lines[first..last].iter().enumerate() {
        let n = req.offset + i;
        let row = format!("{n:>6}\t{line}\n");
        if out.len() + row.len() <= READ_BYTE_LIMIT {
            out.push_str(&row);
            next = n + 1;
            continue;
        }
        // A line too long for an empty output is cut to fit, so one long line does not stop the read at nothing.
        if out.is_empty() {
            out.push_str(cut_at(&row, READ_BYTE_LIMIT - NOTICE_RESERVE));
            out.push('\n');
            cut = Some(n);
            next = n + 1;
        }
        break;
    }
    let mut notes = Vec::new();
    if let Some(n) = cut {
        notes.push(format!("line {n} is longer than the limit and was cut"));
    }
    if next <= total {
        notes.push(format!("showing lines {}-{} of {total}; continue with offset={next}", req.offset, next - 1));
    }
    if !notes.is_empty() {
        out.push_str(&format!("[{}]", notes.join("; ")));
    }
    Ok(out)
}

/// The file's bytes, read up to one byte past the size limit, so a file that grew while it was read is still refused.
fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0)
}

/// What `list_dir` found at one path.
enum Listed {
    Dir,
    File(u64),
    Symlink,
}

fn listed_line(rel: &str, kind: &Listed) -> String {
    match kind {
        Listed::Dir => format!("dir  {rel}"),
        Listed::File(size) => format!("file  {rel}  ({size} bytes)"),
        Listed::Symlink => format!("symlink  {rel}"),
    }
}

/// Lists `dir` down to `depth` levels (1 is `dir` itself), into `out` as paths relative to `base`. Links are listed, not
/// followed. `.git` and the excluded paths are left out.
fn list_into(scope: &Scope, base: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, Listed)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        if out.len() >= WALK_LIMIT {
            return;
        }
        let path = entry.path();
        if entry.file_name() == ".git" || scope.excluded(&path) {
            continue;
        }
        let rel = rel_text(base, &path);
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            out.push((rel, Listed::Symlink));
        } else if kind.is_dir() {
            out.push((format!("{rel}/"), Listed::Dir));
            if depth > 1 {
                list_into(scope, base, &path, depth - 1, out);
            }
        } else {
            let size = entry.metadata().map_or(0, |m| m.len());
            out.push((rel, Listed::File(size)));
        }
    }
}

/// The file-name filter of `glob` and `grep`, as `rg --glob` reads it: a pattern without a `/` matches the file name at any
/// depth (`*.rs`); one with a `/` matches the path relative to the searched folder (`src/*.rs`, where `*` stops at a `/`).
struct NameFilter {
    matcher: GlobMatcher,
    by_name: bool,
}

impl NameFilter {
    fn new(pattern: &str) -> Result<Self, String> {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("invalid glob `{}` ({e})", echo(pattern)))?;
        Ok(NameFilter { matcher: glob.compile_matcher(), by_name: !pattern.contains('/') })
    }

    fn accepts(&self, base: &Path, path: &Path) -> bool {
        if self.by_name {
            path.file_name().is_some_and(|name| self.matcher.is_match(name))
        } else {
            self.matcher.is_match(rel_text(base, path))
        }
    }
}

/// A gitignore-aware walk from `base`, with `.git` and the excluded paths left out. Entries come in path order. The name
/// filter is applied by the caller: an ignore-crate override would re-include files that .gitignore leaves out.
fn walker(base: &Path, scope: &Scope) -> Walk {
    let mut builder = WalkBuilder::new(base);
    builder.hidden(false).require_git(false).sort_by_file_path(|a, b| a.cmp(b));
    let scope = scope.clone();
    builder.filter_entry(move |entry: &DirEntry| entry.file_name() != ".git" && !scope.excluded(entry.path()));
    builder.build()
}

fn is_file(entry: &DirEntry) -> bool {
    entry.file_type().is_some_and(|kind| kind.is_file())
}

/// `path` relative to `base`, with `/` separators. The base itself is its own name.
fn rel_text(base: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    if rel.as_os_str().is_empty() {
        return base.file_name().map_or_else(|| ".".to_string(), |name| name.to_string_lossy().into_owned());
    }
    rel.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The longest prefix of `text` within `max` bytes, cut at a character boundary.
fn cut_at(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` cut to `max` characters, with `...` when it was cut.
fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}...")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    /// A project folder under a fresh temp dir, and a scope over it.
    fn project(tmp: &TempDir) -> (std::path::PathBuf, Scope) {
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        let scope = Scope::new(root.clone(), root.clone(), None);
        (root, scope)
    }

    fn read(scope: &Scope, path: &str, offset: usize, limit: usize) -> Result<String, String> {
        LocalBackend::new(scope).read_file(&ReadRequest { path: path.to_string(), offset, limit })
    }

    fn list(scope: &Scope, path: Option<&str>, depth: usize) -> Result<String, String> {
        LocalBackend::new(scope).list_dir(&ListRequest { path: path.map(str::to_string), depth })
    }

    fn glob(scope: &Scope, pattern: &str) -> Result<String, String> {
        LocalBackend::new(scope).glob(&GlobRequest { pattern: pattern.to_string(), path: None })
    }

    fn grep(scope: &Scope, pattern: &str, glob: Option<&str>, max_results: usize) -> Result<String, String> {
        LocalBackend::new(scope).grep(&GrepRequest {
            pattern: pattern.to_string(),
            path: None,
            glob: glob.map(str::to_string),
            case_insensitive: false,
            max_results,
        })
    }

    #[test]
    fn read_file_numbers_lines_and_says_how_to_continue() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("five.txt"), "a\nb\nc\nd\ne\n").unwrap();

        assert_eq!(read(&scope, "five.txt", 1, 2000).unwrap(), "     1\ta\n     2\tb\n     3\tc\n     4\td\n     5\te\n");
        assert_eq!(
            read(&scope, "five.txt", 2, 2).unwrap(),
            "     2\tb\n     3\tc\n[showing lines 2-3 of 5; continue with offset=4]"
        );
        assert!(read(&scope, "five.txt", 6, 1).unwrap_err().contains("past the end"));
    }

    #[test]
    fn read_file_crlf_and_empty_files() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("crlf.txt"), "one\r\ntwo\r\n").unwrap();
        fs::write(root.join("empty.txt"), "").unwrap();
        assert_eq!(read(&scope, "crlf.txt", 1, 10).unwrap(), "     1\tone\n     2\ttwo\n");
        assert_eq!(read(&scope, "empty.txt", 1, 10).unwrap(), "(empty file)");
    }

    #[test]
    fn read_file_stops_at_the_byte_limit_with_a_notice() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        let line = "x".repeat(200);
        let body: String = (0..3000).map(|_| format!("{line}\n")).collect();
        fs::write(root.join("big.txt"), body).unwrap();

        let text = read(&scope, "big.txt", 1, 3000).unwrap();
        assert!(text.len() <= READ_BYTE_LIMIT + NOTICE_RESERVE, "{} bytes", text.len());
        let notice = text.rsplit("[showing").next().unwrap().trim_start();
        assert!(notice.starts_with("lines 1-") && notice.contains("of 3000; continue with offset="), "{notice}");
    }

    #[test]
    fn read_file_cuts_one_line_longer_than_the_limit() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("long.txt"), "é".repeat(200_000)).unwrap();
        let text = read(&scope, "long.txt", 1, 10).unwrap();
        assert!(text.len() <= READ_BYTE_LIMIT + NOTICE_RESERVE, "{} bytes", text.len());
        assert!(text.contains("line 1 is longer than the limit and was cut"), "{}", &text[text.len() - 120..]);
    }

    #[test]
    fn read_file_refuses_binary_big_non_utf8_missing_and_directories() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("bin.dat"), [0x41, 0x00, 0x42]).unwrap();
        fs::write(root.join("latin.txt"), [0xff, 0xfe, 0x41]).unwrap();
        fs::write(root.join("huge.txt"), vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();

        assert!(read(&scope, "bin.dat", 1, 10).unwrap_err().contains("binary file"));
        assert!(read(&scope, "latin.txt", 1, 10).unwrap_err().contains("not UTF-8"));
        assert!(read(&scope, "huge.txt", 1, 10).unwrap_err().contains("larger than 10 MiB"));
        assert_eq!(read(&scope, "missing.txt", 1, 10).unwrap_err(), "`missing.txt` does not exist");
        assert!(read(&scope, "sub", 1, 10).unwrap_err().contains("is a folder"));
    }

    #[test]
    fn list_dir_is_sorted_depth_limited_and_skips_git_and_secrets() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::create_dir_all(root.join("src").join("deep")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git").join("HEAD"), "ref").unwrap();
        fs::write(root.join(".env"), "TOKEN=1").unwrap();
        fs::write(root.join("README.md"), "hi").unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src").join("deep").join("x.rs"), "").unwrap();

        // Sorted by path: `README.md` sorts before `src/`.
        assert_eq!(list(&scope, None, 1).unwrap(), "file  README.md  (2 bytes)\ndir  src/");
        let deep = list(&scope, None, 3).unwrap();
        assert_eq!(
            deep,
            "file  README.md  (2 bytes)\ndir  src/\ndir  src/deep/\nfile  src/deep/x.rs  (0 bytes)\nfile  src/main.rs  (12 bytes)"
        );
    }

    #[test]
    fn list_dir_caps_the_entries_and_says_so() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        for i in 0..(LIST_ENTRY_LIMIT + 10) {
            fs::write(root.join(format!("f{i:04}.txt")), "").unwrap();
        }
        let text = list(&scope, None, 1).unwrap();
        assert_eq!(text.lines().count(), LIST_ENTRY_LIMIT + 1);
        assert!(text.ends_with(&format!("[showing the first {LIST_ENTRY_LIMIT} of {} entries; list a subfolder or use a lower depth]", LIST_ENTRY_LIMIT + 10)));
    }

    #[test]
    fn list_dir_refuses_a_file_and_an_outside_path() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("a.txt"), "").unwrap();
        assert!(list(&scope, Some("a.txt"), 1).unwrap_err().contains("is not a folder"));
        assert!(list(&scope, Some(".."), 1).unwrap_err().contains("outside the project folder"));
    }

    #[test]
    fn glob_finds_nested_files_and_respects_gitignore() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::create_dir_all(root.join("src").join("inner")).unwrap();
        fs::write(root.join(".gitignore"), "ignored.rs\n").unwrap();
        fs::write(root.join("src").join("main.rs"), "").unwrap();
        fs::write(root.join("src").join("inner").join("lib.rs"), "").unwrap();
        fs::write(root.join("src").join("ignored.rs"), "").unwrap();
        fs::write(root.join("notes.md"), "").unwrap();

        assert_eq!(glob(&scope, "*.rs").unwrap(), "src/inner/lib.rs\nsrc/main.rs");
        assert_eq!(glob(&scope, "src/*.rs").unwrap(), "src/main.rs");
        assert_eq!(glob(&scope, "*.zig").unwrap(), "no files match `*.zig`");
        assert!(glob(&scope, "[unclosed").unwrap_err().starts_with("invalid glob `[unclosed`"));
    }

    #[test]
    fn glob_skips_git_and_secret_files_and_caps_results() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git").join("config.txt"), "").unwrap();
        fs::write(root.join(".env.txt"), "").unwrap();
        fs::write(root.join("visible.txt"), "").unwrap();
        assert_eq!(glob(&scope, "*.txt").unwrap(), "visible.txt");

        for i in 0..(GLOB_RESULT_LIMIT + 5) {
            fs::write(root.join(format!("many{i:04}.log")), "").unwrap();
        }
        let text = glob(&scope, "*.log").unwrap();
        assert!(text.ends_with(&format!("[showing {GLOB_RESULT_LIMIT} of {} matches; narrow the pattern or the path]", GLOB_RESULT_LIMIT + 5)));
    }

    #[test]
    fn grep_returns_file_line_and_text_and_skips_binary_files() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("a.txt"), "alpha\nneedle here\n").unwrap();
        fs::write(root.join("blob.bin"), [b'n', b'e', b'e', b'd', b'l', b'e', 0x00]).unwrap();
        assert_eq!(grep(&scope, "needle", None, 200).unwrap(), "a.txt:2:needle here");
        assert_eq!(grep(&scope, "absent", None, 200).unwrap(), "no matches for `absent`");
    }

    #[test]
    fn grep_refuses_an_invalid_regex_without_panicking() {
        let tmp = TempDir::new();
        let (_root, scope) = project(&tmp);
        let err = grep(&scope, "(unclosed", None, 200).unwrap_err();
        assert!(err.starts_with("invalid regex `(unclosed`"), "{err}");
    }

    #[test]
    fn grep_caps_matches_and_cuts_long_lines() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("many.txt"), "hit\nhit\nhit\nhit\n").unwrap();
        fs::write(root.join("long.txt"), format!("hit {}\n", "y".repeat(1000))).unwrap();

        let text = grep(&scope, "hit", Some("many.txt"), 2).unwrap();
        assert!(text.contains("[stopped at 2 matches;"), "{text}");
        assert_eq!(text.lines().filter(|l| l.starts_with("many.txt:")).count(), 2);

        let long = grep(&scope, "hit", Some("long.txt"), 200).unwrap();
        assert!(long.ends_with("..."), "{long}");
        assert!(long.chars().count() < 320, "{}", long.chars().count());
    }

    #[test]
    fn grep_is_case_sensitive_unless_asked_and_filters_by_glob() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join("code.rs"), "Needle\n").unwrap();
        fs::write(root.join("notes.md"), "needle\n").unwrap();
        assert_eq!(grep(&scope, "needle", Some("*.rs"), 200).unwrap(), "no matches for `needle`");
        let insensitive = LocalBackend::new(&scope)
            .grep(&GrepRequest { pattern: "needle".into(), path: None, glob: Some("*.rs".into()), case_insensitive: true, max_results: 200 })
            .unwrap();
        assert_eq!(insensitive, "code.rs:1:Needle");
    }

    #[test]
    fn grep_respects_gitignore_and_skips_blocked_files() {
        let tmp = TempDir::new();
        let (root, scope) = project(&tmp);
        fs::write(root.join(".gitignore"), "build/\n").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build").join("out.txt"), "token\n").unwrap();
        fs::write(root.join(".env"), "token=secret\n").unwrap();
        fs::write(root.join("src.txt"), "token\n").unwrap();
        assert_eq!(grep(&scope, "token", None, 200).unwrap(), "src.txt:1:token");
    }
}
