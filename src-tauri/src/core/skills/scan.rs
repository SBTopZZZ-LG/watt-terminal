//! Finds the skills under the scan roots: one level of `<root>/<skill-name>/SKILL.md`. Lenient throughout: a bad skill is
//! skipped with a warning, an unreadable root is skipped, and nothing here fails a turn. Symlinked skill folders are
//! followed (users link them in); each skill's `SKILL.md` must be a regular file once links are followed.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::frontmatter;
use super::{Root, Shadowed, Skill, SkillIndex, Source};

pub const SKILL_FILE: &str = "SKILL.md";
/// Most skills loaded across all roots. Once reached, the rest are not loaded and one warning says so.
pub const MAX_SKILLS: usize = 200;
/// A `SKILL.md` larger than this is not read at all.
pub const MAX_SKILL_FILE: u64 = 256 * 1024;
pub const MAX_NAME: usize = 64;
/// Descriptions are cut to this many characters.
pub const MAX_DESCRIPTION: usize = 1024;

/// Scans the roots in order and returns the skills found. The first root that has a name wins: a later skill of the same
/// name is listed as shadowed, with a warning. A skill folder reached twice (a link, or two roots over one folder) is
/// loaded once, silently.
pub fn scan(roots: &[Root]) -> SkillIndex {
    let mut index = SkillIndex { roots: roots.iter().map(|r| r.dir.clone()).collect(), ..SkillIndex::default() };
    let mut winners: HashMap<String, PathBuf> = HashMap::new();
    let mut loaded_dirs: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        let entries = match fs::read_dir(&root.dir) {
            Ok(entries) => entries,
            Err(e) => {
                // A missing root is the normal case; any other failure is worth one warning.
                if e.kind() != io::ErrorKind::NotFound {
                    index.warnings.push(format!("{}: cannot read the folder ({e})", root.dir.display()));
                }
                continue;
            }
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            if !dir.join(SKILL_FILE).is_file() {
                continue;
            }
            if index.skills.len() + index.shadowed.len() >= MAX_SKILLS {
                index.warnings.push(format!("more than {MAX_SKILLS} skills: the rest are not loaded"));
                return index;
            }
            match load(&dir, root.source) {
                Ok(skill) => {
                    if !loaded_dirs.insert(fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
                        continue;
                    }
                    if let Some(winner) = winners.get(&skill.name) {
                        index.warnings.push(format!(
                            "{} in {} is shadowed by the one in {}",
                            skill.name,
                            root.dir.display(),
                            winner.display()
                        ));
                        index.shadowed.push(Shadowed { skill, by: winner.clone() });
                    } else {
                        winners.insert(skill.name.clone(), root.dir.clone());
                        index.skills.push(skill);
                    }
                }
                Err(reason) => index.warnings.push(format!("{}: {reason}", dir.join(SKILL_FILE).display())),
            }
        }
    }
    index
}

