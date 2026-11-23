//! The `state` file of the built-in `watt` skill: a snapshot of this Watt setup, read fresh on each `state` call. It is built
//! from an allowlist, and each line below names one field on purpose. Settings are never serialized whole, so a field added
//! later stays out until someone adds it here.
//!
//! Never shown: API keys and secret header values (only whether they are set), URL paths and queries (a host only), header
//! values, stdio arguments and env values (a count, or names), the text of regex rules (a count), and the reason settings.json
//! cannot be used (its parser can quote a value; the user reads it in Settings). Server messages are scrubbed and cut.
//! The output is plain text, in a fixed order, and at most `STATE_LIMIT` bytes.

use std::path::Path;

use crate::core::chat::redact::Redactor;
use crate::core::mcp::config::{McpLevel, McpTransport};
use crate::core::mcp::logs::scrub;
use crate::core::mcp::service::{McpStateView, McpStatus};
use crate::core::settings::service::SettingsView;
use crate::core::settings::{AgentRequests, ApprovalMode, Nested, OnDenied};
use crate::core::skills::tool::cap_text;
use crate::core::skills::{SkillIndex, SkillLevel, Source};

/// The most bytes of the snapshot.
pub const STATE_LIMIT: usize = 6 * 1024;
/// The longest server message shown, in characters.
const MESSAGE_CHARS: usize = 160;
/// The longest command name shown, in characters.
const COMMAND_CHARS: usize = 64;

/// What the snapshot is read from. `settings` is None when the settings cannot be read at all. `mcp` is the composer's status
/// (`McpService::status`): it starts nothing and reads no secret.
pub struct Input<'a> {
    pub settings: Option<&'a SettingsView>,
    pub mcp: &'a McpStatus,
    pub skills: &'a SkillIndex,
    pub app_version: &'a str,
    pub os: &'a str,
    pub config_dir: Option<&'a Path>,
    pub data_dir: Option<&'a Path>,
    pub home: Option<&'a Path>,
}

/// The snapshot text (see the module doc). Deterministic for the same input.
pub fn render(input: &Input) -> String {
    let redactor = Redactor::new(Vec::new());
    let home = input.home;
    let mut out = String::from(
        "Watt setup snapshot, read now. Secret values, URL paths and queries, header values, command arguments and env values \
         are never shown here.\n",
    );
    out.push_str(&format!("App: Watt {} on {}\n", input.app_version, input.os));
    out.push_str(&format!("Config folder: {}\n", folder_text(input.config_dir, home)));
    out.push_str(&format!("Data folder: {}\n", folder_text(input.data_dir, home)));
    match input.settings {
        None => out.push_str(
            "settings.json: cannot be read. Provider and MCP details are not shown; ask the user to check Settings.\n",
        ),
        Some(view) => settings_lines(&mut out, view, input.mcp, &redactor),
    }
    skill_lines(&mut out, input);
    cap_text(&out, STATE_LIMIT)
}

