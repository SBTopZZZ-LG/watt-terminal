//! Which project a conversation belongs to: the project root of the shell's working directory when the conversation
//! is first saved. Derived once and stored, never re-derived.
//!
//! The root is the nearest ancestor of the working directory (the directory itself included) that has a `.git`
//! entry (a directory, or a file for worktrees and submodules). Two kinds of `.git` do not count: one at the home
//! directory (usually a dotfiles repository, so home is a workspace only when it is the working directory itself),
//! and one whose directory contains another known workspace root (a container of projects, not a project). The walk
//! is plain filesystem checks: no process is spawned. It only runs for this machine and an absolute path. For any
//! other host the root is the working directory as the shell reported it. Without a working directory the workspace
//! is unknown.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// The host the shell runs on. Empty for this machine when the shell did not report one.
    pub host: String,
    /// The project root (or the working directory when no repository was found). Empty when unknown.
    pub root: String,
    /// What the UI shows: "~" for the home directory, "/" for the filesystem root, otherwise the root's basename.
    /// "Unknown" when there is no working directory.
    pub label: String,
}

impl Workspace {
    pub fn unknown() -> Self {
        Workspace { host: String::new(), root: String::new(), label: "Unknown".to_string() }
    }
}

/// The workspace for a shell working directory given as `(host, path)`. `local_host` is this machine's name, and
/// `home` is the user's home directory: used to label the root "~" and to ignore a `.git` there. `known` are the
/// workspaces of the saved conversations; the local ones keep a repository that contains them from being the root.
pub fn workspace_for(
    cwd: Option<(&str, &str)>,
    local_host: &str,
    home: Option<&Path>,
    known: &[Workspace],
) -> Workspace {
    let Some((host, path)) = cwd else {
        return Workspace::unknown();
    };
    let local = is_local(host, local_host);
    let root = if local {
        let known_roots: Vec<&Path> = known
            .iter()
            .filter(|w| is_local(&w.host, local_host) && !w.root.is_empty())
            .map(|w| Path::new(w.root.as_str()))
            .collect();
        project_root(path, home, &known_roots)
    } else {
        path.to_string()
    };
    let label = label_for(&root, local, home);
    Workspace { host: host.to_string(), root, label }
}

/// True for this machine: an empty host (the shell reported none), `localhost`, or this machine's name.
fn is_local(host: &str, local_host: &str) -> bool {
    host.is_empty() || host == "localhost" || host == local_host
}

/// The nearest ancestor of `path` (itself included) that contains a `.git` entry the walk accepts, or `path` when there
/// is none. A `.git` at `home` is accepted only when `home` is `path` itself. A candidate is refused when a known root
/// lies strictly below it. Only an absolute path is searched.
fn project_root(path: &str, home: Option<&Path>, known: &[&Path]) -> String {
    let p = Path::new(path);
    if !p.is_absolute() {
        return path.to_string();
    }
    for dir in p.ancestors() {
        if !dir.join(".git").exists() {
            continue;
        }
        if home == Some(dir) && dir != p {
            continue;
        }
        if known.iter().any(|root| *root != dir && root.starts_with(dir)) {
            continue;
        }
        return dir.to_string_lossy().into_owned();
    }
    path.to_string()
}

/// Control characters and the invisible formatting characters that can reorder or hide text when shown: the
/// bidirectional marks and overrides (U+200E, U+200F, U+202A-U+202E, U+2066-U+2069), the zero-width space, word joiner
/// and byte order mark (U+200B, U+2060, U+FEFF). Zero-width non-joiner and joiner (U+200C, U+200D) are not included:
/// emoji sequences and some scripts need them.
pub fn is_hidden_or_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

