//! Shell injection builders: the loader line typed into a running shell, and the blob the loader reads and evals.
//! Nothing here touches a session or a PTY. The real-shell contract is tested in tests/inject_shell.rs.
//!
//! Handshake, from the caller's side:
//! 1. Type `loader_for(family, token)` into the shell (after Ctrl-C has cleared the line). The family is the shell
//!    the foreground program is (`foreground::shell_family`): POSIX (zsh, bash) or PowerShell.
//! 2. Wait for `ESC ] 633;Ready;t=<token>;sh=<zsh|bash|pwsh> BEL` (scanner Event::Ready). The POSIX loader only prints it when
//!    the shell is zsh, or bash >= 4.4 outside posix mode. The PowerShell loader prints it on PowerShell 5 or newer. Otherwise
//!    it prints `Unsupported shell` and nothing else.
//! 3. Write each line of `blob_lines(nonce, sh)`, then the line `END_MARKER`, each followed by a carriage return.
//!    `blob_lines` does not include END_MARKER; the writer adds it.
//! 4. Wait for Hello carrying the nonce. The loader evals the blob, which installs the hooks and prints Hello last.

use std::fmt;
use std::io;

use crate::core::foreground::ShellFamily;
use crate::core::scanner::READY_FIELD_CAP;

/// The loader reads lines until this one. The writer sends it after the blob. It never appears inside the blob.
pub const END_MARKER: &str = "__AT_END__";
/// Longest line the blob may carry. A longer line is refused, not split.
pub const MAX_LINE: usize = 480;

const NONCE_LEN: usize = 32;
const ZSH_SCRIPT: &str = include_str!("shim/integration.zsh");
const BASH_SCRIPT: &str = include_str!("shim/integration.bash");
const PS_SCRIPT: &str = include_str!("shim/integration.ps1");
/// The loader line ends with `: <tag>` and the bash history cleanup looks for the tag, so both are filled from this one.
const LOADER_TAG: &str = "LDRTAG";
const UNSET_LOADER_VARS: &str = "unset __at_v __at_l __at_ok";

/// The PowerShell loader (PowerShell 5.1 and 7). One line, ends with `# @TAG@`. It reads keys with ReadKey, so nothing is
/// echoed and PSReadLine is not involved, and it ends on the END_MARKER line. In the POC harness Ctrl-C did not stop it, so
/// an abort after Ready sends the marker instead (session.rs). Its variables live in its own script block.
const PS_LOADER: &str = r#"& { $t='@TOKEN@'; if ($PSVersionTable.PSVersion.Major -lt 5) { 'Unsupported shell'; return }; [Console]::Out.Write([string][char]27 + ']633;Ready;t=' + $t + ';sh=pwsh' + [string][char]7); $v=''; $l=''; while ($true) { $k=[Console]::ReadKey($true); if ($k.Key -eq 'Enter') { if ($l -eq '__AT_END__') { break }; $v += $l + "`n"; $l='' } else { $l += $k.KeyChar } }; . ([scriptblock]::Create($v)) } # @TAG@"#;

/// The POSIX loader: guards the shell, prints Ready, reads the blob up to END_MARKER and evals it. One line, leading space
/// (so bash's HISTCONTROL=ignorespace keeps it out of history), ends with `: @TAG@`. `@TOKEN@` and `@TAG@` are replaced.
const LOADER: &str = r#" __at_ok=; if [ -n "${ZSH_VERSION-}" ]; then __at_ok=zsh; elif [ -n "${BASH_VERSION-}" ]; then if [ "${BASH_VERSINFO[0]:-0}" -gt 4 ] || { [ "${BASH_VERSINFO[0]:-0}" -eq 4 ] && [ "${BASH_VERSINFO[1]:-0}" -ge 4 ]; }; then case ":${SHELLOPTS-}:" in *:posix:*) ;; *) __at_ok=bash;; esac; fi; fi; if [ -z "$__at_ok" ]; then echo 'Unsupported shell'; else stty -echo; printf '\033]633;Ready;t=@TOKEN@;sh=%s\007' "$__at_ok"; __at_v=; while IFS= read -r __at_l && [ "$__at_l" != __AT_END__ ]; do __at_v="$__at_v$__at_l"$'\n'; done; stty echo; eval "$__at_v"; fi; : @TAG@"#;