fn settings_lines(out: &mut String, view: &SettingsView, mcp: &McpStatus, redactor: &Redactor) {
    let settings = &view.settings;
    let state = &view.settings_state;
    match state.kind {
        "broken" => out.push_str(
            "settings.json: broken. Watt does not run on it, so the details below may not be what is in use. The user can see \
             the reason and fix or revert it in Settings.\n",
        ),
        "last_good" => out.push_str(
            "settings.json: broken. Watt runs on the last working copy until the user fixes or reverts the file; the details \
             below are that copy.\n",
        ),
        _ => out.push_str("settings.json: ok\n"),
    }

    out.push_str("\nProviders:\n");
    if settings.providers.is_empty() {
        out.push_str("- none\n");
    }
    for provider in &settings.providers {
        let present = view.secrets.get(&provider.id);
        let api_key = if present.is_some_and(|p| p.api_key) { "set" } else { "not set" };
        let headers: Vec<String> = provider
            .headers
            .iter()
            .map(|h| {
                if h.secret {
                    let set = present.is_some_and(|p| p.headers.contains(&h.name));
                    format!("{} (secret, {})", h.name, if set { "set" } else { "not set" })
                } else {
                    format!("{} ({})", h.name, if h.value.is_empty() { "empty" } else { "set" })
                }
            })
            .collect();
        let models: Vec<&str> = provider.models.iter().map(|m| m.id.as_str()).collect();
        out.push_str(&format!(
            "- {} \"{}\": chat {}; api key {}; stream {}; max tokens {}; headers: {}; models: {}\n",
            provider.id,
            provider.name,
            endpoint(&provider.chat_url, true),
            api_key,
            on_off(provider.stream),
            provider.max_tokens.map_or_else(|| "default".to_string(), |n| n.to_string()),
            list(&headers),
            list(&models),
        ));
    }
    let default = settings.default_model.as_ref().map_or_else(
        || "none".to_string(),
        |m| match &m.variant {
            Some(variant) => format!("{} / {} (variant {variant})", m.provider_id, m.model_id),
            None => format!("{} / {}", m.provider_id, m.model_id),
        },
    );
    out.push_str(&format!("Default model: {default}\n"));

    out.push_str("\nMCP servers:\n");
    if settings.mcp.servers.is_empty() {
        out.push_str("- none\n");
    }
    for server in &settings.mcp.servers {
        let live = mcp.servers.iter().find(|s| s.id == server.name);
        let state = match live {
            Some(live) => state_text(&live.state, redactor),
            None if server.enabled => "not started yet".to_string(),
            None => "disabled".to_string(),
        };
        let transport = match &server.transport {
            McpTransport::Http { url, headers } => {
                format!("http at {}, header names {}", endpoint(url, false), list(headers))
            }
            McpTransport::Stdio { command, args, env, .. } => {
                format!("stdio command {} with {} arguments, env names {}", command_name(command), args.len(), list(env))
            }
        };
        out.push_str(&format!(
            "- {}: {}; {}; approval {}; {} per-tool overrides; state {}\n",
            server.name,
            if server.enabled { "enabled" } else { "disabled in settings" },
            transport,
            level_text(server.approval),
            server.tools.len(),
            state,
        ));
    }

    out.push_str(&format!(
        "\nApprovals: mode {}; {} allow rules, {} deny rules; nested shells {}\n",
        match settings.approval.mode {
            ApprovalMode::Manual => "manual",
            ApprovalMode::Regex => "regex",
        },
        settings.approval.allow.len(),
        settings.approval.deny.len(),
        match settings.approval.nested {
            Nested::Ask => "ask",
            Nested::SameAsLocal => "same as local",
        },
    ));
    out.push_str(&format!(
        "Integration requests: {}\n",
        match settings.integration.agent_requests {
            AgentRequests::Ask => "ask",
            AgentRequests::Auto => "auto",
        }
    ));
    out.push_str(&format!(
        "When the user denies a call: {}\n",
        match settings.agent.on_denied {
            OnDenied::Continue => "continue",
            OnDenied::Stop => "stop",
        }
    ));
    let limits = &settings.limits;
    let calls = if limits.model_calls_unlimited { "unlimited".to_string() } else { limits.max_model_calls.to_string() };
    out.push_str(&format!(
        "Limits per turn: model calls {calls}, polling calls {}, commands {}, wait {} minutes; global context limit {}\n",
        limits.max_poll_calls,
        limits.max_commands_per_turn,
        limits.max_wait_minutes,
        settings.global_context_limit.map_or_else(|| "none".to_string(), |n| n.to_string()),
    ));
}

/// The skills: the folders, the settings that apply to them, and the skills in use. The built-in skill is listed too.
fn skill_lines(out: &mut String, input: &Input) {
    let index = input.skills;
    let home = input.home;
    out.push_str("\nSkills:\n");
    if let Some(view) = input.settings {
        let skills = &view.settings.skills;
        let configured = match &skills.folders {
            None => "defaults (~/.agents/skills, ~/.claude/skills, .agents/skills, .claude/skills)".to_string(),
            Some(folders) => list(&folders.iter().map(|f| shorten(f, home)).collect::<Vec<_>>()),
        };
        out.push_str(&format!("Configured folders: {configured}\n"));
        out.push_str(&format!("Global approval: {}\n", level_word(skills.approval)));
        let overrides: Vec<String> = skills.overrides.iter().map(|(name, level)| format!("{name} {}", level_word(*level))).collect();
        out.push_str(&format!("Per-skill approval: {}\n", list(&overrides)));
        out.push_str(&format!("Switched off in settings: {}\n", list(&skills.disabled)));
    }
    let folders: Vec<String> = index.roots.iter().map(|r| shorten(&r.display().to_string(), home)).collect();
    out.push_str(&format!("Folders scanned: {}\n", list(&folders)));
    out.push_str("Skills in use:\n");
    for skill in &index.skills {
        let mut flags = Vec::new();
        if !skill.enabled {
            flags.push("switched off");
        }
        if !skill.model_invocable {
            flags.push("hidden from the model");
        }
        let flags = if flags.is_empty() { "on".to_string() } else { flags.join(", ") };
        out.push_str(&format!(
            "- {} ({}): {}; approval {}\n",
            skill.name,
            source_text(skill.source),
            flags,
            level_word(skill.level),
        ));
    }
    out.push_str(&format!(
        "Shadowed by a same-named skill earlier in the order: {}. Warnings: {}. Their text is in Settings > Skills.\n",
        index.shadowed.len(),
        index.warnings.len(),
    ));
}

