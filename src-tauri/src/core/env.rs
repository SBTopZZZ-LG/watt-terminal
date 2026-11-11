//! Environment settings. The app is named Watt, so its settings are `WATT_*`. The old `AGENT_TERM_*` names still work,
//! so a shell set up with them keeps working. A `WATT_*` value that is set and not empty wins over the old name.

use std::env;
use std::ffi::OsString;

/// The value of the setting `name`, or of its legacy name `legacy` when `name` is unset or empty. `None` when neither
/// has a non-empty value.
pub fn env_setting(name: &str, legacy: &str) -> Option<OsString> {
    setting_from(|key| env::var_os(key), name, legacy)
}

/// `env_setting` over any lookup, so tests need not change the process environment.
fn setting_from(lookup: impl Fn(&str) -> Option<OsString>, name: &str, legacy: &str) -> Option<OsString> {
    [name, legacy].into_iter().find_map(|key| lookup(key).filter(|v| !v.is_empty()))
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
}
