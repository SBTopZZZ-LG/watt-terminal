//! Where the file tools may read: a root folder on this machine, and the sensitive paths that are never read, even inside
//! it. Paths are checked after `..` and symlinks are resolved, so a link inside the root that points at a blocked file is
//! refused as well.
//!
//! The rules are pure functions on text, so they are tested the same on every platform. Only `Scope::resolve` and
//! `Scope::permits` touch the filesystem.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Windows paths compare without case. Other systems compare exactly.
const CASE_INSENSITIVE: bool = cfg!(windows);

/// Folder names whose contents are never read: SSH, GnuPG and AWS credentials.
const SECRET_FOLDERS: [&str; 3] = [".ssh", ".gnupg", ".aws"];
/// Private key file names, and extensions of key files.
const KEY_NAMES: [&str; 3] = ["id_rsa", "id_ed25519", "id_ecdsa"];
const KEY_EXTENSIONS: [&str; 2] = [".pem", ".key"];

/// The longest path echoed in an error message.
const ECHO_LIMIT: usize = 200;

/// The folder the file tools are confined to. Built once per turn (lib.rs) from the session's local directory.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The project root, or the local directory when there is no project root.
    root: PathBuf,
    /// The session's local directory. Relative paths resolve against it.
    cwd: PathBuf,
    /// Watt's config folder: its settings and secret store are never read.
    config_dir: Option<PathBuf>,
}

impl Scope {
    pub fn new(root: PathBuf, cwd: PathBuf, config_dir: Option<PathBuf>) -> Self {
        Scope { root, cwd, config_dir }
    }

    /// Resolves a path argument to a canonical path inside the root that is not blocked. `None` or an empty argument is the
    /// local directory. Relative arguments resolve against the local directory. The error is the text the model reads.
    pub fn resolve(&self, arg: Option<&str>) -> Result<PathBuf, String> {
        let arg = arg.map(str::trim).filter(|a| !a.is_empty());
        let shown = echo(arg.unwrap_or("."));
        let root = canonical(&self.root).map_err(|e| format!("cannot open the project folder ({e})"))?;
        let cwd = canonical(&self.cwd).map_err(|e| format!("cannot open the working directory ({e})"))?;
        let lexical = match arg {
            None => cwd,
            Some(a) if Path::new(a).is_absolute() => PathBuf::from(a),
            Some(a) => cwd.join(a),
        };
        // Refused before the file system is asked, so a missing blocked path gets the same answer as an existing one.
        if let Some(reason) = blocked_reason(&lexical.to_string_lossy()) {
            return Err(blocked_text(&shown, reason));
        }
        let target = match fs::canonicalize(&lexical) {
            Ok(target) => target,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(format!("`{shown}` does not exist")),
            Err(e) => return Err(format!("cannot access `{shown}` ({e})")),
        };
        self.check(&target, &root, &shown)?;
        Ok(target)
    }

    /// Whether a file found by a walk may be read. The walk's paths are under the root, but a link may lead out of it or
    /// into a blocked file, so the target is checked, not the path.
    pub fn permits(&self, entry: &Path) -> bool {
        let (Ok(target), Ok(root)) = (fs::canonicalize(entry), canonical(&self.root)) else {
            return false;
        };
        self.check(&target, &root, "").is_ok()
    }

    /// Whether a walk leaves this entry out: a blocked name, or Watt's config folder. It works on the walk's own (lexical)
    /// path, so a blocked folder is not entered at all.
    pub fn excluded(&self, entry: &Path) -> bool {
        blocked_reason(&entry.to_string_lossy()).is_some() || self.in_config_dir(entry)
    }

    /// The checks on a canonical target: the blocked names, the config folder, and the root.
    fn check(&self, target: &Path, root: &Path, shown: &str) -> Result<(), String> {
        let text = plain(target);
        if let Some(reason) = blocked_reason(&text) {
            return Err(blocked_text(shown, reason));
        }
        if self.in_config_dir(target) {
            return Err(blocked_text(shown, "Watt's own settings and secrets"));
        }
        if !is_within(&text, &plain(root), CASE_INSENSITIVE) {
            return Err(format!(
                "`{shown}` is outside the project folder ({}). The file tools read only inside it; use run_command for other paths.",
                plain(root)
            ));
        }
        Ok(())
    }

    fn in_config_dir(&self, path: &Path) -> bool {
        let Some(dir) = &self.config_dir else {
            return false;
        };
        let dir = canonical(dir).unwrap_or_else(|_| dir.clone());
        is_within(&plain(path), &plain(&dir), CASE_INSENSITIVE)
    }
}

