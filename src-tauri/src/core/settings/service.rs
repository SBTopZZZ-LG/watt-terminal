//! The AI provider settings as one service: what the settings UI and the chat commands call. Nothing here depends on
//! Tauri, so the tests run against a temp directory and an in-memory secret store.
//!
//! A change is checked on a copy first. Then the secrets are written in one save, then `settings.json` is written
//! atomically, and only then is the in-memory copy replaced. If the settings file cannot be written, the secrets go
//! back to what they were, so a failed change leaves nothing different. Changes are serialised by a write lock. The
//! state lock is held only to read or replace the in-memory copy, so a reader (for example `get_status`) never waits
//! on the disk or the Keychain.
//!
//! A `settings.json` that cannot be loaded is never overwritten. Every change is refused, with the reason, until the
//! file is fixed or removed. Reads retry the file while it is broken, so a fix takes effect without a restart.
//!
//! Nothing here returns or logs a secret value. Errors are strings that name a provider, model or header.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::timeout;

use super::models::{FetchedModel, merge_fetched, models_url_for};
use super::resolve::{self, api_key_for, headers_for};
use super::secrets::{SecretMap, Secrets, api_key_name, header_secret_name, provider_prefix};
use super::{ModelRef, ModelSource, Provider, Settings, SettingsError, load, save, settings_path};
use crate::core::chat::agent_loop::{LlmSettings, USER_AGENT, new_conversation_id};
use crate::core::chat::redact::Redactor;
use crate::core::llm::{self, ChatRequest, ProviderConfig, TurnCancel};

/// How long the connection test waits for an answer, the whole request included.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_PROMPT: &str = "Reply with the single word: ok";
/// Keeps the test to a few output tokens.
const TEST_MAX_TOKENS: u32 = 16;
/// Longest reply text shown in a test result, in characters.
const EXCERPT_CHARS: usize = 200;
/// The refusal for a fetch that returned no models. The saved list is kept as it was.
const NO_MODELS_FETCHED: &str = "the provider returned no models; the saved list was not changed";

/// What the settings UI shows. It holds which secrets are stored, never their values.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub settings: Settings,
    /// Per provider id.
    pub secrets: BTreeMap<String, SecretsPresent>,
    /// Where the secrets are kept: `"keychain"` or `"file"`.
    pub store_kind: &'static str,
    /// Set when settings.json could not be loaded. The settings shown are then empty, and changes are refused.
    pub load_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretsPresent {
    pub api_key: bool,
    /// Names of the secret headers that have a stored value.
    pub headers: Vec<String>,
}

/// A provider as the settings UI saves it, with the secrets the user typed.
#[derive(Deserialize)]
pub struct SaveProviderInput {
    pub provider: Provider,
    /// `None` keeps the stored key, `""` removes it, anything else replaces it. Surrounding spaces are trimmed.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Header name to value, for secret headers. An absent name keeps the stored value, `""` removes it. Values for
    /// headers that are not secret are ignored. Holds secrets, so there is no `Debug`.
    #[serde(default)]
    pub header_secrets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FetchSummary {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub total: usize,
}

#[derive(Serialize)]
pub struct FetchOutcome {
    pub view: SettingsView,
    pub summary: FetchSummary,
}

/// What a model-list request needs. `api_key` and the header values are secrets, so `Debug` hides them.
pub struct FetchPlan {
    pub url: String,
    pub api_key: String,
    /// Header pairs with `{session_id}` already resolved.
    pub headers: Vec<(String, String)>,
}

impl fmt::Debug for FetchPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("FetchPlan")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .field("header_names", &header_names)
            .finish()
    }
}

/// The saved default model, ready to run. `settings` holds secrets, but its `Debug` hides them (see `LlmSettings`).
#[derive(Debug)]
pub struct ActiveLlm {
    pub settings: LlmSettings,
    /// The provider's display name.
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
    pub millis: u64,
}

struct State {
    settings: Settings,
    load_error: Option<String>,
}

pub struct SettingsService {
    dir: PathBuf,
    /// Held for a whole change: its checks, the secret write and the file write.
    write: Mutex<()>,
    /// Held only to read or replace the in-memory copy.
    state: Mutex<State>,
    secrets: Arc<Secrets>,
}

