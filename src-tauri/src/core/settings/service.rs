//! The AI provider settings as one service: what the settings UI and the chat commands call. Nothing here depends on
//! Tauri, so the tests run against a temp directory and an in-memory secret store.
//!
//! A change is checked on a copy first. Then the secrets are written in one save, then `settings.json` is written
//! atomically, and only then is the in-memory copy replaced. If the settings file cannot be written, the secrets go
//! back to what they were, so a failed change leaves nothing different. Changes are serialised by a write lock. The
//! state lock is held only to read or replace the in-memory copy, so a reader (for example `get_status`) never waits
//! on the disk or the Keychain.
//!
//! `settings.json` is in one of three modes (see `Mode`). At every launch that finds it valid, a copy is kept as
//! the hidden last-good file. A file that cannot be used (unreadable, bad JSON, failing validation) is never
//! overwritten and nothing runs on it: every change and every read of the settings is refused with the reason, and the
//! terminal keeps working. The user can then revert to the last-good copy (the broken file is kept aside), run the
//! last-good copy for this session (the file is left as it is), or fix the file and reload it.
//!
//! When the file is re-read: at launch; by `reload`; and, while it is broken, on each read or change, so a fixed file
//! takes effect on the next call. An Ok session does not re-read the file, so a hand edit that breaks it during a
//! session is seen by `reload` or at the next launch, and a later save overwrites it, as before.
//!
//! Nothing here returns or logs a secret value. Errors are strings that name a provider, model or header.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::timeout;

use super::models::{FetchedModel, merge_fetched, models_url_for};
use super::resolve::{self, api_key_for, headers_for};
use super::secrets::{SecretMap, Secrets, api_key_name, header_secret_name, provider_prefix};
use super::{
    ApprovalSettings, IntegrationSettings, LimitsSettings, ModelRef, ModelSource, Provider, SETTINGS_FILE, Settings,
    SettingsError, last_good_path, read_checked, save, settings_path, write_private_atomic,
};
use crate::core::chat::agent_loop::{LlmSettings, USER_AGENT, new_conversation_id};
use crate::core::chat::redact::Redactor;
use crate::core::llm::{self, ChatRequest, ProviderConfig, TurnCancel};
use crate::core::persist::now_secs;

/// How long the connection test waits for an answer, the whole request included.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_PROMPT: &str = "Reply with the single word: ok";
/// Keeps the test to a few output tokens.
const TEST_MAX_TOKENS: u32 = 16;
/// Longest reply text shown in a test result, in characters.
const EXCERPT_CHARS: usize = 200;
/// The refusal for a fetch that returned no models. The saved list is kept as it was.
const NO_MODELS_FETCHED: &str = "the provider returned no models; the saved list was not changed";
/// The refusal for every change in last-good mode. The file stays as it is until it is fixed or reverted.
const LAST_GOOD_REFUSAL: &str = "settings.json is broken; fix or revert it first";
const LAST_GOOD_NO_DEFAULT: &str =
    "settings.json is broken, and the last working settings have no default model; fix or revert settings.json";
const NOT_BROKEN: &str = "settings.json is not broken, so there is nothing to recover";
const NO_LAST_GOOD: &str = "there are no last working settings: none was kept from a launch with a valid settings.json";

/// What the settings UI shows. It holds which secrets are stored, never their values.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub settings: Settings,
    /// Per provider id.
    pub secrets: BTreeMap<String, SecretsPresent>,
    /// Where the secrets are kept: `"keychain"` or `"file"`.
    pub store_kind: &'static str,
    /// Set while settings.json cannot be used (`settings_state.kind` is "broken" or "last_good"): the reason.
    pub load_error: Option<String>,
    pub settings_state: SettingsStateView,
}

/// What settings.json is and what the app runs on. `kind` is "ok", "broken" or "last_good".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsStateView {
    pub kind: &'static str,
    /// Why the file cannot be used. Set unless `kind` is "ok".
    pub reason: Option<String>,
    /// Whether the last-good copy exists and checks out, so the recovery actions can run.
    pub last_good_available: bool,
    /// The path of settings.json, for display.
    pub path: String,
}

/// The two recovery actions for a broken settings.json, as the `recover_settings` command names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverAction {
    /// Keeps the broken file aside and restores the last working copy as settings.json.
    Revert,
    /// Runs the last working copy for this session. settings.json is not touched.
    UseLastGood,
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
    /// The default model's variant, if one is selected (its overlay is already in `settings.extra`).
    pub variant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
    pub millis: u64,
}

/// What settings.json is and what the app runs on. `Ok`: the file is in use. `Broken`: the file cannot be used and
/// nothing runs on settings. `LastGood`: the file is broken and this session runs on the last working copy; the file is
/// kept as it is, and changes are refused until the mode ends.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Ok,
    Broken { reason: String },
    LastGood { reason: String },
}

#[derive(Debug, Clone)]
struct State {
    /// The settings in use. Empty while `Broken`.
    settings: Settings,
    mode: Mode,
    /// Whether the last-good copy exists and checks out.
    last_good_available: bool,
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
    /// Loads `config_dir/settings.json` once, and keeps a copy of it as the last-good file when it checks out. A file
    /// that cannot be used is not an error here: it is reported through `view`, and nothing runs on it until it is
    /// fixed, reverted or bypassed.
    pub fn new(config_dir: &Path, secrets: Arc<Secrets>) -> Self {
        let (settings, mode) = match read_checked(&settings_path(config_dir)) {
            Ok(Some((settings, bytes))) => {
                copy_last_good(config_dir, &bytes);
                (settings, Mode::Ok)
            }
            // A missing file is the default settings, and there is nothing to keep.
            Ok(None) => (Settings::default(), Mode::Ok),
            Err(e) => {
                let reason = load_error_text(config_dir, &e);
                eprintln!("watt: the AI settings are not loaded: {reason}");
                (Settings::default(), Mode::Broken { reason })
            }
        };
        let state = State { settings, mode, last_good_available: last_good_ok(config_dir) };
        Self { dir: config_dir.to_path_buf(), write: Mutex::new(()), state: Mutex::new(state), secrets }
    }