/// Whether a skill name is 1 to 64 characters of lowercase ASCII letters, digits and hyphens.
pub fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The text of `<dir>/SKILL.md`. The error is the reason, for a warning or for the tool's answer. A file larger than
/// `MAX_SKILL_FILE` is refused, not cut.
pub fn read_skill_md(dir: &Path) -> Result<String, String> {
    let path = dir.join(SKILL_FILE);
    if !path.is_file() {
        return Err(format!("no {SKILL_FILE} file"));
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|file| file.take(MAX_SKILL_FILE + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read ({e})"))?;
    if bytes.len() as u64 > MAX_SKILL_FILE {
        return Err(format!("{SKILL_FILE} is larger than {MAX_SKILL_FILE} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{SKILL_FILE} is not UTF-8"))
}

/// One skill folder: the frontmatter must give a valid name equal to the folder name, and a description.
fn load(dir: &Path, source: Source) -> Result<Skill, String> {
    let text = read_skill_md(dir)?;
    let parsed = frontmatter::parse(&text)?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = parsed.text("name").map(str::trim).unwrap_or_default();
    if name.is_empty() {
        return Err("no name in the frontmatter".to_string());
    }
    if !valid_name(name) {
        return Err(format!("name `{name}` must be 1 to {MAX_NAME} lowercase letters, digits or hyphens"));
    }
    if name != folder {
        return Err(format!("name `{name}` must match the folder name `{folder}`"));
    }
    let description: String = parsed.text("description").unwrap_or_default().trim().chars().take(MAX_DESCRIPTION).collect();
    if description.is_empty() {
        return Err("no description in the frontmatter".to_string());
    }
    let disabled = parsed
        .flag("disable-model-invocation")
        .or_else(|| parsed.flag("disable_model_invocation"))
        .unwrap_or(false);
    Ok(Skill { name: name.to_string(), description, source, dir: dir.to_path_buf(), model_invocable: !disabled, enabled: true })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn write_skill(root: &Path, folder: &str, body: &str) {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKILL_FILE), body).unwrap();
    }

    fn good(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\nSteps for {name}.\n")
    }

    fn root(dir: &Path, source: Source) -> Root {
        Root { dir: dir.to_path_buf(), source }
    }

    fn names(index: &SkillIndex) -> Vec<&str> {
        index.skills.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn valid_names_are_lowercase_digits_and_hyphens_up_to_64() {
        assert!(valid_name("pdf-tools2"));
        assert!(!valid_name(""));
        assert!(!valid_name("Upper"));
        assert!(!valid_name("with_underscore"));
        assert!(!valid_name("a b"));
        assert!(valid_name(&"a".repeat(64)));
        assert!(!valid_name(&"a".repeat(65)));
    }

    #[test]
    fn missing_roots_are_silent_and_still_listed() {
        let tmp = TempDir::new();
        let index = scan(&[root(&tmp.path().join("absent"), Source::User)]);
        assert!(index.skills.is_empty());
        assert!(index.warnings.is_empty(), "{:?}", index.warnings);
        assert_eq!(index.roots, vec![tmp.path().join("absent")]);
    }

    #[test]
    fn loads_skills_in_name_order_and_skips_folders_without_skill_md() {
        let tmp = TempDir::new();
        write_skill(tmp.path(), "beta", &good("beta", "Second"));
        write_skill(tmp.path(), "alpha", &good("alpha", "First"));
        fs::create_dir_all(tmp.path().join("not-a-skill")).unwrap();
        let index = scan(&[root(tmp.path(), Source::Watt)]);
        assert_eq!(names(&index), ["alpha", "beta"]);
        assert_eq!(index.skills[0].description, "First");
        assert_eq!(index.skills[0].source, Source::Watt);
        assert!(index.skills[0].model_invocable);
        assert!(index.warnings.is_empty(), "{:?}", index.warnings);
    }

    #[test]
    fn the_first_root_wins_and_a_same_named_skill_is_shadowed_with_a_warning() {
        let user = TempDir::new();
        let project = TempDir::new();
        write_skill(user.path(), "shared", &good("shared", "From the user"));
        write_skill(project.path(), "shared", &good("shared", "From the repo"));
        let index = scan(&[root(user.path(), Source::User), root(project.path(), Source::Project)]);
        assert_eq!(index.skills.len(), 1);
        assert_eq!(index.skills[0].description, "From the user");
        assert_eq!(index.skills[0].source, Source::User);
        assert_eq!(index.shadowed.len(), 1);
        assert_eq!(index.shadowed[0].skill.description, "From the repo");
        assert_eq!(index.shadowed[0].by, user.path().to_path_buf());
        assert_eq!(index.warnings.len(), 1, "{:?}", index.warnings);
        assert_eq!(
            index.warnings[0],
            format!("shared in {} is shadowed by the one in {}", project.path().display(), user.path().display())
        );
    }

    #[test]
    fn the_same_skill_folder_reached_through_two_roots_is_loaded_once_silently() {
        let real = TempDir::new();
        let link_root = TempDir::new();
        write_skill(real.path(), "same", &good("same", "Once"));
        fs::create_dir_all(link_root.path()).unwrap();
        if !make_dir_link(&real.path().join("same"), &link_root.path().join("same")) {
            return;
        }
        let index = scan(&[root(real.path(), Source::User), root(link_root.path(), Source::Project)]);
        assert_eq!(names(&index), ["same"]);
        assert!(index.shadowed.is_empty(), "{:?}", index.shadowed);
        assert!(index.warnings.is_empty(), "{:?}", index.warnings);
    }

    #[test]
    fn a_bad_skill_is_skipped_with_a_warning_and_the_rest_load() {
        let tmp = TempDir::new();
        write_skill(tmp.path(), "wrong-name", &good("other", "Name differs from the folder"));
        write_skill(tmp.path(), "no-description", "---\nname: no-description\n---\nBody\n");
        write_skill(tmp.path(), "no-frontmatter", "Just text\n");
        write_skill(tmp.path(), "good", &good("good", "Fine"));
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert_eq!(names(&index), ["good"]);
        assert_eq!(index.warnings.len(), 3, "{:?}", index.warnings);
        assert!(index.warnings.iter().any(|w| w.contains("must match the folder name `wrong-name`")), "{:?}", index.warnings);
        assert!(index.warnings.iter().any(|w| w.contains("no description")), "{:?}", index.warnings);
        assert!(index.warnings.iter().any(|w| w.contains("must start with a --- line")), "{:?}", index.warnings);
    }

    #[test]
    fn disable_model_invocation_keeps_the_skill_listed_but_not_invocable() {
        let tmp = TempDir::new();
        let body = "---\nname: manual\ndescription: Only when asked\ndisable-model-invocation: true\n---\nBody\n";
        write_skill(tmp.path(), "manual", body);
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert_eq!(names(&index), ["manual"]);
        assert!(!index.skills[0].model_invocable);
    }

    #[test]
    fn the_description_is_cut_to_the_limit() {
        let tmp = TempDir::new();
        write_skill(tmp.path(), "long", &good("long", &"x".repeat(MAX_DESCRIPTION + 50)));
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert_eq!(index.skills[0].description.chars().count(), MAX_DESCRIPTION);
    }

    #[test]
    fn an_oversize_skill_md_is_skipped_with_a_warning() {
        let tmp = TempDir::new();
        let big = format!("---\nname: big\ndescription: Big\n---\n{}", "x".repeat(MAX_SKILL_FILE as usize));
        write_skill(tmp.path(), "big", &big);
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert!(index.skills.is_empty());
        assert!(index.warnings.iter().any(|w| w.contains("larger than")), "{:?}", index.warnings);
    }

    #[test]
    fn the_skill_count_is_capped_with_one_warning() {
        let tmp = TempDir::new();
        for i in 0..=MAX_SKILLS {
            let name = format!("s{i:03}");
            write_skill(tmp.path(), &name, &good(&name, "Filler"));
        }
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert_eq!(index.skills.len(), MAX_SKILLS);
        assert_eq!(index.warnings.len(), 1, "{:?}", index.warnings);
        assert!(index.warnings[0].contains("the rest are not loaded"));
    }

    #[test]
    fn a_plain_file_in_a_root_is_not_a_skill() {
        let tmp = TempDir::new();
        fs::create_dir_all(tmp.path()).unwrap();
        fs::write(tmp.path().join("notes.md"), good("notes", "Not a folder")).unwrap();
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert!(index.skills.is_empty());
        assert!(index.warnings.is_empty());
    }

    /// A symlinked skill folder is followed. Creating links needs a privilege on some Windows setups: then the test
    /// returns early instead of failing.
    #[test]
    fn a_symlinked_skill_folder_is_followed() {
        let tmp = TempDir::new();
        let real = TempDir::new();
        write_skill(real.path(), "linked", &good("linked", "Through a link"));
        fs::create_dir_all(tmp.path()).unwrap();
        if !make_dir_link(&real.path().join("linked"), &tmp.path().join("linked")) {
            return;
        }
        let index = scan(&[root(tmp.path(), Source::User)]);
        assert_eq!(names(&index), ["linked"]);
    }

    #[cfg(unix)]
    fn make_dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
}
