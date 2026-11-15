//! Environment settings. The app is named Watt, so its settings are `WATT_*`. The old `AGENT_TERM_*` names still work,
//! so a shell set up with them keeps working. A `WATT_*` value that is set and not empty wins over the old name.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

/// The value of the setting `name`, or of its legacy name `legacy` when `name` is unset or empty. `None` when neither
/// has a non-empty value.
pub fn env_setting(name: &str, legacy: &str) -> Option<OsString> {
    setting_from(|key| env::var_os(key), name, legacy)
}

/// `env_setting` over any lookup, so tests need not change the process environment.
fn setting_from(lookup: impl Fn(&str) -> Option<OsString>, name: &str, legacy: &str) -> Option<OsString> {
    [name, legacy].into_iter().find_map(|key| lookup(key).filter(|v| !v.is_empty()))
}

/// The user's home directory: `HOME` when set and not empty, else `USERPROFILE` (Windows usually has no `HOME`). `None`
/// when neither has a non-empty value. On macOS `HOME` is always set, so this is `HOME` there.
pub fn home_dir() -> Option<PathBuf> {
    home_from(env::var_os("HOME"), env::var_os("USERPROFILE"))
}

/// `home_dir` over the two values, so tests need not change the process environment.
fn home_from(home: Option<OsString>, userprofile: Option<OsString>) -> Option<PathBuf> {
    [home, userprofile].into_iter().flatten().find(|v| !v.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn the_new_name_wins_when_both_are_set() {
        let got = setting_from(lookup(&[("WATT_X", "new"), ("AGENT_TERM_X", "old")]), "WATT_X", "AGENT_TERM_X");
        assert_eq!(got, Some(OsString::from("new")));
    }

    #[test]
    fn the_legacy_name_is_used_when_the_new_one_is_unset() {
        let got = setting_from(lookup(&[("AGENT_TERM_X", "old")]), "WATT_X", "AGENT_TERM_X");
        assert_eq!(got, Some(OsString::from("old")));
    }

    #[test]
    fn an_empty_new_name_falls_back_to_the_legacy_one() {
        let got = setting_from(lookup(&[("WATT_X", ""), ("AGENT_TERM_X", "old")]), "WATT_X", "AGENT_TERM_X");
        assert_eq!(got, Some(OsString::from("old")));
    }

    #[test]
    fn an_empty_legacy_name_gives_none() {
        assert_eq!(setting_from(lookup(&[("AGENT_TERM_X", "")]), "WATT_X", "AGENT_TERM_X"), None);
    }

    #[test]
    fn neither_name_set_gives_none() {
        assert_eq!(setting_from(lookup(&[]), "WATT_X", "AGENT_TERM_X"), None);
    }

    #[test]
    fn home_wins_over_userprofile() {
        assert_eq!(home_from(Some("/Users/me".into()), Some("C:\\Users\\me".into())), Some(PathBuf::from("/Users/me")));
    }

    #[test]
    fn userprofile_is_used_when_home_is_unset() {
        assert_eq!(home_from(None, Some("C:\\Users\\me".into())), Some(PathBuf::from("C:\\Users\\me")));
    }

    #[test]
    fn userprofile_is_used_when_home_is_empty() {
        assert_eq!(home_from(Some("".into()), Some("C:\\Users\\me".into())), Some(PathBuf::from("C:\\Users\\me")));
    }

    #[test]
    fn no_home_gives_none() {
        assert_eq!(home_from(None, None), None);
        assert_eq!(home_from(Some("".into()), Some("".into())), None);
    }
}
