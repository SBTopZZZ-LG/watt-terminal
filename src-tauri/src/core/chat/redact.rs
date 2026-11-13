//! Redaction of text before it reaches the model or the stored history.
//!
//! Two passes. First every known secret (the API key, header values) is replaced by exact match. Then
//! high-precision patterns for common credential formats are replaced with a marker that names the kind, for
//! example `[REDACTED:api_key]`. The patterns are narrow on purpose: a false positive hides useful output from
//! the model, so each one needs a distinctive prefix or shape. The regex crate has no look-around, so the
//! patterns use `\b`, character classes and fixed lengths instead.
//!
//! `redact` only searches and replaces, so it never panics on any `&str`.

use std::sync::LazyLock;

use regex::Regex;

/// Known secrets shorter than this (after trimming) are not replaced: a short value would match ordinary text.
const MIN_SECRET_CHARS: usize = 6;

/// The pattern table, compiled once. Each entry is a pattern and its replacement. The patterns are constants and
/// the unit tests compile them, so a bad one fails the test run rather than a live request.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // PEM private key block. An unterminated block (cut off by the output window) is redacted to the end.
        (
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            "[REDACTED:private_key]",
        ),
        // OpenAI, OpenRouter (sk-or-v1-), Anthropic (sk-ant-) and similar.
        (r"\bsk-[A-Za-z0-9_\-]{20,}", "[REDACTED:api_key]"),
        (r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})", "[REDACTED:github_token]"),
        (r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b", "[REDACTED:aws_key]"),
        (r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", "[REDACTED:slack_token]"),
        (r"\bAIza[0-9A-Za-z_\-]{35}", "[REDACTED:google_key]"),
        (r"\bsk_live_[0-9A-Za-z]{10,}", "[REDACTED:stripe_key]"),
        (r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]*", "[REDACTED:jwt]"),
        // `Authorization: Bearer <token>`. Case-insensitive; the 16-character minimum keeps ordinary words out.
        (r"(?i)\bbearer\s+[A-Za-z0-9._~+/=\-]{16,}", "Bearer [REDACTED:bearer]"),
        // `NAME=value` and `export NAME=value` where NAME is underscore-separated segments ending in or containing
        // a whole credential word (API_KEY/APIKEY, TOKEN, SECRET, PASSWORD, AUTHORIZATION, ...). The name stays, the
        // value goes. The value is a quoted string (closed or cut at the end of the line) or a run of non-space
        // characters. Bare KEY/AUTH never match, so GIT_AUTHOR_NAME, SSH_AUTH_SOCK, XAUTHORITY and OAUTH_CALLBACK_URL
        // are left alone. A value that already starts with `[` (a redaction marker) is left alone, including quoted
        // values, so redacting twice changes nothing.
        (
            r#"(?i)\b(?P<name>(?:[A-Za-z0-9]+_)*(?:API_?KEY|ACCESS_?KEY|SECRET|TOKEN|PASSWORD|PASSWD|CREDENTIAL|AUTHORIZATION)(?:_[A-Za-z0-9]+)*)=(?:"[^"\[\n][^"\n]*"?|'[^'\[\n][^'\n]*'?|[^\s"'\[][^\s"']*)"#,
            "${name}=[REDACTED:env]",
        ),
        // Credential headers, `x-api-key: value` or `Authorization: Basic value`: the header name and an optional
        // scheme word stay, the value goes. The value is at least 8 characters with no space, quote or comma, and
        // never starts with `[`, so an existing marker is not replaced again.
        (
            r#"(?i)\b((?:x-api-key|api-key|x-auth-token|authorization|proxy-authorization)[ \t]*:[ \t]*)((?:bearer|basic|token)[ \t]+)?[^\s"',;\[][^\s"',;]{7,}"#,
            "${1}${2}[REDACTED:header]",
        ),
        // A JSON or YAML entry whose key is exactly a credential name, with a quoted value of at least 8 characters and
        // no space. The key and the quotes stay. A value that is already a marker is left alone, as above.
        (
            r#"(?i)(\b(?:api_key|apikey|api-key|secret|client_secret|password|access_token|auth_token|refresh_token)"?[ \t]*[:=][ \t]*)"[^\s"\[][^\s"]{7,}""#,
            "${1}\"[REDACTED:secret]\"",
        ),
        // Password in URL userinfo: scheme://user:pass@host keeps the user and the host.
        (r"([A-Za-z][A-Za-z0-9+.\-]*://[^\s:/@]+):[^\s/@]+@", "${1}:[REDACTED:url]@"),
    ]
    .into_iter()
    .map(|(pattern, replacement)| {
        (Regex::new(pattern).expect("redaction patterns are constants and must compile"), replacement)
    })
    .collect()
});

