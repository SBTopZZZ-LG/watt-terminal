//! AI provider settings: the types, their validation, and the settings file on disk.
//!
//! The file holds everything except secrets. API keys and secret header values live in a `SecretStore` (see
//! `secrets.rs`), so a copy of `settings.json` never contains a credential. `load` and `save` are the only functions
//! here that touch the disk. `service.rs` is the one place that changes settings at runtime and is what the app uses.

pub mod models;
pub mod presets;
pub mod resolve;
pub mod secrets;
pub mod service;

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::core::approval;

pub const SETTINGS_VERSION: u32 = 1;
const SETTINGS_FILE: &str = "settings.json";
/// The hidden copy of the last settings that checked out at a launch. Written by the app only, with the same mode.
const LAST_GOOD_FILE: &str = ".settings.last-good.json";

/// Longest accepted provider name, in characters.
const MAX_NAME_CHARS: usize = 80;
/// Longest accepted model id, in characters.
const MAX_MODEL_ID_CHARS: usize = 200;
/// Longest accepted non-secret header value, in characters.
const MAX_HEADER_VALUE_CHARS: usize = 500;
/// Longest accepted provider id, in characters.
const MAX_PROVIDER_ID_CHARS: usize = 40;

/// The whole settings document. Written to `settings.json` by `save`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub default_model: Option<ModelRef>,
    /// Tokens. `None` means no cap.
    #[serde(default)]
    pub global_context_limit: Option<u32>,
    /// How `run_command` is approved (core/approval). Missing in older files: manual.
    #[serde(default)]
    pub approval: ApprovalSettings,
    /// How the agent's shell integration requests are answered (`request_integration`). Missing in older files: ask.
    #[serde(default)]
    pub integration: IntegrationSettings,
    /// The per-turn caps of the agent (`LimitsSettings`). Missing in older files: the defaults.
    #[serde(default)]
    pub limits: LimitsSettings,
}

fn current_version() -> u32 {
    SETTINGS_VERSION
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            providers: Vec::new(),
            default_model: None,
            global_context_limit: None,
            approval: ApprovalSettings::default(),
            integration: IntegrationSettings::default(),
            limits: LimitsSettings::default(),
        }
    }
}

/// The per-turn caps of the agent (core/chat/agent_loop, core/tools). Missing fields take their defaults. A saved number out
/// of range makes the whole file invalid, like any other bad setting (see `validate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsSettings {
    /// Model calls in one turn that are not polling (see `max_poll_calls`). At least 10, no upper bound. Still checked while
    /// `model_calls_unlimited` is on.
    pub max_model_calls: u32,
    /// No cap on the model calls above. Its own switch, so the number is kept while it is on.
    pub model_calls_unlimited: bool,
    /// Polling model calls in one turn: a reply whose calls are all wait, get_screen or send_keys while a command the agent
    /// started is still running. 10 to 500.
    pub max_poll_calls: u32,
    /// `run_command` calls in one turn. 1 to 500.
    pub max_commands_per_turn: u32,
    /// Minutes `wait` may spend in one turn. 1 to 720.
    pub max_wait_minutes: u32,
}

impl Default for LimitsSettings {
    fn default() -> Self {
        Self {
            max_model_calls: 50,
            model_calls_unlimited: false,
            max_poll_calls: 60,
            max_commands_per_turn: 30,
            max_wait_minutes: 30,
        }
    }
}

impl LimitsSettings {
    /// The model-call cap of one turn, or `None` when it is unlimited.
    pub fn model_call_cap(&self) -> Option<u32> {
        (!self.model_calls_unlimited).then_some(self.max_model_calls)
    }

    /// The longest `wait` may spend in one turn.
    pub fn max_wait(&self) -> Duration {
        Duration::from_secs(u64::from(self.max_wait_minutes) * 60)
    }

    /// Every problem found, one human-readable string each.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.max_model_calls < 10 {
            problems.push("limits: model calls per turn must be at least 10".into());
        }
        if !(10..=500).contains(&self.max_poll_calls) {
            problems.push("limits: polling calls per turn must be 10 to 500".into());
        }
        if !(1..=500).contains(&self.max_commands_per_turn) {
            problems.push("limits: commands per turn must be 1 to 500".into());
        }
        if !(1..=720).contains(&self.max_wait_minutes) {
            problems.push("limits: wait minutes per turn must be 1 to 720".into());
        }
        problems
    }
}

/// Whether `run_command` calls wait for the user. Approve-all is not here: it is session-only and never saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Every call waits for the user.
    #[default]
    Manual,
    /// The hard gate, then the deny and allow rules, decide (see core/approval).
    Regex,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ApprovalSettings {
    #[serde(default)]
    pub mode: ApprovalMode,
    /// Regular expressions. A command is allowed when one matches the whole command. Used in regex mode.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Regular expressions, matched the same way. A deny match wins over an allow match.
    #[serde(default)]
    pub deny: Vec<String>,
    /// How a nested shell (ssh, a shell the app integrated) is approved. `ask`: its commands always need a card, and approve-all
    /// applies only in the shell it was switched on in. `same_as_local`: the rules and approve-all apply there as they do locally.
    /// The hard gate and the deny rules apply everywhere.
    #[serde(default)]
    pub nested: Nested,
}

