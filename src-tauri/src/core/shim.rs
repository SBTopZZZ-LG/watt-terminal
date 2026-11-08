//! Shell integration files. They are embedded in the binary and written to a fresh per-session
//! directory, which is used as ZDOTDIR. The user's own dotfiles are never touched.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const FILES: &[(&str, &str)] = &[
    (".zshenv", include_str!("shim/.zshenv")),
    (".zprofile", include_str!("shim/.zprofile")),
    (".zshrc", include_str!("shim/.zshrc")),
    (".zlogin", include_str!("shim/.zlogin")),
    ("integration.zsh", include_str!("shim/integration.zsh")),
];

/// `<tmp>/agent-term-<pid>-<session>`. Any leftover directory with that name is removed first.
pub fn dir_for(session_id: &str) -> PathBuf {
    let safe: String = session_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    std::env::temp_dir().join(format!("agent-term-{}-{}", std::process::id(), safe))
}

/// Creates the directory with mode 0700 (zsh sources files from it, so other users must not be able to write there) and writes the files.
pub fn write_to(dir: &Path) -> io::Result<()> {
    let _ = fs::remove_dir_all(dir);
    fs::DirBuilder::new().mode(0o700).create(dir)?;
    for (name, body) in FILES {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dir.join(name))?;
        f.write_all(body.as_bytes())?;
    }
    Ok(())
}
