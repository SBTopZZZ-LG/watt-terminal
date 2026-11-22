//! The program to start for a stdio server. On Windows a bare command name is looked up on Watt's own PATH, in PATH
//! order, the way a shell would find it (`npx` finds `npx.exe` or `npx.cmd`, whichever comes first in its directory).
//! Node's launchers keep a `.cmd` fallback for when nothing is found. Everything else is started as written.
//! On unix the lookup is `resolve_on_path`, over the PATH the child is started with (client.rs).

#[cfg(unix)]
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The extensions tried in each PATH directory for a bare name on Windows, in this order.
const WINDOWS_EXTENSIONS: &[&str] = &["exe", "com", "cmd", "bat"];

/// The Node launchers that are `.cmd` files on Windows. They get `.cmd` only when the PATH search finds nothing.
const WINDOWS_CMD_LAUNCHERS: &[&str] = &["npx", "npm", "pnpm", "yarn"];

/// The command to start for a configured `command`. On Windows a bare name (no path, no extension) becomes the full path
/// of the first matching file on the PATH. An explicit extension or path is returned as it is.
pub fn resolve_launch(command: &str, windows: bool) -> String {
    let dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
    resolve_with(command, windows, &dirs, |path| path.is_file())
}

/// `resolve_launch` with the PATH directories and the file check passed in, so the tests need no real PATH.
fn resolve_with(command: &str, windows: bool, dirs: &[PathBuf], is_file: impl Fn(&Path) -> bool) -> String {
    if !windows || !is_bare(command) {
        return command.to_string();
    }
    if let Some(found) = find_on_path(command, dirs, is_file) {
        return found;
    }
    let is_launcher = WINDOWS_CMD_LAUNCHERS.iter().any(|name| command.eq_ignore_ascii_case(name));
    if is_launcher { format!("{command}.cmd") } else { command.to_string() }
}

/// A bare name has no path separator and no dot, so no extension and no `.\` or `./` prefix. Empty is not a name.
fn is_bare(command: &str) -> bool {
    !command.is_empty() && !command.contains(['/', '\\', '.'])
}

/// The first `command.ext` that is a file, going through the PATH directories in order and the extensions in order
/// within each. Empty PATH entries are skipped, since they would mean the current directory.
fn find_on_path(command: &str, dirs: &[PathBuf], is_file: impl Fn(&Path) -> bool) -> Option<String> {
    dirs.iter()
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| WINDOWS_EXTENSIONS.iter().map(move |ext| dir.join(format!("{command}.{ext}"))))
        .find(|candidate| is_file(candidate.as_path()))
        .map(|path| path.to_string_lossy().into_owned())
}

/// The program to start on unix: a bare name is looked up on `path`, the PATH the child is started with. The first
/// directory holding an executable file of that name wins. Not found, or a name with a `/`, gives the name as written,
/// so the spawn fails with the same NotFound the lookup would have given.
#[cfg(unix)]
pub fn resolve_on_path(command: &str, path: &OsStr) -> String {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    find_executable(command, &dirs, is_executable_file).unwrap_or_else(|| command.to_string())
}