/// How commands in a nested shell are approved. See `ApprovalSettings::nested`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nested {
    /// Allow rules ask in a nested shell, and approve-all is tied to the shell it was switched on in.
    #[default]
    Ask,
    /// Allow rules and approve-all apply in a nested shell as they do locally.
    SameAsLocal,
}

/// The shell integration settings (core/inject, `request_integration`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IntegrationSettings {
    #[serde(default)]
    pub agent_requests: AgentRequests,
}

/// What the agent's `request_integration` call does. `ask`: the user confirms each one. `auto`: the shell is integrated
/// without a dialog. Approve-all never implies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRequests {
    #[default]
    Ask,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    /// Slug: `[a-z0-9][a-z0-9_-]{0,39}`. Also the prefix of this provider's secret names.
    pub id: String,
    pub name: String,
    /// The full chat-completions URL.
    pub chat_url: String,
    /// The full model-list URL, if the provider has one.
    #[serde(default)]
    pub models_url: Option<String>,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
    #[serde(default = "default_stream")]
    pub stream: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub models: Vec<Model>,
}

fn default_stream() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderEntry {
    pub name: String,
    /// The header value. May contain the `{session_id}` placeholder. For a secret header this is always `""` in the
    /// file; the real value is in the `SecretStore`.
    pub value: String,
    #[serde(default)]
    pub secret: bool,
    /// Input only, from the settings UI: the name this header had when it was saved. A secret header renamed with no
    /// typed value keeps its stored value under the new name (see `SettingsService::save_provider`). Never written.
    #[serde(default, skip_serializing)]
    pub previous_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    /// The user's override, in tokens.
    #[serde(default)]
    pub context_limit: Option<u32>,
    /// What the provider reported when the model list was fetched, in tokens.
    #[serde(default)]
    pub provider_context: Option<u32>,
    pub source: ModelSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Fetched,
    Custom,
}

impl Settings {
    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Every problem found, one human-readable string each. Never includes a secret value.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();

        if self.version != SETTINGS_VERSION {
            problems.push(format!("unsupported settings version {}", self.version));
        }

        let mut seen_ids = HashSet::new();
        for provider in &self.providers {
            if !valid_provider_id(&provider.id) {
                problems.push(format!(
                    "provider id {:?} must be 1 to {MAX_PROVIDER_ID_CHARS} characters of a-z, 0-9, '-' or '_', \
                     starting with a-z or 0-9",
                    provider.id
                ));
            }
            if !seen_ids.insert(provider.id.as_str()) {
                problems.push(format!("provider id {:?} is used more than once", provider.id));
            }
            validate_provider(provider, &mut problems);
        }

        if let Some(default) = &self.default_model {
            match self.provider(&default.provider_id) {
                None => problems.push(format!("default model refers to unknown provider {:?}", default.provider_id)),
                Some(provider) if provider.model(&default.model_id).is_none() => problems.push(format!(
                    "default model {:?} is not listed by provider {:?}",
                    default.model_id, default.provider_id
                )),
                Some(_) => {}
            }
        }

        if self.global_context_limit == Some(0) {
            problems.push("global context limit must be at least 1".into());
        }

        problems.extend(approval::validate_settings(&self.approval));
        problems.extend(self.limits.validate());

        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }
}

impl Provider {
    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The context limit that applies to `model_id`: the smallest of the global cap, the user's override and the
    /// provider's reported size, counting only the values that are set. `None` when none of them is set.
    pub fn effective_context_limit(&self, model_id: &str, global: Option<u32>) -> Option<u32> {
        let model = self.model(model_id);
        [global, model.and_then(|m| m.context_limit), model.and_then(|m| m.provider_context)]
            .into_iter()
            .flatten()
            .min()
    }
}

fn validate_provider(provider: &Provider, problems: &mut Vec<String>) {
    let label = format!("provider {:?}", provider.id);

    let name = provider.name.trim();
    if name.is_empty() {
        problems.push(format!("{label}: name must not be empty"));
    } else if provider.name.chars().count() > MAX_NAME_CHARS {
        problems.push(format!("{label}: name must be at most {MAX_NAME_CHARS} characters"));
    } else if provider.name.chars().any(char::is_control) {
        problems.push(format!("{label}: name must not contain control characters"));
    }

    validate_url(&label, "chat URL", &provider.chat_url, problems);
    if let Some(models_url) = &provider.models_url {
        validate_url(&label, "models URL", models_url, problems);
    }

    let mut seen_headers = HashSet::new();
    for header in &provider.headers {
        validate_header(&label, header, problems);
        if !seen_headers.insert(header.name.to_ascii_lowercase()) {
            problems.push(format!("{label}: header {:?} is listed more than once", header.name));
        }
    }

    if provider.max_tokens == Some(0) {
        problems.push(format!("{label}: max tokens must be at least 1"));
    }

    let mut seen_models = HashSet::new();
    for model in &provider.models {
        validate_model(&label, model, problems);
        if !seen_models.insert(model.id.as_str()) {
            problems.push(format!("{label}: model {:?} is listed more than once", model.id));
        }
    }
}