impl SettingsService {
    /// Loads `config_dir/settings.json` once. A file that cannot be loaded is not an error here: it is reported through
    /// `view` and `active`, and every change is refused until it is fixed.
    pub fn new(config_dir: &Path, secrets: Arc<Secrets>) -> Self {
        let state = match load(config_dir) {
            Ok(settings) => State { settings, load_error: None },
            Err(e) => {
                let problem = load_error_text(config_dir, &e);
                eprintln!("agent-term: the AI settings are not loaded: {problem}");
                State { settings: Settings::default(), load_error: Some(problem) }
            }
        };
        Self { dir: config_dir.to_path_buf(), write: Mutex::new(()), state: Mutex::new(state), secrets }
    }

    /// The settings and the load error, if any. While the file is broken this reads it again, so a fixed file is picked
    /// up on the next call.
    fn snapshot(&self) -> (Settings, Option<String>) {
        let mut state = lock(&self.state);
        if state.load_error.is_some() {
            match load(&self.dir) {
                Ok(settings) => *state = State { settings, load_error: None },
                Err(e) => state.load_error = Some(load_error_text(&self.dir, &e)),
            }
        }
        (state.settings.clone(), state.load_error.clone())
    }

    /// The settings a read starts from. Fails while the file is broken.
    fn usable(&self) -> Result<Settings, String> {
        match self.snapshot() {
            (settings, None) => Ok(settings),
            (_, Some(problem)) => Err(format!("the AI settings cannot be used: {problem}")),
        }
    }

    /// The settings a change starts from. Refused while the file is broken, so the file is never overwritten.
    fn writable(&self) -> Result<Settings, String> {
        match self.snapshot() {
            (settings, None) => Ok(settings),
            (_, Some(problem)) => Err(format!("the AI settings are unchanged: {problem}")),
        }
    }

    /// Writes `candidate` to settings.json, then makes it the in-memory copy. The caller holds the write lock and has
    /// validated the candidate.
    fn commit(&self, candidate: Settings) -> Result<(), String> {
        save(&self.dir, &candidate).map_err(|e| e.to_string())?;
        lock(&self.state).settings = candidate;
        Ok(())
    }

    /// Applies `change` to the stored secrets in one save and returns what they were before, for `restore_secrets`.
    fn change_secrets(&self, change: impl FnOnce(&mut SecretMap)) -> Result<SecretMap, String> {
        let before = self.secrets.snapshot().map_err(|e| e.to_string())?;
        self.secrets.edit(change).map_err(|e| e.to_string())?;
        Ok(before)
    }

    fn restore_secrets(&self, before: SecretMap) -> Result<(), String> {
        self.secrets.edit(|map| *map = before).map_err(|e| format!("the secrets could not be restored: {e}"))
    }

    /// Saves `candidate` after the secrets were changed. If the settings file cannot be written, the secrets go back.
    fn commit_with_secrets(&self, candidate: Settings, before: SecretMap) -> Result<(), String> {
        match self.commit(candidate) {
            Ok(()) => Ok(()),
            Err(e) => match self.restore_secrets(before) {
                Ok(()) => Err(e),
                Err(rollback) => Err(format!("{e}; {rollback}")),
            },
        }
    }

    /// Validates a settings-only change, saves it and returns the view.
    fn change_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<SettingsView, String> {
        let _write = lock(&self.write);
        let mut candidate = self.writable()?;
        change(&mut candidate);
        candidate.validate().map_err(|problems| problems.join("; "))?;
        self.commit(candidate)?;
        self.view()
    }

    pub fn view(&self) -> Result<SettingsView, String> {
        let (settings, load_error) = self.snapshot();
        let mut secrets = BTreeMap::new();
        for provider in &settings.providers {
            let api_key = self.has(&api_key_name(&provider.id))?;
            let mut headers = Vec::new();
            for header in provider.headers.iter().filter(|h| h.secret) {
                if self.has(&header_secret_name(&provider.id, &header.name))? {
                    headers.push(header.name.clone());
                }
            }
            secrets.insert(provider.id.clone(), SecretsPresent { api_key, headers });
        }
        Ok(SettingsView { settings, secrets, store_kind: self.secrets.kind(), load_error })
    }

    fn has(&self, name: &str) -> Result<bool, String> {
        self.secrets.has(name).map_err(|e| e.to_string())
    }

