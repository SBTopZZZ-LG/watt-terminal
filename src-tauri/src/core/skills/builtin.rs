//! The built-in `watt` skill: a cheatsheet about Watt itself, compiled into the app from the files in `builtin/`. It is not on
//! disk, the user cannot switch it off or set its approval, and a folder skill of the same name is ignored (see
//! `reserve_builtin_name` in mod.rs). Its files are served by `tool::file_text`; `state` is the live snapshot (see `state.rs`).

use std::path::PathBuf;

use super::frontmatter::{self, Parsed};
use super::{Skill, SkillLevel, Source};

pub const NAME: &str = "watt";
/// The live snapshot file of the skill: `file: "state"`.
pub const STATE_FILE: &str = "state";
/// The skill's folder as it is shown: the skill has no folder on disk.
pub const DIR: &str = "<built in>";

const SKILL_MD: &str = include_str!("builtin/SKILL.md");
/// The other files of the skill, by their name in the skill's folder. Read with the skill tool's `file` argument.
pub const FILES: [(&str, &str); 2] = [
    ("ui.md", include_str!("builtin/ui.md")),
    ("troubleshooting.md", include_str!("builtin/troubleshooting.md")),
];

/// The built-in skill as a skill index entry: always listed, always on, always without a card.
pub fn skill() -> Skill {
    Skill {
        name: NAME.to_string(),
        description: description(),
        source: Source::Builtin,
        dir: PathBuf::from(DIR),
        model_invocable: true,
        enabled: true,
        level: SkillLevel::Auto,
    }
}

/// The instructions of the built-in skill: `SKILL.md` without its frontmatter.
pub fn body() -> String {
    parsed().body
}

fn description() -> String {
    parsed().text("description").unwrap_or_default().trim().to_string()
}

/// The embedded `SKILL.md`, parsed. The text is part of the app, and a test checks that it parses.
fn parsed() -> Parsed {
    frontmatter::parse(SKILL_MD).expect("the built-in SKILL.md has a valid frontmatter block")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_skill_md_parses_and_names_the_skill() {
        let parsed = frontmatter::parse(SKILL_MD).expect("parses");
        assert_eq!(parsed.text("name"), Some(NAME));
        assert!(!description().is_empty() && description().chars().count() <= 250, "{}", description());
        assert!(!body().is_empty());
    }

    #[test]
    fn the_built_in_skill_is_on_always_at_auto_and_has_no_folder() {
        let skill = skill();
        assert_eq!(skill.name, "watt");
        assert_eq!(skill.source, Source::Builtin);
        assert!(skill.model_invocable && skill.enabled);
        assert_eq!(skill.level, SkillLevel::Auto);
        assert_eq!(skill.dir, PathBuf::from(DIR));
    }

    #[test]
    fn the_embedded_files_are_not_empty_and_have_unique_names() {
        for (name, text) in FILES {
            assert!(!text.trim().is_empty(), "{name} is empty");
            assert_ne!(name, STATE_FILE);
        }
        assert_ne!(FILES[0].0, FILES[1].0);
    }
}
