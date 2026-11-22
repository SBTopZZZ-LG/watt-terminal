//! The skill folders: which folders are scanned, in what order, and which are refused. A setting is `~` or `~/...` (the home
//! folder), an absolute path ("your folder"), or a relative path resolved against the session's project root ("this
//! project"). Folders are normalised before they are compared, so one folder listed twice is scanned once. A folder inside
//! Watt's own folders, or in a credential folder, is refused: the skill tool reads every file in a skill folder.
//!
//! Pure functions over text and the file system only. The places (home, config, project) come from the caller.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use super::scan::valid_name;
use super::{DEFAULT_FOLDERS, Root, SkillsSettings, Source};
use crate::core::fs_tools::{blocked_reason, is_within, strip_verbatim};

/// Most folders the settings may hold.
pub const MAX_FOLDERS: usize = 64;
/// Longest folder setting, in characters.
pub const MAX_FOLDER_CHARS: usize = 1024;
/// The folder under Watt's config folder that is always scanned first. It is not a setting and is never refused.
const WATT_SKILLS: &str = "skills";
/// Windows paths compare without case. Other systems compare exactly.
const CASE_INSENSITIVE: bool = cfg!(windows);
const GUARDED_REASON: &str = "Watt's own settings, secrets and conversations";

/// The places a folder setting is resolved against and checked against.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// Watt's config folder. Its `skills` folder is the Watt root; the rest of it is refused.
    pub config: Option<PathBuf>,
    /// The home folder: `~` in a setting.
    pub home: Option<PathBuf>,
    /// The session's project root: relative settings resolve against it. None without a local shell.
    pub project: Option<PathBuf>,
    /// Watt's guarded folders (data folder, secrets file's folder). Refused like the config folder.
    pub guarded: Vec<PathBuf>,
}

/// What became of one configured folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderStatus {
    /// Scanned, and the folder exists.
    Found,
    /// Scanned once it appears. Nothing there yet.
    NotFound,
    /// Refused. The warning says why.
    Blocked,
    /// A relative folder without a project root, or `~` without a home folder: there is nothing to resolve it against.
    Unresolved,
    /// The same folder as an earlier row: scanned once, there.
    Duplicate,
}

/// One configured folder, as the settings editor shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderEntry {
    /// The setting as written (or the default).
    pub folder: String,
    /// The folder it names, when it can be worked out.
    pub path: Option<String>,
    pub status: FolderStatus,
}

/// The folders to scan, in priority order, and what became of each configured folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// The Watt folder first, then the configured folders that are scanned, each folder once.
    pub roots: Vec<Root>,
    /// One entry per configured folder, in the order configured.
    pub folders: Vec<FolderEntry>,
    /// One warning per refused folder.
    pub warnings: Vec<String>,
}

/// The folders the settings give: the saved ones, or the defaults when none are saved.
pub fn configured(settings: &SkillsSettings) -> Vec<String> {
    settings.folders.clone().unwrap_or_else(|| DEFAULT_FOLDERS.iter().map(|f| f.to_string()).collect())
}

/// The layout for `configured` folders under `places`.
pub fn layout(places: &Places, configured: &[String]) -> Layout {
    let mut out = Layout::default();
    let mut seen = HashSet::new();
    if let Some(config) = &places.config {
        let dir = config.join(WATT_SKILLS);
        seen.insert(key(&dir));
        out.roots.push(Root { dir, source: Source::Watt });
    }
    for folder in configured {
        let entry = match resolve(folder, places) {
            Target::Unresolved => FolderEntry { folder: folder.clone(), path: None, status: FolderStatus::Unresolved },
            Target::Dir(dir, source) => {
                let path = Some(dir.display().to_string());
                if let Some(why) = refused(&dir, places) {
                    out.warnings.push(format!("{}: skill folder refused, {why}", dir.display()));
                    FolderEntry { folder: folder.clone(), path, status: FolderStatus::Blocked }
                } else if seen.insert(key(&dir)) {
                    let status = if dir.is_dir() { FolderStatus::Found } else { FolderStatus::NotFound };
                    out.roots.push(Root { dir, source });
                    FolderEntry { folder: folder.clone(), path, status }
                } else {
                    FolderEntry { folder: folder.clone(), path, status: FolderStatus::Duplicate }
                }
            }
        };
        out.folders.push(entry);
    }
    out
}