/// bash only, placed after the script. Deletes the loader line from history, but only when the newest entry holds the
/// loader tag, so it never removes another command. Under ignoreboth the loader is not recorded at all.
const BASH_HISTORY_CLEANUP: &str = r#"if [ -n "$BASH_VERSION" ]; then __at_h=$(history 1); __at_t=${__at_h#"${__at_h%%[![:space:]]*}"}; __at_n=${__at_t%%[!0-9]*}; case "$__at_h" in *@TAG@*) [ -n "$__at_n" ] && history -d "$__at_n";; esac; fi; unset __at_h __at_t __at_n"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectError {
    /// The shell name is neither `zsh` nor `bash`.
    UnknownShell(String),
    /// The nonce is not 32 hex digits.
    BadNonce,
    /// A line holds a byte outside printable ASCII. `line` is 1-based.
    NotPrintableAscii { line: usize },
    /// A line is longer than `MAX_LINE`.
    LineTooLong { line: usize, len: usize },
    /// A line equals `END_MARKER`, which would end the loader early.
    EndMarkerInBlob { line: usize },
}

impl fmt::Display for InjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InjectError::UnknownShell(sh) => write!(f, "unknown shell {sh:?} (need zsh, bash or pwsh)"),
            InjectError::BadNonce => write!(f, "nonce must be {NONCE_LEN} hex digits"),
            InjectError::NotPrintableAscii { line } => write!(f, "blob line {line} has a byte outside printable ASCII"),
            InjectError::LineTooLong { line, len } => write!(f, "blob line {line} is {len} bytes (limit {MAX_LINE})"),
            InjectError::EndMarkerInBlob { line } => write!(f, "blob line {line} is the end marker"),
        }
    }
}

impl std::error::Error for InjectError {}