fn validate_url(label: &str, field: &str, url: &str, problems: &mut Vec<String>) {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        problems.push(format!("{label}: {field} must not contain whitespace or control characters"));
        return;
    }
    let parsed = match Url::parse(url) {
        Ok(parsed) => parsed,
        Err(_) => {
            problems.push(format!("{label}: {field} is not a valid URL"));
            return;
        }
    };
    match parsed.scheme() {
        "https" => {}
        "http" => {
            let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            if !local {
                problems.push(format!(
                    "{label}: {field} uses plain http; only localhost, 127.0.0.1 and [::1] may"
                ));
            }
        }
        _ => problems.push(format!("{label}: {field} must use http or https")),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        problems.push(format!("{label}: {field} must not contain user name or password"));
    }
    if parsed.fragment().is_some() {
        problems.push(format!("{label}: {field} must not contain a fragment"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        problems.push(format!("{label}: {field} must have a host"));
    }
}

fn validate_header(label: &str, header: &HeaderEntry, problems: &mut Vec<String>) {
    let name = &header.name;
    if name.is_empty() || !name.bytes().all(is_token_byte) {
        problems.push(format!("{label}: header name {name:?} is not a valid HTTP header name"));
        return;
    }
    if name.eq_ignore_ascii_case("authorization") && !header.secret {
        problems.push(format!(
            "{label}: the Authorization header must be a secret so its value stays out of settings.json"
        ));
    }
    if header.secret {
        if !header.value.is_empty() {
            problems.push(format!("{label}: secret header {name:?} must not store its value in settings.json"));
        }
    } else {
        if header.value.chars().any(char::is_control) {
            problems.push(format!("{label}: value of header {name:?} must not contain control characters"));
        }
        if header.value.chars().count() > MAX_HEADER_VALUE_CHARS {
            problems.push(format!(
                "{label}: value of header {name:?} must be at most {MAX_HEADER_VALUE_CHARS} characters"
            ));
        }
    }
}

fn validate_model(label: &str, model: &Model, problems: &mut Vec<String>) {
    if model.id.trim().is_empty() {
        problems.push(format!("{label}: model id must not be empty"));
    } else if model.id.chars().count() > MAX_MODEL_ID_CHARS {
        problems.push(format!("{label}: model id must be at most {MAX_MODEL_ID_CHARS} characters"));
    } else if model.id.chars().any(char::is_control) {
        problems.push(format!("{label}: model id must not contain control characters"));
    }
    if model.context_limit == Some(0) {
        problems.push(format!("{label}: context limit of model {:?} must be at least 1", model.id));
    }
    if model.provider_context == Some(0) {
        problems.push(format!("{label}: provider context of model {:?} must be at least 1", model.id));
    }
}

/// `[a-z0-9][a-z0-9_-]{0,39}`
fn valid_provider_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=MAX_PROVIDER_ID_CHARS).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}

/// RFC 7230 `tchar`.
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// The file cannot be used: unreadable, not valid settings JSON, or failing validation. The file is not touched.
    Invalid(String),
    /// The file could not be written. The previous file, if any, is still in place.
    Io(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Invalid(msg) => write!(f, "settings are invalid: {msg}"),
            SettingsError::Io(msg) => write!(f, "settings could not be saved: {msg}"),
        }
    }
}

impl std::error::Error for SettingsError {}

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE)
}

/// The path of the hidden last-good copy of the settings.
pub fn last_good_path(config_dir: &Path) -> PathBuf {
    config_dir.join(LAST_GOOD_FILE)
}

/// Reads `config_dir/settings.json`. A missing file gives the default (empty) settings. Anything else that is wrong
/// with the file is an error, and the file is left exactly as it was.
pub fn load(config_dir: &Path) -> Result<Settings, SettingsError> {
    Ok(read_checked(&settings_path(config_dir))?.map_or_else(Settings::default, |(settings, _)| settings))
}

/// Reads and checks one settings file. `Ok(None)` when it does not exist. Otherwise also returns the file's exact
/// bytes, so a copy of the file is the text that was checked. The file is not touched.
pub fn read_checked(path: &Path) -> Result<Option<(Settings, Vec<u8>)>, SettingsError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SettingsError::Invalid(format!("cannot read {}: {e}", path.display()))),
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| SettingsError::Invalid(format!("cannot read {}: {e}", path.display())))?;
    // The serde message can echo a value from the file, so only the position and the error class are reported.
    let settings: Settings = serde_json::from_str(text).map_err(|e| {
        SettingsError::Invalid(format!(
            "not valid JSON ({:?} error at line {}, column {})",
            e.classify(),
            e.line(),
            e.column()
        ))
    })?;
    settings.validate().map_err(|problems| SettingsError::Invalid(problems.join("; ")))?;
    Ok(Some((settings, bytes)))
}