/// Checks the folders a user saves. Each problem is one line naming the folder. A folder that cannot be resolved here (a
/// relative one with no project root) is checked by its text only; its real path is checked when it is scanned.
pub fn refusals(places: &Places, folders: &[String]) -> Vec<String> {
    folders
        .iter()
        .filter_map(|folder| match resolve(folder, places) {
            Target::Dir(dir, _) => refused(&dir, places).map(|why| format!("skill folder `{folder}` is refused: {why}")),
            Target::Unresolved => {
                blocked_reason(folder).map(|why| format!("skill folder `{folder}` is refused: the file tools never read {why}"))
            }
        })
        .collect()
}

/// The problems with the folder settings: count, length, and empty or NUL text. Each names the folder by its number.
pub fn validate(folders: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    if folders.len() > MAX_FOLDERS {
        problems.push(format!("skill folders: at most {MAX_FOLDERS} folders"));
    }
    for (i, folder) in folders.iter().enumerate() {
        let n = i + 1;
        if folder.trim().is_empty() {
            problems.push(format!("skill folder {n} is empty"));
        } else if folder.chars().count() > MAX_FOLDER_CHARS {
            problems.push(format!("skill folder {n} is longer than {MAX_FOLDER_CHARS} characters"));
        }
        if folder.contains('\0') {
            problems.push(format!("skill folder {n} has a NUL character"));
        }
    }
    problems
}

/// The problems with the disabled skill names: each must be a valid skill name. Each names the entry by its number.
pub fn validate_disabled(disabled: &[String]) -> Vec<String> {
    disabled
        .iter()
        .enumerate()
        .filter(|(_, name)| !valid_name(name))
        .map(|(i, _)| format!("skills: disabled entry {} is not a valid skill name", i + 1))
        .collect()
}

/// Where a folder setting points.
enum Target {
    Dir(PathBuf, Source),
    Unresolved,
}

/// The folder a setting names. `~` is the home folder; an absolute path is "your folder"; a relative path is "this project"
/// and needs a project root. `~user` and drive-relative paths (`C:x`) are not understood and are unresolved.
fn resolve(folder: &str, places: &Places) -> Target {
    let text = folder.trim().replace('\\', "/");
    if text == "~" || text.starts_with("~/") {
        return match &places.home {
            Some(home) => Target::Dir(clean(&home.join(text.trim_start_matches('~').trim_start_matches('/'))), Source::User),
            None => Target::Unresolved,
        };
    }
    if text.starts_with('~') {
        return Target::Unresolved;
    }
    let path = Path::new(&text);
    if path.is_absolute() {
        return Target::Dir(clean(path), Source::User);
    }
    if path.components().next().is_some_and(|c| matches!(c, Component::Prefix(_) | Component::RootDir)) {
        return Target::Unresolved;
    }
    match &places.project {
        Some(project) => Target::Dir(clean(&project.join(path)), Source::Project),
        None => Target::Unresolved,
    }
}

/// The path without `.` parts and without trailing separators.
fn clean(path: &Path) -> PathBuf {
    path.components().filter(|c| !matches!(c, Component::CurDir)).collect()
}

/// Why a folder must not be scanned, or None. Checked on the real path (links followed) when the folder exists, and on
/// the path as written when it does not. Watt's `skills` folder is the one exception under the config folder.
fn refused(dir: &Path, places: &Places) -> Option<String> {
    let real = real_text(dir);
    if let Some(reason) = blocked_reason(&real) {
        return Some(format!("the file tools never read {reason}"));
    }
    if let Some(config) = &places.config
        && is_within(&real, &real_text(&config.join(WATT_SKILLS)), CASE_INSENSITIVE)
    {
        return None;
    }
    let guarded = places.config.iter().chain(places.guarded.iter());
    for guard in guarded {
        let guard = real_text(guard);
        if !guard.is_empty() && is_within(&real, &guard, CASE_INSENSITIVE) {
            return Some(GUARDED_REASON.to_string());
        }
    }
    None
}

