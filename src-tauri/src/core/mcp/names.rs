//! The names the model sees for MCP tools: `mcp__{server}__{tool}`, kept to 64 characters and unique per server.
//! Tool names from the server are arbitrary text; the names built here use only [A-Za-z0-9_-].

use std::collections::BTreeMap;

/// Longest tool name the model is offered, in characters.
pub const MAX_TOOL_NAME_CHARS: usize = 64;
/// Characters of the hash suffix: `_` and 8 hex digits.
const HASH_SUFFIX_CHARS: usize = 9;

/// The name of `tool` of `server` as offered to the model. Every character of `tool` outside [A-Za-z0-9_-] becomes
/// `_`. A name over 64 characters is cut and ends in `_` and 8 hex digits of a hash of the original `{server}__{tool}`.
pub fn external_tool_name(server: &str, tool: &str) -> String {
    let full = format!("mcp__{server}__{}", sanitize(tool));
    if full.chars().count() <= MAX_TOOL_NAME_CHARS {
        return full;
    }
    with_hash_suffix(&full, &format!("{server}__{tool}"))
}

/// The offered names of one server's tools, and the original name behind each. Built once per server, so that two
/// tools that sanitize to the same text still get different names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameTable {
    /// Offered name to the server's original tool name.
    by_name: BTreeMap<String, String>,
}

impl NameTable {
    /// Names every tool in `originals`. Where the plain name is already taken, the later tool gets the hash suffix,
    /// and if that is taken too, a counter.
    pub fn build(server: &str, originals: &[String]) -> NameTable {
        let mut by_name: BTreeMap<String, String> = BTreeMap::new();
        for original in originals {
            let plain = external_tool_name(server, original);
            let name = if by_name.contains_key(&plain) {
                let hashed = with_hash_suffix(
                    &format!("mcp__{server}__{}", sanitize(original)),
                    &format!("{server}__{original}"),
                );
                unique_name(&by_name, hashed)
            } else {
                plain
            };
            by_name.insert(name, original.clone());
        }
        NameTable { by_name }
    }

    /// The server's original tool name for an offered name, if this table names it.
    pub fn original(&self, full_name: &str) -> Option<&str> {
        self.by_name.get(full_name).map(String::as_str)
    }

    /// Whether this table names `full_name`.
    pub fn owns(&self, full_name: &str) -> bool {
        self.by_name.contains_key(full_name)
    }

    /// Every offered name with the server's original tool name behind it, in name order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.by_name.iter().map(|(name, original)| (name.as_str(), original.as_str()))
    }
}