    /// The current state. While the file is broken it is read again first, so a fixed file takes effect on this call.
    fn snapshot(&self) -> State {
        let mut state = lock(&self.state);
        self.refresh(&mut state);
        state.clone()
    }

    /// Re-reads the file while it is broken. Ok and last-good sessions do not read it (see `reload`).
    fn refresh(&self, state: &mut State) {
        if matches!(state.mode, Mode::Broken { .. }) {
            self.read_file_into(state);
        }
    }

    /// Reads settings.json into `state`. A file that checks out makes the mode Ok. A file that does not makes the mode
    /// Broken, except that a last-good session stays last-good and shows the new reason.
    fn read_file_into(&self, state: &mut State) {
        match read_checked(&settings_path(&self.dir)) {
            Ok(found) => {
                state.settings = found.map_or_else(Settings::default, |(settings, _)| settings);
                state.mode = Mode::Ok;
            }
            Err(e) => {
                let reason = load_error_text(&self.dir, &e);
                if matches!(state.mode, Mode::LastGood { .. }) {
                    state.mode = Mode::LastGood { reason };
                } else {
                    state.settings = Settings::default();
                    state.mode = Mode::Broken { reason };
                }
            }
        }
        state.last_good_available = last_good_ok(&self.dir);
    }

    fn state_view(&self, state: &State) -> SettingsStateView {
        let (kind, reason) = match &state.mode {
            Mode::Ok => ("ok", None),
            Mode::Broken { reason } => ("broken", Some(reason.clone())),
            Mode::LastGood { reason } => ("last_good", Some(reason.clone())),
        };
        SettingsStateView {
            kind,
            reason,
            last_good_available: state.last_good_available,
            path: settings_path(&self.dir).display().to_string(),
        }
    }

    /// The settings a read starts from. Fails while the file is broken; a last-good session reads the last working
    /// copy.
    fn usable(&self) -> Result<Settings, String> {
        let State { settings, mode, .. } = self.snapshot();
        match mode {
            Mode::Ok | Mode::LastGood { .. } => Ok(settings),
            Mode::Broken { reason } => Err(format!("the AI settings cannot be used: {reason}")),
        }
    }

    /// The settings a change starts from. Refused while the file is broken, so the file is never overwritten, and in
    /// last-good mode, where the file is kept as it is.
    fn writable(&self) -> Result<Settings, String> {
        let State { settings, mode, .. } = self.snapshot();
        match mode {
            Mode::Ok => Ok(settings),
            Mode::Broken { reason } => Err(format!("the AI settings are unchanged: {reason}")),
            Mode::LastGood { .. } => Err(LAST_GOOD_REFUSAL.to_string()),
        }
    }

