//! Project instructions: the workspace root's `AGENTS.md`, loaded fresh at the start of each turn.
//! Everything is lenient: a missing, unreadable, non-UTF-8 or oversize file never fails a turn, it
//! just means no project rules.

use std::fs;
use std::path::Path;

/// The project instructions file in the workspace root.
pub const FILE_NAME: &str = "AGENTS.md";
/// How much of the instructions is kept (longer files are truncated with a marker).
pub const MAX_BYTES: usize = 32 * 1024;
/// A file larger than this is skipped without being read.
pub const MAX_READ: u64 = 256 * 1024;

/// The project instructions for a workspace root: `<root>/AGENTS.md`, trimmed. None when the file is
/// missing, unreadable, not UTF-8, larger than `MAX_READ`, or empty after trimming. Longer than
/// `MAX_BYTES` is cut at a character boundary with a truncation marker.
pub fn load(root: &Path) -> Option<String> {
    let path = root.join(FILE_NAME);
    if fs::metadata(&path).is_ok_and(|m| !m.is_file() || m.len() > MAX_READ) {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    if bytes.len() as u64 > MAX_READ {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > MAX_BYTES {
        let mut end = MAX_BYTES;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        return Some(format!("{}\n\n(Project instructions truncated.)", &trimmed[..end]));
    }
    Some(trimmed)
}

/// The system block carrying the project rules. It goes right after the base prompt, before the
/// resume and location notes, and is never stored in the history.
pub fn note(rules: &str) -> String {
    format!(
        "Project instructions from the workspace root's AGENTS.md (follow them unless they conflict with the system rules above):\n\n{rules}"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn write(root: &Path, content: &[u8]) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join(FILE_NAME), content).unwrap();
    }

    #[test]
    fn missing_file_reads_as_none() {
        let tmp = TempDir::new();
        fs::create_dir_all(tmp.path()).unwrap();
        assert_eq!(load(tmp.path()), None);
    }

    #[test]
    fn empty_file_reads_as_none() {
        let tmp = TempDir::new();
        write(tmp.path(), b"   \n\n  ");
        assert_eq!(load(tmp.path()), None);
    }

    #[test]
    fn normal_file_is_trimmed() {
        let tmp = TempDir::new();
        write(tmp.path(), b"\n\n  Use tabs.\n\n");
        assert_eq!(load(tmp.path()), Some("Use tabs.".to_string()));
    }

    #[test]
    fn long_file_is_truncated_at_a_char_boundary_with_a_marker() {
        let tmp = TempDir::new();
        // Multi-byte tail so the cut must respect the character boundary.
        let body = format!("{}{}", "x".repeat(MAX_BYTES - 2), "éééé");
        write(tmp.path(), body.as_bytes());
        let rules = load(tmp.path()).expect("long file truncates, not vanishes");
        assert!(rules.ends_with("\n\n(Project instructions truncated.)"), "{rules:?}");
        let kept = rules.strip_suffix("\n\n(Project instructions truncated.)").unwrap();
        assert!(kept.len() <= MAX_BYTES, "kept {} bytes", kept.len());
        assert!(body.starts_with(kept), "the kept text is a prefix of the file");
    }

    #[test]
    fn non_utf8_file_reads_as_none() {
        let tmp = TempDir::new();
        write(tmp.path(), &[0xff, 0xfe, b'x', b'\n']);
        assert_eq!(load(tmp.path()), None);
    }

    #[test]
    fn oversize_file_is_skipped() {
        let tmp = TempDir::new();
        write(tmp.path(), &vec![b'x'; MAX_READ as usize + 1]);
        assert_eq!(load(tmp.path()), None);
    }

    #[test]
    fn note_labels_the_source_and_defers_to_the_system_rules() {
        let text = note("Use tabs.");
        assert!(text.contains("AGENTS.md"), "{text}");
        assert!(text.contains("Use tabs."), "{text}");
        assert!(text.contains("unless they conflict with the system rules"), "{text}");
    }
}