/// The first `dir/command` that is executable, in PATH order. Empty PATH entries are skipped, as on Windows: they would
/// mean the current directory.
#[cfg(unix)]
fn find_executable(command: &str, dirs: &[PathBuf], is_executable: impl Fn(&Path) -> bool) -> Option<String> {
    if command.is_empty() || command.contains('/') {
        return None;
    }
    dirs.iter()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate.as_path()))
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const NODE_DIR: &str = r"C:\Users\me\nvm\.nodejs";
    const PROGRAM_FILES_NODE: &str = r"C:\Program Files\nodejs";

    fn at(dir: &str, name: &str) -> String {
        Path::new(dir).join(name).to_string_lossy().into_owned()
    }

    fn dirs(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// A fake file system: the listed paths are files. Compared case-insensitively, as on Windows.
    fn fake_files(files: &[String]) -> impl Fn(&Path) -> bool + '_ {
        move |path| files.iter().any(|file| file.eq_ignore_ascii_case(&path.to_string_lossy()))
    }

    fn resolve_fake(command: &str, windows: bool, list: &[&str], files: &[String]) -> String {
        resolve_with(command, windows, &dirs(list), fake_files(files))
    }

    #[test]
    fn exe_is_preferred_over_cmd_in_the_same_dir() {
        let files = [at(NODE_DIR, "npx.cmd"), at(NODE_DIR, "npx.exe")];
        assert_eq!(resolve_fake("npx", true, &[NODE_DIR], &files), at(NODE_DIR, "npx.exe"));
    }

    #[test]
    fn earlier_dir_wins_over_later_dir() {
        let files = [at(NODE_DIR, "npx.cmd"), at(PROGRAM_FILES_NODE, "npx.exe")];
        assert_eq!(resolve_fake("npx", true, &[NODE_DIR, PROGRAM_FILES_NODE], &files), at(NODE_DIR, "npx.cmd"));
    }

    #[test]
    fn only_cmd_on_path_resolves_to_that_cmd() {
        let files = [at(PROGRAM_FILES_NODE, "npx.cmd")];
        assert_eq!(resolve_fake("npx", true, &[PROGRAM_FILES_NODE], &files), at(PROGRAM_FILES_NODE, "npx.cmd"));
    }

    #[test]
    fn com_and_bat_are_found_after_exe_and_cmd() {
        let files = [at(NODE_DIR, "tool.bat"), at(NODE_DIR, "tool.com")];
        assert_eq!(resolve_fake("tool", true, &[NODE_DIR], &files), at(NODE_DIR, "tool.com"));
    }

    #[test]
    fn nothing_found_gives_launchers_the_cmd_fallback() {
        for name in ["npx", "npm", "pnpm", "yarn", "NPX"] {
            assert_eq!(resolve_fake(name, true, &[NODE_DIR], &[]), format!("{name}.cmd"), "{name}");
        }
    }

    #[test]
    fn nothing_found_leaves_other_names_unchanged() {
        for name in ["uvx", "node", "python", "npx-helper", "my npx"] {
            assert_eq!(resolve_fake(name, true, &[NODE_DIR], &[]), name, "{name}");
        }
    }

    #[test]
    fn other_names_resolve_like_launchers() {
        let files = [at(NODE_DIR, "uvx.exe")];
        assert_eq!(resolve_fake("uvx", true, &[NODE_DIR], &files), at(NODE_DIR, "uvx.exe"));
        assert_eq!(resolve_fake("NPX", true, &[NODE_DIR], &[at(NODE_DIR, "npx.exe")]), at(NODE_DIR, "NPX.exe"));
    }

    #[test]
    fn explicit_extensions_and_paths_are_untouched_even_when_a_file_exists() {
        let files = [at(NODE_DIR, "npx.exe"), at(NODE_DIR, "npx.cmd")];
        for command in [
            "npx.cmd",
            "npx.exe",
            r"C:\Program Files\nodejs\npx",
            "C:/nodejs/npx",
            r".\npx",
            "./npx",
            "",
        ] {
            assert_eq!(resolve_fake(command, true, &[NODE_DIR], &files), command, "{command:?}");
        }
    }

    #[test]
    fn nothing_changes_off_windows() {
        let files = [at(NODE_DIR, "npx.exe")];
        for name in ["npx", "npm", "pnpm", "yarn", "uvx"] {
            assert_eq!(resolve_fake(name, false, &[NODE_DIR], &files), name, "{name}");
        }
    }

    #[test]
    fn empty_path_entries_and_missing_dirs_are_skipped() {
        // The bare "npx.exe" is what an empty entry would find if it were searched as the current directory.
        let files = [String::from("npx.exe"), at(NODE_DIR, "npx.exe")];
        assert_eq!(resolve_fake("npx", true, &["", r"C:\does\not\exist", NODE_DIR], &files), at(NODE_DIR, "npx.exe"));
    }

    #[test]
    fn a_directory_named_like_the_candidate_is_not_a_hit() {
        let root = std::env::temp_dir().join(format!(
            "watt-launch-test-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let dir_a = root.join("a");
        let dir_b = root.join("b");
        fs::create_dir_all(dir_a.join("npx.exe")).expect("create test dir");
        fs::create_dir_all(&dir_b).expect("create test dir");
        fs::write(dir_b.join("npx.cmd"), b"").expect("create test file");

        let list = [dir_a.clone(), dir_b.clone()];
        let resolved = resolve_with("npx", true, &list, |path| path.is_file());
        let _ = fs::remove_dir_all(&root);

        assert_eq!(resolved, dir_b.join("npx.cmd").to_string_lossy());
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A fresh temp dir with one file per `(name, mode)`. The caller removes it.
        fn make_dir(tag: &str, files: &[(&str, u32)]) -> PathBuf {
            let root = std::env::temp_dir().join(format!(
                "watt-launch-unix-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
            ));
            fs::create_dir_all(&root).expect("create test dir");
            for (name, mode) in files {
                let file = root.join(name);
                fs::write(&file, b"#!/bin/sh\n").expect("create test file");
                fs::set_permissions(&file, fs::Permissions::from_mode(*mode)).expect("chmod test file");
            }
            root
        }

        #[test]
        fn the_first_executable_in_path_order_wins_and_a_plain_file_does_not() {
            let a = make_dir("a", &[("tool", 0o644)]);
            let b = make_dir("b", &[("tool", 0o755)]);
            let c = make_dir("c", &[("tool", 0o755)]);
            let path = std::env::join_paths([&a, &b, &c]).expect("join");
            let resolved = resolve_on_path("tool", &path);
            for dir in [&a, &b, &c] {
                let _ = fs::remove_dir_all(dir);
            }
            assert_eq!(resolved, b.join("tool").to_string_lossy());
        }

        #[test]
        fn a_name_not_found_or_with_a_slash_is_returned_as_written() {
            let a = make_dir("none", &[("other", 0o755)]);
            let path = std::env::join_paths([&a]).expect("join");
            assert_eq!(resolve_on_path("uvx", &path), "uvx");
            assert_eq!(resolve_on_path("./tool", &path), "./tool");
            assert_eq!(resolve_on_path("", &path), "");
            let _ = fs::remove_dir_all(&a);
        }

        #[test]
        fn empty_path_entries_are_skipped() {
            // A bare "tool" would be found first if the empty entry were searched as the current directory.
            let executable = |p: &Path| p == Path::new("tool") || p == Path::new("/opt/bin/tool");
            let dirs = [PathBuf::new(), PathBuf::from("/opt/bin")];
            assert_eq!(find_executable("tool", &dirs, executable).as_deref(), Some("/opt/bin/tool"));
        }
    }
}
