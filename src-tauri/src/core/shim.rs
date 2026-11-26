//! Shell integration files. They are embedded in the binary and written to a fresh per-session
//! directory, which is used as ZDOTDIR. The user's own dotfiles are never touched.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const FILES: &[(&str, &str)] = &[
    (".zshenv", include_str!("shim/.zshenv")),
    (".zprofile", include_str!("shim/.zprofile")),
    (".zshrc", include_str!("shim/.zshrc")),
    (".zlogin", include_str!("shim/.zlogin")),
    ("integration.zsh", include_str!("shim/integration.zsh")),
    ("integration.ps1", include_str!("shim/integration.ps1")),
];

/// `<tmp>/watt-terminal-<pid>-<name>`, where `<name>` encodes `id` injectively: ASCII letters, digits and '-' stay as they
/// are, every other byte becomes '_' and two hex digits ('_' itself is "_5f"). So distinct ids never share a directory
/// ("a.b" and "a_b" differ). Any leftover directory with that name is removed first (by `write_to`).
pub fn dir_for(id: &str) -> PathBuf {
    let mut safe = String::with_capacity(id.len());
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' {
            safe.push(b as char);
        } else {
            safe.push_str(&format!("_{b:02x}"));
        }
    }
    std::env::temp_dir().join(format!("watt-terminal-{}-{}", std::process::id(), safe))
}

/// Creates the directory with mode 0700 (zsh sources files from it, so other users must not be able to write there) and writes the files.
pub fn write_to(dir: &Path) -> io::Result<()> {
    let _ = fs::remove_dir_all(dir);
    create_private_dir(dir)?;
    for (name, body) in FILES {
        let mut f = create_private_file(&dir.join(name))?;
        f.write_all(body.as_bytes())?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_names_are_injective() {
        let ids = ["a.b", "a_b", "a_b_", "a_2eb", "a-b", "ab", "a b", "a/b", "a\\b", "", "_", "é", "a_5f", "a__b"];
        let dirs: Vec<PathBuf> = ids.iter().map(|id| dir_for(id)).collect();
        for (i, a) in dirs.iter().enumerate() {
            for (j, b) in dirs.iter().enumerate() {
                assert!(i == j || a != b, "ids {:?} and {:?} share {}", ids[i], ids[j], a.display());
            }
        }
    }

    #[test]
    fn dir_is_under_temp_dir_and_named_with_pid() {
        let dir = dir_for("7");
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, format!("watt-terminal-{}-7", std::process::id()));
    }
}
