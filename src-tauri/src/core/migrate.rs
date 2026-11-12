//! One-time copy of the app's data folder from the old bundle identifier to the new one (lib.rs `run`, macOS).
//!
//! The folder holds the settings, UI state, window state and conversations. It is copied, never moved, so the old
//! folder is left as it was. The copy is built in a sibling `<new>.migrating` folder and renamed into place, so an
//! interrupted copy never leaves a half-filled new folder that would block the next attempt.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateOutcome {
    /// The legacy folder was copied into place.
    Copied,
    /// There is no legacy folder (or it is not a real folder), so nothing was done.
    NoLegacyFolder,
    /// The new folder already has content, so it was left as it is.
    NewFolderInUse,
}

const STAGING_SUFFIX: &str = ".migrating";

/// Copies the folder `legacy` to `new`, when `legacy` is a folder and `new` is missing or empty. A file already in
/// `new` is never overwritten, and `legacy` is never changed. Symlinks and special files are not copied. Modes are
/// kept, so the app's private files stay private. On an error, nothing is left at `new`.
pub fn migrate_legacy_dir(legacy: &Path, new: &Path) -> io::Result<MigrateOutcome> {
    let staging = staging_path(new);
    remove_leftover(&staging)?;

    if !is_real_dir(legacy)? {
        return Ok(MigrateOutcome::NoLegacyFolder);
    }
    if folder_has_entries(new)? {
        return Ok(MigrateOutcome::NewFolderInUse);
    }

    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    create_private_dir(&staging)?;
    let copied = copy_tree(legacy, &staging)
        .and_then(|()| fs::set_permissions(&staging, fs::metadata(legacy)?.permissions()));
    if let Err(e) = copied {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    // An empty folder at `new` gives way to the copy. Anything else would have counted as in use above.
    match fs::remove_dir(new) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    if let Err(e) = fs::rename(&staging, new) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(MigrateOutcome::Copied)
}

/// `<new>.migrating`, the sibling folder the copy is built in.
fn staging_path(new: &Path) -> PathBuf {
    let mut name = new.as_os_str().to_owned();
    name.push(STAGING_SUFFIX);
    PathBuf::from(name)
}

/// Removes a `.migrating` folder left by an interrupted copy. Nothing in it is trusted.
fn remove_leftover(staging: &Path) -> io::Result<()> {
    match fs::symlink_metadata(staging) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(staging),
        Ok(_) => fs::remove_file(staging),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// True for a folder that is not a symlink. A missing path is false.
fn is_real_dir(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.is_dir()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// True when `path` is a folder with at least one entry. A missing folder is empty. A path that is not a folder is an
/// error, and nothing is touched.
fn folder_has_entries(path: &Path) -> io::Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Copies the children of `src` into `dst`, which exists. Folders are created private and given their source mode once
/// their contents are copied, so a read-only folder still takes its contents.
fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            create_private_dir(&to)?;
            copy_tree(&from, &to)?;
            fs::set_permissions(&to, fs::metadata(&from)?.permissions())?;
        } else if file_type.is_file() {
            copy_file(&from, &to)?;
        }
        // Symlinks and special files (sockets, pipes) are skipped on purpose.
    }
    Ok(())
}

/// Copies one file. It is created new, so an existing file is never overwritten, and it takes the source's mode.
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut input = File::open(from)?;
    let permissions = input.metadata()?.permissions();
    let mut output = create_new_file(to)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    fs::set_permissions(to, permissions)
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(unix)]
fn create_new_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    options.open(path)
}

