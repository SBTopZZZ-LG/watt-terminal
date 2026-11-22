//! The MCP server settings as they are stored in settings.json. Secret values are not here: the config keeps only the
//! names of env variables and headers, and the values live in the secret store (see `secrets.rs`).

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::core::external_tools::ExternalApproval;

/// Longest server name, in characters.
pub const MAX_NAME_CHARS: usize = 32;
/// The per-call timeout when a server sets none.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Lower bound of the per-call timeout. A saved value outside the bounds is clamped when it is used, not refused.
pub const MIN_TIMEOUT_SECS: u64 = 1;
/// Upper bound of the per-call timeout.
pub const MAX_TIMEOUT_SECS: u64 = 3600;

/// The MCP servers the user configured. Missing in older files: no servers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    pub servers: Vec<McpServerConfig>,
}

/// One MCP server. `name` is the slug that prefixes its tool names (see `names.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique slug: 1 to 32 characters of a-z, 0-9, '-' or '_'.
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The level of every tool of this server that has no override in `tools`.
    #[serde(default)]
    pub approval: McpLevel,
    /// Per-tool overrides, keyed by the server's original tool name. A tool with no entry inherits `approval`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, McpLevel>,
    /// Seconds per call. `None` means `DEFAULT_TIMEOUT_SECS`. See `timeout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    pub transport: McpTransport,
}

fn default_enabled() -> bool {
    true
}

/// How the app reaches the server. Stored with an explicit `type` key: `"stdio"` or `"http"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Variable names only. The values are in the secret store, under this server.
        #[serde(default)]
        env: Vec<String>,
    },
    /// Streamable HTTP.
    Http {
        url: String,
        /// Header names only. The values are in the secret store, under this server.
        #[serde(default)]
        headers: Vec<String>,
    },
}

/// The approval level of a server or of one tool. `Never` means the tool is not offered to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpLevel {
    #[default]
    Ask,
    Auto,
    Never,
}

impl From<McpLevel> for ExternalApproval {
    fn from(level: McpLevel) -> Self {
        match level {
            McpLevel::Ask => ExternalApproval::Ask,
            McpLevel::Auto => ExternalApproval::Auto,
            McpLevel::Never => ExternalApproval::Never,
        }
    }
}

impl McpServerConfig {
    /// The per-call timeout, clamped to `MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS`.
    pub fn timeout(&self) -> Duration {
        let secs = self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        Duration::from_secs(secs)
    }

    fn validate(&self, problems: &mut Vec<String>) {
        if !valid_server_name(&self.name) {
            problems.push(format!(
                "mcp server name {:?} must be 1 to {MAX_NAME_CHARS} characters of a-z, 0-9, '-' or '_'",
                self.name
            ));
        }
        let label = format!("mcp server {:?}", self.name);
        match &self.transport {
            McpTransport::Stdio { command, cwd, env, .. } => {
                if command.trim().is_empty() {
                    problems.push(format!("{label}: command must not be empty"));
                } else if command.chars().any(char::is_control) {
                    problems.push(format!("{label}: command must not contain control characters"));
                }
                if let Some(cwd) = cwd {
                    if cwd.trim().is_empty() {
                        problems.push(format!("{label}: cwd must not be empty"));
                    } else if cwd.chars().any(char::is_control) {
                        problems.push(format!("{label}: cwd must not contain control characters"));
                    }
                }
                let mut seen = HashSet::new();
                for name in env {
                    if !valid_env_name(name) {
                        problems.push(format!(
                            "{label}: env variable name {name:?} must be letters, digits and '_', not starting with a digit"
                        ));
                    } else if !seen.insert(name.as_str()) {
                        problems.push(format!("{label}: env variable {name:?} is listed more than once"));
                    }
                }
            }
            McpTransport::Http { url, headers } => {
                validate_mcp_url(&label, url, problems);
                let mut seen = HashSet::new();
                for name in headers {
                    if name.is_empty() || !name.bytes().all(is_token_byte) {
                        problems.push(format!("{label}: header name {name:?} is not a valid HTTP header name"));
                    } else if !seen.insert(name.to_ascii_lowercase()) {
                        problems.push(format!("{label}: header {name:?} is listed more than once"));
                    }
                }
            }
        }
    }
}