/// Why a path is never read, judged by its text: a credential folder anywhere on it, or a key, `.netrc` or `.env` file as
/// its name. None when it is allowed. Either separator and any letter case are accepted.
pub fn blocked_reason(path: &str) -> Option<&'static str> {
    let normal = strip_verbatim(path).replace('\\', "/").to_ascii_lowercase();
    let parts: Vec<&str> = normal.split('/').filter(|p| !p.is_empty()).collect();
    let name = parts.last().copied().unwrap_or("");
    if parts.iter().any(|p| SECRET_FOLDERS.contains(p)) {
        return Some("SSH, GnuPG or AWS credential folders");
    }
    if name == ".env" || name.starts_with(".env.") {
        return Some(".env files");
    }
    if name == ".netrc" {
        return Some(".netrc files");
    }
    if KEY_NAMES.contains(&name) || KEY_EXTENSIONS.iter().any(|ext| name.ends_with(ext)) {
        return Some("private key files");
    }
    None
}

/// Whether `path` is `root` or inside it, comparing whole path segments (`C:/proj` does not contain `C:/project`). Either
/// separator works. `case_insensitive` folds case first.
pub fn is_within(path: &str, root: &str, case_insensitive: bool) -> bool {
    let fold = |text: &str| {
        let text = strip_verbatim(text).replace('\\', "/");
        let text = if case_insensitive { text.to_lowercase() } else { text };
        text.trim_end_matches('/').to_string()
    };
    let (path, root) = (fold(path), fold(root));
    if root.is_empty() {
        // The file system root ("/"): every absolute path is inside it.
        return path.starts_with('/');
    }
    path == root || path.starts_with(&format!("{root}/"))
}

/// Drops the `\\?\` prefix Windows canonical paths carry (`\\?\UNC\server\share` becomes `\\server\share`).
pub fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// A requested path, shortened for an error message.
pub fn echo(text: &str) -> String {
    text.chars().take(ECHO_LIMIT).collect()
}

fn blocked_text(shown: &str, reason: &str) -> String {
    format!("`{shown}` is blocked: the file tools never read {reason}.")
}