/// The text of a server's live state. Messages are scrubbed and cut.
fn state_text(state: &McpStateView, redactor: &Redactor) -> String {
    match state {
        McpStateView::Disabled => "disabled".to_string(),
        McpStateView::Connecting => "connecting".to_string(),
        McpStateView::Connected { tools } => format!("connected ({tools} tools)"),
        McpStateView::Failed { message } => format!("failed: {}", message_text(message, redactor)),
        McpStateView::NeedsAuth { message } => format!("needs_auth: {}", message_text(message, redactor)),
    }
}

/// A server message as it is shown: scrubbed (known secrets, credential-shaped text, URL query strings), then cut.
fn message_text(text: &str, redactor: &Redactor) -> String {
    scrub(text, redactor).chars().take(MESSAGE_CHARS).collect()
}

/// `scheme://host[:port]` when `with_scheme`, else `host[:port]`. The path, query and userinfo are never shown.
fn endpoint(url: &str, with_scheme: bool) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => match parsed.host_str() {
            Some(host) => {
                let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
                if with_scheme { format!("{}://{host}{port}", parsed.scheme()) } else { format!("{host}{port}") }
            }
            None => "(no host)".to_string(),
        },
        Err(_) => "(unreadable URL)".to_string(),
    }
}

/// The program a stdio server starts: the first word of its command, without its folders.
fn command_name(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or("");
    let name = first.rsplit(['/', '\\']).next().unwrap_or("");
    name.chars().take(COMMAND_CHARS).collect()
}

/// A folder as shown: its home-folder prefix is written `~`.
fn shorten(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home else { return path.to_string() };
    let home = home.display().to_string();
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn folder_text(dir: Option<&Path>, home: Option<&Path>) -> String {
    dir.map_or_else(|| "unknown".to_string(), |d| shorten(&d.display().to_string(), home))
}

fn list<S: AsRef<str>>(items: &[S]) -> String {
    if items.is_empty() { "none".to_string() } else { items.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", ") }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn level_text(level: McpLevel) -> &'static str {
    match level {
        McpLevel::Ask => "ask",
        McpLevel::Auto => "auto",
        McpLevel::Never => "never",
    }
}

fn level_word(level: SkillLevel) -> &'static str {
    match level {
        SkillLevel::Ask => "ask",
        SkillLevel::Auto => "auto",
    }
}