/// A random 32-hex-digit value, for the loader token and for nonces. Same OS random source as the session nonce.
pub fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The one-line loader for `token`. Panics if the token is not 1 to 64 ASCII letters or digits (it is interpolated into
/// the shell line, so anything else must never get there).
pub fn loader_line(token: &str) -> String {
    assert!(valid_token(token), "loader token must be 1 to {READY_FIELD_CAP} ASCII letters or digits");
    LOADER.replace("@TOKEN@", token).replace("@TAG@", LOADER_TAG)
}

/// The one-line loader for `family` (see `loader_line` for the POSIX one). Same token rules.
pub fn loader_for(family: ShellFamily, token: &str) -> String {
    match family {
        ShellFamily::Posix => loader_line(token),
        ShellFamily::PowerShell => {
            assert!(valid_token(token), "loader token must be 1 to {READY_FIELD_CAP} ASCII letters or digits");
            PS_LOADER.replace("@TOKEN@", token).replace("@TAG@", LOADER_TAG)
        }
    }
}

/// The blob for `sh` (`zsh`, `bash` or `pwsh`), one string per line, without END_MARKER (the writer sends that after the
/// lines). Line 1 is `_watt_inject_nonce=<nonce>` (`$env:WATT_NONCE=` for pwsh). Then the integration script with comments
/// and blank lines removed, tabs turned into spaces and trailing spaces dropped. For bash, the history cleanup line. Last,
/// the loader cleanup (none for pwsh, whose loader variables die with its script block).
/// The nonce is only ever on line 1, and nothing in the blob is exported.
pub fn blob_lines(nonce: &str, sh: &str) -> Result<Vec<String>, InjectError> {
    if !valid_nonce(nonce) {
        return Err(InjectError::BadNonce);
    }
    let (script, bash) = match sh {
        "zsh" => (ZSH_SCRIPT, false),
        "bash" => (BASH_SCRIPT, true),
        "pwsh" => return powershell_blob(nonce),
        other => return Err(InjectError::UnknownShell(other.to_string())),
    };
    let mut lines = vec![format!("_watt_inject_nonce={nonce}")];
    lines.extend(script_lines(script));
    if bash {
        lines.push(BASH_HISTORY_CLEANUP.replace("@TAG@", LOADER_TAG));
    }
    lines.push(UNSET_LOADER_VARS.to_string());
    check(&lines)?;
    Ok(lines)
}

/// The PowerShell blob: the nonce goes in the environment (integration.ps1 reads it and removes it), then the script as it
/// is. No cleanup lines: the script's locals and the loader's variables do not outlive the script block.
fn powershell_blob(nonce: &str) -> Result<Vec<String>, InjectError> {
    let mut lines = vec![format!("$env:WATT_NONCE='{nonce}'")];
    lines.extend(script_lines(PS_SCRIPT));
    check(&lines)?;
    Ok(lines)
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= READY_FIELD_CAP && token.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == NONCE_LEN && nonce.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Drops full-line comments and blank lines, turns tabs into spaces and trailing spaces off. Code is kept as written.
fn script_lines(script: &str) -> Vec<String> {
    script
        .lines()
        .map(|l| l.replace('\t', " ").trim_end().to_string())
        .filter(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect()
}

fn check(lines: &[String]) -> Result<(), InjectError> {
    for (i, line) in lines.iter().enumerate() {
        let line_no = i + 1;
        if line == END_MARKER {
            return Err(InjectError::EndMarkerInBlob { line: line_no });
        }
        if !line.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(InjectError::NotPrintableAscii { line: line_no });
        }
        if line.len() > MAX_LINE {
            return Err(InjectError::LineTooLong { line: line_no, len: line.len() });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn loader_is_one_line_with_a_leading_space_the_token_and_the_guards() {
        let line = loader_line("RDY123");
        assert!(line.starts_with(' ') && !line.starts_with("  "), "{line:?}");
        assert!(!line.contains('\n') && !line.contains('\r'), "single line");
        assert!(line.contains("t=RDY123;"), "token in the Ready marker");
        assert!(line.contains("Unsupported shell"), "the refusal branch");
        assert!(line.contains("${ZSH_VERSION-}") && line.contains("${BASH_VERSION-}"), "set -u safe guards");
        assert!(line.ends_with(": LDRTAG"), "ends with the loader tag: {line:?}");
        assert!(!line.contains("@TOKEN@"), "placeholder replaced");
        assert!(line.contains(END_MARKER), "the loader reads up to the end marker");
    }

    #[test]
    #[should_panic(expected = "loader token must be")]
    fn loader_refuses_a_token_that_is_not_alphanumeric() {
        loader_line("a;b");
    }

    #[test]
    fn loader_refuses_an_empty_or_long_token() {
        assert!(!valid_token(""));
        assert!(valid_token(&"a".repeat(READY_FIELD_CAP)));
        assert!(!valid_token(&"a".repeat(READY_FIELD_CAP + 1)));
    }

    #[test]
    fn history_cleanup_uses_the_loader_tag() {
        let bash = blob_lines(NONCE, "bash").unwrap();
        assert!(bash.iter().any(|l| l.contains("case \"$__at_h\" in *LDRTAG*)")), "the cleanup matches the loader tag");
    }

    #[test]
    fn blob_for_each_shell_is_valid_and_has_the_nonce_only_on_line_one() {
        for sh in ["zsh", "bash"] {
            let lines = blob_lines(NONCE, sh).unwrap_or_else(|e| panic!("{sh}: {e}"));
            assert_eq!(lines[0], format!("_watt_inject_nonce={NONCE}"));
            assert!(lines[1..].iter().all(|l| !l.contains(NONCE)), "{sh}: nonce leaked past line 1");
            assert!(lines.iter().all(|l| l != END_MARKER), "{sh}: end marker inside the blob");
            assert!(lines.iter().all(|l| !l.contains("export") && !l.contains("-x ")), "{sh}: nothing exported");
            assert!(lines.iter().all(|l| l.len() <= MAX_LINE && !l.ends_with(' ') && !l.contains('\t')), "{sh}: line rules");
            assert_eq!(lines.last().map(String::as_str), Some(UNSET_LOADER_VARS), "{sh}: loader cleanup is last");
            assert!(lines.iter().all(|l| !l.trim().is_empty()), "{sh}: no blank lines");
            assert!(lines.iter().skip(1).all(|l| !l.trim_start().starts_with('#')), "{sh}: no comment lines");
        }
    }

    #[test]
    fn bash_gets_the_history_cleanup_before_the_loader_cleanup_and_zsh_does_not() {
        let bash = blob_lines(NONCE, "bash").unwrap();
        let n = bash.len();
        assert_eq!(bash[n - 2], BASH_HISTORY_CLEANUP.replace("@TAG@", LOADER_TAG));
        let zsh = blob_lines(NONCE, "zsh").unwrap();
        assert!(zsh.iter().all(|l| !l.contains("history 1")));
    }

    #[test]
    fn scripts_end_in_a_call_to_the_installer_and_have_no_top_level_return() {
        let bash = blob_lines(NONCE, "bash").unwrap();
        assert!(bash.iter().any(|l| l == "_at_install"), "bash installs through _at_install");
        // A `return` is only allowed indented (inside a function body). A column-0 return would run under eval.
        for sh in ["zsh", "bash"] {
            let lines = blob_lines(NONCE, sh).unwrap();
            assert!(lines.iter().all(|l| !l.starts_with("return")), "{sh}: top-level return");
        }
    }

    #[test]
    fn bad_nonce_and_unknown_shell_are_refused() {
        assert_eq!(blob_lines("abc", "zsh"), Err(InjectError::BadNonce));
        assert_eq!(blob_lines(&"g".repeat(32), "zsh"), Err(InjectError::BadNonce));
        assert_eq!(blob_lines("abc", "pwsh"), Err(InjectError::BadNonce));
        assert_eq!(blob_lines(NONCE, "fish"), Err(InjectError::UnknownShell("fish".into())));
    }

    #[test]
    fn posix_loader_for_is_the_posix_loader_line() {
        assert_eq!(loader_for(ShellFamily::Posix, "RDY123"), loader_line("RDY123"));
    }

    #[test]
    fn powershell_loader_is_one_ascii_line_with_the_token_and_the_tag() {
        for token in ["RDY123", NONCE] {
            let line = loader_for(ShellFamily::PowerShell, token);
            assert!(!line.contains('\n') && !line.contains('\r'), "single line");
            assert!(line.is_ascii() && line.len() <= MAX_LINE, "{} bytes", line.len());
            assert!(line.contains(&format!("$t='{token}';")), "token in the loader: {line:?}");
            assert!(line.contains("sh=pwsh") && line.contains("Unsupported shell"), "the Ready and refusal branches");
            assert!(line.contains("'__AT_END__'") && line.contains(END_MARKER), "reads up to the end marker");
            assert!(line.ends_with("# LDRTAG"), "ends with the loader tag: {line:?}");
            assert!(!line.contains("@TOKEN@") && !line.contains("@TAG@"), "placeholders replaced");
        }
    }

    #[test]
    #[should_panic(expected = "loader token must be")]
    fn powershell_loader_refuses_a_token_that_is_not_alphanumeric() {
        loader_for(ShellFamily::PowerShell, "a'b");
    }

    /// The shipped integration.ps1 must pass the blob rules (ASCII, line length, no END marker), and the PowerShell blob is
    /// the nonce line followed by the script, with no POSIX cleanup lines.
    #[test]
    fn powershell_blob_is_the_nonce_then_the_shipped_script() {
        let lines = blob_lines(NONCE, "pwsh").expect("the shipped integration.ps1 passes the blob rules");
        assert_eq!(lines[0], format!("$env:WATT_NONCE='{NONCE}'"));
        assert_eq!(&lines[1..], script_lines(PS_SCRIPT).as_slice());
        assert!(lines[1..].iter().all(|l| !l.contains(NONCE)), "nonce leaked past line 1");
        assert!(lines.iter().all(|l| l != END_MARKER), "end marker inside the blob");
        assert!(lines.iter().all(|l| l.is_ascii() && l.len() <= MAX_LINE && !l.ends_with(' ') && !l.contains('\t')), "line rules");
        assert!(lines.iter().all(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')), "no blank or comment lines");
        assert_ne!(lines.last().map(String::as_str), Some(UNSET_LOADER_VARS), "no POSIX loader cleanup");
        assert!(lines.iter().all(|l| !l.contains("_watt_inject_nonce")), "no POSIX nonce variable");
    }

    #[test]
    fn check_refuses_long_non_ascii_and_end_marker_lines() {
        let long = vec!["x".repeat(MAX_LINE + 1)];
        assert_eq!(check(&long), Err(InjectError::LineTooLong { line: 1, len: MAX_LINE + 1 }));
        let exact = vec!["x".repeat(MAX_LINE)];
        assert!(check(&exact).is_ok(), "the limit itself is allowed");
        assert_eq!(check(&["ok".into(), "caf\u{e9}".into()]), Err(InjectError::NotPrintableAscii { line: 2 }));
        assert_eq!(check(&["a\tb".into()]), Err(InjectError::NotPrintableAscii { line: 1 }));
        assert_eq!(check(&["ok".into(), END_MARKER.into()]), Err(InjectError::EndMarkerInBlob { line: 2 }));
    }

    #[test]
    fn script_lines_drop_comments_and_blanks_and_keep_code() {
        let got = script_lines("# note\n\n  # indented note\nfoo\tbar  \n\n  baz\n");
        assert_eq!(got, vec!["foo bar".to_string(), "  baz".to_string()]);
    }

    #[test]
    fn new_token_is_32_hex_digits_and_changes() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert!(valid_nonce(&a) && valid_nonce(&b));
        assert_ne!(a, b);
    }
}