fn canonical(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

fn plain(path: &Path) -> String {
    strip_verbatim(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    #[test]
    fn strip_verbatim_drops_the_windows_prefixes() {
        assert_eq!(strip_verbatim(r"\\?\C:\proj"), r"C:\proj");
        assert_eq!(strip_verbatim(r"\\?\UNC\server\share\x"), r"\\server\share\x");
        assert_eq!(strip_verbatim(r"\\server\share"), r"\\server\share");
        assert_eq!(strip_verbatim("/home/me"), "/home/me");
    }

    #[test]
    fn is_within_compares_whole_segments_with_either_separator() {
        assert!(is_within("/home/me/proj/src", "/home/me/proj", false));
        assert!(is_within("/home/me/proj", "/home/me/proj/", false));
        assert!(!is_within("/home/me/project", "/home/me/proj", false), "a prefix is not a parent");
        assert!(!is_within("/home/me", "/home/me/proj", false));
        assert!(is_within(r"C:\proj\a.txt", "C:/proj", false));
        assert!(is_within("/anything", "/", false));
    }

    #[test]
    fn is_within_is_case_sensitive_unless_asked() {
        assert!(!is_within(r"C:\Proj\a", r"c:\proj", false));
        assert!(is_within(r"C:\Proj\a", r"c:\proj", true));
        assert!(is_within(r"\\?\C:\Users\Me\Proj\x", r"C:\users\me\proj", true), "verbatim prefix is stripped");
        assert!(!is_within(r"C:\Users\Me\Projx", r"C:\users\me\proj", true));
    }

    #[test]
    fn blocked_names_and_folders_are_refused_in_any_case() {
        assert!(blocked_reason(r"C:\Users\me\.ssh\config").is_some());
        assert!(blocked_reason("/home/me/.SSH").is_some(), "the folder itself");
        assert!(blocked_reason("/home/me/.gnupg/pubring.kbx").is_some());
        assert!(blocked_reason(r"C:\Users\me\.aws\credentials").is_some());
        assert!(blocked_reason("/home/me/.ssh/id_ed25519").is_some());
        assert!(blocked_reason("/proj/deploy/server.PEM").is_some());
        assert!(blocked_reason("/proj/tls/private.key").is_some());
        assert!(blocked_reason("/home/me/.netrc").is_some());
    }

    #[test]
    fn env_files_are_refused() {
        assert!(blocked_reason("/proj/.env").is_some());
        assert!(blocked_reason("/proj/app/.env.local").is_some());
        assert!(blocked_reason("/proj/.env.production").is_some());
    }

    #[test]
    fn ordinary_names_are_allowed() {
        for path in ["/proj/src/main.rs", "/proj/.envrc", "/proj/keyboard.rs", "/proj/docs/ssh-notes.md", "/proj/.github/ci.yml"] {
            assert_eq!(blocked_reason(path), None, "{path}");
        }
    }

    #[test]
    fn resolve_reads_inside_the_root_and_refuses_outside() {
        let tmp = TempDir::new();
        let root = tmp.path().join("proj");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("outside.txt"), "x").unwrap();
        let scope = Scope::new(root.clone(), root.clone(), None);

        assert!(scope.resolve(Some("src/main.rs")).is_ok());
        assert!(scope.resolve(Some("./src/../src/main.rs")).is_ok(), "`..` that stays inside is fine");
        assert!(scope.resolve(None).is_ok(), "no path is the local directory");
        let escape = scope.resolve(Some("../outside.txt")).unwrap_err();
        assert!(escape.contains("outside the project folder"), "{escape}");
        let absolute = scope.resolve(Some(&tmp.path().join("outside.txt").to_string_lossy())).unwrap_err();
        assert!(absolute.contains("outside the project folder"), "{absolute}");
        assert_eq!(scope.resolve(Some("nope.txt")).unwrap_err(), "`nope.txt` does not exist");
    }

    #[test]
    fn resolve_refuses_blocked_files_inside_the_root() {
        let tmp = TempDir::new();
        let root = tmp.path().join("proj");
        fs::create_dir_all(root.join(".ssh")).unwrap();
        fs::write(root.join(".ssh").join("id_rsa"), "key").unwrap();
        fs::write(root.join(".env"), "TOKEN=1").unwrap();
        let scope = Scope::new(root.clone(), root.clone(), None);

        let env = scope.resolve(Some(".env")).unwrap_err();
        assert_eq!(env, "`.env` is blocked: the file tools never read .env files.");
        assert!(scope.resolve(Some(".ssh/id_rsa")).unwrap_err().contains("credential folders"));
        assert!(scope.resolve(Some(".ssh")).unwrap_err().contains("blocked"));
    }

    #[test]
    fn the_config_folder_is_refused() {
        let tmp = TempDir::new();
        let root = tmp.path().join("home");
        let config = root.join("watt-config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("settings.json"), "{}").unwrap();
        let scope = Scope::new(root.clone(), root.clone(), Some(config.clone()));

        assert!(scope.resolve(Some("watt-config/settings.json")).unwrap_err().contains("Watt's own settings"));
        assert!(scope.resolve(Some("watt-config")).unwrap_err().contains("Watt's own settings"));
        // A walk passes canonical paths, so the check gets one too.
        assert!(scope.excluded(&fs::canonicalize(config.join("settings.json")).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_root_to_a_blocked_or_outside_file_is_refused() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new();
        let root = tmp.path().join("proj");
        fs::create_dir_all(root.join(".ssh")).unwrap();
        fs::write(root.join(".ssh").join("config"), "Host x").unwrap();
        fs::write(tmp.path().join("outside.txt"), "x").unwrap();
        symlink(root.join(".ssh").join("config"), root.join("innocent.txt")).unwrap();
        symlink(tmp.path().join("outside.txt"), root.join("link-out.txt")).unwrap();
        let scope = Scope::new(root.clone(), root.clone(), None);

        assert!(scope.resolve(Some("innocent.txt")).unwrap_err().contains("credential folders"));
        assert!(scope.resolve(Some("link-out.txt")).unwrap_err().contains("outside the project folder"));
        assert!(!scope.permits(&root.join("innocent.txt")));
        assert!(!scope.permits(&root.join("link-out.txt")));
    }

    #[cfg(windows)]
    #[test]
    fn a_symlink_inside_the_root_to_an_outside_file_is_refused_when_links_can_be_made() {
        let tmp = TempDir::new();
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        fs::write(tmp.path().join("outside.txt"), "x").unwrap();
        // Creating a symlink needs developer mode or admin rights; without them the check is skipped.
        if std::os::windows::fs::symlink_file(tmp.path().join("outside.txt"), root.join("link-out.txt")).is_err() {
            return;
        }
        let scope = Scope::new(root.clone(), root.clone(), None);
        assert!(scope.resolve(Some("link-out.txt")).unwrap_err().contains("outside the project folder"));
    }
}