fn source_text(source: Source) -> &'static str {
    match source {
        Source::Builtin => "built in",
        Source::Watt => "Watt folder",
        Source::User => "your folder",
        Source::Project => "project folder",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::core::mcp::config::{McpLevel, McpServerConfig, McpSettings};
    use crate::core::mcp::service::{McpServerStatus, McpStateView, McpStatus};
    use crate::core::settings::service::{SecretsPresent, SettingsStateView, SettingsView};
    use crate::core::settings::{ApprovalSettings, HeaderEntry, Model, ModelRef, ModelSource, Provider, Settings};
    use crate::core::skills::builtin;
    use crate::core::skills::{SkillIndex, SkillsSettings};

    /// Sentinels placed in every sensitive spot of the fixture. None may appear in the snapshot.
    const SENTINELS: [&str; 12] = [
        "SENT_USERINFO",
        "SENT_URL_PATH",
        "SENT_URL_QUERY",
        "SENT_HDR_VALUE",
        "SENT_SECRET_HDR",
        "SENT_ARG",
        "SENT_INLINE",
        "SENT_RULE",
        "SENT_MCP_PATH",
        "SENT_MCP_QUERY",
        "SENT_FAIL_QUERY",
        "SENT_FAIL_BEARER",
    ];

    fn fixture() -> (SettingsView, McpStatus) {
        let provider = Provider {
            id: "prov-one".to_string(),
            name: "Provider One".to_string(),
            chat_url: "https://SENT_USERINFO:pw@api.example.test:8443/v1/SENT_URL_PATH?key=SENT_URL_QUERY".to_string(),
            models_url: Some("https://models.example.test/SENT_URL_PATH?token=SENT_URL_QUERY".to_string()),
            headers: vec![
                HeaderEntry { name: "X-Plain".to_string(), value: "SENT_HDR_VALUE".to_string(), secret: false, previous_name: None },
                HeaderEntry { name: "X-Token".to_string(), value: "SENT_SECRET_HDR".to_string(), secret: true, previous_name: None },
            ],
            stream: true,
            max_tokens: Some(4096),
            models: vec![Model {
                id: "model-a".to_string(),
                context_limit: None,
                provider_context: None,
                vision: None,
                source: ModelSource::Custom,
                variants: Vec::new(),
            }],
        };
        let http = McpServerConfig {
            name: "remote".to_string(),
            enabled: true,
            approval: McpLevel::Auto,
            tools: BTreeMap::from([("search".to_string(), McpLevel::Never)]),
            timeout_secs: None,
            transport: McpTransport::Http {
                url: "https://mcp.example.test/SENT_MCP_PATH?token=SENT_MCP_QUERY".to_string(),
                headers: vec!["Authorization".to_string()],
            },
        };
        let stdio = McpServerConfig {
            name: "local".to_string(),
            enabled: false,
            approval: McpLevel::Ask,
            tools: BTreeMap::new(),
            timeout_secs: None,
            transport: McpTransport::Stdio {
                command: "C:\\tools\\server.exe".to_string(),
                args: vec!["--mode".to_string(), "SENT_ARG".to_string()],
                cwd: None,
                env: vec!["API_TOKEN".to_string()],
            },
        };
        let inline = McpServerConfig {
            name: "inline".to_string(),
            enabled: true,
            approval: McpLevel::Ask,
            tools: BTreeMap::new(),
            timeout_secs: None,
            transport: McpTransport::Stdio {
                command: "npx --token SENT_INLINE".to_string(),
                args: Vec::new(),
                cwd: None,
                env: Vec::new(),
            },
        };
        let settings = Settings {
            providers: vec![provider],
            default_model: Some(ModelRef { provider_id: "prov-one".to_string(), model_id: "model-a".to_string(), variant: None }),
            approval: ApprovalSettings {
                mode: ApprovalMode::Regex,
                allow: vec!["^git status SENT_RULE$".to_string()],
                deny: vec!["SENT_RULE".to_string()],
                nested: Nested::Ask,
            },
            mcp: McpSettings { servers: vec![http, stdio, inline] },
            skills: SkillsSettings { folders: Some(vec!["~/my-skills".to_string()]), ..SkillsSettings::default() },
            ..Settings::default()
        };
        let view = SettingsView {
            settings,
            secrets: BTreeMap::from([(
                "prov-one".to_string(),
                SecretsPresent { api_key: true, headers: vec!["X-Token".to_string()] },
            )]),
            store_kind: "keychain",
            load_error: None,
            secrets_error: None,
            settings_state: SettingsStateView {
                kind: "ok",
                reason: None,
                last_good_available: false,
                path: "C:/Users/someone/AppData/Roaming/watt/settings.json".to_string(),
            },
        };
        let mcp = McpStatus {
            servers: vec![
                McpServerStatus {
                    id: "remote".to_string(),
                    name: "remote".to_string(),
                    enabled: true,
                    state: McpStateView::Failed {
                        message: "connect failed: https://mcp.example.test/x?token=SENT_FAIL_QUERY Bearer SENT_FAIL_BEARER".to_string(),
                    },
                    tools: 0,
                },
                McpServerStatus { id: "local".to_string(), name: "local".to_string(), enabled: false, state: McpStateView::Disabled, tools: 0 },
            ],
            starting: false,
        };
        (view, mcp)
    }

    fn snapshot(view: &SettingsView, mcp: &McpStatus, index: &SkillIndex) -> String {
        render(&Input {
            settings: Some(view),
            mcp,
            skills: index,
            app_version: "0.0.0-test",
            os: "windows",
            config_dir: Some(Path::new("C:/Users/someone/AppData/Roaming/watt")),
            data_dir: None,
            home: Some(Path::new("C:/Users/someone")),
        })
    }

    fn index() -> SkillIndex {
        SkillIndex { skills: vec![builtin::skill()], roots: vec![PathBuf::from("C:/Users/someone/.agents/skills")], ..SkillIndex::default() }
    }

    /// The security gate: no sensitive value of the fixture is in the snapshot, and the allowlisted fields are.
    #[test]
    fn no_sensitive_value_reaches_the_snapshot_and_the_allowed_fields_do() {
        let (view, mcp) = fixture();
        let text = snapshot(&view, &mcp, &index());
        for sentinel in SENTINELS {
            assert!(!text.contains(sentinel), "{sentinel} leaked into the snapshot:\n{text}");
        }
        assert!(!text.contains("pw@") && !text.contains("?key=") && !text.contains("?token="), "{text}");
        assert!(text.contains("prov-one \"Provider One\": chat https://api.example.test:8443; api key set"), "{text}");
        assert!(text.contains("X-Plain (set)") && text.contains("X-Token (secret, set)"), "{text}");
        assert!(text.contains("stream on; max tokens 4096; headers: X-Plain (set), X-Token (secret, set); models: model-a"), "{text}");
        assert!(text.contains("Default model: prov-one / model-a"), "{text}");
        assert!(
            text.contains("- remote: enabled; http at mcp.example.test, header names Authorization; approval auto; 1 per-tool overrides; state failed: connect failed"),
            "{text}"
        );
        assert!(text.contains("- local: disabled in settings; stdio command server.exe with 2 arguments, env names API_TOKEN;"), "{text}");
        assert!(text.contains("- inline: enabled; stdio command npx with 0 arguments"), "{text}");
        assert!(text.contains("mode regex; 1 allow rules, 1 deny rules; nested shells ask"), "{text}");
        assert!(text.contains("Limits per turn: model calls 50"), "{text}");
        assert!(text.contains("Folders scanned: ~/.agents/skills"), "{text}");
        assert!(text.contains("- watt (built in): on; approval auto"), "{text}");
        assert!(text.contains("Configured folders: ~/my-skills"), "{text}");
        assert!(text.contains("Config folder: ~/AppData/Roaming/watt"), "{text}");
    }

    #[test]
    fn the_snapshot_is_deterministic_and_within_its_limit() {
        let (view, mcp) = fixture();
        let first = snapshot(&view, &mcp, &index());
        assert_eq!(first, snapshot(&view, &mcp, &index()));
        assert!(first.len() <= STATE_LIMIT, "{} bytes", first.len());
    }

    #[test]
    fn a_settings_file_that_cannot_be_used_says_so_and_shows_no_details() {
        let (_, mcp) = fixture();
        let text = render(&Input {
            settings: None,
            mcp: &mcp,
            skills: &index(),
            app_version: "0.0.0-test",
            os: "windows",
            config_dir: None,
            data_dir: None,
            home: None,
        });
        assert!(text.contains("settings.json: cannot be read"), "{text}");
        assert!(!text.contains("prov-one") && !text.contains("remote"), "{text}");
        assert!(text.contains("- watt (built in)"), "{text}");
    }

    #[test]
    fn a_broken_file_is_named_as_such_without_its_reason() {
        let (mut view, mcp) = fixture();
        view.settings_state.kind = "last_good";
        view.settings_state.reason = Some("expected value at SENT_REASON".to_string());
        let text = snapshot(&view, &mcp, &index());
        assert!(text.contains("settings.json: broken. Watt runs on the last working copy"), "{text}");
        assert!(!text.contains("SENT_REASON"), "{text}");
    }

    #[test]
    fn the_host_helpers_keep_the_host_and_port_only() {
        assert_eq!(endpoint("https://u:p@h.test:444/a/b?c=d#e", true), "https://h.test:444");
        assert_eq!(endpoint("https://h.test/a?b=c", false), "h.test");
        assert_eq!(endpoint("not a url SENT", true), "(unreadable URL)");
        assert_eq!(command_name("/opt/mcp/server --token SENT_INLINE"), "server");
        assert_eq!(command_name("C:\\x\\y\\tool.exe"), "tool.exe");
    }

    #[test]
    fn folders_under_home_are_shown_with_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(shorten("/home/me/.agents/skills", Some(home)), "~/.agents/skills");
        assert_eq!(shorten("/home/mex/skills", Some(home)), "/home/mex/skills");
        assert_eq!(shorten("/home/me", Some(home)), "~");
        assert_eq!(shorten("/elsewhere", None), "/elsewhere");
    }
}