/// Validates, then replaces `config_dir/settings.json` atomically. The directory is created (0700) if missing. The
/// file is written with mode 0600.
pub fn save(config_dir: &Path, settings: &Settings) -> Result<(), SettingsError> {
    settings.validate().map_err(|problems| SettingsError::Invalid(problems.join("; ")))?;
    let mut json = serde_json::to_string_pretty(settings)
        .map_err(|e| SettingsError::Io(format!("cannot encode settings: {e}")))?;
    json.push('\n');
    write_private_atomic(&settings_path(config_dir), json.as_bytes())
        .map_err(|e| SettingsError::Io(e.to_string()))
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` through a temp file in the same directory: create (mode 0600), write, fsync, rename. A
/// crash leaves either the old file or the new one, never a partial one. The temp file is removed if anything fails.
/// The directory is created with mode 0700 if it does not exist.
pub(crate) fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    ensure_private_dir(dir)?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{file_name}.{}.{counter}.tmp", std::process::id()));

    let mut file = open_private_new(&tmp)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    set_private_dir_mode(&mut builder);
    builder.create(dir)
}

fn open_private_new(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_private_file_mode(&mut options);
    options.open(path)
}

#[cfg(unix)]
fn set_private_dir_mode(builder: &mut DirBuilder) {
    use std::os::unix::fs::DirBuilderExt;
    builder.mode(0o700);
}

#[cfg(not(unix))]
fn set_private_dir_mode(_: &mut DirBuilder) {}

#[cfg(unix)]
fn set_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_mode(_: &mut OpenOptions) {}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("watt-terminal-test-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::TempDir;
    use super::*;

    fn model(id: &str) -> Model {
        Model { id: id.into(), context_limit: None, provider_context: None, source: ModelSource::Custom }
    }

    fn provider(id: &str) -> Provider {
        Provider {
            id: id.into(),
            name: "Example".into(),
            chat_url: "https://api.example.com/v1/chat/completions".into(),
            models_url: Some("https://api.example.com/v1/models".into()),
            headers: Vec::new(),
            stream: true,
            max_tokens: None,
            models: vec![model("big-1")],
        }
    }

    fn valid() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            providers: vec![provider("example")],
            default_model: Some(ModelRef { provider_id: "example".into(), model_id: "big-1".into() }),
            global_context_limit: Some(32_000),
            approval: ApprovalSettings::default(),
            integration: IntegrationSettings::default(),
            limits: LimitsSettings::default(),
        }
    }

    fn header(name: &str, value: &str, secret: bool) -> HeaderEntry {
        HeaderEntry { name: name.into(), value: value.into(), secret, previous_name: None }
    }

    /// Applies `change` to a valid settings value and returns what validation says.
    fn problems_after(change: impl FnOnce(&mut Settings)) -> Vec<String> {
        let mut settings = valid();
        change(&mut settings);
        settings.validate().err().unwrap_or_default()
    }

    #[test]
    fn valid_settings_pass() {
        assert_eq!(valid().validate(), Ok(()));
        assert_eq!(Settings::default().validate(), Ok(()));
    }

    #[test]
    fn invalid_cases_are_reported() {
        // Each case: a change to the valid settings, and a fragment that the problem message must contain.
        type Case = (&'static str, Box<dyn Fn(&mut Settings)>, &'static str);
        let cases: Vec<Case> = vec![
            ("empty id", Box::new(|s| s.providers[0].id = "".into()), "provider id"),
            ("space in id", Box::new(|s| s.providers[0].id = "bad id".into()), "provider id"),
            ("upper case id", Box::new(|s| s.providers[0].id = "Example".into()), "provider id"),
            ("id starts with dash", Box::new(|s| s.providers[0].id = "-x".into()), "provider id"),
            ("id too long", Box::new(|s| s.providers[0].id = "a".repeat(41)), "provider id"),
            ("version 2", Box::new(|s| s.version = 2), "version"),
            (
                "duplicate id",
                Box::new(|s| s.providers.push(provider("example"))),
                "used more than once",
            ),
            ("empty name", Box::new(|s| s.providers[0].name = "   ".into()), "name must not be empty"),
            ("long name", Box::new(|s| s.providers[0].name = "n".repeat(81)), "at most 80"),
            ("control in name", Box::new(|s| s.providers[0].name = "a\nb".into()), "control characters"),
            ("ftp chat url", Box::new(|s| s.providers[0].chat_url = "ftp://x.example/v1".into()), "http or https"),
            ("not a url", Box::new(|s| s.providers[0].chat_url = "not a url".into()), "whitespace"),
            ("unparseable url", Box::new(|s| s.providers[0].chat_url = "https://[bad".into()), "not a valid URL"),
            (
                "userinfo in url",
                Box::new(|s| s.providers[0].chat_url = "https://user:pw@api.example.com/v1".into()),
                "user name or password",
            ),
            (
                "fragment in url",
                Box::new(|s| s.providers[0].chat_url = "https://api.example.com/v1#x".into()),
                "fragment",
            ),
            ("no host", Box::new(|s| s.providers[0].chat_url = "https://".into()), "not a valid URL"),
            (
                "plain http to a remote host",
                Box::new(|s| s.providers[0].chat_url = "http://api.example.com/v1".into()),
                "plain http",
            ),
            (
                "bad models url",
                Box::new(|s| s.providers[0].models_url = Some("ftp://x.example/models".into())),
                "models URL",
            ),
            (
                "bad header name",
                Box::new(|s| s.providers[0].headers = vec![header("bad name", "x", false)]),
                "header name",
            ),
            ("empty header name", Box::new(|s| s.providers[0].headers = vec![header("", "x", false)]), "header name"),
            (
                "duplicate header ignoring case",
                Box::new(|s| {
                    s.providers[0].headers = vec![header("X-A", "1", false), header("x-a", "2", false)];
                }),
                "more than once",
            ),
            (
                "plain Authorization header",
                Box::new(|s| s.providers[0].headers = vec![header("authorization", "Bearer x", false)]),
                "Authorization",
            ),
            (
                "secret header with a value in the file",
                Box::new(|s| s.providers[0].headers = vec![header("X-Key", "abc123", true)]),
                "must not store its value",
            ),
            (
                "control character in header value",
                Box::new(|s| s.providers[0].headers = vec![header("X-A", "a\r\nb", false)]),
                "control characters",
            ),
            (
                "header value too long",
                Box::new(|s| s.providers[0].headers = vec![header("X-A", &"v".repeat(501), false)]),
                "at most 500",
            ),
            ("empty model id", Box::new(|s| s.providers[0].models[0].id = "  ".into()), "model id must not be empty"),
            ("long model id", Box::new(|s| s.providers[0].models[0].id = "m".repeat(201)), "at most 200"),
            ("control in model id", Box::new(|s| s.providers[0].models[0].id = "m\t1".into()), "control characters"),
            (
                "duplicate model id",
                Box::new(|s| s.providers[0].models.push(model("big-1"))),
                "listed more than once",
            ),
            (
                "zero model context limit",
                Box::new(|s| s.providers[0].models[0].context_limit = Some(0)),
                "at least 1",
            ),
            (
                "zero provider context",
                Box::new(|s| s.providers[0].models[0].provider_context = Some(0)),
                "at least 1",
            ),
            ("zero max tokens", Box::new(|s| s.providers[0].max_tokens = Some(0)), "max tokens"),
            ("zero global limit", Box::new(|s| s.global_context_limit = Some(0)), "global context limit"),
            (
                "default model with unknown provider",
                Box::new(|s| s.default_model = Some(ModelRef { provider_id: "nope".into(), model_id: "big-1".into() })),
                "unknown provider",
            ),
            (
                "default model not listed",
                Box::new(|s| {
                    s.default_model = Some(ModelRef { provider_id: "example".into(), model_id: "nope".into() })
                }),
                "not listed",
            ),
            (
                "approval allow rule that does not compile",
                Box::new(|s| s.approval.allow = vec!["git status".into(), "a(".into()]),
                "approval allow rule 2",
            ),
            (
                "approval deny rule with a line break",
                Box::new(|s| s.approval.deny = vec!["rm\n.*".into()]),
                "approval deny rule 1",
            ),
        ];
        for (label, change, fragment) in cases {
            let problems = problems_after(|s| change(s));
            assert!(!problems.is_empty(), "{label}: expected a problem");
            assert!(
                problems.iter().any(|p| p.contains(fragment)),
                "{label}: no problem mentions {fragment:?}: {problems:?}"
            );
        }
    }

    #[test]
    fn edge_values_that_are_valid() {
        type Change = Box<dyn Fn(&mut Settings)>;
        let cases: Vec<(&str, Change)> = vec![
            ("40 character id", Box::new(|s| {
                s.providers[0].id = "a".repeat(40);
                s.default_model = None;
            })),
            ("digit first id with dash and underscore", Box::new(|s| {
                s.providers[0].id = "1-a_b".into();
                s.default_model = None;
            })),
            ("localhost http", Box::new(|s| s.providers[0].chat_url = "http://localhost:11434/v1".into())),
            ("127.0.0.1 http", Box::new(|s| s.providers[0].chat_url = "http://127.0.0.1:8080/v1".into())),
            ("[::1] http", Box::new(|s| s.providers[0].chat_url = "http://[::1]:8080/v1".into())),
            (
                "header value of 500 characters",
                Box::new(|s| s.providers[0].headers = vec![header("X-A", &"v".repeat(500), false)]),
            ),
            (
                "placeholder in header",
                Box::new(|s| s.providers[0].headers = vec![header("X-Session", "{session_id}", false)]),
            ),
            (
                "secret Authorization with empty value",
                Box::new(|s| s.providers[0].headers = vec![header("Authorization", "", true)]),
            ),
            ("secret header with empty value", Box::new(|s| s.providers[0].headers = vec![header("X-Key", "", true)])),
            ("context limit of 1", Box::new(|s| s.providers[0].models[0].context_limit = Some(1))),
            ("same model id in two providers", Box::new(|s| s.providers.push(provider("other")))),
            ("same header name in two providers", Box::new(|s| {
                s.providers[0].headers = vec![header("X-A", "1", false)];
                let mut other = provider("other");
                other.headers = vec![header("X-A", "2", false)];
                s.providers.push(other);
            })),
            ("no models url, no default model", Box::new(|s| {
                s.providers[0].models_url = None;
                s.default_model = None;
                s.global_context_limit = None;
            })),
        ];
        for (label, change) in cases {
            assert_eq!(problems_after(|s| change(s)), Vec::<String>::new(), "{label} should be valid");
        }
    }

    #[test]
    fn problem_messages_never_contain_secret_values() {
        let secret = "sk-test-DUMMY-value-42";
        let problems = problems_after(|s| {
            s.providers[0].headers = vec![header("X-Key", secret, true)];
        });
        assert!(!problems.is_empty());
        assert!(problems.iter().all(|p| !p.contains(secret)), "a problem message echoed the value: {problems:?}");
    }

    #[test]
    fn effective_context_limit_takes_the_smallest_set_value() {
        let mut p = provider("example");
        p.models = vec![Model {
            id: "m".into(),
            context_limit: None,
            provider_context: None,
            source: ModelSource::Fetched,
        }];
        assert_eq!(p.effective_context_limit("m", None), None);
        assert_eq!(p.effective_context_limit("m", Some(8_000)), Some(8_000));

        p.models[0].context_limit = Some(32_000);
        assert_eq!(p.effective_context_limit("m", Some(8_000)), Some(8_000));
        p.models[0].context_limit = Some(4_000);
        assert_eq!(p.effective_context_limit("m", Some(8_000)), Some(4_000));
        assert_eq!(p.effective_context_limit("m", None), Some(4_000));

        p.models[0].context_limit = None;
        p.models[0].provider_context = Some(16_000);
        assert_eq!(p.effective_context_limit("m", None), Some(16_000));
        p.models[0].context_limit = Some(20_000);
        assert_eq!(p.effective_context_limit("m", Some(30_000)), Some(16_000));

        // An unknown model only sees the global cap.
        assert_eq!(p.effective_context_limit("missing", None), None);
        assert_eq!(p.effective_context_limit("missing", Some(8_000)), Some(8_000));
    }

    #[test]
    fn round_trip_through_the_file() {
        let dir = TempDir::new();
        save(dir.path(), &valid()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), valid());
    }

    #[test]
    fn missing_file_loads_as_default() {
        let dir = TempDir::new();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, Settings::default());
        assert_eq!(loaded.version, SETTINGS_VERSION);
        assert!(loaded.providers.is_empty());
    }

    #[test]
    fn corrupt_file_is_refused_and_untouched() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let path = settings_path(dir.path());
        fs::write(&path, b"{ this is not json").unwrap();
        assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(_))));
        assert_eq!(fs::read(&path).unwrap(), b"{ this is not json");
    }

    #[test]
    fn invalid_file_is_refused_and_untouched() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let path = settings_path(dir.path());
        fs::write(&path, br#"{"version":1,"providers":[],"global_context_limit":0}"#).unwrap();
        assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(msg)) if msg.contains("global context limit")));
        assert_eq!(fs::read(&path).unwrap(), br#"{"version":1,"providers":[],"global_context_limit":0}"#);
    }

    #[test]
    fn failed_validation_leaves_the_old_file_intact() {
        let dir = TempDir::new();
        save(dir.path(), &valid()).unwrap();
        let before = fs::read(settings_path(dir.path())).unwrap();

        let mut bad = valid();
        bad.providers[0].chat_url = "http://api.example.com/v1".into();
        assert!(matches!(save(dir.path(), &bad), Err(SettingsError::Invalid(_))));

        assert_eq!(fs::read(settings_path(dir.path())).unwrap(), before);
        assert_eq!(load(dir.path()).unwrap(), valid());
    }

    #[test]
    fn unknown_fields_are_ignored_and_older_files_get_defaults() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let json = r#"{
            "version": 1,
            "future_top_level": {"anything": [1, 2, 3]},
            "providers": [{
                "id": "example",
                "name": "Example",
                "chat_url": "https://api.example.com/v1/chat/completions",
                "color": "teal",
                "headers": [{"name": "X-A", "value": "1", "future": true}],
                "models": [{"id": "m", "source": "custom", "tags": []}]
            }]
        }"#;
        fs::write(settings_path(dir.path()), json).unwrap();
        let loaded = load(dir.path()).unwrap();
        let p = &loaded.providers[0];
        assert!(p.stream, "stream defaults to true");
        assert_eq!(p.models_url, None);
        assert_eq!(p.max_tokens, None);
        assert!(!p.headers[0].secret, "secret defaults to false");
        assert_eq!(p.models[0].context_limit, None);
        assert_eq!(loaded.default_model, None);
        assert_eq!(loaded.global_context_limit, None);
    }

    #[test]
    fn saved_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        save(dir.path(), &valid()).unwrap();
        let mode = fs::metadata(settings_path(dir.path())).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn missing_config_dir_is_created_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let config = dir.path().join("config");
        save(&config, &valid()).unwrap();
        let mode = fs::metadata(&config).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let dir = TempDir::new();
        save(dir.path(), &valid()).unwrap();
        save(dir.path(), &valid()).unwrap();
        let names: Vec<String> =
            fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![SETTINGS_FILE.to_string()]);
    }

    #[test]
    fn approval_settings_round_trip_through_the_file() {
        let dir = TempDir::new();
        let mut settings = valid();
        settings.approval = ApprovalSettings {
            mode: ApprovalMode::Regex,
            allow: vec!["git status".into(), "cargo (test|check)( .*)?".into()],
            deny: vec!["cargo publish.*".into()],
            ..Default::default()
        };
        save(dir.path(), &settings).unwrap();
        let text = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(text.contains(r#""mode": "regex""#), "{text}");
        assert_eq!(load(dir.path()).unwrap(), settings);
    }

    #[test]
    fn a_file_without_approval_loads_as_manual_with_no_rules() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(settings_path(dir.path()), br#"{"version":1,"providers":[],"global_context_limit":4000}"#).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.approval, ApprovalSettings::default());
        assert_eq!(loaded.approval.mode, ApprovalMode::Manual);
        assert!(loaded.approval.allow.is_empty() && loaded.approval.deny.is_empty());
    }

    #[test]
    fn nested_and_integration_settings_round_trip_and_default_to_ask() {
        let dir = TempDir::new();
        let mut settings = valid();
        settings.approval.nested = Nested::SameAsLocal;
        settings.integration.agent_requests = AgentRequests::Auto;
        save(dir.path(), &settings).unwrap();
        let text = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(text.contains(r#""nested": "same_as_local""#), "{text}");
        assert!(text.contains(r#""agent_requests": "auto""#), "{text}");
        assert_eq!(load(dir.path()).unwrap(), settings);

        assert_eq!(Settings::default().approval.nested, Nested::Ask);
        assert_eq!(Settings::default().integration.agent_requests, AgentRequests::Ask);
    }

    #[test]
    fn an_older_file_loads_with_nested_and_integration_at_their_defaults() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let json = br#"{"version":1,"approval":{"mode":"regex","allow":["git status"]}}"#;
        fs::write(settings_path(dir.path()), json).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.approval.nested, Nested::Ask);
        assert_eq!(loaded.integration, IntegrationSettings::default());
        assert_eq!(loaded.integration.agent_requests, AgentRequests::Ask);
    }

    #[test]
    fn an_unknown_nested_or_integration_value_makes_the_file_invalid() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        for json in [
            &br#"{"version":1,"approval":{"nested":"always"}}"#[..],
            &br#"{"version":1,"integration":{"agent_requests":"yes"}}"#[..],
        ] {
            fs::write(settings_path(dir.path()), json).unwrap();
            assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(_))));
        }
    }

    #[test]
    fn approve_all_is_not_a_saved_mode() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let json = br#"{"version":1,"approval":{"mode":"approve_all"}}"#;
        fs::write(settings_path(dir.path()), json).unwrap();
        assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(_))));
    }

    #[test]
    fn an_invalid_approval_rule_makes_the_whole_file_invalid_and_is_left_alone() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let json = br#"{"version":1,"approval":{"mode":"regex","allow":["a)|(b"]}}"#;
        fs::write(settings_path(dir.path()), json).unwrap();
        assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(msg)) if msg.contains("approval allow rule 1")));
        assert_eq!(fs::read(settings_path(dir.path())).unwrap(), json);
    }

    #[test]
    fn limits_default_to_the_documented_values() {
        let expected = LimitsSettings {
            max_model_calls: 50,
            model_calls_unlimited: false,
            max_poll_calls: 60,
            max_commands_per_turn: 30,
            max_wait_minutes: 30,
        };
        assert_eq!(LimitsSettings::default(), expected);
        assert_eq!(Settings::default().limits, expected);
        assert_eq!(expected.model_call_cap(), Some(50));
        assert_eq!(expected.max_wait(), Duration::from_secs(30 * 60));
    }

    #[test]
    fn limits_round_trip_through_the_file() {
        let dir = TempDir::new();
        let mut settings = valid();
        settings.limits = LimitsSettings {
            max_model_calls: u32::MAX,
            model_calls_unlimited: true,
            max_poll_calls: 500,
            max_commands_per_turn: 1,
            max_wait_minutes: 720,
        };
        save(dir.path(), &settings).unwrap();
        let text = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(text.contains(r#""model_calls_unlimited": true"#), "{text}");
        assert!(text.contains(r#""max_model_calls": 4294967295"#), "{text}");
        assert_eq!(load(dir.path()).unwrap(), settings);
    }

    #[test]
    fn an_older_file_loads_with_the_default_limits_and_a_partial_limits_section_keeps_the_rest() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(settings_path(dir.path()), br#"{"version":1,"providers":[],"global_context_limit":4000}"#).unwrap();
        assert_eq!(load(dir.path()).unwrap().limits, LimitsSettings::default());

        fs::write(settings_path(dir.path()), br#"{"version":1,"limits":{"max_poll_calls":200}}"#).unwrap();
        let loaded = load(dir.path()).unwrap().limits;
        assert_eq!(loaded, LimitsSettings { max_poll_calls: 200, ..LimitsSettings::default() });
    }

    #[test]
    fn model_call_cap_is_none_only_when_unlimited() {
        let limited = LimitsSettings { max_model_calls: 10, ..LimitsSettings::default() };
        assert_eq!(limited.model_call_cap(), Some(10));
        let unlimited = LimitsSettings { model_calls_unlimited: true, ..limited };
        assert_eq!(unlimited.model_call_cap(), None);
        assert_eq!(LimitsSettings { max_wait_minutes: 2, ..LimitsSettings::default() }.max_wait(), Duration::from_secs(120));
    }

    /// The limits edges: each case is (label, limits, whether the file is valid).
    #[test]
    fn limits_edges_are_checked_on_their_ranges() {
        let base = LimitsSettings::default();
        let cases = [
            ("model calls 10", LimitsSettings { max_model_calls: 10, ..base }, true),
            ("model calls 9", LimitsSettings { max_model_calls: 9, ..base }, false),
            ("model calls 0", LimitsSettings { max_model_calls: 0, ..base }, false),
            ("model calls u32::MAX, no upper bound", LimitsSettings { max_model_calls: u32::MAX, ..base }, true),
            ("model calls 200 (a warning in the UI, not an error)", LimitsSettings { max_model_calls: 200, ..base }, true),
            ("model calls 9 while unlimited is still checked", LimitsSettings { max_model_calls: 9, model_calls_unlimited: true, ..base }, false),
            ("unlimited with a valid number", LimitsSettings { model_calls_unlimited: true, ..base }, true),
            ("polling 10", LimitsSettings { max_poll_calls: 10, ..base }, true),
            ("polling 9", LimitsSettings { max_poll_calls: 9, ..base }, false),
            ("polling 500", LimitsSettings { max_poll_calls: 500, ..base }, true),
            ("polling 501", LimitsSettings { max_poll_calls: 501, ..base }, false),
            ("commands 1", LimitsSettings { max_commands_per_turn: 1, ..base }, true),
            ("commands 0", LimitsSettings { max_commands_per_turn: 0, ..base }, false),
            ("commands 500", LimitsSettings { max_commands_per_turn: 500, ..base }, true),
            ("commands 501", LimitsSettings { max_commands_per_turn: 501, ..base }, false),
            ("wait 1", LimitsSettings { max_wait_minutes: 1, ..base }, true),
            ("wait 0", LimitsSettings { max_wait_minutes: 0, ..base }, false),
            ("wait 720", LimitsSettings { max_wait_minutes: 720, ..base }, true),
            ("wait 721", LimitsSettings { max_wait_minutes: 721, ..base }, false),
        ];
        for (label, limits, expect_valid) in cases {
            let mut settings = valid();
            settings.limits = limits;
            let problems = settings.validate().err().unwrap_or_default();
            assert_eq!(problems.is_empty(), expect_valid, "{label}: {problems:?}");
            assert!(problems.iter().all(|p| p.starts_with("limits:")) || problems.is_empty(), "{label}: {problems:?}");
        }
    }

    #[test]
    fn a_limit_out_of_range_is_refused_on_save_and_the_file_is_left_alone() {
        let dir = TempDir::new();
        save(dir.path(), &valid()).unwrap();
        let before = fs::read(settings_path(dir.path())).unwrap();

        let mut bad = valid();
        bad.limits.max_poll_calls = 9;
        let err = save(dir.path(), &bad).unwrap_err();
        assert!(matches!(&err, SettingsError::Invalid(msg) if msg.contains("polling calls per turn must be 10 to 500")), "{err}");
        assert_eq!(fs::read(settings_path(dir.path())).unwrap(), before);
    }

    #[test]
    fn an_out_of_range_limit_in_a_file_makes_the_file_invalid_and_is_left_alone() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path()).unwrap();
        let below = br#"{"version":1,"limits":{"max_model_calls":9}}"#;
        fs::write(settings_path(dir.path()), below).unwrap();
        assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(msg)) if msg.contains("model calls per turn must be at least 10")));
        assert_eq!(fs::read(settings_path(dir.path())).unwrap(), below);

        // Values the number type cannot hold, or that are not whole numbers, make the file unreadable: same result.
        for json in [
            &br#"{"version":1,"limits":{"max_model_calls":4294967296}}"#[..],
            &br#"{"version":1,"limits":{"max_wait_minutes":12.5}}"#[..],
            &br#"{"version":1,"limits":{"max_commands_per_turn":0}}"#[..],
            &br#"{"version":1,"limits":{"max_poll_calls":5000}}"#[..],
        ] {
            fs::write(settings_path(dir.path()), json).unwrap();
            assert!(matches!(load(dir.path()), Err(SettingsError::Invalid(_))), "{}", String::from_utf8_lossy(json));
            assert_eq!(fs::read(settings_path(dir.path())).unwrap(), json);
        }
    }
}
