//! The attachment files of saved conversations: image bytes, kept outside the conversation file, at
//! `<data_dir>/attachments/<conversation-id>/<sha256>.<ext>` (`ext` is `png` or `jpg`). Files are content-addressed, so
//! writing the same image again is harmless. Every write goes through `write_private_atomic` (0600 files, 0700 directories).
//!
//! The `ConversationStore` owns one of these over its sibling `attachments` directory. It removes a conversation's
//! directory when the conversation is deleted, and sweeps the directories of conversations that are no longer listed.

use std::collections::HashSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::conversations::is_valid_id;
use crate::core::settings::write_private_atomic;

/// The lowercase hex SHA-256 of `bytes`: the content address of an attachment file.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// True for 64 lowercase hex digits, the only shape of a content address that becomes a file name.
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The attachment files under one root directory, one subdirectory per conversation id.
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    /// A store over `root`. The directories are created on the first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Writes one file. Writing the same content again replaces it with identical bytes.
    pub fn write(&self, conversation_id: &str, sha256: &str, ext: &str, bytes: &[u8]) -> io::Result<()> {
        write_private_atomic(&self.path_for(conversation_id, sha256, ext)?, bytes)
    }

    /// The bytes of one file. `None` when there is no such file.
    pub fn read(&self, conversation_id: &str, sha256: &str, ext: &str) -> io::Result<Option<Vec<u8>>> {
        match fs::read(self.path_for(conversation_id, sha256, ext)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Removes one conversation's directory. Nothing to remove is not an error.
    pub fn remove_conversation(&self, conversation_id: &str) -> io::Result<()> {
        if !is_valid_id(conversation_id) {
            return Err(invalid_name());
        }
        remove_dir_if_exists(&self.root.join(conversation_id))
    }

    /// Removes every conversation directory whose id is not in `keep`. Entries that are not conversation directories
    /// are left alone.
    pub fn remove_except(&self, keep: &HashSet<String>) -> io::Result<()> {
        for name in self.conversation_dirs()? {
            if !keep.contains(&name) {
                remove_dir_if_exists(&self.root.join(name))?;
            }
        }
        Ok(())
    }

    /// Removes every conversation directory. Entries that are not conversation directories are left alone.
    pub fn remove_all(&self) -> io::Result<()> {
        self.remove_except(&HashSet::new())
    }

    /// The names of the directories that are conversation ids.
    fn conversation_dirs(&self) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_valid_id(&name) && entry.file_type()?.is_dir() {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// The path of one file. Only a valid conversation id, a content address and `png` or `jpg` make a path, so no
    /// argument can point outside the root.
    fn path_for(&self, conversation_id: &str, sha256: &str, ext: &str) -> io::Result<PathBuf> {
        if !is_valid_id(conversation_id) || !is_sha256_hex(sha256) || !matches!(ext, "png" | "jpg") {
            return Err(invalid_name());
        }
        Ok(self.root.join(conversation_id).join(format!("{sha256}.{ext}")))
    }
}

fn invalid_name() -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, "the attachment name is not valid")
}

fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn id(n: u32) -> String {
        format!("ses_{n:032x}")
    }

    fn store(tmp: &TempDir) -> AttachmentStore {
        AttachmentStore::new(tmp.path().join("attachments"))
    }

    fn dir_names(root: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(root).map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_write_is_read_back_and_writing_the_same_content_again_keeps_one_file() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let bytes = b"\x89PNG\r\n\x1a\nimage";
        let sha = sha256_hex(bytes);
        assert_eq!(s.read(&id(1), &sha, "png").unwrap(), None, "nothing written yet");

        s.write(&id(1), &sha, "png", bytes).unwrap();
        s.write(&id(1), &sha, "png", bytes).unwrap();
        assert_eq!(s.read(&id(1), &sha, "png").unwrap().as_deref(), Some(&bytes[..]));
        assert_eq!(dir_names(&s.root().join(id(1))), vec![format!("{sha}.png")], "one content, one file");
    }

    #[test]
    fn files_are_0600_and_the_directories_0700() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let sha = sha256_hex(b"x");
        s.write(&id(2), &sha, "jpg", b"x").unwrap();
        let file = s.root().join(id(2)).join(format!("{sha}.jpg"));
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(s.root().join(id(2))).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(s.root()).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn names_that_are_not_ids_hashes_or_extensions_are_refused_and_touch_nothing() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let sha = sha256_hex(b"x");
        let bad_ids = ["", "../ses_0", "ses_../../etc", "ses_0000000000"];
        for bad in bad_ids {
            assert!(s.write(bad, &sha, "png", b"x").is_err(), "write {bad:?}");
            assert!(s.read(bad, &sha, "png").is_err(), "read {bad:?}");
            assert!(s.remove_conversation(bad).is_err(), "remove {bad:?}");
        }
        assert!(s.write(&id(3), "../../../../etc/passwd", "png", b"x").is_err());
        assert!(s.write(&id(3), &sha.to_uppercase(), "png", b"x").is_err());
        assert!(s.write(&id(3), &sha, "exe", b"x").is_err());
        assert!(s.write(&id(3), &sha, "png/../x", b"x").is_err());
        assert!(!s.root().exists(), "nothing was created");
    }

    #[test]
    fn a_missing_conversation_directory_is_not_an_error_to_remove() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.remove_conversation(&id(4)).unwrap();
        s.remove_all().unwrap();
        s.remove_except(&HashSet::new()).unwrap();
    }

    #[test]
    fn remove_except_and_remove_all_touch_only_conversation_directories() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let sha = sha256_hex(b"x");
        for n in [5, 6, 7] {
            s.write(&id(n), &sha, "png", b"x").unwrap();
        }
        fs::write(s.root().join("notes.txt"), b"not ours").unwrap();

        let keep: HashSet<String> = [id(6)].into_iter().collect();
        s.remove_except(&keep).unwrap();
        assert_eq!(dir_names(s.root()), vec!["notes.txt".to_string(), id(6)]);

        s.remove_all().unwrap();
        assert_eq!(dir_names(s.root()), vec!["notes.txt".to_string()]);
        assert_eq!(fs::read(s.root().join("notes.txt")).unwrap(), b"not ours");
    }

    #[test]
    fn remove_conversation_removes_only_that_directory() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let sha = sha256_hex(b"x");
        s.write(&id(8), &sha, "png", b"x").unwrap();
        s.write(&id(9), &sha, "png", b"x").unwrap();
        s.remove_conversation(&id(8)).unwrap();
        assert_eq!(dir_names(s.root()), vec![id(9)]);
    }

    #[test]
    fn sha256_hex_matches_the_known_digest_of_empty_input() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert!(is_sha256_hex(&sha256_hex(b"abc")));
        assert!(!is_sha256_hex("ABC"));
    }
}