    /// Adds a provider, or replaces the one with the same id. See `SaveProviderInput` for the secrets. A new provider
    /// does not become the default; the user chooses that.
    pub fn save_provider(&self, input: SaveProviderInput) -> Result<SettingsView, String> {
        let _write = lock(&self.write);
        let settings = self.writable()?;
        let SaveProviderInput { mut provider, api_key, header_secrets } = input;
        let id = provider.id.clone();

        let mut problems = Vec::new();
        let mut sets: Vec<(String, String)> = Vec::new();
        let mut removes: Vec<String> = Vec::new();

        match api_key.as_deref().map(str::trim) {
            None => {}
            Some("") => removes.push(api_key_name(&id)),
            Some(key) if key.chars().any(char::is_control) => {
                problems.push(format!("provider {id:?}: the API key must not contain control characters"));
            }
            Some(key) => sets.push((api_key_name(&id), key.to_string())),
        }

        let previous_secret_headers: Vec<String> = settings
            .provider(&id)
            .map(|p| p.headers.iter().filter(|h| h.secret).map(|h| h.name.clone()).collect())
            .unwrap_or_default();
        for header in provider.headers.iter_mut().filter(|h| h.secret) {
            // A secret value never goes into settings.json, whatever the caller sent.
            header.value.clear();
            let Some(value) = header_secrets.get(&header.name) else {
                continue;
            };
            let key = header_secret_name(&id, &header.name);
            if value.is_empty() {
                removes.push(key);
            } else if value.chars().any(char::is_control) {
                problems.push(format!(
                    "provider {id:?}: the value of secret header {:?} must not contain control characters",
                    header.name
                ));
            } else {
                sets.push((key, value.clone()));
            }
        }
        // A stored secret header that was removed, or is no longer secret, loses its stored value.
        for name in previous_secret_headers {
            let still_secret = provider.headers.iter().any(|h| h.secret && h.name.eq_ignore_ascii_case(&name));
            if !still_secret {
                removes.push(header_secret_name(&id, &name));
            }
        }

        let mut candidate = settings;
        match candidate.providers.iter_mut().find(|p| p.id == id) {
            Some(existing) => *existing = provider,
            None => candidate.providers.push(provider),
        }
        if let Err(list) = candidate.validate() {
            problems.extend(list);
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }

        let before = self.change_secrets(move |map| {
            for key in removes {
                map.remove(&key);
            }
            for (key, value) in sets {
                map.insert(key, value);
            }
        })?;
        self.commit_with_secrets(candidate, before)?;
        self.view()
    }

    /// Deletes a provider and every secret it has. A default model that pointed at it is cleared.
    pub fn delete_provider(&self, id: &str) -> Result<SettingsView, String> {
        let _write = lock(&self.write);
        let mut candidate = self.writable()?;
        if candidate.provider(id).is_none() {
            return Err(no_provider(id));
        }
        candidate.providers.retain(|p| p.id != id);
        if candidate.default_model.as_ref().is_some_and(|d| d.provider_id == id) {
            candidate.default_model = None;
        }
        let prefix = provider_prefix(id);
        let before = self.change_secrets(|map| map.retain(|key, _| !key.starts_with(&prefix)))?;
        self.commit_with_secrets(candidate, before)?;
        self.view()
    }

    /// Makes `model` the default, or clears the default with `None`. The model must be listed by its provider.
    pub fn set_default_model(&self, model: Option<ModelRef>) -> Result<SettingsView, String> {
        self.change_settings(|settings| settings.default_model = model)
    }

    /// The global context cap in tokens, or `None` for no cap.
    pub fn set_global_context_limit(&self, limit: Option<u32>) -> Result<SettingsView, String> {
        self.change_settings(|settings| settings.global_context_limit = limit)
    }

    /// What a model-list request for `id` needs: the list URL, the key and the resolved headers.
    pub fn provider_for_fetch(&self, id: &str) -> Result<FetchPlan, String> {
        let settings = self.usable()?;
        let provider = settings.provider(id).ok_or_else(|| no_provider(id))?;
        let url = models_url_for(provider).ok_or_else(|| {
            format!("provider {id:?} has no model list URL; set one in the AI settings")
        })?;
        let api_key = api_key_for(provider, &self.secrets).map_err(|e| e.to_string())?;
        let headers = headers_for(provider, &self.secrets).map_err(|e| e.to_string())?;
        let headers = resolve::resolve_headers_for_fetch(&headers).map_err(|e| e.to_string())?;
        Ok(FetchPlan { url, api_key, headers })
    }

