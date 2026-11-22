//! The PATH a login shell gives, for MCP stdio servers on macOS and Linux. An app started from the Dock, Finder or a
//! desktop launcher gets a minimal PATH, without Homebrew, nvm, volta, asdf or ~/.local/bin. Watt asks the user's login
//! shell for its PATH once per process and puts those entries in front of the inherited ones.
//!
//! The parser and the merge are pure and compile on every platform, so their tests run everywhere. The shell runner is
//! unix-only. Windows does not use this module (launch.rs and the process PATH cover it).

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::{OnceLock, mpsc};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

const BEGIN_MARKER: &str = "__WATT_PATH_BEGIN__";
const END_MARKER: &str = "__WATT_PATH_END__";

/// How long the login shell gets to print its PATH. A slower shell is killed and the app keeps the inherited PATH.
#[cfg(unix)]
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// The PATH entries a login shell printed. Only the text between the markers counts, since rc files may print banners
/// before or after. None when a marker is missing, the text is empty, contains NUL or a line break, or no absolute entry
/// is left. Relative and empty entries are dropped (POSIX `:` separated). CRLF line ends are tolerated.
pub fn parse_login_path(output: &str) -> Option<Vec<PathBuf>> {
    let start = output.find(BEGIN_MARKER)? + BEGIN_MARKER.len();
    let rest = &output[start..];
    let body = &rest[..rest.find(END_MARKER)?];
    if body.contains('\0') {
        return None;
    }
    let line = body.trim_matches(['\r', '\n']);
    if line.is_empty() || line.contains(['\r', '\n']) {
        return None;
    }
    let entries: Vec<PathBuf> = line.split(':').filter(|entry| entry.starts_with('/')).map(PathBuf::from).collect();
    (!entries.is_empty()).then_some(entries)
}

/// The PATH for a child: the login shell's entries first, in their order, then the inherited entries that are not
/// already there. Duplicates are compared as exact strings. None if the entries cannot be joined.
pub fn merge_path(login: &[PathBuf], inherited: &OsStr) -> Option<OsString> {
    let mut seen: HashSet<OsString> = HashSet::new();
    let mut merged: Vec<PathBuf> = Vec::new();
    for dir in login.iter().cloned().chain(std::env::split_paths(inherited)) {
        if seen.insert(dir.as_os_str().to_owned()) {
            merged.push(dir);
        }
    }
    std::env::join_paths(merged).ok()
}

/// The shell to ask: `$SHELL` when it is an absolute path to an existing file. Anything else gives None.
#[cfg(unix)]
fn usable_shell(value: Option<OsString>) -> Option<PathBuf> {
    let shell = PathBuf::from(value?);
    (shell.is_absolute() && shell.is_file()).then_some(shell)
}

/// Runs `shell -i -l -c` with the PATH script and parses what it prints. The shell is killed when it runs past
/// `timeout`. Its stdout is read on another thread, so a grandchild that keeps the pipe open cannot hold up the timeout.
#[cfg(unix)]
fn read_login_path(shell: &Path, timeout: Duration) -> Option<Vec<PathBuf>> {
    // printenv is an external program, so every shell prints the exported PATH colon-joined (not `echo $PATH`).
    let script = format!("printf '{BEGIN_MARKER}\\n'; printenv PATH; printf '{END_MARKER}\\n'");
    let mut child = Command::new(shell)
        .args(["-i", "-l", "-c"])
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = sender.send(out);
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    // The shell has exited, so its output is already in the pipe. The floor covers a reader that is still draining it.
    let wait = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(100));
    let out = receiver.recv_timeout(wait).ok()?;
    parse_login_path(&String::from_utf8(out).ok()?)
}

/// The login shell's PATH entries, read once per process and kept. None when they cannot be read; the cause is logged
/// once, without the PATH itself.
#[cfg(unix)]
fn login_path_entries() -> Option<&'static [PathBuf]> {
    static ENTRIES: OnceLock<Option<Vec<PathBuf>>> = OnceLock::new();
    ENTRIES
        .get_or_init(|| {
            let entries =
                usable_shell(std::env::var_os("SHELL")).and_then(|shell| read_login_path(&shell, SHELL_TIMEOUT));
            if entries.is_none() {
                eprintln!("watt: the login shell's PATH could not be read; MCP servers use Watt's own PATH");
            }
            entries
        })
        .as_deref()
}

