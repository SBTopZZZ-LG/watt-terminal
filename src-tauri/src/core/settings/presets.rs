//! Built-in provider presets: the starting points the settings UI offers when adding a provider.
//!
//! A preset is only a starting point. The user can change every field before saving, and nothing is stored until
//! they do. Every preset passes `Settings::validate` as it is, so a preset can be saved as soon as a key is given.

use serde::Serialize;

use super::{HeaderEntry, Provider};

#[derive(Debug, Clone, Serialize)]
pub struct ProviderPreset {
    /// Stable name for the UI to refer to the preset by.
    pub key: &'static str,
    pub label: &'static str,
    pub provider: Provider,
    /// Whether the provider needs an API key to work.
    pub needs_key: bool,
    pub note: &'static str,
}

pub fn presets() -> Vec<ProviderPreset> {
    vec![
        ProviderPreset {
            key: "openrouter",
            label: "OpenRouter",
            provider: provider(
                "openrouter",
                "OpenRouter",
                "https://openrouter.ai/api/v1/chat/completions",
                Some("https://openrouter.ai/api/v1/models"),
                Vec::new(),
            ),
            needs_key: true,
            note: "The model list includes each model's context size.",
        },
        ProviderPreset {
            key: "opencode-go",
            label: "OpenCode Go",
            provider: provider(
                "opencode-go",
                "OpenCode Go",
                "https://opencode.ai/inference/go/openai/v1/chat/completions",
                Some("https://opencode.ai/inference/go/v1/models"),
                vec![HeaderEntry {
                    name: "x-opencode-session".into(),
                    value: "{session_id}".into(),
                    secret: false,
                    previous_name: None,
                }],
            ),
            needs_key: true,
            note: "The model list format is not officially documented. The context size is read from limit.context \
                   when the list has it.",
        },
        ProviderPreset {
            key: "custom",
            label: "Custom OpenAI-compatible",
            // A local address, so the blank preset can be saved without a key. Edit the URL for a remote server.
            provider: provider(
                "custom",
                "Custom OpenAI-compatible",
                "http://localhost:8080/v1/chat/completions",
                None,
                Vec::new(),
            ),
            needs_key: false,
            note: "Any server with an OpenAI-compatible chat endpoint. Plain http is accepted only for localhost, \
                   127.0.0.1 and [::1].",
        },
    ]
}

fn provider(
    id: &str,
    name: &str,
    chat_url: &str,
    models_url: Option<&str>,
    headers: Vec<HeaderEntry>,
) -> Provider {
    Provider {
        id: id.into(),
        name: name.into(),
        chat_url: chat_url.into(),
        models_url: models_url.map(str::to_string),
        headers,
        stream: true,
        max_tokens: None,
        models: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Settings;
    use super::super::models::models_url_for;
    use super::*;

    #[test]
    fn every_preset_passes_settings_validation_as_saved() {
        for preset in presets() {
            let settings = Settings { providers: vec![preset.provider.clone()], ..Settings::default() };
            assert_eq!(settings.validate(), Ok(()), "preset {:?} is not valid", preset.key);
        }
    }

    #[test]
    fn preset_keys_and_provider_ids_are_unique() {
        let presets = presets();
        let mut keys: Vec<&str> = presets.iter().map(|p| p.key).collect();
        let mut ids: Vec<&str> = presets.iter().map(|p| p.provider.id.as_str()).collect();
        keys.sort_unstable();
        ids.sort_unstable();
        let (key_count, id_count) = (keys.len(), ids.len());
        keys.dedup();
        ids.dedup();
        assert_eq!((keys.len(), ids.len()), (key_count, id_count));
    }

    #[test]
    fn openrouter_and_opencode_go_have_the_documented_endpoints() {
        let presets = presets();
        let openrouter = presets.iter().find(|p| p.key == "openrouter").unwrap();
        assert_eq!(openrouter.provider.chat_url, "https://openrouter.ai/api/v1/chat/completions");
        assert!(openrouter.provider.headers.is_empty());
        assert!(openrouter.needs_key);
        assert_eq!(
            models_url_for(&openrouter.provider).as_deref(),
            Some("https://openrouter.ai/api/v1/models")
        );

        let go = presets.iter().find(|p| p.key == "opencode-go").unwrap();
        assert_eq!(go.provider.chat_url, "https://opencode.ai/inference/go/openai/v1/chat/completions");
        assert_eq!(
            models_url_for(&go.provider).as_deref(),
            Some("https://opencode.ai/inference/go/v1/models")
        );
        assert_eq!(go.provider.headers.len(), 1);
        let session = &go.provider.headers[0];
        assert_eq!(
            (session.name.as_str(), session.value.as_str(), session.secret),
            ("x-opencode-session", "{session_id}", false)
        );
    }

    #[test]
    fn the_custom_preset_needs_no_key_and_is_plain_local_http() {
        let presets = presets();
        let custom = presets.iter().find(|p| p.key == "custom").unwrap();
        assert!(!custom.needs_key);
        assert!(custom.provider.chat_url.starts_with("http://localhost"));
    }

    #[test]
    fn presets_serialize_for_the_ui() {
        let value = serde_json::to_value(presets()).unwrap();
        let first = &value[0];
        assert_eq!(first["key"], "openrouter");
        assert_eq!(first["needs_key"], true);
        assert_eq!(first["provider"]["stream"], true);
    }
}