/// The text of a folder's real path (links followed, `\\?\` dropped) when it exists, else the path as written.
fn real_text(path: &Path) -> String {
    match fs::canonicalize(path) {
        Ok(real) => strip_verbatim(&real.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The dedupe key of a folder: its real path, with `/` separators, no trailing separator, and lower case on Windows.
fn key(path: &Path) -> String {
    let text = real_text(path).replace('\\', "/");
    let text = text.trim_end_matches('/');
    if CASE_INSENSITIVE { text.to_lowercase() } else { text.to_string() }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn places(home: Option<&Path>, project: Option<&Path>) -> Places {
        Places { config: None, home: home.map(Path::to_path_buf), project: project.map(Path::to_path_buf), guarded: Vec::new() }
    }

    fn defaults() -> Vec<String> {
        configured(&SkillsSettings::default())
    }

    #[test]
    fn no_saved_folders_means_the_four_defaults() {
        assert_eq!(defaults(), ["~/.agents/skills", "~/.claude/skills", ".agents/skills", ".claude/skills"]);
        let saved = SkillsSettings { folders: Some(vec!["/x".into()]), disabled: Vec::new() };
        assert_eq!(configured(&saved), ["/x"]);
        let empty = SkillsSettings { folders: Some(Vec::new()), disabled: Vec::new() };
        assert!(configured(&empty).is_empty(), "a saved empty list means no user folders");
    }

    #[test]
    fn the_watt_folder_is_first_then_the_configured_folders_in_order() {
        let tmp = TempDir::new();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        let cfg = Places { config: Some(tmp.path().join("cfg")), ..places(Some(&home), Some(&project)) };
        let layout = layout(&cfg, &defaults());
        let shown: Vec<(PathBuf, Source)> = layout.roots.into_iter().map(|r| (r.dir, r.source)).collect();
        assert_eq!(
            shown,
            [
                (tmp.path().join("cfg").join("skills"), Source::Watt),
                (home.join(".agents").join("skills"), Source::User),
                (home.join(".claude").join("skills"), Source::User),
                (project.join(".agents").join("skills"), Source::Project),
                (project.join(".claude").join("skills"), Source::Project),
            ]
        );
    }

    #[test]
    fn a_project_that_is_the_home_folder_does_not_list_the_same_folder_twice() {
        let tmp = TempDir::new();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agents").join("skills")).unwrap();
        let layout = layout(&places(Some(&home), Some(&home)), &defaults());
        assert_eq!(layout.roots.len(), 2, "{:?}", layout.roots);
        assert_eq!(layout.folders[2].status, FolderStatus::Duplicate);
        assert_eq!(layout.folders[0].status, FolderStatus::Found);
        assert_eq!(layout.folders[1].status, FolderStatus::NotFound);
    }

    #[test]
    fn slash_variants_trailing_separators_and_repeats_are_one_folder() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("skills");
        fs::create_dir_all(&dir).unwrap();
        let text = dir.display().to_string();
        let forward = text.replace('\\', "/");
        let folders = vec![text.clone(), format!("{forward}/"), forward.clone(), format!("{text}\\")];
        let layout = layout(&places(None, None), &folders);
        assert_eq!(layout.roots.len(), 1, "{:?}", layout.roots);
        assert_eq!(layout.folders[0].status, FolderStatus::Found);
        assert!(layout.folders[1..].iter().all(|f| f.status == FolderStatus::Duplicate), "{:?}", layout.folders);
    }

    #[test]
    fn relative_folders_without_a_project_and_tilde_without_home_are_unresolved() {
        let layout = layout(&places(None, None), &["~/.agents/skills".into(), ".claude/skills".into(), "~other/x".into()]);
        assert!(layout.roots.is_empty());
        assert!(layout.folders.iter().all(|f| f.status == FolderStatus::Unresolved && f.path.is_none()), "{:?}", layout.folders);
    }

    #[test]
    fn tilde_expands_to_home_and_absolute_and_relative_resolve_as_their_source_says() {
        let tmp = TempDir::new();
        let home = tmp.path().join("home");
        let project = tmp.path().join("proj");
        let folders = vec![
            "~".into(),
            "~/team/skills".into(),
            tmp.path().join("abs").display().to_string(),
            "shared/skills".into(),
        ];
        let layout = layout(&places(Some(&home), Some(&project)), &folders);
        let shown: Vec<(PathBuf, Source)> = layout.roots.iter().map(|r| (r.dir.clone(), r.source)).collect();
        assert_eq!(
            shown,
            [
                (home.clone(), Source::User),
                (home.join("team").join("skills"), Source::User),
                (tmp.path().join("abs"), Source::User),
                (project.join("shared").join("skills"), Source::Project),
            ]
        );
    }

    #[test]
    fn a_missing_folder_is_listed_as_not_found_and_kept() {
        let tmp = TempDir::new();
        let layout = layout(&places(None, None), &[tmp.path().join("later").display().to_string()]);
        assert_eq!(layout.folders[0].status, FolderStatus::NotFound);
        assert_eq!(layout.roots.len(), 1);
        assert!(layout.warnings.is_empty());
    }

    #[test]
    fn a_folder_under_watt_config_is_refused_except_its_skills_folder() {
        let tmp = TempDir::new();
        let cfg = tmp.path().join("cfg");
        let data = tmp.path().join("data");
        let p = Places {
            config: Some(cfg.clone()),
            guarded: vec![data.clone(), cfg.join("settings.json")],
            ..places(None, None)
        };
        let folders = vec![cfg.join("notes").display().to_string(), data.join("x").display().to_string(), cfg.join("skills").display().to_string()];
        let layout = layout(&p, &folders);
        assert_eq!(layout.folders[0].status, FolderStatus::Blocked);
        assert_eq!(layout.folders[1].status, FolderStatus::Blocked);
        assert_eq!(layout.folders[2].status, FolderStatus::Duplicate, "Watt's own skills folder is the Watt root");
        assert_eq!(layout.warnings.len(), 2, "{:?}", layout.warnings);
        assert!(layout.warnings[0].contains("Watt's own settings"), "{:?}", layout.warnings);
    }

    #[test]
    fn a_credential_folder_is_refused_with_a_warning() {
        let tmp = TempDir::new();
        let folders = vec![tmp.path().join(".ssh").display().to_string(), tmp.path().join(".aws").join("x").display().to_string()];
        let layout = layout(&places(None, None), &folders);
        assert!(layout.roots.is_empty());
        assert!(layout.folders.iter().all(|f| f.status == FolderStatus::Blocked), "{:?}", layout.folders);
        assert!(layout.warnings[0].contains("SSH, GnuPG or AWS credential folders"), "{:?}", layout.warnings);
    }

    #[test]
    fn refusals_name_the_folder_and_pass_good_ones() {
        let tmp = TempDir::new();
        let p = Places { config: Some(tmp.path().join("cfg")), ..places(None, None) };
        let bad = tmp.path().join("cfg").join("notes").display().to_string();
        let good = tmp.path().join("fine").display().to_string();
        let found = refusals(&p, &[good.clone(), bad.clone(), ".ssh/keys".into()]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains(&bad) && found[0].contains("Watt's own settings"), "{found:?}");
        assert!(found[1].contains(".ssh/keys") && found[1].contains("credential"), "{found:?}");
        assert!(refusals(&p, &[good]).is_empty());
    }

    #[test]
    fn validate_limits_count_length_and_text() {
        assert!(validate(&["/a".into()]).is_empty());
        assert!(validate(&["  ".into()])[0].contains("skill folder 1 is empty"));
        assert!(validate(&["/a\0b".into()])[0].contains("NUL"));
        assert!(validate(&["x".repeat(MAX_FOLDER_CHARS + 1)])[0].contains("longer than 1024"));
        assert!(validate(&vec!["/a".to_string(); MAX_FOLDERS]).is_empty());
        assert!(validate(&vec!["/a".to_string(); MAX_FOLDERS + 1])[0].contains("at most 64"));
    }

    #[test]
    fn validate_disabled_accepts_skill_names_only() {
        assert!(validate_disabled(&["pdf-tools".into()]).is_empty());
        assert_eq!(validate_disabled(&["ok".into(), "Bad Name".into()]), ["skills: disabled entry 2 is not a valid skill name"]);
    }

    #[test]
    #[cfg(windows)]
    fn windows_folders_differ_only_in_case_are_one_folder() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("skills");
        fs::create_dir_all(&dir).unwrap();
        let text = dir.display().to_string();
        let layout = layout(&places(None, None), &[text.clone(), text.to_uppercase()]);
        assert_eq!(layout.roots.len(), 1, "{:?}", layout.roots);
        assert_eq!(layout.folders[1].status, FolderStatus::Duplicate);
    }
}