/// The PATH a stdio server is started with on unix: the login shell's entries merged with `inherited`. None when the
/// login shell's PATH is unavailable, and the server keeps the inherited PATH. The first call waits for the shell (up to
/// `SHELL_TIMEOUT`), so this runs on a blocking thread.
#[cfg(unix)]
pub async fn child_path(inherited: OsString) -> Option<OsString> {
    tokio::task::spawn_blocking(move || login_path_entries().and_then(|login| merge_path(login, &inherited)))
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn joined(list: &[&str]) -> OsString {
        std::env::join_paths(list.iter().map(PathBuf::from)).expect("join")
    }

    fn split(path: &OsStr) -> Vec<PathBuf> {
        std::env::split_paths(path).collect()
    }

    #[test]
    fn the_entries_between_the_markers_are_parsed() {
        let out = format!("{BEGIN_MARKER}\n/opt/homebrew/bin:/usr/bin:/bin\n{END_MARKER}\n");
        assert_eq!(parse_login_path(&out), Some(paths(&["/opt/homebrew/bin", "/usr/bin", "/bin"])));
    }

    #[test]
    fn banners_before_and_after_the_markers_are_ignored() {
        let out = format!("Welcome, user!\nlast login: today\n{BEGIN_MARKER}\n/a:/b\n{END_MARKER}\nbye\n");
        assert_eq!(parse_login_path(&out), Some(paths(&["/a", "/b"])));
        let no_newline_before = format!("motd without newline{BEGIN_MARKER}\n/a\n{END_MARKER}");
        assert_eq!(parse_login_path(&no_newline_before), Some(paths(&["/a"])));
    }

    #[test]
    fn missing_markers_give_none() {
        assert_eq!(parse_login_path("/a:/b\n"), None);
        assert_eq!(parse_login_path(&format!("{BEGIN_MARKER}\n/a:/b\n")), None, "no end marker");
        assert_eq!(parse_login_path(&format!("\n/a:/b\n{END_MARKER}\n")), None, "no begin marker");
    }

    #[test]
    fn an_empty_result_gives_none() {
        assert_eq!(parse_login_path(&format!("{BEGIN_MARKER}\n\n{END_MARKER}\n")), None);
        assert_eq!(parse_login_path(&format!("{BEGIN_MARKER}{END_MARKER}")), None);
    }

    #[test]
    fn nul_and_line_breaks_inside_the_result_give_none() {
        assert_eq!(parse_login_path(&format!("{BEGIN_MARKER}\n/a\0:/b\n{END_MARKER}\n")), None);
        assert_eq!(parse_login_path(&format!("{BEGIN_MARKER}\n/a\n/b\n{END_MARKER}\n")), None);
    }

    #[test]
    fn relative_and_empty_entries_are_dropped() {
        let out = format!("{BEGIN_MARKER}\n/a::rel/bin:./x:/b:\n{END_MARKER}\n");
        assert_eq!(parse_login_path(&out), Some(paths(&["/a", "/b"])));
        assert_eq!(parse_login_path(&format!("{BEGIN_MARKER}\nrel:\n{END_MARKER}\n")), None);
    }

    #[test]
    fn crlf_line_ends_are_tolerated() {
        let out = format!("banner\r\n{BEGIN_MARKER}\r\n/a:/b\r\n{END_MARKER}\r\n");
        assert_eq!(parse_login_path(&out), Some(paths(&["/a", "/b"])));
    }

    #[test]
    fn login_entries_come_first_then_the_inherited_ones_not_already_there() {
        let login = paths(&["/opt/homebrew/bin", "/usr/bin"]);
        let merged = merge_path(&login, &joined(&["/usr/bin", "/bin"])).expect("join");
        assert_eq!(split(&merged), paths(&["/opt/homebrew/bin", "/usr/bin", "/bin"]));
    }

    #[test]
    fn duplicates_are_compared_as_exact_strings() {
        let merged = merge_path(&paths(&["/a/"]), &joined(&["/a", "/a/"])).expect("join");
        assert_eq!(split(&merged), paths(&["/a/", "/a"]));
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        /// A fresh temp dir holding one executable script with `body`. The dir is removed by the caller.
        fn script(tag: &str, body: &str) -> (PathBuf, PathBuf) {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("watt-login-path-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).expect("create test dir");
            let file = dir.join("shell");
            fs::write(&file, format!("#!/bin/sh\n{body}\n")).expect("write script");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod script");
            (dir, file)
        }

        #[test]
        fn the_runner_reads_the_path_between_the_markers_of_a_fake_shell() {
            let body = format!(
                "echo 'banner from rc'\nprintf '{BEGIN_MARKER}\\r\\n/opt/tools/bin:rel:/usr/bin\\r\\n{END_MARKER}\\r\\n'\necho trailer"
            );
            let (dir, file) = script("runs", &body);
            let entries = read_login_path(&file, Duration::from_secs(5));
            let _ = fs::remove_dir_all(&dir);
            assert_eq!(entries, Some(paths(&["/opt/tools/bin", "/usr/bin"])));
        }

        #[test]
        fn a_shell_that_runs_past_the_timeout_is_killed_quickly() {
            let (dir, file) = script("hangs", "exec sleep 30");
            let started = Instant::now();
            let entries = read_login_path(&file, Duration::from_millis(300));
            let elapsed = started.elapsed();
            let _ = fs::remove_dir_all(&dir);
            assert_eq!(entries, None);
            assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
        }

        #[test]
        fn a_shell_that_fails_gives_none() {
            let (dir, file) = script("fails", "exit 3");
            let entries = read_login_path(&file, Duration::from_secs(5));
            let _ = fs::remove_dir_all(&dir);
            assert_eq!(entries, None);
        }

        #[test]
        fn only_an_absolute_existing_shell_is_used() {
            assert_eq!(usable_shell(None), None, "unset");
            assert_eq!(usable_shell(Some(OsString::new())), None, "empty");
            assert_eq!(usable_shell(Some(OsString::from("zsh"))), None, "relative");
            assert_eq!(usable_shell(Some(OsString::from("/nonexistent/watt-shell"))), None, "missing");
            assert_eq!(usable_shell(Some(OsString::from("/bin/sh"))), Some(PathBuf::from("/bin/sh")));
        }
    }
}