    /// The last working copy, read and checked. Its text is returned too, for the restore.
    fn last_good_copy(&self) -> Result<(Settings, Vec<u8>), String> {
        match read_checked(&last_good_path(&self.dir)) {
            Ok(Some(found)) => Ok(found),
            Ok(None) => Err(NO_LAST_GOOD.to_string()),
            Err(e) => Err(format!("the last working settings cannot be used: {e}")),
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
        let state = self.snapshot();
        let mut secrets = BTreeMap::new();
        for provider in &state.settings.providers {
            let api_key = self.has(&api_key_name(&provider.id))?;
            let mut headers = Vec::new();
            for header in provider.headers.iter().filter(|h| h.secret) {
                if self.has(&header_secret_name(&provider.id, &header.name))? {
                    headers.push(header.name.clone());
                }
            }
            secrets.insert(provider.id.clone(), SecretsPresent { api_key, headers });
        }
        let settings_state = self.state_view(&state);
        Ok(SettingsView {
            load_error: settings_state.reason.clone(),
            settings_state,
            settings: state.settings,
            secrets,
            store_kind: self.secrets.kind(),
        })
    }

    /// The state of settings.json alone, for the status line. Reads neither the file nor a secret.
    pub fn settings_state(&self) -> SettingsStateView {
        let state = lock(&self.state);
        self.state_view(&state)
    }

    /// Re-reads settings.json now. A file that checks out makes the settings usable again, and ends a last-good
    /// session. A file that is still broken keeps the current mode, with the new reason.
    pub fn reload(&self) -> Result<SettingsView, String> {
        {
            let _write = lock(&self.write);
            let mut state = lock(&self.state);
            self.read_file_into(&mut state);
        }
        self.view()
    }

    /// Recovers from a broken settings.json: the broken file is copied to `settings.json.broken-<unix>` (0600, never
    /// deleted), then the last working copy replaces settings.json atomically. The copy comes first, so the broken text
    /// is kept even if the replacement fails. The file is read again first, so a file fixed by hand is not replaced.
    /// Secrets stay in the store.
    pub fn revert_to_last_good(&self) -> Result<SettingsView, String> {
        {
            let _write = lock(&self.write);
            let mut state = lock(&self.state);
            self.read_file_into(&mut state);
            if matches!(state.mode, Mode::Ok) {
                return Err(NOT_BROKEN.to_string());
            }
            let (good, good_bytes) = self.last_good_copy()?;
            let path = settings_path(&self.dir);
            match fs::read(&path) {
                Ok(broken) => {
                    let aside = aside_path(&self.dir);
                    write_private_atomic(&aside, &broken).map_err(|e| {
                        format!("the broken settings could not be kept at {}: {e}", aside.display())
                    })?;
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("the broken settings could not be read: {e}")),
            }
            write_private_atomic(&path, &good_bytes)
                .map_err(|e| format!("the last working settings could not be restored: {e}"))?;
            state.settings = good;
            state.mode = Mode::Ok;
            state.last_good_available = true;
        }
        self.view()
    }

    /// Runs the last working copy for this session. settings.json is not touched, and every change is refused until
    /// the mode ends (a successful `reload`, or a restart). The file is read again first, as in `revert_to_last_good`.
    pub fn use_last_good(&self) -> Result<SettingsView, String> {
        {
            let _write = lock(&self.write);
            let mut state = lock(&self.state);
            self.read_file_into(&mut state);
            let reason = match &state.mode {
                Mode::Ok => return Err(NOT_BROKEN.to_string()),
                Mode::Broken { reason } | Mode::LastGood { reason } => reason.clone(),
            };
            let (good, _) = self.last_good_copy()?;
            state.settings = good;
            state.mode = Mode::LastGood { reason };
            state.last_good_available = true;
        }
        self.view()
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
        // (new key, old key): a renamed secret header with no typed value keeps the value stored under its old name.
        let mut carries: Vec<(String, String)> = Vec::new();
        for header in provider.headers.iter_mut().filter(|h| h.secret) {
            // A secret value never goes into settings.json, whatever the caller sent.
            header.value.clear();
            let previous = header.previous_name.take().filter(|old| !old.eq_ignore_ascii_case(&header.name));
            let key = header_secret_name(&id, &header.name);
            match header_secrets.get(&header.name).filter(|value| !value.is_empty()) {
                // A typed value always wins over a carried one.
                Some(value) if value.chars().any(char::is_control) => problems.push(format!(
                    "provider {id:?}: the value of secret header {:?} must not contain control characters",
                    header.name
                )),
                Some(value) => sets.push((key, value.clone())),
                None => match previous {
                    Some(old) => carries.push((key, header_secret_name(&id, &old))),
                    // An empty value removes the stored one. An absent name keeps it.
                    None if header_secrets.contains_key(&header.name) => removes.push(key),
                    None => {}
                },
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
            // A carried value is read before the old names are removed below.
            for (to, from) in carries {
                if let Some(value) = map.get(&from).cloned() {
                    map.insert(to, value);
                }
            }
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

    /// Saves the approval rules. Refused while settings.json is broken or in last-good mode, like every change.
    pub fn set_approval(&self, approval: ApprovalSettings) -> Result<SettingsView, String> {
        self.change_settings(|settings| settings.approval = approval)
    }

    /// The approval settings in use: the saved ones, or the last working copy's in last-good mode. Fails while
    /// settings.json is broken, so the caller falls back to manual approval.
    pub fn approval_settings(&self) -> Result<ApprovalSettings, String> {
        Ok(self.usable()?.approval)
    }

    /// Saves the shell integration settings. Refused while settings.json is broken or in last-good mode, like every change.
    pub fn set_integration(&self, integration: IntegrationSettings) -> Result<SettingsView, String> {
        self.change_settings(|settings| settings.integration = integration)
    }

    /// The integration settings in use (the saved ones, or the last working copy's in last-good mode). Fails while
    /// settings.json is broken, so the caller falls back to asking.
    pub fn integration_settings(&self) -> Result<IntegrationSettings, String> {
        Ok(self.usable()?.integration)
    }

    /// Saves the per-turn limits. Refused while settings.json is broken or in last-good mode, like every change.
    pub fn set_limits(&self, limits: LimitsSettings) -> Result<SettingsView, String> {
        self.change_settings(|settings| settings.limits = limits)
    }

    /// The limits in use: the saved ones, or the last working copy's in last-good mode. Fails while settings.json is
    /// broken, so the caller falls back to the defaults.
    pub fn limits_settings(&self) -> Result<LimitsSettings, String> {
        Ok(self.usable()?.limits)
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
    /// default never disappears on a refresh. A default variant that is no longer on the merged model is cleared, so
    /// the default falls back to the base model.
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
        // A fetch replaces the auto variants. When the default's variant is gone from the merged model, the default
        // falls back to the base model; a stale variant would fail validation below.
        let variant_gone = match &candidate.default_model {
            Some(default) if default.provider_id == id => match &default.variant {
                Some(variant) => provider.model(&default.model_id).is_none_or(|m| m.variant(variant).is_none()),
                None => false,
            },
            _ => false,
        };
        if variant_gone {
            candidate.default_model.as_mut().expect("the default was checked just above").variant = None;
        }
        let total = provider.models.len();

        self.commit(candidate)?;
        Ok((self.view()?, FetchSummary { total, ..summary }))
    }

    /// The saved default model and the settings it comes from. `Ok(None)` when there is none in normal operation, so
    /// the environment settings apply. An error while settings.json is broken, and in last-good mode when the last
    /// working copy has no default: nothing falls back to the environment or the mock then.
    fn saved_default(&self) -> Result<Option<(Settings, ModelRef)>, String> {
        let State { settings, mode, .. } = self.snapshot();
        match mode {
            Mode::Ok => Ok(settings.default_model.clone().map(|default| (settings, default))),
            Mode::Broken { reason } => Err(format!(
                "the AI settings cannot be used: {reason}. Fix settings.json, or revert to or use the last working \
                 settings in the AI settings."
            )),
            Mode::LastGood { .. } => match settings.default_model.clone() {
                Some(default) => Ok(Some((settings, default))),
                None => Err(LAST_GOOD_NO_DEFAULT.to_string()),
            },
        }
    }

    /// The default model with its secrets resolved. `None` when no default is set. An error when the default cannot be
    /// used (no key, a missing header value) or the settings file is broken.
    pub fn active(&self) -> Result<Option<ActiveLlm>, String> {
        let Some((settings, default)) = self.saved_default()? else {
            return Ok(None);
        };
        let resolved = resolve::resolve(&settings, &self.secrets, &default)
            .map_err(|e| format!("the default model cannot be used: {e}. Check the AI settings."))?;
        let provider = settings.provider(&default.provider_id).map(|p| p.name.clone()).unwrap_or_default();
        Ok(Some(ActiveLlm { settings: resolved, provider, model: default.model_id, variant: default.variant }))
    }

    /// The default model's provider name, model id and variant, for the status line. `Ok(None)` when there is no saved
    /// default, so the environment settings apply. An error when the saved default cannot be used. Never reads the
    /// secret store.
    pub fn default_label(&self) -> Result<Option<(String, String, Option<String>)>, String> {
        let Some((settings, default)) = self.saved_default()? else {
            return Ok(None);
        };
        Ok(settings
            .provider(&default.provider_id)
            .map(|p| (p.name.clone(), default.model_id, default.variant)))
    }

    /// The settings for one model, for a connection test. Works before the model is the default.
    pub fn test_plan(&self, provider_id: &str, model_id: &str) -> Result<LlmSettings, String> {
        let settings = self.usable()?;
        let model = ModelRef { provider_id: provider_id.to_string(), model_id: model_id.to_string(), variant: None };
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
        // The connection test runs the model as configured, variant overlay included.
        extra: settings.extra.clone(),
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

/// Keeps `bytes`, a settings file that has just checked out, as the last working copy. A failure is logged and the
/// launch goes on: the previous copy, if any, is still there.
fn copy_last_good(config_dir: &Path, bytes: &[u8]) {
    if let Err(e) = write_private_atomic(&last_good_path(config_dir), bytes) {
        eprintln!("watt: the last working AI settings were not kept ({e})");
    }
}

/// Whether the last working copy exists and checks out.
fn last_good_ok(config_dir: &Path) -> bool {
    matches!(read_checked(&last_good_path(config_dir)), Ok(Some(_)))
}

/// `settings.json.broken-<unix time>`, with `-<n>` added while that name is taken. Never an existing file.
fn aside_path(dir: &Path) -> PathBuf {
    let stamp = now_secs();
    let mut path = dir.join(format!("{SETTINGS_FILE}.broken-{stamp}"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{SETTINGS_FILE}.broken-{stamp}-{n}"));
        n += 1;
    }
    path
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
    use super::super::{
        AgentRequests, ApprovalMode, ApprovalSettings, HeaderEntry, IntegrationSettings, Model, ModelVariant, Nested,
        VariantSource, last_good_path, load, save as save_file,
    };
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
        Model { id: id.into(), context_limit: None, provider_context: None, vision: None, source, variants: Vec::new() }
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
        ModelRef { provider_id: provider_id.into(), model_id: model_id.into(), variant: None }
    }

    fn read_file(dir: &TempDir) -> Vec<u8> {
        fs::read(settings_path(dir.path())).unwrap()
    }

    #[test]
    fn secrets_live_only_in_the_store_and_never_in_the_file() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![
            HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true, previous_name: None },
            HeaderEntry { name: "X-Session".into(), value: "{session_id}".into(), secret: false, previous_name: None },
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
        p.headers = vec![HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true, previous_name: None }];
        let mut first = with_key(p.clone(), KEY);
        first.header_secrets.insert("X-Key".into(), HEADER.into());
        f.service.save_provider(first).unwrap();

        // The header is now plain, and the key is removed.
        let mut plain = p;
        plain.headers = vec![HeaderEntry {
            name: "X-Key".into(),
            value: "visible".into(),
            secret: false,
            previous_name: None,
        }];
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
        p.headers = vec![HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true, previous_name: None }];
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
        p.headers = vec![HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true, previous_name: None }];
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
    fn approval_rules_are_saved_validated_and_read_back() {
        let f = fixture();
        assert_eq!(f.service.approval_settings(), Ok(ApprovalSettings::default()), "no file: manual, no rules");

        let bad = ApprovalSettings { mode: ApprovalMode::Regex, allow: vec!["a(".into()], ..ApprovalSettings::default() };
        let err = f.service.set_approval(bad).unwrap_err();
        assert!(err.contains("approval allow rule 1"), "{err}");
        assert_eq!(f.service.approval_settings(), Ok(ApprovalSettings::default()), "a refused change saved nothing");

        let regex = ApprovalSettings { mode: ApprovalMode::Regex, allow: vec!["git status".into()], ..ApprovalSettings::default() };
        let view = f.service.set_approval(regex.clone()).unwrap();
        assert_eq!(view.settings.approval, regex);
        assert_eq!(f.service.approval_settings(), Ok(regex.clone()));
        assert_eq!(service_on(&f.dir, &f.store).approval_settings(), Ok(regex), "a new service reads the saved rules");
    }

    #[test]
    fn integration_and_nested_settings_are_saved_and_read_back() {
        let f = fixture();
        assert_eq!(f.service.integration_settings(), Ok(IntegrationSettings::default()), "no file: ask");
        let auto = IntegrationSettings { agent_requests: AgentRequests::Auto };
        let view = f.service.set_integration(auto).unwrap();
        assert_eq!(view.settings.integration, auto);
        assert_eq!(service_on(&f.dir, &f.store).integration_settings(), Ok(auto), "a new service reads it back");

        let nested = ApprovalSettings { nested: Nested::SameAsLocal, ..ApprovalSettings::default() };
        f.service.set_approval(nested.clone()).unwrap();
        assert_eq!(f.service.approval_settings(), Ok(nested));
    }

    #[test]
    fn limits_are_saved_read_back_and_refused_when_out_of_range() {
        let f = fixture();
        assert_eq!(f.service.limits_settings(), Ok(LimitsSettings::default()), "no file: the defaults");
        let saved = LimitsSettings { max_model_calls: 75, model_calls_unlimited: false, max_poll_calls: 120, max_commands_per_turn: 40, max_wait_minutes: 45 };
        let view = f.service.set_limits(saved).unwrap();
        assert_eq!(view.settings.limits, saved);
        assert_eq!(service_on(&f.dir, &f.store).limits_settings(), Ok(saved), "a new service reads it back");

        let bad = LimitsSettings { max_poll_calls: 9, ..saved };
        let err = f.service.set_limits(bad).unwrap_err();
        assert!(err.contains("polling calls per turn must be 10 to 500"), "{err}");
        assert_eq!(f.service.limits_settings(), Ok(saved), "a refused change saved nothing");
    }

    #[test]
    fn last_good_mode_runs_the_saved_limits_and_refuses_changes() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();
        let saved = LimitsSettings { model_calls_unlimited: true, ..LimitsSettings::default() };
        f.service.set_limits(saved).unwrap();
        let _launch = service_on(&f.dir, &f.store);
        break_settings(&f);

        let service = service_on(&f.dir, &f.store);
        assert!(service.limits_settings().is_err(), "a broken file runs no limits of its own");
        service.use_last_good().unwrap();
        assert_eq!(service.limits_settings(), Ok(saved), "last-good mode runs the last working limits");
        assert!(service.set_limits(LimitsSettings::default()).unwrap_err().contains("broken"));
    }

    #[test]
    fn last_good_mode_runs_the_saved_approval_rules_and_refuses_changes() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();
        let regex = ApprovalSettings { mode: ApprovalMode::Regex, allow: vec!["echo .*".into()], ..ApprovalSettings::default() };
        f.service.set_approval(regex.clone()).unwrap();
        let _launch = service_on(&f.dir, &f.store);
        break_settings(&f);

        let service = service_on(&f.dir, &f.store);
        assert!(service.approval_settings().is_err(), "a broken file runs no approval rules");
        service.use_last_good().unwrap();
        assert_eq!(service.approval_settings(), Ok(regex), "last-good mode runs the last working rules");
        assert!(service.set_approval(ApprovalSettings::default()).unwrap_err().contains("broken"));
    }

    #[test]
    fn default_label_is_read_without_touching_the_store() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();

        let service = service_on(&f.dir, &f.store);
        let loads_before = f.store.loads();
        assert_eq!(service.default_label(), Ok(Some(("acme name".to_string(), "big".to_string(), None))));
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
        assert_eq!(active.variant, None, "no variant is selected");
        assert_eq!(active.settings.api_key, KEY);
        assert_eq!(active.settings.chat_url, "https://api.acme.test/v1/chat/completions");
    }