    /// Folds a fetched model list into provider `id`. A default model that is no longer listed keeps its entry, so the
    /// default never disappears on a refresh.
    pub fn apply_fetched(&self, id: &str, fetched: Vec<FetchedModel>) -> Result<(SettingsView, FetchSummary), String> {
        // An empty list would remove every fetched model, so it is refused before anything is read or changed.
        if fetched.is_empty() {
            return Err(NO_MODELS_FETCHED.to_string());
        }
        let _write = lock(&self.write);
        let mut candidate = self.writable()?;
        let index = candidate.providers.iter().position(|p| p.id == id).ok_or_else(|| no_provider(id))?;
        let default_id: Option<String> =
            candidate.default_model.as_ref().filter(|d| d.provider_id == id).map(|d| d.model_id.clone());

        let provider = &mut candidate.providers[index];
        let before = provider.models.clone();
        let fresh: BTreeSet<String> = fetched.iter().map(|m| m.id.clone()).collect();
        let existing: HashSet<&str> = before.iter().map(|m| m.id.as_str()).collect();
        let is_default = |model_id: &str| default_id.as_deref() == Some(model_id);
        let fetched_before = || before.iter().filter(|m| m.source == ModelSource::Fetched);
        let summary = FetchSummary {
            added: fresh.iter().filter(|m| !existing.contains(m.as_str())).count(),
            updated: fetched_before().filter(|m| fresh.contains(&m.id)).count(),
            removed: fetched_before().filter(|m| !fresh.contains(&m.id) && !is_default(&m.id)).count(),
            total: 0,
        };

        merge_fetched(provider, fetched);
        if let Some(default) = &default_id
            && provider.model(default).is_none()
            && let Some(position) = before.iter().position(|m| &m.id == default)
        {
            provider.models.insert(position.min(provider.models.len()), before[position].clone());
        }
        let total = provider.models.len();

        self.commit(candidate)?;
        Ok((self.view()?, FetchSummary { total, ..summary }))
    }

    /// The default model with its secrets resolved. `None` when no default is set. An error when the default cannot be
    /// used (no key, a missing header value) or the settings file is broken.
    pub fn active(&self) -> Result<Option<ActiveLlm>, String> {
        let settings = self.usable()?;
        let Some(default) = settings.default_model.clone() else {
            return Ok(None);
        };
        let resolved = resolve::resolve(&settings, &self.secrets, &default)
            .map_err(|e| format!("the default model cannot be used: {e}. Check the AI settings."))?;
        let provider = settings.provider(&default.provider_id).map(|p| p.name.clone()).unwrap_or_default();
        Ok(Some(ActiveLlm { settings: resolved, provider, model: default.model_id }))
    }

    /// The default model's provider name and model id, for the status line. Never reads the secret store.
    pub fn default_label(&self) -> Option<(String, String)> {
        let state = lock(&self.state);
        if state.load_error.is_some() {
            return None;
        }
        let default = state.settings.default_model.as_ref()?;
        let provider = state.settings.provider(&default.provider_id)?;
        Some((provider.name.clone(), default.model_id.clone()))
    }

    /// The settings for one model, for a connection test. Works before the model is the default.
    pub fn test_plan(&self, provider_id: &str, model_id: &str) -> Result<LlmSettings, String> {
        let settings = self.usable()?;
        let model = ModelRef { provider_id: provider_id.to_string(), model_id: model_id.to_string() };
        resolve::resolve(&settings, &self.secrets, &model).map_err(|e| e.to_string())
    }
}

/// Sends one short, non-streamed request to the model and reports whether it answered. It costs a few tokens: the
/// prompt is `TEST_PROMPT` and the reply is capped at `TEST_MAX_TOKENS`. Sent once, with no retry. The message is the
/// start of the reply on success, or the error on failure, with secrets removed. Headers are never shown.
pub async fn test_model(client: &reqwest::Client, settings: &LlmSettings) -> TestResult {
    let started = Instant::now();
    let outcome = probe(client, settings).await;
    let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    // The same secrets the agent redacts: the key and every header value.
    let mut known = vec![settings.api_key.clone()];
    known.extend(settings.headers.iter().map(|(_, value)| value.clone()));
    let redactor = Redactor::new(known);
    match outcome {
        Ok(text) => {
            let text = redactor.redact(text.trim());
            let message =
                if text.is_empty() { "the provider answered, with no text".to_string() } else { excerpt(&text) };
            TestResult { ok: true, message, millis }
        }
        Err(message) => TestResult { ok: false, message: redactor.redact(&message), millis },
    }
}

async fn probe(client: &reqwest::Client, settings: &LlmSettings) -> Result<String, String> {
    let session = new_conversation_id().map_err(|e| e.to_string())?;
    let cfg = ProviderConfig {
        chat_url: settings.chat_url.clone(),
        api_key: settings.api_key.clone(),
        headers: settings.resolved_headers(&session),
        user_agent: USER_AGENT.to_string(),
    };
    let req = ChatRequest {
        model: settings.model.clone(),
        messages: vec![json!({"role": "user", "content": TEST_PROMPT})],
        tools: None,
        max_tokens: Some(TEST_MAX_TOKENS),
    };
    let cancel = TurnCancel::new();
    match timeout(TEST_TIMEOUT, llm::complete(client, &cfg, &req, &cancel)).await {
        Ok(Ok(reply)) => Ok(reply.text),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {} seconds", TEST_TIMEOUT.as_secs())),
    }
}

