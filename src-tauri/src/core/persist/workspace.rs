//! Which project a conversation belongs to: the project root of the shell's working directory when the conversation
//! is first saved. Derived once and stored, never re-derived.
//!
//! The root is the nearest ancestor of the working directory (the directory itself included) that has a `.git`
//! entry (a directory, or a file for worktrees and submodules). The walk is plain filesystem checks: no process is
//! spawned. It only runs for this machine and an absolute path. For any other host the root is the working
//! directory as the shell reported it. Without a working directory the workspace is unknown.

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
/// `home` is the user's home directory, used only to label the root "~".
pub fn workspace_for(cwd: Option<(&str, &str)>, local_host: &str, home: Option<&Path>) -> Workspace {
    let Some((host, path)) = cwd else {
        return Workspace::unknown();
    };
    let local = host.is_empty() || host == "localhost" || host == local_host;
    let root = if local { project_root(path) } else { path.to_string() };
    let label = label_for(&root, local, home);
    Workspace { host: host.to_string(), root, label }
}

/// The nearest ancestor of `path` (itself included) that contains a `.git` entry, or `path` when there is none.
/// Only an absolute path is searched.
fn project_root(path: &str) -> String {
    let p = Path::new(path);
    if !p.is_absolute() {
        return path.to_string();
    }
    for dir in p.ancestors() {
        if dir.join(".git").exists() {
            return dir.to_string_lossy().into_owned();
        }
    }
    path.to_string()
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
    match root_path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => root.to_string(),
    }
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
        let w = workspace_for(None, LOCAL, None);
        assert_eq!(w, Workspace { host: String::new(), root: String::new(), label: "Unknown".into() });
    }

    #[test]
    fn repository_root_is_the_nearest_ancestor_with_a_git_directory() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = dir(&repo.join("src").join("deep"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None);
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
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None);
        assert_eq!(w.root, s(&wt));
        assert_eq!(w.label, "wt");
    }

    #[test]
    fn the_working_directory_itself_can_be_the_root() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("here"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let w = workspace_for(Some(("", &s(&repo))), LOCAL, None);
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
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None);
        assert_eq!(w.root, s(&inner));
        assert_eq!(w.label, "inner");
    }

    #[test]
    fn no_repository_means_the_working_directory_is_the_root() {
        let tmp = TempDir::new();
        let cwd = dir(&tmp.path().join("plain").join("notes"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, None);
        assert_eq!(w.root, s(&cwd));
        assert_eq!(w.label, "notes");
    }

    #[test]
    fn home_is_labelled_tilde_and_a_project_under_home_is_not() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        let project = dir(&home.join("proj"));
        let w = workspace_for(Some(("", &s(&home))), LOCAL, Some(&home));
        assert_eq!(w.root, s(&home));
        assert_eq!(w.label, "~");
        let w = workspace_for(Some(("", &s(&project))), LOCAL, Some(&home));
        assert_eq!(w.root, s(&project));
        assert_eq!(w.label, "proj");
    }

    #[test]
    fn a_repository_at_home_is_labelled_tilde() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        fs::create_dir_all(home.join(".git")).unwrap();
        let cwd = dir(&home.join("code"));
        let w = workspace_for(Some(("", &s(&cwd))), LOCAL, Some(&home));
        assert_eq!(w.root, s(&home));
        assert_eq!(w.label, "~");
    }

    #[test]
    fn a_remote_host_keeps_its_path_and_never_walks_the_local_disk() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = dir(&repo.join("src"));
        let w = workspace_for(Some(("build-box", &s(&cwd))), LOCAL, None);
        assert_eq!(w.host, "build-box");
        assert_eq!(w.root, s(&cwd));
        assert_eq!(w.label, "src");
    }

    #[test]
    fn a_remote_home_path_is_not_labelled_tilde() {
        let tmp = TempDir::new();
        let home = dir(&tmp.path().join("home"));
        let w = workspace_for(Some(("build-box", &s(&home))), LOCAL, Some(&home));
        assert_eq!(w.label, "home");
    }

    #[test]
    fn localhost_and_the_local_host_name_count_as_this_machine() {
        let tmp = TempDir::new();
        let repo = dir(&tmp.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = dir(&repo.join("src"));
        for host in ["localhost", LOCAL] {
            let w = workspace_for(Some((host, &s(&cwd))), LOCAL, None);
            assert_eq!(w.root, s(&repo), "host {host}");
            assert_eq!(w.host, host);
        }
    }

    #[test]
    fn a_relative_path_is_not_searched_and_is_its_own_root() {
        let w = workspace_for(Some(("", "relative/dir")), LOCAL, None);
        assert_eq!(w.root, "relative/dir");
        assert_eq!(w.label, "dir");
    }

    #[test]
    fn filesystem_root_is_labelled_slash() {
        let w = workspace_for(Some(("build-box", "/")), LOCAL, None);
        assert_eq!(w.root, "/");
        assert_eq!(w.label, "/");
    }

    #[test]
    fn an_empty_path_is_unknown_by_label() {
        let w = workspace_for(Some(("", "")), LOCAL, None);
        assert_eq!(w.root, "");
        assert_eq!(w.label, "Unknown");
    }
}