/// Replaces every character outside [A-Za-z0-9_-] with `_`, one `_` per character.
fn sanitize(tool: &str) -> String {
    tool.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// `head` cut so that `head` + `_` + 8 hex digits fits in 64 characters, with the hash of `source` as the digits.
fn with_hash_suffix(head: &str, source: &str) -> String {
    let keep = MAX_TOOL_NAME_CHARS - HASH_SUFFIX_CHARS;
    let cut: String = head.chars().take(keep).collect();
    format!("{cut}_{:08x}", fnv1a_64(source.as_bytes()) as u32)
}

/// `hashed`, or `hashed` with a counter in place of its last characters when that name is taken too.
fn unique_name(taken: &BTreeMap<String, String>, hashed: String) -> String {
    if !taken.contains_key(&hashed) {
        return hashed;
    }
    (2..)
        .map(|n| {
            let suffix = format!("_{n}");
            let keep = MAX_TOOL_NAME_CHARS - suffix.len();
            let cut: String = hashed.chars().take(keep).collect();
            format!("{cut}{suffix}")
        })
        .find(|candidate| !taken.contains_key(candidate))
        .expect("an unused counter exists")
}

/// 64-bit FNV-1a. Stable across runs and platforms, which keeps the hash suffix deterministic.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn originals(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn plain_names_are_prefixed_and_unchanged() {
        assert_eq!(external_tool_name("fetch", "fetch"), "mcp__fetch__fetch");
        assert_eq!(external_tool_name("context7", "resolve-library-id"), "mcp__context7__resolve-library-id");
        assert_eq!(external_tool_name("you-com", "search_web"), "mcp__you-com__search_web");
    }

    #[test]
    fn server_names_with_underscores_and_dashes_pass_through() {
        assert_eq!(external_tool_name("my_srv-2", "a_b-c"), "mcp__my_srv-2__a_b-c");
    }

    #[test]
    fn spaces_dots_and_other_characters_become_underscores() {
        assert_eq!(external_tool_name("s", "get file.txt"), "mcp__s__get_file_txt");
        assert_eq!(external_tool_name("s", "a/b:c"), "mcp__s__a_b_c");
    }

    #[test]
    fn unicode_becomes_one_underscore_per_character() {
        assert_eq!(external_tool_name("s", "héllo"), "mcp__s__h_llo");
        assert_eq!(external_tool_name("s", "検索"), "mcp__s____");
        assert_eq!(external_tool_name("s", "🙂x"), "mcp__s___x");
    }

    #[test]
    fn a_long_name_is_cut_to_64_characters_with_a_hash_suffix() {
        let tool = "t".repeat(200);
        let name = external_tool_name("fetch", &tool);
        assert_eq!(name.chars().count(), MAX_TOOL_NAME_CHARS);
        let (head, suffix) = name.split_at(MAX_TOOL_NAME_CHARS - HASH_SUFFIX_CHARS);
        assert!(head.starts_with("mcp__fetch__ttt"));
        assert_eq!(&suffix[..1], "_");
        assert!(suffix[1..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{suffix}");
    }

    #[test]
    fn a_name_of_exactly_64_characters_is_not_cut() {
        let tool = "x".repeat(64 - "mcp__s__".len());
        let name = external_tool_name("s", &tool);
        assert_eq!(name.len(), 64);
        assert_eq!(name, format!("mcp__s__{tool}"));
    }

    #[test]
    fn long_names_that_differ_only_late_stay_distinct() {
        let a = format!("{}_alpha", "p".repeat(100));
        let b = format!("{}_beta", "p".repeat(100));
        assert_ne!(external_tool_name("s", &a), external_tool_name("s", &b));
    }

    #[test]
    fn names_are_deterministic() {
        let tool = "q".repeat(120);
        assert_eq!(external_tool_name("ctx", &tool), external_tool_name("ctx", &tool));
        assert_eq!(fnv1a_64(b"abc"), fnv1a_64(b"abc"));
        // Published FNV-1a 64 test vector for "a".
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn a_very_long_server_name_still_fits_in_64() {
        let server = "s".repeat(32);
        let name = external_tool_name(&server, &"t".repeat(80));
        assert_eq!(name.chars().count(), MAX_TOOL_NAME_CHARS);
    }

    #[test]
    fn table_maps_names_back_to_originals() {
        let table = NameTable::build("ctx", &originals(&["resolve-library-id", "get.docs"]));
        assert_eq!(table.original("mcp__ctx__resolve-library-id"), Some("resolve-library-id"));
        assert_eq!(table.original("mcp__ctx__get_docs"), Some("get.docs"));
        assert!(table.owns("mcp__ctx__get_docs"));
        assert!(!table.owns("mcp__ctx__missing"));
        assert_eq!(table.original("mcp__other__get_docs"), None);
    }

    #[test]
    fn two_originals_that_sanitize_alike_get_different_names() {
        let table = NameTable::build("s", &originals(&["a.b", "a_b", "a b"]));
        // All three sanitize to `mcp__s__a_b`. The first keeps it; the later two get the hash suffix.
        assert_eq!(table.original("mcp__s__a_b"), Some("a.b"));
        assert_eq!(table.by_name.len(), 3, "{:?}", table.by_name);
        let later: Vec<&str> = table.by_name.keys().map(String::as_str).filter(|n| *n != "mcp__s__a_b").collect();
        assert_eq!(later.len(), 2);
        assert!(later.iter().all(|n| n.starts_with("mcp__s__a_b_") && n.len() == "mcp__s__a_b_".len() + 8), "{later:?}");
        let mut behind: Vec<&str> = later.iter().filter_map(|n| table.original(n)).collect();
        behind.sort();
        assert_eq!(behind, vec!["a b", "a_b"]);
    }

    #[test]
    fn a_later_long_collision_gets_the_hash_suffix() {
        let first = "x".repeat(70);
        let second = format!("{}y", "x".repeat(69));
        let table = NameTable::build("s", &originals(&[&first, &second]));
        let names: Vec<String> = table.by_name.keys().cloned().collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|n| n.chars().count() <= MAX_TOOL_NAME_CHARS));
    }

    #[test]
    fn duplicate_originals_do_not_collapse_into_one_name() {
        let table = NameTable::build("s", &originals(&["same", "same", "same"]));
        assert_eq!(table.by_name.len(), 3);
        assert!(table.by_name.values().all(|o| o == "same"));
    }

    #[test]
    fn an_empty_list_gives_an_empty_table() {
        let table = NameTable::build("s", &[]);
        assert!(!table.owns("mcp__s__anything"));
    }

    #[test]
    fn server_with_underscore_and_dash_does_not_confuse_the_table() {
        let table = NameTable::build("a-b_c", &originals(&["x"]));
        assert_eq!(table.original("mcp__a-b_c__x"), Some("x"));
    }
}