fn excerpt(text: &str) -> String {
    if text.chars().count() <= EXCERPT_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(EXCERPT_CHARS).collect();
    format!("{cut}...")
}

fn load_error_text(config_dir: &Path, e: &SettingsError) -> String {
    format!("{}: {e}", settings_path(config_dir).display())
}

fn no_provider(id: &str) -> String {
    format!("no provider {id:?} is saved")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::secrets::MemoryStore;
    use super::super::test_support::TempDir;
    use super::super::{HeaderEntry, Model, save as save_file};
    use super::*;

    const KEY: &str = "DUMMY-key-0123456789abcdef";
    const HEADER: &str = "DUMMY-header-fedcba9876543210";
    const OTHER_KEY: &str = "DUMMY-other-key-abcdef0123456789";

    struct Fixture {
        dir: TempDir,
        store: MemoryStore,
        service: SettingsService,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new();
        let store = MemoryStore::default();
        let service = SettingsService::new(dir.path(), Arc::new(Secrets::new(Box::new(store.clone()))));
        Fixture { dir, store, service }
    }

    fn service_on(dir: &TempDir, store: &MemoryStore) -> SettingsService {
        SettingsService::new(dir.path(), Arc::new(Secrets::new(Box::new(store.clone()))))
    }

    fn model(id: &str, source: ModelSource) -> Model {
        Model { id: id.into(), context_limit: None, provider_context: None, source }
    }

    fn provider(id: &str) -> Provider {
        Provider {
            id: id.into(),
            name: format!("{id} name"),
            chat_url: format!("https://api.{id}.test/v1/chat/completions"),
            models_url: None,
            headers: Vec::new(),
            stream: true,
            max_tokens: None,
            models: vec![model("big", ModelSource::Custom)],
        }
    }

    fn input(provider: Provider) -> SaveProviderInput {
        SaveProviderInput { provider, api_key: None, header_secrets: BTreeMap::new() }
    }

    fn with_key(provider: Provider, key: &str) -> SaveProviderInput {
        SaveProviderInput { api_key: Some(key.into()), ..input(provider) }
    }

    fn model_ref(provider_id: &str, model_id: &str) -> ModelRef {
        ModelRef { provider_id: provider_id.into(), model_id: model_id.into() }
    }

    fn read_file(dir: &TempDir) -> Vec<u8> {
        fs::read(settings_path(dir.path())).unwrap()
    }

    #[test]
    fn secrets_live_only_in_the_store_and_never_in_the_file() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![
            HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true },
            HeaderEntry { name: "X-Session".into(), value: "{session_id}".into(), secret: false },
        ];
        let mut save_input = with_key(p, KEY);
        save_input.header_secrets.insert("X-Key".into(), HEADER.into());

        let view = f.service.save_provider(save_input).unwrap();
        assert_eq!(view.store_kind, "memory");
        assert_eq!(view.load_error, None);
        assert_eq!(view.secrets["acme"], SecretsPresent { api_key: true, headers: vec!["X-Key".into()] });

        let file = String::from_utf8(read_file(&f.dir)).unwrap();
        assert!(!file.contains(KEY) && !file.contains(HEADER), "a secret reached settings.json");
        let saved = load(f.dir.path()).unwrap();
        assert_eq!(saved.providers[0].headers[0].value, "", "a secret header is saved with an empty value");
        assert_eq!(saved.providers[0].headers[1].value, "{session_id}");

        let blob = f.store.stored().unwrap();
        assert!(blob.contains(KEY) && blob.contains(HEADER));
    }

    #[test]
    fn saving_again_replaces_by_id_and_keeps_a_secret_that_was_not_sent() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();

        let mut renamed = provider("acme");
        renamed.name = "Renamed".into();
        let view = f.service.save_provider(input(renamed)).unwrap();
        assert_eq!(view.settings.providers.len(), 1);
        assert_eq!(view.settings.providers[0].name, "Renamed");
        assert!(view.secrets["acme"].api_key, "an absent key is kept");
        assert!(f.store.stored().unwrap().contains(KEY));
    }

    #[test]
    fn an_empty_key_removes_it_and_turning_a_header_plain_removes_its_secret() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true }];
        let mut first = with_key(p.clone(), KEY);
        first.header_secrets.insert("X-Key".into(), HEADER.into());
        f.service.save_provider(first).unwrap();

        // The header is now plain, and the key is removed.
        let mut plain = p;
        plain.headers = vec![HeaderEntry { name: "X-Key".into(), value: "visible".into(), secret: false }];
        let view = f.service.save_provider(SaveProviderInput {
            provider: plain,
            api_key: Some(String::new()),
            header_secrets: BTreeMap::new(),
        })
        .unwrap();
        assert_eq!(view.secrets["acme"], SecretsPresent { api_key: false, headers: vec![] });
        let blob = f.store.stored().unwrap();
        assert!(!blob.contains(KEY) && !blob.contains(HEADER), "{blob}");
        assert_eq!(blob, "{}");
    }

    #[test]
    fn a_removed_secret_header_loses_its_stored_value() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true }];
        let mut first = with_key(p.clone(), KEY);
        first.header_secrets.insert("X-Key".into(), HEADER.into());
        f.service.save_provider(first).unwrap();

        p.headers.clear();
        f.service.save_provider(input(p)).unwrap();
        assert!(!f.store.stored().unwrap().contains(HEADER));
    }

    #[test]
    fn deleting_a_provider_removes_its_secrets_and_clears_the_default_only_when_it_pointed_there() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        f.service.save_provider(with_key(provider("other"), OTHER_KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();

        let view = f.service.delete_provider("acme").unwrap();
        assert_eq!(view.settings.providers.len(), 1);
        assert_eq!(view.settings.default_model, None, "the default pointed at the deleted provider");
        assert!(!view.secrets.contains_key("acme"));
        let blob = f.store.stored().unwrap();
        assert!(!blob.contains(KEY));
        assert!(blob.contains(OTHER_KEY), "another provider's secret is kept");

        f.service.set_default_model(Some(model_ref("other", "big"))).unwrap();
        let err = f.service.delete_provider("acme").unwrap_err();
        assert!(err.contains("no provider"), "{err}");
        let view = f.service.delete_provider("other").unwrap();
        assert_eq!(view.settings.default_model, None);
    }

    #[test]
    fn validation_errors_are_readable_and_change_nothing() {
        let f = fixture();
        let mut bad = provider("acme");
        bad.chat_url = "http://api.acme.test/v1/chat/completions".into();
        let err = f.service.save_provider(with_key(bad, KEY)).unwrap_err();
        assert!(err.contains("plain http"), "{err}");
        assert!(!err.contains(KEY));
        assert_eq!(f.store.stored(), None, "the store was not written");
        assert!(!settings_path(f.dir.path()).exists());

        let err = f.service.save_provider(with_key(provider("Bad Id"), KEY)).unwrap_err();
        assert!(err.contains("provider id"), "{err}");

        let mut p = provider("acme");
        p.headers = vec![HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true }];
        let mut control = input(p);
        control.header_secrets.insert("X-Key".into(), "line one\r\nline two".into());
        let err = f.service.save_provider(control).unwrap_err();
        assert!(err.contains("control characters") && !err.contains("line one"), "{err}");
        assert_eq!(f.store.stored(), None);
    }

    #[test]
    fn a_failed_settings_write_puts_the_secrets_back() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        let blob_before = f.store.stored();
        let file_before = read_file(&f.dir);

        // Make the config dir read-only, so settings.json cannot be written but the store still can.
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(f.dir.path()).unwrap().permissions().mode();
        fs::set_permissions(f.dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
        if fs::File::create(f.dir.path().join("probe")).is_ok() {
            // Running as root: permissions do not apply, so there is nothing to test.
            fs::set_permissions(f.dir.path(), fs::Permissions::from_mode(mode)).unwrap();
            return;
        }

        let err = f.service.save_provider(with_key(provider("acme"), OTHER_KEY)).unwrap_err();
        fs::set_permissions(f.dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(err.contains("could not be saved"), "{err}");
        assert!(!err.contains(OTHER_KEY));
        assert_eq!(f.store.stored(), blob_before, "the secret was not rolled back");
        assert_eq!(read_file(&f.dir), file_before);
        assert!(f.service.view().unwrap().secrets["acme"].api_key);
    }

    #[test]
    fn a_broken_settings_file_refuses_changes_and_is_left_alone() {
        let f = fixture();
        fs::create_dir_all(f.dir.path()).unwrap();
        let broken = b"{ not settings";
        fs::write(settings_path(f.dir.path()), broken).unwrap();
        let service = service_on(&f.dir, &f.store);

        let view = service.view().unwrap();
        let problem = view.load_error.expect("the load error is reported");
        assert!(problem.contains("settings.json"), "{problem}");
        assert!(view.settings.providers.is_empty());

        let err = service.save_provider(with_key(provider("acme"), KEY)).unwrap_err();
        assert!(err.contains("unchanged"), "{err}");
        assert!(service.set_default_model(None).unwrap_err().contains("unchanged"));
        assert!(service.set_global_context_limit(Some(10)).unwrap_err().contains("unchanged"));
        assert!(service.delete_provider("acme").unwrap_err().contains("unchanged"));
        assert!(service.active().unwrap_err().contains("cannot be used"));
        assert_eq!(read_file(&f.dir), broken, "the broken file was overwritten");
        assert_eq!(f.store.stored(), None, "the store was touched");

        // Fixing the file takes effect on the next call, without a restart.
        save_file(f.dir.path(), &Settings::default()).unwrap();
        let view = service.view().unwrap();
        assert_eq!(view.load_error, None);
        service.save_provider(with_key(provider("acme"), KEY)).unwrap();
    }

    #[test]
    fn default_model_and_context_limit_are_validated_and_saved() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();

        let err = f.service.set_default_model(Some(model_ref("acme", "missing"))).unwrap_err();
        assert!(err.contains("not listed"), "{err}");
        let err = f.service.set_default_model(Some(model_ref("nope", "big"))).unwrap_err();
        assert!(err.contains("unknown provider"), "{err}");
        assert!(f.service.set_global_context_limit(Some(0)).unwrap_err().contains("at least 1"));

        let view = f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();
        assert_eq!(view.settings.default_model, Some(model_ref("acme", "big")));
        let view = f.service.set_global_context_limit(Some(8_000)).unwrap();
        assert_eq!(view.settings.global_context_limit, Some(8_000));
        let view = f.service.set_default_model(None).unwrap();
        assert_eq!(view.settings.default_model, None);
    }

    #[test]
    fn default_label_is_read_without_touching_the_store() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();

        let service = service_on(&f.dir, &f.store);
        let loads_before = f.store.loads();
        assert_eq!(service.default_label(), Some(("acme name".to_string(), "big".to_string())));
        assert_eq!(f.store.loads(), loads_before, "the status line read the store");
    }

    #[test]
    fn active_is_none_without_a_default_and_an_error_without_a_key() {
        let f = fixture();
        assert!(f.service.active().unwrap().is_none());

        f.service.save_provider(input(provider("acme"))).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();
        let err = f.service.active().unwrap_err();
        assert!(err.contains("API key") && err.contains("AI settings"), "{err}");

        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        let active = f.service.active().unwrap().unwrap();
        assert_eq!(active.provider, "acme name");
        assert_eq!(active.model, "big");
        assert_eq!(active.settings.api_key, KEY);
        assert_eq!(active.settings.chat_url, "https://api.acme.test/v1/chat/completions");
    }

    #[test]
    fn test_plan_works_before_the_model_is_the_default() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        let plan = f.service.test_plan("acme", "big").unwrap();
        assert_eq!(plan.model, "big");
        assert!(f.service.default_label().is_none());
        assert!(f.service.test_plan("acme", "missing").unwrap_err().contains("does not list"));
    }

    #[test]
    fn view_reports_presence_only() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![
            HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true },
            HeaderEntry { name: "X-Other".into(), value: String::new(), secret: true },
        ];
        let mut save_input = with_key(p, KEY);
        save_input.header_secrets.insert("X-Key".into(), HEADER.into());
        let view = f.service.save_provider(save_input).unwrap();
        assert_eq!(view.secrets["acme"], SecretsPresent { api_key: true, headers: vec!["X-Key".into()] });
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains(KEY) && !json.contains(HEADER));
    }

    #[test]
    fn a_refresh_keeps_the_default_and_reports_what_changed() {
        let f = fixture();
        let mut p = provider("acme");
        p.models = vec![
            model("mine", ModelSource::Custom),
            model("kept", ModelSource::Fetched),
            model("gone", ModelSource::Fetched),
        ];
        f.service.save_provider(with_key(p, KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "gone"))).unwrap();

        let fetched = vec![
            FetchedModel { id: "kept".into(), context: Some(2_000) },
            FetchedModel { id: "fresh".into(), context: None },
        ];
        let (view, summary) = f.service.apply_fetched("acme", fetched.clone()).unwrap();
        assert_eq!(summary, FetchSummary { added: 1, updated: 1, removed: 0, total: 4 });
        let ids: Vec<&str> = view.settings.providers[0].models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["mine", "kept", "gone", "fresh"]);
        assert_eq!(view.settings.providers[0].models[1].provider_context, Some(2_000));
        assert_eq!(view.settings.default_model, Some(model_ref("acme", "gone")));

        // Once it is no longer the default, the same refresh drops it.
        f.service.set_default_model(None).unwrap();
        let (view, summary) = f.service.apply_fetched("acme", fetched).unwrap();
        assert_eq!(summary, FetchSummary { added: 0, updated: 2, removed: 1, total: 3 });
        assert!(view.settings.providers[0].model("gone").is_none());
    }

    #[test]
    fn an_empty_fetched_list_is_refused_and_the_saved_settings_stay_as_they_were() {
        let f = fixture();
        let mut p = provider("acme");
        p.models = vec![model("mine", ModelSource::Custom), model("kept", ModelSource::Fetched)];
        f.service.save_provider(with_key(p, KEY)).unwrap();
        let before = f.service.view().unwrap();
        let file_before = read_file(&f.dir);

        let err = f.service.apply_fetched("acme", Vec::new()).unwrap_err();
        assert_eq!(err, "the provider returned no models; the saved list was not changed");
        assert_eq!(f.service.view().unwrap().settings, before.settings, "the models are not wiped");
        assert_eq!(read_file(&f.dir), file_before, "nothing is written");
        let ids: Vec<&str> = before.settings.providers[0].models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["mine", "kept"]);
    }

    #[test]
    fn a_fetch_plan_has_the_url_key_and_resolved_headers_and_hides_them_in_debug() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![
            HeaderEntry { name: "X-Session".into(), value: "{session_id}".into(), secret: false },
            HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true },
        ];
        let mut save_input = with_key(p, KEY);
        save_input.header_secrets.insert("X-Key".into(), HEADER.into());
        f.service.save_provider(save_input).unwrap();

        let plan = f.service.provider_for_fetch("acme").unwrap();
        assert_eq!(plan.url, "https://api.acme.test/v1/models");
        assert_eq!(plan.api_key, KEY);
        assert_eq!(plan.headers[0].0, "X-Session");
        assert!(plan.headers[0].1.starts_with("ses_"), "{:?}", plan.headers[0]);
        assert_eq!(plan.headers[1], ("X-Key".to_string(), HEADER.to_string()));
        let debug = format!("{plan:?}");
        assert!(!debug.contains(KEY) && !debug.contains(HEADER));

        f.service.save_provider(input(provider("nokey"))).unwrap();
        assert!(f.service.provider_for_fetch("nokey").unwrap_err().contains("API key"));
        let mut no_list = provider("nolist");
        no_list.chat_url = "https://api.nolist.test/v1/responses".into();
        f.service.save_provider(with_key(no_list, KEY)).unwrap();
        assert!(f.service.provider_for_fetch("nolist").unwrap_err().contains("no model list URL"));
    }

    /// Serves one request with the given status line and body, then closes the connection.
    fn one_shot_server(status: &str, body: &str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(response.as_bytes());
        });
        format!("http://{addr}/v1/chat/completions")
    }

    fn llm_settings(chat_url: String) -> LlmSettings {
        LlmSettings {
            chat_url,
            api_key: KEY.to_string(),
            model: "big".to_string(),
            headers: vec![("X-Key".to_string(), HEADER.to_string())],
            stream: true,
            max_tokens: None,
            context_limit: None,
        }
    }

    #[tokio::test]
    async fn the_connection_test_reports_the_reply() {
        let url = one_shot_server("200 OK", r#"{"choices":[{"message":{"role":"assistant","content":" ok "}}]}"#);
        let client = llm::http_client().unwrap();
        let result = test_model(&client, &llm_settings(url)).await;
        assert!(result.ok, "{result:?}");
        assert_eq!(result.message, "ok");
    }

    #[tokio::test]
    async fn the_connection_test_hides_the_key_and_header_values_in_a_failure() {
        let body = format!(r#"{{"error":{{"message":"invalid key {KEY} and {HEADER}"}}}}"#);
        let url = one_shot_server("401 Unauthorized", &body);
        let client = llm::http_client().unwrap();
        let result = test_model(&client, &llm_settings(url)).await;
        assert!(!result.ok);
        assert!(result.message.contains("401"), "{result:?}");
        assert!(!result.message.contains(KEY) && !result.message.contains(HEADER), "{result:?}");
    }
}