fn label_for(root: &str, local: bool, home: Option<&Path>) -> String {
    if root.is_empty() {
        return "Unknown".to_string();
    }
    let root_path = Path::new(root);
    if local && home.is_some_and(|h| h == root_path) {
        return "~".to_string();
    }
    if root_path.parent().is_none() {
        return "/".to_string();
    }
    let name = match root_path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => root.to_string(),
    };
    if !name.chars().any(is_hidden_or_control) {
        return name;
    }
    // Hidden and control characters become spaces, as in titles. A name that is only those reads as unknown.
    let shown: String = name.chars().map(|c| if is_hidden_or_control(c) { ' ' } else { c }).collect();
    let shown = shown.trim();
    if shown.is_empty() { "Unknown".to_string() } else { shown.to_string() }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::core::settings::test_support::TempDir;

    const LOCAL: &str = "this-mac";

    fn dir(path: &Path) -> PathBuf {
        fs::create_dir_all(path).unwrap();
        path.to_path_buf()
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn no_working_directory_is_unknown() {
        let w = workspace_for(None, LOCAL, None, &[]);
        assert_eq!(w, Workspace { host: String::new(), root: String::new(), label: "Unknown".into() });
    }

    #[test]
    fn repository_root_is_the_nearest_ancestor_with_a_git_directory() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = dir(&repo.join("src").join("deep"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &[]);
        assert_eq!(w.root, s(&repo));
        assert_eq!(w.label, "repo");
        assert_eq!(w.host, "");
    }

    #[test]
    fn a_git_file_marks_a_worktree_root() {
        let tmp = TempDir::new();
        let wt = dir(&tmp.path().join("wt"));
        fs::write(wt.join(".git"), "gitdir: /somewhere/else\n").unwrap();
        let cwd = dir(&wt.join("pkg"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &[]);
        assert_eq!(w.root, s(&wt));
        assert_eq!(w.label, "wt");
    }

    #[test]
    fn the_working_directory_itself_can_be_the_root() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("here"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let w = workspace_for(Some(("", &s(&repo))), LOCAL, None, &[]);
        assert_eq!(w.root, s(&repo));
        assert_eq!(w.label, "here");
    }

    #[test]
    fn nested_repository_wins_over_an_outer_one() {
        let tmp = TempDir::new();
        let outer = dir(&tmp.path().join("outer"));
        fs::create_dir_all(outer.join(".git")).unwrap();
        let inner = dir(&outer.join("vendor").join("inner"));
        fs::write(inner.join(".git"), "gitdir: x\n").unwrap();
        let cwd = dir(&inner.join("src"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &[]);
        assert_eq!(w.root, s(&inner));
        assert_eq!(w.label, "inner");
    }

    #[test]
    fn no_repository_means_the_working_directory_is_the_root() {
        let tmp = TempDir::new();
        let cwd = dir(&tmp.path().join("plain").join("notes"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &[]);
        assert_eq!(w.root, s(&cwd));
        assert_eq!(w.label, "notes");
    }

    #[test]
    fn home_is_labelled_tilde_and_a_project_under_home_is_not() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        let project = dir(&home.join("proj"));
        let w = workspace_for(Some(("", &s(&home))), LOCAL, Some(&home), &[]);
        assert_eq!(w.root, s(&home));
        assert_eq!(w.label, "~");
        let w = workspace_for(Some(("", &s(&project))), LOCAL, Some(&home), &[]);
        assert_eq!(w.root, s(&project));
        assert_eq!(w.label, "proj");
    }

    #[test]
    fn a_dotfiles_repository_at_home_is_not_the_workspace_of_its_subdirectories() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        fs::create_dir_all(home.join(".git")).unwrap();
        let cwd = dir(&home.join("code"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, Some(&home), &[]);
        assert_eq!(w.root, s(&cwd), "the .git at home is skipped, so the working directory is the root");
        assert_eq!(w.label, "code");
    }

    #[test]
    fn home_is_the_workspace_when_it_is_the_working_directory_even_with_a_git_entry() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        fs::create_dir_all(home.join(".git")).unwrap();
        let w = workspace_for(Some(("", &s(&home))), LOCAL, Some(&home), &[]);
        assert_eq!(w.root, s(&home));
        assert_eq!(w.label, "~");
    }

    #[test]
    fn a_project_under_a_dotfiles_repository_at_home_still_finds_its_own_repository() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        fs::create_dir_all(home.join(".git")).unwrap();
        let project = dir(&home.join("proj"));
        fs::create_dir_all(project.join(".git")).unwrap();
        let cwd = dir(&project.join("src"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, Some(&home), &[]);
        assert_eq!(w.root, s(&project));
        assert_eq!(w.label, "proj");
    }

    #[test]
    fn a_repository_that_contains_a_known_workspace_is_skipped() {
        let tmp = TempDir::new();
        let outer = dir(&tmp.path().join("outer"));
        fs::create_dir_all(outer.join(".git")).unwrap();
        let inner = dir(&outer.join("vendor").join("inner"));
        fs::create_dir_all(inner.join(".git")).unwrap();
        let cwd = dir(&outer.join("scratch"));
        let known = [Workspace { host: String::new(), root: s(&inner), label: "inner".into() }];
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &known);
        assert_eq!(w.root, s(&cwd), "outer holds a known workspace, so it is not the root");
        assert_eq!(w.label, "scratch");
        // Without the known workspace the outer repository is the root, as before.
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &[]);
        assert_eq!(w.root, s(&outer));
    }

    #[test]
    fn a_known_workspace_on_another_host_does_not_make_a_local_repository_a_container() {
        let tmp = TempDir::new();
        let outer = dir(&tmp.path().join("outer"));
        fs::create_dir_all(outer.join(".git")).unwrap();
        let cwd = dir(&outer.join("scratch"));
        let inner = s(&outer.join("vendor").join("inner"));
        let known = [Workspace { host: "build-box".into(), root: inner, label: "inner".into() }];
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None, &known);
        assert_eq!(w.root, s(&outer));
    }

    #[test]
    fn a_remote_host_keeps_its_path_and_never_walks_the_local_disk() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = dir(&repo.join("src"));
        let w = workspace_for(Some(("build-box", &s(&cwd))), LOCAL, None, &[]);
        assert_eq!(w.host, "build-box");
        assert_eq!(w.root, s(&cwd));
        assert_eq!(w.label, "src");
    }

    #[test]
    fn a_remote_home_path_is_not_labelled_tilde() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        let w = workspace_for(Some(("build-box", &s(&home))), LOCAL, Some(&home), &[]);
        assert_eq!(w.label, "home");
    }

    #[test]
    fn localhost_and_the_local_host_name_count_as_this_machine() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = dir(&repo.join("src"));
        for host in ["localhost", LOCAL] {
            let w = workspace_for(Some((host, &s(&cwd))), LOCAL, None, &[]);
            assert_eq!(w.root, s(&repo), "host {host}");
            assert_eq!(w.host, host);
        }
    }

    #[test]
    fn a_relative_path_is_not_searched_and_is_its_own_root() {
        let w = workspace_for(Some(("", "relative/dir")), LOCAL, None, &[]);
        assert_eq!(w.root, "relative/dir");
        assert_eq!(w.label, "dir");
    }

    #[test]
    fn filesystem_root_is_labelled_slash() {
        let w = workspace_for(Some(("build-box", "/")), LOCAL, None, &[]);
        assert_eq!(w.root, "/");
        assert_eq!(w.label, "/");
    }

    #[test]
    fn hidden_and_control_characters_in_a_label_become_spaces() {
        // The remote path needs no disk access, so these names are plain strings.
        let w = workspace_for(Some(("build-box", "/work/re\u{202E}vop")), LOCAL, None, &[]);
        assert_eq!(w.label, "re vop");
        let w = workspace_for(Some(("build-box", "/work/a\u{200B}b\u{FEFF}")), LOCAL, None, &[]);
        assert_eq!(w.label, "a b");
        let w = workspace_for(Some(("build-box", "/work/\u{2066}\u{2069}")), LOCAL, None, &[]);
        assert_eq!(w.label, "Unknown", "a name that is only hidden characters reads as unknown");
    }

    #[test]
    fn zero_width_joiner_stays_in_a_label_and_ordinary_names_are_unchanged() {
        let family = "/work/family\u{200D}\u{1F467}";
        let w = workspace_for(Some(("build-box", family)), LOCAL, None, &[]);
        assert_eq!(w.label, "family\u{200D}\u{1F467}");
        let w = workspace_for(Some(("build-box", "/work/my  proj ")), LOCAL, None, &[]);
        assert_eq!(w.label, "my  proj ", "no hidden character, so the name is kept as it is");
    }

    #[test]
    fn hidden_character_predicate_covers_its_ranges_only() {
        for cp in [0x200B, 0x200E, 0x200F, 0x202A, 0x202E, 0x2060, 0x2066, 0x2069, 0xFEFF, 0x0007, 0x007F] {
            assert!(is_hidden_or_control(char::from_u32(cp).unwrap()), "U+{cp:04X} is hidden");
        }
        for cp in [0x200C, 0x200D, 0x2029, 0x202F, 0x2065, 0x206A, 0x00A0, 'a' as u32] {
            assert!(!is_hidden_or_control(char::from_u32(cp).unwrap()), "U+{cp:04X} is shown");
        }
    }

    #[test]
    fn an_empty_path_is_unknown_by_label() {
        let w = workspace_for(Some(("", "")), LOCAL, None, &[]);
        assert_eq!(w.root, "");
        assert_eq!(w.label, "Unknown");
    }
}