    #[test]
    fn test_plan_works_before_the_model_is_the_default() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        let plan = f.service.test_plan("acme", "big").unwrap();
        assert_eq!(plan.model, "big");
        assert_eq!(f.service.default_label(), Ok(None));
        assert!(f.service.test_plan("acme", "missing").unwrap_err().contains("does not list"));
    }

    const BROKEN: &[u8] = b"{ not settings";

    fn dir_names(dir: &TempDir) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn aside_files(dir: &TempDir) -> Vec<String> {
        dir_names(dir).into_iter().filter(|n| n.starts_with("settings.json.broken-")).collect()
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Saves one provider with a key and a default model, then launches once, so a last-good copy is kept. Returns the
    /// bytes of the valid settings.json.
    fn launched_with_good_settings(f: &Fixture) -> Vec<u8> {
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        f.service.set_default_model(Some(model_ref("acme", "big"))).unwrap();
        let _launch = service_on(&f.dir, &f.store);
        read_file(&f.dir)
    }

    fn break_settings(f: &Fixture) {
        fs::create_dir_all(f.dir.path()).unwrap();
        fs::write(settings_path(f.dir.path()), BROKEN).unwrap();
    }

    #[test]
    fn a_launch_that_finds_a_valid_file_keeps_a_hidden_last_good_copy() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        let settings_bytes = read_file(&f.dir);
        assert!(!last_good_path(f.dir.path()).exists(), "no copy is made before a launch");

        let service = service_on(&f.dir, &f.store);
        assert_eq!(fs::read(last_good_path(f.dir.path())).unwrap(), settings_bytes);
        assert_eq!(mode_of(&last_good_path(f.dir.path())), 0o600);
        assert!(last_good_path(f.dir.path()).file_name().unwrap().to_string_lossy().starts_with('.'));
        let view = service.view().unwrap();
        assert_eq!(view.settings_state.kind, "ok");
        assert_eq!(view.load_error, None);
        assert!(view.settings_state.last_good_available);
    }

    #[test]
    fn a_launch_without_a_settings_file_keeps_nothing() {
        let f = fixture();
        assert!(!last_good_path(f.dir.path()).exists());
        let view = f.service.view().unwrap();
        assert_eq!(view.settings_state.kind, "ok");
        assert!(!view.settings_state.last_good_available);
    }

    #[test]
    fn a_broken_launch_never_overwrites_the_last_good_copy() {
        let f = fixture();
        let good = launched_with_good_settings(&f);
        break_settings(&f);

        let service = service_on(&f.dir, &f.store);
        assert_eq!(fs::read(last_good_path(f.dir.path())).unwrap(), good, "the copy was overwritten");
        let view = service.view().unwrap();
        assert_eq!(view.settings_state.kind, "broken");
        assert!(view.settings_state.last_good_available);
        assert!(view.settings_state.reason.is_some());
    }

    #[test]
    fn a_broken_file_refuses_reads_and_changes_and_is_left_untouched() {
        let f = fixture();
        launched_with_good_settings(&f);
        break_settings(&f);
        let names_before = dir_names(&f.dir);
        let blob_before = f.store.stored();
        let service = service_on(&f.dir, &f.store);

        let view = service.view().unwrap();
        assert_eq!(view.settings_state.kind, "broken");
        assert!(view.load_error.as_deref().unwrap().contains("settings.json"));
        assert!(view.settings.providers.is_empty());
        assert_eq!(view.settings_state.path, settings_path(f.dir.path()).display().to_string());

        let err = service.active().unwrap_err();
        assert!(err.contains("cannot be used") && err.contains("revert"), "{err}");
        assert!(service.default_label().is_err());
        assert!(service.test_plan("acme", "big").is_err());
        assert!(service.save_provider(with_key(provider("acme"), OTHER_KEY)).unwrap_err().contains("unchanged"));
        assert!(service.set_default_model(None).unwrap_err().contains("unchanged"));
        assert_eq!(read_file(&f.dir), BROKEN, "the broken file was changed");
        assert_eq!(dir_names(&f.dir), names_before);
        assert_eq!(f.store.stored(), blob_before, "the store was touched");
    }

    #[test]
    fn revert_keeps_the_broken_file_aside_and_restores_the_last_working_settings() {
        let f = fixture();
        let good = launched_with_good_settings(&f);
        break_settings(&f);
        let service = service_on(&f.dir, &f.store);
        assert_eq!(service.view().unwrap().settings_state.kind, "broken");

        let view = service.revert_to_last_good().unwrap();
        assert_eq!(view.settings_state.kind, "ok");
        assert_eq!(view.load_error, None);
        assert_eq!(view.settings.default_model, Some(model_ref("acme", "big")));
        assert!(view.secrets["acme"].api_key, "the secrets stay in the store");
        assert_eq!(read_file(&f.dir), good, "settings.json is not the last working copy");
        assert!(service.active().unwrap().is_some(), "the restored default cannot run");

        let asides = aside_files(&f.dir);
        assert_eq!(asides.len(), 1, "{asides:?}");
        let aside = f.dir.path().join(&asides[0]);
        assert_eq!(fs::read(&aside).unwrap(), BROKEN, "the broken text was not kept");
        assert_eq!(mode_of(&aside), 0o600);
        assert_eq!(fs::read(last_good_path(f.dir.path())).unwrap(), good, "the last-good copy changed");
    }

    #[test]
    fn use_last_good_runs_the_copy_for_the_session_and_leaves_the_broken_file_byte_for_byte() {
        let f = fixture();
        launched_with_good_settings(&f);
        break_settings(&f);
        let names_before = dir_names(&f.dir);
        let blob_before = f.store.stored();
        let service = service_on(&f.dir, &f.store);

        let view = service.use_last_good().unwrap();
        assert_eq!(view.settings_state.kind, "last_good");
        assert!(view.load_error.is_some());
        assert_eq!(view.settings.default_model, Some(model_ref("acme", "big")));
        assert_eq!(service.default_label(), Ok(Some(("acme name".to_string(), "big".to_string(), None))));
        assert!(service.active().unwrap().is_some(), "the last working copy did not run");

        let refused = "settings.json is broken; fix or revert it first";
        assert_eq!(service.save_provider(with_key(provider("acme"), OTHER_KEY)).unwrap_err(), refused);
        assert_eq!(service.set_default_model(None).unwrap_err(), refused);
        assert_eq!(service.delete_provider("acme").unwrap_err(), refused);
        assert_eq!(service.set_global_context_limit(Some(10)).unwrap_err(), refused);

        assert_eq!(read_file(&f.dir), BROKEN, "settings.json is not byte-for-byte the broken file");
        assert_eq!(dir_names(&f.dir), names_before, "a file was added or renamed");
        assert_eq!(f.store.stored(), blob_before, "the store was written");
    }

    #[test]
    fn a_last_good_session_with_no_default_refuses_instead_of_falling_back() {
        let f = fixture();
        f.service.save_provider(with_key(provider("acme"), KEY)).unwrap();
        let _launch = service_on(&f.dir, &f.store);
        break_settings(&f);
        let service = service_on(&f.dir, &f.store);
        service.use_last_good().unwrap();
        let err = service.active().unwrap_err();
        assert!(err.contains("no default model"), "{err}");
        assert!(service.default_label().is_err());
    }

    #[test]
    fn recovery_without_a_last_good_copy_is_an_error_and_the_state_stays_broken() {
        let f = fixture();
        break_settings(&f);
        let service = service_on(&f.dir, &f.store);

        let err = service.use_last_good().unwrap_err();
        assert!(err.contains("no last working settings"), "{err}");
        let err = service.revert_to_last_good().unwrap_err();
        assert!(err.contains("no last working settings"), "{err}");
        let view = service.view().unwrap();
        assert_eq!(view.settings_state.kind, "broken");
        assert!(!view.settings_state.last_good_available);
        assert_eq!(read_file(&f.dir), BROKEN);
        assert!(aside_files(&f.dir).is_empty(), "something was moved aside");
    }

    #[test]
    fn reload_returns_to_ok_after_a_hand_fix_and_a_last_good_session_waits_for_it() {
        let f = fixture();
        let good = launched_with_good_settings(&f);
        break_settings(&f);
        let service = service_on(&f.dir, &f.store);
        service.use_last_good().unwrap();

        // A last-good session does not read the file, so a hand fix is seen only by reload.
        fs::write(settings_path(f.dir.path()), &good).unwrap();
        assert_eq!(service.view().unwrap().settings_state.kind, "last_good");
        let view = service.reload().unwrap();
        assert_eq!(view.settings_state.kind, "ok");
        assert_eq!(view.load_error, None);
        service.save_provider(with_key(provider("acme"), OTHER_KEY)).unwrap();
    }

    #[test]
    fn a_reload_that_still_finds_the_file_broken_keeps_the_last_good_session() {
        let f = fixture();
        launched_with_good_settings(&f);
        break_settings(&f);
        let service = service_on(&f.dir, &f.store);
        service.use_last_good().unwrap();

        fs::write(settings_path(f.dir.path()), b"{ still broken").unwrap();
        let view = service.reload().unwrap();
        assert_eq!(view.settings_state.kind, "last_good");
        assert!(view.load_error.unwrap().contains("settings.json"));
        assert_eq!(view.settings.default_model, Some(model_ref("acme", "big")));
    }

    #[test]
    fn a_recovery_reads_the_file_first_so_a_file_fixed_by_hand_is_not_replaced() {
        let f = fixture();
        let good = launched_with_good_settings(&f);
        break_settings(&f);
        let service = service_on(&f.dir, &f.store);
        service.use_last_good().unwrap();
        fs::write(settings_path(f.dir.path()), &good).unwrap();

        let err = service.revert_to_last_good().unwrap_err();
        assert!(err.contains("not broken"), "{err}");
        assert_eq!(read_file(&f.dir), good);
        assert!(aside_files(&f.dir).is_empty());
        assert_eq!(service.view().unwrap().settings_state.kind, "ok");
    }

    fn secret_header_provider(id: &str, header: &str) -> Provider {
        let mut p = provider(id);
        p.headers = vec![HeaderEntry { name: header.into(), value: String::new(), secret: true, previous_name: None }];
        p
    }

    /// Saves `acme` with a secret header `X-Key` holding HEADER, and the key.
    fn with_stored_header(f: &Fixture) {
        let mut first = with_key(secret_header_provider("acme", "X-Key"), KEY);
        first.header_secrets.insert("X-Key".into(), HEADER.into());
        f.service.save_provider(first).unwrap();
    }

    fn renamed_to(name: &str, previous: &str) -> SaveProviderInput {
        let mut save = input(secret_header_provider("acme", name));
        save.provider.headers[0].previous_name = Some(previous.into());
        save
    }

    #[test]
    fn a_renamed_secret_header_with_no_typed_value_keeps_its_stored_value() {
        let f = fixture();
        with_stored_header(&f);
        let view = f.service.save_provider(renamed_to("X-Token", "X-Key")).unwrap();
        assert_eq!(view.secrets["acme"].headers, vec!["X-Token".to_string()]);
        let blob = f.store.stored().unwrap();
        assert!(blob.contains(HEADER), "the value did not move to the new name");
        assert!(!blob.contains("header.x-key"), "the old name was kept: {blob}");
        let plan = f.service.provider_for_fetch("acme").unwrap();
        assert_eq!(plan.headers, vec![("X-Token".to_string(), HEADER.to_string())]);
        let file = String::from_utf8(read_file(&f.dir)).unwrap();
        assert!(!file.contains("previous_name"), "the input-only field was saved");
    }

    #[test]
    fn a_typed_value_wins_over_a_carried_one() {
        let f = fixture();
        with_stored_header(&f);
        let mut save = renamed_to("X-Token", "X-Key");
        save.header_secrets.insert("X-Token".into(), OTHER_KEY.into());
        f.service.save_provider(save).unwrap();
        let blob = f.store.stored().unwrap();
        assert!(blob.contains(OTHER_KEY));
        assert!(!blob.contains(HEADER), "the carried value survived a typed one: {blob}");
        assert!(!blob.contains("header.x-key"));
    }

    #[test]
    fn a_renamed_secret_header_with_nothing_stored_under_the_old_name_carries_nothing() {
        let f = fixture();
        f.service.save_provider(input(secret_header_provider("acme", "X-Key"))).unwrap();
        let view = f.service.save_provider(renamed_to("X-Token", "X-Key")).unwrap();
        assert!(view.secrets["acme"].headers.is_empty());
    }

    #[test]
    fn a_case_only_rename_keeps_the_stored_value() {
        let f = fixture();
        with_stored_header(&f);
        let view = f.service.save_provider(renamed_to("x-key", "X-Key")).unwrap();
        assert_eq!(view.secrets["acme"].headers, vec!["x-key".to_string()]);
        let plan = f.service.provider_for_fetch("acme").unwrap();
        assert_eq!(plan.headers, vec![("x-key".to_string(), HEADER.to_string())]);
    }

    #[test]
    fn view_reports_presence_only() {
        let f = fixture();
        let mut p = provider("acme");
        p.headers = vec![
            HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true, previous_name: None },
            HeaderEntry { name: "X-Other".into(), value: String::new(), secret: true, previous_name: None },
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
            FetchedModel { id: "kept".into(), context: Some(2_000), vision: None, variants: Vec::new() },
            FetchedModel { id: "fresh".into(), context: None, vision: None, variants: Vec::new() },
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

    fn auto_variant(id: &str) -> ModelVariant {
        ModelVariant {
            id: id.into(),
            label: format!("{id} label"),
            body: serde_json::from_str(r#"{"reasoning":{"effort":"high"}}"#).unwrap(),
            source: VariantSource::Auto,
        }
    }

    fn manual_variant(id: &str) -> ModelVariant {
        ModelVariant { source: VariantSource::Manual, ..auto_variant(id) }
    }

    /// A refresh that drops the default's auto variant clears the variant; the default stays on the base model.
    #[test]
    fn a_fetch_clears_a_default_variant_that_disappeared() {
        let f = fixture();
        let mut p = provider("acme");
        p.models = vec![Model {
            id: "big".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: vec![manual_variant("mine"), auto_variant("fast")],
        }];
        f.service.save_provider(with_key(p, KEY)).unwrap();
        f.service
            .set_default_model(Some(ModelRef {
                provider_id: "acme".into(),
                model_id: "big".into(),
                variant: Some("fast".into()),
            }))
            .unwrap();
        assert_eq!(
            f.service.default_label(),
            Ok(Some(("acme name".to_string(), "big".to_string(), Some("fast".to_string()))))
        );

        let fetched = vec![FetchedModel { id: "big".into(), context: None, vision: None, variants: Vec::new() }];
        let (view, _) = f.service.apply_fetched("acme", fetched).unwrap();
        assert_eq!(
            view.settings.default_model,
            Some(model_ref("acme", "big")),
            "the default falls back to the base model"
        );
        let kept = view.settings.providers[0].model("big").unwrap();
        assert_eq!(kept.variants, vec![manual_variant("mine")], "the manual variant is kept");
        assert_eq!(
            f.service.default_label(),
            Ok(Some(("acme name".to_string(), "big".to_string(), None)))
        );
        assert!(f.service.active().unwrap().is_some(), "the cleared default still runs");
    }

    /// A manual default variant survives a fetch whose entry lists no efforts at all.
    #[test]
    fn a_manual_default_variant_survives_an_empty_auto_list() {
        let f = fixture();
        let mut p = provider("acme");
        p.models = vec![Model {
            id: "big".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: vec![manual_variant("mine"), auto_variant("fast")],
        }];
        f.service.save_provider(with_key(p, KEY)).unwrap();
        f.service
            .set_default_model(Some(ModelRef {
                provider_id: "acme".into(),
                model_id: "big".into(),
                variant: Some("mine".into()),
            }))
            .unwrap();

        let fetched = vec![FetchedModel { id: "big".into(), context: None, vision: None, variants: Vec::new() }];
        let (view, _) = f.service.apply_fetched("acme", fetched).unwrap();
        assert_eq!(view.settings.providers[0].model("big").unwrap().variants, vec![manual_variant("mine")]);
        assert_eq!(view.settings.default_model.as_ref().unwrap().variant, Some("mine".to_string()));
    }

    /// A removed default model is re-inserted whole, variants included, so its variant still checks out.
    #[test]
    fn a_removed_default_keeps_its_variants_when_it_is_reinserted() {
        let f = fixture();
        let mut p = provider("acme");
        p.models = vec![Model {
            id: "big".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: vec![auto_variant("fast")],
        }];
        f.service.save_provider(with_key(p, KEY)).unwrap();
        f.service
            .set_default_model(Some(ModelRef {
                provider_id: "acme".into(),
                model_id: "big".into(),
                variant: Some("fast".into()),
            }))
            .unwrap();

        let fetched = vec![FetchedModel { id: "other".into(), context: None, vision: None, variants: Vec::new() }];
        let (view, _) = f.service.apply_fetched("acme", fetched).unwrap();
        let reinserted = view.settings.providers[0].model("big").unwrap();
        assert_eq!(reinserted.variants, vec![auto_variant("fast")]);
        assert_eq!(view.settings.default_model.as_ref().unwrap().variant, Some("fast".to_string()));
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
            HeaderEntry { name: "X-Session".into(), value: "{session_id}".into(), secret: false, previous_name: None },
            HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true, previous_name: None },
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
            extra: None,
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

    /// Serves one request like `one_shot_server` and keeps the raw request it received, so a test can read the body.
    fn one_shot_capture(body: &str) -> (String, Arc<std::sync::Mutex<Vec<u8>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let seen: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = seen.clone();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                data.extend_from_slice(&buf[..n]);
                let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break;
                }
            }
            *kept.lock().unwrap_or_else(|e| e.into_inner()) = data;
            let _ = sock.write_all(response.as_bytes());
        });
        (format!("http://{addr}/v1/chat/completions"), seen)
    }

    /// The connection test sends the resolved variant overlay with the probe request.
    #[tokio::test]
    async fn the_connection_test_sends_the_variant_overlay() {
        let (url, seen) =
            one_shot_capture(r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#);
        let client = llm::http_client().unwrap();
        let mut settings = llm_settings(url);
        settings.extra =
            Some(serde_json::from_str(r#"{"reasoning":{"effort":"high"},"temperature":0.2}"#).unwrap());
        let result = test_model(&client, &settings).await;
        assert!(result.ok, "{result:?}");

        let raw = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let text = String::from_utf8_lossy(&raw);
        let body: serde_json::Value =
            serde_json::from_str(text.split("\r\n\r\n").nth(1).expect("the request has a body"))
                .expect("the probe body is JSON");
        assert_eq!(body["reasoning"], serde_json::json!({"effort": "high"}), "{body}");
        assert_eq!(body["temperature"], 0.2, "{body}");
        assert_eq!(body["model"], "big", "{body}");
    }
}
