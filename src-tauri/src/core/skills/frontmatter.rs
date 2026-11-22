//! The `SKILL.md` frontmatter: a small hand-rolled reader for the YAML subset skills use. It reads top-level `key: value`
//! lines (plain, 'single' or "double" quoted), `|` and `>` block scalars (with `-` to drop the final newline), and
//! `true` / `false`. Anything else (lists, nested maps, comments) is skipped, never an error. No YAML crate is used.

/// A parsed `SKILL.md`: the frontmatter fields and the body after the closing `---` line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    fields: Vec<(String, String)>,
    pub body: String,
}

impl Parsed {
    /// The value of a key: the first one in the file. A block scalar is joined into one string.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The value of a boolean key: `true` or `false` in any case. None when absent or not a boolean.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.text(key)?.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

/// Splits a `SKILL.md` into its frontmatter and body. The file must open with a `---` line (a UTF-8 BOM and CRLF line
/// ends are accepted), and the block must close with a `---` line. The error says what is wrong, for the scan warnings.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err("no frontmatter: the file must start with a --- line".to_string());
    }
    let mut offset = first.len();
    let mut block = Vec::new();
    for line in lines {
        offset += line.len();
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim_end() == "---" {
            return Ok(Parsed { fields: fields(&block), body: text[offset..].to_string() });
        }
        block.push(line);
    }
    Err("the frontmatter is not closed by a --- line".to_string())
}

/// The top-level fields of the frontmatter lines, in file order.
fn fields(block: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < block.len() {
        let line = block[i];
        i += 1;
        // Blank lines, comments and indented lines (nested values) are not top-level keys.
        if line.trim().is_empty() || line.starts_with('#') || line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else { continue };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let raw = raw.trim();
        let value = if matches!(raw, "|" | ">" | "|-" | ">-") {
            let start = i;
            while i < block.len() && (block[i].trim().is_empty() || block[i].starts_with([' ', '\t'])) {
                i += 1;
            }
            block_scalar(&block[start..i], raw.starts_with('>'), raw.ends_with('-'))
        } else {
            scalar(raw)
        };
        out.push((key.to_string(), value));
    }
    out
}

/// The text of a `|` (literal) or `>` (folded) block: its indented lines with the common indent removed. Folded lines
/// are joined with spaces. `strip` (the `-` form) drops the final newline.
fn block_scalar(lines: &[&str], folded: bool, strip: bool) -> String {
    let indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let mut kept: Vec<&str> = lines.iter().map(|l| if l.trim().is_empty() { "" } else { &l[indent..] }).collect();
    while kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }
    let text = if folded {
        kept.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
    } else {
        kept.join("\n")
    };
    if strip { text } else { format!("{text}\n") }
}

/// A single-line value: unquoted, or the inside of a "double" or 'single' quoted one.
fn scalar(raw: &str) -> String {
    if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    // A plain value ends where a ` #` comment starts.
    let plain = match raw.find(" #") {
        Some(at) => &raw[..at],
        None => raw,
    };
    plain.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str, key: &str) -> Option<String> {
        parse(text).expect("parses").text(key).map(str::to_string)
    }

    #[test]
    fn plain_and_quoted_scalars() {
        let text = "---\nname: demo\nquoted: \"a: b # not a comment\"\nsingle: 'it''s'\ncomment: plain # note\n---\nBody\n";
        assert_eq!(field(text, "name").as_deref(), Some("demo"));
        assert_eq!(field(text, "quoted").as_deref(), Some("a: b # not a comment"));
        assert_eq!(field(text, "single").as_deref(), Some("it's"));
        assert_eq!(field(text, "comment").as_deref(), Some("plain"));
    }

    #[test]
    fn folded_block_joins_lines_with_spaces() {
        let text = "---\ndescription: >-\n  Reads the logs\n  and reports errors.\nname: x\n---\n";
        assert_eq!(field(text, "description").as_deref(), Some("Reads the logs and reports errors."));
        assert_eq!(field(text, "name").as_deref(), Some("x"));
    }

    #[test]
    fn literal_block_keeps_line_breaks() {
        let text = "---\ndescription: |\n  first line\n    indented\n  last\n---\n";
        assert_eq!(field(text, "description").as_deref(), Some("first line\n  indented\nlast\n"));
        let stripped = "---\ndescription: |-\n  one\n  two\n---\n";
        assert_eq!(field(stripped, "description").as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn crlf_and_bom_are_accepted() {
        let text = "\u{feff}---\r\nname: demo\r\ndescription: Works\r\n---\r\nBody line\r\n";
        let parsed = parse(text).expect("parses");
        assert_eq!(parsed.text("name"), Some("demo"));
        assert_eq!(parsed.text("description"), Some("Works"));
        assert_eq!(parsed.body, "Body line\r\n");
    }

    #[test]
    fn missing_or_unclosed_frontmatter_is_an_error() {
        assert!(parse("# just a heading\n").unwrap_err().contains("must start with a --- line"));
        assert!(parse("").unwrap_err().contains("must start with a --- line"));
        assert!(parse("---\nname: demo\n").unwrap_err().contains("not closed"));
    }

    #[test]
    fn booleans_read_as_flags_and_other_values_do_not() {
        let text = "---\ndisable-model-invocation: true\nother: yes\n---\n";
        let parsed = parse(text).expect("parses");
        assert_eq!(parsed.flag("disable-model-invocation"), Some(true));
        assert_eq!(parsed.flag("other"), None);
        assert_eq!(parsed.flag("absent"), None);
    }

    #[test]
    fn lists_nested_maps_and_comments_are_ignored() {
        let text = "---\n# a comment\nallowed-tools:\n  - run_command\n  - Read\nmetadata:\n  version: 2\nname: demo\n---\n";
        let parsed = parse(text).expect("parses");
        assert_eq!(parsed.text("name"), Some("demo"));
        assert_eq!(parsed.text("allowed-tools"), Some(""));
        assert_eq!(parsed.text("version"), None);
    }

    #[test]
    fn body_is_everything_after_the_closing_line() {
        let parsed = parse("---\nname: demo\n---\n# Steps\n\n1. Do it\n").expect("parses");
        assert_eq!(parsed.body, "# Steps\n\n1. Do it\n");
    }
}