/// Replaces known secrets and credential-shaped text. Holds the secret values, so it does not implement `Debug`.
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    /// `known_secrets` are the exact values to remove (the API key, secret header values). Each is trimmed, and
    /// values shorter than `MIN_SECRET_CHARS` are ignored.
    pub fn new(known_secrets: Vec<String>) -> Self {
        let mut secrets: Vec<String> = known_secrets
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| s.chars().count() >= MIN_SECRET_CHARS)
            .collect();
        secrets.sort();
        secrets.dedup();
        // Longest first, so a secret that contains a shorter one is replaced whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Redactor { secrets }
    }

    /// Returns `text` with known secrets and credential-shaped text replaced. Applying it twice changes nothing more.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[REDACTED]");
            }
        }
        for (pattern, replacement) in PATTERNS.iter() {
            out = pattern.replace_all(&out, *replacement).into_owned();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Redactor {
        Redactor::new(Vec::new())
    }

    fn with(secrets: &[&str]) -> Redactor {
        Redactor::new(secrets.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn known_secret_is_replaced_everywhere_it_appears() {
        let r = with(&["tok-value-123456"]);
        assert_eq!(r.redact("a tok-value-123456 b tok-value-123456"), "a [REDACTED] b [REDACTED]");
    }

    #[test]
    fn short_blank_and_whitespace_secrets_are_ignored() {
        let r = with(&["abc", "", "     ", "a b"]);
        assert_eq!(r.redact("abc a b   x"), "abc a b   x");
    }

    #[test]
    fn known_secret_is_trimmed_before_matching() {
        let r = with(&["  tok-value-123456\n"]);
        assert_eq!(r.redact("key=tok-value-123456"), "key=[REDACTED]");
    }

    #[test]
    fn longer_secret_wins_over_a_secret_it_contains() {
        let r = with(&["abcdefgh", "abcdefghij"]);
        assert_eq!(r.redact("x abcdefghij y"), "x [REDACTED] y");
    }

    #[test]
    fn pem_private_key_block_is_redacted_through_the_end_line() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAKj34GkxFhD90vcNLYLInFEX6Ppy1tPf9Cnzj4p4WGeKLs1Pt8Qu\n-----END RSA PRIVATE KEY-----\nafter";
        assert_eq!(none().redact(text), "before\n[REDACTED:private_key]\nafter");
    }

    #[test]
    fn unterminated_pem_block_is_redacted_to_the_end_of_text() {
        let text = "x\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU";
        assert_eq!(none().redact(text), "x\n[REDACTED:private_key]");
    }

    #[test]
    fn openai_style_keys_are_redacted() {
        let r = none();
        assert_eq!(r.redact("sk-proj-abcdefghijklmnopqrstuvwx"), "[REDACTED:api_key]");
        assert_eq!(r.redact("sk-or-v1-abcdefghijklmnopqrstuvwx"), "[REDACTED:api_key]");
        assert_eq!(r.redact("sk-ant-api03-AbCdEf0123456789xyz"), "[REDACTED:api_key]");
    }

    #[test]
    fn github_tokens_are_redacted() {
        let r = none();
        assert_eq!(r.redact("ghp_A1b2C3d4E5f6G7h8I9j0K1l2"), "[REDACTED:github_token]");
        assert_eq!(r.redact("gho_A1b2C3d4E5f6G7h8I9j0K1l2"), "[REDACTED:github_token]");
        assert_eq!(
            r.redact("github_pat_11ABCDEFG0123456789_abcdefghijklmnop"),
            "[REDACTED:github_token]"
        );
    }

    #[test]
    fn aws_access_key_ids_are_redacted() {
        let r = none();
        assert_eq!(r.redact("id AKIAIOSFODNN7EXAMPLE end"), "id [REDACTED:aws_key] end");
        assert_eq!(r.redact("ASIAABCDEFGHIJKLMNOP"), "[REDACTED:aws_key]");
    }

    #[test]
    fn slack_tokens_are_redacted() {
        assert_eq!(none().redact("xoxb-1234567890-abcdefghij"), "[REDACTED:slack_token]");
    }

    #[test]
    fn google_api_keys_are_redacted() {
        let key = format!("AIza{}", "0123456789abcdefghijklmnopqrstuvwxy");
        assert_eq!(none().redact(&key), "[REDACTED:google_key]");
    }

    #[test]
    fn stripe_live_keys_are_redacted() {
        assert_eq!(none().redact("sk_live_4eC39HqLyjWDarjtT1zdp7dc"), "[REDACTED:stripe_key]");
    }

    #[test]
    fn jwt_is_redacted() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_eq!(none().redact(&format!("token {jwt} end")), "token [REDACTED:jwt] end");
    }

    #[test]
    fn bearer_token_in_an_authorization_header_is_redacted() {
        assert_eq!(
            none().redact("Authorization: Bearer abcDEF123456789xyz.tok_en~+/=-"),
            "Authorization: Bearer [REDACTED:bearer]"
        );
    }

    #[test]
    fn bearer_token_is_redacted_in_any_case() {
        assert_eq!(none().redact("authorization: bearer abcdefghijklmnop1234"), "authorization: Bearer [REDACTED:bearer]");
        assert_eq!(none().redact("BEARER abcdefghijklmnop1234 end"), "Bearer [REDACTED:bearer] end");
    }

    #[test]
    fn short_word_after_bearer_is_kept() {
        let text = "the bearer token is fine, a bearer of news, bearer 0123456789abcde";
        assert_eq!(none().redact(text), text);
    }

    #[test]
    fn bearer_redaction_is_idempotent() {
        let r = none();
        let once = r.redact("Authorization: bearer abcdefghijklmnop1234");
        assert_eq!(once, "Authorization: Bearer [REDACTED:bearer]");
        assert_eq!(r.redact(&once), once);
    }

    #[test]
    fn env_assignment_keeps_the_name_and_redacts_the_value() {
        let r = none();
        assert_eq!(r.redact("OPENAI_API_KEY=abc123xyz"), "OPENAI_API_KEY=[REDACTED:env]");
        assert_eq!(r.redact("export db_password=hunter2"), "export db_password=[REDACTED:env]");
        assert_eq!(r.redact("GITHUB_TOKEN=ghx_1 next"), "GITHUB_TOKEN=[REDACTED:env] next");
    }

    #[test]
    fn env_assignment_with_a_quoted_value_redacts_the_whole_quote() {
        let r = none();
        assert_eq!(r.redact(r#"MY_SECRET="two words here" done"#), "MY_SECRET=[REDACTED:env] done");
        assert_eq!(r.redact("API_TOKEN='a b c"), "API_TOKEN=[REDACTED:env]");
    }

    #[test]
    fn env_assignment_needs_a_whole_credential_segment() {
        let r = none();
        // True positives: the credential word is a whole underscore-separated segment.
        for text in [
            "OPENAI_API_KEY=abc123xyz",
            "AWS_SECRET_ACCESS_KEY=abc123xyz",
            "APIKEY=abc123xyz",
            "TOKEN=abc123xyz",
            "AUTHORIZATION=abc123xyz",
            "API_TOKEN=abc123xyz next",
        ] {
            assert!(r.redact(text).ends_with("[REDACTED:env]") || r.redact(text).contains("[REDACTED:env]"), "missed: {text}");
        }
        // False positives: KEY/AUTH fragments that are not whole credential words are kept.
        for text in [
            "GIT_AUTHOR_NAME=alice",
            "SSH_AUTH_SOCK=/tmp/sock",
            "XAUTHORITY=/home/u/.Xauthority",
            "OAUTH_CALLBACK_URL=https://example.com/cb",
            "primary_key=12345",
        ] {
            assert_eq!(r.redact(text), text, "changed: {text}");
        }
    }

    #[test]
    fn env_assignment_markers_are_not_redacted_again() {
        let r = none();
        assert_eq!(r.redact("token=[REDACTED]"), "token=[REDACTED]");
        assert_eq!(r.redact("TOKEN=\"[REDACTED:env]\""), "TOKEN=\"[REDACTED:env]\"");
        assert_eq!(r.redact("TOKEN='[REDACTED:env]'"), "TOKEN='[REDACTED:env]'");
    }

    #[test]
    fn env_assignments_without_a_credential_word_are_kept() {
        let text = "PATH=/usr/bin:/bin HOME=/Users/me LANG=en_US.UTF-8";
        assert_eq!(none().redact(text), text);
    }

    #[test]
    fn url_password_is_redacted_and_user_and_host_kept() {
        assert_eq!(
            none().redact("git clone https://alice:hunter2@example.com/repo.git"),
            "git clone https://alice:[REDACTED:url]@example.com/repo.git"
        );
    }

    #[test]
    fn url_without_a_password_is_kept() {
        let text = "https://example.com:8443/path https://alice@example.com/";
        assert_eq!(none().redact(text), text);
    }

    #[test]
    fn patterns_need_their_full_shape() {
        let r = none();
        assert_eq!(r.redact("task-manager-configuration-x"), "task-manager-configuration-x");
        assert_eq!(r.redact("risk-assessment-long-name"), "risk-assessment-long-name");
        assert_eq!(r.redact("sk-short"), "sk-short");
        assert_eq!(r.redact("ghp_short"), "ghp_short");
        assert_eq!(r.redact("AKIASHORT"), "AKIASHORT");
    }

    #[test]
    fn ordinary_text_is_not_changed() {
        let samples = [
            "cargo test finished: 147 passed, 0 failed",
            "commit 9c68bfd3a1b2c4d5e6f708192a3b4c5d6e7f8091 (HEAD -> main)",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=",
            "drwxr-xr-x  5 me staff 160 Nov 10 09:00 src",
            "if a == b then echo done; fi",
            "naïve café — 日本語 テキスト",
            r#"{"token": "short", "count": 3}"#,
            "Reset your password: choose a new one now",
            "the api_key field is optional and the secret is in the vault",
            "authorization: none",
            "x-api-key: short",
            "Authorization: Basic",
        ];
        let r = none();
        for text in samples {
            assert_eq!(r.redact(text), text, "changed: {text}");
        }
    }

    #[test]
    fn credential_headers_keep_the_name_and_redact_the_value() {
        let r = none();
        assert_eq!(r.redact("x-api-key: abcdef123456"), "x-api-key: [REDACTED:header]");
        assert_eq!(r.redact("api-key:abcdef123456"), "api-key:[REDACTED:header]");
        assert_eq!(r.redact("X-Auth-Token: 0123456789abcdef"), "X-Auth-Token: [REDACTED:header]");
        assert_eq!(
            r.redact("Authorization: Basic dXNlcjpwYXNzd29yZA=="),
            "Authorization: Basic [REDACTED:header]"
        );
        assert_eq!(r.redact("proxy-authorization: token abcdefgh1234"), "proxy-authorization: token [REDACTED:header]");
    }

    #[test]
    fn json_and_yaml_credential_keys_keep_the_key_and_redact_the_quoted_value() {
        let r = none();
        assert_eq!(r.redact(r#"{"api_key": "abcdefgh12345"}"#), r#"{"api_key": "[REDACTED:secret]"}"#);
        assert_eq!(r.redact(r#"{"refresh_token":"abcdefgh12"}"#), r#"{"refresh_token":"[REDACTED:secret]"}"#);
        assert_eq!(r.redact("client_secret: \"abcdefgh12\""), "client_secret: \"[REDACTED:secret]\"");
        assert_eq!(r.redact(r#"{"password": "hunter2hunter2"}"#), r#"{"password": "[REDACTED:secret]"}"#);
    }

    #[test]
    fn credential_key_and_header_rules_do_not_match_short_values_or_other_keys() {
        let r = none();
        // A short value, a key that is not on the list, a value with a space, and an unquoted JSON value.
        for text in [
            r#"{"token": "short"}"#,
            r#"{"my_secret_name": "abcdefgh12"}"#,
            r#"{"password": "correct horse battery"}"#,
            r#"{"password": 12345678}"#,
            "password: hunter2hunter2",
            "x-api-key:",
        ] {
            assert_eq!(r.redact(text), text, "changed: {text}");
        }
    }

    #[test]
    fn a_git_sha_and_prose_mentioning_credentials_are_kept() {
        let r = none();
        let sha = "9c68bfd3a1b2c4d5e6f708192a3b4c5d6e7f8091";
        let text = format!("fixed in {sha}; the password reset page and the token list are unchanged");
        assert_eq!(r.redact(&text), text);
        let log = format!("commit {sha}\nAuthor: someone\n    rotate the api key next week");
        assert_eq!(r.redact(&log), log);
    }

    #[test]
    fn new_credential_markers_are_idempotent() {
        let r = none();
        let text = "Authorization: Basic dXNlcjpwYXNzd29yZA== {\"secret\": \"abcdefgh12345\"} x-api-key: abcdef123456";
        let once = r.redact(text);
        assert_eq!(r.redact(&once), once);
    }

    #[test]
    fn redaction_is_idempotent() {
        let r = with(&["known-secret-value"]);
        let text = "known-secret-value sk-proj-abcdefghijklmnopqrstuvwx \
            API_KEY=abc PASS=x https://u:p@h/ -----BEGIN PRIVATE KEY-----\nAAAA";
        let once = r.redact(text);
        assert_eq!(r.redact(&once), once);
    }

    #[test]
    fn odd_input_does_not_panic() {
        let r = with(&["é日本語secret"]);
        let inputs = [
            "",
            "\u{0}\u{1b}[31m",
            "-----BEGIN PRIVATE KEY-----",
            "-----END PRIVATE KEY-----",
            "=",
            "==== ====",
            "://@:",
            "https://:@",
            "KEY=",
            "KEY==",
            "é=ü",
            "sk-",
            "eyJ.eyJ.",
            "AIza",
            "é日本語secret é日本語secret",
            "\n\n\r\n\t",
        ];
        for text in inputs {
            let _ = r.redact(text);
        }
        assert_eq!(r.redact("é日本語secret"), "[REDACTED]");
    }
}