impl McpSettings {
    /// Every problem found, one human-readable string each. Never includes a secret value; a URL is not echoed either.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen = HashSet::new();
        for server in &self.servers {
            server.validate(&mut problems);
            if !seen.insert(server.name.as_str()) {
                problems.push(format!("mcp server {:?} is listed more than once", server.name));
            }
        }
        problems
    }
}

/// `[a-z0-9_-]{1,32}`
pub fn valid_server_name(name: &str) -> bool {
    (1..=MAX_NAME_CHARS).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `[A-Za-z_][A-Za-z0-9_]*`
pub fn valid_env_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// RFC 7230 `tchar`.
pub fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// The URL must parse, use http or https and have a host. Plain http is allowed only for a loopback host. The URL is
/// not put in the problem text: it may carry a key in its query.
fn validate_mcp_url(label: &str, url: &str, problems: &mut Vec<String>) {
    if url.trim().is_empty() {
        problems.push(format!("{label}: url must not be empty"));
        return;
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        problems.push(format!("{label}: url must not contain whitespace or control characters"));
        return;
    }
    match Url::parse(url) {
        Ok(parsed) if parsed.scheme() == "https" && parsed.host_str().is_some_and(|h| !h.is_empty()) => {}
        Ok(parsed) if parsed.scheme() == "http" && parsed.host_str().is_some_and(|h| !h.is_empty()) => {
            if !is_loopback_host(&parsed) {
                problems.push(format!("{label}: url must use https unless the host is localhost"));
            }
        }
        Ok(_) => problems.push(format!("{label}: url must be an http or https URL with a host")),
        Err(_) => problems.push(format!("{label}: url is not a valid URL")),
    }
}

/// `localhost`, `*.localhost`, or a loopback IP address (127.0.0.0/8, ::1).
fn is_loopback_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::test_support::TempDir;
    use crate::core::settings::{self, Settings};

    fn stdio(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            enabled: true,
            approval: McpLevel::Ask,
            tools: BTreeMap::new(),
            timeout_secs: None,
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-fetch".into()],
                cwd: None,
                env: vec!["FETCH_TOKEN".into()],
            },
        }
    }

    fn http(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            enabled: true,
            approval: McpLevel::Auto,
            tools: BTreeMap::from([("search".to_string(), McpLevel::Never)]),
            timeout_secs: Some(30),
            transport: McpTransport::Http {
                url: "https://mcp.context7.com/mcp".into(),
                headers: vec!["CONTEXT7_API_KEY".into()],
            },
        }
    }

    fn with_servers(servers: Vec<McpServerConfig>) -> McpSettings {
        McpSettings { servers }
    }

    /// Validates a settings value holding one server with `change` applied to it.
    fn problems_after(change: impl FnOnce(&mut McpServerConfig)) -> Vec<String> {
        let mut server = http("ctx");
        change(&mut server);
        with_servers(vec![server]).validate()
    }

    fn http_url_problem(url: &str) -> Vec<String> {
        problems_after(|s| s.transport = McpTransport::Http { url: url.into(), headers: vec![] })
    }

    #[test]
    fn an_old_settings_file_without_mcp_loads_with_no_servers() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(settings::settings_path(dir.path()), r#"{"version":1,"providers":[]}"#).unwrap();
        let loaded = settings::load(dir.path()).unwrap();
        assert_eq!(loaded.mcp, McpSettings::default());
        assert!(loaded.mcp.servers.is_empty());
    }

    #[test]
    fn an_empty_mcp_section_round_trips() {
        let dir = TempDir::new();
        settings::save(dir.path(), &Settings::default()).unwrap();
        let text = std::fs::read_to_string(settings::settings_path(dir.path())).unwrap();
        assert!(text.contains(r#""mcp": {"#), "{text}");
        assert_eq!(settings::load(dir.path()).unwrap().mcp, McpSettings::default());
    }

    #[test]
    fn two_servers_one_stdio_one_http_round_trip_through_the_file() {
        let dir = TempDir::new();
        let settings = Settings { mcp: with_servers(vec![stdio("fetch"), http("context7")]), ..Settings::default() };
        settings::save(dir.path(), &settings).unwrap();

        let text = std::fs::read_to_string(settings::settings_path(dir.path())).unwrap();
        assert!(text.contains(r#""type": "stdio""#), "{text}");
        assert!(text.contains(r#""type": "http""#), "{text}");
        assert!(text.contains(r#""CONTEXT7_API_KEY""#), "header names are stored: {text}");

        let loaded = settings::load(dir.path()).unwrap();
        assert_eq!(loaded.mcp, settings.mcp);
        assert_eq!(loaded.mcp.servers[1].tools.get("search"), Some(&McpLevel::Never));
    }

    #[test]
    fn server_defaults_apply_when_fields_are_missing() {
        let json = r#"{"name":"fetch","transport":{"type":"stdio","command":"uvx"}}"#;
        let server: McpServerConfig = serde_json::from_str(json).unwrap();
        assert!(server.enabled);
        assert_eq!(server.approval, McpLevel::Ask);
        assert!(server.tools.is_empty());
        assert_eq!(server.timeout(), Duration::from_secs(120));
        assert_eq!(
            server.transport,
            McpTransport::Stdio { command: "uvx".into(), args: vec![], cwd: None, env: vec![] }
        );
    }

    #[test]
    fn timeout_is_clamped_to_its_bounds() {
        let mut server = stdio("fetch");
        server.timeout_secs = Some(0);
        assert_eq!(server.timeout(), Duration::from_secs(1));
        server.timeout_secs = Some(999_999);
        assert_eq!(server.timeout(), Duration::from_secs(3600));
        server.timeout_secs = Some(45);
        assert_eq!(server.timeout(), Duration::from_secs(45));
    }

    #[test]
    fn level_maps_to_external_approval() {
        assert_eq!(ExternalApproval::from(McpLevel::Ask), ExternalApproval::Ask);
        assert_eq!(ExternalApproval::from(McpLevel::Auto), ExternalApproval::Auto);
        assert_eq!(ExternalApproval::from(McpLevel::Never), ExternalApproval::Never);
    }

    #[test]
    fn a_valid_config_has_no_problems() {
        assert!(with_servers(vec![stdio("fetch"), http("you_com-2")]).validate().is_empty());
        assert!(McpSettings::default().validate().is_empty());
    }

    #[test]
    fn invalid_server_names_are_reported() {
        for bad in ["", "Fetch", "has space", "dot.name", "héllo", &"x".repeat(33)] {
            let problems = problems_after(|s| s.name = bad.into());
            assert!(problems.iter().any(|p| p.contains("name")), "{bad:?}: {problems:?}");
        }
        assert!(problems_after(|s| s.name = "x".repeat(32)).is_empty());
    }

    #[test]
    fn duplicate_server_names_are_reported_once_per_extra_server() {
        let problems = with_servers(vec![stdio("fetch"), stdio("fetch")]).validate();
        assert_eq!(problems, vec![r#"mcp server "fetch" is listed more than once"#.to_string()]);
    }

    #[test]
    fn stdio_command_and_cwd_must_be_present_and_clean() {
        let mut blank = stdio("fetch");
        blank.transport = McpTransport::Stdio { command: "  ".into(), args: vec![], cwd: None, env: vec![] };
        assert!(with_servers(vec![blank]).validate()[0].contains("command must not be empty"));

        let problems = problems_after(|s| {
            s.transport = McpTransport::Stdio { command: "npx".into(), args: vec![], cwd: Some(" ".into()), env: vec![] }
        });
        assert!(problems.iter().any(|p| p.contains("cwd must not be empty")), "{problems:?}");
    }

    #[test]
    fn env_names_must_be_valid_and_unique() {
        let problems = problems_after(|s| {
            s.transport = McpTransport::Stdio {
                command: "npx".into(),
                args: vec![],
                cwd: None,
                env: vec!["GOOD_1".into(), "1BAD".into(), "BAD-NAME".into(), "GOOD_1".into(), "".into()],
            }
        });
        assert!(problems.iter().any(|p| p.contains("\"1BAD\"")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("\"BAD-NAME\"")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("\"GOOD_1\" is listed more than once")), "{problems:?}");
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    #[test]
    fn http_url_must_be_http_or_https_with_a_host() {
        assert!(http_url_problem("https://api.you.com/mcp/free").is_empty());
        assert!(http_url_problem("http://localhost:8080/mcp").is_empty());
        assert!(http_url_problem("").iter().any(|p| p.contains("must not be empty")));
        assert!(http_url_problem("ftp://example.com/mcp").iter().any(|p| p.contains("http or https")));
        assert!(http_url_problem("not a url").iter().any(|p| p.contains("whitespace")));
        assert!(http_url_problem("https://").iter().any(|p| p.contains("not a valid URL")));
    }

    #[test]
    fn plain_http_is_allowed_only_for_loopback_hosts() {
        for ok_url in [
            "http://localhost:3000/mcp",
            "http://api.localhost/mcp",
            "http://127.0.0.1/mcp",
            "http://127.5.6.7:8080/mcp",
            "http://[::1]/mcp",
            "https://example.com/mcp",
        ] {
            assert!(http_url_problem(ok_url).is_empty(), "{ok_url}: {:?}", http_url_problem(ok_url));
        }
        for bad_url in ["http://example.com/mcp", "http://192.168.1.5/mcp", "http://localhost.example.com/mcp"] {
            let problems = http_url_problem(bad_url);
            assert!(
                problems.iter().any(|p| p.contains("url must use https unless the host is localhost")),
                "{bad_url}: {problems:?}"
            );
            assert!(problems.iter().all(|p| !p.contains("example.com") && !p.contains("192.168")), "{problems:?}");
        }
    }

    #[test]
    fn invalid_url_problems_do_not_echo_the_url() {
        let secret_url = "https://example.com/mcp?key=sekrit-value-123";
        let problems = http_url_problem(&format!("ftp://{}", &secret_url[8..]));
        assert_eq!(problems.len(), 1);
        assert!(!problems[0].contains("sekrit"), "{problems:?}");
    }

    #[test]
    fn header_names_must_be_valid_tokens_and_unique_ignoring_case() {
        let problems = problems_after(|s| {
            s.transport = McpTransport::Http {
                url: "https://example.com/mcp".into(),
                headers: vec!["Authorization".into(), "authorization".into(), "bad header".into(), "".into()],
            }
        });
        assert!(problems.iter().any(|p| p.contains("\"authorization\" is listed more than once")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("\"bad header\" is not a valid HTTP header name")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("\"\" is not a valid HTTP header name")), "{problems:?}");
    }

    #[test]
    fn a_settings_value_with_a_bad_server_makes_settings_validation_fail() {
        let settings = Settings { mcp: with_servers(vec![problems_after_server("Bad Name")]), ..Settings::default() };
        let problems = settings.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("mcp server name")), "{problems:?}");
    }

    fn problems_after_server(name: &str) -> McpServerConfig {
        let mut server = stdio("fetch");
        server.name = name.into();
        server
    }

    #[test]
    fn a_bad_mcp_server_in_a_file_is_refused_and_the_file_is_left_alone() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path()).unwrap();
        let text = r#"{"version":1,"mcp":{"servers":[{"name":"ok","transport":{"type":"http","url":"ftp://x.test/mcp"}}]}}"#;
        std::fs::write(settings::settings_path(dir.path()), text).unwrap();
        let err = settings::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("http or https"), "{err}");
        assert_eq!(std::fs::read_to_string(settings::settings_path(dir.path())).unwrap(), text);
    }

    #[test]
    fn mcp_level_serializes_as_lowercase_words() {
        assert_eq!(serde_json::to_string(&McpLevel::Never).unwrap(), r#""never""#);
        assert_eq!(serde_json::from_str::<McpLevel>(r#""auto""#).unwrap(), McpLevel::Auto);
        assert!(serde_json::from_str::<McpLevel>(r#""Auto""#).is_err());
    }
}
