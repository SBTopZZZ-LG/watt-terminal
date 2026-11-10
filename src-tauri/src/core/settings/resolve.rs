//! Turns a saved provider and model into the `LlmSettings` the chat loop runs on.
//!
//! `resolve` reads the API key and the secret header values from the `Secrets` store and fills them in. The result
//! holds secrets, so it goes straight to `LlmAgent::new` (which hands them to the redactor). Nothing here logs it.
//! Errors name the provider, model or header, never a value.

use std::fmt;
use std::io;

use super::secrets::{SecretError, Secrets, api_key_name, header_secret_name};
use super::{ModelRef, Provider, Settings};
use crate::core::chat::agent_loop::{LlmSettings, new_conversation_id};

const SESSION_PLACEHOLDER: &str = "{session_id}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    UnknownProvider { provider: String },
    UnknownModel { provider: String, model: String },
    /// No key is stored, and the chat URL is not a plain-http local address that may run without one.
    MissingApiKey { provider: String },
    MissingHeaderSecret { provider: String, header: String },
    Secret(SecretError),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::UnknownProvider { provider } => write!(f, "no provider {provider:?} is saved"),
            ResolveError::UnknownModel { provider, model } => {
                write!(f, "provider {provider:?} does not list model {model:?}")
            }
            ResolveError::MissingApiKey { provider } => {
                write!(f, "provider {provider:?} needs an API key, and none is stored")
            }
            ResolveError::MissingHeaderSecret { provider, header } => {
                write!(f, "provider {provider:?} needs the value of secret header {header:?}, and none is stored")
            }
            ResolveError::Secret(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// The settings for one model of one provider, with the stored secrets filled in. Header values keep their
/// `{session_id}` placeholder; `LlmSettings::resolved_headers` fills it per conversation.
pub fn resolve(settings: &Settings, secrets: &Secrets, model: &ModelRef) -> Result<LlmSettings, ResolveError> {
    let provider = settings
        .provider(&model.provider_id)
        .ok_or_else(|| ResolveError::UnknownProvider { provider: model.provider_id.clone() })?;
    if provider.model(&model.model_id).is_none() {
        return Err(ResolveError::UnknownModel { provider: provider.id.clone(), model: model.model_id.clone() });
    }

    Ok(LlmSettings {
        chat_url: provider.chat_url.clone(),
        api_key: api_key_for(provider, secrets)?,
        model: model.model_id.clone(),
        headers: headers_for(provider, secrets)?,
        stream: provider.stream,
        max_tokens: provider.max_tokens,
        context_limit: provider.effective_context_limit(&model.model_id, settings.global_context_limit),
    })
}

/// The provider's API key. An empty string is allowed only for a plain-http local chat URL.
pub fn api_key_for(provider: &Provider, secrets: &Secrets) -> Result<String, ResolveError> {
    match stored(secrets, &api_key_name(&provider.id))? {
        Some(key) => Ok(key),
        None if allows_empty_key(&provider.chat_url) => Ok(String::new()),
        None => Err(ResolveError::MissingApiKey { provider: provider.id.clone() }),
    }
}

/// The provider's header pairs in order: plain values as saved, secret values from the store. `{session_id}` is kept.
pub fn headers_for(provider: &Provider, secrets: &Secrets) -> Result<Vec<(String, String)>, ResolveError> {
    let mut headers = Vec::with_capacity(provider.headers.len());
    for header in &provider.headers {
        let value = if header.secret {
            stored(secrets, &header_secret_name(&provider.id, &header.name))?.ok_or_else(|| {
                ResolveError::MissingHeaderSecret { provider: provider.id.clone(), header: header.name.clone() }
            })?
        } else {
            header.value.clone()
        };
        headers.push((header.name.clone(), value));
    }
    Ok(headers)
}

/// Header pairs for a model-list request. `{session_id}` becomes a fresh conversation-style id. Other values are
/// copied as they are.
pub fn resolve_headers_for_fetch(headers: &[(String, String)]) -> io::Result<Vec<(String, String)>> {
    let session = new_conversation_id()?;
    Ok(headers.iter().map(|(name, value)| (name.clone(), value.replace(SESSION_PLACEHOLDER, &session))).collect())
}

/// A stored value, or `None` when it is absent or empty (an empty value is never a usable secret).
fn stored(secrets: &Secrets, name: &str) -> Result<Option<String>, ResolveError> {
    Ok(secrets.get(name).map_err(ResolveError::Secret)?.filter(|value| !value.is_empty()))
}

/// Plain http to localhost, 127.0.0.1 or [::1] may run without a key. Any other URL needs one.
fn allows_empty_key(chat_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(chat_url) else {
        return false;
    };
    url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::super::secrets::MemoryStore;
    use super::super::{HeaderEntry, Model, ModelSource, Provider, Settings};
    use super::*;
    use crate::core::chat::history::History;
    use crate::core::llm::{TurnCancel, http_client};

    const KEY: &str = "DUMMY-api-key-0123456789abcdef";
    const HEADER_SECRET: &str = "DUMMY-header-secret-fedcba9876543210";

    fn provider(chat_url: &str) -> Provider {
        Provider {
            id: "acme".into(),
            name: "Acme".into(),
            chat_url: chat_url.into(),
            models_url: None,
            headers: vec![
                HeaderEntry { name: "X-Session".into(), value: "{session_id}".into(), secret: false },
                HeaderEntry { name: "X-Key".into(), value: String::new(), secret: true },
            ],
            stream: false,
            max_tokens: Some(256),
            models: vec![Model {
                id: "big".into(),
                context_limit: Some(50_000),
                provider_context: Some(100_000),
                source: ModelSource::Fetched,
            }],
        }
    }

    fn settings_for(chat_url: &str) -> Settings {
        Settings {
            providers: vec![provider(chat_url)],
            default_model: None,
            global_context_limit: Some(40_000),
            ..Settings::default()
        }
    }

    fn model_ref(provider_id: &str, model_id: &str) -> ModelRef {
        ModelRef { provider_id: provider_id.into(), model_id: model_id.into() }
    }

    fn secrets_with(pairs: &[(&str, &str)]) -> Secrets {
        let map: BTreeMap<&str, &str> = pairs.iter().copied().collect();
        let blob = serde_json::to_string(&map).unwrap();
        Secrets::new(Box::new(MemoryStore::with_blob(&blob)))
    }

    fn full_secrets() -> Secrets {
        secrets_with(&[("provider.acme.api_key", KEY), ("provider.acme.header.x-key", HEADER_SECRET)])
    }

    const REMOTE: &str = "https://api.acme.test/v1/chat/completions";

    #[test]
    fn resolves_key_headers_and_limits() {
        let resolved = resolve(&settings_for(REMOTE), &full_secrets(), &model_ref("acme", "big")).unwrap();
        assert_eq!(resolved.chat_url, REMOTE);
        assert_eq!(resolved.api_key, KEY);
        assert_eq!(resolved.model, "big");
        assert_eq!(
            resolved.headers,
            vec![
                ("X-Session".to_string(), "{session_id}".to_string()),
                ("X-Key".to_string(), HEADER_SECRET.to_string()),
            ]
        );
        assert_eq!(resolved.resolved_headers("ses_abc")[0].1, "ses_abc");
        assert!(!resolved.stream);
        assert_eq!(resolved.max_tokens, Some(256));
        // Global 40k, override 50k, provider 100k: the smallest set value wins.
        assert_eq!(resolved.context_limit, Some(40_000));

        let mut no_global = settings_for(REMOTE);
        no_global.global_context_limit = None;
        let resolved = resolve(&no_global, &full_secrets(), &model_ref("acme", "big")).unwrap();
        assert_eq!(resolved.context_limit, Some(50_000));
    }

    #[test]
    fn unknown_provider_and_model_are_named() {
        let secrets = full_secrets();
        assert_eq!(
            resolve(&settings_for(REMOTE), &secrets, &model_ref("nope", "big")).unwrap_err(),
            ResolveError::UnknownProvider { provider: "nope".into() }
        );
        assert_eq!(
            resolve(&settings_for(REMOTE), &secrets, &model_ref("acme", "small")).unwrap_err(),
            ResolveError::UnknownModel { provider: "acme".into(), model: "small".into() }
        );
    }

    #[test]
    fn a_missing_or_empty_api_key_is_an_error_for_remote_urls() {
        let settings = settings_for(REMOTE);
        let mr = model_ref("acme", "big");
        let missing = secrets_with(&[("provider.acme.header.x-key", HEADER_SECRET)]);
        let empty = secrets_with(&[("provider.acme.api_key", ""), ("provider.acme.header.x-key", HEADER_SECRET)]);
        for secrets in [missing, empty] {
            let err = resolve(&settings, &secrets, &mr).unwrap_err();
            assert_eq!(err, ResolveError::MissingApiKey { provider: "acme".into() });
            assert!(!err.to_string().contains(HEADER_SECRET));
        }
        // Plain http to a remote host is not a local address, so it needs a key too.
        let http_remote = settings_for("http://api.acme.test/v1/chat/completions");
        let no_key = secrets_with(&[("provider.acme.header.x-key", HEADER_SECRET)]);
        assert!(matches!(resolve(&http_remote, &no_key, &mr), Err(ResolveError::MissingApiKey { .. })));
    }

    #[test]
    fn local_plain_http_may_run_without_a_key() {
        let mr = model_ref("acme", "big");
        let secrets = secrets_with(&[("provider.acme.header.x-key", HEADER_SECRET)]);
        for url in [
            "http://localhost:11434/v1/chat/completions",
            "http://127.0.0.1:8080/v1/chat/completions",
            "http://[::1]:8080/v1/chat/completions",
        ] {
            let resolved = resolve(&settings_for(url), &secrets, &mr).unwrap();
            assert_eq!(resolved.api_key, "", "{url}");
        }
        // Local https is not on the list: it needs a key like any other remote URL.
        let https_local = settings_for("https://localhost:8443/v1/chat/completions");
        assert!(matches!(resolve(&https_local, &secrets, &mr), Err(ResolveError::MissingApiKey { .. })));
    }

    #[test]
    fn a_missing_secret_header_value_is_an_error_that_names_the_header() {
        let secrets = secrets_with(&[("provider.acme.api_key", KEY)]);
        let err = resolve(&settings_for(REMOTE), &secrets, &model_ref("acme", "big")).unwrap_err();
        assert_eq!(err, ResolveError::MissingHeaderSecret { provider: "acme".into(), header: "X-Key".into() });
        assert!(!err.to_string().contains(KEY));
    }

    #[test]
    fn a_corrupt_store_is_reported_without_its_contents() {
        let secrets = Secrets::new(Box::new(MemoryStore::with_blob("not json DUMMY-leak-check")));
        let err = resolve(&settings_for(REMOTE), &secrets, &model_ref("acme", "big")).unwrap_err();
        assert_eq!(err, ResolveError::Secret(SecretError::Corrupt));
        assert!(!err.to_string().contains("DUMMY-leak-check"));
    }

    #[test]
    fn secret_header_value_is_redacted_by_the_agent_that_uses_the_settings() {
        let resolved = resolve(&settings_for(REMOTE), &full_secrets(), &model_ref("acme", "big")).unwrap();
        let history = Arc::new(Mutex::new(History::new()));
        let agent = crate::core::chat::agent_loop::LlmAgent::new(
            &http_client().unwrap(),
            resolved,
            "ses_0123456789abcdef0123456789abcdef",
            history,
            Arc::new(TurnCancel::new()),
        );
        let echoed = format!("header {HEADER_SECRET} and key {KEY}");
        let shown = agent.redactor().redact(&echoed);
        assert!(!shown.contains(HEADER_SECRET), "{shown}");
        assert!(!shown.contains(KEY), "{shown}");
    }

    #[test]
    fn fetch_headers_get_a_fresh_session_id_and_keep_the_rest() {
        let templates = vec![
            ("x-opencode-session".to_string(), "{session_id}".to_string()),
            ("X-Title".to_string(), "agent term".to_string()),
        ];
        let first = resolve_headers_for_fetch(&templates).unwrap();
        let second = resolve_headers_for_fetch(&templates).unwrap();
        assert_eq!(first[1], ("X-Title".to_string(), "agent term".to_string()));
        let id = &first[0].1;
        assert_eq!(id.len(), 36, "{id}");
        assert!(id.starts_with("ses_") && id[4..].chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert_ne!(first[0].1, second[0].1);
    }
}