#[cfg(not(unix))]
fn create_new_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::test_support::TempDir;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn write_file(path: &Path, content: &str, mode: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn names_in(dir: &Path) -> Vec<String> {
        fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    /// A legacy folder shaped like the real one: settings, UI state, and conversations with an attachment.
    fn legacy_folder(root: &Path) -> PathBuf {
        let legacy = root.join("dev.agentterm.app");
        write_file(&legacy.join("settings.json"), r#"{"version":1}"#, 0o600);
        write_file(&legacy.join("ui-state.json"), r#"{"font_size":14}"#, 0o600);
        write_file(&legacy.join("conversations").join("index.json"), r#"{"conversations":[]}"#, 0o600);
        write_file(&legacy.join("conversations").join("attachments").join("a.bin"), "bytes", 0o644);
        set_mode(&legacy, 0o700);
        set_mode(&legacy.join("conversations"), 0o700);
        set_mode(&legacy.join("conversations").join("attachments"), 0o750);
        legacy
    }

    #[test]
    fn copies_the_tree_and_keeps_the_modes() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        let new = root.path().join("dev.watt.terminal");

        assert_eq!(migrate_legacy_dir(&legacy, &new).unwrap(), MigrateOutcome::Copied);

        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"version":1}"#);
        assert_eq!(fs::read_to_string(new.join("ui-state.json")).unwrap(), r#"{"font_size":14}"#);
        let attachment = new.join("conversations").join("attachments").join("a.bin");
        assert_eq!(fs::read_to_string(attachment).unwrap(), "bytes");
        assert_eq!(mode_of(&new), 0o700);
        assert_eq!(mode_of(&new.join("settings.json")), 0o600);
        assert_eq!(mode_of(&new.join("conversations")), 0o700);
        assert_eq!(mode_of(&new.join("conversations").join("attachments")), 0o750);
        assert_eq!(mode_of(&new.join("conversations").join("attachments").join("a.bin")), 0o644);
        assert!(!staging_path(&new).exists(), "the staging folder is renamed away");
    }

    #[test]
    fn the_legacy_folder_is_left_untouched() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        let new = root.path().join("dev.watt.terminal");

        migrate_legacy_dir(&legacy, &new).unwrap();

        assert_eq!(fs::read_to_string(legacy.join("settings.json")).unwrap(), r#"{"version":1}"#);
        assert_eq!(mode_of(&legacy.join("settings.json")), 0o600);
        let attachment = legacy.join("conversations").join("attachments").join("a.bin");
        assert_eq!(fs::read_to_string(attachment).unwrap(), "bytes");
        let mut names = names_in(&legacy);
        names.sort();
        assert_eq!(names, vec!["conversations", "settings.json", "ui-state.json"]);
    }

    #[test]
    fn a_missing_legacy_folder_is_a_no_op() {
        let root = TempDir::new();
        let new = root.path().join("dev.watt.terminal");

        let outcome = migrate_legacy_dir(&root.path().join("dev.agentterm.app"), &new).unwrap();

        assert_eq!(outcome, MigrateOutcome::NoLegacyFolder);
        assert!(!new.exists());
        assert!(!staging_path(&new).exists());
    }

    #[test]
    fn a_non_empty_new_folder_is_never_overwritten() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        let new = root.path().join("dev.watt.terminal");
        write_file(&new.join("settings.json"), r#"{"version":"new"}"#, 0o600);

        let outcome = migrate_legacy_dir(&legacy, &new).unwrap();

        assert_eq!(outcome, MigrateOutcome::NewFolderInUse);
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"version":"new"}"#);
        assert_eq!(names_in(&new), vec!["settings.json".to_string()], "nothing from the old folder is added");
        assert!(!staging_path(&new).exists());
    }

    #[test]
    fn an_empty_new_folder_gets_the_copy() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        let new = root.path().join("dev.watt.terminal");
        fs::create_dir_all(&new).unwrap();

        assert_eq!(migrate_legacy_dir(&legacy, &new).unwrap(), MigrateOutcome::Copied);
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"version":1}"#);
    }

    #[test]
    fn a_leftover_staging_folder_is_removed_before_the_copy() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        let new = root.path().join("dev.watt.terminal");
        write_file(&staging_path(&new).join("half-copied.json"), "junk", 0o600);

        assert_eq!(migrate_legacy_dir(&legacy, &new).unwrap(), MigrateOutcome::Copied);

        assert!(!new.join("half-copied.json").exists());
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"version":1}"#);
        assert!(!staging_path(&new).exists());
    }

    #[test]
    fn symlinks_are_skipped_and_their_targets_are_not_touched() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        let outside = root.path().join("outside.json");
        write_file(&outside, "outside", 0o600);
        symlink(&outside, legacy.join("link.json")).unwrap();
        let new = root.path().join("dev.watt.terminal");

        migrate_legacy_dir(&legacy, &new).unwrap();

        assert!(fs::symlink_metadata(new.join("link.json")).is_err(), "the link is not copied");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"version":1}"#);
    }

    #[test]
    fn a_failed_copy_leaves_no_new_folder() {
        let root = TempDir::new();
        let legacy = legacy_folder(root.path());
        write_file(&legacy.join("unreadable.json"), "x", 0o000);
        let new = root.path().join("dev.watt.terminal");

        assert!(migrate_legacy_dir(&legacy, &new).is_err());

        assert!(!new.exists(), "a failed copy must not leave a new folder that blocks the next try");
        assert!(!staging_path(&new).exists());
        assert_eq!(fs::read_to_string(legacy.join("settings.json")).unwrap(), r#"{"version":1}"#);
    }
}
